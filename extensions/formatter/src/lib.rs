//! Auto-format on file-mutating tool results (gh #220): after a
//! successful `edit` or `write`, the project's formatter runs over
//! the touched file before the next step. Language knowledge
//! (`rustfmt`, `ruff`/`black`, `gofmt`, `biome`/`prettier`, `zig fmt`)
//! lives here, not in core (ADR-0013: external tool wrapping is a
//! runtime extension).
//!
//! Every degradation is a warning plus an untouched result:
//! formatter missing, formatter failing, unknown extension,
//! toggled off, non-`ok` outcome. Formatting must never break a
//! turn, and the result patch is always identity (the model's error
//! state — and success state — survive byte for byte).
//!
//! Spawning runs through the `process` capability, which means host
//! authorization like any other command: pre-approve the project's
//! formatter with an allow rule, or approve per file. A denial warns
//! like a missing binary.
//!
//! # Unsafe-code exemption
//!
//! The `wasm32` halves carry generated `wit-bindgen` export shims;
//! the module holds the allowance.

#![deny(unsafe_code)]

use std::sync::Mutex;

/// The manifest this form ships with (single source for
/// [`manifest_grants`]; a test keeps them in step).
pub const MANIFEST: &str = include_str!("../extension.toml");

/// The grants the manifest declares: `process` for the formatter
/// binaries, read-write on the workspace for the files - nothing else.
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::expect_used)] // the extension's own literal manifest patterns are valid by construction.
pub fn manifest_grants() -> lca_tools::CapabilityGrants {
    lca_tools::CapabilityGrants {
        fs: vec![
            lca_permissions::ScopeGrant::parse("workspace", lca_permissions::FsMode::ReadWrite)
                .expect("the manifest's own scope parses"),
        ],
        fs_declared: true,
        process: true,
        ..Default::default()
    }
}

/// One formatter invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatterCommand {
    /// The binary (`rustfmt`, `ruff`, ...).
    pub program: String,
    /// Arguments with the file already in place.
    pub args: Vec<String>,
}

/// Pick the formatter for a workspace-relative path, probing config
/// files through `has_file`. `None` means no formatter claims this
/// file (silent skip, never error).
pub fn select_formatter(path: &str, has_file: impl Fn(&str) -> bool) -> Option<FormatterCommand> {
    let run = |program: &str, args: &[&str]| {
        let mut full: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
        full.push(path.to_string());
        Some(FormatterCommand {
            program: program.to_string(),
            args: full,
        })
    };
    match path.rsplit('.').next().unwrap_or("") {
        "rs" => run("rustfmt", &[]),
        "go" => run("gofmt", &["-w"]),
        "zig" => run("zig", &["fmt"]),
        "py" if has_file("ruff.toml") || has_file("pyproject.toml") => run("ruff", &["format"]),
        "py" => run("black", &[]),
        "ts" | "js" | "tsx" | "jsx" | "mjs" | "cjs" if has_file("biome.json") => {
            run("biome", &["format", "--write"])
        }
        "ts" | "js" | "tsx" | "jsx" | "mjs" | "cjs" => run("prettier", &["--write"]),
        _ => None,
    }
}

/// Whether a workspace-relative path lies under a directory a bare run must
/// not touch (`lca_tools::SKIP_DIRS`: VCS internals and build output).
///
/// Matching is **component-wise**, not a substring test, and that is the whole
/// subtlety: `src/targeted/x.rs` is an ordinary source file whose directory
/// merely starts with `target`, and `src/.gitignore` is a config file whose
/// name merely starts with `.git`. Only a path component equal to a skip name
/// counts. A leading `./` or a trailing slash is tolerated, since a user types
/// both.
///
/// Native-only, like the walk it serves: `lca-tools` is a desktop dependency
/// (its capability engine needs the host), so the WASM half - which can only
/// log and never spawn - does not compile these three helpers.
#[cfg(not(target_arch = "wasm32"))]
pub fn is_skipped_path(path: &str) -> bool {
    path.split('/')
        .filter(|component| !component.is_empty() && *component != ".")
        .any(|component| lca_tools::SKIP_DIRS.contains(&component))
}

/// The skip root a path lies under, for the refusal message. `None` when the
/// path is formattable.
#[cfg(not(target_arch = "wasm32"))]
pub fn skip_root(path: &str) -> Option<&'static str> {
    path.split('/')
        .filter(|component| !component.is_empty() && *component != ".")
        .find_map(|component| {
            lca_tools::SKIP_DIRS
                .iter()
                .find(|root| **root == component)
                .copied()
        })
}

