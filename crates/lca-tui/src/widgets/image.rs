//! Image rendering for the transcript: the kitty/iterm2 graphics ladder
//! with a legible placeholder fallback (R5).
//!
//! # Identity and rows (gh #324 gap 1)
//!
//! Two properties decide whether a transcript of images renders at all, and the
//! filed bug got both wrong: it hardcoded `i=1`, so a second image overwrote the
//! first, and it returned **one** line for an image `rows` tall, so the entry
//! after it painted over the image's own rows.
//!
//! **Identity is derived, not allocated** ([`kitty_image_id`]). A monotonic
//! counter would fix the collision and hand every repaint a *new* id, which makes
//! each frame a fresh upload; the kitty protocol rewards the opposite. The id is a
//! hash of the payload **and the cell scale**, because the scale is part of a
//! placement: the same image reflowed to a new width is a different upload, and
//! the same image at the same width is the same upload. That also makes the two
//! content-addressed properties fall out for free - a repaint is idempotent, and
//! the terminal's texture cache dedups for us.
//!
//! **The row count is the cell height** ([`image_cell_size`]), and the returned
//! vector is exactly that tall: the sequence on row 0, blanks below. Kitty is told
//! `C=1`, so its cursor never moves and the host's own cursor accounting stays
//! correct while it writes the blanks. iTerm2 moves its cursor to the bottom of
//! the block *itself*, so its sequence goes on the **last** row, prefixed with
//! `CSI {rows-1} A` - which is why [`crate::engine::text`]' vocabulary accepts the
//! cursor movements `A B C D`.
//!
//! # What a repaint emits
//!
//! A **placement**, not a transmission: `a=p,q=2` with the same `i=` and no
//! payload ([`kitty_placement`]). Scrolling past an image and back is then a few
//! dozen bytes rather than a base64 re-upload. The transcript layer holds the
//! cache that decides this; this module only states the two forms and the rules
//! for reading them back.
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
    // gh #358: the magic proves only 16 bytes. Each arm reads further, and
    // used to index directly, so any 16..=29-byte file with the magic
    // panicked in the render path. Every read goes through a checked
    // slice now; a truncated header simply reports no dimensions and the
    // caller draws the placeholder.
    match bytes.get(12..16)? {
        b"VP8X" => {
            // Canvas width/height are 24-bit LE, stored minus one.
            let w = bytes.get(24..27)?;
            let h = bytes.get(27..30)?;
            Some((
                1 + u32::from_le_bytes([w[0], w[1], w[2], 0]),
                1 + u32::from_le_bytes([h[0], h[1], h[2], 0]),
            ))
        }
        b"VP8 " => {
            let w = bytes.get(26..28)?;
            let h = bytes.get(28..30)?;
            Some((
                u16::from_le_bytes([w[0], w[1]]) as u32 & 0x3fff,
                u16::from_le_bytes([h[0], h[1]]) as u32 & 0x3fff,
            ))
        }
        b"VP8L" => {
            let bits = u32::from_le_bytes(bytes.get(21..25)?.try_into().ok()?);
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
    detect_image_protocol_with(|key| std::env::var(key).ok())
}

/// [`detect_image_protocol`] with an injected environment, so the
/// multiplexer-unsafety rule can be pinned without mutating the test process's
/// env (gh #405: this behaviour is **at parity with pi** and gets a guard, not a
/// re-file).
pub fn detect_image_protocol_with(env: impl Fn(&str) -> Option<String>) -> ImageProtocol {
    let term = env("TERM").unwrap_or_default();
    if env("TMUX").is_some()
        || env("STY").is_some()
        || term.starts_with("tmux")
        || term.starts_with("screen")
    {
        return ImageProtocol::None;
    }
    if env("KITTY_WINDOW_ID").is_some()
        || env("GHOSTTY_RESOURCES_DIR").is_some()
        || env("WEZTERM_PANE").is_some()
    {
        return ImageProtocol::Kitty;
    }
    if env("ITERM_SESSION_ID").is_some() {
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
    // Cap the rows: an extreme aspect ratio would otherwise ask for a body
    // taller than any terminal, and the float-to-int cast would saturate at
    // u32::MAX (a nonsense placement).
    // ponytail: a 200-row cap; thread a real max height through if a widget
    // ever needs to fit an image to its viewport exactly.
    let rows = ((px_h / cell_h) * (cols / natural_cols))
        .ceil()
        .clamp(1.0, 200.0);
    (cols as u32, rows as u32)
}

/// The default cell size, in pixels, used when the terminal has not answered
/// `CSI 16 t` (pi's `calculateImageCellSize` default, `terminal-image.ts`).
///
/// **An estimate, and stated as one.** Every reservation a terminal gets without
/// the probe is this guess: a 9x18 font is the common case and a retina or
/// 10x22 font reflows the image slightly taller or shorter than the rows it
/// claims. Gh #324 gap 2 is the probe that replaces it.
pub const DEFAULT_CELL_PIXELS: (f64, f64) = (9.0, 18.0);

/// The image id for a placement: a hash of the payload **and** the cell scale.
///
/// Stable for the same image at the same width, so a repaint re-places rather
/// than re-uploads; distinct across images, so two of them cannot overwrite each
/// other; and different at a new width, because the scale *is* the placement.
///
/// `0` is never returned: the kitty protocol reads `i=0` as "no id", so a hash
/// that landed there would silently fall back to a terminal-chosen id.
pub fn kitty_image_id(bytes: &[u8], columns: u32, rows: u32) -> u32 {
    // FNV-1a, then a final mix. This is a collision *breaker*, not a security
    // boundary: two different images sharing a 32-bit id would be a rendering
    // bug, not an exploit, and the protocol's id space is 32-bit anyway.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in [columns as u8, rows as u8, 0, 0]
        .iter()
        .chain((columns >> 8).to_le_bytes().iter())
        .chain((rows >> 8).to_le_bytes().iter())
        .chain(bytes.iter())
    {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let mixed = (hash ^ (hash >> 32)) as u32;
    mixed.max(1)
}

/// [`image_cell_size`] for a sibling crate's tests.
///
/// `lca-ui` asserts the reservation *number*, and recomputing it there would be a
/// second implementation of the arithmetic the row is checking - the row would
/// then pass while agreeing with itself. The function itself stays private.
#[doc(hidden)]
#[must_use]
pub fn image_cell_size_for_test(info: &ImageInfo, max_width: usize) -> (u32, u32) {
    image_cell_size(info, max_width)
}

/// Encode an image with the kitty graphics protocol (4096-char chunks).
pub fn encode_kitty(bytes: &[u8], columns: u32, rows: u32, image_id: u32) -> String {
    let encoded = base64_encode(bytes);
    let chunks: Vec<&[u8]> = encoded.as_bytes().chunks(4096).collect();
    let mut out = String::new();
    for (index, chunk) in chunks.iter().enumerate() {
        let text = std::str::from_utf8(chunk).unwrap_or("");
        if index == 0 {
            // The header carries everything: the action, the format, the box, the
            // id, and `m=1` when more follows.
            let more = usize::from(chunks.len() > 1);
            out.push_str(&format!(
                "\x1b_Ga=T,f=100,q=2,C=1,c={columns},r={rows},i={image_id},m={more};{text}\x1b\\"
            ));
        } else {
            // A continuation is a **bare** chunk: `m=1` while more follow, `m=0` on
            // the last. Repeating the header on every chunk (what this did before
            // gh #324 gap 1) is accepted by the protocol but is three times the
            // bytes and disagrees with pi - verified against pi 1.3.0, which emits
            // `\x1b_Gm=1;`/`\x1b_Gm=0;` exactly like this.
            let more = usize::from(index + 1 < chunks.len());
            out.push_str(&format!("\x1b_Gm={more};{text}\x1b\\"));
        }
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
        // Kitty is told `C=1`, so the sequence leaves the cursor on the block's
        // first row and the blanks after it are what the host writes there.
        ImageProtocol::Kitty => {
            let id = kitty_image_id(bytes, cols, rows);
            let mut lines = Vec::with_capacity(rows as usize);
            lines.push(encode_kitty(bytes, cols, rows, id));
            lines.resize(rows as usize, String::new());
            lines
        }
        // iTerm2 moves its cursor to the bottom of the inline image by itself,
        // so the reserve comes *first* and the last row walks back up to the top
        // before emitting. That leaves the host's cursor at the block's bottom,
        // where the next row belongs.
        ImageProtocol::Iterm2 => {
            let mut lines = vec![String::new(); rows.saturating_sub(1) as usize];
            let up = if rows > 1 {
                format!("\x1b[{}A", rows - 1)
            } else {
                String::new()
            };
            lines.push(format!("{up}{}", encode_iterm2(bytes, cols, rows)));
            lines
        }
        ImageProtocol::None => render_image_placeholder(info, max_width),
    }
}

/// The **placement-only** form of a kitty image line: the same id and scale, no
/// payload (gh #324 gap 1).
///
/// Once the terminal holds the texture for `i=`, re-sending it is waste;
/// `a=p` says "put the image you already have at this spot". Returns `None` when
/// the line is not a kitty placement this module produced (the placeholder path,
/// or a multi-chunk transmission whose id cannot be read from the first line).
pub fn kitty_placement(line: &str) -> Option<String> {
    // pi's order: `a=p,q=2` then `C,c,r,i` (`terminal-image.ts`'s
    // `KITTY_PLACEMENT_CONTROL_KEYS` filter preserves the upload's order). The
    // protocol does not constrain parameter order, so matching pi is free - and it
    // turns a side-by-side into a byte comparison rather than a field-by-field
    // argument. Measured against pi 1.3.0: `\x1b_Ga=p,q=2,C=1,c=12,r=6,i=4242\x1b\\`.
    let (id, cols, rows) = kitty_image_control(line)?;
    let cursor = if kitty_params(line)?.split(',').any(|p| p == "C=1") {
        "C=1,"
    } else {
        ""
    };
    Some(format!(
        "\x1b_Ga=p,q=2,{cursor}c={cols},r={rows},i={id}\x1b\\"
    ))
}

/// The seed for a fresh upload. Always [`encode_kitty`], so a renderer that has
/// no cache to consult gets the transmission form and the terminal still works -
/// the cache buys smoothness, never correctness.
pub fn kitty_upload(bytes: &[u8], line: &str) -> Option<String> {
    let (id, cols, rows) = kitty_image_control(line)?;
    Some(encode_kitty(bytes, cols, rows, id))
}

/// Read `(id, columns, rows)` back out of a kitty line.
fn kitty_image_control(line: &str) -> Option<(u32, u32, u32)> {
    let params = kitty_params(line)?;
    let mut id = None;
    let mut cols = 0;
    let mut rows = 0;
    for param in params.split(',') {
        let Some((key, value)) = param.split_once('=') else {
            continue;
        };
        // Only the three keys this reads must parse; `a=T,f=100,q=2,m=0` are
        // keywords and numbers this function has no opinion about.
        let parsed: Option<u32> = value.parse().ok();
        match key {
            "i" => id = parsed,
            "c" => cols = parsed.unwrap_or(0),
            "r" => rows = parsed.unwrap_or(0),
            _ => {}
        }
    }
    Some((id?, cols, rows))
}

/// The control block of the **last** kitty command on a line (a multi-chunk
/// transmission puts `m=1` continuations after the header).
fn kitty_params(line: &str) -> Option<&str> {
    let start = line.find("\x1b_G")? + 3;
    let end = line[start..].find(';')? + start;
    Some(&line[start..end])
}

/// How many rows a kitty image line reserves, from its own `r=`.
///
/// The renderer and the transcript both need this and neither may guess: a
/// one-row answer for an `r=10` image is precisely the defect. `None` means the
/// line is not an image.
pub fn kitty_reserved_rows(line: &str) -> Option<u32> {
    let (_id, _cols, rows) = kitty_image_control(line)?;
    Some(rows.max(1))
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

    // Verifies: gh #358 - a truncated WebP header of ANY length never
    // panics. The magic needs 16 bytes; the three chunk arms read up to
    // byte 29 (VP8X/VP8) or 24 (VP8L), so every short input used to index
    // out of bounds in the render path. The whole 0..=30 matrix, not
    // three samples.
    #[test]
    fn truncated_webp_headers_never_panic() {
        for kind in [b"VP8X", b"VP8 ", b"VP8L"] {
            for len in 0..=30usize {
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
                let got = image_dimensions("image/webp", &bytes);
                // Short of the arm's requirement the header is unreadable;
                // a complete one is not truncated, so it may parse.
                let needs = if kind == b"VP8L" { 25 } else { 30 };
                if len < needs {
                    assert_eq!(
                        got,
                        None,
                        "{} at {len} bytes is a truncated header",
                        String::from_utf8_lossy(kind)
                    );
                }
            }
        }
        assert_eq!(image_dimensions("image/webp", &[]), None, "empty input");
    }

    // Verifies: gh #358 AC2 - the three chunk kinds still parse their real
    // dimensions (the safe reads did not change the arithmetic).
    #[test]
    fn webp_dimensions_parse_each_chunk_kind() {
        // VP8X: 30 bytes, 24-bit LE canvas size minus one at 24..30.
        let mut vp8x = webp_header(b"VP8X", 30);
        vp8x[24..27].copy_from_slice(&[0x2f, 0x02, 0x00]); // 560 - 1
        vp8x[27..30].copy_from_slice(&[0x8f, 0x01, 0x00]); // 400 - 1
        assert_eq!(image_dimensions("image/webp", &vp8x), Some((560, 400)));

        // VP8 (lossy): 30 bytes, 14-bit dimensions after the 3-byte frame
        // tag + start code at 26..30.
        let mut vp8 = webp_header(b"VP8 ", 30);
        vp8[26..28].copy_from_slice(&320u16.to_le_bytes());
        vp8[28..30].copy_from_slice(&240u16.to_le_bytes());
        assert_eq!(image_dimensions("image/webp", &vp8), Some((320, 240)));

        // VP8L (lossless): 25 bytes, 14-bit (w-1) then 14-bit (h-1).
        let mut vp8l = webp_header(b"VP8L", 25);
        let bits: u32 = (639 & 0x3fff) | ((479 & 0x3fff) << 14);
        vp8l[21..25].copy_from_slice(&bits.to_le_bytes());
        assert_eq!(image_dimensions("image/webp", &vp8l), Some((640, 480)));
    }

    // Verifies: gh #358 - the class, not the samples: no byte buffer at
    // all panics the dimension sniffer for any supported media type. A
    // panic here is fatal because `image_dimensions` runs on every
    // transcript render and resize.
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

        #[test]
        fn no_byte_buffer_panics_the_dimension_sniffer(
            bytes in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..2048),
        ) {
            for media_type in ["image/png", "image/gif", "image/jpeg", "image/webp"] {
                let _ = image_dimensions(media_type, &bytes);
            }
        }

        /// A buffer that starts with the WebP magic and *then* arbitrary
        /// bytes is the exact shape that used to panic.
        #[test]
        fn no_magic_prefixed_webp_buffer_panics(
            kind in 0usize..3,
            tail in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..64),
        ) {
            let fourcc: &[u8; 4] = [b"VP8X", b"VP8 ", b"VP8L"][kind];
            let mut bytes = vec![0u8; 16];
            bytes[..4].copy_from_slice(b"RIFF");
            bytes[8..12].copy_from_slice(b"WEBP");
            bytes[12..16].copy_from_slice(fourcc);
            bytes.extend_from_slice(&tail);
            let _ = image_dimensions("image/webp", &bytes);
        }
    }

    /// A WebP byte buffer of `len` zeros carrying the RIFF/WEBP magic and
    /// the given chunk fourcc at 12..16.
    fn webp_header(kind: &[u8; 4], len: usize) -> Vec<u8> {
        let mut bytes = vec![0u8; len];
        bytes[..4].copy_from_slice(b"RIFF");
        bytes[8..12].copy_from_slice(b"WEBP");
        bytes[12..16].copy_from_slice(kind);
        bytes
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

    // Verifies: gh #324 gap 1 - identity is **unique and stable**. Two different
    // images must not share an id (the filed bug hardcoded `i=1`), and the same
    // image at the same scale must keep its id, or every repaint is a re-upload.
    #[test]
    fn image_ids_are_unique_per_payload_and_stable_across_renders() {
        let a = kitty_image_id(b"first-image-bytes", 10, 5);
        let b = kitty_image_id(b"second-image-bytes", 10, 5);
        assert_ne!(a, b, "two payloads must not share an id");
        assert_eq!(
            a,
            kitty_image_id(b"first-image-bytes", 10, 5),
            "the same image at the same scale re-derives its id"
        );
        assert_ne!(
            a,
            kitty_image_id(b"first-image-bytes", 20, 10),
            "a new scale is a new placement, so a new id"
        );
        assert_ne!(kitty_image_id(b"", 1, 1), 0, "0 means \"no id\" to kitty");
        assert_ne!(kitty_image_id(b"x", 1, 1), 0);
    }

    // Verifies: R5 + gh #324 gap 1 - the chunked upload matches pi's shape, which
    // is the part a placement only ever *replaces*: the terminal must accept the
    // first frame from either implementation. pi 1.3.0 (measured with node against
    // `~/gits/pi`): header carries `...,m=1;`, middle chunks are `\x1b_Gm=1;`,
    // and the last is `\x1b_Gm=0;`.
    #[test]
    fn the_chunked_upload_matches_pi_shape() {
        let big = vec![7u8; 7000];
        let seq = encode_kitty(&big, 40, 20, 99);
        let parts: Vec<&str> = seq.split("\x1b\\").filter(|p| !p.is_empty()).collect();
        assert_eq!(parts.len(), 3, "7000 bytes base64 into 4096-char chunks");
        assert!(
            parts[0].starts_with("\x1b_Ga=T,f=100,q=2,C=1,c=40,r=20,i=99,m=1;"),
            "the header chunk: {:?}",
            &parts[0][..60.min(parts[0].len())]
        );
        assert!(
            parts[1].starts_with("\x1b_Gm=1;"),
            "a middle chunk: {:?}",
            &parts[1][..20.min(parts[1].len())]
        );
        assert!(
            parts[2].starts_with("\x1b_Gm=0;"),
            "the last chunk closes: {:?}",
            &parts[2][..20.min(parts[2].len())]
        );
    }

    // Verifies: gh #324 gap 1 - **`render_image` actually uses the derived id.**
    // The row above pins the derivation and this one pins the wiring: with only
    // the first, hardcoding `i=1` at the call site passes every local row and is
    // caught only by a sibling crate's transcript row. Measured, not assumed -
    // that mutation was run against this module and all 22 rows stayed green.
    #[test]
    fn the_kitty_render_embeds_the_derived_id_and_two_images_differ() {
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend_from_slice(&[0, 0, 0, 13]);
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&90u32.to_be_bytes());
        png.extend_from_slice(&180u32.to_be_bytes());
        png.extend_from_slice(&[0; 12]);
        let first = render_image(
            &ImageInfo::new("image/png", &png),
            &png,
            ImageProtocol::Kitty,
            60,
        );
        let mut other = png.clone();
        other.push(0x7f); // a different payload, same dimensions
        let second = render_image(
            &ImageInfo::new("image/png", &other),
            &other,
            ImageProtocol::Kitty,
            60,
        );
        let (a, b) = (
            kitty_image_control(&first[0]),
            kitty_image_control(&second[0]),
        );
        let (Some((a_id, _, _)), Some((b_id, _, _))) = (a, b) else {
            panic!("both lines carry an id: {:?} {:?}", first[0], second[0]);
        };
        assert_eq!(
            a_id,
            kitty_image_id(&png, 10, 10),
            "the embedded id is the derived one, not a literal"
        );
        assert_ne!(a_id, b_id, "two payloads get two ids through render_image");
        assert_ne!(a_id, 1, "and the filed bug's literal is not it");
    }

    // Verifies: gh #324 gap 1 - the kitty render reserves the image's cell rows:
    // a header and then blanks, exactly `rows` long, so the entry after the image
    // starts below it instead of painting over it.
    #[test]
    fn the_kitty_render_reserves_exactly_the_cell_rows() {
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend_from_slice(&[0, 0, 0, 13]);
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&90u32.to_be_bytes());
        png.extend_from_slice(&180u32.to_be_bytes());
        png.extend_from_slice(&[0; 12]);
        let info = ImageInfo::new("image/png", &png);
        let lines = render_image(&info, &png, ImageProtocol::Kitty, 60);
        // 90x180 px on a 9x18 cell is 10 columns by 10 rows, and 10 columns fit
        // inside the 60-column cap so nothing is scaled down.
        assert_eq!(lines.len(), 10, "the vector is the reservation");
        assert_eq!(kitty_reserved_rows(&lines[0]), Some(10), "r= states it too");
        assert!(
            lines[1..].iter().all(|l| l.is_empty()),
            "every reserved row below the header is blank: {lines:?}"
        );
    }

    // Verifies: gh #324 gap 1 - the iTerm2 shape puts its reserve first and walks
    // back up on the last row, because iTerm2 moves the cursor itself. Pinned
    // because the two protocols are the mirror image of each other and swapping
    // them would leave the cursor outside the block.
    #[test]
    fn the_iterm2_render_reserves_rows_and_walks_back_up() {
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend_from_slice(&[0, 0, 0, 13]);
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&90u32.to_be_bytes());
        png.extend_from_slice(&180u32.to_be_bytes());
        png.extend_from_slice(&[0; 12]);
        let info = ImageInfo::new("image/png", &png);
        let lines = render_image(&info, &png, ImageProtocol::Iterm2, 60);
        assert_eq!(lines.len(), 10);
        assert!(
            lines[..9].iter().all(|l| l.is_empty()),
            "the reserve comes first for iTerm2: {lines:?}"
        );
        assert!(
            lines[9].starts_with("\x1b[9A\x1b]1337;File="),
            "{:?}",
            lines[9]
        );
    }

    // Verifies: gh #324 gap 1 - **a repaint places, it does not transmit.** The
    // placement form drops the base64 entirely and names the same id and box, so
    // scrolling back over an image costs bytes rather than a re-upload.
    #[test]
    fn the_placement_form_drops_the_payload_and_keeps_the_id_and_box() {
        let upload = encode_kitty(&[7u8; 64], 12, 6, 4242);
        let placement = kitty_placement(&upload).expect("a placement");
        // pi's exact bytes for the same image (`getKittyImagePlacement`, measured
        // with node against `~/gits/pi`): parameter order is free in the protocol,
        // so matching pi costs nothing and makes a byte-level side-by-side possible.
        assert_eq!(placement, "\x1b_Ga=p,q=2,C=1,c=12,r=6,i=4242\x1b\\");
        assert!(!placement.contains("AAAA"), "no payload: {placement}");
        // The property is "the payload is gone", not a hand-picked ratio: a
        // 64-byte image's base64 is short enough that any factor of it would be
        // an arbitrary number. A real transcript image is kilobytes, where the
        // ratio is not in doubt.
        let payload = base64_encode(&[7u8; 64]);
        assert!(!upload.contains(&payload) || upload.len() > payload.len());
        assert!(
            !placement.contains(&payload),
            "the base64 body is not in a placement"
        );
        let big = encode_kitty(&[7u8; 8192], 12, 6, 4242);
        assert!(
            kitty_placement(&big).is_some_and(|p| p.len() * 100 < big.len()),
            "at transcript sizes a placement is a tiny fraction of an upload"
        );
        assert_eq!(kitty_upload(&[7u8; 64], &upload), Some(upload.clone()));
        assert_eq!(kitty_placement("just text"), None);
        assert_eq!(kitty_placement(""), None);
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

#[cfg(test)]
mod width_tests {
    use super::*;

    // Verifies: gh #82 - the caller width caps the cells (pi's
    // `terminal.imageWidthCells` rides this parameter): a 640px image
    // in 30 columns renders 30 wide, uncapped past its nature.
    #[test]
    fn caller_width_caps_the_cells() {
        let info = ImageInfo {
            media_type: "image/png".into(),
            bytes: 2048,
            width: Some(640),
            height: Some(480),
            alt: None,
        };
        assert_eq!(image_cell_size(&info, 30).0, 30);
        assert_eq!(image_cell_size(&info, 200).0, 72, "natural 640/9 ceiling");
    }
}
