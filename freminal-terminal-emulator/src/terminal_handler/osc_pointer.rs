// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! OSC 22 pointer-shape control for [`TerminalHandler`].
//!
//! This module applies a parsed [`PointerShapeOp`] to the active screen's
//! pointer-shape stack and answers queries:
//!
//! - [`TerminalHandler::handle_osc_pointer_shape`] — entry point for the
//!   [`AnsiOscType::PointerShape`](freminal_common::buffer_states::osc::AnsiOscType::PointerShape)
//!   variant.
//!
//! Set, push and pop write nothing to the PTY.  A query always writes exactly
//! one reply, `OSC 22 ; <list> ST` (framed by
//! [`TerminalHandler::write_osc_response`], so it follows S8C1T and ends in ST
//! even for a BEL-terminated request): one comma-joined item per queried name,
//! in order.

use freminal_common::buffer_states::pointer_shape::{PointerShapeOp, PointerShapeQuery};

use super::{TerminalHandler, pointer_shape_stack::PointerShapeStack};

/// The reply item for `__default__` and `__grabbed__`.
///
/// freminal shows the default arrow over terminal content when no shape is set
/// and does not change the pointer while mouse reporting is on or a button is
/// held, so both are truthfully `default`.  (kitty reports its own configured
/// shapes.)
const DEFAULT_SHAPE_REPLY: &str = "default";

/// The reply item for an unsupported name, or a current shape that is unset.
const NONE_REPLY: &str = "0";

/// The reply item for a supported name.
const SUPPORTED_REPLY: &str = "1";

/// The reply item for one query entry.
fn query_item(query: PointerShapeQuery, stack: &PointerShapeStack) -> &'static str {
    match query {
        PointerShapeQuery::Supported => SUPPORTED_REPLY,
        PointerShapeQuery::Unsupported => NONE_REPLY,
        PointerShapeQuery::Current => stack.current().map_or(NONE_REPLY, |shape| shape.css_name()),
        PointerShapeQuery::Default | PointerShapeQuery::Grabbed => DEFAULT_SHAPE_REPLY,
    }
}

