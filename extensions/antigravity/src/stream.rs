//! The Antigravity SSE stream driver, request body envelope, and chunk parser.
//! Split from `lib.rs` to satisfy workspace Gate 11 ceiling.

use crate::{
    DEFAULT_API_BASE, DEFAULT_USER_AGENT, StreamFailure, access_token, endpoint,
    failure_for_status, json_error_message, now_epoch,
};
use lca_protocol::ProviderCap;
use sha1::Digest as _;

/// Pure agy CLI wire alignment (pi-antigravity `src/utils/util.ts` & `src/stream/stream.ts`):
/// Maps public model ID + reasoning effort to backend runtime model ID.
pub fn resolve_runtime_model(model: &str, effort: Option<&str>) -> &'static str {
    match model {
        "gemini-3.8-flash" => match effort {
            Some("medium") => "gemini-3.8-flash-medium",
            Some("high") | Some("xhigh") => "gemini-3.8-flash-high",
            _ => "gemini-3.8-flash-low",
        },
        "gemini-3.7-flash" => match effort {
            Some("medium") => "gemini-3.7-flash-medium",
            Some("high") | Some("xhigh") => "gemini-3.7-flash-high",
            _ => "gemini-3.7-flash-low",
        },
        "gemini-3.6-flash" => match effort {
            Some("medium") => "gemini-3.6-flash-medium",
            Some("high") | Some("xhigh") => "gemini-3.6-flash-high",
            _ => "gemini-3.6-flash-low",
        },
        "gemini-3.5-flash" => match effort {
            Some("low") => "gemini-3.5-flash-low",
            Some("high") | Some("xhigh") => "gemini-3-flash-agent",
            _ => "gemini-3.5-flash-extra-low",
        },
        "gemini-3.1-pro" => match effort {
            Some("high") | Some("xhigh") => "gemini-pro-agent",
            _ => "gemini-3.1-pro-low",
        },
        "claude-opus-4-6" => "claude-opus-4-6-thinking",
        "claude-sonnet-4-6" => "claude-sonnet-4-6",
        "gpt-oss-120b" => "gpt-oss-120b-medium",
        _ => "gemini-3.8-flash-low",
    }
}

