//! GitHub #303 (NFR-24): `SharedPrompt` must forward tool annotations, and a
//! hint must never become a bypass.
//!
//! ## The defect, and what was already fixed when this file was written
//!
//! `PermissionPrompt::ask_annotated` exists so a permission prompt can show a
//! tool's hints (`read-only`, `destructive`, `open-world`), and `authorize`
//! routes every capability decision through it. But `SharedPrompt` — the slot
//! **every** capability engine holds, WASM and native alike — originally
//! implemented only `ask` and `review_proposals`, so the trait's default body
//! silently called plain `ask` and the annotations were discarded on the path
//! almost everything takes. A user approving an MCP call never saw that the
//! server had declared the tool `destructive`.
//!
//! The delegation itself landed in `ba7f7fce` (#418 Phase C) with a row in
//! `crates/lca-permissions/tests/permissions.rs`. This file is the *regression
//! guard* NFR-24 asks for, and it covers the three things that row does not:
//!
//! 1. **A denying prompt stays denying under every hint.** The landed row uses
//!    an allowing prompt and compares "allowed with hints" to "allowed without"
//!    — which proves nothing about the case that matters. ADR-0047: "the host
//!    still refuses to treat a server's `readOnlyHint` as a bypass". A
//!    `read-only` hint on a destructive action must not flip a deny, and this
//!    file pins that for all four hints, both directions.
//! 2. **The slot with nothing installed denies**, hints or no hints — the
//!    headless answer must not become "annotated means approved".
//! 3. **The class sweep**: `CallbackPrompt` in `lca-sdk` is a *production*
//!    prompt that overrides `ask` but not `ask_annotated`, so an embedder's
//!    callback receives the bare action text. It is outside this cycle's file
//!    boundary, so it is recorded here as the sweep's finding and filed as a
//!    follow-up rather than silently fixed.
//!
//! Verifies: NFR-24 (gh #303), ADR-0047's no-bypass line, and gh #78's
//! annotation surface reaching the prompt rather than dying in the wrapper.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use lca_permissions::{Action, Decision, GrantStore, PermissionPrompt, ProposalDiff, SharedPrompt};
use lca_protocol::ToolAnnotations;

fn scratch(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("lca-303-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("scratch");
    root
}

/// Every hint at once, so a row can prove no single one of them decides.
fn all_hints() -> ToolAnnotations {
    ToolAnnotations {
        read_only_hint: Some(true),
        destructive_hint: Some(true),
        idempotent_hint: Some(true),
        open_world_hint: Some(true),
    }
}

/// What a recording prompt saw: the rendered surface, and whether an
/// annotations object was forwarded at all.
type Seen = Arc<Mutex<Vec<(String, bool)>>>;

/// Records what it was shown and answers with a fixed decision, so a row can
/// separate "what reached the prompt" from "what the prompt decided".
struct Fixed {
    decision: Decision,
    seen: Seen,
}

impl Fixed {
    fn new(decision: Decision) -> (Fixed, Seen) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        (
            Fixed {
                decision,
                seen: seen.clone(),
            },
            seen,
        )
    }
}

impl PermissionPrompt for Fixed {
    fn ask(&mut self, action: &Action) -> Decision {
        self.seen
            .lock()
            .expect("lock")
            .push((format!("plain:{}", action.display()), false));
        self.decision
    }

    fn ask_annotated(
        &mut self,
        action: &Action,
        annotations: Option<&ToolAnnotations>,
    ) -> Decision {
        self.seen.lock().expect("lock").push((
            format!("annotated:{}", action.display_annotated(annotations)),
            annotations.is_some(),
        ));
        self.decision
    }

    fn review_proposals(&mut self, _diff: &ProposalDiff) -> bool {
        false
    }
}

/// A write outside the workspace, so a grant rule cannot answer it first.
fn outside_write() -> Action {
    Action::WritePath {
        path: PathBuf::from("/etc/lca-303-hosts"),
    }
}

