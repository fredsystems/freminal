// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Diagnostics for raw escape-sequence bytes.
//!
//! A ring buffer of the most recent input bytes (pushing is allocation-free),
//! plus helpers that render byte slices for logs, with bounded variants for
//! potentially huge payloads.
//!
//! Only the top-level `FreminalAnsiParser` owns a [`SequenceTracer`]. The
//! per-sequence sub-parsers (CSI, OSC, DCS, APC, standard) must NOT embed one:
//! the buffer is 8 KB, and every escape sequence constructs and moves a fresh
//! sub-parser, so an embedded tracer costs ~16 KB of memset + memmove per
//! sequence. Sub-parsers render diagnostics from the bytes they already
//! accumulate, via the bounded helpers below.

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SequenceTracer {
    buf: [u8; 8192],
    len: usize,
    idx: usize,
}

impl Default for SequenceTracer {
    fn default() -> Self {
        Self::new()
    }
}

impl SequenceTracer {
    pub(crate) const fn new() -> Self {
        Self {
            buf: [0; 8192],
            len: 0,
            idx: 0,
        }
    }

    pub(crate) const fn clear(&mut self) {
        self.len = 0;
        self.idx = 0;
    }

    pub(crate) const fn push(&mut self, b: u8) {
        self.buf[self.idx] = b;
        self.idx = (self.idx + 1) % self.buf.len();
        if self.len < self.buf.len() {
            self.len += 1;
        }
    }

    #[must_use]
    pub fn as_str(&self) -> String {
        if self.len == 0 {
            return String::new();
        }
        String::from_utf8_lossy(&self.to_bytes()).into_owned()
    }

    /// Return the traced bytes in order, oldest-to-newest.
    ///
    /// Unlike [`Self::as_str`], this is lossless: it never applies UTF-8
    /// replacement. Use it when the exact bytes matter (diagnostics that must
    /// let a reader reconstruct the offending sequence).
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        if self.len == 0 {
            return Vec::new();
        }
        let end = self.idx;
        let start = (self.idx + self.buf.len() - self.len) % self.buf.len();
        let mut out = Vec::with_capacity(self.len);
        if start < end {
            out.extend_from_slice(&self.buf[start..end]);
        } else {
            out.extend_from_slice(&self.buf[start..]);
            out.extend_from_slice(&self.buf[..end]);
        }
        out
    }

    /// Render the traced bytes as an unambiguous, reconstruction-faithful
    /// string for logging (see [`escape_sequence_for_log`]).
    #[must_use]
    pub fn as_escaped(&self) -> String {
        escape_sequence_for_log(&self.to_bytes())
    }
}

/// Render a raw escape-sequence byte slice as an unambiguous, printable,
/// reconstruction-faithful string for logging.
///
/// Escape-sequence payloads routinely contain non-printable control bytes
/// (`ESC`, `ST`, `BEL`), 8-bit C1 introducers, and non-UTF-8 binary (base64
/// padding, DCS/APC bodies). Rendering them with `String::from_utf8_lossy`
/// destroys that detail — every unrepresentable byte collapses to U+FFFD, so
/// the log no longer identifies the exact bytes that were received. This
/// function is lossless in the sense that the original bytes can be
/// reconstructed from its output:
///
/// - Printable ASCII (`0x20..=0x7E`) is emitted verbatim, **except** the
///   backslash, which is doubled (`\\`), and the double quote, which is
///   escaped (`\"`) — both so the escaping is unambiguous. Nearly every call
///   site embeds the result inside a quoted log string (e.g.
///   `"raw sequence: \"{}\""`), so an unescaped `"` in the payload would break
///   the surrounding quoting.
/// - Every other byte — C0/C1 controls, `DEL`, and all bytes `>= 0x80` — is
///   emitted as a `\xNN` two-digit lowercase hex escape.
///
/// The result is safe to embed in a quoted log line and unambiguously
/// identifies the exact bytes received.
#[must_use]
pub fn escape_sequence_for_log(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    // Worst case every byte becomes a 4-char `\xNN` escape.
    let mut out = String::with_capacity(bytes.len().saturating_mul(4));
    for &b in bytes {
        match b {
            b'\\' => out.push_str("\\\\"),
            b'"' => out.push_str("\\\""),
            0x20..=0x7E => out.push(b as char),
            _ => {
                out.push_str("\\x");
                // Two lowercase hex digits, no allocation.
                out.push(HEX[(b >> 4) as usize] as char);
                out.push(HEX[(b & 0x0f) as usize] as char);
            }
        }
    }
    out
}

