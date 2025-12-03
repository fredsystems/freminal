// Copyright (C) 2024-2025 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::ansi_components::modes::decawm::Decawm;

use super::data::TerminalSections;
use anyhow::Result;
use freminal_common::{
    buffer_states::{buffer_type::BufferType, cursor::CursorPos, tchar::TChar},
    scroll::ScrollDirection,
    terminal_size::{DEFAULT_HEIGHT, DEFAULT_WIDTH},
};
use std::ops::Range;

pub struct PadBufferForWriteResponse {
    /// Where to copy data into
    pub write_idx: usize,
    /// Indexes where we added data
    pub inserted_padding: Range<usize>,
}

pub struct TerminalBufferInsertResponse {
    /// Range of written data after insertion of padding
    pub written_range: Range<usize>,
    /// Range of written data that is new. Note this will shift all data after it
    /// Includes padding that was previously not there, e.g. newlines needed to get to the
    /// requested row for writing
    pub insertion_range: Range<usize>,
    pub new_cursor_pos: CursorPos,
}

#[derive(Debug)]
pub struct TerminalBufferInsertLineResponse {
    /// Range of deleted data **before insertion**
    pub deleted_range: Range<usize>,
    /// Range of inserted data
    pub inserted_range: Range<usize>,
}

pub struct TerminalBufferSetWinSizeResponse {
    pub changed: bool,
    _insertion_range: Range<usize>,
    pub new_cursor_pos: CursorPos,
}

#[derive(Eq, PartialEq, Debug)]
pub struct TerminalBufferHolder {
    pub buf: Vec<TChar>,
    pub width: usize,
    pub height: usize,
    visible_line_ranges: Vec<Range<usize>>,
    buffer_line_ranges: Vec<Range<usize>>,
    viewable_index_bottom: usize, // usize::MAX represents the bottom of the buffer
    top_margin: usize,
    bottom_margin: usize,
    buffer_type: BufferType,
}

impl Default for TerminalBufferHolder {
    fn default() -> Self {
        Self {
            buf: Vec::with_capacity(500_000),
            width: DEFAULT_WIDTH as usize,
            height: DEFAULT_HEIGHT as usize,
            visible_line_ranges: Vec::with_capacity(24),
            buffer_line_ranges: Vec::with_capacity(5000),
            viewable_index_bottom: usize::MAX,
            top_margin: 0,
            bottom_margin: usize::MAX,
            buffer_type: BufferType::Primary,
        }
    }
}

impl TerminalBufferHolder {
    #[must_use]
    pub fn new(width: usize, height: usize, buffer_type: BufferType) -> Self {
        Self {
            buf: Vec::with_capacity(500_000),
            width,
            height,
            visible_line_ranges: Vec::with_capacity(height),
            buffer_line_ranges: Vec::with_capacity(5000),
            viewable_index_bottom: usize::MAX,
            top_margin: 0,
            bottom_margin: usize::MAX,
            buffer_type,
        }
    }

    #[must_use]
    pub const fn show_cursor(&self, cursor_pos: &CursorPos) -> bool {
        // FIXME: I think this logic is partially buggy. If the cursor is not in the last line it may break
        // if we have scrolled more then height from the bottom, we don't want to show the cursor
        // otherwise, if the cursor_pos.y position is within viewable_index_bottom and viewable_index_bottom - height, we want to show the cursor
        if self.viewable_index_bottom == usize::MAX {
            return true;
        }

        if (self.buffer_line_ranges.len() - 1).saturating_sub(self.height)
            < self.viewable_index_bottom
        {
            return false;
        }

        cursor_pos.y >= self.viewable_index_bottom.saturating_sub(self.height)
    }

    pub fn scroll_down(&mut self, num_lines: &usize) {
        if self.buffer_type == BufferType::Alternate {
            return;
        }

        if self.buffer_line_ranges.len() == self.visible_line_ranges.len() {
            debug!("not enough lines for scroll");
            return;
        }

        if self.viewable_index_bottom == usize::MAX {
            debug!("Down scroll already is at the bottom");
            return;
        }

        if self.viewable_index_bottom + num_lines >= self.buffer_line_ranges.len() {
            debug!("Down scroll is now at the bottom");
            self.viewable_index_bottom = usize::MAX;
            return;
        }

        self.viewable_index_bottom += num_lines;
        debug!("Scrolling down to {}", self.viewable_index_bottom);
    }

    pub fn scroll_up(&mut self, num_lines: &usize) {
        if self.buffer_type == BufferType::Alternate {
            return;
        }

        if self.buffer_line_ranges.len() == self.visible_line_ranges.len() {
            debug!("not enough lines for scroll");
            return;
        }

        if self.viewable_index_bottom == usize::MAX {
            self.viewable_index_bottom = self.buffer_line_ranges.len() - 1;
        }

        self.viewable_index_bottom = self.viewable_index_bottom.saturating_sub(*num_lines);
        if self.viewable_index_bottom < self.height {
            self.viewable_index_bottom = self.height - 1;
            debug!("Up scroll already is at the top");
            return;
        }

        debug!("Scrolling up to {}", self.viewable_index_bottom);
    }

    pub fn scroll(&mut self, direction: &ScrollDirection) {
        match direction {
            ScrollDirection::Up(n) => self.scroll_up(n),
            ScrollDirection::Down(n) => self.scroll_down(n),
        }
    }

    // FIXME: I think this is a clippy bug, or I'm stupid. It wants this to be a const fn, but it fails because of a deref error?
    #[allow(clippy::missing_const_for_fn)]
    #[must_use]
    pub fn get_visible_line_ranges(&self) -> &[Range<usize>] {
        &self.visible_line_ranges
    }

