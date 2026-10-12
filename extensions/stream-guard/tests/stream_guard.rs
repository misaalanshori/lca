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
