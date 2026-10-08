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

/// Every configuration key an `LCA_` environment variable can set.
/// `docs/configuration.md` documents two more that have no environment
/// form because they are tables, not single values: `models.thinking_levels`
/// (per-model lists) and `permissions.proposals`.
pub const KNOWN_KEYS: &[&str] = &[
    "provider",
    "model",
    "models.enabled",
    "compaction.threshold",
    "compaction.enabled",
    "compaction.reserve_tokens",
    "compaction.keep_recent_tokens",
    "provider.retry_limit",
    "tool.timeout_seconds",
    "tool.result_limit_bytes",
    "tool.max_iterations",
    "cache.noise_floor_tokens",
    "extensions.log_limit_bytes",
    "update.check",
    "ui.fullscreen",
    "ui.color",
    "ui.theme",
    "thinking",
    "shell.tool",
    "shell.path",
    "shell.command_prefix",
    "permissions.mode",
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

pub(crate) fn parse_typed(key: &str, raw: &str, label: &str) -> Result<TypedValue, ConfigError> {
    let invalid = |reason: String| ConfigError::InvalidValue {
        key: key.to_string(),
        label: label.to_string(),
        reason,
    };
    match key {
        "provider" | "model" => Ok(TypedValue::Text(raw.to_string())),
        // The flag/environment form of a list key is a comma list.
        "models.enabled" => Ok(TypedValue::List(csv(raw))),
        "update.check" | "ui.fullscreen" => raw
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

/// A config value in transit, before `Config::apply` files it.
pub(crate) enum TypedValue {
    Text(String),
    /// A list of strings (`models.enabled`), whose flag/environment form
    /// is a comma list.
    List(Vec<String>),
    /// Per-model allowed thinking levels (`models.thinking_levels`).
    ThinkingLevels(BTreeMap<String, Vec<String>>),
    Count(u64),
    Number(f64),
    Bool(bool),
    Color(ColorMode),
}
