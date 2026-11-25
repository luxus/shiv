//! Performance screen controller: loads the perf catalog (API, with the last
//! good copy cached on disk by `ETag`), lists every setting with its value in
//! `gameinfo.gi`, stages presets, the CompLock set and single edits as
//! pending changes, and writes them with `shiv_core::perf::apply`.
//!
//! Network and file work runs on worker threads that post back with
//! `upgrade_in_event_loop`, like the other screens. Editing is always
//! allowed; Apply is refused while Deadlock runs (checked on the worker).
//! Showing the screen re-reads `gameinfo.gi` (a game update rewrites it) and
//! keeps the pending changes.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use shiv_api::Client;
use shiv_core::complock;
use shiv_core::install::{self, ModMeta};
use shiv_core::perf::{
    self, Catalog, CatalogCache, ConfigEntry, ConvarKind, ConvarMeta, ConvarStatus, Tier,
};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel, Weak};

use crate::browse::{catalog_error_text, fmt_size, game_error_text, install_error_text};
use crate::installed::{Locate, Running};
use crate::{
    App, LoadState, PerfChange, PerfComplock, PerfControl, PerfEditor, PerfPackKind, PerfPackRow,
    PerfRow, PerfState, PerfTier,
};

/// `perf_catalog(etag)`: `Ok(None)` when the cached copy is current.
pub(crate) type Fetch =
    Arc<dyn Fn(Option<&str>) -> Result<Option<(String, Vec<u8>)>, String> + Send + Sync>;

/// One pak piece download: `url` to `dest`, stopping once `cancel` is set,
/// reporting bytes done and the total when known.
pub(crate) type PackDownload = Arc<
    dyn Fn(&str, &PathBuf, &AtomicBool, &mut dyn FnMut(u64, Option<u64>)) -> Result<(), String>
        + Send
        + Sync,
>;

/// The site's front page, HTML. Tests pass a closure; nothing here opens a socket.
pub(crate) type PackPage = Arc<dyn Fn(&str) -> Result<String, String> + Send + Sync>;

/// A page fetch that fails, so a test stays on the baked pack.
#[cfg(test)]
#[must_use]
pub(crate) fn pack_page_offline() -> PackPage {
    Arc::new(|_| Err("pack page is not fetched in tests".into()))
}

/// Injectable so tests never touch the real install, the network or the
/// user's cache.
#[derive(Clone)]
pub(crate) struct Deps {
    pub locate: Locate,
    pub running: Running,
    pub fetch: Fetch,
    pub cache: Option<CatalogCache>,
    pub pack_download: PackDownload,
    pub pack_page: PackPage,
    pub pack_cache: Option<PathBuf>,
}

impl Deps {
    pub(crate) fn system(api_url: &str) -> Self {
        let client = Arc::new(Client::new(api_url));
        let download = client.clone();
        Deps {
            locate: Arc::new(crate::locate_game),
            running: Arc::new(shiv_core::game::running_here),
            fetch: Arc::new(move |etag| {
                client
                    .perf_catalog(etag)
                    .map_err(|e| catalog_error_text(&e))
            }),
            cache: CatalogCache::default_dir().map(CatalogCache::new),
            pack_download: Arc::new(move |url, dest, cancel, progress| {
                download
                    .fetch_cancellable(url, dest, cancel, progress)
                    .map_err(|e| e.to_string())
            }),
            pack_page: system_pack_page(),
            pack_cache: system_pack_cache(),
        }
    }
}

fn system_pack_page() -> PackPage {
    #[cfg(test)]
    {
        pack_page_offline()
    }
    #[cfg(not(test))]
    {
        Arc::new(fetch_pack_page)
    }
}

fn system_pack_cache() -> Option<PathBuf> {
    #[cfg(test)]
    {
        None
    }
    #[cfg(not(test))]
    {
        complock::pack_cache_dir()
    }
}

/// Worker body: fetch the CompLock pack page over HTTPS with bounded timeouts and size.
#[cfg(not(test))]
fn fetch_pack_page(url: &str) -> Result<String, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(std::time::Duration::from_secs(10)))
        .timeout_recv_response(Some(std::time::Duration::from_secs(20)))
        .timeout_recv_body(Some(std::time::Duration::from_secs(20)))
        .build()
        .into();
    let mut resp = agent
        .get(url)
        .header("user-agent", concat!("shiv/", env!("CARGO_PKG_VERSION")))
        .call()
        .map_err(|e| e.to_string())?;
    resp.body_mut()
        .with_config()
        .limit(2 * 1024 * 1024)
        .read_to_string()
        .map_err(|e| e.to_string())
}

const GAME_RUNNING_TEXT: &str = "Deadlock is running. Close it, then apply again.";
const CONVARS: &str = "ConVars";
const COMPLOCK_TITLE: &str = "CompLock engine settings";
const COMPLOCK_NOTE: &str = "From their site: their paks replace whole files and do not mix with QOL Lock and similar packs; cloth-off conflicts with mods that replace hero models. A HUD++ update can hide other mods' entries in its settings menu until those mods update.";
/// Prefix of the mod ids Shiv gives the CompLock paks it installs: stable
/// per option, so re-installing an option updates it in place through
/// `install::install_files` on the same id (the Downloads rows and the
/// cancel routing key on it too).
const PACK_PREFIX: &str = complock::MOD_PREFIX;
const PACK_HUD_ID: &str = complock::HUD_ID;
const PACK_FOV_ID: &str = "fov";

/// The mod id of a CompLock pak: `complock-<option id>`, or
/// `complock-hud` for the HUD++ base whatever damage numbers it carries.
fn pack_mod_id(option_id: &str) -> String {
    format!("{PACK_PREFIX}{option_id}")
}

/// Whether a mod id names a CompLock pak (Downloads routing).
#[must_use]
pub fn is_pack_id(mod_id: &str) -> bool {
    mod_id.starts_with(PACK_PREFIX) && !shiv_core::generated::is_id(mod_id)
}

/// One pak install: what to download and how the result is recorded.
#[derive(Clone)]
struct PackTask {
    mod_id: String,
    name: String,
    pak: complock::Pak,
}

/// Everything a row in the CompLock options list is built from.
struct PackRowInfo<'a> {
    kind: PerfPackKind,
    id: &'a str,
    title: &'a str,
    detail: String,
    by: Option<&'a str>,
    link: Option<&'a str>,
    link_text: Option<&'a str>,
    /// Measured gains, "+9.7%".
    avg: Option<&'a str>,
    low: Option<&'a str>,
    text: Option<&'a str>,
    warn: Option<&'a str>,
    convars: &'a [ConfigEntry],
    choices: &'a [complock::Choice],
    pak: Option<&'a complock::Pak>,
}

impl PackTask {
    fn of(mod_id: String, name: String, pak: &complock::Pak) -> Self {
        PackTask {
            mod_id,
            name,
            pak: pak.clone(),
        }
    }
}

/// Result of a completed CompLock pak install/update for Activity logging.
#[derive(Debug)]
struct PackDone {
    note: String,
    was_installed: bool,
    from_version: Option<String>,
    to_version: Option<String>,
}

/// What a pack-install worker reports while it runs.
enum PackUpdate {
    Progress {
        done: u64,
        total: Option<u64>,
    },
    Installing,
    /// `Ok` carries the install summary and whether an earlier revision was replaced.
    Done(Result<PackDone, (String, bool)>),
}

/// A pack install stopped before it finished; matches what the Downloads
/// screen shows for a cancelled transfer.
const CANCELLED_TEXT: &str = "Cancelled";

struct PreparedParts {
    vpks: Vec<PathBuf>,
    in_place: Vec<String>,
    free_names: Vec<complock::Part>,
    busy: Vec<(String, complock::ReusedPart)>,
}

/// Worker body: download the pak's pieces (a big pak arrives in
/// numbered pieces that concatenate into one raw-DEFLATE stream), unpack
/// and CRC-gate them into a VPK, then install it as a mod through the same
/// lock every install takes. Mirrors Browse's `run_install`: the game is
/// checked before the download so it is not wasted, and progress reports
/// percent steps over the pack's declared download size.
fn run_pack_install(
    deps: &Deps,
    task: PackTask,
    cancel: &AtomicBool,
    mut report: impl FnMut(PackUpdate),
    ask: impl Fn(&[install::SlotOccupant]) -> Option<install::SlotChoice>,
) -> Result<PackDone, (String, bool)> {
    let fail = |text: String| (text, false);
    let paths = (deps.locate)().map_err(|e| fail(game_error_text(&e)))?;
    if (deps.running)() {
        return Err((
            install_error_text(&install::InstallError::GameRunning),
            true,
        ));
    }
    let prior = shiv_core::sidecar::read(&paths.addons, &task.mod_id)
        .ok()
        .flatten();
    let was_installed = prior.is_some();
    let from_version = prior.and_then(|s| s.version_label);
    let to_version = complock::updated_date(&task.mod_id).map(str::to_owned);
    let dir = tempfile::Builder::new()
        .prefix("shiv-pack-")
        .tempdir()
        .map_err(|e| fail(format!("Could not create a temporary folder: {e}")))?;
    let mut prepared = prepare_pack_parts(
        deps,
        &paths.addons,
        &task.pak,
        dir.path(),
        cancel,
        &mut report,
    )?;
    let guard = install::InstallGuard::acquire(&paths.addons).map_err(|e| fail(e.to_string()))?;
    if cancel.load(Ordering::Relaxed) {
        return Err((CANCELLED_TEXT.into(), false));
    }
    let remove_after = stage_busy_reused(
        dir.path(),
        std::mem::take(&mut prepared.busy),
        &mut prepared.vpks,
    )
    .map_err(fail)?;
    let meta = ModMeta {
        mod_id: task.mod_id.clone(),
        name: task.name,
        version: Some(complock::revision(&task.pak)),
        version_label: complock::updated_date(&task.mod_id).map(str::to_owned),
        thumbnail_url: None,
        nsfw: false,
        hero: None,
        category: None,
    };
    let mut reused_moves = Vec::new();
    if prepared.vpks.is_empty() {
        claim_free_reused(
            &paths.addons,
            &prepared.free_names,
            &mut prepared.in_place,
            &mut reused_moves,
            guard.permit(),
        )
        .map_err(fail)?;
        if !prepared.in_place.is_empty() {
            complock::adopt_reused(&paths.addons, &meta, &prepared.in_place)
                .map_err(|e| fail(e.to_string()))?;
        }
        drop(guard);
        return Ok(PackDone {
            note: move_note(&reused_moves),
            was_installed,
            from_version,
            to_version,
        });
    }
    report(PackUpdate::Installing);
    let note = install_staged_pack(
        deps,
        &paths,
        &mut prepared,
        &meta,
        &remove_after,
        &mut reused_moves,
        cancel,
        &ask,
        guard.permit(),
    )?;
    Ok(PackDone {
        note,
        was_installed,
        from_version,
        to_version,
    })
}

fn claim_free_reused(
    addons: &Path,
    free_names: &[complock::Part],
    in_place: &mut Vec<String>,
    reused_moves: &mut Vec<String>,
    permit: install::InstallPermit<'_>,
) -> Result<(), String> {
    for part in free_names {
        let reused = complock::claim_reusable(addons, part, permit).map_err(|e| e.to_string())?;
        if let Some(reused) = reused {
            if let Some(from) = reused.moved_from {
                reused_moves.push(format!("{from} to {}", part.name));
            }
            in_place.push(part.name.clone());
        }
    }
    Ok(())
}

/// Worker body: reuse matching paks already on disk or download and unpack
/// missing parts into `dir`.
fn prepare_pack_parts(
    deps: &Deps,
    addons: &Path,
    pak: &complock::Pak,
    dir: &Path,
    cancel: &AtomicBool,
    report: &mut impl FnMut(PackUpdate),
) -> Result<PreparedParts, (String, bool)> {
    let fail = |text: String| (text, false);
    let total = pak.download_size();
    let mut done = 0u64;
    let mut last_step = u64::MAX;
    let mut out = PreparedParts {
        vpks: Vec::new(),
        in_place: Vec::new(),
        free_names: Vec::new(),
        busy: Vec::new(),
    };
    for part in &pak.parts {
        match complock::locate_reusable(addons, part) {
            Ok(Some(reused)) => {
                let already = reused
                    .path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.eq_ignore_ascii_case(&part.name));
                if already {
                    out.in_place.push(part.name.clone());
                } else if reused.moved_from.is_some() {
                    out.free_names.push(part.clone());
                } else {
                    out.busy.push((part.name.clone(), reused));
                }
                continue;
            }
            Ok(None) => {}
            Err(e) => return Err(fail(e.to_string())),
        }
        let mut pieces = Vec::new();
        for (n, url) in part.urls.iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                return Err((CANCELLED_TEXT.into(), false));
            }
            let full = complock::part_url(url).ok_or_else(|| {
                fail(format!(
                    "The pack data's url {url} does not resolve on the site"
                ))
            })?;
            let piece = dir.join(format!("piece-{n}.z"));
            let base = done;
            {
                let mut tick = |bytes: u64, _piece_total: Option<u64>| {
                    done = base + bytes;
                    let step = done.saturating_mul(100).checked_div(total).unwrap_or(0);
                    if step != last_step {
                        last_step = step;
                        report(PackUpdate::Progress {
                            done,
                            total: Some(total),
                        });
                    }
                };
                (deps.pack_download)(&full, &piece, cancel, &mut tick).map_err(fail)?;
            }
            done = base
                + std::fs::metadata(&piece)
                    .map(|m| m.len())
                    .map_err(|e| fail(e.to_string()))?;
            report(PackUpdate::Progress {
                done,
                total: Some(total),
            });
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
        )
        .map_err(fail)?;
        out.vpks.push(dest);
    }
    Ok(out)
}

