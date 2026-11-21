//! The Deadlock Mod Interchange format (v1): a manager-neutral description
//! of an installed mod library, read and written by Deadlock Mod Manager and
//! Grimoire. An export is a folder holding `mod-interchange.json` and the
//! VPKs under `files/`; an import takes a bundle folder (or the manifest's
//! path) and installs the mods it does not already have.
//!
//! The rules come from the format's RFC: a bundle is untrusted input (its
//! file paths must stay inside it), one broken entry costs that entry only,
//! files that already sit in the addons folder are adopted in place instead
//! of copied, imported mods land after the installed ones in their relative
//! order, the source files are never changed, and which document key became
//! which mod is remembered so a re-import does not duplicate anything.
//!
//! Shiv has profiles and crosshairs, but the interchange view of them does
//! not map one to one onto Shiv's folder-per-profile layout, so an export
//! writes the active profile's state as each mod's `enabled` and `order`
//! (which is what the format asks of a manager without exported profiles),
//! and an import brings the mods into the active profile, ignoring the
//! `profiles` and `crosshairs` sections.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::addons;
use crate::game::GamePaths;
use crate::install;
use crate::sidecar::{self, Sidecar, SidecarError};
use crate::timestamp::iso8601_utc;

/// The manifest every bundle holds, and the only file a reader needs first.
pub const MANIFEST: &str = "mod-interchange.json";
/// The value of `format` every document carries.
pub const FORMAT: &str = "deadlock-mod-interchange";
/// The major version this module reads and writes.
pub const VERSION: u32 = 1;

/// The bundle's file name prefix: `deadlock-mods-export-<stamp>`.
pub const EXPORT_PREFIX: &str = "deadlock-mods-export-";

