//! Gh #591 - two committed components had gone stale, and the gate that exists to
//! catch that covered **one** of them.
//!
//! `scripts/wasm-check.sh` verified `extensions/conformance/fixtures/tool-world.wasm`
//! against a fresh build (gh #427's rule: compare the **interface**, never the bytes)
//! and hard-coded that single path. Every other committed component was covered by
//! nothing, and by 2026-11-10 two had drifted: a fresh build of `extensions/codex` and
//! of `extensions/grok` imports `lca:host/resources@0.6.0` and the `net` interface's
//! `response-headers`, while the committed files import neither.
//!
//! **A load test cannot catch this class**, which this cycle measured rather than
//! assumed: a scratch probe loaded every committed fixture through `ExtHost::load` -
//! including the two stale ones - and all seven reported `ok`, because the stale file
//! is a **subset** of the fresh one (the interface diff has 12 added lines and **0**
//! removed). A 0.6 host links a guest that imports less than it could; only a
//! comparison against the same crate's fresh build sees the difference. That is why the
//! guard is the script's interface diff and not a generalised `fixture_current.rs`.
//!
//! Rows here are structural on purpose: the comparison itself needs a wasm toolchain
//! and a full component build, which the `extension components build` CI job runs on
//! every push (and which `docs/testing-plan.md` §13 names). What has to be pinned in
//! Rust is that the coverage stays **derived** and that the failure modes stay honest -
//! a gate that silently degrades to "nothing checked" is the defect class this issue
//! is about.
//!
//! Verifies: gh #591, gh #427, FR-EXT-1.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn script() -> String {
    std::fs::read_to_string(root().join("scripts/wasm-check.sh")).expect("read wasm-check.sh")
}

/// Every committed component in the tree, `(extension dir, wasm path)`, sorted.
fn committed_components() -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    let extensions = root().join("extensions");
    for entry in std::fs::read_dir(&extensions).expect("read extensions/") {
        let dir = entry.expect("extension entry").path();
        let fixtures = dir.join("fixtures");
        if !fixtures.is_dir() {
            continue;
        }
        let mut wasms: Vec<PathBuf> = std::fs::read_dir(&fixtures)
            .expect("read fixtures/")
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.extension().is_some_and(|ext| ext == "wasm"))
            .collect();
        wasms.sort();
        if wasms.is_empty() {
            continue;
        }
        let name = dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .expect("a directory name");
        for wasm in wasms {
            out.push((name.clone(), wasm));
        }
    }
    out.sort();
    out
}

/// The glob the script's loop uses, as a plain string (the row greps for it).
const DERIVED_GLOB: &str = "extensions/*/fixtures/*.wasm";

// Verifies: gh #591 - **the fixture list is derived, not one hard-coded path.**
//
// The pre-fix script had `fixture=extensions/conformance/fixtures/tool-world.wasm`
// and compared only that file. Mutation: putting that line back (or narrowing the glob
// to `extensions/conformance/fixtures/*.wasm`) reddens this row.
#[test]
fn the_fixture_check_derives_its_list_from_the_tree() {
    let script = script();
    assert!(
        script.contains(&format!("for fixture in {DERIVED_GLOB}; do")),
        "the loop walks the derived glob ({DERIVED_GLOB}), not a literal path"
    );
    assert!(
        !script.contains("fixture=extensions/"),
        "no single fixture path is assigned any more (that is exactly the hole gh #591 closed)"
    );
    // The other half of "derived": the artifact name comes from the directory, so a
    // extension added later is covered without editing the script.
    assert!(
        script.contains("${extension//-/_}.wasm"),
        "the fresh-build path is computed from the extension directory name"
    );
}

