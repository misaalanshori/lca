//! Inventory rows: the keys that carry a list or a map - enabled models and
//! their thinking levels, per-level budgets, matched skills, the compaction
//! budget, the display inventory, and the update check.
//!
//! Split out of `tests/merge.rs` under gate 11's 1,200-line ceiling (gh #545).
//! These are the rows whose values are structures rather than scalars, which
//! is why they are their own subject: a shape change breaks here, not in the
//! typed-key file.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use lca_config::{Config, MergeSource};

use super::support::{scratch, write};

// Verifies: gh #8 / EFG-003 (the enabled model set, pi's `enabledModels`)
// - `models.enabled` is an ordinary layered key: a TOML array in a file,
// a comma list through the environment and the flag layer, and an unset
// key means *everything the provider offers* (empty scope = all).
#[test]
fn the_enabled_model_scope_merges_from_every_layer() {
    let dir = scratch("models-enabled");
    write(
        &dir.join("user.toml"),
        "models.enabled = [\"zen/*\", \"*spark*\"]\n",
    );

    // 1. A file's array is the scope.
    let config = Config::load(&lca_config::LoadInput {
        user_file: Some(dir.join("user.toml")),
        ..Default::default()
    })
    .expect("load");
    assert_eq!(config.models_enabled(), vec!["zen/*", "*spark*"]);

    // 2. Unset means no restriction: every offered model is in scope.
    let unset = Config::load(&lca_config::LoadInput::default()).expect("load");
    assert!(unset.models_enabled().is_empty(), "empty scope = all");

    // 3. The environment reads as a comma list and outranks the file.
    let env = || {
        let mut map = std::collections::BTreeMap::new();
        map.insert(
            "LCA_MODELS_ENABLED".to_string(),
            "mimo-a, mimo-b".to_string(),
        );
        map
    };
    let config = Config::load(&lca_config::LoadInput {
        env: env(),
        user_file: Some(dir.join("user.toml")),
        ..Default::default()
    })
    .expect("load");
    assert_eq!(config.models_enabled(), vec!["mimo-a", "mimo-b"]);

    // 4. `--models a,b` outranks the environment (FR-CFG-1's flag layer).
    let mut flags = std::collections::BTreeMap::new();
    flags.insert("models.enabled".to_string(), "only-this".to_string());
    let config = Config::load(&lca_config::LoadInput {
        flags,
        env: env(),
        user_file: Some(dir.join("user.toml")),
        ..Default::default()
    })
    .expect("load");
    assert_eq!(config.models_enabled(), vec!["only-this"]);
    assert!(
        config
            .resolved()
            .any(|(key, _, source)| key == "models.enabled" && source == MergeSource::Flag),
        "`/settings` names the flag as the winning source"
    );
}

// Verifies: gh #8 phase 4 (pi's `modelThinkingLevels`) - a per-model map
// of the levels that model accepts, in the file (it is a table, so it has
// no single-value environment form); a level outside the vocabulary or a
// value that is not a list of strings is refused at load.
#[test]
fn per_model_thinking_levels_parse_and_refuse_junk() {
    let dir = scratch("thinking-levels");
    write(
        &dir.join("user.toml"),
        // A model id carries dots, so its key is quoted - the same TOML
        // rule any dotted key follows (`tool.timeout_seconds`).
        "[models.thinking_levels]\n\"mimo-v2.6-flash-free\" = [\"low\", \"high\"]\n",
    );
    let config = Config::load(&lca_config::LoadInput {
        user_file: Some(dir.join("user.toml")),
        ..Default::default()
    })
    .expect("load");
    assert_eq!(
        config.allowed_thinking_levels("mimo-v2.6-flash-free"),
        Some(&["low".to_string(), "high".to_string()][..]),
        "the map reads back as the model's allowed set"
    );
    assert_eq!(
        config.allowed_thinking_levels("something-else"),
        None,
        "a model the map does not name has no set"
    );
    assert_eq!(
        config.default_thinking_for("mimo-v2.6-flash-free"),
        Some("low"),
        "the first allowed level is the model's default"
    );

    write(
        &dir.join("user.toml"),
        "[models.thinking_levels]\nbroken = [\"lightning\"]\n",
    );
    let err = Config::load(&lca_config::LoadInput {
        user_file: Some(dir.join("user.toml")),
        ..Default::default()
    })
    .expect_err("a level outside the vocabulary is refused");
    assert!(err.to_string().contains("lightning"), "{err}");

    write(
        &dir.join("user.toml"),
        "[models.thinking_levels]\nbroken = \"high\"\n",
    );
    let err = Config::load(&lca_config::LoadInput {
        user_file: Some(dir.join("user.toml")),
        ..Default::default()
    })
    .expect_err("a single level is not a list");
    assert!(err.to_string().contains("models.thinking_levels"), "{err}");
}

