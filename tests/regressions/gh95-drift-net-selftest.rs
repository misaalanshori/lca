//! GitHub #95: preset and capability facts live in data
//! (`provider-presets.toml`, the manifest schema); docs that restate
//! them are pinned by `scripts/docs-consistency.sh`. A drift check no
//! test ever sees fail is decoration, so this file builds fixture
//! trees - symlinks to the real files plus one drifted copy - and
//! asserts the net fires on each drift shape and passes clean.
//!
//! Unix-only for the execution rows: the net is a bash script and its
//! own CI legs are Linux/macOS, the same reason the release-targets
//! gate test (`25-release-targets-gate.rs`) gives.

use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn the_drift_net_script_exists_and_runs_in_ci() {
    let script = root().join("scripts/docs-consistency.sh");
    assert!(script.is_file(), "the net must exist: {script:?}");
    let workflow = std::fs::read_to_string(root().join(".github/workflows/ci.yml"))
        .expect("read the CI workflow");
    assert!(
        workflow.contains("scripts/docs-consistency.sh"),
        "CI must run the drift net on every push"
    );
}

/// A fixture tree: symlinks to every file the net reads, so the
/// undisrupted tree matches the working copy exactly. One caller
/// replaces a single symlink with a drifted copy.
#[cfg(unix)]
fn fixture_tree(name: &str) -> PathBuf {
    let repo = root();
    let tree = lca_testkit::scratch_path(name);
    let link = |target: &str| {
        let from = repo.join(target);
        let to = tree.join(target);
        std::fs::create_dir_all(to.parent().expect("parent")).expect("mkdir");
        std::os::unix::fs::symlink(&from, &to).expect("symlink");
    };
    link("docs/release-policy.md");
    link("docs/capabilities.md");
    link("docs/providers/README.md");
    link("schemas/extension-manifest.schema.json");
    link("extensions/openai-compatible/resources/provider-presets.toml");
    link("AGENTS.md");
    link("README.md");
    tree
}

/// Replace one symlinked fixture file with a drifted copy.
#[cfg(unix)]
fn drift_file(tree: &Path, target: &str, from: &str, to: &str) {
    let path = tree.join(target);
    std::fs::remove_file(&path).expect("drop the symlink");
    let repo_text = std::fs::read_to_string(root().join(target)).expect("read the real file");
    std::fs::write(&path, repo_text.replacen(from, to, 1)).expect("write the drift");
}

/// Run the real net against a fixture tree as its repo root.
#[cfg(unix)]
fn run_net(tree: &Path) -> std::process::Output {
    let script = root().join("scripts/docs-consistency.sh");
    std::process::Command::new("bash")
        .arg(&script)
        .env("DOCS_CONSISTENCY_ROOT", tree)
        .output()
        .expect("run the net")
}

#[test]
#[cfg(unix)]
fn the_net_passes_on_an_undrifted_tree() {
    let tree = fixture_tree("lca-drift-ok");
    let output = run_net(&tree);
    assert!(
        output.status.success(),
        "the clean tree must pass: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let _ = std::fs::remove_dir_all(&tree);
}

#[test]
#[cfg(unix)]
fn a_wrong_preset_count_in_docs_fails_the_net() {
    let tree = fixture_tree("lca-drift-count");
    drift_file(
        &tree,
        "docs/providers/README.md",
        "19 presets ship today",
        "20 presets ship today",
    );
    let output = run_net(&tree);
    assert!(
        !output.status.success(),
        "a prose count the data disagrees with must fail the net"
    );
    let _ = std::fs::remove_dir_all(&tree);
}

#[test]
#[cfg(unix)]
fn a_tabled_preset_id_missing_from_data_fails_the_net() {
    let tree = fixture_tree("lca-drift-id");
    drift_file(
        &tree,
        "docs/providers/README.md",
        "| `lmstudio` |",
        "| `lmstudio-x` |",
    );
    let output = run_net(&tree);
    assert!(
        !output.status.success(),
        "a tabled id absent from the TOML must fail the net"
    );
    let _ = std::fs::remove_dir_all(&tree);
}

#[test]
#[cfg(unix)]
fn a_dropped_preset_row_fails_the_net() {
    let tree = fixture_tree("lca-drift-row");
    let path = tree.join("docs/providers/README.md");
    let repo_text = std::fs::read_to_string(root().join("docs/providers/README.md")).expect("read");
    let dropped = repo_text
        .lines()
        .filter(|line| !line.starts_with("| `lmstudio` |"))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::remove_file(&path).expect("drop the symlink");
    std::fs::write(&path, dropped).expect("write the drift");
    let output = run_net(&tree);
    assert!(
        !output.status.success(),
        "a table row deleted behind the data's back must fail the net"
    );
    let _ = std::fs::remove_dir_all(&tree);
}

#[test]
#[cfg(unix)]
fn a_documented_capability_missing_from_the_schema_fails_the_net() {
    let tree = fixture_tree("lca-drift-cap");
    drift_file(
        &tree,
        "docs/capabilities.md",
        "[capabilities.pty]",
        "[capabilities.pty-x]",
    );
    let output = run_net(&tree);
    assert!(
        !output.status.success(),
        "a documented capability the schema never heard of must fail the net"
    );
    let _ = std::fs::remove_dir_all(&tree);
}

/// A guard on the file itself: a net that is not executable in a
/// shell context silently never runs.
#[test]
#[cfg(unix)]
fn the_net_script_is_syntactically_valid() {
    let script = root().join("scripts/docs-consistency.sh");
    let output = std::process::Command::new("bash")
        .args(["-n"])
        .arg(&script)
        .output()
        .expect("bash -n");
    assert!(
        output.status.success(),
        "bash -n rejected the net: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
