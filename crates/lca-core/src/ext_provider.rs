//! The provider-world adapter: a `provider`-world handle answers the
//! core's [`Provider`] trait, so the turn loop
//! never learns which world or delivery mode produced the stream
//! (ADR-0019's one-interface rule applied to the model backend).

use std::sync::Arc;

use lca_ext_abi::ExtensionDispatch;
use lca_protocol::{CompletionRequest, EventSink, ModelInfo, StreamEvent};
use lca_provider::{BoxFuture, EventSender, Provider, ProviderError};

/// Wraps one provider-world handle as the core's `Provider`.
pub struct ExtensionProvider {
    handle: Arc<dyn ExtensionDispatch>,
}

impl ExtensionProvider {
    /// Adapt a handle that implements the `provider` world.
    pub fn new(handle: Arc<dyn ExtensionDispatch>) -> ExtensionProvider {
        ExtensionProvider { handle }
    }
}

/// Mid-buffer between the handle's synchronous `EventSink` pushes and
/// the core's bounded channel: the async side below drains it with real
/// backpressure, so no push ever blocks or drops.
struct Mid(tokio::sync::mpsc::UnboundedSender<StreamEvent>);

impl EventSink for Mid {
    fn push(&self, event: StreamEvent) -> bool {
        self.0.send(event).is_ok()
    }
}

/// Recover the headless error class from a dispatch-level message
/// (`docs/headless.md`'s `transport`/`auth`/`invalid` vocabulary): the
/// error boundary is string-only, because WIT errors are text
/// (ADR-0019), so an embedded HTTP status regains the class its status
/// codes derive, a recorded refusal stays out of `transport`, and
/// everything else is a transport failure retried per FR-CORE-6.
/// ponytail: message-shape heuristics; a pre-freeze ABI addition could
/// carry the class explicitly (the `docs/abi-versioning.md` punch list
/// is where that belongs).
fn classify_message(message: &str) -> (&'static str, bool) {
    if let Some(status) = message
        .split("provider returned HTTP ")
        .nth(1)
        .and_then(|tail| tail.get(..3))
        .and_then(|token| token.parse::<u16>().ok())
    {
        let class = match status {
            401 | 403 => "auth",
            400..=499 => "invalid",
            _ => "transport",
        };
        let retryable = status == 429 || status == 408 || (500..=599).contains(&status);
        return (class, retryable);
    }
    let refusal = [
        "permission denied",
        "not granted",
        "not found",
        "invalid argument",
    ]
    .iter()
    .any(|needle| message.contains(needle));
    if refusal {
        ("invalid", false)
    } else {
        ("transport", true)
    }
}

impl Provider for ExtensionProvider {
    fn name(&self) -> &str {
        self.handle.name()
    }

    fn list_models(&self) -> Vec<ModelInfo> {
        // A handle that cannot enumerate answers with an error; the
        // picker falls back to the configured model (FR-PROV-2).
        self.handle.provider_models().unwrap_or_default()
    }

    fn stream(
        &self,
        request: CompletionRequest,
        tx: EventSender,
    ) -> BoxFuture<Result<(), ProviderError>> {
        let handle = self.handle.clone();
        Box::pin(async move {
            let (stx, mut srx) = tokio::sync::mpsc::unbounded_channel();
            let sink = Mid(stx);
            let answered = {
                let dispatch = handle.stream_completion(request, &sink);
                tokio::pin!(dispatch);
                let mut done = None;
                let mut core_gone = false;
                while done.is_none() {
                    tokio::select! {
                        maybe = srx.recv() => {
                            if let Some(event) = maybe {
                                // The core's receiver disappearing leaves
                                // the dispatch running to its end with
                                // events drained into the void; dropping
                                // this whole future cancels instead
                                // (FR-CONC-3), through the handle's own
                                // sink-closed path.
                                if !core_gone && tx.send(event).await.is_err() {
                                    core_gone = true;
                                }
                            }
                        }
                        result = &mut dispatch => done = Some(result),
                    }
                }
                // The loop exits only when the dispatch answered; dropping
                // it here closes the bridge so the drain below terminates.
                done.expect("the loop waits for the dispatch future")
            };
            drop(sink);
            while let Some(event) = srx.recv().await {
                if tx.send(event).await.is_err() {
                    while srx.recv().await.is_some() {}
                    break;
                }
            }
            answered.map_err(|err| {
                let message = err.to_string();
                let (class, retryable) = classify_message(&message);
                ProviderError {
                    message,
                    class,
                    retryable,
                }
            })
        })
    }
}

