//! Split from `lib.rs` (cycle 7, P3). Behaviour unchanged.

use super::*;

impl lca_ext_abi::host::tool::lca::host::log::Host for HostState {
    fn trace(&mut self, message: String) {
        self.record_log(message);
    }
    fn debug(&mut self, message: String) {
        self.record_log(message);
    }
    fn info(&mut self, message: String) {
        self.record_log(message);
    }
    fn warn(&mut self, message: String) {
        self.record_log(message);
    }
    fn error(&mut self, message: String) {
        self.record_log(message);
    }
}

impl lca_ext_abi::host::tool::lca::host::resources::Host for HostState {
    fn list_resources(
        &mut self,
        prefix: String,
    ) -> Result<
        Vec<lca_ext_abi::host::tool::lca::host::resources::ResourceEntry>,
        lca_ext_abi::host::tool::lca::host::resources::Error,
    > {
        self.cap
            .resource_list(&prefix)
            .map(|entries| {
                entries
                    .into_iter()
                    .map(|(path, size)| {
                        lca_ext_abi::host::tool::lca::host::resources::ResourceEntry { path, size }
                    })
                    .collect()
            })
            .map_err(resources_error)
    }

    fn read(
        &mut self,
        path: String,
    ) -> Result<Vec<u8>, lca_ext_abi::host::tool::lca::host::resources::Error> {
        self.cap.resource_read(&path).map_err(resources_error)
    }
}

fn resources_error(err: CapabilityError) -> lca_ext_abi::host::tool::lca::host::resources::Error {
    use lca_ext_abi::host::tool::lca::host::resources::Error as E;
    match err {
        CapabilityError::Permission(detail) | CapabilityError::NotGranted(detail) => {
            E::Permission(detail)
        }
        CapabilityError::NotFound(detail) => E::NotFound(detail),
        CapabilityError::Io(detail)
        | CapabilityError::Invalid(detail)
        | CapabilityError::Timeout(detail) => E::Invalid(detail),
    }
}

impl lca_ext_abi::host::tool::lca::host::state::Host for HostState {
    fn read(&mut self, key: String) -> Option<Vec<u8>> {
        self.cap.state_read(&key).unwrap_or(None)
    }

    fn write(
        &mut self,
        key: String,
        value: Vec<u8>,
    ) -> Result<(), lca_ext_abi::host::tool::lca::host::state::Error> {
        self.cap.state_write(&key, &value).map_err(state_error)
    }

    fn delete(
        &mut self,
        key: String,
    ) -> Result<(), lca_ext_abi::host::tool::lca::host::state::Error> {
        self.cap.state_delete(&key).map_err(state_error)
    }

    fn list_keys(
        &mut self,
    ) -> Result<
        Vec<lca_ext_abi::host::tool::lca::host::state::StateEntry>,
        lca_ext_abi::host::tool::lca::host::state::Error,
    > {
        self.cap
            .state_list()
            .map(|entries| {
                entries
                    .into_iter()
                    .map(
                        |(key, size)| lca_ext_abi::host::tool::lca::host::state::StateEntry {
                            key,
                            size,
                        },
                    )
                    .collect()
            })
            .map_err(state_error)
    }
}

fn state_error(err: CapabilityError) -> lca_ext_abi::host::tool::lca::host::state::Error {
    use lca_ext_abi::host::tool::lca::host::state::Error as E;
    match err {
        CapabilityError::Invalid(detail) => E::Invalid(detail),
        CapabilityError::Permission(detail) | CapabilityError::NotGranted(detail) => {
            E::Permission(detail)
        }
        CapabilityError::NotFound(detail)
        | CapabilityError::Io(detail)
        | CapabilityError::Timeout(detail) => E::Io(detail),
    }
}

impl lca_ext_abi::host::tool::lca::ext::types::Host for HostState {}

impl HostState {
    fn record_log(&mut self, message: String) {
        let truncated = truncate_bytes(&message, self.log_limit);
        lock(&self.logs).push(truncated);
    }
}

