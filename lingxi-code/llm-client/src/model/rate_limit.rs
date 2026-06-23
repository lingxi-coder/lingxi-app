//! Rate-limit header parsing for Anthropic responses.
//!
//! Spec §7 (lines 665-666):
//! * `Retry-After` — seconds, RFC 7231 §7.1.3 delta-seconds form.
//! * `anthropic-ratelimit-requests-reset` — ISO8601 UTC.
//!
//! The user-facing error string `"Rate limited; retrying in {N}s"` is locked
//! byte-for-byte (spec §5 recovery-strategy table line 517).

#![forbid(unsafe_code)]

use chrono::{DateTime, Datelike, Local, TimeZone, Timelike};
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

/// 24h in milliseconds — the `formatResetTime` date-vs-time branch boundary
/// (TS `hoursUntilReset > 24`).
const TWENTY_FOUR_HOURS_MS: i64 = 24 * 60 * 60 * 1000;

/// Port of claude-code `formatResetTime` (`utils/format.ts:238-289`).
///
/// Renders a rate-limit reset timestamp (Unix **seconds**) the way claude-code
/// does for the 429 message: `en-US` locale, 12-hour clock, the minute omitted
/// when it is `:00`, the space before AM/PM removed, and AM/PM lowercased — e.g.
/// `"3pm"`, `"3:30pm"`, `"Jun 7, 3:30pm"`. Resets more than 24h out include the
/// month/day (and the year when it differs from the current year); resets within
/// 24h render the time only. `show_time = false` drops the time entirely on the
/// far-future branch. Returns `None` for a `None`/zero timestamp (TS
/// `if (!timestampInSeconds) return undefined`).
///
/// All component reads (minute, year, formatting) use **local** time, matching
/// the TS `Date` accessors (`getMinutes`/`getFullYear`) and `toLocaleString`.
///
/// `show_timezone` appends `" (<tz>)"`. See [`reset_time_zone`] for the
/// documented divergence from TS `getTimeZone()` (IANA name vs. offset).
#[must_use]
pub fn format_reset_time(
    timestamp_secs: Option<i64>,
    show_timezone: bool,
    show_time: bool,
) -> Option<String> {
    format_reset_time_at(timestamp_secs, Local::now(), show_timezone, show_time)
}

/// Testable core of [`format_reset_time`] with an injected `now` (so unit tests
/// never read the wall clock). `now` is a `DateTime<Local>`; the reset timestamp
/// is interpreted in the same local zone, exactly like the TS `Date` accessors.
#[must_use]
fn format_reset_time_at(
    timestamp_secs: Option<i64>,
    now: DateTime<Local>,
    show_timezone: bool,
    show_time: bool,
) -> Option<String> {
    // TS: `if (!timestampInSeconds) return undefined` — 0 is falsy too.
    let ts = match timestamp_secs {
        Some(t) if t != 0 => t,
        _ => return None,
    };

    // `new Date(timestampInSeconds * 1000)` in the local zone. A timestamp that
    // can't be represented (out of range) fails soft to `None`.
    let date = Local.timestamp_opt(ts, 0).single()?;

    let minutes = date.minute();

    // TS: `hoursUntilReset = (date - now) / 3_600_000ms`, branch on `> 24`.
    // Equivalent integer test `(date_ms - now_ms) > 24h_in_ms` — exact, no float
    // cast, same `>` boundary (a reset exactly 24h out stays time-only).
    let is_far_future =
        date.timestamp_millis() - now.timestamp_millis() > TWENTY_FOUR_HOURS_MS;

    let tz_suffix = |s: String| -> String {
        if show_timezone {
            format!("{s} ({})", reset_time_zone(date))
        } else {
            s
        }
    };

    if is_far_future {
        // Far-future branch: `month: short, day: numeric` always; `hour`/minute
        // only when `show_time`; `year` only when it differs from `now`.
        // en-US layouts (verified against Node `toLocaleString`):
        //   "Jun 7"                 (show_time = false)
        //   "Jun 7, 3pm"            (minutes == 0)
        //   "Jun 7, 3:30pm"         (minutes != 0)
        //   "Jun 7, 2027, 3:30pm"   (year differs)
        let mut out = format!("{} {}", date.format("%b"), date.day());
        if date.year() != now.year() {
            out.push_str(&format!(", {}", date.year()));
        }
        if show_time {
            out.push_str(&format!(", {}", format_en_us_time(&date, minutes)));
        }
        return Some(tz_suffix(out));
    }

    // Within 24h: time only, e.g. "3pm" / "3:30pm".
    Some(tz_suffix(format_en_us_time(&date, minutes)))
}

/// Format the `en-US` 12-hour time component with the claude-code lowercasing
/// applied: hour without a leading zero, the minute as `:MM` only when non-zero,
/// then the AM/PM marker lowercased and joined with no space. Reproduces TS
/// `toLocaleTimeString('en-US', { hour:'numeric', minute: …, hour12:true })`
/// followed by `.replace(/ ([AP]M)/i, …toLowerCase())`.
fn format_en_us_time(date: &DateTime<Local>, minutes: u32) -> String {
    // `%-I` = 12-hour, no leading zero; `%p` = `AM`/`PM`.
    let hour = date.format("%-I");
    let ampm = date.format("%p").to_string().to_lowercase();
    if minutes == 0 {
        format!("{hour}{ampm}")
    } else {
        format!("{hour}:{minutes:02}{ampm}")
    }
}

/// Timezone string appended when `show_timezone` is set.
///
/// Byte-faithful to claude-code `getTimeZone()`
/// (`Intl.DateTimeFormat().resolvedOptions().timeZone`), which returns the IANA
/// zone *name* (e.g. `"America/Los_Angeles"`). `iana-time-zone` is already in the
/// locked dependency graph (transitive via chrono), so this adds an edge, not a
/// new package. If the host zone cannot be resolved (rare), we fall back to the
/// chrono `%Z` local-offset rendering (e.g. `"+08:00"`) so the suffix is never
/// empty — `date` is retained only for that fallback.
fn reset_time_zone(date: DateTime<Local>) -> String {
    iana_time_zone::get_timezone().unwrap_or_else(|_| date.format("%Z").to_string())
}

/// Owned, already-formatted reset strings derived from the unified-reset
/// response headers. Lives one frame above [`ResetTimes`] (which borrows) so the
/// caller can hand the borrowed view to [`rate_limit_error_message`].
///
/// 1:1 with the header reads in claude-code `computeNewLimitsFromHeaders`
/// (`claudeAiLimits.ts:382-398`) feeding `getLimitReachedText`
/// (`rateLimitMessages.ts:143-166`): `resetsAt =
/// Number(anthropic-ratelimit-unified-reset)`, `overageResetsAt =
/// Number(anthropic-ratelimit-unified-overage-reset)`, both rendered with
/// `formatResetTime(..., /* showTimezone */ true)`, and the dual-window branch
/// picking the earlier of `resetsAt < overageResetsAt`.
#[derive(Debug, Clone, Default)]
pub struct FormattedResetTimes {
    /// `formatResetTime(resetsAt, true)`, if `resetsAt` was present.
    pub reset_time: Option<String>,
    /// `formatResetTime(overageResetsAt, true)`, if `overageResetsAt` present.
    pub overage_reset_time: Option<String>,
    /// `resetsAt < overageResetsAt` when both raw timestamps were present.
    pub reset_is_earlier: Option<bool>,
}

impl FormattedResetTimes {
    /// Borrowed view consumed by [`rate_limit_error_message`].
    #[must_use]
    pub fn as_reset_times(&self) -> ResetTimes<'_> {
        ResetTimes {
            reset_time: self.reset_time.as_deref(),
            overage_reset_time: self.overage_reset_time.as_deref(),
            reset_is_earlier: self.reset_is_earlier,
        }
    }
}

/// Read the unified reset headers and produce the locale-formatted reset strings
/// the 429 message consumes — the api-client analogue of claude-code mapping the
/// reset headers through `formatResetTime`. `showTimezone = true` matches the TS
/// call sites (`rateLimitMessages.ts:145-148`).
#[must_use]
pub fn formatted_reset_times_from_headers(headers: &[(String, String)]) -> FormattedResetTimes {
    formatted_reset_times_at(headers, Local::now())
}

