//! Transactions, undo and redo (§5.7).
//!
//! An edit is a [`Transaction`]: primitive replacements, each holding the
//! pieces it removed and the pieces it inserted, so undo and redo are exact
//! and cost no copying (pieces reference the original and the add buffer).
//! Consecutive typing (or deleting) within a second coalesces into one undo
//! step. The history is bounded by entry count and by the bytes it pins in
//! memory.

use std::time::Instant;

use crate::tree::PieceTree;

/// One replacement: at `at`, `removed` was replaced by `inserted`.
#[derive(Clone, Debug)]
pub struct Op {
    pub at: u64,
    pub removed: PieceTree,
    pub inserted: PieceTree,
}

#[derive(Clone, Debug)]
pub enum Change {
    /// Replacements in the order they were applied.
    Ops(Vec<Op>),
    /// The whole tree swapped (replace-all and its undo).
    Tree { before: PieceTree, after: PieceTree },
    /// Save format changed (encoding, BOM, line endings); no text change.
    Format {
        before: crate::document::SaveFormat,
        after: crate::document::SaveFormat,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Typing,
    Deleting,
    Other,
}

#[derive(Clone, Debug)]
pub struct Transaction {
    pub id: u64,
    pub change: Change,
    pub sel_before: Vec<crate::document::Selection>,
    pub sel_after: Vec<crate::document::Selection>,
    pub kind: Kind,
    pub at: Instant,
}

/// Coalescing window for typing.
pub const COALESCE_MS: u128 = 1000;
pub const MAX_ENTRIES: usize = 10_000;

#[derive(Debug, Default)]
pub struct History {
    undo: Vec<Transaction>,
    redo: Vec<Transaction>,
    next_id: u64,
    /// Entries dropped from the bottom to stay within bounds.
    pub trimmed: u64,
}

impl History {
    pub fn new_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// Records `tx`. Returns true if it was merged into the previous entry.
    pub fn push(&mut self, mut tx: Transaction) -> bool {
        self.redo.clear();
        if let Some(prev) = self.undo.last_mut()
            && tx.kind != Kind::Other
            && prev.kind == tx.kind
            && tx.at.duration_since(prev.at).as_millis() < COALESCE_MS
            && prev.sel_after == tx.sel_before
            && let (Change::Ops(a), Change::Ops(b)) = (&mut prev.change, &mut tx.change)
        {
            a.append(b);
            prev.sel_after = tx.sel_after;
            prev.at = tx.at;
            return true;
        }
        self.undo.push(tx);
        if self.undo.len() > MAX_ENTRIES {
            self.undo.remove(0);
            self.trimmed += 1;
        }
        false
    }

    /// Records a replayed transaction with the merge decision taken when it
    /// was first recorded (recovery replays undo grouping exactly).
    pub fn push_forced(&mut self, mut tx: Transaction, merge: bool) {
        self.redo.clear();
        if merge
            && let Some(prev) = self.undo.last_mut()
            && let (Change::Ops(a), Change::Ops(b)) = (&mut prev.change, &mut tx.change)
        {
            a.append(b);
            prev.sel_after = tx.sel_after;
            return;
        }
        self.undo.push(tx);
        if self.undo.len() > MAX_ENTRIES {
            self.undo.remove(0);
            self.trimmed += 1;
        }
    }

    pub fn pop_undo(&mut self) -> Option<Transaction> {
        self.undo.pop()
    }

    pub fn push_redo(&mut self, tx: Transaction) {
        self.redo.push(tx);
    }

    pub fn pop_redo(&mut self) -> Option<Transaction> {
        self.redo.pop()
    }

    /// Back onto the undo stack after a redo (no coalescing, keeps redo).
    pub fn push_undone(&mut self, tx: Transaction) {
        self.undo.push(tx);
    }

    /// Id of the newest applied transaction (0 for none): the document is
    /// clean while this equals the id recorded at the last save.
    pub fn top(&self) -> u64 {
        self.undo.last().map_or(0, |t| t.id)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Every transaction still reachable by undo or redo.
    pub fn all(&self) -> impl Iterator<Item = &Transaction> {
        self.undo.iter().chain(self.redo.iter())
    }

    pub fn all_mut(&mut self) -> impl Iterator<Item = &mut Transaction> {
        self.undo.iter_mut().chain(self.redo.iter_mut())
    }

    /// Drops the oldest entries until `keep` returns true for the rest
    /// (used when undo data can't be preserved across a save).
    pub fn drop_oldest(&mut self, n: usize) {
        let n = n.min(self.undo.len());
        self.undo.drain(..n);
        self.trimmed += n as u64;
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }

    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }
}
