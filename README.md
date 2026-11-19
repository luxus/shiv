<p align="center">
  <img src="style/png/shiv-banner-dark.png" alt="Shiv: Deadlock mod manager. Lean, fast, no bloat. Built in Rust." width="100%">
</p>

<p align="center">
  <a href="LICENSE"><img alt="License: GPL-3.0" src="https://img.shields.io/badge/license-GPL--3.0-c9a84c?labelColor=121110"></a>
  <img alt="Built with Rust" src="https://img.shields.io/badge/built%20with-Rust-c9a84c?labelColor=121110">
  <img alt="UI: Slint" src="https://img.shields.io/badge/UI-Slint-c9a84c?labelColor=121110">
  <img alt="Platform: Linux and Windows" src="https://img.shields.io/badge/platform-Linux%20%7C%20Windows-c9a84c?labelColor=121110">
</p>

**Shiv** is a small, native mod manager for [Deadlock](https://store.steampowered.com/app/1422450/Deadlock/).
It has no webview and no Electron or Tauri layer, so it stays light on memory and
scrolls smoothly on Linux and Windows.

It exists because the standard manager renders through a web engine and idles at
gigabytes of RAM. Shiv does the essentials and nothing else.

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
  several at once — or a pasted download link, all as cleanly managed mods.
- **Move your library** between mod managers with the Deadlock Mod Interchange format: export an
  interchange bundle from Installed, and import one back — duplicate mods are recognised, files
  already in the game folder are adopted in place, and nothing of the source bundle is touched.
- **Installed** mods open their detail page when clicked. Enable, disable and uninstall from
  there, edit the name and notes, and set which pak number a mod sits at; everything else
  renumbers around it.
- **Profiles.** Keep separate sets of mods (a "sweaty" setup, a "just for fun" one) and switch
  between them from the sidebar. Each profile has its own addons folder, so nothing leaks between them.
- **Launch** the game through Steam, or **Launch without mods** for a clean session; your mods
  come back when the game closes.
- **Fix broken mods.** A health check finds truncated, partial or duplicate VPKs, and a
  repair step fixes mods broken by game updates, with a preview before anything changes.
- **Crosshairs** with a live preview, applied as `citadel_crosshair_*` values.
- **Performance presets** (pretty, balanced, sweaty, potato) and per-setting editing,
  written to the game's `GameInfo` file.
- NSFW mods are blurred until you turn them on, and that setting resets every session.

Notes on the toolkit choice, the repair port and the coding rules this
repository enforces (no panics in the libraries, invariants over examples)
are in [`docs/`](docs/).

## Install

### Arch Linux (AUR)

**Not published yet.** AUR account registration is currently paused, so the `shiv-bin` and
`shiv-git` packages are not on the AUR. Until they are, use the AppImage below, or build the
package yourself from the drafts in [`packaging/aur/`](packaging/aur/):

```sh
git clone https://github.com/Siebe-Uy/shiv.git
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

The package builds with the toolchain pinned in `rust-toolchain.toml`,
links the system `zstd`, and runs the workspace tests inside the build —
so an installed Shiv is a tested Shiv, and `nix flake check` builds exactly
what CI builds.

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
