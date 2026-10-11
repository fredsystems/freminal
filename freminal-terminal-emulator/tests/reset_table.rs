// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! One test per behaviour the Task 131 reset table changed, for RIS (`ESC c`)
//! and DECSTR (`CSI ! p`), driven through raw bytes.
//!
//! The authoritative row list is the "131 Reset table" in
//! `Documents/PLAN_VERSION_130.md`.

use freminal_common::cursor::CursorVisualStyle;
use freminal_common::pty_write::PtyWrite;
use freminal_terminal_emulator::ansi::FreminalAnsiParser;
use freminal_terminal_emulator::state::internal::TerminalState;
use freminal_terminal_emulator::terminal_handler::TerminalHandler;

/// Raw 1x1 PNG, base64, used as an image payload.
const PNG_1X1_B64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

const RIS: &[u8] = b"\x1bc";
const DECSTR: &[u8] = b"\x1b[!p";

/// A full terminal pipeline plus the PTY channel it writes to.
struct Rig {
    state: TerminalState,
    rx: crossbeam_channel::Receiver<PtyWrite>,
}

impl Rig {
    fn new(width: usize, height: usize) -> Self {
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        let mut state = TerminalState::new(tx, None);
        state.set_win_size(width, height, 8, 16);
        let rig = Self { state, rx };
        rig.drain();
        rig
    }

    fn feed(&mut self, bytes: &[u8]) {
        self.state.handle_incoming_data(bytes);
    }

    /// Everything written to the PTY since the last drain.
    fn drain(&self) -> Vec<PtyWrite> {
        self.rx.try_iter().collect()
    }

    /// The concatenated `PtyWrite::Write` payloads since the last drain.
    fn drain_bytes(&self) -> Vec<u8> {
        self.drain()
            .into_iter()
            .filter_map(|msg| match msg {
                PtyWrite::Write(bytes) => Some(bytes),
                PtyWrite::Resize(_) => None,
            })
            .flatten()
            .collect()
    }

    /// Ask DECRQM about `mode` (`"4"` for ANSI, `"?45"` for DEC private) and
    /// return the reported status digit (`1` set, `2` reset).
    fn mode_status(&mut self, mode: &str) -> u8 {
        self.drain();
        self.feed(format!("\x1b[{mode}$p").as_bytes());
        let reply = self.drain_bytes();
        let text = String::from_utf8_lossy(&reply).into_owned();
        let status = text
            .strip_suffix("$y")
            .and_then(|head| head.rsplit(';').next())
            .and_then(|digits| digits.parse::<u8>().ok());
        status.unwrap_or_else(|| panic!("no DECRPM reply for {mode}: {text:?}"))
    }
}

/// A bare handler fed through the parser, for tests of handler-owned queues
/// that `TerminalState` drains at the end of a chunk.
struct HandlerRig {
    handler: TerminalHandler,
    parser: FreminalAnsiParser,
}

impl HandlerRig {
    fn new() -> Self {
        Self {
            handler: TerminalHandler::new(80, 24),
            parser: FreminalAnsiParser::new(),
        }
    }

    fn feed(&mut self, bytes: &[u8]) {
        let outputs = self.parser.push(bytes);
        self.handler.process_outputs(&outputs);
    }
}

// ── DECCOLM restore on RIS ──────────────────────────────────────────────────

#[test]
fn ris_without_deccolm_keeps_the_width_and_sends_no_resize() {
    let mut rig = Rig::new(100, 24);
    assert_eq!(rig.state.handler.buffer().terminal_width(), 100);

    rig.feed(RIS);

    assert_eq!(
        rig.state.handler.buffer().terminal_width(),
        100,
        "RIS must not force 80 columns when DECCOLM never changed the width"
    );
    assert!(
        rig.drain()
            .iter()
            .all(|msg| !matches!(msg, PtyWrite::Resize(_))),
        "RIS must not resize the PTY when DECCOLM never changed the width"
    );
}

