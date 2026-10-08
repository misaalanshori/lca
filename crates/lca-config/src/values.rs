//! Value accessors for the merged configuration, split from
//! `lib.rs` (the 1,200-line ceiling): every getter plus the resolved
//! inventory. Behavior lives here; merging lives in `lib.rs`.

use super::*;

impl Config {
    /// The active provider extension's name.
    pub fn provider(&self) -> &str {
        &self.provider
    }

    /// The active model identifier, or `None` for the provider's default.
    pub fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }

    /// The enabled-model scope (`models.enabled`): id patterns the
    /// picker's listing and the model cycle are cut to (gh #8, pi's
    /// `enabledModels`). Empty means no restriction - everything the
    /// provider offers is in scope.
    pub fn models_enabled(&self) -> &[String] {
        &self.models_enabled
    }

    /// Replace the enabled-model scope at runtime (gh #204): the
    /// checklist swaps this cell so the cycle reads the live scope.
    /// Empty means no restriction.
    pub fn set_models_enabled(&mut self, ids: Vec<String>) {
        self.models_enabled = ids;
    }

    /// The levels `model` accepts (`models.thinking_levels`), or `None`
    /// when the map does not name it - which means no restriction.
    pub fn allowed_thinking_levels(&self, model: &str) -> Option<&[String]> {
        self.models_thinking_levels.get(model).map(Vec::as_slice)
    }

    /// The first allowed level of `model`: its default, the entry a
    /// switch to it applies (gh #8 phase 4; pi's per-model default
    /// beating the global one).
    pub fn default_thinking_for(&self, model: &str) -> Option<&str> {
        self.allowed_thinking_levels(model)
            .and_then(|levels| levels.first())
            .map(String::as_str)
    }

    /// The level `model` may run on: `requested` clamped into its set -
    /// outside it becomes the model's default (the first allowed level) -
    /// with no set meaning no restriction. `None` is not a level: unset
    /// is the provider's choice, and it stays unset.
    pub fn clamp_thinking(&self, requested: Option<&str>, model: &str) -> Option<String> {
        let requested = requested?;
        let Some(allowed) = self.allowed_thinking_levels(model) else {
            return Some(requested.to_string());
        };
        if allowed.is_empty() {
            return Some(requested.to_string());
        }
        if allowed.iter().any(|level| level == requested) {
            return Some(requested.to_string());
        }
        allowed.first().cloned()
    }

    /// What a switch to `model` runs on: the model's configured default
    /// when it has one (pi's precedence), else the current level clamped
    /// into what the new model accepts.
    pub fn switch_thinking(&self, current: Option<&str>, model: &str) -> Option<String> {
        if let Some(default) = self.default_thinking_for(model) {
            return Some(default.to_string());
        }
        self.clamp_thinking(current, model)
    }

    /// Context-window fraction that triggers compaction (FR-SESS-4).
    pub fn compaction_threshold(&self) -> f64 {
        self.compaction_threshold
    }

    /// Whether automatic compaction runs (gh #36 phase 1).
    pub fn compaction_enabled(&self) -> bool {
        self.compaction_enabled
    }

    /// Whether `edit` demands a prior fresh `read` (gh #117): off is
    /// pi parity (the model edits right after `grep`), on keeps LCA's
    /// staleness guard. The `/settings` row cycles it live.
    pub fn tool_edit_requires_read(&self) -> bool {
        self.tool_edit_requires_read
    }

    /// Absolute token reserve (gh #36 phase 1): 0 derives it from the
    /// threshold fraction, matching the stopgap's default behavior.
    pub fn compaction_reserve_tokens(&self) -> u64 {
        self.compaction_reserve_tokens
    }

    /// Recent tokens kept verbatim past the cut point (gh #36 phase 1).
    pub fn compaction_keep_recent_tokens(&self) -> u64 {
        self.compaction_keep_recent_tokens
    }

    /// Retry attempts for retryable transport errors (FR-CORE-6).
    pub fn provider_retry_limit(&self) -> u64 {
        self.provider_retry_limit
    }

    /// Token budget for one thinking level (gh #41, pi's
    /// `thinkingBudgets` with its built-ins): the override wins, an
    /// unset level or `off` budgets nothing, and `xhigh`/`max` spend
    /// `high`'s budget (pi clamps extended levels the same way).
    pub fn budget_for_level(&self, level: Option<&str>) -> Option<u64> {
        budget_for_level_in(&self.thinking_budgets, level)
    }

    /// The override map itself (the agent loop carries it; the
    /// request builder resolves per request).
    pub fn thinking_budgets(&self) -> &std::collections::BTreeMap<String, u64> {
        &self.thinking_budgets
    }

    /// First retry delay in milliseconds (gh #83, pi's
    /// `retry.baseDelayMs`); doubles per attempt (FR-CORE-6).
    pub fn provider_retry_base_delay_ms(&self) -> u64 {
        self.provider_retry_base_delay_ms
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

    /// Fullscreen (alt-screen) renderer (`ui.fullscreen`), or `None` when
    /// neither the config nor the caller names one (gh #112).
    pub fn ui_fullscreen(&self) -> Option<bool> {
        self.ui_fullscreen
    }

    /// The startup header level (gh #131): `"false"` shows version +
    /// resources, `"header"` keeps the version line only (pi's value),
    /// `"true"` hides the header entirely.
    pub fn ui_quiet_startup(&self) -> &str {
        &self.ui_quiet_startup
    }

    /// What Esc Esc with an empty editor does (gh #132, pi's
    /// `doubleEscapeAction`): `tree`, `fork`, or `none`.
    pub fn ui_double_escape_action(&self) -> &str {
        &self.ui_double_escape_action
    }

    /// Pi's `treeFilterMode` domain, accepted config-error-free (gh
    /// #132): currently inert by documentation - LCA's `/tree` lists
    /// sessions, not messages, so there is nothing to filter. Known
    /// but dead is honest; silently dropping the key would not be.
    pub fn ui_tree_filter_mode(&self) -> &str {
        &self.ui_tree_filter_mode
    }

    /// Autocomplete popup rows, 3-20 (gh #82, pi's
    /// `autocompleteMaxVisible`).
    pub fn ui_autocomplete_max_visible(&self) -> u64 {
        self.ui_autocomplete_max_visible
    }

    /// Editor horizontal padding cells, 0-3 (gh #82, pi's
    /// `editorPaddingX`).
    pub fn ui_editor_padding_x(&self) -> u64 {
        self.ui_editor_padding_x
    }

    /// Transcript left margin, 0|1 (gh #82, pi's `outputPad`).
    pub fn ui_output_pad(&self) -> u64 {
        self.ui_output_pad
    }

    /// Fullscreen scrollbar behavior (gh #82, pi's
    /// `fullscreenScrollbar`): `auto`, `always`, or `hidden`.
    pub fn ui_fullscreen_scrollbar(&self) -> &str {
        &self.ui_fullscreen_scrollbar
    }

    /// Copy on text selection in fullscreen (gh #82, pi's
    /// `fullscreenCopyOnSelect`).
    pub fn ui_fullscreen_copy_on_select(&self) -> bool {
        self.ui_fullscreen_copy_on_select
    }

    /// Fullscreen wheel lines (gh #82, pi's
    /// `fullscreenWheelScrollLines`): `auto` (three lines) or 1-100.
    pub fn ui_fullscreen_wheel_lines(&self) -> &str {
        &self.ui_fullscreen_wheel_lines
    }

    /// Fullscreen exit output (gh #82, pi's `fullscreenExitOutput`):
    /// `transcript` leaves the transcript, `resume-hint` names the
    /// resume command.
    pub fn ui_fullscreen_exit_output(&self) -> &str {
        &self.ui_fullscreen_exit_output
    }

    /// Use the terminal cursor (gh #82, pi's `showHardwareCursor`):
    /// accepted but inert - LCA paints the caret into the row and
    /// never trusts the terminal cursor.
    pub fn ui_show_hardware_cursor(&self) -> bool {
        self.ui_show_hardware_cursor
    }

    /// Display inline images (gh #82, pi's `terminal.showImages`).
    pub fn terminal_show_images(&self) -> bool {
        self.terminal_show_images
    }

    /// Inline image width cap in cells (gh #82, pi's
    /// `terminal.imageWidthCells`).
    pub fn terminal_image_width_cells(&self) -> u64 {
        self.terminal_image_width_cells
    }

    /// Clear empty rows on shrink (gh #82, pi's
    /// `terminal.clearOnShrink`): accepted but inert - LCA has no
    /// shrink-clearing pass.
    pub fn terminal_clear_on_shrink(&self) -> bool {
        self.terminal_clear_on_shrink
    }

    /// OSC 9;4 taskbar progress (gh #82, pi's
    /// `terminal.showTerminalProgress`): on is current behavior.
    pub fn terminal_show_progress(&self) -> bool {
        self.terminal_show_progress
    }

    /// Hyperlink override (gh #82, pi's `terminal.hyperlinks`):
    /// accepted but inert - `LCA_HYPERLINKS=1|0` already forces the
    /// ladder, and detection lives below config's reach.
    pub fn terminal_hyperlinks(&self) -> &str {
        &self.terminal_hyperlinks
    }

    /// Image protocol override (gh #82, pi's `terminal.images`):
    /// accepted but inert - the kitty/iTerm2 auto-detection stands.
    pub fn terminal_images(&self) -> &str {
        &self.terminal_images
    }

    /// True-color override (gh #82, pi's `terminal.trueColor`):
    /// accepted but inert - `ui.color` (auto|never) owns color.
    pub fn terminal_true_color(&self) -> &str {
        &self.terminal_true_color
    }

    /// Downscale images before sending (gh #82, pi's
    /// `images.autoResize`).
    pub fn images_auto_resize(&self) -> bool {
        self.images_auto_resize
    }

    /// Block images to models (gh #82, pi's `images.blockImages`):
    /// accepted but inert - there is no send-gate yet; viewing gates
    /// through `terminal.show_images`.
    pub fn images_block_images(&self) -> bool {
        self.images_block_images
    }

    /// Code block content indent (gh #82, pi's
    /// `markdown.codeBlockIndent`).
    pub fn markdown_code_block_indent(&self) -> &str {
        &self.markdown_code_block_indent
    }

    /// Mermaid rendering (gh #82, pi's `markdown.mermaid`):
    /// `off` keeps fences raw, `final` renders settled messages,
    /// `streaming` renders mid-stream too.
    pub fn markdown_mermaid(&self) -> &str {
        &self.markdown_mermaid
    }

    /// Daily version check on or off (FR-CFG-6); the default follows the mode.
    pub fn update_check(&self, headless: bool) -> bool {
        self.update_check.unwrap_or(!headless)
    }

    /// Terminal color policy (FR-UI-5).
    pub fn ui_color(&self) -> ColorMode {
        self.ui_color
    }

    /// The configured theme (S5): a built-in name, a custom theme's name, or
    /// `auto` for the detected terminal scheme. `None` is `auto`.
    pub fn ui_theme(&self) -> Option<&str> {
        self.ui_theme.as_deref()
    }

    /// The configured thinking level (`thinking`), or `None` for the
    /// provider's own default (pi's "unset").
    pub fn thinking(&self) -> Option<&str> {
        self.thinking.as_deref()
    }

    /// The configured shell tool (`shell.tool`), or `None` for `auto`
    /// (ADR-0041).
    pub fn shell_tool(&self) -> Option<&str> {
        self.shell_tool.as_deref()
    }

    /// An exact interpreter path (`shell.path`), when set (ADR-0041).
    pub fn shell_path(&self) -> Option<&str> {
        self.shell_path.as_deref()
    }

    /// A prefix prepended to every shell command (`shell.command_prefix`),
    /// when set (gh #133, pi's `shellCommandPrefix`).
    pub fn shell_command_prefix(&self) -> Option<&str> {
        self.shell_command_prefix.as_deref()
    }

    /// `markdown.codeblock_border` (gh #32): `full` (the shipped
    /// four-sided frame), `horizontal` (bars only, so a terminal copy
    /// has no side pipes), or `none` (bare lines). Validated at load;
    /// the renderer owns the shapes themselves (`lca-tui`'s
    /// `CodeBlockBorder`).
    pub fn markdown_codeblock_border(&self) -> &str {
        &self.markdown_codeblock_border
    }

    /// `permissions.mode`: `ask` (default) or `yolo` (ADR-0042).
    pub fn permissions_mode(&self) -> Option<&str> {
        self.permissions_mode.as_deref()
    }

    /// `ui.thinking`: `snippet` (default), `full`, or `hidden` (R6).
    /// Distinct from [`Config::thinking`], which is the effort level; a
    /// dotted `thinking.*` key cannot exist beside a `thinking = "..."`
    /// string in TOML, which is why this one lives under `ui`.
    pub fn thinking_visibility(&self) -> Option<&str> {
        self.thinking_visibility.as_deref()
    }

    /// Whether matched skill text injects into the prompt (gh #43).
    /// Default off: the catalog advertises either way.
    pub fn skills_inject_matched(&self) -> bool {
        self.skills_inject_matched
    }

    /// Permission proposals read from a trusted project file (ADR-0006).
    pub fn permissions_proposals(&self) -> &BTreeMap<String, String> {
        &self.permissions_proposals
    }

    /// Where one key's value came from: anything but `Default` means the
    /// user set it (flag, env, or file). Callers that change behavior on
    /// "explicitly configured" use this rather than comparing values.
    pub fn source_of(&self, key: &str) -> MergeSource {
        self.sources
            .get(key)
            .copied()
            .unwrap_or(MergeSource::Default)
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
                "models.enabled",
                if self.models_enabled.is_empty() {
                    "<all>".to_string()
                } else {
                    self.models_enabled.join(", ")
                },
            ),
            (
                "models.thinking_levels",
                if self.models_thinking_levels.is_empty() {
                    "<unset>".to_string()
                } else {
                    format!("{} model(s)", self.models_thinking_levels.len())
                },
            ),
            (
                "compaction.threshold",
                self.compaction_threshold.to_string(),
            ),
            ("compaction.enabled", self.compaction_enabled.to_string()),
            (
                "compaction.reserve_tokens",
                self.compaction_reserve_tokens.to_string(),
            ),
            (
                "compaction.keep_recent_tokens",
                self.compaction_keep_recent_tokens.to_string(),
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
                "tool.edit_requires_read",
                self.tool_edit_requires_read.to_string(),
            ),
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
                "ui.fullscreen",
                self.ui_fullscreen
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "<unset>".to_string()),
            ),
            ("ui.quiet_startup", self.ui_quiet_startup.clone()),
            (
                "ui.double_escape_action",
                self.ui_double_escape_action.clone(),
            ),
            ("ui.tree_filter_mode", self.ui_tree_filter_mode.clone()),
            (
                "ui.autocomplete_max_visible",
                self.ui_autocomplete_max_visible.to_string(),
            ),
            ("ui.editor_padding_x", self.ui_editor_padding_x.to_string()),
            ("ui.output_pad", self.ui_output_pad.to_string()),
            (
                "ui.fullscreen_scrollbar",
                self.ui_fullscreen_scrollbar.clone(),
            ),
            (
                "ui.fullscreen_copy_on_select",
                self.ui_fullscreen_copy_on_select.to_string(),
            ),
            (
                "ui.fullscreen_wheel_lines",
                self.ui_fullscreen_wheel_lines.clone(),
            ),
            (
                "ui.fullscreen_exit_output",
                self.ui_fullscreen_exit_output.clone(),
            ),
            (
                "ui.show_hardware_cursor",
                self.ui_show_hardware_cursor.to_string(),
            ),
            (
                "terminal.show_images",
                self.terminal_show_images.to_string(),
            ),
            (
                "terminal.image_width_cells",
                self.terminal_image_width_cells.to_string(),
            ),
            (
                "terminal.clear_on_shrink",
                self.terminal_clear_on_shrink.to_string(),
            ),
            (
                "terminal.show_progress",
                self.terminal_show_progress.to_string(),
            ),
            ("terminal.hyperlinks", self.terminal_hyperlinks.clone()),
            ("terminal.images", self.terminal_images.clone()),
            ("terminal.true_color", self.terminal_true_color.clone()),
            ("images.auto_resize", self.images_auto_resize.to_string()),
            ("images.block_images", self.images_block_images.to_string()),
            (
                "markdown.code_block_indent",
                self.markdown_code_block_indent.clone(),
            ),
            ("markdown.mermaid", self.markdown_mermaid.clone()),
            (
                "ui.color",
                match self.ui_color {
                    ColorMode::Auto => "auto",
                    ColorMode::Never => "never",
                }
                .to_string(),
            ),
            (
                "thinking",
                self.thinking
                    .clone()
                    .unwrap_or_else(|| "<provider default>".to_string()),
            ),
            (
                "ui.theme",
                self.ui_theme.clone().unwrap_or_else(|| "auto".to_string()),
            ),
            (
                "shell.tool",
                self.shell_tool
                    .clone()
                    .unwrap_or_else(|| "auto".to_string()),
            ),
            (
                "shell.path",
                self.shell_path
                    .clone()
                    .unwrap_or_else(|| "<ladder>".to_string()),
            ),
            (
                "shell.command_prefix",
                self.shell_command_prefix
                    .clone()
                    .unwrap_or_else(|| "<unset>".to_string()),
            ),
            (
                "permissions.mode",
                self.permissions_mode
                    .clone()
                    .unwrap_or_else(|| "ask".to_string()),
            ),
            (
                "markdown.codeblock_border",
                self.markdown_codeblock_border.clone(),
            ),
            (
                "ui.thinking",
                self.thinking_visibility
                    .clone()
                    .unwrap_or_else(|| "snippet".to_string()),
            ),
            (
                "skills.inject_matched",
                self.skills_inject_matched.to_string(),
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

/// Token budget for one thinking level over an explicit override map
/// (gh #41): the pure core of [`Config::budget_for_level`], so the
/// agent loop (which carries only the map) resolves per request
/// without reimplementing the table.
pub fn budget_for_level_in(
    overrides: &std::collections::BTreeMap<String, u64>,
    level: Option<&str>,
) -> Option<u64> {
    let level = match level {
        None | Some("off") => return None,
        Some("xhigh") | Some("max") => "high",
        Some(level) => level,
    };
    if !["minimal", "low", "medium", "high"].contains(&level) {
        return None;
    }
    let builtin = super::DEFAULT_THINKING_BUDGETS
        .iter()
        .find(|(name, _)| *name == level)
        .map(|(_, budget)| *budget)
        .unwrap_or(0);
    Some(overrides.get(level).copied().unwrap_or(builtin))
}
