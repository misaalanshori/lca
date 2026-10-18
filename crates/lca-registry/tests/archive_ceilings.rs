//! Archive hostile-input ceilings (gh #350): every `read_archive` member is
//! bounded while reading, so a kilobyte archive cannot expand to gigabytes.
//!
//! Split from `registry.rs` for the workspace file ceiling (gate 11).
//!
//! `read_to_end` decompresses everything a member claims before anyone can
//! look at the result, so a size check placed after it is already too late —
//! which was exactly the shape of the defect: components had no check at
//! all, `resources/*` was measured after the allocation, and the manifest had
//! no bound anywhere.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

const MANIFEST: &str = r#"name = "probe"
version = "1.0.0"
abi = "0.6"
worlds = ["tool"]
description = "Probe."
"#;

fn component() -> Vec<u8> {
    b"\0asm\x0d\0\x01\0probe".to_vec()
}

/// Build a zip holding one member named `name` whose *uncompressed* size is
/// `expanded` bytes of a repeated byte, deflated. The repeated byte is what
/// makes this a bomb: `expanded` can be gigabytes while the archive stays
/// kilobytes, and the write side streams so the test itself never holds the
/// expansion either.
fn bomb(name: &str, expanded: u64, extra: Option<(&str, &[u8])>) -> Vec<u8> {
    bomb_with_manifest(name, expanded, extra, MANIFEST.as_bytes())
}

/// [`bomb`] with the manifest bytes chosen by the caller (the manifest
/// ceiling row needs an oversized one).
fn bomb_with_manifest(
    name: &str,
    expanded: u64,
    extra: Option<(&str, &[u8])>,
    manifest: &[u8],
) -> Vec<u8> {
    use std::io::Write;
    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut cursor);
        let deflated = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        if let Some((extra_name, bytes)) = extra {
            writer.start_file(extra_name, deflated).expect("file");
            writer.write_all(bytes).expect("write");
        }
        writer.start_file(name, deflated).expect("file");
        let chunk = [b'\0'; 64 * 1024];
        let mut written = 0u64;
        while written < expanded {
            let take = (expanded - written).min(chunk.len() as u64) as usize;
            writer.write_all(&chunk[..take]).expect("write");
            written += take as u64;
        }
        if !extra.is_some_and(|(extra_name, _)| extra_name == "extension.toml") {
            writer.start_file("extension.toml", deflated).expect("file");
            writer.write_all(manifest).expect("write");
        }
        writer.finish().expect("finish");
    }
    cursor.into_inner()
}

// Verifies: gh #350 acceptance 2 - a component that expands past
// `MAX_COMPONENT_BYTES` is refused by NAME, and the refusal happens while
// reading rather than after the whole entry landed in RAM. The archive
// itself is tiny (a few hundred kilobytes of deflate for 80 MiB of zeros),
// which is exactly the bomb shape.
#[test]
fn an_oversized_component_is_refused_while_reading() {
    let archive = bomb(
        "component.wasm",
        lca_registry::MAX_COMPONENT_BYTES + 1024 * 1024,
        None,
    );
    assert!(
        archive.len() < 4 * 1024 * 1024,
        "the bomb stays small on disk: {} bytes",
        archive.len()
    );
    let err = lca_registry::read_archive(&archive).expect_err("over the component ceiling");
    let text = err.to_string();
    assert!(text.contains("component.wasm"), "names the member: {text}");
    assert!(
        text.contains(&lca_registry::MAX_COMPONENT_BYTES.to_string()),
        "names the limit: {text}"
    );
    assert!(
        text.contains("repackage"),
        "says what to do about it: {text}"
    );
}

