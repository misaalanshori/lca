//! Split from `lib.rs` (cycle 7, P3). Behaviour unchanged.

use super::*;

pub(super) type DispatchCommandSpec = lca_protocol::CommandSpec;

pub(super) fn schema_work(inner: &Inner) -> Result<ToolSpec, CallError> {
    let (mut store, instance) = inner.checkout_tool()?;
    let schema = instance
        .lca_ext_tool_schema()
        .call_get_schema(&mut store)
        .map_err(|err| inner.classify(err))?;
    let parameters = serde_json::from_str(&schema.parameters)
        .unwrap_or_else(|_| serde_json::json!({ "type": "object" }));
    inner.checkin_tool(store, instance);
    Ok(ToolSpec {
        name: schema.name,
        description: schema.description,
        parameters,
        exposure: lca_protocol::ToolExposure::Direct,
        namespace: None,
        annotations: None,
        extras: Default::default(),
    })
}

pub(super) struct InFlightGuard<'a> {
    pub(super) inner: &'a Inner,
}

impl Drop for InFlightGuard<'_> {
    fn drop(&mut self) {
        self.inner.in_flight.fetch_sub(1, Ordering::SeqCst);
    }
}

pub(super) fn execute_work(
    inner: &Inner,
    call: ToolCall,
) -> Result<lca_protocol::ToolResult, CallError> {
    let (mut store, instance) = inner.checkout_tool()?;
    // The `tools` import parents through this store: a nested call
    // made while this execute runs carries this call's id (gh #77).
    store.data_mut().executing_call = Some(call.call_id.clone());
    let guest_call = lca_ext_abi::host::tool::lca::ext::types::ToolCall {
        call_id: call.call_id,
        name: call.name,
        arguments: call.arguments,
        extras: Vec::new(),
    };
    let _inside_guest = inner.in_flight_guard();
    let guest_result = instance
        .lca_ext_execute()
        .call_run(&mut store, &guest_call)
        .map_err(|err| inner.classify(err))?;
    inner.checkin_tool(store, instance);
    Ok(lca_protocol::ToolResult {
        call_id: guest_result.call_id,
        status: match guest_result.status.as_str() {
            "ok" => ToolResultStatus::Ok,
            "denied" => ToolResultStatus::Denied,
            "timeout" => ToolResultStatus::Timeout,
            _ => ToolResultStatus::Error,
        },
        content: guest_result.content.unwrap_or_default(),
        truncated: guest_result.truncated,
        images: Vec::new(),
        extras: Default::default(),
        exit_code: None,
        full_output_path: None,
        nested: Vec::new(),
    })
}

pub(super) fn command_specs_work(inner: &Inner) -> Result<Vec<DispatchCommandSpec>, CallError> {
    let (mut store, instance) = inner.checkout_command()?;
    let spec = instance
        .lca_ext_command_spec()
        .call_get_spec(&mut store)
        .map_err(|err| inner.classify(err))?;
    inner.checkin_command(store, instance);
    Ok(vec![DispatchCommandSpec {
        name: spec.name,
        hint: spec.hint,
        completion: spec.completion,
        extras: spec
            .extras
            .into_iter()
            .map(|pair| (pair.key, pair.value))
            .collect(),
    }])
}

pub(super) fn invoke_work(
    inner: &Inner,
    leaf: &str,
    argument: &str,
) -> Result<CommandEffect, CallError> {
    // Command dispatch runs on the loop thread (gh #124): no questions.
    super::host_imports::without_dialogs(|| {
        let (mut store, instance) = inner.checkout_command()?;
        let effect = instance
            .lca_ext_invoke()
            .call_run(&mut store, argument)
            .map_err(|err| inner.classify(err))?;
        let _ = leaf;
        use lca_ext_abi::host::command::exports::lca::ext::invoke::Effect;
        inner.checkin_command(store, instance);
        Ok(match effect {
            Effect::InsertText(text) => CommandEffect::InsertText(text),
            Effect::SubmitPrompt(text) => CommandEffect::SubmitPrompt(text),
            Effect::ShowWidget(text) => CommandEffect::ShowWidget(text),
            Effect::None => CommandEffect::None,
        })
    })
}

