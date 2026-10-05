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
    call_id: &str,
    media_type: &str,
    bytes: Vec<u8>,
) -> ToolResult {
    if policy.vision == ImageVision::NoVision {
        return ToolResult::ok(call_id.to_string(), NO_VISION_NOTE.to_string());
    }
    let profile = (policy.vision == ImageVision::Vision).then(|| policy.resize.unwrap_or_default());
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
