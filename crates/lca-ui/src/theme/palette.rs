//! The theme palette (S5): pi's `ThemeColor`/`ThemeBg` token vocabulary
//! (`pi-tui-re/src_re/agent-components/theme.md` section 1) as a role →
//! color table, resolved once per theme and overlaid by a custom theme
//! file.

use std::collections::BTreeMap;

use super::{Color, Role};

/// A resolved role table: every theme role maps to a color.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Palette {
    colors: BTreeMap<Role, Color>,
}

impl Palette {
    /// The pi `dark.json` palette.
    pub fn dark() -> Palette {
        let mut palette = Palette::from(&[
            (Role::Accent, "#8abeb7"),
            (Role::Border, "#5f87ff"),
            (Role::BorderAccent, "#00d7ff"),
            (Role::BorderMuted, "#505050"),
            (Role::Success, "#b5bd68"),
            (Role::Error, "#cc6666"),
            (Role::Warning, "#ffff00"),
            (Role::Muted, "#808080"),
            (Role::Dim, "#666666"),
            (Role::Text, "#d4d4d4"),
            (Role::ThinkingText, "#808080"),
            (Role::SelectedBg, "#3a3a4a"),
            (Role::ScrollbarTrack, "#505050"),
            (Role::ScrollbarThumb, "#d4d4d4"),
            (Role::SearchMatchBg, "#3a3a4a"),
            (Role::SearchMatchText, "#d4d4d4"),
            (Role::UserMessageBg, "#343541"),
            (Role::UserMessageText, "#d4d4d4"),
            (Role::CustomMessageBg, "#2d2838"),
            (Role::SidePanel, "#33333f"),
            (Role::CustomMessageText, "#d4d4d4"),
            (Role::CustomMessageLabel, "#9575cd"),
            (Role::ToolPendingBg, "#282832"),
            (Role::ToolSuccessBg, "#283228"),
            (Role::ToolErrorBg, "#3c2828"),
            (Role::ToolTitle, "#d4d4d4"),
            (Role::ToolOutput, "#808080"),
            (Role::MdHeading, "#f0c674"),
            (Role::MdLink, "#81a2be"),
            (Role::MdLinkUrl, "#666666"),
            (Role::MdCode, "#8abeb7"),
            (Role::MdCodeBlock, "#b5bd68"),
            (Role::MdCodeBlockBorder, "#808080"),
            (Role::MdQuote, "#808080"),
            (Role::MdQuoteBorder, "#808080"),
            (Role::MdHr, "#808080"),
            (Role::MdListBullet, "#8abeb7"),
            (Role::ToolDiffAdded, "#b5bd68"),
            (Role::ToolDiffRemoved, "#cc6666"),
            (Role::ToolDiffContext, "#808080"),
            (Role::SyntaxComment, "#6a9955"),
            (Role::SyntaxKeyword, "#569cd6"),
            (Role::SyntaxFunction, "#dcdcaa"),
            (Role::SyntaxVariable, "#9cdcfe"),
            (Role::SyntaxString, "#ce9178"),
            (Role::SyntaxNumber, "#b5cea8"),
            (Role::SyntaxType, "#4ec9b0"),
            (Role::SyntaxOperator, "#d4d4d4"),
            (Role::SyntaxPunctuation, "#d4d4d4"),
            (Role::BashMode, "#b5bd68"),
        ]);
        // Gh #387 gap 4: the thinking border is a *ramp*, not seven literals.
        // See `theme::thinking` - the levels are authored in OKLCH so the
        // ordering is a property, not an accident of seven hex strings.
        palette.apply_thinking(None, lca_tui::engine::colors::ColorScheme::Dark);
        palette
    }

