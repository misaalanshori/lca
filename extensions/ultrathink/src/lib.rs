//! Single-turn maximum reasoning via the `ultrathink` magic keyword
//! (gh #198): a standalone lowercase `ultrathink` in prompt prose
//! boosts that turn's reasoning effort to max, and the session's
//! persistent default never moves.
//!
//! Why an extension and not core: changing effort between turns
//! busts prompt-cache affinity on several providers, so the spike
//! stays opt-in — users who never install this never pay it.
//!
//! Two host pieces do not exist yet under the 0.6 freeze, and the
//! code marks both: #45's turn-level request overrides (today
//! `on-pre-turn` is observe-only, so the hook detects nothing
//! actionable), and the `Thinking: max (ultrathink)` status-line
//! indicator (which needs the `ui` world's render half; the manifest
//! declares the region intent). What ships: the trigger logic, fully
//! pinned, plus both delivery skeletons.
//!
//! # Unsafe-code exemption
//!
//! The `wasm32` half carries generated `wit-bindgen` export shims; the
//! module holds the allowance.

#![deny(unsafe_code)]

/// The manifest this form ships with (a test keeps the worlds and
/// regions in step with it).
pub const MANIFEST: &str = include_str!("../extension.toml");

/// The magic word: standalone and lowercase only.
pub const KEYWORD: &str = "ultrathink";

fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// Whether the prompt asks for a single-turn max-effort boost: the
/// standalone lowercase word `ultrathink` in prose. Fenced code
/// blocks and inline backtick spans never trigger; an unclosed fence
/// fails quiet (a missed boost beats a cache-busting false boost).
pub fn should_boost(prompt: &str) -> bool {
    let bytes = prompt.as_bytes();
    let mut line_start = 0;
    let mut in_fence = false;
    while line_start <= bytes.len() {
        let line_end = bytes[line_start..]
            .iter()
            .position(|&byte| byte == b'\n')
            .map(|offset| line_start + offset)
            .unwrap_or(bytes.len());
        let line = &bytes[line_start..line_end];
        let trimmed = trim_ascii_start(line);
        if trimmed.starts_with(b"```") {
            in_fence = !in_fence;
        } else if !in_fence && line_contains_keyword(line) {
            return true;
        }
        line_start = line_end + 1;
    }
    false
}

fn trim_ascii_start(line: &[u8]) -> &[u8] {
    let mut rest = line;
    while matches!(rest.first(), Some(byte) if byte.is_ascii_whitespace()) {
        rest = &rest[1..];
    }
    rest
}

/// One line's prose with inline-code spans cut out, scanned for the
/// standalone keyword. A lone backtick past the last pair leaves the
/// tail out (unclosed inline code reads as code).
fn line_contains_keyword(line: &[u8]) -> bool {
    let keyword = KEYWORD.as_bytes();
    let mut index = 0;
    while index < line.len() {
        if line[index] == b'`' {
            let rest = &line[index + 1..];
            let Some(close) = rest.iter().position(|&byte| byte == b'`') else {
                return false;
            };
            index += close + 2;
            continue;
        }
        if line[index..].starts_with(keyword) {
            let before = index.checked_sub(1).map(|at| line[at]);
            let after = line.get(index + keyword.len()).copied();
            let bounded = before.is_none_or(|byte| !is_word_byte(byte))
                && after.is_none_or(|byte| !is_word_byte(byte));
            if bounded {
                return true;
            }
        }
        index += 1;
    }
    false
}

