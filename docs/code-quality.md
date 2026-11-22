# How Shiv stays correct

The rules below each came out of a real bug in this repository. Break one and
the build or the test suite fails — that is the point: writing the bad version
again should not be possible, or at least never green.

Every rule names the mechanism that enforces it (a `deny` lint, a test) so a
future change that weakens the mechanism is visible in review.

## No panics in the libraries or on the event loop

`shiv-core`, `shiv-api` and `shiv-ui` deny `clippy::unwrap_used` and
`clippy::expect_used` at the crate root. A library must not panic on the
user's disk, and a panic on the Slint event loop takes the whole app down
(workers report a panic as a message instead — `spawn_worker`).

The few `expect`s that genuinely cannot fail carry a scoped
`#[allow(clippy::expect_used, reason = "…")]` on a small function that holds
nothing else, e.g. `sidecar::pak_of` (the filter that guarantees it sits two
lines above) and `vanilla::mod_mount_pattern` (a fixed regex literal).
Unit tests are exempt through `#![cfg_attr(test, allow(...))]`; the
integration tests in `tests/` are separate crates. Slint's generated
bindings are exempt through the `generated` module in `main.rs`.

If you feel the need for `unwrap`, write the error path — a new error
variant beats a panic on someone's mods folder.

## Error messages name the actual cause

`SidecarError::PakPush` exists because `BadPak(99)` once reported
"99 is outside 01..=99" for a number that was in range — the real cause was
another VPK being pushed past 99, and the misleading text sent the user
hunting for a problem they could not see. Before reusing an error variant,
ask whether its message could be literally false in the user's eyes; if it
could, add the missing variant. Pinned by
`set_pak_refuses_numbers_that_do_not_fit_and_changes_nothing`.

## Validate before you compute, refuse before you touch

Range checks run *before* any arithmetic that could overflow
(`set_pak` checks `pak > 99` before `pak + take - 1`), and every refusal
happens *before* the first file is renamed, so a refused call leaves every
byte untouched. `set_pak_refuses_huge_numbers_without_overflowing` pins the
ordering; the property test below pins the "refused ⇒ disk unchanged"
half.

## One lock for everything that touches pak numbers

Every function that allocates a pak number or renames a `pakNN` file takes
`install::InstallPermit`. `InstallGuard::acquire` holds the lock until
drop, and `permit` is the only constructor, so the call does not compile
without it. That is the interchange import before `8b37a9b`
(`install_files` with no lock) and the edit dialog's `set_pak` before
`6c5beb5`. `forgotten_pak_change_does_not_compile` is those two calls.
The lock is not reentrant: a cancel check stays after `acquire`, and the
function receives the permit.
`cancel_while_installing_then_retry_installs_once` still exercises that
order.

## Never combine a serde default with `skip_serializing_if` on a bool

The interchange reader defaults `selected` to `true`, so a writer that
skips the field when false makes every unselected variant come back
selected for another reader — the field round-trips wrong only for the
value that matters. (This is the general shape of the bug: the default and
the skip condition contradicted each other.) Pinned by
`a_false_selected_survives_serialization` in `tests/interchange.rs`. For
defaults that mean "the opposite of what I skip", always write the field.

## Tests assert invariants, not hand-computed numbers

`set_pak_invariants_hold_across_random_configurations` generates 120
random configurations and asserts the properties that must hold —
uniqueness, the target block, preserved relative order, sidecar integrity,
byte-identical refusals — instead of examples with numbers computed by
hand. The hand-computed ones were wrong twice during development
(a shift amount, a file name that had to exist). When a test needs an
expected value, derive it from the invariant; when the invariant is
checked with a fixed expectation, double-check the expectation against
the implementation's actual contract, not its code.

## Never read UI state that a worker is about to change

Every Installed-screen action is asynchronous: the row data refreshes only
after the worker answers, so reading `row_data(0)` right after invoking an
action sees the *old* world (an uninstalled mod twice, a stale id). In the
test harness, `run_until` between actions; in production code, never chain
a read off an in-flight action. The harness's helpers (`rows_are`) exist
for exactly this.

## Pak numbers come from one occupied set

`pak_slot::occupied` is the set of pak numbers that are not free: a file
in `addons` carries the number, or a sidecar names it. `pak_slot::free_slots`
is how a new mod takes one. While a CompLock pak is installed it keeps
`1..=complock::last_slot()` for the pack, and uses a free pack slot only
when nothing above is left. `set_pak` returns `SidecarError::StalePak`
when a target is in that set and no file in the plan holds it.

`addons::file_pak_numbers` counts files only. A call to it, or to its old
name `used_pak_numbers`, outside `pak_slot.rs` fails
`scripts/check_pak_occupancy.py` (`cargo test -p shiv-core --test pak_occupancy`).
The scan is the Improved Quickbuy loss: a files-only search handed out
pak10 and the stale sidecar took the new file (`fdee4cb`).
`set_pak_onto_a_stale_sidecar_slot_keeps_the_mods_own_record` is that
loss through `set_pak`, which `fdee4cb` left open.
`with_the_pack_in_other_mods_go_above_it_and_only_spill_back_when_full`
is a Browse install taking a pack slot (`e658688`).

## A second pair of eyes for every feature

Every feature PR in this repository gets the same treatment as
`f8714d5..HEAD` did: two review passes (core logic, UI/worker races), each
finding converted into a fix, a regression test, or both. The point is not
the review; it is that the review's findings become rules, like the ones
above, so the same class of mistake cannot ship a second time.
