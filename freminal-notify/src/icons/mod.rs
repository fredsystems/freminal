// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! The icon pipeline: bounded decode, glyph raster, private temp files, XDG lookup.

#[cfg(any(unix, target_os = "windows", test))]
pub mod decode;
#[cfg(all(unix, not(target_os = "macos")))]
pub mod lookup;
#[cfg(any(target_os = "macos", target_os = "windows", test))]
pub mod raster;
#[cfg(any(target_os = "macos", target_os = "windows", test))]
pub mod tempfiles;
