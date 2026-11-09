//! Test seams for the turn lifecycle (gh #388 gap 2). Split from `chat.rs`
//! with the lifecycle itself (gh #574), so the helper sits beside the code it
//! fakes rather than behind it.

use super::Chat;

impl Chat {
    /// Put the separator into a compaction with a reason (gh #388 gap 2
    /// tests). The live path calls the same setter from `poll_compact`.
    #[cfg(test)]
    pub(crate) fn mark_compacting_for_test(&mut self, reason: crate::CompactionReason) {
        self.separator.compacting(reason);
    }
}