#[derive(Debug, thiserror::Error)]
pub enum InterchangeError {
    #[error("{0:?} is not an interchange bundle: expected a folder or a mod-interchange.json")]
    NotABundle(PathBuf),
    #[error("the bundle is version {0}; Shiv reads version 1")]
    Version(u32),
    #[error("the manifest says format {0:?}, not {FORMAT:?}")]
    WrongFormat(String),
    #[error("io error on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not read {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("{0} already holds a bundle")]
    DestinationExists(PathBuf),
    #[error(transparent)]
    Install(#[from] install::InstallError),
    #[error(transparent)]
    Sidecar(#[from] SidecarError),
}

// ---- wire types (the field names the format specifies, not Shiv's) ----

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Document {
    pub format: String,
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contents: Option<Vec<String>>,
    pub mods: Vec<InterchangeMod>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub profiles: Vec<InterchangeProfile>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub crosshairs: Vec<InterchangeCrosshair>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceInfo {
    pub manager: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manager_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InterchangeMod {
    pub key: String,
    pub name: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub order: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<Origin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hero: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nsfw: Option<bool>,
    #[serde(default)]
    pub files: Vec<InterchangeFile>,
    /// Manager-private data; Shiv writes its own mod id and reads only its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Origin {
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submission_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submission_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InterchangeFile {
    /// The VPK file name (`pak01_dir.vpk`).
    pub name: String,
    /// Relative to the bundle folder, forward slashes, inside it.
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(default = "yes")]
    pub selected: bool,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InterchangeProfile {
    pub key: String,
    pub name: String,
    #[serde(default)]
    pub active: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub mods: Vec<ProfileEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crosshair_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autoexec: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileEntry {
    pub mod_key: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub order: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InterchangeCrosshair {
    pub key: String,
    pub name: String,
    #[serde(default)]
    pub active: bool,
    #[serde(default)]
    pub convars: BTreeMap<String, String>,
}

// ---- reading a bundle ----

/// The folder holding the manifest of the bundle `path` points at (the
/// folder itself, or the folder holding a `mod-interchange.json`).
///
/// # Errors
///
/// [`InterchangeError::NotABundle`] when `path` is neither a bundle folder
/// nor a manifest file inside one.
pub fn bundle_dir(path: &Path) -> Result<PathBuf, InterchangeError> {
    let dir = if path.is_file() {
        let parent = path.parent();
        let is_manifest = path.file_name().is_some_and(|n| n == MANIFEST);
        match (parent, is_manifest) {
            (Some(parent), true) => parent.to_path_buf(),
            _ => return Err(InterchangeError::NotABundle(path.to_path_buf())),
        }
    } else if path.join(MANIFEST).is_file() {
        path.to_path_buf()
    } else {
        return Err(InterchangeError::NotABundle(path.to_path_buf()));
    };
    Ok(dir)
}

/// Read and validate the manifest of the bundle `path` points at.
///
/// # Errors
///
/// [`InterchangeError::NotABundle`] as for [`bundle_dir`];
/// [`InterchangeError::Io`] when the manifest cannot be read;
/// [`InterchangeError::Parse`] when it is not the JSON shape the format
/// defines, or names a file outside the bundle.
pub fn read_manifest(path: &Path) -> Result<(PathBuf, Document), InterchangeError> {
    let dir = bundle_dir(path)?;
    let manifest = dir.join(MANIFEST);
    let bytes = fs::read(&manifest).map_err(|source| InterchangeError::Io {
        path: manifest.clone(),
        source,
    })?;
    let doc: Document =
        serde_json::from_slice(&bytes).map_err(|source| InterchangeError::Parse {
            path: manifest.clone(),
            source,
        })?;
    if doc.format != FORMAT {
        return Err(InterchangeError::WrongFormat(doc.format));
    }
    if doc.version > VERSION {
        return Err(InterchangeError::Version(doc.version));
    }
    Ok((dir, doc))
}

/// Resolve a manifest file path against the bundle folder and refuse
/// anything that would leave it: no absolute paths, no `..`, forward
/// slashes only, no empty or `.` segments.
#[must_use]
pub fn bundle_file(dir: &Path, path: &str) -> Option<PathBuf> {
    if path.contains('\\') || path.is_empty() {
        return None;
    }
    let relative = Path::new(path);
    if relative.is_absolute() {
        return None;
    }
    for component in relative.components() {
        match component {
            Component::Normal(_) => {}
            _ => return None,
        }
    }
    Some(dir.join(relative))
}

// ---- hashing ----

/// The SHA-256 of a file's bytes, lowercase hex.
fn sha256_of(path: &Path) -> Result<String, InterchangeError> {
    let mut file = File::open(path).map_err(|source| InterchangeError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).map_err(|source| InterchangeError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

// ---- export ----

/// A `GameBanana` reference the exporting side can supply for a mod Shiv
/// installed from the catalog: the submission id and whether it is a Sound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteRef {
    pub submission_id: String,
    pub is_audio: bool,
}

/// What an export did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExportReport {
    /// The bundle folder that was written.
    pub root: PathBuf,
    /// Mod names written to the bundle.
    pub exported: Vec<String>,
    /// Mod names not written, with the reason.
    pub skipped: Vec<(String, String)>,
}

/// Unix seconds now.
fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// A folder nobody wrote yet: two exports in the same second bump the
/// stamp instead of colliding (and a frozen clock stops at a thousand).
fn unique_folder(root: PathBuf) -> PathBuf {
    if !root.exists() {
        return root;
    }
    let stem = root
        .file_name()
        .and_then(|n| n.to_str())
        .map_or(EXPORT_PREFIX.to_owned(), str::to_owned);
    for bump in 1..1000u64 {
        let next = root.with_file_name(format!("{stem}-{bump}"));
        if !next.exists() {
            return next;
        }
    }
    root
}

/// Write the installed library of `addons` (the active profile's state:
/// enabled and load order) into a new bundle under `dest`, which must not
/// hold one yet. `remote_of` maps a sidecar's mod id to the `GameBanana`
/// submission (the loaded catalog knows it); a mod without one is exported
/// as a local mod keyed by the hash of its primary VPK, which stays stable
/// across exports. Source files are copied, never moved.
///
/// # Errors
///
/// [`InterchangeError::DestinationExists`] when the new bundle folder
/// already holds a manifest; [`InterchangeError::Io`] when the folder or a
/// copy fails. Mods that cannot be exported are skipped into the report,
/// not errors.
pub fn export(
    addons: &Path,
    dest: &Path,
    remote_of: &dyn Fn(&str) -> Option<RemoteRef>,
) -> Result<ExportReport, InterchangeError> {
    let root = dest.join(format!("{EXPORT_PREFIX}{}", unix_now()));
    let root = unique_folder(root);
    fs::create_dir_all(&root).map_err(|source| InterchangeError::Io {
        path: root.clone(),
        source,
    })?;
    if root.join(MANIFEST).exists() {
        return Err(InterchangeError::DestinationExists(root));
    }
    export_into(addons, &root, remote_of)
}

/// [`export`] into `root`, a folder that exists and holds no bundle yet.
///
/// # Errors
///
/// As [`export`].
pub fn export_into(
    addons: &Path,
    root: &Path,
    remote_of: &dyn Fn(&str) -> Option<RemoteRef>,
) -> Result<ExportReport, InterchangeError> {
    if root.join(MANIFEST).exists() {
        return Err(InterchangeError::DestinationExists(root.to_path_buf()));
    }
    let mods = sidecar::group(addons);
    let io_err = |path: PathBuf| move |source| InterchangeError::Io { path, source };
    let files_dir = root.join("files");
    fs::create_dir_all(&files_dir).map_err(io_err(files_dir.clone()))?;

    let mut report = ExportReport {
        root: root.to_path_buf(),
        ..ExportReport::default()
    };
    let mut entries: Vec<InterchangeMod> = Vec::new();
    let mut order = 0i64;
    for m in &mods {
        let name = m
            .sidecar
            .as_ref()
            .map_or_else(|| first_file_name(m), display_name);
        if m.vpks.is_empty() {
            report.skipped.push((name, "no VPK files on disk".into()));
            continue;
        }
        let index = entries.len();
        let folder = format!("{:04}-{}", index, slug(&name));
        let dir = files_dir.join(&folder);
        fs::create_dir_all(&dir).map_err(io_err(dir.clone()))?;
        let mut files: Vec<InterchangeFile> = Vec::new();
        let mut primary_hash: Option<String> = None;
        let mut failed: Option<String> = None;
        for vpk in &m.vpks {
            let raw = vpk
                .file
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            if raw.is_empty() {
                failed = Some("a file has no readable name".into());
                break;
            }
            // The enabled form of the name: a disabled `pakNN_dir.vpk`
            // rides along as the mod's file (the mod's `enabled` carries
            // the on/off state, not a `selected: false` variant, which the
            // format means for alternative files that are kept but never
            // part of the installed mod).
            let file_name = raw.strip_suffix(".disabled").unwrap_or(raw).to_owned();
            let target = dir.join(&file_name);
            if let Err(e) = fs::copy(&vpk.file, &target) {
                failed = Some(format!("copying failed: {e}"));
                break;
            }
            let sha = match sha256_of(&target) {
                Ok(s) => s,
                Err(InterchangeError::Io { source, .. }) => {
                    failed = Some(format!("hashing failed: {source}"));
                    break;
                }
                Err(e) => {
                    failed = Some(e.to_string());
                    break;
                }
            };
            let size = fs::metadata(&target).map_or(0, |m| m.len());
            if primary_hash.is_none() {
                primary_hash = Some(sha.clone());
            }
            files.push(InterchangeFile {
                name: file_name,
                path: url_path_name(&target, root),
                sha256: Some(sha),
                size: Some(size),
                selected: true,
            });
        }
        match (failed, primary_hash) {
            (Some(why), _) => {
                let _ = fs::remove_dir_all(&dir);
                report
                    .skipped
                    .push((name.clone(), format!("could not export it: {why}")));
                continue;
            }
            (None, None) => {
                let _ = fs::remove_dir_all(&dir);
                report.skipped.push((name, "no readable VPK".into()));
                continue;
            }
            (None, Some(_)) => {}
        }
        let enabled = m.is_enabled();
        let remote = m.sidecar.as_ref().and_then(|s| remote_of(&s.mod_id));
        let primary_hash = files.first().and_then(|f| f.sha256.clone());
        let entry = interchange_mod(
            m,
            &name,
            enabled,
            order,
            remote,
            primary_hash.as_deref(),
            files,
        );
        order += 1;
        entries.push(entry);
        report.exported.push(name);
    }

    let document = Document {
        format: FORMAT.into(),
        version: VERSION,
        created_at: Some(iso8601_utc(unix_now())),
        source: Some(SourceInfo {
            manager: "shiv".into(),
            manager_version: Some(env!("CARGO_PKG_VERSION").to_owned()),
            profile_name: None,
        }),
        contents: Some(vec!["mods".into()]),
        mods: entries,
        profiles: Vec::new(),
        crosshairs: Vec::new(),
        warnings: report
            .skipped
            .iter()
            .map(|(name, why)| format!("{name}: {why}"))
            .collect(),
    };
    // The manifest is written last and atomically, so a crash never leaves
    // a folder that looks complete.
    let tmp = root.join(".mod-interchange.json.tmp");
    let json = serde_json::to_vec_pretty(&document).map_err(|source| InterchangeError::Parse {
        path: root.to_path_buf(),
        source,
    })?;
    fs::write(&tmp, json).map_err(io_err(tmp.clone()))?;
    fs::rename(&tmp, root.join(MANIFEST)).map_err(|source| InterchangeError::Io {
        path: root.join(MANIFEST),
        source,
    })?;
    Ok(report)
}

/// The entry for one installed mod: the key from its `GameBanana` reference
/// when known, else `local:sha256:<hash>` of its primary VPK, so the same
/// mod gets the same key in every export and importers can spot a
/// re-import.
#[allow(clippy::too_many_arguments)]
fn interchange_mod(
    m: &sidecar::InstalledMod,
    name: &str,
    enabled: bool,
    order: i64,
    remote: Option<RemoteRef>,
    primary_hash: Option<&str>,
    files: Vec<InterchangeFile>,
) -> InterchangeMod {
    let sc = m.sidecar.as_ref();
    let key = match (remote.as_ref(), primary_hash) {
        (Some(r), _) => format!(
            "gamebanana:{}:{}",
            if r.is_audio { "sound" } else { "mod" },
            r.submission_id
        ),
        (None, Some(hash)) => format!("local:sha256:{hash}"),
        (None, None) => "local:".into(),
    };
    let origin = remote.map(|r| Origin {
        provider: "gamebanana".into(),
        submission_type: Some(if r.is_audio { "sound" } else { "mod" }.into()),
        submission_id: Some(r.submission_id.clone()),
        file_id: None,
        file_name: None,
        local_id: None,
    });
    InterchangeMod {
        key,
        name: name.to_owned(),
        enabled,
        order,
        origin,
        author: None,
        description: sc.and_then(|s| s.notes.clone()),
        category: sc.and_then(|s| s.category.clone()),
        hero: sc.and_then(|s| s.hero.clone()),
        thumbnail_url: sc.and_then(|s| s.thumbnail_url.clone()),
        link: None,
        nsfw: sc.and_then(|s| s.nsfw),
        files,
        extensions: sc.map(|s| serde_json::json!({ "shiv": { "modId": s.mod_id } })),
    }
}

fn display_name(s: &Sidecar) -> String {
    if s.name.is_empty() {
        s.mod_id.clone()
    } else {
        s.name.clone()
    }
}

fn first_file_name(m: &sidecar::InstalledMod) -> String {
    m.vpks
        .first()
        .and_then(|v| v.file.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// `mod-313-dir-vpk` from `QOL Lock`: lowercase, letters digits dashes and
/// underscores kept, everything else folded to a `-`, never empty.
fn slug(name: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.extend(c.to_lowercase());
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    out.trim_end_matches('-').to_owned()
}

/// The manifest path of `file` relative to `root`, forward slashes.
fn url_path_name(file: &Path, root: &Path) -> String {
    file.strip_prefix(root)
        .unwrap_or(file)
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/")
}

// ---- import ----

/// What an import did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ImportReport {
    /// Mod names installed.
    pub installed: Vec<String>,
    /// Mod names adopted in place (their files were already on disk).
    pub adopted: Vec<String>,
    /// Mod names not imported, with the reason.
    pub skipped: Vec<(String, String)>,
}

/// Where the interchange ledger lives: beside the sidecars, per profile.
fn ledger_path(addons: &Path) -> PathBuf {
    addons.join(".shiv").join("interchange.json")
}

/// The remembered `key -> mod id` mapping of earlier imports.
fn read_ledger(addons: &Path) -> BTreeMap<String, String> {
    fs::read(ledger_path(addons))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn write_ledger(addons: &Path, ledger: &BTreeMap<String, String>) -> Result<(), InterchangeError> {
    let dir = addons.join(".shiv");
    let path = ledger_path(addons);
    fs::create_dir_all(&dir).map_err(|source| InterchangeError::Io {
        path: dir.clone(),
        source,
    })?;
    let json = serde_json::to_vec_pretty(ledger).map_err(|source| InterchangeError::Parse {
        path: path.clone(),
        source,
    })?;
    let tmp = dir.join(".interchange.json.tmp");
    fs::write(&tmp, json).map_err(|source| InterchangeError::Io {
        path: tmp.clone(),
        source,
    })?;
    fs::rename(&tmp, &path).map_err(|source| InterchangeError::Io {
        path: path.clone(),
        source,
    })
}

/// Import the bundle `path` points at (its folder, or its manifest) into
/// the active profile. `known` maps an interchange key to a Shiv mod id
/// that already exists (usually through the loaded catalog), so a
/// `GameBanana` mod the user already has is recognised without the ledger.
///
/// Following the format's rules, nothing here deletes or moves source
/// files: a mod whose VPKs are already in the addons folder byte for byte
/// is adopted in place, and everything else is copied in and installed
/// after the mods already there, keeping the bundle's relative order.
/// One broken or unknown entry costs that entry only; the report says why
/// each skip happened.
///
/// # Errors
///
/// The manifest errors as for [`read_manifest`]; [`InterchangeError::Install`]
/// carrying [`install::InstallError::GameRunning`] before anything is
/// touched; [`InterchangeError::Io`] for a failure that stops the whole
/// import. Broken entries are skips in the report, not errors.
pub fn import(
    path: &Path,
    paths: &GamePaths,
    game_running: impl Fn() -> bool,
    known: &dyn Fn(&str) -> Option<String>,
) -> Result<ImportReport, InterchangeError> {
    let (dir, doc) = read_manifest(path)?;
    if game_running() {
        return Err(InterchangeError::Install(
            install::InstallError::GameRunning,
        ));
    }
    let addons = paths.addons.as_path();
    let mut report = ImportReport::default();
    let mut ledger = read_ledger(addons);

    // Relative order first: ascending `order`, ties by the document's own
    // position so the source's view decides.
    let mut mods: Vec<&InterchangeMod> = doc.mods.iter().collect();
    mods.sort_by_key(|m| m.order);

    for m in &mods {
        let name = if m.name.trim().is_empty() {
            "Unnamed mod".to_owned()
        } else {
            m.name.trim().to_owned()
        };
        // Selected VPKs are what the source loaded; a mod with none has
        // nothing to install.
        let selected: Vec<&InterchangeFile> = m.files.iter().filter(|f| f.selected).collect();
        if selected.is_empty() {
            report
                .skipped
                .push((name, "no selected VPK files in the bundle".into()));
            continue;
        }
        // Every file must stay inside the bundle and exist.
        let mut files: Vec<PathBuf> = Vec::with_capacity(selected.len());
        let mut bad: Option<String> = None;
        for f in &selected {
            match bundle_file(&dir, &f.path) {
                Some(p) if p.is_file() => files.push(p),
                Some(_) => {
                    bad = Some(format!("{} is missing from the bundle", f.path));
                    break;
                }
                None => {
                    bad = Some(format!("{} does not stay inside the bundle", f.path));
                    break;
                }
            }
        }
        if let Some(why) = bad {
            report.skipped.push((name, why));
            continue;
        }
        // Duplicate: this key is already a Shiv mod (the ledger of an
        // earlier import, the catalog for GameBanana keys, or Shiv's own
        // extension for a re-imported Shiv bundle).
        let mine = m
            .extensions
            .as_ref()
            .and_then(|e| e.get("shiv"))
            .and_then(|s| s.get("modId"))
            .and_then(|v| v.as_str())
            .map(str::to_owned);
        let existing = known(&m.key)
            .or_else(|| ledger.get(&m.key).cloned())
            .or(mine);
        if existing.is_some_and(|id| sidecar::read(addons, &id).is_ok_and(|s| s.is_some())) {
            report.skipped.push((name, "already installed".into()));
            continue;
        }
        // A file already in the addons folder byte for byte is adopted in
        // place (checked by size, then hash), so the game never loads two
        // copies. All files matching means adopting the whole mod. The
        // install lock serialises this against every other pak-mutating
        // path, exactly like the UI's installs and moves.
        let _guard = install::INSTALL_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match adopt_or_install(addons, paths, m, &name, &files, &game_running) {
            Ok((true, id)) => {
                report.adopted.push(name.clone());
                ledger.insert(m.key.clone(), id);
            }
            Ok((false, id)) => {
                report.installed.push(name.clone());
                ledger.insert(m.key.clone(), id);
            }
            Err(text) => report.skipped.push((name, text)),
        }
    }
    write_ledger(addons, &ledger)?;
    Ok(report)
}

/// Adopt the mod in place when all its selected VPKs already sit in the
/// addons folder byte for byte; otherwise install the files as a new local
/// mod. `Ok(true, id)` means adopted. Errors are per-entry strings, so one
/// bad mod never stops the import.
fn adopt_or_install(
    addons: &Path,
    paths: &GamePaths,
    m: &InterchangeMod,
    name: &str,
    files: &[PathBuf],
    game_running: &dyn Fn() -> bool,
) -> Result<(bool, String), String> {
    // Sizes first, hashes only for same-size files: hashing every VPK in a
    // big library would take a while for nothing.
    let mut by_size: BTreeMap<u64, Vec<PathBuf>> = BTreeMap::new();
    let listed = addons::list(addons).map_err(|e| e.to_string())?;
    for vpk in &listed {
        let Ok(meta) = fs::metadata(&vpk.file) else {
            continue;
        };
        by_size
            .entry(meta.len())
            .or_default()
            .push(vpk.file.clone());
    }
    let mut adoptable: Vec<addons::InstalledVpk> = Vec::new();
    for file in files {
        let want = match fs::metadata(file) {
            Ok(m) => m.len(),
            Err(_) => return Err(format!("{} cannot be read", file.display())),
        };
        let mut found = None;
        for candidate in by_size.get(&want).into_iter().flatten() {
            if let (Ok(a), Ok(b)) = (sha256_of(file), sha256_of(candidate))
                && a == b
            {
                found = Some(candidate.clone());
                break;
            }
        }
        let Some(found) = found else {
            // Not all files are on disk: install the mod instead.
            return install_from_bundle(paths, m, name, files, game_running)
                .map(|sc| (false, sc.mod_id))
                .map_err(|e| e.to_string());
        };
        if let Some(vpk) = listed.iter().find(|v| v.file == found).cloned() {
            adoptable.push(vpk);
        }
    }
    // Adopt: only a sidecar is written; the files stay where they are,
    // disabled or not — the document's enabled state is restored after.
    let group = sidecar::InstalledMod {
        sidecar: None,
        vpks: adoptable,
    };
    let sc =
        sidecar::adopt(addons, &group, name, m.description.clone()).map_err(|e| e.to_string())?;
    // The document's state wins: an enabled mod switches stray `.disabled`
    // files on, a disabled one switches the enabled ones off. The files are
    // the ones just adopted, still in the state the listing read.
    for vpk in &group.vpks {
        if vpk.enabled != m.enabled {
            let _ = addons::set_enabled(vpk, m.enabled);
        }
    }
    Ok((true, sc.mod_id))
}

fn install_from_bundle(
    paths: &GamePaths,
    m: &InterchangeMod,
    name: &str,
    files: &[PathBuf],
    game_running: &dyn Fn() -> bool,
) -> Result<Sidecar, install::InstallError> {
    let meta = install::ModMeta {
        mod_id: sidecar::new_local_id(&paths.addons),
        name: name.to_owned(),
        version: None,
        thumbnail_url: m.thumbnail_url.clone().filter(|u| !u.is_empty()),
        nsfw: m.nsfw.unwrap_or(false),
        hero: m.hero.clone(),
        category: m.category.clone(),
    };
    let sc = install::install_files(files, meta, paths, game_running)?;
    // The mod lands as the source had it: an enabled mod stays on, a
    // disabled one is switched off right after installing.
    if !m.enabled {
        let mods = sidecar::group(&paths.addons);
        if let Some(installed) = mods
            .iter()
            .find(|x| x.sidecar.as_ref().is_some_and(|s| s.mod_id == sc.mod_id))
        {
            for vpk in &installed.vpks {
                let _ = addons::set_enabled(vpk, false);
            }
        }
    }
    Ok(sc)
}
