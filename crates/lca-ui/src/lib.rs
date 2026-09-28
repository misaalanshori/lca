//! The agent interface on the terminal engine, ported from pi's
//! `coding-agent/src/modes/interactive/`.
//!
//! Boundary rule (ADR-0036): this crate may depend on `lca-core`,
//! `lca-session`, `lca-protocol`, and `lca-tui`; `lca-tui` may not depend
//! on any of them. The theme, transcript, footer, chat composition, and the
//! interactive loop live here.

// R17: production paths stay panic-free (test code may unwrap; clippy.toml
// scopes the lints away from tests).
#![warn(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

pub mod chat;
mod chat_overlays;
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
    Action, CUSTOM_OPTION, CommandInvoker, LoginComplete, LoginConfirm, LoginNext, LoginPick,
    LoginRequest, PickerOption, PromptRequest, RegionInteractor, RegionRenderer, TurnChannels,
    TurnRunner, UiHooks, UiOptions, UiState, sanitize_block, sanitize_text, widget_lines,
};
pub use theme::Theme;
pub use transcript::{Entry, ToolStatus, Transcript, image_label};
