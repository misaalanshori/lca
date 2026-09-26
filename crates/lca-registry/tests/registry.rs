//! The distribution rules with receipts: canonical grant hashes, the
//! lockfile tree, the ADR-0010 archive, consent text, and both network
//! resolvers against local mock servers (offline, deterministic -
//! testing plan sections4-5).

use lca_registry::{grant_hash, resolve, resolve_archive, resolve_local, resolve_oci};

const MANIFEST: &str = r#"name = "word-count"
version = "1.0.0"
abi = "0.1"
worlds = ["tool"]
description = "Counts words."

[capabilities.process]
reason = "Runs wc for you."
"#;

const MANIFEST_REORDERED: &str = r#"abi = "0.1"
description = "Counts words."
name = "word-count"
version = "1.0.0"
worlds = ["tool"]

[capabilities.process]
reason = "Runs wc for you."
"#;

const MANIFEST_WIDENED: &str = r#"name = "word-count"
version = "1.1.0"
abi = "0.1"
worlds = ["tool"]
description = "Counts words."

[capabilities.process]
reason = "Runs wc for you."

[capabilities.net]
hosts = ["api.example.com"]
"#;

fn component() -> Vec<u8> {
    // Not a real component: the resolvers treat it as opaque bytes with
    // a content digest, which is all the rules under test care about.
    b"\0asm\x01\0\0\0-lca-test-component".to_vec()
}

fn tree(name: &str) -> lca_registry::InstallTree {
    let root = lca_testkit::scratch_path(&format!("lca-registry-{name}"));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("mkdir");
    lca_registry::InstallTree::new(root)
}

// Verifies: the FR-DIST-7 comparison basis - the hash covers the
// manifest-declared grant set only, is insensitive to the order the
// file writes them in, and changes the moment a value changes.
#[test]
fn the_grant_hash_is_canonical_and_value_sensitive() {
    let a = grant_hash(MANIFEST).expect("hash");
    let b = grant_hash(MANIFEST_REORDERED).expect("hash");
    assert_eq!(a, b, "same grants, different written order, same hash");
    assert!(a.starts_with("sha256:"));

    let widened = grant_hash(MANIFEST_WIDENED).expect("hash");
    assert_ne!(a, widened, "a new capability changes the hash");

    let reason_changed = grant_hash(&MANIFEST.replace("Runs wc", "Runs other")).expect("hash");
    assert_ne!(a, reason_changed, "a changed declaration changes the hash");
}

// Verifies: the install boundary never joins an attacker-supplied
// manifest name onto the filesystem (FR-DIST-5 consent path; ADR-0010's
// unzip). A path-traversal name is refused before any write, and no
// directory escapes the tree.
#[test]
fn install_refuses_a_manifest_name_that_escapes_the_tree() {
    let root = lca_testkit::scratch_path("lca-registry-traversal");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("mkdir");
    let tree = lca_registry::InstallTree::new(root.clone());

    let component = component();
    let resolved = lca_registry::Resolved {
        digest: lca_registry::Resolved::digest_of(&component),
        source: "https://example.invalid/evil.zip".to_string(),
        manifest: MANIFEST.replace("word-count", "../../escape"),
        component,
    };
    let err = tree
        .install(resolved)
        .expect_err("traversal must be refused");
    assert!(
        err.to_string().contains("not a valid extension name"),
        "{err}"
    );
    // Nothing was written two levels above the tree.
    assert!(
        !root.parent().expect("parent").join("escape").exists(),
        "nothing escaped the install tree"
    );
}

// Verifies: FR-DIST-6 (digest and source recorded, reused) and the
// SRDD install tree: component named by content digest, manifest
// beside it, lockfile at the top; remove forgets both (FR-DIST... the
// ext remove command's backing store).
#[test]
fn install_records_by_digest_and_roundtrips() {
    let tree = tree("install");
    let digest = lca_registry::Resolved::digest_of(&component());
    let resolved = lca_registry::Resolved {
        manifest: MANIFEST.to_string(),
        component: component(),
        digest: digest.clone(),
        source: "ghcr.io/example/word-count:abi-0.1".to_string(),
    };
    let entry = tree.install(resolved).expect("install");
    assert_eq!(entry.digest, digest);
    assert_eq!(entry.source, "ghcr.io/example/word-count:abi-0.1");
    assert_eq!(entry.version, "1.0.0");
    assert_eq!(entry.abi, "0.1");
    assert_eq!(entry.grant_hash, grant_hash(MANIFEST).expect("hash"));

    // The component is on disk under its digest (load-by-digest has a
    // filename to open, FR-DIST-8).
    let bytes = tree
        .component("word-count", &digest)
        .expect("read back by digest");
    assert_eq!(bytes, component());
    assert!(tree.manifest_path("word-count").exists());
    assert!(tree.lockfile_path().exists());

    let listed = tree.list().expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].0, "word-count");

    assert!(tree.remove("word-count").expect("remove"), "removed");
    assert!(!tree.manifest_path("word-count").exists());
    assert!(tree.list().expect("list").is_empty());
    assert!(!tree.remove("word-count").expect("second remove is a no-op"));
}

