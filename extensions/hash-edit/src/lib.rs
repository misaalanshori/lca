//! Content-hash line-anchored editing (gh #191): lines resolve by
//! short content hash, never by number, so insertions elsewhere in
//! the turn cannot shift an anchor, and repeated blocks cannot
//! silently mis-resolve the way `str_replace` does.
//!
//! Hashing: FNV-1a over the whitespace-trimmed line, rendered as 3
//! lowercase hex chars (12 bits). Twelve bits collide — the birthday
//! math says so past a few dozen lines — so collisions refuse rather
//! than guess: a hash matching two candidate lines is [`EditError::Ambiguous`],
//! never a silent first-match. The model widens the range with a
//! unique neighbor and retries.
//!
//! Writes preserve the file's bytes outside the edited ranges: a
//! UTF-8 BOM is stripped for matching and restored on write, each
//! line keeps its own ending, and inserted lines take the replaced
//! range's first ending (platform notes: never wholesale-convert).
//!
//! # Unsafe-code exemption
//!
//! The `wasm32` half carries generated `wit-bindgen` export shims; the
//! module holds the allowance.

#![deny(unsafe_code)]

/// The manifest this form ships with (single source for
/// [`manifest_grants`]; a test keeps them in step).
pub const MANIFEST: &str = include_str!("../extension.toml");

/// The grants the manifest declares: read-write on the workspace,
/// which is where the edited files live - nothing else.
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::expect_used)] // the extension's own literal manifest patterns are valid by construction.
pub fn manifest_grants() -> lca_tools::CapabilityGrants {
    lca_tools::CapabilityGrants {
        fs: vec![
            lca_permissions::ScopeGrant::parse("workspace", lca_permissions::FsMode::ReadWrite)
                .expect("the manifest's own scope parses"),
        ],
        fs_declared: true,
        ..Default::default()
    }
}

/// One anchored edit: replace the closed line range
/// [`start_hash` ..= `end_hash`] with `new_text`. An empty `end_hash`
/// inserts after the start line; empty `new_text` deletes the range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    /// Hash of the first line to replace (or to insert after).
    pub start_hash: String,
    /// Hash of the last line to replace; empty means insert.
    pub end_hash: String,
    /// Replacement lines (`\n`-separated); empty means delete.
    pub new_text: String,
}

/// Why a batch refused before touching the disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditError {
    /// No line carries the anchor.
    Mismatch {
        /// Which anchor (`start_hash` / `end_hash`).
        which: &'static str,
        /// The anchor no line carried.
        hash: String,
    },
    /// Several lines carry the anchor: widen the range instead.
    Ambiguous {
        /// Which anchor collided.
        which: &'static str,
        /// The colliding anchor.
        hash: String,
        /// How many lines carry it.
        count: usize,
    },
    /// Two edits share a line.
    Overlap,
    /// The end anchor resolves before the start anchor.
    RangeOrder,
    /// The file is not UTF-8.
    NonUtf8,
    /// The filesystem refused.
    Io(String),
}

impl std::fmt::Display for EditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EditError::Mismatch { which, hash } => write!(
                f,
                "Content hash mismatch at {which} '{hash}': no line carries it; the file changed on disk or the anchor was hallucinated"
            ),
            EditError::Ambiguous { which, hash, count } => write!(
                f,
                "Content hash '{hash}' at {which} matches {count} lines: widen the range with a uniquely-hashed neighbor line"
            ),
            EditError::Overlap => {
                write!(f, "Two edits share a line: split them into disjoint ranges")
            }
            EditError::RangeOrder => write!(
                f,
                "The end anchor resolves before the start anchor: swap them or pick a later end"
            ),
            EditError::NonUtf8 => write!(f, "The file is not UTF-8: hash-edit refuses binary"),
            EditError::Io(detail) => write!(f, "Filesystem refused: {detail}"),
        }
    }
}

impl std::error::Error for EditError {}

/// FNV-1a (32-bit) over the trimmed line, rendered as 3 lowercase hex
/// chars. Trimmed, not whitespace-stripped throughout: indentation
/// hallucinations still anchor, interior changes still refuse.
pub fn line_hash(line: &str) -> String {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in line.trim().as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    format!("{:03x}", hash & 0xfff)
}

/// Present a file with every line hash-annotated: the shape the model
/// copies anchors from.
pub fn annotate(text: &str) -> String {
    let mut out = String::new();
    for line in text.split_inclusive('\n') {
        let (body, ending) = split_ending(line);
        out.push_str(&line_hash(body));
        out.push_str(": ");
        out.push_str(body);
        out.push_str(ending);
    }
    out
}

