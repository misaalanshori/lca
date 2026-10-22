//! gh #261: what a user types compiles into a rule that actually matches.
//!
//! The rows split three ways: the compile step in isolation, the end-to-end
//! journey the issue filed (a deny rule stops the read), and the two neighbours
//! that must not move (the old workaround keeps working; deny still beats allow).

use std::path::PathBuf;

use super::{RuleRejection, RuleVerb, compile};
use crate::{Action, RuleDecision, RuleScope, wildcard_match};

fn project() -> PathBuf {
    PathBuf::from("/home/me/proj")
}

// ---------------------------------------------------------------- compiling

// Verifies: gh #261 - a relative path becomes a pattern that spans the
// directory, because the action's match value is an absolute path.
#[test]
fn a_relative_path_gains_a_leading_wildcard() {
    let rule = compile("read muffin-recipe.txt", &project()).expect("compiles");
    assert_eq!(rule.verb, Some(RuleVerb::Read));
    assert_eq!(rule.pattern, "*muffin-recipe.txt");
    // And it matches what the action actually carries.
    let action = Action::ReadPath {
        path: PathBuf::from("/home/me/proj/muffin-recipe.txt"),
    };
    assert!(
        wildcard_match(&rule.pattern, &action.match_value()),
        "the compiled pattern must match the resolved path: {}",
        action.match_value()
    );
}

// Verifies: gh #261 design point 2 - the same for a write rule.
#[test]
fn a_relative_write_path_gains_a_leading_wildcard() {
    let rule = compile("write out/report.txt", &project()).expect("compiles");
    assert_eq!(rule.verb, Some(RuleVerb::Write), "write is its own verb");
    assert_eq!(rule.pattern, "*out/report.txt");
    let action = Action::WritePath {
        path: PathBuf::from("/home/me/proj/out/report.txt"),
    };
    assert!(wildcard_match(&rule.pattern, &action.match_value()));
}

// Verifies: gh #261 design point 6 - the `*muffin-recipe.txt` workaround keeps
// working byte-for-byte. An anchored pattern is not rewritten.
#[test]
fn an_already_anchored_glob_passes_through_untouched() {
    for typed in [
        "read *muffin-recipe.txt",
        "read /etc/*",
        "write /tmp/**/*.log",
    ] {
        let rule = compile(typed, &project()).expect("compiles");
        let expected = typed.split_once(' ').expect("verb and path").1;
        assert_eq!(
            rule.pattern, expected,
            "`{typed}` must store its path unchanged"
        );
    }
}

// Verifies: gh #261 design point 2 - an absolute path is compared as written.
#[test]
fn an_absolute_path_is_not_prefixed() {
    let rule = compile("read /etc/hostname", &project()).expect("compiles");
    assert_eq!(rule.pattern, "/etc/hostname");
    let action = Action::ReadPath {
        path: PathBuf::from("/etc/hostname"),
    };
    assert!(wildcard_match(&rule.pattern, &action.match_value()));
}

// Verifies: gh #261 - a bare glob (no verb) is unchanged: shell and net rules
// keep the exact semantics ADR-0039 and the threat model rely on.
#[test]
fn a_bare_glob_is_not_rewritten() {
    for typed in ["git push*", "npm run *", "example.com", "git status"] {
        let rule = compile(typed, &project()).expect("compiles");
        assert_eq!(rule.verb, None, "`{typed}` names no action kind");
        assert_eq!(rule.pattern, typed, "and its pattern is what was typed");
    }
}

// Verifies: gh #261 design point 1 - the confirmation can state both what was
// typed and what will be matched, so the line is unambiguous.
#[test]
fn the_compiled_rule_carries_what_was_typed() {
    let rule = compile("read muffin-recipe.txt", &project()).expect("compiles");
    assert_eq!(rule.typed, "read muffin-recipe.txt");
    assert_eq!(rule.pattern, "*muffin-recipe.txt");
    assert_ne!(
        rule.typed, rule.pattern,
        "when the two differ the confirmation must be able to say so"
    );
}

