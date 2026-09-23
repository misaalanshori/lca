//! Real-distribution smoke (the Phase 5 exit test's live half): pulls
//! the published reference extensions from ghcr.io and from the release
//! asset over plain HTTP(S), the way a clean machine does.
//!
//! Offline like everything else by default: without
//! `LCA_REAL_REGISTRY=1` it skips cleanly (NFR-23's rule applied to
//! this credential-free but network-backed check).

use std::process::{Command, Stdio};

/// The published artifacts (the release workflow's naming, ADR-0010's
/// two source kinds).
const OCI_REF: &str = "ghcr.io/misaalanshori/lca/openai-compatible:abi-0.1";
const ARCHIVE_URL: &str =
    "https://github.com/misaalanshori/lca/releases/download/phase5-0.1.0/skills-abi-0.1.zip";

#[test]
fn installs_from_the_published_oci_reference_and_https_archive() {
    if std::env::var_os("LCA_REAL_REGISTRY").is_none() {
        eprintln!("skipping: LCA_REAL_REGISTRY is not set (NFR-23)");
        return;
    }
    let root = std::env::temp_dir().join(format!("lca-real-registry-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let home = root.join("home");
    let data = root.join("data");
    let project = root.join("project");
    for dir in [&home, &data, &project] {
        std::fs::create_dir_all(dir).expect("mkdir");
    }

    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_lca"))
            .args(args)
            .current_dir(&project)
            .env("HOME", &home)
            .env("USERPROFILE", &home)
            .env("XDG_DATA_HOME", &data)
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                use std::io::Write as _;
                child
                    .stdin
                    .as_mut()
                    .expect("piped stdin")
                    .write_all(b"y\n")?;
                child.wait_with_output()
            })
            .expect("run lca")
    };

    let oci = run(&["ext", "install", OCI_REF]);
    assert_eq!(oci.status.code(), Some(0), "oci: {}", {
        let mut text = String::new();
        text.push_str(&String::from_utf8_lossy(&oci.stdout));
        text.push_str(&String::from_utf8_lossy(&oci.stderr));
        text
    });
    let archive = run(&["ext", "install", ARCHIVE_URL]);
    assert_eq!(archive.status.code(), Some(0), "archive stderr: {}", String::from_utf8_lossy(&archive.stderr));

    let list = run(&["ext", "list"]);
    assert_eq!(list.status.code(), Some(0));
    let text = String::from_utf8_lossy(&list.stdout).into_owned();
    assert!(text.contains("openai-compatible"), "{text}");
    assert!(text.contains("skills"), "{text}");

    let lock = std::fs::read_to_string(data.join("lca").join("lockfile.json"))
        .expect("lockfile after both installs");
    assert!(lock.contains("sha256:"), "{lock}");
    assert!(lock.contains("ghcr.io/misaalanshori/lca/openai-compatible"), "{lock}");

    let _ = std::fs::remove_dir_all(&root);
}
