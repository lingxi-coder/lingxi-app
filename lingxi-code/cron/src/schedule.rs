//! 5-field cron expression parser and minute-boundary matcher.
//!
//! Supports `*`, `*/N`, `a-b`, `n,m,k`, and exact integers per field. UTC-only
//! decomposition is intentionally simplified for M1.17 — production swaps in
//! the `time` crate. See plan 11.

use serde::{Deserialize, Serialize};
use std::time::{Duration, SystemTime};
use thiserror::Error;

/// Parsed 5-field cron expression: minute hour day-of-month month day-of-week.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronExpression {
    /// Original input string, kept for diagnostics and round-tripping.
    pub raw: String,
    /// Minute field (0-59 range; not range-validated at parse time).
    pub minute: CronField,
    /// Hour field (0-23).
    pub hour: CronField,
    /// Day-of-month field (1-31).
    pub dom: CronField,
    /// Month field (1-12).
    pub month: CronField,
    /// Day-of-week field (0-6, Sunday = 0).
    pub dow: CronField,
}

/// One parsed cron field — see [`CronExpression`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CronField {
    /// `*` — any value matches.
    Any,
    /// `n` — only this exact value matches.
    Exact(u32),
    /// `*/N` — value matches when `value % N == 0`.
    Step(u32),
    /// `a-b` — inclusive numeric range.
    Range(u32, u32),
    /// `a,b,c` — explicit list of values.
    List(Vec<u32>),
}

/// Errors returned by [`parse_cron`].
#[derive(Debug, Clone, Error)]
pub enum CronParseError {
    /// Input did not split into exactly 5 fields.
    #[error("expected 5 fields, got {0}")]
    FieldCount(usize),
    /// One of the fields could not be parsed.
    #[error("invalid field: {0}")]
    BadField(String),
}

/// Parse a 5-field cron expression.
pub fn parse_cron(s: &str) -> Result<CronExpression, CronParseError> {
    let parts: Vec<&str> = s.split_whitespace().collect();
    if parts.len() != 5 {
        return Err(CronParseError::FieldCount(parts.len()));
    }
    Ok(CronExpression {
        raw: s.to_string(),
        minute: parse_field(parts[0])?,
        hour: parse_field(parts[1])?,
        dom: parse_field(parts[2])?,
        month: parse_field(parts[3])?,
        dow: parse_field(parts[4])?,
    })
}

fn parse_field(s: &str) -> Result<CronField, CronParseError> {
    if s == "*" {
        return Ok(CronField::Any);
    }
    if let Some(rest) = s.strip_prefix("*/") {
        let n = rest
            .parse::<u32>()
            .map_err(|_| CronParseError::BadField(s.into()))?;
        return Ok(CronField::Step(n));
    }
    if let Some((a, b)) = s.split_once('-') {
        let a = a
            .parse::<u32>()
            .map_err(|_| CronParseError::BadField(s.into()))?;
        let b = b
            .parse::<u32>()
            .map_err(|_| CronParseError::BadField(s.into()))?;
        return Ok(CronField::Range(a, b));
    }
    if s.contains(',') {
        let list: Vec<u32> = s
            .split(',')
            .map(str::parse::<u32>)
            .collect::<Result<_, _>>()
            .map_err(|_| CronParseError::BadField(s.into()))?;
        return Ok(CronField::List(list));
    }
    s.parse::<u32>()
        .map(CronField::Exact)
        .map_err(|_| CronParseError::BadField(s.into()))
}

impl CronExpression {
    /// True if this expression should fire at the given minute boundary, in the
    /// process's LOCAL timezone (DST-aware per-instant via [`local_offset_seconds`])
    /// — 1:1 with claude-code, whose cron is "in the user's local timezone"
    /// (`ScheduleCronTool/prompt.ts:89`, `CronCreateTool.ts:32` "in local time").
    #[must_use]
    pub fn matches(&self, time: SystemTime) -> bool {
        let secs = time
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        self.matches_at_offset(time, local_offset_seconds(i64::try_from(secs).unwrap_or(0)))
    }

