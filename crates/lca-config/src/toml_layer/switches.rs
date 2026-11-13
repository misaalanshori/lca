//! Key family: a key whose value is a boolean, and nothing else.
//!
//! Nine of these keys share one arm because they share one rule: a non-boolean
//! is a refusal, never a coercion.

use crate::type_name;
use crate::{ConfigError, TypedValue};

/// The keys this family owns. `toml_layer/mod.rs`'s dispatch table routes each of
/// them here, and `tests/keys_agree.rs` pins that the two lists name one set.
pub(super) const KEYS: &[&str] = &[
    "ui.fullscreen_copy_on_select",
    "ui.show_hardware_cursor",
    "terminal.show_images",
    "terminal.clear_on_shrink",
    "terminal.show_progress",
    "images.auto_resize",
    "images.block_images",
    "update.check",
    "compaction.enabled",
    "ui.fullscreen",
    "cache.miss_notices",
    "tool.edit_requires_read",
    "skills.inject_matched",
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
        "ui.fullscreen_copy_on_select"
        | "ui.show_hardware_cursor"
        | "terminal.show_images"
        | "terminal.clear_on_shrink"
        | "terminal.show_progress"
        | "images.auto_resize"
        | "images.block_images" => {
            let flag = value
                .as_bool()
                .ok_or_else(|| invalid(format!("expected a boolean, got {}", type_name(value))))?;
            Ok(TypedValue::Bool(flag))
        }
        "update.check"
        | "compaction.enabled"
        | "ui.fullscreen"
        | "cache.miss_notices"
        | "tool.edit_requires_read" => {
            let flag = value
                .as_bool()
                .ok_or_else(|| invalid(format!("expected a boolean, got {}", type_name(value))))?;
            Ok(TypedValue::Bool(flag))
        }
        "skills.inject_matched" => {
            let flag = value
                .as_bool()
                .ok_or_else(|| invalid(format!("expected a boolean, got {}", type_name(value))))?;
            Ok(TypedValue::Bool(flag))
        }
        // Unreachable by construction - `KEYS` names exactly the arms above, and
        // the dispatch routes only those keys here (`tests/keys_agree.rs` pins the
        // two lists against each other) - but it carries the closure's own copy so
        // that a mis-route reports what an unknown key always has, rather than a
        // message nobody has ever seen.
        _ => Err(invalid("unknown configuration key".to_string())),
    }
}
