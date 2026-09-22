//! Built-in tools: read, write, edit, list, glob, grep, and shell
//! (FR-TOOL-1), behind the swappable backend trait from ADR-0013.
//!
//! No capability gates these tools: they are what defines the model's
//! access to the workspace (ADR-0013's build-time backend category). The
//! permission layer still sees every shell command and every write outside
//! the workspace through [`ToolExecutor::required_permission`] (FR-TOOL-3).

#![deny(unsafe_code)]

mod ops;

pub use ops::{Entry, ExecOutcome, NativeOps, Stat, ToolOps};

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use lca_permissions::Action;
use lca_protocol::{ToolCall, ToolResult, ToolResultStatus, ToolSpec};

/// Cooperative cancellation for one turn's work (FR-CONC-3).
#[derive(Clone)]
pub struct CancelFlag {
    inner: Arc<(tokio::sync::Notify, std::sync::atomic::AtomicBool)>,
}

impl Default for CancelFlag {
    fn default() -> Self {
        Self::new()
    }
}

impl CancelFlag {
    /// A fresh, uncancelled flag.
    pub fn new() -> CancelFlag {
        CancelFlag {
            inner: Arc::new((
                tokio::sync::Notify::new(),
                std::sync::atomic::AtomicBool::new(false),
            )),
        }
    }

    /// Trigger cancellation; every waiter wakes.
    pub fn cancel(&self) {
        self.inner
            .1
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.inner.0.notify_waiters();
    }