pub(super) fn pre_tool_work(inner: &Inner, call: ToolCall) -> Result<HookAction, CallError> {
    let (mut store, instance) = inner.checkout_hooks()?;
    let action = instance
        .lca_ext_hook_pre_tool_use()
        .call_on_pre_tool_use(
            &mut store,
            &lca_ext_abi::host::hooks::exports::lca::ext::hook_pre_tool_use::ToolCall {
                call_id: call.call_id,
                name: call.name,
                arguments: call.arguments,
                extras: Vec::new(),
            },
        )
        .map_err(|err| inner.classify(err))?;
    use lca_ext_abi::host::hooks::exports::lca::ext::hook_pre_tool_use::Action;
    inner.checkin_hooks(store, instance);
    Ok(match action {
        Action::Allow => HookAction::Allow,
        Action::Deny(reason) => HookAction::Deny(reason),
        Action::Replace(replacement) => HookAction::Replace(ToolCall {
            call_id: replacement.call_id,
            name: replacement.name,
            arguments: replacement.arguments,
            parent_call_id: None,
        }),
    })
}

pub(super) fn observe_work(
    inner: &Inner,
    observation: Option<&PostToolObservation>,
    status: Option<&str>,
    attention: Option<&str>,
) -> Result<(), CallError> {
    let (mut store, instance) = inner.checkout_hooks()?;
    if let Some(observation) = observation {
        instance
            .lca_ext_hook_post_tool_use()
            .call_on_post_tool_use(
                &mut store,
                &lca_ext_abi::host::hooks::exports::lca::ext::hook_post_tool_use::ToolCall {
                    call_id: observation.call.call_id.clone(),
                    name: observation.call.name.clone(),
                    arguments: observation.call.arguments.clone(),
                    extras: Vec::new(),
                },
                &lca_ext_abi::host::hooks::exports::lca::ext::hook_post_tool_use::ToolResult {
                    call_id: observation.result.call_id.clone(),
                    status: match observation.result.status {
                        ToolResultStatus::Ok => "ok".to_string(),
                        ToolResultStatus::Error => "error".to_string(),
                        ToolResultStatus::Denied => "denied".to_string(),
                        ToolResultStatus::Timeout => "timeout".to_string(),
                    },
                    content: Some(observation.result.content.clone()),
                    truncated: observation.result.truncated,
                    extras: Vec::new(),
                },
            )
            .map_err(|err| inner.classify(err))?;
    } else if let Some(status) = status {
        instance
            .lca_ext_hook_post_turn_end()
            .call_on_post_turn_end(&mut store, status)
            .map_err(|err| inner.classify(err))?;
    } else if let Some(reason) = attention {
        instance
            .lca_ext_hook_attention_required()
            .call_on_attention_required(&mut store, reason)
            .map_err(|err| inner.classify(err))?;
    } else {
        instance
            .lca_ext_hook_pre_turn()
            .call_on_pre_turn(&mut store)
            .map_err(|err| inner.classify(err))?;
    }
    inner.checkin_hooks(store, instance);
    Ok(())
}

pub(super) fn session_close_work(inner: &Inner) -> Result<(), CallError> {
    let (mut store, instance) = inner.checkout_hooks()?;
    instance
        .lca_ext_hook_session_close()
        .call_on_session_close(&mut store)
        .map_err(|err| inner.classify(err))?;
    inner.checkin_hooks(store, instance);
    Ok(())
}

