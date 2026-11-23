# Release performance results

## Baselines — lea, 2026-11-20 (Nix package, main @ 81ea837)

Machine: NixOS, Linux 7.3.0-rc6-cachyos, 8 cores, 60 GiB RAM, Wayland.
Binary: the Nix flake package (`nix run github:luxus/shiv`), 45 MB store
path including the desktop entry and icon. Gates from
[the proof plan](proof-plan.md).

| Dimension | Measured | Gate | Holds |
|---|---|---|---|
| Idle RSS, catalog loaded (15 s) | 114.5 MB | < 150 MB | yes |
| Idle RSS after 60 s more | 114.5 MB — no growth | leak gate: ±10 % | yes |
| Idle CPU over 60 s | 0 ticks (`time 00:00:00`) | 0 ticks | yes |
| Process start → first activity | warm 172–198 ms, cold 387–529 ms (3 runs each; the earlier ~40 ms probe read RSS past 50 MB before Wayland mapping settled — this run's two-threshold probe replaces it) | < 1.5 s cold, < 600 ms warm | yes, ~3x each |
| Store path size | 45 MB (binary ~34 MiB) | AppImage < 25 MB | yes |
| 60 s full-list scroll (injected 25,000 px/s) | VmRSS 138 MB loaded → VmHWM 151 MB peak (+9.4 %) | post-scroll within 10 % of loaded | yes, 0.6 pp margin |
| Search keystroke → list rebuilt | 3.3–3.4 ms steady, 6.6 ms first letter (11 letters, query `hazeskinmod`) | < 16 ms per keystroke | yes, ~4.8× margin |
| AppImage size (release v0.2.0) | 19.7 MB | < 25 MB | yes |

Commands (lea, one run each):

```sh
nix build github:luxus/shiv --no-link --print-out-paths   # the binary
# idle RSS + CPU: start, `ps -o rss=,time= -p $PID` at 15 s and 60 s
# start probe (warm/cold): poll RSS past 60 MB in a 2 ms loop, print elapsed
#   ms; cold after `sudo sh -c "echo 3 > /proc/sys/vm/drop_caches"`, warm as
#   the re-run
```

The first warm-start probe run measured 5.4 s once and timed out five
times at RSS > 100 MB: a single threshold conflates process start with
catalog load (the network catalog settles below the baseline's 114.5 MB
on quick re-runs — 97–99 MB observed). The recorded probe uses RSS > 60
MB (the app running, Wayland mapped) for both warm and cold; the loaded
state is already covered by the idle-RSS rows above.

Scroll and search run on a release build with `--features dev-probe`
(the probes are committed, `SHIV_SCROLL_SECS` and `SHIV_SEARCH_PROBE`):

```sh
cargo build --release -p shiv-ui --features dev-probe
SHIV_SCROLL_SECS=60 ./target/release/shiv    # then read VmRSS/VmHWM
SHIV_SEARCH_PROBE=hazeskinmod ./target/release/shiv   # SHIV_SEARCH_REFRESH µs per letter
```

A raw cargo build needs the wayland libraries on `LD_LIBRARY_PATH` on
NixOS (`nix path-info nixpkgs#wayland | grep -v -- -man` gives the lib
path); the Nix flake package carries its own rpath.

---

Date: 2026-11-10. Machine: Arch/Omarchy, Hyprland (Wayland). Monitors: DP-1 2560x1440 @ 144 Hz, HDMI-A-1 1920x1080 @ 100 Hz. The Shiv window was tiled at 941x1030 on monitor 0 (HDMI-A-1).

## Binary

`cargo build --release -p shiv-ui`: `target/release/shiv` = 34,870,264 bytes (33.3 MiB); after fix round 1, 34,875,576 bytes.

## Method

- Release builds ignore `SHIV_GAME_ROOT` and `SHIV_SCREEN` (both are `#[cfg(debug_assertions)]`), so a release run would use the real Steam install. For measurement an **uncommitted, since reverted** patch honoured `SHIV_GAME_ROOT` in release and added a 250 ms timer that read commands from `/tmp/shiv-ctl` (`screen N`, `scroll D`, `stop`, `apply`, `undo`), and an 8 ms timer that injected `PointerScrolled` events (delta_y -200 logical px per event, about 25,000 px/s). The real game folder was never touched: `SHIV_GAME_ROOT=/tmp/shiv-fix/Deadlock` (fixture from `SHIV_FIXTURE_DIR=/tmp/shiv-fix cargo test -p shiv-ui write_repair_fixture -- --ignored`), `XDG_CACHE_HOME=/tmp/shiv-cache`.
- Browse used the live catalog (10,949 mods) over the network.
- RSS = `VmRSS` in `/proc/<pid>/status`; CPU = utime+stime ticks from `/proc/<pid>/stat` (100 ticks/s).
- The 250 ms poll timer adds a little idle CPU not present in the committed build.

## Results (single run, in order)

| Step | VmRSS | Notes |
| --- | --- | --- |
| 10 s after start, Browse, thumbnails showing | 84.0 MB | |
| 60 s idle on Browse | 84.0 MB | idle CPU: 8 ticks in 60 s (0.13 %); earlier run 2 ticks in 30 s |
| Scrolling the whole list (60 s, 25,000 px/s) | 138.7 MB | CPU 959 ticks in 60 s (about 16 % of one core) |
| 10 s after scroll stopped | 138.7 MB | not returned to the OS |
| Repair screen shown (analyze, 1 tiny fixture mod) | 313.2 MB | +175 MB |
| After Repair apply | 312.5 MB (peak VmHWM 393.1 MB) | |
| Performance screen | 320.4 MB | idle CPU 2 ticks in 20 s |
| Left Performance | 227.3 MB | Performance's `malloc_trim` released about 93 MB |

Second process, no scrolling: idle 84.0 MB, Repair screen 258.5 MB, left Repair 258.5 MB, Performance 259.6 MB, left Performance 172.2 MB (peak 258.8 MB).

## Fix round 1: Repair memory and Browse creep (2026-11-10)

Cause, measured with a counting global allocator (temporary test in shiv-repair, then the same allocator in the measuring patch): the embedded VData history (`crates/shiv-repair/assets/vdata-history.bin`, 10 MB, legacy format) decoded into **79 MB of live heap**, almost all of it a `BTreeMap<key, BTreeSet<value>>` with 150,846 one-element sets. It sat in a `static OnceLock` for the life of the process, and `with_observed` deep-copied it on every plan build (heap peak 162 MB for one analyze of the tiny fixture). Glibc then kept the worker thread's freed pages: the dynamic mmap/trim thresholds rise after large frees, and `malloc_trim` does not release a thread arena's top.

Fix (`fix(repair): ...`): string bindings are a sorted flat `Vec` (64 bytes a pair), tables and bindings are shared through `Arc` instead of cloned, and the decoded index is held through a `Weak` cache, so it is freed when a check returns (one decode per `analyze`). Engine heap: peak 162 MB -> 21 MB, retained 79 MB -> 0.2 MB. The UI calls `malloc_trim` after each Repair worker and sets `M_MMAP_THRESHOLD`/`M_TRIM_THRESHOLD` to 128 KiB at startup (glibc only).

Method as above (same measuring patch plus a `stat` command that printed live heap; reverted). Fixture and cache under the session scratchpad, never the real game. Before and after were built from the same tree with the same patch and run back to back (VmRSS in MB = kB / 1000).

| Step | Before | After |
| --- | --- | --- |
| Browse idle (30 s) | 80.0 | 73.4 |
| Scrolling 30 s / 60 s | 134.2 / 137.6 | 112.8 / 115.5 |
| 10 s after scroll stopped | 139.0 | 116.1 |
| Repair shown (analyze) | 313.6 | 120.5 |
| After Repair apply (+ re-check) | 313.0 | 120.5 |
| Left Repair (Browse) | 313.0 | 120.5 |
| Performance screen | 320.9 | 125.6 |
| Left Performance | 227.8 | 121.8 |
| Peak VmHWM | 393.9 | 147.5 |

Fresh process, no scrolling: before idle 80.3, Repair 254.9, after apply 255.1, peak 256.2; after idle 73.4, Repair 84.8, after apply 85.1, left Performance 86.3, peak 104.2.

With only the repair-engine fix and `malloc_trim` (no `mallopt`), the scroll run still reached 153.3 MB on Repair (peak 183.4) and the no-scroll run kept 13 MB after apply in a thread arena; the fixed thresholds removed both.

Browse scroll creep: live heap goes 24.7 -> 50.3 MB while scrolling and then stays flat; that is the thumbnail LRU filling its 24 MiB budget (bounded, as designed). The rest of the old growth (139 MB RSS against 50 MB live heap) was allocator retention in the four thumbnail worker arenas plus file-backed pages for the image decoders (RssFile +8.6 MB); the fixed thresholds cut it by about 23 MB. RSS still rises about 3 MB between 30 s and 60 s of scrolling (fragmentation); no unbounded structure was found.

### Fix round 2: final tree (`df9a76a`) and scroll CPU

Same measuring patch (reverted afterwards), extended to read utime+stime over the 60 s scroll. Two runs each, alternating, same session; "before" is the pre-fix binary (no allocator thresholds), "final" is the committed tree with the thresholds and the last `record_snapshot` change.

| | Before (run 1 / run 2) | Final (run 1 / run 2) |
| --- | --- | --- |
| CPU ticks, 60 s scroll | 1034 / 991 (mean 1012, 16.9 % of a core) | 1078 / 1062 (mean 1070, 17.8 % of a core) |
| Idle CPU ticks, 20 s | 1 / 1 | 1 / 1 |
| 10 s after scroll | 139.0 / 136.0 MB | 115.6 / 114.5 MB |
| Repair shown | 310.6 MB (run 2) | 120.5 / 120.2 MB |
| After apply | 393.7 MB (run 2) | 120.5 / 120.3 MB |
| Left Performance | 220.6 MB (run 2) | 121.4 / 121.1 MB |
| Peak VmHWM | 393.7 MB | 146.8 / 145.7 MB |

Scroll CPU is about 6 % higher with the fixed thresholds (about +0.9 points of one core), consistent across both pairs; the 959 ticks in the first table came from an earlier session and are not directly comparable. The thresholds were not changed.

## Verdict against the 150 MB target

- Browse idle: 73 MB, pass. Browse after scrolling the full list: 116 MB, pass (was 139 MB).
- Repair (tiny fixture): pass after fix round 1. Shown and applied stay at about 120.5 MB after a full scroll (85 MB in a fresh process); peak 145.7-147.5 MB across three runs, just under the target, reached only when a Repair check runs on top of a full thumbnail cache. Before the fix: FAIL (313 MB, peak 394 MB).
- Thumbnails: placeholders show while scrolling at 25,000 px/s; they fill in when scrolling stops (LRU cache is bounded).

## Checks

- `cargo fmt --all --check`: clean (after formatting `thumbs.rs` and the kv-parser fixes).
- `cargo clippy --workspace --all-targets -- -D warnings`: clean (after the mechanical kv-parser fixes).
- `timeout 900 cargo test --workspace`: all pass (kv-parser 74+8+2+7, shiv-api 5+13, shiv-core 18+17+14+15+22+11+7, shiv-repair 228 (30 ignored), shiv-ui 25 (1 ignored), source2-model 86 (4 ignored), vpk-parser 2+1, vpkmanager 65).

## Final fix wave: `panic = "unwind"` (2026-11-10)

Release switched from `panic = "abort"` to `panic = "unwind"` so worker panics are caught (`spawn_worker`). Same `cargo build --release -p shiv-ui`, tree at the fix-wave head (also includes the stale-package warnings and Remove repair package):

- Binary: 37,373,240 bytes (35.6 MiB), was 34,875,576 (33.3 MiB): +2.5 MB (+7.2 %), mostly unwind tables and landing pads.
- RSS: one run of the committed release binary, no measuring patch, `HOME`/`XDG_CACHE_HOME`/`XDG_CONFIG_HOME` pointed at an empty scratch folder so no Steam or game folder is found (the real install was never read), live catalog: Browse 10 s and 30 s after start VmRSS 75.6 MB, VmHWM 79.2 MB. Earlier idle Browse figures: 73.4 MB (fix round 1), 84.0 MB (first run); within run-to-run noise, no measurable cost. Repair and scroll were not re-measured.

## Unverified

- Scroll feel on both monitors; X11.
- RSS with a real, full Deadlock install in Repair (the per-check transient grows with the game VPKs and the mods checked; the 79 MB history no longer stays resident).
- Real downloads/installs during the run (install path not exercised here).

## Bench baselines (criterion) — 2026-11-20, the dev workstation (issue #4)

Machine: Apple M1 Pro (arm64, 10 cores), 16 GB RAM, macOS 27.3, APFS
system disk. **Not lea** — these numbers hold on this machine only; lea
records its own when the suites run there. Benches build with the
workspace `bench` profile (inherits `release`: LTO, codegen-units 1).

Commands (second consecutive run of each; the first run after a build
measures up to ~1.8× higher on reorder — cold tempdirs — and is not
representative):

```sh
cargo bench -p shiv-core --bench reorder -- --warm-up-time 1 --measurement-time 2
cargo bench -p shiv-api --bench catalog -- --warm-up-time 1 --measurement-time 3
```

### reorder (`shiv-core/benches/reorder.rs`)

`move_mod` moves the last mod to the top; `set_pak` gives the last mod
pak 1. Both are the worst case: every VPK file renames and every
sidecar rewrites. Sizes are **25/50/99 movable mods, not the
100/500/1000 of the issue**: the engine mounts at most 99 paks per
search path (`addons::PAK_CAPACITY`) and every movable mod needs its
own `pakNN_dir.vpk`, so a 100th movable mod cannot exist — install
#100 is refused with `AddonsFull`. 99 is the full-capacity worst case.

| Bench | 25 | 50 | 99 |
|---|---|---|---|
| move_mod (worst case) | 7.34 ms | 14.89 ms | 30.28 ms |
| move_mod per mod | 294 µs | 298 µs | 306 µs |
| set_pak (worst case) | 7.42 ms | 14.86 ms | 30.51 ms |
| set_pak per mod | 297 µs | 297 µs | 308 µs |

Per-mod cost is flat: 4× the mods for +4 % per mod (move_mod), +3 %
(set_pak). The claim "O(n log n), not O(n²)" holds over the whole
realizable domain (1..=99); at these sizes the cost is file metadata
I/O (two rename phases plus sidecar rewrites), and the O(n²) scans
(`apply_moves`' collision check, `apply_plan`'s name lookup) are noise
beside it.

`group` (the list build the UI runs before every move) covers the
issue's larger sizes honestly: the dir holds the 99 movable mods at
pak capacity plus hand-added VPKs inside managed mods, to `n` list
rows.

| Bench | 100 | 500 | 1000 |
|---|---|---|---|
| group | 1.73 ms | 9.72 ms | 20.93 ms |
| group per mod | 17.3 µs | 19.4 µs | 20.9 µs |

10× the mods for +21 % per mod: flat within the O(n log n) budget
(a pure log factor allows +50 % over that range; the sidecar parse
and `read_dir` dominate).

### catalog (`shiv-api/benches/catalog.rs`)

`list_mods` over a 10,000-row synthetic catalog (deterministic LCG,
realistic field mix, unknown fields present), 7,798,180 bytes of JSON
served from a temp file by a loopback `tiny_http` stub — the app's
real path: HTTP body read plus one-pass serde parse plus the skip-row
filter. Two consecutive runs:

| Run | Median | Throughput |
|---|---|---|
| 1 | 31.0 ms | 239 MiB/s (251 MB/s) |
| 2 | 35.3 ms | 211 MiB/s (221 MB/s) |

Baseline to hold: **≥ 200 MiB/s** on this machine (the ~12 %
run-to-run spread is loopback scheduling, not parse).

### Pinned by tests (assertions, not numbers)

- `install_writes_each_archive_byte_once` (shiv-core tests/install.rs):
  the install path's own `disk_bytes_written` counter equals the
  archive's VPK content exactly — staging extraction writes each byte
  once, `move_into_place` renames, a copy+recopy would count double and
  fail. `install_writes_are_counted_and_placement_renames` scans
  src/install.rs so a new write site outside `tally_disk_write` or a
  copy in the placement fails loudly.
- `a_large_source_lands_in_the_cache_at_display_bytes` (shiv-ui
  thumbs.rs): a 2000×1500 source decodes to 240×135 (fill) / 720×540
  (fit) RGBA — `width × height × 4` bytes in the cache, never the
  12 MB source bitmap.
- `importing_a_multi_mod_bundle_stays_within_its_largest_vpk`
  (shiv-core tests/interchange_import_peak.rs): a 90-mod bundle with a
  12 MiB largest VPK imports and adopts with total allocation under
  1.2× that VPK — the manifest is the only whole-file read, hashing is
  chunked, nothing re-buffers a VPK. Numbers below.
- `progress_storm_flow` (shiv-ui downloads.rs): 100 progress ticks
  against a 30-row model are exactly 100 `row_changed` notifications
  and 0 model resets — a per-tick `set_vec` rebuild shows as 100 resets
  and 3000 row builds and fails. Numbers below.

The `perf.yml` job runs these gates and the two criterion suites on
every push; the bench diff it posts is graded by the policy below.

### CI regression policy (issue #11)

Every push (and dispatch) benches **two arms of one runner** and posts
the diff to the run's step summary. A baseline persisted across runs —
an `actions/cache` of `target/criterion`, or a scheduled artifact —
would grade runner luck, not the push: GitHub's `ubuntu-latest` pool
spans runner generations, and the logs of four green runs over
identical bench code show `move_mod/25` anywhere between 860 µs and
7.65 ms (9×) depending on which runner picked up the job. So each run
resolves the push's comparison base (`github.event.before`; the parent
commit when dispatched), checks it out, benches it with
`--save-baseline push`, re-checks out the pushed tree and benches it
with `--baseline-lenient push` — both arms back to back on the same
machine, the A/B method the tables above were recorded with — and
`scripts/bench_diff_summary.py` renders criterion's change lines into
the summary table (per bench: base and push point estimates, the
change, criterion's verdict).

The gate **fails** a push only when criterion marks a bench
"Performance has regressed." *and* the change point estimate exceeds
**+25%**; every other movement reports. The threshold is calibrated by
same-code runs: the no-op dispatch
[44901062246](https://github.com/luxus/shiv/actions/runs/44901062246)
drew a significant noise verdict of +10.3% (`set_pak/50`) and
non-significant spikes to +46.8% (`set_pak/25`, CI
[−2.7% +46.8% +153.9%]); the gate's own landing push
[44902389879](https://github.com/luxus/shiv/actions/runs/44902389879)
drew a significant +16.9% (`move_mod/25`). Requiring significance
absorbs the wide-CI spikes, and +25% clears every significant noise
verdict observed. Demonstrated once in the throwaway run
[44901820732](https://github.com/luxus/shiv/actions/runs/44901820732)
(branch since deleted): a commit that made `group` parse every sidecar
twice moved the three `group` benches +79–92% (all "Performance has
regressed.") and failed the job, while the same run's noise — +36.5%
on `move_mod/50` (not significant), +20.5% on `set_pak/25`
(significant, under the line) — passed. A failure that looks like
noise is re-dispatched by hand. The absolute baselines stay with these
tables on the machines that recorded them; CI grades each push's
direction, not the absolute number.

## Allocation ceilings — 2026-11-21, the dev workstation (issue #6)

Machine: Apple M1 Pro (arm64, 10 cores), 16 GB RAM, macOS 27.3 — the
same machine as the criterion baselines above, **not lea**; lea records
its own when the suite runs there. Profile: debug, the profile
`cargo test` (and so CI) runs. Harness:
`shiv_core::alloc_count::CountingAlloc`, declared `#[global_allocator]`
in `shiv-core/tests/alloc_ceilings.rs`, counts **total bytes handed
out** (frees are not subtracted: free timing varies between runs,
totals do not). Each flow runs once unmeasured — a warmup priming lazy
statics, so the ceiling measures the flow, not first-time setup — then
three measured runs on fresh fixtures; the worst run is compared.
Flows and fixtures follow the flow tests (tests/local.rs,
tests/sidecar.rs, tests/interchange.rs), not synthetic loops. Catalog
parse stays with the criterion baseline above (issue #4, shiv-api).

Command (the numbers print only on failure; rerun with a lowered const
to observe):

```sh
cargo test -p shiv-core --test alloc_ceilings
```

| Flow | Fixture | Worst of 3 | Ceiling |
|---|---|---|---|
| `install_local_with` | one-VPK zip (18-byte VPK; includes 2 × 256 KiB extraction buffers added by the install probe fix below) | 594,965 B | 620,288 B |
| `set_pak` | 25 movable mods, last mod → pak 1 (worst case: every file renames, every sidecar rewrites) | 57,957 B | 80,000 B |
| `interchange::export` | 3-mod library, one mod disabled | 26,162 B | 36,000 B |
| `interchange::import` | the exported 3-mod bundle into a fresh library | 53,464 B | 72,000 B |

Run-to-run spread on this machine: ±14 B on the install flow, the other
three byte-identical across runs. The ~35 % slack covers cross-machine
fixture variance — temp-path lengths differ (`/tmp` on CI against a
~50-char macOS `TMPDIR`; every path-derived allocation pays its length)
— and small toolchain drift. A regression beyond the slack fails
`cargo test --workspace` in CI with the observed number in the message;
the author removes the allocation or raises the ceiling with a written
reason (what the bytes are, why the flow needs them).

## Interchange import peak — 2026-11-21, the dev workstation (issue #4)

Machine and profile as the allocation ceilings above (Apple M1 Pro,
macOS, debug; **not lea**). Harness:
`shiv-core/tests/interchange_import_peak.rs`, the same
`CountingAlloc` as `tests/alloc_ceilings.rs` standing in for peak RSS —
total bytes handed out during the import, worst of three runs on fresh
libraries after one unmeasured warmup. Fixture: a synthetic 90-mod
bundle (hand-written manifest, real VPKs, distinct sizes), the last mod
carrying a 12 MiB VPK; 90 is near the realizable domain limit for a
fresh import (every imported VPK takes a pak slot, at most 99 mount per
search path — the bound the reorder bench records). The gate: worst
total ≤ 1.2× the largest VPK's size on disk (12,582,986 B, limit
15,099,583 B).

| Flow | Worst of 3 | Ratio to largest VPK |
|---|---|---|
| fresh import (copy into staging, rename into place) | 5,042,740 B | 0.40× |
| adoption in place (size-then-hash match, chunked SHA-256) | 7,206,852 B | 0.57× |

The adoption number is after a fix the gate forced: `adopt_or_install`
re-ran `sidecar::group` on the whole library after every adopted mod to
find the files it had just claimed — quadratic sidecar parsing, 26.4 MB
on the first 8 MiB fixture (3.1× its largest VPK; 2.1× the shipped
12 MiB one). Adopting writes a sidecar only, so the files are the ones
the listing already read; the loop now walks them directly and the
total dropped 3.7×. A single re-buffered VPK (the failure shape the
gate exists for) would add the full 12 MiB and land past 1.5×. The
remaining per-mod cost — `addons::list` and the by-size map are rebuilt
per mod — is bounded by the same pak capacity and stays well under the
gate.

## Downloads progress storm — 2026-11-21, the dev workstation (issue #4)

The finding: `Downloads::sync()` rebuilt the whole `VecModel` on every
event — `set_vec` is a model reset, so each progress tick re-read all 30
rows. The model exists for the row update; `progress` now updates the
one row it moved (`set_row_data`, the pattern `installed.rs`'s
`transfer_changed` uses), and the whole list is replaced only when
membership changes (a transfer appears, ends or leaves the history
bound). Pinned by `downloads::storm::progress_storm_flow` (run from the
UI harness): 30 rows, one download reporting one update per percent,
100 ticks.

| Path (100 ticks, 30 rows) | row_changed | model resets | row builds |
|---|---|---|---|
| before (set_vec per tick) | 0 | 100 | 3000 |
| after (set_row_data per tick) | 100 | 0 | 0 |

Per tick: 1 row update instead of a reset rebuilding all 30 — O(1), not
O(rows). The counts are exact (a counting sink behind the same
`TransferRows` trait the Slint model implements), so the gate cannot
flake; phase changes still update one row, and only a membership change
resets the model.

## Scroll-peak experiments (lea, 2026-11-21)

The scroll gate held with 0.6 pp margin (138 loaded → 151 peak, +9.4 %).
The obvious lever — the 24 MiB thumbnail cache budget — was A/B-tested at
16 MiB: loaded 138.9 MB, peak 154.9 MB. **No improvement**; the peak comes
from the scroll rendering itself (decode intermediates, layout), not the
cache's standing bytes, and the run-to-run spread (~4 MB in fetch order)
is wider than the budget's effect. The budget stays at 24 MiB; a smaller
cache would only re-decode more thumbnails while scrolling.

Commands: the scroll probe (`SHIV_SCROLL_SECS=60`, dev-probe build) over
the live 10k catalog, `VmRSS`/`VmHWM` read from `/proc/<pid>/status`,
budget edited in `crates/shiv-ui/src/browse.rs` (`THUMB_BUDGET`) between
runs.

## Profile experiments (issue #8) — 2026-11-21, the dev workstation

Machine: emily, the same one as the bench baselines above (Apple M1 Pro,
arm64, 10 cores, 16 GB RAM, macOS 27.3, APFS system disk; Apple `cc`
drives the final link, as `.cargo/config.toml` pins). **Not lea** — the
Linux AppImage gate records its own row when issue #1 runs there. Tree:
main @ `ae0068f`, measured in a scratch worktree pinned to a clean
checkout so no in-flight edits could mix into the builds; each experiment
edited only `[profile.release]` keys in `Cargo.toml` and restored them
with `git checkout` afterwards (every restore rebuilt the byte-identical
reference binary). `CC`/`CXX` pointed at `/usr/bin/cc` for every command,
as the README requires on macOS.

Reference row — the committed profile (fat LTO, opt-level 3,
codegen-units 1, `panic = "unwind"`, `strip = true`):

| | Value |
|---|---|
| `cargo build --release -p shiv-ui`, fresh target dir | 351.6 s wall |
| Binary size | 33,262,176 bytes (31.7 MiB) |
| sha256 | `f9203fd8722067ebb5becc30b367d1038406591bbfd3e823cd119b249e79b1a1` |
| `nm` entries on the binary | 380 total: 1 defined (`__mh_execute_header`), 379 undefined dylib imports |

Method notes:

- Sizes compare; a sha256 identifies. Apple `ld` stamps a fresh `LC_UUID`
  into every link, so two links of the same clean tree and profile (main
  tree at `1c08c38`, twice) came out equal in size and different in
  sha256. A restored profile rebuilds the byte-identical artifact because
  cargo serves it from the fingerprint cache instead of relinking.
- Link time = `touch crates/shiv-ui/src/main.rs` + a timed
  `cargo build --release -p shiv-ui`, two runs per arm against the same
  warm dep cache, with the output checked to recompile `shiv-ui` only.
  Under fat LTO this step carries the whole-program codegen, so it is the
  honest per-change build cost.
- Cold start = the probe from the 2026-11-10 method (process start → RSS
  past 50 MB, `ps` every 10 ms), five runs per arm, with `HOME`,
  `XDG_CACHE_HOME` and `XDG_CONFIG_HOME` pointed at a scratch folder and
  `SHIV_API_URL` at an unreachable port, so no Steam, game folder or
  network is touched. Only the back-to-back alternating rounds below
  compare: the probe's absolute median drifted between 157 and 204 ms
  over the session (machine state, not binary).

### `panic = "abort"` (measured only; the committed key stays unwind)

| | unwind (committed) | abort | Delta |
|---|---|---|---|
| Binary size | 33,262,176 | 29,708,672 | −3,553,504 (−10.7 %) |
| Cold start, median of 5, round 1 / 2 | 201.5 / 202.1 ms | 196.8 / 195.1 ms | ~6 ms, below the 40–60 ms run-to-run spread |
| A worker panics | reported back as `Err`, app keeps running | SIGABRT, the whole app dies | the design cost |

The worker-panic row is `a_panicking_worker_still_reports_back` played
out in release: a standalone program shaped like `spawn_worker` (named
thread, `catch_unwind`, report back to the main thread), built with the
same Apple-cc link the repo pins. Under unwind the panic reaches the
caller as `Err` and the process exits 0. Under abort the panic hook
prints and the process dies with SIGABRT (exit 134) before the
report-back runs — for Shiv, any worker panic would kill the app instead
of showing "Shiv hit an internal error and stopped this task". The abort
binary itself starts normally (the cold-start probe ran on it, 15/15
starts).

Found while building that demo: the macOS linker pin is load-bearing for
the unwind design. The same program linked with the nix GCC `cc`
(GCC 16.2.0, first on `PATH` outside the repo config) instead of Apple
`cc` fails to unwind at all — every panic dies with `fatal runtime
error: failed to initiate panic, error 5` — so without
`.cargo/config.toml` pinning `/usr/bin/cc`, release builds already
behave like panic=abort, only with a worse message.

### LTO: fat (committed) vs thin

| | fat (`lto = true`) | thin (`lto = "thin"`) | Delta |
|---|---|---|---|
| Binary size | 33,262,176 | 35,305,024 | +2,042,848 (+6.1 %) |
| Rebuild after touching `main.rs`, 2 runs | 211.4 / 215.4 s | 136.3 / 141.3 s | thin ~75 s faster (−35 %) |
| Full rebuild, one run each | 351.6 s (fresh target dir) | 222.4 s (warm caches) | not comparable: different cache states |

### `strip = true` (committed) vs symbols kept

| | `strip = true` | no strip key (cargo's release default keeps the symbol table) | Delta |
|---|---|---|---|
| Binary size | 33,262,176 | 37,408,144 | +4,145,968 (+12.5 %) to keep symbols |
| `nm` entries | 380 (1 defined) | 30,858 (30,419 defined) | the symbol table is what strip removes |
| `RUST_BACKTRACE=1` on a panic | `stack backtrace:` with nothing resolvable | named frames (`__rustc::rust_begin_unwind`, `core::panicking::panic_fmt`, …) | the crash-report loss |

The backtrace row was demonstrated on the standalone panic program built
both ways. What the release path actually loses is only the frame names:
the panic hook (`log.rs`) records the panic text and `file:line:col`,
both of which travel in the panic payload rather than the symbol table,
so the log line survives stripping.

### `-Z build-std` with `panic_immediate_abort` — not measured, nightly-gated

No nightly toolchain exists on this machine: `rustup` is not installed
(`rustup: command not found`), and the quick path the issue named,
`nix shell nixpkgs#rustc-nightly`, does not exist for aarch64-darwin
(`flake 'flake:nixpkgs' does not provide attribute … 'rustc-nightly'`);
the only toolchains present are the stable 1.99.0 system cargo and the
flake's pinned 1.99.1. Next step, if it ever earns one: a scratch flake
reusing the repo's locked rust-overlay input to expose
`rust-bin.nightly.latest.aarch64-apple-darwin` with the `rust-src`
extension, then `cargo +nightly build -Z build-std=std,panic_abort
-Z build-std-features=panic_immediate_abort`. Bounded before it runs: it
requires panic=abort, so it inherits the worker-panic cost above, and
its size win is bounded by the abort arm's −3.55 MB — on top of plain
abort it removes only the panic-messaging machinery (expected a few
hundred KB; estimate, not a measurement).

### Verdicts

- **`panic` stays `"unwind"`.** Abort wins 3.55 MB (10.7 %) and nothing
  else — cold start is a tie — while turning every worker panic into a
  whole-app crash that never shows the error message the `spawn_worker`
  contract promises. Adopting it means redesigning worker isolation first
  (out-of-process workers, or proof that no worker body can panic) and
  re-measuring; that is a separate decision.
- **LTO stays fat.** Thin links ~75 s faster per change but ships
  2.04 MB more; the recorded gate is binary size, and link time is a
  developer-only cost.
- **`strip = true` stays.** Keeping symbols costs 4.15 MB (+12.5 %) and
  nothing in the release path needs them; only `RUST_BACKTRACE` frame
  names are lost, and the panic log line carries its own location.
- **`build-std`/`panic_immediate_abort`: not measured, nightly-gated**
  (no nightly on the machine). It is bounded by the abort arm's win and
  cost, so it only matters if the abort question reopens.

## Install probe (lea, 2026-11-21)

Machine: lea (NixOS, Linux 7.3.0-rc6-cachyos, 8 cores, 60 GiB RAM, root
on a WD_BLACK SN850X NVMe), release profile, tree at the issue #10 commit.
The probe is committed: `shiv-core/tests/install_probe.rs`, an `#[ignore]d`
test that builds its fixture, measures, prints, and asserts the
invariants the number stands on — the mod lands whole (dir vpk plus every
chunk in `addons`), every archive byte is written to disk exactly once,
and the fixture stays incompressible so the read baseline and the
extraction move the same bytes. Timing asserts are absent on purpose:
wall-clock asserts flake, and the numbers live here with the machine and
date. Run on the machine being recorded:

```sh
cargo test --release -p shiv-core --test install_probe -- --ignored --nocapture
cargo test --release -p shiv-api --test client cancelled_fetch_returns_within_one_chunk -- --ignored --nocapture
```

The fixture: one mod shaped like a large real one — a small
`pak01_dir.vpk` plus its 20 `pak01_<digits>.vpk` chunk VPKs, real VPKs
packed by vpkmanager, 104,924,900 bytes of pseudorandom content (deflate
degenerates to stored blocks, so the 104,959,189-byte archive is the size
it extracts) — zipped as the catalog serves zips. Both sides run
page-cache warm, the state the real flow is in: a catalog install
extracts the archive the download just wrote. The download itself is
network and is not measured; the running-game check is injected as
`false` (a process-table walk is not archive work).

### Extraction throughput against the raw read

| Path (100 MiB of VPKs) | Median of 5 | Throughput |
|---|---|---|
| `std::fs::read` of the archive | 6–7 ms | 14,600–15,600 MB/s |
| `install_local_with`, before the fix | 24 ms | 4,328 MB/s |
| `install_local_with`, after the fix | 13–14 ms | 7,399–7,606 MB/s |

Against the warm `fs::read` baseline the extraction runs at
**50.7–50.9 %** — below the 60 % gate, and at the ceiling that reading
allows: an install writes what it reads, so a zero-overhead copy can
never pass ~50 % of a pure read (read + write is twice the traffic, with
CRC32 and stored-block inflate on top). Against the disk's actual read
speed — cold, caches dropped (`sudo sh -c 'echo 3 >
/proc/sys/vm/drop_caches'`, then `dd` of a 100 MiB random file on the
same filesystem: 6.6–6.9 GB/s, two runs) — the warm extraction runs at
**~110 % of disk-read speed**: on this machine the install path is not
the bottleneck, the disk is.

### The fix the probe forced

`write_entry` handed the archive readers' output straight to `io::copy`,
which writes what it gets — and the zip entry reader returns 8 KiB per
`read` call (read-size histogram on the dev workstation for this
fixture: 6,400 reads of 8,192 bytes and 6,400 of 8,187; the 8 KiB
`BufReader` sits inside the zip crate and cannot be resized through its
API). A 100 MB archive paid ~12,800 small file writes. The fix:
`write_entry` stages reads into a 256 KiB buffer, one per extraction and
reused across entries (`ENTRY_COPY_BYTES`), writing only full buffers,
and `extract_zip` reads through a `BufReader` of the same size. lea:
24 → 13–14 ms (1.7×); the dev workstation: 102 → 40–51 ms (2.2×).
Buffer-size sweep on the dev workstation: 8 KiB 100 ms, 64 KiB 47 ms,
128 KiB 46 ms, 256 KiB 40–41 ms, 1 MiB 47 ms — 256 KiB is the knee.

The `install_local_with` allocation ceiling rose with the written reason
(`tests/alloc_ceilings.rs`): +524,288 B, exactly the two 256 KiB
extraction buffers. The single-write invariant
(`install_writes_each_archive_byte_once`) still pins each archive byte
written exactly once.

### Cancel

- **Extraction is not cancellable at all — a finding, not a number.**
  `run_install` (shiv-ui `browse.rs`) checks the flag before the
  download, after it, and after taking the install lock, but
  `install_with_limits`/`extract` have no flag check inside the
  extraction loop: a Cancel pressed during extraction changes nothing
  until the install finishes, and a finished install is kept ("a fact
  about the disk even if the job was cancelled late", `job_update`). The
  Downloads row itself flips in the same event-loop turn as the click —
  `cancel()` removes the job and reports `Outcome::Cancelled`
  synchronously — so the observable latency is 0 by construction, not by
  measurement. Cancellable extraction is its own issue if the maintainer
  wants it: the loop would check a flag between entries (staging cleanup
  already exists).
- **The download phase is cancellable and measured.**
  `fetch_cancellable` checks the flag before every 64 KiB chunk. The
  probe (`shiv-api/tests/client.rs`, the same ignored print-only
  pattern) sets the flag after the first chunk arrives and times the
  return: lea **88 µs**, dev workstation 328 µs (loopback stub, 64 MiB
  body). On a real network the return is bounded by the in-flight
  chunk's arrival, not by the check.

### Verdict

- Extraction gate (> 60 % of disk-read speed): **50.7–50.9 % of the warm
  raw read; ~110 % of the cold disk read.** The warm-`fs::read` reading
  is structurally capped at ~50 % for any implementation that writes
  what it reads; the cold-read reading passes with margin. Recorded
  recommendation: the gate's baseline is the disk's read speed (cold),
  with the warm comparison kept as the copy-ceiling context.
- Cancel gate (< 50 ms): holds where cancellation exists (88 µs loopback
  floor on lea; the row flip is synchronous); unsatisfiable for
  extraction as the code stands, because extraction has no cancellation.
  Re-scoped to its own issue unless the maintainer prefers to build
  cancellable extraction now.