// ---------------------------------------------------------------------------
// Native delivery mode
// ---------------------------------------------------------------------------

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::sync::Arc;

    use lca_ext_abi::{DeliveryMode, DispatchFuture, ExtensionDispatch, World};
    use lca_protocol::DispatchError;

    /// The native handle. `on-pre-turn` is observe-only under ABI 0.6
    /// (it receives no prompt and returns no override), so the hook
    /// body is a pass-through: when #45 lands, it calls
    /// [`crate::should_boost`] on the turn's prompt and returns the max
    /// override for that turn only.
    pub struct Ultrathink {
        _cap: Arc<lca_tools::Capabilities>,
    }

    impl Ultrathink {
        /// Build from the engine (no capabilities needed: zero
        /// network, zero filesystem, zero daemons).
        pub fn new(cap: Arc<lca_tools::Capabilities>) -> Ultrathink {
            Ultrathink { _cap: cap }
        }
    }

    impl ExtensionDispatch for Ultrathink {
        fn name(&self) -> &str {
            "ultrathink"
        }

        fn delivery(&self) -> DeliveryMode {
            DeliveryMode::Native
        }

        fn worlds(&self) -> Vec<World> {
            vec![World::Hooks]
        }

        fn tool_specs(&self) -> Result<Vec<lca_protocol::ToolSpec>, DispatchError> {
            Err(DispatchError::MissingWorld {
                extension: "ultrathink".to_string(),
                world: "tool",
            })
        }

        fn execute_tool<'a>(
            &'a self,
            _call: &'a lca_protocol::ToolCall,
        ) -> DispatchFuture<'a, Result<lca_protocol::ToolResult, DispatchError>> {
            Box::pin(std::future::ready(Err(DispatchError::MissingWorld {
                extension: "ultrathink".to_string(),
                world: "tool",
            })))
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
            _messages: Vec<lca_protocol::ChatMessage>,
        ) -> DispatchFuture<
            'static,
            Result<Result<Vec<lca_protocol::ChatMessage>, String>, DispatchError>,
        > {
            Box::pin(std::future::ready(Err(DispatchError::MissingWorld {
                extension: "ultrathink".to_string(),
                world: "context-transform",
            })))
        }

        fn on_pre_turn(&self) -> DispatchFuture<'static, Result<(), DispatchError>> {
            // SEAM (gh #198, needs #45): observe-only today.
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
pub use native::Ultrathink;

// ---------------------------------------------------------------------------
// WASM delivery mode: the observe-only hooks world
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
#[allow(unsafe_code)] // generated wit-bindgen export shims
mod wasm_mode {
    wit_bindgen::generate!({
        path: "../../wit",
        world: "hooks",
        export_macro_name: "export_ultrathink",
        with: {
            "lca:host/ui-dialogs@0.6.0": generate,
        },
    });

    use exports::lca::ext::hook_attention_required::Guest as AttentionGuest;
    use exports::lca::ext::hook_post_tool_use::Guest as PostToolGuest;
    use exports::lca::ext::hook_post_turn_end::Guest as PostTurnGuest;
    use exports::lca::ext::hook_pre_tool_use::{Action, Guest as PreToolGuest, ToolCall};
    use exports::lca::ext::hook_pre_turn::Guest as PreTurnGuest;
    use exports::lca::ext::hook_session_close::Guest as CloseGuest;

    pub struct UltrathinkWasm;

    impl PreTurnGuest for UltrathinkWasm {
        // SEAM (gh #198, needs #45): observe-only today.
        fn on_pre_turn() {}
    }

    impl PreToolGuest for UltrathinkWasm {
        fn on_pre_tool_use(_call: ToolCall) -> Action {
            Action::Allow
        }
    }

    impl PostToolGuest for UltrathinkWasm {
        fn on_post_tool_use(
            _call: exports::lca::ext::hook_post_tool_use::ToolCall,
            _outcome: exports::lca::ext::hook_post_tool_use::ToolResult,
        ) {
        }
    }

    impl PostTurnGuest for UltrathinkWasm {
        fn on_post_turn_end(_status: String) {}
    }

    impl AttentionGuest for UltrathinkWasm {
        fn on_attention_required(_reason: String) {}
    }

    impl CloseGuest for UltrathinkWasm {
        fn on_session_close() {}
    }

    export_ultrathink!(UltrathinkWasm);
}
