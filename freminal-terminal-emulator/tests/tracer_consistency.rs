// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use freminal_terminal_emulator::ansi_components::{
    csi::AnsiCsiParser, osc::AnsiOscParser, standard::StandardParser,
};
use proptest::{prelude::any, prop_assert_eq, proptest};

/// Ensure all parser variants record pushed bytes into their trace buffer.
#[test]
fn seq_trace_updates_on_push() {
    // OSC
    let mut osc = AnsiOscParser::new();
    osc.push(b'A');
    assert!(
        osc.trace_str().contains('A'),
        "OSC parser did not record byte in trace"
    );

    // CSI
    let mut csi = AnsiCsiParser::new();
    csi.push(b'B');
    assert!(
        csi.trace_str().contains('B'),
        "CSI parser did not record byte in trace"
    );

    // Standard
    let mut std = StandardParser::new();
    std.push(b'C');
    assert!(
        std.trace_str().contains('C'),
        "Standard parser did not record byte in trace"
    );
}

// Ensure that pushing bytes in chunks produces the same final trace
// as pushing them in a single pass (streaming determinism).
proptest! {
    #[test]
    fn seq_trace_deterministic_across_chunking(input in proptest::collection::vec(any::<u8>(), 1..128)) {
        // OSC
        {
            let mut full = AnsiOscParser::new();
            let mut chunked = AnsiOscParser::new();
            for &b in &input { full.push(b); }
            for chunk in input.chunks(3) { for &b in chunk { chunked.push(b); } }
            prop_assert_eq!(full.trace_str(), chunked.trace_str());
        }

        // CSI
        {
            let mut full = AnsiCsiParser::new();
            let mut chunked = AnsiCsiParser::new();
            for &b in &input { full.push(b); }
            for chunk in input.chunks(3) { for &b in chunk { chunked.push(b); } }
            prop_assert_eq!(full.trace_str(), chunked.trace_str());
        }

        // Standard
        {
            let mut full = StandardParser::new();
            let mut chunked = StandardParser::new();
            for &b in &input { full.push(b); }
            for chunk in input.chunks(3) { for &b in chunk { chunked.push(b); } }
            prop_assert_eq!(full.trace_str(), chunked.trace_str());
        }
    }
}
