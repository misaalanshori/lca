//! Time-traveling stream rules, observe-only (gh #194): regex rules
//! scan the provider's streaming deltas; a match fires telemetry,
//! never steering. The hooks-stream contract is post-close
//! observation ("observation never steers or stalls a live stream"),
//! so mid-token cancellation and steer-queue reminder injection wait
//! on a host affordance that does not exist yet — the code marks the
//! SEAM, and the matcher below is the half that survives unchanged
//! when it lands.
//!
//! Rules live in `rules.toml`, project before global:
//! `.lca/extensions/stream-guard/rules.toml` under the `workspace`
//! scope (active only when the project is trusted), and `rules.toml`
//! under the `private` scope (`<data>/private/stream-guard/`). The
//! issue's literal `~/.lca/...` global path sits inside the agent
//! state directory, which no scope reaches: a scope resolves only
//! inside its own root, and the two roots that *do* live under the
//! state directory are `private` (`<data>/private/<name>`) and the
//! active session's `temp` (`<data>/tmp/<session>`), each confined to
//! itself. `private` is the vocabulary-legal equivalent, as in
//! project-memory.
//!
//! Matching uses the `regex` crate, which guarantees linear-time
//! matching: a hostile or sloppy pattern from a rule file cannot
//! backtrack the turn into oblivion. The window is bounded for the
//! same reason.
//!
//! # Unsafe-code exemption
//!
//! The `wasm32` halves carry generated `wit-bindgen` export shims;
//! the module holds the allowance.

#![deny(unsafe_code)]

/// The manifest this form ships with (single source for
/// [`manifest_grants`]; a test keeps them in step).
pub const MANIFEST: &str = include_str!("../extension.toml");

/// The grants the manifest declares: read on the workspace (project
/// rules) and on the extension's own `private` directory (global
/// rules) - nothing else. Zero network, zero processes.
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::expect_used)] // the extension's own literal manifest patterns are valid by construction.
pub fn manifest_grants() -> lca_tools::CapabilityGrants {
    use lca_permissions::{FsMode, ScopeGrant};
    lca_tools::CapabilityGrants {
        fs: vec![
            ScopeGrant::parse("workspace", FsMode::Read).expect("the manifest's own scope parses"),
            ScopeGrant::parse("private", FsMode::Read).expect("the manifest's own scope parses"),
        ],
        fs_declared: true,
        ..Default::default()
    }
}

/// Project rules, workspace-relative.
pub const PROJECT_RULES_PATH: &str = ".lca/extensions/stream-guard/rules.toml";
/// Global rules, under the `private` scope.
pub const GLOBAL_RULES_PATH: &str = "rules.toml";

/// Sliding-window size in chars: patterns split across deltas still
/// fire, while a runaway stream cannot grow the buffer.
pub const WINDOW_CHARS: usize = 4096;

/// One compiled rule.
#[derive(Debug, Clone)]
pub struct Rule {
    /// The `id` the status line and reports name.
    pub id: String,
    /// The compiled `pattern`.
    pub pattern: regex::Regex,
    /// The `reminder` a future steer injection will carry.
    pub reminder: String,
}

/// One fired rule: the telemetry the observation produces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    /// The rule that fired.
    pub rule: String,
    /// Its reminder text.
    pub reminder: String,
}

/// Parse `rules.toml` (`[[rule]]` tables with `id`, `pattern`,
/// `reminder`). Failures name the rule and the field, never panic:
/// a user's typo must read as a diagnostic, not a trap.
pub fn parse_rules_toml(text: &str) -> Result<Vec<Rule>, String> {
    let value: toml::Value = text
        .parse()
        .map_err(|err| format!("rules.toml does not parse: {err}"))?;
    let raw = value
        .get("rule")
        .and_then(toml::Value::as_array)
        .ok_or_else(|| "rules.toml needs a [[rule]] array".to_string())?;
    let mut rules = Vec::new();
    for (index, entry) in raw.iter().enumerate() {
        let field = |name: &str| {
            entry
                .get(name)
                .and_then(toml::Value::as_str)
                .ok_or_else(|| format!("rule {index} needs a `{name}` string"))
        };
        let id = field("id")?.to_string();
        let pattern = field("pattern")?;
        let reminder = field("reminder")?.to_string();
        let compiled = regex::Regex::new(pattern)
            .map_err(|err| format!("rule `{id}` has a bad pattern: {err}"))?;
        rules.push(Rule {
            id,
            pattern: compiled,
            reminder,
        });
    }
    Ok(rules)
}

