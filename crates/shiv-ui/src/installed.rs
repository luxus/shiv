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
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use shiv_api::{ApiError, Client};
use shiv_core::addons::{self, AddonsError};
use shiv_core::game::{self, GameError, GamePaths};
use shiv_core::install::{self, InstallError, MatchReason};
use shiv_core::interchange;
use shiv_core::sidecar::{self, InstalledMod, Move};
use shiv_repair::OverlayState;
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel, Weak};

use crate::browse::{game_error_text, install_error_text, to_slint};
use crate::thumbs;
use crate::{
    App, DropState, InstalledRow, InstalledState, LoadState, LocalInstallState, ModEditState,
    UpdateFileState,
};

pub(crate) type Locate = Arc<dyn Fn() -> Result<GamePaths, GameError> + Send + Sync>;
pub(crate) type Running = Arc<dyn Fn() -> bool + Send + Sync>;

/// Where the game is and whether it runs; injectable so tests never touch the
/// real install or depend on a running game.
#[derive(Clone)]
pub(crate) struct Deps {
    pub locate: Locate,
    pub running: Running,
}

impl Deps {
    fn system() -> Self {
        Deps {
            locate: Arc::new(crate::locate_game),
            running: Arc::new(game::is_running),
        }
    }
}

const GAME_RUNNING_TEXT: &str =
    "Deadlock is running. Close it to enable, disable or uninstall mods.";
const SUFFIX: &str = ".disabled";
/// Shown when the file dialog returns nothing: cancelled, or no file dialog
/// service (xdg-desktop-portal) is reachable.
const NO_FILE_PICKED: &str =
    "No file picked. You can also type or paste a path or a link, or drop the file on the window.";

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

// ---------------------------------------------------------------------------
// Row view-models (pure, unit-tested)

#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// 1-based place in the load order (1 loads first and wins conflicts);
    /// 0 for a mod that has no pak number and cannot be moved.
    pub position: u32,
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
        (Some(true), _) | (_, Some(true)) => true,
        (None, None) => true,
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
            position: 0,
            can_up: false,
            can_down: false,
            pak: m.vpks.iter().filter_map(|v| v.pak).min().unwrap_or(0),
        };
    };
    let mut parts: Vec<String> = Vec::new();
    if let Some(v) = s.version.as_deref().filter(|v| !v.is_empty()) {
        // Shiv records the mod's update timestamp as its version.
        let v = crate::browse::short_date(v).unwrap_or(v);
        parts.push(format!("Version {v}"));
    }
    let mut paks: Vec<u32> = m.vpks.iter().filter_map(|v| v.pak).collect();
    paks.sort_unstable();
    paks.dedup();
    match paks.as_slice() {
        [] => {}
        [one] => parts.push(format!("pak{one:02}")),
        many => {
            let list: Vec<String> = many.iter().map(|p| format!("{p:02}")).collect();
            parts.push(format!("paks {}", list.join(", ")));
        }
    }
    if any_enabled && !enabled {
        parts.push("Partly enabled".into());
    }
    Entry {
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
        detail: parts.join(" \u{b7} "),
        managed: true,
        enabled,
        thumb_url: s.thumbnail_url.clone().filter(|u| !u.is_empty()),
        nsfw: s.nsfw,
        version: s.version.clone(),
        position: 0,
        can_up: false,
        can_down: false,
        pak: m.vpks.iter().filter_map(|v| v.pak).min().unwrap_or(0),
    }
}

