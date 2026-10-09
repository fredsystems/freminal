// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! The CSI routing table.
//!
//! [`dispatch_csi`] maps a [`CsiKey`] (prefix, intermediate, final byte) to the
//! handler that implements it. Routing is strict: the whole key must match a
//! route. A key with valid grammar but no route is *recognised and
//! unhandled*: it is logged and emits no output. It never falls through to the
//! handler of a sequence that merely shares its final byte.
//!
//! Handlers keep their existing signatures and still receive the parameter
//! string unchanged (private-marker byte included), so handlers that inspect
//! their own prefix keep working.
//!
//! ## Routed set
//!
//! | Prefix | Intermediate | Finals                                                              |
//! |--------|--------------|---------------------------------------------------------------------|
//! | none   | none         | ``A B C D E F G H I J K L M P S T X Z @ ` b d f g m h l n c r s u t x`` |
//! | none   | `$`          | `p` (DECRQM, ANSI)                                                  |
//! | none   | `!`          | `p` (DECSTR)                                                        |
//! | none   | SP           | `q` (DECSCUSR)                                                      |
//! | `?`    | none         | `h l n u`                                                           |
//! | `?`    | `$`          | `p` (DECRQM, private)                                               |
//! | `>`    | none         | `c q m u`                                                           |
//! | `<`    | none         | `u`                                                                 |
//! | `=`    | none         | `c u`                                                               |

use freminal_common::buffer_states::mode::SetMode;
use freminal_common::buffer_states::terminal_output::TerminalOutput;

use super::csi_commands::{
    cbt::ansi_parser_inner_csi_finished_cbt, cha::ansi_parser_inner_csi_finished_cha,
    cht::ansi_parser_inner_csi_finished_cht, cnl::ansi_parser_inner_csi_finished_cnl,
    cpl::ansi_parser_inner_csi_finished_cpl, cub::ansi_parser_inner_csi_finished_cub,
    cud::ansi_parser_inner_csi_finished_cud, cuf::ansi_parser_inner_csi_finished_cuf,
    cup::ansi_parser_inner_csi_finished_cup, cuu::ansi_parser_inner_csi_finished_cuu,
    da::ansi_parser_inner_csi_finished_da, dch::ansi_parser_inner_csi_finished_dch,
    dec_modes::push_split_mode_params, decreqtparm::ansi_parser_inner_csi_finished_decreqtparm,
    decrqm::ansi_parser_inner_csi_finished_decrqm,
    decscusr::ansi_parser_inner_csi_finished_decscusr,
    decslpp::ansi_parser_inner_csi_finished_decslpp,
    decslrm::ansi_parser_inner_csi_finished_decslrm,
    decstbm::ansi_parser_inner_csi_finished_decstbm, decstr::ansi_parser_inner_csi_finished_decstr,
    dl::ansi_parser_inner_csi_finished_dl, dsr::ansi_parser_inner_csi_finished_dsr,
    ech::ansi_parser_inner_csi_finished_ech, ed::ansi_parser_inner_csi_finished_ed,
    el::ansi_parser_inner_csi_finished_el, ich::ansi_parser_inner_csi_finished_ich,
    il::ansi_parser_inner_csi_finished_il,
    modify_other_keys::ansi_parser_inner_csi_finished_modify_other_keys,
    rep::ansi_parser_inner_csi_finished_rep, scorc::ansi_parser_inner_csi_finished_scorc,
    sd::ansi_parser_inner_csi_finished_sd, sgr::ansi_parser_inner_csi_finished_sgr,
    su::ansi_parser_inner_csi_finished_su, tbc::ansi_parser_inner_csi_finished_tbc,
    vpa::ansi_parser_inner_csi_finished_vpa, xtversion::ansi_parser_inner_csi_finished_xtversion,
};
use super::csi_key::{CsiIntermediate, CsiKey, CsiPrefix};
use crate::ansi::ParserOutcome;

/// Log a syntactically valid CSI sequence that has no route.
///
/// `raw` renders the complete raw sequence; it is only called if the warning
/// is actually emitted.
pub fn warn_unhandled(raw: &dyn Fn() -> String) {
    tracing::warn!(
        "Unhandled CSI final byte (valid grammar, no dispatch): {}",
        raw()
    );
}

