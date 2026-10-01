//! The chat's overlay composition, split from `chat.rs` (the 1,200-line
//! ceiling). The modals and pickers composite over the visible viewport.

use super::chat::{Chat, highlight_matches};
use super::render::{overlay_box, side_panel};
use super::state::widget_lines;
use crate::chat_pickers::{THINKING_LEVELS, TRUST_OPTIONS};

// E3: a picker owns the keyboard while open, so a slash command typed into
// one lands in its search box. The shared hint row says what the keys do,
// per picker, instead of leaving that surprising. Every picker overlay is
// composed through [`picker_overlay`], so a new picker gets the row too.
const HINT_MOVE: &str = "↑↓ move · enter apply · esc close";
const HINT_FILTER: &str = "↑↓ move · enter apply · esc close · type to filter";
const HINT_TREE: &str = "↑↓ move · enter show · esc close";
const HINT_GRANTS: &str = "↑↓ move · enter revoke · esc close";
const HINT_THEME: &str = "↑↓ preview · enter apply · esc restore";
const HINT_LOGIN: &str = "↑↓ move · enter choose · esc cancel";

/// Compose one picker overlay: the body plus its hint row (E3).
fn picker_overlay(
    viewport: &mut [String],
    width: u16,
    height: u16,
    title: &str,
    body: &[String],
    hint: &str,
    theme: &crate::theme::Theme,
) {
    let mut body = body.to_vec();
    body.push(String::new());
    body.push(hint.to_string());
    overlay_box(viewport, width, height, title, &body, theme);
}

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
            picker_overlay(
                viewport,
                width,
                height,
                "tree",
                &body,
                HINT_TREE,
                &self.theme,
            );
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
            picker_overlay(
                viewport,
                width,
                height,
                "resume",
                &body,
                HINT_FILTER,
                &self.theme,
            );
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
            picker_overlay(
                viewport,
                width,
                height,
                "grants",
                &body,
                HINT_GRANTS,
                &self.theme,
            );
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
            picker_overlay(
                viewport,
                width,
                height,
                "theme",
                &body,
                HINT_THEME,
                &self.theme,
            );
            return true;
        }
        if let Some(picker) = &self.trust_picker {
            let mut body = vec![
                "Trust this project folder?".to_string(),
                String::new(),
                "Trusted folders run in-workspace commands without a prompt.".to_string(),
                String::new(),
            ];
            for (index, label) in TRUST_OPTIONS.iter().enumerate() {
                let cur = if index == picker.selected { '>' } else { ' ' };
                body.push(format!(" {cur} {label}"));
            }
            picker_overlay(
                viewport,
                width,
                height,
                "trust",
                &body,
                HINT_MOVE,
                &self.theme,
            );
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
            picker_overlay(
                viewport,
                width,
                height,
                "thinking",
                &body,
                HINT_MOVE,
                &self.theme,
            );
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
            picker_overlay(
                viewport,
                width,
                height,
                "model",
                &body,
                HINT_FILTER,
                &self.theme,
            );
            return true;
        }
        false
    }

    /// Draw the open login/grant/permission/extension modal, if any.
    fn compose_modals(&self, viewport: &mut [String], width: u16, height: u16) {
        if let Some(label) = &self.world.login_waiting {
            // R4: a cancellable waiting state, so a slow OAuth callback is
            // visible and interruptible instead of freezing the app.
            let body = vec![label.clone(), String::new(), "esc cancels".to_string()];
            overlay_box(viewport, width, height, "login", &body, &self.theme);
        } else if let Some(picker) = &self.world.picker {
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
            picker_overlay(
                viewport,
                width,
                height,
                "login",
                &body,
                HINT_LOGIN,
                &self.theme,
            );
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
                &self.theme,
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
                &self.theme,
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
                    "auto-approves in {seconds}s - press a key to decide"
                ));
                body.push(String::new());
            }
            body.push(
                "[o] once / [a] always this pattern / [t] trust this folder this session / [d] deny"
                    .to_string(),
            );
            overlay_box(
                viewport,
                width,
                height,
                "permission required",
                &body,
                &self.theme,
            );
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
            overlay_box(viewport, width, height, &title, &body, &self.theme);
        }
    }
}
