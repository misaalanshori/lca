//! The `/mcp` modal's write-model (gh #418 Phase G): the data rows the
//! interface turns into a list and a detail screen, and the word each state
//! sorts and labels by.
//!
//! Split from `manager.rs` for gate 11's line ceiling (gh #555). The division is
//! the one the modal already had, now named: `manager` owns **lifecycle** -
//! entries, the bridge, background sign-ins, the verbs - and this module owns the
//! **projection** of that state into rows. It reaches back through
//! [`McpManager`]'s existing accessors (`entries`, `bridge`, `drain_landed`) and
//! three `pub(crate)` fields (`user_path`, `project_path`, `bridge` - the same
//! `Arc`, so the projection shares the lifecycle's view rather than opening a
//! second one), so there is still one owner of each fact.
//!
//! UI-free by the same rule as the manager: values, never widgets (ADR-0047).

use std::path::PathBuf;

use crate::config::{EntryKind, ServerSource};
use crate::manager::McpManager;

/// The modal's server list (gh #418 Phase G): data rows plus warnings.
///
/// Deliberately not the display: [`crate::ServerStatus`] carries the same facts
/// for `/mcp`'s text block, and this carries the ones the modal's rows are made
/// of, with the state already reduced to the word it sorts by.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModalSnapshot {
    /// Every configured server.
    pub rows: Vec<ModalServerRow>,
    /// Config errors and override notes to show above the list.
    pub warnings: Vec<String>,
}

/// One server as the modal's list shows it (gh #418 §3.4 item 1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModalServerRow {
    /// pi's server name.
    pub name: String,
    /// The attention word: `needs-auth`, `failed`, `disconnected`,
    /// `connecting`, `connected`, or `disabled`.
    pub state: String,
    /// The state as a sentence (a failure carries its cause).
    pub detail: String,
    /// The effective exposure, which a project override can move.
    pub effective_exposure: String,
    /// Where the entry is defined.
    pub scope: &'static str,
    /// Whether a project override applies.
    pub project_override: bool,
    /// Whether the server is switched on.
    pub enabled: bool,
}

/// One server's detail (gh #418 §3.4 item 2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModalServerDetail {
    /// pi's server name.
    pub name: String,
    /// The attention word.
    pub state: String,
    /// The state sentence, with the stderr suffix already removed.
    pub detail: String,
    /// The command line or URL.
    pub target: String,
    /// The defining file.
    pub source_path: String,
    /// The project override's path, when the project file exists.
    pub override_path: Option<String>,
    /// The effective exposure.
    pub effective_exposure: String,
    /// The exposure written in the defining file.
    pub configured_exposure: String,
    /// Whether the server is switched on.
    pub enabled: bool,
    /// Whether a project override is in effect.
    pub project_override: bool,
    /// Whether `Enable in this project` applies.
    pub project_toggle_available: bool,
    /// Whether a sign-in is wanted.
    pub needs_sign_in: bool,
    /// Whether OAuth is configured at all.
    pub has_oauth: bool,
    /// The child's stderr tail, split out of the failure text.
    pub stderr_tail: Option<String>,
    /// Every tool the server offers.
    pub tools: Vec<ModalToolRow>,
    /// How many tools it offers (the same number as `tools.len()`, carried so a
    /// caller does not have to trust an empty vec to mean zero rather than "not
    /// fetched").
    pub tools_count: usize,
}

/// One tool in the modal's inspector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModalToolRow {
    /// The qualified name the model calls.
    pub name: String,
    /// The effective exposure for this tool.
    pub exposure: String,
    /// The description the model reads.
    pub description: String,
}

/// The word the modal sorts and labels by.
fn state_word(state: &crate::ServerStateKind) -> String {
    match state {
        crate::ServerStateKind::Connected => "connected",
        crate::ServerStateKind::Disabled => "disabled",
        crate::ServerStateKind::Connecting => "connecting",
        crate::ServerStateKind::Failed(_) => "failed",
        // The sort key the modal uses is `needs-auth`; the status block spells
        // it `needs sign-in` because that reads better in a sentence.
        crate::ServerStateKind::NeedsSignIn(_) => "needs-auth",
    }
    .to_string()
}