/// Maximum number of sequence bytes rendered into a single log line or
/// diagnostic string by [`escape_sequence_for_log_bounded`] and
/// [`lossy_sequence_for_log_bounded`].
///
/// Sub-parsers no longer carry an 8 KB ring buffer, so they hand the bytes they
/// already accumulate to the logger. OSC 52 / iTerm2 image payloads can be
/// megabytes, so the rendered form keeps only the first and last
/// `LOG_SEQUENCE_MAX_BYTES / 2` bytes and notes how many were omitted.
pub const LOG_SEQUENCE_MAX_BYTES: usize = 256;

/// Split `bytes` for bounded logging: the whole slice when it fits in
/// [`LOG_SEQUENCE_MAX_BYTES`], otherwise its head, its tail, and the number of
/// bytes omitted between them.
const fn bounded_log_parts(bytes: &[u8]) -> (&[u8], &[u8], usize) {
    if bytes.len() <= LOG_SEQUENCE_MAX_BYTES {
        return (bytes, &[], 0);
    }
    let half = LOG_SEQUENCE_MAX_BYTES / 2;
    let (head, _) = bytes.split_at(half);
    let (_, tail) = bytes.split_at(bytes.len() - half);
    (head, tail, bytes.len() - LOG_SEQUENCE_MAX_BYTES)
}

/// Like [`escape_sequence_for_log`], but bounded to [`LOG_SEQUENCE_MAX_BYTES`].
///
/// A longer sequence is rendered as `<head>...[N bytes omitted]...<tail>`,
/// where head and tail are each escaped exactly as by
/// [`escape_sequence_for_log`].
#[must_use]
pub fn escape_sequence_for_log_bounded(bytes: &[u8]) -> String {
    let (head, tail, omitted) = bounded_log_parts(bytes);
    if omitted == 0 {
        return escape_sequence_for_log(head);
    }
    format!(
        "{}...[{omitted} bytes omitted]...{}",
        escape_sequence_for_log(head),
        escape_sequence_for_log(tail)
    )
}

/// Render `bytes` with lossy UTF-8 decoding, bounded like
/// [`escape_sequence_for_log_bounded`].
///
/// Bounded to [`LOG_SEQUENCE_MAX_BYTES`]. Used for human-readable
/// `recent='...'` diagnostics where the escaped form is unnecessary.
#[must_use]
pub fn lossy_sequence_for_log_bounded(bytes: &[u8]) -> String {
    let (head, tail, omitted) = bounded_log_parts(bytes);
    if omitted == 0 {
        return String::from_utf8_lossy(head).into_owned();
    }
    format!(
        "{}...[{omitted} bytes omitted]...{}",
        String::from_utf8_lossy(head),
        String::from_utf8_lossy(tail)
    )
}

/// A small helper trait that standardizes how parsers collect and present
/// the raw bytes of the *current* sequence they are parsing.
pub trait SequenceTraceable {
    fn seq_tracer(&mut self) -> &mut SequenceTracer;
    fn seq_tracer_ref(&self) -> &SequenceTracer;

    fn append_trace(&mut self, b: u8) {
        self.seq_tracer().push(b);
    }

    fn clear_trace(&mut self) {
        self.seq_tracer().clear();
    }

    fn current_trace_str(&self) -> String {
        self.seq_tracer_ref().as_str()
    }

    /// The current sequence trace rendered as a reconstruction-faithful,
    /// escaped string suitable for diagnostics (see [`escape_sequence_for_log`]).
    fn current_trace_escaped(&self) -> String {
        self.seq_tracer_ref().as_escaped()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        LOG_SEQUENCE_MAX_BYTES, SequenceTraceable, SequenceTracer, escape_sequence_for_log,
        escape_sequence_for_log_bounded, lossy_sequence_for_log_bounded,
    };

