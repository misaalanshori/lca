//! The log's line primitives (gh #564): the tip scan `append` stamps from, the
//! atomic file write `meta.json` and `index.json` both go through, and the
//! project-directory lookup a fork chain needs.
//!
//! `write_atomic` is the one place a small file is replaced (gh #267's rename
//! discipline): the whole rule lives here so a caller cannot half-write one.

use std::io::Write;
use std::path::Path;

use crate::Result;

use super::Session;

/// The newest record id in a log buffer (gh #37): newest line
/// first, skipping blanks and a partial crash tail. Empty when no
/// complete line names one.
pub(super) fn last_id_in(buf: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(buf);
    for line in text.lines().rev() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if let Some(id) = value.get("id").and_then(|v| v.as_str()) {
            return Some(id.to_string());
        }
    }
    None
}

/// The project directory a session lives under. A session directory is
/// always `<data>/sessions/<project>/<id>`, so its parent is the project
/// key; a session whose directory has no parent is a broken handle, reported
/// as an I/O error rather than a panic.
pub(crate) fn project_dir_of(session: &Session) -> Result<&Path> {
    session.dir().parent().ok_or_else(|| {
        crate::Error::Io(std::io::Error::other(format!(
            "session directory {} has no project parent",
            session.dir().display()
        )))
    })
}

pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let temp = path.with_extension("tmp");
    {
        let mut file = std::fs::File::create(&temp)?;
        file.write_all(bytes)?;
        file.flush()?;
    }
    std::fs::rename(&temp, path)?;
    Ok(())
}
