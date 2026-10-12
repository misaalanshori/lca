//! Multi-IDE rulebook compatibility (gh #197): Cursor (`.cursor/rules/*.mdc`,
//! `.cursorrules`), Cline (`.clinerules`), Windsurf (`.windsurfrules`), and
//! Copilot (`.github/copilot-instructions.md`) rule files are discovered in
//! a fixed order and injected as one attributed block at the head of the
//! system prompt. `lca-core` natively reads only `AGENTS.md`/`CLAUDE.md`;
//! everything else rides this opt-in extension, so teams that never used
//! those editors pay no scanning cost.
//!
//! # Unsafe-code exemption
//!
//! The `wasm32` half carries generated `wit-bindgen` export shims; the
//! module holds the allowance.

#![deny(unsafe_code)]

/// The manifest this form ships with (single source for
/// [`manifest_grants`]; a test keeps them in step).
pub const MANIFEST: &str = include_str!("../extension.toml");

/// The grants the manifest declares: read access to the workspace,
/// which is where every rule file lives - nothing else.
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::expect_used)] // the extension's own literal manifest patterns are valid by construction.
pub fn manifest_grants() -> lca_tools::CapabilityGrants {
    lca_tools::CapabilityGrants {
        fs: vec![
            lca_permissions::ScopeGrant::parse("workspace", lca_permissions::FsMode::Read)
                .expect("the manifest's own scope parses"),
        ],
        fs_declared: true,
        ..Default::default()
    }
}

/// One discovered rule file: where it came from, what it says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleFile {
    /// Workspace-relative path, the attribution the model sees.
    pub source: String,
    /// The file's text, verbatim (frontmatter included: small, and the
    /// `alwaysApply` refinement is a later cut, not this one).
    pub text: String,
}

/// The discovery order, fixed so the injected block is byte-stable
/// across turns (ADR-0017): the Cursor rules directory first, then
/// the single-file rulebooks.
const CURSOR_RULES_DIR: &str = ".cursor/rules";

/// Single-file rulebooks in discovery order.
const RULE_FILES: &[&str] = &[
    ".cursorrules",
    ".clinerules",
    ".windsurfrules",
    ".github/copilot-instructions.md",
];

/// The filesystem view the discovery needs: exactly the `fs`
/// capability's read surface, so neither mode reaches around it.
pub trait FsView: Send + Sync {
    /// Entries under a workspace-relative directory; `None` when it
    /// does not exist (no rulebook is a pass-through, not an error).
    fn list(&self, path: &str) -> Result<Option<Vec<String>>, lca_protocol::CapabilityError>;
    /// One file's text.
    fn read(&self, path: &str) -> Result<Option<String>, lca_protocol::CapabilityError>;
}

/// Discover every rule file through the fs view, in fixed order. A
/// missing directory or file is skipped silently: absence is the
/// common case and must cost nothing, not even a warning.
pub fn discover_rules(fs: &dyn FsView) -> Vec<RuleFile> {
    let mut rules = Vec::new();
    if let Ok(Some(mut entries)) = fs.list(CURSOR_RULES_DIR) {
        entries.sort();
        for entry in entries {
            if !entry.ends_with(".mdc") {
                continue;
            }
            let source = format!("{CURSOR_RULES_DIR}/{entry}");
            if let Ok(Some(text)) = fs.read(&source) {
                rules.push(RuleFile { source, text });
            }
        }
    }
    for path in RULE_FILES {
        if let Ok(Some(text)) = fs.read(path) {
            rules.push(RuleFile {
                source: path.to_string(),
                text,
            });
        }
    }
    rules
}

/// Format the discovered rules as one attributed block, or `None`
/// when there is nothing to inject.
pub fn format_block(rules: &[RuleFile]) -> Option<String> {
    if rules.is_empty() {
        return None;
    }
    let mut block = String::from("# External Project Rules\n");
    for rule in rules {
        block.push_str("\n## Discovered from ");
        block.push_str(&rule.source);
        block.push('\n');
        block.push_str(&rule.text);
        if !rule.text.ends_with('\n') {
            block.push('\n');
        }
    }
    Some(block)
}