/// Worker body: copy busy reused paks and their sibling chunks into staging.
fn stage_busy_reused(
    dir: &Path,
    busy: Vec<(String, complock::ReusedPart)>,
    vpks: &mut Vec<PathBuf>,
) -> Result<Vec<PathBuf>, String> {
    let mut remove_after = Vec::new();
    for (name, reused) in busy {
        let staged = dir.join(&name);
        std::fs::copy(&reused.path, &staged).map_err(|e| e.to_string())?;
        vpks.push(staged);
        remove_after.push(reused.path);
        for chunk in reused.chunks {
            let staged_chunk = dir.join(&chunk.name);
            std::fs::copy(&chunk.path, &staged_chunk).map_err(|e| e.to_string())?;
            vpks.push(staged_chunk);
            remove_after.push(chunk.path);
        }
    }
    Ok(remove_after)
}

/// Worker body: place staged pack VPKs into their slots and clean up replaced
/// files.
#[allow(
    clippy::too_many_arguments,
    reason = "installs staged pack VPKs with slot conflict prompt and cleanup"
)]
fn install_staged_pack(
    deps: &Deps,
    paths: &shiv_core::game::GamePaths,
    prepared: &mut PreparedParts,
    meta: &ModMeta,
    remove_after: &[PathBuf],
    reused_moves: &mut Vec<String>,
    cancel: &AtomicBool,
    ask: &dyn Fn(&[install::SlotOccupant]) -> Option<install::SlotChoice>,
    permit: install::InstallPermit<'_>,
) -> Result<String, (String, bool)> {
    let fail = |text: String| (text, false);
    let running = deps.running.clone();
    // Each part goes on the slot its name gives (HUD++ pak01, ...), so the
    // pack's load order does not depend on which download finished first.
    let running_again = running.clone();
    let first = install::install_files_in_slots(
        &prepared.vpks,
        meta.clone(),
        paths,
        move || running(),
        complock::last_slot(),
        None,
        permit,
    );
    let placed = match first {
        Err(install::InstallError::NeedsChoice(occupants)) => {
            let Some(choice) = ask(&occupants) else {
                return Err((CANCELLED_TEXT.into(), false));
            };
            if cancel.load(Ordering::Relaxed) {
                return Err((CANCELLED_TEXT.into(), false));
            }
            install::install_files_in_slots(
                &prepared.vpks,
                meta.clone(),
                paths,
                move || running_again(),
                complock::last_slot(),
                Some(choice),
                permit,
            )
            .map_err(|e| fail(install_error_text(&e)))?
        }
        Err(e) => return Err(fail(install_error_text(&e))),
        Ok(placed) => placed,
    };
    for old in remove_after {
        let _ = std::fs::remove_file(old);
    }
    claim_free_reused(
        &paths.addons,
        &prepared.free_names,
        &mut prepared.in_place,
        reused_moves,
        permit,
    )
    .map_err(fail)?;
    if !prepared.in_place.is_empty() {
        let mut names = std::mem::take(&mut prepared.in_place);
        names.extend(placed.sidecar.vpks.iter().cloned());
        complock::adopt_reused(&paths.addons, meta, &names).map_err(|e| fail(e.to_string()))?;
    }
    let mut note = install::slot_note(&placed);
    let reused = move_note(reused_moves);
    if !reused.is_empty() {
        if !note.is_empty() {
            note.push(' ');
        }
        note.push_str(&reused);
    }
    Ok(note)
}

fn move_note(moves: &[String]) -> String {
    if moves.is_empty() {
        String::new()
    } else {
        format!("Moved: {}.", moves.join(", "))
    }
}

thread_local! {
    static PERF: RefCell<Option<Rc<Perf>>> = const { RefCell::new(None) };
}

fn with_perf(f: impl FnOnce(&Rc<Perf>)) {
    if let Some(p) = PERF.with(|p| p.borrow().clone()) {
        f(&p);
    }
}

/// Stop the pack install behind a Downloads-screen row.
pub(crate) fn cancel_pack(id: &str) {
    with_perf(|p| p.cancel_pack(id));
}

/// Start a cancelled or failed pack install again (Downloads screen).
pub(crate) fn retry_pack(id: &str) {
    with_perf(|p| p.retry_pack(id));
}

/// Re-read gameinfo.gi and installed CompLock paks after a profile switch.
pub(crate) fn reload() {
    with_perf(|p| {
        if p.on_screen.get() && p.catalog.borrow().is_some() {
            p.reload_file();
        }
    });
}

fn truthy(value: &str) -> bool {
    let v = value.trim();
    v.eq_ignore_ascii_case("true") || v.parse::<f64>().is_ok_and(|n| n != 0.0)
}

/// The opposite boolean, in the same spelling (`true`/`false` or `1`/`0`).
pub(crate) fn toggled(value: Option<&str>) -> String {
    match value.map(str::trim) {
        Some(v) if v.parse::<f64>().is_ok() => if truthy(v) { "0" } else { "1" }.into(),
        Some(v) if truthy(v) => "false".into(),
        _ => "true".into(),
    }
}

fn trim_number(text: String) -> String {
    if !text.contains('.') {
        return text;
    }
    let t = text.trim_end_matches('0').trim_end_matches('.');
    if t == "-0" || t.is_empty() {
        "0".into()
    } else {
        t.into()
    }
}

/// A whole f64 from the catalog or the user, shown without a decimal point.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the caller checked the value is whole and inside the setting's range"
)]
fn whole_number_text(v: f64) -> String {
    format!("{}", v as i64)
}

/// `v` as the convar's kind writes it: whole numbers for ints, floats to the
/// step's precision (3 places without a step).
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to 0..=6 decimals before the cast"
)]
pub(crate) fn format_number(v: f64, kind: ConvarKind, step: Option<f64>) -> String {
    if !v.is_finite() {
        return "0".into();
    }
    if kind == ConvarKind::Int {
        return whole_number_text(v.round());
    }
    let decimals = step
        .filter(|s| *s > 0.0 && s.is_finite())
        .map_or(3, |s| (-s.log10().floor()).clamp(0.0, 6.0) as usize);
    trim_number(format!("{v:.decimals$}"))
}

fn finite_range(meta: &ConvarMeta) -> Option<(f64, f64)> {
    match (meta.min, meta.max) {
        (Some(min), Some(max)) if min.is_finite() && max.is_finite() && max > min => {
            Some((min, max))
        }
        _ => None,
    }
}

/// A typed value checked against the convar's kind and range: the value to
/// stage, or why it was refused. Numbers outside the range are clamped.
pub(crate) fn checked_value(meta: Option<&ConvarMeta>, text: &str) -> Result<String, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("Enter a value.".into());
    }
    if !perf::is_writable_value(text) {
        return Err("That value has characters gameinfo.gi can't hold.".into());
    }
    let Some(meta) = meta else {
        return Ok(text.into());
    };
    let number = |whole: bool| -> Result<String, String> {
        let n = text
            .parse::<f64>()
            .ok()
            .filter(|n| n.is_finite())
            .filter(|n| !whole || n.fract() == 0.0)
            .ok_or_else(|| {
                if whole {
                    format!("{} takes a whole number.", meta.name)
                } else {
                    format!("{} takes a number.", meta.name)
                }
            })?;
        let n = finite_range(meta).map_or(n, |(min, max)| n.clamp(min, max));
        Ok(if whole {
            whole_number_text(n)
        } else if n.to_string() == text {
            text.to_string()
        } else {
            trim_number(format!("{n}"))
        })
    };
    match meta.kind {
        ConvarKind::Int => number(true),
        ConvarKind::Float => number(false),
        ConvarKind::Bool => {
            if text.eq_ignore_ascii_case("true")
                || text.eq_ignore_ascii_case("false")
                || text == "0"
                || text == "1"
            {
                Ok(text.into())
            } else {
                Err(format!("{} is on or off: true, false, 1 or 0.", meta.name))
            }
        }
        _ => Ok(text.into()),
    }
}

/// Search rank: exact name, prefix, substring, label, help. `None` when the
/// setting doesn't match.
fn rank(name_lc: &str, meta: Option<&ConvarMeta>, needle: &str) -> Option<u8> {
    if name_lc == needle {
        return Some(0);
    }
    if name_lc.starts_with(needle) {
        return Some(1);
    }
    if name_lc.contains(needle) {
        return Some(2);
    }
    let meta = meta?;
    if meta
        .label
        .as_deref()
        .is_some_and(|l| l.to_lowercase().contains(needle))
    {
        return Some(3);
    }
    meta.help_text()
        .is_some_and(|h| h.to_lowercase().contains(needle))
        .then_some(4)
}

/// `text` as a sentence: capitalised, ending in a full stop.
fn sentence(text: &str) -> String {
    let text = text.trim().trim_end_matches('.');
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => format!("{}{}.", first.to_uppercase(), chars.as_str()),
        None => String::new(),
    }
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or_default().trim()
}

fn entry_path(name: &str) -> Vec<String> {
    vec![CONVARS.into(), name.into()]
}

/// Whether two convar paths name the same setting, ignoring case.
fn paths_equal(a: &[String], b: &[String]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.eq_ignore_ascii_case(y))
}

/// The CompLock card's state: their set as the catalog admits it, with the
/// build, the fetch date, the admitted count and the left-out count beside
/// it. Without a catalog, or with nothing left after admission, the card is
/// unavailable and the subtitle says why.
fn complock_card(catalog: Option<&Catalog>, pack: &complock::PackLoad) -> PerfComplock {
    let by = pack
        .hud()
        .and_then(|h| h.by.as_deref())
        .or_else(|| pack.gameinfo_card().and_then(|c| c.by.as_deref()))
        .unwrap_or("By HoppCX and jcStani");
    let link_url = complock::SITE;
    let link_text = "deadlockconfig.com";
    let unavailable = |subtitle: &str| PerfComplock {
        title: COMPLOCK_TITLE.into(),
        subtitle: subtitle.into(),
        note: COMPLOCK_NOTE.into(),
        by: by.into(),
        link_url: link_url.into(),
        link_text: link_text.into(),
        available: false,
    };
    if pack.too_old() {
        return unavailable(complock::UPDATE_NEEDED);
    }
    let Some(catalog) = catalog else {
        return unavailable("Needs the performance catalog.");
    };
    let admission = catalog.admitted(&pack.entries());
    if admission.entries.is_empty() {
        return unavailable("Nothing Shiv may write from their gameinfo.");
    }
    let left_out = if admission.skipped > 0 {
        format!(" · {} left out (blocked or denied)", admission.skipped)
    } else {
        String::new()
    };
    PerfComplock {
        title: COMPLOCK_TITLE.into(),
        subtitle: format!(
            "deadlockconfig.com · build {} · fetched {} · {} settings{left_out}",
            pack.game(),
            pack.fetched(),
            admission.entries.len()
        )
        .into(),
        note: COMPLOCK_NOTE.into(),
        by: by.into(),
        link_url: link_url.into(),
        link_text: link_text.into(),
        available: true,
    }
}

/// Installed CompLock web-builder paks detected in `addons/`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct BuilderStatus {
    hpbars_custom: Option<bool>,
    hitsound: bool,
}

/// Where the catalog came from.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Origin {
    Api,
    /// The saved copy, because the API failed with this text.
    Saved(String),
}

type CatalogLoad = Result<(Arc<Catalog>, Origin), String>;
/// The file's `ConVars` (`lowercase name -> (name, value)`), whether the
/// game runs, which CompLock pak mods have a sidecar, and detected web-builder
/// paks — or why the file could not be read, with the rest filled as known.
type FileLoad = (
    Result<HashMap<String, (String, String)>, String>,
    bool,
    BTreeMap<String, Option<String>>,
    BuilderStatus,
);

