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
/// The cells the tree's cursor prefix takes from a row (`" > "`).
const TREE_ROW_INSET: usize = 3;
const HINT_TREE: &str =
    "↑↓ move · ←/→ fold · alt+←/→ branch · f filter · e label · enter show · esc close";
const HINT_GRANTS: &str = "↑↓ move · enter revoke · esc close";
const HINT_THEME: &str = "↑↓ preview · enter apply · esc restore";
const HINT_SETTINGS_EDIT: &str = "type the value · enter apply · esc cancel";
/// Pi's `/settings` hint verbatim: the list filters as you type.
const HINT_SETTINGS_SEARCH: &str =
    "↑↓ move · enter/←→ change · tab project · q/esc close · type to search";
const HINT_LOGIN: &str = "↑↓ move · enter choose · esc cancel";

/// The sensitive filename in a permission modal's action text, if the
/// modal asks for a sensitive read (gh #215): the action text is the
/// permission action's own display (`read <path>`), so the `read `
/// prefix peels exactly and the file name decides.
pub(crate) fn sensitive_read_name(action: &str) -> Option<String> {
    let path = std::path::Path::new(action.strip_prefix("read ")?);
    let name = path.file_name()?.to_str()?;
    lca_permissions::sensitive::is_sensitive(path).then(|| name.to_string())
}
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
        // gh #230: the painted window rides in so a hover keeps it.
        let prev = self.picker_window.get();
        let keep = (prev.1 != usize::MAX).then_some((prev.0, prev.1));
        let window: (usize, usize, u16) = overlay_box_picker(
            viewport, width, height, title, &body, theme, selected, above, keep,
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
            side_panel(viewport, width, &panel, &self.theme);
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
        // Gh #397: a stepped settings wizard owns the screen while it walks. It
        // paints above the `/settings` list it was opened from, so the list is
        // back on screen the moment the wizard closes - pi's submenu shape, and
        // the reason a cancel needs no separate "return" path.
        if let Some(wizard) = &self.world.wizard {
            let title = format!("settings · {}", wizard.opener_key);
            let body = wizard.body();
            let hint = wizard.hint();
            self.picker_overlay(
                viewport,
                width,
                height,
                &title,
                &body,
                &hint,
                &self.theme,
                Some(0),
            );
            return true;
        }
        if let Some(picker) = &self.tree_picker {
            // gh #231: the DAG navigator paints connectors and role
            // markers on S37C's rolling-window chrome; the header
            // names the filter, the hint the navigator keys. Gh #248:
            // the header insets one cell like every row, so no modal
            // content touches the box's left edge.
            let mut body = vec![
                format!(" Message branches ({})", picker.filter.name()),
                String::new(),
            ];
            if picker.visible_len() == 0 {
                body.push("  (no matches)".to_string());
            }
            // Gh #394: rows paint at their column budget (the box's inner
            // width less the cells the cursor prefix takes), so the pan and
            // truncate arithmetic sees the width the frame actually has.
            let row_width = usize::from(crate::render::picker_inner_width(width))
                .saturating_sub(TREE_ROW_INSET);
            for (row, line) in picker.paint_rows(&self.theme, row_width).iter().enumerate() {
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
            // Gh #395 gap 1: the row's title comes from the tree painter, which
            // supplies the `│/├─/└─` prefix when the query is empty and the bare
            // title when it is not (a search flattens, by design).
            let painted = picker.paint_rows();
            for (row, index) in picker.matches.iter().enumerate() {
                let entry = &picker.entries[*index];
                let cur = if row == picker.selected { '>' } else { ' ' };
                // gh #75: the confirming row asks inline (same row
                // count, so the hit map holds).
                if picker.confirming == Some(row) {
                    body.push(format!(" {cur} delete '{}'? (y/n)", entry.title));
                    continue;
                }
                // gh #75: bookmark names ride the row they filter by.
                let marks = if entry.labels.is_empty() {
                    String::new()
                } else {
                    format!(" [{}]", entry.labels.join(", "))
                };
                // The painted title carries the tree prefix; the rest of the row
                // grammar (message count, age) is unchanged.
                let title = painted
                    .get(row)
                    .cloned()
                    .unwrap_or_else(|| entry.title.clone());
                body.push(format!(
                    " {cur} {}{} ({} messages, {})",
                    title, marks, entry.messages, entry.age
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
            // Gh #272 parity pass: pi's `ConfigSelector` carries a live
            // type-to-search line; ours becomes one row and only shows
            // while a query is set, so the unfiltered list looks as it
            // did. The visible set drives the cursor, so a filtered-out
            // row can never be selected.
            let visible = picker.visible();
            let mut selected_body;
            if picker.query.is_empty() {
                body.push(String::new());
                selected_body = 2 + picker.selected;
            } else {
                body.push(format!("  search: {}", picker.query));
                body.push(String::new());
                selected_body = 3 + picker.selected;
            }
            if visible.is_empty() {
                // Pi's empty state (`ConfigSelector`): the list says so
                // rather than showing nothing, which would read as a
                // rendering failure (gh #272 parity pass).
                body.push("  No matching settings".to_string());
            }
            let mut section: &str = "";
            for (position, index) in visible.iter().copied().enumerate() {
                let row = &picker.rows[index];
                if !row.section.is_empty() && row.section != section {
                    section = row.section.as_str();
                    body.push(format!(" ── {section} ──"));
                    // gh #235: air under the divider.
                    body.push(String::new());
                    if position <= picker.selected {
                        selected_body += 2;
                    }
                }
                let cur = if index == picker.selected { '>' } else { ' ' };
                // Gh #174: a row being edited shows its buffer, not its
                // stored value.
                if position == picker.selected
                    && let Some(buffer) = picker.editing.as_ref()
                {
                    body.push(format!(" {cur} {} = {}█", row.label, buffer));
                } else {
                    // Gh #397: a project-overridable row shows the tri-state as
                    // a word (NFR-28: never colour alone), so "this project is
                    // overriding your user file" is visible without reading the
                    // source column.
                    let project = match row.project {
                        Some(state) => format!("project {}, ", state.word()),
                        None => String::new(),
                    };
                    body.push(format!(
                        " {cur} {} = {} [{project}{}]",
                        row.label,
                        row.display_value(),
                        row.source
                    ));
                }
            }
            // gh #235: the highlighted row's description rides the
            // pinned hint (outside the items, so the row map holds).
            let hint = if picker.editing.is_some() {
                HINT_SETTINGS_EDIT.to_string()
            } else if let Some(row) = picker.selected_row() {
                let cap = (width as usize)
                    .saturating_sub(HINT_SETTINGS_SEARCH.len() + 12)
                    .max(20);
                let desc =
                    lca_tui::engine::text::truncate_to_width(&row.description, cap, "…", false);
                if desc.is_empty() {
                    HINT_SETTINGS_SEARCH.to_string()
                } else {
                    format!("{} · {desc}", HINT_SETTINGS_SEARCH)
                }
            } else {
                HINT_SETTINGS_SEARCH.to_string()
            };
            self.picker_overlay(
                viewport,
                width,
                height,
                "settings",
                &body,
                &hint,
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
        if let Some(consent) = &self.install_consent {
            // #227: the install-consent step names publisher, exact
            // origin, artifact ref, and declared capabilities before
            // anything is written (FR-PERM-2).
            let mut body = vec![
                format!("{} {}", consent.name, consent.version),
                consent.publisher.clone(),
                format!("source: {}", consent.source_url),
                format!("artifact: {}", consent.artifact_ref),
                String::new(),
            ];
            if consent.capabilities.is_empty() {
                body.push("requests no capabilities.".to_string());
            } else {
                body.push("requests:".to_string());
                for cap in &consent.capabilities {
                    body.push(format!("  - {cap}"));
                }
            }
            body.push(String::new());
            body.push("Approve and install? [y/n]".to_string());
            self.picker_overlay(
                viewport,
                width,
                height,
                "install",
                &body,
                "y approve · n deny · esc back",
                &self.theme,
                None,
            );
            return true;
        }
        if let Some(hub) = &self.extension_hub {
            let mut body = vec![format!("  filter: {}", hub.query), String::new()];
            if hub.rows.is_empty() && hub.loading {
                let frame = crate::separator::FRAMES
                    .get(hub.loading_frame % crate::separator::FRAMES.len());
                body.push(format!("  {} Loading extensions…", frame.unwrap_or(&"…")));
            } else if hub.matches.is_empty() {
                body.push("  (no matches)".to_string());
            }
            let mut last_section = String::new();
            for (row, index) in hub.matches.iter().enumerate() {
                let Some(entry) = hub.rows.get(*index) else {
                    continue;
                };
                let section = if entry.installed {
                    "INSTALLED".to_string()
                } else if entry.official {
                    format!("AVAILABLE — {} [official]", entry.source_name)
                } else {
                    format!("AVAILABLE — {} [third-party]", entry.source_name)
                };
                if section != last_section {
                    body.push(String::new());
                    body.push(format!("  {section}"));
                    last_section = section;
                }
                let cur = if row == hub.selected { '>' } else { ' ' };
                let mark = if entry.installed { "  ✓" } else { "  +" };
                body.push(format!(
                    " {cur} {}{mark} {} v{} [{}]",
                    entry.name,
                    entry.description,
                    entry.version,
                    entry.capabilities.join(", ")
                ));
            }
            if !hub.matches.is_empty() {
                body.push(format!("  ({}/{})", hub.selected + 1, hub.matches.len()));
            }
            self.picker_overlay(
                viewport,
                width,
                height,
                "extensions",
                &body,
                "type to filter · enter install · esc close",
                &self.theme,
                None,
            );
            return true;
        }
        // Gh #418 Phase G: the `/mcp` manager modal. Before the `/ext` manager
        // so a session that somehow opened both shows MCP (it is the one whose
        // actions can be destructive).
        if let Some(manager) = &self.mcp_manager {
            // Gh #398 gap 1: the sign-in screen's URL is the second wrapped-URL
            // trap over SSH, so it gets the same copy hint (and the same in-place
            // confirmation) as the login wait.
            let auth_hint = self.mcp_sign_in_url().map(|url| self.auth_url_hint(&url));
            let (title, body, hint) =
                crate::chat_mcp_render::mcp_overlay(manager, auth_hint.as_deref());
            self.picker_overlay(
                viewport,
                width,
                height,
                &title,
                &body,
                &hint,
                &self.theme,
                None,
            );
            return true;
        }
        if let Some(manager) = &self.extension_manager {
            let mut body = vec![String::new()];
            let mut last_section = String::new();
            for (row, entry) in manager.rows.iter().enumerate() {
                let (section, line) = match entry {
                    crate::chat_extman::ManagedRow::Strategy { name, active } => {
                        let label = name.as_deref().unwrap_or("Automatic");
                        let mark = if *active { "  ●" } else { "  ○" };
                        (
                            "STRATEGY".to_string(),
                            format!(
                                "{} {}{}",
                                if row == manager.selected { '>' } else { ' ' },
                                label,
                                mark
                            ),
                        )
                    }
                    crate::chat_extman::ManagedRow::Extension {
                        name,
                        enabled,
                        compaction,
                    } => {
                        let mark = if *enabled { "  ✓" } else { "  ✗" };
                        let kind = if *compaction { " [compaction]" } else { "" };
                        (
                            "EXTENSIONS".to_string(),
                            format!(
                                "{} {}{}{}",
                                if row == manager.selected { '>' } else { ' ' },
                                name,
                                mark,
                                kind
                            ),
                        )
                    }
                };
                if section != last_section {
                    body.push(String::new());
                    body.push(format!("  {section}"));
                    last_section = section;
                }
                body.push(format!(" {line}"));
            }
            if !manager.rows.is_empty() {
                body.push(format!(
                    "  ({}/{})",
                    manager.selected + 1,
                    manager.rows.len()
                ));
            }
            self.picker_overlay(
                viewport,
                width,
                height,
                "extensions",
                &body,
                "enter toggle/pick · esc close",
                &self.theme,
                None,
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

    /// The waiting screen's body rows (R4, gh #398 gap 1).
    ///
    /// Plain when the step shows no sign-in URL. Otherwise a URL gets the
    /// affordances that make it usable over SSH and in tmux, where the wrapped
    /// rows cannot be mouse-selected cleanly and `OSC 8` click-through is
    /// unavailable:
    ///
    /// - a hint naming the **live** copy binding (the registry, never a
    ///   hardcoded `ctrl+x` - a rebind must change the hint);
    /// - the short `Cmd/Ctrl+click to open` label **only** where the terminal
    ///   forwards `OSC 8`, and then it *is* the link, so a reader clicks one
    ///   word instead of dragging across three wrapped rows. Rendering that
    ///   label on a terminal that cannot click would be a stale hint, which is
    ///   the failure this project keeps closing.
    ///
    /// After a copy the hint row becomes the confirmation, in place, so the
    /// reader who pressed the key sees it worked where they were looking.
    fn login_body(&self, label: &str) -> Vec<String> {
        let Some(url) = self.world.auth_url.as_deref() else {
            return vec![label.to_string(), String::new(), "esc cancels".to_string()];
        };
        vec![
            label.to_string(),
            String::new(),
            self.auth_url_hint(url),
            "esc cancels".to_string(),
        ]
    }

    /// The row under the sign-in URL: the copy confirmation when fresh, else
    /// the hint (gh #398 gap 1).
    pub(crate) fn auth_url_hint(&self, url: &str) -> String {
        let fresh = self.world.url_copied_at.is_some_and(|at| {
            at.elapsed() < std::time::Duration::from_secs(super::chat::TOAST_SECS)
        });
        if fresh {
            return (self.theme.role(crate::theme::Role::Success))("Copied URL to clipboard");
        }
        // pi's `keyHint("app.message.copy", "to copy")`: the key comes from the
        // registry so a rebind is reflected, and the whole hint is dim.
        let copy_key = self.keybindings.display_keys("app.message.copy");
        let mut parts: Vec<String> = Vec::new();
        if self.hyperlinks {
            // pi's `auth-url.ts` labels this by platform, and wraps the label
            // itself in the link so the short word opens the full URL.
            let click = if cfg!(target_os = "macos") {
                "Cmd+click to open"
            } else {
                // The modifier the terminal actually reports for a link click
                // varies; `Ctrl+click` is pi's answer on Linux/Windows and the
                // conservative one - a terminal that treats plain click as
                // select still selects, so the copy hint beside it is the
                // path that always works.
                "Ctrl+click to open"
            };
            parts.push(format!("\x1b]8;;{url}\x07{click}\x1b]8;;\x07"));
        }
        // pi's `keyHint` shape (`keybinding-hints.ts`): the key in `dim`, the
        // words in `muted`, " • " between the two halves. Matching it means the
        // two agents' auth screens read the same way, which is the point of the
        // parity rows.
        let dim = self.theme.role(crate::theme::Role::Dim);
        let muted = self.theme.role(crate::theme::Role::Muted);
        parts.push(format!("{} {}", dim(&copy_key), muted("to copy")));
        parts.join(&format!(" {} ", dim("•")))
    }

    /// Record that the sign-in URL was copied (gh #398 gap 1): the confirmation
    /// shows in place for the toast window. `ok` is false when the copy ladder
    /// could not run, so a failure never claims success.
    pub fn note_url_copied(&mut self, ok: bool) {
        // Either surface can be the one that copied (gh #398 gap 1): the login
        // wait's URL or the MCP sign-in screen's. A failure records nothing, so
        // a broken copy never shows a confirmation.
        self.world.url_copied_at = ok.then(std::time::Instant::now);
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
            // Gh #398 gap 1: when the step shows a sign-in URL, the body
            // carries the copy hint and (where OSC 8 reaches the terminal) the
            // short clickable label, so an SSH reader never has to mouse-select
            // a wrapped URL.
            let body = self.login_body(label);
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
        } else if let Some(review) = &self.world.bug_review {
            // Gh #411 gap 2: the inspection is the gate. The body is the host's
            // rendered summary (file list, sizes, scrub counts), so the screen
            // never re-reads the session and cannot drift from what will be
            // written.
            let mut body: Vec<String> = review.inspection.lines().map(str::to_string).collect();
            body.push(String::new());
            body.push("Write the bundle [y] / Cancel [n]".to_string());
            overlay_box(
                viewport,
                width,
                height,
                "bug bundle - review before sharing",
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
            // Gh #215: a sensitive read names its security boundary —
            // allowing it exposes the file's contents to the active
            // model provider.
            if let Some(name) = sensitive_read_name(&modal.action) {
                body.push(format!(
                    "  ⚠️ Sensitive file read requested ({name}) — allowing this exposes its contents to the active model provider."
                ));
                body.push(String::new());
            }
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
