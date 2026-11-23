# How Shiv stays correct

This document is Shiv's project-specific bug-to-enforcer registry on top of
`docs/engineering-core.md` (`Exemplar` profile). The rules below each came out
of a real bug or review finding in this repository. Break one and the build or
the test suite fails — writing the bad version again should never be green.

Every rule names the mechanism that enforces it (a type, a `deny` lint, a
guard, or a test) so a future change that weakens the mechanism is visible in
review.

## No panics in the libraries or on the event loop

`shiv-core`, `shiv-api`, `shiv-repair`, and `shiv-ui` deny `clippy::unwrap_used`
and `clippy::expect_used` at the crate root. A library must not panic on the
user's disk, and a panic on the Slint event loop takes the whole app down
(workers report a panic as a message instead — `spawn_worker`).

The few `expect`s that genuinely cannot fail carry a scoped
`#[allow(clippy::expect_used, reason = "…")]` on a small function that holds
nothing else, e.g. `sidecar::pak_of` (the filter that guarantees it sits two
lines above). Unit tests are exempt through `#![cfg_attr(test, allow(...))]`;
the integration tests in `tests/` are separate crates. Slint's generated
bindings are exempt through the `generated` module in `main.rs`.

If you feel the need for `unwrap`, write the error path — a new error variant
beats a panic on someone's mods folder. Likewise, never index external data
unchecked (`slice[i]` on parsed configs or byte-index `String::truncate` on
UTF-8 text) on the event loop or in workers.

## Pedantic lints are denied at the crate root

