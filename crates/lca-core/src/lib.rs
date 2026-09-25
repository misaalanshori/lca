//! The agent loop: one Tokio-driven turn at a time, sequential tool
//! execution, retry with backoff, and cancellation that keeps every record
//! already written (ADR-0014, `docs/flows.md`).
//!
//! The pre-tool hook seam (FR-CORE-10) runs before the permission layer;
//! compaction invocation (`FR-SESS-4`) arrives in Phase 4.

#![forbid(unsafe_code)]

mod registry;

pub mod ext_provider;
pub use ext_provider::ExtensionProvider;
pub use registry::{BUILTIN_COMMANDS, BUILTIN_TOOLS, CollisionReport, ExtensionRegistry};
// The turn types live in the protocol layer so the interface and the
// embedding SDK can render them without depending on this crate.
pub use lca_protocol::{StopReason, TurnEvent, TurnOutcome, TurnStatus};

use std::sync::{Arc, Mutex};
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
#[derive(Clone)]
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
    /// Context-window fraction that triggers compaction (FR-SESS-4,
    /// `compaction.threshold`). At or below zero disables the check.
    pub compaction_threshold: f64,
    /// The active model's context window in tokens; `0` means unknown
    /// and skips the threshold check entirely.
    /// ponytail: an endpoint that publishes no window never compacts;
    /// a fallback estimate is the upgrade path if that bites.
    pub model_context_window: u32,
    /// The backend behind the default strategy's `completion` call,
    /// held so the compaction record can carry the summarization's
    /// usage (capability catalog: spend shows in session cost). The
    /// CLI wires the same Arc into the strategy's capability engine.
    pub completion_backend: Option<Arc<dyn lca_tools::CompletionBackend>>,
    /// The full message list sent on the previous provider call, for
    /// FR-CACHE-6's divergence check (`None` before the first call).
    pub sent_stable: Arc<Mutex<Option<Vec<String>>>>,
}

impl std::fmt::Debug for AgentConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The completion backend is a trait object with no Debug of its
        // own; presence is all a log line needs.
        f.debug_struct("AgentConfig")
            .field("provider", &self.provider)
            .field("model", &self.model)
            .field("retry_limit", &self.retry_limit)
            .field("retry_base_delay", &self.retry_base_delay)
            .field("max_iterations", &self.max_iterations)
            .field("system_prompt", &self.system_prompt)
            .field("extensions", &self.extensions)
            .field("compaction_threshold", &self.compaction_threshold)
            .field("model_context_window", &self.model_context_window)
            .field("completion_backend", &self.completion_backend.is_some())
            .finish()
    }
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
            compaction_threshold: 0.8,
            model_context_window: 0,
            completion_backend: None,
            sent_stable: Arc::new(Mutex::new(None)),
        }
    }
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

/// Run one future to completion from a synchronous thread that a
/// runtime is already driving (the interface's command thread: `main`
/// holds `Runtime::block_on`, where a nested `block_on` panics -
/// measured, not guessed - so building a second runtime here is out).
/// The future gets its own thread and its own current-thread runtime;
/// this thread waits for it.
/// ponytail: one thread per invocation; commands are human-paced, so
/// the cost is invisible, and a concurrent caller just joins.
pub fn drive_blocking<T: Send + 'static>(
    future: impl std::future::Future<Output = T> + Send + 'static,
) -> T {
    std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("command runtime")
            .block_on(future)
    })
    .join()
    .expect("command task ended")
}

/// The compaction call itself, shared by the threshold path
/// (FR-SESS-4) and the manual `/compact` trigger: the summary always
/// comes from the compaction world's strategy (FR-SESS-5 - there is no
/// built-in summarizing path), the durable record is the host's, and
/// the candidate range is the caller's to choose.
async fn compact_candidate(
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

/// The resolved message list plus the stable-prefix boundary (FR-CACHE-5).
#[derive(Debug, Clone)]
pub struct Assembled {
    /// Messages, oldest first, system prompt leading.
    pub messages: Vec<ChatMessage>,
    /// Count of leading messages inside the stable cache boundary.
    pub stable_prefix: usize,
    /// Whether a compaction record appeared (FR-CACHE-5's anchor).
    pub compaction_seen: bool,
}

/// Build the outbound message list from a session's resolved (display-view)
/// records: compaction applied, forks followed, transforms not yet run
/// (they arrive with the `context-transform` world in Phase 4).
pub fn assemble(records: &[Record], system_prompt: &str) -> Assembled {
    let mut messages = vec![ChatMessage::text(MessageRole::System, system_prompt)];
    let mut stable_prefix = 0usize;
    let mut compaction_seen = false;
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
            Record::Compaction { summary, .. } => {
                compaction_seen = true;
                // The summary stands in for the range it replaced
                // (session-log-format: the reader substitutes it), and
                // everything through it becomes the stable prefix
                // (FR-CACHE-5, ADR-0017).
                messages.push(ChatMessage::text(MessageRole::User, summary.clone()));
                stable_prefix = messages.len();
            }
        }
    }
    Assembled {
        messages,
        stable_prefix,
        compaction_seen,
    }
}

/// One request's prompt token count, the number FR-CACHE-1 compares.
fn usage_prompt_tokens(usage: &Usage) -> u64 {
    usage.input + usage.cache_read + usage.cache_write + usage.cache_write_1h
}

/// All text a message carries (the comparison key for finding this
/// turn's own user message).
fn message_text(message: &ChatMessage) -> String {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            ContentBlock::Reasoning { reasoning } => Some(reasoning.as_str()),
            ContentBlock::ToolCall { .. } => None,
        })
        .collect()
}