/// Deterministic v5-style UUID from a seed (pi-antigravity
/// `src/utils/util.ts:stableUuid`): SHA-1, first 16 bytes, version and
/// variant bits set. Labels must match pi byte-for-byte — a SHA-256
/// truncation would fingerprint differently.
pub fn stable_uuid(seed: &str) -> String {
    let mut bytes: [u8; 16] = sha1::Sha1::digest(seed.as_bytes())[..16]
        .try_into()
        .unwrap_or([0u8; 16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    )
}

/// Port of pi-antigravity `getFallbackRuntimeModel`
/// (`src/models/models.ts`): when a next-gen preview id 404s, the
/// available backend model. A table, not branches: prefix rewrites
/// first, then exact rows, in pi's order (a `gemini-3.8-flash-tiered`
/// request rewrites its prefix, it does not take the bare `gemini-3.8`
/// row). The tiered row follows the thinking effort like pi's
/// `getAntigravityRequestModelId("gemini-3.6-flash", effort)` does.
pub fn fallback_runtime_model(model: &str, effort: Option<&str>) -> Option<String> {
    const PREFIXES: &[(&str, &str)] = &[
        ("gemini-3.8-flash-", "gemini-3.7-flash-"),
        ("gemini-3.7-flash-", "gemini-3.6-flash-"),
    ];
    const EXACT: &[(&str, &str)] = &[
        ("gemini-3.8-flash", "gemini-3.7-flash-low"),
        ("gemini-3.7-flash", "gemini-3.6-flash-low"),
    ];
    // pi's literal order: the 3.8 prefix before the bare 3.8 row, the
    // tiered row before the 3.7 prefix, the 3.7 prefix before bare 3.7.
    if let Some(rest) = model.strip_prefix(PREFIXES[0].0) {
        return Some(format!("{}{rest}", PREFIXES[0].1));
    }
    if model == EXACT[0].0 {
        return Some(EXACT[0].1.to_string());
    }
    if model == "gemini-3.7-flash-tiered" {
        return Some(
            match effort {
                Some("medium") => "gemini-3.6-flash-medium",
                Some("high") | Some("xhigh") => "gemini-3.6-flash-high",
                _ => "gemini-3.6-flash-low",
            }
            .to_string(),
        );
    }
    if let Some(rest) = model.strip_prefix(PREFIXES[1].0) {
        return Some(format!("{}{rest}", PREFIXES[1].1));
    }
    if model == EXACT[1].0 {
        return Some(EXACT[1].1.to_string());
    }
    None
}

/// Matches ANTIGRAVITY_MODEL_ENUM in pi-antigravity `src/models/models.ts`.
pub fn model_enum_for(runtime_model: &str) -> Option<&'static str> {
    match runtime_model {
        "gemini-3.8-flash-high" | "gemini-3.8-flash" => Some("MODEL_PLACEHOLDER_M318"),
        "gemini-3.8-flash-medium" => Some("MODEL_PLACEHOLDER_M319"),
        "gemini-3.8-flash-low" => Some("MODEL_PLACEHOLDER_M320"),
        "gemini-3.8-flash-tiered" => Some("MODEL_PLACEHOLDER_M322"),
        "gemini-3.7-flash-high" | "gemini-3.7-flash" => Some("MODEL_PLACEHOLDER_M298"),
        "gemini-3.7-flash-medium" => Some("MODEL_PLACEHOLDER_M299"),
        "gemini-3.7-flash-low" => Some("MODEL_PLACEHOLDER_M300"),
        "gemini-3.7-flash-tiered" => Some("MODEL_PLACEHOLDER_M301"),
        "gemini-3.6-flash-high" | "gemini-3.6-flash" => Some("MODEL_PLACEHOLDER_M71"),
        "gemini-3.6-flash-medium" => Some("MODEL_PLACEHOLDER_M72"),
        "gemini-3.6-flash-low" => Some("MODEL_PLACEHOLDER_M73"),
        "gemini-3.6-flash-tiered" => Some("MODEL_PLACEHOLDER_M196"),
        "gemini-3.5-flash-low" | "gemini-3.5-flash" => Some("MODEL_PLACEHOLDER_M20"),
        "gemini-3.5-flash-extra-low" => Some("MODEL_PLACEHOLDER_M187"),
        "gemini-3-flash-agent" => Some("MODEL_PLACEHOLDER_M84"),
        "gemini-3.1-pro-low" | "gemini-3.1-pro" => Some("MODEL_PLACEHOLDER_M36"),
        "gemini-3.1-pro-high" => Some("MODEL_PLACEHOLDER_M37"),
        "gemini-pro-agent" => Some("MODEL_PLACEHOLDER_M16"),
        "claude-sonnet-4-6" => Some("MODEL_PLACEHOLDER_M35"),
        "claude-opus-4-6" | "claude-opus-4-6-thinking" => Some("MODEL_PLACEHOLDER_M26"),
        _ => None,
    }
}

/// Keywords pi strips as schema *metadata* before either tool channel
/// (pi-antigravity `stripMetaSchema`'s `META_SCHEMA_KEYWORDS`), plus the
/// map/value/array keyword sets that decide how deep the strip recurses.
const META_SCHEMA_KEYWORDS: &[&str] = &[
    "$schema",
    "$id",
    "$anchor",
    "$dynamicAnchor",
    "$vocabulary",
    "$comment",
    "$defs",
    "definitions",
];
const SCHEMA_MAP_KEYWORDS: &[&str] = &[
    "properties",
    "patternProperties",
    "dependentSchemas",
    "dependencies",
];
const SCHEMA_VALUE_KEYWORDS: &[&str] = &[
    "additionalItems",
    "additionalProperties",
    "contains",
    "contentSchema",
    "else",
    "if",
    "items",
    "not",
    "propertyNames",
    "then",
    "unevaluatedItems",
    "unevaluatedProperties",
];
const SCHEMA_ARRAY_KEYWORDS: &[&str] = &["allOf", "anyOf", "oneOf", "prefixItems"];

