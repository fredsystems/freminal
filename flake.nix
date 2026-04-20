{
  description = "Consumer repo using shared base + rust precommit system";

  inputs = {
    precommit.url = "github:FredSystems/pre-commit-checks";
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      precommit,
      nixpkgs,
      rust-overlay,
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
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ rust-overlay.overlays.default ];
          };

          rustToolchain = pkgs.rust-bin.stable.latest.default;

          rustPlatform = pkgs.makeRustPlatform {
            cargo = rustToolchain;
            rustc = rustToolchain;
          };

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
          freminal = rustPlatform.buildRustPackage {
            pname = "freminal";
            version = "0.7.0";
            src = pkgs.lib.cleanSource ./.;

            cargoLock.lockFile = ./Cargo.lock;

            nativeBuildInputs = [
              pkgs.pkg-config
              pkgs.makeWrapper
            ]
            ++ pkgs.lib.optionals pkgs.stdenv.isLinux [
              pkgs.copyDesktopItems
            ];

            buildInputs = runtimeLibs;

            desktopItems = pkgs.lib.optionals pkgs.stdenv.isLinux [
              desktopItem
            ];

            postInstall =
              if pkgs.stdenv.isDarwin then
                ''
                                    # Create a macOS .app bundle so Freminal appears in
                                    # Finder / Spotlight / Launchpad.
                                    mkdir -p "$out/Applications"
                                    OUT_APP="$out/Applications/Freminal.app/Contents"
                                    mkdir -p "$OUT_APP/MacOS" "$OUT_APP/Resources"

                                    # macOS only recognises an app bundle if the real binary
                                    # lives inside it.  Move the binary in, then create a
                                    # symlink back to $out/bin/ so CLI usage still works.
                                    mv "$out/bin/freminal" "$OUT_APP/MacOS/freminal"
                                    ln -s "$OUT_APP/MacOS/freminal" "$out/bin/freminal"

                                    # Write Info.plist
                                    cat > "$OUT_APP/Info.plist" << 'PLIST'
                  <?xml version="1.0" encoding="UTF-8"?>
                  <!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
                    "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
                  <plist version="1.0">
                  <dict>
                    <key>CFBundleName</key>
                    <string>Freminal</string>
                    <key>CFBundleDisplayName</key>
                    <string>Freminal</string>
                    <key>CFBundleIdentifier</key>
                    <string>io.github.fredclausen.freminal</string>
                    <key>CFBundleVersion</key>
                    <string>0.6.0</string>
                    <key>CFBundleShortVersionString</key>
                    <string>0.6.0</string>
                    <key>CFBundleExecutable</key>
                    <string>freminal</string>
                    <key>CFBundleIconFile</key>
                    <string>freminal</string>
                    <key>CFBundlePackageType</key>
                    <string>APPL</string>
                    <key>LSMinimumSystemVersion</key>
                    <string>13.0</string>
                    <key>NSHighResolutionCapable</key>
                    <true/>
                  </dict>
                  </plist>
                  PLIST

                                    # Icon — use pre-generated .icns (multi-resolution, Retina-ready).
                                    cp assets/macos/freminal.icns "$OUT_APP/Resources/freminal.icns"
                ''
              else
                ''
                  wrapProgram $out/bin/freminal \
                    --prefix LD_LIBRARY_PATH : ${runtimeLibPath}

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
                "assets/"
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
                pkgs.perf
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
