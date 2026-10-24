//! Display formatting and the model-choice effects (S1): the model
//! picker's text and switch, the session age label, and the stats story.
//! Pure functions over records and store reads - no terminal, no loop.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use lca_protocol::Record;
use lca_session::{Session, SessionStore, ViewMode};

#[cfg(test)]
pub(super) use super::model_rows::model_picker_text;
pub(super) use super::model_rows::{model_effect_on, model_label, model_rows};
#[cfg(test)]
use lca_protocol::CommandEffect;

// gh #402: declared here rather than in `mod.rs` so that file stays untouched
// (`hooks.rs` and `attach.rs` are Worker 2's, and `mod.rs` sits at the ceiling).
#[path = "cache_cause/mod.rs"]
pub(super) mod cache_cause;

/// A short relative age (`now`, `5m`, `3h`, `2d`, `3w`, `2mo`, `1y`), pi's
/// session-row format (`selectors-large.md`).
pub(super) fn age_label(now_ms: u64, then_ms: u64) -> String {
    let seconds = now_ms.saturating_sub(then_ms) / 1000;
    match seconds {
        0..=59 => "now".to_string(),
        60..=3599 => format!("{}m", seconds / 60),
        3600..=86_399 => format!("{}h", seconds / 3600),
        86_400..=604_799 => format!("{}d", seconds / 86_400),
        604_800..=2_591_999 => format!("{}w", seconds / 604_800),
        2_592_000..=31_535_999 => format!("{}mo", seconds / 2_592_000),
        _ => format!("{}y", seconds / 31_536_000),
    }
}

/// A counted cache miss's notice text, if there is news (gh #42, gh #402):
/// `None` when notices are off or no *new cause* has landed.
///
/// gh #402 makes the notice name **why**. Before it, the text said only "N tokens
/// re-billed", so a user watching the footer's ratio fall had no way to learn that
/// a skill had matched or that their project instructions had changed. The cause is
/// read from the signals the measurement already recorded - see
/// [`cache_cause::attribute`], the single place that decides, so the notice and
/// `/session` cannot tell two stories about one miss.
///
/// **Once per cause, not once per turn.** `seen` holds the causes already
/// reported: a repeat is silent, and a cause not yet reported re-arms.
/// The notice `/persona` reports after applying a bundle (gh #412 design point 3).
///
/// It names the bundle as **applied** - model, level, loadout - rather than a diff
/// against the previous state. A diff reads as "nothing happened" when the persona
/// already matches what was set, which is the wrong thing to tell someone who just
/// asked for it.
pub(super) fn persona_notice(bundle: &lca_ui::PersonaBundle) -> String {
    format!(
        "persona {}: {}",
        bundle.name,
        lca_ui::chat_persona::summary(bundle)
    )
}

/// The stats story (FR-UI-19): the same numbers the footer accumulates,
impl super::Ui {
    /// The `cache_miss_notice` hook (gh #42, gh #402).
    ///
    /// Lives here rather than inline in `hooks.rs` so that file keeps its
    /// 1,200-line budget, and because the closure's state is the cause set this
    /// module owns.
    pub(super) fn cache_miss_notice_hook(self: &Arc<Self>) -> lca_ui::CacheMissNotice {
        // Gh #402: causes already reported, not a miss count - a count cannot
        // tell "same divergence again" from "a different cause".
        let seen = Arc::new(Mutex::new(BTreeSet::new()));
        let ui = Arc::clone(self);
        Arc::new(move || {
            let config = crate::lock(&ui.config);
            let enabled = config.cache_miss_notices();
            let floor = config.cache_noise_floor_tokens();
            drop(config);
            let session = crate::lock(&ui.current_session).clone();
            let mut seen = seen.lock().unwrap_or_else(|poison| poison.into_inner());
            miss_notice_for(&ui.store, &session, floor, &mut seen, enabled)
        })
    }
}

pub(super) fn miss_notice_for(
    store: &SessionStore,
    session: &Session,
    floor: u64,
    seen: &mut BTreeSet<cache_cause::Cause>,
    enabled: bool,
) -> Option<String> {
    if !enabled {
        return None;
    }
    let read = store.resolved(session, ViewMode::Display).ok()?;
    let misses = lca_session::collect_cache_misses(&read.records, floor);
    let newest = misses.last()?;
    let cause = cache_cause::attribute(&read.records, newest, cache_ttl_ms());
    if !seen.insert(cause) {
        return None;
    }
    let occurrences = misses
        .iter()
        .filter(|miss| cache_cause::attribute(&read.records, miss, cache_ttl_ms()) == cause)
        .count();
    let occurrences = u64::try_from(occurrences).unwrap_or(u64::MAX);
    Some(cache_cause::notice_text(
        cause,
        newest.missed_tokens,
        newest.missed_cost,
        occurrences,
    ))
}