    /// The pi `light.json` palette.
    pub fn light() -> Palette {
        let mut palette = Palette::from(&[
            (Role::Accent, "#5a8080"),
            (Role::Border, "#547da7"),
            (Role::BorderAccent, "#5a8080"),
            (Role::BorderMuted, "#b0b0b0"),
            (Role::Success, "#588458"),
            (Role::Error, "#aa5555"),
            (Role::Warning, "#9a7326"),
            (Role::Muted, "#6c6c6c"),
            (Role::Dim, "#767676"),
            (Role::Text, "#1f2328"),
            (Role::ThinkingText, "#6c6c6c"),
            (Role::SelectedBg, "#d0d0e0"),
            (Role::ScrollbarTrack, "#b0b0b0"),
            (Role::ScrollbarThumb, "#1f2328"),
            (Role::SearchMatchBg, "#d0d0e0"),
            (Role::SearchMatchText, "#1f2328"),
            (Role::UserMessageBg, "#e8e8e8"),
            (Role::UserMessageText, "#1f2328"),
            (Role::CustomMessageBg, "#ede7f6"),
            (Role::SidePanel, "#e2e2ea"),
            (Role::CustomMessageText, "#1f2328"),
            (Role::CustomMessageLabel, "#7e57c2"),
            (Role::ToolPendingBg, "#e8e8f0"),
            (Role::ToolSuccessBg, "#e8f0e8"),
            (Role::ToolErrorBg, "#f0e8e8"),
            (Role::ToolTitle, "#1f2328"),
            (Role::ToolOutput, "#6c6c6c"),
            (Role::MdHeading, "#9a7326"),
            (Role::MdLink, "#547da7"),
            (Role::MdLinkUrl, "#767676"),
            (Role::MdCode, "#5a8080"),
            (Role::MdCodeBlock, "#588458"),
            (Role::MdCodeBlockBorder, "#6c6c6c"),
            (Role::MdQuote, "#6c6c6c"),
            (Role::MdQuoteBorder, "#6c6c6c"),
            (Role::MdHr, "#6c6c6c"),
            (Role::MdListBullet, "#588458"),
            (Role::ToolDiffAdded, "#588458"),
            (Role::ToolDiffRemoved, "#aa5555"),
            (Role::ToolDiffContext, "#6c6c6c"),
            (Role::SyntaxComment, "#008000"),
            (Role::SyntaxKeyword, "#0000ff"),
            (Role::SyntaxFunction, "#795e26"),
            (Role::SyntaxVariable, "#001080"),
            (Role::SyntaxString, "#a31515"),
            (Role::SyntaxNumber, "#098658"),
            (Role::SyntaxType, "#267f99"),
            (Role::SyntaxOperator, "#000000"),
            (Role::SyntaxPunctuation, "#000000"),
            (Role::BashMode, "#588458"),
        ]);
        palette.apply_thinking(None, lca_tui::engine::colors::ColorScheme::Light);
        palette
    }

    /// Build from `(role, hex)` pairs. A malformed hex is skipped so a
    /// hand-written file cannot take the interface down.
    pub fn from(pairs: &[(Role, &str)]) -> Palette {
        let mut colors = BTreeMap::new();
        for (role, hex) in pairs {
            if let Some(color) = parse_color(hex) {
                colors.insert(*role, color);
            }
        }
        Palette { colors }
    }

    /// The color for a role.
    ///
    /// A named role is its own value. A role the palette does not hold walks its
    /// declared chain (gh #387 gap 5) instead of guessing `Text`: `mdLink` finds
    /// `accent`, `syntaxType` finds `mdCode` and then `accent`, and a role whose
    /// whole chain is absent - or that has no entry in the table at all - comes
    /// back as [`Color::Default`], the terminal's own value. Before this, every
    /// missing role was `Text`, so a background role could be painted with a
    /// foreground colour.
    pub fn get(&self, role: Role) -> Color {
        super::tokens::resolve_chain(role, &|role| self.colors.get(&role).copied())
    }

    /// Whether the palette names `role` itself, rather than resolving it.
    ///
    /// The distinction matters at load: [`Palette::overlay`] must not treat a
    /// *resolved* value as though the theme file had set it, or `setting a
    /// parent carries its children` would collapse into `setting anything sets
    /// everything`.
    fn names(&self, role: Role) -> Option<Color> {
        self.colors.get(&role).copied()
    }

    /// Set one role's color (gh #10: the `system` theme tracks the
    /// terminal's own foreground and background over a scheme base).
    pub fn set(&mut self, role: Role, color: Color) {
        self.colors.insert(role, color);
    }

    /// Write the seven thinking roles from the heat ramp (gh #387 gap 4).
    ///
    /// One call per palette, and the `system` overlay calls it again with the
    /// probed background so the ramp steps away from the terminal's own page
    /// rather than from the scheme's nominal one.
    pub(crate) fn apply_thinking(
        &mut self,
        background: Option<lca_tui::engine::colors::RgbColor>,
        scheme: lca_tui::engine::colors::ColorScheme,
    ) {
        let levels = crate::theme::thinking::levels(background, scheme);
        for (role, level) in Role::THINKING.iter().zip(levels) {
            self.set(*role, level.color);
        }
    }

    /// The `system` palette (gh #10): pi's shape with our TOML
    /// divergence. Hues come from the scheme's built-in palette - full
    /// derivation needs contrast math (deferred oklch work, not this
    /// cycle) - while the foreground is the terminal's own and the
    /// message bands take the reported background when one is known.
    pub fn system(
        background: Option<lca_tui::engine::colors::RgbColor>,
        scheme: lca_tui::engine::colors::ColorScheme,
    ) -> Palette {
        let mut palette = match scheme {
            lca_tui::engine::colors::ColorScheme::Light => Palette::light(),
            lca_tui::engine::colors::ColorScheme::Dark => Palette::dark(),
        };
        palette.set(Role::Text, Color::Default);
        // Gh #387 gap 1: **shaded**, not `background`.
        //
        // These roles used to be set *equal to* the reported background, which
        // made a user band and a tool card the same colour as the page they sat
        // on - no band at all. Each family now takes the probed background,
        // mixes its own hue in, and steps one perceptual notch away from it
        // (lighter on dark, darker on light). The step is the same for every
        // family, so text contrast is equal across panels and the state reads
        // by hue alone; the mix is the background's, so Solarized surfaces stay
        // Solarized.
        //
        // `None` keeps every panel at `Default`: there is nothing to offset
        // from, and inventing a colour would paint a band the user did not ask
        // for (and break a translucent terminal).
        // Gh #387 gap 1: the `system` palette owns **every** panel role.
        //
        // The scheme base sets them so the named themes (`dark`, `light`) are
        // self-contained, but a `system` session has no business painting them
        // from a base: either the terminal told us its background, in which case
        // each family is derived from *that*, or it did not, in which case the
        // panels stay transparent. Leaving the base's values in place for the
        // `None` case would paint opaque bands over a translucent terminal the
        // user configured on purpose.
        // Gh #387 gap 4: the ramp steps away from the *probed* background when
        // there is one, so a Solarized Dark session gets a ramp anchored to its
        // own page rather than to `dark()`'s nominal lightness.
        palette.apply_thinking(background, scheme);
        for family in crate::theme::panels::families() {
            let derived = background
                .and_then(|background| crate::theme::panels::panel(background, scheme, &family));
            palette.set(family.role, derived.unwrap_or(Color::Default));
        }
        palette
    }

