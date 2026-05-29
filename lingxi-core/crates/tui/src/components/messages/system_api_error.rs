//! `SystemAPIErrorMessage` — error body + retry-countdown footer.
//!
//! Literal lock (claude-code `SystemAPIErrorMessage.tsx`): error-colored body;
//! if truncated (>1000 chars, non-verbose) append `…` + a `(ctrl+o to expand)`
//! hint (`CtrlOToExpand`); footer dim:
//! `Retrying in {n} second(s)… (attempt {a}/{m})` (singular `second` when
//! `n == 1`). `MAX_API_ERROR_CHARS = 1000`.
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
    out
}

/// iocraft component — error body (red) + dim footer.
#[component]
pub fn SystemApiErrorMessage(props: &SystemApiErrorProps) -> impl Into<AnyElement<'static>> {
    let unit = if props.retry_in_seconds == 1 {
        "second"
    } else {
        "seconds"
    };
    let body = if props.truncated {
        format!("{}\u{2026}\n{EXPAND_HINT}", props.error)
    } else {
        props.error.clone()
    };
    let footer = format!(
        "Retrying in {} {unit}\u{2026} (attempt {}/{})",
        props.retry_in_seconds, props.retry_attempt, props.max_retries,
    );
    element! {
        View(flex_direction: FlexDirection::Column) {
            Text(content: body, color: TuiTheme::ERROR)
            Text(content: footer, color: TuiTheme::DIM)
        }
    }
}
