//! FileRead image reading: detect → decode → resize/re-encode to fit
//! claude-code's size/dimension budget. Ports the resize ladder of
//! claude-code `src/utils/imageResizer.ts`; Rust how-to mirrors codex
//! `utils/image`. Functional-parity (the `image` crate lacks sharp's PNG-palette
//! and WebP-quality knobs — oversized PNG/WebP fall to the JPEG ladder).

use base64::Engine;
use image::codecs::jpeg::JpegEncoder;
use image::{DynamicImage, GenericImageView, ImageFormat};

/// claude-code `IMAGE_TARGET_RAW_SIZE` (apiLimits.ts): API_IMAGE_MAX_BASE64_SIZE*3/4 = 3.75 MB.
const IMAGE_TARGET_RAW_SIZE: usize = 3_932_160;
/// claude-code `IMAGE_MAX_WIDTH`/`IMAGE_MAX_HEIGHT`.
pub const IMAGE_MAX_DIM: u32 = 2000;
/// claude-code secondary-shrink width @ quality 20 (last resort).
const SECONDARY_SHRINK_WIDTH: u32 = 1000;
/// claude-code JPEG quality ladder (imageResizer.ts).
const JPEG_QUALITY_LADDER: [u8; 4] = [80, 60, 40, 20];

/// Image extensions claude-code routes to the image path (FileReadTool.ts:188).
#[must_use]
pub fn is_image_extension(path: &std::path::Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("png" | "jpg" | "jpeg" | "gif" | "webp")
    )
}

/// A processed image ready for an `ImageSource::Base64`.
pub struct ProcessedImage {
    /// Base64-encoded (standard alphabet) processed image bytes for the Anthropic source.
    pub base64: String,
    /// `image/png` | `image/jpeg` | `image/gif` | `image/webp`.
    pub media_type: String,
    /// `(orig_w, orig_h, disp_w, disp_h)` — present only when the image was resized.
    pub resized: Option<(u32, u32, u32, u32)>,
}

fn format_to_media_type(fmt: Option<ImageFormat>) -> String {
    match fmt {
        Some(ImageFormat::Jpeg) => "image/jpeg",
        Some(ImageFormat::Gif) => "image/gif",
        Some(ImageFormat::WebP) => "image/webp",
        _ => "image/png",
    }
    .to_string()
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn encode_jpeg(img: &DynamicImage, quality: u8) -> Option<Vec<u8>> {
    let mut buf = Vec::new();
    JpegEncoder::new_with_quality(&mut buf, quality)
        .encode_image(img)
        .ok()?;
    Some(buf)
}

/// Decode, then resize/re-encode to fit the size + dimension budget. Ports the
/// core of claude-code `maybeResizeAndDownsampleImageBuffer`: empty-guard →
/// fast-path-unchanged → dimension-resize(2000) → JPEG quality ladder → 1000px
/// secondary shrink. (Functional parity: oversized PNG/WebP go to the JPEG
/// ladder rather than sharp's PNG-palette / WebP-quality paths.)
///
/// # Errors
/// Empty input, or the bytes can't be decoded as a supported image.
pub fn process_image(bytes: Vec<u8>) -> Result<ProcessedImage, String> {
    if bytes.is_empty() {
        return Err("Image file is empty (0 bytes)".to_string());
    }
    let fmt = image::guess_format(&bytes).ok();
    let media_type = format_to_media_type(fmt);
    let img =
        image::load_from_memory(&bytes).map_err(|e| format!("failed to decode image: {e}"))?;
    let (w, h) = img.dimensions();

    if bytes.len() <= IMAGE_TARGET_RAW_SIZE && w <= IMAGE_MAX_DIM && h <= IMAGE_MAX_DIM {
        return Ok(ProcessedImage {
            base64: b64(&bytes),
            media_type,
            resized: None,
        });
    }

    let working = if w > IMAGE_MAX_DIM || h > IMAGE_MAX_DIM {
        img.resize(
            IMAGE_MAX_DIM,
            IMAGE_MAX_DIM,
            image::imageops::FilterType::Triangle,
        )
    } else {
        img
    };
    let (dw, dh) = working.dimensions();

    for &q in &JPEG_QUALITY_LADDER {
        if let Some(enc) = encode_jpeg(&working, q) {
            if enc.len() <= IMAGE_TARGET_RAW_SIZE {
                return Ok(ProcessedImage {
                    base64: b64(&enc),
                    media_type: "image/jpeg".to_string(),
                    // `resized` (and thus the coordinate-mapping metadata message)
                    // only when dimensions actually changed — claude-code's
                    // `wasResized` (a >budget re-encode at full resolution is NOT
                    // a resize). A pure format change emits no metadata.
                    resized: (dw != w || dh != h).then_some((w, h, dw, dh)),
                });
            }
        }
    }

    let sw = working.width().min(SECONDARY_SHRINK_WIDTH);
    let small = working.resize(sw, u32::MAX, image::imageops::FilterType::Triangle);
    let (sdw, sdh) = small.dimensions();
    let enc = encode_jpeg(&small, 20).ok_or_else(|| "jpeg encode failed".to_string())?;
    Ok(ProcessedImage {
        base64: b64(&enc),
        media_type: "image/jpeg".to_string(),
        resized: (sdw != w || sdh != h).then_some((w, h, sdw, sdh)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, RgbImage};

    fn png_bytes(w: u32, h: u32) -> Vec<u8> {
        let img = DynamicImage::ImageRgb8(RgbImage::new(w, h));
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
        buf.into_inner()
    }

    #[test]
    fn detects_image_extensions() {
        for ok in ["a.png", "a.jpg", "a.JPEG", "a.gif", "a.webp"] {
            assert!(is_image_extension(std::path::Path::new(ok)), "{ok}");
        }
        for no in ["a.txt", "a.rs", "a", "a.tar.gz"] {
            assert!(!is_image_extension(std::path::Path::new(no)), "{no}");
        }
    }

    #[test]
    fn small_image_passes_through_unchanged() {
        use base64::Engine;
        let bytes = png_bytes(10, 10);
        let p = process_image(bytes.clone()).unwrap();
        assert_eq!(p.media_type, "image/png");
        assert!(p.resized.is_none(), "small image not resized");
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(&p.base64)
                .unwrap(),
            bytes
        );
    }

    #[test]
    fn oversized_dimensions_are_resized_to_cap() {
        let bytes = png_bytes(3000, 1500);
        let p = process_image(bytes).unwrap();
        let (ow, oh, dw, dh) = p.resized.expect("resized");
        assert_eq!((ow, oh), (3000, 1500));
        assert!(
            dw <= IMAGE_MAX_DIM && dh <= IMAGE_MAX_DIM,
            "fits cap: {dw}x{dh}"
        );
        assert_eq!(dw, 2000, "long side clamped to 2000");
        assert_eq!(
            p.media_type, "image/jpeg",
            "resized images re-encode as jpeg"
        );
    }

    #[test]
    fn empty_image_errors() {
        assert!(process_image(vec![]).is_err());
    }
}
