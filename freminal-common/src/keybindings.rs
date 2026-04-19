// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Configurable key bindings for Freminal.
//!
//! Provides a data-driven keybinding system that maps keyboard shortcuts to
//! application actions. Users can customize bindings via the `[keybindings]`
//! section in `config.toml`.
//!
//! The keybinding types ([`BindingKey`], [`BindingModifiers`], [`KeyCombo`])
//! are framework-independent. Conversion from GUI framework key types
//! (e.g. `egui::Key`) happens in the binary crate's GUI layer, not here.
//!
//! # Default Bindings
//!
//! [`BindingMap::default()`] produces the set of bindings matching common
//! terminal emulator conventions:
//!
//! | Combo              | Action           |
//! |--------------------|------------------|
//! | `Ctrl+Shift+C`     | Copy             |
//! | `Ctrl+Shift+V`     | Paste            |
//! | `Ctrl+Shift+T`     | New Tab          |
//! | `Ctrl+Shift+W`     | Close Tab        |
//! | `Ctrl+Shift+,`     | Open Settings    |
//! | `Ctrl+Shift+N`     | New Window       |
//! | `Shift+PageUp`     | Scroll Page Up   |
//! | `Shift+PageDown`   | Scroll Page Down |
//!
//! See [`BindingMap::default()`] for the complete list.

use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

// ---------------------------------------------------------------------------
//  Error types
// ---------------------------------------------------------------------------

/// Errors that can occur when parsing key binding strings.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum KeyBindingError {
    /// The key name was not recognized.
    #[error("unknown key: \"{0}\"")]
    UnknownKey(String),

    /// The action name was not recognized.
    #[error("unknown action: \"{0}\"")]
    UnknownAction(String),

    /// A modifier name was not recognized.
    #[error("unknown modifier: \"{0}\"")]
    UnknownModifier(String),

    /// The key combo string was empty.
    #[error("empty key combo string")]
    EmptyCombo,
}

// ---------------------------------------------------------------------------
//  BindingKey
// ---------------------------------------------------------------------------

/// A keyboard key that can be part of a binding.
///
/// This is Freminal's own key representation, independent of any GUI framework.
/// Conversion from framework-specific key types (e.g. `egui::Key`) happens in
/// the GUI layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum BindingKey {
    // Letters
    A,
    B,
    C,
    D,
    E,
    F,
    G,
    H,
    I,
    J,
    K,
    L,
    M,
    N,
    O,
    P,
    Q,
    R,
    S,
    T,
    U,
    V,
    W,
    X,
    Y,
    Z,

    // Numbers (top row)
    Num0,
    Num1,
    Num2,
    Num3,
    Num4,
    Num5,
    Num6,
    Num7,
    Num8,
    Num9,

    // Function keys
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,

    // Navigation
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    Home,
    End,
    PageUp,
    PageDown,

    // Editing
    Insert,
    Delete,
    Backspace,

    // Whitespace / control
    Tab,
    Enter,
    Space,
    Escape,

    // Symbols
    Plus,
    Minus,
    Equals,
    Comma,
    Period,
    Semicolon,
    Colon,
    Slash,
    Backslash,
    OpenBracket,
    CloseBracket,
    Backtick,
    Quote,
    /// The pipe character `|` (Shift+Backslash on US keyboards).
    Pipe,
}

impl BindingKey {
    /// Returns `true` if this key produces a printable character that would
    /// be consumed by normal text entry (letters A–Z and digits 0–9).
    ///
    /// Used by the keybinding recorder to reject `Shift+letter` combos
    /// (which would hijack uppercase typing) — alphanumeric keys require
    /// at least Ctrl or Alt as a modifier for bindings.
    #[must_use]
    pub const fn is_alphanumeric(self) -> bool {
        matches!(
            self,
            Self::A
                | Self::B
                | Self::C
                | Self::D
                | Self::E
                | Self::F
                | Self::G
                | Self::H
                | Self::I
                | Self::J
                | Self::K
                | Self::L
                | Self::M
                | Self::N
                | Self::O
                | Self::P
                | Self::Q
                | Self::R
                | Self::S
                | Self::T
                | Self::U
                | Self::V
                | Self::W
                | Self::X
                | Self::Y
                | Self::Z
                | Self::Num0
                | Self::Num1
                | Self::Num2
                | Self::Num3
                | Self::Num4
                | Self::Num5
                | Self::Num6
                | Self::Num7
                | Self::Num8
                | Self::Num9
        )
    }

    /// Returns the canonical display name for this key.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::A => "A",
            Self::B => "B",
            Self::C => "C",
            Self::D => "D",
            Self::E => "E",
            Self::F => "F",
            Self::G => "G",
            Self::H => "H",
            Self::I => "I",
            Self::J => "J",
            Self::K => "K",
            Self::L => "L",
            Self::M => "M",
            Self::N => "N",
            Self::O => "O",
            Self::P => "P",
            Self::Q => "Q",
            Self::R => "R",
            Self::S => "S",
            Self::T => "T",
            Self::U => "U",
            Self::V => "V",
            Self::W => "W",
            Self::X => "X",
            Self::Y => "Y",
            Self::Z => "Z",
            Self::Num0 => "0",
            Self::Num1 => "1",
            Self::Num2 => "2",
            Self::Num3 => "3",
            Self::Num4 => "4",
            Self::Num5 => "5",
            Self::Num6 => "6",
            Self::Num7 => "7",
            Self::Num8 => "8",
            Self::Num9 => "9",
            Self::F1 => "F1",
            Self::F2 => "F2",
            Self::F3 => "F3",
            Self::F4 => "F4",
            Self::F5 => "F5",
            Self::F6 => "F6",
            Self::F7 => "F7",
            Self::F8 => "F8",
            Self::F9 => "F9",
            Self::F10 => "F10",
            Self::F11 => "F11",
            Self::F12 => "F12",
            Self::ArrowUp => "Up",
            Self::ArrowDown => "Down",
            Self::ArrowLeft => "Left",
            Self::ArrowRight => "Right",
            Self::Home => "Home",
            Self::End => "End",
            Self::PageUp => "PageUp",
            Self::PageDown => "PageDown",
            Self::Insert => "Insert",
            Self::Delete => "Delete",
            Self::Backspace => "Backspace",
            Self::Tab => "Tab",
            Self::Enter => "Enter",
            Self::Space => "Space",
            Self::Escape => "Escape",
            Self::Plus => "Plus",
            Self::Minus => "Minus",
            Self::Equals => "Equals",
            Self::Comma => "Comma",
            Self::Period => "Period",
            Self::Semicolon => "Semicolon",
            Self::Colon => "Colon",
            Self::Slash => "Slash",
            Self::Backslash => "Backslash",
            Self::OpenBracket => "OpenBracket",
            Self::CloseBracket => "CloseBracket",
            Self::Backtick => "Backtick",
            Self::Quote => "Quote",
            Self::Pipe => "Pipe",
        }
    }
}

impl fmt::Display for BindingKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for BindingKey {
    type Err = KeyBindingError;

    /// Parse a key name (case-insensitive).
    ///
    /// # Errors
    ///
    /// Returns [`KeyBindingError::UnknownKey`] if the string does not match
    /// any known key name.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "a" => Ok(Self::A),
            "b" => Ok(Self::B),
            "c" => Ok(Self::C),
            "d" => Ok(Self::D),
            "e" => Ok(Self::E),
            "f" => Ok(Self::F),
            "g" => Ok(Self::G),
            "h" => Ok(Self::H),
            "i" => Ok(Self::I),
            "j" => Ok(Self::J),
            "k" => Ok(Self::K),
            "l" => Ok(Self::L),
            "m" => Ok(Self::M),
            "n" => Ok(Self::N),
            "o" => Ok(Self::O),
            "p" => Ok(Self::P),
            "q" => Ok(Self::Q),
            "r" => Ok(Self::R),
            "s" => Ok(Self::S),
            "t" => Ok(Self::T),
            "u" => Ok(Self::U),
            "v" => Ok(Self::V),
            "w" => Ok(Self::W),
            "x" => Ok(Self::X),
            "y" => Ok(Self::Y),
            "z" => Ok(Self::Z),
            "0" => Ok(Self::Num0),
            "1" => Ok(Self::Num1),
            "2" => Ok(Self::Num2),
            "3" => Ok(Self::Num3),
            "4" => Ok(Self::Num4),
            "5" => Ok(Self::Num5),
            "6" => Ok(Self::Num6),
            "7" => Ok(Self::Num7),
            "8" => Ok(Self::Num8),
            "9" => Ok(Self::Num9),
            "f1" => Ok(Self::F1),
            "f2" => Ok(Self::F2),
            "f3" => Ok(Self::F3),
            "f4" => Ok(Self::F4),
            "f5" => Ok(Self::F5),
            "f6" => Ok(Self::F6),
            "f7" => Ok(Self::F7),
            "f8" => Ok(Self::F8),
            "f9" => Ok(Self::F9),
            "f10" => Ok(Self::F10),
            "f11" => Ok(Self::F11),
            "f12" => Ok(Self::F12),
            "up" | "arrowup" => Ok(Self::ArrowUp),
            "down" | "arrowdown" => Ok(Self::ArrowDown),
            "left" | "arrowleft" => Ok(Self::ArrowLeft),
            "right" | "arrowright" => Ok(Self::ArrowRight),
            "home" => Ok(Self::Home),
            "end" => Ok(Self::End),
            "pageup" => Ok(Self::PageUp),
            "pagedown" => Ok(Self::PageDown),
            "insert" => Ok(Self::Insert),
            "delete" => Ok(Self::Delete),
            "backspace" => Ok(Self::Backspace),
            "tab" => Ok(Self::Tab),
            "enter" | "return" => Ok(Self::Enter),
            "space" => Ok(Self::Space),
            "escape" | "esc" => Ok(Self::Escape),
            "plus" | "+" => Ok(Self::Plus),
            "minus" | "-" => Ok(Self::Minus),
            "equals" | "=" => Ok(Self::Equals),
            "comma" | "," => Ok(Self::Comma),
            "period" | "." => Ok(Self::Period),
            "semicolon" | ";" => Ok(Self::Semicolon),
            "colon" | ":" => Ok(Self::Colon),
            "slash" | "/" => Ok(Self::Slash),
            "backslash" | "\\" => Ok(Self::Backslash),
            "openbracket" | "[" => Ok(Self::OpenBracket),
            "closebracket" | "]" => Ok(Self::CloseBracket),
            "backtick" | "`" => Ok(Self::Backtick),
            "quote" | "'" => Ok(Self::Quote),
            "pipe" | "|" => Ok(Self::Pipe),
            other => Err(KeyBindingError::UnknownKey(other.to_string())),
        }
    }
}