/// Parse one catalog spec (gh #77): exposure refuses loudly on
/// unknown values; everything else degrades to the single-tool shape
/// (`direct`, no namespace, no annotations).
fn parse_catalog_spec(
    spec: &lca_ext_abi::host::tool_catalog::exports::lca::ext::catalog::ToolSpec,
) -> Result<ToolSpec, CallError> {
    let exposure =
        lca_protocol::ToolExposure::parse(&spec.exposure).map_err(CallError::InvalidArguments)?;
    let parameters = serde_json::from_str(&spec.parameters)
        .unwrap_or_else(|_| serde_json::json!({ "type": "object" }));
    Ok(ToolSpec {
        name: spec.name.clone(),
        description: spec.description.clone(),
        parameters,
        exposure,
        namespace: spec
            .namespace
            .as_ref()
            .map(|namespace| lca_protocol::ToolNamespace {
                name: namespace.name.clone(),
                description: namespace.description.clone(),
                instructions: namespace.instructions.clone(),
            }),
        annotations: spec
            .annotations
            .as_ref()
            .map(|annotations| lca_protocol::ToolAnnotations {
                read_only_hint: annotations.read_only_hint,
                destructive_hint: annotations.destructive_hint,
                idempotent_hint: annotations.idempotent_hint,
                open_world_hint: annotations.open_world_hint,
            }),
        extras: spec
            .extras
            .iter()
            .map(|pair| (pair.key.clone(), pair.value.clone()))
            .collect(),
    })
}

pub(super) fn catalog_specs_work(inner: &Inner) -> Result<Vec<ToolSpec>, CallError> {
    let (mut store, instance) = inner.checkout_tool_catalog()?;
    let specs = instance
        .lca_ext_catalog()
        .call_get_tools(&mut store)
        .map_err(|err| inner.classify(err))?;
    inner.checkin_tool_catalog(store, instance);
    specs.iter().map(parse_catalog_spec).collect()
}

pub(super) fn execute_catalog_work(
    inner: &Inner,
    name: &str,
    call: ToolCall,
) -> Result<lca_protocol::ToolResult, CallError> {
    let (mut store, instance) = inner.checkout_tool_catalog()?;
    store.data_mut().executing_call = Some(call.call_id.clone());
    let guest_call = lca_ext_abi::host::tool_catalog::lca::ext::types::ToolCall {
        call_id: call.call_id,
        name: call.name,
        arguments: call.arguments,
        extras: Vec::new(),
    };
    let _inside_guest = inner.in_flight_guard();
    let guest_result = instance
        .lca_ext_catalog_run()
        .call_run_tool(&mut store, name, &guest_call)
        .map_err(|err| inner.classify(err))?;
    inner.checkin_tool_catalog(store, instance);
    Ok(map_guest_result(guest_result))
}

/// Map a catalog-world result onto the protocol result (the same
/// status vocabulary as the single-tool world).
fn map_guest_result(
    guest_result: lca_ext_abi::host::tool_catalog::exports::lca::ext::catalog_run::ToolResult,
) -> lca_protocol::ToolResult {
    lca_protocol::ToolResult {
        call_id: guest_result.call_id,
        status: match guest_result.status.as_str() {
            "ok" => ToolResultStatus::Ok,
            "denied" => ToolResultStatus::Denied,
            "timeout" => ToolResultStatus::Timeout,
            _ => ToolResultStatus::Error,
        },
        content: guest_result.content.unwrap_or_default(),
        truncated: guest_result.truncated,
        images: Vec::new(),
        extras: Default::default(),
        exit_code: None,
        full_output_path: None,
        nested: Vec::new(),
    }
}

pub(super) fn message_end_work(
    inner: &Inner,
    role: &str,
    text: &str,
) -> Result<Option<String>, CallError> {
    if inner.hooks_message.is_none() {
        return Ok(None);
    }
    let (mut store, instance) = inner.checkout_hooks_message()?;
    let out = instance
        .lca_ext_hook_message_end()
        .call_on_message_end(&mut store, role, text)
        .map_err(|err| inner.classify(err))?;
    inner.checkin_hooks_message(store, instance);
    Ok(out.replacement)
}

