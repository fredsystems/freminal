// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Types for the Kitty graphics protocol (APC `_G` sequences).
//!
//! Reference: <https://sw.kovidgoyal.net/kitty/graphics-protocol/>
//!
//! The protocol uses APC sequences of the form:
//! `ESC _ G <key=value,…> ; <base64-payload> ESC \`
//!
//! Control data keys are single characters; values are integers or single
//! characters depending on the key.

use std::fmt;

/// Action requested by a Kitty graphics command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KittyAction {
    /// `a=q` — Query whether the terminal supports the protocol.
    Query,
    /// `a=t` — Transmit image data (upload only, no display).
    Transmit,
    /// `a=T` — Transmit and display in one step.
    TransmitAndDisplay,
    /// `a=p` — Display a previously transmitted image.
    Put,
    /// `a=d` — Delete image(s).
    Delete,
    /// `a=f` — Transmit an animation frame.
    AnimationFrame,
    /// `a=a` — Control animation.
    AnimationControl,
    /// `a=c` — Compose animation frames.
    AnimationCompose,
}

/// Pixel data format.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum KittyFormat {
    /// `f=24` — RGB (3 bytes per pixel).
    Rgb,
    /// `f=32` — RGBA (4 bytes per pixel).
    #[default]
    Rgba,
    /// `f=100` — PNG (compressed).
    Png,
}

/// Transmission medium.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum KittyTransmission {
    /// `t=d` — Direct (base64 data inline).
    #[default]
    Direct,
    /// `t=f` — File path (base64-encoded path).
    File,
    /// `t=t` — Temporary file (base64-encoded path, deleted after read).
    TempFile,
    /// `t=s` — Shared memory object name.
    SharedMemory,
}

/// Compression type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KittyCompression {
    /// `o=z` — zlib-compressed payload.
    Zlib,
}

/// Delete target specifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KittyDeleteTarget {
    /// `d=a` — Delete all images visible on screen.
    All,
    /// `d=A` — Delete all images, including those not visible.
    AllIncludingNonVisible,
    /// `d=i` — Delete image with specified ID.
    ById,
    /// `d=I` — Delete newest image with specified ID on cursor or after.
    ByIdCursorOrAfter,
    /// `d=n` — Delete newest image with specified number.
    ByNumber,
    /// `d=N` — Delete newest image with number on cursor or after.
    ByNumberCursorOrAfter,
    /// `d=c` — Delete all images at cursor position.
    AtCursor,
    /// `d=C` — Delete all images at cursor position and after.
    AtCursorAndAfter,
    /// `d=p` — Delete all images that intersect a cell range.
    AtCellRange,
    /// `d=P` — Delete all images that intersect a cell range and after.
    AtCellRangeAndAfter,
    /// `d=x` — Delete all images in column range.
    InColumnRange,
    /// `d=X` — Delete all images in column range and after.
    InColumnRangeAndAfter,
    /// `d=y` — Delete all images in row range.
    InRowRange,
    /// `d=Y` — Delete all images in row range and after.
    InRowRangeAndAfter,
    /// `d=z` — Delete all images at z-index.
    AtZIndex,
    /// `d=Z` — Delete all images at z-index and after.
    AtZIndexAndAfter,
}

