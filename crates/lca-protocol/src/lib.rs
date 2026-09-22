//! Shared data types for LCA: messages, tool calls, tool results, stream
//! events, and session-log records.
//!
//! Every other workspace crate speaks these types. This crate performs no
//! input or output (ADR-0002).

#![forbid(unsafe_code)]

pub mod capability;
pub mod dispatch;
pub mod message;
pub mod record;
pub mod stream;
pub mod tool;
pub mod usage;

pub use capability::CapabilityError;
pub use dispatch::{CommandEffect, CommandSpec, DispatchError, HookAction, PostToolObservation};
pub use message::{ChatMessage, ContentBlock, MessageRole};
pub use record::{FORMAT_VERSION, PermissionDecision, Record, ToolSource};
pub use stream::StreamEvent;
pub use tool::{ToolCall, ToolResult, ToolResultStatus, ToolSpec};
pub use usage::Usage;
