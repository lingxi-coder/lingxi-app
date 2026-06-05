//! Rate-limit header parsing for Anthropic responses.
//!
//! Spec §7 (lines 665-666):
//! * `Retry-After` — seconds, RFC 7231 §7.1.3 delta-seconds form.
//! * `anthropic-ratelimit-requests-reset` — ISO8601 UTC.
//!
//! The user-facing error string `"Rate limited; retrying in {N}s"` is locked
//! byte-for-byte (spec §5 recovery-strategy table line 517).

#![forbid(unsafe_code)]

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Locked rate-limit error message format. Used by `ApiError::RateLimited::fmt`.
/// Spec §5 line 517 — byte-for-byte against claude-code.
pub const RATE_LIMIT_ERR_FMT: &str = "Rate limited; retrying in {N}s";

/// Build the byte-locked user-facing rate-limit error string for `secs` seconds.
#[must_use]
pub fn format_rate_limited_msg(secs: u64) -> String {
    format!("Rate limited; retrying in {secs}s")
}

/// Look up `header_name` case-insensitively in a header vec and return the
/// first matching value, if any.
pub(crate) fn header_value<'a>(
    headers: &'a [(String, String)],
    header_name: &str,
) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(header_name))
        .map(|(_, v)| v.as_str())
}

/// Parse the `Retry-After` header (RFC 7231 §7.1.3 delta-seconds form).
///
/// Returns `None` if the header is missing, empty, or in the HTTP-date form
/// (which we deliberately don't support — claude-code parity).
#[must_use]
pub fn parse_retry_after(headers: &[(String, String)]) -> Option<Duration> {
    let raw = header_value(headers, "retry-after")?.trim();
    if raw.is_empty() {
        return None;
    }
    raw.parse::<u64>().ok().map(Duration::from_secs)
}

/// Parse `anthropic-ratelimit-requests-reset` (ISO8601 `YYYY-MM-DDTHH:MM:SSZ`)
/// into a `Duration` relative to `now`. Negative diffs (past timestamps)
/// clamp to `Duration::ZERO`. Non-conforming values return `None`.
///
/// The parser is hand-rolled to avoid pulling `chrono` into the runtime
/// crate; it accepts the exact `YYYY-MM-DDTHH:MM:SSZ` shape claude-code emits.
#[must_use]
pub fn parse_anthropic_ratelimit_reset(
    headers: &[(String, String)],
    now: SystemTime,
) -> Option<Duration> {
    let raw = header_value(headers, "anthropic-ratelimit-requests-reset")?.trim();
    let target = parse_iso8601_utc(raw)?;
    let now_secs = now.duration_since(UNIX_EPOCH).ok()?.as_secs();
    Some(Duration::from_secs(target.saturating_sub(now_secs)))
}

/// Cap a persistent rate-limit reset wait at 6 hours (claude-code
/// `PERSISTENT_RESET_CAP_MS`).
pub const PERSISTENT_RESET_CAP_MS: u64 = 6 * 60 * 60 * 1000;

/// Parse `anthropic-ratelimit-unified-reset` (a Unix-epoch **seconds** value —
/// distinct from the ISO8601 `…-requests-reset` above) into a `Duration` from
/// `now`, capped at [`PERSISTENT_RESET_CAP_MS`]. 1:1 with claude-code
/// `getRateLimitResetDelayMs` (`withRetry.ts:814-821`): a past-or-equal reset
/// (`delayMs <= 0`) returns `None` so the caller falls through to the next
/// delay source (NOT clamped to zero, unlike the ISO parser above).
///
/// Divergence: claude-code uses JS `Number()`, accepting decimal/scientific
/// forms; we parse integer epoch seconds (the form the server sends). A
/// non-integer value yields `None` (same fail-soft outcome).
#[must_use]
pub fn parse_unified_reset(headers: &[(String, String)], now: SystemTime) -> Option<Duration> {
    let raw = header_value(headers, "anthropic-ratelimit-unified-reset")?.trim();
    let reset_unix_sec: u64 = raw.parse().ok()?;
    let now_ms = u64::try_from(now.duration_since(UNIX_EPOCH).ok()?.as_millis()).ok()?;
    let reset_ms = reset_unix_sec.checked_mul(1000)?;
    // checked_sub → None for a PAST reset; an exactly-now reset (0) also yields
    // None (claude-code `delayMs <= 0 → null`).
    let delay_ms = reset_ms.checked_sub(now_ms)?;
    if delay_ms == 0 {
        return None;
    }
    Some(Duration::from_millis(delay_ms.min(PERSISTENT_RESET_CAP_MS)))
}

