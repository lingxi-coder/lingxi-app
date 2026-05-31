//! Duration + exit-code formatting for task progress.
//!
//! Literal lock: claude-code `src/utils/format.ts` `formatDuration` and
//! `ShellDetailDialog.tsx` exit-code display.

/// Format elapsed milliseconds (claude-code `formatDuration`): seconds under a
/// minute (`12s`), then `Nm`/`Nm Ss`, `Nh`/`Nh Mm`, `Nd`/`Nd Hh`.
#[must_use]
pub fn format_duration(ms: u64) -> String {
    let secs = ms / 1000;
    if secs < 60 {
        return format!("{secs}s");
    }
    let (mins, rem_s) = (secs / 60, secs % 60);
    if mins < 60 {
        return if rem_s == 0 {
            format!("{mins}m")
        } else {
            format!("{mins}m {rem_s}s")
        };
    }
    let (hours, rem_m) = (mins / 60, mins % 60);
    if hours < 24 {
        return if rem_m == 0 {
            format!("{hours}h")
        } else {
            format!("{hours}h {rem_m}m")
        };
    }
    let (days, rem_h) = (hours / 24, hours % 24);
    if rem_h == 0 {
        format!("{days}d")
    } else {
        format!("{days}d {rem_h}h")
    }
}

/// Format an exit code for the detail status line (claude-code `exit code: N`).
#[must_use]
pub fn format_exit_code(code: i32) -> String {
    format!("exit code: {code}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_seconds() {
        assert_eq!(format_duration(0), "0s");
        assert_eq!(format_duration(12_000), "12s");
        assert_eq!(format_duration(59_000), "59s");
    }

    #[test]
    fn duration_minutes() {
        assert_eq!(format_duration(60_000), "1m");
        assert_eq!(format_duration(63_000), "1m 3s");
        assert_eq!(format_duration(150_000), "2m 30s");
    }

    #[test]
    fn duration_hours_days() {
        assert_eq!(format_duration(3_600_000), "1h");
        assert_eq!(format_duration(3_660_000), "1h 1m");
        assert_eq!(format_duration(86_400_000), "1d");
        assert_eq!(format_duration(97_200_000), "1d 3h");
    }

    #[test]
    fn exit_code() {
        assert_eq!(format_exit_code(0), "exit code: 0");
        assert_eq!(format_exit_code(1), "exit code: 1");
    }
}
