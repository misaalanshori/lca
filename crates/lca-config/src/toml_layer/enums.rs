//! Key family: a key whose value is one word from a vocabulary.

use crate::keys::{
    CODEBLOCK_BORDERS, MERMAID_MODES, PERMISSION_MODES, QUEUE_MODES, SHELL_TOOLS,
    THINKING_VISIBILITIES,
};
use crate::type_name;
use crate::{ConfigError, TypedValue};

/// The keys this family owns. `toml_layer/mod.rs`'s dispatch table routes each of
/// them here, and `tests/keys_agree.rs` pins that the two lists name one set.
pub(super) const KEYS: &[&str] = &[
    "markdown.mermaid",
    "compaction.strategy",
    "shell.tool",
    "ui.thinking",
    "markdown.codeblock_border",
    "permissions.mode",
    "queue.steering_mode",
    "queue.follow_up_mode",
];

/// Type one of [`KEYS`]. The arms are the ones the closure held, verbatim except
/// that `this.apply(...)` became the return: the dispatch in `toml_layer/mod.rs`
/// is the single place that applies a typed value.
pub(super) fn parse(
    key: &str,
    value: &toml::Value,
    label: &str,
) -> Result<TypedValue, ConfigError> {
    let invalid = |reason: String| ConfigError::InvalidValue {
        key: key.to_string(),
        label: label.to_string(),
        reason,
    };
    match key {
        "markdown.mermaid" => {
            let text = value
                .as_str()
                .ok_or_else(|| invalid(format!("expected a string, got {}", type_name(value))))?;
            if !MERMAID_MODES.contains(&text) {
                return Err(invalid(format!(
                    "expected {}, got `{text}`",
                    MERMAID_MODES.join(", ")
                )));
            }
            Ok(TypedValue::Text(text.to_string()))
        }
        "compaction.strategy" => {
            let text = value
                .as_str()
                .ok_or_else(|| invalid(format!("expected a string, got {}", type_name(value))))?;
            if text.trim().is_empty() {
                return Err(invalid(
                    "expected an extension name, got an empty string".to_string(),
                ));
            }
            Ok(TypedValue::Text(text.to_string()))
        }
        "shell.tool" => {
            let text = value
                .as_str()
                .ok_or_else(|| invalid(format!("expected a string, got {}", type_name(value))))?;
            if !SHELL_TOOLS.contains(&text) {
                return Err(invalid(format!(
                    "expected one of {}, got `{text}`",
                    SHELL_TOOLS.join(", ")
                )));
            }
            Ok(TypedValue::Text(text.to_string()))
        }
        "ui.thinking" => {
            let text = value
                .as_str()
                .ok_or_else(|| invalid(format!("expected a string, got {}", type_name(value))))?;
            if !THINKING_VISIBILITIES.contains(&text) {
                return Err(invalid(format!(
                    "expected one of {}, got `{text}`",
                    THINKING_VISIBILITIES.join(", ")
                )));
            }
            Ok(TypedValue::Text(text.to_string()))
        }
        "markdown.codeblock_border" => {
            let text = value
                .as_str()
                .ok_or_else(|| invalid(format!("expected a string, got {}", type_name(value))))?;
            if !CODEBLOCK_BORDERS.contains(&text) {
                return Err(invalid(format!(
                    "expected one of {}, got `{text}`",
                    CODEBLOCK_BORDERS.join(", ")
                )));
            }
            Ok(TypedValue::Text(text.to_string()))
        }
        "permissions.mode" => {
            let text = value
                .as_str()
                .ok_or_else(|| invalid(format!("expected a string, got {}", type_name(value))))?;
            if !PERMISSION_MODES.contains(&text) {
                return Err(invalid(format!(
                    "expected one of {}, got `{text}`",
                    PERMISSION_MODES.join(", ")
                )));
            }
            Ok(TypedValue::Text(text.to_string()))
        }
        "queue.steering_mode" | "queue.follow_up_mode" => {
            let text = value
                .as_str()
                .ok_or_else(|| invalid(format!("expected a string, got {}", type_name(value))))?;
            if !QUEUE_MODES.contains(&text) {
                return Err(invalid(format!(
                    "expected one of {}, got `{text}`",
                    QUEUE_MODES.join(", ")
                )));
            }
            Ok(TypedValue::Text(text.to_string()))
        }
        // Unreachable by construction - `KEYS` names exactly the arms above, and
        // the dispatch routes only those keys here (`tests/keys_agree.rs` pins the
        // two lists against each other) - but it carries the closure's own copy so
        // that a mis-route reports what an unknown key always has, rather than a
        // message nobody has ever seen.
        _ => Err(invalid("unknown configuration key".to_string())),
    }
}
