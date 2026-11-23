# The Shiv proof plan

Every rule this repository follows exists because a bug happened here. This
plan does the same for performance: Shiv will not be "as fast as" the webview
managers — it will be measurably faster by a margin, and every performance,
memory, dependency, or architecture decision has a proof that it is the best
available one. (See `docs/engineering-core.md` for the reusable core rules.)

## The three laws of a decision

1. **A performance or architecture decision without a measurement is an
   opinion.** Before a design is accepted, the alternative candidates are named
   and the measurement or benchmark that picked the winner is committed beside
   the code. Obvious choices say so in one line.
2. **The proof lives in the repository.** A claim like "this avoids the
   allocation" is a comment away from being wrong; a `criterion` benchmark, a
   counting-allocator ceiling, a static guard, or an invariant test is not.
3. **The target is the gap, not the tie.** Each measured dimension names the
   competitor number and the margin Shiv holds over it (idle RSS, cold start,
   scroll frame budget, install wall clock), and a regression that closes the
   margin fails the build.

## What "best" means, measured

| Dimension | Metric | Instrument | The margin to hold |
| --- | --- | --- | --- |
| Memory (idle) | RSS after catalog load, 10 min idle | `/proc/<pid>/status` VmRSS, repeated runs | DMM idles in the gigabytes; Shiv holds **< 150 MB** loaded, **< 90 MB** on a cold Browse |
| Memory (work) | RSS after a 60 s full-list scroll, and after Repair | peak VmHWM and post-`malloc_trim` RSS | scrolling may not leak: post-scroll RSS within **10 %** of loaded; Repair releases back |
| Start-up | cold start -> first paint with catalog | probe in `docs/perf-results.md` | **< 1.5 s** cold, **< 600 ms** warm, DMM-class apps measured beside |
| Scroll | frame budget while flinging the list | frame-tap probe (tracked in issue #9) | **no frame > 16 ms** on a mid-range laptop at 144 Hz |
| Install | wall clock for a 100 MB archive | `shiv-core/tests/install_probe.rs` | extraction at **> 60 %** of cold disk-read speed (warm copy ceiling is ~50 %); download cancel **< 50 ms** (cancellable extraction is tracked separately) |
| Search | keystroke -> list updated | `SHIV_SEARCH_PROBE` | **< 16 ms** per keystroke on 10k mods, no main-thread stall |
| Binary | release size, link time | `ls`, CI log | the AppImage stays **< 25 MB**; every heavy dependency added needs a justification below |
| Battery | idle CPU over 10 min | `utime+stime` ticks | **0 ticks in 10 min** idle; a poll is a bug, timers are for user-scale waits |

These numbers are not aspirations; they are gates. CI runs the container-safe
gates (criterion A/B bench diffs, allocation ceilings, static guards, unit and
integration suites), and the hardware-profile numbers are recorded on **lea**
in `docs/perf-results.md` before a release tag.

### Justified dependencies

- **`rquickjs` & `symphonia`:** CompLock's Hitsound and HP Bar builders run as
  their own pinned scripts in QuickJS (`rquickjs`). Deno and Node are the
  runtimes that already execute those scripts, and neither fits this gate: a
  current Node binary is about 50 MB and Deno is larger, against an AppImage
  measured at 19.7 MB. QuickJS adds about 1.5 MB, has no filesystem and no
  network unless the host binds them, and the host binds neither. `symphonia`
  decodes a hitsound the user uploads (WAV, MP3, OGG, FLAC) down to the mono
  44.1 kHz samples the script encodes. The site does that step in the browser,
  which cannot ship here.

## The proof obligations, by area

### UI thread (Slint)

- **Nothing blocks the event loop.** Proof: `crates/shiv-ui/tests/guards.rs`
  scans the controller sources and fails on `fs::`, `rfd::`, `ureq::`, and
  `sleep(` outside `spawn_worker` and `/// Worker body` functions.
- **Rows are O(1) to render.** Proof: models mutate via `set_row_data` rather
  than per-frame `Vec` rebuilds. (Frame-budget tap is tracked in issue #9.)
- **Models change, they do not rebuild.** Proof:
  `downloads::storm::progress_storm_flow` replays 100 progress ticks against a
  30-row model and asserts exactly 100 `row_changed` notifications and 0 model
  resets.

### Worker threads

- **One allocation profile per flow.** Proof:
  `crates/shiv-core/tests/alloc_ceilings.rs` runs core user flows (install,
  `set_pak`, interchange export/import) under `CountingAlloc` with explicit
  ceilings; a regression above the ceiling fails `cargo test --workspace` and
  `perf.yml`.
- **Threads sleep on a `Condvar`, never poll.** Proof:
  `crates/shiv-ui/tests/guards.rs` fails on any loop that re-checks locked
  shared state on a `sleep` timer instead of parking on `Condvar::wait` /
  `wait_timeout`.

### Core paths

- **`set_pak`/`move_mod` stay flat across the realizable domain (1..=99 paks).**
  Proof: `crates/shiv-core/benches/reorder.rs` benchmarks `move_mod` and
  `set_pak` at 25/50/99 movable mods (99 is the engine's `PAK_CAPACITY`) and
  `group` at 100/500/1000 rows, gated on every push and PR by `perf.yml`.
- **Install staging never double-writes.** Proof:
  `install_writes_each_archive_byte_once` and
  `install_writes_are_counted_and_placement_renames` (`shiv-core/tests/install.rs`)
  pin `disk_bytes_written` to the archive's VPK bytes exactly.
- **Interchange import streams.** Proof:
  `crates/shiv-core/tests/interchange_import_peak.rs` imports and adopts a
  90-mod bundle (near the 99-pak engine limit) with a 12 MiB largest VPK and
  asserts total allocation stays <= 1.2x the largest VPK.

### Network

- **The catalog is parsed in one pass.** Proof: `crates/shiv-api/benches/catalog.rs`
  feeds a 10k-row catalog through `list_mods` and reports throughput, gated by
  `perf.yml`.
- **Thumbnails are decoded once, at display size.** Proof:
  `a_large_source_lands_in_the_cache_at_display_bytes` (`shiv-ui/src/thumbs.rs`)
  asserts cached RGBA bytes equal `width * height * 4` at display dimensions.
- **Redirects are followed once and checked per hop.** Proof: stub tests pin
  exact request counts and refuse cross-origin redirects and HTTPS->HTTP
  downgrades.

### Native & release profile

- **Zero unproved panics in production paths.** `deny(clippy::unwrap_used,
  clippy::expect_used)` holds across `shiv-core`, `shiv-api`, and `shiv-ui`.
- **Release profile decisions are measured (`docs/perf-results.md`, issue #8):**
  `panic = "unwind"` stays (abort saves 3.55 MB / 10.7 % with no cold-start
  win while turning any worker panic into a whole-app crash), `lto = true`
  stays (saves 2.04 MB over thin LTO), and `strip = true` stays (saves 4.15 MB).

## The review protocol

1. **Naming pass.** Queries and constructors name what they return; side-effecting
   functions carry a verb; types name the concept. A name that needs a comment
   to clarify is fixed instead of commented.
2. **Comment pass.** No "what" inline comments, section banners, or commented-out
   code. Inline comments are restricted to 1–2 lines of non-obvious **why**
   (`SAFETY`, OS/allocator/protocol quirks). Doc comments state contracts on
   public items (and private items only when read by a guard like
   `/// Worker body`).
3. **Outcome & purpose pass.** Every symbol earns its place with minimal
   visibility; every function reads cleanly toward one outcome with early
   returns for refusals and nesting <= 3 deep; no magic numbers.
4. **Proof & boundary pass.** Performance/architecture choices cite alternatives
   and measurements (or state in one line why they are obvious); trust
   boundaries (paths, symlinks, URLs/redirects, stream caps) have explicit
   tests.
5. **Rules pass.** `cargo clippy --workspace --all-targets -- -D warnings`,
   `cargo fmt --check`, allocation ceilings, static guards, and invariant tests
   all end green.

## Deliverables

- `.github/workflows/ci.yml` — format, clippy, workspace tests (including
  `guards.rs`, `pak_occupancy.rs`, `install_permit.rs`, `alloc_ceilings.rs`,
  `interchange_import_peak.rs`, and `bench_diff_summary.py` unit tests) on
  every push and pull request.
- `.github/workflows/perf.yml` — same-runner A/B criterion bench diffs (fail
  closed on missing base logs or >25 % significant regressions) plus allocation
  gates on pushes to `main`, pull requests, and `workflow_dispatch`.
- `docs/perf-results.md` — recorded baselines, experiments, and verdicts with
  date, machine, method, and commands.
- A release checklist item: refresh hardware-profile numbers in
  `docs/perf-results.md` on **lea** before tagging a release.

## The standing question

Every PR ends by answering: *What did you prove?* (mandatory) — plus *What did
you make faster?* and *What did you make smaller?* (if a PR targets correctness
or security rather than speed/size, say so explicitly and confirm existing
gates hold).
