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

#[cfg(unix)]
use std::path::Path;
use std::path::PathBuf;

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
    // Idempotent: the README-link pass below links every `docs/*.md` the README
    // names, which overlaps the explicit list above and doc links added since.
    // A second symlink for one path is an error, not a copy (the original
    // closure's own comment made this the caller's duty; making it the helper's
    // is one less thing every new fixture caller can get wrong).
    let link = |target: &str| {
        let from = repo.join(target);
        let to = tree.join(target);
        if to.exists() {
            return;
        }
        std::fs::create_dir_all(to.parent().expect("parent")).expect("mkdir");
        std::os::unix::fs::symlink(&from, &to).expect("symlink");
    };
    link("docs/release-policy.md");
    link("docs/capabilities.md");
    link("docs/providers/README.md");
    link("docs/session-log-format.md");
    link("docs/providers/openai-compatible.md");
    // Gh #564: the drift net reads the reader's known types out of the store
    // module, so the fixture must link the directory's files, not a dead path.
    for part in ["mod", "rows", "lines", "read", "write", "forks"] {
        link(&format!("crates/lca-session/src/store/{part}.rs"));
    }
    link("crates/lca-permissions/src/scope.rs");
    // The README link-rot check resolves every `docs/*.md` link
    // against the tree, so the fixture tracks the README's link set
    // instead of a hardcoded copy that rots behind it.
    let readme = std::fs::read_to_string(repo.join("README.md")).expect("read README");
    let mut links: Vec<&str> = Vec::new();
    let mut rest = readme.as_str();
    while let Some(start) = rest.find("(docs/") {
        rest = &rest[start + 1..];
        let end = rest.find(')').expect("link closes");
        let mut target = &rest[..end];
        if let Some(hash) = target.find('#') {
            target = &target[..hash];
        }
        if target.ends_with(".md") && !links.contains(&target) {
            links.push(target);
        }
        rest = &rest[end..];
    }
    for target in links {
        // The explicit links above overlap the README's set; a
        // second symlink for the same path is an error, not a copy.
        if !tree.join(target).exists() {
            link(target);
        }
    }
    link("schemas/extension-manifest.schema.json");
    link("extensions/openai-compatible/resources/provider-presets.toml");
    link("AGENTS.md");
    link("README.md");
    // Gh #340's checks read these too: the SRDD's crate/ADR prose, the ADR
    // index, the crate tree itself, and every shipped manifest.
    link("docs/lca-srdd.md");
    for entry in std::fs::read_dir(repo.join("docs/adr")).expect("read the ADR directory") {
        let entry = entry.expect("ADR entry");
        let name = entry.file_name().to_string_lossy().into_owned();
        // `README.md` is already linked above and the README-link pass may have
        // reached it again; a second symlink for one path is an error, not a copy.
        if !tree.join(format!("docs/adr/{name}")).exists() {
            link(&format!("docs/adr/{name}"));
        }
    }
    // The crate-count and per-crate-table checks enumerate `crates/`. A few
    // files above already created real directories under `tree/crates`, so
    // each *missing* crate name gets its own directory symlink rather than a
    // link for the whole tree (which would also let a later `create_dir_all`
    // write through into the working copy).
    std::fs::create_dir_all(tree.join("crates")).expect("mkdir crates");
    for entry in std::fs::read_dir(repo.join("crates")).expect("read crates") {
        let entry = entry.expect("crate entry");
        let name = entry.file_name().to_string_lossy().into_owned();
        let to = tree.join("crates").join(&name);
        if !to.exists() {
            std::os::unix::fs::symlink(entry.path(), &to).expect("symlink crate");
        }
    }
    std::fs::create_dir_all(tree.join("extensions")).expect("mkdir extensions");
    for entry in std::fs::read_dir(repo.join("extensions")).expect("read extensions") {
        let entry = entry.expect("extension entry");
        let path = entry.path();
        if path.is_dir() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let from = repo.join("extensions").join(&name);
            let to = tree.join("extensions").join(&name);
            // `openai-compatible/resources/...` was linked above, so that one
            // tree already exists in the fixture.
            if !to.exists() {
                std::os::unix::fs::symlink(&from, &to).expect("symlink extension tree");
            }
        }
    }
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
        "21 presets ship today",
        "22 presets ship today",
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

#[test]
#[cfg(unix)]
fn a_dropped_record_tag_in_docs_fails_the_net() {
    let tree = fixture_tree("lca-drift-tag");
    drift_file(
        &tree,
        "docs/session-log-format.md",
        "`branch-summary`",
        "`branch-summary-x`",
    );
    let output = run_net(&tree);
    assert!(
        !output.status.success(),
        "a documented record tag deleted behind the reader's back must fail the net"
    );
    let _ = std::fs::remove_dir_all(&tree);
}

