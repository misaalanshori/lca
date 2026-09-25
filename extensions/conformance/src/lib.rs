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
        extras: Default::default(),
    }
}

// ---------------------------------------------------------------------------
// Provider world: one script, both modes (the Phase 3 half of NFR-25)
// ---------------------------------------------------------------------------

/// The model list both modes return (FR-PROV-2).
pub fn provider_models() -> Vec<lca_protocol::ModelInfo> {
    vec![
        lca_protocol::ModelInfo {
            id: "conformance-a".to_string(),
            name: "Conformance A".to_string(),
            context_window: 4096,
            max_tokens: 512,
        },
        lca_protocol::ModelInfo {
            id: "conformance-b".to_string(),
            name: "Conformance B".to_string(),
            context_window: 8192,
            max_tokens: 1024,
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
            Widget::Column(vec![1, 2, 3, 4, 5, 6, 7]),
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
            _ => UiEffect::None,
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

// ---------------------------------------------------------------------------
// Native delivery mode (compiled into the host, unsandboxed, labeled so)
// ---------------------------------------------------------------------------

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use super::*;
    use lca_tools::Capabilities;
    use std::sync::Arc;

    /// The native twin of the guest's `Cap`: it calls the same
    /// [`Capabilities`] engine the WASM host's imports call.
    pub struct NativeCap(pub Arc<Capabilities>);

    impl Cap for NativeCap {
        fn fs_read(&self, scope: &str, path: &str) -> Result<Vec<u8>, CapabilityError> {
            self.0.fs_read(scope, path)
        }
        fn fs_write(&self, scope: &str, path: &str, bytes: &[u8]) -> Result<(), CapabilityError> {
            self.0.fs_write(scope, path, bytes)
        }
        fn fs_stat(&self, scope: &str, path: &str) -> Result<(bool, u64), CapabilityError> {
            self.0.fs_stat(scope, path)
        }
        fn fs_list(&self, scope: &str, path: &str) -> Result<Vec<String>, CapabilityError> {
            self.0.fs_list(scope, path)
        }
        fn process_spawn(
            &self,
            program: &str,
            args: &[String],
            cwd: &str,
        ) -> Result<u32, CapabilityError> {
            self.0.process_spawn(program, args, cwd)
        }
        fn process_read_stdout(
            &self,
            handle: u32,
            max: usize,
        ) -> Result<Option<Vec<u8>>, CapabilityError> {
            self.0.process_read_stdout(handle, max)
        }
        fn process_read_stderr(
            &self,
            handle: u32,
            max: usize,
        ) -> Result<Option<Vec<u8>>, CapabilityError> {
            self.0.process_read_stderr(handle, max)
        }
        fn process_write_stdin(&self, handle: u32, bytes: &[u8]) -> Result<u64, CapabilityError> {
            self.0.process_write_stdin(handle, bytes)
        }
        fn process_wait(&self, handle: u32) -> Result<i32, CapabilityError> {
            self.0.process_wait(handle)
        }
        fn process_kill(&self, handle: u32) -> Result<(), CapabilityError> {
            self.0.process_kill(handle)
        }
        fn pty_spawn(
            &self,
            program: &str,
            args: &[String],
            cwd: &str,
            rows: u16,
            cols: u16,
        ) -> Result<u32, CapabilityError> {
            self.0.pty_spawn(program, args, cwd, rows, cols)
        }
        fn pty_read(&self, handle: u32, max: usize) -> Result<Option<Vec<u8>>, CapabilityError> {
            self.0.pty_read(handle, max)
        }
        fn pty_write(&self, handle: u32, bytes: &[u8]) -> Result<u64, CapabilityError> {
            self.0.pty_write(handle, bytes)
        }
        fn pty_resize(&self, handle: u32, rows: u16, cols: u16) -> Result<(), CapabilityError> {
            self.0.pty_resize(handle, rows, cols)
        }
        fn pty_wait(&self, handle: u32) -> Result<i32, CapabilityError> {
            self.0.pty_wait(handle)
        }
        fn pty_kill(&self, handle: u32) -> Result<(), CapabilityError> {
            self.0.pty_kill(handle)
        }
    }

    impl crate::IdentityCap for NativeCap {
        fn credentials_set(&self, key: &str, value: &str) -> Result<(), CapabilityError> {
            self.0.credentials_set(key, value)
        }
        fn credentials_get(&self, key: &str) -> Result<Option<String>, CapabilityError> {
            self.0.credentials_get(key)
        }
        fn credentials_delete(&self, key: &str) -> Result<(), CapabilityError> {
            self.0.credentials_delete(key)
        }
        fn oauth_begin(&self, redirect_path: &str) -> Result<(String, u32), CapabilityError> {
            self.0.oauth_begin(redirect_path)
        }
        fn oauth_open(&self, url: &str) -> Result<(), CapabilityError> {
            self.0.oauth_open(url)
        }
        fn oauth_await(&self, handle: u32) -> Result<Vec<(String, String)>, CapabilityError> {
            self.0.oauth_await(handle)
        }
        fn oauth_end(&self, handle: u32) -> Result<(), CapabilityError> {
            self.0.oauth_end(handle)
        }
    }

    /// The native-linked conformance extension: same schema, same shared
    /// dispatch, same capability engine (FR-EXT-6).
    pub struct NativeConformance {
        cap: Arc<Capabilities>,
    }

    impl NativeConformance {
        /// Wrap the extension's capability engine.
        pub fn new(cap: Arc<Capabilities>) -> NativeConformance {
            NativeConformance { cap }
        }

        /// The tool schema (identical to the guest's).
        pub fn schema(&self) -> lca_protocol::ToolSpec {
            let (name, description, parameters) = schema_json();
            lca_protocol::ToolSpec {
                name,
                description,
                parameters: serde_json::from_str(&parameters).expect("schema is json"),
                extras: Default::default(),
            }
        }

        /// Execute one call through the shared dispatch. Guest-only modes
        /// report unknown here; the conformance diff never uses them.
        pub fn execute(&self, call: &ToolCall) -> ToolResult {
            let (mode, args) = mode_and_args(&call.arguments);
            let outcome = run_shared(&NativeCap(self.cap.clone()), &mode, &args);
            outcome_to_result(&call.call_id, outcome)
        }
    }

    impl lca_ext_abi::ExtensionDispatch for NativeConformance {
        fn name(&self) -> &str {
            "conformance"
        }

        fn delivery(&self) -> lca_ext_abi::DeliveryMode {
            lca_ext_abi::DeliveryMode::Native
        }

        fn worlds(&self) -> Vec<lca_ext_abi::World> {
            vec![
                lca_ext_abi::World::Tool,
                lca_ext_abi::World::Command,
                lca_ext_abi::World::Hooks,
                lca_ext_abi::World::Provider,
                lca_ext_abi::World::Compaction,
                lca_ext_abi::World::ContextTransform,
                lca_ext_abi::World::Ui,
            ]
        }

        fn ui_regions(&self) -> Vec<String> {
            vec![
                "status-line".to_string(),
                "footer".to_string(),
                "panel".to_string(),
                "modal".to_string(),
            ]
        }

        fn render(
            &self,
            region: &str,
        ) -> Result<Option<lca_protocol::WidgetTree>, lca_protocol::DispatchError> {
            Ok(crate::ui_script(region).map(|nodes| lca_protocol::WidgetTree { nodes }))
        }

        fn on_ui_event(
            &self,
            region: &str,
            input: &lca_protocol::UiInput,
        ) -> Result<lca_protocol::UiEffect, lca_protocol::DispatchError> {
            Ok(crate::ui_event_script(region, input))
        }

        fn compact(
            &self,
            records: &[lca_protocol::Record],
        ) -> lca_ext_abi::DispatchFuture<'static, Result<String, lca_protocol::DispatchError>>
        {
            let excerpts = compact_excerpts(records);
            let cap = self.cap.clone();
            Box::pin(async move {
                tokio::task::spawn_blocking(move || {
                    let ask = || -> Result<String, String> {
                        let messages = vec![lca_protocol::ChatMessage::text(
                            lca_protocol::MessageRole::User,
                            "conformance completion request",
                        )];
                        cap.complete(messages)
                            .map(|(text, _usage)| text)
                            .map_err(|err| err.to_string())
                    };
                    compact_script(&excerpts, Some(&ask as &dyn Fn() -> Result<String, String>))
                        .map_err(|reason| {
                            // Mirror the host's wrapping byte for byte so
                            // the refusal reads identically in both modes
                            // (NFR-25): to_dispatch prefixes the extension
                            // and the invalid-arguments class, the host's
                            // compact work prefixes the refusal.
                            lca_protocol::DispatchError::Failed(format!(
                                "{}: invalid arguments: compaction refused: {reason}",
                                "conformance"
                            ))
                        })
                })
                .await
                .map_err(|_| lca_protocol::DispatchError::Failed("conformance panicked".into()))
                .and_then(std::convert::identity)
            })
        }

        fn transform_messages(
            &self,
            messages: Vec<lca_protocol::ChatMessage>,
        ) -> lca_ext_abi::DispatchFuture<
            'static,
            Result<Result<Vec<lca_protocol::ChatMessage>, String>, lca_protocol::DispatchError>,
        > {
            Box::pin(std::future::ready(Ok(transform_script(messages))))
        }

        fn provider_models(
            &self,
        ) -> Result<Vec<lca_protocol::ModelInfo>, lca_protocol::DispatchError> {
            Ok(crate::provider_models())
        }

        fn stream_completion<'a>(
            &'a self,
            request: lca_protocol::CompletionRequest,
            sink: &'a dyn lca_protocol::EventSink,
        ) -> lca_ext_abi::DispatchFuture<'a, Result<(), lca_protocol::DispatchError>> {
            // The script is deterministic, so both modes push the same
            // events in the same order before completing (NFR-25).
            for event in crate::scripted_events(&request.model) {
                sink.push(event);
            }
            Box::pin(std::future::ready(Ok(())))
        }

        fn identity_login(
            &self,
        ) -> lca_ext_abi::DispatchFuture<
            'static,
            Result<lca_protocol::IdentityOutcome, lca_protocol::DispatchError>,
        > {
            let cap = self.cap.clone();
            // Lazy: the oauth flow blocks in `oauth_await`, so the caller
            // must be able to run this future off the test thread.
            Box::pin(async move { Ok(crate::scripted_login(&NativeCap(cap))) })
        }

        fn identity_logout(
            &self,
        ) -> lca_ext_abi::DispatchFuture<
            'static,
            Result<lca_protocol::IdentityOutcome, lca_protocol::DispatchError>,
        > {
            Box::pin(std::future::ready(Ok(crate::scripted_logout())))
        }

        fn identity_usage(
            &self,
        ) -> lca_ext_abi::DispatchFuture<
            'static,
            Result<
                Result<lca_protocol::Usage, lca_protocol::IdentityOutcome>,
                lca_protocol::DispatchError,
            >,
        > {
            Box::pin(std::future::ready(Ok(Ok(crate::scripted_usage_report()))))
        }

        fn interrupt(&self) {
            // A native call shares the caller's thread, so there is no epoch
            // to bump: flag the capability engine directly, and a blocked
            // host wait (the oauth callback) polls its way out (FR-CONC-1,
            // NFR-21). This is the pattern a native extension with a
            // blocking host wait must follow.
            self.cap.cancel();
        }

        fn tool_specs(&self) -> Result<Vec<lca_protocol::ToolSpec>, lca_protocol::DispatchError> {
            Ok(vec![self.schema()])
        }

        fn execute_tool<'a>(
            &'a self,
            call: &'a ToolCall,
        ) -> lca_ext_abi::DispatchFuture<
            'a,
            Result<lca_protocol::ToolResult, lca_protocol::DispatchError>,
        > {
            let result = self.execute(call);
            Box::pin(std::future::ready(Ok(result)))
        }

        fn command_specs(
            &self,
        ) -> Result<Vec<lca_protocol::CommandSpec>, lca_protocol::DispatchError> {
            Ok(vec![command_leaf()])
        }

        fn invoke_command(
            &self,
            _name: &str,
            argument: &str,
        ) -> Result<lca_protocol::CommandEffect, lca_protocol::DispatchError> {
            Ok(invoke_command(argument))
        }

        fn on_pre_tool_use<'a>(
            &'a self,
            call: &'a ToolCall,
        ) -> lca_ext_abi::DispatchFuture<
            'a,
            Result<lca_protocol::HookAction, lca_protocol::DispatchError>,
        > {
            let action = pre_tool_action(&call.name);
            Box::pin(std::future::ready(Ok(action)))
        }

        fn on_pre_turn(
            &self,
        ) -> lca_ext_abi::DispatchFuture<'static, Result<(), lca_protocol::DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }

        fn on_post_tool_use<'a>(
            &'a self,
            _observation: &'a lca_protocol::PostToolObservation,
        ) -> lca_ext_abi::DispatchFuture<'a, Result<(), lca_protocol::DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }

        fn on_post_turn_end<'a>(
            &'a self,
            _status: &'a str,
        ) -> lca_ext_abi::DispatchFuture<'a, Result<(), lca_protocol::DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }

        fn on_attention_required<'a>(
            &'a self,
            _reason: &'a str,
        ) -> lca_ext_abi::DispatchFuture<'a, Result<(), lca_protocol::DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }

        fn on_session_close(
            &self,
        ) -> lca_ext_abi::DispatchFuture<'static, Result<(), lca_protocol::DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }
    }
}

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
mod tool_world {
    wit_bindgen::generate!({
        path: "../../wit",
        world: "tool",
        export_macro_name: "export_tool",
        with: {
            "lca:host/log@0.2.0": generate,
            "lca:host/fs@0.2.0": generate,
            "lca:host/process@0.2.0": generate,
            "lca:host/pty@0.2.0": generate,
        },
    });

