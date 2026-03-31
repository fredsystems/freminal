// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use super::mode::SetMode;

pub mod allow_column_mode_switch;
pub mod alternate_scroll;
pub mod application_escape_key;
pub mod decarm;
pub mod decawm;
pub mod decbkm;
pub mod decckm;
pub mod deccolm;
pub mod decnkm;
pub mod decom;
pub mod decsclm;
pub mod decscnm;
pub mod dectcem;
pub mod grapheme;
pub mod keypad;
pub mod lnm;
pub mod modify_other_keys_mode;
pub mod mouse;
pub mod reverse_wrap_around;
pub mod rl_bracket;
pub mod sync_updates;
pub mod theme;
pub mod unknown;
pub mod xtcblink;
pub mod xtextscrn;
pub mod xtmsewin;

pub trait ReportMode {
    fn report(&self, override_mode: Option<SetMode>) -> String;
}

pub trait MouseModeNumber {
    fn mouse_mode_number(&self) -> usize;
}
