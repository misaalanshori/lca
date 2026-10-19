//! The HTML transcript export (gh #57): pi's shape (sidebar,
//! hidden-message toggle), self-contained (inline style, script, and
//! images), offline by construction. The renderer is pure and
//! deterministic, so the golden test pins it byte for byte.

use std::collections::BTreeMap;

use lca_protocol::Record;

/// One attachment for inlining: the media type plus the bytes.
#[derive(Debug, Clone)]
pub struct HtmlImage {
    /// IANA media type, e.g. `image/png`.
    pub media_type: String,
    /// The raw bytes, base64'd into a data URL.
    pub bytes: Vec<u8>,
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'&' => out.push_str("&amp;"),
            b'<' => out.push_str("&lt;"),
            b'>' => out.push_str("&gt;"),
            b'"' => out.push_str("&quot;"),
            _ => out.push(byte as char),
        }
    }
    out
}

/// One transcript row: the anchor id, the sidebar label, the role
/// class, the body HTML, and whether the toggle hides it.
struct Row {
    id: String,
    label: String,
    class: &'static str,
    body: String,
    hidden: bool,
}

fn preview(text: &str) -> String {
    text.chars().take(48).collect()
}

fn image_tag(image: &HtmlImage) -> String {
    // Gh #312: `media_type` is attachment data, not a literal, so it is
    // escaped like every other interpolated value. Unescaped it can close
    // the `src` attribute and inject a handler - the same injection the
    // text escaping already prevents for content.
    format!(
        "<img alt=\"attached image\" src=\"data:{};base64,{}\"/>",
        escape(&image.media_type),
        lca_protocol::base64_encode(&image.bytes)
    )
}

/// Render the records to one self-contained page. Text content is the
/// transcript's text in order; `display: false` records hide behind
/// the toggle; unknown hashes leave the stub text standing.
pub fn render_html(
    title: &str,
    records: &[Record],
    attachments: &BTreeMap<String, HtmlImage>,
) -> String {
    let mut rows: Vec<Row> = Vec::new();
    for (index, record) in records.iter().enumerate() {
        let id = format!("m{index}");
        match record {
            Record::User {
                content,
                attachments: hashes,
                ..
            } => {
                let mut body = format!("<p>{}</p>", escape(content));
                for hash in hashes {
                    if let Some(image) = attachments.get(hash) {
                        body.push_str(&image_tag(image));
                    }
                }
                rows.push(Row {
                    id,
                    label: format!("you: {}", preview(content)),
                    class: "message-user",
                    body,
                    hidden: false,
                });
            }
            Record::Assistant { content, .. } => {
                let mut body = String::new();
                let mut text = String::new();
                let mut images_html = String::new();
                for block in content {
                    match block {
                        lca_protocol::ContentBlock::Text { text: part } => {
                            text.push_str(part);
                        }
                        lca_protocol::ContentBlock::Image { media_type, bytes } => {
                            images_html.push_str(&image_tag(&HtmlImage {
                                media_type: media_type.clone(),
                                bytes: bytes.clone(),
                            }));
                        }
                        _ => {}
                    }
                }
                body.push_str(&format!("<p>{}</p>", escape(&text)));
                body.push_str(&images_html);
                rows.push(Row {
                    id,
                    label: format!("assistant: {}", preview(&text)),
                    class: "message-assistant",
                    body,
                    hidden: false,
                });
            }
            Record::ToolCall {
                name, arguments, ..
            } => rows.push(Row {
                id,
                label: format!("tool: {name}"),
                class: "message-tool-call",
                body: format!(
                    "<p><code>{}</code> <code>{}</code></p>",
                    escape(name),
                    escape(arguments)
                ),
                hidden: false,
            }),
            Record::ToolResult {
                content,
                images,
                truncated,
                ..
            } => {
                let mut body = format!(
                    "<p>{}</p>",
                    escape(content.as_deref().unwrap_or("[no output]"))
                );
                if *truncated {
                    body.push_str("<p>[result truncated]</p>");
                }
                for hash in images {
                    if let Some(image) = attachments.get(hash) {
                        body.push_str(&image_tag(image));
                    }
                }
                rows.push(Row {
                    id,
                    label: "tool result".to_string(),
                    class: "message-tool-result",
                    body,
                    hidden: false,
                });
            }
            Record::Compaction { summary, .. } => rows.push(Row {
                id,
                label: "compaction summary".to_string(),
                class: "message-compaction",
                body: format!("<p>{}</p>", escape(summary)),
                hidden: true,
            }),
            Record::CustomMessage {
                custom_type,
                content,
                display,
                ..
            } => rows.push(Row {
                id,
                label: custom_type.clone(),
                class: "message-custom",
                body: format!("<p>{}</p>", escape(content)),
                hidden: !display,
            }),
            _ => {}
        }
    }
    let mut sidebar = String::new();
    for row in &rows {
        // Gh #312: the id is exporter input, so it is escaped here exactly
        // as it is in the article below; the two must name the same anchor.
        sidebar.push_str(&format!(
            "<a href=\"#{}\" class=\"{}\">{}</a>",
            escape(&row.id),
            row.class,
            escape(&row.label)
        ));
    }
    let mut main = String::new();
    for row in &rows {
        // Gh #312: `row.class` is already `message-user` / `message-assistant`
        // / ... so `"message{}"` concatenated two class names into one
        // unknown token (`messagemessage-user`), matching none of the
        // stylesheet's `.message-*` selectors. The base `message` marker, the
        // row's own class, and the hidden marker are three separate tokens.
        let hidden_class = if row.hidden { " message-hidden" } else { "" };
        main.push_str(&format!(
            "<article id=\"{}\" class=\"message {}{}\">{}</article>",
            escape(&row.id),
            row.class,
            hidden_class,
            row.body
        ));
    }
    format!(
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\"/>\n<title>{}</title>\n<style>{}</style>\n</head>\n<body>\n<header><h1>{}</h1><label><input type=\"checkbox\" id=\"toggle-hidden\"/> show hidden messages</label></header>\n<nav id=\"sidebar\">{}</nav>\n<main>{}</main>\n<script>{}</script>\n</body>\n</html>\n",
        escape(title),
        STYLE,
        escape(title),
        sidebar,
        main,
        SCRIPT
    )
}

