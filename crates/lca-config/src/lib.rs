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
    provider_retry_limit: u64,
    tool_timeout_seconds: u64,
    tool_result_limit_bytes: u64,
    tool_max_iterations: u64,
    cache_noise_floor_tokens: u64,
    extensions_log_limit_bytes: u64,
    update_check: Option<bool>,
    ui_color: ColorMode,
    permissions_proposals: BTreeMap<String, String>,
    sources: BTreeMap<String, MergeSource>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            provider: "openai-compatible".to_string(),
            model: None,
            compaction_threshold: 0.8,
            provider_retry_limit: 3,
            tool_timeout_seconds: 120,
            tool_result_limit_bytes: 65536,
            tool_max_iterations: 50,
            cache_noise_floor_tokens: 1024,
            extensions_log_limit_bytes: 4096,
            update_check: None,
            ui_color: ColorMode::Auto,
            permissions_proposals: BTreeMap::new(),
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

/// Every configuration key, as documented in `docs/configuration.md`.
pub const KNOWN_KEYS: &[&str] = &[
    "provider",
    "model",
    "compaction.threshold",
    "provider.retry_limit",
    "tool.timeout_seconds",
    "tool.result_limit_bytes",
    "tool.max_iterations",
    "cache.noise_floor_tokens",
    "extensions.log_limit_bytes",
    "update.check",
    "ui.color",
];

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

fn parse_typed(key: &str, raw: &str, label: &str) -> Result<TypedValue, ConfigError> {
    let invalid = |reason: String| ConfigError::InvalidValue {
        key: key.to_string(),
        label: label.to_string(),
        reason,
    };
    match key {
        "provider" | "model" => Ok(TypedValue::Text(raw.to_string())),
        "update.check" => raw
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
        "provider.retry_limit"
        | "tool.timeout_seconds"
        | "tool.result_limit_bytes"
        | "tool.max_iterations"
        | "cache.noise_floor_tokens"
        | "extensions.log_limit_bytes" => raw
            .parse::<u64>()
            .map(TypedValue::Count)
            .map_err(|_| invalid(format!("expected a non-negative integer, got `{raw}`"))),
        other => Err(invalid(format!("unknown configuration key `{other}`"))),
    }
}

