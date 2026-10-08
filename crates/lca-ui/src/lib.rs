//! The agent interface on the terminal engine, ported from pi's
//! `coding-agent/src/modes/interactive/`.
//!
//! Boundary rule (ADR-0036): this crate may depend on `lca-core`,
//! `lca-session`, `lca-protocol`, and `lca-tui`; `lca-tui` may not depend
//! on any of them. The theme, transcript, footer, chat composition, and the
//! interactive loop live here.

pub mod chat;
mod chat_commands;
mod chat_confirm;
mod chat_keys;
mod chat_mouse;
mod chat_overlays;
mod chat_pickers;
mod chat_render;
mod chat_search;
mod chat_shell;
pub mod dialogs;
pub mod ext_widgets;
pub mod footer;
pub mod render;
pub mod resume;
pub mod run;
pub mod separator;
pub mod state;
pub mod theme;
pub mod transcript;
mod turn_metrics;

pub use chat::{Chat, PendingMessage};
pub use chat_pickers::ModelPicker;
pub use chat_render::ClickOutcome;
pub use ext_widgets::widget_lines;
pub use footer::Footer;
pub use lca_protocol::{QueuedMessage, SubmitMode, steer_queue};
/// The markdown pre-parse transform seam (gh #12): the pipeline's own
/// types, re-exported so hosts collect native transforms without naming
/// the widget path.
pub use lca_tui::widgets::markdown::{
    MarkdownMessageType, MarkdownTransformContext, MarkdownTransformer,
};
pub use run::run;
pub use separator::{Separator, SeparatorState};
pub use state::{
    Action, CommandInvoker, DialogExchange, DialogModal, LoginCancel, LoginComplete, LoginConfirm,
    LoginNext, LoginPick, LoginPoll, LoginRequest, ModelRow, PickerOption, PromptRequest,
    RegionInteractor, RegionRenderer, SettingRow, SettingsRows, ShellEvent, ShellHandle,
    ShellRunner, SwitchConfirm, TurnChannels, TurnRunner, UiHooks, UiOptions, UiState,
    display_path, sanitize_block, sanitize_text,
};
pub use state::{CompactPoll, CompactState};
pub use theme::Theme;
pub use transcript::{Entry, EntryHit, ToolStatus, Transcript, image_label};