/// The page style: sidebar left, transcript right, hidden rows
/// collapsed until the toggle shows them.
const STYLE: &str = "body{font-family:system-ui,sans-serif;display:grid;grid-template-columns:220px 1fr;gap:1rem;margin:0}\
header{grid-column:1/-1;display:flex;justify-content:space-between;align-items:center;padding:.5rem 1rem;border-bottom:1px solid #ccc}\
#sidebar{padding:1rem;overflow:auto;border-right:1px solid #ccc;max-height:100vh}\
#sidebar a{display:block;padding:.25rem 0;color:inherit;text-decoration:none}\
main{padding:1rem;max-width:60rem}\
article{border-bottom:1px solid #eee;padding:.5rem 0}\
article img{max-width:100%}\
.message-hidden{display:none}\
body.show-hidden .message-hidden{display:block}\
code{background:#f4f4f4;padding:.1rem .3rem}";

/// The hidden-message toggle: one checkbox, one class.
const SCRIPT: &str = "document.getElementById('toggle-hidden').addEventListener('change',function(e){document.body.classList.toggle('show-hidden',e.target.checked);});";

use std::path::{Path, PathBuf};

use super::store::write_atomic;
use super::{ExportOptions, Session, SessionStore};
use crate::Result;
use crate::view::ViewMode;

