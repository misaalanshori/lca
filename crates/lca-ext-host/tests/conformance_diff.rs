//! The Phase 2 exit-test centerpiece: the conformance extension run in
//! both delivery modes produces identical results (NFR-25, the dual-mode
//! interchangeability promise from ADR-0013 and the SRDD).
//!
//! Both sides share the same roots, prompt, and grant store; the WASM
//! side reaches them through host imports, the native side calls the
//! same engine directly. Any divergence is a defect regardless of which
//! side "wins" (docs/testing-plan.md section6).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use std::sync::{Arc, Mutex};

use lca_ext_abi::{DeliveryMode, ExtensionDispatch, World};
use lca_ext_host::{ExtHost, ExtensionLimits, HostEnvironment};
use lca_ext_native::NativeRegistry;
use lca_permissions::{
    Decision, GrantStore, PermissionPrompt, ProposalDiff, ScopeGrant, ScopeRoots,
};
use lca_protocol::{CommandEffect, HookAction, ToolCall};

const FIXTURE: &[u8] = include_bytes!("../../../extensions/conformance/fixtures/tool-world.wasm");
const MANIFEST: &str = include_str!("../../../extensions/conformance/extension.toml");

struct Always;
impl PermissionPrompt for Always {
    fn ask(&mut self, _action: &lca_permissions::Action) -> Decision {
        Decision::Always
    }
    fn review_proposals(&mut self, _diff: &ProposalDiff) -> bool {
        false
    }
}

struct Fixture {
    root: std::path::PathBuf,
    prompt: Arc<Mutex<Always>>,
    store: Arc<Mutex<GrantStore>>,
    roots: ScopeRoots,
}

impl Fixture {
    fn new(name: &str) -> Fixture {
        let root = lca_testkit::scratch_path(&format!("lca-diff-{name}"));
        let _ = std::fs::remove_dir_all(&root);
        for dir in ["project", "private", "config", "data", "tmp"] {
            std::fs::create_dir_all(root.join(dir)).expect("mkdir");
        }
        std::fs::write(root.join("project/notes.txt"), "diff content").expect("file");
        // ADR-0030/0032: the conformance package's own resource bag, at the
        // install layout the host derives (`<state_dir>/extensions/<name>/resources`).
        // The native twin is pointed at the same directory, so the diff is
        // over identical bytes.
        let resources = root.join("data/extensions/conformance/resources");
        std::fs::create_dir_all(resources.join("skills")).expect("mkdir resources");
        std::fs::write(resources.join("greeting.txt"), "hello resources").expect("resource");
        std::fs::write(resources.join("skills/note.txt"), "skill note").expect("resource");
        let roots = ScopeRoots {
            workspace: root.join("project"),
            private: root.join("private"),
            home_config: root.join("config"),
            temp: root.join("tmp"),
            state_dir: root.join("data"),
        };
        Fixture {
            store: Arc::new(Mutex::new(
                GrantStore::open(&root.join("grants.json")).expect("store"),
            )),
            prompt: Arc::new(Mutex::new(Always)),
            roots,
            root,
        }
    }

    fn env(&self) -> Arc<HostEnvironment> {
        Arc::new(HostEnvironment {
            roots: self.roots.clone(),
            prompt: self.prompt.clone(),
            grant_store: self.store.clone(),
            project: self.root.join("project"),
            proposals: None,
            dialogs: lca_permissions::SharedDialogs::default(),
        })
    }

    fn capabilities(&self) -> Arc<lca_tools::Capabilities> {
        let mut cap = lca_tools::Capabilities::new(
            "conformance",
            lca_tools::CapabilityGrants {
                fs: vec![
                    ScopeGrant::parse("workspace", lca_permissions::FsMode::ReadWrite)
                        .expect("grant"),
                ],
                fs_declared: true,
                process: true,
                pty: true,
                ..Default::default()
            },
            self.roots.clone(),
            self.prompt.clone(),
            self.store.clone(),
            self.root.join("project"),
            None,
        );
        cap.set_resources(lca_tools::ResourceSource::Dir(
            self.root.join("data/extensions/conformance/resources"),
        ));
        Arc::new(cap)
    }

    fn both_modes(
        &self,
    ) -> (
        Arc<dyn ExtensionDispatch>,
        Arc<dyn ExtensionDispatch>,
        Arc<lca_tools::Capabilities>,
    ) {
        let mut host = ExtHost::new(
            ExtensionLimits {
                memory_bytes: 64 * 1024 * 1024,
                fuel_per_call: 100_000_000,
                log_limit_bytes: 4096,
            },
            self.env(),
        );
        let wasm = host.load(FIXTURE, MANIFEST).expect("load wasm mode");
        let engine = self.capabilities();
        let native = conformance::NativeConformance::new(engine.clone());
        // The same registry type holds both handle kinds (FR-EXT-6: one
        // handle type, no mode branching).
        let mut registry = NativeRegistry::new();
        registry.register(Arc::new(wasm));
        registry.register(Arc::new(native));
        let handles = registry.into_handles();
        // The SAME engine the native handle ran on, so its recorded
        // denials are visible to the test (FR-PERM-3).
        (handles[0].clone(), handles[1].clone(), engine)
    }
}

