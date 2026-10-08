//! Reads and merges configuration.
//!
//! Precedence, highest first: command line flags, environment variables, the
//! project file at `.lca/config.toml` (only while the project is trusted),
//! the user file in the platform config directory, built-in defaults
//! (FR-CFG-1). `lca config` prints each resolved value with the source that
//! set it (FR-CFG-2), which is why every layer records its provenance.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

mod keys;
mod values;

pub use keys::{
    CODEBLOCK_BORDERS, DOUBLE_ESCAPE_ACTIONS, FULLSCREEN_EXIT_OUTPUTS, FULLSCREEN_SCROLLBARS,
    KNOWN_KEYS, MERMAID_MODES, PERMISSION_MODES, SHELL_TOOLS, THINKING_LEVELS,
    THINKING_VISIBILITIES, TREE_FILTER_MODES, TypedValue, parse_typed,
};

use keys::csv;

mod keybindings;

pub use keybindings::load_keybindings_file;

/// Where a resolved configuration value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MergeSource {
    /// A command line flag.
    Flag,
    /// An `LCA_`-prefixed environment variable.
    Env,
    /// The project file, `.lca/config.toml`.
    ProjectFile,
    /// The user file in the platform config directory.
    UserFile,
    /// A built-in default.
    Default,
}

impl std::fmt::Display for MergeSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            MergeSource::Flag => "flag",
            MergeSource::Env => "environment",
            MergeSource::ProjectFile => "project file",
            MergeSource::UserFile => "user file",
            MergeSource::Default => "default",
        })
    }
}

/// Terminal color policy (`ui.color`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorMode {
    /// Detect terminal color support.
    Auto,
    /// Never emit color (FR-UI-5).
    Never,
}
impl std::str::FromStr for ColorMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "auto" => Ok(ColorMode::Auto),
            "never" => Ok(ColorMode::Never),
            other => Err(format!("expected `auto` or `never`, got `{other}`")),
        }
    }
}

/// A configuration load failure that names the key and the layer.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// A file could not be read.
    #[error("failed to read {label} {path}: {source}")]
    Read {
        /// Which file layer.
        label: &'static str,
        /// The path.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// A file could not be parsed as TOML.
    #[error("{label} {path} is not valid TOML: {source}")]
    Parse {
        /// Which file layer.
        label: &'static str,
        /// The path.
        path: PathBuf,
        /// The underlying parse error, boxed to keep the error enum small.
        #[source]
        source: Box<toml::de::Error>,
    },
    /// A key held a value of the wrong type.
    #[error("{key} in {label} is invalid: {reason}")]
    InvalidValue {
        /// The dotted key.
        key: String,
        /// Which layer set it.
        label: String,
        /// Why it was rejected.
        reason: String,
    },
}

/// Inputs to a configuration merge. Tests and `lca-cli` build this.
#[derive(Debug, Clone, Default)]
pub struct LoadInput {
    /// Flag values, keyed by dotted configuration key.
    pub flags: BTreeMap<String, String>,
    /// Environment variables (already filtered to the `LCA_` prefix or not;
    /// the loader maps `LCA_TOOL_TIMEOUT_SECONDS` to `tool.timeout_seconds`).
    pub env: BTreeMap<String, String>,
    /// Path to the project file, when one exists.
    pub project_file: Option<PathBuf>,
    /// Whether the user trusts this project (FR-PERM-9).
    pub trusted: bool,
    /// Path to the user file, when one exists.
    pub user_file: Option<PathBuf>,
    /// Whether the process runs headless (FR-CFG-6 defaults).
    pub headless: bool,
}

