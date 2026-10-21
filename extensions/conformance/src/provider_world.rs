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

use core::cell::RefCell;

use exports::lca::ext::provider_completion::{CompletionRequest, CompletionStream};
use exports::lca::ext::provider_completion::{
    Guest as CompletionGuest, GuestCompletionStream, StreamEvent as WasmEvent,
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
use lca::host::{credentials, net, oauth, resources};

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

fn map_resources(err: resources::Error) -> crate::CapabilityError {
    use crate::CapabilityError as E;
    match err {
        resources::Error::Permission(d) => E::Permission(d),
        resources::Error::NotFound(d) => E::NotFound(d),
        resources::Error::Invalid(d) => E::Invalid(d),
    }
}

fn map_net(err: net::Error) -> crate::CapabilityError {
    use crate::CapabilityError as E;
    match err {
        net::Error::Permission(d) => E::Permission(d),
        net::Error::NotGranted(d) => E::NotGranted(d),
        net::Error::Dns(d) => E::Io(d),
        net::Error::Tls(d) => E::Io(d),
        net::Error::Io(d) => E::Io(d),
        net::Error::Invalid(d) => E::Invalid(d),
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
    fn oauth_begin(&self, redirect_path: &str) -> Result<(String, u32), crate::CapabilityError> {
        oauth::begin(redirect_path).map_err(map_oauth)
    }
    fn oauth_open(&self, url: &str) -> Result<(), crate::CapabilityError> {
        oauth::open(url).map_err(map_oauth)
    }
    fn oauth_await(&self, handle: u32) -> Result<Vec<(String, String)>, crate::CapabilityError> {
        oauth::await_callback(handle).map_err(map_oauth)
    }
    fn oauth_end(&self, handle: u32) -> Result<(), crate::CapabilityError> {
        oauth::end_flow(handle).map_err(map_oauth)
    }
    fn resource_read(&self, path: &str) -> Result<Vec<u8>, crate::CapabilityError> {
        resources::read(path).map_err(map_resources)
    }
    fn net_response_headers(
        &self,
        handle: u32,
    ) -> Result<Vec<(String, String)>, crate::CapabilityError> {
        net::response_headers(handle).map_err(map_net)
    }
    /// Gh #302: the provider guest is not the world that gained the
    /// import, but the trait is shared, so it answers the same way -
    /// through `lca:host/net`, which the provider world imported all
    /// along.
    fn net_fetch(&self, url: &str) -> Result<(u16, Vec<u8>), crate::CapabilityError> {
        let handle = net::request("GET", url, &[], None).map_err(map_net)?;
        let outcome = (|| {
            let status = net::response_status(handle).map_err(map_net)?;
            let mut body = Vec::new();
            while let Some(chunk) = net::read_body(handle, 64 * 1024).map_err(map_net)? {
                if chunk.is_empty() {
                    break;
                }
                body.extend_from_slice(&chunk);
                if body.len() >= 1024 * 1024 {
                    break;
                }
            }
            Ok((status, body))
        })();
        let _ = net::close_response(handle);
        outcome
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
    fn list_models(settings: Vec<ExtraPair>) -> Vec<WasmModel> {
        let pairs: Vec<(String, String)> = settings
            .into_iter()
            .map(|pair| (pair.key, pair.value))
            .collect();
        crate::provider_models(&pairs)
            .into_iter()
            .map(|model| WasmModel {
                id: model.id,
                name: model.name,
                context_window: model.context_window,
                max_tokens: model.max_tokens,
                // Gh #316: carried, not zeroed. This twin had the same
                // hardcode as the shared macro, which is why the existing
                // cross-mode row could not see the loss: both sides threw
                // the field away, so they "agreed".
                extras: model
                    .extras
                    .into_iter()
                    .map(|(key, value)| ExtraPair { key, value })
                    .collect(),
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
        // Gh #314: the same probe the native twin runs, through this
        // guest's capability view - so a missing delegation shows up as a
        // `<... failed>` in the report rather than as silent agreement.
        let usage = to_wit_usage(&crate::scripted_usage_report_with(
            crate::probe_capability_report(&GuestIdentityCap),
        ));
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

impl LoginGuest for ProviderComponent {
    fn login_options() -> Vec<WasmLoginOption> {
        crate::scripted_login_options()
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
        match crate::scripted_login_submit(&answer) {
            Ok(settings) if settings.is_empty() => WasmLoginResult::Ok,
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

export_provider!(ProviderComponent);