fn load_catalog(deps: &Deps) -> CatalogLoad {
    let cached = deps.cache.as_ref().and_then(CatalogCache::load);
    let etag = cached.as_ref().and_then(|(e, _)| e.clone());
    let fallback = |cached: Option<(Option<String>, Catalog)>, text: String| match cached {
        Some((_, c)) => Ok((Arc::new(c), Origin::Saved(text))),
        None => Err(text),
    };
    match (deps.fetch)(etag.as_deref()) {
        Ok(None) => {
            fallback(cached, "The API sent no catalog.".into()).map(|(c, _)| (c, Origin::Api))
        }
        Ok(Some((etag, body))) => match Catalog::parse(&body) {
            Ok(catalog) => {
                if let Some(cache) = &deps.cache {
                    // A cache that can't be written only costs a download.
                    let _ = cache.save(&etag, &body);
                }
                Ok((Arc::new(catalog), Origin::Api))
            }
            Err(e) => fallback(cached, sentence(&e.to_string())),
        },
        Err(text) => fallback(cached, text),
    }
}

fn convar_name(entry: &ConfigEntry) -> Option<&str> {
    entry.path.get(1).map(String::as_str)
}

fn convars_of(entries: Vec<ConfigEntry>) -> HashMap<String, (String, String)> {
    entries
        .into_iter()
        .filter(|e| e.path.len() == 2 && e.path[0].eq_ignore_ascii_case(CONVARS))
        .filter_map(|e| {
            let name = convar_name(&e)?.to_owned();
            Some((name.to_ascii_lowercase(), (name, e.value?)))
        })
        .collect()
}

fn load_file(deps: &Deps) -> FileLoad {
    let running = (deps.running)();
    let located = (deps.locate)();
    let current = located
        .as_ref()
        .map_err(game_error_text)
        .and_then(|paths| perf::read_current(paths).map_err(|e| sentence(&e.to_string())))
        .map(convars_of);
    let (packs, builders) = located
        .as_ref()
        .map(|paths| {
            let mut packs = BTreeMap::new();
            let mut builders = BuilderStatus::default();
            for m in shiv_core::sidecar::group(&paths.addons) {
                if let Some(sc) = m.sidecar.as_ref()
                    && is_pack_id(&sc.mod_id)
                {
                    packs.insert(sc.mod_id.clone(), sc.version.clone());
                }
                for vpk in &m.vpks {
                    if let Some(found) = complock::detect_builder_pak(&vpk.file, m.sidecar.as_ref())
                    {
                        match found {
                            complock::BuilderPak::HpBars { custom } => {
                                builders.hpbars_custom =
                                    Some(builders.hpbars_custom.unwrap_or(false) || custom);
                            }
                            complock::BuilderPak::Hitsound => builders.hitsound = true,
                        }
                    }
                }
            }
            (packs, builders)
        })
        .unwrap_or_default();
    (current, running, packs, builders)
}

/// `Err((text, game_running))`.
/// What a successful apply returns: the file's `ConVars` as now written, and
/// where the copy from before Shiv's first change is kept.
type Written = (HashMap<String, (String, String)>, std::path::PathBuf);

fn write(deps: &Deps, entries: &[ConfigEntry]) -> Result<Written, (String, bool)> {
    let paths = (deps.locate)().map_err(|e| (game_error_text(&e), false))?;
    if (deps.running)() {
        return Err((GAME_RUNNING_TEXT.into(), true));
    }
    let text = |e: perf::PerfError| (sentence(&e.to_string()), false);
    perf::apply(&paths, entries).map_err(text)?;
    let current = perf::read_current(&paths).map(convars_of).map_err(text)?;
    Ok((current, perf::original_backup_path(&paths)))
}

struct Item {
    name: String,
    lc: String,
    meta: Option<usize>,
}

type Opener = Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>;

struct Perf {
    weak: Weak<App>,
    deps: Deps,
    catalog: RefCell<Option<Arc<Catalog>>>,
    origin: RefCell<Origin>,
    /// `lowercase name -> (name, value)` for the file's `ConVars`.
    current: RefCell<HashMap<String, (String, String)>>,
    file_error: RefCell<Option<String>>,
    items: RefCell<Vec<Item>>,
    /// Indexes into `items`, in list order.
    visible: RefCell<Vec<usize>>,
    rows: Rc<VecModel<PerfRow>>,
    changes: Rc<VecModel<PerfChange>>,
    /// Keys of `changes`, in order.
    change_keys: RefCell<Vec<String>>,
    /// Staged edits by lowercase convar name.
    pending: RefCell<BTreeMap<String, ConfigEntry>>,
    query: RefCell<String>,
    selected: RefCell<Option<String>>,
    note: RefCell<String>,
    loading: Cell<bool>,
    /// The screen is showing; the catalog and list are only kept while it is.
    on_screen: Cell<bool>,
    /// The CompLock options list.
    pack_rows: Rc<VecModel<PerfPackRow>>,
    /// The ids of the rows in `pack_rows`, in order.
    pack_ids: RefCell<Vec<String>>,
    /// Installed CompLock paks: mod id → the revision their sidecar records
    /// (`complock::revision` of the pak installed), `None` when it records
    /// none.
    pack_installed: RefCell<BTreeMap<String, Option<String>>>,
    /// Detected CompLock web-builder paks (HP bars, hitsound).
    builders: Cell<BuilderStatus>,
    /// The choice each option with versions has picked, by option id.
    pack_choices: RefCell<BTreeMap<String, usize>>,
    /// Pack installs by Downloads-screen id.
    pack_jobs: RefCell<HashMap<String, PackJob>>,
    pack_generation: Cell<u64>,
    pack_load: RefCell<complock::PackLoad>,
    pack_at: Cell<Option<std::time::Instant>>,
    pack_busy: Cell<bool>,
    pack_timer: RefCell<Option<slint::Timer>>,
    opener: RefCell<Opener>,
}

#[cfg(test)]
pub(crate) fn set_opener(f: impl Fn(&str) -> Result<(), String> + Send + Sync + 'static) {
    with_perf(|p| *p.opener.borrow_mut() = Arc::new(f));
}

/// A pak row's install button.
#[derive(Clone)]
struct InstallButton {
    label: String,
    installing: bool,
    installed: bool,
    /// Installed, and the same revision the row would install now.
    current: bool,
}

/// A pack install, addressable from the Downloads screen. `generation`
/// tells a late worker answer from a cancelled attempt apart after a retry.
struct PackJob {
    task: PackTask,
    cancel: Arc<AtomicBool>,
    generation: u64,
    /// A worker is running for this row.
    active: bool,
    /// The worker was cancelled and has not answered yet.
    stopping: bool,
}

/// Wire the Performance screen to `app` using the real game and API.
pub fn attach(app: &App, api_url: &str) {
    attach_with(app, Deps::system(api_url));
}

pub(crate) fn attach_with(app: &App, deps: Deps) {
    let controller = Rc::new(Perf {
        weak: app.as_weak(),
        deps,
        catalog: RefCell::new(None),
        origin: RefCell::new(Origin::Api),
        current: RefCell::new(HashMap::new()),
        file_error: RefCell::new(None),
        items: RefCell::new(Vec::new()),
        visible: RefCell::new(Vec::new()),
        rows: Rc::new(VecModel::default()),
        changes: Rc::new(VecModel::default()),
        change_keys: RefCell::new(Vec::new()),
        pending: RefCell::new(BTreeMap::new()),
        query: RefCell::new(String::new()),
        selected: RefCell::new(None),
        note: RefCell::new(String::new()),
        loading: Cell::new(false),
        on_screen: Cell::new(false),
        pack_rows: Rc::new(VecModel::default()),
        pack_ids: RefCell::new(Vec::new()),
        pack_installed: RefCell::new(BTreeMap::new()),
        builders: Cell::new(BuilderStatus::default()),
        pack_choices: RefCell::new(BTreeMap::new()),
        pack_jobs: RefCell::new(HashMap::new()),
        pack_generation: Cell::new(0),
        pack_load: RefCell::new(complock::baked_pack()),
        pack_at: Cell::new(None),
        pack_busy: Cell::new(false),
        pack_timer: RefCell::new(None),
        opener: RefCell::new(Arc::new(|url: &str| {
            shiv_core::link::open_web_link(url).map_err(|e| e.to_string())
        })),
    });
    PERF.with(|p| *p.borrow_mut() = Some(controller.clone()));
    let state = app.global::<PerfState>();
    state.set_hpbar_builder_url(complock::HPBAR_BUILDER_URL.into());
    state.set_hitsound_builder_url(complock::HITSOUND_BUILDER_URL.into());
    state.set_rows(ModelRc::from(controller.rows.clone()));
    state.set_changes(ModelRc::from(controller.changes.clone()));
    state.set_pack_rows(ModelRc::from(controller.pack_rows.clone()));
    state.on_shown(|| with_perf(|p| p.shown()));
    state.on_hidden(|| with_perf(|p| p.hidden()));
    state.on_retry(|| with_perf(|p| p.load()));
    state.on_search(|q| {
        with_perf(|p| {
            *p.query.borrow_mut() = q.to_string();
            p.refresh_list();
        });
    });
    state.on_select(|i| with_perf(|p| p.select_row(i)));
    state.on_pick_tier(|i| with_perf(|p| p.pick_tier(i)));
    state.on_stage_complock(|| with_perf(|p| p.stage_complock()));
    state.on_toggle_pack(|id| with_perf(|p| p.toggle_pack(id.as_str())));
    state.on_pick_pack_choice(|id, i| with_perf(|p| p.pick_pack_choice(id.as_str(), i)));
    state.on_install_pack(|id| with_perf(|p| p.install_pack(id.as_str())));
    state.on_set_fov(|v| with_perf(|p| p.set_fov(f64::from(v))));
    state.on_set_text(|t| with_perf(|p| p.set_text(&t)));
    state.on_set_number(|v| with_perf(|p| p.set_number(f64::from(v))));
    state.on_toggle(|| with_perf(|p| p.toggle()));
    state.on_pick_option(|i| with_perf(|p| p.pick_option(i)));
    state.on_comment_out(|| with_perf(|p| p.edit_selected(|_, _| Ok(None))));
    state.on_revert(|| {
        with_perf(|p| {
            if let Some(key) = p.selected.borrow().clone() {
                p.pending.borrow_mut().remove(&key);
            }
            p.note.borrow_mut().clear();
            p.refresh_after_edit();
        });
    });
    state.on_remove_change(|i| {
        with_perf(|p| {
            let key = usize::try_from(i)
                .ok()
                .and_then(|i| p.change_keys.borrow().get(i).cloned());
            if let Some(key) = key {
                p.pending.borrow_mut().remove(&key);
                p.refresh_after_edit();
            }
        });
    });
    state.on_discard(|| {
        with_perf(|p| {
            if p.busy() {
                return;
            }
            p.pending.borrow_mut().clear();
            p.with_state(|s| {
                s.set_active_tier(-1);
                s.set_complock_staged(false);
            });
            p.refresh_after_edit();
        });
    });
    state.on_apply(|| with_perf(|p| p.apply()));
    state.on_dismiss_result(|| with_perf(|p| p.show_result("", "", false)));
    state.on_open_link(|url| with_perf(|p| p.open_link(url.as_str())));
    controller.publish_tiers();
    controller.publish_complock();
    controller.publish_changes();
}

