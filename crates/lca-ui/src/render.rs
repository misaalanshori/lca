//! Render `UiState` through the new engine, replacing the legacy ratatui
//! `draw_frame`. Pure: a function of the state and the width/height.
//!
//! The transcript flows as line strings (main-screen scrollback); the dock
//! (input + status + extension footer) sits below; modals composite a
//! centered box over the tail. Extensions never write escape sequences —
//! their widget trees pass through `widget_lines` (ADR-0003).

use lca_tui::engine::core::CURSOR_MARKER;
use lca_tui::engine::text::{truncate_to_width, visible_width, wrap_text_with_ansi};
use lca_tui::widgets::markdown::{MarkdownOptions, render_markdown};

use crate::state::{UiState, widget_lines};
use crate::theme::Theme;

/// A centered box drawn over the visible tail of `base` (the last
/// `height` lines), which is what main-screen mode shows.
fn overlay_box(base: &mut [String], width: u16, height: u16, title: &str, body: &[String]) {
    let w = (width as usize).saturating_sub(8).clamp(20, 76);
    let inner = w.saturating_sub(4);
    let mut box_lines: Vec<String> = Vec::new();
    box_lines.push(format!(
        "╭─ {title} {}",
        "─".repeat(w.saturating_sub(title.len() + 5))
    ));
    for line in body {
        for wrapped in wrap_text_with_ansi(line, inner) {
            box_lines.push(format!("│ {wrapped}"));
        }
    }
    box_lines.push(format!("╰{}", "─".repeat(w.saturating_sub(1))));
    let box_h = box_lines.len().min(height as usize);
    let col = (width as usize).saturating_sub(w) / 2;
    // Operate on the visible tail, not the whole (scrolled) document, and
    // center within whatever the tail actually is (it may be shorter than
    // the terminal height).
    let start = base.len().saturating_sub(height as usize);
    let tail = &mut base[start..];
    let top = tail.len().saturating_sub(box_h) / 2;
    for (i, line) in box_lines.iter().take(box_h).enumerate() {
        let row = top + i;
        if let Some(base_line) = tail.get_mut(row) {
            let before = lca_tui::engine::text::slice_by_column(base_line, 0, col, false);
            let before = format!(
                "{before}{}",
                " ".repeat(col.saturating_sub(visible_width(&before)))
            );
            let after_start = col + visible_width(line);
            let after =
                lca_tui::engine::text::slice_by_column(base_line, after_start, 10_000, false);
            let pad = " ".repeat(w.saturating_sub(visible_width(line)));
            *base_line = format!("{before}{line}{pad}{after}");
        }
    }
}

