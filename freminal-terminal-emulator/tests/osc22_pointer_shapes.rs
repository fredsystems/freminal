// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! End-to-end tests for kitty OSC 22 pointer shapes (Task 139.4).
//!
//! Every test feeds raw bytes through `TerminalEmulator::handle_incoming_data`
//! (which wraps `TerminalState::handle_incoming_data`) and asserts only
//! observable state: the `pointer_shape` and `is_alternate_screen` fields of
//! the built snapshot, and the exact bytes written to the PTY.  Ops are
//! covered at unit level in `terminal_handler/osc_pointer.rs` and in the
//! parser; this suite pins how they compose with screen switching, resets,
//! tmux passthrough, S8C1T framing and reply ordering.

use crossbeam_channel::Receiver;
use freminal_common::buffer_states::pointer_shape::PointerShape;
use freminal_common::pty_write::PtyWrite;
use freminal_terminal_emulator::interface::TerminalEmulator;

const RIS: &[u8] = b"\x1bc";
const DECSTR: &[u8] = b"\x1b[!p";
const QUERY_CURRENT: &[u8] = b"\x1b]22;?__current__\x1b\\";

/// Which escape pair switches to and from the alternate screen.
#[derive(Clone, Copy, Debug)]
enum AltMode {
    /// `?47`: switch without clearing.
    Mode47,
    /// `?1047`: switch, clearing the alternate screen on leave.
    Mode1047,
    /// `?1049`: save the cursor, switch and clear.
    Mode1049,
}

impl AltMode {
    const ALL: [Self; 3] = [Self::Mode47, Self::Mode1047, Self::Mode1049];

    const fn enter(self) -> &'static [u8] {
        match self {
            Self::Mode47 => b"\x1b[?47h",
            Self::Mode1047 => b"\x1b[?1047h",
            Self::Mode1049 => b"\x1b[?1049h",
        }
    }

    const fn leave(self) -> &'static [u8] {
        match self {
            Self::Mode47 => b"\x1b[?47l",
            Self::Mode1047 => b"\x1b[?1047l",
            Self::Mode1049 => b"\x1b[?1049l",
        }
    }
}

/// Where the terminal is expected to be after a step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Screen {
    Primary,
    Alternate,
}

struct Rig {
    emu: TerminalEmulator,
    rx: Receiver<PtyWrite>,
}

impl Rig {
    fn new() -> Self {
        let (emu, rx) = TerminalEmulator::new_headless(None);
        Self { emu, rx }
    }

    /// Feed `bytes` in one read and return everything written to the PTY
    /// meanwhile, concatenated.
    fn feed(&mut self, bytes: &[u8]) -> Vec<u8> {
        self.emu.handle_incoming_data(bytes);
        self.rx
            .try_iter()
            .filter_map(|msg| match msg {
                PtyWrite::Write(bytes) => Some(bytes),
                PtyWrite::Resize(_) | PtyWrite::ApplyPendingResize => None,
            })
            .flatten()
            .collect()
    }

    fn shape(&mut self) -> PointerShape {
        self.emu.build_snapshot().pointer_shape
    }

    fn screen(&mut self) -> Screen {
        if self.emu.build_snapshot().is_alternate_screen {
            Screen::Alternate
        } else {
            Screen::Primary
        }
    }

    /// The reply to `OSC 22 ; ? __current__ ST`.
    fn current(&mut self) -> Vec<u8> {
        self.feed(QUERY_CURRENT)
    }
}

/// The bare reply `ESC ] 22 ; <list> ESC \`.
fn reply(list: &str) -> Vec<u8> {
    format!("\x1b]22;{list}\x1b\\").into_bytes()
}

/// Set a shape with `OSC 22 ; = <name> ST`.
fn set(name: &str) -> Vec<u8> {
    format!("\x1b]22;={name}\x1b\\").into_bytes()
}

/// Wrap `inner` as tmux does: `ESC P tmux; <ESC-doubled inner> ESC \`.
fn tmux_wrap(inner: &[u8]) -> Vec<u8> {
    let mut out = b"\x1bPtmux;".to_vec();
    for &b in inner {
        if b == 0x1b {
            out.push(0x1b);
        }
        out.push(b);
    }
    out.extend_from_slice(b"\x1b\\");
    out
}

// ── 1. Per-screen stacks ────────────────────────────────────────────────────