fn truncate_bytes(message: &str, limit: usize) -> String {
    const MARKER: &str = "... [truncated]";
    if message.len() <= limit {
        return message.to_string();
    }
    let keep = limit.saturating_sub(MARKER.len());
    let mut cut = keep.min(message.len());
    while cut > 0 && !message.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}{}", &message[..cut], MARKER)
}

use lca_ext_abi::host::tool::lca::host::fs::Error as FsError;
use lca_ext_abi::host::tool::lca::host::fs::FileInfo;
use lca_ext_abi::host::tool::lca::host::process::Error as ProcessError;
use lca_ext_abi::host::tool::lca::host::pty::Error as PtyError;

fn fs_error(err: CapabilityError) -> FsError {
    match err {
        CapabilityError::Permission(detail) => FsError::Permission(detail),
        CapabilityError::NotGranted(detail) => FsError::NotGranted(detail),
        CapabilityError::NotFound(detail) => FsError::NotFound(detail),
        CapabilityError::Io(detail) => FsError::Io(detail),
        CapabilityError::Invalid(detail) => FsError::Invalid(detail),
        CapabilityError::Timeout(detail) => FsError::Io(format!("timed out: {detail}")),
    }
}

fn process_error(err: CapabilityError) -> ProcessError {
    match err {
        CapabilityError::Permission(detail) => ProcessError::Permission(detail),
        CapabilityError::NotGranted(detail) => ProcessError::NotGranted(detail),
        CapabilityError::NotFound(detail) => ProcessError::NotFound(detail),
        CapabilityError::Io(detail) => ProcessError::Io(detail),
        CapabilityError::Invalid(detail) => ProcessError::Invalid(detail),
        CapabilityError::Timeout(detail) => ProcessError::Io(format!("timed out: {detail}")),
    }
}

fn pty_error(err: CapabilityError) -> PtyError {
    match err {
        CapabilityError::Permission(detail) => PtyError::Permission(detail),
        CapabilityError::NotGranted(detail) => PtyError::NotGranted(detail),
        CapabilityError::NotFound(detail) => PtyError::NotFound(detail),
        CapabilityError::Io(detail) => PtyError::Io(detail),
        CapabilityError::Invalid(detail) => PtyError::Invalid(detail),
        CapabilityError::Timeout(detail) => PtyError::Io(format!("timed out: {detail}")),
    }
}

// ---------------------------------------------------------------------------
// The provider world: capability imports the same engine serves, plus the
// WIT <-> protocol mapping every provider event and record crosses.
// ---------------------------------------------------------------------------

use lca_ext_abi::host::provider::exports::lca::ext::provider_completion as wit_completion;
use lca_ext_abi::host::provider::exports::lca::ext::provider_identity as wit_identity;
use lca_ext_abi::host::provider::lca::host as provider_host;

fn net_error(err: CapabilityError) -> provider_host::net::Error {
    use provider_host::net::Error as E;
    match err {
        CapabilityError::Permission(detail) => E::Permission(detail),
        CapabilityError::NotGranted(detail) => E::NotGranted(detail),
        CapabilityError::NotFound(detail) => E::Invalid(detail),
        CapabilityError::Io(detail) => E::Io(detail),
        CapabilityError::Invalid(detail) => E::Invalid(detail),
        CapabilityError::Timeout(detail) => E::Io(format!("timed out: {detail}")),
    }
}

fn oauth_error(err: CapabilityError) -> provider_host::oauth::Error {
    use provider_host::oauth::Error as E;
    match err {
        CapabilityError::Permission(detail) => E::Permission(detail),
        CapabilityError::NotGranted(detail) => E::NotGranted(detail),
        CapabilityError::NotFound(_) => E::Invalid("not found".to_string()),
        CapabilityError::Io(detail) => E::Io(detail),
        CapabilityError::Invalid(detail) => E::Invalid(detail),
        CapabilityError::Timeout(detail) => E::Timeout(detail),
    }
}