/// Testable core of [`formatted_reset_times_from_headers`] with an injected
/// `now` so unit tests never read the wall clock.
#[must_use]
fn formatted_reset_times_at(
    headers: &[(String, String)],
    now: DateTime<Local>,
) -> FormattedResetTimes {
    // `Number(header)` in TS: an absent/blank/non-numeric value yields `None`
    // (NaN → falsy → `resetsAt` undefined), so the reset clause is dropped.
    let parse = |name: &str| -> Option<i64> {
        header_value(headers, name)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .and_then(|s| s.parse::<i64>().ok())
    };
    let resets_at = parse("anthropic-ratelimit-unified-reset");
    let overage_resets_at = parse("anthropic-ratelimit-unified-overage-reset");

    FormattedResetTimes {
        reset_time: format_reset_time_at(resets_at, now, true, true),
        overage_reset_time: format_reset_time_at(overage_resets_at, now, true, true),
        // `resetsAt < limits.overageResetsAt` (rateLimitMessages.ts:160) — only
        // meaningful when both raw timestamps are present.
        reset_is_earlier: match (resets_at, overage_resets_at) {
            (Some(r), Some(o)) => Some(r < o),
            _ => None,
        },
    }
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
// `Eq` cannot be derived since `utilization: Option<f64>` (NaN never occurs —
// parsing filters non-finite values — but the type still forbids `Eq`).
#[derive(Debug, Clone, Default, PartialEq)]
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
    /// `anthropic-ratelimit-unified-status` — `allowed` / `allowed_warning` /
    /// `rejected`. `None` when the header is absent or empty; claude-code
    /// defaults that case to `'allowed'` via `headers.get(…) || 'allowed'`
    /// (`claudeAiLimits.ts:379-381`), so `status.as_deref().unwrap_or("allowed")`
    /// reproduces the TS value exactly.
    pub status: Option<String>,
    /// `anthropic-ratelimit-unified-reset` — Unix-epoch **seconds** when the
    /// representative window resets (`claudeAiLimits.ts:382-383`,
    /// `Number(resetsAtHeader)`). Absent/malformed → `None` (TS would store
    /// `NaN` for a malformed value; we fail soft, same as
    /// [`parse_unified_reset`]).
    pub resets_at: Option<u64>,
    /// Per-claim `anthropic-ratelimit-unified-{abbrev}-utilization` (0-1
    /// fraction) for the representative claim's abbrev — see [`claim_abbrev`]
    /// and `extractRawUtilization` (`claudeAiLimits.ts:164-179`). `None` when
    /// the claim has no per-claim headers (`seven_day_opus` /
    /// `seven_day_sonnet` / unknown), the header is absent, or the value is
    /// malformed/non-finite.
    pub utilization: Option<f64>,
    /// Per-claim `anthropic-ratelimit-unified-{abbrev}-reset` — Unix-epoch
    /// seconds when the representative claim's window resets
    /// (`claudeAiLimits.ts:173,272-279`). Same `None` conditions as
    /// [`Self::utilization`].
    pub claim_resets_at: Option<u64>,
    /// `anthropic-ratelimit-unified-overage-reset` — Unix-epoch seconds when
    /// the overage window resets (`claudeAiLimits.ts:394-399`).
    /// Absent/malformed → `None`.
    pub overage_resets_at: Option<u64>,
    /// `anthropic-ratelimit-unified-fallback` strict-equals `"available"`
    /// (`claudeAiLimits.ts:384-385`, no trim/case-fold). `None` when the
    /// header is absent (TS collapses that to `false`;
    /// `fallback_available.unwrap_or(false)` reproduces the exact TS boolean);
    /// `Some(false)` when present with any other value.
    pub fallback_available: Option<bool>,
    /// `surpassedThreshold` (`claudeAiLimits.ts:135`) — the warning-threshold
    /// fraction from `anthropic-ratelimit-unified-{abbrev}-surpassed-threshold`.
    /// Set ONLY by the header-based early-warning replacement
    /// (`claudeAiLimits.ts:288`); the base header parse and the time-relative
    /// fallback leave it `None` (the TS fresh object at `:332-339` omits it).
    pub surpassed_threshold: Option<f64>,
}

/// Map a representative-claim value to the abbreviation used in the per-claim
/// headers `anthropic-ratelimit-unified-{abbrev}-utilization` / `-{abbrev}-reset`.
///
/// Pinned 1:1 to the only claim↔abbrev associations in claude-code:
/// `extractRawUtilization` pairs `['five_hour','5h']` / `['seven_day','7d']`
/// (`claudeAiLimits.ts:166-169`) and `EARLY_WARNING_CLAIM_MAP`
/// `{'5h':'five_hour','7d':'seven_day','overage':'overage'}`
/// (`claudeAiLimits.ts:73-77`). `seven_day_opus` / `seven_day_sonnet` have NO
/// per-claim headers anywhere in the TS — the full unified-header inventory
/// (`mockRateLimits.ts:33-40`) lists only `5h` / `7d` / `overage` variants —
/// so they, like unknown claims, return `None` (TS never attaches a
/// utilization to those claims).
fn claim_abbrev(claim: &str) -> Option<&'static str> {
    match claim {
        "five_hour" => Some("5h"),
        "seven_day" => Some("7d"),
        "overage" => Some("overage"),
        _ => None,
    }
}

/// Tolerant epoch-seconds read, mirroring TS `Number(header)` fail-soft:
/// absent/blank/non-numeric → `None`. Divergence (same stance as
/// `parse_unified_reset`): TS `Number()` accepts decimal/scientific epoch
/// forms; we parse integer epoch seconds (the form the server sends) and fail
/// soft otherwise.
fn parse_epoch_secs(headers: &[(String, String)], name: &str) -> Option<u64> {
    header_value(headers, name)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|s| s.parse::<u64>().ok())
}

/// Tolerant 0-1-fraction read (utilization / surpassed-threshold headers).
/// Non-finite values (`NaN`/`inf` parse as valid f64 in Rust) are rejected so
/// `RateLimitInfo: PartialEq` comparisons stay total in practice.
fn parse_fraction(headers: &[(String, String)], name: &str) -> Option<f64> {
    header_value(headers, name)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|f| f.is_finite())
}

/// One window of raw utilization (`RawWindowUtilization`,
/// `claudeAiLimits.ts:150-153`). Both fields are required — a window is
/// parsed ATOMICALLY (both `-utilization` and `-reset` headers, ts:174) or
/// not at all, so a `RawWindow` can never carry a dangling half.
// `Eq` cannot be derived (`utilization: f64`); `parse_fraction` filters
// non-finite values so `PartialEq` stays total in practice.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RawWindow {
    /// 0-1 utilization fraction.
    pub utilization: f64,
    /// Unix epoch seconds when the window resets.
    pub resets_at: u64,
}

/// Raw per-window utilization — claude-code `extractRawUtilization`
/// (`claudeAiLimits.ts:164-179`). Tracked on EVERY response with unified
/// headers, independent of the warning-gated [`RateLimitInfo`] fields
/// (which only surface the representative claim's window). A window needs
/// BOTH its `-utilization` and `-reset` headers (`util !== null && reset
/// !== null`, ts:174); the default (both `None`) is the TS empty `{}`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RawUtilization {
    /// 5-hour window (`anthropic-ratelimit-unified-5h-*`).
    pub five_hour: Option<RawWindow>,
    /// 7-day window (`anthropic-ratelimit-unified-7d-*`).
    pub seven_day: Option<RawWindow>,
}

impl RawUtilization {
    /// Parse from response headers. Absent/empty headers → `Self::default()`.
    ///
    /// Per window (`['five_hour','5h'] / ['seven_day','7d']`,
    /// `claudeAiLimits.ts:166-169`): both the `-utilization` and `-reset`
    /// header must be present AND parse (the file's tolerant-parse stance —
    /// TS `Number()` would store `NaN` for a present-but-malformed value; we
    /// drop the window instead, the same fail-soft divergence documented on
    /// [`parse_fraction`] / [`parse_epoch_secs`]).
    #[must_use]
    pub fn from_headers(headers: &[(String, String)]) -> Self {
        let window = |abbrev: &str| -> Option<RawWindow> {
            let utilization = parse_fraction(
                headers,
                &format!("anthropic-ratelimit-unified-{abbrev}-utilization"),
            )?;
            let resets_at = parse_epoch_secs(
                headers,
                &format!("anthropic-ratelimit-unified-{abbrev}-reset"),
            )?;
            Some(RawWindow {
                utilization,
                resets_at,
            })
        };
        Self {
            five_hour: window("5h"),
            seven_day: window("7d"),
        }
    }
}

/// One early-warning threshold pair (`claudeAiLimits.ts:38-41`): warn when
/// usage `>= utilization` AND the elapsed window fraction `<= time_pct`
/// (high consumption early in the window).
struct EarlyWarningThreshold {
    utilization: f64,
    time_pct: f64,
}

/// One time-relative early-warning configuration (`claudeAiLimits.ts:43-48`).
struct EarlyWarningConfig {
    rate_limit_type: &'static str,
    claim_abbrev: &'static str,
    window_seconds: u64,
    thresholds: &'static [EarlyWarningThreshold],
}

/// `EARLY_WARNING_CONFIGS` (`claudeAiLimits.ts:53-70`) — time-relative
/// fallback configs in priority order (checked first to last), used when the
/// server doesn't send a surpassed-threshold header.
const EARLY_WARNING_CONFIGS: [EarlyWarningConfig; 2] = [
    EarlyWarningConfig {
        rate_limit_type: "five_hour",
        claim_abbrev: "5h",
        window_seconds: 5 * 60 * 60,
        thresholds: &[EarlyWarningThreshold {
            utilization: 0.9,
            time_pct: 0.72,
        }],
    },
    EarlyWarningConfig {
        rate_limit_type: "seven_day",
        claim_abbrev: "7d",
        window_seconds: 7 * 24 * 60 * 60,
        thresholds: &[
            EarlyWarningThreshold {
                utilization: 0.75,
                time_pct: 0.6,
            },
            EarlyWarningThreshold {
                utilization: 0.5,
                time_pct: 0.35,
            },
            EarlyWarningThreshold {
                utilization: 0.25,
                time_pct: 0.15,
            },
        ],
    },
];

