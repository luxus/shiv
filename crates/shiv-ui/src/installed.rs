//! Installed screen controller: lists what is in the addons folder, toggles
//! mods on and off and uninstalls them.
//!
//! Threading matches `browse`: this module runs on the Slint event loop; every
//! disk touch (listing, renaming, deleting, thumbnail fetch) runs on a worker
//! thread that posts its result back with `upgrade_in_event_loop`. Nothing
//! polls: the list is re-read when the screen is shown, on Refresh/Retry and
//! after every action, and that same read refreshes the game-running state.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use shiv_api::{ApiError, Client, DeadlockerClient, NativeListing};
use shiv_core::activity::ActivityKind;
use shiv_core::addons::{self, AddonsError};
use shiv_core::complock;
use shiv_core::drift::{self, Drift, Published};
use shiv_core::game::{self, GameError, GamePaths};
use shiv_core::generated;
use shiv_core::install::{self, InstallError, MatchReason};
use shiv_core::interchange;
use shiv_core::registry::{self, AddedSource};
use shiv_core::reset::{self, ResetError};
use shiv_core::sidecar::{self, InstalledMod, Move};
use shiv_repair::OverlayState;
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel, Weak};

use crate::browse::{game_error_text, install_error_text, to_slint};
use crate::link_target::{self, LinkTarget};
use crate::pack_store::PackStore;
use crate::thumbs;
use crate::{
    App, DropState, InstalledRow, InstalledState, LoadState, LocalInstallState, ModEditState,
    RestoreState, SourceKind, UpdateFileState,
};

pub(crate) type Locate = Arc<dyn Fn() -> Result<GamePaths, GameError> + Send + Sync>;
pub(crate) type Running = Arc<dyn Fn() -> bool + Send + Sync>;
/// Fetches one URL to a file; a reset downloads the release through it.
pub(crate) type Download = Arc<dyn Fn(&str, &Path) -> Result<(), String> + Send + Sync>;

/// Where the game is and whether it runs; injectable so tests never touch the
/// real install or depend on a running game.
#[derive(Clone)]
pub(crate) struct Deps {
    pub locate: Locate,
    pub running: Running,
    pub download: Download,
    pub pack: Arc<PackStore>,
}

impl Deps {
    fn system(pack: Arc<PackStore>) -> Self {
        Deps {
            locate: Arc::new(crate::locate_game),
            running: Arc::new(game::running_here),
            download: Arc::new(|url, dest| {
                Client::new(crate::DEFAULT_API_URL)
                    .fetch_cancellable(url, dest, &AtomicBool::new(false), |_, _| {}, None)
                    .map_err(|e| format!("The download failed. {e}"))
            }),
            pack,
        }
    }
}

const GAME_RUNNING_TEXT: &str =
    "Deadlock is running. Close it to enable, disable or uninstall mods.";
const SUFFIX: &str = ".disabled";
/// Shown when the file dialog returns nothing after being open to the user.
const NO_FILE_PICKED: &str = "No file picked. You can also type or paste a path, a link, or a mod page link, or drop the file on the window.";
/// deadlocker.net named something Shiv cannot resolve to a mod.
const DEADLOCKER_NO_MOD_PAGE: &str =
    "deadlocker.net's answer for that mod named no GameBanana mod page.";
/// Returning in under 150ms means xdg-desktop-portal / zenity could not be opened.
const PORTAL_FAIL_THRESHOLD: std::time::Duration = std::time::Duration::from_millis(150);
const PICKER_UNAVAILABLE: &str = "Could not open the system file dialog (xdg-desktop-portal or zenity unavailable). Paste a path, link, or mod page link, or drop the file on the window.";

fn pick_cancel_text(elapsed: std::time::Duration) -> &'static str {
    if elapsed < PORTAL_FAIL_THRESHOLD {
        PICKER_UNAVAILABLE
    } else {
        NO_FILE_PICKED
    }
}

/// What the banner says when dropped files wait behind a dialog.
pub(crate) fn queued_banner(n: usize) -> String {
    if n == 1 {
        "1 file queued; it continues after the open dialog.".into()
    } else {
        format!("{n} files queued; they continue after the open dialog.")
    }
}

/// What the banner says when cancelling a dialog dropped the rest of a batch.
pub(crate) fn stopped_banner(n: usize) -> String {
    if n == 1 {
        "Stopped; 1 more dropped file was not processed.".into()
    } else {
        format!("Stopped; {n} more dropped files were not processed.")
    }
}

thread_local! {
    static INSTALLED: RefCell<Option<Rc<Installed>>> = const { RefCell::new(None) };
}

fn with_installed(f: impl FnOnce(&Rc<Installed>)) {
    if let Some(i) = INSTALLED.with(|c| c.borrow().clone()) {
        f(&i);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "view-model flags mapped directly to Slint row properties"
)]
pub(crate) struct Entry {
    /// The sidecar mod id, or the enabled-form file name for a hand-added VPK.
    pub id: String,
    pub name: String,
    pub detail: String,
    /// Installed by Shiv (has a sidecar).
    pub managed: bool,
    /// Every VPK of the mod is enabled.
    pub enabled: bool,
    pub thumb_url: Option<String>,
    /// What the sidecar records about NSFW; `None` when it predates the flag.
    pub nsfw: Option<bool>,
    /// The file version recorded at install time.
    pub version: Option<String>,
    /// Human-readable version or release date recorded at install time.
    pub version_label: Option<String>,
    /// File slot summary (`"pak01"`, `"paks 01, 02"`), kept for tooltips only.
    pub paks_detail: Option<String>,
    /// Some of the mod's VPKs are enabled while others are disabled.
    pub partly_enabled: bool,
    /// Human-readable target release/date when `drift == Drift::UpdateAvailable`.
    pub target_version: Option<String>,
    /// 1-based place in the load order (1 loads first and wins conflicts);
    /// 0 for a mod that has no pak number and cannot be moved.
    pub position: usize,
    pub can_up: bool,
    pub can_down: bool,
    /// The mod's lowest pak number; 0 when its files carry none.
    pub pak: u32,
    /// Installed from a local file or adopted (`sidecar::LOCAL_PREFIX` id).
    pub local: bool,
    /// The user's notes from the sidecar.
    pub notes: Option<String>,
    /// The sidecar's own name, without the id fallback (for the edit dialog).
    pub raw_name: String,
    /// Enabled-form file names, for the edit and adopt dialogs.
    pub files: Vec<String>,
    /// What the worker read from disk for the drift check.
    pub facts: Facts,
    /// How the files compare with what the source publishes.
    pub drift: Drift,
    /// A reset can reinstall the published release.
    pub can_reset: bool,
    /// The download that placed this mod published no checksum.
    pub unverified: bool,
    /// JSON of the builder spec, when this row is a CompLock-generated mod.
    pub generated: Option<String>,
}

/// The disk side of a row's drift, read on the worker: whether the files
/// still match their install stamps, the CompLock title inside a pak Shiv
/// did not install from a source (hand-added, adopted or from a file), and
/// whether a reset left a backup to restore.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Facts {
    pub untouched: Option<bool>,
    pub complock_title: Option<String>,
    pub complock_build_date: Option<String>,
    pub is_complock_builder: bool,
    pub has_backup: bool,
    /// The revision the stored inputs would build at with the pinned
    /// template. `None` when the row is not a generated mod.
    pub generated_revision: Option<String>,
}

fn facts(addons: &Path, m: &InstalledMod) -> Facts {
    let id = key(m);
    let sourced = id.starts_with(complock::MOD_PREFIX) || registry::split_mod_id(&id).is_some();
    let complock_id = m.vpks.iter().find_map(|v| complock::identify(&v.file));
    let complock_build_date = complock_id.as_ref().and_then(|i| i.build_date.clone());
    let complock_title = if sourced {
        None
    } else {
        complock_id.map(|i| i.title)
    };
    let is_complock_builder = m
        .vpks
        .iter()
        .any(|v| complock::detect_builder_pak(&v.file, m.sidecar.as_ref()).is_some());
    let generated_revision = m.sidecar.as_ref().and_then(|s| {
        s.generated
            .as_ref()
            .and_then(generated::spec_of)
            .and_then(|spec| generated::revision(&spec).ok())
    });
    Facts {
        untouched: drift::files_untouched(m),
        complock_title,
        complock_build_date,
        is_complock_builder,
        has_backup: m.sidecar.is_some() && reset::latest_backup(addons, &id).is_some(),
        generated_revision,
    }
}

/// What a reset reinstalls, captured on the event loop from the pack data
/// or the loaded registry catalog.
#[derive(Debug, Clone)]
enum ResetPlan {
    CompLock {
        pak: complock::Pak,
        updated_date: Option<String>,
        last_slot: u32,
    },
    Registry(Box<RegistryPlan>),
    Generated(generated::Spec),
}

/// A registry release with a direct artifact, and what its sidecar records.
#[derive(Debug, Clone)]
struct RegistryPlan {
    source: AddedSource,
    listing_id: String,
    title: String,
    revision: String,
    version_label: Option<String>,
    url: String,
    sha256: Option<String>,
    filename: String,
    thumb: Option<String>,
    nsfw: bool,
}

/// Who publishes the row's mod and what: CompLock's pack data for its own
/// ids and for other paks whose `addoninfo.txt` names it, the loaded
/// registry catalog for registry ids (a source not loaded is unknown, a
/// listing it no longer carries or without a release is retired).
fn published(
    e: &Entry,
    catalogs: &[(AddedSource, Vec<NativeListing>)],
    pack: &complock::PackLoad,
) -> Published {
    if generated::is_id(&e.id) {
        return generated::publication(
            generated::builder_review(),
            e.facts.generated_revision.as_deref(),
        );
    }
    if e.id.starts_with(complock::MOD_PREFIX) {
        return pack.published(&e.id);
    }
    if let Some((source_id, listing_id)) = registry::split_mod_id(&e.id) {
        let Some((_, listings)) = catalogs.iter().find(|(s, _)| s.source_id == source_id) else {
            return Published::Unknown;
        };
        return listings
            .iter()
            .find(|l| l.id == listing_id && !l.removed)
            .and_then(|l| l.release.as_ref())
            .map_or(Published::Gone, |r| {
                Published::Current(std::iter::once(r.content_revision.clone()).collect())
            });
    }
    e.facts
        .complock_title
        .as_deref()
        .map_or(Published::Unknown, |title| pack.published_for_title(title))
}

/// The release a reset of the row installs, when one can be fetched
/// directly: a CompLock pak, or a registry release with a direct artifact.
fn reset_plan(
    e: &Entry,
    catalogs: &[(AddedSource, Vec<NativeListing>)],
    pack: &complock::PackLoad,
) -> Option<ResetPlan> {
    if !e.managed {
        return None;
    }
    if generated::is_id(&e.id) {
        if generated::builder_review() {
            return None;
        }
        return e
            .generated
            .as_deref()
            .and_then(generated::spec_from_json)
            .map(ResetPlan::Generated);
    }
    if e.id.starts_with(complock::MOD_PREFIX) {
        return pack
            .release_for(&e.id, e.version.as_deref())
            .cloned()
            .map(|pak| ResetPlan::CompLock {
                pak,
                updated_date: pack.updated_date(&e.id).map(str::to_owned),
                last_slot: pack.last_slot(),
            });
    }
    let (source_id, listing_id) = registry::split_mod_id(&e.id)?;
    let (source, listings) = catalogs.iter().find(|(s, _)| s.source_id == source_id)?;
    let listing = listings.iter().find(|l| l.id == listing_id && !l.removed)?;
    let release = listing.release.as_ref()?;
    let [artifact] = release.artifacts.as_slice() else {
        return None;
    };
    if artifact.resolver_url.is_some() || artifact.url.is_empty() {
        return None;
    }
    let version_label = release
        .version
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            release
                .published_at
                .as_deref()
                .or(listing.updated_at.as_deref())
                .and_then(crate::browse::short_date)
                .map(str::to_owned)
        });
    Some(ResetPlan::Registry(Box::new(RegistryPlan {
        source: source.clone(),
        listing_id: listing.id.clone(),
        title: listing.title.clone(),
        revision: release.content_revision.clone(),
        version_label,
        url: artifact.url.clone(),
        sha256: artifact.sha256.clone(),
        filename: artifact.filename.clone(),
        thumb: e.thumb_url.clone(),
        nsfw: listing.nsfw.unwrap_or(false),
    })))
}

/// The badge a row shows for its drift; empty when there is nothing to say.
pub(crate) fn drift_text(d: Drift) -> &'static str {
    match d {
        Drift::Modified => "Modified: the files differ from the release",
        Drift::UpdateAvailable => "A newer release is published",
        Drift::Retired => "Retired upstream: no longer published",
        Drift::BuilderReview => "Builder changed, review",
        Drift::Pristine | Drift::Untracked => "",
    }
}

const UNVERIFIED_PILL: &str = "Unverified";
const UNVERIFIED_TIP: &str = "This download had no checksum, so Shiv did not verify it.";

/// The pill a row shows. Drift wins when there is something to say about
/// the files. An install whose download had no checksum says so instead.
pub(crate) fn status_pill(drift: Drift, unverified: bool) -> &'static str {
    let pill = drift_pill(drift);
    if pill.is_empty() && unverified {
        UNVERIFIED_PILL
    } else {
        pill
    }
}

/// The tooltip for [`status_pill`].
pub(crate) fn status_tip(drift: Drift, unverified: bool) -> &'static str {
    let tip = drift_text(drift);
    if tip.is_empty() && unverified {
        UNVERIFIED_TIP
    } else {
        tip
    }
}

/// The pill a row shows for its drift; the full text is its tooltip.
pub(crate) fn drift_pill(d: Drift) -> &'static str {
    match d {
        Drift::Modified => "Modified",
        Drift::UpdateAvailable => "Update available",
        Drift::Retired => "Retired",
        Drift::BuilderReview => "Builder changed, review",
        Drift::Pristine | Drift::Untracked => "",
    }
}

/// Longest description a card's tooltip shows, in characters.
const ABOUT_CHARS: usize = 600;

/// What a card says about a mod: the author, one line under the name, and
/// the tooltip with the full description and the file details.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct CardText {
    pub author: String,
    pub blurb: String,
    pub about: String,
}

/// The card text from the catalog's author and description, the row's tag
/// (notes, "Installed from a file", not installed by Shiv) and its detail
/// (version and paks). The line under the name is the description's first
/// line, else the tag, else the detail; the tooltip holds the rest.
#[cfg(test)]
pub(crate) fn card_text(
    author: Option<&str>,
    description: Option<&str>,
    tag: &str,
    detail: &str,
) -> CardText {
    card_text_with_about(author, description, tag, detail, detail)
}

pub(crate) fn card_text_with_about(
    author: Option<&str>,
    description: Option<&str>,
    tag: &str,
    detail: &str,
    about_detail: &str,
) -> CardText {
    let plain = description
        .map(crate::describe::to_plain_text)
        .unwrap_or_default();
    let plain = plain.trim();
    let first = plain
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let blurb = [first, tag, detail]
        .into_iter()
        .find(|t| !t.is_empty())
        .unwrap_or("")
        .to_string();
    let mut about = String::new();
    if !plain.is_empty() {
        about = plain.chars().take(ABOUT_CHARS).collect();
        if plain.chars().count() > ABOUT_CHARS {
            about.push('\u{2026}');
        }
    }
    for extra in [tag, about_detail] {
        if !extra.is_empty() && extra != blurb && !about.contains(extra) {
            if !about.is_empty() {
                about.push_str("\n\n");
            }
            about.push_str(extra);
        }
    }
    if about.is_empty() {
        about.clone_from(&blurb);
    }
    CardText {
        author: author.map_or("", str::trim).to_string(),
        blurb,
        about,
    }
}

fn is_opaque_revision(raw: &str) -> bool {
    raw.starts_with("crc32:")
        || raw.starts_with("sha256:")
        || raw.starts_with("gen:")
        || (raw.starts_with("sha256-") && raw.len() > 7)
        || (raw.len() >= 16 && raw.bytes().all(|b| b.is_ascii_hexdigit()))
}

pub(crate) fn format_version_label(raw: &str) -> String {
    let raw = raw.trim();
    if raw.is_empty() || is_opaque_revision(raw) {
        return String::new();
    }
    if let Some(date) = crate::browse::short_date(raw) {
        format!("Updated {date}")
    } else {
        format!("Version {raw}")
    }
}