// Verifies: ADR-0010's archive is exactly the two files, pack to read
// (the format every platform's zip tools can open, which is the point).
#[test]
fn the_archive_roundtrips_the_two_files() {
    let packed = lca_registry::pack_archive(MANIFEST, &component()).expect("pack");
    let (manifest, component_bytes) = lca_registry::read_archive(&packed).expect("read");
    assert_eq!(manifest, MANIFEST);
    assert_eq!(component_bytes, component());

    let names: Vec<String> = {
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&packed)).expect("open");
        (0..archive.len())
            .map(|i| archive.by_index(i).expect("entry").name().to_string())
            .collect()
    };
    assert_eq!(names.len(), 2, "nothing else added (ADR-0010)");

    let empty = lca_registry::read_archive(b"not a zip").expect_err("refuses junk");
    assert!(empty.to_string().contains("zip"), "{empty}");
    let incomplete = lca_registry::pack_archive(MANIFEST, &component()).expect("pack");
    let _ = incomplete;
}

// Verifies: the consent screen's content - the capability catalog's
// sentences, the required reason shown verbatim with its suffix
// (FR-DIST-5: the local path applies this same flow).
#[test]
fn consent_lines_are_the_catalog_sentences() {
    let lines = lca_registry::consent_lines(MANIFEST).expect("lines");
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(
        lines[0],
        "Runs wc for you. Each command still asks for your approval."
    );

    let provider = r#"name = "p"
version = "1.0.0"
abi = "0.1"
worlds = ["provider"]
description = "x"

[capabilities.net]
hosts = ["api.openai.com", "*.googleapis.com"]

[capabilities.oauth]
redirect_path = "/callback"

[capabilities.credentials]
namespace = "p"
"#;
    let lines = lca_registry::consent_lines(provider).expect("lines");
    assert_eq!(lines.len(), 3, "{lines:?}");
    // Content, not position: the capability table iterates in map order.
    assert!(
        lines
            .iter()
            .any(|l| l.contains("api.openai.com") && l.contains("any subdomain of googleapis.com")),
        "{lines:?}"
    );
    assert!(lines.iter().any(|l| l.contains("local port")), "{lines:?}");
    assert!(
        lines.iter().any(|l| l.contains("saved credentials")),
        "{lines:?}"
    );

    let completer = r#"name = "c"
version = "1.0.0"
abi = "0.1"
worlds = ["compaction"]
description = "x"

[capabilities.completion]
reason = "Summarizes older parts of the conversation when compacting."
"#;
    let lines = lca_registry::consent_lines(completer).expect("lines");
    assert_eq!(
        lines[0],
        "Summarizes older parts of the conversation when compacting. \
         This lets it ask the current model for a response."
    );

    let unknown = lca_registry::consent_lines("name=\"x\"\n[capabilities.mystery]\nreason=\"r\"")
        .expect_err("unknown capability");
    assert!(unknown.to_string().contains("mystery"), "{unknown}");
}

// Verifies: FR-DIST-7's prompt rule - a widened capability set prompts,
// an identical or narrower one does not.
#[test]
fn an_update_prompts_only_when_grants_widen() {
    assert!(
        !lca_registry::update_widens_grants(MANIFEST, MANIFEST).expect("compare"),
        "identical never prompts"
    );
    assert!(
        !lca_registry::update_widens_grants(MANIFEST_WIDENED, MANIFEST).expect("compare"),
        "narrowing never prompts"
    );
    assert!(
        lca_registry::update_widens_grants(MANIFEST, MANIFEST_WIDENED).expect("compare"),
        "a new capability prompts (FR-DIST-7)"
    );
    assert!(
        lca_registry::update_widens_grants(MANIFEST, &MANIFEST.replace("Runs wc", "Runs other"))
            .expect("compare"),
        "a changed declaration prompts: the user approved the old words"
    );
}

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
    let url = format!("http://{addr}/word-count-abi-0.1.zip");
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
        manifest: MANIFEST.to_string(),
        component: component(),
    })
    .expect("install v1");

    // The interrupted update fails without touching the tree.
    let url = format!("http://{addr}/word-count-abi-0.1.zip");
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

