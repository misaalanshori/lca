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

// ===========================================================================
// The cycle half of #397 (this batch): value-preserving cycles and wizards.
//
// The tri-state fix above closed a one-way door on a *boolean*. The same defect
// existed for an enum: cycling a key whose file holds a value outside the preset
// list ran `position(...).unwrap_or(0)`, so a miss fell back to index 0 and the
// first press wrote the *second* preset - the user's hand-set `wheel_lines = 4`
// was gone from the file after one keypress, with no way back.
//
// These rows guard the two mechanism-shaped properties a behaviour row cannot
// see: that the cycle list is built *including* the current value, and that the
// wizard commits through the one persist seam rather than a writer of its own.
// ===========================================================================

// Verifies: gh #397 - **the cycle list is presets union the current value**, the
// pi `settings-selector.ts` shape (`...new Set([1, 2, 3, 5, 10, current])`).
//
// Asserted on the source because the property is "the list is built this way",
// not "this one row cycles to this one value": a future edit could special-case
// one key and pass a behaviour row while leaving every other enum clobbering.
#[test]
fn the_cycle_list_includes_the_value_the_file_holds() {
    let commands = read("crates/lca-ui/src/chat_setting_cycle.rs");
    let cycle = commands
        .split("fn cycle_options_with(")
        .nth(1)
        .and_then(|rest| rest.split("fn next_cycle_value(").next())
        .expect("cycle_options exists");
    assert!(
        cycle.contains("options.push(row.value.clone())"),
        "the current value is added to the list, so a custom value is a stop:\n{cycle}"
    );
    assert!(
        cycle.contains("!row.masked"),
        "and a masked secret's `********` is not treated as a stop (gh #218):\n{cycle}"
    );
    // The defect itself, named: a miss must never be silently "index 0".
    assert!(
        !cycle.contains("unwrap_or(0)"),
        "the cycle no longer falls back to index 0 on a miss:\n{cycle}"
    );
}

// Verifies: gh #397 - **`default` persists as a key removal**. The `default` stop
// is the round trip's other end, and writing it as a magic string would put a
// value in the file that the loader does not know - so it reuses
// `persist_setting(key, None)`, the same unset contract the tri-state's
// `inherit` uses.
#[test]
fn the_default_stop_writes_a_removal_not_a_value() {
    let commands = read("crates/lca-ui/src/chat_setting_cycle.rs");
    let persisted = commands
        .split("fn persisted_for(")
        .nth(1)
        .and_then(|rest| rest.split("fn cycle_setting(").next())
        .expect("persisted_for exists");
    assert!(
        persisted.contains("value != \"default\""),
        "`default` maps to no value, which is how a key is removed:\n{persisted}"
    );
    assert!(
        commands.contains("persist(&key, None)"),
        "and the cycle_setting path writes that removal through the shared hook"
    );
}

// Verifies: gh #397 - **the wizard holds no writer of its own**. Design point 3:
// it commits through the same `persist_setting` seam `/settings` uses. A second
// writer is the drift this repo has closed three times (#403's formatter, #456's
// grant mapping, #407's stop/run/start), so the guard is that the type *names*
// the seam rather than owning one.
#[test]
fn the_wizard_commits_through_the_shared_setting_seam() {
    let pickers = read("crates/lca-ui/src/chat_pickers.rs");
    let commit = pickers
        .split("pub fn commit(")
        .nth(1)
        .and_then(|rest| rest.split("pub fn persist_through(").next())
        .expect("commit exists");
    assert!(
        commit.contains("Vec<(String, Option<String>)>"),
        "the wizard returns the writes instead of performing them:\n{commit}"
    );
    // It must not reach for a config file or a second persistence route.
    for forbidden in ["std::fs::write", "config.toml", "fs::File"] {
        assert!(
            !pickers.contains(forbidden),
            "the picker module never touches the file itself (`{forbidden}`)"
        );
    }
    let keys = read("crates/lca-ui/src/chat_keys.rs");
    assert!(
        keys.contains("self.apply_setting(&wizard_key, &value)"),
        "and the key path commits through `apply_setting`, the `/settings` writer"
    );
}

// Verifies: gh #397 - **a wizard is atomic at the keyboard**, which is the half
// the brief says can do damage. The `esc` arm must not call the commit path, and
// the commit path must exist only under the finishing `enter`.
//
// Source-level because "which arm writes" is exactly the property: a behaviour
// row that cancels would pass even if a *third* arm also wrote.
#[test]
fn the_escape_arm_cancels_instead_of_committing() {
    let keys = read("crates/lca-ui/src/chat_keys.rs");
    let wizard = keys
        .split("self.world.wizard.take()")
        .nth(1)
        .and_then(|rest| {
            rest.split("if let Some(mut picker) = self.settings_picker.take()")
                .next()
        })
        .expect("the wizard key arm exists");
    let escape_at = wizard
        .find("Some(\"escape\")")
        .expect("the escape arm exists");
    let commit_at = wizard
        .find("wizard.commit()")
        .expect("the commit call exists");
    assert!(
        escape_at < commit_at,
        "the escape arm is a separate arm from the commit arm"
    );
    // The text between the escape arm and the next arm must not commit.
    let after_escape = &wizard[escape_at..];
    let escape_arm_end = after_escape
        .find("Some(\"left\")")
        .expect("the left arm follows escape");
    assert!(
        !after_escape[..escape_arm_end].contains("commit()"),
        "escaping never commits - not even the steps already confirmed:\n{}",
        &after_escape[..escape_arm_end]
    );
    assert!(
        after_escape[..escape_arm_end].contains("wizard.cancel()"),
        "it cancels the whole wizard"
    );
}