/// The ONE-LINE state a list row shows (gh #418 Phase G).
///
/// Deliberately shorter than [`state_detail`]: the list is for triage, so a
/// failure reads `failed` here and its cause is one `Enter` away. The stderr tail
/// makes this distinction load-bearing rather than cosmetic - a traceback inlined
/// into a row wraps three lines deep.
fn list_state_summary(state: &crate::ServerStateKind, tools: usize) -> String {
    match state {
        crate::ServerStateKind::Connected => format!("connected ({tools} tools)"),
        crate::ServerStateKind::Disabled => "disabled".to_string(),
        crate::ServerStateKind::Connecting => "connecting".to_string(),
        crate::ServerStateKind::Failed(_) => "failed".to_string(),
        crate::ServerStateKind::NeedsSignIn(_) => "needs sign-in".to_string(),
    }
}

/// The state as a sentence, with the failure's own message.
fn state_detail(state: &crate::ServerStateKind, tools: usize) -> String {
    match state {
        crate::ServerStateKind::Connected => format!("connected ({tools} tools)"),
        crate::ServerStateKind::Disabled => "disabled".to_string(),
        crate::ServerStateKind::Connecting => "connecting".to_string(),
        crate::ServerStateKind::Failed(err) => format!("failed: {err}"),
        crate::ServerStateKind::NeedsSignIn(detail) => format!("needs sign-in: {detail}"),
    }
}

/// Split the ` stderr: …` suffix `connect_one_stdio` appends back out (gh #418
/// Phase C#C1 wrote it there because that is where the pipe is still readable).
///
/// Keeping the split here rather than changing the producer means the text
/// block and the modal both get what they need from one failure string, and the
/// log keeps the joined form.
fn split_stderr(detail: &str) -> (String, Option<String>) {
    match detail.split_once(" stderr: ") {
        Some((head, tail)) if !tail.is_empty() => (head.to_string(), Some(tail.to_string())),
        _ => (detail.to_string(), None),
    }
}

/// The modal's spelling of one exposure level.
fn exposure_word(exposure: crate::config::McpExposure) -> String {
    match exposure {
        crate::config::McpExposure::Codemode => "codemode",
        crate::config::McpExposure::Deferred => "deferred",
        crate::config::McpExposure::Direct => "direct",
        crate::config::McpExposure::Hidden => "hidden",
    }
    .to_string()
}

/// The modal's spelling of one tool's exposure (the protocol vocabulary, which
/// has an extra case: absent means direct).
fn tool_exposure_word(exposure: lca_protocol::ToolExposure) -> String {
    match exposure {
        lca_protocol::ToolExposure::Direct => "direct",
        lca_protocol::ToolExposure::Codemode => "codemode",
        lca_protocol::ToolExposure::Deferred => "deferred",
        lca_protocol::ToolExposure::Hidden => "hidden",
        lca_protocol::ToolExposure::ModelOnly => "model-only",
    }
    .to_string()
}

impl McpManager {
    /// The modal's server list (gh #418 Phase G): one data row per server plus
    /// the load warnings.
    ///
    /// **Data, never a widget.** ADR-0047 pins that the manager answers with
    /// values or a notice string and the interface composes the display; the
    /// architecture test forbids the `lca-ui` edge that rendering here would
    /// need. So this returns the facts a row is made of and nothing more.
    pub fn modal_rows(&self, warnings: &[String]) -> ModalSnapshot {
        self.drain_landed();
        let rows = self
            .bridge()
            .statuses()
            .into_iter()
            .map(|status| ModalServerRow {
                state: state_word(&status.state),
                // The list row carries a SHORT state, never the failure text.
                // The full message and the stderr tail belong on the detail
                // screen: a list row that inlines a traceback wraps across three
                // lines and pushes every other server off the visible box, which
                // is the opposite of what an attention-sorted list is for. (Found
                // in the live pane, not in a unit row.)
                detail: list_state_summary(&status.state, status.tools),
                effective_exposure: exposure_word(status.exposure),
                scope: match status.source {
                    ServerSource::User => "user",
                    ServerSource::Project => "project",
                    ServerSource::Inline => "inline",
                },
                // A user-level entry that has a project file *and* is not the
                // defining source is being overridden for this project.
                project_override: status.source == ServerSource::Project
                    && self.entries().iter().any(|entry| {
                        entry.name == status.name && entry.source == ServerSource::Project
                    }),
                name: status.name,
                enabled: status.enabled,
            })
            .collect();
        ModalSnapshot {
            rows,
            warnings: warnings.to_vec(),
        }
    }

