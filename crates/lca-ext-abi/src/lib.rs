//! The `lca:ext` contract crate: the normative WIT package under `wit/`
//! plus generated bindings.
//!
//! Extension authors depend on this crate for the guest bindings without
//! pulling in the agent (ADR-0002). The binary enables the `host` feature
//! to get Wasmtime-side bindings for every world.

#![forbid(unsafe_code)]

/// The ABI version this build implements (`major.minor`, per the manifest's
/// `abi` field and the support window in docs/abi-versioning.md).
///
/// `0.4` is the line the ADR-0028 development window's next release train
/// carries: the ABI label tracks the product minor, so `lca 0.4.y` ships
/// `abi 0.4` (docs/abi-versioning.md, ADR-0028's second annotation). The
/// in-place 0.2 changes (typed image content, `provider-login`, the
/// `list-models` settings parameter) are the train's breaking items.
pub const ABI_VERSION: &str = "0.6";

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
    /// Turn a candidate range of the session into a replacement summary
    /// (`compaction` world, ADR-0015, FR-SESS-5).
    Compaction,
    /// Reshape the outgoing message list, or reject the turn
    /// (`context-transform` world, ADR-0015, FR-CTX-2/3).
    ContextTransform,
    /// Draw in the four regions as a widget tree (`ui` world,
    /// ADR-0003, FR-UI-1/2/6).
    Ui,
    /// Register a suite of tools with exposure and namespaces
    /// (`tool-catalog` world, gh #77).
    ToolCatalog,
    /// Observe and replace finalized messages (`hooks-message`
    /// world, gh #45).
    HooksMessage,
    /// Mutate or block tool calls compositionally (`hooks-tool-call`
    /// world, gh #45).
    HooksToolCall,
    /// Mutate tool results compositionally (`hooks-tool-result`
    /// world, gh #45).
    HooksToolResult,
    /// Observe normalized provider stream events (`hooks-stream`
    /// world, gh #45).
    HooksStream,
    /// Append entries and continue once before settling
    /// (`hooks-settle` world, gh #45).
    HooksSettle,
    /// Veto compaction and observe its failure (`hooks-compaction`
    /// world, gh #45).
    HooksCompaction,
    /// Vote on prompt-cache warming (`hooks-cache` world, gh #45).
    HooksCache,
    /// Vote on project trust (`hooks-trust` world, gh #45).
    HooksTrust,
}

/// One nested tool request (gh #77): a tool calling another through
/// the host. The turn serves these on its own task while the parent
/// tool's thread waits; the reply carries a result, never a
/// rejection (unknown tools, validation errors, and blocks all
/// arrive as error results, pi's `isError`).
#[derive(Debug)]
pub struct NestedCall {
    /// The calling tool's call id; the child id becomes
    /// `<parent>/<n>`.
    pub parent_call_id: String,
    /// The tool to run.
    pub name: String,
    /// Argument string (JSON object text).
    pub arguments: String,
    /// Where the outcome goes.
    pub reply: std::sync::mpsc::Sender<lca_protocol::ToolResult>,
}

