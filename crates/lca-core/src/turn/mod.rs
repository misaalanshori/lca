//! The turn loop (S2): one model call per round, tools run sequentially
//! between rounds, cancellation that keeps every record already written.
//! Split out of `lib.rs`; the loop body reads as the calls in order
//! (steer drain, assemble, provider call, persist, tool dispatch).

use lca_protocol::{
    ChatMessage, ContentBlock, FORMAT_VERSION, MessageRole, Record, StopReason, ToolCall,
    ToolSource, TurnEvent, TurnOutcome, TurnStatus, Usage,
};

pub(super) mod execute;
pub(super) mod stream;
use self::execute::{NestedGuard, NestedServer, tool_search_spec};
use lca_provider::{CompletionRequest, ProtocolError};
use lca_session::ViewMode;
use lca_tools::{CancelFlag, ToolExecutor};

use super::assemble::{
    Attachment, assemble_with, message_text, stable_fingerprint, usage_prompt_tokens,
};
use super::compact::compact_candidate;
use super::{Agent, TurnSink};

/// One completion call's accumulated result.
pub(super) struct CallResponse {
    pub(super) text: String,
    pub(super) reasoning: Option<String>,
    pub(super) calls: Vec<ToolCall>,
    pub(super) protocol_errors: Vec<ProtocolError>,
    pub(super) usage: Usage,
}

/// Why one completion call did not produce a response.
pub(super) enum CallFail {
    Cancelled,
    Provider {
        message: String,
        class: String,
        retryable: bool,
    },
}

