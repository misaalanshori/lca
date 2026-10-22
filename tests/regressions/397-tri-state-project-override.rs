//! GitHub #397 (tri-state slice): a project override you can see and clear.
//!
//! A project override has three states, not two:
//!
//! | State | Project file holds | Meaning |
//! | --- | --- | --- |
//! | inherit | the key is **absent** | use the user file / the default |
//! | on | `key = true` | this project turns it on |
//! | off | `key = false` | this project turns it off |
//!
//! Conflating *absent* with *false* is the defect. Before this slice nothing in
//! the interface could even **read** the project layer, so a key written there
//! (by hand, or by an older build) was a one-way door: no row could show it and
//! no key could remove it. `docs/configuration.md` documents the ladder that
//! makes the distinction matter — a project value beats the user file, so
//! "stop overriding here" has to mean removal, not an explicit `false` that
//! silently keeps beating it.
//!
//! The behaviour lives in `crates/lca-cli/tests/tri_state_override.rs` (the
//! writer, the byte-identical round trip, the trust gate against real merged
//! configuration) and `crates/lca-ui/src/chat_project_override_tests.rs` (the
//! picker). This is the NFR-24 named guard: it fails if the mechanism is
//! quietly reverted to a two-way toggle.

use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(rel: &str) -> String {
    let path = root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {path:?}: {err}"))
}

// Verifies: gh #397 - **the three states exist as a type**, with `inherit` the
// default, so a row that has never been set says so rather than starting at
// "off".
#[test]
fn the_tri_state_is_a_three_armed_type() {
    let state = read("crates/lca-ui/src/state.rs");
    let start = state
        .find("pub enum ProjectOverride {")
        .expect("ProjectOverride exists");
    let rest = &state[start..];
    let end = rest.find("\n}\n").expect("the enum has a body");
    let body = &rest[..end];
    for arm in ["Inherit", "On", "Off"] {
        assert!(body.contains(arm), "the `{arm}` state exists");
    }
    assert!(
        body.contains("#[default]"),
        "`inherit` is the default: a project with no opinion is where a new \
         project starts, not `off`"
    );
    // `word()` lives in the impl block just below the enum, not inside it.
    assert!(
        state.contains("pub fn word(self) -> &'static str"),
        "each state has a word (NFR-28: never colour alone)"
    );
}

// Verifies: gh #397 - **inherit is the key's absence.** The writer must pass
// `None` to the shared dotted-key writer for `inherit`, because `None` is what
// removes the key; a two-way toggle passed `Some("false")` and could never
// clear.
#[test]
fn inherit_writes_the_key_removal_not_a_false() {
    // The cycle moved to its own module when `chat_commands.rs` crossed the
    // 1,200-line ceiling; the guard follows the code.
    let commands = read("crates/lca-ui/src/chat_project_override.rs");
    let start = commands
        .find("fn cycle_project_override(")
        .expect("the cycle exists");
    let rest = &commands[start..];
    let end = rest.find("\n    }\n").expect("the cycle has a body");
    let body = &rest[..end];
    assert!(
        body.contains("ProjectOverride::Inherit => None"),
        "`inherit` must map to the writer's `None` (remove), never to a value"
    );
    assert!(
        body.contains("ProjectOverride::On => Some(on)")
            && body.contains("ProjectOverride::Off => Some(off)"),
        "the two set states map to real values"
    );

    let persist = read("crates/lca-cli/src/persist.rs");
    assert!(
        persist.contains("pub fn persist_project_setting("),
        "the project layer has its own writer - it is a different file from \
         the user config, with different consequences"
    );
    assert!(
        persist.contains("file_is_empty"),
        "a clear that empties the file removes it: an empty \
         `.lca/config.toml` would still trip the trust prompt (ADR-0039)"
    );
}

// Verifies: gh #397 - **the project layer is read on its own.** Merged
// configuration can only say what won; the row needs to know whether the
// project file mentions the key at all, which is what makes the tri-state
// visible.
#[test]
fn the_project_layer_is_read_separately_from_the_merge() {
    let layers = read("crates/lca-config/src/layers.rs");
    assert!(
        layers.contains("pub fn read_layer("),
        "one layer reads on its own, distinct from `Config::load`"
    );
    assert!(
        layers.contains("/// `None` when the file does not exist"),
        "the inherit state is documented where it is produced"
    );
    let display = read("crates/lca-cli/src/tui/display.rs");
    assert!(
        display.contains("lca_config::layers::read_layer"),
        "the row builder reads the project layer"
    );
    assert!(
        display.contains("is_trusted_here(&ui.cwd)"),
        "and only while the project is trusted (FR-PERM-9): an untrusted \
         file is inert, so it must not appear as an override to change"
    );
}

// Verifies: gh #397 - **trust still gates it.** A project value cannot take
// effect until the project is trusted, because the merged configuration skips
// the layer entirely. The picker states that rather than pretending the write
// applied.
#[test]
fn an_untrusted_project_override_is_inert_and_says_so() {
    let trust = read("crates/lca-cli/src/trust.rs");
    assert!(
        trust.contains("trusted: grants.is_trusted_here(cwd)"),
        "the loader's trust bit is the project layer's gate"
    );
    let commands = read("crates/lca-ui/src/chat_project_override.rs");
    assert!(
        commands.contains("not trusted"),
        "the notice names the reason an override did not apply"
    );
    let state = read("crates/lca-ui/src/state.rs");
    assert!(
        state.contains("pub project_layer_trusted:"),
        "the interface asks the host for the trust bit instead of assuming it"
    );
}

// Verifies: gh #397 - the artist's route: a row shows its state as a word, and
// the hint names the key that cycles it. Without both the feature exists but is
// undiscoverable.
#[test]
fn the_row_names_the_state_and_the_hint_names_the_key() {
    let overlays = read("crates/lca-ui/src/chat_overlays.rs");
    assert!(
        overlays.contains("project {}, "),
        "the row spells the state"
    );
    assert!(
        overlays.contains("tab project"),
        "the hint names `tab` as the project-override key"
    );
    let keys = read("crates/lca-ui/src/chat_keys.rs");
    assert!(
        keys.contains("Some(\"tab\")"),
        "`tab` is the bound key, and it is separate from `Enter`'s session \
         cycle: they write different files"
    );
}

// Verifies: gh #397 - **only a two-valued row gets a tri-state.** An enum or
// free-text key has no "off" for a project to mean; offering one would write a
// value the key's own validator refuses.
#[test]
fn only_boolean_rows_offer_the_override() {
    let display = read("crates/lca-cli/src/tui/display.rs");
    assert!(
        display.contains("value == \"true\"") && display.contains("value == \"false\""),
        "the row builder looks for an on/off pair in the row's own cycle values"
    );
    assert!(
        display.contains("_ => None,"),
        "a row without that pair carries no project state"
    );
}
