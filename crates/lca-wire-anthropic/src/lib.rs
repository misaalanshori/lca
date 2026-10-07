//! The shared Anthropic-family wire protocol kit (gh #189): the
//! Messages request builder and SSE decoder. Builds for native targets
//! and `wasm32-wasip2` alike (no host imports); `lca-core` never
//! depends on it.

pub mod messages;

pub use messages::{
    AnthropicStream, THINKING_SIGNATURE_KIND, build_messages_body, message_usage, parse_sse,
};
