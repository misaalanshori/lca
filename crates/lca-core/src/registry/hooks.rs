//! The hook chain (gh #548): every `World::Hooks*` chain the host runs - the
//! turn boundary, tool-call and tool-result composition, settle, compaction,
//! cache warming, trust, stream observation, session close - plus the context
//! transform chain, which is the same merge rule over a different world.
//!
//! **A reader looking for capability enforcement should follow this chain**
//! (`docs/capabilities.md`, FR-EXT-3/FR-PERM-*): the host asks each extension in
//! registration order and a failure disables or skips rather than ends, so a
//! broken hook cannot wedge a turn.
//!
//! Split from `registry/mod.rs` for the 1,200-line ceiling; the chains are
//! methods on the one [`ExtensionRegistry`], and `enabled()` (defined there) is
//! the only gate every one of them passes through.

use std::sync::Arc;

use lca_ext_abi::{ExtensionDispatch, World};
use lca_protocol::{ChatMessage, DispatchError, HookAction, PostToolObservation, ToolCall};

use super::ExtensionRegistry;

impl ExtensionRegistry {
    /// Apply every enabled `context-transform` extension in
    /// installation order (FR-CTX-2), each output feeding the next. The
    /// first rejection - or host-level failure, which must not let
    /// unreviewed messages through either - stops the chain
    /// (FR-CTX-3).
    ///
    /// Gh #46 (pi's `restoreSystemMessages`): the `context` handlers
    /// own the conversation, not the prompt. After every handler the
    /// host re-attaches the leading system message when the handler
    /// dropped it, so a filter/slice cannot lose the prompt or the
    /// tool declarations and every later handler still sees the full
    /// transcript (pi's `context_with_system` visibility). An edited
    /// system head is honored - only a missing one is restored.
    pub async fn transform(
        &self,
        mut messages: Vec<ChatMessage>,
    ) -> Result<Vec<ChatMessage>, String> {
        // The prompt head the handlers must not lose: the leading
        // system run. Snapshotted once (pi replays the current head
        // per handler; the head cannot change mid-chain except through
        // a handler edit, which the per-output check below honors).
        let head: Vec<ChatMessage> = messages
            .iter()
            .take_while(|message| message.role == lca_protocol::MessageRole::System)
            .cloned()
            .collect();
        for handle in self
            .enabled()
            .filter(|handle| handle.worlds().contains(&World::ContextTransform))
        {
            // The clone keeps the input alive for a Disabled skip;
            // every other arm replaces or returns it anyway.
            match handle.transform_messages(messages.clone()).await {
                Ok(Ok(list)) => {
                    messages = list;
                    if !head.is_empty()
                        && !messages
                            .first()
                            .is_some_and(|first| first.role == lca_protocol::MessageRole::System)
                    {
                        tracing::warn!(
                            extension = handle.name(),
                            "context transform dropped the leading system message; the prompt was restored"
                        );
                        messages.splice(..0, head.iter().cloned());
                    }
                }
                Ok(Err(reason)) => return Err(reason),
                // A handle that just disabled itself (trapped, hit its
                // limits) drops out of the chain the same way the hooks
                // loops skip it: FR-EXT-5's host-continues rule outranks
                // FR-CTX-2 for a dead extension. Every other failure
                // stops the turn - messages that half-transformed must
                // not reach the provider.
                Err(DispatchError::Disabled) => continue,
                Err(err) => {
                    return Err(format!(
                        "the context transform `{}` failed: {err}",
                        handle.name()
                    ));
                }
            }
        }
        Ok(messages)
    }

    /// Merge the `pre-turn` hook across every enabled extension
    /// (`post-turn` and close hooks are the same loop with other
    /// methods). Errors disable inside the host (FR-EXT-3) and are
    /// skipped here so one broken extension cannot end a turn.
    pub async fn on_pre_turn(&self) {
        for handle in self
            .enabled()
            .filter(|h| h.worlds().contains(&World::Hooks))
        {
            let _ = handle.on_pre_turn().await;
        }
    }

    /// Run the `pre-tool-use` hook across enabled extensions in
    /// registration order: the first deny wins, a replace stops the
    /// chain (the replacement is not re-hooked, SRDD hooks), and hook
    /// errors are reported-and-skipped (FR-EXT-3).
    pub async fn pre_tool_use(
        &self,
        call: &ToolCall,
        on_error: &mut dyn FnMut(&Arc<dyn ExtensionDispatch>, &DispatchError),
    ) -> HookAction {
        for handle in self
            .enabled()
            .filter(|h| h.worlds().contains(&World::Hooks))
        {
            match handle.on_pre_tool_use(call).await {
                Ok(HookAction::Allow) => {}
                Ok(action) => return action,
                Err(err) => on_error(handle, &err),
            }
        }
        HookAction::Allow
    }

