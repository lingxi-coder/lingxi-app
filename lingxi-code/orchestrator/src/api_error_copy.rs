//! User-facing API-error copy — the `${IT}: …` family from claude-code's error
//! renderer (2.1.220 @230600350–230602900).
//!
//! Every string here was verified fragment by fragment against the binary, with
//! a control string that must NOT match: a pattern grep over a binary carrying
//! both UTF-8 and UTF-16LE text reports false positives without one.
//!
//! The separator between clauses is U+00B7 `·` (the binary's `\xB7`), not a
//! hyphen and not a bullet.
//!
//! # Scope
//!
//! This module covers the 429 family. The rest of the error-surfacing set
//! (`Credit balance is too low`, `Invalid API key · Please run /login`, the
//! PDF-page and password branches) is still unported and is NOT silently
//! absorbed here.

use serde_json::Value;

/// Oracle `IT` — the prefix every rendered API error carries.
pub(crate) const API_ERROR: &str = "API Error";

/// Oracle `\xB7` — the clause separator. A hyphen here would be wrong.
const SEP: char = '·';

/// Oracle `Gcs`: does this message mean "the 1M-context window needs paid usage
/// credits"? Two accepted phrasings, matched verbatim.
#[must_use]
pub(crate) fn is_long_context_credit_message(message: &str) -> bool {
    message.contains("Extra usage is required for long context")
        || message.contains("Usage credits are required for long context")
}

/// Oracle: `${IT}: Usage credits required for 1M context · ${hint}`.
///
/// `non_interactive` picks the hint (`_n()`): an interactive session is told to
/// run the slash commands, a non-interactive one is pointed at the settings URL
/// and `--model`, because it has no slash commands to run.
#[must_use]
pub(crate) fn usage_credits_required_for_1m_context(non_interactive: bool) -> String {
    let hint = if non_interactive {
        format!("turn on usage credits at {USAGE_SETTINGS_URL}, or use --model to switch to standard context")
    } else {
        "run /usage-credits to turn them on, or /model to switch to standard context".to_string()
    };
    format!("{API_ERROR}: Usage credits required for 1M context {SEP} {hint}")
}

/// Oracle `xYr`.
const USAGE_SETTINGS_URL: &str = "claude.ai/settings/usage?from=cc_cli_limit_message";

/// Oracle `LYr` — the billing surface for `LlmError::QuotaExceeded`.
///
/// Rendered BARE: `yu({content:LYr,error:"billing_error"})` carries no
/// `API Error:` prefix, unlike the 429 family. Getting that wrong would be
/// invisible in review and wrong on screen.
pub(crate) const CREDIT_BALANCE_TOO_LOW: &str = "Credit balance is too low";

/// Oracle `Jq` — the prompt-too-long surface for `LlmError::ContextOverflow`.
///
/// Also bare: `yu({content:Jq,error:"invalid_request"})`.
pub(crate) const PROMPT_TOO_LONG: &str = "Prompt is too long";

/// Oracle `le_` — the first-party variant of the rejection label, used instead
/// of `Request rejected (429)` when the limit is the server's rather than the
/// account's.
///
/// Not selected yet: choosing between this and [`REQUEST_REJECTED_429`] needs
/// the provider-route signal the oracle's `i` carries, which this layer does
/// not have. Kept (and tested) so the string is already byte-verified when that
/// plumbing lands — deleting and re-deriving it later is how transcription
/// errors get in.
#[allow(dead_code)]
pub(crate) const SERVER_LIMITING: &str = "Server is temporarily limiting requests (not your usage limit)";