/// Remove schema metadata without treating user-defined property names
/// as keywords (pi-antigravity `stripMetaSchema`).
fn strip_meta_schema(schema: &serde_json::Value) -> serde_json::Value {
    fn strip_map(map: &serde_json::Map<String, serde_json::Value>) -> serde_json::Value {
        serde_json::Value::Object(
            map.iter()
                .map(|(key, value)| (key.clone(), strip_meta_schema(value)))
                .collect(),
        )
    }
    match schema {
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(strip_meta_schema).collect())
        }
        serde_json::Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (key, value) in map {
                if META_SCHEMA_KEYWORDS.contains(&key.as_str()) {
                    continue;
                }
                let stripped = if SCHEMA_MAP_KEYWORDS.contains(&key.as_str()) {
                    match value {
                        serde_json::Value::Object(inner) => strip_map(inner),
                        _ => strip_meta_schema(value),
                    }
                } else if SCHEMA_VALUE_KEYWORDS.contains(&key.as_str())
                    || SCHEMA_ARRAY_KEYWORDS.contains(&key.as_str())
                {
                    strip_meta_schema(value)
                } else {
                    value.clone()
                };
                out.insert(key.clone(), stripped);
            }
            serde_json::Value::Object(out)
        }
        _ => schema.clone(),
    }
}

/// A schema with no usable root is an empty object schema
/// (pi-antigravity `ensureRootObjectSchema`).
fn ensure_root_object_schema(schema: serde_json::Value) -> serde_json::Value {
    match schema {
        serde_json::Value::Object(mut map) => {
            if !map.contains_key("type") {
                map.insert(
                    "type".to_string(),
                    serde_json::Value::String("object".to_string()),
                );
                if !map.contains_key("properties") {
                    map.insert(
                        "properties".to_string(),
                        serde_json::Value::Object(serde_json::Map::new()),
                    );
                }
            }
            serde_json::Value::Object(map)
        }
        _ => serde_json::json!({ "type": "object", "properties": {} }),
    }
}

/// The protobuf bridge's accepted fields: anything else — `nullable`,
/// `anyOf`, `format`, `$ref` — 400s as `Unknown name "..."`
/// (pi-antigravity `CUSTOM_TOOL_SCHEMA_ALLOW`). Allowlist, not
/// denylist, so new keywords cannot 400 the request.
const CUSTOM_TOOL_SCHEMA_ALLOW: &[&str] = &[
    "type",
    "description",
    "properties",
    "required",
    "items",
    "enum",
];

/// Union types like `["string", "null"]` keep the first non-null
/// scalar (pi-antigravity `normalizeCustomToolType`).
fn normalize_custom_tool_type(value: &serde_json::Value) -> Option<serde_json::Value> {
    match value {
        serde_json::Value::String(_) => Some(value.clone()),
        serde_json::Value::Array(entries) => entries
            .iter()
            .find(|entry| entry.as_str().is_some_and(|text| text != "null"))
            .cloned(),
        _ => None,
    }
}

/// Filter one schema through the bridge allowlist
/// (pi-antigravity `normalizeCustomToolSchema`). Property names are
/// user-defined and never filtered; a non-string enum drops wholesale.
///
/// `$ref` is NOT resolved here (pi's `dereferenceSchema` has no port:
/// LCA tool schemas are inline, so a reference key drops like any
/// non-allowlisted keyword and the tool keeps an empty schema).
pub fn normalize_custom_tool_schema(schema: &serde_json::Value) -> serde_json::Value {
    match schema {
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(normalize_custom_tool_schema).collect())
        }
        serde_json::Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (key, value) in map {
                if !CUSTOM_TOOL_SCHEMA_ALLOW.contains(&key.as_str()) {
                    continue;
                }
                if key == "type" {
                    if let Some(normalized) = normalize_custom_tool_type(value) {
                        out.insert(key.clone(), normalized);
                    }
                    continue;
                }
                if key == "properties" {
                    if let serde_json::Value::Object(props) = value {
                        let mut kept = serde_json::Map::new();
                        for (name, prop) in props {
                            kept.insert(name.clone(), normalize_custom_tool_schema(prop));
                        }
                        out.insert(key.clone(), serde_json::Value::Object(kept));
                    }
                    continue;
                }
                if key == "enum" {
                    let strings_only = value
                        .as_array()
                        .is_some_and(|entries| entries.iter().all(|entry| entry.is_string()));
                    if !strings_only {
                        continue;
                    }
                }
                out.insert(key.clone(), normalize_custom_tool_schema(value));
            }
            serde_json::Value::Object(out)
        }
        _ => schema.clone(),
    }
}

