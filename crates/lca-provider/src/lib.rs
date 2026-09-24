//! The provider trait, streaming types, and the host-side tool-call
//! accumulator.
//!
//! The core calls providers through this trait; the accumulator here joins
//! argument fragments keyed by call identifier, because the host is the side
//! that knows the tool schema (ADR-0004). No vendor logic lives in this
//! crate (FR-PROV-1).

#![forbid(unsafe_code)]

use std::future::Future;
use std::pin::Pin;

use lca_protocol::{StreamEvent, ToolCall};

// The provider-world data types live in the protocol crate now that the
// dispatch trait needs them; re-exported here so existing call sites do
// not change.
pub use lca_protocol::{CompletionRequest, ModelInfo};

/// A boxed future, the hand-rolled async surface that keeps this crate on
/// the documented dependency list.
pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// Where a provider's events go: a bounded channel the host drains while
/// rendering (streaming is host-driven polling, ADR-0004).
pub type EventSender = tokio::sync::mpsc::Sender<StreamEvent>;

/// How a provider call failed. `retryable` decides whether the core retries
/// (FR-CORE-6) and feeds the headless `error` envelope's class.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct ProviderError {
    /// What went wrong, human-readable.
    pub message: String,
    /// Class for the headless envelope (`docs/headless.md`).
    pub class: &'static str,
    /// Whether a retry could succeed (FR-CORE-6).
    pub retryable: bool,
}

impl ProviderError {
    /// A retryable transport failure.
    pub fn transport(message: impl Into<String>) -> Self {
        ProviderError {
            message: message.into(),
            class: "transport",
            retryable: true,
        }
    }

    /// A permanent authentication failure.
    pub fn auth(message: impl Into<String>) -> Self {
        ProviderError {
            message: message.into(),
            class: "auth",
            retryable: false,
        }
    }

    /// A permanent invalid-request failure.
    pub fn invalid(message: impl Into<String>) -> Self {
        ProviderError {
            message: message.into(),
            class: "invalid",
            retryable: false,
        }
    }
}

/// What the core calls to obtain a completion. The Phase 1 shape covers the
/// compiled-in backend and the fake (ADR-0013); Phase 3 wraps this same
/// trait around the `provider` world's ABI.
pub trait Provider: Send + Sync {
    /// Extension-style name, e.g. `openai-compatible`.
    fn name(&self) -> &str;

    /// Models to offer in the picker (FR-PROV-2).
    fn list_models(&self) -> Vec<ModelInfo>;

    /// Start a completion: push typed events into `tx` as they arrive, then
    /// return. Dropping the sender (or aborting the task that runs this
    /// future) cancels the stream (FR-CONC-3).
    fn stream(
        &self,
        request: CompletionRequest,
        tx: EventSender,
    ) -> BoxFuture<Result<(), ProviderError>>;
}

/// Failures the host records when a provider's stream breaks the protocol.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProtocolError {
    /// An argument delta arrived with no open start (FR-PROV-8).
    #[error("argument delta for `{call_id}` with no open start event")]
    DeltaWithoutStart {
        /// The offending call identifier.
        call_id: String,
    },
    /// An end event arrived with no open start.
    #[error("tool call end for `{call_id}` with no open start event")]
    EndWithoutStart {
        /// The offending call identifier.
        call_id: String,
    },
    /// Two starts for one identifier (ADR-0004 failure-path list).
    #[error("duplicate tool call start for `{call_id}`")]
    DuplicateStart {
        /// The duplicated call identifier.
        call_id: String,
    },
    /// The stream ended while a call was still open; the call is discarded
    /// (`docs/flows.md`, streaming pipeline).
    #[error("stream ended with tool call `{call_id}` still open")]
    StreamEndedWithOpenCall {
        /// The abandoned call identifier.
        call_id: String,
    },
}

#[derive(Debug, Clone)]
struct OpenCall {
    call_id: String,
    name: String,
    arguments: String,
}

/// Accumulates streamed tool-call fragments keyed by call identifier
/// (ADR-0004: argument accumulation belongs to the host).
#[derive(Debug, Default)]
pub struct ToolCallAccumulator {
    open: Vec<OpenCall>,
    closed: Vec<ToolCall>,
    errors: Vec<ProtocolError>,
}

