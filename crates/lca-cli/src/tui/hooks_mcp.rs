//! The MCP manager's row conversions, split from `hooks.rs` at the workspace's
//! 1,200-line file ceiling (gh #373 gap 6's cycle).
//!
//! They live in the host because the host is the only crate that may name both
//! sides: `lca-mcp` answers with data (ADR-0047 forbids it rendering) and
//! `lca-ui` cannot name the subsystem's types (the architecture test forbids
//! that edge).
//!
//! `pub(super)` so `hooks.rs` imports them exactly as before; nothing else
//! changes.

/// One manager row as the modal's own type (gh #418 Phase G).
///
/// The conversion lives in the host, not in `lca-mcp` and not in `lca-ui`: the
/// subsystem answers with data (ADR-0047 forbids it rendering), and the interface
/// cannot name the subsystem's types (the architecture test forbids that edge).
/// The host is the only crate that may know both.
pub(super) fn to_ui_row(row: lca_mcp::ModalServerRow) -> lca_ui::McpServerRow {
    lca_ui::McpServerRow {
        scope: row.scope.to_string(),
        name: row.name,
        state: row.state,
        detail: row.detail,
        effective_exposure: row.effective_exposure,
        project_override: row.project_override,
    }
}

/// One server's detail as the modal's own type.
pub(super) fn to_ui_detail(detail: lca_mcp::ModalServerDetail) -> lca_ui::McpServerDetail {
    lca_ui::McpServerDetail {
        tools: detail
            .tools
            .into_iter()
            .map(|tool| lca_ui::McpToolRow {
                name: tool.name,
                exposure: tool.exposure,
                description: tool.description,
            })
            .collect(),
        name: detail.name,
        state: detail.state,
        detail: detail.detail,
        target: detail.target,
        source_path: detail.source_path,
        override_path: detail.override_path,
        effective_exposure: detail.effective_exposure,
        configured_exposure: detail.configured_exposure,
        enabled: detail.enabled,
        project_override: detail.project_override,
        project_toggle_available: detail.project_toggle_available,
        needs_sign_in: detail.needs_sign_in,
        has_oauth: detail.has_oauth,
        stderr_tail: detail.stderr_tail,
    }
}
