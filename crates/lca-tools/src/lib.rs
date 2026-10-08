//! Built-in tools: read, write, edit, list, glob, grep, and shell
//! (FR-TOOL-1), behind the swappable backend trait from ADR-0013.
//!
//! No capability gates these tools: they are what defines the model's
//! access to the workspace (ADR-0013's build-time backend category). The
//! permission layer still sees every shell command and every write outside
//! the workspace through [`ToolExecutor::required_permission`] (FR-TOOL-3).

#![deny(unsafe_code)]

mod bridge;
mod capabilities;
mod diff;
mod edit;
mod image;
mod open;
mod ops;
mod paths;
mod process;
mod pty;
pub mod shell;
pub mod skills;

pub use bridge::{BridgeError, bridge_stream};
pub use capabilities::CompletionBackend;
pub use capabilities::{BrowserError, CompletionError};
pub use capabilities::{
    BrowserOpener, Capabilities, CapabilityGrants, Denial, RESOURCE_FILE_MAX_BYTES,
    RESOURCE_PACKAGE_MAX_BYTES, RESOURCE_READ_MAX_BYTES, ResourceSource, STATE_TOTAL_MAX_BYTES,
    STATE_VALUE_MAX_BYTES,
};
pub use open::{UrlLauncher, open_url, url_launchers, windows_url_launcher};
pub use ops::{Entry, ExecOutcome, NativeOps, Stat, ToolOps};
pub use paths::{is_inside, resolve_target, sha256_hex};
pub use process::{TreeChild, read_up_to, spawn_direct, write_all};
pub use pty::PtyChild;
pub use shell::{Kind as ShellKind, Os as ShellOs, Probe as ShellProbe, Shell, Transport};

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use lca_permissions::Action;
use lca_protocol::{ToolCall, ToolResult, ToolResultStatus, ToolSpec};

pub use image::{
    IMAGE_RESIZE_EXTRA, IMAGE_VISION_EXTRA, ImagePolicy, ImageResize, ImageVision, NO_VISION_NOTE,
};

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

/// Pi's line budget for `read` (`DEFAULT_MAX_LINES` in pi's
/// `tools/truncate.ts`): at most this many lines reach the model from one
/// read, alongside the byte budget, whichever hits first (#39).
pub const READ_MAX_LINES: usize = 2000;

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
    /// The timeout a `shell` call runs under when it names none (gh #40):
    /// `None` runs until completion or cancellation (pi's interactive
    /// default); `Some` keeps a backstop where nobody can cancel.
    default_timeout: Option<Duration>,
    image_policy: ImagePolicy,
    /// Whether `edit` demands a prior fresh `read` (gh #117): on keeps
    /// the historical staleness guard, off is pi parity (blind edits
    /// allowed). The host sets it from `tool.edit_requires_read`.
    edit_requires_read: bool,
    tracker: ReadTracker,
    /// Where over-limit output is spilled, content-addressed, when a
    /// session is attached (`<session>/attachments`); `None` keeps
    /// today's truncate-and-drop behavior (tests, one-shot use).
    spill_dir: Option<PathBuf>,
    /// The host's skill sources for the `skill` tool (gh #43); `None`
    /// answers "no skills" rather than reading somewhere unconfigured.
    skills_roots: Option<crate::skills::SkillsRoots>,
}

impl ToolExecutor {
    /// The backend's resolved shell, when it owns one (ADR-0041): the tool
    /// description and `/settings` read it from here.
    pub fn resolved_shell(&self) -> Option<&Shell> {
        self.ops.shell()
    }

    /// The shell resolution error, when the backend is broken.
    pub fn resolved_shell_error(&self) -> Option<&str> {
        self.ops.shell_error()
    }

