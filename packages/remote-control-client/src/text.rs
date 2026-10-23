//! Text from another device or from the desktop, made safe to show.

fn is_bidi_or_invisible(c: char) -> bool {
    matches!(
        c,
        '\u{061C}' | '\u{200B}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'
            | '\u{2066}'..='\u{2069}' | '\u{FEFF}'
    )
}

fn is_control(c: char) -> bool {
    c <= '\u{1F}' || ('\u{7F}'..='\u{9F}').contains(&c) || c == '\u{2028}' || c == '\u{2029}'
}

/// A name or label that came from another device or from the desktop, made safe
/// to show on one line: direction controls and invisible characters removed,
/// control characters and line breaks turned into spaces, runs of spaces folded,
/// and the result cut to `max_chars` characters (by code point, never mid-pair).
pub fn display_text(value: &str, max_chars: usize) -> String {
    let cleaned: String = value
        .chars()
        .filter(|c| !is_bidi_or_invisible(*c))
        .map(|c| if is_control(c) { ' ' } else { c })
        .collect();
    let clean = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let count = clean.chars().count();
    if count <= max_chars {
        return clean;
    }
    let mut cut: String = clean.chars().take(max_chars.saturating_sub(1)).collect();
    cut.truncate(cut.trim_end().len());
    cut.push('…');
    cut
}

/// Group a pairing code in threes so two people can compare it by eye: `"123456"` becomes `"123 456"`.
pub fn format_pairing_code(code: &str) -> String {
    if code.len() == 6 && code.bytes().all(|b| b.is_ascii_digit()) {
        format!("{} {}", &code[..3], &code[3..])
    } else {
        code.to_owned()
    }
}

/// The longest device name the viewer sends. The offer record allows 128, the plan asks for 64.
pub const DEVICE_NAME_MAX_CHARS: usize = 64;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_and_cuts() {
        assert_eq!(display_text("  a\u{202E}b\n\tc  ", 80), "ab c");
        assert_eq!(display_text("abcdef", 4), "abc…");
        assert_eq!(display_text("ab cdef", 4), "ab…");
        assert_eq!(display_text("小明的 iPhone 📱", 80), "小明的 iPhone 📱");
        assert_eq!(display_text("\u{200B}", 5), "");
        assert_eq!(format_pairing_code("123456"), "123 456");
        assert_eq!(format_pairing_code("12"), "12");
    }
}
