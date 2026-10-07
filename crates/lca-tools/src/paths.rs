//! Path resolution and content addressing, split from `lib.rs` for the
//! workspace file ceiling (gate 11). Behaviour unchanged.

use std::path::{Component, Path, PathBuf};

/// Resolve `path` against `cwd`, canonicalizing the existing prefix
/// and normalizing the rest (unresolvable tails stay lexical).
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

/// Lowercase hex SHA-256 of `bytes`: the attachment content address. Public
/// so the core's attach path can address a file by the same function the
/// spill path uses (one hash, no second implementation).
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}
