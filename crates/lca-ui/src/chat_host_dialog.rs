//! The host-rendered dialog fields (gh #124), split from `chat_overlays.rs`
//! at the workspace ceiling.
//!
//! A dialog is the extension asking a question through the host's chrome, so the
//! extension's name never appears: the frame must read as the agent asking. Gh
//! #371 bug 3 gave the two keyboard-owning kinds their carets here, because these
//! are fields like any other - they ask `Chat::keyboard_owner`, the one
//! predicate, rather than deciding for themselves that they have focus.

use super::chat::Chat;
use crate::chat_focus::Field;

impl Chat {
    /// Draw one host-rendered dialog (gh #124): confirm is a yes/no box,
    /// select is a filterable list on the pickers' chrome, input is one line.
    /// The extension's name never appears - the chrome is the host's, so the
    /// user always knows who is asking.
    pub(super) fn compose_host_dialog(
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
            UiDialog::Select { .. } => self.compose_select_dialog(viewport, width, height, modal),
            UiDialog::Input { .. } => self.compose_input_dialog(viewport, width, height, modal),
            UiDialog::Notify { .. } => {
                // Never opens (the drain notices and answers); reaching here
                // means a bug, and a stuck modal is worse than a dropped notice.
            }
        }
    }

    /// A host `Select`: a filterable list on the pickers' chrome.
    ///
    /// The filter row only exists once something has been typed, which is why
    /// the ownership predicate answers `DialogSelect` only in that state: while
    /// the query is empty there is no field to anchor to, and a marker on a row
    /// nobody painted would park the IME candidate window on unrelated text.
    pub(super) fn compose_select_dialog(
        &self,
        viewport: &mut [String],
        width: u16,
        height: u16,
        modal: &crate::state::DialogModal,
    ) {
        use lca_protocol::UiDialog;
        let UiDialog::Select { title, options, .. } = &modal.exchange.dialog else {
            return;
        };
        let rows = 10usize.min(height.saturating_sub(7) as usize).max(1);
        let total = modal.matches.len();
        let start = modal
            .selected
            .saturating_sub(rows - 1)
            .min(total.saturating_sub(rows));
        let end = (start + rows).min(total);
        let mut body = Vec::new();
        if !modal.query.is_empty() {
            // Gh #371 bug 3: the filter is the field being typed into while it
            // holds the keyboard.
            body.push(self.caret_row("> ", &modal.query, "", Field::DialogSelect));
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
        let selected =
            (!modal.matches.is_empty()).then(|| base + modal.selected.saturating_sub(start));
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

    /// A host `Input`: one line, plus the caret at its insertion point.
    ///
    /// This was the surface where the composer's marker reached the frame with
    /// no competitor at all, so a CJK user typing an extension's prompt got the
    /// candidate window at the bottom of the screen. An empty field shows the
    /// extension's own placeholder under the caret (pi's `Input`: "a focused
    /// empty field looks alive"); with none declared, the host's wording stands
    /// in, so the row never collapses to a bare caret with nothing naming it.
    pub(super) fn compose_input_dialog(
        &self,
        viewport: &mut [String],
        width: u16,
        height: u16,
        modal: &crate::state::DialogModal,
    ) {
        use lca_protocol::UiDialog;
        let UiDialog::Input { label, placeholder } = &modal.exchange.dialog else {
            return;
        };
        let declared = placeholder.clone().unwrap_or_default();
        let placeholder = if declared.is_empty() {
            "(type a value)"
        } else {
            declared.as_str()
        };
        let body = vec![
            self.caret_row("  ", &modal.input, placeholder, Field::DialogInput),
            String::new(),
            "Enter submits; Esc cancels.".to_string(),
        ];
        super::render::overlay_box(viewport, width, height, label, &body, &self.theme);
    }
}
