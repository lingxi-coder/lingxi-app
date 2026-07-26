//! The ←-on-empty gesture and its editing guard (claude-code `W_p` / `G_p`,
//! 2.1.220, telemetry flag `tengu_left_arrow_editing_guard`).
//!
//! Pressing ← on an EMPTY composer is a gesture, not a cursor move: it opens
//! the agents view (or, in an attached background session, detaches). The
//! problem is that ← is also how you leave a character you just deleted — a
//! user who backspaces the last character of a draft and taps ← expects the
//! cursor to move, not the screen to change under them.
//!
//! So the gesture is guarded: if the composer became empty by EDITING within
//! the last two seconds, the first ← only ARMS the gesture and shows
//! "Press ← again"; a second ← within three seconds fires it. Outside that
//! window the first press fires immediately, because there is no recent edit
//! for it to be confused with.
//!
//! Two other cases matter:
//!
//! - **Key repeat.** Holding ← must not arm-then-immediately-fire. A press
//!   within one second of the previous one is ABSORBED, which also means a
//!   held key can never walk through the arm→fire sequence on its own.
//! - **Not a solo keypress.** A ← that arrived in the same read as other bytes
//!   (a paste, an escape sequence) is REJECTED and falls through to an ordinary
//!   cursor move; a gesture must come from a deliberate, isolated keystroke.
//!
//! Every timestamp comparison goes through the same validity test — a stamp
//! must be non-zero AND at least the attach stamp — so state carried across an
//! attach cannot be mistaken for a press in the current session.

/// Feedback timeout for the "Press ← again" hint, ms (claude `MJs`).
pub const FEEDBACK_TIMEOUT_MS: u64 = 3000;
/// A press within this many ms of the previous one is key repeat (claude `q_p`).
pub const KEY_REPEAT_MS: u64 = 1000;
/// Minimum dwell before an attach-armed press may fire (claude `QGy`).
pub const ATTACH_CONFIRM_MIN_MS: u64 = 150;
/// How long an armed gesture stays armed, ms.
pub const ARMED_WINDOW_MS: u64 = 3000;
/// How long after becoming empty by editing the guard applies, ms.
pub const EDITED_EMPTY_WINDOW_MS: u64 = 2000;

/// The hint shown when the gesture arms (claude's default for
/// `leftArrowConfirmHint`).
pub const CONFIRM_HINT: &str = "Press \u{2190} again";
/// The hint shown when an ATTACHED session arms.
pub const ATTACH_CONFIRM_HINT: &str = "Ambiguous \u{2190}, press again to detach";

/// What a ← press on an empty composer should do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeftArrowAction {
    /// Run the gesture.
    Fire,
    /// Show the confirm hint and wait for a second press.
    Arm,
    /// Swallow the press (key repeat) — no gesture, no cursor move.
    Absorb,
    /// Not a solo keypress: fall through to an ordinary cursor move.
    Reject,
    /// Attached session: show the detach hint and wait.
    AttachArm,
    /// Attached session: swallow the press.
    AttachAbsorb,
}

impl LeftArrowAction {
    /// The `tengu_left_arrow_blocked` reason this action reports, if any.
    /// `Fire` and `Absorb` report nothing — one is the gesture running, the
    /// other is key repeat, and neither is a "block" worth counting.
    #[must_use]
    pub fn blocked_reason(self) -> Option<&'static str> {
        match self {
            Self::Arm => Some("editing-quiet"),
            Self::AttachArm => Some("attach-quiet-hint"),
            Self::AttachAbsorb => Some("attach-quiet"),
            Self::Reject => Some("not-solo"),
            Self::Fire | Self::Absorb => None,
        }
    }
}

/// The gesture's timestamps (claude `j_p()`'s initial object). All zero means
/// "nothing has happened yet"; zero is never a valid stamp.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LeftArrowState {
    /// When the composer last became empty by editing.
    pub edited_empty_at_ms: u64,
    /// When the gesture was armed.
    pub armed_at_ms: u64,
    /// When ← was last pressed.
    pub last_left_press_ms: u64,
    /// When the attached-session confirm was armed.
    pub attach_confirm_armed_at_ms: u64,
}

