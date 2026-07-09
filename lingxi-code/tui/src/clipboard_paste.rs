//! Clipboard IMAGE paste (`Ctrl+V` / `Alt+V`) — codex-rs/tui `clipboard_paste`.
//!
//! Bracketed paste only delivers TEXT: a screenshot or a copied image never
//! reaches the composer through `Event::Paste`. This module reads the image
//! straight off the system clipboard (via `arboard`), re-encodes it as PNG
//! into a temp file, and hands the path to the existing image-message flow —
//! the same route a pasted image *path* already takes.
//!
//! Ported from codex's `clipboard_paste.rs` core: the arboard file-list +
//! image-data read and the temp-PNG write. The Windows-PowerShell/WSL
//! fallbacks are intentionally not ported (desktop macOS/Linux targets).

use std::path::PathBuf;

/// Why a clipboard image read failed (surfaced verbatim in the transcript).
#[derive(Debug, Clone)]
pub enum PasteImageError {
    /// The clipboard could not be opened at all.
    ClipboardUnavailable(String),
    /// The clipboard opened but holds no image data.
    NoImage(String),
    /// The clipboard image could not be re-encoded as PNG.
    EncodeFailed(String),
    /// The temp-file write failed.
    IoError(String),
}

impl std::fmt::Display for PasteImageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PasteImageError::ClipboardUnavailable(msg) => write!(f, "clipboard unavailable: {msg}"),
            PasteImageError::NoImage(msg) => write!(f, "no image on clipboard: {msg}"),
            PasteImageError::EncodeFailed(msg) => write!(f, "could not encode image: {msg}"),
            PasteImageError::IoError(msg) => write!(f, "io error: {msg}"),
        }
    }
}

impl std::error::Error for PasteImageError {}

/// Basic metadata about the pasted image (for tracing/labels).
#[derive(Debug, Clone)]
pub struct PastedImageInfo {
    /// Pixel width of the decoded image.
    pub width: u32,
    /// Pixel height of the decoded image.
    pub height: u32,
}

/// Capture an image from the system clipboard and encode it to PNG bytes.
///
/// Images arrive on the clipboard either as FILES (copying from Finder) or as
/// raw image DATA (copying from a browser / taking a screenshot). Accept
/// both, preferring files when present — codex `paste_image_as_png`.
pub fn paste_image_as_png() -> Result<(Vec<u8>, PastedImageInfo), PasteImageError> {
    let mut cb = arboard::Clipboard::new()
        .map_err(|e| PasteImageError::ClipboardUnavailable(e.to_string()))?;
    let files = cb
        .get()
        .file_list()
        .map_err(|e| PasteImageError::ClipboardUnavailable(e.to_string()));
    let dyn_img = if let Some(img) = files
        .unwrap_or_default()
        .into_iter()
        .find_map(|f| image::open(f).ok())
    {
        img
    } else {
        let img = cb
            .get_image()
            .map_err(|e| PasteImageError::NoImage(e.to_string()))?;
        let w = img.width as u32;
        let h = img.height as u32;
        let Some(rgba_img) = image::RgbaImage::from_raw(w, h, img.bytes.into_owned()) else {
            return Err(PasteImageError::EncodeFailed("invalid RGBA buffer".into()));
        };
        image::DynamicImage::ImageRgba8(rgba_img)
    };

    let mut png: Vec<u8> = Vec::new();
    let mut cursor = std::io::Cursor::new(&mut png);
    dyn_img
        .write_to(&mut cursor, image::ImageFormat::Png)
        .map_err(|e| PasteImageError::EncodeFailed(e.to_string()))?;

    Ok((
        png,
        PastedImageInfo {
            width: dyn_img.width(),
            height: dyn_img.height(),
        },
    ))
}

/// [`paste_image_as_png`] written to a persistent temp file: the path feeds
/// the same image-message flow a pasted image *path* takes — codex
/// `paste_image_to_temp_png`.
pub fn paste_image_to_temp_png() -> Result<(PathBuf, PastedImageInfo), PasteImageError> {
    let (png, info) = paste_image_as_png()?;
    let tmp = tempfile::Builder::new()
        .prefix("lingxi-clipboard-")
        .suffix(".png")
        .tempfile()
        .map_err(|e| PasteImageError::IoError(e.to_string()))?;
    std::fs::write(tmp.path(), &png).map_err(|e| PasteImageError::IoError(e.to_string()))?;
    // Persist the file (it must outlive this handle: it is read again when
    // the message is submitted to the model).
    let (_file, path) = tmp
        .keep()
        .map_err(|e| PasteImageError::IoError(e.error.to_string()))?;
    Ok((path, info))
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore = "needs a real GUI clipboard holding an image (run manually)"]
    fn clipboard_image_paste_live() {
        let (path, info) = super::paste_image_to_temp_png().expect("clipboard image");
        assert!(path.exists());
        assert!(info.width > 0 && info.height > 0);
        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes.starts_with(b"\x89PNG"), "temp file is a PNG");
        std::fs::remove_file(path).ok();
    }
}