    /// Whether every role is present.
    pub fn is_complete(&self) -> bool {
        Role::ALL.iter().all(|role| self.colors.contains_key(role))
    }

    /// Overlay a theme file's roles onto this palette (S5): only the roles
    /// the file names change, and only when their value parses. Returns the
    /// number of roles applied, or an error naming the first bad value.
    pub fn overlay(&self, text: &str) -> Result<(Palette, usize), String> {
        let value: toml::Value = toml::from_str(text).map_err(|err| err.to_string())?;
        // A custom theme may nest its roles under `[colors]`, like pi's
        // JSON; a flat file is accepted too.
        let table = value
            .get("colors")
            .and_then(|v| v.as_table())
            .or_else(|| value.as_table())
            .ok_or_else(|| "the theme file is not a table of roles".to_string())?;
        // `vars` is a named-color table a role may reference (pi's shape).
        let vars = value
            .get("vars")
            .and_then(|v| v.as_table())
            .cloned()
            .unwrap_or_default();
        // Gh #387 gap 5: the table has to be sound before we resolve anything
        // through it, or a bad entry would show up as a wrong colour on screen
        // instead of an error at load. A malformed table is a programming error,
        // so this fails loudly and names the role.
        super::tokens::validate()?;
        // What the *file* said, kept separate from what the palette underneath
        // said: a child must follow a parent the **user** set, and must keep its
        // own base value when the user set neither.
        let mut named: BTreeMap<Role, Color> = BTreeMap::new();
        for (key, entry) in table {
            if key == "vars" || key == "name" {
                continue;
            }
            // An unknown role stays an error with the same message it has
            // always had; this cycle adds fallbacks, not a second vocabulary.
            let Some(role) = Role::parse(key) else {
                continue;
            };
            let Some(raw) = entry.as_str() else {
                return Err(format!("role `{key}` is not a string"));
            };
            let reference = vars.get(raw).and_then(|v| v.as_str()).unwrap_or(raw);
            let Some(color) = parse_color(reference) else {
                return Err(format!("role `{key}` has an invalid color `{raw}`"));
            };
            named.insert(role, color);
        }
        let applied = named.len();
        // Resolve **now**, at load, so the render path reads one map value.
        //
        // Precedence per role: what the file said; else the nearest ancestor the
        // file said (this is what makes a parent override carry its children);
        // else this palette's own value for that role (a complete base always
        // has one, so a built-in palette keeps its distinct per-role colours
        // instead of collapsing onto `accent`); else the terminal default.
        let mut palette = self.clone();
        for role in Role::ALL {
            let color = named
                .get(role)
                .copied()
                .or_else(|| super::tokens::nearest_named(*role, &|role| named.get(&role).copied()))
                .or_else(|| self.names(*role))
                .unwrap_or(Color::Default);
            palette.colors.insert(*role, color);
        }
        Ok((palette, applied))
    }
}

/// Parse `#rrggbb`, `#rgb`, `rrggbb`, or a 256-color index (`0`–`255`).
/// `""` means the terminal default, pi's own convention.
pub(super) fn parse_color(value: &str) -> Option<Color> {
    let value = value.trim();
    if value.is_empty() {
        return Some(Color::Default);
    }
    let hex = value.strip_prefix('#').unwrap_or(value);
    // gh #10: pi's three-digit form expands digit by digit
    // (`#abc` is `#aabbcc`, and `0xa * 0x11 == 0xaa`).
    if hex.len() == 3 && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        let mut rgb = [0u8; 3];
        for (i, slot) in rgb.iter_mut().enumerate() {
            *slot = u8::from_str_radix(&hex[i..i + 1], 16).ok()? * 0x11;
        }
        return Some(Color::Rgb(rgb[0], rgb[1], rgb[2]));
    }
    let value = value.trim();
    if value.is_empty() {
        return Some(Color::Default);
    }
    let hex = value.strip_prefix('#').unwrap_or(value);
    if hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
        let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
        let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
        let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
        return Some(Color::Rgb(r, g, b));
    }
    if let Ok(index) = value.parse::<u8>() {
        return Some(Color::Ansi256(index));
    }
    None
}
