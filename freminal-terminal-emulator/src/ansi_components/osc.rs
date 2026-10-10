// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::ansi::{ParserOutcome, PrevByte, parse_param_as};
use crate::ansi_components::tracer::{
    escape_sequence_for_log_bounded, lossy_sequence_for_log_bounded,
};
use crate::error::AnsiParseError;
use freminal_common::buffer_states::ftcs::{is_known_ftcs_marker, parse_ftcs_params};
use freminal_common::buffer_states::osc::{
    AnsiOscInternalType, AnsiOscToken, AnsiOscType, OscTarget, UrlResponse,
};
use freminal_common::buffer_states::pointer_shape::PointerShape;
use freminal_common::buffer_states::terminal_output::TerminalOutput;

use super::osc_clipboard::handle_osc_clipboard;
use super::osc_iterm2::handle_osc_iterm2;
use super::osc_notify::{handle_osc_notify_9, handle_osc_notify_99, handle_osc_notify_777};
use super::osc_palette::{handle_osc_palette_color, handle_osc_reset_palette};
use super::osc_shell_info::handle_osc_shell_info;

/// Default maximum size of an OSC sequence, in bytes.
///
/// The cap applies to the accumulated OSC body **before its terminator**: the
/// OSC number, its `;` separator and the payload. A sequence whose body,
/// excluding the closing BEL or `ESC \`, is `MAX_OSC_BYTES` or fewer is
/// dispatched; one more byte and it is dropped. (A trailing ESC is not counted
/// until the byte after it shows it was not the start of the terminator.)
///
/// kitty's own cap is 256 KiB (`MAX_ESCAPE_CODE_LENGTH` in `vt-parser.c`).
/// Freminal is deliberately more generous at 1 MiB, and raises it to
/// [`MAX_OSC_LARGE_BYTES`] for the OSC numbers that legitimately carry large
/// payloads.
pub const MAX_OSC_BYTES: usize = 1024 * 1024;

/// Maximum size of an OSC sequence whose number is 52 (clipboard) or 1337
/// (iTerm2 inline images and files), in bytes. Counted exactly like
/// [`MAX_OSC_BYTES`].
///
/// A single clipboard payload or inline image is legitimately far larger than
/// 1 MiB. This is far above kitty's 256 KiB cap because Freminal implements the
/// iTerm2 image protocol, which kitty does not.
pub const MAX_OSC_LARGE_BYTES: usize = 64 * 1024 * 1024;

/// Which byte cap applies to the OSC sequence being accumulated.
#[derive(Eq, PartialEq, Debug, Clone, Copy)]
enum OscLimit {
    /// [`MAX_OSC_BYTES`].
    Default,
    /// [`MAX_OSC_LARGE_BYTES`]: the OSC number is 52 or 1337.
    Large,
}

impl OscLimit {
    const fn max_bytes(self) -> usize {
        match self {
            Self::Default => MAX_OSC_BYTES,
            Self::Large => MAX_OSC_LARGE_BYTES,
        }
    }

    /// The limit that applies to a body that has just reached the default cap.
    /// Only here is the OSC number examined, so the common path stays a single
    /// length comparison.
    fn for_body(params: &[u8]) -> Self {
        // Classify the number exactly as `ansiparser_inner_osc` does before
        // dispatch (`parse_param_as::<AnsiOscToken>`), so a body is never
        // dispatched as OSC 52 / 1337 under the default cap (`052`, `+52`).
        let number = params.split(|b| *b == b';').next().unwrap_or_default();
        match parse_param_as::<AnsiOscToken>(number) {
            Ok(Some(AnsiOscToken::OscValue(52 | 1337))) => Self::Large,
            _ => Self::Default,
        }
    }
}

#[derive(Eq, PartialEq, Debug)]
pub(crate) enum AnsiOscParserState {
    Params,
    Finished,
    Invalid,
    InvalidFinished,
    /// The body went over its cap; the buffer was released and only what
    /// terminator detection needs is kept.
    Overflow {
        total: usize,
        prev: PrevByte,
    },
    /// An overflowed sequence reached its terminator. It produces no output.
    OverflowFinished,
}

#[derive(Eq, PartialEq, Debug)]
pub struct AnsiOscParser {
    pub(crate) state: AnsiOscParserState,
    pub(crate) params: Vec<u8>,
    pub(crate) intermediates: Vec<u8>,
    limit: OscLimit,
}

// OSC Sequence looks like this:
// 1b]11;?1b\

impl Default for AnsiOscParser {
    fn default() -> Self {
        Self::new()
    }
}

impl AnsiOscParser {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            state: AnsiOscParserState::Params,
            params: Vec::new(),
            intermediates: Vec::new(),
            limit: OscLimit::Default,
        }
    }

    /// Expose the current OSC body for testing and diagnostics.
    ///
    /// Rendered from `params` (every body byte accepted so far, with
    /// the terminator removed once the sequence finishes), lossily decoded and
    /// bounded to
    /// [`crate::ansi_components::tracer::LOG_SEQUENCE_MAX_BYTES`].
    #[must_use]
    pub fn trace_str(&self) -> String {
        let trace = lossy_sequence_for_log_bounded(&self.params);
        trace!("current buffer trace: {}", trace);
        trace
    }

    /// Push a byte into the parser
    ///
    /// # Errors
    /// Will return an error if the parser is in the `Finished` or `InvalidFinished` state
    #[tracing::instrument(level = "trace", skip_all)]
    pub fn push(&mut self, b: u8) -> ParserOutcome {
        if let AnsiOscParserState::Finished
        | AnsiOscParserState::InvalidFinished
        | AnsiOscParserState::OverflowFinished = &self.state
        {
            return ParserOutcome::Invalid("Parsed Pushed To Once Finished".to_string());
        }

        match self.state {
            AnsiOscParserState::Params => {
                if is_valid_osc_param(b) {
                    self.params.push(b);
                } else {
                    debug!("Invalid OSC param: {:x}", b);
                    {
                        self.state = AnsiOscParserState::Invalid;

                        self.params.clear();
                        self.intermediates.clear();

                        return ParserOutcome::Invalid("Invalid OSC param encountered".to_string());
                    };
                }

                if is_osc_terminator(&self.params) {
                    self.state = AnsiOscParserState::Finished;

                    // Remove exactly the terminator: one byte for BEL, two for
                    // `ESC \`. The two suffixes are disjoint (the last byte is
                    // 0x07 or 0x5c), and any other trailing `\`, BEL-adjacent
                    // ESC, etc. belongs to the body and must be preserved.
                    let terminator_len = if self.params.ends_with(&[0x1b, 0x5c]) {
                        2
                    } else {
                        1
                    };
                    self.params
                        .truncate(self.params.len().saturating_sub(terminator_len));

                    // A body ending in ESC held that ESC uncounted while it
                    // might have been the start of `ESC \`; if BEL then made
                    // it body, the finished body can be one byte over the cap.
                    if self.finished_body_exceeds_cap() {
                        return self.drop_finished_over_cap();
                    }

                    return ParserOutcome::Finished;
                }

                if self.params.len() > self.limit.max_bytes() {
                    self.overflow_if_over_cap();
                }

                ParserOutcome::Continue
            }
            AnsiOscParserState::Overflow { .. } => self.push_overflowed(b),
            AnsiOscParserState::Finished
            | AnsiOscParserState::InvalidFinished
            | AnsiOscParserState::OverflowFinished => {
                // Guarded by the early-return at the top of `push`, but surface
                // explicitly as an invalid outcome rather than panicking if the
                // invariant ever breaks.
                ParserOutcome::Invalid("OSC parser received byte after termination".to_string())
            }
            AnsiOscParserState::Invalid => {
                if is_osc_terminator(&self.params) {
                    self.state = AnsiOscParserState::InvalidFinished;
                }

                ParserOutcome::Invalid("Invalid OSC sequence terminated".to_string())
            }
        }
    }

    /// Whether a just-terminated body is longer than its cap. Only the BEL
    /// terminator after an uncounted trailing ESC can make this true.
    fn finished_body_exceeds_cap(&mut self) -> bool {
        if self.params.len() <= self.limit.max_bytes() {
            return false;
        }
        if self.limit == OscLimit::Default {
            self.limit = OscLimit::for_body(&self.params);
        }
        self.params.len() > self.limit.max_bytes()
    }

    /// Drop a terminated body that is over its cap, exactly as an overflowed
    /// sequence is dropped: no output, one payload-free warn, buffer released.
    fn drop_finished_over_cap(&mut self) -> ParserOutcome {
        // The stored length plus the one-byte BEL terminator.
        let total = self.params.len().saturating_add(1);
        tracing::warn!(
            "OSC sequence over the {}-byte cap dropped: total length {total} bytes",
            self.limit.max_bytes()
        );
        self.params = Vec::new();
        self.intermediates = Vec::new();
        self.state = AnsiOscParserState::OverflowFinished;
        ParserOutcome::Finished
    }

    /// Called only once the accumulated body is longer than the current cap and
    /// is not terminated. A trailing ESC may still be the start of the
    /// terminator, so it is not counted. If the rest is still over the cap, the
    /// body's OSC number picks the cap (52 and 1337 get the large one) and, if
    /// it is still exceeded, the buffer is dropped.
    fn overflow_if_over_cap(&mut self) {
        let last = self.params.last().copied().unwrap_or_default();
        let uncounted = usize::from(last == 0x1b);
        if self.params.len().saturating_sub(uncounted) <= self.limit.max_bytes() {
            return;
        }

        if self.limit == OscLimit::Default {
            self.limit = OscLimit::for_body(&self.params);
            if self.params.len().saturating_sub(uncounted) <= self.limit.max_bytes() {
                return;
            }
        }

        self.state = AnsiOscParserState::Overflow {
            total: self.params.len(),
            prev: PrevByte::of(last),
        };
        self.params = Vec::new();
        self.intermediates = Vec::new();
    }

    /// Consume one byte of an overflowed sequence. Bytes that are invalid in an
    /// OSC body still invalidate it, exactly as when within the cap; BEL and
    /// `ESC \` end it with no output.
    fn push_overflowed(&mut self, b: u8) -> ParserOutcome {
        let AnsiOscParserState::Overflow { total, prev } = &mut self.state else {
            return ParserOutcome::Continue;
        };

        if !is_valid_osc_param(b) {
            debug!("Invalid OSC param: {:x}", b);
            self.state = AnsiOscParserState::Invalid;
            return ParserOutcome::Invalid("Invalid OSC param encountered".to_string());
        }

        *total = total.saturating_add(1);
        let terminated = b == 0x07 || (*prev == PrevByte::Esc && b == 0x5c);
        *prev = PrevByte::of(b);

        if terminated {
            tracing::warn!(
                "OSC sequence over the {}-byte cap dropped: total length {total} bytes",
                self.limit.max_bytes()
            );
            self.state = AnsiOscParserState::OverflowFinished;
            return ParserOutcome::Finished;
        }

        ParserOutcome::Continue
    }

    /// Parse the OSC sequence
    ///
    /// # Errors
    /// Will return an error if the parser is in the `Finished` or `InvalidFinished` state
    #[tracing::instrument(level = "trace", skip_all)]
    pub fn ansiparser_inner_osc(
        &mut self,
        b: u8,
        output: &mut Vec<TerminalOutput>,
    ) -> ParserOutcome {
        let push_result = self.push(b);

        // if we failed the push result with ParserOutcome::Invalid, return push_result
        if let ParserOutcome::Invalid(_) = push_result {
            return push_result;
        }

        match self.state {
            AnsiOscParserState::Finished => {
                // The OSC number is everything before the first `;`; the whole
                // body (number included, terminator already stripped) is handed
                // to the dispatcher so each target can parse what it needs.
                let number_bytes = self.params.split(|b| *b == b';').next().unwrap_or_default();

                let Ok(Some(type_number)) = parse_param_as::<AnsiOscToken>(number_bytes) else {
                    return invalid_osc_params(&self.params, output);
                };

                let osc_target = OscTarget::from(&type_number);

                dispatch_osc_target(&osc_target, &self.params, output)
            }
            AnsiOscParserState::Invalid => ParserOutcome::Invalid("Invalid OSC State".to_string()),
            // An overflowed sequence emits nothing; report exactly what `push`
            // decided (`Continue`, or `Finished` on its terminator).
            AnsiOscParserState::Overflow { .. } | AnsiOscParserState::OverflowFinished => {
                push_result
            }
            _ => ParserOutcome::Continue,
        }
    }
}

