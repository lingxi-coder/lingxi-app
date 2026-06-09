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

/// Guidelines appended for a NON-preapproved domain (the strict default).
/// Byte-faithful to `prompt.ts`.
const GUIDELINES_STRICT: &str = "Provide a concise response based only on the content above. In your response:\n \
- Enforce a strict 125-character maximum for quotes from any source document. \
Open Source Software is ok as long as we respect the license.\n \
- Use quotation marks for exact language from articles; any language outside of \
the quotation should never be word-for-word the same.\n \
- You are not a lawyer and never comment on the legality of your own prompts and \
responses.\n - Never produce or reproduce exact song lyrics.";

/// Guidelines appended for a preapproved domain. Byte-faithful to `prompt.ts`.
const GUIDELINES_PREAPPROVED: &str = "Provide a concise response based on the content above. Include relevant \
details, code examples, and documentation excerpts as needed.";

/// Build the secondary-model prompt. Byte-faithful to `prompt.ts`
/// `makeSecondaryModelPrompt` (note the leading/trailing newlines).
#[must_use]
pub fn make_secondary_model_prompt(
    markdown_content: &str,
    prompt: &str,
    is_preapproved_domain: bool,
) -> String {
    let guidelines = if is_preapproved_domain {
        GUIDELINES_PREAPPROVED
    } else {
        GUIDELINES_STRICT
    };
    format!("\nWeb page content:\n---\n{markdown_content}\n---\n\n{prompt}\n\n{guidelines}\n")
}

/// Whether `host` is on the WebFetch preapproved-domain allowlist. Conservative
/// default (`false` => strict guidelines); porting `preapproved.ts` is a follow-up.
#[must_use]
pub fn is_preapproved_domain(_host: &str) -> bool {
    false
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

    #[test]
    fn secondary_prompt_matches_template_strict() {
        let got = make_secondary_model_prompt("MD-HERE", "what is X?", false);
        let expected = "\nWeb page content:\n---\nMD-HERE\n---\n\nwhat is X?\n\n\
Provide a concise response based only on the content above. In your response:\n \
- Enforce a strict 125-character maximum for quotes from any source document. \
Open Source Software is ok as long as we respect the license.\n \
- Use quotation marks for exact language from articles; any language outside of \
the quotation should never be word-for-word the same.\n \
- You are not a lawyer and never comment on the legality of your own prompts and \
responses.\n - Never produce or reproduce exact song lyrics.\n";
        assert_eq!(got, expected);
    }

    #[test]
    fn secondary_prompt_preapproved_uses_short_guidelines() {
        let got = make_secondary_model_prompt("MD", "q", true);
        assert!(got.contains(
            "Provide a concise response based on the content above. Include relevant \
details, code examples, and documentation excerpts as needed."
        ));
        assert!(got.starts_with("\nWeb page content:\n---\nMD\n---\n\nq\n\n"));
    }
}