/// The prompt-cache TTL the idle-expiry attribution compares against.
///
/// Zero means "unknown", which disables that branch rather than guessing: the
/// provider's declared lifetime lives behind the model handle this display path
/// does not hold, and attributing a miss to expiry without one would be exactly
/// the invented cause [`cache_cause::attribute`] exists to avoid.
fn cache_ttl_ms() -> u64 {
    0
}
/// The stats story (FR-UI-19): the same numbers the footer accumulates,
/// with per-model cost and cache waste.
pub(crate) fn session_stats(store: &SessionStore, session: &Session) -> String {
    let Ok(read) = store.read_with(session, ViewMode::Display) else {
        return "statistics unavailable".to_string();
    };
    let mut messages = 0usize;
    let mut input = 0u64;
    let mut output = 0u64;
    let mut cache_read = 0u64;
    let mut cache_write = 0u64;
    let mut cost = 0.0f64;
    // FR-UI-19: tokens per model, keyed by `provider/model`.
    let mut per_model: std::collections::BTreeMap<String, (u64, u64)> =
        std::collections::BTreeMap::new();
    for record in &read.records {
        match record {
            Record::User { .. } => messages += 1,
            Record::Assistant {
                model,
                provider,
                usage,
                ..
            } => {
                messages += 1;
                if let Some(usage) = usage {
                    input += usage.input;
                    output += usage.output;
                    cache_read += usage.cache_read;
                    cache_write += usage.cache_write;
                    cost += usage.cost;
                    let entry = per_model
                        .entry(model_label_for(provider.as_deref(), model.as_deref()))
                        .or_insert((0, 0));
                    // Cache reads/writes are billed as input on most
                    // endpoints, so they count toward the input side.
                    entry.0 += usage.input + usage.cache_read + usage.cache_write;
                    entry.1 += usage.output;
                }
            }
            _ => {}
        }
    }
    // Gh #42: refresh usage counts toward session totals (pi's
    // rule), bucketed by its own provider/model when named.
    for record in &read.records {
        if let Record::Usage {
            provider,
            model,
            usage,
            ..
        } = record
        {
            input += usage.input;
            output += usage.output;
            cache_read += usage.cache_read;
            cache_write += usage.cache_write;
            cost += usage.cost;
            let entry = per_model
                .entry(model_label_for(provider.as_deref(), model.as_deref()))
                .or_insert((0, 0));
            entry.0 += usage.input + usage.cache_read + usage.cache_write;
            entry.1 += usage.output;
        }
    }
    let waste = lca_session::compute_cache_waste(&read.records, 1024);
    // OpenAI-shaped endpoints report no pricing, so cost is 0; printing
    // `$0.0000` would read as "this was free" rather than "unknown".
    let cost_part = if cost > 0.0 {
        format!(", cost ${cost:.4}")
    } else {
        String::new()
    };
    let waste_cost = if waste.missed_cost > 0.0 {
        format!(" / ${:.4}", waste.missed_cost)
    } else {
        String::new()
    };
    // Only break down by model when more than one was used; a single-model
    // session would just repeat the total.
    let per_model_part = if per_model.len() > 1 {
        let parts: Vec<String> = per_model
            .iter()
            .map(|(model, (i, o))| format!("{model}: in {i}, out {o}"))
            .collect();
        format!("; by model: {}", parts.join("; "))
    } else {
        String::new()
    };
    format!(
        "{messages} messages, in {input} tokens (cache read {cache_read}, cache write {cache_write}), \
         out {output} tokens{cost_part}; cache waste {} tokens{waste_cost} across {} misses{per_model_part}",
        waste.missed_tokens, waste.miss_count
    )
}

/// `provider/model` when both are known, else whichever is, else `unknown`.
fn model_label_for(provider: Option<&str>, model: Option<&str>) -> String {
    match (provider, model) {
        (Some(provider), Some(model)) => format!("{provider}/{model}"),
        (_, Some(model)) => model.to_string(),
        (Some(provider), None) => provider.to_string(),
        _ => "unknown".to_string(),
    }
}

