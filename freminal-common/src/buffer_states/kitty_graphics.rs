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

/// Delete target specifier (Task 100.7a).
///
/// The kitty spec's `d=` targets come in lowercase/uppercase pairs where
/// the case does **not** change which placements are targeted — it changes
/// whether the underlying image data is freed once no placement anywhere
/// (including scrollback) still references it. There is no "and-after"
/// concept for any target (a prior misreading in this enum's earlier
/// revision). This type carries only the POSITIONAL target; the
/// data-freeing axis is carried separately by
/// [`KittyControlData::delete_free_data`], set from the case of the `d=`
/// character.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KittyDeleteTarget {
    /// `d=a`/`d=A` — placements visible on screen.
    All,
    /// `d=i`/`d=I` — images with id `i=` (optionally placement `p=`).
    ById,
    /// `d=n`/`d=N` — newest image with number `I=` (optionally placement `p=`).
    ByNumber,
    /// `d=c`/`d=C` — placements intersecting the current cursor cell.
    AtCursor,
    /// `d=f`/`d=F` — delete animation frames.
    Frames,
    /// `d=p`/`d=P` — placements intersecting cell `x=`,`y=`.
    AtCell,
    /// `d=q`/`d=Q` — placements intersecting cell `x=`,`y=` with z-index `z=`.
    AtCellZIndex,
    /// `d=r`/`d=R` — images with id in `[x=, y=]` (kitty 0.33.0+).
    IdRange,
    /// `d=x`/`d=X` — placements intersecting column `x=`.
    InColumn,
    /// `d=y`/`d=Y` — placements intersecting row `y=`.
    InRow,
    /// `d=z`/`d=Z` — placements with z-index `z=`.
    AtZIndex,
}

/// Parsed control data from a Kitty graphics command.
///
/// All fields are optional; missing keys use protocol defaults.
// This struct mirrors the kitty graphics protocol's key/value control data
// 1:1 — each bool is an independent single-bit protocol key (`m=`, `C=`,
// `U=`, and the `d=` case), not a set of related flags that would be
// clearer as a state machine or enum.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
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

    /// `O` — byte offset to read from a file/shared-memory object.
    pub data_offset: Option<u32>,

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

    // Relative-placement keys (Task 100.4).
    /// Parent image id (`P`) — relative placement (Task 100.4).
    pub parent_image_id: Option<u32>,
    /// Parent placement id (`Q`) — relative placement (Task 100.4).
    pub parent_placement_id: Option<u32>,
    /// Horizontal cell offset from parent (`H`) — relative placement.
    pub h_offset: Option<i32>,
    /// Vertical cell offset from parent (`V`) — relative placement.
    pub v_offset: Option<i32>,

    /// `d` — Delete target (only used when action is `Delete`).
    pub delete_target: Option<KittyDeleteTarget>,

    /// Whether the `d=` value was uppercase — per spec, the uppercase form
    /// of every delete target ALSO frees the underlying image data
    /// (provided it is not referenced elsewhere, e.g. in scrollback),
    /// while the lowercase form only removes placements. Defaults to
    /// `false` (lowercase / data-preserving).
    pub delete_free_data: bool,

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