#[test]
#[cfg(unix)]
fn a_dropped_scope_in_docs_fails_the_net() {
    let tree = fixture_tree("lca-drift-scope");
    drift_file(
        &tree,
        "docs/capabilities.md",
        "| `workspace` |",
        "| `workspace-x` |",
    );
    let output = run_net(&tree);
    assert!(
        !output.status.success(),
        "a documented scope deleted behind the vocabulary's back must fail the net"
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

// ---------------------------------------------------------------------------
// Gh #340: the three checks this cycle added. Each is a drift shape that
// shipped, and each gets its own fixture so the net is seen failing on the
// exact thing it was written for.
// ---------------------------------------------------------------------------

/// The README's crate sentence and its table, plus `crates/` itself.
#[test]
#[cfg(unix)]
fn a_stale_crate_count_fails_the_net() {
    let tree = fixture_tree("lca-drift-crate-count");
    drift_file(
        &tree,
        "README.md",
        "One process, twenty-one crates",
        "One process, sixteen crates",
    );
    let output = run_net(&tree);
    assert!(
        !output.status.success(),
        "a crate count the tree disagrees with must fail the net"
    );
    let _ = std::fs::remove_dir_all(&tree);
}

#[test]
#[cfg(unix)]
fn a_crate_missing_from_the_readme_table_fails_the_net() {
    let tree = fixture_tree("lca-drift-crate-row");
    let path = tree.join("README.md");
    let repo_text = std::fs::read_to_string(root().join("README.md")).expect("read");
    let dropped = repo_text
        .lines()
        .filter(|line| !line.starts_with("| `lca-mcp` |"))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::remove_file(&path).expect("drop the symlink");
    std::fs::write(&path, dropped).expect("write the drift");
    let output = run_net(&tree);
    assert!(
        !output.status.success(),
        "a crate with no README row must fail the net"
    );
    let _ = std::fs::remove_dir_all(&tree);
}

#[test]
#[cfg(unix)]
fn a_stale_adr_range_in_the_srdd_fails_the_net() {
    let tree = fixture_tree("lca-drift-adr-range");
    drift_file(
        &tree,
        "docs/lca-srdd.md",
        "0001 through 0049",
        "0001 through 0046",
    );
    let output = run_net(&tree);
    assert!(
        !output.status.success(),
        "an ADR range the directory disagrees with must fail the net"
    );
    let _ = std::fs::remove_dir_all(&tree);
}

#[test]
#[cfg(unix)]
fn a_missing_adr_index_row_fails_the_net() {
    let tree = fixture_tree("lca-drift-adr-row");
    let path = tree.join("docs/adr/README.md");
    let repo_text = std::fs::read_to_string(root().join("docs/adr/README.md")).expect("read");
    let dropped = repo_text
        .lines()
        .filter(|line| !line.starts_with("| 0049 |"))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::remove_file(&path).expect("drop the symlink");
    std::fs::write(&path, dropped).expect("write the drift");
    let output = run_net(&tree);
    assert!(
        !output.status.success(),
        "an ADR with no index row must fail the net"
    );
    let _ = std::fs::remove_dir_all(&tree);
}

/// A shipped manifest the schema refuses - the defect this cycle fixed, in
/// both directions: the top-level key and the capability rule.
#[test]
#[cfg(unix)]
fn a_shipped_manifest_the_schema_refuses_fails_the_net() {
    let tree = fixture_tree("lca-drift-manifest");
    let path = tree.join("schemas/extension-manifest.schema.json");
    let schema = std::fs::read_to_string(root().join("schemas/extension-manifest.schema.json"))
        .expect("read the schema");
    // Drop the `resources` property the shipped manifest declares: the schema
    // is then `additionalProperties: false` with no way to accept it.
    let schema: serde_json::Value = serde_json::from_str(&schema).expect("valid JSON");
    let mut schema = schema;
    schema["properties"]
        .as_object_mut()
        .expect("properties is an object")
        .remove("resources");
    std::fs::remove_file(&path).expect("drop the symlink");
    std::fs::write(&path, serde_json::to_string(&schema).expect("serialize"))
        .expect("write the drift");
    let output = run_net(&tree);
    assert!(
        !output.status.success(),
        "a shipped manifest the normative schema refuses must fail the net"
    );
    let _ = std::fs::remove_dir_all(&tree);
}
