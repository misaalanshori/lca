//! Tool execution (gh #77 + #45): one model-issued or nested call
//! through mutation, verdict, permission, dispatch, and composition.
//! Split from `turn/mod.rs` for the workspace file ceiling (gate 11).

use std::sync::Arc;

use lca_protocol::{
    DispatchError, FORMAT_VERSION, HookAction, Record, StopReason, ToolCall, ToolResult,
    ToolResultStatus, TurnEvent, TurnOutcome,
};
use lca_tools::{CancelFlag, ToolExecutor};

use crate::{Agent, TurnSink};

/// Deferred discovery, served by the turn (gh #77, pi's deferred):
/// only active `direct` tools declare; everything undisclosed stays
/// out of the request until the model searches and the hit activates.
pub(super) fn tool_search_spec() -> lca_protocol::ToolSpec {
    lca_protocol::ToolSpec {
        name: crate::registry::TOOL_SEARCH_NAME.to_string(),
        description: "Find undisclosed tools by keyword and activate them for later requests.\
            Returns matching tools with their descriptions."
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": { "query": { "type": "string" } },
        }),
        exposure: lca_protocol::ToolExposure::Direct,
        namespace: None,
        annotations: None,
        extras: Default::default(),
    }
}

/// How deep tools may nest (gh #77): the host assigns
/// `<parent>/<n>` ids, and past this depth the call fails as an
/// error result instead of recursing (pi never rejects: unknown
/// tools, validation errors, and blocks all arrive as results).
const MAX_NESTED_DEPTH: u32 = 8;

/// How many nested calls one parent's result record keeps (gh #77's
/// bounded `nestedCalls`): the first entries win; the count notes
/// what fell off.
const MAX_NESTED_RECORDS: usize = 20;

/// Characters kept per nested entry: a bounded record, not a
/// transcript (nested calls never appear as their own records).
const NESTED_CONTENT_HEAD: usize = 500;

/// One nested call's bounded record (gh #77).
#[derive(Debug, Clone)]
pub(super) struct NestedSummary {
    /// The nested tool's name.
    name: String,
    /// Whether it worked.
    status: ToolResultStatus,
    /// The content's head.
    content_head: String,
}

/// The turn's nested-call server (gh #77): the `tools` import sends
/// here from blocking threads while the turn serves on its own task.
/// The turn installs it around the run and clears it after, so a
/// stray request outside a turn fails instead of hanging.
pub(super) struct NestedServer {
    /// Incoming nested requests.
    pub(super) rx: tokio::sync::mpsc::UnboundedReceiver<lca_ext_abi::NestedCall>,
    /// `<parent id>` nth-child counters for `<parent>/<n>` ids.
    pub(super) counts: std::collections::HashMap<String, u64>,
    /// Bounded per-parent records, attached when the parent persists.
    pub(super) records: std::collections::HashMap<String, Vec<NestedSummary>>,
}

/// Clears the registry's nested slot when the turn ends, on every
/// path (a stale slot would serve a dead turn's requests).
pub(super) struct NestedGuard {
    pub(super) registry: Arc<crate::registry::ExtensionRegistry>,
}

