//! The chat composition, ported from pi's
//! `coding-agent/src/modes/interactive/chat-viewport.ts`
//! (`pi-tui-re/src_re/agent-components/utilities.md` §1).
//!
//! The product's frame: the transcript above, the editor dock and footer
//! below. In main-screen mode the whole document is returned each frame and
//! the renderer diffs it, so the transcript grows into scrollback while the
//! dock repaints in place.

use lca_tui::widgets::editor::Editor;

use crate::footer::Footer;
use crate::theme::Theme;
use crate::transcript::Transcript;

/// The interactive chat document.
pub struct Chat {
    /// The transcript.
    pub transcript: Transcript,
    /// The prompt editor.
    pub editor: Editor,
    /// The footer.
    pub footer: Footer,
    /// The theme.
    pub theme: Theme,
    /// A transient notice above the editor.
    pub notice: Option<String>,
    /// Whether a turn is running.
    pub turn_running: bool,
}

impl Chat {
    /// A new chat.
    pub fn new(editor: Editor, theme: Theme) -> Self {
        Self {
            transcript: Transcript::new(),
            editor,
            footer: Footer::default(),
            theme,
            notice: None,
            turn_running: false,
        }
    }

    /// Render the whole document at a width.
    pub fn render(&self, width: u16) -> Vec<String> {
        let mut out = self.transcript.render(width, &self.theme);
        // A separator above the dock.
        out.push(String::new());
        out.push((self.theme.dim)(
            &"─".repeat(width.saturating_sub(0) as usize),
        ));

        if let Some(notice) = &self.notice {
            out.push((self.theme.warn)(notice));
        }

        // The autocomplete popup, when open.
        let popup = self.editor.render_popup(width);
        if !popup.is_empty() {
            out.extend(popup);
        }

        // The editor.
        out.extend(self.editor.render(width));

        // The footer.
        out.extend(self.footer.render(width, &self.theme));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lca_tui::engine::text::strip_terminal_sequences;

    #[test]
    fn chat_renders_transcript_dock_and_footer() {
        let mut editor = Editor::new();
        editor.insert_str("draft");
        let mut chat = Chat::new(editor, Theme::plain());
        chat.transcript.push_user("question");
        chat.transcript.append_text("answer");
        chat.transcript.finish_assistant();
        chat.footer.cwd = "/tmp/x".into();
        chat.footer.model = "p/m".into();
        let lines: Vec<String> = chat
            .render(60)
            .iter()
            .map(|l| strip_terminal_sequences(l))
            .collect();
        assert!(lines.iter().any(|l| l == "› question"));
        assert!(lines.iter().any(|l| l.contains("answer")));
        assert!(lines.iter().any(|l| l.contains("draft")));
        assert!(lines.iter().any(|l| l.contains("/tmp/x")));
    }
}
