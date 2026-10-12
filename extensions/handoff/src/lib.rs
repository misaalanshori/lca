//! Structured task-transition handoffs (gh #196): `/handoff
//! [instructions]` snapshots the session, summarizes it through the
//! host's `completion` capability, saves the document under
//! `.lca/extensions/handoff/`, and returns it as a fresh checkpoint.
//!
//! The pipeline core ([`run_handoff`]) is a pure function of its
//! dependencies so the whole shape is testable without a host. Two
//! host feeds do not exist yet under the 0.6 freeze, and the code
//! marks both: the command invocation carries no session turns (so
//! `invoke_command` reports the seam instead of faking a document),
//! and the WASM `run` has no turns either. When the turns feed lands,
//! `invoke_command` calls `run_handoff` with the live turns and
//! `cap.complete` — one line each.
//!
//! # Unsafe-code exemption
//!
//! The `wasm32` half carries generated `wit-bindgen` export shims; the
//! module holds the allowance.

#![deny(unsafe_code)]

/// The manifest this form ships with (single source for
/// [`manifest_grants`]; a test keeps them in step).
pub const MANIFEST: &str = include_str!("../extension.toml");

/// The grants the manifest declares: the `completion` capability for
/// one-shot summarization, plus read-write on the workspace for the
/// artifact directory - nothing else.
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::expect_used)] // the extension's own literal manifest patterns are valid by construction.
pub fn manifest_grants() -> lca_tools::CapabilityGrants {
    lca_tools::CapabilityGrants {
        fs: vec![
            lca_permissions::ScopeGrant::parse("workspace", lca_permissions::FsMode::ReadWrite)
                .expect("the manifest's own scope parses"),
        ],
        fs_declared: true,
        completion: true,
        ..Default::default()
    }
}

/// One conversation turn the handoff summarizes: the shape the host's
/// turns feed will supply once it exists (today only the pipeline
/// test constructs these).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Turn {
    /// `user`, `assistant`, or the tool event's name.
    pub role: String,
    /// The turn's text.
    pub text: String,
}

/// The handoff artifact directory, workspace-relative.
pub const HANDOFF_DIR: &str = ".lca/extensions/handoff";

/// What an empty session reports instead of summarizing nothing.
pub const EMPTY_SESSION_MESSAGE: &str = "Nothing to hand off (session has not started)";

/// The transcript budget into the summarization prompt: turns are
/// newest-first and cut when the budget runs out, so a long session
/// still fits (the summary covers the tail, which is what the next
/// session continues).
pub const TRANSCRIPT_BUDGET: usize = 12_000;

/// Build the one-shot summarization prompt: the transcript plus the
/// four required sections, steered by the inline instructions.
pub fn build_summarization_prompt(turns: &[Turn], instructions: &str) -> String {
    let mut prompt = String::from(
        "Summarize this session as a task handoff document with exactly these sections:\n\
         \n\
         ## Completed Tasks\n\
         What was accomplished and verified.\n\
         \n\
         ## Architectural Decisions\n\
         Key trade-offs or design choices made during the session.\n\
         \n\
         ## Current State\n\
         Modified files, active branch, and passing/failing tests.\n\
         \n\
         ## Remaining Next Steps\n\
         Bulleted tasks for the next phase.\n",
    );
    if !instructions.trim().is_empty() {
        prompt.push_str("\nFocus: ");
        prompt.push_str(instructions.trim());
        prompt.push('\n');
    }
    prompt.push_str("\nSession transcript (newest first):\n");
    let mut budget = TRANSCRIPT_BUDGET;
    for turn in turns.iter().rev() {
        let line = format!("[{}] {}\n", turn.role, turn.text);
        if line.len() > budget {
            break;
        }
        budget -= line.len();
        prompt.push_str(&line);
    }
    prompt
}

/// The artifact filename for a timestamp.
pub fn artifact_filename(timestamp: u64) -> String {
    format!("{HANDOFF_DIR}/handoff-{timestamp}.md")
}

/// Wrap the summary with its provenance: instructions that steered
/// it and the timestamp it was taken at.
pub fn format_artifact(summary: &str, instructions: &str, timestamp: u64) -> String {
    let mut text = String::from("# Session Handoff\n");
    text.push_str(&format!("\nTaken at timestamp {timestamp}.\n"));
    if !instructions.trim().is_empty() {
        text.push_str("\nSteered by: ");
        text.push_str(instructions.trim());
        text.push('\n');
    }
    text.push('\n');
    text.push_str(summary);
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

/// A finished handoff: the document for the session, plus the
/// artifact path when one was written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandoffOutcome {
    /// The handoff document (or the empty-session message).
    pub document: String,
    /// Where the artifact landed, when one was written.
    pub artifact: Option<String>,
}