/// Render the whole interface at `width`x`height`.
pub fn render_state(state: &UiState, width: u16, height: u16) -> Vec<String> {
    let theme = if state.options.plain {
        Theme::plain()
    } else {
        Theme::colored()
    };
    let w = width as usize;

    // --- Transcript -------------------------------------------------------
    let mut lines: Vec<String> = state.scrollback.clone();
    if let Some(notice) = &state.notice {
        lines.push((theme.warn)(&format!("• {notice}")));
    }
    if !state.reasoning.is_empty() {
        for l in wrap_text_with_ansi(&state.reasoning, w.saturating_sub(2).max(1)) {
            lines.push(format!(
                "{} {}",
                (theme.reasoning)("∴"),
                (theme.reasoning)(&l)
            ));
        }
    }
    if !state.active.is_empty() {
        lines.extend(render_markdown(
            &state.active,
            w,
            &theme.markdown(),
            &MarkdownOptions::default(),
        ));
    }
    if let Some(tool) = &state.tool_line {
        lines.push((theme.tool)(tool));
    }
    if lines.is_empty() {
        lines.push((theme.dim)(
            "Type a message to start.  /help for commands, /login to sign in.",
        ));
    }

    // --- Dock -------------------------------------------------------------
    lines.push(String::new());
    lines.push((theme.dim)(&"─".repeat(w)));

    // Extension footer segments.
    let footer_trees = state
        .options
        .render_regions
        .as_ref()
        .map(|render| render("footer"))
        .unwrap_or_default();
    let footer_lines: Vec<String> = footer_trees
        .iter()
        .flat_map(|(_, tree)| widget_lines(&tree.nodes))
        .take(3)
        .collect();
    for line in &footer_lines {
        lines.push((theme.dim)(line));
    }

    // Input, with the cursor marker.
    let cursor = state.cursor.unwrap_or(state.buffer.len());
    let (before, after) = state.buffer.split_at(cursor.min(state.buffer.len()));
    let input = format!("{} {before}{CURSOR_MARKER}{after}", (theme.accent)(">"));
    lines.extend(wrap_text_with_ansi(&input, w));

    // Status line.
    let model_label = state
        .options
        .model_label
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    let model_label = if model_label.trim().is_empty() {
        "no model".to_string()
    } else {
        model_label
    };
    let mut status = (theme.accent)(&model_label);
    status.push_str(&(theme.dim)(&format!(
        "  in {} · cache {} · out {}",
        state.usage.input, state.usage.cache_read, state.usage.output
    )));
    if state.usage.cost > 0.0 {
        status.push_str(&(theme.dim)(&format!(" · ${:.4}", state.usage.cost)));
    }
    if let Some(cue) = &state.turn_status {
        status.push_str(&(theme.success)(&format!("  · {}", cue.text)));
    }
    if let Some(notice) = state
        .options
        .update_notice
        .as_ref()
        .and_then(|cell| cell.get())
    {
        status.push_str(&(theme.warn)(&format!("  · {notice}")));
    }
    if let Some(render) = &state.options.render_regions {
        for (_name, tree) in render("status-line") {
            for line in widget_lines(&tree.nodes).into_iter().take(1) {
                status.push_str(&(theme.accent)(&format!("  · {line}")));
            }
        }
    }
    lines.push(truncate_to_width(&status, w, "…", false));

    // --- Modals (composite over the tail) ---------------------------------
    // A modal owns the screen: pad the document to the terminal height so
    // the overlay has a full viewport to center in (main-screen mode
    // otherwise leaves the tail as short as the document).
    let modal_open = state.picker.is_some()
        || state.grant.is_some()
        || state.secret.is_some()
        || state.permission.is_some()
        || state.modal_open;
    if modal_open {
        while lines.len() < height as usize {
            lines.push(String::new());
        }
    }
    if let Some(picker) = &state.picker {
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
        overlay_box(&mut lines, width, height, "login", &body);
    } else if let Some(grant) = &state.grant {
        let body = vec![
            grant.prompt.clone(),
            String::new(),
            format!("  connect to {}", grant.host),
            String::new(),
            "Allow [y] / Deny [n]".to_string(),
        ];
        overlay_box(
            &mut lines,
            width,
            height,
            &format!("ad hoc grant: {}", grant.provider),
            &body,
        );
    } else if let Some(secret) = &state.secret {
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
            &mut lines,
            width,
            height,
            &format!("login: {}", secret.provider),
            &body,
        );
    } else if let Some(modal) = &state.permission {
        let body = vec![
            "Allow this action?".to_string(),
            String::new(),
            format!("  {}", modal.action),
            String::new(),
            "Allow once [o] / Allow always for this pattern [a] / Deny [d]".to_string(),
        ];
        overlay_box(&mut lines, width, height, "permission required", &body);
    } else if state.modal_open {
        let trees = state
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
        overlay_box(&mut lines, width, height, &title, &body);
    }

    // --- Side panel (right column) ----------------------------------------
    if state.panel_open {
        let mut panel: Vec<String> = Vec::new();
        if let Some(render) = &state.options.render_regions {
            for (_name, tree) in render("panel") {
                panel.extend(widget_lines(&tree.nodes));
            }
        }
        if panel.is_empty() {
            panel.push("(nothing registered for the panel)".to_string());
        }
        let panel_w = 40usize.min(w / 2);
        for (i, base) in lines.iter_mut().enumerate() {
            let text = panel.get(i).map(String::as_str).unwrap_or("");
            let text = truncate_to_width(text, panel_w, "…", false);
            let col = w.saturating_sub(panel_w);
            let before = lca_tui::engine::text::slice_by_column(base, 0, col, false);
            let before = format!(
                "{before}{}",
                " ".repeat(col.saturating_sub(visible_width(&before)))
            );
            *base = format!("{before}{text}");
        }
    }

    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{SecretPrompt, UiOptions};
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    fn options() -> UiOptions {
        UiOptions {
            model_label: Arc::new(Mutex::new("p/m".into())),
            initial_lines: Vec::new(),
            plain: false,
            invoke_command: Arc::new(|_, _| lca_protocol::CommandEffect::None),
            slash_commands: Vec::new(),
            workspace: PathBuf::new(),
            render_regions: None,
            ui_events: None,
            update_notice: None,
            login: None,
            complete_login: None,
            pick_login: None,
            confirm_login_grant: None,
        }
    }

    #[test]
    fn secret_modal_is_centered_in_a_full_height_viewport() {
        let mut state = UiState::new(options());
        state.secret = Some(SecretPrompt {
            provider: "openai-compatible".into(),
            label: "API key (input hidden)".into(),
            input: String::new(),
            masked: true,
        });
        let lines = render_state(&state, 80, 40);
        assert_eq!(lines.len(), 40, "padded to the viewport");
        let row = lines
            .iter()
            .position(|l| l.contains("input hidden"))
            .expect("the label is rendered");
        assert!((10..=25).contains(&row), "modal centered, got row {row}");
    }

    #[test]
    fn no_modal_does_not_pad() {
        let state = UiState::new(options());
        let lines = render_state(&state, 80, 40);
        assert!(lines.len() < 40, "no padding without a modal");
    }
}
