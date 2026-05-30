//! Tool-output truncation — cross-cutting helper used by every M4 tool that
//! emits user-visible output.
//!
//! Spec §7 (cross-cutting): `MAX_TOOL_OUTPUT_LENGTH = 30_000` chars (NOT
//! bytes — UTF-8 safe). Suffix is the literal `"\n\n[Output truncated due
//! to length]"`.

/// Maximum char count of a tool's user-visible output before truncation
/// kicks in. Locked at 30_000 per spec §7.
pub const MAX_TOOL_OUTPUT_LENGTH: usize = 30_000;

/// Literal appended to truncated output. Locked byte-for-byte per spec §7
/// (matches `claude-code/src/tools/.../utils.ts` truncation suffix).
pub const TRUNCATION_SUFFIX: &str = "\n\n[Output truncated due to length]";

/// Truncate `s` to at most `limit` characters, appending [`TRUNCATION_SUFFIX`]
/// when truncation occurred.
///
/// Returns `(out, did_truncate)`. `out.chars().count() <= limit` always.
/// If `limit` is smaller than `TRUNCATION_SUFFIX.chars().count()`, the
/// suffix alone is returned (degenerate but well-defined).
#[must_use]
pub fn truncate(s: String, limit: usize) -> (String, bool) {
    let suffix_len = TRUNCATION_SUFFIX.chars().count();
    let total = s.chars().count();
    if total <= limit {
        return (s, false);
    }
    if limit <= suffix_len {
        return (TRUNCATION_SUFFIX.chars().take(limit).collect(), true);
    }
    let keep = limit - suffix_len;
    let mut out: String = s.chars().take(keep).collect();
    out.push_str(TRUNCATION_SUFFIX);
    (out, true)
}

/// Convenience wrapper calling [`truncate`] with [`MAX_TOOL_OUTPUT_LENGTH`].
#[must_use]
pub fn truncate_default(s: String) -> (String, bool) {
    truncate(s, MAX_TOOL_OUTPUT_LENGTH)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_tool_output_length_is_30_000() {
        assert_eq!(MAX_TOOL_OUTPUT_LENGTH, 30_000);
    }

    #[test]
    fn truncation_suffix_byte_locked() {
        assert_eq!(TRUNCATION_SUFFIX, "\n\n[Output truncated due to length]");
    }

    #[test]
    fn short_string_passes_through() {
        let (out, trunc) = truncate("hello".to_string(), 100);
        assert_eq!(out, "hello");
        assert!(!trunc);
    }

    #[test]
    fn exact_length_passes_through() {
        let s = "a".repeat(100);
        let (out, trunc) = truncate(s.clone(), 100);
        assert_eq!(out, s);
        assert!(!trunc);
    }

    #[test]
    fn over_limit_appends_suffix() {
        let s = "a".repeat(200);
        let (out, trunc) = truncate(s, 100);
        assert!(trunc);
        assert!(out.ends_with(TRUNCATION_SUFFIX));
        assert_eq!(out.chars().count(), 100);
    }

    #[test]
    fn truncate_keeps_utf8_codepoints_intact() {
        // Deviation from plan: original limit=30 triggers underflow since
        // TRUNCATION_SUFFIX has 35 chars. Use limit=50 (> 35 suffix len) so
        // the normal truncation path is exercised, keeping the test intent
        // (UTF-8 codepoints stay intact) but avoiding the degenerate path.
        let s: String = "🎉".repeat(100);
        let limit = 50;
        let (out, trunc) = truncate(s, limit);
        assert!(trunc);
        assert_eq!(out.chars().count(), limit);
        let suffix_chars = TRUNCATION_SUFFIX.chars().count();
        let prefix_chars = limit - suffix_chars;
        let prefix: String = "🎉".repeat(prefix_chars);
        assert!(out.starts_with(&prefix));
        assert!(out.ends_with(TRUNCATION_SUFFIX));
    }

    #[test]
    fn truncate_default_uses_30k_limit() {
        let s = "x".repeat(40_000);
        let (out, trunc) = truncate_default(s);
        assert!(trunc);
        assert_eq!(out.chars().count(), MAX_TOOL_OUTPUT_LENGTH);
        assert!(out.ends_with(TRUNCATION_SUFFIX));
    }

    #[test]
    fn truncate_below_suffix_length_yields_suffix_prefix() {
        // Degenerate edge case: caller asked for 5 chars of budget; suffix is
        // 35 chars. We return the first 5 chars of the suffix.
        let s = "abcdefghij".to_string();
        let (out, trunc) = truncate(s, 5);
        assert!(trunc);
        let expected: String = TRUNCATION_SUFFIX.chars().take(5).collect();
        assert_eq!(out, expected);
    }
}
