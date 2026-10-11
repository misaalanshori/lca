//! The built-in tool specs handed to the model (FR-TOOL-1), with
//! MCP hint annotations on each (#78). Split from `lib.rs` for the
//! workspace file ceiling (gate 11). Behaviour unchanged.

use super::*;
use lca_protocol::ToolSpec;

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

impl ToolExecutor {
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
        let mut specs = vec![
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
        ];
        // #78: the built-ins declare MCP hints like any extension
        // tool; the permission prompt may show them, never decide on
        // them. Read-only tools only read; writers may destroy; the
        // shell may do either and reach the open world.
        for spec in &mut specs {
            let hints = match spec.name.as_str() {
                "read" | "list" | "glob" | "grep" | "skill" => lca_protocol::ToolAnnotations {
                    read_only_hint: Some(true),
                    ..Default::default()
                },
                "write" | "edit" => lca_protocol::ToolAnnotations {
                    destructive_hint: Some(true),
                    ..Default::default()
                },
                "shell" => lca_protocol::ToolAnnotations {
                    destructive_hint: Some(true),
                    open_world_hint: Some(true),
                    ..Default::default()
                },
                _ => continue,
            };
            spec.annotations = Some(hints);
        }
        specs
    }
}
