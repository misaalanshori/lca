//! The agent loop: one Tokio-driven turn at a time, sequential tool
//! execution, retry with backoff, and cancellation that keeps every record
//! already written (ADR-0014, `docs/flows.md`).
//!
//! The pre-tool hook seam (FR-CORE-10) runs before the permission layer;
//! compaction invocation (`FR-SESS-4`) arrives in Phase 4.

#![forbid(unsafe_code)]

mod registry;

pub use registry::{BUILTIN_COMMANDS, BUILTIN_TOOLS, CollisionReport, ExtensionRegistry};

use std::sync::Arc;
use std::time::Duration;

use lca_ext_abi::ExtensionDispatch;
use lca_permissions::{GrantStore, PermissionPrompt, Proposals};
use lca_protocol::{
    ChatMessage, ContentBlock, DispatchError, FORMAT_VERSION, HookAction, MessageRole, Record,
    StreamEvent, ToolCall, ToolResult, ToolSource, Usage,
};
use lca_provider::{CompletionRequest, ProtocolError, Provider, ToolCallAccumulator};
use lca_session::{Session, SessionStore, ViewMode};
use lca_tools::{CancelFlag, ToolExecutor};

/// Static configuration for the loop, built from merged configuration.
#[derive(Debug, Clone)]
pub struct AgentConfig {
    /// Active provider extension name (recorded on assistant records).
    pub provider: String,
    /// Active model identifier.
    pub model: String,
    /// Retry attempts for retryable transport errors (FR-CORE-6).
    pub retry_limit: u32,
    /// First retry delay; doubles per attempt (FR-CORE-6). Tests use zero.
    pub retry_base_delay: Duration,
    /// Maximum tool-call rounds within one turn (FR-CORE-9).
    pub max_iterations: u32,
    /// System message prepended to every request.
    pub system_prompt: String,
    /// The dispatch table: loaded extensions in registration order
    /// (ADR-0019; empty by default).
    pub extensions: Arc<ExtensionRegistry>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        AgentConfig {
            provider: "openai-compatible".to_string(),
            model: String::new(),
            retry_limit: 3,
            retry_base_delay: Duration::from_millis(250),
            max_iterations: 50,
            system_prompt:
                "You are LCA, a coding agent. Use the tools to read, write, edit, search, \
                 and run commands in the user's workspace."
                    .to_string(),
            extensions: Arc::new(ExtensionRegistry::new()),
        }
    }
}

/// Whether a turn ended cleanly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnStatus {
    /// The turn completed.
    Ok,
    /// The turn ended with an error.
    Error,
}

/// Why a turn stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// The model stopped without requesting a tool.
    Stop,
    /// The tool-call iteration limit was hit (FR-CORE-9).
    IterationLimit,
    /// The user cancelled (FR-CORE-5).
    Cancelled,
    /// A provider error after the retry limit (FR-CORE-7) or a rejection.
    Error,
}

/// What one turn produced.
#[derive(Debug, Clone)]
pub struct TurnOutcome {
    /// Clean or error.
    pub status: TurnStatus,
    /// Why it stopped.
    pub stop_reason: StopReason,
    /// The turn's usage, summed across provider calls (FR-CORE-8).
    pub usage: Usage,
    /// The surfaced error, when the turn ended in one.
    pub error: Option<String>,
}

/// Events the interface and headless mode render. Owned copies, so sinks
/// store them freely.
#[derive(Debug, Clone)]
pub enum TurnEvent {
    /// A chunk of response text (FR-CORE-4).
    TextDelta(String),
    /// A chunk of reasoning text.
    ReasoningDelta(String),
    /// A completed response's text (headless `text` envelope).
    AssistantText(String),
    /// A response's usage (headless `usage` envelope, FR-CORE-8).
    Usage(Usage),
    /// A tool call is about to run (after permission).
    ToolStarted(ToolCall),
    /// A tool call finished (headless `tool-result` envelope).
    ToolFinished(ToolResult),
    /// A chunk of live shell output while a command runs (FR-TOOL-4).
    ToolOutputChunk {
        /// The running call.
        call_id: String,
        /// The chunk as text.
        chunk: String,
    },
    /// A retry was scheduled (FR-CORE-6).
    RetryScheduled {
        ///1-based attempt number about to run.
        attempt: u32,
        /// The configured limit.
        max: u32,
        /// Delay before it runs.
        delay_ms: u64,
        /// The error that caused the retry.
        error: String,
    },
    /// An error surfaced to the interface (headless `error` envelope).
    Error {
        /// What went wrong.
        message: String,
        /// Class from `lca_provider::ProviderError` or `internal`.
        class: String,
        /// Whether a retry could have helped.
        retryable: bool,
    },
    /// An extension lifecycle event: load, disable, trap, capability
    /// denial, or cache divergence (the headless `extension-event`
    /// envelope, `docs/headless.md`).
    ExtensionEvent {
        /// The extension's identity.
        extension: String,
        /// The event kind: `disabled`, `error`, `collision`, ...
        event: String,
        /// Detail text.
        detail: String,
    },
    /// The turn ended.
    TurnEnded {
        /// Clean or error.
        status: TurnStatus,
        /// Why.
        stop_reason: StopReason,
    },
}

