//! End-to-end install tests: the OCI/HTTPS install path against a mock registry.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

use common::*;
use std::net::SocketAddr;

/// blob = extension.toml, layer0 = the component).
async fn mock_registry() -> SocketAddr {
    use sha2::{Digest, Sha256};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let config_digest = format!("sha256:{:x}", Sha256::digest(OPENAI_MANIFEST.as_bytes()));
    let layer_digest = format!("sha256:{:x}", Sha256::digest(OPENAI_COMPONENT));
    let image_manifest = serde_json::json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "config": { "mediaType": "text/plain", "digest": config_digest, "size": OPENAI_MANIFEST.len() },
        "layers": [{ "mediaType": "application/wasm", "digest": layer_digest, "size": OPENAI_COMPONENT.len() }],
    })
    .to_string();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
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
                (OPENAI_MANIFEST.as_bytes().to_vec(), "text/plain")
            } else {
                (OPENAI_COMPONENT.to_vec(), "application/wasm")
            };
            let head = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes()).await;
            let _ = stream.write_all(&body).await;
        }
    });
    addr
}

/// Any HTTPS-style host serving the skills zip (packed here from the
/// committed component fixture).
async fn mock_archive(zip: Vec<u8>) -> SocketAddr {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let mut buf = vec![0u8; 8192];
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
            let head = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/zip\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                zip.len()
            );
            let _ = stream.write_all(head.as_bytes()).await;
            let _ = stream.write_all(&zip).await;
        }
    });
    addr
}

