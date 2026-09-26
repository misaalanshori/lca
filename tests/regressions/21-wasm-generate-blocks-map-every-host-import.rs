//! An extension's `wit_bindgen::generate!` block is target-shaped: when a
//! world grows a `lca:host/*` import, every `with:` block for that world
//! has to map it, and the one that doesn't only fails at
//! `--target wasm32-wasip2`. Nothing built that target, so cycle 4's
//! `resources`/`state` imports broke `compaction-default` and `skills`
//! silently and the break surfaced at release time - the publish job's
//! component step, after the binaries had already been attached.
//!
//! The build gate is `scripts/wasm-check.sh` (the `extension components
//! build` CI job). This pins the contract underneath it: a world's host
//! imports must all be mapped wherever that world is generated.
//!
//! Verifies: docs/testing-plan.md section 13, FR-EXT-1.

use std::collections::BTreeSet;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    // `tests/regressions/` -> repo root.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The `lca:host/<name>@<version>` keys one WIT world file imports.
fn world_imports(path: &std::path::Path) -> BTreeSet<String> {
    let text = std::fs::read_to_string(path).expect("read world");
    text.split_whitespace()
        .filter(|token| token.starts_with("lca:host/"))
        .map(|token| token.trim_end_matches([';', ',']).to_string())
        .collect()
}

/// Each `(world, with-keys)` pair an extension's source generates.
fn generate_blocks(source: &str) -> Vec<(String, BTreeSet<String>)> {
    let mut out = Vec::new();
    let mut rest = source;
    while let Some(start) = rest.find("wit_bindgen::generate!({") {
        let body_start = start + "wit_bindgen::generate!({".len();
        let Some(end) = rest[body_start..].find("});") else {
            break;
        };
        let block = &rest[body_start..body_start + end];
        // Quoted literals in the block: `"../../wit"`, the world name, then
        // the `"lca:host/..."` keys. Take the keys by shape and the world
        // from its own line.
        let world = block
            .lines()
            .find_map(|line| line.trim().strip_prefix("world:"))
            .map(|value| {
                value
                    .trim()
                    .trim_end_matches(',')
                    .trim_matches('"')
                    .to_string()
            })
            .unwrap_or_default();
        let keys = block
            .split('"')
            .skip(1)
            .step_by(2)
            .filter(|literal| literal.starts_with("lca:host/"))
            .map(str::to_string)
            .collect();
        out.push((world, keys));
        rest = &rest[body_start + end..];
    }
    out
}

#[test]
fn every_world_host_import_is_mapped_wherever_that_world_is_generated() {
    let root = repo_root();
    let mut worlds: BTreeSet<String> = BTreeSet::new();
    for entry in std::fs::read_dir(root.join("wit")).expect("wit dir") {
        let path = entry.expect("entry").path();
        if path.extension().is_some_and(|ext| ext == "wit") {
            worlds.insert(
                path.file_stem()
                    .expect("stem")
                    .to_string_lossy()
                    .trim_start_matches("world-")
                    .to_string(),
            );
        }
    }

    let mut checked = 0;
    for entry in std::fs::read_dir(root.join("extensions")).expect("extensions dir") {
        let source = entry.expect("entry").path().join("src/lib.rs");
        let Ok(text) = std::fs::read_to_string(&source) else {
            continue;
        };
        for (world, mapped) in generate_blocks(&text) {
            if world.is_empty() || !worlds.contains(&world) {
                continue;
            }
            let imports = world_imports(&root.join("wit").join(format!("world-{world}.wit")));
            let missing: Vec<&String> = imports.difference(&mapped).collect();
            assert!(
                missing.is_empty(),
                "{source:?} generates world `{world}` but does not map {missing:?}; \
                 the wasm build fails only at --target wasm32-wasip2"
            );
            checked += 1;
        }
    }
    assert!(checked > 0, "no generate! blocks found; the layout moved");
}
