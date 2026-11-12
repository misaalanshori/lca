//! The archive format (FR-DIST-9, gh #350): the two files round-trip, an
//! undeclared or oversized member is refused, and a resource path cannot
//! traverse.
//!
//! Split out of `tests/registry.rs` beside the `archive` module it exercises.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use super::support::{MANIFEST, MANIFEST_WITH_RESOURCES, component};

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
