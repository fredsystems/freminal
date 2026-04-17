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
            comment = "A modern GPU-accelerated terminal emulator";
            exec = "freminal";
            terminal = false;
            categories = [
              "System"
              "TerminalEmulator"
            ];
            keywords = [
              "terminal"
              "shell"
              "console"
              "command line"
            ];
            startupNotify = false;
            icon = "freminal";

            # Match the app_id / WM_CLASS that Freminal sets on its windows.
            # This lets compositors and taskbars associate running windows with
            # this .desktop entry for icon lookup and window grouping.
            startupWMClass = "freminal";

            # Each launch from the .desktop entry or application launcher must
            # spawn a fully independent process. Launchers that honour this key
            # (e.g. vicinae, GNOME Shell) will never coalesce a new launch into
            # an already-running instance.
            singleMainWindow = false;

            # Explicit "New Terminal" action so launchers can offer spawning a
            # fresh instance even when their default is to focus an existing
            # window (e.g. vicinae Ctrl+B "open new").
            actions.new-terminal = {
              name = "New Terminal";
              exec = "freminal";
            };
          };
        in
        {
          freminal = pkgs.rustPlatform.buildRustPackage {
            pname = "freminal";
            version = "0.6.0";
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
              rust_options = {
                xtaskCheck = "pc";
              };
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
                pkgs.perf
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

                # Enable the `playback` feature by default in the devshell so
                # developers get recording/playback support without extra flags.
                CARGO_BUILD_FEATURES = "playback";

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
