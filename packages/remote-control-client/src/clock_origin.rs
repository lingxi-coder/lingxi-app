use std::time::{Duration, Instant};

pub(super) fn with_margin(now: Instant, margin: Duration) -> Instant {
    now.checked_sub(margin).unwrap_or(now)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn falls_back_to_current_time_when_margin_precedes_clock_origin() {
        let now = Instant::now();
        assert!(now.checked_sub(Duration::MAX).is_none());
        assert_eq!(with_margin(now, Duration::MAX), now);
    }

    #[test]
    fn retains_the_margin_when_representable() {
        let now = Instant::now();
        let margin = Duration::from_millis(1);
        if let Some(expected) = now.checked_sub(margin) {
            assert_eq!(with_margin(now, margin), expected);
        }
    }
}