/// Rows for `mods` (a load-ordered `sidecar::group` result), numbering the
/// ones that can be moved.
pub(crate) fn entries(mods: &[InstalledMod]) -> Vec<Entry> {
    let movable = mods
        .iter()
        .filter(|m| m.load_position() != u32::MAX)
        .count() as u32;
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

// ---------------------------------------------------------------------------
// Worker side

struct Loaded {
    mods: Vec<InstalledMod>,
    running: bool,
    /// The repair package no longer matches the mods (after a toggle or an
    /// uninstall, say) but still loads.
    repair_stale: bool,
}

/// `addons::list` runs first: `sidecar::group` swallows an unreadable folder
/// and would show an empty list instead of an error.
fn load(deps: &Deps) -> Result<Loaded, String> {
    let paths = (deps.locate)().map_err(|e| game_error_text(&e))?;
    addons::list(&paths.addons).map_err(|e| format!("The addons folder could not be read. {e}"))?;
    Ok(Loaded {
        mods: sidecar::group(&paths.addons),
        running: (deps.running)(),
        repair_stale: shiv_repair::overlay_state(&paths) == OverlayState::Stale,
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
            // Renumbering shares the install lock: a rename while another
            // worker stages an install would race its pak allocation.
            let _guard = crate::browse::INSTALL_LOCK
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            sidecar::move_mod(&paths.addons, mods, index, to).map_err(|e| fail(&e.to_string()))
        }
        Action::Uninstall => {
            let running = deps.running.clone();
            install::uninstall_with(m, move || running()).map_err(|e| match e {
                InstallError::GameRunning => fail(GAME_RUNNING_TEXT),
                other => fail(&install_error_text(&other)),
            })
        }
    }
}

// ---------------------------------------------------------------------------
// Link installs (custom sources)

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
    let fetched = client.fetch_cancellable(url, &dest, cancel, |done, total| {
        let step = progress_step(done, total);
        if step == last_step {
            return;
        }
        last_step = step;
        report(LinkUpdate::Progress { done, total });
    });
    match fetched {
        Err(ApiError::Cancelled) => return Err(LinkFailure::Cancelled),
        Err(e) => return Err(link_fail(format!("Download failed: {e}"))),
        Ok(()) => {}
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(LinkFailure::Cancelled);
    }
    report(LinkUpdate::Installing);
    let _guard = crate::browse::INSTALL_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // Cancel may have been pressed while waiting for another install.
    if cancel.load(Ordering::Relaxed) {
        return Err(LinkFailure::Cancelled);
    }
    let sc = install::install_local_with(&dest, name, &paths, move || running())
        .map_err(|e| link_fail(install_error_text(&e)))?;
    Ok(format!("Installed {} as {}.", sc.name, sc.vpks.join(", ")))
}

/// Wrap a worker-side message as a link-install failure.
fn link_fail(text: String) -> LinkFailure {
    LinkFailure::Failed(text)
}

// ---------------------------------------------------------------------------
// Controller

struct Installed {
    weak: Weak<App>,
    deps: Deps,
    cache: Arc<thumbs::Cache>,
    rows: Rc<VecModel<InstalledRow>>,
    mods: RefCell<Vec<InstalledMod>>,
    entries: RefCell<Vec<Entry>>,
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
pub fn attach(app: &App, cache: Arc<thumbs::Cache>) {
    attach_with(app, cache, Deps::system());
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
    });
    INSTALLED.with(|c| *c.borrow_mut() = Some(controller));
    let state = app.global::<InstalledState>();
    state.set_rows(ModelRc::from(rows));
    state.on_refresh(|| with_installed(|i| i.refresh(true)));
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
        let to = Move::To((position.max(1) - 1) as usize);
        with_installed(|i| i.act(id.as_str(), Action::Move(to)));
    });
    state.on_update(|id| crate::browse::update_mod(id.as_str()));
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
}

