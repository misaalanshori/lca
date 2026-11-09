//! The Phase 2 exit-test centerpiece: the conformance extension run in
//! both delivery modes produces identical results (NFR-25, the dual-mode
//! interchangeability promise from ADR-0013 and the SRDD).
//!
//! Both sides share the same roots, prompt, and grant store; the WASM
//! side reaches them through host imports, the native side calls the
//! same engine directly. Any divergence is a defect regardless of which
//! side "wins" (docs/testing-plan.md section6).
//!
//! Gh #552 split this file at the world groups it already had, because it was
//! 20 lines under gate 11's ceiling and every new conformance row was blocked.
//! The rows about a *world* moved to `hooks.rs`, `ui.rs`, and
//! `provider_surface.rs`; what stays here is the **harness** (`Fixture`,
//! `both_modes`, `both_modes_keeping_wasm`) plus the rows that cross worlds:
//! the tool/scenario matrix, compaction and transform, and the catalog.
//! The harness stays in one place on purpose - it is what the siblings call
//! through `super`, and duplicating it would pay another ~6 s
//! `Component::new` per row (`tests/regressions/449`).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use std::sync::{Arc, Mutex};

use lca_ext_abi::{DeliveryMode, ExtensionDispatch, World};
use lca_ext_host::{ExtHost, ExtensionLimits, HostEnvironment};
use lca_ext_native::NativeRegistry;
use lca_permissions::{
    Decision, GrantStore, PermissionPrompt, ProposalDiff, ScopeGrant, ScopeRoots,
};
use lca_protocol::ToolCall;

#[path = "conformance_diff/args.rs"]
mod args;
// Gh #552: the world-group rows, split off at the subjects this file already had.
#[path = "conformance_diff/hooks.rs"]
mod hooks;
#[path = "conformance_diff/provider_surface.rs"]
mod provider_surface;
#[path = "conformance_diff/serve_http.rs"]
mod serve_http;
#[path = "conformance_diff/ui.rs"]
mod ui;
use args::{call, env_get_args, net_fetch_args, pty_args, spawn_args};

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

/// The two delivery modes of one fixture, plus the engine the native twin
/// ran on (so its recorded denials stay visible to the row).
type Modes = (
    Arc<dyn ExtensionDispatch>,
    Arc<dyn ExtensionDispatch>,
    Arc<lca_tools::Capabilities>,
);

/// [`Modes`] plus the concrete WASM handle (gh #449: a row wanting the
/// guest's single-tool schema reuses this compile instead of a new one).
type ModesWithGuest = (
    Arc<lca_ext_host::WasmExtension>,
    Arc<dyn ExtensionDispatch>,
    Arc<dyn ExtensionDispatch>,
    Arc<lca_tools::Capabilities>,
);

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
                // gh #302: the manifest grants exactly this host, so the
                // native fixture must too - otherwise the two modes differ
                // for a reason that is the *fixture*, not the product, and
                // the net rows would compare "not granted" against a real
                // attempt. Both sides must start from the same grant.
                net_declared: true,
                net: vec![
                    lca_permissions::parse_net_pattern("conformance.example.com")
                        .expect("the manifest's own host parses"),
                ],
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

    fn both_modes(&self) -> Modes {
        let (_concrete, wasm, native, engine) = self.both_modes_keeping_wasm();
        (wasm, native, engine)
    }

    /// [`Fixture::both_modes`] plus the concrete WASM handle.
    ///
    /// gh #449: a row that also asks the guest for its single-tool schema
    /// used to `host.load` a second time for it, paying `Component::new`
    /// again (~6 s in a test build). `schema()` is just another call into
    /// the same component, so this is the handle to ask.
    fn both_modes_keeping_wasm(&self) -> ModesWithGuest {
        let mut host = ExtHost::new(
            ExtensionLimits {
                memory_bytes: 64 * 1024 * 1024,
                fuel_per_call: 100_000_000,
                log_limit_bytes: 4096,
            },
            self.env(),
        )
        // Gh #170: the env probe is set on the real process environment,
        // because that is exactly what the comparison is about - the native
        // twin reads `std::env::var` directly (as a compiled-in extension
        // does), while the WASM twin reads whatever the host copied into its
        // `WasiCtx`. Injecting a map would have hidden the divergence rather
        // than proving it closed. The values are fixed, non-secret markers.
        ;
        let wasm: Arc<lca_ext_host::WasmExtension> =
            Arc::new(host.load(FIXTURE, MANIFEST).expect("load wasm mode"));
        let engine = self.capabilities();
        // The same registry type holds both handle kinds (FR-EXT-6: one
        // handle type, no mode branching).
        let mut registry = NativeRegistry::new();
        registry.register(wasm.clone());
        registry.register(Arc::new(conformance::NativeConformance::new(
            engine.clone(),
        )));
        let handles = registry.into_handles();
        // The SAME engine the native handle ran on, so its recorded
        // denials are visible to the test (FR-PERM-3).
        (wasm, handles[0].clone(), handles[1].clone(), engine)
    }
}