    /// Pure offset-parameterized core of [`Self::matches`]: evaluate the cron
    /// fields against `time` shifted by `offset_secs` (the local UTC offset in
    /// seconds; `0` = UTC). Deterministic — reads no timezone state — so it is
    /// the unit-test seam for the field/day-rule logic.
    #[must_use]
    #[allow(clippy::cast_possible_wrap, clippy::cast_sign_loss)]
    fn matches_at_offset(&self, time: SystemTime, offset_secs: i64) -> bool {
        let secs = time
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        // Shift the UTC instant by the local offset, then decompose on the real
        // calendar. A negative shifted value (pre-epoch local time) clamps to 0.
        let local = (secs as i64).saturating_add(offset_secs).max(0) as u64;
        let (year, month, day, hour, minute, _, dow) = decompose(local);
        // Standard cron day rule: when BOTH day-of-month and day-of-week are
        // restricted, the day matches if EITHER matches (OR); when one is `*`
        // (wild), only the other constrains. Mirrors claude-code `cron.ts`
        // (dom/dow length===31 / ===7 wild detection + `domSet || dowSet`).
        let dom_wild = Self::field_is_wild(&self.dom, 1, 31);
        let dow_wild = Self::field_is_wild(&self.dow, 0, 6);
        let day_matches = match (dom_wild, dow_wild) {
            (true, true) => true,
            (false, true) => Self::field_match(&self.dom, day),
            (true, false) => Self::field_match(&self.dow, dow),
            (false, false) => {
                Self::field_match(&self.dom, day) || Self::field_match(&self.dow, dow)
            }
        };
        Self::field_match(&self.minute, minute)
            && Self::field_match(&self.hour, hour)
            && Self::field_match(&self.month, month)
            && day_matches
            && year > 1970
    }

    /// The smallest minute boundary STRICTLY after `from` at which this
    /// expression matches, searching up to ~366 days ahead. `None` if no match
    /// is found within that horizon (e.g. an impossible expression like
    /// `0 0 30 2 *`).
    ///
    /// The missed-run CATCH-UP primitive: a recurring job is due when the next
    /// run after its anchor (last-fire, or creation if never fired) has already
    /// passed (`next_match_after(anchor) <= now`), so a run missed while the
    /// scheduler was down fires once on the next tick. The strictly-after
    /// property is what prevents a double-fire: once a run fires and the anchor
    /// advances to it, that same run is never matched again. Cost is bounded by
    /// the schedule's PERIOD for any real schedule (the first match after the
    /// anchor is within one period), not by how old the anchor is.
    #[must_use]
    pub fn next_match_after(&self, from: SystemTime) -> Option<SystemTime> {
        // Per-candidate local offset (DST-aware): the offset can change across a
        // DST boundary within the 366-day horizon, so each candidate resolves its
        // own offset — 1:1 with claude-code's per-instant `Date` evaluation.
        self.next_match_after_with(from, |secs| {
            local_offset_seconds(i64::try_from(secs).unwrap_or(0))
        })
    }

    /// Offset-closure core of [`Self::next_match_after`]: `offset_for(candidate_secs)`
    /// supplies the UTC offset for each candidate minute. Deterministic when the
    /// closure is (e.g. `|_| 0` for UTC) — the unit-test seam.
    pub(crate) fn next_match_after_with<F: Fn(u64) -> i64>(
        &self,
        from: SystemTime,
        offset_for: F,
    ) -> Option<SystemTime> {
        let from_secs = from
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .ok()?;
        // Start at the next whole minute strictly after `from`.
        let start_minute = from_secs / 60 + 1;
        const HORIZON_MINUTES: u64 = 366 * 24 * 60;
        (0..HORIZON_MINUTES).find_map(|m| {
            let cand_secs = (start_minute + m) * 60;
            let candidate = SystemTime::UNIX_EPOCH + Duration::from_secs(cand_secs);
            self.matches_at_offset(candidate, offset_for(cand_secs))
                .then_some(candidate)
        })
    }

    /// Whether `field` matches EVERY value in the inclusive domain `[lo, hi]` —
    /// the "wild" test for the cron OR rule. `*` is wild; so is a range/step/list
    /// that covers the whole domain (TS detects this via expanded-set length
    /// === 31 (dom) / === 7 (dow)).
    fn field_is_wild(field: &CronField, lo: u32, hi: u32) -> bool {
        match field {
            CronField::Any => true,
            CronField::Step(n) => *n == 1,
            CronField::Range(a, b) => *a <= lo && *b >= hi,
            CronField::Exact(_) => lo == hi,
            CronField::List(list) => (lo..=hi).all(|v| list.contains(&v)),
        }
    }

    fn field_match(field: &CronField, value: u32) -> bool {
        match field {
            CronField::Any => true,
            CronField::Exact(v) => *v == value,
            CronField::Step(n) => *n > 0 && value % n == 0,
            CronField::Range(a, b) => value >= *a && value <= *b,
            CronField::List(list) => list.contains(&value),
        }
    }
}

