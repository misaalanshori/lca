//! GitHub #348: `catalog::cache_path` joined an unvalidated catalog id onto
//! `<data>/catalogs/{id}.toml`.
//!
//! `[[extensions.catalogs]]` comes from configuration, and a *trusted*
//! project file may set it (FR-PERM-9 gates the file, not the field), so a
//! repository's `.lca/config.toml` could carry
//! `id = "../../cron.d/malicious"` and the next catalog refresh wrote
//! attacker-chosen bytes wherever the user can write.
//!
//! The rule is a charset, not a traversal blocklist: non-empty ASCII
//! alphanumerics plus `-` and `_`. `.`, `/`, and `\` are simply not in the
//! vocabulary, so no encoding trick reaches the path builder. It is
//! enforced twice — `lca-config` refuses at parse time (so a hostile file
//! never produces a source at all) and `lca-registry::cache_path` refuses at
//! path-build time (so a source that arrives any other way still cannot
//! write).
//!
//! Verifies: end to end through the real `lca` binary — a project config
//! with a traversal catalog and a sentinel target outside `catalogs/` is
//! refused loudly with a non-zero exit, and the sentinel file never appears.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Every traversal / separator shape the id must refuse. The `.` forms are
/// the ones that used to escape; the rest pin that the whole path-
/// metacharacter family stays out of the vocabulary.
const HOSTILE_IDS: &[&str] = &[
    "../../cron.d/malicious",
    "../escape",
    "..",
    "sub/dir",
    "/absolute",
    "back\\slash",
    "dots.in.name",
    "with space",
    "",
    // A NUL cannot even be spelled inside a TOML string (the parser
    // rejects the escape), so it never reaches the id rule. Kept out of
    // this list deliberately; the charset unit row still refuses it for
    // callers that arrive another way.
];

// Verifies: gh #348 - the rule exists in two crates (`lca-config` parses
// the id, `lca-registry` builds the path) because the config crate must
// not depend on the registry. Two copies drift; this pins them to one
// answer across the whole vocabulary, including the boundary characters.
#[test]
fn the_two_copies_of_the_catalog_id_rule_agree() {
    let cases = [
        "",
        "a",
        "official",
        "acme-corp_2",
        "A-Z_09",
        "0",
        "..",
        ".",
        "-",
        "_",
        "../x",
        "a/b",
        "a\\b",
        "a b",
        "a.b",
        "a~b",
        "a:b",
        "*",
        "ünicode",
        "nul\0byte",
        "a-b_c.D",
        "...",
    ];
    for id in cases {
        assert_eq!(
            lca_config::is_valid_catalog_id(id),
            lca_registry::is_valid_catalog_id(id),
            "the config and registry rules disagree about `{id}`"
        );
    }
    assert_eq!(
        lca_config::CATALOG_ID_RULE,
        "a catalog id may use only ASCII letters, digits, `-`, and `_`",
        "the documented rule text is what both refusals show"
    );
}

/// The built `lca` binary, if this run also produced one (the
/// harness-absence rule, testing plan §14): look in the shared target
/// directory, and skip with a named reason when absent.
fn binary() -> Option<PathBuf> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    ["target/debug/lca", "target/release/lca"]
        .iter()
        .map(|relative| manifest.join(relative))
        .find(|path| path.is_file())
}

/// A scratch home + project. `sentinel` lives outside `catalogs/`, one
/// level up from where `catalogs/` would be, which is exactly where
/// `../../` aimed.
struct Layout {
    home: PathBuf,
    project: PathBuf,
    sentinel: PathBuf,
}

fn layout(name: &str, id: &str) -> Layout {
    let root = lca_testkit::scratch_path(&format!("regression-348-{name}"));
    let _ = std::fs::remove_dir_all(&root);
    let home = root.join("home");
    let project = root.join("project");
    std::fs::create_dir_all(home.join(".lca")).expect("mkdir home");
    std::fs::create_dir_all(project.join(".lca")).expect("mkdir project");
    // The traversal target: `~/.lca/catalogs/../../sentinel.toml` resolves
    // to `~/.lca/sentinel.toml` (three levels up from `catalogs/` is
    // `~/.lca`), so that is where the sentinel would land.
    let sentinel = home.join(".lca").join("sentinel.toml");
    std::fs::write(
        project.join(".lca/config.toml"),
        format!(
            "[[extensions.catalogs]]\nid = {id:?}\nname = \"Hostile\"\nurl = \"https://attacker.example/catalog.toml\"\n"
        ),
    )
    .expect("write config");
    // An untrusted project ignores its own config (FR-PERM-9), so the
    // attack requires trust: mark the project trusted, which is the
    // normal state for a repo someone cloned and agreed to work in.
    let grants = home.join(".lca/grants.json");
    let project_key = project.canonicalize().expect("canonical project");
    std::fs::write(
        &grants,
        format!(
            "{{\"version\":1,\"rules\":{{}},\"projects\":{{{}:{{\"trusted\":true,\"patterns\":[],\"net_patterns\":[]}}}}}}",
            serde_json::to_string(&project_key.display().to_string()).expect("json string")
        ),
    )
    .expect("write grants");
    Layout {
        home,
        project,
        sentinel,
    }
}