#[test]
fn ris_after_deccolm_restores_the_pre_deccolm_width() {
    let mut rig = Rig::new(100, 24);
    rig.feed(b"\x1b[?40h\x1b[?3h");
    assert_eq!(rig.state.handler.buffer().terminal_width(), 132);
    rig.drain();

    rig.feed(RIS);

    assert_eq!(
        rig.state.handler.buffer().terminal_width(),
        100,
        "RIS must restore the width DECCOLM replaced"
    );
    assert!(
        rig.drain()
            .iter()
            .any(|msg| matches!(msg, PtyWrite::Resize(size) if size.width == 100)),
        "restoring the width must tell the PTY"
    );
}

// ── RIS resets ──────────────────────────────────────────────────────────────

/// The first three cells of row 0 after: IRM on, optionally a RIS, then
/// writing "AB", homing and writing "X". Replace mode gives "XB", insert mode
/// "XAB".
fn first_row_after_irm(reset: RisAfterIrm) -> String {
    let mut rig = Rig::new(80, 24);
    rig.feed(b"\x1b[4h");
    match reset {
        RisAfterIrm::Ris => rig.feed(RIS),
        RisAfterIrm::Nothing => {}
    }
    rig.feed(b"AB\x1b[1;1HX");
    let rows = rig.state.handler.buffer().visible_rows(0);
    let row = rows.first().expect("first visible row");
    (0..3)
        .map(|col| row.resolve_cell(col).tchar().to_string())
        .collect::<String>()
}

/// Whether a RIS follows the IRM change in [`first_row_after_irm`].
#[derive(Clone, Copy)]
enum RisAfterIrm {
    Nothing,
    Ris,
}

#[test]
fn ris_resets_insert_mode() {
    assert_eq!(
        first_row_after_irm(RisAfterIrm::Nothing),
        "XAB",
        "sanity: IRM inserts"
    );
    assert_eq!(
        first_row_after_irm(RisAfterIrm::Ris).trim_end(),
        "XB",
        "RIS must reset IRM to replace mode"
    );
}

#[test]
fn ris_resets_decnrcm() {
    let mut rig = Rig::new(80, 24);
    rig.feed(b"\x1b[?42h");
    assert_eq!(rig.mode_status("?42"), 1);
    rig.feed(RIS);
    assert_eq!(rig.mode_status("?42"), 2, "RIS must reset DECNRCM");
}

#[test]
fn ris_resets_reverse_wrap() {
    let mut rig = Rig::new(80, 24);
    rig.feed(b"\x1b[?45h");
    assert_eq!(rig.mode_status("?45"), 1);
    rig.feed(RIS);
    assert_eq!(rig.mode_status("?45"), 2, "RIS must reset ?45");
}

#[test]
fn ris_resets_extended_reverse_wrap() {
    let mut rig = Rig::new(80, 24);
    rig.feed(b"\x1b[?1045h");
    assert_eq!(rig.mode_status("?1045"), 1);
    rig.feed(RIS);
    assert_eq!(rig.mode_status("?1045"), 2, "RIS must reset ?1045");
}

#[test]
fn ris_resets_sixel_display_mode() {
    let mut rig = Rig::new(80, 24);
    rig.feed(b"\x1b[?80h");
    assert_eq!(rig.mode_status("?80"), 1);
    rig.feed(RIS);
    assert_eq!(rig.mode_status("?80"), 2, "RIS must reset DECSDM");
}

#[test]
fn ris_resets_private_color_registers() {
    let mut rig = Rig::new(80, 24);
    // ?1070 defaults to set (private registers); reset it first.
    rig.feed(b"\x1b[?1070l");
    assert_eq!(rig.mode_status("?1070"), 2);
    rig.feed(RIS);
    assert_eq!(
        rig.mode_status("?1070"),
        1,
        "RIS must restore private colour registers"
    );
}