    pub fn set_visible_line_ranges(&mut self, visible_line_ranges: Vec<Range<usize>>) {
        self.visible_line_ranges = visible_line_ranges;
    }

    // FIXME: I think this is a clippy bug, or I'm stupid. It wants this to be a const fn, but it fails because of a deref error?
    #[allow(clippy::missing_const_for_fn)]
    #[must_use]
    pub fn get_line_ranges(&self) -> &[Range<usize>] {
        &self.buffer_line_ranges
    }

    pub fn set_line_ranges(&mut self, line_ranges: Vec<Range<usize>>) {
        self.buffer_line_ranges = line_ranges;
    }

    pub fn screen_alignment_test(&mut self) -> Range<usize> {
        for _ in 0..self.height {
            for _ in 0..self.width {
                self.buf.push(TChar::new_from_single_char(b'E'));
            }
            self.buf.push(TChar::NewLine);
        }

        self.line_ranges_to_visible_line_ranges();
        self.visible_line_ranges[0].start..self.visible_line_ranges[self.height - 1].end
    }

    /// Inserts data into the buffer at the cursor position
    ///
    /// # Errors
    /// Will error if the data is not valid utf8
    pub fn insert_data(
        &mut self,
        cursor_pos: &CursorPos,
        data: &[u8],
        decawm: &Decawm,
    ) -> Result<TerminalBufferInsertResponse> {
        // loop through all of the characters
        // if the character is utf8, then we need all of the bytes to be written
        if data.is_empty() {
            return Ok(TerminalBufferInsertResponse {
                written_range: 0..0,
                insertion_range: 0..0,
                new_cursor_pos: *cursor_pos,
            });
        }

        let mut converted_buffer = TChar::from_vec(data)?;
        let mut offset = false;

        if decawm == &Decawm::NoAutoWrap && cursor_pos.x + converted_buffer.len() >= self.width {
            // if the cursor pos + the length of the data is greater than self.width, we need to truncate the incoming data

            // example
            // buffer = AAAAA
            // width = 10
            // incoming data
            // MNOPQRS
            // cursor pos = 5
            // expected truncation is MNOPS

            let last_char = converted_buffer.pop().unwrap_or(TChar::Space); // Unwrap with anything is just to make rust happy. It should never be a None value if we ended up here
            let last_element_index = converted_buffer.len();
            let keep = self.width.saturating_sub(cursor_pos.x).saturating_sub(1);
            let _ = converted_buffer.drain(keep..last_element_index);
            converted_buffer.push(last_char);

            if cursor_pos.x + converted_buffer.len() >= self.width {
                offset = true;
            }
        }

        let PadBufferForWriteResponse {
            write_idx,
            inserted_padding,
        } = self.pad_buffer_for_write(cursor_pos, converted_buffer.len());
        let write_range = write_idx..write_idx + converted_buffer.len();

        self.buf
            .splice(write_range.clone(), converted_buffer.iter().cloned());

        self.line_ranges_to_visible_line_ranges();

        let new_cursor_pos = if offset {
            CursorPos {
                x: self.width.saturating_sub(1),
                y: cursor_pos.y,
            }
        } else {
            self.buf_to_cursor_pos(write_range.end)
        };

        Ok(TerminalBufferInsertResponse {
            written_range: write_range,
            insertion_range: inserted_padding,
            new_cursor_pos,
        })
    }

    /// Inserts data, but will not wrap. If line end is hit, data stops
    pub fn insert_spaces(
        &mut self,
        cursor_pos: &CursorPos,
        mut num_spaces: usize,
    ) -> TerminalBufferInsertResponse {
        num_spaces = self.width.min(num_spaces);

        let buf_pos = self.cursor_to_buf_pos(cursor_pos);
        if let Some((buf_pos, line_range)) = buf_pos {
            // Insert spaces until either we hit num_spaces, or the line width is too long
            let line_len = line_range.end - line_range.start;
            let num_inserted = (num_spaces).min(self.width - line_len);

            // Overwrite existing with spaces until we hit num_spaces or we hit the line end
            let num_overwritten = (num_spaces - num_inserted).min(line_range.end - buf_pos);

            // NOTE: We do the overwrite first so we don't have to worry about adjusting
            // indices for the newly inserted data
            self.buf[buf_pos..buf_pos + num_overwritten].fill(TChar::Space);
            self.buf.splice(
                buf_pos..buf_pos,
                std::iter::repeat_n(TChar::Space, num_inserted),
            );

            let used_spaces = num_inserted + num_overwritten;
            self.line_ranges_to_visible_line_ranges();
            TerminalBufferInsertResponse {
                written_range: buf_pos..buf_pos + used_spaces,
                insertion_range: buf_pos..buf_pos + num_inserted,
                new_cursor_pos: *cursor_pos,
            }
        } else {
            let PadBufferForWriteResponse {
                write_idx,
                inserted_padding,
            } = self.pad_buffer_for_write(cursor_pos, num_spaces);
            self.line_ranges_to_visible_line_ranges();
            TerminalBufferInsertResponse {
                written_range: write_idx..write_idx + num_spaces,
                insertion_range: inserted_padding,
                new_cursor_pos: *cursor_pos,
            }
        }
    }