/// Recover the detail clause from a 429 message — oracle:
///
/// ```js
/// let c = e.message.replace(/^429\s+/,""), u;
/// try { let m = Ut(c), g = m?.error?.message ?? m?.message; if (typeof g==="string") u = g } catch {}
/// let d = u || c;
/// ```
///
/// Strip the status prefix, try to read the body as JSON and take
/// `error.message` (falling back to a top-level `message`), and use the stripped
/// remainder verbatim when it is not JSON. This only works because the decoder
/// stringifies the whole body into the message — see
/// `llm_client::providers::api_error_message`.
#[must_use]
pub(crate) fn rate_limit_detail(message: &str) -> String {
    let stripped = strip_status_prefix(message, 429);
    let parsed = serde_json::from_str::<Value>(stripped)
        .ok()
        .and_then(|v| {
            v.get("error")
                .and_then(|e| e.get("message"))
                .or_else(|| v.get("message"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .filter(|s| !s.is_empty());
    parsed.unwrap_or_else(|| stripped.to_string())
}

/// `message.replace(/^{status}\s+/, "")` — strip the status and the whitespace
/// run after it. Returns the input unchanged when the prefix is absent.
fn strip_status_prefix(message: &str, status: u16) -> &str {
    let digits = status.to_string();
    let Some(rest) = message.strip_prefix(&digits) else {
        return message;
    };
    let trimmed = rest.trim_start_matches([' ', '\t', '\n', '\r']);
    // `\s+` requires at least one whitespace character; `"429x"` keeps its text.
    if trimmed.len() == rest.len() {
        return message;
    }
    trimmed
}

/// Oracle: `${IT}: ${label} · ${detail || fallback}`.
///
/// `label` is `Request rejected (429)` normally, or [`SERVER_LIMITING`] when the
/// throttle is the server's. `fallback` is the
/// `this may be a temporary capacity issue.{suffix}` clause, used only when the
/// detail is empty — the suffix is provider-dependent (oracle `hpo()`), so the
/// caller supplies it.
#[must_use]
pub(crate) fn rate_limited_text(message: &str, label: &str, fallback: &str) -> String {
    let detail = rate_limit_detail(message);
    let clause = if detail.is_empty() { fallback } else { &detail };
    format!("{API_ERROR}: {label} {SEP} {clause}")
}

/// The default rejection label — oracle's non-first-party branch.
pub(crate) const REQUEST_REJECTED_429: &str = "Request rejected (429)";

/// The fallback clause's stem. The oracle appends `hpo()`, which names the
/// status page for a first-party route and the configured gateway host
/// otherwise; that suffix needs provider plumbing this module does not have, so
/// callers pass the whole fallback in.
pub(crate) const TEMPORARY_CAPACITY: &str = "this may be a temporary capacity issue.";

#[cfg(test)]
mod tests {
    use super::*;

    /// Both of these render BARE — no `API Error:` prefix — which is the easy
    /// thing to get wrong when every neighbouring string has one.
    #[test]
    fn the_bare_surfaces_carry_no_prefix() {
        assert_eq!(CREDIT_BALANCE_TOO_LOW, "Credit balance is too low");
        assert_eq!(PROMPT_TOO_LONG, "Prompt is too long");
        for s in [CREDIT_BALANCE_TOO_LOW, PROMPT_TOO_LONG] {
            assert!(!s.starts_with(API_ERROR), "{s} must not be prefixed");
        }
    }

    #[test]
    fn the_rendered_429_is_byte_exact() {
        // The common shape: a decoded body, so the detail comes from JSON.
        let msg = r#"429 {"type":"error","error":{"type":"rate_limit_error","message":"slow down"}}"#;
        assert_eq!(
            rate_limited_text(msg, REQUEST_REJECTED_429, TEMPORARY_CAPACITY),
            "API Error: Request rejected (429) \u{b7} slow down"
        );
        // The separator is U+00B7, not a hyphen or an ASCII middot lookalike.
        assert!(rate_limited_text(msg, REQUEST_REJECTED_429, TEMPORARY_CAPACITY)
            .contains('\u{b7}'));
    }

    #[test]
    fn the_first_party_label_swaps_in() {
        assert_eq!(
            rate_limited_text("429 {\"message\":\"x\"}", SERVER_LIMITING, TEMPORARY_CAPACITY),
            "API Error: Server is temporarily limiting requests (not your usage limit) \u{b7} x"
        );
    }

    #[test]
    fn detail_falls_back_through_json_then_text_then_the_capacity_clause() {
        // `error.message` wins.
        assert_eq!(
            rate_limit_detail(r#"429 {"error":{"message":"a"},"message":"b"}"#),
            "a"
        );
        // Top-level `message` when there is no `error.message`.
        assert_eq!(rate_limit_detail(r#"429 {"message":"b"}"#), "b");
        // Not JSON → the stripped remainder verbatim.
        assert_eq!(rate_limit_detail("429 Too Many Requests"), "Too Many Requests");
        // Nothing after the status → empty, so the caller's fallback shows.
        assert_eq!(
            rate_limited_text("429 ", REQUEST_REJECTED_429, TEMPORARY_CAPACITY),
            "API Error: Request rejected (429) \u{b7} this may be a temporary capacity issue."
        );
    }

    #[test]
    fn the_status_prefix_strip_requires_whitespace() {
        // `/^429\s+/` — no whitespace, no strip.
        assert_eq!(rate_limit_detail("429Too Many"), "429Too Many");
        // Absent prefix is left alone.
        assert_eq!(rate_limit_detail("Too Many"), "Too Many");
        // Multiple spaces are all consumed (`\s+`).
        assert_eq!(rate_limit_detail("429   spaced"), "spaced");
    }

    #[test]
    fn the_1m_context_copy_is_byte_exact_in_both_modes() {
        assert_eq!(
            usage_credits_required_for_1m_context(false),
            "API Error: Usage credits required for 1M context \u{b7} run /usage-credits to \
             turn them on, or /model to switch to standard context"
        );
        assert_eq!(
            usage_credits_required_for_1m_context(true),
            "API Error: Usage credits required for 1M context \u{b7} turn on usage credits at \
             claude.ai/settings/usage?from=cc_cli_limit_message, or use --model to switch to \
             standard context"
        );
    }

    #[test]
    fn the_long_context_credit_gate_matches_both_phrasings() {
        assert!(is_long_context_credit_message(
            "429 Extra usage is required for long context requests"
        ));
        assert!(is_long_context_credit_message(
            "Usage credits are required for long context"
        ));
        assert!(!is_long_context_credit_message("rate limited"));
    }
}
