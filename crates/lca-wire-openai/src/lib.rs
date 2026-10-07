//! The shared OpenAI-family wire protocol kit (gh #189): the Chat
//! Completions SSE decoder and request mappers plus the Responses
//! request builder and event mapper. Pure protocol — provider
//! differences (endpoints, headers, account, credential lifecycle)
//! stay in the extensions. Builds for native targets and
//! `wasm32-wasip2` alike (no host imports); `lca-core` never depends
//! on it.

pub mod chat_completions;
pub mod error;
pub mod responses;

pub use chat_completions::{SseDecoder, map_usage, parse_sse, to_wire, tools_wire};
pub use error::{StreamFailure, classify_status, failure_for_status, json_error_message};
pub use responses::{ResponseStreamDriver, ResponsesStream, build_responses_body, responses_usage};
