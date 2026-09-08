//! `w1` — the display sanitizer claude-code runs over every identifier it
//! interpolates into a task error message.
//!
//! Verbatim from `src_157227596.js` @99533:
//!
//! ```js
//! function w1(e,t=160){
//!   let o=Rae(e)
//!     .replace(/[\p{Cc}\p{Cf}]/gu,(r)=>/\s/.test(r)?r:"")
//!     .replace(/\s+/g," ")
//!     .trim();
//!   return o.length>t?`${oe(o,t)}…`:o
//! }
//! ```
//!
//! It is not cosmetic. Task ids and agent names reach these messages from the
//! model and from other agents, so an unsanitized one can carry bidi overrides
//! or zero-width characters into the transcript.
//!
//! Three of the four helpers are reproduced here; the fourth is a no-op:
//!
//! - `Rae` (`src_156484250.js` @ the `Rae` definition) strips LONE SURROGATES.
//!   A Rust `&str` is always well-formed UTF-8 and cannot hold one, so it has
//!   nothing to do.
//! - `oe(t,n)` (`src_156484250.js` @954) slices to `n` UTF-16 code units and
//!   drops a trailing HIGH surrogate so a pair is never split. Counting
//!   `char::len_utf16` per char reproduces both halves at once: a `char` is
//!   whole by construction, so the boundary can never land inside a pair.
//! - The length test is `o.length > t` — JS string length, i.e. UTF-16 CODE
//!   UNITS, not bytes and not chars. An emoji counts 2.

/// `t = 160`, the default every call site in the task cluster uses.
pub const DISPLAY_LIMIT: usize = 160;

/// The JS `\s` class, which is what decides whether a `Cc`/`Cf` character
/// survives the strip. Note U+FEFF is in it (and is also `Cf`), so a BOM
/// survives the strip and is then collapsed into a plain space.
fn is_js_whitespace(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r' | ' ' | '\u{00A0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200A}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
                | '\u{FEFF}'
    )
}

/// Unicode general category `Cf` (format). `Cc` is covered by
/// [`char::is_control`], which is exactly U+0000–U+001F plus U+007F–U+009F.
///
/// Spelled out rather than pulled from a properties crate: this is the only
/// place in the workspace that needs the category, and the table is short
/// enough to read. Every range is from Unicode 15.
fn is_format_char(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{0600}'..='\u{0605}'
            | '\u{061C}'
            | '\u{06DD}'
            | '\u{070F}'
            | '\u{0890}'..='\u{0891}'
            | '\u{08E2}'
            | '\u{180E}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206F}'
            | '\u{FEFF}'
            | '\u{FFF9}'..='\u{FFFB}'
            | '\u{110BD}'
            | '\u{110CD}'
            | '\u{13430}'..='\u{1343F}'
            | '\u{1BCA0}'..='\u{1BCA3}'
            | '\u{1D173}'..='\u{1D17A}'
            | '\u{E0001}'
            | '\u{E0020}'..='\u{E007F}'
    )
}

/// `w1(e)` with the oracle's default limit.
#[must_use]
pub fn sanitize_display(value: &str) -> String {
    sanitize_display_to(value, DISPLAY_LIMIT)
}

/// `w1(e, t)`.
#[must_use]
pub fn sanitize_display_to(value: &str, limit: usize) -> String {
    // `.replace(/[\p{Cc}\p{Cf}]/gu, r => /\s/.test(r) ? r : "")` — a control or
    // format char survives ONLY when JS would call it whitespace.
    let stripped: String = value
        .chars()
        .filter(|c| !(c.is_control() || is_format_char(*c)) || is_js_whitespace(*c))
        .collect();

    // `.replace(/\s+/g, " ").trim()`.
    let mut collapsed = String::with_capacity(stripped.len());
    let mut in_space = false;
    for c in stripped.chars() {
        if is_js_whitespace(c) {
            in_space = true;
            continue;
        }
        if in_space && !collapsed.is_empty() {
            collapsed.push(' ');
        }
        in_space = false;
        collapsed.push(c);
    }

    // `o.length > t ? `${oe(o,t)}…` : o` — UTF-16 code units on both sides.
    let utf16_len: usize = collapsed.chars().map(char::len_utf16).sum();
    if utf16_len <= limit {
        return collapsed;
    }
    let mut out = String::new();
    let mut used = 0usize;
    for c in collapsed.chars() {
        let width = c.len_utf16();
        if used + width > limit {
            break;
        }
        used += width;
        out.push(c);
    }
    out.push('\u{2026}');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_text_is_untouched() {
        assert_eq!(sanitize_display("buddy@alpha"), "buddy@alpha");
        assert_eq!(sanitize_display(""), "");
    }

    /// The reason this exists: an id carrying a bidi override or a zero-width
    /// character must not reach the transcript intact.
    #[test]
    fn control_and_format_characters_are_stripped() {
        // U+202E RIGHT-TO-LEFT OVERRIDE (Cf, not JS whitespace).
        assert_eq!(sanitize_display("a\u{202E}b"), "ab");
        // U+200B ZERO WIDTH SPACE (Cf, NOT in the JS `\s` class).
        assert_eq!(sanitize_display("a\u{200B}b"), "ab");
        // A C0 control (Cc).
        assert_eq!(sanitize_display("a\u{0007}b"), "ab");
        // U+00AD SOFT HYPHEN (Cf).
        assert_eq!(sanitize_display("a\u{00AD}b"), "ab");
    }

    /// U+FEFF is BOTH `Cf` and JS whitespace, so the strip keeps it and the
    /// collapse turns it into a plain space. Getting the guard backwards would
    /// delete it instead.
    #[test]
    fn a_bom_becomes_a_space_rather_than_vanishing() {
        assert_eq!(sanitize_display("a\u{FEFF}b"), "a b");
    }

    #[test]
    fn whitespace_is_collapsed_and_trimmed() {
        assert_eq!(sanitize_display("  a \t\n  b  "), "a b");
        assert_eq!(sanitize_display("\u{3000}a\u{2028}b\u{00A0}"), "a b");
        assert_eq!(sanitize_display("   "), "");
    }

    /// The limit is UTF-16 code units, not chars and not bytes — an emoji
    /// counts 2. A char-counting port would let 160 emoji (320 code units)
    /// through untruncated.
    #[test]
    fn the_limit_counts_utf16_code_units() {
        let ascii = "a".repeat(160);
        assert_eq!(sanitize_display(&ascii), ascii, "exactly at the limit");

        let over = "a".repeat(161);
        let out = sanitize_display(&over);
        assert_eq!(out.chars().count(), 161, "160 kept plus the ellipsis");
        assert!(out.ends_with('\u{2026}'));

        // 81 astral chars = 162 UTF-16 units ⇒ over the limit.
        let astral = "😀".repeat(81);
        let out = sanitize_display(&astral);
        assert!(out.ends_with('\u{2026}'));
        let kept: usize = out
            .chars()
            .filter(|c| *c != '\u{2026}')
            .map(char::len_utf16)
            .sum();
        assert_eq!(kept, 160, "the cut lands on a whole char, never mid-pair");
    }

    /// `oe`'s high-surrogate guard: the truncation boundary must never split a
    /// pair, which for chars means the last kept char is whole.
    #[test]
    fn truncation_never_splits_an_astral_char() {
        // 159 ASCII + one astral char = 161 units; the astral char cannot fit
        // in the remaining single unit, so it is dropped whole.
        let value = format!("{}{}", "a".repeat(159), "😀");
        let out = sanitize_display(&value);
        assert_eq!(out, format!("{}\u{2026}", "a".repeat(159)));
    }
}