/// One merged configuration.
#[derive(Debug, Clone)]
pub struct Config {
    provider: String,
    model: Option<String>,
    compaction_threshold: f64,
    compaction_enabled: bool,
    compaction_reserve_tokens: u64,
    compaction_keep_recent_tokens: u64,
    provider_retry_limit: u64,
    provider_retry_base_delay_ms: u64,
    tool_timeout_seconds: u64,
    tool_result_limit_bytes: u64,
    tool_max_iterations: u64,
    tool_edit_requires_read: bool,
    cache_noise_floor_tokens: u64,
    extensions_log_limit_bytes: u64,
    update_check: Option<bool>,
    ui_fullscreen: Option<bool>,
    ui_quiet_startup: String,
    ui_double_escape_action: String,
    ui_tree_filter_mode: String,
    ui_autocomplete_max_visible: u64,
    ui_editor_padding_x: u64,
    ui_output_pad: u64,
    ui_fullscreen_scrollbar: String,
    ui_fullscreen_copy_on_select: bool,
    ui_fullscreen_wheel_lines: String,
    ui_fullscreen_exit_output: String,
    ui_show_hardware_cursor: bool,
    terminal_show_images: bool,
    terminal_image_width_cells: u64,
    terminal_clear_on_shrink: bool,
    terminal_show_progress: bool,
    terminal_hyperlinks: String,
    terminal_images: String,
    terminal_true_color: String,
    images_auto_resize: bool,
    images_block_images: bool,
    markdown_code_block_indent: String,
    markdown_mermaid: String,
    ui_color: ColorMode,
    ui_theme: Option<String>,
    thinking: Option<String>,
    shell_tool: Option<String>,
    shell_path: Option<String>,
    shell_command_prefix: Option<String>,
    permissions_mode: Option<String>,
    thinking_visibility: Option<String>,
    // gh #43: matched skill-text injection is opt-in (default OFF);
    // the catalog advertises either way.
    skills_inject_matched: bool,
    // gh #32: how fenced code blocks are framed; `full` is the shipped
    // look, `horizontal` exists so a copy-paste has no side pipes.
    markdown_codeblock_border: String,
    permissions_proposals: BTreeMap<String, String>,
    // gh #8 (EFG-003): the enabled-model scope (pi's `enabledModels`).
    // Empty = no restriction: every model the provider offers.
    models_enabled: Vec<String>,
    // gh #8 phase 4 (pi's `modelThinkingLevels`): per-model sets of the
    // thinking levels that model accepts; the first entry is its default.
    models_thinking_levels: BTreeMap<String, Vec<String>>,
    // gh #41 (pi's `thinkingBudgets`): per-level token-budget
    // overrides; empty means pi's built-in budgets (see
    // `DEFAULT_THINKING_BUDGETS`).
    thinking_budgets: BTreeMap<String, u64>,
    sources: BTreeMap<String, MergeSource>,
}

/// The default `tool.max_iterations` (docs/configuration.md): **0 =
/// unlimited**, because a fixed round cap is a work cap in disguise and
/// long-horizon tasks need the room (owner issue #19; pi caps nothing).
/// The machinery stays: any positive value re-enables the guard and the
/// notice names it. History: 50, then 100 (the runaway-guard era), then
/// 0 from 2026-10-02. `lca-core::AgentConfig::default` must agree with
/// this value; the consistency test in `lca-cli` enforces it.
pub const DEFAULT_TOOL_MAX_ITERATIONS: u64 = 0;

/// The built-in per-level thinking token budgets (gh #41, pi's
/// `DEFAULT_THINKING_BUDGETS` verbatim): `[thinking.budgets]`
/// overrides per level.
pub const DEFAULT_THINKING_BUDGETS: [(&str, u64); 4] = [
    ("minimal", 1024),
    ("low", 2048),
    ("medium", 8192),
    ("high", 16384),
];

