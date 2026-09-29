//! The theme system, ported from pi's
//! `coding-agent/src/modes/interactive/theme/`
//! (`pi-tui-re/src_re/agent-components/theme.md`).
//!
//! **S5: the ~50-role vocabulary, behind the same accessors.** The theme
//! used to be three hard-coded palettes; it is now pi's `ThemeColor`/
//! `ThemeBg` token table ([`Role`], [`Palette`]), with the style functions
//! built from it. Call sites keep using `(theme.dim)(s)` /
//! `theme.markdown()`; only construction changed. A custom theme file
//! (TOML, pi's `theme-json.ts` shape) overlays individual roles; an invalid
//! file keeps the last-good palette and reports why.

mod palette;

pub use palette::Palette;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use lca_tui::widgets::markdown::MarkdownTheme;

/// A styled-text function: wrap text in SGR codes and reset.
pub type StyleFn = Arc<dyn Fn(&str) -> String + Send + Sync>;

macro_rules! roles {
    ($($variant:ident => $key:literal),* $(,)?) => {
        /// One theme role (pi's `ThemeColor`/`ThemeBg` vocabulary).
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
        pub enum Role {
            $(#[doc = $key] $variant),*
        }

        impl Role {
            /// Every role, in declaration order.
            pub const ALL: &'static [Role] = &[$(Role::$variant),*];

            /// The role's config-file key.
            pub fn key(self) -> &'static str {
                match self { $(Role::$variant => $key),* }
            }

            /// Parse a role from its config-file key.
            pub fn parse(key: &str) -> Option<Role> {
                match key { $($key => Some(Role::$variant),)* _ => None }
            }
        }
    };
}

roles! {
    Accent => "accent",
    Border => "border",
    BorderAccent => "borderAccent",
    BorderMuted => "borderMuted",
    Success => "success",
    Error => "error",
    Warning => "warning",
    Muted => "muted",
    Dim => "dim",
    Text => "text",
    ThinkingText => "thinkingText",
    ScrollbarTrack => "scrollbarTrack",
    ScrollbarThumb => "scrollbarThumb",
    SearchMatchText => "searchMatchText",
    UserMessageText => "userMessageText",
    CustomMessageText => "customMessageText",
    CustomMessageLabel => "customMessageLabel",
    ToolTitle => "toolTitle",
    ToolOutput => "toolOutput",
    MdHeading => "mdHeading",
    MdLink => "mdLink",
    MdLinkUrl => "mdLinkUrl",
    MdCode => "mdCode",
    MdCodeBlock => "mdCodeBlock",
    MdCodeBlockBorder => "mdCodeBlockBorder",
    MdQuote => "mdQuote",
    MdQuoteBorder => "mdQuoteBorder",
    MdHr => "mdHr",
    MdListBullet => "mdListBullet",
    ToolDiffAdded => "toolDiffAdded",
    ToolDiffRemoved => "toolDiffRemoved",
    ToolDiffContext => "toolDiffContext",
    SyntaxComment => "syntaxComment",
    SyntaxKeyword => "syntaxKeyword",
    SyntaxFunction => "syntaxFunction",
    SyntaxVariable => "syntaxVariable",
    SyntaxString => "syntaxString",
    SyntaxNumber => "syntaxNumber",
    SyntaxType => "syntaxType",
    SyntaxOperator => "syntaxOperator",
    SyntaxPunctuation => "syntaxPunctuation",
    ThinkingOff => "thinkingOff",
    ThinkingMinimal => "thinkingMinimal",
    ThinkingLow => "thinkingLow",
    ThinkingMedium => "thinkingMedium",
    ThinkingHigh => "thinkingHigh",
    ThinkingXhigh => "thinkingXhigh",
    ThinkingMax => "thinkingMax",
    BashMode => "bashMode",
    SelectedBg => "selectedBg",
    SearchMatchBg => "searchMatchBg",
    UserMessageBg => "userMessageBg",
    CustomMessageBg => "customMessageBg",
    ToolPendingBg => "toolPendingBg",
    ToolSuccessBg => "toolSuccessBg",
    ToolErrorBg => "toolErrorBg",
}

/// One color: the terminal default, a 24-bit color, or a 256-color index.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Color {
    /// The terminal's default foreground (`""` in pi).
    #[default]
    Default,
    /// A 24-bit color.
    Rgb(u8, u8, u8),
    /// A 256-color palette index.
    Ansi256(u8),
}

