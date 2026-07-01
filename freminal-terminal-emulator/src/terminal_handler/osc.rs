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

use super::{TerminalHandler, shell_integration};

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
                    "APC received (not Kitty graphics, ignored): {}",
                    String::from_utf8_lossy(apc)
                );
            }
            Err(e) => {
                tracing::warn!("Kitty graphics parse error: {e}");
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
                    tracing::warn!("OSC 7: failed to parse URI: {value}");
                } else {
                    tracing::debug!("OSC 7: CWD set to {:?}", self.current_working_directory);
                }
            }
            AnsiOscType::ShellInfoHistFile(path) => {
                tracing::debug!("OSC 1338: HISTFILE set to {:?}", path);
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

            // OSC 9 / OSC 777 — desktop notification.  Forward to the GUI via
            // the window-command channel; the GUI's notification router
            // (Task 76.4) applies the `[notifications]` routing policy.
            AnsiOscType::Notify { title, body } => {
                self.window_commands.push(WindowManipulation::Notification {
                    kind: NotificationKind::OscText,
                    title: title.clone(),
                    body: body.clone(),
                });
            }

            // OSC 99 stateful notification (Task 99). Feed each parsed chunk into the
            // reassembly machine; on finalize, map to WindowManipulation::Notification99
            // and forward to the GUI via the window-command channel (Task 99.4).
            AnsiOscType::Notify99(cmd) => {
                if let Some(finalized) = self.reassemble_osc99(cmd.clone()) {
                    self.window_commands
                        .push(WindowManipulation::Notification99(Box::new(
                            finalized.into_notification99_data(),
                        )));
                }
            }

            AnsiOscType::NoOp => {}
        }
    }

    /// Handle an OSC 133 (FTCS) shell integration marker.
    ///
    /// Only markers carrying `freminal=1` and a `fid` (parsed upstream by
    /// [`parse_ftcs_params`]) reach this function.  Foreign markers (`WezTerm`,
    /// Starship, `iTerm2`) are already filtered out at the parse layer and
    /// never arrive here.
    pub(super) fn handle_osc_ftcs(&mut self, marker: &FtcsMarker) {
        tracing::debug!("OSC 133 FTCS marker: {marker}");
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
    use freminal_common::buffer_states::osc::AnsiOscType;
    use freminal_common::buffer_states::osc_notify_99::{
        NotificationOccasion, NotificationUrgency, Osc99Actions, Osc99Command, Osc99PayloadType,
    };
    use freminal_common::buffer_states::terminal_output::TerminalOutput;
    use freminal_common::buffer_states::window_manipulation::{
        NotificationKind, WindowManipulation,
    };

    // ── Existing OSC 9/777 tests ──────────────────────────────────────────────

    #[test]
    fn osc_notify_pushes_window_command() {
        let mut handler = TerminalHandler::new(80, 24);
        assert!(
            handler.window_commands.is_empty(),
            "no window commands initially"
        );

        handler.process_outputs(&[TerminalOutput::OscResponse(AnsiOscType::Notify {
            title: Some("Build".to_owned()),
            body: "done".to_owned(),
        })]);

        assert_eq!(handler.window_commands.len(), 1);
        match &handler.window_commands[0] {
            WindowManipulation::Notification { kind, title, body } => {
                assert_eq!(*kind, NotificationKind::OscText);
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
            title: None,
            body: "hello".to_owned(),
        })]);

        assert_eq!(handler.window_commands.len(), 1);
        match &handler.window_commands[0] {
            WindowManipulation::Notification { kind, title, body } => {
                assert_eq!(*kind, NotificationKind::OscText);
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

    // ── OSC 99 emit tests (Task 99.4) ─────────────────────────────────────────

    /// A single done title-only notification finalizes and pushes exactly one
    /// `Notification99` window command with defaults mapped correctly.
    #[test]
    fn osc_notify99_single_done_title_pushes_window_command() {
        let mut handler = TerminalHandler::new(80, 24);
        assert!(handler.window_commands.is_empty());

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
                assert!(data.button_labels.is_empty());
            }
            other => panic!("expected Notification99, got: {other:?}"),
        }
    }

    /// A fully-specified notification maps urgency/occasion/expiry/actions
    /// correctly into the `Notification99Data` shell.
    #[test]
    fn osc_notify99_full_fields_map_correctly() {
        let mut handler = TerminalHandler::new(80, 24);

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
        let mut handler = TerminalHandler::new(80, 24);

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
    }
}
