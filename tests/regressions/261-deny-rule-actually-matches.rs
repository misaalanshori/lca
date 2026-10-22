//! gh #261 - `/permissions deny read <literal>` refuses the read.
//!
//! The defect: the command accepted `read muffin-recipe.txt`, confirmed it as
//! `deny rule read muffin-recipe.txt`, and then did nothing. Two reasons, and the
//! second is the one that made it invisible:
//!
//! 1. **The verb was stored as part of the glob.** Rules are matched against
//!    `Action::match_value`, which for a read is the resolved **absolute path** —
//!    never the word `read`. So `read muffin-recipe.txt` matched no action at all.
//! 2. **A bare relative path cannot match an absolute value.** Even with the verb
//!    stripped, `muffin-recipe.txt` is an exact comparison against
//!    `/home/me/proj/muffin-recipe.txt`. The workaround people found was
//!    `*muffin-recipe.txt`, which works because `*` spans the directory.
//!
//! A deny rule that silently does nothing is worse than no deny rule: the user did
//! the right thing, was told it worked, and stopped watching.
//!
//! The behaviour rows live in `crates/lca-permissions/src/rules_compile_tests.rs`
//! (19 rows: the compile step, the issue's exact journey for read and write, the
//! old workaround still matching, deny beating allow, a compiled allow actually
//! allowing, a rule not capturing its neighbour, and each refusal) plus the
//! confirmation-line row in `crates/lca-cli`. This is the NFR-24 pin that the
//! mechanism stays wired.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failure.

fn read(path: &str) -> String {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(root.join(path)).unwrap_or_else(|err| panic!("read {path}: {err}"))
}

// Verifies: gh #261 - the command compiles what was typed before storing it.
//
// The fix has to sit *between* the parse and `add_rule`, or the verb lands in the
// glob again. This row pins that the call is there and that its failure is
// reported rather than stored.
#[test]
fn the_command_compiles_before_storing() {
    let commands = read("crates/lca-cli/src/tui/commands.rs");
    assert!(
        commands.contains("lca_permissions::rule_entry::compile(&pattern, &self.cwd)"),
        "`/permissions` compiles the typed rule"
    );
    assert!(
        commands
            .contains("Err(rejection) => return CommandEffect::ShowWidget(rejection.to_string())"),
        "and a rejected rule is refused with the accepted forms, never stored"
    );
    // The stored value must be the *compiled* pattern, not the raw text.
    assert!(
        commands.contains("compiled.pattern.clone()"),
        "`add_rule` receives the compiled pattern"
    );
    assert!(
        !commands.contains("store.add_rule(&self.cwd, rule_scope, decision, pattern.clone())"),
        "the raw typed text must no longer be stored as the pattern - that is the bug"
    );
    // And the confirmation says what will be matched.
    assert!(
        commands.contains("fn confirm_rule("),
        "the confirmation line is built by a named function, so its wording is testable"
    );
}

// Verifies: gh #261 - the compile step lives in the permissions crate, where the
// matcher is, not in the command.
//
// A parse in the CLI that produced a pattern the matcher could not compare is
// exactly the split that let this bug exist: the two halves have to be one fact.
#[test]
fn the_compile_step_lives_with_the_matcher() {
    let rules = read("crates/lca-permissions/src/rule_entry.rs");
    for fact in [
        "pub fn compile(",
        "pub const VERBS",
        "RuleVerb::Read",
        "RuleVerb::Write",
    ] {
        assert!(rules.contains(fact), "`rule_entry` must hold {fact}");
    }
    // The action's match value is what the pattern has to compare against, so the
    // compile module has to be able to see it.
    let action = read("crates/lca-permissions/src/action.rs");
    assert!(
        action.contains("pub(crate) fn match_value(&self)"),
        "`match_value` is visible to the compile step's rows"
    );
    // The row that proves the compiled pattern matches a real action value.
    let tests = read("crates/lca-permissions/src/rules_compile_tests.rs");
    assert!(
        tests.contains("fn the_literal_read_command_stops_the_read"),
        "the issue's own journey is a row"
    );
    assert!(
        tests.contains("fn the_old_workaround_form_still_matches"),
        "and the workaround people learned keeps working"
    );
    assert!(
        tests.contains("fn a_compiled_deny_still_beats_an_allow"),
        "and deny precedence is pinned, so the parsing change cannot weaken it"
    );
}

// Verifies: gh #261 design point 5 - a rule that cannot match anything is
// refused, with the accepted forms named.
//
// Storing a dead rule is how the original bug happened, so this is the guard
// against the next one. Both shapes are pinned: a typo'd verb and a
// matches-everything path.
#[test]
fn dead_rules_are_refused_at_entry() {
    let rules = read("crates/lca-permissions/src/rule_entry.rs");
    for fact in [
        "RuleRejection::UnknownVerb",
        "RuleRejection::EmptyPattern",
        "RuleRejection::MatchesEverything",
        // The refusal names the accepted forms.
        "bare glob such as",
    ] {
        assert!(rules.contains(fact), "the refusal must include {fact}");
    }
    let tests = read("crates/lca-permissions/src/rules_compile_tests.rs");
    for row in [
        "fn an_unknown_verb_is_refused",
        "fn a_verb_without_a_path_is_refused",
        "fn a_pattern_of_only_wildcards_is_refused",
        "fn a_real_command_is_not_mistaken_for_a_typo",
    ] {
        assert!(tests.contains(row), "the row `{row}` is present");
    }
}

// Verifies: gh #261 - the user-facing documentation describes what actually
// happens, and names the confirmation shape.
//
// `docs/usage.md` is the page a user reads after hitting this bug; a one-line
// table entry would leave them writing the same dead rule again.
#[test]
fn the_usage_doc_describes_the_rule_shape() {
    let usage = read("docs/usage.md");
    assert!(
        usage.contains("### Permission rules"),
        "the reference has a section on rules, not just a table row"
    );
    for fact in [
        // The verb-qualified shape.
        "`read <path>`",
        // The confirmation line's form, with the compiled pattern.
        "for read actions (from `read muffin-recipe.txt`)",
        // The relative-path behaviour, stated plainly.
        "spans directories",
        // The refusal.
        "did you mean `read`",
        // Precedence, so the model is not misread.
        "Deny beats allow",
    ] {
        assert!(usage.contains(fact), "docs/usage.md must state {fact}");
    }
}
