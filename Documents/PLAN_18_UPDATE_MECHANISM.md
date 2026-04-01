# PLAN 18 — Client-Side Update Mechanism

## Overview

Add an automatic update checker to Freminal that periodically queries an update service for the
latest release version, compares it against the running version, and displays a non-intrusive
notification in the menu bar when an update is available. The user can open a dialog to download
the new release to `~/Downloads` and see platform-specific installation instructions.

This plan covers the **client-side** implementation only (the `freminal` and `freminal-common`
crates, plus the deploy workflow). The update service itself is a separate repo and is described
in `PLAN_19_UPDATE_SERVICE_AND_WEBSITE.md`.

## Current State

- **Version:** `0.1.4` in workspace `Cargo.toml`; `flake.nix` has hardcoded `"0.1.0"` (out of
  sync). `CARGO_PKG_VERSION` is available at runtime via `env!()` and is already used in
  XTVERSION responses and the `TERM_PROGRAM_VERSION` env var.
- **No update checking exists.** Zero network calls, zero version comparison code.
- **No HTTP client dependency.** The crate graph has no HTTP library.
- **Config system:** `Config` struct in `freminal-common/src/config.rs` with layered loading
  (system → user → env → validate) and `ConfigPartial` for merging. `managed_by: Option<String>`
  is set to `"home-manager"` by the Nix module.
- **Home-manager module:** `nix/home-manager-module.nix` — always injects `managed_by =
"home-manager"`. Does not currently expose any update-related options.
- **Deploy workflow:** `.github/workflows/deploy.yaml` — 4 parallel build jobs (linux-amd64,
  linux-arm64, windows, macos-arm64). Raw binaries are uploaded uncompressed alongside packaged
  formats (.deb, .AppImage, .app.zip). No SHA256 checksums. No webhook.
- **Settings modal pattern:** `SettingsModal` in `freminal/src/gui/settings.rs` provides the
  template for modal dialogs: `open()/show()/is_open` pattern, draft state, `SettingsAction` enum.
- **GUI architecture:** Lock-free rendering via `ArcSwap<TerminalSnapshot>`. GUI sends events
  through `Sender<InputEvent>`. Background threads communicate via crossbeam channels.

## Design

### Config: `[update]` Section

Add a new section to `Config`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateConfig {
    /// Enable automatic update checking. Default: true.
    /// Forced to false when managed_by is set (home-manager, Nix).
    pub check_enabled: bool,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self { check_enabled: true }
    }
}
```

Add `pub update: UpdateConfig` to `Config`. In `Config::validate()`, when `self.managed_by` is
`Some(...)`, force `self.update.check_enabled = false` regardless of what the TOML says. This
ensures Nix-managed installs never phone home — the Nix derivation is the source of truth for
versioning.

### Home-Manager Module

The home-manager module does **not** expose an `update.check_enabled` option. The combination
of `managed_by = "home-manager"` in the generated TOML plus the `validate()` force-to-false
logic above is sufficient. The user cannot override this via Nix options.

### Install Method Detection

At runtime, Freminal detects how it was installed so the update dialog can display
platform-appropriate instructions:

```rust
pub enum InstallMethod {
    AppImage,        // $APPIMAGE env var is set
    Nix,             // binary path starts with /nix/store/
    HomeManager,     // config.managed_by.is_some()
    MacOsApp,        // binary path contains .app/Contents/MacOS/
    WindowsExe,      // cfg(target_os = "windows")
    DebPackage,      // binary at system path + dpkg check
    RawBinary,       // fallback
}
```

Detection is done once at startup. The result is stored and passed through to the update dialog.

### Update Check Cache File

To avoid checking on every launch, the last check result is cached in a JSON file:

```text
Linux:   $XDG_CACHE_HOME/freminal/update_check.json
macOS:   ~/Library/Caches/Freminal/update_check.json
Windows: %LOCALAPPDATA%\Freminal\cache\update_check.json
```

```json
{
  "last_check_time": "2026-03-29T12:00:00Z",
  "latest_version": "0.2.0",
  "download_url": "https://github.com/fredsystems/freminal/releases/tag/v0.2.0",
  "dismissed_version": null
}
```

- `last_check_time`: RFC 3339 timestamp. If < 24 hours ago, skip the HTTP request.
- `dismissed_version`: When the user dismisses the update notification, this is set to the
  offered version string. The notification does not reappear for that version.
- Cache file is best-effort: if it cannot be read/written, the check proceeds normally.

### Update Check HTTP Client

New workspace dependencies (added to `freminal` crate only, not library crates):

- `ureq` — blocking HTTP client, minimal dependencies, no async runtime
- `semver` — SemVer parsing and comparison

The check function:

```rust
fn check_for_update(current_version: &str) -> Result<Option<UpdateInfo>, UpdateCheckError> {
    let response = ureq::get("https://updates.freminal.dev/v1/latest.json")
        .timeout(std::time::Duration::from_secs(5))
        .call()?;

    let info: LatestRelease = response.body_mut().read_json()?;
    let current = semver::Version::parse(current_version)?;
    let latest = semver::Version::parse(&info.version)?;

    if latest > current {
        Ok(Some(UpdateInfo {
            version: info.version,
            download_url: info.download_url,
            release_notes_url: info.release_notes_url,
            assets: info.assets,
        }))
    } else {
        Ok(None)
    }
}
```

### Service API Contract

The update check hits `GET https://updates.freminal.dev/v1/latest.json` which returns:

