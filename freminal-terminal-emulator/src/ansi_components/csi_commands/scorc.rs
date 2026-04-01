// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use freminal_common::buffer_states::terminal_output::TerminalOutput;

/// CSI u — SCORC (Restore Cursor Position) and Kitty keyboard protocol
///
/// The `CSI u` final byte is shared between two unrelated protocols:
///
/// **SCORC (SCO Restore Cursor Position):**
/// Plain `CSI u` with no parameters restores the cursor to the position
/// previously saved with `CSI s` (SCOSC).
///
/// **Kitty keyboard protocol:**
/// - `CSI ? u` — Query current keyboard mode flags.  Respond with
///   `CSI ? 0 u` (mode 0 = protocol not active).
/// - `CSI > flags u` — Push a new keyboard mode onto the stack.
///   We do not support the Kitty keyboard protocol, so this is ignored.
/// - `CSI < number u` — Pop keyboard mode(s) from the stack.
///   We do not support the Kitty keyboard protocol, so this is ignored.
///
/// **Disambiguation:** The leading byte of `params` determines which
/// protocol is in effect:
/// - Empty params → SCORC
/// - `?` prefix  → Kitty query
/// - `>` prefix  → Kitty push
/// - `<` prefix  → Kitty pop
/// - Anything else with digits only → SCORC (numeric params are valid for
///   some SCORC implementations, though rarely used)
pub fn ansi_parser_inner_csi_finished_scorc(params: &[u8], output: &mut Vec<TerminalOutput>) {
    match params.first() {
        None => {
            // Plain CSI u — SCORC: restore cursor position.
            output.push(TerminalOutput::RestoreCursor);
        }
        Some(b'?') => {
            // CSI ? u — Kitty keyboard protocol query.
            // Respond with CSI ? 0 u (protocol not active / mode flags = 0).
            output.push(TerminalOutput::KittyKeyboardQuery);
        }
        Some(b'>') => {
            // CSI > flags u — Kitty keyboard push.
            // We do not implement the Kitty keyboard protocol; silently ignore.
            trace!("Kitty keyboard push (CSI > u) ignored: params={params:?}");
        }
        Some(b'<') => {
            // CSI < number u — Kitty keyboard pop.
            // We do not implement the Kitty keyboard protocol; silently ignore.
            trace!("Kitty keyboard pop (CSI < u) ignored: params={params:?}");
        }
        Some(_) => {
            // Numeric-only params: treat as SCORC (some terminals accept
            // CSI Ps u as a parameterised restore, though this is rare).
            output.push(TerminalOutput::RestoreCursor);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_csi_u_is_scorc() {
        let mut output = Vec::new();
        ansi_parser_inner_csi_finished_scorc(b"", &mut output);
        assert_eq!(output, vec![TerminalOutput::RestoreCursor]);
    }

    #[test]
    fn csi_question_u_is_kitty_query() {
        let mut output = Vec::new();
        ansi_parser_inner_csi_finished_scorc(b"?", &mut output);
        assert_eq!(output, vec![TerminalOutput::KittyKeyboardQuery]);
    }

    #[test]
    fn csi_gt_u_is_kitty_push_ignored() {
        let mut output = Vec::new();
        ansi_parser_inner_csi_finished_scorc(b">1", &mut output);
        assert!(output.is_empty(), "Kitty push should be silently ignored");
    }

    #[test]
    fn csi_lt_u_is_kitty_pop_ignored() {
        let mut output = Vec::new();
        ansi_parser_inner_csi_finished_scorc(b"<1", &mut output);
        assert!(output.is_empty(), "Kitty pop should be silently ignored");
    }

    #[test]
    fn numeric_params_are_scorc() {
        let mut output = Vec::new();
        ansi_parser_inner_csi_finished_scorc(b"1", &mut output);
        assert_eq!(output, vec![TerminalOutput::RestoreCursor]);
    }
}
