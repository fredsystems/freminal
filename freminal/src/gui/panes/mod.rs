// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Pane model for built-in terminal multiplexing.
//!
//! This module provides the data structures for split-pane terminal support:
//!
//! - [`PaneId`] — monotonic unique identifier for each pane.
//! - [`PaneIdGenerator`] — allocator for `PaneId` values.
//! - [`Pane`] — per-terminal struct owning PTY channels, snapshot handle, and view state.
//! - [`SplitDirection`] — horizontal vs vertical split axis.
//! - [`PaneTree`] — binary tree of panes with recursive layout, split, close, and resize.
//! - [`PaneError`] — typed errors for tree operations.
//!
//! The pane tree lives entirely on the GUI thread. PTY threads are unaware of
//! the tree structure — they just own their `TerminalEmulator` and publish
//! snapshots via `ArcSwap`.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;

use arc_swap::ArcSwap;
use crossbeam_channel::{Receiver, Sender};
use egui::Rect;
use freminal_common::buffer_states::tchar::TChar;
use freminal_common::pty_write::PtyWrite;
use freminal_terminal_emulator::io::{InputEvent, WindowCommand};
use freminal_terminal_emulator::snapshot::TerminalSnapshot;

use super::terminal::PaneRenderCache;
use super::terminal::RenderState;
use super::view_state::ViewState;

// ── PaneId ───────────────────────────────────────────────────────────

/// A unique, monotonically increasing identifier for each pane.
///
/// Used to track pane identity within a tab's pane tree without relying
/// on tree structure or indices, which change when panes are split or closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PaneId(u64);

impl PaneId {
    /// The initial `PaneId` used for the first pane (id 0).
    #[must_use]
    pub const fn first() -> Self {
        Self(0)
    }

    /// Return the raw inner value.
    #[must_use]
    pub const fn raw(self) -> u64 {
        self.0
    }
}

impl std::fmt::Display for PaneId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Pane({})", self.0)
    }
}

// ── PaneIdGenerator ──────────────────────────────────────────────────

/// Counter for generating unique `PaneId` values.
///
/// Global to the application — every pane ever created gets a unique id,
/// even across different tabs. This avoids id collisions when panes are
/// moved between tabs in the future.
#[derive(Debug)]
pub struct PaneIdGenerator {
    next: u64,
}

impl PaneIdGenerator {
    /// Create a new generator. The first id it produces will be `PaneId(start)`.
    #[must_use]
    pub const fn new(start: u64) -> Self {
        Self { next: start }
    }

    /// Allocate the next unique `PaneId`.
    pub const fn next_id(&mut self) -> PaneId {
        let id = PaneId(self.next);
        self.next += 1;
        id
    }
}

impl Default for PaneIdGenerator {
    fn default() -> Self {
        Self::new(0)
    }
}

// ── Pane ─────────────────────────────────────────────────────────────

/// A single terminal pane.
///
/// Each pane owns an independent set of channels to its PTY consumer thread
/// and a shared snapshot handle. These are the same fields that currently
/// live on `Tab`; in subtask 58.3, `Tab` will be refactored to hold a
/// `PaneTree` of `Pane` instances instead.
///
/// The pane tree lives on the GUI thread. PTY threads are unaware of the
/// tree structure — they just own their `TerminalEmulator` and publish
/// snapshots via `ArcSwap`.
pub struct Pane {
    /// Unique identifier for this pane.
    pub id: PaneId,

    /// The latest terminal snapshot published by this pane's PTY consumer thread.
    pub arc_swap: Arc<ArcSwap<TerminalSnapshot>>,

    /// Channel sender for input events (key, resize, focus) to this pane's PTY thread.
    pub input_tx: Sender<InputEvent>,

    /// Sender for raw bytes back to this pane's PTY (for Report* responses).
    pub pty_write_tx: Sender<PtyWrite>,

    /// Receiver for window manipulation commands from this pane's PTY thread.
    pub window_cmd_rx: Receiver<WindowCommand>,

    /// Receiver for clipboard text extraction responses from this pane's PTY thread.
    pub clipboard_rx: Receiver<String>,

    /// Receiver for full-buffer search content from this pane's PTY thread.
    ///
    /// When the GUI sends `InputEvent::RequestSearchBuffer`, the PTY thread
    /// concatenates scrollback + visible `TChar` data and sends it here.
    /// The first element of the tuple is `total_rows` at the time the buffer
    /// was captured, used by the GUI to detect stale responses.
    pub search_buffer_rx: Receiver<(usize, Vec<TChar>)>,

    /// Signals that this pane's PTY process has exited.
    ///
    /// The PTY consumer thread sends `()` when the child exits or the PTY read
    /// channel closes. The GUI polls this to close the pane (or the whole
    /// tab when it is the last remaining pane).
    pub pty_dead_rx: Receiver<()>,

    /// Pane title, set by OSC 0/2 escape sequences.
    ///
    /// When empty, the visible label is supplied by pane creation or tab-bar UI
    /// fallback logic rather than a guaranteed model-level default title.
    pub title: String,

    /// Whether a bell has fired in this pane and not yet been cleared.
    pub bell_active: bool,

    /// Per-pane title stack for `SaveWindowTitleToStack` /
    /// `RestoreWindowTitleFromStack` (CSI 22/23 t). Each pane maintains its
    /// own stack so that background shells pushing/popping titles do not
    /// interfere with the active pane.
    pub title_stack: Vec<String>,

    /// Per-pane GUI view state (scroll offset, selection, blink, mouse).
    pub view_state: ViewState,

    /// Shared atomic flag reflecting whether the PTY slave currently has
    /// `ECHO` disabled (i.e. a password prompt is active).
    ///
    /// Read directly every frame by the GUI (cheap `Relaxed` atomic load)
    /// instead of going through `TerminalSnapshot`, because snapshots are
    /// only published on PTY output — if the shell is idle waiting for a
    /// password, the snapshot would be stale.
    pub echo_off: Arc<AtomicBool>,

    /// Per-pane GPU resources (renderer, glyph atlas, vertex buffers).
    ///
    /// Each pane owns an independent `RenderState` so that panes can maintain
    /// their own GL state (VAOs, VBOs, atlas texture) without conflicts.
    pub(crate) render_state: Arc<Mutex<RenderState>>,

    /// Per-pane dirty-tracking cache for incremental rendering.
    ///
    /// Tracks the previous frame's cursor, theme, selection, and content pointers
    /// to detect what changed and enable fast-path (cursor-only) updates.
    pub(crate) render_cache: PaneRenderCache,
}

impl std::fmt::Debug for Pane {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pane")
            .field("id", &self.id)
            .field("title", &self.title)
            .field("bell_active", &self.bell_active)
            .finish_non_exhaustive()
    }
}

// ── SplitDirection ───────────────────────────────────────────────────

/// Axis along which a pane split divides space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitDirection {
    /// Left | Right — the divider is a vertical line.
    Horizontal,
    /// Top / Bottom — the divider is a horizontal line.
    Vertical,
}

// ── PaneError ────────────────────────────────────────────────────────

/// Typed errors for [`PaneTree`] operations.
#[derive(Debug, thiserror::Error)]
pub enum PaneError {
    /// The requested `PaneId` does not exist in the tree.
    #[error("pane {0} not found in the tree")]
    NotFound(PaneId),

    /// Attempted to close the last remaining pane.
    #[error("cannot close the last pane")]
    CannotCloseLastPane,

    /// No ancestor split in the requested direction for resize.
    #[error("no split in direction {direction:?} above pane {pane}")]
    NoSplitInDirection {
        /// The pane from which the resize was attempted.
        pane: PaneId,
        /// The direction that was requested.
        direction: SplitDirection,
    },

    /// The pane tree is in an invalid state (internal invariant violation).
    ///
    /// This should never happen in normal operation. If it does, it
    /// indicates a bug in the pane tree implementation.
    #[error("pane tree is in an invalid state")]
    InvalidState,
}

// ── ClosedPaneResult ─────────────────────────────────────────────────