/// The `/settings` selector rows (gh #30, sectioned gh #174):
/// every scalar key with its live value and winning source, re-read
/// per call. Split from `hooks.rs` (the 1,200-line ceiling).
/// The friendly label and one-line help for a settings key (gh
/// #235): the picker paints these, never the raw dotted key. New
/// keys fall back to a title-cased last segment with no help, so an
/// inventoried key is never blank.
pub(super) fn setting_meta(key: &str) -> (String, String) {
    let known = |label: &str, description: &str| (label.to_string(), description.to_string());
    match key {
        "ui.theme" => known(
            "Theme",
            "Color palette: auto, dark, light, plain, system, or a file",
        ),
        "ui.color" => known("Color", "SGR color output: auto or never"),
        "ui.fullscreen" => known(
            "Display mode",
            "Terminal scrollback or app-owned fullscreen",
        ),
        "ui.quiet_startup" => known(
            "Quiet startup",
            "Startup header level: full, header only, or hidden",
        ),
        "ui.autocomplete_max_visible" => {
            known("Completion rows", "Autocomplete popup rows, 3 to 20")
        }
        "ui.editor_padding_x" => known(
            "Editor padding",
            "Horizontal padding inside the prompt editor",
        ),
        "ui.output_pad" => known("Output margin", "Assistant transcript left margin, 0 or 1"),
        "markdown.codeblock_border" => known(
            "Code border",
            "Fenced code frame: full, horizontal, or none",
        ),
        "thinking" => known(
            "Thinking effort",
            "Reasoning effort level the model is asked for",
        ),
        "ui.thinking" => known(
            "Thinking display",
            "Reasoning block display: snippet, full, or hidden",
        ),
        "permissions.mode" => known("Permissions", "Approval mode for tool and network prompts"),
        "compaction.enabled" => known(
            "Auto compaction",
            "Compact context automatically when it fills",
        ),
        "compaction.threshold" => known(
            "Compaction threshold",
            "Context fraction that triggers compaction",
        ),
        "compaction.reserve_tokens" => {
            known("Compaction reserve", "Token reserve kept past the cut")
        }
        "compaction.keep_recent_tokens" => {
            known("Keep recent", "Recent tokens kept verbatim past the cut")
        }
        "shell.tool" => known("Shell", "Shell interpreter for the shell tool"),
        "shell.path" => known(
            "Shell path",
            "Exact interpreter path, winning over shell choice",
        ),
        "shell.command_prefix" => known("Shell prefix", "Prefix prepended to every shell command"),
        "tool.timeout_seconds" => {
            known("Shell timeout", "Shell command timeout in seconds when set")
        }
        "tool.result_limit_bytes" => {
            known("Result limit", "Tool output cap before truncation marks")
        }
        "tool.max_iterations" => known("Max iterations", "Tool calls per turn; 0 is unlimited"),
        "provider.retry_limit" => {
            known("Retry limit", "Retry attempts on retryable network errors")
        }
        "tool.edit_requires_read" => {
            known("Edit needs read", "Edits refuse without a prior fresh read")
        }
        "ui.double_escape_action" => known(
            "Double escape",
            "What Esc Esc opens: tree, fork, or nothing",
        ),
        "ui.tree_filter_mode" => known("Tree filter", "The /tree navigator opening view"),
        "ui.fullscreen_scrollbar" => {
            known("Scrollbar", "Fullscreen scrollbar: auto, always, or hidden")
        }
        "ui.fullscreen_copy_on_select" => known(
            "Copy on select",
            "Copy fullscreen selections to the clipboard",
        ),
        "ui.fullscreen_wheel_lines" => {
            known("Wheel lines", "Fullscreen lines per mouse-wheel tick")
        }
        "ui.fullscreen_exit_output" => known("Exit output", "What leaving fullscreen prints"),
        "terminal.show_images" => known("Show images", "Display inline images in the transcript"),
        "terminal.image_width_cells" => known("Image width", "Inline image width cap in cells"),
        "terminal.show_progress" => known("Progress bar", "Taskbar progress around turns"),
        "images.auto_resize" => known("Auto resize", "Resize pasted images to fit"),
        "markdown.code_block_indent" => known("Code indent", "Prefix spaces on fenced code lines"),
        "markdown.mermaid" => known("Mermaid", "Mermaid diagram rendering mode"),
        "cache.noise_floor_tokens" => known(
            "Cache noise floor",
            "Cache misses below this are not counted",
        ),
        "cache.warming" => known(
            "Cache warming",
            "Idle prompt-cache refresh: off, or idle past the TTL",
        ),
        "cache.miss_notices" => known(
            "Cache miss notices",
            "Notice when a counted cache miss lands",
        ),
        "extensions.log_limit_bytes" => known("Extension log cap", "Extension log truncation size"),
        "update.check" => known("Update check", "Daily background version check"),
        _ => {
            let label = key.rsplit('.').next().unwrap_or(key).replace('_', " ");
            let mut label: Vec<char> = label.chars().collect();
            if let Some(first) = label.first_mut() {
                first.make_ascii_uppercase();
            }
            (label.into_iter().collect(), String::new())
        }
    }
}

/// Pi's `fullscreenWheelScrollLines` presets
/// (`settings-selector.ts`, `1, 2, 3, 5, 10`).
///
/// Gh #397: a numeric row needs a preset list to be a *cycle* at all. The list is
/// only the offered stops - the picker unions the value the file holds, so a
/// hand-set `4` is reachable too rather than being clobbered.
const WHEEL_LINE_PRESETS: &[&str] = &["1", "2", "3", "5", "10"];

/// Pi's `imageWidthCells` presets (`settings-selector.ts`, `60, 80, 120`).
const IMAGE_WIDTH_PRESETS: &[&str] = &["60", "80", "120"];