    pub fn insert_lines(
        &mut self,
        cursor_pos: &CursorPos,
        mut num_lines: usize,
    ) -> TerminalBufferInsertLineResponse {
        let visible_line_ranges = &self.visible_line_ranges;

        // NOTE: Cursor x position is not used. If the cursor position was too far to the right,
        // there may be no buffer position associated with it. Use Y only
        let Some(line_range) = visible_line_ranges.get(cursor_pos.y) else {
            return TerminalBufferInsertLineResponse {
                deleted_range: 0..0,
                inserted_range: 0..0,
            };
        };

        let available_space = self.height - visible_line_ranges.len();
        // If height is 10, and y is 5, we can only insert 5 lines. If we inserted more it would
        // adjust the visible line range, and that would be a problem
        num_lines = num_lines.min(self.height - cursor_pos.y);

        let deletion_range = if num_lines > available_space {
            let num_lines_removed = num_lines - available_space;
            let removal_start_idx =
                visible_line_ranges[visible_line_ranges.len() - num_lines_removed].start;
            let deletion_range = removal_start_idx..self.buf.len();
            self.buf.truncate(removal_start_idx);
            deletion_range
        } else {
            0..0
        };

        let insertion_pos = line_range.start;

        // Edge case, if the previous line ended in a line wrap, inserting a new line will not
        // result in an extra line being shown on screen. E.g. with a width of 5, 01234 and 01234\n
        // both look like a line of length 5. In this case we need to add another newline
        if insertion_pos > 0 && self.buf[insertion_pos - 1] != TChar::NewLine {
            num_lines += 1;
        }

        self.buf.splice(
            insertion_pos..insertion_pos,
            std::iter::repeat_n(TChar::NewLine, num_lines),
        );

        self.line_ranges_to_visible_line_ranges();

        TerminalBufferInsertLineResponse {
            deleted_range: deletion_range,
            inserted_range: insertion_pos..insertion_pos + num_lines,
        }
    }

    /// Clear backwards from the cursor position to start of screen
    ///
    /// Returns the buffer position that was cleared to
    ///
    /// # Errors
    /// Will error if the cursor position changes during the clear
    pub fn clear_backwards(&mut self, cursor_pos: &CursorPos) -> Option<Range<usize>> {
        let (buf_pos, _) = self.cursor_to_buf_pos(cursor_pos)?;

        if self.visible_line_ranges.is_empty() {
            return None;
        }

        // we want to clear from the start of the visible line to the cursor pos

        // clear from the buf pos that is the start of the visible line to the cursor pos

        let start_pos = self.visible_line_ranges[0].start;
        let clear_range = start_pos..buf_pos;

        for i in clear_range.clone() {
            match self.buf.get(i) {
                Some(TChar::NewLine | TChar::Space) => (),
                Some(_) => self.buf[i] = TChar::Space,
                None => break,
            }
        }

        self.line_ranges_to_visible_line_ranges();
        Some(clear_range)
    }

    /// Clear forwards from the cursor position
    ///
    /// Returns the buffer position that was cleared to
    ///
    /// # Errors
    /// Will error if the cursor position changes during the clear
    pub fn clear_forwards(&mut self, cursor_pos: &CursorPos) -> Option<Range<usize>> {
        let (buf_pos, _) = self.cursor_to_buf_pos(cursor_pos)?;

        for i in buf_pos..self.buf.len() {
            match self.buf.get(i) {
                Some(TChar::NewLine) => (),
                Some(_) => self.buf[i] = TChar::Space,
                None => break,
            }
        }
        self.line_ranges_to_visible_line_ranges();
        Some(buf_pos..self.buf.len())
    }

    pub fn clear_line_forwards(&mut self, cursor_pos: &CursorPos) -> Option<Range<usize>> {
        // Can return early if none, we didn't delete anything if there is nothing to delete
        let (buf_pos, line_range) = self.cursor_to_buf_pos(cursor_pos)?;

        let del_range = buf_pos..line_range.end;

        for i in del_range.clone() {
            match self.buf.get(i) {
                Some(TChar::NewLine) => (),
                Some(_) => self.buf[i] = TChar::Space,
                None => break,
            }
        }

        self.line_ranges_to_visible_line_ranges();

        Some(del_range)
    }

    pub fn clear_line(&mut self, cursor_pos: &CursorPos) -> Option<Range<usize>> {
        let (_buf_pos, line_range) = self.cursor_to_buf_pos(cursor_pos)?;

        let del_range = line_range;

        for i in del_range.clone() {
            match self.buf.get(i) {
                Some(TChar::NewLine) => (),
                Some(_) => self.buf[i] = TChar::Space,
                None => break,
            }
        }

        self.line_ranges_to_visible_line_ranges();

        Some(del_range)
    }

    pub fn clear_line_backwards(&mut self, cursor_pos: &CursorPos) -> Option<Range<usize>> {
        let (buf_pos, line_range) = self.cursor_to_buf_pos(cursor_pos)?;

        let del_range = line_range.start..buf_pos;

        for i in del_range.clone() {
            match self.buf.get(i) {
                Some(TChar::NewLine) => (),
                Some(_) => self.buf[i] = TChar::Space,
                None => break,
            }
        }
        self.line_ranges_to_visible_line_ranges();

        Some(del_range)
    }

    pub fn clear_all(&mut self) {
        self.buf.clear();
        self.visible_line_ranges.clear();
    }

    pub fn clear_visible(&mut self) -> Option<std::ops::Range<usize>> {
        let visible_line_ranges = self.visible_line_ranges.clone();

        if visible_line_ranges.is_empty() {
            return None;
        }

        // replace all NONE newlines with spaces
        for line in &visible_line_ranges {
            self.buf[line.start..line.end].iter_mut().for_each(|c| {
                if *c != TChar::NewLine {
                    *c = TChar::Space;
                }
            });
        }

        self.line_ranges_to_visible_line_ranges();

        Some(visible_line_ranges[0].start..usize::MAX)
    }