/// Drive one ask through the slot the engines actually hold.
fn through_slot(
    tag: &str,
    decision: Decision,
    annotations: Option<&ToolAnnotations>,
) -> (Vec<(String, bool)>, Decision, bool) {
    let _ = decision;
    let root = scratch(tag);
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("mkdir");
    let mut store = GrantStore::open(&root.join("grants.json")).expect("open");
    let slot = SharedPrompt::default();
    let (prompt, seen) = Fixed::new(decision);
    slot.set(Arc::new(Mutex::new(prompt)));
    let mut via_slot = slot.clone();
    let outcome = lca_permissions::authorize(
        &mut store,
        &project,
        &outside_write(),
        None,
        &mut via_slot,
        annotations,
    )
    .expect("authorize");
    let seen = seen.lock().expect("lock").clone();
    let answered = if outcome.allowed {
        Decision::Always
    } else {
        Decision::Denied
    };
    (seen, answered, outcome.prompted)
}

/// The 4th thing this file must prove, and the first: a hint is not a bypass.
///
/// ADR-0047 fixes the host's position — it refuses to treat a server's
/// `readOnlyHint` as permission — so a denying prompt must deny identically
/// with and without hints, for every hint, including the ones a naive
/// optimisation would be tempted to trust (`read_only`, `idempotent`).
#[test]
fn a_hint_never_turns_a_deny_into_an_allow() {
    let (bare, bare_decision, bare_asked) = through_slot("deny-bare", Decision::Denied, None);
    assert_eq!(bare.len(), 1, "the prompt was asked once: {bare:?}");
    assert!(bare_asked, "an outside write asks");
    assert_eq!(bare_decision, Decision::Denied);

    let (hinted, hinted_decision, hinted_asked) =
        through_slot("deny-hinted", Decision::Denied, Some(&all_hints()));
    assert_eq!(hinted.len(), 1, "the prompt was asked once: {hinted:?}");
    assert!(hinted_asked, "an outside write asks, hints or not");
    assert_eq!(
        hinted_decision,
        Decision::Denied,
        "all four hints together must not override a denying prompt"
    );
    assert_eq!(
        hinted_decision, bare_decision,
        "the hint set changed the decision, which is the bypass ADR-0047 forbids"
    );
    // The hints did arrive - this is the display fix, not an absence check.
    let (surface, had_annotations) = hinted.first().expect("one ask");
    assert!(had_annotations, "the slot forwarded an annotations object");
    assert!(
        surface.contains("read-only") && surface.contains("destructive"),
        "the surface shows the hints: {surface}"
    );
}

/// Each hint on its own, so a future "optimisation" on one cannot pass by
/// being masked by another.
#[test]
fn no_single_hint_flips_a_deny() {
    for (name, hint) in [
        (
            "read-only",
            ToolAnnotations {
                read_only_hint: Some(true),
                ..Default::default()
            },
        ),
        (
            "destructive",
            ToolAnnotations {
                destructive_hint: Some(true),
                ..Default::default()
            },
        ),
        (
            "idempotent",
            ToolAnnotations {
                idempotent_hint: Some(true),
                ..Default::default()
            },
        ),
        (
            "open-world",
            ToolAnnotations {
                open_world_hint: Some(true),
                ..Default::default()
            },
        ),
    ] {
        let (seen, decision, _) =
            through_slot(&format!("single-{name}"), Decision::Denied, Some(&hint));
        assert_eq!(decision, Decision::Denied, "a `{name}` hint flipped a deny");
        assert!(
            seen.iter()
                .any(|(surface, annotated)| *annotated && surface.contains(name)),
            "the `{name}` hint still reached the surface: {seen:?}"
        );
    }
}

