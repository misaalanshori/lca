//! Released defect (0.1.x): re-compaction dropped every previously
//! summarized fact. A second compaction's candidate range covers the previous
//! `compaction` record, but the strategy rendered it as the placeholder
//! `[compaction]`, so the earlier summary's text vanished. A codeword stored
//! early in a session was gone after the second compaction.
//!
//! Verifies: FR-SESS-5.

use compaction_default::{mechanical_summary, record_text};

#[test]
fn a_prior_summary_carries_into_the_next_summary() {
    let body = r#"{"v":1,"t":"compaction","summary":"The codeword is quartz-77."}"#;
    assert_eq!(
        record_text("compaction", body),
        "The codeword is quartz-77."
    );
}

#[test]
fn the_mechanical_fallback_also_carries_a_prior_summary() {
    let excerpts = vec![
        (
            "compaction".to_string(),
            r#"{"summary":"codeword quartz-77"}"#.to_string(),
        ),
        ("user".to_string(), r#"{"content":"next"}"#.to_string()),
    ];
    let summary = mechanical_summary(&excerpts);
    assert!(summary.contains("quartz-77"), "{summary}");
}
