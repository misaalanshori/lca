//! The Responses wire engine (`POST /v1/responses`, gh #63/#189), in three
//! layers rather than one file.
//!
//! * `body` - the request body: model, instructions, input items, tools, and the
//!   reasoning effort, plus the usage reader. Nothing here touches the host.
//! * `stream` - the SSE event mapper: `ResponsesStream`, its frame handling, and
//!   the vendor-envelope error.
//! * `driver` - the transport: `ResponseStreamDriver` over the host's
//!   `ProviderCap`, and the guard that owns the response handle (gh #577).
//!
//! **This is a cohesion split, not a ceiling one** - the file was 528 lines, nowhere
//! near gate 11's 1,200. The three layers have different failure modes (a wire shape,
//! a decoder state machine, a resource lifetime), and the lifetime rule gh #577 added
//! reads better beside the type that enforces it.
//!
//! Every public path is unchanged: this barrel re-exports what `responses.rs` did, so
//! `lca_wire_openai::responses::{build_responses_body, responses_usage, ResponsesStream,
//! ResponseStreamDriver}` resolve exactly as before.

mod body;
mod driver;
mod stream;

pub use body::{build_responses_body, responses_usage};
pub use driver::ResponseStreamDriver;
pub use stream::ResponsesStream;
