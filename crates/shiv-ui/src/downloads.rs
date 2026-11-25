//! Activity screen: active transfers and persistent history of Shiv's
//! completed actions (issue #65).
//!
//! Browse, Installed, Performance, and Repair report completed or in-flight
//! actions here. Active downloads tick O(1) in place without rebuilding the
//! model or touching disk; ended items and action records persist atomically to
//! `<settings_dir>/activity.json` on a background worker.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use shiv_core::activity::{self, ActivityEntry, ActivityKind};
use slint::{ComponentHandle, Model, ModelRc, VecModel, Weak};

use crate::browse::fmt_mb;
use crate::{App, DownloadPhase, DownloadsState, TransferRow};

/// Completed entries kept in the list and on disk; older ones drop off.
const HISTORY: usize = activity::MAX_ENTRIES;
/// Speed is averaged over at least this long, so it does not flicker.
const SPEED_WINDOW: Duration = Duration::from_millis(750);

static NEXT_ACTIVITY_ID: AtomicU64 = AtomicU64::new(1);

#[cfg(test)]
thread_local! {
    static TEST_DIR: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

/// Redirects `activity.json` persistence in unit and UI integration tests.
#[cfg(test)]
pub(crate) fn set_dir_for_test(dir: Option<PathBuf>) {
    TEST_DIR.with(|c| *c.borrow_mut() = dir);
}

fn persistence_path(allow_default: bool) -> Option<PathBuf> {
    #[cfg(test)]
    {
        let _ = allow_default;
        TEST_DIR.with(|c| c.borrow().as_ref().map(|d| d.join("activity.json")))
    }
    #[cfg(not(test))]
    {
        allow_default.then(activity::default_path).flatten()
    }
}

/// Worker body: load persisted Activity entries from disk.
fn load_activity(path: &Path) -> Vec<ActivityEntry> {
    activity::load_from(path)
}

/// Worker body: save completed Activity entries atomically to disk.
fn save_activity(path: &Path, entries: &[ActivityEntry], lock: &Mutex<u64>, generation: u64) {
    let mut last = lock
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if generation < *last {
        return;
    }
    let _ = activity::save_to(path, entries);
    *last = generation;
}

/// Where the Activity rows go. `replace_all` swaps the whole list — a model
/// reset, every row re-read — and is for membership changes only; `update_row`
/// changes one row in place, the single `row_changed` a progress tick must be.
pub(crate) trait TransferRows {
    /// Replace the whole list: rows were added or dropped.
    fn replace_all(&self, rows: Vec<TransferRow>);
    /// Update one row in place (its transfer moved on).
    fn update_row(&self, index: usize, row: TransferRow);
    /// The model the Activity screen reads.
    fn model(&self) -> ModelRc<TransferRow>;
}

/// The rows in the Slint [`VecModel`] the Activity screen reads.
struct VecModelRows {
    model: Rc<VecModel<TransferRow>>,
}

impl TransferRows for VecModelRows {
    fn replace_all(&self, rows: Vec<TransferRow>) {
        self.model.set_vec(rows);
    }

    fn update_row(&self, index: usize, row: TransferRow) {
        self.model.set_row_data(index, row);
    }

    fn model(&self) -> ModelRc<TransferRow> {
        ModelRc::from(self.model.clone())
    }
}

#[derive(Clone)]
struct Item {
    id: String,
    name: String,
    phase: DownloadPhase,
    progress: f32,
    done: u64,
    total: Option<u64>,
    speed: f64,
    sample: (Instant, u64),
    detail: String,
    kind: ActivityKind,
    timestamp: u64,
    from_version: Option<String>,
    to_version: Option<String>,
    source: Option<String>,
    retryable: bool,
}

struct Downloads {
    weak: Weak<App>,
    rows: Rc<dyn TransferRows>,
    items: RefCell<Vec<Item>>,
    persist_path: Option<PathBuf>,
    save_gen: Cell<u64>,
    save_lock: Arc<Mutex<u64>>,
    cleared: Cell<bool>,
}

thread_local! {
    static DOWNLOADS: RefCell<Option<Rc<Downloads>>> = const { RefCell::new(None) };
}

fn with_downloads(f: impl FnOnce(&Rc<Downloads>)) {
    if let Some(d) = DOWNLOADS.with(|c| c.borrow().clone()) {
        f(&d);
    }
}

pub fn attach(app: &App) {
    let rows: Rc<dyn TransferRows> = Rc::new(VecModelRows {
        model: Rc::new(VecModel::default()),
    });
    attach_with_path(app, &rows, persistence_path(true));
}

/// Wire the Activity screen to `app`, writing its rows through `rows`
/// (the storm test passes a counting sink; `attach` passes the Slint model).
#[cfg(test)]
pub(crate) fn attach_with(app: &App, rows: &Rc<dyn TransferRows>) {
    attach_with_path(app, rows, persistence_path(false));
}

fn attach_with_path(app: &App, rows: &Rc<dyn TransferRows>, persist_path: Option<PathBuf>) {
    let model = rows.model();
    let controller = Rc::new(Downloads {
        weak: app.as_weak(),
        rows: rows.clone(),
        items: RefCell::new(Vec::new()),
        persist_path: persist_path.clone(),
        save_gen: Cell::new(0),
        save_lock: Arc::new(Mutex::new(0)),
        cleared: Cell::new(false),
    });
    let state = app.global::<DownloadsState>();
    state.set_rows(model);
    state.set_active_count(0);
    DOWNLOADS.with(|c| *c.borrow_mut() = Some(controller));
    state.on_cancel(|id| {
        let id = id.as_str();
        // Link installs belong to the Installed screen, CompLock paks to the
        // Performance screen, catalog ones to Browse.
        if id.starts_with("link-") {
            crate::installed::cancel_link(id);
        } else if crate::performance::is_pack_id(id) {
            crate::performance::cancel_pack(id);
        } else {
            crate::browse::cancel_install(id);
        }
    });
    state.on_retry(|id| {
        let id = id.as_str();
        if id.starts_with("link-") {
            crate::installed::retry_link(id);
        } else if crate::performance::is_pack_id(id) {
            crate::performance::retry_pack(id);
        } else {
            crate::browse::retry_install(id);
        }
    });
    state.on_clear_finished(|| with_downloads(|d| d.clear_finished()));
    if let Some(path) = persist_path {
        let weak = app.as_weak();
        let _ = crate::spawn_worker(
            "shiv-activity-load",
            move || load_activity(&path),
            move |result| {
                if let Ok(entries) = result {
                    let _ = weak.upgrade_in_event_loop(move |_| {
                        with_downloads(|d| d.loaded_history(entries));
                    });
                }
            },
        );
    }
}

/// An install's download began.
pub(crate) fn started(id: &str, name: &str) {
    with_downloads(|d| d.started(id, name));
}

/// Bytes received so far (`total` when the size is known).
pub(crate) fn progress(id: &str, done: u64, total: Option<u64>) {
    with_downloads(|d| d.progress(id, done, total, Instant::now()));
}

/// An install that never reached the download (no file list, no files).
pub(crate) fn failed_to_start(id: &str, name: &str, reason: &str) {
    started(id, name);
    finished(id, Outcome::Failed(reason.to_owned()));
}

pub(crate) fn installing(id: &str) {
    with_downloads(|d| d.set_installing(id));
}

pub(crate) fn finished(id: &str, outcome: Outcome) {
    finished_with(id, outcome, None, None, None, None);
}

pub(crate) fn finished_with(
    id: &str,
    outcome: Outcome,
    kind: Option<ActivityKind>,
    from_version: Option<&str>,
    to_version: Option<&str>,
    source: Option<&str>,
) {
    with_downloads(|d| {
        let (phase, resolved_kind, detail) = match outcome {
            Outcome::Done => {
                let resolved = kind.unwrap_or(ActivityKind::Installed);
                let verb = done_verb(resolved);
                (
                    DownloadPhase::Done,
                    resolved,
                    format_activity_detail(verb, from_version, to_version, source),
                )
            }
            Outcome::DoneUnverified => {
                let resolved = kind.unwrap_or(ActivityKind::Installed);
                let verb = format!("{}, not verified", done_verb(resolved));
                (
                    DownloadPhase::Done,
                    resolved,
                    format_activity_detail(&verb, from_version, to_version, source),
                )
            }
            Outcome::Cancelled => (
                DownloadPhase::Cancelled,
                ActivityKind::Cancelled,
                "Cancelled".into(),
            ),
            Outcome::Failed(text) => (
                DownloadPhase::Failed,
                ActivityKind::Failed,
                format_activity_detail(&text, None, None, source),
            ),
        };
        d.finish_item(
            id,
            phase,
            resolved_kind,
            detail,
            from_version.map(str::to_owned),
            to_version.map(str::to_owned),
            source.map(str::to_owned),
        );
    });
}

/// Record a non-transfer action directly into the persistent Activity log.
pub(crate) fn record(
    kind: ActivityKind,
    title: &str,
    detail: &str,
    from_version: Option<&str>,
    to_version: Option<&str>,
    source: Option<&str>,
) {
    with_downloads(|d| {
        d.record_entry(kind, title, detail, from_version, to_version, source);
    });
}

/// A running transfer of `id`: its progress (negative when the size is
/// unknown) and whether it has reached the install step. `None` otherwise.
pub(crate) fn transfer(id: &str) -> Option<(f32, bool)> {
    let mut out = None;
    with_downloads(|d| {
        out = d
            .items
            .borrow()
            .iter()
            .find(|i| i.id == id)
            .and_then(|i| match i.phase {
                DownloadPhase::Downloading => Some((i.progress, false)),
                DownloadPhase::Installing => Some((1.0, true)),
                _ => None,
            });
    });
    out
}

/// Outcome of the most recent finished transfer for `id`, or `None` when
/// active or absent.
pub(crate) fn last_outcome_ok(id: &str) -> Option<bool> {
    let mut out = None;
    with_downloads(|d| {
        out = d
            .items
            .borrow()
            .iter()
            .find(|i| i.id == id)
            .and_then(|i| match i.phase {
                DownloadPhase::Done => Some(true),
                DownloadPhase::Failed | DownloadPhase::Cancelled => Some(false),
                DownloadPhase::Downloading | DownloadPhase::Installing => None,
            });
    });
    out
}

pub(crate) enum Outcome {
    Done,
    /// Installed, and the download published no checksum.
    DoneUnverified,
    Cancelled,
    Failed(String),
}

fn done_verb(kind: ActivityKind) -> &'static str {
    match kind {
        ActivityKind::Updated => "Updated",
        ActivityKind::Reset => "Reset",
        ActivityKind::Restored => "Restored",
        ActivityKind::Replaced => "Replaced",
        _ => "Installed",
    }
}