impl Drop for NestedGuard {
    fn drop(&mut self) {
        self.registry.clear_nested();
    }
}
impl Agent<'_> {
    /// One tool call: composable mutation first (gh #45), then the
    /// pre-tool verdict (FR-CORE-10 — a hook denial ends the call
    /// without ever prompting), then the permission layer on whatever
    /// call survives (a replaced call passes through like any other
    /// and is not re-hooked), then execution with the nested server
    /// draining (gh #77), then result composition (gh #45). The
    /// result record, the sink event, and the `post-tool-use` hook
    /// all belong here so no caller can forget one.
    // TurnOutcome grew with Usage's cost buckets past clippy's preferred
    // Err size; boxing it would ripple through every caller for a lint.
    #[allow(clippy::result_large_err)]
    pub(super) async fn run_tool_call(
        &mut self,
        call: &ToolCall,
        sink: &mut dyn TurnSink,
        cancel: &CancelFlag,
        server: &mut NestedServer,
    ) -> Result<(), TurnOutcome> {
        // The full path without the transcript tail: mutation,
        // verdict, execution, composition (gh #45 + #77). Nested
        // calls serve through this; the transcript tail (persist,
        // finish event, observe) stays the caller's.
        let (effective, result) = self.run_call_full(call, 0, sink, cancel, server).await?;
        self.finish_tool_call(&effective, result, sink, server)
            .await
    }

    /// Mutate, verdict, execute, compose (gh #45's chain around the
    /// FR-CORE-10 path). Emits `ToolStarted` once the call is final,
    /// so denied and hook-refused calls stay visible; the finish
    /// event and the record belong to the caller.
    #[allow(clippy::result_large_err)]
    async fn run_call_full(
        &mut self,
        call: &ToolCall,
        depth: u32,
        sink: &mut dyn TurnSink,
        cancel: &CancelFlag,
        server: &mut NestedServer,
    ) -> Result<(ToolCall, ToolResult), TurnOutcome> {
        let registry = self.config.extensions.clone();
        // Gh #45's mutation chain runs before the verdict: every
        // handler sees the previous handler's arguments.
        let mutated = match registry.mutate_tool_call(call).await {
            Ok(call) => call,
            Err(reason) => {
                let effective = call.clone();
                sink.on_event(TurnEvent::ToolStarted(effective.clone()));
                let denied = ToolResult::denied(call.call_id.clone(), reason);
                let composed = registry.compose_tool_result(&effective, denied).await;
                return Ok((effective, composed));
            }
        };
        let mut hook_errors: Vec<(String, String)> = Vec::new();
        let mut effective = mutated;
        let action = {
            let mut observe = |handle: &Arc<dyn lca_ext_abi::ExtensionDispatch>,
                               err: &DispatchError| {
                hook_errors.push((handle.name().to_string(), err.to_string()));
            };
            registry.pre_tool_use(&effective, &mut observe).await
        };
        for (extension, detail) in hook_errors {
            if let Err(err) = self.record_extension_event(&extension, "error", &detail, sink) {
                return Err(self.fail(StopReason::Error, err));
            }
        }

        // The card exists from the moment the model *asks*: pi builds tool
        // cards while the call is still streaming, and the session log has
        // already recorded the request (`Record::ToolCall` is written with
        // the response, before any permission). Emitting the start here is
        // what makes a denied, hook-refused, or schema-invalid call visible
        // at all - it settles into the same card instead of leaving the
        // transcript silent - and it keeps the `tool-call` envelope always
        // preceding its `tool-result` (docs/headless.md).
        sink.on_event(TurnEvent::ToolStarted(effective.clone()));

        let result = match action {
            HookAction::Deny(reason) => {
                // No permission prompt: the hook already answered
                // (FR-CORE-10).
                ToolResult::denied(effective.call_id.clone(), reason)
            }
            HookAction::Replace(replacement) => {
                effective = replacement;
                self.execute_after_permission(&effective, &registry, sink, cancel, server, depth)
                    .await?
            }
            HookAction::Allow => {
                self.execute_after_permission(&effective, &registry, sink, cancel, server, depth)
                    .await?
            }
        };
        // Gh #45's composition runs over the executed result: the
        // composed text is what the log keeps and the model sees.
        let result = registry.compose_tool_result(&effective, result).await;
        Ok((effective, result))
    }

    /// Serve one nested request (gh #77): assign `<parent>/<n>`, run
    /// the full path at the next depth, emit its events with the
    /// parent attached, keep the bounded record, and reply. Never
    /// rejects: every failure mode answers as an error result.
    async fn serve_nested(
        &mut self,
        request: lca_ext_abi::NestedCall,
        depth: u32,
        sink: &mut dyn TurnSink,
        cancel: &CancelFlag,
        server: &mut NestedServer,
    ) {
        let lca_ext_abi::NestedCall {
            parent_call_id,
            name,
            arguments,
            reply,
        } = request;
        // The child's id first: even a refused call answers with the
        // id its events would have carried.
        let nth = server.counts.entry(parent_call_id.clone()).or_insert(0);
        *nth += 1;
        let child_id = format!("{parent_call_id}/{nth}");
        let refused_id = child_id.clone();
        let refused = |reason: String| {
            let _ = reply.send(ToolResult::error(refused_id.clone(), reason));
        };
        if depth > MAX_NESTED_DEPTH {
            refused(format!(
                "tool `{name}` nests deeper than {MAX_NESTED_DEPTH} levels; refusing"
            ));
            return;
        }
        if cancel.is_cancelled() {
            refused("the turn was cancelled".to_string());
            return;
        }
        let registry = self.config.extensions.clone();
        if !registry.is_callable(&name) {
            refused(format!("tool `{name}` is not callable right now"));
            return;
        }
        let call = ToolCall {
            call_id: child_id,
            name: name.clone(),
            arguments,
            parent_call_id: Some(parent_call_id.clone()),
        };
        let result = match Box::pin(self.run_call_full(&call, depth, sink, cancel, server)).await {
            Ok((_, result)) => result,
            Err(_) => {
                // The turn itself failed (the store, not the tool):
                // answer the error and let the top-level path surface it.
                refused("the turn failed while serving the nested call".to_string());
                return;
            }
        };
        sink.on_event(TurnEvent::ToolFinished(result.clone()));
        // The bounded record (gh #77's `nestedCalls`): filed under
        // the parent, where the parent's persist picks it up. The
        // first entries win; the parent's result carries what fits.
        let entries = server.records.entry(parent_call_id).or_default();
        if entries.len() < MAX_NESTED_RECORDS {
            entries.push(NestedSummary {
                name,
                status: result.status,
                content_head: result.content.chars().take(NESTED_CONTENT_HEAD).collect(),
            });
        }
        registry.on_post_tool_use(&call, &result).await;
        let _ = reply.send(result);
    }

    /// Run the parent tool while serving nested requests (gh #77):
    /// the parent future borrows only its handle, never the turn, so
    /// the turn stays free to serve on the same task. Depth counts
    /// from the top-level call (0); children nest below it.
    async fn execute_with_nested(
        &mut self,
        parent: lca_ext_abi::DispatchFuture<'_, Result<ToolResult, DispatchError>>,
        depth: u32,
        sink: &mut dyn TurnSink,
        cancel: &CancelFlag,
        server: &mut NestedServer,
    ) -> Result<ToolResult, DispatchError> {
        let mut parent = parent;
        loop {
            tokio::select! {
                result = &mut parent => return result,
                request = server.rx.recv() => {
                    if let Some(request) = request {
                        self.serve_nested(request, depth + 1, sink, cancel, server).await;
                    }
                }
            }
        }
    }

    /// Persist the result, emit its event, and observe (gh #45's
    /// `post-tool-use` still sees every call, composed or not).
    /// Shared by top-level calls and the mutation-blocked path so a
    /// denied-before-permission call records exactly like one.
    #[allow(clippy::result_large_err)]
    async fn finish_tool_call(
        &mut self,
        effective: &ToolCall,
        result: ToolResult,
        sink: &mut dyn TurnSink,
        server: &mut NestedServer,
    ) -> Result<(), TurnOutcome> {
        let registry = self.config.extensions.clone();
        // The id first: a `message_end` replacement below targets
        // this record, not the call. The bounded nested record
        // attaches here, so the event and the log share it (gh #77).
        let result_id = lca_session::new_record_id();
        let mut result = result;
        result.nested = std::mem::take(server.records.entry(result.call_id.clone()).or_default())
            .into_iter()
            .map(|summary| lca_protocol::NestedCallRecord {
                name: summary.name,
                status: summary.status,
                content_head: summary.content_head,
            })
            .collect();
        if let Err(err) = self.store.append(
            self.session,
            Record::ToolResult {
                v: FORMAT_VERSION,
                ts: lca_session::now_ms(),
                id: result_id.clone(),
                call_id: result.call_id.clone(),
                status: result.status,
                content: Some(result.content.clone()),
                attachment: result.extras.get("attachment").cloned(),
                truncated: result.truncated,
                exit_code: result.exit_code,
                full_output_path: result.full_output_path.clone(),
                nested: result.nested.clone(),
            },
        ) {
            return Err(self.fail(
                StopReason::Error,
                format!("cannot write to the session log: {err}"),
            ));
        }
        sink.on_event(TurnEvent::ToolFinished(result.clone()));
        // Gh #45's `message_end` fires for finalized tool results;
        // a replacement lands as an append-only edit (never a
        // rewrite), keeping role and tool linkage.
        if let Some(replacement) = registry
            .message_end_replacement("tool-result", &result.content)
            .await
        {
            let _ = self.store.append(
                self.session,
                Record::ContextEdit {
                    v: FORMAT_VERSION,
                    ts: lca_session::now_ms(),
                    id: lca_session::new_record_id(),
                    target_id: result_id,
                    replacement: Some(replacement),
                },
            );
        }
        registry.on_post_tool_use(effective, &result).await;
        Ok(())
    }

    /// The permission layer plus execution (FR-TOOL-3's path), shared by
    /// allow and replace, top-level and nested alike.
    #[allow(clippy::result_large_err)]
    async fn execute_after_permission(
        &mut self,
        call: &ToolCall,
        registry: &Arc<crate::registry::ExtensionRegistry>,
        sink: &mut dyn TurnSink,
        cancel: &CancelFlag,
        server: &mut NestedServer,
        depth: u32,
    ) -> Result<ToolResult, TurnOutcome> {
        // Phase 2 seam note: hooks have already run by the time this is
        // called (FR-CORE-10: hook before permission).
        //
        // Validate the arguments against the schema the model saw before the
        // tool runs (extension authoring guide); an invalid call never reaches
        // the tool. Extension schemas come from the registry, built-ins from
        // the executor's own table, discovery from its static spec.
        let schema = registry
            .tool_schema(&call.name)
            .map(|spec| spec.parameters.clone())
            .or_else(|| {
                ToolExecutor::specs(self.tools.resolved_shell())
                    .into_iter()
                    .find(|spec| spec.name == call.name)
                    .map(|spec| spec.parameters)
            })
            .or_else(|| {
                (call.name == crate::registry::TOOL_SEARCH_NAME)
                    .then(|| tool_search_spec().parameters)
            });
        if let Some(schema) = schema
            && let Err(reason) = lca_provider::validate_against_schema(&schema, &call.arguments)
        {
            return Ok(ToolResult::error(
                call.call_id.clone(),
                format!("invalid arguments for `{}`: {reason}", call.name),
            ));
        }
        if let Some(action) = self.tools.required_permission(call) {
            match self.authorize(call, &action) {
                Ok(None) => {}
                Ok(Some(denied)) => return Ok(denied),
                Err(outcome) => return Err(outcome),
            }
        }

        // Deferred discovery (gh #77): search, activate the hits, and
        // report them. The next request declares the newly active.
        if call.name == crate::registry::TOOL_SEARCH_NAME {
            return Ok(self.run_tool_search(call, registry));
        }

        // Extension tool or built-in: one dispatch table, no mode
        // branching at this call site beyond asking who owns the name
        // (FR-EXT-6 lives in the registry's single trait). Extension
        // execution drains the nested server while it runs (gh #77);
        // built-ins never nest, so they run straight through.
        if let Some(handle) = registry.tool_owner(&call.name).cloned() {
            let executed = self
                .execute_with_nested(handle.execute_tool(call), depth, sink, cancel, server)
                .await;
            return Ok(match executed {
                Ok(result) => result,
                Err(err) => {
                    let event = match err {
                        DispatchError::Disabled => "disabled",
                        _ => "error",
                    };
                    if let Err(write_err) =
                        self.record_extension_event(handle.name(), event, &err.to_string(), sink)
                    {
                        return Err(self.fail(StopReason::Error, write_err));
                    }
                    ToolResult::error(call.call_id.clone(), err.to_string())
                }
            });
        }

        let cancel_for_tool = cancel.clone();
        let mut sink_chunk = |chunk: &[u8]| {
            sink.on_event(TurnEvent::ToolOutputChunk {
                call_id: call.call_id.clone(),
                chunk: String::from_utf8_lossy(chunk).into_owned(),
            });
        };
        let result = self
            .tools
            .execute(call, &mut sink_chunk, &cancel_for_tool)
            .await;
        Ok(result)
    }

    /// Deferred discovery served (gh #77): match, activate the hits,
    /// and report them with their namespaces. Unknown queries match
    /// nothing; an empty query lists every discoverable tool.
    fn run_tool_search(
        &self,
        call: &ToolCall,
        registry: &Arc<crate::registry::ExtensionRegistry>,
    ) -> ToolResult {
        let query = serde_json::from_str::<serde_json::Value>(&call.arguments)
            .ok()
            .and_then(|value| {
                value
                    .get("query")
                    .and_then(|q| q.as_str())
                    .map(str::to_string)
            })
            .unwrap_or_default();
        let hits = registry.tool_search(&query);
        if hits.is_empty() {
            return ToolResult::ok(
                call.call_id.clone(),
                format!("no undisclosed tools match `{query}`"),
            );
        }
        let names: Vec<String> = hits.iter().map(|spec| spec.name.clone()).collect();
        let mut active = registry.active_tools();
        active.extend(names.iter().cloned());
        active.sort();
        active.dedup();
        let _ = registry.set_active_tools(&active);
        let mut lines = vec![format!("{} tool(s) activated:", hits.len())];
        for spec in &hits {
            match &spec.namespace {
                Some(namespace) => lines.push(format!(
                    "- {} [{}]: {}",
                    spec.name, namespace.name, spec.description
                )),
                None => lines.push(format!("- {}: {}", spec.name, spec.description)),
            }
        }
        ToolResult::ok(call.call_id.clone(), lines.join("\n"))
    }

    /// The permission check for a gated tool: lock the shared store for the
    /// authorize call only, record the decision, and report a denial as a
    /// denied result. `Ok(None)` means proceed; `Ok(Some(result))` is the
    /// denial to return.
    #[allow(clippy::result_large_err)]
    fn authorize(
        &mut self,
        call: &ToolCall,
        action: &lca_permissions::Action,
    ) -> Result<Option<ToolResult>, TurnOutcome> {
        // The grant store is shared with every capability engine and the
        // login flow, so it is locked for the authorize call only, never
        // for the whole turn: an extension's own permission check during
        // this turn re-enters through the same Arc.
        let grants = self.grants.clone();
        let mut guard = match grants.lock() {
            Ok(guard) => guard,
            Err(_) => {
                return Err(self.fail(
                    StopReason::Error,
                    "permission store lock is poisoned".to_string(),
                ));
            }
        };
        let outcome = match lca_permissions::authorize(
            &mut guard,
            self.tools.workspace(),
            action,
            self.proposals,
            self.prompt,
        ) {
            Ok(outcome) => outcome,
            Err(err) => {
                // #152: a grant-store write failure warns instead of
                // evaporating; the `~/.lca/logs/lca.log` line is what a
                // later reader has when the turn itself only says it
                // failed.
                tracing::warn!(%err, "permission store error; failing the turn");
                return Err(self.fail(StopReason::Error, format!("permission store error: {err}")));
            }
        };
        // A prompted answer and a yolo answer both belong in the log; a
        // rule denial is recorded too (ADR-0042: approve everything must
        // never mean forget everything).
        if outcome.prompted || outcome.denied_by_rule || outcome.yolo {
            let record = Record::Permission {
                v: FORMAT_VERSION,
                ts: lca_session::now_ms(),
                id: lca_session::new_record_id(),
                action: action.display(),
                decision: if outcome.stored_pattern.is_some() {
                    lca_protocol::PermissionDecision::Always
                } else if outcome.allowed {
                    lca_protocol::PermissionDecision::Once
                } else {
                    lca_protocol::PermissionDecision::Denied
                },
                pattern: outcome.stored_pattern.clone(),
            };
            if let Err(err) = self.store.append(self.session, record) {
                tracing::error!(%err, "cannot record permission decision");
            }
        }
        if !outcome.allowed {
            let reason = if outcome.denied_by_rule {
                format!("A permission rule denied this action: {}", action.display())
            } else {
                format!("The user denied this action: {}", action.display())
            };
            return Ok(Some(ToolResult::denied(call.call_id.clone(), reason)));
        }
        Ok(None)
    }
}