/// Decompose unix seconds into (year, month 1-12, day 1-31, hour, minute,
/// second, dow 0-6 Sun=0), UTC, on the real proleptic Gregorian calendar.
///
/// Real month lengths (incl. leap-year Feb 29 and the 31-day months) come from
/// Howard Hinnant's public-domain `civil_from_days` algorithm — integer-only, no
/// new dependency. This fixes the old fixed-30-day approximation under which
/// day-of-month 31 could NEVER match and the month drifted off the calendar.
///
/// This is the pure UTC calendar core. LOCAL-time evaluation (claude-code's
/// behavior — cron runs "in the user's local timezone") is layered ON TOP by
/// [`CronExpression::matches`] / [`CronExpression::next_match_after`], which
/// shift the instant by [`local_offset_seconds`] before calling this. Keeping
/// `decompose` UTC-pure makes the calendar math deterministically testable.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss
)]
fn decompose(secs: u64) -> (u32, u32, u32, u32, u32, u32, u32) {
    let minute = (secs / 60 % 60) as u32;
    let hour = (secs / 3600 % 24) as u32;
    let days = (secs / 86_400) as i64; // days since 1970-01-01 (a Thursday)
    let dow = ((days % 7 + 4) % 7) as u32; // 1970-01-01 = Thursday (4)

    // civil_from_days: days-since-epoch -> (year, month[1..=12], day[1..=31]).
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    let year = (y + i64::from(month <= 2)) as u32;

    (year, month, day, hour, minute, 0, dow)
}

/// The local UTC offset (seconds; e.g. `-28800` for PST) in effect at the given
/// unix instant — DST-aware, read from the system timezone database via
/// `libc::localtime_r`'s `tm_gmtoff`. `local_time = utc_time + offset`. No new
/// dependency: `libc` is already a `cron` dep. Falls back to `0` (UTC) when the
/// conversion fails.
///
/// `pub` so the sibling `tool-cron` crate (which mirrors this calendar logic for
/// its `next_fire_unix_secs` preview) reuses the SAME local-time source rather
/// than duplicating the `unsafe` FFI under its own lint policy.
#[cfg(unix)]
#[allow(clippy::cast_possible_truncation)]
pub fn local_offset_seconds(unix_secs: i64) -> i64 {
    // SAFETY: `localtime_r` reads `*t` and writes the broken-down time into our
    // stack `tm`; both pointers are valid for the call. A null return signals
    // failure → fall back to UTC (offset 0).
    unsafe {
        let t = unix_secs as libc::time_t;
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() {
            return 0;
        }
        // `tm_gmtoff` is `c_long` (i64 on LP64, i32 on ILP32); `try_from` widens
        // cleanly on both without a cast lint.
        i64::try_from(tm.tm_gmtoff).unwrap_or(0)
    }
}