fn run(layout: &Layout, args: &[&str]) -> (String, String, i32) {
    let binary = binary().expect("the lca binary is built (harness-absence guard)");
    let out = Command::new(&binary)
        .args(args)
        .current_dir(&layout.project)
        .env("HOME", &layout.home)
        .env("USERPROFILE", &layout.home)
        .env("LCA_UPDATE_CHECK", "false")
        .output()
        .expect("run lca");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

/// Everything under `home` (relative paths), so a test can assert a file
/// appeared *anywhere* rather than only at the guessed target.
fn tree(home: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut stack = vec![home.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path.clone());
            }
            if let Ok(relative) = path.strip_prefix(home) {
                found.push(relative.display().to_string());
            }
        }
    }
    found
}

// Verifies: gh #348 acceptance 1 - a catalog id with traversal or
// separators is refused loudly at config load, and nothing is written
// outside `catalogs/`. Before the fix this id parsed cleanly and the
// refresh wrote `~/.lca/sentinel.toml`.
#[test]
fn a_traversal_catalog_id_is_refused_and_writes_nothing_outside_catalogs() {
    if binary().is_none() {
        eprintln!("skip: the lca binary is not built in this run (harness-absence rule)");
        return;
    }
    for id in HOSTILE_IDS {
        let layout = layout("config", id);
        let (stdout, stderr, code) = run(&layout, &["config"]);
        let combined = format!("{stdout}{stderr}");
        assert_ne!(
            code, 0,
            "a hostile catalog id `{id}` must fail the load:\n{combined}"
        );
        // Either the id rule names the key, or TOML itself refuses the
        // literal before the rule sees it (an unspellable escape). Both are
        // a loud load failure with nothing written; both are acceptable.
        assert!(
            combined.contains("catalog id") || combined.contains("not valid TOML"),
            "the refusal names the cause for `{id}`:\n{combined}"
        );
        // Nothing written outside the catalogs directory. The sentinel is
        // the specific target `../..` aimed at; the whole-tree scan catches
        // any other escape (a stray `cron.d/`, a sibling project file).
        assert!(
            !layout.sentinel.exists(),
            "`{id}` escaped the cache directory and wrote {}",
            layout.sentinel.display()
        );
        let files = tree(&layout.home);
        assert!(
            !files.iter().any(|path| path.contains("sentinel")),
            "`{id}` wrote a traversal target somewhere under the home: {files:?}"
        );
        assert!(
            !files.iter().any(|path| path.contains("cron.d")),
            "`{id}` created an out-of-place directory: {files:?}"
        );
    }
}

// Verifies: gh #348 acceptance 4 - the privileged `update` path (which
// builds cache paths directly, bypassing the config load's refusal) also
// refuses rather than joining. Exercised with `update --models`, the
// catalog-refresh job that used to call `cache_path` unguarded.
#[test]
fn the_catalog_refresh_path_refuses_a_hostile_id() {
    if binary().is_none() {
        eprintln!("skip: the lca binary is not built in this run (harness-absence rule)");
        return;
    }
    // `update --models` reads only the user file, so the hostile id is
    // placed there: the same untrusted-input shape, one layer that the
    // project-trust gate does not cover.
    let layout = layout("update", "acme");
    std::fs::write(
        layout.home.join(".lca/config.toml"),
        "[[extensions.catalogs]]\nid = \"../../sentinel\"\nname = \"Hostile\"\nurl = \"https://attacker.example/c.toml\"\n",
    )
    .expect("write user config");
    let (stdout, stderr, code) = run(&layout, &["update", "--models"]);
    let combined = format!("{stdout}{stderr}");
    assert_ne!(
        code, 0,
        "a hostile id in the user file fails the refresh:\n{combined}"
    );
    assert!(
        combined.contains("catalog id") || combined.contains("not valid TOML"),
        "the refusal names the cause:\n{combined}"
    );
    assert!(
        !layout.sentinel.exists(),
        "the refresh wrote outside the cache directory"
    );
}

// Verifies: gh #348 - a legitimate id still works end to end, so existing
// configurations do not brick. The load succeeds and the catalog reads
// back with its own id (the refresh cannot reach the network here, so only
// the config half is asserted).
#[test]
fn a_legitimate_catalog_id_still_loads() {
    if binary().is_none() {
        eprintln!("skip: the lca binary is not built in this run (harness-absence rule)");
        return;
    }
    let layout = layout("valid", "acme-corp_2");
    let (stdout, stderr, code) = run(&layout, &["config"]);
    let combined = format!("{stdout}{stderr}");
    assert_eq!(
        code, 0,
        "a legitimate id must not fail the load:\n{combined}"
    );
    assert!(
        !combined.contains("catalog id"),
        "no refusal for a valid id:\n{combined}"
    );
}
