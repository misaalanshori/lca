//! Bounded HTML-to-Markdown URL inspection (gh #222): a read-only
//! URL reader ("smarter `curl`") that fetches through the `net`
//! capability and returns clean, bounded text. Not a search engine,
//! not a crawler: one URL in, bounded text out.
//!
//! Conversion is two passes: [`strip_boilerplate`] drops script,
//! style, and chrome blocks with a small scanner, then `html2md`
//! shapes the rest into Markdown (it does not strip scripts itself,
//! which the tests pin). The `text` format skips the second pass
//! and strips all tags instead.
//!
//! Bounds: the read loop stops at 1 MiB input (memory safety for the
//! sandbox); the returned text truncates at 50 KiB with an omission
//! marker (token safety for the model). Timeouts clamp to 1–120s
//! (default 30) and wrap the native fetch; the WASM guest defers
//! entirely (see below).
//!
//! Delivery note: only the provider world imports `net`, so a
//! tool-world guest cannot fetch. The WASM half validates arguments
//! and reports the deferral plainly; fetching is native-only until
//! the tool world gains a `net` import (additive and link-safe for
//! old components, but a contract change either way).
//!
//! # Unsafe-code exemption
//!
//! The `wasm32` half carries generated `wit-bindgen` export shims; the
//! module holds the allowance.

#![deny(unsafe_code)]

/// The manifest this form ships with (single source for
/// [`manifest_grants`]; a test keeps them in step).
pub const MANIFEST: &str = include_str!("../extension.toml");

/// The grants the manifest declares: `net` with no pre-granted hosts
/// (a bare `*` is rejected at manifest parse, so every host arrives
/// via consent or an ad-hoc grant) - and nothing else.
#[cfg(not(target_arch = "wasm32"))]
pub fn manifest_grants() -> lca_tools::CapabilityGrants {
    lca_tools::CapabilityGrants {
        net: Vec::new(),
        ..Default::default()
    }
}

/// Returned text truncates here, with the omission marker.
pub const OUTPUT_MAX: usize = 50 * 1024;
/// The read loop stops here regardless of content length.
pub const INPUT_MAX: usize = 1024 * 1024;
/// Omission marker for truncated output.
pub const TRUNCATION_MARKER: &str = "\n…(truncated to the 50 KiB fetch bound)";

/// Output shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// Boilerplate stripped, structure as Markdown.
    Markdown,
    /// All tags stripped, words only.
    Text,
    /// Raw body, bounded.
    Html,
}

/// One fetch: URL plus knobs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchRequest {
    /// `http(s)://` URL.
    pub url: String,
    /// Output shape.
    pub format: Format,
    /// Seconds, 1–120.
    pub timeout_secs: u64,
}

/// Parse the tool's arguments: URL required, format defaulting to
/// Markdown, timeout defaulting to 30s and clamped to 1–120.
pub fn parse_webfetch_args(json: &str) -> Result<FetchRequest, String> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|err| format!("arguments are not JSON: {err}"))?;
    let url = value
        .get("url")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "the webfetch tool needs a `url` string".to_string())?
        .to_string();
    let format = match value.get("format").and_then(serde_json::Value::as_str) {
        None | Some("markdown") => Format::Markdown,
        Some("text") => Format::Text,
        Some("html") => Format::Html,
        Some(other) => return Err(format!("unknown webfetch format `{other}`")),
    };
    let timeout_secs = value
        .get("timeout")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(30);
    if !(1..=120).contains(&timeout_secs) {
        return Err(format!(
            "webfetch timeout must be 1–120s, got {timeout_secs}"
        ));
    }
    Ok(FetchRequest {
        url,
        format,
        timeout_secs,
    })
}

/// The network surface the fetch needs: request, status, headers, a
/// bounded body read, and close. Both delivery modes adapt their
/// host client behind it, so the flow below runs identically.
pub trait NetView {
    /// Start a GET; the response handle.
    fn request(&self, method: &str, url: &str, headers: &[(&str, &str)]) -> Result<u32, String>;
    /// The response status.
    fn status(&self, handle: u32) -> Result<u16, String>;
    /// The response headers, lowercased names preferred.
    fn headers(&self, handle: u32) -> Result<Vec<(String, String)>, String>;
    /// Up to `max` more body bytes; `None` at EOF.
    fn read_body(&self, handle: u32, max: usize) -> Result<Option<Vec<u8>>, String>;
    /// Release the handle (errors ignored: best effort).
    fn close(&self, handle: u32);
}