/// Split a line (with its terminator, if any) into body and ending.
fn split_ending(line: &str) -> (&str, &str) {
    if let Some(body) = line.strip_suffix("\r\n") {
        (body, "\r\n")
    } else if let Some(body) = line.strip_suffix('\n') {
        (body, "\n")
    } else {
        (line, "")
    }
}

/// Resolve one anchor: the unique line index carrying it within
/// `lines[start_from..]`, or the refusal. A hash two suffix lines
/// share refuses rather than guessing.
fn resolve(
    lines: &[&str],
    hash: &str,
    which: &'static str,
    start_from: usize,
) -> Result<usize, EditError> {
    let mut index = None;
    let mut count = 0;
    for (offset, line) in lines.iter().enumerate().skip(start_from) {
        if line_hash(line) == hash {
            count += 1;
            if index.is_none() {
                index = Some(offset);
            }
        }
    }
    if count == 0 {
        return Err(EditError::Mismatch {
            which,
            hash: hash.to_string(),
        });
    }
    if let Some(i) = index
        && count == 1
    {
        return Ok(i);
    }
    // Past here the count exceeds 1 (a set index with count 1
    // returned above), so the anchor collides and refuses.
    Err(EditError::Ambiguous {
        which,
        hash: hash.to_string(),
        count,
    })
}

/// Apply every edit to the file's bytes, atomically: all ranges
/// resolve against the original, then later file positions apply
/// first so nothing shifts mid-apply. Any refusal vetoes the batch.
pub fn apply_edits(original: &[u8], edits: &[Edit]) -> Result<Vec<u8>, EditError> {
    let text = std::str::from_utf8(original).map_err(|_| EditError::NonUtf8)?;
    let (bom, text) = text
        .strip_prefix("\u{feff}")
        .map_or(("", text), |rest| ("\u{feff}", rest));
    // Bodies plus their own endings; the last line may be unterminated.
    let mut lines: Vec<(&str, &str)> = Vec::new();
    if !text.is_empty() {
        for line in text.split_inclusive('\n') {
            lines.push(split_ending(line));
        }
    }
    let bodies: Vec<&str> = lines.iter().map(|(body, _)| *body).collect();

    // Resolve every range against the original first. Each
    // footprint is a half-open line span: replace [s..=e] touches
    // (s, e+1); insert-after-s touches the empty (s+1, s+1). The
    // standard overlap test covers both, including an insert
    // landing inside a replaced span.
    let mut footprints: Vec<(usize, usize, &Edit)> = Vec::new();
    for edit in edits {
        let start = resolve(&bodies, &edit.start_hash, "start_hash", 0)?;
        let end = if edit.end_hash.is_empty() {
            start
        } else if edit.end_hash == edit.start_hash {
            resolve(&bodies, &edit.end_hash, "end_hash", start)?
        } else {
            match resolve(&bodies, &edit.end_hash, "end_hash", start + 1) {
                Ok(end) => end,
                Err(EditError::Mismatch { .. }) => {
                    let before = bodies[..start.min(bodies.len())]
                        .iter()
                        .any(|line| line_hash(line) == edit.end_hash);
                    if before {
                        return Err(EditError::RangeOrder);
                    }
                    return Err(EditError::Mismatch {
                        which: "end_hash",
                        hash: edit.end_hash.clone(),
                    });
                }
                Err(other) => return Err(other),
            }
        };
        footprints.push((start, end, edit));
    }
    // Disjoint check on sorted footprints: adjacent spans share at
    // most an edge, which the half-open test accepts.
    let mut spans: Vec<(usize, usize)> = footprints
        .iter()
        .map(|(start, end, edit)| {
            if edit.end_hash.is_empty() {
                (*start + 1, *start + 1)
            } else {
                (*start, *end + 1)
            }
        })
        .collect();
    spans.sort();
    for pair in spans.windows(2) {
        let (a_lo, a_hi) = pair[0];
        let (b_lo, b_hi) = pair[1];
        if a_lo < b_hi && b_lo < a_hi {
            return Err(EditError::Overlap);
        }
    }

    // Rebuild: walk the original lines in file order, splicing
    // replacements (resolution fixed every position against the
    // original, so one forward pass cannot shift another edit).
    let mut out = String::new();
    out.push_str(bom);
    let mut cursor = 0;
    let mut ordered = footprints;
    ordered.sort_by_key(|(start, _, _)| *start);
    for (start, end, edit) in ordered {
        while cursor < start.min(lines.len()) {
            let (body, ending) = lines[cursor];
            out.push_str(body);
            out.push_str(ending);
            cursor += 1;
        }
        if edit.end_hash.is_empty() {
            // Insert after the anchor line: emit it, then the new lines.
            if start < lines.len() {
                let (body, ending) = lines[start];
                out.push_str(body);
                out.push_str(ending);
                cursor = start + 1;
            }
            let ending = lines.get(start).map(|(_, e)| *e).unwrap_or("\n");
            emit_new(&mut out, &edit.new_text, ending);
        } else {
            let ending = lines.get(start).map(|(_, e)| *e).unwrap_or("\n");
            emit_new(&mut out, &edit.new_text, ending);
            cursor = end + 1;
        }
    }
    while cursor < lines.len() {
        let (body, ending) = lines[cursor];
        out.push_str(body);
        out.push_str(ending);
        cursor += 1;
    }
    Ok(out.into_bytes())
}