impl Color {
    /// The SGR parameters that select this color, or `None` for the
    /// terminal default.
    fn sgr(self) -> Option<String> {
        match self {
            Color::Default => None,
            Color::Rgb(r, g, b) => Some(format!("38;2;{r};{g};{b}")),
            Color::Ansi256(index) => Some(format!("38;5;{index}")),
        }
    }
}

/// Build a style function from a color and an SGR decoration prefix.
fn style(color: Color, decoration: &str, colored: bool) -> StyleFn {
    let decoration = decoration.to_string();
    Arc::new(move |text: &str| {
        if !colored {
            return text.to_string();
        }
        let mut codes = Vec::new();
        if !decoration.is_empty() {
            codes.push(decoration.clone());
        }
        if let Some(color) = color.sgr() {
            codes.push(color);
        }
        if codes.is_empty() {
            text.to_string()
        } else {
            format!("\x1b[{}m{text}\x1b[0m", codes.join(";"))
        }
    })
}

fn identity() -> StyleFn {
    Arc::new(|text: &str| text.to_string())
}

/// The UI's style roles.
#[derive(Clone)]
pub struct Theme {
    /// Whether color is enabled.
    pub colored: bool,
    /// The resolved role table (S5).
    pub palette: Palette,
    /// The theme's name (a built-in, or the file's stem).
    pub name: String,
    /// Dim text (reasoning, status).
    pub dim: StyleFn,
    /// Bold text.
    pub bold: StyleFn,
    /// The user prompt band.
    pub user: StyleFn,
    /// The assistant answer.
    pub assistant: StyleFn,
    /// Reasoning text.
    pub reasoning: StyleFn,
    /// Tool card text.
    pub tool: StyleFn,
    /// Success status.
    pub success: StyleFn,
    /// Error status.
    pub error: StyleFn,
    /// Warning status.
    pub warn: StyleFn,
    /// The footer's normal text.
    pub footer: StyleFn,
    /// The footer's accent (branch, model).
    pub accent: StyleFn,
    /// Every role's style, precomputed for `role()`.
    roles: BTreeMap<Role, StyleFn>,
}

impl Theme {
    /// Build a theme from a palette: the accessors are derived from the
    /// role table, so a custom overlay moves them too.
    pub fn from_palette(name: &str, palette: Palette, colored: bool) -> Theme {
        let roles: BTreeMap<Role, StyleFn> = Role::ALL
            .iter()
            .map(|role| (*role, style(palette.get(*role), "", colored)))
            .collect();
        let dim = style(palette.get(Role::Dim), "2", colored);
        let bold = style(palette.get(Role::Text), "1", colored);
        let user = style(palette.get(Role::UserMessageText), "1", colored);
        let reasoning = style(palette.get(Role::ThinkingText), "3", colored);
        let tool = style(palette.get(Role::ToolTitle), "", colored);
        let success = style(palette.get(Role::Success), "", colored);
        let error = style(palette.get(Role::Error), "", colored);
        let warn = style(palette.get(Role::Warning), "", colored);
        let footer = style(palette.get(Role::Muted), "", colored);
        let accent = style(palette.get(Role::Accent), "", colored);
        Theme {
            colored,
            palette,
            name: name.to_string(),
            dim: if colored { dim } else { identity() },
            bold: if colored { bold } else { identity() },
            user: if colored { user } else { identity() },
            assistant: identity(),
            reasoning: if colored { reasoning } else { identity() },
            tool,
            success,
            error,
            warn,
            footer,
            accent,
            roles,
        }
    }

    /// The colored theme (the dark-terminal palette).
    pub fn colored() -> Self {
        Self::from_palette("dark", Palette::dark(), true)
    }

    /// A light-terminal palette.
    pub fn light() -> Self {
        Self::from_palette("light", Palette::light(), true)
    }

    /// The plain theme (FR-UI-5).
    pub fn plain() -> Self {
        let mut plain = Self::from_palette("plain", Palette::dark(), false);
        plain.dim = identity();
        plain.bold = identity();
        plain.user = identity();
        plain.reasoning = identity();
        plain.tool = identity();
        plain.success = identity();
        plain.error = identity();
        plain.warn = identity();
        plain.footer = identity();
        plain.accent = identity();
        plain.roles = Role::ALL.iter().map(|role| (*role, identity())).collect();
        plain
    }