// ---------------------------------------------------------------------------
//  BindingModifiers
// ---------------------------------------------------------------------------

/// Modifier keys for a key binding.
///
/// Framework-independent representation. Conversion from `egui::Modifiers`
/// (or any other framework type) happens in the GUI layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, PartialOrd, Ord)]
pub struct BindingModifiers {
    /// The Ctrl key (or Cmd on macOS, at the GUI layer's discretion).
    pub ctrl: bool,
    /// The Shift key.
    pub shift: bool,
    /// The Alt key (Option on macOS).
    pub alt: bool,
}

impl BindingModifiers {
    /// No modifiers held.
    pub const NONE: Self = Self {
        ctrl: false,
        shift: false,
        alt: false,
    };

    /// Ctrl only.
    pub const CTRL: Self = Self {
        ctrl: true,
        shift: false,
        alt: false,
    };

    /// Shift only.
    pub const SHIFT: Self = Self {
        ctrl: false,
        shift: true,
        alt: false,
    };

    /// Ctrl + Shift.
    pub const CTRL_SHIFT: Self = Self {
        ctrl: true,
        shift: true,
        alt: false,
    };

    /// Alt only.
    pub const ALT: Self = Self {
        ctrl: false,
        shift: false,
        alt: true,
    };

    /// Ctrl + Alt.
    pub const CTRL_ALT: Self = Self {
        ctrl: true,
        shift: false,
        alt: true,
    };
}

impl fmt::Display for BindingModifiers {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.ctrl {
            f.write_str("Ctrl+")?;
        }
        if self.shift {
            f.write_str("Shift+")?;
        }
        if self.alt {
            f.write_str("Alt+")?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
//  KeyCombo
// ---------------------------------------------------------------------------

/// A key combination: a key plus zero or more modifiers.
///
/// Used as the lookup key in the binding map. Displayed and parsed in the
/// format `"Ctrl+Shift+T"`, `"Alt+F4"`, `"Escape"`, etc.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct KeyCombo {
    /// The primary key.
    pub key: BindingKey,
    /// Active modifier keys.
    pub modifiers: BindingModifiers,
}

impl KeyCombo {
    /// Create a new key combo from a key and modifiers.
    #[must_use]
    pub const fn new(key: BindingKey, modifiers: BindingModifiers) -> Self {
        Self { key, modifiers }
    }

    /// Create a key combo with no modifiers.
    #[must_use]
    pub const fn bare(key: BindingKey) -> Self {
        Self {
            key,
            modifiers: BindingModifiers::NONE,
        }
    }
}

impl fmt::Display for KeyCombo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.modifiers, self.key)
    }
}

impl KeyCombo {
    /// Format this combo using platform-canonical modifier symbols.
    ///
    /// On macOS: `⌘` (Ctrl mapped to Command), `⌥` (Option/Alt), `⇧` (Shift).
    /// On Linux/Windows: `Ctrl+`, `Shift+`, `Alt+`.
    ///
    /// Note: On macOS, Ctrl in bindings maps to `⌘` (Command) since that is
    /// the platform's primary modifier. True Control (`⌃`) is not currently
    /// representable in the binding model.
    #[must_use]
    pub fn display_platform(&self) -> String {
        use std::fmt::Write;
        let mut s = String::new();
        if cfg!(target_os = "macos") {
            // macOS: use symbols, Ctrl→⌘, Alt→⌥, Shift→⇧
            if self.modifiers.ctrl {
                s.push('\u{2318}'); // ⌘
            }
            if self.modifiers.alt {
                s.push('\u{2325}'); // ⌥
            }
            if self.modifiers.shift {
                s.push('\u{21E7}'); // ⇧
            }
            let _ = write!(s, "{}", self.key);
        } else {
            let _ = write!(s, "{}{}", self.modifiers, self.key);
        }
        s
    }
}

impl FromStr for KeyCombo {
    type Err = KeyBindingError;

    /// Parse a key combo string like `"Ctrl+Shift+T"` or `"Escape"`.
    ///
    /// The last `+`-separated segment is the key; all preceding segments are
    /// modifiers. Matching is case-insensitive.
    ///
    /// # Errors
    ///
    /// Returns [`KeyBindingError::EmptyCombo`] if the string is empty,
    /// [`KeyBindingError::UnknownModifier`] for unrecognized modifier names,
    /// or [`KeyBindingError::UnknownKey`] for unrecognized key names.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if s.is_empty() {
            return Err(KeyBindingError::EmptyCombo);
        }

        let parts: Vec<&str> = s.split('+').collect();
        if parts.is_empty() {
            return Err(KeyBindingError::EmptyCombo);
        }

        // The last `+`-separated segment is the key; all preceding segments
        // are modifiers.
        //
        // Edge case: the key itself can be literal `+` (`BindingKey::Plus`).
        // When `+` is the key the string ends with `+`, producing an empty
        // trailing part after `split('+')`.  Examples:
        //
        //   "+"        → ["", ""]      → modifiers=[], key="+"
        //   "Ctrl+"    → ["Ctrl", ""]  → modifiers=["Ctrl"], key="+"
        //   "Ctrl++"   → ["Ctrl", "", ""] → modifiers=["Ctrl"], key="+"
        //
        // We detect this by checking whether the last part is empty (i.e.
        // the string ended with `+`).  When it is, the key is `"+"` and
        // the modifier parts are everything *before* the last separator
        // that was consumed as the key delimiter.  `rsplit_once('+')` on
        // the string *without the trailing `+`* gives us the modifier
        // prefix cleanly.
        let (modifier_parts_str, key_str) = if parts.last() == Some(&"") {
            // String ended with "+": the key is Plus.
            // Strip the trailing "+" (the key), then the remainder before it
            // is the modifier prefix (possibly empty, possibly "Ctrl+",
            // "Ctrl+Shift+", etc.).
            let without_key = &s[..s.len() - 1]; // remove trailing '+'
            // If there is another trailing '+', it was the separator between
            // the last modifier and the key — strip it too.
            let prefix = without_key.strip_suffix('+').unwrap_or(without_key);
            (prefix, "+")
        } else {
            // Normal case: last segment is a named key.
            let (mods, key) = parts.split_at(parts.len() - 1);
            let mod_str = if mods.is_empty() {
                ""
            } else {
                // Rejoin just the modifier parts.  Safe because `s` started
                // as `"Mod1+Mod2+…+Key"` and we only need the part before
                // the last `+`.
                &s[..s.len() - key[0].len() - 1]
            };
            (mod_str, key[0])
        };

        let modifier_tokens: Vec<&str> = if modifier_parts_str.is_empty() {
            Vec::new()
        } else {
            modifier_parts_str.split('+').collect()
        };

        let mut modifiers = BindingModifiers::NONE;
        for modifier in &modifier_tokens {
            match modifier.to_ascii_lowercase().as_str() {
                "ctrl" | "control" | "cmd" | "command" => modifiers.ctrl = true,
                "shift" => modifiers.shift = true,
                "alt" | "option" | "opt" => modifiers.alt = true,
                other => return Err(KeyBindingError::UnknownModifier(other.to_string())),
            }
        }

        let key = BindingKey::from_str(key_str)?;
        Ok(Self { key, modifiers })
    }
}

// ---------------------------------------------------------------------------
//  KeyAction
// ---------------------------------------------------------------------------

