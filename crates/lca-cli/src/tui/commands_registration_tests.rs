//! Registration guards for the `/` completion list (gh #443, extended by gh #270).
//!
//! # What this proves, and what it did not
//!
//! Gh #443's guards proved the router's allowlist, the host's dispatch arms and
//! `/help`'s description table agree. They did **not** prove that the completion
//! list contains what the router answers - and that gap is exactly how `/label`,
//! `/labels`, `/jump`, `/rename`, `/persona`, `/ext`, `/marketplace` and `/quit`
//! came to be routed, documented, and invisible to anyone who typed `/` (gh #270).
//!
//! So the coverage direction is added here, on the same parsed-source footing:
//! `COMPLETION_NAMES` must contain every name the **UI router** answers and every
//! name the **host dispatch** answers. Both readers parse the source, so neither
//! guard can be satisfied by editing the list it checks.
//!
//! The reverse direction matters too: a name in the completion list that nothing
//! routes is a menu row that answers `unknown command`. That is checked as well,
//! because a literal list is easy to append to.

use super::{COMPLETION_NAMES, HANDLED_NAMES};

/// The host's dispatch arms, read from the source so the guard cannot
/// be satisfied by editing the very list it checks. A helper rather
/// than a line-by-line grep because `cargo fmt` reflows the match.
fn dispatch_arms() -> Vec<String> {
    let source = include_str!("commands.rs");
    let start = source
        .find("fn dispatch_command(self: &Arc<Self>")
        .expect("the host dispatcher exists");
    let body = &source[start..];
    let end = body.find("\n    }\n").expect("the dispatcher has a body");
    let mut arms = Vec::new();
    for line in body[..end].lines() {
        let trimmed = line.trim();
        // Guard arms (`name if name == "skill" || name.starts_with("skill:")`)
        // are matched by a condition rather than a literal at the head,
        // so read the quoted names out of the condition too. Without
        // this the scanner reports `/skill` as having no arm at all.
        if trimmed.starts_with("name if ") && trimmed.contains("=>") {
            for (index, part) in trimmed.split('"').enumerate() {
                if index % 2 == 1 && !part.contains(' ') && !part.ends_with(':') {
                    arms.push(part.to_string());
                }
            }
            continue;
        }
        if !trimmed.starts_with('"') || !trimmed.contains("=>") {
            continue;
        }
        // `"attach" => ...` and the group `"login" | "logout" | "usage" =>`
        for (index, part) in trimmed.split('"').enumerate() {
            if index % 2 == 1 && !part.contains(' ') {
                arms.push(part.to_string());
            }
        }
    }
    arms.sort();
    arms.dedup();
    arms
}

// Verifies: gh #443 - every name the host answers is registered, so the
// router admits it. Red before the fix: `bug`, `export`, and `import`
// answered `unknown command` because they were in the dispatcher and in
// `/help` but missing from `slash_commands`.
#[test]
fn every_handled_command_is_registered() {
    // The three gh #443 casualties specifically, so the regression
    // names them even if someone trims the list later.
    for name in ["bug", "export", "import"] {
        assert!(
            HANDLED_NAMES.contains(&name),
            "/{name} must be registered: it has a dispatch arm and a /help line"
        );
    }
    for name in HANDLED_NAMES {
        assert!(
            !name.is_empty() && !name.starts_with('/'),
            "/{name} is registered without a leading slash"
        );
    }
}

// Verifies: gh #443 - a name with no dispatch arm does not get
// registered, because a listed-but-unhandled name falls through to the
// extension dispatcher and silently does nothing (the converse bug).
#[test]
fn every_registered_name_has_a_dispatch_arm() {
    let arms = dispatch_arms();
    for name in HANDLED_NAMES {
        assert!(
            arms.iter().any(|arm| arm == name),
            "/{name} is registered but the host dispatcher has no arm for it; \
             a listed-but-unhandled name does nothing silently. Arms seen: {arms:?}"
        );
    }
}

// Verifies: gh #443 - the guard's own reader works, so the two rows
// above cannot pass vacuously on an empty arm list. If the dispatcher
// is ever rewritten into a shape this scanner cannot read, this fails
// loudly instead of the guards silently checking nothing.
#[test]
fn the_arm_reader_finds_the_dispatch_arms() {
    let arms = dispatch_arms();
    for expected in ["attach", "bug", "export", "import", "login", "usage"] {
        assert!(
            arms.iter().any(|arm| arm == expected),
            "the arm reader missed /{expected}; arms seen: {arms:?}"
        );
    }
}

