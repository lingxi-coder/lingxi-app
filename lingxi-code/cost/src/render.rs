//! Byte-exact port of claude-code 2.1.206's cost-summary renderers
//! (`i6e`/`qs`/`cbg`/`FTu`/`Bu`). The whole block is rendered dimmed by the
//! consumer; these functions return plain strings.

/// Duration formatter — port of claude-code `qs(ms)` (no options). `ms` is an
/// integer, so the float sub-millisecond `.toFixed(1)` branch (`e < 1`) reduces
/// to the `e === 0` case already handled here.
#[must_use]
pub fn format_duration_ms(ms: u64) -> String {
    if ms < 60_000 {
        return format!("{}s", ms / 1000);
    }
    let mut r = ms / 86_400_000;
    let mut n = (ms % 86_400_000) / 3_600_000;
    let mut o = (ms % 3_600_000) / 60_000;
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let mut i = ((ms % 60_000) as f64 / 1000.0).round() as u64;
    if i == 60 {
        i = 0;
        o += 1;
    }
    if o == 60 {
        o = 0;
        n += 1;
    }
    if n == 24 {
        n = 0;
        r += 1;
    }
    if r > 0 {
        return format!("{r}d {n}h {o}m");
    }
    if n > 0 {
        return format!("{n}h {o}m {i}s");
    }
    if o > 0 {
        return format!("{o}m {i}s");
    }
    if i > 0 {
        return format!("{i}s");
    }
    "0s".to_string()
}

/// Cost formatter — port of claude-code `FTu(usd, 4)`: `> $0.50` renders 2
/// decimals (rounded to cents via `dbg(e,100)=round(e*100)/100`); otherwise 4.
#[must_use]
pub fn format_cost(usd: f64) -> String {
    if usd > 0.5 {
        let cents = (usd * 100.0).round() / 100.0;
        format!("${cents:.2}")
    } else {
        format!("${usd:.4}")
    }
}

/// Token-count formatter — port of claude-code `Bu` (Intl compact notation,
/// lowercased, `maximumFractionDigits: 1`, trailing `.0` dropped). Identical
/// output to the port's existing `format_tokens`.
#[must_use]
pub fn format_token_count(n: u64) -> String {
    const UNITS: [(u64, char); 4] = [
        (1_000_000_000_000, 't'),
        (1_000_000_000, 'b'),
        (1_000_000, 'm'),
        (1_000, 'k'),
    ];
    for &(threshold, suffix) in &UNITS {
        if n >= threshold {
            #[allow(clippy::cast_precision_loss)]
            let rounded = ((n as f64 / threshold as f64) * 10.0).round() / 10.0;
            let s = format!("{rounded:.1}");
            let s = s.strip_suffix(".0").unwrap_or(&s);
            return format!("{s}{suffix}");
        }
    }
    n.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qs_matches_cc() {
        assert_eq!(format_duration_ms(0), "0s");
        assert_eq!(format_duration_ms(500), "0s");     // <1s floors to 0
        assert_eq!(format_duration_ms(1_500), "1s");   // floor(1.5)
        assert_eq!(format_duration_ms(59_000), "59s");
        assert_eq!(format_duration_ms(60_000), "1m 0s");
        assert_eq!(format_duration_ms(65_000), "1m 5s");
        assert_eq!(format_duration_ms(3_661_000), "1h 1m 1s");
        assert_eq!(format_duration_ms(90_061_000), "1d 1h 1m"); // days form drops seconds
        assert_eq!(format_duration_ms(59_500), "59s");  // floor(59.5)=59
        assert_eq!(format_duration_ms(119_500), "2m 0s"); // round(59.5s)=60 -> carry: 1m -> 2m 0s
    }

    #[test]
    fn ftu_matches_cc() {
        assert_eq!(format_cost(0.0), "$0.0000");
        assert_eq!(format_cost(0.05), "$0.0500");
        assert_eq!(format_cost(0.5), "$0.5000");        // not > 0.5
        assert_eq!(format_cost(0.5001), "$0.50");       // > 0.5 -> 2dp rounded
        assert_eq!(format_cost(1.2345), "$1.23");
        assert_eq!(format_cost(12.999), "$13.00");
    }

    #[test]
    fn bu_matches_cc_compact() {
        assert_eq!(format_token_count(0), "0");
        assert_eq!(format_token_count(999), "999");
        assert_eq!(format_token_count(1_000), "1k");
        assert_eq!(format_token_count(1_500), "1.5k");
        assert_eq!(format_token_count(12_345), "12.3k");
        assert_eq!(format_token_count(50_000), "50k");
        assert_eq!(format_token_count(1_000_000), "1m");
        assert_eq!(format_token_count(1_500_000), "1.5m");
    }
}