    pub fn delete_forwards(
        &mut self,
        cursor_pos: &CursorPos,
        num_chars: usize,
    ) -> Option<Range<usize>> {
        let (buf_pos, line_range) = self.cursor_to_buf_pos(cursor_pos)?;

        let mut delete_range = buf_pos..buf_pos + num_chars;

        if delete_range.end > line_range.end
            && self.buf.get(line_range.end) != Some(&TChar::NewLine)
        {
            self.buf.insert(line_range.end, TChar::NewLine);
        }

        delete_range.end = line_range.end.min(delete_range.end);

        self.buf.drain(delete_range.clone());
        self.line_ranges_to_visible_line_ranges();
        Some(delete_range)
    }

    pub fn erase_forwards(
        &mut self,
        cursor_pos: &CursorPos,
        num_chars: usize,
    ) -> Option<Range<usize>> {
        let (buf_pos, line_range) = self.cursor_to_buf_pos(cursor_pos)?;

        let mut erase_range = buf_pos..buf_pos + num_chars;

        if erase_range.end > line_range.end {
            erase_range.end = line_range.end;
        }

        // // remove the range from the buffer
        // self.buf.drain(erase_range.clone());
        // replace all characters in range with spaces
        self.buf[erase_range.clone()].fill(TChar::Space);
        self.line_ranges_to_visible_line_ranges();

        Some(erase_range)
    }

    #[must_use]
    pub fn data_for_gui(&self) -> (TerminalSections<Vec<TChar>>, usize, usize) {
        let end = if self.viewable_index_bottom == usize::MAX {
            self.buffer_line_ranges.len().saturating_sub(1)
        } else {
            self.viewable_index_bottom
        };

        if self.buf.is_empty() {
            return (
                TerminalSections {
                    scrollback: vec![],
                    visible: self.buf.clone(),
                },
                0,
                0,
            );
        }

        let start = end.saturating_sub(self.height.saturating_sub(1));

        // ensure the start is not greater than the end

        if start > end {
            error!("Data for GUI: start is greater than end");
            return (
                TerminalSections {
                    scrollback: vec![],
                    visible: vec![],
                },
                0,
                0,
            );
        }

        // now ensure self.buffer_line_ranges[start].start..self.buffer_line_ranges[end].end falls
        // entirely within self.buf
        if self.buffer_line_ranges[start].start > self.buf.len()
            || self.buffer_line_ranges[end].end > self.buf.len()
        {
            error!("Data for GUI: buffer line ranges are out of bounds. start: {}, buf start: {}, buf end: {}, end: {}, buf len: {}, height: {}, visible: {:?}, buffer ranges: {:?}",
                    start, self.buffer_line_ranges[start].start, end, self.buffer_line_ranges[end].end, self.buf.len(), self.height, self.visible_line_ranges.len(), self.buffer_line_ranges.len());

            return (
                TerminalSections {
                    scrollback: vec![],
                    visible: vec![],
                },
                0,
                0,
            );
        }

        (
            TerminalSections {
                scrollback: vec![],
                visible: self.buf
                    [self.buffer_line_ranges[start].start..self.buffer_line_ranges[end].end]
                    .to_vec(),
            },
            self.buffer_line_ranges[start].start,
            self.buffer_line_ranges[end].end,
        )
    }

    // keep around for tests
    #[must_use]
    pub fn data(&self, include_scrollback: bool) -> TerminalSections<Vec<TChar>> {
        let visible_line_ranges = &self.visible_line_ranges;
        if self.buf.is_empty() {
            return TerminalSections {
                scrollback: vec![],
                visible: self.buf.clone(),
            };
        }

        if visible_line_ranges.is_empty() {
            warn!("visible line ranges is empty but data in the buffer!");
            return TerminalSections {
                scrollback: self.buf.clone(),
                visible: vec![],
            };
        }

        let start = visible_line_ranges[0].start;

        TerminalSections {
            scrollback: if include_scrollback {
                self.buf[..start].to_vec()
            } else {
                vec![]
            },
            visible: self.buf[start..].to_vec(),
        }
    }

    #[must_use]
    pub fn clip_lines_for_primary_buffer(&mut self) -> Option<Range<usize>> {
        if self.buf.is_empty() {
            return None;
        }
        // we want to keep the first 2000 lines + length of visible lines

        let index = self
            .buffer_line_ranges
            .len()
            .saturating_sub(2000 - self.visible_line_ranges.len() - 1);

        if index == 0 {
            return None;
        }

        let keep_buf_pos = self.buffer_line_ranges[index].start - 1;

        self.buf.drain(0..keep_buf_pos);
        self.buffer_line_ranges.drain(0..index);

        // now walk both of the line range buffers and offset them by the keep_buf_pos

        for line_range in &mut self.buffer_line_ranges {
            line_range.start = line_range.start.saturating_sub(keep_buf_pos);
            line_range.end = line_range.end.saturating_sub(keep_buf_pos);
        }

        for line_range in &mut self.visible_line_ranges {
            line_range.start = line_range.start.saturating_sub(keep_buf_pos);
            line_range.end = line_range.end.saturating_sub(keep_buf_pos);
        }

        if self.viewable_index_bottom != usize::MAX {
            self.scroll_up(&index);
        }

        Some(0..keep_buf_pos)
    }

