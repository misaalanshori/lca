//! The chat's overlay composition, split from `chat.rs` (the 1,200-line
//! ceiling). The modals and pickers composite over the visible viewport.

use super::chat::{Chat, highlight_matches};
use super::render::{overlay_box, side_panel};
use super::state::widget_lines;
use crate::chat_pickers::THINKING_LEVELS;

impl Chat {
    /// Composite the modals and the side panel over the viewport.
    pub(super) fn compose_overlays(&self, viewport: &mut [String], width: u16, height: u16) {
        if self.compose_pickers(viewport, width, height) {
            return;
        }
        self.compose_modals(viewport, width, height);

        if self.world.panel_open {
            let mut panel: Vec<String> = Vec::new();
            if let Some(render) = &self.world.options.render_regions {
                for (_name, tree) in render("panel") {
                    panel.extend(widget_lines(&tree.nodes));
                }
            }
            if panel.is_empty() {
                panel.push("(nothing registered for the panel)".to_string());
            }
            side_panel(viewport, width, &panel);
        }

        // Highlight the search matches (FR-UI-12).
        if let Some(query) = &self.search
            && !query.is_empty()
        {
            for line in viewport.iter_mut() {
                *line = highlight_matches(line, query);
            }
        }
    }

    /// Draw the open picker, if any (returns true when one was drawn).
    fn compose_pickers(&self, viewport: &mut [String], width: u16, height: u16) -> bool {
        if let Some(picker) = &self.tree_picker {
            let mut body = vec!["Session branches:".to_string(), String::new()];
            for (index, (_id, label)) in picker.entries.iter().enumerate() {
                let cur = if index == picker.selected { '>' } else { ' ' };
                body.push(format!(" {cur} {label}"));
            }
            body.push(String::new());
            body.push("Up/Down moves; Enter shows the resume command; Esc closes.".to_string());
            overlay_box(viewport, width, height, "tree", &body);
            return true;
        }
        if let Some(picker) = &self.resume_picker {
            let mut body = vec![format!("  search: {}", picker.query), String::new()];
            if picker.matches.is_empty() {
                body.push("  (no matches)".to_string());
            }
            for (row, index) in picker.matches.iter().enumerate() {
                let entry = &picker.entries[*index];
                let cur = if row == picker.selected { '>' } else { ' ' };
                body.push(format!(
                    " {cur} {} ({} messages, {})",
                    entry.title, entry.messages, entry.age
                ));
            }
            body.push(String::new());
            body.push(
                "Type to search (re:/…/, \"phrase\"); Enter shows the resume command; Esc closes."
                    .to_string(),
            );
            overlay_box(viewport, width, height, "resume", &body);
            return true;
        }
        if let Some(picker) = &self.grants_picker {
            let mut body = vec!["Grants for this project:".to_string(), String::new()];
            let mut group: Option<bool> = None;
            for (index, entry) in picker.entries.iter().enumerate() {
                if group != Some(entry.install_consent) {
                    group = Some(entry.install_consent);
                    if !body.last().is_some_and(|line| line.is_empty()) {
                        body.push(String::new());
                    }
                    body.push(if entry.install_consent {
                        "Install consent (revoke with `lca ext disable <name>`):".to_string()
                    } else {
                        "Ad hoc (Enter revokes):".to_string()
                    });
                }
                let cur = if index == picker.selected { '>' } else { ' ' };
                body.push(format!(" {cur} {} - {}", entry.subject, entry.detail));
            }
            body.push(String::new());
            body.push("Up/Down move; Enter revokes an ad hoc grant; Esc closes.".to_string());
            overlay_box(viewport, width, height, "grants", &body);
            return true;
        }
        if let Some(picker) = &self.theme_picker {
            let mut body = vec!["Theme (live preview):".to_string(), String::new()];
            for (index, name) in self.theme_names.iter().enumerate() {
                let cur = if index == picker.selected { '>' } else { ' ' };
                let current = if *name == self.theme_name {
                    "  ✓"
                } else {
                    ""
                };
                body.push(format!(" {cur} {name}{current}"));
            }
            body.push(String::new());
            body.push("Up/Down previews; Enter applies; Esc restores.".to_string());
            overlay_box(viewport, width, height, "theme", &body);
            return true;
        }
        if let Some(picker) = &self.thinking_picker {
            let current = self.thinking_level();
            let mut body = vec!["Thinking level:".to_string(), String::new()];
            let unset = if picker.selected == 0 { '>' } else { ' ' };
            let unset_current = if current.is_none() { "  ✓" } else { "" };
            body.push(format!(" {unset} unset (provider default){unset_current}"));
            for (index, (name, description)) in THINKING_LEVELS.iter().enumerate() {
                let row = index + 1;
                let mark = if picker.selected == row { '>' } else { ' ' };
                let current_mark = if current.as_deref() == Some(*name) {
                    "  ✓"
                } else {
                    ""
                };
                body.push(format!(" {mark} {name:<8} {description}{current_mark}"));
            }
            body.push(String::new());
            body.push("Up/Down moves; Enter applies; Esc closes.".to_string());
            overlay_box(viewport, width, height, "thinking", &body);
            return true;
        }
        if let Some(picker) = &self.model_picker {
            let active = self.model_label();
            let mut body = vec![format!("  search: {}", picker.query), String::new()];
            if picker.matches.is_empty() {
                body.push("  (no matches)".to_string());
            }
            for (row, index) in picker.matches.iter().enumerate() {
                let model = &picker.models[*index];
                let cur = if row == picker.selected { '>' } else { ' ' };
                let mark = if active.ends_with(&format!("/{model}")) {
                    "  ✓"
                } else {
                    ""
                };
                body.push(format!(" {cur} {model}{mark}"));
            }
            body.push(String::new());
            body.push("Type to search; Enter selects; Esc closes.".to_string());
            overlay_box(viewport, width, height, "model", &body);
            return true;
        }
        false
    }