    /// Whether cancellation has been triggered.
    pub fn is_cancelled(&self) -> bool {
        self.inner.1.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Wait until cancellation triggers (returns immediately when already
    /// cancelled, so a cancel between poll wakeups is never missed).
    pub async fn wait_cancelled(&self) {
        loop {
            if self.is_cancelled() {
                return;
            }
            let notified = self.inner.0.notified();
            tokio::pin!(notified);
            // Register interest before re-checking the flag.
            notified.as_mut().enable();
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }
}

/// Fingerprint of a file the session has read, for FR-TOOL-2 staleness.
#[derive(Debug, Default)]
struct ReadTracker {
    fingerprints: HashMap<PathBuf, u64>,
}

impl ReadTracker {
    fn record(&mut self, path: &Path, bytes: &[u8]) {
        self.fingerprints
            .insert(path.to_path_buf(), fingerprint(bytes));
    }

    fn fresh_read(&self, path: &Path, current: &[u8]) -> bool {
        self.fingerprints
            .get(path)
            .is_some_and(|known| *known == fingerprint(current))
    }
}

fn fingerprint(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

/// The built-in tool executor.
pub struct ToolExecutor {
    ops: Arc<dyn ToolOps>,
    workspace: PathBuf,
    cwd: PathBuf,
    result_limit_bytes: usize,
    default_timeout: Duration,
    tracker: ReadTracker,
}

impl ToolExecutor {
    /// Build an executor over a backend.
    pub fn new(
        ops: Arc<dyn ToolOps>,
        workspace: PathBuf,
        cwd: PathBuf,
        result_limit_bytes: usize,
        default_timeout: Duration,
    ) -> ToolExecutor {
        ToolExecutor {
            ops,
            workspace,
            cwd,
            result_limit_bytes,
            default_timeout,
            tracker: ReadTracker::default(),
        }
    }

    /// The workspace root this executor serves.
    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    /// The tool specs handed to the model (FR-TOOL-1).
    pub fn specs() -> Vec<ToolSpec> {
        let spec =
            |name: &str, description: &str, properties: serde_json::Value, required: &[&str]| {
                ToolSpec {
                    name: name.to_string(),
                    description: description.to_string(),
                    parameters: serde_json::json!({
                        "type": "object",
                        "properties": properties,
                        "required": required,
                    }),
                    extras: Default::default(),
                }
            };
        vec![
            spec(
                "read",
                "Read a file. Returns content with line numbers. Use offset (1-indexed) and limit to page through large files.",
                serde_json::json!({
                    "path": {"type": "string", "description": "File to read, relative to the workspace or absolute"},
                    "offset": {"type": "integer", "description": "1-indexed line to start at"},
                    "limit": {"type": "integer", "description": "Maximum number of lines"},
                }),
                &["path"],
            ),
            spec(
                "write",
                "Create or replace a file with the given content, creating parent directories.",
                serde_json::json!({
                    "path": {"type": "string"},
                    "content": {"type": "string"},
                }),
                &["path", "content"],
            ),
            spec(
                "edit",
                "Make precise file edits with exact text replacement. Every edits[].oldText must be unique in the original file and must not overlap another edit. Each edit matches the original file, not the result of earlier edits.",
                serde_json::json!({
                    "path": {"type": "string"},
                    "edits": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "oldText": {"type": "string", "description": "Exact text to replace; must be unique"},
                                "newText": {"type": "string", "description": "Replacement text"},
                            },
                            "required": ["oldText", "newText"],
                        },
                    },
                }),
                &["path", "edits"],
            ),
            spec(
                "list",
                "List a directory. Directories are suffixed with '/'.",
                serde_json::json!({
                    "path": {"type": "string", "description": "Directory relative to the workspace (default: .)"},
                }),
                &[],
            ),
            spec(
                "glob",
                "Find files by glob pattern. Supports *, ?, and ** (any depth). Results are workspace-relative paths.",
                serde_json::json!({
                    "pattern": {"type": "string", "description": "Glob pattern such as src/**/*.rs"},
                }),
                &["pattern"],
            ),
            spec(
                "grep",
                "Search file contents with a regular expression. Returns path:line:match lines. Skips .git, target, and node_modules; .gitignore is not honored.",
                serde_json::json!({
                    "pattern": {"type": "string", "description": "Regular expression, or literal text when literal is true"},
                    "path": {"type": "string", "description": "Directory or file to search (default: workspace root)"},
                    "glob": {"type": "string", "description": "Filter files by glob, e.g. '*.rs'"},
                    "ignoreCase": {"type": "boolean"},
                    "literal": {"type": "boolean", "description": "Treat the pattern as a literal string"},
                    "limit": {"type": "integer", "description": "Maximum matches to return (default 100)"},
                }),
                &["pattern"],
            ),
            spec(
                "shell",
                "Run a command in the platform shell in the workspace directory. Streams output; output is truncated to the last part of the run when too large. Optionally set a timeout in seconds.",
                serde_json::json!({
                    "command": {"type": "string"},
                    "timeout": {"type": "integer", "description": "Seconds before the command is killed (default: configured tool timeout)"},
                }),
                &["command"],
            ),
        ]
    }

    /// What must be approved before this call runs (FR-TOOL-3): every shell
    /// command, and any write whose resolved target leaves the workspace.
    pub fn required_permission(&self, call: &ToolCall) -> Option<Action> {
        let Ok(args) = serde_json::from_str::<serde_json::Value>(&call.arguments) else {
            return None;
        };
        match call.name.as_str() {
            "shell" => {
                let command = args
                    .get("command")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                Some(Action::Shell {
                    command: command.to_string(),
                    cwd: self.cwd.clone(),
                })
            }
            "write" | "edit" => {
                let path = args.get("path").and_then(|v| v.as_str())?;
                let target = resolve_target(&self.cwd, Path::new(path));
                if is_inside(&target, &self.workspace) {
                    None
                } else {
                    Some(Action::WritePath { path: target })
                }
            }
            _ => None,
        }
    }

    /// Run one call and return its result (FR-TOOL-7 marks truncation).
    pub async fn execute(
        &mut self,
        call: &ToolCall,
        on_output: &mut (dyn FnMut(&[u8]) + Send),
        cancel: &CancelFlag,
    ) -> ToolResult {
        let args: serde_json::Value = match serde_json::from_str(&call.arguments) {
            Ok(value) => value,
            Err(err) => {
                return ToolResult::error(call.call_id.clone(), format!("bad arguments: {err}"));
            }
        };
        match call.name.as_str() {
            "read" => self.read(call, &args).await,
            "write" => self.write(call, &args).await,
            "edit" => self.edit(call, &args).await,
            "list" => self.list(call, &args).await,
            "glob" => self.glob(call, &args).await,
            "grep" => self.grep(call, &args).await,
            "shell" => self.shell(call, &args, on_output, cancel).await,
            other => ToolResult::error(call.call_id.clone(), format!("unknown tool `{other}`")),
        }
    }

    async fn read(&mut self, call: &ToolCall, args: &serde_json::Value) -> ToolResult {
        let Some(path) = args.get("path").and_then(|v| v.as_str()) else {
            return ToolResult::error(call.call_id.clone(), "path is required");
        };
        let target = resolve_target(&self.cwd, Path::new(path));
        let bytes = match self.ops.read(&target) {
            Ok(bytes) => bytes,
            Err(err) => {
                return ToolResult::error(
                    call.call_id.clone(),
                    format!("cannot read {path}: {err}"),
                );
            }
        };
        self.tracker.record(&target, &bytes);
        let text = String::from_utf8_lossy(&bytes);
        let mut lines: Vec<&str> = text.split('\n').collect();
        if text.ends_with('\n') {
            lines.pop(); // the trailing newline is not an extra line
        }
        let total = lines.len();
        let offset = args
            .get("offset")
            .and_then(|v| v.as_u64())
            .unwrap_or(1)
            .max(1) as usize;
        if offset > total {
            return ToolResult::error(
                call.call_id.clone(),
                format!("Offset {offset} is beyond end of file ({total} lines total)"),
            );
        }
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize);
        let end = match limit {
            Some(limit) => (offset - 1 + limit).min(total),
            None => total,
        };
        let mut numbered = String::new();
        for (index, line) in lines[offset - 1..end].iter().enumerate() {
            numbered.push_str(&format!("{:6}\t{line}\n", offset + index));
        }
        let (content, truncated) = truncate_head(&numbered, self.result_limit_bytes);
        let mut out = content;
        if truncated {
            let shown_lines = out.lines().count();
            let last = offset - 1 + shown_lines;
            out.push_str(&format!(
                "\n[Showing lines {offset}-{last} of {total} ({} limit). Use offset={} to continue.]",
                format_bytes(self.result_limit_bytes),
                last + 1
            ));
        } else if end < total {
            out.push_str(&format!(
                "\n[{} more lines in file. Use offset={} to continue.]",
                total - end,
                end + 1
            ));
        }
        ToolResult {
            call_id: call.call_id.clone(),
            status: ToolResultStatus::Ok,
            content: out,
            truncated,
            extras: Default::default(),
        }
    }