/// Parsed control data from a Kitty graphics command.
///
/// All fields are optional; missing keys use protocol defaults.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct KittyControlData {
    /// `a` — Action. Defaults to `Transmit`.
    pub action: Option<KittyAction>,

    /// `q` — Quiet mode: 0 = verbose (default), 1 = suppress OK, 2 = suppress all.
    pub quiet: u8,

    /// `f` — Pixel format.
    pub format: Option<KittyFormat>,

    /// `t` — Transmission medium.
    pub transmission: Option<KittyTransmission>,

    /// `o` — Compression.
    pub compression: Option<KittyCompression>,

    /// `s` — Source image width in pixels.
    pub src_width: Option<u32>,

    /// `v` — Source image height in pixels.
    pub src_height: Option<u32>,

    /// `S` — Total data size in bytes (for chunked transfers).
    pub data_size: Option<u32>,

    /// `i` — Image ID (1–`u32::MAX`). 0 means auto-assign.
    pub image_id: Option<u32>,

    /// `I` — Image number (client-side reference).
    pub image_number: Option<u32>,

    /// `p` — Placement ID.
    pub placement_id: Option<u32>,

    /// `m` — More data flag: 0 = last chunk (default), 1 = more chunks.
    pub more_data: bool,

    /// `c` — Display width in terminal columns.
    pub display_cols: Option<u32>,

    /// `r` — Display height in terminal rows.
    pub display_rows: Option<u32>,

    /// `x` — Left edge of source rectangle in pixels.
    pub src_x: Option<u32>,

    /// `y` — Top edge of source rectangle in pixels.
    pub src_y: Option<u32>,

    /// `w` — Width of source rectangle in pixels.
    pub src_rect_width: Option<u32>,

    /// `h` — Height of source rectangle in pixels.
    pub src_rect_height: Option<u32>,

    /// `X` — Horizontal pixel offset within the cell.
    pub cell_x_offset: Option<u32>,

    /// `Y` — Vertical pixel offset within the cell.
    pub cell_y_offset: Option<u32>,

    /// `z` — Z-index for layering.
    pub z_index: Option<i32>,

    /// `C` — Cursor movement: 0 = move cursor after image (default), 1 = don't move.
    pub no_cursor_movement: bool,

    /// `d` — Delete target (only used when action is `Delete`).
    pub delete_target: Option<KittyDeleteTarget>,

    /// `U` — Unicode placeholder mode: 0 = off (default), 1 = on.
    pub unicode_placeholder: bool,
}

/// A fully parsed Kitty graphics command (control data + payload).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KittyGraphicsCommand {
    /// Parsed control-data key/value pairs.
    pub control: KittyControlData,

    /// Base64-decoded payload bytes.
    ///
    /// Empty if no payload was present (e.g., placement or delete commands).
    pub payload: Vec<u8>,
}

/// Error type for Kitty graphics command parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KittyParseError {
    /// The APC data does not start with `_G` (not a Kitty graphics command).
    NotKittyGraphics,
    /// A key=value pair in the control data is malformed.
    InvalidControlPair(String),
    /// An unrecognized action character.
    UnknownAction(u8),
    /// An unrecognized format value.
    UnknownFormat(u32),
    /// An unrecognized transmission type.
    UnknownTransmission(u8),
    /// An unrecognized delete target.
    UnknownDeleteTarget(u8),
    /// An integer value could not be parsed.
    InvalidInteger(String),
    /// An unrecognized compression type.
    UnknownCompression(u8),
}

impl fmt::Display for KittyParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotKittyGraphics => write!(f, "not a Kitty graphics command"),
            Self::InvalidControlPair(s) => write!(f, "invalid control pair: {s}"),
            Self::UnknownAction(c) => write!(f, "unknown action: {}", *c as char),
            Self::UnknownFormat(n) => write!(f, "unknown format: {n}"),
            Self::UnknownTransmission(c) => write!(f, "unknown transmission: {}", *c as char),
            Self::UnknownDeleteTarget(c) => write!(f, "unknown delete target: {}", *c as char),
            Self::InvalidInteger(s) => write!(f, "invalid integer: {s}"),
            Self::UnknownCompression(c) => write!(f, "unknown compression: {}", *c as char),
        }
    }
}

/// Parse the action character.
const fn parse_action(c: u8) -> Result<KittyAction, KittyParseError> {
    match c {
        b'q' => Ok(KittyAction::Query),
        b't' => Ok(KittyAction::Transmit),
        b'T' => Ok(KittyAction::TransmitAndDisplay),
        b'p' => Ok(KittyAction::Put),
        b'd' => Ok(KittyAction::Delete),
        b'f' => Ok(KittyAction::AnimationFrame),
        b'a' => Ok(KittyAction::AnimationControl),
        b'c' => Ok(KittyAction::AnimationCompose),
        _ => Err(KittyParseError::UnknownAction(c)),
    }
}

/// Parse a `u32` from a byte slice representing an ASCII decimal number.
fn parse_u32(value: &[u8]) -> Result<u32, KittyParseError> {
    let s = std::str::from_utf8(value).map_err(|_| {
        KittyParseError::InvalidInteger(String::from_utf8_lossy(value).into_owned())
    })?;
    s.parse::<u32>()
        .map_err(|_| KittyParseError::InvalidInteger(s.to_owned()))
}

/// Parse an `i32` from a byte slice representing an ASCII decimal number.
fn parse_i32(value: &[u8]) -> Result<i32, KittyParseError> {
    let s = std::str::from_utf8(value).map_err(|_| {
        KittyParseError::InvalidInteger(String::from_utf8_lossy(value).into_owned())
    })?;
    s.parse::<i32>()
        .map_err(|_| KittyParseError::InvalidInteger(s.to_owned()))
}

