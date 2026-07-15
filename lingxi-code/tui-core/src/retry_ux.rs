//! Retry UX decision logic (cc 2.1.198).
//!
//! Ports the pure gating rules the binary's retry-status spinner components use
//! (`pHo` @214952343, spinner `Te` branch @214957169):
//!
//! - The concrete **error reason** is shown only once `attempt >= min(3,
//!   maxRetries)` — i.e. from the 3rd attempt on ("after the 2nd attempt");
//!   before that the line reads a generic `"API error"` (binary
//!   `h = !f ? "API error" : n.error.formatted`, `f = n.attempt >= Math.min(3,
//!   n.maxRetries)`).
//! - On an **overloaded** error (`status === 529` or the formatted text
//!   contains "overload") AND `attempt >= min(3, maxRetries)`, the spinner tip
//!   is REPLACED by the status-page link (binary `if(fe&&me)Te=D2n().trim()`,
//!   where `D2n` → `" If it persists, check https://status.claude.com."`).
//!
//! LingXi note: the LIVE per-attempt retry-status event surface from
//! `llm-client` (the binary's `onRetryStatus` callback) is not wired — retries
//! are internal to `llm_client::ApiService`'s drive loop and the only
//! `RenderedMessage::SystemApiError` producer today is a demo fixture. These
//! functions capture the faithful gating so the render (and any future live
//! wiring) matches the binary.

/// The Anthropic status page URL — the binary's `zha` (@210994387).
pub const STATUS_PAGE_URL: &str = "https://status.claude.com";

/// Generic reason shown before the concrete error is revealed (binary `"API
/// error"`).
pub const GENERIC_RETRY_REASON: &str = "API error";

/// The overloaded status-page spinner tip — the binary's `D2n().trim()` for the
/// first-party path (`"If it persists, check https://status.claude.com."`).
#[must_use]
pub fn status_page_tip() -> String {
    format!("If it persists, check {STATUS_PAGE_URL}.")
}

/// `f = attempt >= min(3, max_retries)` — whether the concrete error reason is
/// revealed (binary @214952new spinner `f`/`me`). `attempt` is 1-based.
#[must_use]
pub fn reason_revealed(attempt: u32, max_retries: u32) -> bool {
    attempt >= max_retries.min(3)
}

/// The retry-status reason text: the generic `"API error"` until `attempt >=
/// min(3, max_retries)`, then the concrete `formatted` error (binary
/// `h = !f ? "API error" : n.error.formatted`).
#[must_use]
pub fn retry_error_reason(attempt: u32, max_retries: u32, formatted: &str) -> String {
    if reason_revealed(attempt, max_retries) {
        formatted.to_string()
    } else {
        GENERIC_RETRY_REASON.to_string()
    }
}

/// Whether the error is an overload (binary `fe = h.error.status===529 ||
/// h.error.formatted.toLowerCase().includes("overload")`). `status` is the HTTP
/// status when known; the text check catches the `"overloaded_error"` /
/// `"Overloaded"` cases when only the message is available.
#[must_use]
pub fn is_overloaded(status: Option<u16>, formatted: &str) -> bool {
    status == Some(529) || formatted.to_lowercase().contains("overload")
}

/// The spinner tip that REPLACES the normal tip on a retrying overloaded error:
/// `Some(status_page_tip())` when `is_overloaded && attempt >= min(3,
/// max_retries)` (binary `if(fe&&me)Te=D2n().trim()`), else `None` (keep the
/// normal tip).
#[must_use]
pub fn overloaded_status_tip(
    status: Option<u16>,
    formatted: &str,
    attempt: u32,
    max_retries: u32,
) -> Option<String> {
    if is_overloaded(status, formatted) && reason_revealed(attempt, max_retries) {
        Some(status_page_tip())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reason_hidden_until_third_attempt() {
        // min(3, 10) = 3 → revealed at attempt 3+.
        assert!(!reason_revealed(1, 10));
        assert!(!reason_revealed(2, 10));
        assert!(reason_revealed(3, 10));
        assert!(reason_revealed(4, 10));
    }

    #[test]
    fn reason_gate_respects_small_max_retries() {
        // min(3, 2) = 2 → revealed at attempt 2 when max_retries is only 2.
        assert!(!reason_revealed(1, 2));
        assert!(reason_revealed(2, 2));
        // min(3, 1) = 1 → revealed immediately when max_retries is 1.
        assert!(reason_revealed(1, 1));
    }

    #[test]
    fn error_reason_generic_before_then_concrete() {
        assert_eq!(retry_error_reason(1, 10, "529 Overloaded"), "API error");
        assert_eq!(retry_error_reason(2, 10, "529 Overloaded"), "API error");
        assert_eq!(
            retry_error_reason(3, 10, "529 Overloaded"),
            "529 Overloaded"
        );
    }

    #[test]
    fn overload_detection_by_status_or_text() {
        assert!(is_overloaded(Some(529), "whatever"));
        assert!(is_overloaded(None, "API is Overloaded"));
        assert!(is_overloaded(None, "overloaded_error"));
        assert!(!is_overloaded(Some(500), "internal error"));
        assert!(!is_overloaded(None, "rate limited"));
    }

    #[test]
    fn status_tip_only_when_overloaded_and_revealed() {
        // Overloaded but attempt < 3 → no tip yet.
        assert_eq!(overloaded_status_tip(Some(529), "overloaded", 2, 10), None);
        // Overloaded + attempt 3 → status-page tip.
        assert_eq!(
            overloaded_status_tip(Some(529), "overloaded", 3, 10),
            Some("If it persists, check https://status.claude.com.".to_string())
        );
        // Not overloaded → never the tip, even at high attempts.
        assert_eq!(overloaded_status_tip(Some(500), "internal", 5, 10), None);
    }

    #[test]
    fn status_page_url_is_claude_status() {
        assert_eq!(STATUS_PAGE_URL, "https://status.claude.com");
        assert_eq!(
            status_page_tip(),
            "If it persists, check https://status.claude.com."
        );
    }
}
