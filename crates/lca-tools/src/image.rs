//! Per-model image behavior for the read tool (#39): pi's vision
//! gate and resize contract on LCA's byte machinery.
//!
//! [`ImagePolicy`] is resolved per turn from the active model's metadata
//! and handed to the executor; unknown models behave exactly as before.

use lca_protocol::{ImageContent, ToolResult};

/// What the active model can do with images (#39: pi's `input`
/// modalities and `inputLimits.images.resize` from `docs/models.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ImageVision {
    /// Nothing is known: images pass through at original size, as today.
    #[default]
    Unknown,
    /// The model takes no images: pi's note goes back, never bytes.
    NoVision,
    /// The model takes images: downscale to the resize profile.
    Vision,
}

/// The resize profile shape lives in `lca-protocol` (re-exported here)
/// so the guest target shares it; the executor behavior stays here.
pub use lca_protocol::{IMAGE_RESIZE_EXTRA, IMAGE_VISION_EXTRA, ImageResize};

/// The read tool's image behavior for one executor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ImagePolicy {
    /// What the active model can do with images.
    pub vision: ImageVision,
    /// The resize profile; a vision model without one keeps pi's
    /// conservative defaults (`ImageResize::default`).
    pub resize: Option<ImageResize>,
}

impl ImagePolicy {
    /// Nothing is known about the model: today's behavior, bytes
    /// pass through untouched.
    pub fn unknown() -> Self {
        Self::default()
    }

    /// The policy for a resolved model's extras (#39). `"true"` is a
    /// vision model (the profile when one rides along, pi's conservative
    /// defaults otherwise); `"false"` gets the no-vision note; anything
    /// else is unknown and passes bytes through as today.
    pub fn for_extras(extras: &std::collections::BTreeMap<String, String>) -> Self {
        match extras.get(IMAGE_VISION_EXTRA).map(String::as_str) {
            Some("true") => ImagePolicy {
                vision: ImageVision::Vision,
                resize: extras
                    .get(IMAGE_RESIZE_EXTRA)
                    .and_then(|value| parse_resize_extra(value)),
            },
            Some("false") => ImagePolicy {
                vision: ImageVision::NoVision,
                resize: None,
            },
            _ => ImagePolicy::unknown(),
        }
    }
}

/// Parse an [`IMAGE_RESIZE_EXTRA`] value (`WIDTHxHEIGHT:BYTES`).
fn parse_resize_extra(value: &str) -> Option<ImageResize> {
    let (dims, bytes) = value.split_once(':')?;
    let (width, height) = dims.split_once('x')?;
    Some(ImageResize {
        max_width: width.parse().ok()?,
        max_height: height.parse().ok()?,
        max_bytes: bytes.parse().ok()?,
    })
}

/// Pi's explicit note for a model without vision, verbatim: it is the
/// contract models rely on, not prose to paraphrase (#39).
pub const NO_VISION_NOTE: &str =
    "[Current model does not support images. The image will be omitted from this request.]";
/// Downscale image bytes to a resize profile, encoded as JPEG (pi's
/// `processImage` contract: dimensions fit the profile, quality starts
/// at 80 and steps down until the payload fits `max_bytes`). `None`
/// when the bytes decode to nothing we recognize — the caller then
/// passes them through untouched rather than failing the read.
fn downscale_image(bytes: &[u8], profile: &ImageResize) -> Option<(Vec<u8>, u32, u32)> {
    let decoded = image::load_from_memory(bytes).ok()?;
    let (width, height) = (decoded.width(), decoded.height());
    let scale = (f64::from(profile.max_width) / f64::from(width))
        .min(f64::from(profile.max_height) / f64::from(height))
        .min(1.0);
    let fitted = if scale < 1.0 {
        decoded.resize(
            (f64::from(width) * scale) as u32,
            (f64::from(height) * scale) as u32,
            image::imageops::FilterType::Triangle,
        )
    } else {
        decoded
    };
    let rgb = fitted.to_rgb8();
    let (width, height) = (rgb.width(), rgb.height());
    // ponytail: three quality steps, not a search — dimensions do the
    // real work, and the smallest step always fits anything sane.
    let mut smallest = None;
    for quality in [80u8, 60, 40] {
        let mut buf = Vec::new();
        let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality);
        encoder.encode_image(&rgb).ok()?;
        if buf.len() <= profile.max_bytes {
            return Some((buf, width, height));
        }
        smallest = Some(buf);
    }
    smallest.map(|buf| (buf, width, height))
}