/// The request body: pure agy CLI wire envelope, following pi's `buildRequest`
/// (pi-antigravity `src/stream/stream.ts:879-980`).
pub fn build_request(
    request: &lca_protocol::CompletionRequest,
    system: &str,
    project_id: &str,
    runtime_model: &str,
) -> serde_json::Value {
    let mut contents = Vec::new();
    let call_names: std::collections::HashMap<&str, &str> = request
        .messages
        .iter()
        .flat_map(|message| message.tool_calls.iter())
        .map(|call| (call.call_id.as_str(), call.name.as_str()))
        .collect();
    for message in &request.messages {
        let role = match message.role {
            lca_protocol::MessageRole::Tool => "user",
            lca_protocol::MessageRole::Assistant => "model",
            _ => "user",
        };
        let mut parts = Vec::new();
        let text: String = message
            .content
            .iter()
            .filter_map(|block| match block {
                lca_protocol::ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        if message.role == lca_protocol::MessageRole::Tool {
            let name = message
                .tool_call_id
                .as_deref()
                .and_then(|id| call_names.get(id).copied())
                .unwrap_or("tool");
            parts.push(serde_json::json!({
                "functionResponse": {
                    "name": name,
                    "response": { "result": text },
                }
            }));
        } else {
            if !text.is_empty() {
                parts.push(serde_json::json!({ "text": text }));
            }
            for block in &message.content {
                if let lca_protocol::ContentBlock::Image { media_type, bytes } = block {
                    parts.push(serde_json::json!({
                        "inlineData": {
                            "mimeType": media_type,
                            "data": lca_protocol::base64_encode(bytes),
                        }
                    }));
                }
            }
            for call in &message.tool_calls {
                let args: serde_json::Value = serde_json::from_str(&call.arguments)
                    .unwrap_or_else(|_| serde_json::json!({ "arguments": call.arguments }));
                parts.push(serde_json::json!({
                    "functionCall": { "id": call.call_id, "name": call.name, "args": args }
                }));
            }
        }
        if parts.is_empty() {
            continue;
        }
        contents.push(serde_json::json!({ "role": role, "parts": parts }));
    }
    // pi-antigravity `convertTools(declaredTools, isClaude ||
    // model.id.startsWith("gpt-oss-"))`: Gemini takes the schema as-is
    // through `parametersJsonSchema`; Claude and GPT-OSS go through the
    // allowlist into the legacy `parameters` field. Both channels strip
    // schema metadata and default a missing root first.
    let use_legacy_parameters = runtime_model.starts_with("claude-")
        || request.model.starts_with("claude-")
        || request.model.starts_with("gpt-oss-");
    let tools = if request.tools.is_empty() {
        serde_json::Value::Null
    } else {
        let declarations: Vec<serde_json::Value> = request
            .tools
            .iter()
            .map(|tool| {
                let schema = strip_meta_schema(&ensure_root_object_schema(tool.parameters.clone()));
                if use_legacy_parameters {
                    serde_json::json!({
                        "name": tool.name,
                        "description": tool.description,
                        "parameters": normalize_custom_tool_schema(&schema),
                    })
                } else {
                    serde_json::json!({
                        "name": tool.name,
                        "description": tool.description,
                        "parametersJsonSchema": schema,
                    })
                }
            })
            .collect();
        serde_json::json!([{ "functionDeclarations": declarations }])
    };

    let step = std::cmp::max(1, contents.len());
    let last_step_index = format!("{}", std::cmp::max(0, contents.len().saturating_sub(1)));
    let request_index = request
        .messages
        .iter()
        .filter(|m| m.role == lca_protocol::MessageRole::Assistant)
        .count();

    let trajectory_id = uuid_v4_like();
    let conv_id = uuid_v4_like();
    let is_claude = runtime_model.starts_with("claude-");
    let is_non_gemini = is_claude || runtime_model.starts_with("gpt-oss-");

    let mut labels = serde_json::json!({
        "last_step_index": last_step_index,
        "request_id": format!("{trajectory_id}-{request_index}"),
        "trajectory_id": trajectory_id,
        "used_claude": if is_claude { "true" } else { "false" },
        "used_claude_conservative": if is_claude { "true" } else { "false" },
        "used_non_gemini_model": if is_non_gemini { "true" } else { "false" },
    });
    if let Some(m_enum) = model_enum_for(runtime_model) {
        labels["model_enum"] = serde_json::Value::String(m_enum.to_string());
    }
    // Pure agy CLI wire alignment (pi-antigravity
    // `antigravityRequestEnvelope`): `last_execution_id` is the previous
    // turn's execution id, present on later steps only, never the first.
    if step > 1 {
        labels["last_execution_id"] = serde_json::Value::String(stable_uuid(&format!(
            "antigravity:exec:{trajectory_id}:{}",
            step - 1
        )));
    }

    let mut inner_request = serde_json::json!({
        "contents": contents,
        "systemInstruction": {
            "role": "user",
            "parts": [{ "text": system }],
        },
        "sessionId": format!("{}", now_epoch() * 1000),
        "labels": labels,
    });
    if !tools.is_null() {
        inner_request["tools"] = tools;
    }

    serde_json::json!({
        "project": project_id,
        "model": runtime_model,
        "request": inner_request,
        "requestType": "agent",
        "userAgent": "antigravity",
        "requestId": format!("agent/{conv_id}/{}/{trajectory_id}/{step}", now_epoch() * 1000),
    })
}

fn uuid_v4_like() -> String {
    let mut bytes = [0u8; 16];
    let _ = getrandom::fill(&mut bytes);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    )
}

/// Decode one SSE frame of `streamGenerateContent` into typed events.
/// A `functionCall` part arrives whole, so it opens the call, carries
/// its arguments, and closes it in order (FR-PROV-7's shape).
fn handle_chunk(
    value: &serde_json::Value,
    open_calls: &mut Vec<String>,
    emit: &mut dyn FnMut(lca_protocol::StreamEvent) -> bool,
) -> bool {
    use lca_protocol::StreamEvent as E;
    if let Some(error) = value.get("error") {
        let message = error
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("vendor error")
            .to_string();
        return emit(E::Error {
            message,
            retryable: false,
        });
    }
    if let Some(candidate) = value
        .get("candidates")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("content"))
        .and_then(|c| c.get("parts"))
        .and_then(|p| p.as_array())
    {
        for part in candidate {
            if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                if part.get("thought").and_then(|t| t.as_bool()) == Some(true) {
                    if !emit(E::ReasoningDelta {
                        delta: text.to_string(),
                    }) {
                        return false;
                    }
                } else if !emit(E::TextDelta {
                    delta: text.to_string(),
                }) {
                    return false;
                }
            }
            if let Some(call) = part.get("functionCall") {
                let name = call
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("tool")
                    .to_string();
                let id = call
                    .get("id")
                    .and_then(|i| i.as_str())
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("ag-{}", open_calls.len()));
                if !emit(E::ToolCallStart {
                    call_id: id.clone(),
                    name,
                }) {
                    return false;
                }
                open_calls.push(id.clone());
                let args = call
                    .get("args")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({}));
                if !emit(E::ToolCallArgDelta {
                    call_id: id.clone(),
                    delta: args.to_string(),
                }) {
                    return false;
                }
                if !emit(E::ToolCallEnd { call_id: id }) {
                    return false;
                }
                open_calls.pop();
            }
        }
    }
    if let Some(usage) = value.get("usageMetadata") {
        let count = |key: &str| usage.get(key).and_then(|v| v.as_u64()).unwrap_or(0);
        let cache_read = count("cachedContentTokenCount");
        let prompt = count("promptTokenCount");
        let output = count("candidatesTokenCount") + count("thoughtsTokenCount");
        if prompt > 0 || output > 0 || cache_read > 0 {
            return emit(E::Usage {
                usage: lca_protocol::Usage {
                    input: prompt.saturating_sub(cache_read),
                    output,
                    cache_read,
                    cache_write: 0,
                    cache_write_1h: 0,
                    cost: 0.0,
                    cost_input: 0.0,
                    cost_cache_read: 0.0,
                    cost_cache_write: 0.0,
                    extras: Default::default(),
                },
            });
        }
    }
    true
}

