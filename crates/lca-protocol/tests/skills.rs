//! Skill frontmatter parsing (gh #156): one shared parser behind
//! both skill implementations. Fixture-diffed: every row below is
//! asserted against both `lca-tools` and the `skills` extension in
//! their own suites, so consolidation cannot drift either output.
//!
//! Gh #304 added the rule this file's header rows had been missing: **a line is
//! header chrome only if it *is* header syntax.** Both dialects the docs promise
//! are pinned here, because the headerless one is where instructions used to be
//! lost:
//!
//! * a [`---`-bounded](`parse_skill_text`) header (the standard Claude format, and
//!   what a `SKILL.md` in the wild uses) — everything between the fences is a
//!   header field;
//! * a headerless file — its first field-shaped run is a header, and the first line
//!   that is not a field starts the body.

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
    let skill =
        parsed("---\nname: deploy\ndescription: Ship it\nmatch: ship, deploy\n---\nRun it.\n");
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
// fallback name; unknown keys reserve without failing.
#[test]
fn headerless_and_unknown_keys_stay_compatible() {
    let skill = parsed("just instructions\nmore");
    assert_eq!(skill.name, "fallback");
    assert_eq!(skill.body, "just instructions\nmore");

    // Gh #304 changed what an unknown key does, and this row names the change:
    // `future: yes` used to be swallowed as reserved chrome, which is safe
    // *inside* a fence (the author declared the region a header) and unsafe
    // outside one (the line may be prose). In the bounded form the line is
    // header chrome; the fenced body is still the body.
    let skill = parsed("---\nname: x\nfuture: yes\n---\nbody");
    assert_eq!(skill.name, "x");
    assert_eq!(skill.body, "body");
}

// ============================================================================
// gh #304 - a headerless file may not lose its opening instructions.
//
// The defect: every opening line containing a colon was split, the unknown key
// fell into the "reserved" arm, and `body_start` advanced past the line - so the
// sentences that set the scene never reached the model and nothing said so. The
// issue's own case is the first row.
// ============================================================================

// Verifies: gh #304 - the issue's exact case. A headerless file whose first line
// is prose with a colon in it keeps that line.
#[test]
fn a_headerless_file_keeps_its_first_colon_line() {
    let skill = parsed("Note: this repository requires Rust 1.85.\nRun cargo build.\n");
    assert_eq!(
        skill.body, "Note: this repository requires Rust 1.85.\nRun cargo build.",
        "the first line is body, not header chrome"
    );
    assert_eq!(skill.name, "fallback", "and no field was invented from it");
    assert_eq!(skill.description, "");
}

// Verifies: gh #304 - two consecutive colon-bearing opening lines both survive.
// One row proves the first line is not eaten; this proves the loop stops at the
// first non-field rather than resuming, so a second prose line is not swallowed
// either.
#[test]
fn two_consecutive_colon_lines_both_survive() {
    let skill = parsed("Context: a 25-crate workspace.\nRule: no unwraps.\nThen work.\n");
    assert_eq!(
        skill.body,
        "Context: a 25-crate workspace.\nRule: no unwraps.\nThen work."
    );
}

// Verifies: gh #304 - a **real** header still parses. `name:`, `description:` and
// `match:` all land in their fields, in the headerless dialect, because a field
// whose key is known is header syntax.
#[test]
fn a_real_headerless_header_still_parses() {
    let skill = parsed("name: deploy\ndescription: Ship it\nmatch: ship, deploy\nRun it.\n");
    assert_eq!(skill.name, "deploy");
    assert_eq!(skill.description, "Ship it");
    assert_eq!(skill.match_words, vec!["ship", "deploy"]);
    assert_eq!(skill.body, "Run it.");
}

// Verifies: gh #304 design point 3 - `disable-model-invocation` keeps working.
// This is the consequence row: the field has a real effect (a restricted skill
// never injects), so a fix that stopped parsing headers early must not stop *this*
// one being honoured.
#[test]
fn disable_model_invocation_still_restricts_the_skill() {
    let skill = parsed("name: deploy\ndisable-model-invocation: true\nRun it.\n");
    assert_eq!(skill.name, "deploy");
    assert!(
        !skill.model_invocable,
        "a restricted skill stays restricted"
    );
    assert_eq!(skill.body, "Run it.");

    // And the same field inside a fence.
    let skill = parsed("---\nname: deploy\ndisable-model-invocation: true\n---\nRun it.\n");
    assert!(!skill.model_invocable);
}

// Verifies: gh #304 design point 4 - a `---`-bounded header keeps today's
// behaviour **exactly**. That form is unambiguous, so the fix is scoped to the
// headerless case; this row is the proof it did not move.
#[test]
fn a_bounded_header_behaves_exactly_as_before() {
    let skill = parsed("---\nname: deploy\ndescription: Ship it\nmatch: ship\n---\nRun it.\n");
    assert_eq!(skill.name, "deploy");
    assert_eq!(skill.description, "Ship it");
    assert_eq!(skill.match_words, vec!["ship"]);
    assert!(skill.model_invocable);
    assert_eq!(skill.body, "Run it.");

    // Prose between the fences is header chrome, not prose: the author declared
    // the region a header, so an unparseable line there is reserved, not body.
    let skill = parsed("---\nname: x\nNot: a field, this is the fenced area\n---\nbody\n");
    assert_eq!(skill.name, "x");
    assert_eq!(
        skill.body, "body",
        "a fenced header line never leaks into the body"
    );

    // The closing fence with no opening one — the dialect's other accepted form.
    let skill = parsed("name: x\ndescription: y\n---\nbody\n");
    assert_eq!(skill.name, "x");
    assert_eq!(skill.description, "y");
    assert_eq!(skill.body, "body");

    // A leading fence with no closing one is *not* a bounded header: the file is
    // body, which is the safe reading (nothing is dropped).
    let skill = parsed("---\nname: x\nbody\n");
    assert_eq!(skill.name, "fallback");
    assert!(
        skill.body.contains("name: x"),
        "the text survives: {:?}",
        skill.body
    );
}

