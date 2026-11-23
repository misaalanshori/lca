//! The `lca:ext` contract crate: the normative WIT package under `wit/`
//! plus generated bindings.
//!
//! Extension authors depend on this crate for the guest bindings without
//! pulling in the agent (ADR-0002). The binary enables the `host` feature
//! to get Wasmtime-side bindings for every world.

#![forbid(unsafe_code)]

/// The ABI version this build implements (`major.minor`, per the manifest's
/// `abi` field and the support window in `docs/abi-versioning.md`).
///
/// `0.7` is the line this build opens. The label no longer tracks the product
/// minor - since the 2026-10-05 owner annotation it moves only for a real
/// interface change - and the 2026-10-08 freeze makes that stricter still: a
/// **released** line is frozen, so the next WIT addition opens the next line
/// rather than riding a patch. gh #407's `ui-dialogs.questionnaire` is that
/// addition, which is why this reads `0.7` while `wit/` advertises `@0.7.0`.
///
/// Nothing special-cases the previous line here: [`abi_in_window`] already
/// answers "current and the one before it", so `abi = "0.6"` components keep
/// loading on a 0.7 host with no code change - the window is the rule, not a
/// constant per line. Read that function's comment for why 1.0 stays amnestied.
///
/// This comment has been stale twice (gh #339 finding 8: it said `0.4` and
/// listed the 0.2 train's items, two release trains behind, in the constant that
/// decides what loads), so it names the change that moved it.
pub const ABI_VERSION: &str = "0.7";

/// The WIT package name that crosses every manifest and registry tag.
pub const PACKAGE: &str = "lca:ext";

/// Whether an extension declaring `abi` loads on this host (NFR-19):
/// the current minor line and the one before it, plus the 1.0 freeze
/// line while the 0.x development window is open. The single source
/// for the window rule; the host's manifest check and the catalog
/// filter both answer through it.
///
/// The window is *relative* to [`ABI_VERSION`] on purpose: a line move
/// (`0.6` → `0.7`) needs no edit here, and a hard-coded pair would be the
/// second place to forget. Its two edges: the previous line is accepted only
/// while the current minor is above zero (`0.x` is the development window;
/// `1.0` is the freeze), and `1.0` itself is accepted for the whole `0.x`
/// window because an extension installed against the 1.0-freeze host declares
/// it - the same amnesty the pre-freeze 0.1 line had on a 1.0 host.
pub fn abi_in_window(declared: &str) -> bool {
    fn parse_abi(value: &str) -> Option<(u64, u64)> {
        let (major, minor) = value.split_once('.')?;
        Some((major.parse().ok()?, minor.parse().ok()?))
    }
    let Some((declared_major, declared_minor)) = parse_abi(declared) else {
        return false;
    };
    let Some((current_major, current_minor)) = parse_abi(ABI_VERSION) else {
        return false;
    };
    if declared_major == current_major {
        return declared_minor == current_minor
            || (current_minor > 0 && declared_minor == current_minor - 1);
    }
    // The 1.0 freeze line's amnesty: an extension installed against the
    // 1.0-freeze host declares `1.0`, and it keeps loading while the
    // 0.x development window is open, exactly as the pre-freeze 0.1
    // line kept loading on a 1.0 host.
    (declared_major, declared_minor) == (1, 0) && current_major == 0
}

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
    /// Answer HTTP requests the host's own listener accepted
    /// (`serve-http` world, gh #363, ADR-0049). The direction is
    /// inverted: the host calls in, the guest never binds a socket.
    ServeHttp,
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
pub mod dispatch;

#[cfg(feature = "host")]
pub mod host;

pub use dispatch::{
    DispatchFuture, ExtensionDispatch, MarkdownMessageType, MarkdownTransformContext,
    MarkdownTransformFn,
};

