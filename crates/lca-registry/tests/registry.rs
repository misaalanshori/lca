//! The distribution rules with receipts: canonical grant hashes, the
//! lockfile tree, the ADR-0010 archive, consent text, and both network
//! resolvers against local mock servers (offline, deterministic -
//! testing plan sections4-5).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use lca_registry::{grant_hash, resolve, resolve_archive, resolve_local, resolve_oci};

const MANIFEST: &str = r#"name = "word-count"
version = "1.0.0"
abi = "0.4"
worlds = ["tool"]
description = "Counts words."

[capabilities.process]
reason = "Runs wc for you."
"#;

const MANIFEST_REORDERED: &str = r#"abi = "0.4"
description = "Counts words."
name = "word-count"
version = "1.0.0"
worlds = ["tool"]

[capabilities.process]
reason = "Runs wc for you."
"#;

const MANIFEST_WIDENED: &str = r#"name = "word-count"
version = "1.1.0"
abi = "0.4"
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
        commit: String::new(),
        manifest: MANIFEST.replace("word-count", "../../escape"),
        component,
        resources: Vec::new(),
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
        source: "ghcr.io/example/word-count:abi-0.4".to_string(),
        commit: String::new(),
        resources: Vec::new(),
    };
    let entry = tree.install(resolved).expect("install");
    assert_eq!(entry.digest, digest);
    assert_eq!(entry.source, "ghcr.io/example/word-count:abi-0.4");
    assert_eq!(entry.version, "1.0.0");
    assert_eq!(entry.abi, "0.4");
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
    let archive = lca_registry::read_archive(&packed).expect("read");
    assert_eq!(archive.manifest, MANIFEST);
    assert_eq!(archive.component, component());

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
abi = "0.4"
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
abi = "0.4"
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

/// A local anonymous OCI registry: config blob carries the manifest,
/// layer0 the component, digests honored (our publishing convention).
async fn mock_oci(broken_digests: bool) -> (String, tokio::task::JoinHandle<()>) {
    mock_oci_serving(MANIFEST, broken_digests).await
}

async fn mock_oci_serving(
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
abi = "0.4"
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

const MANIFEST_WITH_RESOURCES: &str = r#"name = "word-count"
version = "1.0.0"
abi = "0.4"
worlds = ["tool"]
description = "Counts words."
resources = ["presets", "skills"]

[capabilities.process]
reason = "Runs wc for you."
"#;

// Verifies: FR-DIST-1 / ADR-0030 (the archive carries `resources/**`).
#[test]
fn the_archive_carries_resources_and_roundtrips_them() {
    let resources = vec![
        ("presets/p.toml".to_string(), b"id = \"x\"".to_vec()),
        ("skills/s.md".to_string(), b"hi".to_vec()),
    ];
    let packed = lca_registry::pack_archive_with_resources(
        MANIFEST_WITH_RESOURCES,
        &component(),
        &resources,
    )
    .expect("pack");
    let archive = lca_registry::read_archive(&packed).expect("read");
    assert_eq!(archive.resources, resources);
}

// Verifies: ADR-0030 (an undeclared kind is refused, and the install
// writes the declared bag where the host serves it).
#[test]
fn install_writes_declared_resources_and_refuses_an_undeclared_kind() {
    let install_tree = tree("resources-install");
    let resources = vec![("presets/p.toml".to_string(), b"id = \"x\"".to_vec())];
    install_tree
        .install(lca_registry::Resolved {
            digest: lca_registry::Resolved::digest_of(&component()),
            source: "https://example.invalid/word-count.zip".to_string(),
            commit: String::new(),
            manifest: MANIFEST_WITH_RESOURCES.to_string(),
            component: component(),
            resources: resources.clone(),
        })
        .expect("install");
    assert_eq!(
        std::fs::read(
            install_tree
                .root()
                .join("word-count/resources/presets/p.toml")
        )
        .expect("read"),
        b"id = \"x\""
    );

    let tree2 = tree("resources-undeclared");
    let err = tree2
        .install(lca_registry::Resolved {
            digest: lca_registry::Resolved::digest_of(&component()),
            source: "https://example.invalid/word-count.zip".to_string(),
            commit: String::new(),
            manifest: MANIFEST_WITH_RESOURCES.to_string(),
            component: component(),
            resources: vec![("wads/doom.wad".to_string(), b"x".to_vec())],
        })
        .expect_err("undeclared kind refused");
    assert!(err.to_string().contains("does not declare"), "{err}");
    assert!(
        !tree2.root().join("word-count").exists(),
        "a refused install writes nothing"
    );
}

// Verifies: ADR-0030 (a resource path cannot escape the bag, at pack
// time or read time).
#[test]
fn the_archive_refuses_a_traversal_resource_path() {
    let packed = lca_registry::pack_archive_with_resources(
        MANIFEST_WITH_RESOURCES,
        &component(),
        &[("../escape".to_string(), b"x".to_vec())],
    );
    assert!(packed.is_err(), "the packer refuses a traversal path");

    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut cursor);
        let options = zip::write::SimpleFileOptions::default();
        writer.start_file("extension.toml", options).unwrap();
        std::io::Write::write_all(&mut writer, MANIFEST_WITH_RESOURCES.as_bytes()).unwrap();
        writer.start_file("component.wasm", options).unwrap();
        std::io::Write::write_all(&mut writer, &component()).unwrap();
        writer.start_file("resources/../escape", options).unwrap();
        std::io::Write::write_all(&mut writer, b"x").unwrap();
        writer.finish().unwrap();
    }
    let err = lca_registry::read_archive(&cursor.into_inner()).expect_err("traversal refused");
    assert!(err.to_string().contains("leaves the package"), "{err}");
}