/// `EARLY_WARNING_CLAIM_MAP` (`claudeAiLimits.ts:73-77`) in the object's
/// insertion order — `Object.entries` iteration order is what gives `5h`
/// priority over `7d` over `overage` in the header-based check (`:260-262`).
const EARLY_WARNING_CLAIM_MAP: [(&str, &str); 3] = [
    ("5h", "five_hour"),
    ("7d", "seven_day"),
    ("overage", "overage"),
];

/// `computeTimeProgress` (`claudeAiLimits.ts:98-103`): fraction (0-1) of the
/// window that has elapsed — `clamp((now − (resetsAt − window)) / window, 0, 1)`.
#[allow(
    clippy::cast_precision_loss,
    reason = "realistic epoch seconds (< 2^53) and the fixed window constants are exactly representable in f64 — same number math as the TS; an adversarial u64 above 2^53 does lose precision but stays finite and the clamp(0, 1) keeps the result safe"
)]
fn compute_time_progress(resets_at: u64, window_seconds: u64, now: SystemTime) -> f64 {
    // `Date.now() / 1000` (ts:99). A pre-epoch clock cannot happen in
    // practice; fail soft to 0.0 rather than panic.
    let now_seconds = now
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64());
    let window_start = resets_at as f64 - window_seconds as f64;
    let elapsed = now_seconds - window_start;
    (elapsed / window_seconds as f64).clamp(0.0, 1.0)
}

/// `getHeaderBasedEarlyWarning` (`claudeAiLimits.ts:255-294`): iterate the
/// claim map in order; the first claim whose
/// `anthropic-ratelimit-unified-{abbrev}-surpassed-threshold` header is
/// PRESENT (`!== null`, `:268`) wins and yields a FRESH replacement limits
/// object (`:281-289`) — only the fields the TS object sets are populated.
fn header_based_early_warning(
    headers: &[(String, String)],
    fallback_available: Option<bool>,
) -> Option<RateLimitInfo> {
    for (abbrev, rate_limit_type) in EARLY_WARNING_CLAIM_MAP {
        let surpassed_name = format!("anthropic-ratelimit-unified-{abbrev}-surpassed-threshold");
        if header_value(headers, &surpassed_name).is_none() {
            continue;
        }
        // `utilizationHeader ? Number(...) : undefined` (ts:276-279) — the
        // tolerant parsers reproduce the absent→undefined collapse.
        let resets_at = parse_epoch_secs(
            headers,
            &format!("anthropic-ratelimit-unified-{abbrev}-reset"),
        );
        return Some(RateLimitInfo {
            status: Some("allowed_warning".to_string()),
            // The TS fresh object folds the per-claim reset into the
            // top-level `resetsAt` (ts:283); our struct also splits out the
            // per-claim value, so the same timestamp lands in both.
            resets_at,
            claim_resets_at: resets_at,
            rate_limit_type: Some(rate_limit_type.to_string()),
            utilization: parse_fraction(
                headers,
                &format!("anthropic-ratelimit-unified-{abbrev}-utilization"),
            ),
            fallback_available,
            // `Number(surpassedThreshold)` (ts:288). Divergence (same
            // fail-soft stance as `parse_fraction` everywhere else): TS would
            // store `NaN` for a malformed value — but `0` for an EMPTY string
            // (`Number('')` is `0`, not `NaN`); we store `None` for both. The
            // warning itself still fires on header PRESENCE alone (ts:268).
            surpassed_threshold: parse_fraction(headers, &surpassed_name),
            // Fresh-object semantics: overage fields stay `None`
            // (`isUsingOverage: false` in TS has no struct counterpart).
            ..RateLimitInfo::default()
        });
    }
    None
}

/// `getTimeRelativeEarlyWarning` (`claudeAiLimits.ts:301-340`): client-side
/// fallback for one config. Requires BOTH per-claim headers (`:315`); warns
/// when ANY threshold has `utilization >= t.utilization && timeProgress <=
/// t.timePct` (`:324-326`). The fresh object carries NO `surpassedThreshold`
/// (`:332-339`).
fn time_relative_early_warning(
    headers: &[(String, String)],
    config: &EarlyWarningConfig,
    fallback_available: Option<bool>,
    now: SystemTime,
) -> Option<RateLimitInfo> {
    let abbrev = config.claim_abbrev;
    // TS gates on header PRESENCE (`=== null`) then `Number()`s the values; a
    // malformed value becomes `NaN`, every `NaN` comparison is false, and no
    // warning fires — requiring a successful parse here is behaviourally
    // identical and keeps the fail-soft stance of the tolerant parsers.
    let utilization = parse_fraction(
        headers,
        &format!("anthropic-ratelimit-unified-{abbrev}-utilization"),
    )?;
    let resets_at = parse_epoch_secs(
        headers,
        &format!("anthropic-ratelimit-unified-{abbrev}-reset"),
    )?;
    let time_progress = compute_time_progress(resets_at, config.window_seconds, now);
    let should_warn = config
        .thresholds
        .iter()
        .any(|t| utilization >= t.utilization && time_progress <= t.time_pct);
    if !should_warn {
        return None;
    }
    Some(RateLimitInfo {
        status: Some("allowed_warning".to_string()),
        // Same `resetsAt` fold as the header-based path (ts:334).
        resets_at: Some(resets_at),
        claim_resets_at: Some(resets_at),
        rate_limit_type: Some(config.rate_limit_type.to_string()),
        utilization: Some(utilization),
        fallback_available,
        ..RateLimitInfo::default()
    })
}

/// `getEarlyWarningFromHeaders` (`claudeAiLimits.ts:347-374`): header-based
/// detection first (preferred when the API sends the header), else the
/// time-relative configs in priority order.
fn early_warning_from_headers(
    headers: &[(String, String)],
    fallback_available: Option<bool>,
    now: SystemTime,
) -> Option<RateLimitInfo> {
    if let Some(warning) = header_based_early_warning(headers, fallback_available) {
        return Some(warning);
    }
    EARLY_WARNING_CONFIGS
        .iter()
        .find_map(|config| time_relative_early_warning(headers, config, fallback_available, now))
}

impl RateLimitInfo {
    /// Parse the unified rate-limit headers the 429 error-message path reads.
    /// 1:1 with the `error.headers?.get(...)` reads in claude-code
    /// `errors.ts:471-516` + `claudeAiLimits.ts` `computeNewLimitsFromHeaders`.
    /// Thin wrapper over [`Self::from_headers_at`] with the wall clock.
    #[must_use]
    pub fn from_headers(headers: &[(String, String)]) -> Self {
        Self::from_headers_at(headers, SystemTime::now())
    }

    /// Testable core of [`Self::from_headers`] with an injected `now` — the
    /// clock only feeds `computeTimeProgress` (`claudeAiLimits.ts:98-103`) on
    /// the time-relative early-warning fallback path.
    ///
    /// After the raw header parse, the `computeNewLimitsFromHeaders`
    /// final-status semantics apply (`claudeAiLimits.ts:411-424`): when the
    /// parsed status is `allowed`/`allowed_warning`, a firing early warning
    /// REPLACES the whole parse, and otherwise a bare `allowed_warning` is
    /// downgraded to `allowed`; `rejected` passes through untouched.
    #[must_use]
    pub fn from_headers_at(headers: &[(String, String)], now: SystemTime) -> Self {
        let rate_limit_type = header_value(
            headers,
            "anthropic-ratelimit-unified-representative-claim",
        )
        .map(str::to_string);

        // Per-claim window reads keyed by the representative claim's abbrev
        // (`anthropic-ratelimit-unified-{abbrev}-utilization` / `-reset`,
        // claudeAiLimits.ts:164-179). No abbrev → no per-claim read.
        let abbrev = rate_limit_type.as_deref().and_then(claim_abbrev);
        let utilization = abbrev.and_then(|a| {
            parse_fraction(headers, &format!("anthropic-ratelimit-unified-{a}-utilization"))
        });
        let claim_resets_at = abbrev.and_then(|a| {
            parse_epoch_secs(headers, &format!("anthropic-ratelimit-unified-{a}-reset"))
        });

        let mut parsed = Self {
            rate_limit_type,
            overage_status: header_value(headers, "anthropic-ratelimit-unified-overage-status")
                .map(str::to_string),
            overage_disabled_reason: overage_disabled_reason(headers).map(str::to_string),
            // `headers.get(…) || 'allowed'` (claudeAiLimits.ts:379-381) —
            // empty string is falsy in TS, so it is "no usable value" → None.
            status: header_value(headers, "anthropic-ratelimit-unified-status")
                .filter(|s| !s.is_empty())
                .map(str::to_string),
            resets_at: parse_epoch_secs(headers, "anthropic-ratelimit-unified-reset"),
            utilization,
            claim_resets_at,
            overage_resets_at: parse_epoch_secs(
                headers,
                "anthropic-ratelimit-unified-overage-reset",
            ),
            // `=== 'available'` (claudeAiLimits.ts:384-385): strict equality
            // on the verbatim header value — no trim, no case-fold.
            fallback_available: header_value(headers, "anthropic-ratelimit-unified-fallback")
                .map(|v| v == "available"),
            // Only ever set by the header-based early-warning replacement
            // below (claudeAiLimits.ts:288) — never by the raw parse.
            surpassed_threshold: None,
        };

        // Final-status semantics (claudeAiLimits.ts:411-424) — the early
        // warning is only consulted when status is allowed/allowed_warning.
        match parsed.status.as_deref() {
            Some("allowed" | "allowed_warning") => {
                if let Some(warning) =
                    early_warning_from_headers(headers, parsed.fallback_available, now)
                {
                    // TS RETURNS the fresh early-warning object (ts:419-421),
                    // discarding the regular parse — including every overage
                    // field. Every field on this struct maps to a
                    // `ClaudeAILimits` member (claudeAiLimits.ts:122-136), so
                    // there are no transport-only fields to carry over.
                    return warning;
                }
                // No early-warning threshold surpassed → a bare
                // allowed_warning is DOWNGRADED (ts:423 `finalStatus = 'allowed'`).
                parsed.status = Some("allowed".to_string());
                parsed
            }
            // `Some(_)`: 'rejected' (and any other non-empty value) passes
            // through — ts:413 keeps `finalStatus = status` outside the
            // allowed branch.
            //
            // `None`: deliberate divergence-guard. TS defaults a missing
            // status to 'allowed' (ts:379-381), which would route it through
            // the branch above — but fabricating a status with zero unified
            // headers present would break `has_unified_headers()` consumers,
            // so an absent header stays `None` (readers apply
            // `.unwrap_or("allowed")`). Note the guard's reach: it suppresses
            // the early-warning check for ANY response missing the status
            // header — even one carrying other unified headers, where TS's
            // 'allowed' default could still fire a warning — not just the
            // zero-header case. Improbable in practice: the server sends the
            // status header alongside the per-claim headers.
            Some(_) | None => parsed,
        }
    }

