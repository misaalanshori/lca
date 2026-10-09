//! Local llama-server router models (gh #62): chat completions on
//! the shared OpenAI wire kit against an external server (pi runs no
//! child process either — `packages/coding-agent/src/extensions/llama/`
//! connects over HTTP), plus one-shot `/llama` management commands
//! (list, load, unload). No subscription, no OAuth: the server is
//! yours, on loopback, behind `net-local`. The interactive browser,
//! download progress, and classifier fallbacks in pi's UI stay out —
//! the thin command covers what the acceptance names.

use lca_protocol::{CompletionRequest, LoginAnswer, LoginOption, ProviderCap, StreamEvent, Usage};
use lca_subscription::IdentityFailure;
use lca_wire_openai::StreamFailure;

/// pi's `DEFAULT_LLAMA_SERVER_URL`.
pub const SERVER_URL_DEFAULT: &str = "http://127.0.0.1:8080";
/// The local login choice.
pub const CHOICE_LOCAL: &str = "local";

/// The grants the manifest declares (loopback only, plus this
/// provider's own credential namespace for the stored endpoint).
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::expect_used)] // the extension's own literal manifest patterns are valid by construction.
pub fn manifest_grants() -> lca_tools::CapabilityGrants {
    lca_tools::CapabilityGrants {
        net_local: vec![
            lca_permissions::parse_local_pattern("localhost").expect("localhost parses"),
            lca_permissions::parse_local_pattern("127.0.0.1").expect("127.0.0.1 parses"),
        ],
        credentials: true,
        ..Default::default()
    }
}

pub(crate) fn stored(cap: &dyn ProviderCap, key: &str) -> String {
    cap.credentials_get(key).unwrap_or_default()
}

/// The manifest next to this source, so tests pin the file the
/// installer reads to the grants the native form carries.
pub const MANIFEST: &str = include_str!("../extension.toml");

/// The server URL: the stored endpoint, the native `LLAMA_BASE_URL`,
/// else the loopback default — normalized like pi (no trailing
/// slash, no `/v1` tail; the inference path adds its own).
pub fn server_url(cap: &dyn ProviderCap) -> String {
    let configured = stored(cap, "base_url");
    let configured = if configured.trim().is_empty() {
        #[cfg(not(target_arch = "wasm32"))]
        {
            std::env::var("LLAMA_BASE_URL").unwrap_or_default()
        }
        #[cfg(target_arch = "wasm32")]
        {
            String::new()
        }
    } else {
        configured
    };
    normalize_server_url(if configured.trim().is_empty() {
        SERVER_URL_DEFAULT
    } else {
        configured.trim()
    })
}

/// pi's `normalizeLlamaServerUrl`, minus the scheme check (the grant
/// is loopback-only either way).
fn normalize_server_url(url: &str) -> String {
    let url = url.trim_end_matches('/');
    let url = url.strip_suffix("/v1").unwrap_or(url);
    if url.is_empty() {
        SERVER_URL_DEFAULT.to_string()
    } else {
        url.to_string()
    }
}

/// The optional bearer: stored first, the native `LLAMA_API_KEY`
/// second (pi's client takes one; most local servers want none).
fn api_key(cap: &dyn ProviderCap) -> Option<String> {
    let key = stored(cap, "api_key");
    if !key.trim().is_empty() {
        return Some(key.trim().to_string());
    }
    #[cfg(not(target_arch = "wasm32"))]
    if let Ok(key) = std::env::var("LLAMA_API_KEY")
        && !key.trim().is_empty()
    {
        return Some(key.trim().to_string());
    }
    None
}

/// The login options: one local endpoint, URL plus optional key.
pub fn login_options() -> Vec<LoginOption> {
    vec![LoginOption {
        id: CHOICE_LOCAL.to_string(),
        name: "Local llama-server".to_string(),
        kind: "custom".to_string(),
        host: "127.0.0.1".to_string(),
        fields: vec!["base-url".to_string(), "api-key".to_string()],
        extras: Default::default(),
    }]
}

