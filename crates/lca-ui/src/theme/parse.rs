//! Resolving a `ui.theme` setting into a palette: the built-in names, the
//! custom-file lookup, and the overlay that applies a file's role table.
//!
//! Split from `theme/mod.rs` at gate 11's 1,200-line ceiling (gh #574). This
//! is the file where a *setting* becomes a palette; `mod.rs` keeps the role
//! vocabulary, the built-in palettes and the accessors that read them. Every
//! function here was already `pub` at `theme::<name>`, and `mod.rs` re-exports
//! them, so no caller moves.

use std::path::Path;

use super::Theme;

/// Resolve the `ui.theme` setting (S5): `"plain"`, `"auto"`, a built-in
/// name, or a custom theme file's name. A custom file overlays the detected
/// scheme's base palette; `name.light` / `name.dark` siblings let one name
/// carry both sides. An unreadable or invalid file keeps the base palette.
pub fn load(
    setting: &str,
    scheme: Option<lca_tui::engine::colors::ColorScheme>,
    dir: &Path,
) -> (Theme, Option<String>) {
    load_all(setting, scheme, std::slice::from_ref(&dir.to_path_buf()))
}

/// Resolve the theme across dirs (gh #70): explicit `--theme` dirs
/// first, then the configured one. The first readable file wins (an
/// invalid file keeps the base and says why, like `load`).
pub fn load_all(
    setting: &str,
    scheme: Option<lca_tui::engine::colors::ColorScheme>,
    dirs: &[std::path::PathBuf],
) -> (Theme, Option<String>) {
    // A built-in name resolves directly; only an unknown name is a custom
    // theme file to look up. F2: these arms used to fall through to the
    // file lookup, so `ui.theme = "dark"` printed a spurious "not found".
    let (base, default_name) = match setting {
        "plain" => return (Theme::plain(), None),
        "auto" | "" => return (Theme::for_scheme(scheme), None),
        "default" | "dark" => return (Theme::colored(), None),
        "light" => return (Theme::light(), None),
        // gh #10: `system` is a reserved name (pi's rule) - it always
        // derives from detection, never from a theme file.
        "system" => return (Theme::system_theme(None, scheme), None),
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
    let mut candidates = Vec::new();
    for dir in dirs {
        candidates.push(dir.join(format!("{setting}.{side}.toml")));
        candidates.push(dir.join(format!("{setting}.toml")));
    }
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
            //
            // Gh #575: the reason is typed, so the two audiences are told
            // apart. A file fault is the user's to fix; a token-table fault is
            // a bug in this crate, and calling it "your theme was ignored"
            // would send the reader to the wrong file.
            Err(reason) => {
                let notice = if reason.is_programming_defect() {
                    format!(
                        "theme `{setting}`: the built-in role table is broken ({reason}); \
                         this is a bug in LCA, not in your theme - keeping the previous palette"
                    )
                } else {
                    format!("theme `{setting}` ignored ({reason}); keeping the previous palette")
                };
                (base, Some(notice))
            }
        };
    }
    // A custom name with no file: say so rather than silently showing the
    // scheme default under the user's chosen name.
    let searched = dirs
        .iter()
        .map(|dir| dir.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    (
        base,
        Some(format!(
            "theme `{setting}` not found in {searched}; using the {default_name} default",
            searched = if searched.is_empty() {
                "(no theme dirs)"
            } else {
                &searched
            },
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