pub(super) fn tool_call_work(
    inner: &Inner,
    call: ToolCall,
) -> Result<lca_protocol::ToolCallPatch, CallError> {
    if inner.hooks_tool_call.is_none() {
        return Ok(lca_protocol::ToolCallPatch::default());
    }
    let (mut store, instance) = inner.checkout_hooks_tool_call()?;
    let guest = lca_ext_abi::host::hooks_tool_call::lca::ext::types::ToolCall {
        call_id: call.call_id,
        name: call.name,
        arguments: call.arguments,
        extras: Vec::new(),
    };
    let out = instance
        .lca_ext_hook_tool_call()
        .call_on_tool_call(&mut store, &guest)
        .map_err(|err| inner.classify(err))?;
    inner.checkin_hooks_tool_call(store, instance);
    Ok(lca_protocol::ToolCallPatch {
        arguments: out.arguments,
        block: out.block,
    })
}

pub(super) fn tool_result_work(
    inner: &Inner,
    call: &ToolCall,
    result: &lca_protocol::ToolResult,
) -> Result<lca_protocol::ToolResultPatch, CallError> {
    if inner.hooks_tool_result.is_none() {
        return Ok(lca_protocol::ToolResultPatch::default());
    }
    let (mut store, instance) = inner.checkout_hooks_tool_result()?;
    use lca_ext_abi::host::hooks_tool_result::lca::ext::types as wit;
    let guest_call = wit::ToolCall {
        call_id: call.call_id.clone(),
        name: call.name.clone(),
        arguments: call.arguments.clone(),
        extras: Vec::new(),
    };
    let guest_result = wit::ToolResult {
        call_id: result.call_id.clone(),
        status: match result.status {
            ToolResultStatus::Ok => "ok".to_string(),
            ToolResultStatus::Error => "error".to_string(),
            ToolResultStatus::Denied => "denied".to_string(),
            ToolResultStatus::Timeout => "timeout".to_string(),
        },
        content: Some(result.content.clone()),
        truncated: result.truncated,
        extras: Vec::new(),
    };
    let out = instance
        .lca_ext_hook_tool_result()
        .call_on_tool_result(&mut store, &guest_call, &guest_result)
        .map_err(|err| inner.classify(err))?;
    inner.checkin_hooks_tool_result(store, instance);
    Ok(lca_protocol::ToolResultPatch {
        content: out.content,
        is_error: out.is_error,
    })
}

pub(super) fn stream_event_work(
    inner: &Inner,
    provider: &str,
    model: &str,
    kind: &str,
    data: &str,
) -> Result<(), CallError> {
    if inner.hooks_stream.is_none() {
        return Ok(());
    }
    let (mut store, instance) = inner.checkout_hooks_stream()?;
    instance
        .lca_ext_hook_stream_event()
        .call_on_stream_event(&mut store, provider, model, kind, data)
        .map_err(|err| inner.classify(err))?;
    inner.checkin_hooks_stream(store, instance);
    Ok(())
}

fn map_settle(
    out: lca_ext_abi::host::hooks_settle::exports::lca::ext::hook_turn_end::SettleDecision,
) -> lca_protocol::SettleDecision {
    lca_protocol::SettleDecision {
        append: out.append,
        continue_once: out.continue_once,
    }
}

pub(super) fn turn_end_work(
    inner: &Inner,
    rounds: u32,
    tool_calls: u32,
    status: &str,
) -> Result<lca_protocol::SettleDecision, CallError> {
    if inner.hooks_settle.is_none() {
        return Ok(lca_protocol::SettleDecision::default());
    }
    let (mut store, instance) = inner.checkout_hooks_settle()?;
    let summary =
        lca_ext_abi::host::hooks_settle::exports::lca::ext::hook_turn_end::SettleSummary {
            rounds,
            tool_calls,
            status: status.to_string(),
        };
    let out = instance
        .lca_ext_hook_turn_end()
        .call_on_turn_end(&mut store, &summary)
        .map_err(|err| inner.classify(err))?;
    inner.checkin_hooks_settle(store, instance);
    Ok(map_settle(out))
}

