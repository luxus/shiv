# AGENTS.md — how to work in this repository

This file is the entry point for any LLM (or human) picking up work here.
Shiv follows **`docs/engineering-core.md` (Profile: `Exemplar`)** plus the
project-specific rules and gates linked below.

## Read these first, in order

1. **`docs/engineering-core.md`** — the reusable rule core (visibility/purpose,
   comments, naming, trust boundaries, fail-closed enforcers, measurements).
2. **`docs/proof-plan.md`** — Shiv's performance gates, proof obligations,
   justified dependencies, and review protocol.
3. **`docs/code-quality.md`** — Shiv's bug-to-enforcer registry (every rule
   traced to the bug that created it and the check that enforces it).
4. **`docs/perf-results.md`** — the recorded baselines and CI regression policy.

## Crate tiers

| Tier | Crates | Standard |
| --- | --- | --- |
| **First-Party** | `shiv-core`, `shiv-api`, `shiv-ui` | Full `Exemplar`: `#![deny(clippy::unwrap_used, clippy::expect_used, clippy::pedantic)]`, strict trust boundaries, minimal `pub` surface |
| **First-Party wrapper + upstream-synced port** | `shiv-repair` (`api.rs`, `gameinfo.rs`, `repair_backup.rs`, `lib.rs` are Shiv-owned; `mod_health.rs`, `vdata_history.rs`, `localization_overlay/` track `docs/upstream.json`) | `#![deny(clippy::unwrap_used, clippy::expect_used)]`, strict trust boundaries, and `CacheScope` idle-released caches; minimize gratuitous diffs in upstream-synced files |
| **Ported / copied support crates** | `kv-parser`, `vpk-parser`, `source2-model`, `vpkmanager` | Workspace `clippy`/`test` green; upstream-tracked via `docs/upstream.json` |

## Where the work is

Check live open issues instead of a static list:

```sh
gh issue list --repo luxus/shiv --state open
```

Pick one issue or one coherent concern per PR, with its own proof.

## Key rules and what enforces them

| Rule | Enforcer |
| --- | --- |
| A new pak number comes from `pak_slot::occupied` / `pak_slot::free_slots`, never from files alone | Private `file_pak_numbers` inside `pak_slot.rs` + `scripts/check_pak_occupancy.py` (`cargo test -p shiv-core --test pak_occupancy`) |
| Every pak rename or slot allocation takes `InstallPermit`, minted only by `InstallGuard` (including `complock::claim_reusable`). `addons::next_pak` is `pub(crate)` | Type system + `crates/shiv-core/tests/install_permit.rs` |
| No blocking I/O on the Slint event loop; workers park on a `Condvar`, never poll | `crates/shiv-ui/tests/guards.rs` |
| No `unwrap`/`expect` in production code of `shiv-core`, `shiv-api`, `shiv-repair`, `shiv-ui` | `#![deny(clippy::unwrap_used, clippy::expect_used)]` at each crate root |
| First-party crates (`shiv-core`, `shiv-api`, `shiv-ui`) stay clean under Clippy's pedantic group | `#![deny(clippy::pedantic)]` at each crate root + `clippy.toml` (`doc-valid-idents`) |
| CI bench diffs fail closed on missing baselines or >25% significant regressions | `.github/workflows/perf.yml` + `scripts/bench_diff_summary.py` |

The full list of rules and the bugs that created them is in `docs/code-quality.md`.

## The non-negotiables

- `cargo test --workspace` green; `cargo clippy --workspace --all-targets -- -D warnings` green; `cargo fmt --all --check` green. Locally, this repository builds on Linux (NixOS flake: `nix develop`) and macOS (needs `export CC=/usr/bin/cc CXX=/usr/bin/cc`, see README).
- **lea** (`ssh lea.local`) is the Linux reference machine for recorded hardware measurements. Numbers go into `docs/perf-results.md` with date, machine, method, and commands.
- No "what" inline comments; inline comments only for non-obvious **why** (1-2 lines). Doc comments state contracts on public items (and private items only when an automated guard reads them, such as `/// Worker body`).
- Every PR ends by answering: *What did you prove?* (mandatory) — plus *What did you make faster?* and *What did you make smaller?* (or an explicit note that existing gates stay unchanged).
