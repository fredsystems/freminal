{
  description = "Consumer repo using shared base + rust precommit system";

  inputs = {
    precommit.url = "github:FredSystems/pre-commit-checks";
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    # cargo-bundle is only used inside the dev shell to produce the Linux
    # deb/appimage artifacts locally.  nixpkgs is pinned at 0.9.0, which predates
    # SVG-icon and `appimage` support; we build from upstream master so the dev
    # shell matches the 0.11.0+ the release CI installs via `cargo install`.
    # `nix flake update cargo-bundle-src` picks up new upstream commits.
    cargo-bundle-src = {
      url = "github:burtonageo/cargo-bundle";
      flake = false;
    };
  };

  outputs =
    {
      self,
      precommit,
      nixpkgs,
      rust-overlay,
      cargo-bundle-src,
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

          version = "0.10.1";

          # The build sandbox strips `.git`, so the crate's build.rs `git
          # describe` can only ever yield "unknown".  Feed it a real value via
          # the `VERGEN_GIT_DESCRIBE` env override (build.rs honours a pre-set
          # value verbatim).  `self.shortRev` is defined for a clean flake
          # source; `self.dirtyShortRev` covers a dirty working tree; "nix" is
          # the last-resort fallback when neither is available.
          gitDescribe = "v${version}-${self.shortRev or self.dirtyShortRev or "nix"}";
        in
        {
          freminal = rustPlatform.buildRustPackage {
            pname = "freminal";
            inherit version;
            src = pkgs.lib.cleanSource ./.;

            # Embedded build version — see `gitDescribe` above.
            env.VERGEN_GIT_DESCRIBE = gitDescribe;

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
                    <string>0.10.1</string>
                    <key>CFBundleShortVersionString</key>
                    <string>0.10.1</string>
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

                  # Install the full hicolor icon tree: square anti-aliased
                  # PNGs at every standard size plus a scalable SVG.  Sized PNGs
                  # are required because some taskbars (e.g. wayle) resolve a
                  # window's icon by app_id and only consult sized PNG
                  # directories, skipping scalable/.  The previous single
                  # 301x289 PNG dropped into a "256x256" directory was both
                  # malformed (wrong dimensions for the directory) and jagged
                  # (1-bit alpha), so loaders fell back to the generic icon.
                  mkdir -p $out/share/icons/hicolor
                  cp -r assets/icons/hicolor/. \
                    $out/share/icons/hicolor/
                '';
          };
        }
      );

      ##########################################################################
      ## CHECKS — unified base+rust via mkCheck
      ##########################################################################
      checks = builtins.listToAttrs (
        map (
          system:
          let
            pkgs = import nixpkgs { inherit system; };

            # git-hooks.nix, reached transitively through the shared precommit
            # flake.  Used to re-run the hook generation with our extra TOML
            # hook merged in (mkCheck has no extraHooks parameter, so we
            # reconstruct the merged hook set the same way mkCheck does).
            gitHooks = precommit.inputs.git-hooks;

            extraExcludes = [
              "^speed_tests/"
              "^Documents/reference"
              "^res/"
              "typos.toml"
              "\\.bin$"
              "assets/"
            ];

            # The shared base + rust modules, exactly as mkCheck would build
            # them.  We merge their hooks/excludes/passthru ourselves so we can
            # add the tombi hook before handing the result to git-hooks.
            baseModule = precommit.lib.mkBaseCheck { inherit system extraExcludes; };
            rustModule = precommit.lib.mkRustCheck {
              inherit system extraExcludes;
              enableXtask = true;
              xtaskType = "pc";
            };

            # Custom hook: lint every staged TOML file with tombi, failing the
            # commit on warnings as well as errors.  TOML files are the Cargo
            # manifests and config_example.toml; `--error-on-warnings` makes
            # tombi exit non-zero on lint warnings (verified: clean files exit
            # 0, any warning exits 1), so this gates the commit.
            tombiHook = {
              tombi = {
                enable = true;
                name = "tombi (TOML lint)";
                description = "Lint TOML files with tombi, failing on warnings.";
                entry = "${pkgs.tombi}/bin/tombi lint --error-on-warnings";
                files = "\\.toml$";
                language = "system";
                pass_filenames = true;
              };
            };

            mergedHooks = baseModule.hooks // rustModule.hooks // tombiHook;
            mergedExcludes = (baseModule.excludes or [ ]) ++ (rustModule.excludes or [ ]) ++ extraExcludes;

            run = gitHooks.lib.${system}.run {
              src = ./.;
              hooks = mergedHooks;
              excludes = mergedExcludes;
            };
          in
          {
            name = system;
            value = {
              pre-commit-check = run // {
                passthru = {
                  devPackages = (baseModule.passthru.devPackages or [ ]) ++ (rustModule.passthru.devPackages or [ ]);
                  libPath = (baseModule.passthru.libPath or [ ]) ++ (rustModule.passthru.libPath or [ ]);
                };
                shellHook = run.shellHook or "";
                enabledPackages = run.enabledPackages or [ ];
              };
            };
          }
        ) systems
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

              # cargo-bundle built from upstream master (see the cargo-bundle-src
              # input).  nixpkgs ships 0.9.0, which cannot bundle SVG icons or
              # produce appimages; this matches the 0.11.0+ the release CI uses.
              # Mirrors the nixpkgs recipe (squashfsTools wrap for appimage).
              cargoBundleLatest = pkgs.rustPlatform.buildRustPackage {
                pname = "cargo-bundle";
                version = "unstable-${cargo-bundle-src.shortRev or "dirty"}";
                src = cargo-bundle-src;
                # Upstream stopped committing Cargo.lock after v0.9.0, so we
                # vendor one here (regenerate with `cargo generate-lockfile`
                # against the input source after a `nix flake update`).  The
                # source tree has no lockfile, so copy ours in for the build's
                # own cargo invocation too.
                cargoLock.lockFile = ./nix/cargo-bundle.lock;
                postPatch = ''
                  cp ${./nix/cargo-bundle.lock} Cargo.lock
                '';
                nativeBuildInputs = [
                  pkgs.pkg-config
                ]
                ++ pkgs.lib.optionals pkgs.stdenv.hostPlatform.isLinux [
                  pkgs.makeBinaryWrapper
                ];
                buildInputs = pkgs.lib.optionals pkgs.stdenv.hostPlatform.isLinux [
                  pkgs.libxkbcommon
                  pkgs.wayland
                  pkgs.openssl
                ];
                # squashfs tools are needed to build appimages for Linux.
                postFixup = pkgs.lib.optionalString pkgs.stdenv.hostPlatform.isLinux ''
                  wrapProgram $out/bin/cargo-bundle \
                    --prefix PATH : ${pkgs.lib.makeBinPath [ pkgs.squashfsTools ]}
                '';
                doCheck = false;
              };

              # Unified check result (base + rust)
              chk = self.checks.${system}."pre-commit-check";

              # Packages that git-hooks.nix / mkCheck say we need
              corePkgs = chk.enabledPackages or [ ];

              # Tooling needed by the CI checks (lint / test / machete / deny /
              # coverage / docs lint).  Kept deliberately lean so the CI `ci`
              # devShell does not build heavy dev-only tools.
              ciRustTools = [
                pkgs.tombi
                pkgs.cargo-deny
                pkgs.cargo-machete
                pkgs.cargo-make
                pkgs.typos
                pkgs.markdownlint-cli2
                pkgs.python313Packages.msgpack # For sequence decoder
              ]
              ++ pkgs.lib.optionals pkgs.stdenv.isLinux [
                pkgs.cargo-llvm-cov
                pkgs.cachix
              ];

              # Interactive-only extras.  These are excluded from the `ci`
              # devShell: cargoBundleLatest is built from source (expensive) and
              # is only needed to produce local deb/appimage artifacts; the
              # profilers, vttest, dpkg and squashfsTools are likewise unused by
              # CI lint/test.  squashfsTools + dpkg let the local artifact patch
              # script (assets/ci/fix-linux-icon-metadata.sh) run by hand, and
              # back `cargo xtask package-local`, which round-trips the deb and
              # AppImage to patch the embedded binary's ELF interpreter.
              # patchelf resets the Nix-store interpreter on locally-built
              # binaries to the portable /lib64 loader so local rpm/deb/AppImage
              # artifacts run on non-Nix distros (see `cargo xtask package-local`).
              # imagemagick + librsvg + libicns regenerate the icon assets
              # (hicolor PNG tree, icon.png, macOS .icns) from the editable
              # vector sources under assets/source/ -- see that directory's
              # SVGs.  librsvg is imagemagick's SVG delegate (text + gradient
              # rendering); libicns provides `png2icns` for the macOS bundle.
              devOnlyTools = [
                pkgs.cargo-profiler
                cargoBundleLatest
                pkgs.vttest
                pkgs.cargo-flamegraph
                pkgs.dpkg
                pkgs.squashfsTools
                pkgs.patchelf
                pkgs.imagemagick
                pkgs.librsvg
                pkgs.libicns
              ]
              ++ pkgs.lib.optionals pkgs.stdenv.isLinux [
                pkgs.perf
                pkgs.fish
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

              # Shared shell builder.  `extraTools` is the only axis on which
              # the `default` and `ci` shells differ.
              mkFreminalShell =
                extraTools:
                pkgs.mkShell {
                  buildInputs = extraDev ++ corePkgs ++ ciRustTools ++ extraTools;

                  LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath libPkgs;

                  # Enable the `playback` feature by default in the devshell so
                  # developers get recording/playback support without extra flags.
                  CARGO_BUILD_FEATURES = "playback";

                  shellHook = ''
                    ${chk.shellHook}

                    alias pre-commit="pre-commit run --all-files"
                  '';
                };
            in
            {
              # Full interactive shell: lint/test tooling plus the dev-only
              # extras (cargo-bundle, profilers, vttest, ...).
              default = mkFreminalShell devOnlyTools;

              # Lean shell for CI lint/test gates.  Omits devOnlyTools so CI
              # never builds cargo-bundle from source.  Used by nightly.yml.
              ci = mkFreminalShell [ ];
            };
        }) systems
      );
    };
}