impl Perf {
    fn with_state(&self, f: impl FnOnce(&PerfState<'_>)) {
        if let Some(app) = self.weak.upgrade() {
            f(&app.global::<PerfState>());
        }
    }

    fn busy(&self) -> bool {
        self.weak
            .upgrade()
            .is_none_or(|a| a.global::<PerfState>().get_busy())
    }

    fn catalog(&self) -> Option<Arc<Catalog>> {
        self.catalog.borrow().clone()
    }

    fn shown(&self) {
        self.on_screen.set(true);
        self.arm_pack_timer();
        self.refresh_pack();
        if self.catalog.borrow().is_none() {
            self.load();
        } else {
            self.reload_file();
        }
    }

    fn open_link(&self, raw: &str) {
        let Some(url) = complock::site_url(raw) else {
            return;
        };
        let opener = self.opener.borrow().clone();
        let weak = self.weak.clone();
        let started = crate::spawn_worker(
            "shiv-perf-link",
            move || opener(&url),
            move |result| {
                if let Err(text) = result.and_then(|r| r) {
                    let _ = weak.upgrade_in_event_loop(move |_| {
                        with_perf(|p| {
                            p.show_result("Could not open link", &text, true);
                        });
                    });
                }
            },
        );
        if !started {
            self.show_result(
                "Could not open link",
                "Could not start a background thread.",
                true,
            );
        }
    }

    /// The screen was left: drop the catalog and the list (several MB) and
    /// keep only the pending edits, the selection and the query. Showing the
    /// screen again reloads the catalog through the cache's `ETag`.
    fn hidden(&self) {
        if !self.on_screen.replace(false) {
            return;
        }
        self.disarm_pack_timer();
        self.drop_catalog();
    }

    /// Load the catalog if the screen was shown again while a read or an
    /// apply that started earlier was still running.
    fn resume(&self) {
        if self.on_screen.get() && self.catalog.borrow().is_none() {
            self.load();
        }
    }

    fn drop_catalog(&self) {
        *self.catalog.borrow_mut() = None;
        *self.items.borrow_mut() = Vec::new();
        *self.visible.borrow_mut() = Vec::new();
        self.rows.set_vec(Vec::new());
        // Emptied in place: `pack-rows` stays bound to this model (set once
        // in `attach_with`). Swapping in a new empty model left the list
        // empty after a nav round trip, since publishing fills this one.
        self.pack_rows.set_vec(Vec::new());
        self.with_state(|s| {
            s.set_tiers(ModelRc::default());
            s.set_complock(PerfComplock::default());
            s.set_editor(PerfEditor::default());
            s.set_load_state(LoadState::Loading);
        });
        // The catalog was parsed on a worker thread, so glibc keeps its pages
        // in that thread's arena after the drop. Trim once the list's row
        // components are gone too (they are released on the next frames).
        slint::Timer::single_shot(
            std::time::Duration::from_secs(1),
            crate::release_freed_memory,
        );
    }

    /// Load the catalog and the file.
    fn load(&self) {
        if self.loading.get() || self.busy() {
            return;
        }
        self.loading.set(true);
        self.with_state(|s| s.set_load_state(LoadState::Loading));
        let deps = self.deps.clone();
        let weak = self.weak.clone();
        let started = crate::spawn_worker(
            "shiv-perf-load",
            move || (load_catalog(&deps), load_file(&deps)),
            move |result| {
                let (catalog, file) = result.unwrap_or_else(|text| {
                    (
                        Err(text),
                        (
                            Err(String::new()),
                            false,
                            BTreeMap::new(),
                            BuilderStatus::default(),
                        ),
                    )
                });
                let _ = weak.upgrade_in_event_loop(move |_| {
                    with_perf(|p| p.loaded(catalog, file));
                });
            },
        );
        if !started {
            self.loaded(
                Err("Could not start a background thread.".into()),
                (
                    Err(String::new()),
                    false,
                    BTreeMap::new(),
                    BuilderStatus::default(),
                ),
            );
        }
    }

    fn loaded(&self, catalog: CatalogLoad, file: FileLoad) {
        self.loading.set(false);
        if !self.on_screen.get() {
            // Left while loading: keep nothing; the next show loads again.
            return;
        }
        match catalog {
            Ok((catalog, origin)) => {
                *self.catalog.borrow_mut() = Some(catalog);
                *self.origin.borrow_mut() = origin;
                self.with_state(|s| s.set_load_state(LoadState::Ready));
                self.publish_tiers();
                self.publish_complock();
                self.file_loaded(file);
            }
            Err(text) => {
                self.with_state(|s| {
                    s.set_error_text(text.into());
                    s.set_load_state(LoadState::Failed);
                });
            }
        }
    }

    /// Re-read gameinfo.gi, keeping the pending changes.
    fn reload_file(&self) {
        if self.loading.get() || self.busy() {
            return;
        }
        self.loading.set(true);
        let deps = self.deps.clone();
        let weak = self.weak.clone();
        let started = crate::spawn_worker(
            "shiv-perf-read",
            move || load_file(&deps),
            move |result| {
                let file = result.unwrap_or_else(|text| {
                    (Err(text), false, BTreeMap::new(), BuilderStatus::default())
                });
                let _ = weak.upgrade_in_event_loop(move |_| {
                    with_perf(|p| {
                        p.loading.set(false);
                        p.file_loaded(file);
                        p.resume();
                    });
                });
            },
        );
        if !started {
            self.loading.set(false);
        }
    }

    fn file_loaded(&self, (current, running, packs, builders): FileLoad) {
        self.builders.set(builders);
        self.with_state(|s| {
            s.set_game_running(running);
            s.set_hitsound_detected(builders.hitsound);
        });
        match current {
            Ok(current) => {
                *self.current.borrow_mut() = current;
                *self.file_error.borrow_mut() = None;
            }
            Err(text) => {
                self.current.borrow_mut().clear();
                *self.file_error.borrow_mut() = Some(text);
            }
        }
        *self.pack_installed.borrow_mut() = packs;
        self.prune_pending();
        self.publish_changes();
        if !self.on_screen.get() {
            return;
        }
        self.rebuild_items();
        self.refresh_list();
        self.publish_editor();
        self.publish_status();
        self.publish_pack_rows();
    }

    fn publish_status(&self) {
        let Some(catalog) = self.catalog() else {
            return;
        };
        let source = match &*self.origin.borrow() {
            Origin::Api => format!("Catalog {}", catalog.version()),
            Origin::Saved(why) => format!(
                "Catalog {} (saved copy: {})",
                catalog.version(),
                why.trim_end_matches('.')
            ),
        };
        let file = match &*self.file_error.borrow() {
            Some(e) => format!("gameinfo.gi could not be read: {e}"),
            None => format!(
                "{} settings set in gameinfo.gi",
                self.current.borrow().len()
            ),
        };
        self.with_state(|s| s.set_status(format!("{source} · {file}").into()));
    }

    fn publish_tiers(&self) {
        let catalog = self.catalog();
        let tiers: Vec<PerfTier> = Tier::ALL
            .iter()
            .map(|&tier| {
                let preset = catalog.as_ref().and_then(|c| c.preset_info(tier));
                let count = catalog.as_ref().map_or(0, |c| c.preset(tier).len());
                PerfTier {
                    label: tier.label().into(),
                    name: preset
                        .map(|p| format!("{} · {count} settings", p.name))
                        .unwrap_or_default()
                        .into(),
                    blurb: preset.map(|p| p.blurb.clone()).unwrap_or_default().into(),
                    available: count > 0,
                }
            })
            .collect();
        self.with_state(|s| s.set_tiers(ModelRc::new(VecModel::from(tiers))));
    }

    fn publish_complock(&self) {
        self.with_state(|s| s.set_complock(complock_card(self.catalog().as_deref(), &self.pack())));
    }

    /// The value a convar of the file or the staged changes reads now:
    /// `None` when nothing sets it.
    fn staged_or_current(&self, key: &str) -> Option<String> {
        if let Some(e) = self.pending.borrow().get(key) {
            return e.value.clone();
        }
        self.current.borrow().get(key).map(|(_, v)| v.clone())
    }

    /// Whether an option is on: every convar it writes reads the value it
    /// writes, staged or already in the file.
    fn pack_option_is_on(&self, entries: &[ConfigEntry]) -> bool {
        entries.iter().all(|e| {
            convar_name(e)
                .and_then(|name| self.staged_or_current(&name.to_ascii_lowercase()))
                .is_some_and(|v| perf::values_equal(&v, e.value.as_deref().unwrap_or_default()))
        }) && !entries.is_empty()
    }

    /// The FOV the slider shows: staged, else the file's, else the site's
    /// default.
    fn fov_shown(&self, fov: &complock::Fov) -> f64 {
        let key = fov.cvar.to_ascii_lowercase();
        self.staged_or_current(&key)
            .or_else(|| Some(fov.default.clone()))
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(fov.from)
            .clamp(fov.from, fov.to)
    }

    /// `v` as the FOV field writes it: the site's step precision.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped to 0.0..=6.0 decimals before cast"
    )]
    fn fov_text(fov: &complock::Fov, v: f64) -> String {
        let decimals = (-fov.by.log10().floor()).clamp(0.0, 6.0) as usize;
        trim_number(format!("{v:.decimals$}"))
    }

    /// The install button of a row with a pak: its label, whether a
    /// download runs, whether the pak is installed, and whether the
    /// installed pak is the one the row would install now (same revision:
    /// the parts' CRCs). Only a different revision is an update; one
    /// installed before Shiv recorded revisions counts as one.
    fn install_label(&self, mod_id: &str, pak: &complock::Pak) -> InstallButton {
        let recorded = self.pack_installed.borrow().get(mod_id).cloned();
        let installed = recorded.is_some();
        let current = recorded.flatten().as_deref() == Some(complock::revision(pak).as_str());
        if let Some((progress, _)) = crate::downloads::transfer(mod_id) {
            let label = if progress < 0.0 {
                "Downloading".to_string()
            } else {
                format!("Downloading {:.0}%", progress * 100.0)
            };
            return InstallButton {
                label,
                installing: true,
                installed,
                current: false,
            };
        }
        let label = if current {
            "Installed".to_string()
        } else if installed {
            "Update".to_string()
        } else {
            format!("Install · {}", fmt_size(Some(pak.download_size())))
        };
        InstallButton {
            label,
            installing: false,
            installed,
            current,
        }
    }

    /// One row of the options list: the switch state from the staged and
    /// current values, the picked choice, and the pak install state.
    fn pack_row(&self, info: PackRowInfo<'_>) -> PerfPackRow {
        let choice_index = self
            .pack_choices
            .borrow()
            .get(info.id)
            .copied()
            .unwrap_or_else(|| {
                info.choices
                    .iter()
                    .position(|c| c.default)
                    .unwrap_or_default()
            });
        let mut button = info.pak.map_or(
            InstallButton {
                label: String::new(),
                installing: false,
                installed: false,
                current: false,
            },
            |pak| self.install_label(&pack_mod_id(info.id), pak),
        );
        let builder_hp = self.builders.get().hpbars_custom;
        let detected_badge = if info.id == "hpbars" {
            let badge = match builder_hp {
                Some(true) => "Custom build",
                Some(false) if !button.installed => "Installed",
                _ => "",
            };
            if !button.installed && !button.installing && builder_hp.is_some() {
                button.installed = true;
                button.current = true;
                button.label = "Installed".into();
            }
            badge
        } else {
            ""
        };
        let link_url = info.link.and_then(complock::site_url).unwrap_or_default();
        let link_text = if link_url.is_empty() {
            ""
        } else {
            info.link_text.unwrap_or(link_url.as_str())
        };
        PerfPackRow {
            kind: info.kind,
            id: info.id.into(),
            title: info.title.into(),
            detail: info.detail.into(),
            text: first_line(info.text.unwrap_or_default()).into(),
            by: info.by.unwrap_or_default().into(),
            link_url: link_url.as_str().into(),
            link_text: link_text.into(),
            detected_badge: detected_badge.into(),
            warn: info.warn.unwrap_or_default().into(),
            has_toggle: !info.convars.is_empty(),
            checked: !info.convars.is_empty() && self.pack_option_is_on(info.convars),
            has_pak: info.pak.is_some(),
            installed: button.installed,
            current: button.current,
            install_label: button.label.into(),
            installing: button.installing,
            choices: ModelRc::new(VecModel::from(
                info.choices
                    .iter()
                    .map(|c| SharedString::from(c.title.as_str()))
                    .collect::<Vec<_>>(),
            )),
            choice_index: i32::try_from(choice_index).unwrap_or(0),
            number: 0.0,
            minimum: 0.0,
            maximum: 1.0,
            step: 0.05,
            value_text: SharedString::new(),
            avg: info.avg.unwrap_or_default().into(),
            low: info.low.unwrap_or_default().into(),
        }
    }

    /// One option row: the switch state from the staged and current values,
    /// the picked choice, and the pak install state.
    fn option_row(&self, option: &complock::PackOption) -> PerfPackRow {
        let choice = self
            .pack_choices
            .borrow()
            .get(&option.id)
            .copied()
            .and_then(|i| option.choices.get(i));
        let (avg, low) = choice.map_or((&option.avg, &option.low), |c| (&c.avg, &c.low));
        self.pack_row(PackRowInfo {
            kind: PerfPackKind::Option,
            id: &option.id,
            title: &option.title,
            detail: String::new(),
            by: option.by.as_deref(),
            link: option.link.as_deref(),
            link_text: option.link_text.as_deref(),
            avg: avg.as_deref(),
            low: low.as_deref(),
            text: choice
                .and_then(|c| c.text.as_deref())
                .or(option.text.as_deref()),
            warn: choice
                .and_then(|c| c.warn.as_deref())
                .or(option.warn.as_deref()),
            convars: choice.map_or(&option.convars, |c| &c.convars),
            choices: &option.choices,
            pak: self.option_pak(option),
        })
    }

    /// The pak an option's Install fetches: the picked choice's, else the
    /// default choice's (an option such as Particle Pruning publishes its
    /// paks only on its choices), else the option's own.
    fn option_pak<'a>(&self, option: &'a complock::PackOption) -> Option<&'a complock::Pak> {
        let index = self
            .pack_choices
            .borrow()
            .get(&option.id)
            .copied()
            .or_else(|| option.choices.iter().position(|c| c.default));
        index
            .and_then(|i| option.choices.get(i))
            .and_then(|c| c.pak.as_ref())
            .or(option.pak.as_ref())
    }

    /// The HUD++ row: its damage-number choices and the pak the picked one
    /// installs.
    fn hud_row(&self, hud: &complock::Hud) -> PerfPackRow {
        let picked = self
            .pack_choices
            .borrow()
            .get(PACK_HUD_ID)
            .copied()
            .unwrap_or_else(|| {
                hud.choices
                    .iter()
                    .position(|c| c.default)
                    .unwrap_or_default()
            });
        self.pack_row(PackRowInfo {
            kind: PerfPackKind::Hud,
            id: PACK_HUD_ID,
            title: &hud.title,
            detail: String::new(),
            by: hud.by.as_deref(),
            link: hud.link.as_deref(),
            link_text: hud.link_text.as_deref(),
            avg: hud.avg.as_deref(),
            low: hud.low.as_deref(),
            text: hud.text.as_deref(),
            warn: None,
            convars: &[],
            choices: &hud.choices,
            pak: hud.pak_of_choice(picked),
        })
    }

    /// A group heading in the options list: its title and blurb.
    fn heading_row(&self, title: &str, blurb: &str) -> PerfPackRow {
        self.pack_row(PackRowInfo {
            kind: PerfPackKind::Heading,
            id: "",
            title,
            detail: blurb.to_string(),
            by: None,
            link: None,
            link_text: None,
            avg: None,
            low: None,
            text: None,
            warn: None,
            convars: &[],
            choices: &[],
            pak: None,
        })
    }

    /// The FOV row: the slider over the site's range at the value the file
    /// or the staged changes hold.
    #[allow(
        clippy::cast_possible_truncation,
        reason = "Slint numeric slider properties are f32"
    )]
    fn fov_row(&self, fov: &complock::Fov) -> PerfPackRow {
        let shown = self.fov_shown(fov);
        let mut row = self.pack_row(PackRowInfo {
            kind: PerfPackKind::Fov,
            id: PACK_FOV_ID,
            title: "Field of view",
            detail: format!("{} · default {}", fov.cvar, fov.default),
            by: None,
            link: None,
            link_text: None,
            avg: None,
            low: None,
            text: None,
            warn: None,
            convars: &[],
            choices: &[],
            pak: None,
        });
        row.number = shown as f32;
        row.minimum = fov.from as f32;
        row.maximum = fov.to as f32;
        row.step = fov.by as f32;
        row.value_text = Self::fov_text(fov, shown).into();
        row
    }

    fn pack(&self) -> complock::PackLoad {
        self.pack_load.borrow().clone()
    }

    fn arm_pack_timer(&self) {
        let mut slot = self.pack_timer.borrow_mut();
        if slot.is_some() {
            return;
        }
        let timer = slint::Timer::default();
        timer.start(slint::TimerMode::Repeated, complock::PACK_REFRESH, || {
            with_perf(|p| {
                if p.on_screen.get() {
                    p.refresh_pack();
                }
            });
        });
        *slot = Some(timer);
    }

    fn disarm_pack_timer(&self) {
        if let Some(timer) = self.pack_timer.borrow_mut().take() {
            timer.stop();
        }
    }

    fn refresh_pack(&self) {
        if !self.on_screen.get() || self.pack_busy.get() {
            return;
        }
        if !complock::pack_refresh_due(self.pack_at.get(), std::time::Instant::now()) {
            return;
        }
        self.pack_busy.set(true);
        let fetch = self.deps.pack_page.clone();
        let cache = self.deps.pack_cache.clone();
        let weak = self.weak.clone();
        let started = crate::spawn_worker(
            "shiv-pack-data",
            move || complock::load_pack(|url| fetch(url), cache.as_deref()),
            move |result| {
                let _ = weak.upgrade_in_event_loop(move |_| {
                    with_perf(|p| p.pack_loaded(result));
                });
            },
        );
        if !started {
            self.pack_busy.set(false);
        }
    }

    fn pack_loaded(&self, result: Result<complock::PackLoad, String>) {
        self.pack_busy.set(false);
        self.pack_at.set(Some(std::time::Instant::now()));
        if let Ok(load) = result {
            *self.pack_load.borrow_mut() = load;
        }
        if self.on_screen.get() {
            self.publish_complock();
            self.publish_pack_rows();
        }
    }

    /// The CompLock options list: the HUD++ base, the FOV slider, then the
    /// options under their group headings, all in the site's order.
    fn publish_pack_rows(&self) {
        let pack = self.pack();
        let mut rows = Vec::new();
        let mut ids = Vec::new();
        if pack.hud().is_some() || pack.fov().is_some() {
            rows.push(self.heading_row("HUD and view", ""));
            ids.push(String::new());
        }
        if let Some(hud) = pack.hud() {
            rows.push(self.hud_row(hud));
            ids.push(PACK_HUD_ID.to_string());
        }
        if let Some(fov) = pack.fov() {
            rows.push(self.fov_row(fov));
            ids.push(PACK_FOV_ID.to_string());
        }
        for group in pack.groups() {
            let options: Vec<&complock::PackOption> = pack
                .options()
                .iter()
                .filter(|o| o.group.as_deref() == Some(group.id.as_str()))
                .collect();
            if options.is_empty() {
                continue;
            }
            rows.push(self.heading_row(&group.title, &group.text));
            ids.push(String::new());
            for option in options {
                rows.push(self.option_row(option));
                ids.push(option.id.clone());
            }
        }
        *self.pack_ids.borrow_mut() = ids;
        self.pack_rows.set_vec(rows);
    }

    /// Re-draw the pack row of `id` in place, from the same state its
    /// builder reads.
    fn refresh_pack_row(&self, id: &str) {
        let pack = self.pack();
        let rebuilt = if id == PACK_HUD_ID {
            pack.hud().map(|hud| self.hud_row(hud))
        } else if id == PACK_FOV_ID {
            pack.fov().map(|fov| self.fov_row(fov))
        } else {
            pack.options()
                .iter()
                .find(|o| o.id == id)
                .map(|option| self.option_row(option))
        };
        let Some(row) = rebuilt else {
            return;
        };
        let index = self
            .pack_ids
            .borrow()
            .iter()
            .position(|i| i == id)
            .filter(|&i| i < self.pack_rows.row_count());
        if let Some(index) = index {
            self.pack_rows.set_row_data(index, row);
        }
    }

    /// Every catalog convar the game still has, plus any `ConVars` key the file
    /// sets that the catalog doesn't know.
    fn rebuild_items(&self) {
        let Some(catalog) = self.catalog() else {
            return;
        };
        let current = self.current.borrow();
        let mut items: Vec<Item> = catalog
            .convars()
            .iter()
            .enumerate()
            .filter_map(|(i, meta)| {
                let lc = meta.name.to_ascii_lowercase();
                let gone = matches!(meta.status, ConvarStatus::Removed | ConvarStatus::NotConvar);
                (!gone || current.contains_key(&lc)).then(|| Item {
                    name: meta.name.clone(),
                    lc,
                    meta: Some(i),
                })
            })
            .collect();
        for (lc, (name, _)) in current.iter() {
            if catalog.convar(lc).is_none() {
                items.push(Item {
                    name: name.clone(),
                    lc: lc.clone(),
                    meta: None,
                });
            }
        }
        items.sort_by(|a, b| a.lc.cmp(&b.lc));
        *self.items.borrow_mut() = items;
    }

    fn meta(&self, item: &Item) -> Option<ConvarMeta> {
        let catalog = self.catalog()?;
        item.meta.and_then(|i| catalog.convars().get(i).cloned())
    }

    /// Filter and order the list for the query: without one, settings the
    /// file sets come first.
    fn refresh_list(&self) {
        let needle = self.query.borrow().trim().to_lowercase();
        let catalog = self.catalog();
        let items = self.items.borrow();
        let current = self.current.borrow();
        let mut ranked: Vec<(u8, usize)> = items
            .iter()
            .enumerate()
            .filter_map(|(i, item)| {
                if needle.is_empty() {
                    let set = current.contains_key(&item.lc)
                        || self.pending.borrow().contains_key(&item.lc);
                    return Some((u8::from(!set), i));
                }
                let meta = item
                    .meta
                    .and_then(|m| catalog.as_ref().and_then(|c| c.convars().get(m)));
                rank(&item.lc, meta, &needle).map(|r| (r, i))
            })
            .collect();
        ranked.sort_by_key(|&(r, i)| (r, i));
        let visible: Vec<usize> = ranked.into_iter().map(|(_, i)| i).collect();
        let note = if needle.is_empty() {
            format!(
                "{} settings · the ones gameinfo.gi sets come first",
                visible.len()
            )
        } else if visible.is_empty() {
            "No settings match".into()
        } else {
            format!("{} matching settings", visible.len())
        };
        let rows: Vec<PerfRow> = visible.iter().map(|&i| self.row(&items[i])).collect();
        drop((items, current));
        *self.visible.borrow_mut() = visible;
        self.rows.set_vec(rows);
        self.with_state(|s| s.set_list_note(note.into()));
    }

    fn row(&self, item: &Item) -> PerfRow {
        let catalog = self.catalog();
        let meta = item
            .meta
            .and_then(|m| catalog.as_ref().and_then(|c| c.convars().get(m)));
        let detail = meta
            .and_then(|m| m.label.as_deref().or(m.help_text()))
            .map(first_line)
            .unwrap_or_default();
        let current = self
            .current
            .borrow()
            .get(&item.lc)
            .map(|(_, v)| v.clone())
            .unwrap_or_default();
        let pending = self.pending.borrow().get(&item.lc).map(|e| match &e.value {
            Some(v) => v.clone(),
            None => "off".into(),
        });
        PerfRow {
            name: item.name.as_str().into(),
            detail: detail.into(),
            current: current.into(),
            pending: pending.unwrap_or_default().into(),
            selected: self.selected.borrow().as_deref() == Some(item.lc.as_str()),
        }
    }

    /// Re-draw the visible rows for these keys only.
    fn refresh_rows(&self, keys: &[&str]) {
        let items = self.items.borrow();
        for (row, &i) in self.visible.borrow().iter().enumerate() {
            if keys.contains(&items[i].lc.as_str()) {
                self.rows.set_row_data(row, self.row(&items[i]));
            }
        }
    }

    fn select_row(&self, row: i32) {
        let key = usize::try_from(row)
            .ok()
            .and_then(|r| self.visible.borrow().get(r).copied())
            .map(|i| self.items.borrow()[i].lc.clone());
        let Some(key) = key else {
            return;
        };
        let previous = self.selected.replace(Some(key.clone()));
        self.note.borrow_mut().clear();
        let mut keys = vec![key.as_str()];
        if let Some(p) = previous.as_deref() {
            keys.push(p);
        }
        self.refresh_rows(&keys);
        self.publish_editor();
    }

    fn selected_item(&self) -> Option<(String, String, Option<ConvarMeta>)> {
        let key = self.selected.borrow().clone()?;
        let items = self.items.borrow();
        let item = items.iter().find(|i| i.lc == key)?;
        Some((key, item.name.clone(), self.meta(item)))
    }

    /// The value the editor shows: pending, else the file's, else the default.
    fn shown_value(&self, key: &str, meta: Option<&ConvarMeta>) -> Option<String> {
        if let Some(e) = self.pending.borrow().get(key) {
            return e.value.clone();
        }
        self.current
            .borrow()
            .get(key)
            .map(|(_, v)| v.clone())
            .or_else(|| meta.and_then(|m| m.default.clone()))
    }

    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        clippy::cast_precision_loss,
        reason = "Slint's editor model carries f32 fields and an i32 option index"
    )]
    fn publish_editor(&self) {
        let Some((key, name, meta)) = self.selected_item() else {
            self.with_state(|s| s.set_editor(PerfEditor::default()));
            return;
        };
        let catalog = self.catalog();
        let current = self.current.borrow().get(&key).map(|(_, v)| v.clone());
        let value = self.shown_value(&key, meta.as_ref());
        let pending = self.pending.borrow().get(&key).cloned();
        let denied = catalog
            .as_ref()
            .and_then(|c| c.writable(&entry_path(&name)).err());

        let range = meta.as_ref().and_then(finite_range);
        let text = editor_text(meta.as_ref(), catalog.as_deref(), range);
        let kind = meta.as_ref().map(|m| m.kind);
        let options: Vec<&perf::EnumValue> = meta
            .as_ref()
            .map(|m| m.enum_values.iter().collect())
            .unwrap_or_default();
        let control = control_for(denied.is_some(), kind, &options, range);
        let (minimum, maximum) = range.unwrap_or((0.0, 1.0));
        let step = meta
            .as_ref()
            .and_then(|m| m.step)
            .filter(|s| *s > 0.0)
            .unwrap_or(if kind == Some(ConvarKind::Int) {
                1.0
            } else {
                (maximum - minimum) / 100.0
            });
        let shown = value.clone().unwrap_or_default();
        let number = shown
            .parse::<f64>()
            .unwrap_or(minimum)
            .clamp(minimum, maximum);
        let option_index = options
            .iter()
            .position(|o| o.value == shown)
            .unwrap_or_default();
        let note = denied.map_or_else(
            || self.note.borrow().clone(),
            |reason| format!("Shiv doesn't change this setting: {reason}"),
        );
        let editor = PerfEditor {
            open: true,
            name: name.into(),
            meta: text.meta_line.into(),
            help: text.help.into(),
            limits: text.limits.into(),
            current: match &current {
                Some(v) => format!("In gameinfo.gi: {v}"),
                None => "Not set in gameinfo.gi".into(),
            }
            .into(),
            control,
            value: if pending.as_ref().is_some_and(|p| p.value.is_none()) {
                SharedString::new()
            } else {
                shown.as_str().into()
            },
            checked: value.as_deref().is_some_and(truthy),
            number: number as f32,
            minimum: minimum as f32,
            maximum: maximum as f32,
            step: step as f32,
            options: ModelRc::new(VecModel::from(
                options
                    .iter()
                    .map(|o| SharedString::from(o.label.as_str()))
                    .collect::<Vec<_>>(),
            )),
            option_index: option_index as i32,
            has_pending: pending.is_some(),
            is_set: current.is_some() && pending.as_ref().is_none_or(|p| p.value.is_some()),
            note: note.into(),
        };
        self.with_state(|s| s.set_editor(editor));
    }

    /// Stage `edit(meta, shown value)` for the selected setting. `Err` shows
    /// as the editor's note and stages nothing.
    fn edit_selected(
        &self,
        edit: impl FnOnce(Option<&ConvarMeta>, Option<String>) -> Result<Option<String>, String>,
    ) {
        if self.busy() {
            return;
        }
        let Some((key, name, meta)) = self.selected_item() else {
            return;
        };
        let denied = self
            .catalog()
            .and_then(|c| c.writable(&entry_path(&name)).err());
        if denied.is_some() {
            return;
        }
        let shown = self.shown_value(&key, meta.as_ref());
        match edit(meta.as_ref(), shown) {
            Ok(value) => {
                self.note.borrow_mut().clear();
                self.stage(&key, &name, value);
            }
            Err(text) => *self.note.borrow_mut() = text,
        }
        self.refresh_after_edit();
    }

    /// Stage a value, or drop the change when the file already has it.
    fn stage(&self, key: &str, name: &str, value: Option<String>) {
        let unchanged = {
            let current = self.current.borrow();
            match (&value, current.get(key)) {
                (Some(v), Some((_, c))) => perf::values_equal(v, c),
                (None, None) => true,
                _ => false,
            }
        };
        let mut pending = self.pending.borrow_mut();
        if unchanged {
            pending.remove(key);
        } else {
            pending.insert(
                key.to_string(),
                ConfigEntry {
                    path: entry_path(name),
                    value,
                },
            );
        }
    }

    /// Drop pending changes the file now already has.
    fn prune_pending(&self) {
        let staged: Vec<(String, ConfigEntry)> = self
            .pending
            .borrow()
            .iter()
            .map(|(k, e)| (k.clone(), e.clone()))
            .collect();
        for (key, entry) in staged {
            if let Some(name) = entry.path.get(1) {
                self.stage(&key, name, entry.value);
            }
        }
    }

    fn refresh_after_edit(&self) {
        let selected = self.selected.borrow().clone();
        let keys: Vec<String> = self
            .change_keys
            .borrow()
            .iter()
            .cloned()
            .chain(self.pending.borrow().keys().cloned())
            .chain(selected)
            .collect();
        let keys: Vec<&str> = keys.iter().map(String::as_str).collect();
        self.refresh_rows(&keys);
        self.publish_changes();
        self.publish_editor();
        self.publish_pack_rows();
    }

    fn publish_changes(&self) {
        let current = self.current.borrow();
        let pending = self.pending.borrow();
        let rows: Vec<PerfChange> = pending
            .iter()
            .map(|(key, e)| PerfChange {
                name: convar_name(e).unwrap_or(key.as_str()).into(),
                before: current
                    .get(key)
                    .map_or_else(|| "not set".to_string(), |(_, v)| v.clone())
                    .into(),
                after: e
                    .value
                    .clone()
                    .unwrap_or_else(|| "commented out".into())
                    .into(),
            })
            .collect();
        let label = match pending.len() {
            0 => "Apply".to_string(),
            1 => "Apply 1 change".to_string(),
            n => format!("Apply {n} changes"),
        };
        *self.change_keys.borrow_mut() = pending.keys().cloned().collect();
        drop((current, pending));
        self.changes.set_vec(rows);
        self.with_state(|s| s.set_apply_label(label.into()));
    }

    fn pick_tier(&self, index: i32) {
        if self.busy() {
            return;
        }
        let Some(tier) = usize::try_from(index).ok().and_then(|i| Tier::ALL.get(i)) else {
            return;
        };
        let Some(catalog) = self.catalog() else {
            return;
        };
        let entries = catalog.preset(*tier);
        if entries.is_empty() {
            return;
        }
        self.pending.borrow_mut().clear();
        for e in entries {
            if let Some(name) = e.path.get(1) {
                self.stage(&name.to_ascii_lowercase(), name, e.value);
            }
        }
        self.note.borrow_mut().clear();
        let blurb = catalog
            .preset_info(*tier)
            .map(|p| format!("{} by {}: {}", p.name, p.author, p.blurb))
            .unwrap_or_default();
        self.with_state(|s| {
            s.set_active_tier(index);
            s.set_complock_staged(false);
            s.set_result_title(format!("{} preset staged", tier.label()).into());
            s.set_result_detail(format!("{blurb} Review the pending changes, then apply.").into());
            s.set_result_error(false);
        });
        self.refresh_list();
        self.publish_changes();
        self.publish_editor();
    }

    /// Stage the CompLock set the way a tier stages its preset: it replaces
    /// whatever was staged, and the result says what admission left out.
    fn stage_complock(&self) {
        if self.busy() {
            return;
        }
        let Some(catalog) = self.catalog() else {
            return;
        };
        let pack = self.pack();
        if pack.too_old() {
            self.show_result("CompLock", complock::UPDATE_NEEDED, true);
            return;
        }
        let admission = catalog.admitted(&pack.entries());
        if admission.entries.is_empty() {
            return;
        }
        self.pending.borrow_mut().clear();
        for e in admission.entries {
            if let Some(name) = e.path.get(1) {
                self.stage(&name.to_ascii_lowercase(), name, e.value);
            }
        }
        self.note.borrow_mut().clear();
        let mut detail = format!(
            "Engine settings from deadlockconfig.com for game build {}. Review the pending changes, then apply.",
            pack.game()
        );
        if admission.skipped > 0 {
            let _ = write!(
                detail,
                " {} of their settings are left out: blocked by the game or refused by the catalog's rules.",
                admission.skipped
            );
        }
        self.with_state(|s| {
            s.set_active_tier(-1);
            s.set_complock_staged(true);
            s.set_result_title("CompLock set staged".into());
            s.set_result_detail(detail.into());
            s.set_result_error(false);
        });
        self.refresh_list();
        self.publish_changes();
        self.publish_editor();
    }

    /// Stage an option's convar side: on stages the values it writes (as
    /// the catalog admits them), off stages its off values — or comments
    /// the lines out when the site states none, exactly as its pack builder
    /// leaves the line out. The switch state itself comes from the staged
    /// and current values, so this is a toggle in the honest sense.
    fn toggle_pack(&self, id: &str) {
        if self.busy() {
            return;
        }
        let pack = self.pack();
        let Some(option) = pack.options().iter().find(|o| o.id == id) else {
            return;
        };
        let on = self.pack_option_is_on(&option.convars);
        let staged: Vec<ConfigEntry> = if on && option.off_convars.is_empty() {
            option
                .convars
                .iter()
                .map(|e| ConfigEntry {
                    path: e.path.clone(),
                    value: None,
                })
                .collect()
        } else if on {
            option.off_convars.clone()
        } else {
            option.convars.clone()
        };
        let Some(catalog) = self.catalog() else {
            return;
        };
        let admission = catalog.admitted(&staged);
        for entry in admission.entries {
            if let Some(name) = entry.path.get(1) {
                self.stage(&name.to_ascii_lowercase(), name, entry.value);
            }
        }
        self.note.borrow_mut().clear();
        let mut detail = format!("{} {}.", option.title, if on { "off" } else { "on" });
        if admission.skipped > 0 {
            let _ = write!(
                detail,
                " {} of its settings are left out: blocked by the game or refused by the catalog's rules.",
                admission.skipped
            );
        }
        self.with_state(|s| {
            s.set_result_title("CompLock option staged".into());
            s.set_result_detail(detail.into());
            s.set_result_error(false);
        });
        self.refresh_after_edit();
    }

    /// Pick a choice: records it, stages the convar lines that choice
    /// writes (commenting out the ones its off values name), and re-draws
    /// the row so Install uses the picked pak.
    fn pick_pack_choice(&self, id: &str, index: i32) {
        if self.busy() {
            return;
        }
        let pack = self.pack();
        let picked = usize::try_from(index).ok().filter(|&i| {
            pack.options()
                .iter()
                .find(|o| o.id == id)
                .is_some_and(|o| i < o.choices.len())
                || pack
                    .hud()
                    .is_some_and(|hud| id == PACK_HUD_ID && i < hud.choices.len())
        });
        let Some(picked) = picked else {
            return;
        };
        self.pack_choices.borrow_mut().insert(id.to_owned(), picked);
        let choice = pack
            .options()
            .iter()
            .find(|o| o.id == id)
            .and_then(|o| o.choices.get(picked))
            .or_else(|| {
                pack.hud()
                    .filter(|_| id == PACK_HUD_ID)
                    .and_then(|hud| hud.choices.get(picked))
            });
        if let Some(choice) = choice {
            let mut staged: Vec<ConfigEntry> = choice.convars.clone();
            for off in &choice.off_convars {
                let already = staged.iter().any(|e| paths_equal(&e.path, &off.path));
                if !already {
                    staged.push(ConfigEntry {
                        path: off.path.clone(),
                        value: None,
                    });
                }
            }
            if let Some(catalog) = self.catalog() {
                for entry in catalog.admitted(&staged).entries {
                    if let Some(name) = entry.path.get(1) {
                        self.stage(&name.to_ascii_lowercase(), name, entry.value);
                    }
                }
            }
        }
        self.note.borrow_mut().clear();
        self.refresh_after_edit();
    }

    /// The FOV slider: stages `r_aspectratio` at the site's step precision.
    fn set_fov(&self, v: f64) {
        if self.busy() {
            return;
        }
        let pack = self.pack();
        let Some(fov) = pack.fov() else {
            return;
        };
        let key = fov.cvar.to_ascii_lowercase();
        let value = Self::fov_text(fov, v.clamp(fov.from, fov.to));
        self.stage(&key, &fov.cvar, Some(value));
        self.note.borrow_mut().clear();
        self.refresh_rows(&[key.as_str()]);
        self.publish_changes();
        self.publish_editor();
        self.refresh_pack_row(PACK_FOV_ID);
    }

    /// Install (or update) a pak: the option's own pak, or the picked
    /// choice's. The download shows on the Downloads screen under the
    /// option's stable mod id; a re-install replaces the mod in place.
    fn install_pack(&self, id: &str) {
        let pack = self.pack();
        if pack.too_old() {
            self.show_result("CompLock", complock::UPDATE_NEEDED, true);
            return;
        }
        let task = if id == PACK_HUD_ID {
            pack.hud().and_then(|hud| {
                let picked = self
                    .pack_choices
                    .borrow()
                    .get(PACK_HUD_ID)
                    .copied()
                    .unwrap_or_else(|| {
                        hud.choices
                            .iter()
                            .position(|c| c.default)
                            .unwrap_or_default()
                    });
                hud.pak_of_choice(picked).map(|pak| {
                    PackTask::of(pack_mod_id(id), format!("CompLock {}", hud.title), pak)
                })
            })
        } else {
            pack.options()
                .iter()
                .find(|o| o.id == id)
                .and_then(|option| {
                    let found_pak = self.option_pak(option)?;
                    Some(PackTask::of(
                        pack_mod_id(id),
                        format!("CompLock {}", option.title),
                        found_pak,
                    ))
                })
        };
        let Some(task) = task else {
            return;
        };
        self.spawn_pack_install(task);
        self.refresh_pack_row(id);
    }

    /// What a pack-install worker reported, on the event loop. A late answer
    /// from a cancelled attempt still clears the job's bookkeeping.
    fn pack_update(&self, id: &str, generation: u64, update: PackUpdate) {
        let current = self
            .pack_jobs
            .borrow()
            .get(id)
            .is_some_and(|j| j.generation == generation);
        match update {
            PackUpdate::Progress { done, total } => {
                if current {
                    crate::downloads::progress(id, done, total);
                    self.refresh_pack_row(Self::row_id_of_mod(id));
                }
            }
            PackUpdate::Installing => {
                if current {
                    crate::downloads::installing(id);
                    self.refresh_pack_row(Self::row_id_of_mod(id));
                }
            }
            PackUpdate::Done(result) => {
                let (name, revision) = self.pack_jobs.borrow().get(id).map_or_else(
                    || (id.to_owned(), None),
                    |j| (j.task.name.clone(), Some(complock::revision(&j.task.pak))),
                );
                if current {
                    match &result {
                        Ok(_) => {
                            self.pack_jobs.borrow_mut().remove(id);
                        }
                        Err(_) => {
                            if let Some(job) = self.pack_jobs.borrow_mut().get_mut(id) {
                                job.active = false;
                                job.stopping = false;
                            }
                        }
                    }
                }
                match result {
                    Ok(done) => {
                        if current {
                            let kind = Some(if done.was_installed {
                                shiv_core::activity::ActivityKind::Updated
                            } else {
                                shiv_core::activity::ActivityKind::Installed
                            });
                            crate::downloads::finished_with(
                                id,
                                crate::downloads::Outcome::Done,
                                kind,
                                done.from_version.as_deref(),
                                done.to_version.as_deref(),
                                Some("CompLock"),
                            );
                        }
                        self.pack_installed
                            .borrow_mut()
                            .insert(id.to_owned(), revision);
                        crate::installed::reload();
                        crate::launch::refresh();
                        if !done.note.is_empty() {
                            self.show_result(&name, &done.note, false);
                        }
                    }
                    Err((text, running)) => {
                        if text != CANCELLED_TEXT {
                            self.show_result(&format!("Couldn't install {name}"), &text, true);
                        }
                        if current {
                            let outcome = if text == CANCELLED_TEXT {
                                crate::downloads::Outcome::Cancelled
                            } else {
                                crate::downloads::Outcome::Failed(text)
                            };
                            crate::downloads::finished(id, outcome);
                        }
                        if running {
                            self.with_state(|s| s.set_game_running(true));
                        }
                    }
                }
                if current {
                    self.refresh_pack_row(Self::row_id_of_mod(id));
                }
            }
        }
    }

    /// The options-list row id behind a mod id (`complock-soundcap` →
    /// `soundcap`).
    fn row_id_of_mod(mod_id: &str) -> &str {
        mod_id.strip_prefix(PACK_PREFIX).unwrap_or(mod_id)
    }

    /// Stop the pack install behind a Downloads-screen row.
    pub(crate) fn cancel_pack(&self, id: &str) {
        let mut jobs = self.pack_jobs.borrow_mut();
        let Some(job) = jobs.get_mut(id).filter(|j| j.active) else {
            return;
        };
        job.cancel.store(true, Ordering::Relaxed);
        job.stopping = true;
        crate::downloads::finished(id, crate::downloads::Outcome::Cancelled);
    }

    /// Start a cancelled or failed pack install again (Downloads screen):
    /// the same pak the attempt downloaded, not whatever is picked now.
    pub(crate) fn retry_pack(&self, id: &str) {
        let task = {
            let mut jobs = self.pack_jobs.borrow_mut();
            let busy = jobs.get(id).is_some_and(|j| j.stopping || j.active);
            if busy {
                return;
            }
            jobs.remove(id).map(|job| job.task)
        };
        let row = Self::row_id_of_mod(id).to_string();
        if let Some(task) = task {
            self.spawn_pack_install(task);
            self.refresh_pack_row(&row);
        } else {
            self.install_pack(&row);
        }
    }

    /// The worker behind one pack install: downloads with progress on the
    /// Downloads screen, then the install under the shared lock.
    fn spawn_pack_install(&self, task: PackTask) {
        let mut jobs = self.pack_jobs.borrow_mut();
        if jobs
            .get(&task.mod_id)
            .is_some_and(|job| job.active || job.stopping)
        {
            return;
        }
        let generation = self.pack_generation.get() + 1;
        self.pack_generation.set(generation);
        let cancel = Arc::new(AtomicBool::new(false));
        jobs.insert(
            task.mod_id.clone(),
            PackJob {
                task: task.clone(),
                cancel: cancel.clone(),
                generation,
                active: true,
                stopping: false,
            },
        );
        drop(jobs);
        crate::downloads::started(&task.mod_id, &task.name);
        let deps = self.deps.clone();
        let weak = self.weak.clone();
        let ask_weak = weak.clone();
        let mod_id = task.mod_id.clone();
        let failed_id = task.mod_id.clone();
        let worker_task = task;
        let poster = move |update: PackUpdate| {
            let id = mod_id.clone();
            let _ = weak.upgrade_in_event_loop(move |_| {
                with_perf(|p| p.pack_update(&id, generation, update));
            });
        };
        let post = poster.clone();
        let started = crate::spawn_worker(
            "shiv-pack-install",
            move || {
                run_pack_install(&deps, worker_task, &cancel, post, |occupants| {
                    crate::conflict::ask_slots(&ask_weak, occupants)
                })
            },
            move |result| {
                poster(PackUpdate::Done(
                    result.unwrap_or_else(|text| Err((text, false))),
                ));
            },
        );
        if !started {
            let text = "Could not start a background thread.".to_string();
            self.pack_update(&failed_id, generation, PackUpdate::Done(Err((text, false))));
        }
    }

    fn set_text(&self, text: &str) {
        self.edit_selected(|meta, _| checked_value(meta, text).map(Some));
    }

    fn set_number(&self, v: f64) {
        self.edit_selected(|meta, _| {
            let meta = meta.ok_or_else(String::new)?;
            let v = finite_range(meta).map_or(v, |(min, max)| v.clamp(min, max));
            Ok(Some(format_number(v, meta.kind, meta.step)))
        });
    }

    fn toggle(&self) {
        self.edit_selected(|_, shown| Ok(Some(toggled(shown.as_deref()))));
    }

    fn pick_option(&self, index: i32) {
        self.edit_selected(|meta, _| {
            let option = meta
                .zip(usize::try_from(index).ok())
                .and_then(|(m, i)| m.enum_values.get(i))
                .ok_or_else(String::new)?;
            Ok(Some(option.value.clone()))
        });
    }

    fn apply(&self) {
        if self.busy() || self.loading.get() {
            return;
        }
        let entries: Vec<ConfigEntry> = self.pending.borrow().values().cloned().collect();
        if entries.is_empty() {
            return;
        }
        self.with_state(|s| s.set_busy(true));
        self.show_result("", "", false);
        let deps = self.deps.clone();
        let weak = self.weak.clone();
        let count = entries.len();
        let started = crate::spawn_worker(
            "shiv-perf-apply",
            move || write(&deps, &entries),
            move |result| {
                let result = result.unwrap_or_else(|text| Err((text, false)));
                let _ = weak.upgrade_in_event_loop(move |_| {
                    with_perf(|p| p.applied(count, result));
                });
            },
        );
        if !started {
            self.applied(
                count,
                Err(("Could not start a background thread.".into(), false)),
            );
        }
    }

    fn applied(&self, count: usize, result: Result<Written, (String, bool)>) {
        self.with_state(|s| s.set_busy(false));
        match result {
            Ok((current, original)) => {
                self.pending.borrow_mut().clear();
                self.with_state(|s| {
                    s.set_active_tier(-1);
                    s.set_complock_staged(false);
                    s.set_game_running(false);
                });
                let packs = self.pack_installed.borrow().clone();
                let builders = self.builders.get();
                self.file_loaded((Ok(current), false, packs, builders));
                crate::launch::refresh();
                let what = if count == 1 {
                    "1 change".to_string()
                } else {
                    format!("{count} changes")
                };
                self.show_result(
                    "Settings applied",
                    &format!(
                        "Wrote {what} to gameinfo.gi and kept the previous file as gameinfo.gi.bak. \
                         Deadlock uses them the next time it starts. A game update replaces \
                         gameinfo.gi, so apply again after one; that also replaces the copy \
                         from before Shiv's change.\nFile from before Shiv's change: {}",
                        original.display()
                    ),
                    false,
                );
                crate::downloads::record(
                    shiv_core::activity::ActivityKind::SettingsApplied,
                    "Performance settings",
                    &format!("Applied {what} to gameinfo.gi"),
                    None,
                    None,
                    Some("Performance"),
                );
            }
            Err((text, running)) => {
                if running {
                    self.with_state(|s| s.set_game_running(true));
                }
                self.show_result("Couldn't apply the settings", &text, true);
            }
        }
        self.resume();
    }

    fn show_result(&self, title: &str, detail: &str, error: bool) {
        self.with_state(|s| {
            s.set_result_title(title.into());
            s.set_result_detail(detail.into());
            s.set_result_error(error);
        });
    }
}