/// The UI router's arms, read from `chat_commands.rs`.
///
/// `lca-ui` is a dependency, not a sibling module, so the file is read at compile
/// time by path. If it ever moves, this fails to compile - which is louder than a
/// guard that silently reads nothing.
fn router_arms() -> Vec<String> {
    let source = include_str!("../../../lca-ui/src/chat_commands.rs");
    let start = source
        .find("pub(crate) fn dispatch_command")
        .expect("the router exists");
    let body = &source[start..];
    let end = body.find("\n    }\n").expect("the router has a body");
    let mut names = Vec::new();
    for line in body[..end].lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("name if ") && trimmed.contains("=>") {
            for (index, part) in trimmed.split('"').enumerate() {
                if index % 2 == 1 && !part.contains(' ') && !part.ends_with(':') {
                    names.push(part.to_string());
                }
            }
            continue;
        }
        if !trimmed.starts_with('"') || !trimmed.contains("=>") {
            continue;
        }
        for (index, part) in trimmed.split('"').enumerate() {
            if index % 2 == 1 && !part.contains(' ') {
                names.push(part.to_string());
            }
        }
    }
    names.sort();
    names.dedup();
    names
}

/// Names that are routed but deliberately **not** in the completion list.
///
/// An empty list is the honest state today and it is asserted as such (see
/// [`the_exception_list_is_empty_today`]) - but the mechanism exists because
/// "we forgot" and "we chose" must not look the same. An entry here needs its
/// reason written beside it, and the reason is what the guard prints when the
/// excluded name is still routed.
const EXCLUDED_FROM_COMPLETION: &[(&str, &str)] = &[
    // Nothing is excluded today. `/quit` is listed beside `/exit`, `/marketplace`
    // beside `/extensions`, `/labels` beside `/label`: a user reaching for the
    // alias gets the same answer as the canonical spelling, so hiding one would
    // only make half the documented vocabulary unreachable from `/`.
];

// Verifies: gh #270 (**the row this issue is about**) - **every command the UI
// router answers is in the completion list.** Red before the fix: `/label`,
// `/labels`, `/jump`, `/rename`, `/persona`, `/ext`, `/marketplace` and `/quit`
// all routed and all were missing, so typing `/` could not reach them.
#[test]
fn every_routed_command_is_in_the_completion_list() {
    let arms = router_arms();
    assert!(
        arms.len() >= 20,
        "the router arm reader found {} arms, too few to be the real router - the \
         source shape changed and this guard needs updating",
        arms.len()
    );
    for name in &arms {
        if let Some((_, why)) = EXCLUDED_FROM_COMPLETION
            .iter()
            .find(|(excluded, _)| excluded == name)
        {
            // Excluded on purpose: still routed, so the reason has to be there.
            assert!(
                !why.is_empty(),
                "/{name} is excluded from completion without a stated reason; \
                 an unexplained exclusion is indistinguishable from an oversight"
            );
            continue;
        }
        assert!(
            COMPLETION_NAMES.contains(&name.as_str()),
            "/{name} is routed but missing from COMPLETION_NAMES, so typing `/` \
             can never reach it. Routed: {arms:?}"
        );
    }
}

// Verifies: gh #270 - and the other direction: **every name in the completion
// list is routed by the UI router or the host dispatch**, so the menu cannot
// offer a row that answers `unknown command`. A literal list is easy to append
// to; this is what makes appending it safe.
#[test]
fn every_completion_entry_is_something_that_runs() {
    let routed = router_arms();
    let handled: Vec<String> = HANDLED_NAMES.iter().map(|name| name.to_string()).collect();
    for name in COMPLETION_NAMES {
        assert!(
            routed.iter().any(|arm| arm == name) || handled.iter().any(|arm| arm == name),
            "/{name} is offered by `/` completion but neither the router nor the \
             host dispatcher answers it: the menu would advertise a command that \
             prints `unknown command`. Routed: {routed:?}"
        );
    }
}

// Verifies: gh #270 - **no duplicates.** The positional-insert shape could emit
// the same name twice (it did: `/settings` and `/attach` appeared in both the
// hand-inserted run and the `HANDLED_NAMES` loop), and nothing downstream would
// notice - the popup dedupes silently, which hides the bookkeeping error rather
// than reporting it.
#[test]
fn the_completion_list_has_no_duplicates() {
    let mut seen: Vec<&str> = Vec::new();
    for name in COMPLETION_NAMES {
        assert!(
            !seen.contains(name),
            "/{name} appears twice in COMPLETION_NAMES; the popup would silently \
             collapse the pair and hide the mistake"
        );
        seen.push(name);
    }
}

