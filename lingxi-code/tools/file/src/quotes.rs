//! Curly-quote normalization for Edit — port of
//! `FileEditTool/utils.ts:21-37` (curly constants + `normalizeQuotes`),
//! `:73-93` (`findActualString`), `:104-199`
//! (`preserveQuoteStyle` / `isOpeningContext` / `applyCurlyDoubleQuotes` /
//! `applyCurlySingleQuotes`, including the contraction rule).
//!
//! Claude cannot emit curly quotes (they are sanitized out of the API), so the
//! model always sends straight quotes. When a file contains typographic
//! (curly) quotes, an exact `old_string` match fails. [`find_actual_string`]
//! recovers the real curly text from the file so the edit still locates its
//! target, and [`preserve_quote_style`] re-applies the file's curly typography
//! to `new_string` so the rewrite does not silently strip curly quotes.
//!
//! ## 1:1 fidelity / divergence note
//!
//! Byte-faithful for BMP text and the contraction heuristic. The one **close**
//! (not byte-faithful) edge: TS slices the recovered substring with
//! `fileContent.substring(idx, idx + searchString.length)` where JS `.length`
//! is a count of **UTF-16 code units**. Rust here uses **char** counts. For
//! astral-plane chars (emoji, etc.) in `old_string`, a single char is one Rust
//! char but two UTF-16 code units, so the recovered window length differs from
//! TS. This only affects `old_string`s containing astral chars whose match
//! relied on quote normalization — pure-BMP text (the overwhelmingly common
//! case, and all curly-quote text) is byte-faithful.

/// Left single curly quote (U+2018). Mirrors `LEFT_SINGLE_CURLY_QUOTE`.
pub const LEFT_SINGLE_CURLY_QUOTE: char = '\u{2018}';
/// Right single curly quote (U+2019). Mirrors `RIGHT_SINGLE_CURLY_QUOTE`.
pub const RIGHT_SINGLE_CURLY_QUOTE: char = '\u{2019}';
/// Left double curly quote (U+201C). Mirrors `LEFT_DOUBLE_CURLY_QUOTE`.
pub const LEFT_DOUBLE_CURLY_QUOTE: char = '\u{201C}';
/// Right double curly quote (U+201D). Mirrors `RIGHT_DOUBLE_CURLY_QUOTE`.
pub const RIGHT_DOUBLE_CURLY_QUOTE: char = '\u{201D}';

/// Normalize quotes in a string by converting curly quotes to straight quotes.
///
/// Port of `normalizeQuotes` (`utils.ts:31-37`).
#[must_use]
pub fn normalize_quotes(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            LEFT_SINGLE_CURLY_QUOTE | RIGHT_SINGLE_CURLY_QUOTE => '\'',
            LEFT_DOUBLE_CURLY_QUOTE | RIGHT_DOUBLE_CURLY_QUOTE => '"',
            other => other,
        })
        .collect()
}

/// Find the actual string in `file` that matches `search`, accounting for quote
/// normalization. Port of `findActualString` (`utils.ts:73-93`).
///
/// First tries an exact match. Otherwise it normalizes curly quotes in both the
/// file and the search string, finds the search in the normalized file, then
/// slices the ORIGINAL file by the same char window to recover the real curly
/// text. Returns `None` when nothing matches.
///
/// Uses char iteration (not bytes). See the module-doc divergence note for the
/// astral-plane edge versus TS UTF-16 `.length`.
#[must_use]
pub fn find_actual_string(file: &str, search: &str) -> Option<String> {
    // First try exact match.
    if file.contains(search) {
        return Some(search.to_string());
    }

    // Try with normalized quotes.
    let normalized_search = normalize_quotes(search);
    let normalized_file = normalize_quotes(file);

    // `normalizeQuotes` maps each curly quote to exactly one straight quote, so
    // char indices in the normalized file align 1:1 with the original file's
    // char indices. Find the search by char index in the normalized file, then
    // slice the original file's chars by the same window.
    let normalized_file_chars: Vec<char> = normalized_file.chars().collect();
    let normalized_search_chars: Vec<char> = normalized_search.chars().collect();
    let search_char_index = char_index_of(&normalized_file_chars, &normalized_search_chars)?;

    // TS: fileContent.substring(searchIndex, searchIndex + searchString.length).
    // We use the ORIGINAL search's char length (JS `.length` is UTF-16 units;
    // see module divergence note — BMP-faithful).
    let window_len = search.chars().count();
    let original_chars: Vec<char> = file.chars().collect();
    let end = (search_char_index + window_len).min(original_chars.len());
    Some(original_chars[search_char_index..end].iter().collect())
}

