//! Dispatch-table tests: name namespaces, reserved built-ins, and
//! collision handling (FR-EXT-11), with both delivery modes registered
//! through the one trait (FR-EXT-6).

use std::sync::Arc;

use lca_core::ExtensionRegistry;
use lca_ext_abi::{DeliveryMode, ExtensionDispatch, World};
use lca_protocol::{CommandEffect, CommandSpec, DispatchError, HookAction, ToolCall, ToolSpec};

struct FakeExt {
    name: String,
    delivery: DeliveryMode,
    worlds: Vec<World>,
    tools: Vec<ToolSpec>,
    commands: Vec<CommandSpec>,
    slots: Vec<String>,
    patch: Option<lca_protocol::ToolCallPatch>,
}

impl FakeExt {
    fn new(name: &str) -> FakeExt {
        FakeExt {
            name: name.to_string(),
            delivery: DeliveryMode::Wasm,
            worlds: Vec::new(),
            tools: Vec::new(),
            commands: Vec::new(),
            slots: Vec::new(),
            patch: None,
        }
    }

    /// Answer every `tool_call` hook with this patch (gh #45's chain).
    fn with_patch(mut self, patch: lca_protocol::ToolCallPatch) -> FakeExt {
        self.worlds.push(World::HooksToolCall);
        self.patch = Some(patch);
        self
    }

    fn with_tool(mut self, tool: &str) -> FakeExt {
        self.worlds.push(World::Tool);
        self.tools.push(ToolSpec {
            name: tool.to_string(),
            description: "test tool".to_string(),
            parameters: serde_json::json!({"type": "object"}),
            exposure: ToolExposure::Direct,
            namespace: None,
            annotations: None,
            extras: Default::default(),
        });
        self
    }

    fn with_command(mut self, leaf: &str) -> FakeExt {
        self.worlds.push(World::Command);
        self.commands.push(CommandSpec {
            name: leaf.to_string(),
            hint: String::new(),
            completion: "none".to_string(),
            extras: Default::default(),
        });
        self
    }

    fn native(mut self) -> FakeExt {
        self.delivery = DeliveryMode::Native;
        self
    }

    fn claiming(mut self, slot: &str) -> FakeExt {
        self.slots.push(slot.to_string());
        self
    }
}

impl ExtensionDispatch for FakeExt {
    fn name(&self) -> &str {
        &self.name
    }
    fn delivery(&self) -> DeliveryMode {
        self.delivery
    }
    fn worlds(&self) -> Vec<World> {
        self.worlds.clone()
    }
    fn tool_specs(&self) -> Result<Vec<ToolSpec>, DispatchError> {
        Ok(self.tools.clone())
    }
    fn execute_tool<'a>(
        &'a self,
        _call: &'a ToolCall,
    ) -> lca_ext_abi::DispatchFuture<'a, Result<lca_protocol::ToolResult, DispatchError>> {
        Box::pin(std::future::ready(Err(DispatchError::MissingWorld {
            extension: self.name.clone(),
            world: "tool",
        })))
    }

    fn on_pre_turn(&self) -> lca_ext_abi::DispatchFuture<'static, Result<(), DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }

    fn on_pre_tool_use<'a>(
        &'a self,
        _call: &'a ToolCall,
    ) -> lca_ext_abi::DispatchFuture<'a, Result<HookAction, DispatchError>> {
        Box::pin(std::future::ready(Ok(HookAction::Allow)))
    }
    fn on_tool_call<'a>(
        &'a self,
        _call: &'a ToolCall,
    ) -> lca_ext_abi::DispatchFuture<'a, Result<lca_protocol::ToolCallPatch, DispatchError>> {
        Box::pin(std::future::ready(Ok(self
            .patch
            .clone()
            .unwrap_or_default())))
    }

    fn on_post_tool_use<'a>(
        &'a self,
        _observation: &'a lca_protocol::PostToolObservation,
    ) -> lca_ext_abi::DispatchFuture<'a, Result<(), DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }

    fn on_post_turn_end<'a>(
        &'a self,
        _status: &'a str,
    ) -> lca_ext_abi::DispatchFuture<'a, Result<(), DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }

    fn on_attention_required<'a>(
        &'a self,
        _reason: &'a str,
    ) -> lca_ext_abi::DispatchFuture<'a, Result<(), DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }

    fn on_session_close(&self) -> lca_ext_abi::DispatchFuture<'static, Result<(), DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }
    fn command_specs(&self) -> Result<Vec<CommandSpec>, DispatchError> {
        Ok(self.commands.clone())
    }
    fn invoke_command(&self, name: &str, _argument: &str) -> Result<CommandEffect, DispatchError> {
        Ok(CommandEffect::ShowWidget(format!("{}:{name}", self.name)))
    }
    fn builtin_command_slots(&self) -> Vec<String> {
        self.slots.clone()
    }
}