/// Extract the pointer-shape name from OSC 22 parameters and emit the
/// corresponding terminal output.
///
/// The second parameter contains the xcursor/CSS name string. An empty or
/// absent token resets the pointer shape to the default.
fn handle_osc_pointer_shape(params: &[Option<AnsiOscToken>], output: &mut Vec<TerminalOutput>) {
    let shape_name = params
        .get(1)
        .and_then(|t| {
            if let Some(AnsiOscToken::String(s)) = t {
                Some(s.as_str())
            } else {
                None
            }
        })
        .unwrap_or("");
    output.push(TerminalOutput::OscResponse(AnsiOscType::SetPointerShape(
        PointerShape::from(shape_name),
    )));
}

/// Push `TerminalOutput::Invalid` and build the matching `ParserOutcome` for
/// an OSC body whose number or parameters could not be parsed.
fn invalid_osc_params(raw_params: &[u8], output: &mut Vec<TerminalOutput>) -> ParserOutcome {
    output.push(TerminalOutput::Invalid);
    ParserOutcome::Invalid(format!(
        "Invalid OSC params: recent='{}'",
        lossy_sequence_for_log_bounded(raw_params)
    ))
}

/// Tokenise the whole OSC body (so token 0 is the OSC number) and run `handler`
/// on the tokens.
///
/// Used by every target that works on tokens rather than the raw body. A
/// tokenisation failure (a non-UTF-8 segment) yields `TerminalOutput::Invalid`
/// and `ParserOutcome::Invalid`, exactly as before raw-body dispatch.
fn with_osc_tokens(
    raw_params: &[u8],
    output: &mut Vec<TerminalOutput>,
    handler: impl FnOnce(Vec<Option<AnsiOscToken>>, &mut Vec<TerminalOutput>),
) -> ParserOutcome {
    match split_params_into_semicolon_delimited_tokens(raw_params) {
        Ok(params) => {
            handler(params, output);
            ParserOutcome::Finished
        }
        Err(_) => invalid_osc_params(raw_params, output),
    }
}

/// The text after the first `;` of an OSC body, as UTF-8.
///
/// Used by the title (OSC 0/1/2) and remote-host (OSC 7) targets, whose value
/// is the whole remainder of the body, `;` included. An empty remainder is an
/// empty string. A body with no `;`, or a remainder that is not valid UTF-8,
/// yields `None` (logged at debug with the body length only; never the
/// payload) and the caller emits nothing.
fn osc_text_remainder(raw_params: &[u8]) -> Option<&str> {
    let Some(sep) = raw_params.iter().position(|b| *b == b';') else {
        tracing::debug!(
            "OSC text sequence without a value dropped: body length {}",
            raw_params.len()
        );
        return None;
    };
    let value = raw_params.get(sep + 1..).unwrap_or_default();
    std::str::from_utf8(value)
        .inspect_err(|_| {
            tracing::debug!(
                "OSC text sequence with a non-UTF-8 value dropped: body length {}",
                raw_params.len()
            );
        })
        .ok()
}

/// Push the text remainder of `raw_params` (see [`osc_text_remainder`]) as the
/// response built by `make`, or push nothing if there is none.
fn push_osc_text(
    raw_params: &[u8],
    output: &mut Vec<TerminalOutput>,
    make: fn(String) -> AnsiOscType,
) -> ParserOutcome {
    if let Some(text) = osc_text_remainder(raw_params) {
        output.push(TerminalOutput::OscResponse(make(text.to_owned())));
    }
    ParserOutcome::Finished
}

/// Handle OSC 8 (hyperlink) from the raw body.
///
/// The body after the OSC number is `params ; URI`; see
/// [`UrlResponse::from_osc8_body`]. A malformed hyperlink produces no output.
fn handle_osc_url(raw_params: &[u8], output: &mut Vec<TerminalOutput>) -> ParserOutcome {
    let body = raw_params
        .iter()
        .position(|b| *b == b';')
        .and_then(|sep| raw_params.get(sep + 1..))
        .unwrap_or_default();
    if let Some(response) = UrlResponse::from_osc8_body(body) {
        output.push(TerminalOutput::OscResponse(AnsiOscType::Url(response)));
    } else {
        tracing::debug!(
            "OSC 8 with a non-UTF-8 URI dropped: body length {}",
            raw_params.len()
        );
    }
    ParserOutcome::Finished
}

/// Handle OSC 133 (FTCS) from the raw body.
///
/// The FTCS items are everything after the first `;` of `raw_params` (the body
/// is empty when there is no `;`); see
/// [`parse_ftcs_params`](freminal_common::buffer_states::ftcs::parse_ftcs_params).
fn handle_osc_ftcs(raw_params: &[u8], output: &mut Vec<TerminalOutput>) {
    let body = raw_params
        .iter()
        .position(|b| *b == b';')
        .and_then(|sep| raw_params.get(sep + 1..))
        .unwrap_or_default();

    if let Some(marker) = parse_ftcs_params(body) {
        output.push(TerminalOutput::OscResponse(AnsiOscType::Ftcs(marker)));
    } else if is_known_ftcs_marker(body) {
        // Known FTCS marker (A/B/C/D/P) that we recognise but did not
        // act on — a foreign emitter sent it without the `freminal=1`
        // tag (e.g. Apple Terminal's `133;A;cl=m;aid=$$`, or
        // Starship/oh-my-zsh/VTE). Freminal uses its own
        // `freminal=1`-tagged FTCS variant (see shell-integration/) and
        // deliberately ignores foreign duplicates to avoid double
        // command blocks. This is expected and benign, so it is
        // silently dropped WITHOUT a log by design.
    } else {
        // Unknown or malformed OSC 133: the marker letter is not one we
        // recognise (e.g. `Z`, or a future FTCS addition like `E`), or
        // the parameter list was empty. Unlike a known-foreign marker,
        // this is a genuine gap — the OSC 133 surface may have grown a
        // variant we do not handle, or a program sent something
        // malformed. Log it at warn with the full raw sequence so the
        // unhandled surface can be audited.
        tracing::warn!("OSC 133: unrecognised or malformed FTCS marker (not a known A/B/C/D/P)");
        tracing::debug!(
            "OSC 133: unrecognised or malformed FTCS marker; raw sequence: \"{}\"",
            escape_sequence_for_log_bounded(raw_params)
        );
    }
}

