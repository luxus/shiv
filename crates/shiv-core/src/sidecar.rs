//! Sidecar metadata linking installed VPKs to an API mod id and version.
//!
//! Files live in `<addons>/.shiv/<mod_id>.json`. `Sidecar::vpks` holds the
//! enabled-form file names (no `.disabled` suffix) so toggling a mod never
//! breaks the link.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::{fs, io};

use serde::{Deserialize, Serialize};

use crate::addons::{self, AddonsError, InstalledVpk};

const DIR: &str = ".shiv";
const DISABLED_EXT: &str = ".disabled";

#[derive(Debug, thiserror::Error)]
pub enum SidecarError {
    #[error("invalid mod id {0:?}: only [A-Za-z0-9_-] allowed, non-empty")]
    InvalidModId(String),
    #[error("io error on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("corrupt sidecar {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error(transparent)]
    Addons(#[from] AddonsError),
    #[error("no such mod, or it is already managed by Shiv")]
    NotAdoptable,
    #[error("pak number {0} is outside 01..=99")]
    BadPak(u32),
    #[error("setting pak {pak} would push another VPK past 99")]
    PakPush { pak: u32 },
    #[error("the mods list has two VPKs on the same pak number")]
    DuplicatePak(u32),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sidecar {
    pub mod_id: String,
    pub name: String,
    pub version: Option<String>,
    pub thumbnail_url: Option<String>,
    /// VPK file names without the `.disabled` suffix.
    pub vpks: Vec<String>,
    /// Whether the catalog marked the mod NSFW when it was installed.
    /// `None` in sidecars written before this field existed (unknown); not
    /// written when `None`, so older Shiv builds read new sidecars too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nsfw: Option<bool>,
    /// The hero the catalog lists the mod for, when it was installed. Together
    /// with `category` it tells which mods replace the same thing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hero: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// Free text the user wrote about the mod. Not written when `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// File name of the local file the mod was last installed or updated
    /// from; used to recognise a newer copy of the same file. Not written when `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// What each of `vpks` looked like on disk when Shiv placed it, in the
    /// same order: the [`Stamp`] a later look compares against to tell a
    /// pristine install from a replaced or patched one. `None` in sidecars
    /// written before this field existed and for adopted mods (Shiv did not
    /// place their files); not written when `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stamps: Option<Vec<Stamp>>,
}

/// One installed VPK as it lay on disk: the bytes of the dir VPK and its
/// chunks together, and the latest modification time among them, in
/// nanoseconds since the Unix epoch. Renames (enable, disable, load order)
/// keep both; a file replaced, patched or partly rewritten changes the
/// time, and almost always the size. Read from metadata only, so taking
/// one costs no read of the bytes (the install wall-clock gate,
/// docs/perf-results.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stamp {
    pub size: u64,
    pub modified_ns: u64,
}

impl Stamp {
    /// The stamp of a dir VPK and its chunk files.
    ///
    /// # Errors
    ///
    /// [`SidecarError::Io`] when a file's metadata cannot be read.
    pub fn of_files<'a>(files: impl IntoIterator<Item = &'a Path>) -> Result<Stamp, SidecarError> {
        let mut stamp = Stamp {
            size: 0,
            modified_ns: 0,
        };
        for file in files {
            let meta = fs::metadata(file).map_err(|source| SidecarError::Io {
                path: file.to_path_buf(),
                source,
            })?;
            stamp.size += meta.len();
            let modified = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX));
            stamp.modified_ns = stamp.modified_ns.max(modified);
        }
        Ok(stamp)
    }

    /// The stamp of an installed VPK as it lies now.
    ///
    /// # Errors
    ///
    /// As [`Stamp::of_files`].
    pub fn of_vpk(vpk: &InstalledVpk) -> Result<Stamp, SidecarError> {
        Stamp::of_files(
            std::iter::once(vpk.file.as_path()).chain(vpk.chunks.iter().map(PathBuf::as_path)),
        )
    }
}

/// Prefix of the ids Shiv gives mods that do not come from the catalog: a VPK
/// or archive installed from a local file, or a hand-added pak adopted in place.
pub const LOCAL_PREFIX: &str = "local-";

/// Whether `mod_id` names a local mod (see [`LOCAL_PREFIX`]), which has no
/// catalog entry, detail page or updates.
#[must_use]
pub fn is_local(mod_id: &str) -> bool {
    mod_id.starts_with(LOCAL_PREFIX)
}

