//! Real inline-image rendering via `ratatui-image`.
//!
//! Loads an image file into a resize protocol matched to the terminal's
//! graphics capability (kitty / iTerm2 / sixel), which the app renders into a
//! preview pane with `StatefulImage`. This path is terminal-dependent and not
//! unit-testable (it needs a live graphics terminal); when the terminal has no
//! support or the file can't be decoded it degrades to the text placeholder,
//! leaving the non-image UI unaffected.

use std::path::Path;

use ratatui_image::picker::Picker;
use ratatui_image::protocol::StatefulProtocol;

/// Build a picker: query the terminal for graphics support + font size, falling
/// back to a fixed 8×16 cell size on a non-tty / query failure (so it never
/// panics in CI or a dumb terminal — the resulting picker just yields no
/// graphics protocol).
#[must_use]
pub fn make_picker() -> Picker {
    Picker::from_query_stdio().unwrap_or_else(|_| Picker::halfblocks())
}

/// Decode the image at `path` into a resize protocol via `picker`. Returns
/// `None` on a read/decode error (missing file, unsupported format — only
/// png+jpeg are compiled in).
#[must_use]
pub fn load_protocol(picker: &Picker, path: &Path) -> Option<StatefulProtocol> {
    let img = image::ImageReader::open(path).ok()?.decode().ok()?;
    Some(picker.new_resize_protocol(img))
}

/// Pixel `(width, height)` of the image at `path`, from a header-only read.
/// `None` when the file is missing or not a decodable image — this doubles as
/// the "is this really an image?" gate before emitting inline-image escapes.
#[must_use]
pub fn image_pixel_size(path: &Path) -> Option<(u32, u32)> {
    image::image_dimensions(path).ok()
}
