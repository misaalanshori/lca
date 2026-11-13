//! Key family: a key whose value is free-form text - a theme name, a shell path
//! or prefix, a theme colour, the code-block indent.
//!
//! What these share is the absence of a vocabulary: the value is taken as
//! written, with at most a shape check (`ui.color` parses a `ColorMode`).

use crate::type_name;
use crate::{ColorMode, ConfigError, TypedValue};

/// The keys this family owns. `toml_layer/mod.rs`'s dispatch table routes each of
/// them here, and `tests/keys_agree.rs` pins that the two lists name one set.
pub(super) const KEYS: &[&str] = &[
    "markdown.code_block_indent",
    "ui.color",
    "ui.theme",
    "shell.path",
    "shell.command_prefix",
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
        "markdown.code_block_indent" => {
            let text = value
                .as_str()
                .ok_or_else(|| invalid(format!("expected a string, got {}", type_name(value))))?;
            if text.contains('\n') || text.len() > 8 {
                return Err(invalid("expected a short single-line indent".to_string()));
            }
            Ok(TypedValue::Text(text.to_string()))
        }
        "ui.color" => {
            let text = value
                .as_str()
                .ok_or_else(|| invalid(format!("expected a string, got {}", type_name(value))))?;
            let color: ColorMode = text.parse().map_err(invalid)?;
            Ok(TypedValue::Color(color))
        }
        "ui.theme" | "shell.path" | "shell.command_prefix" => {
            let text = value
                .as_str()
                .ok_or_else(|| invalid(format!("expected a string, got {}", type_name(value))))?;
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
