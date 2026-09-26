//! Shared data types for LCA: messages, tool calls, tool results, stream
//! events, and session-log records.
//!
//! Every other workspace crate speaks these types. This crate performs no
//! input or output (ADR-0002).

#![forbid(unsafe_code)]

pub mod capability;
pub mod dispatch;
pub mod login;
pub mod message;
pub mod provider;
pub mod record;
pub mod stream;
pub mod tool;
pub mod turn;
pub mod ui;
pub mod usage;

pub use capability::CapabilityError;
pub use dispatch::{CommandEffect, CommandSpec, DispatchError, HookAction, PostToolObservation};
pub use login::{LoginAnswer, LoginOption};
pub use message::{ChatMessage, ContentBlock, MessageRole, base64_encode, sniff_image_media_type};
pub use provider::{
    CompletionRequest, EventSink, IdentityOutcome, ModelInfo, OauthCap, ProviderCap,
};
pub use record::{FORMAT_VERSION, PermissionDecision, Record, ToolSource};
pub use stream::StreamEvent;
pub use tool::{ToolCall, ToolResult, ToolResultStatus, ToolSpec};
pub use turn::{StopReason, TurnEvent, TurnOutcome, TurnStatus};
pub use ui::{UiEffect, UiInput, Widget, WidgetTree};
pub use usage::Usage;