/// The `anthropic-ratelimit-unified-overage-disabled-reason` header value, if
/// the server signalled that overage spend is disabled (claude-code
/// `withRetry.ts:276`). Surfaced so callers can avoid retrying a 429 that
/// cannot succeed until the window resets.
#[must_use]
pub fn overage_disabled_reason(headers: &[(String, String)]) -> Option<&str> {
    header_value(headers, "anthropic-ratelimit-unified-overage-disabled-reason")
}

/// Parsed unified rate-limit state used to render the user-facing 429 message.
///
/// Mirrors the subset of claude-code `ClaudeAILimits` (claudeAiLimits.ts:122)
/// that the *error* (rejected) message path reads: the representative claim
/// (`rate_limit_type`), the overage status, and the overage-disabled reason.
/// The reset-time strings are pre-formatted by the caller (claude-code threads
/// `formatResetTime(...)` output, which is locale/timezone dependent and thus
/// not byte-reproducible here) — the *templates* around them are byte-locked.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RateLimitInfo {
    /// `anthropic-ratelimit-unified-representative-claim` — which window was
    /// exhausted (`five_hour` / `seven_day` / `seven_day_opus` /
    /// `seven_day_sonnet`). `None` when the header is absent.
    pub rate_limit_type: Option<String>,
    /// `anthropic-ratelimit-unified-overage-status` — `allowed` /
    /// `allowed_warning` / `rejected`. `None` when the header is absent.
    pub overage_status: Option<String>,
    /// `anthropic-ratelimit-unified-overage-disabled-reason` — e.g.
    /// `out_of_credits`. `None` when the header is absent.
    pub overage_disabled_reason: Option<String>,
}

impl RateLimitInfo {
    /// Parse the unified rate-limit headers the 429 error-message path reads.
    /// 1:1 with the `error.headers?.get(...)` reads in claude-code
    /// `errors.ts:471-516` + `claudeAiLimits.ts` `computeNewLimitsFromHeaders`.
    #[must_use]
    pub fn from_headers(headers: &[(String, String)]) -> Self {
        Self {
            rate_limit_type: header_value(
                headers,
                "anthropic-ratelimit-unified-representative-claim",
            )
            .map(str::to_string),
            overage_status: header_value(headers, "anthropic-ratelimit-unified-overage-status")
                .map(str::to_string),
            overage_disabled_reason: overage_disabled_reason(headers).map(str::to_string),
        }
    }

    /// `true` when at least one unified-limit header is present — matches the
    /// claude-code gate `if (rateLimitType || overageStatus)` (`errors.ts:480`)
    /// that decides whether the new message generator runs at all.
    #[must_use]
    pub fn has_unified_headers(&self) -> bool {
        self.rate_limit_type.is_some() || self.overage_status.is_some()
    }
}

