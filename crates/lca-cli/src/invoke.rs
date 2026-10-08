//! CLI invocation helpers (phase 4A): piped stdin, `@file` expansion,
//! tool/session/resource flag resolution. Pure shapes live here so the
//! route and guard tests pin them without a terminal; `run` does the
//! I/O (stdin reads, file expansion) before routing.

use std::path::{Path, PathBuf};

#[cfg(test)]
#[path = "invoke_tests.rs"]
mod tests;

/// The run's tool selection (gh #67, pi's `-t`/`-xt`/`-nbt`/`-nt`): an
/// allowlist replaces the default, `+`/`-` deltas modify it, excludes
/// and `--no-builtin-tools` subtract after everything else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolSelection {
    /// Names for `set_active_tools` (the registry ignores unknowns,
    /// reported separately for the warning).
    pub extension: Vec<String>,
    /// The built-in allowlist (`None` is every built-in).
    pub builtin: Option<std::collections::HashSet<String>>,
    /// Whether the `tool_search` offer shows (gh #67): pinned on when
    /// the flag names it, off when any selection flag does not.
    pub tool_search: Option<bool>,
    /// Whether any selection flag was given (untouched runs skip the
    /// registry writes, so the turn records no spurious change).
    pub touched: bool,
}

/// Match one tool entry (gh #67, pi's shape): `*` patterns glob,
/// anything else matches the exact name.
fn tool_entry_matches(entry: &str, name: &str) -> bool {
    let entry = entry.trim();
    if entry.contains('*') {
        lca_permissions::wildcard_match(entry, name)
    } else {
        entry == name
    }
}

/// Resolve the tool flags against the registered and built-in names
/// (gh #67). Returns the selection plus unknown entries for the
/// warning (a typo warns, it does not fail the run).
#[allow(clippy::too_many_arguments)]
pub fn select_tools(
    registered: &[String],
    default_active: &[String],
    builtin: &[String],
    tools: Option<&str>,
    exclude: Option<&str>,
    no_builtin: bool,
    no_tools: bool,
) -> (ToolSelection, Vec<String>) {
    let untouched = ToolSelection {
        extension: Vec::new(),
        builtin: None,
        tool_search: None,
        touched: false,
    };
    if tools.is_none() && exclude.is_none() && !no_builtin && !no_tools {
        return (untouched, Vec::new());
    }
    let mut unknown = Vec::new();
    let mut selected: Vec<String> = if no_tools {
        Vec::new()
    } else if let Some(list) = tools {
        let entries: Vec<&str> = list
            .split(',')
            .map(str::trim)
            .filter(|e| !e.is_empty())
            .collect();
        if entries
            .iter()
            .all(|e| e.starts_with('+') || e.starts_with('-'))
        {
            // Deltas modify the default (pi's shape): exact names only.
            let mut selected = default_active.to_vec();
            for entry in entries {
                if let Some(add) = entry.strip_prefix('+') {
                    if registered.contains(&add.to_string())
                        || builtin.contains(&add.to_string())
                        || add == "tool_search"
                    {
                        if !selected.contains(&add.to_string()) {
                            selected.push(add.to_string());
                        }
                    } else {
                        unknown.push(add.to_string());
                    }
                } else if let Some(remove) = entry.strip_prefix('-') {
                    selected.retain(|name| name != remove);
                }
            }
            selected
        } else {
            // An allowlist replaces the default; patterns glob.
            let mut selected = Vec::new();
            for entry in entries {
                let mut hit = false;
                for name in registered.iter().chain(builtin.iter()) {
                    if tool_entry_matches(entry, name) {
                        if !selected.contains(name) {
                            selected.push(name.clone());
                        }
                        hit = true;
                    }
                }
                if entry == "tool_search" {
                    hit = true;
                    if !selected.contains(&"tool_search".to_string()) {
                        selected.push("tool_search".to_string());
                    }
                }
                if !hit {
                    unknown.push(entry.to_string());
                }
            }
            selected
        }
    } else {
        default_active.to_vec()
    };
    if no_builtin {
        selected.retain(|name| !builtin.contains(name));
    }
    if let Some(list) = exclude {
        let entries: Vec<&str> = list
            .split(',')
            .map(str::trim)
            .filter(|e| !e.is_empty())
            .collect();
        selected.retain(|name| !entries.iter().any(|entry| tool_entry_matches(entry, name)));
    }
    let builtin_set: std::collections::HashSet<String> = selected
        .iter()
        .filter(|name| builtin.contains(name))
        .cloned()
        .collect();
    let tool_search = if no_tools {
        Some(false)
    } else if tools.is_some_and(|list| {
        list.split(',').any(|entry| {
            let entry = entry.trim().trim_start_matches('+');
            entry == "tool_search"
        })
    }) {
        Some(true)
    } else if tools.is_some() || exclude.is_some() || no_builtin {
        Some(false)
    } else {
        None
    };
    let mut extension: Vec<String> = selected
        .iter()
        .filter(|name| *name != "tool_search")
        .cloned()
        .collect();
    extension.sort();
    unknown.sort();
    unknown.dedup();
    (
        ToolSelection {
            extension,
            builtin: Some(builtin_set),
            tool_search,
            touched: true,
        },
        unknown,
    )
}

