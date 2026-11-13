//! Key family: a key whose value is a number - a count, a range-bounded integer,
//! or `compaction.threshold`'s fraction.

use crate::type_name;
use crate::{ConfigError, TypedValue};

/// The keys this family owns. `toml_layer/mod.rs`'s dispatch table routes each of
/// them here, and `tests/keys_agree.rs` pins that the two lists name one set.
pub(super) const KEYS: &[&str] = &[
    "ui.autocomplete_max_visible",
    "ui.editor_padding_x",
    "ui.output_pad",
    "terminal.image_width_cells",
    "compaction.threshold",
    "provider.retry_limit",
    "provider.retry_base_delay_ms",
    "provider.retry_max_delay_ms",
    "tool.timeout_seconds",
    "tool.result_limit_bytes",
    "tool.max_iterations",
    "cache.noise_floor_tokens",
    "extensions.log_limit_bytes",
    "compaction.reserve_tokens",
    "compaction.keep_recent_tokens",
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
        "ui.autocomplete_max_visible" => {
            let count = value.as_integer().and_then(|i| u64::try_from(i).ok());
            match count {
                Some(n) if (3..=20).contains(&n) => Ok(TypedValue::Count(n)),
                _ => Err(invalid(format!(
                    "expected an integer 3-20, got {}",
                    type_name(value)
                ))),
            }
        }
        "ui.editor_padding_x" => {
            let count = value.as_integer().and_then(|i| u64::try_from(i).ok());
            match count {
                Some(n) if n <= 3 => Ok(TypedValue::Count(n)),
                _ => Err(invalid(format!(
                    "expected an integer 0-3, got {}",
                    type_name(value)
                ))),
            }
        }
        "ui.output_pad" => {
            let count = value.as_integer().and_then(|i| u64::try_from(i).ok());
            match count {
                Some(0) | Some(1) => Ok(TypedValue::Count(count.unwrap_or(1))),
                _ => Err(invalid(format!(
                    "expected 0 or 1, got {}",
                    type_name(value)
                ))),
            }
        }
        "terminal.image_width_cells" => {
            let count = value.as_integer().and_then(|i| u64::try_from(i).ok());
            match count {
                Some(n) if n >= 1 => Ok(TypedValue::Count(n)),
                _ => Err(invalid(format!(
                    "expected a positive integer, got {}",
                    type_name(value)
                ))),
            }
        }
        "compaction.threshold" => {
            let number = value
                .as_float()
                .or_else(|| value.as_integer().map(|i| i as f64))
                .ok_or_else(|| invalid(format!("expected a number, got {}", type_name(value))))?;
            if !(0.0..=1.0).contains(&number) {
                return Err(invalid(format!(
                    "expected a fraction in 0.0..=1.0, got {number}"
                )));
            }
            Ok(TypedValue::Number(number))
        }
        "provider.retry_limit"
        | "provider.retry_base_delay_ms"
        | "provider.retry_max_delay_ms"
        | "tool.timeout_seconds"
        | "tool.result_limit_bytes"
        | "tool.max_iterations"
        | "cache.noise_floor_tokens"
        | "extensions.log_limit_bytes"
        | "compaction.reserve_tokens"
        | "compaction.keep_recent_tokens" => {
            let count = value
                .as_integer()
                .and_then(|i| u64::try_from(i).ok())
                .ok_or_else(|| {
                    invalid(format!(
                        "expected a non-negative integer, got {}",
                        type_name(value)
                    ))
                })?;
            Ok(TypedValue::Count(count))
        }
        // Unreachable by construction - `KEYS` names exactly the arms above, and
        // the dispatch routes only those keys here (`tests/keys_agree.rs` pins the
        // two lists against each other) - but it carries the closure's own copy so
        // that a mis-route reports what an unknown key always has, rather than a
        // message nobody has ever seen.
        _ => Err(invalid("unknown configuration key".to_string())),
    }
}