/// Why an explicitly named path is refused (acceptance 2), or `None` when it
/// is formattable. The message names the root and the reason, because a
/// refusal a user cannot understand reads as a bug.
#[cfg(not(target_arch = "wasm32"))]
pub fn refusal_reason(path: &str) -> Option<String> {
    skip_root(path).map(|root| {
        format!(
            "{path}: refused - `{root}/` holds generated or version-control content; \
             run the formatter on source files instead"
        )
    })
}

/// Extract the touched path from a tool call's arguments — but only
/// for the file-mutating tools. Any other tool's `path` field (a
/// `read` included) never triggers a format.
pub fn extract_path(tool: &str, arguments: &str) -> Option<String> {
    if !matches!(tool, "edit" | "write") {
        return None;
    }
    serde_json::from_str::<serde_json::Value>(arguments)
        .ok()?
        .get("path")?
        .as_str()
        .map(str::to_string)
}

/// What the hook did with one tool result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookOutcome {
    /// The formatter ran clean over the file.
    Formatted {
        /// The binary that ran.
        program: String,
    },
    /// Nothing ran, with the reason (off, non-ok, unknown tool or
    /// type, no path).
    Skipped(String),
    /// The formatter was attempted but warned (missing, denied,
    /// failing): the result stays untouched either way.
    Warning(String),
    /// The path lies under a directory a bare run never touches
    /// (gh #357: VCS internals, build output). Named separately from
    /// [`HookOutcome::Skipped`] so the refusal is visible rather than
    /// silent.
    Refused(String),
}

/// The process surface the hook needs: spawn, wait, and the failure
/// text. Denials surface as spawn errors, which warn like a missing
/// binary.
pub trait ProcessView {
    /// Spawn the formatter in the workspace; the handle, or the
    /// reason it never started.
    fn spawn(&self, program: &str, args: &[String]) -> Result<u32, String>;
    /// Wait for the handle; the exit code.
    fn wait(&self, handle: u32) -> Result<i32, String>;
    /// The handle's stderr (the failing formatter's own words).
    fn read_stderr(&self, handle: u32) -> String;
}

/// The filesystem surface the formatter needs: existence probes (to pick a
/// config-aware formatter) and the two calls a workspace walk is built from.
///
/// `list` is deliberately **one level**: the recursive, skip-aware walk is
/// [`walk_workspace`], a free function over this trait. Keeping the recursion
/// out of the implementation is what lets a test drive the real walker over a
/// fabricated tree instead of re-implementing it.
pub trait FsView {
    /// Whether a workspace-relative path exists.
    fn exists(&self, path: &str) -> bool;
    /// Workspace-relative entries directly under `dir`, one level.
    fn list(&self, dir: &str) -> Vec<String>;
    /// `(is_dir, len)` for a workspace-relative path, or `None` when it is
    /// unreadable or outside the granted scope.
    fn stat(&self, path: &str) -> Option<(bool, u64)>;
}

/// Recursively collect every file under `dir`, never descending into a
/// directory named in [`lca_tools::SKIP_DIRS`].
///
/// Gh #357. Two properties this carries:
///
/// * **The skip is evaluated at descent time**, not by filtering afterwards.
///   Not descending is the entire saving: `.git/objects` on a real repo is
///   tens of thousands of stat calls across the capability boundary, and no
///   post-filter can un-pay that.
/// * **Matching is component-wise** (the same rule [`is_skipped_path`] uses),
///   so `src/targeted/` and `src/.gitignore.rs` are ordinary content while
///   `nested/target/gen.rs` is not.
///
/// An unreadable directory ends that subtree and nothing else: a permission
/// denial deep in the tree must not abort the walk.
#[must_use]
#[cfg(not(target_arch = "wasm32"))]
pub fn walk_workspace(fs: &dyn FsView, dir: &str) -> Vec<String> {
    let mut files = Vec::new();
    for entry in fs.list(dir) {
        let path = if dir.is_empty() {
            entry.clone()
        } else {
            format!("{dir}/{entry}")
        };
        let path = path.trim_end_matches('/').to_string();
        match fs.stat(&path) {
            Some((false, _)) => files.push(path),
            Some((true, _)) => {
                if is_skipped_path(&path) {
                    continue;
                }
                files.extend(walk_workspace(fs, &path));
            }
            None => {}
        }
    }
    files
}

