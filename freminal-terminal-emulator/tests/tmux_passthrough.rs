// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! `TerminalState`-level tests for DCS tmux passthrough (`ESC P tmux; ... ST`).
//!
//! Since Task 130.2 a tmux payload is not dispatched by the handler any more.
//! The handler queues the whole un-doubled inner sequence, and `TerminalState`
//! runs it through a **fresh** instance of the real parser, in event order,
//! right after the DCS that carried it.  The central property is therefore
//! *equivalence*: an inner sequence has exactly the effect it would have had
//! if the application had sent it directly.  `inner_sequence_matches_direct_delivery`
//! pins that for every sequence the old direct-dispatch table handled and for
//! every shape that used to fall through to the reparse queue.
//!
//! Replies are never tmux-wrapped (tmux does not unwrap application-bound
//! passthrough), so every reply asserted here is the bare reply.

#![allow(clippy::unwrap_used)]

use crossbeam_channel::{Receiver, unbounded};
use freminal_common::{
    buffer_states::{modes::decckm::Decckm, window_manipulation::WindowManipulation},
    pty_write::PtyWrite,
};
use freminal_terminal_emulator::state::internal::TerminalState;

const WIDTH: usize = 80;
const HEIGHT: usize = 24;

// ─── helpers ────────────────────────────────────────────────────────────────

/// An 80x24 state with a live reply channel.
fn make_state() -> (TerminalState, Receiver<PtyWrite>) {
    let (tx, rx) = unbounded::<PtyWrite>();
    let mut state = TerminalState::new(tx, None);
    state.set_win_size(WIDTH, HEIGHT, 8, 16);
    (state, rx)
}

/// Drain every pending reply, in order.
fn replies(rx: &Receiver<PtyWrite>) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    while let Ok(msg) = rx.try_recv() {
        match msg {
            PtyWrite::Write(bytes) => out.push(bytes),
            PtyWrite::Resize(_) => panic!("unexpected PtyWrite::Resize"),
        }
    }
    out
}

/// Wrap `inner` exactly as tmux does: `ESC P tmux; <ESC-doubled inner> ESC \`.
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

/// Wrap `inner` in `levels` nested tmux envelopes.
fn tmux_nest(inner: &[u8], levels: usize) -> Vec<u8> {
    let mut bytes = inner.to_vec();
    for _ in 0..levels {
        bytes = tmux_wrap(&bytes);
    }
    bytes
}

/// The visible screen as one trimmed string per row.
fn screen_text(state: &TerminalState) -> Vec<String> {
    let buffer = state.handler.buffer();
    let visible = buffer.visible_rows(0);
    (0..HEIGHT)
        .map(|row_index| {
            let mut line = String::new();
            if let Some(row) = visible.get(row_index) {
                for col in 0..WIDTH {
                    let cell = row.resolve_cell(col);
                    if cell.is_continuation() {
                        continue;
                    }
                    line.push_str(&cell.tchar().to_string());
                }
            }
            line.trim_end().to_owned()
        })
        .collect()
}

/// Cursor position as 0-based `(x, y)`.
fn cursor(state: &TerminalState) -> (usize, usize) {
    let pos = state.handler.cursor_pos();
    (pos.x, pos.y)
}

/// Twelve distinctive rows, then the cursor at row 5, column 10 (1-based).
const SETUP_ROWS: usize = 12;

fn feed_setup(state: &mut TerminalState) {
    for i in 1..=SETUP_ROWS {
        state.handle_incoming_data(format!("row{i:02} 0123456789ABCDEFGHIJ\r\n").as_bytes());
    }
    state.handle_incoming_data(b"\x1b[5;10H");
}

/// Queries appended after every equivalence case.  They make hidden state
/// observable: the cursor (CPR), the SGR state and the scroll region.
const QUERIES: &[u8] = b"\x1b[6n\x1bP$qm\x1b\\\x1bP$qr\x1b\\";

/// Everything observable about a state after a scenario.
#[derive(Debug, PartialEq, Eq)]
struct Observed {
    screen: Vec<String>,
    cursor: (usize, usize),
    modes: String,
    replies: Vec<Vec<u8>>,
}