/// The slot with nothing installed denies — the headless answer — and must not
/// become "annotated therefore approved" now that annotations are forwarded.
#[test]
fn an_empty_slot_denies_with_or_without_hints() {
    let root = scratch("empty-slot");
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("mkdir");
    let mut store = GrantStore::open(&root.join("grants.json")).expect("open");
    for annotations in [None, Some(all_hints())] {
        let mut slot = SharedPrompt::default();
        let outcome = lca_permissions::authorize(
            &mut store,
            &project,
            &outside_write(),
            None,
            &mut slot,
            annotations.as_ref(),
        )
        .expect("authorize");
        assert!(
            !outcome.allowed(),
            "an installed-nothing slot denies, hints or not"
        );
    }
}

/// `None` annotations are the model-tool path's shape and must behave exactly
/// like plain `ask`: same answer, and the surface is the bare display text
/// (no empty "(  )" suffix from a present-but-empty annotation object).
#[test]
fn none_annotations_behave_exactly_like_ask() {
    let (seen, decision, _) = through_slot("none", Decision::Always, None);
    let (surface, had_annotations) = seen.first().expect("one ask");
    assert!(!had_annotations, "no annotations object was invented");
    assert_eq!(
        surface.as_str(),
        format!("annotated:{}", outside_write().display()).as_str(),
        "the surface is the action's plain display"
    );
    assert_eq!(decision, Decision::Always, "the wrapped answer stands");

    // An all-`None` annotation object is *present* but must render exactly
    // like the bare action, so the model path and a hint-less MCP server look
    // identical to the user.
    let empty = ToolAnnotations::default();
    assert_eq!(
        outside_write().display_annotated(Some(&empty)),
        outside_write().display(),
        "an annotation object with no hint set appends nothing"
    );
}

/// An allow stays an allow: forwarding must not turn an approving prompt into
/// a denying one either (the landed row covers only this direction, which is
/// why the deny rows above exist).
#[test]
fn an_allow_stays_an_allow() {
    let (bare, bare_decision, _) = through_slot("allow-bare", Decision::Always, None);
    let (hinted, hinted_decision, _) =
        through_slot("allow-hinted", Decision::Always, Some(&all_hints()));
    assert_eq!(bare_decision, Decision::Always);
    assert_eq!(hinted_decision, Decision::Always);
    assert_eq!(bare.len(), hinted.len(), "one ask either way");
    assert!(
        hinted[0].0.len() > bare[0].0.len(),
        "the hinted surface carries the extra words"
    );
}

/// Lock discipline (the brief's point 3).
///
/// Two facts, both checked here:
///
/// 1. **There is no `.await` anywhere under a lock.** `SharedPrompt` is
///    synchronous — `ask`, `ask_annotated` and `review_proposals` all return
///    plain values — so the "prompt held across an unrelated await" defect
///    cannot exist in this type by construction. Recorded rather than assumed:
///    the source scan below fails if anyone makes a method `async`.
/// 2. **The slot lock is released before the inner prompt is called.**
///    `current()` takes the slot mutex, clones the inner `Arc`, and returns —
///    so the mutex is dropped before `.lock().ask(...)` runs. The consequence
///    that matters is `set()`: the interface swaps the prompt between turns,
///    and if `ask` held the slot lock for the whole modal, `set()` would block
///    for as long as a human takes to answer. This row proves it does not, by
///    calling `set()` from another thread while an ask is blocked inside the
///    inner prompt.
///
/// (The inner prompt's *own* `Mutex` is of course held across its call — that
/// is what `&mut self` requires, and it is the same span `ask` uses. A prompt
/// that re-entered itself would deadlock, which is not a shape any prompt has.)
#[test]
fn the_slot_lock_is_released_before_the_inner_prompt_runs() {
    /// Blocks inside `ask` until told to continue, so `set()` has a window in
    /// which a wrongly-held slot lock would block it.
    struct Blocking {
        entered: std::sync::mpsc::Sender<()>,
        release: std::sync::mpsc::Receiver<()>,
    }
    impl PermissionPrompt for Blocking {
        fn ask(&mut self, _action: &Action) -> Decision {
            let _ = self.entered.send(());
            let _ = self.release.recv();
            Decision::Denied
        }
        fn review_proposals(&mut self, _diff: &ProposalDiff) -> bool {
            false
        }
    }

    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let slot = SharedPrompt::default();
    slot.set(Arc::new(Mutex::new(Blocking {
        entered: entered_tx,
        release: release_rx,
    })));

    let asker = {
        let mut via_slot = slot.clone();
        std::thread::spawn(move || PermissionPrompt::ask(&mut via_slot, &outside_write()))
    };
    // Wait until the modal is genuinely open (inside the inner prompt).
    entered_rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("the ask reached the inner prompt");

    // While that ask is blocked, swapping the prompt must not block.
    let (prompt, _seen) = Fixed::new(Decision::Always);
    slot.set(Arc::new(Mutex::new(prompt)));
    release_tx.send(()).expect("release the blocked ask");
    assert_eq!(
        asker.join().expect("asker thread"),
        Decision::Denied,
        "the ask completed against the prompt installed when it started"
    );

    // No `.await` under a lock: the three methods are synchronous. A future
    // `async fn` here would break this scan, which is the point.
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/crates/lca-permissions/src/lib.rs"
    ))
    .expect("permissions source is readable");
    let impl_block = &source[source
        .find("impl PermissionPrompt for SharedPrompt")
        .expect("slot impl found")..];
    let body = &impl_block[..impl_block.find("\n}\n").unwrap_or(impl_block.len())];
    assert!(
        !body.contains("async fn") && !body.contains(".await"),
        "SharedPrompt stays synchronous, so nothing is awaited under its lock"
    );
}

