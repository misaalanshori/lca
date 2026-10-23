//! Painting the `/mcp` manager modal (gh #418 Phase G), split from
//! `chat_mcp.rs` for the 1,200-line ceiling.
//!
//! Everything a user reads is composed **here**, from the data rows the manager
//! handed over. `lca-mcp` composes no display strings beyond a notice: ADR-0047
//! pins that boundary, and the architecture test forbids the `lca-ui` edge that
//! would let a subsystem render.
//!
//! The one design rule the layout follows: **the failure text is body text, not
//! a tooltip.** "It failed" without the server's own stderr is what makes MCP
//! debugging miserable (§3.4's own words), so the tail is drawn in the detail
//! screen, bounded to [`STDERR_VIEW_ROWS`] rows, with a marker saying it was cut.
//! Everything else in the header is one line each, because the actions below need
//! the room.

use super::chat_mcp::{McpManager, McpScreen, STDERR_VIEW_ROWS, exposure_options, stderr_rows};

/// The overlay's `(title, body, hint)` for the current screen.
///
/// `auth_hint` is the sign-in screen's copy affordance (gh #398 gap 1): the
/// caller builds it because it needs the live key binding and the copy
/// confirmation, both of which live on `Chat`, not on the modal's own state.
pub fn mcp_overlay(manager: &McpManager, auth_hint: Option<&str>) -> (String, Vec<String>, String) {
    match &manager.screen {
        McpScreen::List => list_screen(manager),
        McpScreen::Detail { server } => detail_screen(manager, server),
        McpScreen::Tools { server } => tools_screen(manager, server),
        McpScreen::Exposure { server, selected } => exposure_screen(manager, server, *selected),
        McpScreen::SignIn {
            server,
            url,
            pasted,
            status,
        } => signin_screen(server, url.as_deref(), pasted, status, auth_hint),
    }
}

/// The server list (§3.4 item 1): one row per server, then the warnings.
fn list_screen(manager: &McpManager) -> (String, Vec<String>, String) {
    let mut body = vec![String::new()];
    // Warnings first: a config error explains why a server is missing from the
    // list, so it has to be readable before the list is read.
    if !manager.warnings.is_empty() {
        body.push("  WARNINGS".to_string());
        for warning in &manager.warnings {
            for line in wrap(warning, 96) {
                body.push(format!("   {line}"));
            }
        }
        body.push(String::new());
    }
    body.push("  SERVERS".to_string());
    if manager.rows.is_empty() {
        body.push("   (none configured)".to_string());
    }
    for (index, row) in manager.rows.iter().enumerate() {
        let cursor = if index == manager.selected { '>' } else { ' ' };
        // `<name>  <state> · <exposure> · <scope>` is the row pi draws, and the
        // three facts a user triages by.
        let scope = if row.project_override {
            format!("{}, project override", row.scope)
        } else {
            row.scope.clone()
        };
        body.push(format!(
            " {cursor} {:<22} {} · {} · {scope}",
            row.name, row.detail, row.effective_exposure
        ));
    }
    if !manager.rows.is_empty() {
        body.push(format!(
            "  ({}/{})",
            manager.selected + 1,
            manager.rows.len()
        ));
    }
    (
        "MCP servers".to_string(),
        body,
        "enter open · esc close".to_string(),
    )
}

/// One server's detail (§3.4 item 2): header facts, the failure, then actions.
fn detail_screen(manager: &McpManager, server: &str) -> (String, Vec<String>, String) {
    let mut body = vec![String::new()];
    if let Some(detail) = &manager.detail {
        body.push(format!("  {server}"));
        body.push(format!("    state:    {}", detail.detail));
        body.push(format!("    target:   {}", detail.target));
        body.push(format!("    defined:  {}", detail.source_path));
        if let Some(path) = &detail.override_path {
            body.push(format!("    override: {path}"));
        }
        // Show both when they differ: a user who set `direct` in mcp.json and
        // sees `codemode` here needs to know a project override moved it.
        if detail.configured_exposure == detail.effective_exposure {
            body.push(format!("    exposure: {}", detail.effective_exposure));
        } else {
            body.push(format!(
                "    exposure: {} (configured: {})",
                detail.effective_exposure, detail.configured_exposure
            ));
        }
        body.push(format!(
            "    enabled:  {}",
            if detail.enabled { "yes" } else { "no" }
        ));

        // The one thing this screen exists for.
        if let Some(tail) = &detail.stderr_tail {
            let rows = stderr_rows(tail);
            if !rows.is_empty() {
                body.push(String::new());
                body.push(format!(
                    "  STDERR (last {} lines)",
                    STDERR_VIEW_ROWS.min(rows.len())
                ));
                for row in rows {
                    body.push(format!("   {row}"));
                }
            }
        }
    } else {
        body.push(format!("  {server}"));
        body.push("    (no detail available)".to_string());
    }

    body.push(String::new());
    body.push("  ACTIONS".to_string());
    for (index, action) in manager.actions.iter().enumerate() {
        let cursor = if index == manager.action_selected {
            '>'
        } else {
            ' '
        };
        let label = match &manager.detail {
            Some(detail) => action.label(detail),
            None => format!("{action:?}"),
        };
        body.push(format!(" {cursor} {label}"));
    }
    (
        format!("MCP server {}", display_name(server)),
        body,
        "enter run · esc back".to_string(),
    )
}

