//! Skills handling: the first-party `context-transform` consumer
//! (ADR-0015, SRDD scope): match a project skill against the latest
//! user message and inject its instructions as their own message,
//! which is why the cache boundary never sees a rewrite (Phase 4 exit
//! clause4).
//!
//! # Skill format
//!
//! `.lca/skills/<name>/SKILL.md` in the workspace: leading
//! `key: value` header lines (the first one is the skill's name, any
//! `match:` line lists comma-separated trigger words), then a `---`
//! line, then the instruction body. Documented here because no document
//! in `docs/` specifies a skills file layout and inventing one inside
//! the SRDD would be worse than owning it where the code lives.
//!
//! # Unsafe-code exemption
//!
//! The `wasm32` half carries generated `wit-bindgen` export shims; the
//! module holds the allowance.

#![deny(unsafe_code)]

/// Where skills live, relative to the workspace root.
pub const SKILLS_DIR: &str = ".lca/skills";

/// The manifest this form ships with (single source for
/// [`manifest_grants`]; a test keeps them in step).
pub const MANIFEST: &str = include_str!("../extension.toml");

/// The grants the manifest declares: read access to the workspace,
/// which is where `.lca/skills` lives - nothing else.
#[cfg(not(target_arch = "wasm32"))]
pub fn manifest_grants() -> lca_tools::CapabilityGrants {
    lca_tools::CapabilityGrants {
        fs: vec![
            lca_permissions::ScopeGrant::parse("workspace", lca_permissions::FsMode::Read)
                .expect("the manifest's own scope parses"),
        ],
        fs_declared: true,
        ..Default::default()
    }
}

/// One parsed skill.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    /// The `name:` header, else the directory name.
    pub name: String,
    /// Lowercase trigger words from `match:`.
    pub match_words: Vec<String>,
    /// The instruction body after the `---` line.
    pub body: String,
}

/// Parse one `SKILL.md`.
pub fn parse_skill(fallback_name: &str, text: &str) -> Skill {
    let mut name = fallback_name.to_string();
    let mut match_line = String::new();
    let mut body_start = 0usize;
    let mut consumed = 0usize;
    for line in text.split('\n') {
        let trimmed = line.trim();
        body_start = consumed;
        consumed += line.len() + 1;
        if trimmed.is_empty() || trimmed == "---" {
            body_start = consumed;
            break;
        }
        if let Some((key, value)) = trimmed.split_once(':') {
            match key.trim() {
                "name" => name = value.trim().to_string(),
                "match" => match_line = value.to_string(),
                _ => {} // unknown header keys are reserved, not errors
            }
        } else {
            // Not a header line: the body starts here.
            body_start = consumed - line.len() - 1;
            break;
        }
    }
    let body = text.get(body_start..).unwrap_or("").trim().to_string();
    let match_words = match_line
        .split(',')
        .map(|word| word.trim().to_lowercase())
        .filter(|word| !word.is_empty())
        .collect();
    Skill {
        name,
        match_words,
        body,
    }
}

/// Whether a skill matches: any trigger word appears in the latest user
/// message (word-boundary-free case-insensitive containment, which is
/// what "matched instructions" means for a first release - ponytail:
/// upgrade to a real matcher if false positives bite).
pub fn skill_matches(skill: &Skill, latest_user: &str) -> bool {
    let haystack = latest_user.to_lowercase();
    skill.match_words.iter().any(|word| haystack.contains(word))
}

/// The injection: every matched skill becomes one appended message -
/// appended, never edited in place, so the stable cache region is
/// untouched by construction (Phase 4 exit clause4).
pub fn injection_message(matched: &[&Skill]) -> Option<lca_protocol::ChatMessage> {
    if matched.is_empty() {
        return None;
    }
    let mut text = String::new();
    for skill in matched {
        text.push_str("[skill ");
        text.push_str(&skill.name);
        text.push_str("]\n");
        text.push_str(&skill.body);
        text.push_str("\n\n");
    }
    Some(lca_protocol::ChatMessage::text(
        lca_protocol::MessageRole::System,
        text.trim_end().to_string(),
    ))
}