fn call(mode_args: &str) -> ToolCall {
    ToolCall {
        call_id: "c1".to_string(),
        name: "conformance".to_string(),
        arguments: mode_args.to_string(),
        parent_call_id: None,
    }
}

fn spawn_args(program: &str, marker: &str) -> String {
    if cfg!(unix) {
        format!(r#"{{"mode":"spawn","program":"{program}","args":["{marker}"],"cwd":"workspace"}}"#)
    } else {
        format!(
            r#"{{"mode":"spawn","program":"{program}","args":["/C","echo","{marker}"],"cwd":"workspace"}}"#
        )
    }
}

fn pty_args(program: &str, marker: &str) -> String {
    if cfg!(unix) {
        format!(
            r#"{{"mode":"pty","program":"{program}","args":["{marker}"],"cwd":"workspace","rows":24,"cols":80}}"#
        )
    } else {
        format!(
            r#"{{"mode":"pty","program":"{program}","args":["/C","echo","{marker}"],"cwd":"workspace","rows":24,"cols":80}}"#
        )
    }
}

// Verifies: NFR-25's substance, FR-EXT-7 (the two delivery labels
// differ by design), and the SRDD Phase 2 exit test - the conformance
// extension passes in native mode and WASM mode with identical
// results.
#[tokio::test]
async fn native_and_wasm_modes_produce_identical_results() {
    let fixture = Fixture::new("diff");
    let (wasm, native, _engine) = fixture.both_modes();

    // Identity and shape.
    assert_eq!(wasm.name(), native.name());
    assert_eq!(wasm.worlds(), native.worlds());
    assert_eq!(
        wasm.worlds().len(),
        16,
        "tool, tool-catalog, command, hooks, the eight hooks-*, provider, compaction, context-transform, ui"
    );
    assert_eq!(wasm.delivery(), DeliveryMode::Wasm);
    assert_eq!(native.delivery(), DeliveryMode::Native);
    // FR-EXT-7's labels differ BY DESIGN; everything else must not.
    assert_eq!(DeliveryMode::Wasm.label(), "sandboxed");
    assert_eq!(DeliveryMode::Native.label(), "in-process (unsandboxed)");

    // Tool schema.
    let wasm_schema = wasm.tool_specs().expect("wasm schema");
    let native_schema = native.tool_specs().expect("native schema");
    assert_eq!(wasm_schema, native_schema, "identical tool specs");

    // Tool execution: shared modes with exact-equality results.
    let scenarios: Vec<String> = vec![
        r#"{"mode":"ok"}"#.to_string(),
        r#"{"mode":"fs-read","scope":"workspace","path":"notes.txt"}"#.to_string(),
        r#"{"mode":"fs-read","scope":"workspace","path":"../../etc/passwd"}"#.to_string(),
        r#"{"mode":"fs-read","scope":"private","path":"x"}"#.to_string(),
        r#"{"mode":"fs-list","scope":"workspace","path":"."}"#.to_string(),
        r#"{"mode":"resource-list"}"#.to_string(),
        r#"{"mode":"resource-read","path":"greeting.txt"}"#.to_string(),
        r#"{"mode":"resource-read","path":"skills/note.txt"}"#.to_string(),
        // Hostile cases: traversal and absolute paths must be refused in
        // both modes with the same recorded reason (ADR-0030).
        r#"{"mode":"resource-read","path":"../../etc/passwd"}"#.to_string(),
        r#"{"mode":"resource-read","path":"/etc/passwd"}"#.to_string(),
        r#"{"mode":"resource-read","path":"missing.txt"}"#.to_string(),
        // The `state` bag (ADR-0030): ordered so both modes see the same
        // bag (the two engines share the namespace directory).
        r#"{"mode":"state-list"}"#.to_string(),
        r#"{"mode":"state-write","key":"counter","value":"1"}"#.to_string(),
        r#"{"mode":"state-read","key":"counter"}"#.to_string(),
        r#"{"mode":"state-list"}"#.to_string(),
        r#"{"mode":"state-delete","key":"counter"}"#.to_string(),
        r#"{"mode":"state-read","key":"counter"}"#.to_string(),
        // A key that tries to carry a path is refused and recorded.
        r#"{"mode":"state-write","key":"../escape","value":"x"}"#.to_string(),
        r#"{"mode":"fs-write","scope":"workspace","path":"written.txt","content":"hello"}"#
            .to_string(),
        spawn_args("echo", "diff-marker"),
        pty_args("echo", "pty-marker"),
        // The io modes reuse the same program/args as spawn/pty above, so a
        // platform where one runs is a platform where both run.
        spawn_args("echo", "diff-marker").replace("\"mode\":\"spawn\"", "\"mode\":\"process-io\""),
        pty_args("echo", "pty-marker").replace("\"mode\":\"pty\"", "\"mode\":\"pty-io\""),
    ];
    for scenario in &scenarios {
        let wasm_result = wasm.execute_tool(&call(scenario)).await.expect("wasm");
        let native_result = native.execute_tool(&call(scenario)).await.expect("native");
        // pty-io readiness is timing, not behavior: under load one mode
        // reports `write=false` while the other already accepted the
        // write (macOS CI, 3 hits). Compare those scenarios with the
        // readiness flag normalized out; every other byte must match.
        let normalize = |mut result: lca_protocol::ToolResult| {
            if scenario.contains("\"mode\":\"pty-io\"") {
                result.content = result
                    .content
                    .replace(" write=false", "")
                    .replace(" write=true", "");
            }
            result
        };
        // The pty echo tail races too (`exit 0 ` vs `exit 0 pty-marker`,
        // macOS CI): one retry absorbs it. A second mismatch is a real
        // divergence, not timing — assert it loud.
        if normalize(wasm_result.clone()) != normalize(native_result.clone())
            && scenario.contains("\"mode\":\"pty-io\"")
        {
            let wasm_retry = wasm.execute_tool(&call(scenario)).await.expect("wasm");
            let native_retry = native.execute_tool(&call(scenario)).await.expect("native");
            assert_eq!(
                normalize(wasm_retry),
                normalize(native_retry),
                "divergence in scenario {scenario}"
            );
        } else {
            assert_eq!(
                normalize(wasm_result),
                normalize(native_result),
                "divergence in scenario {scenario}"
            );
        }
    }
    // Sanity: the scenarios actually exercised success and refusal paths.
    let ok = wasm
        .execute_tool(&call(r#"{"mode":"ok"}"#))
        .await
        .expect("ok");
    assert!(ok.content.contains("conformance ok"));
    let refused = wasm
        .execute_tool(&call(
            r#"{"mode":"fs-read","scope":"workspace","path":"../../etc/passwd"}"#,
        ))
        .await
        .expect("runs");
    assert_eq!(refused.status, lca_protocol::ToolResultStatus::Error);
    assert!(
        refused.content.contains("left the scope"),
        "{}",
        refused.content
    );
}

// Commands and hooks: same spec, same effects, same verdicts.
#[tokio::test]
async fn commands_and_hooks_agree_across_modes() {
    let fixture = Fixture::new("diff-hooks");
    let (wasm, native, _engine) = fixture.both_modes();

    assert_eq!(
        wasm.command_specs().expect("wasm"),
        native.command_specs().expect("native")
    );
    for argument in ["submit", "insert:hello", "", "unknown"] {
        assert_eq!(
            wasm.invoke_command("probe", argument).expect("wasm"),
            native.invoke_command("probe", argument).expect("native"),
            "command effect divergence for {argument:?}"
        );
    }
    assert_eq!(
        wasm.invoke_command("probe", "submit").expect("effect"),
        CommandEffect::SubmitPrompt("conformance submitted".to_string())
    );

    for tool in ["read", "probe-deny-tool", "shell"] {
        let call = ToolCall {
            call_id: "c9".to_string(),
            name: tool.to_string(),
            arguments: "{}".to_string(),
            parent_call_id: None,
        };
        let wasm_action = wasm.on_pre_tool_use(&call).await.expect("wasm");
        let native_action = native.on_pre_tool_use(&call).await.expect("native");
        assert_eq!(wasm_action, native_action, "hook divergence for {tool}");
    }
    let denied = wasm
        .on_pre_tool_use(&ToolCall {
            call_id: "c9".to_string(),
            name: "probe-deny-tool".to_string(),
            arguments: "{}".to_string(),
            parent_call_id: None,
        })
        .await
        .expect("hook");
    assert_eq!(
        denied,
        HookAction::Deny("conformance policy denied this tool".to_string())
    );

    // Observing hooks return Ok on both sides.
    let observation = lca_protocol::PostToolObservation {
        call: ToolCall {
            call_id: "c1".to_string(),
            name: "read".to_string(),
            arguments: "{}".to_string(),
            parent_call_id: None,
        },
        result: lca_protocol::ToolResult::ok("c1", "fine"),
    };
    wasm.on_post_tool_use(&observation).await.expect("wasm");
    native.on_post_tool_use(&observation).await.expect("native");
    wasm.on_pre_turn().await.expect("wasm");
    native.on_pre_turn().await.expect("native");
    wasm.on_post_turn_end("ok").await.expect("wasm");
    native.on_post_turn_end("ok").await.expect("native");
    wasm.on_session_close().await.expect("wasm");
    native.on_session_close().await.expect("native");

    assert!(wasm.worlds().contains(&World::Hooks));
}

// Verifies: the Phase 4 conformance cases (SRDD: the new worlds and
// the completion capability get conformance coverage alongside) -
// compact, transform, and rejection agree across delivery modes
// (NFR-25), and the undeclared `completion` capability refuses with a
// recorded denial on BOTH sides (FR-PERM-3).
#[tokio::test]
async fn compaction_and_transform_agree_across_modes_with_completion_denied() {
    let fixture = Fixture::new("diff-ctx");
    let (wasm, native, engine) = fixture.both_modes();

    // Mechanical compaction: identical summaries.
    let records: Vec<lca_protocol::Record> = {
        let ts = 1_700_000_000_000u64;
        vec![
            lca_protocol::Record::User {
                v: lca_protocol::FORMAT_VERSION,
                ts,
                id: "01".to_string(),
                parent: None,
                content: "first request".to_string(),
                attachments: Vec::new(),
                queue: None,
            },
            lca_protocol::Record::Assistant {
                v: lca_protocol::FORMAT_VERSION,
                ts,
                id: "02".to_string(),
                parent: None,
                content: vec![lca_protocol::ContentBlock::Text {
                    text: "reply".to_string(),
                }],
                reasoning: None,
                reasoning_signature: None,
                provider_thinking_level: None,
                usage: None,
                provider: Some("fake".to_string()),
                model: Some("faux-1".to_string()),
            },
        ]
    };
    let wasm_summary = wasm.compact(&records).await.expect("wasm compact");
    let native_summary = native.compact(&records).await.expect("native compact");
    assert_eq!(wasm_summary, native_summary, "identical summaries");
    assert_eq!(wasm_summary, "conformance compacted 2 records");

    // The completion-carrying candidate: undeclared in this manifest,
    // so both modes surface the refusal (FR-PERM-3's completion case)
    // and the host records it.
    let mut completion_records = records.clone();
    completion_records.push(lca_protocol::Record::User {
        v: lca_protocol::FORMAT_VERSION,
        ts: 1_700_000_000_100u64,
        id: "03".to_string(),
        parent: None,
        content: "call-completion".to_string(),
        attachments: Vec::new(),
        queue: None,
    });
    let wasm_err = wasm
        .compact(&completion_records)
        .await
        .expect_err("completion is not declared");
    let native_err = native
        .compact(&completion_records)
        .await
        .expect_err("completion is not declared");
    assert!(
        wasm_err.to_string().contains("completion"),
        "wasm: {wasm_err}"
    );
    assert_eq!(format!("{wasm_err}"), format!("{native_err}"));
    assert!(
        engine
            .denials()
            .iter()
            .any(|denial| denial.capability == "completion"),
        "the denial was recorded (FR-PERM-3)"
    );

    // Transform: pass, inject, and reject - identical in both modes.
    let turns = vec![
        vec![lca_protocol::ChatMessage::text(
            lca_protocol::MessageRole::User,
            "plain turn",
        )],
        vec![lca_protocol::ChatMessage::text(
            lca_protocol::MessageRole::User,
            "please conformance-inject here",
        )],
        vec![lca_protocol::ChatMessage::text(
            lca_protocol::MessageRole::User,
            "conformance-reject this one",
        )],
    ];
    for messages in turns {
        let wasm_out = wasm
            .transform_messages(messages.clone())
            .await
            .expect("wasm transform host call");
        let native_out = native
            .transform_messages(messages.clone())
            .await
            .expect("native transform host call");
        assert_eq!(wasm_out, native_out, "transform divergence");
    }
    // The rejection carries its reason (FR-CTX-3's shape at the ABI).
    let rejected = wasm
        .transform_messages(vec![lca_protocol::ChatMessage::text(
            lca_protocol::MessageRole::User,
            "conformance-reject",
        )])
        .await
        .expect("host call");
    assert_eq!(rejected, Err("conformance transform rejection".to_string()));
    // The injection appended one message; nothing else changed.
    let injected = native
        .transform_messages(vec![lca_protocol::ChatMessage::text(
            lca_protocol::MessageRole::User,
            "conformance-inject",
        )])
        .await
        .expect("host call")
        .expect("not a rejection");
    assert_eq!(injected.len(), 2, "one appended message");
}

// Verifies: ADR-0029 and NFR-25 - a typed image content block crosses the
// ABI byte for byte in both delivery modes. This is the window's first
// breaking change: `message.content` went from a joined string to a list of
// `content-block`s, and the native twin and the WASM component must agree.
#[tokio::test]
async fn image_content_round_trips_identically_across_modes() {
    let fixture = Fixture::new("diff-image");
    let (wasm, native, _) = fixture.both_modes();
    // A tiny PNG-shaped byte string: the point is byte-exact transport, not a
    // decodable image.
    let png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 1, 2, 3, 4];
    let message = lca_protocol::ChatMessage {
        role: lca_protocol::MessageRole::User,
        content: vec![
            lca_protocol::ContentBlock::Text {
                text: "what is in this image?".to_string(),
            },
            lca_protocol::ContentBlock::Image {
                media_type: "image/png".to_string(),
                bytes: png.clone(),
            },
        ],
        tool_calls: Vec::new(),
        tool_call_id: None,
        usage: None,
        extras: Default::default(),
    };
    let wasm_out = wasm
        .transform_messages(vec![message.clone()])
        .await
        .expect("wasm transform")
        .expect("no rejection");
    let native_out = native
        .transform_messages(vec![message.clone()])
        .await
        .expect("native transform")
        .expect("no rejection");
    assert_eq!(wasm_out, native_out, "the image survives identically");
    assert_eq!(
        wasm_out,
        vec![message],
        "the echo transform returns the image block unchanged"
    );
}

// Verifies: the Phase 6 conformance cases - both modes return the same
// tree for every region (NFR-25 over ADR-0003's arena), the hostile
// span crosses byte for byte so the host is the one that must defang
// it (FR-UI-2), and interaction effects agree (FR-UI-6's vocabulary).
#[tokio::test]
async fn ui_regions_and_effects_agree_across_modes() {
    let fixture = Fixture::new("diff-ui");
    let (wasm, native, _engine) = fixture.both_modes();

    assert_eq!(wasm.ui_regions(), native.ui_regions());
    assert_eq!(wasm.ui_regions().len(), 4, "all four regions granted");

    for region in ["status-line", "footer", "panel", "modal"] {
        let wasm_tree = wasm.render(region).expect("wasm render");
        let native_tree = native.render(region).expect("native render");
        assert_eq!(wasm_tree, native_tree, "tree divergence in {region}");
        let tree = wasm_tree.expect("a scripted tree");
        assert!(!tree.is_empty(), "{region} has content");
        if region == "status-line" {
            assert_eq!(tree.nodes.len(), 1, "a single span needs no arena");
        }
    }
    // The hostile fixture crossed intact, escape byte and all: the
    // HOST must defang it on the way out (lca-tui's sanitizer test is
    // the other half of FR-UI-2).
    let footer = wasm.render("footer").expect("footer").expect("tree");
    let hostile = footer
        .nodes
        .iter()
        .find_map(|node| match node {
            lca_protocol::Widget::Text { content, .. } if content.contains("NOT A PROMPT") => {
                Some(content.clone())
            }
            _ => None,
        })
        .expect("the hostile span crossed the boundary");
    assert!(
        hostile.contains('\u{1b}'),
        "the real escape byte survived the ABI: {hostile:?}"
    );

    // The freeze gate's coverage claim: every widget case crosses the
    // boundary in both modes (the footer is the vocabulary page).
    use lca_protocol::Widget as W;
    let footer_nodes = &footer.nodes;
    assert!(
        footer_nodes.iter().any(|w| matches!(w, W::Column(_))),
        "column"
    );
    assert!(footer_nodes.iter().any(|w| matches!(w, W::Row(_))), "row");
    assert!(
        footer_nodes.iter().any(|w| matches!(w, W::Spinner { .. })),
        "spinner"
    );
    assert!(
        footer_nodes.iter().any(|w| matches!(w, W::Progress { .. })),
        "progress"
    );
    assert!(
        footer_nodes.iter().any(|w| matches!(w, W::Image { .. })),
        "image"
    );
    assert!(
        footer_nodes.iter().any(|w| matches!(w, W::Vendor(_))),
        "the reserved vendor case"
    );
    let modal = wasm.render("modal").expect("modal").expect("tree");
    assert!(
        modal.nodes.iter().any(|w| matches!(w, W::Boxed { .. })),
        "boxed"
    );
    // The 0.6 vocabulary (gh #172): every new widget case crosses the
    // boundary in both modes, same as the old ones above.
    assert!(
        footer_nodes
            .iter()
            .any(|w| matches!(w, W::StyledText { .. })),
        "styled-text"
    );
    assert!(
        footer_nodes.iter().any(|w| matches!(w, W::Markdown { .. })),
        "markdown"
    );
    assert!(
        footer_nodes.iter().any(|w| matches!(w, W::Button { .. })),
        "button"
    );
    assert!(
        footer_nodes.iter().any(|w| matches!(w, W::Table { .. })),
        "table"
    );
    assert!(
        footer_nodes
            .iter()
            .any(|w| matches!(w, W::ScrollContainer { .. })),
        "scroll-container"
    );
    let panel = wasm.render("panel").expect("panel").expect("tree");
    assert!(
        panel.nodes.iter().any(|w| matches!(w, W::KeyValue(_))),
        "keyvalue"
    );

    // Events: the modal branch answers, other regions do nothing.
    assert_eq!(
        wasm.on_ui_event("modal", &lca_protocol::UiInput::Key { key: "q".into() })
            .expect("event"),
        native
            .on_ui_event("modal", &lca_protocol::UiInput::Key { key: "q".into() })
            .expect("event")
    );
    assert_eq!(
        wasm.on_ui_event("modal", &lca_protocol::UiInput::Key { key: "q".into() })
            .expect("event"),
        lca_protocol::UiEffect::CloseModal
    );
    assert_eq!(
        wasm.on_ui_event(
            "modal",
            &lca_protocol::UiInput::Submit { text: "hi".into() }
        )
        .expect("event"),
        lca_protocol::UiEffect::ShowNotice("heard: hi".to_string())
    );
    assert_eq!(
        wasm.on_ui_event("panel", &lca_protocol::UiInput::Cancel)
            .expect("event"),
        lca_protocol::UiEffect::None,
        "only the modal region answers"
    );
    // The 0.6 mouse inputs (gh #172): a button click names its widget
    // in both modes, and the relative forms cross intact.
    assert_eq!(
        wasm.on_ui_event(
            "modal",
            &lca_protocol::UiInput::ClickWidget { id: "ok".into() }
        )
        .expect("event"),
        native
            .on_ui_event(
                "modal",
                &lca_protocol::UiInput::ClickWidget { id: "ok".into() }
            )
            .expect("event")
    );
    assert_eq!(
        wasm.on_ui_event(
            "modal",
            &lca_protocol::UiInput::ClickWidget { id: "ok".into() }
        )
        .expect("event"),
        lca_protocol::UiEffect::ShowNotice("clicked: ok".to_string())
    );
    assert_eq!(
        wasm.on_ui_event("modal", &lca_protocol::UiInput::Click { col: 3, row: 1 })
            .expect("event"),
        native
            .on_ui_event("modal", &lca_protocol::UiInput::Click { col: 3, row: 1 })
            .expect("event")
    );
    assert_eq!(
        wasm.on_ui_event("modal", &lca_protocol::UiInput::Scroll { delta: -1 })
            .expect("event"),
        lca_protocol::UiEffect::None
    );
}

// Verifies: ADR-0033 (the provider login surface agrees across delivery
// modes: options round-trip and a submit returns the same opaque settings).
#[tokio::test]
async fn the_login_surface_agrees_across_modes() {
    let fixture = Fixture::new("login");
    let (wasm, native, _engine) = fixture.both_modes();

    let wasm_options = wasm.login_options().await.expect("wasm options");
    let native_options = native.login_options().await.expect("native options");
    assert_eq!(wasm_options, native_options, "identical login options");
    assert_eq!(wasm_options.len(), 2, "the probe reports two options");
    assert_eq!(wasm_options[0].kind, "api-key");

    let answer = lca_protocol::LoginAnswer {
        choice: "conformance".to_string(),
        values: [("api-key".to_string(), "secret".to_string())]
            .into_iter()
            .collect(),
    };
    let wasm_settings = wasm
        .login_submit(answer.clone())
        .await
        .expect("wasm submit");
    let native_settings = native.login_submit(answer).await.expect("native submit");
    assert_eq!(wasm_settings, native_settings, "identical settings");
    assert!(
        wasm_settings.iter().any(|(key, _)| key == "base_url"),
        "the settings the host persists came back"
    );
}

// Verifies: gh #124 - a conformance tool asking `ui.confirm` hears the
// same verdict in both modes: the WASM twin crosses the real host
// import, the native twin answers through its injected prompter, and
// the Fixture scripts both sides alike.
#[test]
fn the_dialog_verdict_agrees_across_modes() {
    use lca_permissions::{DialogPrompt, SharedDialogs};

    struct Scripted(bool);
    impl DialogPrompt for Scripted {
        fn confirm(&mut self, _title: &str, _message: &str) -> bool {
            self.0
        }
        fn select(&mut self, _title: &str, _options: &[String]) -> Option<String> {
            None
        }
        fn input(&mut self, _label: &str, _placeholder: Option<&str>) -> Option<String> {
            None
        }
        fn notify(&mut self, _message: &str, _level: &str) {}
    }

    fn asking(handle: &Arc<dyn ExtensionDispatch>) -> String {
        // Tools run off the loop thread (gh #124), so no guard is set
        // here - exactly like the turn worker calling in.
        let call = ToolCall {
            call_id: "dialog-1".to_string(),
            name: "conformance".to_string(),
            arguments: "{\"mode\":\"ask-confirm\"}".to_string(),
            parent_call_id: None,
        };
        let handle = handle.clone();
        std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime")
                .block_on(handle.execute_tool(&call))
        })
        .join()
        .expect("tool thread")
        .expect("tool ran")
        .content
    }

    fn ask(wasm: &Arc<dyn ExtensionDispatch>, native: &Arc<dyn ExtensionDispatch>, verdict: &str) {
        let wasm_text = asking(wasm);
        let native_text = asking(native);
        assert_eq!(wasm_text, native_text, "the verdict agrees");
        assert!(
            wasm_text.contains(verdict),
            "the scripted verdict crossed: {wasm_text}"
        );
    }

    for verdict in [false, true] {
        let fixture = Fixture::new(&format!("dialog-{verdict}"));
        let scripted = Arc::new(Mutex::new(Scripted(verdict)));
        let dialogs = SharedDialogs::default();
        dialogs.set(scripted);

        let mut host = ExtHost::new(
            ExtensionLimits {
                memory_bytes: 64 * 1024 * 1024,
                fuel_per_call: 100_000_000,
                log_limit_bytes: 4096,
            },
            Arc::new(HostEnvironment {
                roots: fixture.roots.clone(),
                prompt: fixture.prompt.clone(),
                dialogs: dialogs.clone(),
                grant_store: fixture.store.clone(),
                project: fixture.root.join("project"),
                proposals: None,
                // (kept alphabetical like the constructor)
            }),
        );
        let wasm: Arc<dyn ExtensionDispatch> =
            Arc::new(host.load(FIXTURE, MANIFEST).expect("load wasm mode"));
        let engine = fixture.capabilities();
        let native: Arc<dyn ExtensionDispatch> =
            Arc::new(conformance::NativeConformance::new(engine).with_dialogs(dialogs));
        ask(&wasm, &native, &format!("confirm: {verdict}"));
    }
}

