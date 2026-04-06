// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Typed enums for every DEC private mode supported by Freminal.
//!
//! Each submodule defines a two-variant enum (`Enabled`/`Disabled` or similar)
//! that replaces a raw `bool` flag. Using typed enums prevents accidental
//! inversion at call sites and makes mode state self-documenting in function
//! signatures and struct fields.

use super::mode::SetMode;

pub mod allow_alt_screen;
pub mod allow_column_mode_switch;
pub mod alternate_scroll;
pub mod application_escape_key;
pub mod decanm;
pub mod decarm;
pub mod decawm;
pub mod decbkm;
pub mod decckm;
pub mod deccolm;
pub mod declrmm;
pub mod decnkm;
pub mod decnrcm;
pub mod decom;
pub mod decsclm;
pub mod decscnm;
pub mod decsdm;
pub mod dectcem;
pub mod grapheme;
pub mod irm;
pub mod keypad;
pub mod kitty_keyboard;
pub mod lnm;
pub mod modify_other_keys_mode;
pub mod mouse;
pub mod private_color_registers;
pub mod reverse_wrap_around;
pub mod rl_bracket;
pub mod s8c1t;
pub mod sync_updates;
pub mod theme;
pub mod unknown;
pub mod xt_rev_wrap2;
pub mod xtcblink;
pub mod xtextscrn;
pub mod xtmsewin;

/// Implemented by all DEC private mode enums to produce a `DECRPM` response
/// string for `CSI ? Ps $ p` queries.
pub trait ReportMode {
    /// Format a `CSI ? Ps ; Pm $ y` response for this mode.
    ///
    /// `override_mode` allows the caller to substitute a specific `SetMode`
    /// value instead of the current mode state (used by the emulator when
    /// reporting modes that are not directly stored as this type).
    fn report(&self, override_mode: Option<SetMode>) -> String;
}

/// Implemented by mouse-mode enums that need to expose their ANSI mode number
/// for mode-reporting and X11 encoding.
pub trait MouseModeNumber {
    /// Return the DEC private mode number for this mouse mode variant.
    fn mouse_mode_number(&self) -> usize;
}