/// Consume one login answer: remember the endpoint (and the key when
/// given), then probe the server so a typo fails here, not mid-turn.
pub fn login_submit(
    cap: &dyn ProviderCap,
    answer: &LoginAnswer,
) -> Result<Vec<(String, String)>, String> {
    if answer.choice != CHOICE_LOCAL {
        return Err(format!("unknown llama login choice `{}`", answer.choice));
    }
    if let Some(url) = answer
        .value("base-url")
        .map(str::trim)
        .filter(|url| !url.is_empty())
    {
        cap.credentials_set("base_url", normalize_server_url(url).as_str())
            .map_err(|err| format!("cannot store the endpoint: {err}"))?;
    }
    if let Some(key) = answer
        .value("api-key")
        .map(str::trim)
        .filter(|key| !key.is_empty())
    {
        cap.credentials_set("api_key", key)
            .map_err(|err| format!("cannot store the key: {err}"))?;
    }
    probe_server(cap).map_err(|err| err.0)?;
    Ok(Vec::new())
}

/// `login`: probe the configured server (headless runs here; the
/// picker route stores first, then probes the same way).
pub fn run_login(cap: &dyn ProviderCap) -> Result<lca_protocol::IdentityOutcome, IdentityFailure> {
    probe_server(cap).map(|()| lca_protocol::IdentityOutcome::Ok)
}

/// `logout`: forget the stored endpoint (the server itself keeps
/// running — this extension never supervises it).
pub fn run_logout(cap: &dyn ProviderCap) -> lca_protocol::IdentityOutcome {
    for key in ["base_url", "api_key"] {
        let _ = cap.credentials_delete(key);
    }
    lca_protocol::IdentityOutcome::Ok
}

/// One bounded GET returning the parsed body (management and probe
/// share it).
fn get_json(
    cap: &dyn ProviderCap,
    path: &str,
) -> Result<(u16, serde_json::Value), IdentityFailure> {
    let url = format!("{}{path}", server_url(cap).trim_end_matches('/'));
    let mut headers: Vec<(&str, &str)> = vec![("accept", "application/json")];
    let bearer;
    if let Some(key) = api_key(cap) {
        bearer = format!("Bearer {key}");
        headers.push(("authorization", bearer.as_str()));
    }
    let handle = cap
        .net_request("GET", &url, &headers, None)
        .map_err(|err| IdentityFailure(err.to_string()))?;
    let status = cap
        .net_response_status(handle)
        .map_err(|err| IdentityFailure(err.to_string()))?;
    let mut body = Vec::new();
    for _ in 0..100 {
        match cap.net_read_body(handle, 64 * 1024) {
            Ok(Some(chunk)) => {
                body.extend_from_slice(&chunk);
                if body.len() > 1024 * 1024 {
                    break;
                }
            }
            _ => break,
        }
    }
    let _ = cap.net_close_response(handle);
    let json: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&body)).unwrap_or_default();
    Ok((status, json))
}

/// The reachability probe: the catalog answers on a live router.
fn probe_server(cap: &dyn ProviderCap) -> Result<(), IdentityFailure> {
    match get_json(cap, "/models") {
        Ok((status, _)) if (200..300).contains(&status) => Ok(()),
        _ => Err(IdentityFailure(format!(
            "no llama-server answers at {}; start it first (llama-server --port 8080)",
            server_url(cap)
        ))),
    }
}

/// `usage`: the reachability probe — a live server is `Ok` with empty
/// counts (local inference has no quota to report).
pub fn run_usage(cap: &dyn ProviderCap) -> Result<Usage, IdentityFailure> {
    probe_server(cap)?;
    Ok(Usage {
        input: 0,
        output: 0,
        cache_read: 0,
        cache_write: 0,
        cache_write_1h: 0,
        cost: 0.0,
        cost_input: 0.0,
        cost_cache_read: 0.0,
        cost_cache_write: 0.0,
        extras: Default::default(),
    })
}

/// One router catalog entry: the id, the load state, the window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlamaModel {
    /// The router's model id.
    pub id: String,
    /// `loaded`, `loading`, `unloaded`, `sleeping`, or the raw word.
    pub status: String,
    /// Context window in tokens; `0` when the server publishes none.
    pub context_window: u32,
}