/// Emit replacement lines with the range's ending (the range's first
/// line sets the style; an empty replacement deletes). One trailing
/// newline is the model's natural terminator, not a blank line, so
/// it is stripped before splitting.
fn emit_new(out: &mut String, new_text: &str, ending: &str) {
    if new_text.is_empty() {
        return;
    }
    let trimmed = new_text.strip_suffix('\n').unwrap_or(new_text);
    for line in trimmed.split('\n') {
        out.push_str(line);
        out.push_str(ending);
    }
}

/// The filesystem view the tool needs: read plus write through the
/// `fs` capability, so neither mode reaches around it.
pub trait FsView: Send + Sync {
    /// One file's bytes; `None` when it does not exist.
    fn read(&self, path: &str) -> Result<Option<Vec<u8>>, lca_protocol::CapabilityError>;
    /// Replace one file's bytes.
    fn write(&self, path: &str, bytes: &[u8]) -> Result<(), lca_protocol::CapabilityError>;
}

/// Run one tool call: parse arguments, apply, write back, and report
/// the byte delta. Everything refuses before any write.
pub fn run_hash_edit(fs: &dyn FsView, path: &str, edits: &[Edit]) -> Result<String, String> {
    let original = fs
        .read(path)
        .map_err(|err| err.to_string())?
        .ok_or_else(|| format!("{path} does not exist"))?;
    let updated = apply_edits(&original, edits).map_err(|err| err.to_string())?;
    fs.write(path, &updated).map_err(|err| err.to_string())?;
    Ok(format!(
        "edited {path}: {} bytes -> {} bytes, {} edit(s)",
        original.len(),
        updated.len(),
        edits.len()
    ))
}

/// Parse the tool's arguments: `path` plus the anchored edits.
pub fn parse_hash_edit_args(json: &str) -> Result<(String, Vec<Edit>), String> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|err| format!("arguments are not JSON: {err}"))?;
    let path = value
        .get("path")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "the hash_edit tool needs a `path` string".to_string())?
        .to_string();
    let raw = value
        .get("edits")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "the hash_edit tool needs an `edits` array".to_string())?;
    if raw.is_empty() {
        return Err("the hash_edit tool needs at least one edit".to_string());
    }
    let mut edits = Vec::new();
    for entry in raw {
        edits.push(Edit {
            start_hash: entry
                .get("start_hash")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "every edit needs a `start_hash` string".to_string())?
                .to_string(),
            end_hash: entry
                .get("end_hash")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "every edit needs an `end_hash` string (empty inserts)".to_string())?
                .to_string(),
            new_text: entry
                .get("new_text")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "every edit needs a `new_text` string (empty deletes)".to_string())?
                .to_string(),
        });
    }
    Ok((path, edits))
}

