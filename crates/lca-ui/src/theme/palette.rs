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
        Palette::from(&[
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
            (Role::ThinkingOff, "#505050"),
            (Role::ThinkingMinimal, "#6e6e6e"),
            (Role::ThinkingLow, "#5f87af"),
            (Role::ThinkingMedium, "#81a2be"),
            (Role::ThinkingHigh, "#b294bb"),
            (Role::ThinkingXhigh, "#d183e8"),
            (Role::ThinkingMax, "#ff5fff"),
            (Role::BashMode, "#b5bd68"),
        ])
    }

    /// The pi `light.json` palette.
    pub fn light() -> Palette {
        Palette::from(&[
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
            (Role::ThinkingOff, "#b0b0b0"),
            (Role::ThinkingMinimal, "#767676"),
            (Role::ThinkingLow, "#547da7"),
            (Role::ThinkingMedium, "#5a8080"),
            (Role::ThinkingHigh, "#875f87"),
            (Role::ThinkingXhigh, "#8b008b"),
            (Role::ThinkingMax, "#af005f"),
            (Role::BashMode, "#588458"),
        ])
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

    /// The color for a role; a missing role falls back to `Text`.
    pub fn get(&self, role: Role) -> Color {
        self.colors
            .get(&role)
            .copied()
            .or_else(|| self.colors.get(&Role::Text).copied())
            .unwrap_or(Color::Default)
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
        let mut palette = self.clone();
        let mut applied = 0usize;
        for (key, entry) in table {
            if key == "vars" || key == "name" {
                continue;
            }
            let Some(role) = Role::parse(key) else {
                continue;
            };
            let Some(raw) = entry.as_str() else {
                return Err(format!("role `{key}` is not a string"));
            };
            let resolved = vars.get(raw).and_then(|v| v.as_str()).unwrap_or(raw);
            let Some(color) = parse_color(resolved) else {
                return Err(format!("role `{key}` has an invalid color `{raw}`"));
            };
            palette.colors.insert(role, color);
            applied += 1;
        }
        Ok((palette, applied))
    }
}

/// Parse `#rrggbb`, `rrggbb`, or a 256-color index (`0`–`255`). `""` means
/// the terminal default, pi's own convention.
pub(super) fn parse_color(value: &str) -> Option<Color> {
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