fn observe(state: &TerminalState, rx: &Receiver<PtyWrite>) -> Observed {
    Observed {
        screen: screen_text(state),
        cursor: cursor(state),
        modes: format!("{:?}", state.modes),
        replies: replies(rx),
    }
}

/// One equivalence scenario.
struct Case {
    label: &'static str,
    /// Fed directly after the common setup, before `inner`.
    pre: &'static [u8],
    /// The inner sequence under test.
    inner: &'static [u8],
    /// Fed directly after `inner`, before the common queries.
    post: &'static [u8],
}

const fn case(label: &'static str, inner: &'static [u8]) -> Case {
    Case {
        label,
        pre: b"",
        inner,
        post: b"",
    }
}

/// Run one case, delivering `inner` directly or wrapped in tmux.
fn run_case(case: &Case, wrapped: bool) -> Observed {
    let (mut state, rx) = make_state();
    feed_setup(&mut state);
    state.handle_incoming_data(case.pre);
    if wrapped {
        state.handle_incoming_data(&tmux_wrap(case.inner));
    } else {
        state.handle_incoming_data(case.inner);
    }
    state.handle_incoming_data(case.post);
    state.handle_incoming_data(QUERIES);
    observe(&state, &rx)
}

// ─── equivalence with direct delivery ───────────────────────────────────────

/// Every sequence the old `dispatch_tmux_csi` handled directly, and every
/// shape that used to fall through to the reparse queue.
fn equivalence_cases() -> Vec<Case> {
    vec![
        // ── the old direct-dispatch table ─────────────────────────────────
        case("CUP default", b"\x1b[H"),
        case("CUP row;col", b"\x1b[3;7H"),
        case("CUP row only", b"\x1b[3H"),
        case("CUP zero params", b"\x1b[0;0H"),
        case("HVP", b"\x1b[3;7f"),
        case("HVP default", b"\x1b[f"),
        case("CUU", b"\x1b[3A"),
        case("CUU default", b"\x1b[A"),
        case("CUD", b"\x1b[2B"),
        case("CUF", b"\x1b[5C"),
        case("CUB", b"\x1b[3D"),
        case("CNL", b"\x1b[2E"),
        case("CPL", b"\x1b[3F"),
        case("CHA", b"\x1b[20G"),
        case("HPA (backtick)", b"\x1b[30`"),
        case("VPA", b"\x1b[8d"),
        case("ED 0", b"\x1b[J"),
        case("ED 1", b"\x1b[1J"),
        case("ED 2", b"\x1b[2J"),
        case("ED 3", b"\x1b[3J"),
        case("ED unknown mode", b"\x1b[9J"),
        case("EL 0", b"\x1b[K"),
        case("EL 1", b"\x1b[1K"),
        case("EL 2", b"\x1b[2K"),
        case("EL unknown mode", b"\x1b[7K"),
        case("IL", b"\x1b[2L"),
        case("DL", b"\x1b[2M"),
        case("DCH", b"\x1b[3P"),
        case("ECH", b"\x1b[5X"),
        case("ICH", b"\x1b[2@"),
        case("SU", b"\x1b[3S"),
        case("SD", b"\x1b[2T"),
        case("DECSTBM", b"\x1b[5;20r"),
        case("DECSTBM default", b"\x1b[r"),
        Case {
            label: "SCOSC",
            pre: b"",
            inner: b"\x1b[s",
            post: b"\x1b[1;1H\x1b[u",
        },
        Case {
            label: "SCORC",
            pre: b"\x1b[s\x1b[1;1H",
            inner: b"\x1b[u",
            post: b"",
        },
        // ── used to fall through to the reparse queue ─────────────────────
        Case {
            label: "DEC private mode ?25l",
            pre: b"",
            inner: b"\x1b[?25l",
            post: b"\x1b[?25$p",
        },
        Case {
            label: "DEC private mode ?1h",
            pre: b"",
            inner: b"\x1b[?1h",
            post: b"\x1b[?1$p",
        },
        Case {
            label: "SGR",
            pre: b"",
            inner: b"\x1b[1;32m",
            post: b"Z",
        },
        case("DECRQM (intermediate)", b"\x1b[?1049$p"),
        case("intermediate mid-body", b"\x1b[1 ;2H"),
        case("'>' prefix is not SD", b"\x1b[>0T"),
        case("'<' prefix is not CUP", b"\x1b[<1H"),
        case("'=' prefix is not CUP", b"\x1b[=1H"),
        case("misplaced private marker", b"\x1b[1?2H"),
        case("'+' intermediate is not SD", b"\x1b[3+T"),
        case("embedded final byte", b"\x1b[1H2J"),
        case("C0 control in body", b"\x1b[1\x082H"),
        case("DEL in body", b"\x1b[1\x7f2H"),
        case("byte above 0x7e in body", b"\x1b[1\xc32H"),
        case("colon sub-parameter", b"\x1b[5:2H"),
        case("overflowing parameter", b"\x1b[99999999999999999999999;2H"),
        case("unknown terminator (DSR)", b"\x1b[6n"),
        case("invalid terminator byte", b"\x1b[1;2?"),
        case("plain text after CUP", b"\x1b[3;3Hhello"),
    ]
}