#[test]
fn ris_resets_in_band_resize() {
    let mut rig = Rig::new(80, 24);
    rig.feed(b"\x1b[?2048h");
    assert_eq!(rig.mode_status("?2048"), 1);
    rig.feed(RIS);
    assert_eq!(rig.mode_status("?2048"), 2, "RIS must reset ?2048");
}

#[test]
fn ris_resets_s8c1t_so_replies_are_seven_bit() {
    let mut rig = Rig::new(80, 24);
    rig.feed(b"\x1b G");
    rig.feed(b"\x1b[c");
    let before = rig.drain_bytes();
    assert_eq!(
        before.first(),
        Some(&0x9b),
        "sanity: S8C1T replies in 8-bit"
    );

    rig.feed(RIS);
    rig.drain();
    rig.feed(b"\x1b[c");
    let after = rig.drain_bytes();

    assert!(
        after.starts_with(b"\x1b["),
        "after RIS the DA1 reply must be 7-bit, got {after:?}"
    );
}

#[test]
fn ris_abandons_an_in_flight_kitty_chunked_transfer() {
    let (head, tail) = PNG_1X1_B64.split_at(40);
    let mut rig = Rig::new(80, 24);

    // Control: with no RIS between the chunks the transfer completes and the
    // image is placed.
    rig.feed(format!("\x1b_Ga=T,f=100,q=2,m=1;{head}\x1b\\").as_bytes());
    rig.feed(format!("\x1b_Gm=0;{tail}\x1b\\").as_bytes());
    assert!(
        rig.state.handler.buffer().has_any_image_cell(),
        "sanity: the uninterrupted chunked transfer places an image"
    );

    let mut rig = Rig::new(80, 24);
    rig.feed(format!("\x1b_Ga=T,f=100,q=2,m=1;{head}\x1b\\").as_bytes());
    rig.feed(RIS);
    rig.feed(format!("\x1b_Gm=0;{tail}\x1b\\").as_bytes());
    assert!(
        !rig.state.handler.buffer().has_any_image_cell(),
        "a continuation chunk after RIS must not complete the abandoned transfer"
    );
}

#[test]
fn ris_abandons_an_in_flight_iterm2_multipart_transfer() {
    let osc = |body: &str| format!("\x1b]1337;{body}\x1b\\");
    let mut rig = Rig::new(80, 24);

    // Control: uninterrupted, the transfer places an image.
    rig.feed(osc("MultipartFile=inline=1").as_bytes());
    rig.feed(osc(&format!("FilePart={PNG_1X1_B64}")).as_bytes());
    rig.feed(osc("FileEnd").as_bytes());
    assert!(
        rig.state.handler.buffer().has_any_image_cell(),
        "sanity: the uninterrupted multipart transfer places an image"
    );

    let mut rig = Rig::new(80, 24);
    rig.feed(osc("MultipartFile=inline=1").as_bytes());
    rig.feed(osc(&format!("FilePart={PNG_1X1_B64}")).as_bytes());
    rig.feed(RIS);
    rig.feed(osc("FileEnd").as_bytes());
    assert!(
        !rig.state.handler.buffer().has_any_image_cell(),
        "FileEnd after RIS must not complete the abandoned transfer"
    );
}

// ── RIS keeps ───────────────────────────────────────────────────────────────

#[test]
fn ris_keeps_the_osc7_working_directory() {
    let mut rig = Rig::new(80, 24);
    rig.feed(b"\x1b]7;file://localhost/home/user/project\x1b\\");
    let before = rig
        .state
        .handler
        .current_working_directory()
        .map(str::to_owned);
    assert!(before.is_some(), "sanity: OSC 7 was recorded");

    rig.feed(RIS);

    assert_eq!(
        rig.state
            .handler
            .current_working_directory()
            .map(str::to_owned),
        before,
        "RIS must keep the cwd: it describes the process, not the screen"
    );
}