fn credentials_error(err: CapabilityError) -> provider_host::credentials::Error {
    use provider_host::credentials::Error as E;
    match err {
        CapabilityError::Permission(detail) => E::Permission(detail),
        CapabilityError::NotGranted(detail) => E::NotGranted(detail),
        CapabilityError::NotFound(_) | CapabilityError::Timeout(_) => {
            E::Io("unavailable".to_string())
        }
        CapabilityError::Io(detail) => E::Io(detail),
        CapabilityError::Invalid(detail) => E::Invalid(detail),
    }
}

impl provider_host::log::Host for HostState {
    fn trace(&mut self, message: String) {
        self.record_log(message);
    }
    fn debug(&mut self, message: String) {
        self.record_log(message);
    }
    fn info(&mut self, message: String) {
        self.record_log(message);
    }
    fn warn(&mut self, message: String) {
        self.record_log(message);
    }
    fn error(&mut self, message: String) {
        self.record_log(message);
    }
}

impl lca_ext_abi::host::provider::lca::ext::types::Host for HostState {}

impl provider_host::net::Host for HostState {
    fn request(
        &mut self,
        method: String,
        url: String,
        headers: Vec<(String, String)>,
        body: Option<Vec<u8>>,
    ) -> Result<u32, provider_host::net::Error> {
        let refs: Vec<(&str, &str)> = headers
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect();
        self.cap
            .net_request(&method, &url, &refs, body.as_deref())
            .map_err(net_error)
    }

    fn response_status(&mut self, handle: u32) -> Result<u16, provider_host::net::Error> {
        self.cap.net_response_status(handle).map_err(net_error)
    }

    fn response_headers(
        &mut self,
        handle: u32,
    ) -> Result<Vec<(String, String)>, provider_host::net::Error> {
        self.cap.net_response_headers(handle).map_err(net_error)
    }

    fn read_body(
        &mut self,
        handle: u32,
        max: u64,
    ) -> Result<Option<Vec<u8>>, provider_host::net::Error> {
        self.cap
            .net_read_body(handle, max as usize)
            .map_err(net_error)
    }

    fn close_response(&mut self, handle: u32) -> Result<(), provider_host::net::Error> {
        self.cap.net_close_response(handle).map_err(net_error)
    }
}

impl provider_host::oauth::Host for HostState {
    fn begin(
        &mut self,
        redirect_path: String,
    ) -> Result<(String, u32), provider_host::oauth::Error> {
        self.cap.oauth_begin(&redirect_path).map_err(oauth_error)
    }

    fn open(&mut self, url: String) -> Result<(), provider_host::oauth::Error> {
        self.cap.oauth_open(&url).map_err(oauth_error)
    }

    fn await_callback(
        &mut self,
        handle: u32,
    ) -> Result<Vec<(String, String)>, provider_host::oauth::Error> {
        self.cap.oauth_await(handle).map_err(oauth_error)
    }

    fn end_flow(&mut self, handle: u32) -> Result<(), provider_host::oauth::Error> {
        self.cap.oauth_end(handle).map_err(oauth_error)
    }
}

impl provider_host::credentials::Host for HostState {
    fn get(&mut self, key: String) -> Option<String> {
        // Denial reads as absence (capability catalog): checking for an
        // existing login needs no denial/absence distinction.
        self.cap.credentials_get(&key).unwrap_or(None)
    }

    fn set(&mut self, key: String, value: String) -> Result<(), provider_host::credentials::Error> {
        self.cap
            .credentials_set(&key, &value)
            .map_err(credentials_error)
    }

    fn delete(&mut self, key: String) -> Result<(), provider_host::credentials::Error> {
        self.cap.credentials_delete(&key).map_err(credentials_error)
    }
}

