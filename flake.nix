{
  description = "Consumer repo using shared base + rust precommit system";

  inputs = {
    precommit.url = "github:FredSystems/pre-commit-checks";
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
  };

  outputs =
    {
      self,
      precommit,
      nixpkgs,
      ...
    }:
    let
      inherit (nixpkgs) lib;
      systems = precommit.lib.supportedSystems;
    in
    {
      ##########################################################################
      ## OVERLAY — adds `pkgs.freminal` when applied
      ##########################################################################
      overlays.default = import ./nix/overlay.nix { freminal-flake = self; };

      ##########################################################################
      ## HOME-MANAGER MODULE — `programs.freminal` option set
      ##########################################################################
      homeManagerModules.default = import ./nix/home-manager-module.nix { freminal-flake = self; };

      packages = lib.genAttrs systems (
        system:
        let
          pkgs = import nixpkgs { inherit system; };

          runtimeLibs = [
            pkgs.libxkbcommon
          ]
          ++ pkgs.lib.optionals pkgs.stdenv.isLinux [
            pkgs.wayland
            pkgs.libGL
          ];

          runtimeLibPath = pkgs.lib.makeLibraryPath runtimeLibs;

          desktopItem = pkgs.makeDesktopItem {
            name = "freminal";
            desktopName = "Freminal";
            comment = "Terminal emulator";
            exec = "freminal";
            terminal = false;
            categories = [
              "System"
              "TerminalEmulator"
            ];
            startupNotify = false;
            icon = "freminal";
          };

          desktopItemTest = pkgs.makeDesktopItem {
            name = "freminal-recording";
            desktopName = "Freminal (recording)";
            comment = "Terminal emulator";
            exec = "freminal --recording-path /home/fred/freminal.bin";
            terminal = false;
            categories = [
              "System"
              "TerminalEmulator"
            ];
            startupNotify = false;
            icon = "freminal";
          };
        in
        {
          freminal = pkgs.rustPlatform.buildRustPackage {
            pname = "freminal";
            version = "0.1.4";
            src = pkgs.lib.cleanSource ./.;

            cargoLock.lockFile = ./Cargo.lock;

            nativeBuildInputs = [
              pkgs.pkg-config
              pkgs.makeWrapper
              pkgs.copyDesktopItems
            ];

            buildInputs = runtimeLibs;

            desktopItems = [
              desktopItem
              desktopItemTest
            ];

            postInstall = ''
              wrapProgram $out/bin/freminal \
                --prefix LD_LIBRARY_PATH : ${runtimeLibPath}

              # optional, once you have icons in the repo:
              install -Dm644 assets/icon.png \
                $out/share/icons/hicolor/256x256/apps/freminal.png
            '';
          };
        }
      );

      ##########################################################################
      ## CHECKS — unified base+rust via mkCheck
      ##########################################################################
      checks = builtins.listToAttrs (
        map (system: {
          name = system;
          value = {
            pre-commit-check = precommit.lib.mkCheck {
              inherit system;
              src = ./.;
              check_rust = true;
              enableXtask = true;
              extraExcludes = [
                "^speed_tests/"
                "^Documents/reference"
                "^res/"
                "typos.toml"
                "\\.bin$"
              ];
            };
          };
        }) systems
      );

      ##########################################################################
      ## DEV SHELLS — merged env + your extra Rust goodies
      ##########################################################################
      devShells = builtins.listToAttrs (
        map (system: {
          name = system;

          value =
            let
              pkgs = import nixpkgs { inherit system; };

              # Unified check result (base + rust)
              chk = self.checks.${system}."pre-commit-check";

              # Packages that git-hooks.nix / mkCheck say we need
              corePkgs = chk.enabledPackages or [ ];

              # Extra Rust / tooling packages (NO extra rustc here)
              extraRustTools = [
                chk.passthru.devPackages
                pkgs.cargo-deny
                pkgs.cargo-machete
                pkgs.cargo-make
                pkgs.cargo-profiler
                pkgs.cargo-bundle
                pkgs.typos
                pkgs.vttest
                pkgs.markdownlint-cli2
                pkgs.cargo-flamegraph
              ]
              ++ pkgs.lib.optionals pkgs.stdenv.isLinux [
                pkgs.cargo-llvm-cov
                pkgs.cachix
              ];

              # Extra dev packages provided by mkCheck (includes rustToolchain)
              extraDev = chk.passthru.devPackages or [ ];

              # Library path packages: whatever mkCheck wants + your GL/Wayland bits
              libPkgs =
                (chk.passthru.libPath or [ ])
                ++ [
                  pkgs.libGL
                  pkgs.libxkbcommon
                ]
                ++ pkgs.lib.optionals pkgs.stdenv.isLinux [
                  pkgs.wayland
                ];
            in
            {
              default = pkgs.mkShell {
                buildInputs = extraRustTools ++ corePkgs ++ extraDev;

                LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath libPkgs;

                shellHook = ''
                  ${chk.shellHook}

                  alias pre-commit="pre-commit run --all-files"
                '';
              };
            };
        }) systems
      );
    };
}
