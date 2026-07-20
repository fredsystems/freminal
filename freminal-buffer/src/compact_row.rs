// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! A compact, format-run-sharing representation of a scrollback [`Row`].
//!
//! [`Row`] stores one [`Cell`] per occupied column, and every `Cell` carries
//! its own owned [`FormatTag`] (40 bytes). For scrollback rows — which are
//! rarely mutated once pushed out of the visible viewport — that per-cell
//! `FormatTag` duplication is wasteful: long runs of cells sharing identical
//! formatting (the common case) each pay the full `FormatTag` cost.
//!
//! [`CompactRow`] stores the same information with formatting and wide-glyph
//! bookkeeping run-length-encoded instead of duplicated per cell. Conversion
//! is lossless in both directions: [`CompactRow::from_row`] /
//! [`CompactRow::to_row`] round-trip a compactable row exactly (see the
//! module's test suite).
//!
//! This module is a pure data transform with no knowledge of `Buffer`. It is
//! wired into the scrollback storage path via [`crate::row::Row`]'s
//! `RowStorage::Compact` representation and `Buffer::compact_idle_scrollback`.

use std::sync::Arc;

use conv2::ValueFrom;
use freminal_common::{
    buffer_states::{
        cursor::{ReverseVideo, StateColors},
        fonts::{BlinkState, FontDecorationFlags, FontDecorations, FontWeight, UnderlineStyle},
        format_tag::FormatTag,
        tchar::TChar,
        url::Url,
    },
    colors::TerminalColor,
};

use crate::{
    cell::Cell,
    row::{LineWidth, Row, RowJoin, RowOrigin},
};

// Bit flags used to pack a cell's wide-glyph bookkeeping into a single byte
// for run-length encoding.
const WIDE_FLAG_HEAD: u8 = 0b01;
const WIDE_FLAG_CONTINUATION: u8 = 0b10;

/// Returns `true` if `row` can be losslessly represented as a [`CompactRow`].
///
/// Rows containing any cell with an inline image placement are excluded:
/// image placements are per-cell data that doesn't benefit from format-run
/// sharing, and keeping them out of `CompactRow` keeps its representation
/// simple (no `Option<Box<ImagePlacement>>` bookkeeping to carry through the
/// run-length encoding). Callers that need to compact scrollback rows should
/// check this before calling [`CompactRow::from_row`], though `from_row`
/// itself also performs this check and returns `None` rather than silently
/// dropping image data.
#[must_use]
pub fn is_compactable(row: &Row) -> bool {
    !row.cells().iter().any(Cell::has_image)
}

/// A compact, format-run-sharing representation of a scrollback [`Row`].
///
/// See the module documentation for the motivation. Construct via
/// [`CompactRow::from_row`] and rebuild an equivalent `Row` via
/// [`CompactRow::to_row`].
#[derive(Debug, Clone)]
pub struct CompactRow {
    /// One [`TChar`] per stored cell, in column order. Unlike `Row`, there is
    /// no wide-continuation special-casing here: continuation cells store
    /// whatever `TChar` they actually carried (almost always `TChar::Space`),
    /// and their wide-glyph role is recovered from `wide_runs`.
    chars: Vec<TChar>,
    /// Run-length-encoded formatting: `(tag, run_length)`. Adjacent cells
    /// carrying `==` tags are coalesced into a single run. The sum of all
    /// `run_length`s equals `chars.len()`.
    tag_runs: Vec<(FormatTag, u32)>,
    /// Run-length-encoded wide-glyph bookkeeping: `(flags, run_length)`,
    /// where `flags` packs `is_wide_head` (bit 0) and `is_wide_continuation`
    /// (bit 1). The sum of all `run_length`s equals `chars.len()`.
    wide_runs: Vec<(u8, u32)>,
    width: usize,
    origin: RowOrigin,
    join: RowJoin,
    line_width: LineWidth,
}

/// Pack a cell's wide-glyph flags into a single byte for run-length coding.
const fn wide_flags_of(cell: &Cell) -> u8 {
    let mut flags = 0u8;
    if cell.is_head() {
        flags |= WIDE_FLAG_HEAD;
    }
    if cell.is_continuation() {
        flags |= WIDE_FLAG_CONTINUATION;
    }
    flags
}

/// Append `value` to a run-length-encoded vec, coalescing with the last run
/// if `value` matches it, or starting a new run of length 1 otherwise.
///
/// Run lengths grow one cell at a time (via `saturating_add(1)`), so no
/// `usize -> u32` conversion is ever needed on this path: a run's length can
/// only ever be one more than the previous count, never an arbitrary `usize`
/// total. If a single run somehow exceeded `u32::MAX` cells (not physically
/// possible for a terminal row width), the run length simply saturates at
/// `u32::MAX` rather than panicking or wrapping.
fn push_run<T: PartialEq + Clone>(runs: &mut Vec<(T, u32)>, value: T) {
    if let Some(last) = runs.last_mut()
        && last.0 == value
    {
        last.1 = last.1.saturating_add(1);
        return;
    }
    runs.push((value, 1));
}

/// Expand a run-length-encoded vec back into a flat per-cell iterator.
///
/// `u32 -> usize` is lossless on every platform this crate targets (desktop
/// platforms, where `usize` is at least 32 bits), so the conversion cannot
/// fail in practice. The `unwrap_or(0)` fallback is unreachable there; it
/// degrades a hypothetical failure to "contribute zero cells from this run"
/// rather than panicking or under/over-allocating.
fn expand_runs<T>(runs: &[(T, u32)]) -> impl Iterator<Item = &T> {
    runs.iter().flat_map(|(value, len)| {
        let count = usize::value_from(*len).unwrap_or(0);
        std::iter::repeat_n(value, count)
    })
}

impl CompactRow {
    /// Build a `CompactRow` from `row`, or `None` if `row` contains any
    /// image cell (see [`is_compactable`]).
    #[must_use]
    pub fn from_row(row: &Row) -> Option<Self> {
        let cells = row.cells();
        if cells.iter().any(Cell::has_image) {
            return None;
        }

        let mut chars = Vec::with_capacity(cells.len());
        let mut tag_runs: Vec<(FormatTag, u32)> = Vec::new();
        let mut wide_runs: Vec<(u8, u32)> = Vec::new();

        for cell in cells {
            chars.push(*cell.tchar());
            push_run(&mut tag_runs, cell.tag().clone());
            push_run(&mut wide_runs, wide_flags_of(cell));
        }

        Some(Self {
            chars,
            tag_runs,
            wide_runs,
            width: row.max_width(),
            origin: row.origin,
            join: row.join,
            line_width: row.line_width,
        })
    }

    /// Rebuild an equivalent [`Row`] from this compact representation.
    ///
    /// The rebuilt row's cell contents (value, format, wide-glyph flags),
    /// width, origin, join, and line-width are exactly equal to the source
    /// row's. The rebuilt row's `dirty` flag is *not* part of this identity:
    /// `dirty` is a cache-staleness marker, not row content, and
    /// [`Row::from_cells`] always constructs with `dirty: true`.
    #[must_use]
    pub fn to_row(&self) -> Row {
        let tags = expand_runs(&self.tag_runs);
        let wide_flags = expand_runs(&self.wide_runs);

        let cells: Vec<Cell> = self
            .chars
            .iter()
            .zip(tags)
            .zip(wide_flags)
            .map(|((value, tag), flags)| {
                let is_head = flags & WIDE_FLAG_HEAD != 0;
                let is_continuation = flags & WIDE_FLAG_CONTINUATION != 0;
                Cell::from_parts(*value, tag.clone(), is_head, is_continuation)
            })
            .collect();

        let mut row = Row::from_cells(self.width, self.origin, self.join, cells);
        row.line_width = self.line_width;
        row
    }

