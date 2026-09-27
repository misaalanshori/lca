//! The agent interface on the terminal engine, ported from pi's
//! `coding-agent/src/modes/interactive/`.
//!
//! Boundary rule (ADR-0036): this crate may depend on `lca-core`,
//! `lca-session`, `lca-protocol`, and `lca-tui`; `lca-tui` may not depend
//! on any of them. The theme, transcript, footer, chat composition, and the
//! interactive loop live here.

pub mod app;
pub mod chat;
pub mod footer;
pub mod theme;
pub mod transcript;

pub use app::{App, AppOptions, PromptRequest, TurnChannels, TurnRunner, run};
pub use chat::Chat;
pub use footer::Footer;
pub use theme::Theme;
pub use transcript::{Entry, ToolStatus, Transcript};