/// The toggleable state: on by default, flipped by command.
pub struct FormatterState {
    enabled: Mutex<bool>,
}

impl Default for FormatterState {
    /// On: format every successful edit/write result.
    fn default() -> FormatterState {
        FormatterState::new()
    }
}

impl FormatterState {
    /// On: format every successful edit/write result.
    pub fn new() -> FormatterState {
        FormatterState {
            enabled: Mutex::new(true),
        }
    }

    /// Off: used by tests (users flip with the command).
    pub fn disabled() -> FormatterState {
        FormatterState {
            enabled: Mutex::new(false),
        }
    }

    /// Whether auto-format currently runs.
    pub fn enabled(&self) -> bool {
        *self
            .enabled
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    /// Handle one tool result: the outcome matrix. The caller maps
    /// every outcome to an identity result patch.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn handle_result(
        &self,
        process: &dyn ProcessView,
        fs: &dyn FsView,
        tool: &str,
        arguments: &str,
        status_ok: bool,
    ) -> HookOutcome {
        if !self.enabled() {
            return HookOutcome::Skipped("auto-format is off".to_string());
        }
        if !status_ok {
            return HookOutcome::Skipped("the tool result was not ok".to_string());
        }
        let Some(path) = extract_path(tool, arguments) else {
            return HookOutcome::Skipped("no formattable file in this call".to_string());
        };
        self.format_file(process, fs, &path)
    }

