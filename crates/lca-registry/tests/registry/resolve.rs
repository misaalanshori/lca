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
    // The default tag tracks the ABI line, so it is derived: a literal here
    // would redden on every line move while saying nothing about the rule.
    assert_eq!(
        lca_registry::expand_shorthand("antigravity"),
        format!(
            "ghcr.io/misaalanshori/lca/antigravity:abi-{}",
            lca_ext_abi::ABI_VERSION
        )
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

// ---------------------------------------------------------------------------
// gh #615: what a registry status *means*, in the user's words
//
// The line move (gh #407) published no `abi-0.7` artifact yet, so a bare name
// failed with `fetch failed: ...: HTTP401 and the token endpoint answered 403
// Forbidden` - two registry statuses and no next step. The three shapes below
// are the ones a registry actually produces, and each has a different fix, so
// each gets its own sentence.
// ---------------------------------------------------------------------------

/// A missing tag is reported as a **missing tag**, naming the line and what to
/// do: the same sentence for both wire shapes, because the meaning is the same
/// - a `404` (a plain registry) and ghcr's `401`-then-refused-anonymous-token
/// dance for an absent package are the same fact.
#[tokio::test]
async fn a_missing_abi_line_tag_names_the_line_and_the_next_release() {
    let line = lca_ext_abi::ABI_VERSION;
    for statuses in [
        // A registry that answers 404 for an absent tag.
        vec!["404 Not Found"],
        // ghcr's shape for an absent *package*: challenge, then the anonymous
        // token is refused.
        vec!["401 Unauthorized", "403 Forbidden"],
        // ghcr's shape for an absent *tag* (measured 2026-11-24 against the
        // real registry): challenge, token issued, manifest 404.
        vec!["401 Unauthorized", "200 OK", "404 Not Found"],
    ] {
        let (base, server) = super::support::mock_statuses(statuses.clone()).await;
        let err = resolve_oci(&format!("{base}/example/word-count:abi-{line}"))
            .await
            .expect_err("an absent tag cannot resolve");
        let text = err.to_string();
        assert!(
            text.contains(&format!(
                "no artifact for word-count at ABI line {line} yet"
            )),
            "names the extension and the line ({statuses:?}): {text}"
        );
        assert!(
            text.contains("`lca ext install word-count:unstable`"),
            "names the escape hatch ({statuses:?}): {text}"
        );
        assert!(
            !text.contains("HTTP") && !text.contains("token endpoint"),
            "no registry statuses in the user's sentence ({statuses:?}): {text}"
        );
        server.abort();
    }
}

/// The control: a tag that **does** exist still fetches through the very same
/// code path - the classification must not swallow success.
#[tokio::test]
async fn an_existing_tag_still_fetches_and_prompts() {
    let (reference, server) = mock_oci(false).await;
    let resolved = resolve_oci(&format!("{reference}:abi-0.6"))
        .await
        .expect("an existing tag resolves");
    assert_eq!(resolved.manifest, MANIFEST);
    assert_eq!(resolved.component, component());
    assert_eq!(
        resolved.source,
        format!("{reference}:abi-0.6"),
        "the control keeps the ref it was given"
    );
    server.abort();
}

/// A missing name on the unstable channel is its own sentence: that channel is a
/// curated set, so the release line is where the name lives - not "check the tag"
/// (which is what a wrong `:b<sha>` pin needs) and not "this line is new" (which
/// is what a line tag needs).
#[test]
fn a_name_the_unstable_channel_does_not_publish_names_the_release_line() {
    let text = lca_registry::Error::MissingArtifact {
        name: "codex".to_string(),
        tag: "unstable".to_string(),
    }
    .to_string();
    assert!(
        text.contains("no artifact for codex on the unstable channel"),
        "names the channel: {text}"
    );
    assert!(
        text.contains(&format!("codex:abi-{}", lca_ext_abi::ABI_VERSION))
            && text.contains("next release"),
        "names the line that does publish it: {text}"
    );
}

/// A token *was* issued and the pull was still refused: a different sentence,
/// because the fix is different. Reaching this wire shape takes a registry that
/// authenticates and then denies the artifact, which is what a private package
/// looks like.
#[tokio::test]
async fn a_refused_authorized_pull_names_the_refusal_not_the_absence() {
    let (base, server) =
        super::support::mock_statuses(vec!["401 Unauthorized", "200 OK", "401 Unauthorized"]).await;
    let tag = format!("abi-{}", lca_ext_abi::ABI_VERSION); // derived: a literal reddens on a line move
    let err = resolve_oci(&format!("{base}/example/word-count:{tag}"))
        .await
        .expect_err("a refused pull cannot resolve");
    let text = err.to_string();
    assert!(
        text.contains(&format!("refused to pull word-count:{tag}")),
        "names the refusal and the ref: {text}"
    );
    assert!(
        !text.contains("no artifact"),
        "a refusal is not an absence: {text}"
    );
    server.abort();
}

// ---------------------------------------------------------------------------
// gh #615: the channel decides the *default* tag, and nothing else
//
// The two release lines publish different tags for the same extension, so the
// tag a bare name resolves to is a property of the build, not a constant.
// ---------------------------------------------------------------------------

/// An unstable build resolves a bare name to the tag its own channel publishes.
#[test]
fn an_unstable_build_resolves_a_bare_name_to_the_unstable_tag() {
    assert_eq!(
        lca_registry::expand_shorthand_with("antigravity", lca_registry::Channel::Unstable),
        "ghcr.io/misaalanshori/lca/antigravity:unstable"
    );
}

/// A release build keeps the line tag - the tag #615's release publishes.
#[test]
fn a_release_build_resolves_a_bare_name_to_the_abi_line_tag() {
    assert_eq!(
        lca_registry::expand_shorthand_with("antigravity", lca_registry::Channel::Release),
        format!(
            "ghcr.io/misaalanshori/lca/antigravity:abi-{}",
            lca_ext_abi::ABI_VERSION
        )
    );
}

/// An explicit tag is never rewritten, on either channel: someone who asked for
/// `abi-0.6` gets a fetch of `abi-0.6` (and the absence copy if it is gone),
/// never a silent substitution to a tag the channel likes better.
#[test]
fn an_explicit_tag_is_never_rewritten() {
    for channel in [
        lca_registry::Channel::Release,
        lca_registry::Channel::Unstable,
    ] {
        for source in [
            "antigravity:unstable",
            "antigravity:1.0.1",
            "antigravity:b9653a3e",
            "antigravity:abi-0.6",
        ] {
            let expected = lca_registry::expand_shorthand(source);
            assert_eq!(
                lca_registry::expand_shorthand_with(source, channel),
                expected,
                "{source} on {channel:?}"
            );
        }
        for full in [
            "ghcr.io/misaalanshori/lca/antigravity:abi-0.6",
            "ghcr.io/misaalanshori/lca/antigravity:1.0.1",
            "quay.io/team/ext:1.0",
        ] {
            assert_eq!(
                lca_registry::expand_shorthand_with(full, channel),
                full,
                "{full} on {channel:?}"
            );
        }
    }
}

/// The hub's path: a catalog ref is authored in its release form, so the line
/// tag is the catalog's *default* and moves to the channel - but only in our
/// own namespace, and only that one tag. `extensions/catalog.toml` is the
/// authored form this rule reads.
#[test]
fn a_catalog_ref_dated_to_the_line_moves_with_the_channel() {
    let line = lca_ext_abi::ABI_VERSION;
    let authored = format!("ghcr.io/misaalanshori/lca/antigravity:abi-{line}");
    assert_eq!(
        lca_registry::catalog_ref_for_channel(&authored, lca_registry::Channel::Unstable),
        "ghcr.io/misaalanshori/lca/antigravity:unstable"
    );
    assert_eq!(
        lca_registry::catalog_ref_for_channel(&authored, lca_registry::Channel::Release),
        authored,
        "a release build installs the authored ref unchanged"
    );
    // Another line, another org, another registry: as written.
    for untouched in [
        "ghcr.io/misaalanshori/lca/antigravity:abi-0.6",
        "ghcr.io/acme/tools/antigravity:abi-0.7",
        "quay.io/acme/antigravity:abi-0.7",
        "ghcr.io/misaalanshori/lca/antigravity:1.0.1",
    ] {
        assert_eq!(
            lca_registry::catalog_ref_for_channel(untouched, lca_registry::Channel::Unstable),
            untouched,
            "{untouched}"
        );
    }
    // And this build's own channel is a release form (no `LCA_BUILD_VERSION`
    // in a test build), which is what keeps every other row here honest.
    assert_eq!(
        lca_registry::Channel::current(),
        lca_registry::Channel::Release,
        "a test build is a release-form build"
    );
}

/// The channel changes the *ref*, never the *eligibility*: the catalog filter
/// is still the ABI window, so an entry outside it stays hidden on both
/// channels.
#[test]
fn the_unstable_ref_keeps_the_catalog_line_filter() {
    let outside = format!("abi-{}", 999_u32);
    let entry = lca_registry::CatalogEntry {
        name: "outside".to_string(),
        version: "9.0.0".to_string(),
        abi: outside,
        category: "tool".to_string(),
        description: "Not for this host.".to_string(),
        oci_ref: "ghcr.io/misaalanshori/lca/outside:abi-999".to_string(),
        capabilities: Vec::new(),
    };
    // The hub applies the window rule to every source's rows (see
    // `crates/lca-cli/src/tui/hub.rs`); the channel rule must not reach it.
    let visible: Vec<_> =
        lca_registry::merge_catalogs(&[(lca_registry::official_source(), vec![entry.clone()])])
            .into_iter()
            .filter(|row| lca_registry::abi_supported(&row.entry))
            .collect();
    assert!(
        visible.is_empty(),
        "out-of-window entries stay hidden: {visible:?}"
    );
    // The ref rule is independent of the filter: it would name a tag, and the
    // filter is what decides the entry is never offered.
    assert_eq!(
        lca_registry::catalog_ref_for_channel(&entry.oci_ref, lca_registry::Channel::Unstable),
        entry.oci_ref,
        "an entry on another line keeps its own tag"
    );
}