    #[must_use]
    pub fn clip_lines_for_alternate_buffer(&mut self) -> Option<Range<usize>> {
        if self.buf.is_empty() {
            return None;
        }
        // we want to keep the first height lines + length of visible lines

        let index = self
            .visible_line_ranges
            .len()
            .saturating_sub(self.height + 1);

        let keep_buf_pos = self.visible_line_ranges[index].start.saturating_sub(1);

        if keep_buf_pos == 0 {
            self.buffer_line_ranges = self.visible_line_ranges.clone();
            return None;
        }

        debug!("Clipping alternate buffer to {keep_buf_pos}");

        self.buf.drain(0..keep_buf_pos);

        // now walk both of the line range buffers and offset them by the keep_buf_pos

        for line_range in &mut self.visible_line_ranges {
            line_range.start = line_range.start.saturating_sub(keep_buf_pos);
            line_range.end = line_range.end.saturating_sub(keep_buf_pos);

            // ensure line range falls within the buffer
            if line_range.start > self.buf.len() - 1 {
                error!(
                    "Line range start is greater than buffer length: {} > {}",
                    line_range.start,
                    self.buf.len()
                );
            }

            if line_range.end > self.buf.len() {
                error!(
                    "Line range end is greater than buffer length: {} > {}",
                    line_range.end,
                    self.buf.len()
                );
            }
        }

        self.buffer_line_ranges = self.visible_line_ranges.clone();

        Some(0..keep_buf_pos)
    }

    // FIXME: I think this is a clippy bug, or I'm stupid. It wants this to be a const fn, but it fails because of a deref error?
    #[allow(clippy::missing_const_for_fn)]
    #[must_use]
    pub fn get_raw_buffer(&self) -> &[TChar] {
        &self.buf
    }

    #[must_use]
    pub const fn get_win_size(&self) -> (usize, usize) {
        (self.width, self.height)
    }

    pub fn set_win_size(
        &mut self,
        width: usize,
        height: usize,
        cursor_pos: &CursorPos,
    ) -> TerminalBufferSetWinSizeResponse {
        let changed = self.width != width || self.height != height;
        if !changed {
            return TerminalBufferSetWinSizeResponse {
                changed: false,
                _insertion_range: 0..0,
                new_cursor_pos: *cursor_pos,
            };
        }

        // Ensure that the cursor position has a valid buffer position. That way when we resize we
        // can just look up where the cursor is supposed to be and map it back to it's new cursor
        // position
        let pad_response = self.pad_buffer_for_write(cursor_pos, 0);

        self.visible_line_ranges.clear();
        self.buffer_line_ranges.clear();

        self.line_ranges_to_visible_line_ranges();
        let buf_pos = pad_response.write_idx;
        let inserted_padding = pad_response.inserted_padding;
        let new_cursor_pos = self.buf_to_cursor_pos(buf_pos);

        self.width = width;
        self.height = height;

        TerminalBufferSetWinSizeResponse {
            changed,
            _insertion_range: inserted_padding,
            new_cursor_pos,
        }
    }

    pub const fn set_top_and_bottom_margins(&mut self, top_margin: usize, bottom_margin: usize) {
        self.top_margin = top_margin.saturating_sub(1);
        self.bottom_margin = if bottom_margin == 0 {
            usize::MAX
        } else {
            bottom_margin.saturating_sub(1)
        };
    }

