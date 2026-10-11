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
  emoji font. The bundled face is always used for the terminal grid; system emoji
  fonts are intentionally not consulted, because a system font's aggregate coverage
  is not a stable guarantee that it maps any particular codepoint (issue #431).

## Bundled images

All bundled image assets are first-party to the freminal project and are
covered by freminal's own [MIT License](LICENSE):

- Application/window icon — `assets/icon.png`
- Logo — `assets/logo.png`
- macOS bundle icon — `assets/macos/freminal.icns`

## Data tables

### X11 colour names (rgb.txt)

The table of X11 colour names in
[`freminal-common/src/colors/x11_names.rs`](freminal-common/src/colors/x11_names.rs)
is transcribed by script from the X.Org `rgb.txt` colour database.

- Upstream: <https://gitlab.freedesktop.org/xorg/app/rgb>
- File: `rgb.txt` at commit `2b2019cabfe5697f54fde2f95c5d00395909458f`
- Retrieved: 2026-10-11
- Licence: permissive notices of The Open Group and Oracle (MIT-style); the
  copyright and permission notices from the upstream `COPYING` file are
  reproduced in full below, as the licence requires.

```text
Copyright 1985, 1989, 1998  The Open Group

Permission to use, copy, modify, distribute, and sell this software and its
documentation for any purpose is hereby granted without fee, provided that
the above copyright notice appear in all copies and that both that
copyright notice and this permission notice appear in supporting
documentation.

The above copyright notice and this permission notice shall be included
in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS
OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.
IN NO EVENT SHALL THE OPEN GROUP BE LIABLE FOR ANY CLAIM, DAMAGES OR
OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE,
ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR
OTHER DEALINGS IN THE SOFTWARE.

Except as contained in this notice, the name of The Open Group shall
not be used in advertising or otherwise to promote the sale, use or
other dealings in this Software without prior written authorization
from The Open Group.

Copyright (c) 1994, 2008, Oracle and/or its affiliates.

Permission is hereby granted, free of charge, to any person obtaining a
copy of this software and associated documentation files (the "Software"),
to deal in the Software without restriction, including without limitation
the rights to use, copy, modify, merge, publish, distribute, sublicense,
and/or sell copies of the Software, and to permit persons to whom the
Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice (including the next
paragraph) shall be included in all copies or substantial portions of the
Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.  IN NO EVENT SHALL
THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
```

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
