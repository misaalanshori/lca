//! Curated file-based project memory (gh #199): a `learn` tool
//! appends facts to `MEMORY.md`, and a context-transform injects a
//! session-start-pinned snapshot at the head of the system prompt. No
//! embeddings, no daemons, no vector anything: plain Markdown the
//! owner can read, edit, and commit.
//!
//! Paths: project memory lives at
//! `.lca/extensions/project-memory/MEMORY.md` under the `workspace`
//! scope; global memory lives at `MEMORY.md` under the `private`
//! scope (per-extension, `<data>/private/project-memory/`). The
//! issue's literal `~/.lca/extensions/...` global path sits inside
//! the agent state directory, which every non-`private` scope
//! refuses by construction — `private` is the vocabulary-legal
//! equivalent, and the test pins the mapping.
//!
//! # Unsafe-code exemption
//!
//! The `wasm32` half carries generated `wit-bindgen` export shims; the
//! module holds the allowance.

#![deny(unsafe_code)]

use std::sync::Mutex;

/// The manifest this form ships with (single source for
/// [`manifest_grants`]; a test keeps them in step).
pub const MANIFEST: &str = include_str!("../extension.toml");

/// The grants the manifest declares: read-write on the workspace
/// (project memory) and on the extension's own `private` directory
/// (global memory) - nothing else.
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::expect_used)] // the extension's own literal manifest patterns are valid by construction.
pub fn manifest_grants() -> lca_tools::CapabilityGrants {
    use lca_permissions::{FsMode, ScopeGrant};
    lca_tools::CapabilityGrants {
        fs: vec![
            ScopeGrant::parse("workspace", FsMode::ReadWrite)
                .expect("the manifest's own scope parses"),
            ScopeGrant::parse("private", FsMode::ReadWrite)
                .expect("the manifest's own scope parses"),
        ],
        fs_declared: true,
        ..Default::default()
    }
}

/// Which memory file a fact belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryScope {
    /// This repository's memory.
    Project,
    /// The user's cross-repository memory.
    Global,
}

/// Project memory's path under the `workspace` scope.
pub const PROJECT_PATH: &str = ".lca/extensions/project-memory/MEMORY.md";
/// Global memory's path under the `private` scope.
pub const GLOBAL_PATH: &str = "MEMORY.md";

/// The file header a fresh memory file starts with.
pub const MEMORY_HEADER: &str = "# Project Memory & Learnings\n";

/// The snapshot budget in tokens (gh #199): roughly four characters
/// per token, truncation marked so the model knows it reads an
/// excerpt rather than the whole file.
pub const MEMORY_TOKEN_BUDGET: usize = 1024;

/// The filesystem view the memory needs: read plus append-write
/// through the `fs` capability, so neither mode reaches around it.
pub trait FsView: Send + Sync {
    /// One file's text; `None` when it does not exist yet.
    fn read(
        &self,
        scope: &str,
        path: &str,
    ) -> Result<Option<String>, lca_protocol::CapabilityError>;
    /// Replace one file's text (parents are the host's business).
    fn write(
        &self,
        scope: &str,
        path: &str,
        text: &str,
    ) -> Result<(), lca_protocol::CapabilityError>;
}

fn location(scope: MemoryScope) -> (&'static str, &'static str) {
    match scope {
        MemoryScope::Project => ("workspace", PROJECT_PATH),
        MemoryScope::Global => ("private", GLOBAL_PATH),
    }
}

/// Read one memory file's text; empty when it does not exist yet.
pub fn read_memory(
    fs: &dyn FsView,
    scope: MemoryScope,
) -> Result<String, lca_protocol::CapabilityError> {
    let (fs_scope, path) = location(scope);
    Ok(fs.read(fs_scope, path)?.unwrap_or_default())
}

