//! Gh #272: the three hand-mirrored configuration vocabularies drifted.
//!
//! `KNOWN_KEYS` (what a `LCA_` environment variable can set), `Config::resolved()`
//! (what `lca config` / `/settings` reports, FR-CFG-2), and the TOML
//! application loop in `apply_toml_layer` are all hand-maintained mirrors of
//! the same vocabulary. Two drifts were filed:
//!
//! 1. `skills.inject_matched` was missing from `KNOWN_KEYS`, so
//!    `LCA_SKILLS_INJECT_MATCHED=true` silently did nothing (the environment
//!    loop at `Config::load` iterates `KNOWN_KEYS`).
//! 2. Seven settings were missing from `resolved()`, so `lca config`
//!    under-reported them: FR-CFG-2's inspector lied by omission.
//!
//! Verifies: gh #272 acceptance 1-2.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::collections::BTreeMap;

use lca_config::{Config, LoadInput, MergeSource};

fn scratch(name: &str) -> std::path::PathBuf {
    lca_testkit::scratch_path(name)
}

/// Resolve one key out of `lca config`'s inventory.
fn resolved_value(config: &Config, key: &str) -> Option<(String, MergeSource)> {
    config
        .resolved()
        .find(|(name, _, _)| *name == key)
        .map(|(_, value, source)| (value, source))
}

// Verifies: gh #272 acceptance 1 - the environment variable works. This is
// the row that was impossible before: the key was absent from `KNOWN_KEYS`,
// which is the list the environment loop iterates.
#[test]
fn the_environment_variable_for_skills_inject_matched_takes_effect() {
    let config = Config::load(&LoadInput {
        env: BTreeMap::from([("LCA_SKILLS_INJECT_MATCHED".to_string(), "true".to_string())]),
        ..Default::default()
    })
    .expect("load");
    assert!(
        config.skills_inject_matched(),
        "LCA_SKILLS_INJECT_MATCHED=true must turn the setting on"
    );
    assert_eq!(
        config.source_of("skills.inject_matched"),
        MergeSource::Env,
        "and it must say the environment set it"
    );
}

// Verifies: gh #272 acceptance 1 - the inverse direction too, so the row
// cannot pass on a default that happens to be `true`.
#[test]
fn the_environment_can_also_turn_skills_injection_off() {
    let config = Config::load(&LoadInput {
        env: BTreeMap::from([("LCA_SKILLS_INJECT_MATCHED".to_string(), "false".to_string())]),
        ..Default::default()
    })
    .expect("load");
    assert!(!config.skills_inject_matched());
    assert_eq!(config.source_of("skills.inject_matched"), MergeSource::Env);
}

// Verifies: gh #272 acceptance 2 - every documented key is reported by
// `lca config`. The seven that used to be missing are named explicitly, so
// this row fails loudly if any is dropped again.
#[test]
fn the_inspector_reports_every_documented_key() {
    let config = Config::defaults();
    for key in [
        "compaction.strategy",
        "provider.retry_base_delay_ms",
        "provider.retry_max_delay_ms",
        "queue.steering_mode",
        "queue.follow_up_mode",
        "thinking.budgets",
        "extensions.catalogs",
    ] {
        assert!(
            resolved_value(&config, key).is_some(),
            "`lca config` must report `{key}`; it is missing from resolved()"
        );
    }
}

// Verifies: gh #272 acceptance 2 - the seven report their *source*, not just
// a value: a key set in the user file says so, and an unset one says default.
#[test]
fn the_seven_keys_carry_their_sources() {
    let dir = scratch("272-sources");
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(
        dir.join("user.toml"),
        r#"
compaction.strategy = "summarize"
provider.retry_base_delay_ms = 100
provider.retry_max_delay_ms = 5000
queue.steering_mode = "one-at-a-time"
queue.follow_up_mode = "all"

[thinking.budgets]
low = 3000

[[extensions.catalogs]]
id = "acme"
name = "Acme"
url = "https://example.invalid/c.toml"
"#,
    )
    .expect("write");
    let config = Config::load(&LoadInput {
        user_file: Some(dir.join("user.toml")),
        ..Default::default()
    })
    .expect("load");

    for key in [
        "compaction.strategy",
        "provider.retry_base_delay_ms",
        "provider.retry_max_delay_ms",
        "queue.steering_mode",
        "queue.follow_up_mode",
        "thinking.budgets",
        "extensions.catalogs",
    ] {
        let (_, source) = resolved_value(&config, key).expect("reported");
        assert_eq!(
            source,
            MergeSource::UserFile,
            "`{key}` must report the user file as its source"
        );
    }
}