/// The tool inspector (§3.4 item 2, `Tools`).
fn tools_screen(manager: &McpManager, server: &str) -> (String, Vec<String>, String) {
    let tools = manager
        .detail
        .as_ref()
        .map(|detail| detail.tools.clone())
        .unwrap_or_default();
    let mut body = vec![String::new()];
    if tools.is_empty() {
        body.push("  (this server offers no tools)".to_string());
    }
    for (index, tool) in tools.iter().enumerate() {
        let cursor = if index == manager.tool_selected {
            '>'
        } else {
            ' '
        };
        // Name and effective exposure on one line; the description wrapped
        // beneath, because the description is what tells a user whether the
        // exposure is right for this tool.
        body.push(format!(" {cursor} {} [{}]", tool.name, tool.exposure));
        if !tool.description.is_empty() {
            for line in wrap(&tool.description, 92) {
                body.push(format!("     {line}"));
            }
        }
    }
    if !tools.is_empty() {
        body.push(format!("  ({}/{})", manager.tool_selected + 1, tools.len()));
    }
    (
        format!("MCP tools · {}", display_name(server)),
        body,
        "esc back".to_string(),
    )
}

/// The exposure picker (§3.4 item 2, `Exposure`).
fn exposure_screen(
    manager: &McpManager,
    server: &str,
    selected: usize,
) -> (String, Vec<String>, String) {
    let options = manager
        .detail
        .as_ref()
        .map(exposure_options)
        .unwrap_or_default();
    let mut body = vec![String::new()];
    for (index, (level, effective)) in options.iter().enumerate() {
        let cursor = if index == selected { '>' } else { ' ' };
        // The active marker is the **effective** value: the picker's whole job
        // is to answer "what does the model get", and a project override can
        // make that differ from what this list last wrote.
        let mark = if *effective { "  ●" } else { "  ○" };
        body.push(format!(" {cursor} {level}{mark}"));
    }
    body.push(String::new());
    body.push(format!(
        "  {}",
        match options.iter().find(|(_, active)| *active) {
            Some((level, _)) => format!("currently effective: {level}"),
            None => "currently effective: (unknown)".to_string(),
        }
    ));
    (
        format!("MCP exposure · {}", display_name(server)),
        body,
        "enter apply · esc back".to_string(),
    )
}

/// The OAuth sign-in screen (§3.4 item 3).
///
/// The URL is drawn in full, unwrapped, because it is meant to be copied or read
/// aloud on another machine. The paste field is the SSH case: on a remote box
/// the loopback callback can never fire, so the flow has to accept the redirect
/// URL the user's local browser produced.
fn signin_screen(
    server: &str,
    url: Option<&str>,
    pasted: &str,
    status: &str,
    hint: Option<&str>,
) -> (String, Vec<String>, String) {
    let mut body = vec![String::new()];
    match url {
        Some(url) => {
            body.push("  Open this URL in a browser on any machine:".to_string());
            body.push(String::new());
            // Unwrapped on purpose: a wrapped authorization URL is unusable.
            // Gh #398 gap 1: the hint sits **above** the URL, not below it.
            // The body is windowed (`▼ N more`) and a 5-row wrapped URL eats the
            // visible rows, so a hint underneath is the first thing scrolled out
            // - which is the state where a reader most needs it. Above the URL
            // it is always on screen (found in a live 30-row pane).
            if let Some(hint) = hint {
                body.push(format!("  {hint}"));
                body.push(String::new());
            }
            body.push(format!("  {url}"));
            body.push(String::new());
            body.push(
                "  If the browser is elsewhere, paste the redirect URL it lands on:".to_string(),
            );
        }
        None => {
            body.push(format!("  {status}"));
            body.push(String::new());
            body.push("  Paste the redirect URL, or esc to cancel:".to_string());
        }
    }
    body.push(String::new());
    body.push(format!("  > {pasted}"));
    if url.is_some() && !status.is_empty() {
        body.push(String::new());
        body.push(format!("  {status}"));
    }
    (
        format!("Sign in to {}", display_name(server)),
        body,
        "type/paste redirect · enter submit · esc cancel".to_string(),
    )
}

/// Wrap `text` to at most `width` characters on word boundaries.
///
/// A hand-rolled wrapper rather than the transcript's: these lines are overlay
/// body text with a fixed small indent, and reusing the markdown engine here
/// would paint styling the picker box does not strip.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if current.is_empty() {
            current.push_str(word);
        } else if current.len() + 1 + word.len() <= width {
            current.push(' ');
            current.push_str(word);
        } else {
            lines.push(std::mem::take(&mut current));
            current.push_str(word);
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// A server name as the title shows it.
fn display_name(server: &str) -> String {
    if server.is_empty() {
        "(unnamed)".to_string()
    } else {
        server.to_string()
    }
}