/// Parse one `/models` answer (router mode: `data` rows with `id`,
/// `status.value`, and `meta` windows; pi's precedence is runtime
/// `n_ctx`, then the launch `--ctx-size`, then trained `n_ctx_train`).
fn parse_catalog(json: &serde_json::Value) -> Vec<LlamaModel> {
    let Some(data) = json.get("data").and_then(|value| value.as_array()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in data {
        let Some(id) = entry.get("id").and_then(|value| value.as_str()) else {
            continue;
        };
        if id.is_empty() {
            continue;
        }
        // Decision-only models cannot generate text (pi's filter, read
        // off `architecture.output_modalities`).
        let modalities = entry
            .get("architecture")
            .and_then(|arch| arch.get("output_modalities"))
            .and_then(|modalities| modalities.as_array());
        if let Some(modalities) = modalities {
            let texts = modalities.iter().filter_map(|value| value.as_str());
            let mut has_text = false;
            let mut has_decisions = false;
            for modality in texts {
                has_text |= modality == "text";
                has_decisions |= modality == "decisions";
            }
            if has_decisions && !has_text {
                continue;
            }
        }
        let status = entry
            .get("status")
            .and_then(|status| status.get("value"))
            .and_then(|value| value.as_str())
            .unwrap_or("unknown")
            .to_string();
        let meta = entry.get("meta");
        let runtime = meta
            .and_then(|meta| meta.get("n_ctx"))
            .and_then(|value| value.as_u64())
            .unwrap_or(0);
        let trained = meta
            .and_then(|meta| meta.get("n_ctx_train"))
            .and_then(|value| value.as_u64())
            .unwrap_or(0);
        let window = runtime.max(configured_window(entry)).max(trained);
        out.push(LlamaModel {
            id: id.to_string(),
            status,
            context_window: u32::try_from(window).unwrap_or(0),
        });
    }
    out
}

/// The launch `--ctx-size`/`-c` from the status args (pi's
/// `configuredContextWindow`).
fn configured_window(entry: &serde_json::Value) -> u64 {
    let Some(args) = entry
        .get("status")
        .and_then(|status| status.get("args"))
        .and_then(|args| args.as_array())
    else {
        return 0;
    };
    let flags = ["--ctx-size", "-c", "-ctx"];
    let mut window = 0;
    for pair in args.windows(2) {
        let flag = pair[0].as_str().unwrap_or("");
        // The server serves args as strings; tolerate numbers too.
        let size = pair[1]
            .as_u64()
            .or_else(|| pair[1].as_str().and_then(|text| text.parse::<u64>().ok()));
        if flags.contains(&flag)
            && let Some(size) = size
        {
            window = size;
        }
    }
    window
}

/// The catalog, empty when the server is down (the command names the
/// way; the picker shows what is there).
fn catalog(cap: &dyn ProviderCap) -> Vec<LlamaModel> {
    match get_json(cap, "/models") {
        Ok((status, json)) if (200..300).contains(&status) => parse_catalog(&json),
        _ => Vec::new(),
    }
}

/// The model catalog: live rows when the server answers, nothing
/// otherwise (there is no curated floor for your own GGUFs).
pub fn list_models(cap: &dyn ProviderCap) -> Vec<lca_protocol::ModelInfo> {
    catalog(cap)
        .into_iter()
        .map(|model| lca_protocol::ModelInfo {
            id: model.id.clone(),
            name: format!("{} ({})", model.id, model.status),
            context_window: model.context_window,
            max_tokens: 0,
            extras: Default::default(),
        })
        .collect()
}

/// The `/llama` command spec: one command, subcommands in the hint.
pub fn command_spec() -> lca_protocol::CommandSpec {
    lca_protocol::CommandSpec {
        name: "llama".to_string(),
        hint: "list | load <model> | unload <model>".to_string(),
        completion: "none".to_string(),
        extras: Default::default(),
    }
}

/// Run one `/llama` invocation over capabilities, answering text (the
/// native side; the sandboxed command world has no `net` import, so
/// its guest declines — see the provider doc).
pub fn invoke_command(cap: &dyn ProviderCap, argument: &str) -> lca_protocol::CommandEffect {
    use lca_protocol::CommandEffect as Effect;
    let mut words = argument.split_whitespace();
    match words.next().unwrap_or("list") {
        "list" => {
            let models = catalog(cap);
            if models.is_empty() {
                return Effect::ShowWidget(format!(
                    "no llama-server answers at {}; start it first (llama-server --port 8080)",
                    server_url(cap)
                ));
            }
            let mut rows = vec![format!("models on {}:", server_url(cap))];
            for model in models {
                let window = if model.context_window > 0 {
                    format!(" ({}k ctx)", model.context_window / 1024)
                } else {
                    String::new()
                };
                rows.push(format!("- {} [{}]{window}", model.id, model.status));
            }
            Effect::ShowWidget(rows.join("\n"))
        }
        "load" => {
            let Some(id) = words.next() else {
                return Effect::ShowWidget("usage: /llama load <model>".to_string());
            };
            match manage_model(cap, "/models/load", id) {
                Ok(()) => Effect::ShowWidget(format!("loading {id} (watch /llama list for it)")),
                Err(reason) => Effect::ShowWidget(reason),
            }
        }
        "unload" => {
            let Some(id) = words.next() else {
                return Effect::ShowWidget("usage: /llama unload <model>".to_string());
            };
            match manage_model(cap, "/models/unload", id) {
                Ok(()) => Effect::ShowWidget(format!("unloaded {id}")),
                Err(reason) => Effect::ShowWidget(reason),
            }
        }
        other => Effect::ShowWidget(format!(
            "usage: /llama list | load <model> | unload <model> (not `{other}`)"
        )),
    }
}

/// One management POST (`/models/load`, `/models/unload`).
fn manage_model(cap: &dyn ProviderCap, path: &str, model: &str) -> Result<(), String> {
    let url = format!("{}{path}", server_url(cap).trim_end_matches('/'));
    let body = serde_json::json!({ "model": model });
    let bytes = serde_json::to_vec(&body).map_err(|err| format!("cannot build request: {err}"))?;
    let mut headers: Vec<(&str, &str)> = vec![("content-type", "application/json")];
    let bearer;
    if let Some(key) = api_key(cap) {
        bearer = format!("Bearer {key}");
        headers.push(("authorization", bearer.as_str()));
    }
    let handle = cap
        .net_request("POST", &url, &headers, Some(&bytes))
        .map_err(|err| err.to_string())?;
    let status = cap
        .net_response_status(handle)
        .map_err(|err| err.to_string())?;
    let _ = cap.net_close_response(handle);
    if (200..300).contains(&status) {
        Ok(())
    } else {
        Err(format!("llama-server returned HTTP {status} for {model}"))
    }
}

/// The wire parts of one call (URL, headers, body): the native driver
/// and the WASM guest share it, so the two deliveries send
/// byte-identical requests.
pub type RequestParts = (String, Vec<(String, String)>, Vec<u8>);

/// Build one chat call over `{base}/v1/chat/completions` (pi's
/// `llamaInferenceUrl`), with the bearer when configured.
pub fn build_request(
    cap: &dyn ProviderCap,
    request: &CompletionRequest,
) -> Result<RequestParts, StreamFailure> {
    let model = if request.model.is_empty() {
        return Err(StreamFailure {
            message: "this server has no model selected; run /model to pick one".to_string(),
            class: "invalid",
            retryable: false,
        });
    } else {
        request.model.clone()
    };
    let mut body = serde_json::json!({
        "model": model,
        "messages": lca_wire_openai::to_wire(&request.messages),
        "stream": true,
        "stream_options": { "include_usage": true },
    });
    if let Some(budget) = request
        .extras
        .get("max-tokens")
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|budget| *budget > 0)
    {
        body["max_tokens"] = serde_json::json!(budget);
    }
    let tools = lca_wire_openai::tools_wire(&request.tools);
    if !tools.is_empty() {
        body["tools"] = serde_json::Value::Array(tools);
    }
    let body_bytes = serde_json::to_vec(&body).map_err(|err| StreamFailure {
        message: format!("cannot build request: {err}"),
        class: "invalid",
        retryable: false,
    })?;
    let url = format!(
        "{}/v1/chat/completions",
        server_url(cap).trim_end_matches('/')
    );
    let mut headers = vec![("content-type".to_string(), "application/json".to_string())];
    if let Some(key) = api_key(cap) {
        headers.push(("authorization".to_string(), format!("Bearer {key}")));
    }
    Ok((url, headers, body_bytes))
}