#[test]
fn inner_sequence_matches_direct_delivery() {
    for case in equivalence_cases() {
        let direct = run_case(&case, false);
        let wrapped = run_case(&case, true);
        assert_eq!(
            direct, wrapped,
            "case `{}`: tmux-wrapped delivery differs from direct delivery",
            case.label
        );
    }
}

// ─── the old direct-dispatch scenarios, with concrete expectations ──────────

#[test]
fn inner_cursor_motion_moves_the_cursor() {
    // The setup leaves the cursor at row 5, column 10 (1-based), i.e. (9, 4).
    let cases: &[(&[u8], (usize, usize))] = &[
        (b"\x1b[H", (0, 0)),
        (b"\x1b[5;10H", (9, 4)),
        (b"\x1b[3;7H", (6, 2)),
        (b"\x1b[3;7f", (6, 2)),
        (b"\x1b[3A", (9, 1)),
        (b"\x1b[2B", (9, 6)),
        (b"\x1b[5C", (14, 4)),
        (b"\x1b[3D", (6, 4)),
        (b"\x1b[2E", (0, 6)),
        (b"\x1b[3F", (0, 1)),
        (b"\x1b[20G", (19, 4)),
        (b"\x1b[30`", (29, 4)),
        (b"\x1b[8d", (9, 7)),
    ];
    for &(inner, expected) in cases {
        let (mut state, _rx) = make_state();
        feed_setup(&mut state);
        assert_eq!(cursor(&state), (9, 4), "setup precondition");
        state.handle_incoming_data(&tmux_wrap(inner));
        assert_eq!(
            cursor(&state),
            expected,
            "inner {:?} must move the cursor like a direct send",
            String::from_utf8_lossy(inner)
        );
    }
}

#[test]
fn inner_erase_display_clears_the_screen() {
    let (mut state, _rx) = make_state();
    feed_setup(&mut state);
    assert_eq!(screen_text(&state)[0], "row01 0123456789ABCDEFGHIJ");
    state.handle_incoming_data(&tmux_wrap(b"\x1b[2J"));
    assert!(
        screen_text(&state).iter().all(String::is_empty),
        "ED 2 inside tmux must clear every row"
    );
}

#[test]
fn inner_erase_line_truncates_the_row_from_the_cursor() {
    let (mut state, _rx) = make_state();
    feed_setup(&mut state);
    state.handle_incoming_data(&tmux_wrap(b"\x1b[0K"));
    let screen = screen_text(&state);
    // Cursor was at column 10 (index 9) of row 5.
    assert_eq!(screen[4], "row05 012");
    assert_eq!(
        screen[3], "row04 0123456789ABCDEFGHIJ",
        "other rows untouched"
    );
}

#[test]
fn inner_save_and_restore_cursor_round_trip() {
    let (mut state, _rx) = make_state();
    feed_setup(&mut state);
    state.handle_incoming_data(&tmux_wrap(b"\x1b[s"));
    state.handle_incoming_data(b"\x1b[1;1H");
    assert_eq!(cursor(&state), (0, 0));
    state.handle_incoming_data(&tmux_wrap(b"\x1b[u"));
    assert_eq!(cursor(&state), (9, 4));
}