    async fn write(&mut self, call: &ToolCall, args: &serde_json::Value) -> ToolResult {
        let (Some(path), Some(content)) = (
            args.get("path").and_then(|v| v.as_str()),
            args.get("content").and_then(|v| v.as_str()),
        ) else {
            return ToolResult::error(call.call_id.clone(), "path and content are required");
        };
        let target = resolve_target(&self.cwd, Path::new(path));
        match self.ops.write(&target, content.as_bytes()) {
            Ok(()) => {
                self.tracker.record(&target, content.as_bytes());
                ToolResult::ok(
                    call.call_id.clone(),
                    format!("Wrote {} to {path}.", format_bytes(content.len())),
                )
            }
            Err(err) => {
                ToolResult::error(call.call_id.clone(), format!("cannot write {path}: {err}"))
            }
        }
    }

    async fn edit(&mut self, call: &ToolCall, args: &serde_json::Value) -> ToolResult {
        let Some(path) = args.get("path").and_then(|v| v.as_str()) else {
            return ToolResult::error(call.call_id.clone(), "path is required");
        };
        let Some(edits) = args.get("edits").and_then(|v| v.as_array()) else {
            return ToolResult::error(
                call.call_id.clone(),
                "edits must contain at least one replacement",
            );
        };
        if edits.is_empty() {
            return ToolResult::error(
                call.call_id.clone(),
                "edits must contain at least one replacement",
            );
        }
        let target = resolve_target(&self.cwd, Path::new(path));
        let original = match self.ops.read(&target) {
            Ok(bytes) => bytes,
            Err(err) => {
                return ToolResult::error(
                    call.call_id.clone(),
                    format!("Could not edit file: {path}. {err}."),
                );
            }
        };
        // FR-TOOL-2: reject when the file changed since this session last
        // read it (or was never read at all).
        if !self.tracker.fresh_read(&target, &original) {
            return ToolResult::error(
                call.call_id.clone(),
                format!(
                    "The file {path} changed since it was last read (or was never read this session). \
                     Read it again before editing."
                ),
            );
        }
        let original = String::from_utf8_lossy(&original).into_owned();

        // Match every edit against the ORIGINAL, require uniqueness, reject
        // overlap: the contract stated in the tool description.
        let mut spans: Vec<(usize, usize, String)> = Vec::new();
        for (index, edit) in edits.iter().enumerate() {
            let (Some(old), Some(new)) = (
                edit.get("oldText").and_then(|v| v.as_str()),
                edit.get("newText").and_then(|v| v.as_str()),
            ) else {
                return ToolResult::error(
                    call.call_id.clone(),
                    format!("edits[{index}] needs oldText and newText"),
                );
            };
            let mut matches = original.match_indices(old);
            let first = matches.next();
            if first.is_none() {
                return ToolResult::error(
                    call.call_id.clone(),
                    format!("edits[{index}].oldText not found in {path}"),
                );
            }
            if matches.next().is_some() {
                return ToolResult::error(
                    call.call_id.clone(),
                    format!(
                        "edits[{index}].oldText matches more than once in {path}; make it unique"
                    ),
                );
            }
            let (start, text) = first.expect("one match");
            spans.push((start, start + text.len(), new.to_string()));
        }
        spans.sort_by_key(|(start, ..)| *start);
        for pair in spans.windows(2) {
            if pair[0].1 > pair[1].0 {
                return ToolResult::error(
                    call.call_id.clone(),
                    "edits overlap; merge overlapping changes into one edit",
                );
            }
        }
        let mut out = String::with_capacity(original.len());
        let mut cursor = 0usize;
        for (start, end, new) in &spans {
            out.push_str(&original[cursor..*start]);
            out.push_str(new);
            cursor = *end;
        }
        out.push_str(&original[cursor..]);
        match self.ops.write(&target, out.as_bytes()) {
            Ok(()) => {
                self.tracker.record(&target, out.as_bytes());
                ToolResult::ok(
                    call.call_id.clone(),
                    format!("Successfully replaced {} block(s) in {path}.", spans.len()),
                )
            }
            Err(err) => {
                ToolResult::error(call.call_id.clone(), format!("cannot write {path}: {err}"))
            }
        }
    }