fn spec_name(tool: &str) -> ToolSpec {
    ToolSpec {
        name: tool.to_string(),
        description: String::new(),
        parameters: serde_json::json!({}),
        exposure: ToolExposure::Direct,
        namespace: None,
        annotations: None,
        extras: Default::default(),
    }
}

// Verifies: FR-EXT-11 (an extension registering a built-in tool name
// loses the name; the built-in registration stays).
#[test]
fn builtin_tool_names_are_reserved() {
    let mut registry = ExtensionRegistry::new();
    registry.register(Arc::new(FakeExt::new("greedy").with_tool("read")));
    assert!(
        registry.tool_owner("read").is_none(),
        "built-in keeps the name"
    );
    let collisions = registry.collisions();
    assert_eq!(collisions.len(), 1);
    assert_eq!(collisions[0].kind, "tool");
    assert_eq!(collisions[0].winner, "built-in");
    assert_eq!(collisions[0].extension, "greedy");
    assert!(
        registry.enabled().count() == 1,
        "the extension keeps its other worlds"
    );
}

// Verifies: FR-EXT-11 + gh #119 (a pi-name alias is reserved like its
// canonical tool): an extension registering `find` loses to the host's
// `glob` alias the same way registering `read` loses.
#[test]
fn pi_name_aliases_are_reserved_like_builtin_tools() {
    let mut registry = ExtensionRegistry::new();
    registry.register(Arc::new(FakeExt::new("greedy").with_tool("find")));
    assert!(
        registry.tool_owner("find").is_none(),
        "the host alias keeps the name"
    );
    let collisions = registry.collisions();
    assert_eq!(collisions.len(), 1);
    assert_eq!(collisions[0].kind, "tool");
    assert_eq!(collisions[0].winner, "built-in");
    assert_eq!(collisions[0].extension, "greedy");
}

// Verifies: FR-EXT-11 (of two extensions, the earlier registration wins).
#[test]
fn the_earlier_tool_registration_wins() {
    let mut registry = ExtensionRegistry::new();
    registry.register(Arc::new(FakeExt::new("first").with_tool("widget")));
    registry.register(Arc::new(FakeExt::new("second").with_tool("widget")));

    let owner = registry.tool_owner("widget").expect("registered").clone();
    assert_eq!(owner.name(), "first");
    assert_eq!(registry.tool_specs().len(), 1, "one surviving spec");
    assert_eq!(registry.collisions().len(), 1);
    assert_eq!(registry.collisions()[0].winner, "first");
}

// Verifies: FR-EXT-11's extension identity rule: a duplicate handle is
// disabled for the session and reported.
#[test]
fn duplicate_extension_identities_disable_the_later_one() {
    let mut registry = ExtensionRegistry::new();
    registry.register(Arc::new(FakeExt::new("twin").with_command("probe")));
    registry.register(Arc::new(FakeExt::new("twin").with_command("probe")));

    let collisions = registry.collisions();
    assert_eq!(
        collisions.len(),
        1,
        "the duplicate stops at the identity check: {collisions:?}"
    );
    assert_eq!(collisions[0].kind, "extension");
    assert_eq!(collisions[0].winner, "earlier registration");
    // The second handle contributed nothing: only one command entry.
    assert_eq!(registry.command_names().len(), 1);
}

