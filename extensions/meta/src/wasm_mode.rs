//! WASM delivery mode: the hand-written guest (the subscription
//! macro speaks Responses; Messages needs its own driver). The shape
//! mirrors the OpenRouter guest: one `GuestCap`, one pull stream,
//! thin conversions both ways.

use super::*;
use core::cell::RefCell;

wit_bindgen::generate!({
    path: "../../wit",
    world: "provider",
    export_macro_name: "export_provider",
    with: {
        "lca:host/log@0.6.0": generate,
        "lca:host/net@0.6.0": generate,
        "lca:host/oauth@0.6.0": generate,
        "lca:host/credentials@0.6.0": generate,
        "lca:host/resources@0.6.0": generate,
        "lca:host/state@0.6.0": generate,
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
use lca::host::{credentials, net, oauth};

use crate::{IdentityOutcomeAlias, run_login, run_logout, run_usage};

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

fn map_oauth(err: oauth::Error) -> lca_protocol::CapabilityError {
    use lca_protocol::CapabilityError as E;
    match err {
        oauth::Error::Permission(d) => E::Permission(d),
        oauth::Error::NotGranted(d) => E::NotGranted(d),
        oauth::Error::Timeout(d) => E::Timeout(d),
        oauth::Error::Io(d) => E::Io(d),
        oauth::Error::Invalid(d) => E::Invalid(d),
    }
}

/// The guest's capability view: host imports only.
struct GuestCap;

/// A `'static` view so the streaming driver can borrow the capability
/// view for as long as its resource lives (a unit struct: free).
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
}

impl OauthCap for GuestCap {
    fn oauth_begin(
        &self,
        redirect_path: &str,
    ) -> Result<(String, u32), lca_protocol::CapabilityError> {
        oauth::begin(redirect_path).map_err(map_oauth)
    }

    fn oauth_open(&self, url: &str) -> Result<(), lca_protocol::CapabilityError> {
        oauth::open(url).map_err(map_oauth)
    }

    fn oauth_await(
        &self,
        handle: u32,
    ) -> Result<Vec<(String, String)>, lca_protocol::CapabilityError> {
        oauth::await_callback(handle).map_err(map_oauth)
    }

    fn oauth_end(&self, handle: u32) -> Result<(), lca_protocol::CapabilityError> {
        oauth::end_flow(handle).map_err(map_oauth)
    }
}

fn to_wit_usage(usage: &lca_protocol::Usage) -> WasmUsage {
    WasmUsage {
        input: usage.input,
        output: usage.output,
        cache_read: usage.cache_read,
        cache_write: usage.cache_write,
        cache_write_hour: usage.cache_write_1h,
        cost: usage.cost,
        extras: usage
            .extras
            .iter()
            .map(|(key, value)| ExtraPair {
                key: key.clone(),
                value: value.clone(),
            })
            .collect(),
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

fn to_wit_outcome(outcome: IdentityOutcomeAlias) -> WasmOutcome {
    match outcome {
        lca_protocol::IdentityOutcome::Ok => WasmOutcome::Ok,
        lca_protocol::IdentityOutcome::NotSupported => WasmOutcome::NotSupported,
        lca_protocol::IdentityOutcome::Failed(reason) => WasmOutcome::Failed(reason),
    }
}

pub struct MetaWasm;

/// The `completion-stream` resource: a pull stream over the Chat
/// driver, so events leave as the host yields body chunks instead of
/// after the whole response (C1).
pub struct QueuedStream {
    driver: RefCell<Option<crate::ChatDriver<'static>>>,
}

impl GuestCompletionStream for QueuedStream {
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
                Some(WasmEvent::Error((failure.message, false)))
            }
        }
    }
}

impl ModelsGuest for MetaWasm {
    fn list_models(_settings: Vec<ExtraPair>) -> Vec<WasmModel> {
        crate::list_models(&GUEST_CAP)
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

impl CompletionGuest for MetaWasm {
    type CompletionStream = QueuedStream;

    fn stream_completion(
        request: exports::lca::ext::provider_completion::CompletionRequest,
    ) -> Result<CompletionStream, String> {
        // The WIT record -> the protocol shape (the exact inverse of
        // the host's conversion; reasoning never crosses).
        let messages = request
            .messages
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
                        parent_call_id: None,
                    })
                    .collect(),
                tool_call_id: message.tool_call_id.clone(),
                usage: None,
                extras: Default::default(),
            })
            .collect();
        let tools = request
            .tools
            .iter()
            .map(|tool| lca_protocol::ToolSpec {
                name: tool.name.clone(),
                description: tool.description.clone(),
                parameters: serde_json::from_str(&tool.parameters)
                    .unwrap_or_else(|_| serde_json::json!({ "type": "object" })),
                exposure: lca_protocol::ToolExposure::Direct,
                namespace: None,
                annotations: None,
                extras: tool
                    .extras
                    .iter()
                    .map(|pair| (pair.key.clone(), pair.value.clone()))
                    .collect(),
            })
            .collect();
        let protocol_request = lca_protocol::CompletionRequest {
            messages,
            tools,
            model: request.model,
            stable_prefix: request.stable_prefix as usize,
            extras: request
                .extras
                .iter()
                .map(|pair| (pair.key.clone(), pair.value.clone()))
                .collect(),
        };
        // Byte-identical to the native delivery: one builder, both drivers.
        let (url, headers, body) = crate::build_request(&GUEST_CAP, &protocol_request)
            .map_err(|failure| failure.message)?;
        let refs: Vec<(&str, &str)> = headers
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect();
        let driver = crate::ChatDriver::open(&GUEST_CAP, &url, &refs, &body)
            .map_err(|failure| failure.message)?;
        Ok(CompletionStream::new(QueuedStream {
            driver: RefCell::new(Some(driver)),
        }))
    }
}

impl IdentityGuest for MetaWasm {
    fn login() -> WasmOutcome {
        match run_login(&GUEST_CAP, &GUEST_CAP) {
            Ok(outcome) => to_wit_outcome(outcome),
            Err(err) => WasmOutcome::Failed(err.0),
        }
    }

    fn logout() -> WasmOutcome {
        to_wit_outcome(run_logout(&GUEST_CAP))
    }

    fn usage() -> Result<TokenUsage, WasmOutcome> {
        match run_usage(&GUEST_CAP) {
            Ok(usage) => Ok(TokenUsage {
                input: usage.input,
                output: usage.output,
                cache_read: usage.cache_read,
                cache_write: usage.cache_write,
                cache_write_hour: usage.cache_write_1h,
                cost: usage.cost,
                extras: usage
                    .extras
                    .iter()
                    .map(|(key, value)| ExtraPair {
                        key: key.clone(),
                        value: value.clone(),
                    })
                    .collect(),
            }),
            Err(err) => Err(WasmOutcome::Failed(err.0)),
        }
    }
}

impl LoginGuest for MetaWasm {
    fn login_options() -> Vec<WasmLoginOption> {
        // No picker presets: the identity flow runs directly (the
        // host routes an option-less provider to it, ADR-0033).
        Vec::new()
    }

    fn login_submit(_answer: WasmLoginAnswer) -> WasmLoginResult {
        WasmLoginResult::Ok
    }
}

export_provider!(MetaWasm);
