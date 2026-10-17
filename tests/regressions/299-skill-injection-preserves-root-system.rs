//! GitHub #299: a mid-session skill match mutated the vendor's ROOT system
//! prompt, busting the KV cache.
//!
//! `injection_message` built the skill excerpt as `MessageRole::System` and
//! appended it. Every provider builder (`anthropic`, `codex`, `grok`,
//! `antigravity`) filters `role == System` and joins those messages into
//! the vendor's top-level `system` / `systemInstruction` — so the "appended"
//! injection was in fact hoisted into the root prompt. The root grew on the
//! turn a skill matched and shrank again on the next turn that matched
//! nothing: a full cache bust in both directions, destroying the KV prompt
//! cache back to token 0 and violating ADR-0017's byte-identical
//! `messages[0]` guarantee (FR-CACHE-5/6).
//!
//! The fix is the issue's own shape: a `User`-role message carrying the
//! `[skill instructions]` wrapper, which stays in the conversational stream
//! where the model reads it as injected context.
//!
//! Verifies: the whole turn through the real host-side skill transform —
//! the hoisted root string and `messages[0]` are byte-identical across a
//! match and across match-then-unmatch, and the injection is the only
//! appended message, carrying the wrapper as a user turn.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use lca_protocol::{ChatMessage, CompletionRequest, ContentBlock, MessageRole};
use lca_tools::skills::{Skill, SkillSource};

fn skill(name: &str, word: &str, body: &str) -> Skill {
    Skill {
        name: name.to_string(),
        description: String::new(),
        match_words: vec![word.to_string()],
        model_invocable: true,
        body: body.to_string(),
        source: SkillSource::Project,
    }
}

/// The conversation a mid-session turn sees: the root system prompt, two
/// completed turns, and the newest user message.
fn conversation(latest: &str) -> Vec<ChatMessage> {
    vec![
        ChatMessage::text(MessageRole::System, "You are LCA."),
        ChatMessage::text(MessageRole::User, "earlier question"),
        ChatMessage::text(MessageRole::Assistant, "earlier answer"),
        ChatMessage::text(MessageRole::User, latest),
    ]
}

/// The hoisting rule every provider builder applies: join every `System`
/// message into the vendor's root string. Asserting on it asserts the
/// observable contract the builders rely on.
fn hoist(messages: &[ChatMessage]) -> String {
    messages
        .iter()
        .filter(|message| message.role == MessageRole::System)
        .flat_map(|message| {
            message.content.iter().filter_map(|block| match block {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn text_of(message: &ChatMessage) -> String {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

/// Run the real host-side transform (gh #34/ADR-0034's merge path).
fn transformed(latest: &str, skills: &[Skill]) -> Vec<ChatMessage> {
    lca_tools::skills::transform(conversation(latest), skills, true)
}

// Verifies: gh #299 acceptance 1+2 - a match leaves the hoisted root
// byte-identical and `messages[0]` untouched.
#[test]
fn a_skill_match_never_mutates_the_root_system_prompt() {
    let skills = [skill("deploy", "deploy", "Always run the smoke test.")];
    let baseline = conversation("please deploy it");
    let matched = transformed("please deploy it", &skills);
    assert_eq!(
        hoist(&matched),
        hoist(&baseline),
        "the root system prompt must not grow when a skill matches"
    );
    assert_eq!(
        matched[0], baseline[0],
        "messages[0] stays byte-identical (FR-CACHE-5)"
    );
}

// Verifies: gh #299 acceptance 1 - match-then-unmatch: the root never
// changes in either direction, so the cache is never busted twice.
#[test]
fn match_then_unmatch_never_touches_the_root() {
    let skills = [skill("deploy", "deploy", "Always run the smoke test.")];
    let baseline = conversation("please deploy it");
    let root = hoist(&baseline);

    let matched = transformed("please deploy it", &skills);
    let unmatched = transformed("please deploy it", &[]);
    for (label, messages) in [("match", &matched), ("unmatch", &unmatched)] {
        assert_eq!(hoist(messages), root, "{label} turn");
        assert_eq!(messages[0], baseline[0], "{label} turn: messages[0]");
    }
}

// Verifies: gh #299 acceptance 3 - the injection is the only appended
// message, it is a USER turn, and it carries the issue's wrapper.
#[test]
fn the_injection_is_an_appended_user_message_with_the_wrapper() {
    let skills = [skill("deploy", "deploy", "Always run the smoke test.")];
    let baseline = conversation("please deploy it");
    let matched = transformed("please deploy it", &skills);

    assert_eq!(
        matched.len(),
        baseline.len() + 1,
        "exactly one appended message"
    );
    assert_eq!(
        matched[..baseline.len()],
        baseline[..],
        "the prefix is byte-identical"
    );
    let injection = matched.last().expect("the injection");
    assert_eq!(injection.role, MessageRole::User);
    let text = text_of(injection);
    assert!(
        text.starts_with("[skill instructions]\n"),
        "the issue's exact wrapper: {text}"
    );
    assert!(text.contains("Always run the smoke test."), "{text}");
    assert!(
        text.contains("from project"),
        "the host-side merge attributes its source: {text}"
    );
}

// Verifies: gh #299 acceptance 3 (structural) - after a match the only
// `System` message in the whole request is `messages[0]`. This is the
// property that makes the hoist safe for every builder at once, pinned
// independently of any single vendor's payload shape.
#[test]
fn no_second_system_message_ever_appears() {
    let skills = [skill("deploy", "deploy", "Always run the smoke test.")];
    let matched = transformed("please deploy it", &skills);
    let system_indices: Vec<usize> = matched
        .iter()
        .enumerate()
        .filter(|(_, message)| message.role == MessageRole::System)
        .map(|(index, _)| index)
        .collect();
    assert_eq!(
        system_indices,
        vec![0],
        "a trailing System message is hoisted into the root prompt"
    );
}

// Verifies: gh #299 acceptance 2 - the stable prefix is untouched, so the
// cache boundary the provider is told about still covers the same bytes.
#[test]
fn the_stable_prefix_still_covers_the_same_bytes() {
    let skills = [skill("deploy", "deploy", "Always run the smoke test.")];
    let matched = transformed("please deploy it", &skills);
    let request = CompletionRequest {
        messages: matched,
        tools: Vec::new(),
        model: "any-model".to_string(),
        stable_prefix: 4,
        extras: Default::default(),
    };
    let prefix = &request.messages[..request.stable_prefix];
    assert_eq!(
        prefix,
        &conversation("please deploy it")[..4],
        "the stable prefix is the un-injected conversation, byte for byte"
    );
    assert!(
        request
            .messages
            .iter()
            .skip(request.stable_prefix)
            .any(|message| text_of(message).contains("[skill instructions]")),
        "the injection lands past the boundary, never inside it (FR-CACHE-6)"
    );
}
