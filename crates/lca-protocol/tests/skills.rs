//! Skill frontmatter parsing (gh #156): one shared parser behind
//! both skill implementations. Fixture-diffed: every row below is
//! asserted against both `lca-tools` and the `skills` extension in
//! their own suites, so consolidation cannot drift either output.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use lca_protocol::skills::{ParsedSkill, parse_skill_text};

fn parsed(text: &str) -> ParsedSkill {
    parse_skill_text("fallback", text)
}

// Verifies: gh #156 - headers parse (name, description, match,
// disable-model-invocation), unknown keys reserve, the body starts
// after `---` or the first non-header line.
#[test]
fn frontmatter_headers_parse_and_body_follows() {
    let skill = parsed("name: deploy\ndescription: Ship it\nmatch: ship, deploy\n---\nRun it.\n");
    assert_eq!(skill.name, "deploy");
    assert_eq!(skill.description, "Ship it");
    assert_eq!(skill.match_words, vec!["ship", "deploy"]);
    assert!(skill.model_invocable);
    assert_eq!(skill.body, "Run it.");

    let skill = parsed("match: a, , B\ndisable-model-invocation: true\nbody here");
    assert_eq!(skill.name, "fallback");
    assert_eq!(skill.match_words, vec!["a", "b"]);
    assert!(!skill.model_invocable);
    assert_eq!(skill.body, "body here");
}

// Verifies: gh #156 - a file with no headers is all body under the
// fallback name; unknown header keys reserve without failing.
#[test]
fn headerless_and_unknown_keys_stay_compatible() {
    let skill = parsed("just instructions\nmore");
    assert_eq!(skill.name, "fallback");
    assert_eq!(skill.body, "just instructions\nmore");

    let skill = parsed("name: x\nfuture: yes\n---\nbody");
    assert_eq!(skill.name, "x");
    assert_eq!(skill.body, "body");
}
