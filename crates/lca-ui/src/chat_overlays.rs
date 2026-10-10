//! The chat's overlay composition, split from `chat.rs` (the 1,200-line
//! ceiling). The modals and pickers composite over the visible viewport.

use super::chat::{Chat, highlight_matches};
use super::ext_widgets::widget_lines;
use super::render::{overlay_box, overlay_box_picker, side_panel};
use crate::chat_pickers::TRUST_OPTIONS;

// E3: a picker owns the keyboard while open, so a slash command typed into
// one lands in its search box. The shared hint row says what the keys do,
// per picker, instead of leaving that surprising. Every picker overlay is
// composed through [`Chat::picker_overlay`], so a new picker gets the row too.
const HINT_MOVE: &str = "↑↓ move · enter apply · esc close";
const HINT_FILTER: &str = "↑↓ move · enter apply · esc close · type to filter";
const HINT_TREE: &str = "↑↓ move · ←/→ fold · f filter · e label · enter show · esc close";
const HINT_GRANTS: &str = "↑↓ move · enter revoke · esc close";
const HINT_THEME: &str = "↑↓ preview · enter apply · esc restore";
const HINT_SETTINGS: &str = "↑↓ move · enter/←→ change · q/esc close";
const HINT_SETTINGS_EDIT: &str = "type the value · enter apply · esc cancel";
const HINT_LOGIN: &str = "↑↓ move · enter choose · esc cancel";
const HINT_SCOPED_MODELS: &str = "↑↓ move · space toggle · a all · enter save · esc close";

/// The modal's title from one rendered line (gh #172): a bordered box
/// paints its `[title]` in the border role, so the chrome strips escapes
/// before reading it. `None` is not a title line.
pub(super) fn modal_title_line(line: &str) -> Option<String> {
    let plain = lca_tui::engine::text::strip_terminal_sequences(line);
    plain
        .starts_with('[')
        .then(|| plain.trim_matches(['[', ']']).to_string())
}

impl Chat {
    /// Compose one picker overlay: the body plus its hint row (E3),
    /// bottom-anchored above the composer (gh #16 - pi's shape), so a
    /// tall notice is overlaid, never stacked under.
    // Geometry, title, body, hint, theme, selection: seven inputs is the
    // picker contract; a parameter struct would only rename them.
    #[allow(clippy::too_many_arguments)]
    fn picker_overlay(
        &self,
        viewport: &mut [String],
        width: u16,
        height: u16,
        title: &str,
        body: &[String],
        hint: &str,
        theme: &crate::theme::Theme,
        selected: Option<usize>,
    ) {
        let mut body = body.to_vec();
        body.push(String::new());
        body.push(hint.to_string());
        // The hint rows sit past the caller's body, so its index still lands
        // on the selected row.
        let above = self.composer_height(width);
        let window: (usize, usize, u16) = overlay_box_picker(
            viewport, width, height, title, &body, theme, selected, above,
        );
        // The mouse hit test reads back exactly what this frame drew.
        self.picker_window.set(window);
    }
}