/// Result of a successful [`PaneTree::close`] operation.
///
/// The caller is responsible for cleaning up the closed pane's PTY channels
/// (dropping the `Pane` will close them naturally).
#[derive(Debug)]
pub struct ClosedPaneResult {
    /// The pane that was removed from the tree.
    pub closed_pane: Pane,
}

// ── SplitBorder ──────────────────────────────────────────────────────

/// Describes a single split border between adjacent panes.
///
/// Used by the GUI to create invisible drag sensor rects on the border,
/// enabling mouse drag-to-resize. Each border maps to exactly one split
/// node in the tree: dragging the border changes that node's `ratio`.
#[derive(Debug, Clone)]
pub struct SplitBorder {
    /// The split axis.
    ///
    /// - `Horizontal`: a vertical dividing line (drag left/right).
    /// - `Vertical`: a horizontal dividing line (drag up/down).
    pub direction: SplitDirection,

    /// A pane id from the **first** child of the split node.
    ///
    /// Passed to [`PaneTree::resize_split`] to identify which split to
    /// adjust. The `resize_split` algorithm searches for the nearest
    /// ancestor split matching `direction`, so any leaf in the first
    /// subtree will find the correct node.
    pub first_child_pane: PaneId,

    /// The rectangle of the border line.
    ///
    /// For a horizontal split (vertical line): a thin vertical rect at
    /// the split x-coordinate, spanning the full height of the parent.
    /// For a vertical split (horizontal line): a thin horizontal rect at
    /// the split y-coordinate, spanning the full width of the parent.
    pub rect: Rect,

    /// The extent of the parent node along the split axis.
    ///
    /// - For a horizontal split (vertical line), this is the width of the parent.
    /// - For a vertical split (horizontal line), this is the height of the parent.
    ///
    /// Used by the GUI to correctly scale pixel drag distance into ratio delta.
    pub parent_extent: f32,

    /// Whether the active pane lives in the **first** child's subtree.
    ///
    /// Used by the GUI to implement tmux-style half-highlighted borders:
    /// when `true`, the first half (top or left) of the border is drawn
    /// in the active color; when `false`, the second half is highlighted.
    /// If the active pane is in neither subtree, both halves are inactive.
    pub active_in_first: Option<bool>,
}

// ── PaneNode (internal) ─────────────────────────────────────────────

/// Minimum fraction for a split ratio, preventing invisible panes.
const MIN_SPLIT_RATIO: f32 = 0.1;

/// Maximum fraction for a split ratio, preventing invisible panes.
const MAX_SPLIT_RATIO: f32 = 0.9;

/// Internal tree node. Callers interact with [`PaneTree`] instead.
///
/// `Pane` is boxed inside `Leaf` to keep the enum small — `Split` only
/// holds two `Box<Self>`, an `f32`, and a `SplitDirection`.
enum PaneNode {
    /// A terminal pane (leaf node).
    Leaf(Box<Pane>),

    /// An internal split node dividing space between two subtrees.
    Split {
        /// The axis of the split.
        direction: SplitDirection,

        /// Fraction of space allocated to the first child (0.0..=1.0).
        /// Clamped to [`MIN_SPLIT_RATIO`]..=[`MAX_SPLIT_RATIO`].
        ratio: f32,

        /// The first (left or top) child.
        first: Box<Self>,

        /// The second (right or bottom) child.
        second: Box<Self>,
    },
}

impl std::fmt::Debug for PaneNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Leaf(pane) => f.debug_tuple("Leaf").field(&**pane).finish(),
            Self::Split {
                direction,
                ratio,
                first,
                second,
            } => f
                .debug_struct("Split")
                .field("direction", direction)
                .field("ratio", ratio)
                .field("first", first)
                .field("second", second)
                .finish(),
        }
    }
}

impl PaneNode {
    /// Return the number of leaf panes in this subtree.
    fn pane_count(&self) -> usize {
        match self {
            Self::Leaf(_) => 1,
            Self::Split { first, second, .. } => first.pane_count() + second.pane_count(),
        }
    }

    /// Find a pane by id (immutable).
    fn find(&self, id: PaneId) -> Option<&Pane> {
        match self {
            Self::Leaf(pane) if pane.id == id => Some(pane),
            Self::Leaf(_) => None,
            Self::Split { first, second, .. } => first.find(id).or_else(|| second.find(id)),
        }
    }

    /// Find a pane by id (mutable).
    fn find_mut(&mut self, id: PaneId) -> Option<&mut Pane> {
        match self {
            Self::Leaf(pane) if pane.id == id => Some(pane),
            Self::Leaf(_) => None,
            Self::Split { first, second, .. } => first.find_mut(id).or_else(|| second.find_mut(id)),
        }
    }

    /// Check if the target pane exists in this subtree.
    fn contains(&self, id: PaneId) -> bool {
        self.find(id).is_some()
    }

