//! Iterative summaries (`docs/compaction.md` phase 2): a compaction refines the
//! previous summary instead of restarting, and its marker is recognised in-band
//! so the candidate range can skip it.
//!
//! The marker is one line of text in a `user` record; that shape is why
//! `is_previous_summary_marker` and `previous_summary_text` sit together.

use lca_protocol::{FORMAT_VERSION, Record};

/// The previous summary for the next compaction (gh #36 phase 2): the
/// latest compaction record before the turn with a non-empty summary.
/// Searched before `cut` (not the range start): the kept boundary
/// sorts before its own compaction record once the replaced range is
/// suppressed from the view. `None` means a first compaction -
/// unchanged behavior.
pub(crate) fn previous_summary_text(records: &[Record], cut: usize) -> Option<String> {
    records[..cut.min(records.len())]
        .iter()
        .rev()
        .find_map(|record| match record {
            Record::Compaction { summary, .. } if !summary.is_empty() => Some(summary.clone()),
            _ => None,
        })
}

/// The in-band previous-summary marker (gh #36 phase 2): the latest
/// summary rides as the candidate's first record, capped at
/// [`PREVIOUS_SUMMARY_MAX_CHARS`], so the strategy refines it with no
/// WIT change. The strategy never persists it; the range computation
/// skips it. Later phases (file tracking, checkpoints) extend the
/// marker's data, not the plumbing.
pub(crate) const PREVIOUS_SUMMARY_MAX_CHARS: usize = 4000;

/// Whether a record is the in-band previous-summary marker.
pub(crate) fn is_previous_summary_marker(record: &Record) -> bool {
    match record {
        Record::Custom { custom_type, .. } => custom_type == lca_protocol::PREVIOUS_SUMMARY_TYPE,
        _ => false,
    }
}

pub(crate) fn previous_summary_marker(summary: &str) -> Record {
    let text: String = summary.chars().take(PREVIOUS_SUMMARY_MAX_CHARS).collect();
    Record::Custom {
        v: FORMAT_VERSION,
        ts: lca_session::now_ms(),
        id: "previous-summary".to_string(),
        parent: None,
        custom_type: lca_protocol::PREVIOUS_SUMMARY_TYPE.to_string(),
        data: serde_json::json!({ "text": text }),
    }
}