pub(crate) fn format_activity_detail(
    detail: &str,
    from: Option<&str>,
    to: Option<&str>,
    source: Option<&str>,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(src) = source.map(str::trim).filter(|s| !s.is_empty()) {
        parts.push(src.to_owned());
    }
    let version = match (
        from.map(str::trim).filter(|s| !s.is_empty()),
        to.map(str::trim).filter(|s| !s.is_empty()),
    ) {
        (Some(f), Some(t)) if f != t => Some(format!("{f} \u{2192} {t}")),
        (_, Some(t)) => Some(t.to_owned()),
        (Some(f), None) => Some(f.to_owned()),
        (None, None) => None,
    };
    if let Some(v) = version {
        parts.push(v);
    }
    let d = detail.trim();
    if !d.is_empty() {
        parts.push(d.to_owned());
    }
    parts.join(" \u{b7} ")
}

fn speed_text(bytes_per_sec: f64) -> String {
    format!("{:.1} MB/s", bytes_per_sec / 1_000_000.0)
}

/// "12.3 MB of 52.0 MB \u{b7} 4.2 MB/s", or just what is known.
fn progress_text(done: u64, total: Option<u64>, speed: f64) -> String {
    let mut text = match total {
        Some(t) => format!("{} of {}", fmt_mb(done), fmt_mb(t)),
        None => fmt_mb(done),
    };
    if speed > 0.0 {
        text.push_str(" \u{b7} ");
        text.push_str(&speed_text(speed));
    }
    text
}