// Verifies: gh #77 (EFG-035's three tools) - the suite registers
// three tools with their exposures and namespaces in both modes,
// and all three dispatch through the same shared modes.
#[tokio::test]
async fn the_catalog_registers_three_tools_in_both_modes() {
    let fixture = Fixture::new("catalog");
    let (wasm, native, _engine) = fixture.both_modes();

    let wasm_specs = wasm.tool_specs().expect("wasm catalog");
    let native_specs = native.tool_specs().expect("native catalog");
    assert_eq!(wasm_specs, native_specs, "identical catalog specs");
    // Handle-level specs keep catalog order; the registry sorts.
    let names: Vec<&str> = wasm_specs.iter().map(|spec| spec.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["conformance", "conformance-deferred", "conformance-code"],
        "the three suite tools, in catalog order"
    );
    let exposures: Vec<&str> = wasm_specs
        .iter()
        .map(|spec| spec.exposure.as_str())
        .collect();
    assert_eq!(exposures, vec!["direct", "deferred", "codemode"]);
    // The legacy tool stays bare; the discovery pair shares one
    // namespace with its instructions.
    assert!(wasm_specs[0].namespace.is_none());
    for spec in &wasm_specs[1..] {
        let namespace = spec.namespace.as_ref().expect("namespaced");
        assert_eq!(namespace.name, "demo");
    }
    assert!(
        wasm_specs[2]
            .namespace
            .as_ref()
            .expect("ns")
            .instructions
            .is_none()
    );
    assert!(
        wasm_specs[1]
            .namespace
            .as_ref()
            .expect("ns")
            .instructions
            .is_some()
    );

    // Every suite tool dispatches, in both modes.
    for name in names {
        let arguments = r#"{"mode":"ok"}"#.to_string();
        let wasm_result = wasm
            .execute_tool(&ToolCall {
                call_id: "c1".to_string(),
                name: name.to_string(),
                arguments: arguments.clone(),
                parent_call_id: None,
            })
            .await
            .expect("wasm suite dispatch");
        let native_result = native
            .execute_tool(&ToolCall {
                call_id: "c1".to_string(),
                name: name.to_string(),
                arguments,
                parent_call_id: None,
            })
            .await
            .expect("native suite dispatch");
        assert_eq!(
            wasm_result.content, "conformance ok",
            "{name} dispatches in wasm mode"
        );
        assert_eq!(
            native_result.content, "conformance ok",
            "{name} dispatches in native mode"
        );
    }
}

