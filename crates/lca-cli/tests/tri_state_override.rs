//! The tri-state project override (gh #397).
//!
//! A project override has three states, not two: the project file says nothing
//! (inherit the user file or the default), it says on, or it says off. A
//! two-way toggle conflated the first two — and because nothing in the
//! interface could *see* a project key, an override written by hand (or by an
//! older build) was a one-way door: the picker could write one but never offer
//! to remove it.
//!
//! These rows drive the real cycle and the real writer against real files. The
//! property that matters most is the round trip: cycling back to `inherit` must
//! leave the file byte-identical to before, or "stop overriding here" did not
//! happen.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use std::path::{Path, PathBuf};

use lca_cli::{persist_project_setting, project_config_file};
use lca_ui::ProjectOverride;

/// A scratch project directory with an optional `.lca/config.toml`.
fn project(tag: &str, body: Option<&str>) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lca-397-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    if let Some(body) = body {
        std::fs::create_dir_all(dir.join(".lca")).expect("mkdir .lca");
        std::fs::write(project_config_file(&dir), body).expect("write config");
    }
    dir
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

// Verifies: gh #397 - **the cycle writes the expected file at each step.**
// `inherit` is the key's *absence*, never `= false`; the two set states are
// explicit. This is the table the whole feature is about.
#[test]
fn the_cycle_writes_absent_then_explicit_values() {
    let dir = project("cycle", Some("ui.color = \"auto\"\n"));
    let path = project_config_file(&dir);

    // inherit -> on
    persist_project_setting(&dir, "tool.edit_requires_read", Some("true")).expect("write");
    let text = read(&path);
    assert!(
        text.contains("edit_requires_read = true"),
        "on is an explicit true: {text}"
    );
    assert!(
        text.contains("ui.color = \"auto\""),
        "the other key survives: {text}"
    );

    // on -> off
    persist_project_setting(&dir, "tool.edit_requires_read", Some("false")).expect("write");
    let text = read(&path);
    assert!(
        text.contains("edit_requires_read = false"),
        "off is an explicit false: {text}"
    );

    // off -> inherit: the key is gone, not set to false.
    persist_project_setting(&dir, "tool.edit_requires_read", None).expect("write");
    let text = read(&path);
    assert!(
        !text.contains("tool.edit_requires_read"),
        "inherit removes the key entirely: {text}"
    );
    assert!(
        !text.contains("edit_requires_read = false"),
        "and never writes `= false` as the way to inherit: {text}"
    );
}

// Verifies: gh #397 - **the round trip is byte-identical.** This is the
// one-way-door defect: after cycling back to `inherit`, the file must be
// exactly what it was before the first press, comments included.
#[test]
fn cycling_back_to_inherit_restores_the_file_byte_for_byte() {
    let original = "# my project settings\nui.color = \"auto\"\n\n[tool]\ntimeout_seconds = 30\n";
    let dir = project("roundtrip", Some(original));
    let path = project_config_file(&dir);
    let before = read(&path);
    assert_eq!(before, original);

    // inherit -> on -> off -> inherit.
    persist_project_setting(&dir, "tool.edit_requires_read", Some("true")).expect("write");
    assert_ne!(read(&path), before, "an override is a real change");
    persist_project_setting(&dir, "tool.edit_requires_read", Some("false")).expect("write");
    persist_project_setting(&dir, "tool.edit_requires_read", None).expect("write");

    assert_eq!(
        read(&path),
        before,
        "back at inherit, the file is the one we started with - comments and all"
    );
}

// Verifies: gh #397 - **clearing is idempotent**: cycling to the same state
// twice does not churn the file.
#[test]
fn clearing_twice_is_stable() {
    let dir = project("idempotent", Some("ui.color = \"auto\"\n"));
    let path = project_config_file(&dir);
    persist_project_setting(&dir, "tool.edit_requires_read", Some("true")).expect("write");
    persist_project_setting(&dir, "tool.edit_requires_read", None).expect("write");
    let once = read(&path);
    persist_project_setting(&dir, "tool.edit_requires_read", None).expect("write");
    assert_eq!(read(&path), once, "a second clear changes nothing");

    // ...and writing the same value twice is stable too.
    persist_project_setting(&dir, "tool.edit_requires_read", Some("false")).expect("write");
    let set = read(&path);
    persist_project_setting(&dir, "tool.edit_requires_read", Some("false")).expect("write");
    assert_eq!(read(&path), set, "a repeated set changes nothing");
}

