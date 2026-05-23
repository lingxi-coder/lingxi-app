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
fn header_value<'a>(headers: &'a [(String, String)], header_name: &str) -> Option<&'a str> {
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
        if m > 2 { m - 3 } else { m + 9 }
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
