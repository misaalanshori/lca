//! The compaction view and fork-chain resolution.
//!
//! Display mode hides records inside a compacted range and shows the
//! summary in their place; audit mode walks everything. A fork's resolved
//! history is its ancestors' records up to the fork point, then its own.

use std::collections::BTreeSet;

use lca_protocol::Record;

use crate::Result;
use crate::store::Session;
use crate::store::{ReadOutcome, SessionStore};

/// How much of a log a read resolves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    /// Substitute compaction summaries and follow forks: what the interface
    /// and a normal export show.
    Display,
    /// Walk every record, following forks but replacing nothing: the audit
    /// view behind `lca export --audit`.
    Audit,
}

impl SessionStore {
    /// Read a session's resolved view (FR-SESS-7 export and the interface).
    pub fn read_with(&self, session: &Session, mode: ViewMode) -> Result<ReadOutcome> {
        let mut outcome = self.chain(session)?;
        if matches!(mode, ViewMode::Display) {
            outcome.records = suppress_compacted(outcome.records);
        }
        Ok(outcome)
    }

    /// All raw records of this session and its ancestors, forks followed:
    /// the ancestors stop at the fork point, and the child's own
    /// `session-start` is dropped once a parent contributed history.
    fn chain(&self, session: &Session) -> Result<ReadOutcome> {
        let mut outcome = ReadOutcome {
            records: Vec::new(),
            truncated: false,
            skipped_unknown: 0,
            warnings: Vec::new(),
        };
        let mut current = Some(session.clone());
        let mut visited = BTreeSet::new();
        while let Some(handle) = current {
            if !visited.insert(handle.id().to_string()) {
                return Err(crate::Error::ForkCycle {
                    session: handle.id().to_string(),
                });
            }
            let meta = self.meta(&handle)?;
            let own = self.read(&handle)?;
            outcome.truncated |= own.truncated;
            outcome.skipped_unknown += own.skipped_unknown;
            outcome.warnings.extend(own.warnings);
            let cut_at = match &meta.parent_session {
                Some(parent_id) => {
                    let parent = self.session_handle(parent_id, &handle)?;
                    let parent_chain = self.chain(&parent)?;
                    outcome.truncated |= parent_chain.truncated;
                    outcome.skipped_unknown += parent_chain.skipped_unknown;
                    outcome.warnings.extend(parent_chain.warnings);
                    let parent_record = meta.parent_record.as_deref().unwrap_or_default();
                    let cut = parent_chain
                        .records
                        .iter()
                        .position(|r| r.id() == Some(parent_record))
                        .ok_or_else(|| crate::Error::ForkPointMissing {
                            session: handle.id().to_string(),
                            record: parent_record.to_string(),
                        })?;
                    outcome
                        .records
                        .splice(0..0, parent_chain.records[..=cut].iter().cloned());
                    true
                }
                None => false,
            };
            let mut tail = own.records;
            if cut_at {
                // Keep exactly one session-start: the root's.
                if matches!(tail.first(), Some(Record::SessionStart { .. })) {
                    tail.remove(0);
                }
            }
            outcome.records.extend(tail);
            current = match meta.parent_session {
                Some(_) => break, // ancestors already spliced in above
                None => None,
            };
        }
        Ok(outcome)
    }

    /// Reconstruct the parent handle for a fork (parent id plus the child's
    /// project key, since both live under the same project).
    fn session_handle(&self, parent_id: &str, like: &Session) -> Result<Session> {
        let dir = like.dir().parent().expect("project dir").join(parent_id);
        if !dir.is_dir() {
            return Err(crate::Error::MissingParent {
                session: parent_id.to_string(),
            });
        }
        Ok(Session::new(parent_id.to_string(), dir))
    }
}

/// Hide every record inside any `compaction` record's replaced range; a
/// compacted record that is itself a compaction is hidden too (an outer
/// range covering an inner one keeps one coherent view).
pub(crate) fn suppress_compacted(records: Vec<Record>) -> Vec<Record> {
    let positions = replaced_positions(&records);
    records
        .into_iter()
        .enumerate()
        .filter(|(index, _)| !positions.contains(index))
        .map(|(_, record)| record)
        .collect()
}

fn replaced_positions(records: &[Record]) -> BTreeSet<usize> {
    let mut suppressed = BTreeSet::new();
    for record in records {
        let Record::Compaction {
            replaced_from,
            replaced_to,
            ..
        } = record
        else {
            continue;
        };
        let Some(start) = records
            .iter()
            .position(|r| r.id() == Some(replaced_from.as_str()))
        else {
            continue; // unknown range: hide nothing, warn in the reader
        };
        let Some(end) = records
            .iter()
            .position(|r| r.id() == Some(replaced_to.as_str()))
        else {
            continue;
        };
        if start <= end {
            suppressed.extend(start..=end);
        }
    }
    suppressed
}
