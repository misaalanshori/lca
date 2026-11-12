//! The fixtures these rows share: a probe component, an install tree, the two
//! loopback mock registries, and the git-fixture helpers.
//!
//! Gh #561 split `tests/registry.rs` (1,140 lines) by the subjects it and the crate
//! already had. These were the only shared items in the file, so they live here;
//! `component` in `tests/archive_ceilings.rs` is a *different* fixture (a 3-line
//! magic blob for the bomb rows), so this is not a dedupe of a copy - it is one
//! file's own fixtures being shared inside its new siblings.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

pub(super) const MANIFEST: &str = r#"name = "word-count"
version = "1.0.0"
abi = "0.4"
worlds = ["tool"]
description = "Counts words."

[capabilities.process]
reason = "Runs wc for you."
"#;

pub(super) const MANIFEST_REORDERED: &str = r#"abi = "0.4"
description = "Counts words."
name = "word-count"
version = "1.0.0"
worlds = ["tool"]

[capabilities.process]
reason = "Runs wc for you."
"#;

pub(super) const MANIFEST_WIDENED: &str = r#"name = "word-count"
version = "1.1.0"
abi = "0.4"
worlds = ["tool"]
description = "Counts words."

[capabilities.process]
reason = "Runs wc for you."

[capabilities.net]
hosts = ["api.example.com"]
"#;

pub(super) const MANIFEST_WITH_RESOURCES: &str = r#"name = "word-count"
version = "1.0.0"
abi = "0.4"
worlds = ["tool"]
description = "Counts words."
resources = ["presets", "skills"]

[capabilities.process]
reason = "Runs wc for you."
"#;

pub(super) fn component() -> Vec<u8> {
    // Not a real component: the resolvers treat it as opaque bytes with
    // a content digest, which is all the rules under test care about.
    b"\0asm\x01\0\0\0-lca-test-component".to_vec()
}

pub(super) fn tree(name: &str) -> lca_registry::InstallTree {
    let root = lca_testkit::scratch_path(&format!("lca-registry-{name}"));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("mkdir");
    lca_registry::InstallTree::new(root)
}

/// A local anonymous OCI registry: config blob carries the manifest,
/// layer0 the component, digests honored (our publishing convention).
pub(super) async fn mock_oci(broken_digests: bool) -> (String, tokio::task::JoinHandle<()>) {
    mock_oci_serving(MANIFEST, broken_digests).await
}

pub(super) async fn mock_oci_serving(
    manifest: &str,
    broken_digests: bool,
) -> (String, tokio::task::JoinHandle<()>) {
    use sha2::{Digest, Sha256};
    let manifest_toml = manifest.as_bytes().to_vec();
    let component_bytes = component();
    let config_digest = format!("sha256:{:x}", Sha256::digest(&manifest_toml));
    let layer_digest = format!("sha256:{:x}", Sha256::digest(&component_bytes));
    let claimed_layer = if broken_digests {
        format!("sha256:{:x}", Sha256::digest(b"something else entirely"))
    } else {
        layer_digest.clone()
    };
    let image_manifest = serde_json::json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "config": {
            "mediaType": "text/plain",
            "digest": config_digest,
            "size": manifest_toml.len(),
        },
        "layers": [{
            "mediaType": "application/wasm",
            "digest": claimed_layer,
            "size": component_bytes.len(),
        }],
    })
    .to_string();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let handle = tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let mut buf = vec![0u8; 16384];
            let mut read = 0;
            loop {
                match stream.read(&mut buf[read..]).await {
                    Ok(0) => break,
                    Ok(n) => {
                        read += n;
                        if buf[..read].windows(4).any(|w| w == b"\r\n\r\n") {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            let request = String::from_utf8_lossy(&buf[..read]).into_owned();
            let target = request.split_whitespace().nth(1).unwrap_or("/").to_string();
            let (body, content_type) = if target.contains("/manifests/") {
                (
                    image_manifest.clone().into_bytes(),
                    "application/vnd.oci.image.manifest.v1+json",
                )
            } else if target.contains(&config_digest) {
                (manifest_toml.clone(), "text/plain")
            } else if target.contains("/blobs/") {
                (component_bytes.clone(), "application/wasm")
            } else {
                (b"{}".to_vec(), "application/json")
            };
            let head = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: {}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                content_type,
                body.len()
            );
            let _ = stream.write_all(head.as_bytes()).await;
            let _ = stream.write_all(&body).await;
        }
    });
    (format!("{addr}/example/word-count"), handle)
}

// Gh #59 helpers: a local git repo with two commits (v1 then v2 of
// one component package), so git-source tests run offline. `None`
// when git is missing: the resolver names the fix instead.
pub(super) fn git_repo(name: &str) -> Option<std::path::PathBuf> {
    if std::process::Command::new("git")
        .arg("--version")
        .output()
        .is_err()
    {
        return None;
    }
    let root = lca_testkit::scratch_path(&format!("lca-registry-{name}"));
    let _ = std::fs::remove_dir_all(&root);
    let dir = root.join("repo");
    std::fs::create_dir_all(&dir).expect("mkdir");
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(&dir)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .expect("git runs");
        assert!(out.status.success(), "git {args:?}: {:?}", out.stderr);
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    git(&["init", "-q"]);
    std::fs::write(dir.join("extension.toml"), MANIFEST).expect("manifest");
    std::fs::write(dir.join("word-count.wasm"), b"v1-bytes").expect("component");
    git(&["add", "."]);
    git(&["commit", "-qm", "v1"]);
    std::fs::write(dir.join("word-count.wasm"), b"v2-bytes-longer").expect("component");
    git(&["add", "."]);
    git(&["commit", "-qm", "v2"]);
    Some(dir)
}

pub(super) fn head_sha(dir: &std::path::Path) -> String {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(dir)
        .output()
        .expect("git runs");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}