    /// Build an executor over a backend.
    pub fn new(
        ops: Arc<dyn ToolOps>,
        workspace: PathBuf,
        cwd: PathBuf,
        result_limit_bytes: usize,
        default_timeout: Option<Duration>,
    ) -> ToolExecutor {
        ToolExecutor {
            ops,
            workspace,
            cwd,
            result_limit_bytes,
            default_timeout,
            image_policy: ImagePolicy::unknown(),
            edit_requires_read: true,
            tracker: ReadTracker::default(),
            spill_dir: None,
            skills_roots: None,
        }
    }

    /// What the active model can do with images (#39). Unknown by
    /// default: images pass through at original size. Set per turn from
    /// the resolved model's metadata.
    pub fn set_image_policy(&mut self, policy: ImagePolicy) {
        self.image_policy = policy;
    }

    /// Point the executor at the host's skill sources, so the `skill`
    /// tool loads bodies through the same files the merge reads (gh #43).
    /// Set by the front end alongside the agent config's own roots.
    pub fn set_skills_roots(&mut self, roots: Option<crate::skills::SkillsRoots>) {
        self.skills_roots = roots;
    }

    /// Point the executor at the session's attachment directory. Set by the
    /// agent loop per turn (the session owns the directory).
    pub fn set_spill_dir(&mut self, dir: Option<PathBuf>) {
        self.spill_dir = dir;
    }

    /// Set the read-before-edit gate (gh #117). `ToolExecutor::new`
    /// keeps it on (the historical behavior every existing row pins);
    /// the product default is off (pi parity) and arrives here through
    /// the host's config.
    pub fn set_edit_requires_read(&mut self, required: bool) {
        self.edit_requires_read = required;
    }

