// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

/// freminal's application icon, the one PNG embedded in the binary. It is the
/// OS window icon (`run`) and, on Linux/BSD, the picture of every plain
/// desktop notification (`osc99::dbus_service`).
pub(in crate::gui) static FREMINAL_ICON_PNG: &[u8] = include_bytes!("../../../assets/icon.png");
