//! Gh #273: unknown configuration keys were silently ignored, and
//! `[provider]` retry keys were parsed twice.
//!
//! Two defects in the TOML layer:
//!
//! 1. `apply_toml_layer` walks a fixed list of dotted keys and looks each up.
//!    A key *outside* that list is never seen by the loop at all, so the
//!    `other =>` arm that returns `UnknownKey` is dead code — a typo like
//!    `compaction.theshold = 0.5` loads clean and does nothing. That is the
//!    worst failure shape for a config file: the user believes they changed
//!    a setting, and the tool agrees with them by silence.
//! 2. `[provider] retry_limit = N` / `[provider] retry_base_delay_ms = N`
//!    were applied twice: once through `table_value`'s section form (which
//!    gained subtable support later) and once through the hand-written block
//!    after the loop. Identical values, so it was invisible — but it is two
//!    code paths for one key, which is how the next drift starts.
//!
//! Verifies: gh #273 acceptance 3-4.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::collections::BTreeMap;

use lca_config::{Config, ConfigError, LoadInput};

fn scratch(name: &str) -> std::path::PathBuf {
    lca_testkit::scratch_path(name)
}

/// Load a user file holding `body`.
fn load_user(name: &str, body: &str) -> Result<Config, ConfigError> {
    let dir = scratch(name);
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(dir.join("user.toml"), body).expect("write");
    Config::load(&LoadInput {
        user_file: Some(dir.join("user.toml")),
        ..Default::default()
    })
}

/// The failure message, for assertion readability.
fn message(err: ConfigError) -> String {
    err.to_string()
}

// Verifies: gh #273 acceptance 3 - a typo'd leaf fails loud, naming the key.
// Before the fix this loaded clean and changed nothing.
#[test]
fn a_typo_in_a_top_level_key_fails_loudly_and_names_it() {
    let err = load_user("273-typo", "compaction.theshold = 0.5\n")
        .expect_err("a typo'd key must be refused");
    let text = message(err);
    assert!(
        text.contains("compaction.theshold"),
        "the error must name the offending key, got: {text}"
    );
}

// Verifies: gh #273 acceptance 3 - a wholly invented key is refused the same
// way, so "I read about it somewhere" gets an answer instead of silence.
#[test]
fn an_invented_key_fails_loudly() {
    let err = load_user("273-invented", "ui.colour = \"never\"\n")
        .expect_err("an unknown key must be refused");
    let text = message(err);
    assert!(text.contains("ui.colour"), "names the key: {text}");
}

// Verifies: gh #273 acceptance 3 - the error copy says what to DO. Naming the
// key alone leaves the user guessing whether the file is wrong or the binary
// is old, and the second is a real case (see the compat note in the report).
#[test]
fn the_error_copy_says_what_to_check() {
    let err = load_user("273-copy", "tool.timeout_second = 5\n").expect_err("refused");
    let text = message(err);
    assert!(
        text.contains("tool.timeout_second"),
        "names the key: {text}"
    );
    let lower = text.to_lowercase();
    assert!(
        lower.contains("check") || lower.contains("spelling") || lower.contains("newer"),
        "the copy should point at what to check, got: {text}"
    );
}

// Verifies: gh #273 acceptance 3 - an unknown leaf INSIDE a documented
// subtree is caught too. A walk that only checked top-level segment names
// would pass `[ui] colur = "never"` straight through.
#[test]
fn an_unknown_leaf_inside_a_documented_subtree_fails_loudly() {
    let err = load_user("273-subtree", "[ui]\ncolur = \"never\"\n")
        .expect_err("an unknown leaf under [ui] must be refused");
    let text = message(err);
    assert!(
        text.contains("ui.colur") || text.contains("colur"),
        "names the leaf: {text}"
    );
}

// Verifies: gh #273 acceptance 3 - and deeper still: the walk must reach a
// leaf under a subtree of a subtree, not stop at the first table.
#[test]
fn an_unknown_leaf_two_levels_deep_fails_loudly() {
    let err = load_user("273-deep", "[thinking.budgets]\nlow = 3000\nlwo = 5\n")
        .expect_err("an unknown level name under [thinking.budgets] must be refused");
    let text = message(err);
    assert!(text.contains("lwo"), "names the leaf: {text}");
}

