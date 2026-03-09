// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use freminal_common::cursor::CursorVisualStyle; // adjust module path if needed
use proptest::{prop_assert_eq, prop_assert_ne, proptest};

/// ---------- Deterministic Unit Tests ----------

#[test]
fn default_is_block_cursor_steady() {
    assert_eq!(
        CursorVisualStyle::default(),
        CursorVisualStyle::BlockCursorSteady
    );
}

#[test]
fn equality_and_debug_work() {
    let a = CursorVisualStyle::BlockCursorBlink;
    let b = CursorVisualStyle::BlockCursorBlink;
    let c = CursorVisualStyle::UnderlineCursorSteady;

    assert_eq!(a, b);
    assert_ne!(a, c);

    // Debug output should contain the variant name
    let dbg = format!("{a:?}");
    assert!(dbg.contains("BlockCursorBlink"));
}

#[test]
fn from_usize_known_values() {
    assert_eq!(
        CursorVisualStyle::from(2),
        CursorVisualStyle::BlockCursorSteady
    );
    assert_eq!(
        CursorVisualStyle::from(3),
        CursorVisualStyle::UnderlineCursorBlink
    );
    assert_eq!(
        CursorVisualStyle::from(4),
        CursorVisualStyle::UnderlineCursorSteady
    );
    assert_eq!(
        CursorVisualStyle::from(5),
        CursorVisualStyle::VerticalLineCursorBlink
    );
    assert_eq!(
        CursorVisualStyle::from(6),
        CursorVisualStyle::VerticalLineCursorSteady
    );
}

#[test]
fn from_usize_default_fallback() {
    // Anything not in 2..=6 falls back to BlockCursorBlink
    for v in [0, 1, 7, 42, usize::MAX] {
        assert_eq!(
            CursorVisualStyle::from(v),
            CursorVisualStyle::BlockCursorBlink
        );
    }
}

proptest! {
    /// Values outside 2..=6 map to BlockCursorBlink
    #[test]
    fn from_usize_outside_range_defaults_to_block(value in proptest::num::usize::ANY) {
        let style = CursorVisualStyle::from(value);
        if !(2..=6).contains(&value) {
            prop_assert_eq!(style, CursorVisualStyle::BlockCursorBlink);
        }
    }

    /// Values 2–6 map to non-default styles
    #[test]
    fn from_usize_in_range_not_default(value in 2usize..=6usize) {
        let style = CursorVisualStyle::from(value);
        prop_assert_ne!(style, CursorVisualStyle::BlockCursorBlink);
    }
}