// ---------------------------------------------------------------- refusing

// Verifies: gh #261 design point 5 - a verb that names no action kind is
// refused with the accepted forms, rather than stored as a dead pattern.
#[test]
fn an_unknown_verb_is_refused() {
    let error = compile("raed muffin-recipe.txt", &project()).expect_err("not a verb");
    match &error {
        RuleRejection::UnknownVerb(word, suggestion) => {
            assert_eq!(word, "raed", "the refusal names what was typed");
            assert_eq!(suggestion, "read", "and what was probably meant");
        }
        // gh #261: a typo'd verb would today be stored as a *bare glob* and
        // never match a read. It must be refused instead.
        other => panic!("expected UnknownVerb, got {other:?}"),
    }
    let text = error.to_string();
    for expected in ["raed", "did you mean `read`", "write", "bare glob"] {
        assert!(
            text.contains(expected),
            "the refusal names `{expected}`: {text}"
        );
    }
}

// Verifies: gh #261 - a verb with nothing after it is refused.
#[test]
fn a_verb_without_a_path_is_refused() {
    let error = compile("read", &project()).expect_err("no path");
    assert!(
        matches!(error, RuleRejection::EmptyPattern(RuleVerb::Read)),
        "got {error:?}"
    );
    assert!(error.to_string().contains("needs a path"), "{error}");
}

// Verifies: gh #261 - a rule that matches everything is refused: it is not a
// rule about anything, and it is the widest possible deny dressed as a targeted
// one.
#[test]
fn a_pattern_of_only_wildcards_is_refused() {
    for typed in ["read *", "read **", "write *"] {
        let error = compile(typed, &project()).expect_err("matches everything");
        assert!(
            matches!(error, RuleRejection::MatchesEverything(_)),
            "`{typed}`: got {error:?}"
        );
        assert!(error.to_string().contains("every file"), "{error}");
    }
}

// Verifies: gh #261 - an empty payload is refused rather than storing `""`.
#[test]
fn an_empty_payload_is_refused() {
    assert!(compile("", &project()).is_err());
    assert!(compile("   ", &project()).is_err());
}

// ------------------------------------------------------------ end to end

/// A store in a scratch project, so the rows below exercise the real authorize
/// flow rather than a matcher in isolation.
fn scratch_store(tag: &str) -> (crate::GrantStore, PathBuf, PathBuf) {
    let root = lca_testkit::scratch_path(&format!("lca-rule-entry-{tag}"));
    let _ = std::fs::remove_dir_all(&root);
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("mkdir");
    let store = crate::GrantStore::open(&root.join("grants.json")).expect("open");
    (store, root, project)
}

struct NoPrompt;
impl crate::PermissionPrompt for NoPrompt {
    fn ask(&mut self, _action: &crate::Action) -> crate::Decision {
        crate::Decision::Denied
    }
    fn review_proposals(&mut self, _diff: &crate::ProposalDiff) -> bool {
        false
    }
}

// Verifies: gh #261 - **the issue's own journey.** `/permissions deny read
// muffin-recipe.txt` compiles to a rule, and a read of that file is refused.
//
// This is the row the brief calls the acceptance criterion: "typing the literal
// command makes the read stop", not "the glob form works".
#[test]
fn the_literal_read_command_stops_the_read() {
    let (mut store, root, project) = scratch_store("literal-read");
    let compiled = compile("read muffin-recipe.txt", &project).expect("compiles");
    store
        .add_rule(
            &project,
            RuleScope::Project,
            RuleDecision::Deny,
            compiled.pattern.clone(),
        )
        .expect("add");

    let read = Action::ReadPath {
        path: project.join("muffin-recipe.txt"),
    };
    let mut prompt = NoPrompt;
    let outcome =
        crate::authorize(&mut store, &project, &read, None, &mut prompt, None).expect("authorize");
    assert!(
        !outcome.allowed(),
        "the deny rule must refuse the read of {}",
        read.match_value()
    );
    assert!(outcome.denied_by_rule, "and say it was a rule");
    assert!(!outcome.prompted, "a deny never prompts");
    let _ = std::fs::remove_dir_all(&root);
}