fn matching_registry_label<'a>(
    e: &Entry,
    catalogs: &'a [(AddedSource, Vec<NativeListing>)],
) -> Option<&'a str> {
    let (source_id, listing_id) = registry::split_mod_id(&e.id)?;
    let (_, listings) = catalogs.iter().find(|(s, _)| s.source_id == source_id)?;
    let listing = listings.iter().find(|l| l.id == listing_id && !l.removed)?;
    let release = listing.release.as_ref()?;
    if e.version.as_deref() != Some(release.content_revision.as_str()) {
        return None;
    }
    release
        .version
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .or_else(|| {
            release
                .published_at
                .as_deref()
                .or(listing.updated_at.as_deref())
                .and_then(crate::browse::short_date)
        })
}

/// Human-readable version or update date for a card, omitting raw CRC/hash
/// codes and pak numbers.
pub(crate) fn display_version(
    e: &Entry,
    catalogs: &[(AddedSource, Vec<NativeListing>)],
    pack: Option<&complock::PackLoad>,
) -> String {
    if !e.managed {
        return String::new();
    }
    let raw = if generated::is_id(&e.id) {
        e.version_label.as_deref().or_else(|| {
            (e.drift == Drift::Pristine)
                .then_some(complock::fetched())
                .filter(|d| !d.is_empty())
        })
    } else if e.id.starts_with(complock::MOD_PREFIX) {
        e.version_label
            .as_deref()
            .filter(|v| !v.is_empty())
            .or_else(|| {
                e.facts
                    .complock_build_date
                    .as_deref()
                    .filter(|d| !d.is_empty())
            })
            .or_else(|| {
                (e.drift == Drift::Pristine).then(|| pack.and_then(|p| p.updated_date(&e.id)))?
            })
    } else if registry::split_mod_id(&e.id).is_some() {
        e.version_label
            .as_deref()
            .filter(|v| !v.is_empty())
            .or_else(|| matching_registry_label(e, catalogs))
    } else {
        e.version_label
            .as_deref()
            .filter(|v| !v.is_empty())
            .or_else(|| e.version.as_deref().filter(|v| !v.is_empty()))
    };
    raw.map(format_version_label).unwrap_or_default()
}

fn detail_line(ver: &str, partly_enabled: bool) -> String {
    match (ver.is_empty(), partly_enabled) {
        (true, false) => String::new(),
        (false, false) => ver.to_owned(),
        (true, true) => "Partly enabled".to_owned(),
        (false, true) => format!("{ver} \u{b7} Partly enabled"),
    }
}

fn tooltip_detail(e: &Entry) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if !e.detail.is_empty() {
        parts.push(&e.detail);
    }
    if let Some(raw) = e
        .version
        .as_deref()
        .filter(|v| !v.is_empty() && !e.detail.contains(v))
    {
        parts.push(raw);
    }
    if let Some(paks) = e.paks_detail.as_deref() {
        parts.push(paks);
    }
    parts.join(" \u{b7} ")
}

fn current_version_text(e: &Entry) -> Option<String> {
    e.version_label
        .as_deref()
        .map(format_version_label)
        .filter(|s| !s.is_empty())
        .or_else(|| {
            e.version
                .as_deref()
                .map(format_version_label)
                .filter(|s| !s.is_empty())
        })
        .or_else(|| {
            e.facts
                .complock_build_date
                .as_deref()
                .map(format_version_label)
                .filter(|s| !s.is_empty())
        })
}

fn source_tag(e: &Entry) -> (SourceKind, &'static str) {
    if e.id.starts_with(complock::MOD_PREFIX)
        || generated::is_id(&e.id)
        || e.facts.complock_title.is_some()
        || e.facts.is_complock_builder
    {
        (SourceKind::Complock, "CompLock")
    } else if e.local || !e.managed {
        (SourceKind::Manual, "Manual")
    } else {
        (SourceKind::Registry, "Registry")
    }
}

/// Whether the catalog's file version is newer than the installed one. Both
/// are ISO-8601 timestamps, which sort correctly as text; a missing or empty
/// side means unknown, never an update.
pub(crate) fn update_available(installed: Option<&str>, catalog: Option<&str>) -> bool {
    match (installed, catalog) {
        (Some(i), Some(c)) => !i.is_empty() && !c.is_empty() && c > i,
        _ => false,
    }
}

/// Whether a row's thumbnail counts as NSFW, from the sidecar's flag and the
/// loaded catalog (`None` when none has loaded). Either saying NSFW wins; a
/// mod with no recorded flag and no catalog to ask is blurred (fail closed).
pub(crate) fn is_nsfw(recorded: Option<bool>, catalog: Option<bool>) -> bool {
    match (recorded, catalog) {
        (Some(true), _) | (_, Some(true)) | (None, None) => true,
        (Some(false), _) | (None, Some(false)) => false,
    }
}

fn enabled_file_name(vpk: &addons::InstalledVpk) -> String {
    let name = vpk
        .file
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    name.strip_suffix(SUFFIX)
        .map_or(name.clone(), str::to_owned)
}

fn key(m: &InstalledMod) -> String {
    match (&m.sidecar, m.vpks.first()) {
        (Some(s), _) => s.mod_id.clone(),
        (None, Some(v)) => enabled_file_name(v),
        (None, None) => String::new(),
    }
}

fn entry(m: &InstalledMod) -> Entry {
    let enabled = !m.vpks.is_empty() && m.vpks.iter().all(|v| v.enabled);
    let any_enabled = m.vpks.iter().any(|v| v.enabled);
    let mut files: Vec<String> = m.vpks.iter().map(enabled_file_name).collect();
    files.sort();
    let Some(s) = &m.sidecar else {
        return Entry {
            local: false,
            notes: None,
            raw_name: String::new(),
            files,
            id: key(m),
            name: key(m),
            detail: String::new(),
            managed: false,
            enabled,
            thumb_url: None,
            nsfw: None,
            version: None,
            version_label: None,
            paks_detail: None,
            partly_enabled: false,
            target_version: None,
            position: 0,
            can_up: false,
            can_down: false,
            pak: m.vpks.iter().filter_map(|v| v.pak).min().unwrap_or(0),
            facts: Facts::default(),
            drift: Drift::Untracked,
            can_reset: false,
            unverified: false,
            generated: None,
        };
    };
    let mut paks: Vec<u32> = m.vpks.iter().filter_map(|v| v.pak).collect();
    paks.sort_unstable();
    paks.dedup();
    let paks_detail = match paks.as_slice() {
        [] => None,
        [one] => Some(format!("pak{one:02}")),
        many => {
            let list: Vec<String> = many.iter().map(|p| format!("{p:02}")).collect();
            Some(format!("paks {}", list.join(", ")))
        }
    };
    let partly_enabled = any_enabled && !enabled;
    let mut out = Entry {
        local: sidecar::is_local(&s.mod_id),
        notes: s.notes.clone().filter(|n| !n.is_empty()),
        raw_name: s.name.clone(),
        files,
        id: s.mod_id.clone(),
        name: if s.name.is_empty() {
            s.mod_id.clone()
        } else {
            s.name.clone()
        },
        detail: String::new(),
        managed: true,
        enabled,
        thumb_url: s.thumbnail_url.clone().filter(|u| !u.is_empty()),
        nsfw: s.nsfw,
        version: s.version.clone(),
        version_label: s.version_label.clone().filter(|v| !v.is_empty()),
        paks_detail,
        partly_enabled,
        target_version: None,
        position: 0,
        can_up: false,
        can_down: false,
        pak: m.vpks.iter().filter_map(|v| v.pak).min().unwrap_or(0),
        facts: Facts::default(),
        drift: Drift::Untracked,
        can_reset: false,
        unverified: s.unverified == Some(true),
        generated: s.generated.as_ref().map(ToString::to_string),
    };
    out.detail = detail_line(&display_version(&out, &[], None), out.partly_enabled);
    out
}

/// Rows for `mods` (a load-ordered `sidecar::group` result), numbering the
/// ones that can be moved.
pub(crate) fn entries(mods: &[InstalledMod]) -> Vec<Entry> {
    let movable = mods
        .iter()
        .filter(|m| m.load_position() != u32::MAX)
        .count();
    let mut rank = 0;
    mods.iter()
        .map(|m| {
            let mut e = entry(m);
            if m.load_position() != u32::MAX {
                rank += 1;
                e.position = rank;
                e.can_up = rank > 1;
                e.can_down = rank < movable;
            }
            e
        })
        .collect()
}

fn summary(n: usize) -> String {
    if n == 1 {
        "1 mod".into()
    } else {
        format!("{n} mods")
    }
}

struct Loaded {
    mods: Vec<InstalledMod>,
    running: bool,
    /// The repair package no longer matches the mods (after a toggle or an
    /// uninstall, say) but still loads.
    repair_stale: bool,
    /// Each mod's drift facts, in `mods` order.
    facts: Vec<Facts>,
}

/// `addons::list` runs first: `sidecar::group` swallows an unreadable folder
/// and would show an empty list instead of an error.
fn load(deps: &Deps) -> Result<Loaded, String> {
    let paths = (deps.locate)().map_err(|e| game_error_text(&e))?;
    addons::list(&paths.addons).map_err(|e| format!("The addons folder could not be read. {e}"))?;
    let mods = sidecar::group(&paths.addons);
    Ok(Loaded {
        facts: mods.iter().map(|m| facts(&paths.addons, m)).collect(),
        mods,
        running: (deps.running)(),
        repair_stale: shiv_repair::overlay_state(&paths) == OverlayState::Stale,
    })
}

fn reset_error_text(name: &str, e: &ResetError) -> String {
    match e {
        ResetError::GameRunning => GAME_RUNNING_TEXT.into(),
        ResetError::Install(detail) => format!("Could not reset {name}. {detail}"),
        other => format!("Could not reset {name}. {other}"),
    }
}

/// Download the plan's release and reinstall it through `reset::reset_with`,
/// which sets the current files aside first and puts them back on failure.
fn run_reset(
    deps: &Deps,
    id: &str,
    name: &str,
    plan: &ResetPlan,
    ask: impl Fn(&[install::SlotOccupant]) -> Option<install::SlotChoice>,
) -> Result<String, String> {
    let paths = (deps.locate)().map_err(|e| game_error_text(&e))?;
    if (deps.running)() {
        return Err(GAME_RUNNING_TEXT.into());
    }
    let dir = tempfile::Builder::new()
        .prefix("shiv-reset-")
        .tempdir()
        .map_err(|e| format!("Could not create a temporary folder: {e}"))?;
    let (fetched, unverified) = fetch_release(deps, dir.path(), plan)?;
    let running = deps.running.clone();
    let guard = install::InstallGuard::acquire(&paths.addons).map_err(|e| e.to_string())?;
    if let ResetPlan::Generated(spec) = plan {
        return reset_generated(
            &paths,
            id,
            name,
            spec,
            &fetched,
            &|| running(),
            guard.permit(),
        );
    }
    let note = RefCell::new(String::new());
    reset::reset_with(
        &paths,
        id,
        || running(),
        || match plan {
            ResetPlan::CompLock {
                pak,
                updated_date,
                last_slot,
            } => {
                *note.borrow_mut() = reset_complock(
                    &paths,
                    id,
                    name,
                    pak,
                    updated_date.as_deref(),
                    *last_slot,
                    &fetched,
                    &|| running(),
                    &ask,
                    guard.permit(),
                )?;
                Ok(())
            }
            ResetPlan::Registry(r) => registry::install_registry_mod(
                &paths,
                &r.source,
                &r.listing_id,
                &r.title,
                &r.revision,
                r.version_label.as_deref(),
                r.thumb.as_deref(),
                r.nsfw,
                &r.filename,
                &fetched[0],
                true,
                guard.permit(),
            )
            .map(|_| ())
            .map_err(|e| e.to_string()),
            ResetPlan::Generated(_) => {
                Err("a generated update installs through the stash path, before this backup".into())
            }
        },
        guard.permit(),
    )
    .map_err(|e| reset_error_text(name, &e))?;
    if unverified {
        let _ = sidecar::mark_unverified(&paths.addons, id);
    }
    Ok(note.into_inner())
}

fn reset_generated(
    paths: &GamePaths,
    id: &str,
    name: &str,
    spec: &generated::Spec,
    fetched: &[PathBuf],
    running: &dyn Fn() -> bool,
    permit: install::InstallPermit<'_>,
) -> Result<String, String> {
    let version = generated::revision(spec).map_err(|e| e.to_string())?;
    let mut side = install::install_files(
        fetched,
        install::ModMeta {
            mod_id: id.to_owned(),
            name: name.to_owned(),
            version: Some(version.clone()),
            version_label: (!complock::fetched().is_empty())
                .then(|| complock::fetched().to_owned()),
            thumbnail_url: None,
            nsfw: false,
            hero: None,
            category: None,
        },
        paths,
        running,
        permit,
    )
    .map_err(|e| install_error_text(&e))?;
    side.generated = generated::spec_value(spec);
    side.version = Some(version);
    side.unverified = None;
    shiv_core::sidecar::write(&paths.addons, &side).map_err(|e| e.to_string())?;
    Ok(String::new())
}

#[allow(
    clippy::too_many_arguments,
    reason = "passes the reset context and conflict callback into slot installation"
)]
fn reset_complock(
    paths: &GamePaths,
    id: &str,
    name: &str,
    pak: &complock::Pak,
    updated_date: Option<&str>,
    last_slot: u32,
    fetched: &[PathBuf],
    running: &dyn Fn() -> bool,
    ask: &dyn Fn(&[install::SlotOccupant]) -> Option<install::SlotChoice>,
    permit: install::InstallPermit<'_>,
) -> Result<String, String> {
    let meta = install::ModMeta {
        mod_id: id.to_owned(),
        name: name.to_owned(),
        version: Some(complock::revision(pak)),
        version_label: updated_date.map(str::to_owned),
        thumbnail_url: None,
        nsfw: false,
        hero: None,
        category: None,
    };
    let first = install::install_files_in_slots(
        fetched,
        meta.clone(),
        paths,
        running,
        last_slot,
        None,
        permit,
    );
    let placed = match first {
        Err(install::InstallError::NeedsChoice(occupants)) => {
            let Some(choice) = ask(&occupants) else {
                return Err("Cancelled".into());
            };
            install::install_files_in_slots(
                fetched,
                meta,
                paths,
                running,
                last_slot,
                Some(choice),
                permit,
            )
            .map_err(|e| install_error_text(&e))?
        }
        Err(e) => return Err(install_error_text(&e)),
        Ok(placed) => placed,
    };
    Ok(install::slot_note(&placed))
}

/// The plan's release on disk in `dir`: a CompLock pak's VPKs unpacked and
/// CRC-checked, or a registry artifact checked against its sha256.
fn fetch_release(
    deps: &Deps,
    dir: &Path,
    plan: &ResetPlan,
) -> Result<(Vec<PathBuf>, bool), String> {
    match plan {
        ResetPlan::CompLock { pak, .. } => {
            let mut vpks = Vec::new();
            for part in &pak.parts {
                let mut pieces = Vec::new();
                for (n, url) in part.urls.iter().enumerate() {
                    let full = complock::part_url(url).ok_or_else(|| {
                        format!("The pack data's url {url} does not resolve on the site")
                    })?;
                    let piece = dir.join(format!("piece-{}-{n}.z", vpks.len()));
                    (deps.download)(&full, &piece)?;
                    pieces.push(piece);
                }
                let dest = dir.join(&part.name);
                complock::unpack_pak(
                    &pieces,
                    &dest,
                    part.crc,
                    part.size,
                    part.csize,
                    part.urls.first().map_or("", String::as_str),
                )?;
                vpks.push(dest);
            }
            Ok((vpks, false))
        }
        ResetPlan::Registry(r) => {
            let dest = dir.join("archive");
            (deps.download)(&r.url, &dest)?;
            let trust = registry::check_download(&dest, r.sha256.as_deref(), None)
                .map_err(|e| e.to_string())?;
            Ok((vec![dest], trust == registry::DownloadTrust::Unverified))
        }
        ResetPlan::Generated(spec) => {
            let dest = dir.join("complock_dir.vpk");
            generated::write_rebuild(spec, &dest).map_err(|e| e.to_string())?;
            Ok((vec![dest], false))
        }
    }
}