/// What one message looks like on the wire, for FR-CACHE-6's
/// previous-versus-current comparison: role, text, and tool calls.
fn stable_fingerprint(message: &ChatMessage) -> String {
    let text: String = message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            ContentBlock::Reasoning { reasoning } => Some(reasoning.as_str()),
            ContentBlock::ToolCall { .. } => None,
        })
        .collect();
    let calls: Vec<String> = message
        .tool_calls
        .iter()
        .map(|call| format!("{}:{}:{}", call.call_id, call.name, call.arguments))
        .collect();
    format!("{:?}|{}|{:?}", message.role, text, calls)
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
    grants: Arc<Mutex<GrantStore>>,
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
        grants: Arc<Mutex<GrantStore>>,
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
        // `attention-required`: the turn failed and the user should look. The
        // reason is the same text the interface surfaced.
        if outcome.status == TurnStatus::Error {
            let reason = outcome
                .error
                .clone()
                .unwrap_or_else(|| "the turn ended with an error".to_string());
            self.config.extensions.on_attention_required(&reason).await;
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
        let turn_record_id = lca_session::new_record_id();
        if let Err(err) = self.store.append(
            self.session,
            Record::User {
                v: FORMAT_VERSION,
                ts,
                id: turn_record_id.clone(),
                content: input.to_string(),
                attachments: Vec::new(),
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

            let mut records = match self.store.read_with(self.session, ViewMode::Display) {
                Ok(outcome) => outcome.records,
                Err(err) => {
                    return self.fail(StopReason::Error, format!("cannot read the session: {err}"));
                }
            };
            // FR-SESS-4: one threshold check per turn, before the first
            // provider call; a strategy refusal leaves the turn running
            // and lands in the log as an extension event.
            if rounds == 0 && self.maybe_compact(&records, &turn_record_id, sink).await {
                records = match self.store.read_with(self.session, ViewMode::Display) {
                    Ok(outcome) => outcome.records,
                    Err(err) => {
                        return self
                            .fail(StopReason::Error, format!("cannot re-read the log: {err}"));
                    }
                };
            }
            let assembled = assemble(&records, &self.config.system_prompt);
            // The boundary base (FR-CACHE-5): everything through the
            // most recent compaction record, never reaching this turn's
            // own user message - content sent for the first time this
            // call cannot be claimed as cached. Without a record the
            // base is unbounded and only the current message caps it,
            // so the boundary grows turn by turn (what
            // docs/providers/antigravity.md relies on) and resets to
            // the summary when compaction lands. Computed before the
            // transforms run, on the list where the record ids still
            // line up with the messages.
            let mut stable_cap = if assembled.compaction_seen {
                assembled.stable_prefix
            } else {
                usize::MAX
            };
            stable_cap = stable_cap.min(assembled.messages.len());
            if let Some(index) = assembled.messages.iter().rposition(|message| {
                message.role == lca_protocol::MessageRole::User && message_text(message) == input
            }) {
                stable_cap = stable_cap.min(index);
            }
            // FR-CTX-2: every enabled transform, in installation order,
            // before every provider call; a rejection ends the turn
            // here, with no call (FR-CTX-3) and nothing persisted
            // (FR-CTX-4).
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
                    return TurnOutcome {
                        status: TurnStatus::Error,
                        stop_reason: StopReason::Error,
                        usage: turn_usage,
                        error: Some(reason),
                    };
                }
            };
            let mut stable_prefix = stable_cap.min(messages.len());
            // FR-CACHE-6: content inside that boundary which differs
            // from what actually went out on the previous call narrows
            // it to end before the earliest differing message,
            // recorded as an extension event - never a rejection.
            {
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
                let previous = self
                    .config
                    .sent_stable
                    .lock()
                    .expect("sent-stable lock")
                    .clone();
                if let Some(previous) = previous {
                    let diverged = (0..stable_prefix).find(|&index| {
                        previous
                            .get(index)
                            .map(|there| *there != current[index])
                            .unwrap_or(true)
                    });
                    if let Some(index) = diverged {
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
                    }
                }
                // Store the FULL list actually sent (the divergence
                // basis is content, not the previous claim).
                *self.config.sent_stable.lock().expect("sent-stable lock") =
                    Some(messages.iter().map(stable_fingerprint).collect());
                let _ = current;
            }
            let mut tools = ToolExecutor::specs();
            tools.extend(self.config.extensions.tool_specs());
            let mut extras = std::collections::BTreeMap::new();
            // ADR-0023: the conversation's routing identity travels on
            // every request; the OpenCode Go endpoint requires its
            // header and every other endpoint ignores it.
            extras.insert("session-id".to_string(), self.session.id().to_string());
            let request = CompletionRequest {
                messages,
                tools,
                model: self.config.model.clone(),
                stable_prefix,
                extras,
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
        //
        // Validate the arguments against the schema the model saw before the
        // tool runs (extension authoring guide); an invalid call never reaches
        // the tool. Extension schemas come from the registry, built-ins from
        // the executor's own table.
        let schema = registry
            .tool_schema(&call.name)
            .map(|spec| spec.parameters.clone())
            .or_else(|| {
                ToolExecutor::specs()
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
    /// FR-SESS-4's threshold check, then the shared compaction call.
    /// Returns whether a record was written (the caller re-reads).
    async fn maybe_compact(
        &self,
        records: &[Record],
        turn_record_id: &str,
        sink: &mut dyn TurnSink,
    ) -> bool {
        let threshold = self.config.compaction_threshold;
        let window = self.config.model_context_window;
        if window == 0 || threshold <= 0.0 {
            // ponytail: no published window means no ratio to cross;
            // see AgentConfig::model_context_window.
            return false;
        }
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
