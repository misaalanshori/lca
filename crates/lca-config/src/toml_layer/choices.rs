//! Key family: a key whose value is one of a closed list, or a string whose
//! shape is documented (`terminal.hyperlinks`'s true/false/auto, a scrollbar
//! mode, a wheel-lines count that may also be `auto`).

use crate::keys::{
    CACHE_WARMING_MODES, DOUBLE_ESCAPE_ACTIONS, FULLSCREEN_EXIT_OUTPUTS, FULLSCREEN_SCROLLBARS,
    TREE_FILTER_MODES, TRUST_DEFAULTS,
};
use crate::type_name;
use crate::{ConfigError, TypedValue};

/// The keys this family owns. `toml_layer/mod.rs`'s dispatch table routes each of
/// them here, and `tests/keys_agree.rs` pins that the two lists name one set.
pub(super) const KEYS: &[&str] = &[
    "trust.default_project",
    "ui.double_escape_action",
    "ui.tree_filter_mode",
    "cache.warming",
    "ui.fullscreen_scrollbar",
    "ui.fullscreen_wheel_lines",
    "ui.fullscreen_exit_output",
    "terminal.hyperlinks",
    "terminal.true_color",
    "terminal.images",
    "ui.quiet_startup",
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
        "trust.default_project"
        | "ui.double_escape_action"
        | "ui.tree_filter_mode"
        | "cache.warming" => {
            // The domain lives here too, not just in the
            // flag/env parser: a file value skips that path.
            let text = value
                .as_str()
                .ok_or_else(|| invalid(format!("expected a string, got {}", type_name(value))))?;
            let ok = match key {
                "ui.double_escape_action" => DOUBLE_ESCAPE_ACTIONS.contains(&text),
                "trust.default_project" => TRUST_DEFAULTS.contains(&text),
                "cache.warming" => CACHE_WARMING_MODES.contains(&text),
                _ => TREE_FILTER_MODES.contains(&text),
            };
            if !ok {
                return Err(invalid(format!("unexpected {key} value `{text}`")));
            }
            Ok(TypedValue::Text(text.to_string()))
        }
        "ui.fullscreen_scrollbar" => {
            let text = value
                .as_str()
                .ok_or_else(|| invalid(format!("expected a string, got {}", type_name(value))))?;
            if !FULLSCREEN_SCROLLBARS.contains(&text) {
                return Err(invalid(format!(
                    "expected {}, got `{text}`",
                    FULLSCREEN_SCROLLBARS.join(", ")
                )));
            }
            Ok(TypedValue::Text(text.to_string()))
        }
        "ui.fullscreen_wheel_lines" => {
            let text = match value {
                toml::Value::String(text) => text.clone(),
                toml::Value::Integer(n) => n.to_string(),
                _ => {
                    return Err(invalid(format!(
                        "expected auto or an integer 1-100, got {}",
                        type_name(value)
                    )));
                }
            };
            let ok = text == "auto" || text.parse::<u64>().is_ok_and(|n| (1..=100).contains(&n));
            if !ok {
                return Err(invalid(format!(
                    "expected auto or an integer 1-100, got `{text}`"
                )));
            }
            Ok(TypedValue::Text(text))
        }
        "ui.fullscreen_exit_output" => {
            let text = value
                .as_str()
                .ok_or_else(|| invalid(format!("expected a string, got {}", type_name(value))))?;
            if !FULLSCREEN_EXIT_OUTPUTS.contains(&text) {
                return Err(invalid(format!(
                    "expected {}, got `{text}`",
                    FULLSCREEN_EXIT_OUTPUTS.join(", ")
                )));
            }
            Ok(TypedValue::Text(text.to_string()))
        }
        "terminal.hyperlinks" | "terminal.true_color" => {
            let text = match value {
                toml::Value::Boolean(flag) => flag.to_string(),
                toml::Value::String(text) => text.clone(),
                _ => {
                    return Err(invalid(format!(
                        "expected true, false, or auto, got {}",
                        type_name(value)
                    )));
                }
            };
            if !["true", "false", "auto"].contains(&text.as_str()) {
                return Err(invalid(format!(
                    "expected true, false, or auto, got `{text}`"
                )));
            }
            Ok(TypedValue::Text(text))
        }
        "terminal.images" => {
            let text = match value {
                toml::Value::Boolean(false) => "false".to_string(),
                toml::Value::String(text) => text.clone(),
                _ => {
                    return Err(invalid(format!(
                        "expected kitty, iterm2, auto, or false, got {}",
                        type_name(value)
                    )));
                }
            };
            if !["kitty", "iterm2", "auto", "false"].contains(&text.as_str()) {
                return Err(invalid(format!(
                    "expected kitty, iterm2, auto, or false, got `{text}`"
                )));
            }
            Ok(TypedValue::Text(text))
        }
        "ui.quiet_startup" => {
            // bool | "header": TOML true/false or the string.
            let text = match value {
                toml::Value::Boolean(flag) => flag.to_string(),
                toml::Value::String(text) if text == "header" => text.clone(),
                _ => {
                    return Err(invalid(format!(
                        "expected true, false, or \"header\", got {}",
                        type_name(value)
                    )));
                }
            };
            Ok(TypedValue::Text(text))
        }
        // Unreachable by construction - `KEYS` names exactly the arms above, and
        // the dispatch routes only those keys here (`tests/keys_agree.rs` pins the
        // two lists against each other) - but it carries the closure's own copy so
        // that a mis-route reports what an unknown key always has, rather than a
        // message nobody has ever seen.
        _ => Err(invalid("unknown configuration key".to_string())),
    }
}
