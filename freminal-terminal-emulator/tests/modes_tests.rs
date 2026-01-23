// Copyright (C) 2024–2025 Fred Clausen
// Licensed under the MIT license (https://opensource.org/licenses/MIT).

use freminal_terminal_emulator::ansi_components::mode::SetMode;
use freminal_terminal_emulator::ansi_components::modes::ReportMode;
use freminal_terminal_emulator::ansi_components::modes::decawm::Decawm;

#[test]
fn default_is_autowrap_enabled() {
    let mode = Decawm::default();
    assert_eq!(mode, Decawm::AutoWrap);
    assert_eq!(format!("{}", mode), "Autowrap Mode (DECAWM) Enabled");
}

#[test]
fn new_from_set_mode_variants() {
    assert_eq!(Decawm::new(&SetMode::DecSet), Decawm::AutoWrap);
    assert_eq!(Decawm::new(&SetMode::DecRst), Decawm::NoAutoWrap);
    assert_eq!(Decawm::new(&SetMode::DecQuery), Decawm::Query);
}

#[test]
fn report_without_override_reflects_internal_state() {
    let no_autowrap = Decawm::NoAutoWrap;
    let autowrap = Decawm::AutoWrap;
    let query = Decawm::Query;

    assert_eq!(no_autowrap.report(None), "\x1b[?7;2$y");
    assert_eq!(autowrap.report(None), "\x1b[?7;1$y");
    assert_eq!(query.report(None), "\x1b[?7;0$y");
}

#[test]
fn report_with_override_takes_precedence() {
    let mode = Decawm::NoAutoWrap;

    assert_eq!(mode.report(Some(SetMode::DecSet)), "\x1b[?7;1$y");
    assert_eq!(mode.report(Some(SetMode::DecRst)), "\x1b[?7;2$y");
    assert_eq!(mode.report(Some(SetMode::DecQuery)), "\x1b[?7;0$y");
}

#[test]
fn display_and_debug_match_expected_strings() {
    let no_auto = Decawm::NoAutoWrap;
    let auto = Decawm::AutoWrap;
    let query = Decawm::Query;

    // Display
    assert_eq!(format!("{}", no_auto), "Autowrap Mode (DECAWM) Disabled");
    assert_eq!(format!("{}", auto), "Autowrap Mode (DECAWM) Enabled");
    assert_eq!(format!("{}", query), "Autowrap Mode (DECAWM) Query");

    // Debug should include variant names
    let dbg = format!("{:?} {:?} {:?}", no_auto, auto, query);
    assert!(dbg.contains("NoAutoWrap"));
    assert!(dbg.contains("AutoWrap"));
    assert!(dbg.contains("Query"));
}

#[test]
fn equality_and_clone_semantics() {
    let a = Decawm::AutoWrap;
    let b = a.clone();
    assert_eq!(a, b);
    assert_ne!(a, Decawm::NoAutoWrap);
}
