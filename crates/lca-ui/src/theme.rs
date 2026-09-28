//! The theme system, ported from pi's
//! `coding-agent/src/modes/interactive/theme/`
//! (`pi-tui-re/src_re/agent-components/theme.md`).
//!
//! A small role vocabulary of style functions over the engine's markdown
//! theme. Colors are plain SGR codes; `plain` mode is the identity so a
//! terminal without color renders text only (FR-UI-5).

use std::sync::Arc;

use lca_tui::widgets::markdown::MarkdownTheme;

/// Wrap `text` in an SGR code and reset.
fn sgr(code: &str, text: &str) -> String {
    format!("\x1b[{code}m{text}\x1b[0m")
}

fn style(code: &'static str) -> Arc<dyn Fn(&str) -> String + Send + Sync> {
    Arc::new(move |t: &str| sgr(code, t))
}

fn identity() -> Arc<dyn Fn(&str) -> String + Send + Sync> {
    Arc::new(|t: &str| t.to_string())
}

/// The UI's style roles.
#[derive(Clone)]
pub struct Theme {
    /// Whether color is enabled.
    pub colored: bool,
    /// Dim text (reasoning, status).
    pub dim: Arc<dyn Fn(&str) -> String + Send + Sync>,
    /// Bold text.
    pub bold: Arc<dyn Fn(&str) -> String + Send + Sync>,
    /// The user prompt band.
    pub user: Arc<dyn Fn(&str) -> String + Send + Sync>,
    /// The assistant answer.
    pub assistant: Arc<dyn Fn(&str) -> String + Send + Sync>,
    /// Reasoning text.
    pub reasoning: Arc<dyn Fn(&str) -> String + Send + Sync>,
    /// Tool card text.
    pub tool: Arc<dyn Fn(&str) -> String + Send + Sync>,
    /// Success status.
    pub success: Arc<dyn Fn(&str) -> String + Send + Sync>,
    /// Error status.
    pub error: Arc<dyn Fn(&str) -> String + Send + Sync>,
    /// Warning status.
    pub warn: Arc<dyn Fn(&str) -> String + Send + Sync>,
    /// The footer's normal text.
    pub footer: Arc<dyn Fn(&str) -> String + Send + Sync>,
    /// The footer's accent (branch, model).
    pub accent: Arc<dyn Fn(&str) -> String + Send + Sync>,
}

impl Theme {
    /// The colored theme (dark-terminal palette).
    pub fn colored() -> Self {
        Self {
            colored: true,
            dim: style("2"),
            bold: style("1"),
            user: style("1;36"),
            assistant: identity(),
            reasoning: style("2;3"),
            tool: style("35"),
            success: style("32"),
            error: style("31"),
            warn: style("33"),
            footer: style("2"),
            accent: style("36"),
        }
    }

    /// The plain theme (FR-UI-5).
    pub fn plain() -> Self {
        Self {
            colored: false,
            dim: identity(),
            bold: identity(),
            user: identity(),
            assistant: identity(),
            reasoning: identity(),
            tool: identity(),
            success: identity(),
            error: identity(),
            warn: identity(),
            footer: identity(),
            accent: identity(),
        }
    }

    /// The markdown theme derived from these roles.
    pub fn markdown(&self) -> MarkdownTheme {
        MarkdownTheme {
            heading: self.bold.clone(),
            bold: self.bold.clone(),
            italic: self.reasoning.clone(),
            strike: self.dim.clone(),
            code: self.accent.clone(),
            code_block: self.dim.clone(),
            code_block_border: self.dim.clone(),
            link: self.accent.clone(),
            quote: self.reasoning.clone(),
            hr: self.dim.clone(),
        }
    }

    /// A light-terminal palette (darker foregrounds for a light background).
    pub fn light() -> Self {
        Self {
            colored: true,
            dim: style("2"),
            bold: style("1"),
            user: style("1;34"),
            assistant: identity(),
            reasoning: style("2;3"),
            tool: style("35"),
            success: style("32"),
            error: style("31"),
            warn: style("33"),
            footer: style("2"),
            accent: style("34"),
        }
    }

    /// A named theme, or `None` when the name is unknown.
    pub fn named(name: &str) -> Option<Theme> {
        match name {
            "default" => Some(Self::colored()),
            "light" => Some(Self::light()),
            "plain" => Some(Self::plain()),
            _ => None,
        }
    }

    /// The scheme-aware default (FR-UI-17): the light palette when the
    /// terminal reports a light background, the dark one otherwise.
    pub fn auto() -> Self {
        match detect_scheme() {
            Some(lca_tui::engine::colors::ColorScheme::Light) => Self::light(),
            _ => Self::colored(),
        }
    }
}

/// The available theme names (FR-UI-17's picker).
pub const THEMES: &[&str] = &["default", "light", "plain"];

/// Detect the terminal's color scheme from `COLORFGBG` (the OSC 11 / DEC
/// report ladder is the engine's, consumed at negotiation).
pub fn detect_scheme() -> Option<lca_tui::engine::colors::ColorScheme> {
    use lca_tui::engine::colors::ColorScheme;
    let value = std::env::var("COLORFGBG").ok()?;
    let background: u32 = value.rsplit(';').next()?.trim().parse().ok()?;
    Some(if background >= 8 {
        ColorScheme::Light
    } else {
        ColorScheme::Dark
    })
}

impl Default for Theme {
    fn default() -> Self {
        Self::colored()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colored_styles_wrap_and_reset() {
        let t = Theme::colored();
        assert_eq!((t.bold)("x"), "\x1b[1mx\x1b[0m");
        assert_eq!((t.user)("hi"), "\x1b[1;36mhi\x1b[0m");
    }

    #[test]
    fn plain_is_identity() {
        let t = Theme::plain();
        assert_eq!((t.bold)("x"), "x");
        assert_eq!((t.user)("hi"), "hi");
        assert!(!t.colored);
    }
}