    /// Collect all leaf pane references into `out`.
    fn collect_panes<'a>(&'a self, out: &mut Vec<&'a Pane>) {
        match self {
            Self::Leaf(pane) => out.push(pane),
            Self::Split { first, second, .. } => {
                first.collect_panes(out);
                second.collect_panes(out);
            }
        }
    }

    /// Collect all mutable leaf pane references into `out`.
    fn collect_panes_mut<'a>(&'a mut self, out: &mut Vec<&'a mut Pane>) {
        match self {
            Self::Leaf(pane) => out.push(pane),
            Self::Split { first, second, .. } => {
                first.collect_panes_mut(out);
                second.collect_panes_mut(out);
            }
        }
    }

    /// Compute the layout rectangle for each leaf pane.
    fn layout(&self, rect: Rect, out: &mut Vec<(PaneId, Rect)>) {
        match self {
            Self::Leaf(pane) => {
                out.push((pane.id, rect));
            }
            Self::Split {
                direction,
                ratio,
                first,
                second,
            } => {
                let (r1, r2) = split_rect(rect, *direction, *ratio);
                first.layout(r1, out);
                second.layout(r2, out);
            }
        }
    }

    /// Collect all split borders with their screen rects.
    ///
    /// For each `Split` node, computes the split coordinate and emits a
    /// [`SplitBorder`] whose `rect` is a thin (1px) strip at the split
    /// line spanning the full cross-axis extent of the parent rect.
    /// Then recurses into both children with their respective sub-rects.
    ///
    /// `active_pane` is the currently focused pane; used to compute
    /// [`SplitBorder::active_in_first`] for tmux-style half-highlighted
    /// borders.
    fn split_borders(&self, rect: Rect, active_pane: PaneId, out: &mut Vec<SplitBorder>) {
        match self {
            Self::Leaf(_) => {}
            Self::Split {
                direction,
                ratio,
                first,
                second,
            } => {
                let (r1, r2) = split_rect(rect, *direction, *ratio);

                // Emit a border descriptor for this split.
                let border_rect = match direction {
                    SplitDirection::Horizontal => {
                        // Vertical dividing line at split_x.
                        let split_x = r1.max.x;
                        Rect::from_min_max(
                            egui::pos2(split_x - 0.5, rect.min.y),
                            egui::pos2(split_x + 0.5, rect.max.y),
                        )
                    }
                    SplitDirection::Vertical => {
                        // Horizontal dividing line at split_y.
                        let split_y = r1.max.y;
                        Rect::from_min_max(
                            egui::pos2(rect.min.x, split_y - 0.5),
                            egui::pos2(rect.max.x, split_y + 0.5),
                        )
                    }
                };

                // Determine which subtree (if either) contains the active pane.
                let active_in_first = if first.contains(active_pane) {
                    Some(true)
                } else if second.contains(active_pane) {
                    Some(false)
                } else {
                    None
                };

                // Find any leaf pane in the first subtree to use as the
                // target_id for resize_split. The leftmost/topmost leaf
                // is always reachable and will find this split node.
                if let Some(first_leaf_id) = first.first_leaf_id() {
                    out.push(SplitBorder {
                        direction: *direction,
                        first_child_pane: first_leaf_id,
                        rect: border_rect,
                        parent_extent: match direction {
                            SplitDirection::Horizontal => rect.width(),
                            SplitDirection::Vertical => rect.height(),
                        },
                        active_in_first,
                    });
                }

                // Recurse into children.
                first.split_borders(r1, active_pane, out);
                second.split_borders(r2, active_pane, out);
            }
        }
    }

    /// Return the `PaneId` of the leftmost/topmost leaf in this subtree.
    fn first_leaf_id(&self) -> Option<PaneId> {
        match self {
            Self::Leaf(pane) => Some(pane.id),
            Self::Split { first, .. } => first.first_leaf_id(),
        }
    }

    /// Split the target leaf into a split node. Takes `self` by value and
    /// returns the transformed tree. The new pane becomes the second child.
    fn split(self, target_id: PaneId, direction: SplitDirection, new_pane: Pane) -> Self {
        match self {
            Self::Leaf(pane) if pane.id == target_id => Self::Split {
                direction,
                ratio: 0.5,
                first: Box::new(Self::Leaf(pane)),
                second: Box::new(Self::Leaf(Box::new(new_pane))),
            },
            leaf @ Self::Leaf(_) => leaf,
            Self::Split {
                direction: d,
                ratio,
                first,
                second,
            } => {
                // Route to the correct subtree. Only one can contain the target.
                if first.contains(target_id) {
                    Self::Split {
                        direction: d,
                        ratio,
                        first: Box::new(first.split(target_id, direction, new_pane)),
                        second,
                    }
                } else {
                    Self::Split {
                        direction: d,
                        ratio,
                        first,
                        second: Box::new(second.split(target_id, direction, new_pane)),
                    }
                }
            }
        }
    }

    /// Close the target leaf. Takes `self` by value and returns:
    /// - `Ok((replacement_node, closed_pane))` on success
    /// - `Err(self)` if the target was not found in this subtree
    ///
    /// The caller must handle the single-leaf case before calling this
    /// (returns `CannotCloseLastPane`).
    fn close(self, target_id: PaneId) -> Result<(Self, Pane), Self> {
        match self {
            Self::Leaf(pane) if pane.id == target_id => {
                // The parent split should be replaced with the sibling.
                // But we don't know the sibling here — the parent handles it.
                // This case is actually handled by the Split arm checking children.
                // If we reach here, it means close was called on a bare leaf,
                // which should have been caught by the PaneTree wrapper.
                Err(Self::Leaf(pane))
            }
            leaf @ Self::Leaf(_) => Err(leaf),
            Self::Split {
                direction,
                ratio,
                first,
                second,
            } => {
                // Check if first is the target leaf
                if let Self::Leaf(ref pane) = *first
                    && pane.id == target_id
                {
                    let Self::Leaf(closed) = *first else {
                        // We just checked it's a Leaf
                        return Err(Self::Split {
                            direction,
                            ratio,
                            first,
                            second,
                        });
                    };
                    return Ok((*second, *closed));
                }

                // Check if second is the target leaf
                if let Self::Leaf(ref pane) = *second
                    && pane.id == target_id
                {
                    let Self::Leaf(closed) = *second else {
                        return Err(Self::Split {
                            direction,
                            ratio,
                            first,
                            second,
                        });
                    };
                    return Ok((*first, *closed));
                }

                // Target is deeper — recurse
                if first.contains(target_id) {
                    match first.close(target_id) {
                        Ok((new_first, closed)) => Ok((
                            Self::Split {
                                direction,
                                ratio,
                                first: Box::new(new_first),
                                second,
                            },
                            closed,
                        )),
                        Err(old_first) => Err(Self::Split {
                            direction,
                            ratio,
                            first: Box::new(old_first),
                            second,
                        }),
                    }
                } else {
                    match second.close(target_id) {
                        Ok((new_second, closed)) => Ok((
                            Self::Split {
                                direction,
                                ratio,
                                first,
                                second: Box::new(new_second),
                            },
                            closed,
                        )),
                        Err(old_second) => Err(Self::Split {
                            direction,
                            ratio,
                            first,
                            second: Box::new(old_second),
                        }),
                    }
                }
            }
        }
    }

    /// Adjust the split ratio of the nearest ancestor split matching
    /// `direction` above `target_id`. Mutates in place.
    ///
    /// Returns `true` if a matching split was found and resized.
    fn resize(&mut self, target_id: PaneId, direction: SplitDirection, delta: f32) -> bool {
        match self {
            Self::Leaf(_) => false,
            Self::Split {
                direction: split_dir,
                ratio,
                first,
                second,
            } => {
                let in_first = first.contains(target_id);
                let in_second = second.contains(target_id);

                if !in_first && !in_second {
                    return false;
                }

                // Try deeper splits first (closest ancestor wins).
                if in_first && first.resize(target_id, direction, delta) {
                    return true;
                }
                if in_second && second.resize(target_id, direction, delta) {
                    return true;
                }

                // No deeper match — try this split.
                if *split_dir == direction {
                    *ratio = (*ratio + delta).clamp(MIN_SPLIT_RATIO, MAX_SPLIT_RATIO);
                    return true;
                }

                false
            }
        }
    }

    /// Set the ratio of the split whose subtree contains `target_id` to an
    /// absolute value.  Mirror of `resize` but sets rather than adjusts.
    fn set_ratio(&mut self, target_id: PaneId, direction: SplitDirection, new_ratio: f32) -> bool {
        match self {
            Self::Leaf(_) => false,
            Self::Split {
                direction: split_dir,
                ratio,
                first,
                second,
            } => {
                let in_first = first.contains(target_id);
                let in_second = second.contains(target_id);

                if !in_first && !in_second {
                    return false;
                }

                // Try deeper splits first (closest ancestor wins).
                if in_first && first.set_ratio(target_id, direction, new_ratio) {
                    return true;
                }
                if in_second && second.set_ratio(target_id, direction, new_ratio) {
                    return true;
                }

                // No deeper match — try this split.
                if *split_dir == direction {
                    *ratio = new_ratio;
                    return true;
                }

                false
            }
        }
    }
}

// ── PaneTree (public wrapper) ────────────────────────────────────────

/// A binary tree of terminal panes.
///
/// A single-pane tab is represented as a tree with one leaf — functionally
/// identical to the current non-split `Tab`. Splits produce internal nodes.
/// The tree is always non-empty after construction: the last pane cannot be
/// closed (returns [`PaneError::CannotCloseLastPane`]).
///
/// The tree lives on the GUI thread and is never shared with PTY threads.
///
/// Internally uses `Option<PaneNode>` so that by-value tree transforms can
/// temporarily take the root. The `Option` is always `Some` outside of
/// method bodies.
pub struct PaneTree {
    /// The root of the pane tree. Always `Some` after construction.
    root: Option<PaneNode>,
}

impl std::fmt::Debug for PaneTree {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.root {
            Some(node) => f.debug_tuple("PaneTree").field(node).finish(),
            None => f.write_str("PaneTree(<invalid>)"),
        }
    }
}

impl PaneTree {
    /// Create a new tree containing a single pane.
    #[must_use]
    #[allow(clippy::missing_const_for_fn)] // Box::new is not const-stable
    pub fn new(pane: Pane) -> Self {
        Self {
            root: Some(PaneNode::Leaf(Box::new(pane))),
        }
    }

    /// Borrow the root node. Returns `Err` if the tree is in an invalid
    /// (empty) state, which should never happen in normal operation.
    fn root(&self) -> Result<&PaneNode, PaneError> {
        self.root.as_ref().ok_or(PaneError::InvalidState)
    }

    /// Borrow the root node mutably.
    fn root_mut(&mut self) -> Result<&mut PaneNode, PaneError> {
        self.root.as_mut().ok_or(PaneError::InvalidState)
    }

    /// Take the root node out for by-value transformation, leaving `None`
    /// temporarily.
    fn take_root(&mut self) -> Result<PaneNode, PaneError> {
        self.root.take().ok_or(PaneError::InvalidState)
    }

    // ── Queries ──────────────────────────────────────────────────────

    /// Return the number of leaf panes in the tree.
    ///
    /// # Errors
    ///
    /// Returns [`PaneError::InvalidState`] if the tree is empty (bug).
    pub fn pane_count(&self) -> Result<usize, PaneError> {
        Ok(self.root()?.pane_count())
    }