#[test]
fn inner_decstbm_sets_the_scroll_region() {
    let (mut state, rx) = make_state();
    state.handle_incoming_data(&tmux_wrap(b"\x1b[5;20r"));
    state.handle_incoming_data(b"\x1bP$qr\x1b\\");
    assert_eq!(replies(&rx), vec![b"\x1bP1$r5;20r\x1b\\".to_vec()]);
}

// ─── inner sequences that used to be reparsed ───────────────────────────────

#[test]
fn inner_sgr_changes_the_current_rendition() {
    let (mut state, rx) = make_state();
    state.handle_incoming_data(&tmux_wrap(b"\x1b[1;31m"));
    state.handle_incoming_data(b"\x1bP$qm\x1b\\");
    assert_eq!(replies(&rx), vec![b"\x1bP1$r0;1;31m\x1b\\".to_vec()]);
}

#[test]
fn inner_mode_set_updates_terminal_state_flags() {
    // DECCKM is mirrored into `TerminalState::modes` by `sync_mode_flags`,
    // so this only works if nested outputs go through the full per-output
    // routine and not just the handler.
    let (mut state, _rx) = make_state();
    assert_eq!(state.modes.cursor_key, Decckm::Ansi);
    state.handle_incoming_data(&tmux_wrap(b"\x1b[?1h"));
    assert_eq!(state.modes.cursor_key, Decckm::Application);
    state.handle_incoming_data(&tmux_wrap(b"\x1b[?1l"));
    assert_eq!(state.modes.cursor_key, Decckm::Ansi);
}

#[test]
fn inner_ris_resets_terminal_state_modes() {
    let (mut state, _rx) = make_state();
    state.handle_incoming_data(b"\x1b[?1h");
    assert_eq!(state.modes.cursor_key, Decckm::Application);
    state.handle_incoming_data(&tmux_wrap(b"\x1bc"));
    assert_eq!(
        state.modes.cursor_key,
        Decckm::Ansi,
        "RIS inside tmux must apply the TerminalState reset too"
    );
}

#[test]
fn inner_osc_title_reaches_window_commands() {
    let (mut state, _rx) = make_state();
    state.handle_incoming_data(&tmux_wrap(b"\x1b]0;hello\x07"));
    assert!(
        state
            .window_commands
            .iter()
            .any(|c| matches!(c, WindowManipulation::SetTitleBarText(t) if t == "hello")),
        "expected SetTitleBarText(\"hello\"), got {:?}",
        state.window_commands
    );
}

// ─── replies are never tmux-wrapped ─────────────────────────────────────────

#[test]
fn inner_kitty_query_reply_is_unwrapped() {
    let (mut state, rx) = make_state();
    state.handle_incoming_data(&tmux_wrap(b"\x1b_Ga=q,f=24,i=31;AAAA\x1b\\"));
    assert_eq!(replies(&rx), vec![b"\x1b_Gi=31;OK\x1b\\".to_vec()]);
}

#[test]
fn inner_decrqss_reply_is_unwrapped() {
    let (mut state, rx) = make_state();
    state.handle_incoming_data(&tmux_wrap(b"\x1bP$qm\x1b\\"));
    assert_eq!(replies(&rx), vec![b"\x1bP1$r0m\x1b\\".to_vec()]);
}

#[test]
fn inner_da1_sits_in_order_between_two_dsr_replies() {
    let (mut state, rx) = make_state();
    let mut wire = b"\x1b[5n".to_vec();
    wire.extend_from_slice(&tmux_wrap(b"\x1b[c"));
    wire.extend_from_slice(b"\x1b[5n");
    state.handle_incoming_data(&wire);
    let got = replies(&rx);

    // The same three queries delivered directly give the same three replies.
    let (mut direct_state, direct_rx) = make_state();
    direct_state.handle_incoming_data(b"\x1b[5n\x1b[c\x1b[5n");
    let expected = replies(&direct_rx);

    assert_eq!(got.len(), 3, "expected DSR, DA1, DSR; got {got:?}");
    assert_eq!(got[0], b"\x1b[0n".to_vec());
    assert!(
        got[1].starts_with(b"\x1b[?") && got[1].ends_with(b"c"),
        "second reply must be DA1, got {:?}",
        String::from_utf8_lossy(&got[1])
    );
    assert_eq!(got[2], b"\x1b[0n".to_vec());
    assert_eq!(got, expected, "must equal direct delivery");
    for reply in &got {
        assert!(
            !reply.starts_with(b"\x1bPtmux;"),
            "replies must never be tmux-wrapped"
        );
    }
}