/// The class sweep, as an executable finding: every in-tree `impl
/// PermissionPrompt` that overrides `ask` without overriding
/// `ask_annotated` gets the trait default, which drops hints.
///
/// `SharedPrompt` is fixed. `CallbackPrompt` in `lca-sdk` is the one
/// **production** implementer still relying on the default, so an embedder's
/// approval callback is shown the bare action even when the host had hints.
/// It is outside this cycle's boundary (`crates/lca-sdk/**` is not permitted),
/// so it is pinned here as a named follow-up rather than fixed in passing.
#[test]
fn the_only_production_prompt_still_dropping_hints_is_the_known_one() {
    let sdk = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/crates/lca-sdk/src/lib.rs"
    ))
    .expect("the sdk source is readable");

    // `CallbackPrompt` overrides `ask` (it is the embedder's callback) and has
    // no `ask_annotated`, so the default flattens the hints.
    assert!(
        sdk.contains("struct CallbackPrompt")
            && sdk.contains("impl PermissionPrompt for CallbackPrompt"),
        "the sweep still has its subject: CallbackPrompt exists"
    );
    let after = &sdk[sdk
        .find("impl PermissionPrompt for CallbackPrompt")
        .expect("impl found")..];
    let body = &after[..after.find("\n}\n").unwrap_or(after.len())];
    assert!(
        !body.contains("fn ask_annotated"),
        "CallbackPrompt still relies on the default body - when this fails, the \
         follow-up landed and this row should become a positive assertion"
    );

    // And the fixed one stays fixed.
    let permissions = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/crates/lca-permissions/src/lib.rs"
    ))
    .expect("permissions source is readable");
    let shared = &permissions[permissions
        .find("impl PermissionPrompt for SharedPrompt")
        .expect("slot impl found")..];
    let shared_body = &shared[..shared.find("\n}\n").unwrap_or(shared.len())];
    assert!(
        shared_body.contains("fn ask_annotated") && shared_body.contains("ask_annotated(action"),
        "the slot forwards annotations to the installed prompt"
    );
}