impl Default for Config {
    fn default() -> Self {
        Config {
            provider: "openai-compatible".to_string(),
            model: None,
            compaction_threshold: 0.8,
            compaction_enabled: true,
            // 0 = derive the reserve from the threshold fraction (gh
            // #36 phase 1): the stopgap's fraction behavior stays the
            // default, an absolute token budget is opt-in.
            compaction_reserve_tokens: 0,
            compaction_keep_recent_tokens: 20_000,
            provider_retry_limit: 3,
            provider_retry_base_delay_ms: 250,
            tool_timeout_seconds: 120,
            tool_result_limit_bytes: 65536,
            // 0 = unlimited (see `DEFAULT_TOOL_MAX_ITERATIONS`): the
            // runaway guard is opt-in via `tool.max_iterations`.
            tool_max_iterations: DEFAULT_TOOL_MAX_ITERATIONS,
            // gh #117: off is pi parity (blind edits allowed); on keeps
            // the historical read-before-edit staleness guard.
            tool_edit_requires_read: false,
            cache_noise_floor_tokens: 1024,
            extensions_log_limit_bytes: 4096,
            update_check: None,
            ui_fullscreen: None,
            ui_quiet_startup: "false".to_string(),
            // gh #132: pi's defaults; the filter documents its no-op
            // (LCA's tree lists sessions, nothing to filter).
            ui_double_escape_action: "tree".to_string(),
            ui_tree_filter_mode: "default".to_string(),
            // gh #82: pi's display inventory in LCA naming. Defaults
            // match pi except where noted (show_progress stays on:
            // current behavior).
            ui_autocomplete_max_visible: 5,
            ui_editor_padding_x: 0,
            ui_output_pad: 1,
            ui_fullscreen_scrollbar: "auto".to_string(),
            ui_fullscreen_copy_on_select: true,
            ui_fullscreen_wheel_lines: "auto".to_string(),
            ui_fullscreen_exit_output: "transcript".to_string(),
            ui_show_hardware_cursor: false,
            terminal_show_images: true,
            terminal_image_width_cells: 60,
            terminal_clear_on_shrink: false,
            terminal_show_progress: true,
            terminal_hyperlinks: "auto".to_string(),
            terminal_images: "auto".to_string(),
            terminal_true_color: "auto".to_string(),
            images_auto_resize: true,
            images_block_images: false,
            markdown_code_block_indent: "  ".to_string(),
            markdown_mermaid: "streaming".to_string(),
            ui_color: ColorMode::Auto,
            ui_theme: None,
            thinking: None,
            shell_tool: None,
            shell_path: None,
            shell_command_prefix: None,
            permissions_mode: None,
            thinking_visibility: None,
            skills_inject_matched: false,
            markdown_codeblock_border: "full".to_string(),
            permissions_proposals: BTreeMap::new(),
            models_enabled: Vec::new(),
            models_thinking_levels: BTreeMap::new(),
            thinking_budgets: BTreeMap::new(),
            sources: BTreeMap::new(),
        }
    }
}

fn label_for(source: MergeSource) -> &'static str {
    match source {
        MergeSource::Flag => "flag",
        MergeSource::Env => "environment",
        MergeSource::ProjectFile => "project file",
        MergeSource::UserFile => "user file",
        MergeSource::Default => "default",
    }
}

fn dotted_to_env_key(dotted: &str) -> String {
    format!("LCA_{}", dotted.to_ascii_uppercase().replace('.', "_"))
}
/// Look a dotted key up in a TOML table: literal keys (`"tool.timeout_seconds"`)
/// first, then section form (`[tool] timeout_seconds = ...`). A key path that
/// hits a non-table midway does not exist in section form.
fn table_value<'a>(table: &'a toml::Table, dotted: &str) -> Option<&'a toml::Value> {
    if let Some(value) = table.get(dotted) {
        return Some(value);
    }
    let mut current = table;
    let mut segments = dotted.split('.').peekable();
    while let Some(segment) = segments.next() {
        let value = current.get(segment)?;
        if segments.peek().is_none() {
            return Some(value);
        }
        current = value.as_table()?;
    }
    None
}

/// Split a flag/environment comma list into patterns: trimmed, empties
/// dropped, so `a, b` and `a,b` are one scope.
fn type_name(value: &toml::Value) -> &'static str {
    match value {
        toml::Value::String(..) => "a string",
        toml::Value::Integer(..) => "an integer",
        toml::Value::Float(..) => "a float",
        toml::Value::Boolean(..) => "a boolean",
        toml::Value::Array(..) => "an array",
        toml::Value::Table(..) => "a table",
        toml::Value::Datetime(..) => "a datetime",
    }
}

fn read_table(path: &Path, label: &'static str) -> Result<toml::Table, ConfigError> {
    let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
        label,
        path: path.to_path_buf(),
        source,
    })?;
    text.parse::<toml::Table>()
        .map_err(|source| ConfigError::Parse {
            label,
            path: path.to_path_buf(),
            source: Box::new(source),
        })
}

impl Config {
    /// The built-in defaults, as `lca config` reports them.
    pub fn defaults() -> Self {
        Config::default()
    }