fn kind_label(kind: ConvarKind) -> &'static str {
    match kind {
        ConvarKind::Bool => "on or off",
        ConvarKind::Int => "whole number",
        ConvarKind::Float => "number",
        ConvarKind::String => "text",
        ConvarKind::Enum => "choice",
        ConvarKind::Color => "colour",
        ConvarKind::Vector => "vector",
    }
}

/// The editor's descriptive text: the meta line under the name, the help
/// body, and the limits line.
struct EditorText {
    meta_line: String,
    help: String,
    limits: String,
}

/// The editor's descriptive text for `meta`: category, kind and status on
/// the meta line, the help text with the side effects appended, the range
/// and default as the limits.
fn editor_text(
    meta: Option<&ConvarMeta>,
    catalog: Option<&Catalog>,
    range: Option<(f64, f64)>,
) -> EditorText {
    let mut meta_line = match meta {
        Some(m) => format!(
            "{} · {}",
            catalog.map_or(m.category.as_str(), |c| c.category_label(&m.category)),
            kind_label(m.kind)
        ),
        None => "Not in the catalog".into(),
    };
    match meta.map(|m| m.status) {
        Some(ConvarStatus::Blocked) => meta_line.push_str(" · blocked by the game"),
        Some(ConvarStatus::Removed) => meta_line.push_str(" · not in the current game build"),
        Some(ConvarStatus::NotConvar) => meta_line.push_str(" · a console command"),
        _ => {}
    }
    let mut help = meta
        .and_then(|m| m.help_text())
        .unwrap_or_default()
        .to_string();
    if let Some(side) = meta.and_then(|m| m.side_effects.as_deref()) {
        if !help.is_empty() {
            help.push_str("\n\n");
        }
        help.push_str(side);
    }
    let mut limits = Vec::new();
    if let (Some(m), Some((min, max))) = (meta, range) {
        limits.push(format!(
            "Range {} to {}",
            format_number(min, m.kind, m.step),
            format_number(max, m.kind, m.step)
        ));
    }
    if let Some(d) = meta.and_then(|m| m.default.as_deref()) {
        limits.push(format!("default {d}"));
    }
    EditorText {
        meta_line,
        help,
        limits: limits.join(" · "),
    }
}

