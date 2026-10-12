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
mod toml_layer;
mod values;

pub use keys::{
    CACHE_WARMING_MODES, CODEBLOCK_BORDERS, CustomCatalog, DOUBLE_ESCAPE_ACTIONS,
    FULLSCREEN_EXIT_OUTPUTS, FULLSCREEN_SCROLLBARS, KNOWN_KEYS, MERMAID_MODES, PERMISSION_MODES,
    QUEUE_MODES, SHELL_TOOLS, THINKING_LEVELS, THINKING_VISIBILITIES, TREE_FILTER_MODES,
    TRUST_DEFAULTS, TypedValue, parse_typed,
};

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
    compaction_strategy: Option<String>,
    compaction_reserve_tokens: u64,
    compaction_keep_recent_tokens: u64,
    provider_retry_limit: u64,
    provider_retry_base_delay_ms: u64,
    provider_retry_max_delay_ms: u64,
    tool_timeout_seconds: u64,
    tool_result_limit_bytes: u64,
    tool_max_iterations: u64,
    tool_edit_requires_read: bool,
    cache_noise_floor_tokens: u64,
    // gh #42: off by default (no warming without opt-in); idle may
    // refresh an eligible cache past its TTL.
    cache_warming: String,
    // gh #42: counted-miss notices, off by default like pi.
    cache_miss_notices: bool,
    extensions_log_limit_bytes: u64,
    update_check: Option<bool>,
    ui_fullscreen: Option<bool>,
    ui_quiet_startup: String,
    ui_double_escape_action: String,
    ui_tree_filter_mode: String,
    // gh #80 (pi's `defaultProjectTrust`): the fallback when no stored
    // trust decision exists. User file or environment only.
    trust_default_project: String,
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
    // gh #76: queue delivery modes (pi's names verbatim). Steering
    // defaults to `all` (today's flush); follow-ups to `one-at-a-time`
    // (today's one-per-turn-end). Both differ from pi's defaults by
    // decision: changing them would change long-standing behavior.
    queue_steering_mode: String,
    queue_follow_up_mode: String,
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
    // #227: user-configured custom extension catalogs, merged
    // behind the official catalog with their own provenance.
    extensions_catalogs: Vec<crate::keys::CustomCatalog>,
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
            compaction_strategy: None,
            // 0 = derive the reserve from the threshold fraction (gh
            // #36 phase 1): the stopgap's fraction behavior stays the
            // default, an absolute token budget is opt-in.
            compaction_reserve_tokens: 0,
            compaction_keep_recent_tokens: 20_000,
            provider_retry_limit: 3,
            provider_retry_base_delay_ms: 250,
            provider_retry_max_delay_ms: 60_000,
            tool_timeout_seconds: 120,
            tool_result_limit_bytes: 65536,
            // 0 = unlimited (see `DEFAULT_TOOL_MAX_ITERATIONS`): the
            // runaway guard is opt-in via `tool.max_iterations`.
            tool_max_iterations: DEFAULT_TOOL_MAX_ITERATIONS,
            // gh #117: off is pi parity (blind edits allowed); on keeps
            // the historical read-before-edit staleness guard.
            tool_edit_requires_read: false,
            cache_noise_floor_tokens: 1024,
            cache_warming: "off".to_string(),
            cache_miss_notices: false,
            extensions_log_limit_bytes: 4096,
            extensions_catalogs: Vec::new(),
            update_check: None,
            ui_fullscreen: None,
            ui_quiet_startup: "false".to_string(),
            // gh #132: pi's defaults; the filter documents its no-op
            // (LCA's tree lists sessions, nothing to filter).
            ui_double_escape_action: "tree".to_string(),
            ui_tree_filter_mode: "default".to_string(),
            trust_default_project: "ask".to_string(),
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
            queue_steering_mode: "all".to_string(),
            queue_follow_up_mode: "one-at-a-time".to_string(),
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
            // gh #80 (FR-CFG-7): a project file that granted its own
            // trust would defeat FR-PERM-9 - refused loudly, like any
            // other key the host will not honor.
            if config.sources.get("trust.default_project") == Some(&MergeSource::ProjectFile) {
                return Err(ConfigError::InvalidValue {
                    key: "trust.default_project".to_string(),
                    label: "project file".to_string(),
                    reason: "can only be set in the user file".to_string(),
                });
            }
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
            ("compaction.strategy", TypedValue::Text(v)) => self.compaction_strategy = Some(v),
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
            ("provider.retry_max_delay_ms", TypedValue::Count(v)) => {
                self.provider_retry_max_delay_ms = v;
            }
            ("tool.timeout_seconds", TypedValue::Count(v)) => self.tool_timeout_seconds = v,
            ("tool.result_limit_bytes", TypedValue::Count(v)) => self.tool_result_limit_bytes = v,
            ("tool.max_iterations", TypedValue::Count(v)) => self.tool_max_iterations = v,
            ("cache.noise_floor_tokens", TypedValue::Count(v)) => self.cache_noise_floor_tokens = v,
            ("cache.warming", TypedValue::Text(v)) => self.cache_warming = v,
            ("cache.miss_notices", TypedValue::Bool(v)) => self.cache_miss_notices = v,
            ("extensions.log_limit_bytes", TypedValue::Count(v)) => {
                self.extensions_log_limit_bytes = v
            }
            ("extensions.catalogs", TypedValue::Catalogs(v)) => self.extensions_catalogs = v,
            ("update.check", TypedValue::Bool(v)) => self.update_check = Some(v),
            ("tool.edit_requires_read", TypedValue::Bool(v)) => self.tool_edit_requires_read = v,
            ("ui.fullscreen", TypedValue::Bool(v)) => self.ui_fullscreen = Some(v),
            ("ui.quiet_startup", TypedValue::Text(v)) => self.ui_quiet_startup = v,
            ("ui.double_escape_action", TypedValue::Text(v)) => self.ui_double_escape_action = v,
            ("ui.tree_filter_mode", TypedValue::Text(v)) => self.ui_tree_filter_mode = v,
            ("trust.default_project", TypedValue::Text(v)) => self.trust_default_project = v,
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
            ("queue.steering_mode", TypedValue::Text(v)) => self.queue_steering_mode = v,
            ("queue.follow_up_mode", TypedValue::Text(v)) => self.queue_follow_up_mode = v,
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
