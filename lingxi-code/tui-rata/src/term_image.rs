//! Terminal inline-image capability detection.
//!
//! Reports which inline-image protocol the host terminal supports (kitty
//! graphics, iTerm2 inline images, or sixel), detected from environment
//! variables. This is the FOUNDATION for real inline image display.
//!
//! DATA-MODEL BLOCKER (documented, intentional): the neutral
//! `tui_core::message::RenderedMessage::UserImage` variant carries only an
//! `image_id` + optional `metadata` — NOT the pixel bytes. Real inline
//! rendering therefore needs an engine-side change to thread the image data
//! through to the render model; until then `tui-rata` renders the aligned
//! `[Image #N]` placeholder and this module only reports the terminal's
//! capability (so a future data feed can pick the right protocol).

/// An inline-image protocol a terminal may support.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageProtocol {
    /// The kitty graphics protocol (kitty, WezTerm, Konsole).
    Kitty,
    /// iTerm2's inline-image escape (iTerm2, WezTerm).
    ITerm2,
    /// DEC sixel graphics.
    Sixel,
    /// No known inline-image support — use the text placeholder.
    None,
}

impl ImageProtocol {
    /// Whether the terminal can display inline images at all.
    #[must_use]
    pub fn supports_inline_images(self) -> bool {
        self != Self::None
    }

    /// A short human label (for a capability note / `/doctor`).
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Kitty => "kitty graphics",
            Self::ITerm2 => "iTerm2 inline images",
            Self::Sixel => "sixel",
            Self::None => "none",
        }
    }
}

/// Detect the inline-image protocol from the live environment (`$TERM`,
/// `$TERM_PROGRAM`, `$KITTY_WINDOW_ID`).
#[must_use]
pub fn detect() -> ImageProtocol {
    detect_from(
        std::env::var("TERM").ok().as_deref(),
        std::env::var("TERM_PROGRAM").ok().as_deref(),
        std::env::var("KITTY_WINDOW_ID").ok().as_deref(),
    )
}

/// Pure detection over the raw env values (testable without touching process
/// env). Kitty is preferred over iTerm2 over sixel when several match.
#[must_use]
pub fn detect_from(
    term: Option<&str>,
    term_program: Option<&str>,
    kitty_window_id: Option<&str>,
) -> ImageProtocol {
    let term = term.unwrap_or("").to_ascii_lowercase();
    let program = term_program.unwrap_or("").to_ascii_lowercase();

    // kitty: sets KITTY_WINDOW_ID, or TERM=xterm-kitty; WezTerm/Konsole also
    // speak the kitty protocol.
    if kitty_window_id.is_some_and(|v| !v.is_empty())
        || term.contains("kitty")
        || program == "wezterm"
    {
        return ImageProtocol::Kitty;
    }
    if program == "iterm.app" {
        return ImageProtocol::ITerm2;
    }
    // A few terminals advertise sixel in $TERM.
    if term.contains("sixel") || term.contains("mlterm") || term == "yaft-256color" {
        return ImageProtocol::Sixel;
    }
    ImageProtocol::None
}