enum TypedValue {
    Text(String),
    Count(u64),
    Number(f64),
    Bool(bool),
    Color(ColorMode),
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
            "compaction.threshold",
            "provider.retry_limit",
            "tool.timeout_seconds",
            "tool.result_limit_bytes",
            "tool.max_iterations",
            "cache.noise_floor_tokens",
            "extensions.log_limit_bytes",
            "update.check",
            "ui.color",
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
                "update.check" => {
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
                "provider.retry_limit"
                | "tool.timeout_seconds"
                | "tool.result_limit_bytes"
                | "tool.max_iterations"
                | "cache.noise_floor_tokens"
                | "extensions.log_limit_bytes" => {
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
            "compaction.threshold",
            "provider.retry_limit",
            "tool.timeout_seconds",
            "tool.result_limit_bytes",
            "tool.max_iterations",
            "cache.noise_floor_tokens",
            "extensions.log_limit_bytes",
            "update.check",
            "ui.color",
        ] {
            if let Some(value) = table_value(table, key) {
                // `provider` doubles as a section: `[provider] retry_limit = N`
                // cannot coexist with the `provider = "name"` leaf in one TOML
                // document, so a table form carries only its children.
                if key == "provider" && value.is_table() {
                    continue;
                }
                apply(self, key, value.clone())?;
            }
        }
        if let Some(table) = table_value(table, "provider").and_then(toml::Value::as_table)
            && let Some(limit) = table.get("retry_limit")
        {
            apply(self, "provider.retry_limit", limit.clone())?;
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
            ("compaction.threshold", TypedValue::Number(v)) => self.compaction_threshold = v,
            ("provider.retry_limit", TypedValue::Count(v)) => self.provider_retry_limit = v,
            ("tool.timeout_seconds", TypedValue::Count(v)) => self.tool_timeout_seconds = v,
            ("tool.result_limit_bytes", TypedValue::Count(v)) => self.tool_result_limit_bytes = v,
            ("tool.max_iterations", TypedValue::Count(v)) => self.tool_max_iterations = v,
            ("cache.noise_floor_tokens", TypedValue::Count(v)) => self.cache_noise_floor_tokens = v,
            ("extensions.log_limit_bytes", TypedValue::Count(v)) => {
                self.extensions_log_limit_bytes = v
            }
            ("update.check", TypedValue::Bool(v)) => self.update_check = Some(v),
            ("ui.color", TypedValue::Color(v)) => self.ui_color = v,
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

    /// The active provider extension's name.
    pub fn provider(&self) -> &str {
        &self.provider
    }

    /// The active model identifier, or `None` for the provider's default.
    pub fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }

    /// Context-window fraction that triggers compaction (FR-SESS-4).
    pub fn compaction_threshold(&self) -> f64 {
        self.compaction_threshold
    }

    /// Retry attempts for retryable transport errors (FR-CORE-6).
    pub fn provider_retry_limit(&self) -> u64 {
        self.provider_retry_limit
    }

    /// Shell command timeout in seconds (FR-TOOL-5).
    pub fn tool_timeout_seconds(&self) -> u64 {
        self.tool_timeout_seconds
    }

    /// Tool results above this many bytes are truncated (FR-TOOL-7).
    pub fn tool_result_limit_bytes(&self) -> u64 {
        self.tool_result_limit_bytes
    }

    /// Maximum tool calls within one turn (FR-CORE-9).
    pub fn tool_max_iterations(&self) -> u64 {
        self.tool_max_iterations
    }

    /// Cache misses below this token count are not counted (FR-CACHE-3).
    pub fn cache_noise_floor_tokens(&self) -> u64 {
        self.cache_noise_floor_tokens
    }

    /// Extension log messages above this many bytes are truncated (FR-EXT-10).
    pub fn extensions_log_limit_bytes(&self) -> u64 {
        self.extensions_log_limit_bytes
    }

    /// Daily version check on or off (FR-CFG-6); the default follows the mode.
    pub fn update_check(&self, headless: bool) -> bool {
        self.update_check.unwrap_or(!headless)
    }

    /// Terminal color policy (FR-UI-5).
    pub fn ui_color(&self) -> ColorMode {
        self.ui_color
    }

    /// Permission proposals read from a trusted project file (ADR-0006).
    pub fn permissions_proposals(&self) -> &BTreeMap<String, String> {
        &self.permissions_proposals
    }

    /// Every resolved key with its display value and source (FR-CFG-2).
    pub fn resolved(&self) -> impl Iterator<Item = (&str, String, MergeSource)> + '_ {
        let values: BTreeMap<&str, String> = [
            ("provider", self.provider.clone()),
            (
                "model",
                self.model
                    .clone()
                    .unwrap_or_else(|| "<provider default>".to_string()),
            ),
            (
                "compaction.threshold",
                self.compaction_threshold.to_string(),
            ),
            (
                "provider.retry_limit",
                self.provider_retry_limit.to_string(),
            ),
            (
                "tool.timeout_seconds",
                self.tool_timeout_seconds.to_string(),
            ),
            (
                "tool.result_limit_bytes",
                self.tool_result_limit_bytes.to_string(),
            ),
            ("tool.max_iterations", self.tool_max_iterations.to_string()),
            (
                "cache.noise_floor_tokens",
                self.cache_noise_floor_tokens.to_string(),
            ),
            (
                "extensions.log_limit_bytes",
                self.extensions_log_limit_bytes.to_string(),
            ),
            (
                "update.check",
                self.update_check
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "<mode default>".to_string()),
            ),
            (
                "ui.color",
                match self.ui_color {
                    ColorMode::Auto => "auto",
                    ColorMode::Never => "never",
                }
                .to_string(),
            ),
            (
                "permissions.proposals",
                if self.permissions_proposals.is_empty() {
                    "<empty>".to_string()
                } else {
                    format!("{} proposal(s)", self.permissions_proposals.len())
                },
            ),
        ]
        .into();
        values.into_iter().map(move |(key, value)| {
            let source = self
                .sources
                .get(key)
                .copied()
                .unwrap_or(MergeSource::Default);
            (key, value, source)
        })
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