/// The supply side: **three** distinct surfaces, and only one of them supplies.
///
/// An earlier version of this row said "nothing in a real tool-call path
/// supplies hints". That was too broad, and correcting it is the substance of
/// this cycle. There are three places a permission decision is made, and the
/// hint reaches exactly one of them:
///
/// | surface | supplied? |
/// |---|---|
/// | the turn's gate for a **built-in** tool (`lca-core` `authorize`) | **yes** — `tool_annotations(name)` resolves the spec's hints, and `shell_prompt_shows_builtin_hints` proves the surface reads `destructive` |
/// | anything an **extension** reaches through `Capabilities` (`fs_write`, `process`, `net`) | no — `Capabilities` has no annotations channel |
/// | an **MCP tool call** | no — and not because the hint is dropped: `required_permission` returns `None` for a name it does not recognise, so the turn never asks for an MCP call at all |
///
/// So the user-visible defect is real but its cause is narrower than the issue
/// states: a built-in's hint already shows, an MCP tool's cannot because there
/// is no modal to show it in. This row pins all three so the follow-up is
/// named precisely instead of repeated as folklore.
#[test]
fn only_the_builtin_turn_gate_supplies_hints() {
    // (1) The built-in gate DOES supply them - a positive assertion, so this
    // row fails if the join is ever removed.
    let execute = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/crates/lca-core/src/turn/execute.rs"
    ))
    .expect("the turn source is readable");
    assert!(
        execute.contains("fn tool_annotations(")
            && execute.contains("tool_annotations(&call.name)"),
        "the turn resolves the called tool's annotations before authorize"
    );
    assert!(
        execute.contains("annotations.as_ref(),"),
        "the turn passes them to authorize"
    );

    // (2) `Capabilities` has no annotations channel, so an extension's own
    // capability action cannot carry a hint. Every `authorize(` there passes
    // `None`; a `Some(` would mean the channel exists and this row should
    // become a positive one.
    let capabilities = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/crates/lca-tools/src/capabilities/mod.rs"
    ))
    .expect("capabilities source is readable");
    for (index, _) in capabilities.match_indices("authorize(") {
        let call = &capabilities[index..];
        let end = call.find(")\n").unwrap_or(call.len().min(400));
        assert!(
            !call[..end].contains("Some("),
            "a capability authorize call now supplies hints - replace this row \
             with a positive assertion"
        );
    }

    // (3) An MCP tool call is never gated, so no prompt exists for its hints
    // to reach. `required_permission` recognises only built-in names.
    let tools = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/crates/lca-tools/src/lib.rs"
    ))
    .expect("the tool executor source is readable");
    let gate = &tools[tools
        .find("fn required_permission(")
        .expect("required_permission exists")..];
    let gate_body = &gate[..gate.find("\n    }\n").unwrap_or(gate.len().min(2000))];
    assert!(
        !gate_body.contains("mcp__"),
        "an MCP call is now gated - the supply question changed, so re-read this row"
    );
    // And the bridge itself never asks per call (it asks per spawn).
    let bridge = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/crates/lca-mcp/src/native.rs"
    ))
    .expect("mcp bridge source is readable");
    assert!(
        !bridge.contains("authorize("),
        "the MCP bridge now asks per tool call - replace this row with the modal assertion"
    );
}

/// `Some(ToolAnnotations::default())` is not `None`: default means "the tool
/// made no claims", `None` means "nothing is known about this tool". The brief
/// asked this be either distinguished or the flattening pinned; it is
/// **distinguished**, because the wrapper forwards whatever it is given and
/// `display_annotated` renders nothing for an all-`None` object — so the two
/// arrive as different values and decide identically.
#[test]
fn an_all_default_annotation_object_is_not_none() {
    let action = outside_write();
    let empty = ToolAnnotations::default();
    // Different values...
    assert_ne!(
        Some(&empty).is_some(),
        Option::<&ToolAnnotations>::None.is_some(),
        "`Some(default)` and `None` are different inputs"
    );
    // ...that render identically (no claims = nothing to show)...
    assert_eq!(
        action.display_annotated(Some(&empty)),
        action.display_annotated(None),
        "an all-default object appends no words"
    );
    // ...and both decide exactly as the bare action does, because the wrapper
    // forwards rather than inspecting.
    let (with_default, default_decision, _) =
        through_slot("default-object", Decision::Denied, Some(&empty));
    let (bare, bare_decision, _) = through_slot("default-bare", Decision::Denied, None);
    assert_eq!(default_decision, bare_decision);
    assert_eq!(default_decision, Decision::Denied);
    // The distinction is observable at the prompt, which is the point: the
    // recording prompt saw an annotations object in one case and none in the
    // other, even though the rendered text matches.
    assert!(with_default[0].1, "an object was forwarded");
    assert!(!bare[0].1, "no object was forwarded");
}