// Verifies: the Phase 5 exit test end to end - OCI install with the
// consent screen shown before anything is written and a decline
// writing nothing (FR-PERM-2), HTTPS-archive install with its own
// consent, both recorded in the lockfile (FR-DIST-6), a turn that runs
// against the INSTALLED provider loaded by its recorded digest with no
// moving tag consulted (FR-DIST-8), the denial journal behind
// `ext info` (FR-EXT-9), an update that finds itself up to date, and a
// remove (FR-DIST-1/2/5/9).
#[test]
fn a_clean_machine_installs_from_oci_and_https_then_runs_a_turn() {
    let runtime = rt();
    let registry_addr = runtime.block_on(mock_registry());
    let zip = lca_registry::pack_archive(SKILLS_MANIFEST, SKILLS_COMPONENT).expect("pack zip");
    let archive_addr = runtime.block_on(mock_archive(zip));
    let model = runtime.block_on(start_mock(vec![Reply::Sse(sse_text(
        "installed and chatting",
    ))]));

    let sandbox = sandbox("clean-machine");
    // The grant store exists but consents to nothing yet: the first
    // turn must be denied by the capability engine (no ad hoc net),
    // which is what writes the journal `ext info` counts.
    sandbox.write_grants(false);

    // --- OCI install, consent shown and approved (FR-DIST-1).
    // `registry_addr` already renders host:port.
    let reference = format!("{registry_addr}/library/openai-compatible:abi-0.5");
    let output = sandbox.run_with_stdin(Some(&model), &["ext", "install", &reference], "y\n");
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let text = stdout(&output);
    // The consent names the manifest's own declared host, whatever it
    // is (gh #157): read it off the extension's manifest, never a
    // host literal.
    let manifest = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../extensions/openai-compatible/extension.toml"),
    )
    .expect("the openai-compatible manifest reads");
    let host = manifest
        .lines()
        .find_map(|line| line.trim().strip_prefix("hosts = "))
        .and_then(|list| list.split('"').nth(1))
        .expect("the manifest declares a net host");
    assert!(
        text.contains(&format!("Connect to {host}")),
        "the net consent sentence, verbatim: {text}"
    );
    assert!(
        text.contains("Store and read its own saved credentials"),
        "the credentials consent sentence: {text}"
    );
    assert!(text.contains("Allow these capabilities?"), "{text}");
    assert!(text.contains("installed openai-compatible"), "{text}");

    // The lockfile records digest + source (FR-DIST-6); the component
    // sits beside its manifest, named by that digest.
    let lock = sandbox.extensions_root().join("lockfile.json");
    let root = sandbox.extensions_root();
    let lock_text = std::fs::read_to_string(&lock).unwrap_or_else(|err| {
        panic!(
            "lockfile at {}: {err} (root exists: {}, entries: {:?})",
            lock.display(),
            root.exists(),
            std::fs::read_dir(&root)
                .map(|entries| entries
                    .filter_map(|e| e.ok())
                    .map(|e| e.file_name())
                    .collect::<Vec<_>>())
                .unwrap_or_default()
        )
    });
    assert!(lock_text.contains(&reference), "{lock_text}");
    assert!(lock_text.contains("sha256:"), "{lock_text}");
    assert!(
        sandbox
            .extensions_root()
            .join("openai-compatible")
            .join("extension.toml")
            .exists()
    );
    let component_dir: Vec<_> =
        std::fs::read_dir(sandbox.extensions_root().join("openai-compatible"))
            .expect("dir")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".wasm"))
            .collect();
    assert_eq!(component_dir.len(), 1, "one component, digest-named");

    // --- HTTPS archive install with its own consent (FR-DIST-9).
    let url = format!("http://{archive_addr}/skills-abi-0.5.zip");
    let output = sandbox.run_with_stdin(Some(&model), &["ext", "install", &url], "yes\n");
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let text = stdout(&output);
    assert!(
        text.contains("Files: workspace (read)"),
        "the fs consent sentence: {text}"
    );
    assert!(text.contains("installed skills"), "{text}");

    // --- ext list shows both sources (the exit test's "sees ... each").
    let output = sandbox.run(Some(&model), &["ext", "list"]);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("openai-compatible"), "{text}");
    assert!(text.contains("skills"), "{text}");
    assert!(text.contains(&reference), "{text}");
    assert!(text.contains(&url), "{text}");

    // --- ext info: digest, consent, denial count (FR-EXT-9). No
    // denial yet: nothing has tried anything.
    let output = sandbox.run(Some(&model), &["ext", "info", "openai-compatible"]);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("digest:   sha256:"), "{text}");
    assert!(text.contains("denials:  0"), "{text}");

    // R5: `ext info` accepts the source ref and a digest prefix, and a miss
    // names what does exist instead of refusing flatly.
    let full_digest = text
        .lines()
        .find_map(|line| line.strip_prefix("digest:   "))
        .expect("the digest line")
        .trim()
        .to_string();
    assert!(full_digest.starts_with("sha256:"), "{full_digest}");
    let prefix = &full_digest[7..19];
    let by_source = sandbox.run(Some(&model), &["ext", "info", &reference]);
    assert_eq!(
        by_source.status.code(),
        Some(0),
        "info by source ref: {}",
        stderr(&by_source)
    );
    assert!(
        stdout(&by_source).contains("digest:"),
        "{}",
        stdout(&by_source)
    );
    let by_digest = sandbox.run(Some(&model), &["ext", "info", prefix]);
    assert_eq!(
        by_digest.status.code(),
        Some(0),
        "info by digest prefix: {}",
        stderr(&by_digest)
    );
    let miss = sandbox.run(Some(&model), &["ext", "info", "no-such-ext"]);
    assert_eq!(miss.status.code(), Some(2), "a miss is a usage error");
    let miss_err = stderr(&miss);
    assert!(
        miss_err.contains("Known:") && miss_err.contains("openai-compatible"),
        "the miss names what exists: {miss_err}"
    );

    // The installed WASM provider cannot read the host environment
    // (sandboxing is the point), so its endpoint and key live in its own
    // credential namespace - exactly what `/login` writes for a real
    // install. Without this the WASM provider would default to the
    // manifest's default host and the turn would fail with a connect error.
    sandbox.write_credentials(
        "openai-compatible",
        serde_json::json!({ "api_key": "test-key", "base_url": model.url() }),
    );

    // --- Turn one, WITHOUT ad hoc consent: the installed provider
    // reaches for127.0.0.1, the engine refuses and journals it. The
    // host env override is cleared for this run (gh #177): the
    // guest reads its endpoint from the namespace, and a set host
    // variable would trip the headless env-consent gate (exit 4)
    // before the engine ever refuses (exit 3 + journal, below).
    let output = sandbox.run_env(Some(&model), &["-p", "hi"], &[("OPENAI_BASE_URL", "")]);
    assert_eq!(output.status.code(), Some(3), "stderr: {}", stderr(&output));
    let text = stderr(&output);
    assert!(
        text.contains("matches no granted") || text.contains("permission denied"),
        "{text}"
    );

    let output = sandbox.run(Some(&model), &["ext", "info", "openai-compatible"]);
    let text = stdout(&output);
    assert!(text.contains("denials:  1"), "the journal counted: {text}");

    // --- Grant the loopback consent (FR-PERM-16's modal, standing in
    // offline) and run the turn: the INSTALLED provider, loaded from
    // its digest record, talks to the mocked model (FR-DIST-8).
    sandbox.write_grants(true);
    let output = sandbox.run(Some(&model), &["-p", "hi", "--json"]);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let lines = json_lines(&output);
    let text_line = lines
        .iter()
        .find(|l| l["type"] == "text")
        .expect("a text envelope");
    assert_eq!(text_line["content"], "installed and chatting");
    assert_eq!(model.request_count(), 1, "exactly one model call");

    // --- ext update against the same source: already current.
    let output = sandbox.run(Some(&model), &["ext", "update", "openai-compatible"]);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("up to date"), "{text}");

    // --- ext remove forgets the tree and the record.
    let output = sandbox.run(Some(&model), &["ext", "remove", "skills"]);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert!(!sandbox.extensions_root().join("skills").exists());
    let output = sandbox.run(Some(&model), &["ext", "list"]);
    let text = stdout(&output);
    assert!(!text.contains("\nskills "), "gone from the list: {text}");

    // Consent refused writes nothing at all: EOF at the prompt declines.
    let archive2 = lca_registry::pack_archive(SKILLS_MANIFEST, SKILLS_COMPONENT).expect("pack");
    let runtime2 = rt();
    let archive_addr2 = runtime2.block_on(mock_archive(archive2));
    let url2 = format!("http://{archive_addr2}/skills-abi-0.5.zip");
    let output = sandbox.run_with_stdin(Some(&model), &["ext", "install", &url2], "");
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert!(stdout(&output).contains("aborted"), "{}", stdout(&output));
    assert!(
        !sandbox.extensions_root().join("skills").exists(),
        "nothing written"
    );
}

// ---------------------------------------------------------------------------
// Real-terminal tests (docs/testing-plan.md section 14): the TUI in a tmux
// pane, asserting what is on screen. Unix only; a machine without tmux
// skips rather than fails.
// ---------------------------------------------------------------------------