// Verifies: gh #8 - the clamp: a level the model does not offer becomes
// the model's default (its first allowed level); a level inside the set
// passes through untouched; an unconfigured model keeps what it was
// asked for; and "unset" is not a level, so it stays unset.
#[test]
fn a_level_outside_the_models_set_clamps_to_its_default() {
    let dir = scratch("thinking-clamp");
    write(
        &dir.join("user.toml"),
        "[models.thinking_levels]\nstrict = [\"low\", \"medium\"]\nloose = [\"off\", \"high\"]\n",
    );
    let config = Config::load(&lca_config::LoadInput {
        user_file: Some(dir.join("user.toml")),
        ..Default::default()
    })
    .expect("load");

    assert_eq!(
        config.clamp_thinking(Some("high"), "strict"),
        Some("low".to_string()),
        "high is not offered here, so the model's default is"
    );
    assert_eq!(
        config.clamp_thinking(Some("medium"), "strict"),
        Some("medium".to_string()),
        "an offered level passes through"
    );
    assert_eq!(
        config.clamp_thinking(Some("xhigh"), "loose"),
        Some("off".to_string()),
        "the clamp lands on the model's own default, not the nearest level"
    );
    assert_eq!(
        config.clamp_thinking(Some("high"), "unconfigured"),
        Some("high".to_string()),
        "no map entry means no restriction"
    );
    assert_eq!(
        config.clamp_thinking(None, "strict"),
        None,
        "unset is the provider's choice, not a level to clamp"
    );
}

// Verifies: gh #8 - a model switch applies the new model's configured
// default (pi's per-model default beating the global one); with no
// configured default the session's current level survives, clamped to
// what the new model accepts.
#[test]
fn switching_applies_the_models_configured_default() {
    let dir = scratch("thinking-switch");
    write(
        &dir.join("user.toml"),
        "[models.thinking_levels]\ndeep = [\"medium\", \"high\"]\nshallow = [\"off\"]\n",
    );
    let config = Config::load(&lca_config::LoadInput {
        user_file: Some(dir.join("user.toml")),
        ..Default::default()
    })
    .expect("load");

    assert_eq!(
        config.switch_thinking(Some("off"), "deep"),
        Some("medium".to_string()),
        "the model's default wins over the session's level"
    );
    assert_eq!(
        config.switch_thinking(Some("high"), "shallow"),
        Some("off".to_string()),
        "and a level the new model refuses is clamped into its set"
    );
    assert_eq!(
        config.switch_thinking(Some("high"), "unconfigured"),
        Some("high".to_string()),
        "no default configured: the level keeps going"
    );
    assert_eq!(
        config.switch_thinking(None, "deep"),
        Some("medium".to_string()),
        "a model with a default still gets it, from unset too"
    );
    assert_eq!(config.switch_thinking(None, "unconfigured"), None);
}

