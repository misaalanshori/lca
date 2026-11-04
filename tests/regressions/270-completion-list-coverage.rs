//! NFR-24 guard for gh #270: the `/` completion list is one ordered literal, and
//! it covers every command the interface routes.
//!
//! # The defect
//!
//! `slash_commands()` started from `BUILTIN_SLOTS` and then called
//! `names.insert(n, ..)` at hand-maintained indices whose values **collided**
//! (`insert(7, "/clone")` followed by `insert(7, "/thinking")` shifted `/clone`
//! out of place). So the menu's order was a scramble no reader could predict from
//! the source - and eight routed, documented commands (`/label`, `/labels`,
//! `/jump`, `/rename`, `/persona`, `/ext`, `/marketplace`, `/quit`) were never
//! inserted at all, which made them unreachable for anyone who finds commands by
//! typing `/`.
//!
//! # What this file adds over the in-crate rows
//!
//! The behavioural rows live beside the list (`tui/commands_registration_tests.rs`)
//! and are the real proof. This file is the NFR-24 named guard: it pins the
//! *observation* those rows depend on - that the list is still a `const` array the
//! guards can read, that it is still the one source `slash_commands()` builds
//! from, and that the router file it is compared against has not moved - so a
//! refactor cannot leave the guards silently comparing nothing.
//!
//! # The issue's named omissions, and what is not among them
//!
//! The issue lists `/yolo`, `/export`, `/labels`/`/label`, `/jump`, `/new`/`/close`/
//! `/tab`/`/tabs` and `/quit`. Of those:
//!
//! - **`/yolo` does not exist as a command** in this tree. `permissions.mode =
//!   yolo` is a *setting* (`/settings`, and `--yolo` for one process, ADR-0042),
//!   and `docs/security.md`'s `/yolo` line is stale prose, not a handler. Adding a
//!   `/yolo` command would be a new feature (and a second way to set a permission
//!   mode), not a completion-list fix.
//! - **`/new`, `/close`, `/tab` and `/tabs` do not exist as commands either.**
//!   Tabs are opened and closed by keys (`Ctrl+T`, `Ctrl+W`, and the tab bar's own
//!   click targets), which is where pi puts them too - pi's builtin command list
//!   has `/new` but no tab command, and LCA's tab vocabulary is the keybinding
//!   table's. The commands this issue lists are the ones that *do* exist and were
//!   missing.
//!
//! So this file asserts the ones that exist, and states the rest plainly rather
//! than silently narrowing the issue.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(rel: &str) -> String {
    let path = root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {path:?}: {err}"))
}

/// The entries of `COMPLETION_NAMES`, read from the source in declaration order.
fn completion_names() -> Vec<String> {
    let source = read("crates/lca-cli/src/tui/commands_list.rs");
    let start = source
        .find("pub(crate) const COMPLETION_NAMES")
        .expect("COMPLETION_NAMES exists");
    let body = &source[start..];
    let end = body.find("];").expect("the list is terminated");
    let mut names = Vec::new();
    for line in body[..end].lines() {
        let trimmed = line.trim();
        // Entries are quoted literals; the doc comment above the list is outside
        // `body` because `start` is the declaration itself.
        if let Some(rest) = trimmed.strip_prefix('"')
            && let Some(name) = rest.split('"').next()
            && !name.is_empty()
        {
            names.push(name.to_string());
        }
    }
    names
}

// Verifies: gh #270 - **the list is readable as a declaration**, which is what the
// guards in the crate need in order to check anything at all. If the list becomes a
// computed value, every coverage row silently starts comparing against nothing.
#[test]
fn the_completion_list_is_a_declaration_the_guards_can_read() {
    let names = completion_names();
    assert!(
        names.len() >= 30,
        "the reader found {} entries, too few to be the real list - the source \
         shape changed and this guard needs updating",
        names.len()
    );
    // Every entry is a bare name: `slash_commands()` prefixes the slash, so an
    // entry that already carries one would produce `//help`.
    for name in &names {
        assert!(
            !name.starts_with('/'),
            "`{name}` carries a leading slash; the list holds bare names"
        );
    }
}

// Verifies: gh #270 - **`slash_commands()` builds from that list in its order.**
// The defect was the *build*, so the build site is what has to stay honest: if a
// future edit appends a hand-maintained tail before the dynamic rows, the pinned
// order in the crate collapses without any row noticing.
#[test]
fn the_build_site_reads_the_list_in_order() {
    let commands = read("crates/lca-cli/src/tui/commands.rs");
    assert!(
        commands.contains("COMPLETION_NAMES"),
        "`slash_commands()` builds from COMPLETION_NAMES"
    );
    // The old positional shape must not come back at the build site either.
    let slash = commands
        .find("fn slash_commands(&self)")
        .expect("the build site exists");
    let body = &commands[slash..];
    let end = body.find("\n    }\n").expect("it has a body");
    let code: String = body[..end]
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !code.contains(".insert("),
        "no positional insert at the build site: that is the gh #270 defect"
    );
    // Only the dynamic rows may be appended after the fixed list.
    assert!(
        code.contains("skill:"),
        "per-skill rows are still appended (the one dynamic part of the list)"
    );
}

// Verifies: gh #270 - **the compatibility list in `lca-ui`'s autocomplete dedupe
// still has something to do with its name.** The popup dedupes whatever the host
// hands it (gh #408), which is a safety net, not a licence: with the list a single
// literal, `slash_commands()` no longer hands over a repeat, so the dedupe is now
// defence in depth rather than a fix for this layer's bookkeeping.
#[test]
fn the_host_list_no_longer_hands_the_popup_a_duplicate() {
    let names = completion_names();
    let mut seen = std::collections::BTreeSet::new();
    for name in &names {
        assert!(
            seen.insert(name.clone()),
            "`{name}` appears twice in COMPLETION_NAMES; the popup's dedupe would \
             hide the bookkeeping error"
        );
    }
}

// Verifies: gh #270 - **the names the issue lists that really are commands are in
// the list.** One assertion per name so a failure says which one regressed; the
// four that are not commands in this tree (`/yolo`, `/new`, `/close`, `/tab`) are
// documented above rather than asserted, because inventing them would be a
// feature, not a fix.
#[test]
fn the_issue_named_commands_are_completed() {
    let names = completion_names();
    for name in ["export", "labels", "label", "jump", "quit"] {
        assert!(
            names.iter().any(|entry| entry == name),
            "/{name} is routed and documented but is missing from the completion \
             list; that is the defect gh #270 names. Found: {names:?}"
        );
    }
}

// Verifies: gh #270 - **the router is still where the coverage guard looks.** The
// in-crate row compares the list against `lca-ui`'s `dispatch_command` arms by
// reading that file; if the router moves (a rename, a split), the guard's
// `include_str!` breaks the build - but a *silent* move (the file staying, the
// function renamed) would make it read nothing, so the function's presence is
// asserted here too.
#[test]
fn the_router_the_coverage_guard_reads_still_exists() {
    let ui = read("crates/lca-ui/src/chat_commands.rs");
    assert!(
        ui.contains("pub(crate) fn dispatch_command"),
        "the coverage guard reads the router's arms from this function"
    );
    // And the host dispatcher the other direction reads.
    let cli = read("crates/lca-cli/src/tui/commands.rs");
    assert!(
        cli.contains("fn dispatch_command(self: &Arc<Self>"),
        "the host-dispatch guard reads its arms from this function"
    );
}
