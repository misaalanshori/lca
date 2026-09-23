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
        }
    }

    fn with_tool(mut self, tool: &str) -> FakeExt {
        self.worlds.push(World::Tool);
        self.tools.push(ToolSpec {
            name: tool.to_string(),
            description: "test tool".to_string(),
            parameters: serde_json::json!({"type": "object"}),
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
    let store_root = std::env::temp_dir().join(format!("lca-reg-conf-{}", std::process::id()));
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
    assert_eq!(registry.tool_specs().len(), 1);
    let _ = spec_name("unused");
}
