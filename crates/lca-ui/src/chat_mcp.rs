//! The `/mcp` manager modal (gh #418 Phase G, §3.4).
//!
//! Three screens under one owner: the server list, one server's detail with its
//! actions, and (from the detail) the tool inspector, the exposure picker, and
//! the OAuth sign-in screen.
//!
//! **The manager answers with data; this file owns the widget.** `lca-mcp`
//! returns [`McpServerDetail`] rows and notice strings, never a rendered line,
//! because ADR-0047 pins that boundary and `lca-core`'s architecture test forbids
//! an `lca-ui` edge into `lca-mcp` (a subsystem that renders is a subsystem that
//! depends upward). So every string in this file that a user reads is composed
//! here, from data the manager handed over.
//!
//! The direct verbs (`/mcp login <server>`, `reconnect`, `enable`,
//! `exposure <server> <level>`) stay exactly as they were: they route through
//! [`crate::state::McpAction`] and answer with a notice. The modal is an
//! addition, not a replacement, and
//! `chat_mcp_tests::mcp_verbs_route_through_the_hooks` still pins them.

use super::chat::Chat;
use crate::state::{Action, McpServerDetail, McpServerRow};

/// How much of a failure's stderr the modal shows.
///
/// The manager keeps ≤2,000 bytes and joins it to one line; the modal shows at
/// most this many rows of it. A chatty server (a Python traceback, a repeated
/// retry banner) would otherwise push the actions off the screen, and the
/// actions are what the user opened the modal to reach. pi's rule is the same
/// shape: show the tail, because that is where a crash explains itself.
pub const STDERR_VIEW_ROWS: usize = 12;

/// Which screen the modal is showing.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum McpScreen {
    /// The server list (§3.4 item 1).
    #[default]
    List,
    /// One server's detail and actions (§3.4 item 2).
    Detail {
        /// The server this screen belongs to.
        server: String,
    },
    /// The tool inspector for one server (§3.4 item 2, `Tools`).
    Tools {
        /// The server whose tools are listed.
        server: String,
    },
    /// The exposure picker for one server.
    Exposure {
        /// The server whose exposure is being picked.
        server: String,
        /// The picker's highlight, one of [`EXPOSURE_LEVELS`].
        selected: usize,
    },
    /// The OAuth sign-in screen (§3.4 item 3).
    SignIn {
        /// The server being signed in to.
        server: String,
        /// The authorization URL, once the manager has it.
        url: Option<String>,
        /// The redirect URL the user is typing (the SSH case).
        pasted: String,
        /// What the manager last said about the flow.
        status: String,
    },
}

/// The four exposure levels, in the order the picker shows them.
///
/// Ordered widest-reach first because the picker's job is to answer "how much of
/// the model's context does this cost", and `direct` is the expensive end.
pub const EXPOSURE_LEVELS: [&str; 4] = ["direct", "codemode", "deferred", "hidden"];

/// One action on the detail screen.
///
/// The list is computed per server, so a button that makes no sense for this
/// state is **absent** rather than shown-and-refused: a greyed "Sign in" on a
/// stdio server teaches the user nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpActionRow {
    /// Start the OAuth flow (only when the server wants a sign-in).
    SignIn,
    /// Open the tool inspector (only when connected).
    Tools,
    /// Reconnect in the background.
    Reconnect,
    /// Forget stored tokens (only when connected over OAuth).
    SignOut,
    /// Open the exposure picker.
    Exposure,
    /// Toggle this server on or off.
    Enable,
    /// Toggle this server for the current project only.
    ProjectToggle,
    /// Go back to the list.
    Back,
}

impl McpActionRow {
    /// The label the modal draws.
    #[must_use]
    pub fn label(&self, detail: &McpServerDetail) -> String {
        match self {
            McpActionRow::SignIn => "Sign in".to_string(),
            McpActionRow::Tools => format!("Tools ({})", detail.tools.len()),
            McpActionRow::Reconnect => "Reconnect".to_string(),
            McpActionRow::SignOut => "Sign out".to_string(),
            McpActionRow::Exposure => format!("Exposure: {}", detail.effective_exposure),
            McpActionRow::Enable => {
                if detail.enabled {
                    "Disable".to_string()
                } else {
                    "Enable".to_string()
                }
            }
            McpActionRow::ProjectToggle => {
                if detail.project_override {
                    "Remove this project's override".to_string()
                } else {
                    "Enable in this project".to_string()
                }
            }
            McpActionRow::Back => "Back".to_string(),
        }
    }
}