    /// A built-in theme by name, or `None` when the name is unknown.
    pub fn named(name: &str) -> Option<Theme> {
        match name {
            "default" | "dark" => Some(Self::colored()),
            "light" => Some(Self::light()),
            "plain" => Some(Self::plain()),
            _ => None,
        }
    }

    /// The scheme-aware default (FR-UI-17): the light palette when the
    /// terminal reports a light background, the dark one otherwise.
    pub fn auto() -> Self {
        Self::for_scheme(detect_scheme())
    }

    /// The palette for a detected scheme (`None`: assume dark).
    pub fn for_scheme(scheme: Option<lca_tui::engine::colors::ColorScheme>) -> Self {
        match scheme {
            Some(lca_tui::engine::colors::ColorScheme::Light) => Self::light(),
            _ => Self::colored(),
        }
    }

    /// The underline decoration (pi's `chalk.underline`).
    pub fn underline(&self) -> StyleFn {
        if self.colored {
            style(self.palette.get(Role::Text), "4", true)
        } else {
            identity()
        }
    }

    /// The style function for any role in the vocabulary.
    pub fn role(&self, role: Role) -> StyleFn {
        self.roles
            .get(&role)
            .cloned()
            .unwrap_or_else(|| style(self.palette.get(role), "", self.colored))
    }

    /// The markdown theme derived from these roles.
    pub fn markdown(&self) -> MarkdownTheme {
        MarkdownTheme {
            heading: if self.colored {
                style(self.palette.get(Role::MdHeading), "1", true)
            } else {
                identity()
            },
            bold: self.bold.clone(),
            underline: self.underline(),
            italic: self.reasoning.clone(),
            strike: self.dim.clone(),
            code: self.role(Role::MdCode),
            code_block: self.role(Role::MdCodeBlock),
            code_block_border: self.role(Role::MdCodeBlockBorder),
            link: self.role(Role::MdLink),
            quote: self.role(Role::MdQuote),
            hr: self.role(Role::MdHr),
        }
    }
}

/// Resolve the `ui.theme` setting (S5): `"plain"`, `"auto"`, a built-in
/// name, or a custom theme file's name. A custom file overlays the detected
/// scheme's base palette; `name.light` / `name.dark` siblings let one name
/// carry both sides. An unreadable or invalid file keeps the base palette.
pub fn load(
    setting: &str,
    scheme: Option<lca_tui::engine::colors::ColorScheme>,
    dir: &Path,
) -> (Theme, Option<String>) {
    // A built-in name resolves directly; only an unknown name is a custom
    // theme file to look up. F2: these arms used to fall through to the
    // file lookup, so `ui.theme = "dark"` printed a spurious "not found".
    let (base, default_name) = match setting {
        "plain" => return (Theme::plain(), None),
        "auto" | "" => return (Theme::for_scheme(scheme), None),
        "default" | "dark" => return (Theme::colored(), None),
        "light" => return (Theme::light(), None),
        _ => {
            let scheme_theme = Theme::for_scheme(scheme);
            let default_name = scheme_theme.name.clone();
            (scheme_theme.with_name(setting), default_name)
        }
    };
    // A custom theme: `<dir>/<name>.<side>.toml` first, then `<name>.toml`.
    let side = match scheme {
        Some(lca_tui::engine::colors::ColorScheme::Light) => "light",
        _ => "dark",
    };
    let candidates = [
        dir.join(format!("{setting}.{side}.toml")),
        dir.join(format!("{setting}.toml")),
    ];
    for path in candidates {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        return match base.palette.overlay(&text) {
            Ok((palette, applied)) => {
                let theme = Theme::from_palette(setting, palette, base.colored);
                let notice = format!("theme `{setting}`: {applied} role(s) applied");
                (theme, Some(notice))
            }
            // Invalid file: keep the last-good base and say why.
            Err(reason) => (
                base,
                Some(format!(
                    "theme `{setting}` ignored ({reason}); keeping the previous palette"
                )),
            ),
        };
    }
    // A custom name with no file: say so rather than silently showing the
    // scheme default under the user's chosen name.
    (
        base,
        Some(format!(
            "theme `{setting}` not found in {}; using the {default_name} default",
            dir.display(),
        )),
    )
}

impl Theme {
    /// A copy carrying a different name.
    fn with_name(mut self, name: &str) -> Theme {
        self.name = name.to_string();
        self
    }
}