    /// Minimal `SequenceTraceable` host so the trait's default methods can be
    /// exercised directly (rather than only via real parsers).
    struct TraceHost {
        tracer: SequenceTracer,
    }

    impl SequenceTraceable for TraceHost {
        fn seq_tracer(&mut self) -> &mut SequenceTracer {
            &mut self.tracer
        }
        fn seq_tracer_ref(&self) -> &SequenceTracer {
            &self.tracer
        }
    }

    #[test]
    fn escape_printable_ascii_is_verbatim() {
        assert_eq!(
            escape_sequence_for_log(b"1337;SetUserVar"),
            "1337;SetUserVar"
        );
    }

    #[test]
    fn escape_backslash_is_doubled() {
        assert_eq!(escape_sequence_for_log(b"a\\b"), "a\\\\b");
    }

    #[test]
    fn escape_double_quote_is_escaped() {
        // Call sites embed the output inside a quoted log string, so a raw `"`
        // in the payload must be escaped to keep the surrounding quoting intact.
        assert_eq!(escape_sequence_for_log(b"a\"b"), "a\\\"b");
        // Mixed backslash + quote (e.g. a JSON-ish OSC payload).
        assert_eq!(escape_sequence_for_log(b"\\\""), "\\\\\\\"");
    }

    #[test]
    fn escape_control_bytes_as_hex() {
        // ESC, BEL, ST-final backslash handled as controls / doubled backslash.
        assert_eq!(escape_sequence_for_log(&[0x1b, b'[', b'm']), "\\x1b[m");
        assert_eq!(escape_sequence_for_log(&[0x07]), "\\x07");
        assert_eq!(escape_sequence_for_log(&[0x00, 0x1f]), "\\x00\\x1f");
    }

    #[test]
    fn escape_high_and_c1_bytes_as_hex() {
        // 8-bit CSI introducer (0x9b) and arbitrary high bytes.
        assert_eq!(
            escape_sequence_for_log(&[0x9b, 0xff, 0x80]),
            "\\x9b\\xff\\x80"
        );
    }

    #[test]
    fn escape_non_utf8_is_lossless() {
        // A byte sequence that is NOT valid UTF-8 must round-trip through the
        // escaper without information loss (no U+FFFD).
        let raw = &[b'A', 0xC3, 0x28, b'B'];
        let escaped = escape_sequence_for_log(raw);
        assert_eq!(escaped, "A\\xc3(B");
        assert!(!escaped.contains('\u{fffd}'));
    }

    #[test]
    fn tracer_as_escaped_matches_free_fn() {
        let mut tracer = SequenceTracer::new();
        for &b in &[0x1b, b'[', b'3', b'8', b';', b'2', b'm'] {
            tracer.push(b);
        }
        assert_eq!(tracer.as_escaped(), "\\x1b[38;2m");
        assert_eq!(
            tracer.as_escaped(),
            escape_sequence_for_log(&tracer.to_bytes())
        );
    }

    #[test]
    fn current_trace_escaped_renders_traced_bytes() {
        // Directly exercise the public trait method: it must render the current
        // trace using the same lossless escaping as `escape_sequence_for_log`,
        // including non-printable and non-UTF-8 bytes.
        let mut host = TraceHost {
            tracer: SequenceTracer::new(),
        };
        for &b in &[0x1b, b'[', b'3', b'8', b';', 0xff, b'"'] {
            host.append_trace(b);
        }
        assert_eq!(host.current_trace_escaped(), "\\x1b[38;\\xff\\\"");
        // Empty trace renders as an empty string.
        let empty = TraceHost {
            tracer: SequenceTracer::new(),
        };
        assert_eq!(empty.current_trace_escaped(), "");
    }

    #[test]
    fn tracer_to_bytes_is_lossless() {
        let mut tracer = SequenceTracer::new();
        for &b in &[0x9c, 0xff, b'x'] {
            tracer.push(b);
        }
        assert_eq!(tracer.to_bytes(), vec![0x9c, 0xff, b'x']);
    }

    #[test]
    fn new_tracer_is_empty() {
        let tracer = SequenceTracer::new();
        assert_eq!(tracer.as_str(), "");
    }