/// Blocks dropped before conversion: scripts, styles, and page
/// chrome. The issue's list, plus `svg` (inline sprite sheets flood
/// context) — the tests pin the set.
const DROP_BLOCKS: &[&str] = &[
    "script", "style", "noscript", "iframe", "nav", "header", "footer", "svg",
];

/// Drop boilerplate blocks, keeping the remaining HTML byte for
/// byte. Tag matching is ASCII case-insensitive; `<script`
/// attributes (including quoted `>` inside them) never confuse the
/// scanner; unclosed blocks swallow to EOF rather than emitting
/// half a page.
pub fn strip_boilerplate(html: &str) -> String {
    let bytes = html.as_bytes();
    let mut out = String::with_capacity(html.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'<'
            && let Some((name, closing, after)) = read_tag(bytes, index)
        {
            let lower = name.to_ascii_lowercase();
            if !closing && DROP_BLOCKS.contains(&lower.as_str()) {
                index = skip_block(bytes, after, &lower);
                continue;
            }
            out.push_str(&html[index..after]);
            index = after;
            continue;
        }
        // Copy one char (UTF-8 safe: advance by its width).
        let width = utf8_width(bytes[index]);
        out.push_str(&html[index..index + width.min(bytes.len() - index)]);
        index += width.min(bytes.len() - index);
    }
    out
}

/// Read `<...>` at `start`: (tag name, is-closing, end offset past
/// `>`). Handles comments and quoted attribute values; `None` when
/// no well-formed tag opens here.
fn read_tag(bytes: &[u8], start: usize) -> Option<(String, bool, usize)> {
    if bytes.get(start) != Some(&b'<') {
        return None;
    }
    let mut index = start + 1;
    // Comments: skip to `-->` wholesale.
    if bytes.get(index..index + 3) == Some(b"!--".as_ref()) {
        let mut end = index + 3;
        while end + 2 < bytes.len() {
            if &bytes[end..end + 3] == b"-->" {
                return Some(("!--".to_string(), true, end + 3));
            }
            end += 1;
        }
        return Some(("!--".to_string(), true, bytes.len()));
    }
    let closing = bytes.get(index) == Some(&b'/');
    if closing {
        index += 1;
    }
    while bytes
        .get(index)
        .is_some_and(|byte| byte.is_ascii_whitespace())
    {
        index += 1;
    }
    let name_start = index;
    while bytes
        .get(index)
        .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'-' || *byte == b':')
    {
        index += 1;
    }
    if index == name_start {
        return None;
    }
    let name = String::from_utf8_lossy(&bytes[name_start..index]).into_owned();
    // Skip attributes (quoted `>` stays inside).
    let mut quote = None;
    while index < bytes.len() {
        let byte = bytes[index];
        if let Some(open) = quote {
            if byte == open {
                quote = None;
            }
        } else if byte == b'"' || byte == b'\'' {
            quote = Some(byte);
        } else if byte == b'>' {
            return Some((name, closing, index + 1));
        }
        index += 1;
    }
    None
}

/// Skip a dropped block from after its open tag through the matching
/// close, depth-counting nested same-name opens.
fn skip_block(bytes: &[u8], mut index: usize, name: &str) -> usize {
    let mut depth = 1;
    while index < bytes.len() && depth > 0 {
        if bytes[index] == b'<'
            && let Some((found, closing, after)) = read_tag(bytes, index)
            && found.eq_ignore_ascii_case(name)
        {
            if closing {
                depth -= 1;
            } else if !is_void_close(bytes, after) {
                depth += 1;
            }
            index = after;
            continue;
        }
        index += utf8_width(bytes[index]).min(bytes.len() - index);
    }
    index
}

/// Whether the tag just read was self-closed (`<br/>`): check the
/// byte before `>` (attribute quotes already consumed by `read_tag`).
fn is_void_close(bytes: &[u8], after: usize) -> bool {
    after >= 2 && bytes.get(after - 2) == Some(&b'/')
}

/// UTF-8 leading-byte width (lone continuations count 1, never 0).
fn utf8_width(byte: u8) -> usize {
    if byte & 0x80 == 0 {
        1
    } else if byte & 0xe0 == 0xc0 {
        2
    } else if byte & 0xf0 == 0xe0 {
        3
    } else if byte & 0xf8 == 0xf0 {
        4
    } else {
        1
    }
}