/// Non-Unix fallback: the platform `libc::tm` has no `tm_gmtoff`, so cron
/// evaluates in UTC there (documented divergence; local-tz on Windows is a
/// follow-up via `GetTimeZoneInformation`).
#[cfg(not(unix))]
pub fn local_offset_seconds(_unix_secs: i64) -> i64 {
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_expression() {
        let c = parse_cron("*/5 9-17 * * 1-5").unwrap();
        assert!(matches!(c.minute, CronField::Step(5)));
        assert!(matches!(c.hour, CronField::Range(9, 17)));
        assert!(matches!(c.dom, CronField::Any));
    }

    #[test]
    fn rejects_too_few_fields() {
        assert!(matches!(
            parse_cron("* * *"),
            Err(CronParseError::FieldCount(3))
        ));
    }

    /// `secs` for a known UTC instant: 2023-11-14 22:13:20 UTC, a Tuesday.
    const TUE_2023_11_14: u64 = 1_700_000_000;

    fn at(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs)
    }

    #[test]
    fn decompose_uses_real_calendar() {
        let (y, m, d, h, min, _, dow) = decompose(TUE_2023_11_14);
        assert_eq!((y, m, d, h, min, dow), (2023, 11, 14, 22, 13, 2));
    }

    #[test]
    fn day_of_month_31_can_match() {
        // 2023-12-31 00:00:00 UTC = 1703980800. DOM 31 must match (the old
        // fixed-30-day decompose made this impossible).
        let dec31 = 1_703_980_800;
        let (_, m, d, ..) = decompose(dec31);
        assert_eq!((m, d), (12, 31));
        assert!(parse_cron("0 0 31 * *")
            .unwrap()
            .matches_at_offset(at(dec31), 0));
    }

    #[test]
    fn leap_day_feb_29() {
        // 2024-02-29 00:00:00 UTC = 1709164800.
        let feb29 = 1_709_164_800;
        let (y, m, d, ..) = decompose(feb29);
        assert_eq!((y, m, d), (2024, 2, 29));
    }

    #[test]
    fn dom_and_dow_are_ored_when_both_restricted() {
        // `0 0 1 * 2` = midnight on the 1st OR on any Tuesday. 2023-11-14 is a
        // Tuesday (not the 1st) → must still match via the DOW branch.
        let expr = parse_cron("0 0 1 * 2").unwrap();
        // Force minute/hour to match: 2023-11-14 00:00:00 UTC = 1699920000 (Tue).
        let tue_midnight = 1_699_920_000;
        assert_eq!(decompose(tue_midnight).6, 2); // dow == Tue
        assert!(expr.matches_at_offset(at(tue_midnight), 0));
        // And the 1st of a month that is NOT a Tuesday also matches (DOM branch):
        // 2023-11-01 00:00:00 UTC = 1698796800 (a Wednesday, dow==3).
        let nov1 = 1_698_796_800;
        assert_eq!(decompose(nov1).2, 1); // day == 1
        assert!(expr.matches_at_offset(at(nov1), 0));
    }

    #[test]
    fn restricted_dom_with_wild_dow_does_not_or() {
        // `0 0 15 * *` = only the 15th (dow is `*` → wild, no OR). 2023-11-14 is
        // the 14th → must NOT match.
        let expr = parse_cron("0 0 15 * *").unwrap();
        let nov14_midnight = 1_699_920_000;
        assert!(!expr.matches_at_offset(at(nov14_midnight), 0));
    }

    // ── next_match_after (missed-run catch-up primitive) ──────────────────

    #[test]
    fn next_match_after_every_minute() {
        let expr = parse_cron("* * * * *").unwrap();
        let next = expr
            .next_match_after_with(at(TUE_2023_11_14), |_| 0)
            .unwrap();
        assert_eq!(next, at((TUE_2023_11_14 / 60 + 1) * 60));
    }

    #[test]
    fn next_match_after_is_strictly_after() {
        // Even when `from` is exactly ON a matching minute, the result is the
        // NEXT one — this strictly-after property prevents catch-up double-fires.
        let expr = parse_cron("* * * * *").unwrap();
        assert_eq!(
            expr.next_match_after_with(at(1_700_000_040), |_| 0)
                .unwrap(),
            at(1_700_000_100)
        );
    }

    #[test]
    fn next_match_after_daily_rolls_to_next_day() {
        // `0 9 * * *` (09:00 UTC). From 2023-11-14 22:13 → 2023-11-15 09:00.
        let expr = parse_cron("0 9 * * *").unwrap();
        let next = expr
            .next_match_after_with(at(TUE_2023_11_14), |_| 0)
            .unwrap();
        let (y, m, d, h, min, ..) = decompose(
            next.duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        );
        assert_eq!((y, m, d, h, min), (2023, 11, 15, 9, 0));
    }

    #[test]
    fn next_match_after_impossible_expression_is_none() {
        // Feb 30 never occurs → no match within the ~366-day horizon.
        let expr = parse_cron("0 0 30 2 *").unwrap();
        assert_eq!(expr.next_match_after_with(at(TUE_2023_11_14), |_| 0), None);
    }

    // ── local-timezone evaluation (claude-code parity) ────────────────────

    #[test]
    fn matches_at_offset_evaluates_in_shifted_local_time() {
        // "0 9 * * *" = 09:00 LOCAL. 2023-11-14 09:00:00 UTC = 1_699_952_400
        // (= the test's `nov14_midnight` 1_699_920_000 + 9h).
        let expr = parse_cron("0 9 * * *").unwrap();
        let nine_utc = 1_699_952_400;
        // Offset 0 (UTC): 09:00 UTC matches; 10:00 UTC does not.
        assert!(expr.matches_at_offset(at(nine_utc), 0));
        assert!(!expr.matches_at_offset(at(nine_utc + 3600), 0));
        // A −1h offset shifts 10:00 UTC → 09:00 local → matches; and 09:00 UTC →
        // 08:00 local → no match. This is how a local-time cron actually fires.
        assert!(expr.matches_at_offset(at(nine_utc + 3600), -3600));
        assert!(!expr.matches_at_offset(at(nine_utc), -3600));
    }

    #[test]
    fn local_offset_seconds_is_a_valid_utc_offset() {
        // Exercises the libc `localtime_r`/`tm_gmtoff` path; the result must be a
        // real-world UTC offset (−12:00 Baker Island … +14:00 Kiribati) on any
        // host timezone (it is exactly 0 on a UTC host / CI).
        let off = local_offset_seconds(i64::try_from(TUE_2023_11_14).unwrap());
        assert!(
            (-12 * 3600..=14 * 3600).contains(&off),
            "implausible UTC offset {off}"
        );
    }
}
