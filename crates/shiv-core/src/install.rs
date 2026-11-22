//! Install and uninstall mods from zip, rar and 7z archives.
//!
//! The archive is extracted (only `.vpk` entries) into a staging directory inside
//! `<addons>/.shiv/tmp`, which lives on the same filesystem as `addons` so the final
//! moves are renames. Every entry path is validated before anything is written, the
//! staged VPKs are renamed to free `pakNN_dir.vpk` slots, and the sidecar is written
//! last. Entry-path validation is adapted from the old manager's `archive_extractor.rs`.

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{self, BufReader, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::addons::{self, AddonsError, PakKind, parse_pak};
use crate::game::{self, GamePaths};
use crate::sidecar::{self, InstalledMod, Sidecar, SidecarError};

const DISABLED_EXT: &str = ".disabled";
const SHIV_DIR: &str = ".shiv";
const TMP_DIR: &str = "tmp";

static INSTALL_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Holds the install lock until dropped.
///
/// [`permit`](Self::permit) is the only way to call a function that
/// allocates a pak number or renames a `pakNN` file. Not reentrant:
/// acquiring it again on the same thread waits forever, so those
/// functions take the permit instead of locking themselves. A cancel
/// check belongs after [`acquire`](Self::acquire), before the permit
/// is used.
pub struct InstallGuard {
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl InstallGuard {
    /// Take the install lock, recovering it if a holder panicked.
    pub fn acquire() -> Self {
        Self {
            _guard: INSTALL_LOCK
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        }
    }

    /// Mint the proof those functions require. It dies with this guard.
    pub fn permit(&self) -> InstallPermit<'_> {
        InstallPermit {
            _guard: &self._guard,
        }
    }
}

/// Proof an [`InstallGuard`] is still held.
///
/// Copy, so one guard covers every pak change in its scope. No other
/// constructor exists.
#[derive(Clone, Copy)]
pub struct InstallPermit<'a> {
    _guard: &'a std::sync::MutexGuard<'static, ()>,
}

impl InstallPermit<'_> {
    pub(crate) fn keep(self) {
        let Self { _guard } = self;
        let _ = _guard;
    }
}

thread_local! {
    static DISK_BYTES_WRITTEN: Cell<u64> = const { Cell::new(0) };
}

/// Bytes the install paths have written to disk on the calling thread
/// since the last [`reset_disk_bytes_written`]: staged extraction of an
/// archive and copied source files each add exactly what they wrote.
/// Files move into place with renames, which write nothing, so an
/// install that extracts an archive once counts each of its bytes
/// exactly once. `install_writes_each_archive_byte_once` pins that
/// number, and `install_writes_are_counted_and_placement_renames`
/// scans this module's source so a write site outside
/// `tally_disk_write` cannot appear unnoticed.
pub fn disk_bytes_written() -> u64 {
    DISK_BYTES_WRITTEN.with(Cell::get)
}

/// Zero the calling thread's [`disk_bytes_written`] counter.
pub fn reset_disk_bytes_written() {
    DISK_BYTES_WRITTEN.with(|c| c.set(0));
}

fn tally_disk_write(bytes: u64) {
    DISK_BYTES_WRITTEN.with(|c| c.set(c.get() + bytes));
}

/// Default cap on total extracted bytes (decompression-bomb guard).
pub const MAX_EXTRACTED_BYTES: u64 = 8 * 1024 * 1024 * 1024;
/// Default cap on archive entries.
pub const MAX_ENTRIES: usize = 100_000;
/// Bytes staged per file write while extracting, and the size of the read
/// buffer under the zip reader. The archive readers hand out small reads
/// (the zip entry reader returns 8 KiB a call), and writing them as they
/// come costs tens of thousands of small file writes on a large archive;
/// staging them into writes of this size measured 2.2× faster end to end
/// (tests/install_probe.rs, docs/perf-results.md).
const ENTRY_COPY_BYTES: usize = 256 * 1024;

/// What to do with a pak that already occupies a slot the pack names.
///
/// [`SlotChoice::Move`] moves that pak to the lowest free number above the
/// pack, and the pack takes the slot. [`SlotChoice::Replace`] removes a
/// look-alike and moves every other occupant the same way, so a plain
/// foreign pak is never deleted. `None` at [`install_files_in_slots`] does
/// neither: the call returns [`InstallError::NeedsChoice`] and leaves every
/// file where it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotChoice {
    /// Move the occupant above the pack.
    Move,
    /// Remove look-alike occupants. Every other occupant is moved.
    Replace,
}

/// A pak sitting on a slot a pack install wants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotOccupant {
    /// `pakNN_dir.vpk` as it is on disk.
    pub file: String,
    pub slot: u32,
    /// The mod's name, or [`Self::file`] when the pak has no sidecar.
    pub label: String,
    /// The on-disk tree is at least half contained in the incoming pak, or
    /// each tree is at least three fifths of the other.
    pub look_alike: bool,
}

/// A pack install that finished, including which paks moved or were removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotPlaced {
    pub sidecar: Sidecar,
    /// `pak05_dir.vpk to pak08_dir.vpk`, for occupants that were moved.
    pub moved: Vec<String>,
    /// File names of occupants that were removed.
    pub replaced: Vec<String>,
}

/// The sentence a finished install shows. Empty when nothing moved or was removed.
#[must_use]
pub fn slot_note(placed: &SlotPlaced) -> String {
    let mut parts = Vec::new();
    if !placed.replaced.is_empty() {
        parts.push(format!("Replaced: {}.", placed.replaced.join(", ")));
    }
    if !placed.moved.is_empty() {
        parts.push(format!("Moved: {}.", placed.moved.join(", ")));
    }
    parts.join(" ")
}

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("Deadlock is running; close the game before changing mods")]
    GameRunning,
    #[error("unsupported file (expected a .vpk, or a zip, rar or 7z archive)")]
    UnsupportedArchive,
    #[error("the archive contains no .vpk file")]
    NoVpk,
    #[error("unsafe path in archive: {0}")]
    UnsafePath(String),
    #[error("the addons folder already holds 99 paks")]
    AddonsFull,
    #[error("archive expands beyond the extraction limit")]
    TooLarge,
    #[error("io error on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("zip error: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("rar error: {0}")]
    Rar(#[from] unrar::error::UnrarError),
    #[error("7z error: {0}")]
    SevenZ(#[from] sevenz_rust2::Error),
    #[error(transparent)]
    Addons(#[from] AddonsError),
    #[error(transparent)]
    Sidecar(#[from] SidecarError),
    #[error("only mods installed from a file or adopted can be updated from a file")]
    NotLocal,
    #[error("the new file has {new} VPK(s) but the mod has {old}; install it as a new mod instead")]
    VpkCountMismatch { old: usize, new: usize },
    /// A pak this mod does not own is on a slot the pack names. Nothing was renamed.
    #[error("other mods use slots this pack needs")]
    NeedsChoice(Vec<SlotOccupant>),
}

fn io_err(path: &Path) -> impl FnOnce(io::Error) -> InstallError + '_ {
    move |source| InstallError::Io {
        path: path.to_path_buf(),
        source,
    }
}

#[derive(Debug, Clone)]
pub struct ModMeta {
    pub mod_id: String,
    pub name: String,
    pub version: Option<String>,
    pub thumbnail_url: Option<String>,
    /// Marked NSFW in the catalog (recorded in the sidecar).
    pub nsfw: bool,
    pub hero: Option<String>,
    pub category: Option<String>,
}

/// Extraction limits; injectable so tests do not need huge archives.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_bytes: u64,
    pub max_entries: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_bytes: MAX_EXTRACTED_BYTES,
            max_entries: MAX_ENTRIES,
        }
    }
}

/// Install an archive ([`install_with`] with the real running-game check).
///
/// # Errors
///
/// As [`install_with_limits`].
pub fn install(archive: &Path, meta: ModMeta, paths: &GamePaths) -> Result<Sidecar, InstallError> {
    let guard = InstallGuard::acquire();
    install_with(archive, meta, paths, game::is_running, guard.permit())
}

/// [`install`] with an injectable running-game check.
///
/// # Errors
///
/// As [`install_with_limits`].
pub fn install_with(
    archive: &Path,
    meta: ModMeta,
    paths: &GamePaths,
    game_running: impl Fn() -> bool,
    permit: InstallPermit<'_>,
) -> Result<Sidecar, InstallError> {
    install_with_limits(
        archive,
        meta,
        paths,
        game_running,
        Limits::default(),
        permit,
    )
}