/// Decode the common entities plus decimal/hex numeric refs;
/// anything unknown passes through verbatim.
pub fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let Some(end) = rest.find(';').filter(|end| *end < 12) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..end];
        let decoded = if entity.starts_with("#x") || entity.starts_with("#X") {
            u32::from_str_radix(entity.trim_start_matches(['#', 'x', 'X']), 16)
                .ok()
                .and_then(char::from_u32)
        } else if let Some(number) = entity.strip_prefix('#') {
            number.parse::<u32>().ok().and_then(char::from_u32)
        } else {
            match entity {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" | "#39" => Some('\''),
                "nbsp" => Some(' '),
                _ => None,
            }
        };
        match decoded {
            Some(character) => out.push(character),
            None => out.push_str(&rest[..=end]),
        }
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out
}

/// Block-level tags break lines in the `text` format; inline tags
/// (`code`, `a`, `strong`, ...) emit nothing so sentences survive.
const BLOCK_TAGS: &[&str] = &[
    "p",
    "div",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "li",
    "ul",
    "ol",
    "br",
    "tr",
    "table",
    "section",
    "article",
    "blockquote",
    "pre",
    "hr",
];
/// Strip every remaining tag for the `text` format: block
/// boundaries become newlines, runs collapse, entities decode.
pub fn html_to_text(html: &str) -> String {
    let bytes = html.as_bytes();
    let mut out = String::with_capacity(html.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'<'
            && let Some((name, _closing, after)) = read_tag(bytes, index)
        {
            if !name.starts_with('!') && BLOCK_TAGS.contains(&name.to_ascii_lowercase().as_str()) {
                out.push('\n');
            }
            index = after;
            continue;
        }
        let width = utf8_width(bytes[index]).min(bytes.len() - index);
        out.push_str(&html[index..index + width]);
        index += width;
    }
    // Collapse blank runs, then decode.
    let mut collapsed = String::with_capacity(out.len());
    let mut blanks = 0;
    for line in out.split('\n') {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            blanks += 1;
            if blanks <= 1 {
                collapsed.push('\n');
            }
        } else {
            blanks = 0;
            collapsed.push_str(trimmed);
            collapsed.push('\n');
        }
    }
    decode_entities(collapsed.trim()).to_string()
}

/// Convert HTML to the requested shape: Markdown through the
/// boilerplate strip plus `html2md`, text through the tag strip,
/// both entity-decoded by construction of the passes.
pub fn convert(html: &str, format: Format) -> String {
    match format {
        Format::Html => html.to_string(),
        Format::Text => html_to_text(&strip_boilerplate(html)),
        Format::Markdown => {
            let stripped = strip_boilerplate(html);
            html2md::parse_html(&stripped)
        }
    }
}

/// Fetch one URL through the capability: scheme gate, request,
/// status gate, MIME gate, bounded read, conversion, output bound.
/// Every refusal names itself; the handle closes on every path.
pub fn fetch(request: &FetchRequest, net: &dyn NetView) -> Result<String, String> {
    if !(request.url.starts_with("http://") || request.url.starts_with("https://")) {
        return Err(format!(
            "webfetch needs an http(s) URL, got `{}`",
            request.url.chars().take(32).collect::<String>()
        ));
    }
    if !(1..=120).contains(&request.timeout_secs) {
        return Err(format!(
            "webfetch timeout must be 1–120s, got {}",
            request.timeout_secs
        ));
    }
    let handle = net.request("GET", &request.url, &[])?;
    let outcome = fetch_via(request, net, handle);
    net.close(handle);
    outcome
}

fn fetch_via(request: &FetchRequest, net: &dyn NetView, handle: u32) -> Result<String, String> {
    let status = net.status(handle)?;
    if !(200..300).contains(&status) {
        return Err(format!("{} answered HTTP {status}", request.url));
    }
    let headers = net.headers(handle)?;
    let mime = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
        .map(|(_, value)| {
            value
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase()
        });
    let is_html = mime.as_deref().is_none_or(|mime| mime.contains("html"));
    let is_text = is_html
        || mime.as_deref().is_some_and(|mime| {
            mime.starts_with("text/") || mime.contains("json") || mime.contains("xml")
        });
    if !is_text {
        return Err(format!(
            "webfetch refuses non-text content: {}",
            mime.as_deref().unwrap_or("(no content type)")
        ));
    }
    let mut body = Vec::new();
    while body.len() < INPUT_MAX {
        match net.read_body(handle, 64 * 1024)? {
            Some(chunk) if !chunk.is_empty() => body.extend(chunk),
            _ => break,
        }
    }
    let text = String::from_utf8_lossy(&body).into_owned();
    let converted = if is_html {
        convert(&text, request.format)
    } else {
        // Plain text needs no conversion; the html shape still
        // bounds it like everything else.
        text
    };
    Ok(bound(&converted))
}

