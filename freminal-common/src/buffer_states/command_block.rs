// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Command-block storage types backing OSC 133 (FinalTerm/FTCS) shell
//! integration.
//!
//! A `CommandBlock` represents one user command's full lifecycle as derived
//! from `OSC 133 A` (prompt start), `B` (command input start), `C` (output
//! start), and `D` (command finished with optional exit code).  Blocks are
//! stored on the buffer alongside `prompt_rows` and surfaced through the
//! terminal snapshot.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

/// Process-global monotonic counter for allocating `CommandBlockId` values.
static NEXT_BLOCK_ID: AtomicU64 = AtomicU64::new(1);

/// Stable identifier for a `CommandBlock`, monotonically increasing for the life of the process.
///
/// Used by GUI view state to remember per-block UI state (fold/collapse, selection highlight)
/// across re-renders, and to correlate notification events with rendered blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CommandBlockId(pub u64);

impl CommandBlockId {
    /// Allocate the next monotonic id.  Process-global counter.
    #[must_use]
    pub fn next() -> Self {
        Self(NEXT_BLOCK_ID.fetch_add(1, Ordering::Relaxed))
    }
}

impl fmt::Display for CommandBlockId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.0)
    }
}

/// Lifecycle/status of a `CommandBlock`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandStatus {
    /// Command has started (OSC 133 A received) but no `D` yet.
    Running,
    /// Finished with exit code 0.
    Success,
    /// Finished with a non-zero exit code (carries the code).
    Failure(i32),
    /// Finished but no exit code was provided in the `D` marker.
    Unknown,
}

impl fmt::Display for CommandStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Running => write!(f, "Running"),
            Self::Success => write!(f, "Success"),
            Self::Failure(code) => write!(f, "Failure({code})"),
            Self::Unknown => write!(f, "Unknown"),
        }
    }
}

/// A single shell command's full lifecycle.
#[derive(Debug, Clone)]
pub struct CommandBlock {
    /// Stable identifier, monotonically increasing for the life of the process.
    pub id: CommandBlockId,

    /// Freminal correlation ID — matches the `fid` carried in the `A` and `D`
    /// markers emitted by the freminal shell integration scripts.  Used to
    /// correlate `A`/`B`/`C`/`D` marker pairs explicitly rather than relying
    /// on "most-recent-open" heuristics.
    pub fid: String,

    /// Row of `OSC 133 A` (prompt start).
    pub prompt_start_row: usize,

    /// Row of `OSC 133 B` (end of prompt, start of user input).  May equal
    /// `prompt_start_row` for single-line prompts.  `None` until `B` is
    /// received.
    pub command_start_row: Option<usize>,

    /// Row of `OSC 133 C` (output start / command executing).  `None` until
    /// `C` is received.
    pub output_start_row: Option<usize>,

    /// Row of `OSC 133 D` (command finished).  `None` while the command is
    /// still running.
    pub end_row: Option<usize>,

    /// Exit code from `OSC 133 D ; <code>`.  `None` if not yet finished or
    /// if the shell omitted the code.
    pub exit_code: Option<i32>,

    /// CWD captured from OSC 7 at the time of prompt start.
    pub cwd: Option<String>,

    /// Wall-clock timestamp of prompt start.
    pub started_at: SystemTime,

    /// Wall-clock timestamp of command-finished, if known.
    pub finished_at: Option<SystemTime>,
}

impl CommandBlock {
    /// Construct a fresh block at the given prompt row, with the given cwd and
    /// freminal correlation ID, started right now (`SystemTime::now()`).
    /// Allocates a new `CommandBlockId`.
    #[must_use]
    pub fn new_running(prompt_start_row: usize, cwd: Option<String>, fid: String) -> Self {
        Self {
            id: CommandBlockId::next(),
            fid,
            prompt_start_row,
            command_start_row: None,
            output_start_row: None,
            end_row: None,
            exit_code: None,
            cwd,
            started_at: SystemTime::now(),
            finished_at: None,
        }
    }

    /// Status derived from `finished_at` and `exit_code`.
    ///
    /// - `finished_at` is `None` ⇒ [`CommandStatus::Running`]
    /// - `finished_at` is `Some` and `exit_code == Some(0)` ⇒ [`CommandStatus::Success`]
    /// - `finished_at` is `Some` and `exit_code == Some(n)` where `n != 0` ⇒ `Failure(n)`
    /// - `finished_at` is `Some` and `exit_code` is `None` ⇒ [`CommandStatus::Unknown`]
    #[must_use]
    pub const fn status(&self) -> CommandStatus {
        match (self.finished_at, self.exit_code) {
            (None, _) => CommandStatus::Running,
            (Some(_), Some(0)) => CommandStatus::Success,
            (Some(_), Some(n)) => CommandStatus::Failure(n),
            (Some(_), None) => CommandStatus::Unknown,
        }
    }

    /// Duration if finished; `None` while still running or if `SystemTime`
    /// arithmetic fails (clock skew).
    #[must_use]
    pub fn duration(&self) -> Option<Duration> {
        let finished = self.finished_at?;
        finished.duration_since(self.started_at).ok()
    }