    /// Merge every configured layer (FR-CFG-1).
    pub fn load(input: &LoadInput) -> Result<Config, ConfigError> {
        let mut config = Config::defaults();
        for key in [
            "provider",
            "model",
            "models.enabled",
            "models.thinking_levels",
            "thinking.budgets",
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
            "ui.thinking",
            "skills.inject_matched",
            "markdown.codeblock_border",
        ] {
            config.sources.insert(key.to_string(), MergeSource::Default);
        }

        if let Some(path) = &input.user_file {
            let layer = read_table(path, "user file")?;
            config.apply_toml_layer(&layer, MergeSource::UserFile)?;
        }

        // The project file participates only for a trusted project:
        // FR-PERM-9, ADR-0006.
        if input.trusted
            && let Some(path) = &input.project_file
        {
            let layer = read_table(path, "project file")?;
            config.apply_toml_layer(&layer, MergeSource::ProjectFile)?;
            if let Some(toml::Value::Table(proposals)) =
                table_value(&layer, "permissions.proposals")
            {
                let map = proposals
                    .iter()
                    .filter_map(|(k, v)| v.as_str().map(|note| (k.clone(), note.to_string())))
                    .collect();
                config.permissions_proposals = map;
            }
        }

        for key in KNOWN_KEYS {
            let env_name = dotted_to_env_key(key);
            let Some(raw) = input.env.get(&env_name) else {
                continue;
            };
            let value = parse_typed(key, raw, "environment")?;
            config.apply(key.to_string(), value, MergeSource::Env)?;
        }

        for (key, raw) in &input.flags {
            if key == "permissions.proposals" {
                continue;
            }
            let value = parse_typed(key, raw, "flag")?;
            config.apply(key.clone(), value, MergeSource::Flag)?;
        }

        Ok(config)
    }