/// Truncate to the output bound with the omission marker (char
/// boundary safe).
pub fn bound(text: &str) -> String {
    if text.len() <= OUTPUT_MAX {
        return text.to_string();
    }
    let mut end = OUTPUT_MAX;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{TRUNCATION_MARKER}", &text[..end])
}

/// The `webfetch` tool spec the model sees.
pub fn webfetch_tool_spec() -> lca_protocol::ToolSpec {
    lca_protocol::ToolSpec {
        name: "webfetch".to_string(),
        description:
            "Fetch an http(s) URL and return bounded Markdown or text; refuses non-text content."
                .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "url": {"type": "string", "description": "The http(s) URL to fetch."},
                "format": {"type": "string", "enum": ["markdown", "text", "html"], "default": "markdown"},
                "timeout": {"type": "integer", "description": "Seconds, 1-120, default 30.", "default": 30}
            },
            "required": ["url"]
        }),
        exposure: lca_protocol::ToolExposure::Direct,
        namespace: None,
        annotations: None,
        extras: Default::default(),
    }
}

// ---------------------------------------------------------------------------
// Native delivery mode
// ---------------------------------------------------------------------------

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use super::*;
    use std::sync::Arc;

    use lca_ext_abi::{DeliveryMode, DispatchFuture, ExtensionDispatch, World};
    use lca_protocol::DispatchError;

    impl NetView for lca_tools::Capabilities {
        fn request(
            &self,
            method: &str,
            url: &str,
            headers: &[(&str, &str)],
        ) -> Result<u32, String> {
            let owned: Vec<(&str, &str)> = headers.to_vec();
            self.net_request(method, url, &owned, None)
                .map_err(|err| err.to_string())
        }

        fn status(&self, handle: u32) -> Result<u16, String> {
            self.net_response_status(handle)
                .map_err(|err| err.to_string())
        }

        fn headers(&self, handle: u32) -> Result<Vec<(String, String)>, String> {
            self.net_response_headers(handle)
                .map_err(|err| err.to_string())
        }

        fn read_body(&self, handle: u32, max: usize) -> Result<Option<Vec<u8>>, String> {
            self.net_read_body(handle, max)
                .map_err(|err| err.to_string())
        }

        fn close(&self, handle: u32) {
            let _ = self.net_close_response(handle);
        }
    }

    /// The native handle. Timeouts wrap the whole fetch: the host's
    /// read timeouts guard each read, the parameter guards the total.
    pub struct Webfetch {
        cap: Arc<lca_tools::Capabilities>,
    }

    impl Webfetch {
        /// Build from the engine whose manifest grants carry `net`.
        pub fn new(cap: Arc<lca_tools::Capabilities>) -> Webfetch {
            Webfetch { cap }
        }
    }

    impl ExtensionDispatch for Webfetch {
        fn name(&self) -> &str {
            "webfetch"
        }

        fn delivery(&self) -> DeliveryMode {
            DeliveryMode::Native
        }

        fn worlds(&self) -> Vec<World> {
            vec![World::Tool]
        }

        fn tool_specs(&self) -> Result<Vec<lca_protocol::ToolSpec>, DispatchError> {
            Ok(vec![webfetch_tool_spec()])
        }

        fn execute_tool<'a>(
            &'a self,
            call: &'a lca_protocol::ToolCall,
        ) -> DispatchFuture<'a, Result<lca_protocol::ToolResult, DispatchError>> {
            let cap = self.cap.clone();
            let arguments = call.arguments.clone();
            let call_id = call.call_id.clone();
            Box::pin(async move {
                let (status, content) = match parse_webfetch_args(&arguments) {
                    Err(reason) => (lca_protocol::ToolResultStatus::Error, reason),
                    Ok(fetched) => {
                        let timeout = std::time::Duration::from_secs(fetched.timeout_secs);
                        let owned = fetched.clone();
                        let outcome = tokio::time::timeout(timeout, async move {
                            tokio::task::spawn_blocking(move || fetch(&owned, cap.as_ref()))
                                .await
                                .map_err(|err| format!("fetch task failed: {err}"))
                        })
                        .await;
                        match outcome {
                            Ok(Ok(Ok(text))) => (lca_protocol::ToolResultStatus::Ok, text),
                            Ok(Ok(Err(reason))) | Ok(Err(reason)) => {
                                (lca_protocol::ToolResultStatus::Error, reason)
                            }
                            Err(_) => (
                                lca_protocol::ToolResultStatus::Error,
                                format!("webfetch timed out after {}s", fetched.timeout_secs),
                            ),
                        }
                    }
                };
                Ok(lca_protocol::ToolResult {
                    call_id,
                    status,
                    content,
                    truncated: false,
                    exit_code: None,
                    full_output_path: None,
                    nested: Vec::new(),
                    duration_ms: None,
                    images: Vec::new(),
                    extras: Default::default(),
                })
            })
        }

        fn command_specs(&self) -> Result<Vec<lca_protocol::CommandSpec>, DispatchError> {
            Ok(Vec::new())
        }

        fn invoke_command(
            &self,
            _name: &str,
            _argument: &str,
        ) -> Result<lca_protocol::CommandEffect, DispatchError> {
            Ok(lca_protocol::CommandEffect::None)
        }

        fn transform_messages(
            &self,
            _messages: Vec<lca_protocol::ChatMessage>,
        ) -> DispatchFuture<
            'static,
            Result<Result<Vec<lca_protocol::ChatMessage>, String>, DispatchError>,
        > {
            Box::pin(std::future::ready(Err(DispatchError::MissingWorld {
                extension: "webfetch".to_string(),
                world: "tool",
            })))
        }

        fn on_pre_turn(&self) -> DispatchFuture<'static, Result<(), DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }

        fn on_pre_tool_use<'a>(
            &'a self,
            _call: &'a lca_protocol::ToolCall,
        ) -> DispatchFuture<'a, Result<lca_protocol::HookAction, DispatchError>> {
            Box::pin(std::future::ready(Ok(lca_protocol::HookAction::Allow)))
        }

        fn on_post_tool_use<'a>(
            &'a self,
            _observation: &'a lca_protocol::PostToolObservation,
        ) -> DispatchFuture<'a, Result<(), DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }

        fn on_post_turn_end<'a>(
            &'a self,
            _status: &'a str,
        ) -> DispatchFuture<'a, Result<(), DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }

        fn on_attention_required<'a>(
            &'a self,
            _reason: &'a str,
        ) -> DispatchFuture<'a, Result<(), DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }

        fn on_session_close(&self) -> DispatchFuture<'static, Result<(), DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use native::Webfetch;