// Verifies: gh #261 - the same journey for a write.
#[test]
fn the_literal_write_command_stops_the_write() {
    let (mut store, root, project) = scratch_store("literal-write");
    let compiled = compile("write out/report.txt", &project).expect("compiles");
    store
        .add_rule(
            &project,
            RuleScope::Project,
            RuleDecision::Deny,
            compiled.pattern.clone(),
        )
        .expect("add");
    let write = Action::WritePath {
        path: project.join("out/report.txt"),
    };
    let mut prompt = NoPrompt;
    let outcome =
        crate::authorize(&mut store, &project, &write, None, &mut prompt, None).expect("authorize");
    assert!(!outcome.allowed(), "the write is refused");
    assert!(outcome.denied_by_rule);
    let _ = std::fs::remove_dir_all(&root);
}

// Verifies: gh #261 design point 6 - a rule stored in the old workaround form
// still matches after the fix. People learned `*muffin-recipe.txt` from this bug.
#[test]
fn the_old_workaround_form_still_matches() {
    let (mut store, root, project) = scratch_store("compat");
    // Stored literally, as a user would have typed it before this cycle.
    store
        .add_rule(
            &project,
            RuleScope::Project,
            RuleDecision::Deny,
            "*muffin-recipe.txt",
        )
        .expect("add");
    let read = Action::ReadPath {
        path: project.join("muffin-recipe.txt"),
    };
    let mut prompt = NoPrompt;
    let outcome =
        crate::authorize(&mut store, &project, &read, None, &mut prompt, None).expect("authorize");
    assert!(!outcome.allowed(), "the workaround keeps working");
    assert!(outcome.denied_by_rule);
    let _ = std::fs::remove_dir_all(&root);
}

// Verifies: gh #261 design point 4 - ADR-0039 precedence is untouched: a
// compiled deny still beats an allow for the same path.
#[test]
fn a_compiled_deny_still_beats_an_allow() {
    let (mut store, root, project) = scratch_store("precedence");
    let deny = compile("read muffin-recipe.txt", &project).expect("compiles");
    let allow = compile("read muffin-recipe.txt", &project).expect("compiles");
    store
        .add_rule(
            &project,
            RuleScope::Project,
            RuleDecision::Allow,
            allow.pattern.clone(),
        )
        .expect("allow");
    store
        .add_rule(
            &project,
            RuleScope::Project,
            RuleDecision::Deny,
            deny.pattern.clone(),
        )
        .expect("deny");
    let read = Action::ReadPath {
        path: project.join("muffin-recipe.txt"),
    };
    let mut prompt = NoPrompt;
    let outcome =
        crate::authorize(&mut store, &project, &read, None, &mut prompt, None).expect("authorize");
    assert!(
        !outcome.allowed(),
        "deny beats allow for the same path, even when both were compiled from the \
         literal command"
    );
    assert!(outcome.denied_by_rule);
    let _ = std::fs::remove_dir_all(&root);
}

// Verifies: gh #261 - and an allow compiled from the literal command actually
// allows, so the fix did not accidentally produce a pattern that matches
// nothing (which would look like "deny works" for the wrong reason).
#[test]
fn a_compiled_allow_actually_allows() {
    let (mut store, root, project) = scratch_store("allow");
    let allow = compile("read muffin-recipe.txt", &project).expect("compiles");
    store
        .add_rule(
            &project,
            RuleScope::Project,
            RuleDecision::Allow,
            allow.pattern,
        )
        .expect("allow");
    let read = Action::ReadPath {
        path: project.join("muffin-recipe.txt"),
    };
    let mut prompt = NoPrompt;
    let outcome =
        crate::authorize(&mut store, &project, &read, None, &mut prompt, None).expect("authorize");
    assert!(outcome.allowed(), "the allow rule allows");
    assert!(outcome.allowed_by_rule, "and says it was a rule");
    let _ = std::fs::remove_dir_all(&root);
}