/// The image branch of `read`: pi's vision gate and resize profile.
/// Unknown models behave exactly as before — original bytes at original
/// size — and undecodable bytes pass through rather than failing the read.
pub(crate) fn read_image_result(
    policy: &ImagePolicy,
    auto_resize: bool,
    call_id: &str,
    media_type: &str,
    bytes: Vec<u8>,
) -> ToolResult {
    if policy.vision == ImageVision::NoVision {
        return ToolResult::ok(call_id.to_string(), NO_VISION_NOTE.to_string());
    }
    // Gh #82: without auto-resize the original bytes ride through.
    let profile = (auto_resize && policy.vision == ImageVision::Vision)
        .then(|| policy.resize.unwrap_or_default());
    let passthrough = |bytes: Vec<u8>| {
        let mut result = ToolResult::ok(
            call_id.to_string(),
            format!("[image {media_type}, {} bytes]", bytes.len()),
        );
        result.images.push(ImageContent {
            media_type: media_type.to_string(),
            bytes,
        });
        result
    };
    let Some(profile) = profile else {
        return passthrough(bytes);
    };
    match downscale_image(&bytes, &profile) {
        Some((resized, width, height)) => {
            let mut result = ToolResult::ok(
                call_id.to_string(),
                format!(
                    "[image image/jpeg, {} bytes, resized to {width}x{height}]",
                    resized.len()
                ),
            );
            result.images.push(ImageContent {
                media_type: "image/jpeg".to_string(),
                bytes: resized,
            });
            result
        }
        None => passthrough(bytes),
    }
}

/// A **display-sized PNG** from any image the `image` crate can decode (gh #599).
///
/// The kitty graphics protocol names no code for JPEG (gh #598), so a resized vision
/// attachment - which [`downscale_image`] emits as JPEG q80 - could not be sent and
/// took the placeholder: a label where a picture belongs. The terminal can draw a
/// PNG, so the display path converts at **render** time instead of storing a second
/// copy. The size is `cells × cell_pixels`: the `c=`/`r=` box the renderer chose,
/// stretched to the terminal's own cell. That is the point of deriving rather than
/// persisting - a display copy's correct size is the pane's, which the session cannot
/// know when it writes the record (#599's measurement: 2.06x the wire JPEG at 60
/// cells, 0.85x at 40).
///
/// Nothing is upscaled: [`image::DynamicImage::resize`] fits *within* the box, so a
/// small picture keeps its own pixels and the terminal's `c=`/`r=` stretch does the
/// scaling, exactly as a PNG's does today. Returns `None` when the bytes do not
/// decode, so a caller falls back to what it has: a renderer never fails a frame
/// because a picture would not decode.
pub fn display_png(
    bytes: &[u8],
    cells: (u32, u32),
    cell_pixels: (f64, f64),
) -> Option<(Vec<u8>, u32)> {
    let decoded = image::load_from_memory(bytes).ok()?;
    let cell_w = cell_pixels.0.max(1.0);
    let cell_h = cell_pixels.1.max(1.0);
    let max_w = (f64::from(cells.0.max(1)) * cell_w).round().max(1.0) as u32;
    let max_h = (f64::from(cells.1.max(1)) * cell_h).round().max(1.0) as u32;
    // Fit within the box **and never past the source's own pixels**: a small JPEG in a
    // large pane must not be upscaled (it would cost bytes for a blurrier picture), and
    // the ladder's `c=`/`r=` stretch already scales it on the terminal side, exactly as
    // it does a PNG's.
    //
    // `thumbnail` rather than `resize`: the `image` crate documents it as the cheaper
    // path for shrinking, because it samples rather than filtering the whole source, and
    // it keeps the aspect ratio inside the box. Measured on the read tool's 1280×853
    // q80 output at 60 cells × 9×18 px: **16-17 ms against 52-58 ms** for
    // `resize(Triangle)`, at identical output dimensions.
    let fitted = if decoded.width() > max_w || decoded.height() > max_h {
        decoded.thumbnail(max_w, max_h)
    } else {
        decoded
    };
    let height = fitted.height();
    let mut png = Vec::new();
    fitted
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .ok()?;
    Some((png, height))
}

