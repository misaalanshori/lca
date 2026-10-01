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

mod highlight;
mod palette;

pub use highlight::{SyntaxStyles, highlight};
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

/// The SGR parameters that select this color on the given channel, or
/// `None` for the terminal default. `channel` is `38` (foreground) or `48`
/// (background) - pi's `getFgAnsi`/`getBgAnsi` pair.
fn color_sgr(color: Color, channel: u8) -> Option<String> {
    match color {
        Color::Default => None,
        Color::Rgb(r, g, b) => Some(format!("{channel};2;{r};{g};{b}")),
        Color::Ansi256(index) => Some(format!("{channel};5;{index}")),
    }
}

/// The SGR code that turns one decoration back off. A style closes only
/// what it opened, so a nested style cannot kill the one around it
/// (`theme.md` §2's channel-scoped reset, extended to decorations).
fn decoration_reset(decoration: &str) -> &'static str {
    match decoration {
        "1" | "2" => "22", // bold, dim share the reset code
        "3" => "23",       // italic
        "4" => "24",       // underline
        "9" => "27",       // strikethrough
        _ => "22",
    }
}

/// Build a foreground style function: pi's `fg()` - opens the color (and
/// any decoration) and resets **only those channels** (`ESC[39m`), so
/// layered styles compose and a nested reset cannot blank the outer color.
fn style(color: Color, decoration: &str, colored: bool) -> StyleFn {
    let decoration = decoration.to_string();
    Arc::new(move |text: &str| {
        if !colored {
            return text.to_string();
        }
        let mut open = Vec::new();
        let mut close = Vec::new();
        if !decoration.is_empty() {
            open.push(decoration.clone());
            close.push(decoration_reset(&decoration).to_string());
        }
        if let Some(color) = color_sgr(color, 38) {
            open.push(color);
            close.push("39".to_string());
        }
        if open.is_empty() {
            text.to_string()
        } else {
            format!("\x1b[{}m{text}\x1b[{}m", open.join(";"), close.join(";"))
        }
    })
}

