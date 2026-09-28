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
pub(super) async fn compact_candidate(
    store: &SessionStore,
    session: &Session,
    extensions: &ExtensionRegistry,
    completion_backend: Option<&Arc<dyn lca_tools::CompletionBackend>>,
    candidate: Vec<Record>,
    sink: &mut dyn TurnSink,
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
    let summary = match strategy.compact(&candidate).await {
        Ok(summary) => summary,
        Err(err) => {
            let detail = format!("compaction strategy `{}` failed: {err}", strategy.name());
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
        return Err(detail);
    }
    Ok(summary)
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
            &mut NullSink,
        )
        .await
    })
}