/// [`install_with`] with explicit extraction limits. Nothing is written
/// while the game runs; a refusal leaves the disk untouched.
///
/// # Errors
///
/// [`InstallError::GameRunning`] before anything is written;
/// [`InstallError::UnsupportedArchive`] when the file is no recognised
/// archive; [`InstallError::NoVpk`] when it holds no VPK;
/// [`InstallError::UnsafePath`] for an archive entry that escapes staging;
/// [`InstallError::TooLarge`] beyond `limits`; [`InstallError::AddonsFull`]
/// when every pak number is taken; the archive, io, addons and sidecar
/// errors for their respective failures.
pub fn install_with_limits(
    archive: &Path,
    meta: ModMeta,
    paths: &GamePaths,
    game_running: impl Fn() -> bool,
    limits: Limits,
    permit: InstallPermit<'_>,
) -> Result<Sidecar, InstallError> {
    permit.keep();
    // Checked first: nothing may be written while the game runs.
    if game_running() {
        return Err(InstallError::GameRunning);
    }
    let addons_dir = paths.addons.as_path();
    clean_stale_staging(addons_dir);
    let format = detect_format(archive)?;

    // Also validates the mod id before any file is written. A corrupt old sidecar
    // is treated as absent so it cannot block reinstalling forever.
    let old = match sidecar::read(addons_dir, &meta.mod_id) {
        Ok(s) => s,
        Err(SidecarError::Parse { .. }) => None,
        Err(e) => return Err(e.into()),
    };

    let staging = Staging::create(addons_dir)?;
    extract(format, archive, staging.path(), limits)?;
    let staged = collect_vpks(staging.path())?;
    place_and_record(addons_dir, &staged, meta, old)
}

/// Install several plain VPK files (already extracted, for example from an
/// interchange bundle) as one mod. Works like [`install_with`], minus the
/// archive step: the files are copied into staging under their own names,
/// so a pak-named VPK keeps its chunks (its `pakNN_<digits>.vpk` siblings
/// must be in `files` too).
///
/// # Errors
///
/// [`InstallError::GameRunning`] before anything is written;
/// [`InstallError::NoVpk`] when no file is a VPK; the io, addons and
/// sidecar errors as for [`install_with_limits`].
pub fn install_files(
    files: &[PathBuf],
    meta: ModMeta,
    paths: &GamePaths,
    game_running: impl Fn() -> bool,
    permit: InstallPermit<'_>,
) -> Result<Sidecar, InstallError> {
    install_files_to(files, meta, paths, game_running, None, None, permit)
        .map(|placed| placed.sidecar)
}

/// Like [`install_files`], for a pack whose paks have a fixed load order
/// (CompLock: HUD++ is pak01): every VPK named `pakNN_dir.vpk` with NN in
/// `1..=reserved` goes to slot NN, whatever order the pack's paks are
/// installed in. A VPK of this same mod already in a wanted slot is moved
/// aside and then dropped with the old version. A VPK of another mod is
/// left untouched when `choice` is `None`, and the call returns
/// [`InstallError::NeedsChoice`]. [`SlotChoice::Move`] moves that pak to the
/// lowest free slot above `reserved` (its sidecar kept in step).
/// [`SlotChoice::Replace`] removes a look-alike and moves every other
/// occupant the same way. Other VPKs of the pack get free slots above
/// `reserved`. `permit`
/// is the install lock.
///
/// # Errors
///
/// As [`install_files`]; [`InstallError::NeedsChoice`] before anything is
/// renamed; [`InstallError::AddonsFull`] when nothing above `reserved` is
/// free for a VPK that has to move.
pub fn install_files_in_slots(
    files: &[PathBuf],
    meta: ModMeta,
    paths: &GamePaths,
    game_running: impl Fn() -> bool,
    reserved: u32,
    choice: Option<SlotChoice>,
    permit: InstallPermit<'_>,
) -> Result<SlotPlaced, InstallError> {
    if game_running() {
        return Err(InstallError::GameRunning);
    }
    if choice.is_none() {
        let occupants = foreign_occupants(&paths.addons, files, &meta.mod_id, reserved)?;
        if !occupants.is_empty() {
            return Err(InstallError::NeedsChoice(occupants));
        }
    }
    install_files_to(
        files,
        meta,
        paths,
        game_running,
        Some(reserved),
        choice,
        permit,
    )
}

fn install_files_to(
    files: &[PathBuf],
    meta: ModMeta,
    paths: &GamePaths,
    game_running: impl Fn() -> bool,
    reserved: Option<u32>,
    choice: Option<SlotChoice>,
    permit: InstallPermit<'_>,
) -> Result<SlotPlaced, InstallError> {
    // Checked first: nothing may be written while the game runs.
    if game_running() {
        return Err(InstallError::GameRunning);
    }
    let addons_dir = paths.addons.as_path();
    clean_stale_staging(addons_dir);
    let old = match sidecar::read(addons_dir, &meta.mod_id) {
        Ok(s) => s,
        Err(SidecarError::Parse { .. }) => None,
        Err(e) => return Err(e.into()),
    };

    let staging = Staging::create(addons_dir)?;
    for file in files {
        let name = file
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .ok_or(InstallError::UnsupportedArchive)?;
        if !is_plain_vpk_name(name) {
            return Err(InstallError::UnsupportedArchive);
        }
        let copied = fs::copy(file, staging.path().join(name)).map_err(io_err(file))?;
        tally_disk_write(copied);
    }
    let staged = collect_vpks(staging.path())?;
    match reserved {
        None => {
            permit.keep();
            place_and_record(addons_dir, &staged, meta, old).map(|sidecar| SlotPlaced {
                sidecar,
                moved: Vec::new(),
                replaced: Vec::new(),
            })
        }
        Some(reserved) => place_in_slots(addons_dir, &staged, meta, reserved, choice, permit),
    }
}

/// The slots `staged` wants under `reserved`: its own `pakNN` where NN is
/// within the pack's range, else `None` for a free slot above it.
fn wanted_slots(staged: &[StagedVpk], reserved: u32) -> Vec<Option<u32>> {
    staged
        .iter()
        .map(|v| v.original.filter(|n| (1..=reserved).contains(n)))
        .collect()
}

/// [`install_files_in_slots`]'s placement. `choice` of `None` with a foreign
/// pak on a wanted slot returns [`InstallError::NeedsChoice`] before a rename.
fn place_in_slots(
    addons_dir: &Path,
    staged: &[StagedVpk],
    meta: ModMeta,
    reserved: u32,
    choice: Option<SlotChoice>,
    permit: InstallPermit<'_>,
) -> Result<SlotPlaced, InstallError> {
    if staged.is_empty() {
        return Err(InstallError::NoVpk);
    }
    let wanted = wanted_slots(staged, reserved);
    let claimed: BTreeSet<u32> = wanted.iter().flatten().copied().collect();
    let incoming = staged_by_slot(staged, reserved);
    let mut mods = sidecar::group(addons_dir);
    let occupants = occupants_in(&mods, &incoming, &meta.mod_id, &claimed);
    if !occupants.is_empty() && choice.is_none() {
        return Err(InstallError::NeedsChoice(occupants));
    }
    let replaced = if choice == Some(SlotChoice::Replace) {
        let look_alikes: Vec<SlotOccupant> = occupants
            .iter()
            .filter(|occupant| occupant.look_alike)
            .cloned()
            .collect();
        let names = remove_occupants(addons_dir, &mods, &look_alikes)?;
        mods = sidecar::group(addons_dir);
        names
    } else {
        Vec::new()
    };
    let foreign_slots: BTreeSet<u32> = occupants.iter().map(|o| o.slot).collect();
    let mut used = crate::pak_slot::occupied(addons_dir)?;
    used.extend(&claimed);
    let mut above = (reserved + 1..=addons::PAK_CAPACITY).filter(|n| !used.contains(n));
    let mut moves: Vec<(&addons::InstalledVpk, u32)> = Vec::new();
    let mut moved = Vec::new();
    for vpk in mods.iter().flat_map(|m| &m.vpks) {
        if vpk.pak.is_some_and(|n| claimed.contains(&n)) {
            let to = above.next().ok_or(InstallError::AddonsFull)?;
            if vpk.pak.is_some_and(|n| foreign_slots.contains(&n))
                && let Some(name) = vpk.file.file_name().and_then(|n| n.to_str())
            {
                moved.push(format!("{name} to pak{to:02}_dir.vpk"));
            }
            moves.push((vpk, to));
        }
    }
    let mut numbers = Vec::with_capacity(staged.len());
    for slot in &wanted {
        numbers.push(match slot {
            Some(n) => *n,
            None => above.next().ok_or(InstallError::AddonsFull)?,
        });
    }
    sidecar::renumber(addons_dir, &mods, &moves, permit)?;
    // Moving aside may have renamed this mod's own earlier files.
    let old = match sidecar::read(addons_dir, &meta.mod_id) {
        Ok(s) => s,
        Err(SidecarError::Parse { .. }) => None,
        Err(e) => return Err(e.into()),
    };
    let sidecar = place_numbered(addons_dir, staged, &numbers, meta, old)?;
    Ok(SlotPlaced {
        sidecar,
        moved,
        replaced,
    })
}

