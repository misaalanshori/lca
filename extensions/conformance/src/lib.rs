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
pub fn run_shared(cap: &dyn Cap, mode: &str, args: &serde_json::Value) -> ModeOutcome {
    match mode {
        "ok" => ModeOutcome {
            ok: true,
            text: "conformance ok".to_string(),
        },
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
    }
}

// ---------------------------------------------------------------------------
// Provider world: one script, both modes (the Phase 3 half of NFR-25)
// ---------------------------------------------------------------------------

/// The model list both modes return (FR-PROV-2).
/// The models the probe reports. A `models` setting (what `login-submit`
/// hands the host to persist, ADR-0033) is the discovered list and wins -
/// the same rule in both delivery modes (ADR-0035).
pub fn provider_models(settings: &[(String, String)]) -> Vec<lca_protocol::ModelInfo> {
    if let Some((_, list)) = settings.iter().find(|(key, _)| key == "models") {
        return list
            .split(',')
            .filter(|id| !id.is_empty())
            .map(|id| lca_protocol::ModelInfo {
                id: id.to_string(),
                name: id.to_string(),
                context_window: 4096,
                max_tokens: 0,
                extras: Default::default(),
            })
            .collect();
    }
    provider_models_default()
}

/// The probe's fixed list, when no `models` setting was passed.
pub fn provider_models_default() -> Vec<lca_protocol::ModelInfo> {
    vec![
        lca_protocol::ModelInfo {
            id: "conformance-a".to_string(),
            name: "Conformance A".to_string(),
            context_window: 4096,
            max_tokens: 512,
            extras: Default::default(),
        },
        lca_protocol::ModelInfo {
            id: "conformance-b".to_string(),
            name: "Conformance B".to_string(),
            context_window: 8192,
            max_tokens: 1024,
            extras: Default::default(),
        },
    ]
}

/// The per-completion usage record every successful script ends with
/// (testing plan: usage is mandatory on every turn).
pub fn scripted_usage() -> lca_protocol::Usage {
    lca_protocol::Usage {
        input: 100,
        output: 50,
        cache_read: 1000,
        cache_write: 20,
        cache_write_1h: 0,
        cost: 0.002,
        cost_input: 0.0,
        cost_cache_read: 0.0,
        cost_cache_write: 0.0,
        extras: Default::default(),
    }
}

/// The event script, chosen by the request's model string. Both modes
/// walk this same list, so their event streams are identical by
/// construction (NFR-25).
pub fn scripted_events(model: &str) -> Vec<lca_protocol::StreamEvent> {
    use lca_protocol::StreamEvent as E;
    let usage = || E::Usage {
        usage: scripted_usage(),
    };
    match model {
        // FR-PROV-7's shape: start strictly before every delta.
        "conformance-tool" => vec![
            E::ToolCallStart {
                call_id: "c1".to_string(),
                name: "read".to_string(),
            },
            E::ToolCallArgDelta {
                call_id: "c1".to_string(),
                delta: "{\"path\":\"".to_string(),
            },
            E::ToolCallArgDelta {
                call_id: "c1".to_string(),
                delta: "notes.txt\"}".to_string(),
            },
            E::ToolCallEnd {
                call_id: "c1".to_string(),
            },
            usage(),
        ],
        // FR-PROV-8's shape: a delta with no open start, which the host's
        // accumulator must discard and record as a protocol error.
        "conformance-orphan-delta" => vec![
            E::ToolCallArgDelta {
                call_id: "ghost".to_string(),
                delta: "{}".to_string(),
            },
            usage(),
        ],
        // The reserved escape hatch (ABI `vendor-event`).
        "conformance-vendor" => vec![
            E::VendorEvent {
                kind: "image.generate".to_string(),
                payload: serde_json::json!({ "size": "1024x1024" }),
            },
            usage(),
        ],
        // Ends with a typed error, so no usage follows.
        "conformance-error" => vec![E::Error {
            message: "scripted failure".to_string(),
            retryable: false,
        }],
        // The default text/reasoning script.
        _ => vec![
            E::TextDelta {
                delta: "Hel".to_string(),
            },
            E::TextDelta {
                delta: "lo".to_string(),
            },
            E::ReasoningDelta {
                delta: "reason".to_string(),
            },
            usage(),
        ],
    }
}

/// The compaction script: a body carrying `call-completion` makes the
/// strategy ask the host through the `completion` capability (whose
/// denial must surface as the refusal - FR-PERM-3's completion case);
/// anything else gets the mechanical summary both modes must agree on.
pub fn compact_script(
    excerpts: &[(String, String)],
    ask: Option<&dyn Fn() -> Result<String, String>>,
) -> Result<String, String> {
    let wants_model = excerpts
        .iter()
        .any(|(_kind, body)| body.contains("call-completion"));
    if wants_model {
        match ask {
            Some(ask) => ask(),
            None => Err("completion is not available".to_string()),
        }
    } else {
        Ok(format!("conformance compacted {} records", excerpts.len()))
    }
}