/// Parse the format value.
fn parse_format(value: &[u8]) -> Result<KittyFormat, KittyParseError> {
    let n = parse_u32(value)?;
    match n {
        24 => Ok(KittyFormat::Rgb),
        32 => Ok(KittyFormat::Rgba),
        100 => Ok(KittyFormat::Png),
        _ => Err(KittyParseError::UnknownFormat(n)),
    }
}

/// Parse the transmission type.
const fn parse_transmission(c: u8) -> Result<KittyTransmission, KittyParseError> {
    match c {
        b'd' => Ok(KittyTransmission::Direct),
        b'f' => Ok(KittyTransmission::File),
        b't' => Ok(KittyTransmission::TempFile),
        b's' => Ok(KittyTransmission::SharedMemory),
        _ => Err(KittyParseError::UnknownTransmission(c)),
    }
}

/// Parse the delete target.
const fn parse_delete_target(c: u8) -> Result<KittyDeleteTarget, KittyParseError> {
    match c {
        b'a' => Ok(KittyDeleteTarget::All),
        b'A' => Ok(KittyDeleteTarget::AllIncludingNonVisible),
        b'i' => Ok(KittyDeleteTarget::ById),
        b'I' => Ok(KittyDeleteTarget::ByIdCursorOrAfter),
        b'n' => Ok(KittyDeleteTarget::ByNumber),
        b'N' => Ok(KittyDeleteTarget::ByNumberCursorOrAfter),
        b'c' => Ok(KittyDeleteTarget::AtCursor),
        b'C' => Ok(KittyDeleteTarget::AtCursorAndAfter),
        b'p' => Ok(KittyDeleteTarget::AtCellRange),
        b'P' => Ok(KittyDeleteTarget::AtCellRangeAndAfter),
        b'x' => Ok(KittyDeleteTarget::InColumnRange),
        b'X' => Ok(KittyDeleteTarget::InColumnRangeAndAfter),
        b'y' => Ok(KittyDeleteTarget::InRowRange),
        b'Y' => Ok(KittyDeleteTarget::InRowRangeAndAfter),
        b'z' => Ok(KittyDeleteTarget::AtZIndex),
        b'Z' => Ok(KittyDeleteTarget::AtZIndexAndAfter),
        _ => Err(KittyParseError::UnknownDeleteTarget(c)),
    }
}

/// Parse a single key=value pair and apply it to `ctrl`.
fn apply_control_pair(
    ctrl: &mut KittyControlData,
    key: u8,
    value: &[u8],
) -> Result<(), KittyParseError> {
    if value.is_empty() {
        return Err(KittyParseError::InvalidControlPair(format!(
            "empty value for key '{}'",
            key as char
        )));
    }

    match key {
        b'a' => ctrl.action = Some(parse_action(value[0])?),
        b'q' => ctrl.quiet = parse_u32(value)?.min(2) as u8,
        b'f' => ctrl.format = Some(parse_format(value)?),
        b't' => ctrl.transmission = Some(parse_transmission(value[0])?),
        b'o' => {
            ctrl.compression = Some(match value[0] {
                b'z' => KittyCompression::Zlib,
                c => return Err(KittyParseError::UnknownCompression(c)),
            });
        }
        b's' => ctrl.src_width = Some(parse_u32(value)?),
        b'v' => ctrl.src_height = Some(parse_u32(value)?),
        b'S' => ctrl.data_size = Some(parse_u32(value)?),
        b'i' => ctrl.image_id = Some(parse_u32(value)?),
        b'I' => ctrl.image_number = Some(parse_u32(value)?),
        b'p' => ctrl.placement_id = Some(parse_u32(value)?),
        b'm' => ctrl.more_data = parse_u32(value)? != 0,
        b'c' => ctrl.display_cols = Some(parse_u32(value)?),
        b'r' => ctrl.display_rows = Some(parse_u32(value)?),
        b'x' => ctrl.src_x = Some(parse_u32(value)?),
        b'y' => ctrl.src_y = Some(parse_u32(value)?),
        b'w' => ctrl.src_rect_width = Some(parse_u32(value)?),
        b'h' => ctrl.src_rect_height = Some(parse_u32(value)?),
        b'X' => ctrl.cell_x_offset = Some(parse_u32(value)?),
        b'Y' => ctrl.cell_y_offset = Some(parse_u32(value)?),
        b'z' => ctrl.z_index = Some(parse_i32(value)?),
        b'C' => ctrl.no_cursor_movement = parse_u32(value)? != 0,
        b'd' => ctrl.delete_target = Some(parse_delete_target(value[0])?),
        b'U' => ctrl.unicode_placeholder = parse_u32(value)? != 0,
        // Unknown keys are silently ignored per the protocol spec.
        _ => {}
    }

    Ok(())
}