    async fn list(&mut self, call: &ToolCall, args: &serde_json::Value) -> ToolResult {
        let path = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        let target = resolve_target(&self.cwd, Path::new(path));
        let entries = match self.ops.list(&target) {
            Ok(entries) => entries,
            Err(err) => {
                return ToolResult::error(
                    call.call_id.clone(),
                    format!("cannot list {path}: {err}"),
                );
            }
        };
        let mut entries = entries;
        entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then(a.rel_path.cmp(&b.rel_path)));
        let mut out = String::new();
        for entry in &entries {
            if entry.is_dir {
                out.push_str(&format!("{}/\n", entry.rel_path));
            } else {
                out.push_str(&format!("{}\n", entry.rel_path));
            }
        }
        if out.is_empty() {
            out.push_str("(empty directory)\n");
        }
        let (content, truncated) = truncate_head(&out, self.result_limit_bytes);
        ToolResult {
            call_id: call.call_id.clone(),
            status: ToolResultStatus::Ok,
            content,
            truncated,
            extras: Default::default(),
        }
    }

    async fn glob(&mut self, call: &ToolCall, args: &serde_json::Value) -> ToolResult {
        let Some(pattern) = args.get("pattern").and_then(|v| v.as_str()) else {
            return ToolResult::error(call.call_id.clone(), "pattern is required");
        };
        let entries = match self.ops.walk(&self.workspace) {
            Ok(entries) => entries,
            Err(err) => {
                return ToolResult::error(call.call_id.clone(), format!("walk failed: {err}"));
            }
        };
        let mut matches: Vec<String> = entries
            .iter()
            .filter(|e| !e.is_dir)
            .map(|e| e.rel_path.clone())
            .filter(|path| glob_match(pattern, path))
            .collect();
        matches.sort();
        if matches.is_empty() {
            return ToolResult::ok(call.call_id.clone(), format!("No matches for {pattern}"));
        }
        let mut out = matches.join("\n");
        out.push('\n');
        let (content, truncated) = truncate_head(&out, self.result_limit_bytes);
        ToolResult {
            call_id: call.call_id.clone(),
            status: ToolResultStatus::Ok,
            content,
            truncated,
            extras: Default::default(),
        }
    }

    async fn grep(&mut self, call: &ToolCall, args: &serde_json::Value) -> ToolResult {
        let Some(pattern) = args.get("pattern").and_then(|v| v.as_str()) else {
            return ToolResult::error(call.call_id.clone(), "pattern is required");
        };
        let literal = args
            .get("literal")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let ignore_case = args
            .get("ignoreCase")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(100) as usize;
        let glob_filter = args
            .get("glob")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let path_arg = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        let root = resolve_target(&self.cwd, Path::new(path_arg));

        let effective = if literal {
            regex::escape(pattern)
        } else {
            pattern.to_string()
        };
        let regex = {
            let mut builder = regex::RegexBuilder::new(&effective);
            builder.case_insensitive(ignore_case);
            match builder.build() {
                Ok(regex) => regex,
                Err(err) => {
                    return ToolResult::error(
                        call.call_id.clone(),
                        format!("invalid pattern: {err}"),
                    );
                }
            }
        };

        let entries = match self.ops.walk(&root) {
            Ok(entries) => entries,
            Err(err) => {
                return ToolResult::error(call.call_id.clone(), format!("walk failed: {err}"));
            }
        };
        let prefix = root.strip_prefix(&self.workspace).unwrap_or(&root);
        let mut matches: Vec<String> = Vec::new();
        let mut truncated = false;
        'files: for entry in entries.iter().filter(|e| !e.is_dir) {
            if let Some(filter) = &glob_filter
                && !glob_match(filter, &entry.rel_path)
            {
                continue;
            }
            let full = root.join(&entry.rel_path);
            let bytes = match self.ops.read(&full) {
                Ok(bytes) => bytes,
                Err(_) => continue,
            };
            if bytes.contains(&0) {
                continue; // binary
            }
            let text = String::from_utf8_lossy(&bytes);
            for (index, line) in text.lines().enumerate() {
                if !regex.is_match(line) {
                    continue;
                }
                if matches.len() >= limit {
                    truncated = true;
                    break 'files;
                }
                let display_path = if prefix.as_os_str().is_empty() {
                    entry.rel_path.clone()
                } else {
                    format!(
                        "{}/{}",
                        prefix.to_string_lossy().replace('\\', "/"),
                        entry.rel_path
                    )
                };
                matches.push(format!(
                    "{display_path}:{}:{}",
                    index + 1,
                    truncate_line(line, 500)
                ));
            }
        }
        if matches.is_empty() {
            return ToolResult::ok(
                call.call_id.clone(),
                format!("No matches for pattern `{pattern}`"),
            );
        }
        let mut out = matches.join("\n");
        out.push('\n');
        if truncated {
            out.push_str(&format!(
                "\n[Match limit of {limit} reached. Narrow the pattern or the path.]\n"
            ));
        }
        let (content, bytes_truncated) = truncate_head(&out, self.result_limit_bytes);
        ToolResult {
            call_id: call.call_id.clone(),
            status: ToolResultStatus::Ok,
            content,
            truncated: truncated || bytes_truncated,
            extras: Default::default(),
        }
    }

    async fn shell(
        &mut self,
        call: &ToolCall,
        args: &serde_json::Value,
        on_output: &mut (dyn FnMut(&[u8]) + Send),
        cancel: &CancelFlag,
    ) -> ToolResult {
        let Some(command) = args.get("command").and_then(|v| v.as_str()) else {
            return ToolResult::error(call.call_id.clone(), "command is required");
        };
        let timeout = args
            .get("timeout")
            .and_then(|v| v.as_u64())
            .map(Duration::from_secs)
            .unwrap_or(self.default_timeout);

        let (outcome, full) = match self
            .ops
            .exec(command, &self.cwd, timeout, on_output, cancel.clone())
            .await
        {
            Ok(pair) => pair,
            Err(err) => {
                return ToolResult::error(
                    call.call_id.clone(),
                    format!("cannot run command: {err}"),
                );
            }
        };
        let (content, truncated) =
            truncate_tail(&String::from_utf8_lossy(&full), self.result_limit_bytes);
        match outcome {
            ExecOutcome::Exit { code: 0 } => ToolResult {
                call_id: call.call_id.clone(),
                status: ToolResultStatus::Ok,
                content: if content.is_empty() {
                    "(no output)".to_string()
                } else {
                    content
                },
                truncated,
                extras: Default::default(),
            },
            ExecOutcome::Exit { code } => ToolResult {
                call_id: call.call_id.clone(),
                status: ToolResultStatus::Error,
                content: format!("{content}\nCommand exited with code {code}"),
                truncated,
                extras: Default::default(),
            },
            ExecOutcome::Timeout => ToolResult {
                call_id: call.call_id.clone(),
                status: ToolResultStatus::Timeout,
                content: format!(
                    "{content}\nCommand timed out after {} seconds",
                    timeout.as_secs()
                ),
                truncated,
                extras: Default::default(),
            },
            ExecOutcome::Cancelled => ToolResult {
                call_id: call.call_id.clone(),
                status: ToolResultStatus::Error,
                content: format!("{content}\nCommand cancelled"),
                truncated,
                extras: Default::default(),
            },
        }
    }
}