// ─── nesting ────────────────────────────────────────────────────────────────

const MARKER_CUP: &[u8] = b"\x1b[7;7H";

#[test]
fn nested_tmux_depth_two_works() {
    let (mut state, _rx) = make_state();
    state.handle_incoming_data(&tmux_nest(MARKER_CUP, 2));
    assert_eq!(cursor(&state), (6, 6));
}

#[test]
fn nested_tmux_depth_four_works() {
    let (mut state, _rx) = make_state();
    state.handle_incoming_data(&tmux_nest(MARKER_CUP, 4));
    assert_eq!(
        cursor(&state),
        (6, 6),
        "four nesting levels is within MAX_TMUX_PASSTHROUGH_DEPTH"
    );
}

#[test]
fn nested_tmux_depth_five_is_dropped() {
    let (mut state, rx) = make_state();
    state.handle_incoming_data(&tmux_nest(MARKER_CUP, 5));
    assert_eq!(cursor(&state), (0, 0), "a fifth level must have no effect");
    assert!(replies(&rx).is_empty());

    // The state is still healthy afterwards.
    state.handle_incoming_data(&tmux_nest(MARKER_CUP, 1));
    assert_eq!(cursor(&state), (6, 6));
}

// ─── event order ────────────────────────────────────────────────────────────

/// `a=T` (transmit and display) of a 1x1 RGB image occupying one cell.
const KITTY_TRANSMIT_AND_PLACE: &[u8] = b"\x1b_Ga=T,f=24,s=1,v=1,c=1,r=1,i=7,q=2;AAAA\x1b\\";
/// `a=p` (put) of the image transmitted above.
const KITTY_PUT: &[u8] = b"\x1b_Ga=p,c=1,r=1,i=7,q=2\x1b\\";

/// `(row, col)` of every visible cell that carries an image placement.
fn image_cells(state: &TerminalState) -> Vec<(usize, usize)> {
    state
        .handler
        .visible_image_placements(0)
        .iter()
        .enumerate()
        .filter_map(|(index, placement)| placement.as_ref().map(|_| (index / WIDTH, index % WIDTH)))
        .collect()
}

#[test]
fn cup_then_kitty_put_places_each_image_at_its_own_cursor_position() {
    // The scenario `dispatch_tmux_csi` existed for: each CUP must run before
    // the put that follows it, even though every item arrives in one read.
    let (mut state, _rx) = make_state();
    let mut wire = Vec::new();
    wire.extend_from_slice(&tmux_wrap(b"\x1b[H"));
    wire.extend_from_slice(&tmux_wrap(KITTY_TRANSMIT_AND_PLACE));
    wire.extend_from_slice(&tmux_wrap(b"\x1b[5;5H"));
    wire.extend_from_slice(&tmux_wrap(KITTY_PUT));
    state.handle_incoming_data(&wire);

    assert_eq!(
        image_cells(&state),
        vec![(0, 0), (4, 4)],
        "the two images must land at the two cursor positions"
    );
}

#[test]
fn tmux_payload_runs_at_its_position_among_surrounding_direct_output() {
    let (mut state, _rx) = make_state();
    // Direct text, a tmux CUP, direct text: the text after the CUP must land
    // at the new position, the text before it at the old one.
    state.handle_incoming_data(b"AAA");
    state.handle_incoming_data(&{
        let mut wire = Vec::new();
        wire.extend_from_slice(&tmux_wrap(b"\x1b[3;1H"));
        wire.extend_from_slice(b"BBB");
        wire
    });
    let screen = screen_text(&state);
    assert_eq!(screen[0], "AAA");
    assert_eq!(screen[2], "BBB");
}