/// The whole pipeline: prompt from turns, summary from completion,
/// artifact to the filesystem, document back. Empty turns short
/// out before completion; a failed completion propagates and writes
/// nothing.
pub fn run_handoff(
    turns: &[Turn],
    instructions: &str,
    complete: impl FnOnce(String) -> Result<String, String>,
    write_artifact: impl FnOnce(&str, &str) -> Result<(), String>,
    timestamp: u64,
) -> Result<HandoffOutcome, String> {
    if turns.is_empty() {
        return Ok(HandoffOutcome {
            document: EMPTY_SESSION_MESSAGE.to_string(),
            artifact: None,
        });
    }
    let summary = complete(build_summarization_prompt(turns, instructions))?;
    let path = artifact_filename(timestamp);
    let text = format_artifact(&summary, instructions, timestamp);
    write_artifact(&path, &text)?;
    Ok(HandoffOutcome {
        document: summary,
        artifact: Some(path),
    })
}

/// The `/handoff` command spec the host lists and tab-completes.
pub fn handoff_command_spec() -> lca_protocol::CommandSpec {
    lca_protocol::CommandSpec {
        name: "handoff".to_string(),
        hint: "[instructions]".to_string(),
        completion: "none".to_string(),
        extras: Default::default(),
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

    /// The native handle. The `completion` backend rides the
    /// engine; the session turns do not reach `invoke_command` under
    /// ABI 0.6 (see the module docs), so the command reports the seam
    /// until the host feeds turns to command invocations.
    pub struct Handoff {
        _cap: Arc<lca_tools::Capabilities>,
    }

    impl Handoff {
        /// Build from the engine whose manifest grants carry
        /// `completion` and workspace `fs`.
        pub fn new(cap: Arc<lca_tools::Capabilities>) -> Handoff {
            Handoff { _cap: cap }
        }
    }

    impl ExtensionDispatch for Handoff {
        fn name(&self) -> &str {
            "handoff"
        }

        fn delivery(&self) -> DeliveryMode {
            DeliveryMode::Native
        }

        fn worlds(&self) -> Vec<World> {
            vec![World::Command]
        }

        fn tool_specs(&self) -> Result<Vec<lca_protocol::ToolSpec>, DispatchError> {
            Err(DispatchError::MissingWorld {
                extension: "handoff".to_string(),
                world: "tool",
            })
        }

        fn execute_tool<'a>(
            &'a self,
            _call: &'a lca_protocol::ToolCall,
        ) -> DispatchFuture<'a, Result<lca_protocol::ToolResult, DispatchError>> {
            Box::pin(std::future::ready(Err(DispatchError::MissingWorld {
                extension: "handoff".to_string(),
                world: "tool",
            })))
        }

        fn command_specs(&self) -> Result<Vec<lca_protocol::CommandSpec>, DispatchError> {
            Ok(vec![handoff_command_spec()])
        }

        fn invoke_command(
            &self,
            _name: &str,
            _argument: &str,
        ) -> Result<lca_protocol::CommandEffect, DispatchError> {
            // SEAM (gh #196): the invocation carries the inline
            // instructions but no session turns, so there is nothing
            // to summarize yet. Report the seam in the notice area
            // rather than faking a document; when the host feeds
            // turns, this calls `run_handoff` with the live turns
            // and `cap.complete`.
            Ok(lca_protocol::CommandEffect::ShowWidget(
                "The handoff document needs the session-turns feed, which command invocations do not carry yet (gh #196). The pipeline itself is built and tested; only the feed is missing.".to_string(),
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
                extension: "handoff".to_string(),
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
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use native::Handoff;

// ---------------------------------------------------------------------------
// WASM delivery mode: the command world behind the host's imports
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
#[allow(unsafe_code)] // generated wit-bindgen export shims
mod wasm_mode {
    wit_bindgen::generate!({
        path: "../../wit",
        world: "command",
        export_macro_name: "export_handoff",
        with: {
            "lca:host/ui-dialogs@0.6.0": generate,
        },
    });

    use exports::lca::ext::command_spec::{Guest as SpecGuest, Spec};
    use exports::lca::ext::invoke::{Effect, Guest as InvokeGuest};

    pub struct HandoffWasm;

    impl SpecGuest for HandoffWasm {
        fn get_spec() -> Spec {
            let spec = super::handoff_command_spec();
            Spec {
                name: spec.name,
                hint: spec.hint,
                completion: spec.completion,
                extras: Vec::new(),
            }
        }
    }

    impl InvokeGuest for HandoffWasm {
        fn run(_argument: String) -> Effect {
            // SEAM (gh #196): same missing turns feed as the native
            // handle; the notice carries the reason, not a document.
            Effect::ShowWidget(
                "The handoff document needs the session-turns feed, which command invocations do not carry yet (gh #196).".to_string(),
            )
        }
    }

    export_handoff!(HandoffWasm);
}