/// The registry surface the `tools` host import needs (gh #77):
/// listing, activation, callability, and the per-turn nested slot.
/// Implemented by the core registry; the host holds it behind this
/// trait so neither crate depends on the other.
pub trait ToolsRegistryView: Send + Sync {
    /// Callable tools as name plus description, sorted.
    fn callable_names(&self) -> Vec<(String, String)>;
    /// Whether the host may run a tool by name right now.
    fn is_callable(&self, name: &str) -> bool;
    /// The active tool names, sorted.
    fn active_tools(&self) -> Vec<String>;
    /// Replace the active set; returns `(applied, ignored)`.
    fn set_active_tools(&self, names: &[String]) -> (Vec<String>, Vec<String>);
    /// Install the turn's nested-call server.
    fn install_nested(&self, tx: tokio::sync::mpsc::UnboundedSender<NestedCall>);
    /// Remove the turn's nested-call server.
    fn clear_nested(&self);
    /// The installed server, if a turn is running.
    fn nested_slot(&self) -> Option<tokio::sync::mpsc::UnboundedSender<NestedCall>>;
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
        ChatMessage, CommandEffect, CommandSpec, CompactVerdict, CompletionRequest, DispatchError,
        EventSink, HookAction, IdentityOutcome, LoginAnswer, LoginOption, ModelInfo,
        PostToolObservation, Record, SettleDecision, ToolCall, ToolCallPatch, ToolResult,
        ToolResultPatch, ToolSpec, TrustVote, Usage,
    };

    /// Boxed future bound for dispatch calls, tied to the handle's life.
    pub type DispatchFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

    /// Whose markdown a pre-parse transform sees (gh #12): pi's
    /// `MarkdownTransformContext["messageType"]`, Rust-side only.
    /// Reasoning runs render as plain wrapped lines rather than parsed
    /// markdown, so there is no thinking variant.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum MarkdownMessageType {
        /// A user prompt's markdown.
        User,
        /// An assistant answer's markdown.
        Assistant,
    }

    /// What a pre-parse markdown transform sees alongside the source
    /// (gh #12): pi's `MarkdownTransformContext`, Rust-side only. The
    /// host converts it from the render pipeline's own context; a WIT
    /// export ships with the first third-party-shaped consumer, not here.
    #[derive(Debug, Clone)]
    pub struct MarkdownTransformContext {
        /// Whose markdown this is.
        pub message_type: MarkdownMessageType,
        /// The message is still streaming.
        pub is_streaming: bool,
        /// The width the render was asked for, in columns.
        pub available_width: usize,
    }

    /// A pre-parse markdown transform (gh #12): pi's
    /// `registerMarkdownTransformer` as a Rust closure. Runs in
    /// registration order over the raw source before parsing; a
    /// transform that panics behaves as identity (pi's try/catch).
    pub type MarkdownTransformFn =
        std::sync::Arc<dyn Fn(&str, &MarkdownTransformContext) -> String + Send + Sync>;

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

        /// The handle's `extension.toml` text (gh #157): the host reads
        /// manifest-declared provider needs (default hosts, the login
        /// env override) off whichever handle answers. `None` (the
        /// default) means the host falls back to the installed package's
        /// manifest file, if any. Native handles return their compiled
        /// manifest; WASM handles return the text they loaded with.
        fn manifest_text(&self) -> Option<String> {
            None
        }

        /// Install the tool registry surface (gh #77): the running
        /// turn calls this per handle so the `tools` import serves
        /// through it. The default ignores (handles without tool
        /// worlds never need it).
        fn set_tools_view(&self, _view: std::sync::Arc<dyn crate::ToolsRegistryView>) {}

        /// A pre-parse markdown transform (gh #12): pi's
        /// `registerMarkdownTransformer`, Rust-side only. The host runs
        /// it in registration order over the raw markdown source before
        /// parsing; `None` (the default) contributes nothing. Native
        /// handles only for now - the WIT export ships with the first
        /// third-party-shaped consumer, so the world stays as-is.
        fn markdown_transformer(&self) -> Option<MarkdownTransformFn> {
            None
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

        /// `message_end` replacement text (`hooks-message` world, gh
        /// #45). `None` observes. The default implements nothing.
        fn on_message_end<'a>(
            &'a self,
            _role: &'a str,
            _text: &'a str,
        ) -> DispatchFuture<'a, Result<Option<String>, DispatchError>> {
            Box::pin(std::future::ready(Ok(None)))
        }

        /// Composable `tool_call` mutation (`hooks-tool-call` world, gh
        /// #45). The default is the identity patch (observation).
        fn on_tool_call<'a>(
            &'a self,
            _call: &'a ToolCall,
        ) -> DispatchFuture<'a, Result<ToolCallPatch, DispatchError>> {
            Box::pin(std::future::ready(Ok(ToolCallPatch::default())))
        }

        /// Composable `tool_result` mutation (`hooks-tool-result`
        /// world, gh #45). The default is the identity patch.
        fn on_tool_result<'a>(
            &'a self,
            _call: &'a ToolCall,
            _result: &'a ToolResult,
        ) -> DispatchFuture<'a, Result<ToolResultPatch, DispatchError>> {
            Box::pin(std::future::ready(Ok(ToolResultPatch::default())))
        }

        /// Normalized provider stream observation (`hooks-stream`
        /// world, gh #45). The default ignores.
        fn on_stream_event<'a>(
            &'a self,
            _provider: &'a str,
            _model: &'a str,
            _kind: &'a str,
            _data: &'a str,
        ) -> DispatchFuture<'a, Result<(), DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }

        /// Actionable turn end (`hooks-settle` world, gh #45). The
        /// default settles.
        fn on_turn_end<'a>(
            &'a self,
            _rounds: u32,
            _tool_calls: u32,
            _status: &'a str,
        ) -> DispatchFuture<'a, Result<SettleDecision, DispatchError>> {
            Box::pin(std::future::ready(Ok(SettleDecision::default())))
        }

        /// The last word before settlement (`hooks-settle` world, gh
        /// #45). The default settles.
        fn on_agent_before_settle<'a>(
            &'a self,
            _rounds: u32,
            _tool_calls: u32,
            _status: &'a str,
        ) -> DispatchFuture<'a, Result<SettleDecision, DispatchError>> {
            Box::pin(std::future::ready(Ok(SettleDecision::default())))
        }

        /// Compaction veto (`hooks-compaction` world, gh #45). The
        /// default allows.
        fn on_session_before_compact<'a>(
            &'a self,
            _reason: &'a str,
        ) -> DispatchFuture<'a, Result<CompactVerdict, DispatchError>> {
            Box::pin(std::future::ready(Ok(CompactVerdict::Allow)))
        }

        /// Compaction failure observation (`hooks-compaction` world, gh
        /// #45). The default ignores.
        fn on_session_compact_failed<'a>(
            &'a self,
            _reason: &'a str,
            _error: Option<&'a str>,
        ) -> DispatchFuture<'a, Result<(), DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }

        /// Cache-warming vote (`hooks-cache` world, gh #45). The
        /// default warms.
        fn on_cache_warming_decision<'a>(
            &'a self,
            _provider: &'a str,
            _model: &'a str,
        ) -> DispatchFuture<'a, Result<bool, DispatchError>> {
            Box::pin(std::future::ready(Ok(true)))
        }

        /// Project-trust vote (`hooks-trust` world, gh #45). Returns
        /// the vote and whether to remember it. The default is
        /// undecided (the operator decides).
        fn on_project_trust<'a>(
            &'a self,
            _cwd: &'a str,
        ) -> DispatchFuture<'a, Result<(TrustVote, bool), DispatchError>> {
            Box::pin(std::future::ready(Ok((TrustVote::Undecided, false))))
        }

        /// Models this provider offers (`provider` world, FR-PROV-2).
        /// Synchronous like `tool_specs`: the picker reads it at
        /// registration time. The default answers `MissingWorld`, so a
        /// handle without the provider world is rejected rather than
        /// silently empty.
        fn provider_models(
            &self,
            _settings: &[(String, String)],
        ) -> Result<Vec<ModelInfo>, DispatchError> {
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

        /// The provider's login options (ADR-0033). Default: none, so the
        /// host's picker shows only its own "Custom endpoint…".
        fn login_options(
            &self,
        ) -> DispatchFuture<'static, Result<Vec<LoginOption>, DispatchError>> {
            Box::pin(std::future::ready(Ok(Vec::new())))
        }

        /// Consume the user's answers (ADR-0033): store the secret in the
        /// extension's own credentials namespace, return opaque settings
        /// for the host to persist. Default: nothing to do.
        fn login_submit(
            &self,
            _answer: LoginAnswer,
        ) -> DispatchFuture<'static, Result<Vec<(String, String)>, DispatchError>> {
            Box::pin(std::future::ready(Ok(Vec::new())))
        }

        /// Deliver a manually pasted OAuth callback to the flow this
        /// provider is waiting on (R4's manual fallback). The default:
        /// this provider has no OAuth flow to deliver to.
        fn oauth_manual_callback(
            &self,
            _params: Vec<(String, String)>,
        ) -> Result<(), DispatchError> {
            Ok(())
        }

        /// The most recent auth URL this provider asked the host to open,
        /// for the interface to display when the auto-open fails (R3).
        /// Default: none.
        fn oauth_last_url(&self) -> Option<String> {
            None
        }

        /// The regions this handle registered under `capabilities.ui`
        /// (the host only ever asks for these: the catalog's `ui`
        /// table, deny-by-default). Empty by default - no rendering.
        fn ui_regions(&self) -> Vec<String> {
            Vec::new()
        }

        /// The tree for one region (`ui` world, ADR-0003, FR-UI-1).
        /// Synchronous on purpose: this runs on a frame's clock, and a
        /// native handle answers immediately while a WASM handle joins
        /// its blocking call (registration-path pattern). `Ok(None)`
        /// means nothing to draw for that region, which is also how an
        /// ungranted ask is declined - the host never asks outside
        /// `ui_regions`, so a hostile extension simply is not called
        /// (capability catalog: its render export is never invoked).
        fn render(&self, _region: &str) -> Result<Option<lca_protocol::WidgetTree>, DispatchError> {
            Ok(None)
        }

        /// One user interaction in, one effect out (`ui` world,
        /// FR-UI-6): the host delivers these only in response to real
        /// user input, which is what makes "no modal without the user"
        /// enforceable host-side.
        fn on_ui_event(
            &self,
            _region: &str,
            _input: &lca_protocol::UiInput,
        ) -> Result<lca_protocol::UiEffect, DispatchError> {
            Ok(lca_protocol::UiEffect::None)
        }

        /// Compact a candidate range of the session into a replacement
        /// summary (`compaction` world, FR-SESS-5/FR-CTX-1). The host
        /// writes what comes back as a durable record and reuses it on
        /// later reads; an error means no record and no compaction.
        fn compact(
            &self,
            _records: &[Record],
        ) -> DispatchFuture<'static, Result<String, DispatchError>> {
            Box::pin(std::future::ready(Err(DispatchError::MissingWorld {
                extension: self.name().to_string(),
                world: "compaction",
            })))
        }

        /// Reshape the outgoing messages (`context-transform` world,
        /// FR-CTX-2). `Err(reason)` is a rejection: the host ends the
        /// turn with that reason and never calls the provider
        /// (FR-CTX-3). The default passes the list through unchanged;
        /// only handles whose `worlds` include `ContextTransform` are
        /// ever asked.
        fn transform_messages(
            &self,
            messages: Vec<ChatMessage>,
        ) -> DispatchFuture<'static, Result<Result<Vec<ChatMessage>, String>, DispatchError>>
        {
            // The outer Result separates a host-level failure (trap,
            // disabled) from the extension's own verdict; the inner is
            // the transformed list or the rejection reason (FR-CTX-3).
            Box::pin(std::future::ready(Ok(Ok(messages))))
        }

        /// Force a running call to trap: Wasmtime epoch interruption for
        /// WASM handles (FR-CONC-1), a no-op for native code that shares
        /// the caller's cancellation flag.
        fn interrupt(&self) {}

        /// Start-of-turn bookkeeping on the host side: clear the
        /// cancellation `interrupt` left behind, so the next turn's first
        /// call is not pre-cancelled by the previous turn's cancel
        /// (FR-CONC-1). Called once per turn, before that turn's work.
        fn turn_started(&self) {}
    }
}

