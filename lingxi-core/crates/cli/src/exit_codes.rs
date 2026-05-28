//! Locked exit codes for `lingxi-cli`.
//!
//! See plan M5-12 Task 0 step 2 — these values MUST NOT change without an
//! explicit plan amendment because integration tests and downstream tooling
//! depend on them.

/// Conversation completed successfully (POSIX success).
pub const SUCCESS: i32 = 0;

/// Runtime error: orchestrator failure, API error, IO error, etc.
pub const RUNTIME_ERROR: i32 = 1;

/// Argv parse error (clap rejected the input).
pub const ARGV_ERROR: i32 = 2;

/// Subcommand not yet implemented (REPL stub pre-M5-13; reserved future
/// stubs). Matches BSD `EX_USAGE` / sysexits.h convention.
pub const NOT_IMPLEMENTED: i32 = 64;

/// SIGINT (Ctrl+C) received during turn execution. POSIX convention
/// (128 + SIGINT=2).
pub const SIGINT: i32 = 130;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locked_values() {
        assert_eq!(SUCCESS, 0);
        assert_eq!(RUNTIME_ERROR, 1);
        assert_eq!(ARGV_ERROR, 2);
        assert_eq!(NOT_IMPLEMENTED, 64);
        assert_eq!(SIGINT, 130);
    }
}
