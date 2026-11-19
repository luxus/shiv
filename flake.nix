{
  description = "Shiv — a small, native Deadlock mod manager";

  # Same caches as the luxusAi flake: rust-overlay's toolchains come from
  # nix-community, so `nix develop` is a download, not a compiler run.
  nixConfig = {
    substituters = [
      "https://cache.nixos.org"
      "https://nix-community.cachix.org"
    ];
    trusted-public-keys = [
      "cache.nixos.org-1:6NCHdD59X431o0gWypbMrAURkbJ16ZPMQFGspcDShjY="
      "nix-community.cachix.org-1:mB9FSh9qf2dCimDSUo8Zy7bkq5CX+/rkCWyvRCYg3Fs="
    ];
  };

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    { self, nixpkgs, rust-overlay }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forAllSystems =
        f:
        nixpkgs.lib.genAttrs systems (
          system:
          f (
            import nixpkgs {
              inherit system;
              overlays = [ rust-overlay.overlays.default ];
            }
          )
        );

      toolchain =
        pkgs:
        pkgs.rust-bin.fromRustupToolchainFile "${self}/rust-toolchain.toml";

      rustPlatform =
        pkgs:
        pkgs.makeRustPlatform {
          rustc = toolchain pkgs;
          cargo = toolchain pkgs;
        };

      # What Slint needs to render, and pkg-config so the build scripts
      # find it — on Linux. macOS development needs the toolchain alone.
      windowingDeps =
        pkgs:
        nixpkgs.lib.optionals pkgs.stdenv.hostPlatform.isLinux (
          with pkgs;
          [
            pkg-config
            fontconfig
            libxkbcommon
            wayland
          ]
        );

      # zstd-sys builds its C in the sandbox when nothing tells it not to;
      # linking the nixpkgs zstd instead is one pkg-config lookup and
      # skips a fragile vendored build.
      zstdFromPkgConfig =
        pkgs:
        nixpkgs.lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
          ZSTD_SYS_USE_PKG_CONFIG = "1";
        };

      shivPackage =
        pkgs:
        (rustPlatform pkgs).buildRustPackage {
          pname = "shiv";
          version = (pkgs.lib.importTOML "${self}/crates/shiv-ui/Cargo.toml").package.version;

          src = self;
          cargoLock.lockFile = "${self}/Cargo.lock";

          doCheck = true;

          env = zstdFromPkgConfig pkgs;

          nativeBuildInputs = windowingDeps pkgs ++ [ pkgs.copyDesktopItems ];
          buildInputs = windowingDeps pkgs ++ [ pkgs.zstd ];

          desktopItems = [
            (pkgs.makeDesktopItem {
              name = "io.github.Siebe_Uy.shiv";
              desktopName = "Shiv";
              genericName = "Deadlock Mod Manager";
              comment = "Lean, fast mod manager for Deadlock";
              exec = "shiv";
              icon = "io.github.Siebe_Uy.shiv";
              terminal = false;
              categories = [ "Game" ];
              keywords = [
                "Deadlock"
                "Mods"
                "Valve"
              ];
              startupWMClass = "shiv";
            })
          ];

          postInstall = ''
            install -Dm644 ${self}/packaging/flatpak/io.github.Siebe_Uy.shiv.128.png \
              $out/share/icons/hicolor/128x128/apps/io.github.Siebe_Uy.shiv.png
          '';

          meta = with pkgs.lib; {
            description = "A small, native mod manager for Deadlock";
            homepage = "https://github.com/luxus/shiv";
            license = licenses.gpl3Only;
            mainProgram = "shiv";
            platforms = platforms.linux;
          };
        };
    in
    {
      # The shipped app: Linux only, with a desktop entry and an icon so
      # any desktop that added it lists Shiv in the application menu.
      packages = forAllSystems (
        pkgs:
        nixpkgs.lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
          default = shivPackage pkgs;
          shiv = shivPackage pkgs;
        }
      );

      # `nix develop` — the pinned toolchain and the libraries Slint
      # renders with; cargo fetches the rest as usual.
      devShells = forAllSystems (
        pkgs:
        {
          default = pkgs.mkShell {
            packages =
              [
                (toolchain pkgs)
                pkgs.git
              ]
              ++ windowingDeps pkgs
              ++ nixpkgs.lib.optionals pkgs.stdenv.hostPlatform.isLinux [ pkgs.zstd ];
            env = zstdFromPkgConfig pkgs;
            shellHook = ''
              echo "shiv $(rustc --version)"
              echo "cargo test --workspace          — everything"
              echo "cargo run --release -p shiv-ui  — the app"
            '';
          };
        }
      );

      # `nix flake check` — the package builds with its tests inside, the
      # same gates CI runs, from one command.
      checks = forAllSystems (
        pkgs:
        nixpkgs.lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
          shiv = shivPackage pkgs;
        }
      );
    };
}