/// Receives turn events as they happen.
pub trait TurnSink: Send {
    /// Handle one event.
    fn on_event(&mut self, event: TurnEvent);
}

/// A sink that drops everything.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullSink;

impl TurnSink for NullSink {
    fn on_event(&mut self, _event: TurnEvent) {}
}

/// The resolved message list plus the stable-prefix boundary (FR-CACHE-5).
#[derive(Debug, Clone)]
pub struct Assembled {
    /// Messages, oldest first, system prompt leading.
    pub messages: Vec<ChatMessage>,
    /// Count of leading messages inside the stable cache boundary.
    pub stable_prefix: usize,
}

/// Build the outbound message list from a session's resolved (display-view)
/// records: compaction applied, forks followed, transforms not yet run
/// (they arrive with the `context-transform` world in Phase 4).
pub fn assemble(records: &[Record], system_prompt: &str) -> Assembled {
    let mut messages = vec![ChatMessage::text(MessageRole::System, system_prompt)];
    let mut stable_prefix = 0usize;
    for record in records {
        match record {
            Record::SessionStart { .. }
            | Record::SessionEnd { .. }
            | Record::ForkPoint { .. }
            | Record::Permission { .. }
            | Record::ExtensionEvent { .. } => {}
            Record::User { content, .. } => {
                messages.push(ChatMessage::text(MessageRole::User, content.clone()));
            }
            Record::Assistant {
                content, reasoning, ..
            } => {
                let mut message = ChatMessage {
                    role: MessageRole::Assistant,
                    content: content.clone(),
                    tool_calls: Vec::new(),
                    tool_call_id: None,
                    usage: None,
                    extras: Default::default(),
                };
                if let Some(reasoning) = reasoning {
                    message.content.insert(
                        0,
                        ContentBlock::Reasoning {
                            reasoning: reasoning.clone(),
                        },
                    );
                }
                messages.push(message);
            }
            Record::ToolCall {
                call_id,
                name,
                arguments,
                ..
            } => {
                // Tool calls belong to the assistant message that requested
                // them; the log keeps them as their own records.
                if let Some(assistant) = messages
                    .iter_mut()
                    .rev()
                    .find(|m| m.role == MessageRole::Assistant)
                {
                    assistant.content.push(ContentBlock::ToolCall {
                        call_id: call_id.clone(),
                        name: name.clone(),
                        arguments: arguments.clone(),
                    });
                    assistant.tool_calls.push(ToolCall {
                        call_id: call_id.clone(),
                        name: name.clone(),
                        arguments: arguments.clone(),
                    });
                }
            }
            Record::ToolResult {
                call_id,
                content,
                attachment,
                truncated,
                ..
            } => {
                let mut text = content
                    .clone()
                    .or_else(|| {
                        attachment
                            .clone()
                            .map(|hash| format!("[attachment {hash}]"))
                    })
                    .unwrap_or_default();
                if *truncated {
                    text.push_str("\n[result truncated]");
                }
                messages.push(ChatMessage::tool_result(call_id.clone(), text));
            }
            Record::Compaction { .. } => {
                // The compaction record marks the cache boundary: everything
                // before it is the stable prefix (FR-CACHE-5, ADR-0017).
                stable_prefix = messages.len();
            }
        }
    }
    Assembled {
        messages,
        stable_prefix,
    }
}

struct CallResponse {
    text: String,
    reasoning: Option<String>,
    calls: Vec<ToolCall>,
    protocol_errors: Vec<ProtocolError>,
    usage: Usage,
}

enum CallFail {
    Cancelled,
    Provider {
        message: String,
        class: String,
        retryable: bool,
    },
}

