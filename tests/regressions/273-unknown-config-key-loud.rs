//! Gh #273: unknown `config.toml` keys were silently ignored, and the
//! `[provider]` retry keys were applied by two code paths.
//!
//! `apply_toml_layer` walks a fixed list of documented keys and looks each
//! one up in the parsed table. A key *outside* that list is therefore never
//! examined — the `other =>` arm that returns `UnknownKey` could not be
//! reached, because the loop only ever passes known keys to it. A typo such
//! as `compaction.theshold = 0.5` loaded clean, changed nothing, and said
//! nothing: the user believes they configured a threshold and the tool
//! agrees with them by silence.
//!
//! The fix walks the table instead of the key list, so a key the vocabulary
//! does not contain is refused by name. That is only safe once the
//! vocabulary is complete — which is gh #272, landed in the previous commit.
//!
//! Verifies: gh #273 acceptance 3-4 at the crate's public surface.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::collections::BTreeMap;

use lca_config::{Config, ConfigError, KNOWN_KEYS, LoadInput, TABLE_KEYS};

fn scratch(name: &str) -> std::path::PathBuf {
    lca_testkit::scratch_path(name)
}

fn load_user(name: &str, body: &str) -> Result<Config, ConfigError> {
    let dir = scratch(name);
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(dir.join("user.toml"), body).expect("write");
    Config::load(&LoadInput {
        user_file: Some(dir.join("user.toml")),
        ..Default::default()
    })
}

// Verifies: gh #273 acceptance 3 - the defect itself. A typo'd key is an
// error naming the key, not a silent no-op.
#[test]
fn a_typo_is_refused_instead_of_silently_ignored() {
    let err = load_user("273r-typo", "compaction.theshold = 0.5\n")
        .expect_err("the typo must be refused");
    assert!(
        err.to_string().contains("compaction.theshold"),
        "names the key: {err}"
    );
    assert!(
        matches!(err, ConfigError::UnknownKey { .. }),
        "its own variant, so callers can tell 'unknown' from 'bad value': {err:?}"
    );
}

// Verifies: gh #273 acceptance 3 - the message tells the user what to do. The
// two real causes are a typo and an older binary, so the copy names both;
// this is also the recorded compat trade (see the report).
#[test]
fn the_refusal_says_what_to_check() {
    let err = load_user("273r-copy", "ui.colour = \"never\"\n").expect_err("refused");
    let text = err.to_string();
    assert!(text.contains("ui.colour"), "names the key: {text}");
    assert!(
        text.contains("lca config"),
        "points at the key list the build has: {text}"
    );
    assert!(
        text.contains("newer"),
        "names the older-binary case: {text}"
    );
}

// Verifies: gh #273 acceptance 3 - section form is judged the same as dotted
// form. A walk that matched only the literal dotted spelling would let
// `[ui] colour = ...` through.
#[test]
fn section_form_is_judged_like_dotted_form() {
    let err = load_user("273r-section", "[ui]\ncolour = \"never\"\n")
        .expect_err("the section spelling is the same key");
    assert!(err.to_string().contains("ui.colour"), "{err}");
}

// Verifies: gh #273 acceptance 3 - the reported path is the one the user
// typed, including the table it sat under, so the fix is obvious.
#[test]
fn the_reported_path_names_the_whole_key() {
    let err = load_user("273r-fullpath", "[net]\nproxy_url = \"http://p:8080\"\n")
        .expect_err("the proxy is environment-only, not a key");
    let text = err.to_string();
    assert!(
        text.contains("net.proxy_url"),
        "the whole path, not just the leaf: {text}"
    );
}