/// One streamed completion over capabilities. `emit` returning `false`
/// stops the read (FR-CONC-3).
pub fn run_provider_stream(
    cap: &dyn ProviderCap,
    request: &CompletionRequest,
    emit: &mut dyn FnMut(StreamEvent) -> bool,
) -> Result<(), StreamFailure> {
    let (url, headers, body_bytes) = build_request(cap, request)?;
    let refs: Vec<(&str, &str)> = headers
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let mut driver = ChatDriver::open(cap, &url, &refs, &body_bytes)?;
    while let Some(event) = driver.next_event() {
        if !emit(event?) {
            break;
        }
    }
    Ok(())
}

/// The SSE driver over capabilities: the openrouter chunk loop over
/// the shared chat decoder (provenance noted, not forked).
pub struct ChatDriver<'a> {
    cap: &'a dyn ProviderCap,
    handle: u32,
    decoder: lca_wire_openai::SseDecoder,
    pending: std::collections::VecDeque<StreamEvent>,
    finished: bool,
}

impl<'a> ChatDriver<'a> {
    /// Send the request and check the status. A non-2xx response is
    /// read (bounded) and classified by the kit.
    pub fn open(
        cap: &'a dyn ProviderCap,
        url: &str,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> Result<ChatDriver<'a>, StreamFailure> {
        let handle = cap.net_request("POST", url, headers, Some(body))?;
        let status = cap.net_response_status(handle)?;
        if !(200..300).contains(&status) {
            let mut detail = Vec::new();
            while let Some(chunk) = cap.net_read_body(handle, 64 * 1024)? {
                detail.extend_from_slice(&chunk);
                if detail.len() > 1024 * 1024 {
                    break;
                }
            }
            let _ = cap.net_close_response(handle);
            let text = String::from_utf8_lossy(&detail);
            let message = format!(
                "provider returned HTTP {status}: {}",
                lca_wire_openai::json_error_message(&text)
            );
            let classified = lca_wire_openai::failure_for_status(status, &message);
            return Err(StreamFailure {
                message,
                class: classified.class,
                retryable: classified.retryable,
            });
        }
        Ok(ChatDriver {
            cap,
            handle,
            decoder: lca_wire_openai::SseDecoder::default(),
            pending: std::collections::VecDeque::new(),
            finished: false,
        })
    }