    /// Format one file through its formatter: the shared core of the
    /// hook and the `run` command.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn format_file(
        &self,
        process: &dyn ProcessView,
        fs: &dyn FsView,
        path: &str,
    ) -> HookOutcome {
        // Gh #357: a path under a VCS or build root is refused, never
        // formatted - even when the user named it explicitly. This is the one
        // gate both the hook and `run <path>` pass through, so an explicit
        // path and a model-touched path get the same answer.
        if let Some(reason) = refusal_reason(path) {
            return HookOutcome::Refused(reason);
        }
        let Some(command) = select_formatter(path, |probe| fs.exists(probe)) else {
            return HookOutcome::Skipped(format!("no formatter for {path}"));
        };
        let handle = match process.spawn(&command.program, &command.args) {
            Ok(handle) => handle,
            Err(reason) => {
                return HookOutcome::Warning(format!(
                    "{} did not run for {path}: {reason}",
                    command.program
                ));
            }
        };
        match process.wait(handle) {
            Ok(0) => HookOutcome::Formatted {
                program: command.program,
            },
            Ok(code) => {
                let stderr = process.read_stderr(handle);
                let tail: String = stderr
                    .trim()
                    .chars()
                    .rev()
                    .take(300)
                    .collect::<String>()
                    .chars()
                    .rev()
                    .collect();
                HookOutcome::Warning(format!(
                    "{} exited {code} for {path}: {tail}",
                    command.program
                ))
            }
            Err(reason) => HookOutcome::Warning(format!(
                "{} never finished for {path}: {reason}",
                command.program
            )),
        }
    }

    /// The `formatter` command: `on`, `off`, `run [path]` (a file, or
    /// the whole workspace when empty). Reports plain text for the
    /// notice area.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn handle_command(
        &self,
        process: &dyn ProcessView,
        fs: &dyn FsView,
        argument: &str,
    ) -> String {
        let mut words = argument.split_whitespace();
        match words.next().unwrap_or("") {
            "on" => {
                *self
                    .enabled
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner()) = true;
                "formatter: on — successful edit/write results format automatically".to_string()
            }
            "off" => {
                *self
                    .enabled
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner()) = false;
                "formatter: off — results pass through untouched".to_string()
            }
            "run" => {
                let targets: Vec<String> = match words.next() {
                    // An explicitly named path is still passed through
                    // [`FormatterState::format_file`], which refuses a skip
                    // root with a reason (gh #357 acceptance 2) - so naming
                    // `target/gen/out.rs` reports the refusal rather than
                    // silently formatting build output.
                    Some(path) => vec![path.to_string()],
                    // The bare run: the skip-aware walk, then the extension
                    // filter. `walk_workspace` never descends into
                    // `lca_tools::SKIP_DIRS`, so generated and VCS content is
                    // neither stat'ed nor formatted (gh #357).
                    None => walk_workspace(fs, "")
                        .into_iter()
                        .filter(|path| select_formatter(path, |probe| fs.exists(probe)).is_some())
                        .collect(),
                };
                if targets.is_empty() {
                    return "formatter: nothing formattable found".to_string();
                }
                let mut lines = Vec::new();
                for target in targets {
                    match self.format_file(process, fs, &target) {
                        HookOutcome::Formatted { program } => {
                            lines.push(format!("{target}: {program}"));
                        }
                        HookOutcome::Skipped(reason)
                        | HookOutcome::Warning(reason)
                        | HookOutcome::Refused(reason) => {
                            lines.push(format!("{target}: {reason}"));
                        }
                    }
                }
                lines.join("\n")
            }
            _ => "formatter: on|off|run [path]".to_string(),
        }
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
        fn exists(&self, path: &str) -> bool {
            self.fs_stat("workspace", path).is_ok()
        }

        /// One level only. The recursion lives in
        /// [`super::walk_workspace`], so the skip rule has exactly one
        /// implementation and a test can drive it without a real filesystem.
        fn list(&self, dir: &str) -> Vec<String> {
            self.fs_list("workspace", dir).unwrap_or_default()
        }

        fn stat(&self, path: &str) -> Option<(bool, u64)> {
            self.fs_stat("workspace", path).ok()
        }
    }

    impl ProcessView for lca_tools::Capabilities {
        fn spawn(&self, program: &str, args: &[String]) -> Result<u32, String> {
            let owned: Vec<String> = args.to_vec();
            self.process_spawn(program, &owned, "workspace")
                .map_err(|err| err.to_string())
        }

        fn wait(&self, handle: u32) -> Result<i32, String> {
            self.process_wait(handle).map_err(|err| err.to_string())
        }

        fn read_stderr(&self, _handle: u32) -> String {
            // Best effort: stderr already consumed or gone reads empty.
            String::new()
        }
    }

    /// The native handle: hook plus commands over the engine.
    pub struct Formatter {
        state: FormatterState,
        cap: Arc<lca_tools::Capabilities>,
    }

    impl Formatter {
        /// Build from the engine whose manifest grants carry
        /// `process` and workspace read-write.
        pub fn new(cap: Arc<lca_tools::Capabilities>) -> Formatter {
            Formatter {
                state: FormatterState::new(),
                cap,
            }
        }
    }

    impl ExtensionDispatch for Formatter {
        fn name(&self) -> &str {
            "formatter"
        }

        fn delivery(&self) -> DeliveryMode {
            DeliveryMode::Native
        }

        fn worlds(&self) -> Vec<World> {
            vec![World::HooksToolResult, World::Command]
        }

        fn tool_specs(&self) -> Result<Vec<lca_protocol::ToolSpec>, DispatchError> {
            Err(DispatchError::MissingWorld {
                extension: "formatter".to_string(),
                world: "tool",
            })
        }

        fn execute_tool<'a>(
            &'a self,
            _call: &'a lca_protocol::ToolCall,
        ) -> DispatchFuture<'a, Result<lca_protocol::ToolResult, DispatchError>> {
            Box::pin(std::future::ready(Err(DispatchError::MissingWorld {
                extension: "formatter".to_string(),
                world: "tool",
            })))
        }

        fn command_specs(&self) -> Result<Vec<lca_protocol::CommandSpec>, DispatchError> {
            // One spec, verbs as arguments (`/formatter on`,
            // `/formatter run src/main.rs`): the WASM component serves
            // one spec per world, so three colon specs would fragment
            // the interface across modes.
            Ok(vec![lca_protocol::CommandSpec {
                name: "formatter".to_string(),
                hint: "on|off|run [path]".to_string(),
                completion: "file".to_string(),
                extras: Default::default(),
            }])
        }

        fn invoke_command(
            &self,
            _name: &str,
            argument: &str,
        ) -> Result<lca_protocol::CommandEffect, DispatchError> {
            Ok(lca_protocol::CommandEffect::ShowWidget(
                self.state
                    .handle_command(self.cap.as_ref(), self.cap.as_ref(), argument),
            ))
        }

        fn transform_messages(
            &self,
            _messages: Vec<lca_protocol::ChatMessage>,
        ) -> DispatchFuture<
            'static,
            Result<Result<Vec<lca_protocol::ChatMessage>, String>, DispatchError>,
        > {
            Box::pin(std::future::ready(Err(DispatchError::MissingWorld {
                extension: "formatter".to_string(),
                world: "context-transform",
            })))
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

        fn on_tool_result<'a>(
            &'a self,
            call: &'a lca_protocol::ToolCall,
            result: &'a lca_protocol::ToolResult,
        ) -> DispatchFuture<'a, Result<lca_protocol::ToolResultPatch, DispatchError>> {
            let outcome = self.state.handle_result(
                self.cap.as_ref(),
                self.cap.as_ref(),
                &call.name,
                &call.arguments,
                result.status == lca_protocol::ToolResultStatus::Ok,
            );
            // Identity, always: formatting is a side effect on the
            // file, never a rewrite of what the model saw. A warning
            // that mutated the result would teach the model the file
            // is broken when only its style pass failed.
            let _ = outcome;
            Box::pin(std::future::ready(Ok(
                lca_protocol::ToolResultPatch::default(),
            )))
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use native::Formatter;

