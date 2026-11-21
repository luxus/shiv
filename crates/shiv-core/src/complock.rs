//! CompLock's engine settings, from [deadlockconfig.com](https://deadlockconfig.com/):
//! the ConVars of their published `gameinfo.gi`, staged into the Performance
//! screen and written through [`crate::perf::apply`]. The asset is their file
//! verbatim, kept at the site's current build by the `complock-watch`
//! workflow; the meta beside it names the build and the fetch date. Only the
//! ConVars are taken — their search paths match what Shiv's own fix-mount
//! writes, and their mod paks are ordinary mods Shiv manages like any other.

use kv_parser::{ParseOptions, SourceDocument, SourceKind};

use crate::perf::ConfigEntry;

const GAMEINFO: &str = include_str!("../assets/complock-gameinfo.gi");
const META: &str = include_str!("../assets/complock.json");

/// The game build the settings were crafted for, from the site's meta.
#[must_use]
pub fn game() -> &'static str {
    build_meta().map_or("", |(game, _)| game)
}

/// When the embedded files were fetched, as `YYYY-MM-DD`.
#[must_use]
pub fn fetched() -> &'static str {
    build_meta().map_or("", |(_, fetched)| fetched)
}

/// CompLock's ConVars as staged entries: every scalar pair in their
/// `gameinfo.gi`'s `ConVars` block, in file order. The file is the only
/// source of truth; a malformed asset yields no entries, which the core
/// test on the asset catches before it reaches a build.
#[must_use]
pub fn entries() -> Vec<ConfigEntry> {
    let Ok(doc) = SourceDocument::parse(GAMEINFO, parse_options()) else {
        return Vec::new();
    };
    doc.sites
        .iter()
        .filter_map(|site| {
            let SourceKind::Scalar { value, .. } = &site.kind else {
                return None;
            };
            let path: &[String] = site.path.split_first().map_or(&site.path, |(_, rest)| rest);
            (path.len() == 2 && path[0].eq_ignore_ascii_case("ConVars")).then(|| ConfigEntry {
                path: path.to_vec(),
                value: Some(value.clone()),
            })
        })
        .collect()
}

/// The parsed meta, or `None` when the workflow wrote something malformed —
/// which the core test on the asset catches before it reaches a build.
fn build_meta() -> Option<(&'static str, &'static str)> {
    static META_ONCE: std::sync::OnceLock<Option<(&'static str, &'static str)>> =
        std::sync::OnceLock::new();
    *META_ONCE.get_or_init(parse_meta)
}

fn parse_meta() -> Option<(&'static str, &'static str)> {
    let parsed: serde_json::Value = serde_json::from_str(META).ok()?;
    let game: &'static str = Box::leak(parsed.get("game")?.as_str()?.to_owned().into_boxed_str());
    let fetched: &'static str =
        Box::leak(parsed.get("fetched")?.as_str()?.to_owned().into_boxed_str());
    Some((game, fetched))
}

/// Source 2 reads backslashes literally in gameinfo.gi (as in perf).
fn parse_options() -> ParseOptions {
    ParseOptions {
        allow_escape_sequences: false,
        ..ParseOptions::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_meta_names_a_build_and_a_fetch_date() {
        assert!(build_meta().is_some(), "the workflow's meta parses");
        assert!(game().bytes().all(|b| b.is_ascii_digit()) && !game().is_empty());
        assert_eq!(fetched().len(), 10, "a YYYY-MM-DD date");
    }

    #[test]
    fn the_entries_are_the_convars_block_verbatim() {
        let first = entries();
        assert!(!first.is_empty(), "a CompLock gameinfo carries settings");
        for entry in &first {
            assert_eq!(entry.path.len(), 2);
            assert_eq!(entry.path[0].to_ascii_lowercase(), "convars");
            assert!(
                entry.value.as_deref().is_some_and(|v| !v.is_empty()),
                "{} carries a value",
                entry.path[1]
            );
        }
        // Re-parsing reproduces the same entries: the file is the truth.
        assert_eq!(entries(), first);
    }

    #[test]
    fn the_asset_is_a_gameinfo_with_convars_and_search_paths() {
        assert!(GAMEINFO.contains("ConVars"));
        assert!(GAMEINFO.contains("SearchPaths"));
        assert!(
            GAMEINFO.contains("citadel/addons"),
            "their mounts are intact"
        );
    }
}
