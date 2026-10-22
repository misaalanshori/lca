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
mod chat_ext_commands;
mod chat_extman;
mod chat_hub;
mod chat_keys;
/// The `/mcp` manager modal (gh #418 Phase G).
mod chat_mcp;
mod chat_mcp_keys;
mod chat_mcp_render;
mod chat_modals;
mod chat_models;
mod chat_mouse;
mod chat_notices;
mod chat_overlays;
mod chat_pickers;
mod chat_poll;
mod chat_project_override;
mod chat_queue;
mod chat_render;
mod chat_search;
mod chat_shell;
mod chat_tabs;
mod chat_tree;
mod command_help_table;
pub mod dialogs;
pub mod ext_widgets;
pub mod footer;
mod mcp_types;
pub mod render;
pub mod resume;
pub mod run;
pub mod run_early_input;
mod run_keybindings;
mod run_selection;
mod run_status;
pub mod separator;
pub mod state;
mod status;
pub mod theme;
pub mod transcript;
mod turn_metrics;
pub mod ui_state;

pub use chat::{Chat, PendingMessage};
pub use chat_extman::{
    ExtensionManager, ListManaged, ManagedData, ManagedEntry, SetManagedStrategy, ToggleManaged,
};
pub use chat_hub::{ConfirmInstall, FetchExtensions, HubData, InstallConsent, InstallPoll};
pub use chat_mcp::{
    EXPOSURE_LEVELS, McpActionRow, McpManager, McpScreen, detail_actions, exposure_options,
    stderr_rows,
};
pub use chat_pickers::HubRow;
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
pub use separator::{CompactionReason, Separator, SeparatorState};
pub use state::{
    Action, CacheMissNotice, CommandInvoker, DialogExchange, DialogModal, LabelRecord, LoginCancel,
    LoginComplete, LoginConfirm, LoginNext, LoginPick, LoginPoll, LoginRequest, McpAction,
    McpBeginSignIn, McpCancelSignIn, McpDetail, McpList, McpPoll, McpServerDetail, McpServerRow,
    McpSnapshot, McpSubmitRedirect, McpToolRow, ModelRow, PickerOption, ProjectOverride,
    ProjectSettingPersist, PromptRequest, RegionInteractor, RegionRenderer, SettingRow,
    SettingsRows, ShellEvent, ShellHandle, ShellRunner, SwitchConfirm, TreeFilterMode, TreeRow,
    TreeRowKind, TurnChannels, TurnRunner, UiHooks, UiOptions, UiState, display_path,
    sanitize_block, sanitize_text,
};
pub use state::{CompactPoll, CompactState};
pub use theme::Theme;
pub use transcript::{Entry, EntryHit, ToolStatus, Transcript, image_label};