/// Route a classified CSI sequence to its handler.
///
/// Selects a group on `(prefix, intermediate)` first, then the handler on the
/// final byte within that group. A key no group routes is logged via
/// [`warn_unhandled`], emits nothing, and yields [`ParserOutcome::Finished`].
/// [`TerminalOutput::Invalid`] is reserved for routed keys whose handler
/// rejects their parameters.
pub fn dispatch_csi(
    key: CsiKey,
    params: &[u8],
    intermediates: &[u8],
    raw: &dyn Fn() -> String,
    output: &mut Vec<TerminalOutput>,
) -> ParserOutcome {
    let routed = match (key.prefix, key.intermediate) {
        (CsiPrefix::None, CsiIntermediate::None) => {
            plain(key.final_byte, params, intermediates, output)
        }
        (CsiPrefix::Question, CsiIntermediate::None) => question(key.final_byte, params, output),
        (CsiPrefix::Greater, CsiIntermediate::None) => greater(key.final_byte, params, output),
        (CsiPrefix::Less, CsiIntermediate::None) => less(key.final_byte, params, output),
        (CsiPrefix::Equals, CsiIntermediate::None) => {
            equals(key.final_byte, params, intermediates, output)
        }
        (CsiPrefix::None, CsiIntermediate::Space) => space(key.final_byte, params, output),
        (CsiPrefix::None, CsiIntermediate::Bang) => bang(key.final_byte, params, output),
        (CsiPrefix::None, CsiIntermediate::Dollar) => {
            dollar(key.final_byte, params, intermediates, output)
        }
        (CsiPrefix::Question, CsiIntermediate::Dollar) => {
            question_dollar(key.final_byte, params, intermediates, output)
        }
        _ => None,
    };

    routed.unwrap_or_else(|| {
        warn_unhandled(raw);
        ParserOutcome::Finished
    })
}

/// `CSI Ps ... <final>`: no prefix, no intermediate. `None` means no route.
fn plain(
    final_byte: u8,
    params: &[u8],
    intermediates: &[u8],
    output: &mut Vec<TerminalOutput>,
) -> Option<ParserOutcome> {
    let outcome = match final_byte {
        b'A' => ansi_parser_inner_csi_finished_cuu(params, output),
        b'B' => ansi_parser_inner_csi_finished_cud(params, output),
        b'C' => ansi_parser_inner_csi_finished_cuf(params, output),
        b'D' => ansi_parser_inner_csi_finished_cub(params, output),
        b'E' => ansi_parser_inner_csi_finished_cnl(params, output),
        b'F' => ansi_parser_inner_csi_finished_cpl(params, output),
        // CHA (CSI G) and HPA (CSI `): cursor horizontal absolute.
        b'G' | b'`' => ansi_parser_inner_csi_finished_cha(params, output),
        // CUP (CSI H) and HVP (CSI f).
        b'H' | b'f' => ansi_parser_inner_csi_finished_cup(params, output),
        b'I' => ansi_parser_inner_csi_finished_cht(params, output),
        b'J' => ansi_parser_inner_csi_finished_ed(params, output),
        b'K' => ansi_parser_inner_csi_finished_el(params, output),
        b'L' => ansi_parser_inner_csi_finished_il(params, output),
        b'M' => ansi_parser_inner_csi_finished_dl(params, output),
        b'P' => ansi_parser_inner_csi_finished_dch(params, output),
        b'S' => ansi_parser_inner_csi_finished_su(params, output),
        b'T' => ansi_parser_inner_csi_finished_sd(params, output),
        b'X' => ansi_parser_inner_csi_finished_ech(params, output),
        b'Z' => ansi_parser_inner_csi_finished_cbt(params, output),
        b'@' => ansi_parser_inner_csi_finished_ich(params, output),
        b'b' => ansi_parser_inner_csi_finished_rep(params, output),
        b'd' => ansi_parser_inner_csi_finished_vpa(params, output),
        b'g' => ansi_parser_inner_csi_finished_tbc(params, output),
        b'm' => ansi_parser_inner_csi_finished_sgr(params, output),
        _ => return plain_modes_and_reports(final_byte, params, intermediates, output),
    };
    Some(outcome)
}

/// The remainder of the plain group: mode set/reset, reports, margins, window
/// operations. Split from [`plain`] only to keep each function readable.
fn plain_modes_and_reports(
    final_byte: u8,
    params: &[u8],
    intermediates: &[u8],
    output: &mut Vec<TerminalOutput>,
) -> Option<ParserOutcome> {
    let outcome = match final_byte {
        b'h' => {
            push_split_mode_params(params, SetMode::DecSet, output);
            ParserOutcome::Finished
        }
        b'l' => {
            push_split_mode_params(params, SetMode::DecRst, output);
            ParserOutcome::Finished
        }
        b'n' => ansi_parser_inner_csi_finished_dsr(params, output),
        b'c' => ansi_parser_inner_csi_finished_da(params, intermediates, output),
        b'r' => ansi_parser_inner_csi_finished_decstbm(params, output),
        // With params this is DECSLRM (set left/right margins); empty it is
        // SCOSC (save cursor). The handler (`process_outputs`) ignores
        // SetLeftAndRightMargins when DECLRMM is not active, so the parse is
        // always safe.
        b's' if params.is_empty() => {
            output.push(TerminalOutput::SaveCursor);
            ParserOutcome::Finished
        }
        b's' => ansi_parser_inner_csi_finished_decslrm(params, output),
        b'u' => {
            ansi_parser_inner_csi_finished_scorc(params, output);
            ParserOutcome::Finished
        }
        b't' => ansi_parser_inner_csi_finished_decslpp(params, output),
        b'x' => {
            // DECREQTPARM: the handler emits RequestTerminalParameters or
            // Invalid for bad parameters.
            ansi_parser_inner_csi_finished_decreqtparm(params, output);
            ParserOutcome::Finished
        }
        _ => return None,
    };
    Some(outcome)
}