/// Append one fact to a memory file. Returns a confirmation the tool
/// hands back to the model. Empty facts refuse; an untrusted
/// workspace refuses the global scope (the anti-poisoning
/// guardrail: a hostile repo must not reach user-global memory).
pub fn learn_fact(
    fs: &dyn FsView,
    project_trusted: bool,
    fact: &str,
    scope: MemoryScope,
) -> Result<String, String> {
    if scope == MemoryScope::Global && !project_trusted {
        return Err(
            "global memory refuses writes from an untrusted workspace; trust the project first"
                .to_string(),
        );
    }
    let fact = fact.split_whitespace().collect::<Vec<_>>().join(" ");
    if fact.is_empty() {
        return Err("there is no fact to learn in an empty message".to_string());
    }
    let (fs_scope, path) = location(scope);
    let mut text = fs
        .read(fs_scope, path)
        .map_err(|err| err.to_string())?
        .unwrap_or_default();
    if text.trim().is_empty() {
        text = MEMORY_HEADER.to_string();
    }
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str("- ");
    text.push_str(&fact);
    text.push('\n');
    fs.write(fs_scope, path, &text)
        .map_err(|err| err.to_string())?;
    Ok(format!("learned ({scope:?}): {fact}"))
}

/// Parse the `learn` tool's arguments: `fact` required, `scope`
/// defaulting to project, anything else refused.
pub fn parse_learn_args(json: &str) -> Result<(String, MemoryScope), String> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|err| format!("arguments are not JSON: {err}"))?;
    let fact = value
        .get("fact")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "the learn tool needs a `fact` string".to_string())?
        .to_string();
    let scope = match value.get("scope").and_then(serde_json::Value::as_str) {
        None | Some("project") => MemoryScope::Project,
        Some("global") => MemoryScope::Global,
        Some(other) => return Err(format!("unknown learn scope `{other}`")),
    };
    Ok((fact, scope))
}

/// Shape one memory file's text into the injected block, truncated
/// near the token budget with the cut marked.
pub fn snapshot_block(memory_text: &str) -> String {
    let mut block = String::from("# Project Memory\n");
    let char_budget = 4 * MEMORY_TOKEN_BUDGET;
    if memory_text.len() <= char_budget {
        block.push_str(memory_text);
        return block;
    }
    let mut end = char_budget;
    while !memory_text.is_char_boundary(end) {
        end -= 1;
    }
    block.push_str(&memory_text[..end]);
    block.push_str("\n…(memory truncated to the session budget)");
    block
}

/// Read the project memory once and freeze it: the first call loads
/// the file into the pin, every later call reuses the pin. A
/// mid-session `learn` updates the file for future sessions but never
/// moves the active session's head (ADR-0017).
pub fn pinned_snapshot(fs: &dyn FsView, pin: &Mutex<Option<String>>) -> Option<String> {
    let mut guard = pin.lock().unwrap_or_else(|poison| poison.into_inner());
    if guard.is_none() {
        let text = read_memory(fs, MemoryScope::Project).unwrap_or_default();
        *guard = if text.trim().is_empty() {
            None
        } else {
            Some(snapshot_block(&text))
        };
    }
    guard.clone()
}

/// The whole transform: prepend the frozen block to the head of the
/// system prompt (same head rule as rules-compat: grow the leading
/// system message in place, else insert one). No snapshot, no touch.
pub fn transform_with_memory(
    mut messages: Vec<lca_protocol::ChatMessage>,
    snapshot: Option<&str>,
) -> Vec<lca_protocol::ChatMessage> {
    let Some(block) = snapshot else {
        return messages;
    };
    let prepend_to_system = messages
        .first()
        .is_some_and(|first| first.role == lca_protocol::MessageRole::System);
    if prepend_to_system {
        let first = &mut messages[0];
        let mut text = block.to_string();
        for part in std::mem::take(&mut first.content) {
            match part {
                lca_protocol::ContentBlock::Text { text: tail } => {
                    text.push('\n');
                    text.push_str(&tail);
                }
                other => first.content.push(other),
            }
        }
        first
            .content
            .insert(0, lca_protocol::ContentBlock::Text { text });
    } else {
        messages.insert(
            0,
            lca_protocol::ChatMessage::text(lca_protocol::MessageRole::System, block.to_string()),
        );
    }
    messages
}

