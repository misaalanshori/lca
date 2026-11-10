//! Where a shell token points, and whether that is inside the workspace.
//!
//! This file holds **every containment decision** the analyzer makes - the one
//! question a security reviewer asks of `lca-permissions`: "given a token, can
//! this say it stays inside the folder?". The lexical answer
//! ([`looks_like_path`], the redirection target) and the filesystem answer
//! ([`path_ok`], gh #346's symlink check) sit together on purpose, next to the
//! one table that is safe to name anywhere ([`DEV_SAFE`]).
//!
//! Split from the analyzer file for gate 11 (the 1,200-line ceiling); the code,
//! its comments and its guards are unchanged.

use std::path::{Path, PathBuf};

/// Paths that are safe redirection/read targets anywhere.
const DEV_SAFE: &[&str] = &["/dev/null", "/dev/stdin", "/dev/stdout", "/dev/stderr"];

/// If `token` is a redirection operator, its target: attached (`>file`) or the
/// following token (`> file`).
pub(super) fn redirection_target<'a>(token: &'a str, next: Option<&'a String>) -> Option<&'a str> {
    let stripped = token.trim_start_matches(|c: char| c.is_ascii_digit());
    let op = stripped
        .strip_prefix("&>>")
        .or_else(|| stripped.strip_prefix("&>"))
        .or_else(|| stripped.strip_prefix(">>"))
        .or_else(|| stripped.strip_prefix(">|"))
        .or_else(|| stripped.strip_prefix(">>"))
        .or_else(|| stripped.strip_prefix("<<"))
        .or_else(|| stripped.strip_prefix(">&"))
        .or_else(|| stripped.strip_prefix("<&"))
        .or_else(|| stripped.strip_prefix(">"))
        .or_else(|| stripped.strip_prefix("<"))?;
    if stripped.starts_with("<<") {
        // A heredoc: the body is untracked, refuse it elsewhere by returning a
        // target that fails path validation.
        return Some("<<");
    }
    if op.is_empty() {
        next.map(String::as_str)
    } else {
        Some(op)
    }
}

pub(super) fn looks_like_path(token: &str) -> bool {
    if token.is_empty() {
        return false;
    }
    token.contains('/')
        || token.starts_with('~')
        || token == "."
        || token == ".."
        || token.contains("..")
        || (cfg!(windows) && (token.contains('\\') || token.as_bytes().get(1) == Some(&b':')))
}

/// Whether a path token resolves inside `workspace` (base `cwd`).
///
/// Gh #346: the resolution is canonical as far as the path exists, so a
/// symlink inside the workspace that points outside (or into `.git/`) is
/// caught here, not just by the lexical join. This is the same
/// `canonicalize_deepest` the `fs` scope uses (ADR-0005), so the two
/// boundaries agree on what "inside" means.
pub(super) fn path_ok(token: &str, cwd: &Path, workspace: &Path) -> bool {
    let token = token.trim_matches(['"', '\'']);
    if token.is_empty() {
        return false;
    }
    // A leftover redirection operator (an untracked heredoc body, `>&`, …).
    if token.starts_with('<') || token.starts_with('>') {
        return false;
    }
    if DEV_SAFE.contains(&token) {
        return true;
    }
    if token.contains("://") {
        return false; // a URL, not a path
    }
    if token.starts_with('~') {
        return false; // the home directory is outside the workspace
    }
    let raw = Path::new(token);
    let joined = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        cwd.join(raw)
    };
    // Containment is decided in two steps, and the canonical step may only
    // ever narrow the allow set, never widen it.
    //
    // Step 1, lexical: the same rule the analyzer used before gh #346. This
    // is the necessary condition and keeps the drive-relative and synthetic
    // cases (which the lexical tests pin) exactly as they were.
    let normalized = crate::scope::lexical_normalize(&joined);
    if normalized != workspace && !normalized.starts_with(workspace) {
        return false;
    }
    // Step 2, canonical: only meaningful on a real filesystem. In production
    // the workspace always exists, so this always runs and closes the
    // symlink hole (gh #346): a link inside the workspace pointing outside
    // fails the prefix test here. When the workspace does not exist (the
    // synthetic-path unit tests), there is no real root to compare against,
    // so the lexical answer stands. One fs probe per path-like token, at
    // authorize time only - never on the per-turn path.
    if let Ok(real_root) = std::fs::canonicalize(workspace) {
        let resolved = crate::scope::canonicalize_deepest(&joined);
        if resolved != real_root && !resolved.starts_with(&real_root) {
            return false;
        }
    }
    true
}

pub(super) fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| crate::scope::lexical_normalize(path))
}