    fn apply_toml_layer(
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
                "ui.double_escape_action" | "ui.tree_filter_mode" => {
                    // The domain lives here too, not just in the
                    // flag/env parser: a file value skips that path.
                    let text = value.as_str().ok_or_else(|| {
                        invalid(format!("expected a string, got {}", type_name(&value)))
                    })?;
                    let ok = match key {
                        "ui.double_escape_action" => DOUBLE_ESCAPE_ACTIONS.contains(&text),
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

    fn apply(
        &mut self,
        key: String,
        value: TypedValue,
        source: MergeSource,
    ) -> Result<(), ConfigError> {
        match (key.as_str(), value) {
            ("provider", TypedValue::Text(v)) => self.provider = v,
            ("model", TypedValue::Text(v)) => self.model = Some(v),
            ("models.enabled", TypedValue::List(v)) => self.models_enabled = v,
            ("models.thinking_levels", TypedValue::ThinkingLevels(v)) => {
                self.models_thinking_levels = v
            }
            ("thinking.budgets", TypedValue::Budgets(v)) => self.thinking_budgets = v,
            ("compaction.threshold", TypedValue::Number(v)) => self.compaction_threshold = v,
            ("compaction.enabled", TypedValue::Bool(v)) => self.compaction_enabled = v,
            ("compaction.reserve_tokens", TypedValue::Count(v)) => {
                self.compaction_reserve_tokens = v;
            }
            ("compaction.keep_recent_tokens", TypedValue::Count(v)) => {
                self.compaction_keep_recent_tokens = v;
            }
            ("provider.retry_limit", TypedValue::Count(v)) => self.provider_retry_limit = v,
            ("provider.retry_base_delay_ms", TypedValue::Count(v)) => {
                self.provider_retry_base_delay_ms = v;
            }
            ("tool.timeout_seconds", TypedValue::Count(v)) => self.tool_timeout_seconds = v,
            ("tool.result_limit_bytes", TypedValue::Count(v)) => self.tool_result_limit_bytes = v,
            ("tool.max_iterations", TypedValue::Count(v)) => self.tool_max_iterations = v,
            ("cache.noise_floor_tokens", TypedValue::Count(v)) => self.cache_noise_floor_tokens = v,
            ("extensions.log_limit_bytes", TypedValue::Count(v)) => {
                self.extensions_log_limit_bytes = v
            }
            ("update.check", TypedValue::Bool(v)) => self.update_check = Some(v),
            ("tool.edit_requires_read", TypedValue::Bool(v)) => self.tool_edit_requires_read = v,
            ("ui.fullscreen", TypedValue::Bool(v)) => self.ui_fullscreen = Some(v),
            ("ui.quiet_startup", TypedValue::Text(v)) => self.ui_quiet_startup = v,
            ("ui.double_escape_action", TypedValue::Text(v)) => self.ui_double_escape_action = v,
            ("ui.tree_filter_mode", TypedValue::Text(v)) => self.ui_tree_filter_mode = v,
            ("ui.autocomplete_max_visible", TypedValue::Count(v)) => {
                self.ui_autocomplete_max_visible = v
            }
            ("ui.editor_padding_x", TypedValue::Count(v)) => self.ui_editor_padding_x = v,
            ("ui.output_pad", TypedValue::Count(v)) => self.ui_output_pad = v,
            ("ui.fullscreen_scrollbar", TypedValue::Text(v)) => self.ui_fullscreen_scrollbar = v,
            ("ui.fullscreen_copy_on_select", TypedValue::Bool(v)) => {
                self.ui_fullscreen_copy_on_select = v
            }
            ("ui.fullscreen_wheel_lines", TypedValue::Text(v)) => {
                self.ui_fullscreen_wheel_lines = v
            }
            ("ui.fullscreen_exit_output", TypedValue::Text(v)) => {
                self.ui_fullscreen_exit_output = v
            }
            ("ui.show_hardware_cursor", TypedValue::Bool(v)) => self.ui_show_hardware_cursor = v,
            ("terminal.show_images", TypedValue::Bool(v)) => self.terminal_show_images = v,
            ("terminal.image_width_cells", TypedValue::Count(v)) => {
                self.terminal_image_width_cells = v
            }
            ("terminal.clear_on_shrink", TypedValue::Bool(v)) => self.terminal_clear_on_shrink = v,
            ("terminal.show_progress", TypedValue::Bool(v)) => self.terminal_show_progress = v,
            ("terminal.hyperlinks", TypedValue::Text(v)) => self.terminal_hyperlinks = v,
            ("terminal.images", TypedValue::Text(v)) => self.terminal_images = v,
            ("terminal.true_color", TypedValue::Text(v)) => self.terminal_true_color = v,
            ("images.auto_resize", TypedValue::Bool(v)) => self.images_auto_resize = v,
            ("images.block_images", TypedValue::Bool(v)) => self.images_block_images = v,
            ("markdown.code_block_indent", TypedValue::Text(v)) => {
                self.markdown_code_block_indent = v
            }
            ("markdown.mermaid", TypedValue::Text(v)) => self.markdown_mermaid = v,
            ("ui.color", TypedValue::Color(v)) => self.ui_color = v,
            ("ui.theme", TypedValue::Text(v)) => self.ui_theme = Some(v),
            ("shell.tool", TypedValue::Text(v)) => self.shell_tool = Some(v),
            ("shell.path", TypedValue::Text(v)) => self.shell_path = Some(v),
            ("shell.command_prefix", TypedValue::Text(v)) => self.shell_command_prefix = Some(v),
            ("permissions.mode", TypedValue::Text(v)) => self.permissions_mode = Some(v),
            ("ui.thinking", TypedValue::Text(v)) => self.thinking_visibility = Some(v),
            ("skills.inject_matched", TypedValue::Bool(v)) => self.skills_inject_matched = v,
            ("thinking", TypedValue::Text(v)) => self.thinking = Some(v),
            ("markdown.codeblock_border", TypedValue::Text(v)) => {
                self.markdown_codeblock_border = v
            }
            (other, _) => {
                return Err(ConfigError::InvalidValue {
                    key: other.to_string(),
                    label: label_for(source).to_string(),
                    reason: "unknown configuration key".to_string(),
                });
            }
        }
        self.sources.insert(key, source);
        Ok(())
    }
}

/// The dotted key an `LCA_` environment variable sets, if it is one of the
/// documented keys. Mapping is by enumeration, because underscores cannot be
/// split back into dots unambiguously (`tool.timeout_seconds`).
pub fn env_key_target(key: &str) -> Option<&'static str> {
    KNOWN_KEYS
        .iter()
        .copied()
        .find(|dotted| dotted_to_env_key(dotted) == key)
}

/// Build the `LCA_` environment map from the process environment.
pub fn collect_env() -> BTreeMap<String, String> {
    std::env::vars()
        .filter(|(key, _)| key.starts_with("LCA_"))
        .collect()
}

/// The environment variable that sets `key`, inverse of [`env_key_target`].
pub fn dotted_key_to_env(key: &str) -> String {
    dotted_to_env_key(key)
}