/// A fresh local mod id (`local-<unix seconds>-<n>`) no sidecar in `addons` uses.
#[must_use]
pub fn new_local_id(addons: &Path) -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    unused_local_id(&addons.join(DIR), secs)
}

/// The counter runs over u32, so a name no sidecar uses exists.
#[allow(
    clippy::expect_used,
    reason = "the counter is bounded by u32, so an unused id exists"
)]
fn unused_local_id(dir: &Path, secs: u64) -> String {
    (0..u32::MAX)
        .map(|n| format!("{LOCAL_PREFIX}{secs}-{n}"))
        .find(|id| !dir.join(format!("{id}.json")).exists())
        .expect("an unused local id")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledMod {
    pub sidecar: Option<Sidecar>,
    pub vpks: Vec<InstalledVpk>,
}

impl InstalledMod {
    /// The lowest pak number among its VPKs: where it sits in the load order
    /// (`u32::MAX` when none of them has a pak number).
    #[must_use]
    pub fn load_position(&self) -> u32 {
        self.vpks
            .iter()
            .filter_map(|v| v.pak)
            .min()
            .unwrap_or(u32::MAX)
    }

    /// Whether the mod counts as switched on: it has VPKs and all are enabled.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        !self.vpks.is_empty() && self.vpks.iter().all(|v| v.enabled)
    }
}

/// Enabled mods, other than `mod_id`, that the catalog lists for the same hero
/// and category: they replace the same thing, so only one can win in game.
/// Mods without a recorded hero (hand-added, or installed before it was
/// recorded) never count.
#[must_use]
pub fn conflicts<'a>(
    mods: &'a [InstalledMod],
    mod_id: &str,
    hero: &str,
    category: Option<&str>,
) -> Vec<&'a Sidecar> {
    mods.iter()
        .filter(|m| m.is_enabled())
        .filter_map(|m| m.sidecar.as_ref())
        .filter(|s| {
            s.mod_id != mod_id
                && s.hero.as_deref() == Some(hero)
                && s.category.as_deref() == category
        })
        .collect()
}

/// `(mod id, display name)` of the enabled mods that conflict with `mod_id`.
/// `kind_of` supplies a mod's `(hero, category)` when its sidecar records none
/// (installed before they were recorded), usually from the catalog.
pub fn conflicts_with(
    mods: &[InstalledMod],
    mod_id: &str,
    kind_of: impl Fn(&str) -> Option<(Option<String>, Option<String>)>,
) -> Vec<(String, String)> {
    let mut mods = mods.to_vec();
    for s in mods.iter_mut().filter_map(|m| m.sidecar.as_mut()) {
        if let (None, Some((hero, category))) = (&s.hero, kind_of(&s.mod_id)) {
            s.hero = hero;
            s.category = category;
        }
    }
    let Some(me) = mods
        .iter()
        .filter_map(|m| m.sidecar.as_ref())
        .find(|s| s.mod_id == mod_id)
    else {
        return Vec::new();
    };
    let Some(hero) = me.hero.as_deref() else {
        return Vec::new();
    };
    conflicts(&mods, mod_id, hero, me.category.as_deref())
        .into_iter()
        .map(|s| {
            let label = if s.name.is_empty() {
                &s.mod_id
            } else {
                &s.name
            };
            (s.mod_id.clone(), label.clone())
        })
        .collect()
}

fn validate(mod_id: &str) -> Result<(), SidecarError> {
    let ok = !mod_id.is_empty()
        && mod_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if ok {
        Ok(())
    } else {
        Err(SidecarError::InvalidModId(mod_id.to_owned()))
    }
}

fn path_for(addons: &Path, mod_id: &str) -> PathBuf {
    addons.join(DIR).join(format!("{mod_id}.json"))
}

fn read_file(path: &Path) -> Result<Option<Sidecar>, SidecarError> {
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(SidecarError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|source| SidecarError::Parse {
            path: path.to_path_buf(),
            source,
        })
}

/// Read the sidecar for `mod_id`. Missing is `Ok(None)`; corrupt is an error.
///
/// # Errors
///
/// [`SidecarError::BadId`] when the id cannot name a file;
/// [`SidecarError::Parse`] when the sidecar exists but is not valid JSON
/// for a sidecar; [`SidecarError::Io`] when it cannot be read.
pub fn read(addons: &Path, mod_id: &str) -> Result<Option<Sidecar>, SidecarError> {
    validate(mod_id)?;
    read_file(&path_for(addons, mod_id))
}

