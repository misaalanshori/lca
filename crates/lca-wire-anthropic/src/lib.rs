//! The shared Anthropic-family wire protocol kit (gh #189): the
//! Messages request builder and SSE decoder. Builds for native targets
//! and `wasm32-wasip2` alike (no host imports); `lca-core` never
//! depends on it.

pub mod messages;

pub use messages::{
    AnthropicStream, DEFERRED_PLACEHOLDER, INLINE_TOOLS_BETA, THINKING_SIGNATURE_KIND,
    build_messages_body, frozen_tools, message_usage, parse_sse, tool_addition_block,
    tool_removal_block,
};
