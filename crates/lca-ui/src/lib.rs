//! The agent interface on the terminal engine, ported from pi's
//! `coding-agent/src/modes/interactive/`.
//!
//! Boundary rule (ADR-0036): this crate may depend on `lca-core`,
//! `lca-session`, `lca-protocol`, and `lca-tui`; `lca-tui` may not depend
//! on any of them. The theme, transcript, footer, chat composition, and the
//! interactive loop live here.

pub mod chat;
mod chat_commands;
mod chat_keys;
mod chat_overlays;
mod chat_pickers;
mod chat_shell;
pub mod footer;
pub mod render;
pub mod resume;
pub mod run;
pub mod state;
pub mod theme;
pub mod transcript;

pub use chat::{Chat, PendingMessage};
pub use footer::Footer;
pub use lca_protocol::{QueuedMessage, SubmitMode, steer_queue};
pub use run::run;
pub use state::{
    Action, CUSTOM_OPTION, CommandInvoker, LoginCancel, LoginComplete, LoginConfirm, LoginNext,
    LoginPick, LoginPoll, LoginRequest, PickerOption, PromptRequest, RegionInteractor,
    RegionRenderer, ShellEvent, ShellHandle, ShellRunner, TurnChannels, TurnRunner, UiHooks,
    UiOptions, UiState, display_path, sanitize_block, sanitize_text, widget_lines,
};
pub use theme::Theme;
pub use transcript::{Entry, ToolStatus, Transcript, image_label};