// Verifies: gh #45 - every new hook point answers identically in both
// modes: marker-gated replace/mutate/compose plus inert defaults.
#[tokio::test]
async fn the_new_hooks_agree_across_modes() {
    let fixture = Fixture::new("hooks-new");
    let (wasm, native, _engine) = fixture.both_modes();

    // message_end replaces the marked token, observes the rest.
    for text in [
        "the token is conformance-secret, handle with care",
        "nothing to redact here",
    ] {
        assert_eq!(
            wasm.on_message_end("assistant", text).await.expect("wasm"),
            native
                .on_message_end("assistant", text)
                .await
                .expect("native"),
            "message_end agrees on {text:?}"
        );
    }
    assert_eq!(
        wasm.on_message_end(
            "assistant",
            "the token is conformance-secret, handle with care"
        )
        .await
        .expect("wasm"),
        Some("the token is conformance-redacted, handle with care".to_string())
    );

    // tool_call mutates the marked args, blocks the marked call.
    let plain = ToolCall {
        call_id: "c1".to_string(),
        name: "conformance".to_string(),
        arguments: r#"{"mode":"ok"}"#.to_string(),
        parent_call_id: None,
    };
    assert_eq!(
        wasm.on_tool_call(&plain).await.expect("wasm"),
        native.on_tool_call(&plain).await.expect("native"),
    );
    let mutate = ToolCall {
        arguments: r#"{"mode":"ok","tag":"tag-mutate"}"#.to_string(),
        ..plain.clone()
    };
    let wasm_patch = wasm.on_tool_call(&mutate).await.expect("wasm patch");
    assert_eq!(
        wasm_patch,
        native.on_tool_call(&mutate).await.expect("native patch"),
    );
    assert!(
        wasm_patch
            .arguments
            .expect("mutated")
            .contains("tag-mutated")
    );
    assert!(wasm_patch.block.is_none());
    let block = ToolCall {
        arguments: r#"{"tag":"tag-block"}"#.to_string(),
        ..plain.clone()
    };
    let wasm_block = wasm.on_tool_call(&block).await.expect("wasm block");
    assert_eq!(
        wasm_block,
        native.on_tool_call(&block).await.expect("native block")
    );
    assert_eq!(
        wasm_block.block.as_deref(),
        Some("conformance blocked this call")
    );

    // tool_result composes the marked content, observes the rest.
    let result = lca_protocol::ToolResult::ok("c1", "the token is conformance-secret!");
    let wasm_composed = wasm.on_tool_result(&plain, &result).await.expect("wasm");
    assert_eq!(
        wasm_composed,
        native
            .on_tool_result(&plain, &result)
            .await
            .expect("native"),
    );
    assert_eq!(
        wasm_composed.content.as_deref(),
        Some("the token is conformance-redacted!")
    );

    // Settle, compact-allow, and stream-observe are inert by default.
    for (wasm_d, native_d) in [
        (
            wasm.on_turn_end(2, 1, "ok").await.expect("wasm"),
            native.on_turn_end(2, 1, "ok").await.expect("native"),
        ),
        (
            wasm.on_agent_before_settle(2, 1, "ok").await.expect("wasm"),
            native
                .on_agent_before_settle(2, 1, "ok")
                .await
                .expect("native"),
        ),
    ] {
        assert_eq!(wasm_d, native_d);
        assert_eq!(wasm_d, lca_protocol::SettleDecision::default());
    }
    assert_eq!(
        wasm.on_session_before_compact("threshold")
            .await
            .expect("wasm"),
        native
            .on_session_before_compact("threshold")
            .await
            .expect("native"),
    );
    wasm.on_session_compact_failed("threshold", Some("boom"))
        .await
        .expect("wasm failed-hook");
    native
        .on_session_compact_failed("threshold", Some("boom"))
        .await
        .expect("native failed-hook");
    wasm.on_stream_event("p", "m", "text-delta", "hi")
        .await
        .expect("wasm stream");
    native
        .on_stream_event("p", "m", "text-delta", "hi")
        .await
        .expect("native stream");

    // Cache votes decline the marked model; trust votes decide the
    // marked paths and fall through everywhere else.
    for model in ["m", "model-no-warm"] {
        assert_eq!(
            wasm.on_cache_warming_decision("p", model)
                .await
                .expect("wasm"),
            native
                .on_cache_warming_decision("p", model)
                .await
                .expect("native"),
            "cache agrees on {model}"
        );
    }
    assert!(
        !(wasm
            .on_cache_warming_decision("p", "model-no-warm")
            .await
            .expect("wasm"))
    );
    for cwd in ["/tmp/work", "/tmp/trust-yes", "/tmp/trust-no"] {
        assert_eq!(
            wasm.on_project_trust(cwd).await.expect("wasm"),
            native.on_project_trust(cwd).await.expect("native"),
            "trust agrees on {cwd}"
        );
    }
    assert_eq!(
        wasm.on_project_trust("/tmp/trust-yes").await.expect("wasm"),
        (lca_protocol::TrustVote::Yes, true)
    );
}