/// Index of the first occurrence of `needle` within `haystack` (char slices),
/// or `None`. Mirrors JS `String.prototype.indexOf` over normalized content.
fn char_index_of(haystack: &[char], needle: &[char]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    if needle.len() > haystack.len() {
        return None;
    }
    (0..=haystack.len() - needle.len()).find(|&start| haystack[start..start + needle.len()] == *needle)
}

/// When `old_string` matched via quote normalization (curly quotes in file,
/// straight quotes from model), apply the same curly quote style to `new_string`
/// so the edit preserves the file's typography. Port of `preserveQuoteStyle`
/// (`utils.ts:104-136`).
#[must_use]
pub fn preserve_quote_style(old_string: &str, actual_old_string: &str, new_string: &str) -> String {
    // If they're the same, no normalization happened.
    if old_string == actual_old_string {
        return new_string.to_string();
    }

    // Detect which curly quote types were in the file.
    let has_double_quotes = actual_old_string.contains(LEFT_DOUBLE_CURLY_QUOTE)
        || actual_old_string.contains(RIGHT_DOUBLE_CURLY_QUOTE);
    let has_single_quotes = actual_old_string.contains(LEFT_SINGLE_CURLY_QUOTE)
        || actual_old_string.contains(RIGHT_SINGLE_CURLY_QUOTE);

    if !has_double_quotes && !has_single_quotes {
        return new_string.to_string();
    }

    let mut result = new_string.to_string();

    if has_double_quotes {
        result = apply_curly_double_quotes(&result);
    }
    if has_single_quotes {
        result = apply_curly_single_quotes(&result);
    }

    result
}

/// A quote character preceded by whitespace, start of string, or opening
/// punctuation is treated as an opening quote. Port of `isOpeningContext`
/// (`utils.ts:138-154`).
fn is_opening_context(chars: &[char], index: usize) -> bool {
    if index == 0 {
        return true;
    }
    let prev = chars[index - 1];
    matches!(
        prev,
        ' ' | '\t'
            | '\n'
            | '\r'
            | '('
            | '['
            | '{'
            | '\u{2014}' // em dash
            | '\u{2013}' // en dash
    )
}

/// Replace straight double quotes with open/close curly doubles. Port of
/// `applyCurlyDoubleQuotes` (`utils.ts:156-171`).
fn apply_curly_double_quotes(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut result = String::with_capacity(s.len());
    for (i, &c) in chars.iter().enumerate() {
        if c == '"' {
            result.push(if is_opening_context(&chars, i) {
                LEFT_DOUBLE_CURLY_QUOTE
            } else {
                RIGHT_DOUBLE_CURLY_QUOTE
            });
        } else {
            result.push(c);
        }
    }
    result
}

