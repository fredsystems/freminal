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

use crate::ansi_components::tracer::{
    escape_sequence_for_log_bounded, lossy_sequence_for_log_bounded,
};
use freminal_common::buffer_states::{
    ftcs::{FtcsMarker, FtcsState},
    kitty_graphics::{KittyParseError, parse_kitty_graphics},
    osc::{AnsiOscType, UrlResponse},
    url::Url,
    window_manipulation::{NotificationKind, WindowManipulation},
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

            // Colour control: OSC 4 / 104 (palette) and OSC 10-19 / 110-119
            // (dynamic colours) set, query and reset.
            AnsiOscType::PaletteColors(_)
            | AnsiOscType::ResetPaletteColors(_)
            | AnsiOscType::DynamicColors(_)
            | AnsiOscType::ResetDynamicColor(_) => {
                self.handle_osc_colors(osc);
            }

            // Remote host / CWD: OSC 7 ; file://hostname/path ST
            AnsiOscType::RemoteHost(value) => {
                // OSC 7 is unauthenticated, so the URI is validated before
                // it is trusted. A rejected URI clears the stored cwd so the
                // GUI falls back to the shell process's real directory.
                match shell_integration::parse_osc7_uri(value, shell_integration::local_hostname())
                {
                    Ok(cwd) => {
                        tracing::debug!(
                            "OSC 7: CWD set to {:?}",
                            lossy_sequence_for_log_bounded(cwd.as_bytes())
                        );
                        self.current_working_directory = Some(cwd);
                    }
                    Err(reason) => {
                        self.current_working_directory = None;
                        // `reason` is a static, payload-free description, so
                        // it is safe at warn; the raw URI stays at debug.
                        tracing::warn!("OSC 7: URI rejected: {reason}");
                        tracing::debug!(
                            "OSC 7: URI rejected ({reason}): {}",
                            lossy_sequence_for_log_bounded(value.as_bytes())
                        );
                    }
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

            // OSC 99 stateful notification (Task 99). See `handle_osc99`: while
            // unsupported every chunk is dropped before anything else; while
            // supported, control payloads (`p=?`, `p=close`, `p=alive`) act at
            // once and content payloads are reassembled.
            AnsiOscType::Notify99(cmd) => self.handle_osc99(cmd),
            // A chunk the parser refused: abandons the transfer it names,
            // behind the same support gate as every other OSC 99 chunk.
            AnsiOscType::Notify99Rejected(rejected) => self.handle_osc99_rejected(rejected),

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
mod tests {
    use super::TerminalHandler;
    use crate::log_capture::{Captured, capture, warnings};
    use crate::state::internal::TerminalState;
    use freminal_common::buffer_states::osc::{AnsiOscType, OscNotifySource};
    use freminal_common::buffer_states::osc_notify_99::{
        Osc99Action, Osc99ActionEdit, Osc99ChunkState, Osc99Command, Osc99PayloadEncoding,
        Osc99PayloadType,
    };
    use freminal_common::buffer_states::osc99_notification::{
        Notification99Data, NotificationOccasion, NotificationUrgency, Osc99ActivationFocus,
        Osc99ActivationReporting, Osc99CloseReporting, Osc99ControlKind, Osc99Expiry,
    };
    use freminal_common::buffer_states::terminal_output::TerminalOutput;
    use freminal_common::buffer_states::window_manipulation::{
        NotificationKind, WindowManipulation,
    };
    use freminal_common::host_capabilities::{
        HostCapabilities, Osc99ActivationReport, Osc99AutoExpiry, Osc99BodySupport,
        Osc99ButtonSupport, Osc99CloseEvents, Osc99Features, Osc99FocusSupport, Osc99IconSupport,
        Osc99SoundNames, Osc99Support, Osc99UrgencySupport,
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
            chunk_state: Osc99ChunkState::Done,
            payload: Vec::new(),
            payload_encoding: Osc99PayloadEncoding::Plain,
            action_edits: Vec::new(),
            close_reporting: None,
            app_name: None,
            icon_cache_key: None,
            icon_names: Vec::new(),
            occasion: None,
            sound: None,
            notification_types: Vec::new(),
            urgency: None,
            expiry: None,
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
        let result = handler.reassemble_osc99(&cmd);
        let finalized = result.expect("single done chunk must finalize");
        assert_eq!(finalized.title, "Hi");
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
            chunk_state: Osc99ChunkState::More,
            payload: b"He".to_vec(),
            ..default_osc99()
        };
        let r1 = handler.reassemble_osc99(&chunk1);
        assert!(r1.is_none(), "first chunk must not finalize");

        // Chunk 2: id=n1, Title, d=false, payload "llo"
        let chunk2 = Osc99Command {
            id: Some("n1".to_owned()),
            payload_type: Osc99PayloadType::Title,
            chunk_state: Osc99ChunkState::More,
            payload: b"llo".to_vec(),
            ..default_osc99()
        };
        let r2 = handler.reassemble_osc99(&chunk2);
        assert!(r2.is_none(), "second chunk must not finalize");

        // Chunk 3: id=n1, Body, d=true, payload "World"
        let chunk3 = Osc99Command {
            id: Some("n1".to_owned()),
            payload_type: Osc99PayloadType::Body,
            chunk_state: Osc99ChunkState::Done,
            payload: b"World".to_vec(),
            ..default_osc99()
        };
        let r3 = handler.reassemble_osc99(&chunk3);
        let finalized = r3.expect("terminating chunk must finalize");
        assert_eq!(finalized.title, "Hello");
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
        let r1 = handler.reassemble_osc99(&first);
        assert!(r1.is_some(), "first n1 must finalize");

        // Second notification: same id, new content.
        let second = Osc99Command {
            id: Some("n1".to_owned()),
            payload: b"Second".to_vec(),
            ..default_osc99()
        };
        let r2 = handler.reassemble_osc99(&second);
        let finalized = r2.expect("second n1 must finalize (fresh start)");
        // Must not carry stale bytes from the first notification.
        assert_eq!(finalized.title, "Second");
    }

    /// An id-less non-final chunk accumulates in the single unidentified slot
    /// (never in the id map), and an id-less final chunk completes it.
    #[test]
    fn unidentified_chunks_accumulate_in_the_shared_slot() {
        let mut handler = TerminalHandler::new(80, 24);
        let first = Osc99Command {
            id: None,
            chunk_state: Osc99ChunkState::More,
            payload: b"Wor".to_vec(),
            ..default_osc99()
        };
        assert!(handler.reassemble_osc99(&first).is_none());
        assert_eq!(
            handler.pending_osc99.identified_len(),
            0,
            "an id-less chunk must not enter the id map"
        );
        assert!(
            !handler.pending_osc99.is_empty(),
            "the unidentified slot must hold the chunk"
        );

        let last = Osc99Command {
            id: None,
            payload: b"ld".to_vec(),
            ..default_osc99()
        };
        let finalized = handler
            .reassemble_osc99(&last)
            .expect("the id-less final chunk must finalize");
        assert_eq!(finalized.title, "World");
        assert!(handler.pending_osc99.is_empty());
    }

    /// Two interleaved ids must not cross-contaminate each other's payloads.
    #[test]
    fn two_interleaved_ids_no_cross_contamination() {
        let mut handler = TerminalHandler::new(80, 24);

        // id=a: non-final title chunk.
        let a1 = Osc99Command {
            id: Some("a".to_owned()),
            payload_type: Osc99PayloadType::Title,
            chunk_state: Osc99ChunkState::More,
            payload: b"AAA".to_vec(),
            ..default_osc99()
        };
        assert!(handler.reassemble_osc99(&a1).is_none());

        // id=b: non-final title chunk.
        let b1 = Osc99Command {
            id: Some("b".to_owned()),
            payload_type: Osc99PayloadType::Title,
            chunk_state: Osc99ChunkState::More,
            payload: b"BBB".to_vec(),
            ..default_osc99()
        };
        assert!(handler.reassemble_osc99(&b1).is_none());

        // Finalize id=a.
        let a2 = Osc99Command {
            id: Some("a".to_owned()),
            payload_type: Osc99PayloadType::Title,
            chunk_state: Osc99ChunkState::Done,
            payload: b"aaa".to_vec(),
            ..default_osc99()
        };
        let fa = handler.reassemble_osc99(&a2).expect("a must finalize");
        assert_eq!(fa.title, "AAAaaa", "a title contaminated");
        assert_eq!(fa.body, None);

        // Finalize id=b.
        let b2 = Osc99Command {
            id: Some("b".to_owned()),
            payload_type: Osc99PayloadType::Title,
            chunk_state: Osc99ChunkState::Done,
            payload: b"bbb".to_vec(),
            ..default_osc99()
        };
        let fb = handler.reassemble_osc99(&b2).expect("b must finalize");
        assert_eq!(fb.title, "BBBbbb", "b title contaminated");
        assert_eq!(fb.body, None);
    }

    /// `full_reset()` clears all in-flight pending notifications, the
    /// unidentified slot included.
    #[test]
    fn full_reset_clears_pending_osc99() {
        let mut handler = TerminalHandler::new(80, 24);

        // Accumulate a non-final chunk.
        let chunk = Osc99Command {
            id: Some("pending".to_owned()),
            chunk_state: Osc99ChunkState::More,
            payload: b"partial".to_vec(),
            ..default_osc99()
        };
        handler.reassemble_osc99(&chunk);
        let unidentified = Osc99Command {
            id: None,
            chunk_state: Osc99ChunkState::More,
            payload: b"partial".to_vec(),
            ..default_osc99()
        };
        handler.reassemble_osc99(&unidentified);
        assert_eq!(handler.pending_osc99.identified_len(), 1);
        assert!(handler.pending_osc99.get(None).is_some());

        handler.full_reset();
        assert!(
            handler.pending_osc99.is_empty(),
            "full_reset must clear identified and unidentified pending state"
        );
        assert_eq!(handler.pending_osc99.identified_len(), 0);
        assert!(handler.pending_osc99.get(None).is_none());
    }

    /// Host capabilities under which OSC 99 is honoured.
    fn osc99_supported_caps() -> HostCapabilities {
        HostCapabilities {
            osc99: Osc99Support::Supported(Osc99Features {
                activation_report: Osc99ActivationReport::Reported,
                close_events: Osc99CloseEvents::Reported,
                activation_focus: Osc99FocusSupport::Unsupported,
                auto_expiry: Osc99AutoExpiry::Supported,
                urgency: Osc99UrgencySupport::Supported,
                sound_names: Osc99SoundNames::SystemAndSilent,
                body: Osc99BodySupport::Supported,
                buttons: Osc99ButtonSupport::Supported,
                icons: Osc99IconSupport::Supported,
            }),
        }
    }

    /// A handler whose host supports OSC 99: while it does not, every OSC 99
    /// request is dropped (130 adversarial review finding 14).
    fn osc99_supported_handler() -> TerminalHandler {
        let mut handler = TerminalHandler::new(80, 24);
        handler.set_host_capabilities(osc99_supported_caps());
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
                assert_eq!(data.title, "Hello");
                assert_eq!(data.occasion, NotificationOccasion::Always);
                assert_eq!(data.expiry, Osc99Expiry::OsDefault, "default -1");
                assert_eq!(data.buttons, [] as [String; 0]);
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
                assert_eq!(data.title, "Hello");
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
            occasion: Some(NotificationOccasion::Unfocused),
            expiry: Some(Osc99Expiry::After(std::num::NonZeroU32::new(3000).unwrap())),
            close_reporting: Some(Osc99CloseReporting::Report),
            action_edits: vec![
                Osc99ActionEdit::Add(Osc99Action::Report),
                Osc99ActionEdit::Remove(Osc99Action::Focus),
            ],
            payload: b"Body text".to_vec(),
            payload_type: Osc99PayloadType::Body,
            ..default_osc99()
        };
        handler.process_outputs(&[TerminalOutput::OscResponse(AnsiOscType::Notify99(cmd))]);

        assert_eq!(handler.window_commands.len(), 1);
        match &handler.window_commands[0] {
            WindowManipulation::Notification99(data) => {
                assert_eq!(data.id.as_deref(), Some("notif-1"));
                // A body-only notification is promoted into the title.
                assert_eq!(data.title, "Body text");
                assert_eq!(data.body, None);
                assert_eq!(data.urgency, NotificationUrgency::Critical);
                assert_eq!(data.occasion, NotificationOccasion::Unfocused);
                assert_eq!(
                    data.expiry,
                    Osc99Expiry::After(std::num::NonZeroU32::new(3000).unwrap())
                );
                assert_eq!(data.close_reporting, Osc99CloseReporting::Report);
                assert_eq!(data.activation_reporting, Osc99ActivationReporting::Report);
                assert_eq!(data.activation_focus, Osc99ActivationFocus::NoFocus);
            }
            other => panic!("expected Notification99, got: {other:?}"),
        }
    }

    /// A non-final chunk (`chunk_state: More`, with an id) must not push a window
    /// command — it is still awaiting more chunks.
    #[test]
    fn osc_notify99_non_final_chunk_does_not_push_window_command() {
        let mut handler = osc99_supported_handler();

        let cmd = Osc99Command {
            id: Some("pending-1".to_owned()),
            chunk_state: Osc99ChunkState::More,
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
            chunk_state: Osc99ChunkState::Done,
            payload: b"-rest".to_vec(),
            ..default_osc99()
        };
        handler.process_outputs(&[TerminalOutput::OscResponse(AnsiOscType::Notify99(last))]);
        assert_eq!(handler.window_commands.len(), 1);
        match &handler.window_commands[0] {
            WindowManipulation::Notification99(data) => {
                assert_eq!(data.title, "partial-rest");
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
                assert_eq!(id, "close-1");
                assert_eq!(*kind, Osc99ControlKind::Close);
            }
            other => panic!("expected Osc99Control, got: {other:?}"),
        }
    }

    /// A `p=alive` payload with an id must push `Osc99Control { kind: Alive }`.
    #[test]
    fn osc_notify99_alive_pushes_osc99_control_alive() {
        let mut handler = osc99_supported_handler();

        let cmd = Osc99Command {
            id: Some("alive-1".to_owned()),
            payload_type: Osc99PayloadType::Alive,
            ..default_osc99()
        };
        handler.process_outputs(&[TerminalOutput::OscResponse(AnsiOscType::Notify99(cmd))]);

        assert_eq!(handler.window_commands.len(), 1);
        match &handler.window_commands[0] {
            WindowManipulation::Osc99Control { id, kind } => {
                assert_eq!(id, "alive-1");
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
                activation_focus: Osc99FocusSupport::Unsupported,
                auto_expiry: Osc99AutoExpiry::Supported,
                urgency: Osc99UrgencySupport::Supported,
                sound_names: Osc99SoundNames::SystemAndSilent,
                body: Osc99BodySupport::Supported,
                buttons: Osc99ButtonSupport::Supported,
                icons: Osc99IconSupport::Supported,
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
                assert_eq!(data.title, "Hello");
            }
            other => panic!("expected Notification99, got: {other:?}"),
        }
    }

    // ── 138.3: reassembly rework, fed as real escape sequences ────────────────

    /// A full terminal state whose host supports OSC 99, and the receiver of
    /// everything it writes to the PTY.
    fn osc99_state() -> (TerminalState, crossbeam_channel::Receiver<PtyWrite>) {
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        let mut state = TerminalState::new(tx, None);
        state.handler.set_host_capabilities(osc99_supported_caps());
        (state, rx)
    }

    /// Feed one real OSC 99 escape sequence, `ESC ] 99 ; metadata ; payload ST`.
    fn osc99_send(state: &mut TerminalState, metadata: &str, payload: &str) {
        let bytes = format!("\x1b]99;{metadata};{payload}\x1b\\");
        state.handle_incoming_data(bytes.as_bytes());
    }

    /// Every `Notification99` pushed so far.
    fn osc99_notifications(state: &TerminalState) -> Vec<&Notification99Data> {
        state
            .window_commands
            .iter()
            .filter_map(|c| match c {
                WindowManipulation::Notification99(data) => Some(&**data),
                _ => None,
            })
            .collect()
    }

    /// The title of every `Notification99` pushed so far.
    fn osc99_titles(state: &TerminalState) -> Vec<String> {
        osc99_notifications(state)
            .iter()
            .map(|n| n.title.clone())
            .collect()
    }

    /// Everything written to the PTY so far, concatenated.
    fn osc99_written(rx: &crossbeam_channel::Receiver<PtyWrite>) -> Vec<u8> {
        let mut out = Vec::new();
        while let Ok(PtyWrite::Write(bytes)) = rx.try_recv() {
            out.extend_from_slice(&bytes);
        }
        out
    }

    /// Whether the in-flight entry for `id` is a tombstone.
    fn osc99_has_tombstone(state: &TerminalState, id: &str) -> bool {
        matches!(
            state.handler.pending_osc99.get(Some(id)),
            Some(super::super::notify_99::PendingEntry::Dropped)
        )
    }

    #[test]
    fn merged_chunks_keep_report_and_default_focus() {
        let (mut state, _rx) = osc99_state();
        osc99_send(&mut state, "i=x:d=0:a=report", "T");
        assert_eq!(osc99_titles(&state), Vec::<String>::new());
        osc99_send(&mut state, "i=x", "U");

        let notes = osc99_notifications(&state);
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].title, "TU");
        assert_eq!(
            notes[0].activation_reporting,
            Osc99ActivationReporting::Report
        );
        assert_eq!(notes[0].activation_focus, Osc99ActivationFocus::Focus);
    }

    #[test]
    fn earlier_urgency_survives_a_final_chunk_without_u() {
        let (mut state, _rx) = osc99_state();
        osc99_send(&mut state, "i=u:d=0:u=2", "A");
        osc99_send(&mut state, "i=u", "B");

        let notes = osc99_notifications(&state);
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].urgency, NotificationUrgency::Critical);
    }

    #[test]
    fn names_and_types_from_two_chunks_keep_arrival_order() {
        let (mut state, _rx) = osc99_state();
        // n=a, n=b and t=ta, t=tb, base64 encoded.
        osc99_send(&mut state, "i=m:d=0:n=YQ==:t=dGE=", "T");
        osc99_send(&mut state, "i=m:n=Yg==:t=dGI=", "");

        let notes = osc99_notifications(&state);
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].icon_names, ["a", "b"]);
        assert_eq!(notes[0].notification_types, ["ta", "tb"]);
    }

    #[test]
    fn id_less_chunks_assemble_one_notification() {
        let (mut state, _rx) = osc99_state();
        osc99_send(&mut state, "d=0", "Wor");
        assert_eq!(osc99_titles(&state), Vec::<String>::new());
        osc99_send(&mut state, "", "ld");

        let notes = osc99_notifications(&state);
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].title, "World");
        assert_eq!(notes[0].id, None);
        assert!(state.handler.pending_osc99.is_empty());
    }

    #[test]
    fn id_less_transfer_dropped_mid_way_stays_dropped_until_the_final_chunk() {
        let (mut state, _rx) = osc99_state();
        osc99_send(&mut state, "d=0:e=1", "@@@@");
        osc99_send(&mut state, "d=0", "ignored");
        osc99_send(&mut state, "", "tail");
        assert_eq!(osc99_titles(&state), Vec::<String>::new());
        assert!(state.handler.pending_osc99.is_empty());

        osc99_send(&mut state, "", "fresh");
        assert_eq!(osc99_notifications(&state).len(), 1);
    }

    // ── Controls act immediately ──────────────────────────────────────────────

    #[test]
    fn query_with_d0_is_answered_at_once() {
        let (mut state, rx) = osc99_state();
        osc99_send(&mut state, "i=q:p=?:d=0", "");
        let written = osc99_written(&rx);
        assert!(
            written.starts_with(b"\x1b]99;i=q:p=?;"),
            "unexpected reply: {written:?}"
        );
        assert_eq!(state.window_commands, []);
    }

    #[test]
    fn close_with_d0_is_forwarded_at_once() {
        let (mut state, _rx) = osc99_state();
        osc99_send(&mut state, "i=c:p=close:d=0", "");
        assert_eq!(
            state.window_commands,
            [WindowManipulation::Osc99Control {
                id: "c".to_owned(),
                kind: Osc99ControlKind::Close,
            }]
        );
    }

    #[test]
    fn controls_without_an_id_push_nothing() {
        let (mut state, _rx) = osc99_state();
        osc99_send(&mut state, "p=close", "");
        osc99_send(&mut state, "p=alive", "");
        osc99_send(&mut state, "p=close:d=0", "");
        osc99_send(&mut state, "p=alive:d=0", "");
        assert_eq!(state.window_commands, []);
        assert!(state.handler.pending_osc99.is_empty());
    }

    #[test]
    fn pending_content_survives_a_control_chunk_for_the_same_id() {
        let (mut state, _rx) = osc99_state();
        osc99_send(&mut state, "i=x:d=0", "A");
        osc99_send(&mut state, "i=x:p=alive", "");
        osc99_send(&mut state, "i=x", "B");

        assert_eq!(state.window_commands.len(), 2);
        assert_eq!(
            state.window_commands[0],
            WindowManipulation::Osc99Control {
                id: "x".to_owned(),
                kind: Osc99ControlKind::Alive,
            }
        );
        let notes = osc99_notifications(&state);
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].title, "AB");
    }

    #[test]
    fn a_tombstone_is_unaffected_by_a_control_chunk() {
        let (mut state, _rx) = osc99_state();
        osc99_send(&mut state, "i=t:d=0:e=1", "@@@@");
        assert!(osc99_has_tombstone(&state, "t"));

        osc99_send(&mut state, "i=t:p=close", "");
        assert_eq!(
            state.window_commands,
            [WindowManipulation::Osc99Control {
                id: "t".to_owned(),
                kind: Osc99ControlKind::Close,
            }]
        );
        assert!(
            osc99_has_tombstone(&state, "t"),
            "a control chunk must not remove the tombstone"
        );

        // The rest of the dropped transfer is still discarded.
        osc99_send(&mut state, "i=t", "tail");
        assert_eq!(osc99_titles(&state), Vec::<String>::new());
        assert!(state.handler.pending_osc99.is_empty());
    }

    // ── Saturation ────────────────────────────────────────────────────────────

    /// Open 128 pending ids (`id-0` .. `id-127`), each holding `"x"`.
    fn osc99_saturate(state: &mut TerminalState) {
        for i in 0..128 {
            osc99_send(state, &format!("i=id-{i}:d=0"), "x");
        }
        assert_eq!(state.handler.pending_osc99.identified_len(), 128);
    }

    #[test]
    fn a_complete_notification_on_a_saturated_map_displays_without_evicting() {
        let (mut state, _rx) = osc99_state();
        osc99_saturate(&mut state);

        osc99_send(&mut state, "i=new", "Hello");
        let notes = osc99_notifications(&state);
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].title, "Hello");

        // A complete single-chunk notification never enters the store, so no
        // in-progress transfer is evicted to make room for it.
        assert_eq!(state.handler.pending_osc99.identified_len(), 128);
        for i in 0..128 {
            assert!(
                state
                    .handler
                    .pending_osc99
                    .get(Some(&format!("id-{i}")))
                    .is_some(),
                "id-{i} must survive"
            );
        }
    }

    #[test]
    fn touching_an_id_protects_it_from_eviction() {
        let (mut state, _rx) = osc99_state();
        osc99_saturate(&mut state);
        // Touch the oldest id again: `id-1` is now the least recently touched.
        osc99_send(&mut state, "i=id-0:d=0", "y");

        osc99_send(&mut state, "i=new:d=0", "n");
        assert!(state.handler.pending_osc99.get(Some("id-1")).is_none());
        assert!(state.handler.pending_osc99.get(Some("id-0")).is_some());

        // The protected id kept its content; the evicted one starts afresh.
        osc99_send(&mut state, "i=id-0", "z");
        osc99_send(&mut state, "i=id-1", "z");
        assert_eq!(osc99_titles(&state), ["xyz", "z"]);
    }

    #[test]
    fn a_new_pending_id_on_a_saturated_map_evicts_without_growing_it() {
        let (mut state, _rx) = osc99_state();
        osc99_saturate(&mut state);

        osc99_send(&mut state, "i=new:d=0", "n");
        assert_eq!(state.handler.pending_osc99.identified_len(), 128);
        assert!(state.handler.pending_osc99.get(Some("id-0")).is_none());
        assert!(state.handler.pending_osc99.get(Some("new")).is_some());
    }

    #[test]
    fn tombstones_are_evicted_like_any_other_entry() {
        let (mut state, _rx) = osc99_state();
        osc99_send(&mut state, "i=bad:d=0:e=1", "@@@@");
        assert!(osc99_has_tombstone(&state, "bad"));
        for i in 0..128 {
            osc99_send(&mut state, &format!("i=id-{i}:d=0"), "x");
        }
        assert_eq!(state.handler.pending_osc99.identified_len(), 128);
        assert!(
            state.handler.pending_osc99.get(Some("bad")).is_none(),
            "the oldest entry, a tombstone, must have been evicted"
        );
    }

    // ── Finalisation ──────────────────────────────────────────────────────────

    #[test]
    fn control_characters_are_stripped_from_the_title_but_lf_is_kept() {
        let (mut state, _rx) = osc99_state();
        let title = freminal_common::base64::encode("a\u{1b}b\u{85}c\td\ne\u{7f}f".as_bytes());
        osc99_send(&mut state, "e=1", &title);

        let notes = osc99_notifications(&state);
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].title, "abcd\nef");
    }

    #[test]
    fn a_long_body_is_cut_hard_at_1024_chars() {
        let (mut state, _rx) = osc99_state();
        osc99_send(&mut state, "i=b:d=0", "T");
        osc99_send(&mut state, "i=b:p=body", &"x".repeat(2000));

        let notes = osc99_notifications(&state);
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].body.as_deref(), Some("x".repeat(1024).as_str()));
    }

    #[test]
    fn nine_buttons_become_eight() {
        let (mut state, _rx) = osc99_state();
        osc99_send(&mut state, "i=b:d=0", "T");
        let labels = ["1", "2", "3", "4", "5", "6", "7", "8", "9"].join("\u{2028}");
        osc99_send(
            &mut state,
            "i=b:p=buttons:e=1",
            &freminal_common::base64::encode(labels.as_bytes()),
        );

        let notes = osc99_notifications(&state);
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].buttons, ["1", "2", "3", "4", "5", "6", "7", "8"]);
    }

    #[test]
    fn a_body_only_notification_becomes_title_only() {
        let (mut state, _rx) = osc99_state();
        osc99_send(&mut state, "p=body", "Hello");

        let notes = osc99_notifications(&state);
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].title, "Hello");
        assert_eq!(notes[0].body, None);
    }

    #[test]
    fn a_notification_without_title_or_body_pushes_nothing() {
        let (mut state, rx) = osc99_state();
        osc99_send(&mut state, "", "");
        osc99_send(&mut state, "i=a:p=icon:e=1", "AAAA");
        osc99_send(&mut state, "i=b:p=buttons", "OK");
        // A body that is all control characters strips to nothing.
        osc99_send(
            &mut state,
            "i=c:p=body:e=1",
            &freminal_common::base64::encode(b"\x01\x02"),
        );
        assert_eq!(state.window_commands, []);
        assert!(
            osc99_written(&rx).is_empty(),
            "a dropped notification gets no reply"
        );
        assert!(state.handler.pending_osc99.is_empty());
    }

    #[test]
    fn an_app_name_that_strips_to_nothing_is_absent() {
        let (mut state, _rx) = osc99_state();
        let name = freminal_common::base64::encode(b"\x1b");
        osc99_send(&mut state, &format!("f={name}"), "T");

        let notes = osc99_notifications(&state);
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].app_name, None);
    }

    // ── 138.C7: rejected chunks, i=0, plain multi-byte text, g= priming ───────

    /// Metadata for a middle chunk of transfer `id` with 65 items, one more
    /// than the parser accepts.
    fn osc99_over_cap_metadata(id: &str) -> String {
        let filler = vec!["u=1"; 63].join(":");
        format!("i={id}:d=0:{filler}")
    }

    #[test]
    fn a_middle_chunk_over_the_item_cap_tombstones_the_transfer() {
        let (mut state, rx) = osc99_state();
        osc99_send(&mut state, "i=r:d=0", "head ");
        osc99_send(&mut state, &osc99_over_cap_metadata("r"), "middle ");
        assert!(osc99_has_tombstone(&state, "r"));
        state.handler.pending_osc99.assert_costs_consistent();
        assert_eq!(state.handler.pending_osc99.total_cost(), 0);

        // The final chunk would otherwise assemble "head tail" or just "tail".
        osc99_send(&mut state, "i=r", "tail");
        assert_eq!(osc99_titles(&state), Vec::<String>::new());
        assert!(state.handler.pending_osc99.is_empty());
        assert!(osc99_written(&rx).is_empty(), "no reply for a dropped one");

        // The id is reusable afterwards.
        osc99_send(&mut state, "i=r", "fresh");
        assert_eq!(osc99_titles(&state), ["fresh"]);
    }

    #[test]
    fn a_rejected_first_chunk_still_tombstones_the_transfer() {
        let (mut state, _rx) = osc99_state();
        osc99_send(&mut state, &osc99_over_cap_metadata("first"), "head ");
        assert!(osc99_has_tombstone(&state, "first"));
        osc99_send(&mut state, "i=first", "tail");
        assert_eq!(osc99_titles(&state), Vec::<String>::new());
        assert!(state.handler.pending_osc99.is_empty());
    }

    #[test]
    fn a_rejected_final_chunk_ends_the_transfer_without_a_tombstone() {
        let (mut state, _rx) = osc99_state();
        osc99_send(&mut state, "i=r:d=0", "head ");
        // 65 items, none of them `d=0`: the rejected chunk is a final one.
        let metadata = format!("i=r:{}", vec!["u=1"; 64].join(":"));
        osc99_send(&mut state, &metadata, "tail");
        assert_eq!(osc99_titles(&state), Vec::<String>::new());
        assert!(state.handler.pending_osc99.is_empty());
        state.handler.pending_osc99.assert_costs_consistent();
    }

    #[test]
    fn a_rejected_id_less_chunk_abandons_the_unidentified_slot() {
        let (mut state, _rx) = osc99_state();
        // A rejected id-less final chunk with nothing pending is a no-op: no
        // tombstone is left to swallow the next id-less notification.
        let over_cap_final = vec!["u=1"; 65].join(":");
        osc99_send(&mut state, &over_cap_final, "x");
        assert!(state.handler.pending_osc99.is_empty());
        osc99_send(&mut state, "", "shown");
        assert_eq!(osc99_titles(&state), ["shown"]);

        // Something accumulating: it is abandoned, and its tail is dropped.
        osc99_send(&mut state, "d=0", "head ");
        osc99_send(&mut state, &osc99_over_cap_metadata(""), "x");
        assert!(matches!(
            state.handler.pending_osc99.get(None),
            Some(super::super::notify_99::PendingEntry::Dropped)
        ));
        osc99_send(&mut state, "", "tail");
        assert_eq!(osc99_titles(&state), ["shown"]);
        assert!(state.handler.pending_osc99.is_empty());
    }

    #[test]
    fn a_rejected_id_less_first_chunk_tombstones_the_rest_of_the_transfer() {
        let (mut state, rx) = osc99_state();
        // Nothing is pending, yet the rejected `d=0` chunk starts a transfer
        // whose remaining chunks must not assemble a truncated notification.
        osc99_send(&mut state, &osc99_over_cap_metadata(""), "head ");
        assert!(matches!(
            state.handler.pending_osc99.get(None),
            Some(super::super::notify_99::PendingEntry::Dropped)
        ));
        state.handler.pending_osc99.assert_costs_consistent();
        assert_eq!(state.handler.pending_osc99.total_cost(), 0);

        // The final chunk would otherwise show "tail" as a notification.
        osc99_send(&mut state, "d=1", "tail");
        assert_eq!(osc99_titles(&state), Vec::<String>::new());
        assert!(state.handler.pending_osc99.is_empty());
        assert!(osc99_written(&rx).is_empty(), "no reply for a dropped one");

        // The slot is reusable: a fresh id-less notification is displayed.
        osc99_send(&mut state, "", "fresh");
        assert_eq!(osc99_titles(&state), ["fresh"]);
        state.handler.pending_osc99.assert_costs_consistent();
    }

    /// A rejection for size is reachable only by a direct call to the parser:
    /// the OSC accumulator drops any body over 1 MiB before OSC 99 sees it, so
    /// over the wire a sequence this large never gets here. Spec-conforming
    /// senders use chunks of at most 4096 bytes. This test builds the
    /// rejection directly to pin what the handler does with it.
    #[test]
    fn a_rejected_oversized_chunk_tombstones_the_transfer() {
        let mut handler = osc99_supported_handler();
        let first = Osc99Command {
            id: Some("big".to_owned()),
            chunk_state: Osc99ChunkState::More,
            payload: b"head ".to_vec(),
            ..default_osc99()
        };
        handler.process_outputs(&[TerminalOutput::OscResponse(AnsiOscType::Notify99(first))]);
        // What `handle_osc_notify_99` forwards for a sequence over 1 MiB.
        let oversized = freminal_common::buffer_states::osc_notify_99::parse_osc_99(
            b"i=big:d=0",
            &vec![b'x'; 1_048_576],
        )
        .unwrap_err();
        handler.process_outputs(&[TerminalOutput::OscResponse(AnsiOscType::Notify99Rejected(
            oversized.rejected().clone(),
        ))]);
        assert!(matches!(
            handler.pending_osc99.get(Some("big")),
            Some(super::super::notify_99::PendingEntry::Dropped)
        ));
        assert_eq!(handler.window_commands, []);
    }

    #[test]
    fn a_rejected_chunk_while_unsupported_is_dropped_with_the_rest() {
        let mut state = TerminalState::default();
        let metadata = osc99_over_cap_metadata("u");
        osc99_send(&mut state, &metadata, "x");
        assert!(state.handler.pending_osc99.is_empty());
        assert_eq!(state.window_commands, []);
    }

    #[test]
    fn id_zero_is_an_ordinary_id_end_to_end() {
        let (mut state, rx) = osc99_state();

        // The query reply names it, as it does for an id-less query.
        osc99_send(&mut state, "i=0:p=?", "");
        let written = osc99_written(&rx);
        assert!(
            written.starts_with(b"\x1b]99;i=0:p=?;"),
            "unexpected reply: {written:?}"
        );

        // A notification with it is displayed carrying that id, not `None`.
        osc99_send(&mut state, "i=0", "Hello");
        let notes = osc99_notifications(&state);
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].id.as_deref(), Some("0"));
        assert_eq!(notes[0].title, "Hello");
    }

    #[test]
    fn a_plain_multibyte_char_split_across_sequences_reassembles() {
        let (mut state, _rx) = osc99_state();
        // "é" is C3 A9; each half is an invalid UTF-8 fragment by itself.
        state.handle_incoming_data(b"\x1b]99;i=m:d=0;ol\xC3\x1b\\");
        assert_eq!(osc99_titles(&state), Vec::<String>::new());
        state.handle_incoming_data(b"\x1b]99;i=m;\xA9\x1b\\");
        assert_eq!(osc99_titles(&state), ["ol\u{e9}"]);
    }

    #[test]
    fn an_icon_only_notification_with_a_g_key_primes_the_gui_cache() {
        let (mut state, rx) = osc99_state();
        // base64 "AAAA" is three zero bytes.
        osc99_send(&mut state, "i=p:p=icon:e=1:g=my-key", "AAAA");
        assert_eq!(
            state.window_commands,
            [WindowManipulation::Osc99CacheIcon {
                key: "my-key".to_owned(),
                data: vec![0, 0, 0],
            }]
        );
        assert!(osc99_written(&rx).is_empty(), "priming gets no reply");
        assert!(state.handler.pending_osc99.is_empty());
    }

    #[test]
    fn a_g_only_notification_after_priming_is_still_a_normal_notification() {
        let (mut state, _rx) = osc99_state();
        osc99_send(&mut state, "p=icon:e=1:g=k", "AAAA");
        osc99_send(&mut state, "g=k", "Hello");
        let notes = osc99_notifications(&state);
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].icon_cache_key.as_deref(), Some("k"));
        assert_eq!(
            notes[0].icon_data, None,
            "the GUI resolves it from its cache"
        );
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
        // OSC 66 (colour-scheme notification) is recognised but unimplemented.
        let events = feed_and_capture(b"\x1b]66;SECRETPAYLOAD\x07");
        assert_warns_without(&events, "SECRETPAYLOAD");
    }

    #[test]
    fn ignored_dynamic_colour_osc_logs_no_payload() {
        // OSC 13 / 14 / 113 consume their value and are ignored (debug only).
        for seq in [
            &b"\x1b]13;SECRETPAYLOAD\x07"[..],
            b"\x1b]14;SECRETPAYLOAD\x07",
            b"\x1b]113;SECRETPAYLOAD\x07",
        ] {
            let events = feed_and_capture(seq);
            assert!(
                events
                    .iter()
                    .all(|(_, text)| !text.contains("SECRETPAYLOAD")),
                "a log line leaked the payload for {seq:?}: {events:?}"
            );
        }
    }

    #[test]
    fn osc_4_odd_field_count_warn_omits_payload() {
        let events = feed_and_capture(b"\x1b]4;1;red;SECRETPAYLOAD\x07");
        assert_warns_without(&events, "SECRETPAYLOAD");
    }

    #[test]
    fn osc_104_invalid_index_warn_omits_payload() {
        let events = feed_and_capture(b"\x1b]104;SECRETPAYLOAD\x07");
        assert_warns_without(&events, "SECRETPAYLOAD");
    }

    #[test]
    fn osc_4_many_invalid_pairs_warn_once_per_sequence() {
        let mut seq = b"\x1b]4".to_vec();
        for _ in 0..50 {
            seq.extend_from_slice(b";1;notacolor");
        }
        seq.push(0x07);
        let events = feed_and_capture(&seq);
        assert_eq!(warnings(&events).len(), 1, "got: {events:?}");
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
