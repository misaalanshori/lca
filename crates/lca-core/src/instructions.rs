//! Mid-session instruction updates (gh #217, OpenCode's
//! `InstructionDiscovery.renderUpdate` shape): the host snapshots
//! instruction sources at startup; each turn re-scans them, and a
//! change appends a unified diff as a trailing system message while
//! `messages[0]` stays byte-identical (FR-CACHE-5, ADR-0017).
//!
//! The baseline lives behind an `Arc<Mutex<..>>` in
//! [`crate::AgentConfig`],
//! so hosts that rebuild the config per generation keep one session
//! baseline without extra state.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use lca_protocol::{ChatMessage, MessageRole};

/// The session's instruction baseline: last-seen bodies by path, plus
/// whether the startup scan already adopted them (the first scan is
/// always silent - adoption, not a change).
#[derive(Debug, Default)]
pub struct InstructionBaseline {
    primed: bool,
    bodies: HashMap<PathBuf, String>,
}

/// The shareable baseline handle [`crate::AgentConfig`] carries.
pub type SharedBaseline = Arc<Mutex<InstructionBaseline>>;

/// Re-scan `sources` against the baseline, returning one trailing
/// system message per change: unified diffs for edits (the full text
/// when shorter than its diff), the full text for new files, and a
/// retirement note for deletions. Unchanged files and the priming
/// scan produce nothing. The baseline updates in place.
pub async fn check_instructions(
    sources: &[PathBuf],
    baseline: &SharedBaseline,
) -> Vec<ChatMessage> {
    let mut seen: HashMap<PathBuf, String> = HashMap::new();
    for path in sources {
        if let Ok(body) = tokio::fs::read_to_string(path).await {
            seen.insert(path.clone(), body);
        }
    }
    let mut guard = lock(baseline);
    let mut deltas = Vec::new();
    if !guard.primed {
        guard.primed = true;
        guard.bodies = seen;
        return deltas;
    }
    for (path, body) in &seen {
        match guard.bodies.get(path) {
            None => deltas.push(new_instructions(path, body)),
            Some(before) if before != body => deltas.push(changed_instructions(path, before, body)),
            _ => {}
        }
    }
    for path in guard.bodies.keys() {
        if !seen.contains_key(path) {
            deltas.push(ChatMessage::text(
                MessageRole::System,
                format!("The instructions from {} no longer apply.", path.display()),
            ));
        }
    }
    guard.bodies = seen;
    deltas
}

/// A new instruction file's full text (OpenCode's added shape).
fn new_instructions(path: &Path, body: &str) -> ChatMessage {
    ChatMessage::text(
        MessageRole::System,
        format!(
            "New instructions apply from {}:\n{}",
            path.display(),
            capped(body)
        ),
    )
}

/// An edit's unified diff (OpenCode's changed shape): the hunk when
/// more compact than the full text, the full text otherwise.
fn changed_instructions(path: &Path, before: &str, after: &str) -> ChatMessage {
    let patch = similar::TextDiff::from_lines(before, after)
        .unified_diff()
        .header("before", "after")
        .to_string();
    let hunk = patch
        .find("@@")
        .map(|at| patch[at..].trim_end())
        .unwrap_or("");
    let full = format!(
        "Project instructions in `{}` changed:\n{}",
        path.display(),
        capped(after)
    );
    let text = if hunk.is_empty() || full.len() <= diff_wrapped(path, hunk).len() {
        full
    } else {
        diff_wrapped(path, hunk)
    };
    ChatMessage::text(MessageRole::System, text)
}

/// The diff-shaped delta body.
fn diff_wrapped(path: &Path, hunk: &str) -> String {
    format!(
        "Project instructions in `{}` changed. Here is the diff:\n```diff\n{hunk}\n```",
        path.display()
    )
}

/// Bound one delta's bytes: instruction files are small, but a
/// pathological one must not eat the context window.
fn capped(body: &str) -> String {
    const DELTA_LIMIT_BYTES: usize = 16384;
    if body.len() <= DELTA_LIMIT_BYTES {
        return body.to_string();
    }
    let (kept, _) =
        lca_tools::truncate::truncate_head_tail(body, DELTA_LIMIT_BYTES, usize::MAX, None);
    kept
}

fn lock(baseline: &SharedBaseline) -> std::sync::MutexGuard<'_, InstructionBaseline> {
    baseline.lock().unwrap_or_else(|poison| poison.into_inner())
}
