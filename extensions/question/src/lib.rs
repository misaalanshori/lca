//! Model-facing interactive questions (gh #221): the model asks
//! structured clarification mid-turn, the user picks or types on
//! host chrome, and the turn resumes with the answers in context.
//!
//! The dialogs cross two boundaries: the WASM `ui-dialogs` import
//! and the native [`Dialogs`] trait (implemented over the host's
//! `SharedDialogs`, which defaults to declined values when nothing
//! is installed — the headless shape, where every ask declines and
//! nothing hangs). The flow over either boundary is the shared
//! [`ask_all`]: options plus a standing custom-answer option go to a
//! picker; the custom option routes to a free-text input.
//!
//! The manifest declares no capabilities: dialogs ride the tool
//! world's import, not the manifest (an empty `[capabilities.ui]`
//! is rejected at manifest parse, and regions would claim chrome
//! this extension never draws).
//!
//! # Unsafe-code exemption
//!
//! The `wasm32` half carries generated `wit-bindgen` export shims; the
//! module holds the allowance.

#![deny(unsafe_code)]

/// The manifest this form ships with (a test keeps it in step: tool
/// world, zero capabilities).
pub const MANIFEST: &str = include_str!("../extension.toml");

/// The standing custom-answer option appended to every picker.
pub const CUSTOM_OPTION: &str = "Type your own answer...";
/// The finish option for multi-pick questions.
pub const DONE_OPTION: &str = "Done";

/// One selectable answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptionItem {
    /// The short name the report quotes.
    pub label: String,
    /// The picker annotation; empty shows the bare label.
    pub description: String,
}

/// One question for the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    /// The report's key for this answer.
    pub header: String,
    /// The picker title.
    pub question: String,
    /// The choices; empty means free text only.
    pub options: Vec<OptionItem>,
    /// Accumulate several picks until Done.
    pub multiple: bool,
}

/// The dialog surface the flow needs: a picker and a line of text.
// Implemented over both boundaries (WASM import, native slot) plus
// scripted fakes in tests.
pub trait Dialogs {
    /// A picker over options; `None` is dismissed.
    fn select(&self, title: &str, options: &[String]) -> Option<String>;
    /// One line of text; `None` is dismissed.
    fn input(&self, label: &str, placeholder: Option<&str>) -> Option<String>;
}

