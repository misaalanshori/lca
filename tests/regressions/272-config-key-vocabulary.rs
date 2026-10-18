//! Gh #272: `KNOWN_KEYS` and `Config::resolved()` are two hand-maintained
//! mirrors of the configuration vocabulary, and both had drifted.
//!
//! - `skills.inject_matched` was missing from `KNOWN_KEYS`. That list is what
//!   `Config::load` iterates to map `LCA_*` environment variables, so
//!   `LCA_SKILLS_INJECT_MATCHED=true` parsed (the parser knew the key) but the
//!   loop never reached it: the variable silently did nothing. A user setting
//!   it got the catalog behavior with no error and no notice.
//! - Seven settings were missing from `resolved()`: `compaction.strategy`,
//!   `provider.retry_base_delay_ms`, `provider.retry_max_delay_ms`,
//!   `queue.steering_mode`, `queue.follow_up_mode`, `thinking.budgets`, and
//!   `extensions.catalogs`. FR-CFG-2 says `lca config` prints every resolved
//!   key with the source that set it; these seven were invisible, so a user
//!   debugging a setting they *did* set got no row at all.
//!
//! Verifies: gh #272 acceptance 1-2 at the crate's public surface.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::collections::BTreeMap;

use lca_config::{Config, KNOWN_KEYS, LoadInput, MergeSource};

// Verifies: gh #272 acceptance 1 - the environment variable is live. Before
// the fix this parsed at the crate level but never reached `Config::apply`,
// because the loop that walks the environment reads `KNOWN_KEYS`.
#[test]
fn the_skills_injection_environment_variable_is_not_inert() {
    let config = Config::load(&LoadInput {
        env: BTreeMap::from([("LCA_SKILLS_INJECT_MATCHED".to_string(), "true".to_string())]),
        ..Default::default()
    })
    .expect("load");
    assert!(
        config.skills_inject_matched(),
        "LCA_SKILLS_INJECT_MATCHED=true must reach the merged config"
    );
    assert_eq!(
        config.source_of("skills.inject_matched"),
        MergeSource::Env,
        "the winning source must be the environment"
    );
}

// Verifies: gh #272 - the key is in the list that drives environment mapping,
// so `LCA_SKILLS_INJECT_MATCHED` can be *discovered* at all. This is the
// direct assertion on the defect: membership, not behavior.
#[test]
fn the_key_is_in_the_environment_vocabulary() {
    assert!(
        KNOWN_KEYS.contains(&"skills.inject_matched"),
        "skills.inject_matched must be in KNOWN_KEYS; it is what the env loop iterates"
    );
}

// Verifies: gh #272 acceptance 2 - each of the seven previously missing
// settings has a row in `resolved()`, which is what `lca config` and
// `/settings` both print.
#[test]
fn the_seven_previously_missing_settings_are_reported() {
    let config = Config::defaults();
    let reported: Vec<&str> = config.resolved().map(|(key, _, _)| key).collect();
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
            reported.contains(&key),
            "`{key}` must appear in resolved(); reported keys: {reported:?}"
        );
    }
}

// Verifies: gh #272 acceptance 2 - and they report the *winning source*, so a
// user can tell a file value from a default. A row with a value but a wrong
// source would still under-inform.
#[test]
fn the_seven_report_where_their_value_came_from() {
    let dir = lca_testkit::scratch_path("272-regression-sources");
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
        let (_, _, source) = config
            .resolved()
            .find(|(name, _, _)| *name == key)
            .unwrap_or_else(|| panic!("`{key}` missing from resolved()"));
        assert_eq!(
            source,
            MergeSource::UserFile,
            "`{key}` must name the user file as its source"
        );
    }
}

// Verifies: gh #272 - a default-valued row still carries a usable display
// value rather than an empty string, matching the convention the older rows
// (`model`, `ui.theme`, `permissions.proposals`) established.
#[test]
fn an_unset_row_shows_a_placeholder_not_an_empty_string() {
    let config = Config::defaults();
    let value_of = |key: &str| {
        config
            .resolved()
            .find(|(name, _, _)| *name == key)
            .map(|(_, value, _)| value)
            .unwrap_or_else(|| panic!("`{key}` missing"))
    };
    for key in [
        "compaction.strategy",
        "thinking.budgets",
        "extensions.catalogs",
    ] {
        let value = value_of(key);
        assert!(
            value.starts_with('<') && value.ends_with('>'),
            "`{key}` placeholder must be `<...>`-shaped, got {value:?}"
        );
    }
}