fn assert_screens_have_separate_stacks(mode: AltMode) {
    let mut rig = Rig::new();
    rig.feed(&set("crosshair"));
    assert_eq!(rig.shape(), PointerShape::Crosshair, "{mode:?}");

    rig.feed(mode.enter());
    assert_eq!(rig.screen(), Screen::Alternate, "{mode:?}");
    assert_eq!(
        rig.shape(),
        PointerShape::Default,
        "{mode:?}: the alternate stack starts empty"
    );
    assert_eq!(rig.current(), reply("0"), "{mode:?}");

    rig.feed(&set("wait"));
    assert_eq!(rig.shape(), PointerShape::Wait, "{mode:?}");
    assert_eq!(rig.current(), reply("wait"), "{mode:?}");

    rig.feed(mode.leave());
    assert_eq!(rig.screen(), Screen::Primary, "{mode:?}");
    assert_eq!(
        rig.shape(),
        PointerShape::Crosshair,
        "{mode:?}: the primary shape is back"
    );
    assert_eq!(rig.current(), reply("crosshair"), "{mode:?}");
}

#[test]
fn stacks_are_separate_across_mode_47() {
    assert_screens_have_separate_stacks(AltMode::Mode47);
}

#[test]
fn stacks_are_separate_across_mode_1047() {
    assert_screens_have_separate_stacks(AltMode::Mode1047);
}

#[test]
fn stacks_are_separate_across_mode_1049() {
    assert_screens_have_separate_stacks(AltMode::Mode1049);
}

#[test]
fn a_push_on_the_alternate_screen_does_not_disturb_the_primary_stack() {
    let mut rig = Rig::new();
    rig.feed(b"\x1b]22;>help\x1b\\\x1b]22;>text\x1b\\");
    rig.feed(AltMode::Mode1049.enter());
    rig.feed(b"\x1b]22;>wait\x1b\\\x1b]22;<\x1b\\\x1b]22;<\x1b\\");
    assert_eq!(rig.shape(), PointerShape::Default);
    rig.feed(AltMode::Mode1049.leave());
    assert_eq!(rig.shape(), PointerShape::Text);
    rig.feed(b"\x1b]22;<\x1b\\");
    assert_eq!(
        rig.shape(),
        PointerShape::Help,
        "the primary stack kept both entries"
    );
}

// ── 2. The alternate stack persists across sessions ─────────────────────────

fn assert_alternate_stack_persists(mode: AltMode) {
    let mut rig = Rig::new();
    rig.feed(&set("help"));
    rig.feed(mode.enter());
    rig.feed(&set("crosshair"));
    rig.feed(mode.leave());
    assert_eq!(rig.shape(), PointerShape::Help, "{mode:?}");
    assert_eq!(rig.current(), reply("help"), "{mode:?}");

    rig.feed(mode.enter());
    assert_eq!(rig.screen(), Screen::Alternate, "{mode:?}");
    assert_eq!(
        rig.shape(),
        PointerShape::Crosshair,
        "{mode:?}: the alternate stack survived the round trip"
    );
    assert_eq!(rig.current(), reply("crosshair"), "{mode:?}");

    rig.feed(mode.leave());
    assert_eq!(rig.current(), reply("help"), "{mode:?}");
}

#[test]
fn alternate_stack_persists_across_1049_sessions() {
    assert_alternate_stack_persists(AltMode::Mode1049);
}

#[test]
fn alternate_stack_persists_across_1047_sessions() {
    assert_alternate_stack_persists(AltMode::Mode1047);
}

#[test]
fn alternate_stack_persists_across_47_sessions() {
    assert_alternate_stack_persists(AltMode::Mode47);
}

// ── 3. RIS ──────────────────────────────────────────────────────────────────

#[test]
fn ris_from_the_alternate_screen_clears_both_stacks_and_returns_to_primary() {
    for mode in AltMode::ALL {
        let mut rig = Rig::new();
        rig.feed(&set("help"));
        rig.feed(mode.enter());
        rig.feed(&set("wait"));
        assert_eq!(rig.screen(), Screen::Alternate, "{mode:?}");

        rig.feed(RIS);

        assert_eq!(rig.screen(), Screen::Primary, "{mode:?}: RIS leaves alt");
        assert_eq!(rig.shape(), PointerShape::Default, "{mode:?}");
        assert_eq!(rig.current(), reply("0"), "{mode:?}: primary stack empty");

        rig.feed(mode.enter());
        assert_eq!(rig.screen(), Screen::Alternate, "{mode:?}");
        assert_eq!(rig.shape(), PointerShape::Default, "{mode:?}");
        assert_eq!(rig.current(), reply("0"), "{mode:?}: alternate stack empty");

        rig.feed(mode.leave());
        assert_eq!(rig.shape(), PointerShape::Default, "{mode:?}");
    }
}

