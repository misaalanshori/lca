//! The ABI conformance extension (NFR-25), `tool` world.
//!
//! One mode-dispatching probe covers every host path, and the dispatch
//! itself is shared code: the WASM guest and the native twin both run
//! [`run_shared`] against a [`Cap`] implementation, so their results are
//! identical by construction — the property the Phase 2 exit test diffs.
//!
//! Guest-only modes (`trap`, `loop`, `log`, `alloc`) exercise the host's
//! trap/fuel/memory/log handling (FR-EXT-3/4/5/10) and have no native
//! twin: a native panic would take the process with it.
//!
//! # Unsafe-code exemption
//!
//! Generated `wit-bindgen` export shims are `unsafe extern "f"` items;
//! the three world modules below carry the allowance (and nothing else in
//! this crate does). Review: phase 2 review.

#![deny(unsafe_code)]

#[cfg(not(target_arch = "wasm32"))]
use lca_protocol::ToolCall;
use lca_protocol::{CapabilityError, ToolResult, ToolResultStatus};

/// The capability surface a delivery mode provides to the probe.
pub trait Cap {
    /// Read a file inside a granted scope.
    fn fs_read(&self, scope: &str, path: &str) -> Result<Vec<u8>, CapabilityError>;
    /// Write a file inside a granted scope.
    fn fs_write(&self, scope: &str, path: &str, bytes: &[u8]) -> Result<(), CapabilityError>;
    /// Stat a path inside a granted scope; returns `(is_dir, len)`.
    fn fs_stat(&self, scope: &str, path: &str) -> Result<(bool, u64), CapabilityError>;
    /// List a directory inside a granted scope.
    fn fs_list(&self, scope: &str, path: &str) -> Result<Vec<String>, CapabilityError>;
    /// List the extension's own resource entries under a prefix.
    fn resource_list(&self, prefix: &str) -> Result<Vec<(String, u64)>, CapabilityError>;
    /// Read one of the extension's own resources.
    fn resource_read(&self, path: &str) -> Result<Vec<u8>, CapabilityError>;
    /// Read one key from the extension's own state bag (absence is `None`).
    fn state_read(&self, key: &str) -> Result<Option<Vec<u8>>, CapabilityError>;
    /// Write one key into the extension's own state bag.
    fn state_write(&self, key: &str, value: &[u8]) -> Result<(), CapabilityError>;
    /// Delete one key from the extension's own state bag.
    fn state_delete(&self, key: &str) -> Result<(), CapabilityError>;
    /// List the extension's own state bag.
    fn state_list(&self) -> Result<Vec<(String, u64)>, CapabilityError>;
    /// Ask a host-rendered yes/no question (gh #124): the WASM twin
    /// crosses the `ui-dialogs` import, the native twin answers through
    /// its own injected prompter, and both report the same verdict.
    fn dialog_confirm(&self, title: &str, message: &str) -> Result<bool, String>;
    /// Call another tool through the host (gh #77): the WASM twin
    /// crosses the `tools` import, the native twin sends to the turn's
    /// slot. Both report the nested outcome as data, never a rejection.
    fn tools_execute(&self, parent_call_id: &str, name: &str, args: &str) -> ModeOutcome;
    /// Spawn a program in a granted scope.
    fn process_spawn(
        &self,
        program: &str,
        args: &[String],
        cwd: &str,
    ) -> Result<u32, CapabilityError>;
    /// Read stdout until EOF.
    fn process_read_stdout(
        &self,
        handle: u32,
        max: usize,
    ) -> Result<Option<Vec<u8>>, CapabilityError>;
    /// Read stderr until EOF.
    fn process_read_stderr(
        &self,
        handle: u32,
        max: usize,
    ) -> Result<Option<Vec<u8>>, CapabilityError>;
    /// Write to the child's stdin.
    fn process_write_stdin(&self, handle: u32, bytes: &[u8]) -> Result<u64, CapabilityError>;
    /// Wait for exit.
    fn process_wait(&self, handle: u32) -> Result<i32, CapabilityError>;
    /// Release the handle.
    fn process_kill(&self, handle: u32) -> Result<(), CapabilityError>;
    /// Spawn on a pseudo-terminal.
    fn pty_spawn(
        &self,
        program: &str,
        args: &[String],
        cwd: &str,
        rows: u16,
        cols: u16,
    ) -> Result<u32, CapabilityError>;
    /// Read terminal bytes until EOF.
    fn pty_read(&self, handle: u32, max: usize) -> Result<Option<Vec<u8>>, CapabilityError>;
    /// Forward keystrokes to the program.
    fn pty_write(&self, handle: u32, bytes: &[u8]) -> Result<u64, CapabilityError>;
    /// Resize the terminal.
    fn pty_resize(&self, handle: u32, rows: u16, cols: u16) -> Result<(), CapabilityError>;
    /// Wait for exit.
    fn pty_wait(&self, handle: u32) -> Result<i32, CapabilityError>;
    /// Release the handle.
    fn pty_kill(&self, handle: u32) -> Result<(), CapabilityError>;
}

