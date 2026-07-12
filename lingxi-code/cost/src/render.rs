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
}