pub(super) fn role_str(role: lca_protocol::MessageRole) -> &'static str {
    use lca_protocol::MessageRole;
    match role {
        MessageRole::System => "system",
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => "tool",
    }
}

/// Protocol request -> the WIT record. Text and image blocks travel as
/// typed content (the ABI 0.2 `content-block` variant); reasoning blocks are
/// model-internal and not resent.
pub(super) fn to_wit_request(request: &CompletionRequest) -> wit_completion::CompletionRequest {
    use wit_completion::{
        CompletionRequest as WitRequest, Message as WitMessage, ToolSpec as WitToolSpec,
    };
    let extra_pairs = |extras: &std::collections::BTreeMap<String, String>| {
        extras
            .iter()
            .map(|(key, value)| wit_completion::ExtraPair {
                key: key.clone(),
                value: value.clone(),
            })
            .collect()
    };
    WitRequest {
        messages: request
            .messages
            .iter()
            .map(|message| WitMessage {
                role: role_str(message.role).to_string(),
                content: message
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        lca_protocol::ContentBlock::Text { text } => Some(
                            lca_ext_abi::host::provider::lca::ext::types::ContentBlock::Text(
                                text.clone(),
                            ),
                        ),
                        lca_protocol::ContentBlock::Image { media_type, bytes } => Some(
                            lca_ext_abi::host::provider::lca::ext::types::ContentBlock::Image((
                                media_type.clone(),
                                bytes.clone(),
                            )),
                        ),
                        // Reasoning is model-internal and tool calls travel in
                        // their own field, so neither is resent as content.
                        lca_protocol::ContentBlock::Reasoning { .. }
                        | lca_protocol::ContentBlock::ToolCall { .. } => None,
                    })
                    .collect(),
                tool_calls: message
                    .tool_calls
                    .iter()
                    .map(
                        |call| lca_ext_abi::host::provider::lca::ext::types::ToolCall {
                            call_id: call.call_id.clone(),
                            name: call.name.clone(),
                            arguments: call.arguments.clone(),
                            extras: Vec::new(),
                        },
                    )
                    .collect(),
                tool_call_id: message.tool_call_id.clone(),
                extras: Vec::new(),
            })
            .collect(),
        tools: request
            .tools
            .iter()
            .map(|tool| WitToolSpec {
                name: tool.name.clone(),
                description: tool.description.clone(),
                parameters: tool.parameters.to_string(),
                extras: extra_pairs(&tool.extras),
            })
            .collect(),
        model: request.model.clone(),
        stable_prefix: request.stable_prefix as u32,
        extras: extra_pairs(&request.extras),
    }
}

/// The WIT stream event -> the protocol event, case per case
/// (ADR-0004: one case each, `vendor-event` the reserved hatch).
pub(super) fn from_wit_event(event: wit_completion::StreamEvent) -> lca_protocol::StreamEvent {
    use lca_protocol::StreamEvent as P;
    match event {
        wit_completion::StreamEvent::TextDelta(delta) => P::TextDelta { delta },
        wit_completion::StreamEvent::ReasoningDelta(delta) => P::ReasoningDelta { delta },
        wit_completion::StreamEvent::ToolCallStart((call_id, name)) => {
            P::ToolCallStart { call_id, name }
        }
        wit_completion::StreamEvent::ToolCallArgDelta((call_id, delta)) => {
            P::ToolCallArgDelta { call_id, delta }
        }
        wit_completion::StreamEvent::ToolCallEnd(call_id) => P::ToolCallEnd { call_id },
        wit_completion::StreamEvent::Usage(usage) => P::Usage {
            usage: from_wit_usage(usage),
        },
        wit_completion::StreamEvent::Error((message, retryable)) => P::Error { message, retryable },
        wit_completion::StreamEvent::VendorEvent((kind, payload)) => P::VendorEvent {
            kind,
            payload: serde_json::from_str(&payload).unwrap_or(serde_json::Value::String(payload)),
        },
    }
}