/// The transform script: a rejection marker rejects, an injection
/// marker appends one message, everything else passes through - both
/// modes run this exact decision (NFR-25).
pub fn transform_script(
    mut messages: Vec<lca_protocol::ChatMessage>,
) -> Result<Vec<lca_protocol::ChatMessage>, String> {
    let text: String = messages
        .iter()
        .flat_map(|message| message.content.iter())
        .filter_map(|block| match block {
            lca_protocol::ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" ");
    if text.contains("conformance-reject") {
        return Err("conformance transform rejection".to_string());
    }
    if text.contains("conformance-inject") {
        messages.push(lca_protocol::ChatMessage::text(
            lca_protocol::MessageRole::System,
            "[conformance injected] instructions",
        ));
    }
    Ok(messages)
}

/// Render the candidate records for the compaction script (the same
/// `(kind, body)` pair in both modes).
pub fn compact_excerpts(records: &[lca_protocol::Record]) -> Vec<(String, String)> {
    records
        .iter()
        .map(|record| {
            (
                record.type_tag().to_string(),
                serde_json::to_string(record).unwrap_or_default(),
            )
        })
        .collect()
}

/// The scripted tree for each region - identical in both modes
/// (NFR-25). The footer carries an escape-sequence span on purpose:
/// it is the hostile-extension fixture, and the host must render those
/// bytes literally (FR-UI-2, ADR-0003).
pub fn ui_script(region: &str) -> Option<Vec<lca_protocol::Widget>> {
    use lca_protocol::Widget;
    let text = |content: &str, role: &str| Widget::Text {
        content: content.to_string(),
        role: role.to_string(),
    };
    Some(match region {
        "status-line" => vec![text("conformance", "accent")],
        // The footer is the vocabulary page: every widget kind the ABI
        // carries, arena-style from one root, with the hostile bytes as
        // a real escape character - the freeze gate says conformance
        // covers every surface, so the widget variant's cases all cross
        // here in both modes.
        "footer" => vec![
            Widget::Column(vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13]),
            text("hostile: \u{1b}[31mNOT A PROMPT\u{1b}[0m", "warning"),
            Widget::Row(vec![3, 4]),
            text("row-left", "muted"),
            text("row-right", "muted"),
            Widget::Spinner {
                frames: "\u{280b}\u{2819}".to_string(),
            },
            Widget::Progress {
                label: "conformance".to_string(),
                fill: 0.5,
            },
            Widget::Image {
                media_type: "image/png".to_string(),
                bytes: vec![1, 2, 3, 4],
            },
            Widget::Vendor("conformance.demo".to_string()),
            // The 0.6 vocabulary page (gh #172): dual-channel hex
            // plus a hostile-role twin the host must degrade, not
            // paint.
            Widget::StyledText {
                content: "styled \u{1b}[32mhex".to_string(),
                style: lca_protocol::TextStyle {
                    fg: Some("#50fa7b".to_string()),
                    bg: Some("#282a36".to_string()),
                    bold: true,
                    dim: false,
                    italic: false,
                    underline: true,
                },
            },
            Widget::Markdown {
                source: "# conformance\n\n- one\n- two".to_string(),
            },
            Widget::Button {
                id: "ok".to_string(),
                label: "OK".to_string(),
            },
            Widget::Table {
                headers: vec!["name".to_string(), "value".to_string()],
                rows: vec![vec!["a".to_string(), "1".to_string()]],
            },
            Widget::ScrollContainer {
                max_height: 2,
                children: vec![10, 12],
            },
        ],
        "panel" => vec![Widget::KeyValue(vec![
            ("mode".to_string(), "stateless".to_string()),
            ("arena".to_string(), "node0 is the root".to_string()),
        ])],
        // The modal pairs the two remaining cases: a boxed child and a
        // column beneath it.
        "modal" => vec![
            Widget::Boxed {
                title: Some("conformance modal".to_string()),
                border: Some("accent".to_string()),
                background: None,
                child: 1,
            },
            Widget::Column(vec![2, 3]),
            text("modal body", "default"),
            Widget::KeyValue(vec![("dismiss".to_string(), "esc".to_string())]),
        ],
        _ => return None,
    })
}

/// The scripted response to one interaction, both modes.
pub fn ui_event_script(region: &str, input: &lca_protocol::UiInput) -> lca_protocol::UiEffect {
    use lca_protocol::{UiEffect, UiInput};
    if region == "modal" {
        return match input {
            UiInput::Key { key } if key == "q" => UiEffect::CloseModal,
            UiInput::Key { key } if key == "m" => UiEffect::OpenModal,
            UiInput::Submit { text } => UiEffect::ShowNotice(format!("heard: {text}")),
            UiInput::Cancel => UiEffect::CloseModal,
            UiInput::ClickWidget { id } => UiEffect::ShowNotice(format!("clicked: {id}")),
            UiInput::Click { .. } | UiInput::Scroll { .. } | UiInput::Key { .. } => UiEffect::None,
        };
    }
    UiEffect::None
}