// Extension commands namespace as `<extension>.<command>` (ADR-0012's
// mechanism per the SRDD), so they cannot shadow a built-in slash name.
#[test]
fn extension_commands_are_namespaced() {
    let mut registry = ExtensionRegistry::new();
    registry.register(Arc::new(FakeExt::new("foo").with_command("stats")));
    assert_eq!(registry.command_names(), vec!["foo.stats".to_string()]);
    assert_eq!(
        registry.invoke_command("foo.stats", ""),
        Some(CommandEffect::ShowWidget("foo:stats".to_string()))
    );
    assert_eq!(registry.invoke_command("stats", ""), None, "no bare shadow");
}

// A first-party native handle fills a reserved built-in slot, which is
// how /stats moved onto the extension path (ADR-0019, the Phase 2 move).
#[test]
fn a_native_handle_fills_a_reserved_builtin_slot() {
    let mut registry = ExtensionRegistry::new();
    registry.register(Arc::new(
        FakeExt::new("stats-native")
            .with_command("stats")
            .native()
            .claiming("stats"),
    ));
    assert_eq!(registry.command_names(), vec!["stats".to_string()]);
    assert_eq!(
        registry.invoke_command("stats", ""),
        Some(CommandEffect::ShowWidget("stats-native:stats".to_string()))
    );

    // A second claim of the same slot loses to the earlier one
    // (FR-EXT-11).
    registry.register(Arc::new(
        FakeExt::new("stats-two")
            .with_command("stats")
            .native()
            .claiming("stats"),
    ));
    let collisions = registry.collisions();
    assert_eq!(collisions.len(), 1, "{collisions:?}");
    assert_eq!(collisions[0].kind, "command");
    assert_eq!(collisions[0].winner, "stats-native");
    assert_eq!(
        registry.invoke_command("stats", ""),
        Some(CommandEffect::ShowWidget("stats-native:stats".to_string())),
        "the winner keeps the slot"
    );
}

// Sandboxed handles may not claim slots even if they ask (the trait
// default is not reachable from a manifest); their commands always
// namespace (FR-EXT-7's sibling rule, deny by default).
#[test]
fn wasm_handles_never_claim_builtin_slots() {
    let mut registry = ExtensionRegistry::new();
    registry.register(Arc::new(
        FakeExt::new("sandboxed")
            .with_command("stats")
            .claiming("stats"),
    ));
    assert_eq!(
        registry.command_names(),
        vec!["sandboxed.stats".to_string()]
    );
    assert!(
        registry.collisions().is_empty(),
        "namespacing needs no collision"
    );
}

// Verifies: FR-EXT-6 (both delivery modes register through the same
// trait and table — a native first-party handle sits alongside a
// sandboxed one).
#[test]
fn both_delivery_modes_share_one_table() {
    use lca_ext_abi::DeliveryMode as Mode;
    assert_eq!(Mode::Wasm.label(), "sandboxed");
    assert_eq!(Mode::Native.label(), "in-process (unsandboxed)");

    let mut registry = ExtensionRegistry::new();
    registry.register(Arc::new(FakeExt::new("sandboxed").with_command("probe")));
    registry.register(Arc::new(
        FakeExt::new("native").with_command("probe").native(),
    ));
    // A native handle without a command world contributes nothing but
    // still registers through the identical path.
    assert_eq!(
        registry.command_names(),
        vec!["native.probe".to_string(), "sandboxed.probe".to_string()],
        "both namespace through the same table"
    );
    let native: Arc<dyn ExtensionDispatch> = Arc::new(FakeExt::new("native-two").native());
    assert_eq!(native.delivery(), DeliveryMode::Native);
    assert_eq!(native.name(), "native-two");
}

