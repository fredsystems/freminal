// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! The buffer's OSC 133 command blocks, with a change generation.
//!
//! Every terminal snapshot carries the command blocks as an
//! `Arc<[CommandBlock]>`. Building that slice clones every block, and each
//! block owns heap `String`s (`fid`, `cwd`), so it costs O(blocks)
//! allocations -- about 330 us at the 10 000-block cap -- whether or not a
//! block changed, and blocks change only on OSC 133 events and when pruned.
//! [`CommandBlockLog`] therefore stamps a [`CommandBlocksGeneration`] on every
//! mutation, so a snapshot builder can keep the slice it built and reuse it
//! until the generation moves.
//!
//! The stamp is only trustworthy if no mutation can bypass it, so the log
//! hands out **no** `DerefMut`: reads go through `Deref<Target = VecDeque<_>>`
//! and every mutation is one of the explicit methods below, each of which
//! advances the generation. Adding a mutator that forgets to do so is the one
//! way to break the cache, which is why the surface is deliberately small.

use std::collections::VecDeque;
use std::ops::Deref;
use std::sync::atomic::{AtomicU64, Ordering};

use freminal_common::buffer_states::command_block::CommandBlock;

/// Source of generation values.
///
/// Process-global, not per-log, so two different logs -- a fresh `Buffer`
/// replacing an old one, say -- can never produce the same generation for
/// different contents. A cache keyed on a per-log counter would be fooled by
/// a replacement log that happened to reach the same count.
static NEXT_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Identifies one state of a [`CommandBlockLog`]'s contents.
///
/// Equal generations from the same log (or a clone of it) mean identical
/// contents. Generations are otherwise opaque: they are not ordered and say
/// nothing about how much changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandBlocksGeneration(u64);

impl CommandBlocksGeneration {
    fn next() -> Self {
        Self(NEXT_GENERATION.fetch_add(1, Ordering::Relaxed))
    }
}

/// Command blocks, oldest first, plus the generation of their contents.
/// See the module docs.
#[derive(Debug, Clone)]
pub(in crate::buffer) struct CommandBlockLog {
    blocks: VecDeque<CommandBlock>,
    generation: CommandBlocksGeneration,
    /// The generation at which the owner last vouched for some property of
    /// the contents (see [`Self::checkpoint`]). `None` until it has.
    checkpoint: Option<CommandBlocksGeneration>,
}

impl CommandBlockLog {
    /// An empty log with a fresh generation.
    pub(in crate::buffer) fn new() -> Self {
        Self {
            blocks: VecDeque::new(),
            generation: CommandBlocksGeneration::next(),
            checkpoint: None,
        }
    }

    /// The generation of the current contents.
    pub(in crate::buffer) const fn generation(&self) -> CommandBlocksGeneration {
        self.generation
    }

    /// Record that the current contents are known to have some property the
    /// caller cares about (for the buffer: no block holds an alternate-screen
    /// row number). The log does not know what the property is.
    ///
    /// Any later mutation advances the generation, so
    /// [`Self::changed_since_checkpoint`] reports it and the caller must
    /// re-establish the property by inspection. This makes "has anything
    /// changed since I last checked" an O(1) question, which the mutators'
    /// generation stamp already answers.
    pub(in crate::buffer) const fn checkpoint(&mut self) {
        self.checkpoint = Some(self.generation);
    }

    /// Whether the contents differ from those at the last
    /// [`Self::checkpoint`]. `true` if there has never been one.
    pub(in crate::buffer) fn changed_since_checkpoint(&self) -> bool {
        self.checkpoint != Some(self.generation)
    }

    fn touch(&mut self) {
        self.generation = CommandBlocksGeneration::next();
    }

    /// Append a block as the newest.
    pub(in crate::buffer) fn push_back(&mut self, block: CommandBlock) {
        self.blocks.push_back(block);
        self.touch();
    }

    /// Remove and return the oldest block. The generation only advances when
    /// a block was actually removed.
    pub(in crate::buffer) fn pop_front(&mut self) -> Option<CommandBlock> {
        let popped = self.blocks.pop_front();
        if popped.is_some() {
            self.touch();
        }
        popped
    }

    /// Remove every block. The generation only advances when there was
    /// something to remove.
    pub(in crate::buffer) fn clear(&mut self) {
        if !self.blocks.is_empty() {
            self.blocks.clear();
            self.touch();
        }
    }

    /// Keep only the blocks for which `keep` returns `true`, letting it edit
    /// the blocks it keeps.
    ///
    /// The generation always advances: `keep` may rewrite a surviving block's
    /// row fields (reflow, ED 2, leaving the alternate screen), and a closure
    /// that edits cannot be told apart from one that does not. Every caller is
    /// a rare structural event, so over-reporting costs one rebuild.
    pub(in crate::buffer) fn retain_mut(&mut self, keep: impl FnMut(&mut CommandBlock) -> bool) {
        self.blocks.retain_mut(keep);
        self.touch();
    }