    /// Given terminal height `height`, extract the visible line ranges from all line ranges (which
    /// include scrollback) assuming "visible" is the bottom N lines
    // FIXME: This is remarkably efficient compared to where we started, but it still could be running 10s of thousands of times per read
    // We should probably only recalculate what has changed and not the entire deal
    pub fn line_ranges_to_visible_line_ranges(&mut self) {
        let buf = &self.buf;
        let height = self.height;
        let width = self.width;
        if buf.is_empty() {
            self.visible_line_ranges = vec![];
            self.buffer_line_ranges = vec![];
            return;
        }

        // FIXME: This entire thing is janky af. It probably needs a rewrite

        // The goal here is to get the visible line ranges from the buffer. This is easy if we walk the buffer from the start, because we can track where lines start and end with ease
        // However, for efficiency reasons we need to walk the buffer *from the back* because there is no sense in going through 500,000 characters, representing 10000+ lines, if we only care about the last x lines that represent the visible screen
        // The problem becomes tricky with line wrapping in this case. If a consecutive sequence of non-newline characters is longer than the width of the terminal, we need to split it into multiple lines, but if the line is not % 0 of the width, then starting at the back and walking forward we will end up with different break points than if we started at the front and walked back.

        let mut current_start = buf.len() - 1; // start of the current line
        let mut ret: Vec<Range<usize>> = Vec::with_capacity(height); // the ranges of the visible lines
        let mut wrapping = false; // flag to indicate if we are wrapping
        let mut previous_char_was_newline = false; // This flag is used to determine some special cases when we are wrapping
        let mut consecutive_newlines = false; // This is used to track the number of consecutive newlines we have encountered. If we have more than the height of the terminal, we need to stop

        // iterate over the buffer in reverse order
        for (position, character) in buf.iter().enumerate().rev() {
            // special case for the last character in the buffer. If the character is a new line, we DO NOT want to include it in the output. Why, not entirely sure. But it's what the original code did
            // Otherwise, we want the line range to capture the character so we set the current start to be inclusive of the character
            if buf.len() - 1 == position {
                if *character == TChar::NewLine {
                    current_start = position;
                } else {
                    current_start = position + 1;
                }
                continue;
            }

            // if we have enough lines, we can break out of the loop
            if ret.len() == height {
                current_start = position;
                break;
            }

            // We've encountered a newline character. This means we need to add a new line to the output
            if character == &TChar::NewLine {
                // If we are wrapping, we need to take the position to the current start, splitting the ranges on width
                if wrapping {
                    // take the position to current start, splitting the ranges on width

                    // The total characters in the line is the current start minus the position because we are already including the start character in the range
                    let mut current_length = current_start.saturating_sub(position);

                    // If the previous character was a newline, we need to subtract one from the length because the newline is implied
                    if previous_char_was_newline {
                        current_length = current_length.saturating_sub(1);
                    }

                    let new_position = position + 1;
                    let to_add = ranges_from_start_and_end(current_length, new_position, width, 0);
                    ret.extend_from_slice(&to_add);
                } else if previous_char_was_newline {
                    // If the previous character was a newline, we need to add an empty line but the range is just going to include the newline character
                    ret.push(position + 1..position + 1);
                } else {
                    // If we are not wrapping, we can just add the line as is
                    ret.push(position + 1..current_start);
                }

                current_start = position;
                consecutive_newlines = previous_char_was_newline;
                previous_char_was_newline = true;
                wrapping = false;

                continue;
            }

            if !wrapping && current_start.saturating_sub(position) > width {
                // if we have not hit the max length already, AND the current line is the width of the terminal, we need to set the wrapping flag. We also set the newline flag in case the very next character is a newline
                // current_start = position;
                previous_char_was_newline = true;
                wrapping = true;
            } else if !wrapping {
                // if we are not wrapping, we need to set the newline flag to false
                previous_char_was_newline = false;
            }
        }

        // Done looping. If we have not hit the max length, we need to add the last line to the output
        if ret.len() < height {
            // If we are wrapping, we need to take the position to the current start, splitting the ranges on width using the same logic as above for wrapping
            if wrapping && current_start > width {
                let mut current_length = current_start;
                let mut offset_end = 1;
                if previous_char_was_newline
                    && !consecutive_newlines
                    && !ret.is_empty()
                    && buf[current_start.saturating_sub(1)] == TChar::NewLine
                {
                    current_length = current_length.saturating_sub(1);
                } else if consecutive_newlines {
                    offset_end = 0;
                }
                let new_position = 0;
                let to_add =
                    ranges_from_start_and_end(current_length, new_position, width, offset_end);
                ret.extend_from_slice(&to_add);
            } else {
                // otherwise, just add the line
                ret.push(0..current_start);
            }
        }

        // sort the ranges by start position
        ret.sort_by(|a, b| a.start.cmp(&b.start));

        // if we have more lines than the height, we need to remove the extra lines
        if ret.len() > height {
            // remove extra lines from the front of the buffer
            let to_remove = ret.len() - height;
            ret.drain(0..to_remove);
        }

        self.visible_line_ranges = ret;
        self.calculate_line_ranges();

        // check and make sure the last buffer line range end is not greater than the buffer length
        if let Some(last) = self.buffer_line_ranges.last() {
            if last.end > self.buf.len() {
                error!("Last buffer line range end is greater than buffer length");
            }
        }
    }

    // FIXME: can we combine this with the above function? Or separate out the internal logic of each function to a common function?
    /// Given terminal height `height`, extract the visible line ranges from all line ranges (which
    /// include scrollback) assuming "visible" is the bottom N lines
    #[must_use]
    pub fn line_ranges(&self, end: usize, current_start: usize) -> Option<Vec<Range<usize>>> {
        let buf = &self.buf;
        let width = self.width;
        let mut current_start = current_start;
        if buf.is_empty() {
            return None;
        }

        // FIXME: This entire thing is janky af. It probably needs a rewrite

        // The goal here is to get the visible line ranges from the buffer. This is easy if we walk the buffer from the start, because we can track where lines start and end with ease
        // However, for efficiency reasons we need to walk the buffer *from the back* because there is no sense in going through 500,000 characters, representing 10000+ lines, if we only care about the last x lines that represent the visible screen
        // The problem becomes tricky with line wrapping in this case. If a consecutive sequence of non-newline characters is longer than the width of the terminal, we need to split it into multiple lines, but if the line is not % 0 of the width, then starting at the back and walking forward we will end up with different break points than if we started at the front and walked back.

        let mut ret: Vec<Range<usize>> = Vec::with_capacity(end); // the ranges of the visible lines
        let mut wrapping = false; // flag to indicate if we are wrapping
        let mut previous_char_was_newline = false; // This flag is used to determine some special cases when we are wrapping
        let mut consecutive_newlines = false; // This is used to track the number of consecutive newlines we have encountered. If we have more than the height of the terminal, we need to stop

        let test_current_start = current_start;
        let test_end = end;

        // iterate over the buffer in reverse order
        for (position, character) in buf
            .iter()
            .enumerate()
            .filter(|(i, _): &(usize, &TChar)| i <= &test_current_start && i >= &test_end)
            .rev()
        {
            // special case for the last character in the buffer. If the character is a new line, we DO NOT want to include it in the output. Why, not entirely sure. But it's what the original code did
            // Otherwise, we want the line range to capture the character so we set the current start to be inclusive of the character
            if test_current_start == position {
                if *character == TChar::NewLine {
                    current_start = position;
                } else {
                    current_start = position + 1;
                }
                continue;
            }

            // We've encountered a newline character. This means we need to add a new line to the output
            if character == &TChar::NewLine {
                // If we are wrapping, we need to take the position to the current start, splitting the ranges on width
                if wrapping {
                    // take the position to current start, splitting the ranges on width

                    // The total characters in the line is the current start minus the position because we are already including the start character in the range
                    let mut current_length = current_start.saturating_sub(position);

                    // If the previous character was a newline, we need to subtract one from the length because the newline is implied
                    if previous_char_was_newline {
                        current_length = current_length.saturating_sub(1);
                    }

                    let new_position = position + 1;
                    let to_add = ranges_from_start_and_end(current_length, new_position, width, 0);
                    ret.extend_from_slice(&to_add);
                    wrapping = false;
                } else if previous_char_was_newline {
                    // If the previous character was a newline, we need to add an empty line but the range is just going to include the newline character
                    ret.push(position + 1..position + 1);
                } else {
                    // If we are not wrapping, we can just add the line as is
                    ret.push(position + 1..current_start);
                }

                current_start = position;
                consecutive_newlines = previous_char_was_newline;
                previous_char_was_newline = true;

                continue;
            }

            if !wrapping && current_start.saturating_sub(position) > width {
                // if we have not hit the max length already, AND the current line is the width of the terminal, we need to set the wrapping flag. We also set the newline flag in case the very next character is a newline
                // current_start = position;
                previous_char_was_newline = true;
                wrapping = true;
            } else if !wrapping {
                // if we are not wrapping, we need to set the newline flag to false
                previous_char_was_newline = false;
            }
        }

        // Done looping. If we have not hit the max length, we need to add the last line to the output

        // If we are wrapping, we need to take the position to the current start, splitting the ranges on width using the same logic as above for wrapping

        if wrapping && current_start > test_end {
            let mut current_length = current_start;
            let mut offset_end = 1;

            if previous_char_was_newline && !consecutive_newlines {
                current_length = current_length.saturating_sub(1);
            } else if consecutive_newlines {
                offset_end = 0;
            }
            let new_position = test_end;
            let to_add = ranges_from_start_and_end(current_length, new_position, width, offset_end);

            ret.extend_from_slice(&to_add);
        } else {
            // otherwise, just add the line
            ret.push(test_end..current_start);
        }

        // sort the ranges by start position
        ret.sort_by(|a, b| a.start.cmp(&b.start));

        // if we have more lines than the height, we need to remove the extra lines
        // if ret.len() > end {
        //     // remove extra lines from the front of the buffer
        //     let to_remove = ret.len() - end;
        //     ret.drain(0..to_remove);
        // }

        Some(ret)
    }

