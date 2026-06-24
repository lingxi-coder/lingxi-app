//! `SystemAPIErrorMessage` — error body + retry-countdown footer.
//!
//! Literal lock (claude-code `SystemAPIErrorMessage.tsx`): error-colored body;
//! if truncated (>1000 chars, non-verbose) append `…` + a `(ctrl+o to expand)`
//! hint (`CtrlOToExpand`); footer dim:
//! `Retrying in {n} second(s)… (attempt {a}/{m})` (singular `second` when
//! `n == 1`), plus (ma-09) a ` · API_TIMEOUT_MS={ms}ms, try increasing it`
//! suffix when that env var is set. `MAX_API_ERROR_CHARS = 1000`.
#![allow(clippy::needless_pass_by_value)]

use iocraft::prelude::*;

use crate::theme::TuiTheme;

/// Truncation hint surface (`CtrlOToExpand`, renders `(ctrl+o to expand)`).
pub const EXPAND_HINT: &str = "(ctrl+o to expand)";

/// Props for [`SystemApiErrorMessage`].
#[derive(Debug, Clone, Default, Props)]
pub struct SystemApiErrorProps {
    /// Formatted API error text.
    pub error: String,
    /// 1-based retry attempt.
    pub retry_attempt: u32,
    /// Seconds until the next retry.
    pub retry_in_seconds: u32,
    /// Max retry attempts.
    pub max_retries: u32,
    /// `true` → error body was clipped; append `…` + hint.
    pub truncated: bool,
}

/// Pure-string renderer.
#[must_use]
pub fn render_system_api_error_to_string(props: SystemApiErrorProps) -> String {
    let mut out = props.error.clone();
    if props.truncated {
        out.push('\u{2026}');
        out.push('\n');
        out.push_str(EXPAND_HINT);
    }
    let unit = if props.retry_in_seconds == 1 {
        "second"
    } else {
        "seconds"
    };
    out.push('\n');
    out.push_str(&format!(
        "Retrying in {n} {unit}\u{2026} (attempt {a}/{m})",
        n = props.retry_in_seconds,
        a = props.retry_attempt,
        m = props.max_retries,
    ));
    // (ma-09) When API_TIMEOUT_MS is set, claude-code appends a hint that the
    // retry/backoff is governed by it.
    if let Ok(ms) = std::env::var("API_TIMEOUT_MS") {
        if !ms.is_empty() {
            out.push_str(&format!(" \u{00B7} API_TIMEOUT_MS={ms}ms, try increasing it"));
        }
    }
    out
}

/// iocraft component — error body (red) + dim footer.
#[component]
pub fn SystemApiErrorMessage(props: &SystemApiErrorProps) -> impl Into<AnyElement<'static>> {
    // Delegate to the oracle for the body/footer/truncation computation rather
    // than recomputing it (keeps the two in lock-step, no drift). The oracle
    // emits `body…\n(hint)\nfooter`; the footer is always the last line. We
    // need a per-line color split (error body red, retry footer dim) that the
    // flat oracle string can't express, so we split the footer off the last
    // `\n` and color the two parts — byte-identical to the oracle string.
    let full = render_system_api_error_to_string(props.clone());
    let (body, footer) = full
        .rsplit_once('\n')
        .map_or((full.as_str(), ""), |(b, f)| (b, f));
    let body = body.to_string();
    let footer = footer.to_string();
    element! {
        View(flex_direction: FlexDirection::Column) {
            Text(content: body, color: TuiTheme::ERROR)
            Text(content: footer, color: TuiTheme::DIM)
        }
    }
}