/// The identity probe's capability surface: credentials plus the loopback
/// oauth flow, one impl per delivery mode (the [`Cap`] pattern; NFR-25).
/// The generic `lca_protocol::ProviderCap`/`OauthCap` pair does not fit
/// here because `ProviderCap` also bundles the net methods this probe never
/// uses.
pub trait IdentityCap {
    /// Set a key in this extension's own credential namespace.
    fn credentials_set(&self, key: &str, value: &str) -> Result<(), CapabilityError>;
    /// Read a key back (`None` when absent, or when the capability is denied).
    fn credentials_get(&self, key: &str) -> Result<Option<String>, CapabilityError>;
    /// Delete a key.
    fn credentials_delete(&self, key: &str) -> Result<(), CapabilityError>;
    /// Bind the loopback listener; returns `(redirect_url, handle)`.
    fn oauth_begin(&self, redirect_path: &str) -> Result<(String, u32), CapabilityError>;
    /// Ask the host to open a URL in the browser (best effort).
    fn oauth_open(&self, url: &str) -> Result<(), CapabilityError>;
    /// Block for the callback's parsed query parameters.
    fn oauth_await(&self, handle: u32) -> Result<Vec<(String, String)>, CapabilityError>;
    /// Abandon a flow.
    fn oauth_end(&self, handle: u32) -> Result<(), CapabilityError>;
}

/// The redirect path the identity probe's oauth flow uses (the manifest
/// declares the same).
pub const CALLBACK_PATH: &str = "/callback";

/// The code the test injects into the callback. The probe accepts only
/// this exact value, so the outcome is deterministic across modes (no port
/// or URL leaks into the parity comparison).
pub const FIXTURE_CODE: &str = "fixture-code";

/// The authorization URL the probe asks the host to open. It is never
/// fetched; a test replaces the browser launcher with a recorder.
pub const AUTHORIZE_URL: &str = "https://conformance.example.com/authorize";

/// What a mode produced: success plus the deterministic text both modes
/// must agree on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModeOutcome {
    /// Whether the probe succeeded.
    pub ok: bool,
    /// The text handed back to the model.
    pub text: String,
}

fn fail(err: CapabilityError) -> ModeOutcome {
    ModeOutcome {
        ok: false,
        text: err.text(),
    }
}

/// The tool schema, identical in both modes.
pub fn schema_json() -> (String, String, String) {
    (
        "conformance".to_string(),
        "ABI conformance probe: dispatches on the mode argument.".to_string(),
        r#"{"type":"object","properties":{"mode":{"type":"string"}}}"#.to_string(),
    )
}