/// Read stdin, split `@path` tokens, and expand them (gh #71): the
/// I/O half of invocation prep. Returns the routing struct plus the
/// `@file` images for attach staging; a missing `@path` is a usage
/// error. Never called in RPC mode (which owns stdin).
pub fn prepare_invocation(
    cli: &crate::Cli,
    cwd: &Path,
    rpc_mode: bool,
) -> Result<(super::session_cmds::Invocation, Vec<PathBuf>), i32> {
    use std::io::IsTerminal as _;
    let stdin_piped = !std::io::stdin().is_terminal();
    let stdout_piped = !std::io::stdout().is_terminal();
    let stdin_text = read_piped_stdin(rpc_mode);
    // `@path` tokens split here for expansion; routing re-plans with
    // the expanded text, so the field below is read, not repeated.
    let at_files = plan_messages(
        cli.prompt.as_deref(),
        cli.print.as_deref(),
        &cli.messages,
        stdin_text.as_deref(),
        "",
    )
    .files;
    let (file_text, file_images) = match expand_at_files(&at_files, cwd) {
        Ok(expanded) => expanded,
        Err(err) => {
            eprintln!("{err}");
            return Err(crate::exit::USAGE);
        }
    };
    Ok((
        super::session_cmds::Invocation {
            stdin_text,
            file_text,
            file_images: file_images.clone(),
            stdin_piped,
            stdout_piped,
        },
        file_images,
    ))
}

/// Piped stdin and `@file` text cap (gh #71): the read tool's own file
/// bound, so one pasted diff cannot eat the context window whole.
const INPUT_MAX_BYTES: u64 = lca_tools::RESOURCE_FILE_MAX_BYTES;

/// The marker a truncated input carries (gh #71).
const TRUNCATED_MARKER: &str = "… (input truncated at 1 MiB)";

/// A planned first message (gh #71, pi's `buildInitialMessage`): stdin,
/// then `@file` text, then the first CLI message joined with no
/// separator, with the remaining messages riding behind.
pub struct PlannedMessages {
    /// `@path` arguments in order (the `@` stripped).
    pub files: Vec<String>,
    /// The composed first message, if any part exists.
    pub first: Option<String>,
    /// The messages after the first, in order.
    pub rest: Vec<String>,
}

/// Split `@path` tokens from positionals and compose the first message
/// (gh #71): any token starting with `@` is a path (pi's shape, even a
/// lone `@` - the expander refuses it out loud). Only positionals
/// split; `-p`/`--prompt` values stay literal, like pi's.
/// Split `@path` tokens out of positionals (gh #71): any token
/// starting with `@` is a path with the `@` stripped.
pub fn split_at_files(positionals: &[String]) -> (Vec<String>, Vec<String>) {
    let mut files = Vec::new();
    let mut prompts = Vec::new();
    for positional in positionals {
        if let Some(path) = positional.strip_prefix('@') {
            files.push(path.to_string());
        } else {
            prompts.push(positional.clone());
        }
    }
    (files, prompts)
}

pub fn plan_messages(
    legacy: Option<&str>,
    print: Option<&str>,
    positionals: &[String],
    stdin_text: Option<&str>,
    file_text: &str,
) -> PlannedMessages {
    let (files, prompts) = split_at_files(positionals);
    let mut all = Vec::new();
    if let Some(legacy) = legacy {
        all.push(legacy.to_string());
    }
    if let Some(first) = print
        && !first.is_empty()
    {
        all.push(first.to_string());
    }
    all.extend(prompts);
    let mut head = String::new();
    if let Some(stdin) = stdin_text {
        head.push_str(stdin);
    }
    head.push_str(file_text);
    let first = all.first();
    if let Some(first) = first {
        head.push_str(first);
    }
    let composed = (!head.is_empty() || first.is_some()).then_some(head);
    PlannedMessages {
        files,
        first: composed,
        rest: all.into_iter().skip(1).collect(),
    }
}

#[allow(clippy::too_many_arguments)] // thin entry seam: every arg is used once, at one call site.
pub fn interactive(
    cwd: &Path,
    resume: Option<&str>,
    resume_picker: bool,
    yolo: bool,
    model: Option<&str>,
    initial: &[String],
    initial_attachments: &[std::path::PathBuf],
    allow_host: &[String],
    flags: &crate::CliFlags,
) -> i32 {
    // Wired to `lca-tui`; kept as one seam so the headless
    // contract stays independently testable.
    match crate::tui::run(
        cwd,
        resume,
        resume_picker,
        yolo,
        model,
        initial,
        initial_attachments,
        allow_host,
        flags,
    ) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("error: {err:#}");
            crate::exit::INTERNAL
        }
    }
}