/// The open modal.
#[derive(Debug, Clone, Default)]
pub struct McpManager {
    /// The screen being shown.
    pub screen: McpScreen,
    /// Fresh rows, refreshed after every action.
    pub rows: Vec<McpServerRow>,
    /// The list's highlight.
    pub selected: usize,
    /// The detail screen's fetched data, when it is the active screen.
    pub detail: Option<McpServerDetail>,
    /// The detail screen's action rows and highlight.
    pub actions: Vec<McpActionRow>,
    /// The action highlight.
    pub action_selected: usize,
    /// The tool inspector's highlight.
    pub tool_selected: usize,
    /// Warnings or config errors the manager reported with the rows.
    pub warnings: Vec<String>,
}

impl McpManager {
    /// Open on the list, sorted by attention.
    #[must_use]
    pub fn ready(mut rows: Vec<McpServerRow>, warnings: Vec<String>) -> McpManager {
        sort_by_attention(&mut rows);
        McpManager {
            screen: McpScreen::List,
            rows,
            selected: 0,
            detail: None,
            actions: Vec::new(),
            action_selected: 0,
            tool_selected: 0,
            warnings,
        }
    }

    /// The highlighted server's name, if the list is showing one.
    #[must_use]
    pub fn selected_server(&self) -> Option<&str> {
        self.rows.get(self.selected).map(|row| row.name.as_str())
    }
}

/// Attention order (§3.4 item 1): a server needing a decision leads, one that
/// is fine trails.
///
/// The point is that opening `/mcp` puts the thing that needs the user at the
/// top. Alphabetical within a bucket, so the order is stable between opens — a
/// list that reshuffles under the cursor is worse than an unsorted one.
fn sort_by_attention(rows: &mut [McpServerRow]) {
    rows.sort_by(|left, right| {
        let rank = |row: &McpServerRow| match row.state.as_str() {
            "needs-auth" | "needs sign-in" => 0,
            "failed" => 1,
            "disconnected" => 2,
            "connecting" => 3,
            "connected" => 4,
            "disabled" => 5,
            _ => 6,
        };
        rank(left)
            .cmp(&rank(right))
            .then_with(|| left.name.cmp(&right.name))
    });
}

impl Chat {
    /// `/mcp` with no argument, or `/mcp list`: open the modal.
    ///
    /// A host with no `mcp_list` hook (headless, or a build without MCP) falls
    /// back to the verb route, which answers with the guide notice — so the old
    /// behaviour survives wherever the modal cannot exist.
    pub(crate) fn open_mcp_manager(&mut self) -> Action {
        let Some(list) = self.world.options.hooks.mcp_list.clone() else {
            return self.mcp_verb_fallback("");
        };
        let snapshot = list();
        if snapshot.rows.is_empty() && snapshot.warnings.is_empty() {
            // Nothing configured: the verb route's wording is better than an
            // empty modal, and it names where to add one.
            return self.mcp_verb_fallback("");
        }
        self.mcp_manager = Some(McpManager::ready(snapshot.rows, snapshot.warnings));
        // A pending background sign-in should still land in the modal.
        Action::Continue
    }

    /// Route a bare `/mcp` through the verb hook (the pre-modal path).
    pub(crate) fn mcp_verb_fallback(&mut self, argument: &str) -> Action {
        let Some(act) = self.world.options.hooks.mcp_action.clone() else {
            self.say("no MCP servers configured (see the authoring guide)".to_string());
            return Action::Continue;
        };
        let mut words = argument.splitn(2, ' ');
        let verb = words.next().unwrap_or("").to_string();
        let target = words.next().unwrap_or("").trim().to_string();
        self.say(act(&verb, &target));
        Action::Continue
    }

    /// Whether the modal owns the keyboard.
    #[must_use]
    pub fn mcp_manager_open(&self) -> bool {
        self.mcp_manager.is_some()
    }