// Verifies: gh #82 - the display inventory loads with pi-matching
// defaults, and out-of-domain values refuse at load.
#[test]
fn display_inventory_loads_with_documented_defaults() {
    let defaults = Config::defaults();
    assert_eq!(defaults.ui_autocomplete_max_visible(), 5);
    assert_eq!(defaults.ui_editor_padding_x(), 0);
    assert_eq!(defaults.ui_output_pad(), 1);
    assert_eq!(defaults.ui_fullscreen_scrollbar(), "auto");
    assert!(defaults.ui_fullscreen_copy_on_select());
    assert_eq!(defaults.ui_fullscreen_wheel_lines(), "auto");
    assert_eq!(defaults.ui_fullscreen_exit_output(), "transcript");
    assert!(!defaults.ui_show_hardware_cursor());
    assert!(defaults.terminal_show_images());
    assert_eq!(defaults.terminal_image_width_cells(), 60);
    assert!(!defaults.terminal_clear_on_shrink());
    assert!(defaults.terminal_show_progress(), "on: current behavior");
    assert_eq!(defaults.terminal_hyperlinks(), "auto");
    assert_eq!(defaults.terminal_images(), "auto");
    assert_eq!(defaults.terminal_true_color(), "auto");
    assert!(defaults.images_auto_resize());
    assert!(!defaults.images_block_images());
    assert_eq!(defaults.markdown_code_block_indent(), "  ");
    assert_eq!(defaults.markdown_mermaid(), "streaming");

    let dir = scratch("display-inventory");
    write(
        &dir.join("user.toml"),
        "ui.autocomplete_max_visible = 8\nui.editor_padding_x = 2\nui.output_pad = 0\nui.fullscreen_scrollbar = \"hidden\"\nui.fullscreen_copy_on_select = false\nui.fullscreen_wheel_lines = \"6\"\nui.fullscreen_exit_output = \"resume-hint\"\nui.show_hardware_cursor = true\nterminal.show_images = false\nterminal.image_width_cells = 80\nterminal.clear_on_shrink = true\nterminal.show_progress = false\nterminal.hyperlinks = false\nterminal.images = \"kitty\"\nterminal.true_color = false\nimages.auto_resize = false\nimages.block_images = true\nmarkdown.code_block_indent = \">>\"\nmarkdown.mermaid = \"off\"\n",
    );
    let config = Config::load(&lca_config::LoadInput {
        user_file: Some(dir.join("user.toml")),
        ..Default::default()
    })
    .expect("load");
    assert_eq!(config.ui_autocomplete_max_visible(), 8);
    assert_eq!(config.ui_editor_padding_x(), 2);
    assert_eq!(config.ui_output_pad(), 0);
    assert_eq!(config.ui_fullscreen_scrollbar(), "hidden");
    assert!(!config.ui_fullscreen_copy_on_select());
    assert_eq!(config.ui_fullscreen_wheel_lines(), "6");
    assert_eq!(config.ui_fullscreen_exit_output(), "resume-hint");
    assert!(config.ui_show_hardware_cursor());
    assert!(!config.terminal_show_images());
    assert_eq!(config.terminal_image_width_cells(), 80);
    assert!(config.terminal_clear_on_shrink());
    assert!(!config.terminal_show_progress());
    assert_eq!(config.terminal_hyperlinks(), "false");
    assert_eq!(config.terminal_images(), "kitty");
    assert_eq!(config.terminal_true_color(), "false");
    assert!(!config.images_auto_resize());
    assert!(config.images_block_images());
    assert_eq!(config.markdown_code_block_indent(), ">>");
    assert_eq!(config.markdown_mermaid(), "off");

    write(&dir.join("user.toml"), "ui.autocomplete_max_visible = 99\n");
    let err = Config::load(&lca_config::LoadInput {
        user_file: Some(dir.join("user.toml")),
        ..Default::default()
    })
    .expect_err("out of range refuses");
    assert!(
        err.to_string().contains("ui.autocomplete_max_visible"),
        "{err}"
    );
}

