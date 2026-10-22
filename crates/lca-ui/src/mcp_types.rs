//! The `/mcp` modal's data types (gh #418 Phase G), split from `state.rs` for
//! the 1,200-line ceiling.
//!
//! These are the interface's own vocabulary, not `lca-mcp`'s: the host converts
//! between them in `lca-cli`'s hooks. That is deliberate - `lca-mcp` answers with
//! data (ADR-0047 forbids it rendering) and `lca-ui` cannot name the subsystem's
//! types (the architecture test forbids the edge), so the host is the only crate
//! allowed to know both, and keeping the two type sets distinct makes that
//! boundary visible instead of hiding it behind a re-export.

use std::sync::Arc;

/// One server as the `/mcp` modal lists it (gh #418 Phase G).
///
/// Data only: `lca-mcp` never hands over a rendered line (ADR-0047 - a
/// subsystem that renders is a subsystem that depends upward, and the
/// architecture test pins the missing edge). Every string here is a value the
/// modal composes into a row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerRow {
    /// pi's server name.
    pub name: String,
    /// The state word the modal sorts and labels by: `needs-auth`, `failed`,
    /// `disconnected`, `connecting`, `connected`, or `disabled`.
    pub state: String,
    /// The state as a sentence, e.g. `failed: handshake timed out`.
    pub detail: String,
    /// The **effective** exposure (what the model actually gets), which is not
    /// always the configured one - a project override moves it.
    pub effective_exposure: String,
    /// Where the entry is defined: `user`, `project`, or `inline`.
    pub scope: String,
    /// Whether this row carries a project override.
    pub project_override: bool,
}

/// Everything the modal's detail screen shows about one server (gh #418 §3.4
/// item 2). One fetch, so a background reconnect cannot change the state between
/// two questions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerDetail {
    /// The server name.
    pub name: String,
    /// The state word, as in [`McpServerRow::state`].
    pub state: String,
    /// The state as a sentence, including the failure text when there is one.
    pub detail: String,
    /// The command line (stdio) or URL (http), for the header.
    pub target: String,
    /// The file that defines this entry.
    pub source_path: String,
    /// The project override's path, when one exists.
    pub override_path: Option<String>,
    /// The **effective** exposure.
    pub effective_exposure: String,
    /// The exposure written in the defining file, which can differ.
    pub configured_exposure: String,
    /// Whether the server is switched on.
    pub enabled: bool,
    /// Whether this entry has a project override.
    pub project_override: bool,
    /// Whether the project-scoped toggle applies (a user-level server in a
    /// trusted project).
    pub project_toggle_available: bool,
    /// The server wants a sign-in.
    pub needs_sign_in: bool,
    /// The server is configured for OAuth (so `Sign out` has something to do).
    pub has_oauth: bool,
    /// The child's stderr tail, when a stdio server failed.
    pub stderr_tail: Option<String>,
    /// Every tool the server offers, with its effective exposure.
    pub tools: Vec<McpToolRow>,
}

/// One tool in the modal's inspector (gh #418 §3.4 item 2, `Tools`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpToolRow {
    /// The qualified name the model calls (`mcp__<server>__<tool>`).
    pub name: String,
    /// The **effective** exposure for this tool, after the server default and
    /// any per-tool override.
    pub exposure: String,
    /// The description the model reads.
    pub description: String,
}

/// The modal's whole first fetch: rows plus the warnings to show at the top.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct McpSnapshot {
    /// Every configured server.
    pub rows: Vec<McpServerRow>,
    /// Config errors and overridden-server notes.
    pub warnings: Vec<String>,
}

/// Fetch the modal's server list plus its warnings.
pub type McpList = Arc<dyn Fn() -> McpSnapshot + Send + Sync>;

/// Fetch one server's detail screen data; `None` when the server is unknown.
pub type McpDetail = Arc<dyn Fn(&str) -> Option<McpServerDetail> + Send + Sync>;

/// Begin an interactive sign-in and answer the authorization URL (or the
/// reason there is none) plus a status line.
///
/// Separate from `McpAction` because the sign-in screen needs the URL *and* a
/// place to paste a redirect, and the verb form's answer is one notice string.
pub type McpBeginSignIn = Arc<dyn Fn(&str) -> (Option<String>, String) + Send + Sync>;

/// Deliver a pasted redirect URL to a waiting sign-in flow. The answer is the
/// status line to show.
pub type McpSubmitRedirect = Arc<dyn Fn(&str, &str) -> String + Send + Sync>;

/// Abandon a waiting sign-in flow.
pub type McpCancelSignIn = Arc<dyn Fn(&str) + Send + Sync>;