fn staged_by_slot(staged: &[StagedVpk], reserved: u32) -> BTreeMap<u32, &Path> {
    staged
        .iter()
        .filter_map(|v| {
            v.original
                .filter(|n| (1..=reserved).contains(n))
                .map(|n| (n, v.file.as_path()))
        })
        .collect()
}

fn foreign_occupants(
    addons_dir: &Path,
    files: &[PathBuf],
    mod_id: &str,
    reserved: u32,
) -> Result<Vec<SlotOccupant>, InstallError> {
    let incoming: BTreeMap<u32, &Path> = files
        .iter()
        .filter_map(|file| {
            let name = file.file_name().and_then(|n| n.to_str())?;
            let (n, kind) = parse_pak(name)?;
            matches!(kind, PakKind::Dir)
                .then_some(n)
                .filter(|n| (1..=reserved).contains(n))
                .map(|n| (n, file.as_path()))
        })
        .collect();
    let claimed: BTreeSet<u32> = incoming.keys().copied().collect();
    Ok(occupants_in(
        &sidecar::group(addons_dir),
        &incoming,
        mod_id,
        &claimed,
    ))
}

fn occupants_in(
    mods: &[InstalledMod],
    incoming: &BTreeMap<u32, &Path>,
    mod_id: &str,
    claimed: &BTreeSet<u32>,
) -> Vec<SlotOccupant> {
    let mut occupants = Vec::new();
    for m in mods {
        if m.sidecar.as_ref().is_some_and(|s| s.mod_id == mod_id) {
            continue;
        }
        let label = m.sidecar.as_ref().map(|s| s.name.clone());
        for vpk in &m.vpks {
            let Some(slot) = vpk.pak.filter(|n| claimed.contains(n)) else {
                continue;
            };
            let Some(file) = vpk.file.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let remote = incoming
                .get(&slot)
                .map(|p| vpk_paths(p))
                .unwrap_or_default();
            occupants.push(SlotOccupant {
                file: file.to_owned(),
                slot,
                label: label.clone().unwrap_or_else(|| file.to_owned()),
                look_alike: look_alike(&vpk_paths(&vpk.file), &remote),
            });
        }
    }
    occupants.sort_by_key(|o| o.slot);
    occupants
}

fn remove_occupants(
    addons_dir: &Path,
    mods: &[InstalledMod],
    occupants: &[SlotOccupant],
) -> Result<Vec<String>, InstallError> {
    let slots: BTreeSet<u32> = occupants.iter().map(|o| o.slot).collect();
    let mut replaced = Vec::new();
    for m in mods {
        let mut dropped = false;
        for vpk in &m.vpks {
            if vpk.pak.is_some_and(|n| slots.contains(&n)) {
                remove_pak_file(&vpk.file)?;
                for chunk in &vpk.chunks {
                    remove_pak_file(chunk)?;
                }
                if let Some(name) = vpk.file.file_name().and_then(|n| n.to_str()) {
                    replaced.push(name.to_owned());
                }
                dropped = true;
            }
        }
        let Some(sidecar) = &m.sidecar else {
            continue;
        };
        if !dropped {
            continue;
        }
        let mut updated = sidecar.clone();
        updated
            .vpks
            .retain(|name| parse_pak(name).is_none_or(|(n, _)| !slots.contains(&n)));
        if updated.vpks.is_empty() {
            sidecar::remove(addons_dir, &sidecar.mod_id)?;
        } else if updated.vpks.len() != sidecar.vpks.len() {
            sidecar::write(addons_dir, &updated)?;
        }
    }
    Ok(replaced)
}

fn remove_pak_file(path: &Path) -> Result<(), InstallError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(io_err(path)(e)),
    }
}

/// The shared tail of the install paths: give the staged VPKs free pak
/// slots, move them in and record the sidecar, then drop the replaced
/// files of an earlier version of the same mod.
fn place_and_record(
    addons_dir: &Path,
    staged: &[StagedVpk],
    meta: ModMeta,
    old: Option<Sidecar>,
) -> Result<Sidecar, InstallError> {
    if staged.is_empty() {
        return Err(InstallError::NoVpk);
    }
    let numbers = allocate_paks(addons_dir, staged.len())?;
    place_numbered(addons_dir, staged, &numbers, meta, old)
}

/// Move `staged` in on `numbers` (one per VPK), record the sidecar, then
/// drop the replaced files of `old`.
fn place_numbered(
    addons_dir: &Path,
    staged: &[StagedVpk],
    numbers: &[u32],
    meta: ModMeta,
    old: Option<Sidecar>,
) -> Result<Sidecar, InstallError> {
    let mut plan: Vec<(PathBuf, PathBuf)> = Vec::new();
    let mut new_names = Vec::with_capacity(staged.len());
    let mut placed: Vec<Vec<PathBuf>> = Vec::with_capacity(staged.len());
    for (vpk, number) in staged.iter().zip(numbers) {
        let dir_name = format!("pak{number:02}_dir.vpk");
        let mut files = vec![addons_dir.join(&dir_name)];
        plan.push((vpk.file.clone(), addons_dir.join(&dir_name)));
        for (digits, chunk) in &vpk.chunks {
            let to = addons_dir.join(format!("pak{number:02}_{digits}.vpk"));
            files.push(to.clone());
            plan.push((chunk.clone(), to));
        }
        new_names.push(dir_name);
        placed.push(files);
    }

    let mut moved: Vec<PathBuf> = Vec::new();
    let rollback = |moved: &[PathBuf]| {
        for p in moved {
            let _ = fs::remove_file(p);
        }
    };
    for (from, to) in &plan {
        if let Err(e) = move_into_place(from, to) {
            rollback(&moved);
            return Err(e);
        }
        moved.push(to.clone());
    }

    let stamps = match placed
        .iter()
        .map(|files| sidecar::Stamp::of_files(files.iter().map(PathBuf::as_path)))
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(stamps) => stamps,
        Err(e) => {
            rollback(&moved);
            return Err(e.into());
        }
    };
    let new_sidecar = Sidecar {
        mod_id: meta.mod_id,
        name: meta.name,
        version: meta.version,
        thumbnail_url: meta.thumbnail_url,
        vpks: new_names,
        nsfw: Some(meta.nsfw),
        hero: meta.hero,
        category: meta.category,
        notes: None,
        source: None,
        stamps: Some(stamps),
    };
    if let Err(e) = sidecar::write(addons_dir, &new_sidecar) {
        rollback(&moved);
        return Err(e.into());
    }

    // The new mod is complete and recorded; only now drop the files of the old one.
    // Best effort: the new mod is fully installed, so leftovers must not fail the call.
    if let Some(old) = old {
        for name in &old.vpks {
            if !new_sidecar.vpks.contains(name) {
                let _ = remove_pak_files(addons_dir, name);
            }
        }
    }
    Ok(new_sidecar)
}

/// Install a local `.vpk` file, or a zip, rar or 7z archive holding VPKs, as
/// a local mod named `name` (see [`sidecar::LOCAL_PREFIX`]). The source file
/// is copied or extracted, never moved or changed; it gets free pak slots
/// exactly like a catalog download.
///
/// # Errors
///
/// As [`install_with_limits`].
pub fn install_local(file: &Path, name: &str, paths: &GamePaths) -> Result<Sidecar, InstallError> {
    let guard = InstallGuard::acquire();
    install_local_with(file, name, paths, game::is_running, guard.permit())
}

