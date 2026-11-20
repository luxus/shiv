# For an LLM agent picking up Shiv

You are continuing a codebase that is mid-transformation: the features
are built and shipping (v0.2.0), and the current phase is *proof* —
turning every performance and quality claim into a measured gate that
CI enforces. This document is your map.

## The 30-second orientation

```sh
cat AGENTS.md                       # the rules and the reading order
gh issue list --repo luxus/shiv     # the open work, ordered by leverage
ls docs/                            # proof-plan, code-quality, perf-results
cargo test --workspace              # must be green before you start
```

## The rules, compressed

- **Decisions carry measurements.** docs/proof-plan.md is the law; if you
  cannot name the alternative you rejected and the number that rejected
  it, the decision is not made.
- **No panics in the libraries.** `deny(clippy::unwrap_used)` and
  `deny(clippy::expect_used)` are enforced at the crate roots; tests are
  exempt; scoped allows need a `reason =`.
- **Tests pin invariants, not examples.** Look at
  `set_pak_invariants_hold_across_random_configurations` in
  `crates/shiv-core/tests/sidecar.rs` — that is the standard.
- **No inline comments.** A comment is a bug report against a name.
- **The margin is the goal.** Every performance dimension has a gate
  with a recorded number in docs/perf-results.md. You are not done
  until the number is recorded or improved.

## The machines

- **Your workstation**: builds and unit-test everything. macOS needs
  `export CC=/usr/bin/cc CXX=/usr/bin/cc` first (README explains why).
- **lea** (`ssh lea.local`): the Linux reference machine — NixOS, 8
  cores, 60 GiB, Wayland. All *recorded* performance numbers come from
  here, via `nix build github:luxus/shiv` and the probe scripts whose
  results are in docs/perf-results.md. Write probe scripts to
  `/tmp/shiv-*.sh` and their output next to them; record the numbers and
  the commands in the doc, then discard the scripts.

## How to finish an issue

1. Re-read the issue body; it names its own acceptance.
2. Make the change in the smallest honest PR.
3. Prove it: the benchmark, the guard, or the recorded number — in the
   PR, not in the description.
4. Answer the three questions in the PR body (faster? smaller? proved?).
5. Close the issue only when its checkboxes are true.

## Where things live

| Thing | Place |
|---|---|
| The proof law, gates, review protocol | `docs/proof-plan.md` |
| The lint/test rules and their origin bugs | `docs/code-quality.md` |
| Recorded baselines, method, commands | `docs/perf-results.md` |
| The issues (the work queue) | GitHub: luxus/shiv issues #1–#8 |
| The Nix package and dev shell | `flake.nix`, `rust-toolchain.toml` |
| The interchange and registry protocols | `crates/shiv-core/src/interchange.rs`, `crates/shiv-api/src/native.rs` |
| The UI controllers (threading model) | `crates/shiv-ui/src/*.rs` — event loop calls `spawn_worker`; results come back via `upgrade_in_event_loop` |
| The test harness for UI flows | `crates/shiv-ui/src/main.rs`, `tests::` module, `run_until` |

## What "done" looks like for the whole phase

All eight issues closed; `cargo clippy --workspace --all-targets --
-D warnings` green; the pedantic findings at zero in first-party code;
every proof-plan dimension recorded on lea with its gate holding; a
`perf` CI job posting diffs against a baseline file; and the README
stating the mission with numbers a reader can verify.
