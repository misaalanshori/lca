//! The shared OpenAI-family wire protocol kit (gh #189): the Chat
//! Completions SSE decoder and request mappers plus the Responses
//! request builder and event mapper. Pure protocol — provider
//! differences (endpoints, headers, account, credential lifecycle)
//! stay in the extensions. Builds for native targets and
//! `wasm32-wasip2` alike (no host imports); `lca-core` never depends
//! on it.
//!
//! # The shared failure type, and the RAII rule for a host handle
//!
//! `StreamFailure` lives in [`lca_protocol`], beside `CapabilityError`, and this kit
//! re-exports it. One definition and one `From<CapabilityError>` is the point: the
//! type used to exist twice (here and in the Anthropic kit) with two classification
//! impls that happened to agree, which is luck rather than design (gh #576). The
//! fields stay **public** because 45 struct-literal sites in this tree build one and
//! a mechanical sweep is its own cycle; the constructors (`invalid`, `auth`,
//! `transport`, `new`) are the shape new code uses.
//!
//! **A host handle acquired from a capability and followed by fallible calls gets a
//! guard on the line after the call that produced it.** `ResponseStreamDriver::open`
//! gets its handle from `net_request`, then `?`s on `net_response_status` and inside
//! the non-2xx `net_read_body` loop *before* the driver exists - so the driver's own
//! `Drop` cannot own it yet, and closing only on the success paths leaked the handle
//! once per failed request (gh #577). `ResponseGuard` owns it from that line on.
//!
//! The guard is [`lca_protocol::ResponseGuard`], not a private struct here: eight
//! provider extensions open a response with this same shape, and one of them drives
//! the Anthropic kit, which has no transport. A guard used by eight extensions but
//! exported by none is a type eight extensions copy.

pub mod chat_completions;
pub mod error;
pub mod responses;
pub mod sse;

pub use chat_completions::{SseDecoder, map_usage, parse_sse, to_wire, tools_wire};
pub use error::{StreamFailure, classify_status, failure_for_status, json_error_message};
pub use responses::{ResponseStreamDriver, ResponsesStream, build_responses_body, responses_usage};
