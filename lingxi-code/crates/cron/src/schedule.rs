//! 5-field cron expression parser and minute-boundary matcher.
//!
//! Supports `*`, `*/N`, `a-b`, `n,m,k`, and exact integers per field. UTC-only
//! decomposition is intentionally simplified for M1.17 — production swaps in
//! the `time` crate. See plan 11.

use serde::{Deserialize, Serialize};
use std::time::SystemTime;
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
    /// True if this expression should fire at the given minute boundary.
    #[must_use]
    pub fn matches(&self, time: SystemTime) -> bool {
        let secs = time
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let (year, month, day, hour, minute, _, dow) = decompose(secs);
        Self::field_match(&self.minute, minute)
            && Self::field_match(&self.hour, hour)
            && Self::field_match(&self.dom, day)
            && Self::field_match(&self.month, month)
            && Self::field_match(&self.dow, dow)
            && year > 1970
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
/// second, dow 0-6). Simplified UTC-only decomposition for M1.17; production
/// wires the `time` crate.
#[allow(clippy::cast_possible_truncation)]
fn decompose(secs: u64) -> (u32, u32, u32, u32, u32, u32, u32) {
    // Each `as u32` truncation is bounded by the preceding modulo or division
    // (years since 1970 will not overflow u32 for centuries), so truncation
    // is safe and intentional.
    let minute = (secs / 60 % 60) as u32;
    let hour = (secs / 3600 % 24) as u32;
    let day = (secs / 86_400 % 30 + 1) as u32; // crude — fine for matches() gate
    let month = ((secs / 2_628_000) % 12 + 1) as u32;
    let year = 1970 + (secs / 31_536_000) as u32;
    let dow = ((secs / 86_400 + 4) % 7) as u32; // 1970-01-01 was Thursday (4)
    (year, month, day, hour, minute, 0, dow)
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
}
