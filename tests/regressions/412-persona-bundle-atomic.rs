//! gh #412 - `/persona` applies a bundle atomically, or nothing.
//!
//! A persona names a model, a thinking level, a tool loadout, and optional
//! instructions. The defect class this guard exists for is **half-application**: a
//! persona whose model resolves but whose thinking level does not would, in the
//! obvious implementation, switch the model and leave the level stale - and the user
//! would believe they are in a persona they are not in, with nothing on screen to
//! say otherwise.
//!
//! Two things make that impossible, and both are pinned here:
//!
//! 1. **The check runs before anything is written**, and a host without a check
//!    refuses rather than applying unverified fields.
//! 2. **The word.** `Preset` is reserved for provider endpoint entries
//!    (`docs/glossary.md`), so this feature is `/persona` and `[personas]` in the
//!    code, the config key, and the docs. A second spelling re-opens the collision
//!    that note exists to close.
//!
//! The behaviour rows are in `crates/lca-ui/src/chat_persona_tests.rs` (apply,
//! refuse-and-apply-nothing, list, unknown name, case-insensitivity, the notice) and
//! `crates/lca-config/tests/personas.rs` (parsing, partial personas, the provider
//! default, the refusals, precedence, trust, listing order).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failure.

use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &str) -> String {
    let path = root().join(path);
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()))
}

/// Every `.rs` file under the given trees, skipping test modules.
fn sources(trees: &[&str]) -> Vec<(PathBuf, String)> {
    fn walk(dir: &Path, out: &mut Vec<(PathBuf, String)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
                continue;
            }
            if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
                continue;
            }
            let name = path.to_string_lossy();
            if name.ends_with("_tests.rs") || name.ends_with("/tests.rs") {
                continue;
            }
            if let Ok(text) = std::fs::read_to_string(&path) {
                out.push((path, text));
            }
        }
    }
    let mut out = Vec::new();
    for tree in trees {
        walk(&root().join(tree), &mut out);
    }
    out
}

// Verifies: gh #412 design point 1 - the atomic ordering is structural.
//
// The interface validates the whole bundle through a seam that writes nothing, and
// only then calls the apply seam. A future "simplification" that applies first and
// validates afterwards would pass every other row in this cycle and half-apply a
// broken persona, so the *order* is what this pins.
#[test]
fn the_check_runs_before_any_application() {
    let persona = read("crates/lca-ui/src/chat_persona.rs");
    // The ordering that matters is inside `dispatch_persona`: the `check` call is
    // reached, and the `apply` call is *after* the refusal returns. A plain
    // `find(persona_check) < find(persona_apply)` is too weak - a first version
    // passed with an apply injected before the check, because the seam *names* also
    // appear in the "no seam" guards further down the function. So this slices the
    // function and looks at where the two calls actually are.
    let body = persona
        .split("pub(super) fn dispatch_persona")
        .nth(1)
        .expect("the dispatcher exists");
    let body = body
        .split("\n    /// The `/persona` listing")
        .next()
        .unwrap_or(body);
    let check = body
        .find("check(&bundle)")
        .expect("the dispatcher calls the check");
    let apply = body.find("apply(&bundle)").expect("and the apply");
    assert!(
        check < apply,
        "the check is reached before the apply inside dispatch_persona"
    );
    // And the refusal returns before the apply can be reached: a `return` between
    // them is what makes the ordering a guard rather than a sequence.
    let between = &body[check..apply];
    assert!(
        between.contains("return"),
        "a failed check returns before the apply seam is touched:\n{between}"
    );
    // And a host with no check refuses: an unverified bundle is never applied.
    assert!(
        persona.contains("cannot validate a persona; nothing was changed"),
        "a host without a check refuses rather than applying blind"
    );
    // The seam that validates writes nothing, so a refusal cannot leave a trace.
    // The seam types live in their own module (split for the ceiling, gate 11).
    let state = read("crates/lca-ui/src/state_persona.rs");
    let check_type = state
        .split("pub type PersonaCheck")
        .nth(1)
        .expect("the check seam exists")
        .split("\n\n")
        .next()
        .unwrap_or_default();
    assert!(
        check_type.contains("Result<(), String>"),
        "the check returns a verdict, not a side effect: {check_type}"
    );
}