    fn find_index_containing_range(
        &self,
        visible_start: usize,
        visible_end: usize,
    ) -> Option<usize> {
        let max = self.buffer_line_ranges.len().saturating_sub(1);

        if max == 0 {
            return None;
        }

        for index in (0..=max).rev() {
            let r = &self.buffer_line_ranges[index];
            // the base base that we have a range that our range is included/the same as a range already in the buffer
            if visible_end <= r.end && visible_start >= r.start
            // the other case is that we're matching against a single character and we want to match if the range is included in the visible_start..visible_end range
                || (r.start == r.end
                    && ((visible_start..visible_end).contains(&r.start)
                        || (visible_start..visible_end).contains(&r.end)))
            {
                return Some(index);
            }

            // otherwise, we need to walk back and see if we can find a range that is included in the visible range

            if index == max {
                continue;
            }

            for i in index + 1..=max {
                let r = r.start..self.buffer_line_ranges[i].end;

                // the base base that we have a range that our range is included/the same as a range already in the buffer
                if visible_end <= r.end && visible_start >= r.start
                // the other case is that we're matching against a single character and we want to match if the range is included in the visible_start..visible_end range
                || (r.start == r.end
                    && ((visible_start..visible_end).contains(&r.start)
                        || (visible_start..visible_end).contains(&r.end)))
                {
                    return Some(index);
                }
            }
        }

        None
    }

    pub fn calculate_line_ranges(&mut self) {
        // we need to compare visible line ranges to the bottom x values of buffer line ranges
        // the idea here is that we want to have line ranges for the scroll back buffer, but if

        // we want to push any new/changed lines to the buffer line ranges
        // as well as update any changed lines.

        if self.visible_line_ranges.is_empty()
            || self.visible_line_ranges.len() < self.height
            || self.buffer_type == BufferType::Alternate
        {
            self.buffer_line_ranges = self.visible_line_ranges.clone();
            return;
        }

        let visible_line_ranges = &self.visible_line_ranges;

        // find the start position of the visible lines in the buffer_line_ranges
        let visible_start = visible_line_ranges[0].start;
        let visible_end = visible_line_ranges[0].end;
        // let mut visible_start = visible_line_ranges[0].start;
        // let mut visible_end = visible_line_ranges[0].end;

        if let Some(i) = self.find_index_containing_range(visible_start, visible_end) {
            // if we found the start of the visible lines in the buffer line ranges, we need to update the buffer line ranges
            self.buffer_line_ranges.truncate(i);
            // buffer_line_ranges.extend_from_slice(visible_line_ranges);
        } else {
            // the visible line ranges do not overlap with the buffer line ranges
            // this means likely we have added more lines to the buffer then we have visible
            // we need to walk the buffer from the end of line ranges to the start of the visible line ranges to find the start of the visible lines and add those missing lines to the buffer line ranges
            let mut start_pos = visible_start.saturating_sub(1);
            let mut end = self.buffer_line_ranges.last().unwrap_or(&(0..0)).end;
            let mut walk_anyway = false;
            if self.buf[end.saturating_sub(1)] == TChar::NewLine {
                start_pos -= 1;
            }

            if start_pos < end {
                error!(
                    "start pos is less than end: {} < {}. Test position: {}/{}",
                    start_pos, end, visible_start, visible_end
                );
                error!(
                    "visible:\n{:?}\nbuffer:\n{:?}",
                    visible_line_ranges, self.buffer_line_ranges
                );
                error!("Resetting line buffer and walking the whole thing. RIP CPU.");
                self.buffer_line_ranges.clear();
                end = 0;
                walk_anyway = true;
            }

            if walk_anyway || (start_pos != end && start_pos.saturating_sub(1) > 0) {
                let to_add = self.line_ranges(end, start_pos);

                if let Some(to_add) = to_add {
                    // if the character at the end of the buffer_line_ranges is a new line, we need to remove it
                    if self.buf[end.saturating_sub(1)] == TChar::NewLine {
                        self.buffer_line_ranges
                            .truncate(self.buffer_line_ranges.len().saturating_sub(1));
                    }

                    self.buffer_line_ranges.extend_from_slice(&to_add);
                }
            }
        }

        self.buffer_line_ranges
            .extend_from_slice(visible_line_ranges);
    }

