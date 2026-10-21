//! Time-traveling stream rules, observe-only (gh #194): regex rules
//! scan streaming deltas; a match fires telemetry (status-line,
//! command report), never steering — the hooks-stream contract is
//! post-close observation, so cancellation and steer-queue injection
//! wait on a host affordance that does not exist yet (marked SEAM).
//!
//! Verifies: rules parse from TOML, project rules need trust, the
//! scanner fires across delta boundaries, observation never mutates
//! the text, the command lists and probes rules, and the manifest
//! grants match the manifest.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use stream_guard::{Match, Rule, Scanner, parse_rules_toml};

const RULES: &str = r#"
[[rule]]
id = "no-unwrap"
pattern = '\.unwrap\(\)'
reminder = "Avoid .unwrap(); return a Result with ? instead."

[[rule]]
id = "no-box-leak"
pattern = 'Box::leak\b'
reminder = "Do not use Box::leak; use Arc instead."
"#;

fn rules() -> Vec<Rule> {
    parse_rules_toml(RULES).expect("parse")
}

// Verifies: every rule carries its id, compiled pattern, and reminder.
#[test]
fn rules_parse_with_ids_and_reminders() {
    let rules = rules();
    assert_eq!(rules.len(), 2);
    assert_eq!(rules[0].id, "no-unwrap");
    assert!(rules[1].reminder.contains("Arc"), "{}", rules[1].reminder);
}

// Verifies: malformed rules fail with the reason, never a panic, and
// a bad regex names its rule.
#[test]
fn malformed_rules_fail_loudly() {
    assert!(parse_rules_toml("[[rule]]\nid = \"x\"").is_err());
    assert!(parse_rules_toml("[[rule]]\nid = \"x\"\npattern = \"(\"\nreminder = \"y\"").is_err());
    assert!(parse_rules_toml("not toml [[[").is_err());
}

// Verifies: a forbidden pattern split across two deltas still fires
// (the window, not the chunk, is the unit).
#[test]
fn matches_fire_across_delta_boundaries() {
    let rules = rules();
    let mut scanner = Scanner::new(rules);
    assert!(scanner.feed("text-delta", "let x = opt.").is_empty());
    let matches = scanner.feed("text-delta", "unwrap();");
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].rule, "no-unwrap");
    assert!(
        matches[0].reminder.contains("? instead"),
        "{}",
        matches[0].reminder
    );
}

// Verifies: clean text stays silent, and non-text kinds never match
// (usage payloads are numbers, not prose).
#[test]
fn clean_text_and_non_text_kinds_stay_silent() {
    let rules = rules();
    let mut scanner = Scanner::new(rules);
    assert!(
        scanner
            .feed("text-delta", "let x = opt.unwrap_or_default();")
            .is_empty()
    );
    assert!(scanner.feed("usage", ".unwrap()").is_empty());
    assert!(
        scanner
            .feed("reasoning-delta", "consider Box::leak here")
            .len()
            == 1
    );
}

// Verifies: the observation contract — feeding returns matches but
// never alters the stream (the scanner borrows, the text passes
// through untouched by construction: feed returns matches only).
#[test]
fn observation_never_steers() {
    let rules = rules();
    let mut scanner = Scanner::new(rules);
    let delta = "let x = opt.unwrap();";
    let matches: Vec<Match> = scanner.feed("text-delta", delta);
    assert_eq!(delta, "let x = opt.unwrap();", "input untouched");
    assert_eq!(matches.len(), 1);
}

// Verifies: the window is bounded (a runaway stream cannot grow the
// buffer without limit).
#[test]
fn the_window_is_bounded() {
    let rules = rules();
    let mut scanner = Scanner::new(rules);
    for _ in 0..100 {
        scanner.feed("text-delta", &"x".repeat(1000));
    }
    assert!(scanner.buffered() <= stream_guard::WINDOW_CHARS + 64);
}