/// The widget the editor shows for a setting: locked when the catalog
/// denies the write, else the kind's natural control.
fn control_for(
    denied: bool,
    kind: Option<ConvarKind>,
    options: &[&perf::EnumValue],
    range: Option<(f64, f64)>,
) -> PerfControl {
    if denied {
        PerfControl::Locked
    } else if kind == Some(ConvarKind::Bool) {
        PerfControl::Toggle
    } else if kind == Some(ConvarKind::Enum) && !options.is_empty() {
        PerfControl::Choice
    } else if matches!(kind, Some(ConvarKind::Int | ConvarKind::Float)) && range.is_some() {
        PerfControl::Slider
    } else {
        PerfControl::Text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(kind: ConvarKind, min: Option<f64>, max: Option<f64>) -> ConvarMeta {
        ConvarMeta {
            name: "x".into(),
            kind,
            default: None,
            min,
            max,
            step: None,
            enum_values: Vec::new(),
            help: None,
            label: None,
            description: None,
            category: "other".into(),
            side_effects: None,
            status: ConvarStatus::Active,
        }
    }

    fn deps(fetch: Fetch, cache: CatalogCache) -> Deps {
        Deps {
            locate: Arc::new(|| Err(shiv_core::game::GameError::NotFound)),
            running: Arc::new(|| false),
            fetch,
            cache: Some(cache),
            pack_download: Arc::new(|_, _, _, _| Err("no pack downloads in unit tests".into())),
            pack_page: pack_page_offline(),
            pack_cache: None,
        }
    }

    /// Diagnosis: the real pack install (real client, real site) into a
    /// temp copy of an addons folder (`SHIV_ADDONS_COPY`).
    #[test]
    #[ignore = "network; needs SHIV_ADDONS_COPY"]
    fn real_pack_install_into_a_copy() {
        let Ok(copy) = std::env::var("SHIV_ADDONS_COPY") else {
            return;
        };
        let temp = tempfile::tempdir().expect("tempdir");
        let paths = shiv_core::game::GamePaths::from_root(temp.path().join("Deadlock"));
        std::fs::create_dir_all(&paths.addons).expect("addons");
        let ok = std::process::Command::new("cp")
            .arg("-a")
            .arg(format!("{copy}/."))
            .arg(&paths.addons)
            .status()
            .expect("cp")
            .success();
        assert!(ok);
        let mut deps = Deps::system(crate::DEFAULT_API_URL);
        let located = paths.clone();
        deps.locate = Arc::new(move || Ok(located.clone()));
        deps.running = Arc::new(|| false);
        let ids: Vec<String> = std::iter::once(PACK_HUD_ID.to_owned())
            .chain(
                complock::options()
                    .iter()
                    .filter(|o| o.pak.is_some())
                    .map(|o| o.id.clone()),
            )
            .collect();
        for id in ids {
            let pak = complock::release_for(&pack_mod_id(&id), None).expect("published");
            let task = PackTask::of(pack_mod_id(&id), format!("CompLock {id}"), pak);
            let r = run_pack_install(
                &deps,
                task,
                &AtomicBool::new(false),
                |_| {},
                |_| Some(install::SlotChoice::Move),
            );
            eprintln!("{id}: {r:?}");
        }
        for m in shiv_core::sidecar::group(&paths.addons) {
            eprintln!(
                "{:?} {:?}",
                m.sidecar.map(|s| s.mod_id),
                m.vpks
                    .iter()
                    .map(|v| v.file.file_name().unwrap().to_owned())
                    .collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn catalog_uses_the_etag_and_falls_back_to_the_saved_copy() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cache = CatalogCache::new(dir.path().to_path_buf());
        let body = crate::tests::PERF_CATALOG.as_bytes().to_vec();
        // Nothing saved and the API down: an error.
        let down: Fetch = Arc::new(|_| Err("The mod service could not be reached.".into()));
        assert!(load_catalog(&deps(down.clone(), cache.clone())).is_err());
        // A download is saved with its ETag.
        let sent = body.clone();
        let fresh: Fetch = Arc::new(move |etag| {
            assert_eq!(etag, None);
            Ok(Some(("\"e1\"".into(), sent.clone())))
        });
        let (catalog, origin) = load_catalog(&deps(fresh, cache.clone())).expect("fresh");
        assert_eq!(origin, Origin::Api);
        assert_eq!(catalog.version(), "2026.11.07-1");
        // Not modified: the saved copy, asked for by its ETag.
        let same: Fetch = Arc::new(|etag| {
            assert_eq!(etag, Some("\"e1\""));
            Ok(None)
        });
        assert_eq!(
            load_catalog(&deps(same, cache.clone())).expect("304").1,
            Origin::Api
        );
        // API down, or a catalog from a newer schema: the saved copy, said so.
        let (_, origin) = load_catalog(&deps(down, cache.clone())).expect("offline");
        assert!(matches!(origin, Origin::Saved(ref t) if t.contains("could not be reached")));
        let newer = String::from_utf8(body)
            .expect("utf8")
            .replace("\"schemaVersion\":1", "\"schemaVersion\":2");
        let newer: Fetch =
            Arc::new(move |_| Ok(Some(("\"e2\"".into(), newer.clone().into_bytes()))));
        let (catalog, origin) = load_catalog(&deps(newer, cache.clone())).expect("newer schema");
        assert!(
            matches!(origin, Origin::Saved(ref t) if t.contains("schema version 2")),
            "{origin:?}"
        );
        assert_eq!(catalog.version(), "2026.11.07-1");
        assert_eq!(cache.load().expect("kept").0.as_deref(), Some("\"e1\""));
    }

    #[test]
    fn toggling_keeps_the_spelling() {
        assert_eq!(toggled(Some("true")), "false");
        assert_eq!(toggled(Some("FALSE")), "true");
        assert_eq!(toggled(Some("1")), "0");
        assert_eq!(toggled(Some("0")), "1");
        assert_eq!(toggled(None), "true");
    }

    #[test]
    fn numbers_follow_kind_and_step() {
        assert_eq!(format_number(3.6, ConvarKind::Int, None), "4");
        assert_eq!(format_number(0.300_000_01, ConvarKind::Float, None), "0.3");
        assert_eq!(format_number(0.26, ConvarKind::Float, Some(0.5)), "0.3");
        assert_eq!(format_number(12.0, ConvarKind::Float, Some(1.0)), "12");
        assert_eq!(format_number(-0.0001, ConvarKind::Float, None), "0");
    }

    #[test]
    fn control_for_locks_denied_settings_and_maps_kinds_to_widgets() {
        let option = perf::EnumValue {
            value: "0".into(),
            label: "Off".into(),
        };
        assert_eq!(
            control_for(true, Some(ConvarKind::Bool), &[&option], Some((0.0, 1.0))),
            PerfControl::Locked,
            "a denied write locks the widget whatever the kind"
        );
        assert_eq!(
            control_for(false, Some(ConvarKind::Bool), &[], None),
            PerfControl::Toggle
        );
        assert_eq!(
            control_for(false, Some(ConvarKind::Enum), &[&option], None),
            PerfControl::Choice
        );
        assert_eq!(
            control_for(false, Some(ConvarKind::Enum), &[], None),
            PerfControl::Text,
            "an enum without options is free text"
        );
        assert_eq!(
            control_for(false, Some(ConvarKind::Int), &[], Some((0.0, 100.0))),
            PerfControl::Slider
        );
        assert_eq!(
            control_for(false, Some(ConvarKind::Float), &[], None),
            PerfControl::Text,
            "a number without a range is free text"
        );
        assert_eq!(control_for(false, None, &[], None), PerfControl::Text);
    }

    #[test]
    fn typed_values_are_checked_against_kind_and_range() {
        let int = meta(ConvarKind::Int, Some(0.0), Some(1000.0));
        assert_eq!(checked_value(Some(&int), " 240 ").unwrap(), "240");
        assert_eq!(checked_value(Some(&int), "5000").unwrap(), "1000");
        assert!(checked_value(Some(&int), "1.5").is_err());
        assert!(checked_value(Some(&int), "fast").is_err());
        let float = meta(ConvarKind::Float, None, None);
        assert_eq!(checked_value(Some(&float), "0.50").unwrap(), "0.5");
        assert_eq!(checked_value(Some(&float), "1e-3").unwrap(), "0.001");
        let text = meta(ConvarKind::String, None, None);
        assert_eq!(checked_value(Some(&text), "a b").unwrap(), "a b");
        assert!(checked_value(Some(&text), "a\"b").is_err());
        assert!(checked_value(None, "").is_err());
        assert!(checked_value(Some(&meta(ConvarKind::Bool, None, None)), "yes").is_err());
    }

    #[test]
    fn the_complock_card_states_the_admitted_and_left_out_counts() {
        let catalog = Catalog::parse(crate::tests::PERF_CATALOG.as_bytes()).expect("catalog");
        let admission = catalog.admitted(&complock::entries());
        assert!(
            admission.skipped > 0,
            "the fixture denies some of their set"
        );
        let card = complock_card(Some(&catalog), &complock::baked_pack());
        assert!(card.available, "{}", card.subtitle);
        assert_eq!(card.title.as_str(), "CompLock engine settings");
        assert!(
            card.subtitle
                .contains(&format!("{} settings", admission.entries.len())),
            "{}",
            card.subtitle
        );
        assert!(
            card.subtitle
                .contains(&format!("{} left out", admission.skipped)),
            "{}",
            card.subtitle
        );
        assert!(card.note.contains("hero models"), "{}", card.note);
        let bare = complock_card(None, &complock::baked_pack());
        assert!(!bare.available);
        assert!(bare.subtitle.contains("catalog"), "{}", bare.subtitle);
    }

    /// A pack pak already on disk under another number is not renamed until
    /// the install is going ahead. Cancelling, or a download that fails,
    /// leaves that folder byte for byte as it was.
    #[test]
    fn a_cancelled_or_failed_pack_download_leaves_addons_unchanged() {
        let root = tempfile::tempdir().expect("tempdir");
        let paths = shiv_core::game::GamePaths::from_root(root.path().join("Deadlock"));
        std::fs::create_dir_all(&paths.addons).expect("addons");
        let bytes = b"browser install of a pack pak";
        std::fs::write(paths.addons.join("pak12_dir.vpk"), bytes).expect("pak");
        std::fs::write(paths.addons.join("pak12_000.vpk"), b"chunk").expect("chunk");
        let before = dir_bytes(&paths.addons);
        let pak = complock::Pak {
            title: "HUD++".into(),
            parts: vec![
                complock::Part {
                    name: "pak01_dir.vpk".into(),
                    size: bytes.len() as u64,
                    crc: complock::crc32(bytes),
                    csize: bytes.len() as u64,
                    urls: vec!["/dl/pack/hud.z".into()],
                },
                complock::Part {
                    name: "pak02_dir.vpk".into(),
                    size: 4,
                    crc: 1,
                    csize: 4,
                    urls: vec!["/dl/pack/other.z".into()],
                },
            ],
        };
        let task = PackTask::of("complock-hud".into(), "HUD++".into(), &pak);
        let located = paths.clone();
        let mut deps = deps(
            Arc::new(|_| Err("unused".into())),
            CatalogCache::new(root.path().join("cache")),
        );
        deps.locate = Arc::new(move || Ok(located.clone()));
        deps.running = Arc::new(|| false);
        deps.pack_download = Arc::new(|_, _, _, _| Err("the download failed".into()));
        let failed = run_pack_install(
            &deps,
            task.clone(),
            &AtomicBool::new(false),
            |_| {},
            |_| Some(install::SlotChoice::Move),
        );
        assert!(failed.is_err(), "{failed:?}");
        assert_eq!(
            dir_bytes(&paths.addons),
            before,
            "a failed download renamed a pak"
        );
        let cancelled = run_pack_install(
            &deps,
            task,
            &AtomicBool::new(true),
            |_| {},
            |_| Some(install::SlotChoice::Move),
        );
        assert!(
            matches!(cancelled, Err((ref text, _)) if text == "Cancelled"),
            "{cancelled:?}"
        );
        assert_eq!(dir_bytes(&paths.addons), before, "cancel renamed a pak");
        assert!(paths.addons.join("pak12_dir.vpk").is_file());
        assert!(paths.addons.join("pak12_000.vpk").is_file());
        assert!(!paths.addons.join("pak01_dir.vpk").exists());
    }

    fn dir_bytes(dir: &std::path::Path) -> BTreeMap<String, Vec<u8>> {
        let mut out = BTreeMap::new();
        let entries = std::fs::read_dir(dir).expect("read");
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                out.insert(
                    entry.file_name().to_string_lossy().into_owned(),
                    std::fs::read(&path).expect("bytes"),
                );
            }
        }
        out
    }

    #[test]
    fn the_card_says_shiv_update_needed_when_app_min_is_newer() {
        let json = r#"{"files":{},"base":[],"option":[],"gameinfo":"GameInfo { ConVars { fps_max \"1\" } }","app_min":2}"#;
        let html = format!(
            "<html><script id=\"pack-data\" type=\"application/json\">{json}</script></html>"
        );
        let pack = complock::load_pack(|_| Ok(html), None);
        let catalog = Catalog::parse(crate::tests::PERF_CATALOG.as_bytes()).expect("catalog");
        let card = complock_card(Some(&catalog), &pack);
        assert!(pack.too_old());
        assert!(!card.available);
        assert_eq!(card.subtitle.as_str(), "shiv update needed");
    }

    #[test]
    fn search_ranks_names_before_help() {
        let mut m = meta(ConvarKind::Bool, None, None);
        m.help = Some("Ambient occlusion".into());
        assert_eq!(rank("r_ssao", Some(&m), "r_ssao"), Some(0));
        assert_eq!(rank("r_ssao", Some(&m), "r_s"), Some(1));
        assert_eq!(rank("r_ssao", Some(&m), "ssao"), Some(2));
        assert_eq!(rank("r_ssao", Some(&m), "occlusion"), Some(4));
        assert_eq!(rank("r_ssao", Some(&m), "shadow"), None);
    }
}