// Verifies: gh #350 acceptance 2 - the same for a `resources/` member, which
// used to be decompressed in full and only then measured against
// `RESOURCE_FILE_MAX_BYTES`. The component is a real one so the only
// possible refusal is the resource ceiling.
#[test]
fn an_oversized_resource_is_refused_while_reading() {
    let manifest = MANIFEST.replace(
        "worlds = [\"tool\"]",
        "worlds = [\"tool\"]\nresources = [\"blob.bin\"]",
    );
    let _ = manifest;
    let archive = bomb(
        "resources/blob.bin",
        lca_registry::RESOURCE_FILE_MAX_BYTES + 1,
        Some(("component.wasm", &component())),
    );
    let err = lca_registry::read_archive(&archive).expect_err("over the per-file ceiling");
    let text = err.to_string();
    assert!(
        text.contains("resources/blob.bin"),
        "names the member: {text}"
    );
    assert!(
        text.contains(&lca_registry::RESOURCE_FILE_MAX_BYTES.to_string()),
        "names the limit: {text}"
    );
}

// Verifies: gh #350 - a manifest member is bounded too, the one member that
// had no ceiling anywhere. A 2 MiB `extension.toml` is not a manifest.
#[test]
fn an_oversized_manifest_is_refused_while_reading() {
    let oversized = vec![b'#'; 2 * 1024 * 1024];
    let archive = bomb_with_manifest("resources/keep.bin", 1, None, &oversized);
    let err = lca_registry::read_archive(&archive).expect_err("over the manifest ceiling");
    let text = err.to_string();
    assert!(text.contains("extension.toml"), "names the member: {text}");
    assert!(
        text.contains(&lca_registry::MAX_MANIFEST_BYTES.to_string()),
        "names the limit: {text}"
    );
}

// Verifies: gh #350 acceptance 3 + 4 - an archive under every ceiling still
// installs byte-identically. A member exactly AT its limit is accepted: the
// bound is `take(limit + 1)`, and the `+ 1` must not turn an exact fit into a
// refusal.
#[test]
fn a_member_exactly_at_its_limit_is_accepted() {
    let manifest = MANIFEST.replace(
        "worlds = [\"tool\"]",
        "worlds = [\"tool\"]\nresources = [\"blob.bin\"]",
    );
    let at_limit = vec![b'r'; lca_registry::RESOURCE_FILE_MAX_BYTES as usize];
    let packed = lca_registry::pack_archive_with_resources(
        &manifest,
        &component(),
        &[("blob.bin".to_string(), at_limit.clone())],
    )
    .expect("pack");
    let archive = lca_registry::read_archive(&packed).expect("an exact fit is accepted");
    assert_eq!(
        archive.resources,
        vec![("blob.bin".to_string(), at_limit)],
        "the resource round-trips byte-identically"
    );
    assert_eq!(archive.component, component(), "the component is untouched");
    assert_eq!(archive.manifest, manifest, "the manifest is untouched");
}

// Verifies: gh #350 - the member-count ceiling. A bomb made of many tiny
// members multiplies the per-entry cost and the name vector; the cap refuses
// it before any entry is opened.
#[test]
fn an_archive_over_the_entry_cap_is_refused() {
    use std::io::Write;
    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut cursor);
        let deflated = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        writer.start_file("extension.toml", deflated).expect("file");
        writer.write_all(MANIFEST.as_bytes()).expect("write");
        writer.start_file("component.wasm", deflated).expect("file");
        writer.write_all(&component()).expect("write");
        for index in 0..=lca_registry::MAX_ARCHIVE_ENTRIES {
            writer
                .start_file(format!("resources/{index}.bin"), deflated)
                .expect("file");
            writer.write_all(b"x").expect("write");
        }
        writer.finish().expect("finish");
    }
    let err = lca_registry::read_archive(&cursor.into_inner()).expect_err("over the entry cap");
    let text = err.to_string();
    assert!(
        text.contains(&lca_registry::MAX_ARCHIVE_ENTRIES.to_string()),
        "names the cap: {text}"
    );
    assert!(text.contains("entries"), "says what was too many: {text}");
}