    use lca::ext::types::ToolCall;
    use lca::host::{fs, process, pty};

    use crate::{Cap, ModeOutcome, mode_and_args, run_shared, schema_json};
    use exports::lca::ext::execute::Guest as ExecuteTrait;
    use exports::lca::ext::execute::ToolResult as WasmResult;
    use exports::lca::ext::tool_schema::{Guest as SchemaGuest, Schema};
    use lca::host::log;

    fn map_fs(err: fs::Error) -> crate::CapabilityError {
        use crate::CapabilityError as E;
        match err {
            fs::Error::Permission(d) => E::Permission(d),
            fs::Error::NotGranted(d) => E::NotGranted(d),
            fs::Error::NotFound(d) => E::NotFound(d),
            fs::Error::Io(d) => E::Io(d),
            fs::Error::Invalid(d) => E::Invalid(d),
        }
    }

    fn map_process(err: process::Error) -> crate::CapabilityError {
        use crate::CapabilityError as E;
        match err {
            process::Error::Permission(d) => E::Permission(d),
            process::Error::NotGranted(d) => E::NotGranted(d),
            process::Error::NotFound(d) => E::NotFound(d),
            process::Error::Io(d) => E::Io(d),
            process::Error::Invalid(d) => E::Invalid(d),
        }
    }

