//! Skills parsing, matching, and the append-only injection that keeps
//! the cache boundary clean (Phase 4 exit clause4, ADR-0015).
//!
//! Verifies: FR-CTX-2 (the chain's content), FR-CTX-4 (nothing
//! persisted - the injection exists only in the returned list).

use lca_protocol::{ChatMessage, ContentBlock, MessageRole};
use skills::{parse_skill, skill_matches, transform_with_skills};

const DOC: &str = "name: commit style
match: commit, changelog
---
Write commit messages in the imperative.";

// Verifies: the documented header/body split and both header keys.
#[test]
fn parses_the_documented_header() {
    let skill = parse_skill("fallback", DOC);
    assert_eq!(skill.name, "commit style");
    assert_eq!(skill.match_words, vec!["commit", "changelog"]);
    assert_eq!(skill.body, "Write commit messages in the imperative.");
    // A body with no header at all still parses (name falls back).
    let bare = parse_skill("bare", "Just instructions.\nWith a second line.");
    assert_eq!(bare.name, "bare");
    assert!(bare.body.starts_with("Just instructions."), "{}", bare.body);
    assert!(bare.match_words.is_empty());
}

// Verifies: matching is against the latest user message, in the
// documented case-insensitive way.
#[test]
fn matches_against_the_latest_user_message() {
    let skill = parse_skill("s", DOC);
    assert!(skill_matches(&skill, "Please COMMIT this change"));
    assert!(skill_matches(&skill, "update the changelog"));
    assert!(!skill_matches(&skill, "read the file"));
}

fn conversation(last_user: &str) -> Vec<ChatMessage> {
    vec![
        ChatMessage::text(MessageRole::System, "sys"),
        ChatMessage::text(MessageRole::User, "first"),
        ChatMessage::text(MessageRole::Assistant, "reply"),
        ChatMessage::text(MessageRole::User, last_user),
    ]
}

// Verifies: FR-CTX-2's shape and exit clause4 - a match appends ONE
// message carrying the instructions; the original list is untouched
// prefix-for-prefix, which is why the cache boundary cannot move; a
// non-match changes nothing at all.
#[test]
fn a_match_appends_and_never_rewrites() {
    let skill = parse_skill("commit", DOC);
    let original = conversation("please commit this");
    let transformed = transform_with_skills(original.clone(), std::slice::from_ref(&skill));
    assert_eq!(
        transformed.len(),
        original.len() + 1,
        "exactly one appended message"
    );
    assert_eq!(
        transformed[..original.len()],
        original[..],
        "prefix byte-identical: the stable region cannot have moved"
    );
    let injection = transformed.last().expect("the injection");
    assert_eq!(injection.role, MessageRole::System);
    let text = match &injection.content[..] {
        [ContentBlock::Text { text }] => text.clone(),
        other => panic!("one text block, got {other:?}"),
    };
    assert!(text.contains("[skill commit style]"), "{text}");
    assert!(text.contains("imperative"), "{text}");

    let untouched =
        transform_with_skills(conversation("read the file"), std::slice::from_ref(&skill));
    assert_eq!(untouched.len(), 4, "no match, no injection");
}