fn is_active(phase: DownloadPhase) -> bool {
    matches!(
        phase,
        DownloadPhase::Downloading | DownloadPhase::Installing
    )
}

fn transfer_row_of(i: &Item) -> TransferRow {
    let (kind_label, when) = if is_active(i.phase) {
        (String::new(), String::new())
    } else {
        (
            i.kind.label().to_owned(),
            activity::format_relative(i.timestamp, activity::now_secs()),
        )
    };
    TransferRow {
        id: i.id.as_str().into(),
        name: i.name.as_str().into(),
        phase: i.phase,
        progress: i.progress,
        detail: i.detail.as_str().into(),
        kind_label: kind_label.into(),
        when: when.into(),
        retryable: i.retryable,
    }
}

fn trim_history(items: &mut Vec<Item>) -> bool {
    let before = items.len();
    let mut ended_seen = 0;
    items.retain(|i| {
        if is_active(i.phase) {
            true
        } else {
            ended_seen += 1;
            ended_seen <= HISTORY
        }
    });
    items.len() != before
}

/// What a phase change did to the list.
enum PhaseOutcome {
    /// The row changed in place.
    Row(usize),
    /// The history bound dropped rows: the whole list changed.
    List,
}

impl Downloads {
    fn loaded_history(&self, entries: Vec<ActivityEntry>) {
        if self.cleared.get() {
            return;
        }
        {
            let mut items = self.items.borrow_mut();
            let mut existing: HashSet<String> = items.iter().map(|i| i.id.clone()).collect();
            for entry in entries {
                if !existing.insert(entry.id.clone()) {
                    continue;
                }
                let phase = match entry.kind {
                    ActivityKind::Failed => DownloadPhase::Failed,
                    ActivityKind::Cancelled => DownloadPhase::Cancelled,
                    _ => DownloadPhase::Done,
                };
                items.push(Item {
                    id: entry.id,
                    name: entry.title,
                    phase,
                    progress: if phase == DownloadPhase::Done {
                        1.0
                    } else {
                        0.0
                    },
                    done: 0,
                    total: None,
                    speed: 0.0,
                    sample: (Instant::now(), 0),
                    detail: entry.detail,
                    kind: entry.kind,
                    timestamp: entry.timestamp,
                    from_version: entry.from_version,
                    to_version: entry.to_version,
                    source: entry.source,
                    retryable: false,
                });
            }
            trim_history(&mut items);
        }
        self.rows.replace_all(self.transfer_rows());
        self.set_active_count();
    }

