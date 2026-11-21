//! Browse's source filter: every mod in the list comes from GameBanana (the
//! Deadlock Mod Manager catalog) or from one added source, and the user can
//! narrow the list to one of them. The choice is kept in
//! `<settings>/shiv/browse-source` across restarts.

use std::fs;
use std::path::{Path, PathBuf};

use shiv_core::registry::AddedSource;

/// What the list shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum SourceKey {
    #[default]
    All,
    /// The Deadlock Mod Manager catalog (GameBanana mods).
    Catalog,
    /// One added source, by origin and source id (what makes it unique).
    Added { origin: String, source_id: String },
}

impl SourceKey {
    pub(crate) fn of(source: &AddedSource) -> Self {
        Self::Added {
            origin: source.origin.clone(),
            source_id: source.source_id.clone(),
        }
    }

    pub(crate) fn shows_catalog(&self) -> bool {
        matches!(self, Self::All | Self::Catalog)
    }

    pub(crate) fn shows_source(&self, source: &AddedSource) -> bool {
        match self {
            Self::All => true,
            Self::Catalog => false,
            Self::Added { origin, source_id } => {
                *origin == source.origin && *source_id == source.source_id
            }
        }
    }

    fn encode(&self) -> String {
        match self {
            Self::All => "all".into(),
            Self::Catalog => "catalog".into(),
            Self::Added { origin, source_id } => format!("source\t{origin}\t{source_id}"),
        }
    }

    fn decode(text: &str) -> Self {
        let mut parts = text.trim_end_matches(['\r', '\n']).split('\t');
        match (parts.next(), parts.next(), parts.next(), parts.next()) {
            (Some("catalog"), None, ..) => Self::Catalog,
            (Some("source"), Some(origin), Some(source_id), None)
                if !origin.is_empty() && !source_id.is_empty() =>
            {
                Self::Added {
                    origin: origin.into(),
                    source_id: source_id.into(),
                }
            }
            _ => Self::All,
        }
    }
}

/// The dropdown: one label per key, index-aligned, with mod counts.
pub(crate) struct Options {
    pub keys: Vec<SourceKey>,
    pub labels: Vec<String>,
}

impl Options {
    /// `catalog` mods from GameBanana and `(source, mods)` per added source.
    /// A remembered source that is not (yet) loaded stays in the list, so
    /// the choice survives a slow or failed fetch.
    pub(crate) fn new(
        catalog: usize,
        sources: &[(AddedSource, usize)],
        current: &SourceKey,
    ) -> Self {
        let total = catalog + sources.iter().map(|(_, n)| n).sum::<usize>();
        let mut keys = vec![SourceKey::All, SourceKey::Catalog];
        let mut labels = vec![
            format!("All sources ({})", crate::browse::fmt_count(total as u64)),
            format!("GameBanana ({})", crate::browse::fmt_count(catalog as u64)),
        ];
        for (source, n) in sources {
            let key = SourceKey::of(source);
            if keys.contains(&key) {
                continue;
            }
            labels.push(format!(
                "{} ({})",
                source.name,
                crate::browse::fmt_count(*n as u64)
            ));
            keys.push(key);
        }
        if let SourceKey::Added { source_id, .. } = current
            && !keys.contains(current)
        {
            labels.push(format!("{source_id} (not loaded)"));
            keys.push(current.clone());
        }
        Self { keys, labels }
    }

    pub(crate) fn index_of(&self, key: &SourceKey) -> usize {
        self.keys.iter().position(|k| k == key).unwrap_or(0)
    }
}

/// Tests keep the choice in a throwaway folder (read on worker threads,
/// hence not a thread local).
#[cfg(test)]
static TEST_DIR: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

/// Point the saved choice at `dir` (tests).
#[cfg(test)]
pub(crate) fn set_dir_for_test(dir: Option<PathBuf>) {
    *TEST_DIR
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = dir;
}

fn file() -> Option<PathBuf> {
    #[cfg(test)]
    {
        TEST_DIR
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .map(|d| d.join("browse-source"))
    }
    #[cfg(not(test))]
    {
        shiv_core::settings_dir().map(|d| d.join("browse-source"))
    }
}

/// Worker body: the remembered choice; `All` when none was saved or it
/// cannot be read.
pub(crate) fn load() -> SourceKey {
    file().map_or(SourceKey::All, |f| load_from(&f))
}

/// Worker body: remember `key`. Best effort: a read-only settings folder
/// only costs the choice at the next start.
pub(crate) fn save(key: &SourceKey) {
    if let Some(f) = file() {
        save_to(&f, key);
    }
}

/// Worker body: [`load`] from `file`.
fn load_from(file: &Path) -> SourceKey {
    fs::read_to_string(file).map_or(SourceKey::All, |t| SourceKey::decode(&t))
}

/// Worker body: [`save`] to `file`, atomically.
fn save_to(file: &Path, key: &SourceKey) {
    if let Some(dir) = file.parent()
        && fs::create_dir_all(dir).is_err()
    {
        return;
    }
    let tmp = file.with_extension("tmp");
    if fs::write(&tmp, key.encode() + "\n").is_ok() && fs::rename(&tmp, file).is_err() {
        let _ = fs::remove_file(&tmp);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(origin: &str, id: &str, name: &str) -> AddedSource {
        AddedSource {
            descriptor_url: format!("{origin}/dmm-source.json"),
            origin: origin.into(),
            source_id: id.into(),
            name: name.into(),
            api_origins: Vec::new(),
            download_origins: Vec::new(),
        }
    }

    #[test]
    fn keys_round_trip_and_junk_reads_as_all() {
        let mine = SourceKey::of(&source(
            "https://luxus.github.io",
            "luxus",
            "luxus Deadlock Mods",
        ));
        for key in [SourceKey::All, SourceKey::Catalog, mine] {
            assert_eq!(SourceKey::decode(&(key.encode() + "\n")), key);
        }
        for junk in ["", "nonsense", "source\t\tx", "source\ta", "catalog\textra"] {
            assert_eq!(SourceKey::decode(junk), SourceKey::All, "{junk:?}");
        }
    }

    #[test]
    fn options_count_per_source_and_keep_a_remembered_one_not_yet_loaded() {
        let mine = source("https://luxus.github.io", "luxus", "luxus Deadlock Mods");
        let other = source("https://example.org", "x", "Other");
        let gone = SourceKey::Added {
            origin: "https://gone.example".into(),
            source_id: "gone".into(),
        };
        let o = Options::new(1234, &[(mine.clone(), 3), (other, 2)], &gone);
        assert_eq!(
            o.labels,
            [
                "All sources (1,239)",
                "GameBanana (1,234)",
                "luxus Deadlock Mods (3)",
                "Other (2)",
                "gone (not loaded)"
            ]
        );
        assert_eq!(o.index_of(&SourceKey::of(&mine)), 2);
        assert_eq!(o.index_of(&gone), 4);
        assert!(SourceKey::of(&mine).shows_source(&mine));
        assert!(!SourceKey::of(&mine).shows_catalog());
        assert!(!SourceKey::Catalog.shows_source(&mine));
    }

    #[test]
    fn the_choice_survives_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("nested").join("browse-source");
        assert_eq!(load_from(&file), SourceKey::All);
        let key = SourceKey::Added {
            origin: "https://luxus.github.io".into(),
            source_id: "luxus".into(),
        };
        save_to(&file, &key);
        assert_eq!(load_from(&file), key);
        save_to(&file, &SourceKey::Catalog);
        assert_eq!(load_from(&file), SourceKey::Catalog);
    }
}