pub(super) fn f64_extra(extras: &std::collections::BTreeMap<String, String>, key: &str) -> f64 {
    extras
        .get(key)
        .and_then(|value| value.parse().ok())
        .unwrap_or(0.0)
}

/// WIT usage -> protocol usage. The cost buckets (ADR-0017) travel in
/// `extras`, since the WIT record predates the bucket split; the reserved
/// keys stay in `extras` too, where a consumer that does not know them
/// ignores them safely.
pub(super) fn from_wit_usage(usage: wit_completion::Usage) -> Usage {
    let extras: std::collections::BTreeMap<String, String> = usage
        .extras
        .into_iter()
        .map(|pair| (pair.key, pair.value))
        .collect();
    Usage {
        input: usage.input,
        output: usage.output,
        cache_read: usage.cache_read,
        cache_write: usage.cache_write,
        cache_write_1h: usage.cache_write_hour,
        cost: usage.cost,
        cost_input: f64_extra(&extras, "cost_input"),
        cost_cache_read: f64_extra(&extras, "cost_cache_read"),
        cost_cache_write: f64_extra(&extras, "cost_cache_write"),
        extras,
    }
}

pub(super) fn from_wit_identity(outcome: wit_identity::IdentityOutcome) -> IdentityOutcome {
    match outcome {
        wit_identity::IdentityOutcome::Ok => IdentityOutcome::Ok,
        wit_identity::IdentityOutcome::NotSupported => IdentityOutcome::NotSupported,
        wit_identity::IdentityOutcome::Failed(reason) => IdentityOutcome::Failed(reason),
    }
}

impl lca_ext_abi::host::tool::lca::host::fs::Host for HostState {
    fn read(&mut self, scope: String, path: String) -> Result<Vec<u8>, FsError> {
        self.cap.fs_read(&scope, &path).map_err(fs_error)
    }

    fn write(&mut self, scope: String, path: String, bytes: Vec<u8>) -> Result<(), FsError> {
        self.cap.fs_write(&scope, &path, &bytes).map_err(fs_error)
    }

    fn list_entries(&mut self, scope: String, path: String) -> Result<Vec<String>, FsError> {
        self.cap.fs_list(&scope, &path).map_err(fs_error)
    }

    fn stat(&mut self, scope: String, path: String) -> Result<FileInfo, FsError> {
        self.cap
            .fs_stat(&scope, &path)
            .map(|(is_dir, len)| FileInfo { is_dir, len })
            .map_err(fs_error)
    }
}

fn usize_from(max: u64) -> usize {
    usize::try_from(max).unwrap_or(usize::MAX).max(1)
}

impl lca_ext_abi::host::tool::lca::host::process::Host for HostState {
    fn spawn(
        &mut self,
        program: String,
        args: Vec<String>,
        cwd_scope: String,
    ) -> Result<u32, ProcessError> {
        self.cap
            .process_spawn(&program, &args, &cwd_scope)
            .map_err(process_error)
    }

    fn read_stdout(&mut self, handle: u32, max: u64) -> Result<Option<Vec<u8>>, ProcessError> {
        self.cap
            .process_read_stdout(handle, usize_from(max))
            .map_err(process_error)
    }

    fn read_stderr(&mut self, handle: u32, max: u64) -> Result<Option<Vec<u8>>, ProcessError> {
        self.cap
            .process_read_stderr(handle, usize_from(max))
            .map_err(process_error)
    }

    fn write_stdin(&mut self, handle: u32, bytes: Vec<u8>) -> Result<u64, ProcessError> {
        self.cap
            .process_write_stdin(handle, &bytes)
            .map_err(process_error)
    }

    fn wait(&mut self, handle: u32) -> Result<i32, ProcessError> {
        self.cap.process_wait(handle).map_err(process_error)
    }

    fn kill(&mut self, handle: u32) -> Result<(), ProcessError> {
        self.cap.process_kill(handle).map_err(process_error)
    }
}