/// The filesystem view the loader needs: read through the `fs`
/// capability, so neither mode reaches around it.
pub trait FsView: Send + Sync {
    /// One file's text; `None` when it does not exist.
    fn read(
        &self,
        scope: &str,
        path: &str,
    ) -> Result<Option<String>, lca_protocol::CapabilityError>;
}

/// Load global rules always, project rules only when trusted. A
/// missing file is silent (opt-in means zero-cost absence); a broken
/// file warns through the returned notes and loads nothing from it.
pub fn load_rules(fs: &dyn FsView, project_trusted: bool) -> (Vec<Rule>, Vec<String>) {
    let mut rules = Vec::new();
    let mut warnings = Vec::new();
    let mut load = |scope: &str, path: &str| match fs.read(scope, path) {
        Ok(Some(text)) => match parse_rules_toml(&text) {
            Ok(parsed) => rules.extend(parsed),
            Err(reason) => warnings.push(format!("{scope}:{path}: {reason}")),
        },
        Ok(None) => {}
        Err(err) => warnings.push(format!("{scope}:{path}: {err}")),
    };
    load("private", GLOBAL_RULES_PATH);
    if project_trusted {
        load("workspace", PROJECT_RULES_PATH);
    }
    (rules, warnings)
}

/// The bounded matcher: each `feed` appends one delta's text to the
/// window and reports every rule firing inside it. Kinds without
/// matchable text (`usage`, `vendor-event`, tool-call opens) never
/// fire. The input passes through untouched by construction — this
/// returns matches only, so observation cannot steer.
///
/// **One match, one emit (gh #300).** Each rule remembers the window
/// offset it was last reported up to, and only text *after* that offset
/// is examined. Without it the matched text stayed in the window, so
/// every later delta re-matched it for the next [`WINDOW_CHARS`] — and a
/// match from one turn re-fired on the next turn's first token.
///
/// The bookkeeping is a per-rule cursor, not a suppression window: new
/// text is always examined, so a rule that legitimately matches again
/// fires again. Only the span already reported is skipped.
pub struct Scanner {
    rules: Vec<Rule>,
    window: String,
    /// Per rule, in `rules` order: the window offset its last report
    /// covered. Text before it was already examined and reported.
    scanned: Vec<usize>,
}

impl Scanner {
    /// Bind to a loaded rule set (rules are cheap to move: the
    /// compiled patterns share their automata).
    pub fn new(rules: Vec<Rule>) -> Scanner {
        let scanned = vec![0; rules.len()];
        Scanner {
            rules,
            window: String::new(),
            scanned,
        }
    }

    /// Feed one stream event; the matches firing in the window after
    /// this delta. `kind` is the host's stream vocabulary.
    pub fn feed(&mut self, kind: &str, data: &str) -> Vec<Match> {
        if !matches!(kind, "text-delta" | "reasoning-delta") {
            return Vec::new();
        }
        self.window.push_str(data);
        let trimmed = self.trim_window();
        // Every cursor moves back by what the window dropped, so an
        // offset keeps meaning "this far into the text I have read".
        for cursor in &mut self.scanned {
            *cursor = cursor.saturating_sub(trimmed);
        }
        let mut matches = Vec::new();
        for (index, rule) in self.rules.iter().enumerate() {
            let from = self.scanned[index];
            // A cursor past the end can happen only if a previous trim
            // left it there; clamping keeps the slice sound.
            let from = from.min(self.window.len());
            let region = &self.window[from..];
            // `find`, not `is_match`: the end of the reported span is
            // what advances the cursor, and only a located match has
            // one.
            let Some(found) = rule.pattern.find(region) else {
                continue;
            };
            // Advance past the span that was just reported. Two rules
            // matching the same span keep independent cursors, so each
            // reports once (dedup is per rule, not global).
            self.scanned[index] = from + found.end();
            matches.push(Match {
                rule: rule.id.clone(),
                reminder: rule.reminder.clone(),
            });
        }
        matches
    }