/// The whole transform: prepend the block to the head of the system
/// prompt. When the first message is a system message it grows in
/// place (message count stable, cache prefix stable); otherwise the
/// block becomes its own leading system message. No rules, no touch.
pub fn transform_with_rules(
    mut messages: Vec<lca_protocol::ChatMessage>,
    rules: &[RuleFile],
) -> Vec<lca_protocol::ChatMessage> {
    let Some(block) = format_block(rules) else {
        return messages;
    };
    let prepend_to_system = messages
        .first()
        .is_some_and(|first| first.role == lca_protocol::MessageRole::System);
    if prepend_to_system {
        let first = &mut messages[0];
        let mut text = block;
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
            lca_protocol::ChatMessage::text(lca_protocol::MessageRole::System, block),
        );
    }
    messages
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
        fn list(&self, path: &str) -> Result<Option<Vec<String>>, lca_protocol::CapabilityError> {
            match self.fs_list("workspace", path) {
                Ok(entries) => Ok(Some(
                    entries
                        .into_iter()
                        .map(|entry| entry.trim_end_matches('/').to_string())
                        .collect(),
                )),
                Err(lca_protocol::CapabilityError::NotFound(_)) => Ok(None),
                Err(err) => Err(err),
            }
        }

        fn read(&self, path: &str) -> Result<Option<String>, lca_protocol::CapabilityError> {
            match self.fs_read("workspace", path) {
                Ok(bytes) => Ok(Some(String::from_utf8_lossy(&bytes).into_owned())),
                Err(lca_protocol::CapabilityError::NotFound(_)) => Ok(None),
                Err(err) => Err(err),
            }
        }
    }

    /// The native handle: discover on every turn through the `fs`
    /// capability (declared `read` on the workspace only).
    pub struct RulesCompat {
        cap: Arc<lca_tools::Capabilities>,
    }

    impl RulesCompat {
        /// Build from the engine whose manifest grants carry `fs`.
        pub fn new(cap: Arc<lca_tools::Capabilities>) -> RulesCompat {
            RulesCompat { cap }
        }
    }

    /// Boilerplate shared by every world this extension does not
    /// serve: exactly the skills extension's shape, one world only.
    macro_rules! missing_world {
        ($world:literal) => {
            Err(DispatchError::MissingWorld {
                extension: "rules-compat".to_string(),
                world: $world,
            })
        };
    }

    impl ExtensionDispatch for RulesCompat {
        fn name(&self) -> &str {
            "rules-compat"
        }

        fn delivery(&self) -> DeliveryMode {
            DeliveryMode::Native
        }

        fn worlds(&self) -> Vec<World> {
            vec![World::ContextTransform]
        }

        fn tool_specs(&self) -> Result<Vec<lca_protocol::ToolSpec>, DispatchError> {
            missing_world!("tool")
        }

        fn execute_tool<'a>(
            &'a self,
            _call: &'a lca_protocol::ToolCall,
        ) -> DispatchFuture<'a, Result<lca_protocol::ToolResult, DispatchError>> {
            Box::pin(std::future::ready(missing_world!("tool")))
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
            Box::pin(async move {
                tokio::task::spawn_blocking(move || {
                    let rules = discover_rules(cap.as_ref());
                    transform_with_rules(messages, &rules)
                })
                .await
                .map_err(|_| DispatchError::Failed("rules-compat panicked".to_string()))
                .map(Ok)
            })
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
pub use native::RulesCompat;

// ---------------------------------------------------------------------------
// WASM delivery mode: the same logic behind the host's fs import
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
#[allow(unsafe_code)] // generated wit-bindgen export shims
mod wasm_mode {
    use super::*;

    wit_bindgen::generate!({
        path: "../../wit",
        world: "context-transform",
        export_macro_name: "export_transform",
        with: {
            "lca:host/log@0.6.0": generate,
            "lca:host/fs@0.6.0": generate,
            // ADR-0030's bags: every world imports them.
            "lca:host/resources@0.6.0": generate,
            "lca:host/state@0.6.0": generate,
        },
    });

    use exports::lca::ext::transform::{Guest as TransformGuest, Message as WasmMessage};
    use lca::ext::types::ToolCall as WasmToolCall;
    use lca::host::fs;

    struct GuestFs;

    impl FsView for GuestFs {
        fn list(&self, path: &str) -> Result<Option<Vec<String>>, lca_protocol::CapabilityError> {
            match fs::list_entries("workspace", path) {
                Ok(entries) => Ok(Some(
                    entries
                        .into_iter()
                        .map(|entry| entry.trim_end_matches('/').to_string())
                        .collect(),
                )),
                Err(_) => Ok(None),
            }
        }

        fn read(&self, path: &str) -> Result<Option<String>, lca_protocol::CapabilityError> {
            match fs::read("workspace", path) {
                Ok(bytes) => Ok(Some(String::from_utf8_lossy(&bytes).into_owned())),
                Err(_) => Ok(None),
            }
        }
    }

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
                    lca_protocol::ContentBlock::Image { media_type, bytes } => Some(
                        lca::ext::types::ContentBlock::Image((media_type.clone(), bytes.clone())),
                    ),
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

    pub struct RulesCompatWasm;

    impl TransformGuest for RulesCompatWasm {
        fn transform(messages: Vec<WasmMessage>) -> Result<Vec<WasmMessage>, String> {
            let protocol: Vec<lca_protocol::ChatMessage> = messages.iter().map(from_wasm).collect();
            let rules = discover_rules(&GuestFs);
            Ok(transform_with_rules(protocol, &rules)
                .iter()
                .map(to_wasm)
                .collect())
        }
    }

    export_transform!(RulesCompatWasm);
}
