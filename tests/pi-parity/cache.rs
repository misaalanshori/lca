//! Cache-waste parity: pi's `cache-stats.ts` method, replayed through LCA's
//! measurement. Token math matches pi exactly; dollar rates come from the
//! turn's own cost buckets (see `docs/pi-parity.md`).

use lca_protocol::{Record, Usage};
use lca_session::compute_cache_waste;

const NOISE_FLOOR: u64 = 1024;

fn usage(input: u64, cache_read: u64, cache_write: u64, cost_input: f64, cost_write: f64) -> Usage {
    Usage {
        input,
        output: 10,
        cache_read,
        cache_write,
        cache_write_1h: 0,
        cost: 0.0,
        cost_input,
        cost_cache_read: 0.0,
        cost_cache_write: cost_write,
        extras: Default::default(),
    }
}

fn assistant(id: &str, ts: u64, model: &str, usage: Usage) -> Record {
    Record::Assistant {
        v: 1,
        ts,
        id: id.to_string(),
        parent: None,
        content: vec![],
        reasoning: None,
        reasoning_signature: None,
        provider_thinking_level: None,
        model: Some(model.to_string()),
        provider: Some("p".to_string()),
        usage: Some(usage),
    }
}

fn compaction(id: &str) -> Record {
    Record::Compaction {
        v: 1,
        ts: 3,
        id: id.to_string(),
        parent: None,
        replaced_from: "u0".to_string(),
        replaced_to: "a0".to_string(),
        first_kept_id: String::new(),
        read_files: Vec::new(),
        modified_files: Vec::new(),
        system_prompt: None,
        summary: "summary".to_string(),
        strategy: "parity-harness".to_string(),
        usage: None,
    }
}

// Verifies: pi:packages/coding-agent/test/cache-stats.test.ts#counts-nothing-for-healthy-sessions
#[test]
fn pi_parity_cache_healthy_turns_have_no_waste() {
    let records = vec![
        assistant("a1", 0, "m", usage(0, 0, 100_000, 0.0, 0.375)),
        assistant("a2", 60_000, "m", usage(0, 100_000, 5_000, 0.0, 0.019)),
    ];
    let totals = compute_cache_waste(&records, NOISE_FLOOR);
    assert_eq!(
        totals.missed_tokens, 0,
        "a full cache read bills nothing twice"
    );
    assert_eq!(totals.missed_cost, 0.0);
}

// Verifies: pi:packages/coding-agent/test/cache-stats.test.ts#accumulates-missed-tokens-and-cost-across-turns
#[test]
fn pi_parity_cache_full_miss_bills_the_previous_prompt() {
    let records = vec![
        assistant("a1", 0, "m", usage(0, 0, 100_000, 0.0, 0.375)),
        assistant("a2", 60_000, "m", usage(0, 100_000, 5_000, 0.0, 0.019)),
        assistant("a3", 120_000, "m", usage(0, 0, 110_000, 0.4125, 0.0)),
    ];
    let totals = compute_cache_waste(&records, NOISE_FLOOR);
    assert_eq!(
        totals.missed_tokens, 105_000,
        "the previous 105k prompt, rebilled"
    );
    assert!(
        totals.missed_cost > 0.0,
        "a real miss has a real dollar cost"
    );
}

// Verifies: pi:packages/coding-agent/test/cache-stats.test.ts#skips-the-turn-after-a-compaction-reset
#[test]
fn pi_parity_cache_baseline_resets_on_compaction() {
    let records = vec![
        assistant("a1", 0, "m", usage(0, 0, 100_000, 0.0, 0.375)),
        compaction("c1"),
        assistant("a2", 60_000, "m", usage(0, 0, 20_000, 0.075, 0.0)),
    ];
    let totals = compute_cache_waste(&records, NOISE_FLOOR);
    assert_eq!(
        totals.missed_tokens, 0,
        "a changed prompt is new content, not waste"
    );
}

// Verifies: pi:packages/coding-agent/docs/settings.md (the 1024-token
// noise floor LCA documents beside `cache.noise_floor_tokens`).
#[test]
fn pi_parity_cache_noise_floor_swallows_small_misses() {
    let records = vec![
        assistant("a1", 0, "m", usage(0, 0, 100_000, 0.0, 0.375)),
        assistant("a2", 60_000, "m", usage(0, 99_500, 500, 0.0, 0.0)),
    ];
    let totals = compute_cache_waste(&records, NOISE_FLOOR);
    assert_eq!(
        totals.missed_tokens, 0,
        "a 500-token miss is breakpoint noise"
    );
    assert_eq!(totals.miss_count, 0);
}