// Verifies: the status widget names the last fired rule, and stays
// empty before anything fires.
#[test]
fn the_status_widget_reports_the_last_match() {
    assert!(stream_guard::status_widget(None).is_none());
    let widget = stream_guard::status_widget(Some("no-unwrap")).expect("widget");
    assert!(
        matches!(&widget, lca_protocol::Widget::Text { content, .. } if content.contains("no-unwrap")),
        "status is plain text naming the rule, got {widget:?}"
    );
}

// Verifies: the probe command lists rules with no argument and scans
// text with one.
#[test]
fn the_command_lists_and_probes() {
    let rules = rules();
    let list = stream_guard::run_command(&rules, "");
    assert!(
        list.contains("no-unwrap") && list.contains("no-box-leak"),
        "{list}"
    );
    let hit = stream_guard::run_command(&rules, "call Box::leak(y)");
    assert!(hit.contains("no-box-leak") && hit.contains("Arc"), "{hit}");
    let clean = stream_guard::run_command(&rules, "all good here");
    assert!(clean.contains("clean"), "{clean}");
}

// Verifies: the manifest test contract — the WASM install
// contract names the tool world (the only world whose guests can
// read the rules files); the native handle additionally wires
// stream, command, and status without manifest binding.
#[test]
fn manifest_grants_match_the_manifest() {
    let manifest = stream_guard::MANIFEST;
    assert!(manifest.contains("worlds = [\"tool\"]"), "{manifest}");
    let grants = stream_guard::manifest_grants();
    assert_eq!(grants.fs.len(), 2, "workspace + private, read only");
    assert!(
        manifest.contains("regions = [\"status-line\"]"),
        "{manifest}"
    );
}

// ---------------------------------------------------------------------------
// gh #300: one match, one emit
// ---------------------------------------------------------------------------

// Verifies: gh #300 acceptance 1 - a single occurrence reports *once*,
// not once per delta for the next window. The defect: the matched text
// stayed in the window, so every later delta re-matched it.
#[test]
fn one_occurrence_reports_once_not_once_per_delta() {
    let mut scanner = Scanner::new(rules());
    let mut fired = Vec::new();
    // The match lands in delta 3 of 12; deltas 4..12 are ordinary text
    // that keeps the matched span inside the 4096-char window.
    let deltas = [
        "let x = ",
        "opt",
        ".unwrap();",
        " then ",
        "more ",
        "text ",
        "keeps ",
        "coming ",
        "after ",
        "the ",
        "match ",
        "here",
    ];
    for delta in deltas {
        for hit in scanner.feed("text-delta", delta) {
            fired.push(hit.rule);
        }
    }
    assert_eq!(
        fired,
        vec!["no-unwrap".to_string()],
        "the same span must not re-fire on later deltas: {fired:?}"
    );
}

// Verifies: gh #300 - the rule is not silenced for a fixed distance; it
// fires again when *new* text contains it. This is the row that keeps
// the fix from degenerating into "suppress this rule for N chars".
#[test]
fn the_same_rule_fires_again_on_new_text() {
    let mut scanner = Scanner::new(rules());
    let mut fired = Vec::new();
    for delta in [
        "first ",
        "opt.unwrap(); ",
        "then another ",
        "opt.unwrap(); ",
        "done",
    ] {
        for hit in scanner.feed("text-delta", delta) {
            fired.push(hit.rule);
        }
    }
    assert_eq!(
        fired,
        vec!["no-unwrap".to_string(), "no-unwrap".to_string()],
        "two distinct occurrences are two matches: {fired:?}"
    );
}

// Verifies: gh #300 - the positive control. A pattern split across delta
// boundaries still matches, which is the whole reason the window exists.
// A fix that shrank or bypassed the window would break this row.
#[test]
fn a_pattern_split_across_deltas_still_matches() {
    let mut scanner = Scanner::new(rules());
    assert!(scanner.feed("text-delta", "a Box").is_empty());
    let hits = scanner.feed("text-delta", "::leak(x)");
    assert_eq!(hits.len(), 1, "the window, not the chunk, is the unit");
    assert_eq!(hits[0].rule, "no-box-leak");
}