    /// Heap bytes retained by this `CompactRow`'s backing allocations,
    /// computed from allocation *capacity* (the real resident cost), not
    /// `len()`. Mirrors the accounting style of `Buffer::heap_bytes`.
    #[must_use]
    pub const fn heap_bytes(&self) -> usize {
        self.chars.capacity() * core::mem::size_of::<TChar>()
            + self.tag_runs.capacity() * core::mem::size_of::<(FormatTag, u32)>()
            + self.wide_runs.capacity() * core::mem::size_of::<(u8, u32)>()
    }

    /// Number of distinct format runs. Exposed for tests/diagnostics that
    /// want to verify run-coalescing behavior directly.
    #[must_use]
    pub const fn tag_run_count(&self) -> usize {
        self.tag_runs.len()
    }

    /// Number of stored cells (one `TChar` per stored cell), without
    /// decompacting. Equals the source row's `cells().len()`.
    #[must_use]
    pub const fn stored_cell_count(&self) -> usize {
        self.chars.len()
    }

    /// Serialize this row to bytes for LZ4 block compression
    /// ([`crate::compressed_block::CompressedBlock`], Task 119).
    ///
    /// Manual little-endian encoding (matching the convention already used
    /// in `freminal-terminal-emulator/src/recording.rs`) rather than
    /// pulling in a serde/bincode dependency. See [`CompactRow::from_bytes`]
    /// for the inverse and the module's test suite for the round-trip
    /// coverage matrix.
    ///
    /// `FormatTag::start`/`FormatTag::end` are deliberately **not**
    /// serialized. Those fields are flatten-time positional bookkeeping
    /// (see `FormatTag`'s doc comment: they index into the flat `Vec<TChar>`
    /// produced by `Buffer::visible_as_tchars_and_tags`) — every `Cell`
    /// actually stored in a `Row`/`CompactRow` carries a tag derived from
    /// `TerminalHandler::current_format`, which starts as
    /// `FormatTag::default()` and is only ever mutated on its visual fields
    /// (colors, weight, decorations, url, blink) by SGR handling; `start`/
    /// `end` are never touched there and remain `(0, usize::MAX)` for the
    /// lifetime of every stored cell. `start`/`end` are populated only in
    /// the separate, ephemeral `FormatTag`s built by `buffer/flatten.rs`
    /// from the flat character vector — a different `FormatTag` value that
    /// never reaches `Cell`/`CompactRow` storage. Confirmed by this
    /// module's own `assert_round_trip_exact` (used by every round-trip
    /// test below): it compares full `Cell` equality — which includes the
    /// full `FormatTag`, `start`/`end` included, via `Cell`'s derived
    /// `PartialEq` — and passes for every stored tag despite `start`/`end`
    /// never being serialized, because they are always the defaults on the
    /// stored side already.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();

        // Every count below (`chars.len()`, `tag_runs.len()`,
        // `wide_runs.len()`) is bounded by a single row's column width —
        // nowhere near `u32::MAX` in practice. Degrade to `u32::MAX` rather
        // than panicking if it somehow were not, mirroring the
        // `unwrap_or(0)` convention `expand_runs` already established.
        let char_count = u32::value_from(self.chars.len()).unwrap_or(u32::MAX);
        out.extend_from_slice(&char_count.to_le_bytes());
        for c in &self.chars {
            encode_tchar(c, &mut out);
        }

        let tag_run_count = u32::value_from(self.tag_runs.len()).unwrap_or(u32::MAX);
        out.extend_from_slice(&tag_run_count.to_le_bytes());
        for (tag, len) in &self.tag_runs {
            encode_format_tag(tag, &mut out);
            out.extend_from_slice(&len.to_le_bytes());
        }

        let wide_run_count = u32::value_from(self.wide_runs.len()).unwrap_or(u32::MAX);
        out.extend_from_slice(&wide_run_count.to_le_bytes());
        for (flags, len) in &self.wide_runs {
            out.push(*flags);
            out.extend_from_slice(&len.to_le_bytes());
        }

        let width = u64::value_from(self.width).unwrap_or(u64::MAX);
        out.extend_from_slice(&width.to_le_bytes());
        out.push(encode_row_origin(self.origin));
        out.push(encode_row_join(self.join));
        out.push(encode_line_width(self.line_width));

        out
    }

    /// Decode a single `CompactRow` from the start of `bytes`.
    ///
    /// Returns the decoded row and the number of bytes consumed from the
    /// front of `bytes` (so callers decoding several concatenated rows —
    /// e.g. [`crate::compressed_block::CompressedBlock`] — can slice past
    /// it and decode the next one), or `None` if `bytes` is
    /// malformed/truncated. Every field read is bounds-checked; this never
    /// panics or indexes out of bounds on malformed input.
    #[must_use]
    pub fn from_bytes(bytes: &[u8]) -> Option<(Self, usize)> {
        let mut pos = 0usize;

        let char_count = usize::value_from(read_u32(bytes, &mut pos)?).unwrap_or(0);
        let mut chars = Vec::with_capacity(char_count.min(bytes.len()));
        for _ in 0..char_count {
            chars.push(decode_tchar(bytes, &mut pos)?);
        }

        let tag_run_count = usize::value_from(read_u32(bytes, &mut pos)?).unwrap_or(0);
        let mut tag_runs = Vec::with_capacity(tag_run_count.min(bytes.len()));
        for _ in 0..tag_run_count {
            let tag = decode_format_tag(bytes, &mut pos)?;
            let len = read_u32(bytes, &mut pos)?;
            tag_runs.push((tag, len));
        }

        let wide_run_count = usize::value_from(read_u32(bytes, &mut pos)?).unwrap_or(0);
        let mut wide_runs = Vec::with_capacity(wide_run_count.min(bytes.len()));
        for _ in 0..wide_run_count {
            let flags = read_u8(bytes, &mut pos)?;
            let len = read_u32(bytes, &mut pos)?;
            wide_runs.push((flags, len));
        }

        let width = usize::value_from(read_u64(bytes, &mut pos)?).unwrap_or(usize::MAX);
        let origin = decode_row_origin(read_u8(bytes, &mut pos)?)?;
        let join = decode_row_join(read_u8(bytes, &mut pos)?)?;
        let line_width = decode_line_width(read_u8(bytes, &mut pos)?)?;

        Some((
            Self {
                chars,
                tag_runs,
                wide_runs,
                width,
                origin,
                join,
                line_width,
            },
            pos,
        ))
    }
}

// ===========================================================================
// Byte-cursor primitives shared by every field encoder/decoder below.
// ===========================================================================

/// Read a single byte at the cursor, advancing it by one. `None` if `pos` is
/// at or past the end of `bytes`.
fn read_u8(bytes: &[u8], pos: &mut usize) -> Option<u8> {
    let b = *bytes.get(*pos)?;
    *pos += 1;
    Some(b)
}

