//! Compact number formatting for token counts (claude-code `formatNumber`):
//! `< 1000` → as-is; `>= 1000` → `1.2k`; `>= 1_000_000` → `1.2M`.

/// Format a token count compactly. Drops a trailing `.0`.
#[must_use]
pub fn format_token_count(n: u64) -> String {
    if n < 1000 {
        return n.to_string();
    }
    if n < 1_000_000 {
        let v = (n as f64) / 1000.0;
        return trim_one_decimal(v, 'k');
    }
    let v = (n as f64) / 1_000_000.0;
    trim_one_decimal(v, 'M')
}

fn trim_one_decimal(v: f64, suffix: char) -> String {
    // One decimal, but drop `.0`.
    let s = format!("{v:.1}");
    let s = s.strip_suffix(".0").map_or(s.clone(), ToString::to_string);
    format!("{s}{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(format_token_count(0), "0");
        assert_eq!(format_token_count(512), "512");
        assert_eq!(format_token_count(1000), "1k");
        assert_eq!(format_token_count(1200), "1.2k");
        assert_eq!(format_token_count(15_300), "15.3k");
        assert_eq!(format_token_count(1_000_000), "1M");
        assert_eq!(format_token_count(2_500_000), "2.5M");
    }
}
