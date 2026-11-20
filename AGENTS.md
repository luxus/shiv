# AGENTS.md — how to work in this repository

This file is the entry point for any LLM (or human) picking up work here.
Everything below is enforceable truth; nothing else is.

## Read these first, in order

1. **`docs/proof-plan.md`** — the law of the repository: every decision
   needs its measurement, every measurement lives here, the target is a
   margin, not a tie. Includes the review protocol (naming pass,
   zero-comment pass, outcome pass, proof pass, rules pass) and the
   standing PR question.
2. **`docs/code-quality.md`** — the lint and test rules, each traced to
   the bug that created it. No `unwrap`/`expect` outside tests without a
   scoped, reasoned allow. Tests assert invariants, never hand-computed
   examples.
3. **`docs/perf-results.md`** — the recorded baselines (lea, 2026-11-20)
   and the gates they hold. A PR that worsens a recorded number without
   a written reason fails.

## Where the work is

The open work is on the GitHub issues, ordered by leverage:

- **#1** — record the remaining baselines on lea (scroll, keystroke, sizes)
- **#2** — dependency audit: each dependency proves itself or leaves
- **#3** — pedantic pass: every line of first-party code is the example
- **#4** — criterion suites turning the plan's O-claims into CI gates
- **#5** — event-loop and worker guards as tests, not review hopes
- **#6** — allocation ceilings per user flow
- **#7** — README states the mission and the measured numbers
- **#8** — panic=abort and profile experiments, answered with numbers

Pick one issue. Do not batch them; each lands as its own PR with its own
proof.

## The non-negotiables

- `cargo test --workspace` green; `cargo clippy --workspace --all-targets
  -- -D warnings` green; `cargo fmt --check` green. Locally, this
  repository builds on Linux (NixOS flake: `nix develop`) and macOS
  (needs `export CC=/usr/bin/cc CXX=/usr/bin/cc`, see README).
- **lea** (`ssh lea.local`) is the Linux reference machine for
  measurements. Numbers go into `docs/perf-results.md` with date,
  machine, method, and the commands used — see the existing tables for
  the format.
- No new inline comments. Clarity lands in names; doc comments state
  contracts on public items only (proof-plan.md, review protocol).
- Every PR ends by answering: *What did you make faster? What did you
  make smaller? What did you prove?*
