# Attributions

Freminal is licensed under the [MIT License](LICENSE). It also vendors source
code and bundles font and image assets from third parties whose licenses
require attribution. Those components and their licenses are listed below.

This document is the single source of truth for attribution: the README and
the in-app About dialog both point here rather than duplicating license text.

## Vendored source code

### portable-pty

- **Upstream:** WezTerm — <https://github.com/wezterm/wezterm> (crate `pty/`)
- **Version:** 0.9.1
- **License:** MIT — Copyright (c) 2018 Wez Furlong
- **Location:** vendored in-tree at [`portable-pty/`](portable-pty/); the full
  license text is in [`portable-pty/LICENSE`](portable-pty/LICENSE) and the
  local modifications are logged in
  [`portable-pty/vendored.md`](portable-pty/vendored.md).

## Bundled fonts

The following fonts are embedded into the freminal binary at compile time. The
full upstream license texts are bundled under [`res/fonts/`](res/fonts/).

### CaskaydiaCove Nerd Font

- **Upstream:** Nerd Fonts — <https://github.com/ryanoasis/nerd-fonts> (Cascadia
  Code patched and renamed to "CaskaydiaCove"); base typeface Cascadia Code by
  Microsoft — <https://github.com/microsoft/cascadia-code>
- **License:** SIL Open Font License 1.1. The base typeface reserves the name
  "Cascadia Code" under the OFL Reserved Font Name clause; the Nerd Fonts patch
  renames the bundled faces to "CaskaydiaCove" to comply.
- **License text:**
  [`res/fonts/CaskaydiaCove-NerdFont-LICENSE.md`](res/fonts/CaskaydiaCove-NerdFont-LICENSE.md)
- **Files:** `res/CaskaydiaCoveNerdFont-{Regular,Bold,Italic,BoldItalic}.ttf`

### Noto Color Emoji

- **Upstream:** Google Noto Fonts —
  <https://github.com/googlefonts/noto-emoji>
- **License:** SIL Open Font License 1.1.
- **License text:**
  [`res/fonts/NotoColorEmoji-LICENSE.txt`](res/fonts/NotoColorEmoji-LICENSE.txt)
- **Files:** `res/NotoColorEmoji.ttf`
- **Notes:** Bundled so color emoji render without depending on a system-installed
  emoji font. When a suitable system color-emoji font is present it is preferred;
  the bundled face is the guaranteed fallback.

## Bundled images

All bundled image assets are first-party to the freminal project and are
covered by freminal's own [MIT License](LICENSE):

- Application/window icon — `assets/icon.png`
- Logo — `assets/logo.png`
- macOS bundle icon — `assets/macos/freminal.icns`

## Color themes

Freminal ships color palettes adapted from a number of community theme
projects (Catppuccin, Dracula, Nord, Solarized, Gruvbox, Tokyo Night,
Kanagawa, Rosé Pine, Everforest, Ayu, One Dark/Light, Material, Monokai Pro,
WezTerm, Ghostty, and the xterm defaults). Each palette is attributed at its
definition site in
[`freminal-common/src/themes.rs`](freminal-common/src/themes.rs) with its
upstream source and license (most are MIT). Only numeric color values are
reproduced; no upstream theme source is vendored.

## Rust dependencies

Freminal depends on a graph of permissively licensed Rust crates resolved
through Cargo. These are not enumerated individually here; their licenses are
constrained by the allowlist in [`deny.toml`](deny.toml) and enforced by
`cargo deny`. A full software bill of materials can be generated with
`cargo deny list` or `cargo license`.