/// Pre-formatted reset-time strings threaded into the 429 message template.
///
/// claude-code derives these from `formatResetTime(limits.resetsAt, true)` and
/// `formatResetTime(limits.overageResetsAt, true)` (`rateLimitMessages.ts:144-147`).
/// That formatter is locale/timezone dependent, so the api-client layer accepts
/// the already-formatted strings and only owns the byte-locked surrounding
/// template. Pass `None` when the corresponding reset timestamp was absent.
#[derive(Debug, Clone, Default)]
pub struct ResetTimes<'a> {
    /// Formatted `limits.resetsAt` (the primary window reset), if present.
    pub reset_time: Option<&'a str>,
    /// Formatted `limits.overageResetsAt` (the overage window reset), if present.
    pub overage_reset_time: Option<&'a str>,
    /// `true` when the *unformatted* `resetsAt` timestamp was present — used by
    /// the dual-reset overage branch to pick the earlier of the two windows.
    /// claude-code compares the raw `resetsAt < overageResetsAt` numbers
    /// (`rateLimitMessages.ts:155-166`); the caller passes the comparison result
    /// via [`Self::reset_is_earlier`] so this layer stays format-agnostic.
    pub reset_is_earlier: Option<bool>,
}

impl<'a> ResetTimes<'a> {
    /// Pick the reset message for the dual-overage-rejected branch
    /// (claude-code `rateLimitMessages.ts:154-166`): when both reset times are
    /// present, use the earlier window's formatted string (per
    /// `reset_is_earlier`); otherwise fall back to whichever single one exists.
    fn overage_reset_message(&self) -> String {
        match (self.reset_time, self.overage_reset_time) {
            (Some(rt), Some(ort)) => {
                // Both present: claude-code uses the earlier of resetsAt /
                // overageResetsAt. `reset_is_earlier` carries that numeric
                // comparison; default to the primary reset when unknown.
                if self.reset_is_earlier.unwrap_or(true) {
                    format!(" · resets {rt}")
                } else {
                    format!(" · resets {ort}")
                }
            }
            (Some(rt), None) => format!(" · resets {rt}"),
            (None, Some(ort)) => format!(" · resets {ort}"),
            (None, None) => String::new(),
        }
    }
}

/// Whether the running user is a Pro or Enterprise subscriber — gates the
/// `seven_day_sonnet` wording (claude-code `rateLimitMessages.ts:176-181`,
/// `getSubscriptionType() === 'pro' || 'enterprise'`). Pre-computed by the
/// caller (subscription state lives outside api-client) and handed in, the same
/// seam as `is_subscriber` / `is_enterprise` on the retry path.
#[derive(Debug, Clone, Copy, Default)]
pub struct SubscriptionContext {
    /// `true` when `getSubscriptionType()` is `pro` or `enterprise`.
    pub is_pro_or_enterprise: bool,
}

/// Build the byte-faithful user-facing 429 rate-limit error message from the
/// rejected-limit state — the **error** (not warning) branch of claude-code
/// `getRateLimitErrorMessage` → `getRateLimitMessage` → `getLimitReachedText`
/// (`rateLimitMessages.ts:110-197`). Returns `None` when no error message
/// applies (the TS warning/overage paths that return `null`), letting the
/// caller fall through to its generic 429 handling.
///
/// Byte-locked templates (external build; the `USER_TYPE === 'ant'` feedback
/// channel variant at `:339-340` is omitted — same external-only stance as the
/// rest of api-client). `reset` carries the locale-formatted reset strings.
#[must_use]
pub fn rate_limit_error_message(
    info: &RateLimitInfo,
    reset: &ResetTimes<'_>,
    sub: SubscriptionContext,
) -> Option<String> {
    // claude-code builds `limits` with status='rejected' and isUsingOverage
    // defaulted false (errors.ts:482-486). getRateLimitMessage therefore skips
    // the `isUsingOverage` branch and the `allowed_warning` warning branch, and
    // lands directly on the rejected → getLimitReachedText path
    // (rateLimitMessages.ts:63-64). getRateLimitErrorMessage only returns the
    // message when severity === 'error', which this path always is.
    Some(limit_reached_text(info, reset, sub))
}