// Verifies: gh #272 acceptance 2 - the reported values are the effective
// ones, so the dump is not merely a list of names.
#[test]
fn the_seven_keys_report_their_effective_values() {
    let dir = scratch("272-values");
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(
        dir.join("user.toml"),
        r#"
compaction.strategy = "summarize"
provider.retry_base_delay_ms = 100
provider.retry_max_delay_ms = 5000
queue.steering_mode = "one-at-a-time"
queue.follow_up_mode = "all"

[thinking.budgets]
low = 3000

[[extensions.catalogs]]
id = "acme"
name = "Acme"
url = "https://example.invalid/c.toml"
"#,
    )
    .expect("write");
    let config = Config::load(&LoadInput {
        user_file: Some(dir.join("user.toml")),
        ..Default::default()
    })
    .expect("load");

    assert_eq!(
        resolved_value(&config, "compaction.strategy").map(|(v, _)| v),
        Some("summarize".to_string())
    );
    assert_eq!(
        resolved_value(&config, "provider.retry_base_delay_ms").map(|(v, _)| v),
        Some("100".to_string())
    );
    assert_eq!(
        resolved_value(&config, "provider.retry_max_delay_ms").map(|(v, _)| v),
        Some("5000".to_string())
    );
    assert_eq!(
        resolved_value(&config, "queue.steering_mode").map(|(v, _)| v),
        Some("one-at-a-time".to_string())
    );
    assert_eq!(
        resolved_value(&config, "queue.follow_up_mode").map(|(v, _)| v),
        Some("all".to_string())
    );
    // The table-valued keys report a shape, not a dump (the `permissions`
    // and `models.thinking_levels` rows do the same): a level count and a
    // catalog count, so the row says something without printing a table.
    assert_eq!(
        resolved_value(&config, "thinking.budgets").map(|(v, _)| v),
        Some("1 level(s)".to_string())
    );
    assert_eq!(
        resolved_value(&config, "extensions.catalogs").map(|(v, _)| v),
        Some("1 catalog(s)".to_string())
    );
}

// Verifies: gh #272 acceptance 2 - the table-valued keys say `<unset>` when
// empty rather than rendering an empty string, matching the existing
// `models.thinking_levels` / `permissions.proposals` rows.
#[test]
fn an_unset_table_key_says_so() {
    let config = Config::defaults();
    assert_eq!(
        resolved_value(&config, "thinking.budgets").map(|(v, _)| v),
        Some("<unset>".to_string())
    );
    assert_eq!(
        resolved_value(&config, "extensions.catalogs").map(|(v, _)| v),
        Some("<unset>".to_string())
    );
    // `compaction.strategy` is an optional string and follows the same
    // `<>` convention the other optional rows use.
    assert_eq!(
        resolved_value(&config, "compaction.strategy").map(|(v, _)| v),
        Some("<automatic>".to_string())
    );
}

// Verifies: gh #272 acceptance 2 - `resolved()` reports each key once. A
// duplicate would silently shadow a row in the `BTreeMap` the dump builds.
#[test]
fn no_key_is_reported_twice() {
    let config = Config::defaults();
    let keys: Vec<&str> = config.resolved().map(|(key, _, _)| key).collect();
    let mut sorted = keys.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(
        keys.len(),
        sorted.len(),
        "duplicate key in resolved(): {keys:?}"
    );
}

// ============================================================================
// gh #538: `terminal.clear_on_shrink` is wired, so it is exercised from every
// layer it can be set from. The renderer side lives in `MainScreenRenderer`
// (gh #372 bug 4); these rows pin the *value* the renderer receives.
// ============================================================================

/// The value the renderer will be handed, keyed by the dotted key, from a
/// loaded config. One helper so every row below asserts on the same thing.
fn loaded(input: LoadInput) -> Config {
    Config::load(&input).expect("load")
}

// Verifies: gh #538 - the documented default is off, from an empty load. Off
// is pi's default and the deliberate choice of the cycle that implemented the
// renderer half (gh #372 bug 4): a key that defaults to changing behaviour is
// not a setting.
#[test]
fn clear_on_shrink_defaults_to_off_when_nothing_sets_it() {
    let config = loaded(LoadInput::default());
    assert!(
        !config.terminal_clear_on_shrink(),
        "unset means off, exactly as pi's terminal.clearOnShrink ships"
    );
    assert_eq!(
        resolved_value(&config, "terminal.clear_on_shrink"),
        Some(("false".to_string(), MergeSource::Default)),
        "the inspector reports the built-in default, and says it is the default (FR-CFG-2)"
    );
}

