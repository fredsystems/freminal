// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! OSC and APC sequence dispatch for [`TerminalHandler`].
//!
//! Handles OSC (Operating System Command) sequences, the OSC 133 (FTCS)
//! shell integration sub-protocol, and APC (Application Program Command)
//! sequences (used by the Kitty graphics protocol).

use std::sync::Arc;

use freminal_common::buffer_states::{
    ftcs::{FtcsMarker, FtcsState},
    kitty_graphics::{KittyParseError, parse_kitty_graphics},
    osc::{AnsiOscType, UrlResponse},
    url::Url,
    window_manipulation::{NotificationKind, WindowManipulation},
};
use freminal_common::host_capabilities::Osc99Support;

use crate::ansi_components::tracer::{
    escape_sequence_for_log_bounded, lossy_sequence_for_log_bounded,
};

use super::{TerminalHandler, shell_integration};

/// A payload-free, static description of a Kitty graphics parse failure, safe
/// for a warn-level log line. The `Display` of [`KittyParseError`] embeds
/// strings and bytes copied from the sequence, so it must not be logged at
/// warn.
const fn kitty_parse_error_kind(err: &KittyParseError) -> &'static str {
    match err {
        KittyParseError::NotKittyGraphics => "not a Kitty graphics command",
        KittyParseError::InvalidControlPair(_) => "invalid control pair",
        KittyParseError::UnknownAction(_) => "unknown action",
        KittyParseError::UnknownFormat(_) => "unknown format",
        KittyParseError::UnknownTransmission(_) => "unknown transmission",
        KittyParseError::UnknownDeleteTarget(_) => "unknown delete target",
        KittyParseError::InvalidInteger(_) => "invalid integer",
        KittyParseError::UnknownCompression(_) => "unknown compression",
        KittyParseError::TooManyControlItems { .. } => "too many control data items",
    }
}

impl TerminalHandler {
    /// Handle an APC (Application Program Command) sequence.
    ///
    /// Attempts to parse the data as a Kitty graphics command (`_G...`).
    /// If it is not a Kitty graphics command, logs and ignores.
    pub fn handle_application_program_command(&mut self, apc: &[u8]) {
        match parse_kitty_graphics(apc) {
            Ok(cmd) => self.handle_kitty_graphics(cmd),
            Err(KittyParseError::NotKittyGraphics) => {
                tracing::warn!(
                    "APC received (not Kitty graphics, ignored): {} bytes",
                    apc.len()
                );
                tracing::debug!(
                    "APC received (not Kitty graphics, ignored); raw sequence: \"{}\"",
                    escape_sequence_for_log_bounded(apc)
                );
            }
            Err(e) => {
                tracing::warn!("Kitty graphics parse error: {}", kitty_parse_error_kind(&e));
                tracing::debug!(
                    "Kitty graphics parse error: {}",
                    lossy_sequence_for_log_bounded(e.to_string().as_bytes())
                );
            }
        }
    }