#[test]
fn ris_keeps_a_window_command_queued_earlier_in_the_same_chunk() {
    let mut rig = HandlerRig::new();
    rig.feed(b"\x1b]2;kept title\x07\x1bc");

    let commands = rig.handler.take_window_commands();
    assert!(
        commands
            .iter()
            .any(|cmd| format!("{cmd:?}").contains("kept title")),
        "an already-emitted title command must survive RIS, got {commands:?}"
    );
}

#[test]
fn ris_keeps_pending_command_events() {
    let mut rig = HandlerRig::new();
    // A complete OSC 133 prompt / command / output / finished cycle, then RIS.
    // The `freminal=1;fid=` properties are what make the block reportable.
    rig.feed(
        b"\x1b]133;A;freminal=1;fid=7\x07$ \x1b]133;B;freminal=1;fid=7\x07ls\r\n\
          \x1b]133;C;freminal=1;fid=7\x07out\r\n\x1b]133;D;0;freminal=1;fid=7\x07",
    );
    rig.feed(RIS);

    assert_eq!(
        rig.handler.drain_command_events().len(),
        1,
        "a finished command event already emitted must survive RIS"
    );
}

// ── DECSTR resets ───────────────────────────────────────────────────────────

#[test]
fn decstr_resets_modify_other_keys() {
    let mut rig = Rig::new(80, 24);
    rig.feed(b"\x1b[>4;2m");
    assert_eq!(rig.state.handler.modify_other_keys_level(), 2);
    rig.feed(DECSTR);
    assert_eq!(
        rig.state.handler.modify_other_keys_level(),
        0,
        "DECSTR must reset modifyOtherKeys"
    );
}

#[test]
fn decstr_resets_palette_overrides() {
    let mut rig = Rig::new(80, 24);
    let query = b"\x1b]4;1;?\x1b\\";
    rig.feed(query);
    let default_reply = rig.drain_bytes();
    assert!(!default_reply.is_empty(), "sanity: OSC 4 query is answered");

    rig.feed(b"\x1b]4;1;rgb:12/34/56\x1b\\");
    rig.feed(query);
    let overridden = rig.drain_bytes();
    assert_ne!(
        overridden, default_reply,
        "sanity: the override took effect"
    );

    rig.feed(DECSTR);
    rig.feed(query);
    assert_eq!(
        rig.drain_bytes(),
        default_reply,
        "DECSTR must reset OSC 4 palette overrides"
    );
}

#[test]
fn decstr_turns_autowrap_on() {
    let mut rig = Rig::new(80, 24);
    rig.feed(b"\x1b[?7l");
    assert_eq!(rig.mode_status("?7"), 2);
    rig.feed(DECSTR);
    assert_eq!(
        rig.mode_status("?7"),
        1,
        "DECSTR must leave DECAWM on (xterm, Ghostty, WezTerm, kitty)"
    );
}

#[test]
fn decstr_and_ris_restore_the_configured_cursor_style() {
    for reset in [DECSTR, RIS] {
        let mut rig = Rig::new(80, 24);
        rig.state
            .handler
            .set_configured_cursor_visual_style(CursorVisualStyle::VerticalLineCursorBlink);
        assert_eq!(
            rig.state.handler.cursor_visual_style(),
            CursorVisualStyle::VerticalLineCursorBlink,
            "the setter applies the style immediately"
        );

        // DECSCUSR 4: steady underline.
        rig.feed(b"\x1b[4 q");
        assert_eq!(
            rig.state.handler.cursor_visual_style(),
            CursorVisualStyle::UnderlineCursorSteady
        );

        rig.feed(reset);
        assert_eq!(
            rig.state.handler.cursor_visual_style(),
            CursorVisualStyle::VerticalLineCursorBlink,
            "reset {reset:?} must restore the configured style, not the compiled default"
        );
    }
}