/// Encode clipboard RGBA rows as PNG (gh #126): the clipboard hands
/// raw row-major bytes; the attachment store wants a real image file.
/// Dimensions must be non-zero and match the row length, or the paste
/// refuses instead of storing a corrupt file. PNG always, never the
/// clipboard's own format: one stored shape keeps the sniff/resize
/// path single, and PNG is lossless.
pub fn clipboard_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    if width == 0 || height == 0 {
        return Err("the clipboard image has no size".to_string());
    }
    if rgba.len() != width as usize * height as usize * 4 {
        return Err(format!(
            "the clipboard rows ({} bytes) do not fill {width}x{height} RGBA",
            rgba.len()
        ));
    }
    let pixels = image::RgbaImage::from_raw(width, height, rgba.to_vec())
        .ok_or_else(|| "the clipboard bytes are not RGBA rows".to_string())?;
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(pixels)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|err| format!("cannot encode the clipboard image: {err}"))?;
    Ok(png)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Verifies: gh #599 - the display derivative is a real PNG of the requested
    // cell box, and bytes that do not decode answer `None` (the renderer's
    // fallback is the placeholder, not a failed frame).
    #[test]
    fn display_png_fits_the_cell_box_and_refuses_undecodable_bytes() {
        let rgb = image::RgbImage::from_fn(400, 300, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 90])
        });
        let mut jpeg = Vec::new();
        let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 80);
        encoder.encode_image(&rgb).expect("encode");
        // Two cells wide, one tall at the estimate: 18x18 px is the fitting box,
        // and the aspect is preserved inside it (300/400 -> 13 rows of pixels).
        let (png, height) = display_png(&jpeg, (2, 1), (9.0, 18.0)).expect("a JPEG decodes");
        assert_eq!(
            lca_protocol::sniff_image_media_type(&png),
            Some("image/png")
        );
        let decoded = image::load_from_memory(&png).expect("the derivative decodes");
        assert_eq!(decoded.width(), 18, "the box's pixel width");
        assert_eq!(decoded.height(), 14, "and the aspect that fits inside it");
        assert_eq!(height, 14, "the payload's height rides back for the crop");
        assert!(
            display_png(b"not an image at all", (2, 1), (9.0, 18.0)).is_none(),
            "undecodable bytes are the caller's fallback, not a panic"
        );
        // A box larger than the picture is the **terminal's** to stretch: upscaling
        // here would spend bytes on a blurrier copy, so the payload keeps its own
        // pixels and the `c=`/`r=` box does the scaling, as it does for a PNG.
        let small = image::RgbImage::from_fn(12, 9, |_, _| image::Rgb([10, 20, 30]));
        let mut small_png = Vec::new();
        image::DynamicImage::ImageRgb8(small)
            .write_to(
                &mut std::io::Cursor::new(&mut small_png),
                image::ImageFormat::Png,
            )
            .expect("encode");
        let (fitted_png, small_height) =
            display_png(&small_png, (60, 20), (9.0, 18.0)).expect("a PNG decodes");
        let decoded = image::load_from_memory(&fitted_png).expect("decodes");
        assert_eq!(
            (decoded.width(), decoded.height()),
            (12, 9),
            "a picture smaller than its box is not upscaled"
        );
        assert_eq!(small_height, 9, "and its height is what the crop must use");
    }

    // Verifies: gh #126 - clipboard RGBA rows encode to a PNG the
    // attachment store accepts (the mocked clipboard source: no
    // system clipboard in tests); bad shapes refuse.
    #[test]
    fn clipboard_rgba_encodes_to_a_storable_png() {
        // 2x1: red, then green, row-major RGBA.
        let rgba = vec![255, 0, 0, 255, 0, 255, 0, 255];
        let png = clipboard_png(2, 1, &rgba).expect("encodes");
        assert_eq!(
            lca_protocol::sniff_image_media_type(&png),
            Some("image/png"),
            "the store sniffs what the clipboard encoded"
        );
        assert!(clipboard_png(0, 1, &rgba).is_err(), "empty dims refuse");
        assert!(clipboard_png(2, 2, &rgba).is_err(), "short rows refuse");
    }
}