/// Standard base64 alphabet (RFC 4648).
const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Encode bytes as standard base64 (`=`-padded). Self-contained so `tui-rata`
/// needs no base64 dependency.
#[must_use]
pub fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        let n = (u32::from(b0) << 16) | (u32::from(b1) << 8) | u32::from(b2);
        out.push(B64[((n >> 18) & 63) as usize] as char);
        out.push(B64[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            B64[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            B64[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// Encode `bytes` (a PNG/JPEG/… file) as an iTerm2 inline-image escape
/// (`ESC ] 1337 ; File=inline=1;size=N : <base64> BEL`). iTerm2 / WezTerm
/// render it at the cursor.
#[must_use]
pub fn encode_iterm2(bytes: &[u8]) -> String {
    let b64 = base64_encode(bytes);
    format!("\x1b]1337;File=inline=1;size={}:{b64}\x07", bytes.len())
}

/// Max base64 chars per kitty transmission chunk (protocol limit is 4096).
const KITTY_CHUNK: usize = 4096;

/// Encode `bytes` as a kitty graphics-protocol transmit-and-display sequence
/// (`f=100` PNG), chunked at [`KITTY_CHUNK`] with the `m=` continuation flag.
#[must_use]
pub fn encode_kitty(bytes: &[u8]) -> String {
    let b64 = base64_encode(bytes);
    let raw = b64.as_bytes();
    if raw.is_empty() {
        return String::new();
    }
    let chunks: Vec<&[u8]> = raw.chunks(KITTY_CHUNK).collect();
    let mut out = String::new();
    for (i, chunk) in chunks.iter().enumerate() {
        let more = u8::from(i + 1 < chunks.len());
        let payload = std::str::from_utf8(chunk).unwrap_or("");
        if i == 0 {
            out.push_str(&format!("\x1b_Ga=T,f=100,m={more};{payload}\x1b\\"));
        } else {
            out.push_str(&format!("\x1b_Gm={more};{payload}\x1b\\"));
        }
    }
    out
}

/// Read the image file at `path` and encode it for `protocol`. Returns `None`
/// when the file can't be read or the protocol has no byte-stream encoder here
/// (sixel needs pixel rasterization, which is out of scope).
#[must_use]
pub fn render_inline_image(path: &std::path::Path, protocol: ImageProtocol) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    match protocol {
        ImageProtocol::ITerm2 => Some(encode_iterm2(&bytes)),
        ImageProtocol::Kitty => Some(encode_kitty(&bytes)),
        ImageProtocol::Sixel | ImageProtocol::None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kitty_detected_from_window_id_or_term() {
        assert_eq!(
            detect_from(Some("xterm-256color"), None, Some("1")),
            ImageProtocol::Kitty
        );
        assert_eq!(
            detect_from(Some("xterm-kitty"), None, None),
            ImageProtocol::Kitty
        );
        assert_eq!(
            detect_from(None, Some("WezTerm"), None),
            ImageProtocol::Kitty
        );
    }

    #[test]
    fn iterm2_detected_from_term_program() {
        assert_eq!(
            detect_from(Some("xterm-256color"), Some("iTerm.app"), None),
            ImageProtocol::ITerm2
        );
    }

    #[test]
    fn sixel_detected_from_term() {
        assert_eq!(
            detect_from(Some("mlterm"), None, None),
            ImageProtocol::Sixel
        );
        assert_eq!(
            detect_from(Some("xterm-sixel"), None, None),
            ImageProtocol::Sixel
        );
    }

    #[test]
    fn plain_terminal_has_no_protocol() {
        let p = detect_from(Some("xterm-256color"), Some("Apple_Terminal"), None);
        assert_eq!(p, ImageProtocol::None);
        assert!(!p.supports_inline_images());
        assert_eq!(p.label(), "none");
    }

    #[test]
    fn empty_kitty_window_id_is_not_kitty() {
        // An exported-but-empty KITTY_WINDOW_ID must not trigger detection.
        assert_eq!(
            detect_from(Some("xterm-256color"), None, Some("")),
            ImageProtocol::None
        );
    }

    #[test]
    fn base64_matches_rfc_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn iterm2_escape_wraps_base64_with_size() {
        let esc = encode_iterm2(b"foo");
        assert!(esc.starts_with("\x1b]1337;File=inline=1;size=3:"));
        assert!(esc.ends_with('\x07'));
        assert!(esc.contains("Zm9v"));
    }

    #[test]
    fn kitty_escape_is_chunked_with_continuation_flag() {
        // A payload larger than one chunk splits into m=1 … m=0 segments.
        let big = vec![b'A'; KITTY_CHUNK * 2]; // base64 grows this past 2 chunks
        let esc = encode_kitty(&big);
        assert!(esc.starts_with("\x1b_Ga=T,f=100,m=1;"));
        assert!(esc.contains("\x1b_Gm=1;")); // a middle continuation chunk
        assert!(esc.ends_with("\x1b\\"));
        assert!(esc.contains("m=0;")); // the final chunk clears the flag
                                       // A small payload is a single m=0 chunk.
        let small = encode_kitty(b"hi");
        assert!(small.starts_with("\x1b_Ga=T,f=100,m=0;"));
    }

    #[test]
    fn render_inline_image_reads_file_and_dispatches_by_protocol() {
        let path = std::env::temp_dir().join(format!("lingxi-img-test-{}.bin", std::process::id()));
        std::fs::write(&path, b"foo").unwrap();
        let iterm = render_inline_image(&path, ImageProtocol::ITerm2).unwrap();
        assert!(iterm.contains("Zm9v"));
        assert!(render_inline_image(&path, ImageProtocol::None).is_none());
        let _ = std::fs::remove_file(&path);
        // A missing file yields None.
        assert!(render_inline_image(
            std::path::Path::new("/no/such/img.png"),
            ImageProtocol::Kitty
        )
        .is_none());
    }
}