/// `login`: exercise the credentials round trip and the full oauth
/// begin/await/end flow through whichever mode's [`IdentityCap`] is supplied,
/// then report a deterministic outcome (NFR-25: both modes produce the same
/// `IdentityOutcome` for the same injected callback).
pub fn scripted_login(cap: &dyn IdentityCap) -> lca_protocol::IdentityOutcome {
    use lca_protocol::IdentityOutcome;
    let credentials = (|| -> Result<(), CapabilityError> {
        cap.credentials_set("probe", "1")?;
        match cap.credentials_get("probe")? {
            Some(value) if value == "1" => {}
            other => {
                return Err(CapabilityError::Io(format!(
                    "credential read back {other:?}, expected \"1\""
                )));
            }
        }
        cap.credentials_delete("probe")?;
        if cap.credentials_get("probe")?.is_some() {
            return Err(CapabilityError::Io(
                "credential survived delete".to_string(),
            ));
        }
        Ok(())
    })();
    if let Err(err) = credentials {
        return IdentityOutcome::Failed(err.to_string());
    }
    let (url, handle) = match cap.oauth_begin(CALLBACK_PATH) {
        Ok(pair) => pair,
        Err(err) => return IdentityOutcome::Failed(err.to_string()),
    };
    if let Err(err) = cap.oauth_open(AUTHORIZE_URL) {
        let _ = cap.oauth_end(handle);
        return IdentityOutcome::Failed(err.to_string());
    }
    let params = match cap.oauth_await(handle) {
        Ok(params) => params,
        Err(err) => {
            let _ = cap.oauth_end(handle);
            return IdentityOutcome::Failed(err.to_string());
        }
    };
    if let Err(err) = cap.oauth_end(handle) {
        return IdentityOutcome::Failed(err.to_string());
    }
    let code = params
        .iter()
        .find(|(key, _)| key == "code")
        .map(|(_, value)| value.as_str());
    if url.starts_with("http://127.0.0.1:") && code == Some(FIXTURE_CODE) {
        IdentityOutcome::Ok
    } else {
        IdentityOutcome::Failed(format!("unexpected oauth callback at {url}: {params:?}"))
    }
}

/// `logout` is this provider's not-supported case.
pub fn scripted_logout() -> lca_protocol::IdentityOutcome {
    lca_protocol::IdentityOutcome::NotSupported
}

/// The standard usage shape the generic `/usage` prints.
pub fn scripted_usage_report() -> lca_protocol::Usage {
    lca_protocol::Usage {
        input: 700,
        output: 70,
        cache_read: 7000,
        cache_write: 0,
        cache_write_1h: 0,
        cost: 0.007,
        cost_input: 0.0,
        cost_cache_read: 0.0,
        cost_cache_write: 0.0,
        extras: Default::default(),
    }
}

/// The login options the conformance probe reports (ADR-0033): a fixed
/// pair so both delivery modes are compared on identical data.
pub fn scripted_login_options() -> Vec<lca_protocol::LoginOption> {
    vec![
        lca_protocol::LoginOption {
            id: "conformance".to_string(),
            name: "Conformance".to_string(),
            kind: "api-key".to_string(),
            host: "conformance.example.com".to_string(),
            fields: vec!["api-key".to_string()],
            extras: Default::default(),
        },
        lca_protocol::LoginOption {
            id: "local".to_string(),
            name: "Local".to_string(),
            kind: "api-key".to_string(),
            host: "localhost".to_string(),
            fields: vec!["api-key".to_string()],
            extras: Default::default(),
        },
    ]
}

/// Consume one answer the way a real provider would: return the opaque
/// settings the host persists (ADR-0033).
pub fn scripted_login_submit(
    answer: &lca_protocol::LoginAnswer,
) -> Result<Vec<(String, String)>, String> {
    if answer.choice.is_empty() {
        return Err("no choice given".to_string());
    }
    let key = answer.value("api-key").unwrap_or_default();
    Ok(vec![
        (
            "base_url".to_string(),
            format!("https://{}/v1", answer.choice),
        ),
        ("key_len".to_string(), key.len().to_string()),
    ])
}

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

#[cfg(target_arch = "wasm32")]
#[allow(unsafe_code)] // generated wit-bindgen export shims (see crate docs)
mod command_world;

#[cfg(target_arch = "wasm32")]
#[allow(unsafe_code)] // generated wit-bindgen export shims (see crate docs)
mod hooks_world;

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