/// Dispatch a finished OSC body to the handler for `osc_target`.
///
/// `raw_params` is the whole OSC body (`number[;rest]`, terminator stripped).
/// Raw-body targets consume it directly; token-based targets tokenise it
/// themselves via [`with_osc_tokens`].
fn dispatch_osc_target(
    osc_target: &OscTarget,
    raw_params: &[u8],
    output: &mut Vec<TerminalOutput>,
) -> ParserOutcome {
    match *osc_target {
        OscTarget::Background => with_osc_tokens(raw_params, output, |params, out| {
            out.push(TerminalOutput::OscResponse(
                AnsiOscType::RequestColorQueryBackground(AnsiOscInternalType::from(&params)),
            ));
        }),
        OscTarget::Foreground => with_osc_tokens(raw_params, output, |params, out| {
            out.push(TerminalOutput::OscResponse(
                AnsiOscType::RequestColorQueryForeground(AnsiOscInternalType::from(&params)),
            ));
        }),
        OscTarget::CursorColor => with_osc_tokens(raw_params, output, |params, out| {
            out.push(TerminalOutput::OscResponse(
                AnsiOscType::RequestColorQueryCursor(AnsiOscInternalType::from(&params)),
            ));
        }),
        OscTarget::TitleBar | OscTarget::IconName => {
            push_osc_text(raw_params, output, AnsiOscType::SetTitleBar)
        }
        OscTarget::Ftcs => {
            handle_osc_ftcs(raw_params, output);
            ParserOutcome::Finished
        }
        OscTarget::Clipboard => with_osc_tokens(raw_params, output, |params, out| {
            handle_osc_clipboard(&params, raw_params, out);
        }),
        OscTarget::PaletteColor => with_osc_tokens(raw_params, output, |params, out| {
            handle_osc_palette_color(&params, raw_params, out);
        }),
        OscTarget::ResetPaletteColor => with_osc_tokens(raw_params, output, |params, out| {
            handle_osc_reset_palette(&params, out);
        }),
        OscTarget::RemoteHost => push_osc_text(raw_params, output, AnsiOscType::RemoteHost),
        OscTarget::Url => handle_osc_url(raw_params, output),
        // OSC 22 — set the pointer (mouse cursor) shape.
        OscTarget::PointerShape => with_osc_tokens(raw_params, output, |params, out| {
            handle_osc_pointer_shape(&params, out);
        }),
        OscTarget::ResetCursorColor => {
            output.push(TerminalOutput::OscResponse(AnsiOscType::ResetCursorColor));
            ParserOutcome::Finished
        }
        OscTarget::ResetForeground => {
            output.push(TerminalOutput::OscResponse(
                AnsiOscType::ResetForegroundColor,
            ));
            ParserOutcome::Finished
        }
        OscTarget::ResetBackground => {
            output.push(TerminalOutput::OscResponse(
                AnsiOscType::ResetBackgroundColor,
            ));
            ParserOutcome::Finished
        }
        OscTarget::ITerm2 => {
            handle_osc_iterm2(raw_params, output);
            ParserOutcome::Finished
        }
        OscTarget::ShellInfo => {
            handle_osc_shell_info(raw_params, output);
            ParserOutcome::Finished
        }
        // OSC 9 / OSC 777 — desktop notifications (Task 76).  Parsed from the
        // raw bytes so notification bodies containing `;` survive intact.
        OscTarget::Notify9 => {
            handle_osc_notify_9(raw_params, output);
            ParserOutcome::Finished
        }
        OscTarget::Notify777 => {
            handle_osc_notify_777(raw_params, output);
            ParserOutcome::Finished
        }
        OscTarget::Notify99 => {
            handle_osc_notify_99(raw_params, output);
            ParserOutcome::Finished
        }
        // Known-but-unimplemented OSC targets.  These are recognised
        // sequences sent by common programs (vim/neovim, zsh, tmux) that
        // Freminal cannot meaningfully act on (X11 mouse colors, Tektronix
        // graphics, color-scheme notifications).  Logged at warn with the
        // full raw sequence so the unhandled surface can be audited.
        OscTarget::MouseForeground
        | OscTarget::MouseBackground
        | OscTarget::TekForeground
        | OscTarget::TekBackground
        | OscTarget::HighlightBackground
        | OscTarget::HighlightForeground
        | OscTarget::ColorSchemeNotification => {
            warn_unimplemented_osc(osc_target, raw_params);
            ParserOutcome::Finished
        }
        OscTarget::Unknown => {
            warn_unknown_osc(raw_params);
            ParserOutcome::Finished
        }
    }
}

/// Warn about a recognised-but-unimplemented OSC target, with the full raw
/// sequence so the unhandled surface can be audited.
fn warn_unimplemented_osc(osc_target: &OscTarget, raw_params: &[u8]) {
    tracing::warn!("Recognised but unimplemented OSC (silently consumed): target={osc_target:?}");
    tracing::debug!(
        "Recognised but unimplemented OSC; raw sequence: \"{}\"",
        escape_sequence_for_log_bounded(raw_params)
    );
}

/// Unknown OSC sequences are silently consumed (like xterm/VTE). The warn
/// carries only the OSC number, and only when the text before the first `;`
/// parses as a number (an unmapped OSC number); a non-numeric first token is
/// attacker text and is reported as "non-numeric". The raw sequence goes to an
/// adjacent bounded `debug!` for auditing.
fn warn_unknown_osc(raw_params: &[u8]) {
    let number_bytes = raw_params.split(|b| *b == b';').next().unwrap_or_default();
    let number = std::str::from_utf8(number_bytes)
        .ok()
        .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|s| s.parse::<u32>().ok());
    if let Some(n) = number {
        tracing::warn!("Unknown OSC Target (silently consumed): OSC {n}");
    } else {
        tracing::warn!("Unknown OSC Target (silently consumed): non-numeric OSC number");
    }
    tracing::debug!(
        "Unknown OSC; raw sequence: \"{}\"",
        escape_sequence_for_log_bounded(raw_params)
    );
}

// Detects the full OSC terminator (BEL or `ESC \`) at the end of the accumulated buffer.
// `push` removes exactly the matched terminator once this returns true.
const fn is_osc_terminator(b: &[u8]) -> bool {
    matches!(b, [.., 0x07] | [.., 0x1b, 0x5c])
}

fn is_valid_osc_param(b: u8) -> bool {
    // if the character is a printable character, or is 0x1b or 0x5c then it is valid
    (0x20..=0x7E).contains(&b) || (0x80..=0xff).contains(&b) || b == 0x1b || b == 0x07
}

/// # Errors
/// Will return an error if a parameter segment cannot be parsed as an `AnsiOscToken`.
fn split_params_into_semicolon_delimited_tokens(
    params: &[u8],
) -> Result<Vec<Option<AnsiOscToken>>, AnsiParseError> {
    params
        .split(|b| *b == b';')
        .map(parse_param_as::<AnsiOscToken>)
        .collect::<Result<Vec<Option<AnsiOscToken>>, AnsiParseError>>()
}

#[cfg(test)]
mod tests {
    use super::{AnsiOscParser, AnsiOscParserState, MAX_OSC_BYTES, MAX_OSC_LARGE_BYTES, OscLimit};
    use crate::ansi::{FreminalAnsiParser, ParserOutcome};
    use freminal_common::buffer_states::ftcs::FtcsMarker;
    use freminal_common::buffer_states::osc::{AnsiOscType, UrlResponse};
    use freminal_common::buffer_states::pointer_shape::PointerShape;
    use freminal_common::buffer_states::terminal_output::TerminalOutput;
    use freminal_common::buffer_states::url::Url;

    fn feed_osc(payload: &[u8]) -> Vec<TerminalOutput> {
        let mut parser = AnsiOscParser::new();
        let mut output = Vec::new();
        for &b in payload {
            parser.ansiparser_inner_osc(b, &mut output);
        }
        output
    }

    // ------------------------------------------------------------------
    // push() state machine tests
    // ------------------------------------------------------------------