    /// Find a pane by id (immutable).
    #[must_use]
    pub fn find(&self, id: PaneId) -> Option<&Pane> {
        self.root.as_ref().and_then(|r| r.find(id))
    }

    /// Find a pane by id (mutable).
    #[must_use]
    pub fn find_mut(&mut self, id: PaneId) -> Option<&mut Pane> {
        self.root.as_mut().and_then(|r| r.find_mut(id))
    }

    /// Collect references to all leaf panes (depth-first order).
    ///
    /// # Errors
    ///
    /// Returns [`PaneError::InvalidState`] if the tree is empty (bug).
    pub fn iter_panes(&self) -> Result<Vec<&Pane>, PaneError> {
        let root = self.root()?;
        let mut panes = Vec::with_capacity(root.pane_count());
        root.collect_panes(&mut panes);
        Ok(panes)
    }

    /// Collect mutable references to all leaf panes (depth-first order).
    ///
    /// # Errors
    ///
    /// Returns [`PaneError::InvalidState`] if the tree is empty (bug).
    pub fn iter_panes_mut(&mut self) -> Result<Vec<&mut Pane>, PaneError> {
        let root = self.root_mut()?;
        let count = root.pane_count();
        let mut panes = Vec::with_capacity(count);
        root.collect_panes_mut(&mut panes);
        Ok(panes)
    }

    // ── Layout ───────────────────────────────────────────────────────

    /// Compute the layout rectangle for each leaf pane.
    ///
    /// Returns a `Vec` of `(PaneId, Rect)` pairs in depth-first order.
    /// The caller uses these rects to position each pane's terminal widget.
    ///
    /// # Errors
    ///
    /// Returns [`PaneError::InvalidState`] if the tree is empty (bug).
    pub fn layout(&self, rect: Rect) -> Result<Vec<(PaneId, Rect)>, PaneError> {
        let root = self.root()?;
        let mut result = Vec::with_capacity(root.pane_count());
        root.layout(rect, &mut result);
        Ok(result)
    }

    /// Compute the split borders for the current tree layout.
    ///
    /// Returns a [`SplitBorder`] for each internal split node, with the
    /// screen-space rect of the border line and enough info to drive
    /// [`PaneTree::resize_split`] on drag.
    ///
    /// `active_pane` is the currently focused pane; each returned
    /// [`SplitBorder`] carries an `active_in_first` field indicating
    /// which subtree the active pane belongs to, enabling tmux-style
    /// half-highlighted border rendering.
    ///
    /// Returns an empty `Vec` when the tree has only one pane.
    ///
    /// # Errors
    ///
    /// Returns [`PaneError::InvalidState`] if the tree is empty (bug).
    pub fn split_borders(
        &self,
        rect: Rect,
        active_pane: PaneId,
    ) -> Result<Vec<SplitBorder>, PaneError> {
        let root = self.root()?;
        let mut borders = Vec::new();
        root.split_borders(rect, active_pane, &mut borders);
        Ok(borders)
    }

    // ── Mutations ────────────────────────────────────────────────────

    /// Split the pane identified by `target_id`, creating a new pane.
    ///
    /// The `make_pane` closure receives the new pane's `PaneId` and must
    /// return a fully constructed `Pane` (with PTY channels etc.). The
    /// existing pane becomes the first child; the new pane becomes the
    /// second child.
    ///
    /// Returns the new pane's `PaneId` on success.
    ///
    /// # Errors
    ///
    /// - [`PaneError::NotFound`] if `target_id` does not exist.
    /// - [`PaneError::InvalidState`] if the tree is empty (bug).
    pub fn split<F>(
        &mut self,
        target_id: PaneId,
        direction: SplitDirection,
        id_gen: &mut PaneIdGenerator,
        make_pane: F,
    ) -> Result<PaneId, PaneError>
    where
        F: FnOnce(PaneId) -> Pane,
    {
        let root = self.take_root()?;

        // Verify target exists before allocating the new pane.
        if root.find(target_id).is_none() {
            self.root = Some(root);
            return Err(PaneError::NotFound(target_id));
        }

        let new_id = id_gen.next_id();
        let new_pane = make_pane(new_id);
        self.root = Some(root.split(target_id, direction, new_pane));
        Ok(new_id)
    }

    /// Split the pane identified by `target_id` using a pre-built `Pane`.
    ///
    /// This is a lower-level variant of [`split`] for callers that need to
    /// pre-allocate the `PaneId` before acquiring other borrows.  The caller is
    /// responsible for ensuring `new_pane.id` was obtained from the same
    /// [`PaneIdGenerator`] that manages this tree.
    ///
    /// # Errors
    ///
    /// - [`PaneError::NotFound`] if `target_id` does not exist.
    /// - [`PaneError::InvalidState`] if the tree is empty (bug).
    pub fn split_with_id(
        &mut self,
        target_id: PaneId,
        direction: SplitDirection,
        new_pane: Pane,
    ) -> Result<PaneId, PaneError> {
        let root = self.take_root()?;

        if root.find(target_id).is_none() {
            self.root = Some(root);
            return Err(PaneError::NotFound(target_id));
        }

        let new_id = new_pane.id;
        self.root = Some(root.split(target_id, direction, new_pane));
        Ok(new_id)
    }

    /// Close the pane identified by `target_id`, collapsing its parent split.
    ///
    /// # Errors
    ///
    /// - [`PaneError::CannotCloseLastPane`] if this is the only pane.
    /// - [`PaneError::NotFound`] if `target_id` does not exist.
    /// - [`PaneError::InvalidState`] if the tree is empty (bug).
    pub fn close(&mut self, target_id: PaneId) -> Result<ClosedPaneResult, PaneError> {
        let root = self.take_root()?;

        // Single leaf: cannot close
        if let PaneNode::Leaf(ref pane) = root {
            if pane.id == target_id {
                self.root = Some(root);
                return Err(PaneError::CannotCloseLastPane);
            }
            self.root = Some(root);
            return Err(PaneError::NotFound(target_id));
        }

        // Verify target exists
        if root.find(target_id).is_none() {
            self.root = Some(root);
            return Err(PaneError::NotFound(target_id));
        }

        match root.close(target_id) {
            Ok((new_root, closed_pane)) => {
                self.root = Some(new_root);
                Ok(ClosedPaneResult { closed_pane })
            }
            Err(old_root) => {
                // Should not happen since we verified the target exists
                self.root = Some(old_root);
                Err(PaneError::NotFound(target_id))
            }
        }
    }

    /// Adjust the split ratio of the nearest ancestor split matching
    /// `direction` above the pane identified by `target_id`.
    ///
    /// `delta` is added to the current ratio and clamped to
    /// `0.1..=0.9`.
    ///
    /// # Errors
    ///
    /// - [`PaneError::NotFound`] if `target_id` does not exist.
    /// - [`PaneError::NoSplitInDirection`] if no ancestor split matches.
    /// - [`PaneError::InvalidState`] if the tree is empty (bug).
    pub fn resize_split(
        &mut self,
        target_id: PaneId,
        direction: SplitDirection,
        delta: f32,
    ) -> Result<(), PaneError> {
        let root = self.root_mut()?;

        if root.find(target_id).is_none() {
            return Err(PaneError::NotFound(target_id));
        }

        if root.resize(target_id, direction, delta) {
            Ok(())
        } else {
            Err(PaneError::NoSplitInDirection {
                pane: target_id,
                direction,
            })
        }
    }

    /// Set the split ratio for the split whose `first` subtree contains
    /// `target_id`, for a split of the given `direction`.
    ///
    /// The ratio is clamped to `[MIN_SPLIT_RATIO, MAX_SPLIT_RATIO]`.
    ///
    /// # Errors
    ///
    /// - [`PaneError::NotFound`] if `target_id` does not exist.
    /// - [`PaneError::InvalidState`] if the tree is empty (bug).
    /// - [`PaneError::NoSplitInDirection`] if no matching split is found.
    pub fn set_split_ratio(
        &mut self,
        target_id: PaneId,
        direction: SplitDirection,
        ratio: f32,
    ) -> Result<(), PaneError> {
        let root = self.root_mut()?;

        if root.find(target_id).is_none() {
            return Err(PaneError::NotFound(target_id));
        }

        let clamped = ratio.clamp(MIN_SPLIT_RATIO, MAX_SPLIT_RATIO);
        if root.set_ratio(target_id, direction, clamped) {
            Ok(())
        } else {
            Err(PaneError::NoSplitInDirection {
                pane: target_id,
                direction,
            })
        }
    }
}