/// Port of claude-code `getLimitReachedText` (`rateLimitMessages.ts:143-197`).
fn limit_reached_text(
    info: &RateLimitInfo,
    reset: &ResetTimes<'_>,
    sub: SubscriptionContext,
) -> String {
    // `const resetMessage = resetTime ? ` · resets ${resetTime}` : ''` (:149).
    let reset_message = reset
        .reset_time
        .map(|rt| format!(" · resets {rt}"))
        .unwrap_or_default();

    // if BOTH subscription and overage are exhausted (:152).
    if info.overage_status.as_deref() == Some("rejected") {
        let overage_reset_message = reset.overage_reset_message();
        // `out_of_credits` → "You're out of extra usage…" (:168-170).
        if info.overage_disabled_reason.as_deref() == Some("out_of_credits") {
            return format!("You're out of extra usage{overage_reset_message}");
        }
        // else formatLimitReachedText('limit', overageResetMessage) (:172).
        return format_limit_reached_text("limit", &overage_reset_message);
    }

    match info.rate_limit_type.as_deref() {
        Some("seven_day_sonnet") => {
            // pro/enterprise: Sonnet limit is the weekly limit (:176-181).
            let limit = if sub.is_pro_or_enterprise {
                "weekly limit"
            } else {
                "Sonnet limit"
            };
            format_limit_reached_text(limit, &reset_message)
        }
        Some("seven_day_opus") => format_limit_reached_text("Opus limit", &reset_message),
        Some("seven_day") => format_limit_reached_text("weekly limit", &reset_message),
        Some("five_hour") => format_limit_reached_text("session limit", &reset_message),
        _ => format_limit_reached_text("usage limit", &reset_message),
    }
}

/// Port of claude-code `formatLimitReachedText` (`rateLimitMessages.ts:333-344`),
/// external build only — the `USER_TYPE === 'ant'` feedback-channel variant is
/// intentionally omitted (api-client is the external CLI surface).
fn format_limit_reached_text(limit: &str, reset_message: &str) -> String {
    format!("You've hit your {limit}{reset_message}")
}