// Verifies: gh #538 - the user config file sets it, in both directions, so the
// row cannot pass on a default that happens to match.
#[test]
fn the_user_file_sets_clear_on_shrink_both_ways() {
    let dir = scratch("lca-clear-on-shrink-file");
    let file = dir.join("user.toml");
    for (written, expected) in [("true", true), ("false", false)] {
        std::fs::write(
            file.as_path(),
            format!("terminal.clear_on_shrink = {written}\n"),
        )
        .expect("write");
        let config = loaded(LoadInput {
            user_file: Some(file.clone()),
            ..Default::default()
        });
        assert_eq!(
            config.terminal_clear_on_shrink(),
            expected,
            "the file said {written}"
        );
        assert_eq!(
            resolved_value(&config, "terminal.clear_on_shrink"),
            Some((written.to_string(), MergeSource::UserFile)),
            "and the inspector names the user file as the source (FR-CFG-2)"
        );
    }
}

// Verifies: gh #538 - the environment layer sets it too, in both directions:
// `terminal.clear_on_shrink` is in `KNOWN_KEYS`, which is the list the
// environment loop iterates (gh #272's drift class).
#[test]
fn the_environment_sets_clear_on_shrink_both_ways() {
    for (raw, expected) in [("true", true), ("false", false)] {
        let config = loaded(LoadInput {
            env: BTreeMap::from([("LCA_TERMINAL_CLEAR_ON_SHRINK".to_string(), raw.to_string())]),
            ..Default::default()
        });
        assert_eq!(
            config.terminal_clear_on_shrink(),
            expected,
            "env said {raw}"
        );
        assert_eq!(
            resolved_value(&config, "terminal.clear_on_shrink"),
            Some((raw.to_string(), MergeSource::Env))
        );
    }
}

// Verifies: gh #538 - the precedence ladder is the thing being exercised:
// environment outranks the user file, and a flag outranks both.
#[test]
fn clear_on_shrink_follows_the_precedence_ladder() {
    let dir = scratch("lca-clear-on-shrink-ladder");
    let file = dir.join("user.toml");
    std::fs::write(file.as_path(), "terminal.clear_on_shrink = true\n").expect("write");
    let from_file = loaded(LoadInput {
        user_file: Some(file.clone()),
        ..Default::default()
    });
    assert!(from_file.terminal_clear_on_shrink());

    let from_env = loaded(LoadInput {
        user_file: Some(file.clone()),
        env: BTreeMap::from([(
            "LCA_TERMINAL_CLEAR_ON_SHRINK".to_string(),
            "false".to_string(),
        )]),
        ..Default::default()
    });
    assert!(
        !from_env.terminal_clear_on_shrink(),
        "environment outranks the user file (FR-CFG-1)"
    );

    let from_flag = loaded(LoadInput {
        user_file: Some(file),
        env: BTreeMap::from([(
            "LCA_TERMINAL_CLEAR_ON_SHRINK".to_string(),
            "false".to_string(),
        )]),
        flags: BTreeMap::from([("terminal.clear_on_shrink".to_string(), "true".to_string())]),
        ..Default::default()
    });
    assert!(
        from_flag.terminal_clear_on_shrink(),
        "a flag outranks the environment (FR-CFG-1)"
    );
}

// Verifies: gh #538 - a value that is not a boolean is refused at load, naming
// the key, like every other key in the inventory (the gh #273 rule: nothing is
// silently ignored).
#[test]
fn a_non_boolean_clear_on_shrink_is_refused_at_load_naming_the_key() {
    let dir = scratch("lca-clear-on-shrink-bad");
    let file = dir.join("user.toml");
    std::fs::write(file.as_path(), "terminal.clear_on_shrink = \"yes\"\n").expect("write");
    let err = Config::load(&LoadInput {
        user_file: Some(file),
        ..Default::default()
    })
    .expect_err("a string is not a boolean");
    let text = err.to_string();
    assert!(
        text.contains("terminal.clear_on_shrink"),
        "the message names the key: {text}"
    );
    assert!(
        text.contains("expected a boolean"),
        "and says what was expected: {text}"
    );
}
