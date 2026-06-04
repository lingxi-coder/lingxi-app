//! Classifier kinds + the documented "no LLM classifier in external builds"
//! stub.
//!
//! For M1, [`ClassifierKind`] lives in `result` to avoid a forward cycle.
//! This module re-exports it for the canonical `classifier::ClassifierKind`
//! path.
//!
//! # External 1:1 parity: there is NO LLM classifier
//!
//! claude-code's LLM auto-mode / YOLO (transcript) classifier
//! (`utils/permissions/yoloClassifier.ts`, `classifierShared.ts`,
//! `classifierDecision.ts`) is gated behind the `feature('TRANSCRIPT_CLASSIFIER')`
//! `GrowthBook`/Statsig flag and is ant-internal. In **external** builds that
//! flag is always off, so the shipped behavior is a hardcoded stub:
//! `isClassifierPermissionsEnabled() === false` and every classifier call
//! resolves to `matches: false` (no auto-approval / no auto-rejection). There
//! is no `GrowthBook` in Rust and no LLM call to make, so the faithful external
//! behavior IS "no classifier" — which this crate already matches. **No LLM
//! port is attempted** (explicit non-goal; see SPECS Batch 6).
//!
//! The classifier plumbing that already exists in [`crate::result`] is
//! therefore *reserved*, not wired:
//! - [`ClassifierKind`] (`Yolo` / `Bash` / `Transcript`),
//! - `PermissionDecisionReason::ClassifierApproved` / `ClassifierRejected`,
//! - [`crate::result::PendingClassifierCheck`].
//!
//! These exist so the rule/result types are shape-complete for a future
//! ant-internal build, but nothing on the `authorize` path produces them in the
//! external build.

pub use crate::result::ClassifierKind;

/// Whether LLM-classifier-based permissions are enabled.
///
/// 1:1 with claude-code's external `isClassifierPermissionsEnabled()`: in
/// external builds the `TRANSCRIPT_CLASSIFIER` feature flag is off, so this is
/// hardcoded `false`. Exposed for call-site parity — any code that would gate
/// on the classifier should branch on this and take the "no classifier" path.
#[must_use]
pub const fn is_classifier_permissions_enabled() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifier_permissions_disabled_in_external_build() {
        // External-build parity: the LLM classifier never runs.
        assert!(!is_classifier_permissions_enabled());
    }
}