/// A path typed or pasted into the install dialog: `~/` is the home folder,
/// a `file://` URI its path, surrounding quotes and spaces are dropped.
pub(crate) fn typed_path(text: &str, home: Option<&Path>) -> PathBuf {
    let t = text.trim().trim_matches(|c| c == '"' || c == '\'');
    let t = t.strip_prefix("file://").unwrap_or(t);
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

/// Whether the text in the install dialog is a download link rather than a
/// local path.
pub(crate) fn is_link(text: &str) -> bool {
    let t = text.trim();
    t.starts_with("http://") || t.starts_with("https://")
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
        let entries = i.entries.borrow();
        let pos = entries.iter().position(|e| e.id == id);
        if let Some(pos) = pos.filter(|p| *p < i.rows.row_count()) {
            i.rows.set_row_data(pos, i.build_row(&entries[pos]));
        }
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
// Only the Wayland drop path calls this; macOS builds without it, so the
// lint fires there while Linux and the tests keep it alive.
#[cfg_attr(target_vendor = "apple", allow(dead_code))]
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
        if !i.idle() {
            return;
        }
        let weak = i.weak.clone();
        crate::spawn_worker(
            "shiv-pick-export",
            || {
                rfd::FileDialog::new()
                    .set_title("Export your mods for another mod manager")
                    .pick_folder()
            },
            move |picked| {
                let Some(folder) = picked.ok().flatten() else {
                    return;
                };
                let _ = weak.upgrade_in_event_loop(move |_| {
                    with_installed(|i| i.export_bundle(folder));
                });
            },
        );
    });
}

/// Pick an interchange bundle folder (or its manifest) and import the mods
/// it holds (the Installed screen's Import button).
pub(crate) fn pick_import_folder() {
    with_installed(|i| {
        if !i.idle() {
            return;
        }
        let weak = i.weak.clone();
        crate::spawn_worker(
            "shiv-pick-import",
            || {
                rfd::FileDialog::new()
                    .set_title("Import mods from another mod manager")
                    .pick_folder()
            },
            move |picked| {
                let Some(folder) = picked.ok().flatten() else {
                    return;
                };
                let _ = weak.upgrade_in_event_loop(move |_| {
                    with_installed(|i| i.import_bundle(folder));
                });
            },
        );
    });
}

/// Export without the folder picker (tests drive this directly).
pub(crate) fn export_bundle_to(folder: PathBuf) {
    with_installed(|i| i.export_bundle(folder));
}

/// Import without the folder picker (tests drive this directly).
pub(crate) fn import_bundle_from(folder: PathBuf) {
    with_installed(|i| i.import_bundle(folder));
}