// Verifies: gh #300 acceptance 2 - the window resets at a turn
// boundary, so turn N's match cannot fire on turn N+1's first delta.
#[test]
fn a_match_does_not_survive_into_the_next_turn() {
    let mut scanner = Scanner::new(rules());
    assert_eq!(scanner.feed("text-delta", "opt.unwrap();").len(), 1);
    scanner.reset();
    assert!(
        scanner.feed("text-delta", "clean start").is_empty(),
        "the previous turn's text is gone, so nothing re-fires"
    );
    // And the reset did not disable the rules.
    assert_eq!(
        scanner.feed("text-delta", "opt.unwrap();").len(),
        1,
        "rules still fire after a reset"
    );
}

// Verifies: gh #300's stated overlap rule - two rules matching the same
// span each report once (dedup is per rule, not global), and the pair
// does not re-report on the next delta.
#[test]
fn two_rules_matching_one_span_each_report_once() {
    let rules = parse_rules_toml(
        r#"
[[rule]]
id = "mentions-ok"
pattern = 'OK'
reminder = "first"

[[rule]]
id = "mentions-ok-loud"
pattern = 'OK\+'
reminder = "second"
"#,
    )
    .expect("parse");
    let mut scanner = Scanner::new(rules);
    let first = scanner.feed("text-delta", "status OK+");
    let mut ids: Vec<String> = first.iter().map(|hit| hit.rule.clone()).collect();
    ids.sort();
    assert_eq!(
        ids,
        vec!["mentions-ok".to_string(), "mentions-ok-loud".to_string()],
        "both rules report once for the same span"
    );
    assert!(
        scanner.feed("text-delta", " still going").is_empty(),
        "neither re-reports on the next delta"
    );
}

// ---------------------------------------------------------------------------
// gh #300: trust parity across delivery modes (the security guard)
// ---------------------------------------------------------------------------

#[cfg(not(target_arch = "wasm32"))]
struct DenyPrompt;

#[cfg(not(target_arch = "wasm32"))]
impl lca_permissions::PermissionPrompt for DenyPrompt {
    fn ask(&mut self, _action: &lca_permissions::Action) -> lca_permissions::Decision {
        lca_permissions::Decision::Denied
    }
    fn review_proposals(&mut self, _diff: &lca_permissions::ProposalDiff) -> bool {
        false
    }
}

/// An in-memory filesystem view: the same shape both modes read through.
struct FakeFs {
    files: std::collections::BTreeMap<(String, String), String>,
}

impl FakeFs {
    fn with(paths: &[(&str, &str)]) -> FakeFs {
        let mut files = std::collections::BTreeMap::new();
        for (path, text) in paths {
            let (scope, rel) = path
                .split_once(':')
                .expect("fixture paths are `scope:path`");
            files.insert((scope.to_string(), rel.to_string()), text.to_string());
        }
        FakeFs { files }
    }
}

impl stream_guard::FsView for FakeFs {
    fn read(
        &self,
        scope: &str,
        path: &str,
    ) -> Result<Option<String>, lca_protocol::CapabilityError> {
        Ok(self
            .files
            .get(&(scope.to_string(), path.to_string()))
            .cloned())
    }
}

const PROJECT_RULES: &str = r#"
[[rule]]
id = "injected-by-the-repo"
pattern = 'password'
reminder = "the repository decided this is worth saying"
"#;

