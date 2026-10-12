//! Rules-compat discovery and head injection (gh #197): Cursor,
//! Cline, Windsurf, and Copilot rule files become one attributed
//! block at the head of the system prompt.
//!
//! Verifies: rules inject with source attribution, discovery order is
//! fixed, missing rule files are a silent pass-through, and the
//! session log is untouched (the injection exists only in the
//! returned list, FR-CTX-4).

use std::collections::HashMap;

use lca_protocol::{ChatMessage, ContentBlock, MessageRole};
use rules_compat::{RuleFile, discover_rules, format_block, transform_with_rules};

/// An in-memory workspace: paths that exist map to their text.
struct FakeFs {
    files: HashMap<String, String>,
    dirs: HashMap<String, Vec<String>>,
}

impl rules_compat::FsView for FakeFs {
    fn list(&self, path: &str) -> Result<Option<Vec<String>>, lca_protocol::CapabilityError> {
        Ok(self.dirs.get(path).cloned())
    }

    fn read(&self, path: &str) -> Result<Option<String>, lca_protocol::CapabilityError> {
        Ok(self.files.get(path).cloned())
    }
}

fn workspace() -> FakeFs {
    FakeFs {
        files: HashMap::from([
            (
                ".cursorrules".to_string(),
                "Use tabs, never spaces.".to_string(),
            ),
            (
                ".clinerules".to_string(),
                "Ask before deleting.".to_string(),
            ),
            (".windsurfrules".to_string(), "Wind says flow.".to_string()),
            (
                ".github/copilot-instructions.md".to_string(),
                "Prefer iterators.".to_string(),
            ),
            (
                ".cursor/rules/rust.mdc".to_string(),
                "No unwrap in production.".to_string(),
            ),
            (
                ".cursor/rules/notes.txt".to_string(),
                "Not a rule file.".to_string(),
            ),
        ]),
        dirs: HashMap::from([(
            ".cursor/rules".to_string(),
            vec!["rust.mdc".to_string(), "notes.txt".to_string()],
        )]),
    }
}

fn sources(rules: &[RuleFile]) -> Vec<&str> {
    rules.iter().map(|rule| rule.source.as_str()).collect()
}

// Verifies: every documented location is discovered, in the fixed
// order, with only `*.mdc` files taken from the Cursor rules
// directory.
#[test]
fn discovers_every_rule_location_in_fixed_order() {
    let rules = discover_rules(&workspace());
    assert_eq!(
        sources(&rules),
        [
            ".cursor/rules/rust.mdc",
            ".cursorrules",
            ".clinerules",
            ".windsurfrules",
            ".github/copilot-instructions.md",
        ],
        "order is the documented discovery order, not filesystem order"
    );
}

// Verifies: a workspace with no rule files anywhere injects nothing
// and warns about nothing (opt-in means zero-cost absence).
#[test]
fn missing_rule_files_are_a_silent_pass_through() {
    let empty = FakeFs {
        files: HashMap::new(),
        dirs: HashMap::new(),
    };
    let rules = discover_rules(&empty);
    assert!(rules.is_empty());
    assert!(format_block(&rules).is_none());
}

// Verifies: the block carries every rule under its own source
// attribution, so the model can tell Cursor's rules from Copilot's.
#[test]
fn the_block_attributes_every_source() {
    let rules = discover_rules(&workspace());
    let block = format_block(&rules).expect("rules format");
    assert!(block.starts_with("# External Project Rules"), "{block}");
    for source in sources(&rules) {
        assert!(block.contains(source), "attribution for {source}:\n{block}");
    }
    assert!(block.contains("Use tabs, never spaces."), "{block}");
}

fn conversation(first_is_system: bool) -> Vec<ChatMessage> {
    let first = if first_is_system {
        ChatMessage::text(MessageRole::System, "Be helpful.".to_string())
    } else {
        ChatMessage::text(MessageRole::User, "Hi.".to_string())
    };
    vec![first]
}

fn head_text(messages: &[ChatMessage]) -> String {
    messages[0]
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// Verifies: the rules land at the head of the system prompt, ahead of
// the existing system text, byte-stable across turns (ADR-0017: the
// cache prefix never moves).
#[test]
fn rules_inject_at_the_head_of_the_system_prompt() {
    let rules = discover_rules(&workspace());
    let out = transform_with_rules(conversation(true), &rules);
    assert_eq!(out.len(), 1, "no message added, the system prompt grows");
    let head = head_text(&out);
    assert!(
        head.starts_with("# External Project Rules"),
        "rules first:\n{head}"
    );
    assert!(
        head.contains("Be helpful."),
        "existing system text kept:\n{head}"
    );
}

// Verifies: without a leading system message the rules become their
// own head message rather than rewriting history.
#[test]
fn rules_become_the_head_message_when_no_system_leads() {
    let rules = discover_rules(&workspace());
    let out = transform_with_rules(conversation(false), &rules);
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].role, MessageRole::System);
    assert!(head_text(&out).starts_with("# External Project Rules"));
    assert_eq!(out[1].role, MessageRole::User);
}

// Verifies: no rules, no touch (the transform is the identity, so the
// chain's cache math cannot even see us).
#[test]
fn no_rules_leaves_messages_untouched() {
    let before = conversation(true);
    let out = transform_with_rules(before.clone(), &[]);
    assert_eq!(out, before);
}

// Verifies: the manifest test contract — the declared grants stay in
// step with `extension.toml` (a grant added to one must land in both).
#[test]
fn manifest_grants_match_the_manifest() {
    let manifest = rules_compat::MANIFEST;
    assert!(
        manifest.contains("worlds = [\"context-transform\"]"),
        "{manifest}"
    );
    let grants = rules_compat::manifest_grants();
    assert_eq!(grants.fs.len(), 1, "workspace read only");
    assert!(manifest.contains("workspace = \"read\""), "{manifest}");
}