/// The `learn` tool spec the model sees.
pub fn learn_tool_spec() -> lca_protocol::ToolSpec {
    lca_protocol::ToolSpec {
        name: "learn".to_string(),
        description: "Record an architectural rule or fact for future sessions.".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "fact": {
                    "type": "string",
                    "description": "Concise architectural rule or fact to remember."
                },
                "scope": {
                    "type": "string",
                    "enum": ["project", "global"],
                    "default": "project",
                    "description": "'project' (default) for repository rules; 'global' for user-wide preferences."
                }
            },
            "required": ["fact"]
        }),
        exposure: lca_protocol::ToolExposure::Direct,
        namespace: None,
        annotations: None,
        extras: Default::default(),
    }
}

/// Convenience over the free functions for tests and the WASM guest:
/// one memory bound to one filesystem view and one trust bit.
pub struct ProjectMemory<'a> {
    fs: &'a dyn FsView,
    project_trusted: bool,
    pin: Mutex<Option<String>>,
}

impl<'a> ProjectMemory<'a> {
    /// Bind to a view of a trusted workspace.
    pub fn new_trusted(fs: &'a dyn FsView) -> ProjectMemory<'a> {
        ProjectMemory {
            fs,
            project_trusted: true,
            pin: Mutex::new(None),
        }
    }

    /// Bind to a view of an untrusted workspace (global refuses).
    pub fn new_untrusted(fs: &'a dyn FsView) -> ProjectMemory<'a> {
        ProjectMemory {
            fs,
            project_trusted: false,
            pin: Mutex::new(None),
        }
    }

    /// Learn one fact; the confirmation the tool returns.
    pub fn learn(&self, fact: &str, scope: MemoryScope) -> Result<String, String> {
        learn_fact(self.fs, self.project_trusted, fact, scope)
    }

    /// Transform one turn's messages through the pinned snapshot.
    pub fn transform(
        &self,
        messages: Vec<lca_protocol::ChatMessage>,
    ) -> Vec<lca_protocol::ChatMessage> {
        let snapshot = pinned_snapshot(self.fs, &self.pin);
        transform_with_memory(messages, snapshot.as_deref())
    }
}

// ---------------------------------------------------------------------------
// Native delivery mode
// ---------------------------------------------------------------------------

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use super::*;
    use std::sync::Arc;

    use lca_ext_abi::{DeliveryMode, DispatchFuture, ExtensionDispatch, World};
    use lca_protocol::DispatchError;

    impl FsView for lca_tools::Capabilities {
        fn read(
            &self,
            scope: &str,
            path: &str,
        ) -> Result<Option<String>, lca_protocol::CapabilityError> {
            match self.fs_read(scope, path) {
                Ok(bytes) => Ok(Some(String::from_utf8_lossy(&bytes).into_owned())),
                Err(lca_protocol::CapabilityError::NotFound(_)) => Ok(None),
                Err(err) => Err(err),
            }
        }

        fn write(
            &self,
            scope: &str,
            path: &str,
            text: &str,
        ) -> Result<(), lca_protocol::CapabilityError> {
            self.fs_write(scope, path, text.as_bytes())
        }
    }

    /// The native handle. `project_trusted` arrives at construction
    /// from the host's trust store (`is_trusted(cwd)`): the sandbox
    /// has no trust import, so the bit crosses here, not through a
    /// model-controlled argument (which prompt injection could set).
    pub struct ProjectMemoryNative {
        cap: Arc<lca_tools::Capabilities>,
        project_trusted: bool,
        pin: Mutex<Option<String>>,
    }

    impl ProjectMemoryNative {
        /// Build from the engine and the workspace's trust bit.
        pub fn new(
            cap: Arc<lca_tools::Capabilities>,
            project_trusted: bool,
        ) -> ProjectMemoryNative {
            ProjectMemoryNative {
                cap,
                project_trusted,
                pin: Mutex::new(None),
            }
        }
    }

    impl ExtensionDispatch for ProjectMemoryNative {
        fn name(&self) -> &str {
            "project-memory"
        }

        fn delivery(&self) -> DeliveryMode {
            DeliveryMode::Native
        }

        fn worlds(&self) -> Vec<World> {
            vec![World::Tool, World::ContextTransform]
        }

        fn tool_specs(&self) -> Result<Vec<lca_protocol::ToolSpec>, DispatchError> {
            Ok(vec![learn_tool_spec()])
        }

        fn execute_tool<'a>(
            &'a self,
            call: &'a lca_protocol::ToolCall,
        ) -> DispatchFuture<'a, Result<lca_protocol::ToolResult, DispatchError>> {
            let cap = self.cap.clone();
            let trusted = self.project_trusted;
            let arguments = call.arguments.clone();
            let call_id = call.call_id.clone();
            Box::pin(async move {
                let ok = |content: String| lca_protocol::ToolResult {
                    call_id: call_id.clone(),
                    status: lca_protocol::ToolResultStatus::Ok,
                    content,
                    truncated: false,
                    exit_code: None,
                    full_output_path: None,
                    nested: Vec::new(),
                    images: Vec::new(),
                    extras: Default::default(),
                };
                let refused = |content: String| lca_protocol::ToolResult {
                    call_id: call_id.clone(),
                    status: lca_protocol::ToolResultStatus::Error,
                    content,
                    truncated: false,
                    exit_code: None,
                    full_output_path: None,
                    nested: Vec::new(),
                    images: Vec::new(),
                    extras: Default::default(),
                };
                match parse_learn_args(&arguments) {
                    Ok((fact, scope)) => match learn_fact(cap.as_ref(), trusted, &fact, scope) {
                        Ok(confirmation) => Ok(ok(confirmation)),
                        Err(reason) => Ok(refused(reason)),
                    },
                    Err(reason) => Ok(refused(reason)),
                }
            })
        }