    /// The next typed event, reading more of the body when the buffer is
    /// empty; `None` at end of stream.
    pub fn next_event(&mut self) -> Option<Result<StreamEvent, StreamFailure>> {
        loop {
            if let Some(event) = self.pending.pop_front() {
                return Some(Ok(event));
            }
            if self.finished {
                return None;
            }
            match self.cap.net_read_body(self.handle, 64 * 1024) {
                Ok(Some(chunk)) => {
                    let mut events = Vec::new();
                    self.decoder.feed(&chunk, &mut |event| events.push(event));
                    self.pending.extend(events);
                }
                Ok(None) => {
                    self.finished = true;
                    let mut events = Vec::new();
                    self.decoder.finish(&mut |event| events.push(event));
                    self.pending.extend(events);
                }
                Err(err) => {
                    self.finished = true;
                    return Some(Err(StreamFailure::from(err)));
                }
            }
        }
    }
}

impl Drop for ChatDriver<'_> {
    fn drop(&mut self) {
        let _ = self.cap.net_close_response(self.handle);
    }
}

/// The outcome alias identity returns on the success path.
pub type IdentityOutcomeAlias = lca_protocol::IdentityOutcome;

#[cfg(not(target_arch = "wasm32"))]
mod native;

#[cfg(not(target_arch = "wasm32"))]
pub use native::Llama;

#[cfg(target_arch = "wasm32")]
mod wasm_command;

#[cfg(target_arch = "wasm32")]
mod wasm_provider;