// Verifies: ADR-0030 (a data-only package: `worlds = []`, resources only).
#[test]
fn a_data_only_package_installs_and_removes() {
    const DATA_MANIFEST: &str = r#"name = "skill-pack"
version = "1.0.0"
abi = "0.4"
worlds = []
description = "A skill pack."
resources = ["skills"]
"#;
    let resources = vec![(
        "skills/commits/SKILL.md".to_string(),
        b"name: commits\n---\nbody".to_vec(),
    )];
    let packed =
        lca_registry::pack_archive_with_resources(DATA_MANIFEST, &[], &resources).expect("pack");
    let archive = lca_registry::read_archive(&packed).expect("read");
    assert!(
        archive.component.is_empty(),
        "no component in a data-only pack"
    );

    let tree = tree("data-only");
    tree.install(lca_registry::Resolved {
        digest: lca_registry::Resolved::digest_of(&[]),
        source: "https://example.invalid/skill-pack.zip".to_string(),
        commit: String::new(),
        manifest: DATA_MANIFEST.to_string(),
        component: Vec::new(),
        resources: resources.clone(),
    })
    .expect("install");
    assert!(
        tree.root()
            .join("skill-pack/resources/skills/commits/SKILL.md")
            .is_file()
    );
    assert!(tree.remove("skill-pack").expect("remove"));
    assert!(!tree.root().join("skill-pack").exists());
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

// Verifies: #227 - catalog parsing: the format gate refuses, entries
// carry every field the hub lists, and capabilities ride along.
#[test]
fn catalog_parsing_keeps_every_hub_field() {
    let text = r#"
[version]
format = 1

[[extension]]
name = "probe"
version = "1.2.0"
abi = "0.6"
category = "provider"
description = "A probe."
oci_ref = "ghcr.io/example/probe:abi-0.6"
capabilities = ["net", "credentials"]
"#;
    let entries = lca_registry::parse_catalog(text).expect("parses");
    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    assert_eq!(entry.name, "probe");
    assert_eq!(entry.version, "1.2.0");
    assert_eq!(entry.abi, "0.6");
    assert_eq!(entry.category, "provider");
    assert_eq!(entry.oci_ref, "ghcr.io/example/probe:abi-0.6");
    assert_eq!(entry.capabilities, vec!["net", "credentials"]);
    assert!(lca_registry::parse_catalog("no toml [[[[").is_err());
    assert!(lca_registry::parse_catalog("[version]\nformat = 99\n").is_err());
    assert!(
        lca_registry::parse_catalog("[version]\nformat = 1\n[[extension]]\nname = \"x\"\n")
            .is_err()
    );
}

// Verifies: #227 - provenance merging: official rows lead, each custom
// catalog follows under its own name, and a name present twice lists
// twice so the hub can show both origins.
#[test]
fn catalog_merging_marks_every_provenance() {
    let official = lca_registry::CatalogSource {
        id: "official".to_string(),
        name: "Official (misaalanshori/lca)".to_string(),
        url: None,
    };
    let custom = lca_registry::CatalogSource {
        id: "acme".to_string(),
        name: "Acme Corp".to_string(),
        url: Some("https://example.invalid/catalog.toml".to_string()),
    };
    let entry = |name: &str| lca_registry::CatalogEntry {
        name: name.to_string(),
        version: "1.0.0".to_string(),
        abi: "0.6".to_string(),
        category: "provider".to_string(),
        description: String::new(),
        oci_ref: String::new(),
        capabilities: Vec::new(),
    };
    let merged = lca_registry::merge_catalogs(&[
        (official, vec![entry("a"), entry("shared")]),
        (custom, vec![entry("shared"), entry("b")]),
    ]);
    let names: Vec<(&str, &str, bool, Option<&str>)> = merged
        .iter()
        .map(|row| {
            (
                row.entry.name.as_str(),
                row.source_id.as_str(),
                row.official,
                row.source_url.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        names,
        vec![
            ("a", "official", true, None),
            ("shared", "official", true, None),
            (
                "shared",
                "acme",
                false,
                Some("https://example.invalid/catalog.toml")
            ),
            (
                "b",
                "acme",
                false,
                Some("https://example.invalid/catalog.toml")
            ),
        ]
    );
}

// Verifies: #227 - ABI filtering follows the host window: current and
// previous minors load, older lines and garbage do not.
#[test]
fn catalog_abi_filtering_follows_the_host_window() {
    let entry = |abi: &str| lca_registry::CatalogEntry {
        name: "x".to_string(),
        version: "1.0.0".to_string(),
        abi: abi.to_string(),
        category: "provider".to_string(),
        description: String::new(),
        oci_ref: String::new(),
        capabilities: Vec::new(),
    };
    for abi in ["0.6", "0.5", "1.0"] {
        assert!(
            lca_registry::abi_supported(&entry(abi)),
            "{abi} loads on this host"
        );
    }
    for abi in ["0.4", "0.7", "1.1", "nope", ""] {
        assert!(
            !lca_registry::abi_supported(&entry(abi)),
            "{abi} is filtered out"
        );
    }
}

// Verifies: #227 - the shipped first-party catalog parses with every
// row loadable: versions, refs, and capabilities travel together, so
// catalog drift fails here instead of in the hub.
#[test]
fn shipped_catalog_parses_with_every_row_loadable() {
    let text = include_str!("../../../extensions/catalog.toml");
    let entries = lca_registry::parse_catalog(text).expect("shipped catalog parses");
    assert!(entries.len() >= 13, "all shippable extensions listed");
    for entry in &entries {
        assert!(
            lca_registry::abi_supported(entry),
            "{} names an installable line: {}",
            entry.name,
            entry.abi
        );
        assert!(
            entry.oci_ref.starts_with("ghcr.io/misaalanshori/lca/"),
            "{} names the first-party registry: {}",
            entry.name,
            entry.oci_ref
        );
        assert!(
            !entry.description.is_empty(),
            "{} carries a line",
            entry.name
        );
    }
    let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
    for expected in [
        "openai-compatible",
        "anthropic",
        "antigravity",
        "codex",
        "github-copilot",
        "grok",
        "kimi-coding",
        "llama",
        "meta",
        "openrouter",
        "compaction-default",
        "mcp",
        "skills",
    ] {
        assert!(names.contains(&expected), "{expected} is catalogued");
    }
}

// Verifies: #227 - the TTL cache: a fresh copy reads without network,
// and freshness is an age comparison the hub can trust.
#[test]
fn catalog_cache_distinguishes_fresh_from_stale() {
    let dir = lca_testkit::scratch_path("lca-catalog-cache");
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("official.toml");
    assert!(lca_registry::read_cache(&path).is_none(), "absent is none");
    let text = "[version]\nformat = 1\n";
    lca_registry::write_cache(&path, text).expect("write");
    let (entries, fresh) = lca_registry::read_cache(&path).expect("reads");
    assert!(entries.is_empty());
    assert!(fresh, "a just-written copy is fresh");
    assert!(
        lca_registry::cache_fresh(std::time::Duration::from_secs(60)),
        "a minute is fresh"
    );
    assert!(
        !lca_registry::cache_fresh(lca_registry::CATALOG_TTL + std::time::Duration::from_secs(1)),
        "past the TTL is stale"
    );
}

// Gh #59 helpers: a local git repo with two commits (v1 then v2 of
// one component package), so git-source tests run offline. `None`
// when git is missing: the resolver names the fix instead.
fn git_repo(name: &str) -> Option<std::path::PathBuf> {
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

fn head_sha(dir: &std::path::Path) -> String {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(dir)
        .output()
        .expect("git runs");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

// Verifies: gh #59 - `git+<path>` resolves the checkout like any
// other source: digest-verified bytes, consent-ready manifest, and
// the pinned commit recorded.
#[tokio::test]
async fn git_source_resolves_a_pinned_commit() {
    let Some(repo) = git_repo("git-pin") else {
        eprintln!("skip: git is not installed");
        return;
    };
    let sha = head_sha(&repo);
    let source = format!("git+{}", repo.display());
    let resolved = lca_registry::resolve(&source, None)
        .await
        .expect("git resolves");
    assert_eq!(
        resolved.digest,
        lca_registry::Resolved::digest_of(b"v2-bytes-longer")
    );
    assert_eq!(resolved.commit, sha, "the HEAD commit pins the install");
    assert!(resolved.source.starts_with("git+"), "{}", resolved.source);
    assert!(resolved.manifest.contains("word-count"));
}

// Verifies: gh #59 - an explicit `#rev` pins that commit, not the
// floating HEAD: the older digest comes back.
#[tokio::test]
async fn git_source_pins_an_explicit_rev() {
    let Some(repo) = git_repo("git-rev") else {
        eprintln!("skip: git is not installed");
        return;
    };
    let out = std::process::Command::new("git")
        .args(["rev-list", "--max-parents=0", "HEAD"])
        .current_dir(&repo)
        .output()
        .expect("git runs");
    let first = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let resolved = lca_registry::resolve(&format!("git+{}#{first}", repo.display()), None)
        .await
        .expect("resolves");
    assert_eq!(
        resolved.digest,
        lca_registry::Resolved::digest_of(b"v1-bytes")
    );
    assert_eq!(resolved.commit, first);
}
