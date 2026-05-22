//! Static dangerous-command bash regex table.
//!
//! M1 ships a small high-confidence subset. Plan 03 Tools task expands this.

/// Bash command patterns considered dangerous (always require `Ask` or block).
pub const PATTERNS: &[&str] = &[
    r"rm\s+-rf\s+/",
    r"sudo\s+",
    r"curl\s+.*\|\s*(sh|bash)",
    r"chmod\s+777",
    r":\(\)\{.*:.*\|.*&.*\};:",
];
