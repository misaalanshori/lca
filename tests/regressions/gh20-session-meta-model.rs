//! GitHub issue #20: `SessionMeta.model` was never populated, so
//! `meta.json` serialized without the model and provider that
//! `docs/session-log-format.md` promises it carries.
//!
//! The field is specified, not dead, so it is populated rather than
//! dropped - at both points the session's model is established: the
//! `/model` handler (ADR-0024 already rewrites the model "everywhere it
//! is read", and `meta.json` joins that list) and turn start, from
//! `AgentConfig`, which is where a turn that ends before any reply still
//! gets its model recorded.
//!
//! The rows drive the real interface in tmux, because the switch they
//! pin happens in `/model`'s handler and nothing shorter reaches it; the
//! binary comes from this package's `target/` the way gh #19's rows get
//! theirs, so a run without a built binary skips instead of failing.
//!
//! Verifies: NFR-24 (a released finding's guard), GitHub issue #20.

#![cfg(unix)] // tmux and the drive below are Unix-only; Windows CI compiles nothing here.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

/// The built `lca` binary, if this run also produced one.
fn binary() -> Option<PathBuf> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    ["target/debug/lca", "target/release/lca"]
        .iter()
        .map(|relative| manifest.join(relative))
        .find(|path| path.is_file())
}

/// tmux, on this suite's own private socket: never the default socket and
/// never the lca-cli suite's, so a parallel run cannot disturb either.
fn tmux(args: &[&str]) -> Output {
    let mut full = vec!["-L", "lca-tui-gh20"];
    full.extend_from_slice(args);
    Command::new("tmux").args(&full).output().expect("run tmux")
}

fn tmux_available() -> bool {
    Command::new("tmux")
        .arg("-V")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Whether the drive can run here: a missing binary or a missing tmux is
/// a skip (the same contract gh #19's rows use), never a failure.
fn require() -> bool {
    if binary().is_none() {
        eprintln!(
            "skip: no built `lca` in this package's target directory \
             (run it alongside `-p lca-cli`, as CI's `shell` group does)"
        );
        return false;
    }
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal rows are Unix-only)");
        return false;
    }
    true
}

/// One real-terminal session against a scratch `HOME`.
struct Drive {
    name: String,
    home: PathBuf,
}

impl Drive {
    /// Start the interface in a pane on a fresh sandbox.
    fn spawn(tag: &str, env: &[(&str, &str)], args: &[&str]) -> Drive {
        let home = lca_testkit::scratch_path(&format!("gh20-{tag}-home"));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join("project")).expect("project dir");
        Drive::start(tag, &home, env, args)
    }

    /// Start the interface again on an existing sandbox - the resume path.
    fn reopen(tag: &str, home: &Path, args: &[&str]) -> Drive {
        Drive::start(tag, home, &[], args)
    }

    fn start(tag: &str, home: &Path, env: &[(&str, &str)], args: &[&str]) -> Drive {
        let name = format!("lca-gh20-{}-{tag}", std::process::id());
        let _ = tmux(&["kill-session", "-t", &name]);
        let mut command = format!(
            "cd {project} && HOME={home} USERPROFILE={home} XDG_CONFIG_HOME={config} \
             LCA_UPDATE_CHECK=false",
            project = home.join("project").display(),
            home = home.display(),
            config = home.join(".config").display(),
        );
        for (key, value) in env {
            command.push_str(&format!(" {key}={value}"));
        }
        command.push_str(&format!(" {}", binary().expect("checked").display()));
        for arg in args {
            command.push_str(&format!(" {arg}"));
        }
        let out = tmux(&[
            "new-session",
            "-d",
            "-s",
            &name,
            "-x",
            "120",
            "-y",
            "40",
            &command,
        ]);
        assert!(
            out.status.success(),
            "tmux new-session: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        Drive {
            name,
            home: home.to_path_buf(),
        }
    }

    fn send(&self, keys: &[&str]) {
        let mut args = vec!["send-keys", "-t", &self.name];
        args.extend_from_slice(keys);
        let out = tmux(&args);
        assert!(
            out.status.success(),
            "tmux send-keys: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Poll the pane until `needle` appears (150 ms cadence).
    fn wait_for(&self, needle: &str, timeout: Duration) -> String {
        let deadline = Instant::now() + timeout;
        loop {
            let out = tmux(&["capture-pane", "-p", "-t", &self.name]);
            let pane = String::from_utf8_lossy(&out.stdout).into_owned();
            if pane.contains(needle) {
                return pane;
            }
            if Instant::now() > deadline {
                panic!("`{needle}` never appeared in the pane:\n{pane}");
            }
            std::thread::sleep(Duration::from_millis(150));
        }
    }

    /// `/exit`, then wait for the session to be gone so the log is closed.
    fn quit(&self) {
        self.send(&["/exit", "Enter"]);
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if !tmux(&["has-session", "-t", &self.name]).status.success() {
                return;
            }
            if Instant::now() > deadline {
                panic!("the session did not exit");
            }
            std::thread::sleep(Duration::from_millis(150));
        }
    }

    /// The newest `meta.json` under this home - what a resume would read.
    fn latest_meta(&self) -> String {
        let sessions = self.home.join(".lca").join("sessions");
        let path = newest_file(&sessions, "meta.json").expect("a meta.json under the scratch home");
        std::fs::read_to_string(path).expect("read meta.json")
    }
}

