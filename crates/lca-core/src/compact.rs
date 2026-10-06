//! Compaction invocation (S2): the strategy call shared by the threshold
//! path (FR-SESS-4) and the manual `/compact` trigger, and the manual
//! trigger itself. Split out of `lib.rs`.

use std::sync::Arc;

use lca_protocol::{FORMAT_VERSION, Record, TurnEvent};
use lca_session::{Session, SessionStore, ViewMode};

use super::{NullSink, TurnSink, drive_blocking};
use crate::registry::ExtensionRegistry;

/// The compaction call itself, shared by the threshold path
/// (FR-SESS-4) and the manual `/compact` trigger: the summary always
/// comes from the compaction world's strategy (FR-SESS-5 - there is no
/// built-in summarizing path), the durable record is the host's, and
/// the candidate range is the caller's to choose.
// Eight inputs: the five context handles plus candidate, kept anchor,
// and reason - a parameter struct would only rename them.
#[allow(clippy::too_many_arguments)]
pub(super) async fn compact_candidate(
    store: &SessionStore,
    session: &Session,
    extensions: &ExtensionRegistry,
    completion_backend: Option<&Arc<dyn lca_tools::CompletionBackend>>,
    candidate: Vec<Record>,
    first_kept_id: &str,
    sink: &mut dyn TurnSink,
    reason: &str,
) -> Result<String, String> {
    let Some(strategy) = extensions.compaction_strategy().cloned() else {
        return Err(
            "no compaction extension is enabled: compaction runs through a compaction-world extension (FR-SESS-5)"
                .to_string(),
        );
    };
    if candidate.len() < 2 {
        return Err("nothing to compact: fewer than two compactable records".to_string());
    }
    sink.on_event(TurnEvent::CompactionStarted {
        reason: reason.to_string(),
    });
    let summary = match strategy.compact(&candidate).await {
        Ok(summary) => summary,
        Err(err) => {
            let detail = format!("compaction strategy `{}` failed: {err}", strategy.name());
            sink.on_event(TurnEvent::CompactionEnded {
                reason: reason.to_string(),
                success: false,
            });
            sink.on_event(TurnEvent::ExtensionEvent {
                extension: strategy.name().to_string(),
                event: "compaction-failed".to_string(),
                detail: detail.clone(),
            });
            let _ = store.append(
                session,
                Record::ExtensionEvent {
                    v: FORMAT_VERSION,
                    ts: lca_session::now_ms(),
                    id: lca_session::new_record_id(),
                    extension: strategy.name().to_string(),
                    event: "compaction-failed".to_string(),
                    detail: detail.clone(),
                },
            );
            return Err(detail);
        }
    };
    let usage = completion_backend.and_then(|backend| backend.take_usage());
    let replaced_from = candidate.first().and_then(Record::id).unwrap_or_default();
    let replaced_to = candidate.last().and_then(Record::id).unwrap_or_default();
    if let Err(err) = store.append(
        session,
        Record::Compaction {
            v: FORMAT_VERSION,
            ts: lca_session::now_ms(),
            id: lca_session::new_record_id(),
            replaced_from: replaced_from.to_string(),
            replaced_to: replaced_to.to_string(),
            first_kept_id: first_kept_id.to_string(),
            summary: summary.clone(),
            strategy: strategy.name().to_string(),
            usage,
        },
    ) {
        let detail = format!("cannot write the compaction record: {err}");
        sink.on_event(TurnEvent::ExtensionEvent {
            extension: strategy.name().to_string(),
            event: "compaction-failed".to_string(),
            detail: detail.clone(),
        });
        sink.on_event(TurnEvent::CompactionEnded {
            reason: reason.to_string(),
            success: false,
        });
        return Err(detail);
    }
    sink.on_event(TurnEvent::CompactionEnded {
        reason: reason.to_string(),
        success: true,
    });
    Ok(summary)
}

/// The effective token reserve (gh #36 phase 1): the configured
/// absolute reserve, or the stopgap's fraction derivation when unset
/// (0), so the default behavior is the old threshold by construction.
///
/// Later #36 phases hook here without changing the shape: split-span
/// cuts, iterative previous-summary context, file tracking, the
/// system-message checkpoint, recovery ordering, retain-none. Each
/// gets a parameter on the plan below, not a new trigger.
pub fn compaction_reserve(threshold: f64, reserve_tokens: u64, window: u32) -> u64 {
    if reserve_tokens > 0 {
        return reserve_tokens;
    }
    let fraction = threshold.clamp(0.0, 1.0);
    // Rounded, not floored: binary float dust turns `0.2 * 128000`
    // into 25599.99, and the reserve should read 25600.
    ((1.0 - fraction) * f64::from(window)).round() as u64
}

/// Whether the trigger fires (gh #36 phase 1): pi's formula,
/// `context_tokens > context_window - reserve`, strict on both sides.
pub(crate) fn compaction_fires(context_tokens: u64, window: u32, reserve: u64) -> bool {
    context_tokens > u64::from(window).saturating_sub(reserve)
}

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
fn previous_kept_start(records: &[Record], cut: usize) -> usize {
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
        return found;
    }
    position + 1
}