/// Read `len` bytes at the cursor, advancing it by `len`. `None` if fewer
/// than `len` bytes remain, or if `pos + len` would overflow `usize`
/// (guards against a malformed/malicious length field causing an overflow
/// panic in range indexing).
fn read_bytes<'a>(bytes: &'a [u8], pos: &mut usize, len: usize) -> Option<&'a [u8]> {
    let end = pos.checked_add(len)?;
    let slice = bytes.get(*pos..end)?;
    *pos = end;
    Some(slice)
}

/// Read a little-endian `u32` at the cursor, advancing it by 4.
fn read_u32(bytes: &[u8], pos: &mut usize) -> Option<u32> {
    let slice = read_bytes(bytes, pos, 4)?;
    let arr: [u8; 4] = slice.try_into().ok()?;
    Some(u32::from_le_bytes(arr))
}

/// Read a little-endian `u64` at the cursor, advancing it by 8.
fn read_u64(bytes: &[u8], pos: &mut usize) -> Option<u64> {
    let slice = read_bytes(bytes, pos, 8)?;
    let arr: [u8; 8] = slice.try_into().ok()?;
    Some(u64::from_le_bytes(arr))
}

// ===========================================================================
// `TChar` encoding
// ===========================================================================

/// Byte tags identifying which `TChar` variant follows.
///
/// `TChar::Ascii(b' ')` and `TChar::Space` (likewise `Ascii(b'\n')` and
/// `TChar::NewLine`) are byte-identical via `as_bytes()`, so the variant
/// must be explicitly tagged rather than inferred from the payload byte —
/// otherwise decoding would silently normalize an `Ascii` space/newline
/// into the dedicated `Space`/`NewLine` variant.
const TCHAR_TAG_ASCII: u8 = 0;
const TCHAR_TAG_UTF8: u8 = 1;
const TCHAR_TAG_SPACE: u8 = 2;
const TCHAR_TAG_NEWLINE: u8 = 3;

fn encode_tchar(value: &TChar, out: &mut Vec<u8>) {
    match value {
        TChar::Ascii(b) => {
            out.push(TCHAR_TAG_ASCII);
            out.push(*b);
        }
        TChar::Utf8(_, _) => {
            let bytes = value.as_bytes();
            out.push(TCHAR_TAG_UTF8);
            // `bytes.len()` is bounded by `TCHAR_MAX_UTF8_LEN` (16), so
            // `u8::value_from` cannot fail in practice; degrade to an
            // (unreachable) empty-payload length rather than panicking.
            let len = u8::value_from(bytes.len()).unwrap_or(0);
            out.push(len);
            out.extend_from_slice(bytes);
        }
        TChar::Space => out.push(TCHAR_TAG_SPACE),
        TChar::NewLine => out.push(TCHAR_TAG_NEWLINE),
    }
}

fn decode_tchar(bytes: &[u8], pos: &mut usize) -> Option<TChar> {
    match read_u8(bytes, pos)? {
        TCHAR_TAG_ASCII => Some(TChar::Ascii(read_u8(bytes, pos)?)),
        TCHAR_TAG_UTF8 => {
            let len = usize::from(read_u8(bytes, pos)?);
            let payload = read_bytes(bytes, pos, len)?;
            // Reconstruct explicitly via `new_from_many_chars`, never
            // `new_from_single_char`/`From<char>` — those would alias a
            // single-byte space/newline payload back to `Space`/`NewLine`
            // instead of the `Utf8` variant the tag byte says was stored.
            TChar::new_from_many_chars(payload).ok()
        }
        TCHAR_TAG_SPACE => Some(TChar::Space),
        TCHAR_TAG_NEWLINE => Some(TChar::NewLine),
        _ => None,
    }
}

// ===========================================================================
// `TerminalColor` encoding
// ===========================================================================

const COLOR_TAG_DEFAULT: u8 = 0;
const COLOR_TAG_DEFAULT_BACKGROUND: u8 = 1;
const COLOR_TAG_DEFAULT_UNDERLINE: u8 = 2;
const COLOR_TAG_DEFAULT_CURSOR: u8 = 3;
const COLOR_TAG_BLACK: u8 = 4;
const COLOR_TAG_RED: u8 = 5;
const COLOR_TAG_GREEN: u8 = 6;
const COLOR_TAG_YELLOW: u8 = 7;
const COLOR_TAG_BLUE: u8 = 8;
const COLOR_TAG_MAGENTA: u8 = 9;
const COLOR_TAG_CYAN: u8 = 10;
const COLOR_TAG_WHITE: u8 = 11;
const COLOR_TAG_BRIGHT_YELLOW: u8 = 12;
const COLOR_TAG_BRIGHT_BLACK: u8 = 13;
const COLOR_TAG_BRIGHT_RED: u8 = 14;
const COLOR_TAG_BRIGHT_GREEN: u8 = 15;
const COLOR_TAG_BRIGHT_BLUE: u8 = 16;
const COLOR_TAG_BRIGHT_MAGENTA: u8 = 17;
const COLOR_TAG_BRIGHT_CYAN: u8 = 18;
const COLOR_TAG_BRIGHT_WHITE: u8 = 19;
const COLOR_TAG_CUSTOM: u8 = 20;
const COLOR_TAG_PALETTE_INDEX: u8 = 21;

fn encode_terminal_color(color: TerminalColor, out: &mut Vec<u8>) {
    match color {
        TerminalColor::Default => out.push(COLOR_TAG_DEFAULT),
        TerminalColor::DefaultBackground => out.push(COLOR_TAG_DEFAULT_BACKGROUND),
        TerminalColor::DefaultUnderlineColor => out.push(COLOR_TAG_DEFAULT_UNDERLINE),
        TerminalColor::DefaultCursorColor => out.push(COLOR_TAG_DEFAULT_CURSOR),
        TerminalColor::Black => out.push(COLOR_TAG_BLACK),
        TerminalColor::Red => out.push(COLOR_TAG_RED),
        TerminalColor::Green => out.push(COLOR_TAG_GREEN),
        TerminalColor::Yellow => out.push(COLOR_TAG_YELLOW),
        TerminalColor::Blue => out.push(COLOR_TAG_BLUE),
        TerminalColor::Magenta => out.push(COLOR_TAG_MAGENTA),
        TerminalColor::Cyan => out.push(COLOR_TAG_CYAN),
        TerminalColor::White => out.push(COLOR_TAG_WHITE),
        TerminalColor::BrightYellow => out.push(COLOR_TAG_BRIGHT_YELLOW),
        TerminalColor::BrightBlack => out.push(COLOR_TAG_BRIGHT_BLACK),
        TerminalColor::BrightRed => out.push(COLOR_TAG_BRIGHT_RED),
        TerminalColor::BrightGreen => out.push(COLOR_TAG_BRIGHT_GREEN),
        TerminalColor::BrightBlue => out.push(COLOR_TAG_BRIGHT_BLUE),
        TerminalColor::BrightMagenta => out.push(COLOR_TAG_BRIGHT_MAGENTA),
        TerminalColor::BrightCyan => out.push(COLOR_TAG_BRIGHT_CYAN),
        TerminalColor::BrightWhite => out.push(COLOR_TAG_BRIGHT_WHITE),
        TerminalColor::Custom(r, g, b) => {
            out.push(COLOR_TAG_CUSTOM);
            out.push(r);
            out.push(g);
            out.push(b);
        }
        TerminalColor::PaletteIndex(idx) => {
            out.push(COLOR_TAG_PALETTE_INDEX);
            out.push(idx);
        }
    }
}