impl lca_ext_abi::host::tool::lca::host::pty::Host for HostState {
    fn spawn(
        &mut self,
        program: String,
        args: Vec<String>,
        cwd_scope: String,
        rows: u16,
        cols: u16,
    ) -> Result<u32, PtyError> {
        self.cap
            .pty_spawn(&program, &args, &cwd_scope, rows, cols)
            .map_err(pty_error)
    }

    fn read(&mut self, handle: u32, max: u64) -> Result<Option<Vec<u8>>, PtyError> {
        self.cap
            .pty_read(handle, usize_from(max))
            .map_err(pty_error)
    }

    fn write(&mut self, handle: u32, bytes: Vec<u8>) -> Result<u64, PtyError> {
        self.cap.pty_write(handle, &bytes).map_err(pty_error)
    }

    fn resize(&mut self, handle: u32, rows: u16, cols: u16) -> Result<(), PtyError> {
        self.cap.pty_resize(handle, rows, cols).map_err(pty_error)
    }

    fn wait(&mut self, handle: u32) -> Result<i32, PtyError> {
        self.cap.pty_wait(handle).map_err(pty_error)
    }

    fn kill(&mut self, handle: u32) -> Result<(), PtyError> {
        self.cap.pty_kill(handle).map_err(pty_error)
    }
}

// ---------------------------------------------------------------------------
// The `completion` capability: the host's side of ADR-0008's star
// ---------------------------------------------------------------------------

use lca_ext_abi::host::compaction::lca::host::completion as wit_cap_completion;

fn completion_error(err: CapabilityError) -> wit_cap_completion::Error {
    use wit_cap_completion::Error as E;
    match err {
        CapabilityError::Permission(detail) => E::Permission(detail),
        CapabilityError::NotGranted(detail) => E::NotGranted(detail),
        CapabilityError::NotFound(_) | CapabilityError::Timeout(_) => E::Io("unavailable".into()),
        CapabilityError::Io(detail) => E::Io(detail),
        CapabilityError::Invalid(detail) => E::Invalid(detail),
    }
}

/// `lca:host/types.message` -> the protocol message (the host package
/// keeps its own copies of the records; this is the crossing).
fn from_host_message(
    message: lca_ext_abi::host::compaction::lca::host::types::Message,
) -> lca_protocol::ChatMessage {
    use lca_protocol::{ChatMessage, ContentBlock, MessageRole};
    ChatMessage {
        role: match message.role.as_str() {
            "system" => MessageRole::System,
            "user" => MessageRole::User,
            "assistant" => MessageRole::Assistant,
            _ => MessageRole::Tool,
        },
        content: if message.content.is_empty() {
            Vec::new()
        } else {
            vec![ContentBlock::Text {
                text: message.content,
            }]
        },
        tool_calls: message
            .tool_calls
            .iter()
            .map(|call| ToolCall {
                call_id: call.call_id.clone(),
                name: call.name.clone(),
                arguments: call.arguments.clone(),
                parent_call_id: None,
            })
            .collect(),
        tool_call_id: message.tool_call_id,
        usage: None,
        extras: Default::default(),
    }
}

/// The protocol reply -> `lca:host/completion.response`.
fn to_host_response(text: String, usage: lca_protocol::Usage) -> wit_cap_completion::Response {
    let mut extras = usage
        .extras
        .iter()
        .map(
            |(key, value)| lca_ext_abi::host::compaction::lca::host::types::ExtraPair {
                key: key.clone(),
                value: value.clone(),
            },
        )
        .collect::<Vec<_>>();
    for (key, value) in [
        ("cost_input", usage.cost_input),
        ("cost_cache_read", usage.cost_cache_read),
        ("cost_cache_write", usage.cost_cache_write),
    ] {
        if value != 0.0 {
            extras.push(lca_ext_abi::host::compaction::lca::host::types::ExtraPair {
                key: key.to_string(),
                value: value.to_string(),
            });
        }
    }
    use lca_ext_abi::host::compaction::lca::host::types::Usage as HostUsage;
    wit_cap_completion::Response {
        text,
        usage: HostUsage {
            input: usage.input,
            output: usage.output,
            cache_read: usage.cache_read,
            cache_write: usage.cache_write,
            cache_write_hour: usage.cache_write_1h,
            cost: usage.cost,
            extras,
        },
        extras: Vec::new(),
    }
}