        fn command_specs(&self) -> Result<Vec<lca_protocol::CommandSpec>, DispatchError> {
            Ok(Vec::new())
        }

        fn invoke_command(
            &self,
            _name: &str,
            _argument: &str,
        ) -> Result<lca_protocol::CommandEffect, DispatchError> {
            Ok(lca_protocol::CommandEffect::None)
        }

        fn transform_messages(
            &self,
            messages: Vec<lca_protocol::ChatMessage>,
        ) -> DispatchFuture<
            'static,
            Result<Result<Vec<lca_protocol::ChatMessage>, String>, DispatchError>,
        > {
            let cap = self.cap.clone();
            // The pin lives on the handle, and one handle serves one
            // workspace (the engine's roots fix it at build): same
            // workspace shares the freeze, another workspace builds
            // another handle with its own pin.
            let snapshot = pinned_snapshot(cap.as_ref(), &self.pin);
            Box::pin(std::future::ready(Ok(Ok(transform_with_memory(
                messages,
                snapshot.as_deref(),
            )))))
        }

        fn on_pre_turn(&self) -> DispatchFuture<'static, Result<(), DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }

        fn on_pre_tool_use<'a>(
            &'a self,
            _call: &'a lca_protocol::ToolCall,
        ) -> DispatchFuture<'a, Result<lca_protocol::HookAction, DispatchError>> {
            Box::pin(std::future::ready(Ok(lca_protocol::HookAction::Allow)))
        }

        fn on_post_tool_use<'a>(
            &'a self,
            _observation: &'a lca_protocol::PostToolObservation,
        ) -> DispatchFuture<'a, Result<(), DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }

        fn on_post_turn_end<'a>(
            &'a self,
            _status: &'a str,
        ) -> DispatchFuture<'a, Result<(), DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }

        fn on_attention_required<'a>(
            &'a self,
            _reason: &'a str,
        ) -> DispatchFuture<'a, Result<(), DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }

        fn on_session_close(&self) -> DispatchFuture<'static, Result<(), DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use native::ProjectMemoryNative;