/// [`install_local`] with an injectable running-game check.
///
/// # Errors
///
/// As [`install_with_limits`].
pub fn install_local_with(
    file: &Path,
    name: &str,
    paths: &GamePaths,
    game_running: impl Fn() -> bool,
    permit: InstallPermit<'_>,
) -> Result<Sidecar, InstallError> {
    let fallback = file
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let name = match name.trim() {
        "" => fallback,
        n => n.to_owned(),
    };
    let meta = ModMeta {
        mod_id: sidecar::new_local_id(&paths.addons),
        name,
        version: None,
        thumbnail_url: None,
        nsfw: false,
        hero: None,
        category: None,
    };
    let mut sc = install_with(file, meta, paths, game_running, permit)?;
    // Recorded so a newer copy of the same file is recognised as an update.
    if let Some(source) = source_name(file) {
        sc.source = Some(source);
        // Best effort: the mod is installed; only update matching gets weaker.
        let _ = sidecar::write(&paths.addons, &sc);
    }
    Ok(sc)
}

/// Remove all of a mod's VPK files (including disabled forms and chunk files when it
/// has a sidecar) and its sidecar. A mod without a sidecar loses only its own VPK files.
///
/// # Errors
///
/// [`InstallError::GameRunning`] before anything is removed;
/// [`InstallError::Io`] when a removal fails.
pub fn uninstall(m: &InstalledMod) -> Result<(), InstallError> {
    let guard = InstallGuard::acquire();
    uninstall_with(m, game::is_running, guard.permit())
}

/// [`uninstall`] with an injectable running-game check; refuses before touching anything.
///
/// # Errors
///
/// As [`uninstall`].
pub fn uninstall_with(
    m: &InstalledMod,
    game_running: impl Fn() -> bool,
    permit: InstallPermit<'_>,
) -> Result<(), InstallError> {
    permit.keep();
    if game_running() {
        return Err(InstallError::GameRunning);
    }
    let Some(sc) = &m.sidecar else {
        for vpk in &m.vpks {
            let name = vpk.file.file_name().and_then(|n| n.to_str());
            match (name, vpk.file.parent()) {
                (Some(n), Some(dir)) => {
                    remove_pak_files(dir, n.strip_suffix(DISABLED_EXT).unwrap_or(n))?;
                }
                _ => remove_if_exists(&vpk.file)?,
            }
        }
        return Ok(());
    };
    // Validates the id too, so it cannot be used to escape `.shiv`.
    let valid_id = !sc.mod_id.is_empty()
        && sc
            .mod_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if !valid_id {
        return Err(SidecarError::InvalidModId(sc.mod_id.clone()).into());
    }
    let Some(dir) = m.vpks.first().and_then(|v| v.file.parent()) else {
        return Ok(());
    };
    // Sidecar content is untrusted: only plain `*.vpk` names inside `dir` are acted on.
    let mut names: BTreeSet<String> = sc
        .vpks
        .iter()
        .filter(|n| is_plain_vpk_name(n))
        .cloned()
        .collect();
    for vpk in &m.vpks {
        if let Some(n) = vpk.file.file_name().and_then(|n| n.to_str()) {
            names.insert(n.strip_suffix(DISABLED_EXT).unwrap_or(n).to_owned());
        }
    }
    for name in &names {
        remove_pak_files(dir, name)?;
    }
    remove_if_exists(&dir.join(SHIV_DIR).join(format!("{}.json", sc.mod_id)))
}

fn remove_if_exists(path: &Path) -> Result<(), InstallError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(io_err(path)(e)),
    }
}

/// Remove `name` (enabled form), its `.disabled` form and, for `pakNN_dir.vpk`, the
/// `pakNN_<digits>.vpk[.disabled]` chunk files next to it.
fn remove_pak_files(dir: &Path, name: &str) -> Result<(), InstallError> {
    if !is_plain_vpk_name(name) {
        return Ok(());
    }
    remove_if_exists(&dir.join(name))?;
    remove_if_exists(&dir.join(format!("{name}{DISABLED_EXT}")))?;
    if let Some((number, PakKind::Dir)) = parse_pak(name) {
        let entries = match fs::read_dir(dir) {
            Ok(e) => e,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(io_err(dir)(e)),
        };
        for entry in entries {
            let entry = entry.map_err(io_err(dir))?;
            let file_name = entry.file_name();
            let Some(n) = file_name.to_str() else {
                continue;
            };
            let enabled = n.strip_suffix(DISABLED_EXT).unwrap_or(n);
            if matches!(parse_pak(enabled), Some((c, PakKind::Chunk(_))) if c == number) {
                remove_if_exists(&entry.path())?;
            }
        }
    }
    Ok(())
}

/// A single plain path component ending in `.vpk`: no separators, `..`, or absolute paths.
fn is_plain_vpk_name(name: &str) -> bool {
    let mut comps = Path::new(name).components();
    let plain_component = matches!(
        (comps.next(), comps.next()),
        (Some(Component::Normal(c)), None) if c == name
    );
    let vpk_extension = Path::new(name)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("vpk"));
    plain_component && !name.contains('\\') && vpk_extension
}

/// Remove staging dirs left by a crashed run: owned by another process and over an hour old.
fn clean_stale_staging(addons_dir: &Path) {
    const MAX_AGE: Duration = Duration::from_secs(3600);
    let tmp_root = addons_dir.join(SHIV_DIR).join(TMP_DIR);
    let Ok(entries) = fs::read_dir(&tmp_root) else {
        return;
    };
    let me = std::process::id().to_string();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.split('-').next() == Some(me.as_str()) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        let old = meta
            .modified()
            .ok()
            .and_then(|m| m.elapsed().ok())
            .is_some_and(|age| age > MAX_AGE);
        if meta.is_dir() && old {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
    let _ = fs::remove_dir(&tmp_root);
    let _ = fs::remove_dir(addons_dir.join(SHIV_DIR));
}

// ---- pak names ----

fn allocate_paks(addons_dir: &Path, count: usize) -> Result<Vec<u32>, InstallError> {
    match crate::pak_slot::free_slots(addons_dir, count) {
        Ok(numbers) => Ok(numbers),
        Err(AddonsError::Full { .. }) => Err(InstallError::AddonsFull),
        Err(e) => Err(e.into()),
    }
}

fn move_into_place(from: &Path, to: &Path) -> Result<(), InstallError> {
    let disabled = PathBuf::from(format!("{}{DISABLED_EXT}", to.display()));
    if to.exists() || disabled.exists() {
        return Err(io_err(to)(io::Error::from(io::ErrorKind::AlreadyExists)));
    }
    fs::rename(from, to).map_err(io_err(to))
}

// ---- staging ----

/// A unique directory under `<addons>/.shiv/tmp`, removed (with now-empty parents) on drop.
struct Staging {
    path: PathBuf,
    tmp_root: PathBuf,
    shiv_dir: PathBuf,
}

impl Staging {
    fn create(addons_dir: &Path) -> Result<Self, InstallError> {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let unique = format!(
            "{}-{nanos}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let shiv_dir = addons_dir.join(SHIV_DIR);
        let tmp_root = shiv_dir.join(TMP_DIR);
        let path = tmp_root.join(unique);
        let staging = Self {
            path,
            tmp_root,
            shiv_dir,
        };
        // Drop cleans up even when creation half-succeeds.
        fs::create_dir_all(&staging.path).map_err(io_err(&staging.path))?;
        Ok(staging)
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
        // Fail harmlessly when other staging dirs or sidecars are still there.
        let _ = fs::remove_dir(&self.tmp_root);
        let _ = fs::remove_dir(&self.shiv_dir);
    }
}

// ---- archive detection and extraction ----

#[derive(Debug, Clone, Copy)]
enum Format {
    Zip,
    Rar,
    SevenZ,
    /// A bare VPK file, not an archive.
    Vpk,
}

/// Valve's VPK header signature, 0x55aa1234 little-endian.
const VPK_SIGNATURE: [u8; 4] = [0x34, 0x12, 0xaa, 0x55];

fn detect_format(archive: &Path) -> Result<Format, InstallError> {
    let mut magic = [0u8; 6];
    let mut f = File::open(archive).map_err(io_err(archive))?;
    let mut filled = 0;
    while filled < magic.len() {
        match f.read(&mut magic[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(io_err(archive)(e)),
        }
    }
    let head = &magic[..filled];
    if head.starts_with(b"PK\x03\x04") {
        Ok(Format::Zip)
    } else if head.starts_with(b"Rar!\x1a\x07") {
        Ok(Format::Rar)
    } else if head.starts_with(b"7z\xbc\xaf\x27\x1c") {
        Ok(Format::SevenZ)
    } else if head.starts_with(&VPK_SIGNATURE) {
        Ok(Format::Vpk)
    } else {
        Err(InstallError::UnsupportedArchive)
    }
}

/// Bytes still allowed to be extracted; `exceeded` records that the cap was the cause
/// of a read error so it can be reported as [`InstallError::TooLarge`].
struct Budget {
    remaining: u64,
    exceeded: bool,
}

impl Budget {
    fn charge(&mut self, bytes: u64) -> Result<(), InstallError> {
        if let Some(r) = self.remaining.checked_sub(bytes) {
            self.remaining = r;
            Ok(())
        } else {
            self.exceeded = true;
            Err(InstallError::TooLarge)
        }
    }
}

struct Budgeted<'a, R> {
    inner: R,
    budget: &'a mut Budget,
}

impl<R: Read> Read for Budgeted<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        if self.budget.charge(n as u64).is_err() {
            return Err(io::Error::other("extraction size limit exceeded"));
        }
        Ok(n)
    }
}

