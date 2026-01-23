// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

#[derive(Eq, PartialEq, Debug, Default, Clone)]
pub enum DecSpecialGraphics {
    Replace,
    #[default]
    DontReplace,
}