    /// Handle an OSC (Operating System Command) sequence.
    ///
    /// Ports the logic from `TerminalState::osc_response` in the old buffer.
    // Inherently large: exhaustive match over all `AnsiOscType` variants. Each arm is a
    // tightly-coupled, single-line-or-few-lines dispatch. Splitting would require passing the
    // full handler context to sub-functions without any reduction in complexity.
    #[allow(clippy::too_many_lines)]
    pub fn handle_osc(&mut self, osc: &AnsiOscType) {
        match osc {
            // Hyperlink: OSC 8 ; params ; url ST  (start) / OSC 8 ; ; ST  (end)
            AnsiOscType::Url(UrlResponse::Url(url)) => {
                self.current_format.url = Some(Arc::new(Url {
                    id: url.id.clone(),
                    url: url.url.clone(),
                }));
                self.buffer.set_format(self.current_format.clone());
            }
            AnsiOscType::Url(UrlResponse::End) => {
                self.current_format.url = None;
                self.buffer.set_format(self.current_format.clone());
            }

            // Window title
            AnsiOscType::SetTitleBar(title) => {
                self.window_commands
                    .push(WindowManipulation::SetTitleBarText(title.clone()));
            }

            // OSC 10/11/12 foreground/background/cursor color query, set, and reset.
            AnsiOscType::RequestColorQueryBackground(_)
            | AnsiOscType::RequestColorQueryForeground(_)
            | AnsiOscType::RequestColorQueryCursor(_)
            | AnsiOscType::ResetForegroundColor
            | AnsiOscType::ResetBackgroundColor
            | AnsiOscType::ResetCursorColor => {
                self.handle_osc_fg_bg_color(osc);
            }

            // Remote host / CWD: OSC 7 ; file://hostname/path ST
            AnsiOscType::RemoteHost(value) => {
                self.current_working_directory = shell_integration::parse_osc7_uri(value);
                if self.current_working_directory.is_none() {
                    tracing::warn!("OSC 7: failed to parse URI");
                    tracing::debug!(
                        "OSC 7: failed to parse URI: {}",
                        lossy_sequence_for_log_bounded(value.as_bytes())
                    );
                } else if let Some(cwd) = &self.current_working_directory {
                    tracing::debug!(
                        "OSC 7: CWD set to {:?}",
                        lossy_sequence_for_log_bounded(cwd.as_bytes())
                    );
                }
            }
            AnsiOscType::ShellInfoHistFile(path) => {
                tracing::debug!(
                    "OSC 1338: HISTFILE set to {:?}",
                    lossy_sequence_for_log_bounded(path.to_string_lossy().as_bytes())
                );
                self.shell_histfile = Some(path.clone());
            }
            AnsiOscType::Ftcs(marker) => {
                self.handle_osc_ftcs(marker);
            }
            AnsiOscType::ITerm2FileInline(data) => {
                self.handle_iterm2_inline_image(data);
            }
            AnsiOscType::ITerm2MultipartBegin(data) => {
                self.handle_iterm2_multipart_begin(data);
            }
            AnsiOscType::ITerm2FilePart(bytes) => {
                self.handle_iterm2_file_part(bytes);
            }
            AnsiOscType::ITerm2FileEnd => {
                self.handle_iterm2_file_end();
            }
            AnsiOscType::ITerm2Unknown => {
                tracing::warn!("OSC 1337: unrecognised sub-command (ignored)");
            }

            // Clipboard: forward to GUI via window_commands
            AnsiOscType::SetClipboard(sel, content) => {
                self.window_commands.push(WindowManipulation::SetClipboard(
                    sel.clone(),
                    content.clone(),
                ));
            }
            AnsiOscType::QueryClipboard(sel) => {
                self.window_commands
                    .push(WindowManipulation::QueryClipboard(sel.clone()));
            }

            // Palette manipulation: OSC 4 (set/query) and OSC 104 (reset)
            AnsiOscType::SetPaletteColor(idx, r, g, b) => {
                self.palette.set(*idx, *r, *g, *b);
            }
            AnsiOscType::QueryPaletteColor(idx) => {
                let (r, g, b) = self.palette.rgb(*idx, self.theme);
                let body = format!(
                    "4;{idx};rgb:{:04x}/{:04x}/{:04x}",
                    u16::from(r) * 257,
                    u16::from(g) * 257,
                    u16::from(b) * 257,
                );
                self.write_osc_response(&body);
            }
            AnsiOscType::ResetPaletteColor(Some(idx)) => {
                self.palette.reset(*idx);
            }
            AnsiOscType::ResetPaletteColor(None) => {
                self.palette.reset_all();
            }

            // OSC 22 — set pointer (mouse cursor) shape.
            AnsiOscType::SetPointerShape(shape) => {
                self.pointer_shape = *shape;
            }

            // OSC 9;4 — ConEmu-style progress-state update (issue #507).
            //
            // A direct field mutation, not a `window_commands` push:
            // `window_commands` is a fire-once event queue and the wrong
            // transport for this level-triggered state, which is read back
            // out of the handler on every snapshot via `progress()` instead.
            AnsiOscType::Progress(update) => {
                self.progress.apply(*update);
                self.progress_updated_at = Some(std::time::Instant::now());
            }

            // OSC 9 / OSC 777 — desktop notification.  Forward to the GUI via
            // the window-command channel; the GUI's notification router
            // (Task 76.4) applies the `[notifications]` routing policy.
            AnsiOscType::Notify {
                source,
                title,
                body,
            } => {
                self.window_commands.push(WindowManipulation::Notification {
                    kind: NotificationKind::OscText,
                    source: *source,
                    title: title.clone(),
                    body: body.clone(),
                });
            }

            // OSC 99 stateful notification (Task 99).
            // - while OSC 99 is unsupported EVERY chunk (display payloads,
            //   `p=close`, `p=alive`, `p=?`) is dropped BEFORE reassembly, so the
            //   terminal looks like one that does not speak the protocol and no
            //   state accumulates for a transfer that could complete after the
            //   host later enables the protocol;
            // - while supported, each chunk is fed into the reassembly machine and
            //   on finalize `dispatch_finalized_osc99` acts on it: `p=?` is answered
            //   here from the host capabilities (Task 130.5) and never reaches the
            //   GUI, `p=alive` / `p=close` are forwarded as `Osc99Control`, and
            //   display payloads (title/body/icon/buttons) push Notification99
            //   (Task 99.4).
            AnsiOscType::Notify99(cmd) => {
                if matches!(self.host_capabilities().osc99, Osc99Support::Unsupported) {
                    tracing::debug!("OSC 99 request dropped: OSC 99 is unsupported");
                } else if let Some(finalized) = self.reassemble_osc99(cmd.clone()) {
                    self.dispatch_finalized_osc99(finalized);
                }
            }

            AnsiOscType::NoOp => {}
        }
    }