// Verifies: gh #300 acceptance 3 - an untrusted workspace's rules are
// never loaded. This is the security assertion: a repository the user
// has not trusted cannot put words in the guard's mouth.
#[test]
fn an_untrusted_workspace_contributes_no_rules() {
    let fs = FakeFs::with(&[
        (
            "workspace:.lca/extensions/stream-guard/rules.toml",
            PROJECT_RULES,
        ),
        (
            "private:rules.toml",
            "[[rule]]\nid = \"global\"\npattern = 'x'\nreminder = \"r\"\n",
        ),
    ]);
    let (rules, _) = stream_guard::load_rules(&fs, false);
    let ids: Vec<&str> = rules.iter().map(|rule| rule.id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["global"],
        "only the user's own global rules survive an untrusted project"
    );
}

// Verifies: gh #300 - a trusted workspace's rules do load. The mirror of
// the row above, so the gate is shown to be a gate and not an outage.
#[test]
fn a_trusted_workspace_contributes_its_rules() {
    let fs = FakeFs::with(&[(
        "workspace:.lca/extensions/stream-guard/rules.toml",
        PROJECT_RULES,
    )]);
    let (rules, _) = stream_guard::load_rules(&fs, true);
    let ids: Vec<&str> = rules.iter().map(|rule| rule.id.as_str()).collect();
    assert_eq!(ids, vec!["injected-by-the-repo"]);
}

// Verifies: gh #300 - exactly one predicate decides whether workspace
// rules load, and the WASM guest routes through that predicate rather
// than reaching for the file itself. The defect was a second, ungated
// read in the guest; a source-level pin is the guard that keeps it gone,
// because the WASM half cannot be instantiated from this crate's tests.
#[test]
fn the_wasm_guest_reads_rules_through_the_shared_loader() {
    let source = include_str!("../src/lib.rs");
    let wasm_half = source
        .split("mod wasm_mode")
        .nth(1)
        .expect("the WASM module is in this file");
    assert!(
        wasm_half.contains("super::load_rules"),
        "the guest must load through the one trust-gated path"
    );
    assert!(
        !wasm_half.contains("PROJECT_RULES_PATH"),
        "the guest must not name the workspace rules path itself, or it can \
         bypass the trust gate again"
    );
}

// ---------------------------------------------------------------------------
// gh #300: the turn-boundary hook is reachable
// ---------------------------------------------------------------------------

// Verifies: gh #300 acceptance 2's wiring. The registry dispatches
// `pre-turn` (where the window reset lives) only to handles declaring
// `World::Hooks`, and `stream-event` only to `World::HooksStream`. A
// handle that declares the stream world alone gets stream events and
// never the turn boundary, so `on_pre_turn` was dead code and the window
// survived across turns. This row pins both worlds on the handle.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn the_handle_declares_the_worlds_the_turn_boundary_needs() {
    use lca_ext_abi::{ExtensionDispatch, World};
    let root = std::env::temp_dir().join(format!("lca-sg-worlds-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for dir in ["project", "private", "config", "data", "tmp"] {
        std::fs::create_dir_all(root.join(dir)).expect("mkdir");
    }
    let prompt: std::sync::Arc<std::sync::Mutex<dyn lca_permissions::PermissionPrompt>> =
        std::sync::Arc::new(std::sync::Mutex::new(DenyPrompt));
    let store = std::sync::Arc::new(std::sync::Mutex::new(
        lca_permissions::GrantStore::open(&root.join("grants.json")).expect("grants"),
    ));
    let cap = std::sync::Arc::new(lca_tools::Capabilities::new(
        "stream-guard",
        stream_guard::manifest_grants(),
        lca_permissions::ScopeRoots {
            workspace: root.join("project"),
            private: root.join("private"),
            home_config: root.join("config"),
            temp: root.join("tmp"),
            state_dir: root.join("data"),
        },
        prompt,
        store,
        root.join("project"),
        None,
    ));
    let handle = stream_guard::StreamGuard::new(cap, true);
    let worlds = handle.worlds();
    assert!(
        worlds.contains(&World::Hooks),
        "the turn boundary dispatches on World::Hooks: {worlds:?}"
    );
    assert!(
        worlds.contains(&World::HooksStream),
        "stream events dispatch on World::HooksStream: {worlds:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}
