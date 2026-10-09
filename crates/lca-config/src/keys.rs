//! Key definitions and string-form parsing (ceiling split): the
//! known-key table, the per-key vocabularies, and the flag/environment
//! string parser. The TOML layer and the merge live in `lib.rs`.

use std::collections::BTreeMap;

use super::{ColorMode, ConfigError};

/// The thinking levels a model can be asked for (`thinking`), matching pi's
/// `ThinkingLevel` vocabulary (`packages/agent/src/types.ts`). `off` disables
/// reasoning; the rest scale it. An unknown value is refused at load.
pub const THINKING_LEVELS: &[&str] = &["off", "minimal", "low", "medium", "high", "xhigh", "max"];

/// The `shell.tool` vocabulary (ADR-0041): `auto` plus each interpreter the
/// ladder knows how to resolve exactly.
pub const SHELL_TOOLS: &[&str] = &["auto", "bash", "pwsh", "powershell", "cmd"];

/// The `permissions.mode` vocabulary (ADR-0042).
pub const PERMISSION_MODES: &[&str] = &["ask", "yolo"];

/// The shapes `markdown.codeblock_border` accepts (gh #32).
pub const CODEBLOCK_BORDERS: &[&str] = &["full", "horizontal", "none"];

/// The `ui.thinking` vocabulary (R6): how much of a reasoning run
/// the transcript shows, separate from `thinking`'s effort level.
pub const THINKING_VISIBILITIES: &[&str] = &["snippet", "full", "hidden"];

/// The `ui.double_escape_action` vocabulary (gh #132, pi's
/// `doubleEscapeAction`).
pub const DOUBLE_ESCAPE_ACTIONS: &[&str] = &["tree", "fork", "none"];

/// Fullscreen scrollbar behavior (gh #82, pi's `fullscreenScrollbar`).
pub const FULLSCREEN_SCROLLBARS: &[&str] = &["auto", "always", "hidden"];

/// Fullscreen exit behavior (gh #82, pi's `fullscreenExitOutput`).
pub const FULLSCREEN_EXIT_OUTPUTS: &[&str] = &["transcript", "resume-hint"];

/// Mermaid rendering modes (gh #82, pi's `markdown.mermaid`).
pub const MERMAID_MODES: &[&str] = &["off", "final", "streaming"];

/// Pi's `treeFilterMode` vocabulary (gh #132): accepted
/// config-error-free, currently inert (the no-op documents itself in
/// `docs/configuration.md`).
pub const TREE_FILTER_MODES: &[&str] = &["default", "no-tools", "user-only", "labeled-only", "all"];

/// Pi's `defaultProjectTrust` vocabulary (gh #80): the fallback project-trust
/// behavior when no stored decision exists. User file or environment only;
/// a project file that granted its own trust would defeat FR-PERM-9.
pub const TRUST_DEFAULTS: &[&str] = &["ask", "always", "never"];

/// Every configuration key an `LCA_` environment variable can set.
/// `docs/configuration.md` documents two more that have no environment
/// form because they are tables, not single values: `models.thinking_levels`
/// (per-model lists), `thinking.budgets` (per-level token counts), and
/// `permissions.proposals`.
pub const KNOWN_KEYS: &[&str] = &[
    "provider",
    "model",
    "models.enabled",
    "compaction.threshold",
    "compaction.enabled",
    "compaction.reserve_tokens",
    "compaction.keep_recent_tokens",
    "provider.retry_limit",
    "provider.retry_base_delay_ms",
    "tool.timeout_seconds",
    "tool.result_limit_bytes",
    "tool.max_iterations",
    "tool.edit_requires_read",
    "cache.noise_floor_tokens",
    "extensions.log_limit_bytes",
    "update.check",
    "ui.fullscreen",
    "ui.quiet_startup",
    "ui.double_escape_action",
    "ui.tree_filter_mode",
    "ui.autocomplete_max_visible",
    "ui.editor_padding_x",
    "ui.output_pad",
    "ui.fullscreen_scrollbar",
    "ui.fullscreen_copy_on_select",
    "ui.fullscreen_wheel_lines",
    "ui.fullscreen_exit_output",
    "ui.show_hardware_cursor",
    "terminal.show_images",
    "terminal.image_width_cells",
    "terminal.clear_on_shrink",
    "terminal.show_progress",
    "terminal.hyperlinks",
    "terminal.images",
    "terminal.true_color",
    "images.auto_resize",
    "images.block_images",
    "markdown.code_block_indent",
    "markdown.mermaid",
    "ui.color",
    "ui.theme",
    "thinking",
    "shell.tool",
    "shell.path",
    "shell.command_prefix",
    "permissions.mode",
    "trust.default_project",
    "ui.thinking",
    "markdown.codeblock_border",
];

pub(crate) fn csv(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect()
}