    fn save_history(&self) {
        let Some(path) = self.persist_path.clone() else {
            return;
        };
        let generation = self.save_gen.get().wrapping_add(1);
        self.save_gen.set(generation);
        let lock = Arc::clone(&self.save_lock);
        let snapshot: Vec<ActivityEntry> = self
            .items
            .borrow()
            .iter()
            .filter(|i| !is_active(i.phase))
            .take(HISTORY)
            .map(|i| ActivityEntry {
                id: i.id.clone(),
                timestamp: i.timestamp,
                kind: i.kind,
                title: i.name.clone(),
                detail: i.detail.clone(),
                from_version: i.from_version.clone(),
                to_version: i.to_version.clone(),
                source: i.source.clone(),
            })
            .collect();
        let _ = crate::spawn_worker(
            "shiv-activity-save",
            move || save_activity(&path, &snapshot, &lock, generation),
            |_| {},
        );
    }

    fn started(&self, id: &str, name: &str) {
        {
            let mut items = self.items.borrow_mut();
            items.retain(|i| i.id != id);
            items.insert(
                0,
                Item {
                    id: id.to_owned(),
                    name: name.to_owned(),
                    phase: DownloadPhase::Downloading,
                    progress: -1.0,
                    done: 0,
                    total: None,
                    speed: 0.0,
                    sample: (Instant::now(), 0),
                    detail: "Starting".into(),
                    kind: ActivityKind::Installed,
                    timestamp: activity::now_secs(),
                    from_version: None,
                    to_version: None,
                    source: None,
                    retryable: true,
                },
            );
        }
        self.rows.replace_all(self.transfer_rows());
        self.set_active_count();
        crate::installed::transfer_changed(id);
    }

    fn record_entry(
        &self,
        kind: ActivityKind,
        title: &str,
        detail: &str,
        from_version: Option<&str>,
        to_version: Option<&str>,
        source: Option<&str>,
    ) {
        let phase = match kind {
            ActivityKind::Failed => DownloadPhase::Failed,
            ActivityKind::Cancelled => DownloadPhase::Cancelled,
            _ => DownloadPhase::Done,
        };
        let seq = NEXT_ACTIVITY_ID.fetch_add(1, Ordering::Relaxed);
        let entry = Item {
            id: format!("act-{seq}"),
            name: title.to_owned(),
            phase,
            progress: if phase == DownloadPhase::Done {
                1.0
            } else {
                0.0
            },
            done: 0,
            total: None,
            speed: 0.0,
            sample: (Instant::now(), 0),
            detail: format_activity_detail(detail, from_version, to_version, source),
            kind,
            timestamp: activity::now_secs(),
            from_version: from_version.map(str::to_owned),
            to_version: to_version.map(str::to_owned),
            source: source.map(str::to_owned),
            retryable: false,
        };
        {
            let mut items = self.items.borrow_mut();
            let insert_at = items
                .iter()
                .position(|i| !is_active(i.phase))
                .unwrap_or(items.len());
            items.insert(insert_at, entry);
            trim_history(&mut items);
        }
        self.rows.replace_all(self.transfer_rows());
        self.set_active_count();
        self.save_history();
    }

