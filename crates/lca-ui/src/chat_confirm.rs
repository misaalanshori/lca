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
                self.end_login_with(&crate::state::sanitize_block(&message));
            }
            Some("n" | "N" | "escape") => {
                self.end_login_with(&format!("kept {} without the ad hoc grant", prompt.host));
            }
            _ => self.world.grant = Some(prompt),
        }
        Some(Action::Continue)
    }

    /// The `/bug` inspection screen, while open (gh #411 gap 2).
    ///
    /// The inspection **is** the gate: nothing is zipped until the user says so.
    /// `y`/Enter confirms, `n`/Escape cancels, and any other key keeps the screen
    /// up rather than guessing - the whole point is that the user reads it.
    /// Cancelling routes to the host's resolve with `false`, which removes the
    /// staging directory; a half-written bundle left in `/tmp` is exactly the
    /// accident this prevents.
    pub(super) fn handle_bug_review(&mut self, key: Option<&str>) -> Option<Action> {
        let review = self.world.bug_review.take()?;
        match key {
            Some("y" | "Y" | "enter") => {
                let notice = (review.resolve)(true);
                self.say(crate::state::sanitize_block(&notice));
            }
            Some("n" | "N" | "escape") => {
                let notice = (review.resolve)(false);
                self.say(crate::state::sanitize_block(&notice));
            }
            // Every other key keeps the screen up: the user is reading it.
            _ => self.world.bug_review = Some(review),
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
                self.say(crate::state::sanitize_block(&message));
            }
            Some("n" | "N" | "escape") => {
                self.say("staying with the current provider".to_string());
            }
            _ => self.world.switch_confirm = Some(prompt),
        }
        Some(Action::Continue)
    }
}
