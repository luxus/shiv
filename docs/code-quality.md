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

`install::INSTALL_LOCK` (in `shiv-core`, beside the install code that needs
it) serialises every path that allocates pak numbers or renames
`pakNN_*` files: catalog installs, file and link installs, interchange
imports, and the load-order moves (`set_pak`, `move_mod`) from the UI.
`install_with` deliberately does **not** take it itself — its callers hold
it and rely on a cancel-check right after acquiring — so the contract
lives on the static's documentation. A new mutating path that skips the
lock races the others for pak slots; the link-install stall test in
`shiv-ui` (`cancel_while_installing_then_retry_installs_once`) exercises
the ordering and would catch a dropped lock in the paths it covers.

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

## A second pair of eyes for every feature

Every feature PR in this repository gets the same treatment as
`f8714d5..HEAD` did: two review passes (core logic, UI/worker races), each
finding converted into a fix, a regression test, or both. The point is not
the review; it is that the review's findings become rules, like the ones
above, so the same class of mistake cannot ship a second time.
