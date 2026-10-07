//! The yes/no confirms: the login ad hoc-grant and the provider
//! switch (gh #177). One key each, answered through the host's seams.
//! Split from `chat.rs` for the 1,200-line ceiling (gate 11).

use super::chat::Chat;
use crate::state::Action;

impl Chat {
    /// The ad hoc `net` grant confirm, while open.
    pub(super) fn handle_login_grant(&mut self, key: Option<&str>) -> Option<Action> {
        let prompt = self.world.grant.take()?;
        match key {
            Some("y" | "Y" | "enter") => {
                let message = self.world.options.confirm_login_grant.as_ref().map_or_else(
                    || "nothing was changed".to_string(),
                    |confirm| confirm(&prompt.provider, &prompt.host),
                );
                self.world.notice = Some(crate::state::sanitize_block(&message));
            }
            Some("n" | "N" | "escape") => {
                self.world.notice = Some(format!("kept {} without the ad hoc grant", prompt.host));
            }
            _ => self.world.grant = Some(prompt),
        }
        Some(Action::Continue)
    }

    /// The provider-switch confirm, while open (gh #177).
    pub(super) fn handle_switch_confirm(&mut self, key: Option<&str>) -> Option<Action> {
        let prompt = self.world.switch_confirm.take()?;
        match key {
            Some("y" | "Y" | "enter") => {
                let message = self.world.options.confirm_switch.as_ref().map_or_else(
                    || "nothing was changed".to_string(),
                    |confirm| confirm(&prompt.provider),
                );
                self.world.notice = Some(crate::state::sanitize_block(&message));
            }
            Some("n" | "N" | "escape") => {
                self.world.notice = Some("staying with the current provider".to_string());
            }
            _ => self.world.switch_confirm = Some(prompt),
        }
        Some(Action::Continue)
    }
}
