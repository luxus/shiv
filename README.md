<p align="center">
  <img src="style/png/shiv-banner-dark.png" alt="Shiv: Deadlock mod manager. Lean, fast, no bloat. Built in Rust." width="100%">
</p>

<p align="center">
  <a href="LICENSE"><img alt="License: GPL-3.0" src="https://img.shields.io/badge/license-GPL--3.0-c9a84c?labelColor=121110"></a>
  <img alt="Built with Rust" src="https://img.shields.io/badge/built%20with-Rust-c9a84c?labelColor=121110">
  <img alt="UI: Slint" src="https://img.shields.io/badge/UI-Slint-c9a84c?labelColor=121110">
  <img alt="Platform: Linux and Windows" src="https://img.shields.io/badge/platform-Linux%20%7C%20Windows-c9a84c?labelColor=121110">
</p>

**Shiv** is a mod manager for
[Deadlock](https://store.steampowered.com/app/1422450/Deadlock/) — native,
tiny, and fast. No Electron, no web engine: the whole app is a ~21 MB
download that opens in a blink and stays out of the way. Linux and
Windows.

## What you get

- **Browse and search** the Deadlock Mods catalog — 10,000+ mods with
  thumbnails, hero and category filters, sorting, and a search that
  answers as you type.
- **Install** mods from the catalog, from a zip/rar/7z/VPK file, from a
  dropped file, or from a pasted link (GameBanana page, 1-click link,
  direct download). Each install shows progress, speed, cancel and
  retry, and lands in your **Activity** history.
- **Update everything** from Installed: one button checks every mod
  against its source and updates what changed, in place.
- **Shiv updates itself** where it can: the AppImage and the portable
  `.exe` download a new release, verify its published SHA-256 and swap
  it in, all from the sidebar. Flatpak, Nix and AUR installs are pointed
  at their package manager instead.
- **Mod pages** with an image gallery, description, files, changelog,
  comments, and the author's support links.
- **CompLock built in** ([deadlockconfig.com](https://deadlockconfig.com/)):
  stage their engine settings like a preset, build their HUD/FOV/convar
  pack inside Shiv, CRC-checked. A pack update on the site reaches Shiv
  on its own — no app update needed.
- **Crosshair editor** with a live preview, and **performance presets**
  (pretty, balanced, sweaty, potato) with per-setting tweaks, written to
  the game's config.
- **Profiles** — separate mod sets with their own addons folders, switch
  any time.
- **Health check and repair**: finds truncated or broken mods and fixes
  them after game updates, with a preview before anything changes.
- **Move your library** between mod managers (Deadlock Mod Interchange
  bundles) — imports adopt files already in place.
- **Launch** the game through Steam with your mods, or without them.
- NSFW mods stay blurred until you turn them on; the choice resets every
  session.

## Why it feels fast

- **~100 MB of RAM** with the whole catalog loaded — not gigabytes.
- **Opens in ~0.2 s** warm, ~0.5 s cold.
- **No dropped frame** while flinging the 10k-mod list — measured, not
  felt: no frame over 16 ms across full-list scrolls.
- **Search answers in ~5 ms** per keystroke.
- **The whole app is a 21 MB AppImage.**

These are gates, not vibes: every number is re-measured on the same
reference machine, recorded with date and method in
[docs/perf-results.md](docs/perf-results.md), and a regression that
loses the margin fails the build.

## Install

### Windows

Download from the [latest release](../../releases/latest):

- `Shiv-<version>-x64-setup.exe` — installer with a Start menu entry and
  an uninstaller (per user, no admin rights).
- `Shiv-<version>-x64-portable.exe` — one file, run it from anywhere.

The files are not code-signed, so SmartScreen may warn you: choose
**More info**, then **Run anyway**. A `.sha256` sits next to each file on
the release page if you want to verify it.

### Linux: AppImage

```sh
chmod +x Shiv-*-x86_64.AppImage
./Shiv-*-x86_64.AppImage
```

### Linux: Flatpak

```sh
flatpak install --user Shiv-*-x86_64.flatpak
flatpak run io.github.Siebe_Uy.shiv
```

Reaches the default Steam folders (native and Flatpak Steam). For a
Steam library on another drive:
`flatpak override --user --filesystem=/path/to/library io.github.Siebe_Uy.shiv`.
Inside the sandbox Shiv cannot see whether Deadlock is already running.

### Nix

```sh
nix run github:luxus/shiv
```

In a flake config, add the input once:

```nix
inputs.shiv.url = "github:luxus/shiv";
```

### Arch Linux (AUR)

**Not published yet** (AUR registration is paused). Until then, use the
AppImage above, or build the draft from `packaging/aur/` yourself.

Shiv finds your Deadlock install through the Steam library on its own.

## Credits

Shiv was started by [Siebe-Uy](https://github.com/Siebe-Uy) as a lean,
no-bloat manager. Most of it has since been rewritten by
[luxus](https://github.com/luxus) with the original ideals in mind — and
made law instead of taste: small, measured, and enforced by tests and CI
on every push, not by review.

A big thank you to the developers of the original
[Deadlock Mod Manager](https://github.com/deadlock-mod-manager/deadlock-mod-manager):
Shiv builds on their work and their API, and would not exist without
them. Mod data comes from the [Deadlock Mods](https://deadlockmods.app)
API, which mirrors [GameBanana](https://gamebanana.com); the bundled
parsing crates carry their own licenses (see `crates/`).

Shiv is not affiliated with Valve.

## For developers

```sh
git clone https://github.com/luxus/shiv.git
cd shiv
nix develop                # pinned toolchain + Wayland/fontconfig libs
cargo run --release -p shiv-ui
cargo test --workspace     # the full suite
```

Without Nix you need a recent stable Rust (edition 2024) and the usual
Wayland/X11, `libxkbcommon` and `fontconfig` libraries. On macOS
(dev only, not shipped) export `CC=/usr/bin/cc CXX=/usr/bin/cc` for the
unrar C++ build.

| Crate | Purpose |
|---|---|
| `shiv-ui` | The Slint application: screens, state, thumbnail pipeline |
| `shiv-core` | Game detection, install, load order, profiles, health checks, launch, crosshair, performance, interchange, registries, CompLock |
| `shiv-api` | Clients for the Deadlock Mods API, GameBanana, and native `dmm-source` registries |
| `shiv-repair` | Content repair for VPKs broken by game updates |
| `vpkmanager`, `vpk-parser`, `kv-parser`, `source2-model` | VPK, KeyValues and Source 2 parsing, derived from the Deadlock Mod Manager |

The rules of the codebase are enforced by the build, not by review: no
panics on production paths, nothing blocks the UI thread, allocation
ceilings and invariant tests on the hot paths, and every push runs the
gates. Every rule traces to the bug that created it in
[docs/code-quality.md](docs/code-quality.md); the measurement law is
[docs/proof-plan.md](docs/proof-plan.md); how to work in this repo is
[AGENTS.md](AGENTS.md). Packaging lives in [`packaging/`](packaging/),
brand assets in [`style/`](style/README.md).

## License

Shiv is free software under the [GNU General Public License
v3.0](LICENSE) (`GPL-3.0`).