    /// Row range covered by this block: `(start, end)`.  `end` is `None`
    /// while running.  `start` is always `prompt_start_row`.
    #[must_use]
    pub const fn row_range(&self) -> (usize, Option<usize>) {
        (self.prompt_start_row, self.end_row)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    // ── CommandBlockId ───────────────────────────────────────────────────

    #[test]
    fn id_next_is_strictly_increasing() {
        let a = CommandBlockId::next();
        let b = CommandBlockId::next();
        let c = CommandBlockId::next();
        assert!(a < b, "expected {a} < {b}");
        assert!(b < c, "expected {b} < {c}");
    }

    #[test]
    fn id_display_formats_with_hash_prefix() {
        assert_eq!(CommandBlockId(1).to_string(), "#1");
        assert_eq!(CommandBlockId(2).to_string(), "#2");
        assert_eq!(CommandBlockId(42).to_string(), "#42");
    }

    // ── CommandStatus Display ────────────────────────────────────────────

    #[test]
    fn status_display_running() {
        assert_eq!(CommandStatus::Running.to_string(), "Running");
    }

    #[test]
    fn status_display_success() {
        assert_eq!(CommandStatus::Success.to_string(), "Success");
    }

    #[test]
    fn status_display_failure() {
        assert_eq!(CommandStatus::Failure(127).to_string(), "Failure(127)");
        assert_eq!(CommandStatus::Failure(1).to_string(), "Failure(1)");
        assert_eq!(CommandStatus::Failure(-1).to_string(), "Failure(-1)");
    }

    #[test]
    fn status_display_unknown() {
        assert_eq!(CommandStatus::Unknown.to_string(), "Unknown");
    }

    // ── CommandBlock::new_running ────────────────────────────────────────

    #[test]
    fn new_running_initializes_fields() {
        let before = SystemTime::now();
        let block =
            CommandBlock::new_running(7, Some("/home/user".to_string()), "test-fid".to_owned());
        let after = SystemTime::now();

        assert_eq!(block.prompt_start_row, 7);
        assert_eq!(block.cwd.as_deref(), Some("/home/user"));
        assert_eq!(block.fid, "test-fid");
        assert!(
            block.started_at >= before && block.started_at <= after,
            "started_at should be recent"
        );
        assert!(block.finished_at.is_none());
        assert!(block.exit_code.is_none());
        assert!(block.command_start_row.is_none());
        assert!(block.output_start_row.is_none());
        assert!(block.end_row.is_none());
    }

    #[test]
    fn new_running_allocates_unique_ids() {
        let b1 = CommandBlock::new_running(0, None, "fid-a".to_owned());
        let b2 = CommandBlock::new_running(0, None, "fid-b".to_owned());
        assert_ne!(b1.id, b2.id, "each block must get a unique id");
    }

    #[test]
    fn new_running_preserves_cwd_none() {
        let block = CommandBlock::new_running(0, None, "fid-c".to_owned());
        assert!(block.cwd.is_none());
    }

    #[test]
    fn new_running_stores_fid() {
        let block = CommandBlock::new_running(0, None, "my-correlation-id".to_owned());
        assert_eq!(block.fid, "my-correlation-id");
    }

    // ── status() ────────────────────────────────────────────────────────

    #[test]
    fn status_fresh_block_is_running() {
        let block = CommandBlock::new_running(0, None, "f1".to_owned());
        assert_eq!(block.status(), CommandStatus::Running);
    }

    #[test]
    fn status_finished_exit_0_is_success() {
        let mut block = CommandBlock::new_running(0, None, "f1".to_owned());
        block.finished_at = Some(SystemTime::now());
        block.exit_code = Some(0);
        assert_eq!(block.status(), CommandStatus::Success);
    }

    #[test]
    fn status_finished_exit_1_is_failure() {
        let mut block = CommandBlock::new_running(0, None, "f1".to_owned());
        block.finished_at = Some(SystemTime::now());
        block.exit_code = Some(1);
        assert_eq!(block.status(), CommandStatus::Failure(1));
    }

    #[test]
    fn status_finished_exit_negative_is_failure() {
        let mut block = CommandBlock::new_running(0, None, "f1".to_owned());
        block.finished_at = Some(SystemTime::now());
        block.exit_code = Some(-1);
        assert_eq!(block.status(), CommandStatus::Failure(-1));
    }

    #[test]
    fn status_finished_no_exit_code_is_unknown() {
        let mut block = CommandBlock::new_running(0, None, "f1".to_owned());
        block.finished_at = Some(SystemTime::now());
        block.exit_code = None;
        assert_eq!(block.status(), CommandStatus::Unknown);
    }

    // ── duration() ──────────────────────────────────────────────────────

    #[test]
    fn duration_running_is_none() {
        let block = CommandBlock::new_running(0, None, "f1".to_owned());
        assert!(block.duration().is_none());
    }

    #[test]
    fn duration_finished_after_start_is_some_positive() {
        let mut block = CommandBlock::new_running(0, None, "f1".to_owned());
        block.finished_at = Some(block.started_at + Duration::from_millis(500));
        match block.duration() {
            Some(dur) => {
                assert!(dur > Duration::ZERO, "duration should be positive");
                assert_eq!(dur, Duration::from_millis(500));
            }
            None => panic!("expected Some duration but got None"),
        }
    }

    #[test]
    fn duration_clock_skew_is_none() {
        let mut block = CommandBlock::new_running(0, None, "f1".to_owned());
        // finished_at before started_at — clock skew
        block.finished_at = Some(block.started_at - Duration::from_secs(1));
        assert!(
            block.duration().is_none(),
            "clock skew should produce None duration"
        );
    }

    // ── row_range() ──────────────────────────────────────────────────────

    #[test]
    fn row_range_running_block() {
        let block = CommandBlock::new_running(5, None, "f1".to_owned());
        assert_eq!(block.row_range(), (5, None));
    }

    #[test]
    fn row_range_finished_block() {
        let mut block = CommandBlock::new_running(5, None, "f1".to_owned());
        block.end_row = Some(12);
        assert_eq!(block.row_range(), (5, Some(12)));
    }
}
