//! The scripted fake provider, modeled on pi's `providers/faux.ts`
//! (testing plan section 3), plus the sandboxed test environment fixture
//! (testing plan section 5).
//!
//! # Unsafe-code exemption
//!
//! `unsafe` is denied crate-wide and allowed only inside [`fixture`], because
//! mutating the process environment is an `unsafe fn` in edition 2024 and the
//! testing plan requires a sandboxed `HOME`. The fixture serializes every
//! environment user behind one lock; each `unsafe` block carries a `SAFETY`
//! comment. Review: phase 1 review.

#![deny(unsafe_code)]

pub mod fixture;

use std::collections::VecDeque;
use std::sync::Mutex;

use lca_protocol::{StreamEvent, Usage};
use lca_provider::{CompletionRequest, EventSender, ModelInfo, ProviderError};

pub use fixture::TestEnv;
pub use lca_provider::Provider;

/// Usage with cache fields set explicitly: every scripted turn carries one.
pub fn fake_usage(input: u64, output: u64, cache_read: u64, cache_write: u64) -> Usage {
    Usage {
        input,
        output,
        cache_read,
        cache_write,
        cache_write_1h: 0,
        cost: 0.0,
        cost_input: 0.0,
        cost_cache_read: 0.0,
        cost_cache_write: 0.0,
        extras: Default::default(),
    }
}

/// One step of a scripted turn: an event, or a pause before the next step.
#[derive(Debug, Clone)]
pub enum Step {
    /// A stream event.
    Event(StreamEvent),
    /// Sleep before continuing, so cancellation tests can land mid-stream.
    Pause(std::time::Duration),
}

/// Builds one scripted turn. Usage is mandatory (testing plan section 3).
#[derive(Debug, Default)]
pub struct TurnBuilder {
    events: Vec<Step>,
    usage: Option<Usage>,
}

impl TurnBuilder {
    /// Append a text delta.
    pub fn text(mut self, text: &str) -> Self {
        self.events.push(Step::Event(StreamEvent::TextDelta {
            delta: text.to_string(),
        }));
        self
    }

    /// Append a reasoning delta.
    pub fn reasoning(mut self, text: &str) -> Self {
        self.events.push(Step::Event(StreamEvent::ReasoningDelta {
            delta: text.to_string(),
        }));
        self
    }

    /// A complete tool call: start, one argument fragment, end.
    pub fn tool_call(mut self, name: &str, arguments: &str) -> Self {
        let call_id = format!("call-{}", self.events.len());
        self.events.push(Step::Event(StreamEvent::ToolCallStart {
            call_id: call_id.clone(),
            name: name.to_string(),
        }));
        self.events.push(Step::Event(StreamEvent::ToolCallArgDelta {
            call_id: call_id.clone(),
            delta: arguments.to_string(),
        }));
        self.events
            .push(Step::Event(StreamEvent::ToolCallEnd { call_id }));
        self
    }

    /// A tool call that never closes: the accumulator failure path
    /// (`docs/flows.md`).
    pub fn open_tool_call(mut self, name: &str, arguments_prefix: &str) -> Self {
        let call_id = format!("call-{}", self.events.len());
        self.events.push(Step::Event(StreamEvent::ToolCallStart {
            call_id: call_id.clone(),
            name: name.to_string(),
        }));
        self.events.push(Step::Event(StreamEvent::ToolCallArgDelta {
            call_id,
            delta: arguments_prefix.to_string(),
        }));
        self
    }

    /// An error event, retryable or not (FR-CORE-6 needs both).
    pub fn error(mut self, message: &str, retryable: bool) -> Self {
        self.events.push(Step::Event(StreamEvent::Error {
            message: message.to_string(),
            retryable,
        }));
        self
    }

    /// A capability denial: a vendor event plus a non-retryable error, the
    /// shape an extension-mediated provider reports.
    pub fn capability_denied(mut self, capability: &str, target: &str) -> Self {
        self.events.push(Step::Event(StreamEvent::VendorEvent {
            kind: "capability-denied".to_string(),
            payload: serde_json::json!({ "capability": capability, "target": target }),
        }));
        self.events.push(Step::Event(StreamEvent::Error {
            message: format!("capability {capability} denied for {target}"),
            retryable: false,
        }));
        self
    }

