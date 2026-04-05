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
///   `CSI ? <flags> u` where `<flags>` is the current stack-top value.
/// - `CSI > flags u` — Push a new keyboard mode onto the stack.
/// - `CSI < number u` — Pop keyboard mode(s) from the stack.
/// - `CSI = flags ; mode u` — Set flags on the current stack entry.
///
/// **Disambiguation:** The leading byte of `params` determines which
/// protocol is in effect:
/// - Empty params → SCORC
/// - `?` prefix  → Kitty query
/// - `>` prefix  → Kitty push
/// - `<` prefix  → Kitty pop
/// - `=` prefix  → Kitty set
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
            output.push(TerminalOutput::KittyKeyboardQuery);
        }
        Some(b'>') => {
            // CSI > flags u — Kitty keyboard push.
            let flags = parse_decimal(&params[1..]).unwrap_or(0);
            output.push(TerminalOutput::KittyKeyboardPush(flags));
        }
        Some(b'<') => {
            // CSI < number u — Kitty keyboard pop.
            // Default to 1 per the spec; 0 also means 1.
            let n = parse_decimal(&params[1..]).unwrap_or(1).max(1);
            output.push(TerminalOutput::KittyKeyboardPop(n));
        }
        Some(b'=') => {
            // CSI = flags ; mode u — Kitty keyboard set.
            let (flags, mode) = parse_two_params(&params[1..]);
            // mode defaults to 1 (replace) per the spec.
            let mode = if mode == 0 { 1 } else { mode };
            output.push(TerminalOutput::KittyKeyboardSet { flags, mode });
        }
        Some(_) => {
            // Numeric-only params: treat as SCORC (some terminals accept
            // CSI Ps u as a parameterised restore, though this is rare).
            output.push(TerminalOutput::RestoreCursor);
        }
    }
}

/// Parse a decimal integer from a byte slice.  Returns `None` for empty or
/// non-digit input.
fn parse_decimal(bytes: &[u8]) -> Option<u32> {
    if bytes.is_empty() {
        return None;
    }
    let mut value: u32 = 0;
    for &b in bytes {
        if b.is_ascii_digit() {
            value = value.saturating_mul(10).saturating_add(u32::from(b - b'0'));
        } else {
            // Stop at the first non-digit (e.g. `;`).
            break;
        }
    }
    Some(value)
}

/// Parse two semicolon-separated decimal parameters from a byte slice.
///
/// Returns `(first, second)`.  Missing or non-parseable values default to 0.
fn parse_two_params(bytes: &[u8]) -> (u32, u32) {
    // Find the semicolon separator.
    let mut split_pos = None;
    for (i, &b) in bytes.iter().enumerate() {
        if b == b';' {
            split_pos = Some(i);
            break;
        }
    }

    let first = parse_decimal(bytes).unwrap_or(0);
    let second = split_pos.map_or(0, |pos| parse_decimal(&bytes[pos + 1..]).unwrap_or(0));

    (first, second)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── SCORC tests ─────────────────────────────────────────────────

    #[test]
    fn plain_csi_u_is_scorc() {
        let mut output = Vec::new();
        ansi_parser_inner_csi_finished_scorc(b"", &mut output);
        assert_eq!(output, vec![TerminalOutput::RestoreCursor]);
    }

    #[test]
    fn numeric_params_are_scorc() {
        let mut output = Vec::new();
        ansi_parser_inner_csi_finished_scorc(b"1", &mut output);
        assert_eq!(output, vec![TerminalOutput::RestoreCursor]);
    }

    // ── Kitty query ─────────────────────────────────────────────────

    #[test]
    fn csi_question_u_is_kitty_query() {
        let mut output = Vec::new();
        ansi_parser_inner_csi_finished_scorc(b"?", &mut output);
        assert_eq!(output, vec![TerminalOutput::KittyKeyboardQuery]);
    }

    // ── Kitty push ──────────────────────────────────────────────────

    #[test]
    fn csi_gt_u_push_flags_0() {
        let mut output = Vec::new();
        ansi_parser_inner_csi_finished_scorc(b">0", &mut output);
        assert_eq!(output, vec![TerminalOutput::KittyKeyboardPush(0)]);
    }

    #[test]
    fn csi_gt_u_push_flags_27() {
        let mut output = Vec::new();
        ansi_parser_inner_csi_finished_scorc(b">27", &mut output);
        assert_eq!(output, vec![TerminalOutput::KittyKeyboardPush(27)]);
    }

    #[test]
    fn csi_gt_u_push_no_params_defaults_to_0() {
        let mut output = Vec::new();
        ansi_parser_inner_csi_finished_scorc(b">", &mut output);
        assert_eq!(output, vec![TerminalOutput::KittyKeyboardPush(0)]);
    }

    // ── Kitty pop ───────────────────────────────────────────────────

    #[test]
    fn csi_lt_u_pop_default_1() {
        let mut output = Vec::new();
        ansi_parser_inner_csi_finished_scorc(b"<", &mut output);
        assert_eq!(output, vec![TerminalOutput::KittyKeyboardPop(1)]);
    }

    #[test]
    fn csi_lt_u_pop_explicit_3() {
        let mut output = Vec::new();
        ansi_parser_inner_csi_finished_scorc(b"<3", &mut output);
        assert_eq!(output, vec![TerminalOutput::KittyKeyboardPop(3)]);
    }

    // ── Kitty set ───────────────────────────────────────────────────

    #[test]
    fn csi_eq_u_set_replace() {
        let mut output = Vec::new();
        ansi_parser_inner_csi_finished_scorc(b"=3", &mut output);
        assert_eq!(
            output,
            vec![TerminalOutput::KittyKeyboardSet { flags: 3, mode: 1 }]
        );
    }

    #[test]
    fn csi_eq_u_set_or() {
        let mut output = Vec::new();
        ansi_parser_inner_csi_finished_scorc(b"=3;2", &mut output);
        assert_eq!(
            output,
            vec![TerminalOutput::KittyKeyboardSet { flags: 3, mode: 2 }]
        );
    }

    #[test]
    fn csi_eq_u_set_clear() {
        let mut output = Vec::new();
        ansi_parser_inner_csi_finished_scorc(b"=3;3", &mut output);
        assert_eq!(
            output,
            vec![TerminalOutput::KittyKeyboardSet { flags: 3, mode: 3 }]
        );
    }

    // ── parse_decimal helper ────────────────────────────────────────

    #[test]
    fn parse_decimal_empty_is_none() {
        assert_eq!(parse_decimal(b""), None);
    }

    #[test]
    fn parse_decimal_simple_number() {
        assert_eq!(parse_decimal(b"42"), Some(42));
    }

    #[test]
    fn parse_decimal_stops_at_semicolon() {
        assert_eq!(parse_decimal(b"3;2"), Some(3));
    }

    // ── parse_two_params helper ─────────────────────────────────────

    #[test]
    fn parse_two_params_both_present() {
        assert_eq!(parse_two_params(b"3;2"), (3, 2));
    }

    #[test]
    fn parse_two_params_second_missing() {
        assert_eq!(parse_two_params(b"3"), (3, 0));
    }

    #[test]
    fn parse_two_params_empty() {
        assert_eq!(parse_two_params(b""), (0, 0));
    }
}