    /// Drop the oldest chars so the window stays bounded; returns how
    /// many bytes were dropped.
    fn trim_window(&mut self) -> usize {
        if self.window.len() <= WINDOW_CHARS {
            return 0;
        }
        let cut = self.window.len() - WINDOW_CHARS;
        let mut at = cut;
        while at < self.window.len() && !self.window.is_char_boundary(at) {
            at += 1;
        }
        self.window.drain(..at);
        at
    }

    /// Start a new turn: forget the previous turn's text and cursors, so
    /// a match from turn N cannot fire on turn N+1's first delta
    /// (gh #300 acceptance 2). The rules themselves stay loaded.
    pub fn reset(&mut self) {
        self.window.clear();
        self.scanned.fill(0);
    }

    /// Buffered chars, for the bounded-window test.
    pub fn buffered(&self) -> usize {
        self.window.len()
    }
}

/// The status-line widget for the last fired rule, or nothing before
/// anything fires. Shared by both delivery modes (the ui-example
/// rule: identical across modes by construction).
pub fn status_widget(last_match: Option<&str>) -> Option<lca_protocol::Widget> {
    last_match.map(|rule| lca_protocol::Widget::Text {
        content: format!("guard: {rule}"),
        role: "warning".to_string(),
    })
}

/// The `guard-scan` tool spec the model sees: scan arbitrary text
/// against the loaded rules without spending a turn.
pub fn guard_scan_tool_spec() -> lca_protocol::ToolSpec {
    lca_protocol::ToolSpec {
        name: "guard-scan".to_string(),
        description: "Scan text against the stream-guard rules; reports the first firing rule or a clean bill."
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "text": { "type": "string", "description": "The text to scan." }
            },
            "required": ["text"]
        }),
        exposure: lca_protocol::ToolExposure::Direct,
        namespace: None,
        annotations: None,
        extras: Default::default(),
    }
}

