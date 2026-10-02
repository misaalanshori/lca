//! The turn loop (S2): one model call per round, tools run sequentially
//! between rounds, cancellation that keeps every record already written.
//! Split out of `lib.rs`; the loop body reads as the calls in order
//! (steer drain, assemble, provider call, persist, tool dispatch).

use std::sync::Arc;

use lca_protocol::{
    ChatMessage, ContentBlock, DispatchError, FORMAT_VERSION, HookAction, MessageRole, Record,
    StopReason, StreamEvent, ToolCall, ToolResult, ToolSource, TurnEvent, TurnOutcome, TurnStatus,
    Usage,
};
use lca_provider::{CompletionRequest, ProtocolError, ToolCallAccumulator};
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

        // `pre-turn` fires once, after the user record and before any provider
        // or compaction work (SRDD hook points; `docs/flows.md`).
        self.config.extensions.on_pre_turn().await;

        let mut turn_usage = Usage::default();
        let mut rounds = 0u32;
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
                .assemble_request(input, &turn_record_id, rounds == 0, sink)
                .await
            {
                Ok(request) => request,
                Err(outcome) => return outcome,
            };

            let response = match self.provider_call(request, sink, cancel).await {
                Ok(response) => response,
                Err(CallFail::Cancelled) => return self.cancelled(turn_usage, sink),
                Err(CallFail::Provider {
                    message,
                    class,
                    retryable,
                }) => {
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

            accumulate_usage(&mut turn_usage, &response.usage);
            if let Err(outcome) = self.reject_incomplete(&response, sink, &turn_usage) {
                return outcome;
            }
            if let Err(outcome) = self.persist_response(&response, sink) {
                return outcome;
            }

            if response.calls.is_empty() {
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
                if let Err(outcome) = self.run_tool_call(call, sink, cancel).await {
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
    ) -> Result<CompletionRequest, TurnOutcome> {
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
            let skills = crate::skills::collect(&self.config.skills_roots);
            crate::skills::transform(messages, &skills)
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
    /// (ADR-0023, R1).
    fn build_request(&self, messages: Vec<ChatMessage>, stable_prefix: usize) -> CompletionRequest {
        let mut tools = ToolExecutor::specs(self.tools.resolved_shell());
        tools.extend(self.config.extensions.tool_specs());
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
    fn persist_response(
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
            if let Err(err) = self.store.append(
                self.session,
                Record::ToolCall {
                    v: FORMAT_VERSION,
                    ts,
                    id: format!("{assistant_id}-call{index}"),
                    call_id: call.call_id.clone(),
                    name: call.name.clone(),
                    arguments: call.arguments.clone(),
                    source: ToolSource::Builtin,
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
        sink.on_event(TurnEvent::Usage(response.usage.clone()));
        Ok(())
    }

    /// One tool call: pre-tool hooks first (FR-CORE-10 — a hook denial
    /// ends the call without ever prompting), then the permission layer
    /// on whatever call survives (a replaced call passes through like
    /// any other and is not re-hooked), then execution through the
    /// dispatch table or the built-in table. The result record, the
    /// sink event, and the `post-tool-use` hook all belong here so no
    /// caller can forget one.
    // TurnOutcome grew with Usage's cost buckets past clippy's preferred
    // Err size; boxing it would ripple through every caller for a lint.
    #[allow(clippy::result_large_err)]
    async fn run_tool_call(
        &mut self,
        call: &ToolCall,
        sink: &mut dyn TurnSink,
        cancel: &CancelFlag,
    ) -> Result<(), TurnOutcome> {
        let registry = self.config.extensions.clone();
        let mut hook_errors: Vec<(String, String)> = Vec::new();
        let mut effective = call.clone();
        let action = {
            let mut observe = |handle: &Arc<dyn lca_ext_abi::ExtensionDispatch>,
                               err: &DispatchError| {
                hook_errors.push((handle.name().to_string(), err.to_string()));
            };
            registry.pre_tool_use(&effective, &mut observe).await
        };
        for (extension, detail) in hook_errors {
            if let Err(err) = self.record_extension_event(&extension, "error", &detail, sink) {
                return Err(self.fail(StopReason::Error, err));
            }
        }

        // The card exists from the moment the model *asks*: pi builds tool
        // cards while the call is still streaming, and the session log has
        // already recorded the request (`Record::ToolCall` is written with
        // the response, before any permission). Emitting the start here is
        // what makes a denied, hook-refused, or schema-invalid call visible
        // at all - it settles into the same card instead of leaving the
        // transcript silent - and it keeps the `tool-call` envelope always
        // preceding its `tool-result` (docs/headless.md).
        sink.on_event(TurnEvent::ToolStarted(effective.clone()));

        let result = match action {
            HookAction::Deny(reason) => {
                // No permission prompt: the hook already answered
                // (FR-CORE-10).
                ToolResult::denied(effective.call_id.clone(), reason)
            }
            HookAction::Replace(replacement) => {
                effective = replacement;
                self.execute_after_permission(&effective, &registry, sink, cancel)
                    .await?
            }
            HookAction::Allow => {
                self.execute_after_permission(&effective, &registry, sink, cancel)
                    .await?
            }
        };

        if let Err(err) = self.store.append(
            self.session,
            Record::ToolResult {
                v: FORMAT_VERSION,
                ts: lca_session::now_ms(),
                id: lca_session::new_record_id(),
                call_id: result.call_id.clone(),
                status: result.status,
                content: Some(result.content.clone()),
                attachment: result.extras.get("attachment").cloned(),
                truncated: result.truncated,
            },
        ) {
            return Err(self.fail(
                StopReason::Error,
                format!("cannot write to the session log: {err}"),
            ));
        }
        sink.on_event(TurnEvent::ToolFinished(result.clone()));
        registry.on_post_tool_use(&effective, &result).await;
        Ok(())
    }

    /// The permission layer plus execution (FR-TOOL-3's path), shared by
    /// allow and replace.
    #[allow(clippy::result_large_err)]
    async fn execute_after_permission(
        &mut self,
        call: &ToolCall,
        registry: &Arc<crate::registry::ExtensionRegistry>,
        sink: &mut dyn TurnSink,
        cancel: &CancelFlag,
    ) -> Result<ToolResult, TurnOutcome> {
        // Phase 2 seam note: hooks have already run by the time this is
        // called (FR-CORE-10: hook before permission).
        //
        // Validate the arguments against the schema the model saw before the
        // tool runs (extension authoring guide); an invalid call never reaches
        // the tool. Extension schemas come from the registry, built-ins from
        // the executor's own table.
        let schema = registry
            .tool_schema(&call.name)
            .map(|spec| spec.parameters.clone())
            .or_else(|| {
                ToolExecutor::specs(self.tools.resolved_shell())
                    .into_iter()
                    .find(|spec| spec.name == call.name)
                    .map(|spec| spec.parameters)
            });
        if let Some(schema) = schema
            && let Err(reason) = lca_provider::validate_against_schema(&schema, &call.arguments)
        {
            return Ok(ToolResult::error(
                call.call_id.clone(),
                format!("invalid arguments for `{}`: {reason}", call.name),
            ));
        }
        if let Some(action) = self.tools.required_permission(call) {
            match self.authorize(call, &action) {
                Ok(None) => {}
                Ok(Some(denied)) => return Ok(denied),
                Err(outcome) => return Err(outcome),
            }
        }

        // Extension tool or built-in: one dispatch table, no mode
        // branching at this call site beyond asking who owns the name
        // (FR-EXT-6 lives in the registry's single trait).
        if let Some(handle) = registry.tool_owner(&call.name).cloned() {
            let executed = handle.execute_tool(call).await;
            return Ok(match executed {
                Ok(result) => result,
                Err(err) => {
                    let event = match err {
                        DispatchError::Disabled => "disabled",
                        _ => "error",
                    };
                    if let Err(write_err) =
                        self.record_extension_event(handle.name(), event, &err.to_string(), sink)
                    {
                        return Err(self.fail(StopReason::Error, write_err));
                    }
                    ToolResult::error(call.call_id.clone(), err.to_string())
                }
            });
        }

        let cancel_for_tool = cancel.clone();
        let mut sink_chunk = |chunk: &[u8]| {
            sink.on_event(TurnEvent::ToolOutputChunk {
                call_id: call.call_id.clone(),
                chunk: String::from_utf8_lossy(chunk).into_owned(),
            });
        };
        let result = self
            .tools
            .execute(call, &mut sink_chunk, &cancel_for_tool)
            .await;
        Ok(result)
    }

    /// The permission check for a gated tool: lock the shared store for the
    /// authorize call only, record the decision, and report a denial as a
    /// denied result. `Ok(None)` means proceed; `Ok(Some(result))` is the
    /// denial to return.
    #[allow(clippy::result_large_err)]
    fn authorize(
        &mut self,
        call: &ToolCall,
        action: &lca_permissions::Action,
    ) -> Result<Option<ToolResult>, TurnOutcome> {
        // The grant store is shared with every capability engine and the
        // login flow, so it is locked for the authorize call only, never
        // for the whole turn: an extension's own permission check during
        // this turn re-enters through the same Arc.
        let grants = self.grants.clone();
        let mut guard = match grants.lock() {
            Ok(guard) => guard,
            Err(_) => {
                return Err(self.fail(
                    StopReason::Error,
                    "permission store lock is poisoned".to_string(),
                ));
            }
        };
        let outcome = match lca_permissions::authorize(
            &mut guard,
            self.tools.workspace(),
            action,
            self.proposals,
            self.prompt,
        ) {
            Ok(outcome) => outcome,
            Err(err) => {
                return Err(self.fail(StopReason::Error, format!("permission store error: {err}")));
            }
        };
        // A prompted answer and a yolo answer both belong in the log; a
        // rule denial is recorded too (ADR-0042: approve everything must
        // never mean forget everything).
        if outcome.prompted || outcome.denied_by_rule || outcome.yolo {
            let record = Record::Permission {
                v: FORMAT_VERSION,
                ts: lca_session::now_ms(),
                id: lca_session::new_record_id(),
                action: action.display(),
                decision: if outcome.stored_pattern.is_some() {
                    lca_protocol::PermissionDecision::Always
                } else if outcome.allowed {
                    lca_protocol::PermissionDecision::Once
                } else {
                    lca_protocol::PermissionDecision::Denied
                },
                pattern: outcome.stored_pattern.clone(),
            };
            if let Err(err) = self.store.append(self.session, record) {
                tracing::error!(%err, "cannot record permission decision");
            }
        }
        if !outcome.allowed {
            let reason = if outcome.denied_by_rule {
                format!("A permission rule denied this action: {}", action.display())
            } else {
                format!("The user denied this action: {}", action.display())
            };
            return Ok(Some(ToolResult::denied(call.call_id.clone(), reason)));
        }
        Ok(None)
    }

    /// FR-SESS-4's threshold check, then the shared compaction call.
    /// Returns whether a record was written (the caller re-reads).
    async fn maybe_compact(
        &self,
        records: &[Record],
        turn_record_id: &str,
        sink: &mut dyn TurnSink,
    ) -> bool {
        let threshold = self.config.compaction_threshold;
        if threshold <= 0.0 {
            return false;
        }
        let window = effective_context_window(self.config.model_context_window);
        if self.config.extensions.compaction_strategy().is_none() {
            return false;
        }
        let last_prompt = records.iter().rev().find_map(|record| match record {
            Record::Assistant {
                usage: Some(usage), ..
            } => Some(usage_prompt_tokens(usage)),
            _ => None,
        });
        let Some(last_prompt) = last_prompt else {
            return false;
        };
        if (last_prompt as f64) < (window as f64) * threshold {
            return false;
        }
        // The candidate range: everything visible before this turn's
        // own user record, minus records without ids (session-start).
        let cut = records
            .iter()
            .position(|record| record.id() == Some(turn_record_id))
            .unwrap_or(records.len());
        let candidate: Vec<Record> = records[..cut]
            .iter()
            .filter(|record| record.id().is_some())
            .cloned()
            .collect();
        if candidate.len() < 2 {
            // Nothing worth replacing: a session-start-plus-one-message
            // range would trade the whole conversation for a line.
            return false;
        }
        compact_candidate(
            self.store,
            self.session,
            &self.config.extensions,
            self.config.completion_backend.as_ref(),
            candidate,
            sink,
        )
        .await
        .is_ok()
    }

    /// One completion call, with retry (FR-CORE-6) and cancellation
    /// (FR-CONC-3: dropping the producer stops the in-flight stream).
    pub(super) async fn provider_call(
        &self,
        request: CompletionRequest,
        sink: &mut dyn TurnSink,
        cancel: &CancelFlag,
    ) -> Result<CallResponse, CallFail> {
        let mut attempt = 0u32;
        loop {
            match self.stream_once(request.clone(), sink, cancel).await {
                Ok(response) => return Ok(response),
                Err(CallFail::Cancelled) => return Err(CallFail::Cancelled),
                Err(CallFail::Provider {
                    message,
                    class,
                    retryable,
                }) => {
                    if retryable && attempt < self.config.retry_limit {
                        let delay = self
                            .config
                            .retry_base_delay
                            .saturating_mul(1u32 << attempt.min(6));
                        sink.on_event(TurnEvent::RetryScheduled {
                            attempt: attempt + 1,
                            max: self.config.retry_limit,
                            delay_ms: delay.as_millis() as u64,
                            error: message.clone(),
                        });
                        if !delay.is_zero() {
                            tokio::select! {
                                _ = tokio::time::sleep(delay) => {}
                                _ = cancel.wait_cancelled() => return Err(CallFail::Cancelled),
                            }
                        } else if cancel.is_cancelled() {
                            return Err(CallFail::Cancelled);
                        }
                        attempt += 1;
                        continue;
                    }
                    // FR-CORE-7: the user sees that retries were tried and
                    // gave up, not just the raw transport error.
                    let message = if retryable && attempt > 0 {
                        format!("{message} (retries exhausted after {attempt})")
                    } else {
                        message
                    };
                    return Err(CallFail::Provider {
                        message,
                        class,
                        retryable,
                    });
                }
            }
        }
    }

    async fn stream_once(
        &self,
        request: CompletionRequest,
        sink: &mut dyn TurnSink,
        cancel: &CancelFlag,
    ) -> Result<CallResponse, CallFail> {
        let (tx, mut rx) = tokio::sync::mpsc::channel(32);
        let producer = self.provider.stream(request, tx);
        tokio::pin!(producer);

        let mut text = String::new();
        let mut reasoning = String::new();
        let mut acc = ToolCallAccumulator::default();
        let mut usage = Usage::default();
        let mut failure: Option<(String, &'static str, bool)> = None;
        let mut natural_end = false;

        loop {
            tokio::select! {
                biased;
                _ = cancel.wait_cancelled() => return Err(CallFail::Cancelled),
                item = rx.recv() => match item {
                    Some(StreamEvent::TextDelta { delta }) => {
                        text.push_str(&delta);
                        sink.on_event(TurnEvent::TextDelta(delta));
                    }
                    Some(StreamEvent::ReasoningDelta { delta }) => {
                        reasoning.push_str(&delta);
                        sink.on_event(TurnEvent::ReasoningDelta(delta));
                    }
                    Some(event @ (StreamEvent::ToolCallStart { .. }
                    | StreamEvent::ToolCallArgDelta { .. }
                    | StreamEvent::ToolCallEnd { .. })) => acc.handle(event),
                    Some(StreamEvent::Usage { usage: reported }) => usage = reported,
                    Some(StreamEvent::Error { message, retryable }) => {
                        failure = Some((message, "transport", retryable));
                        break;
                    }
                    Some(StreamEvent::VendorEvent { kind, payload }) => {
                        tracing::debug!(%kind, %payload, "vendor event");
                    }
                    None => break,
                },
                produced = &mut producer => {
                    // The producer finished; drain whatever it buffered.
                    if let Err(err) = produced {
                        failure = Some((err.message, err.class, err.retryable));
                    }
                    while let Some(item) = rx.recv().await {
                        match item {
                            StreamEvent::TextDelta { delta } => {
                                text.push_str(&delta);
                                sink.on_event(TurnEvent::TextDelta(delta));
                            }
                            StreamEvent::ReasoningDelta { delta } => {
                                reasoning.push_str(&delta);
                                sink.on_event(TurnEvent::ReasoningDelta(delta));
                            }
                            event @ (StreamEvent::ToolCallStart { .. }
                            | StreamEvent::ToolCallArgDelta { .. }
                            | StreamEvent::ToolCallEnd { .. }) => acc.handle(event),
                            StreamEvent::Usage { usage: reported } => usage = reported,
                            StreamEvent::Error { message, retryable } => {
                                failure = Some((message, "transport", retryable));
                            }
                            StreamEvent::VendorEvent { kind, payload } => {
                                tracing::debug!(%kind, %payload, "vendor event");
                            }
                        }
                    }
                    natural_end = failure.is_none();
                    break;
                }
            }
        }

        if let Some((message, class, retryable)) = failure {
            return Err(CallFail::Provider {
                message,
                class: class.to_string(),
                retryable,
            });
        }
        let (calls, protocol_errors) = acc.finish(natural_end);
        Ok(CallResponse {
            text,
            reasoning: if reasoning.is_empty() {
                None
            } else {
                Some(reasoning)
            },
            calls,
            protocol_errors,
            usage,
        })
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

fn effective_context_window(configured: u32) -> u32 {
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
