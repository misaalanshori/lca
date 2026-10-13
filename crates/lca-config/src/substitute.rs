//! `{env:VAR}` and `{file:path}` substitution (gh #218, OpenCode's
//! `ConfigVariable.substitute` shape): TOML string values may reference
//! the process environment or a secret file, so teams can commit
//! `.lca/config.toml` without committing plaintext credentials.
//!
//! Security boundaries, all loud:
//! * Credential stores are never readable: any `{file:}` whose path
//!   touches `credentials/`, `sessions/`, or `grants` is refused.
//! * There is no execution hook by construction: `!command` is inert
//!   text, never spawned.
//! * Missing or unreadable files are errors, never silent empties.
//! * Unset variables substitute empty (OpenCode's rule).
//! * Resolved secrets are never logged: only the dotted keys that
//!   substituted are reported (see [`Config::substituted`]), never
//!   their values.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use super::ConfigError;

/// Substitute every `{env:}`/`{file:}` token in the table's strings
/// (recursing into nested tables and arrays). Returns the dotted keys
/// whose values substituted, so displays can mask them. `dir` is the
/// config file's directory (relative `{file:}` roots there); `label`
/// names the layer for error messages.
pub fn substitute_table(
    table: &mut toml::Table,
    dir: &Path,
    label: &str,
) -> Result<BTreeSet<String>, ConfigError> {
    let mut substituted = BTreeSet::new();
    for (key, value) in table.iter_mut() {
        substitute_value(value, dir, label, key, &mut substituted)?;
    }
    Ok(substituted)
}

fn substitute_value(
    value: &mut toml::Value,
    dir: &Path,
    label: &str,
    path: &str,
    substituted: &mut BTreeSet<String>,
) -> Result<(), ConfigError> {
    match value {
        toml::Value::String(text) => {
            if let Some(resolved) = substitute_text(text, dir, label)? {
                *text = resolved;
                substituted.insert(path.to_string());
            }
        }
        toml::Value::Array(items) => {
            for item in items {
                substitute_value(item, dir, label, path, substituted)?;
            }
        }
        toml::Value::Table(inner) => {
            for (key, item) in inner.iter_mut() {
                let nested = format!("{path}.{key}");
                substitute_value(item, dir, label, &nested, substituted)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Interpolate one string, or `None` when it holds no tokens.
fn substitute_text(text: &str, dir: &Path, label: &str) -> Result<Option<String>, ConfigError> {
    if !text.contains("{env:") && !text.contains("{file:") {
        return Ok(None);
    }
    let fail = |token: &str, reason: String| ConfigError::Substitution {
        token: token.to_string(),
        label: label.to_string(),
        reason,
    };
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    let mut found = false;
    while let Some(open) = rest.find('{') {
        let Some(close) = rest[open..].find('}') else {
            break;
        };
        let token = &rest[open..open + close + 1];
        // The argument is the text between the prefix and the closing
        // brace (the token slice keeps both).
        let (kind, arg) = match token.strip_prefix("{env:") {
            Some(arg) => ("env", &arg[..arg.len() - 1]),
            None => match token.strip_prefix("{file:") {
                Some(arg) => ("file", &arg[..arg.len() - 1]),
                None => {
                    out.push_str(&rest[..open + 1]);
                    rest = &rest[open + 1..];
                    continue;
                }
            },
        };
        found = true;
        out.push_str(&rest[..open]);
        match kind {
            "env" => {
                if arg.is_empty() {
                    return Err(fail(token, "empty variable name".to_string()));
                }
                out.push_str(&std::env::var(arg).unwrap_or_default());
            }
            _ => {
                if arg.is_empty() {
                    return Err(fail(token, "empty file path".to_string()));
                }
                out.push_str(&read_secret_file(arg, dir, token, label)?);
            }
        }
        rest = &rest[open + close + 1..];
    }
    out.push_str(rest);
    Ok(found.then_some(out))
}

/// Read a `{file:}` reference: `~/` grows from the home directory,
/// relative paths root at the config file's directory, absolute paths
/// stand as written. Fails loudly on credential stores, missing files,
/// and unreadable files; trims trailing whitespace.
fn read_secret_file(
    arg: &str,
    dir: &Path,
    token: &str,
    label: &str,
) -> Result<String, ConfigError> {
    let fail = |reason: String| ConfigError::Substitution {
        token: token.to_string(),
        label: label.to_string(),
        reason,
    };
    let expanded = if let Some(rest) = arg.strip_prefix("~/") {
        home_dir()
            .map(|home| home.join(rest))
            .ok_or_else(|| fail("cannot resolve the home directory for {file:~/...}".to_string()))?
    } else {
        PathBuf::from(arg)
    };
    let resolved = if expanded.is_absolute() {
        expanded
    } else {
        dir.join(expanded)
    };
    let normalized = normalize(&resolved);
    if normalized.components().any(|component| {
        matches!(
            component,
            Component::Normal(name)
                if matches!(
                    name.to_str(),
                    Some("credentials" | "sessions" | "grants" | "grants.json")
                )
        )
    }) {
        return Err(fail(format!(
            "refused: {arg} touches a credential store (credentials/, sessions/, grants)"
        )));
    }
    std::fs::read_to_string(&resolved)
        .map(|text| text.trim_end().to_string())
        .map_err(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                fail(format!(
                    "file referenced by {token} does not exist: {}",
                    normalized.display()
                ))
            } else {
                fail(format!("cannot read {}: {err}", normalized.display()))
            }
        })
}

/// The home directory without a new dependency: `HOME` on Unix,
/// `USERPROFILE` on Windows.
fn home_dir() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)
}

/// Lexical `.`/`..` normalization (no filesystem touch): `..`
/// spellings cannot dodge the credential refusal.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}