/// Parse the tool's arguments: a non-empty `questions` array.
pub fn parse_question_args(json: &str) -> Result<Vec<Question>, String> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|err| format!("arguments are not JSON: {err}"))?;
    let raw = value
        .get("questions")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "the question tool needs a `questions` array".to_string())?;
    if raw.is_empty() {
        return Err("the question tool needs at least one question".to_string());
    }
    let mut questions = Vec::new();
    for (index, entry) in raw.iter().enumerate() {
        let field = |name: &str| {
            entry
                .get(name)
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| format!("question {index} needs a `{name}` string"))
        };
        let header = field("header")?.to_string();
        let question = field("question")?.to_string();
        let options = entry
            .get("options")
            .and_then(serde_json::Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .map(|item| OptionItem {
                        label: item
                            .get("label")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        description: item
                            .get("description")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let multiple = entry
            .get("multiple")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        questions.push(Question {
            header,
            question,
            options,
            multiple,
        });
    }
    Ok(questions)
}

/// Render one option for the picker line.
fn display(option: &OptionItem) -> String {
    if option.description.trim().is_empty() {
        option.label.clone()
    } else {
        format!("{} — {}", option.label, option.description)
    }
}

/// Ask one question through the dialogs; the quoted answer(s).
fn ask_one(dialogs: &dyn Dialogs, question: &Question) -> String {
    if question.options.is_empty() {
        return dialogs
            .input(&question.question, None)
            .unwrap_or_else(|| "(declined)".to_string());
    }
    let mut remaining: Vec<&OptionItem> = question.options.iter().collect();
    let mut picked = Vec::new();
    loop {
        let mut lines: Vec<String> = remaining.iter().map(|option| display(option)).collect();
        lines.push(CUSTOM_OPTION.to_string());
        if question.multiple {
            lines.push(DONE_OPTION.to_string());
        }
        let Some(choice) = dialogs.select(&question.question, &lines) else {
            break;
        };
        if choice == DONE_OPTION {
            break;
        }
        if choice == CUSTOM_OPTION {
            if let Some(text) = dialogs.input(&question.question, None) {
                picked.push(text);
            }
            if !question.multiple {
                break;
            }
            continue;
        }
        match remaining
            .iter()
            .position(|option| display(option) == choice)
        {
            Some(at) => {
                picked.push(remaining.remove(at).label.clone());
            }
            // The host returned something unlisted: take it at face
            // value rather than looping forever.
            None => {
                picked.push(choice);
            }
        }
        if !question.multiple || remaining.is_empty() {
            break;
        }
    }
    if picked.is_empty() {
        "(declined)".to_string()
    } else {
        picked.join(", ")
    }
}

/// Ask every question; the report the tool hands back to the model.
pub fn ask_all(dialogs: &dyn Dialogs, questions: &[Question]) -> String {
    let mut report = String::from("User has answered your questions: ");
    for (index, question) in questions.iter().enumerate() {
        if index > 0 {
            report.push_str(", ");
        }
        report.push('"');
        report.push_str(&question.header);
        report.push_str("\" = \"");
        report.push_str(&ask_one(dialogs, question));
        report.push('"');
    }
    report.push_str(". You can now continue with the user's answers in mind.");
    report
}

// ---------------------------------------------------------------------------
// Native delivery mode
// ---------------------------------------------------------------------------

/// The `question` tool spec the model sees, built for **both** delivery
/// modes.
///
/// gh #298: the WASM guest used to carry a hand-written copy of this
/// schema with the `items` object missing, so a WASM-mode model was told
/// `questions` was an untyped array and answered with plain strings - which
/// the guest's own parser then rejected. The copy is gone: the guest calls
/// this function, exactly as `hash-edit` and `webfetch` already share one
/// spec between their two modes. Two literals that must stay equal are a
/// defect waiting for the next edit; one literal cannot drift from itself.
///
/// Only `lca-protocol` and `serde_json` are needed, so it builds on
/// `wasm32` as well as natively - it is deliberately outside the
/// `#[cfg(not(target_arch = "wasm32"))]` module below.
pub fn question_tool_spec() -> lca_protocol::ToolSpec {
    lca_protocol::ToolSpec {
        name: "question".to_string(),
        description:
            "Ask the user structured clarification questions mid-turn; resumes with their answers."
                .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "questions": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "header": {"type": "string"},
                            "question": {"type": "string"},
                            "options": {
                                "type": "array",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "label": {"type": "string"},
                                        "description": {"type": "string"}
                                    },
                                    "required": ["label"]
                                }
                            },
                            "multiple": {"type": "boolean", "default": false}
                        },
                        "required": ["header", "question"]
                    }
                }
            },
            "required": ["questions"]
        }),
        exposure: lca_protocol::ToolExposure::Direct,
        namespace: None,
        annotations: None,
        extras: Default::default(),
    }
}