/// One agent, bound to one session for one turn at a time.
pub struct Agent<'a> {
    store: &'a SessionStore,
    session: &'a Session,
    provider: &'a dyn Provider,
    tools: &'a mut ToolExecutor,
    grants: &'a mut GrantStore,
    prompt: &'a mut dyn PermissionPrompt,
    proposals: Option<&'a Proposals>,
    config: AgentConfig,
}

impl<'a> Agent<'a> {
    /// Bind the loop to its collaborators.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        store: &'a SessionStore,
        session: &'a Session,
        provider: &'a dyn Provider,
        tools: &'a mut ToolExecutor,
        grants: &'a mut GrantStore,
        prompt: &'a mut dyn PermissionPrompt,
        proposals: Option<&'a Proposals>,
        config: AgentConfig,
    ) -> Agent<'a> {
        Agent {
            store,
            session,
            provider,
            tools,
            grants,
            prompt,
            proposals,
            config,
        }
    }

    /// Run one turn to completion (or cancellation, or error).
    ///
    /// While it runs, a watcher watches the cancellation flag and, when it
    /// fires, bumps every WASM extension's epoch so a spinning call traps
    /// at its next yield (FR-CONC-1, ADR-0014) regardless of fuel.
    pub async fn run_turn(
        &mut self,
        input: &str,
        sink: &mut dyn TurnSink,
        cancel: &CancelFlag,
    ) -> TurnOutcome {
        let handles: Vec<Arc<dyn ExtensionDispatch>> =
            self.config.extensions.enabled().cloned().collect();
        // A plain thread, not a task: a synchronous WASM call blocks the
        // runtime thread it runs on, and cancellation must still reach a
        // spinning instance from a thread that is definitely running
        // (FR-CONC-1). One-millisecond poll keeps NFR-29's budget.
        let shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let watcher = if handles.is_empty() {
            None
        } else {
            let cancel = cancel.clone();
            let shutdown = shutdown.clone();
            Some(std::thread::spawn(move || {
                while !cancel.is_cancelled() && !shutdown.load(std::sync::atomic::Ordering::SeqCst)
                {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                if cancel.is_cancelled() {
                    for handle in handles {
                        handle.interrupt();
                    }
                }
            }))
        };
        let outcome = self.turn_body(input, sink, cancel).await;
        shutdown.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(watcher) = watcher {
            let _ = watcher.join();
        }
        let status = match outcome.status {
            TurnStatus::Ok => "ok",
            TurnStatus::Error => "error",
        };
        self.config.extensions.on_post_turn_end(status).await;
        outcome
    }

    async fn turn_body(
        &mut self,
        input: &str,
        sink: &mut dyn TurnSink,
        cancel: &CancelFlag,
    ) -> TurnOutcome {
        let ts = lca_session::now_ms();
        if let Err(err) = self.store.append(
            self.session,
            Record::User {
                v: FORMAT_VERSION,
                ts,
                id: lca_session::new_record_id(),
                content: input.to_string(),
                attachments: Vec::new(),
            },
        ) {
            return self.fail(
                StopReason::Error,
                format!("cannot write to the session log: {err}"),
            );
        }

        let mut turn_usage = Usage::default();
        let mut rounds = 0u32;
        loop {
            if cancel.is_cancelled() {
                sink.on_event(TurnEvent::TurnEnded {
                    status: TurnStatus::Ok,
                    stop_reason: StopReason::Cancelled,
                });
                return TurnOutcome {
                    status: TurnStatus::Ok,
                    stop_reason: StopReason::Cancelled,
                    usage: turn_usage,
                    error: None,
                };
            }

            let records = match self.store.read_with(self.session, ViewMode::Display) {
                Ok(outcome) => outcome.records,
                Err(err) => {
                    return self.fail(StopReason::Error, format!("cannot read the session: {err}"));
                }
            };
            let assembled = assemble(&records, &self.config.system_prompt);
            let mut tools = ToolExecutor::specs();
            tools.extend(self.config.extensions.tool_specs());
            let request = CompletionRequest {
                messages: assembled.messages,
                tools,
                model: self.config.model.clone(),
                stable_prefix: assembled.stable_prefix,
                extras: Default::default(),
            };

            let response = match self.provider_call(request, sink, cancel).await {
                Ok(response) => response,
                Err(CallFail::Cancelled) => {
                    sink.on_event(TurnEvent::TurnEnded {
                        status: TurnStatus::Ok,
                        stop_reason: StopReason::Cancelled,
                    });
                    return TurnOutcome {
                        status: TurnStatus::Ok,
                        stop_reason: StopReason::Cancelled,
                        usage: turn_usage,
                        error: None,
                    };
                }
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
            if !response.protocol_errors.is_empty() {
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
                    return TurnOutcome {
                        status: TurnStatus::Error,
                        stop_reason: StopReason::Error,
                        usage: turn_usage,
                        error: Some(message),
                    };
                }
            }

            // Persist the response: one assistant record plus one record per
            // tool call (docs/session-log-format.md).
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
                return self.fail(
                    StopReason::Error,
                    format!("cannot write to the session log: {err}"),
                );
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
                    return self.fail(
                        StopReason::Error,
                        format!("cannot write to the session log: {err}"),
                    );
                }
            }
            if !response.text.is_empty() {
                sink.on_event(TurnEvent::AssistantText(response.text.clone()));
            }
            sink.on_event(TurnEvent::Usage(response.usage.clone()));

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

            // FR-CORE-9: bound the tool rounds.
            rounds += 1;
            if rounds > self.config.max_iterations {
                let message = format!(
                    "iteration limit of {} tool-call rounds reached",
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
                    sink.on_event(TurnEvent::TurnEnded {
                        status: TurnStatus::Ok,
                        stop_reason: StopReason::Cancelled,
                    });
                    return TurnOutcome {
                        status: TurnStatus::Ok,
                        stop_reason: StopReason::Cancelled,
                        usage: turn_usage,
                        error: None,
                    };
                }
            }
        }
    }

    fn fail(&self, reason: StopReason, message: String) -> TurnOutcome {
        TurnOutcome {
            status: TurnStatus::Error,
            stop_reason: reason,
            usage: Usage::default(),
            error: Some(message),
        }
    }

    /// Append an extension lifecycle record and surface it (FR-EXT-3's
    /// report half; the headless `extension-event` envelope).
    fn record_extension_event(
        &mut self,
        extension: &str,
        event: &str,
        detail: &str,
        sink: &mut dyn TurnSink,
    ) -> Result<(), String> {
        self.store
            .append(
                self.session,
                Record::ExtensionEvent {
                    v: FORMAT_VERSION,
                    ts: lca_session::now_ms(),
                    id: lca_session::new_record_id(),
                    extension: extension.to_string(),
                    event: event.to_string(),
                    detail: detail.to_string(),
                },
            )
            .map_err(|err| format!("cannot write to the session log: {err}"))?;
        sink.on_event(TurnEvent::ExtensionEvent {
            extension: extension.to_string(),
            event: event.to_string(),
            detail: detail.to_string(),
        });
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
            let mut observe = |handle: &Arc<dyn ExtensionDispatch>, err: &DispatchError| {
                hook_errors.push((handle.name().to_string(), err.to_string()));
            };
            registry.pre_tool_use(&effective, &mut observe).await
        };
        for (extension, detail) in hook_errors {
            if let Err(err) = self.record_extension_event(&extension, "error", &detail, sink) {
                return Err(self.fail(StopReason::Error, err));
            }
        }

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
                attachment: None,
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
        registry: &Arc<ExtensionRegistry>,
        sink: &mut dyn TurnSink,
        cancel: &CancelFlag,
    ) -> Result<ToolResult, TurnOutcome> {
        // Phase 2 seam note: hooks have already run by the time this is
        // called (FR-CORE-10: hook before permission).
        if let Some(action) = self.tools.required_permission(call) {
            let outcome = match lca_permissions::authorize(
                self.grants,
                self.tools.workspace(),
                &action,
                self.proposals,
                self.prompt,
            ) {
                Ok(outcome) => outcome,
                Err(err) => {
                    return Err(
                        self.fail(StopReason::Error, format!("permission store error: {err}"))
                    );
                }
            };
            if outcome.prompted {
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
                return Ok(ToolResult::denied(
                    call.call_id.clone(),
                    format!("The user denied this action: {}", action.display()),
                ));
            }
        }

        sink.on_event(TurnEvent::ToolStarted(call.clone()));

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

    /// One completion call, with retry (FR-CORE-6) and cancellation
    /// (FR-CONC-3: dropping the producer stops the in-flight stream).
    async fn provider_call(
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

fn accumulate_usage(total: &mut Usage, per_call: &Usage) {
    total.input = total.input.saturating_add(per_call.input);
    total.output = total.output.saturating_add(per_call.output);
    total.cache_read = total.cache_read.saturating_add(per_call.cache_read);
    total.cache_write = total.cache_write.saturating_add(per_call.cache_write);
    total.cache_write_1h = total.cache_write_1h.saturating_add(per_call.cache_write_1h);
    total.cost += per_call.cost;
}