#[test]
fn read_ending_mid_sequence_is_not_corrupted_by_a_tmux_payload() {
    let (mut state, _rx) = make_state();
    feed_setup(&mut state);
    assert_eq!(screen_text(&state)[0], "row01 0123456789ABCDEFGHIJ");

    // Read 1: a complete tmux OSC title, then the start of an outer
    // `CSI 1;1 H` cut off after the `1`.  The old code queued the OSC for a
    // reparse through the *outer* parser at the end of the batch, splicing
    // it into that parser's partial CSI: the title was lost and the CUP
    // corrupted.
    let mut read_one = tmux_wrap(b"\x1b]0;spliced\x07");
    read_one.extend_from_slice(b"\x1b[1");
    state.handle_incoming_data(&read_one);
    assert!(
        state
            .window_commands
            .iter()
            .any(|c| matches!(c, WindowManipulation::SetTitleBarText(t) if t == "spliced")),
        "the tmux OSC title must take effect, got {:?}",
        state.window_commands
    );

    // Read 2: the rest of the outer sequence.
    state.handle_incoming_data(b";1H");
    assert_eq!(
        cursor(&state),
        (0, 0),
        "the outer CUP must complete as row 1, column 1"
    );
    assert_eq!(
        screen_text(&state)[0],
        "row01 0123456789ABCDEFGHIJ",
        "no stray bytes may be printed"
    );
}

#[test]
fn tmux_payload_after_a_partial_outer_sequence_leaves_both_intact() {
    let (mut state, _rx) = make_state();
    // Read 1 ends inside an outer CSI; read 2 completes it and carries a tmux
    // payload.
    state.handle_incoming_data(b"\x1b[3");
    let mut read_two = b";4H".to_vec();
    read_two.extend_from_slice(&tmux_wrap(b"\x1b[2C"));
    state.handle_incoming_data(&read_two);
    // Outer CUP 3;4 -> (3, 2), then tmux CUF 2 -> (5, 2).
    assert_eq!(cursor(&state), (5, 2));
}

#[test]
fn incomplete_inner_sequence_is_discarded_and_does_not_join_the_outer_stream() {
    // The payload is only the start of a CUP (`ESC [ 5`).  The inner parser is
    // fresh per payload, so the fragment must die with the payload; if it
    // leaked into the outer parser, the direct `;1H` that follows in the same
    // buffer would complete it into `CUP 5;1` and `X` would land on row 5.
    let mut wire = tmux_wrap(b"\x1b[5");
    wire.extend_from_slice(b";1HX");

    let (mut wrapped, wrapped_rx) = make_state();
    wrapped.handle_incoming_data(&wire);

    // Reference: the payload absent, only the direct bytes.
    let (mut direct, direct_rx) = make_state();
    direct.handle_incoming_data(b";1HX");

    assert_eq!(
        screen_text(&wrapped)[0],
        ";1HX",
        "the direct bytes are plain text on row 1"
    );
    assert!(
        screen_text(&wrapped)[4].is_empty(),
        "no CUP may be formed across the payload boundary"
    );
    assert_eq!(screen_text(&wrapped), screen_text(&direct));
    assert_eq!(cursor(&wrapped), cursor(&direct));
    assert_eq!(cursor(&wrapped), (4, 0));
    assert_eq!(replies(&wrapped_rx), replies(&direct_rx));
}

// ─── malformed payloads ─────────────────────────────────────────────────────

#[test]
fn malformed_tmux_payloads_have_no_effect() {
    for dcs in [
        &b"\x1bPtmux;\x1b\\"[..],
        b"\x1bPtmux;junk\x1b\\",
        b"\x1bPtmux;\x1b\x1b\x1b\\",
    ] {
        let (mut state, rx) = make_state();
        feed_setup(&mut state);
        let before = (screen_text(&state), cursor(&state));
        state.handle_incoming_data(dcs);
        assert_eq!(
            (screen_text(&state), cursor(&state)),
            before,
            "malformed payload {dcs:?} must not change the screen"
        );
        assert!(replies(&rx).is_empty());
    }
}
