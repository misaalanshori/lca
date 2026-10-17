//! GitHub #358: `webp_dimensions` proved 16 bytes of RIFF/WEBP magic and
//! then direct-indexed up to byte 29 (`VP8X`/`VP8 `) or 24 (`VP8L`). Any
//! 16..=29-byte file carrying the magic — a truncated download, a corrupt
//! attachment, a text file renamed `.webp` that still sniffs by magic —
//! panicked with an index-out-of-bounds in the transcript render path
//! (`image_dimensions` runs on every render and resize, and a panic there
//! is fatal in the release profile).
//!
//! Verifies: the whole 0..=30 truncation matrix per chunk kind reaches
//! `None` (the placeholder path) instead of panicking — a panic IS the
//! failure this guards — plus empty input and the three positive rows
//! proving the safe reads kept the arithmetic intact.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use lca_tui::widgets::image::image_dimensions;

/// A WebP buffer of `len` zeros carrying the magic and the chunk fourcc.
fn webp(kind: &[u8; 4], len: usize) -> Vec<u8> {
    let mut bytes = vec![0u8; len];
    if len >= 4 {
        bytes[..4].copy_from_slice(b"RIFF");
    }
    if len >= 12 {
        bytes[8..12].copy_from_slice(b"WEBP");
    }
    if len >= 16 {
        bytes[12..16].copy_from_slice(kind);
    }
    bytes
}

// Verifies: gh #358 AC1 - every length 0..=30 per chunk kind is safe.
// `image_dimensions` returning is the assertion; the bug panicked here.
#[test]
fn every_truncated_webp_length_reports_no_dimensions() {
    for kind in [b"VP8X", b"VP8 ", b"VP8L"] {
        // Short of the arm's requirement the header cannot be read.
        let needs = if kind == b"VP8L" { 25 } else { 30 };
        for len in 0..=30usize {
            let bytes = webp(kind, len);
            let got = image_dimensions("image/webp", &bytes);
            if len < needs {
                assert_eq!(
                    got,
                    None,
                    "{} truncated to {len} bytes is unreadable",
                    String::from_utf8_lossy(kind)
                );
            }
        }
    }
    assert_eq!(image_dimensions("image/webp", &[]), None, "empty input");
    assert_eq!(
        image_dimensions("image/webp", b"RIFF\x00\x00\x00\x00WEB"),
        None,
        "a 15-byte prefix never reaches the chunk arms"
    );
}

// Verifies: gh #358 AC2 - the three chunk kinds still decode real
// dimensions, so the checked slices did not change the arithmetic.
#[test]
fn each_webp_chunk_kind_still_decodes_its_dimensions() {
    // VP8X: 24-bit LE canvas size minus one at 24..30.
    let mut vp8x = webp(b"VP8X", 30);
    vp8x[24..27].copy_from_slice(&[0x2f, 0x02, 0x00]); // 560 - 1
    vp8x[27..30].copy_from_slice(&[0x8f, 0x01, 0x00]); // 400 - 1
    assert_eq!(image_dimensions("image/webp", &vp8x), Some((560, 400)));

    // VP8 (lossy): 14-bit width/height at 26..30.
    let mut vp8 = webp(b"VP8 ", 30);
    vp8[26..28].copy_from_slice(&320u16.to_le_bytes());
    vp8[28..30].copy_from_slice(&240u16.to_le_bytes());
    assert_eq!(image_dimensions("image/webp", &vp8), Some((320, 240)));

    // VP8L (lossless): 14-bit (w-1) then 14-bit (h-1) at 21..25.
    let mut vp8l = webp(b"VP8L", 25);
    let bits: u32 = (639 & 0x3fff) | ((479 & 0x3fff) << 14);
    vp8l[21..25].copy_from_slice(&bits.to_le_bytes());
    assert_eq!(image_dimensions("image/webp", &vp8l), Some((640, 480)));
}
