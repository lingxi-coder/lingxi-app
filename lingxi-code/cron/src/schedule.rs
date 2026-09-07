//! 5-field cron expression parser and minute-boundary matcher.
//!
//! 1:1 with claude-code `utils/cron.ts` `parseCronExpression` / `expandField`:
//! every field is expanded at parse time into the sorted set of concrete values
//! it matches (`*`, `*/N`, `a-b`, `a-b/N`, `n`, and comma lists of those, all
//! range-checked; day-of-week `7` is Sunday). Live matching uses the platform's
//! local UTC offset (including DST); the calendar decomposition core stays
//! UTC-pure so tests can inject a deterministic offset.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::time::{Duration, SystemTime};
use thiserror::Error;

/// Parsed 5-field cron expression: minute hour day-of-month month day-of-week.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronExpression {
    /// Original input string, kept for diagnostics and round-tripping.
    pub raw: String,
    /// Minute field (0-59).
    pub minute: CronField,
    /// Hour field (0-23).
    pub hour: CronField,
    /// Day-of-month field (1-31).
    pub dom: CronField,
    /// Month field (1-12).
    pub month: CronField,
    /// Day-of-week field (0-6, Sunday = 0; `7` is accepted as Sunday).
    pub dow: CronField,
}

/// One parsed cron field, stored the way claude-code stores it: the sorted set
/// of concrete values the field matches (`expandField` → `number[]`), already
/// range-checked and normalised. `*/N` steps from the field's MINIMUM (so
/// day-of-month `*/5` is 1,6,11,…, not 5,10,…), exactly like the TS
/// `for (let v = min; v <= max; v += step)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CronField {
    values: Vec<u32>,
}

impl CronField {
    /// The sorted, de-duplicated values this field matches.
    #[must_use]
    pub fn values(&self) -> &[u32] {
        &self.values
    }

    /// Whether `value` is one of the matched values.
    #[must_use]
    pub fn contains(&self, value: u32) -> bool {
        self.values.binary_search(&value).is_ok()
    }

    /// Whether the field covers EVERY value of its domain `[lo, hi]` — the
    /// "wild" test for the cron OR day rule. claude-code detects this from the
    /// expanded set's length (`dayOfMonth.length === 31`, `dayOfWeek.length === 7`).
    #[must_use]
    pub fn is_wild(&self, lo: u32, hi: u32) -> bool {
        u32::try_from(self.values.len()).is_ok_and(|len| len == hi - lo + 1)
    }
}

/// Errors returned by [`parse_cron`].
#[derive(Debug, Clone, Error)]
pub enum CronParseError {
    /// Input did not split into exactly 5 fields.
    #[error("expected 5 fields, got {0}")]
    FieldCount(usize),
    /// One of the fields could not be parsed or has an out-of-range value.
    #[error("invalid field: {0}")]
    BadField(String),
}

/// Inclusive domain of one cron field (claude-code `FIELD_RANGES`).
#[derive(Clone, Copy)]
struct FieldRange {
    min: u32,
    max: u32,
}

/// minute, hour, day-of-month, month, day-of-week.
const FIELD_RANGES: [FieldRange; 5] = [
    FieldRange { min: 0, max: 59 },
    FieldRange { min: 0, max: 23 },
    FieldRange { min: 1, max: 31 },
    FieldRange { min: 1, max: 12 },
    FieldRange { min: 0, max: 6 },
];

/// Parse a 5-field cron expression (claude-code `parseCronExpression`: trim,
/// split on whitespace, exactly five fields, each expanded by [`expand_field`]).
pub fn parse_cron(s: &str) -> Result<CronExpression, CronParseError> {
    let parts: Vec<&str> = s.split_whitespace().collect();
    if parts.len() != 5 {
        return Err(CronParseError::FieldCount(parts.len()));
    }
    let mut fields = parts.iter().zip(FIELD_RANGES).map(|(part, range)| expand_field(part, range));
    let mut next = || fields.next().expect("five fields");
    Ok(CronExpression {
        raw: s.to_string(),
        minute: next()?,
        hour: next()?,
        dom: next()?,
        month: next()?,
        dow: next()?,
    })
}