    /// One server's detail for the modal (gh #418 §3.4 item 2).
    ///
    /// Everything the detail screen shows comes from here: the header facts, the
    /// stderr tail when a stdio server failed, and every tool with its effective
    /// exposure. `None` for an unknown name, so the modal can say so rather than
    /// paint a blank screen.
    pub fn modal_detail(&self, server: &str) -> Option<ModalServerDetail> {
        self.drain_landed();
        let entry = self
            .entries()
            .into_iter()
            .find(|entry| entry.name == server)?;
        let status = self
            .bridge()
            .statuses()
            .into_iter()
            .find(|status| status.name == server);
        let (state_word, state_detail, tools_count) = match status.as_ref().map(|s| &s.state) {
            Some(state) => (
                state_word(state),
                state_detail(state, status.as_ref().map(|s| s.tools).unwrap_or(0)),
                status.as_ref().map(|s| s.tools).unwrap_or(0),
            ),
            // A configured entry the bridge has no row for is not connected:
            // saying "connecting" would claim work nobody started.
            None => ("disconnected".to_string(), "not connected".to_string(), 0),
        };
        let target = match &entry.kind {
            EntryKind::Stdio { command, args, .. } => {
                if args.is_empty() {
                    command.clone()
                } else {
                    format!("{command} {}", args.join(" "))
                }
            }
            EntryKind::Http { url, .. } => url.clone(),
        };
        let (needs_sign_in, has_oauth) = match status.as_ref().map(|s| &s.state) {
            Some(crate::ServerStateKind::NeedsSignIn(_)) => {
                (true, entry.http_config().is_some_and(|c| c.oauth.is_some()))
            }
            _ => (
                false,
                entry.http_config().is_some_and(|c| c.oauth.is_some()),
            ),
        };
        // The failure text already carries ` stderr: …` (gh #418 Phase C#C1),
        // because that suffix is appended where the child died and is the only
        // place the pipe is still readable. Split it back out here so the modal
        // can draw it as its own block rather than as one very long line.
        let (clean_detail, stderr_tail) = split_stderr(&state_detail);
        let configured_exposure = exposure_word(entry.exposure);
        let effective_exposure = status
            .as_ref()
            .map(|s| exposure_word(s.exposure))
            .unwrap_or_else(|| configured_exposure.clone());
        Some(ModalServerDetail {
            tools: self
                .bridge
                .server_tools(server)
                .into_iter()
                .map(|spec| ModalToolRow {
                    exposure: tool_exposure_word(spec.exposure),
                    description: spec.description,
                    name: spec.name,
                })
                .collect(),
            name: entry.name.clone(),
            state: state_word,
            detail: clean_detail,
            target,
            source_path: self
                .source_path(entry.source)
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "(inline)".to_string()),
            override_path: self
                .project_path
                .as_ref()
                .map(|path| path.display().to_string()),
            effective_exposure,
            configured_exposure,
            enabled: entry.enabled,
            project_override: entry.source == ServerSource::Project,
            // A project-scoped toggle only applies to a server this file did not
            // define: a project-level entry is already the project's own.
            project_toggle_available: entry.source == ServerSource::User
                && self.project_path.is_some(),
            needs_sign_in,
            has_oauth,
            stderr_tail,
            tools_count,
        })
    }

    /// The file one source resolves to.
    fn source_path(&self, source: ServerSource) -> Option<&PathBuf> {
        match source {
            ServerSource::Project => self.project_path.as_ref(),
            _ => Some(&self.user_path),
        }
    }
}