/// Parse the control-data portion (before the `;` separator).
fn parse_control_data(data: &[u8]) -> Result<KittyControlData, KittyParseError> {
    let mut ctrl = KittyControlData::default();

    for pair in data.split(|&b| b == b',') {
        if pair.is_empty() {
            continue;
        }

        // Find the '=' separator
        let eq_pos = pair.iter().position(|&b| b == b'=').ok_or_else(|| {
            KittyParseError::InvalidControlPair(String::from_utf8_lossy(pair).into_owned())
        })?;

        if eq_pos == 0 || eq_pos + 1 >= pair.len() {
            return Err(KittyParseError::InvalidControlPair(
                String::from_utf8_lossy(pair).into_owned(),
            ));
        }

        let key = pair[0];
        let value = &pair[eq_pos + 1..];

        apply_control_pair(&mut ctrl, key, value)?;
    }

    Ok(ctrl)
}

/// Strip the APC envelope (`_` prefix and `ESC \` suffix) from the raw
/// sequence bytes produced by `StandardParser`.
///
/// Returns the inner content (everything between the `_` and `ESC \`),
/// or the original slice if the envelope is not present.
fn strip_apc_envelope(apc: &[u8]) -> &[u8] {
    let start = usize::from(apc.first() == Some(&b'_'));
    let end = if apc.len() >= 2 && apc[apc.len() - 2] == 0x1b && apc[apc.len() - 1] == b'\\' {
        apc.len() - 2
    } else {
        apc.len()
    };
    if start <= end { &apc[start..end] } else { &[] }
}

/// Parse a raw APC byte sequence into a `KittyGraphicsCommand`.
///
/// The input `apc` is the raw bytes from `TerminalOutput::ApplicationProgramCommand`,
/// which includes the `_` prefix and `ESC \` suffix from `StandardParser`.
///
/// # Errors
///
/// Returns `KittyParseError` if the sequence is not a Kitty graphics command
/// or if the control data is malformed.
pub fn parse_kitty_graphics(apc: &[u8]) -> Result<KittyGraphicsCommand, KittyParseError> {
    let inner = strip_apc_envelope(apc);

    // Must start with 'G'
    if inner.first() != Some(&b'G') {
        return Err(KittyParseError::NotKittyGraphics);
    }

    let content = &inner[1..];

    // Split on first ';' into control data and payload
    let (control_bytes, payload_b64) = content
        .iter()
        .position(|&b| b == b';')
        .map_or((content, &[] as &[u8]), |semi_pos| {
            (&content[..semi_pos], &content[semi_pos + 1..])
        });

    let control = parse_control_data(control_bytes)?;

    // Decode the base64 payload
    let payload = if payload_b64.is_empty() {
        Vec::new()
    } else {
        let b64_str = std::str::from_utf8(payload_b64).map_err(|_| {
            KittyParseError::InvalidControlPair("payload is not valid UTF-8".to_owned())
        })?;
        crate::base64::decode(b64_str)
            .map_err(|e| KittyParseError::InvalidControlPair(format!("base64 decode error: {e}")))?
    };

    Ok(KittyGraphicsCommand { control, payload })
}

