//! Split from `lib.rs` (cycle 7, P3). Behaviour unchanged.

use super::*;

use super::host_imports::{from_wit_event, from_wit_identity, from_wit_usage, to_wit_request};
use lca_ext_abi::host::provider::exports::lca::ext::provider_completion as wit_completion;
use lca_ext_abi::host::provider::exports::lca::ext::provider_identity as wit_identity;
use lca_ext_abi::host::provider::exports::lca::ext::provider_login as wit_login;
use lca_ext_abi::host::provider::exports::lca::ext::provider_models as wit_models;

// ---------------------------------------------------------------------------
// Provider-world work: model listing, the streaming completion, identity
// ---------------------------------------------------------------------------

pub(super) fn provider_models_work(
    inner: &Inner,
    settings: &[(String, String)],
) -> Result<Vec<ModelInfo>, CallError> {
    let (mut store, instance) = inner.checkout_provider()?;
    // The same opaque settings `complete` gets in its request `extras`
    // (ADR-0035): one source of truth for the extension's configuration.
    let pairs: Vec<wit_models::ExtraPair> = settings
        .iter()
        .map(|(key, value)| wit_models::ExtraPair {
            key: key.clone(),
            value: value.clone(),
        })
        .collect();
    let models = instance
        .lca_ext_provider_models()
        .call_list_models(&mut store, &pairs)
        .map_err(|err| inner.classify(err))?;
    inner.checkin_provider(store, instance);
    Ok(models
        .into_iter()
        .map(|model| ModelInfo {
            id: model.id,
            name: model.name,
            context_window: model.context_window,
            max_tokens: model.max_tokens,
            // The ABI record has always carried these; the host dropped
            // them, which is why no caller could see per-model data.
            extras: model
                .extras
                .into_iter()
                .map(|pair| (pair.key, pair.value))
                .collect(),
        })
        .collect())
}

/// One streaming completion, run on a blocking thread: check out the
/// cached guest, call `stream-completion`, then poll the pull resource
/// until it ends, pushing every event into `bridge` (ADR-0004's
/// host-driven shape).
pub(super) fn provider_stream_work(
    inner: &Inner,
    request: CompletionRequest,
    bridge: Arc<dyn EventSink>,
) -> Result<(), CallError> {
    let (mut store, instance) = inner.checkout_provider()?;
    let wit_request = to_wit_request(&request);
    let created = instance
        .lca_ext_provider_completion()
        .call_stream_completion(&mut store, &wit_request)
        .map_err(|err| inner.classify(err))?;
    let stream = match created {
        Ok(stream) => stream,
        Err(detail) => {
            // The guest declined the request; the guest itself is
            // healthy, so it stays cached (#103).
            inner.checkin_provider(store, instance);
            return Err(CallError::InvalidArguments(format!(
                "stream-completion: {detail}"
            )));
        }
    };
    loop {
        let next = instance
            .lca_ext_provider_completion()
            .completion_stream()
            .call_next(&mut store, stream)
            .map_err(|err| inner.classify(err))?;
        match next {
            Some(event) => {
                if !bridge.push(from_wit_event(event)) {
                    // The receiver is gone (FR-CONC-3): stop polling; the
                    // stream resource is dropped with the store.
                    break;
                }
            }
            None => break,
        }
    }
    // The handle indexes the guest's own table inside this store's
    // instance; a cached instance keeps serving it until the next
    // checkout, and eviction drops both together, so nothing leaks
    // even without an explicit delete (#103).
    let _ = stream;
    inner.checkin_provider(store, instance);
    Ok(())
}

pub(super) enum IdentityOp {
    Login,
    Logout,
}

pub(super) fn identity_simple_work(
    inner: &Inner,
    op: IdentityOp,
) -> Result<IdentityOutcome, CallError> {
    let (mut store, instance) = inner.checkout_provider()?;
    let outcome = match op {
        IdentityOp::Login => instance
            .lca_ext_provider_identity()
            .call_login(&mut store)
            .map_err(|err| inner.classify(err))?,
        IdentityOp::Logout => instance
            .lca_ext_provider_identity()
            .call_logout(&mut store)
            .map_err(|err| inner.classify(err))?,
    };
    inner.checkin_provider(store, instance);
    Ok(from_wit_identity(outcome))
}

pub(super) fn identity_usage_work(
    inner: &Inner,
) -> Result<Result<Usage, IdentityOutcome>, CallError> {
    let (mut store, instance) = inner.checkout_provider()?;
    let outcome = instance
        .lca_ext_provider_identity()
        .call_usage(&mut store)
        .map_err(|err| inner.classify(err))?;
    inner.checkin_provider(store, instance);
    Ok(match outcome {
        Ok(usage) => Ok(from_wit_identity_usage(usage)),
        Err(fallback) => Err(from_wit_identity(fallback)),
    })
}

/// Identity `usage` returns the imported `types.usage` record
/// (`token-usage`); convert it through the same WIT usage shape.
fn from_wit_identity_usage(usage: wit_identity::TokenUsage) -> Usage {
    from_wit_usage(wit_completion::Usage {
        input: usage.input,
        output: usage.output,
        cache_read: usage.cache_read,
        cache_write: usage.cache_write,
        cache_write_hour: usage.cache_write_hour,
        cost: usage.cost,
        extras: usage.extras,
    })
}

/// The provider's login options (ADR-0033) from the component's export.
pub(super) fn login_options_work(
    inner: &Inner,
) -> Result<Vec<lca_protocol::LoginOption>, CallError> {
    let (mut store, instance) = inner.checkout_provider()?;
    let options = instance
        .lca_ext_provider_login()
        .call_login_options(&mut store)
        .map_err(|err| inner.classify(err))?;
    inner.checkin_provider(store, instance);
    Ok(options.into_iter().map(from_wit_login_option).collect())
}

fn from_wit_login_option(option: wit_login::LoginOption) -> lca_protocol::LoginOption {
    lca_protocol::LoginOption {
        id: option.id,
        name: option.name,
        kind: option.kind,
        host: option.host,
        fields: option.fields,
        extras: option
            .extras
            .into_iter()
            .map(|pair| (pair.key, pair.value))
            .collect(),
    }
}

/// Consume the user's answers (ADR-0033).
pub(super) fn login_submit_work(
    inner: &Inner,
    answer: lca_protocol::LoginAnswer,
) -> Result<Vec<(String, String)>, CallError> {
    let (mut store, instance) = inner.checkout_provider()?;
    let wit_answer = wit_login::LoginAnswer {
        choice: answer.choice.clone(),
        values: answer
            .values
            .iter()
            .map(|(key, value)| wit_login::ExtraPair {
                key: key.clone(),
                value: value.clone(),
            })
            .collect(),
    };
    let result = instance
        .lca_ext_provider_login()
        .call_login_submit(&mut store, &wit_answer)
        .map_err(|err| inner.classify(err))?;
    // A `Failed` answer is the guest's healthy verdict, not a poisoned
    // guest: the instance stays cached (#103).
    inner.checkin_provider(store, instance);
    match result {
        wit_login::LoginResult::Ok => Ok(Vec::new()),
        wit_login::LoginResult::Settings(pairs) => Ok(pairs
            .into_iter()
            .map(|pair| (pair.key, pair.value))
            .collect()),
        wit_login::LoginResult::Failed(reason) => Err(CallError::LoginFailed(reason)),
    }
}