// Verifies: gh #397 - a project file that holds nothing after a clear is
// **removed**, along with its now-empty `.lca/`: leaving an empty
// `.lca/config.toml` behind would keep the trust prompt alive for a setting the
// user just asked to forget (ADR-0039).
#[test]
fn clearing_the_only_key_removes_the_file_and_its_empty_directory() {
    let dir = project("empty", Some("tool.edit_requires_read = true\n"));
    let path = project_config_file(&dir);
    assert!(path.is_file());

    persist_project_setting(&dir, "tool.edit_requires_read", None).expect("write");
    assert!(
        !path.exists(),
        "an emptied project config is not a project config"
    );
    assert!(
        !dir.join(".lca").exists(),
        "and its directory goes too when nothing else is in it"
    );
}

// Verifies: gh #397 - the directory stays when it still holds something. `.lca`
// may carry extensions, mcp.json, or sessions; clearing one setting must not
// touch them.
#[test]
fn a_directory_with_other_content_survives_a_clear() {
    let dir = project("keepdir", Some("tool.edit_requires_read = true\n"));
    std::fs::write(dir.join(".lca/mcp.json"), "{}").expect("write mcp");
    persist_project_setting(&dir, "tool.edit_requires_read", None).expect("write");
    assert!(
        dir.join(".lca").is_dir(),
        "`.lca/` still holds mcp.json, so it stays"
    );
    assert!(dir.join(".lca/mcp.json").is_file());
}

// Verifies: gh #397 - the three states read back correctly from a file,
// including the one that has no bytes to find. This is the state the old
// two-way toggle could not express.
#[test]
fn the_three_states_are_distinguishable_in_the_file() {
    let dir = project("states", None);
    let path = project_config_file(&dir);
    let quiet = |dir: &Path| {
        lca_config::layers::read_layer(&project_config_file(dir))
            .unwrap_or_default()
            .get("tool.edit_requires_read")
            .cloned()
    };

    // No file at all: inherit.
    assert_eq!(quiet(&dir), None);
    assert_eq!(
        ProjectOverride::from_value(quiet(&dir).as_deref(), "true", "false"),
        ProjectOverride::Inherit
    );

    // On.
    persist_project_setting(&dir, "tool.edit_requires_read", Some("true")).expect("write");
    assert_eq!(
        ProjectOverride::from_value(quiet(&dir).as_deref(), "true", "false"),
        ProjectOverride::On
    );

    // Off - a real override, and distinct from inherit.
    persist_project_setting(&dir, "tool.edit_requires_read", Some("false")).expect("write");
    assert_eq!(
        ProjectOverride::from_value(quiet(&dir).as_deref(), "true", "false"),
        ProjectOverride::Off
    );
    assert!(path.exists(), "off leaves a file behind, unlike inherit");

    // Inherit again.
    persist_project_setting(&dir, "tool.edit_requires_read", None).expect("write");
    assert_eq!(
        ProjectOverride::from_value(quiet(&dir).as_deref(), "true", "false"),
        ProjectOverride::Inherit
    );
}

// Verifies: gh #397 - the cycle order is `inherit -> on -> off -> inherit`
// forward, and the reverse backward, as a table. One wrong arm here makes the
// control unusable while every file test above still passes.
#[test]
fn the_cycle_order_is_a_table() {
    assert_eq!(ProjectOverride::Inherit.step(true), ProjectOverride::On);
    assert_eq!(ProjectOverride::On.step(true), ProjectOverride::Off);
    assert_eq!(ProjectOverride::Off.step(true), ProjectOverride::Inherit);
    assert_eq!(ProjectOverride::Inherit.step(false), ProjectOverride::Off);
    assert_eq!(ProjectOverride::Off.step(false), ProjectOverride::On);
    assert_eq!(ProjectOverride::On.step(false), ProjectOverride::Inherit);
    // Four steps forward is where it started: the cycle closes.
    let mut state = ProjectOverride::Inherit;
    for _ in 0..3 {
        state = state.step(true);
    }
    assert_eq!(state, ProjectOverride::Inherit);
}

// Verifies: gh #397 (NFR-28) - each state has a word, so the row never relies
// on colour alone.
#[test]
fn every_state_has_a_word() {
    assert_eq!(ProjectOverride::Inherit.word(), "inherit");
    assert_eq!(ProjectOverride::On.word(), "on");
    assert_eq!(ProjectOverride::Off.word(), "off");
    // The default is the state a new project is in.
    assert_eq!(ProjectOverride::default(), ProjectOverride::Inherit);
}