/// 1:1 port of `expandField` in claude-code `utils/cron.ts`. Each comma part is
/// independently one of: a wildcard / step (`*`, `*/N`), a range or stepped
/// range (`N-M`, `N-M/S`), or a single value (`N`). Mixed forms compose freely
/// in one field (e.g. `1-5,0`, `0-30/10,45`, `*/15,7`). Day-of-week `7` is
/// normalised to `0` (Sunday) — both as a bare value and at the high end of a
/// range. Any value outside the field's `[min, max]` (with `7` allowed for
/// day-of-week), a zero step, an empty part, or any other syntax makes the
/// whole field invalid (TS returns `null`).
fn expand_field(field: &str, range: FieldRange) -> Result<CronField, CronParseError> {
    let FieldRange { min, max } = range;
    let is_dow = min == 0 && max == 6;
    let bad = || CronParseError::BadField(field.to_string());
    let mut out: BTreeSet<u32> = BTreeSet::new();

    for part in field.split(',') {
        // `*` or `*/N`  (regex `^\*(?:\/(\d+))?$`)
        if let Some(step) = match_star(part) {
            if step < 1 {
                return Err(bad());
            }
            let mut i = min;
            while i <= max {
                out.insert(i);
                i = i.saturating_add(step);
            }
            continue;
        }

        // `N-M` or `N-M/S`  (regex `^(\d+)-(\d+)(?:\/(\d+))?$`)
        if let Some((lo, hi, step)) = match_range(part) {
            let eff_max = if is_dow { 7 } else { max };
            if lo > hi || step < 1 || lo < min || hi > eff_max {
                return Err(bad());
            }
            let mut i = lo;
            while i <= hi {
                out.insert(if is_dow && i == 7 { 0 } else { i });
                i = i.saturating_add(step);
            }
            continue;
        }

        // plain `N`  (regex `^\d+$`)
        if let Some(mut n) = parse_digits(part) {
            if is_dow && n == 7 {
                n = 0;
            }
            if n < min || n > max {
                return Err(bad());
            }
            out.insert(n);
            continue;
        }

        return Err(bad());
    }

    if out.is_empty() {
        return Err(bad());
    }
    Ok(CronField {
        values: out.into_iter().collect(),
    })
}

/// `^\d+$` → the number (JS `parseInt` on an ASCII-digit run). Values beyond
/// `u32` saturate, which — like the TS `for` loop with a huge step — yields
/// only the field minimum for `*/N` and an out-of-range rejection otherwise.
fn parse_digits(part: &str) -> Option<u32> {
    if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(part.parse::<u32>().unwrap_or(u32::MAX))
}

/// Match `*` or `*/N`, returning the step (`1` for a bare `*`).
fn match_star(part: &str) -> Option<u32> {
    if part == "*" {
        return Some(1);
    }
    parse_digits(part.strip_prefix("*/")?)
}

/// Match `N-M` or `N-M/S`, returning `(lo, hi, step)` with `step` defaulting to `1`.
fn match_range(part: &str) -> Option<(u32, u32, u32)> {
    let (range_part, step) = match part.split_once('/') {
        Some((range_part, step)) => (range_part, parse_digits(step)?),
        None => (part, 1),
    };
    let (lo, hi) = range_part.split_once('-')?;
    Some((parse_digits(lo)?, parse_digits(hi)?, step))
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
        let dom_wild = self.dom.is_wild(1, 31);
        let dow_wild = self.dow.is_wild(0, 6);
        let day_matches = match (dom_wild, dow_wild) {
            (true, true) => true,
            (false, true) => self.dom.contains(day),
            (true, false) => self.dow.contains(dow),
            (false, false) => self.dom.contains(day) || self.dow.contains(dow),
        };
        self.minute.contains(minute)
            && self.hour.contains(hour)
            && self.month.contains(month)
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
}

