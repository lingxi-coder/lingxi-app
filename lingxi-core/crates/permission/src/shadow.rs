//! Shadowed-rule detection — no-op stub in M1.
//!
//! Plan 03 (Tools) fills this in.

use crate::rule::PermissionRule;

/// Detects when a new rule would be shadowed by a higher-priority existing rule.
pub struct ShadowedRuleDetector;

impl ShadowedRuleDetector {
    /// Returns the shadowing rule, if any. M1 always returns `None`.
    #[must_use]
    pub fn find_shadowing(
        &self,
        _new_rule: &PermissionRule,
        _existing: &[&PermissionRule],
    ) -> Option<PermissionRule> {
        None
    }
}