pub(super) fn settings_rows(
    ui: &Arc<super::Ui>,
    data: std::path::PathBuf,
) -> Option<lca_ui::SettingsRows> {
    let ui = ui.clone();
    Some(Arc::new(move || {
        use lca_config::{PERMISSION_MODES, SHELL_TOOLS, THINKING_VISIBILITIES};
        use lca_ui::SettingRow;
        // The winning layer per key (FR-CFG-2's column) -
        // re-read now, not from the startup snapshot: the
        // selector's own last write has to show up as the
        // layer that won. `Chat::settings_rows` mirrors the
        // three keys whose session value can outrun the file.
        let config =
            crate::load_config_flags(&ui.cwd, &crate::lock(&ui.grants), false, false, &ui.flags)
                .unwrap_or_else(|_| crate::lock(&ui.config).clone());
        let resolved: std::collections::BTreeMap<String, (String, lca_config::MergeSource)> =
            config
                .resolved()
                .map(|(key, value, source)| (key.to_string(), (value, source)))
                .collect();
        // Gh #397: the project layer on its own, so a row can say whether this
        // project inherits the key, turns it on, or turns it off. Merged
        // configuration cannot answer that - it only says what won - and
        // without it a hand-written project key is a one-way door: the picker
        // can write one but cannot see that it is there.
        //
        // Read only while the project is trusted, because FR-PERM-9 says an
        // untrusted file is inert. So an untrusted project's key never shows
        // as an override the picker offers to change; it shows in the source
        // column as whatever actually won, which is the truth.
        //
        // gh #481: and only when the project file is a *different file* from the
        // user file. From `$HOME` they are one file, so every key the user set
        // would show up as a project override the picker offers to remove - and
        // removing it would rewrite the user's own configuration. There is no
        // project layer in that case, which is also what the loader concludes
        // (the two paths are compared the same way in both places).
        let project_config = ui.cwd.join(".lca").join("config.toml");
        let user_config = crate::config_file();
        let project_layer = crate::lock(&ui.grants)
            .is_trusted_here(&ui.cwd)
            .then(|| {
                (!crate::project_ext::same_path(&project_config, &user_config))
                    .then(|| lca_config::layers::read_layer(&project_config))
                    .flatten()
            })
            .flatten()
            .unwrap_or_default();
        let row = |section: &str, key: &str, values: &[&str]| {
            let (value, source) = match key {
                // The one setting with two homes (gh #112):
                // a persisted `ui.json` wins when present
                // (FR-UI-21) and says so in the source
                // column; else the config file's
                // `ui.fullscreen` (or the default) shows
                // with the layer that won it.
                "ui.fullscreen" => match super::hooks::read_ui_json(&data) {
                    super::hooks::UiJson::Fullscreen(persisted) => {
                        (persisted.to_string(), "ui.json".to_string())
                    }
                    super::hooks::UiJson::Absent | super::hooks::UiJson::Malformed(_) => resolved
                        .get(key)
                        .map(|(value, source)| (value.clone(), source.to_string()))
                        .unwrap_or_else(|| ("<unset>".to_string(), "default".to_string())),
                },
                _ => resolved
                    .get(key)
                    .map(|(value, source)| (value.clone(), source.to_string()))
                    .unwrap_or_else(|| ("<unset>".to_string(), "default".to_string())),
            };
            let (label, description) = setting_meta(key);
            // Gh #397: only a two-valued on/off row has a tri-state to show;
            // an enum or free-text key has no "off" for a project to mean.
            let project = {
                let on = values.iter().copied().find(|value| *value == "true");
                let off = values.iter().copied().find(|value| *value == "false");
                match (on, off) {
                    (Some(on), Some(off)) => Some(lca_ui::ProjectOverride::from_value(
                        project_layer.get(key).map(String::as_str),
                        on,
                        off,
                    )),
                    _ => None,
                }
            };
            SettingRow {
                section: section.to_string(),
                key: key.to_string(),
                label,
                description,
                value,
                // Gh #218: substituted values may be secrets.
                masked: config.substituted(key),
                source,
                values: values.iter().map(|value| value.to_string()).collect(),
                project,
            }
        };
        // The categorized inventory (gh #174): every scalar
        // key `lca config` owns, grouped under dividers -
        // pi's list is flat, ours is sectioned. `provider`,
        // `model`, and `models.enabled` keep their own
        // pickers; table keys (`models.thinking_levels`,
        // `permissions.proposals`) have no row shape.
        use lca_config::{
            CACHE_WARMING_MODES, CODEBLOCK_BORDERS, FULLSCREEN_EXIT_OUTPUTS, FULLSCREEN_SCROLLBARS,
            MERMAID_MODES,
        };
        vec![
            row("Display & Appearance", "ui.theme", &[]),
            row("Display & Appearance", "ui.color", &["auto", "never"]),
            row("Display & Appearance", "ui.fullscreen", &["false", "true"]),
            row(
                "Display & Appearance",
                "ui.quiet_startup",
                &["false", "true", "header"],
            ),
            row("Display & Appearance", "ui.autocomplete_max_visible", &[]),
            row("Display & Appearance", "ui.editor_padding_x", &[]),
            row("Display & Appearance", "ui.output_pad", &["1", "0"]),
            row(
                "Display & Appearance",
                "markdown.codeblock_border",
                CODEBLOCK_BORDERS,
            ),
            row("Model & Reasoning", "thinking", &[]),
            row("Model & Reasoning", "ui.thinking", THINKING_VISIBILITIES),
            row(
                "Permissions & Security",
                "permissions.mode",
                PERMISSION_MODES,
            ),
            row(
                "Context & Compaction",
                "compaction.enabled",
                &["false", "true"],
            ),
            row("Context & Compaction", "compaction.threshold", &[]),
            row("Context & Compaction", "compaction.reserve_tokens", &[]),
            row("Context & Compaction", "compaction.keep_recent_tokens", &[]),
            row("Terminal & Execution", "shell.tool", SHELL_TOOLS),
            row("Terminal & Execution", "shell.path", &[]),
            row("Terminal & Execution", "shell.command_prefix", &[]),
            row("Terminal & Execution", "tool.timeout_seconds", &[]),
            row("Terminal & Execution", "tool.result_limit_bytes", &[]),
            row("Terminal & Execution", "tool.max_iterations", &[]),
            row("Terminal & Execution", "provider.retry_limit", &[]),
            row(
                "Terminal & Execution",
                "tool.edit_requires_read",
                &["false", "true"],
            ),
            row(
                "Terminal & Execution",
                "ui.double_escape_action",
                lca_config::DOUBLE_ESCAPE_ACTIONS,
            ),
            row(
                "Terminal & Execution",
                "ui.tree_filter_mode",
                lca_config::TREE_FILTER_MODES,
            ),
            row(
                "Terminal & Execution",
                "ui.fullscreen_scrollbar",
                FULLSCREEN_SCROLLBARS,
            ),
            row(
                "Terminal & Execution",
                "ui.fullscreen_copy_on_select",
                &["true", "false"],
            ),
            // Gh #397: the numeric rows cycle pi's presets, so a value the
            // presets do not contain is a *stop on the cycle* rather than a
            // free-text edit. That is the issue's own example
            // (`fullscreenWheelScrollLines: 4` against `1, 2, 3, 5, 10`), and
            // without a preset list here the row never takes the cycle path at
            // all - found in a live pane, where `→` opened the editor instead.
            row(
                "Terminal & Execution",
                "ui.fullscreen_wheel_lines",
                WHEEL_LINE_PRESETS,
            ),
            row(
                "Terminal & Execution",
                "ui.fullscreen_exit_output",
                FULLSCREEN_EXIT_OUTPUTS,
            ),
            row(
                "Terminal & Execution",
                "terminal.show_images",
                &["true", "false"],
            ),
            row(
                "Terminal & Execution",
                "terminal.image_width_cells",
                IMAGE_WIDTH_PRESETS,
            ),
            row(
                "Terminal & Execution",
                "terminal.show_progress",
                &["true", "false"],
            ),
            row(
                "Terminal & Execution",
                "images.auto_resize",
                &["true", "false"],
            ),
            row("Terminal & Execution", "markdown.code_block_indent", &[]),
            row("Terminal & Execution", "markdown.mermaid", MERMAID_MODES),
            row("Data & Updates", "cache.noise_floor_tokens", &[]),
            row("Data & Updates", "cache.warming", CACHE_WARMING_MODES),
            row("Data & Updates", "cache.miss_notices", &["false", "true"]),
            row("Data & Updates", "extensions.log_limit_bytes", &[]),
            row("Data & Updates", "update.check", &["false", "true"]),
        ]
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lca_protocol::ModelInfo;

    fn models(ids: &[&str]) -> Vec<ModelInfo> {
        ids.iter()
            .map(|id| ModelInfo {
                id: (*id).to_string(),
                name: format!("Model {id}"),
                context_window: 100_000,
                max_tokens: 8_192,
                extras: Default::default(),
            })
            .collect()
    }

    // Verifies: gh #31 - the row names the service that will bill the
    // call: the model's own label out of `extras` (the carrier the WIT
    // record already had) beats the caller's provider name, and a
    // provider that carries no per-model identity keeps the old label.
    #[test]
    fn a_models_own_label_replaces_the_crate_name_in_the_row() {
        let offered = vec![
            ModelInfo {
                id: "mimo-v2.6-flash".to_string(),
                name: "mimo-v2.6-flash".to_string(),
                context_window: 100_000,
                max_tokens: 0,
                extras: [("label".to_string(), "opencode-go".to_string())].into(),
            },
            ModelInfo {
                id: "plain-model".to_string(),
                name: "plain-model".to_string(),
                context_window: 100_000,
                max_tokens: 0,
                extras: Default::default(),
            },
        ];
        let rows = model_rows(&offered, "openai-compatible");
        assert_eq!(rows[0].id, "mimo-v2.6-flash");
        assert_eq!(
            rows[0].label, "mimo-v2.6-flash (opencode-go)",
            "the service that will bill the call names the row"
        );
        // Gh #396: the provider is a field so the search can use it - and it is the
        // *provider* (`openai-compatible`), not the display label `opencode-go`,
        // which is a brand name on the row and not who bills the call.
        assert_eq!(
            rows[0].provider, "openai-compatible",
            "the searchable provider is who bills, not the display label"
        );
        assert_eq!(rows[1].id, "plain-model");
        assert_eq!(
            rows[1].label, "plain-model (openai-compatible)",
            "a model with no per-model identity falls back to the provider"
        );
        // Selection is untouched by any of it: the id stays raw.
        for row in &rows {
            let picker = lca_ui::ModelPicker::new(vec![row.clone()]);
            assert_eq!(picker.selected_model(), Some(row.id.as_str()));
        }
    }

    // Verifies: G2 (issue #3's safety half) - the `model (provider)` label
    // is display-only, asserted in both directions: the row decorates the
    // label and leaves the id raw, and what the picker hands to selection
    // is that raw id. `weird (v2)` is in the row on purpose - the old
    // label-strip cut at the first ` (` and would have returned `weird`.
    #[test]
    fn the_picker_label_maps_both_ways_without_touching_the_id() {
        let offered = models(&["deepseek-v4.1-flash", "org/qualified", "weird (v2)"]);
        let rows = model_rows(&offered, "openai-compatible");

        // Direction one: id -> label.
        assert_eq!(rows[0].id, "deepseek-v4.1-flash");
        assert_eq!(
            rows[0].label, "deepseek-v4.1-flash (openai-compatible)",
            "a bare id gets the provider label"
        );
        assert_eq!(rows[1].id, "org/qualified");
        assert_eq!(
            rows[1].label, "org/qualified",
            "a provider-qualified id is not decorated twice"
        );
        assert_eq!(rows[2].id, "weird (v2)");
        assert_eq!(
            rows[2].label, "weird (v2) (openai-compatible)",
            "an id containing its own parenthesis decorates without losing text"
        );

        // Direction two: label -> id. The picker's selection is the row's
        // raw id for every row, whatever the label shows.
        for row in &rows {
            let picker = lca_ui::ModelPicker::new(vec![row.clone()]);
            assert_eq!(
                picker.selected_model(),
                Some(row.id.as_str()),
                "selection returns the raw id of {}",
                row.label
            );
            assert_eq!(
                picker.models[0].label, row.label,
                "the label shows as built"
            );
        }

        // The decorated label stays searchable, and so does the raw id.
        let mut picker = lca_ui::ModelPicker::new(rows);
        picker.query = "deepseek".to_string();
        picker.refilter();
        assert_eq!(picker.matches, vec![0], "the label matches by model id");
        picker.query = "weird (v2)".to_string();
        picker.refilter();
        assert_eq!(picker.matches, vec![2], "the raw id matches by itself");
    }

    // Verifies: FR-PROV-2 (the model picker lists every model the
    // active provider offers - the /model built-in's listing half,
    // the interface where the provider world's listing reaches a
    // human).
    #[test]
    fn the_model_picker_lists_every_offered_model_and_marks_the_active_one() {
        let text = model_picker_text(&models(&["alpha", "beta"]), "beta", "openai-compatible");
        assert!(text.contains("alpha"), "first model listed:\n{text}");
        assert!(text.contains("beta"), "second model listed:\n{text}");
        assert!(
            text.contains("beta") && text.contains("(active)"),
            "the active model is marked:\n{text}"
        );
    }

    // Verifies: gh #177 - a model from another provider names it in
    // both listings, while the active provider's own rows read as they
    // always did (no redundant suffix when origin and provider agree).
    #[test]
    fn a_foreign_model_names_its_provider_in_both_listings() {
        let offered = vec![
            ModelInfo {
                id: "gpt-5.4-mini".to_string(),
                name: "GPT 5.4 Mini (Codex)".to_string(),
                context_window: 272000,
                max_tokens: 0,
                extras: [("provider".to_string(), "codex".to_string())].into(),
            },
            ModelInfo {
                id: "home-model".to_string(),
                name: "home-model".to_string(),
                context_window: 100_000,
                max_tokens: 0,
                extras: [("provider".to_string(), "openai-compatible".to_string())].into(),
            },
        ];
        let rows = model_rows(&offered, "openai-compatible");
        assert_eq!(
            rows[0].label, "gpt-5.4-mini (codex)",
            "the foreign row names its provider"
        );
        assert_eq!(
            rows[1].label, "home-model (openai-compatible)",
            "the home row keeps the old fallback label"
        );
        let text = model_picker_text(&offered, "home-model", "openai-compatible");
        assert!(
            !text.contains("gpt-5.4-mini (active)"),
            "active marks the current model, not the foreign one:\n{text}"
        );
        assert!(
            text.contains("gpt-5.4-mini (codex)"),
            "the text listing names the foreign provider:\n{text}"
        );
        assert!(
            text.contains("home-model (active)"),
            "the home row marks active without a suffix:\n{text}"
        );
    }

    #[test]
    fn an_unknown_model_is_refused_with_the_real_alternatives() {
        let offered = models(&["alpha", "beta"]);
        let effect = model_effect_on(
            &offered,
            "openai-compatible",
            "openai-compatible",
            "gamma",
            None,
            None,
            None,
        );
        let CommandEffect::ShowWidget(text) = effect else {
            panic!("an unknown model answers with text, not an action")
        };
        assert!(text.contains("gamma"), "names the mistake: {text}");
        assert!(
            text.contains("alpha") && text.contains("beta"),
            "offers the real list: {text}"
        );
    }

    // Verifies: FR-UI-19 (the stats story: tokens, cost, and cache waste
    // per model) and FR-UI-2 (the stats line does not print `$0.0000` when
    // the provider reports no pricing - that reads as "free", not
    // "unknown").
    #[test]
    fn stats_omit_the_cost_when_no_price_is_reported() {
        let root = lca_testkit::scratch_path("lca-stats");
        let _ = std::fs::remove_dir_all(&root);
        let project = root.join("project");
        std::fs::create_dir_all(&project).expect("mkdir");
        let store = SessionStore::new(root.join("data"));
        let session = store.create_session(&project, "test").expect("session");
        let text = session_stats(&store, &session);
        assert!(!text.contains('$'), "no dead cost at zero:\n{text}");
        assert!(text.contains("0 messages"), "{text}");
        let _ = std::fs::remove_dir_all(&root);
    }

    // Verifies: gh #125 - a reported cost prints with four decimals;
    // a zero cost stays hidden (tokens-only, never a made-up number).
    #[test]
    fn stats_print_reported_cost_and_hide_zero() {
        let root = lca_testkit::scratch_path("lca-stats-cost");
        let _ = std::fs::remove_dir_all(&root);
        let project = root.join("project");
        std::fs::create_dir_all(&project).expect("mkdir");
        let store = SessionStore::new(root.join("data"));
        let session = store.create_session(&project, "test").expect("session");
        let assistant =
            |id: &str, model: &str, usage: lca_protocol::Usage| lca_protocol::Record::Assistant {
                v: lca_protocol::FORMAT_VERSION,
                ts: 1,
                id: id.to_string(),
                parent: None,
                content: Vec::new(),
                reasoning: None,
                reasoning_signature: None,
                provider_thinking_level: None,
                model: Some(model.to_string()),
                provider: Some("test".to_string()),
                usage: Some(usage),
            };
        store
            .append(
                &session,
                assistant(
                    "a1",
                    "gpt-4o",
                    lca_protocol::Usage {
                        input: 1_000_000,
                        output: 1_000_000,
                        cost: 12.50,
                        ..Default::default()
                    },
                ),
            )
            .expect("append");
        let text = session_stats(&store, &session);
        assert!(
            text.contains("$12.5000"),
            "the reported cost prints: {text}"
        );
        store
            .append(
                &session,
                assistant(
                    "a2",
                    "gpt-5",
                    lca_protocol::Usage {
                        input: 1,
                        ..Default::default()
                    },
                ),
            )
            .expect("append");
        let text = session_stats(&store, &session);
        assert!(
            text.contains("$12.5000") && !text.contains("$12.5001"),
            "the unpriced record adds tokens, not cost: {text}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn ages_read_like_pis_session_rows() {
        assert_eq!(age_label(1_000_000, 1_000_000), "now");
        assert_eq!(age_label(1_000_000 + 5 * 60_000, 1_000_000), "5m");
        assert_eq!(age_label(1_000_000 + 3 * 3_600_000, 1_000_000), "3h");
        assert_eq!(age_label(1_000_000 + 2 * 86_400_000, 1_000_000), "2d");
    }
}

#[cfg(test)]
// Verifies: gh #42 - a `cache_warm` refresh counts toward session
// totals (pi's rule) without joining model context.
#[test]
fn stats_count_cache_warm_refreshes() {
    let root = lca_testkit::scratch_path("lca-stats-warm");
    let _ = std::fs::remove_dir_all(&root);
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("mkdir");
    let store = SessionStore::new(root.join("data"));
    let session = store.create_session(&project, "test").expect("session");
    store
        .append(
            &session,
            lca_protocol::Record::Usage {
                v: lca_protocol::FORMAT_VERSION,
                ts: 1,
                id: "w1".to_string(),
                parent: None,
                kind: "cache_warm".to_string(),
                provider: Some("test".to_string()),
                model: Some("warmer".to_string()),
                usage: lca_protocol::Usage {
                    output: 1,
                    cache_read: 50_000,
                    cost: 0.01,
                    ..Default::default()
                },
            },
        )
        .expect("append");
    let text = session_stats(&store, &session);
    assert!(text.contains("50000"), "the refresh counts:\n{text}");
    assert!(text.contains("$0.0100"), "its cost counts:\n{text}");
    let _ = std::fs::remove_dir_all(&root);
}

#[cfg(test)]
mod setting_meta_tests {
    use super::setting_meta;

    // Verifies: gh #235 - every inventoried key carries a friendly
    // label and help; unknown keys degrade to a title-cased label,
    // never a blank.
    #[test]
    fn every_inventoried_key_has_a_label_and_help() {
        for key in [
            "ui.theme",
            "ui.color",
            "ui.fullscreen",
            "ui.quiet_startup",
            "ui.autocomplete_max_visible",
            "ui.editor_padding_x",
            "ui.output_pad",
            "markdown.codeblock_border",
            "thinking",
            "ui.thinking",
            "permissions.mode",
            "compaction.enabled",
            "compaction.threshold",
            "compaction.reserve_tokens",
            "compaction.keep_recent_tokens",
            "shell.tool",
            "shell.path",
            "shell.command_prefix",
            "tool.timeout_seconds",
            "tool.result_limit_bytes",
            "tool.max_iterations",
            "provider.retry_limit",
            "tool.edit_requires_read",
            "ui.double_escape_action",
            "ui.tree_filter_mode",
            "ui.fullscreen_scrollbar",
            "ui.fullscreen_copy_on_select",
            "ui.fullscreen_wheel_lines",
            "ui.fullscreen_exit_output",
            "terminal.show_images",
            "terminal.image_width_cells",
            "terminal.show_progress",
            "images.auto_resize",
            "markdown.code_block_indent",
            "markdown.mermaid",
            "cache.noise_floor_tokens",
            "extensions.log_limit_bytes",
            "update.check",
        ] {
            let (label, description) = setting_meta(key);
            assert!(!label.is_empty(), "{key} has a label");
            assert_ne!(label, key, "{key} is not raw");
            assert!(!description.is_empty(), "{key} has help");
        }
    }

    #[test]
    fn unknown_keys_degrade_gracefully() {
        assert_eq!(
            setting_meta("whatever.new_key"),
            ("New key".to_string(), String::new())
        );
    }
}

// Verifies: gh #42 - miss notices fire once per new counted miss
// when enabled, and stay silent when off or when nothing is new.
#[test]
fn miss_notices_fire_once_per_new_miss() {
    let root = lca_testkit::scratch_path("lca-miss-notice");
    let _ = std::fs::remove_dir_all(&root);
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("mkdir");
    let store = SessionStore::new(root.join("data"));
    let session = store.create_session(&project, "test").expect("session");
    let assistant =
        |id: &str, ts: u64, usage: lca_protocol::Usage| lca_protocol::Record::Assistant {
            v: lca_protocol::FORMAT_VERSION,
            ts,
            id: id.to_string(),
            parent: None,
            content: vec![],
            reasoning: None,
            reasoning_signature: None,
            provider_thinking_level: None,
            model: Some("m".to_string()),
            provider: Some("test".to_string()),
            usage: Some(usage),
        };
    // The first turn reports cache activity (FR-CACHE-4: an endpoint
    // that never reports caching has nothing to measure).
    let cached = lca_protocol::Usage {
        input: 1_000,
        cache_write: 49_000,
        ..Default::default()
    };
    let rebilled = lca_protocol::Usage {
        input: 50_000,
        ..Default::default()
    };
    store
        .append(&session, assistant("a1", 1, cached))
        .expect("append");
    let mut seen = BTreeSet::new();
    assert!(
        miss_notice_for(&store, &session, 1024, &mut seen, true).is_none(),
        "one turn is no miss"
    );
    // A second turn re-billing the same 50k prompt is a counted miss.
    store
        .append(&session, assistant("a2", 2, rebilled))
        .expect("append");
    let notice = miss_notice_for(&store, &session, 1024, &mut seen, true).expect("a notice");
    assert!(notice.contains("/session"), "{notice}");
    assert!(
        miss_notice_for(&store, &session, 1024, &mut seen, true).is_none(),
        "the same cause never repeats"
    );
    assert!(
        miss_notice_for(&store, &session, 1024, &mut BTreeSet::new(), false).is_none(),
        "off stays silent"
    );
    let _ = std::fs::remove_dir_all(&root);
}
