//! Key family: which model, at what thinking level, over which models.
//!
//! Gh #545 phase 2: moved out of `apply_toml_layer`'s one 612-line closure, so
//! one file owns the keys that name a model and nothing else. `provider` and
//! `thinking` double as section names (`[provider] retry_limit = N`), which the
//! call site resolves before a value reaches here.

use std::collections::BTreeMap;

use crate::keys::{THINKING_LEVELS, csv};
use crate::type_name;
use crate::{ConfigError, TypedValue};

/// The keys this family owns. `toml_layer/mod.rs`'s dispatch table routes each of
/// them here, and `tests/keys_agree.rs` pins that the two lists name one set.
pub(super) const KEYS: &[&str] = &[
    "provider",
    "model",
    "models.thinking_levels",
    "thinking.budgets",
    "models.enabled",
    "thinking",
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
        "provider" | "model" => {
            let text = value
                .as_str()
                .ok_or_else(|| invalid(format!("expected a string, got {}", type_name(value))))?;
            Ok(TypedValue::Text(text.to_string()))
        }
        "models.thinking_levels" => {
            let table = value.as_table().ok_or_else(|| {
                invalid(format!(
                    "expected a table of model = [levels], got {}",
                    type_name(value)
                ))
            })?;
            let mut map = BTreeMap::new();
            for (model, levels) in table {
                let list = levels.as_array().ok_or_else(|| {
                    invalid(format!(
                        "`{model}` must be a list of levels, got {}",
                        type_name(levels)
                    ))
                })?;
                let mut parsed = Vec::new();
                for level in list {
                    let text = level.as_str().ok_or_else(|| {
                        invalid(format!(
                            "`{model}` must be a list of levels, found {}",
                            type_name(level)
                        ))
                    })?;
                    if !THINKING_LEVELS.contains(&text) {
                        return Err(invalid(format!(
                            "expected one of {}, got `{text}` for `{model}`",
                            THINKING_LEVELS.join(", ")
                        )));
                    }
                    parsed.push(text.to_string());
                }
                map.insert(model.clone(), parsed);
            }
            Ok(TypedValue::ThinkingLevels(map))
        }
        "thinking.budgets" => {
            let table = value.as_table().ok_or_else(|| {
                invalid(format!(
                    "expected a table of level = tokens, got {}",
                    type_name(value)
                ))
            })?;
            let mut map = BTreeMap::new();
            for (level, tokens) in table {
                if !["minimal", "low", "medium", "high"].contains(&level.as_str()) {
                    return Err(invalid(format!(
                        "expected one of minimal, low, medium, high, got `{level}`"
                    )));
                }
                let tokens = tokens
                    .as_integer()
                    .and_then(|i| u64::try_from(i).ok())
                    .ok_or_else(|| {
                        invalid(format!(
                            "`{level}` must be a non-negative token count, got {}",
                            type_name(tokens)
                        ))
                    })?;
                map.insert(level.clone(), tokens);
            }
            Ok(TypedValue::Budgets(map))
        }
        "models.enabled" => {
            let list = match value {
                toml::Value::Array(items) => {
                    let mut list = Vec::new();
                    for item in items {
                        let text = item.as_str().ok_or_else(|| {
                            invalid(format!(
                                "expected an array of strings, found {}",
                                type_name(item)
                            ))
                        })?;
                        list.push(text.to_string());
                    }
                    list
                }
                // A single string reads as one pattern, so a
                // hand-written `models.enabled = "zen/*"` is not
                // an error either.
                toml::Value::String(text) => csv(text),
                other => {
                    return Err(invalid(format!(
                        "expected an array of strings, got {}",
                        type_name(other)
                    )));
                }
            };
            Ok(TypedValue::List(list))
        }
        "thinking" => {
            let text = value
                .as_str()
                .ok_or_else(|| invalid(format!("expected a string, got {}", type_name(value))))?;
            if !THINKING_LEVELS.contains(&text) {
                return Err(invalid(format!(
                    "expected one of {}, got `{text}`",
                    THINKING_LEVELS.join(", ")
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