    fn map_pty(err: pty::Error) -> crate::CapabilityError {
        use crate::CapabilityError as E;
        match err {
            pty::Error::Permission(d) => E::Permission(d),
            pty::Error::NotGranted(d) => E::NotGranted(d),
            pty::Error::NotFound(d) => E::NotFound(d),
            pty::Error::Io(d) => E::Io(d),
            pty::Error::Invalid(d) => E::Invalid(d),
        }
    }

    /// The guest's capability view: host imports behind every call.
    struct GuestCap;

    impl Cap for GuestCap {
        fn fs_read(&self, scope: &str, path: &str) -> Result<Vec<u8>, crate::CapabilityError> {
            fs::read(scope, path).map_err(map_fs)
        }
        fn fs_write(
            &self,
            scope: &str,
            path: &str,
            bytes: &[u8],
        ) -> Result<(), crate::CapabilityError> {
            fs::write(scope, path, bytes).map_err(map_fs)
        }
        fn fs_stat(&self, scope: &str, path: &str) -> Result<(bool, u64), crate::CapabilityError> {
            let info = fs::stat(scope, path).map_err(map_fs)?;
            Ok((info.is_dir, info.len))
        }
        fn fs_list(&self, scope: &str, path: &str) -> Result<Vec<String>, crate::CapabilityError> {
            fs::list_entries(scope, path).map_err(map_fs)
        }
        fn process_spawn(
            &self,
            program: &str,
            args: &[String],
            cwd: &str,
        ) -> Result<u32, crate::CapabilityError> {
            process::spawn(program, args, cwd).map_err(map_process)
        }
        fn process_read_stdout(
            &self,
            handle: u32,
            max: usize,
        ) -> Result<Option<Vec<u8>>, crate::CapabilityError> {
            process::read_stdout(handle, max as u64).map_err(map_process)
        }
        fn process_read_stderr(
            &self,
            handle: u32,
            max: usize,
        ) -> Result<Option<Vec<u8>>, crate::CapabilityError> {
            process::read_stderr(handle, max as u64).map_err(map_process)
        }
        fn process_write_stdin(
            &self,
            handle: u32,
            bytes: &[u8],
        ) -> Result<u64, crate::CapabilityError> {
            process::write_stdin(handle, bytes).map_err(map_process)
        }
        fn process_wait(&self, handle: u32) -> Result<i32, crate::CapabilityError> {
            process::wait(handle).map_err(map_process)
        }
        fn process_kill(&self, handle: u32) -> Result<(), crate::CapabilityError> {
            process::kill(handle).map_err(map_process)
        }
        fn pty_spawn(
            &self,
            program: &str,
            args: &[String],
            cwd: &str,
            rows: u16,
            cols: u16,
        ) -> Result<u32, crate::CapabilityError> {
            pty::spawn(program, args, cwd, rows, cols).map_err(map_pty)
        }
        fn pty_read(
            &self,
            handle: u32,
            max: usize,
        ) -> Result<Option<Vec<u8>>, crate::CapabilityError> {
            pty::read(handle, max as u64).map_err(map_pty)
        }
        fn pty_write(&self, handle: u32, bytes: &[u8]) -> Result<u64, crate::CapabilityError> {
            pty::write(handle, bytes).map_err(map_pty)
        }
        fn pty_resize(
            &self,
            handle: u32,
            rows: u16,
            cols: u16,
        ) -> Result<(), crate::CapabilityError> {
            pty::resize(handle, rows, cols).map_err(map_pty)
        }
        fn pty_wait(&self, handle: u32) -> Result<i32, crate::CapabilityError> {
            pty::wait(handle).map_err(map_pty)
        }
        fn pty_kill(&self, handle: u32) -> Result<(), crate::CapabilityError> {
            pty::kill(handle).map_err(map_pty)
        }
    }