/// A pull-based driver over one Antigravity streaming completion.
pub struct StreamDriver<'a> {
    cap: &'a dyn ProviderCap,
    handle: u32,
    buffer: String,
    open_calls: Vec<String>,
    pending: std::collections::VecDeque<lca_protocol::StreamEvent>,
    finished: bool,
}

impl<'a> StreamDriver<'a> {
    /// Authenticate, build the request, send it, and check the status.
    pub fn open(
        cap: &'a dyn ProviderCap,
        request: &lca_protocol::CompletionRequest,
    ) -> Result<StreamDriver<'a>, StreamFailure> {
        let token = access_token(cap).map_err(StreamFailure::from)?;
        let api_base = endpoint(cap, "api_base", DEFAULT_API_BASE);
        let system: String = request
            .messages
            .iter()
            .filter(|message| message.role == lca_protocol::MessageRole::System)
            .flat_map(|message| {
                message.content.iter().filter_map(|block| match block {
                    lca_protocol::ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
            })
            .collect::<Vec<_>>()
            .join("\n");
        let project_id = crate::stored(cap, "project");
        let effort = request.extras.get("reasoning-effort").map(String::as_str);
        // pi-antigravity's candidate walk: the requested model first,
        // then its fallback row when one exists. A 404 advances to the
        // next candidate (a preview id the backend does not serve yet);
        // any other status ends the walk with that response.
        let mut candidates = vec![resolve_runtime_model(&request.model, effort).to_string()];
        if let Some(fallback) = fallback_runtime_model(&candidates[0], effort) {
            candidates.push(fallback);
        }
        let url = format!(
            "{}/v1internal:streamGenerateContent?alt=sse",
            api_base.trim_end_matches('/')
        );
        let bearer = format!("Bearer {token}");
        let headers = [
            ("content-type", "application/json"),
            ("authorization", bearer.as_str()),
            ("user-agent", DEFAULT_USER_AGENT),
        ];
        let mut handle = 0;
        let mut status = 0;
        for (index, candidate) in candidates.iter().enumerate() {
            let body = build_request(request, &system, &project_id, candidate);
            let body_bytes = serde_json::to_vec(&body).map_err(|err| StreamFailure {
                message: format!("cannot build request: {err}"),
                class: "invalid",
                retryable: false,
            })?;
            handle = cap.net_request("POST", &url, &headers, Some(&body_bytes))?;
            status = cap.net_response_status(handle)?;
            if status == 404 && index + 1 < candidates.len() {
                let _ = cap.net_close_response(handle);
                continue;
            }
            break;
        }
        if !(200..300).contains(&status) {
            if status == 401 {
                // A rejected token poisons every later call: purge it so
                // the next call re-authenticates instead of looping.
                crate::purge_tokens(cap);
            }
            let mut detail = Vec::new();
            while let Some(chunk) = cap.net_read_body(handle, 64 * 1024)? {
                detail.extend_from_slice(&chunk);
                if detail.len() > 1024 * 1024 {
                    break;
                }
            }
            let _ = cap.net_close_response(handle);
            let text = String::from_utf8_lossy(&detail);
            return Err(failure_for_status(status, &json_error_message(&text)));
        }
        Ok(StreamDriver {
            cap,
            handle,
            buffer: String::new(),
            open_calls: Vec::new(),
            pending: std::collections::VecDeque::new(),
            finished: false,
        })
    }

    /// The next typed event.
    pub fn next_event(&mut self) -> Option<Result<lca_protocol::StreamEvent, StreamFailure>> {
        loop {
            if let Some(event) = self.pending.pop_front() {
                return Some(Ok(event));
            }
            if self.finished {
                return None;
            }
            match self.cap.net_read_body(self.handle, 64 * 1024) {
                Ok(Some(chunk)) => {
                    self.buffer.push_str(&String::from_utf8_lossy(&chunk));
                    self.drain_frames();
                }
                Ok(None) => self.finished = true,
                Err(err) => {
                    self.finished = true;
                    return Some(Err(StreamFailure::from(err)));
                }
            }
        }
    }

    /// Decode every complete `\n\n`-terminated frame currently buffered.
    fn drain_frames(&mut self) {
        loop {
            let Some(end) = self.buffer.find("\n\n") else {
                return;
            };
            let frame: String = self.buffer.drain(..end + 2).collect();
            let mut events = Vec::new();
            for line in frame.lines() {
                let line = line.trim_end_matches('\r');
                let Some(payload) = line.strip_prefix("data:").map(str::trim) else {
                    continue;
                };
                if payload.is_empty() || payload == "[DONE]" {
                    continue;
                }
                let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) else {
                    continue;
                };
                handle_chunk(&value, &mut self.open_calls, &mut |event| {
                    events.push(event);
                    true
                });
            }
            self.pending.extend(events);
        }
    }
}

impl Drop for StreamDriver<'_> {
    fn drop(&mut self) {
        let _ = self.cap.net_close_response(self.handle);
    }
}