// Verifies: gh #273 acceptance 3 (FR-PERM-9) - the DoS boundary. An UNTRUSTED
// project file is never read, so a hostile repository cannot brick startup by
// shipping a junk key. This is the deliberate asymmetry the brief asked for:
// hard-fail on files the user opted into, ignore entirely the one they did
// not.
#[test]
fn a_hostile_untrusted_project_file_cannot_brick_startup() {
    let dir = scratch("273r-untrusted");
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(
        dir.join("project.toml"),
        "hostile = 1\n[another]\njunk = 2\ncompaction.theshold = 0.5\n",
    )
    .expect("write");
    let config = Config::load(&LoadInput {
        project_file: Some(dir.join("project.toml")),
        trusted: false,
        ..Default::default()
    })
    .expect("an untrusted project file is ignored wholesale");
    assert_eq!(config.compaction_threshold(), 0.8, "the default stands");
}

// Verifies: gh #273 acceptance 3 - a TRUSTED project file does participate, so
// its junk key is refused like the user file's.
#[test]
fn a_trusted_project_file_with_a_junk_key_is_refused() {
    let dir = scratch("273r-trusted");
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(dir.join("project.toml"), "compaction.theshold = 0.5\n").expect("write");
    let err = Config::load(&LoadInput {
        project_file: Some(dir.join("project.toml")),
        trusted: true,
        ..Default::default()
    })
    .expect_err("the trusted project file participates");
    assert!(err.to_string().contains("project file"), "{err}");
}

// Verifies: gh #273 acceptance 4 - `[provider]` retry keys have ONE code path
// now. The section form and the dotted form must agree on every retry key,
// including the max-delay key the deleted duplicate never handled.
#[test]
fn the_provider_section_and_dotted_forms_agree_on_every_retry_key() {
    let dotted = load_user(
        "273r-dotted",
        "provider.retry_limit = 7\nprovider.retry_base_delay_ms = 111\nprovider.retry_max_delay_ms = 222\n",
    )
    .expect("dotted loads");
    let section = load_user(
        "273r-section-retry",
        "[provider]\nretry_limit = 7\nretry_base_delay_ms = 111\nretry_max_delay_ms = 222\n",
    )
    .expect("section loads");
    for (a, b) in [
        (
            dotted.provider_retry_limit(),
            section.provider_retry_limit(),
        ),
        (
            dotted.provider_retry_base_delay_ms(),
            section.provider_retry_base_delay_ms(),
        ),
        (
            dotted.provider_retry_max_delay_ms(),
            section.provider_retry_max_delay_ms(),
        ),
    ] {
        assert_eq!(a, b, "the two forms must produce identical values");
    }
    assert_eq!(section.provider_retry_limit(), 7);
    assert_eq!(section.provider_retry_base_delay_ms(), 111);
    assert_eq!(section.provider_retry_max_delay_ms(), 222);
}