    pub struct ToolComponent;

    impl SchemaGuest for ToolComponent {
        fn get_schema() -> Schema {
            let (name, description, parameters) = schema_json();
            Schema {
                name,
                description,
                parameters,
                extras: Vec::new(),
            }
        }
    }

    impl ExecuteTrait for ToolComponent {
        fn run(call: ToolCall) -> WasmResult {
            let (mode, args) = mode_and_args(&call.arguments);
            let outcome = match mode.as_str() {
                "trap" => panic!("conformance trap requested"),
                "loop" => loop {
                    std::hint::spin_loop();
                },
                "log" => {
                    log::info(&"x".repeat(50_000));
                    ModeOutcome {
                        ok: true,
                        text: "logged".to_string(),
                    }
                }
                "alloc" => {
                    let mut hog: Vec<Vec<u8>> = Vec::new();
                    for i in 0..64u64 {
                        let mut block = vec![0u8; 4 * 1024 * 1024];
                        block[0] = i as u8;
                        hog.push(block);
                    }
                    ModeOutcome {
                        ok: true,
                        text: "allocated".to_string(),
                    }
                }
                _ => run_shared(&GuestCap, &mode, &args),
            };
            WasmResult {
                call_id: call.call_id,
                status: if outcome.ok { "ok" } else { "error" }.to_string(),
                content: Some(outcome.text),
                truncated: false,
                extras: Vec::new(),
            }
        }
    }

