//! Tool-output truncation — populated in Task 3.

/// Placeholder — real value lands in Task 3.
pub const MAX_TOOL_OUTPUT_LENGTH: usize = 30_000;
/// Placeholder — real value lands in Task 3.
pub const TRUNCATION_SUFFIX: &str = "\n\n[Output truncated due to length]";

/// Placeholder — real impl lands in Task 3.
#[must_use]
pub fn truncate(s: String, _limit: usize) -> (String, bool) {
    (s, false)
}

/// Placeholder — real impl lands in Task 3.
#[must_use]
pub fn truncate_default(s: String) -> (String, bool) {
    truncate(s, MAX_TOOL_OUTPUT_LENGTH)
}