/// The suite both modes register (gh #77, EFG-035's three tools):
/// the legacy `conformance` tool stays `direct` with no namespace, and
/// two namespaced tools exercise discovery (`deferred`) and
/// registration-time callability (`codemode`). All three dispatch
/// through the same shared modes.
pub fn catalog_specs() -> Vec<lca_protocol::ToolSpec> {
    let parameters = serde_json::json!({"type":"object","properties":{"mode":{"type":"string"}}});
    vec![
        lca_protocol::ToolSpec {
            name: "conformance".to_string(),
            description: "ABI conformance probe: dispatches on the mode argument.".to_string(),
            parameters: parameters.clone(),
            exposure: lca_protocol::ToolExposure::Direct,
            namespace: None,
            annotations: None,
            extras: Default::default(),
        },
        lca_protocol::ToolSpec {
            name: "conformance-deferred".to_string(),
            description: "ABI conformance probe (deferred discovery).".to_string(),
            parameters: parameters.clone(),
            exposure: lca_protocol::ToolExposure::Deferred,
            namespace: Some(lca_protocol::ToolNamespace {
                name: "demo".to_string(),
                description: "Deferred demonstration tools.".to_string(),
                instructions: Some("Search for these, then call them nested.".to_string()),
            }),
            annotations: None,
            extras: Default::default(),
        },
        lca_protocol::ToolSpec {
            name: "conformance-code".to_string(),
            description: "ABI conformance probe (codemode callability).".to_string(),
            parameters,
            exposure: lca_protocol::ToolExposure::Codemode,
            namespace: Some(lca_protocol::ToolNamespace {
                name: "demo".to_string(),
                description: "Deferred demonstration tools.".to_string(),
                instructions: None,
            }),
            annotations: None,
            extras: Default::default(),
        },
    ]
}

/// The redaction token (gh #45): message and result hooks rewrite
/// exactly this, so journeys asserting the rewritten text prove
/// composition ran. Absent everywhere else by construction.
pub fn redact_text(text: &str) -> Option<String> {
    text.contains("conformance-secret")
        .then(|| text.replace("conformance-secret", "conformance-redacted"))
}

/// The mutation markers (gh #45): `tag-mutate` rewrites in place,
/// `tag-block` vetoes with a reason. Both inert unless asked.
pub fn mutate_args(arguments: &str) -> lca_protocol::ToolCallPatch {
    if arguments.contains("tag-block") {
        lca_protocol::ToolCallPatch {
            arguments: None,
            block: Some("conformance blocked this call".to_string()),
        }
    } else if arguments.contains("tag-mutate") {
        lca_protocol::ToolCallPatch {
            arguments: Some(arguments.replace("tag-mutate", "tag-mutated")),
            block: None,
        }
    } else {
        lca_protocol::ToolCallPatch::default()
    }
}

/// The guest-side dispatch both tool worlds share (gh #77): the
/// divergent modes (`trap`, `loop`) live here so the catalog serves
/// exactly what the single-tool world serves, trap for trap.
#[allow(clippy::panic)] // the trap probe traps by design (NFR-25's trap row).
pub fn run_tool_mode(
    cap: &dyn Cap,
    parent_call_id: &str,
    mode: &str,
    args: &serde_json::Value,
) -> ModeOutcome {
    match mode {
        "trap" => panic!("conformance trap requested"),
        "loop" => loop {
            std::hint::spin_loop();
        },
        "log" => ModeOutcome {
            ok: true,
            text: "logged".to_string(),
        },
        "alloc" => {
            let mut hog: Vec<Vec<u8>> = Vec::new();
            for i in 0..64u64 {
                let mut block = vec![0u8; 4 * 1024 * 1024];
                block[0] = i as u8;
                hog.push(block);
            }
            let _ = hog;
            ModeOutcome {
                ok: true,
                text: "allocated".to_string(),
            }
        }
        _ => run_shared(cap, parent_call_id, mode, args),
    }
}

/// Parse the `mode` argument; anything absent means `ok`.
pub fn mode_and_args(arguments: &str) -> (String, serde_json::Value) {
    let value = serde_json::from_str::<serde_json::Value>(arguments).unwrap_or_default();
    let mode = value
        .get("mode")
        .and_then(|m| m.as_str())
        .unwrap_or("ok")
        .to_string();
    (mode, value)
}