fn decode_terminal_color(bytes: &[u8], pos: &mut usize) -> Option<TerminalColor> {
    match read_u8(bytes, pos)? {
        COLOR_TAG_DEFAULT => Some(TerminalColor::Default),
        COLOR_TAG_DEFAULT_BACKGROUND => Some(TerminalColor::DefaultBackground),
        COLOR_TAG_DEFAULT_UNDERLINE => Some(TerminalColor::DefaultUnderlineColor),
        COLOR_TAG_DEFAULT_CURSOR => Some(TerminalColor::DefaultCursorColor),
        COLOR_TAG_BLACK => Some(TerminalColor::Black),
        COLOR_TAG_RED => Some(TerminalColor::Red),
        COLOR_TAG_GREEN => Some(TerminalColor::Green),
        COLOR_TAG_YELLOW => Some(TerminalColor::Yellow),
        COLOR_TAG_BLUE => Some(TerminalColor::Blue),
        COLOR_TAG_MAGENTA => Some(TerminalColor::Magenta),
        COLOR_TAG_CYAN => Some(TerminalColor::Cyan),
        COLOR_TAG_WHITE => Some(TerminalColor::White),
        COLOR_TAG_BRIGHT_YELLOW => Some(TerminalColor::BrightYellow),
        COLOR_TAG_BRIGHT_BLACK => Some(TerminalColor::BrightBlack),
        COLOR_TAG_BRIGHT_RED => Some(TerminalColor::BrightRed),
        COLOR_TAG_BRIGHT_GREEN => Some(TerminalColor::BrightGreen),
        COLOR_TAG_BRIGHT_BLUE => Some(TerminalColor::BrightBlue),
        COLOR_TAG_BRIGHT_MAGENTA => Some(TerminalColor::BrightMagenta),
        COLOR_TAG_BRIGHT_CYAN => Some(TerminalColor::BrightCyan),
        COLOR_TAG_BRIGHT_WHITE => Some(TerminalColor::BrightWhite),
        COLOR_TAG_CUSTOM => {
            let r = read_u8(bytes, pos)?;
            let g = read_u8(bytes, pos)?;
            let b = read_u8(bytes, pos)?;
            Some(TerminalColor::Custom(r, g, b))
        }
        COLOR_TAG_PALETTE_INDEX => Some(TerminalColor::PaletteIndex(read_u8(bytes, pos)?)),
        _ => None,
    }
}

// ===========================================================================
// `FontDecorationFlags` / `FontWeight` / `ReverseVideo` / `BlinkState` encoding
// ===========================================================================

const FONT_DECO_ITALIC_BIT: u8 = 0b0000_0001;
const FONT_DECO_FAINT_BIT: u8 = 0b0000_0010;
const FONT_DECO_STRIKETHROUGH_BIT: u8 = 0b0000_0100;
const FONT_DECO_UNDERLINE_SHIFT: u8 = 3;

/// Encode via `FontDecorationFlags`'s public API (`contains`/
/// `underline_style`) rather than reaching into its private inner `u8` —
/// that bitfield is a `freminal-common` implementation detail, not a
/// stable wire format, and the two happen to already look similar only by
/// coincidence.
const fn encode_font_decorations(flags: FontDecorationFlags) -> u8 {
    let mut byte = 0u8;
    if flags.contains(FontDecorations::Italic) {
        byte |= FONT_DECO_ITALIC_BIT;
    }
    if flags.contains(FontDecorations::Faint) {
        byte |= FONT_DECO_FAINT_BIT;
    }
    if flags.contains(FontDecorations::Strikethrough) {
        byte |= FONT_DECO_STRIKETHROUGH_BIT;
    }
    let underline_bits: u8 = match flags.underline_style() {
        UnderlineStyle::None => 0,
        UnderlineStyle::Single => 1,
        UnderlineStyle::Double => 2,
        UnderlineStyle::Curly => 3,
        UnderlineStyle::Dotted => 4,
        UnderlineStyle::Dashed => 5,
    };
    byte | (underline_bits << FONT_DECO_UNDERLINE_SHIFT)
}

/// Total: any byte value decodes to *some* `FontDecorationFlags` (unknown
/// underline bit patterns fall back to `UnderlineStyle::None` via
/// `set_underline_style`), so this never fails.
const fn decode_font_decorations(byte: u8) -> FontDecorationFlags {
    let mut flags = FontDecorationFlags::empty();
    if byte & FONT_DECO_ITALIC_BIT != 0 {
        flags.insert(FontDecorations::Italic);
    }
    if byte & FONT_DECO_FAINT_BIT != 0 {
        flags.insert(FontDecorations::Faint);
    }
    if byte & FONT_DECO_STRIKETHROUGH_BIT != 0 {
        flags.insert(FontDecorations::Strikethrough);
    }
    let underline_bits = byte >> FONT_DECO_UNDERLINE_SHIFT;
    let style = match underline_bits {
        1 => UnderlineStyle::Single,
        2 => UnderlineStyle::Double,
        3 => UnderlineStyle::Curly,
        4 => UnderlineStyle::Dotted,
        5 => UnderlineStyle::Dashed,
        _ => UnderlineStyle::None,
    };
    flags.set_underline_style(style);
    flags
}

const fn encode_font_weight(weight: FontWeight) -> u8 {
    match weight {
        FontWeight::Normal => 0,
        FontWeight::Bold => 1,
    }
}

const fn decode_font_weight(byte: u8) -> Option<FontWeight> {
    match byte {
        0 => Some(FontWeight::Normal),
        1 => Some(FontWeight::Bold),
        _ => None,
    }
}

const fn encode_reverse_video(reverse_video: ReverseVideo) -> u8 {
    match reverse_video {
        ReverseVideo::Off => 0,
        ReverseVideo::On => 1,
    }
}

const fn decode_reverse_video(byte: u8) -> Option<ReverseVideo> {
    match byte {
        0 => Some(ReverseVideo::Off),
        1 => Some(ReverseVideo::On),
        _ => None,
    }
}

const fn encode_blink(state: BlinkState) -> u8 {
    match state {
        BlinkState::None => 0,
        BlinkState::Slow => 1,
        BlinkState::Fast => 2,
    }
}

const fn decode_blink(byte: u8) -> Option<BlinkState> {
    match byte {
        0 => Some(BlinkState::None),
        1 => Some(BlinkState::Slow),
        2 => Some(BlinkState::Fast),
        _ => None,
    }
}

// ===========================================================================
// `Url` encoding
// ===========================================================================

fn encode_string(s: &str, out: &mut Vec<u8>) {
    let bytes = s.as_bytes();
    // A hyperlink URL/id is bounded by realistic OSC 8 payload sizes, far
    // below `u32::MAX`; degrade to a truncated length rather than
    // panicking if it somehow were not (this would only ever lose the
    // ability to fully reconstruct a pathological multi-gigabyte URL —
    // not something a real terminal session produces).
    let len = u32::value_from(bytes.len()).unwrap_or(u32::MAX);
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(bytes);
}

fn decode_string(bytes: &[u8], pos: &mut usize) -> Option<String> {
    let len = usize::value_from(read_u32(bytes, pos)?).unwrap_or(usize::MAX);
    let payload = read_bytes(bytes, pos, len)?;
    String::from_utf8(payload.to_vec()).ok()
}

