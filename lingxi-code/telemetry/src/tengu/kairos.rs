//! `tengu_kairos_*` event name constants — the `/loop` (Kairos) autonomous-loop
//! subsystem.
//!
//! Like [`workflow`](crate::tengu::workflow) these are NOT added to the
//! count-locked `ALL_EVENT_NAMES` / `tengu_events.json` fixture (that snapshot is
//! from an OLDER claude event set; adding these would break the 347-entry
//! byte-parity lock). They live here for string-lock testing only.
//!
//! Binary: `pJr()` (`logAutonomousLoopActivation`, cc_all.txt:504950) emits
//! `W("tengu_kairos_loop_persistent_activated",{variant})`.

/// `tengu_kairos_loop_persistent_activated` — the autonomous-loop default was
/// activated by the `/loop` command (or a fire-time resolver). `variant` carries
/// `isLoopPersistentPreambleEnabled()`.
pub const LOOP_PERSISTENT_ACTIVATED: &str = "tengu_kairos_loop_persistent_activated";

/// Every reachable kairos telemetry event name (string-lock only, NOT in
/// `ALL_EVENT_NAMES`).
pub const NAMES: &[&str] = &[LOOP_PERSISTENT_ACTIVATED];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_tengu_kairos_prefixed() {
        for n in NAMES {
            assert!(n.starts_with("tengu_kairos_"), "{n} must be tengu_kairos_*");
        }
    }

    #[test]
    fn loop_persistent_activated_is_byte_exact() {
        // PARITY: binary pJr() event name (cc_all.txt:504950).
        assert_eq!(LOOP_PERSISTENT_ACTIVATED, "tengu_kairos_loop_persistent_activated");
    }
}