/// An application-level action that can be triggered by a key binding.
///
/// Every user-facing keyboard shortcut in Freminal maps to one of these
/// variants. The dispatch layer in the GUI checks incoming key events
/// against the [`BindingMap`] and executes the corresponding action.
///
/// Variants are grouped by feature area. Future features add new variants
/// here and register default bindings in [`BindingMap::default()`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyAction {
    // -- Tab actions --------------------------------------------------------
    /// Create a new tab.
    NewTab,
    /// Close the active tab.
    CloseTab,
    /// Switch to the next tab.
    NextTab,
    /// Switch to the previous tab.
    PrevTab,
    /// Switch to tab 1.
    SwitchToTab1,
    /// Switch to tab 2.
    SwitchToTab2,
    /// Switch to tab 3.
    SwitchToTab3,
    /// Switch to tab 4.
    SwitchToTab4,
    /// Switch to tab 5.
    SwitchToTab5,
    /// Switch to tab 6.
    SwitchToTab6,
    /// Switch to tab 7.
    SwitchToTab7,
    /// Switch to tab 8.
    SwitchToTab8,
    /// Switch to tab 9.
    SwitchToTab9,
    /// Move the active tab one position to the left.
    MoveTabLeft,
    /// Move the active tab one position to the right.
    MoveTabRight,
    /// Rename the active tab.
    RenameTab,

    // -- Clipboard / selection ----------------------------------------------
    /// Copy the current selection to the system clipboard.
    Copy,
    /// Paste the system clipboard contents into the terminal.
    Paste,
    /// Select all visible terminal content.
    SelectAll,

    // -- Search -------------------------------------------------------------
    /// Open the search overlay.
    OpenSearch,
    /// Navigate to the next search match.
    SearchNext,
    /// Navigate to the previous search match.
    SearchPrev,
    /// Jump to the previous command prompt boundary (OSC 133).
    PrevCommand,
    /// Jump to the next command prompt boundary (OSC 133).
    NextCommand,

    // -- Font zoom ----------------------------------------------------------
    /// Increase font size.
    ZoomIn,
    /// Decrease font size.
    ZoomOut,
    /// Reset font size to the configured default.
    ZoomReset,

    // -- UI -----------------------------------------------------------------
    /// Toggle the menu bar visibility.
    ToggleMenuBar,
    /// Open the settings modal.
    OpenSettings,
    /// Open a new OS window with an initial tab.
    NewWindow,

    // -- Scrollback ---------------------------------------------------------
    /// Scroll up by one page.
    ScrollPageUp,
    /// Scroll down by one page.
    ScrollPageDown,
    /// Scroll to the top of the scrollback buffer.
    ScrollToTop,
    /// Scroll to the bottom (live terminal output).
    ScrollToBottom,
    /// Scroll up by one line.
    ScrollLineUp,
    /// Scroll down by one line.
    ScrollLineDown,

    // -- Pane management ---------------------------------------------------
    /// Split the focused pane vertically (left | right, vertical divider).
    SplitVertical,
    /// Split the focused pane horizontally (top / bottom, horizontal divider).
    SplitHorizontal,
    /// Close the focused pane (last pane closes the tab).
    ClosePane,
    /// Move focus to the pane to the left.
    FocusPaneLeft,
    /// Move focus to the pane below.
    FocusPaneDown,
    /// Move focus to the pane above.
    FocusPaneUp,
    /// Move focus to the pane to the right.
    FocusPaneRight,
    /// Grow the focused pane leftward (shrink right neighbor).
    ResizePaneLeft,
    /// Grow the focused pane downward (shrink top neighbor).
    ResizePaneDown,
    /// Grow the focused pane upward (shrink bottom neighbor).
    ResizePaneUp,
    /// Grow the focused pane rightward (shrink left neighbor).
    ResizePaneRight,
    /// Toggle zoom on the focused pane (full-tab or restore).
    ZoomPane,
}

impl KeyAction {
    /// Returns the canonical `snake_case` name for this action.
    ///
    /// This matches the serde serialization format and the TOML config key.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::NewTab => "new_tab",
            Self::CloseTab => "close_tab",
            Self::NextTab => "next_tab",
            Self::PrevTab => "prev_tab",
            Self::SwitchToTab1 => "switch_to_tab_1",
            Self::SwitchToTab2 => "switch_to_tab_2",
            Self::SwitchToTab3 => "switch_to_tab_3",
            Self::SwitchToTab4 => "switch_to_tab_4",
            Self::SwitchToTab5 => "switch_to_tab_5",
            Self::SwitchToTab6 => "switch_to_tab_6",
            Self::SwitchToTab7 => "switch_to_tab_7",
            Self::SwitchToTab8 => "switch_to_tab_8",
            Self::SwitchToTab9 => "switch_to_tab_9",
            Self::MoveTabLeft => "move_tab_left",
            Self::MoveTabRight => "move_tab_right",
            Self::RenameTab => "rename_tab",
            Self::Copy => "copy",
            Self::Paste => "paste",
            Self::SelectAll => "select_all",
            Self::OpenSearch => "open_search",
            Self::SearchNext => "search_next",
            Self::SearchPrev => "search_prev",
            Self::PrevCommand => "prev_command",
            Self::NextCommand => "next_command",
            Self::ZoomIn => "zoom_in",
            Self::ZoomOut => "zoom_out",
            Self::ZoomReset => "zoom_reset",
            Self::ToggleMenuBar => "toggle_menu_bar",
            Self::OpenSettings => "open_settings",
            Self::NewWindow => "new_window",
            Self::ScrollPageUp => "scroll_page_up",
            Self::ScrollPageDown => "scroll_page_down",
            Self::ScrollToTop => "scroll_to_top",
            Self::ScrollToBottom => "scroll_to_bottom",
            Self::ScrollLineUp => "scroll_line_up",
            Self::ScrollLineDown => "scroll_line_down",
            Self::SplitVertical => "split_vertical",
            Self::SplitHorizontal => "split_horizontal",
            Self::ClosePane => "close_pane",
            Self::FocusPaneLeft => "focus_pane_left",
            Self::FocusPaneDown => "focus_pane_down",
            Self::FocusPaneUp => "focus_pane_up",
            Self::FocusPaneRight => "focus_pane_right",
            Self::ResizePaneLeft => "resize_pane_left",
            Self::ResizePaneDown => "resize_pane_down",
            Self::ResizePaneUp => "resize_pane_up",
            Self::ResizePaneRight => "resize_pane_right",
            Self::ZoomPane => "zoom_pane",
        }
    }

    /// Returns a human-friendly label for display in the Settings UI.
    ///
    /// Unlike [`name()`](Self::name), which returns the `snake_case` config
    /// key, this returns a title-cased string with spaces (e.g. "New Tab",
    /// "Scroll Page Up", "Switch to Tab 1").
    #[must_use]
    pub const fn display_label(self) -> &'static str {
        match self {
            Self::NewTab => "New Tab",
            Self::CloseTab => "Close Tab",
            Self::NextTab => "Next Tab",
            Self::PrevTab => "Previous Tab",
            Self::SwitchToTab1 => "Switch to Tab 1",
            Self::SwitchToTab2 => "Switch to Tab 2",
            Self::SwitchToTab3 => "Switch to Tab 3",
            Self::SwitchToTab4 => "Switch to Tab 4",
            Self::SwitchToTab5 => "Switch to Tab 5",
            Self::SwitchToTab6 => "Switch to Tab 6",
            Self::SwitchToTab7 => "Switch to Tab 7",
            Self::SwitchToTab8 => "Switch to Tab 8",
            Self::SwitchToTab9 => "Switch to Tab 9",
            Self::MoveTabLeft => "Move Tab Left",
            Self::MoveTabRight => "Move Tab Right",
            Self::RenameTab => "Rename Tab",
            Self::Copy => "Copy",
            Self::Paste => "Paste",
            Self::SelectAll => "Select All",
            Self::OpenSearch => "Open Search",
            Self::SearchNext => "Search Next",
            Self::SearchPrev => "Search Previous",
            Self::PrevCommand => "Previous Command",
            Self::NextCommand => "Next Command",
            Self::ZoomIn => "Zoom In",
            Self::ZoomOut => "Zoom Out",
            Self::ZoomReset => "Zoom Reset",
            Self::ToggleMenuBar => "Toggle Menu Bar",
            Self::OpenSettings => "Open Settings",
            Self::NewWindow => "New Window",
            Self::ScrollPageUp => "Scroll Page Up",
            Self::ScrollPageDown => "Scroll Page Down",
            Self::ScrollToTop => "Scroll to Top",
            Self::ScrollToBottom => "Scroll to Bottom",
            Self::ScrollLineUp => "Scroll Line Up",
            Self::ScrollLineDown => "Scroll Line Down",
            Self::SplitVertical => "Split Vertical",
            Self::SplitHorizontal => "Split Horizontal",
            Self::ClosePane => "Close Pane",
            Self::FocusPaneLeft => "Focus Pane Left",
            Self::FocusPaneDown => "Focus Pane Down",
            Self::FocusPaneUp => "Focus Pane Up",
            Self::FocusPaneRight => "Focus Pane Right",
            Self::ResizePaneLeft => "Resize Pane Left",
            Self::ResizePaneDown => "Resize Pane Down",
            Self::ResizePaneUp => "Resize Pane Up",
            Self::ResizePaneRight => "Resize Pane Right",
            Self::ZoomPane => "Zoom Pane",
        }
    }

    /// All defined actions, in declaration order.
    ///
    /// Useful for iterating over all actions in the settings UI or for
    /// generating documentation.
    pub const ALL: &[Self] = &[
        Self::NewTab,
        Self::CloseTab,
        Self::NextTab,
        Self::PrevTab,
        Self::SwitchToTab1,
        Self::SwitchToTab2,
        Self::SwitchToTab3,
        Self::SwitchToTab4,
        Self::SwitchToTab5,
        Self::SwitchToTab6,
        Self::SwitchToTab7,
        Self::SwitchToTab8,
        Self::SwitchToTab9,
        Self::MoveTabLeft,
        Self::MoveTabRight,
        Self::RenameTab,
        Self::Copy,
        Self::Paste,
        Self::SelectAll,
        Self::OpenSearch,
        Self::SearchNext,
        Self::SearchPrev,
        Self::PrevCommand,
        Self::NextCommand,
        Self::ZoomIn,
        Self::ZoomOut,
        Self::ZoomReset,
        Self::ToggleMenuBar,
        Self::OpenSettings,
        Self::NewWindow,
        Self::ScrollPageUp,
        Self::ScrollPageDown,
        Self::ScrollToTop,
        Self::ScrollToBottom,
        Self::ScrollLineUp,
        Self::ScrollLineDown,
        Self::SplitVertical,
        Self::SplitHorizontal,
        Self::ClosePane,
        Self::FocusPaneLeft,
        Self::FocusPaneDown,
        Self::FocusPaneUp,
        Self::FocusPaneRight,
        Self::ResizePaneLeft,
        Self::ResizePaneDown,
        Self::ResizePaneUp,
        Self::ResizePaneRight,
        Self::ZoomPane,
    ];
}

