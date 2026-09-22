//! Sandboxed test environment (testing plan section 5): an isolated `HOME`,
//! config directory, and temp directory with an environment restored on drop.
//!
//! Follows pi's own `test.sh`: isolated `HOME`, isolated cache, stripped
//! environment, so no test can read the developer's real credentials.

#![allow(unsafe_code)] // documented exemption: see the crate-level docs

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::MutexGuard;

/// Every environment user serializes on this lock.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Take the environment lock for custom harnesses.
pub fn env_lock() -> MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// An isolated `HOME`. Held for the fixture's lifetime; dropping restores
/// the previous environment and removes the sandbox.
pub struct TestEnv {
    _guard: MutexGuard<'static, ()>,
    saved: Vec<(OsString, Option<OsString>)>,
    home: PathBuf,
    root: PathBuf,
}

impl TestEnv {
    /// Create a fresh sandbox for `name`.
    pub fn new(name: &str) -> TestEnv {
        let guard = env_lock();
        let root = std::env::temp_dir().join(format!("lca-testkit-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join("home");
        std::fs::create_dir_all(home.join(".config")).expect("mkdir");
        std::fs::create_dir_all(root.join("data")).expect("mkdir");
        std::fs::create_dir_all(root.join("tmp")).expect("mkdir");

        let mut saved = Vec::new();
        for (key, value) in [
            ("HOME", home.clone()),
            ("USERPROFILE", home.clone()),
            ("XDG_DATA_HOME", root.join("data")),
            ("XDG_CONFIG_HOME", home.join(".config")),
            ("TMPDIR", root.join("tmp")),
        ] {
            saved.push((OsString::from(key), std::env::var_os(key)));
            // SAFETY: every TestEnv holds ENV_LOCK for its whole life, and
            // tests that read or write the environment must take the same
            // lock (`env_lock()`); cargo-nextest additionally runs each test
            // in its own process. No concurrent environment access can
            // observe a half-updated environment.
            unsafe { std::env::set_var(key, value) };
        }
        TestEnv {
            _guard: guard,
            saved,
            home,
            root,
        }
    }

    /// The sandboxed home directory.
    pub fn home(&self) -> &Path {
        &self.home
    }

    /// The sandbox root (data, tmp, and friends live here).
    pub fn root(&self) -> &Path {
        &self.root
    }
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        for (key, value) in self.saved.drain(..) {
            // SAFETY: same lock discipline as in `new`.
            unsafe {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
