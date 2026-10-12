//! The TOML file layer (ceiling split from `lib.rs`): reading one
//! TOML table into typed values. Behavior unchanged; the table is the
//! single source, these are its file-reading methods.

use std::collections::BTreeMap;

use super::keys::csv;
use super::keys::{
    CACHE_WARMING_MODES, CODEBLOCK_BORDERS, DOUBLE_ESCAPE_ACTIONS, FULLSCREEN_EXIT_OUTPUTS,
    FULLSCREEN_SCROLLBARS, MERMAID_MODES, PERMISSION_MODES, QUEUE_MODES, SHELL_TOOLS,
    THINKING_LEVELS, THINKING_VISIBILITIES, TREE_FILTER_MODES, TRUST_DEFAULTS,
};
use super::{ColorMode, Config, ConfigError, MergeSource, TypedValue};

use super::{label_for, table_value, type_name};

impl Config {
    pub(super) fn apply_toml_layer(
        &mut self,
        table: &toml::Table,
        source: MergeSource,
    ) -> Result<(), ConfigError> {
        let label = label_for(source);
        let apply = |this: &mut Self, key: &str, value: toml::Value| -> Result<(), ConfigError> {
            let invalid = |reason: String| ConfigError::InvalidValue {
                key: key.to_string(),
                label: label.to_string(),
                reason,
            };
            match key {
                "provider" | "model" => {
                    let text = value.as_str().ok_or_else(|| {
                        invalid(format!("expected a string, got {}", type_name(&value)))
                    })?;
                    this.apply(key.to_string(), TypedValue::Text(text.to_string()), source)?;
                }
                "models.thinking_levels" => {
                    let table = value.as_table().ok_or_else(|| {
                        invalid(format!(
                            "expected a table of model = [levels], got {}",
                            type_name(&value)
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
                    this.apply(key.to_string(), TypedValue::ThinkingLevels(map), source)?;
                }
                "extensions.catalogs" => {
                    let list = value.as_array().ok_or_else(|| {
                        invalid(format!(
                            "expected a list of catalog tables, got {}",
                            type_name(&value)
                        ))
                    })?;
                    let mut catalogs = Vec::new();
                    for entry in list {
                        let table = entry.as_table().ok_or_else(|| {
                            invalid(format!(
                                "each catalog must be a table, got {}",
                                type_name(entry)
                            ))
                        })?;
                        let field = |name: &str| {
                            table
                                .get(name)
                                .and_then(|value| value.as_str())
                                .map(str::to_string)
                                .ok_or_else(|| invalid(format!("each catalog needs `{name}`")))
                        };
                        catalogs.push(crate::keys::CustomCatalog {
                            id: field("id")?,
                            name: field("name")?,
                            url: field("url")?,
                        });
                    }
                    this.apply(key.to_string(), TypedValue::Catalogs(catalogs), source)?;
                }
                "thinking.budgets" => {
                    let table = value.as_table().ok_or_else(|| {
                        invalid(format!(
                            "expected a table of level = tokens, got {}",
                            type_name(&value)
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
                    this.apply(key.to_string(), TypedValue::Budgets(map), source)?;
                }
                "models.enabled" => {
                    let list = match &value {
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
                    this.apply(key.to_string(), TypedValue::List(list), source)?;
                }
                "trust.default_project"
                | "ui.double_escape_action"
                | "ui.tree_filter_mode"
                | "cache.warming" => {
                    // The domain lives here too, not just in the
                    // flag/env parser: a file value skips that path.
                    let text = value.as_str().ok_or_else(|| {
                        invalid(format!("expected a string, got {}", type_name(&value)))
                    })?;
                    let ok = match key {
                        "ui.double_escape_action" => DOUBLE_ESCAPE_ACTIONS.contains(&text),
                        "trust.default_project" => TRUST_DEFAULTS.contains(&text),
                        "cache.warming" => CACHE_WARMING_MODES.contains(&text),
                        _ => TREE_FILTER_MODES.contains(&text),
                    };
                    if !ok {
                        return Err(invalid(format!("unexpected {key} value `{text}`")));
                    }
                    this.apply(key.to_string(), TypedValue::Text(text.to_string()), source)?;
                }
                "ui.autocomplete_max_visible" => {
                    let count = value.as_integer().and_then(|i| u64::try_from(i).ok());
                    match count {
                        Some(n) if (3..=20).contains(&n) => {
                            this.apply(key.to_string(), TypedValue::Count(n), source)?;
                        }
                        _ => {
                            return Err(invalid(format!(
                                "expected an integer 3-20, got {}",
                                type_name(&value)
                            )));
                        }
                    }
                }
                "ui.editor_padding_x" => {
                    let count = value.as_integer().and_then(|i| u64::try_from(i).ok());
                    match count {
                        Some(n) if n <= 3 => {
                            this.apply(key.to_string(), TypedValue::Count(n), source)?;
                        }
                        _ => {
                            return Err(invalid(format!(
                                "expected an integer 0-3, got {}",
                                type_name(&value)
                            )));
                        }
                    }
                }
                "ui.output_pad" => {
                    let count = value.as_integer().and_then(|i| u64::try_from(i).ok());
                    match count {
                        Some(0) | Some(1) => {
                            this.apply(
                                key.to_string(),
                                TypedValue::Count(count.unwrap_or(1)),
                                source,
                            )?;
                        }
                        _ => {
                            return Err(invalid(format!(
                                "expected 0 or 1, got {}",
                                type_name(&value)
                            )));
                        }
                    }
                }
                "ui.fullscreen_scrollbar" => {
                    let text = value.as_str().ok_or_else(|| {
                        invalid(format!("expected a string, got {}", type_name(&value)))
                    })?;
                    if !FULLSCREEN_SCROLLBARS.contains(&text) {
                        return Err(invalid(format!(
                            "expected {}, got `{text}`",
                            FULLSCREEN_SCROLLBARS.join(", ")
                        )));
                    }
                    this.apply(key.to_string(), TypedValue::Text(text.to_string()), source)?;
                }
                "ui.fullscreen_wheel_lines" => {
                    let text = match &value {
                        toml::Value::String(text) => text.clone(),
                        toml::Value::Integer(n) => n.to_string(),
                        _ => {
                            return Err(invalid(format!(
                                "expected auto or an integer 1-100, got {}",
                                type_name(&value)
                            )));
                        }
                    };
                    let ok =
                        text == "auto" || text.parse::<u64>().is_ok_and(|n| (1..=100).contains(&n));
                    if !ok {
                        return Err(invalid(format!(
                            "expected auto or an integer 1-100, got `{text}`"
                        )));
                    }
                    this.apply(key.to_string(), TypedValue::Text(text), source)?;
                }
                "ui.fullscreen_exit_output" => {
                    let text = value.as_str().ok_or_else(|| {
                        invalid(format!("expected a string, got {}", type_name(&value)))
                    })?;
                    if !FULLSCREEN_EXIT_OUTPUTS.contains(&text) {
                        return Err(invalid(format!(
                            "expected {}, got `{text}`",
                            FULLSCREEN_EXIT_OUTPUTS.join(", ")
                        )));
                    }
                    this.apply(key.to_string(), TypedValue::Text(text.to_string()), source)?;
                }
                "terminal.image_width_cells" => {
                    let count = value.as_integer().and_then(|i| u64::try_from(i).ok());
                    match count {
                        Some(n) if n >= 1 => {
                            this.apply(key.to_string(), TypedValue::Count(n), source)?;
                        }
                        _ => {
                            return Err(invalid(format!(
                                "expected a positive integer, got {}",
                                type_name(&value)
                            )));
                        }
                    }
                }
                "terminal.hyperlinks" | "terminal.true_color" => {
                    let text = match &value {
                        toml::Value::Boolean(flag) => flag.to_string(),
                        toml::Value::String(text) => text.clone(),
                        _ => {
                            return Err(invalid(format!(
                                "expected true, false, or auto, got {}",
                                type_name(&value)
                            )));
                        }
                    };
                    if !["true", "false", "auto"].contains(&text.as_str()) {
                        return Err(invalid(format!(
                            "expected true, false, or auto, got `{text}`"
                        )));
                    }
                    this.apply(key.to_string(), TypedValue::Text(text), source)?;
                }
                "terminal.images" => {
                    let text = match &value {
                        toml::Value::Boolean(false) => "false".to_string(),
                        toml::Value::String(text) => text.clone(),
                        _ => {
                            return Err(invalid(format!(
                                "expected kitty, iterm2, auto, or false, got {}",
                                type_name(&value)
                            )));
                        }
                    };
                    if !["kitty", "iterm2", "auto", "false"].contains(&text.as_str()) {
                        return Err(invalid(format!(
                            "expected kitty, iterm2, auto, or false, got `{text}`"
                        )));
                    }
                    this.apply(key.to_string(), TypedValue::Text(text), source)?;
                }
                "markdown.code_block_indent" => {
                    let text = value.as_str().ok_or_else(|| {
                        invalid(format!("expected a string, got {}", type_name(&value)))
                    })?;
                    if text.contains('\n') || text.len() > 8 {
                        return Err(invalid("expected a short single-line indent".to_string()));
                    }
                    this.apply(key.to_string(), TypedValue::Text(text.to_string()), source)?;
                }
                "markdown.mermaid" => {
                    let text = value.as_str().ok_or_else(|| {
                        invalid(format!("expected a string, got {}", type_name(&value)))
                    })?;
                    if !MERMAID_MODES.contains(&text) {
                        return Err(invalid(format!(
                            "expected {}, got `{text}`",
                            MERMAID_MODES.join(", ")
                        )));
                    }
                    this.apply(key.to_string(), TypedValue::Text(text.to_string()), source)?;
                }
                "ui.fullscreen_copy_on_select"
                | "ui.show_hardware_cursor"
                | "terminal.show_images"
                | "terminal.clear_on_shrink"
                | "terminal.show_progress"
                | "images.auto_resize"
                | "images.block_images" => {
                    let flag = value.as_bool().ok_or_else(|| {
                        invalid(format!("expected a boolean, got {}", type_name(&value)))
                    })?;
                    this.apply(key.to_string(), TypedValue::Bool(flag), source)?;
                }
                "ui.quiet_startup" => {
                    // bool | "header": TOML true/false or the string.
                    let text = match &value {
                        toml::Value::Boolean(flag) => flag.to_string(),
                        toml::Value::String(text) if text == "header" => text.clone(),
                        _ => {
                            return Err(invalid(format!(
                                "expected true, false, or \"header\", got {}",
                                type_name(&value)
                            )));
                        }
                    };
                    this.apply(key.to_string(), TypedValue::Text(text), source)?;
                }
                "update.check"
                | "compaction.enabled"
                | "ui.fullscreen"
                | "cache.miss_notices"
                | "tool.edit_requires_read" => {
                    let flag = value.as_bool().ok_or_else(|| {
                        invalid(format!("expected a boolean, got {}", type_name(&value)))
                    })?;
                    this.apply(key.to_string(), TypedValue::Bool(flag), source)?;
                }
                "compaction.threshold" => {
                    let number = value
                        .as_float()
                        .or_else(|| value.as_integer().map(|i| i as f64))
                        .ok_or_else(|| {
                            invalid(format!("expected a number, got {}", type_name(&value)))
                        })?;
                    if !(0.0..=1.0).contains(&number) {
                        return Err(invalid(format!(
                            "expected a fraction in 0.0..=1.0, got {number}"
                        )));
                    }
                    this.apply(key.to_string(), TypedValue::Number(number), source)?;
                }
                "compaction.strategy" => {
                    let text = value.as_str().ok_or_else(|| {
                        invalid(format!("expected a string, got {}", type_name(&value)))
                    })?;
                    if text.trim().is_empty() {
                        return Err(invalid(
                            "expected an extension name, got an empty string".to_string(),
                        ));
                    }
                    this.apply(key.to_string(), TypedValue::Text(text.to_string()), source)?;
                }
                "ui.color" => {
                    let text = value.as_str().ok_or_else(|| {
                        invalid(format!("expected a string, got {}", type_name(&value)))
                    })?;
                    let color: ColorMode = text.parse().map_err(invalid)?;
                    this.apply(key.to_string(), TypedValue::Color(color), source)?;
                }
                "ui.theme" | "shell.path" | "shell.command_prefix" => {
                    let text = value.as_str().ok_or_else(|| {
                        invalid(format!("expected a string, got {}", type_name(&value)))
                    })?;
                    this.apply(key.to_string(), TypedValue::Text(text.to_string()), source)?;
                }
                "shell.tool" => {
                    let text = value.as_str().ok_or_else(|| {
                        invalid(format!("expected a string, got {}", type_name(&value)))
                    })?;
                    if !SHELL_TOOLS.contains(&text) {
                        return Err(invalid(format!(
                            "expected one of {}, got `{text}`",
                            SHELL_TOOLS.join(", ")
                        )));
                    }
                    this.apply(key.to_string(), TypedValue::Text(text.to_string()), source)?;
                }
                "ui.thinking" => {
                    let text = value.as_str().ok_or_else(|| {
                        invalid(format!("expected a string, got {}", type_name(&value)))
                    })?;
                    if !THINKING_VISIBILITIES.contains(&text) {
                        return Err(invalid(format!(
                            "expected one of {}, got `{text}`",
                            THINKING_VISIBILITIES.join(", ")
                        )));
                    }
                    this.apply(key.to_string(), TypedValue::Text(text.to_string()), source)?;
                }
                "skills.inject_matched" => {
                    let flag = value.as_bool().ok_or_else(|| {
                        invalid(format!("expected a boolean, got {}", type_name(&value)))
                    })?;
                    this.apply(key.to_string(), TypedValue::Bool(flag), source)?;
                }
                "markdown.codeblock_border" => {
                    let text = value.as_str().ok_or_else(|| {
                        invalid(format!("expected a string, got {}", type_name(&value)))
                    })?;
                    if !CODEBLOCK_BORDERS.contains(&text) {
                        return Err(invalid(format!(
                            "expected one of {}, got `{text}`",
                            CODEBLOCK_BORDERS.join(", ")
                        )));
                    }
                    this.apply(key.to_string(), TypedValue::Text(text.to_string()), source)?;
                }
                "permissions.mode" => {
                    let text = value.as_str().ok_or_else(|| {
                        invalid(format!("expected a string, got {}", type_name(&value)))
                    })?;
                    if !PERMISSION_MODES.contains(&text) {
                        return Err(invalid(format!(
                            "expected one of {}, got `{text}`",
                            PERMISSION_MODES.join(", ")
                        )));
                    }
                    this.apply(key.to_string(), TypedValue::Text(text.to_string()), source)?;
                }
                "queue.steering_mode" | "queue.follow_up_mode" => {
                    let text = value.as_str().ok_or_else(|| {
                        invalid(format!("expected a string, got {}", type_name(&value)))
                    })?;
                    if !QUEUE_MODES.contains(&text) {
                        return Err(invalid(format!(
                            "expected one of {}, got `{text}`",
                            QUEUE_MODES.join(", ")
                        )));
                    }
                    this.apply(key.to_string(), TypedValue::Text(text.to_string()), source)?;
                }
                "thinking" => {
                    let text = value.as_str().ok_or_else(|| {
                        invalid(format!("expected a string, got {}", type_name(&value)))
                    })?;
                    if !THINKING_LEVELS.contains(&text) {
                        return Err(invalid(format!(
                            "expected one of {}, got `{text}`",
                            THINKING_LEVELS.join(", ")
                        )));
                    }
                    this.apply(key.to_string(), TypedValue::Text(text.to_string()), source)?;
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
                                type_name(&value)
                            ))
                        })?;
                    this.apply(key.to_string(), TypedValue::Count(count), source)?;
                }
                other => {
                    return Err(ConfigError::InvalidValue {
                        key: other.to_string(),
                        label: label.to_string(),
                        reason: "unknown configuration key".to_string(),
                    });
                }
            }
            Ok(())
        };
        for key in [
            "provider",
            "model",
            "models.enabled",
            "models.thinking_levels",
            "thinking.budgets",
            "compaction.threshold",
            "compaction.enabled",
            "compaction.strategy",
            "compaction.reserve_tokens",
            "compaction.keep_recent_tokens",
            "provider.retry_limit",
            "provider.retry_base_delay_ms",
            "provider.retry_max_delay_ms",
            "tool.timeout_seconds",
            "tool.result_limit_bytes",
            "tool.max_iterations",
            "tool.edit_requires_read",
            "cache.noise_floor_tokens",
            "cache.warming",
            "cache.miss_notices",
            "extensions.log_limit_bytes",
            "extensions.catalogs",
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
            "queue.steering_mode",
            "queue.follow_up_mode",
            "trust.default_project",
            "ui.thinking",
            "skills.inject_matched",
            "markdown.codeblock_border",
        ] {
            if let Some(value) = table_value(table, key) {
                // `provider` doubles as a section: `[provider] retry_limit = N`
                // cannot coexist with the `provider = "name"` leaf in one TOML
                // document, so a table form carries only its children.
                // `thinking` doubles the same way for `[thinking.budgets]`.
                if (key == "provider" || key == "thinking") && value.is_table() {
                    continue;
                }
                apply(self, key, value.clone())?;
            }
        }
        if let Some(table) = table_value(table, "provider").and_then(toml::Value::as_table) {
            if let Some(limit) = table.get("retry_limit") {
                apply(self, "provider.retry_limit", limit.clone())?;
            }
            if let Some(delay) = table.get("retry_base_delay_ms") {
                apply(self, "provider.retry_base_delay_ms", delay.clone())?;
            }
        }
        Ok(())
    }
}