    /// The row index and row view of the downloading item `id` after its
    /// byte counts moved; `None` when no downloading item has that id.
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        reason = "byte counts become a speed and a 0..1 share for display; Slint's progress is f32"
    )]
    fn item_moved(
        &self,
        id: &str,
        done: u64,
        total: Option<u64>,
        now: Instant,
    ) -> Option<(usize, TransferRow)> {
        let mut items = self.items.borrow_mut();
        let index = items
            .iter()
            .position(|i| i.id == id && i.phase == DownloadPhase::Downloading)?;
        let item = &mut items[index];
        let elapsed = now.saturating_duration_since(item.sample.0);
        if elapsed >= SPEED_WINDOW {
            let instant = done.saturating_sub(item.sample.1) as f64 / elapsed.as_secs_f64();
            item.speed = if item.speed > 0.0 {
                f64::midpoint(item.speed, instant)
            } else {
                instant
            };
            item.sample = (now, done);
        }
        item.done = done;
        item.total = total.filter(|t| *t > 0);
        item.progress = item
            .total
            .map_or(-1.0, |t| (done as f64 / t as f64).clamp(0.0, 1.0) as f32);
        item.detail = progress_text(done, item.total, item.speed);
        Some((index, transfer_row_of(item)))
    }

    fn progress(&self, id: &str, done: u64, total: Option<u64>, now: Instant) {
        let Some((index, row)) = self.item_moved(id, done, total, now) else {
            return;
        };
        self.rows.update_row(index, row);
        crate::installed::transfer_changed(id);
    }

    fn set_installing(&self, id: &str) {
        let outcome = {
            let mut items = self.items.borrow_mut();
            let Some(index) = items.iter().position(|i| i.id == id) else {
                return;
            };
            let item = &mut items[index];
            if !is_active(item.phase) {
                return;
            }
            item.phase = DownloadPhase::Installing;
            item.detail = "Installing".into();
            PhaseOutcome::Row(index)
        };
        if let PhaseOutcome::Row(index) = outcome {
            self.rows.update_row(index, self.transfer_row(index));
        }
        self.set_active_count();
        crate::installed::transfer_changed(id);
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "updates one item with outcome metadata and persists"
    )]
    fn finish_item(
        &self,
        id: &str,
        phase: DownloadPhase,
        kind: ActivityKind,
        detail: String,
        from_version: Option<String>,
        to_version: Option<String>,
        source: Option<String>,
    ) {
        let outcome = {
            let mut items = self.items.borrow_mut();
            let Some(index) = items.iter().position(|i| i.id == id) else {
                return;
            };
            let item = &mut items[index];
            // A late event for a row that already ended (a cancel races the
            // worker's own finish) must not overwrite the outcome.
            if !is_active(item.phase) && phase != DownloadPhase::Done {
                return;
            }
            item.phase = phase;
            item.kind = kind;
            item.detail = detail;
            item.timestamp = activity::now_secs();
            item.from_version = from_version;
            item.to_version = to_version;
            item.source = source;
            if phase == DownloadPhase::Done {
                item.progress = 1.0;
                item.retryable = false;
            }
            if trim_history(&mut items) {
                PhaseOutcome::List
            } else {
                PhaseOutcome::Row(index)
            }
        };
        match outcome {
            PhaseOutcome::Row(index) => {
                self.rows.update_row(index, self.transfer_row(index));
            }
            PhaseOutcome::List => self.rows.replace_all(self.transfer_rows()),
        }
        self.set_active_count();
        crate::installed::transfer_changed(id);
        self.save_history();
    }

    fn clear_finished(&self) {
        self.cleared.set(true);
        self.items.borrow_mut().retain(|i| is_active(i.phase));
        self.rows.replace_all(self.transfer_rows());
        self.set_active_count();
        self.save_history();
    }

    fn transfer_rows(&self) -> Vec<TransferRow> {
        self.items.borrow().iter().map(transfer_row_of).collect()
    }

    fn transfer_row(&self, index: usize) -> TransferRow {
        transfer_row_of(&self.items.borrow()[index])
    }

    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        reason = "Slint exposes i32 for the active download count"
    )]
    fn set_active_count(&self) {
        let active = self
            .items
            .borrow()
            .iter()
            .filter(|i| is_active(i.phase))
            .count();
        if let Some(app) = self.weak.upgrade() {
            app.global::<DownloadsState>()
                .set_active_count(active as i32);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_text_shows_what_is_known() {
        assert_eq!(progress_text(1_500_000, None, 0.0), "1.5 MB");
        assert_eq!(
            progress_text(12_300_000, Some(52_000_000), 4_200_000.0),
            "12.3 MB of 52.0 MB \u{b7} 4.2 MB/s"
        );
    }

    #[test]
    fn activity_detail_formats_source_and_version_transition() {
        assert_eq!(
            format_activity_detail("Updated", Some("1.0.0"), Some("1.1.0"), Some("Registry")),
            "Registry \u{b7} 1.0.0 \u{2192} 1.1.0 \u{b7} Updated"
        );
        assert_eq!(
            format_activity_detail("Installed", Some("1.0.0"), Some("1.0.0"), Some("CompLock")),
            "CompLock \u{b7} 1.0.0 \u{b7} Installed"
        );
        assert_eq!(
            format_activity_detail("Applied 3 changes to gameinfo.gi", None, None, None),
            "Applied 3 changes to gameinfo.gi"
        );
    }
}

/// The progress storm (docs/proof-plan.md, "Models change, they do not
/// rebuild"), as a flow the UI harness runs: a 30-row model, one slow
/// download reporting one update per percent, 100 progress ticks.
#[cfg(test)]
pub(crate) mod storm {
    use super::*;
    use std::cell::Cell;

    /// A rows sink that counts the two model operations: `update_row` is
    /// one `row_changed`, `replace_all` is a model reset that rebuilds
    /// every row it is handed.
    #[derive(Default)]
    struct CountingRows {
        model: Rc<VecModel<TransferRow>>,
        row_changes: Cell<usize>,
        resets: Cell<usize>,
        rebuilt_rows: Cell<usize>,
    }

    impl TransferRows for CountingRows {
        fn replace_all(&self, rows: Vec<TransferRow>) {
            self.resets.set(self.resets.get() + 1);
            self.rebuilt_rows.set(self.rebuilt_rows.get() + rows.len());
            self.model.set_vec(rows);
        }

        fn update_row(&self, index: usize, row: TransferRow) {
            self.row_changes.set(self.row_changes.get() + 1);
            self.model.set_row_data(index, row);
        }

        fn model(&self) -> ModelRc<TransferRow> {
            ModelRc::from(self.model.clone())
        }
    }

    impl CountingRows {
        fn reset_counts(&self) {
            self.row_changes.set(0);
            self.resets.set(0);
            self.rebuilt_rows.set(0);
        }
    }

    /// Each tick must be exactly one `row_changed` — the O(1) row update.
    /// A `set_vec` per tick fires a model reset and rebuilds all 30 rows
    /// every time (3000 row builds over the storm, O(rows) per tick) and
    /// fails this.
    pub(crate) fn progress_storm_flow(app: &App) {
        let counting = Rc::new(CountingRows::default());
        attach_with(app, &(counting.clone() as Rc<dyn TransferRows>));

        for i in 0..29 {
            started(&format!("finished-{i}"), &format!("Finished {i}"));
            finished(&format!("finished-{i}"), Outcome::Done);
        }
        started("slow-1", "Slow Mod");
        let state = app.global::<DownloadsState>();
        let rows = state.get_rows();
        assert_eq!(
            rows.row_count(),
            30,
            "29 finished transfers plus the downloading one"
        );
        assert_eq!(state.get_active_count(), 1);
        counting.reset_counts();

        let total = 52_428_800u64;
        for percent in 1..=100u64 {
            progress("slow-1", total * percent / 100, Some(total));
        }

        assert_eq!(
            counting.row_changes.get(),
            100,
            "100 progress ticks against 30 rows must be 100 row_changed notifications \
             (one row updated per tick, O(1)); a per-tick set_vec rebuilds all 30 rows \
             (3000 row builds over the storm, O(rows) per tick)"
        );
        assert_eq!(
            counting.resets.get(),
            0,
            "a progress tick must not reset the model: membership did not change"
        );
        assert_eq!(
            counting.rebuilt_rows.get(),
            0,
            "a progress tick must not rebuild rows a reset would drop"
        );
        let row = rows.row_data(0).expect("the downloading row");
        assert_eq!(row.id.as_str(), "slow-1");
        assert!(
            row.progress >= 0.99,
            "the storm's last tick reports a full download: {}",
            row.progress
        );
        assert_eq!(transfer("slow-1"), Some((1.0, false)));
        assert_eq!(rows.row_count(), 30, "the storm changed no membership");
    }
}