/// A local anonymous OCI registry: config blob carries the manifest,
/// layer0 the component, digests honored (our publishing convention).
async fn mock_oci(broken_digests: bool) -> (String, tokio::task::JoinHandle<()>) {
    use sha2::{Digest, Sha256};
    let manifest_toml = MANIFEST.as_bytes().to_vec();
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

// Verifies: FR-DIST-1 (OCI with the built-in client, no external tool -
// FR-DIST-2) and FR-DIST-3: both digests are verified before the
// result is returned, and a lying registry is refused (FR-DIST-4's
// trigger: nothing reaches the store).
#[tokio::test]
async fn oci_resolution_verifies_both_digests() {
    let (reference, server) = mock_oci(false).await;
    let resolved = resolve_oci(&format!("{reference}:abi-0.1"))
        .await
        .expect("resolve");
    assert_eq!(resolved.manifest, MANIFEST);
    assert_eq!(resolved.component, component());
    assert_eq!(
        resolved.digest,
        lca_registry::Resolved::digest_of(&component())
    );
    assert_eq!(resolved.source, format!("{reference}:abi-0.1"));

    // The default tag when the reference carries none (moving-tag
    // installs spell theirs out; bare names resolve to `latest`).
    let resolved = resolve_oci(&reference).await.expect("default tag");
    assert_eq!(resolved.component, component());
    server.abort();

    // A registry whose layer digest does not match the bytes it then
    // serves: refused at resolve time, so FR-DIST-4's delete never has
    // to happen - nothing was written.
    let (reference, server) = mock_oci(true).await;
    let err = resolve_oci(&format!("{reference}:abi-0.1"))
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
#[test]
fn oci_references_split_host_name_and_tag() {
    // Exercised through resolve_local's absence here: the splitter is
    // private, so its behavior shows up as successful resolves above
    // (mock server addresses carry host:port) and clear errors below.
    let err = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(async { resolve_oci("no-slash-no-host").await })
        .expect_err("no host");
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

// Verifies: ADR-0010 - the archive carries extension.toml and the component
// and nothing else; an extra entry is refused rather than silently ignored.
#[test]
fn the_archive_refuses_extra_entries() {
    use std::io::Write;
    fn options() -> zip::write::SimpleFileOptions {
        zip::write::SimpleFileOptions::default()
    }
    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut cursor);
        writer
            .start_file("extension.toml", options())
            .expect("file");
        writer.write_all(MANIFEST.as_bytes()).expect("write");
        writer
            .start_file("component.wasm", options())
            .expect("file");
        writer.write_all(&component()).expect("write");
        writer.start_file("evil.sh", options()).expect("file");
        writer.write_all(b"rm -rf /").expect("write");
        writer.finish().expect("finish");
    }
    let err = lca_registry::read_archive(&cursor.into_inner()).expect_err("extra entry refused");
    assert!(
        err.to_string().contains("unexpected archive entry"),
        "{err}"
    );
}

// The fs consent sentence matches the mode: a read-only grant must not
// claim it can write (the install screen is the user's only warning).
#[test]
fn the_fs_consent_sentence_matches_the_granted_mode() {
    let read_only = r#"name = "r"
version = "1.0.0"
abi = "0.2"
worlds = ["context-transform"]
description = "x"

[capabilities.fs]
workspace = "read"
"#;
    let files_line = |manifest: &str| {
        lca_registry::consent_lines(manifest)
            .expect("lines")
            .into_iter()
            .find(|line| line.starts_with("Files:"))
            .expect("an fs line")
    };
    assert_eq!(
        files_line(read_only),
        "Files: workspace (read). It can read those files."
    );
    let read_write = read_only.replace("workspace = \"read\"", "workspace = \"read-write\"");
    assert_eq!(
        files_line(&read_write),
        "Files: workspace (read and write). It can read and write those files."
    );
}
