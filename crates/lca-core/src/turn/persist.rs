//! Persistence and compaction for the turn loop (FR-SESS-4, docs/session-log-format.md).
//!
//! Split out of `turn/mod.rs` under the workspace's 1,200-line file ceiling
//! (gate 11, #548 phase 2). Three seams:
//!
//! - `persist_response` writes the assistant record, the tool-call records and
//!   the `meta.json` model line, then surfaces the text and usage;
//! - `maybe_compact` is the threshold check (FR-SESS-4) and decides whether a
//!   record was written, so the caller re-reads;
//! - `compact_before_turn` is the call both the threshold path and the
//!   overflow-recovery path share.
//!
//! `turn_body` in `mod.rs` stays **the loop**, deliberately one function: its
//! phases share long-lived state across `await` points, so splitting the loop
//! itself is a separate, riskier change than moving these boundaries out.

//! One spelling changed and only that: `crate::compact::{…}` was written from
//! `turn/mod.rs`, where `super` is the crate root; from a child of `turn` the
//! same items are `crate::compact` and `crate::assemble`. The bodies are
//! otherwise byte-identical.

use lca_protocol::{
    ContentBlock, FORMAT_VERSION, Record, StopReason, ToolSource, TurnEvent, TurnOutcome,
};

use crate::assemble::usage_prompt_tokens;
use crate::compact::compact_candidate;
use crate::{Agent, TurnSink};

use super::CallResponse;
use super::effective_context_window;