// ── Layout helpers ───────────────────────────────────────────────────

/// Split a rectangle along the given direction at the given ratio.
fn split_rect(rect: Rect, direction: SplitDirection, ratio: f32) -> (Rect, Rect) {
    match direction {
        SplitDirection::Horizontal => {
            let split_x = rect.width().mul_add(ratio, rect.min.x).round();
            let left = Rect::from_min_max(rect.min, egui::pos2(split_x, rect.max.y));
            let right = Rect::from_min_max(egui::pos2(split_x, rect.min.y), rect.max);
            (left, right)
        }
        SplitDirection::Vertical => {
            let split_y = rect.height().mul_add(ratio, rect.min.y).round();
            let top = Rect::from_min_max(rect.min, egui::pos2(rect.max.x, split_y));
            let bottom = Rect::from_min_max(egui::pos2(rect.min.x, split_y), rect.max);
            (top, bottom)
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    // ── PaneId tests ─────────────────────────────────────────────────

    #[test]
    fn pane_id_first_is_zero() {
        let id = PaneId::first();
        assert_eq!(id, PaneId(0));
    }

    #[test]
    fn pane_id_equality() {
        assert_eq!(PaneId(42), PaneId(42));
        assert_ne!(PaneId(1), PaneId(2));
    }

    #[test]
    fn pane_id_display() {
        let id = PaneId(7);
        assert_eq!(format!("{id}"), "Pane(7)");
    }

    #[test]
    fn pane_id_hash_works() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(PaneId(0));
        set.insert(PaneId(1));
        set.insert(PaneId(0)); // duplicate
        assert_eq!(set.len(), 2);
    }

    // ── PaneIdGenerator tests ────────────────────────────────────────

    #[test]
    fn generator_default_starts_at_zero() {
        let mut id_gen = PaneIdGenerator::default();
        assert_eq!(id_gen.next_id(), PaneId(0));
        assert_eq!(id_gen.next_id(), PaneId(1));
        assert_eq!(id_gen.next_id(), PaneId(2));
    }

    #[test]
    fn generator_custom_start() {
        let mut id_gen = PaneIdGenerator::new(100);
        assert_eq!(id_gen.next_id(), PaneId(100));
        assert_eq!(id_gen.next_id(), PaneId(101));
    }

    #[test]
    fn generator_ids_are_unique() {
        let mut id_gen = PaneIdGenerator::default();
        let ids: Vec<PaneId> = (0..50).map(|_| id_gen.next_id()).collect();
        let unique: std::collections::HashSet<PaneId> = ids.iter().copied().collect();
        assert_eq!(ids.len(), unique.len(), "all generated ids must be unique");
    }

    // ── Pane struct tests ────────────────────────────────────────────

    /// Create a dummy `Pane` for testing.
    ///
    /// Uses disconnected channels that will fail on send/recv, which is fine
    /// for testing the pane data model.
    fn dummy_pane(id: PaneId, title: &str) -> Pane {
        let arc_swap = Arc::new(ArcSwap::from_pointee(TerminalSnapshot::empty()));
        let (input_tx, _input_rx) = crossbeam_channel::unbounded();
        let (pty_write_tx, _pty_write_rx) = crossbeam_channel::unbounded();
        let (_window_cmd_tx, window_cmd_rx) = crossbeam_channel::unbounded();
        let (_clipboard_tx, clipboard_rx) = crossbeam_channel::bounded(1);
        let (_search_buffer_tx, search_buffer_rx) =
            crossbeam_channel::bounded::<(usize, Vec<TChar>)>(1);
        let (_pty_dead_tx, pty_dead_rx) = crossbeam_channel::bounded(1);

        Pane {
            id,
            arc_swap,
            input_tx,
            pty_write_tx,
            window_cmd_rx,
            clipboard_rx,
            search_buffer_rx,
            pty_dead_rx,
            title: title.to_owned(),
            bell_active: false,
            title_stack: Vec::new(),
            view_state: ViewState::new(),
            echo_off: Arc::new(AtomicBool::new(false)),
            render_state: crate::gui::terminal::new_render_state(Arc::new(std::sync::Mutex::new(
                crate::gui::renderer::WindowPostRenderer::new(),
            ))),
            render_cache: crate::gui::terminal::PaneRenderCache::new(),
        }
    }

    /// Create a dummy pane from a `PaneId` (for use as a `make_pane` closure).
    fn make_dummy(id: PaneId) -> Pane {
        dummy_pane(id, &format!("pane-{id}"))
    }

    #[test]
    fn pane_debug_includes_id_and_title() {
        let pane = dummy_pane(PaneId(42), "test pane");
        let debug = format!("{pane:?}");
        assert!(debug.contains("42"));
        assert!(debug.contains("test pane"));
    }

    #[test]
    fn pane_fields_are_accessible() {
        let pane = dummy_pane(PaneId(5), "my pane");
        assert_eq!(pane.id, PaneId(5));
        assert_eq!(pane.title, "my pane");
        assert!(!pane.bell_active);
        assert!(pane.title_stack.is_empty());
        assert_eq!(pane.view_state.scroll_offset, 0);
        assert!(!pane.echo_off.load(std::sync::atomic::Ordering::Relaxed));
    }

    #[test]
    fn pane_view_state_is_independent() {
        let mut pane1 = dummy_pane(PaneId(0), "pane 1");
        let pane2 = dummy_pane(PaneId(1), "pane 2");

        pane1.view_state.scroll_offset = 42;
        assert_eq!(pane1.view_state.scroll_offset, 42);
        assert_eq!(pane2.view_state.scroll_offset, 0);
    }

    // ── PaneTree: single pane ────────────────────────────────────────

    #[test]
    fn tree_single_pane_count() {
        let tree = PaneTree::new(dummy_pane(PaneId(0), "root"));
        assert_eq!(tree.pane_count().unwrap(), 1);
    }

    #[test]
    fn tree_single_find() {
        let tree = PaneTree::new(dummy_pane(PaneId(0), "root"));
        assert!(tree.find(PaneId(0)).is_some());
        assert!(tree.find(PaneId(1)).is_none());
    }

    #[test]
    fn tree_single_find_mut() {
        let mut tree = PaneTree::new(dummy_pane(PaneId(0), "root"));
        let pane = tree.find_mut(PaneId(0)).unwrap();
        pane.title = "modified".to_owned();
        assert_eq!(tree.find(PaneId(0)).unwrap().title, "modified");
    }

    #[test]
    fn tree_single_layout() {
        let tree = PaneTree::new(dummy_pane(PaneId(0), "root"));
        let rect = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(800.0, 600.0));
        let layout = tree.layout(rect).unwrap();
        assert_eq!(layout.len(), 1);
        assert_eq!(layout[0].0, PaneId(0));
        assert_eq!(layout[0].1, rect);
    }

    #[test]
    fn tree_single_iter_panes() {
        let tree = PaneTree::new(dummy_pane(PaneId(0), "root"));
        let panes = tree.iter_panes().unwrap();
        assert_eq!(panes.len(), 1);
        assert_eq!(panes[0].id, PaneId(0));
    }

    #[test]
    fn tree_single_cannot_close() {
        let mut tree = PaneTree::new(dummy_pane(PaneId(0), "root"));
        let err = tree.close(PaneId(0)).unwrap_err();
        assert!(matches!(err, PaneError::CannotCloseLastPane));
    }

    #[test]
    fn tree_single_close_not_found() {
        let mut tree = PaneTree::new(dummy_pane(PaneId(0), "root"));
        let err = tree.close(PaneId(99)).unwrap_err();
        assert!(matches!(err, PaneError::NotFound(_)));
    }

    // ── PaneTree: split ──────────────────────────────────────────────

    #[test]
    fn tree_split_horizontal() {
        let mut tree = PaneTree::new(dummy_pane(PaneId(0), "root"));
        let mut id_gen = PaneIdGenerator::new(1);

        let new_id = tree
            .split(
                PaneId(0),
                SplitDirection::Horizontal,
                &mut id_gen,
                make_dummy,
            )
            .unwrap();
        assert_eq!(new_id, PaneId(1));
        assert_eq!(tree.pane_count().unwrap(), 2);
        assert!(tree.find(PaneId(0)).is_some());
        assert!(tree.find(PaneId(1)).is_some());
    }

    #[test]
    fn tree_split_vertical() {
        let mut tree = PaneTree::new(dummy_pane(PaneId(0), "root"));
        let mut id_gen = PaneIdGenerator::new(1);

        let new_id = tree
            .split(PaneId(0), SplitDirection::Vertical, &mut id_gen, make_dummy)
            .unwrap();
        assert_eq!(new_id, PaneId(1));
        assert_eq!(tree.pane_count().unwrap(), 2);
    }

    #[test]
    fn tree_split_not_found() {
        let mut tree = PaneTree::new(dummy_pane(PaneId(0), "root"));
        let mut id_gen = PaneIdGenerator::new(1);

        let err = tree
            .split(
                PaneId(99),
                SplitDirection::Horizontal,
                &mut id_gen,
                make_dummy,
            )
            .unwrap_err();
        assert!(matches!(err, PaneError::NotFound(_)));
        // Tree unchanged
        assert_eq!(tree.pane_count().unwrap(), 1);
    }

    #[test]
    fn tree_split_layout_horizontal() {
        let mut tree = PaneTree::new(dummy_pane(PaneId(0), "root"));
        let mut id_gen = PaneIdGenerator::new(1);
        tree.split(
            PaneId(0),
            SplitDirection::Horizontal,
            &mut id_gen,
            make_dummy,
        )
        .unwrap();

        let rect = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(800.0, 600.0));
        let layout = tree.layout(rect).unwrap();
        assert_eq!(layout.len(), 2);

        // First pane gets left half
        assert_eq!(layout[0].0, PaneId(0));
        let left = layout[0].1;
        assert!((left.width() - 400.0).abs() < 0.01);
        assert!((left.height() - 600.0).abs() < 0.01);

        // Second pane gets right half
        assert_eq!(layout[1].0, PaneId(1));
        let right = layout[1].1;
        assert!((right.width() - 400.0).abs() < 0.01);
        assert!((right.min.x - 400.0).abs() < 0.01);
    }

    #[test]
    fn tree_split_layout_vertical() {
        let mut tree = PaneTree::new(dummy_pane(PaneId(0), "root"));
        let mut id_gen = PaneIdGenerator::new(1);
        tree.split(PaneId(0), SplitDirection::Vertical, &mut id_gen, make_dummy)
            .unwrap();

        let rect = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(800.0, 600.0));
        let layout = tree.layout(rect).unwrap();
        assert_eq!(layout.len(), 2);

        // First pane gets top half
        let top = layout[0].1;
        assert!((top.height() - 300.0).abs() < 0.01);

        // Second pane gets bottom half
        let bottom = layout[1].1;
        assert!((bottom.height() - 300.0).abs() < 0.01);
        assert!((bottom.min.y - 300.0).abs() < 0.01);
    }

    // ── PaneTree: nested splits ──────────────────────────────────────

    #[test]
    fn tree_nested_split() {
        // Start with one pane, split horizontally, then split the left pane vertically
        let mut tree = PaneTree::new(dummy_pane(PaneId(0), "root"));
        let mut id_gen = PaneIdGenerator::new(1);

        // Split root horizontally: [0 | 1]
        tree.split(
            PaneId(0),
            SplitDirection::Horizontal,
            &mut id_gen,
            make_dummy,
        )
        .unwrap();

        // Split pane 0 vertically: [0/2 | 1]
        tree.split(PaneId(0), SplitDirection::Vertical, &mut id_gen, make_dummy)
            .unwrap();

        assert_eq!(tree.pane_count().unwrap(), 3);
        assert!(tree.find(PaneId(0)).is_some());
        assert!(tree.find(PaneId(1)).is_some());
        assert!(tree.find(PaneId(2)).is_some());
    }

    #[test]
    fn tree_nested_layout() {
        let mut tree = PaneTree::new(dummy_pane(PaneId(0), "root"));
        let mut id_gen = PaneIdGenerator::new(1);

        // [0 | 1] horizontally
        tree.split(
            PaneId(0),
            SplitDirection::Horizontal,
            &mut id_gen,
            make_dummy,
        )
        .unwrap();
        // [0/2 | 1] — split left pane vertically
        tree.split(PaneId(0), SplitDirection::Vertical, &mut id_gen, make_dummy)
            .unwrap();

        let rect = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(800.0, 600.0));
        let layout = tree.layout(rect).unwrap();
        assert_eq!(layout.len(), 3);

        // Pane 0: top-left quarter
        assert_eq!(layout[0].0, PaneId(0));
        assert!((layout[0].1.width() - 400.0).abs() < 0.01);
        assert!((layout[0].1.height() - 300.0).abs() < 0.01);

        // Pane 2: bottom-left quarter
        assert_eq!(layout[1].0, PaneId(2));
        assert!((layout[1].1.min.y - 300.0).abs() < 0.01);

        // Pane 1: right half
        assert_eq!(layout[2].0, PaneId(1));
        assert!((layout[2].1.width() - 400.0).abs() < 0.01);
        assert!((layout[2].1.height() - 600.0).abs() < 0.01);
    }

    #[test]
    fn tree_deep_nesting() {
        let mut tree = PaneTree::new(dummy_pane(PaneId(0), "root"));
        let mut id_gen = PaneIdGenerator::new(1);

        // Chain of horizontal splits: split pane 0 four times
        for _ in 0..4 {
            let last_id = PaneId(id_gen.next - 1);
            // Split the newest pane each time
            let target = if id_gen.next == 1 { PaneId(0) } else { last_id };
            tree.split(target, SplitDirection::Horizontal, &mut id_gen, make_dummy)
                .unwrap();
        }

        assert_eq!(tree.pane_count().unwrap(), 5);
        for i in 0..5 {
            assert!(tree.find(PaneId(i)).is_some(), "pane {i} should exist");
        }
    }

    // ── PaneTree: close ──────────────────────────────────────────────

    #[test]
    fn tree_close_second_pane() {
        let mut tree = PaneTree::new(dummy_pane(PaneId(0), "root"));
        let mut id_gen = PaneIdGenerator::new(1);

        tree.split(
            PaneId(0),
            SplitDirection::Horizontal,
            &mut id_gen,
            make_dummy,
        )
        .unwrap();
        assert_eq!(tree.pane_count().unwrap(), 2);

        let result = tree.close(PaneId(1)).unwrap();
        assert_eq!(result.closed_pane.id, PaneId(1));
        assert_eq!(tree.pane_count().unwrap(), 1);
        assert!(tree.find(PaneId(0)).is_some());
        assert!(tree.find(PaneId(1)).is_none());
    }

    #[test]
    fn tree_close_first_pane() {
        let mut tree = PaneTree::new(dummy_pane(PaneId(0), "root"));
        let mut id_gen = PaneIdGenerator::new(1);

        tree.split(
            PaneId(0),
            SplitDirection::Horizontal,
            &mut id_gen,
            make_dummy,
        )
        .unwrap();

        let result = tree.close(PaneId(0)).unwrap();
        assert_eq!(result.closed_pane.id, PaneId(0));
        assert_eq!(tree.pane_count().unwrap(), 1);
        assert!(tree.find(PaneId(1)).is_some());
    }

    #[test]
    fn tree_close_in_nested_tree() {
        let mut tree = PaneTree::new(dummy_pane(PaneId(0), "root"));
        let mut id_gen = PaneIdGenerator::new(1);

        // [0 | 1]
        tree.split(
            PaneId(0),
            SplitDirection::Horizontal,
            &mut id_gen,
            make_dummy,
        )
        .unwrap();
        // [0/2 | 1]
        tree.split(PaneId(0), SplitDirection::Vertical, &mut id_gen, make_dummy)
            .unwrap();
        assert_eq!(tree.pane_count().unwrap(), 3);

        // Close pane 0 — should collapse the vertical split, leaving [2 | 1]
        let result = tree.close(PaneId(0)).unwrap();
        assert_eq!(result.closed_pane.id, PaneId(0));
        assert_eq!(tree.pane_count().unwrap(), 2);
        assert!(tree.find(PaneId(1)).is_some());
        assert!(tree.find(PaneId(2)).is_some());
    }

    #[test]
    fn tree_close_deep_nested() {
        let mut tree = PaneTree::new(dummy_pane(PaneId(0), "root"));
        let mut id_gen = PaneIdGenerator::new(1);

        // Build: [0 | 1], then [0 | 1/2], then [0 | 1/2/3]
        tree.split(
            PaneId(0),
            SplitDirection::Horizontal,
            &mut id_gen,
            make_dummy,
        )
        .unwrap();
        tree.split(PaneId(1), SplitDirection::Vertical, &mut id_gen, make_dummy)
            .unwrap();
        tree.split(
            PaneId(2),
            SplitDirection::Horizontal,
            &mut id_gen,
            make_dummy,
        )
        .unwrap();
        assert_eq!(tree.pane_count().unwrap(), 4);

        // Close pane 2 (deeply nested)
        let result = tree.close(PaneId(2)).unwrap();
        assert_eq!(result.closed_pane.id, PaneId(2));
        assert_eq!(tree.pane_count().unwrap(), 3);
        // Remaining: 0, 1, 3
        assert!(tree.find(PaneId(0)).is_some());
        assert!(tree.find(PaneId(1)).is_some());
        assert!(tree.find(PaneId(3)).is_some());
    }

    #[test]
    fn tree_close_not_found() {
        let mut tree = PaneTree::new(dummy_pane(PaneId(0), "root"));
        let mut id_gen = PaneIdGenerator::new(1);
        tree.split(
            PaneId(0),
            SplitDirection::Horizontal,
            &mut id_gen,
            make_dummy,
        )
        .unwrap();

        let err = tree.close(PaneId(99)).unwrap_err();
        assert!(matches!(err, PaneError::NotFound(_)));
    }

    // ── PaneTree: resize ─────────────────────────────────────────────

    #[test]
    fn tree_resize_horizontal_split() {
        let mut tree = PaneTree::new(dummy_pane(PaneId(0), "root"));
        let mut id_gen = PaneIdGenerator::new(1);
        tree.split(
            PaneId(0),
            SplitDirection::Horizontal,
            &mut id_gen,
            make_dummy,
        )
        .unwrap();

        // Resize: increase first pane's share
        tree.resize_split(PaneId(0), SplitDirection::Horizontal, 0.1)
            .unwrap();

        let rect = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(800.0, 600.0));
        let layout = tree.layout(rect).unwrap();
        // First pane should now be 60% wide (0.5 + 0.1 = 0.6)
        let first_width = layout[0].1.width();
        assert!(
            (first_width - 480.0).abs() < 0.01,
            "expected 480, got {first_width}"
        );
    }

    #[test]
    fn tree_resize_clamped_min() {
        let mut tree = PaneTree::new(dummy_pane(PaneId(0), "root"));
        let mut id_gen = PaneIdGenerator::new(1);
        tree.split(
            PaneId(0),
            SplitDirection::Horizontal,
            &mut id_gen,
            make_dummy,
        )
        .unwrap();

        // Try to shrink way below minimum
        tree.resize_split(PaneId(0), SplitDirection::Horizontal, -0.9)
            .unwrap();

        let rect = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1000.0, 600.0));
        let layout = tree.layout(rect).unwrap();
        // Clamped to MIN_SPLIT_RATIO (0.1)
        let first_width = layout[0].1.width();
        assert!(
            (first_width - 100.0).abs() < 0.01,
            "expected 100 (10%), got {first_width}"
        );
    }

    #[test]
    fn tree_resize_clamped_max() {
        let mut tree = PaneTree::new(dummy_pane(PaneId(0), "root"));
        let mut id_gen = PaneIdGenerator::new(1);
        tree.split(
            PaneId(0),
            SplitDirection::Horizontal,
            &mut id_gen,
            make_dummy,
        )
        .unwrap();

        // Try to grow way above maximum
        tree.resize_split(PaneId(0), SplitDirection::Horizontal, 0.9)
            .unwrap();

        let rect = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1000.0, 600.0));
        let layout = tree.layout(rect).unwrap();
        // Clamped to MAX_SPLIT_RATIO (0.9)
        let first_width = layout[0].1.width();
        assert!(
            (first_width - 900.0).abs() < 0.01,
            "expected 900 (90%), got {first_width}"
        );
    }

    #[test]
    fn tree_resize_wrong_direction() {
        let mut tree = PaneTree::new(dummy_pane(PaneId(0), "root"));
        let mut id_gen = PaneIdGenerator::new(1);
        // Split horizontally
        tree.split(
            PaneId(0),
            SplitDirection::Horizontal,
            &mut id_gen,
            make_dummy,
        )
        .unwrap();

        // Try to resize in vertical direction — no matching split
        let err = tree
            .resize_split(PaneId(0), SplitDirection::Vertical, 0.1)
            .unwrap_err();
        assert!(matches!(err, PaneError::NoSplitInDirection { .. }));
    }

    #[test]
    fn tree_resize_not_found() {
        let mut tree = PaneTree::new(dummy_pane(PaneId(0), "root"));
        let err = tree
            .resize_split(PaneId(99), SplitDirection::Horizontal, 0.1)
            .unwrap_err();
        assert!(matches!(err, PaneError::NotFound(_)));
    }

    #[test]
    fn tree_resize_nearest_ancestor() {
        // Outer horizontal split, inner vertical split.
        // Resizing in horizontal direction from an inner pane should hit the outer split.
        let mut tree = PaneTree::new(dummy_pane(PaneId(0), "root"));
        let mut id_gen = PaneIdGenerator::new(1);

        // [0 | 1] horizontal
        tree.split(
            PaneId(0),
            SplitDirection::Horizontal,
            &mut id_gen,
            make_dummy,
        )
        .unwrap();
        // [0/2 | 1] vertical split inside left
        tree.split(PaneId(0), SplitDirection::Vertical, &mut id_gen, make_dummy)
            .unwrap();

        // Resize pane 2 in horizontal direction — should find the outer horizontal split
        tree.resize_split(PaneId(2), SplitDirection::Horizontal, 0.1)
            .unwrap();

        let rect = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1000.0, 600.0));
        let layout = tree.layout(rect).unwrap();

        // The outer horizontal split should now be at 0.6 ratio
        // Pane 0 and 2 share the left side, which should be 600px wide
        // Pane 0 is in the top-left
        let pane0_width = layout[0].1.width();
        assert!(
            (pane0_width - 600.0).abs() < 0.01,
            "expected ~600, got {pane0_width}"
        );
    }

    // ── PaneTree: iter_panes_mut ─────────────────────────────────────

    #[test]
    fn tree_iter_panes_mut_modifies_all() {
        let mut tree = PaneTree::new(dummy_pane(PaneId(0), "root"));
        let mut id_gen = PaneIdGenerator::new(1);
        tree.split(
            PaneId(0),
            SplitDirection::Horizontal,
            &mut id_gen,
            make_dummy,
        )
        .unwrap();

        for pane in tree.iter_panes_mut().unwrap() {
            pane.bell_active = true;
        }

        for pane in tree.iter_panes().unwrap() {
            assert!(pane.bell_active, "pane {} should have bell_active", pane.id);
        }
    }

    // ── PaneTree: debug ──────────────────────────────────────────────

    #[test]
    fn tree_debug_single() {
        let tree = PaneTree::new(dummy_pane(PaneId(0), "root"));
        let debug = format!("{tree:?}");
        assert!(debug.contains("PaneTree"));
        assert!(debug.contains("Leaf"));
    }

    #[test]
    fn tree_debug_split() {
        let mut tree = PaneTree::new(dummy_pane(PaneId(0), "root"));
        let mut id_gen = PaneIdGenerator::new(1);
        tree.split(
            PaneId(0),
            SplitDirection::Horizontal,
            &mut id_gen,
            make_dummy,
        )
        .unwrap();
        let debug = format!("{tree:?}");
        assert!(debug.contains("Split"));
        assert!(debug.contains("Horizontal"));
    }

    // ── PaneTree: unbalanced tree ────────────────────────────────────

    #[test]
    fn tree_unbalanced_right_chain() {
        // All splits go to the right: root -> [0 | [1 | [2 | 3]]]
        let mut tree = PaneTree::new(dummy_pane(PaneId(0), "root"));
        let mut id_gen = PaneIdGenerator::new(1);

        tree.split(
            PaneId(0),
            SplitDirection::Horizontal,
            &mut id_gen,
            make_dummy,
        )
        .unwrap();
        tree.split(
            PaneId(1),
            SplitDirection::Horizontal,
            &mut id_gen,
            make_dummy,
        )
        .unwrap();
        tree.split(
            PaneId(2),
            SplitDirection::Horizontal,
            &mut id_gen,
            make_dummy,
        )
        .unwrap();

        assert_eq!(tree.pane_count().unwrap(), 4);

        // Close middle pane
        tree.close(PaneId(2)).unwrap();
        assert_eq!(tree.pane_count().unwrap(), 3);
        assert!(tree.find(PaneId(0)).is_some());
        assert!(tree.find(PaneId(1)).is_some());
        assert!(tree.find(PaneId(3)).is_some());
    }

    #[test]
    fn tree_split_then_close_all_but_one() {
        let mut tree = PaneTree::new(dummy_pane(PaneId(0), "root"));
        let mut id_gen = PaneIdGenerator::new(1);

        // Create 4 panes
        tree.split(
            PaneId(0),
            SplitDirection::Horizontal,
            &mut id_gen,
            make_dummy,
        )
        .unwrap();
        tree.split(PaneId(0), SplitDirection::Vertical, &mut id_gen, make_dummy)
            .unwrap();
        tree.split(PaneId(1), SplitDirection::Vertical, &mut id_gen, make_dummy)
            .unwrap();
        assert_eq!(tree.pane_count().unwrap(), 4);

        // Close all but pane 0
        tree.close(PaneId(1)).unwrap();
        tree.close(PaneId(2)).unwrap();
        tree.close(PaneId(3)).unwrap();
        assert_eq!(tree.pane_count().unwrap(), 1);
        assert!(tree.find(PaneId(0)).is_some());

        // Verify can't close last one
        assert!(matches!(
            tree.close(PaneId(0)).unwrap_err(),
            PaneError::CannotCloseLastPane
        ));
    }

    // ── split_borders ────────────────────────────────────────────────

    #[test]
    fn split_borders_single_pane_returns_empty() {
        let tree = PaneTree::new(dummy_pane(PaneId(0), "root"));
        let rect = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(800.0, 600.0));
        let borders = tree.split_borders(rect, PaneId(0)).unwrap();
        assert!(borders.is_empty());
    }

    #[test]
    fn split_borders_single_horizontal_split() {
        let mut tree = PaneTree::new(dummy_pane(PaneId(0), "root"));
        let mut id_gen = PaneIdGenerator::new(1);

        tree.split(
            PaneId(0),
            SplitDirection::Horizontal,
            &mut id_gen,
            make_dummy,
        )
        .unwrap();

        let rect = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(800.0, 600.0));

        // Active pane in first child (left).
        let borders = tree.split_borders(rect, PaneId(0)).unwrap();
        assert_eq!(borders.len(), 1);
        let border = &borders[0];
        assert_eq!(border.direction, SplitDirection::Horizontal);
        assert_eq!(border.first_child_pane, PaneId(0));
        assert_eq!(border.active_in_first, Some(true));

        // The split should be at x=400 (50% of 800) ± 0.5
        let center_x = border.rect.center().x;
        assert!((center_x - 400.0).abs() < 1.0, "center_x = {center_x}");
        // Vertical extent should span full height
        assert!((border.rect.min.y - 0.0).abs() < 0.01);
        assert!((border.rect.max.y - 600.0).abs() < 0.01);

        // Active pane in second child (right).
        let borders = tree.split_borders(rect, PaneId(1)).unwrap();
        assert_eq!(borders[0].active_in_first, Some(false));

        // Active pane not in either subtree.
        let borders = tree.split_borders(rect, PaneId(99)).unwrap();
        assert_eq!(borders[0].active_in_first, None);
    }

    #[test]
    fn split_borders_single_vertical_split() {
        let mut tree = PaneTree::new(dummy_pane(PaneId(0), "root"));
        let mut id_gen = PaneIdGenerator::new(1);

        tree.split(PaneId(0), SplitDirection::Vertical, &mut id_gen, make_dummy)
            .unwrap();

        let rect = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(800.0, 600.0));
        let borders = tree.split_borders(rect, PaneId(0)).unwrap();

        assert_eq!(borders.len(), 1);
        let border = &borders[0];
        assert_eq!(border.direction, SplitDirection::Vertical);
        assert_eq!(border.active_in_first, Some(true));

        // The split should be at y=300 (50% of 600) ± 0.5
        let center_y = border.rect.center().y;
        assert!((center_y - 300.0).abs() < 1.0, "center_y = {center_y}");
        // Horizontal extent should span full width
        assert!((border.rect.min.x - 0.0).abs() < 0.01);
        assert!((border.rect.max.x - 800.0).abs() < 0.01);
    }

    #[test]
    fn split_borders_nested_tree_returns_all_borders() {
        let mut tree = PaneTree::new(dummy_pane(PaneId(0), "root"));
        let mut id_gen = PaneIdGenerator::new(1);

        // [0 | 1]
        tree.split(
            PaneId(0),
            SplitDirection::Horizontal,
            &mut id_gen,
            make_dummy,
        )
        .unwrap();
        // [0 / 2 | 1]  (vertical split within left pane)
        tree.split(PaneId(0), SplitDirection::Vertical, &mut id_gen, make_dummy)
            .unwrap();

        let rect = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(800.0, 600.0));

        // Active pane 0: in first child of outer H-split AND first child of inner V-split.
        let borders = tree.split_borders(rect, PaneId(0)).unwrap();
        assert_eq!(borders.len(), 2);

        // One horizontal, one vertical
        let h_count = borders
            .iter()
            .filter(|b| b.direction == SplitDirection::Horizontal)
            .count();
        let v_count = borders
            .iter()
            .filter(|b| b.direction == SplitDirection::Vertical)
            .count();
        assert_eq!(h_count, 1);
        assert_eq!(v_count, 1);

        // The outer horizontal border: active pane 0 is in the first (left) subtree.
        let h_border = borders
            .iter()
            .find(|b| b.direction == SplitDirection::Horizontal)
            .unwrap();
        assert_eq!(h_border.active_in_first, Some(true));

        // The inner vertical border: active pane 0 is in the first (top) subtree.
        let v_border = borders
            .iter()
            .find(|b| b.direction == SplitDirection::Vertical)
            .unwrap();
        assert_eq!(v_border.active_in_first, Some(true));

        // Active pane 1: in second child of outer H-split, not in inner V-split.
        let borders = tree.split_borders(rect, PaneId(1)).unwrap();
        let h_border = borders
            .iter()
            .find(|b| b.direction == SplitDirection::Horizontal)
            .unwrap();
        assert_eq!(h_border.active_in_first, Some(false));
        let v_border = borders
            .iter()
            .find(|b| b.direction == SplitDirection::Vertical)
            .unwrap();
        assert_eq!(v_border.active_in_first, None);

        // Active pane 2: in first child of outer H-split, second child of inner V-split.
        let borders = tree.split_borders(rect, PaneId(2)).unwrap();
        let h_border = borders
            .iter()
            .find(|b| b.direction == SplitDirection::Horizontal)
            .unwrap();
        assert_eq!(h_border.active_in_first, Some(true));
        let v_border = borders
            .iter()
            .find(|b| b.direction == SplitDirection::Vertical)
            .unwrap();
        assert_eq!(v_border.active_in_first, Some(false));
    }
}