// Verifies: NFR-25's substance, FR-EXT-7 (the two delivery labels
// differ by design), and the SRDD Phase 2 exit test - the conformance
// extension passes in native mode and WASM mode with identical
// results.
#[tokio::test]
async fn native_and_wasm_modes_produce_identical_results() {
    // Gh #170: `set_var` is `unsafe` (concurrent-reader race); these names are
    // unique to this row and hold fixed non-secret markers.
    unsafe {
        std::env::set_var("LCA_CONFORMANCE_ENV", "probe-value");
        std::env::set_var("LCA_CONFORMANCE_UNDECLARED", "must-not-be-visible");
        std::env::remove_var("LCA_CONFORMANCE_UNSET");
    }
    let fixture = Fixture::new("diff");
    let (wasm, native, _engine) = fixture.both_modes();

    // Identity and shape.
    assert_eq!(wasm.name(), native.name());
    assert_eq!(wasm.worlds(), native.worlds());
    // The count is a guard against a world silently dropping out of the
    // fixture's manifest: the list is asserted, not just the number, so a
    // dropped world and an added one cannot cancel each other out.
    assert_eq!(
        wasm.worlds().len(),
        17,
        "tool, tool-catalog, command, hooks, the eight hooks-*, provider, compaction, context-transform, ui, serve-http"
    );
    // Gh #363: the `serve-http` world is the newest, and this is the row that
    // notices if it ever stops being declared.
    assert!(
        wasm.worlds().contains(&World::ServeHttp),
        "the fixture must exercise every world, including serve-http: {:?}",
        wasm.worlds()
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
        // gh #302: the `tool` world now imports `net`, so the matrix has to
        // cover it in both modes. Three hosts, because the interesting part
        // is not that a granted host works - it is that the *refusals* are
        // identical. The granted one is the manifest's own host (the fixture
        // grants exactly `conformance.example.com`); the other two are
        // ungranted and must fail the same way in both modes.
        net_fetch_args("https://conformance.example.com/probe"), // granted
        net_fetch_args("https://ungranted.example.com/probe"),   // denied
        net_fetch_args("https://conformance.example.com:8443/x"), // port not allowed
        // gh #170: declared+set, declared+unset (absent, never empty), and
        // set-but-undeclared (absent - the negative that makes this a grant).
        env_get_args("LCA_CONFORMANCE_ENV"),
        env_get_args("LCA_CONFORMANCE_UNSET"),
        env_get_args("LCA_CONFORMANCE_UNDECLARED"),
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
        //
        // It is not specific to the io twins: the plain `pty` scenario runs the
        // same program through the same terminal transport, so it loses the
        // same trailing bytes under load. Gating the retry on `pty-io` alone
        // left `mode:pty` to fail on the first race, which is what happened on
        // macOS CI. Both pty flavours get the retry; `spawn` has no terminal in
        // the path and keeps the strict single comparison.
        let pty_scenario =
            scenario.contains("\"mode\":\"pty\"") || scenario.contains("\"mode\":\"pty-io\"");
        if normalize(wasm_result.clone()) != normalize(native_result.clone()) && pty_scenario {
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
