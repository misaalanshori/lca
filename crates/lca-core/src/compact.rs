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
// Seven inputs: the five context handles plus candidate, the phase-3
// meta bundle, and reason - one struct for what the caller chose.
#[allow(clippy::too_many_arguments)]
pub(super) async fn compact_candidate(
    store: &SessionStore,
    session: &Session,
    extensions: &ExtensionRegistry,
    completion_backend: Option<&Arc<dyn lca_tools::CompletionBackend>>,
    candidate: Vec<Record>,
    meta: CompactMeta,
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
    // The in-band previous-summary marker is strategy context, not
    // replaced range: the range names real log records only.
    let replaced_from = candidate
        .iter()
        .filter(|record| !is_previous_summary_marker(record))
        .filter_map(Record::id)
        .next()
        .unwrap_or_default();
    let replaced_to = candidate
        .iter()
        .filter(|record| !is_previous_summary_marker(record))
        .filter_map(Record::id)
        .next_back()
        .unwrap_or_default();
    // Gh #36 phase 3: retain-none anchors the record's own id (pi's
    // shape), so the id exists before the append.
    let id = lca_session::new_record_id();
    let first_kept_id = meta.first_kept_id.unwrap_or_else(|| id.clone());
    // Gh #36 phase 3: the file lists ride the summary text (pi
    // appends them when relevant) and the record fields.
    let summary = format!(
        "{summary}{}",
        format_file_lists(&meta.read_files, &meta.modified_files)
    );
    if let Err(err) = store.append(
        session,
        Record::Compaction {
            v: FORMAT_VERSION,
            ts: lca_session::now_ms(),
            id,
            replaced_from: replaced_from.to_string(),
            replaced_to: replaced_to.to_string(),
            first_kept_id,
            summary: summary.clone(),
            strategy: strategy.name().to_string(),
            usage,
            read_files: meta.read_files,
            modified_files: meta.modified_files,
            system_prompt: meta.system_prompt,
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
/// The #36 epic is complete (trigger, reserve, cut points, split
/// spans, iteration, file tracking, checkpoint, recovery, retain-none).
/// Later compaction work gets a parameter on the plan below, not a
/// new trigger.
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

/// Whether a provider failure is a context overflow (gh #36 phase 3):
/// the provider patterns from the #36 discussion plus our own budgeted
/// `token cap` message. Anything else (validation, auth, transport)
/// surfaces without compacting.
pub(crate) fn is_overflow_error(message: &str) -> bool {
    let lower = message.to_lowercase();
    lower.contains("token cap")
        || lower.contains("prompt is too long")
        || lower.contains("exceeds token limit")
        || lower.contains("context window")
        || lower.contains("context length")
        || lower.contains("context_length")
        || (lower.contains("max_tokens") && lower.contains("exceed"))
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
        custom_type: lca_protocol::PREVIOUS_SUMMARY_TYPE.to_string(),
        data: serde_json::json!({ "text": text }),
    }
}

/// Files per list a compaction record carries (gh #36 phase 3):
/// cumulative tracking stays bounded storage, sorted and truncated.
/// Pi tracks unbounded sets; the cap is the documented divergence.
pub const MAX_TRACKED_FILES: usize = 200;

/// The `custom` record type noting a system-prompt change across
/// compactions (gh #36 phase 3): the detection, not a migration.
pub const SYSTEM_PROMPT_CHANGE_TYPE: &str = "system-prompt-change";

/// What the caller chose for this compaction (gh #36 phase 3): the
/// kept anchor (`None` compacts everything and anchors the record's
/// own id, pi's retain-none shape), the accumulated file lists, and
/// the system-prompt checkpoint (`None` when the caller never knew
/// the prompt).
pub(super) struct CompactMeta {
    pub(super) first_kept_id: Option<String>,
    pub(super) read_files: Vec<String>,
    pub(super) modified_files: Vec<String>,
    pub(super) system_prompt: Option<String>,
}

/// Cumulative file lists for a compaction (gh #36 phase 3, pi's
/// `CompactionDetails`): tool calls in the candidate plus the file
/// lists of earlier compactions in scope, read-only files sorted
/// first, each list capped at [`MAX_TRACKED_FILES`]. A file both read
/// and modified counts as modified, pi's rule.
pub(crate) fn accumulate_files(
    candidate: &[Record],
    priors: &[Record],
) -> (Vec<String>, Vec<String>) {
    use std::collections::BTreeSet;
    let mut read: BTreeSet<String> = BTreeSet::new();
    let mut modified: BTreeSet<String> = BTreeSet::new();
    let mut file_op = |name: &str, arguments: &str| {
        let path = serde_json::from_str::<serde_json::Value>(arguments)
            .ok()
            .and_then(|args| args.get("path")?.as_str().map(str::to_string));
        let Some(path) = path else { return };
        match lca_tools::canonical_tool_name(name) {
            "read" => {
                read.insert(path);
            }
            "write" | "edit" => {
                modified.insert(path);
            }
            _ => {}
        }
    };
    for record in candidate {
        match record {
            Record::ToolCall {
                name, arguments, ..
            } => file_op(name, arguments),
            Record::Assistant { content, .. } => {
                for block in content {
                    if let lca_protocol::ContentBlock::ToolCall {
                        name, arguments, ..
                    } = block
                    {
                        file_op(name, arguments);
                    }
                }
            }
            _ => {}
        }
    }
    // Earlier compactions accumulate: their lists survive into the
    // next record, so repeated compactions refine the same picture.
    for record in priors {
        if let Record::Compaction {
            read_files,
            modified_files,
            ..
        } = record
        {
            read.extend(read_files.iter().cloned());
            modified.extend(modified_files.iter().cloned());
        }
    }
    for path in &modified {
        read.remove(path);
    }
    (
        read.into_iter().take(MAX_TRACKED_FILES).collect(),
        modified.into_iter().take(MAX_TRACKED_FILES).collect(),
    )
}

/// The file sections appended to a summary when relevant (gh #36
/// phase 3, pi's `<read-files>` / `<modified-files>` shape). Empty
/// lists append nothing: a fileless range reads unchanged.
pub(crate) fn format_file_lists(read: &[String], modified: &[String]) -> String {
    let mut sections = Vec::new();
    if !read.is_empty() {
        sections.push(format!("<read-files>\n{}\n</read-files>", read.join("\n")));
    }
    if !modified.is_empty() {
        sections.push(format!(
            "<modified-files>\n{}\n</modified-files>",
            modified.join("\n")
        ));
    }
    if sections.is_empty() {
        return String::new();
    }
    format!("\n\n{}", sections.join("\n\n"))
}

/// A system-prompt change across compactions (gh #36 phase 3): the
/// latest checkpointed prompt differs from the current one, so the
/// change is recorded as its own `custom` record - the detection,
/// not a migration. `None` means no earlier checkpoint or no change.
pub(crate) fn detect_system_change(records: &[Record], current: &str) -> Option<Record> {
    let (id, previous) = records.iter().rev().find_map(|record| match record {
        Record::Compaction {
            id,
            system_prompt: Some(previous),
            ..
        } => Some((id.clone(), previous.clone())),
        _ => None,
    })?;
    if previous == current {
        return None;
    }
    Some(Record::Custom {
        v: FORMAT_VERSION,
        ts: lca_session::now_ms(),
        id: lca_session::new_record_id(),
        custom_type: SYSTEM_PROMPT_CHANGE_TYPE.to_string(),
        data: serde_json::json!({
            "detail": format!(
                "system prompt changed since compaction {id} ({} to {} chars); \
                 the new compaction checkpoints the current prompt",
                previous.chars().count(),
                current.chars().count(),
            ),
        }),
    })
}

/// The manual trigger behind the `/compact` built-in: compact now,
/// every compactable record, no threshold involved - still through the
/// compaction world only (FR-SESS-5). Synchronous by contract: the
/// caller is the interface's command thread, and `drive_blocking`
/// gives the work a thread of its own.
///
/// Manual compaction keeps nothing past the range (gh #36 phase 3):
/// the record anchors its own id, pi's retain-none shape, so the
/// next plan starts after the entry instead of the session start.
pub fn compact_now(
    store: Arc<SessionStore>,
    session: Session,
    extensions: Arc<ExtensionRegistry>,
    completion_backend: Option<Arc<dyn lca_tools::CompletionBackend>>,
    system_prompt: Option<String>,
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
        let (read_files, modified_files) = accumulate_files(&candidate, &read.records);
        if let Some(change) = system_prompt
            .as_deref()
            .and_then(|current| detect_system_change(&read.records, current))
        {
            // A changed prompt is recorded; a failed write never
            // blocks the compaction the user asked for.
            let _ = store.append(&session, change);
        }
        compact_candidate(
            &store,
            &session,
            &extensions,
            completion_backend.as_ref(),
            candidate,
            CompactMeta {
                first_kept_id: None,
                read_files,
                modified_files,
                system_prompt,
            },
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
            nested: Vec::new(),
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

    // Verifies: gh #36 phase 2 - the previous summary is found for the
    // next compaction (latest compaction record before the range with a
    // non-empty summary), or absent when there is nothing to iterate.
    #[test]
    fn the_previous_summary_is_found_or_absent() {
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
                read_files: Vec::new(),
                modified_files: Vec::new(),
                system_prompt: None,
                summary: "S1".to_string(),
                strategy: "x".to_string(),
                usage: None,
            },
            user("u2"),
            assistant("a2", 100_000),
        ];
        assert_eq!(previous_summary_text(&records, 5), Some("S1".to_string()));
        assert_eq!(previous_summary_text(&records, 0), None, "nothing before");
        let mut empty = records.clone();
        if let Record::Compaction { summary, .. } = &mut empty[2] {
            summary.clear();
        }
        assert_eq!(
            previous_summary_text(&empty, 5),
            None,
            "an empty summary is nothing to iterate"
        );
    }

    // Verifies: gh #36 phase 2 - a span straddling the cut splits at
    // the documented point: the span head and prefix summarize, the
    // tail stays verbatim, and no tool pair splits across the cut.
    #[test]
    fn a_straddling_span_splits_at_the_documented_point() {
        // One span [u1..a2] exceeding the budget; the walk stops mid-span.
        let records = vec![
            user("u1"),
            assistant("a1", 30_000),
            call("c1", "k1"),
            result("r1", "k1"),
            assistant("a2", 100_000),
        ];
        let plan = plan_cut(&records, 5, 20_000, 100_000);
        assert_eq!(plan.start, 0, "no previous compaction");
        assert_eq!(plan.kept_start, 2, "the tail stays verbatim");
        let summarized: Vec<&str> = records[plan.start..plan.kept_start]
            .iter()
            .filter_map(Record::id)
            .collect();
        assert_eq!(
            summarized,
            vec!["u1", "a1"],
            "the span head and prefix summarize"
        );
        let kept: Vec<&str> = records[plan.kept_start..5]
            .iter()
            .filter_map(Record::id)
            .collect();
        assert!(
            kept.contains(&"c1") == kept.contains(&"r1"),
            "no orphan: {kept:?}"
        );
        assert_eq!(plan.first_kept_id, "c1");
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
                read_files: Vec::new(),
                modified_files: Vec::new(),
                system_prompt: None,
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

    fn file_call(id: &str, name: &str, path: Option<&str>) -> Record {
        let arguments = match path {
            Some(path) => format!("{{\"path\": \"{path}\"}}"),
            None => "{}".to_string(),
        };
        Record::ToolCall {
            v: FORMAT_VERSION,
            ts: 1,
            id: id.to_string(),
            call_id: format!("k-{id}"),
            name: name.to_string(),
            arguments,
            source: lca_protocol::ToolSource::Builtin,
        }
    }

    fn compaction_with_files(id: &str, kept: &str, read: &[&str], modified: &[&str]) -> Record {
        Record::Compaction {
            v: FORMAT_VERSION,
            ts: 1,
            id: id.to_string(),
            replaced_from: "u0".to_string(),
            replaced_to: "a0".to_string(),
            first_kept_id: kept.to_string(),
            read_files: read.iter().map(ToString::to_string).collect(),
            modified_files: modified.iter().map(ToString::to_string).collect(),
            system_prompt: None,
            summary: "s".to_string(),
            strategy: "x".to_string(),
            usage: None,
        }
    }

    // Verifies: gh #36 phase 3 - file extraction reads tool calls
    // (records and assistant blocks): reads stay reads, writes and
    // edits count as modified, a file both read and modified counts
    // as modified, and calls without a path never count.
    #[test]
    fn file_extraction_sorts_reads_from_modifications() {
        let candidate = vec![
            file_call("c1", "read", Some("b.md")),
            file_call("c2", "edit", Some("a.rs")),
            file_call("c3", "write", Some("b.md")),
            file_call("c4", "read", None),
            file_call("c5", "bash", Some("run.sh")),
            // Pi tracks read/write/edit only: a grep read never counts.
            file_call("c6", "grep", Some("x")),
            file_call("c7", "read", Some("notes.md")),
        ];
        let (read, modified) = accumulate_files(&candidate, &[]);
        assert_eq!(read, vec!["notes.md".to_string()], "read-only survivors");
        assert_eq!(
            modified,
            vec!["a.rs".to_string(), "b.md".to_string()],
            "edited plus written, sorted"
        );
    }

    // Verifies: gh #36 phase 3 - earlier compactions accumulate: the
    // next record's lists union the candidate's with the priors'.
    #[test]
    fn file_lists_union_with_earlier_compactions() {
        let candidate = vec![file_call("c1", "read", Some("new.md"))];
        let priors = vec![compaction_with_files(
            "cmp0",
            "u1",
            &["old.md"],
            &["main.rs"],
        )];
        let (read, modified) = accumulate_files(&candidate, &priors);
        assert_eq!(read, vec!["new.md".to_string(), "old.md".to_string()]);
        assert_eq!(modified, vec!["main.rs".to_string()]);
    }

    // Verifies: gh #36 phase 3 - the lists stay bounded: past
    // `MAX_TRACKED_FILES` the record keeps the first sorted entries.
    #[test]
    fn file_lists_stay_bounded() {
        let candidate: Vec<Record> = (0..MAX_TRACKED_FILES + 50)
            .map(|n| file_call(&format!("c{n}"), "read", Some(&format!("f{n:04}.md"))))
            .collect();
        let (read, _) = accumulate_files(&candidate, &[]);
        assert_eq!(read.len(), MAX_TRACKED_FILES, "capped");
        assert_eq!(read[0], "f0000.md", "sorted head kept");
    }

    // Verifies: gh #36 phase 3 - the summary sections use pi's
    // `<read-files>` / `<modified-files>` shape, and empty lists
    // append nothing.
    #[test]
    fn file_sections_shape_or_vanish() {
        let shaped = format_file_lists(&["r.md".to_string()], &["m.rs".to_string()]);
        assert_eq!(
            shaped,
            "\n\n<read-files>\nr.md\n</read-files>\n\n<modified-files>\nm.rs\n</modified-files>"
        );
        assert_eq!(
            format_file_lists(&[], &[]),
            "",
            "a fileless range reads unchanged"
        );
    }

    // Verifies: gh #36 phase 3 - the overflow patterns from the #36
    // discussion plus our budgeted cap message trigger recovery;
    // anything else surfaces without compacting.
    #[test]
    fn overflow_errors_match_the_documented_patterns() {
        for message in [
            "generation hit the token cap (max_tokens 13107) before finishing",
            "This model's maximum context length is 200000 tokens, prompt is too long",
            "request exceeds token limit",
            "max_tokens 100 exceeds model maximum",
            "input exceeds the context window",
            "CONTEXT_LENGTH exceeded",
        ] {
            assert!(is_overflow_error(message), "recovers: {message}");
        }
        for message in [
            "model not found",
            "max_tokens must be positive",
            "connection reset by peer",
            "invalid api key",
        ] {
            assert!(!is_overflow_error(message), "surfaces: {message}");
        }
    }

    // Verifies: gh #36 phase 3 - the checkpoint detects a move: an
    // unchanged prompt records nothing, a changed one records the
    // detection, and a first compaction has nothing to compare.
    #[test]
    fn system_change_detects_only_a_move() {
        let plain = vec![user("u1"), assistant("a1", 10)];
        assert_eq!(detect_system_change(&plain, "prompt"), None);
        let mut checked = plain.clone();
        checked.push(compaction_with_files("cmp1", "u1", &[], &[]));
        // No checkpoint on the record: nothing to compare.
        assert_eq!(detect_system_change(&checked, "prompt"), None);
        let mut checkpointed = plain.clone();
        let mut first = compaction_with_files("cmp1", "u1", &[], &[]);
        if let Record::Compaction { system_prompt, .. } = &mut first {
            *system_prompt = Some("prompt A".to_string());
        }
        checkpointed.push(first);
        assert_eq!(
            detect_system_change(&checkpointed, "prompt A"),
            None,
            "unchanged prompt records nothing"
        );
        let change =
            detect_system_change(&checkpointed, "prompt B").expect("a changed prompt is recorded");
        match &change {
            Record::Custom { custom_type, .. } => {
                assert_eq!(custom_type, SYSTEM_PROMPT_CHANGE_TYPE);
            }
            other => panic!("a custom detection record, not {other:?}"),
        }
    }

    // Verifies: gh #36 phase 3 - a retain-none anchor (the record's
    // own id, pi's shape) starts the next range after the entry
    // itself; any other anchor still starts at the kept record.
    #[test]
    fn retain_none_starts_after_the_entry() {
        let records = vec![
            user("u1"),
            assistant("a1", 30_000),
            compaction_with_files("cmp1", "cmp1", &[], &[]),
            user("u2"),
            assistant("a2", 100_000),
        ];
        assert_eq!(
            previous_kept_start(&records, 5),
            3,
            "after the retain-none entry"
        );
    }
}