    #[test]
    fn push_invalid_byte_transitions_to_invalid() {
        let mut parser = AnsiOscParser::new();
        // 0x01 is not a valid OSC param byte
        let result = parser.push(0x01);
        assert!(matches!(result, ParserOutcome::Invalid(_)));
        assert_eq!(parser.state, AnsiOscParserState::Invalid);
    }

    #[test]
    fn push_after_finished_returns_invalid() {
        let mut parser = AnsiOscParser::new();
        let mut output = Vec::new();
        // Feed a complete BEL-terminated sequence
        for &b in b"10;?\x07" {
            parser.ansiparser_inner_osc(b, &mut output);
        }
        assert_eq!(parser.state, AnsiOscParserState::Finished);
        // Pushing after finish should return Invalid
        let result = parser.push(b'x');
        assert!(matches!(result, ParserOutcome::Invalid(_)));
    }

    #[test]
    fn push_in_invalid_state_continues_until_terminator() {
        let mut parser = AnsiOscParser::new();
        // Drive parser into Invalid state
        parser.push(0x01);
        assert_eq!(parser.state, AnsiOscParserState::Invalid);
        // Continue pushing — should return Invalid but not crash
        let result = parser.push(b'A');
        assert!(matches!(result, ParserOutcome::Invalid(_)));
    }

    // ------------------------------------------------------------------
    // OSC 10 / 11 / 12 — foreground / background / cursor color queries
    // ------------------------------------------------------------------

    #[test]
    fn osc10_foreground_query() {
        // OSC 10 ; ? BEL
        let output = feed_osc(b"10;?\x07");
        assert_eq!(output.len(), 1);
        assert!(matches!(
            &output[0],
            TerminalOutput::OscResponse(AnsiOscType::RequestColorQueryForeground(_))
        ));
    }

    #[test]
    fn osc11_background_query() {
        // OSC 11 ; ? BEL
        let output = feed_osc(b"11;?\x07");
        assert_eq!(output.len(), 1);
        assert!(matches!(
            &output[0],
            TerminalOutput::OscResponse(AnsiOscType::RequestColorQueryBackground(_))
        ));
    }

    #[test]
    fn osc12_cursor_color_query() {
        // OSC 12 ; ? BEL
        let output = feed_osc(b"12;?\x07");
        assert_eq!(output.len(), 1);
        assert!(matches!(
            &output[0],
            TerminalOutput::OscResponse(AnsiOscType::RequestColorQueryCursor(_))
        ));
    }

    // ------------------------------------------------------------------
    // OSC 22 — pointer (cursor) shape
    // ------------------------------------------------------------------

    #[test]
    fn osc22_set_pointer_shape_text() {
        // OSC 22 ; text BEL
        let output = feed_osc(b"22;text\x07");
        assert_eq!(output.len(), 1);
        assert!(matches!(
            &output[0],
            TerminalOutput::OscResponse(AnsiOscType::SetPointerShape(PointerShape::Text))
        ));
    }

    #[test]
    fn osc22_set_pointer_shape_crosshair() {
        // OSC 22 ; crosshair BEL
        let output = feed_osc(b"22;crosshair\x07");
        assert_eq!(output.len(), 1);
        assert!(matches!(
            &output[0],
            TerminalOutput::OscResponse(AnsiOscType::SetPointerShape(PointerShape::Crosshair))
        ));
    }

    #[test]
    fn osc22_empty_shape_resets_to_default() {
        // OSC 22 ; BEL — empty name → default pointer shape
        let output = feed_osc(b"22;\x07");
        assert_eq!(output.len(), 1);
        assert!(matches!(
            &output[0],
            TerminalOutput::OscResponse(AnsiOscType::SetPointerShape(PointerShape::Default))
        ));
    }

    #[test]
    fn osc22_no_param_defaults_to_default_shape() {
        // OSC 22 BEL — no second param
        let output = feed_osc(b"22\x07");
        assert_eq!(output.len(), 1);
        assert!(matches!(
            &output[0],
            TerminalOutput::OscResponse(AnsiOscType::SetPointerShape(PointerShape::Default))
        ));
    }

    // ------------------------------------------------------------------
    // OSC 52 — clipboard (via osc.rs dispatcher)
    // ------------------------------------------------------------------

    #[test]
    fn osc52_query_dispatched_correctly() {
        let output = feed_osc(b"52;c;?\x07");
        assert_eq!(output.len(), 1);
        assert!(matches!(
            &output[0],
            TerminalOutput::OscResponse(AnsiOscType::QueryClipboard(_))
        ));
    }

    // ------------------------------------------------------------------
    // OSC 4 — palette color (via dispatcher)
    // ------------------------------------------------------------------

    #[test]
    fn osc4_dispatched_correctly() {
        let output = feed_osc(b"4;7;rgb:ff/00/00\x07");
        assert_eq!(output.len(), 1);
        assert!(matches!(
            &output[0],
            TerminalOutput::OscResponse(AnsiOscType::SetPaletteColor(7, 0xff, 0x00, 0x00))
        ));
    }

    // ------------------------------------------------------------------
    // OSC 104 — reset palette (via dispatcher)
    // ------------------------------------------------------------------

    #[test]
    fn osc104_dispatched_correctly() {
        let output = feed_osc(b"104\x07");
        assert_eq!(output.len(), 1);
        assert!(matches!(
            &output[0],
            TerminalOutput::OscResponse(AnsiOscType::ResetPaletteColor(None))
        ));
    }

    // ------------------------------------------------------------------
    // Known-but-unimplemented targets (silently consumed)
    // ------------------------------------------------------------------

    #[test]
    fn osc13_mouse_foreground_silently_consumed() {
        // OSC 13 — MouseForeground — known but unimplemented
        let output = feed_osc(b"13;?\x07");
        assert_eq!(output, []);
    }

    #[test]
    fn osc14_mouse_background_silently_consumed() {
        // OSC 14 — MouseBackground — known but unimplemented
        let output = feed_osc(b"14;?\x07");
        assert_eq!(output, []);
    }

    #[test]
    fn osc66_color_scheme_notification_silently_consumed() {
        // OSC 66 — ColorSchemeNotification — known but unimplemented
        let output = feed_osc(b"66;dark\x07");
        assert_eq!(output, []);
    }

    // ------------------------------------------------------------------
    // Unknown OSC targets (silently consumed with warn)
    // ------------------------------------------------------------------

    #[test]
    fn unknown_osc_target_silently_consumed() {
        // OSC 999 — totally unknown target
        let output = feed_osc(b"999;whatever\x07");
        assert_eq!(output, []);
    }

    // ------------------------------------------------------------------
    // ansiparser_inner_osc in Invalid state
    // ------------------------------------------------------------------

    #[test]
    fn ansiparser_inner_osc_invalid_byte_returns_invalid() {
        let mut parser = AnsiOscParser::new();
        let mut output = Vec::new();
        // 0x01 is not valid → immediate Invalid
        let result = parser.ansiparser_inner_osc(0x01, &mut output);
        assert!(matches!(result, ParserOutcome::Invalid(_)));
    }

    // ------------------------------------------------------------------
    // trace_str coverage
    // ------------------------------------------------------------------

    #[test]
    fn trace_str_returns_string() {
        let mut parser = AnsiOscParser::new();
        let mut output = Vec::new();
        for &b in b"10;?\x07" {
            parser.ansiparser_inner_osc(b, &mut output);
        }
        // trace_str renders the accumulated OSC params (bounded for logging)
        let _ = parser.trace_str();
    }

    // =========================================================================
    // Coverage-gap tests
    // =========================================================================

    // ── Empty body after stripping the terminator ───────────────────────────
    // BEL alone terminates the OSC, but after stripping the terminator byte
    // the body is empty, so there is no OSC number and `ansiparser_inner_osc`
    // reports `Invalid`.
    #[test]
    fn empty_osc_just_bel_terminator() {
        let mut parser = AnsiOscParser::new();
        let mut output = Vec::new();
        let result = parser.ansiparser_inner_osc(0x07, &mut output);
        assert!(matches!(result, ParserOutcome::Invalid(_)));
    }

    // ── Invalid state + terminator → InvalidFinished ────────────────────────
    // NOTE: `self.state = AnsiOscParserState::InvalidFinished` is
    // effectively unreachable through `push()`: transitioning to Invalid
    // clears `self.params` and the Invalid arm never pushes bytes
    // into params, so `is_osc_terminator(&self.params)` always sees an empty
    // slice and returns false.  We test the reachable Invalid-state behavior
    // instead: push returns Invalid and state stays Invalid.
    #[test]
    fn invalid_state_stays_invalid_on_further_push() {
        let mut parser = AnsiOscParser::new();
        // Drive to Invalid with a control byte outside valid param range
        parser.push(0x01);
        assert_eq!(parser.state, AnsiOscParserState::Invalid);
        // Push BEL — state stays Invalid because params is empty
        let result = parser.push(0x07);
        assert!(matches!(result, ParserOutcome::Invalid(_)));
        assert_eq!(parser.state, AnsiOscParserState::Invalid);
    }