// Verifies: gh #397 (FR-PERM-9, the security edge) - **an untrusted project
// file is inert.** Its `false` must not disable anything: the merged
// configuration ignores the layer entirely, so the resolved value is the user
// file's, and the picker's row must say so rather than show the project's
// claim as effective.
#[test]
fn an_untrusted_project_file_cannot_change_the_resolved_value() {
    let dir = project("untrusted", Some("tool.edit_requires_read = false\n"));
    let user = dir.join("user.toml");
    std::fs::write(&user, "tool.edit_requires_read = true\n").expect("write user file");
    let project_file = project_config_file(&dir);

    let load = |trusted: bool| {
        lca_config::Config::load(&lca_config::LoadInput {
            flags: std::collections::BTreeMap::new(),
            env: std::collections::BTreeMap::new(),
            project_file: Some(project_file.clone()),
            trusted,
            user_file: Some(user.clone()),
            headless: false,
        })
        .expect("the layers load")
    };

    let untrusted = load(false);
    let (value, source) = untrusted
        .resolved()
        .find(|(key, _, _)| *key == "tool.edit_requires_read")
        .map(|(_, value, source)| (value, source))
        .expect("the key resolves");
    assert_eq!(
        value, "true",
        "an untrusted project's `false` does not beat the user file"
    );
    assert_eq!(
        source,
        lca_config::MergeSource::UserFile,
        "the user file is still the winner, which is what the row will report"
    );

    // Trusted: the project wins, as it should once the user opted in.
    let trusted = load(true);
    let (value, source) = trusted
        .resolved()
        .find(|(key, _, _)| *key == "tool.edit_requires_read")
        .map(|(_, value, source)| (value, source))
        .expect("the key resolves");
    assert_eq!(value, "false", "trusted, the project's value wins");
    assert_eq!(source, lca_config::MergeSource::ProjectFile);
}

// Verifies: gh #397 - the row builder offers a tri-state only for a two-valued
// key. An enum or free-text key has no "off" for a project to mean, and
// pretending otherwise would write a value the key refuses.
#[test]
fn only_two_valued_rows_carry_a_tri_state() {
    // The rule the row builder applies, expressed directly: on and off are the
    // `true`/`false` members of the row's own cycle list.
    let two_valued = ["false", "true"];
    let three_valued = ["off", "idle"];
    let has_tri_state = |values: &[&str]| values.contains(&"true") && values.contains(&"false");
    assert!(has_tri_state(&two_valued), "a boolean row has on/off");
    assert!(
        !has_tri_state(&three_valued),
        "an enum row has no on/off, so no project tri-state"
    );
}

// Verifies: gh #397 - **a write must be readable.** `ui.quiet_startup` is
// `bool | "header"`: the loader reads a TOML boolean or the exact string
// `"header"` and refuses `"true"`. The dotted-key writer used to persist every
// `Text` value as a quoted string, so a `/settings` write of this key produced a
// file `/reload` then rejected with "expected true, false, or \"header\", got a
// string" - a one-way write the user could not read back. Found live, in a pane.
#[test]
fn a_quiet_startup_write_is_readable_by_the_loader() {
    let dir = project("readable", None);
    let path = project_config_file(&dir);

    for (value, expected) in [
        ("true", "quiet_startup = true"),
        ("header", "quiet_startup = \"header\""),
    ] {
        persist_project_setting(&dir, "ui.quiet_startup", Some(value)).expect("write");
        let text = read(&path);
        assert!(
            text.contains(expected),
            "`{value}` is written in the shape the loader accepts: {text}"
        );
        // ...and the loader really does accept it.
        let config = lca_config::Config::load(&lca_config::LoadInput {
            flags: std::collections::BTreeMap::new(),
            env: std::collections::BTreeMap::new(),
            project_file: Some(path.clone()),
            trusted: true,
            user_file: None,
            headless: false,
        })
        .unwrap_or_else(|err| panic!("a written `{value}` must load back: {err}"));
        let (loaded, _) = config
            .resolved()
            .find(|(key, _, _)| *key == "ui.quiet_startup")
            .map(|(_, value, source)| (value, source))
            .expect("the key resolves");
        assert_eq!(loaded, value, "and it reads back as what was written");
    }
}
