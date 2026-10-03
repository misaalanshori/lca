//! GitHub issue #19 (open): `lca ext info | head -1` died with
//! `failed printing to stdout: Broken pipe`.
//!
//! Rust ignores `SIGPIPE` at start-up and gives a program no way to undo
//! that, so a write to a pipe whose reader has already gone away arrives as
//! `EPIPE` and `println!` panics. Every other Unix CLI is *terminated* by
//! the signal instead - which is what the shell expects: the first line is
//! delivered, the process stops quietly, and nothing is written to stderr.
//!
//! The row spawns the real binary with a stdout pipe whose read end it
//! closes before the child can write, then asserts the only thing that
//! matters: no panic text. It deliberately does not assert an exit code -
//! signal-terminated and a clean exit are both correct; a panic is not.
//!
//! The binary is looked up in the shared target directory rather than
//! through `CARGO_BIN_EXE_lca`, which Cargo only sets for the package that
//! owns the bin target. CI's `shell` group runs `-p lca-cli` and
//! `-p lca-regressions` together, so it is built there; a run of this
//! package alone skips with a named reason instead of failing (the
//! harness-absence rule, `docs/testing-plan.md` §14).
//!
//! Verifies: NFR-24 (a released defect's guard), GitHub issue #19.

// The row is `cfg(unix)`: Windows has no SIGPIPE to restore, so a closed
// pipe there is a recoverable error and the guard has nothing to assert.
// The helpers are gated with it - an ungated `use` or fn is an unused
// import/function in a Windows build, which clippy denies.
#[cfg(unix)]
use std::path::Path;
#[cfg(unix)]
use std::process::{Command, Stdio};

/// The built `lca` binary, if this run also produced one.
#[cfg(unix)]
fn binary() -> Option<std::path::PathBuf> {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    ["target/debug/lca", "target/release/lca"]
        .iter()
        .map(|relative| manifest.join(relative))
        .find(|path| path.is_file())
}

/// `ext info` on a fresh data home: two lines of stdout, no setup needed.
#[cfg(unix)]
fn info_command(binary: &Path, home: &Path) -> Command {
    let mut command = Command::new(binary);
    command
        .args(["ext", "info", "openai-compatible"])
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("LCA_UPDATE_CHECK", "false");
    command
}

// Verifies: gh #19 - a stdout that closes early ends the process quietly,
// with no panic on stderr, instead of `failed printing to stdout`.
#[cfg(unix)]
#[test]
fn a_stdout_closed_early_ends_the_process_without_a_panic() {
    let Some(binary) = binary() else {
        eprintln!(
            "skip: no built `lca` in this package's target directory \
             (run it alongside `-p lca-cli`, as CI's `shell` group does)"
        );
        return;
    };
    let home = lca_testkit::scratch_path("regression-gh19-sigpipe");
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).expect("scratch home");

    // Control: the command really does write to stdout, so the closed-pipe
    // run below exercises a write and not an early usage error.
    let control = info_command(&binary, &home)
        .output()
        .expect("spawn the control run");
    assert!(
        !control.stdout.is_empty(),
        "`ext info` writes stdout at all: {:?}",
        String::from_utf8_lossy(&control.stderr)
    );

    // The run under test: the read end goes before the child can write, so
    // its very first `println!` lands on a pipe with no reader.
    let mut child = info_command(&binary, &home)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    drop(child.stdout.take());
    let output = child.wait_with_output().expect("wait for the child");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("failed printing to stdout"),
        "a closed stdout panicked: {stderr}"
    );
    assert!(
        !stderr.contains("panicked at"),
        "the child panicked for any reason: {stderr}"
    );
    let _ = std::fs::remove_dir_all(&home);
}