/// claude-code `K_(cron)` — the human-readable schedule string (local branch):
/// `Every minute` / `Every N minutes` / `Every hour[ at :MM]` / `Every N hours[ at :MM]` /
/// `Every day at h:MM AM` / `Every <Weekday> at …` / `Weekdays at …`, else the raw
/// expression. Time formatting mirrors en-US `toLocaleTimeString({hour:"numeric",
/// minute:"2-digit"})`.
#[must_use]
pub fn human_schedule(cron: &str) -> String {
    const DAY_NAMES: [&str; 7] = [
        "Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday",
    ];
    fn all_digits(s: &str) -> bool {
        !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
    }
    fn step(s: &str) -> Option<u32> {
        let rest = s.strip_prefix("*/")?;
        all_digits(rest).then(|| rest.parse().ok()).flatten()
    }
    fn time(minute: u32, hour: u32) -> String {
        let period = if hour < 12 { "AM" } else { "PM" };
        let h12 = match hour % 12 {
            0 => 12,
            h => h,
        };
        format!("{h12}:{minute:02} {period}")
    }
    let parts: Vec<&str> = cron.trim().split_whitespace().collect();
    if parts.len() != 5 {
        return cron.to_string();
    }
    let (minute, hour, dom, month, dow) = (parts[0], parts[1], parts[2], parts[3], parts[4]);
    if hour == "*" && dom == "*" && month == "*" && dow == "*" {
        if minute == "*" {
            return "Every minute".to_string();
        }
        if let Some(n) = step(minute) {
            return if n == 1 {
                "Every minute".to_string()
            } else {
                format!("Every {n} minutes")
            };
        }
    }
    if all_digits(minute) && hour == "*" && dom == "*" && month == "*" && dow == "*" {
        let m: u32 = minute.parse().unwrap_or(0);
        if m == 0 {
            return "Every hour".to_string();
        }
        return format!("Every hour at :{m:02}");
    }
    if all_digits(minute) {
        if let Some(n) = step(hour) {
            if dom == "*" && month == "*" && dow == "*" {
                let m: u32 = minute.parse().unwrap_or(0);
                let suffix = if m == 0 {
                    String::new()
                } else {
                    format!(" at :{m:02}")
                };
                return if n == 1 {
                    format!("Every hour{suffix}")
                } else {
                    format!("Every {n} hours{suffix}")
                };
            }
        }
    }
    if !all_digits(minute) || !all_digits(hour) {
        return cron.to_string();
    }
    let m: u32 = minute.parse().unwrap_or(0);
    let h: u32 = hour.parse().unwrap_or(0);
    let at = time(m, h);
    if dom == "*" && month == "*" && dow == "*" {
        return format!("Every day at {at}");
    }
    if dom == "*" && month == "*" && dow.len() == 1 && all_digits(dow) {
        let index = dow.parse::<usize>().unwrap_or(0) % 7;
        if let Some(name) = DAY_NAMES.get(index) {
            return format!("Every {name} at {at}");
        }
    }
    if dom == "*" && month == "*" && dow == "1-5" {
        return format!("Weekdays at {at}");
    }
    cron.to_string()
}

/// JS `new Date(ms).toLocaleString()` in the en-US default form the binary
/// prints into the missed-task prompt: `M/D/YYYY, h:mm:ss AM`, local time.
#[must_use]
pub fn local_date_time_string(epoch_ms: u64) -> String {
    let secs = i64::try_from(epoch_ms / 1000).unwrap_or(i64::MAX);
    let local = secs.saturating_add(local_offset_seconds(secs)).max(0) as u64;
    let (year, month, day, hour, minute, _, _) = decompose(local);
    let second = local % 60;
    let period = if hour < 12 { "AM" } else { "PM" };
    let h12 = match hour % 12 {
        0 => 12,
        h => h,
    };
    format!("{month}/{day}/{year}, {h12}:{minute:02}:{second:02} {period}")
}