// Verifies: pi:packages/coding-agent/src/core/cache-stats.ts (a model
// switch re-bills the full prompt; that cost is real and worth surfacing,
// so no exemption — LCA's `docs/inspiration.md` adopts this explicitly).
#[test]
fn pi_parity_cache_model_change_does_not_reset() {
    let records = vec![
        assistant("a1", 0, "m1", usage(0, 0, 100_000, 0.0, 0.375)),
        assistant("a2", 60_000, "m2", usage(0, 0, 100_000, 0.375, 0.0)),
    ];
    let totals = compute_cache_waste(&records, NOISE_FLOOR);
    assert_eq!(
        totals.missed_tokens, 100_000,
        "the switch is counted, never exempt"
    );
}

// Verifies: pi:packages/coding-agent/src/core/cache-stats.ts (a provider
// that never reported cache activity reads as no measurable waste, not a
// permanent total miss).
#[test]
fn pi_parity_cache_silent_provider_has_no_measurable_waste() {
    let records = vec![
        assistant("a1", 0, "m", usage(50_000, 0, 0, 0.0, 0.0)),
        assistant("a2", 60_000, "m", usage(52_000, 0, 0, 0.0, 0.0)),
    ];
    let totals = compute_cache_waste(&records, NOISE_FLOOR);
    assert_eq!(
        totals.missed_tokens, 0,
        "no cache signal means no measurable waste"
    );
    assert_eq!(totals.miss_count, 0);
}

// ---------------------------------------------------------------------------
// gh #299: skill injection must not mutate the vendor's root system prompt
// ---------------------------------------------------------------------------
//
// A skill match mid-session appends its instructions to the conversational
// stream. If that message is a `System` role, every provider builder's
// `filter(role == System)` hoists it into the vendor's top-level `system` /
// `systemInstruction`, so the root prompt grows on the match turn and
// shrinks again on the next turn that matches nothing — a full cache bust
// in both directions, destroying the KV cache back to token 0 and violating
// ADR-0017's `messages[0]` byte-identical guarantee (FR-CACHE-5/6).
//
// These rows assert the OBSERVABLE contract: the built payload's root
// string, before and after a match.

use lca_protocol::{ChatMessage, CompletionRequest, ContentBlock, MessageRole};
use skills::Skill;

fn skill(name: &str, word: &str, body: &str) -> Skill {
    Skill {
        name: name.to_string(),
        match_words: vec![word.to_string()],
        body: body.to_string(),
    }
}

/// A request shaped like a real mid-session turn: a system prompt, a
/// couple of prior turns, and the newest user message.
fn session_request(model: &str, latest: &str) -> CompletionRequest {
    CompletionRequest {
        messages: vec![
            ChatMessage::text(MessageRole::System, "You are LCA."),
            ChatMessage::text(MessageRole::User, "earlier question"),
            ChatMessage::text(MessageRole::Assistant, "earlier answer"),
            ChatMessage::text(MessageRole::User, latest),
        ],
        tools: Vec::new(),
        model: model.to_string(),
        stable_prefix: 1,
        extras: Default::default(),
    }
}

/// Apply the real transform the extension runs: match against the latest
/// user message and append the injection.
fn with_skill_injection(request: &CompletionRequest, skills: &[Skill]) -> CompletionRequest {
    let messages = skills::transform_with_skills(request.messages.clone(), skills);
    CompletionRequest {
        messages,
        ..request.clone()
    }
}