    export_tool!(ToolComponent);
}

#[cfg(target_arch = "wasm32")]
#[allow(unsafe_code)] // generated wit-bindgen export shims (see crate docs)
mod command_world {
    wit_bindgen::generate!({
        path: "../../wit",
        world: "command",
        export_macro_name: "export_command",
    });

    use exports::lca::ext::command_spec::{Guest as SpecGuest, Spec};
    use exports::lca::ext::invoke::{Effect, Guest as InvokeGuest};

    pub struct CommandComponent;

    impl SpecGuest for CommandComponent {
        fn get_spec() -> Spec {
            let leaf = crate::command_leaf();
            Spec {
                name: leaf.name,
                hint: leaf.hint,
                completion: leaf.completion,
                extras: Vec::new(),
            }
        }
    }

    impl InvokeGuest for CommandComponent {
        fn run(argument: String) -> Effect {
            match crate::invoke_command(&argument) {
                lca_protocol::CommandEffect::InsertText(text) => Effect::InsertText(text),
                lca_protocol::CommandEffect::SubmitPrompt(text) => Effect::SubmitPrompt(text),
                lca_protocol::CommandEffect::ShowWidget(text) => Effect::ShowWidget(text),
                lca_protocol::CommandEffect::None => Effect::None,
            }
        }
    }

    export_command!(CommandComponent);
}

#[cfg(target_arch = "wasm32")]
#[allow(unsafe_code)] // generated wit-bindgen export shims (see crate docs)
mod hooks_world {
    wit_bindgen::generate!({
        path: "../../wit",
        world: "hooks",
        export_macro_name: "export_hooks",
    });

    use exports::lca::ext::hook_attention_required::Guest as AttentionGuest;
    use exports::lca::ext::hook_post_tool_use::{
        Guest as PostToolGuest, ToolCall as PostCall, ToolResult as PostResult,
    };
    use exports::lca::ext::hook_post_turn_end::Guest as PostTurnGuest;
    use exports::lca::ext::hook_pre_tool_use::{Action, Guest as PreToolGuest, ToolCall};
    use exports::lca::ext::hook_pre_turn::Guest as PreTurnGuest;
    use exports::lca::ext::hook_session_close::Guest as CloseGuest;

    pub struct HooksComponent;

    impl PreTurnGuest for HooksComponent {
        fn on_pre_turn() {}
    }

    impl PreToolGuest for HooksComponent {
        fn on_pre_tool_use(call: ToolCall) -> Action {
            // One policy source (crate::pre_tool_action), mapped to the
            // WIT variant: the native twin runs the identical decision.
            match crate::pre_tool_action(&call.name) {
                lca_protocol::HookAction::Allow => Action::Allow,
                lca_protocol::HookAction::Deny(reason) => Action::Deny(reason),
                lca_protocol::HookAction::Replace(replacement) => Action::Replace(ToolCall {
                    call_id: replacement.call_id,
                    name: replacement.name,
                    arguments: replacement.arguments,
                    extras: Vec::new(),
                }),
            }
        }
    }

    impl PostToolGuest for HooksComponent {
        fn on_post_tool_use(_call: PostCall, _outcome: PostResult) {}
    }

    impl PostTurnGuest for HooksComponent {
        fn on_post_turn_end(_status: String) {}
    }

    impl AttentionGuest for HooksComponent {
        fn on_attention_required(_reason: String) {}
    }

    impl CloseGuest for HooksComponent {
        fn on_session_close() {}
    }

    export_hooks!(HooksComponent);
}

// ---------------------------------------------------------------------------
// WASM delivery mode: the provider world
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
#[allow(unsafe_code)] // generated wit-bindgen export shims (see crate docs)
mod provider_world {
    wit_bindgen::generate!({
        path: "../../wit",
        world: "provider",
        export_macro_name: "export_provider",
        with: {
            "lca:host/log@0.2.0": generate,
            "lca:host/net@0.2.0": generate,
            "lca:host/oauth@0.2.0": generate,
            "lca:host/credentials@0.2.0": generate,
        },
    });

    use core::cell::RefCell;

    use exports::lca::ext::provider_completion::{CompletionRequest, CompletionStream};
    use exports::lca::ext::provider_completion::{
        Guest as CompletionGuest, GuestCompletionStream, StreamEvent as WasmEvent,
    };
    use exports::lca::ext::provider_identity::{
        Guest as IdentityGuest, IdentityOutcome as WasmOutcome, TokenUsage,
    };
    use exports::lca::ext::provider_models::{Guest as ModelsGuest, ModelInfo as WasmModel};
    use lca::ext::types::{ExtraPair, Usage as WasmUsage};
    use lca::host::{credentials, oauth};

