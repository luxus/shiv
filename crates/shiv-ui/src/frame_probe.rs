//! Scroll frame-budget probe (`SHIV_FRAME_PROBE`, `dev-probe` / debug builds).
//!
//! Slint's `SoftwareRenderer` returns `Unsupported` from
//! `set_rendering_notifier`, so the probe attaches a
//! [`slint::winit_030::CustomApplicationHandler`] via
//! `BackendSelector::with_winit_custom_application_handler` to bracket
//! `WinitWindowAdapter::draw()` on `RedrawRequested` and every event-loop turn
//! (`new_events` -> `about_to_wait`). Alongside wall-clock `draw` and `turn`
//! times, Linux GNU builds record `draw_cpu` and `turn_cpu` from
//! `CLOCK_THREAD_CPUTIME_ID` so Shiv's own UI-thread work is separated from
//! Wayland `softbuffer` `wl_buffer.release` wait. Normal release builds omit
//! this module cleanly.

use std::cell::RefCell;
use std::ffi::OsStr;
use std::time::{Duration, Instant};

use slint::winit_030::{CustomApplicationHandler, EventResult, winit};

const BUDGET_US: u64 = 16_000;
const FRAME_CAPACITY: usize = 16_384;
const TURN_CAPACITY: usize = 32_768;
const TOP_SLOW: usize = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Off,
    Summary,
    Verbose,
}

fn parse_mode(value: Option<&OsStr>) -> Mode {
    let Some(raw) = value else {
        return Mode::Off;
    };
    let Some(text) = raw.to_str() else {
        return Mode::Summary;
    };
    if text.is_empty() || text == "0" {
        Mode::Off
    } else if text.eq_ignore_ascii_case("verbose") {
        Mode::Verbose
    } else {
        Mode::Summary
    }
}

