//! CLI invocation helpers (phase 4A): piped stdin, `@file` expansion,
//! tool/session/resource flag resolution. Pure shapes live here so the
//! route and guard tests pin them without a terminal; `run` does the
//! I/O (stdin reads, file expansion) before routing.

use std::path::{Path, PathBuf};

#[cfg(test)]
#[path = "invoke_tests.rs"]
mod tests;

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
