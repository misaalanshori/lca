//! The default compaction strategy (FR-SESS-5, ADR-0015): turn the
//! candidate range into one summary, asking the active provider through
//! the `completion` capability when it is granted and falling back to a
//! mechanical excerpt summary when it is not - a strategy with no
//! capability still works, which is the whole point of keeping the two
//! separable.
//!
//! Both delivery modes share this file: the native handle speaks the
//! capability engine directly, the guest speaks the host's
//! `completion` import.
//!
//! # Unsafe-code exemption
//!
//! The `wasm32` half carries generated `wit-bindgen` export shims; the
//! module holds the allowance.

#![deny(unsafe_code)]

/// The manifest this form ships with (single source for
/// [`manifest_grants`]; a test keeps them in step).
pub const MANIFEST: &str = include_str!("../extension.toml");

/// The grants the manifest declares: `completion` only.
#[cfg(not(target_arch = "wasm32"))]
pub fn manifest_grants() -> lca_tools::CapabilityGrants {
    lca_tools::CapabilityGrants {
        completion: true,
        ..Default::default()
    }
}

/// The instruction the summarization call carries.
const SUMMARY_PROMPT: &str = "Summarize this conversation excerpt for \
someone who will continue it later. Keep the user's requests, the \
decisions made, and anything left unfinished. Under200 words. Reply with \
the summary only.";

/// One candidate record reduced to what a summary needs: its kind and
/// its raw JSON body (both modes hand over the same shape).
pub type Excerpt = (String, String);

/// The text a record contributes, read out of its JSON body by kind so
/// both delivery modes render identically.
pub fn record_text(kind: &str, body: &str) -> String {
    let value: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
    match kind {
        "user" => value
            .get("content")
            .and_then(|c| c.as_str())
            .unwrap_or_default()
            .to_string(),
        "assistant" => {
            let blocks = value.get("content").and_then(|c| c.as_array());
            match blocks {
                Some(blocks) => blocks
                    .iter()
                    .filter_map(|block| block.get("text").and_then(|t| t.as_str()))
                    .map(str::trim)
                    .collect::<Vec<_>>()
                    .join(" "),
                None => String::new(),
            }
        }
        "tool-call" => format!(
            "-> {}({})",
            value.get("name").and_then(|n| n.as_str()).unwrap_or("tool"),
            value
                .get("arguments")
                .and_then(|a| a.as_str())
                .unwrap_or("")
        ),
        "tool-result" => format!(
            "[{}] {}",
            value
                .get("status")
                .and_then(|s| s.as_str())
                .unwrap_or("result"),
            value
                .get("content")
                .and_then(|c| c.as_str())
                .unwrap_or_default()
        ),
        // A re-compaction's range covers the previous compaction record;
        // its summary must carry forward, or every earlier fact is lost.
        "compaction" => value
            .get("summary")
            .and_then(|s| s.as_str())
            .unwrap_or_default()
            .to_string(),
        other => format!("[{other}]"),
    }
}

/// The model-facing prompt: every excerpt's text, each capped so one
/// enormous tool result cannot crowd out the rest.
fn build_prompt(excerpts: &[Excerpt]) -> String {
    let mut prompt = String::from(SUMMARY_PROMPT);
    prompt.push_str("\n\n");
    for (kind, body) in excerpts {
        let text = record_text(kind, body);
        prompt.push_str(kind);
        prompt.push_str(": ");
        prompt.push_str(&text.chars().take(4000).collect::<String>());
        prompt.push('\n');
    }
    prompt
}

/// The mechanical fallback (ADR-0015's honest low-tech path): the
/// leading user requests and the last exchange, which is what a
/// summary without a model can still get right.
pub fn mechanical_summary(excerpts: &[Excerpt]) -> String {
    let user_lines: Vec<String> = excerpts
        .iter()
        .filter(|(kind, _)| kind == "user")
        .map(|(_, body)| {
            record_text("user", body)
                .chars()
                .take(300)
                .collect::<String>()
        })
        .collect();
    let last = excerpts.last().map(|(kind, body)| {
        format!(
            "{kind}: {}",
            record_text(kind, body)
                .chars()
                .take(300)
                .collect::<String>()
        )
    });
    // Carry forward anything an earlier summary already distilled: a
    // re-compaction's range covers the previous compaction record.
    let earlier: Vec<String> = excerpts
        .iter()
        .filter(|(kind, _)| kind == "compaction")
        .map(|(_, body)| {
            record_text("compaction", body)
                .chars()
                .take(600)
                .collect::<String>()
        })
        .collect();
    let earlier_line = if earlier.is_empty() {
        String::new()
    } else {
        format!("\nEarlier summary: {}", earlier.join(" | "))
    };
    format!(
        "Conversation so far ({} messages). Requests: {}{}{}",
        excerpts.len(),
        user_lines.join(" | "),
        earlier_line,
        last.map(|text| format!("\nLast: {text}"))
            .unwrap_or_default()
    )
}

/// One compaction run over a capability view: ask the host when the
/// capability answers, fall back to the mechanical summary when it does
/// not (an undeclared capability stays recorded - FR-PERM-3 - the
/// strategy just degrades instead of failing the turn).
/// The completion path: ask for a summary of the built prompt.
pub type CompleteFn<'a> = &'a dyn Fn(&str) -> Result<String, String>;