impl ToolCallAccumulator {
    /// Feed one stream event; only the three tool-call cases matter here.
    pub fn handle(&mut self, event: StreamEvent) {
        match event {
            StreamEvent::ToolCallStart { call_id, name } => {
                if let Some(existing) = self.open.iter_mut().find(|c| c.call_id == call_id) {
                    self.errors.push(ProtocolError::DuplicateStart {
                        call_id: call_id.clone(),
                    });
                    existing.name = name;
                    existing.arguments.clear();
                } else {
                    self.open.push(OpenCall {
                        call_id,
                        name,
                        arguments: String::new(),
                    });
                }
            }
            StreamEvent::ToolCallArgDelta { call_id, delta } => {
                match self.open.iter_mut().find(|c| c.call_id == call_id) {
                    Some(call) => call.arguments.push_str(&delta),
                    None => self
                        .errors
                        .push(ProtocolError::DeltaWithoutStart { call_id }),
                }
            }
            StreamEvent::ToolCallEnd { call_id } => {
                match self.open.iter().position(|c| c.call_id == call_id) {
                    Some(index) => {
                        let call = self.open.remove(index);
                        self.closed.push(ToolCall {
                            call_id: call.call_id,
                            name: call.name,
                            arguments: call.arguments,
                        });
                    }
                    None => self.errors.push(ProtocolError::EndWithoutStart { call_id }),
                }
            }
            _ => {}
        }
    }

    /// Wrap up after the stream: `stream_ended` reports whether the stream
    /// reached its natural end. Any call still open is discarded and
    /// reported, never run with partial arguments.
    pub fn finish(self, stream_ended: bool) -> (Vec<ToolCall>, Vec<ProtocolError>) {
        let mut errors = self.errors;
        if stream_ended {
            for call in self.open {
                errors.push(ProtocolError::StreamEndedWithOpenCall {
                    call_id: call.call_id,
                });
            }
        }
        (self.closed, errors)
    }

    /// Errors recorded so far, without consuming the accumulator.
    pub fn errors(&self) -> &[ProtocolError] {
        &self.errors
    }
}

/// Parse a completed call's joined argument string: the host parses once,
/// after joining (ADR-0004).
pub fn validate_arguments(call: &ToolCall) -> Result<serde_json::Value, serde_json::Error> {
    serde_json::from_str(&call.arguments)
}

/// Validate a tool call's arguments against its JSON-schema `parameters`
/// before the host calls `execute` (extension authoring guide). Covers the
/// subset the project's tools declare - `object`/`array`/`string`/`integer`/
/// `number`/`boolean` types, `required`, and `items` - and names the offending
/// field. Unknown type keywords are ignored rather than rejected.
pub fn validate_against_schema(schema: &serde_json::Value, arguments: &str) -> Result<(), String> {
    let value: serde_json::Value =
        serde_json::from_str(arguments).map_err(|err| format!("not valid JSON: {err}"))?;
    validate_value(schema, &value, "$")
}

fn validate_value(
    schema: &serde_json::Value,
    value: &serde_json::Value,
    path: &str,
) -> Result<(), String> {
    let Some(expected) = schema.get("type").and_then(serde_json::Value::as_str) else {
        return Ok(());
    };
    let matches = match expected {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "integer" => value.is_i64() || value.is_u64(),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        _ => true,
    };
    if !matches {
        return Err(format!("{path} must be {expected}"));
    }
    match (expected, value) {
        ("object", serde_json::Value::Object(map)) => {
            if let Some(required) = schema.get("required").and_then(|r| r.as_array()) {
                for key in required.iter().filter_map(serde_json::Value::as_str) {
                    if !map.contains_key(key) {
                        return Err(format!("{path}.{key} is required"));
                    }
                }
            }
            if let Some(properties) = schema.get("properties").and_then(|p| p.as_object()) {
                for (key, subschema) in properties {
                    if let Some(sub) = map.get(key) {
                        validate_value(subschema, sub, &format!("{path}.{key}"))?;
                    }
                }
            }
        }
        ("array", serde_json::Value::Array(items)) => {
            if let Some(item_schema) = schema.get("items") {
                for (index, item) in items.iter().enumerate() {
                    validate_value(item_schema, item, &format!("{path}[{index}]"))?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}
