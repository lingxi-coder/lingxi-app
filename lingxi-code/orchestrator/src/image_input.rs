//! Load a pasted image file into a canonical [`protocol::ImageSource::Base64`].
//!
//! The TUI records pasted image *paths*; the orchestrator reads + base64-encodes
//! them here, just before building the outgoing user message, so a read failure
//! surfaces as a turn error (and the TUI never blocks on disk I/O during paste).

use crate::error::OrchestratorError;
use protocol::ImageSource;
use std::path::Path;

/// Detect the image MIME type from a file extension. `None` for unsupported
/// extensions — the caller fails the turn rather than guessing a type.
fn media_type_for(path: &Path) -> Option<&'static str> {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => Some("image/png"),
        Some("jpg" | "jpeg") => Some("image/jpeg"),
        Some("gif") => Some("image/gif"),
        Some("webp") => Some("image/webp"),
        _ => None,
    }
}

/// Standard base64 (RFC 4648) encoder with padding. No `data:` prefix.
fn base64_encode(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0];
        let b1 = *chunk.get(1).unwrap_or(&0);
        let b2 = *chunk.get(2).unwrap_or(&0);
        out.push(T[(b0 >> 2) as usize] as char);
        out.push(T[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize] as char);
        out.push(if chunk.len() > 1 {
            T[(((b1 & 0x0f) << 2) | (b2 >> 6)) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[(b2 & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// Read an image file from disk and build a base64 [`ImageSource`].
///
/// # Errors
/// Returns [`OrchestratorError::Internal`] if the extension is unsupported or
/// the file cannot be read.
pub fn load_image_source(path: &Path) -> Result<ImageSource, OrchestratorError> {
    let media_type = media_type_for(path).ok_or_else(|| {
        OrchestratorError::Internal(format!(
            "unsupported image type for {} (expected png/jpg/jpeg/gif/webp)",
            path.display()
        ))
    })?;
    let bytes = std::fs::read(path).map_err(|e| {
        OrchestratorError::Internal(format!("could not read image {}: {e}", path.display()))
    })?;
    Ok(ImageSource::Base64 {
        media_type: media_type.to_string(),
        data: base64_encode(&bytes),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn base64_encodes_known_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"hello world!"), "aGVsbG8gd29ybGQh");
    }

    #[test]
    fn media_type_detection() {
        assert_eq!(media_type_for(Path::new("a.PNG")), Some("image/png"));
        assert_eq!(media_type_for(Path::new("a.jpeg")), Some("image/jpeg"));
        assert_eq!(media_type_for(Path::new("a.webp")), Some("image/webp"));
        assert_eq!(media_type_for(Path::new("a.txt")), None);
        assert_eq!(media_type_for(Path::new("noext")), None);
    }

    #[test]
    fn loads_png_file_as_base64_source() {
        let dir = std::env::temp_dir();
        let p = dir.join("lingxi_test_img.png");
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(b"\x89PNG\r\n\x1a\n test bytes").unwrap();
        let src = load_image_source(&p).unwrap();
        match src {
            ImageSource::Base64 { media_type, data } => {
                assert_eq!(media_type, "image/png");
                assert!(!data.is_empty());
            }
            ImageSource::Url { .. } => panic!("expected base64"),
        }
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn unsupported_extension_errors() {
        assert!(load_image_source(Path::new("/tmp/whatever.bmp")).is_err());
    }
}