    /// `true` when at least one unified-limit header is present — matches the
    /// claude-code gate `if (rateLimitType || overageStatus)` (`errors.ts:480`)
    /// that decides whether the new message generator runs at all.
    #[must_use]
    pub fn has_unified_headers(&self) -> bool {
        self.rate_limit_type.is_some() || self.overage_status.is_some()
    }

    /// Build the rejected-limits view from a 429 **error** response's headers
    /// — 1:1 with claude-code `errors.ts:471-516`, which constructs a FRESH
    /// `ClaudeAILimits` object directly from `error.headers` (it does NOT run
    /// `computeNewLimitsFromHeaders`, so no early-warning replacement and no
    /// final-status downgrade apply here).
    ///
    /// Returns `None` when neither the representative-claim nor the
    /// overage-status header carries a non-empty value — the
    /// `if (rateLimitType || overageStatus)` gate (`errors.ts:480`; an empty
    /// header value is falsy in TS). On `Some`, `status` is FORCED to
    /// `"rejected"` (`errors.ts:482-486` builds `{ status: 'rejected', … }`)
    /// and only the five fields the TS object sets are populated:
    /// `resetsAt`/`rateLimitType`/`overageStatus`/`overageResetsAt`/
    /// `overageDisabledReason` (`errors.ts:488-517`, each behind its own
    /// truthiness check).
    #[must_use]
    pub fn from_429_error_headers(headers: &[(String, String)]) -> Option<Self> {
        // `headers?.get?.(…)` + TS truthiness: empty string is falsy, so it
        // neither passes the gate nor is assigned onto the limits object.
        let rate_limit_type = header_value(
            headers,
            "anthropic-ratelimit-unified-representative-claim",
        )
        .filter(|s| !s.is_empty())
        .map(str::to_string);
        let overage_status = header_value(headers, "anthropic-ratelimit-unified-overage-status")
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        if rate_limit_type.is_none() && overage_status.is_none() {
            return None;
        }
        Some(Self {
            // errors.ts:483 — status is 'rejected' regardless of any
            // `anthropic-ratelimit-unified-status` header on the error.
            status: Some("rejected".to_string()),
            rate_limit_type,
            overage_status,
            // `if (resetHeader) limits.resetsAt = Number(resetHeader)`
            // (errors.ts:489-494) — the tolerant parse reproduces the
            // absent/empty→skip collapse; malformed values fail soft to
            // `None` (the documented divergence from TS storing `NaN`).
            resets_at: parse_epoch_secs(headers, "anthropic-ratelimit-unified-reset"),
            overage_resets_at: parse_epoch_secs(
                headers,
                "anthropic-ratelimit-unified-overage-reset",
            ),
            // `if (overageDisabledReason)` (errors.ts:511-516) — empty is falsy.
            overage_disabled_reason: overage_disabled_reason(headers)
                .filter(|s| !s.is_empty())
                .map(str::to_string),
            ..Self::default()
        })
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

/// Seconds to wait, from an llm-client rate-limit error (server value wins).
///
/// Returns `Some(secs)` for a `RateLimited` error with a `retry_after` duration
/// (minimum 1 second), `None` for `RateLimited` with no duration, and `None`
/// for all other error types.
///
/// Note: the retry driver sleeps on the `LlmError`'s Duration verbatim; this helper is only for user-facing display.
#[must_use]
pub fn retry_secs_from_error(error: &crate::LlmError) -> Option<u64> {
    match error {
        crate::LlmError::RateLimited {
            retry_after: Some(d),
            ..
        } => Some(d.as_secs().max(1)),
        _ => None,
    }
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

    // ---- adapter tests (Step 2) ----

    #[test]
    fn retry_secs_from_rate_limited_7s() {
        let err = crate::LlmError::RateLimited {
            retry_after: Some(Duration::from_secs(7)),
            scope: None,
        };
        assert_eq!(retry_secs_from_error(&err), Some(7));
    }

    #[test]
    fn retry_secs_from_rate_limited_zero_ms_clamps_to_1() {
        let err = crate::LlmError::RateLimited {
            retry_after: Some(Duration::from_millis(0)),
            scope: None,
        };
        // 0ms → as_secs() == 0, max(1) → Some(1)
        assert_eq!(retry_secs_from_error(&err), Some(1));
    }

    #[test]
    fn retry_secs_from_rate_limited_none_retry_after() {
        let err = crate::LlmError::RateLimited {
            retry_after: None,
            scope: None,
        };
        assert_eq!(retry_secs_from_error(&err), None);
    }

    #[test]
    fn retry_secs_from_non_rate_limit_error_is_none() {
        assert_eq!(retry_secs_from_error(&crate::LlmError::ProviderInternal), None);
        assert_eq!(retry_secs_from_error(&crate::LlmError::Authentication), None);
        assert_eq!(retry_secs_from_error(&crate::LlmError::Overloaded { repeated: false }), None);
    }

    #[test]
    fn format_rate_limited_msg_7_byte_locked() {
        // The byte-locked template output for secs=7.
        assert_eq!(format_rate_limited_msg(7), "Rate limited; retrying in 7s");
    }
}

#[cfg(test)]
mod format_reset_time_tests {
    //! Byte-faithful tests for the `formatResetTime` port. `now` and the reset
    //! instant are both built from **local** wall-clock components and injected
    //! into `format_reset_time_at`, so the assertions are independent of the
    //! runner's timezone: the function re-derives local Y/M/D/H/M from the epoch
    //! it is given, which round-trips the components we constructed. The tz
    //! suffix (`reset_time_zone`) renders the host IANA zone name via
    //! `iana-time-zone` (byte-faithful to TS `getTimeZone()`), which is
    //! host-dependent — so the showTimezone tests assert the structural ` (…)`
    //! shape, not a literal zone string.
    use super::*;

    /// A `DateTime<Local>` for the given local wall-clock components. Tests pass
    /// `.timestamp()` of this into the formatter; `format_reset_time_at` then
    /// rebuilds the same local components, so the rendered output matches what
    /// these inputs describe regardless of the machine timezone.
    fn local(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(y, mo, d, h, mi, 0).single().unwrap()
    }

    fn fmt(
        reset: DateTime<Local>,
        now: DateTime<Local>,
        show_tz: bool,
        show_time: bool,
    ) -> Option<String> {
        format_reset_time_at(Some(reset.timestamp()), now, show_tz, show_time)
    }

    #[test]
    fn none_or_zero_timestamp_returns_none() {
        let now = local(2026, 6, 5, 12, 0);
        assert_eq!(format_reset_time_at(None, now, false, true), None);
        // TS `if (!timestampInSeconds)` treats 0 as falsy.
        assert_eq!(format_reset_time_at(Some(0), now, false, true), None);
    }

    #[test]
    fn within_24h_same_minute_zero_drops_minutes() {
        // 3:00pm, ~3h out → "3pm".
        let now = local(2026, 6, 5, 12, 0);
        let reset = local(2026, 6, 5, 15, 0);
        assert_eq!(fmt(reset, now, false, true).as_deref(), Some("3pm"));
    }

    #[test]
    fn within_24h_with_minutes_renders_colon_minutes() {
        // 3:30pm, ~3.5h out → "3:30pm".
        let now = local(2026, 6, 5, 12, 0);
        let reset = local(2026, 6, 5, 15, 30);
        assert_eq!(fmt(reset, now, false, true).as_deref(), Some("3:30pm"));
    }

    #[test]
    fn within_24h_midnight_and_noon_edges() {
        // 12am (midnight) and 12pm (noon) — hour12 = 12, not 0.
        let now = local(2026, 6, 5, 23, 0);
        let midnight = local(2026, 6, 6, 0, 0);
        assert_eq!(fmt(midnight, now, false, true).as_deref(), Some("12am"));

        let now2 = local(2026, 6, 5, 10, 0);
        let noon = local(2026, 6, 5, 12, 5);
        assert_eq!(fmt(noon, now2, false, true).as_deref(), Some("12:05pm"));
    }

    #[test]
    fn over_24h_same_year_includes_month_day_and_time() {
        // Reset Jun 7 3:30pm, now Jun 5 → >24h, same year → "Jun 7, 3:30pm".
        let now = local(2026, 6, 5, 10, 0);
        let reset = local(2026, 6, 7, 15, 30);
        assert_eq!(fmt(reset, now, false, true).as_deref(), Some("Jun 7, 3:30pm"));
    }

    #[test]
    fn over_24h_same_year_minute_zero_drops_minutes() {
        // Reset Jun 7 3:00pm → "Jun 7, 3pm".
        let now = local(2026, 6, 5, 10, 0);
        let reset = local(2026, 6, 7, 15, 0);
        assert_eq!(fmt(reset, now, false, true).as_deref(), Some("Jun 7, 3pm"));
    }

    #[test]
    fn over_24h_different_year_includes_year() {
        // Reset 2027 → year differs → "Jun 7, 2027, 3:30pm".
        let now = local(2026, 6, 5, 10, 0);
        let reset = local(2027, 6, 7, 15, 30);
        assert_eq!(
            fmt(reset, now, false, true).as_deref(),
            Some("Jun 7, 2027, 3:30pm")
        );
    }

    #[test]
    fn over_24h_show_time_false_drops_time() {
        // showTime=false on the far-future branch → "Jun 7" (no time at all).
        let now = local(2026, 6, 5, 10, 0);
        let reset = local(2026, 6, 7, 15, 30);
        assert_eq!(fmt(reset, now, false, false).as_deref(), Some("Jun 7"));
    }

    #[test]
    fn over_24h_different_year_show_time_false_keeps_year() {
        let now = local(2026, 6, 5, 10, 0);
        let reset = local(2027, 6, 7, 15, 30);
        assert_eq!(fmt(reset, now, false, false).as_deref(), Some("Jun 7, 2027"));
    }

    #[test]
    fn ampm_is_lowercased_with_no_space() {
        // Explicitly assert the AM/PM lowercasing + space removal: never "PM",
        // never " pm".
        let now = local(2026, 6, 5, 1, 0);
        let am = local(2026, 6, 5, 9, 15);
        let s = fmt(am, now, false, true).unwrap();
        assert_eq!(s, "9:15am");
        assert!(!s.contains("AM") && !s.contains("PM") && !s.contains(' '));
    }

    #[test]
    fn show_timezone_appends_parenthesised_suffix() {
        // Date/time portion is byte-faithful; the tz suffix is the chrono %Z
        // offset (documented divergence), so assert the structural shape: the
        // base string, a single space, then a non-empty "(…)".
        let now = local(2026, 6, 5, 12, 0);
        let reset = local(2026, 6, 5, 15, 30);
        let off = fmt(reset, now, false, true).unwrap();
        assert_eq!(off, "3:30pm");

        let on = fmt(reset, now, true, true).unwrap();
        assert!(on.starts_with("3:30pm ("), "got {on}");
        assert!(on.ends_with(')'), "got {on}");
        // The suffix is exactly " (<tz>)" with a non-empty tz.
        let suffix = on.strip_prefix("3:30pm ").unwrap();
        assert!(suffix.len() > 2, "tz suffix should be non-empty: {suffix}");
    }

    #[test]
    fn show_timezone_on_far_future_branch_appends_suffix_after_time() {
        let now = local(2026, 6, 5, 10, 0);
        let reset = local(2026, 6, 7, 15, 30);
        let on = fmt(reset, now, true, true).unwrap();
        assert!(on.starts_with("Jun 7, 3:30pm ("), "got {on}");
        assert!(on.ends_with(')'), "got {on}");
    }

    #[test]
    fn exactly_24h_boundary_uses_time_only_branch() {
        // hoursUntilReset must be strictly > 24 for the date branch. Exactly 24h
        // stays on the time-only branch (TS `hoursUntilReset > 24`).
        let now = local(2026, 6, 5, 15, 30);
        let reset = local(2026, 6, 6, 15, 30); // exactly 24h
        assert_eq!(fmt(reset, now, false, true).as_deref(), Some("3:30pm"));

        // One minute past 24h flips to the date branch.
        let reset_past = local(2026, 6, 6, 15, 31);
        assert_eq!(
            fmt(reset_past, now, false, true).as_deref(),
            Some("Jun 6, 3:31pm")
        );
    }

    #[test]
    fn formatted_reset_times_from_headers_maps_both_windows() {
        let now = local(2026, 6, 5, 10, 0);
        let resets_at = local(2026, 6, 5, 13, 0).timestamp(); // 1pm, within 24h
        let overage_at = local(2026, 6, 7, 9, 0).timestamp(); // Jun 7 9am, >24h
        let headers = vec![
            (
                "anthropic-ratelimit-unified-reset".into(),
                resets_at.to_string(),
            ),
            (
                "anthropic-ratelimit-unified-overage-reset".into(),
                overage_at.to_string(),
            ),
        ];
        // `formatted_reset_times_*` calls the formatter with showTimezone=true
        // (matching TS `formatResetTime(resetsAt, true)`), so each string carries
        // the tz suffix. The date/time prefix is byte-faithful; the parenthesised
        // suffix is the chrono %Z offset (documented divergence), so assert the
        // prefix + shape rather than a literal zone.
        let f = formatted_reset_times_at(&headers, now);
        let rt = f.reset_time.as_deref().unwrap();
        assert!(rt.starts_with("1pm ("), "got {rt}");
        assert!(rt.ends_with(')'), "got {rt}");
        let ort = f.overage_reset_time.as_deref().unwrap();
        assert!(ort.starts_with("Jun 7, 9am ("), "got {ort}");
        assert!(ort.ends_with(')'), "got {ort}");
        // resetsAt (1pm today) < overageResetsAt (Jun 7) → earlier is the primary.
        assert_eq!(f.reset_is_earlier, Some(true));

        // The borrowed view threads straight into the message template, carrying
        // the formatted reset string (with its tz suffix) into ` · resets …`.
        let info = RateLimitInfo {
            rate_limit_type: Some("five_hour".into()),
            overage_status: None,
            overage_disabled_reason: None,
            ..RateLimitInfo::default()
        };
        let msg = rate_limit_error_message(
            &info,
            &f.as_reset_times(),
            SubscriptionContext::default(),
        )
        .unwrap();
        assert!(
            msg.starts_with("You've hit your session limit · resets 1pm ("),
            "got {msg}"
        );
        assert!(msg.ends_with(')'), "got {msg}");
    }

    #[test]
    fn formatted_reset_times_absent_or_non_numeric_headers_yield_none() {
        let now = local(2026, 6, 5, 10, 0);
        // Missing both → all None (Number(undefined) → NaN → undefined).
        let f = formatted_reset_times_at(&[], now);
        assert_eq!(f.reset_time, None);
        assert_eq!(f.overage_reset_time, None);
        assert_eq!(f.reset_is_earlier, None);

        // Non-numeric `reset` header → None (Number("soon") → NaN).
        let headers = vec![("anthropic-ratelimit-unified-reset".into(), "soon".into())];
        let f2 = formatted_reset_times_at(&headers, now);
        assert_eq!(f2.reset_time, None);
        assert_eq!(f2.reset_is_earlier, None);
    }

    #[test]
    fn formatted_reset_times_overage_only_picks_overage_window() {
        let now = local(2026, 6, 5, 10, 0);
        let overage_at = local(2026, 6, 5, 14, 0).timestamp(); // 2pm
        let headers = vec![(
            "anthropic-ratelimit-unified-overage-reset".into(),
            overage_at.to_string(),
        )];
        let f = formatted_reset_times_at(&headers, now);
        assert_eq!(f.reset_time, None);
        // showTimezone=true → "2pm (<offset>)"; assert prefix/shape.
        let ort = f.overage_reset_time.as_deref().unwrap();
        assert!(ort.starts_with("2pm ("), "got {ort}");
        assert!(ort.ends_with(')'), "got {ort}");
        // Only one timestamp present → comparison is undefined → None.
        assert_eq!(f.reset_is_earlier, None);
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
            ..RateLimitInfo::default()
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
            ..RateLimitInfo::default()
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
            ..RateLimitInfo::default()
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
            ..RateLimitInfo::default()
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

#[cfg(test)]
mod unified_header_parse {
    //! Tests for the additive `RateLimitInfo` unified-header extension
    //! (`status` / `resets_at` / `utilization` / `claim_resets_at` /
    //! `overage_resets_at` / `fallback_available`), pinned against claude-code
    //! `computeNewLimitsFromHeaders` (`claudeAiLimits.ts:376-436`) and the
    //! claim→abbrev associations in `extractRawUtilization` /
    //! `EARLY_WARNING_CLAIM_MAP` (`claudeAiLimits.ts:73-77`, `:164-179`).
    use super::*;

    fn hdrs(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn full_header_set_parses_every_field() {
        let headers = hdrs(&[
            ("anthropic-ratelimit-unified-status", "rejected"),
            ("anthropic-ratelimit-unified-reset", "1750000000"),
            (
                "anthropic-ratelimit-unified-representative-claim",
                "five_hour",
            ),
            ("anthropic-ratelimit-unified-5h-utilization", "0.92"),
            ("anthropic-ratelimit-unified-5h-reset", "1750000100"),
            ("anthropic-ratelimit-unified-overage-status", "allowed"),
            ("anthropic-ratelimit-unified-overage-reset", "1750000200"),
            (
                "anthropic-ratelimit-unified-overage-disabled-reason",
                "out_of_credits",
            ),
            ("anthropic-ratelimit-unified-fallback", "available"),
        ]);
        let p = RateLimitInfo::from_headers(&headers);
        // Existing three fields — behaviour unchanged.
        assert_eq!(p.rate_limit_type.as_deref(), Some("five_hour"));
        assert_eq!(p.overage_status.as_deref(), Some("allowed"));
        assert_eq!(p.overage_disabled_reason.as_deref(), Some("out_of_credits"));
        // New additive fields.
        assert_eq!(p.status.as_deref(), Some("rejected"));
        assert_eq!(p.resets_at, Some(1_750_000_000));
        assert_eq!(p.utilization, Some(0.92));
        assert_eq!(p.claim_resets_at, Some(1_750_000_100));
        assert_eq!(p.overage_resets_at, Some(1_750_000_200));
        assert_eq!(p.fallback_available, Some(true));
    }

    #[test]
    fn no_headers_yield_all_none() {
        let p = RateLimitInfo::from_headers(&[]);
        assert_eq!(p, RateLimitInfo::default());
        assert_eq!(p.status, None);
        assert_eq!(p.resets_at, None);
        assert_eq!(p.utilization, None);
        assert_eq!(p.claim_resets_at, None);
        assert_eq!(p.overage_resets_at, None);
        assert_eq!(p.fallback_available, None);
    }

    #[test]
    fn partial_headers_leave_missing_fields_none() {
        // Only the representative claim — no per-claim headers, no status.
        let headers = hdrs(&[(
            "anthropic-ratelimit-unified-representative-claim",
            "seven_day",
        )]);
        let p = RateLimitInfo::from_headers(&headers);
        assert_eq!(p.rate_limit_type.as_deref(), Some("seven_day"));
        assert_eq!(p.status, None);
        assert_eq!(p.resets_at, None);
        assert_eq!(p.utilization, None);
        assert_eq!(p.claim_resets_at, None);
        assert_eq!(p.overage_resets_at, None);
        assert_eq!(p.fallback_available, None);
    }

    #[test]
    fn empty_status_header_is_none_like_ts_falsy_default() {
        // TS: `headers.get('anthropic-ratelimit-unified-status') || 'allowed'`
        // (claudeAiLimits.ts:379-381) — an empty string is falsy, so the header
        // value is discarded. Our `None` is that "no usable value" state.
        let headers = hdrs(&[("anthropic-ratelimit-unified-status", "")]);
        assert_eq!(RateLimitInfo::from_headers(&headers).status, None);
    }

    #[test]
    fn malformed_utilization_is_none() {
        for bad in ["abc", "", "NaN", "inf"] {
            let headers = hdrs(&[
                (
                    "anthropic-ratelimit-unified-representative-claim",
                    "five_hour",
                ),
                ("anthropic-ratelimit-unified-5h-utilization", bad),
                ("anthropic-ratelimit-unified-5h-reset", "1750000100"),
            ]);
            let p = RateLimitInfo::from_headers(&headers);
            assert_eq!(p.utilization, None, "utilization {bad:?} should be None");
            // The sibling reset header still parses on its own.
            assert_eq!(p.claim_resets_at, Some(1_750_000_100));
        }
    }

    #[test]
    fn malformed_resets_are_none() {
        let headers = hdrs(&[
            ("anthropic-ratelimit-unified-reset", "soon"),
            (
                "anthropic-ratelimit-unified-representative-claim",
                "five_hour",
            ),
            ("anthropic-ratelimit-unified-5h-reset", ""),
            ("anthropic-ratelimit-unified-overage-reset", "-5"),
        ]);
        let p = RateLimitInfo::from_headers(&headers);
        assert_eq!(p.resets_at, None);
        assert_eq!(p.claim_resets_at, None);
        assert_eq!(p.overage_resets_at, None);
    }

    /// Per-claim headers for BOTH known windows; which one is read must follow
    /// the representative claim's abbrev.
    fn both_window_headers(claim: &str) -> Vec<(String, String)> {
        hdrs(&[
            ("anthropic-ratelimit-unified-representative-claim", claim),
            ("anthropic-ratelimit-unified-5h-utilization", "0.55"),
            ("anthropic-ratelimit-unified-5h-reset", "1750000005"),
            ("anthropic-ratelimit-unified-7d-utilization", "0.77"),
            ("anthropic-ratelimit-unified-7d-reset", "1750000007"),
        ])
    }

    #[test]
    fn five_hour_claim_reads_5h_window() {
        let p = RateLimitInfo::from_headers(&both_window_headers("five_hour"));
        assert_eq!(p.utilization, Some(0.55));
        assert_eq!(p.claim_resets_at, Some(1_750_000_005));
    }

    #[test]
    fn seven_day_claim_reads_7d_window() {
        let p = RateLimitInfo::from_headers(&both_window_headers("seven_day"));
        assert_eq!(p.utilization, Some(0.77));
        assert_eq!(p.claim_resets_at, Some(1_750_000_007));
    }

    #[test]
    fn opus_and_sonnet_claims_have_no_per_claim_headers() {
        // claude-code has NO per-claim headers for seven_day_opus /
        // seven_day_sonnet (mockRateLimits.ts:33-40 lists only 5h/7d/overage
        // variants), so no utilization is ever attached to those claims.
        for claim in ["seven_day_opus", "seven_day_sonnet"] {
            let p = RateLimitInfo::from_headers(&both_window_headers(claim));
            assert_eq!(p.rate_limit_type.as_deref(), Some(claim));
            assert_eq!(p.utilization, None, "claim {claim} must not read windows");
            assert_eq!(p.claim_resets_at, None);
        }
    }

    #[test]
    fn unknown_claim_reads_no_per_claim_headers() {
        let p = RateLimitInfo::from_headers(&both_window_headers("lunar_month"));
        assert_eq!(p.rate_limit_type.as_deref(), Some("lunar_month"));
        assert_eq!(p.utilization, None);
        assert_eq!(p.claim_resets_at, None);
    }

    #[test]
    fn overage_claim_reads_overage_window() {
        // EARLY_WARNING_CLAIM_MAP maps 'overage' → overage
        // (claudeAiLimits.ts:76); its per-claim headers are
        // `…-overage-utilization` / `…-overage-reset` (mockRateLimits.ts:27,39).
        let headers = hdrs(&[
            (
                "anthropic-ratelimit-unified-representative-claim",
                "overage",
            ),
            ("anthropic-ratelimit-unified-overage-utilization", "0.33"),
            ("anthropic-ratelimit-unified-overage-reset", "1750000009"),
        ]);
        let p = RateLimitInfo::from_headers(&headers);
        assert_eq!(p.utilization, Some(0.33));
        assert_eq!(p.claim_resets_at, Some(1_750_000_009));
        // The same header doubles as the overage window reset.
        assert_eq!(p.overage_resets_at, Some(1_750_000_009));
    }

    #[test]
    fn per_claim_headers_without_representative_claim_are_ignored() {
        let headers = hdrs(&[
            ("anthropic-ratelimit-unified-5h-utilization", "0.55"),
            ("anthropic-ratelimit-unified-5h-reset", "1750000005"),
        ]);
        let p = RateLimitInfo::from_headers(&headers);
        assert_eq!(p.utilization, None);
        assert_eq!(p.claim_resets_at, None);
    }

    #[test]
    fn claim_abbrev_mapping_is_pinned() {
        assert_eq!(claim_abbrev("five_hour"), Some("5h"));
        assert_eq!(claim_abbrev("seven_day"), Some("7d"));
        assert_eq!(claim_abbrev("overage"), Some("overage"));
        assert_eq!(claim_abbrev("seven_day_opus"), None);
        assert_eq!(claim_abbrev("seven_day_sonnet"), None);
        assert_eq!(claim_abbrev("lunar_month"), None);
        assert_eq!(claim_abbrev(""), None);
    }

    #[test]
    fn fallback_absent_available_and_other_values() {
        // Absent → None (TS collapses to `false`; `unwrap_or(false)` restores
        // the exact TS boolean).
        assert_eq!(RateLimitInfo::from_headers(&[]).fallback_available, None);
        // `=== 'available'` (claudeAiLimits.ts:384-385) → Some(true).
        let avail = hdrs(&[("anthropic-ratelimit-unified-fallback", "available")]);
        assert_eq!(
            RateLimitInfo::from_headers(&avail).fallback_available,
            Some(true)
        );
        // Present but any other value → strict-equality false → Some(false).
        for other in ["unavailable", "", "AVAILABLE", " available "] {
            let h = hdrs(&[("anthropic-ratelimit-unified-fallback", other)]);
            assert_eq!(
                RateLimitInfo::from_headers(&h).fallback_available,
                Some(false),
                "value {other:?} must be Some(false)"
            );
        }
    }
}

#[cfg(test)]
mod raw_utilization {
    //! Tests for [`RawUtilization::from_headers`], pinned against claude-code
    //! `extractRawUtilization` (`claudeAiLimits.ts:164-179`): a window needs
    //! BOTH its `-utilization` AND `-reset` headers (ts:174) — emitted
    //! atomically (both fields) or not at all.
    use super::*;

    fn h(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn raw_utilization_requires_both_headers_per_window() {
        // 5h has BOTH headers → Some; 7d has only -utilization → None
        // (`util !== null && reset !== null`, claudeAiLimits.ts:174).
        let headers = h(&[
            ("anthropic-ratelimit-unified-5h-utilization", "0.42"),
            ("anthropic-ratelimit-unified-5h-reset", "1750000005"),
            ("anthropic-ratelimit-unified-7d-utilization", "0.77"),
        ]);
        let raw = RawUtilization::from_headers(&headers);
        assert_eq!(
            raw.five_hour,
            Some(RawWindow {
                utilization: 0.42,
                resets_at: 1_750_000_005,
            })
        );
        assert_eq!(raw.seven_day, None);
    }

    #[test]
    fn raw_utilization_empty_headers_is_default() {
        let raw = RawUtilization::from_headers(&[]);
        assert_eq!(raw, RawUtilization::default());
        assert_eq!(raw.five_hour, None);
        assert_eq!(raw.seven_day, None);
    }

    #[test]
    fn raw_utilization_malformed_values_drop_window() {
        // Malformed 5h utilization drops ONLY that window (the file's
        // tolerant-parse stance; TS would store NaN — we fail soft to None);
        // the well-formed 7d window still parses.
        let headers = h(&[
            ("anthropic-ratelimit-unified-5h-utilization", "garbage"),
            ("anthropic-ratelimit-unified-5h-reset", "1750000005"),
            ("anthropic-ratelimit-unified-7d-utilization", "0.77"),
            ("anthropic-ratelimit-unified-7d-reset", "1750000007"),
        ]);
        let raw = RawUtilization::from_headers(&headers);
        assert_eq!(raw.five_hour, None);
        assert_eq!(
            raw.seven_day,
            Some(RawWindow {
                utilization: 0.77,
                resets_at: 1_750_000_007,
            })
        );

        // Malformed RESET also drops the window — atomic either-both-or-none.
        let headers = h(&[
            ("anthropic-ratelimit-unified-5h-utilization", "0.42"),
            ("anthropic-ratelimit-unified-5h-reset", "soon"),
        ]);
        let raw = RawUtilization::from_headers(&headers);
        assert_eq!(raw.five_hour, None);
        assert_eq!(raw.seven_day, None);
        assert_eq!(raw, RawUtilization::default());
    }
}

#[cfg(test)]
mod early_warning {
    //! Early-warning port tests, pinned against claude-code
    //! `getHeaderBasedEarlyWarning` (`claudeAiLimits.ts:255-294`),
    //! `getTimeRelativeEarlyWarning` (`:301-340`),
    //! `getEarlyWarningFromHeaders` (`:347-374`) and the
    //! `computeNewLimitsFromHeaders` final-status semantics (`:411-424`).
    use super::*;

    fn h(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    /// Fixed clock: `UNIX_EPOCH + secs` (the TS code reads `Date.now()/1000`).
    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    #[test]
    fn surpassed_threshold_header_forces_allowed_warning_replacement() {
        // getHeaderBasedEarlyWarning (claudeAiLimits.ts:255-294): the
        // surpassed-threshold header replaces the regular parse with a FRESH
        // allowed_warning object built from the per-claim headers.
        let headers = h(&[
            ("anthropic-ratelimit-unified-status", "allowed"),
            ("anthropic-ratelimit-unified-fallback", "available"),
            ("anthropic-ratelimit-unified-7d-surpassed-threshold", "0.5"),
            ("anthropic-ratelimit-unified-7d-utilization", "0.55"),
            ("anthropic-ratelimit-unified-7d-reset", "1750000000"),
            ("anthropic-ratelimit-unified-overage-status", "allowed"),
        ]);
        let p = RateLimitInfo::from_headers_at(&headers, at(1_000_000));
        assert_eq!(p.status.as_deref(), Some("allowed_warning"));
        assert_eq!(p.rate_limit_type.as_deref(), Some("seven_day"));
        assert_eq!(p.utilization, Some(0.55));
        assert_eq!(p.resets_at, Some(1_750_000_000));
        assert_eq!(p.surpassed_threshold, Some(0.5));
        assert_eq!(p.fallback_available, Some(true));
        // Fresh-object semantics (claudeAiLimits.ts:281-289): the replacement
        // carries NO overage fields even though the header was present.
        assert_eq!(p.overage_status, None);
    }

    #[test]
    fn claim_priority_is_5h_then_7d_then_overage() {
        // EARLY_WARNING_CLAIM_MAP iteration order (claudeAiLimits.ts:73-77,
        // :260-262): '5h' is checked first, so it wins over '7d'.
        let headers = h(&[
            ("anthropic-ratelimit-unified-status", "allowed"),
            ("anthropic-ratelimit-unified-5h-surpassed-threshold", "0.9"),
            ("anthropic-ratelimit-unified-7d-surpassed-threshold", "0.5"),
        ]);
        let p = RateLimitInfo::from_headers_at(&headers, at(1_000_000));
        assert_eq!(p.status.as_deref(), Some("allowed_warning"));
        assert_eq!(p.rate_limit_type.as_deref(), Some("five_hour"));
        assert_eq!(p.surpassed_threshold, Some(0.9));
    }

    #[test]
    fn overage_claim_surpassed_threshold_maps_to_overage_type() {
        // 'overage' → 'overage' (claudeAiLimits.ts:76).
        let headers = h(&[
            ("anthropic-ratelimit-unified-status", "allowed"),
            (
                "anthropic-ratelimit-unified-overage-surpassed-threshold",
                "0.8",
            ),
        ]);
        let p = RateLimitInfo::from_headers_at(&headers, at(1_000_000));
        assert_eq!(p.status.as_deref(), Some("allowed_warning"));
        assert_eq!(p.rate_limit_type.as_deref(), Some("overage"));
        assert_eq!(p.surpassed_threshold, Some(0.8));
    }

    #[test]
    fn surpassed_threshold_header_fires_on_presence_even_when_malformed() {
        // getHeaderBasedEarlyWarning gates on header PRESENCE alone
        // (`!== null`, claudeAiLimits.ts:268) — the value is only `Number()`ed
        // for storage (ts:288). A malformed value therefore still fires the
        // warning; our documented divergence stores `None` where TS would
        // store `NaN`.
        let headers = h(&[
            ("anthropic-ratelimit-unified-status", "allowed"),
            (
                "anthropic-ratelimit-unified-5h-surpassed-threshold",
                "garbage",
            ),
        ]);
        let p = RateLimitInfo::from_headers_at(&headers, at(1_000_000));
        assert_eq!(p.status.as_deref(), Some("allowed_warning"));
        assert_eq!(p.rate_limit_type.as_deref(), Some("five_hour"));
        assert_eq!(p.surpassed_threshold, None);
    }

    #[test]
    fn time_relative_5h_fires_at_high_utilization_early_in_window() {
        // getTimeRelativeEarlyWarning, five_hour config (claudeAiLimits.ts:54-59):
        // threshold {utilization: 0.9, timePct: 0.72}, window 18000s.
        // elapsed = 1_000_000 − (1_009_000 − 18_000) = 9_000 → progress 0.5 ≤ 0.72
        // and 0.95 ≥ 0.9 → warn.
        let headers = h(&[
            ("anthropic-ratelimit-unified-status", "allowed"),
            ("anthropic-ratelimit-unified-5h-utilization", "0.95"),
            ("anthropic-ratelimit-unified-5h-reset", "1009000"),
        ]);
        let p = RateLimitInfo::from_headers_at(&headers, at(1_000_000));
        assert_eq!(p.status.as_deref(), Some("allowed_warning"));
        assert_eq!(p.rate_limit_type.as_deref(), Some("five_hour"));
        assert_eq!(p.utilization, Some(0.95));
        assert_eq!(p.resets_at, Some(1_009_000));
        // The time-relative fresh object has NO surpassedThreshold
        // (claudeAiLimits.ts:332-339).
        assert_eq!(p.surpassed_threshold, None);
    }

    #[test]
    fn time_relative_5h_does_not_fire_late_in_window() {
        // elapsed = 1_000_000 − (1_001_000 − 18_000) = 17_000 → progress
        // ≈ 0.944 > 0.72 → no warn. The bare allowed_warning status is then
        // DOWNGRADED to allowed (claudeAiLimits.ts:423 `finalStatus = 'allowed'`).
        let headers = h(&[
            ("anthropic-ratelimit-unified-status", "allowed_warning"),
            ("anthropic-ratelimit-unified-5h-utilization", "0.95"),
            ("anthropic-ratelimit-unified-5h-reset", "1001000"),
        ]);
        let p = RateLimitInfo::from_headers_at(&headers, at(1_000_000));
        assert_eq!(p.status.as_deref(), Some("allowed"));
        assert_eq!(p.surpassed_threshold, None);
    }

    #[test]
    fn time_relative_boundaries_are_inclusive() {
        // Both comparisons are INCLUSIVE (`utilization >= t.utilization &&
        // timeProgress <= t.timePct`, claudeAiLimits.ts:324-326): utilization
        // EXACTLY 0.9 and timeProgress EXACTLY 0.72 still warn on the
        // five_hour config. Exact-representable arithmetic: window 18_000,
        // elapsed = 1_000_000 − (1_005_040 − 18_000) = 12_960 →
        // 12_960 / 18_000 == 0.72 exactly (correctly rounded quotient equals
        // the 0.72 literal).
        let headers = h(&[
            ("anthropic-ratelimit-unified-status", "allowed"),
            ("anthropic-ratelimit-unified-5h-utilization", "0.9"),
            ("anthropic-ratelimit-unified-5h-reset", "1005040"),
        ]);
        let p = RateLimitInfo::from_headers_at(&headers, at(1_000_000));
        assert_eq!(p.status.as_deref(), Some("allowed_warning"));
        assert_eq!(p.rate_limit_type.as_deref(), Some("five_hour"));
        assert_eq!(p.utilization, Some(0.9));
        assert_eq!(p.resets_at, Some(1_005_040));
        assert_eq!(p.surpassed_threshold, None);
    }

    #[test]
    fn time_relative_7d_middle_threshold() {
        // seven_day config (claudeAiLimits.ts:60-69), middle threshold
        // {utilization: 0.5, timePct: 0.35}: window 604_800s,
        // reset = 2_000_000 + (604_800 − 181_440) → elapsed 181_440 →
        // progress 0.3 ≤ 0.35 and 0.6 ≥ 0.5 → warn (utilization 0.6 < 0.75
        // keeps the first threshold from firing — `.some()` over all).
        let reset = 2_000_000 + (604_800 - 181_440);
        let headers = h(&[
            ("anthropic-ratelimit-unified-status", "allowed"),
            ("anthropic-ratelimit-unified-7d-utilization", "0.6"),
            ("anthropic-ratelimit-unified-7d-reset", &reset.to_string()),
        ]);
        let p = RateLimitInfo::from_headers_at(&headers, at(2_000_000));
        assert_eq!(p.status.as_deref(), Some("allowed_warning"));
        assert_eq!(p.rate_limit_type.as_deref(), Some("seven_day"));
        assert_eq!(p.utilization, Some(0.6));
    }

    #[test]
    fn rejected_status_passes_through_untouched_by_early_warning() {
        // computeNewLimitsFromHeaders only consults the early warning when
        // status is allowed/allowed_warning (claudeAiLimits.ts:414); 'rejected'
        // falls through to the regular parse, overage fields intact.
        let headers = h(&[
            ("anthropic-ratelimit-unified-status", "rejected"),
            ("anthropic-ratelimit-unified-5h-surpassed-threshold", "0.9"),
            ("anthropic-ratelimit-unified-overage-status", "allowed"),
        ]);
        let p = RateLimitInfo::from_headers_at(&headers, at(1_000_000));
        assert_eq!(p.status.as_deref(), Some("rejected"));
        assert_eq!(p.overage_status.as_deref(), Some("allowed"));
        assert_eq!(p.surpassed_threshold, None);
    }

    #[test]
    fn missing_status_header_stays_none_no_fabricated_allowed() {
        // Deliberate divergence-guard: TS defaults a missing status to
        // 'allowed' (claudeAiLimits.ts:379-381), but fabricating a status from
        // zero unified headers would break `has_unified_headers()` consumers.
        let p = RateLimitInfo::from_headers_at(&[], at(1_000_000));
        assert_eq!(p.status, None);
        assert_eq!(p, RateLimitInfo::default());
    }

    #[test]
    fn from_headers_delegates_with_wall_clock() {
        // The public wrapper still exists and parses with `SystemTime::now()`.
        let headers = h(&[("anthropic-ratelimit-unified-status", "rejected")]);
        let p = RateLimitInfo::from_headers(&headers);
        assert_eq!(p.status.as_deref(), Some("rejected"));
    }
}

/// Task 6 (llm-client future-work batch 5): the fresh-from-error-headers
/// rejected-limits constructor (`errors.ts:471-516`).
#[cfg(test)]
mod from_429_error_headers {
    use super::*;

    fn h(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    /// Gate (`errors.ts:480`): no representative-claim AND no overage-status
    /// → `None` (claude-code falls through to the generic 429 branch).
    #[test]
    fn gate_fails_without_unified_headers() {
        assert_eq!(RateLimitInfo::from_429_error_headers(&[]), None);
        // Other unified headers alone don't pass the gate.
        let headers = h(&[
            ("anthropic-ratelimit-unified-reset", "1760000000"),
            ("anthropic-ratelimit-unified-status", "rejected"),
        ]);
        assert_eq!(RateLimitInfo::from_429_error_headers(&headers), None);
        // Empty values are falsy in the TS gate.
        let empty = h(&[
            ("anthropic-ratelimit-unified-representative-claim", ""),
            ("anthropic-ratelimit-unified-overage-status", ""),
        ]);
        assert_eq!(RateLimitInfo::from_429_error_headers(&empty), None);
    }

    /// Full header set maps the five TS-set fields; status is FORCED
    /// 'rejected' (errors.ts:483) even when the status header says otherwise.
    #[test]
    fn full_headers_map_with_forced_rejected_status() {
        let headers = h(&[
            ("anthropic-ratelimit-unified-representative-claim", "seven_day"),
            ("anthropic-ratelimit-unified-overage-status", "rejected"),
            ("anthropic-ratelimit-unified-status", "allowed"),
            ("anthropic-ratelimit-unified-reset", "1760000000"),
            ("anthropic-ratelimit-unified-overage-reset", "1760000200"),
            (
                "anthropic-ratelimit-unified-overage-disabled-reason",
                "out_of_credits",
            ),
        ]);
        let info = RateLimitInfo::from_429_error_headers(&headers).expect("gate passes");
        assert_eq!(info.status.as_deref(), Some("rejected"), "forced (errors.ts:483)");
        assert_eq!(info.rate_limit_type.as_deref(), Some("seven_day"));
        assert_eq!(info.overage_status.as_deref(), Some("rejected"));
        assert_eq!(info.resets_at, Some(1_760_000_000));
        assert_eq!(info.overage_resets_at, Some(1_760_000_200));
        assert_eq!(info.overage_disabled_reason.as_deref(), Some("out_of_credits"));
        // Fields the TS error path never sets stay at their defaults.
        assert_eq!(info.utilization, None);
        assert_eq!(info.claim_resets_at, None);
        assert_eq!(info.fallback_available, None);
        assert_eq!(info.surpassed_threshold, None);
    }

    /// representative-claim alone passes the gate; NO early-warning
    /// replacement runs (unlike `from_headers`, the error path builds the
    /// object directly — a surpassed-threshold header is ignored).
    #[test]
    fn claim_only_passes_gate_and_skips_early_warning() {
        let headers = h(&[
            ("anthropic-ratelimit-unified-representative-claim", "five_hour"),
            ("anthropic-ratelimit-unified-5h-surpassed-threshold", "0.9"),
        ]);
        let info = RateLimitInfo::from_429_error_headers(&headers).expect("gate passes");
        assert_eq!(info.status.as_deref(), Some("rejected"));
        assert_eq!(info.rate_limit_type.as_deref(), Some("five_hour"));
        assert_eq!(info.surpassed_threshold, None, "no early-warning on the error path");
    }

    /// The constructed info composes the byte-locked rejected copy through
    /// `rate_limit_error_message` (rateLimitMessages.ts:333-344).
    #[test]
    fn composes_byte_locked_rejected_copy() {
        let headers = h(&[(
            "anthropic-ratelimit-unified-representative-claim",
            "seven_day",
        )]);
        let info = RateLimitInfo::from_429_error_headers(&headers).expect("gate passes");
        let msg = rate_limit_error_message(
            &info,
            &ResetTimes {
                reset_time: Some("3pm"),
                ..ResetTimes::default()
            },
            SubscriptionContext::default(),
        );
        assert_eq!(msg.as_deref(), Some("You've hit your weekly limit · resets 3pm"));
    }
}
