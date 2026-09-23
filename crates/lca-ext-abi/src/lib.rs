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
    /// Offer models, streamed completions, and account identity
    /// (`provider` world, ADR-0004/ADR-0012).
    Provider,
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
///
/// Shape: registration-time metadata and input-editor invocations are
/// synchronous (they run before or outside the turn's async path), while
/// tool execution and hooks return boxed futures, because a component
/// call runs through `spawn_blocking` and synchronous WASI only works
/// off the runtime's poll path (ADR-0014: WASM calls share the Tokio
/// runtime through blocking-pool execution, not per-call OS threads).
pub mod dispatch {
    use core::future::Future;
    use std::pin::Pin;

    use crate::{DeliveryMode, World};
    use lca_protocol::{
        CommandEffect, CommandSpec, CompletionRequest, DispatchError, EventSink, HookAction,
        IdentityOutcome, ModelInfo, PostToolObservation, ToolCall, ToolResult, ToolSpec, Usage,
    };

    /// Boxed future bound for dispatch calls, tied to the handle's life.
    pub type DispatchFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

    /// A loaded extension, whichever way it is delivered.
    pub trait ExtensionDispatch: Send + Sync {
        /// The extension's identity (its manifest name).
        fn name(&self) -> &str;

        /// Sandboxed or in-process (FR-EXT-7's data).
        fn delivery(&self) -> DeliveryMode;

        /// Worlds this handle implements.
        fn worlds(&self) -> Vec<World>;

        /// Tool specs (`tool` world). Registration-time only: a WASM
        /// handle runs one blocking component call and joins it.
        fn tool_specs(&self) -> Result<Vec<ToolSpec>, DispatchError>;

        /// Run one tool call (`tool` world).
        fn execute_tool<'a>(
            &'a self,
            call: &'a ToolCall,
        ) -> DispatchFuture<'a, Result<ToolResult, DispatchError>>;

        /// Registered commands (`command` world), registration-time.
        fn command_specs(&self) -> Result<Vec<CommandSpec>, DispatchError>;

        /// Invoke one command (`command` world). Runs on the input
        /// editor's thread, outside any async context.
        fn invoke_command(
            &self,
            name: &str,
            argument: &str,
        ) -> Result<CommandEffect, DispatchError>;

        /// Reserved built-in slash slots this handle fills (SRDD's
        /// built-in command list). Only first-party *native* handles may
        /// return names here — the WASM implementation of this trait
        /// hardcodes the default, and a manifest cannot ask for it; this
        /// is how a built-in behavior moves onto the extension path
        /// while the user-facing name stays put (ADR-0019).
        fn builtin_command_slots(&self) -> Vec<String> {
            Vec::new()
        }

        /// `pre-turn`: observe.
        fn on_pre_turn(&self) -> DispatchFuture<'static, Result<(), DispatchError>>;

        /// `pre-tool-use`: allow, deny, or replace (FR-CORE-10).
        fn on_pre_tool_use<'a>(
            &'a self,
            call: &'a ToolCall,
        ) -> DispatchFuture<'a, Result<HookAction, DispatchError>>;

        /// `post-tool-use`: observe.
        fn on_post_tool_use<'a>(
            &'a self,
            observation: &'a PostToolObservation,
        ) -> DispatchFuture<'a, Result<(), DispatchError>>;

        /// `post-turn-end`: observe with `ok` or `error`.
        fn on_post_turn_end<'a>(
            &'a self,
            status: &'a str,
        ) -> DispatchFuture<'a, Result<(), DispatchError>>;

        /// `attention-required`: observe a reason.
        fn on_attention_required<'a>(
            &'a self,
            reason: &'a str,
        ) -> DispatchFuture<'a, Result<(), DispatchError>>;

        /// `session-close`: observe.
        fn on_session_close(&self) -> DispatchFuture<'static, Result<(), DispatchError>>;

        /// Models this provider offers (`provider` world, FR-PROV-2).
        /// Synchronous like `tool_specs`: the picker reads it at
        /// registration time. The default answers `MissingWorld`, so a
        /// handle without the provider world is rejected rather than
        /// silently empty.
        fn provider_models(&self) -> Result<Vec<ModelInfo>, DispatchError> {
            Err(DispatchError::MissingWorld {
                extension: self.name().to_string(),
                world: "provider",
            })
        }

        /// Stream one completion (`provider` world). Events are pushed
        /// into `sink` as they arrive; `sink.push` returning `false`
        /// cancels the stream (FR-CONC-3). Runs through the host's
        /// blocking-pool bridge for WASM handles (ADR-0014).
        fn stream_completion<'a>(
            &'a self,
            _request: CompletionRequest,
            _sink: &'a dyn EventSink,
        ) -> DispatchFuture<'a, Result<(), DispatchError>> {
            Box::pin(std::future::ready(Err(DispatchError::MissingWorld {
                extension: self.name().to_string(),
                world: "provider",
            })))
        }

        /// `login` (`provider` world, ADR-0012). The default: this
        /// provider has no login (the optional-export rule).
        fn identity_login(
            &self,
        ) -> DispatchFuture<'static, Result<IdentityOutcome, DispatchError>> {
            Box::pin(std::future::ready(Ok(IdentityOutcome::NotSupported)))
        }

        /// `logout` (`provider` world, ADR-0012).
        fn identity_logout(
            &self,
        ) -> DispatchFuture<'static, Result<IdentityOutcome, DispatchError>> {
            Box::pin(std::future::ready(Ok(IdentityOutcome::NotSupported)))
        }

        /// `usage` (`provider` world, ADR-0012): the standard usage shape,
        /// or the outcome variant for a provider without one. The outer
        /// `Result` separates host-level failures (trap, disabled) from
        /// the provider's own outcome.
        fn identity_usage(
            &self,
        ) -> DispatchFuture<'static, Result<Result<Usage, IdentityOutcome>, DispatchError>>
        {
            Box::pin(std::future::ready(Ok(Err(IdentityOutcome::NotSupported))))
        }

        /// Force a running call to trap: Wasmtime epoch interruption for
        /// WASM handles (FR-CONC-1), a no-op for native code that shares
        /// the caller's cancellation flag.
        fn interrupt(&self) {}
    }
}

pub use dispatch::{DispatchFuture, ExtensionDispatch};

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

    /// The `provider` world (streaming, identity, model listing).
    pub mod provider {
        wasmtime::component::bindgen!({ path: "../../wit", world: "provider" });
    }
}