/// The probe command: no argument lists the rule ids; text scans it
/// and reports the first firing rule or a clean bill.
pub fn run_command(rules: &[Rule], argument: &str) -> String {
    let argument = argument.trim();
    if argument.is_empty() {
        if rules.is_empty() {
            return "stream-guard: no rules loaded".to_string();
        }
        let mut out = String::from("stream-guard rules:");
        for rule in rules {
            out.push_str("\n- ");
            out.push_str(&rule.id);
        }
        return out;
    }
    let mut scanner = Scanner::new(rules.to_vec());
    match scanner.feed("text-delta", argument).first() {
        Some(hit) => format!("stream-guard fired {}: {}", hit.rule, hit.reminder),
        None => "stream-guard: clean".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Native delivery mode
// ---------------------------------------------------------------------------

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use super::*;
    use std::sync::{Arc, Mutex};

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
    }

    /// The native handle. Rules load once at construction (global
    /// always, project when `project_trusted`); the scanner keeps its
    /// window across the session's events, and the last match feeds
    /// the status-line render.
    pub struct StreamGuard {
        cap: Arc<lca_tools::Capabilities>,
        project_trusted: bool,
        rules: Vec<Rule>,
        warnings: Vec<String>,
        scanner: Mutex<Scanner>,
        last_match: Mutex<Option<String>>,
    }

    impl StreamGuard {
        /// Build from the engine and the workspace's trust bit:
        /// global rules always, project rules only when trusted.
        pub fn new(cap: Arc<lca_tools::Capabilities>, project_trusted: bool) -> StreamGuard {
            let (rules, warnings) = load_rules(cap.as_ref(), project_trusted);
            let scanner = Scanner::new(rules.clone());
            StreamGuard {
                cap,
                project_trusted,
                rules,
                warnings,
                scanner: Mutex::new(scanner),
                last_match: Mutex::new(None),
            }
        }

        /// Load warnings for the startup notice.
        pub fn warnings(&self) -> &[String] {
            &self.warnings
        }
    }

    impl ExtensionDispatch for StreamGuard {
        fn name(&self) -> &str {
            "stream-guard"
        }

        fn delivery(&self) -> DeliveryMode {
            DeliveryMode::Native
        }

        fn worlds(&self) -> Vec<World> {
            // The manifest (the WASM install contract) names the
            // tool world only; the native handle additionally wires
            // the worlds the sandbox cannot feed. Native wiring is
            // not manifest-bound.
            //
            // `Hooks` is required as well as `HooksStream`, and the two
            // are not interchangeable: the registry dispatches
            // `pre-turn`, `post-tool-use`, and `post-turn-end` on
            // `World::Hooks` and `stream-event` on `World::HooksStream`.
            // Gh #300's window reset rides `pre-turn`, so without this a
            // stream observer's `on_pre_turn` was never called and the
            // window lived across turns - the second half of the defect.
            vec![
                World::Tool,
                World::Hooks,
                World::HooksStream,
                World::Command,
                World::Ui,
            ]
        }

        fn tool_specs(&self) -> Result<Vec<lca_protocol::ToolSpec>, DispatchError> {
            Ok(vec![guard_scan_tool_spec()])
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
                let text: serde_json::Value =
                    serde_json::from_str(&arguments).unwrap_or(serde_json::Value::Null);
                let text = text
                    .get("text")
                    .and_then(|value| value.as_str())
                    .unwrap_or("");
                let (rules, _) = load_rules(cap.as_ref(), trusted);
                Ok(lca_protocol::ToolResult {
                    call_id,
                    status: lca_protocol::ToolResultStatus::Ok,
                    content: run_command(&rules, text),
                    truncated: false,
                    exit_code: None,
                    full_output_path: None,
                    nested: Vec::new(),
                    duration_ms: None,
                    images: Vec::new(),
                    extras: Default::default(),
                })
            })
        }

        fn command_specs(&self) -> Result<Vec<lca_protocol::CommandSpec>, DispatchError> {
            Ok(vec![lca_protocol::CommandSpec {
                name: "stream-guard".to_string(),
                hint: "[text to probe]".to_string(),
                completion: "none".to_string(),
                extras: Default::default(),
            }])
        }

        fn invoke_command(
            &self,
            _name: &str,
            argument: &str,
        ) -> Result<lca_protocol::CommandEffect, DispatchError> {
            Ok(lca_protocol::CommandEffect::ShowWidget(run_command(
                &self.rules,
                argument,
            )))
        }

        fn transform_messages(
            &self,
            _messages: Vec<lca_protocol::ChatMessage>,
        ) -> DispatchFuture<
            'static,
            Result<Result<Vec<lca_protocol::ChatMessage>, String>, DispatchError>,
        > {
            Box::pin(std::future::ready(Err(DispatchError::MissingWorld {
                extension: "stream-guard".to_string(),
                world: "context-transform",
            })))
        }

        fn on_pre_turn(&self) -> DispatchFuture<'static, Result<(), DispatchError>> {
            // Gh #300 acceptance 2: a fresh turn starts with a fresh
            // window, so a match from turn N cannot re-fire on turn
            // N+1's first token. `pre-turn` is the turn boundary the
            // host fires once per turn, before any provider work.
            //
            // Note the world: this fires because the handle also
            // declares `HooksStream` - the registry's `on_pre_turn`
            // loop filters on `World::Hooks`, which a stream observer
            // does not declare. Adding it is the fix for "the reset
            // never ran"; see `worlds()` below.
            self.scanner
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .reset();
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

        fn on_stream_event<'a>(
            &'a self,
            _provider: &'a str,
            _model: &'a str,
            kind: &'a str,
            data: &'a str,
        ) -> DispatchFuture<'a, Result<(), DispatchError>> {
            let mut scanner = self
                .scanner
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            let matches = scanner.feed(kind, data);
            drop(scanner);
            if let Some(hit) = matches.first() {
                *self
                    .last_match
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner()) = Some(hit.rule.clone());
            }
            // SEAM (gh #194): observation only. A cancellation plus
            // steer-queue reminder needs a host affordance the
            // hooks-stream contract deliberately withholds; when it
            // exists, this is where the match fires it.
            Box::pin(std::future::ready(Ok(())))
        }

        fn ui_regions(&self) -> Vec<String> {
            vec!["status-line".to_string()]
        }

        fn render(&self, region: &str) -> Result<Option<lca_protocol::WidgetTree>, DispatchError> {
            if region != "status-line" {
                return Ok(None);
            }
            let last = self
                .last_match
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            Ok(
                status_widget(last.as_deref()).map(|widget| lca_protocol::WidgetTree {
                    nodes: vec![widget],
                }),
            )
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use native::StreamGuard;

// ---------------------------------------------------------------------------
// WASM delivery mode: the `guard-scan` tool.
//
// Only the `tool` (and `context-transform`) worlds import the `fs`
// capability — `hooks-stream`, `command`, and `ui` guests cannot
// read `rules.toml` at all. So the WASM component serves the tool
// world, where every call re-reads the rules behind the shared
// matcher: the model (or a scripted tool call) scans any text
// against the user's rules. The stream-observe, probe-command, and
// status halves are native-only for exactly this reason; the
// manifest below is the WASM install contract (one component, one
// servable world), while the native handle additionally wires the
// worlds the sandbox cannot feed.
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
#[allow(unsafe_code)] // generated wit-bindgen export shims
mod wasm_mode {
    wit_bindgen::generate!({
        path: "../../wit",
        world: "tool",
        export_macro_name: "export_guard_tool",
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

    /// The guest's view of the filesystem, so rule loading goes through
    /// the same [`super::load_rules`] the native handle uses.
    struct GuestFs;

    impl super::FsView for GuestFs {
        fn read(
            &self,
            scope: &str,
            path: &str,
        ) -> Result<Option<String>, lca_protocol::CapabilityError> {
            match fs::read(scope, path) {
                Ok(bytes) => Ok(Some(String::from_utf8_lossy(&bytes).into_owned())),
                // Absence is the normal case (opt-in rules), never a
                // warning: the capability reports every refusal the same
                // way it reports a missing file, and `load_rules` already
                // turns a real error into a note.
                Err(_) => Ok(None),
            }
        }
    }

    /// Scan `text` against the rules in force, returning the report.
    ///
    /// **gh #300: the workspace is not trusted here.** The guest used to
    /// read `.lca/extensions/stream-guard/rules.toml` unconditionally, so
    /// any repository it was opened in could inject rules the user never
    /// approved — while the native handle refused exactly that behind its
    /// `project_trusted` bit. One surface, two consents; a repository is
    /// not a consenting party.
    ///
    /// The route is `load_rules(fs, false)`, the one trust-gated loader:
    /// global rules load, workspace rules do not. The guest has no import
    /// that answers "is this project trusted" (the `hooks-trust` world is
    /// an operator-decision input, not a readable bit, and the `state` bag
    /// is the extension's own storage), so it takes the same answer
    /// `project-memory` takes for its global scope: the trust-dependent
    /// half refuses, with the reason, rather than assuming consent.
    ///
    /// This is a behaviour change a user can notice: an installed WASM
    /// `stream-guard` no longer scans project rules. That is the correct
    /// direction — the alternative is a repository steering the guard —
    /// and the native handle (where trust arrives at construction) keeps
    /// serving project rules for trusted projects.
    fn scan_text(text: &str) -> String {
        let (rules, warnings) = super::load_rules(&GuestFs, false);
        let report = super::run_command(&rules, text);
        if warnings.is_empty() {
            return report;
        }
        // Warnings ride the report so a broken rules file is visible in
        // the tool's answer, the way the native handle's startup notice
        // makes it visible there (gh #194's diagnostic shape).
        format!("{report}\n{}", warnings.join("\n"))
    }

    pub struct GuardTool;

    impl SchemaGuest for GuardTool {
        fn get_schema() -> Schema {
            let spec = super::guard_scan_tool_spec();
            Schema {
                name: spec.name,
                description: spec.description,
                parameters: spec.parameters.to_string(),
                extras: Vec::new(),
            }
        }
    }

    impl ExecuteTrait for GuardTool {
        fn run(call: ToolCall) -> WasmResult {
            let text: serde_json::Value =
                serde_json::from_str(&call.arguments).unwrap_or(serde_json::Value::Null);
            let text = text
                .get("text")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            WasmResult {
                call_id: call.call_id,
                status: "ok".to_string(),
                content: Some(scan_text(text)),
                truncated: false,
                extras: Vec::new(),
            }
        }
    }

    export_guard_tool!(GuardTool);
}