/// JS `new Date(ms).toISOString()` — `YYYY-MM-DDTHH:MM:SS.mmmZ`.
#[must_use]
pub fn iso_8601_utc(epoch_ms: u64) -> String {
    let (year, month, day, hour, minute, _, _) = decompose(epoch_ms / 1000);
    let second = (epoch_ms / 1000) % 60;
    let millis = epoch_ms % 1000;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
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

/// Windows local UTC offset at `unix_secs`, including the historical DST rule
/// Windows applies to that instant. `SystemTimeToTzSpecificLocalTimeEx` with a
/// null timezone pointer selects the machine's current dynamic timezone.
#[cfg(windows)]
pub fn local_offset_seconds(unix_secs: i64) -> i64 {
    use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows_sys::Win32::System::Time::{
        FileTimeToSystemTime, SystemTimeToFileTime, SystemTimeToTzSpecificLocalTimeEx,
    };

    const WINDOWS_EPOCH_OFFSET_SECS: i128 = 11_644_473_600;
    const TICKS_PER_SECOND: i128 = 10_000_000;
    let ticks = (i128::from(unix_secs) + WINDOWS_EPOCH_OFFSET_SECS)
        .checked_mul(TICKS_PER_SECOND)
        .and_then(|ticks| u64::try_from(ticks).ok());
    let Some(ticks) = ticks else {
        return 0;
    };
    let utc_file = FILETIME {
        dwLowDateTime: ticks as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    };

    // SAFETY: all pointers refer to initialized stack values with the exact
    // Win32 ABI layouts. Each API returns zero on conversion failure; no
    // pointers escape the call.
    unsafe {
        let mut utc_system: SYSTEMTIME = std::mem::zeroed();
        if FileTimeToSystemTime(&utc_file, &mut utc_system) == 0 {
            return 0;
        }
        let mut local_system: SYSTEMTIME = std::mem::zeroed();
        if SystemTimeToTzSpecificLocalTimeEx(std::ptr::null(), &utc_system, &mut local_system) == 0
        {
            return 0;
        }
        let mut local_file: FILETIME = std::mem::zeroed();
        if SystemTimeToFileTime(&local_system, &mut local_file) == 0 {
            return 0;
        }
        let local_ticks =
            (u64::from(local_file.dwHighDateTime) << 32) | u64::from(local_file.dwLowDateTime);
        let delta = i128::from(local_ticks) - i128::from(ticks);
        i64::try_from(delta / TICKS_PER_SECOND).unwrap_or(0)
    }
}

/// Platforms without either POSIX `localtime_r` or Win32 timezone APIs retain
/// the UTC fallback. Current desktop targets never use this branch.
#[cfg(not(any(unix, windows)))]
pub fn local_offset_seconds(_unix_secs: i64) -> i64 {
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(expr: &str, pick: fn(&CronExpression) -> &CronField) -> Vec<u32> {
        pick(&parse_cron(expr).unwrap()).values().to_vec()
    }

    #[test]
    fn parses_basic_expression() {
        let c = parse_cron("*/5 9-17 * * 1-5").unwrap();
        assert_eq!(c.minute.values(), &[0, 5, 10, 15, 20, 25, 30, 35, 40, 45, 50, 55]);
        assert_eq!(c.hour.values(), &[9, 10, 11, 12, 13, 14, 15, 16, 17]);
        assert!(c.dom.is_wild(1, 31));
        assert_eq!(c.dom.values().len(), 31);
        assert_eq!(c.dow.values(), &[1, 2, 3, 4, 5]);
        assert!(!c.dow.is_wild(0, 6));
    }

    // PARITY: claude-code `expandField` steps from the field MINIMUM. For
    // day-of-month and month (min 1) `*/N` therefore starts at 1, not at N.
    #[test]
    fn step_starts_at_field_minimum() {
        assert_eq!(values("0 0 */5 * *", |c| &c.dom), vec![1, 6, 11, 16, 21, 26, 31]);
        assert_eq!(values("0 0 * */2 *", |c| &c.month), vec![1, 3, 5, 7, 9, 11]);
        assert_eq!(values("0 */7 * * *", |c| &c.hour), vec![0, 7, 14, 21]);
        // 2023-11-16 00:00 UTC (dom 16) matches `*/5` on dom; the 15th does not.
        let expr = parse_cron("0 0 */5 * *").unwrap();
        assert!(expr.matches_at_offset(at(1_700_092_800), 0));
        assert!(!expr.matches_at_offset(at(1_700_006_400), 0));
    }

    #[test]
    fn accepts_stepped_ranges_mixed_lists_and_sunday_alias() {
        assert_eq!(values("0-30/10,45 * * * *", |c| &c.minute), vec![0, 10, 20, 30, 45]);
        assert_eq!(values("*/15,7 * * * *", |c| &c.minute), vec![0, 7, 15, 30, 45]);
        assert_eq!(values("0 0 * * 1-5,0", |c| &c.dow), vec![0, 1, 2, 3, 4, 5]);
        assert_eq!(values("0 0 * * 7", |c| &c.dow), vec![0]);
        assert_eq!(values("0 0 * * 5-7", |c| &c.dow), vec![0, 5, 6]);
        // A range covering the whole domain is wild, exactly like `*`.
        assert!(parse_cron("0 0 1-31 * *").unwrap().dom.is_wild(1, 31));
        assert!(parse_cron("0 0 * * 0-7").unwrap().dow.is_wild(0, 6));
        assert!(parse_cron("0 0 * * 1-7").unwrap().dow.is_wild(0, 6));
    }

    #[test]
    fn rejects_out_of_range_zero_step_and_malformed_parts() {
        for bad in [
            "60 * * * *",
            "0 24 * * *",
            "0 0 0 * *",
            "0 0 32 * *",
            "0 0 * 13 *",
            "0 0 * * 8",
            "*/0 * * * *",
            "5-3 * * * *",
            "1,,2 * * * *",
            "0 0 * * 6-8",
            "a * * * *",
            "0 0 * * mon",
            "1-5/x * * * *",
        ] {
            assert!(
                matches!(parse_cron(bad), Err(CronParseError::BadField(_))),
                "{bad} must be rejected"
            );
        }
        // Whitespace is trimmed and collapsed before splitting (TS `trim().split(/\s+/)`).
        assert!(parse_cron("  0   9  *  *  *  ").is_ok());
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
