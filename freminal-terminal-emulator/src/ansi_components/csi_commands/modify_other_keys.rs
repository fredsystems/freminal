// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Parser for xterm's `modifyOtherKeys` resource.
//!
//! The sequence `CSI > Ps ; Pv m` sets the `modifyOtherKeys` level:
//!
//! | Pv | Level                                                                                 |
//! |----|-----------------------------------------------------------------------------------------|
//! | 0  | Disable (same as `CSI > 4 m` with no value)                                          |
//! | 1  | Modified keys that would produce control chars use the extended `CSI 27;…~` encoding |
//! | 2  | ALL modified keys use the extended encoding (e.g. `CSI 27;…~`)                       |
//!
//! The `Ps` parameter selects *which* xterm resource to modify.  Only
//! `Ps = 4` (`modifyOtherKeys`) is supported here; other values are
//! silently ignored.
//!
//! Note: `CSI u` sequences are parsed separately as SCORC/Kitty (see `scorc`
//! handling) and are not used by `modifyOtherKeys` in this module.
//!
//! `CSI > 4 m` (no `Pv`, i.e. only one parameter) resets the resource to
//! its default value (level 0).
//!
//! Reference: <https://invisible-island.net/xterm/ctlseqs/ctlseqs.html>

use crate::ansi::ParserOutcome;
use freminal_common::buffer_states::terminal_output::TerminalOutput;

/// Parse `CSI > …  m` — xterm `modifyOtherKeys` and related key-modifier
/// resources.
///
/// `params` is the raw byte slice between `CSI` and the final `m`,
/// including the leading `>`.
///
/// Returns [`ParserOutcome::Finished`] after pushing the appropriate
/// [`TerminalOutput`] variant into `output`.
pub fn parse_modify_other_keys(params: &[u8], output: &mut Vec<TerminalOutput>) -> ParserOutcome {
    // Strip the leading `>`.
    let rest = params.strip_prefix(b">").unwrap_or(params);

    // Split on `;` and parse each segment as a usize.
    let parts: Vec<Option<usize>> = rest
        .split(|&b| b == b';')
        .map(|seg| {
            if seg.is_empty() {
                None
            } else {
                std::str::from_utf8(seg).ok().and_then(|s| s.parse().ok())
            }
        })
        .collect();

    let resource = parts.first().copied().flatten();

    match resource {
        // `Ps = 4` → `modifyOtherKeys`
        Some(4) => {
            let level = parts.get(1).copied().flatten().unwrap_or(0);
            // Clamp to valid range 0–2.
            let level = if level > 2 { 2 } else { level };
            #[allow(clippy::cast_possible_truncation)]
            output.push(TerminalOutput::ModifyOtherKeys(level as u8));
        }
        // Other resources (modifyCursorKeys=1, modifyFunctionKeys=2,
        // modifyKeyboard=8, etc.) — not yet supported; skip.
        _ => {
            output.push(TerminalOutput::Skipped);
        }
    }

    ParserOutcome::Finished
}

#[cfg(test)]
mod tests {
    use super::*;
    use freminal_common::buffer_states::terminal_output::TerminalOutput;

    #[test]
    fn test_set_level_0() {
        let mut output = Vec::new();
        let result = parse_modify_other_keys(b">4;0", &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        assert_eq!(output.len(), 1);
        assert_eq!(output[0], TerminalOutput::ModifyOtherKeys(0));
    }

    #[test]
    fn test_set_level_1() {
        let mut output = Vec::new();
        let result = parse_modify_other_keys(b">4;1", &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        assert_eq!(output.len(), 1);
        assert_eq!(output[0], TerminalOutput::ModifyOtherKeys(1));
    }

    #[test]
    fn test_set_level_2() {
        let mut output = Vec::new();
        let result = parse_modify_other_keys(b">4;2", &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        assert_eq!(output.len(), 1);
        assert_eq!(output[0], TerminalOutput::ModifyOtherKeys(2));
    }

    #[test]
    fn test_reset_no_value() {
        // CSI > 4 m — no Pv means reset to 0
        let mut output = Vec::new();
        let result = parse_modify_other_keys(b">4", &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        assert_eq!(output.len(), 1);
        assert_eq!(output[0], TerminalOutput::ModifyOtherKeys(0));
    }

    #[test]
    fn test_clamp_out_of_range() {
        // Level 5 should clamp to 2
        let mut output = Vec::new();
        let result = parse_modify_other_keys(b">4;5", &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        assert_eq!(output.len(), 1);
        assert_eq!(output[0], TerminalOutput::ModifyOtherKeys(2));
    }

    #[test]
    fn test_other_resource_skipped() {
        // Ps = 1 (modifyCursorKeys) — not supported, should skip
        let mut output = Vec::new();
        let result = parse_modify_other_keys(b">1;2", &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        assert_eq!(output.len(), 1);
        assert_eq!(output[0], TerminalOutput::Skipped);
    }

    #[test]
    fn test_empty_params_skipped() {
        let mut output = Vec::new();
        let result = parse_modify_other_keys(b">", &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        assert_eq!(output.len(), 1);
        // Empty resource param → None → falls through to Skipped
        assert_eq!(output[0], TerminalOutput::Skipped);
    }

    #[test]
    fn test_completely_empty_params() {
        // No `>` at all — strip_prefix returns the input unchanged, which is
        // empty; parsed resource is None → Skipped.
        let mut output = Vec::new();
        let result = parse_modify_other_keys(b"", &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        assert_eq!(output.len(), 1);
        assert_eq!(output[0], TerminalOutput::Skipped);
    }

    #[test]
    fn test_semicolon_empty_pv() {
        // `>4;` — semicolon present but empty Pv field → defaults to 0
        let mut output = Vec::new();
        let result = parse_modify_other_keys(b">4;", &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        assert_eq!(output.len(), 1);
        assert_eq!(output[0], TerminalOutput::ModifyOtherKeys(0));
    }

    #[test]
    fn test_non_numeric_pv() {
        // `>4;abc` — non-numeric Pv → parse fails → None → defaults to 0
        let mut output = Vec::new();
        let result = parse_modify_other_keys(b">4;abc", &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        assert_eq!(output.len(), 1);
        assert_eq!(output[0], TerminalOutput::ModifyOtherKeys(0));
    }

    #[test]
    fn test_level_3_clamps_to_2() {
        // Level 3 is out-of-range — should clamp to 2
        let mut output = Vec::new();
        let result = parse_modify_other_keys(b">4;3", &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        assert_eq!(output.len(), 1);
        assert_eq!(output[0], TerminalOutput::ModifyOtherKeys(2));
    }

    #[test]
    fn test_each_test_pushes_exactly_one_output() {
        // Verify that all variants push exactly one TerminalOutput
        let cases: &[&[u8]] = &[b">4;0", b">4;1", b">4;2", b">4", b">1;2", b">"];
        for params in cases {
            let mut output = Vec::new();
            let _ = parse_modify_other_keys(params, &mut output);
            assert_eq!(
                output.len(),
                1,
                "Expected exactly 1 output for params {:?}, got {}",
                std::str::from_utf8(params).unwrap_or("<invalid>"),
                output.len()
            );
        }
    }
}