impl wit_cap_completion::Host for HostState {
    fn request(
        &mut self,
        messages: Vec<lca_ext_abi::host::compaction::lca::host::types::Message>,
    ) -> Result<wit_cap_completion::Response, wit_cap_completion::Error> {
        let messages = messages.into_iter().map(from_host_message).collect();
        let (text, usage) = self.cap.complete(messages).map_err(completion_error)?;
        Ok(to_host_response(text, usage))
    }
}

thread_local! {
    /// Whether this thread is inside loop-thread dispatch (gh #124):
    /// `render`, `interaction`, and command `invoke` all run on the
    /// TUI loop's own thread, so a dialog there would wait for the
    /// loop to answer itself - a deadlock. Worker threads (tools,
    /// hooks) never set it, so their questions reach the user.
    static DIALOG_FORBIDDEN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Run `call` with dialogs forbidden on this thread (gh #124): the
/// loop-thread dispatch entries (`render`, `interaction`, command
/// `invoke`) wrap their guest call in this.
pub fn without_dialogs<R>(call: impl FnOnce() -> R) -> R {
    DIALOG_FORBIDDEN.with(|forbidden| {
        let was = forbidden.get();
        forbidden.set(true);
        let out = call();
        forbidden.set(was);
        out
    })
}

/// The denial text for a dialog asked on the loop thread.
const LOOP_THREAD_DENIAL: &str =
    "cannot ask a question while handling interface input; ask from a tool or hook instead.";

/// Whether this thread is currently inside loop-thread dispatch.
fn dialogs_forbidden() -> bool {
    DIALOG_FORBIDDEN.with(|forbidden| forbidden.get())
}

/// The `ui-dialogs` import (gh #124/gh #172): through the session's
/// live slot, or the denied values with nothing installed (the
/// headless contract). Every world shares the one canonical import
/// name, so the tool world's registration serves all four.
impl lca_ext_abi::host::tool::lca::host::ui_dialogs::Host for HostState {
    fn confirm(&mut self, title: String, message: String) -> Result<bool, String> {
        if dialogs_forbidden() {
            return Err(LOOP_THREAD_DENIAL.to_string());
        }
        Ok(self.dialogs.confirm(&title, &message))
    }
    fn select(&mut self, title: String, options: Vec<String>) -> Result<Option<String>, String> {
        if dialogs_forbidden() {
            return Err(LOOP_THREAD_DENIAL.to_string());
        }
        Ok(self.dialogs.select(&title, &options))
    }
    fn input(
        &mut self,
        label: String,
        placeholder: Option<String>,
    ) -> Result<Option<String>, String> {
        if dialogs_forbidden() {
            return Err(LOOP_THREAD_DENIAL.to_string());
        }
        Ok(self.dialogs.input(&label, placeholder.as_deref()))
    }
    fn notify(&mut self, message: String, level: String) {
        if dialogs_forbidden() {
            return;
        }
        self.dialogs.notify(&message, &level);
    }
}

/// Shared `tools` import logic (gh #77): one tool calling another
/// through the host. Both the `tool` and `tool-catalog` worlds import
/// the identical interface, so the two impls below share these
/// helpers and map one answer onto each world's generated types.
use lca_ext_abi::host::tool::lca::host::tools::{ActiveToolsResult, NestedResult, ToolName};
use lca_ext_abi::host::tool_catalog::lca::host::tools::{
    ActiveToolsResult as CatalogActiveToolsResult, NestedResult as CatalogNestedResult,
    ToolName as CatalogToolName,
};

impl HostState {
    /// The grant gate every `tools` function passes first: without
    /// the manifest grant the import refuses and records (FR-PERM-3),
    /// and without a loaded registry there is nothing to call through.
    fn tools_gate(&self) -> Result<std::sync::Arc<dyn lca_ext_abi::ToolsRegistryView>, String> {
        if let Err(err) = self.cap.check_tools() {
            return Err(err.to_string());
        }
        self.tools_view
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone()
            .ok_or_else(|| "no turn is running to serve tools".to_string())
    }

    fn tools_execute_answer(&mut self, name: &str, arguments: &str) -> (String, bool, String) {
        let fail = |content: String| (String::new(), true, content);
        let view = match self.tools_gate() {
            Ok(view) => view,
            Err(content) => return fail(content),
        };
        let Some(parent) = self.executing_call.clone() else {
            return fail("no tool call is executing on this store".to_string());
        };
        let Some(slot) = view.nested_slot() else {
            return fail("no turn is running to serve the nested call".to_string());
        };
        let (reply_tx, reply_rx) = std::sync::mpsc::channel();
        let request = lca_ext_abi::NestedCall {
            parent_call_id: parent,
            name: name.to_string(),
            arguments: arguments.to_string(),
            reply: reply_tx,
        };
        // The turn serves on its own task while this thread waits; a
        // dead turn drops the reply, which reads as a failure, never
        // a hang of the host itself (the guest still burns fuel).
        if slot.send(request).is_err() {
            return fail("the turn is gone; the nested call was dropped".to_string());
        }
        match reply_rx.recv() {
            Ok(result) => (
                result.call_id,
                result.status != lca_protocol::ToolResultStatus::Ok,
                result.content,
            ),
            Err(_) => fail("the turn dropped the nested call".to_string()),
        }
    }
}

impl lca_ext_abi::host::tool::lca::host::tools::Host for HostState {
    fn execute_tool(&mut self, name: String, arguments: String) -> NestedResult {
        let (call_id, is_error, content) = self.tools_execute_answer(&name, &arguments);
        NestedResult {
            call_id,
            is_error,
            content,
        }
    }
    fn list_tools(&mut self) -> Vec<ToolName> {
        self.tools_gate()
            .map(|view| view.callable_names())
            .unwrap_or_default()
            .into_iter()
            .map(|(name, description)| ToolName { name, description })
            .collect()
    }
    fn set_active_tools(&mut self, names: Vec<String>) -> ActiveToolsResult {
        let (applied, ignored) = self
            .tools_gate()
            .map(|view| view.set_active_tools(&names))
            .unwrap_or_default();
        ActiveToolsResult { applied, ignored }
    }
    fn get_active_tools(&mut self) -> Vec<String> {
        self.tools_gate()
            .map(|view| view.active_tools())
            .unwrap_or_default()
    }
}

impl lca_ext_abi::host::tool_catalog::lca::host::tools::Host for HostState {
    fn execute_tool(&mut self, name: String, arguments: String) -> CatalogNestedResult {
        let (call_id, is_error, content) = self.tools_execute_answer(&name, &arguments);
        CatalogNestedResult {
            call_id,
            is_error,
            content,
        }
    }
    fn list_tools(&mut self) -> Vec<CatalogToolName> {
        self.tools_gate()
            .map(|view| view.callable_names())
            .unwrap_or_default()
            .into_iter()
            .map(|(name, description)| CatalogToolName { name, description })
            .collect()
    }
    fn set_active_tools(&mut self, names: Vec<String>) -> CatalogActiveToolsResult {
        let (applied, ignored) = self
            .tools_gate()
            .map(|view| view.set_active_tools(&names))
            .unwrap_or_default();
        CatalogActiveToolsResult { applied, ignored }
    }
    fn get_active_tools(&mut self) -> Vec<String> {
        self.tools_gate()
            .map(|view| view.active_tools())
            .unwrap_or_default()
    }
}