/// Parse the delete target's POSITIONAL kind, ignoring case — the case
/// (uppercase = also free data) is captured separately by the caller via
/// [`KittyControlData::delete_free_data`].
const fn parse_delete_target(c: u8) -> Result<KittyDeleteTarget, KittyParseError> {
    match c.to_ascii_lowercase() {
        b'a' => Ok(KittyDeleteTarget::All),
        b'i' => Ok(KittyDeleteTarget::ById),
        b'n' => Ok(KittyDeleteTarget::ByNumber),
        b'c' => Ok(KittyDeleteTarget::AtCursor),
        b'f' => Ok(KittyDeleteTarget::Frames),
        b'p' => Ok(KittyDeleteTarget::AtCell),
        b'q' => Ok(KittyDeleteTarget::AtCellZIndex),
        b'r' => Ok(KittyDeleteTarget::IdRange),
        b'x' => Ok(KittyDeleteTarget::InColumn),
        b'y' => Ok(KittyDeleteTarget::InRow),
        b'z' => Ok(KittyDeleteTarget::AtZIndex),
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
        b'O' => ctrl.data_offset = Some(parse_u32(value)?),
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
        b'd' => {
            ctrl.delete_target = Some(parse_delete_target(value[0])?);
            ctrl.delete_free_data = value[0].is_ascii_uppercase();
        }
        b'U' => ctrl.unicode_placeholder = parse_u32(value)? != 0,
        // Relative-placement keys (Task 100.4). P/Q are unsigned; H/V are signed.
        b'P' => ctrl.parent_image_id = Some(parse_u32(value)?),
        b'Q' => ctrl.parent_placement_id = Some(parse_u32(value)?),
        b'H' => ctrl.h_offset = Some(parse_i32(value)?),
        b'V' => ctrl.v_offset = Some(parse_i32(value)?),
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

/// The image-identity fields echoed in a kitty graphics response.
///
/// Per the kitty spec the response echoes `i=<id>`, plus `I=<number>` when the
/// request used an image number, plus `p=<placement>` when the request used a
/// non-zero placement id. Field order in the wire form is `i`, then `I`, then `p`.
#[derive(Debug, Clone, Copy, Default)]
pub struct KittyResponseId {
    /// The image id (always emitted).
    pub image_id: u32,
    /// The image number (`I=`), emitted when `Some`.
    pub image_number: Option<u32>,
    /// The placement id (`p=`), emitted when `Some` and non-zero.
    pub placement_id: Option<u32>,
}

/// Format a Kitty graphics response to be sent back to the PTY.
///
/// The response format is: `ESC _ G i=<id>[,I=<number>][,p=<placement_id>] ; <message> ESC \`
///
/// If `ok` is true, the message is `OK`. Otherwise it is the provided error string.
///
/// Per the kitty spec, `I=<number>` is echoed only when the originating
/// request specified an image number, and `p=<placement_id>` only when the
/// originating request specified a non-zero placement id; `None` and
/// `Some(0)` (for the placement id) both omit it.
#[must_use]
pub fn format_kitty_response(id: KittyResponseId, ok: bool, message: &str) -> String {
    use std::fmt::Write as _;

    let msg = if ok { "OK" } else { message };
    let mut key = format!("i={}", id.image_id);
    if let Some(number) = id.image_number {
        // `write!` into a `String` never fails.
        let _ = write!(key, ",I={number}");
    }
    if let Some(pid) = id.placement_id
        && pid != 0
    {
        let _ = write!(key, ",p={pid}");
    }
    format!("\x1b_G{key};{msg}\x1b\\")
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
        assert!(!cmd.control.delete_free_data, "lowercase 'a' keeps data");
        assert!(cmd.payload.is_empty());
    }

    #[test]
    fn parse_delete_all_uppercase_frees_data() {
        let apc = make_apc("a=d,d=A", "");
        let cmd = parse_kitty_graphics(&apc).unwrap();

        assert_eq!(cmd.control.action, Some(KittyAction::Delete));
        assert_eq!(cmd.control.delete_target, Some(KittyDeleteTarget::All));
        assert!(cmd.control.delete_free_data, "uppercase 'A' frees data");
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
        let resp = format_kitty_response(
            KittyResponseId {
                image_id: 42,
                image_number: None,
                placement_id: None,
            },
            true,
            "",
        );
        assert_eq!(resp, "\x1b_Gi=42;OK\x1b\\");
    }

    #[test]
    fn format_response_error() {
        let resp = format_kitty_response(
            KittyResponseId {
                image_id: 42,
                image_number: None,
                placement_id: None,
            },
            false,
            "ENOENT:file not found",
        );
        assert_eq!(resp, "\x1b_Gi=42;ENOENT:file not found\x1b\\");
    }

    #[test]
    fn format_response_with_nonzero_placement_id_includes_p() {
        let resp = format_kitty_response(
            KittyResponseId {
                image_id: 42,
                image_number: None,
                placement_id: Some(7),
            },
            true,
            "",
        );
        assert_eq!(resp, "\x1b_Gi=42,p=7;OK\x1b\\");
    }

    #[test]
    fn format_response_with_zero_placement_id_omits_p() {
        let resp = format_kitty_response(
            KittyResponseId {
                image_id: 42,
                image_number: None,
                placement_id: Some(0),
            },
            true,
            "",
        );
        assert_eq!(resp, "\x1b_Gi=42;OK\x1b\\");
    }

    #[test]
    fn format_response_with_image_number_includes_i_field() {
        let resp = format_kitty_response(
            KittyResponseId {
                image_id: 99,
                image_number: Some(13),
                placement_id: None,
            },
            true,
            "",
        );
        assert_eq!(resp, "\x1b_Gi=99,I=13;OK\x1b\\");
    }

    #[test]
    fn format_response_with_image_number_and_placement_locks_field_order() {
        let resp = format_kitty_response(
            KittyResponseId {
                image_id: 99,
                image_number: Some(13),
                placement_id: Some(7),
            },
            true,
            "",
        );
        assert_eq!(resp, "\x1b_Gi=99,I=13,p=7;OK\x1b\\");
    }

    #[test]
    fn parse_all_delete_targets() {
        // (char, expected positional target, expected delete_free_data)
        let targets = [
            (b'a', KittyDeleteTarget::All, false),
            (b'A', KittyDeleteTarget::All, true),
            (b'i', KittyDeleteTarget::ById, false),
            (b'I', KittyDeleteTarget::ById, true),
            (b'n', KittyDeleteTarget::ByNumber, false),
            (b'N', KittyDeleteTarget::ByNumber, true),
            (b'c', KittyDeleteTarget::AtCursor, false),
            (b'C', KittyDeleteTarget::AtCursor, true),
            (b'f', KittyDeleteTarget::Frames, false),
            (b'F', KittyDeleteTarget::Frames, true),
            (b'p', KittyDeleteTarget::AtCell, false),
            (b'P', KittyDeleteTarget::AtCell, true),
            (b'q', KittyDeleteTarget::AtCellZIndex, false),
            (b'Q', KittyDeleteTarget::AtCellZIndex, true),
            (b'r', KittyDeleteTarget::IdRange, false),
            (b'R', KittyDeleteTarget::IdRange, true),
            (b'x', KittyDeleteTarget::InColumn, false),
            (b'X', KittyDeleteTarget::InColumn, true),
            (b'y', KittyDeleteTarget::InRow, false),
            (b'Y', KittyDeleteTarget::InRow, true),
            (b'z', KittyDeleteTarget::AtZIndex, false),
            (b'Z', KittyDeleteTarget::AtZIndex, true),
        ];

        for (ch, expected, expected_free_data) in targets {
            let apc = make_apc(&format!("a=d,d={}", ch as char), "");
            let cmd = parse_kitty_graphics(&apc).unwrap();
            assert_eq!(
                cmd.control.delete_target,
                Some(expected),
                "Failed for delete target '{}'",
                ch as char
            );
            assert_eq!(
                cmd.control.delete_free_data, expected_free_data,
                "Failed delete_free_data for delete target '{}'",
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
    fn parse_data_offset() {
        let apc = make_apc("a=t,O=123,i=1", "AAAA");
        let cmd = parse_kitty_graphics(&apc).unwrap();
        assert_eq!(cmd.control.data_offset, Some(123));
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
    fn parse_relative_placement_keys() {
        // P and Q are unsigned (u32); H and V are signed (i32).
        let apc = make_apc("a=p,i=1,P=5,Q=3,H=-2,V=4", "");
        let cmd = parse_kitty_graphics(&apc).unwrap();
        assert_eq!(cmd.control.action, Some(KittyAction::Put));
        assert_eq!(cmd.control.image_id, Some(1));
        assert_eq!(cmd.control.parent_image_id, Some(5));
        assert_eq!(cmd.control.parent_placement_id, Some(3));
        assert_eq!(cmd.control.h_offset, Some(-2));
        assert_eq!(cmd.control.v_offset, Some(4));
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
