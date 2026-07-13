// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Internal, lightweight ring buffer for capturing the most recent input bytes.
//! Kept fully internal (pub(crate)) and allocation-free on the hot path.

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

    /// Trim trailing control terminators (ESC, '\', BEL) from the end of the trace.
    pub(crate) const fn trim_control_tail(&mut self) {
        while self.len > 0 {
            let end_idx = if self.idx == 0 {
                self.buf.len() - 1
            } else {
                self.idx - 1
            };
            let c = self.buf[end_idx];
            if matches!(c, 0x1B | 0x5C | 0x07) {
                self.idx = end_idx;
                self.len -= 1;
            } else {
                break;
            }
        }
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
    use super::{SequenceTraceable, SequenceTracer, escape_sequence_for_log};

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
    fn trim_control_tail_removes_bel() {
        let mut tracer = SequenceTracer::new();
        tracer.push(b'A');
        tracer.push(b'B');
        tracer.push(0x07); // BEL
        tracer.trim_control_tail();
        assert_eq!(tracer.as_str(), "AB");
    }

    #[test]
    fn trim_control_tail_removes_esc_backslash() {
        let mut tracer = SequenceTracer::new();
        tracer.push(b'X');
        tracer.push(0x1b); // ESC
        tracer.push(0x5c); // '\'
        tracer.trim_control_tail();
        assert_eq!(tracer.as_str(), "X");
    }

    #[test]
    fn trim_control_tail_on_empty_is_safe() {
        let mut tracer = SequenceTracer::new();
        // Should not panic on empty
        tracer.trim_control_tail();
        assert_eq!(tracer.as_str(), "");
    }

    #[test]
    fn trim_control_tail_removes_multiple_trailing_controls() {
        let mut tracer = SequenceTracer::new();
        tracer.push(b'Z');
        tracer.push(0x07); // BEL
        tracer.push(0x1b); // ESC
        tracer.push(0x5c); // '\'
        tracer.trim_control_tail();
        // All control characters after 'Z' are stripped
        assert_eq!(tracer.as_str(), "Z");
    }

    #[test]
    fn trim_control_tail_stops_at_non_control() {
        let mut tracer = SequenceTracer::new();
        tracer.push(b'H');
        tracer.push(b'i');
        tracer.push(0x07); // BEL
        tracer.trim_control_tail();
        assert_eq!(tracer.as_str(), "Hi");
    }
}
