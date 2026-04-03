// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

// NOTE: `terminfo.tar` is regenerated automatically by `freminal-terminal-emulator/build.rs`
// whenever `res/freminal.ti` changes.  The build script runs `tic` + `tar` and writes the result
// back into `res/terminfo.tar` which is committed to source control so that builds without `tic`
// (e.g. Windows CI, minimal containers) still work.
pub const TERMINFO: &[u8] = include_bytes!("../../res/terminfo.tar");