impl Agent<'_> {
    /// The loop skeleton: cancel check, steer drain, assemble, call,
    /// persist, tool rounds.
    pub(super) async fn turn_body(
        &mut self,
        input: &str,
        queue: Option<lca_protocol::SubmitMode>,
        attachments: &[String],
        sink: &mut dyn TurnSink,
        cancel: &CancelFlag,
    ) -> TurnOutcome {
        let ts = lca_session::now_ms();
        let turn_record_id = lca_session::new_record_id();
        if let Err(err) = self.store.append(
            self.session,
            Record::User {
                v: FORMAT_VERSION,
                ts,
                id: turn_record_id.clone(),
                content: input.to_string(),
                attachments: attachments.to_vec(),
                // ADR-0038's marker: the record says how the message was
                // submitted, wherever it finally lands in the log.
                queue: queue.map(|mode| mode.marker().to_string()),
            },
        ) {
            return self.fail(
                StopReason::Error,
                format!("cannot write to the session log: {err}"),
            );
        }

        // The run boundaries headless `--mode json` and `--mode rpc`
        // reconstruct from (pi's agent/turn/message taxonomy, our names).
        sink.on_event(TurnEvent::TurnStarted);
        sink.on_event(TurnEvent::MessageStarted { role: "user" });
        sink.on_event(TurnEvent::MessageEnded { role: "user" });

        // `pre-turn` fires once, after the user record and before any provider
        // or compaction work (SRDD hook points; `docs/flows.md`).
        self.config.extensions.on_pre_turn().await;

        // Gh #77's nested server lives for exactly this turn: the
        // `tools` import sends here from blocking threads while the
        // turn serves on its own task. The guard clears the slot on
        // every exit, so no request outlives its turn.
        let (nested_tx, nested_rx) = tokio::sync::mpsc::unbounded_channel();
        self.config.extensions.install_nested(nested_tx);
        // Gh #77's registry surface reaches every handle before the
        // first provider call, so the `tools` import serves from any
        // thread the turn's tools run on.
        for handle in self.config.extensions.handles() {
            handle.set_tools_view(self.config.extensions.as_tools_view());
        }
        let _nested_guard = NestedGuard {
            registry: self.config.extensions.clone(),
        };
        let mut nested = NestedServer {
            rx: nested_rx,
            counts: std::collections::HashMap::new(),
            records: std::collections::HashMap::new(),
        };
        // The active set this turn has recorded: the first request
        // carries the opening set implicitly (the declaration
        // itself); only mid-turn changes write entries (gh #77).
        let mut active_seen = self.config.extensions.active_revision();
        // Gh #45's settle summary counts rounds and tool calls.
        let mut tool_calls = 0u32;
        let mut settle_continued = false;

        let mut turn_usage = Usage::default();
        let mut rounds = 0u32;
        // Gh #36 phase 3: one overflow recovery per turn - compact and
        // retry once, then surface if still capped.
        let mut overflow_recovered = false;
        loop {
            if cancel.is_cancelled() {
                return self.cancelled(turn_usage, sink);
            }

            // ADR-0038: steered messages join the turn's input at this
            // model-call boundary (never mid-stream).
            if let Err(outcome) = self.drain_steer(sink) {
                return outcome;
            }

            let request = match self
                .assemble_request(input, &turn_record_id, rounds == 0, sink, &mut active_seen)
                .await
            {
                Ok(request) => request,
                Err(outcome) => return outcome,
            };

            let mut response = match self.provider_call(request, sink, cancel).await {
                Ok(response) => response,
                Err(CallFail::Cancelled) => return self.cancelled(turn_usage, sink),
                Err(CallFail::Provider {
                    message,
                    class,
                    retryable,
                }) => {
                    // Gh #36 phase 3: the error is the signal - a capped
                    // generation compacts and retries (pi's recovery
                    // ordering: the aborted attempt stays visible to
                    // TurnEnded, recovery compacts, the retry runs fresh).
                    // The partial attempt text is not persisted (LCA never
                    // persists failed attempts); the error marks the
                    // boundary. A failed recovery compacts nothing and
                    // retries nothing: the error surfaces as-is.
                    if !overflow_recovered && super::compact::is_overflow_error(&message) {
                        overflow_recovered = true;
                        sink.on_event(TurnEvent::Error {
                            message: message.clone(),
                            class: class.clone(),
                            retryable,
                        });
                        sink.on_event(TurnEvent::TurnEnded {
                            status: TurnStatus::Error,
                            stop_reason: StopReason::Error,
                        });
                        let records = match self.store.read_with(self.session, ViewMode::Display) {
                            Ok(outcome) => outcome.records,
                            Err(_) => {
                                return self.fail(
                                    StopReason::Error,
                                    format!(
                                        "cannot re-read the log after the capped turn: {message}"
                                    ),
                                );
                            }
                        };
                        if self
                            .compact_before_turn(&records, &turn_record_id, sink, "overflow")
                            .await
                        {
                            continue;
                        }
                    }
                    sink.on_event(TurnEvent::Error {
                        message: message.clone(),
                        class: class.clone(),
                        retryable,
                    });
                    sink.on_event(TurnEvent::TurnEnded {
                        status: TurnStatus::Error,
                        stop_reason: StopReason::Error,
                    });
                    return TurnOutcome {
                        status: TurnStatus::Error,
                        stop_reason: StopReason::Error,
                        usage: turn_usage,
                        error: Some(message),
                    };
                }
            };

            // Gh #125: a provider that reports no cost gets the curated
            // table's math for listed models; a reported cost is never
            // overwritten, and unlisted models stay tokens-only.
            if response.usage.cost == 0.0
                && let Some(priced) =
                    super::pricing::table_cost(&self.config.model, &response.usage)
            {
                response.usage.cost = priced;
            }
            accumulate_usage(&mut turn_usage, &response.usage);
            if let Err(outcome) = self.reject_incomplete(&response, sink, &turn_usage) {
                return outcome;
            }
            if let Err(outcome) = self.persist_response(&response, sink).await {
                return outcome;
            }

            if response.calls.is_empty() {
                // Gh #45's actionable settle: `turn_end` handlers run
                // first, then `agent_before_settle` gets the last word.
                // Appends inject as context messages the next request
                // sees; one continuation runs one more request, then the
                // turn settles regardless (a handler cannot loop it).
                if !settle_continued {
                    let mut decision = self
                        .config
                        .extensions
                        .settle_phase(false, rounds, tool_calls, "ok")
                        .await;
                    let late = self
                        .config
                        .extensions
                        .settle_phase(true, rounds, tool_calls, "ok")
                        .await;
                    crate::registry::ExtensionRegistry::merge_append(&mut decision, late.append);
                    decision.continue_once |= late.continue_once;
                    if let Some(append) = decision.append
                        && let Err(err) = self.store.append(
                            self.session,
                            Record::CustomMessage {
                                v: FORMAT_VERSION,
                                ts: lca_session::now_ms(),
                                id: lca_session::new_record_id(),
                                custom_type: "settle-append".to_string(),
                                content: append,
                                display: false,
                                details: None,
                            },
                        )
                    {
                        return self.fail(
                            StopReason::Error,
                            format!("cannot record the settle append: {err}"),
                        );
                    }
                    if decision.continue_once {
                        settle_continued = true;
                        continue;
                    }
                }
                sink.on_event(TurnEvent::TurnEnded {
                    status: TurnStatus::Ok,
                    stop_reason: StopReason::Stop,
                });
                return TurnOutcome {
                    status: TurnStatus::Ok,
                    stop_reason: StopReason::Stop,
                    usage: turn_usage,
                    error: None,
                };
            }

            // FR-CORE-9: bound the tool rounds. `0` (the default since
            // 2026-10-02) means unlimited - the mechanism is unchanged and
            // a configured value still stops the turn here.
            rounds += 1;
            if self.config.max_iterations > 0 && rounds > self.config.max_iterations {
                let message = format!(
                    "iteration limit of {} tool-call rounds reached; send another message to continue",
                    self.config.max_iterations
                );
                sink.on_event(TurnEvent::Error {
                    message: message.clone(),
                    class: "iteration-limit".to_string(),
                    retryable: false,
                });
                sink.on_event(TurnEvent::TurnEnded {
                    status: TurnStatus::Error,
                    stop_reason: StopReason::IterationLimit,
                });
                return TurnOutcome {
                    status: TurnStatus::Error,
                    stop_reason: StopReason::IterationLimit,
                    usage: turn_usage,
                    error: Some(message),
                };
            }

            // Sequential execution inside one turn (FR-CONC-2).
            for call in &response.calls {
                tool_calls += 1;
                if let Err(outcome) = self.run_tool_call(call, sink, cancel, &mut nested).await {
                    return outcome;
                }
                if cancel.is_cancelled() {
                    return self.cancelled(turn_usage, sink);
                }
            }
        }
    }

    /// The cancellation exit: the turn ends `Ok` with `Cancelled`, keeping
    /// every record already written.
    fn cancelled(&self, usage: Usage, sink: &mut dyn TurnSink) -> TurnOutcome {
        sink.on_event(TurnEvent::TurnEnded {
            status: TurnStatus::Ok,
            stop_reason: StopReason::Cancelled,
        });
        TurnOutcome {
            status: TurnStatus::Ok,
            stop_reason: StopReason::Cancelled,
            usage,
            error: None,
        }
    }

    /// Drain the steering queue into user records at this boundary
    /// (ADR-0038): each steered message extends the message list the way any
    /// user message does, so the stable cache prefix keeps its value.
    #[allow(clippy::result_large_err)]
    fn drain_steer(&self, sink: &mut dyn TurnSink) -> Result<(), TurnOutcome> {
        let steered: Vec<lca_protocol::QueuedMessage> = {
            let mut queue = super::lock(&self.config.steer);
            std::mem::take(&mut *queue)
        };
        for message in steered {
            let marker = message.mode.marker().to_string();
            if let Err(err) = self.store.append(
                self.session,
                Record::User {
                    v: FORMAT_VERSION,
                    ts: lca_session::now_ms(),
                    id: lca_session::new_record_id(),
                    content: message.text.clone(),
                    attachments: Vec::new(),
                    queue: Some(marker.clone()),
                },
            ) {
                return Err(self.fail(
                    StopReason::Error,
                    format!("cannot write to the session log: {err}"),
                ));
            }
            sink.on_event(TurnEvent::UserInjected {
                text: message.text,
                mode: marker,
            });
        }
        Ok(())
    }

    /// Assemble one provider request: read the log, compact once on the
    /// first round (FR-SESS-4), build the message list, run the transform
    /// chain and the skills merge, narrow the stable prefix (FR-CACHE-6),
    /// and collect the tool specs and extras.
    #[allow(clippy::result_large_err)]
    async fn assemble_request(
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
            if let Err(err) = self.store.append(
                self.session,
                Record::Custom {
                    v: FORMAT_VERSION,
                    ts: lca_session::now_ms(),
                    id: lca_session::new_record_id(),
                    custom_type: "tool-set-change".to_string(),
                    data: serde_json::json!({ "active": active }),
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
        let assembled = assemble_with(&records, &self.config.system_prompt, &|hash| {
            let path = self.store.attachment_path(self.session, hash)?;
            let bytes = std::fs::read(path).ok()?;
            let media_type = lca_protocol::sniff_image_media_type(&bytes)?;
            Some(Attachment {
                media_type: media_type.to_string(),
                bytes,
            })
        });
        let stable_cap = self.stable_cap(&assembled, input);
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
        // prefix is untouched by construction.
        let messages = {
            let skills = lca_tools::skills::collect(&self.config.skills_roots);
            lca_tools::skills::transform(messages, &skills, self.config.skills_inject_matched)
        };
        let stable_prefix = self.narrow_stable_prefix(&messages, stable_cap, sink);
        Ok(self.build_request(messages, stable_prefix))
    }

    /// The stable-prefix boundary base (FR-CACHE-5): everything through the
    /// most recent compaction record, never reaching this turn's own user
    /// message - content sent for the first time this call cannot be
    /// claimed as cached.
    fn stable_cap(&self, assembled: &super::assemble::Assembled, input: &str) -> usize {
        let mut stable_cap = if assembled.compaction_seen {
            assembled.stable_prefix
        } else {
            usize::MAX
        };
        stable_cap = stable_cap.min(assembled.messages.len());
        if let Some(index) = assembled.messages.iter().rposition(|message| {
            message.role == MessageRole::User && message_text(message) == input
        }) {
            stable_cap = stable_cap.min(index);
        }
        stable_cap
    }

    /// FR-CACHE-6: content inside the boundary which differs from what
    /// actually went out on the previous call narrows it to end before the
    /// earliest differing message, recorded as an extension event - never a
    /// rejection.
    fn narrow_stable_prefix(
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
        let previous = super::lock(&self.config.sent_stable).clone();
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
        *super::lock(&self.config.sent_stable) =
            Some(messages.iter().map(stable_fingerprint).collect());
        stable_prefix
    }

    /// The request's tools, routing identity, and thinking-level hint
    /// (ADR-0023, R1). Extension tools declare through the exposure
    /// filter (gh #77): only active `direct` tools; discovery
    /// (`tool_search`) joins the list exactly while undisclosed
    /// tools exist, so requests without any carry no new bytes.
    fn build_request(&self, messages: Vec<ChatMessage>, stable_prefix: usize) -> CompletionRequest {
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
    fn reject_incomplete(
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

    /// Persist the response: one assistant record plus one record per tool
    /// call, then surface the text and usage (docs/session-log-format.md).
    #[allow(clippy::result_large_err)]
    async fn persist_response(
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
        for call in &response.calls {
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
                content: blocks,
                reasoning: response.reasoning.clone(),
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
        for (index, call) in response.calls.iter().enumerate() {
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
    async fn maybe_compact(
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
        let reserve = super::compact::compaction_reserve(
            threshold,
            self.config.compaction_reserve_tokens,
            window,
        );
        if !super::compact::compaction_fires(last_prompt, window, reserve) {
            return false;
        }
        self.compact_before_turn(records, turn_record_id, sink, "threshold")
            .await
    }

    /// The shared compaction call behind threshold and overflow
    /// recovery (gh #36 phase 3): no threshold of its own, so the
    /// recovery path compacts on the error alone. Still needs a
    /// strategy, and still refuses a range too small to summarize.
    /// Returns whether a record was written (the caller re-reads).
    async fn compact_before_turn(
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
        let plan = super::compact::plan_cut(
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
        if let Some(summary) = super::compact::previous_summary_text(records, cut) {
            candidate.insert(0, super::compact::previous_summary_marker(&summary));
        }
        // Gh #36 phase 3: cumulative file lists, the prompt
        // checkpoint, and a recorded detection when the prompt moved
        // since the last checkpoint. A failed detection write never
        // blocks the compaction itself.
        let (read_files, modified_files) =
            super::compact::accumulate_files(&candidate, &records[..cut]);
        if let Some(change) =
            super::compact::detect_system_change(records, &self.config.system_prompt)
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
            super::compact::CompactMeta {
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

/// The window the compaction ratio uses: the model's published window when
/// one is known, else a conservative estimate.
///
/// An endpoint that publishes no context window (`opencode-go`'s `/models`
/// carries none) would otherwise never cross a threshold, and a long session
/// would grow until the provider rejects it. The estimate is a safety net,
/// not a claim: the footer still shows `ctx ?` when the window is unknown,
/// and `OPENAI_CONTEXT_WINDOW` (or a provider that reports one) overrides it.
/// ponytail: a fixed 128k; a per-model catalog is the accurate upgrade.
const FALLBACK_CONTEXT_WINDOW: u32 = 128_000;

/// The effective window, shared with the front ends (gh #36 phase 1):
/// they derive the summarization budget from the same window the
/// trigger uses.
pub fn effective_context_window(configured: u32) -> u32 {
    if configured > 0 {
        configured
    } else {
        FALLBACK_CONTEXT_WINDOW
    }
}

fn accumulate_usage(total: &mut Usage, per_call: &Usage) {
    total.input = total.input.saturating_add(per_call.input);
    total.output = total.output.saturating_add(per_call.output);
    total.cache_read = total.cache_read.saturating_add(per_call.cache_read);
    total.cache_write = total.cache_write.saturating_add(per_call.cache_write);
    total.cache_write_1h = total.cache_write_1h.saturating_add(per_call.cache_write_1h);
    total.cost += per_call.cost;
}

#[cfg(test)]
mod window_tests {
    use super::*;

    // A published window is used as-is; an unknown one falls back to a
    // conservative estimate so compaction still runs.
    #[test]
    fn an_unknown_window_still_compacts() {
        assert_eq!(effective_context_window(1_000_000), 1_000_000);
        assert_eq!(effective_context_window(0), FALLBACK_CONTEXT_WINDOW);
    }
}
