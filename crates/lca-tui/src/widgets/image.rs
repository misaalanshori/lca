//! Image rendering for the transcript: the kitty/iterm2 graphics ladder
//! with a legible placeholder fallback (R5).
//!
//! The terminal capabilities are detected from the environment (pi's
//! `terminal-image.ts` ladder): tmux and screen never get graphics, kitty
//! and its forks get the kitty protocol, iTerm2 gets its own, and
//! everything else gets the placeholder. `/attach` is the path that has
//! bytes today; the ladder renders wherever they exist.

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

/// The terminal graphics protocol an image can use (pi's detection ladder,
/// `terminal-image.md` §1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageProtocol {
    /// The kitty graphics protocol (kitty, ghostty, wezterm, Warp).
    Kitty,
    /// The iTerm2 inline-image protocol.
    Iterm2,
    /// No graphics: the placeholder.
    None,
}

/// Detect the terminal's graphics protocol from the environment. tmux and
/// screen are always `None`: pi's ladder calls images "unreliable under
/// tmux", and a wrong guess is invisible data loss.
pub fn detect_image_protocol() -> ImageProtocol {
    let term = std::env::var("TERM").unwrap_or_default();
    if std::env::var_os("TMUX").is_some()
        || std::env::var_os("STY").is_some()
        || term.starts_with("tmux")
        || term.starts_with("screen")
    {
        return ImageProtocol::None;
    }
    if std::env::var_os("KITTY_WINDOW_ID").is_some()
        || std::env::var_os("GHOSTTY_RESOURCES_DIR").is_some()
        || std::env::var_os("WEZTERM_PANE").is_some()
    {
        return ImageProtocol::Kitty;
    }
    if std::env::var_os("ITERM_SESSION_ID").is_some() {
        return ImageProtocol::Iterm2;
    }
    ImageProtocol::None
}

/// The cell size an image occupies at `max_width` columns, preserving its
/// aspect ratio (pi's `calculateImageCellSize`, default 9×18 cells).
fn image_cell_size(info: &ImageInfo, max_width: usize) -> (u32, u32) {
    let max_cols = max_width.max(1) as f64;
    let px_w = info.width.unwrap_or(0) as f64;
    let px_h = info.height.unwrap_or(0) as f64;
    if px_w <= 0.0 || px_h <= 0.0 {
        return (max_cols.min(60.0) as u32, 1);
    }
    let (cell_w, cell_h) = (9.0_f64, 18.0_f64);
    let natural_cols = (px_w / cell_w).ceil().max(1.0);
    let cols = natural_cols.min(max_cols).max(1.0);
    let rows = ((px_h / cell_h) * (cols / natural_cols)).ceil().max(1.0);
    (cols as u32, rows as u32)
}

/// Encode an image with the kitty graphics protocol (4096-char chunks).
pub fn encode_kitty(bytes: &[u8], columns: u32, rows: u32, image_id: u32) -> String {
    let encoded = base64_encode(bytes);
    let chunks: Vec<&[u8]> = encoded.as_bytes().chunks(4096).collect();
    let mut out = String::new();
    for (index, chunk) in chunks.iter().enumerate() {
        let more = usize::from(index + 1 < chunks.len());
        let text = std::str::from_utf8(chunk).unwrap_or("");
        out.push_str(&format!(
            "\x1b_Ga=T,f=100,q=2,C=1,c={columns},r={rows},i={image_id},m={more};{text}\x1b\\"
        ));
    }
    out
}

/// Encode an image with the iTerm2 inline-image protocol.
pub fn encode_iterm2(bytes: &[u8], columns: u32, rows: u32) -> String {
    let encoded = base64_encode(bytes);
    let name = base64_encode(b"image");
    format!(
        "\x1b]1337;File=inline=1;size={};width={columns};height={rows};name={name};preserveAspectRatio=0:{encoded}\x07",
        bytes.len()
    )
}

/// Render an image through the protocol ladder (R5): kitty → iterm2 → the
/// placeholder.
pub fn render_image(
    info: &ImageInfo,
    bytes: &[u8],
    protocol: ImageProtocol,
    max_width: usize,
) -> Vec<String> {
    let (cols, rows) = image_cell_size(info, max_width);
    match protocol {
        ImageProtocol::Kitty => vec![encode_kitty(bytes, cols, rows, 1)],
        ImageProtocol::Iterm2 => vec![encode_iterm2(bytes, cols, rows)],
        ImageProtocol::None => render_image_placeholder(info, max_width),
    }
}

/// A self-contained base64 encoder (no new dependency).
fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = chunk.get(1).copied().map(u32::from).unwrap_or(0);
        let b2 = chunk.get(2).copied().map(u32::from).unwrap_or(0);
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            TABLE[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
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

    // Verifies: R5 - the kitty encoder emits a complete chunk with its id.
    #[test]
    fn the_kitty_encoder_carries_the_id_and_closes() {
        let out = encode_kitty(&[0u8; 8], 4, 2, 7);
        assert!(out.starts_with("\x1b_Ga=T,f=100,q=2,C=1,c=4,r=2,i=7,m=0;"));
        assert!(out.ends_with("\x1b\\"));
    }

    // Verifies: R5 - the iTerm2 encoder sizes the inline file.
    #[test]
    fn the_iterm2_encoder_sizes_and_inlines() {
        let out = encode_iterm2(&[1, 2, 3], 5, 3);
        assert!(out.starts_with("\x1b]1337;File=inline=1;size=3;width=5;height=3;"));
        assert!(out.ends_with('\x07'));
    }

    // Verifies: R5 - the ladder falls back to the placeholder on no graphics.
    #[test]
    fn the_ladder_falls_back_to_the_placeholder() {
        let info = ImageInfo {
            media_type: "image/png".into(),
            bytes: 3,
            width: Some(10),
            height: Some(10),
            alt: None,
        };
        let lines = render_image(&info, &[1, 2, 3], ImageProtocol::None, 40);
        assert!(lines[0].contains("image/png"));
    }
}