    /// `post-tool-use` across enabled hook extensions (FR-CORE-10's
    /// observe side).
    pub async fn on_post_tool_use(&self, call: &ToolCall, result: &lca_protocol::ToolResult) {
        let observation = PostToolObservation {
            call: call.clone(),
            result: result.clone(),
        };
        for handle in self
            .enabled()
            .filter(|h| h.worlds().contains(&World::Hooks))
        {
            let _ = handle.on_post_tool_use(&observation).await;
        }
    }

    /// `post-turn-end` with `ok` or `error`.
    pub async fn on_post_turn_end(&self, status: &str) {
        for handle in self
            .enabled()
            .filter(|h| h.worlds().contains(&World::Hooks))
        {
            let _ = handle.on_post_turn_end(status).await;
        }
    }

    /// `attention-required`: the turn failed and the user should look. The
    /// reason is the same text the interface surfaced.
    pub async fn on_attention_required(&self, reason: &str) {
        for handle in self
            .enabled()
            .filter(|h| h.worlds().contains(&World::Hooks))
        {
            let _ = handle.on_attention_required(reason).await;
        }
    }

    /// Composable `tool_call` mutation (gh #45): every enabled
    /// `hooks-tool-call` extension runs in order, each seeing the
    /// previous handler's arguments. A `block` veto ends the chain
    /// with the reason; hook errors are reported-and-skipped
    /// (FR-EXT-3), never silent vetoes.
    pub async fn mutate_tool_call(&self, call: &ToolCall) -> Result<ToolCall, String> {
        let mut current = call.clone();
        for handle in self
            .enabled()
            .filter(|h| h.worlds().contains(&World::HooksToolCall))
        {
            match handle.on_tool_call(&current).await {
                Ok(patch) => {
                    if let Some(reason) = patch.block {
                        return Err(reason);
                    }
                    if let Some(arguments) = patch.arguments {
                        current.arguments = arguments;
                    }
                }
                Err(err) => {
                    tracing::warn!(extension = handle.name(), %err, "tool_call hook failed; skipped");
                }
            }
        }
        Ok(current)
    }

    /// Composable `tool_result` mutation (gh #45): every enabled
    /// `hooks-tool-result` extension runs in order over the same
    /// result; omitted fields stay. Errors skip (FR-EXT-3).
    pub async fn compose_tool_result(
        &self,
        call: &ToolCall,
        result: lca_protocol::ToolResult,
    ) -> lca_protocol::ToolResult {
        let mut current = result;
        for handle in self
            .enabled()
            .filter(|h| h.worlds().contains(&World::HooksToolResult))
        {
            match handle.on_tool_result(call, &current).await {
                Ok(patch) => {
                    if let Some(content) = patch.content {
                        current.content = content;
                    }
                    if let Some(is_error) = patch.is_error {
                        current.status = if is_error {
                            lca_protocol::ToolResultStatus::Error
                        } else {
                            lca_protocol::ToolResultStatus::Ok
                        };
                    }
                }
                Err(err) => {
                    tracing::warn!(extension = handle.name(), %err, "tool_result hook failed; skipped");
                }
            }
        }
        current
    }

    /// `message_end` replacement text (gh #45): every enabled
    /// `hooks-message` extension sees the current text; the last
    /// replacement wins. `None` observes. Errors skip (FR-EXT-3).
    pub async fn message_end_replacement(&self, role: &str, text: &str) -> Option<String> {
        let mut current = text.to_string();
        let mut replaced = false;
        for handle in self
            .enabled()
            .filter(|h| h.worlds().contains(&World::HooksMessage))
        {
            match handle.on_message_end(role, &current).await {
                Ok(Some(replacement)) => {
                    current = replacement;
                    replaced = true;
                }
                Ok(None) => {}
                Err(err) => {
                    tracing::warn!(extension = handle.name(), %err, "message_end hook failed; skipped");
                }
            }
        }
        replaced.then_some(current)
    }

    /// Fold one handler's append into the composed decision (gh #45):
    /// non-empty appends concatenate with a blank line.
    pub(crate) fn merge_append(
        decision: &mut lca_protocol::SettleDecision,
        append: Option<String>,
    ) {
        if let Some(append) = append.filter(|text| !text.is_empty()) {
            match &mut decision.append {
                Some(existing) => {
                    existing.push_str("\n\n");
                    existing.push_str(&append);
                }
                None => decision.append = Some(append),
            }
        }
    }

