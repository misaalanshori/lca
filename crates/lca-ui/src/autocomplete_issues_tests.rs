//! Rows for the first-party `#`-issue provider (gh #408).
//!
//! Parsing and filtering are pinned here; the live `gh` call is exercised for real
//! in the tmux receipt, not in a unit row (a row that shells out would fail on a
//! machine without `gh`, which is a harness defect, not a product one).

use super::*;

fn issue(number: u64, state: &str, title: &str) -> Issue {
    Issue {
        number,
        state: state.into(),
        title: title.into(),
    }
}

fn sample() -> Vec<Issue> {
    vec![
        issue(12, "OPEN", "Fix the redaction bug"),
        issue(123, "OPEN", "Add a redacting filter"),
        issue(45, "CLOSED", "Old redaction attempt"),
        issue(7, "OPEN", "Unrelated"),
    ]
}

#[test]
fn a_gh_line_parses_into_number_state_and_title() {
    let parsed =
        Issue::parse_line("10285\tOPEN\tAdd pi.registerToolRenderer() for unregistered tools")
            .expect("a well-formed line");
    assert_eq!(parsed.number, 10285);
    assert_eq!(parsed.state, "OPEN");
    assert_eq!(
        parsed.title,
        "Add pi.registerToolRenderer() for unregistered tools"
    );
}

#[test]
fn a_line_that_is_not_an_issue_is_skipped() {
    // A `gh` banner or an update warning must never become a row.
    assert!(Issue::parse_line("").is_none());
    assert!(Issue::parse_line("gh: something went wrong\tOPEN\tx").is_none());
    assert!(Issue::parse_line("not-a-number\tOPEN\ttitle").is_none());
    // A line missing its state column has no row to offer.
    assert!(Issue::parse_line("12\tonly-two-fields").is_none());
}

#[test]
fn the_labels_and_timestamp_columns_are_not_part_of_the_title() {
    // The real `gh` shape is five columns. A title that swallowed the labels and
    // the timestamp made `#999` match `#53` on its date, so this is the row that
    // keeps the parse honest.
    let parsed = Issue::parse_line(
        "53\tOPEN\tPort the MCP client\tenhancement, p0-critical\t2026-10-17T10:31:25Z",
    )
    .expect("parsed");
    assert_eq!(parsed.title, "Port the MCP client");
    assert!(!parsed.title.contains("enhancement"));
    assert!(!parsed.title.contains("2026"));
}

#[test]
fn a_row_inserts_the_hash_number_and_describes_the_state() {
    let item = issue(12, "OPEN", "Fix the redaction bug").to_item();
    assert_eq!(item.value, "#12", "accepting inserts the reference");
    assert_eq!(item.label, "#12");
    assert_eq!(
        item.description.as_deref(),
        Some("[open] Fix the redaction bug"),
        "the state leads, lowercase as pi renders it"
    );
    assert!(!item.dim, "a real issue is a choice, not a notice");
}

#[test]
fn an_empty_query_lists_issues_in_gh_order() {
    let items = filter_issues(&sample(), "");
    assert_eq!(items.len(), 4);
    assert_eq!(items[0].value, "#12", "gh's order is kept");
}

#[test]
fn a_numeric_query_matches_numbers_by_prefix() {
    // `12` must offer #12 and #123, and must not offer #45 or #7.
    let items = filter_issues(&sample(), "12");
    let values: Vec<&str> = items.iter().map(|i| i.value.as_str()).collect();
    assert_eq!(values, vec!["#12", "#123"], "{values:?}");
}

#[test]
fn a_numeric_prefix_beats_a_fuzzy_number_match() {
    // #9123 *contains* `12` but does not *start* with it; a reader typing a number
    // means the prefix, so only the prefix rows appear. A fuzzy pass would offer
    // #9123, which is how `#12` starts selecting the wrong issue.
    let issues = vec![
        issue(9123, "OPEN", "unrelated"),
        issue(12, "OPEN", "the one"),
    ];
    let items = filter_issues(&issues, "12");
    let values: Vec<&str> = items.iter().map(|i| i.value.as_str()).collect();
    assert_eq!(values, vec!["#12"], "{values:?}");
}

#[test]
fn a_text_query_finds_a_title_by_fuzzy_match() {
    let items = filter_issues(&sample(), "redact");
    let values: Vec<&str> = items.iter().map(|i| i.value.as_str()).collect();
    assert!(
        values.contains(&"#12") && values.contains(&"#123") && values.contains(&"#45"),
        "every redaction issue is a candidate: {values:?}"
    );
    assert!(
        !values.contains(&"#7"),
        "an unrelated title is not: {values:?}"
    );
}

#[test]
fn a_query_that_matches_nothing_offers_nothing() {
    // The chain falls through to the built-in provider on an empty result, so this
    // is what keeps a stray `#` from swallowing the keystroke.
    assert!(filter_issues(&sample(), "zzzzz").is_empty());
}

#[test]
fn an_issue_number_that_does_not_exist_offers_nothing_rather_than_a_fuzzy_neighbour() {
    // Live, `#999` offered `#53` because a fuzzy pass matched digits inside the
    // timestamp. A number means a number: no match is no rows.
    assert!(
        filter_issues(&sample(), "999").is_empty(),
        "a missing issue number must not turn into a fuzzy guess"
    );
    assert!(filter_issues(&sample(), "45000").is_empty());
}

#[test]
fn the_rows_are_capped() {
    let many: Vec<Issue> = (1..=200)
        .map(|n| issue(n, "OPEN", &format!("issue {n}")))
        .collect();
    assert_eq!(filter_issues(&many, "").len(), MAX_SUGGESTIONS);
    assert_eq!(filter_issues(&many, "issue").len(), MAX_SUGGESTIONS);
}