/// Parse the strict `YYYY-MM-DDTHH:MM:SSZ` form into a Unix-epoch second.
/// Hand-rolled — no chrono runtime dep needed.
#[allow(
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    clippy::similar_names,
    clippy::unreadable_literal,
    reason = "Hinnant days_from_civil algorithm: ranges are mathematically bounded by the YYYY-MM-DDTHH:MM:SSZ validation above; literals 146097/719468 are canonical algorithm constants"
)]
fn parse_iso8601_utc(s: &str) -> Option<u64> {
    // 1234567890123456789012345
    // YYYY-MM-DDTHH:MM:SSZ
    if s.len() != 20 || !s.ends_with('Z') {
        return None;
    }
    let year: i64 = s[0..4].parse().ok()?;
    let month: u32 = s[5..7].parse().ok()?;
    let day: u32 = s[8..10].parse().ok()?;
    let hour: u32 = s[11..13].parse().ok()?;
    let minute: u32 = s[14..16].parse().ok()?;
    let second: u32 = s[17..19].parse().ok()?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour >= 24
        || minute >= 60
        || second >= 60
    {
        return None;
    }
    // Days since Unix epoch (1970-01-01), using the proleptic Gregorian
    // calendar. We only need to handle 1970-2099 for our use case; the
    // formula is the standard "civil from days" inverse.
    let y = year;
    let m = i64::from(month);
    let d = i64::from(day);
    // Howard Hinnant's "days_from_civil" algorithm:
    let y_adj = y - i64::from(m <= 2);
    let era = if y_adj >= 0 { y_adj } else { y_adj - 399 } / 400;
    let yoe = (y_adj - era * 400) as u64; // [0, 399]
    let doy: u64 = (153 * {
        let m = m as u64;
        if m > 2 {
            m - 3
        } else {
            m + 9
        }
    } + 2)
        / 5
        + d as u64
        - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    let days = era * 146097 + doe as i64 - 719468; // 719468 = days from 0000-03-01 to 1970-01-01
    if days < 0 {
        return None;
    }
    let secs = (days as u64) * 86_400
        + u64::from(hour) * 3_600
        + u64::from(minute) * 60
        + u64::from(second);
    Some(secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(k: &str, v: &str) -> Vec<(String, String)> {
        vec![(k.into(), v.into())]
    }

    #[test]
    fn rate_limit_err_fmt_is_byte_locked() {
        assert_eq!(RATE_LIMIT_ERR_FMT, "Rate limited; retrying in {N}s");
        assert_eq!(format_rate_limited_msg(7), "Rate limited; retrying in 7s");
    }

    #[test]
    fn retry_after_seconds_form_parses() {
        let r = parse_retry_after(&h("Retry-After", "5"));
        assert_eq!(r, Some(Duration::from_secs(5)));
    }

    #[test]
    fn unified_reset_future_epoch_yields_delay() {
        let now = UNIX_EPOCH + Duration::from_secs(1_000_000);
        // reset 60s in the future (epoch seconds 1_000_060)
        let r = parse_unified_reset(&h("anthropic-ratelimit-unified-reset", "1000060"), now);
        assert_eq!(r, Some(Duration::from_secs(60)));
    }

    #[test]
    fn unified_reset_past_or_now_is_none() {
        let now = UNIX_EPOCH + Duration::from_secs(1_000_000);
        // past → None (fall through, NOT zero)
        assert_eq!(
            parse_unified_reset(&h("anthropic-ratelimit-unified-reset", "999000"), now),
            None
        );
        // exactly now → None
        assert_eq!(
            parse_unified_reset(&h("anthropic-ratelimit-unified-reset", "1000000"), now),
            None
        );
    }

    #[test]
    fn unified_reset_clamps_to_six_hours() {
        let now = UNIX_EPOCH + Duration::from_secs(1_000_000);
        // reset a year out → capped at 6h
        let r = parse_unified_reset(
            &h("anthropic-ratelimit-unified-reset", "1031536000"),
            now,
        );
        assert_eq!(r, Some(Duration::from_millis(PERSISTENT_RESET_CAP_MS)));
    }

    #[test]
    fn unified_reset_non_numeric_and_missing_are_none() {
        let now = UNIX_EPOCH + Duration::from_secs(1_000_000);
        assert_eq!(
            parse_unified_reset(&h("anthropic-ratelimit-unified-reset", "soon"), now),
            None
        );
        assert_eq!(parse_unified_reset(&[], now), None);
    }

    #[test]
    fn overage_disabled_reason_reads_header() {
        assert_eq!(
            overage_disabled_reason(&h(
                "anthropic-ratelimit-unified-overage-disabled-reason",
                "spend_limit"
            )),
            Some("spend_limit")
        );
        assert_eq!(overage_disabled_reason(&[]), None);
    }

    #[test]
    fn retry_after_case_insensitive_lookup() {
        let r = parse_retry_after(&h("retry-after", "12"));
        assert_eq!(r, Some(Duration::from_secs(12)));
    }

    #[test]
    fn retry_after_http_date_form_rejected() {
        let r = parse_retry_after(&h("Retry-After", "Wed, 21 Oct 2026 07:28:00 GMT"));
        assert_eq!(r, None);
    }

    #[test]
    fn retry_after_missing_returns_none() {
        let r = parse_retry_after(&[]);
        assert_eq!(r, None);
    }

    #[test]
    fn anthropic_ratelimit_reset_parses_future_iso8601() {
        // 2026-05-23T12:00:00Z relative to 2026-05-23T11:59:50Z should be 10s.
        let now = SystemTime::UNIX_EPOCH
            + Duration::from_secs(parse_iso8601_utc("2026-05-23T11:59:50Z").unwrap());
        let r = parse_anthropic_ratelimit_reset(
            &h("anthropic-ratelimit-requests-reset", "2026-05-23T12:00:00Z"),
            now,
        );
        assert_eq!(r, Some(Duration::from_secs(10)));
    }

    #[test]
    fn anthropic_ratelimit_reset_past_clamps_to_zero() {
        let now = SystemTime::UNIX_EPOCH
            + Duration::from_secs(parse_iso8601_utc("2026-05-23T13:00:00Z").unwrap());
        let r = parse_anthropic_ratelimit_reset(
            &h("anthropic-ratelimit-requests-reset", "2026-05-23T12:00:00Z"),
            now,
        );
        assert_eq!(r, Some(Duration::ZERO));
    }

    #[test]
    fn anthropic_ratelimit_reset_malformed_returns_none() {
        let r = parse_anthropic_ratelimit_reset(
            &h("anthropic-ratelimit-requests-reset", "not-an-iso-date"),
            SystemTime::UNIX_EPOCH,
        );
        assert_eq!(r, None);
    }

    #[test]
    fn iso8601_parser_handles_2026_05_23_correctly() {
        // 2026-05-23T00:00:00Z must round-trip to a positive Unix epoch
        // and be exactly 56 years × 365.25 days × 86400 sec ≈ 1.77 * 10^9.
        let secs = parse_iso8601_utc("2026-05-23T00:00:00Z").unwrap();
        assert!(secs > 1_700_000_000 && secs < 1_900_000_000);
    }
}

#[cfg(test)]
mod rate_limit_message {
    //! Byte-faithful tests for the 429 user-facing message
    //! (`rate_limit_error_message` / `RateLimitInfo`), named so the cargo filter
    //! `rate_limit::rate_limit_message` matches them all. Templates are locked
    //! against claude-code `rateLimitMessages.ts`.
    use super::*;

    fn info(rate_limit_type: Option<&str>, overage_status: Option<&str>) -> RateLimitInfo {
        RateLimitInfo {
            rate_limit_type: rate_limit_type.map(str::to_string),
            overage_status: overage_status.map(str::to_string),
            overage_disabled_reason: None,
        }
    }

    #[test]
    fn from_headers_reads_unified_headers() {
        let headers = vec![
            (
                "anthropic-ratelimit-unified-representative-claim".into(),
                "five_hour".into(),
            ),
            (
                "anthropic-ratelimit-unified-overage-status".into(),
                "rejected".into(),
            ),
            (
                "anthropic-ratelimit-unified-overage-disabled-reason".into(),
                "out_of_credits".into(),
            ),
        ];
        let parsed = RateLimitInfo::from_headers(&headers);
        assert_eq!(parsed.rate_limit_type.as_deref(), Some("five_hour"));
        assert_eq!(parsed.overage_status.as_deref(), Some("rejected"));
        assert_eq!(
            parsed.overage_disabled_reason.as_deref(),
            Some("out_of_credits")
        );
        assert!(parsed.has_unified_headers());
        assert!(!RateLimitInfo::default().has_unified_headers());
    }

    #[test]
    fn five_hour_session_limit_message_is_byte_locked() {
        // claude-code rateLimitMessages.ts:192-193 + :343.
        let msg = rate_limit_error_message(
            &info(Some("five_hour"), None),
            &ResetTimes {
                reset_time: Some("3pm"),
                ..ResetTimes::default()
            },
            SubscriptionContext::default(),
        );
        assert_eq!(msg.as_deref(), Some("You've hit your session limit · resets 3pm"));
    }

    #[test]
    fn five_hour_without_reset_time_omits_reset_clause() {
        let msg = rate_limit_error_message(
            &info(Some("five_hour"), None),
            &ResetTimes::default(),
            SubscriptionContext::default(),
        );
        assert_eq!(msg.as_deref(), Some("You've hit your session limit"));
    }

    #[test]
    fn seven_day_weekly_and_opus_messages_are_byte_locked() {
        let weekly = rate_limit_error_message(
            &info(Some("seven_day"), None),
            &ResetTimes::default(),
            SubscriptionContext::default(),
        );
        assert_eq!(weekly.as_deref(), Some("You've hit your weekly limit"));

        let opus = rate_limit_error_message(
            &info(Some("seven_day_opus"), None),
            &ResetTimes::default(),
            SubscriptionContext::default(),
        );
        assert_eq!(opus.as_deref(), Some("You've hit your Opus limit"));
    }

    #[test]
    fn seven_day_sonnet_wording_depends_on_subscription() {
        // Non pro/enterprise → "Sonnet limit" (rateLimitMessages.ts:180).
        let standard = rate_limit_error_message(
            &info(Some("seven_day_sonnet"), None),
            &ResetTimes::default(),
            SubscriptionContext {
                is_pro_or_enterprise: false,
            },
        );
        assert_eq!(standard.as_deref(), Some("You've hit your Sonnet limit"));

        // Pro/enterprise → "weekly limit" (rateLimitMessages.ts:178-181).
        let pro = rate_limit_error_message(
            &info(Some("seven_day_sonnet"), None),
            &ResetTimes::default(),
            SubscriptionContext {
                is_pro_or_enterprise: true,
            },
        );
        assert_eq!(pro.as_deref(), Some("You've hit your weekly limit"));
    }

    #[test]
    fn unknown_or_absent_rate_limit_type_falls_back_to_usage_limit() {
        // claude-code rateLimitMessages.ts:196 — default `usage limit`.
        let msg = rate_limit_error_message(
            &info(None, None),
            &ResetTimes::default(),
            SubscriptionContext::default(),
        );
        assert_eq!(msg.as_deref(), Some("You've hit your usage limit"));
    }

    #[test]
    fn overage_rejected_out_of_credits_message_is_byte_locked() {
        // claude-code rateLimitMessages.ts:168-170.
        let limits = RateLimitInfo {
            rate_limit_type: Some("five_hour".into()),
            overage_status: Some("rejected".into()),
            overage_disabled_reason: Some("out_of_credits".into()),
        };
        let msg = rate_limit_error_message(
            &limits,
            &ResetTimes {
                overage_reset_time: Some("Jun 7, 9am"),
                ..ResetTimes::default()
            },
            SubscriptionContext::default(),
        );
        assert_eq!(
            msg.as_deref(),
            Some("You're out of extra usage · resets Jun 7, 9am")
        );
    }

    #[test]
    fn overage_rejected_other_reason_uses_limit_wording() {
        // claude-code rateLimitMessages.ts:172 — formatLimitReachedText('limit', …).
        let limits = RateLimitInfo {
            rate_limit_type: Some("seven_day".into()),
            overage_status: Some("rejected".into()),
            overage_disabled_reason: None,
        };
        let msg = rate_limit_error_message(
            &limits,
            &ResetTimes {
                reset_time: Some("3pm"),
                ..ResetTimes::default()
            },
            SubscriptionContext::default(),
        );
        // The overage-rejected branch outranks rate_limit_type wording.
        assert_eq!(msg.as_deref(), Some("You've hit your limit · resets 3pm"));
    }

    #[test]
    fn overage_rejected_dual_reset_picks_earlier_window() {
        // claude-code rateLimitMessages.ts:154-166 — both present, earlier wins.
        let limits = RateLimitInfo {
            rate_limit_type: Some("seven_day".into()),
            overage_status: Some("rejected".into()),
            overage_disabled_reason: None,
        };
        // resetsAt is the earlier window → use its formatted string.
        let earlier_primary = rate_limit_error_message(
            &limits,
            &ResetTimes {
                reset_time: Some("3pm"),
                overage_reset_time: Some("Jun 9, 9am"),
                reset_is_earlier: Some(true),
            },
            SubscriptionContext::default(),
        );
        assert_eq!(earlier_primary.as_deref(), Some("You've hit your limit · resets 3pm"));

        // overageResetsAt is the earlier window → use the overage string.
        let earlier_overage = rate_limit_error_message(
            &limits,
            &ResetTimes {
                reset_time: Some("Jun 9, 9am"),
                overage_reset_time: Some("3pm"),
                reset_is_earlier: Some(false),
            },
            SubscriptionContext::default(),
        );
        assert_eq!(earlier_overage.as_deref(), Some("You've hit your limit · resets 3pm"));
    }
}
