//! Persistent Activity log (issue #65).
//!
//! Records completed user-facing actions (installs, updates, file
//! replacements, restores, repairs, settings writes) to `<settings_dir>/activity.json`
//! via atomic rename, bounded to [`MAX_ENTRIES`].

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Maximum number of completed entries kept in `activity.json`.
pub const MAX_ENTRIES: usize = 100;

/// Kind of recorded action shown in the Activity feed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityKind {
    Installed,
    Updated,
    Replaced,
    Removed,
    Reset,
    Restored,
    Repaired,
    SettingsApplied,
    Failed,
    Cancelled,
}

impl ActivityKind {
    /// Short human label displayed beside an Activity row.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Installed => "Installed",
            Self::Updated => "Updated",
            Self::Replaced => "Replaced",
            Self::Removed => "Removed",
            Self::Reset => "Reset",
            Self::Restored => "Restored",
            Self::Repaired => "Repaired",
            Self::SettingsApplied => "Settings",
            Self::Failed => "Failed",
            Self::Cancelled => "Cancelled",
        }
    }
}

/// One persisted Activity entry, newest first when saved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityEntry {
    pub id: String,
    pub timestamp: u64,
    pub kind: ActivityKind,
    pub title: String,
    pub detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// Current Unix timestamp in seconds.
#[must_use]
pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Format an entry's Unix timestamp relative to `now` (`"Just now"`, `"5m ago"`, etc.).
#[must_use]
pub fn format_relative(timestamp: u64, now: u64) -> String {
    if timestamp == 0 {
        return String::new();
    }
    let elapsed = now.saturating_sub(timestamp);
    if elapsed < 60 {
        "Just now".to_owned()
    } else if elapsed < 3600 {
        format!("{}m ago", elapsed / 60)
    } else if elapsed < 86_400 {
        format!("{}h ago", elapsed / 3600)
    } else {
        format!("{}d ago", elapsed / 86_400)
    }
}

/// Default location of `activity.json` inside Shiv's settings directory.
#[must_use]
pub fn default_path() -> Option<PathBuf> {
    crate::settings_dir().map(|d| d.join("activity.json"))
}

/// Load entries from `path`. Missing, unreadable, or corrupt files yield an
/// empty vector; oversize lists are truncated to [`MAX_ENTRIES`].
#[must_use]
pub fn load_from(path: &Path) -> Vec<ActivityEntry> {
    let Ok(bytes) = fs::read(path) else {
        return Vec::new();
    };
    let Ok(mut entries) = serde_json::from_slice::<Vec<ActivityEntry>>(&bytes) else {
        return Vec::new();
    };
    entries.truncate(MAX_ENTRIES);
    entries
}

/// Atomically persist up to [`MAX_ENTRIES`] entries to `path`.
///
/// # Errors
///
/// Returns serialization errors or filesystem errors when creating the parent
/// directory or writing/renaming the file.
pub fn save_to(path: &Path, entries: &[ActivityEntry]) -> io::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }
    let slice = &entries[..entries.len().min(MAX_ENTRIES)];
    let bytes = serde_json::to_vec_pretty(slice).map_err(io::Error::other)?;
    crate::fs_util::write_atomic(path, &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn sample(i: usize, kind: ActivityKind) -> ActivityEntry {
        ActivityEntry {
            id: format!("act-{i}"),
            timestamp: 1_700_000_000 + i as u64,
            kind,
            title: format!("Item {i}"),
            detail: format!("Detail {i}"),
            from_version: (i == 0).then(|| "1.0.0".to_owned()),
            to_version: (i == 0).then(|| "1.1.0".to_owned()),
            source: Some("Registry".to_owned()),
        }
    }

    #[test]
    fn save_and_load_round_trip_and_cap_at_max_entries() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nested").join("activity.json");
        let many: Vec<_> = (0..(MAX_ENTRIES + 25))
            .map(|i| sample(i, ActivityKind::Updated))
            .collect();
        save_to(&path, &many).unwrap();
        let loaded = load_from(&path);
        assert_eq!(loaded.len(), MAX_ENTRIES);
        assert_eq!(loaded[0], many[0]);
        assert_eq!(loaded[MAX_ENTRIES - 1], many[MAX_ENTRIES - 1]);
    }

    #[test]
    fn missing_and_corrupt_files_load_empty() {
        let dir = tempdir().unwrap();
        assert!(load_from(&dir.path().join("missing.json")).is_empty());
        let bad = dir.path().join("bad.json");
        fs::write(&bad, b"{not valid json").unwrap();
        assert!(load_from(&bad).is_empty());
    }

    #[test]
    fn relative_ages_format_cleanly() {
        let base = 1_700_000_000;
        assert_eq!(format_relative(0, base), "");
        assert_eq!(format_relative(base, base), "Just now");
        assert_eq!(format_relative(base - 45, base), "Just now");
        assert_eq!(format_relative(base - 120, base), "2m ago");
        assert_eq!(format_relative(base - 7200, base), "2h ago");
        assert_eq!(format_relative(base - 172_800, base), "2d ago");
    }
}
