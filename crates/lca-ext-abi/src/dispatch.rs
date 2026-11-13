//! The dispatch contract every handle implements (gh #551: split out of
//! `lib.rs`, which held it inline as a 524-line `pub mod dispatch`).

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

/// Whose markdown a pre-parse transform sees (gh #12, gh #390 gap 2):
/// pi's `MarkdownTransformContext["messageType"]`, Rust-side only.
///
/// `AssistantThinking` is pi's `assistant-thinking`: a **reasoning run**,
/// which is the model drafting to itself rather than the answer it stands
/// behind. A transformer that produces an artefact (mermaid, and anything
/// similar later) skips that type, so a diagram sketched in reasoning is not
/// drawn twice.
///
/// Additive and ABI-internal: no `wit/` declaration carries this type (the
/// WIT export ships with the first third-party-shaped consumer), so a new
/// variant breaks no guest. Every in-tree adapter is forced to handle it by
/// the compiler, which is the guard that matters - a `_ =>` arm is what would
/// silently mislabel a reasoning run as an answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkdownMessageType {
    /// A user prompt's markdown.
    User,
    /// An assistant answer's markdown.
    Assistant,
    /// A reasoning run's markdown (gh #390 gap 2): pi's
    /// `assistant-thinking`.
    AssistantThinking,
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

    /// Tool specs (`tool` world). Read at registration and again whenever
    /// this handle's [`specs_version`](ExtensionDispatch::specs_version)
    /// moves (gh #584). A WASM handle runs one blocking component call and
    /// joins it.
    fn tool_specs(&self) -> Result<Vec<ToolSpec>, DispatchError>;

    /// A version that moves whenever this handle's **tool specs may have
    /// changed**, or `None` for "my specs never change" (gh #584).
    ///
    /// `None` is the default on purpose: it keeps every handle whose specs
    /// are fixed at registration - every WASM guest, every in-tree
    /// extension - on the path they already had, with no re-plan and no
    /// allocation. Only a handle that discovers tools *after* registration
    /// (the MCP bridge, whose servers land on background workers and whose
    /// rows change on every `/mcp` edit, reconnect and shutdown) answers
    /// `Some`.
    ///
    /// It is a **version**, never an identity: the registry re-plans only
    /// when the value differs from the one it planned from last, so a
    /// handle that bumps on every call would bust the prompt cache every
    /// turn. Bump it on change, not on read.
    ///
    /// Host-side only. This is not a `wit/` export: it is the host asking
    /// its own handles a question, so a guest neither sees it nor is
    /// required to answer it.
    fn specs_version(&self) -> Option<u64> {
        None
    }

    /// Run one tool call (`tool` world).
    fn execute_tool<'a>(
        &'a self,
        call: &'a ToolCall,
    ) -> DispatchFuture<'a, Result<ToolResult, DispatchError>>;

    /// Registered commands (`command` world), registration-time.
    fn command_specs(&self) -> Result<Vec<CommandSpec>, DispatchError>;

    /// Invoke one command (`command` world). Runs on the input
    /// editor's thread, outside any async context.
    fn invoke_command(&self, name: &str, argument: &str) -> Result<CommandEffect, DispatchError>;

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
    ///
    /// **The six `hooks` methods below have default bodies on purpose**
    /// (gh #578). They are exported only by the `hooks` world
    /// (`wit/world-hooks.wit`), and the host dispatches each one only to a
    /// handle whose `worlds()` declares `Hooks`
    /// (`ExtensionRegistry`'s per-hook filters). While they were *required*,
    /// the compiler forced every implementor - including a `ui`-only handle
    /// like the reference `ui-example` - to write a body the host could never
    /// call. `ui-example` did exactly that and shipped a dead
    /// `on_session_close` under a comment claiming it was "the correct half
    /// of the contract", which is the trap: a required method reads as a live
    /// obligation.
    ///
    /// Defaulted, an undeclared world simply inherits the observation no-op,
    /// which is what the dispatch filter already means. A `hooks` implementor
    /// overrides what it needs, exactly as the `gh #45` worlds below already
    /// work.
    fn on_pre_turn(&self) -> DispatchFuture<'static, Result<(), DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }

    /// `pre-tool-use`: allow, deny, or replace (FR-CORE-10).
    ///
    /// The default allows, matching the dispatch filter's answer for a handle
    /// that never declared `hooks`.
    fn on_pre_tool_use<'a>(
        &'a self,
        _call: &'a ToolCall,
    ) -> DispatchFuture<'a, Result<HookAction, DispatchError>> {
        Box::pin(std::future::ready(Ok(HookAction::Allow)))
    }

    /// `post-tool-use`: observe.
    fn on_post_tool_use<'a>(
        &'a self,
        _observation: &'a PostToolObservation,
    ) -> DispatchFuture<'a, Result<(), DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }

    /// `post-turn-end`: observe with `ok` or `error`.
    fn on_post_turn_end<'a>(
        &'a self,
        _status: &'a str,
    ) -> DispatchFuture<'a, Result<(), DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }

    /// `attention-required`: observe a reason.
    fn on_attention_required<'a>(
        &'a self,
        _reason: &'a str,
    ) -> DispatchFuture<'a, Result<(), DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }

    /// `session-close`: observe.
    ///
    /// **Not guaranteed to run** (gh #578): it fires only for a handle that
    /// declared `hooks`, and even then a crash or `kill -9` skips it. A guest
    /// that owns a child process must still handle its own teardown; the
    /// reference `ui-example` does it in `Drop`, which is the expectation
    /// `docs/extension-authoring.md` states.
    fn on_session_close(&self) -> DispatchFuture<'static, Result<(), DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }

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
    fn identity_login(&self) -> DispatchFuture<'static, Result<IdentityOutcome, DispatchError>> {
        Box::pin(std::future::ready(Ok(IdentityOutcome::NotSupported)))
    }

    /// `logout` (`provider` world, ADR-0012).
    fn identity_logout(&self) -> DispatchFuture<'static, Result<IdentityOutcome, DispatchError>> {
        Box::pin(std::future::ready(Ok(IdentityOutcome::NotSupported)))
    }

    /// `usage` (`provider` world, ADR-0012): the standard usage shape,
    /// or the outcome variant for a provider without one. The outer
    /// `Result` separates host-level failures (trap, disabled) from
    /// the provider's own outcome.
    fn identity_usage(
        &self,
    ) -> DispatchFuture<'static, Result<Result<Usage, IdentityOutcome>, DispatchError>> {
        Box::pin(std::future::ready(Ok(Err(IdentityOutcome::NotSupported))))
    }

    /// The provider's login options (ADR-0033). Default: none, so the
    /// host's picker shows only its own "Custom endpoint…".
    fn login_options(&self) -> DispatchFuture<'static, Result<Vec<LoginOption>, DispatchError>> {
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
    fn oauth_manual_callback(&self, _params: Vec<(String, String)>) -> Result<(), DispatchError> {
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

    /// This handle's `serve-http` mount, when it declared the capability
    /// (gh #363, ADR-0049). `None` means "no route": the session mounts
    /// only what this returns, so an extension without the grant is absent
    /// from the port rather than reachable-but-refusing.
    ///
    /// The trait object is the protocol crate's, not this crate's, because
    /// the listener lives in `lca-tools` and the contract at the bottom is
    /// what lets the two name each other (see `lca_protocol::http`).
    fn serve_http_mount(
        &self,
        _session: &str,
    ) -> Option<std::sync::Arc<dyn lca_protocol::http::ServeHttpNamespace>> {
        None
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
    ///
    /// # Why the two `Result` layers stay (gh #551, answered by measurement)
    ///
    /// The nesting is the shape the ABI has, not an accident of this trait:
    /// `wit/world-context-transform.wit` exports
    /// `transform: func(messages) -> result<list<message>, string>`, and every
    /// host import is reached through a `Result` of its own (a trap, a disabled
    /// extension, a missing world). So the *outer* `DispatchError` is a **host
    /// transport** fact and the *inner* is the **extension's verdict**; they are
    /// different worlds' worth of failure and a caller acts on them differently
    /// (the inner reason is shown to the user under FR-CTX-3, the outer stops the
    /// turn as an internal error).
    ///
    /// Flattening it into one enum (`TransformVerdict { Transformed, Rejected }`
    /// inside a single `Result`) is a **change to a published crate's public
    /// trait**, which every one of the eleven `context-transform` extension crates
    /// implements - i.e. an ABI-line decision (0.7), not a hygiene edit. Measured
    /// for this cycle: the enum alone compiles, and then eleven crates in
    /// `extensions/**` fail with `E0053 incompatible type for trait`. Declined
    /// here; `docs/extension-authoring.md` records the layering so a later cycle
    /// that owns the ABI line can make the change with its own receipt.
    fn transform_messages(
        &self,
        messages: Vec<ChatMessage>,
    ) -> DispatchFuture<'static, Result<Result<Vec<ChatMessage>, String>, DispatchError>> {
        // The outer Result separates a host-level failure (trap,
        // disabled) from the extension's own verdict; the inner is
        // the transformed list or the rejection reason (FR-CTX-3).
        Box::pin(std::future::ready(Ok(Ok(messages))))
    }

    /// Force a running call to trap: Wasmtime epoch interruption for
    /// WASM handles (FR-CONC-1), a no-op for native code that shares
    /// the caller's cancellation flag.
    fn interrupt(&self) {}

    /// Install the turn's nested-completion backend (#49): the turn
    /// wraps it in `lca_tools::SharedBackend` because this contract
    /// cannot name the backend type. Hosts with a capability engine
    /// install where empty (compaction keeps its own); the default
    /// ignores it, so handles without that machinery keep today's
    /// shape (their tools cannot complete, exactly as before).
    fn set_completion_backend(&self, _backend: &std::sync::Arc<dyn std::any::Any + Send + Sync>) {}

    /// Start-of-turn bookkeeping on the host side: clear the
    /// cancellation `interrupt` left behind, so the next turn's first
    /// call is not pre-cancelled by the previous turn's cancel
    /// (FR-CONC-1). Called once per turn, before that turn's work.
    fn turn_started(&self) {}
}