impl Agent<'_> {
    /// Persist the response: one assistant record plus one record per tool
    /// call, then surface the text and usage (docs/session-log-format.md).
    #[allow(clippy::result_large_err)]
    pub(super) async fn persist_response(
        &self,
        response: &CallResponse,
        sink: &mut dyn TurnSink,
    ) -> Result<(), TurnOutcome> {
        let mut blocks: Vec<ContentBlock> = Vec::new();
        if !response.text.is_empty() {
            blocks.push(ContentBlock::Text {
                text: response.text.clone(),
            });
        }
        // Gh #390 gap 1: a **truncated** response's calls are not recorded.
        // Their arguments are the prefix the model managed to stream, so the
        // JSON is incomplete; persisting one would put a half-written call in the
        // session log, where a resume would replay it and offer to run it. The
        // text is kept - that is the usable part - and the turn reports `Length`
        // so the user knows something was cut.
        for call in response.calls.iter().filter(|_| !response.truncated) {
            blocks.push(ContentBlock::ToolCall {
                call_id: call.call_id.clone(),
                name: call.name.clone(),
                arguments: call.arguments.clone(),
            });
        }
        let ts = lca_session::now_ms();
        let assistant_id = lca_session::new_record_id();
        if let Err(err) = self.store.append(
            self.session,
            Record::Assistant {
                v: FORMAT_VERSION,
                ts,
                id: assistant_id.clone(),
                parent: None,
                content: blocks,
                reasoning: response.reasoning.clone(),
                // gh #41: the signature the model issued (if any) and
                // the level it ran at persist for replay; compaction
                // carries both (append-only markers never rewrite).
                reasoning_signature: response.signature.clone(),
                provider_thinking_level: self.config.reasoning_effort.clone(),
                model: Some(self.config.model.clone()),
                provider: Some(self.config.provider.clone()),
                usage: Some(response.usage.clone()),
            },
        ) {
            return Err(self.fail(
                StopReason::Error,
                format!("cannot write to the session log: {err}"),
            ));
        }
        // Gh #45's `message_end` fires for the finalized assistant
        // message; a replacement lands as an append-only edit (never
        // a rewrite), keeping role and tool linkage.
        if !response.text.is_empty()
            && let Some(replacement) = self
                .config
                .extensions
                .message_end_replacement("assistant", &response.text)
                .await
        {
            let _ = self.store.append(
                self.session,
                Record::ContextEdit {
                    v: FORMAT_VERSION,
                    ts: lca_session::now_ms(),
                    id: lca_session::new_record_id(),
                    parent: None,
                    target_id: assistant_id.clone(),
                    replacement: Some(replacement),
                },
            );
        }
        // The doc's promise (session-log-format §meta.json): meta carries
        // the model and provider last used. Written here, where both are
        // known; `record_model_used` skips the write when nothing changed.
        if let Err(err) =
            self.store
                .record_model_used(self.session, &self.config.provider, &self.config.model)
        {
            return Err(self.fail(
                StopReason::Error,
                format!("cannot update the session metadata: {err}"),
            ));
        }
        // Gh #390 gap 1: **no `ToolCall` record for a truncated response.**
        //
        // The blocks above already omit them, but this loop writes the records the
        // resume path replays - so filtering only the blocks would leave the
        // half-written call in the log, and a resume would offer to run arguments
        // that are an invalid JSON prefix. Two writers, one rule; the first version
        // of this fix filtered one and a row caught the other, which is why the
        // guard below reads the *records* rather than the assistant block.
        for (index, call) in response
            .calls
            .iter()
            .enumerate()
            .filter(|_| !response.truncated)
        {
            // Gh #128: the record names who owns the tool - an
            // extension-registered name records `Extension`, the same
            // `tool_owner` question dispatch asks below.
            let source = if self.config.extensions.tool_owner(&call.name).is_some() {
                ToolSource::Extension
            } else {
                ToolSource::Builtin
            };
            if let Err(err) = self.store.append(
                self.session,
                Record::ToolCall {
                    v: FORMAT_VERSION,
                    ts,
                    id: format!("{assistant_id}-call{index}"),
                    parent: None,
                    call_id: call.call_id.clone(),
                    name: call.name.clone(),
                    arguments: call.arguments.clone(),
                    source,
                },
            ) {
                return Err(self.fail(
                    StopReason::Error,
                    format!("cannot write to the session log: {err}"),
                ));
            }
        }
        if !response.text.is_empty() {
            sink.on_event(TurnEvent::AssistantText(response.text.clone()));
        }
        sink.on_event(TurnEvent::MessageEnded { role: "assistant" });
        sink.on_event(TurnEvent::Usage(response.usage.clone()));
        Ok(())
    }

    /// FR-SESS-4's budget check, then the shared compaction call.
    /// Returns whether a record was written (the caller re-reads).
    pub(super) async fn maybe_compact(
        &self,
        records: &[Record],
        turn_record_id: &str,
        sink: &mut dyn TurnSink,
    ) -> bool {
        // gh #36 phase 1: disabled means no compaction and no error.
        if !self.config.compaction_enabled {
            return false;
        }
        let threshold = self.config.compaction_threshold;
        if threshold <= 0.0 {
            return false;
        }
        let window = effective_context_window(self.config.model_context_window);
        let last_prompt = records.iter().rev().find_map(|record| match record {
            Record::Assistant {
                usage: Some(usage), ..
            } => Some(usage_prompt_tokens(usage)),
            _ => None,
        });
        let Some(last_prompt) = last_prompt else {
            return false;
        };
        let reserve = crate::compact::compaction_reserve(
            threshold,
            self.config.compaction_reserve_tokens,
            window,
        );
        if !crate::compact::compaction_fires(last_prompt, window, reserve) {
            return false;
        }
        // Gh #258: the trigger (reason + distance-to-threshold
        // inputs, never prompt text).
        lca_debug::core::event(
            "compact.trigger",
            &format!(
                "reason=threshold prompt_tokens={last_prompt} window={window} reserve={reserve}"
            ),
        );
        self.compact_before_turn(records, turn_record_id, sink, "threshold")
            .await
    }

    /// The shared compaction call behind threshold and overflow
    /// recovery (gh #36 phase 3): no threshold of its own, so the
    /// recovery path compacts on the error alone. Still needs a
    /// strategy, and still refuses a range too small to summarize.
    /// Returns whether a record was written (the caller re-reads).
    pub(super) async fn compact_before_turn(
        &self,
        records: &[Record],
        turn_record_id: &str,
        sink: &mut dyn TurnSink,
        reason: &str,
    ) -> bool {
        if !self.config.compaction_enabled {
            return false;
        }
        if self.config.extensions.compaction_strategy().is_none() {
            return false;
        }
        // Gh #45's veto runs before any compaction work: a deny
        // cancels this compaction, and the cancellation observes as
        // a failure with no error (the veto, not a breakdown).
        if let Err(veto) = self.config.extensions.session_before_compact(reason).await {
            // The veto is the operator-visible outcome (not a silent
            // skip): the failure hook observes the cancellation, and
            // the event names the refusing reason.
            sink.on_event(TurnEvent::Error {
                message: format!("compaction vetoed: {veto}"),
                class: "vetoed".to_string(),
                retryable: false,
            });
            self.config
                .extensions
                .session_compact_failed(reason, None)
                .await;
            return false;
        }
        // The candidate range ends at this turn's own user record; the
        // cut planner holds the keep-recent window verbatim, anchors
        // the kept boundary, and never splits a tool pair.
        let cut = records
            .iter()
            .position(|record| record.id() == Some(turn_record_id))
            .unwrap_or(records.len());
        let last_prompt = records.iter().rev().find_map(|record| match record {
            Record::Assistant {
                usage: Some(usage), ..
            } => Some(usage_prompt_tokens(usage)),
            _ => None,
        });
        let plan = crate::compact::plan_cut(
            records,
            cut,
            self.config.compaction_keep_recent_tokens,
            last_prompt.unwrap_or(0),
        );
        let mut candidate: Vec<Record> = records[plan.start..plan.kept_start]
            .iter()
            .filter(|record| record.id().is_some())
            .cloned()
            .collect();
        if candidate.len() < 2 {
            // Nothing worth replacing: a session-start-plus-one-message
            // range would trade the whole conversation for a line.
            return false;
        }
        // Gh #36 phase 2: the latest summary rides in-band at the
        // head, so the strategy refines instead of restarting. The
        // range computation skips it; a first compaction is unchanged.
        if let Some(summary) = crate::compact::previous_summary_text(records, cut) {
            candidate.insert(0, crate::compact::previous_summary_marker(&summary));
        }
        // Gh #36 phase 3: cumulative file lists, the prompt
        // checkpoint, and a recorded detection when the prompt moved
        // since the last checkpoint. A failed detection write never
        // blocks the compaction itself.
        let (read_files, modified_files) =
            crate::compact::accumulate_files(&candidate, &records[..cut]);
        if let Some(change) =
            crate::compact::detect_system_change(records, &self.config.system_prompt)
        {
            let _ = self.store.append(self.session, change);
        }
        // Gh #45's failure observation: a strategy breakdown reports
        // with its error; the veto path above already reported.
        let ok = compact_candidate(
            self.store,
            self.session,
            &self.config.extensions,
            self.config.completion_backend.as_ref(),
            candidate,
            crate::compact::CompactMeta {
                first_kept_id: Some(plan.first_kept_id),
                read_files,
                modified_files,
                system_prompt: Some(self.config.system_prompt.clone()),
            },
            sink,
            reason,
        )
        .await
        .is_ok();
        if !ok {
            self.config
                .extensions
                .session_compact_failed(reason, Some("the compaction strategy failed"))
                .await;
        }
        ok
    }
}