/// Write the sidecar atomically (temp file in the same directory, then rename).
///
/// # Errors
///
/// [`SidecarError::BadId`] when the id cannot name a file;
/// [`SidecarError::Io`] when the temp file cannot be written or renamed —
/// the temp file is removed then.
pub fn write(addons: &Path, sidecar: &Sidecar) -> Result<(), SidecarError> {
    validate(&sidecar.mod_id)?;
    let dir = addons.join(DIR);
    let io_err = |path: &Path, source| SidecarError::Io {
        path: path.to_path_buf(),
        source,
    };
    fs::create_dir_all(&dir).map_err(|e| io_err(&dir, e))?;
    let json = serde_json::to_vec_pretty(sidecar).map_err(|source| SidecarError::Parse {
        path: dir.clone(),
        source,
    })?;
    let final_path = path_for(addons, &sidecar.mod_id);
    let tmp = dir.join(format!(".{}.json.tmp", sidecar.mod_id));
    fs::write(&tmp, json).map_err(|e| io_err(&tmp, e))?;
    fs::rename(&tmp, &final_path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        io_err(&final_path, e)
    })
}

/// Record a mod Shiv did not install (no sidecar) as a local mod named
/// `name`, so it can be named, noted and kept together. Only a sidecar is
/// written: the mod's files are not moved, renamed or changed.
///
/// # Errors
///
/// [`SidecarError::NotAdoptable`] when the mod already has a sidecar or has
/// no VPK files; the write errors as for [`write`].
pub fn adopt(
    addons: &Path,
    m: &InstalledMod,
    name: &str,
    notes: Option<String>,
) -> Result<Sidecar, SidecarError> {
    let mut vpks: Vec<String> = m.vpks.iter().filter_map(enabled_name).collect();
    vpks.sort();
    vpks.dedup();
    if m.sidecar.is_some() || vpks.is_empty() {
        return Err(SidecarError::NotAdoptable);
    }
    let sidecar = Sidecar {
        mod_id: new_local_id(addons),
        name: name.trim().to_owned(),
        version: None,
        thumbnail_url: None,
        vpks,
        nsfw: Some(false),
        hero: None,
        category: None,
        notes: notes.filter(|n| !n.trim().is_empty()),
        source: None,
        stamps: None,
    };
    write(addons, &sidecar)?;
    Ok(sidecar)
}

/// Change the display name and notes of the mod `mod_id`. Only its sidecar
/// is rewritten; an empty name falls back to the id on screen.
///
/// # Errors
///
/// [`SidecarError::NotAdoptable`] when there is no sidecar to edit; the
/// read and write errors as for [`read`] and [`write`].
pub fn edit(
    addons: &Path,
    mod_id: &str,
    name: &str,
    notes: Option<String>,
) -> Result<Sidecar, SidecarError> {
    let mut sidecar = read(addons, mod_id)?.ok_or(SidecarError::NotAdoptable)?;
    name.trim().clone_into(&mut sidecar.name);
    sidecar.notes = notes.filter(|n| !n.trim().is_empty());
    write(addons, &sidecar)?;
    Ok(sidecar)
}

fn enabled_name(vpk: &InstalledVpk) -> Option<String> {
    let name = vpk.file.file_name()?.to_str()?;
    Some(name.strip_suffix(DISABLED_EXT).unwrap_or(name).to_owned())
}

/// All sidecars in `<addons>/.shiv`; only `<valid-id>.json` files are considered
/// and corrupt ones are skipped.
fn read_all(addons: &Path) -> Vec<Sidecar> {
    let Ok(entries) = fs::read_dir(addons.join(DIR)) else {
        return Vec::new();
    };
    let mut out = BTreeMap::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(id) = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_suffix(".json"))
        else {
            continue;
        };
        if validate(id).is_err() || !path.is_file() {
            continue;
        }
        if let Ok(Some(s)) = read_file(&path)
            && s.mod_id == id
        {
            out.insert(s.mod_id.clone(), s);
        }
    }
    out.into_values().collect()
}

