//! The `/mcp` manager's key handling (gh #418 Phase G), split from
//! `chat_mcp.rs` for the 1,200-line ceiling.
//!
//! Five screens, one owner: list → detail → (tools | exposure | sign-in). Every
//! screen answers with an action through the host hooks, and every answer
//! refreshes the rows so what the modal shows is what the manager believes.
//!
//! Two rules worth stating because they are easy to get wrong:
//!
//! - **`Esc` cancels, it does not unwind one level at a time.** On the sign-in
//!   screen it abandons the flow (the loopback listener must stop, or a later
//!   sign-in inherits it); everywhere else it closes the modal. pi's modal does
//!   the same, and a user pressing `Esc` twice expecting "out" and getting "out
//!   one screen" is worse than the lost navigation.
//! - **A background refresh never moves the cursor to another server.** The list
//!   is sorted by attention, so a reconnect reorders it; the selection is kept
//!   **by name**, not by index.

use super::chat::Chat;
use super::chat_commands::{paste_text, printable};
use super::chat_mcp::{EXPOSURE_LEVELS, McpActionRow, McpManager, McpScreen};
use super::state::Action;

impl Chat {
    /// Handle a key while the `/mcp` modal owns the keyboard. `None` means
    /// "not mine", so the caller falls through.
    pub(crate) fn handle_mcp_manager_key(
        &mut self,
        _data: &str,
        key: Option<&str>,
    ) -> Option<Action> {
        let mut manager = self.mcp_manager.take()?;
        match manager.screen.clone() {
            McpScreen::List => self.mcp_list_key(&mut manager, key),
            McpScreen::Detail { server } => self.mcp_detail_key(&mut manager, key, &server),
            McpScreen::Tools { server } => self.mcp_tools_key(&mut manager, key, &server),
            McpScreen::Exposure { server, .. } => self.mcp_exposure_key(&mut manager, key, &server),
            McpScreen::SignIn { server, .. } => {
                self.mcp_signin_key(&mut manager, _data, key, &server)
            }
        }
        None
    }

    /// The server list: up/down move, enter opens the detail, esc closes.
    fn mcp_list_key(&mut self, manager: &mut McpManager, key: Option<&str>) {
        match key {
            Some("escape") => return, // dropped: `manager` stays taken
            Some("up") => {
                manager.selected = manager.selected.saturating_sub(1);
            }
            Some("down") => {
                if !manager.rows.is_empty() {
                    manager.selected = (manager.selected + 1).min(manager.rows.len() - 1);
                }
            }
            Some("enter") => {
                let server = manager.selected_server().map(str::to_string);
                self.mcp_manager = Some(manager.clone());
                if let Some(server) = server {
                    self.open_mcp_detail(&server);
                }
                return;
            }
            _ => {}
        }
        self.mcp_manager = Some(manager.clone());
    }

    /// One server's detail: up/down over the actions, enter runs one.
    fn mcp_detail_key(&mut self, manager: &mut McpManager, key: Option<&str>, server: &str) {
        match key {
            Some("escape") => {
                // Back to the list, keeping the cursor on this server.
                manager.screen = McpScreen::List;
                manager.detail = None;
                self.mcp_manager = Some(manager.clone());
                return;
            }
            Some("up") => {
                manager.action_selected = manager.action_selected.saturating_sub(1);
            }
            Some("down") => {
                if !manager.actions.is_empty() {
                    manager.action_selected =
                        (manager.action_selected + 1).min(manager.actions.len() - 1);
                }
            }
            Some("enter") => {
                let action = manager.actions.get(manager.action_selected).cloned();
                self.mcp_manager = Some(manager.clone());
                if let Some(action) = action {
                    self.run_mcp_action(server, &action);
                }
                return;
            }
            _ => {}
        }
        self.mcp_manager = Some(manager.clone());
    }

    /// The tool inspector: up/down scroll the list, esc returns to the detail.
    fn mcp_tools_key(&mut self, manager: &mut McpManager, key: Option<&str>, server: &str) {
        match key {
            Some("escape") | Some("enter") | Some("left") => {
                manager.screen = McpScreen::Detail {
                    server: server.to_string(),
                };
            }
            Some("up") => {
                manager.tool_selected = manager.tool_selected.saturating_sub(1);
            }
            Some("down") => {
                let count = manager
                    .detail
                    .as_ref()
                    .map(|detail| detail.tools.len())
                    .unwrap_or(0);
                if count > 0 {
                    manager.tool_selected = (manager.tool_selected + 1).min(count - 1);
                }
            }
            _ => {}
        }
        self.mcp_manager = Some(manager.clone());
    }

    /// The exposure picker: up/down move over the four levels, enter persists.
    fn mcp_exposure_key(&mut self, manager: &mut McpManager, key: Option<&str>, server: &str) {
        match key {
            Some("escape") => {
                manager.screen = McpScreen::Detail {
                    server: server.to_string(),
                };
                self.mcp_manager = Some(manager.clone());
                return;
            }
            Some("up") => {
                manager.screen = McpScreen::Exposure {
                    server: server.to_string(),
                    selected: match &manager.screen {
                        McpScreen::Exposure { selected, .. } => selected.saturating_sub(1),
                        _ => 0,
                    },
                };
            }
            Some("down") => {
                let selected = match &manager.screen {
                    McpScreen::Exposure { selected, .. } => {
                        (selected + 1).min(EXPOSURE_LEVELS.len() - 1)
                    }
                    _ => 0,
                };
                manager.screen = McpScreen::Exposure {
                    server: server.to_string(),
                    selected,
                };
            }
            Some("enter") => {
                let selected = match &manager.screen {
                    McpScreen::Exposure { selected, .. } => *selected,
                    _ => 0,
                };
                let level = EXPOSURE_LEVELS[selected.min(EXPOSURE_LEVELS.len() - 1)];
                // The verb form is the one place the write happens: the modal
                // does not grow a second persistence path (gh #267's atomic
                // writer is called from exactly one place).
                self.call_mcp_verb("exposure", &format!("{server} {level}"));
                // Reopen the detail with fresh data so the header shows the new
                // effective value rather than the one it was opened with.
                self.open_mcp_detail(server);
                return;
            }
            _ => {}
        }
        self.mcp_manager = Some(manager.clone());
    }

