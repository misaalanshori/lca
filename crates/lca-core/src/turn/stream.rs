//! Provider streaming (gh #45's observation buffer): one attempt's
//! events through the accumulator, replayed to `hooks-stream` in order
//! once the stream closes. Split from `turn/mod.rs` for the workspace
//! file ceiling (gate 11).

use lca_protocol::{StreamEvent, TurnEvent, Usage};
use lca_provider::{CompletionRequest, ToolCallAccumulator};
use lca_tools::CancelFlag;

use super::{CallFail, CallResponse};
use crate::{Agent, TurnSink};

impl Agent<'_> {
    /// Replay one closed stream to `hooks-stream` observers (gh #45),
    /// in arrival order, labeled with the producing provider+model.
    /// Observation only: nothing here steers the turn.
    async fn replay_stream(&self, model: &str, events: Vec<StreamEvent>) {
        for event in &events {
            let (kind, data) = match event {
                StreamEvent::TextDelta { delta } => ("text-delta", delta.clone()),
                StreamEvent::ReasoningDelta { delta } => ("reasoning-delta", delta.clone()),
                StreamEvent::ToolCallStart { call_id, name } => {
                    ("tool-call-start", format!("{name} {call_id}"))
                }
                StreamEvent::ToolCallArgDelta { call_id, delta } => {
                    ("tool-call-arg-delta", format!("{call_id} {delta}"))
                }
                StreamEvent::ToolCallEnd { call_id } => ("tool-call-end", call_id.clone()),
                StreamEvent::Usage { usage } => (
                    "usage",
                    format!("input {} output {}", usage.input, usage.output),
                ),
                StreamEvent::Error { message, .. } => ("error", message.clone()),
                StreamEvent::VendorEvent { kind, payload } => {
                    ("vendor-event", format!("{kind} {payload}"))
                }
            };
            self.config
                .extensions
                .observe_stream_event(&self.config.provider, model, kind, &data)
                .await;
        }
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
                Ok(response) => {
                    if attempt > 0 {
                        sink.on_event(TurnEvent::RetryFinished { success: true });
                    }
                    return Ok(response);
                }
                Err(CallFail::Cancelled) => return Err(CallFail::Cancelled),
                Err(CallFail::Provider {
                    message,
                    class,
                    retryable,
                }) => {
                    // Gh #202: capacity names retry even when the
                    // provider flagged the failure non-retryable.
                    let retry = retryable || lca_protocol::is_capacity_error(&message);
                    if retry && attempt < self.config.retry_limit {
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
                    // gave up, not just the raw transport error. Any
                    // attempt past zero scheduled a retry (by flag or by
                    // the gh #202 capacity override), so it owns the
                    // finish event and the exhausted note either way.
                    let message = if attempt > 0 {
                        sink.on_event(TurnEvent::RetryFinished { success: false });
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
        sink.on_event(TurnEvent::MessageStarted { role: "assistant" });
        let (tx, mut rx) = tokio::sync::mpsc::channel(32);
        // The replay labels events with the producing model; the
        // request itself moves into the producer below.
        let model = request.model.clone();
        let producer = self.provider.stream(request, tx);
        tokio::pin!(producer);

        let mut text = String::new();
        let mut reasoning = String::new();
        // gh #41: the latest thinking signature (one reasoning run
        // carries one; a later block replaces an earlier one, matching
        // the transcript order replay resends).
        let mut signature: Option<String> = None;
        let mut acc = ToolCallAccumulator::default();
        let mut usage = Usage::default();
        let mut failure: Option<(String, &'static str, bool)> = None;
        // Gh #45's stream observation buffers the normalized events
        // and replays them in order once the stream closes:
        // observation never steers or stalls a live stream.
        let mut stream_log: Vec<StreamEvent> = Vec::new();
        let mut natural_end = false;

        loop {
            tokio::select! {
                biased;
                _ = cancel.wait_cancelled() => return Err(CallFail::Cancelled),
                item = rx.recv() => match item {
                    Some(StreamEvent::TextDelta { delta }) => {
                        stream_log.push(StreamEvent::TextDelta { delta: delta.clone() });
                        text.push_str(&delta);
                        sink.on_event(TurnEvent::TextDelta(delta));
                    }
                    Some(StreamEvent::ReasoningDelta { delta }) => {
                        stream_log.push(StreamEvent::ReasoningDelta { delta: delta.clone() });
                        reasoning.push_str(&delta);
                        sink.on_event(TurnEvent::ReasoningDelta(delta));
                    }
                    Some(event @ (StreamEvent::ToolCallStart { .. }
                    | StreamEvent::ToolCallArgDelta { .. }
                    | StreamEvent::ToolCallEnd { .. })) => {
                        stream_log.push(event.clone());
                        acc.handle(event)
                    }
                    Some(StreamEvent::VendorEvent { kind, payload })
                        if kind == lca_protocol::THINKING_SIGNATURE_KIND =>
                    {
                        stream_log.push(StreamEvent::VendorEvent {
                            kind: kind.clone(),
                            payload: payload.clone(),
                        });
                        note_signature(&mut signature, &payload);
                    }
                    Some(StreamEvent::Usage { usage: reported }) => {
                        stream_log.push(StreamEvent::Usage { usage: reported.clone() });
                        usage = reported;
                    }
                    Some(StreamEvent::Error { message, retryable }) => {
                        stream_log.push(StreamEvent::Error { message: message.clone(), retryable });
                        failure = Some((message, "transport", retryable));
                        break;
                    }
                    Some(StreamEvent::VendorEvent { kind, payload }) => {
                        stream_log.push(StreamEvent::VendorEvent { kind: kind.clone(), payload: payload.clone() });
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
                                stream_log.push(StreamEvent::TextDelta { delta: delta.clone() });
                                text.push_str(&delta);
                                sink.on_event(TurnEvent::TextDelta(delta));
                            }
                            StreamEvent::ReasoningDelta { delta } => {
                                stream_log.push(StreamEvent::ReasoningDelta { delta: delta.clone() });
                                reasoning.push_str(&delta);
                                sink.on_event(TurnEvent::ReasoningDelta(delta));
                            }
                            event @ (StreamEvent::ToolCallStart { .. }
                            | StreamEvent::ToolCallArgDelta { .. }
                            | StreamEvent::ToolCallEnd { .. }) => {
                                stream_log.push(event.clone());
                                acc.handle(event)
                            }
                            StreamEvent::Usage { usage: reported } => {
                                stream_log.push(StreamEvent::Usage { usage: reported.clone() });
                                usage = reported;
                            }
                            StreamEvent::Error { message, retryable } => {
                                stream_log.push(StreamEvent::Error { message: message.clone(), retryable });
                                failure = Some((message, "transport", retryable));
                            }
                            StreamEvent::VendorEvent { kind, payload } => {
                                stream_log.push(StreamEvent::VendorEvent { kind: kind.clone(), payload: payload.clone() });
                                if kind == lca_protocol::THINKING_SIGNATURE_KIND {
                                    note_signature(&mut signature, &payload);
                                }
                                tracing::debug!(%kind, %payload, "vendor event");
                            }
                        }
                    }
                    natural_end = failure.is_none();
                    break;
                }
            }
        }

        self.replay_stream(&model, stream_log).await;
        if let Some((message, class, retryable)) = failure {
            return Err(CallFail::Provider {
                message,
                class: class.to_string(),
                retryable,
            });
        }
        let (mut calls, protocol_errors) = acc.finish(natural_end);
        // Pi-name aliases dispatch as their canonical tool (gh #119):
        // one seam, before records, permission, and dispatch see the name.
        for call in &mut calls {
            call.name = lca_tools::canonical_tool_name(&call.name).to_string();
        }
        Ok(CallResponse {
            text,
            reasoning: if reasoning.is_empty() {
                None
            } else {
                Some(reasoning)
            },
            signature,
            calls,
            protocol_errors,
            usage,
        })
    }
}

// gh #41: the latest thinking signature (one reasoning run carries
// one; a later block replaces an earlier one, matching the transcript
// order replay resends).
fn note_signature(signature: &mut Option<String>, payload: &serde_json::Value) {
    if let Some(bytes) = payload.get("signature").and_then(|value| value.as_str()) {
        *signature = Some(bytes.to_string());
    }
}