/// Format a Kitty graphics response to be sent back to the PTY.
///
/// The response format is: `ESC _ G i=<id> ; <message> ESC \`
///
/// If `ok` is true, the message is `OK`. Otherwise it is the provided error string.
#[must_use]
pub fn format_kitty_response(image_id: u32, ok: bool, message: &str) -> String {
    let msg = if ok { "OK" } else { message };
    format!("\x1b_Gi={image_id};{msg}\x1b\\")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// Helper to build a raw APC sequence: `_ G <control> ; <payload_b64> ESC \`
    fn make_apc(control: &str, payload_b64: &str) -> Vec<u8> {
        let mut v = Vec::new();
        v.push(b'_');
        v.push(b'G');
        v.extend_from_slice(control.as_bytes());
        if !payload_b64.is_empty() {
            v.push(b';');
            v.extend_from_slice(payload_b64.as_bytes());
        }
        v.push(0x1b);
        v.push(b'\\');
        v
    }

    #[test]
    fn parse_simple_transmit_and_display() {
        let apc = make_apc("a=T,f=100,s=200,v=100,i=42", "iVBOR");
        let cmd = parse_kitty_graphics(&apc).unwrap();

        assert_eq!(cmd.control.action, Some(KittyAction::TransmitAndDisplay));
        assert_eq!(cmd.control.format, Some(KittyFormat::Png));
        assert_eq!(cmd.control.src_width, Some(200));
        assert_eq!(cmd.control.src_height, Some(100));
        assert_eq!(cmd.control.image_id, Some(42));
        // Payload is base64-decoded
        assert!(!cmd.payload.is_empty());
    }

    #[test]
    fn parse_query() {
        let apc = make_apc("a=q,i=1,s=1,v=1,f=32,t=d", "AAAA");
        let cmd = parse_kitty_graphics(&apc).unwrap();

        assert_eq!(cmd.control.action, Some(KittyAction::Query));
        assert_eq!(cmd.control.image_id, Some(1));
        assert_eq!(cmd.control.format, Some(KittyFormat::Rgba));
        assert_eq!(cmd.control.transmission, Some(KittyTransmission::Direct));
    }

    #[test]
    fn parse_delete_all() {
        let apc = make_apc("a=d,d=a", "");
        let cmd = parse_kitty_graphics(&apc).unwrap();

        assert_eq!(cmd.control.action, Some(KittyAction::Delete));
        assert_eq!(cmd.control.delete_target, Some(KittyDeleteTarget::All));
        assert!(cmd.payload.is_empty());
    }

    #[test]
    fn parse_put() {
        let apc = make_apc("a=p,i=42,c=10,r=5,C=1", "");
        let cmd = parse_kitty_graphics(&apc).unwrap();

        assert_eq!(cmd.control.action, Some(KittyAction::Put));
        assert_eq!(cmd.control.image_id, Some(42));
        assert_eq!(cmd.control.display_cols, Some(10));
        assert_eq!(cmd.control.display_rows, Some(5));
        assert!(cmd.control.no_cursor_movement);
    }

    #[test]
    fn parse_chunked_first() {
        let apc = make_apc("a=t,f=100,i=1,m=1", "AAAA");
        let cmd = parse_kitty_graphics(&apc).unwrap();

        assert_eq!(cmd.control.action, Some(KittyAction::Transmit));
        assert!(cmd.control.more_data);
    }

    #[test]
    fn parse_chunked_last() {
        let apc = make_apc("m=0", "BBBB");
        let cmd = parse_kitty_graphics(&apc).unwrap();

        assert!(!cmd.control.more_data);
    }

    #[test]
    fn parse_with_no_payload() {
        let apc = make_apc("a=d,d=i,i=5", "");
        let cmd = parse_kitty_graphics(&apc).unwrap();

        assert!(cmd.payload.is_empty());
    }

    #[test]
    fn parse_defaults() {
        let apc = make_apc("i=1", "AAAA");
        let cmd = parse_kitty_graphics(&apc).unwrap();

        // No explicit action → None (caller should default to Transmit)
        assert_eq!(cmd.control.action, None);
        assert_eq!(cmd.control.quiet, 0);
        assert!(!cmd.control.more_data);
        assert!(!cmd.control.no_cursor_movement);
        assert!(!cmd.control.unicode_placeholder);
    }

    #[test]
    fn parse_quiet_mode() {
        let apc = make_apc("a=t,q=2,i=1", "AAAA");
        let cmd = parse_kitty_graphics(&apc).unwrap();
        assert_eq!(cmd.control.quiet, 2);
    }

    #[test]
    fn parse_quiet_mode_clamped() {
        let apc = make_apc("a=t,q=99,i=1", "AAAA");
        let cmd = parse_kitty_graphics(&apc).unwrap();
        // Should be clamped to 2
        assert_eq!(cmd.control.quiet, 2);
    }

    #[test]
    fn error_not_kitty_graphics() {
        let apc = b"_Xsomething\x1b\\";
        let err = parse_kitty_graphics(apc).unwrap_err();
        assert_eq!(err, KittyParseError::NotKittyGraphics);
    }

    #[test]
    fn error_unknown_action() {
        let apc = make_apc("a=Z", "");
        let err = parse_kitty_graphics(&apc).unwrap_err();
        assert_eq!(err, KittyParseError::UnknownAction(b'Z'));
    }

    #[test]
    fn error_unknown_format() {
        let apc = make_apc("f=99", "AAAA");
        let err = parse_kitty_graphics(&apc).unwrap_err();
        assert_eq!(err, KittyParseError::UnknownFormat(99));
    }

    #[test]
    fn error_unknown_transmission() {
        let apc = make_apc("t=Z", "AAAA");
        let err = parse_kitty_graphics(&apc).unwrap_err();
        assert_eq!(err, KittyParseError::UnknownTransmission(b'Z'));
    }

    #[test]
    fn error_unknown_delete_target() {
        let apc = make_apc("a=d,d=9", "");
        let err = parse_kitty_graphics(&apc).unwrap_err();
        assert_eq!(err, KittyParseError::UnknownDeleteTarget(b'9'));
    }

    #[test]
    fn error_invalid_integer() {
        let apc = make_apc("i=abc", "AAAA");
        let err = parse_kitty_graphics(&apc).unwrap_err();
        assert!(matches!(err, KittyParseError::InvalidInteger(_)));
    }

    #[test]
    fn error_missing_equals() {
        let apc = make_apc("abc", "AAAA");
        let err = parse_kitty_graphics(&apc).unwrap_err();
        assert!(matches!(err, KittyParseError::InvalidControlPair(_)));
    }

    #[test]
    fn error_empty_value() {
        let apc = make_apc("a=", "AAAA");
        let err = parse_kitty_graphics(&apc).unwrap_err();
        assert!(matches!(err, KittyParseError::InvalidControlPair(_)));
    }

    #[test]
    fn strip_apc_envelope_handles_missing_prefix() {
        let data = b"Ga=q\x1b\\";
        let inner = strip_apc_envelope(data);
        assert_eq!(inner, b"Ga=q");
    }

    #[test]
    fn strip_apc_envelope_handles_missing_suffix() {
        let data = b"_Ga=q";
        let inner = strip_apc_envelope(data);
        assert_eq!(inner, b"Ga=q");
    }

    #[test]
    fn format_response_ok() {
        let resp = format_kitty_response(42, true, "");
        assert_eq!(resp, "\x1b_Gi=42;OK\x1b\\");
    }

    #[test]
    fn format_response_error() {
        let resp = format_kitty_response(42, false, "ENOENT:file not found");
        assert_eq!(resp, "\x1b_Gi=42;ENOENT:file not found\x1b\\");
    }

    #[test]
    fn parse_all_delete_targets() {
        let targets = [
            (b'a', KittyDeleteTarget::All),
            (b'A', KittyDeleteTarget::AllIncludingNonVisible),
            (b'i', KittyDeleteTarget::ById),
            (b'I', KittyDeleteTarget::ByIdCursorOrAfter),
            (b'n', KittyDeleteTarget::ByNumber),
            (b'N', KittyDeleteTarget::ByNumberCursorOrAfter),
            (b'c', KittyDeleteTarget::AtCursor),
            (b'C', KittyDeleteTarget::AtCursorAndAfter),
            (b'p', KittyDeleteTarget::AtCellRange),
            (b'P', KittyDeleteTarget::AtCellRangeAndAfter),
            (b'x', KittyDeleteTarget::InColumnRange),
            (b'X', KittyDeleteTarget::InColumnRangeAndAfter),
            (b'y', KittyDeleteTarget::InRowRange),
            (b'Y', KittyDeleteTarget::InRowRangeAndAfter),
            (b'z', KittyDeleteTarget::AtZIndex),
            (b'Z', KittyDeleteTarget::AtZIndexAndAfter),
        ];

        for (ch, expected) in targets {
            let apc = make_apc(&format!("a=d,d={}", ch as char), "");
            let cmd = parse_kitty_graphics(&apc).unwrap();
            assert_eq!(
                cmd.control.delete_target,
                Some(expected),
                "Failed for delete target '{}'",
                ch as char
            );
        }
    }

    #[test]
    fn parse_all_actions() {
        let actions = [
            (b'q', KittyAction::Query),
            (b't', KittyAction::Transmit),
            (b'T', KittyAction::TransmitAndDisplay),
            (b'p', KittyAction::Put),
            (b'd', KittyAction::Delete),
            (b'f', KittyAction::AnimationFrame),
            (b'a', KittyAction::AnimationControl),
            (b'c', KittyAction::AnimationCompose),
        ];

        for (ch, expected) in actions {
            let apc = make_apc(&format!("a={}", ch as char), "");
            let cmd = parse_kitty_graphics(&apc).unwrap();
            assert_eq!(
                cmd.control.action,
                Some(expected),
                "Failed for action '{}'",
                ch as char
            );
        }
    }

    #[test]
    fn parse_all_formats() {
        let formats = [
            ("24", KittyFormat::Rgb),
            ("32", KittyFormat::Rgba),
            ("100", KittyFormat::Png),
        ];

        for (val, expected) in formats {
            let apc = make_apc(&format!("f={val}"), "AAAA");
            let cmd = parse_kitty_graphics(&apc).unwrap();
            assert_eq!(
                cmd.control.format,
                Some(expected),
                "Failed for format '{val}'"
            );
        }
    }

    #[test]
    fn parse_all_transmissions() {
        let transmissions = [
            (b'd', KittyTransmission::Direct),
            (b'f', KittyTransmission::File),
            (b't', KittyTransmission::TempFile),
            (b's', KittyTransmission::SharedMemory),
        ];

        for (ch, expected) in transmissions {
            let apc = make_apc(&format!("t={}", ch as char), "AAAA");
            let cmd = parse_kitty_graphics(&apc).unwrap();
            assert_eq!(
                cmd.control.transmission,
                Some(expected),
                "Failed for transmission '{}'",
                ch as char
            );
        }
    }

    #[test]
    fn parse_z_index_negative() {
        let apc = make_apc("a=p,i=1,z=-5", "");
        let cmd = parse_kitty_graphics(&apc).unwrap();
        assert_eq!(cmd.control.z_index, Some(-5));
    }

    #[test]
    fn parse_compression_zlib() {
        let apc = make_apc("a=t,o=z,i=1", "AAAA");
        let cmd = parse_kitty_graphics(&apc).unwrap();
        assert_eq!(cmd.control.compression, Some(KittyCompression::Zlib));
    }

    #[test]
    fn error_unknown_compression() {
        let apc = make_apc("o=x", "AAAA");
        let err = parse_kitty_graphics(&apc).unwrap_err();
        assert_eq!(err, KittyParseError::UnknownCompression(b'x'));
    }

    #[test]
    fn parse_unicode_placeholder() {
        let apc = make_apc("a=T,U=1,i=1", "AAAA");
        let cmd = parse_kitty_graphics(&apc).unwrap();
        assert!(cmd.control.unicode_placeholder);
    }

    #[test]
    fn parse_source_rect_and_offsets() {
        let apc = make_apc("a=p,i=1,x=10,y=20,w=100,h=50,X=2,Y=3", "");
        let cmd = parse_kitty_graphics(&apc).unwrap();
        assert_eq!(cmd.control.src_x, Some(10));
        assert_eq!(cmd.control.src_y, Some(20));
        assert_eq!(cmd.control.src_rect_width, Some(100));
        assert_eq!(cmd.control.src_rect_height, Some(50));
        assert_eq!(cmd.control.cell_x_offset, Some(2));
        assert_eq!(cmd.control.cell_y_offset, Some(3));
    }

    #[test]
    fn parse_data_size() {
        let apc = make_apc("a=t,S=65536,i=1", "AAAA");
        let cmd = parse_kitty_graphics(&apc).unwrap();
        assert_eq!(cmd.control.data_size, Some(65536));
    }

    #[test]
    fn parse_image_number() {
        let apc = make_apc("I=999", "AAAA");
        let cmd = parse_kitty_graphics(&apc).unwrap();
        assert_eq!(cmd.control.image_number, Some(999));
    }

    #[test]
    fn parse_placement_id() {
        let apc = make_apc("a=p,i=1,p=7", "");
        let cmd = parse_kitty_graphics(&apc).unwrap();
        assert_eq!(cmd.control.placement_id, Some(7));
    }

    #[test]
    fn unknown_keys_are_ignored() {
        // 'Z' is not a known key; should be silently ignored
        let apc = make_apc("a=t,Z=99,i=1", "AAAA");
        let cmd = parse_kitty_graphics(&apc).unwrap();
        assert_eq!(cmd.control.action, Some(KittyAction::Transmit));
        assert_eq!(cmd.control.image_id, Some(1));
    }

    #[test]
    fn display_all_error_variants() {
        let errors = [
            KittyParseError::NotKittyGraphics,
            KittyParseError::InvalidControlPair("test".into()),
            KittyParseError::UnknownAction(b'Z'),
            KittyParseError::UnknownFormat(99),
            KittyParseError::UnknownTransmission(b'Z'),
            KittyParseError::UnknownDeleteTarget(b'9'),
            KittyParseError::InvalidInteger("abc".into()),
            KittyParseError::UnknownCompression(b'x'),
        ];
        for e in &errors {
            let s = format!("{e}");
            assert!(!s.is_empty());
        }
    }

    // --- parse_u32 / parse_i32 with non-UTF-8 bytes ---

    #[test]
    fn parse_u32_non_utf8_bytes() {
        // parse_u32 is private but reachable via apply_control_pair through
        // parse_kitty_graphics. We inject invalid UTF-8 in the value position of
        // a numeric key ('s' = src_width) via a hand-crafted APC.
        // The value bytes \xff\xfe are not valid UTF-8 → InvalidInteger.
        let mut apc = Vec::new();
        apc.push(b'_');
        apc.push(b'G');
        // control data: "s=\xff\xfe"
        apc.extend_from_slice(b"s=");
        apc.push(0xff);
        apc.push(0xfe);
        apc.push(0x1b);
        apc.push(b'\\');
        let err = parse_kitty_graphics(&apc).unwrap_err();
        assert!(matches!(err, KittyParseError::InvalidInteger(_)));
    }

    #[test]
    fn parse_i32_non_utf8_bytes() {
        // Same approach for 'z' (z_index) which uses parse_i32.
        let mut apc = Vec::new();
        apc.push(b'_');
        apc.push(b'G');
        // control data: "z=\xff\xfe"
        apc.extend_from_slice(b"z=");
        apc.push(0xff);
        apc.push(0xfe);
        apc.push(0x1b);
        apc.push(b'\\');
        let err = parse_kitty_graphics(&apc).unwrap_err();
        assert!(matches!(err, KittyParseError::InvalidInteger(_)));
    }

    #[test]
    fn apply_control_pair_empty_value_via_raw_control_data() {
        // Build a sequence where the value is empty AFTER the '=' (i.e. "a=")
        // but a subsequent comma makes the pair non-empty from split perspective.
        // The key 'a' with an empty value must trigger InvalidControlPair from
        // apply_control_pair (line 320), not from the eq_pos check in parse_control_data.
        //
        // To hit apply_control_pair's empty-value check directly we need a pair
        // like "a=," which after splitting on ',' becomes ["a=", ""].
        // For "a=": eq_pos=1, pair.len()=2, eq_pos+1 == pair.len() → hits the
        // parse_control_data guard at line 377, not apply_control_pair.
        //
        // To hit apply_control_pair we need eq_pos+1 < pair.len() but value is
        // still empty — that can't happen with a normal split. Instead we use
        // the fact that `apply_control_pair` checks `value.is_empty()` explicitly.
        // We reach it directly via parse_kitty_graphics with a fabricated key.
        //
        // Craft: "k=x" where k is an unknown key (silently ignored) followed by a
        // leading comma: ",a=t" — the leading comma produces an empty pair which
        // is skipped (the `if pair.is_empty() { continue }` at line 368-370).
        let apc = make_apc(",a=t", "");
        // The leading comma produces an empty pair → continue (no error); "a=t" parses fine.
        let cmd = parse_kitty_graphics(&apc).unwrap();
        assert_eq!(cmd.control.action, Some(KittyAction::Transmit));
    }

    #[test]
    fn parse_control_data_empty_pair_continue() {
        // Trailing comma: ",," in control → two empty pairs, both skipped via `continue`.
        let apc = make_apc("a=t,,i=1", "");
        let cmd = parse_kitty_graphics(&apc).unwrap();
        assert_eq!(cmd.control.action, Some(KittyAction::Transmit));
        assert_eq!(cmd.control.image_id, Some(1));
    }

    #[test]
    fn parse_kitty_graphics_non_utf8_payload() {
        // A payload with non-UTF-8 bytes triggers the UTF-8 check in parse_kitty_graphics.
        let mut apc = Vec::new();
        apc.push(b'_');
        apc.push(b'G');
        apc.extend_from_slice(b"a=t;"); // valid control, then ';' payload separator
        // Non-UTF-8 payload bytes
        apc.push(0xff);
        apc.push(0xfe);
        apc.push(0x1b);
        apc.push(b'\\');
        let err = parse_kitty_graphics(&apc).unwrap_err();
        assert!(matches!(err, KittyParseError::InvalidControlPair(_)));
    }
}