/// The rule every provider builder uses: join every `System` message into
/// the vendor's root string. Reproduced here because the builders take that
/// string as a parameter — so this is exactly the quantity a trailing
/// `System` message would corrupt, and asserting on it asserts the
/// contract the builders rely on.
fn hoist_system(request: &CompletionRequest) -> String {
    request
        .messages
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

fn root_of(body: &serde_json::Value, key: &str) -> String {
    match body.get(key) {
        Some(serde_json::Value::String(text)) => text.clone(),
        Some(serde_json::Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|block| block.get("text").and_then(|text| text.as_str()))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

// Verifies: gh #299 acceptance 1 (Anthropic) - a mid-session skill match
// leaves the vendor's root `system` string byte-identical.
#[test]
fn pi_parity_a_skill_match_leaves_the_anthropic_root_system_unchanged() {
    let skills = [skill("deploy", "deploy", "Always run the smoke test.")];
    let before = session_request("claude-x", "how do I work here?");
    let after = session_request("claude-x", "please deploy it");

    let before_body =
        lca_wire_anthropic::build_messages_body(&before, "You are LCA.", "claude-x", 1024, true);
    let after_body = lca_wire_anthropic::build_messages_body(
        &with_skill_injection(&after, &skills),
        "You are LCA.",
        "claude-x",
        1024,
        true,
    );
    assert_eq!(
        root_of(&before_body, "system"),
        root_of(&after_body, "system"),
        "the root system prompt must not grow when a skill matches"
    );
}

// Verifies: gh #299 acceptance 3 - the injection rides the body stream as
// a USER message carrying the `[skill instructions]` wrapper, never the
// root.
#[test]
fn pi_parity_the_injection_lands_in_the_stream_as_a_wrapped_user_message() {
    let skills = [skill("deploy", "deploy", "Always run the smoke test.")];
    let request = with_skill_injection(&session_request("claude-x", "please deploy it"), &skills);
    let injection = request.messages.last().expect("the injection appends");
    assert_eq!(
        injection.role,
        MessageRole::User,
        "a trailing system message would be hoisted into the root"
    );
    let text: String = injection
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        text.starts_with("[skill instructions]\n"),
        "the issue's exact wrapper shape: {text}"
    );
    assert!(text.contains("Always run the smoke test."), "{text}");
}

// Verifies: gh #299 acceptance 1+2 - match-then-unmatch: the root never
// changes in either direction, and `messages[0]` stays byte-identical.
#[test]
fn pi_parity_match_then_unmatch_never_touches_the_root() {
    let skills = [skill("deploy", "deploy", "Always run the smoke test.")];
    let baseline = session_request("claude-x", "please deploy it");
    let base_root = hoist_system(&baseline);
    let base_body =
        lca_wire_anthropic::build_messages_body(&baseline, &base_root, "claude-x", 1024, true);

    let matched = with_skill_injection(&session_request("claude-x", "please deploy it"), &skills);
    let unmatched =
        with_skill_injection(&session_request("claude-x", "unrelated question"), &skills);
    for (label, request) in [("match", &matched), ("unmatch", &unmatched)] {
        let body = lca_wire_anthropic::build_messages_body(
            request,
            "You are LCA.",
            "claude-x",
            1024,
            true,
        );
        assert_eq!(root_of(&body, "system"), base_root, "{label} turn");
        assert_eq!(
            body["messages"][0], base_body["messages"][0],
            "{label} turn: messages[0] must stay byte-identical (FR-CACHE-5)"
        );
    }
}

// Verifies: gh #299 acceptance 1 (Google shape) - antigravity's
// `systemInstruction` is unchanged by a skill match.
#[test]
fn pi_parity_a_skill_match_leaves_the_google_system_instruction_unchanged() {
    let skills = [skill("deploy", "deploy", "Always run the smoke test.")];
    let matched = with_skill_injection(
        &session_request("gemini-3.8-flash", "please deploy it"),
        &skills,
    );
    let body = antigravity::build_request(&matched, "You are LCA.", "proj", "gemini-3.8-flash");
    let root = body
        .get("systemInstruction")
        .map(|value| value.to_string())
        .unwrap_or_default();
    assert_eq!(
        root,
        antigravity::build_request(
            &session_request("gemini-3.8-flash", "how do I work here?"),
            "You are LCA.",
            "proj",
            "gemini-3.8-flash",
        )
        .get("systemInstruction")
        .map(|value| value.to_string())
        .unwrap_or_default(),
        "the Google root instruction must not grow when a skill matches"
    );
    // And the injection still reached the body's contents as a user turn.
    assert!(
        body["request"]["contents"]
            .to_string()
            .contains("[skill instructions]"),
        "the wrapper rides the conversational stream: {}",
        body["request"]["contents"]
    );
}

// Verifies: gh #299 acceptance 1 (OpenAI shape, the codex/grok builders
// share it) - the root `instructions` string is unchanged.
#[test]
fn pi_parity_a_skill_match_leaves_the_openai_instructions_unchanged() {
    let skills = [skill("deploy", "deploy", "Always run the smoke test.")];
    let before = session_request("gpt-x", "please deploy it");
    let matched = with_skill_injection(&session_request("gpt-x", "please deploy it"), &skills);
    let base = lca_wire_openai::build_responses_body(&before, "You are LCA.", "gpt-x", None);
    let after = lca_wire_openai::build_responses_body(&matched, "You are LCA.", "gpt-x", None);
    assert_eq!(
        base.get("instructions").map(|v| v.to_string()),
        after.get("instructions").map(|v| v.to_string()),
        "the OpenAI root instructions must not grow when a skill matches"
    );
}

// Verifies: gh #299 acceptance 3 (the root cause, structurally) - after a
// skill match the ONLY `System` message is `messages[0]`. This is the
// property that makes the hoist safe for every builder at once, so it is
// pinned independently of any one vendor's payload.
#[test]
fn pi_parity_a_skill_match_adds_no_second_system_message() {
    let skills = [skill("deploy", "deploy", "Always run the smoke test.")];
    let injected = with_skill_injection(&session_request("any-model", "please deploy it"), &skills);
    let system_roles: Vec<usize> = injected
        .messages
        .iter()
        .enumerate()
        .filter(|(_, message)| message.role == MessageRole::System)
        .map(|(index, _)| index)
        .collect();
    assert_eq!(
        system_roles,
        vec![0],
        "only messages[0] may be System; a trailing one is hoisted into the root"
    );
    assert_eq!(
        injected.messages[0],
        session_request("any-model", "please deploy it").messages[0],
        "messages[0] stays byte-identical (FR-CACHE-5)"
    );
}
