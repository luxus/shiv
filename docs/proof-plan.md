# The Shiv proof plan

Every rule this repository follows exists because a bug happened here.
This plan does the same for performance: Shiv will not be "as fast as"
the webview managers — it will be measurably faster by a margin, and every
decision in the code will have a proof that it is the best available one.

## The three laws of a decision

1. **A decision without a measurement is an opinion.** Before a design is
   accepted, the alternative candidates are named and the benchmark that
   picked the winner is committed beside the code.
2. **The proof lives in the repository.** A claim like "this avoids the
   allocation" is a comment away from being wrong; a `criterion` benchmark,
   a `perf` diff or a stress test is not.
3. **The target is the gap, not the tie.** Each measured dimension names
   the competitor number and the margin Shiv holds over it (idle RSS,
   cold start, scroll frame budget, install wall clock), and a regression
   that closes the margin fails the build.

## What "best" means, measured

| Dimension | Metric | Instrument | The margin to hold |
|---|---|---|---|
| Memory (idle) | RSS after catalog load, 10 min idle | `/proc/<pid>/status` VmRSS, repeated runs | DMM idles in the gigabytes; Shiv holds **< 150 MB** loaded, **< 90 MB** on a cold Browse |
| Memory (work) | RSS after a 60 s full-list scroll, and after Repair | peak VmHWM and post-`malloc_trim` RSS | scrolling may not leak: post-scroll RSS within **10 %** of loaded; Repair releases back |
| Start-up | cold start → first paint with catalog | `hyperfine` + a frame-timestamp probe | **< 1.5 s** cold, **< 600 ms** warm, DMM-class apps measured beside |
| Scroll | frame budget while flinging the list | `perf` (Linux), `tracy`-style frame taps if needed | **no frame > 16 ms** on a mid-range laptop at 144 Hz |
| Install | wall clock for a 100 MB archive | timer in the test | extraction at **> 60 %** of disk-read speed, cancel < 50 ms to respond |
| Search | keystroke → list updated | in-test timer | **< 16 ms** per keystroke on 10k mods, no main-thread stall |
| Binary | release size, link time | `ls`, CI log | the AppImage stays **< 25 MB**; every dependency added needs a line here saying why nothing leaner does the job |
| Battery | idle CPU over 10 min | `utime+stime` ticks | **0 ticks in 10 min** idle; a poll is a bug, timers are for user-scale waits |

These numbers are not aspirations; they are gates. A CI job runs the ones
that can run in a container (search, install, binary size, allocation
counts) on every push, and the full suite runs on a hardware profile
before a release tag.

## The proof obligations, by area

### UI thread (Slint)

- **Nothing blocks the event loop.** Proof: a debug-build watcher asserts
  that every `spawn_worker` callback returns within a budget; any disk,
  network or dialog call found on the event loop fails a clippy-adjacent
  lint test that greps the controller modules for `std::fs::`, `ureq` and
  `rfd::` outside `spawn_worker` closures.
- **Rows are O(1) to render.** Proof: a benchmark builds a 10k-row model
  and scrolls it; the row build cost is measured per row and must not
  scale with list length (no per-frame `Vec` rebuilds — the
  `set_row_data` path is the only allowed mutation).
- **Models change, they do not rebuild.** Proof: a test replays the
  Downloads progress storm (one update per percent on a slow download)
  against a 30-row model and counts `row_changed` notifications; a rebuild
  path would show as O(n) per tick and fail.

### Worker threads

- **One allocation profile per flow.** Proof: each user flow (install,
  toggle, import, catalog load) runs under a counting allocator in the
  test suite with an expected allocation ceiling; a new allocation above
  the ceiling fails the test and the author either removes it or raises
  the ceiling with a written reason.
- **Threads sleep, never poll.** Proof: a debug-build assertion fails if
  a worker's park time is below a threshold while its queue is empty; the
  grep test also fails on `sleep(` in a loop.

### Core paths

- **`set_pak`/`move_mod` are O(n log n) in mods, not O(n²).** Proof: a
  benchmark with 500 mods moves the top one and asserts the time is
  flat per mod count (the plan is built once and applied twice).
- **Install staging never double-writes.** Proof: counted bytes written
  in the install test; a copy+rename path that writes the archive bytes
  twice fails.
- **Interchange import streams.** Proof: a 10k-mod bundle import's peak
  RSS stays within **1.2×** the largest VPK (hashing is chunked, files are
  moved when the filesystem allows it, never re-buffered).

### Network

- **The catalog is parsed in one pass.** Proof: a benchmark feeds the
  10.9k-mod catalog fixture through `list_mods` and reports MB/s; a
  regression below the recorded baseline fails (serde struct-per-row,
  no intermediate `serde_json::Value`).
- **Thumbnails are decoded once, at display size.** Proof: the decode
  budget test asserts the decoded bytes in the cache equal
  `width × height × 4` per image, not the source bitmap's size.
- **Redirects are followed once, not per body read.** Proof: the stub
  tests pin the request count exactly; any extra request fails.

### Native

- **Zero unwinds in hot paths.** The `deny(clippy::unwrap_used)` rule
  stands; this plan adds `panic = "abort"` as a release-profile question
  to answer with a measured binary/latency diff before adopting.
- **SIMD-friendly layouts where it pays.** Hashing (SHA-256 over VPK
  bytes) and image decode are the only loops where the CPU vendor
  libraries can win; the proof is a benchmark against the `sha2` crate's
  feature matrix, not a hand-rolled kernel.

## The review protocol

1. **Naming pass.** Every function name says what it returns, not what it
   does; every type name is the concept. A name that needs a comment to
   clarify is a wrong name, and the comment is not written — the name is
   fixed.
2. **Zero-comment pass.** A comment in new code is a bug report against
   the code's clarity. Doc comments on public items state the contract
   (the *why*, the invariants, the refusal reasons); anything else is
   rewritten into a name, a type, or a smaller function. The existing
   comments are audited the same way and retired where the code now
   speaks.
3. **Outcome pass.** Every function reads as a sentence toward its one
   outcome; early returns for every refusal; no nested conditionals over
   three deep; the last line of a function is its result, not a `let mut`.
4. **Proof pass.** For every decision the reviewer asks: *what were the
   alternatives, what measurement picked this one, where is that
   measurement*? If the answer is silence, the decision is either
   obvious (and the PR says so in one line) or it is not made yet.
5. **Rules pass.** `cargo clippy --workspace --all-targets -- -D
   warnings`, `cargo fmt --check`, the allocation ceilings, the grep
   lints, the property tests. The pass ends green or the review does not
   end.

## Deliverables

- `.github/workflows/perf.yml` — the container-safe gates (bench
  baselines, allocation ceilings, size budget, grep lints) on every push,
  with `bench` results posted to the PR as a diff against the recorded
  baseline file.
- `docs/perf-results.md` extended into the format above: the table, the
  competitor beside it, the margin, the date, the machine.
- `crates/shiv-core/benches/` (and friends) with the criterion suites for
  the O(1)/O(n) claims named above.
- A release checklist item: no tag ships without the hardware-profile
  numbers refreshed in `docs/perf-results.md`.

## The standing question

Every PR ends by answering, in one sentence each: *What did you make
faster? What did you make smaller? What did you prove while doing it?*
A PR that cannot answer stays open until it can.