fn record_usage(record: &Record) -> Option<u64> {
    match record {
        Record::Assistant {
            usage: Some(usage), ..
        } => Some(super::assemble::usage_prompt_tokens(usage)),
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

/// The manual trigger behind the `/compact` built-in: compact now,
/// every compactable record, no threshold involved - still through the
/// compaction world only (FR-SESS-5). Synchronous by contract: the
/// caller is the interface's command thread, and `drive_blocking`
/// gives the work a thread of its own.
pub fn compact_now(
    store: Arc<SessionStore>,
    session: Session,
    extensions: Arc<ExtensionRegistry>,
    completion_backend: Option<Arc<dyn lca_tools::CompletionBackend>>,
) -> Result<String, String> {
    drive_blocking(async move {
        let read = store
            .read_with(&session, ViewMode::Display)
            .map_err(|err| format!("cannot read the session: {err}"))?;
        let candidate: Vec<Record> = read
            .records
            .iter()
            .filter(|record| record.id().is_some())
            .cloned()
            .collect();
        compact_candidate(
            &store,
            &session,
            &extensions,
            completion_backend.as_ref(),
            candidate,
            // Manual compaction keeps nothing past the range: no kept
            // boundary for the next plan to start from.
            "",
            &mut NullSink,
            "manual",
        )
        .await
    })
}

#[cfg(test)]
mod cut_tests {
    use super::*;
    use lca_protocol::Usage;

    fn user(id: &str) -> Record {
        Record::User {
            v: FORMAT_VERSION,
            ts: 1,
            id: id.to_string(),
            content: id.to_string(),
            attachments: Vec::new(),
            queue: None,
        }
    }

    fn assistant(id: &str, prompt_tokens: u64) -> Record {
        Record::Assistant {
            v: FORMAT_VERSION,
            ts: 1,
            id: id.to_string(),
            content: Vec::new(),
            reasoning: None,
            model: None,
            provider: None,
            usage: Some(Usage {
                input: prompt_tokens,
                ..Default::default()
            }),
        }
    }

    fn call(id: &str, call_id: &str) -> Record {
        Record::ToolCall {
            v: FORMAT_VERSION,
            ts: 1,
            id: id.to_string(),
            call_id: call_id.to_string(),
            name: "read".to_string(),
            arguments: "{}".to_string(),
            source: lca_protocol::ToolSource::Builtin,
        }
    }

    fn result(id: &str, call_id: &str) -> Record {
        Record::ToolResult {
            v: FORMAT_VERSION,
            ts: 1,
            id: id.to_string(),
            call_id: call_id.to_string(),
            status: lca_protocol::ToolResultStatus::Ok,
            content: None,
            attachment: None,
            truncated: false,
            exit_code: None,
            full_output_path: None,
        }
    }

    // Verifies: gh #36 phase 1 - the trigger is strict on both sides
    // of `window - reserve`, and reserve 0 derives the stopgap's
    // fraction (the old behavior by construction).
    #[test]
    fn the_trigger_fires_strictly_past_window_minus_reserve() {
        assert!(compaction_fires(5001, 10_000, 5000));
        assert!(!compaction_fires(5000, 10_000, 5000), "equal does not fire");
        assert!(!compaction_fires(4999, 10_000, 5000));
        assert_eq!(
            compaction_reserve(0.5, 0, 10_000),
            5000,
            "reserve 0 derives"
        );
        assert_eq!(compaction_reserve(0.8, 0, 128_000), 25_600);
        assert_eq!(compaction_reserve(0.5, 4096, 10_000), 4096, "absolute wins");
    }

    // Verifies: gh #36 phase 1 - the keep-recent window keeps the
    // latest exchange verbatim and summarizes the rest.
    #[test]
    fn the_keep_recent_window_holds_the_latest_exchange() {
        // Eras: u1/a1 at 30k, u2/a2 at 70k, u3/a3 at 100k; keep 20k.
        let records = vec![
            user("u1"),
            assistant("a1", 30_000),
            user("u2"),
            assistant("a2", 70_000),
            user("u3"),
            assistant("a3", 100_000),
        ];
        let plan = plan_cut(&records, 6, 20_000, 100_000);
        assert_eq!(plan.start, 0, "no previous compaction");
        assert_eq!(plan.kept_start, 4, "u3/a3 stay verbatim");
        assert_eq!(plan.first_kept_id, "u3");
    }

    // Verifies: gh #36 phase 1 - a cut that would strand a tool result
    // retreats so the call and its results stay on one side.
    #[test]
    fn a_cut_never_splits_a_tool_call_from_its_result() {
        let records = vec![
            user("u1"),
            assistant("a1", 30_000),
            call("c1", "k1"),
            result("r1", "k1"),
            user("u2"),
            assistant("a2", 100_000),
        ];
        // keep_recent lands the head on the result without the rule.
        let plan = plan_cut(&records, 6, 60_000, 100_000);
        let kept: Vec<&str> = records[plan.kept_start..6]
            .iter()
            .filter_map(Record::id)
            .collect();
        assert!(
            !(kept.contains(&"r1") && !kept.contains(&"c1")),
            "no stranded result: {kept:?}"
        );
        assert!(
            !(kept.contains(&"c1") && !kept.contains(&"r1")),
            "no stranded call: {kept:?}"
        );
    }

    // Verifies: gh #36 phase 1 - the next compaction starts at the
    // previous kept boundary, not at the session start.
    #[test]
    fn the_next_compaction_starts_at_the_previous_kept_boundary() {
        let records = vec![
            user("u1"),
            assistant("a1", 30_000),
            Record::Compaction {
                v: FORMAT_VERSION,
                ts: 1,
                id: "cmp1".to_string(),
                replaced_from: "u1".to_string(),
                replaced_to: "a1".to_string(),
                first_kept_id: "u2".to_string(),
                summary: "s".to_string(),
                strategy: "x".to_string(),
                usage: None,
            },
            user("u2"),
            assistant("a2", 100_000),
        ];
        let plan = plan_cut(&records, 5, 20_000, 100_000);
        assert_eq!(plan.start, 3, "starts at the kept boundary");
    }
}