    /// The sign-in screen: paste a redirect URL, enter submits, esc cancels.
    fn mcp_signin_key(
        &mut self,
        manager: &mut McpManager,
        data: &str,
        key: Option<&str>,
        server: &str,
    ) {
        let McpScreen::SignIn {
            url,
            mut pasted,
            mut status,
            ..
        } = manager.screen.clone()
        else {
            self.mcp_manager = Some(manager.clone());
            return;
        };
        match key {
            Some("escape") => {
                // Cancel the flow before leaving: a listener left running would
                // make the next sign-in answer into this one's channel.
                if let Some(cancel) = self.world.options.hooks.mcp_cancel_sign_in.clone() {
                    cancel(server);
                }
                self.say(format!("`{server}` sign-in cancelled"));
                // Return before the write-back below, which would put the
                // sign-in screen straight back (caught by this row).
                manager.screen = McpScreen::Detail {
                    server: server.to_string(),
                };
                self.mcp_manager = Some(manager.clone());
                return;
            }
            Some("enter") => {
                if pasted.trim().is_empty() {
                    status = "paste the whole redirect URL first".to_string();
                } else if let Some(submit) = self.world.options.hooks.mcp_submit_redirect.clone() {
                    // The pasted URL carries a one-time code: never echoed into
                    // the transcript, never logged (secrets hygiene).
                    status = submit(server, pasted.trim());
                    pasted.clear();
                    self.say(format!("`{server}`: {status}"));
                }
            }
            Some("backspace") => {
                pasted.pop();
            }
            None => {
                if let Some(text) = printable(data).or_else(|| paste_text(data)) {
                    pasted.push_str(&text);
                }
            }
            _ => {}
        }
        manager.screen = McpScreen::SignIn {
            server: server.to_string(),
            url,
            pasted,
            status,
        };
        self.mcp_manager = Some(manager.clone());
    }

    /// Run one detail action.
    fn run_mcp_action(&mut self, server: &str, action: &McpActionRow) {
        match action {
            McpActionRow::SignIn => self.open_mcp_signin(server),
            McpActionRow::Tools => {
                if let Some(manager) = self.mcp_manager.as_mut() {
                    manager.tool_selected = 0;
                    manager.screen = McpScreen::Tools {
                        server: server.to_string(),
                    };
                }
            }
            McpActionRow::Reconnect => {
                self.call_mcp_verb("reconnect", server);
                self.open_mcp_detail(server);
            }
            McpActionRow::SignOut => {
                self.call_mcp_verb("logout", server);
                self.open_mcp_detail(server);
            }
            McpActionRow::Exposure => {
                let selected = self
                    .mcp_manager
                    .as_ref()
                    .and_then(|manager| manager.detail.as_ref())
                    .and_then(|detail| {
                        EXPOSURE_LEVELS
                            .iter()
                            .position(|level| *level == detail.effective_exposure)
                    })
                    .unwrap_or(0);
                if let Some(manager) = self.mcp_manager.as_mut() {
                    manager.screen = McpScreen::Exposure {
                        server: server.to_string(),
                        selected,
                    };
                }
            }
            McpActionRow::Enable => {
                let enabled = self
                    .mcp_manager
                    .as_ref()
                    .and_then(|manager| manager.detail.as_ref())
                    .map(|detail| detail.enabled)
                    .unwrap_or(true);
                let verb = if enabled { "disable" } else { "enable" };
                self.call_mcp_verb(verb, server);
                self.open_mcp_detail(server);
            }
            McpActionRow::ProjectToggle => {
                // The project-scoped write goes through the same verb the modal
                // would otherwise duplicate: `enable`/`disable` already write a
                // knob-only project override for a user-level server under a
                // trusted project (the gh #418 Phase B rule), which is exactly
                // what this row promises.
                let enabled = self
                    .mcp_manager
                    .as_ref()
                    .and_then(|manager| manager.detail.as_ref())
                    .map(|detail| detail.enabled)
                    .unwrap_or(true);
                let verb = if enabled { "disable" } else { "enable" };
                self.call_mcp_verb(verb, server);
                self.open_mcp_detail(server);
            }
            McpActionRow::Back => {
                if let Some(manager) = self.mcp_manager.as_mut() {
                    manager.screen = McpScreen::List;
                    manager.detail = None;
                }
            }
        }
    }

    /// Open the OAuth screen and start the flow.
    fn open_mcp_signin(&mut self, server: &str) {
        let (url, status) = match self.world.options.hooks.mcp_begin_sign_in.clone() {
            Some(begin) => begin(server),
            None => (None, "this host cannot start a sign-in".to_string()),
        };
        if let Some(manager) = self.mcp_manager.as_mut() {
            manager.screen = McpScreen::SignIn {
                server: server.to_string(),
                url,
                pasted: String::new(),
                status,
            };
        }
    }

    /// Call one `/mcp` verb and post its notice, then refresh the rows.
    fn call_mcp_verb(&mut self, verb: &str, target: &str) {
        let Some(act) = self.world.options.hooks.mcp_action.clone() else {
            return;
        };
        self.say(act(verb, target));
        self.refresh_mcp_rows();
    }
}
