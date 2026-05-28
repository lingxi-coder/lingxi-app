//! `tengu_command_<name>_<phase>` event names — the M5-10 batch-1 surface.
//!
//! M5-10 ships 18 events (6 commands × 3 phases). M5-11 will append another
//! 36 to this same module (12 batch-2 commands × 3). See plan
//! `docs/superpowers/plans/2026-05-25-m5-10-commands-batch-1.md` Task 1.

/// Emitted when `/clear` begins executing.
pub const CLEAR_STARTED: &str = "tengu_command_clear_started";
/// Emitted when `/clear` succeeds.
pub const CLEAR_COMPLETED: &str = "tengu_command_clear_completed";
/// Emitted when `/clear` fails.
pub const CLEAR_FAILED: &str = "tengu_command_clear_failed";

/// Emitted when `/compact` begins executing.
pub const COMPACT_STARTED: &str = "tengu_command_compact_started";
/// Emitted when `/compact` succeeds.
pub const COMPACT_COMPLETED: &str = "tengu_command_compact_completed";
/// Emitted when `/compact` fails.
pub const COMPACT_FAILED: &str = "tengu_command_compact_failed";

/// Emitted when `/exit` begins executing.
pub const EXIT_STARTED: &str = "tengu_command_exit_started";
/// Emitted when `/exit` succeeds (always — `request_exit` is infallible).
pub const EXIT_COMPLETED: &str = "tengu_command_exit_completed";
/// Emitted when `/exit` fails (currently unreachable, reserved for future).
pub const EXIT_FAILED: &str = "tengu_command_exit_failed";

/// Emitted when `/help` begins executing.
pub const HELP_STARTED: &str = "tengu_command_help_started";
/// Emitted when `/help` succeeds.
pub const HELP_COMPLETED: &str = "tengu_command_help_completed";
/// Emitted when `/help` fails (currently unreachable, reserved for future).
pub const HELP_FAILED: &str = "tengu_command_help_failed";

/// Emitted when `/init` begins executing.
pub const INIT_STARTED: &str = "tengu_command_init_started";
/// Emitted when `/init` succeeds (template injected as next user message).
pub const INIT_COMPLETED: &str = "tengu_command_init_completed";
/// Emitted when `/init` fails (currently unreachable, reserved for future).
pub const INIT_FAILED: &str = "tengu_command_init_failed";

/// Emitted when `/memory` begins executing (before spawning `$EDITOR`).
pub const MEMORY_STARTED: &str = "tengu_command_memory_started";
/// Emitted when `/memory` succeeds (after `$EDITOR` exits, regardless of code).
pub const MEMORY_COMPLETED: &str = "tengu_command_memory_completed";
/// Emitted when `/memory` fails (editor spawn / target-path I/O failure).
pub const MEMORY_FAILED: &str = "tengu_command_memory_failed";

/// All 18 command-event names (sorted ASCII-ascending: by command-name then phase).
///
/// Locked at length **18** for M5-10 ([`super::ALL_EVENT_NAMES`] formula must
/// add **18** for the `command` slot). M5-11 expands this to **54** (+36).
///
/// Sort: per-command alphabetical; within each command the phase order is
/// `_completed < _failed < _started` (ASCII: `c` < `f` < `s`).
pub const NAMES: &[&str; 18] = &[
    CLEAR_COMPLETED,
    CLEAR_FAILED,
    CLEAR_STARTED,
    COMPACT_COMPLETED,
    COMPACT_FAILED,
    COMPACT_STARTED,
    EXIT_COMPLETED,
    EXIT_FAILED,
    EXIT_STARTED,
    HELP_COMPLETED,
    HELP_FAILED,
    HELP_STARTED,
    INIT_COMPLETED,
    INIT_FAILED,
    INIT_STARTED,
    MEMORY_COMPLETED,
    MEMORY_FAILED,
    MEMORY_STARTED,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_length_locked_at_18() {
        assert_eq!(NAMES.len(), 18);
    }

    #[test]
    fn names_are_sorted_ascii_ascending() {
        let mut sorted = NAMES.to_vec();
        sorted.sort_unstable();
        assert_eq!(NAMES, sorted.as_slice());
    }

    #[test]
    fn every_name_starts_with_tengu_command() {
        for n in NAMES {
            assert!(n.starts_with("tengu_command_"), "{n} missing prefix");
        }
    }

    #[test]
    fn every_phase_is_started_completed_or_failed() {
        for n in NAMES {
            assert!(
                n.ends_with("_started") || n.ends_with("_completed") || n.ends_with("_failed"),
                "{n} has wrong phase"
            );
        }
    }

    #[test]
    fn exactly_three_phases_per_command() {
        let mut by_cmd: std::collections::BTreeMap<&str, Vec<&str>> =
            std::collections::BTreeMap::new();
        for n in NAMES {
            let stripped = n.strip_prefix("tengu_command_").unwrap();
            // Split at last underscore — the part after is the phase.
            let last_under = stripped.rfind('_').expect("phase delimiter missing");
            let cmd = &stripped[..last_under];
            by_cmd.entry(cmd).or_default().push(n);
        }
        assert_eq!(by_cmd.len(), 6, "expected 6 distinct command names");
        for (cmd, names) in &by_cmd {
            assert_eq!(
                names.len(),
                3,
                "/{cmd} should have exactly 3 phases, got {}",
                names.len()
            );
        }
    }

    #[test]
    fn covers_all_6_batch_1_commands() {
        let expected: std::collections::HashSet<&str> =
            ["clear", "compact", "exit", "help", "init", "memory"]
                .iter()
                .copied()
                .collect();
        let mut found: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for n in NAMES {
            let stripped = n.strip_prefix("tengu_command_").unwrap();
            let last_under = stripped.rfind('_').unwrap();
            found.insert(&stripped[..last_under]);
        }
        assert_eq!(found, expected, "missing or extra command names");
    }

    #[test]
    fn individual_constants_match_event_strings() {
        assert_eq!(CLEAR_STARTED, "tengu_command_clear_started");
        assert_eq!(CLEAR_COMPLETED, "tengu_command_clear_completed");
        assert_eq!(CLEAR_FAILED, "tengu_command_clear_failed");
        assert_eq!(INIT_FAILED, "tengu_command_init_failed");
        assert_eq!(MEMORY_FAILED, "tengu_command_memory_failed");
    }
}
