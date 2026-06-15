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

### Hack

- **Upstream:** <https://github.com/source-foundry/Hack>
- **License:** MIT (Hack project) plus the Bitstream Vera License for the
  Bitstream-derived components — Copyright 2018 Source Foundry Authors;
  Bitstream Vera Sans Mono Copyright 2003 Bitstream Inc.
- **License text:** [`res/fonts/Hack-LICENSE.md`](res/fonts/Hack-LICENSE.md)
- **Files:** `res/Hack-{Regular,Bold,Italic,BoldItalic}.ttf`

### MesloLGS Nerd Font Mono

- **Upstream:** Nerd Fonts — <https://github.com/ryanoasis/nerd-fonts>
  (Meslo LG patched); base typeface Meslo LG by André Berg —
  <https://github.com/andreberg/Meslo-Font>
- **License:** SIL Open Font License 1.1 for the bundled patched font files
  (Copyright (c) 2014 Ryan L McIntyre); the underlying Meslo LG typeface is
  Apache-2.0; the Nerd Fonts patcher source (not bundled) is MIT.
- **License text:**
  [`res/fonts/MesloLGS-NerdFont-LICENSE.md`](res/fonts/MesloLGS-NerdFont-LICENSE.md)
- **Files:** `res/MesloLGSNerdFontMono-{Regular,Bold,Italic,BoldItalic}.ttf`

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