/// Copy `reader` into a new file at `dest` (creating parents), counting
/// the budget. `scratch` is the write buffer, reused across entries: the
/// archive readers hand out small reads (the zip entry reader returns
/// 8 KiB a call), and staging them into full scratch-sized writes keeps a
/// large archive's file writes in the hundreds, not the tens of
/// thousands (docs/perf-results.md).
fn write_entry(
    reader: impl Read,
    dest: &Path,
    budget: &mut Budget,
    scratch: &mut [u8],
) -> Result<(), InstallError> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(io_err(parent))?;
    }
    let mut out = File::create(dest).map_err(io_err(dest))?;
    let mut limited = Budgeted {
        inner: reader,
        budget,
    };
    let mut staged = 0usize;
    let outcome = loop {
        if staged == scratch.len() {
            if let Err(e) = out.write_all(scratch) {
                break Err(e);
            }
            tally_disk_write(scratch.len() as u64);
            staged = 0;
        }
        match limited.read(&mut scratch[staged..]) {
            Ok(0) => {
                if staged > 0 {
                    if let Err(e) = out.write_all(&scratch[..staged]) {
                        break Err(e);
                    }
                    tally_disk_write(staged as u64);
                }
                break Ok(());
            }
            Ok(n) => staged += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => break Err(e),
        }
    };
    match outcome {
        Err(_) if limited.budget.exceeded => Err(InstallError::TooLarge),
        Err(e) => Err(io_err(dest)(e)),
        Ok(()) => Ok(()),
    }
}

fn is_vpk_name(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".vpk")
}

fn starts_with_drive_letter(component: &str) -> bool {
    let b = component.as_bytes();
    b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
}

/// Interpret an entry name as a relative path of plain names. Backslashes are
/// separators on every platform. Absolute paths, `..`, drive/prefix components
/// (anywhere) and empty names are rejected.
fn relative_entry_path(entry_name: &str, allow_empty: bool) -> Result<PathBuf, InstallError> {
    let unsafe_path = || InstallError::UnsafePath(entry_name.to_owned());
    let normalized = entry_name.replace('\\', "/");
    let mut relative = PathBuf::new();
    for component in Path::new(&normalized).components() {
        match component {
            Component::Normal(part) if starts_with_drive_letter(&part.to_string_lossy()) => {
                return Err(unsafe_path());
            }
            Component::Normal(part) => relative.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(unsafe_path());
            }
        }
    }
    if relative.as_os_str().is_empty() && !allow_empty {
        return Err(unsafe_path());
    }
    Ok(relative)
}

const S_IFMT: u32 = 0o170_000;
const S_IFLNK: u32 = 0o120_000;

fn extract(
    format: Format,
    archive: &Path,
    dest: &Path,
    limits: Limits,
) -> Result<(), InstallError> {
    let mut budget = Budget {
        remaining: limits.max_bytes,
        exceeded: false,
    };
    match format {
        Format::Zip => extract_zip(archive, dest, limits, &mut budget),
        Format::Rar => extract_rar(archive, dest, limits, &mut budget),
        Format::SevenZ => extract_7z(archive, dest, limits, &mut budget),
        Format::Vpk => copy_vpk(archive, dest, &mut budget),
    }
}

/// Stage a bare VPK by copying it (the source stays untouched). A
/// `pakNN_dir.vpk` brings its `pakNN_<digits>.vpk` chunk files along.
fn copy_vpk(file: &Path, dest: &Path, budget: &mut Budget) -> Result<(), InstallError> {
    let name = file
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| is_vpk_name(n))
        .unwrap_or("mod.vpk")
        .to_owned();
    let mut sources = vec![(file.to_path_buf(), name.clone())];
    if let (Some((number, PakKind::Dir)), Some(dir)) = (parse_pak(&name), file.parent()) {
        for entry in fs::read_dir(dir).map_err(io_err(dir))?.flatten() {
            let Some(n) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if matches!(parse_pak(&n), Some((c, PakKind::Chunk(_))) if c == number) {
                sources.push((entry.path(), n));
            }
        }
    }
    for (from, n) in sources {
        let len = fs::metadata(&from).map_err(io_err(&from))?.len();
        budget.charge(len)?;
        let to = dest.join(&n);
        let copied = fs::copy(&from, &to).map_err(io_err(&to))?;
        tally_disk_write(copied);
    }
    Ok(())
}

/// Counts listed entries against the cap.
fn count_entry(seen: &mut usize, limits: Limits) -> Result<(), InstallError> {
    *seen += 1;
    if *seen > limits.max_entries {
        return Err(InstallError::TooLarge);
    }
    Ok(())
}

fn extract_zip(
    archive: &Path,
    dest: &Path,
    limits: Limits,
    budget: &mut Budget,
) -> Result<(), InstallError> {
    let mut scratch = vec![0u8; ENTRY_COPY_BYTES];
    let file = File::open(archive).map_err(io_err(archive))?;
    let mut zip = zip::ZipArchive::new(BufReader::with_capacity(scratch.len(), file))?;

    // Pass 1: validate every entry and the declared sizes before writing anything.
    let mut seen = 0;
    let mut declared: u64 = 0;
    for i in 0..zip.len() {
        let entry = zip.by_index_raw(i)?;
        count_entry(&mut seen, limits)?;
        relative_entry_path(entry.name(), entry.is_dir())?;
        if entry.unix_mode().is_some_and(|m| m & S_IFMT == S_IFLNK) {
            return Err(InstallError::UnsafePath(format!(
                "symlink entry {}",
                entry.name()
            )));
        }
        if !entry.is_dir() && is_vpk_name(entry.name()) {
            declared = declared.saturating_add(entry.size());
        }
    }
    if declared > limits.max_bytes {
        return Err(InstallError::TooLarge);
    }

    // Pass 2: extract `.vpk` entries.
    for i in 0..zip.len() {
        let entry = zip.by_index(i)?;
        if entry.is_dir() || !is_vpk_name(entry.name()) {
            continue;
        }
        let dest_path = dest.join(relative_entry_path(entry.name(), false)?);
        write_entry(entry, &dest_path, budget, &mut scratch)?;
    }
    Ok(())
}