    // ── ansiparser_inner_osc in Invalid state (non-terminator) ──────────────
    #[test]
    fn ansiparser_inner_osc_invalid_state_non_terminator() {
        let mut parser = AnsiOscParser::new();
        let mut output = Vec::new();
        // Drive to Invalid
        parser.push(0x01);
        assert_eq!(parser.state, AnsiOscParserState::Invalid);
        // Feed a non-terminator printable byte through ansiparser_inner_osc
        // push() returns Invalid while the state is Invalid, and
        // ansiparser_inner_osc returns that result early, so its own
        // `AnsiOscParserState::Invalid` match arm is not reached here.
        let result = parser.ansiparser_inner_osc(b'A', &mut output);
        assert!(matches!(result, ParserOutcome::Invalid(_)));
    }

    // ── OSC 133 (FTCS) ──────────────────────────────────────────────────────
    #[test]
    fn osc133_ftcs_prompt_start() {
        // OSC 133 ; A ; freminal=1 ; fid=t1 BEL — FTCS prompt start (freminal format)
        let output = feed_osc(b"133;A;freminal=1;fid=t1\x07");
        assert_eq!(output.len(), 1);
        assert!(matches!(
            &output[0],
            TerminalOutput::OscResponse(AnsiOscType::Ftcs(_))
        ));
    }

    #[test]
    fn osc133_ftcs_command_start() {
        // OSC 133 ; B ; freminal=1 ; fid=t1 BEL — FTCS command start (freminal format)
        let output = feed_osc(b"133;B;freminal=1;fid=t1\x07");
        assert_eq!(output.len(), 1);
        assert!(matches!(
            &output[0],
            TerminalOutput::OscResponse(AnsiOscType::Ftcs(_))
        ));
    }

    #[test]
    fn osc133_ftcs_command_output_start() {
        // OSC 133 ; C ; freminal=1 ; fid=t1 BEL — FTCS output start (freminal format)
        let output = feed_osc(b"133;C;freminal=1;fid=t1\x07");
        assert_eq!(output.len(), 1);
        assert!(matches!(
            &output[0],
            TerminalOutput::OscResponse(AnsiOscType::Ftcs(_))
        ));
    }

    #[test]
    fn osc133_ftcs_command_done_with_exit_code() {
        // OSC 133 ; D ; 0 ; freminal=1 ; fid=t1 BEL — FTCS done (freminal format)
        let output = feed_osc(b"133;D;0;freminal=1;fid=t1\x07");
        assert_eq!(output.len(), 1);
        assert!(matches!(
            &output[0],
            TerminalOutput::OscResponse(AnsiOscType::Ftcs(_))
        ));
    }

    #[test]
    fn osc133_ftcs_plain_marker_silently_consumed() {
        // OSC 133 ; A BEL — plain marker without freminal=1 is silently dropped
        let output = feed_osc(b"133;A\x07");
        assert!(
            output.is_empty(),
            "plain marker without freminal=1 must produce no output"
        );
    }

    #[test]
    fn osc133_ftcs_foreign_marker_silently_consumed() {
        // OSC 133 ; A ; aid=12345 BEL — WezTerm-style marker is silently dropped
        let output = feed_osc(b"133;A;aid=12345\x07");
        assert!(
            output.is_empty(),
            "foreign marker without freminal=1 must produce no output"
        );
    }

    #[test]
    fn osc133_ftcs_unknown_marker_consumed_and_logged() {
        // OSC 133 ; Z BEL — `Z` is NOT a known FTCS marker (A/B/C/D/P), so this
        // is treated as an unrecognised/malformed OSC 133: no buffer output,
        // but it IS logged at warn (with the raw sequence) so a new/unknown
        // OSC 133 variant surfaces for auditing rather than vanishing.
        let output = feed_osc(b"133;Z\x07");
        assert_eq!(output, []);
    }

    #[test]
    fn osc133_known_foreign_marker_consumed_without_output() {
        // Broaden coverage beyond `A` (see `osc133_ftcs_foreign_marker_...`):
        // the `freminal=1`-gated markers B/C/D, when sent WITHOUT `freminal=1`,
        // are foreign duplicates — recognised, deliberately ignored, no output
        // (and, by design, no unhandled-log). `P` is excluded here: it is
        // intentionally freminal-independent and DOES emit output.
        for seq in [
            b"133;B\x07".as_slice(),   // prompt end / command input start
            b"133;C\x07".as_slice(),   // command output start
            b"133;D;1\x07".as_slice(), // command finished, exit code 1
        ] {
            let output = feed_osc(seq);
            assert!(
                output.is_empty(),
                "known foreign marker {seq:?} must produce no output"
            );
        }
    }

    // ── OSC 7 (RemoteHost) ──────────────────────────────────────────────────
    #[test]
    fn osc7_remote_host() {
        // OSC 7 ; file:///home/user BEL
        let output = feed_osc(b"7;file:///home/user\x07");
        assert_eq!(
            output,
            [TerminalOutput::OscResponse(AnsiOscType::RemoteHost(
                "file:///home/user".to_owned()
            ))]
        );
    }

    // ── Reset color OSCs ────────────────────────────────────────────────────
    #[test]
    fn osc112_reset_cursor_color() {
        // OSC 112 BEL — reset cursor color
        let output = feed_osc(b"112\x07");
        assert_eq!(output.len(), 1);
        assert!(matches!(
            &output[0],
            TerminalOutput::OscResponse(AnsiOscType::ResetCursorColor)
        ));
    }

    #[test]
    fn osc110_reset_foreground() {
        // OSC 110 BEL — reset foreground color
        let output = feed_osc(b"110\x07");
        assert_eq!(output.len(), 1);
        assert!(matches!(
            &output[0],
            TerminalOutput::OscResponse(AnsiOscType::ResetForegroundColor)
        ));
    }

    #[test]
    fn osc111_reset_background() {
        // OSC 111 BEL — reset background color
        let output = feed_osc(b"111\x07");
        assert_eq!(output.len(), 1);
        assert!(matches!(
            &output[0],
            TerminalOutput::OscResponse(AnsiOscType::ResetBackgroundColor)
        ));
    }

    // ── OSC 8 (URL) ─────────────────────────────────────────────────────────
    #[test]
    fn osc8_url() {
        // OSC 8 ; ; https://example.com BEL
        let output = feed_osc(b"8;;https://example.com\x07");
        assert_eq!(output.len(), 1);
        assert!(matches!(
            &output[0],
            TerminalOutput::OscResponse(AnsiOscType::Url(_))
        ));
    }

    // ── OSC title sequences ─────────────────────────────────────────────────
    #[test]
    fn osc0_set_title_bar() {
        // OSC 0 ; My Title BEL — set window title
        let output = feed_osc(b"0;My Title\x07");
        assert_eq!(output.len(), 1);
        assert!(matches!(
            &output[0],
            TerminalOutput::OscResponse(AnsiOscType::SetTitleBar(_))
        ));
    }

    #[test]
    fn osc2_set_title_bar() {
        // OSC 2 ; Another Title BEL — set window title (same as 0)
        let output = feed_osc(b"2;Another Title\x07");
        assert_eq!(output.len(), 1);
        assert!(matches!(
            &output[0],
            TerminalOutput::OscResponse(AnsiOscType::SetTitleBar(_))
        ));
    }

    // ── Terminator stripping removes exactly the terminator ─────────────────
    fn title_of(output: &[TerminalOutput]) -> &str {
        match output {
            [TerminalOutput::OscResponse(AnsiOscType::SetTitleBar(t))] => t.as_str(),
            other => panic!("expected a single SetTitleBar, got {other:?}"),
        }
    }

    #[test]
    fn osc_title_ending_in_backslash_bel_keeps_backslash() {
        let output = feed_osc(b"0;C:\\\x07");
        assert_eq!(title_of(&output), "C:\\");
    }

    #[test]
    fn osc_title_ending_in_backslash_st_keeps_backslash() {
        let output = feed_osc(b"0;C:\\\x1b\\");
        assert_eq!(title_of(&output), "C:\\");
    }

    #[test]
    fn osc_title_ending_in_two_backslashes_bel_keeps_both() {
        let output = feed_osc(b"0;a\\\\\x07");
        assert_eq!(title_of(&output), "a\\\\");
    }

    #[test]
    fn osc_body_ending_in_esc_terminated_by_bel_keeps_esc() {
        // OSC 9 ; hi ESC BEL — the ESC belongs to the body, BEL is the terminator.
        let output = feed_osc(b"9;hi\x1b\x07");
        assert_eq!(output.len(), 1);
        match &output[0] {
            TerminalOutput::OscResponse(AnsiOscType::Notify { body, .. }) => {
                assert_eq!(body, "hi\u{1b}");
            }
            other => panic!("expected Notify, got {other:?}"),
        }
    }