    #[test]
    fn default_tracer_is_empty() {
        let tracer = SequenceTracer::default();
        assert_eq!(tracer.as_str(), "");
    }

    #[test]
    fn push_and_as_str_basic() {
        let mut tracer = SequenceTracer::new();
        tracer.push(b'A');
        tracer.push(b'B');
        tracer.push(b'C');
        assert_eq!(tracer.as_str(), "ABC");
    }

    #[test]
    fn clear_resets_tracer() {
        let mut tracer = SequenceTracer::new();
        tracer.push(b'X');
        tracer.clear();
        assert_eq!(tracer.as_str(), "");
    }

    #[test]
    fn as_str_wraps_around_ring_buffer() {
        let mut tracer = SequenceTracer::new();
        // Fill more than the ring buffer capacity (8192 bytes)
        // to exercise the wraparound branch in as_str().
        // We push 8193 bytes: 8192 'A' bytes + 1 'B' byte.
        // After wrap, the buffer contains 8191 'A' + 1 'B' (the oldest 'A' is overwritten).
        for _ in 0..8192 {
            tracer.push(b'A');
        }
        // Now push one more byte to force wraparound
        tracer.push(b'B');
        let s = tracer.as_str();
        // The result is exactly 8192 bytes long (buffer capacity)
        assert_eq!(s.len(), 8192);
        // The last character should be 'B'
        assert!(s.ends_with('B'));
        // The remaining 8191 characters should all be 'A'
        assert!(s.chars().take(8191).all(|c| c == 'A'));
    }

    #[test]
    fn bounded_escape_short_input_matches_unbounded() {
        let raw = b"1337;File=inline=1\x1b";
        assert_eq!(
            escape_sequence_for_log_bounded(raw),
            escape_sequence_for_log(raw)
        );
        assert_eq!(escape_sequence_for_log_bounded(b""), "");
    }

    #[test]
    fn bounded_escape_at_limit_is_not_truncated() {
        let raw = vec![b'a'; LOG_SEQUENCE_MAX_BYTES];
        assert_eq!(escape_sequence_for_log_bounded(&raw).len(), raw.len());
    }

    #[test]
    fn bounded_escape_one_past_limit_omits_one_byte() {
        let mut raw = vec![b'H'; LOG_SEQUENCE_MAX_BYTES / 2];
        raw.push(b'M');
        raw.extend(vec![b'T'; LOG_SEQUENCE_MAX_BYTES / 2]);
        assert_eq!(raw.len(), LOG_SEQUENCE_MAX_BYTES + 1);
        assert_eq!(
            escape_sequence_for_log_bounded(&raw),
            format!(
                "{}...[1 bytes omitted]...{}",
                "H".repeat(LOG_SEQUENCE_MAX_BYTES / 2),
                "T".repeat(LOG_SEQUENCE_MAX_BYTES / 2)
            )
        );
    }

    #[test]
    fn bounded_escape_truncates_to_head_and_tail() {
        let mut raw = vec![b'H'; LOG_SEQUENCE_MAX_BYTES / 2];
        raw.extend(vec![b'M'; 5000]);
        raw.extend(vec![b'T'; LOG_SEQUENCE_MAX_BYTES / 2]);
        let rendered = escape_sequence_for_log_bounded(&raw);
        let omitted = raw.len() - LOG_SEQUENCE_MAX_BYTES;
        assert_eq!(
            rendered,
            format!(
                "{}...[{omitted} bytes omitted]...{}",
                "H".repeat(LOG_SEQUENCE_MAX_BYTES / 2),
                "T".repeat(LOG_SEQUENCE_MAX_BYTES / 2)
            )
        );
        assert!(!rendered.contains('M'));
    }

    #[test]
    fn bounded_lossy_matches_from_utf8_lossy_and_truncates() {
        assert_eq!(lossy_sequence_for_log_bounded(b"ab\xffc"), "ab\u{fffd}c");
        let raw = vec![b'x'; 10_000];
        let rendered = lossy_sequence_for_log_bounded(&raw);
        assert!(rendered.contains("[9744 bytes omitted]"));
        assert!(rendered.len() < 400);
    }
}
