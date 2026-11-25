<p align="center">
  <img src="style/png/shiv-banner-dark.png" alt="Shiv: Deadlock mod manager. Lean, fast, no bloat. Built in Rust." width="100%">
</p>

<p align="center">
  <a href="LICENSE"><img alt="License: GPL-3.0" src="https://img.shields.io/badge/license-GPL--3.0-c9a84c?labelColor=121110"></a>
  <img alt="Built with Rust" src="https://img.shields.io/badge/built%20with-Rust-c9a84c?labelColor=121110">
  <img alt="UI: Slint" src="https://img.shields.io/badge/UI-Slint-c9a84c?labelColor=121110">
  <img alt="Platform: Linux and Windows" src="https://img.shields.io/badge/platform-Linux%20%7C%20Windows-c9a84c?labelColor=121110">
</p>

**Shiv** is a native mod manager for
[Deadlock](https://store.steampowered.com/app/1422450/Deadlock/) on Linux and
Windows: no webview, no Electron or Tauri layer.

It exists because the standard manager renders its UI through a web engine and
idles in the gigabytes. Shiv does not aim to tie with the webview managers. The
target is the gap, not the tie: a measured margin in idle memory, cold start,
scroll frame budget, install wall clock and binary size. Every dimension has a
gate, every recorded number carries its date, machine and command, and the law
is that a regression which closes the margin fails the build. The gates and the
method are [docs/proof-plan.md](docs/proof-plan.md); the numbers Shiv holds
today are [below](#performance) and in [docs/perf-results.md](docs/perf-results.md).

## Performance

The [proof plan](docs/proof-plan.md#what-best-means-measured) defines eight
measured dimensions. The table lists each gate and the number recorded so
far; the reference machine is **lea** (NixOS, Linux, 8 cores, 60 GiB RAM,
Wayland), the linked run 2026-11-20, the Nix flake package at
`main @ 81ea837`, with the measurement commands recorded beside the results.

| Dimension | Gate | Measured | Run |
|---|---|---|---|
| Memory — idle, catalog loaded | < 150 MB RSS | 114.5 MB | [lea, 2026-11-20](docs/perf-results.md#baselines--lea-2026-11-20-nix-package-main--81ea837) |
| Memory — idle, 60 s later | no leak while idle (±10 %) | 114.5 MB, no growth | [lea, 2026-11-20](docs/perf-results.md#baselines--lea-2026-11-20-nix-package-main--81ea837) |
| Battery — idle CPU | 0 ticks in 10 min | 0 ticks over 60 s | [lea, 2026-11-20](docs/perf-results.md#baselines--lea-2026-11-20-nix-package-main--81ea837) |
| Start-up — process start → first activity | < 1.5 s cold, < 600 ms warm | warm 172–198 ms, cold 387–529 ms (3 runs each) | [lea, 2026-11-21](docs/perf-results.md#baselines--lea-2026-11-20-nix-package-main--81ea837) |
| Binary — size | AppImage < 25 MB | 19.7 MB AppImage (v0.2.0); 45 MB Nix store path (binary ~34 MiB) | [lea, 2026-11-20](docs/perf-results.md#baselines--lea-2026-11-20-nix-package-main--81ea837) |
| Memory — 60 s full-list scroll | post-scroll within 10 % of loaded | 138 MB loaded → 151 MB peak (+9.4 %) | [lea, 2026-11-21](docs/perf-results.md#scroll-peak-experiments-lea-2026-11-21) |
| Scroll — frame budget while flinging | no frame > 16 ms at 144 Hz | `draw` p50 0.56–1.75 ms, p99 0.98–2.79 ms, max 5.01 ms (UI `turn` max 5.01 ms); **0 frames > 16 ms** across 3 × 60 s runs | [lea, 2026-11-25](docs/perf-results.md#scroll-frame-budget-lea-2026-11-25) |
| Search — keystroke → list updated | < 16 ms on 10k mods | 3.3–3.4 ms steady, 6.6 ms first letter | [lea, 2026-11-21](docs/perf-results.md#baselines--lea-2026-11-20-nix-package-main--81ea837) |
| Install — 100 MB archive | extraction > 60 % of cold disk-read speed; download cancel < 50 ms | 7.4–7.6 GB/s: 51 % of the warm raw read (the copy ceiling), ~110 % of the disk's cold read; cancel 88 µs in the download phase, extraction not cancellable | [lea, 2026-11-21](docs/perf-results.md#install-probe-lea-2026-11-21) |

Two core paths are also held by criterion benches, recorded 2026-11-20 on the dev workstation (Apple M1
Pro, macOS — not lea, which records its own when the suites run there):

- **Load order.** `move_mod` and `set_pak`, worst case (every VPK renames):
  294–308 µs per mod, flat from 25 to 99 movable mods — 99 is the engine's
  pak capacity, the whole realizable domain. The list build the UI runs
  before every move: 17.3–20.9 µs per mod at 100 to 1000 rows
  ([run](docs/perf-results.md#reorder-shiv-corebenchesreorderrs)).
- **Catalog parse.** `list_mods` over a 10,000-row catalog on the real
  HTTP-body-plus-serde path: 239 MiB/s, second consecutive run 211 MiB/s;
  the baseline to hold is ≥ 200 MiB/s on that machine
  ([run](docs/perf-results.md#catalog-shiv-apibenchescatalogrs)).

To verify any number: its commands are recorded beside it in
[docs/perf-results.md](docs/perf-results.md) — re-run them and compare.

## Features

- **Browse** the Deadlock Mods catalog with search, sorting, category and hero filters, pagination and
  thumbnails. Pick several heroes at once.
- **Mod registries.** Add any `dmm-source` catalog (a mod site or your own static hosting) under
  **Sources**: Shiv reviews its name and download origin before saving it, browses its mods beside
  the catalog, verifies the published SHA-256 of every download, and updates a registry mod when
  its release's content revision changes.
- **Mod pages** with an image gallery, description, **Files**, **Changelog** and **Comments** tabs,
  the author's **Ko-fi** (or Patreon, Buy Me a Coffee, PayPal) link when they have one, and a link
  from the author's name to all of their mods. "Updated 5 days ago" instead of raw dates.
- **Install** mods from zip, rar, 7z and multi-VPK archives, with a file picker when a mod offers
  several downloads, and a **Downloads** screen with progress, speed, cancel and retry.
- **Install local files and links** the same way: a `.vpk` or archive from disk, a dropped file —
  several at once — or a pasted download link, managed like any other mod. A pasted mod page from
  gamebanana.com or deadlocker.net, or a Deadlock Mod Manager or Grimoire 1-click link, opens the
  mod's catalog page; when the catalog has no such mod, a 1-click link downloads the GameBanana
  archive it names.
- **Move your library** between mod managers with the Deadlock Mod Interchange format: export an
  interchange bundle from Installed, and import one back — duplicate mods are recognised, files
  already in the game folder are adopted in place, and nothing of the source bundle is touched.
- **Installed** mods open their detail page when clicked. Enable, disable and uninstall from
  there, edit the name and notes, and set which pak number a mod sits at; everything else
  renumbers around it.
- **Profiles.** Keep separate sets of mods (a "sweaty" setup, a "just for fun" one) and switch
  between them from the sidebar. Each profile has its own addons folder, so nothing leaks between them.
- **Launch** the game through Steam, or **Launch without mods**; your mods come back when the
  game closes.
- **Fix broken mods.** A health check finds truncated, partial or duplicate VPKs, and a
  repair step fixes mods broken by game updates, with a preview before anything changes.
- **Crosshairs** with a live preview, applied as `citadel_crosshair_*` values.
- **Performance presets** (pretty, balanced, sweaty, potato) and per-setting editing,
  written to the game's `GameInfo` file.
- **CompLock** from [deadlockconfig.com](https://deadlockconfig.com/) (by jcStani and HoppCX):
  stage their engine settings like a preset, and build their pack inside Shiv: the HUD++
  base with its damage-number choice, the FOV slider, the convar options (with their off
  values) and the optional paks, downloaded straight from the site, CRC-checked and
  installed as mods with progress on Downloads. A re-install updates in place. The data
  ships with Shiv and a daily watch keeps it at the site's build. The paks replace whole
  files, so they don't mix with QOL Lock and similar packs, and No Cloth conflicts with
  hero-model mods.
- NSFW mods are blurred until you turn them on, and that setting resets every session.

## Quality

The rules are enforced by the build, not by review:

- **No panics in the libraries or on the event loop.** `shiv-core`, `shiv-api`,
  `shiv-repair` and `shiv-ui` deny `clippy::unwrap_used` and
  `clippy::expect_used` at the crate root; the few `expect`s that cannot fail
  carry a scoped `#[allow]` naming the reason.
- **Nothing blocks the event loop; no worker polls.**
  [`crates/shiv-ui/tests/guards.rs`](crates/shiv-ui/tests/guards.rs) reads the
  controller sources and fails the suite on file, network, dialog or sleep
  calls outside `spawn_worker`, and on a queue re-checked on a sleep timer
  instead of parking on its condvar.
- **Tests assert invariants, not hand-computed examples.** The `set_pak`
  property test generates 120 random configurations and asserts uniqueness,
  preserved order, sidecar integrity and byte-identical refusals
  ([`crates/shiv-core/tests/sidecar.rs`](crates/shiv-core/tests/sidecar.rs));
  the install test counts bytes written and fails on a double write.
- **Every push to main and every pull request runs the gates.**
  `cargo fmt --all --check`,
  `cargo clippy --workspace --all-targets -- -D warnings` and
  `cargo test --workspace` on Ubuntu
  ([ci.yml](.github/workflows/ci.yml)). `nix flake check` builds the package
  and runs the workspace tests that need no windowing environment beside it.

Every rule traces to the bug that created it —
[docs/code-quality.md](docs/code-quality.md). The measurement law (a decision
without a measurement is an opinion) is
[docs/proof-plan.md](docs/proof-plan.md). The toolkit notes and the repair
port story are in [docs/](docs/).

## Install

### Arch Linux (AUR)

**Not published yet.** AUR account registration is currently paused, so the `shiv-bin` and
`shiv-git` packages are not on the AUR. Until they are, use the AppImage below, or build the
package yourself from the drafts in [`packaging/aur/`](packaging/aur/):

```sh
git clone https://github.com/luxus/shiv.git
cd shiv/packaging/aur/shiv-git
makepkg -si
```

### Any Linux: AppImage

Download `Shiv-<version>-x86_64.AppImage` from the
[latest release](../../releases/latest), then:

```sh
chmod +x Shiv-*-x86_64.AppImage
./Shiv-*-x86_64.AppImage
```

A `.sha256` file sits next to it if you want to verify the download.

### Any Linux: Flatpak

Download `Shiv-<version>-x86_64.flatpak` from the
[latest release](../../releases/latest), then:

```sh
flatpak install --user Shiv-*-x86_64.flatpak
flatpak run io.github.Siebe_Uy.shiv
```

The Flatpak can reach the default Steam folders (native and Flatpak Steam). For a Steam
library on another drive, grant it access:
`flatpak override --user --filesystem=/path/to/library io.github.Siebe_Uy.shiv`.
Inside the sandbox Shiv cannot see whether Deadlock is already running.

### Nix (NixOS, home-manager, any flake)

One command, no install step:

```sh
nix run github:luxus/shiv
```

The package ships a desktop entry and an icon, so a system that installed
it lists Shiv in the application menu.

**In a flake-based config**, add the input once:

```nix
inputs.shiv.url = "github:luxus/shiv";
```

then use the package wherever your packages live:

```nix
# NixOS module
environment.systemPackages = [ inputs.shiv.packages.${system}.default ];

# home-manager / hjem, the same shape
home.packages = [ inputs.shiv.packages.${system}.default ];
```

The package builds with the toolchain pinned in `rust-toolchain.toml` and
links the system `zstd`. Tests do not run inside the build — the Nix sandbox
has no `$HOME` or runtime directory for the one integration test that starts
the Slint platform — so `nix flake check` runs them beside the package
instead: every workspace test that needs no windowing environment. The
GitHub CI runs the full suite, that test included.

**Developers:** `nix develop` gives you the pinned toolchain, the Wayland
and fontconfig libraries Slint renders with, and a `pkg-config` that finds
them; `cargo` fetches the rest as usual.

```sh
nix develop
cargo test --workspace          # everything
cargo run --release -p shiv-ui  # the app
```

### Windows

Download from the [latest release](../../releases/latest):

- `Shiv-<version>-x64-setup.exe`: installer (per user, no admin rights needed), with a Start
  menu entry and an uninstaller.
- `Shiv-<version>-x64-portable.exe`: a single file, no install. Run it from anywhere.

The files are not code-signed, so Windows SmartScreen may warn you: choose
**More info**, then **Run anyway**. Each file has a `.sha256` next to it on the release page
if you want to verify the download (`certutil -hashfile <file> SHA256` on Windows).

## Build from source

You need a recent stable Rust toolchain (edition 2024) and the usual Linux
windowing libraries (Wayland or X11, `libxkbcommon`, `fontconfig`). On a
Nix system, `nix develop` in a checkout provides all of it, pinned:

```sh
git clone https://github.com/luxus/shiv.git
cd shiv
nix develop
cargo run --release -p shiv-ui      # the binary is target/release/shiv
```

Without Nix:

```sh
git clone https://github.com/luxus/shiv.git
cd shiv
cargo run --release -p shiv-ui      # the binary is target/release/shiv
```

On macOS (not shipped, but fine for development) a GCC from nix or Homebrew on
`PATH` builds unrar's C++ against its own libstdc++, which the Apple linker
cannot resolve; `.cargo/config.toml` already pins the final link to Apple
clang. Point the C++ compilation there too:

```sh
export CC=/usr/bin/cc CXX=/usr/bin/cc
cargo test --workspace
```

Run the tests:

```sh
cargo test --workspace
```

Shiv locates Deadlock through your Steam library automatically.

## Project layout

| Crate | Purpose |
|---|---|
| `shiv-ui` | The Slint application: screens, state and thumbnail pipeline |
| `shiv-core` | Game detection, install, enable/disable, load order, profiles, health checks, launch, crosshair, performance, interchange, registries |
| `shiv-api` | Clients for the [Deadlock Mods API](https://api.deadlockmods.app), GameBanana (files, changelog, comments) and native `dmm-source` registries |
| `shiv-repair` | Content repair for VPKs broken by game updates |
| `vpkmanager`, `vpk-parser`, `kv-parser`, `source2-model` | VPK, KeyValues and Source 2 parsing, copied from the Deadlock Mod Manager |

Brand assets live in [`style/`](style/README.md). Packaging (AppImage, Flatpak, Windows, AUR) is in [`packaging/`](packaging/).

## Credits

A big thank you to the developers of the original
[Deadlock Mod Manager](https://github.com/deadlock-mod-manager/deadlock-mod-manager).
Shiv would not exist without their work, their API and the parsing code it builds on.

Mod data comes from the [Deadlock Mods](https://deadlockmods.app) API, which mirrors
[GameBanana](https://gamebanana.com). The VPK and Source 2 crates derive from the
[Deadlock Mod Manager](https://github.com/deadlock-mod-manager/deadlock-mod-manager)
and bundle code under their own licenses (see `LICENSE-ValveResourceFormat` and
`LICENSE-vpkmerge` in `crates/`).

Shiv is not affiliated with Valve.

## License

Shiv is free software under the [GNU General Public License v3.0](LICENSE) (`GPL-3.0`).