/// Inputs the decision reads from the host.
#[derive(Debug, Clone, Copy)]
pub struct LeftArrowInputs {
    /// Now, in ms.
    pub now_ms: u64,
    /// Whether this ← arrived alone rather than inside a paste / escape
    /// sequence.
    pub solo_keypress: bool,
    /// The `tengu_left_arrow_editing_guard` flag (default ON). With it off the
    /// gesture always fires on the first press.
    pub guard_enabled: bool,
    /// Whether the session is inside the post-attach quiet window.
    ///
    /// Claude 2.1.220 hard-codes its probe to `false` (`DJr(t){return !1}`), so
    /// the two attach arms are unreachable there too. Modeled anyway because
    /// they are part of the state machine's contract, and the port's
    /// background-attach path is where a `true` would come from.
    pub in_attach_quiet_window: bool,
    /// The attach stamp (claude `Kke()`, currently always 0). A stored stamp
    /// older than this is from before the attach and does not count.
    pub attach_stamp_ms: u64,
}

/// Decide what a ← on an empty composer does (claude `W_p`).
#[must_use]
pub fn decide_left_arrow(state: &LeftArrowState, inputs: &LeftArrowInputs) -> LeftArrowAction {
    use LeftArrowAction::{Absorb, Arm, AttachArm, AttachAbsorb, Fire, Reject};

    // A ← that came in with other bytes is not a gesture.
    if !inputs.solo_keypress {
        return Reject;
    }
    // A stamp counts only if it was set AND set in this attach epoch.
    let valid = |stamp: u64| stamp != 0 && stamp >= inputs.attach_stamp_ms;
    let since = |stamp: u64| inputs.now_ms.saturating_sub(stamp);

    if inputs.in_attach_quiet_window {
        if valid(state.last_left_press_ms) && since(state.last_left_press_ms) < KEY_REPEAT_MS {
            return AttachAbsorb;
        }
        if valid(state.attach_confirm_armed_at_ms)
            && since(state.attach_confirm_armed_at_ms) <= ARMED_WINDOW_MS
        {
            // A minimum dwell, so the second half of a fast double-tap cannot
            // detach the session the user only just attached.
            return if since(state.attach_confirm_armed_at_ms) >= ATTACH_CONFIRM_MIN_MS {
                Fire
            } else {
                AttachAbsorb
            };
        }
        return AttachArm;
    }

    // Guard off ⇒ the gesture is unconditional.
    if !inputs.guard_enabled {
        return Fire;
    }
    // Key repeat: swallow, and (via the state update) keep swallowing, so a
    // held key can never walk itself through arm → fire.
    if valid(state.last_left_press_ms) && since(state.last_left_press_ms) < KEY_REPEAT_MS {
        return Absorb;
    }
    // Already armed and still within the window ⇒ this is the confirmation.
    if valid(state.armed_at_ms) && since(state.armed_at_ms) <= ARMED_WINDOW_MS {
        return Fire;
    }
    // Only a RECENT edit-to-empty makes the press ambiguous. Otherwise the
    // composer has simply been empty, and there is nothing to confuse it with.
    if valid(state.edited_empty_at_ms) && since(state.edited_empty_at_ms) < EDITED_EMPTY_WINDOW_MS {
        Arm
    } else {
        Fire
    }
}

/// Fold an action back into the state (claude `G_p`).
///
/// `Reject` and `AttachAbsorb` deliberately record NOTHING: a rejected press
/// was not a gesture at all, and an attach-absorbed one must not refresh the
/// repeat window it was absorbed by.
pub fn apply_left_arrow(state: &mut LeftArrowState, action: LeftArrowAction, now_ms: u64) {
    match action {
        LeftArrowAction::Fire => {
            state.armed_at_ms = 0;
            state.attach_confirm_armed_at_ms = 0;
            state.last_left_press_ms = now_ms;
        }
        LeftArrowAction::Arm => {
            state.armed_at_ms = now_ms;
            state.last_left_press_ms = now_ms;
        }
        LeftArrowAction::Absorb => state.last_left_press_ms = now_ms,
        LeftArrowAction::AttachArm => state.attach_confirm_armed_at_ms = now_ms,
        LeftArrowAction::Reject | LeftArrowAction::AttachAbsorb => {}
    }
}

impl LeftArrowState {
    /// Record that the composer just became empty by EDITING (claude `Ce()` and
    /// the `text !== "" && next === ""` transition).
    ///
    /// This is what makes the very next ← ambiguous, so it must be stamped on
    /// the edit that empties the composer — not on every keystroke while it is
    /// already empty, which would keep the guard armed forever.
    pub fn note_edited_to_empty(&mut self, now_ms: u64) {
        self.edited_empty_at_ms = now_ms;
    }