// ---------------------------------------------------------------------------
// WASM delivery mode
// ---------------------------------------------------------------------------
//
// The guest has no project-trust import, so it cannot enforce the
// global-write guardrail itself: WASM `learn` serves the project
// scope only and refuses `global` with the reason, while the native
// handle above serves both behind its constructor trust bit.

#[cfg(target_arch = "wasm32")]
#[allow(unsafe_code)] // generated wit-bindgen export shims
mod wasm_mode {
    use super::*;

    /// The guest-side pin: one component instance serves one session,
    /// so a process-wide freeze is the session-start snapshot.
    static PIN: Mutex<Option<String>> = Mutex::new(None);

    /// The guest filesystem behind the shared [`FsView`], one copy
    /// per world module (each `generate!` owns its host bindings).
    struct GuestFs;

    mod wasm_tool {
        wit_bindgen::generate!({
            path: "../../wit",
            world: "tool",
            export_macro_name: "export_memory_tool",
            with: {
                "lca:host/log@0.6.0": generate,
                "lca:host/fs@0.6.0": generate,
                "lca:host/process@0.6.0": generate,
                "lca:host/pty@0.6.0": generate,
                "lca:host/ui-dialogs@0.6.0": generate,
                "lca:host/tools@0.6.0": generate,
                "lca:host/resources@0.6.0": generate,
                "lca:host/state@0.6.0": generate,
            },
        });

        use exports::lca::ext::execute::{Guest as ExecuteTrait, ToolResult as WasmResult};
        use exports::lca::ext::tool_schema::{Guest as SchemaGuest, Schema};
        use lca::ext::types::ToolCall;
        use lca::host::fs;

        impl super::super::FsView for super::GuestFs {
            fn read(
                &self,
                scope: &str,
                path: &str,
            ) -> Result<Option<String>, lca_protocol::CapabilityError> {
                match fs::read(scope, path) {
                    Ok(bytes) => Ok(Some(String::from_utf8_lossy(&bytes).into_owned())),
                    Err(_) => Ok(None),
                }
            }

            fn write(
                &self,
                scope: &str,
                path: &str,
                text: &str,
            ) -> Result<(), lca_protocol::CapabilityError> {
                fs::write(scope, path, text.as_bytes()).map_err(|_| {
                    lca_protocol::CapabilityError::Io("fs write refused".to_string())
                })?;
                Ok(())
            }
        }
        pub struct LearnWasm;

        impl SchemaGuest for LearnWasm {
            fn get_schema() -> Schema {
                let spec = super::super::learn_tool_spec();
                Schema {
                    name: spec.name,
                    description: spec.description,
                    parameters: spec.parameters.to_string(),
                    extras: Vec::new(),
                }
            }
        }

        impl ExecuteTrait for LearnWasm {
            fn run(call: ToolCall) -> WasmResult {
                let fail = |content: String| WasmResult {
                    call_id: call.call_id.clone(),
                    status: "error".to_string(),
                    content: Some(content),
                    truncated: false,
                    extras: Vec::new(),
                };
                let (fact, scope) = match super::super::parse_learn_args(&call.arguments) {
                    Ok(parsed) => parsed,
                    Err(reason) => return fail(reason),
                };
                // The guest has no project-trust import: the project
                // scope always serves; `global` refuses with the reason
                // (the native handle serves it behind its trust bit).
                if scope == super::super::MemoryScope::Global {
                    return fail(
                        "global memory refuses writes from the WASM sandbox; trust the project and use the native handle"
                            .to_string(),
                    );
                }
                match super::super::learn_fact(&super::GuestFs, true, &fact, scope) {
                    Ok(confirmation) => WasmResult {
                        call_id: call.call_id,
                        status: "ok".to_string(),
                        content: Some(confirmation),
                        truncated: false,
                        extras: Vec::new(),
                    },
                    Err(reason) => fail(reason),
                }
            }
        }

