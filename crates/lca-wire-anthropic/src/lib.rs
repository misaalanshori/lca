//! The shared Anthropic-family wire protocol kit (gh #189): the
//! Messages request builder and SSE decoder. Builds for native targets
//! and `wasm32-wasip2` alike (no host imports); `lca-core` never
//! depends on it.
//!
//! # The shared failure type
//!
//! `StreamFailure` is re-exported from [`lca_protocol`], not defined here (gh #576).
//! The kit used to carry its own copy, with its own `From<CapabilityError>`; the two
//! copies agreed, which is exactly the drift that one definition prevents. This kit
//! has **no transport**: it builds bodies and decodes frames, so it never acquires a
//! host handle - the `ResponseGuard` rule in `lca-wire-openai`'s docs applies to the
//! provider extension that drives it (`extensions/anthropic`'s `MessagesDriver`).

pub mod messages;

pub use messages::{
    AnthropicStream, DEFERRED_PLACEHOLDER, INLINE_TOOLS_BETA, StreamFailure,
    THINKING_SIGNATURE_KIND, build_messages_body, frozen_tools, message_usage, parse_sse,
    tool_addition_block, tool_removal_block,
};