// The real native conformance handle registers through the same table
// (FR-EXT-6 with the Phase 2 fixture, not just a test double).
#[test]
fn the_native_conformance_handle_registers_through_the_table() {
    let store_root = lca_testkit::scratch_path("lca-reg-conf");
    let _ = std::fs::remove_dir_all(&store_root);
    std::fs::create_dir_all(store_root.join("workspace")).expect("mkdir");
    let roots = lca_permissions::ScopeRoots {
        workspace: store_root.join("workspace"),
        private: store_root.join("private"),
        home_config: store_root.join("config"),
        temp: store_root.join("tmp"),
        state_dir: store_root.join("data"),
    };
    struct Allow;
    impl lca_permissions::PermissionPrompt for Allow {
        fn ask(&mut self, _: &lca_permissions::Action) -> lca_permissions::Decision {
            lca_permissions::Decision::Always
        }
        fn review_proposals(&mut self, _: &lca_permissions::ProposalDiff) -> bool {
            false
        }
    }
    let caps = Arc::new(lca_tools::Capabilities::new(
        "conformance",
        lca_tools::CapabilityGrants {
            fs: vec![
                lca_permissions::ScopeGrant::parse("workspace", lca_permissions::FsMode::ReadWrite)
                    .expect("grant"),
            ],
            fs_declared: true,
            process: true,
            pty: true,
            ..Default::default()
        },
        roots,
        Arc::new(std::sync::Mutex::new(Allow)),
        Arc::new(std::sync::Mutex::new(
            lca_permissions::GrantStore::open(&store_root.join("grants.json")).expect("store"),
        )),
        store_root.join("workspace"),
        None,
    ));
    let handle: Arc<dyn ExtensionDispatch> = Arc::new(conformance::NativeConformance::new(caps));
    let mut registry = ExtensionRegistry::new();
    registry.register(handle);
    // The command world's own spec plus ADR-0012's auto-namespaced
    // identity trio (FR-PROV-10), since conformance is a provider.
    assert_eq!(
        registry.command_names(),
        vec![
            "conformance.login".to_string(),
            "conformance.logout".to_string(),
            "conformance.probe".to_string(),
            "conformance.usage".to_string(),
        ]
    );
    assert_eq!(
        registry.invoke_command("conformance.probe", "submit"),
        Some(CommandEffect::SubmitPrompt(
            "conformance submitted".to_string()
        ))
    );
    // Gh #77's suite: three tools through the table, sorted.
    assert_eq!(registry.tool_specs().len(), 3);
    let _ = spec_name("unused");
}

// Verifies: the tool table handed to the provider is deterministic (sorted by
// name), so prompt-cache bytes and the assembly snapshot do not depend on a
// HashMap's iteration order.
#[test]
fn tool_specs_are_sorted_for_a_stable_request() {
    let mut registry = ExtensionRegistry::new();
    registry.register(Arc::new(FakeExt::new("zeta").with_tool("zebra")));
    registry.register(Arc::new(FakeExt::new("alpha").with_tool("aardvark")));
    let names: Vec<String> = registry
        .tool_specs()
        .into_iter()
        .map(|spec| spec.name)
        .collect();
    assert_eq!(names, vec!["aardvark".to_string(), "zebra".to_string()]);
}

// ---------------------------------------------------------------------------
// Gh #77: exposure, namespaces, and dynamic activation.
// ---------------------------------------------------------------------------

use lca_protocol::{ToolAnnotations, ToolExposure, ToolNamespace};

impl FakeExt {
    fn with_suite_tool(
        mut self,
        tool: &str,
        exposure: ToolExposure,
        namespace: Option<ToolNamespace>,
    ) -> FakeExt {
        if !self.worlds.contains(&World::Tool) {
            self.worlds.push(World::Tool);
        }
        self.tools.push(ToolSpec {
            name: tool.to_string(),
            description: format!("{tool} tool"),
            parameters: serde_json::json!({"type": "object"}),
            exposure,
            namespace,
            annotations: None,
            extras: Default::default(),
        });
        self
    }
}

