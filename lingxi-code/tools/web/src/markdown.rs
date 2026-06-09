//! HTML→markdown conversion + the secondary-model prompt for the WebFetch apply
//! step. Ports `claude-code/src/tools/WebFetchTool/{utils.ts,prompt.ts}`.

use crate::web_fetch::WEBFETCH_TRUNCATION_SUFFIX;

/// Markdown is truncated to this many bytes before the secondary model, to avoid
/// "Prompt is too long" errors. claude-code `utils.ts` `MAX_MARKDOWN_LENGTH`.
pub const MAX_MARKDOWN_LENGTH: usize = 100_000;

/// True when the `Content-Type` header denotes HTML (case-insensitive substring
/// match on `text/html`). Non-HTML bodies are used as-is (no conversion).
#[must_use]
pub fn is_html_content_type(content_type: &str) -> bool {
    content_type.to_ascii_lowercase().contains("text/html")
}

/// Truncate `markdown` to `MAX_MARKDOWN_LENGTH` bytes (char-boundary safe),
/// appending [`WEBFETCH_TRUNCATION_SUFFIX`] when truncated. Mirrors the
/// `markdownContent.length > MAX_MARKDOWN_LENGTH` slice in `utils.ts`.
#[must_use]
pub fn truncate_markdown(markdown: String) -> String {
    if markdown.len() <= MAX_MARKDOWN_LENGTH {
        return markdown;
    }
    let mut cut = MAX_MARKDOWN_LENGTH;
    while cut > 0 && !markdown.is_char_boundary(cut) {
        cut -= 1;
    }
    let mut out = markdown[..cut].to_string();
    out.push_str(WEBFETCH_TRUNCATION_SUFFIX);
    out
}

/// Convert HTML to markdown via `htmd` (the `turndown` analogue). On conversion
/// error, fall back to the original HTML. Only compiled under `web-markdown`.
#[cfg(feature = "web-markdown")]
#[must_use]
pub fn html_to_markdown(html: &str) -> String {
    htmd::convert(html).unwrap_or_else(|_| html.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_html_content_type() {
        assert!(is_html_content_type("text/html; charset=utf-8"));
        assert!(is_html_content_type("TEXT/HTML"));
        assert!(!is_html_content_type("text/markdown"));
        assert!(!is_html_content_type("application/json"));
        assert!(!is_html_content_type(""));
    }

    #[test]
    fn truncates_markdown_at_cap() {
        let big = "a".repeat(MAX_MARKDOWN_LENGTH + 10);
        let out = truncate_markdown(big);
        assert!(out.ends_with(crate::web_fetch::WEBFETCH_TRUNCATION_SUFFIX));
        let body = &out[..out.len() - crate::web_fetch::WEBFETCH_TRUNCATION_SUFFIX.len()];
        assert_eq!(body.len(), MAX_MARKDOWN_LENGTH);
    }

    #[test]
    fn does_not_truncate_short_markdown() {
        let s = "hello".to_string();
        assert_eq!(truncate_markdown(s.clone()), s);
    }
}
