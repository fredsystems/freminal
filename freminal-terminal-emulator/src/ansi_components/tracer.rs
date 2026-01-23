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

    #[allow(dead_code)]
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
#[allow(dead_code)]
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
