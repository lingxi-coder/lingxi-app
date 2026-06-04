//! Empty-message-text guard — strips prompt-XML scaffolding tags and detects
//! the `(no content)` placeholder so messages whose body is only stripped tags
//! (or the placeholder) render nothing, matching claude-code's suppression.
//!
//! claude-code reference (`src/utils/messages.ts:2753-2763`,
//! `src/constants/messages.ts:1`):
//! ```ts
//! export const NO_CONTENT_MESSAGE = '(no content)'
//!
//! export function isEmptyMessageText(text: string): boolean {
//!   return (
//!     stripPromptXMLTags(text).trim() === '' || text.trim() === NO_CONTENT_MESSAGE
//!   )
//! }
//! const STRIPPED_TAGS_RE =
//!   /<(commit_analysis|context|function_analysis|pr_analysis)>.*?<\/\1>\n?/gs
//!
//! export function stripPromptXMLTags(content: string): string {
//!   return content.replace(STRIPPED_TAGS_RE, '').trim()
//! }
//! ```
//!
//! 1:1 fidelity: the TS regex uses a `\1` backreference, which Rust's `regex`
//! crate cannot express. We port it as a hand-written scanner over the four
//! explicit tag names. The scanner mirrors the regex byte-for-byte:
//!   - it walks the string left→right (the `g` flag),
//!   - for an opening `<name>` it finds the NEAREST following `</name>` of the
//!     SAME name (lazy `.*?` + the `\1` backreference; `.` is dotall under `s`,
//!     so the body may span newlines),
//!   - it then consumes a single optional trailing `\n` (the regex's `\n?`),
//!   - and continues scanning after the match.
//!
//! Any opening tag without a matching close is left untouched (the regex would
//! not match it either). The result is `trim()`-ed, exactly like
//! `stripPromptXMLTags`.

/// claude-code `NO_CONTENT_MESSAGE` (`src/constants/messages.ts:1`).
pub const NO_CONTENT_MESSAGE: &str = "(no content)";

/// The four prompt-scaffolding tag names stripped by claude-code's
/// `STRIPPED_TAGS_RE` (`src/utils/messages.ts:2758-2759`). Order matters only
/// for the leftmost-match scan below (claude-code's regex alternation tries
/// them in this order at each position, but since they all start with a
/// distinct second character the order is immaterial — kept identical anyway).
const STRIPPED_TAGS: [&str; 4] = ["commit_analysis", "context", "function_analysis", "pr_analysis"];

/// Port of claude-code `stripPromptXMLTags` (`src/utils/messages.ts:2761-2763`):
/// remove every `<tag>…</tag>` block (plus one optional trailing `\n`) for the
/// four scaffolding tags, then `trim()` the result.
#[must_use]
pub fn strip_prompt_xml_tags(content: &str) -> String {
    let bytes = content.as_bytes();
    let mut out = String::with_capacity(content.len());
    // Scan over byte offsets. The four tag names + `<`, `>`, `/`, `\n` are all
    // ASCII, so a `<` only ever marks the start of a candidate block at a UTF-8
    // boundary; non-ASCII body bytes never equal `b'<'`. We always advance by
    // whole UTF-8 chars when copying, so multi-byte sequences stay intact.
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'<' {
            if let Some(next) = try_match_tag_block(content, bytes, i) {
                // Matched a full `<name>…</name>\n?` block — drop it and resume
                // scanning immediately after.
                i = next;
                continue;
            }
        }
        // Not the start of a stripped block: copy the whole char through.
        // `content[i..]` is a valid str (i is on a boundary), so the next char
        // is well-defined and advances i by its full UTF-8 length.
        let ch = content[i..].chars().next().expect("i is a char boundary");
        out.push(ch);
        i += ch.len_utf8();
    }
    out.trim().to_string()
}

/// If a stripped `<name>…</name>` block (with an optional trailing `\n`) starts
/// at byte offset `start` (which must point at `<`), return the byte offset
/// just past the block. Otherwise return `None`.
fn try_match_tag_block(content: &str, bytes: &[u8], start: usize) -> Option<usize> {
    for name in STRIPPED_TAGS {
        // Opening tag: `<name>`.
        let open = format!("<{name}>");
        if !content[start..].starts_with(&open) {
            continue;
        }
        let body_start = start + open.len();
        // Lazy `.*?` + `\1`: the NEAREST following `</name>`.
        let close = format!("</{name}>");
        if let Some(rel) = content[body_start..].find(&close) {
            let mut end = body_start + rel + close.len();
            // `\n?` — consume one optional trailing newline.
            if end < bytes.len() && bytes[end] == b'\n' {
                end += 1;
            }
            return Some(end);
        }
        // Opening tag with no matching close: the regex wouldn't match either.
        // Stop trying other names (the prefixes are disjoint) and treat `<` as
        // literal.
        return None;
    }
    None
}