impl fmt::Display for KeyAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for KeyAction {
    type Err = KeyBindingError;

    /// Parse an action name (case-insensitive, underscore-separated).
    ///
    /// # Errors
    ///
    /// Returns [`KeyBindingError::UnknownAction`] if the string does not match
    /// any known action name.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "new_tab" => Ok(Self::NewTab),
            "close_tab" => Ok(Self::CloseTab),
            "next_tab" => Ok(Self::NextTab),
            "prev_tab" => Ok(Self::PrevTab),
            "switch_to_tab_1" => Ok(Self::SwitchToTab1),
            "switch_to_tab_2" => Ok(Self::SwitchToTab2),
            "switch_to_tab_3" => Ok(Self::SwitchToTab3),
            "switch_to_tab_4" => Ok(Self::SwitchToTab4),
            "switch_to_tab_5" => Ok(Self::SwitchToTab5),
            "switch_to_tab_6" => Ok(Self::SwitchToTab6),
            "switch_to_tab_7" => Ok(Self::SwitchToTab7),
            "switch_to_tab_8" => Ok(Self::SwitchToTab8),
            "switch_to_tab_9" => Ok(Self::SwitchToTab9),
            "move_tab_left" => Ok(Self::MoveTabLeft),
            "move_tab_right" => Ok(Self::MoveTabRight),
            "rename_tab" => Ok(Self::RenameTab),
            "copy" => Ok(Self::Copy),
            "paste" => Ok(Self::Paste),
            "select_all" => Ok(Self::SelectAll),
            "open_search" => Ok(Self::OpenSearch),
            "search_next" => Ok(Self::SearchNext),
            "search_prev" => Ok(Self::SearchPrev),
            "prev_command" => Ok(Self::PrevCommand),
            "next_command" => Ok(Self::NextCommand),
            "zoom_in" => Ok(Self::ZoomIn),
            "zoom_out" => Ok(Self::ZoomOut),
            "zoom_reset" => Ok(Self::ZoomReset),
            "toggle_menu_bar" => Ok(Self::ToggleMenuBar),
            "open_settings" => Ok(Self::OpenSettings),
            "new_window" => Ok(Self::NewWindow),
            "scroll_page_up" => Ok(Self::ScrollPageUp),
            "scroll_page_down" => Ok(Self::ScrollPageDown),
            "scroll_to_top" => Ok(Self::ScrollToTop),
            "scroll_to_bottom" => Ok(Self::ScrollToBottom),
            "scroll_line_up" => Ok(Self::ScrollLineUp),
            "scroll_line_down" => Ok(Self::ScrollLineDown),
            "split_vertical" => Ok(Self::SplitVertical),
            "split_horizontal" => Ok(Self::SplitHorizontal),
            "close_pane" => Ok(Self::ClosePane),
            "focus_pane_left" => Ok(Self::FocusPaneLeft),
            "focus_pane_down" => Ok(Self::FocusPaneDown),
            "focus_pane_up" => Ok(Self::FocusPaneUp),
            "focus_pane_right" => Ok(Self::FocusPaneRight),
            "resize_pane_left" => Ok(Self::ResizePaneLeft),
            "resize_pane_down" => Ok(Self::ResizePaneDown),
            "resize_pane_up" => Ok(Self::ResizePaneUp),
            "resize_pane_right" => Ok(Self::ResizePaneRight),
            "zoom_pane" => Ok(Self::ZoomPane),
            other => Err(KeyBindingError::UnknownAction(other.to_string())),
        }
    }
}

// ---------------------------------------------------------------------------
//  BindingMap
// ---------------------------------------------------------------------------

/// The set of key bindings mapping key combos to application actions.
///
/// A single combo maps to exactly one action. Multiple combos may map to the
/// same action (e.g. both `Ctrl+=` and `Ctrl+Plus` can trigger `ZoomIn`).
///
/// The [`Default`] implementation produces the standard set of bindings
/// matching common terminal emulator conventions.
#[derive(Debug, Clone)]
pub struct BindingMap {
    /// Primary lookup: key combo → action.
    combo_to_action: HashMap<KeyCombo, KeyAction>,
}

impl BindingMap {
    /// Create an empty binding map with no bindings.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            combo_to_action: HashMap::new(),
        }
    }

    /// Look up the action bound to a key combo.
    #[must_use]
    pub fn lookup(&self, combo: &KeyCombo) -> Option<KeyAction> {
        self.combo_to_action.get(combo).copied()
    }

    /// Find the smallest combo bound to a given action, if any.
    ///
    /// When multiple combos are bound to the same action, the combo with the
    /// smallest `Ord` value is returned, ensuring deterministic display even
    /// though the underlying map is a `HashMap`.
    #[must_use]
    pub fn combo_for(&self, action: KeyAction) -> Option<KeyCombo> {
        self.combo_to_action
            .iter()
            .filter(|(_, a)| **a == action)
            .map(|(combo, _)| *combo)
            .min()
    }

    /// Find all combos bound to a given action, sorted for deterministic output.
    #[must_use]
    pub fn all_combos_for(&self, action: KeyAction) -> Vec<KeyCombo> {
        let mut combos: Vec<KeyCombo> = self
            .combo_to_action
            .iter()
            .filter(|(_, a)| **a == action)
            .map(|(combo, _)| *combo)
            .collect();
        combos.sort();
        combos
    }

    /// Bind a key combo to an action.
    ///
    /// If the combo was already bound to a different action, the previous
    /// action is returned.
    pub fn bind(&mut self, combo: KeyCombo, action: KeyAction) -> Option<KeyAction> {
        self.combo_to_action.insert(combo, action)
    }

    /// Remove the binding for a specific key combo.
    ///
    /// Returns the action that was bound to the combo, if any.
    pub fn unbind_combo(&mut self, combo: &KeyCombo) -> Option<KeyAction> {
        self.combo_to_action.remove(combo)
    }

    /// Remove all bindings for a specific action.
    ///
    /// Returns the combos that were bound to the action.
    pub fn unbind_action(&mut self, action: KeyAction) -> Vec<KeyCombo> {
        let combos: Vec<KeyCombo> = self
            .combo_to_action
            .iter()
            .filter(|(_, a)| **a == action)
            .map(|(combo, _)| *combo)
            .collect();

        for combo in &combos {
            self.combo_to_action.remove(combo);
        }

        combos
    }

    /// Iterate over all bindings as `(combo, action)` pairs.
    pub fn iter(&self) -> impl Iterator<Item = (&KeyCombo, &KeyAction)> {
        self.combo_to_action.iter()
    }

    /// Returns the number of bindings in the map.
    #[must_use]
    pub fn len(&self) -> usize {
        self.combo_to_action.len()
    }

    /// Returns `true` if the map contains no bindings.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.combo_to_action.is_empty()
    }

    /// Apply user-specified overrides on top of the current bindings.
    ///
    /// Each entry in `overrides` maps an action name (`snake_case`) to a key
    /// combo string. A combo string of `"none"` or `""` removes all bindings
    /// for that action.
    ///
    /// # Errors
    ///
    /// Returns the first parse error encountered. Bindings applied before
    /// the error are retained (partial application).
    pub fn apply_overrides(
        &mut self,
        overrides: &HashMap<String, String>,
    ) -> Result<(), KeyBindingError> {
        for (action_str, combo_str) in overrides {
            let action = KeyAction::from_str(action_str)?;

            let combo_str = combo_str.trim();
            if combo_str.is_empty() || combo_str.eq_ignore_ascii_case("none") {
                // Unbind this action entirely.
                self.unbind_action(action);
            } else {
                let combo = KeyCombo::from_str(combo_str)?;
                self.bind(combo, action);
            }
        }
        Ok(())
    }
}

