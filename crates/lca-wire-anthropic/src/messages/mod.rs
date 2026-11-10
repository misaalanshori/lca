//! The Anthropic Messages wire engine (`POST /v1/messages`, gh #189), split by
//! layer: the request builder in `body`, the SSE decoder in `stream`.
//!
//! **A cohesion split, not a ceiling one** - the file was 655 lines, nowhere near
//! gate 11's 1,200. The builder and the decoder have different failure modes (a
//! wire shape versus a decoder state machine), and the split puts the mapper's
//! thinking-signature rule next to the code that emits it (gh #41).
//!
//! Every public path is unchanged: this barrel re-exports what `messages.rs` did,
//! so `lca_wire_anthropic::messages::{build_messages_body, AnthropicStream,
//! parse_sse, message_usage, frozen_tools, tool_addition_block,
//! tool_removal_block, INLINE_TOOLS_BETA, DEFERRED_PLACEHOLDER,
//! THINKING_SIGNATURE_KIND, StreamFailure}` resolve exactly as before.

mod body;
mod stream;

pub use body::{
    DEFERRED_PLACEHOLDER, INLINE_TOOLS_BETA, build_messages_body, frozen_tools,
    tool_addition_block, tool_removal_block,
};
pub use stream::{AnthropicStream, message_usage, parse_sse};

/// A provider call failed: re-exported from [`lca_protocol`] (gh #576), where the
/// one definition and its one `From<CapabilityError>` live.
pub use lca_protocol::StreamFailure;
/// Thinking-signature vendor-event kind: re-exported from [`lca_protocol`], where
/// the host matches it.
pub use lca_protocol::THINKING_SIGNATURE_KIND;