fn plan_result_banner(name: &str, as_update: bool, target: Option<&str>, note: &str) -> String {
    let base = if as_update {
        match target.map(str::trim).filter(|t| !t.is_empty()) {
            Some(t) => format!("Updated {name} to {t}."),
            None => format!("Updated {name}."),
        }
    } else {
        format!("Reset {name} to release.")
    };
    if note.trim().is_empty() {
        base
    } else {
        format!("{base} {note}")
    }
}

/// Worker body: read a human-readable version or date for a backup folder.
fn format_backup_label(sc: &shiv_core::sidecar::Sidecar, dir: &Path) -> String {
    if let Some(label) = sc
        .version_label
        .as_deref()
        .map(format_version_label)
        .filter(|s| !s.is_empty())
    {
        return label;
    }
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("vpk")
                && let Some(id) = complock::identify(&path)
                && let Some(date) = id.build_date.as_deref()
            {
                let formatted = format_version_label(date);
                if !formatted.is_empty() {
                    return formatted;
                }
            }
        }
    }
    if let Some(ver) = sc
        .version
        .as_deref()
        .map(format_version_label)
        .filter(|s| !s.is_empty())
    {
        return ver;
    }
    "earlier backup".to_owned()
}

/// Worker body: inspect the newest backup stored for `id` before opening the
/// restore confirmation dialog.
fn inspect_restore(deps: &Deps, id: &str) -> Result<String, String> {
    let paths = (deps.locate)().map_err(|e| game_error_text(&e))?;
    let backup = reset::latest_backup(&paths.addons, id)
        .ok_or_else(|| "No previous version backup is kept for this mod.".to_owned())?;
    Ok(format_backup_label(&backup.sidecar, &backup.dir))
}

fn run_restore(deps: &Deps, id: &str, name: &str, backup_label: &str) -> Result<String, String> {
    let paths = (deps.locate)().map_err(|e| game_error_text(&e))?;
    let running = deps.running.clone();
    let guard = install::InstallGuard::acquire(&paths.addons).map_err(|e| e.to_string())?;
    reset::restore_previous(&paths, id, || running(), guard.permit())
        .map(|_| format!("Restored {name} to {backup_label}. Current files were kept as a backup."))
        .map_err(|e| match e {
            ResetError::GameRunning => GAME_RUNNING_TEXT.into(),
            other => format!("Could not restore {name}. {other}"),
        })
}

#[derive(Clone, Copy)]
enum Action {
    Toggle { enable: bool },
    Uninstall,
    Move(Move),
}

impl Action {
    fn verb(self) -> &'static str {
        match self {
            Action::Toggle { enable: true } => "enable",
            Action::Toggle { enable: false } => "disable",
            Action::Uninstall => "uninstall",
            Action::Move(_) => "move",
        }
    }
}

fn addons_error_text(e: &AddonsError) -> String {
    match e {
        AddonsError::Full { .. } => install_error_text(&InstallError::AddonsFull),
        other => other.to_string(),
    }
}

fn action_error_text(name: &str, action: Action, detail: &str) -> String {
    format!("Could not {} {name}. {detail}", action.verb())
}

fn run_action(
    deps: &Deps,
    mods: &[InstalledMod],
    index: usize,
    name: &str,
    action: Action,
) -> Result<(), String> {
    let m = &mods[index];
    let fail = |detail: &str| action_error_text(name, action, detail);
    if (deps.running)() {
        return Err(fail(GAME_RUNNING_TEXT));
    }
    match action {
        Action::Toggle { enable } => {
            // `set_enabled` moves a VPK and its chunks together and keeps the pak number.
            for vpk in &m.vpks {
                addons::set_enabled(vpk, enable).map_err(|e| fail(&addons_error_text(&e)))?;
            }
            Ok(())
        }
        Action::Move(to) => {
            let paths = (deps.locate)().map_err(|e| fail(&game_error_text(&e)))?;
            let guard = shiv_core::install::InstallGuard::acquire(&paths.addons)
                .map_err(|e| fail(&e.to_string()))?;
            sidecar::move_mod(&paths.addons, mods, index, to, guard.permit())
                .map_err(|e| fail(&e.to_string()))
        }
        Action::Uninstall => {
            let running = deps.running.clone();
            let addons = m
                .vpks
                .first()
                .and_then(|vpk| vpk.file.parent())
                .ok_or_else(|| fail("the mod has no pak file"))?;
            let guard = install::InstallGuard::acquire(addons).map_err(|e| fail(&e.to_string()))?;
            install::uninstall_with(m, move || running(), guard.permit()).map_err(|e| match e {
                InstallError::GameRunning => fail(GAME_RUNNING_TEXT),
                other => fail(&install_error_text(&other)),
            })
        }
    }
}

/// The link-install worker: download to a temporary folder, then install
/// like a local file. Mirrors Browse's `run_install`: the game is checked
/// before the download so it is not wasted, progress reports percent steps
/// (per MB when the size is unknown), and the install takes the same lock
/// so two installs never allocate pak slots against each other.
fn run_link_install(
    client: &Client,
    deps: &Deps,
    url: &str,
    file: &str,
    name: &str,
    cancel: &AtomicBool,
    mut report: impl FnMut(LinkUpdate),
) -> Result<String, LinkFailure> {
    let paths = (deps.locate)().map_err(|e| link_fail(game_error_text(&e)))?;
    let running = deps.running.clone();
    if running() {
        return Err(link_fail(install_error_text(&InstallError::GameRunning)));
    }
    let dir = tempfile::Builder::new()
        .prefix("shiv-download-")
        .tempdir()
        .map_err(|e| link_fail(format!("Could not create a temporary folder: {e}")))?;
    // The file name becomes the mod's `source`, so a newer copy of the same
    // download is recognised as an update.
    let dest = dir.path().join(file);
    let mut last_step = u64::MAX;
    let fetched = client.fetch_cancellable(
        url,
        &dest,
        cancel,
        |done, total| {
            let step = progress_step(done, total);
            if step == last_step {
                return;
            }
            last_step = step;
            report(LinkUpdate::Progress { done, total });
        },
        None,
    );
    match fetched {
        Err(ApiError::Cancelled) => return Err(LinkFailure::Cancelled),
        Err(e) => return Err(link_fail(format!("Download failed: {e}"))),
        Ok(()) => {}
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(LinkFailure::Cancelled);
    }
    report(LinkUpdate::Installing);
    let guard = shiv_core::install::InstallGuard::acquire(&paths.addons)
        .map_err(|e| link_fail(e.to_string()))?;
    // Cancel may have been pressed while waiting for another install.
    if cancel.load(Ordering::Relaxed) {
        return Err(LinkFailure::Cancelled);
    }
    let sc = install::install_local_with(&dest, name, &paths, move || running(), guard.permit())
        .map_err(|e| link_fail(install_error_text(&e)))?;
    Ok(format!("Installed {} as {}.", sc.name, sc.vpks.join(", ")))
}

/// Wrap a worker-side message as a link-install failure.
fn link_fail(text: String) -> LinkFailure {
    LinkFailure::Failed(text)
}

struct Installed {
    weak: Weak<App>,
    deps: Deps,
    cache: Arc<thumbs::Cache>,
    rows: Rc<VecModel<InstalledRow>>,
    mods: RefCell<Vec<InstalledMod>>,
    entries: RefCell<Vec<Entry>>,
    /// What Reset reinstalls, by row id, for the rows that can reset.
    plans: RefCell<HashMap<String, ResetPlan>>,
    generation: Cell<u64>,
    /// A re-read of the folder is running.
    refreshing: Cell<bool>,
    /// A toggle or uninstall is running. Separate from `refreshing`: a
    /// Refresh finishing mid-action must not let a second action start.
    acting: Cell<bool>,
    /// The mod the edit/adopt dialog is for: its id and whether it is adopted.
    editing: RefCell<Option<(String, bool)>>,
    /// Key of the mod the Update from file dialog replaces.
    updating: RefCell<Option<String>>,
    /// Link installs by Downloads-screen id; `link-<n>`.
    links: RefCell<HashMap<String, LinkJob>>,
    next_link: Cell<u64>,
    /// The HTTP agent link installs download with; its base is never used,
    /// the links themselves are absolute.
    client: Arc<Client>,
    /// Dropped files waiting for their turn: the open dialog (or any busy
    /// state) holds the queue until it clears.
    pending: RefCell<VecDeque<PathBuf>>,
    /// A dropped file is being inspected on a worker.
    stepping: Cell<bool>,
    /// An OS file/folder dialog is currently open.
    picking: Cell<bool>,
    /// Mod pending confirmation in `RestoreDialog`.
    restore_target: RefCell<Option<RestoreTarget>>,
    /// Active "Update all" queue and progress counters.
    batch_updates: RefCell<Option<BatchUpdate>>,
}

#[derive(Clone)]
struct ActivityRecord {
    kind: ActivityKind,
    title: String,
    detail: String,
    from_version: Option<String>,
    to_version: Option<String>,
    source: Option<String>,
}

impl ActivityRecord {
    fn record(&self) {
        crate::downloads::record(
            self.kind,
            &self.title,
            &self.detail,
            self.from_version.as_deref(),
            self.to_version.as_deref(),
            self.source.as_deref(),
        );
    }
}

struct RestoreTarget {
    id: String,
    current_version: Option<String>,
    backup: String,
    source: String,
}

struct BatchUpdate {
    queue: VecDeque<String>,
    in_flight: Option<(String, BatchKind, Option<bool>)>,
    total: usize,
    succeeded: usize,
    failed: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BatchKind {
    Plan,
    Browse,
}

/// A link install, addressable from the Downloads screen. `generation`
/// tells a late worker answer from a cancelled attempt apart from the
/// current one after a retry.
#[derive(Clone)]
struct LinkJob {
    url: String,
    /// The name the mod installs under.
    name: String,
    /// The file name shown in the Downloads row and used on disk.
    file: String,
    cancel: Arc<AtomicBool>,
    generation: u64,
    /// A worker is running for this row.
    active: bool,
    /// The worker was cancelled and has not answered yet: retrying now
    /// would run a second install beside the winding-down first one.
    stopping: bool,
}

/// A link install worker's answer.
pub(crate) enum LinkFailure {
    Cancelled,
    Failed(String),
}

/// What a link-install worker reports while it runs.
enum LinkUpdate {
    Progress { done: u64, total: Option<u64> },
    Installing,
}

/// Wire the Installed screen to `app` using the real game location.
pub fn attach(app: &App, cache: Arc<thumbs::Cache>, pack: Arc<PackStore>) {
    attach_with(app, cache, Deps::system(pack));
}

pub(crate) fn attach_with(app: &App, cache: Arc<thumbs::Cache>, deps: Deps) {
    let rows = Rc::new(VecModel::default());
    let controller = Rc::new(Installed {
        weak: app.as_weak(),
        deps,
        cache,
        rows: rows.clone(),
        mods: RefCell::new(Vec::new()),
        entries: RefCell::new(Vec::new()),
        plans: RefCell::new(HashMap::new()),
        generation: Cell::new(0),
        refreshing: Cell::new(false),
        acting: Cell::new(false),
        editing: RefCell::new(None),
        updating: RefCell::new(None),
        links: RefCell::new(HashMap::new()),
        next_link: Cell::new(0),
        client: Arc::new(Client::new(crate::DEFAULT_API_URL)),
        pending: RefCell::new(VecDeque::new()),
        stepping: Cell::new(false),
        picking: Cell::new(false),
        restore_target: RefCell::new(None),
        batch_updates: RefCell::new(None),
    });
    INSTALLED.with(|c| *c.borrow_mut() = Some(controller));
    let state = app.global::<InstalledState>();
    state.set_rows(ModelRc::from(rows));
    wire_callbacks(app, &state);
}

fn wire_callbacks(app: &App, state: &InstalledState<'_>) {
    state.on_refresh(|| with_installed(|i| i.refresh(true)));
    state.on_update_all(|| with_installed(|i| i.start_update_all()));
    state.on_toggle(|id| with_installed(|i| i.toggle(id.as_str())));
    state.on_uninstall(|id| with_installed(|i| i.act(id.as_str(), Action::Uninstall)));
    state.on_move(|id, to| {
        let to = match to {
            0 => Move::Up,
            1 => Move::Down,
            _ => Move::Top,
        };
        with_installed(|i| i.act(id.as_str(), Action::Move(to)));
    });
    state.on_move_to(|id, position| {
        // The row shows 1-based places; 0 or below means first.
        let to = Move::To(usize::try_from(position.max(1) - 1).unwrap_or(0));
        with_installed(|i| i.act(id.as_str(), Action::Move(to)));
    });
    state.on_update(|id| with_installed(|i| i.update_mod(id.as_str())));
    state.on_reset(|id| with_installed(|i| i.reset_mod(id.as_str())));
    state.on_restore(|id| with_installed(|i| i.open_restore(id.as_str())));
    state.on_open_detail(|id| {
        with_installed(|i| {
            if !crate::browse::show_detail_from(id.as_str(), 1) {
                i.with_state(|s| {
                    s.set_banner_text(
                        "This mod is not in the catalog, so it has no detail page.".into(),
                    );
                });
            }
        });
    });
    state.on_open_repair(|| {
        with_installed(|i| {
            if let Some(app) = i.weak.upgrade() {
                crate::open_repair(&app);
            }
        });
    });
    state.on_dismiss_banner(|| {
        with_installed(|i| i.with_state(|s| s.set_banner_text("".into())));
    });
    state.on_edit(|id| with_installed(|i| i.open_edit(id.as_str())));
    state.on_install_file(|| with_installed(|i| i.open_local_install()));
    state.on_export_library(crate::installed::pick_export_folder);
    state.on_import_library(crate::installed::pick_import_folder);
    wire_dialog_callbacks(app, state);
}

fn wire_dialog_callbacks(app: &App, state: &InstalledState<'_>) {
    let edit = app.global::<ModEditState>();
    edit.on_save(|| with_installed(|i| i.save_edit()));
    edit.on_cancel(|| with_installed(|i| i.close_edit()));
    state.on_update_file(|id| with_installed(|i| i.open_update(id.as_str())));
    let update = app.global::<UpdateFileState>();
    update.on_browse(|| with_installed(|i| i.browse_update_file()));
    update.on_replace(|| with_installed(|i| i.replace_file()));
    update.on_install_new(|| with_installed(|i| i.update_as_new()));
    update.on_cancel(|| {
        with_installed(|i| {
            i.with_app(|app| {
                let s = app.global::<UpdateFileState>();
                if !s.get_busy() {
                    s.set_open(false);
                }
            });
            *i.updating.borrow_mut() = None;
            // Cancelling stops a dropped batch, not just this file.
            i.stop_batch();
        });
    });
    let local = app.global::<LocalInstallState>();
    local.on_browse(|| with_installed(|i| i.browse_file()));
    local.on_install(|| with_installed(|i| i.install_file()));
    local.on_cancel(|| {
        with_installed(|i| {
            i.with_app(|app| {
                let s = app.global::<LocalInstallState>();
                if !s.get_busy() {
                    s.set_open(false);
                }
            });
            // Cancelling stops a dropped batch, not just this file.
            i.stop_batch();
        });
    });
    let restore = app.global::<RestoreState>();
    restore.on_confirm(|| with_installed(|i| i.confirm_restore()));
    restore.on_cancel(|| with_installed(|i| i.close_restore()));
}

/// A path typed or pasted into the install dialog: `~/` is the home folder,
/// a `file://` URI its decoded local path, surrounding quotes and spaces are dropped.
pub(crate) fn typed_path(text: &str, home: Option<&Path>) -> PathBuf {
    let t = text.trim().trim_matches(|c| c == '"' || c == '\'');
    let decoded;
    let t = if let Some(rest) = t.strip_prefix("file://") {
        let raw = match rest.find('/') {
            Some(0) => rest,
            Some(i) if &rest[..i] == "localhost" => &rest[i..],
            _ => rest,
        };
        decoded = percent_decode_lossy(raw);
        decoded.as_str()
    } else {
        t
    };
    match (t.strip_prefix("~/"), home) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => PathBuf::from(t),
    }
}