    /// Write `full` under the spill dir by content hash, returning the hash.
    /// `None` when no session is attached or the write fails; the caller
    /// then keeps today's truncate-only behavior.
    fn spill(&self, full: &str) -> Option<String> {
        let dir = self.spill_dir.as_ref()?;
        let digest = sha256_hex(full.as_bytes());
        let path = dir.join(&digest);
        if path.exists() {
            return Some(digest);
        }
        if std::fs::create_dir_all(dir).is_err() {
            return None;
        }
        let temp = path.with_extension("tmp");
        std::fs::write(&temp, full).ok()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o600));
        }
        std::fs::rename(&temp, &path).ok()?;
        Some(digest)
    }

    /// Build a result, spilling the untruncated text when the display was
    /// cut (FR-TOOL-7). The hash rides in `extras` (the ABI's non-structural
    /// extension point) and the model gets a one-line stub naming it.
    /// The spill path rides the structured fields (gh #40), so the
    /// `--json` envelope and the session record name the file instead of
    /// burying it in prose.
    fn spilled(
        &self,
        call_id: &str,
        full: &str,
        mut content: String,
        truncated: bool,
        status: ToolResultStatus,
        exit_code: Option<i32>,
    ) -> ToolResult {
        let attachment = truncated.then(|| self.spill(full)).flatten();
        if let Some(hash) = &attachment {
            content.push_str(&format!(
                "\n[full output: {} bytes, attachment {hash}]",
                full.len()
            ));
        }
        let full_output_path = attachment.as_deref().and_then(|hash| {
            self.spill_dir
                .as_ref()
                .map(|dir| dir.join(hash).to_string_lossy().into_owned())
        });
        let mut result = ToolResult {
            call_id: call_id.to_string(),
            status,
            content,
            truncated,
            images: Vec::new(),
            extras: Default::default(),
            exit_code,
            full_output_path,
            nested: Vec::new(),
        };
        if let Some(hash) = attachment {
            result.extras.insert("attachment".to_string(), hash);
        }
        result
    }

    /// The workspace root this executor serves.
    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    /// The tool specs handed to the model (FR-TOOL-1). `shell` is the
    /// resolved interpreter, so the `shell` tool's description names it and
    /// its dialect (ADR-0041): the owner's log shows an agent burning ten
    /// calls working out which shell it was in.
    pub fn specs(shell: Option<&Shell>) -> Vec<ToolSpec> {
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
                    exposure: lca_protocol::ToolExposure::Direct,
                    namespace: None,
                    annotations: None,
                    extras: Default::default(),
                }
            };
        vec![
            spec(
                "skill",
                "Load a skill's full instructions by name (the system prompt advertises the catalog, not the bodies). Restricted skills answer here only through /skill:name.",
                serde_json::json!({
                    "name": {"type": "string", "description": "Skill name from the catalog"},
                }),
                &["name"],
            ),
            spec(
                "read",
                "Read a file. Returns content with line numbers. Output is truncated to 2000 lines or the result size limit (whichever is hit first). Use offset (1-indexed) and limit to page through large files; when you need the full file, continue with offset until complete.",
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
                "Make precise file edits with exact text replacement. Every edits[].oldText must be unique in the original file and must not overlap another edit. Each edit matches the original file, not the result of earlier edits. When exact text fails, a fuzzy fallback matches after Unicode normalization (quotes, dashes, spacing, compatibility forms); several fuzzy hits are an error, not a guess. CRLF files match LF text and keep CRLF; a UTF-8 BOM is preserved. Files that are not valid UTF-8 are refused, never rewritten.",
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
                "List a directory (also callable as `ls`). Entries sort alphabetically with '/' suffixing directories, dotfiles included.",
                serde_json::json!({
                    "path": {"type": "string", "description": "Directory relative to the workspace (default: .)"},
                    "limit": {"type": "integer", "description": "Maximum entries to return (default 500)"},
                }),
                &[],
            ),
            spec(
                "glob",
                "Find files by glob pattern (also callable as `find`). Supports *, ?, ** (any depth), character classes. Honors .gitignore. Results are workspace-relative paths.",
                serde_json::json!({
                    "pattern": {"type": "string", "description": "Glob pattern such as src/**/*.rs"},
                    "path": {"type": "string", "description": "Directory to search in (default: workspace root)"},
                    "limit": {"type": "integer", "description": "Maximum results to return (default 1000)"},
                }),
                &["pattern"],
            ),
            spec(
                "grep",
                "Search file contents with a regular expression. Returns path:line:match lines, with path-line- context lines when context is set. Honors .gitignore; skips .git, target, and node_modules.",
                serde_json::json!({
                    "pattern": {"type": "string", "description": "Regular expression, or literal text when literal is true"},
                    "path": {"type": "string", "description": "Directory or file to search (default: workspace root)"},
                    "glob": {"type": "string", "description": "Filter files by glob, e.g. '*.rs'"},
                    "ignoreCase": {"type": "boolean"},
                    "literal": {"type": "boolean", "description": "Treat the pattern as a literal string"},
                    "context": {"type": "integer", "description": "Lines to show before and after each match (default 0)"},
                    "limit": {"type": "integer", "description": "Maximum matches to return (default 100)"},
                }),
                &["pattern"],
            ),
            spec(
                "shell",
                &shell_description(shell),
                serde_json::json!({
                    "command": {"type": "string"},
                    "timeout": {"type": "integer", "description": "Seconds before the command is killed (optional; overrides the default for this call only)"},
                }),
                &["command"],
            ),
        ]
    }

    /// What must be approved before this call runs (FR-TOOL-3): every shell
    /// command, and any tool call whose resolved target leaves the workspace -
    /// a write, or a read/list/search.
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
            // FR-TOOL-3 reads "a tool call targets a path outside the
            // workspace root", not just a write; a read/list/grep outside the
            // workspace asks first too. `glob` walks only the workspace, so it
            // owns no out-of-workspace path.
            "read" | "list" | "grep" => {
                let path = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
                let target = resolve_target(&self.cwd, Path::new(path));
                (!is_inside(&target, &self.workspace)).then_some(Action::ReadPath { path: target })
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
            "skill" => self.skill(call, &args).await,
            "write" => self.write(call, &args).await,
            "edit" => self.edit(call, &args).await,
            "list" => self.list(call, &args).await,
            "glob" => self.glob(call, &args).await,
            "grep" => self.grep(call, &args).await,
            "shell" => self.shell(call, &args, on_output, cancel).await,
            other => ToolResult::error(call.call_id.clone(), format!("unknown tool `{other}`")),
        }
    }

    /// The `skill` tool (gh #43): load one skill's full instructions by
    /// name, through the same files the merge reads. No permission gate:
    /// the merge already reads these files every turn without asking,
    /// so gating the lazy load would be theater. A restricted skill
    /// (`disable-model-invocation`) answers here only through
    /// `/skill:name`, never through this tool.
    async fn skill(&mut self, call: &ToolCall, args: &serde_json::Value) -> ToolResult {
        let Some(name) = args.get("name").and_then(|v| v.as_str()) else {
            return ToolResult::error(call.call_id.clone(), "name is required");
        };
        let Some(roots) = self.skills_roots.as_ref() else {
            return ToolResult::error(
                call.call_id.clone(),
                "skills are not configured on this host",
            );
        };
        let found = crate::skills::collect(roots)
            .into_iter()
            .find(|skill| skill.name == name);
        match found {
            None => ToolResult::error(call.call_id.clone(), format!("no skill named `{name}`")),
            Some(skill) if !skill.model_invocable => ToolResult::error(
                call.call_id.clone(),
                format!("skill `{name}` is available only through /skill:{name}"),
            ),
            Some(skill) => ToolResult::ok(
                call.call_id.clone(),
                format!(
                    "[skill {} from {}]\n{}",
                    name,
                    skill.source.label(),
                    skill.body
                ),
            ),
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
        // R5: an image file comes back as image content, not lossy UTF-8; the
        // interface renders it through the terminal's graphics ladder.
        // (#39: per-model vision and resize ride the executor's image
        // policy; unknown models behave exactly as before.)
        if let Some(media_type) = lca_protocol::sniff_image_media_type(&bytes) {
            return crate::image::read_image_result(
                &self.image_policy,
                &call.call_id,
                media_type,
                bytes,
            );
        }
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
            Some(limit) => (offset.saturating_sub(1)).saturating_add(limit).min(total),
            None => total,
        };
        // #39: pi's line budget rides on top of the byte budget,
        // whichever hits first. A user `limit` smaller than both keeps
        // its own continuation note below, unchanged.
        let line_capped_end = (offset.saturating_sub(1))
            .saturating_add(READ_MAX_LINES)
            .min(total);
        let line_capped = line_capped_end < end;
        // The spill holds the whole requested window: the line budget
        // cuts the display, never the recoverable record.
        let wanted_end = end;
        let end = end.min(line_capped_end);
        // #39: pi's first-line advice. One line over the whole budget is
        // not content to truncate; point at the shell instead.
        if lines[offset - 1].len() > self.result_limit_bytes {
            let size = format_bytes(lines[offset - 1].len());
            let limit = format_bytes(self.result_limit_bytes);
            let out = format!(
                "[Line {offset} is {size}, exceeds {limit} limit. Use shell: sed -n '{offset}p' {path} | head -c {limit}]"
            );
            return self.spilled(
                &call.call_id,
                &numbered_window(&lines, offset, wanted_end),
                out,
                true,
                ToolResultStatus::Ok,
                None,
            );
        }
        let numbered = numbered_window(&lines, offset, end);
        let (content, byte_truncated) = truncate_head(&numbered, self.result_limit_bytes);
        let mut out = content;
        let truncated = line_capped || byte_truncated;
        if line_capped {
            let shown_lines = out.lines().count();
            let last = (offset.saturating_sub(1)).saturating_add(shown_lines);
            out.push_str(&format!(
                "\n[Showing lines {offset}-{last} of {total} ({READ_MAX_LINES}-line limit). Use offset={} to continue.]",
                last + 1
            ));
        } else if byte_truncated {
            let shown_lines = out.lines().count();
            let last = (offset.saturating_sub(1)).saturating_add(shown_lines);
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
        self.spilled(
            &call.call_id,
            &numbered_window(&lines, offset, wanted_end),
            out,
            truncated,
            ToolResultStatus::Ok,
            None,
        )
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

    async fn list(&mut self, call: &ToolCall, args: &serde_json::Value) -> ToolResult {
        let path = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(500) as usize;
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
        // pi's `ls` order (gh #120): alphabetical, case-insensitive,
        // files and directories interleaved, dotfiles in place.
        let mut entries = entries;
        entries.sort_by_key(|a| a.rel_path.to_lowercase());
        let mut out = String::new();
        for entry in entries.iter().take(limit) {
            if entry.is_dir {
                out.push_str(&format!("{}/\n", entry.rel_path));
            } else {
                out.push_str(&format!("{}\n", entry.rel_path));
            }
        }
        let capped = entries.len() > limit;
        if out.is_empty() {
            out.push_str("(empty directory)\n");
        }
        if capped {
            out.push_str(&format!(
                "\n[{limit} entries limit reached. Use limit={} for more]\n",
                limit * 2
            ));
        }
        let (content, truncated) = truncate_head(&out, self.result_limit_bytes);
        self.spilled(
            &call.call_id,
            &out,
            content,
            truncated,
            ToolResultStatus::Ok,
            None,
        )
    }

    async fn glob(&mut self, call: &ToolCall, args: &serde_json::Value) -> ToolResult {
        let Some(pattern) = args.get("pattern").and_then(|v| v.as_str()) else {
            return ToolResult::error(call.call_id.clone(), "pattern is required");
        };
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(1000) as usize;
        let path_arg = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        let root = resolve_target(&self.cwd, Path::new(path_arg));
        let matcher = match glob_matcher(pattern) {
            Ok(matcher) => matcher,
            Err(err) => {
                return ToolResult::error(call.call_id.clone(), format!("invalid pattern: {err}"));
            }
        };
        let entries = match self.ops.walk(&root) {
            Ok(entries) => entries,
            Err(err) => {
                return ToolResult::error(call.call_id.clone(), format!("walk failed: {err}"));
            }
        };
        let prefix = root.strip_prefix(&self.workspace).unwrap_or(&root);
        let mut matches: Vec<String> = entries
            .iter()
            .filter(|e| !e.is_dir)
            .filter(|e| matcher.is_match(&e.rel_path))
            .map(|e| {
                if prefix.as_os_str().is_empty() {
                    e.rel_path.clone()
                } else {
                    format!(
                        "{}/{}",
                        prefix.to_string_lossy().replace('\\', "/"),
                        e.rel_path
                    )
                }
            })
            .collect();
        matches.sort();
        if matches.is_empty() {
            return ToolResult::ok(call.call_id.clone(), format!("No matches for {pattern}"));
        }
        let capped = matches.len() > limit;
        let mut out = matches
            .into_iter()
            .take(limit)
            .collect::<Vec<_>>()
            .join("\n");
        out.push('\n');
        if capped {
            out.push_str(&format!(
                "\n[{limit} results limit reached. Use limit={} for more, or refine pattern]\n",
                limit * 2
            ));
        }
        let (content, truncated) = truncate_head(&out, self.result_limit_bytes);
        self.spilled(
            &call.call_id,
            &out,
            content,
            truncated,
            ToolResultStatus::Ok,
            None,
        )
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
        let glob_filter = match args.get("glob").and_then(|v| v.as_str()) {
            Some(filter) => match glob_matcher(filter) {
                Ok(matcher) => Some(matcher),
                Err(err) => {
                    return ToolResult::error(call.call_id.clone(), format!("invalid glob: {err}"));
                }
            },
            None => None,
        };
        let context = args.get("context").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let path_arg = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        let target = resolve_target(&self.cwd, Path::new(path_arg));
        // A file path greps that one file; walking it as a directory failed
        // with "Not a directory".
        let (root, entries) = if target.is_file() {
            let parent = target
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| self.workspace.clone());
            let name = target
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let len = target.metadata().map(|meta| meta.len()).unwrap_or(0);
            (
                parent,
                vec![Entry {
                    rel_path: name,
                    is_dir: false,
                    len,
                }],
            )
        } else {
            match self.ops.walk(&target) {
                Ok(entries) => (target, entries),
                Err(err) => {
                    return ToolResult::error(call.call_id.clone(), format!("walk failed: {err}"));
                }
            }
        };

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

        let prefix = root.strip_prefix(&self.workspace).unwrap_or(&root);
        let mut matches: Vec<String> = Vec::new();
        let mut match_count = 0usize;
        let mut truncated = false;
        'files: for entry in entries.iter().filter(|e| !e.is_dir) {
            if let Some(filter) = &glob_filter
                && !filter.is_match(&entry.rel_path)
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
            let display_path = if prefix.as_os_str().is_empty() {
                entry.rel_path.clone()
            } else {
                format!(
                    "{}/{}",
                    prefix.to_string_lossy().replace('\\', "/"),
                    entry.rel_path
                )
            };
            let lines: Vec<&str> = text.lines().collect();
            let mut hits: Vec<usize> = Vec::new();
            for (index, line) in lines.iter().enumerate() {
                if !regex.is_match(line) {
                    continue;
                }
                if match_count >= limit {
                    truncated = true;
                    break 'files;
                }
                match_count += 1;
                hits.push(index);
            }
            // pi's context-block shape: `path:line:` marks the hit,
            // `path-line-` marks context. Adjacent hits repeat their
            // shared context lines, exactly like pi's per-match blocks.
            for hit in &hits {
                let start = hit.saturating_sub(context);
                let end = (*hit + context).min(lines.len().saturating_sub(1));
                for (lineno, line) in lines.iter().enumerate().take(end + 1).skip(start) {
                    let rendered = truncate_line(line, 500);
                    if lineno == *hit {
                        matches.push(format!("{display_path}:{}: {rendered}", lineno + 1));
                    } else {
                        matches.push(format!("{display_path}-{}- {rendered}", lineno + 1));
                    }
                }
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
        self.spilled(
            &call.call_id,
            &out,
            content,
            truncated || bytes_truncated,
            ToolResultStatus::Ok,
            None,
        )
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
        // A per-call `timeout` overrides the configured default for this
        // call (gh #40, pi's `bash` parameter of the same name). Zero or
        // negative is rejected like pi rejects it: a timeout that fires
        // immediately helps nothing and hides the mistake.
        let timeout = match args.get("timeout").and_then(|v| v.as_u64()) {
            Some(0) => {
                return ToolResult::error(
                    call.call_id.clone(),
                    "timeout must be at least 1 second",
                );
            }
            Some(secs) => Some(Duration::from_secs(secs)),
            None => self.default_timeout,
        };

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
        let full_text = String::from_utf8_lossy(&full);
        let (content, truncated) = truncate_tail(&full_text, self.result_limit_bytes);
        let (status, content, exit_code) = match outcome {
            ExecOutcome::Exit { code: 0 } => (
                ToolResultStatus::Ok,
                if content.is_empty() {
                    "(no output)".to_string()
                } else {
                    content
                },
                Some(0),
            ),
            ExecOutcome::Exit { code } => (
                ToolResultStatus::Error,
                format!("{content}\nCommand exited with code {code}"),
                Some(code),
            ),
            ExecOutcome::Timeout => (
                ToolResultStatus::Timeout,
                match timeout {
                    Some(limit) => format!(
                        "{content}\nCommand timed out after {} seconds",
                        limit.as_secs()
                    ),
                    None => format!("{content}\nCommand timed out"),
                },
                None,
            ),
            ExecOutcome::Cancelled => (
                ToolResultStatus::Error,
                format!("{content}\nCommand cancelled"),
                None,
            ),
        };
        self.spilled(
            &call.call_id,
            &full_text,
            content,
            truncated,
            status,
            exit_code,
        )
    }
}

/// The `shell` tool's model-facing description (ADR-0041): what the tool
/// does, plus the resolved interpreter, its dialect, and the fidelity
/// guarantee. `None` when the backend owns no interpreter (the web target's
/// host-delegated backend).
fn shell_description(shell: Option<&Shell>) -> String {
    let base = "Run a command in the workspace directory and return its output (also callable as `bash`). \
                Output streams as it runs; the tail is kept when too large. \
                Optionally set a timeout in seconds.";
    match shell {
        Some(shell) => format!("{base} {}", shell.describe()),
        None => base.to_string(),
    }
}

/// Resolve a tool-supplied path against the working directory, following
/// the deepest existing ancestor so `..` and symlinks are both honoured
/// before any inside-workspace check (FR-TOOL-3, threat model's traversal
/// scenarios).
/// Truncate at a line boundary, keeping the head (FR-TOOL-7).
/// The offset window with line numbers, as the model sees it.
fn numbered_window(lines: &[&str], offset: usize, end: usize) -> String {
    let mut numbered = String::new();
    for (index, line) in lines[offset - 1..end].iter().enumerate() {
        numbered.push_str(&format!("{:6}\t{line}\n", offset + index));
    }
    numbered
}

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

/// Pi-name aliases for built-in tools (gh #119 decision): the model may
/// call these names; everything downstream (permission, dispatch, records)
/// sees the canonical name. One entry per tool in the schema (no
/// tool-count bloat); the descriptions name the aliases. Records always
/// store the canonical name, so logs, the reserved-name registry
/// (FR-EXT-11), and headless consumers never break.
pub fn canonical_tool_name(name: &str) -> &str {
    match name {
        "find" => "glob",
        "ls" => "list",
        "bash" => "shell",
        _ => name,
    }
}

/// A compiled glob with `*`-stays-in-segment semantics (gh #119,
/// pi's `find` contract): `*` and `?` never cross `/`, `**` spans any
/// number of segments including none. Character classes and alternates
/// ride along; the hand-rolled matcher they replace knew neither.
pub fn glob_matcher(pattern: &str) -> Result<globset::GlobMatcher, String> {
    globset::GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .map(|glob| glob.compile_matcher())
        .map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::{glob_matcher, is_inside, resolve_target, truncate_head, truncate_tail};
    use std::path::Path;

    #[test]
    fn head_truncation_keeps_a_clean_line_boundary() {
        let (text, truncated) = truncate_head("a\nbb\nccc\n", 5);
        assert!(truncated);
        assert!(text.len() <= 5);
        assert!(!text.ends_with('\n'));
    }

    #[test]
    fn tail_truncation_keeps_the_end() {
        let content = format!("{}\nKEEP\n", "x".repeat(100));
        let (text, truncated) = truncate_tail(&content, 10);
        assert!(truncated);
        assert!(text.contains("KEEP"), "{text}");
        assert!(text.contains("truncated"));
    }

    #[test]
    fn glob_matches_segments_and_double_star() {
        let yes = |pattern: &str, path: &str| {
            glob_matcher(pattern).expect("valid pattern").is_match(path)
        };
        assert!(yes("src/**/*.rs", "src/a/b/main.rs"));
        assert!(yes("**/*.rs", "main.rs"));
        assert!(yes("*.rs", "main.rs"));
        assert!(!yes("*.rs", "src/main.rs"));
        assert!(yes("a?c", "abc"));
        assert!(!yes("a?c", "ac"));
    }

    #[test]
    fn inside_is_component_wise_not_a_string_prefix() {
        assert!(is_inside(Path::new("/ws/src/x"), Path::new("/ws")));
        assert!(!is_inside(Path::new("/ws-evil/x"), Path::new("/ws")));
    }

    #[test]
    fn resolve_target_normalizes_parent_traversal() {
        let root = lca_testkit::scratch_path("lca-resolve");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("ws")).expect("mkdir");
        let resolved = resolve_target(&root.join("ws"), Path::new("../outside.txt"));
        let canonical_root = std::fs::canonicalize(&root).expect("canonical");
        assert_eq!(resolved, canonical_root.join("outside.txt"));
        std::fs::remove_dir_all(&root).ok();
    }
}