    fn map_credentials(err: credentials::Error) -> crate::CapabilityError {
        use crate::CapabilityError as E;
        match err {
            credentials::Error::Permission(d) => E::Permission(d),
            credentials::Error::NotGranted(d) => E::NotGranted(d),
            credentials::Error::Io(d) => E::Io(d),
            credentials::Error::Invalid(d) => E::Invalid(d),
        }
    }

    fn map_oauth(err: oauth::Error) -> crate::CapabilityError {
        use crate::CapabilityError as E;
        match err {
            oauth::Error::Permission(d) => E::Permission(d),
            oauth::Error::NotGranted(d) => E::NotGranted(d),
            oauth::Error::Timeout(d) => E::Timeout(d),
            oauth::Error::Io(d) => E::Io(d),
            oauth::Error::Invalid(d) => E::Invalid(d),
        }
    }

    /// The guest's credentials/oauth view: the provider world's host
    /// imports behind every call (the [`crate::IdentityCap`] counterpart of
    /// the tool world's `GuestCap`).
    struct GuestIdentityCap;

    impl crate::IdentityCap for GuestIdentityCap {
        fn credentials_set(&self, key: &str, value: &str) -> Result<(), crate::CapabilityError> {
            credentials::set(key, value).map_err(map_credentials)
        }
        fn credentials_get(&self, key: &str) -> Result<Option<String>, crate::CapabilityError> {
            Ok(credentials::get(key))
        }
        fn credentials_delete(&self, key: &str) -> Result<(), crate::CapabilityError> {
            credentials::delete(key).map_err(map_credentials)
        }
        fn oauth_begin(
            &self,
            redirect_path: &str,
        ) -> Result<(String, u32), crate::CapabilityError> {
            oauth::begin(redirect_path).map_err(map_oauth)
        }
        fn oauth_open(&self, url: &str) -> Result<(), crate::CapabilityError> {
            oauth::open(url).map_err(map_oauth)
        }
        fn oauth_await(
            &self,
            handle: u32,
        ) -> Result<Vec<(String, String)>, crate::CapabilityError> {
            oauth::await_callback(handle).map_err(map_oauth)
        }
        fn oauth_end(&self, handle: u32) -> Result<(), crate::CapabilityError> {
            oauth::end_flow(handle).map_err(map_oauth)
        }
    }

    /// Protocol usage -> the WIT record (cost buckets in reserved extras,
    /// matching the host's conversion exactly for NFR-25 parity).
    fn to_wit_usage(usage: &lca_protocol::Usage) -> WasmUsage {
        let mut extras: Vec<ExtraPair> = usage
            .extras
            .iter()
            .map(|(key, value)| ExtraPair {
                key: key.clone(),
                value: value.clone(),
            })
            .collect();
        for (key, value) in [
            ("cost_input", usage.cost_input),
            ("cost_cache_read", usage.cost_cache_read),
            ("cost_cache_write", usage.cost_cache_write),
        ] {
            if value != 0.0 {
                extras.push(ExtraPair {
                    key: key.to_string(),
                    value: value.to_string(),
                });
            }
        }
        WasmUsage {
            input: usage.input,
            output: usage.output,
            cache_read: usage.cache_read,
            cache_write: usage.cache_write,
            cache_write_hour: usage.cache_write_1h,
            cost: usage.cost,
            extras,
        }
    }

    fn to_wit_event(event: lca_protocol::StreamEvent) -> WasmEvent {
        use lca_protocol::StreamEvent as P;
        match event {
            P::TextDelta { delta } => WasmEvent::TextDelta(delta),
            P::ReasoningDelta { delta } => WasmEvent::ReasoningDelta(delta),
            P::ToolCallStart { call_id, name } => WasmEvent::ToolCallStart((call_id, name)),
            P::ToolCallArgDelta { call_id, delta } => WasmEvent::ToolCallArgDelta((call_id, delta)),
            P::ToolCallEnd { call_id } => WasmEvent::ToolCallEnd(call_id),
            P::Usage { usage } => WasmEvent::Usage(to_wit_usage(&usage)),
            P::Error { message, retryable } => WasmEvent::Error((message, retryable)),
            P::VendorEvent { kind, payload } => WasmEvent::VendorEvent((kind, payload.to_string())),
        }
    }

    pub struct ProviderComponent;

    impl ModelsGuest for ProviderComponent {
        fn list_models() -> Vec<WasmModel> {
            crate::provider_models()
                .into_iter()
                .map(|model| WasmModel {
                    id: model.id,
                    name: model.name,
                    context_window: model.context_window,
                    max_tokens: model.max_tokens,
                    extras: Vec::new(),
                })
                .collect()
        }
    }

    /// The pull stream: the script precomputed, `next` walks it. The
    /// host polls from a task; no thread is dedicated to the call
    /// (ADR-0004).
    pub struct ScriptedStream {
        events: RefCell<std::vec::IntoIter<lca_protocol::StreamEvent>>,
    }