        export_memory_tool!(LearnWasm);
    }

    mod wasm_transform {
        wit_bindgen::generate!({
            path: "../../wit",
            world: "context-transform",
            export_macro_name: "export_memory_transform",
            with: {
                "lca:host/log@0.6.0": generate,
                "lca:host/fs@0.6.0": generate,
                "lca:host/resources@0.6.0": generate,
                "lca:host/state@0.6.0": generate,
            },
        });

        use exports::lca::ext::transform::{Guest as TransformGuest, Message as WasmMessage};
        use lca::ext::types::ToolCall as WasmToolCall;

        fn from_wasm(message: &WasmMessage) -> lca_protocol::ChatMessage {
            lca_protocol::ChatMessage {
                role: match message.role.as_str() {
                    "system" => lca_protocol::MessageRole::System,
                    "user" => lca_protocol::MessageRole::User,
                    "assistant" => lca_protocol::MessageRole::Assistant,
                    _ => lca_protocol::MessageRole::Tool,
                },
                content: message
                    .content
                    .iter()
                    .map(|block| match block {
                        lca::ext::types::ContentBlock::Text(text) => {
                            lca_protocol::ContentBlock::Text { text: text.clone() }
                        }
                        lca::ext::types::ContentBlock::Image((media_type, bytes)) => {
                            lca_protocol::ContentBlock::Image {
                                media_type: media_type.clone(),
                                bytes: bytes.clone(),
                            }
                        }
                    })
                    .collect(),
                tool_calls: message
                    .tool_calls
                    .iter()
                    .map(|call| lca_protocol::ToolCall {
                        call_id: call.call_id.clone(),
                        name: call.name.clone(),
                        arguments: call.arguments.clone(),
                        parent_call_id: None,
                    })
                    .collect(),
                tool_call_id: message.tool_call_id.clone(),
                usage: None,
                extras: Default::default(),
            }
        }

        fn to_wasm(message: &lca_protocol::ChatMessage) -> WasmMessage {
            WasmMessage {
                role: match message.role {
                    lca_protocol::MessageRole::System => "system".to_string(),
                    lca_protocol::MessageRole::User => "user".to_string(),
                    lca_protocol::MessageRole::Assistant => "assistant".to_string(),
                    lca_protocol::MessageRole::Tool => "tool".to_string(),
                },
                content: message
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        lca_protocol::ContentBlock::Text { text } => {
                            Some(lca::ext::types::ContentBlock::Text(text.clone()))
                        }
                        lca_protocol::ContentBlock::Image { media_type, bytes } => {
                            Some(lca::ext::types::ContentBlock::Image((
                                media_type.clone(),
                                bytes.clone(),
                            )))
                        }
                        _ => None,
                    })
                    .collect(),
                tool_calls: message
                    .tool_calls
                    .iter()
                    .map(|call| WasmToolCall {
                        call_id: call.call_id.clone(),
                        name: call.name.clone(),
                        arguments: call.arguments.clone(),
                        extras: Vec::new(),
                    })
                    .collect(),
                tool_call_id: message.tool_call_id.clone(),
                extras: Vec::new(),
            }
        }

        pub struct MemoryWasm;

        impl TransformGuest for MemoryWasm {
            fn transform(messages: Vec<WasmMessage>) -> Result<Vec<WasmMessage>, String> {
                let protocol: Vec<lca_protocol::ChatMessage> =
                    messages.iter().map(from_wasm).collect();
                let snapshot = super::super::pinned_snapshot(&super::GuestFs, &super::PIN);
                Ok(
                    super::super::transform_with_memory(protocol, snapshot.as_deref())
                        .iter()
                        .map(to_wasm)
                        .collect(),
                )
            }
        }

        export_memory_transform!(MemoryWasm);
    }
}
