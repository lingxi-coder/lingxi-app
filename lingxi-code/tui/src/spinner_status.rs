//! The spinner status parenthetical — `formatDuration` / `formatNumber` and the
//! `(<timer> · <↑|↓> <N> tokens)` builder, 1:1 with claude-code 2.1.201's
//! `SpinnerAnimationRow.tsx` + `utils/format.ts`.
//!
//! The real spinner line is `<glyph> <verb>… (<formatDuration(elapsed)> ·
//! <mode-arrow> <formatNumber(tokens)> tokens)`, shown from the start of the
//! turn (verified live against the real 2.1.201 binary: `(3s · ↓ 111 tokens)`
//! by ~3s in). The token count is a char→token estimate:
//! `round(response_chars / 4)` (`Spinner.tsx:210`). The mode arrow is `↑` while
//! requesting (awaiting the response) and `↓` while receiving (tool-use /
//! responding / thinking). "esc to interrupt" is NOT in the spinner — it lives
//! in the status row.

/// Up-arrow (`figures.arrowUp`) — shown while requesting (awaiting the response).
pub(crate) const ARROW_UP: &str = "↑";
/// Down-arrow (`figures.arrowDown`) — shown while receiving (tool-use / responding).
pub(crate) const ARROW_DOWN: &str = "↓";

/// `formatDuration(ms)` (`utils/format.ts`): `< 60s` → `"{whole}s"` (0 → `"0s"`);
/// `>= 60s` → most-significant-first `"{d}d {h}h {m}m {s}s"` dropping any
/// leading zero units (e.g. `65_000` → `"1m 5s"`, `3_600_000` → `"1h"`).
#[must_use]
pub(crate) fn format_duration(ms: u128) -> String {
    if ms < 60_000 {
        return format!("{}s", ms / 1000);
    }
    let mut days = ms / 86_400_000;
    let mut hours = (ms % 86_400_000) / 3_600_000;
    let mut minutes = (ms % 3_600_000) / 60_000;
    // Round seconds to nearest, carrying over (mirrors Math.round + carry).
    let mut seconds = ((ms % 60_000) as f64 / 1000.0).round() as u128;
    if seconds == 60 {
        seconds = 0;
        minutes += 1;
    }
    if minutes == 60 {
        minutes = 0;
        hours += 1;
    }
    if hours == 24 {
        hours = 0;
        days += 1;
    }
    let mut parts: Vec<String> = Vec::new();
    if days > 0 {
        parts.push(format!("{days}d"));
    }
    if hours > 0 {
        parts.push(format!("{hours}h"));
    }
    if minutes > 0 {
        parts.push(format!("{minutes}m"));
    }
    if seconds > 0 {
        parts.push(format!("{seconds}s"));
    }
    if parts.is_empty() {
        "0s".to_string()
    } else {
        parts.join(" ")
    }
}

/// `formatNumber(n)` (`utils/format.ts`): `< 1000` → the bare integer; `>= 1000`
/// → compact notation with one fraction digit, lowercased (`1300` → `"1.3k"`,
/// `1000` → `"1.0k"`, `12_345` → `"12.3k"`, `1_500_000` → `"1.5m"`).
#[must_use]
pub(crate) fn format_number(n: u64) -> String {
    if n < 1000 {
        return n.to_string();
    }
    let (value, suffix) = if n >= 1_000_000_000 {
        (n as f64 / 1_000_000_000.0, "b")
    } else if n >= 1_000_000 {
        (n as f64 / 1_000_000.0, "m")
    } else {
        (n as f64 / 1000.0, "k")
    };
    // One fraction digit, matching Intl compact w/ minimumFractionDigits:1.
    format!("{value:.1}{suffix}")
}

/// The mode arrow: `↑` while requesting (awaiting the first response byte),
/// `↓` while receiving (any response/thinking delta seen, or a tool running).
#[must_use]
pub(crate) fn mode_arrow(receiving: bool) -> &'static str {
    if receiving {
        ARROW_DOWN
    } else {
        ARROW_UP
    }
}

/// Build the trailing status parenthetical for the spinner: `"(5s)"` early, or
/// `"(1m 5s · ↓ 1.3k tokens)"` once the token estimate (`chars / 4`) is `> 0`.
/// Shown from the start of the turn (the real binary shows the timer + counter
/// within a few seconds, NOT after a 30s delay). `receiving` picks the ↑/↓ arrow.
#[must_use]
pub(crate) fn status_paren(elapsed_ms: u128, response_chars: u64, receiving: bool) -> String {
    let mut parts: Vec<String> = vec![format_duration(elapsed_ms)];
    let tokens = (response_chars as f64 / 4.0).round() as u64;
    if tokens > 0 {
        parts.push(format!(
            "{} {} tokens",
            mode_arrow(receiving),
            format_number(tokens)
        ));
    }
    format!("({})", parts.join(" · "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_duration_matches_reference() {
        assert_eq!(format_duration(0), "0s");
        assert_eq!(format_duration(5_000), "5s");
        assert_eq!(format_duration(59_000), "59s");
        assert_eq!(format_duration(60_000), "1m");
        assert_eq!(format_duration(65_000), "1m 5s");
        assert_eq!(format_duration(3_600_000), "1h");
        assert_eq!(format_duration(3_665_000), "1h 1m 5s");
        assert_eq!(format_duration(86_400_000), "1d");
    }

    #[test]
    fn format_number_matches_reference() {
        assert_eq!(format_number(0), "0");
        assert_eq!(format_number(900), "900");
        assert_eq!(format_number(999), "999");
        assert_eq!(format_number(1000), "1.0k");
        assert_eq!(format_number(1300), "1.3k");
        assert_eq!(format_number(12_345), "12.3k");
        assert_eq!(format_number(1_500_000), "1.5m");
    }

    #[test]
    fn status_paren_shows_from_the_start_timer_only_when_no_tokens() {
        // Timer from 0s (no 30s gate); no tokens yet → timer only.
        assert_eq!(status_paren(0, 0, false), "(0s)");
        assert_eq!(status_paren(3_000, 0, false), "(3s)");
    }

    #[test]
    fn status_paren_timer_and_tokens_with_arrow() {
        // 3s, 444 chars → 111 tokens, receiving → ↓ (matches the live capture).
        assert_eq!(status_paren(3_000, 444, true), "(3s · ↓ 111 tokens)");
        // 40s, 5200 chars → 1300 tokens → "1.3k".
        assert_eq!(status_paren(40_000, 5200, true), "(40s · ↓ 1.3k tokens)");
        // requesting (not yet receiving) → ↑.
        assert_eq!(status_paren(40_000, 5200, false), "(40s · ↑ 1.3k tokens)");
        // Over a minute → formatDuration "1m 5s".
        assert_eq!(status_paren(65_000, 400, true), "(1m 5s · ↓ 100 tokens)");
    }
}
