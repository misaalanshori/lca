//! GitHub #350: `read_archive` decompressed zip members without a read
//! ceiling, so a tiny archive could expand to gigabytes in RAM.
//!
//! Three shapes were unbounded, all in the same loop:
//! - `.wasm` components: `read_to_end` with **no** size check at all;
//! - `resources/*`: `read_to_end` *then* compare against
//!   `RESOURCE_FILE_MAX_BYTES` — the allocation already happened;
//! - `extension.toml`: no ceiling anywhere.
//!
//! `read_to_end` decompresses everything a member claims before anyone can
//! look at the result, so the size check that followed was already too
//! late. Every member now goes through `Read::take(limit + 1)` and is
//! refused when it lands past the limit; the `+ 1` distinguishes an exact
//! fit from an overrun without trusting the declared size. A member-count
//! cap bounds the name vector, and the download itself is capped too.
//!
//! Verifies: gh #350 acceptance 1-4 at the crate boundary — the expansion
//! never lands in memory, the refusals name the member and the limit, and
//! an archive inside every ceiling still round-trips byte-identically.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::io::Write;

const MANIFEST: &str = r#"name = "probe"
version = "1.0.0"
abi = "0.6"
worlds = ["tool"]
description = "Probe."
"#;

/// A minimal valid component payload (the bytes are opaque to `read_archive`).
fn component() -> Vec<u8> {
    b"\0asm\x0d\0\x01\0probe".to_vec()
}

/// Build an archive holding one member named `name` whose uncompressed size
/// is `expanded` bytes of zeros, deflated, plus a component and a manifest.
///
/// The zeros are what make it a bomb: `expanded` can be gigabytes while the
/// archive stays kilobytes. The write side streams in 64 KiB slices, so this
/// helper itself never materializes the expansion either.
fn bomb(name: &str, expanded: u64) -> Vec<u8> {
    bomb_with_component(name, expanded, true)
}

/// [`bomb`] with the small valid component included or omitted (the
/// component-ceiling row names its own `component.wasm`, and an archive may
/// carry only one).
fn bomb_with_component(name: &str, expanded: u64, with_component: bool) -> Vec<u8> {
    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut cursor);
        let deflated = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        if with_component {
            writer.start_file("component.wasm", deflated).expect("file");
            writer.write_all(&component()).expect("write");
        }
        writer.start_file(name, deflated).expect("file");
        let chunk = [0u8; 64 * 1024];
        let mut written = 0u64;
        while written < expanded {
            let take = (expanded - written).min(chunk.len() as u64) as usize;
            writer.write_all(&chunk[..take]).expect("write");
            written += take as u64;
        }
        writer.start_file("extension.toml", deflated).expect("file");
        writer.write_all(MANIFEST.as_bytes()).expect("write");
        writer.finish().expect("finish");
    }
    cursor.into_inner()
}

// Verifies: gh #350 acceptance 2 — a 100 MiB-expanding member in a
// kilobyte-sized archive is refused, not allocated. Before the fix this
// reached `read_archive`'s size check only after 100 MiB had landed.
#[test]
fn a_zip_bomb_is_refused_without_expanding() {
    let expanded = 100 * 1024 * 1024;
    let archive = bomb("resources/bomb.bin", expanded);
    assert!(
        archive.len() < 4 * 1024 * 1024,
        "the bomb stays small on disk ({expanded} bytes expand from {} bytes): \
         deflate caps how far it can go",
        archive.len()
    );
    let err = lca_registry::read_archive(&archive).expect_err("refused, not expanded");
    let text = err.to_string();
    assert!(
        text.contains("resources/bomb.bin"),
        "the refusal names the member: {text}"
    );
    assert!(
        text.contains(&lca_registry::RESOURCE_FILE_MAX_BYTES.to_string()),
        "and the limit it crossed: {text}"
    );
}

// Verifies: gh #350 acceptance 3 — a component over `MAX_COMPONENT_BYTES`
// refuses, and the refusal names both the member and the ceiling. This was
// the member with no check at all.
#[test]
fn an_oversized_component_is_refused() {
    let archive = bomb_with_component(
        "component.wasm",
        lca_registry::MAX_COMPONENT_BYTES + 1024,
        false,
    );
    // The component itself is the oversized member; the manifest is the
    // only other entry.
    let err = lca_registry::read_archive(&archive).expect_err("over the component ceiling");
    let text = err.to_string();
    assert!(text.contains("component.wasm"), "names the member: {text}");
    assert!(
        text.contains(&lca_registry::MAX_COMPONENT_BYTES.to_string()),
        "names the ceiling: {text}"
    );
}

// Verifies: gh #350 acceptance 4 — an archive under every ceiling installs
// byte-identically. The bound is `take(limit + 1)`, so a member exactly at
// its limit must be accepted: an off-by-one here would brick real packages.
#[test]
fn an_archive_inside_the_ceilings_round_trips_byte_identically() {
    let manifest = MANIFEST.replace(
        "worlds = [\"tool\"]",
        "worlds = [\"tool\"]\nresources = [\"blob.bin\"]",
    );
    let exact = vec![b'z'; lca_registry::RESOURCE_FILE_MAX_BYTES as usize];
    let packed = lca_registry::pack_archive_with_resources(
        &manifest,
        &component(),
        &[("blob.bin".to_string(), exact.clone())],
    )
    .expect("pack");
    let archive = lca_registry::read_archive(&packed).expect("an exact fit is accepted");
    assert_eq!(archive.resources, vec![("blob.bin".to_string(), exact)]);
    assert_eq!(archive.component, component());
    assert_eq!(archive.manifest, manifest);
}
