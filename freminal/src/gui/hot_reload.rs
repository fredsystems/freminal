// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

// Hot-reload propagation is now handled inline in `FreminalGui::update()`
// by iterating `self.windows`. The old `propagate_shader_to_secondary_windows`,
// `propagate_bg_image_to_secondary_windows`, and `copy_root_shader_to` methods
// have been removed as part of the multi-window parity refactor (Task 64).