fn extract_7z(
    archive: &Path,
    dest: &Path,
    limits: Limits,
    budget: &mut Budget,
) -> Result<(), InstallError> {
    let mut reader = sevenz_rust2::ArchiveReader::open(archive, sevenz_rust2::Password::empty())?;

    let mut seen = 0;
    let mut declared: u64 = 0;
    for entry in &reader.archive().files {
        count_entry(&mut seen, limits)?;
        relative_entry_path(entry.name(), entry.is_directory())?;
        let attrs = entry.windows_attributes();
        let reparse = attrs & 0x400 != 0;
        let unix_symlink = attrs & 0x8000 != 0 && (attrs >> 16) & S_IFMT == S_IFLNK;
        if entry.has_windows_attributes && (reparse || unix_symlink) {
            return Err(InstallError::UnsafePath(format!(
                "symlink entry {}",
                entry.name()
            )));
        }
        if !entry.is_directory() && is_vpk_name(entry.name()) {
            declared = declared.saturating_add(entry.size());
        }
    }
    if declared > limits.max_bytes {
        return Err(InstallError::TooLarge);
    }

    let mut failure: Option<InstallError> = None;
    let mut scratch = vec![0u8; ENTRY_COPY_BYTES];
    let result = reader.for_each_entries(|entry, data| {
        if entry.is_directory() || entry.is_anti_item() || !is_vpk_name(entry.name()) {
            return Ok(true);
        }
        let outcome = relative_entry_path(entry.name(), false)
            .and_then(|rel| write_entry(data, &dest.join(rel), budget, &mut scratch));
        match outcome {
            Ok(()) => Ok(true),
            Err(e) => {
                failure = Some(e);
                Err(sevenz_rust2::Error::Other("extraction aborted".into()))
            }
        }
    });
    if let Some(e) = failure {
        return Err(e);
    }
    result?;
    Ok(())
}

fn extract_rar(
    archive: &Path,
    dest: &Path,
    limits: Limits,
    budget: &mut Budget,
) -> Result<(), InstallError> {
    // Pass 1: list and validate.
    let mut seen = 0;
    let mut declared: u64 = 0;
    for header in unrar::Archive::new(archive).open_for_listing()? {
        let header = header?;
        let name = header.filename.to_string_lossy().into_owned();
        count_entry(&mut seen, limits)?;
        relative_entry_path(&name, header.is_directory())?;
        // Windows hosts store attribute flags (no file-type bits); 0x400 is the reparse point.
        let reparse = header.file_attr & S_IFMT == 0 && header.file_attr & 0x400 != 0;
        if header.file_attr & S_IFMT == S_IFLNK || reparse {
            return Err(InstallError::UnsafePath(format!("symlink entry {name}")));
        }
        if header.is_file() && is_vpk_name(&name) {
            declared = declared.saturating_add(header.unpacked_size);
        }
    }
    if declared > limits.max_bytes {
        return Err(InstallError::TooLarge);
    }

    // Pass 2: extract `.vpk` files to the paths validated above.
    let mut open = unrar::Archive::new(archive).open_for_processing()?;
    while let Some(header) = open.read_header()? {
        let name = header.entry().filename.to_string_lossy().into_owned();
        open = if header.entry().is_file() && is_vpk_name(&name) {
            let target = dest.join(relative_entry_path(&name, false)?);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(io_err(parent))?;
                ensure_no_symlink_chain(dest, parent)?;
            }
            let next = header.extract_to(&target)?;
            // Header metadata is untrusted: check what the library really wrote.
            let meta = fs::symlink_metadata(&target).map_err(io_err(&target))?;
            if !meta.file_type().is_file() {
                let _ = fs::remove_file(&target);
                return Err(InstallError::UnsafePath(format!(
                    "non-regular entry {name}"
                )));
            }
            budget.charge(meta.len())?;
            tally_disk_write(meta.len());
            next
        } else {
            header.skip()?
        };
    }
    Ok(())
}

// ---- staged VPK discovery ----

struct StagedVpk {
    file: PathBuf,
    /// `pakNN` number it had in the archive, if it followed the naming.
    original: Option<u32>,
    sort_name: String,
    /// `(digits, path)` of `pakNN_<digits>.vpk` siblings that move with it.
    chunks: Vec<(String, PathBuf)>,
}

fn collect_vpks(root: &Path) -> Result<Vec<StagedVpk>, InstallError> {
    let mut files: Vec<PathBuf> = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).map_err(io_err(&dir))? {
            let entry = entry.map_err(io_err(&dir))?;
            let kind = entry.file_type().map_err(io_err(&dir))?;
            if kind.is_dir() {
                stack.push(entry.path());
            } else if kind.is_file() {
                files.push(entry.path());
            }
        }
    }

    // Chunks by (directory, pak number).
    let mut chunks: BTreeMap<(PathBuf, u32), Vec<(String, PathBuf)>> = BTreeMap::new();
    let mut dirs: Vec<StagedVpk> = Vec::new();
    for file in files {
        let Some(name) = file.file_name().and_then(|n| n.to_str()).map(str::to_owned) else {
            continue;
        };
        let parent = file.parent().map(Path::to_path_buf).unwrap_or_default();
        match parse_pak(&name) {
            Some((num, PakKind::Chunk(digits))) => {
                chunks
                    .entry((parent, num))
                    .or_default()
                    .push((digits, file));
            }
            Some((num, PakKind::Dir)) => dirs.push(StagedVpk {
                original: Some(num),
                sort_name: name.to_ascii_lowercase(),
                file,
                chunks: Vec::new(),
            }),
            None if is_vpk_name(&name) => dirs.push(StagedVpk {
                original: None,
                sort_name: name.to_ascii_lowercase(),
                file,
                chunks: Vec::new(),
            }),
            None => {}
        }
    }
    for vpk in &mut dirs {
        if let (Some(num), Some(parent)) = (vpk.original, vpk.file.parent())
            && let Some(mut found) = chunks.remove(&(parent.to_path_buf(), num))
        {
            found.sort();
            vpk.chunks = found;
        }
    }
    dirs.sort_by(|a, b| {
        (a.original.unwrap_or(u32::MAX), &a.sort_name, &a.file).cmp(&(
            b.original.unwrap_or(u32::MAX),
            &b.sort_name,
            &b.file,
        ))
    });
    Ok(dirs)
}

/// No existing directory between `root` (exclusive) and `dir` (inclusive) may be a symlink.
fn ensure_no_symlink_chain(root: &Path, dir: &Path) -> Result<(), InstallError> {
    let rel = dir.strip_prefix(root).unwrap_or(dir);
    let mut cur = root.to_path_buf();
    for c in rel.components() {
        cur.push(c);
        match fs::symlink_metadata(&cur) {
            Ok(m) if m.file_type().is_symlink() => {
                return Err(InstallError::UnsafePath(format!(
                    "symlinked directory {}",
                    cur.display()
                )));
            }
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(io_err(&cur)(e)),
        }
    }
    Ok(())
}

// ---- updating a local mod from a file ----

/// What a local file would install, read before anything in `addons` changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inspected {
    /// The file name (`cool_mod_v2.zip`).
    pub source: String,
    /// The file stem, the default name for a new mod.
    pub name: String,
    /// Number of dir VPKs it holds.
    pub vpk_count: usize,
    /// Inner paths of all its VPKs (empty when they cannot be parsed).
    pub paths: BTreeSet<String>,
}

/// Why a file was taken for a newer version of an installed mod.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchReason {
    /// Same file name as the mod was installed or last updated from.
    SameSource,
    /// The file name matches the mod's name.
    SameName,
    /// The VPKs replace mostly the same game files.
    SameFiles,
}

/// An installed mod a dropped file most likely updates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateMatch {
    /// Index into the `mods` slice given to [`find_update_target`].
    pub index: usize,
    pub reason: MatchReason,
}

/// Share of inner paths two VPK sets must have in common (Jaccard index)
/// to count as the same mod.
pub const OVERLAP_THRESHOLD: f64 = 0.5;

/// Extract `file` into a scratch dir and read what it holds. Nothing in
/// `addons` changes (the scratch dir is removed again).
///
/// # Errors
///
/// [`InstallError::UnsupportedArchive`] when the file is no recognised
/// archive; [`InstallError::NoVpk`] when it holds no VPK;
/// [`InstallError::UnsafePath`] and [`InstallError::TooLarge`] as for
/// extraction; [`InstallError::Io`] for read failures.
pub fn inspect(file: &Path, paths: &GamePaths) -> Result<Inspected, InstallError> {
    let addons_dir = paths.addons.as_path();
    clean_stale_staging(addons_dir);
    let format = detect_format(file)?;
    let staging = Staging::create(addons_dir)?;
    extract(format, file, staging.path(), Limits::default())?;
    let staged = collect_vpks(staging.path())?;
    if staged.is_empty() {
        return Err(InstallError::NoVpk);
    }
    let mut inner = BTreeSet::new();
    for vpk in &staged {
        inner.extend(vpk_paths(&vpk.file));
    }
    Ok(Inspected {
        source: source_name(file).unwrap_or_default(),
        name: file_stem(file),
        vpk_count: staged.len(),
        paths: inner,
    })
}