impl Installed {
    /// Write the installed library into a new bundle inside `folder`.
    fn export_bundle(&self, folder: PathBuf) {
        self.with_state(|s| {
            s.set_banner_text("Exporting mods\u{2026}".into());
        });
        self.set_flags(None, Some(true));
        let deps = self.deps.clone();
        let weak = self.weak.clone();
        let started = crate::spawn_worker(
            "shiv-export",
            move || -> Result<String, String> {
                let paths = (deps.locate)().map_err(|e| game_error_text(&e))?;
                let report = interchange::export(&paths.addons, &folder, &|id| {
                    crate::browse::catalog_remote(id).map(|(submission_id, is_audio)| {
                        interchange::RemoteRef {
                            submission_id,
                            is_audio,
                        }
                    })
                })
                .map_err(|e| e.to_string())?;
                let mut text = format!(
                    "Exported {} to {}.",
                    report.exported.len(),
                    report.root.display()
                );
                for (name, why) in &report.skipped {
                    text.push_str(&format!(" {name} was skipped: {why}."));
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
        self.with_state(|s| {
            s.set_banner_text("Importing mods\u{2026}".into());
        });
        self.set_flags(None, Some(true));
        let deps = self.deps.clone();
        let weak = self.weak.clone();
        let started = crate::spawn_worker(
            "shiv-import",
            move || -> Result<String, String> {
                let paths = (deps.locate)().map_err(|e| game_error_text(&e))?;
                let running = deps.running.clone();
                let report = interchange::import(&folder, &paths, move || running(), &|key| {
                    key.strip_prefix("gamebanana:")
                        .and_then(|rest| {
                            let (kind, id) = rest.split_once(':')?;
                            Some((id.to_owned(), kind == "sound"))
                        })
                        .and_then(|(id, _)| {
                            // The catalog lists Shiv ids, not GameBanana
                            // submissions; find the mod whose remote id this
                            // key names.
                            crate::browse::catalog_remote_of_submission(&id)
                        })
                })
                .map_err(|e| e.to_string())?;
                let mut text = format!(
                    "Imported {} mods ({} already on disk).",
                    report.installed.len(),
                    report.adopted.len()
                );
                for (name, why) in &report.skipped {
                    text.push_str(&format!(" {name}: {why}."));
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

    fn build_row(&self, e: &Entry) -> InstalledRow {
        let mut row = InstalledRow {
            id: e.id.as_str().into(),
            name: e.name.as_str().into(),
            detail: e.detail.as_str().into(),
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
                (false, p) => format!("Downloading {}%", (p * 100.0).round() as u32).into(),
            };
        }
        if e.managed
            && !e.local
            && update_available(
                e.version.as_deref(),
                crate::browse::catalog_version(&e.id).as_deref(),
            )
        {
            row.update = true;
            row.tag = "Update available".into();
        }
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
    }

    fn loaded(&self, generation: u64, result: Result<Loaded, String>) {
        if generation != self.generation.get() {
            return;
        }
        let loaded = match result {
            Ok(l) => l,
            Err(text) => return self.fail(text),
        };
        let list = entries(&loaded.mods);
        self.rows
            .set_vec(list.iter().map(|e| self.build_row(e)).collect::<Vec<_>>());
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
            // Switching on a mod another enabled mod already replaces: ask first.
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
        if self.acting.get() {
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
        // A snapshot to work on: the mods list, where the mod sits in it,
        // the mod itself, and the pak number to move it to (when asked).
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
            // A new pak number moves the mod in the load order; an empty or
            // unchanged field leaves it alone. The range is checked here;
            // deeper conflicts (a move that would push another VPK past 99)
            // are refused by the worker without touching anything.
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
        let (mods, index, mod_, pak_to_set) = job;
        s.set_busy(true);
        s.set_error_text("".into());
        self.set_flags(None, Some(true));
        let deps = self.deps.clone();
        let weak = self.weak.clone();
        let started = crate::spawn_worker(
            "shiv-installed-edit",
            move || -> Result<(), String> {
                let paths = (deps.locate)().map_err(|e| game_error_text(&e))?;
                // Renumbering shares the install lock: a rename while another
                // worker stages an install would race its pak allocation.
                let _guard = crate::browse::INSTALL_LOCK
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                // The sidecar edit runs first: it is idempotent, so a failed
                // `set_pak` after it can be retried without renaming twice.
                let done = if adopt {
                    sidecar::adopt(&paths.addons, &mod_, &name, notes)
                } else {
                    sidecar::edit(&paths.addons, &id, &name, notes)
                };
                done.map(|_| ()).map_err(|e| e.to_string())?;
                if let Some(pak) = pak_to_set {
                    sidecar::set_pak(&paths.addons, &mods, index, pak)
                        .map_err(|e| e.to_string())?;
                }
                Ok(())
            },
            move |result| {
                let result = result.and_then(|r| r);
                let _ = weak.upgrade_in_event_loop(move |_| {
                    with_installed(|i| i.edited(result));
                });
            },
        );
        if !started {
            self.edited(Err("Could not start a background thread.".into()));
        }
    }

    fn edited(&self, result: Result<(), String>) {
        self.with_app(|app| {
            let s = app.global::<ModEditState>();
            s.set_busy(false);
            match &result {
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

    /// The system file dialog blocks, so it runs on a worker.
    fn browse_file(&self) {
        let weak = self.weak.clone();
        crate::spawn_worker(
            "shiv-pick-file",
            || {
                rfd::FileDialog::new()
                    .set_title("Install a mod from a file")
                    .add_filter("Mods", &["vpk", "zip", "rar", "7z"])
                    .add_filter("All files", &["*"])
                    .pick_file()
            },
            move |picked| {
                let picked = picked.ok().flatten();
                let _ = weak.upgrade_in_event_loop(move |app| {
                    let s = app.global::<LocalInstallState>();
                    match picked {
                        Some(path) => {
                            if s.get_name().is_empty() {
                                s.set_name(default_name(&path).into());
                            }
                            s.set_path(path.to_string_lossy().as_ref().into());
                            s.set_error_text("".into());
                        }
                        None => {
                            if s.get_path().is_empty() {
                                s.set_error_text(NO_FILE_PICKED.into());
                            }
                        }
                    }
                });
            },
        );
    }

    fn install_file(&self) {
        if !self.idle() {
            return;
        }
        let Some(app) = self.weak.upgrade() else {
            return;
        };
        let s = app.global::<LocalInstallState>();
        // A link downloads first; everything else is a local path.
        let text = s.get_path().to_string();
        if is_link(&text) {
            self.install_link(&text, &s.get_name());
            return;
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
            move || -> Result<String, String> {
                let paths = (deps.locate)().map_err(|e| game_error_text(&e))?;
                let running = deps.running.clone();
                // The lock every install path shares: pak slots are
                // allocated against the folder while staging, so an install
                // must not renumber beside another.
                let _guard = crate::browse::INSTALL_LOCK
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let sc = install::install_local_with(&path, &name, &paths, move || running())
                    .map_err(|e| install_error_text(&e))?;
                Ok(format!("Installed {} as {}.", sc.name, sc.vpks.join(", ")))
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

    fn installed_file(&self, result: Result<String, String>) {
        self.with_app(|app| {
            let s = app.global::<LocalInstallState>();
            s.set_busy(false);
            match &result {
                Ok(_) => s.set_open(false),
                Err(text) => s.set_error_text(text.as_str().into()),
            }
        });
        match result {
            Ok(text) => {
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
        // The mod name defaults to the link's file stem, like a local file.
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
                    crate::downloads::progress(&job_id, done, total)
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
                crate::downloads::finished(id, crate::downloads::Outcome::Done);
                self.with_state(|s| s.set_banner_text(banner.into()));
                self.acted(Ok(()));
            }
            Err(LinkFailure::Cancelled) => {
                crate::downloads::finished(id, crate::downloads::Outcome::Cancelled);
            }
            Err(LinkFailure::Failed(text)) => {
                crate::downloads::finished(id, crate::downloads::Outcome::Failed(text.clone()));
                self.with_state(|s| {
                    s.set_banner_text(format!("Could not install {name}. {text}").into())
                });
            }
        }
    }

    /// Whether any of the Installed dialogs is open, or an install conflict
    /// is being offered: dropped files queue behind all of them.
    fn dialog_open(&self, app: &App) -> bool {
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
            || {
                rfd::FileDialog::new()
                    .set_title("Update a mod from a file")
                    .add_filter("Mods", &["vpk", "zip", "rar", "7z"])
                    .add_filter("All files", &["*"])
                    .pick_file()
            },
            move |picked| {
                let picked = picked.ok().flatten();
                let _ = weak.upgrade_in_event_loop(move |app| {
                    let s = app.global::<UpdateFileState>();
                    match picked {
                        Some(path) => {
                            s.set_path(path.to_string_lossy().as_ref().into());
                            s.set_error_text("".into());
                        }
                        None if s.get_path().is_empty() => s.set_error_text(NO_FILE_PICKED.into()),
                        None => {}
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
        let Some(id) = self.updating.borrow().clone() else {
            return;
        };
        if !self.idle() {
            return;
        }
        let Some(app) = self.weak.upgrade() else {
            return;
        };
        let s = app.global::<UpdateFileState>();
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
        s.set_busy(true);
        s.set_error_text("".into());
        self.set_flags(None, Some(true));
        let deps = self.deps.clone();
        let weak = self.weak.clone();
        let started = crate::spawn_worker(
            "shiv-replace-file",
            move || -> Result<String, String> {
                let paths = (deps.locate)().map_err(|e| game_error_text(&e))?;
                let running = deps.running.clone();
                let r = install::replace_local_with(&path, &target, &paths, move || running())
                    .map_err(|e| install_error_text(&e))?;
                let backup = r
                    .backup
                    .strip_prefix(&paths.addons)
                    .unwrap_or(&r.backup)
                    .display()
                    .to_string();
                Ok(format!(
                    "Updated {name} in {}. The old file is in addons/{backup}.",
                    r.sidecar.vpks.join(", ")
                ))
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

    fn replaced(&self, result: Result<String, String>) {
        self.with_app(|app| {
            let s = app.global::<UpdateFileState>();
            s.set_busy(false);
            match &result {
                Ok(_) => s.set_open(false),
                Err(text) => s.set_error_text(text.as_str().into()),
            }
        });
        match result {
            Ok(text) => {
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
        // A dialog, a re-read or an action in flight holds the queue.
        if self.dialog_open(&app) || !self.idle() {
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
                    with_installed(|i| i.inspected(path_for_result, result));
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
        path: PathBuf,
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
                        self.show_update(&id, &name, &shown, reason_text(reason), true)
                    }
                    None => self.show_local_install(&path),
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

    /// Files were dropped on the window. They are queued and processed one
    /// by one; anything that is not a file is skipped with a note.
    fn dropped_batch(&self, paths: Vec<PathBuf>) {
        let Some(app) = self.weak.upgrade() else {
            return;
        };
        app.global::<DropState>().set_hover(false);
        if app.get_screen() != 1 {
            app.set_screen(1);
        }
        let (files, skipped): (Vec<PathBuf>, Vec<PathBuf>) =
            paths.into_iter().partition(|p| p.is_file());
        let n_queued = {
            let mut pending = self.pending.borrow_mut();
            pending.extend(files);
            pending.len()
        };
        let held = self.dialog_open(&app) || !self.idle() || self.stepping.get();
        let mut parts: Vec<String> = Vec::new();
        if !skipped.is_empty() {
            let n = skipped.len();
            parts.push(if n == 1 {
                "Skipped 1 dropped item that is not a file.".into()
            } else {
                format!("Skipped {n} dropped items that are not files.")
            });
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
            entries
                .iter()
                .position(|e| e.id == id)
                .map(|i| (i, entries[i].name.clone()))
        };
        let Some((index, name)) = found else {
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
                let _ = weak.upgrade_in_event_loop(move |_| {
                    with_installed(|i| i.acted(result));
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
        // The disk changed: keep the Launch sidebar count and Browse's
        // Installed badges current.
        crate::launch::refresh();
        crate::browse::refresh_installed();
        self.with_state(|s| {
            if let Err(text) = &result {
                s.set_banner_text(text.as_str().into());
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
    use shiv_core::addons::InstalledVpk;
    use shiv_core::sidecar::Sidecar;
    use std::path::PathBuf;

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
            thumbnail_url: Some("https://x.test/t.png".into()),
            vpks: Vec::new(),
            nsfw: None,
            hero: None,
            category: None,
            notes: None,
            source: None,
        }
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
        assert_eq!(e.detail, "Version 1.2 \u{b7} pak03");
        let mut dated = sidecar("m2", "Dated");
        dated.version = Some("2026-11-08T10:00:00.000Z".into());
        let m = InstalledMod {
            sidecar: Some(dated),
            vpks: vec![vpk("pak04_dir.vpk", Some(4), true)],
        };
        assert_eq!(entries(&[m])[0].detail, "Version 2026-11-08 \u{b7} pak04");
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
        assert!(e.detail.contains("paks 01, 02"), "{}", e.detail);
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
        assert_eq!(typed_path("\"/x/c.vpk\"", None), PathBuf::from("/x/c.vpk"));
        assert_eq!(typed_path("~/d.vpk", None), PathBuf::from("~/d.vpk"));
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
    fn links_are_told_apart_from_paths() {
        assert!(is_link("https://host/Haze Skin.vpk"));
        assert!(is_link(" http://host/mod.zip "));
        assert!(!is_link("/home/kai/Haze Skin.vpk"));
        assert!(!is_link("file:///tmp/a.vpk"));
        assert!(!is_link(""));
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
}