pub use dispatch::{
    DispatchFuture, ExtensionDispatch, MarkdownMessageType, MarkdownTransformContext,
    MarkdownTransformFn,
};

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

    /// The `compaction` world.
    pub mod compaction {
        wasmtime::component::bindgen!({ path: "../../wit", world: "compaction" });
    }

    /// The `context-transform` world.
    pub mod context_transform {
        wasmtime::component::bindgen!({ path: "../../wit", world: "context-transform" });
    }

    /// The `ui` world.
    pub mod ui {
        wasmtime::component::bindgen!({ path: "../../wit", world: "ui" });
    }

    /// The `tool-catalog` world (gh #77): multi-tool registration.
    /// The `tool` module above stays the single-tool world.
    pub mod tool_catalog {
        wasmtime::component::bindgen!({ path: "../../wit", world: "tool-catalog" });
    }

    /// The `hooks-message` world (gh #45): `message_end` replace.
    pub mod hooks_message {
        wasmtime::component::bindgen!({ path: "../../wit", world: "hooks-message" });
    }

    /// The `hooks-tool-call` world (gh #45): composable mutation.
    pub mod hooks_tool_call {
        wasmtime::component::bindgen!({ path: "../../wit", world: "hooks-tool-call" });
    }

    /// The `hooks-tool-result` world (gh #45): composable results.
    pub mod hooks_tool_result {
        wasmtime::component::bindgen!({ path: "../../wit", world: "hooks-tool-result" });
    }

    /// The `hooks-stream` world (gh #45): stream observation.
    pub mod hooks_stream {
        wasmtime::component::bindgen!({ path: "../../wit", world: "hooks-stream" });
    }

    /// The `hooks-settle` world (gh #45): actionable settle.
    pub mod hooks_settle {
        wasmtime::component::bindgen!({ path: "../../wit", world: "hooks-settle" });
    }

    /// The `hooks-compaction` world (gh #45): compact veto.
    pub mod hooks_compaction {
        wasmtime::component::bindgen!({ path: "../../wit", world: "hooks-compaction" });
    }

    /// The `hooks-cache` world (gh #45): cache-warming votes.
    pub mod hooks_cache {
        wasmtime::component::bindgen!({ path: "../../wit", world: "hooks-cache" });
    }

    /// The `hooks-trust` world (gh #45): project-trust votes.
    pub mod hooks_trust {
        wasmtime::component::bindgen!({ path: "../../wit", world: "hooks-trust" });
    }
}