fn string_list(value: Option<&serde_json::Value>) -> Vec<String> {
    value
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// The delivery-independent probe logic. `ok`/`fs-read`/`fs-list`/
/// `spawn`/`pty` run here in both modes; anything else is the caller's
/// to handle (guest-only modes).
pub fn run_shared(
    cap: &dyn Cap,
    parent_call_id: &str,
    mode: &str,
    args: &serde_json::Value,
) -> ModeOutcome {
    match mode {
        "ok" => ModeOutcome {
            ok: true,
            text: "conformance ok".to_string(),
        },
        // #103 (QA-017): the guest-side call counter. Mode-gated so
        // no other probe observes it, and asserted relatively (each
        // call returns one more than the last on the same guest), so
        // the native twin's process-wide counter and the WASM guest's
        // per-instance one both qualify without agreeing absolutely.
        // A fresh instance per call answers 1 every time; a cached one
        // counts up - the statefulness probe.
        "call-count" => {
            static CALLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let n = CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            ModeOutcome {
                ok: true,
                text: format!("call {n}"),
            }
        }
        // Gh #124's acceptance: a tool asking `ui.confirm`, reporting
        // the verdict as data (a denial is `ok` too - the question was
        // asked and answered, which is what the mode probes).
        "ask-confirm" => {
            let title = args
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("conformance");
            let message = args
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("proceed?");
            match cap.dialog_confirm(title, message) {
                Ok(verdict) => ModeOutcome {
                    ok: true,
                    text: format!("confirm: {verdict}"),
                },
                Err(err) => fail(CapabilityError::Invalid(err)),
            }
        }
        // Gh #77's nested path: call the sibling tool through the
        // host and report what came back, id included.
        "nested" => {
            let target = args
                .get("target")
                .and_then(|v| v.as_str())
                .unwrap_or("conformance-deferred");
            let nested_args = args
                .get("nested-args")
                .and_then(|v| v.as_str())
                .unwrap_or(r#"{"mode":"ok"}"#);
            let outcome = cap.tools_execute(parent_call_id, target, nested_args);
            ModeOutcome {
                ok: outcome.ok,
                text: format!("nested: {}", outcome.text),
            }
        }
        // Gh #45's redaction marker: the tool-result hook rewrites
        // exactly this token, so a journey asserting the redacted
        // text proves composition ran live.
        "spill-secret" => ModeOutcome {
            ok: true,
            text: "the token is conformance-secret, handle with care".to_string(),
        },
        "fs-read" => {
            let (Some(scope), Some(path)) = (
                args.get("scope").and_then(|v| v.as_str()),
                args.get("path").and_then(|v| v.as_str()),
            ) else {
                return fail(CapabilityError::Invalid(
                    "fs-read needs scope and path".to_string(),
                ));
            };
            match cap.fs_read(scope, path) {
                Ok(bytes) => ModeOutcome {
                    ok: true,
                    text: format!("fs: {}", String::from_utf8_lossy(&bytes)),
                },
                Err(err) => fail(err),
            }
        }
        "fs-list" => {
            let scope = args
                .get("scope")
                .and_then(|v| v.as_str())
                .unwrap_or("workspace");
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
            match cap.fs_list(scope, path) {
                Ok(names) => ModeOutcome {
                    ok: true,
                    text: format!("list: {}", names.join(",")),
                },
                Err(err) => fail(err),
            }
        }
        "resource-list" => {
            let prefix = args.get("prefix").and_then(|v| v.as_str()).unwrap_or("");
            match cap.resource_list(prefix) {
                Ok(entries) => ModeOutcome {
                    ok: true,
                    text: format!(
                        "resources: {}",
                        entries
                            .iter()
                            .map(|(path, size)| format!("{path}:{size}"))
                            .collect::<Vec<_>>()
                            .join(",")
                    ),
                },
                Err(err) => fail(err),
            }
        }
        "resource-read" => {
            let Some(path) = args.get("path").and_then(|v| v.as_str()) else {
                return fail(CapabilityError::Invalid(
                    "resource-read needs path".to_string(),
                ));
            };
            match cap.resource_read(path) {
                Ok(bytes) => ModeOutcome {
                    ok: true,
                    text: format!("resource: {}", String::from_utf8_lossy(&bytes)),
                },
                Err(err) => fail(err),
            }
        }
        "state-write" => {
            let (Some(key), Some(value)) = (
                args.get("key").and_then(|v| v.as_str()),
                args.get("value").and_then(|v| v.as_str()),
            ) else {
                return fail(CapabilityError::Invalid(
                    "state-write needs key and value".to_string(),
                ));
            };
            match cap.state_write(key, value.as_bytes()) {
                Ok(()) => ModeOutcome {
                    ok: true,
                    text: format!("state wrote {key}"),
                },
                Err(err) => fail(err),
            }
        }
        "state-read" => {
            let Some(key) = args.get("key").and_then(|v| v.as_str()) else {
                return fail(CapabilityError::Invalid("state-read needs key".to_string()));
            };
            match cap.state_read(key) {
                Ok(Some(bytes)) => ModeOutcome {
                    ok: true,
                    text: format!("state: {}", String::from_utf8_lossy(&bytes)),
                },
                Ok(None) => ModeOutcome {
                    ok: true,
                    text: "state: <none>".to_string(),
                },
                Err(err) => fail(err),
            }
        }
        "state-delete" => {
            let Some(key) = args.get("key").and_then(|v| v.as_str()) else {
                return fail(CapabilityError::Invalid(
                    "state-delete needs key".to_string(),
                ));
            };
            match cap.state_delete(key) {
                Ok(()) => ModeOutcome {
                    ok: true,
                    text: format!("state deleted {key}"),
                },
                Err(err) => fail(err),
            }
        }
        "state-list" => match cap.state_list() {
            Ok(entries) => ModeOutcome {
                ok: true,
                text: format!(
                    "state: {}",
                    entries
                        .iter()
                        .map(|(key, size)| format!("{key}:{size}"))
                        .collect::<Vec<_>>()
                        .join(",")
                ),
            },
            Err(err) => fail(err),
        },
        "spawn" => {
            let (Some(program), Some(cwd)) = (
                args.get("program").and_then(|v| v.as_str()),
                args.get("cwd").and_then(|v| v.as_str()),
            ) else {
                return fail(CapabilityError::Invalid(
                    "spawn needs program and cwd".to_string(),
                ));
            };
            let program_args = string_list(args.get("args"));
            let handle = match cap.process_spawn(program, &program_args, cwd) {
                Ok(handle) => handle,
                Err(err) => return fail(err),
            };
            let mut output = Vec::new();
            loop {
                match cap.process_read_stdout(handle, 4096) {
                    Ok(Some(chunk)) => output.extend_from_slice(&chunk),
                    Ok(None) => break,
                    Err(err) => {
                        let _ = cap.process_kill(handle);
                        return fail(err);
                    }
                }
            }
            let code = cap.process_wait(handle).unwrap_or(-1);
            let _ = cap.process_kill(handle);
            ModeOutcome {
                ok: code == 0,
                text: format!("exit {code} {}", String::from_utf8_lossy(&output).trim()),
            }
        }
        "pty" => {
            let (Some(program), Some(cwd)) = (
                args.get("program").and_then(|v| v.as_str()),
                args.get("cwd").and_then(|v| v.as_str()),
            ) else {
                return fail(CapabilityError::Invalid(
                    "pty needs program and cwd".to_string(),
                ));
            };
            let rows = args.get("rows").and_then(|v| v.as_u64()).unwrap_or(24) as u16;
            let cols = args.get("cols").and_then(|v| v.as_u64()).unwrap_or(80) as u16;
            let program_args = string_list(args.get("args"));
            let handle = match cap.pty_spawn(program, &program_args, cwd, rows, cols) {
                Ok(handle) => handle,
                Err(err) => return fail(err),
            };
            let mut output = Vec::new();
            loop {
                match cap.pty_read(handle, 4096) {
                    Ok(Some(chunk)) => output.extend_from_slice(&chunk),
                    Ok(None) => break,
                    Err(err) => {
                        let _ = cap.pty_kill(handle);
                        return fail(err);
                    }
                }
            }
            let code = cap.pty_wait(handle).unwrap_or(-1);
            let _ = cap.pty_kill(handle);
            ModeOutcome {
                ok: code == 0,
                text: format!("exit {code} {}", String::from_utf8_lossy(&output).trim()),
            }
        }
        "fs-write" => {
            let (Some(scope), Some(path)) = (
                args.get("scope").and_then(|v| v.as_str()),
                args.get("path").and_then(|v| v.as_str()),
            ) else {
                return fail(CapabilityError::Invalid(
                    "fs-write needs scope and path".to_string(),
                ));
            };
            let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("");
            match cap
                .fs_write(scope, path, content.as_bytes())
                .and_then(|()| cap.fs_stat(scope, path))
            {
                Ok((is_dir, len)) => ModeOutcome {
                    ok: true,
                    text: format!("fs-write: dir={is_dir} len={len}"),
                },
                Err(err) => fail(err),
            }
        }
        // Exercises `process.write-stdin` and `read-stderr` alongside spawn;
        // the report is boolean so the two delivery modes agree regardless of
        // how much the child printed.
        "process-io" => {
            let (Some(program), Some(cwd)) = (
                args.get("program").and_then(|v| v.as_str()),
                args.get("cwd").and_then(|v| v.as_str()),
            ) else {
                return fail(CapabilityError::Invalid(
                    "process-io needs program and cwd".to_string(),
                ));
            };
            let program_args = string_list(args.get("args"));
            let stdin = args.get("stdin").and_then(|v| v.as_str()).unwrap_or("");
            let handle = match cap.process_spawn(program, &program_args, cwd) {
                Ok(handle) => handle,
                Err(err) => return fail(err),
            };
            let wrote = cap.process_write_stdin(handle, stdin.as_bytes()).is_ok();
            let read_err = cap.process_read_stderr(handle, 4096).is_ok();
            let _ = cap.process_kill(handle);
            ModeOutcome {
                ok: true,
                text: format!("process-io stdin={wrote} stderr={read_err}"),
            }
        }
        // Exercises `pty.write` and `pty.resize`; no output is read or waited
        // on, so the two modes agree without depending on terminal timing.
        "pty-io" => {
            let (Some(program), Some(cwd)) = (
                args.get("program").and_then(|v| v.as_str()),
                args.get("cwd").and_then(|v| v.as_str()),
            ) else {
                return fail(CapabilityError::Invalid(
                    "pty-io needs program and cwd".to_string(),
                ));
            };
            let program_args = string_list(args.get("args"));
            let handle = match cap.pty_spawn(program, &program_args, cwd, 24, 80) {
                Ok(handle) => handle,
                Err(err) => return fail(err),
            };
            let resized = cap.pty_resize(handle, 30, 100).is_ok();
            let wrote = cap.pty_write(handle, b"\r").is_ok();
            let _ = cap.pty_kill(handle);
            ModeOutcome {
                ok: true,
                text: format!("pty-io resize={resized} write={wrote}"),
            }
        }
        _ => ModeOutcome {
            ok: false,
            text: format!("unknown mode {mode}"),
        },
    }
}

/// The pre-tool policy both modes run: tool names carry the verdict so
/// a native and a wasm build agree without configuration.
pub fn pre_tool_action(name: &str) -> lca_protocol::HookAction {
    use lca_protocol::HookAction;
    if name.starts_with("probe-deny") {
        HookAction::Deny("conformance policy denied this tool".to_string())
    } else {
        HookAction::Allow
    }
}

/// The command leaf both modes register (`<extension>.probe` once the
/// host namespaces it).
pub fn command_leaf() -> lca_protocol::CommandSpec {
    lca_protocol::CommandSpec {
        name: "probe".to_string(),
        hint: "conformance command probe".to_string(),
        completion: "none".to_string(),
        extras: Default::default(),
    }
}

/// The command effect both modes produce.
pub fn invoke_command(argument: &str) -> lca_protocol::CommandEffect {
    use lca_protocol::CommandEffect;
    if argument == "submit" {
        CommandEffect::SubmitPrompt("conformance submitted".to_string())
    } else if let Some(text) = argument.strip_prefix("insert:") {
        CommandEffect::InsertText(text.to_string())
    } else {
        CommandEffect::None
    }
}

/// Build a protocol result from an outcome (the native path).
pub fn outcome_to_result(call_id: &str, outcome: ModeOutcome) -> ToolResult {
    ToolResult {
        call_id: call_id.to_string(),
        status: if outcome.ok {
            ToolResultStatus::Ok
        } else {
            ToolResultStatus::Error
        },
        content: outcome.text,
        truncated: false,
        images: Vec::new(),
        extras: Default::default(),
        exit_code: None,
        full_output_path: None,
        nested: Vec::new(),
    }
}

mod scripts;

pub use scripts::{
    compact_excerpts, compact_script, provider_models, provider_models_default, scripted_events,
    scripted_login, scripted_login_options, scripted_login_submit, scripted_logout, scripted_usage,
    scripted_usage_report, transform_script, ui_event_script, ui_script,
};

// ---------------------------------------------------------------------------
// Native delivery mode (compiled into the host, unsandboxed, labeled so)
// ---------------------------------------------------------------------------

#[cfg(not(target_arch = "wasm32"))]
mod native;

#[cfg(not(target_arch = "wasm32"))]
pub use native::{NativeCap, NativeConformance};

// ---------------------------------------------------------------------------
// WASM delivery mode
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// WASM delivery mode: one component exporting every world
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
#[allow(unsafe_code)] // generated wit-bindgen export shims (see crate docs)
mod tool_world;

// ---------------------------------------------------------------------------
// WASM delivery mode: the tool-catalog world (gh #77)
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
#[allow(unsafe_code)] // generated wit-bindgen export shims (see crate docs)
mod catalog_world;

#[cfg(target_arch = "wasm32")]
#[allow(unsafe_code)] // generated wit-bindgen export shims (see crate docs)
mod command_world;

#[cfg(target_arch = "wasm32")]
#[allow(unsafe_code)] // generated wit-bindgen export shims (see crate docs)
mod hooks_world;
// ---------------------------------------------------------------------------
// WASM delivery mode: the new hooks worlds (gh #45)
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
#[allow(unsafe_code)] // generated wit-bindgen export shims (see crate docs)
mod hooks_message;

#[cfg(target_arch = "wasm32")]
#[allow(unsafe_code)] // generated wit-bindgen export shims (see crate docs)
mod hooks_tool_call;

#[cfg(target_arch = "wasm32")]
#[allow(unsafe_code)] // generated wit-bindgen export shims (see crate docs)
mod hooks_tool_result;

#[cfg(target_arch = "wasm32")]
#[allow(unsafe_code)] // generated wit-bindgen export shims (see crate docs)
mod hooks_stream;

#[cfg(target_arch = "wasm32")]
#[allow(unsafe_code)] // generated wit-bindgen export shims (see crate docs)
mod hooks_settle;

#[cfg(target_arch = "wasm32")]
#[allow(unsafe_code)] // generated wit-bindgen export shims (see crate docs)
mod hooks_compaction;

#[cfg(target_arch = "wasm32")]
#[allow(unsafe_code)] // generated wit-bindgen export shims (see crate docs)
mod hooks_cache;

#[cfg(target_arch = "wasm32")]
#[allow(unsafe_code)] // generated wit-bindgen export shims (see crate docs)
mod hooks_trust;

// ---------------------------------------------------------------------------
// WASM delivery mode: the provider world
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
#[allow(unsafe_code)] // generated wit-bindgen export shims (see crate docs)
mod provider_world;

// ---------------------------------------------------------------------------
// WASM delivery mode: the compaction world
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
#[allow(unsafe_code)] // generated wit-bindgen export shims
mod compaction_world;

// ---------------------------------------------------------------------------
// WASM delivery mode: the context-transform world
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
#[allow(unsafe_code)] // generated wit-bindgen export shims
mod transform_world;

// ---------------------------------------------------------------------------
// WASM delivery mode: the ui world
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
#[allow(unsafe_code)] // generated wit-bindgen export shims
mod ui_world;
