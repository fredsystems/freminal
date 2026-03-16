# PLAN 17 — Update README

## Overview

Update the README to reflect the current state of the project. The existing README is stale
in several areas: missing features (theming, image protocol, font ligatures, scrollback),
outdated descriptions ("Full theming system planned" — theming is complete), and missing
install instructions for non-Nix users.

## Current State (what's wrong)

- "Beautiful Catppuccin Theme" section says "Full theming system planned" — theming is
  complete with 25 built-in themes.
- Features section missing: font ligatures, inline images (iTerm2 + Kitty), primary screen
  scrollback, session recording/playback, 25 built-in color themes, TOML config system,
  settings modal with live preview.
- "Modern Rendering Pipeline" description is outdated — now uses custom OpenGL renderer with
  glyph atlas, rustybuzz shaping, and swash rasterization (not raw egui text rendering).
- CLI options table missing `--with-playback-file`.
- Architecture diagram is oversimplified — doesn't mention lock-free ArcSwap snapshot model.
- No install instructions for: Nix flake (`nix run`), `.deb` packages, Windows binary, cargo
  install from source.
- "Getting Started" only covers development, not end-user installation.
- Project status section at bottom is minimal.
- Links to docs that may not exist or may be stale.
- Badge URLs point to `fredclausen` org but repo is now under `fredsystems`.

## Design

### Structure

1. **Header** — name, tagline, badges
2. **Features** — updated feature list with all current capabilities
3. **Installation** — end-user install instructions (Nix, deb, Windows, cargo install)
4. **Configuration** — brief config overview with link to `config_example.toml`
5. **CLI Options** — updated table with all current flags
6. **Development** — build from source, run tests, dev shell setup
7. **Architecture** — updated high-level diagram
8. **Documentation** — links to detailed docs
9. **Contributing** — contribution guidelines
10. **License** — MIT

### Badge Updates

Update badge URLs from `fredclausen/freminal` to `fredsystems/freminal` if the repo has
moved. Verify current org name.

### Install Instructions to Add

- **Nix flake**: `nix run github:fredsystems/freminal`
- **Nix home-manager**: brief snippet or link to `config_example.toml`
- **Debian/Ubuntu**: download `.deb` from releases (once Task 16 is done)
- **Windows**: download `.exe` from releases (once Task 16 is done)
- **From source**: `cargo install --git`

## Affected Files

| File        | Change       |
| ----------- | ------------ |
| `README.md` | Full rewrite |

## Subtasks

- [ ] **17.1** Verify current GitHub org/repo URL and badge correctness
- [ ] **17.2** Rewrite README with updated structure, features, install instructions,
      CLI table, architecture, and links
- [ ] **17.3** Review for accuracy against current codebase

## Verification

- All links in README are valid (no broken references)
- CLI options match `freminal --help` output
- Feature descriptions match actual implemented features
- No stale "planned" language for completed features
- `cargo test --all` passes (no code changes)
