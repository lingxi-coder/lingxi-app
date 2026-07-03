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

/// `isEnvDefinedFalsy(envVar)` (`utils/envUtils.ts:39-47`): a defined,
/// non-empty value that normalizes (lowercase + trim) to one of
/// `0`/`false`/`no`/`off`. An undefined or empty value is NOT falsy (TS returns
/// `false` for `undefined` and for `''`). Mirror of [`is_env_truthy`]'s negative
/// pole, used by gates that distinguish "explicitly off" from "unset".
#[must_use]
pub fn is_env_defined_falsy(value: Option<&str>) -> bool {
    match value {
        None => false,
        Some(v) if v.is_empty() => false,
        Some(v) => matches!(v.to_lowercase().trim(), "0" | "false" | "no" | "off"),
    }
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

    /// TS-faithful truth table for `isEnvDefinedFalsy` (`utils/envUtils.ts:39-47`):
    /// a defined, non-empty value normalized to one of `0`/`false`/`no`/`off`;
    /// `undefined`/`''` are NOT falsy.
    #[test]
    fn env_defined_falsy_matrix() {
        for v in ["0", "false", "FALSE", " no ", "Off"] {
            assert!(
                is_env_defined_falsy(Some(v)),
                "{v:?} should be defined-falsy"
            );
        }
        // Not falsy: empty, unset, or out-of-set (incl. the truthy values).
        for v in ["", "1", "true", "yes", "on", "2", "disabled"] {
            assert!(
                !is_env_defined_falsy(Some(v)),
                "{v:?} should NOT be defined-falsy"
            );
        }
        assert!(!is_env_defined_falsy(None));
    }
}
