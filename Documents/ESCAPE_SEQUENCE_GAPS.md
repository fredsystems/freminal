# Escape Sequence Gaps

Last updated: 2025-11-09 — Generated from Freminal snapshot

This document lists ANSI / DEC / xterm / iTerm2 / WezTerm escape sequences **not yet fully implemented in Freminal**.
It serves as a roadmap for improving compatibility and feature parity with modern terminals.

---

## Summary

Freminal already covers the majority of core VT100–VT520 sequences:

- ✅ Complete cursor motion, SGR color/attribute, and screen manipulation commands
- ✅ Full DEC private mode toggles for wrapping, cursor visibility, and alt buffers
- ✅ Baseline OSC 0/2 window title support
- ✅ Modern color support (256 + TrueColor)

However, several higher-level, modern, and legacy extensions remain to be implemented.
These are categorized below by **importance** and **implementation difficulty**.

Legend:

- **Importance:** 🟩 High (affects interoperability) | 🟨 Medium | ⬜ Low / optional
- **Difficulty:** ⚙️ Trivial | 🧩 Moderate | 🧠 Complex

---

## Missing or Partial Sequences

| Category      | Sequence                                           | Importance | Difficulty | Description / Notes                                       |
| ------------- | -------------------------------------------------- | ---------- | ---------- | --------------------------------------------------------- |
| **OSC**       | `OSC 52 ; c ; data BEL`                            | 🟩         | 🧩         | Clipboard copy/paste (used by shells like zsh, vim, tmux) |
| **OSC**       | `OSC 8 ; params ; URI BEL`                         | 🟩         | 🧩         | Hyperlinks (supported by WezTerm, Kitty, iTerm2)          |
| **OSC**       | `OSC 4 ; index ; rgb BEL`                          | 🟨         | 🧩         | Dynamic palette change; optional color table updates      |
| **OSC**       | `OSC 10/11 ; ? BEL`                                | 🟨         | 🧩         | Foreground/background color queries                       |
| **OSC**       | `OSC 777`                                          | ⬜         | ⚙️         | Konsole notification command (rarely used)                |
| **CSI**       | `CSI M` – DL (Delete Line)                         | 🟩         | ⚙️         | Required for curses/tui redraws                           |
| **CSI**       | `CSI P` – DCH (Delete Character)                   | 🟩         | ⚙️         | Basic editing; required for full curses support           |
| **CSI**       | `CSI @` – ICH (Insert Character)                   | 🟨         | ⚙️         | Used by editors for mid-line inserts                      |
| **CSI**       | `CSI ' 'q` – DECLL (Load LEDs)                     | ⬜         | 🧠         | Keyboard LED controls (legacy)                            |
| **CSI**       | `CSI ? 1 ; 2 c` – DA2                              | 🟨         | 🧩         | Secondary device attributes query                         |
| **CSI**       | `CSI 6 n` – DSR (Report cursor pos)                | 🟩         | ⚙️         | Used by readline, editors, and shells                     |
| **CSI**       | `CSI > Ps n` – Device status reports               | 🟨         | 🧩         | Optional, informational only                              |
| **CSI**       | `CSI Ps ; Ps s` – DECSLRM (Set left/right margins) | 🟨         | 🧩         | Useful for region-based scrolling                         |
| **ESC**       | `ESC # 8` – DECALN (Screen alignment test)         | ⬜         | ⚙️         | Optional test pattern                                     |
| **ESC**       | `ESC M` – Reverse Index                            | 🟩         | ⚙️         | Needed for proper scroll-up in alternate screen           |
| **OSC/Kitty** | `OSC 1337` extensions                              | 🟨         | 🧠         | iTerm2 / WezTerm graphics, file transfer, notifications   |
| **Mouse**     | `CSI < … M/m` – SGR mouse reporting                | 🟩         | 🧩         | Enables accurate mouse events                             |
| **Mouse**     | `CSI M …` – X10/X11 mouse reporting                | 🟨         | 🧩         | Basic mouse support for ncurses apps                      |
| **Graphics**  | Sixel (`ESC P q …`)                                | ⬜         | 🧠         | Raster graphics; large undertaking                        |
| **FTCS**      | `OSC 133 …` – FinalTerm Control Sequences          | ⬜         | 🧩         | Rarely used, but modern UIs recognize them                |
| **Other**     | `CSI Ps t` – Window ops (resize/query)             | 🟨         | 🧩         | Terminal geometry interactions                            |

---

## Roadmap by Priority

### 🟩 High-Priority (Compatibility / Daily-Use)

| Sequence                   | Rationale                                  |
| -------------------------- | ------------------------------------------ |
| OSC 52 (clipboard)         | Needed for copy/paste in tmux/vim/zsh      |
| OSC 8 (hyperlinks)         | Common in modern terminals                 |
| CSI M/P (Delete Line/Char) | Required by curses-based apps              |
| CSI 6n (DSR)               | Shells rely on cursor position reports     |
| ESC M (Reverse Index)      | Needed for smooth scrollback/up            |
| Mouse Tracking (1000–1006) | Essential for TUI apps like htop, mc, etc. |

### 🟨 Medium-Priority (UX / Feature Parity)

| Sequence            | Rationale                           |
| ------------------- | ----------------------------------- |
| OSC 4 / 10 / 11     | Dynamic color / theme awareness     |
| DA/DA2 / >n queries | Help programs identify Freminal     |
| DECSLRM             | Improves ncurses region behavior    |
| OSC 1337 subset     | iTerm2/WezTerm integration features |

### ⬜ Low-Priority / Optional

| Sequence    | Rationale                           |
| ----------- | ----------------------------------- |
| OSC 777     | Konsole notification support (rare) |
| Sixel       | Heavy lift, limited demand          |
| FTCS        | Experimental / niche adoption       |
| DECLL / LED | Legacy hardware control             |

---

## Implementation Hints

- **OSC 52**: Minimal: encode/decode base64 payloads and forward to system clipboard provider (Wayland/X11/macOS).
- **OSC 8**: Track hyperlink start/end; store URL + text range metadata for GUI.
- **Mouse Reporting**: Already partially scaffolded — wire GUI → PTY event channel.
- **CSI M/P**: Integrate with text buffer’s scroll/edit routines; straightforward.
- **DSR (6n)**: Respond via PTY write with `ESC [ row ; col R`.

---

## Strategic Notes

- Freminal already aligns closely with **xterm**’s functional core.
- The **gap area** is largely about modern conveniences (OSC, mouse, rich hyperlinks).
- Filling the high-priority items (OSC 52, OSC 8, CSI M/P, DSR, Reverse Index, Mouse Tracking)
  will deliver near-complete compatibility with most applications and shells.
- Medium-priority items bring parity with **WezTerm/iTerm2** quality-of-life features.

---

© 2025 Freminal Project — MIT License.
