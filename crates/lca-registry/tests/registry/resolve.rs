//! The four resolvers (FR-DIST-1/5/9): a local path, an HTTPS archive, an OCI
//! reference through the token dance, and the shorthand expansion.
//!
//! Split out of `tests/registry.rs`; the two loopback mock registries live in
//! `support.rs` with the other shared fixtures.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use super::support::{MANIFEST, component, mock_oci};
use lca_registry::{resolve, resolve_archive, resolve_local, resolve_oci};

// Verifies: FR-DIST-5 - a local path install reads both files and
// applies the same downstream consent flow (identical Resolved shape).
#[test]
fn a_local_path_resolves_to_the_same_shape() {
    let dir = lca_testkit::scratch_path("lca-registry-local");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(dir.join("extension.toml"), MANIFEST).expect("manifest");
    std::fs::write(dir.join("component.wasm"), component()).expect("component");

    let resolved = resolve_local(&dir.join("component.wasm"), None).expect("resolve");
    assert_eq!(resolved.manifest, MANIFEST);
    assert_eq!(resolved.component, component());
    assert_eq!(
        resolved.digest,
        lca_registry::Resolved::digest_of(&component())
    );
    assert!(resolved.source.ends_with("component.wasm"));
    let _ = std::fs::remove_dir_all(&dir);
}

// Verifies: FR-DIST-9 - the plain HTTPS(-style) archive source fetches,
// unpacks, and digests exactly like the OCI path does.
#[tokio::test]
async fn an_archive_url_resolves_through_the_shared_path() {
    let packed = lca_registry::pack_archive(MANIFEST, &component()).expect("pack");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let mut buf = vec![0u8; 8192];
            let _ = stream.read(&mut buf).await;
            let head = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/zip\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                packed.len()
            );
            let _ = stream.write_all(head.as_bytes()).await;
            let _ = stream.write_all(&packed).await;
        }
    });
    let url = format!("http://{addr}/word-count-abi-0.4.zip");
    let resolved = resolve_archive(&url).await.expect("resolve");
    assert_eq!(resolved.manifest, MANIFEST);
    assert_eq!(resolved.component, component());
    assert_eq!(
        resolved.digest,
        lca_registry::Resolved::digest_of(&component())
    );
    assert_eq!(resolved.source, url);

    // Dispatch: the same fn for any https URL.
    let via_dispatch = resolve(&url, None).await.expect("dispatch");
    assert_eq!(via_dispatch, resolved);
}

// Verifies: FR-DIST-2/FR-DIST-4 (a download that cannot finish leaves the
// installed version working: resolution returns a whole component or an
// error, and only a complete resolve reaches the tree).
#[tokio::test]
async fn an_interrupted_download_leaves_the_installed_version_working() {
    let packed = lca_registry::pack_archive(MANIFEST, &component()).expect("pack");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let mut buf = vec![0u8; 8192];
            let _ = stream.read(&mut buf).await;
            // Promise the whole archive, deliver half, then close.
            let head = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/zip\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                packed.len()
            );
            let _ = stream.write_all(head.as_bytes()).await;
            let _ = stream.write_all(&packed[..packed.len() / 2]).await;
            let _ = stream.shutdown().await;
        }
    });

    // Install v1 from a complete copy.
    let root = lca_testkit::scratch_path("lca-registry-interrupted");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("mkdir");
    let tree = lca_registry::InstallTree::new(root);
    let digest = lca_registry::Resolved::digest_of(&component());
    tree.install(lca_registry::Resolved {
        digest: digest.clone(),
        source: "https://example.invalid/word-count.zip".to_string(),
        commit: String::new(),
        manifest: MANIFEST.to_string(),
        component: component(),
        resources: Vec::new(),
    })
    .expect("install v1");

    // The interrupted update fails without touching the tree.
    let url = format!("http://{addr}/word-count-abi-0.4.zip");
    let err = resolve_archive(&url)
        .await
        .expect_err("the interrupted download fails");
    assert!(!err.to_string().is_empty(), "a real error, not a panic");
    let entry = tree
        .entry("word-count")
        .expect("entry")
        .expect("the old version is still installed");
    assert_eq!(entry.digest, digest, "the installed version is untouched");
}

