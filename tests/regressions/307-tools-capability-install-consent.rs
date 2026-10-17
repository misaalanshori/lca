//! GitHub #307: `lca_registry::consent_lines` enumerated the capability
//! vocabulary with a 9-arm match and a wildcard error arm. The `tools`
//! capability (ABI 0.6, gh #77) landed in the WIT, in the host's manifest
//! parser, and in `docs/capabilities.md`, but never got its consent arm —
//! so installing ANY extension that declared `[capabilities.tools]` failed
//! immediately with `invalid artifact: unknown capability 'tools' in the
//! manifest`, before any consent could be shown.
//!
//! The root-cause class is vocabulary drift between mirrors. The fix is the
//! arm plus a schema-vocabulary guard test (`crates/lca-registry`) that
//! turns red at the vocabulary's growth point.
//!
//! Verifies: end to end through the real `lca ext install` binary — a local
//! data-only package declaring `[capabilities.tools]` installs, and its
//! consent text names what the capability lets it do.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

// The CLI round trip is Unix-gated like its siblings (the harness spawns a
// POSIX shell). The consent vocabulary itself is pinned portably by
// `crates/lca-registry`'s `every_schema_capability_has_a_consent_line` and
// `the_tools_capability_has_its_consent_sentence`.
#[cfg(unix)]
mod unix {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    /// The built `lca` binary, or `None` when this run did not build one
    /// (the harness-absence rule, testing plan §14).
    fn binary() -> Option<PathBuf> {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        ["target/debug/lca", "target/release/lca"]
            .iter()
            .map(|relative| manifest.join(relative))
            .find(|path| path.is_file())
    }

    fn run(home: &Path, project: &Path, args: &[&str]) -> (String, String, i32) {
        let binary = binary().expect("the lca binary is built (harness-absence guard)");
        let out = Command::new(&binary)
            .args(args)
            .current_dir(project)
            .env("HOME", home)
            .env("USERPROFILE", home)
            .env("LCA_UPDATE_CHECK", "false")
            .output()
            .expect("run lca");
        (
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
            out.status.code().unwrap_or(-1),
        )
    }

    /// A data-only package (ADR-0030) declaring the `tools` capability: a
    /// `resources` bag with one skill, so no component is needed.
    fn write_tools_package(root: &Path) -> PathBuf {
        let pack = root.join("pack");
        std::fs::create_dir_all(pack.join("resources/skills/demo")).expect("mkdir pack");
        std::fs::write(
            pack.join("extension.toml"),
            r#"name = "tools-pack"
version = "1.0.0"
abi = "0.6"
worlds = ["tool"]
description = "A package that asks for the tools capability."
resources = ["skills"]

[capabilities.tools]
reason = "Chains a second opinion through another tool."
"#,
        )
        .expect("write manifest");
        std::fs::write(
            pack.join("resources/skills/demo/SKILL.md"),
            "# demo\nA skill file, so the package is a real data-only bag.\n",
        )
        .expect("write skill");
        pack
    }

    // Verifies: gh #307 AC1 + AC2 - the package installs and the consent
    // text names what the capability lets it do (before the fix this was
    // `invalid artifact: unknown capability 'tools' in the manifest`).
    #[test]
    fn a_tools_capability_package_installs_with_its_consent_text() {
        if binary().is_none() {
            eprintln!("skip: the lca binary is not built in this run (harness-absence rule)");
            return;
        }
        let root = lca_testkit::scratch_path("gh307-tools-consent");
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join("home");
        let project = root.join("project");
        std::fs::create_dir_all(&home).expect("mkdir home");
        std::fs::create_dir_all(&project).expect("mkdir project");
        let pack = write_tools_package(&root);

        let pack_arg = pack.display().to_string();
        let (stdout, stderr, code) = run(&home, &project, &["ext", "install", &pack_arg, "--yes"]);
        let combined = format!("{stdout}{stderr}");

        assert_eq!(
            code, 0,
            "a `tools` capability package installs instead of being refused: {combined}"
        );
        assert!(
            !combined.contains("unknown capability"),
            "no unknown-capability refusal: {combined}"
        );
        assert!(
            combined.contains("list, activate, and call other tools in the session"),
            "the consent text names what `tools` lets it do: {combined}"
        );
        assert!(
            combined.contains("Chains a second opinion through another tool."),
            "the declared reason is shown verbatim: {combined}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
