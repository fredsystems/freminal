# PLAN 16 — GitHub Actions for Building and Releasing

## Overview

Set up CI/CD to build release artifacts and publish them as GitHub Releases. The workflow
triggers on version tags (`v*`) and creates a GitHub Release with prebuilt binaries and
packages.

## Current State

- `.github/workflows/deploy.yaml` exists but is half-done:
  - Manual trigger only (`workflow_dispatch`) — no automatic runs on tags.
  - Linux AMD64/ARM64: Uses `cargo-bundle` for `.deb` packaging, uploads as artifacts only.
  - Windows: `cargo-bundle` commented out (cargo-bundle issue #77), just `cargo build --release`.
  - macOS: Entirely commented out — had code-signing skeleton with Apple Developer ID.
  - No GitHub Release creation, no git tagging logic, no `softprops/action-gh-release`.
- `.github/workflows/ci.yml` exists and runs on PRs with multi-OS matrix.
- `freminal/Cargo.toml` has `[package.metadata.bundle]` with `version = "1.0.0"` (should
  match workspace version `0.1.0`).
- `secrets/public_key.txt` listed as bundle resource — likely unintentional for release.

## Design

### Trigger

```yaml
on:
  push:
    tags:
      - "v*"
  workflow_dispatch:
```

Tags like `v0.1.0` trigger the workflow. Manual dispatch is retained for testing.

### Phase 1: Linux + Windows (initial implementation)

#### Linux (AMD64 + ARM64)

- Build with `cargo build --release`
- Package with `cargo-bundle` for `.deb`
- Upload both the `.deb` and the raw binary as release assets
- Artifact naming: `freminal-<version>-linux-amd64.deb`, `freminal-<version>-linux-amd64`,
  `freminal-<version>-linux-arm64.deb`, `freminal-<version>-linux-arm64`

#### Windows

- Build with `cargo build --release`
- Upload `freminal.exe` as release asset
- Artifact naming: `freminal-<version>-windows-amd64.exe`

### Phase 2: macOS (deferred)

macOS `.app` bundling and code signing had problems. Deferred to a future task.
The workflow will include a commented-out macOS job as a placeholder.

### GitHub Release

Use `softprops/action-gh-release` to:

- Create a GitHub Release from the tag
- Attach all platform artifacts
- Auto-generate release notes from commits

### Version Extraction

Extract version from the git tag (`GITHUB_REF_NAME` strips `refs/tags/`):

```yaml
- name: Extract version
  run: echo "VERSION=${GITHUB_REF_NAME#v}" >> $GITHUB_ENV
```

### Bundle Metadata Fix

Fix `freminal/Cargo.toml` `[package.metadata.bundle]`:

- Remove hardcoded `version = "1.0.0"` (cargo-bundle reads from `[package]` automatically)
- Remove `secrets/public_key.txt` from resources if not intentional
- Uncomment and verify `deb_depends` for required runtime libraries

## Affected Files

| File                            | Change                                                         |
| ------------------------------- | -------------------------------------------------------------- |
| `.github/workflows/deploy.yaml` | Rewrite: tag trigger, release creation, proper artifact naming |
| `freminal/Cargo.toml`           | Fix `[package.metadata.bundle]` version and resources          |

## Subtasks

- [ ] **16.1** Fix `[package.metadata.bundle]` in `freminal/Cargo.toml`:
      remove hardcoded version, clean up resources, verify deb_depends
- [ ] **16.2** Rewrite `.github/workflows/deploy.yaml`:
  - Tag trigger (`v*`) + `workflow_dispatch`
  - Linux AMD64 job: build, bundle .deb, upload artifacts with versioned names
  - Linux ARM64 job: build, bundle .deb, upload artifacts with versioned names
  - Windows job: build, upload .exe with versioned name
  - Release job: depends on all build jobs, uses `softprops/action-gh-release`
    to create release and attach all artifacts
  - macOS job: commented-out placeholder
- [ ] **16.3** Verify workflow syntax with `actionlint` or manual review
- [ ] **16.4** Test with a manual `workflow_dispatch` run (if possible) or verify
      the YAML is syntactically correct

## Verification

- Workflow YAML is syntactically valid
- `cargo build --release` succeeds locally
- `cargo-machete` clean
- `cargo test --all` passes (no code changes that affect tests)