// Verifies: gh #591 - **every committed component in the tree is reached by the glob.**
//
// This is the coverage claim itself, checked against the tree rather than against the
// script's intent: the seven committed components today (conformance among them) all
// match the pattern the loop walks. A new `extensions/x/fixtures/component.wasm` is
// therefore covered the moment it lands, and this row's count keeps the number honest.
#[test]
fn the_derived_glob_reaches_every_committed_component() {
    let components = committed_components();
    assert!(
        components.len() >= 7,
        "the tree still ships its committed fixtures ({} found)",
        components.len()
    );
    let names: Vec<&str> = components.iter().map(|(name, _)| name.as_str()).collect();
    for expected in [
        "antigravity",
        "codex",
        "conformance",
        "grok",
        "openai-compatible",
        "skills",
        "tool-legacy",
    ] {
        assert!(
            names.contains(&expected),
            "the component set includes {expected} (gh #591 refreshed its fixture): {names:?}"
        );
    }
    // Every one of them sits at `<extension>/fixtures/<file>.wasm`, which is what the
    // script's glob matches - a fixture anywhere else would be silently uncovered.
    for (name, path) in &components {
        let tail = Path::new("extensions").join(name).join("fixtures");
        assert!(
            path.starts_with(root().join(&tail)),
            "{} lives under extensions/{name}/fixtures, where the glob reaches it",
            path.display()
        );
    }
}

// Verifies: gh #591 (gh #427's rule, kept) - **the comparison is interface-level.**
//
// `wasm-tools component wit` on both sides; no byte comparison of the committed binary
// (the profile is `lto = "fat"`, `codegen-units = 1`, so a hosted runner produces a
// different file for the same component - a gate that reddens on codegen noise is worse
// than none). Mutation: replacing the `component wit` pair with `cmp`/`sha256sum`
// reddens this row.
#[test]
fn the_comparison_is_the_interface_not_the_bytes() {
    let script = script();
    assert!(
        script.contains("component wit"),
        "the check reads a component interface"
    );
    assert_eq!(
        script.matches("component wit").count(),
        2,
        "once for the committed file, once for the fresh build"
    );
    for forbidden in ["cmp ", "sha256sum", "md5sum", "diff --binary"] {
        assert!(
            !script.contains(forbidden),
            "no byte-level comparison ({forbidden}) - an interface diff is what a link failure is about"
        );
    }
    // The reason must survive in the comment, or the next person re-tries bytes.
    assert!(
        script.contains("Compared by **interface**, not by bytes"),
        "the why stays written down beside the check"
    );
}

// Verifies: gh #591 - **the missing-tool branch skips with a reason, never silently.**
//
// `wasm-tools` absent must (a) not exit non-zero for a reason that is not a failure,
// (b) name what went unchecked, and (c) point at the check that still runs. A branch
// that printed nothing would be the silent-pass shape this issue is about, so the row
// asserts the words.
#[test]
fn the_missing_tool_branch_says_what_went_unchecked() {
    let script = script();
    let branch = script
        .find("if [ -z \"$wasm_tools\" ]; then\n    # Skip, never pass silently")
        .expect("the skip branch exists, with its own comment");
    let body = &script[branch..];
    assert!(
        body.contains("UNCHECKED"),
        "it says the fixtures went unchecked:\n{}",
        &body[..body.len().min(400)]
    );
    assert!(
        body.contains("fixture_current.rs"),
        "and it names the guard that still runs"
    );
    assert!(
        body.contains("exit 0"),
        "an unavailable tool is not a failing tree - it is an unverified one, said out loud"
    );
}

// Verifies: gh #591 - **a failure names the artifact, and every check is announced.**
//
// Each compared fixture prints a line on success, and a mismatch prints the fixture path,
// the extension, the interface diff and the exact `cp` that repairs it. The pre-fix script
// named only the conformance file; the reason this matters is the issue: two stale
// fixtures sat unnoticed because nothing ever mentioned them.
#[test]
fn a_mismatch_names_the_component_and_every_check_is_printed() {
    let script = script();
    assert!(
        script.contains(r#"echo "wasm-check: $extension fixture matches a fresh build""#),
        "each compared component is announced by name"
    );
    assert!(
        script
            .contains(r#"echo "wasm-check: $fixture no longer matches its source (gh #427/#591)""#),
        "a mismatch names the committed file"
    );
    assert!(
        script.contains(r#"echo "  reinstall it: cp $artifact $fixture""#),
        "and gives the one command that repairs it, with both paths"
    );
    assert!(
        script.contains("all $checked committed components match their sources"),
        "the summary counts what was actually compared"
    );
    // Skipping an unbuilt directory must say so as well - `2>/dev/null` style silence
    // around a whole extension is the failure mode.
    assert!(
        script.contains("skipping $extension/"),
        "a directory with no fresh build is named, not silently dropped"
    );
}
