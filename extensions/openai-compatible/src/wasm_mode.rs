use super::*;
use core::cell::RefCell;

wit_bindgen::generate!({
    path: "../../wit",
    world: "provider",
    export_macro_name: "export_provider",
    with: {
        "lca:host/log@0.5.0": generate,
        "lca:host/net@0.5.0": generate,
        "lca:host/oauth@0.5.0": generate,
        "lca:host/credentials@0.5.0": generate,
        "lca:host/resources@0.5.0": generate,
        "lca:host/state@0.5.0": generate,
    },
});

use exports::lca::ext::provider_completion::{
    CompletionStream, Guest as CompletionGuest, GuestCompletionStream, StreamEvent as WasmEvent,
};
use exports::lca::ext::provider_identity::{
    Guest as IdentityGuest, IdentityOutcome as WasmOutcome, TokenUsage,
};
use exports::lca::ext::provider_login::{
    Guest as LoginGuest, LoginAnswer as WasmLoginAnswer, LoginOption as WasmLoginOption,
    LoginResult as WasmLoginResult,
};
use exports::lca::ext::provider_models::{Guest as ModelsGuest, ModelInfo as WasmModel};
use lca::ext::types::{ExtraPair, Usage as WasmUsage};
use lca::host::{credentials, net, resources};

use crate::{Settings, login_options, login_submit, profiles, run_login, run_logout};

fn map_resources(err: resources::Error) -> lca_protocol::CapabilityError {
    use lca_protocol::CapabilityError as E;
    match err {
        resources::Error::Permission(d) => E::Permission(d),
        resources::Error::NotFound(d) => E::NotFound(d),
        resources::Error::Invalid(d) => E::Invalid(d),
    }
}

fn map_net(err: net::Error) -> lca_protocol::CapabilityError {
    use lca_protocol::CapabilityError as E;
    match err {
        net::Error::Permission(d) => E::Permission(d),
        net::Error::NotGranted(d) => E::NotGranted(d),
        net::Error::Dns(d) => E::Io(d),
        net::Error::Tls(d) => E::Io(d),
        net::Error::Io(d) => E::Io(d),
        net::Error::Invalid(d) => E::Invalid(d),
    }
}

/// The guest's capability view: host imports, no sockets, no files.
struct GuestCap;

/// A `'static` instance so the streaming driver can borrow the capability
/// view for as long as its resource lives (a unit struct, so this is free).
static GUEST_CAP: GuestCap = GuestCap;

impl ProviderCap for GuestCap {
    fn net_request(
        &self,
        method: &str,
        url: &str,
        headers: &[(&str, &str)],
        body: Option<&[u8]>,
    ) -> Result<u32, lca_protocol::CapabilityError> {
        net::request(
            method,
            url,
            &headers
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect::<Vec<_>>(),
            body,
        )
        .map_err(map_net)
    }

    fn net_response_status(&self, handle: u32) -> Result<u16, lca_protocol::CapabilityError> {
        net::response_status(handle).map_err(map_net)
    }

    fn net_read_body(
        &self,
        handle: u32,
        max: usize,
    ) -> Result<Option<Vec<u8>>, lca_protocol::CapabilityError> {
        net::read_body(handle, max as u64).map_err(map_net)
    }

    fn net_close_response(&self, handle: u32) -> Result<(), lca_protocol::CapabilityError> {
        net::close_response(handle).map_err(map_net)
    }

    fn credentials_get(&self, key: &str) -> Option<String> {
        credentials::get(key)
    }

    fn credentials_set(&self, key: &str, value: &str) -> Result<(), lca_protocol::CapabilityError> {
        credentials::set(key, value).map_err(|err| match err {
            credentials::Error::Permission(d) => lca_protocol::CapabilityError::Permission(d),
            credentials::Error::NotGranted(d) => lca_protocol::CapabilityError::NotGranted(d),
            credentials::Error::Io(d) => lca_protocol::CapabilityError::Io(d),
            credentials::Error::Invalid(d) => lca_protocol::CapabilityError::Invalid(d),
        })
    }

