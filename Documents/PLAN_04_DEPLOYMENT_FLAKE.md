# PLAN_04 — Deployment Flake (Nix + Home-Manager Module)

## Overview

Extend the existing Nix flake with a home-manager module that generates `config.toml` from Nix
attributes, add an overlay for composability, and support multi-architecture builds.

**Dependencies:** Task 2 (CLI Args + TOML Config) — needs finalized config schema
**Dependents:** None
**Primary files:** `flake.nix`, new `nix/` directory
**Estimated scope:** Medium

---

## Problem Statement

The current `flake.nix` (123 lines) provides a package definition and devShell but no mechanism
for declarative configuration. NixOS and home-manager users must manually create and maintain a
`config.toml` file, losing the benefits of reproducible, type-checked configuration.

### Target User Experience

```nix
# In home-manager configuration
{
  programs.freminal = {
    enable = true;
    settings = {
      font = {
        family = "JetBrainsMono Nerd Font";
        size = 14.0;
      };
      cursor = {
        shape = "bar";
        blink = false;
      };
      theme.name = "catppuccin-mocha";
      scrollback.limit = 10000;
      shell.path = "/run/current-system/sw/bin/zsh";
      logging.write_to_file = false;
    };
  };
}
```

This should:

1. Install the `freminal` package
2. Generate `~/.config/freminal/config.toml` with the specified settings
3. Validate settings at Nix evaluation time (type errors caught before build)

---

## Current State

### Existing `flake.nix` Capabilities

- Package definition using `crane` (Rust build)
- devShell with Rust toolchain, pre-commit hooks
- No NixOS module
- No home-manager module
- No overlay
- No multi-arch matrix (only builds for build system arch)

### Deploy Workflow

- GitHub Actions builds Linux AMD64/ARM64 + Windows
- macOS is commented out
- Uses Nix for Linux builds

---

## Subtasks

### 4.1 — Create Nix module directory structure

- **Status:** Complete
- **Scope:** New `nix/` directory
- **Details:**
  - Create `nix/home-manager-module.nix` — home-manager module
  - Create `nix/overlay.nix` — package overlay
  - Keep `flake.nix` as the entry point that imports from `nix/`
  - Directory structure:

    ```text
    nix/
    ├── home-manager-module.nix
    └── overlay.nix
    ```

- **Acceptance criteria:** Directory structure exists, files are importable

### 4.2 — Implement Nix overlay

- **Status:** Complete
- **Scope:** `nix/overlay.nix`, `flake.nix`
- **Details:**
  - Create overlay that adds `freminal` to `pkgs`
  - Export overlay from flake outputs: `overlays.default`
  - Overlay should compose with other overlays
  - Package should be accessible as `pkgs.freminal` when overlay is applied
- **Acceptance criteria:**
  - `overlays.default` is exported from flake
  - Overlay adds `freminal` to package set
  - Can be composed: `nixpkgs.overlays = [ freminal.overlays.default ];`
- **Tests required:**
  - Overlay evaluation doesn't fail
  - Package is accessible through overlay

### 4.3 — Implement home-manager module

- **Status:** Complete
- **Scope:** `nix/home-manager-module.nix`, `flake.nix`
- **Details:**
  - Module options:

    ```nix
    options.programs.freminal = {
      enable = mkEnableOption "Freminal terminal emulator";
      package = mkOption {
        type = types.package;
        default = pkgs.freminal;
        description = "The Freminal package to install";
      };
      settings = mkOption {
        type = types.submodule { ... };
        default = {};
        description = "Freminal configuration settings";
      };
    };
    ```

  - Settings submodule mirrors the TOML schema from Task 2:

    ```nix
    settings = {
      font.family = mkOption { type = types.str; default = "MesloLGS Nerd Font Mono"; };
      font.size = mkOption { type = types.float; default = 12.0; };
      cursor.shape = mkOption { type = types.enum ["block" "underline" "bar"]; default = "block"; };
      cursor.blink = mkOption { type = types.bool; default = true; };
      theme.name = mkOption { type = types.str; default = "catppuccin-mocha"; };
      scrollback.limit = mkOption { type = types.ints.between 1 100000; default = 4000; };
      shell.path = mkOption { type = types.nullOr types.str; default = null; };
      logging.write_to_file = mkOption { type = types.bool; default = false; };
    };
    ```

  - Config generation:
    - Convert `settings` attrset to TOML format
    - Write to `$XDG_CONFIG_HOME/freminal/config.toml` via `home.file`
    - Include `version = 1` header
  - Module activation (`config = mkIf cfg.enable { ... }`):
    - Add package to `home.packages`
    - Generate config file if any settings differ from defaults
  - Export from flake: `homeManagerModules.default`

- **Acceptance criteria:**
  - `programs.freminal.enable = true;` installs package and generates default config
  - Custom settings are correctly serialized to TOML
  - Type checking catches invalid settings at eval time (e.g., `cursor.shape = "invalid"`)
  - Module composes with standard home-manager setup
