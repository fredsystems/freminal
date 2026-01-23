// Copyright (C) 2024-2025 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use anyhow::Result;
use freminal_common::buffer_states::{
    cursor::{CursorState, StateColors},
    fonts::FontWeight,
    format_tag::FormatTag,
};
use std::ops::Range;

#[must_use]
pub const fn ranges_overlap(a: &Range<usize>, b: &Range<usize>) -> bool {
    !(a.end <= b.start || a.start >= b.end)
}
/// if a and b overlap like
/// a:  [         ]
/// b:      [  ]
const fn range_fully_contains(a: &Range<usize>, b: &Range<usize>) -> bool {
    a.start <= b.start && a.end >= b.end
}

/// if a and b overlap like
/// a:     [      ]
/// b:  [     ]
const fn range_starts_overlapping(a: &Range<usize>, b: &Range<usize>) -> bool {
    a.start > b.start && a.end > b.end
}

/// if a and b overlap like
/// a: [      ]
/// b:    [      ]
const fn range_ends_overlapping(a: &Range<usize>, b: &Range<usize>) -> bool {
    range_starts_overlapping(b, a)
}

fn adjust_existing_format_range(
    existing_elem: &mut FormatTag,
    range: &Range<usize>,
) -> ColorRangeAdjustment {
    let mut ret = ColorRangeAdjustment {
        should_delete: false,
        to_insert: None,
    };

    let existing_range = existing_elem.start..existing_elem.end;
    if range_fully_contains(range, &existing_range) {
        ret.should_delete = true;
    } else if range_fully_contains(&existing_range, range) {
        if existing_elem.start == range.start {
            ret.should_delete = true;
        }

        if range.end != existing_elem.end {
            ret.to_insert = Some(FormatTag {
                start: range.end,
                end: existing_elem.end,
                colors: existing_elem.colors.clone(),
                font_weight: existing_elem.font_weight.clone(),
                font_decorations: existing_elem.font_decorations.clone(),
                url: existing_elem.url.clone(),
            });
        }

        existing_elem.end = range.start;
    } else if range_starts_overlapping(range, &existing_range) {
        existing_elem.end = range.start;
        if existing_elem.start == existing_elem.end {
            ret.should_delete = true;
        }
    } else if range_ends_overlapping(range, &existing_range) {
        existing_elem.start = range.end;
        if existing_elem.start == existing_elem.end {
            ret.should_delete = true;
        }
    } else {
        panic!(
            "Unhandled case {}-{}, {}-{}",
            existing_elem.start, existing_elem.end, range.start, range.end
        );
    }

    ret
}

fn delete_items_from_vec<T>(mut to_delete: Vec<usize>, vec: &mut Vec<T>) {
    to_delete.sort_unstable();
    for idx in to_delete.iter().rev() {
        vec.remove(*idx);
    }
}

fn adjust_existing_format_ranges(existing: &mut Vec<FormatTag>, range: &Range<usize>) {
    let mut effected_infos = existing
        .iter_mut()
        .enumerate()
        .filter(|(_i, item)| ranges_overlap(&(item.start..item.end), range))
        .collect::<Vec<_>>();

    let mut to_delete = Vec::new();
    let mut to_push = Vec::new();
    for info in &mut effected_infos {
        let adjustment = adjust_existing_format_range(info.1, range);
        if adjustment.should_delete {
            to_delete.push(info.0);
        }
        if let Some(item) = adjustment.to_insert {
            to_push.push(item);
        }
    }

    delete_items_from_vec(to_delete, existing);
    existing.extend(to_push);
}

struct ColorRangeAdjustment {
    // If a range adjustment results in a 0 width element we need to delete it
    should_delete: bool,
    // If a range was split we need to insert a new one
    to_insert: Option<FormatTag>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormatTracker {
    color_info: Vec<FormatTag>,
}

impl Default for FormatTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl FormatTracker {
    #[must_use]
    pub fn new() -> Self {
        Self {
            color_info: vec![FormatTag {
                start: 0,
                end: usize::MAX,
                colors: StateColors::default(),
                font_weight: FontWeight::Normal,
                font_decorations: Vec::new(),
                url: None,
            }],
        }
    }

    pub fn push_range(&mut self, cursor: &CursorState, range: Range<usize>) {
        adjust_existing_format_ranges(&mut self.color_info, &range);

        self.color_info.push(FormatTag {
            start: range.start,
            end: range.end,
            colors: cursor.colors.clone(),
            font_weight: cursor.font_weight.clone(),
            font_decorations: cursor.font_decorations.clone(),
            url: cursor.url.clone(),
        });

        // FIXME: Insertion sort
        // FIXME: Merge adjacent
        self.color_info.sort_by(|a, b| a.start.cmp(&b.start));
    }

    /// Move all tags > range.start to range.start + range.len
    /// No gaps in coloring data, so one range must expand instead of just be adjusted
    pub fn push_range_adjustment(&mut self, range: Range<usize>) {
        let range_len = range.end - range.start;
        for info in &mut self.color_info {
            if info.end <= range.start {
                continue;
            }
            if info.start > range.start {
                info.start += range_len;
                if info.end != usize::MAX {
                    info.end += range_len;
                }
            } else if info.end != usize::MAX {
                info.end += range_len;
            }
        }
    }

    #[must_use]
    pub fn tags(&self) -> Vec<FormatTag> {
        self.color_info.clone()
    }

    /// Delete ranges
    ///
    /// # Errors
    /// if the ranges overlap in an unhandled way it will return an error
    pub fn delete_range(&mut self, range: Range<usize>) -> Result<()> {
        let mut to_delete = Vec::new();
        let del_size = range.end - range.start;

        for (i, info) in &mut self.color_info.iter_mut().enumerate() {
            let info_range = info.start..info.end;
            if info.end <= range.start {
                continue;
            }

            if ranges_overlap(&range, &info_range) {
                if range_fully_contains(&range, &info_range) {
                    to_delete.push(i);
                } else if range_starts_overlapping(&range, &info_range) {
                    if info.end != usize::MAX {
                        info.end = range.start;
                    }
                } else if range_ends_overlapping(&range, &info_range) {
                    info.start = range.start;
                    if info.end != usize::MAX {
                        info.end -= del_size;
                    }
                } else if range_fully_contains(&info_range, &range) {
                    if info.end != usize::MAX {
                        info.end -= del_size;
                    }
                } else {
                    return Err(anyhow::anyhow!(
                        "Unhandled overlap case {}-{}, {}-{}",
                        info.start,
                        info.end,
                        range.start,
                        range.end
                    ));
                }
            } else {
                //assert!(!ranges_overlap(range.clone(), info_range.clone()));

                if ranges_overlap(&range, &info_range) {
                    return Err(anyhow::anyhow!(
                        "Unhandled overlap case {}-{}, {}-{}",
                        info.start,
                        info.end,
                        range.start,
                        range.end
                    ));
                }
                info.start -= del_size;
                if info.end != usize::MAX {
                    info.end -= del_size;
                }
            }
        }

        for i in to_delete.into_iter().rev() {
            self.color_info.remove(i);
        }

        Ok(())
    }
}
