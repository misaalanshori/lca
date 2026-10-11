//! Split from `lib.rs` for the workspace file ceiling (gate 11,
//! #158). Behaviour unchanged.

use std::path::{Path, PathBuf};
/// The user data directory for sessions, grants, and state.
pub fn data_dir() -> PathBuf {
    lca_session::default_data_dir()
}

/// Load the user's key overrides (gh #66): `keybindings.toml` beside the
/// config, action → keys. A missing file is no overrides and no error;
/// anything else loud (returned for the startup notice) with defaults
/// kept by the caller.
pub fn load_user_keybindings(
    dir: &std::path::Path,
) -> (
    std::collections::BTreeMap<String, Vec<String>>,
    Option<String>,
) {
    let path = dir.join("keybindings.toml");
    if !path.is_file() {
        return (std::collections::BTreeMap::new(), None);
    }
    match lca_config::load_keybindings_file(&path) {
        Ok(map) => (map, None),
        Err(err) => (std::collections::BTreeMap::new(), Some(err.to_string())),
    }
}

/// Resolve and create one session's temporary directory (`<data>/tmp/<id>`),
/// gh #160: a passed-through value, never a process global, so in-process
/// session switching resolves a fresh dir per session. Creation failures
/// propagate to the caller (`?`, never a silent `let _`); contexts with
/// no session (extension install, pre-session login) pass the system
/// temp dir explicitly instead.
pub fn ensure_session_temp(data_dir: &Path, session_id: &str) -> std::io::Result<PathBuf> {
    let path = data_dir.join("tmp").join(session_id);
    std::fs::create_dir_all(&path)?;
    Ok(path)
}

/// Remove one session temp directory, best-effort: exit-time cleanup,
/// not a fallible operation the exit path reports.
pub(crate) fn cleanup_session_temp(path: &Path) {
    let _ = std::fs::remove_dir_all(path);
}

/// Removes its session temp directory on drop, so every exit path after
/// the session starts cleans up.
pub(crate) struct SessionTempGuard(pub PathBuf);

impl Drop for SessionTempGuard {
    fn drop(&mut self) {
        cleanup_session_temp(&self.0);
    }
}

/// The platform configuration directory `home-config` resolves to. This is
/// the directory an extension reads *another tool's* saved login from, not
/// the agent's own subdirectory; the state-directory exclusion keeps the
/// agent's own tree (sessions, extensions, credentials) unreadable even on
/// macOS and Windows, where it sits under this directory.
pub fn config_dir() -> PathBuf {
    if cfg!(target_os = "linux") {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| {
                let home = std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| ".".into());
                home.join(".config")
            })
    } else if cfg!(target_os = "macos") {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| ".".into());
        home.join("Library/Application Support")
    } else {
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| ".".into())
    }
}

/// The user configuration file: `~/.lca/config.toml` (R7). It lives with
/// the rest of the agent's data, not in the platform config directory -
/// `config_dir` stays what it always was, the `home-config` scope another
/// tool's logins resolve to.
pub fn config_file() -> PathBuf {
    data_dir().join("config.toml")
}

#[cfg(test)]
mod tests {
    use super::*;

    // Verifies: gh #66 - a missing key file is silent defaults; a broken
    // one is a loud string for the startup notice.
    #[test]
    fn a_missing_key_file_is_silent_and_a_broken_one_is_loud() {
        let dir = std::env::temp_dir().join(format!("lca-kb-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let (map, err) = load_user_keybindings(&dir);
        assert!(map.is_empty() && err.is_none(), "absent file, absent error");
        std::fs::write(dir.join("keybindings.toml"), "app.clear = [\n").expect("write");
        let (map, err) = load_user_keybindings(&dir);
        assert!(map.is_empty(), "broken file binds nothing");
        assert!(
            err.as_deref().unwrap_or_default().contains("keybindings"),
            "and says so loud: {err:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
