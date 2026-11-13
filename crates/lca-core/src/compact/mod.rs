//! Compaction (S2, FR-SESS-4): the trigger's vocabulary (when it fires, why an
//! overflow compacts), the strategy call shared by the threshold path and the
//! manual `/compact` trigger, and the manual trigger itself.
//!
//! The cut-point, iterative-summary and file-tracking rules live in the sibling
//! modules below; `docs/compaction.md` is the behaviour they implement.

use std::sync::Arc;

use lca_protocol::{FORMAT_VERSION, Record, TurnEvent};
use lca_session::{Session, SessionStore, ViewMode};

use super::{NullSink, TurnSink, drive_blocking};
use crate::registry::ExtensionRegistry;

// Every item keeps the visibility it had in the flat file. `crate::compact::x` is
// how `turn/persist.rs` and `lib.rs` name these, so the directory's root carries
// them: a `pub(crate) use` for the ones the crate shares, a `pub use` for the two
// `lib.rs` re-exports. A move that forced callers to learn a submodule path would
// be a path change, which this cycle does not allow.
pub use self::files::{MAX_TRACKED_FILES, SYSTEM_PROMPT_CHANGE_TYPE};
// Visible crate-internally under the path they had: nothing outside `lca-core`
// names any of these, and each stays exactly as reachable as it was.
// The names other modules of this crate already spell as `crate::compact::x`
// (`turn/persist.rs`) keep that path; the rest are used only here.
pub(crate) use self::cut::plan_cut;
pub(crate) use self::files::{
    CompactMeta, accumulate_files, detect_system_change, format_file_lists,
};
pub(crate) use self::summary::{previous_summary_marker, previous_summary_text};

// What `mod.rs` itself calls, in scope only (not re-exported). `cut_tests.rs`'s
// `use super::*` sees all of these, which is how the test module keeps working
// from a `#[path]` child.
use self::summary::is_previous_summary_marker;

mod cut;
mod files;
mod summary;

#[cfg(test)]
#[path = "cut_tests.rs"]
mod cut_tests;

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
                    parent: None,
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
            parent: None,
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
/// `token cap` message. Gh #52 adds pi's per-vendor catalog (z.ai CN
/// and siblings); the non-overflow exclusions keep throttling and
/// rate-limit text on the retry path, never the compact path.
/// Anything else (validation, auth, transport) surfaces without
/// compacting.
pub(crate) fn is_overflow_error(message: &str) -> bool {
    let lower = message.to_lowercase();
    // Throttling is not overflow, even when it names tokens (pi's
    // non-overflow exclusions; a Bedrock-style wait-please stays a
    // retry, not a compaction).
    if lower.contains("rate limit")
        || lower.contains("too many requests")
        || lower.contains("throttl")
    {
        return false;
    }
    lower.contains("token cap")
        || lower.contains("prompt is too long")
        || lower.contains("prompt too long")
        || lower.contains("prompt exceeds max length")
        || lower.contains("exceeds token limit")
        || lower.contains("token limit exceeded")
        || lower.contains("too many tokens")
        || lower.contains("context window")
        || lower.contains("context length")
        || lower.contains("context_length")
        || lower.contains("model_context_window_exceeded")
        || lower.contains("request_too_large")
        || lower.contains("exceeds the context window")
        || lower.contains("maximum context length")
        || lower.contains("input is too long")
        || lower.contains("range of input length should be")
        || (lower.contains("max_tokens") && lower.contains("exceed"))
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