    /// Actionable settle (gh #45): consult one phase (`turn_end`,
    /// then `agent_before_settle`). Appends concatenate; any
    /// `continue-once` continues. Errors settle (FR-EXT-3).
    pub async fn settle_phase(
        &self,
        before_settle: bool,
        rounds: u32,
        tool_calls: u32,
        status: &str,
    ) -> lca_protocol::SettleDecision {
        let mut decision = lca_protocol::SettleDecision::default();
        for handle in self
            .enabled()
            .filter(|h| h.worlds().contains(&World::HooksSettle))
        {
            let outcome = if before_settle {
                handle
                    .on_agent_before_settle(rounds, tool_calls, status)
                    .await
            } else {
                handle.on_turn_end(rounds, tool_calls, status).await
            };
            match outcome {
                Ok(patch) => {
                    Self::merge_append(&mut decision, patch.append);
                    decision.continue_once |= patch.continue_once;
                }
                Err(err) => {
                    tracing::warn!(extension = handle.name(), %err, "settle hook failed; settling");
                }
            }
        }
        decision
    }

    /// `session_before_compact` veto (gh #45): the first deny
    /// cancels the compaction with its reason. Errors allow
    /// (FR-EXT-3: a broken watcher must not freeze the session).
    pub async fn session_before_compact(&self, reason: &str) -> Result<(), String> {
        for handle in self
            .enabled()
            .filter(|h| h.worlds().contains(&World::HooksCompaction))
        {
            match handle.on_session_before_compact(reason).await {
                Ok(lca_protocol::CompactVerdict::Allow) => {}
                Ok(lca_protocol::CompactVerdict::Deny(reason)) => return Err(reason),
                Err(err) => {
                    tracing::warn!(extension = handle.name(), %err, "before-compact hook failed; allowing");
                }
            }
        }
        Ok(())
    }

    /// `session_compact_failed` observation (gh #45).
    pub async fn session_compact_failed(&self, reason: &str, error: Option<&str>) {
        for handle in self
            .enabled()
            .filter(|h| h.worlds().contains(&World::HooksCompaction))
        {
            let _ = handle.on_session_compact_failed(reason, error).await;
        }
    }

    /// `cache_warming_decision` votes (gh #45): every enabled
    /// `hooks-cache` extension votes; any `false` skips warming.
    /// Errors warm (FR-EXT-3).
    pub async fn cache_warm(&self, provider: &str, model: &str) -> bool {
        for handle in self
            .enabled()
            .filter(|h| h.worlds().contains(&World::HooksCache))
        {
            match handle.on_cache_warming_decision(provider, model).await {
                Ok(true) => {}
                Ok(false) => return false,
                Err(err) => {
                    tracing::warn!(extension = handle.name(), %err, "cache hook failed; warming");
                }
            }
        }
        true
    }

    /// `project_trust` votes (gh #45): the first yes/no decides;
    /// undecided falls through. Returns the vote and whether to
    /// remember it. Errors are undecided (FR-EXT-3).
    pub async fn project_trust(&self, cwd: &str) -> (lca_protocol::TrustVote, bool) {
        for handle in self
            .enabled()
            .filter(|h| h.worlds().contains(&World::HooksTrust))
        {
            match handle.on_project_trust(cwd).await {
                Ok((lca_protocol::TrustVote::Undecided, _)) => {}
                Ok((vote, remember)) => return (vote, remember),
                Err(err) => {
                    tracing::warn!(extension = handle.name(), %err, "trust hook failed; undecided");
                }
            }
        }
        (lca_protocol::TrustVote::Undecided, false)
    }

    /// `provider_stream_event` observation (gh #45): normalized
    /// events, in order, after the stream closes. Errors skip.
    pub async fn observe_stream_event(&self, provider: &str, model: &str, kind: &str, data: &str) {
        for handle in self
            .enabled()
            .filter(|h| h.worlds().contains(&World::HooksStream))
        {
            let _ = handle.on_stream_event(provider, model, kind, data).await;
        }
    }

    /// `session-close`, called when a session ends.
    pub async fn on_session_close(&self) {
        for handle in self
            .enabled()
            .filter(|h| h.worlds().contains(&World::Hooks))
        {
            let _ = handle.on_session_close().await;
        }
    }
}