/// The newest file named `name` under `root`: sessions are keyed by
/// directory, so the scan picks the most recently written one.
fn newest_file(root: &Path, name: &str) -> Option<PathBuf> {
    let mut found: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.file_name().and_then(|n| n.to_str()) == Some(name) {
                let stamp = entry
                    .metadata()
                    .and_then(|meta| meta.modified())
                    .unwrap_or(std::time::UNIX_EPOCH);
                found.push((stamp, path));
            }
        }
    }
    found.sort_by_key(|left| left.0);
    found.pop().map(|(_, path)| path)
}

/// Two models for `/model` to choose between - the env one and a second
/// from the extension's own credentials namespace, no endpoint needed.
/// The namespace carries a key (gh #177): the catalog lists a provider
/// only when it is ready in the `auth check` sense, and a keyless
/// namespace is not ready. No turn runs here, so the key is never used.
fn two_models(home: &Path) {
    let dir = home.join(".lca").join("credentials");
    std::fs::create_dir_all(&dir).expect("credentials dir");
    std::fs::write(
        dir.join("openai-compatible.json"),
        "{\"models\":\"first-model,second-model\",\"api_key\":\"test-key\"}\n",
    )
    .expect("write credentials");
}

/// Drive the switch: open the picker, take the second row, confirm it.
fn switch_to_second_model(drive: &Drive) {
    drive.wait_for("[session in", Duration::from_secs(25));
    drive.send(&["/model", "Enter"]);
    drive.wait_for("second-model", Duration::from_secs(15));
    drive.send(&["Down"]);
    std::thread::sleep(Duration::from_millis(300));
    drive.send(&["Enter"]);
    drive.wait_for(
        "model for this session: openai-compatible/second-model",
        Duration::from_secs(15),
    );
}

// Verifies: gh #20 - the `/model` handler joins ADR-0024's "everywhere it
// is read": a switch with no turn at all still lands `meta.model`, and
// `meta.provider` with it. Red before the fix: only a completed reply
// wrote metadata, so this session's `meta.json` carries neither.
#[test]
fn a_model_switch_writes_the_model_to_the_session_metadata() {
    if !require() {
        return;
    }
    let drive = Drive::spawn("switch", &[("OPENAI_MODEL", "first-model")], &[]);
    two_models(&drive.home);

    switch_to_second_model(&drive);
    // No turn: the switch alone must carry the model.
    drive.quit();

    let meta = drive.latest_meta();
    assert!(
        meta.contains("\"model\": \"second-model\""),
        "meta.json names the model the session switched to: {meta}"
    );
    assert!(
        meta.contains("\"provider\": \"openai-compatible\""),
        "meta.json names the provider last used: {meta}"
    );
    let _ = std::fs::remove_dir_all(&drive.home);
}

// Verifies: gh #20 - the field is durable, which is the half a resume
// reads: reopening the session after the switch still shows the last-used
// model in `meta.json`. The turn below only exists to give the session a
// message - `list_sessions` hides a session with none, so `-c` would have
// nothing to continue - and its key-less failure is exactly the turn
// whose metadata used to be lost.
#[test]
fn a_resumed_session_still_names_the_last_used_model() {
    if !require() {
        return;
    }
    let drive = Drive::spawn("resume", &[("OPENAI_MODEL", "first-model")], &[]);
    two_models(&drive.home);
    switch_to_second_model(&drive);
    drive.send(&["hello", "Enter"]);
    std::thread::sleep(Duration::from_secs(3));
    drive.quit();
    let home = drive.home.clone();

    // `-c` continues this project's last session: the resume path a user
    // takes, reading the same `meta.json`.
    let resumed = Drive::reopen("resume-again", &home, &["-c"]);
    resumed.wait_for("[session in", Duration::from_secs(25));
    resumed.quit();

    let sessions = home.join(".lca").join("sessions");
    let path = newest_file(&sessions, "meta.json").expect("meta.json");
    let text = std::fs::read_to_string(path).expect("read meta");
    assert!(
        text.contains("\"model\": \"second-model\""),
        "a resumed session's meta.json shows the last-used model: {text}"
    );
    let _ = std::fs::remove_dir_all(&home);
}

// Verifies: gh #20 - turn start records the model from `AgentConfig`,
// before the model answers: a turn that ends in an error (no key, so the
// request never completes) still leaves `meta.model` on the session.
// Red before the fix: the only write sat after the assistant's reply, so
// a failed turn recorded nothing.
#[test]
fn a_turn_that_ends_before_any_reply_still_records_the_model() {
    if !require() {
        return;
    }
    let drive = Drive::spawn("turn", &[], &["--model", "env-model"]);

    drive.wait_for("env-model", Duration::from_secs(25));
    // No key: the turn starts and fails without a reply.
    drive.send(&["hello", "Enter"]);
    std::thread::sleep(Duration::from_secs(3));
    drive.quit();

    let meta = drive.latest_meta();
    assert!(
        meta.contains("\"model\": \"env-model\""),
        "the turn start recorded the model even though no reply landed: {meta}"
    );
    let _ = std::fs::remove_dir_all(&drive.home);
}
