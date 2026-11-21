//! Reset to release and Restore previous (issue #21): put back the version a
//! mod's source publishes, keeping the files it replaces so the reset can
//! be undone.
//!
//! A reset moves the mod's current files (dir VPKs, chunks, `.disabled`
//! names as they are) and a copy of its sidecar into
//! `<addons>/.shiv/backup/reset/<mod id>/<unix ns>/`, runs the caller's
//! install — the same download, verify and install path the mod came
//! through, so there is no second installer — and then returns the mod to
//! its load position and enabled state. A failed install moves the files
//! back. Restore previous swaps: the newest backup's files return under
//! their old names, and the files they displace become a backup in turn.
//! Both refuse while the game runs, before anything moves.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::addons;
use crate::game::GamePaths;
use crate::sidecar::{self, InstalledMod, Sidecar, SidecarError};

const RESET_DIR: &str = ".shiv/backup/reset";
const MANIFEST: &str = "manifest.json";

#[derive(Debug, thiserror::Error)]
pub enum ResetError {
    #[error("Deadlock is running")]
    GameRunning,
    #[error("no installed mod {0}")]
    NotFound(String),
    #[error("no earlier version of {0} is kept")]
    NoBackup(String),
    #[error("{0} is taken by another mod")]
    SlotTaken(String),
    #[error("the release did not install: {0}")]
    Install(String),
    #[error(transparent)]
    Sidecar(#[from] SidecarError),
    #[error("io error on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// The files a reset or restore moved aside, and how to put them back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Backup {
    /// The backup's folder under `<addons>/.shiv/backup/reset/<mod id>/`.
    #[serde(skip)]
    pub dir: PathBuf,
    /// The sidecar the files belonged to.
    pub sidecar: Sidecar,
    /// The file names as they lay in addons, `.disabled` included.
    pub files: Vec<String>,
}

/// Reset `mod_id` to its release: back its files up, run `install` (which
/// installs the release under the same mod id), then restore the load
/// position and the enabled state it had. Returns the backup the replaced
/// files went to.
///
/// # Errors
///
/// [`ResetError::GameRunning`] before anything moves;
/// [`ResetError::NotFound`] when no installed mod has the id;
/// [`ResetError::Install`] when `install` fails — the files are back then;
/// the io and sidecar errors of moving files and writing the backup.
pub fn reset_with(
    paths: &GamePaths,
    mod_id: &str,
    game_running: impl Fn() -> bool,
    install: impl FnOnce() -> Result<(), String>,
) -> Result<Backup, ResetError> {
    if game_running() {
        return Err(ResetError::GameRunning);
    }
    let addons = paths.addons.as_path();
    let current = find(addons, mod_id)?;
    let position = current.load_position();
    let was_disabled = !current.vpks.is_empty() && current.vpks.iter().all(|v| !v.enabled);
    let backup = move_aside(addons, &current)?;
    if let Err(e) = install() {
        put_back(addons, &backup)?;
        return Err(ResetError::Install(e));
    }
    settle(addons, mod_id, position, was_disabled)?;
    Ok(backup)
}

/// Restore the newest backup of `mod_id`: its files return under their old
/// names with their sidecar, and the mod's current files become a backup
/// themselves (so a second restore swaps back).
///
/// # Errors
///
/// [`ResetError::GameRunning`] before anything moves;
/// [`ResetError::NoBackup`] when nothing is kept for the mod;
/// [`ResetError::SlotTaken`] when another mod took one of the old names —
/// nothing is changed then; the io and sidecar errors of moving files.
pub fn restore_previous(
    paths: &GamePaths,
    mod_id: &str,
    game_running: impl Fn() -> bool,
) -> Result<Sidecar, ResetError> {
    if game_running() {
        return Err(ResetError::GameRunning);
    }
    let addons = paths.addons.as_path();
    let previous =
        latest_backup(addons, mod_id).ok_or_else(|| ResetError::NoBackup(mod_id.into()))?;
    let current = find(addons, mod_id).ok();
    let own: Vec<PathBuf> = current
        .iter()
        .flat_map(|m| {
            m.vpks
                .iter()
                .flat_map(|v| std::iter::once(v.file.clone()).chain(v.chunks.iter().cloned()))
        })
        .collect();
    if let Some(taken) = previous
        .files
        .iter()
        .map(|name| addons.join(name))
        .find(|p| p.exists() && !own.contains(p))
    {
        return Err(ResetError::SlotTaken(file_name(&taken)));
    }
    let displaced = match &current {
        Some(m) => Some(move_aside(addons, m)?),
        None => None,
    };
    if let Err(e) = put_back(addons, &previous) {
        if let Some(d) = &displaced {
            let _ = put_back(addons, d);
        }
        return Err(e);
    }
    Ok(previous.sidecar)
}

/// The newest backup kept for `mod_id`, if any.
#[must_use]
pub fn latest_backup(addons: &Path, mod_id: &str) -> Option<Backup> {
    let root = addons.join(RESET_DIR).join(safe(mod_id));
    let mut dirs: Vec<PathBuf> = fs::read_dir(root)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.join(MANIFEST).is_file())
        .collect();
    dirs.sort();
    let dir = dirs.pop()?;
    let text = fs::read_to_string(dir.join(MANIFEST)).ok()?;
    let mut backup: Backup = serde_json::from_str(&text).ok()?;
    backup.dir = dir;
    Some(backup)
}