// ---------------------------------------------------------------------------
// WASM delivery mode: the tool world over the net import
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
#[allow(unsafe_code)] // generated wit-bindgen export shims
mod wasm_mode {
    wit_bindgen::generate!({
        path: "../../wit",
        world: "tool",
        export_macro_name: "export_webfetch",
        with: {
            "lca:host/log@0.6.0": generate,
            "lca:host/fs@0.6.0": generate,
            "lca:host/process@0.6.0": generate,
            "lca:host/pty@0.6.0": generate,
            "lca:host/ui-dialogs@0.6.0": generate,
            "lca:host/tools@0.6.0": generate,
            "lca:host/resources@0.6.0": generate,
            "lca:host/state@0.6.0": generate,
        },
    });

    use exports::lca::ext::execute::{Guest as ExecuteTrait, ToolResult as WasmResult};
    use exports::lca::ext::tool_schema::{Guest as SchemaGuest, Schema};
    use lca::ext::types::ToolCall;

    // The tool world has no `net` import (only the provider world
    // does), so the guest cannot fetch. It validates the arguments
    // and says so plainly — an explicit deferral, not a silent one.
    // The unblocker is a `net` import on the tool world (additive:
    // old components do not import it and keep linking); until
    // then, fetching is the native half's job.
    pub struct WebfetchWasm;

    impl SchemaGuest for WebfetchWasm {
        fn get_schema() -> Schema {
            let spec = super::webfetch_tool_spec();
            Schema {
                name: spec.name,
                description: spec.description,
                parameters: spec.parameters.to_string(),
                extras: Vec::new(),
            }
        }
    }

    impl ExecuteTrait for WebfetchWasm {
        fn run(call: ToolCall) -> WasmResult {
            let fail = |content: String| WasmResult {
                call_id: call.call_id.clone(),
                status: "error".to_string(),
                content: Some(content),
                truncated: false,
                extras: Vec::new(),
            };
            if let Err(reason) = super::parse_webfetch_args(&call.arguments) {
                return fail(reason);
            }
            fail(
                "webfetch needs the native half: the tool world has no net import (gh #222)"
                    .to_string(),
            )
        }
    }

    export_webfetch!(WebfetchWasm);
}