/// The `hash_edit` tool spec the model sees.
pub fn hash_edit_tool_spec() -> lca_protocol::ToolSpec {
    lca_protocol::ToolSpec {
        name: "hash_edit".to_string(),
        description: "Replace line ranges anchored by content hashes (see annotated reads); refuses on mismatch instead of guessing."
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Workspace-relative file path."
                },
                "edits": {
                    "type": "array",
                    "description": "One or more anchored edits, applied atomically.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "start_hash": {"type": "string"},
                            "end_hash": {"type": "string", "description": "Empty inserts after the start line."},
                            "new_text": {"type": "string", "description": "Empty deletes the range."}
                        },
                        "required": ["start_hash", "end_hash", "new_text"]
                    }
                }
            },
            "required": ["path", "edits"]
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

    impl FsView for lca_tools::Capabilities {
        fn read(&self, path: &str) -> Result<Option<Vec<u8>>, lca_protocol::CapabilityError> {
            match self.fs_read("workspace", path) {
                Ok(bytes) => Ok(Some(bytes)),
                Err(lca_protocol::CapabilityError::NotFound(_)) => Ok(None),
                Err(err) => Err(err),
            }
        }

        fn write(&self, path: &str, bytes: &[u8]) -> Result<(), lca_protocol::CapabilityError> {
            self.fs_write("workspace", path, bytes)
        }
    }

    /// The native handle.
    pub struct HashEdit {
        cap: Arc<lca_tools::Capabilities>,
    }

    impl HashEdit {
        /// Build from the engine whose manifest grants carry
        /// workspace read-write.
        pub fn new(cap: Arc<lca_tools::Capabilities>) -> HashEdit {
            HashEdit { cap }
        }
    }

    impl ExtensionDispatch for HashEdit {
        fn name(&self) -> &str {
            "hash-edit"
        }

        fn delivery(&self) -> DeliveryMode {
            DeliveryMode::Native
        }

        fn worlds(&self) -> Vec<World> {
            vec![World::Tool]
        }

        fn tool_specs(&self) -> Result<Vec<lca_protocol::ToolSpec>, DispatchError> {
            Ok(vec![hash_edit_tool_spec()])
        }

        fn execute_tool<'a>(
            &'a self,
            call: &'a lca_protocol::ToolCall,
        ) -> DispatchFuture<'a, Result<lca_protocol::ToolResult, DispatchError>> {
            let cap = self.cap.clone();
            let arguments = call.arguments.clone();
            let call_id = call.call_id.clone();
            Box::pin(async move {
                let outcome = (|| {
                    let (path, edits) = parse_hash_edit_args(&arguments)?;
                    run_hash_edit(cap.as_ref(), &path, &edits)
                })();
                let (status, content) = match outcome {
                    Ok(report) => (lca_protocol::ToolResultStatus::Ok, report),
                    Err(reason) => (lca_protocol::ToolResultStatus::Error, reason),
                };
                Ok(lca_protocol::ToolResult {
                    call_id,
                    status,
                    content,
                    truncated: false,
                    exit_code: None,
                    full_output_path: None,
                    nested: Vec::new(),
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
                extension: "hash-edit".to_string(),
                world: "context-transform",
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
pub use native::HashEdit;

// ---------------------------------------------------------------------------
// WASM delivery mode: the tool world behind the host's fs import
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
#[allow(unsafe_code)] // generated wit-bindgen export shims
mod wasm_mode {
    wit_bindgen::generate!({
        path: "../../wit",
        world: "tool",
        export_macro_name: "export_hash_edit",
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
    use lca::host::fs;

    struct GuestFs;

    impl super::FsView for GuestFs {
        fn read(&self, path: &str) -> Result<Option<Vec<u8>>, lca_protocol::CapabilityError> {
            match fs::read("workspace", path) {
                Ok(bytes) => Ok(Some(bytes)),
                Err(_) => Ok(None),
            }
        }

        fn write(&self, path: &str, bytes: &[u8]) -> Result<(), lca_protocol::CapabilityError> {
            fs::write("workspace", path, bytes)
                .map_err(|_| lca_protocol::CapabilityError::Io("fs write refused".to_string()))?;
            Ok(())
        }
    }

    pub struct HashEditWasm;

    impl SchemaGuest for HashEditWasm {
        fn get_schema() -> Schema {
            let spec = super::hash_edit_tool_spec();
            Schema {
                name: spec.name,
                description: spec.description,
                parameters: spec.parameters.to_string(),
                extras: Vec::new(),
            }
        }
    }

    impl ExecuteTrait for HashEditWasm {
        fn run(call: ToolCall) -> WasmResult {
            let fail = |content: String| WasmResult {
                call_id: call.call_id.clone(),
                status: "error".to_string(),
                content: Some(content),
                truncated: false,
                extras: Vec::new(),
            };
            let (path, edits) = match super::parse_hash_edit_args(&call.arguments) {
                Ok(parsed) => parsed,
                Err(reason) => return fail(reason),
            };
            match super::run_hash_edit(&GuestFs, &path, &edits) {
                Ok(report) => WasmResult {
                    call_id: call.call_id,
                    status: "ok".to_string(),
                    content: Some(report),
                    truncated: false,
                    extras: Vec::new(),
                },
                Err(reason) => fail(reason),
            }
        }
    }

    export_hash_edit!(HashEditWasm);
}