fn find(addons: &Path, mod_id: &str) -> Result<InstalledMod, ResetError> {
    sidecar::group(addons)
        .into_iter()
        .find(|m| m.sidecar.as_ref().is_some_and(|s| s.mod_id == mod_id))
        .ok_or_else(|| ResetError::NotFound(mod_id.into()))
}

/// Move a mod's files and a copy of its sidecar into a fresh backup folder.
/// On a failed move the files moved so far go back.
fn move_aside(addons: &Path, m: &InstalledMod) -> Result<Backup, ResetError> {
    let Some(sidecar) = m.sidecar.clone() else {
        return Err(ResetError::NotFound(String::new()));
    };
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let root = addons.join(RESET_DIR).join(safe(&sidecar.mod_id));
    let mut dir = root.join(format!("{nanos:020}"));
    let mut n = 0u32;
    while dir.exists() {
        n += 1;
        dir = root.join(format!("{nanos:020}-{n}"));
    }
    fs::create_dir_all(&dir).map_err(io(&dir))?;
    let files: Vec<PathBuf> = m
        .vpks
        .iter()
        .flat_map(|v| std::iter::once(v.file.clone()).chain(v.chunks.iter().cloned()))
        .collect();
    let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();
    for file in &files {
        let to = dir.join(file_name(file));
        if let Err(e) = fs::rename(file, &to) {
            for (from, to) in moved.iter().rev() {
                let _ = fs::rename(to, from);
            }
            return Err(io(file)(e));
        }
        moved.push((file.clone(), to));
    }
    let backup = Backup {
        dir: dir.clone(),
        sidecar,
        files: files.iter().map(|f| file_name(f)).collect(),
    };
    let manifest =
        serde_json::to_string_pretty(&backup).map_err(|e| io(&dir)(std::io::Error::other(e)))?;
    fs::write(dir.join(MANIFEST), manifest).map_err(io(&dir))?;
    Ok(backup)
}

/// Move a backup's files back under their names, write its sidecar, and
/// drop the emptied backup folder.
fn put_back(addons: &Path, backup: &Backup) -> Result<(), ResetError> {
    for name in &backup.files {
        let to = addons.join(name);
        if to.exists() {
            return Err(ResetError::SlotTaken(name.clone()));
        }
    }
    for name in &backup.files {
        let from = backup.dir.join(name);
        let to = addons.join(name);
        fs::rename(&from, &to).map_err(io(&from))?;
    }
    sidecar::write(addons, &backup.sidecar)?;
    let _ = fs::remove_file(backup.dir.join(MANIFEST));
    let _ = fs::remove_dir(&backup.dir);
    Ok(())
}

/// After the release installed: back to the load position the mod had, and
/// disabled again when it was.
fn settle(
    addons: &Path,
    mod_id: &str,
    position: u32,
    was_disabled: bool,
) -> Result<(), ResetError> {
    let mods = sidecar::group(addons);
    if let Some(index) = mods
        .iter()
        .position(|m| m.sidecar.as_ref().is_some_and(|s| s.mod_id == mod_id))
        && position != u32::MAX
        && mods[index].load_position() != position
    {
        sidecar::set_pak(addons, &mods, index, position)?;
    }
    if was_disabled {
        for v in &find(addons, mod_id)?.vpks {
            addons::set_enabled(v, false).map_err(SidecarError::from)?;
        }
    }
    Ok(())
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn safe(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn io(path: &Path) -> impl Fn(std::io::Error) -> ResetError + '_ {
    move |source| ResetError::Io {
        path: path.to_path_buf(),
        source,
    }
}