/// The built-in theme names (`/theme`'s picker).
pub const THEMES: &[&str] = &["dark", "light", "plain"];

/// The built-in theme names plus every custom `<name>.toml` in `dir`,
/// sorted (S5: a custom theme joins the picker).
pub fn theme_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = THEMES.iter().map(|name| name.to_string()).collect();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            // `<name>.toml` and `<name>.light.toml` / `<name>.dark.toml`
            // both register the bare `<name>`.
            if let Some(stem) = name.strip_suffix(".toml") {
                let stem = stem
                    .strip_suffix(".light")
                    .or_else(|| stem.strip_suffix(".dark"))
                    .unwrap_or(stem);
                if !stem.is_empty() && !names.iter().any(|existing| existing == stem) {
                    names.push(stem.to_string());
                }
            }
        }
    }
    names
}

/// The directory custom theme files live in.
pub fn themes_dir(config_dir: &Path) -> std::path::PathBuf {
    config_dir.join("themes")
}

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
        assert_eq!((t.bold)("x"), "\x1b[1;38;2;212;212;212mx\x1b[0m");
        assert!(t.colored);
    }

    #[test]
    fn plain_is_identity() {
        let t = Theme::plain();
        assert_eq!((t.bold)("x"), "x");
        assert_eq!((t.user)("hi"), "hi");
        assert!(!t.colored);
    }

    #[test]
    fn every_role_resolves_in_both_builtin_palettes() {
        assert!(Palette::dark().is_complete(), "dark palette is complete");
        assert!(Palette::light().is_complete(), "light palette is complete");
        assert_eq!(Role::ALL.len(), 56, "the ~50-token vocabulary");
    }

    #[test]
    fn a_custom_overlay_moves_only_the_roles_it_names() {
        let base = Palette::dark();
        let (overlaid, applied) = base
            .overlay("accent = \"#123456\"\nmdCode = \"#abcdef\"\n")
            .expect("valid overlay");
        assert_eq!(applied, 2);
        assert_eq!(overlaid.get(Role::Accent), Color::Rgb(0x12, 0x34, 0x56));
        assert_eq!(overlaid.get(Role::MdCode), Color::Rgb(0xab, 0xcd, 0xef));
        // Untouched roles keep the base value.
        assert_eq!(overlaid.get(Role::Error), base.get(Role::Error));
    }

    #[test]
    fn a_custom_overlay_resolves_named_vars_and_256_indices() {
        let base = Palette::dark();
        let (overlaid, _) = base
            .overlay(
                "vars = { teal = \"#00ffcc\" }\n[colors]\naccent = \"teal\"\nborder = \"42\"\n",
            )
            .expect("valid overlay");
        assert_eq!(overlaid.get(Role::Accent), Color::Rgb(0, 0xff, 0xcc));
        assert_eq!(overlaid.get(Role::Border), Color::Ansi256(42));
    }

    #[test]
    fn an_invalid_overlay_is_refused() {
        let base = Palette::dark();
        assert!(base.overlay("accent = \"not-a-color\"").is_err());
        assert!(base.overlay("this is not toml = = =").is_err());
    }

    #[test]
    fn a_missing_role_falls_back_to_text() {
        let palette = Palette::from(&[(Role::Text, "#ffffff")]);
        assert_eq!(palette.get(Role::Accent), Color::Rgb(0xff, 0xff, 0xff));
    }

    #[test]
    fn plain_theme_keeps_the_role_table_but_paints_nothing() {
        let t = Theme::plain();
        assert_eq!((t.role(Role::MdHeading))("h"), "h");
        assert_eq!((t.role(Role::SyntaxKeyword))("fn"), "fn");
    }

    // Verifies: F2 - a built-in theme name resolves without the custom-file
    // "not found" notice, and an unknown name still says so.
    #[test]
    fn built_in_theme_names_resolve_without_a_notice() {
        let dir = std::path::Path::new("/nonexistent-theme-dir");
        assert!(load("dark", None, dir).1.is_none());
        assert!(load("light", None, dir).1.is_none());
        assert!(load("plain", None, dir).1.is_none());
        assert!(load("auto", None, dir).1.is_none());
        let notice = load("nope", None, dir).1.expect("unknown name reports");
        assert!(notice.contains("not found"), "{notice}");
    }

    #[test]
    fn role_keys_round_trip() {
        for role in Role::ALL {
            assert_eq!(Role::parse(role.key()), Some(*role));
        }
    }
}
