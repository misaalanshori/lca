//! The `lca:ext` contract crate: the normative WIT package under `wit/`
//! plus generated bindings.
//!
//! Extension authors depend on this crate for the guest bindings without
//! pulling in the agent (ADR-0002). The binary enables the `host` feature
//! to get Wasmtime-side bindings for every world.

#![forbid(unsafe_code)]

/// The ABI version this build implements (`major.minor`, per the manifest's
/// `abi` field and the support window in docs/abi-versioning.md).
pub const ABI_VERSION: &str = "0.1";

/// The WIT package name that crosses every manifest and registry tag.
pub const PACKAGE: &str = "lca:ext";

/// One implemented world; additive as worlds land (ADR-0019).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum World {
    /// Register one callable tool.
    Tool,
    /// Provide one slash command.
    Command,
    /// Observe and gate the agent loop.
    Hooks,
}

/// Whether this handle runs sandboxed (WASM) or in-process (native); the
/// extension list labels both (FR-EXT-7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryMode {
    /// Compiled into the binary: no sandbox, labeled unsandboxed
    /// (ADR-0013).
    Native,
    /// A component under the capability-enforced host.
    Wasm,
}

impl DeliveryMode {
    /// The label the extension list shows (FR-EXT-7).
    pub fn label(&self) -> &'static str {
        match self {
            DeliveryMode::Native => "in-process (unsandboxed)",
            DeliveryMode::Wasm => "sandboxed",
        }
    }
}

/// The one interface both delivery modes implement (ADR-0019,
/// FR-EXT-6). Call sites hold `Arc<dyn ExtensionDispatch>` and never
/// branch on the mode.
pub mod dispatch {
    use crate::{DeliveryMode, World};
    use lca_protocol::{
        CommandEffect, CommandSpec, DispatchError, HookAction, PostToolObservation, ToolCall,
        ToolResult, ToolSpec,
    };

    /// A loaded extension, whichever way it is delivered.
    pub trait ExtensionDispatch: Send + Sync {
        /// The extension's identity (its manifest name).
        fn name(&self) -> &str;

        /// Sandboxed or in-process (FR-EXT-7's data).
        fn delivery(&self) -> DeliveryMode;

        /// Worlds this handle implements.
        fn worlds(&self) -> Vec<World>;

        /// Tool specs (`tool` world).
        fn tool_specs(&self) -> Result<Vec<ToolSpec>, DispatchError>;

        /// Run one tool call (`tool` world).
        fn execute_tool(&self, call: &ToolCall) -> Result<ToolResult, DispatchError>;

        /// Registered commands (`command` world).
        fn command_specs(&self) -> Result<Vec<CommandSpec>, DispatchError>;

        /// Invoke one command (`command` world).
        fn invoke_command(
            &self,
            name: &str,
            argument: &str,
        ) -> Result<CommandEffect, DispatchError>;

        /// `pre-turn`: observe, nothing to return (FR hooks).
        fn on_pre_turn(&self) -> Result<(), DispatchError> {
            Ok(())
        }

        /// `pre-tool-use`: allow, deny, or replace (FR-CORE-10).
        fn on_pre_tool_use(&self, call: &ToolCall) -> Result<HookAction, DispatchError> {
            let _ = call;
            Ok(HookAction::Allow)
        }

        /// `post-tool-use`: observe.
        fn on_post_tool_use(&self, observation: &PostToolObservation) -> Result<(), DispatchError> {
            let _ = observation;
            Ok(())
        }

        /// `post-turn-end`: observe with `ok` or `error`.
        fn on_post_turn_end(&self, status: &str) -> Result<(), DispatchError> {
            let _ = status;
            Ok(())
        }

        /// `attention-required`: observe a reason.
        fn on_attention_required(&self, reason: &str) -> Result<(), DispatchError> {
            let _ = reason;
            Ok(())
        }

        /// `session-close`: observe.
        fn on_session_close(&self) -> Result<(), DispatchError> {
            Ok(())
        }

        /// Force a running call to trap: Wasmtime epoch interruption for
        /// WASM handles (FR-CONC-1), a no-op for native code that shares
        /// the caller's cancellation flag.
        fn interrupt(&self) {}
    }
}

pub use dispatch::ExtensionDispatch;

#[cfg(feature = "host")]
#[allow(missing_docs)] // generated bindings: the WIT files carry the docs
pub mod host {
    //! Host-side bindings for the worlds written so far. Phase 3, 4, and6
    //! add their worlds to this module as they land.

    /// The `tool` world.
    pub mod tool {
        wasmtime::component::bindgen!({ path: "../../wit", world: "tool" });
    }

    /// The `command` world.
    pub mod command {
        wasmtime::component::bindgen!({ path: "../../wit", world: "command" });
    }

    /// The `hooks` world.
    pub mod hooks {
        wasmtime::component::bindgen!({ path: "../../wit", world: "hooks" });
    }
}

#[cfg(test)]
mod tests {
    /// The constants every manifest check compares against.
    #[test]
    fn abi_version_matches_the_manifest_line() {
        assert_eq!(super::ABI_VERSION, "0.1");
        assert_eq!(super::PACKAGE, "lca:ext");
    }
}