/// The extras path (gh #78): a WIT guest crosses no annotations field in WIT,
/// so its hints ride `ToolSpec.extras["annotations"]` as JSON and
/// `with_annotations_from_extras` lifts them into the real field. That decode
/// must not be duplicated - this row pins that it is the one path, and that a
/// guest's JSON arrives as typed hints a prompt can render.
#[test]
fn a_guest_annotations_json_reaches_the_prompt_through_the_one_decode() {
    use lca_protocol::ToolSpec;

    let spec = ToolSpec {
        name: "guest_tool".to_string(),
        description: "a guest tool".to_string(),
        parameters: serde_json::json!({"type": "object"}),
        exposure: lca_protocol::ToolExposure::Direct,
        namespace: None,
        annotations: None,
        extras: [(
            "annotations".to_string(),
            r#"{"readOnlyHint":false,"destructiveHint":true,"openWorldHint":true}"#.to_string(),
        )]
        .into_iter()
        .collect(),
    }
    .with_annotations_from_extras();
    let hints = spec.annotations.expect("an object was lifted from extras");

    // **Finding (gh #303 supply half), fixed in gh #454.** The JSON above uses
    // the camelCase spelling MCP servers and guests actually send
    // (`destructiveHint`), and `ToolAnnotations` used to derive `Deserialize`
    // with no `rename_all`, so serde expected `destructive_hint` and a camelCase
    // payload decoded to an all-`None` object *silently*. That is what this row
    // used to assert as a recorded gap.
    //
    // It is asserted the other way now, because the gap is closed: the payload
    // the world sends decodes, and the prompt renders what it says. The row is
    // **sanctioned to change by gh #454's brief** - it pinned the defect, so it
    // had to move with the fix.
    assert_eq!(
        hints.destructive_hint,
        Some(true),
        "a camelCase extras payload now decodes (gh #454)"
    );
    assert_eq!(hints.open_world_hint, Some(true));
    assert_eq!(hints.read_only_hint, Some(false));
    // The snake_case shape still decodes too (acceptance widened, not moved).
    assert_eq!(
        ToolAnnotations {
            destructive_hint: Some(true),
            open_world_hint: Some(true),
            ..Default::default()
        },
        serde_json::from_str::<ToolAnnotations>(
            r#"{"destructive_hint":true,"open_world_hint":true}"#
        )
        .expect("the old spelling still decodes"),
        "the snake_case shape keeps working"
    );

    // And what a prompt renders for a payload now names both.
    let understood = hints.clone();
    let rendered = outside_write().display_annotated(Some(&understood));
    assert!(
        rendered.contains("destructive") && rendered.contains("open-world"),
        "the lifted hints render: {rendered}"
    );

    // An explicit annotation wins over the extras copy (no second decode).
    let explicit = ToolSpec {
        name: "guest_tool".to_string(),
        description: String::new(),
        parameters: serde_json::json!({"type": "object"}),
        exposure: lca_protocol::ToolExposure::Direct,
        namespace: None,
        annotations: Some(ToolAnnotations {
            read_only_hint: Some(true),
            ..Default::default()
        }),
        extras: [(
            "annotations".to_string(),
            r#"{"destructiveHint":true}"#.to_string(),
        )]
        .into_iter()
        .collect(),
    }
    .with_annotations_from_extras();
    let kept = explicit.annotations.expect("explicit hints survive");
    assert_eq!(
        kept.read_only_hint,
        Some(true),
        "the structural field wins over the extras fallback"
    );
    assert_eq!(
        kept.destructive_hint, None,
        "the extras copy did not overwrite the structural field"
    );
}
