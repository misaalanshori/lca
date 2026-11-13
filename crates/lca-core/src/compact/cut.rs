//! Where the cut lands (FR-SESS-4, docs/compaction.md's cut rules): the kept
//! window, the stable boundary the next compaction starts from, and the rule
//! that a tool call is never separated from its result.
//!
//! Every function here reads records and returns indices or a plan; nothing
//! writes, and nothing awaits.

use lca_protocol::Record;

/// One cut decision (gh #36 phase 1): where the summarized range
/// starts, where the kept window starts, and the kept boundary's id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CutPlan {
    /// First summarized record index.
    pub start: usize,
    /// First kept record index (the keep-recent window's head).
    pub kept_start: usize,
    /// Id of the first kept record (`""` when nothing is kept).
    pub first_kept_id: String,
}

/// Plan the cut (gh #36 phase 1): the summarized range runs from the
/// previous compaction's kept boundary (or the session start) to the
/// keep-recent window's head; the head snaps back past tool-pair
/// interiors, so a call and its results never split across the cut.
pub(crate) fn plan_cut(
    records: &[Record],
    cut: usize,
    keep_recent: u64,
    last_prompt: u64,
) -> CutPlan {
    let start = previous_kept_start(records, cut);
    let mut kept_start = if keep_recent == 0 {
        // Zero keeps nothing: the whole range summarizes (the stopgap's
        // shape for small windows, now an explicit choice).
        cut
    } else {
        let mut head = cut;
        let mut era = last_prompt;
        for (index, record) in records.iter().enumerate().take(cut).skip(start).rev() {
            if let Some(usage) = record_usage(record) {
                era = usage;
            }
            if last_prompt.saturating_sub(era) >= keep_recent {
                break;
            }
            head = index;
        }
        head
    };
    let end = cut.min(records.len());
    // Never cut inside a tool-call/result pair: retreat past a kept
    // result whose call summarized, and past a summarized call whose
    // result is kept (both reunite the pair on the kept side).
    while kept_start > start && kept_start < end && is_tool_result(&records[kept_start]) {
        kept_start -= 1;
    }
    while kept_start > start
        && is_tool_call(&records[kept_start - 1])
        && tool_result_in(&records[kept_start - 1], &records[kept_start..end])
    {
        kept_start -= 1;
    }
    // The anchor is the first record past the summarized range that
    // stays - the kept window's head, or the current turn itself when
    // the window keeps nothing - so the next plan starts here.
    let first_kept_id = records[kept_start..]
        .iter()
        .filter_map(Record::id)
        .next()
        .unwrap_or_default()
        .to_string();
    CutPlan {
        start,
        kept_start,
        first_kept_id,
    }
}

/// The summarized range's start: the previous compaction's kept
/// boundary when it is still in the log, else the entry after that
/// compaction, else the session start.
pub(super) fn previous_kept_start(records: &[Record], cut: usize) -> usize {
    let end = cut.min(records.len());
    let previous = records[..end]
        .iter()
        .rposition(|record| matches!(record, Record::Compaction { .. }));
    let Some(position) = previous else {
        return 0;
    };
    if let Record::Compaction { first_kept_id, .. } = &records[position]
        && !first_kept_id.is_empty()
        && let Some(found) = records[..end]
            .iter()
            .position(|record| record.id() == Some(first_kept_id.as_str()))
    {
        // Gh #36 phase 3: a retain-none anchor (the record's own id)
        // starts the next range after the entry itself, pi's shape;
        // any other anchor starts at the kept record.
        if matches!(&records[found], Record::Compaction { .. }) {
            return (found + 1).min(end);
        }
        return found;
    }
    position + 1
}

fn record_usage(record: &Record) -> Option<u64> {
    match record {
        Record::Assistant {
            usage: Some(usage), ..
        } => Some(crate::assemble::usage_prompt_tokens(usage)),
        _ => None,
    }
}

fn is_tool_result(record: &Record) -> bool {
    matches!(record, Record::ToolResult { .. })
}

fn is_tool_call(record: &Record) -> bool {
    matches!(record, Record::ToolCall { .. })
}

fn tool_result_in(call: &Record, kept: &[Record]) -> bool {
    let Record::ToolCall { call_id, .. } = call else {
        return false;
    };
    kept.iter().any(|record| match record {
        Record::ToolResult {
            call_id: answer, ..
        } => answer == call_id,
        _ => false,
    })
}