/// The pak number a user typed into the edit dialog: "5", "05" and "pak05"
/// all mean 5. `None` when the field is empty or not a number.
pub(crate) fn typed_pak(text: &str) -> Option<u32> {
    let t = text.trim().to_ascii_lowercase();
    let t = t.strip_prefix("pak").unwrap_or(&t).trim();
    t.parse().ok()
}

/// The progress step for a downloaded chunk: percent when the size is
/// known and positive — a zero `Content-Length` with a body must not
/// divide by zero — otherwise one per MB. A server that understates the
/// size cannot push the step past 100. Browse's `run_install` computes
/// the same steps.
pub(crate) fn progress_step(done: u64, total: Option<u64>) -> u64 {
    match total.filter(|t| *t > 0) {
        Some(t) => (done.saturating_mul(100) / t).min(100),
        None => done / 1_000_000,
    }
}

/// The file name a link downloads under: its last path segment, without
/// query or fragment, `%XX`-decoded. Names that are empty, dot-only or
/// carry separators (including `:`, a drive-relative path on Windows)
/// never reach the disk as-is: "mod" is used instead.
pub(crate) fn link_file_name(url: &str) -> String {
    let clean = url.trim();
    let no_frag = clean.split(['#', '?']).next().unwrap_or(clean);
    let raw = no_frag.rsplit('/').next().unwrap_or_default();
    let decoded = percent_decode_lossy(raw);
    let name = decoded.trim();
    let safe =
        !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', '\0', ':']);
    if safe { name.to_owned() } else { "mod".into() }
}

/// `%XX` escapes decoded; anything malformed stays as it was written.
fn percent_decode_lossy(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(hex) = std::str::from_utf8(bytes.get(i + 1..i + 3).unwrap_or_default()).ok()
            && let Ok(b) = u8::from_str_radix(hex, 16)
        {
            out.push(b);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The name a file installs under by default: its name without extension.
fn default_name(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Switch the installed mod `id` on (after the conflict dialog allowed it).
pub(crate) fn enable(id: &str) {
    with_installed(|i| i.act(id, Action::Toggle { enable: true }));
}

/// A download or install of `id` moved on: redraw just that row.
pub(crate) fn transfer_changed(id: &str) {
    with_installed(|i| {
        let pos = i.entries.borrow().iter().position(|e| e.id == id);
        if let Some(pos) = pos.filter(|p| *p < i.rows.row_count()) {
            let entry = i.entries.borrow()[pos].clone();
            i.rows.set_row_data(pos, i.build_row(&entry));
            i.publish_update_count();
        }
        i.note_browse_transfer_done(id);
    });
}

/// Re-read the list after something outside the screen changed the addons
/// folder (an update or a swap finished).
/// Why the dialog thinks a dropped file updates the mod.
pub(crate) fn reason_text(reason: MatchReason) -> &'static str {
    match reason {
        MatchReason::SameSource => "It has the same file name the mod was installed from.",
        MatchReason::SameName => "Its file name matches the mod's name.",
        MatchReason::SameFiles => "It changes mostly the same game files.",
    }
}

/// A file is dragged over the window (`true`) or the drag left or ended.
pub(crate) fn drop_hover(on: bool) {
    with_installed(|i| i.with_app(|app| app.global::<DropState>().set_hover(on)));
}

/// A file was dropped on the window: install it, or offer to replace the
/// local mod it is a newer version of.
pub(crate) fn file_dropped(path: PathBuf) {
    with_installed(|i| i.dropped_batch(vec![path]));
}

/// Several files were dropped on the window: they are queued and processed
/// one by one, each opening the dialog it needs.
#[cfg_attr(
    any(target_vendor = "apple", windows),
    allow(dead_code, reason = "called only from the Wayland drop path and tests")
)]
pub(crate) fn files_dropped(paths: Vec<PathBuf>) {
    with_installed(|i| i.dropped_batch(paths));
}

/// Queued dropped files continue (a dialog that held them closed).
pub(crate) fn queue_step() {
    with_installed(|i| i.step());
}

/// Pick a folder and write the installed library into an interchange
/// bundle in it (the Installed screen's Export button).
pub(crate) fn pick_export_folder() {
    with_installed(|i| {
        if !i.idle() || i.picking.get() {
            return;
        }
        i.picking.set(true);
        let weak = i.weak.clone();
        let started = crate::spawn_worker(
            "shiv-pick-export",
            || {
                rfd::FileDialog::new()
                    .set_title("Export your mods for another mod manager")
                    .pick_folder()
            },
            move |picked| {
                let folder = picked.ok().flatten();
                let _ = weak.upgrade_in_event_loop(move |_| {
                    with_installed(|i| {
                        i.picking.set(false);
                        if let Some(folder) = folder {
                            i.export_bundle(folder);
                        }
                    });
                });
            },
        );
        if !started {
            i.picking.set(false);
        }
    });
}

/// Pick an interchange bundle folder (or its manifest) and import the mods
/// it holds (the Installed screen's Import button).
pub(crate) fn pick_import_folder() {
    with_installed(|i| {
        if !i.idle() || i.picking.get() {
            return;
        }
        i.picking.set(true);
        let weak = i.weak.clone();
        let started = crate::spawn_worker(
            "shiv-pick-import",
            || {
                rfd::FileDialog::new()
                    .set_title("Import mods from another mod manager")
                    .pick_folder()
            },
            move |picked| {
                let folder = picked.ok().flatten();
                let _ = weak.upgrade_in_event_loop(move |_| {
                    with_installed(|i| {
                        i.picking.set(false);
                        if let Some(folder) = folder {
                            i.import_bundle(folder);
                        }
                    });
                });
            },
        );
        if !started {
            i.picking.set(false);
        }
    });
}

/// Export without the folder picker (tests drive this directly).
#[cfg(test)]
pub(crate) fn export_bundle_to(folder: PathBuf) {
    with_installed(|i| i.export_bundle(folder));
}

/// Import without the folder picker (tests drive this directly).
#[cfg(test)]
pub(crate) fn import_bundle_from(folder: PathBuf) {
    with_installed(|i| i.import_bundle(folder));
}

impl Installed {
    /// Write the installed library into a new bundle inside `folder`.
    fn export_bundle(&self, folder: PathBuf) {
        if !self.idle() {
            return;
        }
        self.with_state(|s| {
            s.set_banner_text("Exporting mods\u{2026}".into());
        });
        self.set_flags(None, Some(true));
        let remotes = crate::browse::catalog_remote_map();
        let deps = self.deps.clone();
        let weak = self.weak.clone();
        let started = crate::spawn_worker(
            "shiv-export",
            move || -> Result<String, String> {
                let paths = (deps.locate)().map_err(|e| game_error_text(&e))?;
                let report = interchange::export(&paths.addons, &folder, &|id| {
                    remotes
                        .get(id)
                        .map(|(submission_id, is_audio)| interchange::RemoteRef {
                            submission_id: submission_id.clone(),
                            is_audio: *is_audio,
                        })
                })
                .map_err(|e| e.to_string())?;
                let mut text = format!(
                    "Exported {} to {}.",
                    report.exported.len(),
                    report.root.display()
                );
                for (name, why) in &report.skipped {
                    let _ = write!(text, " {name} was skipped: {why}.");
                }
                Ok(text)
            },
            move |result| {
                let result = result.and_then(|r| r);
                let _ = weak.upgrade_in_event_loop(move |_| {
                    with_installed(|i| i.interchanged(result));
                });
            },
        );
        if !started {
            self.interchanged(Err("Could not start a background thread.".into()));
        }
    }

    /// Import the bundle `folder` points at into the active profile.
    fn import_bundle(&self, folder: PathBuf) {
        if !self.idle() {
            return;
        }
        self.with_state(|s| {
            s.set_banner_text("Importing mods\u{2026}".into());
        });
        self.set_flags(None, Some(true));
        let submissions = crate::browse::catalog_submission_map();
        let deps = self.deps.clone();
        let weak = self.weak.clone();
        let started = crate::spawn_worker(
            "shiv-import",
            move || -> Result<String, String> {
                let paths = (deps.locate)().map_err(|e| game_error_text(&e))?;
                let running = deps.running.clone();
                let report = interchange::import(&folder, &paths, move || running(), &|key| {
                    key.strip_prefix("gamebanana:").and_then(|rest| {
                        let (_kind, id) = rest.split_once(':')?;
                        submissions.get(id).cloned()
                    })
                })
                .map_err(|e| e.to_string())?;
                let mut text = format!(
                    "Imported {} mods ({} already on disk).",
                    report.installed.len(),
                    report.adopted.len()
                );
                for (name, why) in &report.skipped {
                    let _ = write!(text, " {name}: {why}.");
                }
                Ok(text)
            },
            move |result| {
                let result = result.and_then(|r| r);
                let _ = weak.upgrade_in_event_loop(move |_| {
                    with_installed(|i| i.interchanged(result));
                });
            },
        );
        if !started {
            self.interchanged(Err("Could not start a background thread.".into()));
        }
    }

    /// An export or import finished: its report (or error) is the banner.
    fn interchanged(&self, result: Result<String, String>) {
        match result {
            Ok(text) => {
                self.with_state(|s| s.set_banner_text(text.into()));
                // The disk may have changed on an import.
                self.acted(Ok(()));
            }
            Err(text) => {
                self.with_state(|s| s.set_banner_text(text.into()));
                self.set_flags(None, Some(false));
            }
        }
    }
}

/// Cancel the link install behind a Downloads-screen row.
pub(crate) fn cancel_link(id: &str) {
    with_installed(|i| {
        let mut links = i.links.borrow_mut();
        let Some(job) = links.get_mut(id).filter(|j| j.active) else {
            return;
        };
        // The worker notices at its next chunk or install step; its late
        // answer is ignored. Until it answers, a retry would run beside it.
        job.cancel.store(true, Ordering::Relaxed);
        job.stopping = true;
        crate::downloads::finished(id, crate::downloads::Outcome::Cancelled);
    });
}

/// Start a cancelled or failed link install again (Downloads screen).
pub(crate) fn retry_link(id: &str) {
    with_installed(|i| i.retry_link(id));
}

pub(crate) fn reload() {
    with_installed(|i| i.refresh(false));
}

/// The list may already have been read. Recompute its drift from the
/// catalogs now in memory.
pub(crate) fn catalogs_ready() {
    with_installed(|i| i.note_loaded_catalogs());
}

/// The live CompLock pack changed; recompute drift and update badges.
pub(crate) fn pack_changed() {
    with_installed(|i| i.note_loaded_catalogs());
}

fn plan_target_version(plan: &ResetPlan) -> Option<String> {
    match plan {
        ResetPlan::CompLock { updated_date, .. } => updated_date.clone(),
        ResetPlan::Registry(r) => r.version_label.clone(),
        ResetPlan::Generated(_) => {
            (!complock::fetched().is_empty()).then(|| complock::fetched().to_owned())
        }
    }
}

fn note_drift(list: &mut [Entry], pack: &complock::PackLoad) -> HashMap<String, ResetPlan> {
    let catalogs = crate::sources::catalogs();
    let mut plans = HashMap::new();
    for entry in list {
        entry.drift = drift::state(
            entry.facts.untouched,
            entry.version.as_deref(),
            &published(entry, &catalogs, pack),
        );
        entry.can_reset = false;
        entry.target_version = None;
        if matches!(entry.drift, Drift::Modified | Drift::UpdateAvailable)
            && let Some(plan) = reset_plan(entry, &catalogs, pack)
        {
            entry.can_reset = entry.drift == Drift::Modified;
            entry.target_version = plan_target_version(&plan);
            plans.insert(entry.id.clone(), plan);
        }
        let ver = display_version(entry, &catalogs, Some(pack));
        entry.detail = detail_line(&ver, entry.partly_enabled);
    }
    plans
}

/// The Show NSFW switch flipped or the catalog loaded: re-blur or unblur
/// the Installed thumbnails.
pub(crate) fn nsfw_changed() {
    with_installed(|i| i.rebuild_rows());
}