/// Group installed VPKs by sidecar. Sidecar mods come first (by mod id), then
/// unknown VPKs (by file name), each alone. A sidecar with no VPKs on disk is
/// omitted. Never fails: unreadable or corrupt sidecars leave their VPKs unknown.
pub fn group(addons: &Path) -> Vec<InstalledMod> {
    let Ok(vpks) = addons::list(addons) else {
        return Vec::new();
    };
    let mut remaining: Vec<(String, InstalledVpk)> = vpks
        .into_iter()
        .filter_map(|v| enabled_name(&v).map(|n| (n, v)))
        .collect();
    let mut out = Vec::new();
    for s in read_all(addons) {
        let mut mine = Vec::new();
        remaining.retain(|(name, v)| {
            if s.vpks.contains(name) {
                mine.push(v.clone());
                false
            } else {
                true
            }
        });
        if !mine.is_empty() {
            out.push(InstalledMod {
                sidecar: Some(s),
                vpks: mine,
            });
        }
    }
    remaining.sort_by(|a, b| a.0.cmp(&b.0));
    out.extend(remaining.into_iter().map(|(_, v)| InstalledMod {
        sidecar: None,
        vpks: vec![v],
    }));
    // Load order: the engine mounts the lowest pak number first. The sort is
    // stable, so mods without a pak number stay last in the order above.
    out.sort_by_key(InstalledMod::load_position);
    out
}

/// Where [`move_mod`] puts a mod in the load order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Move {
    /// One place earlier.
    Up,
    /// One place later.
    Down,
    /// The very first place: it wins every conflict.
    Top,
    /// A 0-based place among the movable mods; past the last means last.
    To(usize),
}

/// Move `mods[index]` in the load order by renumbering paks, and keep the
/// sidecars pointing at the renamed files. `mods` is a [`group`] result.
/// Mods without a pak number (hand-added under another name) cannot move and
/// are not counted. Moving past either end does nothing. If recording the
/// new names fails, the file renames are undone.
///
/// # Errors
///
/// [`SidecarError::Addons`] when the renumbering plan cannot be built or a
/// rename fails — renames already done are undone; [`SidecarError::Io`]
/// when a sidecar cannot be rewritten.
pub fn move_mod(
    addons: &Path,
    mods: &[InstalledMod],
    index: usize,
    to: Move,
) -> Result<(), SidecarError> {
    let mut order: Vec<&InstalledMod> = mods
        .iter()
        .filter(|m| m.load_position() != u32::MAX)
        .collect();
    let Some(from) = mods
        .get(index)
        .and_then(|m| order.iter().position(|o| std::ptr::eq(*o, m)))
    else {
        return Ok(());
    };
    let target = match to {
        Move::Up => from.saturating_sub(1),
        Move::Down => (from + 1).min(order.len() - 1),
        Move::Top => 0,
        Move::To(n) => n.min(order.len() - 1),
    };
    if target == from {
        return Ok(());
    }
    let moved = order.remove(from);
    order.insert(target, moved);

    let flat: Vec<InstalledVpk> = order
        .iter()
        .flat_map(|m| {
            let mut vpks = m.vpks.clone();
            vpks.sort_by_key(|v| v.pak);
            vpks
        })
        .collect();
    let plan = addons::plan_reorder(&flat)?;
    apply_plan(addons, &order, &plan)
}