// Verifies: gh #304 design point 1 - an unknown key **terminates the header**, and
// its line appears in the body.
//
// This is the chosen rule, stated so it cannot drift. Inside a fence an unknown key
// is reserved chrome (row above); outside one, a line whose key is not one of the
// four known fields is the first line of the body. The asymmetry is deliberate:
// dropping content is invisible, while treating a header line as prose merely shows
// it to the model, which a reader can see and correct.
#[test]
fn an_unknown_key_terminates_the_header_and_joins_the_body() {
    let skill = parsed("name: x\nfuture: yes\nmore instructions\n");
    assert_eq!(skill.name, "x", "the known field before it still parsed");
    assert_eq!(
        skill.body, "future: yes\nmore instructions",
        "the unknown line starts the body rather than vanishing"
    );
}

// Verifies: gh #304 - a file that is **only** header parses without panic and
// yields an empty body. Both dialects, because the headerless loop's termination
// is what this row pins.
#[test]
fn a_header_only_file_yields_an_empty_body() {
    let skill = parsed("name: x\ndescription: y\n");
    assert_eq!(skill.name, "x");
    assert_eq!(skill.description, "y");
    assert_eq!(skill.body, "");

    let skill = parsed("---\nname: x\ndescription: y\n---\n");
    assert_eq!(skill.name, "x");
    assert_eq!(skill.body, "");

    assert_eq!(parsed("").body, "");
    assert_eq!(parsed("name: x").body, "");
}

// Verifies: gh #304 - a field **value** may contain a colon. The split is the
// first colon, so the tail belongs to the value; a `description:` that reads like
// a sentence is a real thing to write.
#[test]
fn a_field_value_may_contain_a_colon() {
    let skill = parsed("description: Use when: X happened\nthen do this\n");
    assert_eq!(skill.description, "Use when: X happened");
    assert_eq!(skill.body, "then do this");
}

// Verifies: gh #304 design point 1 - the distinction that *is* the fix: a line is
// header chrome only when its key is one of the four known fields.
//
// Two prose shapes that a looser reading would have swallowed or misparsed: a key
// with whitespace before its colon (`this is: prose`), and a capitalised English
// word that happens to be followed by a colon (`Note:`). Neither is a field, so both
// start the body - and `Note:` matters most because it is the issue's own example of
// a sentence an author writes on the first line of a skill.
#[test]
fn prose_with_a_colon_is_never_a_header_field() {
    for prose in [
        "this is: prose\nand more\n",
        "Note: this repository requires Rust 1.85.\nBuild it.\n",
        "Context: a 25-crate workspace\nRule: no unwraps\n",
    ] {
        let skill = parsed(prose);
        assert_eq!(
            skill.body,
            prose.trim(),
            "the whole file is body: {prose:?} -> {:?}",
            skill.body
        );
        assert_eq!(skill.name, "fallback", "and no field was invented");
        assert_eq!(skill.description, "");
        assert!(skill.match_words.is_empty());
        assert!(skill.model_invocable);
    }
}

// Verifies: gh #304 - the field list is matched exactly, so a near-miss key is prose
// rather than a silently-half-understood field. YAML keys are exact strings; a
// reader who writes `Name:` (capitalised) gets their line shown to the model instead
// of a skill named after the fallback with its instruction missing.
#[test]
fn a_near_miss_key_is_prose_not_a_field() {
    let skill = parsed("Name: deploy\nRun it.\n");
    assert_eq!(skill.name, "fallback", "`Name:` is not `name:`");
    assert_eq!(skill.body, "Name: deploy\nRun it.");

    // A prefix of a real key is not that key either.
    let skill = parsed("nam: deploy\nRun it.\n");
    assert_eq!(skill.name, "fallback");
    assert_eq!(skill.body, "nam: deploy\nRun it.");
}

// Verifies: gh #304 - a fence with trailing text is **not** a fence. `--- see below`
// is prose, so it terminates a headerless header as body rather than silently
// becoming chrome and taking its own text with it.
#[test]
fn a_dashes_line_with_text_is_not_a_fence() {
    let skill = parsed("name: x\n--- see the appendix\nappendix text\n");
    assert_eq!(skill.name, "x");
    assert_eq!(
        skill.body, "--- see the appendix\nappendix text",
        "the line survives as body"
    );
}

// Verifies: gh #304 - CRLF files parse the same as LF ones. A `SKILL.md` authored on
// Windows reaches this parser with `\r\n`, and a fence check that compared the raw
// line would miss every one of them (`is_fence` trims).
#[test]
fn a_crlf_file_parses_the_same_as_an_lf_one() {
    let lf = parsed("name: deploy\ndescription: Ship it\n---\nRun it.\n");
    let crlf = parsed("name: deploy\r\ndescription: Ship it\r\n---\r\nRun it.\r\n");
    assert_eq!(crlf.name, "deploy");
    assert_eq!(crlf.description, "Ship it");
    assert_eq!(crlf.body, "Run it.", "the body loses its carriage returns");
    assert_eq!(lf.body, crlf.body);
}