/// Replace straight single quotes with open/close curly singles, treating an
/// apostrophe between two letters as a contraction (→ right single curly). Port
/// of `applyCurlySingleQuotes` (`utils.ts:173-199`).
fn apply_curly_single_quotes(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut result = String::with_capacity(s.len());
    for i in 0..chars.len() {
        if chars[i] == '\'' {
            // Don't convert apostrophes in contractions (e.g., "don't", "it's").
            // An apostrophe between two letters is a contraction, not a quote.
            let prev_is_letter = i > 0 && chars[i - 1].is_alphabetic();
            let next_is_letter = i + 1 < chars.len() && chars[i + 1].is_alphabetic();
            if prev_is_letter && next_is_letter {
                // Apostrophe in a contraction — use right single curly quote.
                result.push(RIGHT_SINGLE_CURLY_QUOTE);
            } else {
                result.push(if is_opening_context(&chars, i) {
                    LEFT_SINGLE_CURLY_QUOTE
                } else {
                    RIGHT_SINGLE_CURLY_QUOTE
                });
            }
        } else {
            result.push(chars[i]);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_quotes_maps_curly_to_straight() {
        let input = format!(
            "{LEFT_DOUBLE_CURLY_QUOTE}hi{RIGHT_DOUBLE_CURLY_QUOTE} {LEFT_SINGLE_CURLY_QUOTE}yo{RIGHT_SINGLE_CURLY_QUOTE}"
        );
        assert_eq!(normalize_quotes(&input), "\"hi\" 'yo'");
    }

    #[test]
    fn normalize_quotes_leaves_straight_untouched() {
        assert_eq!(normalize_quotes("\"hi\" 'yo'"), "\"hi\" 'yo'");
    }

    #[test]
    fn find_actual_string_exact_match() {
        assert_eq!(
            find_actual_string("hello world", "world"),
            Some("world".to_string())
        );
    }

    #[test]
    fn find_actual_string_recovers_curly_double() {
        // File has curly double quotes, model sent straight double quotes.
        let file = format!("say {LEFT_DOUBLE_CURLY_QUOTE}hello{RIGHT_DOUBLE_CURLY_QUOTE} now");
        let actual = find_actual_string(&file, "\"hello\"").unwrap();
        assert_eq!(
            actual,
            format!("{LEFT_DOUBLE_CURLY_QUOTE}hello{RIGHT_DOUBLE_CURLY_QUOTE}")
        );
    }

    #[test]
    fn find_actual_string_recovers_curly_single() {
        let file = format!("a {LEFT_SINGLE_CURLY_QUOTE}b{RIGHT_SINGLE_CURLY_QUOTE} c");
        let actual = find_actual_string(&file, "'b'").unwrap();
        assert_eq!(
            actual,
            format!("{LEFT_SINGLE_CURLY_QUOTE}b{RIGHT_SINGLE_CURLY_QUOTE}")
        );
    }

    #[test]
    fn find_actual_string_none_when_absent() {
        assert_eq!(find_actual_string("hello world", "absent"), None);
    }

    #[test]
    fn preserve_quote_style_noop_when_same() {
        // old == actual_old ⇒ no normalization happened ⇒ new untouched.
        assert_eq!(preserve_quote_style("foo", "foo", "\"bar\""), "\"bar\"");
    }

    #[test]
    fn preserve_quote_style_double_open_close_context() {
        // File used curly doubles, so new straight doubles become curly by
        // open/close context.
        let actual_old = format!("{LEFT_DOUBLE_CURLY_QUOTE}x{RIGHT_DOUBLE_CURLY_QUOTE}");
        let out = preserve_quote_style("\"x\"", &actual_old, "say \"hi\" ok");
        assert_eq!(
            out,
            format!("say {LEFT_DOUBLE_CURLY_QUOTE}hi{RIGHT_DOUBLE_CURLY_QUOTE} ok")
        );
    }

    #[test]
    fn preserve_quote_style_contraction_keeps_right_single() {
        // `don't` — apostrophe between two letters is a contraction, not an
        // opening quote ⇒ right single curly.
        let actual_old = format!("{LEFT_SINGLE_CURLY_QUOTE}x{RIGHT_SINGLE_CURLY_QUOTE}");
        let out = preserve_quote_style("'x'", &actual_old, "don't");
        assert_eq!(out, format!("don{RIGHT_SINGLE_CURLY_QUOTE}t"));
    }

    #[test]
    fn preserve_quote_style_single_open_vs_close_positions() {
        let actual_old = format!("{LEFT_SINGLE_CURLY_QUOTE}x{RIGHT_SINGLE_CURLY_QUOTE}");
        // Leading quote (start of string) ⇒ opening; trailing quote (after a
        // letter, no letter after) ⇒ closing.
        let out = preserve_quote_style("'x'", &actual_old, "'word'");
        assert_eq!(
            out,
            format!("{LEFT_SINGLE_CURLY_QUOTE}word{RIGHT_SINGLE_CURLY_QUOTE}")
        );
    }

    #[test]
    fn preserve_quote_style_no_curly_in_file_leaves_new_untouched() {
        // actual_old differs from old but contains no curly quotes ⇒ unchanged.
        let out = preserve_quote_style("abc", "abd", "\"new\"");
        assert_eq!(out, "\"new\"");
    }
}