impl Chat {
    /// Composite overlays bottom-anchored into a full document line buffer
    /// for main-screen (scrollback) rendering.
    pub(super) fn compose_overlays_bottom_anchored(
        &self,
        lines: &mut Vec<String>,
        width: u16,
        height: u16,
    ) {
        // Pad to at least terminal height so overlays have screen-relative positions
        // at the visible bottom.
        let working_height = lines.len().max(height as usize);
        if lines.len() < working_height {
            lines.resize(working_height, String::new());
        }
        let viewport_start = lines.len().saturating_sub(height as usize);
        let viewport_slice = &mut lines[viewport_start..];
        self.compose_overlays(viewport_slice, width, height);
    }

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
                    panel.extend(widget_lines(
                        &tree.nodes,
                        &crate::ext_widgets::widget_ctx(
                            &self.theme,
                            "panel",
                            width as usize,
                            &self.world.ext_scroll,
                        ),
                    ));
                }
            }
            if panel.is_empty() {
                panel.push("(nothing registered for the panel)".to_string());
            }
            side_panel(viewport, width, &panel);
        }
        self.paint_drawer_tab(viewport, width, height);

        // Highlight the search matches (FR-UI-12).
        if let Some(query) = &self.search
            && !query.is_empty()
        {
            for line in viewport.iter_mut() {
                *line = highlight_matches(line, query);
            }
        }
    }

    /// Paint the drawer tab over the margin (gh #207): `◀` opens, `▶`
    /// closes - the visible face of the Alt+X binding. One cell only,
    /// accent on hover, so an idle frame stays quiet.
    fn paint_drawer_tab(&self, viewport: &mut [String], width: u16, height: u16) {
        let Some((col, row)) = self.drawer_rect(width, height) else {
            return;
        };
        let Some(line) = viewport.get_mut(row as usize) else {
            return;
        };
        let glyph = if self.world.panel_open { "▶" } else { "◀" };
        let role = if self.drawer_hover {
            crate::theme::Role::Accent
        } else {
            crate::theme::Role::Border
        };
        // gh #238: the glyph splices at the seam, so the row behind
        // cannot tint it and its accent cannot leak right.
        *line =
            crate::render::splice_segment(line, col as usize, 1, &(self.theme.role(role))(glyph));
    }

    /// Draw the open picker, if any (returns true when one was drawn).
    fn compose_pickers(&self, viewport: &mut [String], width: u16, height: u16) -> bool {
        if let Some(picker) = &self.tree_picker {
            // gh #231: the DAG navigator paints connectors and role
            // markers on S37C's rolling-window chrome; the header
            // names the filter, the hint the navigator keys.
            let mut body = vec![
                format!("Message branches ({})", picker.filter.name()),
                String::new(),
            ];
            if picker.visible_len() == 0 {
                body.push("  (no matches)".to_string());
            }
            for (row, line) in picker.paint_rows().iter().enumerate() {
                let cur = if row == picker.selected { '>' } else { ' ' };
                body.push(format!(" {cur} {line}"));
            }
            self.picker_overlay(
                viewport,
                width,
                height,
                "tree",
                &body,
                HINT_TREE,
                &self.theme,
                (picker.visible_len() > 0).then_some(2 + picker.selected),
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
            self.picker_overlay(
                viewport,
                width,
                height,
                "resume",
                &body,
                HINT_FILTER,
                &self.theme,
                (!picker.matches.is_empty()).then_some(2 + picker.selected),
            );
            return true;
        }
        // Gh #203: the `/fork` user-message picker - pi's
        // `UserMessageSelector` shape (one-line preview + position) in
        // our picker chrome.
        if let Some(picker) = &self.fork_picker {
            let mut body = vec![
                "Select a user message to fork into a new session".to_string(),
                String::new(),
            ];
            for (row, text) in picker.messages.iter().enumerate() {
                let cur = if row == picker.selected { '>' } else { ' ' };
                let preview = text.replace('\n', " ");
                let preview = preview.trim();
                body.push(format!(" {cur} {preview}"));
                body.push(format!(
                    "   Message {} of {}",
                    row + 1,
                    picker.messages.len()
                ));
                body.push(String::new());
            }
            self.picker_overlay(
                viewport,
                width,
                height,
                "fork",
                &body,
                HINT_MOVE,
                &self.theme,
                (!picker.messages.is_empty()).then_some(2 + picker.selected * 3),
            );
            return true;
        }
        // Gh #204: the `/scoped-models` checklist - checkbox + label +
        // context, pi's `scoped-models-selector` rows in our chrome.
        if let Some(picker) = &self.scoped_models_picker {
            let mut body = vec![
                "Toggle the quick-cycle rotation (Ctrl+P):".to_string(),
                String::new(),
            ];
            for (row, item) in picker.rows.iter().enumerate() {
                let mark = if picker.is_checked(&item.id) {
                    "[x]"
                } else {
                    "[ ]"
                };
                let cur = if row == picker.selected { '>' } else { ' ' };
                let context = if item.context == 0 {
                    String::new()
                } else {
                    format!(" ({}k ctx)", item.context / 1_000)
                };
                body.push(format!(" {cur} {mark} {}{context}", item.label));
            }
            self.picker_overlay(
                viewport,
                width,
                height,
                "scoped-models",
                &body,
                HINT_SCOPED_MODELS,
                &self.theme,
                (!picker.rows.is_empty()).then_some(2 + picker.selected),
            );
            return true;
        }
        if let Some(picker) = &self.grants_picker {
            let mut body = vec!["Grants for this project:".to_string(), String::new()];
            let mut group: Option<bool> = None;
            let mut selected_row = None;
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
                if index == picker.selected {
                    selected_row = Some(body.len() - 1);
                }
            }
            self.picker_overlay(
                viewport,
                width,
                height,
                "grants",
                &body,
                HINT_GRANTS,
                &self.theme,
                selected_row,
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
            self.picker_overlay(
                viewport,
                width,
                height,
                "theme",
                &body,
                HINT_THEME,
                &self.theme,
                Some(2 + picker.selected),
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
            self.picker_overlay(
                viewport,
                width,
                height,
                "trust",
                &body,
                HINT_MOVE,
                &self.theme,
                Some(4 + picker.selected),
            );
            return true;
        }
        if let Some(picker) = &self.thinking_picker {
            let current = self.thinking_level();
            let mut body = vec!["Thinking level:".to_string(), String::new()];
            let unset = if picker.selected == 0 { '>' } else { ' ' };
            let unset_current = if current.is_none() { "  ✓" } else { "" };
            body.push(format!(" {unset} unset (provider default){unset_current}"));
            for (index, name) in picker.offered.iter().enumerate() {
                let row = index + 1;
                let mark = if picker.selected == row { '>' } else { ' ' };
                let current_mark = if current.as_deref() == Some(name.as_str()) {
                    "  ✓"
                } else {
                    ""
                };
                body.push(format!(
                    " {mark} {name:<8} {}{current_mark}",
                    super::chat_pickers::thinking_description(name)
                ));
            }
            self.picker_overlay(
                viewport,
                width,
                height,
                "thinking",
                &body,
                HINT_MOVE,
                &self.theme,
                Some(2 + picker.selected),
            );
            return true;
        }
        // Gh #174: categorized settings - consecutive rows sharing a
        // section share one divider; the cursor only ever sits on rows
        // (selection is row-indexed, so headers skip themselves).
        if let Some(picker) = &self.settings_picker {
            let mut body = vec!["key = value [source]; /grants for permissions".to_string()];
            body.push(String::new());
            let mut section: &str = "";
            let mut selected_body = 2 + picker.selected;
            for (index, row) in picker.rows.iter().enumerate() {
                if !row.section.is_empty() && row.section != section {
                    section = row.section.as_str();
                    body.push(format!(" ── {section} ──"));
                    if index <= picker.selected {
                        selected_body += 1;
                    }
                }
                let cur = if index == picker.selected { '>' } else { ' ' };
                // Gh #174: a row being edited shows its buffer, not its
                // stored value.
                if index == picker.selected
                    && let Some(buffer) = picker.editing.as_ref()
                {
                    body.push(format!(" {cur} {} = {}█", row.key, buffer));
                } else {
                    body.push(format!(
                        " {cur} {} = {} [{}]",
                        row.key, row.value, row.source
                    ));
                }
            }
            self.picker_overlay(
                viewport,
                width,
                height,
                "settings",
                &body,
                if picker.editing.is_some() {
                    HINT_SETTINGS_EDIT
                } else {
                    HINT_SETTINGS
                },
                &self.theme,
                Some(selected_body),
            );
            return true;
        }
        if let Some(picker) = &self.model_picker {
            let active = self.model_label();
            let mut body = vec![format!("  search: {}", picker.query), String::new()];
            if picker.models.is_empty() && picker.loading {
                // gh #232: the background discovery is still running -
                // a spinner line, never a blank box.
                let frame = crate::separator::FRAMES
                    .get(picker.loading_frame % crate::separator::FRAMES.len());
                body.push(format!("  {} Loading models…", frame.unwrap_or(&"…")));
            } else if picker.matches.is_empty() {
                body.push("  (no matches)".to_string());
            }
            for (row, index) in picker.matches.iter().enumerate() {
                let (raw_id, label) = &picker.models[*index];
                if label.trim().is_empty() {
                    continue;
                }
                let cur = if row == picker.selected { '>' } else { ' ' };
                let mark = if active.ends_with(&format!("/{raw_id}")) || active == raw_id.as_str() {
                    "  ✓"
                } else {
                    ""
                };
                body.push(format!(" {cur} {label}{mark}"));
            }
            // pi's `(1/126)` position/size row (model-selector.ts): the
            // list says where you are in it and how big it is.
            if !picker.matches.is_empty() {
                body.push(format!(
                    "  ({}/{})",
                    picker.selected + 1,
                    picker.matches.len()
                ));
            }
            self.picker_overlay(
                viewport,
                width,
                height,
                "model",
                &body,
                HINT_FILTER,
                &self.theme,
                (!picker.matches.is_empty()).then_some(2 + picker.selected),
            );
            return true;
        }
        false
    }

    /// Draw one host-rendered dialog (gh #124): confirm is a yes/no
    /// box, select is a filterable list on the pickers' chrome, input
    /// is one line. The extension's name never appears - the chrome is
    /// the host's, so the user always knows who is asking.
    fn compose_dialog(
        &self,
        viewport: &mut [String],
        width: u16,
        height: u16,
        modal: &crate::state::DialogModal,
    ) {
        use lca_protocol::UiDialog;
        match &modal.exchange.dialog {
            UiDialog::Confirm { title, message } => {
                let body = vec![
                    message.clone(),
                    String::new(),
                    "Yes [y] / No [n]".to_string(),
                ];
                super::render::overlay_box(viewport, width, height, title, &body, &self.theme);
            }
            UiDialog::Select { title, options, .. } => {
                let rows = 10usize.min(height.saturating_sub(7) as usize).max(1);
                let total = modal.matches.len();
                let start = modal
                    .selected
                    .saturating_sub(rows - 1)
                    .min(total.saturating_sub(rows));
                let end = (start + rows).min(total);
                let mut body = Vec::new();
                if !modal.query.is_empty() {
                    body.push(format!("> {}", modal.query));
                }
                let base = body.len();
                for position in start..end {
                    let marker = if position == modal.selected { '>' } else { ' ' };
                    let option = modal
                        .matches
                        .get(position)
                        .and_then(|index| options.get(*index))
                        .map(String::as_str)
                        .unwrap_or("");
                    body.push(format!(" {marker} {option}"));
                }
                if modal.matches.is_empty() {
                    body.push(" (no matches)".to_string());
                }
                let selected = (!modal.matches.is_empty())
                    .then(|| base + modal.selected.saturating_sub(start));
                super::render::overlay_box_selected(
                    viewport,
                    width,
                    height,
                    title,
                    &body,
                    &self.theme,
                    selected,
                );
            }
            UiDialog::Input { label, .. } => {
                let shown = if modal.input.is_empty() {
                    "(type a value)".to_string()
                } else {
                    modal.input.clone()
                };
                let body = vec![
                    format!("  {shown}"),
                    String::new(),
                    "Enter submits; Esc cancels.".to_string(),
                ];
                super::render::overlay_box(viewport, width, height, label, &body, &self.theme);
            }
            UiDialog::Notify { .. } => {
                // Never opens (the drain notices and answers); reaching
                // here means a bug, and a stuck modal is worse than a
                // dropped notice.
            }
        }
    }

    /// Draw the open login/grant/permission/extension modal, if any.
    /// Compose the extension modal over the viewport.
    fn compose_modals(&self, viewport: &mut [String], width: u16, height: u16) {
        // A host-rendered dialog owns the screen outright (gh #124):
        // nothing stacks under it while it is open.
        if let Some(modal) = &self.world.dialog {
            self.compose_dialog(viewport, width, height, modal);
        } else if let Some(label) = &self.world.login_waiting {
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
            self.picker_overlay(
                viewport,
                width,
                height,
                "login",
                &body,
                HINT_LOGIN,
                &self.theme,
                Some(2 + picker.selected.saturating_sub(start)),
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
        } else if let Some(switch) = &self.world.switch_confirm {
            let body = vec![
                switch.prompt.clone(),
                String::new(),
                "Switch [y] / Stay [n]".to_string(),
            ];
            overlay_box(
                viewport,
                width,
                height,
                &format!("switch provider: {}", switch.provider),
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
                for line in widget_lines(
                    &tree.nodes,
                    &crate::ext_widgets::widget_ctx(
                        &self.theme,
                        "modal",
                        width as usize,
                        &self.world.ext_scroll,
                    ),
                ) {
                    // A bordered box paints its title (gh #172): the
                    // chrome reads through the escapes.
                    if title == "extension"
                        && let Some(name) = modal_title_line(&line)
                    {
                        title = name;
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