/// Resolve a tool-supplied path against the working directory, following
/// the deepest existing ancestor so `..` and symlinks are both honoured
/// before any inside-workspace check (FR-TOOL-3, threat model's traversal
/// scenarios).
pub fn resolve_target(cwd: &Path, path: &Path) -> PathBuf {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    let mut existing = joined.clone();
    let mut remainder: Vec<std::ffi::OsString> = Vec::new();
    loop {
        if let Ok(canonical) = std::fs::canonicalize(&existing) {
            let mut out = canonical;
            for part in remainder.iter().rev() {
                out.push(part);
            }
            return normalize(&out);
        }
        match (existing.file_name(), existing.parent()) {
            (Some(name), Some(parent)) => {
                remainder.push(name.to_os_string());
                existing = parent.to_path_buf();
            }
            _ => return normalize(&joined),
        }
    }
}

fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Whether a resolved path sits inside the workspace root.
pub fn is_inside(path: &Path, root: &Path) -> bool {
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    path.starts_with(&root)
}

/// Truncate at a line boundary, keeping the head (FR-TOOL-7).
fn truncate_head(content: &str, limit: usize) -> (String, bool) {
    if content.len() <= limit {
        return (content.to_string(), false);
    }
    let mut cut = limit;
    while cut > 0 && !content.is_char_boundary(cut) {
        cut -= 1;
    }
    if let Some(newline) = content[..cut].rfind('\n') {
        cut = newline;
    }
    (content[..cut].trim_end().to_string(), true)
}