pub(super) fn before_settle_work(
    inner: &Inner,
    rounds: u32,
    tool_calls: u32,
    status: &str,
) -> Result<lca_protocol::SettleDecision, CallError> {
    if inner.hooks_settle.is_none() {
        return Ok(lca_protocol::SettleDecision::default());
    }
    let (mut store, instance) = inner.checkout_hooks_settle()?;
    let summary =
        lca_ext_abi::host::hooks_settle::exports::lca::ext::hook_turn_end::SettleSummary {
            rounds,
            tool_calls,
            status: status.to_string(),
        };
    let out = instance
        .lca_ext_hook_agent_before_settle()
        .call_on_agent_before_settle(&mut store, &summary)
        .map_err(|err| inner.classify(err))?;
    inner.checkin_hooks_settle(store, instance);
    Ok(map_settle(out))
}

pub(super) fn before_compact_work(
    inner: &Inner,
    reason: &str,
) -> Result<lca_protocol::CompactVerdict, CallError> {
    if inner.hooks_compaction.is_none() {
        return Ok(lca_protocol::CompactVerdict::Allow);
    }
    let (mut store, instance) = inner.checkout_hooks_compaction()?;
    let out = instance
        .lca_ext_hook_session_before_compact()
        .call_on_session_before_compact(&mut store, reason)
        .map_err(|err| inner.classify(err))?;
    use lca_ext_abi::host::hooks_compaction::exports::lca::ext::hook_session_before_compact::CompactVerdict as WitVerdict;
    inner.checkin_hooks_compaction(store, instance);
    Ok(match out {
        WitVerdict::Allow => lca_protocol::CompactVerdict::Allow,
        WitVerdict::Deny(reason) => lca_protocol::CompactVerdict::Deny(reason),
    })
}

pub(super) fn compact_failed_work(
    inner: &Inner,
    reason: &str,
    error: Option<&str>,
) -> Result<(), CallError> {
    if inner.hooks_compaction.is_none() {
        return Ok(());
    }
    let (mut store, instance) = inner.checkout_hooks_compaction()?;
    instance
        .lca_ext_hook_session_compact_failed()
        .call_on_session_compact_failed(&mut store, reason, error)
        .map_err(|err| inner.classify(err))?;
    inner.checkin_hooks_compaction(store, instance);
    Ok(())
}

pub(super) fn cache_decision_work(
    inner: &Inner,
    provider: &str,
    model: &str,
) -> Result<bool, CallError> {
    if inner.hooks_cache.is_none() {
        return Ok(true);
    }
    let (mut store, instance) = inner.checkout_hooks_cache()?;
    let out = instance
        .lca_ext_hook_cache_warming()
        .call_on_cache_warming_decision(&mut store, provider, model)
        .map_err(|err| inner.classify(err))?;
    inner.checkin_hooks_cache(store, instance);
    Ok(out.warm)
}

pub(super) fn trust_work(
    inner: &Inner,
    cwd: &str,
) -> Result<(lca_protocol::TrustVote, bool), CallError> {
    if inner.hooks_trust.is_none() {
        return Ok((lca_protocol::TrustVote::Undecided, false));
    }
    let (mut store, instance) = inner.checkout_hooks_trust()?;
    let out = instance
        .lca_ext_hook_project_trust()
        .call_on_project_trust(&mut store, cwd)
        .map_err(|err| inner.classify(err))?;
    inner.checkin_hooks_trust(store, instance);
    Ok((lca_protocol::TrustVote::parse(&out.trusted), out.remember))
}