    /// Draw the open login/grant/permission/extension modal, if any.
    fn compose_modals(&self, viewport: &mut [String], width: u16, height: u16) {
        if let Some(picker) = &self.world.picker {
            let rows = 15usize.min(height.saturating_sub(7) as usize).max(1);
            let total = picker.options.len();
            let start = (picker.selected + 1)
                .saturating_sub(rows)
                .min(total.saturating_sub(rows));
            let end = (start + rows).min(total);
            let mut body = vec!["Sign in with:".to_string(), String::new()];
            for (index, option) in picker.options.iter().enumerate().take(end).skip(start) {
                let cur = if index == picker.selected { '>' } else { ' ' };
                let hint = if option.hint.is_empty() {
                    String::new()
                } else {
                    format!("   {}", option.hint)
                };
                body.push(format!(" {cur} {}{hint}", option.label));
            }
            if start > 0 || end < total {
                body.push(format!("   [{}/{}]", picker.selected + 1, total));
            }
            body.push(String::new());
            body.push("Up/Down moves; Enter chooses; Esc cancels.".to_string());
            overlay_box(viewport, width, height, "login", &body);
        } else if let Some(grant) = &self.world.grant {
            let body = vec![
                grant.prompt.clone(),
                String::new(),
                format!("  connect to {}", grant.host),
                String::new(),
                "Allow [y] / Deny [n]".to_string(),
            ];
            overlay_box(
                viewport,
                width,
                height,
                &format!("ad hoc grant: {}", grant.provider),
                &body,
            );
        } else if let Some(secret) = &self.world.secret {
            let shown = if secret.masked {
                let masked = "*".repeat(secret.input.chars().count());
                if masked.is_empty() {
                    "(type the secret)".to_string()
                } else {
                    masked
                }
            } else if secret.input.is_empty() {
                "(type a value)".to_string()
            } else {
                secret.input.clone()
            };
            let body = vec![
                secret.label.clone(),
                String::new(),
                format!("  {shown}"),
                String::new(),
                "Enter stores it; Esc cancels.".to_string(),
            ];
            overlay_box(
                viewport,
                width,
                height,
                &format!("login: {}", secret.provider),
                &body,
            );
        } else if let Some(modal) = &self.world.permission {
            let mut body = vec![
                "Allow this action?".to_string(),
                String::new(),
                format!("  {}", modal.action),
                String::new(),
            ];
            if let Some(deadline) = modal.deadline {
                let seconds = deadline
                    .saturating_duration_since(std::time::Instant::now())
                    .as_secs();
                body.push(format!(
                    "auto-approves in {seconds}s - press a key to decide (FR-UI-18)"
                ));
                body.push(String::new());
            }
            body.push("Allow once [o] / Allow always for this pattern [a] / Deny [d]".to_string());
            overlay_box(viewport, width, height, "permission required", &body);
        } else if self.world.modal_open {
            let trees = self
                .world
                .options
                .render_regions
                .as_ref()
                .map(|render| render("modal"))
                .unwrap_or_default();
            let mut body: Vec<String> = Vec::new();
            let mut title = String::from("extension");
            for (name, tree) in &trees {
                for line in widget_lines(&tree.nodes) {
                    if title == "extension" && line.starts_with('[') {
                        title = line.trim_matches(['[', ']']).to_string();
                    }
                    body.push(line);
                }
                if body.is_empty() {
                    body.push(name.clone());
                }
            }
            overlay_box(viewport, width, height, &title, &body);
        }
    }
}