/// Apply the run's tool selection to a fresh registry (gh #67):
/// extension names through `set_active_tools`, built-ins through
/// their own set, `tool_search` pinned by explicit naming. No flags
/// means no writes (the turn records no spurious change). Returns
/// warnings for unknown entries (a typo warns, it does not fail).
pub fn apply_tool_selection(
    registry: &lca_core::ExtensionRegistry,
    flags: &crate::CliFlags,
) -> Vec<String> {
    let registered: Vec<String> = registry
        .tool_specs()
        .iter()
        .map(|spec| spec.name.clone())
        .collect();
    let default_active = registry.active_tools();
    let builtin: Vec<String> = lca_core::BUILTIN_TOOLS
        .iter()
        .map(|name| name.to_string())
        .collect();
    let (selection, unknown) = select_tools(
        &registered,
        &default_active,
        &builtin,
        flags.tools.as_deref(),
        flags.exclude_tools.as_deref(),
        flags.no_builtin_tools,
        flags.no_tools,
    );
    if !selection.touched {
        return Vec::new();
    }
    let mut warnings = Vec::new();
    for name in &unknown {
        warnings.push(format!("warning: --tools: unknown tool `{name}`"));
    }
    let _ = registry.set_active_tools(&selection.extension);
    registry.set_builtin_active(selection.builtin);
    registry.set_tool_search(selection.tool_search);
    warnings
}

/// Read piped stdin (gh #71, pi's `readPipedStdin`): `None` on a
/// terminal, on empty input, or when `skip` (RPC mode owns stdin).
/// Overlong input truncates at the bound with the marker.
pub fn read_piped_stdin(skip: bool) -> Option<String> {
    use std::io::{IsTerminal as _, Read as _};
    if skip || std::io::stdin().is_terminal() {
        return None;
    }
    let mut bytes = Vec::new();
    let limit = INPUT_MAX_BYTES.saturating_add(1);
    if std::io::stdin()
        .take(limit)
        .read_to_end(&mut bytes)
        .is_err()
    {
        return None;
    }
    let truncated = (bytes.len() as u64) > INPUT_MAX_BYTES;
    if truncated {
        bytes.truncate(INPUT_MAX_BYTES as usize);
    }
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    // pi trims piped input; an empty pipe contributes nothing.
    text = text.trim().to_string();
    if text.is_empty() {
        return None;
    }
    if truncated {
        text.push('\n');
        text.push_str(TRUNCATED_MARKER);
    }
    Some(text)
}

/// Whether `LCA_OFFLINE` switches offline mode (gh #71, pi's
/// `isTruthyEnvFlag`): `1`, `true`, or `yes` in any case.
pub fn offline_env() -> bool {
    match std::env::var("LCA_OFFLINE") {
        Ok(value) => {
            let normalized = value.trim().to_lowercase();
            normalized == "1" || normalized == "true" || normalized == "yes"
        }
        Err(_) => false,
    }
}

/// Expand `@path` arguments (gh #71, pi's `processFileArguments`):
/// text inlines as `<file name="ABS">…</file>`, images return their
/// paths for `--attach` staging, empty files skip. Missing paths,
/// directories, and unreadable bytes refuse out loud.
pub fn expand_at_files(file_args: &[String], cwd: &Path) -> Result<(String, Vec<PathBuf>), String> {
    let mut text = String::new();
    let mut images = Vec::new();
    for arg in file_args {
        let path = cwd.join(arg);
        let absolute = if path.is_absolute() {
            path
        } else {
            cwd.join(path)
        };
        let Ok(meta) = std::fs::metadata(&absolute) else {
            return Err(format!("error: @{}: no such file", arg));
        };
        if !meta.is_file() {
            return Err(format!("error: @{}: not a file", arg));
        }
        if meta.len() == 0 {
            continue;
        }
        let bytes = std::fs::read(&absolute)
            .map_err(|err| format!("error: @{}: cannot read: {err}", arg))?;
        if lca_protocol::sniff_image_media_type(&bytes).is_some() {
            images.push(absolute.clone());
            text.push_str(&format!("<file name=\"{}\"></file>\n", absolute.display()));
            continue;
        }
        let mut content =
            String::from_utf8(bytes).map_err(|_| format!("error: @{}: not UTF-8 text", arg))?;
        // Strip a BOM like pi's reader does.
        if let Some(stripped) = content.strip_prefix('\u{feff}') {
            content = stripped.to_string();
        }
        if (content.len() as u64) > INPUT_MAX_BYTES {
            content.truncate(INPUT_MAX_BYTES as usize);
            content.push('\n');
            content.push_str(TRUNCATED_MARKER);
        }
        text.push_str(&format!(
            "<file name=\"{}\">\n{content}\n</file>\n",
            absolute.display()
        ));
    }
    Ok((text, images))
}