#[test]
fn decstr_resets_reverse_wrap() {
    let mut rig = Rig::new(80, 24);
    rig.feed(b"\x1b[?45h\x1b[?1045h");
    assert_eq!(rig.mode_status("?45"), 1);
    assert_eq!(rig.mode_status("?1045"), 1);
    rig.feed(DECSTR);
    assert_eq!(rig.mode_status("?45"), 2, "DECSTR must reset ?45");
    assert_eq!(rig.mode_status("?1045"), 2, "DECSTR must reset ?1045");
}

#[test]
fn decstr_resets_the_pointer_shape() {
    let mut rig = Rig::new(80, 24);
    rig.feed(b"\x1b]22;text\x1b\\");
    assert_ne!(
        rig.state.handler.pointer_shape(),
        freminal_common::buffer_states::pointer_shape::PointerShape::Default,
        "sanity: OSC 22 changed the shape"
    );
    rig.feed(DECSTR);
    assert_eq!(
        rig.state.handler.pointer_shape(),
        freminal_common::buffer_states::pointer_shape::PointerShape::Default,
        "DECSTR must reset the pointer shape"
    );
}

// ── DECSTR and the per-screen DECSC slot ────────────────────────────────────

#[test]
fn decstr_on_the_alternate_screen_leaves_the_primary_saved_charset_alone() {
    let mut rig = Rig::new(80, 24);

    // Primary: designate DEC Special Graphics, then DECSC.
    rig.feed(b"\x1b(0\x1b7");
    // Alternate screen (?47, which does not save the cursor itself), DECSTR
    // there records "home, default charset" in the alternate's DECSC slot only.
    rig.feed(b"\x1b[?47h");
    rig.feed(DECSTR);
    rig.feed(b"\x1b[?47l");
    // Back on the primary, switch charset off and DECRC: the primary's own
    // saved charset (DEC Special Graphics) must come back.
    rig.feed(b"\x1b(B\x1b8");
    rig.feed(b"q");

    let handler = &rig.state.handler;
    let cell_text = handler
        .buffer()
        .visible_rows(0)
        .first()
        .map(|row| row.resolve_cell(0).tchar().to_string());
    assert_eq!(
        cell_text.as_deref(),
        Some("\u{2500}"),
        "DECRC on the primary must restore its own DEC Special Graphics slot"
    );
}

/// The DECRQM status digit for DECANM (`?2`) after feeding `bytes` as one
/// chunk, ending in the query.
///
/// Once the parser is in VT52 mode it no longer parses `CSI ? 2 $ p`, so the
/// whole sequence has to arrive in a single chunk, which the parser reads in
/// ANSI mode before any mode sync.
fn decanm_status_after(rig: &mut Rig, bytes: &[u8]) -> u8 {
    rig.drain();
    let mut chunk = bytes.to_vec();
    chunk.extend_from_slice(b"\x1b[?2$p");
    rig.feed(&chunk);
    let reply = rig.drain_bytes();
    let text = String::from_utf8_lossy(&reply).into_owned();
    text.strip_suffix("$y")
        .and_then(|head| head.rsplit(';').next())
        .and_then(|digits| digits.parse::<u8>().ok())
        .unwrap_or_else(|| panic!("no DECRPM reply for ?2: {text:?}"))
}

#[test]
fn ris_resets_the_handlers_decanm_mirror_to_ansi() {
    // DECRQM ?2 reports 2 (reset) while in VT52 mode and 1 (set) in ANSI mode.
    let mut rig = Rig::new(80, 24);
    assert_eq!(
        decanm_status_after(&mut rig, b"\x1b[?2l"),
        2,
        "sanity: DECANM reset means VT52"
    );
    let mut rig = Rig::new(80, 24);
    assert_eq!(
        decanm_status_after(&mut rig, b"\x1b[?2l\x1bc"),
        1,
        "RIS must return the handler's DECANM mirror to ANSI"
    );
}