- **Tests required:**
  - Module evaluates without errors
  - Generated TOML matches expected output for known settings
  - Type validation rejects invalid values
  - Default settings produce valid config

### 4.4 — TOML generation from Nix attributes

- **Status:** Complete
- **Scope:** `nix/home-manager-module.nix`
- **Details:**
  - Implement Nix function to convert settings attrset to TOML string
  - Use `lib.generators.toTOML` if available in nixpkgs, otherwise implement manually
  - Handle nested sections: `font.family` → `[font]\nfamily = "..."`
  - Handle types correctly: strings quoted, numbers unquoted, bools lowercase
  - Add `version = 1` at the top
  - Handle null/missing values: omit from output (use defaults)
- **Acceptance criteria:**
  - Generated TOML is valid and loadable by Freminal
  - All value types serialize correctly
  - Missing optional values are omitted
  - Output is human-readable (not minified)
- **Tests required:**
  - Full settings generate complete TOML
  - Partial settings generate partial TOML (missing = defaults)
  - Null values are omitted
  - Generated TOML round-trips through Freminal's config loader

### 4.5 — Multi-architecture support

- **Status:** Complete (already supported via `precommit.lib.supportedSystems`)
- **Scope:** `flake.nix`
- **Details:**
  - Use `flake-utils` or `systems` to define supported architectures
  - Target systems: `x86_64-linux`, `aarch64-linux`, `x86_64-darwin`, `aarch64-darwin`
  - Package definitions should work for all targets
  - devShell should work for all targets
  - Cross-compilation support (stretch goal)
- **Acceptance criteria:**
  - `nix build .#packages.x86_64-linux.default` works
  - `nix build .#packages.aarch64-linux.default` works (or cross-compiles)
  - flake check passes on available architectures

### 4.6 — Update flake.nix to export new outputs

- **Status:** Complete
- **Scope:** `flake.nix`
- **Details:**
  - Add `homeManagerModules.default` output
  - Add `overlays.default` output
  - Keep existing outputs (packages, devShells, checks)
  - Update flake metadata/description if needed
- **Acceptance criteria:**
  - `nix flake show` lists all outputs correctly
  - `nix flake check` passes
  - All existing functionality preserved

### 4.7 — Documentation and examples

- **Status:** Complete
- **Scope:** Update `config_example.toml`, potentially README references
- **Details:**
  - Add Nix usage example to config_example.toml as a comment
  - Document home-manager module options
  - Document overlay usage
  - Provide example flake.nix for users who want to use Freminal via Nix
- **Acceptance criteria:**
  - Clear documentation for home-manager integration
  - Example configuration in comments

### 4.8 — Integration testing

- **Status:** Complete
- **Scope:** All Nix files
- **Details:**
  - Add flake check that evaluates home-manager module with test config
  - Verify generated TOML is valid
  - Run `nix flake check` as part of CI
  - Test with minimal config (just `enable = true`)
  - Test with full custom config
- **Acceptance criteria:**
  - `nix flake check` passes
  - Module evaluation tests pass
  - Generated configs are valid TOML

---

## Affected Files

| File                          | Change Type                         |
| ----------------------------- | ----------------------------------- |
| `flake.nix`                   | Extend with new outputs, multi-arch |
| `flake.lock`                  | Updated when inputs change          |
| `nix/home-manager-module.nix` | NEW — home-manager module           |
| `nix/overlay.nix`             | NEW — package overlay               |
| `config_example.toml`         | Add Nix usage examples              |

---

## Config Schema Mapping (Nix ↔ TOML)

| Nix Path                         | Nix Type                 | TOML Path               | TOML Type |
| -------------------------------- | ------------------------ | ----------------------- | --------- |
| `settings.font.family`           | `types.str`              | `font.family`           | string    |
| `settings.font.size`             | `types.float`            | `font.size`             | float     |
| `settings.cursor.shape`          | `types.enum`             | `cursor.shape`          | string    |
| `settings.cursor.blink`          | `types.bool`             | `cursor.blink`          | boolean   |
| `settings.theme.name`            | `types.str`              | `theme.name`            | string    |
| `settings.scrollback.limit`      | `types.ints.between`     | `scrollback.limit`      | integer   |
| `settings.shell.path`            | `types.nullOr types.str` | `shell.path`            | string    |
| `settings.logging.write_to_file` | `types.bool`             | `logging.write_to_file` | boolean   |

---

## Risk Assessment

| Risk                                            | Likelihood | Impact | Mitigation                                           |
| ----------------------------------------------- | ---------- | ------ | ---------------------------------------------------- |
| Schema drift between Nix module and Rust config | Medium     | High   | Generated from single source of truth where possible |
| macOS builds fail (commented out in deploy)     | Medium     | Medium | Test incrementally, Darwin may need extra deps       |
| TOML generation edge cases                      | Low        | Medium | Round-trip testing against Rust parser               |
| home-manager API changes                        | Low        | Low    | Pin nixpkgs version, test with stable                |