/// Give `mods[index]`'s VPKs `pak` as their pak numbers. A mod with several
/// VPKs takes `pak`, `pak + 1`, ... in its load order; every VPK of another
/// mod with a number at or above `pak` moves up by that many places, and
/// everything below stays, so no other mod changes its relative order. Files
/// are renamed in two phases and the sidecars are kept pointing at them,
/// with the same rollback as [`move_mod`]. A number outside `01..=99`, or one
/// that would push another VPK past 99 ([`SidecarError::PakPush`]), fails
/// before anything changes. Like [`move_mod`], a mod without a pak number
/// (a hand-added file under another name) is not movable and the call is a
/// no-op.
///
/// `mods` must come from [`group`], which never puts two VPKs on one pak
/// number; a slice that does is refused with [`SidecarError::DuplicatePak`]
/// instead of planning a rename that would overwrite a file.
///
/// # Errors
///
/// [`SidecarError::BadPak`] for a number outside `01..=99`;
/// [`SidecarError::PakPush`] when the move would push another VPK past 99;
/// [`SidecarError::DuplicatePak`] for a `mods` slice `group` did not build;
/// the rename and sidecar errors as for [`move_mod`]. Every refusal happens
/// before the first rename.
pub fn set_pak(
    addons: &Path,
    mods: &[InstalledMod],
    index: usize,
    pak: u32,
) -> Result<(), SidecarError> {
    let Some(target) = mods.get(index).filter(|m| m.load_position() != u32::MAX) else {
        return Ok(());
    };
    // Range first, so the block arithmetic below cannot overflow: a `pak`
    // within 01..=99 plus any VPK count stays far below `u64::MAX`.
    if pak == 0 || pak > addons::PAK_CAPACITY {
        return Err(SidecarError::BadPak(pak));
    }
    let take = u32::try_from(target.vpks.iter().filter(|v| v.pak.is_some()).count())
        .map_err(|_| SidecarError::PakPush { pak })?;
    if u64::from(pak) + u64::from(take) - 1 > u64::from(addons::PAK_CAPACITY) {
        return Err(SidecarError::PakPush { pak });
    }
    // Every pak-numbered VPK in load order, marked when it belongs to the mod.
    let mut ranked: Vec<(&InstalledVpk, bool)> = Vec::new();
    for m in mods.iter().filter(|m| m.load_position() != u32::MAX) {
        let own = std::ptr::eq(m, target);
        let mut vpks: Vec<&InstalledVpk> = m.vpks.iter().filter(|v| v.pak.is_some()).collect();
        vpks.sort_by_key(|v| v.pak);
        ranked.extend(vpks.into_iter().map(|v| (v, own)));
    }
    debug_assert!(ranked.iter().any(|(_, own)| *own), "target is ranked");
    let mut numbers: Vec<u32> = Vec::with_capacity(ranked.len());
    let mut nth = 0u32;
    for (v, own) in &ranked {
        let old = pak_of(v);
        let n = if *own {
            nth += 1;
            pak + nth - 1
        } else if old >= pak {
            old + take
        } else {
            old
        };
        if n > addons::PAK_CAPACITY {
            // The asked number fits; this VPK's shift does not.
            return Err(SidecarError::PakPush { pak });
        }
        numbers.push(n);
    }
    // Two VPKs landing on one number means the slice broke the invariant
    // `group` guarantees (two mods claiming the same pak number). A plan
    // with duplicate targets would rename one file onto another, so it
    // is refused before anything changes.
    let mut assigned = numbers.clone();
    assigned.sort_unstable();
    if assigned.windows(2).any(|w| w[0] == w[1]) {
        return Err(SidecarError::DuplicatePak(pak));
    }
    let refs: Vec<&InstalledVpk> = ranked.iter().map(|(v, _)| *v).collect();
    let order: Vec<&InstalledMod> = mods
        .iter()
        .filter(|m| m.load_position() != u32::MAX)
        .collect();
    let plan = addons::plan_renumber(&refs, &numbers)?;
    apply_plan(addons, &order, &plan)
}

/// The pak number of a ranked VPK. `ranked` only holds pak-numbered VPKs
/// (the loop that builds it filters `pak.is_some()`), so this cannot fail;
/// a panic here would mean the plan is built on a broken invariant.
#[allow(
    clippy::expect_used,
    reason = "ranked only holds pak-numbered VPKs; the filter is where it is built"
)]
fn pak_of(v: &addons::InstalledVpk) -> u32 {
    v.pak.expect("ranked holds pak-numbered VPKs")
}

/// Apply `plan`'s renames, then keep the sidecars in `order` pointing at the
/// renamed files. On a failed sidecar write, the renames and the sidecars
/// written so far are undone.
fn apply_plan(
    addons: &Path,
    order: &[&InstalledMod],
    plan: &addons::ReorderPlan,
) -> Result<(), SidecarError> {
    addons::apply_moves(&plan.moves)?;

    let mut written: Vec<&Sidecar> = Vec::new();
    for s in order.iter().filter_map(|m| m.sidecar.as_ref()) {
        let mut updated = s.clone();
        for name in &mut updated.vpks {
            if let Some((_, new)) = plan.names.iter().find(|(old, _)| old == name) {
                name.clone_from(new);
            }
        }
        if updated == *s {
            continue;
        }
        if let Err(e) = write(addons, &updated) {
            for old in written {
                let _ = write(addons, old);
            }
            let undo: Vec<_> = plan
                .moves
                .iter()
                .map(|(f, t)| (t.clone(), f.clone()))
                .collect();
            let _ = addons::apply_moves(&undo);
            return Err(e);
        }
        written.push(s);
    }
    Ok(())
}