/// The installed mod `new` most likely is a newer version of, if any. Only
/// local mods (installed from a file or adopted) and hand-added paks are
/// candidates; catalog mods update through the catalog. Hand-added paks
/// match only by content. Checks, in order: same source file name, file name
/// equal to the mod name, then the largest inner-path overlap of at least
/// [`OVERLAP_THRESHOLD`]. A mod with a different VPK count never matches.
#[must_use]
pub fn find_update_target(mods: &[InstalledMod], new: &Inspected) -> Option<UpdateMatch> {
    let candidates: Vec<(usize, &InstalledMod)> = mods
        .iter()
        .enumerate()
        .filter(|(_, m)| {
            m.vpks.len() == new.vpk_count
                && m.sidecar
                    .as_ref()
                    .is_none_or(|s| sidecar::is_local(&s.mod_id))
        })
        .collect();
    let source = name_key(&new.source);
    let stem = name_key(&new.name);
    let by_key = |reason: MatchReason, key: &dyn Fn(&Sidecar) -> Option<String>, want: &str| {
        if want.is_empty() {
            return None;
        }
        candidates.iter().find_map(|(i, m)| {
            let sc = m.sidecar.as_ref()?;
            (key(sc).map(|k| name_key(&k)).as_deref() == Some(want))
                .then_some(UpdateMatch { index: *i, reason })
        })
    };
    if let Some(m) = by_key(MatchReason::SameSource, &|s| s.source.clone(), &source) {
        return Some(m);
    }
    if let Some(m) = by_key(MatchReason::SameName, &|s| Some(s.name.clone()), &stem) {
        return Some(m);
    }
    if new.paths.is_empty() {
        return None;
    }
    candidates
        .iter()
        .filter_map(|(i, m)| {
            let old: BTreeSet<String> = m.vpks.iter().flat_map(|v| vpk_paths(&v.file)).collect();
            let score = overlap(&old, &new.paths);
            (score >= OVERLAP_THRESHOLD).then_some((*i, score))
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(index, _)| UpdateMatch {
            index,
            reason: MatchReason::SameFiles,
        })
}

/// The on-disk tree shares at least half its paths with `incoming`
/// (CompLock `Alike`: `common > 0 && common * 2 >= local`).
#[must_use]
pub fn alike(local: &BTreeSet<String>, incoming: &BTreeSet<String>) -> bool {
    let common = local.intersection(incoming).count();
    common > 0 && common.saturating_mul(2) >= local.len()
}

/// Each tree is at least three fifths the other (CompLock `Twin`).
#[must_use]
pub fn twin(local: &BTreeSet<String>, incoming: &BTreeSet<String>) -> bool {
    let common = local.intersection(incoming).count();
    common > 0
        && at_least_ratio(common, local.len(), 5, 3)
        && at_least_ratio(common, incoming.len(), 5, 3)
}

/// [`alike`] or [`twin`]. A small local tree mostly contained in a large
/// pak is a look-alike even when the Jaccard index is far under
/// [`OVERLAP_THRESHOLD`].
#[must_use]
pub fn look_alike(local: &BTreeSet<String>, incoming: &BTreeSet<String>) -> bool {
    alike(local, incoming) || twin(local, incoming)
}

fn at_least_ratio(common: usize, total: usize, num: usize, den: usize) -> bool {
    common.saturating_mul(num) >= total.saturating_mul(den)
}

/// Jaccard index of two path sets (0 when both are empty).
#[must_use]
#[allow(
    clippy::cast_precision_loss,
    reason = "file counts are far below 2^53; the ratio needs a float"
)]
pub fn overlap(a: &BTreeSet<String>, b: &BTreeSet<String>) -> f64 {
    let union = a.union(b).count();
    if union == 0 {
        return 0.0;
    }
    a.intersection(b).count() as f64 / union as f64
}

/// A file or mod name reduced for comparison: extension dropped, lowercase
/// letters and digits only, and trailing version digits (with a `v` before
/// them) removed. `Cool Mod v1.2.zip` and `cool_mod_v13.7z` both give
/// `coolmod`. Returns "" when less than three characters are left.
pub fn name_key(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    let stem = [".vpk", ".zip", ".rar", ".7z"]
        .iter()
        .find_map(|ext| lower.strip_suffix(ext))
        .unwrap_or(&lower);
    let mut key: String = stem.chars().filter(char::is_ascii_alphanumeric).collect();
    // Trailing version: digits, optionally preceded by a `v`.
    let digits = key.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    if digits < key.len() {
        key.truncate(digits);
        if key.ends_with('v') {
            key.pop();
        }
    }
    if key.len() < 3 { String::new() } else { key }
}

/// The result of [`replace_local`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replaced {
    pub sidecar: Sidecar,
    /// Where the old files were moved, inside `<addons>/.shiv/backup`.
    pub backup: PathBuf,
}

/// Replace the files of the local or hand-added mod `target` with the VPKs
/// in `file`, in place: each new VPK takes the pak name (and enabled state)
/// of the old one it replaces, so the load order does not change. The name
/// and notes stay; a hand-added pak gets a sidecar named after the file. The
/// old files are moved to `<addons>/.shiv/backup/<stamp>-<id>/`, not deleted.
///
/// # Errors
///
/// [`InstallError::GameRunning`] and [`InstallError::NotLocal`] before
/// anything is touched; the extraction errors as for [`inspect`]; the io,
/// addons and sidecar errors while moving and recording.
pub fn replace_local(
    file: &Path,
    target: &InstalledMod,
    paths: &GamePaths,
) -> Result<Replaced, InstallError> {
    let guard = InstallGuard::acquire();
    replace_local_with(file, target, paths, game::is_running, guard.permit())
}

/// [`replace_local`] with an injectable running-game check.
///
/// # Errors
///
/// As [`replace_local`].
pub fn replace_local_with(
    file: &Path,
    target: &InstalledMod,
    paths: &GamePaths,
    game_running: impl Fn() -> bool,
    permit: InstallPermit<'_>,
) -> Result<Replaced, InstallError> {
    permit.keep();
    if game_running() {
        return Err(InstallError::GameRunning);
    }
    if target.vpks.is_empty()
        || target
            .sidecar
            .as_ref()
            .is_some_and(|s| !sidecar::is_local(&s.mod_id))
    {
        return Err(InstallError::NotLocal);
    }
    let addons_dir = paths.addons.as_path();
    clean_stale_staging(addons_dir);
    let format = detect_format(file)?;
    let staging = Staging::create(addons_dir)?;
    extract(format, file, staging.path(), Limits::default())?;
    let staged = collect_vpks(staging.path())?;
    if staged.is_empty() {
        return Err(InstallError::NoVpk);
    }
    let mut old: Vec<&addons::InstalledVpk> = target.vpks.iter().collect();
    old.sort_by(|a, b| {
        (a.pak.unwrap_or(u32::MAX), &a.file).cmp(&(b.pak.unwrap_or(u32::MAX), &b.file))
    });
    let plan = plan_replacement(&staged, &old)?;

    let id = match &target.sidecar {
        Some(sc) => sc.mod_id.clone(),
        None => file_stem(&old[0].file),
    };
    let backup = backup_dir(addons_dir, &id)?;

    // Old files out, then new files in; any failure puts the old ones back.
    let mut retired: Vec<(PathBuf, PathBuf)> = Vec::new();
    let mut placed: Vec<PathBuf> = Vec::new();
    let undo = |retired: &[(PathBuf, PathBuf)], placed: &[PathBuf]| {
        for p in placed {
            let _ = fs::remove_file(p);
        }
        for (from, to) in retired {
            let _ = fs::rename(to, from);
        }
        let _ = fs::remove_dir(&backup);
    };
    for from in &plan.retire {
        let Some(name) = from.file_name() else {
            continue;
        };
        let to = backup.join(name);
        if let Err(e) = fs::rename(from, &to) {
            undo(&retired, &placed);
            return Err(io_err(from)(e));
        }
        retired.push((from.clone(), to));
    }
    for (from, to) in &plan.place {
        if let Err(e) = move_into_place(from, to) {
            undo(&retired, &placed);
            return Err(e);
        }
        placed.push(to.clone());
    }

    let source = source_name(file);
    let sc = if let Some(mut sc) = target.sidecar.clone() {
        sc.source = source;
        sc
    } else {
        let mut vpks = plan.dir_names;
        vpks.sort();
        Sidecar {
            mod_id: sidecar::new_local_id(addons_dir),
            name: file_stem(file),
            version: None,
            thumbnail_url: None,
            vpks,
            nsfw: Some(false),
            hero: None,
            category: None,
            notes: None,
            source,
            stamps: None,
        }
    };
    if let Err(e) = sidecar::write(addons_dir, &sc) {
        undo(&retired, &placed);
        return Err(e.into());
    }
    Ok(Replaced {
        sidecar: sc,
        backup,
    })
}