impl SessionStore {
    /// The export's `attachments` map: every hash a record references,
    /// mapped to its sidecar path. A fork's resolved records reference the
    /// ancestor's files, so the search walks the fork chain and the path is
    /// relative to the export file: `attachments/<hash>` when the session
    /// owns the file, `../<owner>/attachments/<hash>` when an ancestor does.
    /// Only files that exist are listed; the format allows inline base64
    /// too, but the sidecar keeps large tool output out of the JSON
    /// (`docs/session-log-format.md`).
    fn attachment_map(
        &self,
        session: &Session,
        records: &[lca_protocol::Record],
    ) -> serde_json::Map<String, serde_json::Value> {
        let mut map = serde_json::Map::new();
        // gh #311: the SAME walker the GC uses, so "export resolves" and "gc
        // keeps" cannot drift apart again. It used to be an inline closure
        // here and a different one in `store::gc`, and they disagreed about
        // tool-result images.
        for hash in crate::attachments::reachable_set(records) {
            let Some(path) = self.attachment_path(session, &hash) else {
                continue;
            };
            let own = session.dir().join("attachments").join(&hash);
            let value = if path == own {
                format!("attachments/{hash}")
            } else {
                let owner = path
                    .parent()
                    .and_then(Path::parent)
                    .and_then(Path::file_name)
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                format!("../{owner}/attachments/{hash}")
            };
            map.insert(hash, serde_json::Value::String(value));
        }
        map
    }

    /// Export the resolved record list with metadata
    /// (`docs/session-log-format.md`); `permission`, `extension-event`,
    /// `usage`, and `custom` records are stripped unless `audit` is set
    /// (FR-SESS-7).
    pub fn export(&self, session: &Session, options: ExportOptions) -> Result<PathBuf> {
        let resolved = self.read_with(session, ViewMode::Audit)?;
        let records: Vec<_> = if options.audit {
            resolved.records
        } else {
            resolved
                .records
                .into_iter()
                .filter(|r| {
                    !matches!(
                        r,
                        lca_protocol::Record::Permission { .. }
                            | lca_protocol::Record::ExtensionEvent { .. }
                            | lca_protocol::Record::Usage { .. }
                            | lca_protocol::Record::Custom { .. }
                    )
                })
                .collect()
        };
        let meta = self.meta(session)?;
        let attachments = self.attachment_map(session, &records);
        let document = serde_json::json!({
            "meta": meta,
            "records": records,
            "attachments": attachments,
        });
        let out = session.dir().join(if options.audit {
            "export-audit.json"
        } else {
            "export.json"
        });
        write_atomic(&out, &serde_json::to_vec_pretty(&document)?)?;
        Ok(out)
    }

    /// Write the self-contained HTML transcript beside the JSON
    /// export (gh #57): the same records (audit-filtered alike),
    /// attachments inlined as data URLs. Missing files resolve to
    /// their stub text, never an error.
    pub fn export_html(&self, session: &Session, options: ExportOptions) -> Result<PathBuf> {
        let resolved = self.read_with(session, ViewMode::Audit)?;
        let records: Vec<_> = if options.audit {
            resolved.records
        } else {
            resolved
                .records
                .into_iter()
                .filter(|r| {
                    !matches!(
                        r,
                        lca_protocol::Record::Permission { .. }
                            | lca_protocol::Record::ExtensionEvent { .. }
                            | lca_protocol::Record::Usage { .. }
                            | lca_protocol::Record::Custom { .. }
                    )
                })
                .collect()
        };
        let mut images = std::collections::BTreeMap::new();
        // gh #311: the same walker as gc and the JSON export map. A
        // tool-result text spill also lands here now, which is harmless: the
        // `sniff_image_media_type` gate below drops anything that is not an
        // image, so the inlined set is unchanged.
        for hash in crate::attachments::reachable_set(&records) {
            let Some(path) = self.attachment_path(session, &hash) else {
                continue;
            };
            let Ok(bytes) = std::fs::read(path) else {
                continue;
            };
            let Some(media_type) = lca_protocol::sniff_image_media_type(&bytes) else {
                continue;
            };
            images.insert(
                hash,
                HtmlImage {
                    media_type: media_type.to_string(),
                    bytes,
                },
            );
        }
        let title = self
            .meta(session)
            .map(|meta| meta.title)
            .unwrap_or_else(|_| session.id().to_string());
        let html = render_html(&title, &records, &images);
        let out = session.dir().join("export.html");
        write_atomic(&out, html.as_bytes())?;
        Ok(out)
    }
}