// Verifies: gh #273 acceptance 3 - a valid config with every documented key
// still loads, so the walk is not simply refusing everything.
#[test]
fn a_valid_config_still_loads_cleanly() {
    let config = load_user(
        "273-valid",
        r#"
model = "m1"
compaction.threshold = 0.5
compaction.strategy = "summarize"
provider.retry_limit = 5
provider.retry_base_delay_ms = 100
provider.retry_max_delay_ms = 5000
queue.steering_mode = "one-at-a-time"
queue.follow_up_mode = "all"
skills.inject_matched = true
ui.theme = "dark"

[thinking.budgets]
low = 3000

[models.thinking_levels]
"m1" = ["low", "high"]

[[extensions.catalogs]]
id = "acme"
name = "Acme"
url = "https://example.invalid/c.toml"
"#,
    )
    .expect("a fully documented config must load");
    assert_eq!(config.compaction_strategy(), Some("summarize"));
    assert!(config.skills_inject_matched());
}

// Verifies: gh #273 acceptance 4 - `[provider]` retry keys parsed through the
// section form give the same values as the dotted form. The duplicate block
// was removed; this row pins that removal did not change any value.
#[test]
fn provider_section_retry_keys_match_their_dotted_form() {
    let dotted = load_user(
        "273-dotted",
        "provider.retry_limit = 7\nprovider.retry_base_delay_ms = 111\n",
    )
    .expect("dotted form loads");
    let section = load_user(
        "273-section",
        "[provider]\nretry_limit = 7\nretry_base_delay_ms = 111\n",
    )
    .expect("section form loads");
    assert_eq!(dotted.provider_retry_limit(), 7);
    assert_eq!(section.provider_retry_limit(), 7);
    assert_eq!(dotted.provider_retry_base_delay_ms(), 111);
    assert_eq!(section.provider_retry_base_delay_ms(), 111);
}

// Verifies: gh #273 acceptance 4 - the section form also carries the max-delay
// key, which the deleted duplicate block never handled. This is the row that
// would have been red had the duplicate been the only path.
#[test]
fn the_provider_section_carries_every_retry_key() {
    let config = load_user(
        "273-section-all",
        "[provider]\nretry_limit = 9\nretry_base_delay_ms = 22\nretry_max_delay_ms = 333\n",
    )
    .expect("section form loads");
    assert_eq!(config.provider_retry_limit(), 9);
    assert_eq!(config.provider_retry_base_delay_ms(), 22);
    assert_eq!(config.provider_retry_max_delay_ms(), 333);
}

// Verifies: gh #273 acceptance 3 - `provider = "name"` (a leaf) and
// `[provider]` (a table) cannot coexist in one TOML document, so the walk must
// not treat a provider *string* as an unvisited table. The leaf is a
// documented key; only the table shape is a section.
#[test]
fn a_provider_string_leaf_is_a_documented_key_not_a_section() {
    let config = load_user("273-provider-leaf", "provider = \"acme\"\n")
        .expect("a provider string is the documented leaf form");
    assert_eq!(config.provider(), "acme");
}

// Verifies: gh #273 - `[thinking.budgets]` is a documented table, so its
// parent key `thinking` is a section name, not an unknown key. Both shapes
// must load (they cannot coexist in one document).
#[test]
fn the_thinking_subtree_is_a_documented_section() {
    let budgets = load_user("273-thinking-table", "[thinking.budgets]\nlow = 100\n")
        .expect("the budgets table loads");
    assert!(budgets.budget_for_level(Some("low")).is_some());
    let leaf =
        load_user("273-thinking-leaf", "thinking = \"high\"\n").expect("the thinking leaf loads");
    assert_eq!(leaf.thinking(), Some("high"));
}