/// Truncate at a line boundary, keeping the tail: errors and final results
/// live at the end of command output.
fn truncate_tail(content: &str, limit: usize) -> (String, bool) {
    if content.len() <= limit {
        return (content.to_string(), false);
    }
    let mut cut = content.len() - limit;
    while cut < content.len() && !content.is_char_boundary(cut) {
        cut += 1;
    }
    if let Some(newline) = content[cut..].find('\n') {
        cut += newline + 1;
    }
    let kept = &content[cut..];
    let line_total = content.lines().count();
    let shown = kept.lines().count();
    (
        format!(
            "[Output truncated: showing last {shown} of {line_total} lines ({} limit)]\n{kept}",
            format_bytes(limit)
        ),
        true,
    )
}

fn truncate_line(line: &str, max_chars: usize) -> String {
    if line.chars().count() <= max_chars {
        line.to_string()
    } else {
        let head: String = line.chars().take(max_chars).collect();
        format!("{head}... [truncated]")
    }
}

fn format_bytes(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes}B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

/// Glob matching over `/`-separated paths: `*` and `?` stay inside one
/// segment, `**` spans any number of segments (including none).
pub fn glob_match(pattern: &str, path: &str) -> bool {
    let p: Vec<&str> = pattern.split('/').collect();
    let v: Vec<&str> = path.split('/').collect();
    fn walk(p: &[&str], v: &[&str]) -> bool {
        match p.first() {
            None => v.is_empty(),
            Some(&"**") => (0..=v.len()).any(|skip| walk(&p[1..], &v[skip..])),
            Some(segment) => match v.first() {
                None => false,
                Some(&head) => {
                    segment_match(segment.as_bytes(), head.as_bytes()) && walk(&p[1..], &v[1..])
                }
            },
        }
    }
    walk(&p, &v)
}

fn segment_match(pattern: &[u8], value: &[u8]) -> bool {
    let (mut pi, mut vi) = (0usize, 0usize);
    let (mut star, mut backtrack) = (None, 0usize);
    while vi < value.len() {
        if pi < pattern.len() && (pattern[pi] == b'?' || pattern[pi] == value[vi]) {
            pi += 1;
            vi += 1;
        } else if pi < pattern.len() && pattern[pi] == b'*' {
            star = Some(pi);
            backtrack = vi;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            backtrack += 1;
            vi = backtrack;
        } else {
            return false;
        }
    }
    while pi < pattern.len() && pattern[pi] == b'*' {
        pi += 1;
    }
    pi == pattern.len()
}
