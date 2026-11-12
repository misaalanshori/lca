//! Request assembly for the turn loop (FR-CTX-2/3/4, FR-CACHE-5/6, FR-SESS-4).
//!
//! Split out of `turn/mod.rs` under the workspace's 1,200-line file ceiling
//! (gate 11, #548 phase 2). The methods here are the boundary between the
//! session log and one provider request: the tool-set-change record, the
//! first-round compaction check, the transform chain and the skills merge,
//! the three stable-prefix steps (`stable_cap`, `durable_end`,
//! `narrow_stable_prefix`, FR-CACHE-5/6), the request itself, and the
//! protocol refusal for a response that ended on an open call.
//!
//! They read but never mutate the turn's long-lived loop state: no round
//! counters, no nested-completion server, no cancellation flag. `turn_body`
//! stays in `mod.rs` as **the loop**, deliberately one function - its phases
//! share that state across `await` points, so splitting it is a separate and
//! riskier change than moving code (see `mod.rs`).
//!
//! Two call paths needed a new spelling and only that: `super::assemble::{…}`
//! and `super::lock` were written from `turn/mod.rs`, where `super` is the
//! crate root; from a child of `turn` the same items are `crate::assemble` and
//! `crate::lock`. Everything else in the moved bodies is byte-identical.

use lca_protocol::{
    ChatMessage, FORMAT_VERSION, MessageRole, Record, StopReason, TurnEvent, TurnOutcome,
    TurnStatus, Usage,
};
use lca_provider::{CompletionRequest, ProtocolError};
use lca_session::ViewMode;
use lca_tools::ToolExecutor;

use crate::assemble::{COMPACTION_SUMMARY_PREFIX, assemble_with, message_text, stable_fingerprint};
use crate::{Agent, TurnSink};

use super::CallResponse;
use super::execute::tool_search_spec;