impl Installed {
    fn with_state(&self, f: impl FnOnce(&InstalledState<'_>)) {
        if let Some(app) = self.weak.upgrade() {
            f(&app.global::<InstalledState>());
        }
    }

    /// `busy` in the UI: a re-read or an action is running.
    fn set_flags(&self, refreshing: Option<bool>, acting: Option<bool>) {
        if let Some(r) = refreshing {
            self.refreshing.set(r);
        }
        if let Some(a) = acting {
            self.acting.set(a);
        }
        let busy = self.refreshing.get() || self.acting.get();
        self.with_state(|s| s.set_busy(busy));
    }

    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        reason = "Slint exposes i32 at the row boundary"
    )]
    fn build_row(&self, e: &Entry) -> InstalledRow {
        let (source_kind, source_label) = source_tag(e);
        let mut row = InstalledRow {
            id: e.id.as_str().into(),
            name: e.name.as_str().into(),
            detail: e.detail.as_str().into(),
            source_label: source_label.into(),
            source_kind,
            tag: match (e.managed, &e.notes) {
                (false, _) => "Not installed by Shiv. Adopt it to give it a name.".into(),
                (true, Some(notes)) => notes.as_str().into(),
                (true, None) if e.local => "Installed from a file".into(),
                (true, None) => SharedString::new(),
            },
            local: e.local,
            enabled: e.enabled,
            managed: e.managed,
            position: e.position as i32,
            can_up: e.can_up,
            can_down: e.can_down,
            ..InstalledRow::default()
        };
        if let Some((progress, installing)) = crate::downloads::transfer(&e.id) {
            row.transferring = true;
            row.progress = progress;
            row.transfer_text = match (installing, progress) {
                (true, _) => "Installing".into(),
                (false, p) if p < 0.0 => "Downloading".into(),
                (false, p) => format!("Downloading {:.0}%", p * 100.0).into(),
            };
        }
        let plan_update =
            e.drift == Drift::UpdateAvailable && self.plans.borrow().contains_key(&e.id);
        let browse_update = e.managed
            && !e.local
            && update_available(
                e.version.as_deref(),
                crate::browse::catalog_version(&e.id).as_deref(),
            );
        if plan_update || browse_update {
            row.update = true;
            let target = if plan_update {
                e.target_version.clone()
            } else {
                crate::browse::catalog_version_label(&e.id)
            };
            row.update_label = target
                .as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map_or_else(|| "Update".into(), |t| format!("Update to {t}").into());
        }
        let (author, description) = crate::browse::catalog_about(&e.id).unwrap_or_default();
        let about_detail = tooltip_detail(e);
        let card = card_text_with_about(
            author.as_deref(),
            description.as_deref(),
            &row.tag,
            &e.detail,
            &about_detail,
        );
        row.author = card.author.into();
        row.blurb = card.blurb.into();
        row.about = card.about.into();
        row.drift = status_tip(e.drift, e.unverified).into();
        row.pill = status_pill(e.drift, e.unverified).into();
        row.can_reset = e.can_reset;
        row.can_restore = e.facts.has_backup;
        if let Some(img) = e.thumb_url.as_deref().and_then(|u| self.cache.get(u)) {
            let nsfw = is_nsfw(e.nsfw, crate::browse::catalog_nsfw(&e.id));
            let shown = if nsfw && !crate::browse::show_nsfw() {
                // Blurred like Browse; no blurred copy means no thumbnail.
                thumbs::obscured(&img).map(|b| to_slint(&b))
            } else {
                Some(to_slint(&img))
            };
            if let Some(thumb) = shown {
                row.thumb = thumb;
                row.has_thumb = true;
            }
        }
        row
    }

    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        reason = "Slint exposes i32 for badge counts"
    )]
    fn publish_update_count(&self) {
        let count = (0..self.rows.row_count())
            .filter_map(|i| self.rows.row_data(i))
            .filter(|r| r.update && !r.transferring)
            .count();
        self.with_state(|s| s.set_update_count(count as i32));
    }

    /// Rebuild every row's thumbnail for a changed Show NSFW state or catalog.
    fn rebuild_rows(&self) {
        let entries = self.entries.borrow();
        for (i, e) in entries.iter().enumerate() {
            if i < self.rows.row_count() {
                self.rows.set_row_data(i, self.build_row(e));
            }
        }
    }

    /// Re-read the addons folder. `user` refreshes (screen shown, Refresh,
    /// Retry) also clear the banner; the refresh after an action keeps it.
    fn refresh(&self, user: bool) {
        if user {
            PackStore::refresh_if_due(&self.weak, &self.deps.pack);
        }
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        self.with_state(|s| {
            if user {
                s.set_banner_text("".into());
            }
            // Keep a loaded list on screen while it is re-read.
            if s.get_load_state() != LoadState::Ready {
                s.set_load_state(LoadState::Loading);
            }
        });
        // Busy until the result for this generation arrives: actions are
        // dropped while the entries may be stale.
        self.set_flags(Some(true), None);
        let deps = self.deps.clone();
        let weak = self.weak.clone();
        let started = crate::spawn_worker(
            "shiv-installed",
            move || load(&deps),
            move |result| {
                let result = result.and_then(|r| r);
                let _ = weak.upgrade_in_event_loop(move |_| {
                    with_installed(|i| i.loaded(generation, result));
                });
            },
        );
        if !started {
            self.fail("Could not start a background thread.".into());
        }
    }

    fn fail(&self, text: String) {
        self.set_flags(Some(false), None);
        self.with_state(|s| {
            s.set_error_text(text.into());
            s.set_load_state(LoadState::Failed);
        });
        self.maybe_advance_batch();
    }

    fn loaded(&self, generation: u64, result: Result<Loaded, String>) {
        if generation != self.generation.get() {
            return;
        }
        let loaded = match result {
            Ok(l) => l,
            Err(text) => return self.fail(text),
        };
        let mut list = entries(&loaded.mods);
        for (entry, facts) in list.iter_mut().zip(loaded.facts) {
            entry.facts = facts;
        }
        let pack = self.deps.pack.current();
        let plans = note_drift(&mut list, &pack);
        *self.plans.borrow_mut() = plans;
        self.rows
            .set_vec(list.iter().map(|e| self.build_row(e)).collect::<Vec<_>>());
        self.publish_update_count();
        self.set_flags(Some(false), None);
        self.with_state(|s| {
            s.set_game_running(loaded.running);
            s.set_repair_stale(loaded.repair_stale);
            s.set_summary(summary(list.len()).into());
            s.set_load_state(LoadState::Ready);
            let pending = s.get_confirm_id();
            if !list.iter().any(|e| e.id.as_str() == pending.as_str()) {
                s.set_confirm_id(SharedString::new());
            }
        });
        self.fetch_thumbs(&list);
        *self.mods.borrow_mut() = loaded.mods;
        *self.entries.borrow_mut() = list;
        // Dropped files waiting behind the last action or dialog go next.
        self.step();
        self.maybe_advance_batch();
    }

    fn note_loaded_catalogs(&self) {
        if self.generation.get() == 0 {
            return;
        }
        let mut list = self.entries.borrow().clone();
        let pack = self.deps.pack.current();
        let plans = note_drift(&mut list, &pack);
        *self.plans.borrow_mut() = plans;
        self.rows
            .set_vec(list.iter().map(|e| self.build_row(e)).collect::<Vec<_>>());
        self.publish_update_count();
        *self.entries.borrow_mut() = list;
    }

    /// Fetch thumbnails the cache does not hold yet, one worker for the batch.
    fn fetch_thumbs(&self, list: &[Entry]) {
        let mut seen = HashSet::new();
        let urls: Vec<String> = list
            .iter()
            .filter_map(|e| e.thumb_url.clone())
            .filter(|u| self.cache.get(u).is_none() && !self.cache.has_failed(u))
            .filter(|u| seen.insert(u.clone()))
            .collect();
        if urls.is_empty() {
            return;
        }
        let cache = self.cache.clone();
        let weak = self.weak.clone();
        crate::spawn_worker(
            "shiv-installed-thumbs",
            move || {
                for url in urls {
                    if cache.get_or_fetch(&url).is_none() {
                        continue;
                    }
                    let posted = weak.upgrade_in_event_loop(move |_| {
                        with_installed(|i| i.thumb_ready(&url));
                    });
                    if posted.is_err() {
                        return;
                    }
                }
            },
            // Rows keep their placeholder; nothing waits on this worker.
            |_| {},
        );
    }

    fn thumb_ready(&self, url: &str) {
        let entries = self.entries.borrow();
        for (i, e) in entries.iter().enumerate() {
            if e.thumb_url.as_deref() == Some(url) && i < self.rows.row_count() {
                self.rows.set_row_data(i, self.build_row(e));
            }
        }
    }

    fn toggle(&self, id: &str) {
        let (enable, name) = match self.entries.borrow().iter().find(|e| e.id == id) {
            Some(e) => (!e.enabled, e.name.clone()),
            None => return,
        };
        if enable {
            let with = shiv_core::sidecar::conflicts_with(
                &self.mods.borrow(),
                id,
                crate::browse::catalog_kind,
            );
            if !with.is_empty() {
                crate::conflict::offer(&self.weak, id.to_owned(), name, with, Some(id.to_owned()));
                return;
            }
        }
        self.act(id, Action::Toggle { enable });
    }

    fn with_app(&self, f: impl FnOnce(&App)) {
        if let Some(app) = self.weak.upgrade() {
            f(&app);
        }
    }

    /// Whether a row action or dialog may start now.
    fn idle(&self) -> bool {
        !self.refreshing.get() && !self.acting.get()
    }

    fn open_edit(&self, id: &str) {
        if !self.idle() {
            return;
        }
        let Some(e) = self.entries.borrow().iter().find(|e| e.id == id).cloned() else {
            return;
        };
        let adopt = !e.managed;
        *self.editing.borrow_mut() = Some((e.id.clone(), adopt));
        self.with_app(|app| {
            let s = app.global::<ModEditState>();
            s.set_adopt(adopt);
            s.set_files(e.files.join(", ").into());
            s.set_name(if adopt { "" } else { e.raw_name.as_str() }.into());
            s.set_notes(e.notes.as_deref().unwrap_or("").into());
            // The pak number is editable when the mod's files carry one; a
            // hand-added file without one (or one about to be adopted) is not.
            s.set_has_pak(!adopt && e.pak > 0);
            s.set_pak_text(if e.pak > 0 {
                format!("{:02}", e.pak).into()
            } else {
                String::new().into()
            });
            s.set_error_text("".into());
            s.set_busy(false);
            s.set_open(true);
        });
    }

    fn close_edit(&self) {
        *self.editing.borrow_mut() = None;
        self.with_app(|app| app.global::<ModEditState>().set_open(false));
        // Dropped files queued behind the dialog go next.
        self.step();
    }

    fn save_edit(&self) {
        let Some((id, adopt)) = self.editing.borrow().clone() else {
            return;
        };
        if self.acting.get() || self.refreshing.get() {
            return;
        }
        let Some(app) = self.weak.upgrade() else {
            return;
        };
        let s = app.global::<ModEditState>();
        let name = s.get_name().to_string();
        let notes = Some(s.get_notes().to_string());
        if adopt && name.trim().is_empty() {
            s.set_error_text("Give the mod a name.".into());
            return;
        }
        let job = {
            let mods = self.mods.borrow();
            let entries = self.entries.borrow();
            let Some(index) = mods.iter().position(|m| key(m) == id) else {
                s.set_error_text("The mod is gone. Press Refresh.".into());
                return;
            };
            let Some(entry) = entries.get(index).cloned() else {
                s.set_error_text("The mod is gone. Press Refresh.".into());
                return;
            };
            let pak_to_set = if adopt {
                None
            } else {
                let typed = s.get_pak_text();
                if typed.trim().is_empty() {
                    None
                } else {
                    match typed_pak(&typed) {
                        Some(n) if n == entry.pak => None,
                        Some(n) if n == 0 || n > addons::PAK_CAPACITY => {
                            s.set_error_text("Pak numbers run from 01 to 99.".into());
                            return;
                        }
                        Some(n) => Some(n),
                        // Non-empty but not a number must not silently do
                        // nothing while Save reports success.
                        None => {
                            s.set_error_text("Enter the pak number as a number, 01 to 99.".into());
                            return;
                        }
                    }
                }
            };
            (mods.clone(), index, mods[index].clone(), pak_to_set)
        };
        let (mods, index, target_mod, pak_to_set) = job;
        s.set_busy(true);
        s.set_error_text("".into());
        self.set_flags(None, Some(true));
        let deps = self.deps.clone();
        let weak = self.weak.clone();
        let started = crate::spawn_worker(
            "shiv-installed-edit",
            move || -> Result<(), String> {
                let paths = (deps.locate)().map_err(|e| game_error_text(&e))?;
                let guard = shiv_core::install::InstallGuard::acquire(&paths.addons)
                    .map_err(|e| e.to_string())?;
                // The sidecar edit runs first: it is idempotent, so a failed
                // `set_pak` after it can be retried without renaming twice.
                let done = if adopt {
                    sidecar::adopt(&paths.addons, &target_mod, &name, notes)
                } else {
                    sidecar::edit(&paths.addons, &id, &name, notes)
                };
                done.map(|_| ()).map_err(|e| e.to_string())?;
                if let Some(pak) = pak_to_set {
                    sidecar::set_pak(&paths.addons, &mods, index, pak, guard.permit())
                        .map_err(|e| e.to_string())?;
                }
                Ok(())
            },
            move |result| {
                let result = result.and_then(|r| r);
                let _ = weak.upgrade_in_event_loop(move |_| {
                    with_installed(|i| i.edited(&result));
                });
            },
        );
        if !started {
            self.edited(&Err("Could not start a background thread.".into()));
        }
    }

    fn edited(&self, result: &Result<(), String>) {
        self.with_app(|app| {
            let s = app.global::<ModEditState>();
            s.set_busy(false);
            match result {
                Ok(()) => s.set_open(false),
                Err(text) => s.set_error_text(text.as_str().into()),
            }
        });
        if result.is_ok() {
            *self.editing.borrow_mut() = None;
            self.acted(Ok(()));
        } else {
            self.set_flags(None, Some(false));
        }
    }

    fn open_local_install(&self) {
        if !self.idle() {
            return;
        }
        self.with_app(|app| {
            let s = app.global::<LocalInstallState>();
            s.set_path("".into());
            s.set_name("".into());
            s.set_error_text("".into());
            s.set_busy(false);
            s.set_open(true);
        });
    }

    /// Worker body: `rfd::FileDialog::pick_file` blocks until the portal closes.
    fn pick_mod_file(title: &str) -> (Option<PathBuf>, std::time::Duration) {
        let start = std::time::Instant::now();
        let picked = rfd::FileDialog::new()
            .set_title(title)
            .add_filter("Mods", &["vpk", "zip", "rar", "7z"])
            .add_filter("All files", &["*"])
            .pick_file();
        (picked, start.elapsed())
    }

    /// The system file dialog blocks, so it runs on a worker.
    fn browse_file(&self) {
        let weak = self.weak.clone();
        crate::spawn_worker(
            "shiv-pick-file",
            || Self::pick_mod_file("Install a mod from a file"),
            move |outcome| {
                let outcome = outcome.ok();
                let _ = weak.upgrade_in_event_loop(move |app| {
                    let s = app.global::<LocalInstallState>();
                    match outcome {
                        Some((Some(path), _)) => {
                            if s.get_name().is_empty() {
                                s.set_name(default_name(&path).into());
                            }
                            s.set_path(path.to_string_lossy().as_ref().into());
                            s.set_error_text("".into());
                        }
                        Some((None, elapsed)) => {
                            if elapsed < PORTAL_FAIL_THRESHOLD || s.get_path().is_empty() {
                                s.set_error_text(pick_cancel_text(elapsed).into());
                            }
                        }
                        None => {
                            s.set_error_text(PICKER_UNAVAILABLE.into());
                        }
                    }
                });
            },
        );
    }

    fn install_file(&self) {
        let Some(app) = self.weak.upgrade() else {
            return;
        };
        let s = app.global::<LocalInstallState>();
        if !self.idle() {
            s.set_error_text("Wait for the current refresh or action to finish.".into());
            return;
        }
        let text = s.get_path().to_string();
        // A link routes by what it names; everything else is a local path.
        match link_target::classify(&text) {
            Ok(None) => {}
            Ok(Some(target)) => {
                self.install_target(target, &s.get_name());
                return;
            }
            Err(refusal) => {
                s.set_error_text(refusal.into());
                return;
            }
        }
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let path = typed_path(&s.get_path(), home.as_deref());
        if !path.is_file() {
            s.set_error_text(format!("{} is not a file.", path.display()).into());
            return;
        }
        let name = match s.get_name().trim() {
            "" => default_name(&path),
            n => n.to_owned(),
        };
        s.set_busy(true);
        s.set_error_text("".into());
        self.set_flags(None, Some(true));
        let deps = self.deps.clone();
        let weak = self.weak.clone();
        let started = crate::spawn_worker(
            "shiv-install-file",
            move || -> Result<(String, ActivityRecord), String> {
                let paths = (deps.locate)().map_err(|e| game_error_text(&e))?;
                let running = deps.running.clone();
                let guard = shiv_core::install::InstallGuard::acquire(&paths.addons)
                    .map_err(|e| e.to_string())?;
                let sc = install::install_local_with(
                    &path,
                    &name,
                    &paths,
                    move || running(),
                    guard.permit(),
                )
                .map_err(|e| install_error_text(&e))?;
                let vpks = sc.vpks.join(", ");
                let banner = format!("Installed {} as {vpks}.", sc.name);
                let rec = ActivityRecord {
                    kind: ActivityKind::Installed,
                    title: sc.name,
                    detail: format!("Installed ({vpks})"),
                    from_version: None,
                    to_version: sc.version_label.or(sc.version),
                    source: Some("Manual".to_owned()),
                };
                Ok((banner, rec))
            },
            move |result| {
                let result = result.and_then(|r| r);
                let _ = weak.upgrade_in_event_loop(move |_| {
                    with_installed(|i| i.installed_file(result));
                });
            },
        );
        if !started {
            self.installed_file(Err("Could not start a background thread.".into()));
        }
    }

    fn installed_file(&self, result: Result<(String, ActivityRecord), String>) {
        self.with_app(|app| {
            let s = app.global::<LocalInstallState>();
            s.set_busy(false);
            match &result {
                Ok(_) => s.set_open(false),
                Err(text) => s.set_error_text(text.as_str().into()),
            }
        });
        match result {
            Ok((text, rec)) => {
                rec.record();
                self.with_state(|s| s.set_banner_text(text.into()));
                self.acted(Ok(()));
            }
            Err(_) => self.set_flags(None, Some(false)),
        }
    }

    /// Download the link in the install dialog and install it as a local
    /// mod. The dialog closes at once; progress shows on the Downloads
    /// screen, where the row can be cancelled and retried.
    fn install_link(&self, url: &str, name: &str) {
        let Some(app) = self.weak.upgrade() else {
            return;
        };
        let s = app.global::<LocalInstallState>();
        let url = url.trim().to_owned();
        let file = link_file_name(&url);
        let stem = Path::new(&file)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let name = match name.trim() {
            "" => stem,
            n => n.to_owned(),
        };
        let n = self.next_link.get() + 1;
        self.next_link.set(n);
        let id = format!("link-{n}");
        let cancel = Arc::new(AtomicBool::new(false));
        self.links.borrow_mut().insert(
            id.clone(),
            LinkJob {
                url: url.clone(),
                name: name.clone(),
                file: file.clone(),
                cancel: cancel.clone(),
                generation: n,
                active: true,
                stopping: false,
            },
        );
        s.set_error_text("".into());
        s.set_open(false);
        crate::downloads::started(&id, &name);
        self.start_link_job(&id, n, &url, &name, &file, cancel);
    }

    /// Route a classified link from the install dialog to its flow.
    fn install_target(&self, target: LinkTarget, name: &str) {
        match target {
            LinkTarget::Download { url } => self.install_link(&url, name),
            LinkTarget::GameBananaPage { audio, remote_id } => {
                if let Err(refusal) = self.try_open_submission(&remote_id, audio) {
                    self.dialog_refusal(refusal);
                }
            }
            LinkTarget::DeadlockerPage { slug } => self.resolve_deadlocker(&slug),
            LinkTarget::ManagerInstall {
                archive_url,
                audio,
                mod_id,
            } => {
                // A 1-click link knows the mod it installs: its page when the
                // catalog carries it, the named archive otherwise.
                let opened = mod_id
                    .as_deref()
                    .is_some_and(|id| self.try_open_submission(id, audio).is_ok());
                if !opened {
                    self.install_link(&archive_url, name);
                }
            }
        }
    }

    /// Open the catalog page of the GameBanana submission `remote_id` from
    /// the Installed screen. The refusal names why nothing opened.
    fn try_open_submission(&self, remote_id: &str, audio: bool) -> Result<(), String> {
        if crate::browse::show_detail_for_submission(remote_id, audio, 1) {
            self.with_app(|app| app.global::<LocalInstallState>().set_open(false));
            return Ok(());
        }
        if !crate::browse::gamebanana_enabled() {
            return Err(
                "GameBanana is disabled. Enable it in Sources to open GameBanana mod pages.".into(),
            );
        }
        let kind = if audio { "sound" } else { "mod" };
        if !crate::browse::catalog_loaded() {
            return Err("The catalog has not finished loading; try again in a moment.".into());
        }
        Err(format!(
            "Shiv's catalog does not carry GameBanana {kind} {remote_id}."
        ))
    }

    /// Show `refusal` in the install dialog, which stays open.
    fn dialog_refusal(&self, refusal: String) {
        self.with_app(|app| {
            app.global::<LocalInstallState>()
                .set_error_text(refusal.into());
        });
    }

    /// Ask deadlocker.net which GameBanana submission its page names. The
    /// dialog stays busy until the answer arrives; the request runs off the
    /// event loop.
    fn resolve_deadlocker(&self, slug: &str) {
        self.with_app(|app| {
            let s = app.global::<LocalInstallState>();
            s.set_busy(true);
            s.set_error_text("".into());
        });
        let slug = slug.to_owned();
        let weak = self.weak.clone();
        let started = crate::spawn_worker(
            "shiv-resolve-link",
            move || DeadlockerClient::default().mod_link(&slug),
            move |result| {
                let answer = result.and_then(|fetched| fetched.map_err(|e| e.to_string()));
                let _ = weak.upgrade_in_event_loop(move |_| {
                    with_installed(|i| i.deadlocker_resolved(answer));
                });
            },
        );
        if !started {
            self.deadlocker_resolved(Err("Could not start a background thread.".into()));
        }
    }

    /// deadlocker.net answered: open the GameBanana page it names.
    fn deadlocker_resolved(&self, answer: Result<String, String>) {
        self.with_app(|app| app.global::<LocalInstallState>().set_busy(false));
        let submission = answer.and_then(|link| {
            link_target::gamebanana_submission(&link).map_err(|_| DEADLOCKER_NO_MOD_PAGE.to_owned())
        });
        match submission {
            Ok((audio, remote_id)) => {
                if let Err(refusal) = self.try_open_submission(&remote_id, audio) {
                    self.dialog_refusal(refusal);
                }
            }
            Err(text) => self.dialog_refusal(text),
        }
    }

    /// Start (or restart, after a retry) the worker behind a link row.
    fn start_link_job(
        &self,
        id: &str,
        generation: u64,
        url: &str,
        name: &str,
        file: &str,
        cancel: Arc<AtomicBool>,
    ) {
        let client = self.client.clone();
        let deps = self.deps.clone();
        let weak = self.weak.clone();
        let job_id = id.to_owned();
        let report = move |update: LinkUpdate| {
            let job_id = job_id.clone();
            let _ = weak.clone().upgrade_in_event_loop(move |_| match update {
                LinkUpdate::Progress { done, total } => {
                    crate::downloads::progress(&job_id, done, total);
                }
                LinkUpdate::Installing => crate::downloads::installing(&job_id),
            });
        };
        let weak_done = self.weak.clone();
        let done_id = id.to_owned();
        let url = url.to_owned();
        let name = name.to_owned();
        let file = file.to_owned();
        let started = crate::spawn_worker(
            "shiv-install-link",
            move || run_link_install(&client, &deps, &url, &file, &name, &cancel, report),
            move |result| {
                let result = result.map_err(LinkFailure::Failed).and_then(|r| r);
                let _ = weak_done.upgrade_in_event_loop(move |_| {
                    with_installed(|i| i.link_finished(&done_id, generation, result));
                });
            },
        );
        if !started {
            self.link_finished(
                id,
                generation,
                Err(LinkFailure::Failed(
                    "Could not start a background thread.".into(),
                )),
            );
        }
    }

    /// Retry the link install `id` with a fresh worker, same row. Only once
    /// the last attempt answered (finished, failed or confirmed its
    /// cancel); a worker that is merely winding down would install beside
    /// the new attempt.
    fn retry_link(&self, id: &str) {
        let job = self.links.borrow().get(id).cloned();
        let Some(job) = job.filter(|j| !j.active && !j.stopping) else {
            return;
        };
        let n = self.next_link.get() + 1;
        self.next_link.set(n);
        let cancel = Arc::new(AtomicBool::new(false));
        let fresh = LinkJob {
            url: job.url.clone(),
            name: job.name.clone(),
            file: job.file.clone(),
            cancel: cancel.clone(),
            generation: n,
            active: true,
            stopping: false,
        };
        self.links.borrow_mut().insert(id.to_owned(), fresh);
        crate::downloads::started(id, &job.name);
        self.start_link_job(id, n, &job.url, &job.name, &job.file, cancel);
    }

    /// A link worker answered. A stale generation (the row was retried
    /// meanwhile) is ignored, except for an install that made it to disk.
    fn link_finished(&self, id: &str, generation: u64, result: Result<String, LinkFailure>) {
        let name = self.links.borrow().get(id).map(|j| j.name.clone());
        let Some(name) = name else {
            return;
        };
        let current = self
            .links
            .borrow()
            .get(id)
            .is_some_and(|j| j.generation == generation);
        if !current {
            if result.is_ok() {
                // The disk changed under a newer attempt of the same row.
                crate::launch::refresh();
                crate::browse::refresh_installed();
                self.refresh(false);
            }
            return;
        }
        if let Some(j) = self.links.borrow_mut().get_mut(id) {
            j.active = false;
            j.stopping = false;
        }
        match result {
            Ok(banner) => {
                crate::downloads::finished_with(
                    id,
                    crate::downloads::Outcome::Done,
                    Some(ActivityKind::Installed),
                    None,
                    None,
                    Some("Manual"),
                );
                self.with_state(|s| s.set_banner_text(banner.into()));
                self.acted(Ok(()));
            }
            Err(LinkFailure::Cancelled) => {
                crate::downloads::finished_with(
                    id,
                    crate::downloads::Outcome::Cancelled,
                    None,
                    None,
                    None,
                    Some("Manual"),
                );
            }
            Err(LinkFailure::Failed(text)) => {
                crate::downloads::finished_with(
                    id,
                    crate::downloads::Outcome::Failed(text.clone()),
                    None,
                    None,
                    None,
                    Some("Manual"),
                );
                self.with_state(|s| {
                    s.set_banner_text(format!("Could not install {name}. {text}").into());
                });
            }
        }
    }

    /// Whether any of the Installed dialogs is open, or an install conflict
    /// is being offered: dropped files queue behind all of them.
    fn dialog_open(app: &App) -> bool {
        app.global::<ModEditState>().get_open()
            || app.global::<LocalInstallState>().get_open()
            || app.global::<UpdateFileState>().get_open()
            || app.global::<crate::ConflictState>().get_open()
    }

    fn open_update(&self, id: &str) {
        if !self.idle() {
            return;
        }
        let Some(e) = self.entries.borrow().iter().find(|e| e.id == id).cloned() else {
            return;
        };
        self.show_update(&e.id, &e.name, "", "", false);
    }

    fn show_update(&self, id: &str, name: &str, path: &str, reason: &str, dropped: bool) {
        *self.updating.borrow_mut() = Some(id.to_owned());
        self.with_app(|app| {
            let s = app.global::<UpdateFileState>();
            s.set_target_name(name.into());
            s.set_path(path.into());
            s.set_reason_text(reason.into());
            s.set_dropped(dropped);
            s.set_error_text("".into());
            s.set_busy(false);
            s.set_open(true);
        });
    }

    fn browse_update_file(&self) {
        let weak = self.weak.clone();
        crate::spawn_worker(
            "shiv-pick-update",
            || Self::pick_mod_file("Update a mod from a file"),
            move |outcome| {
                let outcome = outcome.ok();
                let _ = weak.upgrade_in_event_loop(move |app| {
                    let s = app.global::<UpdateFileState>();
                    match outcome {
                        Some((Some(path), _)) => {
                            s.set_path(path.to_string_lossy().as_ref().into());
                            s.set_error_text("".into());
                        }
                        Some((None, elapsed)) => {
                            if elapsed < PORTAL_FAIL_THRESHOLD || s.get_path().is_empty() {
                                s.set_error_text(pick_cancel_text(elapsed).into());
                            }
                        }
                        None => {
                            s.set_error_text(PICKER_UNAVAILABLE.into());
                        }
                    }
                });
            },
        );
    }

    /// Hand the dropped file over to the plain install dialog.
    fn update_as_new(&self) {
        let Some(app) = self.weak.upgrade() else {
            return;
        };
        let s = app.global::<UpdateFileState>();
        if s.get_busy() {
            return;
        }
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let path = typed_path(&s.get_path(), home.as_deref());
        s.set_open(false);
        *self.updating.borrow_mut() = None;
        self.show_local_install(&path);
    }

    fn show_local_install(&self, path: &Path) {
        self.with_app(|app| {
            let s = app.global::<LocalInstallState>();
            s.set_path(path.to_string_lossy().as_ref().into());
            s.set_name(default_name(path).into());
            s.set_error_text("".into());
            s.set_busy(false);
            s.set_open(true);
        });
    }

    fn replace_file(&self) {
        let Some(app) = self.weak.upgrade() else {
            return;
        };
        let s = app.global::<UpdateFileState>();
        let Some(id) = self.updating.borrow().clone() else {
            s.set_error_text("No mod selected to replace. Close and try again.".into());
            return;
        };
        if !self.idle() {
            s.set_error_text("Wait for the current refresh or action to finish.".into());
            return;
        }
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let path = typed_path(&s.get_path(), home.as_deref());
        if !path.is_file() {
            s.set_error_text(format!("{} is not a file.", path.display()).into());
            return;
        }
        let target = self.mods.borrow().iter().find(|m| key(m) == id).cloned();
        let Some(target) = target else {
            s.set_error_text("The mod is gone. Press Refresh.".into());
            return;
        };
        let name = s.get_target_name().to_string();
        let (source, from_version) = self
            .entries
            .borrow()
            .iter()
            .find(|e| e.id == id)
            .map_or_else(
                || ("Manual".to_owned(), None),
                |e| (source_tag(e).1.to_owned(), current_version_text(e)),
            );
        s.set_busy(true);
        s.set_error_text("".into());
        self.set_flags(None, Some(true));
        let deps = self.deps.clone();
        let weak = self.weak.clone();
        let started = crate::spawn_worker(
            "shiv-replace-file",
            move || -> Result<(String, ActivityRecord), String> {
                let paths = (deps.locate)().map_err(|e| game_error_text(&e))?;
                let running = deps.running.clone();
                let guard =
                    install::InstallGuard::acquire(&paths.addons).map_err(|e| e.to_string())?;
                let r = install::replace_local_with(
                    &path,
                    &target,
                    &paths,
                    move || running(),
                    guard.permit(),
                )
                .map_err(|e| install_error_text(&e))?;
                let backup = r
                    .backup
                    .strip_prefix(&paths.addons)
                    .unwrap_or(&r.backup)
                    .display()
                    .to_string();
                let to_version = r
                    .sidecar
                    .version_label
                    .clone()
                    .or_else(|| r.sidecar.version.clone());
                let banner = format!(
                    "Updated {name} in {}. The old file is in addons/{backup}.",
                    r.sidecar.vpks.join(", ")
                );
                let rec = ActivityRecord {
                    kind: ActivityKind::Replaced,
                    title: name,
                    detail: format!("Replaced (backup in addons/{backup})"),
                    from_version,
                    to_version,
                    source: Some(source),
                };
                Ok((banner, rec))
            },
            move |result| {
                let result = result.and_then(|r| r);
                let _ = weak.upgrade_in_event_loop(move |_| {
                    with_installed(|i| i.replaced(result));
                });
            },
        );
        if !started {
            self.replaced(Err("Could not start a background thread.".into()));
        }
    }

    fn replaced(&self, result: Result<(String, ActivityRecord), String>) {
        self.with_app(|app| {
            let s = app.global::<UpdateFileState>();
            s.set_busy(false);
            match &result {
                Ok(_) => s.set_open(false),
                Err(text) => s.set_error_text(text.as_str().into()),
            }
        });
        match result {
            Ok((text, rec)) => {
                rec.record();
                *self.updating.borrow_mut() = None;
                self.with_state(|s| s.set_banner_text(text.into()));
                self.acted(Ok(()));
            }
            Err(_) => self.set_flags(None, Some(false)),
        }
    }

    /// Look inside the next dropped file on a worker, then open the replace
    /// dialog for the mod it matches or the install dialog prefilled.
    fn step(&self) {
        if self.stepping.get() {
            return;
        }
        let Some(app) = self.weak.upgrade() else {
            return;
        };
        if Self::dialog_open(&app) || !self.idle() {
            return;
        }
        let Some(path) = self.pending.borrow_mut().pop_front() else {
            return;
        };
        self.stepping.set(true);
        let file = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.with_state(|s| s.set_banner_text(format!("Checking {file}\u{2026}").into()));
        self.set_flags(None, Some(true));
        let deps = self.deps.clone();
        let weak = self.weak.clone();
        let path_for_result = path.clone();
        let started = crate::spawn_worker(
            "shiv-inspect-drop",
            move || -> Result<Option<(String, String, MatchReason)>, String> {
                let paths = (deps.locate)().map_err(|e| game_error_text(&e))?;
                let ins = install::inspect(&path, &paths).map_err(|e| install_error_text(&e))?;
                let mods = sidecar::group(&paths.addons);
                Ok(install::find_update_target(&mods, &ins).map(|m| {
                    let target = &mods[m.index];
                    let e = entry(target);
                    (key(target), e.name, m.reason)
                }))
            },
            move |result| {
                let result = result.and_then(|r| r);
                let _ = weak.upgrade_in_event_loop(move |_| {
                    with_installed(|i| i.inspected(&path_for_result, result));
                });
            },
        );
        if !started {
            self.set_flags(None, Some(false));
            self.stepping.set(false);
            self.with_state(|s| {
                s.set_banner_text("Could not start a background thread.".into());
            });
        }
    }

    fn inspected(
        &self,
        path: &Path,
        result: Result<Option<(String, String, MatchReason)>, String>,
    ) {
        self.stepping.set(false);
        self.set_flags(None, Some(false));
        match result {
            // A file that cannot be read is reported; the rest still go.
            Err(text) => {
                self.with_state(|s| s.set_banner_text(text.into()));
                self.step();
            }
            Ok(found) => {
                self.with_state(|s| s.set_banner_text("".into()));
                let shown = path.to_string_lossy().into_owned();
                match found {
                    Some((id, name, reason)) => {
                        self.show_update(&id, &name, &shown, reason_text(reason), true);
                    }
                    None => self.show_local_install(path),
                }
                // The dialog holds the queue: it resumes when the dialog
                // closes (success refreshes and `loaded` steps again) or is
                // cancelled (which also stops the batch).
            }
        }
    }

    /// Stop processing dropped files: the queue is dropped and a note says
    /// how many were left.
    fn stop_batch(&self) {
        let left = self.pending.borrow().len();
        if left > 0 {
            self.with_state(|s| s.set_banner_text(stopped_banner(left).into()));
        }
        self.pending.borrow_mut().clear();
    }

    fn skipped_banner(n: usize) -> String {
        if n == 1 {
            "Skipped 1 dropped item that is not a file.".into()
        } else {
            format!("Skipped {n} dropped items that are not files.")
        }
    }

    fn fill_open_picker(app: &App, file: &Path) -> bool {
        let update = app.global::<UpdateFileState>();
        if update.get_open()
            && !update.get_busy()
            && (!update.get_dropped() || update.get_path().trim().is_empty())
        {
            update.set_path(file.to_string_lossy().as_ref().into());
            update.set_error_text("".into());
            return true;
        }
        let local = app.global::<LocalInstallState>();
        if local.get_open() && !local.get_busy() && local.get_path().trim().is_empty() {
            if local.get_name().is_empty() {
                local.set_name(default_name(file).into());
            }
            local.set_path(file.to_string_lossy().as_ref().into());
            local.set_error_text("".into());
            return true;
        }
        false
    }

    /// Files were dropped on the window. A single file dropped onto an open
    /// "replace" or empty "install from file" picker fills that dialog
    /// directly; otherwise drops queue behind open dialogs and process in order.
    fn dropped_batch(&self, paths: Vec<PathBuf>) {
        let Some(app) = self.weak.upgrade() else {
            return;
        };
        app.global::<DropState>().set_hover(false);
        if app.get_screen() != crate::screen::INSTALLED {
            app.set_screen(crate::screen::INSTALLED);
        }
        let (files, skipped): (Vec<PathBuf>, Vec<PathBuf>) =
            paths.into_iter().partition(|p| p.is_file());
        if files.len() == 1 && Self::fill_open_picker(&app, &files[0]) {
            if !skipped.is_empty() {
                let text = Self::skipped_banner(skipped.len());
                self.with_state(|s| s.set_banner_text(text.into()));
            }
            return;
        }
        let n_queued = {
            let mut pending = self.pending.borrow_mut();
            pending.extend(files);
            pending.len()
        };
        if self.generation.get() == 0 && !self.refreshing.get() {
            self.refresh(false);
        }
        let held = Self::dialog_open(&app) || !self.idle() || self.stepping.get();
        let mut parts: Vec<String> = Vec::new();
        if !skipped.is_empty() {
            parts.push(Self::skipped_banner(skipped.len()));
        }
        if n_queued > 0 && held {
            parts.push(queued_banner(n_queued));
        }
        if !parts.is_empty() {
            self.with_state(|s| s.set_banner_text(parts.join(" ").into()));
        }
        self.step();
    }

    fn act(&self, id: &str, action: Action) {
        if self.weak.upgrade().is_none() || self.refreshing.get() || self.acting.get() {
            return;
        }
        let found = {
            let entries = self.entries.borrow();
            entries.iter().position(|e| e.id == id).map(|i| {
                let e = &entries[i];
                let rec = matches!(action, Action::Uninstall).then(|| ActivityRecord {
                    kind: ActivityKind::Removed,
                    title: e.name.clone(),
                    detail: "Removed".to_owned(),
                    from_version: current_version_text(e),
                    to_version: None,
                    source: Some(source_tag(e).1.to_owned()),
                });
                (i, e.name.clone(), rec)
            })
        };
        let Some((index, name, uninstall_rec)) = found else {
            return;
        };
        let mods = self.mods.borrow().clone();
        self.set_flags(None, Some(true));
        // The confirmation is consumed only once the action is accepted.
        self.with_state(|s| s.set_confirm_id(SharedString::new()));
        let deps = self.deps.clone();
        let weak = self.weak.clone();
        let started = crate::spawn_worker(
            "shiv-installed-action",
            move || run_action(&deps, &mods, index, &name, action),
            move |result| {
                let result = result.and_then(|r| r);
                let rec = if result.is_ok() { uninstall_rec } else { None };
                let _ = weak.upgrade_in_event_loop(move |_| {
                    with_installed(|i| i.acted_with(result, rec.as_ref()));
                });
            },
        );
        if !started {
            self.acted(Err("Could not start a background thread.".into()));
        }
    }

    /// Reinstall the published release over a drifted mod, keeping its files
    /// as a backup that Restore previous puts back.
    fn reset_mod(&self, id: &str) {
        self.run_plan_mod(id, false);
    }

    /// Update a mod either via its in-place release plan (CompLock / Registry)
    /// or through Browse.
    fn update_mod(&self, id: &str) {
        if self.plans.borrow().contains_key(id) {
            self.run_plan_mod(id, true);
        } else {
            crate::browse::update_mod(id);
        }
    }

    fn run_plan_mod(&self, id: &str, as_update: bool) {
        let Some(plan) = self.plans.borrow().get(id).cloned() else {
            return;
        };
        let (target, from_version, source) = self
            .entries
            .borrow()
            .iter()
            .find(|e| e.id == id)
            .map_or((None, None, None), |e| {
                (
                    e.target_version.clone(),
                    current_version_text(e),
                    Some(source_tag(e).1.to_owned()),
                )
            });
        let deps = self.deps.clone();
        let id = id.to_owned();
        let weak = self.weak.clone();
        self.run_job(&id.clone(), move |name| {
            let note = run_reset(&deps, &id, name, &plan, |occupants| {
                crate::conflict::ask_slots(&weak, occupants)
            })?;
            let banner = plan_result_banner(name, as_update, target.as_deref(), &note);
            let rec = ActivityRecord {
                kind: if as_update {
                    ActivityKind::Updated
                } else {
                    ActivityKind::Reset
                },
                title: name.to_owned(),
                detail: if as_update {
                    "Updated".to_owned()
                } else {
                    "Reset to release".to_owned()
                },
                from_version,
                to_version: target,
                source,
            };
            Ok((banner, Some(rec)))
        });
    }

    /// Open `RestoreDialog` after inspecting the backup version on a worker.
    fn open_restore(&self, id: &str) {
        if self.weak.upgrade().is_none() || self.refreshing.get() || self.acting.get() {
            return;
        }
        let Some((name, current_display, current_version, source)) = self
            .entries
            .borrow()
            .iter()
            .find(|e| e.id == id && e.facts.has_backup)
            .map(|e| {
                let cur_ver = current_version_text(e);
                let cur_display = if !e.detail.is_empty() {
                    e.detail.clone()
                } else if let Some(v) = cur_ver.clone() {
                    v
                } else {
                    "current version".to_owned()
                };
                (
                    e.name.clone(),
                    cur_display,
                    cur_ver,
                    source_tag(e).1.to_owned(),
                )
            })
        else {
            return;
        };
        let deps = self.deps.clone();
        let mod_id = id.to_owned();
        let reply_id = mod_id.clone();
        let weak = self.weak.clone();
        crate::spawn_worker(
            "shiv-restore-info",
            move || inspect_restore(&deps, &mod_id),
            move |result| {
                let result = result.and_then(|r| r);
                let _ = weak.upgrade_in_event_loop(move |_| {
                    with_installed(|i| {
                        i.show_restore_dialog(
                            reply_id,
                            &name,
                            &current_display,
                            current_version,
                            source,
                            &result,
                        );
                    });
                });
            },
        );
    }

    fn show_restore_dialog(
        &self,
        id: String,
        name: &str,
        current_display: &str,
        current_version: Option<String>,
        source: String,
        result: &Result<String, String>,
    ) {
        match result {
            Ok(backup_label) => {
                *self.restore_target.borrow_mut() = Some(RestoreTarget {
                    id,
                    current_version,
                    backup: backup_label.clone(),
                    source,
                });
                self.with_app(|app| {
                    let r = app.global::<RestoreState>();
                    r.set_mod_name(name.into());
                    r.set_current_version(current_display.into());
                    r.set_backup_version(backup_label.as_str().into());
                    r.set_error_text(SharedString::new());
                    r.set_busy(false);
                    r.set_open(true);
                });
            }
            Err(text) => {
                self.with_state(|s| s.set_banner_text(text.as_str().into()));
            }
        }
    }

    fn confirm_restore(&self) {
        if self.refreshing.get() || self.acting.get() {
            return;
        }
        let Some(target) = self.restore_target.borrow_mut().take() else {
            self.close_restore();
            return;
        };
        self.with_app(|app| {
            let r = app.global::<RestoreState>();
            r.set_busy(false);
            r.set_open(false);
        });
        let deps = self.deps.clone();
        let mod_id = target.id.clone();
        self.run_job(&target.id, move |mod_name| {
            let banner = run_restore(&deps, &mod_id, mod_name, &target.backup)?;
            let rec = ActivityRecord {
                kind: ActivityKind::Restored,
                title: mod_name.to_owned(),
                detail: "Restored".to_owned(),
                from_version: target.current_version,
                to_version: Some(target.backup),
                source: Some(target.source),
            };
            Ok((banner, Some(rec)))
        });
    }

    fn close_restore(&self) {
        *self.restore_target.borrow_mut() = None;
        self.with_app(|app| {
            let r = app.global::<RestoreState>();
            if !r.get_busy() {
                r.set_open(false);
                r.set_error_text(SharedString::new());
            }
        });
    }

    fn start_update_all(&self) {
        if self.weak.upgrade().is_none()
            || self.refreshing.get()
            || self.acting.get()
            || self.batch_updates.borrow().is_some()
        {
            return;
        }
        let game_running = self
            .weak
            .upgrade()
            .is_some_and(|a| a.global::<InstalledState>().get_game_running());
        if game_running {
            self.with_state(|s| s.set_banner_text(GAME_RUNNING_TEXT.into()));
            return;
        }
        let queue: VecDeque<String> = (0..self.rows.row_count())
            .filter_map(|i| self.rows.row_data(i))
            .filter(|r| r.update && !r.transferring)
            .map(|r| r.id.to_string())
            .collect();
        let total = queue.len();
        if total == 0 {
            return;
        }
        *self.batch_updates.borrow_mut() = Some(BatchUpdate {
            queue,
            in_flight: None,
            total,
            succeeded: 0,
            failed: 0,
        });
        self.step_update_batch();
    }

    fn note_browse_transfer_done(&self, id: &str) {
        let active = crate::downloads::transfer(id).is_some() || crate::browse::is_updating(id);
        if active {
            return;
        }
        let mut advance = false;
        if let Some(batch) = self.batch_updates.borrow_mut().as_mut()
            && let Some((in_id, BatchKind::Browse, outcome)) = batch.in_flight.as_mut()
            && in_id == id
        {
            *outcome = Some(crate::downloads::last_outcome_ok(id).unwrap_or(false));
            advance = !self.refreshing.get() && !self.acting.get();
        }
        if advance {
            self.maybe_advance_batch();
        }
    }

    fn maybe_advance_batch(&self) {
        if self.refreshing.get() || self.acting.get() {
            return;
        }
        let finished = {
            let mut borrow = self.batch_updates.borrow_mut();
            let Some(batch) = borrow.as_mut() else {
                return;
            };
            let Some((_, _, Some(ok))) = batch.in_flight else {
                return;
            };
            batch.in_flight = None;
            if ok {
                batch.succeeded += 1;
            } else {
                batch.failed += 1;
            }
            true
        };
        if finished {
            self.step_update_batch();
        }
    }

    fn step_update_batch(&self) {
        loop {
            let next = {
                let mut borrow = self.batch_updates.borrow_mut();
                let Some(batch) = borrow.as_mut() else {
                    return;
                };
                if batch.in_flight.is_some() {
                    return;
                }
                batch.queue.pop_front()
            };
            let Some(id) = next else {
                let Some(done) = self.batch_updates.borrow_mut().take() else {
                    return;
                };
                let summary = if done.failed == 0 {
                    if done.succeeded == 1 {
                        "Updated 1 mod.".to_owned()
                    } else {
                        format!("Updated {} mods.", done.succeeded)
                    }
                } else {
                    format!(
                        "Updated {} of {} mods; {} failed.",
                        done.succeeded, done.total, done.failed
                    )
                };
                self.with_state(|s| s.set_banner_text(summary.into()));
                return;
            };
            if self.plans.borrow().contains_key(&id) {
                if let Some(batch) = self.batch_updates.borrow_mut().as_mut() {
                    batch.in_flight = Some((id.clone(), BatchKind::Plan, None));
                }
                self.run_plan_mod(&id, true);
                return;
            }
            if crate::browse::catalog_version(&id).is_some() {
                if let Some(batch) = self.batch_updates.borrow_mut().as_mut() {
                    batch.in_flight = Some((id.clone(), BatchKind::Browse, None));
                }
                crate::browse::update_mod(&id);
                let active =
                    crate::downloads::transfer(&id).is_some() || crate::browse::is_updating(&id);
                if active {
                    return;
                }
                if let Some(batch) = self.batch_updates.borrow_mut().as_mut() {
                    let ok = batch
                        .in_flight
                        .take()
                        .and_then(|(_, _, o)| o)
                        .or_else(|| crate::downloads::last_outcome_ok(&id))
                        .unwrap_or(false);
                    if ok {
                        batch.succeeded += 1;
                    } else {
                        batch.failed += 1;
                    }
                }
                continue;
            }
            if let Some(batch) = self.batch_updates.borrow_mut().as_mut() {
                batch.failed += 1;
            }
        }
    }

    /// An action on one row that is not an [`Action`]: same busy guard,
    /// worker and follow-up refresh.
    fn run_job(
        &self,
        id: &str,
        job: impl FnOnce(&str) -> Result<(String, Option<ActivityRecord>), String> + Send + 'static,
    ) {
        if self.weak.upgrade().is_none() || self.refreshing.get() || self.acting.get() {
            return;
        }
        let Some(name) = self
            .entries
            .borrow()
            .iter()
            .find(|e| e.id == id)
            .map(|e| e.name.clone())
        else {
            return;
        };
        self.set_flags(None, Some(true));
        let weak = self.weak.clone();
        let started = crate::spawn_worker(
            "shiv-installed-action",
            move || job(&name),
            move |result| {
                let result = result.and_then(|r| r);
                let _ = weak.upgrade_in_event_loop(move |_| {
                    with_installed(|i| i.finish_job(&result));
                });
            },
        );
        if !started {
            self.acted(Err("Could not start a background thread.".into()));
        }
    }

    /// Success or failure, re-read the folder: that also refreshes the
    /// game-running state (a game-running refusal means it just started).
    fn acted(&self, result: Result<(), String>) {
        self.acted_with(result, None);
    }

    fn acted_with(&self, result: Result<(), String>, rec: Option<&ActivityRecord>) {
        if result.is_ok()
            && let Some(rec) = rec
        {
            rec.record();
        }
        self.finish(&result.map(|()| String::new()));
    }

    fn finish_job(&self, result: &Result<(String, Option<ActivityRecord>), String>) {
        if let Ok((_, Some(rec))) = result {
            rec.record();
        }
        let banner_result = result
            .as_ref()
            .map(|(text, _)| text.clone())
            .map_err(Clone::clone);
        self.finish(&banner_result);
    }

    fn finish(&self, result: &Result<String, String>) {
        // The disk changed: keep the Launch sidebar count and Browse's
        // Installed badges current.
        crate::launch::refresh();
        crate::browse::refresh_installed();
        if let Some(batch) = self.batch_updates.borrow_mut().as_mut()
            && let Some((_, BatchKind::Plan, outcome)) = batch.in_flight.as_mut()
        {
            *outcome = Some(result.is_ok());
        }
        self.with_state(|s| {
            let text = match result {
                Ok(note) if !note.is_empty() => Some(note.as_str()),
                Err(text) => Some(text.as_str()),
                Ok(_) => None,
            };
            if let Some(text) = text {
                s.set_banner_text(text.into());
            }
        });
        // The refresh marks busy before the action flag clears, so `busy`
        // stays set until the list is current.
        self.refresh(false);
        self.set_flags(None, Some(false));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shiv_api::{NativeArtifact, NativeRelease};
    use shiv_core::addons::InstalledVpk;
    use shiv_core::sidecar::Sidecar;
    use std::path::PathBuf;

    #[test]
    fn card_text_shows_the_first_description_line_and_keeps_the_rest_for_the_tooltip() {
        let c = card_text(
            Some(" jcStani "),
            Some("<p>Cleaner HUD.</p><p>Second paragraph.</p>"),
            "",
            "Version 1 \u{b7} pak01",
        );
        assert_eq!(c.author, "jcStani");
        assert_eq!(c.blurb, "Cleaner HUD.");
        assert!(c.about.contains("Second paragraph."), "{}", c.about);
        assert!(c.about.ends_with("Version 1 \u{b7} pak01"), "{}", c.about);
    }

    #[test]
    fn card_text_without_a_description_falls_back_to_the_tag_then_the_detail() {
        let tagged = card_text(None, None, "Installed from a file", "pak06");
        assert_eq!(tagged.blurb, "Installed from a file");
        assert_eq!(tagged.about, "pak06");
        let bare = card_text(None, Some("   "), "", "pak06");
        assert_eq!(bare.blurb, "pak06");
        assert_eq!(bare.author, "");
    }

    #[test]
    fn a_long_description_is_cut_in_the_tooltip() {
        let long = "word ".repeat(400);
        let c = card_text(None, Some(&long), "", "");
        assert!(c.about.chars().count() <= ABOUT_CHARS + 1);
        assert!(c.about.ends_with('\u{2026}'));
    }

    fn vpk(file: &str, pak: Option<u32>, enabled: bool) -> InstalledVpk {
        InstalledVpk {
            file: PathBuf::from("/a").join(file),
            pak,
            enabled,
            chunks: Vec::new(),
        }
    }

    fn sidecar(id: &str, name: &str) -> Sidecar {
        Sidecar {
            mod_id: id.into(),
            name: name.into(),
            version: Some("1.2".into()),
            version_label: None,
            thumbnail_url: Some("https://x.test/t.png".into()),
            vpks: Vec::new(),
            nsfw: None,
            hero: None,
            category: None,
            notes: None,
            source: None,
            stamps: None,
            unverified: None,
            generated: None,
        }
    }

    #[test]
    fn an_unverified_install_shows_the_marker_and_a_verified_one_does_not() {
        assert_eq!(status_pill(Drift::Pristine, true), "Unverified");
        assert_eq!(
            status_tip(Drift::Pristine, true),
            "This download had no checksum, so Shiv did not verify it."
        );
        assert_eq!(status_pill(Drift::Pristine, false), "");
        assert_eq!(status_tip(Drift::Pristine, false), "");
        assert_eq!(
            status_pill(Drift::Modified, true),
            "Modified",
            "a file that differs from its release still says that"
        );
        let mut recorded = sidecar("haze", "Haze");
        recorded.unverified = Some(true);
        recorded.vpks = vec!["pak01_dir.vpk".into()];
        let row = entries(&[InstalledMod {
            sidecar: Some(recorded),
            vpks: vec![vpk("pak01_dir.vpk", Some(1), true)],
        }]);
        assert!(row[0].unverified);
        assert_eq!(status_pill(row[0].drift, row[0].unverified), "Unverified");
    }

    #[test]
    fn update_needs_a_newer_catalog_version() {
        let old = Some("2026-11-01T10:00:00.000Z");
        let new = Some("2026-11-08T10:00:00.000Z");
        assert!(update_available(old, new));
        assert!(!update_available(new, new));
        assert!(!update_available(new, old), "catalog older");
        assert!(!update_available(None, new), "unknown installed version");
        assert!(!update_available(old, None), "mod not in the catalog");
        assert!(!update_available(Some(""), new));
        assert!(!update_available(old, Some("")));
    }

    #[test]
    fn a_registry_row_is_published_by_its_loaded_listing() {
        let source = AddedSource {
            descriptor_url: "https://reg.test/d.json".into(),
            origin: "https://reg.test".into(),
            source_id: "reg".into(),
            name: "Reg".into(),
            api_origins: Vec::new(),
            download_origins: Vec::new(),
        };
        let row = |listing: &str| {
            let m = InstalledMod {
                sidecar: Some(sidecar(&source.mod_id_of(listing), "Listed")),
                vpks: vec![vpk("pak01_dir.vpk", Some(1), true)],
            };
            entries(std::slice::from_ref(&m)).remove(0)
        };
        let artifact = |resolver: Option<&str>| NativeArtifact {
            filename: "a.zip".into(),
            url: "https://reg.test/a.zip".into(),
            resolver_url: resolver.map(Into::into),
            ..NativeArtifact::default()
        };
        let listing = |id: &str, revision: Option<&str>, resolver: Option<&str>| NativeListing {
            id: id.into(),
            title: id.into(),
            release: revision.map(|r| NativeRelease {
                content_revision: r.into(),
                artifacts: vec![artifact(resolver)],
                ..NativeRelease::default()
            }),
            ..NativeListing::default()
        };
        let catalogs = vec![(
            source.clone(),
            vec![
                listing("live", Some("sha256-1"), None),
                listing("bare", None, None),
                listing("rotating", Some("sha256-2"), Some("https://reg.test/r")),
            ],
        )];

        let pack = complock::baked_pack();
        let current = Published::Current(std::iter::once("sha256-1".to_owned()).collect());
        assert_eq!(published(&row("live"), &catalogs, &pack), current);
        assert_eq!(published(&row("bare"), &catalogs, &pack), Published::Gone);
        assert_eq!(
            published(&row("dropped"), &catalogs, &pack),
            Published::Gone
        );
        assert_eq!(
            published(&row("live"), &[], &pack),
            Published::Unknown,
            "source not loaded"
        );

        let Some(ResetPlan::Registry(plan)) = reset_plan(&row("live"), &catalogs, &pack) else {
            panic!("a direct release resets");
        };
        assert_eq!(
            (plan.revision.as_str(), plan.url.as_str()),
            ("sha256-1", "https://reg.test/a.zip")
        );
        assert_eq!(plan.source, source);
        assert!(
            reset_plan(&row("rotating"), &catalogs, &pack).is_none(),
            "resolver urls rotate"
        );
        assert!(reset_plan(&row("bare"), &catalogs, &pack).is_none());
    }

    #[test]
    fn nsfw_fails_closed_when_unknown() {
        assert!(is_nsfw(Some(true), None));
        assert!(is_nsfw(Some(true), Some(false)));
        assert!(is_nsfw(Some(false), Some(true)));
        assert!(is_nsfw(None, Some(true)));
        assert!(is_nsfw(None, None), "legacy sidecar, no catalog");
        assert!(!is_nsfw(None, Some(false)));
        assert!(!is_nsfw(Some(false), None));
        assert!(!is_nsfw(Some(false), Some(false)));
    }

    #[test]
    fn managed_mod_uses_sidecar_name_and_thumbnail() {
        let m = InstalledMod {
            sidecar: Some(sidecar("m1", "Neon Hud")),
            vpks: vec![vpk("pak03_dir.vpk", Some(3), true)],
        };
        let e = &entries(&[m])[0];
        assert_eq!(e.id, "m1");
        assert_eq!(e.name, "Neon Hud");
        assert!(e.managed && e.enabled);
        assert_eq!(e.detail, "Version 1.2");
        assert_eq!(e.paks_detail.as_deref(), Some("pak03"));
        assert!(tooltip_detail(e).contains("pak03"));
        let mut dated = sidecar("m2", "Dated");
        dated.version = Some("2026-11-08T10:00:00.000Z".into());
        let m = InstalledMod {
            sidecar: Some(dated),
            vpks: vec![vpk("pak04_dir.vpk", Some(4), true)],
        };
        assert_eq!(entries(&[m])[0].detail, "Updated 2026-11-08");
        assert_eq!(e.thumb_url.as_deref(), Some("https://x.test/t.png"));
    }

    #[test]
    fn hand_added_vpk_shows_its_file_name() {
        let m = InstalledMod {
            sidecar: None,
            vpks: vec![vpk("custom.vpk.disabled", None, false)],
        };
        let e = &entries(&[m])[0];
        assert_eq!(e.id, "custom.vpk");
        assert_eq!(e.name, "custom.vpk");
        assert!(!e.managed && !e.enabled);
        assert_eq!(e.detail, "");
        assert_eq!(e.thumb_url, None);
    }

    #[test]
    fn mixed_state_reads_as_disabled_and_is_flagged() {
        let m = InstalledMod {
            sidecar: Some(sidecar("m1", "")),
            vpks: vec![
                vpk("pak01_dir.vpk", Some(1), true),
                vpk("pak02_dir.vpk.disabled", Some(2), false),
            ],
        };
        let e = &entries(&[m])[0];
        assert_eq!(e.name, "m1", "empty sidecar name falls back to the id");
        assert!(!e.enabled);
        assert_eq!(e.paks_detail.as_deref(), Some("paks 01, 02"));
        assert!(e.detail.contains("Partly enabled"), "{}", e.detail);
    }

    #[test]
    fn rows_number_movable_mods_in_load_order() {
        let mods = [
            InstalledMod {
                sidecar: Some(sidecar("a", "A")),
                vpks: vec![vpk("pak01_dir.vpk", Some(1), true)],
            },
            InstalledMod {
                sidecar: Some(sidecar("b", "B")),
                vpks: vec![vpk("pak02_dir.vpk", Some(2), true)],
            },
            InstalledMod {
                sidecar: None,
                vpks: vec![vpk("hand.vpk", None, true)],
            },
        ];
        let e = entries(&mods);
        let got: Vec<_> = e
            .iter()
            .map(|e| (e.position, e.can_up, e.can_down))
            .collect();
        assert_eq!(got, [(1, false, true), (2, true, false), (0, false, false)]);
    }

    #[test]
    fn typed_paths_expand_home_uris_and_quotes() {
        let home = Path::new("/home/kai");
        assert_eq!(
            typed_path("~/mods/a.vpk", Some(home)),
            home.join("mods/a.vpk")
        );
        assert_eq!(
            typed_path(" 'file:///tmp/b.zip' ", Some(home)),
            PathBuf::from("/tmp/b.zip")
        );
        assert_eq!(
            typed_path("file:///tmp/my%20mod%20%281%29.zip", Some(home)),
            PathBuf::from("/tmp/my mod (1).zip")
        );
        assert_eq!(
            typed_path("file://localhost/tmp/b%20c.vpk", Some(home)),
            PathBuf::from("/tmp/b c.vpk")
        );
        assert_eq!(typed_path("\"/x/c.vpk\"", None), PathBuf::from("/x/c.vpk"));
        assert_eq!(typed_path("~/d.vpk", None), PathBuf::from("~/d.vpk"));
    }

    #[test]
    fn pick_cancel_distinguishes_portal_failure() {
        assert_eq!(
            pick_cancel_text(std::time::Duration::from_millis(5)),
            PICKER_UNAVAILABLE
        );
        assert_eq!(
            pick_cancel_text(std::time::Duration::from_secs(1)),
            NO_FILE_PICKED
        );
    }

    #[test]
    fn batch_banners_count_what_is_left() {
        assert!(
            queued_banner(1).contains("1 file queued"),
            "{}",
            queued_banner(1)
        );
        assert!(
            queued_banner(3).contains("3 files queued"),
            "{}",
            queued_banner(3)
        );
        assert!(
            stopped_banner(1).contains("1 more dropped file"),
            "{}",
            stopped_banner(1)
        );
        assert!(
            stopped_banner(4).contains("4 more dropped files"),
            "{}",
            stopped_banner(4)
        );
    }

    #[test]
    fn typed_pak_numbers_parse_leniently() {
        assert_eq!(typed_pak("05"), Some(5));
        assert_eq!(typed_pak(" pak05 "), Some(5));
        assert_eq!(typed_pak("PAK40"), Some(40));
        assert_eq!(typed_pak("7"), Some(7));
        assert_eq!(typed_pak(""), None);
        assert_eq!(typed_pak("  "), None);
        assert_eq!(typed_pak("many"), None);
        assert_eq!(typed_pak("-1"), None);
        assert_eq!(typed_pak("1.5"), None);
    }

    #[test]
    fn progress_steps_survive_a_zero_content_length() {
        assert_eq!(progress_step(50, Some(100)), 50);
        assert_eq!(progress_step(150, Some(100)), 100, "clamped by the UI");
        assert_eq!(progress_step(1, Some(0)), 0, "no divide by zero");
        assert_eq!(progress_step(2_500_000, None), 2, "one step per MB");
        assert_eq!(progress_step(0, None), 0);
    }

    #[test]
    fn link_file_names_decode_and_never_travel() {
        assert_eq!(
            link_file_name("https://host/dl/Haze%20Skin.zip"),
            "Haze Skin.zip"
        );
        assert_eq!(
            link_file_name("https://host/dl/a.vpk?token=x#frag"),
            "a.vpk",
            "query and fragment dropped"
        );
        assert_eq!(link_file_name("https://host/dl/%C3%A4.vpk"), "ä.vpk");
        assert_eq!(
            link_file_name("https://host/dl/%ZZ.vpk"),
            "%ZZ.vpk",
            "a broken escape stays as written"
        );
        assert_eq!(link_file_name("https://host/"), "mod");
        assert_eq!(link_file_name("https://host/dl/"), "mod");
        assert_eq!(link_file_name("https://host/dl/.."), "mod");
        assert_eq!(link_file_name("https://host/dl/.hidden"), ".hidden");
        // A decoded separator is refused, not written to disk.
        assert_eq!(link_file_name("https://host/dl/a%2Fb.vpk"), "mod");
        assert_eq!(link_file_name("https://host/dl/a%5Cb.vpk"), "mod");
    }

    #[test]
    fn entries_carry_the_lowest_pak_number() {
        let multi = InstalledMod {
            sidecar: Some(sidecar("m1", "Multi")),
            vpks: vec![
                vpk("pak07_dir.vpk", Some(7), true),
                vpk("pak03_dir.vpk.disabled", Some(3), false),
            ],
        };
        let hand = InstalledMod {
            sidecar: None,
            vpks: vec![vpk("custom.vpk", None, true)],
        };
        let e = entries(&[multi, hand]);
        assert_eq!(e[0].pak, 3, "the lowest number decides where it sits");
        assert_eq!(e[1].pak, 0);
    }

    #[test]
    fn local_mods_show_notes_and_files() {
        let mut sc = sidecar("local-1-0", "Outlines");
        sc.notes = Some("HoppCX".into());
        sc.version = None;
        let m = InstalledMod {
            sidecar: Some(sc),
            vpks: vec![vpk("pak56_dir.vpk", Some(56), true)],
        };
        let e = &entries(&[m])[0];
        assert!(e.managed && e.local);
        assert_eq!(e.notes.as_deref(), Some("HoppCX"));
        assert_eq!(e.files, ["pak56_dir.vpk"]);
        assert_eq!(e.raw_name, "Outlines");

        let hand = InstalledMod {
            sidecar: None,
            vpks: vec![vpk("pak55_dir.vpk.disabled", Some(55), false)],
        };
        let e = &entries(&[hand])[0];
        assert!(!e.managed && !e.local);
        assert_eq!(e.files, ["pak55_dir.vpk"]);
    }

    #[test]
    fn version_display_hides_hashes_and_paks_while_keeping_them_for_tooltips() {
        let mut comp = sidecar("complock-soundcap", "Sound cap");
        comp.version = Some("crc32:deadbeef".into());
        comp.version_label = Some("2026-11-22".into());
        let mut reg = sidecar("src-my-mod", "My Reg Mod");
        reg.version = Some("sha256:abcdef0123456789".into());
        let mut items = entries(&[
            InstalledMod {
                sidecar: Some(comp),
                vpks: vec![vpk("pak03_dir.vpk", Some(3), true)],
            },
            InstalledMod {
                sidecar: Some(reg),
                vpks: vec![vpk("pak04_dir.vpk", Some(4), true)],
            },
        ]);
        let pack = complock::baked_pack();
        items[0].drift = Drift::Pristine;
        assert_eq!(
            display_version(&items[0], &[], Some(&pack)),
            "Updated 2026-11-22"
        );
        assert!(!items[0].detail.contains("crc32"));
        assert!(!items[0].detail.contains("pak03"));
        let tip = tooltip_detail(&items[0]);
        assert!(
            tip.contains("crc32:deadbeef") && tip.contains("pak03"),
            "{tip}"
        );

        assert_eq!(display_version(&items[1], &[], Some(&pack)), "");
        assert_eq!(source_tag(&items[0]), (SourceKind::Complock, "CompLock"));
        assert_eq!(source_tag(&items[1]), (SourceKind::Registry, "Registry"));
        let mut local = entries(&[InstalledMod {
            sidecar: Some(sidecar("local-1-0", "Manual Mod")),
            vpks: vec![vpk("pak09_dir.vpk", Some(9), true)],
        }]);
        assert_eq!(source_tag(&local[0]), (SourceKind::Manual, "Manual"));
        local[0].facts.is_complock_builder = true;
        assert_eq!(source_tag(&local[0]), (SourceKind::Complock, "CompLock"));
    }
}
