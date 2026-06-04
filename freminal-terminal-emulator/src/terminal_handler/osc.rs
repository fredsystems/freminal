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
    window_manipulation::WindowManipulation,
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