    #[test]
    fn push_strips_one_byte_for_bel_and_two_for_st() {
        let mut bel = AnsiOscParser::new();
        for &b in b"0;a\\\x07" {
            bel.push(b);
        }
        assert_eq!(bel.params, b"0;a\\");

        let mut st = AnsiOscParser::new();
        for &b in b"0;a\\\x1b\\" {
            st.push(b);
        }
        assert_eq!(st.params, b"0;a\\");

        let mut esc_bel = AnsiOscParser::new();
        for &b in b"0;a\x1b\x07" {
            esc_bel.push(b);
        }
        assert_eq!(esc_bel.params, b"0;a\x1b");
    }

    // ── ST terminator (ESC \) ───────────────────────────────────────────────
    #[test]
    fn osc_with_st_terminator() {
        // OSC 0 ; Title ESC \ — terminated with ST instead of BEL
        let output = feed_osc(b"0;Title\x1b\\");
        assert_eq!(output.len(), 1);
        assert!(matches!(
            &output[0],
            TerminalOutput::OscResponse(AnsiOscType::SetTitleBar(_))
        ));
    }

    // ── Raw-body dispatch (Task 129.5) ──────────────────────────────────────

    /// Feed a full OSC body and return both the output and the outcome of the
    /// final byte.
    fn feed_osc_with_outcome(payload: &[u8]) -> (Vec<TerminalOutput>, ParserOutcome) {
        let mut parser = AnsiOscParser::new();
        let mut output = Vec::new();
        let mut outcome = ParserOutcome::Continue;
        for &b in payload {
            outcome = parser.ansiparser_inner_osc(b, &mut output);
        }
        (output, outcome)
    }

    fn assert_no_invalid(output: &[TerminalOutput], outcome: &ParserOutcome) {
        assert!(
            !output.iter().any(|o| matches!(o, TerminalOutput::Invalid)),
            "unexpected TerminalOutput::Invalid in {output:?}"
        );
        assert_eq!(*outcome, ParserOutcome::Finished);
    }

    fn assert_invalid(payload: &[u8]) {
        let (output, outcome) = feed_osc_with_outcome(payload);
        assert!(
            output.iter().any(|o| matches!(o, TerminalOutput::Invalid)),
            "expected TerminalOutput::Invalid for {payload:?}, got {output:?}"
        );
        assert!(
            matches!(&outcome, ParserOutcome::Invalid(m) if m.starts_with("Invalid OSC params: recent='")),
            "unexpected outcome {outcome:?}"
        );
    }

    #[test]
    fn osc9_with_latin1_byte_reaches_handler() {
        // The OSC 9 handler validates UTF-8 itself and drops the payload. It
        // emits nothing, so its own debug line is the proof it ran.
        let events = crate::log_capture::capture(|| {
            let (output, outcome) = feed_osc_with_outcome(b"9;naiv\xe9\x07");
            assert_no_invalid(&output, &outcome);
            assert_eq!(output, []);
        });
        assert!(
            events
                .iter()
                .any(|(_, m)| m.contains("OSC notification: non-UTF-8 payload")),
            "the OSC 9 handler did not run: {events:?}"
        );
    }

    #[test]
    fn osc9_valid_notification_still_dispatched() {
        let (output, outcome) = feed_osc_with_outcome(b"9;hello; world\x07");
        assert_no_invalid(&output, &outcome);
        assert_eq!(output.len(), 1);
        assert!(matches!(
            &output[0],
            TerminalOutput::OscResponse(AnsiOscType::Notify { .. })
        ));
    }

    #[test]
    fn osc777_with_latin1_byte_reaches_handler() {
        // The handler emits nothing for a non-UTF-8 payload, so its own
        // debug line is the proof it ran.
        let events = crate::log_capture::capture(|| {
            let (output, outcome) = feed_osc_with_outcome(b"777;notify;t\xe9;body\x07");
            assert_no_invalid(&output, &outcome);
            assert_eq!(output, []);
        });
        assert!(
            events
                .iter()
                .any(|(_, m)| m.contains("OSC notification: non-UTF-8 payload")),
            "the OSC 777 handler did not run: {events:?}"
        );
    }

    #[test]
    fn osc99_with_latin1_byte_reaches_handler() {
        // A Latin-1 byte inside the value of an unknown metadata key does not
        // make the sequence Invalid, and the OSC 99 handler still emits its
        // command (a plain, complete payload).
        let (output, outcome) = feed_osc_with_outcome(b"99;i=1:d=1:z=naiv\xe9;hello\x07");
        assert_no_invalid(&output, &outcome);
        assert!(
            matches!(
                output.as_slice(),
                [TerminalOutput::OscResponse(AnsiOscType::Notify99(_))]
            ),
            "the OSC 99 handler did not emit a command: {output:?}"
        );
    }

    #[test]
    fn osc1337_with_latin1_byte_reaches_handler() {
        // Unrecognised iTerm2 sub-command carrying a non-UTF-8 byte: the
        // handler reports it as unknown; the parser must not flag the
        // sequence invalid.
        let (output, outcome) = feed_osc_with_outcome(b"1337;SetMark\xe9=1\x07");
        assert_no_invalid(&output, &outcome);
        assert_eq!(
            output,
            [TerminalOutput::OscResponse(AnsiOscType::ITerm2Unknown)]
        );
    }

    #[test]
    fn osc1338_with_latin1_byte_reaches_handler() {
        // The handler emits nothing for a non-UTF-8 path, so its own warn is
        // the proof it ran.
        let events = crate::log_capture::capture(|| {
            let (output, outcome) = feed_osc_with_outcome(b"1338;HISTFILE=/home/\xe9/h\x07");
            assert_no_invalid(&output, &outcome);
            assert_eq!(output, []);
        });
        assert!(
            crate::log_capture::warnings(&events)
                .iter()
                .any(|(_, m)| m.contains("OSC 1338 HISTFILE=: non-UTF-8 path")),
            "the OSC 1338 handler did not run: {events:?}"
        );
    }

    #[test]
    fn osc1338_valid_histfile_still_dispatched() {
        let (output, outcome) = feed_osc_with_outcome(b"1338;HISTFILE=/home/u/h\x07");
        assert_no_invalid(&output, &outcome);
        assert_eq!(output.len(), 1);
        assert!(matches!(
            &output[0],
            TerminalOutput::OscResponse(AnsiOscType::ShellInfoHistFile(_))
        ));
    }

    #[test]
    fn tokenising_targets_with_non_utf8_byte_still_invalid() {
        // OSC 52 clipboard, plus the colour, palette and pointer-shape
        // targets that tokenise. (Titles and OSC 7 no longer tokenise; see
        // the 129.6 tests below. Neither does OSC 8; see the 129.7 tests
        // below. Neither does OSC 133; see the 129.8 tests below.)
        for payload in [
            &b"52;c;\xe9\x07"[..],
            b"10;\xe9\x07",
            b"11;\xe9\x07",
            b"12;\xe9\x07",
            b"4;1;\xe9\x07",
            b"104;\xe9\x07",
            b"22;\xe9\x07",
        ] {
            assert_invalid(payload);
        }
    }

    #[test]
    fn non_utf8_in_later_segment_of_tokenising_target_is_invalid() {
        assert_invalid(b"4;1;ok;\xe9\x07");
    }

    #[test]
    fn empty_osc_number_is_invalid() {
        assert_invalid(b";body\x07");
        assert_invalid(b"\x07");
    }

    #[test]
    fn non_numeric_osc_number_is_unknown_not_invalid() {
        let (output, outcome) = feed_osc_with_outcome(b"abc;body\x07");
        assert_no_invalid(&output, &outcome);
        assert_eq!(output, []);
    }

    #[test]
    fn non_utf8_osc_number_is_invalid() {
        assert_invalid(b"\xe9;body\x07");
        assert_invalid(b"9\xe9;body\x07");
    }

    #[test]
    fn unknown_target_with_non_utf8_body_consumed_without_invalid() {
        let (output, outcome) = feed_osc_with_outcome(b"999;\xe9\x07");
        assert_no_invalid(&output, &outcome);
        assert_eq!(output, []);
    }

    #[test]
    fn logging_targets_with_non_utf8_body_consumed_without_invalid() {
        for payload in [&b"13;\xe9\x07"[..], b"66;\xe9\x07"] {
            let (output, outcome) = feed_osc_with_outcome(payload);
            assert_no_invalid(&output, &outcome);
            assert_eq!(output, []);
        }
    }

    // ── Titles and OSC 7 take the full remainder (Task 129.6) ───────────────

    fn feed_text_osc(payload: &[u8]) -> Vec<TerminalOutput> {
        let (output, outcome) = feed_osc_with_outcome(payload);
        assert_no_invalid(&output, &outcome);
        output
    }

    fn set_title(text: &str) -> TerminalOutput {
        TerminalOutput::OscResponse(AnsiOscType::SetTitleBar(text.to_owned()))
    }

