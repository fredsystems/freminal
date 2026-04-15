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
        let end = self.idx;
        let start = (self.idx + self.buf.len() - self.len) % self.buf.len();
        let mut out = Vec::with_capacity(self.len);
        if start < end {
            out.extend_from_slice(&self.buf[start..end]);
        } else {
            out.extend_from_slice(&self.buf[start..]);
            out.extend_from_slice(&self.buf[..end]);
        }
        String::from_utf8_lossy(&out).into_owned()
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
}

#[cfg(test)]
mod tests {
    use super::SequenceTracer;

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