impl FakeExt {
    fn with_annotated_tool(mut self, tool: &str) -> FakeExt {
        if !self.worlds.contains(&World::Tool) {
            self.worlds.push(World::Tool);
        }
        self.tools.push(ToolSpec {
            name: tool.to_string(),
            description: format!("{tool} tool"),
            parameters: serde_json::json!({"type": "object"}),
            exposure: ToolExposure::Direct,
            namespace: None,
            annotations: Some(ToolAnnotations {
                read_only_hint: Some(true),
                destructive_hint: Some(false),
                idempotent_hint: None,
                open_world_hint: None,
            }),
            extras: Default::default(),
        });
        self
    }
}

fn namespaced(name: &str) -> Option<ToolNamespace> {
    Some(ToolNamespace {
        name: name.to_string(),
        description: format!("{name} things"),
        instructions: None,
    })
}

// Verifies: gh #77 - only `direct` tools are declared to the model;
// hidden and model-only tools never are.
#[test]
fn only_direct_tools_are_declared_to_the_model() {
    let mut registry = ExtensionRegistry::new();
    registry.register(Arc::new(
        FakeExt::new("suite")
            .with_suite_tool("seen", ToolExposure::Direct, None)
            .with_suite_tool("quiet", ToolExposure::ModelOnly, None)
            .with_suite_tool("lazy", ToolExposure::Deferred, None)
            .with_suite_tool("gone", ToolExposure::Hidden, None),
    ));
    let declared: Vec<String> = registry
        .declared_tool_specs()
        .iter()
        .map(|spec| spec.name.clone())
        .collect();
    assert_eq!(declared, vec!["seen".to_string()]);
}

// Verifies: gh #77 - namespace grouping rows name the namespace and
// its tools.
#[test]
fn namespaces_group_their_tools() {
    let mut registry = ExtensionRegistry::new();
    registry.register(Arc::new(
        FakeExt::new("suite")
            .with_suite_tool("gh_open", ToolExposure::Direct, namespaced("github"))
            .with_suite_tool("gh_close", ToolExposure::Direct, namespaced("github"))
            .with_suite_tool("lonely", ToolExposure::Direct, None),
    ));
    let groups = registry.namespaces();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].name, "github");
    assert_eq!(
        groups[0].tools,
        vec!["gh_close".to_string(), "gh_open".to_string()]
    );
}

// Verifies: gh #77 - deferred tools resolve on first use: search
// finds them, activation declares them, unknown names are ignored.
#[test]
fn a_deferred_tool_resolves_on_first_use() {
    let mut registry = ExtensionRegistry::new();
    registry.register(Arc::new(
        FakeExt::new("suite")
            .with_suite_tool("seen", ToolExposure::Direct, None)
            .with_suite_tool("lazy", ToolExposure::Deferred, namespaced("github")),
    ));
    let found: Vec<String> = registry
        .tool_search("lazy")
        .iter()
        .map(|spec| spec.name.clone())
        .collect();
    assert_eq!(found, vec!["lazy".to_string()]);
    // Hidden tools are unreachable even to search.
    registry.register(Arc::new(FakeExt::new("hideout").with_suite_tool(
        "gone",
        ToolExposure::Hidden,
        None,
    )));
    assert!(registry.tool_search("gone").is_empty());
    // Activation applies registered names and ignores the rest;
    // the set replaces, so `seen` must be named to stay declared.
    let (applied, ignored) =
        registry.set_active_tools(&["lazy".to_string(), "seen".to_string(), "nope".to_string()]);
    assert_eq!(applied, vec!["lazy".to_string(), "seen".to_string()]);
    assert_eq!(ignored, vec!["nope".to_string()]);
    // A deferred tool stays undeclared even when active: discovery,
    // not declaration, is its path to the model.
    let declared: Vec<String> = registry
        .declared_tool_specs()
        .iter()
        .map(|spec| spec.name.clone())
        .collect();
    assert_eq!(declared, vec!["seen".to_string()]);
}

