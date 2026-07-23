//! FileRead image-reading shim over the shared image budget processor.
//!
//! FileRead keeps the extension gate locally, but the actual decode/resize
//! logic now lives in `tool-api` so other tool crates can reuse it without a
//! forbidden tool-to-tool dependency edge.

pub use tool_api::util::image_budget::{
    process_image, process_image_with_base64_budget, ProcessedImage, IMAGE_MAX_DIM,
};

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
        assert!(dw <= IMAGE_MAX_DIM && dh <= IMAGE_MAX_DIM);
        assert_eq!(dw, 2000);
        assert_eq!(p.media_type, "image/jpeg");
    }

    #[test]
    fn base64_budget_is_enforced() {
        let p = process_image_with_base64_budget(png_bytes(3000, 1), 4_000).unwrap();
        assert!(p.base64.len() <= 4_000);
    }
}