/// Build a background style function: pi's `bg()` - same discipline on the
/// background channel (`ESC[49m`).
fn bg_style(color: Color, colored: bool) -> StyleFn {
    Arc::new(move |text: &str| {
        if !colored {
            return text.to_string();
        }
        match color_sgr(color, 48) {
            Some(open) => format!("\x1b[{open}m{text}\x1b[49m"),
            None => text.to_string(),
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
    /// Every role's background style, precomputed for `bg()`.
    bgs: BTreeMap<Role, StyleFn>,
}

impl Theme {
    /// Build a theme from a palette: the accessors are derived from the
    /// role table, so a custom overlay moves them too.
    pub fn from_palette(name: &str, palette: Palette, colored: bool) -> Theme {
        let roles: BTreeMap<Role, StyleFn> = Role::ALL
            .iter()
            .map(|role| (*role, style(palette.get(*role), "", colored)))
            .collect();
        let bgs: BTreeMap<Role, StyleFn> = Role::ALL
            .iter()
            .map(|role| (*role, bg_style(palette.get(*role), colored)))
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
            bgs,
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
        plain.bgs = Role::ALL.iter().map(|role| (*role, identity())).collect();
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

    /// A role's color with pi's bold on top: every tool-card title is
    /// `theme.fg("toolTitle", theme.bold(name))` in pi's renderers, and a
    /// colorless bold is what lets the role's color survive (`theme.md`
    /// §3's nesting rule).
    pub fn role_bold(&self, role: Role) -> StyleFn {
        style(self.palette.get(role), "1", self.colored)
    }

    /// The background style function for any role in the vocabulary (pi's
    /// `theme.bg(...)`): wraps a run of text - or a row padded to the
    /// screen width - in the role's background and resets only that
    /// channel.
    pub fn bg(&self, role: Role) -> StyleFn {
        self.bgs
            .get(&role)
            .cloned()
            .unwrap_or_else(|| bg_style(self.palette.get(role), self.colored))
    }

    /// The markdown theme derived from these roles.
    pub fn markdown(&self) -> MarkdownTheme {
        // pi's `getCliHighlightTheme`: the nine `syntax*` roles resolved
        // once per call. An operator/punctuation role that paints exactly
        // like `text` (pi's own defaults do) stays `None`, so a block of
        // code carries no no-op escapes.
        let syntax = SyntaxStyles {
            comment: self.role(Role::SyntaxComment),
            keyword: self.role(Role::SyntaxKeyword),
            function: self.role(Role::SyntaxFunction),
            variable: self.role(Role::SyntaxVariable),
            string: self.role(Role::SyntaxString),
            number: self.role(Role::SyntaxNumber),
            type_: self.role(Role::SyntaxType),
            operator: (self.palette.get(Role::SyntaxOperator) != self.palette.get(Role::Text))
                .then(|| self.role(Role::SyntaxOperator)),
            punctuation: (self.palette.get(Role::SyntaxPunctuation)
                != self.palette.get(Role::Text))
            .then(|| self.role(Role::SyntaxPunctuation)),
        };
        // `(code, lang)` -> styled lines, or `None` for a language this
        // port does not know; the markdown renderer then paints every line
        // `mdCodeBlock`, exactly pi's unknown-language path.
        let hook: lca_tui::widgets::markdown::HighlightFn =
            Arc::new(move |code, lang| highlight(code, lang, &syntax));
        MarkdownTheme {
            // pi's heading is a color and nothing else - the renderer adds
            // the bold (markdown.md §4), and h3+ gets no bold at all.
            heading: if self.colored {
                style(self.palette.get(Role::MdHeading), "", true)
            } else {
                identity()
            },
            // pi's markdown decorations are chalk: bold, italic, underline
            // and strikethrough are decoration-only, so a colored style
            // around them survives (markdown.md §3 - a nested reset must
            // never carry a color, or `heading(bold(x))` renders in the
            // body color instead of `mdHeading`).
            bold: self.decoration("1"),
            underline: self.decoration("4"),
            italic: self.decoration("3"),
            strike: self.decoration("9"),
            code: self.role(Role::MdCode),
            code_block: self.role(Role::MdCodeBlock),
            code_block_border: self.role(Role::MdCodeBlockBorder),
            link: self.role(Role::MdLink),
            link_url: self.role(Role::MdLinkUrl),
            list_bullet: self.role(Role::MdListBullet),
            quote: self.role(Role::MdQuote),
            quote_border: self.role(Role::MdQuoteBorder),
            hr: self.role(Role::MdHr),
            // pi's mermaid roles (chrome.md §mermaid): the title is
            // accent-over-bold, the rest are the roles pi's theme maps.
            mermaid_border: self.role(Role::BorderMuted),
            mermaid_text: self.role(Role::Text),
            mermaid_edge: self.role(Role::Accent),
            mermaid_edge_label: self.role(Role::Muted),
            mermaid_title: {
                let accent = self.role(Role::Accent);
                let bold = self.decoration("1");
                Arc::new(move |s: &str| accent(&bold(s))) as StyleFn
            },
            warning: self.role(Role::Warning),
            highlight: Some(hook),
        }
    }

    /// A decoration-only style: SGR code on, its own close code off, no
    /// color touched.
    fn decoration(&self, code: &'static str) -> StyleFn {
        if self.colored {
            style(Color::Default, code, true)
        } else {
            identity()
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
        // `theme.md` §2: a style closes only the channels it opened - the
        // decoration and the color - so a style around it survives.
        assert_eq!((t.bold)("x"), "\x1b[1;38;2;212;212;212mx\x1b[22;39m");
        assert!(t.colored);
    }

    // Verifies: R1 - `fg()`/`bg()` reset only their own channel, so a
    // colored run inside another colored run composes instead of blanking
    // the outer style.
    #[test]
    fn fg_and_bg_reset_only_their_own_channel() {
        let t = Theme::colored();
        assert!(
            (t.role(Role::Accent))("x").ends_with("\x1b[39m"),
            "foreground closes with 39"
        );
        let bg = (t.bg(Role::UserMessageBg))("x");
        assert!(
            bg.starts_with("\x1b[48;2;"),
            "background opens on channel 48: {bg:?}"
        );
        assert!(
            bg.ends_with("\x1b[49m"),
            "background closes with 49: {bg:?}"
        );
        // The band's content color inside the band's background: the
        // inner close must not be `0m`, or the band would go transparent
        // for the rest of the row.
        let inner = (t.role(Role::UserMessageText))("hello");
        let row = (t.bg(Role::UserMessageBg))(&inner);
        assert!(
            !row.contains("\x1b[0m") && row.ends_with("\x1b[49m"),
            "no full reset inside a band: {row:?}"
        );
        assert!(
            row.contains("\x1b[39m"),
            "the inner color still closes: {row:?}"
        );
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

    // Verifies: R3 - the operator and punctuation roles are painted only
    // when the palette paints them differently from `text` (pi's own
    // defaults are the text color), so a default block carries no no-op
    // escapes while a custom theme that colors them does.
    #[test]
    fn operator_roles_paint_only_when_the_palette_colors_them() {
        let code = "a = 1;";
        let run = |theme: &Theme| {
            let hook = theme.markdown().highlight.expect("a highlight hook");
            hook(code, "rust").expect("rust is known")
        };

        let default = run(&Theme::colored());
        assert!(
            !default.iter().any(|l| l.contains("38;2;212;212;212")),
            "the default palette paints operators exactly like text, so it skips them: {default:?}"
        );
        assert!(
            default.iter().any(|l| l.contains("38;2;156;220;254")),
            "the variable is still a variable: {default:?}"
        );

        let (palette, _) = Palette::dark()
            .overlay("syntaxOperator = \"#ff0000\"\nsyntaxPunctuation = \"#00ff00\"\n")
            .expect("a valid overlay");
        let custom = run(&Theme::from_palette("custom", palette, true));
        assert!(
            custom.iter().any(|l| l.contains("38;2;255;0;0")),
            "a palette that colors operators gets them: {custom:?}"
        );
        assert!(
            custom.iter().any(|l| l.contains("38;2;0;255;0")),
            "and punctuation too: {custom:?}"
        );
    }

    #[test]
    fn role_keys_round_trip() {
        for role in Role::ALL {
            assert_eq!(Role::parse(role.key()), Some(*role));
        }
    }
}