// Verifies: gh #261 - a rule about one file does not silently deny its
// neighbours. The leading `*` spans directories, not filenames.
#[test]
fn a_rule_does_not_capture_a_neighbouring_file() {
    let (mut store, root, project) = scratch_store("neighbour");
    let compiled = compile("read muffin-recipe.txt", &project).expect("compiles");
    store
        .add_rule(
            &project,
            RuleScope::Project,
            RuleDecision::Deny,
            compiled.pattern,
        )
        .expect("add");
    let other = Action::ReadPath {
        path: project.join("banana-bread.txt"),
    };
    let mut prompt = NoPrompt;
    let outcome =
        crate::authorize(&mut store, &project, &other, None, &mut prompt, None).expect("authorize");
    assert!(
        outcome.allowed(),
        "a rule about muffin-recipe.txt must not deny banana-bread.txt"
    );
    let _ = std::fs::remove_dir_all(&root);
}

// Verifies: gh #261 - the near-miss refusal does not capture real shell rules.
//
// The typo *check* is deliberately narrow (one edit from `read` or `write`), and
// this row is the reason it has to be: refusing a legitimate command would block
// work the user meant to allow, which is worse than storing a rule that does not
// match. Anything that is not one slip from a verb stays a bare glob.
#[test]
fn a_real_command_is_not_mistaken_for_a_typo() {
    for typed in [
        "git push*",
        "npm run build",
        "cargo test",
        "rm -rf target", // `rm` is two edits from `read`
        "bread --toast", // contains but is not one edit from `read`
        "writer",        // singular-word, no space: not verb+path
        "go test ./...",
        "make install",
        "wget example.com", // `wget` is three edits from `write`
    ] {
        let rule = compile(typed, &project())
            .unwrap_or_else(|err| panic!("`{typed}` is a real command, refused: {err}"));
        assert_eq!(
            rule.verb, None,
            "`{typed}` names no action kind and must stay a bare glob"
        );
        assert_eq!(rule.pattern, typed);
    }
}

// Verifies: gh #261 - the verb is case-insensitive.
//
// `Read secrets.txt` is the same intent as `read secrets.txt`, and treating it as
// a bare glob would store a rule that matches nothing a read action carries - the
// silent no-op again, from a shift key. Found by this file's own near-miss row
// going red in the other direction.
#[test]
fn the_verb_is_case_insensitive() {
    for typed in ["Read secrets.txt", "READ secrets.txt", "Write out.txt"] {
        let rule = compile(typed, &project()).expect("a real verb, any case");
        assert!(
            rule.verb.is_some(),
            "`{typed}` names an action kind regardless of case"
        );
    }
    assert_eq!(
        compile("Read secrets.txt", &project())
            .expect("compiles")
            .pattern,
        "*secrets.txt",
        "and compiles exactly as the lower-case form does"
    );
}

// Verifies: gh #261 - a single typo in either verb is caught, and the
// suggestion is the verb the user meant.
#[test]
fn each_common_typo_shape_is_caught() {
    for (typed, expected) in [
        ("raed secrets.txt", "read"),   // transposition
        ("red secrets.txt", "read"),    // deletion
        ("reaad secrets.txt", "read"),  // insertion
        ("wrote secrets.txt", "write"), // substitution
        ("wite secrets.txt", "write"),  // deletion
    ] {
        match compile(typed, &project()) {
            Err(RuleRejection::UnknownVerb(_, suggestion)) => assert_eq!(
                suggestion, expected,
                "`{typed}` should suggest `{expected}`, suggested `{suggestion}`"
            ),
            other => panic!("`{typed}` must be refused as a near-miss, got {other:?}"),
        }
    }
}
