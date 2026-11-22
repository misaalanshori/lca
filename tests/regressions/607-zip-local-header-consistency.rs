//! Gh #607 / RUSTSEC-2026-0548: a zip names every entry **twice**.
//!
//! The local file header in front of an entry's data carries a name, and the central directory
//! at the end of the archive carries one too. `ZipArchive` lists and opens entries by the
//! central-directory name, while the `ZipStreamReader` family reads local headers in order and
//! uses *those* names. An archive can therefore show one set of names to code that inspects it
//! and write files under another to code that extracts it - which is exactly a supply-chain
//! attack against a registry that validates names before it acts on them.
//!
//! `zip` 9 added an **opt-in** check for this (`ZipArchive::validate_header_consistency`,
//! `ZipReadOptions::enforce_header_consistency`, `ZipStreamReader::enforce_header_consistency`)
//! and leaves it off by default, so upgrading alone fixes nothing. `lca-registry::read_archive`
//! reads packages the user did not author, so it turns the check on; this file is the guard that
//! it stayed on.
//!
//! The archive below is a **real** one, packed by `pack_archive` and then patched by one byte:
//! the local header's name for `component.wasm` becomes `component.wasn`. Nothing else changes -
//! no CRC, size, method or timestamp - so the archive is still perfectly readable by every
//! reader that trusts local headers. Only the consistency check can tell.

use lca_registry::{pack_archive, read_archive};

const MANIFEST: &str = "name = \"header-consistency\"\n";

/// The four-byte signature that opens a local file header.
const LOCAL_SIGNATURE: &[u8] = b"PK\x03\x04";

/// The four-byte signature that opens a central-directory entry. Everything before the first
/// one of these belongs to the local headers, which is how the patch below finds the right copy
/// of the name.
const CENTRAL_SIGNATURE: &[u8] = b"PK\x01\x02";

fn component() -> Vec<u8> {
    // A real wasm module header is enough: `read_archive` never validates bytecode (the digest
    // does that downstream), and a tiny component keeps this row's archive honest.
    b"\0asm\x01\0\0\0".to_vec()
}

/// Return `archive` with the **local** header's name for `name` changed to `patched`, in place.
///
/// The name must be the same length as the original: the patch changes one byte inside a local
/// header and rewrites no offset, so the archive stays structurally valid and every size, CRC
/// and extra field is untouched. That is what makes the refusal below a claim about *names*
/// rather than about a corrupted file.
fn patch_local_name(archive: Vec<u8>, name: &str, patched: &str) -> Vec<u8> {
    assert_eq!(name.len(), patched.len(), "the patch keeps every offset");
    let mut bytes = archive;
    let central_at = bytes
        .windows(CENTRAL_SIGNATURE.len())
        .position(|window| window == CENTRAL_SIGNATURE)
        .expect("a packed archive ends with a central directory");
    let local_at = bytes[..central_at]
        .windows(LOCAL_SIGNATURE.len())
        .rposition(|window| window == LOCAL_SIGNATURE)
        .expect("the last local header precedes the central directory")
        + LOCAL_SIGNATURE.len();
    // Local file header, counted from just past its signature: version(2) flags(2) method(2)
    // time(2) date(2) crc(4) compressed(4) uncompressed(4) name-length(2) extra-length(2),
    // then the name itself - so the length sits at +22 and the name at +26.
    let name_len = u16::from_le_bytes([bytes[local_at + 22], bytes[local_at + 23]]) as usize;
    let name_at = local_at + 26;
    let found = std::str::from_utf8(&bytes[name_at..name_at + name_len]).expect("utf-8 name");
    assert_eq!(found, name, "the last local header carries this name");
    bytes[name_at..name_at + name_len].copy_from_slice(patched.as_bytes());
    bytes
}

// Verifies: gh #607 / RUSTSEC-2026-0548 - a package whose local header names disagree with its
// central directory is **refused**, and the refusal names the reason instead of surfacing a raw
// zip error. This is the row that reddens if `read_archive` stops calling the check.
#[test]
fn a_zip_archive_whose_local_header_names_disagree_with_the_central_directory_is_refused() {
    let packed = pack_archive(MANIFEST, &component()).expect("pack");
    // The control: the pristine archive is accepted, so the refusal below is about the patch.
    read_archive(&packed).expect("the unpatched archive reads");

    let patched = patch_local_name(packed, "component.wasm", "component.wasn");

    // The patch really is invisible to a reader that trusts local headers: `ZipArchive` lists
    // this archive and opens the entry by its central-directory name without complaint. That is
    // the attack - and the reason "it opens fine" proves nothing.
    {
        let mut lenient = zip::ZipArchive::new(std::io::Cursor::new(&patched)).expect("opens");
        let mut body = Vec::new();
        std::io::Read::read_to_end(
            &mut lenient.by_name("component.wasm").expect("by central name"),
            &mut body,
        )
        .expect("reads");
        assert_eq!(body, component(), "the entry's data is intact");
    }

    let refused = read_archive(&patched).expect_err("the disagreeing archive is refused");
    let message = refused.to_string();
    assert!(
        message.contains("not a trustworthy zip archive"),
        "the refusal says what the archive is, not which subsystem's error it was: {message}"
    );
    assert!(
        message.contains("filename mismatch") || message.contains("header inconsistency"),
        "and it names the disagreement: {message}"
    );
    assert!(
        message.contains("repackage"),
        "and tells the user what to do: {message}"
    );
}

// Verifies: gh #607 - the check is a **name** check and not an accident of some other field: the
// same one-byte patch on a *central-directory* name only (local names left alone) also disagrees,
// because the two copies are compared to each other rather than to a constant.
#[test]
fn a_zip_archive_whose_central_directory_name_disagrees_is_refused_too() {
    let packed = pack_archive(MANIFEST, &component()).expect("pack");
    let central_at = packed
        .windows(CENTRAL_SIGNATURE.len())
        .position(|window| window == CENTRAL_SIGNATURE)
        .expect("central directory");
    // Central directory entry: signature(4) version-made(2) version-needed(2) flags(2)
    // method(2) time(2) date(2) crc(4) compressed(4) uncompressed(4) name-length(2)
    // extra-length(2) comment-length(2) disk(2) internal(2) external(4) offset(4), then the name.
    let name_len = u16::from_le_bytes([packed[central_at + 28], packed[central_at + 29]]) as usize;
    let name_at = central_at + 46;
    let found = std::str::from_utf8(&packed[name_at..name_at + name_len]).expect("utf-8 name");
    assert_eq!(found, "extension.toml", "the first central entry");
    let mut patched = packed;
    patched[name_at..name_at + name_len].copy_from_slice(b"extension.toml"[..].as_ref());
    patched[name_at] = b'E';

    let refused = read_archive(&patched).expect_err("the disagreeing archive is refused");
    let message = refused.to_string();
    assert!(
        message.contains("not a trustworthy zip archive"),
        "the same refusal path: {message}"
    );
}