impl TerminalHandler {
    /// Apply one OSC 22 operation to the active screen's stack.
    ///
    /// - `Set`: each entry replaces the top entry, in order.  An empty list (a
    ///   set whose names were all unknown) changes nothing.
    /// - `Push`: each shape is pushed, in order.  An empty list changes nothing.
    /// - `Pop`: removes one entry.
    /// - `Query`: replies `22;<list>`.
    pub(super) fn handle_osc_pointer_shape(&mut self, op: &PointerShapeOp) {
        let kind = self.buffer.kind();
        match op {
            PointerShapeOp::Set(entries) => {
                let stack = self.pointer_shape_stack.get_mut(kind);
                for entry in entries {
                    stack.set(*entry);
                }
            }
            PointerShapeOp::Push(shapes) => {
                let stack = self.pointer_shape_stack.get_mut(kind);
                for shape in shapes {
                    stack.push(*shape);
                }
            }
            PointerShapeOp::Pop => self.pointer_shape_stack.get_mut(kind).pop(),
            PointerShapeOp::Query(items) => {
                let stack = self.pointer_shape_stack.get(kind);
                let list = items
                    .iter()
                    .map(|item| query_item(*item, stack))
                    .collect::<Vec<_>>()
                    .join(",");
                self.write_osc_response(&format!("22;{list}"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::state::internal::TerminalState;
    use freminal_common::buffer_states::pointer_shape::PointerShape;
    use freminal_common::pty_write::PtyWrite;

    /// A full terminal state and the receiver of everything it writes to the PTY.
    struct Rig {
        state: TerminalState,
        rx: crossbeam_channel::Receiver<PtyWrite>,
    }

    impl Rig {
        fn new() -> Self {
            let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
            Self {
                state: TerminalState::new(tx, None),
                rx,
            }
        }

        /// Feed `bytes` and return everything written to the PTY meanwhile.
        fn feed(&mut self, bytes: &[u8]) -> Vec<u8> {
            self.state.handle_incoming_data(bytes);
            self.rx
                .try_iter()
                .filter_map(|msg| match msg {
                    PtyWrite::Write(bytes) => Some(bytes),
                    PtyWrite::Resize(_) | PtyWrite::ApplyPendingResize => None,
                })
                .flatten()
                .collect()
        }

        fn shape(&self) -> PointerShape {
            self.state.handler.pointer_shape()
        }

        /// The reply to `OSC 22 ; ? <items> ST`.
        fn query(&mut self, items: &str) -> Vec<u8> {
            self.feed(format!("\x1b]22;?{items}\x1b\\").as_bytes())
        }
    }

    /// The bytes written when nothing is written.
    const NOTHING: &[u8] = &[];

    fn reply(list: &str) -> Vec<u8> {
        format!("\x1b]22;{list}\x1b\\").into_bytes()
    }

    #[test]
    fn current_on_an_empty_stack_is_zero() {
        let mut rig = Rig::new();
        assert_eq!(rig.query("__current__"), reply("0"));
    }

    #[test]
    fn support_query_reports_each_name() {
        let mut rig = Rig::new();
        assert_eq!(
            rig.query("pointer,crosshair,no-such-name,wait"),
            reply("1,1,0,1")
        );
    }

    #[test]
    fn default_and_grabbed_are_default() {
        let mut rig = Rig::new();
        assert_eq!(
            rig.query("__default__,__grabbed__"),
            reply("default,default")
        );
    }

    #[test]
    fn alias_query_is_supported() {
        let mut rig = Rig::new();
        assert_eq!(rig.query("left_ptr"), reply("1"));
    }

    #[test]
    fn empty_query_name_is_unsupported() {
        let mut rig = Rig::new();
        assert_eq!(rig.feed(b"\x1b]22;?\x1b\\"), reply("0"));
        assert_eq!(rig.query("text,,text"), reply("1,0,1"));
    }

    #[test]
    fn current_reports_the_canonical_name_of_an_alias() {
        let mut rig = Rig::new();
        assert_eq!(rig.feed(b"\x1b]22;=left_ptr\x1b\\"), NOTHING);
        assert_eq!(rig.query("__current__"), reply("default"));
        assert_eq!(rig.feed(b"\x1b]22;=col-resize\x1b\\"), NOTHING);
        assert_eq!(rig.query("__current__"), reply("ew-resize"));
    }

    #[test]
    fn push_then_current_reports_the_pushed_shape() {
        let mut rig = Rig::new();
        assert_eq!(rig.feed(b"\x1b]22;>fleur\x1b\\"), NOTHING);
        assert_eq!(rig.query("__current__"), reply("move"));
        assert_eq!(rig.shape(), PointerShape::Move);
    }

    #[test]
    fn seventeen_pushes_then_sixteen_pops_leave_an_empty_stack() {
        let mut rig = Rig::new();
        for _ in 0..17 {
            assert_eq!(rig.feed(b"\x1b]22;>help\x1b\\"), NOTHING);
        }
        for _ in 0..16 {
            assert_eq!(rig.feed(b"\x1b]22;<\x1b\\"), NOTHING);
        }
        assert_eq!(
            rig.query("__current__"),
            reply("0"),
            "the 17th push evicted the bottom entry, so 16 pops empty the stack"
        );
    }

    #[test]
    fn seventeenth_push_evicts_the_bottom() {
        let mut rig = Rig::new();
        rig.feed(b"\x1b]22;>text\x1b\\");
        for _ in 0..16 {
            rig.feed(b"\x1b]22;>help\x1b\\");
        }
        for _ in 0..15 {
            rig.feed(b"\x1b]22;<\x1b\\");
        }
        assert_eq!(rig.shape(), PointerShape::Help, "bottom `text` was evicted");
    }

    #[test]
    fn pop_on_an_empty_stack_is_harmless() {
        let mut rig = Rig::new();
        assert_eq!(rig.feed(b"\x1b]22;<\x1b\\"), NOTHING);
        assert_eq!(rig.query("__current__"), reply("0"));
    }

    #[test]
    fn empty_set_hides_and_pop_restores_the_lower_shape() {
        let mut rig = Rig::new();
        rig.feed(b"\x1b]22;>help\x1b\\");
        rig.feed(b"\x1b]22;>wait\x1b\\");
        rig.feed(b"\x1b]22;\x1b\\");
        assert_eq!(rig.query("__current__"), reply("0"));
        assert_eq!(rig.shape(), PointerShape::Default);
        rig.feed(b"\x1b]22;<\x1b\\");
        assert_eq!(rig.query("__current__"), reply("help"));
    }

    #[test]
    fn empty_set_on_a_single_entry_replaces_it() {
        let mut rig = Rig::new();
        rig.feed(b"\x1b]22;>help\x1b\\");
        rig.feed(b"\x1b]22;\x1b\\");
        assert_eq!(rig.query("__current__"), reply("0"));
        rig.feed(b"\x1b]22;<\x1b\\");
        assert_eq!(
            rig.query("__current__"),
            reply("0"),
            "the set replaced `help` rather than pushing above it"
        );
    }

    #[test]
    fn unknown_set_name_leaves_the_current_shape_alone() {
        let mut rig = Rig::new();
        rig.feed(b"\x1b]22;text\x1b\\");
        rig.feed(b"\x1b]22;banana\x1b\\");
        assert_eq!(rig.shape(), PointerShape::Text);
        assert_eq!(rig.query("__current__"), reply("text"));
    }

    #[test]
    fn set_list_last_valid_name_wins() {
        let mut rig = Rig::new();
        rig.feed(b"\x1b]22;text,banana,pointer\x1b\\");
        assert_eq!(rig.shape(), PointerShape::Pointer);
    }

    #[test]
    fn set_replaces_the_top_entry_without_pushing() {
        let mut rig = Rig::new();
        rig.feed(b"\x1b]22;>help\x1b\\");
        rig.feed(b"\x1b]22;=wait\x1b\\");
        assert_eq!(rig.shape(), PointerShape::Wait);
        rig.feed(b"\x1b]22;<\x1b\\");
        assert_eq!(
            rig.shape(),
            PointerShape::Default,
            "the set replaced `help`"
        );
    }

    #[test]
    fn bel_terminated_query_gets_an_st_reply() {
        let mut rig = Rig::new();
        assert_eq!(rig.feed(b"\x1b]22;?__current__\x07"), reply("0"));
    }

    #[test]
    fn eight_bit_controls_frame_the_reply() {
        let mut rig = Rig::new();
        rig.feed(b"\x1b G");
        assert_eq!(rig.query("__current__"), b"\x9d22;0\x9c".to_vec());
    }

    #[test]
    fn reply_precedes_a_following_da1() {
        let mut rig = Rig::new();
        let bytes = rig.feed(b"\x1b]22;?__current__\x1b\\\x1b[c");
        let osc = reply("0");
        assert!(bytes.starts_with(&osc), "got {bytes:?}");
        let rest = &bytes[osc.len()..];
        assert!(
            rest.starts_with(b"\x1b[?"),
            "the DA1 reply follows the OSC 22 reply, got {rest:?}"
        );
    }

    #[test]
    fn set_push_and_pop_write_nothing() {
        let mut rig = Rig::new();
        assert_eq!(rig.feed(b"\x1b]22;text\x1b\\"), NOTHING);
        assert_eq!(rig.feed(b"\x1b]22;=pointer\x1b\\"), NOTHING);
        assert_eq!(rig.feed(b"\x1b]22;>wait,help\x1b\\"), NOTHING);
        assert_eq!(rig.feed(b"\x1b]22;<\x1b\\"), NOTHING);
        assert_eq!(rig.feed(b"\x1b]22;\x1b\\"), NOTHING);
        assert_eq!(rig.feed(b"\x1b]22;>banana\x1b\\"), NOTHING);
    }

    #[test]
    fn unknown_push_name_pushes_nothing() {
        let mut rig = Rig::new();
        rig.feed(b"\x1b]22;text\x1b\\");
        rig.feed(b"\x1b]22;>banana\x1b\\");
        rig.feed(b"\x1b]22;<\x1b\\");
        assert_eq!(
            rig.shape(),
            PointerShape::Default,
            "the pop removed `text`: nothing was pushed"
        );
    }
}
