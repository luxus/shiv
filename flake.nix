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

      # What Slint needs to render, and pkg-config so the build scripts find
      # it. The unrar C++ links against the same nixpkgs, so one glibc
      # runs the whole binary.
      windowingDeps =
        pkgs:
        with pkgs;
        [
          pkg-config
          fontconfig
          libxkbcommon
          wayland
        ];
    in
    {
      packages = forAllSystems (
        pkgs:
        let
          version = (pkgs.lib.importTOML "${self}/crates/shiv-ui/Cargo.toml").package.version;
        in
        {
          default = self.packages.${pkgs.stdenv.hostPlatform.system}.shiv;

          # `nix run github:luxus/shiv` — the release binary, its tests run
          # inside the build, with a launcher entry so the app appears in
          # the application menu of any desktop that installed it.
          shiv = (rustPlatform pkgs).buildRustPackage {
            pname = "shiv";
            inherit version;

            src = self;
            cargoLock.lockFile = "${self}/Cargo.lock";

            doCheck = true;

            nativeBuildInputs =
              windowingDeps pkgs
              ++ [ pkgs.copyDesktopItems ];
            buildInputs = windowingDeps pkgs;

            desktopItems = [
              (pkgs.makeDesktopItem {
                name = "io.github.Siebe_Uy.shiv";
                desktopName = "Shiv";
                genericName = "Deadlock Mod Manager";
                comment = "Lean, fast mod manager for Deadlock";
                exec = "shiv";
                icon = "io.github.Siebe_Uy.shiv";
                terminal = false;
                categories = [
                  "Game"
                ];
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
        }
      );

      devShells = forAllSystems (
        pkgs:
        {
          # `nix develop` — the pinned toolchain, the windowing libraries
          # and a pkg-config that finds them; cargo fetches the rest.
          default = pkgs.mkShell {
            packages =
              [
                (toolchain pkgs)
                git
              ]
              ++ windowingDeps pkgs;
            shellHook = ''
              echo "shiv $(rustc --version)"
              echo "cargo test --workspace          — everything"
              echo "cargo run --release -p shiv-ui  — the app"
            '';
          };
        }
      );

      # `nix flake check` builds the package, tests included, on one
      # command — the same gates CI runs.
      checks = forAllSystems (
        pkgs:
        {
          shiv = self.packages.${pkgs.stdenv.hostPlatform.system}.shiv;
        }
      );
    };
}