/// The moves one replacement makes: the staged files into the slots the old
/// files vacate, the old files into the backup, and the enabled names the
/// new sidecar lists.
struct ReplacementPlan {
    place: Vec<(PathBuf, PathBuf)>,
    retire: Vec<PathBuf>,
    dir_names: Vec<String>,
}

/// The swap plan pairing each staged VPK with the old slot it takes over,
/// or why no safe pairing exists. `old` is sorted by pak number and name,
/// matching the staged order.
fn plan_replacement(
    staged: &[StagedVpk],
    old: &[&addons::InstalledVpk],
) -> Result<ReplacementPlan, InstallError> {
    if old.len() != staged.len() {
        return Err(InstallError::VpkCountMismatch {
            old: old.len(),
            new: staged.len(),
        });
    }
    let mut plan = ReplacementPlan {
        place: Vec::new(),
        retire: Vec::new(),
        dir_names: Vec::new(),
    };
    for (new, old) in staged.iter().zip(old) {
        let dir = old.file.parent().ok_or(InstallError::NotLocal)?;
        let name = old
            .file
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or(InstallError::NotLocal)?;
        let enabled_name = name.strip_suffix(DISABLED_EXT).unwrap_or(name);
        if !is_plain_vpk_name(enabled_name) {
            return Err(InstallError::NotLocal);
        }
        let suffix = if old.enabled { "" } else { DISABLED_EXT };
        plan.place.push((
            new.file.clone(),
            dir.join(format!("{enabled_name}{suffix}")),
        ));
        if !new.chunks.is_empty() {
            // Chunk files are named after the pak number.
            let Some(number) = old.pak else {
                return Err(InstallError::UnsafePath(format!(
                    "{enabled_name} has no pak number for the new chunk files"
                )));
            };
            for (digits, chunk) in &new.chunks {
                plan.place.push((
                    chunk.clone(),
                    dir.join(format!("pak{number:02}_{digits}.vpk{suffix}")),
                ));
            }
        }
        plan.dir_names.push(enabled_name.to_owned());
        plan.retire.push(old.file.clone());
        plan.retire.extend(old.chunks.iter().cloned());
    }
    Ok(plan)
}

/// A fresh `<addons>/.shiv/backup/<unix secs>-<id>[-n]` directory.
fn backup_dir(addons_dir: &Path, id: &str) -> Result<PathBuf, InstallError> {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let safe: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let root = addons_dir.join(SHIV_DIR).join(BACKUP_DIR);
    fs::create_dir_all(&root).map_err(io_err(&root))?;
    for n in 0u32.. {
        let name = if n == 0 {
            format!("{secs}-{safe}")
        } else {
            format!("{secs}-{safe}-{n}")
        };
        let dir = root.join(name);
        match fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(io_err(&dir)(e)),
        }
    }
    unreachable!("an unused backup dir")
}

/// Directory under `<addons>/.shiv` that keeps the files an update replaced.
pub const BACKUP_DIR: &str = "backup";

fn source_name(file: &Path) -> Option<String> {
    file.file_name().map(|n| n.to_string_lossy().into_owned())
}

fn file_stem(file: &Path) -> String {
    let name = file
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let name = name.strip_suffix(DISABLED_EXT).unwrap_or(&name).to_owned();
    Path::new(&name)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or(name)
}

/// Inner file paths of one dir VPK; empty when it cannot be parsed.
fn vpk_paths(file: &Path) -> BTreeSet<String> {
    vpk_parser::VpkParser::parse_directory_from_file(file)
        .map(|entries| {
            entries
                .into_iter()
                .map(|e| e.full_path.to_ascii_lowercase())
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn old_vpk(
        file: &str,
        pak: Option<u32>,
        enabled: bool,
        chunks: &[&str],
    ) -> addons::InstalledVpk {
        addons::InstalledVpk {
            file: PathBuf::from(file),
            pak,
            enabled,
            chunks: chunks.iter().map(PathBuf::from).collect(),
        }
    }

    fn staged_vpk(file: &str, chunks: &[&str]) -> StagedVpk {
        let parent = Path::new(file)
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        StagedVpk {
            file: PathBuf::from(file),
            original: None,
            sort_name: file.to_ascii_lowercase(),
            chunks: chunks
                .iter()
                .map(|digits| ((*digits).to_owned(), parent.join(format!("{digits}.vpk"))))
                .collect(),
        }
    }

    fn borrowed(owned: &[addons::InstalledVpk]) -> Vec<&addons::InstalledVpk> {
        owned.iter().collect()
    }

    #[test]
    fn plan_replacement_keeps_the_slot_the_suffix_and_the_chunk_names() {
        let owned_old = [old_vpk(
            "/game/addons/pak02_dir.vpk.disabled",
            Some(2),
            false,
            &["/game/addons/pak02_000.vpk.disabled"],
        )];
        let old: Vec<&addons::InstalledVpk> = owned_old.iter().collect();
        let staged = vec![staged_vpk("/stage/skin.vpk", &["000"])];

        let plan = plan_replacement(&staged, &old).unwrap();

        assert_eq!(
            plan.place,
            vec![
                (
                    PathBuf::from("/stage/skin.vpk"),
                    PathBuf::from("/game/addons/pak02_dir.vpk.disabled"),
                ),
                (
                    PathBuf::from("/stage/000.vpk"),
                    PathBuf::from("/game/addons/pak02_000.vpk.disabled"),
                ),
            ],
            "the staged file and its chunks land in the old slots, disabled suffix kept"
        );
        assert_eq!(
            plan.retire,
            vec![
                PathBuf::from("/game/addons/pak02_dir.vpk.disabled"),
                PathBuf::from("/game/addons/pak02_000.vpk.disabled"),
            ],
            "the old file and its chunks retire"
        );
        assert_eq!(
            plan.dir_names,
            vec!["pak02_dir.vpk"],
            "the sidecar lists the enabled name"
        );
    }

    #[test]
    fn plan_replacement_refuses_what_it_cannot_name_safely() {
        let old_of = |file: &str, pak: Option<u32>| vec![old_vpk(file, pak, true, &[])];

        let not_a_vpk = old_of("/game/addons/pak02_dir.txt", Some(2));
        assert!(matches!(
            plan_replacement(&[staged_vpk("/stage/skin.vpk", &[])], &borrowed(&not_a_vpk)),
            Err(InstallError::NotLocal)
        ));

        let no_pak_number = old_of("/game/addons/skin.vpk", None);
        let with_chunks = vec![staged_vpk("/stage/skin.vpk", &["001"])];
        assert!(matches!(
            plan_replacement(&with_chunks, &borrowed(&no_pak_number)),
            Err(InstallError::UnsafePath(_))
        ));

        let one_slot = old_of("/game/addons/skin.vpk", None);
        let two_staged = vec![
            staged_vpk("/stage/a.vpk", &[]),
            staged_vpk("/stage/b.vpk", &[]),
        ];
        assert!(matches!(
            plan_replacement(&two_staged, &borrowed(&one_slot)),
            Err(InstallError::VpkCountMismatch { old: 1, new: 2 })
        ));
    }

    /// Six of a ten-file pak sit inside a hundred-file pak. Alike (half the
    /// local tree) holds; the Jaccard index does not.
    #[test]
    fn a_small_set_mostly_inside_a_large_one_is_a_look_alike() {
        let local: BTreeSet<String> = (0..10).map(|i| format!("a{i}")).collect();
        let incoming: BTreeSet<String> = (0..6)
            .map(|i| format!("a{i}"))
            .chain((0..94).map(|i| format!("b{i}")))
            .collect();
        assert!(alike(&local, &incoming));
        assert!(!twin(&local, &incoming));
        assert!(look_alike(&local, &incoming));
        assert!(overlap(&local, &incoming) < OVERLAP_THRESHOLD);
    }
}
