# For an LLM agent picking up Shiv

This document is a quick orientation map. The enforceable rules live in
`docs/engineering-core.md` (generic core, `Exemplar` profile), `AGENTS.md`,
`docs/code-quality.md`, and `docs/proof-plan.md`.

## The 30-second orientation

```sh
cat AGENTS.md                                  # rules, crate tiers, reading order
gh issue list --repo luxus/shiv --state open   # current open work
ls docs/                                       # core, proof-plan, code-quality, perf-results
cargo test --workspace                         # must be green before you start
```

## The machines

- **Your workstation**: builds and tests everything. macOS needs
  `export CC=/usr/bin/cc CXX=/usr/bin/cc` first (README explains why).
- **lea** (`ssh lea.local`): the Linux reference machine — NixOS, 8 cores,
  60 GiB, Wayland. All *recorded* hardware performance numbers come from here,
  via `nix build github:luxus/shiv` and the probe commands recorded in
  `docs/perf-results.md`.

## How to finish an issue

1. Re-read the issue body; it names its own acceptance.
2. Make the change in the smallest honest PR.
3. Prove it: the regression/invariant test, guard, benchmark, or recorded
   number — committed in the PR, not only in the description.
4. Answer the PR standing questions (*What did you prove?* + *faster/smaller?*).
5. Close the issue only when its checkboxes are true.

## Where things live

| Thing | Place |
| --- | --- |
| Reusable generic rule core & profiles | `docs/engineering-core.md` |
| Project entry point & crate tiers | `AGENTS.md` |
| Performance gates & proof obligations | `docs/proof-plan.md` |
| Bug-to-enforcer quality rules | `docs/code-quality.md` |
| Recorded baselines, method, commands | `docs/perf-results.md` |
| Open issues | `gh issue list --repo luxus/shiv --state open` |
| Nix package and dev shell | `flake.nix`, `rust-toolchain.toml` |
| Interchange and registry protocols | `crates/shiv-core/src/interchange.rs`, `crates/shiv-api/src/native.rs` |
| UI controllers & threading guards | `crates/shiv-ui/src/*.rs`, `crates/shiv-ui/tests/guards.rs` |
| UI test harness | `crates/shiv-ui/src/main.rs` (`tests::`, `run_until`) |
