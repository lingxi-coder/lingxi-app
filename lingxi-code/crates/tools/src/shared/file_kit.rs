//! Binary detection, BOM stripping, UTF-8 decoding.
//!
//! Used by every M4 file-touching tool. Pure functional; no I/O.

/// Window size for binary detection — spec §7 lock.
///
/// Matches claude-code's `src/tools/FileReadTool/utils.ts` first-pass
/// NUL-byte scan. A file whose first 8 KB contain any `0x00` byte is
/// classified as binary and rejected.
pub const NUL_SCAN_WINDOW: usize = 8 * 1024;

/// True iff `prefix` (capped at [`NUL_SCAN_WINDOW`]) contains a NUL byte.
#[must_use]
pub fn looks_binary(prefix: &[u8]) -> bool {
    let window = &prefix[..prefix.len().min(NUL_SCAN_WINDOW)];
    window.iter().any(|b| *b == 0)
}

/// Return `bytes` with the UTF-8 BOM (`EF BB BF`) stripped if present.
#[must_use]
pub fn strip_utf8_bom(bytes: &[u8]) -> &[u8] {
    if bytes.starts_with(b"\xEF\xBB\xBF") {
        &bytes[3..]
    } else {
        bytes
    }
}

/// Decode `bytes` as UTF-8, stripping the BOM if present. Rejects non-UTF-8
/// (no fallback to latin-1 / cp1252 in M4-01 per spec §7 "Default encoding:
/// UTF-8 (BOM-aware); reject non-UTF-8").
///
/// # Errors
/// Returns `Utf8Error` if the bytes are not valid UTF-8.
pub fn decode_utf8_strict(bytes: &[u8]) -> Result<String, std::str::Utf8Error> {
    let trimmed = strip_utf8_bom(bytes);
    std::str::from_utf8(trimmed).map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nul_scan_window_is_8kb() {
        assert_eq!(NUL_SCAN_WINDOW, 8 * 1024);
    }

    #[test]
    fn looks_binary_finds_nul_in_window() {
        let mut buf = vec![b'A'; NUL_SCAN_WINDOW];
        buf[100] = 0;
        assert!(looks_binary(&buf));
    }

    #[test]
    fn looks_binary_ignores_nul_outside_window() {
        // Deviation from plan: plan wrote `vec![b'A'; NUL_SCAN_WINDOW + 10]`
        // and then `buf[9000] = 0`, but 9000 > 8202 → OOB panic. Resize
        // buffer so the NUL at offset 9000 is in-bounds but past the
        // 8192-byte scan window.
        let mut buf = vec![b'A'; NUL_SCAN_WINDOW + 1000];
        // NUL at position 9000 (past the 8192-byte window) → not binary
        buf[9000] = 0;
        assert!(!looks_binary(&buf));
    }

    #[test]
    fn looks_binary_empty_input_is_text() {
        assert!(!looks_binary(b""));
    }

    #[test]
    fn looks_binary_pure_text_is_text() {
        assert!(!looks_binary(b"hello world\nline 2\n"));
    }

    #[test]
    fn strip_utf8_bom_removes_bom() {
        let with_bom = b"\xEF\xBB\xBFhello";
        assert_eq!(strip_utf8_bom(with_bom), b"hello");
    }

    #[test]
    fn strip_utf8_bom_preserves_non_bom() {
        let no_bom = b"hello";
        assert_eq!(strip_utf8_bom(no_bom), b"hello");
    }

    #[test]
    fn decode_utf8_strict_handles_bom() {
        let s = decode_utf8_strict(b"\xEF\xBB\xBFhello \xE4\xB8\x96\xE7\x95\x8C").unwrap();
        assert_eq!(s, "hello 世界");
    }

    #[test]
    fn decode_utf8_strict_rejects_invalid() {
        // 0xFF is never a valid UTF-8 starter
        assert!(decode_utf8_strict(&[0xFF, 0xFE, b'a']).is_err());
    }
}