/// Register the standard tab-management bindings (new, close, next, prev, switch 1–9).
fn register_tab_bindings(map: &mut BindingMap) {
    map.bind(
        KeyCombo::new(BindingKey::T, BindingModifiers::CTRL_SHIFT),
        KeyAction::NewTab,
    );
    // Note: Ctrl+Shift+W is now ClosePane (registered in register_pane_bindings).
    // CloseTab has no default binding but can be configured by the user.
    map.bind(
        KeyCombo::new(BindingKey::Tab, BindingModifiers::CTRL),
        KeyAction::NextTab,
    );
    map.bind(
        KeyCombo::new(BindingKey::Tab, BindingModifiers::CTRL_SHIFT),
        KeyAction::PrevTab,
    );

    // Switch to tab N via Ctrl+Shift+<digit>.
    let digit_actions = [
        (BindingKey::Num1, KeyAction::SwitchToTab1),
        (BindingKey::Num2, KeyAction::SwitchToTab2),
        (BindingKey::Num3, KeyAction::SwitchToTab3),
        (BindingKey::Num4, KeyAction::SwitchToTab4),
        (BindingKey::Num5, KeyAction::SwitchToTab5),
        (BindingKey::Num6, KeyAction::SwitchToTab6),
        (BindingKey::Num7, KeyAction::SwitchToTab7),
        (BindingKey::Num8, KeyAction::SwitchToTab8),
        (BindingKey::Num9, KeyAction::SwitchToTab9),
    ];
    for (key, action) in digit_actions {
        map.bind(KeyCombo::new(key, BindingModifiers::CTRL_SHIFT), action);
    }
}

/// Register clipboard, zoom, UI, and scrollback bindings.
fn register_misc_bindings(map: &mut BindingMap) {
    // -- Clipboard / selection --
    map.bind(
        KeyCombo::new(BindingKey::C, BindingModifiers::CTRL_SHIFT),
        KeyAction::Copy,
    );
    map.bind(
        KeyCombo::new(BindingKey::V, BindingModifiers::CTRL_SHIFT),
        KeyAction::Paste,
    );

    // -- Font zoom --
    // Ctrl+= is the primary zoom-in binding (= is next to - on US keyboards).
    // Ctrl+Plus is an alias (Shift+= on US keyboards produces +).
    map.bind(
        KeyCombo::new(BindingKey::Equals, BindingModifiers::CTRL),
        KeyAction::ZoomIn,
    );
    map.bind(
        KeyCombo::new(BindingKey::Plus, BindingModifiers::CTRL),
        KeyAction::ZoomIn,
    );
    map.bind(
        KeyCombo::new(BindingKey::Minus, BindingModifiers::CTRL),
        KeyAction::ZoomOut,
    );
    map.bind(
        KeyCombo::new(BindingKey::Num0, BindingModifiers::CTRL),
        KeyAction::ZoomReset,
    );

    // -- UI --
    map.bind(
        KeyCombo::new(BindingKey::Comma, BindingModifiers::CTRL_SHIFT),
        KeyAction::OpenSettings,
    );

    // -- Search --
    map.bind(
        KeyCombo::new(BindingKey::F, BindingModifiers::CTRL_SHIFT),
        KeyAction::OpenSearch,
    );
    map.bind(
        KeyCombo::new(BindingKey::ArrowUp, BindingModifiers::CTRL_SHIFT),
        KeyAction::PrevCommand,
    );
    map.bind(
        KeyCombo::new(BindingKey::ArrowDown, BindingModifiers::CTRL_SHIFT),
        KeyAction::NextCommand,
    );

    // -- Scrollback --
    // Shift+PageUp/Down is the standard terminal scrollback shortcut.
    map.bind(
        KeyCombo::new(BindingKey::PageUp, BindingModifiers::SHIFT),
        KeyAction::ScrollPageUp,
    );
    map.bind(
        KeyCombo::new(BindingKey::PageDown, BindingModifiers::SHIFT),
        KeyAction::ScrollPageDown,
    );
    map.bind(
        KeyCombo::new(BindingKey::Home, BindingModifiers::SHIFT),
        KeyAction::ScrollToTop,
    );
    map.bind(
        KeyCombo::new(BindingKey::End, BindingModifiers::SHIFT),
        KeyAction::ScrollToBottom,
    );
    map.bind(
        KeyCombo::new(BindingKey::ArrowUp, BindingModifiers::SHIFT),
        KeyAction::ScrollLineUp,
    );
    map.bind(
        KeyCombo::new(BindingKey::ArrowDown, BindingModifiers::SHIFT),
        KeyAction::ScrollLineDown,
    );
}

/// Register built-in multiplexer (split pane) bindings.
fn register_pane_bindings(map: &mut BindingMap) {
    // Split the focused pane with a vertical divider (left | right).
    // Ctrl+Shift+Pipe mirrors the tmux/zellij convention.
    map.bind(
        KeyCombo::new(BindingKey::Pipe, BindingModifiers::CTRL_SHIFT),
        KeyAction::SplitVertical,
    );
    // Split the focused pane with a horizontal divider (top / bottom).
    // Ctrl+Shift+Minus (underscore row) is the natural complement.
    map.bind(
        KeyCombo::new(BindingKey::Minus, BindingModifiers::CTRL_SHIFT),
        KeyAction::SplitHorizontal,
    );
    // Close the focused pane (last pane in tab closes the tab).
    // Replaces the tab-level CloseTab binding on Ctrl+Shift+W.
    map.bind(
        KeyCombo::new(BindingKey::W, BindingModifiers::CTRL_SHIFT),
        KeyAction::ClosePane,
    );

    // Directional navigation (vim-style).
    map.bind(
        KeyCombo::new(BindingKey::H, BindingModifiers::CTRL_SHIFT),
        KeyAction::FocusPaneLeft,
    );
    map.bind(
        KeyCombo::new(BindingKey::J, BindingModifiers::CTRL_SHIFT),
        KeyAction::FocusPaneDown,
    );
    map.bind(
        KeyCombo::new(BindingKey::K, BindingModifiers::CTRL_SHIFT),
        KeyAction::FocusPaneUp,
    );
    map.bind(
        KeyCombo::new(BindingKey::L, BindingModifiers::CTRL_SHIFT),
        KeyAction::FocusPaneRight,
    );

    // Directional resize (vim-style, Ctrl+Alt prefix).
    map.bind(
        KeyCombo::new(BindingKey::H, BindingModifiers::CTRL_ALT),
        KeyAction::ResizePaneLeft,
    );
    map.bind(
        KeyCombo::new(BindingKey::J, BindingModifiers::CTRL_ALT),
        KeyAction::ResizePaneDown,
    );
    map.bind(
        KeyCombo::new(BindingKey::K, BindingModifiers::CTRL_ALT),
        KeyAction::ResizePaneUp,
    );
    map.bind(
        KeyCombo::new(BindingKey::L, BindingModifiers::CTRL_ALT),
        KeyAction::ResizePaneRight,
    );

    // Zoom toggle.
    map.bind(
        KeyCombo::new(BindingKey::Z, BindingModifiers::CTRL_SHIFT),
        KeyAction::ZoomPane,
    );
}

/// Register window management bindings.
fn register_window_bindings(map: &mut BindingMap) {
    // Open a new OS window (mirrors "New Tab" but at the window level).
    map.bind(
        KeyCombo::new(BindingKey::N, BindingModifiers::CTRL_SHIFT),
        KeyAction::NewWindow,
    );
}

impl Default for BindingMap {
    /// Produce the standard set of key bindings matching common terminal
    /// emulator conventions.
    fn default() -> Self {
        let mut map = Self::empty();
        register_tab_bindings(&mut map);
        register_misc_bindings(&mut map);
        register_pane_bindings(&mut map);
        register_window_bindings(&mut map);
        map
    }
}

