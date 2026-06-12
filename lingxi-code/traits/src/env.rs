//! Shared TS-faithful env-truthiness helper.
//!
//! Port of claude-code `isEnvTruthy` (`utils/envUtils.ts:32-37`): unset/empty
//! ⇒ false; otherwise the lowercased, trimmed value must be one of
//! `1`/`true`/`yes`/`on`. The workspace previously carried several private
//! copies; TS-faithful ones consolidate here (llm-client future-work batch 5,
//! Task 1). Copies with deliberately different semantics stay local and
//! documented.

/// `isEnvTruthy(envVar)` — see module docs.
#[must_use]
pub fn is_env_truthy(value: Option<&str>) -> bool {
    let Some(v) = value else { return false };
    matches!(v.to_lowercase().trim(), "1" | "true" | "yes" | "on")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// TS-faithful truth table for `isEnvTruthy` (`utils/envUtils.ts:32-37`):
    /// unset/empty ⇒ false; otherwise the lowercased, trimmed value must be
    /// one of `1`/`true`/`yes`/`on`. Note `"off "` trims to `"off"`, which is
    /// NOT in the TS set, so it is falsy (only `"on"` is in the set).
    #[test]
    fn env_truthy_matrix() {
        // Truthy: in-set after lowercase + trim.
        for v in ["1", "true", "TRUE", " yes ", "On"] {
            assert!(is_env_truthy(Some(v)), "{v:?} should be truthy");
        }
        // Falsy: empty or out-of-set after lowercase + trim.
        for v in ["", "0", "false", "off ", "no", "2", "enabled"] {
            assert!(!is_env_truthy(Some(v)), "{v:?} should be falsy");
        }
        // Falsy: unset.
        assert!(!is_env_truthy(None));
    }
}