    /// Handle an OSC 133 (FTCS) shell integration marker.
    ///
    /// The `A`/`B`/`C`/`D` markers reach this function only when they carry
    /// `freminal=1` and a `fid` (parsed upstream by [`parse_ftcs_params`]);
    /// foreign `A`/`B`/`C`/`D` markers (`WezTerm`, Starship, `iTerm2`) are
    /// filtered out at the parse layer and never arrive here.  The `P`
    /// (`PromptProperty`) marker is the exception: it is freminal-independent
    /// (accepted from any emitter, no `freminal=1`/`fid` required), so it
    /// *does* reach this function — where it is handled as an informational
    /// no-op (see the `PromptProperty` arm below).
    pub(super) fn handle_osc_ftcs(&mut self, marker: &FtcsMarker) {
        tracing::debug!(
            "OSC 133 FTCS marker: {}",
            lossy_sequence_for_log_bounded(marker.to_string().as_bytes())
        );
        match marker {
            FtcsMarker::PromptStart { fid } => {
                self.ftcs_state = FtcsState::InPrompt;
                // mark_prompt_row() powers PrevCommand/NextCommand navigation
                // and must stay. start_command_block() is a sibling that
                // opens the new CommandBlock storage introduced in 72.2/72.3.
                self.buffer.mark_prompt_row();
                let cwd = self.current_working_directory().map(str::to_owned);
                let _id = self.buffer.start_command_block(cwd, fid.clone());
            }
            FtcsMarker::CommandStart { fid } => {
                self.ftcs_state = FtcsState::InCommand;
                self.buffer.mark_command_start_row(fid);
            }
            FtcsMarker::OutputStart { fid } => {
                self.ftcs_state = FtcsState::InOutput;
                self.buffer.mark_output_start_row(fid);
            }
            FtcsMarker::CommandFinished { exit_code, fid } => {
                self.last_exit_code = *exit_code;
                self.ftcs_state = FtcsState::None;
                if let Some(block) = self.buffer.finish_command_block(*exit_code, fid) {
                    self.pending_command_events.push(block);
                }
            }
            FtcsMarker::PromptProperty(_kind) => {
                // Prompt property is informational metadata — it annotates
                // the type of the next prompt (initial, continuation, right)
                // but does not change the FTCS state machine.
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::TerminalHandler;
    use crate::log_capture::{Captured, capture, warnings};
    use crate::state::internal::TerminalState;
    use freminal_common::buffer_states::osc::{AnsiOscType, OscNotifySource};
    use freminal_common::buffer_states::osc_notify_99::{
        NotificationOccasion, NotificationUrgency, Osc99Actions, Osc99Command,
        Osc99PayloadEncoding, Osc99PayloadType,
    };
    use freminal_common::buffer_states::terminal_output::TerminalOutput;
    use freminal_common::buffer_states::window_manipulation::{
        NotificationKind, Osc99ControlKind, WindowManipulation,
    };
    use freminal_common::host_capabilities::{
        HostCapabilities, Osc99ActivationReport, Osc99CloseEvents, Osc99Features, Osc99Support,
    };
    use freminal_common::pty_write::PtyWrite;

    // ── Existing OSC 9/777 tests ──────────────────────────────────────────────

    #[test]
    fn osc_notify_pushes_window_command() {
        let mut handler = TerminalHandler::new(80, 24);
        assert!(
            handler.window_commands.is_empty(),
            "no window commands initially"
        );

        handler.process_outputs(&[TerminalOutput::OscResponse(AnsiOscType::Notify {
            source: OscNotifySource::Osc777,
            title: Some("Build".to_owned()),
            body: "done".to_owned(),
        })]);

        assert_eq!(handler.window_commands.len(), 1);
        match &handler.window_commands[0] {
            WindowManipulation::Notification {
                kind,
                source,
                title,
                body,
            } => {
                assert_eq!(*kind, NotificationKind::OscText);
                assert_eq!(*source, OscNotifySource::Osc777);
                assert_eq!(title.as_deref(), Some("Build"));
                assert_eq!(body, "done");
            }
            other => panic!("expected Notification, got: {other:?}"),
        }
    }

    #[test]
    fn osc_notify_without_title_pushes_window_command() {
        let mut handler = TerminalHandler::new(80, 24);

        handler.process_outputs(&[TerminalOutput::OscResponse(AnsiOscType::Notify {
            source: OscNotifySource::Osc9,
            title: None,
            body: "hello".to_owned(),
        })]);

        assert_eq!(handler.window_commands.len(), 1);
        match &handler.window_commands[0] {
            WindowManipulation::Notification {
                kind,
                source,
                title,
                body,
            } => {
                assert_eq!(*kind, NotificationKind::OscText);
                assert_eq!(*source, OscNotifySource::Osc9);
                assert_eq!(*title, None);
                assert_eq!(body, "hello");
            }
            other => panic!("expected Notification, got: {other:?}"),
        }
    }

    // ── OSC 99 reassembly tests ───────────────────────────────────────────────

    /// Build a default `Osc99Command` for use in tests.  All fields are set to
    /// protocol defaults so individual tests only need to override the fields
    /// they care about.
    fn default_osc99() -> Osc99Command {
        Osc99Command {
            id: None,
            payload_type: Osc99PayloadType::Title,
            done: true,
            payload: Vec::new(),
            payload_encoding: Osc99PayloadEncoding::Plain,
            actions: Osc99Actions::default(),
            close_report: false,
            app_name: None,
            icon_cache_key: None,
            icon_names: Vec::new(),
            occasion: NotificationOccasion::Always,
            sound: None,
            notification_type: Vec::new(),
            urgency: None,
            expire_ms: -1,
        }
    }

    /// Single-chunk `done` title with no id → finalizes immediately.
    #[test]
    fn single_chunk_done_title_no_id() {
        let mut handler = TerminalHandler::new(80, 24);
        let cmd = Osc99Command {
            payload: b"Hi".to_vec(),
            ..default_osc99()
        };
        let result = handler.reassemble_osc99(cmd);
        let finalized = result.expect("single done chunk must finalize");
        assert_eq!(finalized.title, Some("Hi".to_owned()));
        assert_eq!(finalized.body, None);
        assert_eq!(finalized.icon, None);
        assert!(finalized.meta.id.is_none());
    }

    /// Three chunks with the same id: two title chunks (`d=0`) then a body
    /// chunk (`d=1`). Expect the two title chunks concatenated in `title` and
    /// the body in `body`.
    #[test]
    fn chunked_title_plus_body_by_id() {
        let mut handler = TerminalHandler::new(80, 24);

        // Chunk 1: id=n1, Title, d=false, payload "He"
        let chunk1 = Osc99Command {
            id: Some("n1".to_owned()),
            payload_type: Osc99PayloadType::Title,
            done: false,
            payload: b"He".to_vec(),
            ..default_osc99()
        };
        let r1 = handler.reassemble_osc99(chunk1);
        assert!(r1.is_none(), "first chunk must not finalize");

        // Chunk 2: id=n1, Title, d=false, payload "llo"
        let chunk2 = Osc99Command {
            id: Some("n1".to_owned()),
            payload_type: Osc99PayloadType::Title,
            done: false,
            payload: b"llo".to_vec(),
            ..default_osc99()
        };
        let r2 = handler.reassemble_osc99(chunk2);
        assert!(r2.is_none(), "second chunk must not finalize");

        // Chunk 3: id=n1, Body, d=true, payload "World"
        let chunk3 = Osc99Command {
            id: Some("n1".to_owned()),
            payload_type: Osc99PayloadType::Body,
            done: true,
            payload: b"World".to_vec(),
            ..default_osc99()
        };
        let r3 = handler.reassemble_osc99(chunk3);
        let finalized = r3.expect("terminating chunk must finalize");
        assert_eq!(finalized.title, Some("Hello".to_owned()));
        assert_eq!(finalized.body, Some("World".to_owned()));
        assert_eq!(finalized.icon, None);
        assert_eq!(finalized.meta.id, Some("n1".to_owned()));
    }

    /// After finalizing id=n1, a new chunk with the same id starts fresh.
    #[test]
    fn update_by_id_after_finalize_starts_fresh() {
        let mut handler = TerminalHandler::new(80, 24);

        // First notification: id=n1, done immediately.
        let first = Osc99Command {
            id: Some("n1".to_owned()),
            payload: b"First".to_vec(),
            ..default_osc99()
        };
        let r1 = handler.reassemble_osc99(first);
        assert!(r1.is_some(), "first n1 must finalize");

        // Second notification: same id, new content.
        let second = Osc99Command {
            id: Some("n1".to_owned()),
            payload: b"Second".to_vec(),
            ..default_osc99()
        };
        let r2 = handler.reassemble_osc99(second);
        let finalized = r2.expect("second n1 must finalize (fresh start)");
        // Must not carry stale bytes from the first notification.
        assert_eq!(finalized.title, Some("Second".to_owned()));
    }

    /// Unidentified non-final chunk → dropped, map stays empty.
    #[test]
    fn unidentified_non_final_never_merged() {
        let mut handler = TerminalHandler::new(80, 24);
        let cmd = Osc99Command {
            id: None,
            done: false,
            payload: b"ignored".to_vec(),
            ..default_osc99()
        };
        let result = handler.reassemble_osc99(cmd);
        assert!(result.is_none(), "non-final, no-id chunk must return None");
        assert!(
            handler.pending_notifications.is_empty(),
            "no map entry must be created for a no-id non-final chunk"
        );
    }

    /// Two interleaved ids must not cross-contaminate each other's payloads.
    #[test]
    fn two_interleaved_ids_no_cross_contamination() {
        let mut handler = TerminalHandler::new(80, 24);

        // id=a: non-final title chunk.
        let a1 = Osc99Command {
            id: Some("a".to_owned()),
            payload_type: Osc99PayloadType::Title,
            done: false,
            payload: b"AAA".to_vec(),
            ..default_osc99()
        };
        assert!(handler.reassemble_osc99(a1).is_none());

        // id=b: non-final title chunk.
        let b1 = Osc99Command {
            id: Some("b".to_owned()),
            payload_type: Osc99PayloadType::Title,
            done: false,
            payload: b"BBB".to_vec(),
            ..default_osc99()
        };
        assert!(handler.reassemble_osc99(b1).is_none());

        // Finalize id=a.
        let a2 = Osc99Command {
            id: Some("a".to_owned()),
            payload_type: Osc99PayloadType::Title,
            done: true,
            payload: b"aaa".to_vec(),
            ..default_osc99()
        };
        let fa = handler.reassemble_osc99(a2).expect("a must finalize");
        assert_eq!(fa.title, Some("AAAaaa".to_owned()), "a title contaminated");
        assert_eq!(fa.body, None);

        // Finalize id=b.
        let b2 = Osc99Command {
            id: Some("b".to_owned()),
            payload_type: Osc99PayloadType::Title,
            done: true,
            payload: b"bbb".to_vec(),
            ..default_osc99()
        };
        let fb = handler.reassemble_osc99(b2).expect("b must finalize");
        assert_eq!(fb.title, Some("BBBbbb".to_owned()), "b title contaminated");
        assert_eq!(fb.body, None);
    }

    /// `full_reset()` clears all in-flight pending notifications.
    #[test]
    fn full_reset_clears_pending_notifications() {
        let mut handler = TerminalHandler::new(80, 24);

        // Accumulate a non-final chunk.
        let chunk = Osc99Command {
            id: Some("pending".to_owned()),
            done: false,
            payload: b"partial".to_vec(),
            ..default_osc99()
        };
        handler.reassemble_osc99(chunk);
        assert!(
            !handler.pending_notifications.is_empty(),
            "pending map must be non-empty before reset"
        );

        handler.full_reset();
        assert!(
            handler.pending_notifications.is_empty(),
            "full_reset must clear pending_notifications"
        );
    }

    /// A handler whose host supports OSC 99: while it does not, every OSC 99
    /// request is dropped (130 adversarial review finding 14).
    fn osc99_supported_handler() -> TerminalHandler {
        let mut handler = TerminalHandler::new(80, 24);
        handler.set_host_capabilities(HostCapabilities {
            osc99: Osc99Support::Supported(Osc99Features {
                activation_report: Osc99ActivationReport::Reported,
                close_events: Osc99CloseEvents::Reported,
            }),
        });
        handler
    }

    // ── OSC 99 emit tests (Task 99.4) ─────────────────────────────────────────

    /// A single done title-only notification finalizes and pushes exactly one
    /// `Notification99` window command with defaults mapped correctly.
    #[test]
    fn osc_notify99_single_done_title_pushes_window_command() {
        let mut handler = osc99_supported_handler();
        assert_eq!(handler.window_commands, []);

        let cmd = Osc99Command {
            payload: b"Hello".to_vec(),
            ..default_osc99()
        };
        handler.process_outputs(&[TerminalOutput::OscResponse(AnsiOscType::Notify99(cmd))]);

        assert_eq!(handler.window_commands.len(), 1);
        match &handler.window_commands[0] {
            WindowManipulation::Notification99(data) => {
                assert_eq!(data.title.as_deref(), Some("Hello"));
                assert_eq!(data.occasion, None, "default Always maps to None");
                assert_eq!(data.expire_ms, None, "default -1 maps to None");
                assert_eq!(data.button_labels, [] as [String; 0]);
            }
            other => panic!("expected Notification99, got: {other:?}"),
        }
    }

    /// A base64 title split mid-quantum across two sequences reaches the GUI
    /// as one decoded `Notification99` window command.
    #[test]
    fn osc_notify99_base64_title_split_mid_quantum_pushes_decoded_command() {
        use freminal_common::buffer_states::osc_notify_99::parse_osc_99;

        let mut handler = osc99_supported_handler();
        // "Hello" -> "SGVsbG8=", split after "SGV" (mid-quantum).
        let first = parse_osc_99(b"i=s:d=0:e=1", b"SGV").unwrap();
        let second = parse_osc_99(b"i=s:e=1", b"sbG8=").unwrap();
        handler.process_outputs(&[
            TerminalOutput::OscResponse(AnsiOscType::Notify99(first)),
            TerminalOutput::OscResponse(AnsiOscType::Notify99(second)),
        ]);

        assert_eq!(handler.window_commands.len(), 1);
        match &handler.window_commands[0] {
            WindowManipulation::Notification99(data) => {
                assert_eq!(data.title.as_deref(), Some("Hello"));
            }
            other => panic!("expected Notification99, got: {other:?}"),
        }
    }

    /// Invalid base64 in a standalone notification pushes no window command.
    #[test]
    fn osc_notify99_invalid_base64_pushes_no_window_command() {
        use freminal_common::buffer_states::osc_notify_99::parse_osc_99;

        let mut handler = osc99_supported_handler();
        let cmd = parse_osc_99(b"e=1", b"@@@@").unwrap();
        handler.process_outputs(&[TerminalOutput::OscResponse(AnsiOscType::Notify99(cmd))]);
        assert_eq!(handler.window_commands, []);

        // Control: a valid base64 title through the same handler DOES push.
        let ok = parse_osc_99(b"e=1", b"SGVsbG8=").unwrap();
        handler.process_outputs(&[TerminalOutput::OscResponse(AnsiOscType::Notify99(ok))]);
        assert_eq!(handler.window_commands.len(), 1);
    }

    /// A fully-specified notification maps urgency/occasion/expiry/actions
    /// correctly into the `Notification99Data` shell.
    #[test]
    fn osc_notify99_full_fields_map_correctly() {
        let mut handler = osc99_supported_handler();

        let cmd = Osc99Command {
            id: Some("notif-1".to_owned()),
            urgency: Some(NotificationUrgency::Critical),
            occasion: NotificationOccasion::Unfocused,
            expire_ms: 3000,
            close_report: true,
            actions: Osc99Actions {
                report_activation: true,
                focus_on_activation: false,
            },
            payload: b"Body text".to_vec(),
            payload_type: Osc99PayloadType::Body,
            ..default_osc99()
        };
        handler.process_outputs(&[TerminalOutput::OscResponse(AnsiOscType::Notify99(cmd))]);

        assert_eq!(handler.window_commands.len(), 1);
        match &handler.window_commands[0] {
            WindowManipulation::Notification99(data) => {
                assert_eq!(data.id.as_deref(), Some("notif-1"));
                assert_eq!(data.body.as_deref(), Some("Body text"));
                assert_eq!(data.urgency, Some(2));
                assert_eq!(data.occasion.as_deref(), Some("unfocused"));
                assert_eq!(data.expire_ms, Some(3000));
                assert!(data.close_report);
                assert!(data.report_activation);
                assert!(!data.focus_on_activation);
            }
            other => panic!("expected Notification99, got: {other:?}"),
        }
    }

    /// A non-final chunk (`done: false`, with an id) must not push a window
    /// command — it is still awaiting more chunks.
    #[test]
    fn osc_notify99_non_final_chunk_does_not_push_window_command() {
        let mut handler = osc99_supported_handler();

        let cmd = Osc99Command {
            id: Some("pending-1".to_owned()),
            done: false,
            payload: b"partial".to_vec(),
            ..default_osc99()
        };
        handler.process_outputs(&[TerminalOutput::OscResponse(AnsiOscType::Notify99(cmd))]);

        assert!(
            handler.window_commands.is_empty(),
            "non-final chunk must not emit a window command"
        );

        // Control: the final chunk of the same transfer DOES push, proving the
        // empty assertion above is not vacuous.
        let last = Osc99Command {
            id: Some("pending-1".to_owned()),
            done: true,
            payload: b"-rest".to_vec(),
            ..default_osc99()
        };
        handler.process_outputs(&[TerminalOutput::OscResponse(AnsiOscType::Notify99(last))]);
        assert_eq!(handler.window_commands.len(), 1);
        match &handler.window_commands[0] {
            WindowManipulation::Notification99(data) => {
                assert_eq!(data.title.as_deref(), Some("partial-rest"));
            }
            other => panic!("expected Notification99, got: {other:?}"),
        }
    }

    // ── OSC 99 control routing (Task 99.5c) ───────────────────────────────────

    /// A `p=close` payload finalizing must push `Osc99Control { kind: Close }`,
    /// NOT a `Notification99` (Gap 1 fix — control payloads were previously
    /// misrouted as empty display notifications).
    #[test]
    fn osc_notify99_close_pushes_osc99_control_close() {
        let mut handler = osc99_supported_handler();

        let cmd = Osc99Command {
            id: Some("close-1".to_owned()),
            payload_type: Osc99PayloadType::Close,
            ..default_osc99()
        };
        handler.process_outputs(&[TerminalOutput::OscResponse(AnsiOscType::Notify99(cmd))]);

        assert_eq!(handler.window_commands.len(), 1);
        match &handler.window_commands[0] {
            WindowManipulation::Osc99Control { id, kind } => {
                assert_eq!(id.as_deref(), Some("close-1"));
                assert_eq!(*kind, Osc99ControlKind::Close);
            }
            other => panic!("expected Osc99Control, got: {other:?}"),
        }
    }

    /// A `p=alive` payload finalizing must push `Osc99Control { kind: Alive }`.
    #[test]
    fn osc_notify99_alive_pushes_osc99_control_alive() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.set_host_capabilities(HostCapabilities {
            osc99: Osc99Support::Supported(Osc99Features {
                activation_report: Osc99ActivationReport::Reported,
                close_events: Osc99CloseEvents::Reported,
            }),
        });

        let cmd = Osc99Command {
            id: None,
            payload_type: Osc99PayloadType::Alive,
            ..default_osc99()
        };
        handler.process_outputs(&[TerminalOutput::OscResponse(AnsiOscType::Notify99(cmd))]);

        assert_eq!(handler.window_commands.len(), 1);
        match &handler.window_commands[0] {
            WindowManipulation::Osc99Control { id, kind } => {
                assert_eq!(*id, None);
                assert_eq!(*kind, Osc99ControlKind::Alive);
            }
            other => panic!("expected Osc99Control, got: {other:?}"),
        }
    }

    /// A `p=?` payload finalizing is answered by the handler and pushes no
    /// window command (Task 130.5).
    #[test]
    fn osc_notify99_query_is_answered_without_window_command() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut handler = TerminalHandler::new(80, 24);
        handler.set_write_tx(tx);
        handler.set_host_capabilities(HostCapabilities {
            osc99: Osc99Support::Supported(Osc99Features {
                activation_report: Osc99ActivationReport::Reported,
                close_events: Osc99CloseEvents::Reported,
            }),
        });

        let cmd = Osc99Command {
            id: Some("query-1".to_owned()),
            payload_type: Osc99PayloadType::Query,
            ..default_osc99()
        };
        handler.process_outputs(&[TerminalOutput::OscResponse(AnsiOscType::Notify99(cmd))]);

        assert_eq!(handler.window_commands.len(), 0);
        let PtyWrite::Write(bytes) = rx.try_recv().unwrap() else {
            panic!("expected PtyWrite::Write");
        };
        assert!(bytes.starts_with(b"\x1b]99;i=query-1:p=?;"));
    }

    /// A display payload type (`Title`) still produces `Notification99`, not
    /// `Osc99Control` — the display path is unchanged by the 99.5c branch.
    #[test]
    fn osc_notify99_title_still_pushes_notification99_not_control() {
        let mut handler = osc99_supported_handler();

        let cmd = Osc99Command {
            payload_type: Osc99PayloadType::Title,
            payload: b"Hello".to_vec(),
            ..default_osc99()
        };
        handler.process_outputs(&[TerminalOutput::OscResponse(AnsiOscType::Notify99(cmd))]);

        assert_eq!(handler.window_commands.len(), 1);
        match &handler.window_commands[0] {
            WindowManipulation::Notification99(data) => {
                assert_eq!(data.title.as_deref(), Some("Hello"));
            }
            other => panic!("expected Notification99, got: {other:?}"),
        }
    }

    // ── Payload-free warn logging (129.14) ───────────────────────────────────

    /// Feed `bytes` through the whole parser + handler pipeline and return
    /// every log event emitted on this thread while doing so.
    fn feed_and_capture(bytes: &[u8]) -> Vec<Captured> {
        capture(|| {
            let mut state = TerminalState::default();
            state.handle_incoming_data(bytes);
        })
    }

    /// Assert that at least one warn/error was logged and that none of them
    /// contains `secret`.
    fn assert_warns_without(events: &[Captured], secret: &str) {
        let warns = warnings(events);
        assert!(!warns.is_empty(), "expected a warn, got: {events:?}");
        for (level, text) in warns {
            assert!(
                !text.contains(secret),
                "{level} line leaked payload: {text}"
            );
        }
    }

    #[test]
    fn osc99_request_dropped_while_unsupported_logs_a_payload_free_debug() {
        // `TerminalState::default()` carries the default host capabilities:
        // OSC 99 unsupported.
        let events = feed_and_capture(b"\x1b]99;i=1;SECRETPAYLOAD\x1b\\");
        assert!(
            events
                .iter()
                .all(|(_, text)| !text.contains("SECRETPAYLOAD")),
            "a log line leaked the payload: {events:?}"
        );
        let drops = events
            .iter()
            .filter(|(_, text)| text.contains("OSC 99 request dropped"))
            .count();
        assert_eq!(drops, 1, "exactly one drop line, got: {events:?}");
    }

    #[test]
    fn unknown_osc_warn_omits_payload_and_names_number() {
        let events = feed_and_capture(b"\x1b]9999;SECRETPAYLOAD\x07");
        assert_warns_without(&events, "SECRETPAYLOAD");
        let warns = warnings(&events);
        assert!(
            warns.iter().any(|(_, text)| text.contains("OSC 9999")),
            "warn should name the OSC number: {warns:?}"
        );
    }

    #[test]
    fn unknown_osc_with_non_numeric_number_warn_omits_attacker_text() {
        let events = feed_and_capture(b"\x1b]SECRETPAYLOAD;x\x07");
        assert_warns_without(&events, "SECRETPAYLOAD");
        let warns = warnings(&events);
        assert!(
            warns.iter().any(|(_, text)| text.contains("non-numeric")),
            "warn should say the number is non-numeric: {warns:?}"
        );
    }

    #[test]
    fn unimplemented_osc_warn_omits_payload() {
        let events = feed_and_capture(b"\x1b]13;SECRETPAYLOAD\x07");
        assert_warns_without(&events, "SECRETPAYLOAD");
    }

    #[test]
    fn non_kitty_apc_warn_omits_payload() {
        let events = feed_and_capture(b"\x1b_XSECRETPAYLOAD\x1b\\");
        assert_warns_without(&events, "SECRETPAYLOAD");
    }

    #[test]
    fn malformed_kitty_graphics_apc_warn_omits_payload() {
        let events = feed_and_capture(b"\x1b_GSECRETPAYLOAD;AAAA\x1b\\");
        assert_warns_without(&events, "SECRETPAYLOAD");
    }

    #[test]
    fn osc_52_invalid_base64_warn_omits_payload() {
        let events = feed_and_capture(b"\x1b]52;c;SECRET!!\x07");
        assert_warns_without(&events, "SECRET");
    }

    #[test]
    fn osc_4_invalid_color_spec_warn_omits_payload() {
        let events = feed_and_capture(b"\x1b]4;1;SECRETPAYLOAD\x07");
        assert_warns_without(&events, "SECRETPAYLOAD");
    }

    #[test]
    fn osc_10_unrecognised_color_spec_warn_omits_payload() {
        let events = feed_and_capture(b"\x1b]10;SECRETPAYLOAD\x07");
        assert_warns_without(&events, "SECRETPAYLOAD");
    }

    #[test]
    fn osc_7_unparsable_uri_warn_omits_payload() {
        let events = feed_and_capture(b"\x1b]7;SECRETPAYLOAD\x07");
        assert_warns_without(&events, "SECRETPAYLOAD");
    }

    #[test]
    fn osc_1337_unknown_sub_command_warn_omits_payload() {
        let events = feed_and_capture(b"\x1b]1337;SECRETPAYLOAD=1\x07");
        assert_warns_without(&events, "SECRETPAYLOAD");
    }

    #[test]
    fn osc_1337_file_unknown_arg_warn_omits_payload() {
        let events = feed_and_capture(b"\x1b]1337;File=SECRETKEY=SECRETVALUE:QUJD\x07");
        let warns = warnings(&events);
        for (level, text) in warns {
            assert!(
                !text.contains("SECRETKEY") && !text.contains("SECRETVALUE"),
                "{level} line leaked payload: {text}"
            );
        }
    }

    #[test]
    fn osc_133_unknown_marker_warn_omits_payload() {
        let events = feed_and_capture(b"\x1b]133;ZSECRETPAYLOAD\x07");
        assert_warns_without(&events, "SECRETPAYLOAD");
    }

    #[test]
    fn osc_1338_unknown_sub_command_warn_omits_payload() {
        let events = feed_and_capture(b"\x1b]1338;SECRETPAYLOAD=1\x07");
        assert_warns_without(&events, "SECRETPAYLOAD");
    }
}
