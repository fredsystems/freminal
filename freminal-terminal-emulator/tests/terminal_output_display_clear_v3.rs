// Copyright (C) 2024-2026 Fred Clausen
// This test guards the Display string mapping for clear-display variants.

use freminal_common::buffer_states::terminal_output::TerminalOutput;

#[test]
fn display_clear_display_backwards_maps_stably() {
    // Ensure the corrected variant name is available and Display is stable.
    let variant = TerminalOutput::ClearDisplayfromStartofDisplaytoCursor;
    let s = variant.to_string();
    // Historically this maps to "ClearBackwards"; keep that stable.
    assert_eq!(s, "ClearBackwards");
}