    /// Pause before the next step: makes mid-stream cancellation testable.
    pub fn pause(mut self, millis: u64) -> Self {
        self.events
            .push(Step::Pause(std::time::Duration::from_millis(millis)));
        self
    }

    /// The mandatory usage record for this turn.
    pub fn usage(mut self, usage: Usage) -> Self {
        self.usage = Some(usage);
        self
    }

    fn finish(self) -> (Vec<Step>, Option<Usage>) {
        (self.events, self.usage)
    }
}

/// Assembles a provider from scripted turns.
#[derive(Debug, Default)]
pub struct FakeBuilder {
    turns: Vec<(Vec<Step>, Option<Usage>)>,
}

impl FakeBuilder {
    /// Script one turn with the builder closure.
    pub fn turn(mut self, build: impl FnOnce(TurnBuilder) -> TurnBuilder) -> Self {
        self.turns.push(build(TurnBuilder::default()).finish());
        self
    }

    /// Build the provider; panics when a turn lacks its usage record.
    pub fn build(self) -> FakeProvider {
        let turns = self
            .turns
            .into_iter()
            .enumerate()
            .map(|(index, (mut events, usage))| {
                let usage = usage
                    .unwrap_or_else(|| panic!("turn {index} is missing a mandatory usage record"));
                events.push(Step::Event(StreamEvent::Usage { usage }));
                events
            })
            .collect::<Vec<_>>();
        FakeProvider {
            turns: Mutex::new(VecDeque::from(turns)),
            call_count: std::sync::atomic::AtomicUsize::new(0),
            last_request: Mutex::new(None),
        }
    }
}

/// A deterministic provider: scripted turns, no network, no credentials.
pub struct FakeProvider {
    turns: Mutex<VecDeque<Vec<Step>>>,
    call_count: std::sync::atomic::AtomicUsize,
    last_request: Mutex<Option<CompletionRequest>>,
}

impl FakeProvider {
    /// Start scripting.
    pub fn builder() -> FakeBuilder {
        FakeBuilder::default()
    }

    /// The last request the core handed this provider (FR-CACHE-5 checks).
    pub fn last_request(&self) -> Option<CompletionRequest> {
        self.last_request.lock().expect("request lock").clone()
    }

    /// How many completion calls happened, including exhausted ones.
    pub fn call_count(&self) -> usize {
        self.call_count.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Run one scripted turn to completion on a current-thread runtime and
    /// collect its events: the sync convenience tests use.
    pub fn run_next_turn_blocking(&self) -> Vec<StreamEvent> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");
        runtime.block_on(self.run_one(CompletionRequest {
            model: "faux-1".to_string(),
            ..CompletionRequest::default()
        }))
    }

    async fn run_one(&self, request: CompletionRequest) -> Vec<StreamEvent> {
        let (tx, mut rx) = tokio::sync::mpsc::channel(64);
        self.stream(request, tx).await.expect("fake stream runs");
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        events
    }
}

impl Provider for FakeProvider {
    fn name(&self) -> &str {
        "fake"
    }

    fn list_models(&self) -> Vec<ModelInfo> {
        vec![ModelInfo {
            id: "faux-1".to_string(),
            name: "Faux Model".to_string(),
            context_window: 128_000,
            max_tokens: 16_384,
        }]
    }

    fn stream(
        &self,
        request: CompletionRequest,
        tx: EventSender,
    ) -> lca_provider::BoxFuture<Result<(), ProviderError>> {
        self.call_count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        *self.last_request.lock().expect("request lock") = Some(request);
        let events = self
            .turns
            .lock()
            .expect("turn lock")
            .pop_front()
            .unwrap_or_else(|| {
                vec![Step::Event(StreamEvent::Error {
                    message: "no more scripted responses queued".to_string(),
                    retryable: false,
                })]
            });
        Box::pin(async move {
            for step in events {
                match step {
                    Step::Pause(duration) => tokio::time::sleep(duration).await,
                    Step::Event(event) => {
                        if tx.send(event).await.is_err() {
                            return Ok(()); // host stopped listening: cancellation
                        }
                    }
                }
            }
            Ok(())
        })
    }
}