// Verifies: gh #273 (FR-PERM-9) - an unknown key in an UNTRUSTED project file
// neither fails nor takes effect: the file is not read at all. This is the
// DoS boundary: a hostile repo must not be able to brick startup with a junk
// key, and it cannot, because the file never reaches the parser.
#[test]
fn an_untrusted_project_file_with_a_junk_key_does_not_fail_startup() {
    let dir = scratch("273-untrusted");
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(
        dir.join("project.toml"),
        "this.key.does.not.exist = 1\ncompaction.theshold = 0.5\n",
    )
    .expect("write");
    let config = Config::load(&LoadInput {
        project_file: Some(dir.join("project.toml")),
        trusted: false,
        ..Default::default()
    })
    .expect("an untrusted project file is ignored, junk keys included");
    assert_eq!(config.provider(), "openai-compatible");
}

// Verifies: gh #273 (FR-PERM-9) - and in a TRUSTED project file a junk key
// fails like any other, because the file participates and the user has opted
// into it. The trust prompt is what separates the two cases.
#[test]
fn a_trusted_project_file_with_a_junk_key_fails_loudly() {
    let dir = scratch("273-trusted");
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(dir.join("project.toml"), "compaction.theshold = 0.5\n").expect("write");
    let err = Config::load(&LoadInput {
        project_file: Some(dir.join("project.toml")),
        trusted: true,
        ..Default::default()
    })
    .expect_err("a trusted project file participates, so its junk key is refused");
    assert!(message(err).contains("compaction.theshold"));
}

// Verifies: gh #273 - a flag or environment key still resolves; the walk is a
// file-layer check and must not reject the other layers' vocabulary.
#[test]
fn the_flags_and_environment_still_apply() {
    let config = Config::load(&LoadInput {
        flags: BTreeMap::from([("ui.theme".to_string(), "dark".to_string())]),
        env: BTreeMap::from([("LCA_COMPACTION_THRESHOLD".to_string(), "0.25".to_string())]),
        ..Default::default()
    })
    .expect("flags and env load");
    assert_eq!(config.ui_theme(), Some("dark"));
    assert!((config.compaction_threshold() - 0.25).abs() < f64::EPSILON);
}

// Verifies: gh #273 (a limitation found while writing these rows, NOT
// introduced by this cycle) - `provider` is both a leaf (`provider =
// "name"`) and a section (`[provider] retry_limit = N`), and TOML forbids
// reusing one path as both. So a file that names its provider cannot also
// carry a retry key. Recorded here so the constraint is visible; the retry
// keys remain reachable through `LCA_PROVIDER_RETRY_*`.
#[test]
fn the_provider_leaf_and_section_cannot_share_a_document() {
    let err = load_user(
        "273-provider-both",
        "provider = \"acme\"\n\n[provider]\nretry_limit = 5\n",
    )
    .expect_err("TOML refuses one path as both a string and a table");
    // The failure is the parser's, before the vocabulary walk: this is a
    // shape rule, not an unknown key.
    assert!(
        matches!(err, ConfigError::Parse { .. }),
        "expected a TOML parse error, got {err:?}"
    );
    // The workaround: the environment form.
    let config = Config::load(&LoadInput {
        env: BTreeMap::from([("LCA_PROVIDER_RETRY_LIMIT".to_string(), "5".to_string())]),
        ..Default::default()
    })
    .expect("the environment form works");
    assert_eq!(config.provider_retry_limit(), 5);
}

// Verifies: gh #273 - an empty table is legal TOML and changes nothing, so a
// section head with nothing under it is accepted: `[models]` alone must not
// fail a load. Refusing it would be hostile to a file someone is halfway
// through editing.
#[test]
fn an_empty_documented_section_is_harmless() {
    let config =
        load_user("273-empty-section", "[models]\n").expect("an empty section head is not a typo");
    assert!(config.models_enabled().is_empty());
}

// Verifies: gh #273 - but an empty table whose name heads nothing IS a
// misspelling of a section, and gets reported. This is the case where the
// empty shape carries the whole signal.
#[test]
fn an_empty_undocumented_section_is_a_misspelling_and_is_reported() {
    let err = load_user("273-empty-typo", "[modles]\n").expect_err("not a section");
    assert!(err.to_string().contains("modles"), "{err}");
}