/// The parameters a WASM-mode model is declared, as JSON text - the same
/// value the native spec carries, through the same constructor (gh #298).
///
/// The WIT `tool-schema` export takes `parameters` as a string rather than
/// a typed document, so the one conversion lives here beside the one
/// schema. Both delivery modes call this, so the parity test cannot pass
/// while either mode drifts.
pub fn question_schema_parameters() -> String {
    question_tool_spec().parameters.to_string()
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use super::*;

    use lca_ext_abi::{DeliveryMode, DispatchFuture, ExtensionDispatch, World};
    use lca_protocol::DispatchError;

    impl Dialogs for lca_permissions::SharedDialogs {
        fn select(&self, title: &str, options: &[String]) -> Option<String> {
            lca_permissions::DialogPrompt::select(&mut self.clone(), title, options)
        }

        fn input(&self, label: &str, placeholder: Option<&str>) -> Option<String> {
            lca_permissions::DialogPrompt::input(&mut self.clone(), label, placeholder)
        }
    }

    /// The native handle: dialogs arrive on the shared slot the host
    /// installs per session (empty headless, where every ask
    /// declines through the slot's denied defaults).
    pub struct QuestionTool {
        dialogs: lca_permissions::SharedDialogs,
    }

    impl QuestionTool {
        /// Build from the host's dialog slot.
        pub fn new(dialogs: lca_permissions::SharedDialogs) -> QuestionTool {
            QuestionTool { dialogs }
        }
    }

    impl ExtensionDispatch for QuestionTool {
        fn name(&self) -> &str {
            "question"
        }

        fn delivery(&self) -> DeliveryMode {
            DeliveryMode::Native
        }

        fn worlds(&self) -> Vec<World> {
            vec![World::Tool]
        }

        fn tool_specs(&self) -> Result<Vec<lca_protocol::ToolSpec>, DispatchError> {
            Ok(vec![question_tool_spec()])
        }

        fn execute_tool<'a>(
            &'a self,
            call: &'a lca_protocol::ToolCall,
        ) -> DispatchFuture<'a, Result<lca_protocol::ToolResult, DispatchError>> {
            let arguments = call.arguments.clone();
            let call_id = call.call_id.clone();
            let dialogs = self.dialogs.clone();
            Box::pin(async move {
                let content = match parse_question_args(&arguments) {
                    Ok(questions) => ask_all(&dialogs, &questions),
                    Err(reason) => reason,
                };
                // Argument failures report as answers, not dispatch
                // failures: the model reads the complaint and retries
                // in the same turn.
                Ok(lca_protocol::ToolResult {
                    call_id,
                    status: lca_protocol::ToolResultStatus::Ok,
                    content,
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
            _messages: Vec<lca_protocol::ChatMessage>,
        ) -> DispatchFuture<
            'static,
            Result<Result<Vec<lca_protocol::ChatMessage>, String>, DispatchError>,
        > {
            Box::pin(std::future::ready(Err(DispatchError::MissingWorld {
                extension: "question".to_string(),
                world: "tool",
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

// gh #298: `question_tool_spec` lives at the crate root now, because both
// delivery modes construct the schema from it; only the handle is native.
#[cfg(not(target_arch = "wasm32"))]
pub use native::QuestionTool;

// ---------------------------------------------------------------------------
// WASM delivery mode: the tool world over the dialogs import
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
#[allow(unsafe_code)] // generated wit-bindgen export shims
mod wasm_mode {
    wit_bindgen::generate!({
        path: "../../wit",
        world: "tool",
        export_macro_name: "export_question",
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
    use lca::host::ui_dialogs;

    struct GuestDialogs;

    impl super::Dialogs for GuestDialogs {
        fn select(&self, title: &str, options: &[String]) -> Option<String> {
            let options: Vec<String> = options.to_vec();
            ui_dialogs::select(title, &options).unwrap_or(None)
        }

        fn input(&self, label: &str, placeholder: Option<&str>) -> Option<String> {
            let placeholder = placeholder.map(str::to_string);
            ui_dialogs::input(label, placeholder.as_deref()).unwrap_or(None)
        }
    }

    pub struct QuestionWasm;

    impl SchemaGuest for QuestionWasm {
        fn get_schema() -> Schema {
            // gh #298: the same constructor the native spec uses. The guest
            // used to hand-write a shortened copy (no `items`), which told
            // the model `questions` was an untyped array; the model then
            // answered with plain strings and this guest's own parser
            // rejected them. One schema, two modes.
            let spec = super::question_tool_spec();
            Schema {
                name: spec.name,
                description: spec.description,
                parameters: super::question_schema_parameters(),
                extras: Vec::new(),
            }
        }
    }

    impl ExecuteTrait for QuestionWasm {
        fn run(call: ToolCall) -> WasmResult {
            let content = match super::parse_question_args(&call.arguments) {
                Ok(questions) => super::ask_all(&GuestDialogs, &questions),
                Err(reason) => reason,
            };
            WasmResult {
                call_id: call.call_id,
                status: "ok".to_string(),
                content: Some(content),
                truncated: false,
                extras: Vec::new(),
            }
        }
    }

    export_question!(QuestionWasm);
}
