//! Downloads screen: a passive view over the installs Browse runs.
//!
//! Browse owns the install machinery (workers, cancellation, the in-flight
//! guards); it reports four events here: started, progress, installing and
//! finished. This module keeps the list (newest first, a bounded history for
//! this session) and turns Cancel and Retry back into Browse calls.
//!
//! The rows change, they do not rebuild (docs/proof-plan.md, "Models
//! change, they do not rebuild"): a progress tick updates the one row it
//! moved, and the whole model is replaced only when membership changes —
//! a transfer appears, ends or leaves. Every mutation goes through the
//! [`TransferRows`] sink, so the storm test can count the two shapes.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::{ComponentHandle, Model, ModelRc, VecModel, Weak};

use crate::browse::fmt_mb;
use crate::{App, DownloadPhase, DownloadsState, TransferRow};

/// Finished rows kept; older ones drop off.
const HISTORY: usize = 30;
/// Speed is averaged over at least this long, so it does not flicker.
const SPEED_WINDOW: Duration = Duration::from_millis(750);

/// Where the Downloads rows go. `replace_all` swaps the whole list — a
/// model reset, every row re-read — and is for membership changes only;
/// `update_row` changes one row in place, the single `row_changed` a
/// progress tick must be. `model` is what the screen reads.
pub(crate) trait TransferRows {
    /// Replace the whole list: rows were added or dropped.
    fn replace_all(&self, rows: Vec<TransferRow>);
    /// Update one row in place (its transfer moved on).
    fn update_row(&self, index: usize, row: TransferRow);
    /// The model the Downloads screen reads.
    fn model(&self) -> ModelRc<TransferRow>;
}

/// The rows in the Slint [`VecModel`] the Downloads screen reads.
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
}

struct Downloads {
    weak: Weak<App>,
    rows: Rc<dyn TransferRows>,
    items: RefCell<Vec<Item>>,
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
    attach_with(app, &rows);
}

/// Wire the Downloads screen to `app`, writing its rows through `rows`
/// (the storm test passes a counting sink; `attach` passes the Slint model).
pub(crate) fn attach_with(app: &App, rows: &Rc<dyn TransferRows>) {
    let model = rows.model();
    let controller = Rc::new(Downloads {
        weak: app.as_weak(),
        rows: rows.clone(),
        items: RefCell::new(Vec::new()),
    });
    let state = app.global::<DownloadsState>();
    state.set_rows(model);
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
    with_downloads(|d| d.set_phase(id, DownloadPhase::Installing, "Installing".into()));
}

pub(crate) fn finished(id: &str, outcome: Outcome) {
    with_downloads(|d| match outcome {
        Outcome::Done => d.set_phase(id, DownloadPhase::Done, "Installed".into()),
        Outcome::DoneUnverified => {
            d.set_phase(id, DownloadPhase::Done, "Installed, not verified".into());
        }
        Outcome::Cancelled => d.set_phase(id, DownloadPhase::Cancelled, "Cancelled".into()),
        Outcome::Failed(text) => d.set_phase(id, DownloadPhase::Failed, text),
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

fn transfer_row_of(i: &Item) -> TransferRow {
    TransferRow {
        id: i.id.as_str().into(),
        name: i.name.as_str().into(),
        phase: i.phase,
        progress: i.progress,
        detail: i.detail.as_str().into(),
    }
}

/// What a phase change did to the list.
enum PhaseOutcome {
    /// Nothing to show: unknown id, or a late event for a row that
    /// already ended.
    Unchanged,
    /// The row changed in place.
    Row(usize),
    /// The history bound dropped rows: the whole list changed.
    List,
}

impl Downloads {
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
                },
            );
        }
        self.rows.replace_all(self.transfer_rows());
        self.set_active_count();
        crate::installed::transfer_changed(id);
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

    /// Apply a phase change to the item `id`, keeping the finished history
    /// within [`HISTORY`]: what the rows sink must do to show it.
    fn phase_applied(&self, id: &str, phase: DownloadPhase, detail: String) -> PhaseOutcome {
        let mut items = self.items.borrow_mut();
        let Some(index) = items.iter().position(|i| i.id == id) else {
            return PhaseOutcome::Unchanged;
        };
        let item = &mut items[index];
        // A late event for a row that already ended (a cancel races the
        // worker's own finish) must not overwrite the outcome.
        let ended = matches!(
            item.phase,
            DownloadPhase::Done | DownloadPhase::Failed | DownloadPhase::Cancelled
        );
        if ended && phase != DownloadPhase::Done {
            return PhaseOutcome::Unchanged;
        }
        item.phase = phase;
        item.detail = detail;
        if phase == DownloadPhase::Done {
            item.progress = 1.0;
        }
        let before = items.len();
        let mut finished = 0;
        items.retain(|i| {
            let ended = matches!(
                i.phase,
                DownloadPhase::Done | DownloadPhase::Failed | DownloadPhase::Cancelled
            );
            if ended {
                finished += 1;
            }
            !ended || finished <= HISTORY
        });
        match items.len() {
            len if len != before => PhaseOutcome::List,
            _ => PhaseOutcome::Row(index),
        }
    }

    fn set_phase(&self, id: &str, phase: DownloadPhase, detail: String) {
        match self.phase_applied(id, phase, detail) {
            PhaseOutcome::Unchanged => {}
            PhaseOutcome::Row(index) => {
                self.rows.update_row(index, self.transfer_row(index));
            }
            PhaseOutcome::List => self.rows.replace_all(self.transfer_rows()),
        }
        self.set_active_count();
        crate::installed::transfer_changed(id);
    }

    fn clear_finished(&self) {
        self.items.borrow_mut().retain(|i| {
            matches!(
                i.phase,
                DownloadPhase::Downloading | DownloadPhase::Installing
            )
        });
        self.rows.replace_all(self.transfer_rows());
        self.set_active_count();
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
            .filter(|i| {
                matches!(
                    i.phase,
                    DownloadPhase::Downloading | DownloadPhase::Installing
                )
            })
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