    /// Clear an armed gesture (claude `Ee()`), because the user did something
    /// else. Returns whether a hint was showing and should be taken down.
    pub fn disarm(&mut self) -> bool {
        if self.armed_at_ms == 0 {
            return false;
        }
        self.armed_at_ms = 0;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs(now_ms: u64) -> LeftArrowInputs {
        LeftArrowInputs {
            now_ms,
            solo_keypress: true,
            guard_enabled: true,
            in_attach_quiet_window: false,
            attach_stamp_ms: 0,
        }
    }

    /// The common case: the composer has just been sitting empty, so the very
    /// first ← runs the gesture. Requiring two presses here would tax every
    /// ordinary use of it.
    #[test]
    fn a_long_empty_composer_fires_on_the_first_press() {
        let mut s = LeftArrowState::default();
        let i = inputs(10_000);
        assert_eq!(decide_left_arrow(&s, &i), LeftArrowAction::Fire);
        apply_left_arrow(&mut s, LeftArrowAction::Fire, i.now_ms);
        assert_eq!(s.last_left_press_ms, 10_000);
    }

    /// The whole point of the guard: backspacing the last character and tapping
    /// ← must not yank the screen away. The first press only arms.
    #[test]
    fn a_recent_edit_to_empty_arms_instead_of_firing() {
        let mut s = LeftArrowState::default();
        s.note_edited_to_empty(10_000);
        let i = inputs(10_500);
        assert_eq!(decide_left_arrow(&s, &i), LeftArrowAction::Arm);
        apply_left_arrow(&mut s, LeftArrowAction::Arm, i.now_ms);

        // The confirming press fires, and clears the armed state.
        let i2 = inputs(12_000);
        assert_eq!(decide_left_arrow(&s, &i2), LeftArrowAction::Fire);
        apply_left_arrow(&mut s, LeftArrowAction::Fire, i2.now_ms);
        assert_eq!(s.armed_at_ms, 0);
    }

    /// The edit-to-empty window is bounded: two seconds later the edit is no
    /// longer what the user is thinking about, so the press fires directly.
    #[test]
    fn an_old_edit_to_empty_no_longer_arms() {
        let mut s = LeftArrowState::default();
        s.note_edited_to_empty(10_000);
        assert_eq!(
            decide_left_arrow(&s, &inputs(10_000 + EDITED_EMPTY_WINDOW_MS)),
            LeftArrowAction::Fire
        );
        // …and just inside it still arms.
        assert_eq!(
            decide_left_arrow(&s, &inputs(10_000 + EDITED_EMPTY_WINDOW_MS - 1)),
            LeftArrowAction::Arm
        );
    }

    /// An armed gesture expires. Pressing ← again much later starts over
    /// rather than firing on a confirmation the user has forgotten giving.
    #[test]
    fn an_armed_gesture_expires_after_the_window() {
        let mut s = LeftArrowState::default();
        s.note_edited_to_empty(10_000);
        apply_left_arrow(&mut s, LeftArrowAction::Arm, 10_100);
        // Past the armed window AND past the edit window ⇒ fires as a fresh
        // first press rather than as a stale confirmation.
        assert_eq!(
            decide_left_arrow(&s, &inputs(10_100 + ARMED_WINDOW_MS + 1)),
            LeftArrowAction::Fire
        );
    }

    /// Holding ← must not arm-then-fire on its own. Repeat presses are
    /// absorbed, and each absorb refreshes the window, so the whole hold stays
    /// absorbed however long it lasts.
    #[test]
    fn a_held_key_is_absorbed_and_never_walks_itself_to_fire() {
        let mut s = LeftArrowState::default();
        s.note_edited_to_empty(10_000);
        let mut now = 10_500;
        assert_eq!(decide_left_arrow(&s, &inputs(now)), LeftArrowAction::Arm);
        apply_left_arrow(&mut s, LeftArrowAction::Arm, now);

        // ~30ms repeat for two seconds: every one absorbed.
        for _ in 0..60 {
            now += 30;
            let a = decide_left_arrow(&s, &inputs(now));
            assert_eq!(a, LeftArrowAction::Absorb, "at {now}");
            apply_left_arrow(&mut s, a, now);
        }
        // A deliberate press after the repeat stops does confirm.
        now += KEY_REPEAT_MS + 1;
        assert_eq!(decide_left_arrow(&s, &inputs(now)), LeftArrowAction::Fire);
    }

    /// A ← arriving with other bytes (paste, escape sequence) is not a
    /// gesture — it falls through to an ordinary cursor move, and records
    /// nothing so it cannot influence a later real press.
    #[test]
    fn a_non_solo_keypress_is_rejected_and_records_nothing() {
        let mut s = LeftArrowState::default();
        let mut i = inputs(10_000);
        i.solo_keypress = false;
        assert_eq!(decide_left_arrow(&s, &i), LeftArrowAction::Reject);
        let before = s;
        apply_left_arrow(&mut s, LeftArrowAction::Reject, i.now_ms);
        assert_eq!(s, before, "a rejected press leaves no trace");
    }

    /// With the guard flag off the gesture is unconditional, even right after
    /// an edit to empty.
    #[test]
    fn the_guard_flag_disables_the_confirmation() {
        let mut s = LeftArrowState::default();
        s.note_edited_to_empty(10_000);
        let mut i = inputs(10_100);
        i.guard_enabled = false;
        assert_eq!(decide_left_arrow(&s, &i), LeftArrowAction::Fire);
    }

    /// Zero is never a valid stamp — a default state must not read as "edited
    /// to empty at time 0" and arm on the first press.
    #[test]
    fn a_zero_stamp_is_not_a_press() {
        let s = LeftArrowState::default();
        assert_eq!(decide_left_arrow(&s, &inputs(500)), LeftArrowAction::Fire);
    }

    /// A stamp from BEFORE the attach does not count: state carried across an
    /// attach must not be read as activity in the current session.
    #[test]
    fn stamps_older_than_the_attach_are_ignored() {
        let mut s = LeftArrowState::default();
        s.note_edited_to_empty(1_000);
        let mut i = inputs(1_500);
        i.attach_stamp_ms = 1_200;
        assert_eq!(
            decide_left_arrow(&s, &i),
            LeftArrowAction::Fire,
            "a pre-attach edit stamp cannot arm the guard"
        );
    }

    /// Attached: the first press arms with the detach hint, and a confirmation
    /// after the minimum dwell detaches.
    #[test]
    fn an_attached_session_arms_then_detaches() {
        let mut s = LeftArrowState::default();
        let mut i = inputs(10_000);
        i.in_attach_quiet_window = true;
        assert_eq!(decide_left_arrow(&s, &i), LeftArrowAction::AttachArm);
        apply_left_arrow(&mut s, LeftArrowAction::AttachArm, i.now_ms);

        // Too soon: the second half of a fast double-tap must not detach.
        let mut early = inputs(10_000 + ATTACH_CONFIRM_MIN_MS - 1);
        early.in_attach_quiet_window = true;
        assert_eq!(decide_left_arrow(&s, &early), LeftArrowAction::AttachAbsorb);

        let mut ok = inputs(10_000 + ATTACH_CONFIRM_MIN_MS);
        ok.in_attach_quiet_window = true;
        assert_eq!(decide_left_arrow(&s, &ok), LeftArrowAction::Fire);
    }

    /// An attach-absorbed press records nothing, so it cannot refresh the very
    /// repeat window that absorbed it.
    #[test]
    fn an_attach_absorb_records_nothing() {
        let mut s = LeftArrowState::default();
        s.attach_confirm_armed_at_ms = 10_000;
        let before = s;
        apply_left_arrow(&mut s, LeftArrowAction::AttachAbsorb, 10_050);
        assert_eq!(s, before);
    }

    #[test]
    fn blocked_reasons_are_byte_locked() {
        assert_eq!(LeftArrowAction::Arm.blocked_reason(), Some("editing-quiet"));
        assert_eq!(
            LeftArrowAction::AttachArm.blocked_reason(),
            Some("attach-quiet-hint")
        );
        assert_eq!(
            LeftArrowAction::AttachAbsorb.blocked_reason(),
            Some("attach-quiet")
        );
        assert_eq!(LeftArrowAction::Reject.blocked_reason(), Some("not-solo"));
        assert_eq!(LeftArrowAction::Fire.blocked_reason(), None);
        assert_eq!(LeftArrowAction::Absorb.blocked_reason(), None);
    }

    #[test]
    fn hints_and_timings_are_byte_locked() {
        assert_eq!(CONFIRM_HINT, "Press ← again");
        assert_eq!(ATTACH_CONFIRM_HINT, "Ambiguous ←, press again to detach");
        assert_eq!(FEEDBACK_TIMEOUT_MS, 3000);
        assert_eq!(KEY_REPEAT_MS, 1000);
        assert_eq!(ATTACH_CONFIRM_MIN_MS, 150);
    }

    /// `disarm` reports whether a hint was up, so the caller knows whether to
    /// take one down; disarming twice must not claim a second removal.
    #[test]
    fn disarm_reports_only_the_first_time() {
        let mut s = LeftArrowState::default();
        assert!(!s.disarm(), "nothing armed ⇒ nothing to take down");
        s.armed_at_ms = 10_000;
        assert!(s.disarm());
        assert!(!s.disarm());
    }
}