    fn buf_to_cursor_pos(&self, buf_pos: usize) -> CursorPos {
        let visible_line_ranges = &self.visible_line_ranges;
        let (new_cursor_y, new_cursor_line) = if let Some((i, r)) = visible_line_ranges
            .iter()
            .enumerate()
            .find(|(_i, r)| r.end >= buf_pos)
        {
            (i, r.clone())
        } else {
            info!("Buffer position not on screen");
            return CursorPos::default();
        };

        if buf_pos < new_cursor_line.start {
            info!("Old cursor position no longer on screen");
            return CursorPos::default();
        }

        let new_cursor_x = buf_pos - new_cursor_line.start;

        CursorPos {
            x: new_cursor_x,
            y: new_cursor_y,
        }
    }

    #[must_use]
    pub fn cursor_pos_to_buf_pos(&self, cursor_pos: &CursorPos) -> Option<usize> {
        let visible_line_ranges = &self.visible_line_ranges;
        let line_range = visible_line_ranges.get(cursor_pos.y)?;

        let buf_pos = line_range.start + cursor_pos.x;
        if buf_pos >= line_range.end {
            None
        } else {
            Some(buf_pos)
        }
    }

    pub fn pad_buffer_for_write(
        &mut self,
        cursor_pos: &CursorPos,
        write_len: usize,
    ) -> PadBufferForWriteResponse {
        let visible_line_ranges = &mut self.visible_line_ranges;
        let buf = &mut self.buf;

        let mut padding_start_pos = None;
        let mut num_inserted_characters = 0;

        let vertical_padding_needed = if cursor_pos.y + 1 > visible_line_ranges.len() {
            cursor_pos.y + 1 - visible_line_ranges.len()
        } else {
            0
        };

        if vertical_padding_needed != 0 {
            padding_start_pos = Some(buf.len());
            num_inserted_characters += vertical_padding_needed;
        }

        for _ in 0..vertical_padding_needed {
            buf.push(TChar::NewLine);
            let newline_pos = buf.len() - 1;
            visible_line_ranges.push(newline_pos..newline_pos);
        }

        let line_range = &visible_line_ranges[cursor_pos.y];

        let desired_start = line_range.start + cursor_pos.x;
        let desired_end = desired_start + write_len;

        // NOTE: We only want to pad if we hit an early newline. If we wrapped because we hit the edge
        // of the screen we can just keep writing and the wrapping will stay as is. This is an
        // important distinction because in the no-newline case we want to make sure we overwrite
        // whatever was in the buffer before
        let actual_end = buf
            .iter()
            .enumerate()
            .skip(line_range.start)
            .find_map(|(i, c)| match *c {
                TChar::NewLine => Some(i),
                _ => None,
            })
            .unwrap_or(buf.len());

        // If we did not set the padding start position, it means that we are padding not at the end of
        // the buffer, but at the end of a line
        if padding_start_pos.is_none() {
            padding_start_pos = Some(actual_end);
        }

        let number_of_spaces = desired_end.saturating_sub(actual_end);

        num_inserted_characters += number_of_spaces;

        for i in 0..number_of_spaces {
            buf.insert(actual_end + i, TChar::Space);
        }

        let start_buf_pos = padding_start_pos.map_or_else(
            || {
                // If we did not insert padding, we are at the end of a line
                error!("Padding start position not set and it should have been. This is a bug");
                actual_end
            },
            |p| p,
        );

        PadBufferForWriteResponse {
            write_idx: desired_start,
            inserted_padding: start_buf_pos..start_buf_pos + num_inserted_characters,
        }
    }

    fn cursor_to_buf_pos(&self, cursor_pos: &CursorPos) -> Option<(usize, Range<usize>)> {
        let visible_line_ranges = &self.visible_line_ranges;
        visible_line_ranges.get(cursor_pos.y).and_then(|range| {
            let candidate_pos = range.start + cursor_pos.x;
            if candidate_pos > range.end {
                None
            } else {
                Some((candidate_pos, range.clone()))
            }
        })
    }
}

fn ranges_from_start_and_end(
    current_length: usize,
    position: usize,
    width: usize,
    offset_end: usize,
) -> Vec<Range<usize>> {
    let mut to_add = vec![];
    let mut current_length = current_length;
    let mut current_range = position..position;

    if current_length <= width {
        to_add.push(position..position + current_length);

        return to_add;
    }

    let mut did_just_add: bool;
    loop {
        did_just_add = false;
        current_range.end += 1;

        if current_range.end - current_range.start == width {
            to_add.push(current_range.clone());
            current_range.start = current_range.end;
            did_just_add = true;
        }

        if current_length.saturating_sub(1) == 0 {
            break;
        }

        current_length -= 1;
    }

    if !did_just_add {
        current_range.end += offset_end;
        to_add.push(current_range);
    }

    to_add
}
