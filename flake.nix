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
      # winit dlopens the wayland client library at runtime, so it belongs
      # in buildInputs, not only in the build-time lookup path.
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

      runtimeDeps =
        pkgs:
        nixpkgs.lib.optionals pkgs.stdenv.hostPlatform.isLinux (
          with pkgs;
          [
            wayland
            libxkbcommon
            fontconfig
            zstd
          ]
        );

      # zstd-sys builds its vendored C in the sandbox and fails there; the
      # crate links the system zstd when told to. zstd.dev carries the
      # libzstd.pc the build script looks up, so it belongs beside
      # pkg-config in the native inputs.
      zstdForPkgConfig =
        pkgs:
        nixpkgs.lib.optionals pkgs.stdenv.hostPlatform.isLinux (
          with pkgs;
          [
            pkg-config
            zstd.dev
          ]
        );

      shivPackage =
        pkgs:
        (rustPlatform pkgs).buildRustPackage {
          pname = "shiv";
          version = (pkgs.lib.importTOML "${self}/crates/shiv-ui/Cargo.toml").package.version;

          src = self;
          cargoLock.lockFile = "${self}/Cargo.lock";

          # The install must not hinge on a UI test's environment: the
          # sandbox has no $HOME or runtime dir, and the one integration
          # test that inits the Slint platform needs one. The workspace
          # tests run in `checks.tests` and on the GitHub CI instead.
          doCheck = false;

          env = nixpkgs.lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
            ZSTD_SYS_USE_PKG_CONFIG = "1";
          };

          nativeBuildInputs =
            windowingDeps pkgs
            ++ zstdForPkgConfig pkgs
            ++ [
              pkgs.copyDesktopItems
              pkgs.patchelf
            ];
          buildInputs = runtimeDeps pkgs;

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

          # winit dlopens libwayland-client and libxkbcommon when a window
          # opens, so nothing links them at build time and the automatic
          # RPATH skips them. The binary cannot run without them, so the
          # paths go in by hand.
          postFixup = ''
            rpaths="${
              pkgs.lib.makeSearchPath "lib" [
                pkgs.wayland
                pkgs.libxkbcommon
              ]
            }"
            existing="$(patchelf --print-rpath $out/bin/shiv)"
            patchelf --set-rpath "$existing:$rpaths" $out/bin/shiv
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
              ++ zstdForPkgConfig pkgs
              ++ nixpkgs.lib.optionals pkgs.stdenv.hostPlatform.isLinux [ pkgs.zstd ];
            env = nixpkgs.lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
              ZSTD_SYS_USE_PKG_CONFIG = "1";
            };
            shellHook = ''
              echo "shiv $(rustc --version)"
              echo "cargo test --workspace          — everything"
              echo "cargo run --release -p shiv-ui  — the app"
            '';
          };
        }
      );

      # `nix flake check` — the package builds, and the workspace tests
      # run beside it as their own derivation.
      checks = forAllSystems (
        pkgs:
        nixpkgs.lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
          shiv = shivPackage pkgs;

          # The library, core and API tests: everything that needs no
          # windowing environment. The one Slint integration test runs on
          # the GitHub CI, where the environment is a real one.
          tests =
            pkgs.runCommand "shiv-tests"
              {
                nativeBuildInputs =
                  [
                    (toolchain pkgs)
                    pkgs.git
                  ]
                  ++ windowingDeps pkgs
                  ++ zstdForPkgConfig pkgs;
                env = nixpkgs.lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
                  ZSTD_SYS_USE_PKG_CONFIG = "1";
                };
              }
              ''
                cd ${self}
                cargo test --workspace --offline -p shiv-core -p shiv-api -p shiv-repair -p kv-parser -p vpk-parser -p vpkmanager -p source2-model
                touch $out
              '';
        }
      );
    };
}