```json
{
  "version": "0.2.0",
  "tag": "v0.2.0",
  "published_at": "2026-03-29T12:00:00Z",
  "download_url": "https://github.com/fredsystems/freminal/releases/tag/v0.2.0",
  "release_notes_url": "https://github.com/fredsystems/freminal/releases/tag/v0.2.0",
  "assets": [
    {
      "name": "freminal-0.2.0-linux-amd64.tar.gz",
      "url": "https://github.com/fredsystems/freminal/releases/download/v0.2.0/freminal-0.2.0-linux-amd64.tar.gz",
      "size": 12345678,
      "sha256": "abc123..."
    }
  ],
  "sha256sums_url": "https://github.com/fredsystems/freminal/releases/download/v0.2.0/SHA256SUMS"
}
```

### Background Check Thread

The update check runs on a dedicated background thread spawned from `main.rs` after the
config is loaded and before `gui::run()` is called. It communicates with the GUI via a
`crossbeam_channel::bounded::<UpdateNotification>(1)` channel.

```text
main.rs startup
  ├── load config
  ├── detect install method
  ├── if update.check_enabled && !is_managed:
  │     spawn update_check_thread(current_version, cache_path, update_tx)
  ├── create emulator, channels, etc.
  └── gui::run(..., update_rx, install_method)
```

The background thread:

1. Reads the cache file. If `last_check_time` < 24h ago and the cache is valid, uses cached data.
2. Otherwise, calls `check_for_update()`.
3. Writes updated cache file.
4. If an update is available and `dismissed_version` does not match, sends `UpdateNotification`
   through the channel.

### Menu Bar Update Indicator

When `FreminalGui` receives an `UpdateNotification` via `try_recv()` on its `update_rx`:

- A small label "Update available" (or similar) appears in the menu bar, right-aligned.
- Clicking it opens the update dialog.

The indicator persists for the lifetime of the session (it does not disappear after being shown).

### Update Dialog Modal

Modeled after `SettingsModal`. An `UpdateModal` struct with `open()/show()/is_open`:

- Displays: new version number, release notes URL (clickable link via `open` crate), and
  install-method-specific instructions.
- **Download button:** Downloads the appropriate asset for the current platform to `~/Downloads`
  using `ureq`. Shows a progress indication (or spinner, since ureq is blocking — the download
  runs on a background thread with progress sent via channel).
- **Dismiss button:** Sets `dismissed_version` in the cache file and closes the modal. The
  menu bar indicator disappears for this version.
- **Install instructions** vary by `InstallMethod`:
  - `AppImage`: "Replace your current AppImage with the downloaded file"
  - `Nix/HomeManager`: "Update your flake input and rebuild" (should not reach here since
    check is disabled, but defensive)
  - `MacOsApp`: "Drag the new .app to /Applications, replacing the old one"
  - `WindowsExe`: "Replace the current exe with the downloaded file"
  - `DebPackage`: "Install with: sudo dpkg -i filename"
  - `RawBinary`: "Replace the current binary with the downloaded file"

### Asset Selection

The download button selects the correct asset based on:

- `cfg(target_os)` + `cfg(target_arch)` to determine the platform key
- Preferred format: `.deb` for Debian-detected, `.AppImage` for AppImage-detected, `.app.zip`
  for macOS, `.zip` for Windows, `.tar.gz` for raw binary

### Deploy Workflow Changes

#### Compress Raw Binaries

Replace the raw binary copies in each build job with compressed versions:

- Linux: `tar czf dist/freminal-${VERSION}-linux-amd64.tar.gz -C target/release freminal`
- macOS: `tar czf dist/freminal-${VERSION}-macos-arm64.tar.gz -C target/release freminal`
- Windows: PowerShell `Compress-Archive` to create `.zip`