    #[test]
    fn osc2_title_keeps_semicolons() {
        assert_eq!(feed_text_osc(b"2;a;b\x07"), [set_title("a;b")]);
    }

    #[test]
    fn osc0_numeric_title_is_text() {
        assert_eq!(feed_text_osc(b"0;123\x07"), [set_title("123")]);
    }

    #[test]
    fn osc0_question_mark_title_is_text() {
        assert_eq!(feed_text_osc(b"0;?\x07"), [set_title("?")]);
    }

    #[test]
    fn osc2_empty_remainder_is_empty_title() {
        assert_eq!(feed_text_osc(b"2;\x07"), [set_title("")]);
    }

    #[test]
    fn osc2_without_separator_produces_no_output() {
        assert_eq!(feed_text_osc(b"2\x07"), []);
    }

    #[test]
    fn osc_title_non_utf8_produces_no_output_and_is_not_invalid() {
        for payload in [&b"2;t\xe9\x07"[..], b"0;t\xe9\x07", b"2;ok;\xe9\x07"] {
            assert_eq!(feed_text_osc(payload), []);
        }
    }

    #[test]
    fn osc1_icon_name_maps_to_set_title_bar() {
        assert_eq!(feed_text_osc(b"1;icon\x07"), [set_title("icon")]);
    }

    #[test]
    fn osc7_keeps_semicolons_in_uri() {
        assert_eq!(
            feed_text_osc(b"7;file://host/a;b\x07"),
            [TerminalOutput::OscResponse(AnsiOscType::RemoteHost(
                "file://host/a;b".to_owned()
            ))]
        );
    }

    #[test]
    fn osc7_without_separator_or_non_utf8_produces_no_output() {
        for payload in [&b"7\x07"[..], b"7;file://h\xe9/\x07"] {
            assert_eq!(feed_text_osc(payload), []);
        }
    }

    // ── OSC 8 parsed from the raw body (Task 129.7) ─────────────────────────

    fn hyperlink(id: Option<&str>, url: &str) -> TerminalOutput {
        TerminalOutput::OscResponse(AnsiOscType::Url(UrlResponse::Url(Url {
            id: id.map(str::to_owned),
            url: url.to_owned(),
        })))
    }

    fn hyperlink_end() -> TerminalOutput {
        TerminalOutput::OscResponse(AnsiOscType::Url(UrlResponse::End))
    }

    #[test]
    fn osc8_url_with_semicolon_is_one_link() {
        assert_eq!(
            feed_text_osc(b"8;;http://a;b\x07"),
            [hyperlink(None, "http://a;b")]
        );
    }

    #[test]
    fn osc8_id_is_parsed_from_params() {
        assert_eq!(
            feed_text_osc(b"8;foo=1:id=x;u\x07"),
            [hyperlink(Some("x"), "u")]
        );
    }

    #[test]
    fn osc8_empty_uri_and_bare_number_end_the_link() {
        for payload in [&b"8;;\x07"[..], b"8\x07", b"8;id=x\x07"] {
            assert_eq!(feed_text_osc(payload), [hyperlink_end()]);
        }
    }

    #[test]
    fn osc8_non_utf8_uri_produces_no_output_and_is_not_invalid() {
        for payload in [&b"8;;http://e\xe9\x07"[..], b"8;;ok;\xe9\x07"] {
            assert_eq!(feed_text_osc(payload), []);
        }
    }

    #[test]
    fn osc8_non_utf8_params_do_not_invalidate_the_sequence() {
        assert_eq!(
            feed_text_osc(b"8;\xe9=1:id=x;u\x07"),
            [hyperlink(Some("x"), "u")]
        );
    }

    // ── OSC 133 parsed from the raw body (Task 129.8) ───────────────────────

    fn ftcs_marker(output: &[TerminalOutput]) -> &FtcsMarker {
        match output {
            [TerminalOutput::OscResponse(AnsiOscType::Ftcs(marker))] => marker,
            other => panic!("expected a single FTCS marker, got {other:?}"),
        }
    }

    #[test]
    fn osc133_non_utf8_item_does_not_invalidate_the_sequence() {
        // A foreign, non-UTF-8 item no longer makes the sequence Invalid.
        let (output, outcome) = feed_osc_with_outcome(b"133;A;aid=\xe9;freminal=1;fid=x\x07");
        assert_no_invalid(&output, &outcome);
        assert_eq!(
            ftcs_marker(&output),
            &FtcsMarker::PromptStart {
                fid: "x".to_owned()
            }
        );
    }

    #[test]
    fn osc133_non_utf8_fid_is_dropped_without_invalid() {
        let (output, outcome) = feed_osc_with_outcome(b"133;A;freminal=1;fid=\xe9\x07");
        assert_no_invalid(&output, &outcome);
        assert_eq!(output, []);
    }

    #[test]
    fn osc133_exit_code_with_leading_zeros_and_empty_fields() {
        let output = feed_osc(b"133;D;;007;;freminal=1;fid=t1\x07");
        assert_eq!(
            ftcs_marker(&output),
            &FtcsMarker::CommandFinished {
                exit_code: Some(7),
                fid: "t1".to_owned()
            }
        );
    }

    #[test]
    fn osc133_without_body_is_consumed_without_output() {
        for payload in [&b"133\x07"[..], b"133;\x07", b"133;;\x07"] {
            let (output, outcome) = feed_osc_with_outcome(payload);
            assert_no_invalid(&output, &outcome);
            assert_eq!(output, [], "payload {payload:?}");
        }
    }

    // ── Byte caps (Task 129.9) ──────────────────────────────────────────────

    /// An OSC body `prefix` padded with `x` to exactly `len` bytes.
    fn body_of_len(prefix: &[u8], len: usize) -> Vec<u8> {
        let mut body = prefix.to_vec();
        body.resize(len, b'x');
        body
    }

    /// A parser that has already accepted `body`, without a terminator.
    ///
    /// Sets `params` directly so the 64 MiB boundaries do not need a 64 MiB
    /// byte-at-a-time loop. The limit stays `Default`, exactly as if the bytes
    /// had been pushed, because the OSC number is only examined once a push
    /// crosses the default cap.
    fn parser_with_body(body: Vec<u8>) -> AnsiOscParser {
        let mut parser = AnsiOscParser::new();
        parser.params = body;
        parser
    }

    #[test]
    fn exactly_at_cap_is_dispatched_with_bel_and_with_st() {
        // The cap counts the body before its terminator: `2;` + title.
        let body = body_of_len(b"2;", MAX_OSC_BYTES);
        for terminator in [&b"\x07"[..], &b"\x1b\\"[..]] {
            let mut payload = body.clone();
            payload.extend_from_slice(terminator);
            let (output, outcome) = feed_osc_with_outcome(&payload);
            assert_eq!(outcome, ParserOutcome::Finished);
            assert_eq!(output.len(), 1);
            let TerminalOutput::OscResponse(AnsiOscType::SetTitleBar(title)) = &output[0] else {
                panic!("expected a title, got {:?}", output[0]);
            };
            assert_eq!(title.len(), MAX_OSC_BYTES - 2);
        }
    }

    #[test]
    fn one_byte_over_cap_is_dropped() {
        let body = body_of_len(b"2;", MAX_OSC_BYTES + 1);
        for terminator in [&b"\x07"[..], &b"\x1b\\"[..]] {
            let mut payload = body.clone();
            payload.extend_from_slice(terminator);
            let (output, outcome) = feed_osc_with_outcome(&payload);
            assert_eq!(outcome, ParserOutcome::Finished);
            assert_eq!(output, []);
        }
    }

    #[test]
    fn cap_body_ending_in_esc_terminated_by_bel_is_dropped() {
        // A body, then ESC (held uncounted), then BEL: the ESC
        // becomes body. A MAX-byte body plus ESC is one over; MAX - 1 plus ESC
        // is exactly at the cap.
        let mut over = body_of_len(b"2;", MAX_OSC_BYTES);
        over.extend_from_slice(b"\x1b\x07");
        let (output, outcome) = feed_osc_with_outcome(&over);
        assert_eq!(outcome, ParserOutcome::Finished);
        assert_eq!(output, [], "a MAX + 1 body must not be dispatched");

        let mut at_cap = body_of_len(b"2;", MAX_OSC_BYTES - 1);
        at_cap.extend_from_slice(b"\x1b\x07");
        let (output, outcome) = feed_osc_with_outcome(&at_cap);
        assert_eq!(outcome, ParserOutcome::Finished);
        let [TerminalOutput::OscResponse(AnsiOscType::SetTitleBar(title))] = output.as_slice()
        else {
            panic!("a MAX body must be dispatched, got {output:?}");
        };
        assert_eq!(title.len(), MAX_OSC_BYTES - 2);
        assert!(title.ends_with('\u{1b}'));
    }