fn encode_optional_string(s: Option<&str>, out: &mut Vec<u8>) {
    match s {
        None => out.push(0),
        Some(s) => {
            out.push(1);
            encode_string(s, out);
        }
    }
}

fn encode_url(url: Option<&Url>, out: &mut Vec<u8>) {
    match url {
        None => out.push(0),
        Some(u) => {
            out.push(1);
            encode_optional_string(u.id.as_deref(), out);
            encode_string(&u.url, out);
        }
    }
}

// ===========================================================================
// `FormatTag` encoding
// ===========================================================================

fn encode_format_tag(tag: &FormatTag, out: &mut Vec<u8>) {
    // `start`/`end` are deliberately not serialized — see the doc comment
    // on `CompactRow::to_bytes` for the full evidence trail. That safety
    // rests on the invariant that every `Cell`-resident `FormatTag` carries
    // the default `(0, usize::MAX)` range. Assert it in debug builds so a
    // future change that ever puts a non-default range on a stored cell's
    // tag fails loudly here rather than silently normalizing it away.
    debug_assert!(
        tag.start == 0 && tag.end == usize::MAX,
        "CompactRow serialization drops FormatTag::start/end; a stored cell \
         tag with a non-default range ({}, {}) would be silently normalized",
        tag.start,
        tag.end,
    );
    encode_terminal_color(tag.colors.color, out);
    encode_terminal_color(tag.colors.background_color, out);
    encode_terminal_color(tag.colors.underline_color, out);
    out.push(encode_reverse_video(tag.colors.reverse_video));
    out.push(encode_font_weight(tag.font_weight));
    out.push(encode_font_decorations(tag.font_decorations));
    encode_url(tag.url.as_deref(), out);
    out.push(encode_blink(tag.blink));
}

/// Decode the `url: Option<Arc<Url>>` field written by [`encode_url`].
///
/// Inlined directly into [`decode_format_tag`] rather than factored into a
/// standalone `decode_url` helper: the field's own type is already
/// `Option<Arc<Url>>`, so a helper mirroring the rest of this module's
/// "`Option<T>`, `None` means malformed" convention would need to return
/// `Option<Option<Arc<Url>>>` — clippy's `option_option` lint (correctly)
/// flags that shape. Keeping the presence-byte match inline here means the
/// single `Option<Arc<Url>>` produced is unambiguous: it *is* the decoded
/// field value, and a malformed/truncated input instead short-circuits the
/// enclosing `decode_format_tag` via `return None` before reaching the
/// point where it would need to be wrapped again.
fn decode_format_tag(bytes: &[u8], pos: &mut usize) -> Option<FormatTag> {
    let color = decode_terminal_color(bytes, pos)?;
    let background_color = decode_terminal_color(bytes, pos)?;
    let underline_color = decode_terminal_color(bytes, pos)?;
    let reverse_video = decode_reverse_video(read_u8(bytes, pos)?)?;
    let font_weight = decode_font_weight(read_u8(bytes, pos)?)?;
    let font_decorations = decode_font_decorations(read_u8(bytes, pos)?);

    let url = match read_u8(bytes, pos)? {
        0 => None,
        1 => {
            let id = match read_u8(bytes, pos)? {
                0 => None,
                1 => Some(decode_string(bytes, pos)?),
                _ => return None,
            };
            let url_string = decode_string(bytes, pos)?;
            Some(Arc::new(Url {
                id,
                url: url_string,
            }))
        }
        _ => return None,
    };

    let blink = decode_blink(read_u8(bytes, pos)?)?;

    Some(FormatTag {
        // Reconstructed with the default positional range — see the
        // doc comment on `CompactRow::to_bytes`.
        start: 0,
        end: usize::MAX,
        colors: StateColors {
            color,
            background_color,
            underline_color,
            reverse_video,
        },
        font_weight,
        font_decorations,
        url,
        blink,
    })
}

// ===========================================================================
// `RowOrigin` / `RowJoin` / `LineWidth` encoding
// ===========================================================================

const fn encode_row_origin(origin: RowOrigin) -> u8 {
    match origin {
        RowOrigin::HardBreak => 0,
        RowOrigin::SoftWrap => 1,
        RowOrigin::ScrollFill => 2,
    }
}

const fn decode_row_origin(byte: u8) -> Option<RowOrigin> {
    match byte {
        0 => Some(RowOrigin::HardBreak),
        1 => Some(RowOrigin::SoftWrap),
        2 => Some(RowOrigin::ScrollFill),
        _ => None,
    }
}

const fn encode_row_join(join: RowJoin) -> u8 {
    match join {
        RowJoin::NewLogicalLine => 0,
        RowJoin::ContinueLogicalLine => 1,
    }
}

const fn decode_row_join(byte: u8) -> Option<RowJoin> {
    match byte {
        0 => Some(RowJoin::NewLogicalLine),
        1 => Some(RowJoin::ContinueLogicalLine),
        _ => None,
    }
}

const fn encode_line_width(width: LineWidth) -> u8 {
    match width {
        LineWidth::Normal => 0,
        LineWidth::DoubleWidth => 1,
        LineWidth::DoubleHeightTop => 2,
        LineWidth::DoubleHeightBottom => 3,
    }
}