// Verifies: gh #270 - **the order is pinned exactly.** Order is user-visible:
// `/help` leads because a lost user starts by typing `/`, and `/exit`/`/quit` sit
// last so a mistyped Enter cannot land on them. A "contains" row would pass while
// the menu silently reordered - which is precisely what the colliding
// `insert(n, ..)` indices did - so this asserts the **whole sequence**, and adding
// or moving a command therefore has to be a deliberate edit here as well as
// there. That cost is the guard: it is what makes a reorder impossible to do by
// accident.
#[test]
fn the_completion_order_is_exactly_the_documented_one() {
    assert_eq!(
        COMPLETION_NAMES,
        &[
            // 1. Answering the interface.
            "help",
            "hotkeys",
            "fullscreen",
            "theme",
            "thinking",
            // 2. The session.
            "session",
            "tree",
            "fork",
            "clone",
            "resume",
            "label",
            "labels",
            "jump",
            "name",
            "rename",
            // 3. Its content.
            "compact",
            "export",
            "import",
            // 4. Model and configuration.
            "model",
            "scoped-models",
            "settings",
            "persona",
            "permissions",
            "grants",
            "trust",
            // 5. Integrations.
            "mcp",
            "extensions",
            "marketplace",
            "ext",
            "reload",
            "changelog",
            // 6. Provider identity.
            "login",
            "logout",
            "usage",
            // 7. Attachments.
            "attach",
            "paste",
            // 8. Diagnostics.
            "bug",
            // 9. Leaving.
            "exit",
            "quit",
            // 10. The skill umbrella.
            "skill",
        ],
        "the completion order is the documented grouping; a reorder is a \
         user-visible change and belongs in this row's diff"
    );
}

// Verifies: gh #270 - **the issue's named omissions each appear**, one row per
// name rather than a table, because a table hides which one was missed. The
// names are the ones the issue lists; `/yolo`, `/new`, `/close` and `/tab` are
// not among them (see `the_exception_list_is_empty_today` for why), and
// `tests/regressions/270-*` states that exclusion in prose.
#[test]
fn the_named_omissions_are_completed() {
    for name in ["export", "labels", "label", "jump", "quit"] {
        assert!(
            COMPLETION_NAMES.contains(&name),
            "/{name} is routed and documented but was missing from the completion \
             list - the exact defect gh #270 names"
        );
    }
}

// Verifies: gh #270 - the **exception list is empty today**, and the mechanism
// is what matters. An exclusion without a reason would be an oversight wearing a
// decision's clothes, so the list may only ever be empty or explained (this row
// fails if an entry appears with an empty reason, which the coverage guard above
// also refuses - two readers, because this is the shape most likely to rot).
#[test]
fn the_exception_list_is_empty_today() {
    assert!(
        EXCLUDED_FROM_COMPLETION.is_empty(),
        "nothing is excluded today; if this is now a real decision, its reason \
         belongs beside the entry and this row's name should change with it"
    );
}

// Verifies: gh #270 - **the completion list is one ordered literal**, not a
// positional build-up. The defect was not the order it happened to produce; it
// was that `insert(n, ..)` at hand-maintained indices made the order unreadable
// and collided with itself. This asserts the shape, because a future
// "convenience" rebuild would reintroduce exactly that.
#[test]
fn the_completion_list_is_one_ordered_literal() {
    let source = include_str!("commands_list.rs");
    assert!(
        source.contains("pub(crate) const COMPLETION_NAMES: &[&str]"),
        "the list is a const array"
    );
    // Scan the **code** only: the doc comment above the list quotes the old
    // `.insert(n, ..)` shape to explain the defect, and a guard that could be
    // satisfied by deleting its own explanation would be worse than no guard.
    let code: String = source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !code.contains(".insert("),
        "no positional insert may come back: `insert(n, ..)` at hand-maintained \
         indices is the defect gh #270 fixed"
    );
    // And the list is built from it in that order, with only the dynamic parts
    // appended.
    let commands = include_str!("commands.rs");
    let build = commands
        .find("COMPLETION_NAMES\n            .iter()")
        .or_else(|| commands.find("COMPLETION_NAMES"))
        .expect("`slash_commands()` builds from the constant");
    let after = &commands[build..];
    assert!(
        after.contains(".map(|name| format!(\"/{name}\"))"),
        "each entry becomes `/name` in list order"
    );
}
