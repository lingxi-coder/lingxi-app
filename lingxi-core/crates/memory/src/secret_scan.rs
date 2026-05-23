//! Thin adapter over `lingxi_secret::SecretScanner`. Filled in Task 12.

/// Placeholder redaction. Real impl in Task 12 forwards to
/// `lingxi_secret::SecretScanner::builtin().redact(content)`.
#[must_use]
pub fn redact(content: &str) -> String {
    content.to_string()
}