const fn decode_line_width(byte: u8) -> Option<LineWidth> {
    match byte {
        0 => Some(LineWidth::Normal),
        1 => Some(LineWidth::DoubleWidth),
        2 => Some(LineWidth::DoubleHeightTop),
        3 => Some(LineWidth::DoubleHeightBottom),
        _ => None,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::sync::Arc;

    use freminal_common::{
        buffer_states::{fonts::FontWeight, url::Url},
        colors::TerminalColor,
    };

    use crate::image_store::{ImagePlacement, ImageProtocol};

    use super::*;

    /// Guard the measured sizes of the types this module's space accounting
    /// depends on. If either changes, the heap-savings claims documented at
    /// the top of this module (and measured in the round-trip tests) must be
    /// re-verified. Kept as a targeted `#[test]` rather than a crate-level
    /// `const` assertion so an unrelated field addition (or a different
    /// pointer width) surfaces as a single failing test in CI rather than a
    /// crate-wide `cargo build` failure.
    #[test]
    fn cell_and_format_tag_sizes_match_documented_space_savings() {
        assert_eq!(
            core::mem::size_of::<Cell>(),
            72,
            "Cell size changed — re-measure CompactRow's space savings"
        );
        assert_eq!(
            core::mem::size_of::<FormatTag>(),
            40,
            "FormatTag size changed — re-measure CompactRow's space savings"
        );
    }

    fn ascii_row(width: usize, text: &str, tag: &FormatTag) -> Row {
        let mut row = Row::new(width);
        let chars: Vec<TChar> = text.bytes().map(TChar::Ascii).collect();
        row.insert_text(0, &chars, tag);
        row
    }

    fn assert_round_trip_exact(row: &Row) {
        let compact = CompactRow::from_row(row).expect("row should be compactable");
        let rebuilt = compact.to_row();

        assert_eq!(
            rebuilt.cells(),
            row.cells(),
            "cell contents must match exactly"
        );
        assert_eq!(rebuilt.max_width(), row.max_width());
        assert_eq!(rebuilt.origin, row.origin);
        assert_eq!(rebuilt.join, row.join);
        assert_eq!(rebuilt.line_width, row.line_width);
        // `dirty` is explicitly exempt from the round-trip identity.
    }

    // -----------------------------------------------------------------------
    // Plain ASCII row, default format
    // -----------------------------------------------------------------------

    #[test]
    fn round_trip_plain_ascii_row() {
        let row = ascii_row(20, "hello world", &FormatTag::default());
        assert_round_trip_exact(&row);

        let compact = CompactRow::from_row(&row).unwrap();
        // Uniform default formatting across the whole row -> a single run.
        assert_eq!(compact.tag_run_count(), 1);
    }

    // -----------------------------------------------------------------------
    // Colored runs: multiple FormatTag runs, coalescing verified
    // -----------------------------------------------------------------------

    #[test]
    fn round_trip_colored_runs_and_coalescing() {
        let mut row = Row::new(10);
        let default_tag = FormatTag::default();
        let mut red_tag = FormatTag::default();
        red_tag.colors.set_color(TerminalColor::Red);
        let bold_tag = FormatTag {
            font_weight: FontWeight::Bold,
            ..FormatTag::default()
        };

        // "AA" default, "BB" red, "CC" bold, "DD" red again (not adjacent to
        // the first red run, so it must NOT be coalesced with it).
        row.insert_text(0, &[TChar::Ascii(b'A'), TChar::Ascii(b'A')], &default_tag);
        row.insert_text(2, &[TChar::Ascii(b'B'), TChar::Ascii(b'B')], &red_tag);
        row.insert_text(4, &[TChar::Ascii(b'C'), TChar::Ascii(b'C')], &bold_tag);
        row.insert_text(6, &[TChar::Ascii(b'D'), TChar::Ascii(b'D')], &red_tag);

        assert_round_trip_exact(&row);

        let compact = CompactRow::from_row(&row).unwrap();
        // 4 distinct runs: default(2), red(2), bold(2), red(2) — the two red
        // runs are NOT adjacent, so they are not coalesced into one.
        assert_eq!(compact.tag_run_count(), 4);
    }

    #[test]
    fn uniform_row_has_far_fewer_runs_than_varied_row() {
        let uniform = ascii_row(40, &"x".repeat(40), &FormatTag::default());
        let varied_tag_a = FormatTag {
            font_weight: FontWeight::Bold,
            ..FormatTag::default()
        };

        let mut varied = Row::new(40);
        for i in 0..40 {
            let tag = if i % 2 == 0 {
                FormatTag::default()
            } else {
                varied_tag_a.clone()
            };
            varied.insert_text(i, &[TChar::Ascii(b'x')], &tag);
        }

        let uniform_compact = CompactRow::from_row(&uniform).unwrap();
        let varied_compact = CompactRow::from_row(&varied).unwrap();

        assert_eq!(uniform_compact.tag_run_count(), 1);
        assert_eq!(varied_compact.tag_run_count(), 40);
        assert!(uniform_compact.tag_run_count() < varied_compact.tag_run_count());
    }

    // -----------------------------------------------------------------------
    // Mixed tags with a URL (Arc<Url>): preserved and shared, not deep-copied
    // -----------------------------------------------------------------------

    #[test]
    fn round_trip_preserves_and_shares_url_arc() {
        let mut row = Row::new(10);
        let default_tag = FormatTag::default();
        let url = Arc::new(Url {
            id: None,
            url: "https://example.com".to_string(),
        });
        let url_tag = FormatTag {
            url: Some(Arc::clone(&url)),
            ..FormatTag::default()
        };

        row.insert_text(0, &[TChar::Ascii(b'a')], &default_tag);
        row.insert_text(1, &[TChar::Ascii(b'b'), TChar::Ascii(b'c')], &url_tag);

        assert_round_trip_exact(&row);

        let compact = CompactRow::from_row(&row).unwrap();
        // The url-tagged run should be a single coalesced run (both cells
        // carry `==` tags).
        assert_eq!(compact.tag_run_count(), 2);

        // The Arc for the shared run is the very same allocation (refcount
        // bump only, not a deep copy of the URL string).
        let url_run_tag = &compact.tag_runs[1].0;
        let stored_url = url_run_tag.url.as_ref().expect("expected a url");
        assert!(Arc::ptr_eq(stored_url, &url));
        assert_eq!(stored_url.url, "https://example.com");

        let rebuilt = compact.to_row();
        let rebuilt_url = rebuilt.cells()[1].tag().url.as_ref().expect("url");
        assert_eq!(rebuilt_url.url, "https://example.com");
        assert_eq!(rebuilt_url, &url);
    }

    // -----------------------------------------------------------------------
    // Wide char (head + continuation)
    // -----------------------------------------------------------------------

    #[test]
    fn round_trip_wide_char_head_and_continuation() {
        let mut row = Row::new(10);
        let tag = FormatTag::default();
        row.insert_text(0, &[TChar::from('中')], &tag);

        assert!(row.cells()[0].is_head());
        assert!(row.cells()[1].is_continuation());

        assert_round_trip_exact(&row);
    }

    // -----------------------------------------------------------------------
    // Orphan continuation cell (continuation with no head)
    // -----------------------------------------------------------------------

    #[test]
    fn round_trip_orphan_continuation_cell() {
        let mut row = Row::new(5);
        row.cells_mut_push(Cell::new(TChar::Ascii(b'a'), FormatTag::default()));
        // An orphan continuation: no preceding head cell.
        row.cells_mut_push(Cell::wide_continuation());
        row.cells_mut_push(Cell::new(TChar::Ascii(b'b'), FormatTag::default()));

        assert!(!row.cells()[0].is_continuation());
        assert!(row.cells()[1].is_continuation());
        assert!(!row.cells()[1].is_head());

        assert_round_trip_exact(&row);
    }

    // -----------------------------------------------------------------------
    // Blank/sparse row (empty cells vec)
    // -----------------------------------------------------------------------

    #[test]
    fn round_trip_blank_sparse_row() {
        let row = Row::new(80);
        assert!(row.cells().is_empty());
        assert_round_trip_exact(&row);

        let compact = CompactRow::from_row(&row).unwrap();
        assert!(compact.tag_runs.is_empty());
        assert!(compact.wide_runs.is_empty());
        assert!(compact.chars.is_empty());
    }

    // -----------------------------------------------------------------------
    // Non-default origin/join/line_width
    // -----------------------------------------------------------------------

    #[test]
    fn round_trip_nondefault_origin_join_line_width() {
        let tag = FormatTag::default();
        let chars: Vec<TChar> = b"soft-wrapped".iter().map(|&b| TChar::Ascii(b)).collect();
        let mut cells = Vec::new();
        for c in &chars {
            cells.push(Cell::new(*c, tag.clone()));
        }
        let mut row = Row::from_cells(40, RowOrigin::SoftWrap, RowJoin::ContinueLogicalLine, cells);
        row.line_width = LineWidth::DoubleWidth;

        assert_round_trip_exact(&row);
    }

    // -----------------------------------------------------------------------
    // Image rows opt out
    // -----------------------------------------------------------------------

    fn image_placement() -> ImagePlacement {
        ImagePlacement {
            image_id: 1,
            col_in_image: 0,
            row_in_image: 0,
            protocol: ImageProtocol::Kitty,
            image_number: None,
            placement_id: None,
            z_index: 0,
            source_crop: None,
            placement_instance: 1,
            subcell_offset: None,
        }
    }

    #[test]
    fn image_row_is_not_compactable() {
        let mut row = Row::new(10);
        row.set_image_cell(2, image_placement(), FormatTag::default());

        assert!(!is_compactable(&row));
        assert!(CompactRow::from_row(&row).is_none());
    }

    #[test]
    fn compactable_row_without_images_reports_true() {
        let row = ascii_row(10, "abc", &FormatTag::default());
        assert!(is_compactable(&row));
        assert!(CompactRow::from_row(&row).is_some());
    }

    // -----------------------------------------------------------------------
    // Size / heap-savings assertion
    // -----------------------------------------------------------------------

    #[test]
    fn compact_row_heap_bytes_smaller_than_per_cell_row_for_uniform_formatting() {
        // A representative 200-column row with uniform (non-default, so it's
        // not trivially sparse) formatting: every cell in `Row` pays the full
        // `size_of::<Cell>()` cost, but `CompactRow` collapses the formatting
        // into a single run.
        let width = 200;
        let mut tag = FormatTag::default();
        tag.colors.set_color(TerminalColor::Green);

        let text = "x".repeat(width);
        let row = ascii_row(width, &text, &tag);

        let compact = CompactRow::from_row(&row).unwrap();
        assert_eq!(compact.tag_run_count(), 1);

        let row_per_cell_cost = core::mem::size_of_val(row.cells());
        let compact_cost = compact.heap_bytes();

        assert!(
            compact_cost < row_per_cell_cost,
            "compact heap cost ({compact_cost}) should be materially smaller than \
             the equivalent Row's per-cell cost ({row_per_cell_cost})"
        );
        // Sanity: the saving should not be trivial for a 200-cell uniform row.
        assert!(row_per_cell_cost / compact_cost.max(1) >= 2);
    }

    // -----------------------------------------------------------------------
    // Run-length invariants
    // -----------------------------------------------------------------------

    #[test]
    fn run_lengths_sum_to_chars_len() {
        let row = ascii_row(30, "some text here to test runs!!", &FormatTag::default());
        let compact = CompactRow::from_row(&row).unwrap();

        let tag_total: u32 = compact.tag_runs.iter().map(|(_, len)| *len).sum();
        let wide_total: u32 = compact.wide_runs.iter().map(|(_, len)| *len).sum();

        assert_eq!(usize::try_from(tag_total).unwrap(), compact.chars.len());
        assert_eq!(usize::try_from(wide_total).unwrap(), compact.chars.len());
    }

    // -----------------------------------------------------------------------
    // Byte serialization round-trip (`to_bytes`/`from_bytes`, Task 119.3)
    // -----------------------------------------------------------------------

    /// Round-trip `row` through `CompactRow::to_bytes`/`from_bytes` and
    /// assert the decoded row is cell-for-cell, metadata-for-metadata
    /// identical to the original (mirrors `assert_round_trip_exact`, but
    /// through the byte format rather than the in-memory `CompactRow`).
    fn assert_byte_round_trip_exact(row: &Row) {
        let compact = CompactRow::from_row(row).expect("row should be compactable");
        let bytes = compact.to_bytes();
        let (decoded, consumed) = CompactRow::from_bytes(&bytes).expect("bytes should decode");
        assert_eq!(
            consumed,
            bytes.len(),
            "from_bytes must consume every byte to_bytes wrote"
        );

        let rebuilt = decoded.to_row();
        assert_eq!(
            rebuilt.cells(),
            row.cells(),
            "cell contents must match exactly"
        );
        assert_eq!(rebuilt.max_width(), row.max_width());
        assert_eq!(rebuilt.origin, row.origin);
        assert_eq!(rebuilt.join, row.join);
        assert_eq!(rebuilt.line_width, row.line_width);
    }

    #[test]
    fn byte_round_trip_plain_ascii_row() {
        let row = ascii_row(20, "hello world", &FormatTag::default());
        assert_byte_round_trip_exact(&row);
    }

    #[test]
    fn byte_round_trip_colored_runs() {
        let mut row = Row::new(10);
        let default_tag = FormatTag::default();
        let mut red_tag = FormatTag::default();
        red_tag.colors.set_color(TerminalColor::Red);
        let mut custom_tag = FormatTag::default();
        custom_tag
            .colors
            .set_background_color(TerminalColor::Custom(10, 20, 30));
        let mut palette_tag = FormatTag::default();
        palette_tag
            .colors
            .set_underline_color(TerminalColor::PaletteIndex(200));

        row.insert_text(0, &[TChar::Ascii(b'A'), TChar::Ascii(b'A')], &default_tag);
        row.insert_text(2, &[TChar::Ascii(b'B'), TChar::Ascii(b'B')], &red_tag);
        row.insert_text(4, &[TChar::Ascii(b'C'), TChar::Ascii(b'C')], &custom_tag);
        row.insert_text(6, &[TChar::Ascii(b'D'), TChar::Ascii(b'D')], &palette_tag);

        assert_byte_round_trip_exact(&row);
    }

    #[test]
    fn byte_round_trip_all_named_colors() {
        // Every named `TerminalColor` variant, to exercise every encode/decode tag byte.
        let colors = [
            TerminalColor::Default,
            TerminalColor::DefaultBackground,
            TerminalColor::DefaultUnderlineColor,
            TerminalColor::DefaultCursorColor,
            TerminalColor::Black,
            TerminalColor::Red,
            TerminalColor::Green,
            TerminalColor::Yellow,
            TerminalColor::Blue,
            TerminalColor::Magenta,
            TerminalColor::Cyan,
            TerminalColor::White,
            TerminalColor::BrightYellow,
            TerminalColor::BrightBlack,
            TerminalColor::BrightRed,
            TerminalColor::BrightGreen,
            TerminalColor::BrightBlue,
            TerminalColor::BrightMagenta,
            TerminalColor::BrightCyan,
            TerminalColor::BrightWhite,
            TerminalColor::Custom(1, 2, 3),
            TerminalColor::PaletteIndex(42),
        ];

        let mut row = Row::new(colors.len());
        for (i, color) in colors.iter().enumerate() {
            let mut tag = FormatTag::default();
            tag.colors.set_color(*color);
            row.insert_text(i, &[TChar::Ascii(b'x')], &tag);
        }

        assert_byte_round_trip_exact(&row);
    }

    #[test]
    fn byte_round_trip_bold_and_each_decoration() {
        let mut row = Row::new(10);

        let bold_tag = FormatTag {
            font_weight: FontWeight::Bold,
            ..FormatTag::default()
        };
        row.insert_text(0, &[TChar::Ascii(b'a')], &bold_tag);

        let mut italic_flags = FontDecorationFlags::empty();
        italic_flags.insert(FontDecorations::Italic);
        let italic_tag = FormatTag {
            font_decorations: italic_flags,
            ..FormatTag::default()
        };
        row.insert_text(1, &[TChar::Ascii(b'b')], &italic_tag);

        let mut faint_flags = FontDecorationFlags::empty();
        faint_flags.insert(FontDecorations::Faint);
        let faint_tag = FormatTag {
            font_decorations: faint_flags,
            ..FormatTag::default()
        };
        row.insert_text(2, &[TChar::Ascii(b'c')], &faint_tag);

        let mut strike_flags = FontDecorationFlags::empty();
        strike_flags.insert(FontDecorations::Strikethrough);
        let strike_tag = FormatTag {
            font_decorations: strike_flags,
            ..FormatTag::default()
        };
        row.insert_text(3, &[TChar::Ascii(b'd')], &strike_tag);

        let mut combined_flags = FontDecorationFlags::empty();
        combined_flags.insert(FontDecorations::Italic);
        combined_flags.insert(FontDecorations::Faint);
        combined_flags.insert(FontDecorations::Strikethrough);
        let combined_tag = FormatTag {
            font_weight: FontWeight::Bold,
            font_decorations: combined_flags,
            blink: BlinkState::Slow,
            ..FormatTag::default()
        };
        row.insert_text(4, &[TChar::Ascii(b'e')], &combined_tag);

        assert_byte_round_trip_exact(&row);
    }

    #[test]
    fn byte_round_trip_every_underline_style() {
        let styles = [
            UnderlineStyle::None,
            UnderlineStyle::Single,
            UnderlineStyle::Double,
            UnderlineStyle::Curly,
            UnderlineStyle::Dotted,
            UnderlineStyle::Dashed,
        ];

        let mut row = Row::new(styles.len());
        for (i, style) in styles.iter().enumerate() {
            let mut flags = FontDecorationFlags::empty();
            flags.set_underline_style(*style);
            let tag = FormatTag {
                font_decorations: flags,
                ..FormatTag::default()
            };
            row.insert_text(i, &[TChar::Ascii(b'u')], &tag);
        }

        assert_byte_round_trip_exact(&row);
    }

    #[test]
    fn byte_round_trip_reverse_video() {
        let mut row = Row::new(5);
        let mut tag = FormatTag::default();
        tag.colors.set_reverse_video(ReverseVideo::On);
        row.insert_text(0, &[TChar::Ascii(b'r')], &tag);
        assert_byte_round_trip_exact(&row);
    }

    #[test]
    fn byte_round_trip_every_blink_state() {
        let states = [BlinkState::None, BlinkState::Slow, BlinkState::Fast];
        let mut row = Row::new(states.len());
        for (i, state) in states.iter().enumerate() {
            let tag = FormatTag {
                blink: *state,
                ..FormatTag::default()
            };
            row.insert_text(i, &[TChar::Ascii(b'b')], &tag);
        }
        assert_byte_round_trip_exact(&row);
    }

    #[test]
    fn byte_round_trip_wide_char_head_and_continuation() {
        let mut row = Row::new(10);
        let tag = FormatTag::default();
        row.insert_text(0, &[TChar::from('中')], &tag);
        assert_byte_round_trip_exact(&row);
    }

    #[test]
    fn byte_round_trip_orphan_continuation_cell() {
        let mut row = Row::new(5);
        row.cells_mut_push(Cell::new(TChar::Ascii(b'a'), FormatTag::default()));
        row.cells_mut_push(Cell::wide_continuation());
        row.cells_mut_push(Cell::new(TChar::Ascii(b'b'), FormatTag::default()));
        assert_byte_round_trip_exact(&row);
    }

    #[test]
    fn byte_round_trip_utf8_multibyte_chars() {
        let mut row = Row::new(10);
        let tag = FormatTag::default();
        // Multi-byte non-wide UTF-8 char (accented Latin, 2 bytes).
        row.insert_text(0, &[TChar::from('é')], &tag);
        // 3-byte UTF-8, single-width.
        row.insert_text(1, &[TChar::from('★')], &tag);
        assert_byte_round_trip_exact(&row);
    }

    #[test]
    fn byte_round_trip_blank_sparse_row() {
        let row = Row::new(80);
        assert_byte_round_trip_exact(&row);
    }

    #[test]
    fn byte_round_trip_url_tag() {
        let mut row = Row::new(10);
        let default_tag = FormatTag::default();
        let url_no_id = Arc::new(Url {
            id: None,
            url: "https://example.com".to_string(),
        });
        let url_with_id_tag = FormatTag {
            url: Some(Arc::new(Url {
                id: Some("link1".to_string()),
                url: "https://example.org".to_string(),
            })),
            ..FormatTag::default()
        };
        let url_no_id_tag = FormatTag {
            url: Some(Arc::clone(&url_no_id)),
            ..FormatTag::default()
        };

        row.insert_text(0, &[TChar::Ascii(b'a')], &default_tag);
        row.insert_text(
            1,
            &[TChar::Ascii(b'b'), TChar::Ascii(b'c')],
            &url_with_id_tag,
        );
        row.insert_text(3, &[TChar::Ascii(b'd')], &url_no_id_tag);

        assert_byte_round_trip_exact(&row);
    }

    #[test]
    fn byte_round_trip_every_row_origin_join_line_width() {
        let origins = [
            RowOrigin::HardBreak,
            RowOrigin::SoftWrap,
            RowOrigin::ScrollFill,
        ];
        let joins = [RowJoin::NewLogicalLine, RowJoin::ContinueLogicalLine];
        let line_widths = [
            LineWidth::Normal,
            LineWidth::DoubleWidth,
            LineWidth::DoubleHeightTop,
            LineWidth::DoubleHeightBottom,
        ];

        for &origin in &origins {
            for &join in &joins {
                for &line_width in &line_widths {
                    let tag = FormatTag::default();
                    let cells = vec![Cell::new(TChar::Ascii(b'x'), tag.clone())];
                    let mut row = Row::from_cells(10, origin, join, cells);
                    row.line_width = line_width;
                    assert_byte_round_trip_exact(&row);
                }
            }
        }
    }

    #[test]
    fn from_bytes_truncated_input_never_panics() {
        let row = ascii_row(10, "hello", &FormatTag::default());
        let compact = CompactRow::from_row(&row).unwrap();
        let bytes = compact.to_bytes();

        // Truncate at every possible prefix length: decoding must either
        // fail gracefully (`None`) or, if a shorter prefix happens to
        // still parse, never panic or index out of bounds.
        for len in 0..bytes.len() {
            let _ = CompactRow::from_bytes(&bytes[..len]);
        }
    }

    #[test]
    fn from_bytes_empty_input_returns_none() {
        assert!(CompactRow::from_bytes(&[]).is_none());
    }

    #[test]
    fn from_bytes_garbage_input_returns_none_not_panic() {
        // A huge (bogus) declared char count in the first 4 little-endian
        // bytes must not panic or attempt an absurd allocation.
        let garbage = vec![0xFF, 0xFF, 0xFF, 0xFF, 0xAB, 0xCD, 0xEF, 0x01];
        assert!(CompactRow::from_bytes(&garbage).is_none());
    }

    #[test]
    fn from_bytes_invalid_tchar_tag_returns_none() {
        // char_count = 1, then an invalid TChar tag byte (99).
        let bytes: Vec<u8> = vec![1, 0, 0, 0, 99];
        assert!(CompactRow::from_bytes(&bytes).is_none());
    }

    #[test]
    fn from_bytes_invalid_color_tag_returns_none() {
        // char_count = 1, one Ascii 'x', tag_run_count = 1, then an
        // invalid `TerminalColor` tag byte (255) for `colors.color`.
        let mut bytes: Vec<u8> = Vec::new();
        bytes.extend_from_slice(&1u32.to_le_bytes()); // char_count
        bytes.push(0); // TCHAR_TAG_ASCII
        bytes.push(b'x');
        bytes.extend_from_slice(&1u32.to_le_bytes()); // tag_run_count
        bytes.push(255); // invalid color tag
        assert!(CompactRow::from_bytes(&bytes).is_none());
    }
}
