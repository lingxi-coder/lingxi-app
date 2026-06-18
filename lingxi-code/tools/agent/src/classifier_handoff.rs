//! G12 — handoff safety classifier (feature-gated; OFF by default).
//!
//! Port of claude-code's `classifyHandoffIfNeeded` (agentToolUtils.ts:389-481).
//! When a subagent hands control back to the main agent in AUTO permission mode,
//! claude runs a 2-stage LLM "yolo" classifier over the subagent's transcript
//! and, if it flags the work, PREPENDS a SECURITY WARNING text block to the
//! result content the main agent sees (call site AgentTool.tsx:1236-1252).
//!
//! claude gates the WHOLE thing behind `feature('TRANSCRIPT_CLASSIFIER')`, which
//! is OFF by default — so a faithful Rust port is a NO-OP on the common path,
//! and the result content stays byte-identical to today.
//!
//! STATUS (PLANNED + guarded): the actual `yoloClassifier` (the 2-stage LLM
//! classify + `tengu_auto_mode_decision` 13-field emit) is a LARGE deferred
//! subsystem with no Rust analog. This module ships ONLY:
//!   - the OFF-by-default gate ([`is_transcript_classifier_enabled`]), matching
//!     claude's `feature('TRANSCRIPT_CLASSIFIER')`;
//!   - the two byte-locked warning strings claude prepends; and
//!   - [`classify_handoff_if_needed`], which short-circuits to `None` while the
//!     classifier is unwired (gate OFF), and — once the classifier lands — will
//!     run the auto-mode-gated classify + return the warning to prepend.
//!
//! Do NOT half-merge a broken classifier: an OFF feature is the clean guard.

use permission::PermissionMode;

/// claude warning when the classifier was UNAVAILABLE while reviewing a
/// subagent's work (agentToolUtils.ts:469). Byte-locked.
pub const CLASSIFIER_UNAVAILABLE_WARNING: &str = "Note: The safety classifier was unavailable when reviewing this sub-agent's work. Please carefully verify the sub-agent's actions and output before acting on them.";

/// claude warning prefix when the classifier FLAGGED a subagent's work
/// (agentToolUtils.ts:476) — the full string is built by interpolating the
/// classifier's `reason` via [`format_security_warning`]. The prefix + suffix
/// are byte-locked; only `{reason}` varies.
pub const SECURITY_WARNING_PREFIX: &str =
    "SECURITY WARNING: This sub-agent performed actions that may violate security policy. Reason: ";
/// Suffix appended after the interpolated `{reason}` (agentToolUtils.ts:476).
pub const SECURITY_WARNING_SUFFIX: &str =
    ". Review the sub-agent's actions carefully before acting on its output.";

/// Build the full claude SECURITY WARNING string for a flagged handoff
/// (agentToolUtils.ts:476): `"SECURITY WARNING: … Reason: {reason}. Review …"`.
#[must_use]
pub fn format_security_warning(reason: &str) -> String {
    format!("{SECURITY_WARNING_PREFIX}{reason}{SECURITY_WARNING_SUFFIX}")
}

/// Whether the handoff transcript classifier is enabled (claude
/// `feature('TRANSCRIPT_CLASSIFIER')`).
///
/// OFF by default — there is no GrowthBook in Rust, so this mirrors claude's
/// default-OFF feature flag via an opt-in env override
/// (`CLAUDE_CODE_TRANSCRIPT_CLASSIFIER`). When OFF (the default),
/// [`classify_handoff_if_needed`] is a strict no-op and the result content is
/// byte-identical to today.
#[must_use]
pub fn is_transcript_classifier_enabled() -> bool {
    traits::env::is_env_truthy(
        std::env::var("CLAUDE_CODE_TRANSCRIPT_CLASSIFIER")
            .ok()
            .as_deref(),
    )
}