    /// Mutable iteration. The generation advances up front, because the caller
    /// may edit any block it visits. Callers are the OSC 133 marker handlers,
    /// which run once per shell prompt cycle.
    pub(in crate::buffer) fn iter_mut(
        &mut self,
    ) -> std::collections::vec_deque::IterMut<'_, CommandBlock> {
        self.touch();
        self.blocks.iter_mut()
    }
}

impl Deref for CommandBlockLog {
    type Target = VecDeque<CommandBlock>;

    fn deref(&self) -> &Self::Target {
        &self.blocks
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use freminal_common::buffer_states::row_number::RowNumber;

    fn block(fid: &str) -> CommandBlock {
        CommandBlock::new_running(RowNumber::ZERO, None, fid.to_owned())
    }

    #[test]
    fn push_back_advances_the_generation() {
        let mut log = CommandBlockLog::new();
        let before = log.generation();
        log.push_back(block("a"));
        assert_ne!(log.generation(), before);
        assert_eq!(log.len(), 1);
    }

    #[test]
    fn pop_front_advances_the_generation_only_when_it_removes_something() {
        let mut log = CommandBlockLog::new();
        let empty = log.generation();
        assert!(log.pop_front().is_none());
        assert_eq!(log.generation(), empty, "nothing removed, nothing changed");

        log.push_back(block("a"));
        let one = log.generation();
        assert!(log.pop_front().is_some());
        assert_ne!(log.generation(), one);
    }

    #[test]
    fn clear_advances_the_generation_only_when_it_removes_something() {
        let mut log = CommandBlockLog::new();
        let empty = log.generation();
        log.clear();
        assert_eq!(log.generation(), empty);

        log.push_back(block("a"));
        let one = log.generation();
        log.clear();
        assert_ne!(log.generation(), one);
        assert!(log.is_empty());
    }

    #[test]
    fn retain_mut_always_advances_the_generation() {
        let mut log = CommandBlockLog::new();
        log.push_back(block("a"));
        let before = log.generation();
        // Keeps everything but edits it: the contents differ, so must the
        // generation.
        log.retain_mut(|b| {
            b.fid.push('!');
            true
        });
        assert_ne!(log.generation(), before);
        assert_eq!(log.front().unwrap().fid, "a!");
    }

    #[test]
    fn iter_mut_advances_the_generation() {
        let mut log = CommandBlockLog::new();
        log.push_back(block("a"));
        let before = log.generation();
        for b in log.iter_mut() {
            b.exit_code = Some(3);
        }
        assert_ne!(log.generation(), before);
        assert_eq!(log.front().unwrap().exit_code, Some(3));
    }

    #[test]
    fn a_log_has_changed_since_checkpoint_until_one_is_taken() {
        let mut log = CommandBlockLog::new();
        assert!(log.changed_since_checkpoint(), "no checkpoint yet");
        log.checkpoint();
        assert!(!log.changed_since_checkpoint());
    }

    #[test]
    fn every_mutation_invalidates_the_checkpoint() {
        let mut log = CommandBlockLog::new();
        log.push_back(block("a"));

        log.checkpoint();
        log.push_back(block("b"));
        assert!(log.changed_since_checkpoint(), "push_back");

        log.checkpoint();
        assert!(log.pop_front().is_some());
        assert!(log.changed_since_checkpoint(), "pop_front");

        log.checkpoint();
        log.retain_mut(|_| true);
        assert!(log.changed_since_checkpoint(), "retain_mut");

        log.checkpoint();
        let _ = log.iter_mut().count();
        assert!(log.changed_since_checkpoint(), "iter_mut");

        log.checkpoint();
        log.clear();
        assert!(log.changed_since_checkpoint(), "clear of a non-empty log");
    }

    #[test]
    fn reads_and_no_op_removals_keep_the_checkpoint() {
        let mut log = CommandBlockLog::new();
        log.checkpoint();
        let _ = (log.len(), log.front(), log.iter().count());
        assert!(log.pop_front().is_none());
        log.clear();
        assert!(!log.changed_since_checkpoint());
    }

    #[test]
    fn reads_do_not_advance_the_generation() {
        let mut log = CommandBlockLog::new();
        log.push_back(block("a"));
        let before = log.generation();
        let _ = (log.len(), log.front(), log.iter().count(), log.is_empty());
        assert_eq!(log.generation(), before);
    }

    #[test]
    fn a_clone_shares_the_generation_until_either_changes() {
        let mut log = CommandBlockLog::new();
        log.push_back(block("a"));
        let mut copy = log.clone();
        assert_eq!(copy.generation(), log.generation());
        copy.push_back(block("b"));
        assert_ne!(copy.generation(), log.generation());
    }

    #[test]
    fn two_logs_never_share_a_generation_for_different_contents() {
        // A replacement log that reaches the same length must not look like
        // the log it replaced.
        let mut a = CommandBlockLog::new();
        let mut b = CommandBlockLog::new();
        a.push_back(block("a"));
        b.push_back(block("b"));
        assert_ne!(a.generation(), b.generation());
        assert_ne!(CommandBlockLog::new().generation(), a.generation());
    }
}
