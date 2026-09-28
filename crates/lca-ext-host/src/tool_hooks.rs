//! Split from `lib.rs` (cycle 7, P3). Behaviour unchanged.

use super::*;

pub(super) type DispatchCommandSpec = lca_protocol::CommandSpec;

pub(super) fn schema_work(inner: &Inner) -> Result<ToolSpec, CallError> {
    let pre = inner
        .tool
        .as_ref()
        .ok_or_else(|| CallError::InvalidArguments("no tool world".into()))?;
    let mut store = inner.build_store()?;
    let instance = pre
        .instantiate(&mut store)
        .map_err(|err| inner.classify(err))?;
    let schema = instance
        .lca_ext_tool_schema()
        .call_get_schema(&mut store)
        .map_err(|err| inner.classify(err))?;
    let parameters = serde_json::from_str(&schema.parameters)
        .unwrap_or_else(|_| serde_json::json!({ "type": "object" }));
    Ok(ToolSpec {
        name: schema.name,
        description: schema.description,
        parameters,
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
    let pre = inner
        .tool
        .as_ref()
        .ok_or_else(|| CallError::InvalidArguments("no tool world".into()))?;
    let mut store = inner.build_store()?;
    let instance = pre
        .instantiate(&mut store)
        .map_err(|err| inner.classify(err))?;
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
    })
}

pub(super) fn command_specs_work(inner: &Inner) -> Result<Vec<DispatchCommandSpec>, CallError> {
    let pre = inner
        .command
        .as_ref()
        .ok_or_else(|| CallError::InvalidArguments("no command world".into()))?;
    let mut store = inner.build_store()?;
    let instance = pre
        .instantiate(&mut store)
        .map_err(|err| inner.classify(err))?;
    let spec = instance
        .lca_ext_command_spec()
        .call_get_spec(&mut store)
        .map_err(|err| inner.classify(err))?;
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
    let pre = inner
        .command
        .as_ref()
        .ok_or_else(|| CallError::InvalidArguments("no command world".into()))?;
    let mut store = inner.build_store()?;
    let instance = pre
        .instantiate(&mut store)
        .map_err(|err| inner.classify(err))?;
    let effect = instance
        .lca_ext_invoke()
        .call_run(&mut store, argument)
        .map_err(|err| inner.classify(err))?;
    let _ = leaf;
    use lca_ext_abi::host::command::exports::lca::ext::invoke::Effect;
    Ok(match effect {
        Effect::InsertText(text) => CommandEffect::InsertText(text),
        Effect::SubmitPrompt(text) => CommandEffect::SubmitPrompt(text),
        Effect::ShowWidget(text) => CommandEffect::ShowWidget(text),
        Effect::None => CommandEffect::None,
    })
}

pub(super) fn pre_tool_work(inner: &Inner, call: ToolCall) -> Result<HookAction, CallError> {
    let pre = inner
        .hooks
        .as_ref()
        .ok_or_else(|| CallError::InvalidArguments("no hooks world".into()))?;
    let mut store = inner.build_store()?;
    let instance = pre
        .instantiate(&mut store)
        .map_err(|err| inner.classify(err))?;
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
    Ok(match action {
        Action::Allow => HookAction::Allow,
        Action::Deny(reason) => HookAction::Deny(reason),
        Action::Replace(replacement) => HookAction::Replace(ToolCall {
            call_id: replacement.call_id,
            name: replacement.name,
            arguments: replacement.arguments,
        }),
    })
}

pub(super) fn observe_work(
    inner: &Inner,
    observation: Option<&PostToolObservation>,
    status: Option<&str>,
    attention: Option<&str>,
) -> Result<(), CallError> {
    let pre = inner
        .hooks
        .as_ref()
        .ok_or_else(|| CallError::InvalidArguments("no hooks world".into()))?;
    let mut store = inner.build_store()?;
    let instance = pre
        .instantiate(&mut store)
        .map_err(|err| inner.classify(err))?;
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
    Ok(())
}

pub(super) fn session_close_work(inner: &Inner) -> Result<(), CallError> {
    let pre = inner
        .hooks
        .as_ref()
        .ok_or_else(|| CallError::InvalidArguments("no hooks world".into()))?;
    let mut store = inner.build_store()?;
    let instance = pre
        .instantiate(&mut store)
        .map_err(|err| inner.classify(err))?;
    instance
        .lca_ext_hook_session_close()
        .call_on_session_close(&mut store)
        .map_err(|err| inner.classify(err))
}