`shiv-core`, `shiv-api`, and `shiv-ui` also deny `clippy::pedantic` in
production code (unit tests stay exempt via `#![cfg_attr(test, allow(...))]`,
and Slint's generated bindings via `generated` in `main.rs`). `clippy.toml`
records Shiv's proper domain nouns in `doc-valid-idents` so `clippy::doc_markdown`
keeps flagging un-backticked code and paths without noise on prose. Any rare
exception carries a tightly scoped `#[allow(..., reason = "…")]`.

## Every symbol earns its place

Visibility defaults to the narrowest scope (`private` -> `pub(crate)` -> `pub`).
Internal helpers and modules stay private so the compiler catches dead code;
never widen visibility to silence warnings. Avoid speculative abstractions and
magic integer indices (use typed enums such as Slint's screen enum).

## No "what" comments; "why" only where code cannot carry it

Inline comments that restate what code does, section banners (`// ---`),
commented-out code, and untracked `TODO`s are forbidden. Keep inline comments
strictly to 1–2 lines explaining non-obvious **why** (OS/allocator/protocol
quirks, `SAFETY` invariants, ordering hazards). Doc comments (`///`) state
contracts on public items and on private items only when an automated guard
reads them (e.g., `/// Worker body` in `shiv-ui/tests/guards.rs`).

## Error messages name the actual cause

`SidecarError::PakPush` exists because `BadPak(99)` once reported "99 is
outside 01..=99" for a number that was in range — the real cause was another
VPK being pushed past 99. Similarly, `ureq::Error::Io` (network socket errors)
maps to `ApiError::Network`, never `ApiError::Io` ("file error"). Before
reusing an error variant, ask whether its message could be literally false in
the user's eyes; if it could, add or pick the honest variant. Pinned by
`set_pak_refuses_numbers_that_do_not_fit_and_changes_nothing` and
`transport_errors_are_reported_as_network_not_file_io`.

## Validate before you compute, refuse before you touch

Range and capacity checks run *before* any arithmetic that could overflow
(`set_pak` checks `pak > 99` before `pak + take - 1`), and every refusal
happens *before* the first file is deleted or renamed, so a refused call leaves
every byte untouched. Pinned by
`set_pak_refuses_huge_numbers_without_overflowing` and
`set_pak_invariants_hold_across_random_configurations`.

## Guard every trust boundary

- **External paths & symlinks:** Profile IDs, interchange bundle entries,
  sidecar `clip.pcm` names, CompLock `part.name` values, and repair VPK
  localization paths must be validated against traversal (`..`), absolute/drive
  paths, and symlink escapes before any read, write, or `remove_dir_all`.
- **External URLs & redirects:** HTTP clients disable automatic redirects
  (`.max_redirects(0)`), enforce per-read timeouts (`.timeout_per_read(...)`),
  reject HTTPS->HTTP downgrades, and verify allowed origins on **every redirect
  hop**. `origin_of` / `is_https_or_loopback` reject userinfo (`@`) tricks and
  handle IPv6 `[::1]` authorities properly.
- **Streams & decoders:** Audio decode/resample enforces sample limits and
  validates sample rates *before* allocating; downloads enforce a hard byte cap
  and retry `EINTR`.

## One lock for everything that touches pak numbers

Every function that allocates a pak number or renames a `pakNN` file takes
`install::InstallPermit` (including `complock::claim_reusable`).
`InstallGuard::acquire` holds the lock until drop, and `permit` is the only
constructor, so the call does not compile without it.
`forgotten_pak_change_does_not_compile` pins those calls and verifies that
`addons::next_pak` stays `pub(crate)`. The lock is not reentrant: a cancel
check stays after `acquire`, and the function receives the permit.

## Never combine a serde default with `skip_serializing_if` on a bool

The interchange reader defaults `selected` to `true`, so a writer that skips
the field when false makes every unselected variant come back selected for
another reader. Pinned by `a_false_selected_survives_serialization` in
`tests/interchange.rs`. For defaults that mean "the opposite of what I skip",
always write the field.

## Tests assert invariants, not hand-computed numbers

`set_pak_invariants_hold_across_random_configurations` generates 120 random
configurations and asserts the properties that must hold — uniqueness, the
target block, preserved relative order, sidecar integrity, byte-identical
refusals — instead of examples with numbers computed by hand. Derive expected
values from invariants rather than hand-computed examples.

## Never read UI state across threads or while a worker is changing it

Every Installed-screen action is asynchronous: in tests, `run_until` between
actions (`rows_are`); in production, never chain a read off an in-flight
action. Never read or mutate UI-thread `thread_local!` state from worker
threads (`spawn_worker`'s `work` or `done` closures): snapshot needed data on
the UI thread before spawning and perform all UI-state mutations inside
`upgrade_in_event_loop`. `crates/shiv-ui/tests/guards.rs` enforces that
blocking calls do not run on the event loop and that workers park on a
`Condvar` instead of polling under a lock.

## Pak numbers come from one occupied set

`pak_slot::occupied` is the set of pak numbers that are not free: a file in
`addons` carries the number, or a sidecar names it. `pak_slot::free_slots` is
how a new mod takes one. While a CompLock pak is installed it keeps
`1..=complock::last_slot()` for the pack, and uses a free pack slot only when
nothing above is left. `set_pak` and `renumber` refuse invalid, duplicate, or
stale slots with `SidecarError::BadPak`, `DuplicatePak`, or `StalePak`.

`file_pak_numbers` is private inside `pak_slot.rs` (compile-time boundary),
backed by `scripts/check_pak_occupancy.py`
(`cargo test -p shiv-core --test pak_occupancy`).

## Global caches release when idle

Process-lifetime `OnceLock` caches must either stay within an explicit budget
in `docs/proof-plan.md` or release back to the OS when a batch finishes (`Weak`
or scope-bound). `VdataHistoryIndex::embedded()` uses `Weak`, and
`shiv-repair`'s `compiled_resource`, `observed_data`, and
`animation_contract::cache` caches are released by `CacheScope` when the last
active `analyze` or `apply` call finishes.

## A second pair of eyes for every feature

Every feature PR gets two review passes (core logic / trust boundaries, and
UI/worker races), each finding converted into a fix, a regression/invariant
test, or a compile-time/CI enforcer so the same class of mistake cannot ship a
second time.
