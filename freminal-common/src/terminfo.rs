// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

// FIXME: I would really really like this to be compiled as part of the build pipeline
// We had it that way. However, because I am stupid (or cargo is stupid, unclear but likely me)
// it would ALWAYS rerun the `tic` and `tar` part of the build, even with a rerun-if-changed
// directive. This is a workaround until I can figure out how to make it work properly.
//
// WE NEED TO ALWAYS HAND RECOMPILE THE terminfo IF WE CHANGE IT!!!!
pub const TERMINFO: &[u8] = include_bytes!("../../res/terminfo.tar");