// ---------------------------------------------------------------------------
//  Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    // -- BindingKey parsing -------------------------------------------------

    #[test]
    fn binding_key_parse_letters() {
        for c in 'a'..='z' {
            let key: BindingKey = c.to_string().parse().expect("letter should parse");
            let upper: BindingKey = c.to_ascii_uppercase().to_string().parse().unwrap();
            assert_eq!(key, upper, "case-insensitive letter parsing");
        }
    }

    #[test]
    fn binding_key_parse_digits() {
        for d in '0'..='9' {
            let _key: BindingKey = d.to_string().parse().expect("digit should parse");
        }
    }

    #[test]
    fn binding_key_parse_function_keys() {
        for n in 1..=12 {
            let s = format!("F{n}");
            let _key: BindingKey = s.parse().expect("function key should parse");
        }
    }

    #[test]
    fn binding_key_parse_navigation() {
        for name in &[
            "Up",
            "Down",
            "Left",
            "Right",
            "Home",
            "End",
            "PageUp",
            "PageDown",
            "ArrowUp",
            "ArrowDown",
            "ArrowLeft",
            "ArrowRight",
        ] {
            let _key: BindingKey = name.parse().expect("navigation key should parse");
        }
    }

    #[test]
    fn binding_key_parse_special() {
        for name in &[
            "Tab",
            "Enter",
            "Return",
            "Space",
            "Escape",
            "Esc",
            "Insert",
            "Delete",
            "Backspace",
        ] {
            let _key: BindingKey = name.parse().expect("special key should parse");
        }
    }

    #[test]
    fn binding_key_parse_symbols() {
        for name in &[
            "Plus",
            "+",
            "Minus",
            "-",
            "Equals",
            "=",
            "Comma",
            ",",
            "Period",
            ".",
            "Semicolon",
            ";",
            "Colon",
            ":",
            "Slash",
            "/",
            "Backslash",
            "\\",
            "OpenBracket",
            "[",
            "CloseBracket",
            "]",
            "Backtick",
            "`",
            "Quote",
            "'",
        ] {
            let _key: BindingKey = name.parse().expect("symbol key should parse");
        }
    }

    #[test]
    fn binding_key_parse_unknown_returns_error() {
        let err = "NotAKey".parse::<BindingKey>().unwrap_err();
        assert!(matches!(err, KeyBindingError::UnknownKey(_)));
    }

    #[test]
    fn binding_key_display_roundtrip() {
        // Every key's Display output should parse back to the same key.
        let all_keys = [
            BindingKey::A,
            BindingKey::Z,
            BindingKey::Num0,
            BindingKey::Num9,
            BindingKey::F1,
            BindingKey::F12,
            BindingKey::ArrowUp,
            BindingKey::Home,
            BindingKey::Tab,
            BindingKey::Escape,
            BindingKey::Plus,
            BindingKey::Minus,
            BindingKey::Equals,
            BindingKey::Comma,
            BindingKey::Backtick,
        ];
        for key in all_keys {
            let s = key.to_string();
            let parsed: BindingKey = s.parse().unwrap_or_else(|e| {
                panic!("BindingKey::Display output {s:?} should parse back: {e}")
            });
            assert_eq!(key, parsed);
        }
    }

    // -- KeyCombo parsing ---------------------------------------------------

    #[test]
    fn key_combo_parse_bare_key() {
        let combo: KeyCombo = "Escape".parse().unwrap();
        assert_eq!(combo.key, BindingKey::Escape);
        assert_eq!(combo.modifiers, BindingModifiers::NONE);
    }

    #[test]
    fn key_combo_parse_ctrl_shift() {
        let combo: KeyCombo = "Ctrl+Shift+T".parse().unwrap();
        assert_eq!(combo.key, BindingKey::T);
        assert!(combo.modifiers.ctrl);
        assert!(combo.modifiers.shift);
        assert!(!combo.modifiers.alt);
    }

    #[test]
    fn key_combo_parse_alt() {
        let combo: KeyCombo = "Alt+F4".parse().unwrap();
        assert_eq!(combo.key, BindingKey::F4);
        assert!(combo.modifiers.alt);
        assert!(!combo.modifiers.ctrl);
        assert!(!combo.modifiers.shift);
    }

    #[test]
    fn key_combo_parse_case_insensitive() {
        let combo: KeyCombo = "ctrl+shift+c".parse().unwrap();
        assert_eq!(combo.key, BindingKey::C);
        assert!(combo.modifiers.ctrl);
        assert!(combo.modifiers.shift);
    }

    #[test]
    fn key_combo_parse_command_alias() {
        let combo: KeyCombo = "Command+C".parse().unwrap();
        assert_eq!(combo.key, BindingKey::C);
        assert!(combo.modifiers.ctrl);
    }

    #[test]
    fn key_combo_parse_option_alias() {
        let combo: KeyCombo = "Option+Tab".parse().unwrap();
        assert_eq!(combo.key, BindingKey::Tab);
        assert!(combo.modifiers.alt);
    }

    #[test]
    fn key_combo_parse_plus_key() {
        // "Ctrl++" means Ctrl+Plus — trailing "+" is the key
        let combo: KeyCombo = "Ctrl++".parse().unwrap();
        assert_eq!(combo.key, BindingKey::Plus);
        assert!(combo.modifiers.ctrl);
    }

    #[test]
    fn key_combo_parse_ctrl_plus_preserves_modifier() {
        // "Ctrl+" means Ctrl+Plus — the Ctrl modifier must be preserved.
        // Regression test for a bug where "Ctrl+" dropped Ctrl because
        // the modifier_parts slice was empty.
        let combo: KeyCombo = "Ctrl+".parse().unwrap();
        assert_eq!(combo.key, BindingKey::Plus);
        assert!(combo.modifiers.ctrl, "Ctrl modifier must be preserved");
        assert!(!combo.modifiers.shift);
        assert!(!combo.modifiers.alt);
    }

    #[test]
    fn key_combo_parse_bare_plus() {
        // "+" alone is a bare Plus key with no modifiers.
        let combo: KeyCombo = "+".parse().unwrap();
        assert_eq!(combo.key, BindingKey::Plus);
        assert_eq!(combo.modifiers, BindingModifiers::NONE);
    }

    #[test]
    fn key_combo_parse_ctrl_shift_plus() {
        // "Ctrl+Shift++" means Ctrl+Shift+Plus.
        let combo: KeyCombo = "Ctrl+Shift++".parse().unwrap();
        assert_eq!(combo.key, BindingKey::Plus);
        assert!(combo.modifiers.ctrl);
        assert!(combo.modifiers.shift);
    }

    #[test]
    fn key_combo_parse_empty_returns_error() {
        let err = "".parse::<KeyCombo>().unwrap_err();
        assert!(matches!(err, KeyBindingError::EmptyCombo));
    }

    #[test]
    fn key_combo_parse_whitespace_only_returns_error() {
        let err = "  ".parse::<KeyCombo>().unwrap_err();
        assert!(matches!(err, KeyBindingError::EmptyCombo));
    }

    #[test]
    fn key_combo_parse_unknown_modifier() {
        let err = "Super+A".parse::<KeyCombo>().unwrap_err();
        assert!(matches!(err, KeyBindingError::UnknownModifier(_)));
    }

    #[test]
    fn key_combo_parse_unknown_key() {
        let err = "Ctrl+FooBar".parse::<KeyCombo>().unwrap_err();
        assert!(matches!(err, KeyBindingError::UnknownKey(_)));
    }

    #[test]
    fn key_combo_display_roundtrip() {
        let combos = [
            KeyCombo::bare(BindingKey::Escape),
            KeyCombo::new(BindingKey::T, BindingModifiers::CTRL_SHIFT),
            KeyCombo::new(BindingKey::F4, BindingModifiers::ALT),
            KeyCombo::new(BindingKey::C, BindingModifiers::CTRL),
            KeyCombo::new(BindingKey::PageUp, BindingModifiers::SHIFT),
            // Plus key combos: exercise the trailing-"+" edge case in FromStr.
            KeyCombo::bare(BindingKey::Plus),
            KeyCombo::new(BindingKey::Plus, BindingModifiers::CTRL),
            KeyCombo::new(BindingKey::Plus, BindingModifiers::CTRL_SHIFT),
        ];
        for combo in combos {
            let s = combo.to_string();
            let parsed: KeyCombo = s.parse().unwrap_or_else(|e| {
                panic!("KeyCombo::Display output {s:?} should parse back: {e}")
            });
            assert_eq!(combo, parsed);
        }
    }

    // -- KeyAction parsing --------------------------------------------------

    #[test]
    fn key_action_parse_all_actions() {
        for action in KeyAction::ALL {
            let s = action.name();
            let parsed: KeyAction = s
                .parse()
                .unwrap_or_else(|e| panic!("KeyAction name {s:?} should parse back: {e}"));
            assert_eq!(*action, parsed);
        }
    }

    #[test]
    fn key_action_parse_case_insensitive() {
        let action: KeyAction = "NEW_TAB".parse().unwrap();
        assert_eq!(action, KeyAction::NewTab);
    }

    #[test]
    fn key_action_parse_unknown_returns_error() {
        let err = "launch_rockets".parse::<KeyAction>().unwrap_err();
        assert!(matches!(err, KeyBindingError::UnknownAction(_)));
    }

    #[test]
    fn key_action_all_count() {
        // Ensure ALL contains every variant. If a new variant is added but
        // not added to ALL, this test will fail because the Display/parse
        // roundtrip test above covers ALL, and name() is exhaustive.
        assert_eq!(
            KeyAction::ALL.len(),
            48,
            "KeyAction::ALL should contain all variants"
        );
    }

    // -- BindingMap ----------------------------------------------------------

    #[test]
    fn default_binding_map_is_not_empty() {
        let map = BindingMap::default();
        assert!(!map.is_empty());
    }

    #[test]
    fn default_copy_binding() {
        let map = BindingMap::default();
        let combo = KeyCombo::new(BindingKey::C, BindingModifiers::CTRL_SHIFT);
        assert_eq!(map.lookup(&combo), Some(KeyAction::Copy));
    }

    #[test]
    fn default_paste_binding() {
        let map = BindingMap::default();
        let combo = KeyCombo::new(BindingKey::V, BindingModifiers::CTRL_SHIFT);
        assert_eq!(map.lookup(&combo), Some(KeyAction::Paste));
    }

    #[test]
    fn default_new_tab_binding() {
        let map = BindingMap::default();
        let combo = KeyCombo::new(BindingKey::T, BindingModifiers::CTRL_SHIFT);
        assert_eq!(map.lookup(&combo), Some(KeyAction::NewTab));
    }

    #[test]
    fn default_zoom_in_has_two_combos() {
        let map = BindingMap::default();
        let combos = map.all_combos_for(KeyAction::ZoomIn);
        assert_eq!(combos.len(), 2, "ZoomIn should have Ctrl+= and Ctrl+Plus");
    }

    #[test]
    fn default_scroll_page_up_binding() {
        let map = BindingMap::default();
        let combo = KeyCombo::new(BindingKey::PageUp, BindingModifiers::SHIFT);
        assert_eq!(map.lookup(&combo), Some(KeyAction::ScrollPageUp));
    }

    #[test]
    fn default_open_settings_binding() {
        let map = BindingMap::default();
        let combo = KeyCombo::new(BindingKey::Comma, BindingModifiers::CTRL_SHIFT);
        assert_eq!(map.lookup(&combo), Some(KeyAction::OpenSettings));
    }

    #[test]
    fn lookup_unbound_combo_returns_none() {
        let map = BindingMap::default();
        let combo = KeyCombo::bare(BindingKey::Z);
        assert_eq!(map.lookup(&combo), None);
    }

    #[test]
    fn combo_for_returns_a_bound_combo() {
        let map = BindingMap::default();
        let combo = map.combo_for(KeyAction::Copy);
        assert!(combo.is_some());
        // Verify the reverse lookup is consistent.
        assert_eq!(map.lookup(&combo.unwrap()), Some(KeyAction::Copy));
    }

    #[test]
    fn combo_for_unbound_action_returns_none() {
        let map = BindingMap::empty();
        assert_eq!(map.combo_for(KeyAction::Copy), None);
    }

    #[test]
    fn bind_overwrites_previous() {
        let mut map = BindingMap::empty();
        let combo = KeyCombo::bare(BindingKey::A);
        map.bind(combo, KeyAction::Copy);
        let prev = map.bind(combo, KeyAction::Paste);
        assert_eq!(prev, Some(KeyAction::Copy));
        assert_eq!(map.lookup(&combo), Some(KeyAction::Paste));
    }

    #[test]
    fn unbind_combo_removes_binding() {
        let mut map = BindingMap::default();
        let combo = KeyCombo::new(BindingKey::C, BindingModifiers::CTRL_SHIFT);
        let removed = map.unbind_combo(&combo);
        assert_eq!(removed, Some(KeyAction::Copy));
        assert_eq!(map.lookup(&combo), None);
    }

    #[test]
    fn unbind_action_removes_all_combos() {
        let mut map = BindingMap::default();
        let removed = map.unbind_action(KeyAction::ZoomIn);
        assert_eq!(removed.len(), 2);
        assert!(map.all_combos_for(KeyAction::ZoomIn).is_empty());
    }

    #[test]
    fn apply_overrides_adds_new_binding() {
        let mut map = BindingMap::empty();
        let mut overrides = HashMap::new();
        overrides.insert("copy".to_string(), "Ctrl+Shift+C".to_string());
        map.apply_overrides(&overrides).unwrap();
        let combo = KeyCombo::new(BindingKey::C, BindingModifiers::CTRL_SHIFT);
        assert_eq!(map.lookup(&combo), Some(KeyAction::Copy));
    }

    #[test]
    fn apply_overrides_removes_with_none() {
        let mut map = BindingMap::default();
        let mut overrides = HashMap::new();
        overrides.insert("copy".to_string(), "none".to_string());
        map.apply_overrides(&overrides).unwrap();
        assert!(map.all_combos_for(KeyAction::Copy).is_empty());
    }

    #[test]
    fn apply_overrides_removes_with_empty() {
        let mut map = BindingMap::default();
        let mut overrides = HashMap::new();
        overrides.insert("copy".to_string(), String::new());
        map.apply_overrides(&overrides).unwrap();
        assert!(map.all_combos_for(KeyAction::Copy).is_empty());
    }

    #[test]
    fn apply_overrides_rejects_bad_action() {
        let mut map = BindingMap::default();
        let mut overrides = HashMap::new();
        overrides.insert("launch_rockets".to_string(), "Ctrl+R".to_string());
        let err = map.apply_overrides(&overrides).unwrap_err();
        assert!(matches!(err, KeyBindingError::UnknownAction(_)));
    }

    #[test]
    fn apply_overrides_rejects_bad_combo() {
        let mut map = BindingMap::default();
        let mut overrides = HashMap::new();
        overrides.insert("copy".to_string(), "Ctrl+???".to_string());
        let err = map.apply_overrides(&overrides).unwrap_err();
        assert!(matches!(err, KeyBindingError::UnknownKey(_)));
    }

    #[test]
    fn empty_map_is_empty() {
        let map = BindingMap::empty();
        assert!(map.is_empty());
        assert_eq!(map.len(), 0);
    }

    #[test]
    fn iter_yields_all_bindings() {
        let map = BindingMap::default();
        let count = map.iter().count();
        assert_eq!(count, map.len());
    }

    #[test]
    fn default_switch_to_tab_bindings() {
        let map = BindingMap::default();
        let expected = [
            (BindingKey::Num1, KeyAction::SwitchToTab1),
            (BindingKey::Num2, KeyAction::SwitchToTab2),
            (BindingKey::Num3, KeyAction::SwitchToTab3),
            (BindingKey::Num4, KeyAction::SwitchToTab4),
            (BindingKey::Num5, KeyAction::SwitchToTab5),
            (BindingKey::Num6, KeyAction::SwitchToTab6),
            (BindingKey::Num7, KeyAction::SwitchToTab7),
            (BindingKey::Num8, KeyAction::SwitchToTab8),
            (BindingKey::Num9, KeyAction::SwitchToTab9),
        ];
        for (key, action) in expected {
            let combo = KeyCombo::new(key, BindingModifiers::CTRL_SHIFT);
            assert_eq!(
                map.lookup(&combo),
                Some(action),
                "default binding for {combo} should be {action}"
            );
        }
    }

    #[test]
    fn default_scroll_bindings() {
        let map = BindingMap::default();
        let expected = [
            (BindingKey::PageUp, KeyAction::ScrollPageUp),
            (BindingKey::PageDown, KeyAction::ScrollPageDown),
            (BindingKey::Home, KeyAction::ScrollToTop),
            (BindingKey::End, KeyAction::ScrollToBottom),
            (BindingKey::ArrowUp, KeyAction::ScrollLineUp),
            (BindingKey::ArrowDown, KeyAction::ScrollLineDown),
        ];
        for (key, action) in expected {
            let combo = KeyCombo::new(key, BindingModifiers::SHIFT);
            assert_eq!(
                map.lookup(&combo),
                Some(action),
                "default binding for {combo} should be {action}"
            );
        }
    }

    // ── display_label tests ──────────────────────────────────────────────

    #[test]
    fn display_label_non_empty_for_all_actions() {
        for action in KeyAction::ALL {
            let label = action.display_label();
            assert!(!label.is_empty(), "{action:?} has empty display_label()");
        }
    }

    #[test]
    fn display_label_distinct_for_all_actions() {
        let mut seen = std::collections::HashSet::new();
        for action in KeyAction::ALL {
            let label = action.display_label();
            assert!(
                seen.insert(label),
                "duplicate display_label() {label:?} for {action:?}"
            );
        }
    }

    // ── name() → FromStr round-trip completeness ─────────────────────────

    #[test]
    fn key_action_name_roundtrip_all() {
        for action in KeyAction::ALL {
            let name = action.name();
            let parsed: KeyAction = name
                .parse()
                .unwrap_or_else(|_| panic!("failed to parse name {name:?} for {action:?}"));
            assert_eq!(
                *action, parsed,
                "round-trip failed for {action:?} (name={name:?})"
            );
        }
    }

    // ── Default binding exhaustive tests ──────────────────────────────────

    #[test]
    fn default_next_tab_binding() {
        let map = BindingMap::default();
        let combo = KeyCombo::new(
            BindingKey::Tab,
            BindingModifiers {
                ctrl: true,
                shift: false,
                alt: false,
            },
        );
        assert_eq!(map.lookup(&combo), Some(KeyAction::NextTab));
    }

    #[test]
    fn default_prev_tab_binding() {
        let map = BindingMap::default();
        let combo = KeyCombo::new(
            BindingKey::Tab,
            BindingModifiers {
                ctrl: true,
                shift: true,
                alt: false,
            },
        );
        assert_eq!(map.lookup(&combo), Some(KeyAction::PrevTab));
    }

    #[test]
    fn default_zoom_out_binding() {
        let map = BindingMap::default();
        let combo = KeyCombo::new(
            BindingKey::Minus,
            BindingModifiers {
                ctrl: true,
                shift: false,
                alt: false,
            },
        );
        assert_eq!(map.lookup(&combo), Some(KeyAction::ZoomOut));
    }

    #[test]
    fn default_zoom_reset_binding() {
        let map = BindingMap::default();
        let combo = KeyCombo::new(
            BindingKey::Num0,
            BindingModifiers {
                ctrl: true,
                shift: false,
                alt: false,
            },
        );
        assert_eq!(map.lookup(&combo), Some(KeyAction::ZoomReset));
    }

    #[test]
    fn default_zoom_in_specific_combos() {
        let map = BindingMap::default();
        let combos = map.all_combos_for(KeyAction::ZoomIn);
        assert_eq!(combos.len(), 2, "ZoomIn should have exactly 2 combos");

        let equals = KeyCombo::new(
            BindingKey::Equals,
            BindingModifiers {
                ctrl: true,
                shift: false,
                alt: false,
            },
        );
        let plus = KeyCombo::new(
            BindingKey::Plus,
            BindingModifiers {
                ctrl: true,
                shift: false,
                alt: false,
            },
        );
        assert!(
            combos.contains(&equals),
            "ZoomIn should include Ctrl+Equals"
        );
        assert!(combos.contains(&plus), "ZoomIn should include Ctrl+Plus");
    }

    #[test]
    fn default_close_pane_binding() {
        let map = BindingMap::default();
        let combo = KeyCombo::new(
            BindingKey::W,
            BindingModifiers {
                ctrl: true,
                shift: true,
                alt: false,
            },
        );
        // Ctrl+Shift+W now maps to ClosePane (was CloseTab before muxing).
        assert_eq!(map.lookup(&combo), Some(KeyAction::ClosePane));
    }

    #[test]
    fn default_binding_total_count() {
        // The default map should have a known number of bindings.
        // This catches silent additions or removals.
        let map = BindingMap::default();
        // Count: Copy(1) + Paste(1) + NewTab(1) + NextTab(1)
        //        + PrevTab(1) + SwitchToTab1-9(9) + ZoomIn(2) + ZoomOut(1)
        //        + ZoomReset(1) + OpenSettings(1) + OpenSearch(1)
        //        + PrevCommand(1) + NextCommand(1) + ScrollPageUp(1)
        //        + ScrollPageDown(1) + ScrollToTop(1) + ScrollToBottom(1)
        //        + ScrollLineUp(1) + ScrollLineDown(1)
        //        + SplitVertical(1) + SplitHorizontal(1) + ClosePane(1)
        //        + FocusPaneLeft/Down/Up/Right(4) + ResizePaneLeft/Down/Up/Right(4)
        //        + ZoomPane(1) + NewWindow(1) = 41
        assert_eq!(
            map.len(),
            41,
            "default binding map should have exactly 41 bindings"
        );
    }

    #[test]
    fn unbound_actions_not_in_default_map() {
        // These actions have no default binding.
        let map = BindingMap::default();
        let unbound = [
            KeyAction::CloseTab,
            KeyAction::MoveTabLeft,
            KeyAction::MoveTabRight,
            KeyAction::RenameTab,
            KeyAction::SelectAll,
            KeyAction::SearchNext,
            KeyAction::SearchPrev,
            KeyAction::ToggleMenuBar,
        ];
        for action in unbound {
            assert!(
                map.combo_for(action).is_none(),
                "{action:?} should have no default binding"
            );
        }
    }

    // ── combo_for determinism on multi-combo action ────────────────────

    #[test]
    fn combo_for_multi_combo_action_returns_smallest_combo() {
        let map = BindingMap::default();
        // ZoomIn has Ctrl+Equals and Ctrl+Plus. combo_for must always return
        // the same one (the Ord-smallest) regardless of HashMap iteration order.
        let combo = map.combo_for(KeyAction::ZoomIn).unwrap();
        let all = map.all_combos_for(KeyAction::ZoomIn);
        assert_eq!(all.len(), 2, "ZoomIn should have exactly 2 combos");
        assert_eq!(
            combo, all[0],
            "combo_for must return the Ord-smallest combo (first in sorted all_combos_for)"
        );

        // Call combo_for multiple times and verify it is stable.
        for _ in 0..100 {
            assert_eq!(
                map.combo_for(KeyAction::ZoomIn),
                Some(combo),
                "combo_for must be deterministic across calls"
            );
        }
    }

    // ── is_alphanumeric tests ────────────────────────────────────────────

    #[test]
    fn is_alphanumeric_letters() {
        let letters = [BindingKey::A, BindingKey::B, BindingKey::C, BindingKey::Z];
        for key in letters {
            assert!(key.is_alphanumeric(), "{key:?} should be alphanumeric");
        }
    }

    #[test]
    fn is_alphanumeric_digits() {
        let digits = [BindingKey::Num0, BindingKey::Num5, BindingKey::Num9];
        for key in digits {
            assert!(key.is_alphanumeric(), "{key:?} should be alphanumeric");
        }
    }

    #[test]
    fn is_alphanumeric_non_text_keys() {
        let non_alpha = [
            BindingKey::F1,
            BindingKey::ArrowUp,
            BindingKey::PageUp,
            BindingKey::Tab,
            BindingKey::Enter,
            BindingKey::Escape,
            BindingKey::Plus,
            BindingKey::Minus,
            BindingKey::Comma,
            BindingKey::Space,
        ];
        for key in non_alpha {
            assert!(!key.is_alphanumeric(), "{key:?} should not be alphanumeric");
        }
    }

    // --- KeyAction Display ---

    #[test]
    fn key_action_display_delegates_to_name() {
        // Exercises `fmt::Display for KeyAction` (lines 899-901).
        for action in KeyAction::ALL {
            let display = format!("{action}");
            assert_eq!(display, action.name(), "Display must equal name()");
        }
    }

    // --- BindingKey::name() exhaustive coverage ---

    #[test]
    fn binding_key_name_all_variants_roundtrip() {
        // Exercises every arm of `BindingKey::name()` by iterating ALL known keys
        // and parsing their name() back.  This covers every `name()` arm that
        // binding_key_parse_* tests did not already hit.
        let all_keys = [
            BindingKey::A,
            BindingKey::B,
            BindingKey::C,
            BindingKey::D,
            BindingKey::E,
            BindingKey::F,
            BindingKey::G,
            BindingKey::H,
            BindingKey::I,
            BindingKey::J,
            BindingKey::K,
            BindingKey::L,
            BindingKey::M,
            BindingKey::N,
            BindingKey::O,
            BindingKey::P,
            BindingKey::Q,
            BindingKey::R,
            BindingKey::S,
            BindingKey::T,
            BindingKey::U,
            BindingKey::V,
            BindingKey::W,
            BindingKey::X,
            BindingKey::Y,
            BindingKey::Z,
            BindingKey::Num0,
            BindingKey::Num1,
            BindingKey::Num2,
            BindingKey::Num3,
            BindingKey::Num4,
            BindingKey::Num5,
            BindingKey::Num6,
            BindingKey::Num7,
            BindingKey::Num8,
            BindingKey::Num9,
            BindingKey::F1,
            BindingKey::F2,
            BindingKey::F3,
            BindingKey::F4,
            BindingKey::F5,
            BindingKey::F6,
            BindingKey::F7,
            BindingKey::F8,
            BindingKey::F9,
            BindingKey::F10,
            BindingKey::F11,
            BindingKey::F12,
            BindingKey::ArrowUp,
            BindingKey::ArrowDown,
            BindingKey::ArrowLeft,
            BindingKey::ArrowRight,
            BindingKey::Home,
            BindingKey::End,
            BindingKey::PageUp,
            BindingKey::PageDown,
            BindingKey::Insert,
            BindingKey::Delete,
            BindingKey::Backspace,
            BindingKey::Tab,
            BindingKey::Enter,
            BindingKey::Space,
            BindingKey::Escape,
            BindingKey::Plus,
            BindingKey::Minus,
            BindingKey::Equals,
            BindingKey::Comma,
            BindingKey::Period,
            BindingKey::Semicolon,
            BindingKey::Colon,
            BindingKey::Slash,
            BindingKey::Backslash,
            BindingKey::OpenBracket,
            BindingKey::CloseBracket,
            BindingKey::Backtick,
            BindingKey::Quote,
            BindingKey::Pipe,
        ];
        for key in all_keys {
            let name = key.name();
            let parsed: BindingKey = name.parse().unwrap_or_else(|e| {
                panic!("BindingKey::name() output {name:?} should parse back: {e}")
            });
            assert_eq!(key, parsed, "name→parse roundtrip failed for {key:?}");
        }
    }

    #[test]
    #[cfg(not(target_os = "macos"))]
    fn display_platform_linux_format() {
        // On Linux (the test host), display_platform should produce "Ctrl+Shift+T" style.
        let combo = KeyCombo::new(BindingKey::T, BindingModifiers::CTRL_SHIFT);
        let displayed = combo.display_platform();
        assert_eq!(displayed, "Ctrl+Shift+T");
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn display_platform_macos_format() {
        let combo = KeyCombo::new(BindingKey::T, BindingModifiers::CTRL_SHIFT);
        let displayed = combo.display_platform();
        assert_eq!(displayed, "\u{2318}\u{21E7}T");
    }

    #[test]
    fn display_platform_bare_key() {
        let combo = KeyCombo::bare(BindingKey::Escape);
        let displayed = combo.display_platform();
        assert_eq!(displayed, "Escape");
    }

    #[test]
    fn combo_for_returns_bound_action() {
        let map = BindingMap::default();
        // NewTab has a default binding of Ctrl+Shift+T.
        let combo = map.combo_for(KeyAction::NewTab);
        assert!(combo.is_some(), "NewTab should have a default binding");
        let combo = combo.unwrap();
        assert_eq!(combo.key, BindingKey::T);
        assert!(combo.modifiers.ctrl);
        assert!(combo.modifiers.shift);
    }

    #[test]
    fn combo_for_returns_none_for_unbound() {
        let map = BindingMap::default();
        // CloseTab has no default binding.
        let combo = map.combo_for(KeyAction::CloseTab);
        assert!(combo.is_none(), "CloseTab should have no default binding");
    }
}
