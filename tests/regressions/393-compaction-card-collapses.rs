//! NFR-24 guard for gh #393: the compaction card stays a collapsed *view*, and the
//! record stays the record.
//!
//! These rows read the source, because the promise is about *shape*: a later cycle
//! could pass every behaviour row while re-expanding the summary by default,
//! growing a second collapse mechanism beside the tool-card lifecycle, or moving
//! the collapse into the stored record. Each row keys on a property that cannot be
//! renamed away without losing it.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use lca_protocol::Record;

fn read(path: &str) -> String {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(root.join(path)).unwrap_or_else(|err| panic!("read {path}: {err}"))
}

/// Code only: comments removed, so prose naming a past mistake cannot trip a guard.
fn code_only(text: &str) -> String {
    text.lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn compaction_record(id: &str, summary: &str) -> Record {
    Record::Compaction {
        v: 1,
        ts: 0,
        id: id.to_string(),
        parent: None,
        replaced_from: "u1".to_string(),
        replaced_to: "u2".to_string(),
        first_kept_id: String::new(),
        summary: summary.to_string(),
        strategy: "llm".to_string(),
        usage: None,
        read_files: Vec::new(),
        modified_files: Vec::new(),
        system_prompt: None,
    }
}

// Verifies: gh #393 - **the default is collapsed.** The exact defect was that every
// compaction printed its whole summary; a later "helpful" default of expanded would
// restore the wall. The card's default comes from `expanded.unwrap_or(global)`, with
// `global` starting false.
#[test]
fn a_replayed_compaction_starts_collapsed_by_default() {
    let mut transcript = lca_ui::Transcript::default();
    transcript.replay_records(&[compaction_record("c1", "THE-SUMMARY")], None);
    let text = transcript.render(100, &lca_ui::Theme::default()).join("\n");
    assert!(
        !text.contains("THE-SUMMARY"),
        "the summary is not printed by default:\n{text}"
    );
    assert!(
        text.contains("[compaction]"),
        "but the card names itself, so collapsed is not gone:\n{text}"
    );
}

// Verifies: gh #393 - **the stored record does not change.** This is presentation
// only: the summary text, the strategy and the range all stay in the log. A
// shrinking-record "fix" would destroy the user's own history.
#[test]
fn the_stored_summary_and_its_fields_survive_the_collapse() {
    let record = compaction_record("c1", "THE-WHOLE-SUMMARY");
    let Record::Compaction {
        summary,
        strategy,
        replaced_from,
        replaced_to,
        ..
    } = &record
    else {
        panic!("the fixture is a compaction");
    };
    assert_eq!(summary, "THE-WHOLE-SUMMARY", "the summary is stored whole");
    assert_eq!(strategy, "llm", "the strategy is still recorded");
    assert_eq!(replaced_from, "u1");
    assert_eq!(replaced_to, "u2");

    // And the record round-trips through the wire shape unchanged.
    let json = serde_json::to_string(&record).expect("the record serialises");
    assert!(
        json.contains("THE-WHOLE-SUMMARY"),
        "the persisted form still carries the summary: {json}"
    );
    let back: Record = serde_json::from_str(&json).expect("and parses back");
    assert_eq!(back, record, "the record is unchanged by the view");
}

// Verifies: gh #393 design point 2 - **one collapse mechanism, not a third.** The
// card reuses the tool card's per-entry `Option<bool>` override over a global
// default. A second lifecycle (its own flag, its own key, its own cache rule) is
// the drift this repo has closed three times recently.
#[test]
fn the_card_reuses_the_per_entry_override_lifecycle() {
    let source = code_only(&read("crates/lca-ui/src/transcript/mod.rs"));
    // The variant carries the same `Option<bool>` override a tool card does.
    assert!(
        source.contains("Compaction {") && source.contains("expanded: Option<bool>"),
        "the entry carries the per-entry override"
    );
    // And the toggle reads the global default exactly as the tool toggle does.
    // The toggle lives in `compaction_card.rs` (it moved there for the 1,200-line
    // ceiling), so this reads the card module - keyed on the function, not on the
    // file, because the file is what the split changes.
    let card_source = code_only(&read("crates/lca-ui/src/transcript/compaction_card.rs"));
    let toggle = card_source
        .find("fn toggle_entry_compaction")
        .expect("the compaction toggle exists");
    let body = &card_source[toggle..];
    let end = body.find("\n    }\n").unwrap_or(body.len());
    let body = &body[..end];
    assert!(
        body.contains("unwrap_or(global)"),
        "the toggle follows the tool card's override rule:\n{body}"
    );
    assert!(
        body.contains("invalidate_cache"),
        "and drops the cached frame, or the next paint is stale:\n{body}"
    );
    // The card must not invent a second keybinding.
    let card = code_only(&read("crates/lca-ui/src/transcript/compaction_card.rs"));
    assert!(
        card.contains("\"app.tools.expand\""),
        "the hint names the existing expand action, not a new one"
    );
}

// Verifies: gh #393 design point 5 - **live and replay show the same shape.** #496
// allowed them to differ on *content*; this change requires the same *form*, so a
// reader recognises the row live and on resume. Both go through the one card type.
#[test]
fn both_the_live_and_replay_paths_build_the_same_card_type() {
    let events = code_only(&read("crates/lca-ui/src/chat_turn_events.rs"));
    let live = events
        .find("fn note_live_compaction")
        .expect("the live marker exists");
    let body = &events[live..];
    let end = body.find("\n    }\n").unwrap_or(body.len());
    assert!(
        body[..end].contains("Card::live("),
        "the live path builds the shared card"
    );

    let transcript = code_only(&read("crates/lca-ui/src/transcript/mod.rs"));
    assert!(
        transcript.contains("Card::replay("),
        "the replay path builds the shared card"
    );
    // And neither path renders text itself any more: the renderer is one function.
    assert!(
        !transcript.contains("\"[compaction] {summary}"),
        "the replay path no longer formats a pre-rendered string"
    );
}

// Verifies: gh #393 gap 2 - **a branch summary renders at all.** Before this change
// `Record::BranchSummary` matched `_ => {}` and was silently dropped, so a resumed
// fork lost the text explaining it. A guard, because "renders nothing" is the
// failure mode that leaves no trace to notice.
#[test]
fn a_branch_summary_is_handled_rather_than_dropped() {
    let transcript = code_only(&read("crates/lca-ui/src/transcript/mod.rs"));
    assert!(
        transcript.contains("Record::BranchSummary"),
        "a dedicated arm exists rather than the catch-all"
    );
    let mut live = lca_ui::Transcript::default();
    live.replay_records(
        &[Record::BranchSummary {
            v: 1,
            ts: 0,
            id: "b1".to_string(),
            parent: None,
            from_id: None,
            summary: "BRANCH-TEXT".to_string(),
        }],
        None,
    );
    let text = live.render(100, &lca_ui::Theme::default()).join("\n");
    assert!(
        text.contains("branch summary"),
        "the branch summary has a card:\n{text}"
    );
}
