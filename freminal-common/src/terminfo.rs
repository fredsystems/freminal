// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! The embedded compiled terminfo archive.
//!
//! [`TERMINFO`] is `res/terminfo.tar`: exactly four regular files (no directory entries), the
//! compiled `xterm-freminal` entry and its `freminal` alias, each in both directory layouts --
//! the letter layout (`x/xterm-freminal`, `f/freminal`) used by most ncurses and the hex layout
//! (`78/xterm-freminal`, `66/freminal`) used by Apple's ncurses.  The letter and hex copies of an
//! entry are byte-identical.
//!
//! The archive is packed from the committed tree `res/terminfo/`, and both are regenerated
//! automatically by `freminal-terminal-emulator/build.rs` whenever `res/freminal.ti` changes: the
//! build script runs `tic -x`, writes the four files, and packs the tar.  Both are committed to
//! source control so that builds without `tic` (e.g. Windows CI, minimal containers) still work
//! and so that packaging (the macOS bundle, the rpm) can copy the tree without running `tic`.

/// The compiled terminfo archive, extracted per PTY into a temporary terminfo directory.
pub const TERMINFO: &[u8] = include_bytes!("../../res/terminfo.tar");