    impl GuestCompletionStream for ScriptedStream {
        fn next(&self) -> Option<WasmEvent> {
            self.events.borrow_mut().next().map(to_wit_event)
        }
    }

    impl CompletionGuest for ProviderComponent {
        type CompletionStream = ScriptedStream;

        fn stream_completion(request: CompletionRequest) -> Result<CompletionStream, String> {
            let events = crate::scripted_events(&request.model);
            Ok(CompletionStream::new(ScriptedStream {
                events: RefCell::new(events.into_iter()),
            }))
        }
    }

    impl IdentityGuest for ProviderComponent {
        fn login() -> WasmOutcome {
            match crate::scripted_login(&GuestIdentityCap) {
                lca_protocol::IdentityOutcome::Ok => WasmOutcome::Ok,
                lca_protocol::IdentityOutcome::NotSupported => WasmOutcome::NotSupported,
                lca_protocol::IdentityOutcome::Failed(reason) => WasmOutcome::Failed(reason),
            }
        }

        fn logout() -> WasmOutcome {
            match crate::scripted_logout() {
                lca_protocol::IdentityOutcome::Ok => WasmOutcome::Ok,
                lca_protocol::IdentityOutcome::NotSupported => WasmOutcome::NotSupported,
                lca_protocol::IdentityOutcome::Failed(reason) => WasmOutcome::Failed(reason),
            }
        }

        fn usage() -> Result<TokenUsage, WasmOutcome> {
            let usage = to_wit_usage(&crate::scripted_usage_report());
            Ok(TokenUsage {
                input: usage.input,
                output: usage.output,
                cache_read: usage.cache_read,
                cache_write: usage.cache_write,
                cache_write_hour: usage.cache_write_hour,
                cost: usage.cost,
                extras: usage.extras,
            })
        }
    }

    export_provider!(ProviderComponent);
}

// ---------------------------------------------------------------------------
// WASM delivery mode: the compaction world
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
#[allow(unsafe_code)] // generated wit-bindgen export shims
mod compaction_world {
    wit_bindgen::generate!({
        path: "../../wit",
        world: "compaction",
        export_macro_name: "export_compaction",
        with: {
            "lca:host/log@0.2.0": generate,
            "lca:host/completion@0.2.0": generate,
            "lca:host/types@0.2.0": generate,
        },
    });

    use exports::lca::ext::compact::{Guest as CompactGuest, SessionRecord};
    use lca::host::completion;

    fn to_capability(err: completion::Error) -> String {
        use lca_protocol::CapabilityError as E;
        match err {
            completion::Error::Permission(d) => E::Permission(d).to_string(),
            completion::Error::NotGranted(d) => E::NotGranted(d).to_string(),
            completion::Error::Io(d) => E::Io(d).to_string(),
            completion::Error::Invalid(d) => E::Invalid(d).to_string(),
        }
    }

    pub struct CompactionWasm;

    impl CompactGuest for CompactionWasm {
        fn compact(records: Vec<SessionRecord>) -> Result<String, String> {
            let excerpts: Vec<(String, String)> = records
                .into_iter()
                .map(|record| (record.kind, record.body))
                .collect();
            let ask = || -> Result<String, String> {
                let request = completion::Message {
                    role: "user".to_string(),
                    content: "conformance completion request".to_string(),
                    tool_calls: Vec::new(),
                    tool_call_id: None,
                    extras: Vec::new(),
                };
                let response = completion::request(&[request]).map_err(to_capability)?;
                Ok(response.text)
            };
            crate::compact_script(&excerpts, Some(&ask as &dyn Fn() -> Result<String, String>))
        }
    }

    export_compaction!(CompactionWasm);
}

// ---------------------------------------------------------------------------
// WASM delivery mode: the context-transform world
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
#[allow(unsafe_code)] // generated wit-bindgen export shims
mod transform_world {
    wit_bindgen::generate!({
        path: "../../wit",
        world: "context-transform",
        export_macro_name: "export_transform",
        with: {
            "lca:host/log@0.2.0": generate,
            "lca:host/fs@0.2.0": generate,
        },
    });

    use exports::lca::ext::transform::{Guest as TransformGuest, Message as WasmMessage};
    use lca::ext::types::ToolCall as WasmToolCall;

    pub struct TransformWasm;