    #[test]
    fn cap_body_ending_in_esc_terminated_by_bel_warns_without_payload() {
        let mut over = body_of_len(b"2;SECRETMARK", MAX_OSC_BYTES);
        over.extend_from_slice(b"\x1b\x07");
        let events = crate::log_capture::capture(|| {
            let (output, _) = feed_osc_with_outcome(&over);
            assert_eq!(output, []);
        });
        let warns = crate::log_capture::warnings(&events);
        assert_eq!(warns.len(), 1, "{events:?}");
        assert!(warns[0].1.contains("OSC") && !warns[0].1.contains("SECRETMARK"));
    }

    #[test]
    fn overflow_warns_with_introducer_and_length_but_not_payload() {
        // A body at the cap carrying the marker, then one more byte (over the
        // cap) and BEL: the terminator is part of the reported total.
        let mut parser = parser_with_body(body_of_len(b"2;SECRETMARK", MAX_OSC_BYTES));
        let events = crate::log_capture::capture(|| {
            assert_eq!(parser.push(b'x'), ParserOutcome::Continue);
            assert_eq!(parser.push(0x07), ParserOutcome::Finished);
        });
        let warns = crate::log_capture::warnings(&events);
        assert_eq!(warns.len(), 1, "{events:?}");
        let total = MAX_OSC_BYTES + 2;
        assert!(warns[0].1.contains("OSC"), "{warns:?}");
        assert!(
            warns[0].1.contains(&format!("total length {total} bytes")),
            "{warns:?}"
        );
        assert!(!warns[0].1.contains("SECRETMARK"), "{warns:?}");
    }

    #[test]
    fn overflow_releases_the_buffer() {
        let mut parser = AnsiOscParser::new();
        for &b in &body_of_len(b"2;", MAX_OSC_BYTES + 1) {
            assert_eq!(parser.push(b), ParserOutcome::Continue);
        }
        assert!(matches!(parser.state, AnsiOscParserState::Overflow { .. }));
        assert_eq!(parser.params.capacity(), 0);
    }

    #[test]
    fn large_cap_applies_to_osc_52_and_osc_1337_but_not_osc_2() {
        for prefix in [&b"52;c;"[..], &b"1337;File=inline=1:"[..]] {
            let mut parser = AnsiOscParser::new();
            for &b in &body_of_len(prefix, MAX_OSC_BYTES + 1024) {
                assert_eq!(parser.push(b), ParserOutcome::Continue);
            }
            assert_eq!(parser.state, AnsiOscParserState::Params, "{prefix:?}");
            assert_eq!(parser.params.len(), MAX_OSC_BYTES + 1024, "{prefix:?}");
        }

        for prefix in [
            &b"2;"[..],
            &b"522;"[..],
            &b"13370;"[..],
            &b"no-semicolon"[..],
        ] {
            let mut parser = AnsiOscParser::new();
            for &b in &body_of_len(prefix, MAX_OSC_BYTES + 1024) {
                assert_eq!(parser.push(b), ParserOutcome::Continue);
            }
            assert!(
                matches!(parser.state, AnsiOscParserState::Overflow { .. }),
                "{prefix:?}"
            );
            assert_eq!(parser.params.capacity(), 0, "{prefix:?}");
        }
    }

    #[test]
    fn osc_52_over_the_default_cap_is_dispatched() {
        // 2 MiB of base64 'A' (a multiple of four) after `52;c;`.
        let mut payload = b"52;c;".to_vec();
        payload.resize(5 + 2 * 1024 * 1024, b'A');
        payload.push(0x07);
        let (output, outcome) = feed_osc_with_outcome(&payload);
        assert_eq!(outcome, ParserOutcome::Finished);
        assert_eq!(output.len(), 1);
        assert!(
            !matches!(output[0], TerminalOutput::Invalid),
            "{:?}",
            output[0]
        );
    }

    #[test]
    fn large_cap_exactly_at_cap_is_dispatched_and_one_over_is_dropped() {
        for prefix in [&b"52;c;"[..], &b"1337;File=inline=1:"[..]] {
            // At the cap: BEL completes it.
            let mut parser = parser_with_body(body_of_len(prefix, MAX_OSC_LARGE_BYTES - 1));
            assert_eq!(parser.push(b'x'), ParserOutcome::Continue);
            assert_eq!(parser.state, AnsiOscParserState::Params, "{prefix:?}");
            assert_eq!(parser.params.len(), MAX_OSC_LARGE_BYTES);
            assert_eq!(parser.push(0x07), ParserOutcome::Finished);
            assert_eq!(parser.state, AnsiOscParserState::Finished, "{prefix:?}");
            assert_eq!(parser.params.len(), MAX_OSC_LARGE_BYTES);

            // One byte over: dropped, and ST ends it with no output.
            let mut parser = parser_with_body(body_of_len(prefix, MAX_OSC_LARGE_BYTES));
            assert_eq!(parser.push(b'x'), ParserOutcome::Continue);
            assert!(
                matches!(parser.state, AnsiOscParserState::Overflow { .. }),
                "{prefix:?}"
            );
            assert_eq!(parser.params.capacity(), 0);
            assert_eq!(parser.push(0x1b), ParserOutcome::Continue);
            assert_eq!(parser.push(b'\\'), ParserOutcome::Finished);
            assert_eq!(parser.state, AnsiOscParserState::OverflowFinished);
        }
    }

    #[test]
    fn overflowed_osc_ends_only_at_bel_or_a_real_st() {
        let overflowed = || {
            let mut parser = parser_with_body(body_of_len(b"2;", MAX_OSC_BYTES));
            assert_eq!(parser.push(b'x'), ParserOutcome::Continue);
            assert!(matches!(parser.state, AnsiOscParserState::Overflow { .. }));
            parser
        };

        // A lone backslash and an ESC followed by something else do not end it.
        let mut parser = overflowed();
        assert_eq!(parser.push(b'\\'), ParserOutcome::Continue);
        assert_eq!(parser.push(0x1b), ParserOutcome::Continue);
        assert_eq!(parser.push(b'x'), ParserOutcome::Continue);
        assert_eq!(parser.push(b'\\'), ParserOutcome::Continue);
        // ESC ESC \: the second ESC starts the terminator.
        assert_eq!(parser.push(0x1b), ParserOutcome::Continue);
        assert_eq!(parser.push(0x1b), ParserOutcome::Continue);
        assert_eq!(parser.push(b'\\'), ParserOutcome::Finished);

        // BEL ends it.
        let mut parser = overflowed();
        assert_eq!(parser.push(0x07), ParserOutcome::Finished);
    }

    #[test]
    fn overflowed_osc_still_rejects_invalid_bytes() {
        let mut parser = parser_with_body(body_of_len(b"2;", MAX_OSC_BYTES));
        assert_eq!(parser.push(b'x'), ParserOutcome::Continue);
        // 0x01 is not a valid OSC body byte; same handling as within the cap.
        assert!(matches!(parser.push(0x01), ParserOutcome::Invalid(_)));
        assert_eq!(parser.state, AnsiOscParserState::Invalid);
    }

    #[test]
    fn overflowed_sequence_is_followed_by_normal_text() {
        for terminator in [&b"\x07"[..], &b"\x1b\\"[..]] {
            let mut bytes = b"\x1b]2;".to_vec();
            bytes.extend(std::iter::repeat_n(b'T', MAX_OSC_BYTES + 10));
            bytes.extend_from_slice(terminator);
            bytes.extend_from_slice(b"hello");

            let mut parser = FreminalAnsiParser::new();
            let output = parser.push(&bytes);
            assert_eq!(output, vec![TerminalOutput::Data(b"hello".to_vec())]);
        }
    }

    #[test]
    fn at_cap_sequence_through_the_full_parser_is_dispatched() {
        let mut bytes = b"\x1b]".to_vec();
        bytes.extend(body_of_len(b"2;", MAX_OSC_BYTES));
        bytes.extend_from_slice(b"\x07hello");

        let mut parser = FreminalAnsiParser::new();
        let output = parser.push(&bytes);
        assert_eq!(output.len(), 2);
        assert!(matches!(
            &output[0],
            TerminalOutput::OscResponse(AnsiOscType::SetTitleBar(_))
        ));
        assert_eq!(output[1], TerminalOutput::Data(b"hello".to_vec()));
    }

    #[test]
    fn osc_limit_large_for_numerically_equal_osc_numbers() {
        for body in [
            &b"52;c;x"[..],
            b"052;c;x",
            b"0052",
            b"1337;File=x",
            b"01337;File=x",
            b"+52;c;x",
            b"+1337;File=x",
        ] {
            assert_eq!(OscLimit::for_body(body), OscLimit::Large, "{body:?}");
        }
    }

    #[test]
    fn osc_limit_default_for_other_osc_numbers() {
        let overlong = "99999999999999999999;x".to_string();
        for body in [
            &b"2;title"[..],
            b"52x;c;x",
            b"5;c;x",
            b";52",
            b"",
            overlong.as_bytes(),
        ] {
            assert_eq!(OscLimit::for_body(body), OscLimit::Default, "{body:?}");
        }
    }
}
