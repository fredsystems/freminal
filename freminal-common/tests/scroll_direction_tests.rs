// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use freminal_common::scroll::ScrollDirection; // adjust module path if needed
use proptest::{prop_assert, prop_assert_eq, proptest};

/// ---------- Deterministic Unit Tests ----------

#[test]
fn default_is_up_one() {
    assert_eq!(ScrollDirection::default(), ScrollDirection::Up(1));
}

#[test]
fn equality_and_clone_work() {
    let a = ScrollDirection::Up(3);
    let b = ScrollDirection::Up(3);
    let c = ScrollDirection::Down(3);
    let d = a.clone();

    assert_eq!(a, b);
    assert_ne!(a, c);
    assert_eq!(a, d);
}

#[test]
fn debug_output_contains_variant_name() {
    let up = ScrollDirection::Up(5);
    let down = ScrollDirection::Down(2);

    let up_str = format!("{up:?}");
    let down_str = format!("{down:?}");

    assert!(up_str.contains("Up"));
    assert!(down_str.contains("Down"));
    assert!(up_str.contains("5"));
    assert!(down_str.contains("2"));
}

#[test]
fn stores_and_preserves_value() {
    let up = ScrollDirection::Up(10);
    let down = ScrollDirection::Down(42);

    if let ScrollDirection::Up(v) = up {
        assert_eq!(v, 10);
    } else {
        panic!("Expected ScrollDirection::Up");
    }

    if let ScrollDirection::Down(v) = down {
        assert_eq!(v, 42);
    } else {
        panic!("Expected ScrollDirection::Down");
    }
}

proptest! {
    /// Arbitrary payloads preserve their usize value when pattern matched.
    #[test]
    fn up_and_down_preserve_value(v in 0usize..=10_000usize) {
        let up = ScrollDirection::Up(v);
        let down = ScrollDirection::Down(v);

        if let ScrollDirection::Up(inner) = up {
            prop_assert_eq!(inner, v);
        } else {
            prop_assert!(false, "Expected Up variant");
        }

        if let ScrollDirection::Down(inner) = down {
            prop_assert_eq!(inner, v);
        } else {
            prop_assert!(false, "Expected Down variant");
        }
    }

    /// Cloning produces identical value and variant.
    #[test]
    fn clone_preserves_variant_and_value(v in 0usize..=1000usize) {
        let dir = if v % 2 == 0 {
            ScrollDirection::Up(v)
        } else {
            ScrollDirection::Down(v)
        };

        let clone = dir.clone();
        prop_assert_eq!(dir, clone);
    }
}
