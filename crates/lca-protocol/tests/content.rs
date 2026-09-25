//! The typed content block (ADR-0029): magic-byte sniffing, base64 for a
//! provider's data URI, and a JSON round trip.

use lca_protocol::{ContentBlock, base64_encode, sniff_image_media_type};

// Verifies: ADR-0029 - the media type is sniffed from the bytes, never taken
// from a user-controlled file name (D8's rule).
#[test]
fn the_media_type_comes_from_magic_bytes() {
    assert_eq!(
        sniff_image_media_type(b"\x89PNG\r\n\x1a\nrest"),
        Some("image/png")
    );
    assert_eq!(
        sniff_image_media_type(b"\xff\xd8\xff\xe0jpeg"),
        Some("image/jpeg")
    );
    assert_eq!(sniff_image_media_type(b"GIF89a..."), Some("image/gif"));
    assert_eq!(
        sniff_image_media_type(b"RIFF\x00\x00\x00\x00WEBPVP8 "),
        Some("image/webp")
    );
    assert_eq!(sniff_image_media_type(b"not an image at all"), None);
    assert_eq!(sniff_image_media_type(b""), None);
}

// Verifies: ADR-0029 - the hand-rolled encoder matches RFC 4648 §4, including
// the padding cases (no `base64` crate in the closed dependency list).
#[test]
fn base64_matches_rfc_4648() {
    assert_eq!(base64_encode(b""), "");
    assert_eq!(base64_encode(b"f"), "Zg==");
    assert_eq!(base64_encode(b"fo"), "Zm8=");
    assert_eq!(base64_encode(b"foo"), "Zm9v");
    assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
    assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
    assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    // The full byte range, so the high bits are exercised.
    assert_eq!(base64_encode(&[0xff, 0xfe, 0xfd]), "//79");
}

// Verifies: ADR-0029 - an image block survives serde unchanged, bytes intact.
#[test]
fn an_image_block_round_trips_through_json() {
    let block = ContentBlock::Image {
        media_type: "image/png".to_string(),
        bytes: vec![0, 1, 2, 250, 255],
    };
    let json = serde_json::to_string(&block).expect("serialize");
    assert!(json.contains("\"type\":\"image\""), "tagged case: {json}");
    let back: ContentBlock = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back, block);
}
