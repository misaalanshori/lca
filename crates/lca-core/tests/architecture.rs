//! The architecture's dependency rule, as a test (SRDD crate
//! decomposition: "`lca-core` ... sits above the domain and platform
//! crates, and only `lca-sdk` and `lca-cli` depend on it"; "No crate
//! depends on `lca-cli`"; "`lca-protocol` and `lca-config` sit at the
//! bottom with no workspace dependencies"). Drift here is invisible to
//! every behavior test, so the graph itself gets checked.

use std::path::{Path, PathBuf};

fn workspace_crates() -> Vec<(String, PathBuf)> {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates dir")
        .to_path_buf();
    let mut found = Vec::new();
    for entry in std::fs::read_dir(&crates).expect("read crates") {
        let entry = entry.expect("dir entry");
        let manifest = entry.path().join("Cargo.toml");
        if !manifest.is_file() {
            continue;
        }
        let doc: toml::Table = toml::from_str(&std::fs::read_to_string(&manifest).expect("read"))
            .unwrap_or_else(|err| panic!("{} parses: {err}", manifest.display()));
        let name = doc
            .get("package")
            .and_then(|package| package.get("name"))
            .and_then(toml::Value::as_str)
            .expect("package name")
            .to_string();
        found.push((name, manifest));
    }
    found
}

fn dependency_names(manifest: &Path) -> Vec<String> {
    let doc: toml::Table =
        toml::from_str(&std::fs::read_to_string(manifest).expect("read")).expect("manifest parses");
    doc.get("dependencies")
        .and_then(toml::Value::as_table)
        .map(|table| table.keys().cloned().collect())
        .unwrap_or_default()
}

#[test]
fn only_the_sdk_and_the_binary_depend_on_the_core() {
    for (name, manifest) in workspace_crates() {
        if name == "lca-core" {
            continue;
        }
        if dependency_names(&manifest)
            .iter()
            .any(|dep| dep == "lca-core")
        {
            assert!(
                matches!(name.as_str(), "lca-cli" | "lca-sdk"),
                "{name} depends on lca-core; the architecture allows only lca-sdk and lca-cli"
            );
        }
    }
}

#[test]
fn nothing_depends_on_the_binary() {
    for (name, manifest) in workspace_crates() {
        if name == "lca-cli" {
            continue;
        }
        assert!(
            !dependency_names(&manifest)
                .iter()
                .any(|dep| dep == "lca-cli"),
            "{name} depends on lca-cli; nothing may"
        );
    }
}

#[test]
fn the_bottom_layer_has_no_workspace_dependencies() {
    for bottom in ["lca-protocol", "lca-config"] {
        let (_, manifest) = workspace_crates()
            .into_iter()
            .find(|(name, _)| name == bottom)
            .unwrap_or_else(|| panic!("{bottom} exists"));
        let doc: toml::Table =
            toml::from_str(&std::fs::read_to_string(&manifest).expect("read")).expect("parse");
        let deps = doc
            .get("dependencies")
            .and_then(toml::Value::as_table)
            .cloned()
            .unwrap_or_default();
        for (dep, spec) in deps {
            assert!(
                spec.get("path").is_none(),
                "{bottom} depends on the workspace path {dep}; the bottom layer must not"
            );
        }
    }
}
