// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use freminal_common::buffer_states::window_manipulation::WindowManipulation;
use proptest::{prop_assert, prop_assert_eq, proptest};
use std::convert::TryFrom;

/// ---------- Deterministic Unit Tests ----------

#[test]
fn basic_variants_without_payload() {
    assert_eq!(
        WindowManipulation::try_from((1, 0, 0)).unwrap(),
        WindowManipulation::DeIconifyWindow
    );
    assert_eq!(
        WindowManipulation::try_from((2, 0, 0)).unwrap(),
        WindowManipulation::MinimizeWindow
    );
    assert_eq!(
        WindowManipulation::try_from((5, 0, 0)).unwrap(),
        WindowManipulation::RaiseWindowToTopOfStackingOrder
    );
    assert_eq!(
        WindowManipulation::try_from((6, 0, 0)).unwrap(),
        WindowManipulation::LowerWindowToBottomOfStackingOrder
    );
    assert_eq!(
        WindowManipulation::try_from((7, 0, 0)).unwrap(),
        WindowManipulation::RefreshWindow
    );
    assert_eq!(
        WindowManipulation::try_from((9, 1, 0)).unwrap(),
        WindowManipulation::MaximizeWindow
    );
    assert_eq!(
        WindowManipulation::try_from((9, 0, 0)).unwrap(),
        WindowManipulation::RestoreNonMaximizedWindow
    );
    assert_eq!(
        WindowManipulation::try_from((10, 0, 0)).unwrap(),
        WindowManipulation::NotFullScreen
    );
    assert_eq!(
        WindowManipulation::try_from((10, 1, 0)).unwrap(),
        WindowManipulation::FullScreen
    );
    assert_eq!(
        WindowManipulation::try_from((10, 2, 0)).unwrap(),
        WindowManipulation::ToggleFullScreen
    );
    assert_eq!(
        WindowManipulation::try_from((11, 0, 0)).unwrap(),
        WindowManipulation::ReportWindowState
    );
}

#[test]
fn position_and_size_reports() {
    assert_eq!(
        WindowManipulation::try_from((13, 0, 0)).unwrap(),
        WindowManipulation::ReportWindowPositionWholeWindow
    );
    assert_eq!(
        WindowManipulation::try_from((13, 1, 0)).unwrap(),
        WindowManipulation::ReportWindowPositionWholeWindow
    );
    assert_eq!(
        WindowManipulation::try_from((13, 2, 0)).unwrap(),
        WindowManipulation::ReportWindowPositionTextArea
    );

    assert_eq!(
        WindowManipulation::try_from((14, 0, 0)).unwrap(),
        WindowManipulation::ReportWindowSizeInPixels
    );
    assert_eq!(
        WindowManipulation::try_from((14, 1, 0)).unwrap(),
        WindowManipulation::ReportWindowSizeInPixels
    );
    assert_eq!(
        WindowManipulation::try_from((14, 2, 0)).unwrap(),
        WindowManipulation::ReportWindowTextAreaSizeInPixels
    );

    assert_eq!(
        WindowManipulation::try_from((15, 0, 0)).unwrap(),
        WindowManipulation::ReportRootWindowSizeInPixels
    );
    assert_eq!(
        WindowManipulation::try_from((16, 0, 0)).unwrap(),
        WindowManipulation::ReportCharacterSizeInPixels
    );
    assert_eq!(
        WindowManipulation::try_from((18, 0, 0)).unwrap(),
        WindowManipulation::ReportTerminalSizeInCharacters
    );
    assert_eq!(
        WindowManipulation::try_from((19, 0, 0)).unwrap(),
        WindowManipulation::ReportRootWindowSizeInCharacters
    );
    assert_eq!(
        WindowManipulation::try_from((20, 0, 0)).unwrap(),
        WindowManipulation::ReportIconLabel
    );
    assert_eq!(
        WindowManipulation::try_from((21, 0, 0)).unwrap(),
        WindowManipulation::ReportTitle
    );
}

#[test]
fn save_restore_and_title_bar_text_variants() {
    assert_eq!(
        WindowManipulation::try_from((22, 0, 0)).unwrap(),
        WindowManipulation::SaveWindowTitleToStack
    );
    assert_eq!(
        WindowManipulation::try_from((23, 2, 0)).unwrap(),
        WindowManipulation::RestoreWindowTitleFromStack
    );

    // Title bar text should produce a variant with an empty string
    match WindowManipulation::try_from((24, 1, 0)).unwrap() {
        WindowManipulation::SetTitleBarText(s) => assert!(s.is_empty()),
        _ => panic!("Expected SetTitleBarText"),
    }
}

#[test]
fn payload_variants_are_correct() {
    let move_cmd = WindowManipulation::try_from((3, 10, 20)).unwrap();
    let resize_cmd = WindowManipulation::try_from((4, 80, 24)).unwrap();
    let resize_lines_cols = WindowManipulation::try_from((8, 50, 100)).unwrap();

    assert_eq!(move_cmd, WindowManipulation::MoveWindow(10, 20));
    assert_eq!(resize_cmd, WindowManipulation::ResizeWindow(80, 24));
    assert_eq!(
        resize_lines_cols,
        WindowManipulation::ResizeWindowToLinesAndColumns(50, 100)
    );
}

#[test]
fn invalid_command_returns_error() {
    let result = WindowManipulation::try_from((99, 0, 0));
    assert!(result.is_err());
}

#[test]
fn clone_and_debug_work() {
    let cmd = WindowManipulation::FullScreen;
    let cloned = cmd.clone();
    assert_eq!(cmd, cloned);

    let s = format!("{cmd:?}");
    assert!(s.contains("FullScreen"));
}

proptest! {
    /// Any command code not explicitly defined should yield an error.
    #[test]
    fn invalid_commands_return_error(cmd in 25usize..=1000usize, p2 in 0usize..10usize, p3 in 0usize..10usize) {
        let result = WindowManipulation::try_from((cmd, p2, p3));
        prop_assert!(result.is_err());
    }

    /// Move and resize preserve coordinates.
    #[test]
    fn payload_values_preserved(x in 0usize..500, y in 0usize..500) {
        let move_cmd = WindowManipulation::try_from((3, x, y)).unwrap();
        let resize_cmd = WindowManipulation::try_from((4, x, y)).unwrap();
        let resize_lines = WindowManipulation::try_from((8, x, y)).unwrap();

        match move_cmd {
            WindowManipulation::MoveWindow(a, b) => prop_assert_eq!((a,b), (x,y)),
            _ => prop_assert!(false, "Expected MoveWindow"),
        }
        match resize_cmd {
            WindowManipulation::ResizeWindow(a, b) => prop_assert_eq!((a,b), (x,y)),
            _ => prop_assert!(false, "Expected ResizeWindow"),
        }
        match resize_lines {
            WindowManipulation::ResizeWindowToLinesAndColumns(a, b) => prop_assert_eq!((a,b), (x,y)),
            _ => prop_assert!(false, "Expected ResizeWindowToLinesAndColumns"),
        }
    }
}