/// Every world's manifest spelling, in vocabulary order.
///
/// **One source for the string↔[`World`] mapping.** Before gh #288 the mapping
/// lived in three places that could disagree without anything noticing: the
/// `match` in `lca-ext-host`'s `WasmExtension::worlds`, the hand-copied
/// `enum` in `schemas/extension-manifest.schema.json`, and the `==` chain in the
/// loader. A world added to one and not the others gave exactly the bug that
/// issue filed — a manifest the schema accepts, the loader accepts, and the
/// dispatcher silently drops.
///
/// The entries are a plain table rather than a `World::name()`/`World::parse()`
/// pair because the validator needs to *enumerate* the vocabulary (to list it in
/// an error and to suggest the nearest match), which a match arm cannot do.
pub const WORLDS: &[(&str, World)] = &[
    ("tool", World::Tool),
    ("tool-catalog", World::ToolCatalog),
    ("command", World::Command),
    ("hooks", World::Hooks),
    ("hooks-message", World::HooksMessage),
    ("hooks-tool-call", World::HooksToolCall),
    ("hooks-tool-result", World::HooksToolResult),
    ("hooks-stream", World::HooksStream),
    ("hooks-settle", World::HooksSettle),
    ("hooks-compaction", World::HooksCompaction),
    ("hooks-cache", World::HooksCache),
    ("hooks-trust", World::HooksTrust),
    ("provider", World::Provider),
    ("compaction", World::Compaction),
    ("context-transform", World::ContextTransform),
    ("ui", World::Ui),
    ("serve-http", World::ServeHttp),
];

/// The manifest spelling of `world`.
#[must_use]
pub fn world_name(world: World) -> &'static str {
    WORLDS
        .iter()
        .find(|(_, candidate)| *candidate == world)
        .map(|(name, _)| *name)
        .unwrap_or("unknown")
}

/// The [`World`] a manifest spelling names, or `None` when it names none.
///
/// Exact match only: a typo is not a synonym. Callers that want a suggestion
/// use [`nearest_world`].
#[must_use]
pub fn parse_world(name: &str) -> Option<World> {
    WORLDS
        .iter()
        .find(|(candidate, _)| *candidate == name)
        .map(|(_, world)| *world)
}

/// The vocabulary entry closest to `name`, when one is close enough to be worth
/// suggesting.
///
/// Edit distance, capped: a suggestion the author cannot see the relationship to
/// is noise, so a name more than a third of the way from its nearest neighbour
/// gets no hint. That keeps the hint useful for the case this issue is about (one
/// transposed letter) and quiet for a name that is simply something else.
#[must_use]
pub fn nearest_world(name: &str) -> Option<&'static str> {
    let mut best: Option<(usize, &'static str)> = None;
    for (candidate, _) in WORLDS {
        let distance = edit_distance(name, candidate);
        // Allow a third of the longer name's length: enough for a transposition
        // or one wrong character, not enough to suggest an unrelated world.
        let budget = candidate.len().max(name.len()) / 3 + 1;
        if distance <= budget && best.is_none_or(|(best_distance, _)| distance < best_distance) {
            best = Some((distance, candidate));
        }
    }
    best.map(|(_, candidate)| candidate)
}

/// Levenshtein distance, two rows, iterative.
///
/// Written out rather than pulled in: it is fifteen lines and the workspace does
/// not carry a string-distance crate.
fn edit_distance(left: &str, right: &str) -> usize {
    let left: Vec<char> = left.chars().collect();
    let right: Vec<char> = right.chars().collect();
    if left.is_empty() {
        return right.len();
    }
    if right.is_empty() {
        return left.len();
    }
    let mut previous: Vec<usize> = (0..=right.len()).collect();
    let mut current = vec![0usize; right.len() + 1];
    for (i, left_char) in left.iter().enumerate() {
        current[0] = i + 1;
        for (j, right_char) in right.iter().enumerate() {
            let cost = usize::from(left_char != right_char);
            current[j + 1] = (previous[j] + cost)
                .min(previous[j + 1] + 1)
                .min(current[j] + 1);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[right.len()]
}