/// One compaction run: the model's answer when the path works, the
/// mechanical summary otherwise (an empty or failed answer degrades
/// instead of refusing - the strategy never fails a turn).
pub fn run_compact(excerpts: &[Excerpt], complete: Option<CompleteFn<'_>>) -> String {
    if let Some(complete) = complete
        && let Ok(summary) = complete(&build_prompt(excerpts))
    {
        let summary = summary.trim().to_string();
        if !summary.is_empty() {
            return summary;
        }
    }
    mechanical_summary(excerpts)
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

    /// The native handle over the shared capability engine.
    pub struct CompactionDefault {
        cap: Arc<lca_tools::Capabilities>,
    }

    impl CompactionDefault {
        /// Build from the engine whose manifest grants carry
        /// `completion`.
        pub fn new(cap: Arc<lca_tools::Capabilities>) -> CompactionDefault {
            CompactionDefault { cap }
        }

        /// The engine, for tests that inspect recorded denials.
        pub fn capabilities(&self) -> &Arc<lca_tools::Capabilities> {
            &self.cap
        }
    }

    impl ExtensionDispatch for CompactionDefault {
        fn name(&self) -> &str {
            "compaction-default"
        }

        fn delivery(&self) -> DeliveryMode {
            DeliveryMode::Native
        }

        fn worlds(&self) -> Vec<World> {
            vec![World::Compaction]
        }

        fn tool_specs(&self) -> Result<Vec<lca_protocol::ToolSpec>, DispatchError> {
            Err(DispatchError::MissingWorld {
                extension: "compaction-default".to_string(),
                world: "tool",
            })
        }

        fn execute_tool<'a>(
            &'a self,
            _call: &'a lca_protocol::ToolCall,
        ) -> DispatchFuture<'a, Result<lca_protocol::ToolResult, DispatchError>> {
            Box::pin(std::future::ready(Err(DispatchError::MissingWorld {
                extension: "compaction-default".to_string(),
                world: "tool",
            })))
        }

        fn command_specs(&self) -> Result<Vec<lca_protocol::CommandSpec>, DispatchError> {
            Ok(Vec::new())
        }

        fn invoke_command(
            &self,
            _name: &str,
            _argument: &str,
        ) -> Result<lca_protocol::CommandEffect, DispatchError> {
            Ok(lca_protocol::CommandEffect::None)
        }

        fn compact(
            &self,
            records: &[lca_protocol::Record],
        ) -> DispatchFuture<'static, Result<String, DispatchError>> {
            let excerpts: Vec<Excerpt> = records
                .iter()
                .map(|record| {
                    (
                        record.type_tag().to_string(),
                        serde_json::to_string(record).unwrap_or_default(),
                    )
                })
                .collect();
            let cap = self.cap.clone();
            Box::pin(async move {
                tokio::task::spawn_blocking(move || {
                    let complete = |prompt: &str| -> Result<String, String> {
                        let messages = vec![lca_protocol::ChatMessage::text(
                            lca_protocol::MessageRole::User,
                            prompt,
                        )];
                        cap.complete(messages)
                            .map(|(text, _usage)| text)
                            .map_err(|err| err.to_string())
                    };
                    run_compact(&excerpts, Some(&complete as CompleteFn<'_>))
                })
                .await
                .map_err(|_| DispatchError::Failed("compaction-default panicked".to_string()))
            })
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
pub use native::CompactionDefault;

// ---------------------------------------------------------------------------
// WASM delivery mode: the same logic behind the host's completion import
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
#[allow(unsafe_code)] // generated wit-bindgen export shims
mod wasm_mode {
    use super::*;

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

    fn to_wit_completion(err: completion::Error) -> lca_protocol::CapabilityError {
        use lca_protocol::CapabilityError as E;
        match err {
            completion::Error::Permission(d) => E::Permission(d),
            completion::Error::NotGranted(d) => E::NotGranted(d),
            completion::Error::Io(d) => E::Io(d),
            completion::Error::Invalid(d) => E::Invalid(d),
        }
    }

    pub struct CompactionWasm;

    impl CompactGuest for CompactionWasm {
        fn compact(records: Vec<SessionRecord>) -> Result<String, String> {
            let excerpts: Vec<Excerpt> = records
                .into_iter()
                .map(|record| (record.kind, record.body))
                .collect();
            let complete = |prompt: &str| -> Result<String, String> {
                let messages = vec![lca_protocol::ChatMessage::text(
                    lca_protocol::MessageRole::User,
                    prompt,
                )];
                let wit_messages: Vec<completion::Message> = messages
                    .iter()
                    .map(|message| completion::Message {
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
                                lca_protocol::ContentBlock::Text { text } => Some(text.as_str()),
                                _ => None,
                            })
                            .collect::<Vec<_>>()
                            .join(""),
                        tool_calls: Vec::new(),
                        tool_call_id: None,
                        extras: Vec::new(),
                    })
                    .collect();
                let response = completion::request(&wit_messages)
                    .map_err(to_wit_completion)
                    .map_err(|err| err.to_string())?;
                Ok(response.text)
            };
            Ok(run_compact(&excerpts, Some(&complete as CompleteFn<'_>)))
        }
    }

    export_compaction!(CompactionWasm);
}
