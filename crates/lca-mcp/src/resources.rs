//! The resource side of MCP (gh #418 Phase C split; pi's `resources.rs`):
//! listing resources and templates, reading one, and the three bridge tools
//! that expose them to the model.
//!
//! Split out of `lib.rs` to keep every file under the 1,200-line ceiling
//! (gate 11). The types stay re-exported from the crate root, so callers
//! see no change.

use std::collections::BTreeMap;

/// One listed resource (pi's `list_mcp_resources` row).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpResource {
    /// The address `read_mcp_resource` reads.
    pub uri: String,
    /// The display name.
    pub name: String,
    /// The longer title, when listed.
    pub title: Option<String>,
    /// One line about it, when listed.
    pub description: Option<String>,
    /// The content type, when listed.
    pub mime_type: Option<String>,
}

/// One listed URI template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpResourceTemplate {
    /// The template (`echo://{name}`).
    pub uri_template: String,
    /// The display name.
    pub name: String,
    /// The longer title, when listed.
    pub title: Option<String>,
    /// One line about it, when listed.
    pub description: Option<String>,
    /// The content type, when listed.
    pub mime_type: Option<String>,
}

/// One read resource content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceContent {
    /// Text (or text-like) content.
    Text {
        /// Where it was read from.
        uri: String,
        /// The listed type, when sent.
        mime_type: Option<String>,
        /// The text itself.
        text: String,
    },
    /// Binary content (base64 on the wire, bytes here).
    Blob {
        /// Where it was read from.
        uri: String,
        /// The listed type, when sent.
        mime_type: Option<String>,
        /// The decoded bytes.
        bytes: Vec<u8>,
    },
}

/// Map one `resources/list` result to resources plus the next cursor.
pub fn resources_from_list(result: &serde_json::Value) -> (Vec<McpResource>, Option<String>) {
    let empty = Vec::new();
    let listed = result
        .get("resources")
        .and_then(|resources| resources.as_array())
        .unwrap_or(&empty);
    let text = |entry: &serde_json::Value, key: &str| {
        entry
            .get(key)
            .and_then(|value| value.as_str())
            .map(str::to_string)
    };
    let resources = listed
        .iter()
        .filter_map(|entry| {
            Some(McpResource {
                uri: text(entry, "uri")?,
                name: text(entry, "name").unwrap_or_default(),
                title: text(entry, "title"),
                description: text(entry, "description"),
                mime_type: text(entry, "mimeType"),
            })
        })
        .collect();
    let cursor = result
        .get("nextCursor")
        .and_then(|cursor| cursor.as_str())
        .map(str::to_string);
    (resources, cursor)
}

/// Map one `resources/templates/list` result to templates (a server
/// without the method answers method-not-found; the sessions turn
/// that into an empty list, pi's `withoutTemplates`).
pub fn templates_from_list(result: &serde_json::Value) -> Vec<McpResourceTemplate> {
    let empty = Vec::new();
    let listed = result
        .get("resourceTemplates")
        .and_then(|templates| templates.as_array())
        .unwrap_or(&empty);
    let text = |entry: &serde_json::Value, key: &str| {
        entry
            .get(key)
            .and_then(|value| value.as_str())
            .map(str::to_string)
    };
    listed
        .iter()
        .filter_map(|entry| {
            Some(McpResourceTemplate {
                uri_template: text(entry, "uriTemplate")?,
                name: text(entry, "name").unwrap_or_default(),
                title: text(entry, "title"),
                description: text(entry, "description"),
                mime_type: text(entry, "mimeType"),
            })
        })
        .collect()
}

/// Map one `resources/read` result to contents.
pub fn contents_from_read(result: &serde_json::Value) -> Result<Vec<ResourceContent>, String> {
    let empty = Vec::new();
    let listed = result
        .get("contents")
        .and_then(|contents| contents.as_array())
        .unwrap_or(&empty);
    let mut out = Vec::with_capacity(listed.len());
    for entry in listed {
        let uri = entry
            .get("uri")
            .and_then(|uri| uri.as_str())
            .unwrap_or("")
            .to_string();
        let mime_type = entry
            .get("mimeType")
            .and_then(|mime| mime.as_str())
            .map(str::to_string);
        if let Some(text) = entry.get("text").and_then(|text| text.as_str()) {
            out.push(ResourceContent::Text {
                uri,
                mime_type,
                text: text.to_string(),
            });
        } else if let Some(blob) = entry.get("blob").and_then(|blob| blob.as_str()) {
            out.push(ResourceContent::Blob {
                uri,
                mime_type,
                bytes: crate::wire::base64_decode(blob)?,
            });
        } else {
            return Err("the server sent unreadable resource content".to_string());
        }
    }
    Ok(out)
}

/// The bridge's resource tools (pi's names): they reach every
/// connected, non-hidden server that offers resources.
pub const LIST_RESOURCES_TOOL: &str = "list_mcp_resources";
/// The bridge's template lister.
pub const LIST_TEMPLATES_TOOL: &str = "list_mcp_resource_templates";
/// The bridge's resource reader.
pub const READ_RESOURCE_TOOL: &str = "read_mcp_resource";

/// The three resource tools at one exposure.
pub fn resource_tool_specs(exposure: lca_protocol::ToolExposure) -> Vec<lca_protocol::ToolSpec> {
    let spec =
        |name: &str, description: &str, parameters: serde_json::Value| lca_protocol::ToolSpec {
            name: name.to_string(),
            description: description.to_string(),
            parameters,
            exposure,
            namespace: None,
            annotations: None,
            extras: BTreeMap::new(),
        };
    vec![
        spec(
            LIST_RESOURCES_TOOL,
            "List MCP resources as JSON. With `server`, one page (`cursor` continues); without, the first page of every server.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "server": {"type": "string"},
                    "cursor": {"type": "string"},
                },
            }),
        ),
        spec(
            LIST_TEMPLATES_TOOL,
            "List MCP resource URI templates. With `server`, one server's; without, every server's.",
            serde_json::json!({
                "type": "object",
                "properties": {"server": {"type": "string"}},
            }),
        ),
        spec(
            READ_RESOURCE_TOOL,
            "Read one MCP resource by `server` and `uri`. Text arrives as text, images as images, other bytes as a file path.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "server": {"type": "string"},
                    "uri": {"type": "string"},
                },
                "required": ["server", "uri"],
            }),
        ),
    ]
}

/// The widest exposure among resource-serving servers (pi's rule:
/// `direct`, then `codemode` or `deferred`). `None` registers no
/// resource tools.
pub fn widest_resource_exposure(
    exposures: &[lca_protocol::ToolExposure],
) -> Option<lca_protocol::ToolExposure> {
    use lca_protocol::ToolExposure as Exposure;
    [Exposure::Direct, Exposure::Codemode, Exposure::Deferred]
        .into_iter()
        .find(|level| exposures.contains(level))
}