// Verifies: gh #273 - a config using every documented scalar key still loads.
// The vocabulary, not a hand-written list in the test, is the source: if a
// key cannot round-trip through a file, the walk and the parser disagree.
#[test]
fn every_documented_key_round_trips_through_a_file() {
    // One representative value per key. Table-valued keys are exercised by
    // their own suites (a table needs shape-specific fixtures).
    let values: &[(&str, &str)] = &[
        ("model", "\"m1\""),
        ("compaction.threshold", "0.5"),
        ("compaction.enabled", "false"),
        ("compaction.strategy", "\"summarize\""),
        ("compaction.reserve_tokens", "10"),
        ("compaction.keep_recent_tokens", "20"),
        ("provider.retry_limit", "1"),
        ("provider.retry_base_delay_ms", "2"),
        ("provider.retry_max_delay_ms", "3"),
        ("tool.timeout_seconds", "4"),
        ("tool.result_limit_bytes", "5"),
        ("tool.max_iterations", "6"),
        ("tool.edit_requires_read", "true"),
        ("cache.noise_floor_tokens", "7"),
        ("cache.warming", "\"idle\""),
        ("cache.miss_notices", "true"),
        ("extensions.log_limit_bytes", "8"),
        ("update.check", "false"),
        ("ui.fullscreen", "true"),
        ("ui.quiet_startup", "\"header\""),
        ("ui.double_escape_action", "\"fork\""),
        ("ui.tree_filter_mode", "\"all\""),
        ("ui.autocomplete_max_visible", "4"),
        ("ui.editor_padding_x", "1"),
        ("ui.output_pad", "0"),
        ("ui.fullscreen_scrollbar", "\"always\""),
        ("ui.fullscreen_copy_on_select", "false"),
        ("ui.fullscreen_wheel_lines", "\"auto\""),
        ("ui.fullscreen_exit_output", "\"resume-hint\""),
        ("ui.show_hardware_cursor", "true"),
        ("terminal.show_images", "false"),
        ("terminal.image_width_cells", "2"),
        ("terminal.clear_on_shrink", "true"),
        ("terminal.show_progress", "false"),
        ("terminal.hyperlinks", "\"auto\""),
        ("terminal.images", "\"kitty\""),
        ("terminal.true_color", "\"true\""),
        ("images.auto_resize", "false"),
        ("images.block_images", "true"),
        ("markdown.code_block_indent", "\"    \""),
        ("markdown.mermaid", "\"off\""),
        ("ui.color", "\"never\""),
        ("ui.theme", "\"dark\""),
        ("thinking", "\"low\""),
        ("shell.tool", "\"bash\""),
        ("shell.path", "\"/bin/sh\""),
        ("shell.command_prefix", "\"set -e\""),
        ("permissions.mode", "\"yolo\""),
        ("queue.steering_mode", "\"all\""),
        ("queue.follow_up_mode", "\"all\""),
        ("trust.default_project", "\"ask\""),
        ("ui.thinking", "\"full\""),
        ("markdown.codeblock_border", "\"none\""),
        ("skills.inject_matched", "true"),
        ("models.enabled", "[\"m1\"]"),
    ];
    // Every scalar key in the vocabulary must appear above, and nothing
    // extra: this is the assertion that keeps the fixture honest as keys
    // are added.
    let documented_scalars: Vec<&str> = KNOWN_KEYS
        .iter()
        .copied()
        .filter(|key| !TABLE_KEYS.contains(key))
        .filter(|key| *key != "provider") // a leaf AND a section; see below
        .collect();
    let covered: Vec<&str> = values.iter().map(|(key, _)| *key).collect();
    for key in &documented_scalars {
        assert!(
            covered.contains(key),
            "`{key}` is a documented key but this round-trip fixture does not cover it"
        );
    }

    let body: String = values
        .iter()
        .map(|(key, value)| format!("{key} = {value}\n"))
        .collect();
    let config = load_user("273r-roundtrip", &body)
        .expect("every documented scalar key must round-trip through a file");
    assert!(config.skills_inject_matched());
    assert_eq!(config.ui_theme(), Some("dark"));

    // `provider` cannot ride the same document (TOML forbids one path as both
    // a string and a table, and `provider.retry_limit` above makes it a
    // table), so its leaf form gets its own load.
    let leaf = load_user("273r-provider-leaf", "provider = \"acme\"\n").expect("leaf form loads");
    assert_eq!(leaf.provider(), "acme");
}

// Verifies: gh #273 - flags and environment are unaffected: the walk is a
// file-layer check, and `parse_typed` already refuses unknown flag/env keys.
#[test]
fn non_file_layers_keep_their_own_vocabulary() {
    let config = Config::load(&LoadInput {
        flags: BTreeMap::from([("ui.theme".to_string(), "dark".to_string())]),
        env: BTreeMap::from([("LCA_COMPACTION_THRESHOLD".to_string(), "0.25".to_string())]),
        ..Default::default()
    })
    .expect("flags and env load");
    assert_eq!(config.ui_theme(), Some("dark"));
    assert!((config.compaction_threshold() - 0.25).abs() < f64::EPSILON);
}
