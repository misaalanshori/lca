//! Image placeholders for the transcript.
//!
//! The live event stream carries no image bytes (a provider's vision output
//! is a content block on the final message, not a stream delta), so the
//! transcript shows a legible placeholder: media type, dimensions when the
//! header is readable, byte size, and alt text. A terminal-graphics
//! capability ladder (kitty/iterm2 passthrough) is the `ponytail:` upgrade
//! when the agent gains an image-producing path.

/// What is known about one image without decoding it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageInfo {
    /// The media type (`image/png`, ...).
    pub media_type: String,
    /// The byte length.
    pub bytes: usize,
    /// Pixel width, when the header was readable.
    pub width: Option<u32>,
    /// Pixel height, when the header was readable.
    pub height: Option<u32>,
    /// Alt text, when the source carried one.
    pub alt: Option<String>,
}

impl ImageInfo {
    /// Build from a media type and its bytes, sniffing dimensions.
    pub fn new(media_type: impl Into<String>, bytes: &[u8]) -> Self {
        let media_type = media_type.into();
        let (width, height) = image_dimensions(&media_type, bytes)
            .map(|(w, h)| (Some(w), Some(h)))
            .unwrap_or((None, None));
        Self {
            media_type,
            bytes: bytes.len(),
            width,
            height,
            alt: None,
        }
    }
}

/// Parse an image's pixel dimensions from its header, when cheap.
pub fn image_dimensions(media_type: &str, bytes: &[u8]) -> Option<(u32, u32)> {
    match media_type {
        "image/png" => png_dimensions(bytes),
        "image/gif" => gif_dimensions(bytes),
        "image/jpeg" => jpeg_dimensions(bytes),
        "image/webp" => webp_dimensions(bytes),
        _ => None,
    }
}

fn be_u32(bytes: &[u8], at: usize) -> Option<u32> {
    let slice = bytes.get(at..at + 4)?;
    Some(u32::from_be_bytes(slice.try_into().ok()?))
}

fn le_u16(bytes: &[u8], at: usize) -> Option<u16> {
    let slice = bytes.get(at..at + 2)?;
    Some(u16::from_le_bytes(slice.try_into().ok()?))
}

fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    // 8-byte signature, then an IHDR chunk: length(4) type(4) width(4) height(4).
    if bytes.get(..8) != Some(b"\x89PNG\r\n\x1a\n") {
        return None;
    }
    if bytes.get(12..16)? != b"IHDR" {
        return None;
    }
    Some((be_u32(bytes, 16)?, be_u32(bytes, 20)?))
}

fn gif_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.get(..6)?.starts_with(b"GIF8") {
        Some((le_u16(bytes, 6)? as u32, le_u16(bytes, 8)? as u32))
    } else {
        None
    }
}

fn jpeg_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.get(..2)? != b"\xff\xd8" {
        return None;
    }
    let mut i = 2;
    while i + 9 < bytes.len() {
        if bytes[i] != 0xff {
            i += 1;
            continue;
        }
        let marker = bytes[i + 1];
        // SOF0..SOF15 except DHT/DAC/RSTn carry the frame dimensions.
        if (0xc0..=0xcf).contains(&marker) && !matches!(marker, 0xc4 | 0xc8 | 0xcc) {
            let height = u16::from_be_bytes([bytes[i + 5], bytes[i + 6]]) as u32;
            let width = u16::from_be_bytes([bytes[i + 7], bytes[i + 8]]) as u32;
            return Some((width, height));
        }
        let length = u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]) as usize;
        if length < 2 {
            return None;
        }
        i += 2 + length;
    }
    None
}

fn webp_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.get(..4)? != b"RIFF" || bytes.get(8..12)? != b"WEBP" {
        return None;
    }
    match bytes.get(12..16)? {
        b"VP8X" => {
            let w = 1 + u32::from_le_bytes([bytes[24], bytes[25], bytes[26], 0]);
            let h = 1 + u32::from_le_bytes([bytes[27], bytes[28], bytes[29], 0]);
            Some((w, h))
        }
        b"VP8 " => {
            let w = u16::from_le_bytes([bytes[26], bytes[27]]) as u32 & 0x3fff;
            let h = u16::from_le_bytes([bytes[28], bytes[29]]) as u32 & 0x3fff;
            Some((w, h))
        }
        b"VP8L" => {
            let bits = u32::from_le_bytes([bytes[21], bytes[22], bytes[23], bytes[24]]);
            Some(((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1))
        }
        _ => None,
    }
}

fn human_bytes(n: usize) -> String {
    if n >= 1024 * 1024 {
        format!("{:.1} MB", n as f64 / (1024.0 * 1024.0))
    } else if n >= 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{n} B")
    }
}

/// Render a legible image placeholder: one or two lines naming the media
/// type, dimensions, size, and alt text. Never panics on bad input.
pub fn render_image_placeholder(info: &ImageInfo, width: usize) -> Vec<String> {
    let dimensions = match (info.width, info.height) {
        (Some(w), Some(h)) => format!("{w}×{h}"),
        _ => "unknown size".to_string(),
    };
    let mut label = format!(
        "[image {} · {} · {}]",
        info.media_type,
        dimensions,
        human_bytes(info.bytes)
    );
    if let Some(alt) = &info.alt
        && !alt.is_empty()
    {
        label.push_str(&format!(" {alt}"));
    }
    let mut lines = vec![crate::engine::text::truncate_to_width(
        &label,
        width.max(1),
        "…",
        false,
    )];
    if info.width.is_none() {
        lines.push(crate::engine::text::truncate_to_width(
            "  (terminal graphics unavailable; shown as a placeholder)",
            width.max(1),
            "…",
            false,
        ));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_dimensions_parse() {
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend_from_slice(&[0, 0, 0, 13]);
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&640u32.to_be_bytes());
        png.extend_from_slice(&480u32.to_be_bytes());
        assert_eq!(image_dimensions("image/png", &png), Some((640, 480)));
    }

    #[test]
    fn gif_dimensions_parse() {
        let mut gif = b"GIF89a".to_vec();
        gif.extend_from_slice(&320u16.to_le_bytes());
        gif.extend_from_slice(&200u16.to_le_bytes());
        assert_eq!(image_dimensions("image/gif", &gif), Some((320, 200)));
    }

    #[test]
    fn jpeg_dimensions_parse() {
        let jpeg = [
            0xff, 0xd8, // SOI
            0xff, 0xc0, 0x00, 0x11, 0x08, // SOF0, length 17, precision 8
            0x00, 0x64, // height 100
            0x00, 0xc8, // width 200
            0x03, 0x01, 0x11, 0x00, // trailing component bytes
        ];
        assert_eq!(image_dimensions("image/jpeg", &jpeg), Some((200, 100)));
    }

    #[test]
    fn placeholder_names_type_size_and_dimensions() {
        let info = ImageInfo {
            media_type: "image/png".into(),
            bytes: 2048,
            width: Some(640),
            height: Some(480),
            alt: Some("a chart".into()),
        };
        let out = render_image_placeholder(&info, 80);
        assert!(out[0].contains("image/png"));
        assert!(out[0].contains("640×480"));
        assert!(out[0].contains("2.0 KB"));
        assert!(out[0].contains("a chart"));
    }

    #[test]
    fn an_unreadable_image_still_renders_a_placeholder() {
        let info = ImageInfo::new("image/tiff", &[0, 1, 2, 3]);
        let out = render_image_placeholder(&info, 80);
        assert!(out[0].contains("image/tiff"));
        assert!(out[0].contains("unknown size"));
        assert!(out.len() > 1, "a second line explains the placeholder");
    }
}
