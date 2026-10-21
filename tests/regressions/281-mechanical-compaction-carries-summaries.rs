//! GitHub #281: the mechanical compaction fallback dropped every
//! carried-forward summary.
//!
//! Iterative re-compaction carries the previous summary forward as a
//! `custom` record whose `custom_type` is `lca_protocol::PREVIOUS_SUMMARY_TYPE`
//! (`"previous-summary"`), with the summary text under `data.text` - the shape
//! `crates/lca-core/src/compact.rs::previous_summary_marker` builds.
//!
//! `mechanical_summary` selected those records correctly and then decoded each
//! one with `record_text("compaction", body)` - a hardcoded kind. The
//! `compaction` branch reads `value["summary"]`, which a marker record does not
//! have, so every carried summary decoded to `""` and the fallback's
//! "Earlier summary" line either vanished or read as an empty slot. The
//! failure only ever showed on the path that exists for when the model is
//! unavailable, i.e. on long sessions, which is where the history matters.
//!
//! This guard drives the real extension over a real store-shaped record set and
//! asserts the two summaries a second compaction must carry. On the pre-fix
//! tree the carried line is empty and the first pass's facts are gone.
//!
//! Verifies: NFR-24 (a shipped defect's guard), gh #281, FR-CTX-1 (a
//! compaction's content), and `docs/compaction.md`'s "Iterative summaries
//! (phase 2)" rule that the mechanical fallback carries the summary as
//! "Earlier summary".

use compaction_default::{Excerpt, mechanical_summary, run_compact};

/// The marker body the host writes, built from the shared constant so this
/// guard cannot drift from the host the way the extension did.
fn host_marker(text: &str) -> String {
    serde_json::json!({
        "v": lca_protocol::FORMAT_VERSION,
        "t": "custom",
        "ts": 1_760_000_000_000u64,
        "id": "previous-summary",
        "custom_type": lca_protocol::PREVIOUS_SUMMARY_TYPE,
        "data": { "text": text },
    })
    .to_string()
}

/// The excerpt list the host hands the strategy: marker first, then the
/// session's own records (`to_wit_session_record`'s kind + body shape).
fn candidate_range(carried: &str) -> Vec<Excerpt> {
    vec![
        ("custom".to_string(), host_marker(carried)),
        (
            "user".to_string(),
            serde_json::json!({
                "v": 1, "t": "user", "id": "u1",
                "content": "now wire the parser into the CLI",
            })
            .to_string(),
        ),
        (
            "assistant".to_string(),
            serde_json::json!({
                "v": 1, "t": "assistant", "id": "a1",
                "content": [{ "type": "text", "text": "wired it up" }],
            })
            .to_string(),
        ),
    ]
}

// Verifies: gh #281 - the first pass's distilled facts are still in the
// second pass's mechanical summary. This is the issue's headline row.
#[test]
fn a_second_mechanical_compaction_carries_the_first_pass_summary() {
    let first_pass = "## Goal\nBuild the parser.\n## Critical Context\nThe codeword is quartz-77.";
    let summary = mechanical_summary(&candidate_range(first_pass));
    assert!(
        summary.contains("quartz-77"),
        "the first pass's facts survive a mechanical second compaction:\n{summary}"
    );
    assert!(
        summary.contains("Earlier summary:"),
        "and they are labelled as a carried summary:\n{summary}"
    );
    // The range's own content is still summarized, not replaced.
    assert!(
        summary.contains("wire the parser into the CLI"),
        "the new range is still summarized:\n{summary}"
    );
}

// Verifies: gh #281 - the same carry-forward holds on the degraded path, which
// is what a user actually sees when the model call fails: the "summarization
// failed" line plus a mechanical summary that still remembers.
#[test]
fn the_degraded_path_keeps_the_carried_summary() {
    let degraded = run_compact(
        &candidate_range("The codeword is quartz-77."),
        Some(&|_prompt| Err("provider returned HTTP 429".to_string())),
    );
    assert!(
        degraded.starts_with("[summarization failed:"),
        "the failure is announced:\n{degraded}"
    );
    assert!(
        degraded.contains("quartz-77"),
        "and the fallback still carries the earlier summary:\n{degraded}"
    );
}

// Verifies: gh #281 - the negative, so the guard pins the fix and not a
// widening: an unrelated `custom` record must not become an "Earlier
// summary". Only the marker type carries a summary.
#[test]
fn an_unrelated_custom_record_is_never_a_carried_summary() {
    let excerpts = vec![
        (
            "custom".to_string(),
            serde_json::json!({
                "v": 1, "t": "custom", "id": "c1",
                "custom_type": "system-prompt-change",
                "data": { "text": "the system prompt changed" },
            })
            .to_string(),
        ),
        (
            "user".to_string(),
            serde_json::json!({ "v": 1, "t": "user", "id": "u2", "content": "continue" })
                .to_string(),
        ),
    ];
    let summary = mechanical_summary(&excerpts);
    assert!(
        !summary.contains("Earlier summary:"),
        "unrelated extension state is not a summary:\n{summary}"
    );
}

// Verifies: gh #281 - prose that merely says "compaction" is not a compaction
// record. The old selection matched a substring in some earlier code; the
// surviving rule is by type, and this row pins it from the text side so a
// future loosening is caught.
#[test]
fn prose_about_compaction_is_not_a_compaction_record() {
    let excerpts = vec![
        (
            "user".to_string(),
            serde_json::json!({
                "v": 1, "t": "user", "id": "u3",
                "content": "explain what compaction does in this agent",
            })
            .to_string(),
        ),
        (
            "assistant".to_string(),
            serde_json::json!({
                "v": 1, "t": "assistant", "id": "a3",
                "content": [{ "type": "text", "text": "Compaction replaces old turns with a summary." }],
            })
            .to_string(),
        ),
    ];
    let summary = mechanical_summary(&excerpts);
    assert!(
        !summary.contains("Earlier summary:"),
        "a conversation about compaction is not a summary to carry:\n{summary}"
    );
}