/// `CSI ? Ps ... <final>`.
fn question(
    final_byte: u8,
    params: &[u8],
    output: &mut Vec<TerminalOutput>,
) -> Option<ParserOutcome> {
    let outcome = match final_byte {
        // DECSET / DECRST.
        b'h' => {
            push_split_mode_params(params, SetMode::DecSet, output);
            ParserOutcome::Finished
        }
        b'l' => {
            push_split_mode_params(params, SetMode::DecRst, output);
            ParserOutcome::Finished
        }
        // DEC private DSR.
        b'n' => ansi_parser_inner_csi_finished_dsr(params, output),
        // Kitty keyboard protocol: query.
        b'u' => {
            ansi_parser_inner_csi_finished_scorc(params, output);
            ParserOutcome::Finished
        }
        _ => return None,
    };
    Some(outcome)
}

/// `CSI > Ps ... <final>`.
fn greater(
    final_byte: u8,
    params: &[u8],
    output: &mut Vec<TerminalOutput>,
) -> Option<ParserOutcome> {
    let outcome = match final_byte {
        // DA2: the handler reads its own `>` prefix from `params`.
        b'c' => ansi_parser_inner_csi_finished_da(params, &[], output),
        // XTVERSION.
        b'q' => ansi_parser_inner_csi_finished_xtversion(params, output),
        // XTMODKEYS (xterm modifyOtherKeys).
        b'm' => ansi_parser_inner_csi_finished_modify_other_keys(params, output),
        // Kitty keyboard protocol: push.
        b'u' => {
            ansi_parser_inner_csi_finished_scorc(params, output);
            ParserOutcome::Finished
        }
        _ => return None,
    };
    Some(outcome)
}

/// `CSI < Ps ... <final>`.
fn less(final_byte: u8, params: &[u8], output: &mut Vec<TerminalOutput>) -> Option<ParserOutcome> {
    match final_byte {
        // Kitty keyboard protocol: pop.
        b'u' => {
            ansi_parser_inner_csi_finished_scorc(params, output);
            Some(ParserOutcome::Finished)
        }
        _ => None,
    }
}

/// `CSI = Ps ... <final>`.
fn equals(
    final_byte: u8,
    params: &[u8],
    intermediates: &[u8],
    output: &mut Vec<TerminalOutput>,
) -> Option<ParserOutcome> {
    let outcome = match final_byte {
        // DA3: the handler reads its own `=` prefix from `params`.
        b'c' => ansi_parser_inner_csi_finished_da(params, intermediates, output),
        // Kitty keyboard protocol: set.
        b'u' => {
            ansi_parser_inner_csi_finished_scorc(params, output);
            ParserOutcome::Finished
        }
        _ => return None,
    };
    Some(outcome)
}

/// `CSI Ps SP <final>`.
fn space(final_byte: u8, params: &[u8], output: &mut Vec<TerminalOutput>) -> Option<ParserOutcome> {
    match final_byte {
        // DECSCUSR.
        b'q' => Some(ansi_parser_inner_csi_finished_decscusr(params, output)),
        _ => None,
    }
}

/// `CSI Ps ! <final>`.
fn bang(final_byte: u8, params: &[u8], output: &mut Vec<TerminalOutput>) -> Option<ParserOutcome> {
    match final_byte {
        // DECSTR: exactly one `!` intermediate and no parameters.
        b'p' => Some(ansi_parser_inner_csi_finished_decstr(params, output)),
        _ => None,
    }
}

/// `CSI Ps $ <final>`.
fn dollar(
    final_byte: u8,
    params: &[u8],
    intermediates: &[u8],
    output: &mut Vec<TerminalOutput>,
) -> Option<ParserOutcome> {
    match final_byte {
        // DECRQM (ANSI mode).
        b'p' => Some(ansi_parser_inner_csi_finished_decrqm(
            params,
            intermediates,
            final_byte,
            output,
        )),
        _ => None,
    }
}

/// `CSI ? Ps $ <final>`.
fn question_dollar(
    final_byte: u8,
    params: &[u8],
    intermediates: &[u8],
    output: &mut Vec<TerminalOutput>,
) -> Option<ParserOutcome> {
    match final_byte {
        // DECRQM (DEC private mode).
        b'p' => Some(ansi_parser_inner_csi_finished_decrqm(
            params,
            intermediates,
            final_byte,
            output,
        )),
        _ => None,
    }
}