    /// Re-fetch the rows after an action, keeping the selection on the same
    /// server when it still exists (§3.4: a background reconnect must not move
    /// the cursor to a different server).
    pub(crate) fn refresh_mcp_rows(&mut self) {
        let Some(list) = self.world.options.hooks.mcp_list.clone() else {
            return;
        };
        let keep = self
            .mcp_manager
            .as_ref()
            .and_then(|manager| manager.selected_server().map(str::to_string));
        let snapshot = list();
        let Some(mut manager) = self.mcp_manager.take() else {
            return;
        };
        let mut rows = snapshot.rows;
        sort_by_attention(&mut rows);
        manager.rows = rows;
        manager.warnings = snapshot.warnings;
        // Keep the cursor on the server it was on, by name: index-preserving
        // would move it when the attention order changed, which is exactly what
        // a reconnect does.
        if let Some(name) = keep
            && let Some(index) = manager.rows.iter().position(|row| row.name == name)
        {
            manager.selected = index;
        }
        manager.selected = manager.selected.min(manager.rows.len().saturating_sub(1));
        self.mcp_manager = Some(manager);
    }

    /// Open one server's detail screen: its state, its files, its error, and the
    /// actions that make sense for that state.
    pub(crate) fn open_mcp_detail(&mut self, server: &str) {
        let Some(detail) = self.fetch_mcp_detail(server) else {
            self.say(format!("no MCP server {server:?}"));
            return;
        };
        let actions = detail_actions(&detail);
        if let Some(manager) = self.mcp_manager.as_mut() {
            let same = matches!(&manager.screen, McpScreen::Detail { server: current } if current == server);
            manager.actions = actions;
            // A revisit keeps the action highlight; a fresh open starts at the
            // top, because the actions differ per state and a remembered index
            // would land on a different verb.
            if !same {
                manager.action_selected = 0;
            }
            manager.action_selected = manager
                .action_selected
                .min(manager.actions.len().saturating_sub(1));
            manager.tool_selected = 0;
            manager.detail = Some(detail);
            manager.screen = McpScreen::Detail {
                server: server.to_string(),
            };
        }
    }

    /// Ask the host for one server's detail (tools, error, file paths).
    pub(crate) fn fetch_mcp_detail(&self, server: &str) -> Option<McpServerDetail> {
        let detail = self.world.options.hooks.mcp_detail.clone()?;
        detail(server)
    }
}

/// Which actions the detail screen offers for this server's state.
///
/// Absent rather than disabled: see [`McpActionRow`].
#[must_use]
pub fn detail_actions(detail: &McpServerDetail) -> Vec<McpActionRow> {
    let mut actions = Vec::new();
    if detail.needs_sign_in {
        actions.push(McpActionRow::SignIn);
    }
    if detail.state == "connected" {
        actions.push(McpActionRow::Tools);
    }
    actions.push(McpActionRow::Reconnect);
    if detail.state == "connected" && detail.has_oauth {
        actions.push(McpActionRow::SignOut);
    }
    actions.push(McpActionRow::Exposure);
    actions.push(McpActionRow::Enable);
    // The project-scoped toggle only exists for a user-level server under a
    // trusted project: there is nothing to override otherwise, and offering it
    // would promise a write that cannot happen.
    if detail.project_toggle_available {
        actions.push(McpActionRow::ProjectToggle);
    }
    actions.push(McpActionRow::Back);
    actions
}

/// The stderr tail, bounded to `STDERR_VIEW_ROWS` rows and unwrapped.
///
/// The manager joins a traceback to one line (` | `-separated) so a status row
/// stays one row. Here it is split back out, because a traceback the user cannot
/// read is the exact failure this view exists to fix — but bounded, because a
/// server in a retry loop would otherwise scroll the actions away.
#[must_use]
pub fn stderr_rows(tail: &str) -> Vec<String> {
    if tail.is_empty() {
        return Vec::new();
    }
    let mut rows: Vec<String> = tail
        .split(['\n', '\r'])
        .flat_map(|line| line.split(" | "))
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect();
    if rows.len() > STDERR_VIEW_ROWS {
        let dropped = rows.len() - STDERR_VIEW_ROWS;
        rows = rows.split_off(dropped);
        // Say that it was cut, so nobody reads the tail as the whole story.
        rows.insert(0, format!("… {dropped} earlier lines omitted"));
    }
    rows
}

/// The exposure picker's level list as `(label, is_effective)`.
#[must_use]
pub fn exposure_options(detail: &McpServerDetail) -> Vec<(String, bool)> {
    EXPOSURE_LEVELS
        .iter()
        .map(|level| ((*level).to_string(), detail.effective_exposure == *level))
        .collect()
}