// ── 4. DECSTR ───────────────────────────────────────────────────────────────

#[test]
fn decstr_on_the_alternate_screen_clears_both_stacks() {
    for mode in AltMode::ALL {
        let mut rig = Rig::new();
        rig.feed(&set("help"));
        rig.feed(mode.enter());
        rig.feed(&set("wait"));

        rig.feed(DECSTR);

        assert_eq!(
            rig.screen(),
            Screen::Alternate,
            "{mode:?}: DECSTR does not leave the alternate screen"
        );
        assert_eq!(rig.shape(), PointerShape::Default, "{mode:?}");
        assert_eq!(rig.current(), reply("0"), "{mode:?}: alternate stack empty");

        rig.feed(mode.leave());
        assert_eq!(rig.screen(), Screen::Primary, "{mode:?}");
        assert_eq!(
            rig.shape(),
            PointerShape::Default,
            "{mode:?}: the primary stack was cleared too"
        );
        assert_eq!(rig.current(), reply("0"), "{mode:?}: primary stack empty");
    }
}

// ── 5. tmux passthrough ─────────────────────────────────────────────────────

#[test]
fn a_set_inside_tmux_passthrough_takes_effect() {
    let mut rig = Rig::new();
    let written = rig.feed(&tmux_wrap(b"\x1b]22;=crosshair\x1b\\"));
    assert!(written.is_empty(), "a set writes nothing, got {written:?}");
    assert_eq!(rig.shape(), PointerShape::Crosshair);
    assert_eq!(rig.current(), reply("crosshair"));
}

#[test]
fn a_query_inside_tmux_passthrough_is_answered_unwrapped() {
    let mut rig = Rig::new();
    rig.feed(&set("wait"));
    let written = rig.feed(&tmux_wrap(QUERY_CURRENT));
    assert_eq!(
        written,
        reply("wait"),
        "the reply is a plain OSC 22, never a DCS tmux envelope"
    );
}

#[test]
fn tmux_passthrough_ops_hit_the_active_screen_stack() {
    let mut rig = Rig::new();
    rig.feed(&set("help"));
    rig.feed(AltMode::Mode1049.enter());
    rig.feed(&tmux_wrap(b"\x1b]22;>move\x1b\\"));
    assert_eq!(rig.shape(), PointerShape::Move);
    rig.feed(AltMode::Mode1049.leave());
    assert_eq!(rig.shape(), PointerShape::Help);
}

// ── 6. Mixed ops in one read ────────────────────────────────────────────────

#[test]
fn mixed_ops_in_a_single_read() {
    let mut rig = Rig::new();
    let written = rig
        .feed(b"\x1b]22;>wait\x1b\\\x1b]22;>help\x1b\\\x1b]22;<\x1b\\\x1b]22;?__current__\x1b\\");
    assert_eq!(written, reply("wait"));
    assert_eq!(rig.shape(), PointerShape::Wait);
}

// ── 7. Reply ordering ───────────────────────────────────────────────────────

#[test]
fn a_query_reply_arrives_in_stream_order_between_other_replies() {
    let mut rig = Rig::new();
    rig.feed(&set("text"));
    let written = rig.feed(b"\x1b[5n\x1b]22;?__current__\x1b\\\x1b[c");

    let dsr_ok = b"\x1b[0n";
    let osc = reply("text");
    assert!(
        written.starts_with(dsr_ok),
        "the DSR reply comes first, got {written:?}"
    );
    let rest = &written[dsr_ok.len()..];
    assert!(
        rest.starts_with(&osc),
        "the OSC 22 reply is second, got {rest:?}"
    );
    let tail = &rest[osc.len()..];
    assert!(
        tail.starts_with(b"\x1b[?"),
        "the DA1 reply is last, got {tail:?}"
    );
}

// ── 8. S8C1T framing ────────────────────────────────────────────────────────

#[test]
fn a_query_in_eight_bit_mode_is_framed_with_c1_controls() {
    let mut rig = Rig::new();
    rig.feed(b"\x1b G");
    rig.feed(&set("pointer"));
    assert_eq!(rig.current(), b"\x9d22;pointer\x9c".to_vec());
}

#[test]
fn seven_bit_mode_restores_escape_framing() {
    let mut rig = Rig::new();
    rig.feed(b"\x1b G");
    rig.feed(b"\x1b F");
    assert_eq!(rig.current(), reply("0"));
}