/// The whole transform over a loaded skill set: find the latest user
/// message, select the matches, append the injection (FR-CTX-2).
pub fn transform_with_skills(
    mut messages: Vec<lca_protocol::ChatMessage>,
    skills: &[Skill],
) -> Vec<lca_protocol::ChatMessage> {
    let latest_user = messages
        .iter()
        .rev()
        .find(|message| message.role == lca_protocol::MessageRole::User)
        .map(|message| {
            message
                .content
                .iter()
                .filter_map(|block| match block {
                    lca_protocol::ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default();
    let matched: Vec<&Skill> = skills
        .iter()
        .filter(|skill| skill_matches(skill, &latest_user))
        .collect();
    if let Some(injection) = injection_message(&matched) {
        messages.push(injection);
    }
    messages
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

    /// The filesystem view the transform needs: exactly the `fs`
    /// capability's read surface, so neither mode reaches around it.
    pub trait FsView: Send + Sync {
        /// Entries under a workspace-relative directory; `None` when it
        /// does not exist (no skill is a pass-through, not an error).
        fn list(&self, path: &str) -> Result<Option<Vec<String>>, lca_protocol::CapabilityError>;
        /// One file's text.
        fn read(&self, path: &str) -> Result<Option<String>, lca_protocol::CapabilityError>;
    }

    impl FsView for lca_tools::Capabilities {
        fn list(&self, path: &str) -> Result<Option<Vec<String>>, lca_protocol::CapabilityError> {
            match self.fs_list("workspace", path) {
                Ok(entries) => Ok(Some(
                    entries
                        .into_iter()
                        .map(|entry| entry.trim_end_matches('/').to_string())
                        .collect(),
                )),
                Err(lca_protocol::CapabilityError::NotFound(_)) => Ok(None),
                Err(err) => Err(err),
            }
        }

        fn read(&self, path: &str) -> Result<Option<String>, lca_protocol::CapabilityError> {
            match self.fs_read("workspace", path) {
                Ok(bytes) => Ok(Some(String::from_utf8_lossy(&bytes).into_owned())),
                Err(lca_protocol::CapabilityError::NotFound(_)) => Ok(None),
                Err(err) => Err(err),
            }
        }
    }

    /// Load every skill under the skills directory through the fs view.
    /// A missing directory, an unreadable file, or a denial is a
    /// pass-through: skills handling must never break a turn (its only
    /// failure mode is injecting nothing).
    pub fn load_skills(fs: &dyn FsView) -> Vec<Skill> {
        let Ok(Some(entries)) = fs.list(SKILLS_DIR) else {
            return Vec::new();
        };
        let mut skills = Vec::new();
        for entry in entries {
            if let Ok(Some(text)) = fs.read(&format!("{SKILLS_DIR}/{entry}/SKILL.md")) {
                skills.push(parse_skill(&entry, &text));
            }
        }
        skills
    }

    /// The native handle: transform over the workspace's skills
    /// directory, read through the `fs` capability (declared `read` on
    /// the workspace only).
    pub struct Skills {
        cap: Arc<lca_tools::Capabilities>,
    }

    impl Skills {
        /// Build from the engine whose manifest grants carry `fs`.
        pub fn new(cap: Arc<lca_tools::Capabilities>) -> Skills {
            Skills { cap }
        }

        /// The engine, for tests that inspect recorded denials.
        pub fn capabilities(&self) -> &Arc<lca_tools::Capabilities> {
            &self.cap
        }
    }

    impl ExtensionDispatch for Skills {
        fn name(&self) -> &str {
            "skills"
        }

        fn delivery(&self) -> DeliveryMode {
            DeliveryMode::Native
        }

        fn worlds(&self) -> Vec<World> {
            vec![World::ContextTransform]
        }

        fn tool_specs(&self) -> Result<Vec<lca_protocol::ToolSpec>, DispatchError> {
            Err(DispatchError::MissingWorld {
                extension: "skills".to_string(),
                world: "tool",
            })
        }

        fn execute_tool<'a>(
            &'a self,
            _call: &'a lca_protocol::ToolCall,
        ) -> DispatchFuture<'a, Result<lca_protocol::ToolResult, DispatchError>> {
            Box::pin(std::future::ready(Err(DispatchError::MissingWorld {
                extension: "skills".to_string(),
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

        fn transform_messages(
            &self,
            messages: Vec<lca_protocol::ChatMessage>,
        ) -> DispatchFuture<
            'static,
            Result<Result<Vec<lca_protocol::ChatMessage>, String>, DispatchError>,
        > {
            let cap = self.cap.clone();
            Box::pin(async move {
                tokio::task::spawn_blocking(move || {
                    let skills = load_skills(cap.as_ref());
                    transform_with_skills(messages, &skills)
                })
                .await
                .map_err(|_| DispatchError::Failed("skills panicked".to_string()))
                .map(Ok)
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
pub use native::{FsView, Skills, load_skills};

// ---------------------------------------------------------------------------
// WASM delivery mode: the same logic behind the host's fs import
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
#[allow(unsafe_code)] // generated wit-bindgen export shims
mod wasm_mode {
    use super::*;

    wit_bindgen::generate!({
        path: "../../wit",
        world: "context-transform",
        export_macro_name: "export_transform",
        with: {
            "lca:host/log@0.2.0": generate,
            "lca:host/fs@0.2.0": generate,
            // ADR-0030's bags: every world imports them.
            "lca:host/resources@0.2.0": generate,
            "lca:host/state@0.2.0": generate,
        },
    });

    use exports::lca::ext::transform::{Guest as TransformGuest, Message as WasmMessage};
    use lca::ext::types::ToolCall as WasmToolCall;
    use lca::host::fs;

    fn load_guest_skills() -> Vec<Skill> {
        let Ok(entries) = fs::list_entries("workspace", SKILLS_DIR) else {
            return Vec::new();
        };
        let mut skills = Vec::new();
        for entry in entries {
            let entry = entry.trim_end_matches('/').to_string();
            if let Ok(bytes) = fs::read("workspace", &format!("{SKILLS_DIR}/{entry}/SKILL.md")) {
                skills.push(parse_skill(&entry, &String::from_utf8_lossy(&bytes)));
            }
        }
        skills
    }

    pub struct SkillsWasm;

    impl TransformGuest for SkillsWasm {
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
            let transformed = transform_with_skills(protocol, &load_guest_skills());
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

    export_transform!(SkillsWasm);
}