// Verifies: gh #41 (pi's `thinkingBudgets` row) — per-level token
// budgets default to pi's built-ins, a file overrides per level, and
// an unknown budget name is refused rather than silently kept.
#[test]
fn thinking_budgets_default_override_and_refuse_unknown() {
    let dir = scratch("thinking-budgets");
    let config = Config::load(&lca_config::LoadInput {
        user_file: None,
        ..Default::default()
    })
    .expect("load");
    for (level, budget) in [
        ("minimal", 1024),
        ("low", 2048),
        ("medium", 8192),
        ("high", 16384),
    ] {
        assert_eq!(config.budget_for_level(Some(level)), Some(budget));
    }
    assert_eq!(config.budget_for_level(Some("xhigh")), Some(16384));
    assert_eq!(config.budget_for_level(Some("max")), Some(16384));
    assert_eq!(config.budget_for_level(None), None);
    assert_eq!(config.budget_for_level(Some("off")), None);

    write(&dir.join("user.toml"), "[thinking.budgets]\nlow = 3000\n");
    let config = Config::load(&lca_config::LoadInput {
        user_file: Some(dir.join("user.toml")),
        ..Default::default()
    })
    .expect("load");
    assert_eq!(config.budget_for_level(Some("low")), Some(3000));
    assert_eq!(config.budget_for_level(Some("high")), Some(16384));

    write(&dir.join("user.toml"), "[thinking.budgets]\nenormous = 1\n");
    let err = Config::load(&lca_config::LoadInput {
        user_file: Some(dir.join("user.toml")),
        ..Default::default()
    })
    .expect_err("an unknown budget name is refused at load");
    assert!(err.to_string().contains("enormous"), "{err}");
}

// Verifies: gh #43 (`skills.inject_matched`): matched skill-text
// injection is opt-in, default off, settable from a file.
#[test]
fn skills_inject_matched_defaults_off_and_loads_from_file() {
    let plain = Config::defaults();
    assert!(!plain.skills_inject_matched(), "the default is the catalog");
    assert_eq!(
        plain.source_of("skills.inject_matched"),
        lca_config::MergeSource::Default
    );

    let dir = scratch("skills-inject");
    write(&dir.join("user.toml"), "[skills]\ninject_matched = true\n");
    let config = Config::load(&lca_config::LoadInput {
        user_file: Some(dir.join("user.toml")),
        ..Default::default()
    })
    .expect("load");
    assert!(config.skills_inject_matched());
    assert_eq!(
        config.source_of("skills.inject_matched"),
        lca_config::MergeSource::UserFile
    );
}

// Verifies: gh #36 phase 1 - the compaction budget keys load from the
// user file and report their source; a bad value names its key.
#[test]
fn compaction_budget_keys_load_and_a_bad_value_names_its_key() {
    let dir = scratch("compaction-budget");
    write(
        &dir.join("user.toml"),
        "[compaction]\nenabled = false\nreserve_tokens = 4096\nkeep_recent_tokens = 5000\n",
    );
    let config = Config::load(&lca_config::LoadInput {
        user_file: Some(dir.join("user.toml")),
        ..Default::default()
    })
    .expect("load");
    assert!(!config.compaction_enabled());
    assert_eq!(config.compaction_reserve_tokens(), 4096);
    assert_eq!(config.compaction_keep_recent_tokens(), 5000);
    assert_eq!(
        config.source_of("compaction.reserve_tokens"),
        lca_config::MergeSource::UserFile
    );

    let bad = scratch("compaction-budget-bad");
    write(
        &bad.join("user.toml"),
        "[compaction]\nreserve_tokens = -5\n",
    );
    let err = Config::load(&lca_config::LoadInput {
        user_file: Some(bad.join("user.toml")),
        ..Default::default()
    })
    .expect_err("rejects");
    assert!(
        err.to_string().contains("compaction.reserve_tokens"),
        "{err}"
    );
}

// Verifies: FR-CFG-6 (update check defaults to on interactively, off headless)
#[test]
fn update_check_default_depends_on_mode() {
    let interactive = Config::defaults();
    assert!(interactive.update_check(false), "interactive default is on");
    let headless = Config::load(&lca_config::LoadInput {
        flags: Default::default(),
        env: Default::default(),
        project_file: None,
        trusted: true,
        user_file: None,
        headless: true,
    })
    .expect("load");
    assert!(!headless.update_check(true), "headless default is off");
}