    fn credentials_delete(&self, key: &str) -> Result<(), lca_protocol::CapabilityError> {
        credentials::delete(key).map_err(|err| match err {
            credentials::Error::Permission(d) => lca_protocol::CapabilityError::Permission(d),
            credentials::Error::NotGranted(d) => lca_protocol::CapabilityError::NotGranted(d),
            credentials::Error::Io(d) => lca_protocol::CapabilityError::Io(d),
            credentials::Error::Invalid(d) => lca_protocol::CapabilityError::Invalid(d),
        })
    }

    fn resource_read(&self, path: &str) -> Result<Vec<u8>, lca_protocol::CapabilityError> {
        resources::read(path).map_err(map_resources)
    }
}

fn to_wit_usage(usage: &Usage) -> WasmUsage {
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

fn to_wit_event(event: StreamEvent) -> WasmEvent {
    use StreamEvent as P;
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

fn to_wit_outcome(outcome: IdentityOutcome) -> WasmOutcome {
    match outcome {
        IdentityOutcome::Ok => WasmOutcome::Ok,
        IdentityOutcome::NotSupported => WasmOutcome::NotSupported,
        IdentityOutcome::Failed(reason) => WasmOutcome::Failed(reason),
    }
}

/// The WIT request -> the protocol shape the shared logic expects.
/// The host already converted on its side; this is the exact inverse
/// (typed content blocks become protocol blocks; reasoning never crossed
/// the boundary in the first place).
fn from_wit_request(request: provider_completion::CompletionRequest) -> CompletionRequest {
    let messages = request
        .messages
        .iter()
        .map(|message| ChatMessage {
            role: match message.role.as_str() {
                "system" => MessageRole::System,
                "user" => MessageRole::User,
                "assistant" => MessageRole::Assistant,
                _ => MessageRole::Tool,
            },
            content: message
                .content
                .iter()
                .map(|block| match block {
                    lca::ext::types::ContentBlock::Text(text) => {
                        ContentBlock::Text { text: text.clone() }
                    }
                    lca::ext::types::ContentBlock::Image((media_type, bytes)) => {
                        ContentBlock::Image {
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
            // The WIT record carries no usage; the host's own copy
            // keeps it, this side never needs it.
            usage: None,
            extras: BTreeMap::new(),
        })
        .collect();
    let tools = request
        .tools
        .iter()
        .map(|tool| ToolSpec {
            name: tool.name.clone(),
            description: tool.description.clone(),
            parameters: serde_json::from_str(&tool.parameters)
                .unwrap_or_else(|_| serde_json::json!({ "type": "object" })),
            extras: tool
                .extras
                .iter()
                .map(|pair| (pair.key.clone(), pair.value.clone()))
                .collect(),
        })
        .collect();
    CompletionRequest {
        messages,
        tools,
        model: request.model,
        stable_prefix: request.stable_prefix as usize,
        extras: request
            .extras
            .iter()
            .map(|pair| (pair.key.clone(), pair.value.clone()))
            .collect(),
    }
}

use exports::lca::ext::provider_completion;

pub struct OpenAiCompatWasm;

/// The `completion-stream` resource: a pull stream over the driver, so
/// events leave as the host yields body chunks instead of after the whole
/// response (C1).
pub struct WasmStream {
    driver: RefCell<Option<crate::StreamDriver<'static, GuestCap>>>,
}

impl GuestCompletionStream for WasmStream {
    fn next(&self) -> Option<WasmEvent> {
        let mut slot = self.driver.borrow_mut();
        let driver = slot.as_mut()?;
        match driver.next_event() {
            None => {
                *slot = None;
                None
            }
            Some(Ok(event)) => Some(to_wit_event(event)),
            Some(Err(failure)) => {
                *slot = None;
                Some(WasmEvent::Error((failure.message, failure.retryable)))
            }
        }
    }
}

impl ModelsGuest for OpenAiCompatWasm {
    fn list_models(pairs: Vec<ExtraPair>) -> Vec<WasmModel> {
        let settings = Settings::default();
        // ADR-0035: the passed settings carry the discovered list - and the
        // `model` a login just persisted, so it is offered immediately
        // (G3, issue #2): environment first (documented precedence), then
        // the pair, then this extension's credential namespace. Never an
        // empty row.
        let configured = if !settings.model.is_empty() {
            settings.model.clone()
        } else {
            pairs
                .iter()
                .find(|pair| pair.key == "model")
                .map(|pair| pair.value.clone())
                .filter(|value| !value.is_empty())
                .or_else(|| GUEST_CAP.credentials_get("model").filter(|v| !v.is_empty()))
                .unwrap_or_default()
        };
        // The list `login-submit` discovered (or the preset's short list):
        // the pairs the host passes first, the extension's own namespace
        // the fallback - the same read `native.rs` makes, so both delivery
        // modes answer from one shape (NFR-25).
        let stored = pairs
            .iter()
            .find(|pair| pair.key == "models")
            .map(|pair| pair.value.clone())
            .unwrap_or_else(|| GUEST_CAP.credentials_get("models").unwrap_or_default());
        // gh #34's window resolution and gh #31's per-model profile both
        // come from `profiles`: one resolver, no drift (NFR-25).
        let windows = load_context_windows(&GUEST_CAP);
        profiles::picker_models(&GUEST_CAP, &settings, &stored, &configured)
            .into_iter()
            .map(|picked| {
                let context_window = context_window_for(
                    &picked.id,
                    settings.context_window,
                    picked.window,
                    &windows,
                );
                let extras = profiles::row_extras(&picked)
                    .into_iter()
                    .map(|(key, value)| ExtraPair { key, value })
                    .collect();
                WasmModel {
                    context_window,
                    id: picked.id.clone(),
                    name: picked.id,
                    max_tokens: 0,
                    extras,
                }
            })
            .collect()
    }
}

impl CompletionGuest for OpenAiCompatWasm {
    type CompletionStream = WasmStream;

    fn stream_completion(
        request: provider_completion::CompletionRequest,
    ) -> Result<CompletionStream, String> {
        let protocol_request = from_wit_request(request);
        let driver = crate::StreamDriver::open(&GUEST_CAP, &Settings::default(), &protocol_request)
            .map_err(|failure| failure.message)?;
        Ok(CompletionStream::new(WasmStream {
            driver: RefCell::new(Some(driver)),
        }))
    }
}

impl IdentityGuest for OpenAiCompatWasm {
    fn login() -> WasmOutcome {
        to_wit_outcome(run_login(&GuestCap, &Settings::default()))
    }

    fn logout() -> WasmOutcome {
        to_wit_outcome(run_logout(&GuestCap))
    }

    fn usage() -> Result<TokenUsage, WasmOutcome> {
        Err(WasmOutcome::NotSupported)
    }
}

impl LoginGuest for OpenAiCompatWasm {
    fn login_options() -> Vec<WasmLoginOption> {
        login_options(&GuestCap)
            .into_iter()
            .map(|option| WasmLoginOption {
                id: option.id,
                name: option.name,
                kind: option.kind,
                host: option.host,
                fields: option.fields,
                extras: option
                    .extras
                    .into_iter()
                    .map(|(key, value)| ExtraPair { key, value })
                    .collect(),
            })
            .collect()
    }

    fn login_submit(answer: WasmLoginAnswer) -> WasmLoginResult {
        let answer = lca_protocol::LoginAnswer {
            choice: answer.choice,
            values: answer
                .values
                .into_iter()
                .map(|pair| (pair.key, pair.value))
                .collect(),
        };
        match login_submit(&GuestCap, &answer) {
            Ok(settings) => WasmLoginResult::Settings(
                settings
                    .into_iter()
                    .map(|(key, value)| ExtraPair { key, value })
                    .collect(),
            ),
            Err(reason) => WasmLoginResult::Failed(reason),
        }
    }
}

export_provider!(OpenAiCompatWasm);