/// Port of claude `classifyHandoffIfNeeded` (agentToolUtils.ts:389-481):
/// returns `Some(warning)` to PREPEND to the subagent's result content when the
/// handoff classifier flags the work, else `None`.
///
/// Behavior reproduced EXACTLY (claude order):
/// 1. gate on `feature('TRANSCRIPT_CLASSIFIER')` — OFF by default → `None`;
/// 2. `if (toolPermissionContext.mode !== 'auto') return null` (line 405);
/// 3. build the classifier transcript; if empty → `None`;
/// 4. run the classifier; emit `tengu_auto_mode_decision` (13 fields);
/// 5. unavailable+block → [`CLASSIFIER_UNAVAILABLE_WARNING`]; block →
///    [`format_security_warning`]`(reason)`; else `None`.
///
/// PLANNED: steps 3-5 require the 2-stage `yoloClassifier` LLM subsystem, which
/// has no Rust analog yet. While the feature is OFF (the default) the function
/// returns `None` BEFORE any classify call (`AgentTool` also wraps the call in
/// the same gate), so the common path is byte-identical. When the classifier
/// lands, the auto-mode gate + classify + warning-prepend wire in here.
#[must_use]
pub async fn classify_handoff_if_needed(permission_mode: PermissionMode) -> Option<String> {
    // (1) feature gate (claude line 404). OFF by default → no-op.
    if !is_transcript_classifier_enabled() {
        return None;
    }
    // (2) auto-mode gate (claude line 405): only AUTO permission mode classifies.
    if permission_mode != PermissionMode::Auto {
        return None;
    }
    // (3-5) PLANNED: the 2-stage yoloClassifier + tengu_auto_mode_decision emit
    // is the LARGE deferred piece. Until it lands, even with the feature ON we
    // cannot classify, so we return None (no fabricated warning, no broken
    // classify). The warning-prepend SHAPE is exercised by `format_security_warning`
    // / the byte-locked consts above.
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warning_strings_are_byte_locked() {
        // Byte-for-byte vs claude agentToolUtils.ts:469.
        assert_eq!(CLASSIFIER_UNAVAILABLE_WARNING, "Note: The safety classifier was unavailable when reviewing this sub-agent's work. Please carefully verify the sub-agent's actions and output before acting on them.");
    }

    #[test]
    fn security_warning_is_byte_locked() {
        // Byte-for-byte vs claude agentToolUtils.ts:476 with reason interpolated.
        assert_eq!(format_security_warning("wrote to /etc/passwd"), "SECURITY WARNING: This sub-agent performed actions that may violate security policy. Reason: wrote to /etc/passwd. Review the sub-agent's actions carefully before acting on its output.");
    }

    /// The gate defaults OFF (no GrowthBook in Rust). Guarded by a process-wide
    /// lock because it mutates a shared env var.
    #[test]
    fn classifier_gate_default_off_and_env_override() {
        use std::sync::Mutex;
        static ENV_LOCK: Mutex<()> = Mutex::new(());
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        std::env::remove_var("CLAUDE_CODE_TRANSCRIPT_CLASSIFIER");
        assert!(!is_transcript_classifier_enabled(), "default OFF");

        std::env::set_var("CLAUDE_CODE_TRANSCRIPT_CLASSIFIER", "1");
        assert!(is_transcript_classifier_enabled(), "truthy ⇒ ON");

        std::env::remove_var("CLAUDE_CODE_TRANSCRIPT_CLASSIFIER");
    }

    /// With the feature OFF (default), the classify call is a strict no-op even
    /// in AUTO mode — so the common path is byte-identical (no warning prepend).
    #[tokio::test]
    async fn classify_is_noop_when_feature_off() {
        use std::sync::Mutex;
        static ENV_LOCK: Mutex<()> = Mutex::new(());
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("CLAUDE_CODE_TRANSCRIPT_CLASSIFIER");

        assert_eq!(classify_handoff_if_needed(PermissionMode::Auto).await, None);
        assert_eq!(
            classify_handoff_if_needed(PermissionMode::Default).await,
            None
        );
    }
}