/// Port of claude-code `isEmptyMessageText` (`src/utils/messages.ts:2753-2757`):
/// a message body is "empty" when stripping the scaffolding tags leaves only
/// whitespace, OR when the trimmed body equals `(no content)`.
#[must_use]
pub fn is_empty_message_text(text: &str) -> bool {
    strip_prompt_xml_tags(text).is_empty() || text.trim() == NO_CONTENT_MESSAGE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_content_literal_is_empty() {
        assert!(is_empty_message_text("(no content)"));
        // Trimmed, per `text.trim() === NO_CONTENT_MESSAGE`.
        assert!(is_empty_message_text("  (no content)  \n"));
    }

    #[test]
    fn plain_text_is_not_empty() {
        assert!(!is_empty_message_text("hello world"));
        assert!(!is_empty_message_text("(no content) and more"));
    }

    #[test]
    fn strips_each_of_four_tags() {
        for name in ["commit_analysis", "context", "function_analysis", "pr_analysis"] {
            let body = format!("<{name}>scaffolding body</{name}>");
            assert_eq!(strip_prompt_xml_tags(&body), "", "tag {name} not stripped");
            assert!(is_empty_message_text(&body), "tag {name} not empty");
        }
    }

    #[test]
    fn dotall_matches_multiline_tag_body() {
        // `.` is dotall under the `s` flag — the body spans newlines.
        let body = "<commit_analysis>\nline one\nline two\n</commit_analysis>";
        assert_eq!(strip_prompt_xml_tags(body), "");
        assert!(is_empty_message_text(body));
    }

    #[test]
    fn mixed_tag_plus_text_keeps_text() {
        let body = "<context>scaffolding</context>\nreal answer";
        // The optional trailing `\n` after the close tag is consumed, leaving
        // "real answer" after the outer trim.
        assert_eq!(strip_prompt_xml_tags(body), "real answer");
        assert!(!is_empty_message_text(body));
    }

    #[test]
    fn consumes_single_optional_trailing_newline() {
        // `\n?` consumes exactly one newline; a second blank line survives
        // (then is trimmed only if it is leading/trailing whitespace).
        let body = "<context>x</context>\n\nkept";
        assert_eq!(strip_prompt_xml_tags(body), "kept");
    }

    #[test]
    fn strips_multiple_blocks() {
        let body = "<context>a</context>\n<commit_analysis>b</commit_analysis>\n";
        assert_eq!(strip_prompt_xml_tags(body), "");
        assert!(is_empty_message_text(body));
    }

    #[test]
    fn nearest_close_tag_pairs_lazily() {
        // Lazy `.*?` pairs the first `</context>` with the first `<context>`.
        // What remains is the text BETWEEN the first close and the second open
        // (plus the second block, which is then itself stripped).
        let body = "<context>one</context>middle<context>two</context>";
        assert_eq!(strip_prompt_xml_tags(body), "middle");
    }

    #[test]
    fn unmatched_open_tag_is_left_intact() {
        // No closing tag → the regex wouldn't match → `<context>` stays.
        let body = "<context>no close here";
        assert_eq!(strip_prompt_xml_tags(body), "<context>no close here");
        assert!(!is_empty_message_text(body));
    }

    #[test]
    fn non_stripped_tag_is_left_intact() {
        let body = "<other>keep me</other>";
        assert_eq!(strip_prompt_xml_tags(body), "<other>keep me</other>");
        assert!(!is_empty_message_text(body));
    }

    #[test]
    fn empty_string_is_empty() {
        assert!(is_empty_message_text(""));
        assert!(is_empty_message_text("   \n  "));
    }

    #[test]
    fn non_ascii_body_survives() {
        let body = "café — \u{1f600}";
        assert_eq!(strip_prompt_xml_tags(body), "café — \u{1f600}");
        assert!(!is_empty_message_text(body));
        // …and a stripped tag wrapping non-ASCII strips cleanly.
        let wrapped = "<context>café \u{1f600}</context>";
        assert_eq!(strip_prompt_xml_tags(wrapped), "");
    }
}
