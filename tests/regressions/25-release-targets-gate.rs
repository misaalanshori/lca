//! Cycle-7 P2: the six release targets were the last surface that nothing
//! built until release time. The "nothing builds target X, so X broke
//! silently" class had struck three times (the fuzz workspace, a bare release
//! build, the wasm32-wasip2 target) and each surfaced at a publish. This
//! pins the gate that closes it: the script exists, covers every target in
//! the release policy's artifact matrix, is wired into CI, and fails loudly
//! rather than silently passing an unbuildable target.
//!
//! Verifies: docs/release-policy.md (the artifact matrix), NFR-8, NFR-9,
//! NFR-10.

use std::path::PathBuf;
use std::process::Command;

/// The six targets the release policy packages, exactly.
const RELEASE_TARGETS: &[&str] = &[
    "x86_64-unknown-linux-musl",
    "aarch64-unknown-linux-musl",
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
    "x86_64-pc-windows-msvc",
    "aarch64-pc-windows-msvc",
];

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn the_gate_names_every_release_target_and_is_wired_into_ci() {
    let script = root().join("scripts/release-targets-check.sh");
    assert!(script.is_file(), "the gate script must exist: {script:?}");
    let text = std::fs::read_to_string(&script).expect("read the gate");
    for target in RELEASE_TARGETS {
        assert!(
            text.contains(target),
            "the gate's defaults must name {target}"
        );
    }

    // The workflow runs it, so a push exercises it (not just a manual step).
    let workflow = std::fs::read_to_string(root().join(".github/workflows/ci.yml"))
        .expect("read the CI workflow");
    assert!(
        workflow.contains("release-targets-check"),
        "CI must run the release-target gate on every push"
    );
    assert!(
        workflow.contains("scripts/release-targets-check.sh"),
        "CI must invoke the gate script"
    );
}

#[test]
#[cfg(unix)]
fn the_gate_fails_loudly_instead_of_passing_an_unbuildable_target() {
    let script = root().join("scripts/release-targets-check.sh");
    // An unknown triple is not a release target; the gate must exit non-zero
    // rather than report success it did not verify.
    let output = Command::new("bash")
        .arg(&script)
        .arg("bogus-target-xyz")
        .env("RELEASE_TARGETS_TIMEOUT", "60")
        .output()
        .expect("run the gate");
    assert!(
        !output.status.success(),
        "an unbuildable target must fail the gate, stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}

/// A guard on the file itself: a script that is not executable in a shell
/// context is a script that silently never runs. Unix-only: the check runs
/// the script through `bash`, and the gate's own CI legs are Linux/macOS
/// (the Windows targets are checked from the Linux leg).
#[test]
#[cfg(unix)]
fn the_gate_script_is_syntactically_valid() {
    let script = root().join("scripts/release-targets-check.sh");
    let output = Command::new("bash")
        .args(["-n"])
        .arg(&script)
        .output()
        .expect("bash -n");
    assert!(
        output.status.success(),
        "bash -n rejected the gate: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