This reduces asset sizes from ~38 MB to ~10-12 MB.

#### SHA256SUMS

Add a step after all artifacts are downloaded in the `release` job:

```yaml
- name: Generate SHA256SUMS
  run: |
    cd artifacts
    sha256sum * > SHA256SUMS
```

The `SHA256SUMS` file is included in the release assets.

#### Webhook to Update Service

After the release is created, send a cache invalidation webhook to the update service:

```yaml
- name: Invalidate update service cache
  run: |
    curl -X POST https://updates.freminal.dev/v1/invalidate \
      -H "Authorization: Bearer ${{ secrets.UPDATE_SERVICE_TOKEN }}" \
      -H "Content-Type: application/json" \
      -d '{"tag": "${{ github.ref_name }}"}'
```

### Fix flake.nix Version Sync

The `version = "0.1.0"` in `flake.nix` line 77 is out of sync with the workspace `Cargo.toml`
(`0.1.4`). Fix by reading it from `Cargo.toml` at build time:

```nix
version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).workspace.package.version;
```

## Affected Files

| File                                    | Change                                                                             |
| --------------------------------------- | ---------------------------------------------------------------------------------- |
| `freminal-common/src/config.rs`         | Add `UpdateConfig` struct, `update` field on `Config`, `ConfigPartial`, validation |
| `freminal-common/Cargo.toml`            | Add `serde_json` dependency (for cache file)                                       |
| `config_example.toml`                   | Add `[update]` section with documentation                                          |
| `nix/home-manager-module.nix`           | No changes (managed_by + validate() handles it)                                    |
| `freminal/Cargo.toml`                   | Add `ureq`, `semver`, `serde_json` dependencies                                    |
| `freminal/src/update/mod.rs`            | New module: install detection, cache, HTTP check, types                            |
| `freminal/src/update/cache.rs`          | Cache file read/write                                                              |
| `freminal/src/update/check.rs`          | HTTP check + version comparison                                                    |
| `freminal/src/update/install_method.rs` | Install method detection                                                           |
| `freminal/src/gui/update_modal.rs`      | Update dialog modal UI                                                             |
| `freminal/src/gui/mod.rs`               | Add update indicator to menu bar, wire update_rx, add update_modal                 |
| `freminal/src/main.rs`                  | Spawn background check thread, create update channel, pass to GUI                  |
| `.github/workflows/deploy.yaml`         | Compress binaries, SHA256SUMS, webhook                                             |
| `flake.nix`                             | Fix hardcoded version                                                              |
| `Cargo.toml` (workspace)                | Add `ureq`, `semver`, `serde_json` to workspace deps                               |

## Subtasks

- [ ] **18.1** Add `[update]` config section
  - Add `UpdateConfig` struct to `freminal-common/src/config.rs` with `check_enabled: bool`
    (default `true`).
  - Add `pub update: UpdateConfig` to `Config` and `Option<UpdateConfig>` to `ConfigPartial`.
  - Update `apply_partial()`, `Default`, and `validate()` (force `check_enabled = false` when
    `managed_by` is `Some`).
  - Add `[update]` section to `config_example.toml`.
  - Add tests: default is `true`, managed_by forces `false`, TOML round-trip.
  - **Verify:** `cargo test --all` passes.

- [ ] **18.2** Confirm home-manager behavior
  - Verify that the generated TOML from the home-manager module, combined with the
    `validate()` force-to-false logic, correctly disables update checking.
  - No code changes to the Nix module needed (it always sets `managed_by = "home-manager"`).
  - Add a test: load a TOML string with `managed_by = "home-manager"` and
    `update.check_enabled = true`, call `validate()`, assert `check_enabled` is `false`.
  - **Verify:** `cargo test --all` passes.

- [ ] **18.3** Add install method detection module
  - Create `freminal/src/update/install_method.rs` with `InstallMethod` enum and
    `detect_install_method(config: &Config) -> InstallMethod`.
  - Detection logic: `$APPIMAGE` env → `AppImage`; `/nix/store/` path → `Nix`;
    `config.is_managed()` → `HomeManager`; `.app/Contents/MacOS/` → `MacOsApp`;
    `cfg(windows)` → `WindowsExe`; system path + dpkg → `DebPackage`; fallback → `RawBinary`.
  - Add tests with mocked env/paths where possible.
  - **Verify:** `cargo test --all` passes.