    impl TransformGuest for TransformWasm {
        fn transform(messages: Vec<WasmMessage>) -> Result<Vec<WasmMessage>, String> {
            let protocol: Vec<lca_protocol::ChatMessage> = messages
                .iter()
                .map(|message| lca_protocol::ChatMessage {
                    role: match message.role.as_str() {
                        "system" => lca_protocol::MessageRole::System,
                        "user" => lca_protocol::MessageRole::User,
                        "assistant" => lca_protocol::MessageRole::Assistant,
                        _ => lca_protocol::MessageRole::Tool,
                    },
                    content: message
                        .content
                        .iter()
                        .map(|block| match block {
                            lca::ext::types::ContentBlock::Text(text) => {
                                lca_protocol::ContentBlock::Text { text: text.clone() }
                            }
                            lca::ext::types::ContentBlock::Image((media_type, bytes)) => {
                                lca_protocol::ContentBlock::Image {
                                    media_type: media_type.clone(),
                                    bytes: bytes.clone(),
                                }
                            }
                        })
                        .collect(),
                    tool_calls: message
                        .tool_calls
                        .iter()
                        .map(|call| lca_protocol::ToolCall {
                            call_id: call.call_id.clone(),
                            name: call.name.clone(),
                            arguments: call.arguments.clone(),
                        })
                        .collect(),
                    tool_call_id: message.tool_call_id.clone(),
                    usage: None,
                    extras: Default::default(),
                })
                .collect();
            let transformed = crate::transform_script(protocol)?;
            Ok(transformed
                .into_iter()
                .map(|message| WasmMessage {
                    role: match message.role {
                        lca_protocol::MessageRole::System => "system".to_string(),
                        lca_protocol::MessageRole::User => "user".to_string(),
                        lca_protocol::MessageRole::Assistant => "assistant".to_string(),
                        lca_protocol::MessageRole::Tool => "tool".to_string(),
                    },
                    content: message
                        .content
                        .iter()
                        .filter_map(|block| match block {
                            lca_protocol::ContentBlock::Text { text } => {
                                Some(lca::ext::types::ContentBlock::Text(text.clone()))
                            }
                            lca_protocol::ContentBlock::Image { media_type, bytes } => {
                                Some(lca::ext::types::ContentBlock::Image((
                                    media_type.clone(),
                                    bytes.clone(),
                                )))
                            }
                            // Reasoning and tool-call blocks never cross (the
                            // host filters them); drop them here too.
                            lca_protocol::ContentBlock::Reasoning { .. }
                            | lca_protocol::ContentBlock::ToolCall { .. } => None,
                        })
                        .collect(),
                    tool_calls: message
                        .tool_calls
                        .iter()
                        .map(|call| WasmToolCall {
                            call_id: call.call_id.clone(),
                            name: call.name.clone(),
                            arguments: call.arguments.clone(),
                            extras: Vec::new(),
                        })
                        .collect(),
                    tool_call_id: message.tool_call_id,
                    extras: Vec::new(),
                })
                .collect())
        }
    }

    export_transform!(TransformWasm);
}

// ---------------------------------------------------------------------------
// WASM delivery mode: the ui world
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
#[allow(unsafe_code)] // generated wit-bindgen export shims
mod ui_world {
    wit_bindgen::generate!({
        path: "../../wit",
        world: "ui",
        export_macro_name: "export_ui",
        with: {
            "lca:host/log@0.2.0": generate,
            "lca:host/ui@0.2.0": generate,
        },
    });

    use exports::lca::ext::interaction::{
        Effect as WasmEffect, Guest as InteractionGuest, Input as WasmInput,
    };
    use exports::lca::ext::render::{Guest as RenderGuest, Widget as WasmWidget};

    pub struct UiWasm;

    fn to_wit(widget: lca_protocol::Widget) -> WasmWidget {
        use lca_protocol::Widget as W;
        match widget {
            W::Text { content, role } => WasmWidget::Text((content, role)),
            W::Image { media_type, bytes } => WasmWidget::Image((media_type, bytes)),
            W::Boxed { title, child } => WasmWidget::Boxed((title, child)),
            W::Row(children) => WasmWidget::Row(children),
            W::Column(children) => WasmWidget::Column(children),
            W::Spinner { frames } => WasmWidget::Spinner(frames),
            W::Progress { label, fill } => WasmWidget::Progress((label, fill)),
            W::KeyValue(pairs) => WasmWidget::Keyvalue(pairs),
            W::Vendor(kind) => WasmWidget::Vendor(kind),
        }
    }

    impl RenderGuest for UiWasm {
        fn render(region: String) -> Option<Vec<WasmWidget>> {
            crate::ui_script(&region).map(|nodes| nodes.into_iter().map(to_wit).collect())
        }
    }

    impl InteractionGuest for UiWasm {
        fn handle(region: String, input: WasmInput) -> WasmEffect {
            let input = match input {
                WasmInput::Key(key) => lca_protocol::UiInput::Key { key },
                WasmInput::Submit(text) => lca_protocol::UiInput::Submit { text },
                WasmInput::Cancel => lca_protocol::UiInput::Cancel,
            };
            match crate::ui_event_script(&region, &input) {
                lca_protocol::UiEffect::None => WasmEffect::None,
                lca_protocol::UiEffect::CloseModal => WasmEffect::CloseModal,
                lca_protocol::UiEffect::OpenModal => WasmEffect::OpenModal,
                lca_protocol::UiEffect::ShowNotice(text) => WasmEffect::ShowNotice(text),
                lca_protocol::UiEffect::InsertText(text) => WasmEffect::InsertText(text),
                lca_protocol::UiEffect::SubmitPrompt(text) => WasmEffect::SubmitPrompt(text),
            }
        }
    }

    export_ui!(UiWasm);
}