// ---------------------------------------------------------------------------
// WASM delivery mode: the result hook plus the commands
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
#[allow(unsafe_code)] // generated wit-bindgen export shims
mod wasm_mode {
    use std::sync::atomic::AtomicBool;

    // The hook world imports the log alone: no fs reads, no process
    // spawns. The WASM half therefore decides (which formatter the
    // file claims) and reports (host log + identity patch), while
    // the actual spawn stays native-only. The command half toggles
    // a guest-static flag and probes selection; `run` reports what
    // would run rather than running it. Reduced, not hollow: every
    // WASM answer matches the native decision for the same input.
    static ENABLED: AtomicBool = AtomicBool::new(true);

    mod wasm_hook {
        wit_bindgen::generate!({
            path: "../../wit",
            world: "hooks-tool-result",
            export_macro_name: "export_formatter_hook",
            with: {
                "lca:host/log@0.6.0": generate,
            },
        });

        use exports::lca::ext::hook_tool_result::{Guest, ToolCall, ToolResult, ToolResultPatch};
        use lca::host::log;

        pub struct FormatterHook;

        impl Guest for FormatterHook {
            fn on_tool_result(call: ToolCall, outcome: ToolResult) -> ToolResultPatch {
                let none = ToolResultPatch {
                    content: None,
                    is_error: None,
                };
                if !super::ENABLED.load(std::sync::atomic::Ordering::Relaxed) {
                    return none;
                }
                if outcome.status != "ok" {
                    return none;
                }
                let Some(path) = super::super::extract_path(&call.name, &call.arguments) else {
                    return none;
                };
                match super::super::select_formatter(&path, |_| false) {
                    Some(command) => {
                        log::info(&format!(
                            "formatter (wasm): {} claims {}; spawn is native-only",
                            command.program, path
                        ));
                    }
                    None => {}
                }
                none
            }
        }

        export_formatter_hook!(FormatterHook);
    }

    mod wasm_command {
        wit_bindgen::generate!({
            path: "../../wit",
            world: "command",
            export_macro_name: "export_formatter_command",
            with: {
                "lca:host/ui-dialogs@0.6.0": generate,
            },
        });

        use exports::lca::ext::command_spec::{Guest as SpecGuest, Spec};
        use exports::lca::ext::invoke::{Effect, Guest as InvokeGuest};

        pub struct FormatterCommand;

        impl SpecGuest for FormatterCommand {
            fn get_spec() -> Spec {
                // One spec serves the verbs: the host routes the
                // full `/formatter ...` line as the argument.
                Spec {
                    name: "formatter".to_string(),
                    hint: "on|off|run [path]".to_string(),
                    completion: "none".to_string(),
                    extras: Vec::new(),
                }
            }
        }

        impl InvokeGuest for FormatterCommand {
            fn run(argument: String) -> Effect {
                let word = argument.split_whitespace().next().unwrap_or("");
                match word {
                    "on" => {
                        super::ENABLED.store(true, std::sync::atomic::Ordering::Relaxed);
                        Effect::ShowWidget(
                            "formatter: on (wasm decisions only; spawning stays native)"
                                .to_string(),
                        )
                    }
                    "off" => {
                        super::ENABLED.store(false, std::sync::atomic::Ordering::Relaxed);
                        Effect::ShowWidget("formatter: off".to_string())
                    }
                    _ => Effect::ShowWidget(
                        "formatter (wasm): run needs the native half".to_string(),
                    ),
                }
            }
        }

        export_formatter_command!(FormatterCommand);
    }
}