/// Validate and type a single string value for a key (gh #82 fix):
/// the settings writer runs cycle values through this so bools and
/// numbers persist typed instead of as quoted strings the loader
/// would refuse. `label` names the source in refusal errors.
pub fn parse_typed(key: &str, raw: &str, label: &str) -> Result<TypedValue, ConfigError> {
    let invalid = |reason: String| ConfigError::InvalidValue {
        key: key.to_string(),
        label: label.to_string(),
        reason,
    };
    match key {
        "provider" | "model" => Ok(TypedValue::Text(raw.to_string())),
        // The flag/environment form of a list key is a comma list.
        "models.enabled" => Ok(TypedValue::List(csv(raw))),
        "ui.double_escape_action" => match raw {
            _ if DOUBLE_ESCAPE_ACTIONS.contains(&raw) => Ok(TypedValue::Text(raw.to_string())),
            _ => Err(invalid(format!(
                "expected {}, got `{raw}`",
                DOUBLE_ESCAPE_ACTIONS.join(", ")
            ))),
        },
        "trust.default_project" => match raw {
            _ if TRUST_DEFAULTS.contains(&raw) => Ok(TypedValue::Text(raw.to_string())),
            _ => Err(invalid(format!(
                "expected {}, got `{raw}`",
                TRUST_DEFAULTS.join(", ")
            ))),
        },
        "ui.tree_filter_mode" => match raw {
            _ if TREE_FILTER_MODES.contains(&raw) => Ok(TypedValue::Text(raw.to_string())),
            _ => Err(invalid(format!(
                "expected {}, got `{raw}`",
                TREE_FILTER_MODES.join(", ")
            ))),
        },
        "ui.autocomplete_max_visible" => match raw.parse::<u64>() {
            Ok(n) if (3..=20).contains(&n) => Ok(TypedValue::Count(n)),
            _ => Err(invalid(format!("expected an integer 3-20, got `{raw}`"))),
        },
        "ui.editor_padding_x" => match raw.parse::<u64>() {
            Ok(n) if n <= 3 => Ok(TypedValue::Count(n)),
            _ => Err(invalid(format!("expected an integer 0-3, got `{raw}`"))),
        },
        "ui.output_pad" => match raw {
            "0" => Ok(TypedValue::Count(0)),
            "1" => Ok(TypedValue::Count(1)),
            _ => Err(invalid(format!("expected 0 or 1, got `{raw}`"))),
        },
        "ui.fullscreen_scrollbar" => match raw {
            _ if FULLSCREEN_SCROLLBARS.contains(&raw) => Ok(TypedValue::Text(raw.to_string())),
            _ => Err(invalid(format!(
                "expected {}, got `{raw}`",
                FULLSCREEN_SCROLLBARS.join(", ")
            ))),
        },
        "ui.fullscreen_wheel_lines" => match raw {
            "auto" => Ok(TypedValue::Text(raw.to_string())),
            _ => match raw.parse::<u64>() {
                Ok(n) if (1..=100).contains(&n) => Ok(TypedValue::Text(raw.to_string())),
                _ => Err(invalid(format!(
                    "expected auto or an integer 1-100, got `{raw}`"
                ))),
            },
        },
        "ui.fullscreen_exit_output" => match raw {
            _ if FULLSCREEN_EXIT_OUTPUTS.contains(&raw) => Ok(TypedValue::Text(raw.to_string())),
            _ => Err(invalid(format!(
                "expected {}, got `{raw}`",
                FULLSCREEN_EXIT_OUTPUTS.join(", ")
            ))),
        },
        "terminal.image_width_cells" => match raw.parse::<u64>() {
            Ok(n) if n >= 1 => Ok(TypedValue::Count(n)),
            _ => Err(invalid(format!("expected a positive integer, got `{raw}`"))),
        },
        "terminal.hyperlinks" | "terminal.true_color" => match raw {
            "true" | "false" | "auto" => Ok(TypedValue::Text(raw.to_string())),
            _ => Err(invalid(format!(
                "expected true, false, or auto, got `{raw}`"
            ))),
        },
        "terminal.images" => match raw {
            "kitty" | "iterm2" | "auto" | "false" => Ok(TypedValue::Text(raw.to_string())),
            _ => Err(invalid(format!(
                "expected kitty, iterm2, auto, or false, got `{raw}`"
            ))),
        },
        "markdown.code_block_indent" => match raw {
            _ if !raw.contains('\n') && raw.len() <= 8 => Ok(TypedValue::Text(raw.to_string())),
            _ => Err(invalid("expected a short single-line indent".to_string())),
        },
        "markdown.mermaid" => match raw {
            _ if MERMAID_MODES.contains(&raw) => Ok(TypedValue::Text(raw.to_string())),
            _ => Err(invalid(format!(
                "expected {}, got `{raw}`",
                MERMAID_MODES.join(", ")
            ))),
        },
        "ui.fullscreen_copy_on_select"
        | "ui.show_hardware_cursor"
        | "terminal.show_images"
        | "terminal.clear_on_shrink"
        | "terminal.show_progress"
        | "images.auto_resize"
        | "images.block_images" => raw
            .parse::<bool>()
            .map(TypedValue::Bool)
            .map_err(|_| invalid(format!("expected a boolean, got `{raw}`"))),
        "ui.quiet_startup" => match raw {
            "true" | "false" | "header" => Ok(TypedValue::Text(raw.to_string())),
            _ => Err(invalid(format!(
                "expected true, false, or \"header\", got `{raw}`"
            ))),
        },
        "update.check" | "ui.fullscreen" | "tool.edit_requires_read" => raw
            .parse::<bool>()
            .map(TypedValue::Bool)
            .map_err(|_| invalid(format!("expected a boolean, got `{raw}`"))),
        "compaction.threshold" => {
            let value: f64 = raw
                .parse()
                .map_err(|_| invalid(format!("expected a number, got `{raw}`")))?;
            if !(0.0..=1.0).contains(&value) {
                return Err(invalid(format!(
                    "expected a fraction in 0.0..=1.0, got {value}"
                )));
            }
            Ok(TypedValue::Number(value))
        }
        "ui.color" => Ok(TypedValue::Color(
            raw.parse::<ColorMode>().map_err(invalid)?,
        )),
        "ui.theme" => Ok(TypedValue::Text(raw.to_string())),
        "shell.path" | "shell.command_prefix" => Ok(TypedValue::Text(raw.to_string())),
        "markdown.codeblock_border" => {
            if CODEBLOCK_BORDERS.contains(&raw) {
                Ok(TypedValue::Text(raw.to_string()))
            } else {
                Err(invalid(format!(
                    "expected one of {}, got `{raw}`",
                    CODEBLOCK_BORDERS.join(", ")
                )))
            }
        }
        "ui.thinking" => {
            if THINKING_VISIBILITIES.contains(&raw) {
                Ok(TypedValue::Text(raw.to_string()))
            } else {
                Err(invalid(format!(
                    "expected one of {}, got `{raw}`",
                    THINKING_VISIBILITIES.join(", ")
                )))
            }
        }
        "skills.inject_matched" => raw
            .parse::<bool>()
            .map(TypedValue::Bool)
            .map_err(|_| invalid(format!("expected a boolean, got `{raw}`"))),
        "compaction.enabled" => raw
            .parse::<bool>()
            .map(TypedValue::Bool)
            .map_err(|_| invalid(format!("expected a boolean, got `{raw}`"))),
        "permissions.mode" => {
            if PERMISSION_MODES.contains(&raw) {
                Ok(TypedValue::Text(raw.to_string()))
            } else {
                Err(invalid(format!(
                    "expected one of {}, got `{raw}`",
                    PERMISSION_MODES.join(", ")
                )))
            }
        }
        "shell.tool" => {
            if SHELL_TOOLS.contains(&raw) {
                Ok(TypedValue::Text(raw.to_string()))
            } else {
                Err(invalid(format!(
                    "expected one of {}, got `{raw}`",
                    SHELL_TOOLS.join(", ")
                )))
            }
        }
        "thinking" => {
            if THINKING_LEVELS.contains(&raw) {
                Ok(TypedValue::Text(raw.to_string()))
            } else {
                Err(invalid(format!(
                    "expected one of {}, got `{raw}`",
                    THINKING_LEVELS.join(", ")
                )))
            }
        }
        "provider.retry_limit"
        | "tool.timeout_seconds"
        | "tool.result_limit_bytes"
        | "tool.max_iterations"
        | "cache.noise_floor_tokens"
        | "extensions.log_limit_bytes"
        | "compaction.reserve_tokens"
        | "compaction.keep_recent_tokens" => raw
            .parse::<u64>()
            .map(TypedValue::Count)
            .map_err(|_| invalid(format!("expected a non-negative integer, got `{raw}`"))),
        other => Err(invalid(format!("unknown configuration key `{other}`"))),
    }
}

/// A config value in transit, before `Config::apply` files it. The
/// settings writer maps these to typed TOML so the file stays
/// loadable (gh #82 fix).
#[derive(Debug, Clone)]
pub enum TypedValue {
    /// Free text or a validated vocabulary word.
    Text(String),
    /// A list of strings (`models.enabled`), whose flag/environment form
    /// is a comma list.
    List(Vec<String>),
    /// Per-model allowed thinking levels (`models.thinking_levels`).
    ThinkingLevels(BTreeMap<String, Vec<String>>),
    /// Per-level token-budget overrides (`thinking.budgets`).
    Budgets(BTreeMap<String, u64>),
    /// A validated non-negative integer.
    Count(u64),
    /// A validated floating-point number.
    Number(f64),
    /// A validated boolean.
    Bool(bool),
    /// A validated color mode.
    Color(ColorMode),
}