// Verifies: gh #77 - callable means direct-while-active plus
// codemode/deferred-while-registered; model-only and hidden never.
#[test]
fn callable_is_direct_active_plus_registered_codemode() {
    let mut registry = ExtensionRegistry::new();
    registry.register(Arc::new(
        FakeExt::new("suite")
            .with_suite_tool("seen", ToolExposure::Direct, None)
            .with_suite_tool("code", ToolExposure::Codemode, None)
            .with_suite_tool("lazy", ToolExposure::Deferred, None)
            .with_suite_tool("quiet", ToolExposure::ModelOnly, None)
            .with_suite_tool("gone", ToolExposure::Hidden, None),
    ));
    for name in ["seen", "code", "lazy"] {
        assert!(registry.is_callable(name), "{name} is callable");
    }
    for name in ["quiet", "gone", "unregistered"] {
        assert!(!registry.is_callable(name), "{name} is not callable");
    }
    // Deactivating a direct tool withdraws its callability; a
    // codemode tool stays callable while registered.
    registry.set_active_tools(&["code".to_string()]);
    assert!(!registry.is_callable("seen"));
    assert!(registry.is_callable("code"));
    assert!(registry.is_callable("lazy"));
}

// Verifies: gh #77 - annotations ride the spec without deciding
// anything (a hint is not a bypass: the registry keeps them, the
// permission layer never reads them).
#[test]
fn annotations_ride_the_spec() {
    let mut registry = ExtensionRegistry::new();
    registry.register(Arc::new(
        FakeExt::new("suite").with_annotated_tool("readish"),
    ));
    let spec = registry.tool_schema("readish").expect("registered");
    assert_eq!(spec.exposure, ToolExposure::Direct);
    let annotations = spec.annotations.as_ref().expect("kept");
    assert_eq!(annotations.read_only_hint, Some(true));
    assert_eq!(annotations.destructive_hint, Some(false));
}

// Verifies: gh #45 - the mutation chain composes in order (each
// handler sees the previous arguments) and a block vetoes with its
// reason.
#[tokio::test]
async fn the_mutation_chain_composes_and_blocks() {
    use lca_protocol::ToolCallPatch;
    let call = ToolCall {
        call_id: "c1".to_string(),
        name: "read".to_string(),
        arguments: r#"{"path":"a"}"#.to_string(),
        parent_call_id: None,
    };
    // Identity: no hooks-tool-call handle, the call passes through.
    let plain = ExtensionRegistry::new();
    assert_eq!(plain.mutate_tool_call(&call).await.expect("passes"), call);
    // Composition in registration order.
    let mut registry = ExtensionRegistry::new();
    registry.register(Arc::new(FakeExt::new("first").with_patch(ToolCallPatch {
        arguments: Some(r#"{"path":"b"}"#.to_string()),
        block: None,
    })));
    registry.register(Arc::new(FakeExt::new("second").with_patch(ToolCallPatch {
        arguments: Some(r#"{"path":"c"}"#.to_string()),
        block: None,
    })));
    let mutated = registry.mutate_tool_call(&call).await.expect("mutates");
    assert_eq!(mutated.arguments, r#"{"path":"c"}"#);
    // A block vetoes with its reason, wherever it sits.
    let mut blocked = ExtensionRegistry::new();
    blocked.register(Arc::new(FakeExt::new("veto").with_patch(ToolCallPatch {
        arguments: None,
        block: Some("no reads here".to_string()),
    })));
    assert_eq!(
        blocked.mutate_tool_call(&call).await.expect_err("blocks"),
        "no reads here"
    );
}

// Verifies: gh #45 - cache votes default warm and trust votes default
// undecided when no extension declares those worlds.
#[tokio::test]
async fn cache_and_trust_default_without_their_worlds() {
    let registry = ExtensionRegistry::new();
    assert!(registry.cache_warm("p", "m").await);
    assert_eq!(
        registry.project_trust("/tmp/work").await,
        (lca_protocol::TrustVote::Undecided, false)
    );
}