- [ ] **18.4** Add update check types and cache file support
  - Create `freminal/src/update/cache.rs` with `UpdateCache` struct (JSON serde),
    `read_cache()`, `write_cache()`, `cache_dir()`.
  - Create `freminal/src/update/mod.rs` with `UpdateInfo`, `UpdateNotification`,
    `UpdateCheckError` types.
  - Add `serde_json` to workspace deps and `freminal/Cargo.toml`.
  - Add tests: cache round-trip, expired cache detection, missing file handling.
  - **Verify:** `cargo test --all` passes.

- [ ] **18.5** Add update check HTTP client
  - Create `freminal/src/update/check.rs` with `check_for_update()` function.
  - Add `ureq` and `semver` to workspace deps and `freminal/Cargo.toml`.
  - Function hits `GET https://updates.freminal.dev/v1/latest.json` with 5-second timeout.
  - Parses response JSON, compares versions with `semver`, returns `Option<UpdateInfo>`.
  - Add unit tests with mock responses (test version comparison logic, not actual HTTP).
  - **Verify:** `cargo test --all` passes.

- [ ] **18.6** Add background check thread and channel to GUI
  - In `freminal/src/main.rs`, after config load and install method detection:
    if `check_enabled` and not managed, spawn a thread that runs the full check flow
    (read cache → HTTP if expired → write cache → send notification if update available).
  - Create `crossbeam_channel::bounded::<UpdateNotification>(1)` and pass `update_rx` to
    `gui::run()`.
  - Add `update_rx: Receiver<UpdateNotification>` and `install_method: InstallMethod` fields
    to `FreminalGui`.
  - **Verify:** `cargo test --all` passes. `cargo clippy` clean.

- [ ] **18.7** Add menu bar update indicator
  - In `FreminalGui::show_menu_bar()`, check `update_rx.try_recv()` once per frame (store
    the result in a field so it persists).
  - When an update is available, show a right-aligned "Update available (vX.Y.Z)" label.
  - Clicking the label opens the `UpdateModal`.
  - **Verify:** `cargo test --all` passes. Manual: indicator appears when channel has data.

- [ ] **18.8** Add update dialog modal
  - Create `freminal/src/gui/update_modal.rs` modeled after `SettingsModal`.
  - Displays: version, release notes link, platform-specific install instructions based on
    `InstallMethod`.
  - Download button: spawns background thread to download the correct asset to `~/Downloads`
    via `ureq`, sends progress/completion through a channel.
  - Dismiss button: writes `dismissed_version` to cache file, closes modal, hides indicator.
  - Wire into `FreminalGui::ui()` similar to how `settings_modal.show()` is called.
  - **Verify:** `cargo test --all` passes. `cargo clippy` clean.

- [ ] **18.9** Deploy workflow: compress raw binaries
  - In each build job in `.github/workflows/deploy.yaml`, replace the raw `cp` of the
    binary with:
    - Linux: `tar czf dist/freminal-${VERSION}-linux-{arch}.tar.gz -C target/release freminal`
    - macOS: `tar czf dist/freminal-${VERSION}-macos-arm64.tar.gz -C target/release freminal`
    - Windows: `Compress-Archive -Path target/release/freminal.exe -DestinationPath dist/freminal-${VERSION}-windows-amd64.zip`
  - Remove the raw uncompressed binary copy from each job.
  - **Verify:** Workflow YAML is valid. Manual: push a test tag and verify artifacts.

- [ ] **18.10** Deploy workflow: SHA256SUMS and webhook
  - In the `release` job, after downloading artifacts:
    - Generate `SHA256SUMS` file: `cd artifacts && sha256sum * > SHA256SUMS`
    - Add `SHA256SUMS` to the release assets (already picked up by `files: artifacts/*`).
  - After release creation, add a step to POST to
    `https://updates.freminal.dev/v1/invalidate` with the tag name and a bearer token from
    `secrets.UPDATE_SERVICE_TOKEN`.
  - **Verify:** Workflow YAML is valid.

- [ ] **18.11** Fix flake.nix version sync
  - Change the hardcoded `version = "0.1.0"` in `flake.nix` to read from workspace
    `Cargo.toml`:

    ```nix
    version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).workspace.package.version;
    ```

  - **Verify:** `nix build` succeeds. Version in built package matches `Cargo.toml`.

## Verification

- `cargo test --all` passes
- `cargo clippy --all-targets --all-features -- -D warnings` clean
- `cargo-machete` clean
- Manual: launch Freminal, verify no update check when `check_enabled = false`
- Manual: launch Freminal with `check_enabled = true`, verify background check runs
- Manual: verify update indicator appears in menu bar when update is available
- Manual: verify download to ~/Downloads works
- Manual: verify dismiss persists across restarts (for same version)
- Manual: verify home-manager config disables checking