/// The backend behind the `completion` capability: the active provider
/// driven to one non-streaming response (capability catalog
/// `completion`, ADR-0015's forcing case). The CLI wires the same Arc
/// into the default strategy's capability engine and into
/// `AgentConfig`, so the usage this accumulates lands on the
/// compaction record that caused the spend.
pub struct ProviderBackend {
    provider: std::sync::Arc<dyn lca_provider::Provider>,
    /// The session's current model: `/model` moves it, so compaction
    /// keeps asking whichever model the conversation switched to.
    model: std::sync::Mutex<String>,
    session_id: String,
    usage: std::sync::Mutex<Option<lca_protocol::Usage>>,
}

impl ProviderBackend {
    /// Adapt the provider the agent itself talks to. `model` follows
    /// the session's configured model; `session_id` rides as the
    /// conversation-routing extras (ADR-0023).
    pub fn new(
        provider: std::sync::Arc<dyn lca_provider::Provider>,
        model: impl Into<String>,
        session_id: impl Into<String>,
    ) -> ProviderBackend {
        ProviderBackend {
            provider,
            model: std::sync::Mutex::new(model.into()),
            session_id: session_id.into(),
            usage: std::sync::Mutex::new(None),
        }
    }

    /// Follow a session-scoped model change (`/model`).
    pub fn set_model(&self, model: impl Into<String>) {
        *self
            .model
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = model.into();
    }
}

impl lca_tools::CompletionBackend for ProviderBackend {
    fn complete(
        &self,
        messages: &[lca_protocol::ChatMessage],
    ) -> Result<(String, lca_protocol::Usage), String> {
        let mut extras = std::collections::BTreeMap::new();
        extras.insert("session-id".to_string(), self.session_id.clone());
        let model = self
            .model
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let request = CompletionRequest {
            messages: messages.to_vec(),
            tools: Vec::new(),
            model,
            stable_prefix: 0,
            extras,
        };
        let (tx, mut rx) = tokio::sync::mpsc::channel::<StreamEvent>(64);
        let run = async move {
            // Poll the producer and the channel together until the
            // producer finishes - then drop it (closing the channel)
            // and drain whatever it buffered: a scripted or cached
            // response can complete before the first event is read.
            let done = {
                let producer = self.provider.stream(request, tx);
                tokio::pin!(producer);
                let done;
                let mut collected = (
                    String::new(),
                    lca_protocol::Usage::default(),
                    None::<String>,
                );
                let absorb = |event: StreamEvent, collected: &mut (String, lca_protocol::Usage, Option<String>)| {
                    match event {
                        StreamEvent::TextDelta { delta } => collected.0.push_str(&delta),
                        StreamEvent::Usage { usage: reported } => collected.1 = reported,
                        StreamEvent::Error { message, .. } => collected.2 = Some(message),
                        _ => {}
                    }
                };
                loop {
                    tokio::select! {
                        result = &mut producer => {
                            done = Some(result);
                            break;
                        }
                        item = rx.recv() => {
                            if let Some(event) = item {
                                absorb(event, &mut collected);
                            }
                        }
                    }
                }
                (done, collected)
            };
            let (result, mut collected) = done;
            while let Some(event) = rx.recv().await {
                match event {
                    StreamEvent::TextDelta { delta } => collected.0.push_str(&delta),
                    StreamEvent::Usage { usage: reported } => collected.1 = reported,
                    StreamEvent::Error { message, .. } => collected.2 = Some(message),
                    _ => {}
                }
            }
            if collected.2.is_none()
                && let Some(Err(err)) = result
            {
                collected.2 = Some(err.message);
            }
            collected
        };
        let (text, usage, failure) = match tokio::runtime::Handle::try_current() {
            Ok(handle) => handle.block_on(run),
            Err(_) => {
                // ponytail: a fresh current-thread runtime outside any
                // async context (tests, synchronous host calls); the
                // ambient-handle path is the one production uses.
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|err| format!("runtime: {err}"))?;
                runtime.block_on(run)
            }
        };
        if let Some(message) = failure {
            return Err(message);
        }
        let mut slot = self.usage.lock().expect("backend usage");
        let total = slot.get_or_insert_with(lca_protocol::Usage::default);
        total.input += usage.input;
        total.output += usage.output;
        total.cache_read += usage.cache_read;
        total.cache_write += usage.cache_write;
        total.cache_write_1h += usage.cache_write_1h;
        total.cost += usage.cost;
        total.cost_input += usage.cost_input;
        total.cost_cache_read += usage.cost_cache_read;
        total.cost_cache_write += usage.cost_cache_write;
        Ok((text, usage))
    }

    fn take_usage(&self) -> Option<lca_protocol::Usage> {
        self.usage.lock().expect("backend usage").take()
    }
}
