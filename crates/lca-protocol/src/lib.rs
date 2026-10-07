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
    CompletionRequest, EventSink, IMAGE_RESIZE_EXTRA, IMAGE_VISION_EXTRA, IdentityOutcome,
    ImageResize, ModelInfo, OauthCap, PROMPT_CACHE_EXTRA, ProviderCap,
};
pub use record::{
    FORMAT_VERSION, PREVIOUS_SUMMARY_TYPE, PermissionDecision, Record, ToolSource,
    is_previous_summary,
};
pub use stream::StreamEvent;
pub use tool::{ImageContent, ToolCall, ToolResult, ToolResultStatus, ToolSpec};
pub use turn::{
    QueuedMessage, SteerQueue, StopReason, SubmitMode, TurnEvent, TurnOutcome, TurnStatus,
    steer_queue,
};
pub use ui::{DialogAnswer, TextStyle, UiDialog, UiEffect, UiInput, Widget, WidgetTree};
pub use usage::Usage;