impl Agent<'_> {
    /// Assemble one provider request: read the log, compact once on the
    /// first round (FR-SESS-4), build the message list, run the transform
    /// chain and the skills merge, narrow the stable prefix (FR-CACHE-6),
    /// and collect the tool specs and extras.
    #[allow(clippy::result_large_err)]
    pub(super) async fn assemble_request(
        &self,
        input: &str,
        turn_record_id: &str,
        first_round: bool,
        sink: &mut dyn TurnSink,
        active_seen: &mut u64,
    ) -> Result<CompletionRequest, TurnOutcome> {
        // Gh #77's transcript entry: a mid-turn active-set change
        // records before the next model request (pi appends tool and
        // prompt changes at the same boundary), so the log shows
        // what each request declared.
        let revision = self.config.extensions.active_revision();
        if revision != *active_seen {
            *active_seen = revision;
            let active = self.config.extensions.active_tools();
            let discovered = self.config.extensions.discovered_tools();
            if let Err(err) = self.store.append(
                self.session,
                Record::Custom {
                    v: FORMAT_VERSION,
                    ts: lca_session::now_ms(),
                    id: lca_session::new_record_id(),
                    parent: None,
                    custom_type: "tool-set-change".to_string(),
                    data: serde_json::json!({ "active": active, "discovered": discovered }),
                },
            ) {
                return Err(self.fail(
                    StopReason::Error,
                    format!("cannot record the tool change: {err}"),
                ));
            }
        }
        let mut records = match self.store.read_with(self.session, ViewMode::Display) {
            Ok(outcome) => outcome.records,
            Err(err) => {
                return Err(self.fail(StopReason::Error, format!("cannot read the session: {err}")));
            }
        };
        // FR-SESS-4: one threshold check per turn, before the first
        // provider call; a strategy refusal leaves the turn running and
        // lands in the log as an extension event.
        if first_round && self.maybe_compact(&records, turn_record_id, sink).await {
            records = match self.store.read_with(self.session, ViewMode::Display) {
                Ok(outcome) => outcome.records,
                Err(err) => {
                    return Err(
                        self.fail(StopReason::Error, format!("cannot re-read the log: {err}"))
                    );
                }
            };
        }
        let assembled = {
            // Gh #153: attachments pre-read through `tokio::fs` (never
            // a blocking read inside assembly); the resolver below is a
            // map lookup.
            let mut table = std::collections::HashMap::new();
            for hash in crate::assemble::attachment_hashes(&records) {
                if let Some(attachment) =
                    crate::assemble::read_attachment_async(self.store, self.session, &hash).await
                {
                    table.insert(hash, attachment);
                }
            }
            assemble_with(&records, &self.config.system_prompt, &|hash| {
                table.get(hash).cloned()
            })
        };
        // FR-CTX-2: every enabled transform, in installation order, before
        // every provider call; a rejection ends the turn here, with no call
        // (FR-CTX-3) and nothing persisted (FR-CTX-4).
        let transformed = self.config.extensions.transform(assembled.messages).await;
        let messages = match transformed {
            Ok(messages) => messages,
            Err(reason) => {
                sink.on_event(TurnEvent::Error {
                    message: reason.clone(),
                    class: "invalid".to_string(),
                    retryable: false,
                });
                sink.on_event(TurnEvent::TurnEnded {
                    status: TurnStatus::Error,
                    stop_reason: StopReason::Error,
                });
                return Err(TurnOutcome {
                    status: TurnStatus::Error,
                    stop_reason: StopReason::Error,
                    usage: Usage::default(),
                    error: Some(reason),
                });
            }
        };
        // FR-CTX-2 / ADR-0030: the host's own three-source skills merge
        // (project > user > extension resources), appended after the
        // extension transforms as a system message, so the stable cache
        // prefix is untouched by construction. Gh #153: the scan is
        // cached (re-scans only when the trees change, off the runtime
        // threads), so per-round assembly never walks the filesystem.
        let messages = {
            let skills = lca_tools::skills::SkillCache::collect_cached(
                &self.config.skills_cache,
                &self.config.skills_roots,
            )
            .await;
            lca_tools::skills::transform(messages, &skills, self.config.skills_inject_matched)
        };
        let mut messages = messages;
        messages.extend(self.instruction_deltas(first_round).await);
        // Gh #275: the cap is computed on the POST-transform list. A
        // transform that inserts a message before the current turn's user
        // message shifts every index after it, so a cap computed on the
        // pre-transform list pointed at the wrong message (or past the end).
        let stable_cap = self.stable_cap(&messages, input);
        // Gh #275: `sent_stable` records only the durable prefix - through
        // the current turn's user message, plus a compaction summary when
        // one sits after it. A trailing per-turn injection (a matched skill,
        // an instruction delta) is ephemeral under FR-CTX-4: it is never
        // written to the log, so on the next turn a different message sits at
        // that index. Storing it made the next turn read as a divergence and
        // narrow the boundary for no reason.
        let durable_end = self.durable_end(&messages, stable_cap);
        let stable_prefix = self.narrow_stable_prefix(&messages[..durable_end], stable_cap, sink);
        Ok(self.build_request(messages, stable_prefix))
    }

    /// Gh #217's turn-boundary instruction check: re-scan the
    /// startup snapshotted sources on the first round and append any
    /// deltas as trailing system messages (the skills-merge position),
    /// so `messages[0]` stays byte-identical and the stable prefix
    /// advances naturally instead of collapsing.
    pub(super) async fn instruction_deltas(&self, first_round: bool) -> Vec<ChatMessage> {
        if !first_round || self.config.instruction_sources.is_empty() {
            return Vec::new();
        }
        crate::instructions::check_instructions(
            &self.config.instruction_sources,
            &self.config.instruction_baseline,
        )
        .await
    }

    /// The stable-prefix boundary base (FR-CACHE-5): the index of this
    /// turn's own user message, so content sent for the first time this call
    /// is never claimed as cached.
    ///
    /// Gh #275 removed two things from this. It used to run on the
    /// pre-transform list, so an inserted message shifted the answer; and it
    /// used to clamp to `assembled.stable_prefix` whenever a compaction had
    /// ever been seen, which froze the boundary at the summary on every
    /// subsequent turn and left the whole post-compaction conversation
    /// permanently un-cacheable. The current turn's user message is already
    /// the correct upper bound: `narrow_stable_prefix` bounds it further
    /// against what the previous request actually sent, which is the only
    /// claim the provider could have cached.
    pub(super) fn stable_cap(&self, messages: &[ChatMessage], input: &str) -> usize {
        messages
            .iter()
            .rposition(|message| {
                message.role == MessageRole::User && message_text(message) == input
            })
            .unwrap_or(messages.len())
    }

    /// Where the durable part of the outgoing request ends (gh #275).
    ///
    /// Everything through the current turn's user message is in the session
    /// log. On the turn which just compacted, the summary record lands *after*
    /// that user message, so the summary is durable too. Anything past those
    /// two is a per-turn injection that FR-CTX-4 keeps out of the log.
    pub(super) fn durable_end(&self, messages: &[ChatMessage], stable_cap: usize) -> usize {
        let through_input = stable_cap.saturating_add(1).min(messages.len());
        // A summary can only appear after the end of the compaction range,
        // so scanning forward from the user message is enough and cannot
        // mistake an earlier summary for this turn's.
        let summary_end = messages[through_input..]
            .iter()
            .position(|message| {
                message.role == MessageRole::User
                    && message_text(message).starts_with(COMPACTION_SUMMARY_PREFIX)
            })
            .map(|offset| through_input + offset + 1);
        summary_end.map_or(through_input, |end| end.max(through_input))
    }

    /// FR-CACHE-6: content inside the boundary which differs from what
    /// actually went out on the previous call narrows it to end before the
    /// earliest differing message, recorded as an extension event - never a
    /// rejection.
    pub(super) fn narrow_stable_prefix(
        &self,
        messages: &[ChatMessage],
        stable_cap: usize,
        sink: &mut dyn TurnSink,
    ) -> usize {
        let mut stable_prefix = stable_cap.min(messages.len());
        let current: Vec<String> = messages
            .iter()
            .enumerate()
            .map(|(index, message)| {
                if index < stable_prefix {
                    stable_fingerprint(message)
                } else {
                    String::new()
                }
            })
            .collect();
        let previous = crate::lock(&self.config.sent_stable).clone();
        if let Some(previous) = previous {
            // A previously-sent message whose content changed is a real
            // divergence: narrow to end before it and record it (FR-CACHE-6's
            // event is for rewritten content).
            let previously_sent = previous.len();
            let changed = (0..stable_prefix.min(previously_sent))
                .find(|&index| previous[index] != current[index]);
            if let Some(index) = changed {
                let before = stable_prefix;
                stable_prefix = index;
                let detail = format!(
                    "stable region diverged at message {index} (boundary {before} -> {index})"
                );
                sink.on_event(TurnEvent::ExtensionEvent {
                    extension: "host".to_string(),
                    event: "cache-boundary-narrowed".to_string(),
                    detail: detail.clone(),
                });
                let _ = self.store.append(
                    self.session,
                    Record::ExtensionEvent {
                        v: FORMAT_VERSION,
                        ts: lca_session::now_ms(),
                        id: lca_session::new_record_id(),
                        parent: None,
                        extension: "host".to_string(),
                        event: "cache-boundary-narrowed".to_string(),
                        detail,
                    },
                );
            } else if stable_prefix > previously_sent {
                // Messages appended since the last request. The provider
                // cached only that request, so the cacheable prefix ends
                // there; this is normal growth, not a divergence.
                stable_prefix = previously_sent;
            }
        }
        // Store the FULL list actually sent (the divergence basis is content,
        // not the previous claim).
        *crate::lock(&self.config.sent_stable) =
            Some(messages.iter().map(stable_fingerprint).collect());
        stable_prefix
    }

    /// The request's tools, routing identity, and thinking-level hint
    /// (ADR-0023, R1). Extension tools declare through the exposure
    /// filter (gh #77): only active `direct` tools; discovery
    /// (`tool_search`) joins the list exactly while undisclosed
    /// tools exist, so requests without any carry no new bytes.
    pub(super) fn build_request(
        &self,
        messages: Vec<ChatMessage>,
        stable_prefix: usize,
    ) -> CompletionRequest {
        // Gh #67: the run's tool selection filters the executor table
        // through the registry's built-in set (extension tools arrive
        // pre-filtered from `declared_tool_specs`).
        let mut tools = ToolExecutor::specs(self.tools.resolved_shell());
        tools.retain(|spec| self.config.extensions.is_builtin_active(&spec.name));
        tools.extend(self.config.extensions.declared_tool_specs());
        // Gh #67: a pinned selection offers `tool_search` only when the
        // flag named it; otherwise the standing rule holds.
        let offer_search = match self.config.extensions.tool_search_pinned() {
            Some(show) => show,
            None => !self.config.extensions.tool_search("").is_empty(),
        };
        if offer_search {
            tools.push(tool_search_spec());
        }
        let mut extras = std::collections::BTreeMap::new();
        // ADR-0023: the conversation's routing identity travels on every
        // request; the OpenCode Go endpoint requires its header and every
        // other endpoint ignores it.
        extras.insert("session-id".to_string(), self.session.id().to_string());
        // R1: the session's thinking level is a hint the provider honors
        // where meaningful (ADR-0035's settings shape; no ABI change).
        if let Some(effort) = &self.config.reasoning_effort {
            extras.insert("reasoning-effort".to_string(), effort.clone());
            // gh #41: budget-taking vendors (Anthropic-style) get the
            // turn's token budget alongside the level; others ignore it.
            if let Some(budget) = self.config.thinking_budget {
                extras.insert("thinking-budget-tokens".to_string(), budget.to_string());
            }
        }
        CompletionRequest {
            messages,
            tools,
            model: self.config.model.clone(),
            stable_prefix,
            extras,
        }
    }

    /// A response that ended with an open tool call and no completed call is
    /// a protocol failure, not an empty turn.
    #[allow(clippy::result_large_err)]
    pub(super) fn reject_incomplete(
        &self,
        response: &CallResponse,
        sink: &mut dyn TurnSink,
        usage: &Usage,
    ) -> Result<(), TurnOutcome> {
        if response.protocol_errors.is_empty() {
            return Ok(());
        }
        for error in &response.protocol_errors {
            tracing::warn!(%error, "provider protocol error");
        }
        if response.calls.is_empty()
            && response
                .protocol_errors
                .iter()
                .any(|e| matches!(e, ProtocolError::StreamEndedWithOpenCall { .. }))
        {
            let message =
                "the model response ended with an incomplete tool call; the call was discarded"
                    .to_string();
            sink.on_event(TurnEvent::Error {
                message: message.clone(),
                class: "protocol".to_string(),
                retryable: false,
            });
            sink.on_event(TurnEvent::TurnEnded {
                status: TurnStatus::Error,
                stop_reason: StopReason::Error,
            });
            return Err(TurnOutcome {
                status: TurnStatus::Error,
                stop_reason: StopReason::Error,
                usage: usage.clone(),
                error: Some(message),
            });
        }
        Ok(())
    }
}
