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
    // Gh #258: the guest-export crossing (setting names, never values).
    lca_debug::wasm::event(
        "export.models.list",
        &format!(
            "settings=[{}]",
            settings
                .iter()
                .map(|(key, _)| key.as_str())
                .collect::<Vec<_>>()
                .join(",")
        ),
    );
    let started = std::time::Instant::now();
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
    lca_debug::wasm::event(
        "export.models.done",
        &format!(
            "count={} ms={}",
            models.len(),
            started.elapsed().as_millis()
        ),
    );
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

#[cfg(test)]
mod reasoning_bridge_tests {
    //! The host half of the reasoning-signature extras bridge (gh #289).
    //!
    //! The frozen 0.6 `content-block` variant has no `reasoning` case, so
    //! a thinking run's replay signature crosses the ABI in the reserved
    //! extras map under [`lca_protocol::THINKING_SIGNATURE_EXTRA`], and
    //! the guest rebuilds the `Reasoning` block from it. These rows pin
    //! the *host* end of that bridge - what `to_wit_request` writes.
    //!
    //! `to_wit_request` is `pub(super)`, so this is the only placement
    //! that can pin it without widening the surface for a test. The guest
    //! end is pinned by the antigravity guest's own rows
    //! (`extensions/antigravity/src/{lib,stream}.rs`) and end to end by
    //! `crates/lca-ext-host/tests/provider.rs`'s catalog rows; the typed
    //! `reasoning` case is gh #352's, not pinned here.

    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

    use lca_protocol::{
        ChatMessage, ContentBlock, MessageRole, THINKING_SIGNATURE_EXTRA, ToolCall,
    };

    fn assistant_with(reasoning: Option<(&str, Option<&str>)>) -> ChatMessage {
        let mut content = Vec::new();
        if let Some((text, signature)) = reasoning {
            content.push(ContentBlock::Reasoning {
                reasoning: text.to_string(),
                signature: signature.map(str::to_string),
            });
        }
        ChatMessage {
            role: MessageRole::Assistant,
            content,
            tool_calls: vec![ToolCall {
                call_id: "call-1".to_string(),
                name: "default_api:list".to_string(),
                arguments: "{}".to_string(),
                parent_call_id: None,
            }],
            tool_call_id: None,
            usage: None,
            extras: Default::default(),
        }
    }

    fn request_with(message: ChatMessage) -> lca_protocol::CompletionRequest {
        lca_protocol::CompletionRequest {
            messages: vec![message],
            tools: Vec::new(),
            model: "gemini".to_string(),
            stable_prefix: 0,
            extras: Default::default(),
        }
    }

    fn signature_on_the_wire(
        converted: &lca_ext_abi::host::provider::exports::lca::ext::provider_completion::CompletionRequest,
    ) -> Option<String> {
        converted.messages[0]
            .extras
            .iter()
            .find(|pair| pair.key == THINKING_SIGNATURE_EXTRA)
            .map(|pair| pair.value.clone())
    }

    // Verifies: gh #289 (the host WRITES the extras key for a signed
    // thinking run - link one of the four in the bridge)
    #[test]
    fn a_signed_thinking_run_travels_as_the_reserved_extras_key() {
        let converted = super::to_wit_request(&request_with(assistant_with(Some((
            "plan the call",
            Some("sig-abc-123"),
        )))));
        assert_eq!(
            signature_on_the_wire(&converted).as_deref(),
            Some("sig-abc-123"),
            "the signature crosses under the reserved key"
        );
    }

    // Verifies: gh #289 (reasoning is never resent as a content block -
    // the frozen variant has no case for it, so it must NOT appear as
    // text; the signature is the only channel)
    #[test]
    fn a_reasoning_block_never_becomes_a_content_block_or_text() {
        let converted = super::to_wit_request(&request_with(assistant_with(Some((
            "plan the call",
            Some("sig-abc-123"),
        )))));
        use lca_ext_abi::host::provider::lca::ext::types::ContentBlock;
        assert!(
            converted.messages[0].content.iter().all(
                |block| !matches!(block, ContentBlock::Text(text) if text.contains("plan the call"))
            ),
            "the thinking text is model-internal and must not leak as content"
        );
    }

    // Verifies: gh #289 (an unsigned thinking run writes NO signature
    // key - absent means absent, and the live-verified rule is that the
    // guest then sends no field rather than an empty one)
    #[test]
    fn an_unsigned_thinking_run_writes_no_signature_key() {
        let converted = super::to_wit_request(&request_with(assistant_with(Some(("plan", None)))));
        assert_eq!(
            signature_on_the_wire(&converted),
            None,
            "no signature means no key, never an empty value"
        );
    }

    // Verifies: gh #289 (message extras cross the ABI verbatim, so the
    // reserved map stays a general channel rather than a signature-only
    // slot; a caller-supplied key survives untouched)
    #[test]
    fn message_extras_cross_the_abi_verbatim() {
        let mut message = assistant_with(None);
        message
            .extras
            .insert("vendor-note".to_string(), "carried".to_string());
        message
            .extras
            .insert("empty-value".to_string(), String::new());
        let converted = super::to_wit_request(&request_with(message));
        let seen: Vec<(&str, &str)> = converted.messages[0]
            .extras
            .iter()
            .map(|pair| (pair.key.as_str(), pair.value.as_str()))
            .collect();
        assert!(seen.contains(&("vendor-note", "carried")), "{seen:?}");
        assert!(
            seen.contains(&("empty-value", "")),
            "an empty string is a value, not an absence: {seen:?}"
        );
    }

    // Verifies: gh #289 (a caller that already set the key wins - the
    // host does not overwrite it from the block, so a caller has the last
    // word on its own reserved-map entry)
    #[test]
    fn a_caller_supplied_signature_key_is_not_overwritten() {
        let mut message = assistant_with(Some(("plan", Some("from-the-block"))));
        message.extras.insert(
            THINKING_SIGNATURE_EXTRA.to_string(),
            "from-extras".to_string(),
        );
        let converted = super::to_wit_request(&request_with(message));
        assert_eq!(
            signature_on_the_wire(&converted).as_deref(),
            Some("from-extras"),
            "the host adds the key only when absent"
        );
    }
}