// Verifies: gh #412 design point 2 - a persona writes through the existing paths.
//
// There is deliberately **no** "current persona" state cell: a cell that could
// disagree with the model, the level, and the loadout it describes is a second owner
// for the same facts (the gh #369 lesson). The footer, the border ramp, and the
// per-model overrides stay in sync because nothing new has to be kept in step.
#[test]
fn a_persona_adds_no_parallel_state_cell() {
    let state = read("crates/lca-ui/src/state.rs");
    // The three settings a persona drives already exist.
    for existing in ["pub thinking:", "pub model_label:", "pub set_thinking:"] {
        assert!(
            state.contains(existing),
            "the existing setting {existing} is what a persona writes through"
        );
    }
    // And no persona-named cell was added beside them.
    for forbidden in [
        "pub current_persona:",
        "pub active_persona:",
        "pub persona_state:",
    ] {
        assert!(
            !state.contains(forbidden),
            "{forbidden} would be a second owner for the model/level/loadout"
        );
    }
}

// Verifies: gh #412's binding terminology note - `Preset` stays reserved.
//
// The issue's own note says reusing it for personas collides with the login-picker
// vocabulary. This scans the feature's sources and the docs for the reserved word
// used as a *persona* name; the glossary's own definition and the provider-preset
// code are the only legitimate users.
#[test]
fn the_reserved_word_stays_reserved() {
    let mut offenders = Vec::new();
    for (path, text) in sources(&[
        "crates/lca-ui/src",
        "crates/lca-config/src",
        "crates/lca-cli/src/tui",
    ]) {
        for (index, line) in text.lines().enumerate() {
            let lower = line.to_lowercase();
            // A persona-spelled preset: the word next to the feature's vocabulary.
            if lower.contains("preset")
                && (lower.contains("persona") || lower.contains("/preset"))
                && !line.trim_start().starts_with("//")
            {
                offenders.push(format!("{}:{}: {line}", path.display(), index + 1));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "`Preset` is reserved for provider endpoints (docs/glossary.md); this feature \
         is `/persona` and `[personas]`:\n{}",
        offenders.join("\n")
    );
    // The glossary still says so, so the rule has a source rather than being a
    // memory.
    let glossary = read("docs/glossary.md");
    assert!(
        glossary.contains("**Preset.**"),
        "the glossary defines the reserved word"
    );
}

// Verifies: gh #412 design point 6 - the trust gate is on the **project** layer, and
// the reason is the one FR-PERM-9 gives.
#[test]
fn a_persona_from_an_untrusted_project_cannot_apply() {
    let persona = read("crates/lca-config/src/persona.rs");
    assert!(
        persona.contains("PersonaSource::ProjectFile => project_trusted"),
        "the project layer's personas are gated on trust"
    );
    assert!(
        persona.contains("PersonaSource::UserFile => true"),
        "the user's own file is never gated"
    );
    // The gate is applied where a persona is *read*, not only where it is listed:
    // a listing-only check would leave the apply path open.
    assert!(
        persona.contains("filter(|(_, source)| source.applies(project_trusted))"),
        "`get` and `available` both filter on trust"
    );
}

// Verifies: gh #412 - the loader and the catalogue list the key in **both**
// allowlists. The gh #272 lesson: a key present in one and missing from the other
// loads as silence.
#[test]
fn the_key_is_in_both_allowlists_and_the_docs() {
    let keys = read("crates/lca-config/src/keys.rs");
    assert!(
        keys.contains("\"personas\","),
        "the table key is registered (so the unknown-key walk stops there)"
    );
    let lib = read("crates/lca-config/src/lib.rs");
    let toml_layer = read("crates/lca-config/src/toml_layer.rs");
    for (name, text) in [("lib.rs", &lib), ("toml_layer.rs", &toml_layer)] {
        assert!(
            text.contains("\"personas\","),
            "{name} lists the key in its apply loop; a key missing from either loads \
             as silence (gh #272)"
        );
    }
    let docs = read("docs/configuration.md");
    assert!(
        docs.contains("[personas"),
        "the reference documents the section"
    );
}

// Verifies: gh #412 design point 5 - instructions are conversational, never a
// trailing `System` message.
//
// gh #299 is the rule: a trailing System message mutates the root prompt and busts
// the prompt cache. A persona that injected one would make every switch pay for a
// whole-context recompute, and would silently change the model's framing in a way
// that outlived the persona.
#[test]
fn persona_instructions_are_never_a_system_message() {
    let persona = read("crates/lca-ui/src/chat_persona.rs");
    assert!(
        persona.contains("gh #299"),
        "the file records the rule it is obeying"
    );
    assert!(
        !persona.contains("\"role\": \"system\"")
            && !persona.contains("Role::System")
            && !persona.contains("System {"),
        "no System-role message is constructed here"
    );
    // They land as a transcript notice, which is the conversational surface.
    assert!(
        persona.contains("self.say(format!(\"persona `{}`: {instructions}\""),
        "the instructions are attributed and said conversationally"
    );
}