// Verifies: FR-DIST-1 (OCI with the built-in client, no external tool -
// FR-DIST-2) and FR-DIST-3: both digests are verified before the
// result is returned, and a lying registry is refused (FR-DIST-4's
// trigger: nothing reaches the store).
#[tokio::test]
async fn oci_resolution_verifies_both_digests() {
    let (reference, server) = mock_oci(false).await;
    let resolved = resolve_oci(&format!("{reference}:abi-0.4"))
        .await
        .expect("resolve");
    assert_eq!(resolved.manifest, MANIFEST);
    assert_eq!(resolved.component, component());
    assert_eq!(
        resolved.digest,
        lca_registry::Resolved::digest_of(&component())
    );
    assert_eq!(resolved.source, format!("{reference}:abi-0.4"));

    // The default tag when the reference carries none (moving-tag
    // installs spell theirs out; bare names resolve to `latest`).
    let resolved = resolve_oci(&reference).await.expect("default tag");
    assert_eq!(resolved.component, component());
    server.abort();

    // A registry whose layer digest does not match the bytes it then
    // serves: refused at resolve time, so FR-DIST-4's delete never has
    // to happen - nothing was written.
    let (reference, server) = mock_oci(true).await;
    let err = resolve_oci(&format!("{reference}:abi-0.4"))
        .await
        .expect_err("digest lie");
    assert!(
        matches!(err, lca_registry::Error::DigestMismatch { .. }),
        "{err}"
    );
    server.abort();
}

// Verifies: reference splitting survives registry hosts with ports and
// untagged names (the update path rebuilds tags from these parts).
// Gh #252: bare names now expand instead of erroring, so the error
// case is the nameless `:tag`.
#[test]
fn oci_references_split_host_name_and_tag() {
    // Exercised through resolve_local's absence here: the splitter is
    // private, so its behavior shows up as successful resolves above
    // (mock server addresses carry host:port) and clear errors below.
    let err = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(async { resolve_oci(":tag").await })
        .expect_err("no name");
    assert!(err.to_string().contains("not an OCI reference"), "{err}");
}

// Verifies: FR-CFG-6 (the update check's transport: plain_get fetches
// over the same closed-stack client the OCI path uses, with the header
// the API demands; the check's once-a-day decisions are covered in
// crates/lca-cli/tests/update_check.rs).
#[tokio::test]
async fn plain_get_fetches_a_body_over_the_shared_client() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let mut buf = vec![0u8; 8192];
            let _ = stream.read(&mut buf).await;
            let payload = br#"{"tag_name":"phase6-0.2.0"}"#;
            let head = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                payload.len()
            );
            let _ = stream.write_all(head.as_bytes()).await;
            let _ = stream.write_all(payload).await;
        }
    });
    let body = lca_registry::plain_get(&format!("http://{addr}/latest"))
        .await
        .expect("get");
    assert_eq!(body, br#"{"tag_name":"phase6-0.2.0"}"#);
}

// Verifies: FR-DIST-5 / ADR-0030 (a directory installs as a data-only
// package: manifest plus `resources/`, no component).
#[test]
fn a_local_directory_resolves_as_a_data_only_package() {
    let root = lca_testkit::scratch_path("lca-registry-data-local");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("resources/skills/pack")).expect("mkdir");
    std::fs::write(
        root.join("extension.toml"),
        "name = \"skill-pack\"\nversion = \"1.0.0\"\nabi = \"0.4\"\nworlds = []\nresources = [\"skills\"]\n",
    )
    .expect("manifest");
    std::fs::write(
        root.join("resources/skills/pack/SKILL.md"),
        "name: pack\n---\nbody",
    )
    .expect("skill");

    let resolved = resolve_local(&root, None).expect("resolve");
    assert!(resolved.component.is_empty());
    assert_eq!(resolved.resources.len(), 1);
    assert_eq!(resolved.resources[0].0, "skills/pack/SKILL.md");
}

// Verifies: gh #252 - bare names expand to the official repository
// (default tag tracks the ABI line); explicit refs pass through.
#[test]
fn bare_names_expand_to_the_official_repository() {
    assert_eq!(
        lca_registry::expand_shorthand("antigravity"),
        "ghcr.io/misaalanshori/lca/antigravity:abi-0.6"
    );
    assert_eq!(
        lca_registry::expand_shorthand("antigravity:unstable"),
        "ghcr.io/misaalanshori/lca/antigravity:unstable"
    );
    assert_eq!(
        lca_registry::expand_shorthand("antigravity:1.0.1"),
        "ghcr.io/misaalanshori/lca/antigravity:1.0.1"
    );
    for untouched in [
        "ghcr.io/misaalanshori/lca/antigravity:abi-0.6",
        "quay.io/team/ext:1.0",
        "https://example.invalid/ext.zip",
        "./path/to/ext.wasm",
        "git+https://example.invalid/repo",
    ] {
        assert_eq!(lca_registry::expand_shorthand(untouched), untouched);
    }
}