fn duration_us(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

fn duration_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn thread_cpu_ns() -> Option<u64> {
    // SAFETY: `timespec` is plain-old-data and `clock_gettime` initializes it
    // for the calling thread when it returns 0.
    let ts = unsafe {
        let mut ts: libc::timespec = std::mem::zeroed();
        if libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &raw mut ts) != 0 {
            return None;
        }
        ts
    };
    let secs = u64::try_from(ts.tv_sec).ok()?;
    let nanos = u64::try_from(ts.tv_nsec).ok()?;
    secs.checked_mul(1_000_000_000)?.checked_add(nanos)
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
const fn thread_cpu_ns() -> Option<u64> {
    None
}

fn cpu_delta_us(start: Option<u64>, end: Option<u64>) -> Option<u64> {
    start.zip(end).map(|(s, e)| e.saturating_sub(s) / 1_000)
}

#[derive(Clone, Copy)]
struct ClockSample {
    wall: Instant,
    cpu_ns: Option<u64>,
}

impl ClockSample {
    fn now() -> Self {
        Self {
            wall: Instant::now(),
            cpu_ns: thread_cpu_ns(),
        }
    }
}

#[derive(Clone, Copy)]
struct FrameSample {
    offset_ms: u64,
    draw_us: u64,
    draw_cpu_us: Option<u64>,
}

#[derive(Clone, Copy)]
struct TurnSample {
    offset_ms: u64,
    turn_us: u64,
    turn_cpu_us: Option<u64>,
    draw_us: Option<u64>,
    draw_cpu_us: Option<u64>,
}

struct ProbeState {
    mode: Mode,
    recording: bool,
    window_origin: Option<Instant>,
    turn_start: Option<Instant>,
    turn_cpu_start: Option<u64>,
    turn_draw_us: Option<u64>,
    turn_draw_cpu_us: Option<u64>,
    redraw_start: Option<Instant>,
    redraw_cpu_start: Option<u64>,
    last_redraw: Option<Instant>,
    frames: Vec<FrameSample>,
    turns: Vec<TurnSample>,
    cadence_us: Vec<u64>,
}

impl ProbeState {
    fn new(mode: Mode, wait_for_scroll_window: bool) -> Self {
        let (recording, window_origin) = if wait_for_scroll_window {
            (false, None)
        } else {
            (true, Some(Instant::now()))
        };
        Self {
            mode,
            recording,
            window_origin,
            turn_start: None,
            turn_cpu_start: None,
            turn_draw_us: None,
            turn_draw_cpu_us: None,
            redraw_start: None,
            redraw_cpu_start: None,
            last_redraw: None,
            frames: Vec::with_capacity(FRAME_CAPACITY),
            turns: Vec::with_capacity(TURN_CAPACITY),
            cadence_us: Vec::with_capacity(FRAME_CAPACITY),
        }
    }

    fn flush_redraw(&mut self, sample: ClockSample) {
        let cpu_start = self.redraw_cpu_start.take();
        let Some(start) = self.redraw_start.take() else {
            return;
        };
        let draw_us = duration_us(sample.wall.saturating_duration_since(start));
        let draw_cpu_us = cpu_delta_us(cpu_start, sample.cpu_ns);
        let offset_ms = self.window_origin.map_or(0, |origin| {
            duration_ms(start.saturating_duration_since(origin))
        });
        self.frames.push(FrameSample {
            offset_ms,
            draw_us,
            draw_cpu_us,
        });
        if self.turn_start.is_some() {
            self.turn_draw_us = Some(self.turn_draw_us.unwrap_or(0).saturating_add(draw_us));
            if let Some(cpu_us) = draw_cpu_us {
                self.turn_draw_cpu_us =
                    Some(self.turn_draw_cpu_us.unwrap_or(0).saturating_add(cpu_us));
            }
        }
    }

    fn on_new_events(&mut self, sample: ClockSample) {
        self.flush_redraw(sample);
        self.turn_start = Some(sample.wall);
        self.turn_cpu_start = sample.cpu_ns;
        self.turn_draw_us = None;
        self.turn_draw_cpu_us = None;
    }

    fn on_redraw_requested(&mut self, sample: ClockSample) {
        self.flush_redraw(sample);
        if !self.recording {
            return;
        }
        if let Some(prev) = self.last_redraw {
            self.cadence_us
                .push(duration_us(sample.wall.saturating_duration_since(prev)));
        }
        self.last_redraw = Some(sample.wall);
        self.redraw_start = Some(sample.wall);
        self.redraw_cpu_start = sample.cpu_ns;
    }

    fn on_about_to_wait(&mut self, sample: ClockSample) {
        self.flush_redraw(sample);
        let turn_start = self.turn_start.take();
        let turn_cpu_start = self.turn_cpu_start.take();
        let draw_us = self.turn_draw_us.take();
        let draw_cpu_us = self.turn_draw_cpu_us.take();
        if self.recording
            && let Some(turn_start) = turn_start
        {
            let turn_us = duration_us(sample.wall.saturating_duration_since(turn_start));
            let turn_cpu_us = cpu_delta_us(turn_cpu_start, sample.cpu_ns);
            let offset_ms = self.window_origin.map_or(0, |origin| {
                duration_ms(turn_start.saturating_duration_since(origin))
            });
            self.turns.push(TurnSample {
                offset_ms,
                turn_us,
                turn_cpu_us,
                draw_us,
                draw_cpu_us,
            });
        }
    }
}

// CustomApplicationHandler runs exclusively on the UI thread, so thread-local
// storage avoids mutex traffic during redraws.
thread_local! {
    static STATE: RefCell<Option<ProbeState>> = const { RefCell::new(None) };
}

fn with_state(f: impl FnOnce(&mut ProbeState)) {
    STATE.with(|slot| {
        if let Some(state) = slot.borrow_mut().as_mut() {
            f(state);
        }
    });
}

/// Winit event-loop tap installed via `BackendSelector` when `SHIV_FRAME_PROBE` is set.
pub(crate) struct ProbeHandler;

impl CustomApplicationHandler for ProbeHandler {
    fn resumed(&mut self, _event_loop: &winit::event_loop::ActiveEventLoop) -> EventResult {
        let sample = ClockSample::now();
        with_state(|state| state.flush_redraw(sample));
        EventResult::Propagate
    }

    fn window_event(
        &mut self,
        _event_loop: &winit::event_loop::ActiveEventLoop,
        _window_id: winit::window::WindowId,
        _winit_window: Option<&winit::window::Window>,
        slint_window: Option<&slint::Window>,
        event: &winit::event::WindowEvent,
    ) -> EventResult {
        let sample = ClockSample::now();
        with_state(|state| {
            if slint_window.is_some() && matches!(event, winit::event::WindowEvent::RedrawRequested)
            {
                state.on_redraw_requested(sample);
            } else {
                state.flush_redraw(sample);
            }
        });
        EventResult::Propagate
    }

    fn new_events(
        &mut self,
        _event_loop: &winit::event_loop::ActiveEventLoop,
        _cause: winit::event::StartCause,
    ) -> EventResult {
        let sample = ClockSample::now();
        with_state(|state| state.on_new_events(sample));
        EventResult::Propagate
    }

    fn device_event(
        &mut self,
        _event_loop: &winit::event_loop::ActiveEventLoop,
        _device_id: winit::event::DeviceId,
        _event: winit::event::DeviceEvent,
    ) -> EventResult {
        let sample = ClockSample::now();
        with_state(|state| state.flush_redraw(sample));
        EventResult::Propagate
    }

    fn about_to_wait(&mut self, _event_loop: &winit::event_loop::ActiveEventLoop) -> EventResult {
        let sample = ClockSample::now();
        with_state(|state| state.on_about_to_wait(sample));
        EventResult::Propagate
    }

    fn suspended(&mut self, _event_loop: &winit::event_loop::ActiveEventLoop) -> EventResult {
        let sample = ClockSample::now();
        with_state(|state| state.flush_redraw(sample));
        EventResult::Propagate
    }

    fn exiting(&mut self, _event_loop: &winit::event_loop::ActiveEventLoop) -> EventResult {
        let sample = ClockSample::now();
        with_state(|state| state.flush_redraw(sample));
        EventResult::Propagate
    }
}

/// Initialize the frame probe when `SHIV_FRAME_PROBE` is set, returning `None`
/// otherwise so normal launches pay no cost.
pub(crate) fn make_handler() -> Option<ProbeHandler> {
    let mode = parse_mode(std::env::var_os("SHIV_FRAME_PROBE").as_deref());
    if mode == Mode::Off {
        return None;
    }
    // Wait for scroll_probe's first tick when SHIV_SCROLL_SECS is active so
    // startup and initial catalog fetch stay outside the measured window.
    let wait_for_scroll = std::env::var("SHIV_SCROLL_SECS").is_ok_and(|v| v.parse::<u64>().is_ok());
    STATE.with(|slot| {
        *slot.borrow_mut() = Some(ProbeState::new(mode, wait_for_scroll));
    });
    Some(ProbeHandler)
}

/// Begin recording scroll frames once the automated scroll window starts.
pub(crate) fn start_window() {
    with_state(|state| {
        state.recording = true;
        state.window_origin = Some(Instant::now());
        state.last_redraw = None;
        state.redraw_start = None;
        state.redraw_cpu_start = None;
        state.turn_draw_us = None;
        state.turn_draw_cpu_us = None;
    });
}

/// Stop recording scroll frames right before the event loop quits.
pub(crate) fn stop_window() {
    let sample = ClockSample::now();
    with_state(|state| {
        state.flush_redraw(sample);
        state.recording = false;
        state.last_redraw = None;
        state.redraw_cpu_start = None;
        state.turn_start = None;
        state.turn_cpu_start = None;
        state.turn_draw_us = None;
        state.turn_draw_cpu_us = None;
    });
}

#[derive(Debug, PartialEq, Eq)]
struct Stats {
    count: usize,
    min: u64,
    p50: u64,
    p95: u64,
    p99: u64,
    max: u64,
    over_budget: usize,
}

fn percentile(sorted: &[u64], pct: usize) -> u64 {
    let rank = (sorted.len() * pct).div_ceil(100);
    let idx = rank.saturating_sub(1).min(sorted.len().saturating_sub(1));
    sorted.get(idx).copied().unwrap_or(0)
}

impl Stats {
    fn compute(samples: &mut [u64]) -> Option<Self> {
        if samples.is_empty() {
            return None;
        }
        samples.sort_unstable();
        let count = samples.len();
        let min = samples.first().copied().unwrap_or(0);
        let max = samples.last().copied().unwrap_or(0);
        let over_budget = samples.iter().filter(|&&us| us > BUDGET_US).count();
        Some(Self {
            count,
            min,
            p50: percentile(samples, 50),
            p95: percentile(samples, 95),
            p99: percentile(samples, 99),
            max,
            over_budget,
        })
    }
}

fn print_budget_line(label: &str, samples: &mut [u64]) {
    let Some(stats) = Stats::compute(samples) else {
        eprintln!("SHIV_FRAME_PROBE {label} count=0");
        return;
    };
    eprintln!(
        "SHIV_FRAME_PROBE {label} count={} min={}us p50={}us p95={}us p99={}us max={}us over_16ms={}",
        stats.count, stats.min, stats.p50, stats.p95, stats.p99, stats.max, stats.over_budget,
    );
}

fn print_cadence_line(samples: &mut [u64]) {
    let Some(stats) = Stats::compute(samples) else {
        eprintln!("SHIV_FRAME_PROBE cadence count=0");
        return;
    };
    eprintln!(
        "SHIV_FRAME_PROBE cadence count={} p50={}us p95={}us p99={}us max={}us",
        stats.count, stats.p50, stats.p95, stats.p99, stats.max,
    );
}

fn fmt_optional_us(us: Option<u64>) -> String {
    us.map_or_else(|| "n/a".to_owned(), |value| format!("{value}us"))
}

fn print_slow_draws(mut frames: Vec<FrameSample>) {
    frames.sort_by(|a, b| {
        b.draw_us
            .cmp(&a.draw_us)
            .then_with(|| a.offset_ms.cmp(&b.offset_ms))
    });
    for (rank, frame) in frames.iter().take(TOP_SLOW).enumerate() {
        eprintln!(
            "SHIV_FRAME_PROBE slow_draw #{} t=+{}ms draw={}us draw_cpu={}",
            rank + 1,
            frame.offset_ms,
            frame.draw_us,
            fmt_optional_us(frame.draw_cpu_us),
        );
    }
}

fn print_slow_turns(mut turns: Vec<TurnSample>) {
    turns.sort_by(|a, b| {
        b.turn_us
            .cmp(&a.turn_us)
            .then_with(|| a.offset_ms.cmp(&b.offset_ms))
    });
    for (rank, turn) in turns.iter().take(TOP_SLOW).enumerate() {
        eprintln!(
            "SHIV_FRAME_PROBE slow_turn #{} t=+{}ms turn={}us turn_cpu={} draw={} draw_cpu={}",
            rank + 1,
            turn.offset_ms,
            turn.turn_us,
            fmt_optional_us(turn.turn_cpu_us),
            fmt_optional_us(turn.draw_us),
            fmt_optional_us(turn.draw_cpu_us),
        );
    }
}

/// Flush any in-flight redraw and print summary statistics to stderr if enabled.
pub(crate) fn report_if_enabled() {
    let Some(mut state) = STATE.with(|slot| slot.borrow_mut().take()) else {
        return;
    };
    state.flush_redraw(ClockSample::now());
    let mut draws: Vec<u64> = state.frames.iter().map(|frame| frame.draw_us).collect();
    let mut draw_cpus: Vec<u64> = state
        .frames
        .iter()
        .filter_map(|frame| frame.draw_cpu_us)
        .collect();
    let mut turns: Vec<u64> = state.turns.iter().map(|turn| turn.turn_us).collect();
    let mut turn_cpus: Vec<u64> = state
        .turns
        .iter()
        .filter_map(|turn| turn.turn_cpu_us)
        .collect();
    print_budget_line("draw", &mut draws);
    if !draw_cpus.is_empty() {
        print_budget_line("draw_cpu", &mut draw_cpus);
    }
    print_budget_line("turn", &mut turns);
    if !turn_cpus.is_empty() {
        print_budget_line("turn_cpu", &mut turn_cpus);
    }
    print_cadence_line(&mut state.cadence_us);
    if state.mode == Mode::Verbose {
        print_slow_draws(state.frames);
        print_slow_turns(state.turns);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_mode_values() {
        assert_eq!(parse_mode(None), Mode::Off);
        assert_eq!(parse_mode(Some(OsStr::new(""))), Mode::Off);
        assert_eq!(parse_mode(Some(OsStr::new("0"))), Mode::Off);
        assert_eq!(parse_mode(Some(OsStr::new("1"))), Mode::Summary);
        assert_eq!(parse_mode(Some(OsStr::new("yes"))), Mode::Summary);
        assert_eq!(parse_mode(Some(OsStr::new("verbose"))), Mode::Verbose);
        assert_eq!(parse_mode(Some(OsStr::new("VERBOSE"))), Mode::Verbose);
    }

    #[test]
    fn stats_empty_and_single() {
        assert_eq!(Stats::compute(&mut []), None);
        assert_eq!(
            Stats::compute(&mut [4200]),
            Some(Stats {
                count: 1,
                min: 4200,
                p50: 4200,
                p95: 4200,
                p99: 4200,
                max: 4200,
                over_budget: 0,
            })
        );
        assert_eq!(Stats::compute(&mut [16_000]).unwrap().over_budget, 0);
        assert_eq!(Stats::compute(&mut [16_001]).unwrap().over_budget, 1);
    }

    #[test]
    fn stats_percentiles_and_budget() {
        let mut samples: Vec<u64> = (1..=100).rev().map(|n| n * 200).collect();
        let stats = Stats::compute(&mut samples).unwrap();
        assert_eq!(stats.count, 100);
        assert_eq!(stats.min, 200);
        assert_eq!(stats.p50, 10_000);
        assert_eq!(stats.p95, 19_000);
        assert_eq!(stats.p99, 19_800);
        assert_eq!(stats.max, 20_000);
        assert_eq!(stats.over_budget, 20);
    }

    #[test]
    fn records_non_redraw_turns_while_recording() {
        let mut state = ProbeState::new(Mode::Summary, false);
        let t0 = ClockSample::now();
        state.on_new_events(t0);
        let t1 = ClockSample {
            wall: t0.wall + Duration::from_micros(350),
            cpu_ns: t0.cpu_ns.map(|ns| ns + 350_000),
        };
        state.on_about_to_wait(t1);
        assert!(state.frames.is_empty());
        assert_eq!(state.turns.len(), 1);
        assert_eq!(state.turns[0].turn_us, 350);
        assert_eq!(state.turns[0].draw_us, None);
    }

    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    #[test]
    fn thread_cpu_clock_is_monotonic_on_linux() {
        let t0 = thread_cpu_ns().expect("CLOCK_THREAD_CPUTIME_ID available");
        let mut spin = 0u64;
        for i in 0..10_000u64 {
            spin = spin.wrapping_add(i);
        }
        std::hint::black_box(spin);
        let t1 = thread_cpu_ns().expect("CLOCK_THREAD_CPUTIME_ID still available");
        assert!(t1 >= t0, "thread cpu clock went backwards: {t0} -> {t1}");
    }
}
