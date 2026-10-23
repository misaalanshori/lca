//! gh #481 - running from `$HOME` reads the user config as the user config.
//!
//! The unit and regression rows pin the loader's classification. This one drives
//! the real binary from a real `$HOME`, because the bug's whole shape is "the
//! process's working directory and its data home are the same directory" - a fact
//! only the spawned process can establish.
//!
//! Before the fix, the first test's command printed:
//!
//! ```text
//! error: trust.default_project in project file is invalid: can only be set in the user file
//! ```
//!
//! and exited non-zero. After it, the documented setting loads and `lca config`
//! names the user file as its source.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

mod common;

use common::{Sandbox, sandbox, stderr, stdout};

/// Run the binary with `$HOME` as the working directory - the coincidence itself.
fn run_from_home(sandbox: &Sandbox, args: &[&str]) -> std::process::Output {
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_lca"));
    command
        .args(args)
        .current_dir(&sandbox.home)
        .env("HOME", &sandbox.home)
        .env("USERPROFILE", &sandbox.home)
        .env("XDG_CONFIG_HOME", sandbox.home.join(".config"))
        .env("LCA_UPDATE_CHECK", "false")
        .env("NO_COLOR", "1");
    command.output().expect("spawn lca")
}

fn write_user_config(sandbox: &Sandbox, body: &str) {
    let dir = sandbox.state_dir();
    std::fs::create_dir_all(&dir).expect("mkdir data home");
    std::fs::write(dir.join("config.toml"), body).expect("write user config");
}

// Verifies: gh #481 - **the acceptance.** From `$HOME`, a documented user setting
// loads, takes effect, and is attributed to the user file.
#[test]
fn lca_started_from_its_own_home_honours_the_user_config() {
    let sandbox = sandbox("481-from-home");
    write_user_config(&sandbox, "[trust]\ndefault_project = \"always\"\n");
    let output = run_from_home(&sandbox, &["config"]);
    let text = stdout(&output);
    let err = stderr(&output);
    assert!(
        output.status.success(),
        "the user's own config must not be a fatal project-file error:\n{err}"
    );
    assert!(
        !err.contains("project file is invalid"),
        "the FR-CFG-7 refusal must not fire for the user's own file:\n{err}"
    );
    assert!(
        text.contains("trust.default_project = always [user file]"),
        "the setting takes effect and the source names the user file:\n{text}"
    );
    let _ = std::fs::remove_dir_all(&sandbox.root);
}

// Verifies: gh #481 / FR-CFG-7 - **the pin, end to end.** A real project's
// `.lca/config.toml` that sets `trust.default_project` is still refused, through
// the real binary, with the message unchanged. Relaxing the refusal was the
// tempting fix; this is the row that says it did not happen.
#[test]
fn a_real_project_config_is_still_refused_from_the_binary() {
    let sandbox = sandbox("481-real-project");
    write_user_config(&sandbox, "provider = \"openai-compatible\"\n");
    let project = sandbox.project();
    std::fs::create_dir_all(project.join(".lca")).expect("mkdir project .lca");
    std::fs::write(
        project.join(".lca").join("config.toml"),
        "[trust]\ndefault_project = \"always\"\n",
    )
    .expect("write project config");
    // Trust the project, so the project file actually participates: an untrusted
    // project file is inert (FR-PERM-9) and would not reach the refusal.
    sandbox.approve_loopback_net(serde_json::json!({}));
    let output = sandbox.run(None, &["config"]);
    let err = stderr(&output);
    assert!(
        !output.status.success(),
        "a project file granting its own trust must be refused"
    );
    assert!(
        err.contains("trust.default_project") && err.contains("user file"),
        "the FR-CFG-7 message is unchanged:\n{err}"
    );
    let _ = std::fs::remove_dir_all(&sandbox.root);
}

// Verifies: gh #481 - **the `mcp list` half of the sweep.** The user's own server
// is reported with source `user` (it used to say `project`, because the same file
// was loaded a second time as a project override of itself).
#[test]
fn mcp_list_from_home_calls_the_users_own_servers_user_level() {
    let sandbox = sandbox("481-mcp-list");
    let data = sandbox.state_dir();
    std::fs::create_dir_all(&data).expect("mkdir data home");
    std::fs::write(
        data.join("mcp.json"),
        r#"{"mcpServers": {"echo": {"command": "/bin/cat"}}}"#,
    )
    .expect("write mcp.json");
    let output = run_from_home(&sandbox, &["mcp", "list"]);
    let text = stdout(&output);
    let err = stderr(&output);
    let combined = format!("{text}{err}");
    assert!(
        combined.contains(", user)"),
        "the user's own server is user-level, not a project override:\n{combined}"
    );
    assert!(
        !combined.contains(", project)"),
        "and nothing was loaded a second time as its own project override:\n{combined}"
    );
    assert!(
        !combined.contains("is ignored because the project is not trusted"),
        "the file was read as the user file; it is not an ignored project file:\n{combined}"
    );
    let _ = std::fs::remove_dir_all(&sandbox.root);
}
