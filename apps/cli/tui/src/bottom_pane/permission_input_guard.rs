//! Claude Code 2.1.318's permission input grace windows. The window starts
//! when the prompt is drawn, so a slow render cannot consume an older key.
//!
//! Artifact `qEe` (296713xxx) uses `OO = 150` for a newly shown/displaced
//! dialog and `aJ = 600` after releasing typed composer input. Escape uses
//! the shorter window. The selector's refusal tracker extends the short
//! window while a burst of stale confirmation keys is still arriving.

use std::cell::Cell;
use std::time::{Duration, Instant};

const SHOWN_GRACE: Duration = Duration::from_millis(150);
const SUBMIT_GRACE: Duration = Duration::from_millis(600);

#[derive(Default)]
pub(super) struct PermissionInputGuard {
    shown_at: Cell<Option<Instant>>,
    submitted_at: Cell<Option<Instant>>,
    refused_at: Cell<Option<Instant>>,
}

impl PermissionInputGuard {
    pub(super) fn new(submitted_at: Option<Instant>) -> Self {
        Self {
            submitted_at: Cell::new(submitted_at),
            ..Self::default()
        }
    }

    pub(super) fn note_presented(&self, now: Instant) {
        if self.shown_at.get().is_none() {
            self.shown_at.set(Some(now));
        }
    }

    pub(super) fn note_submission_presented(&self, now: Instant) {
        if let Some(submitted) = self.submitted_at.get() {
            self.submitted_at.set(Some(submitted.max(now)));
        }
    }

    pub(super) fn may_answer(&self, now: Instant, escape: bool) -> bool {
        let within = |at: Option<Instant>, grace| {
            at.is_some_and(|at| now.saturating_duration_since(at) < grace)
        };
        if self.shown_at.get().is_none()
            || within(self.shown_at.get(), SHOWN_GRACE)
            || within(
                self.submitted_at.get(),
                if escape { SHOWN_GRACE } else { SUBMIT_GRACE },
            )
            || (!escape && within(self.refused_at.get(), SHOWN_GRACE))
        {
            // Native's Escape predicate does not pass through the selector's
            // refusal tracker. Holding Escape cannot keep cancellation locked.
            if !escape {
                self.refused_at.set(Some(now));
            }
            return false;
        }
        true
    }

    #[cfg(test)]
    pub(crate) fn settle_for_test(&self) {
        // Lifecycle tests drive keys directly without a terminal. Explicitly
        // model an already-painted, quiet prompt; protection tests below use
        // the real state machine and controlled timestamps.
        self.shown_at.set(Some(Instant::now() - SUBMIT_GRACE));
        self.submitted_at.set(None);
        self.refused_at.set(None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unpainted_prompt_cannot_answer_even_after_a_slow_mount() {
        let now = Instant::now();
        let guard = PermissionInputGuard::default();
        assert!(!guard.may_answer(now, false));
        guard.note_presented(now + Duration::from_secs(30));
        assert!(!guard.may_answer(now + Duration::from_millis(30_149), false));
        assert!(guard.may_answer(now + Duration::from_millis(30_299), false));
    }

    #[test]
    fn redraws_do_not_restart_the_window_but_repeated_keys_do() {
        let now = Instant::now();
        let guard = PermissionInputGuard::default();
        guard.note_presented(now);
        guard.note_presented(now + Duration::from_millis(100));
        assert!(!guard.may_answer(now + Duration::from_millis(149), false));
        assert!(!guard.may_answer(now + Duration::from_millis(298), false));
        assert!(guard.may_answer(now + Duration::from_millis(448), false));
    }

    #[test]
    fn a_fast_prompt_keeps_the_six_hundred_ms_submit_window() {
        let now = Instant::now();
        let guard = PermissionInputGuard::new(Some(now));
        guard.note_presented(now + Duration::from_millis(10));
        assert!(!guard.may_answer(now + Duration::from_millis(160), false));
        assert!(guard.may_answer(now + SUBMIT_GRACE, false));
    }

    #[test]
    fn slow_release_frame_does_not_consume_the_submit_window() {
        let now = Instant::now();
        let guard = PermissionInputGuard::new(Some(now));
        let drawn = now + Duration::from_secs(1);
        guard.note_submission_presented(drawn);
        guard.note_presented(drawn);
        assert!(!guard.may_answer(drawn + SHOWN_GRACE, false));
        assert!(guard.may_answer(drawn + SUBMIT_GRACE, false));
    }

    #[test]
    fn escape_uses_the_shorter_submit_window() {
        let now = Instant::now();
        let guard = PermissionInputGuard::new(Some(now));
        guard.note_presented(now);
        assert!(guard.may_answer(now + SHOWN_GRACE, true));
        assert!(!guard.may_answer(now + SHOWN_GRACE, false));
    }

    #[test]
    fn repeated_escape_does_not_extend_the_initial_window() {
        let now = Instant::now();
        let guard = PermissionInputGuard::new(Some(now));
        guard.note_presented(now);
        for ms in [0, 50, 100, 149] {
            assert!(!guard.may_answer(now + Duration::from_millis(ms), true));
        }
        // A rejected confirmation key also cannot extend Escape's window.
        assert!(!guard.may_answer(now + Duration::from_millis(149), false));
        for ms in [150, 200, 250] {
            assert!(guard.may_answer(now + Duration::from_millis(ms), true));
        }
        assert!(!guard.may_answer(now + Duration::from_millis(250), false));
    }
}
