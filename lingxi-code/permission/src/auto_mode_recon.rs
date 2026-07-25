//! WIZARD-06 recon (environment scan) fact-gathering core for `/auto-mode-setup`.
//!
//! The full recon step is an LLM side-query (the `auto_mode_scan` tool, "environment
//! scan for /auto-mode-setup") whose large system prompt guides the model to render
//! the user's environment into the proposal's `environment` slots. This module is the
//! byte-verifiable, LLM-free part: the recon VOCABULARY (scan-tool description + error
//! codes/messages, byte-exact vs 2.1.218) and the pure settings-tier fact gatherer the
//! recon feeds the model — which existing `autoMode` blocks are present (for the "Found
//! N inert autoMode entries" observation) and which `permissions.allow` rules are
//! destructive (the `remove_from_permissions_allow` removal offer). The model prompt,
//! transcript-mining, and background orchestration are a separate (product) effort.

use serde_json::Value;

use crate::dangerous_perms::{find_dangerous_classifier_permissions, DangerousPermissionInfo};
use crate::rule::{PermissionBehavior, PermissionRule, PermissionRuleSource, PermissionRuleValue};

/// The `auto_mode_scan` side-query tool description (byte-exact vs 2.1.218).
pub const RECON_SCAN_DESCRIPTION: &str = "environment scan for /auto-mode-setup";

/// Telemetry/return code when the recon gather failed (`recon_failed`).
pub const RECON_FAILED_CODE: &str = "recon_failed";

/// Telemetry/return code when the proposal JSON could not be parsed (`parse_failed`).
pub const PARSE_FAILED_CODE: &str = "parse_failed";

/// Telemetry code when the proposal JSON was recovered by repair (`parse_repaired`).
pub const PARSE_REPAIRED_CODE: &str = "parse_repaired";

/// The byte-exact recon-gather failure message prefix (2.1.218:
/// `auto-mode-setup gather failed: ${err}`).
#[must_use]
pub fn gather_failed_message(err: &str) -> String {
    format!("auto-mode-setup gather failed: {err}")
}

/// The recon facts gathered from ONE settings tier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsTierRecon {
    /// Which settings tier this came from.
    pub source: PermissionRuleSource,
    /// How many `autoMode.*` keys the tier already has (the `N` in the model's
    /// "Found N inert autoMode entries in <file>" recon-status note). `0` when the
    /// tier has no `autoMode` block (or a non-object one).
    pub auto_mode_entry_count: usize,
    /// The destructive `permissions.allow` rules in this tier — the removal offer
    /// (`remove_from_permissions_allow`), each with its byte-exact display + source.
    pub dangerous_allow: Vec<DangerousPermissionInfo>,
}

/// Gather the recon facts from one settings tier's parsed JSON (`settings`),
/// tagging every allow rule with `source`. Reads `autoMode` (entry count) and
/// `permissions.allow` (destructive-rule enumeration via
/// [`find_dangerous_classifier_permissions`]). A missing/malformed section
/// contributes nothing (never errors — recon is best-effort).
#[must_use]
pub fn scan_settings_tier(settings: &Value, source: PermissionRuleSource) -> SettingsTierRecon {
    let auto_mode_entry_count = settings
        .get("autoMode")
        .and_then(Value::as_object)
        .map_or(0, serde_json::Map::len);

    let rules: Vec<PermissionRule> = settings
        .get("permissions")
        .and_then(|p| p.get("allow"))
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(|spec| PermissionRule {
                    value: PermissionRuleValue::from_rule_string(spec),
                    behavior: PermissionBehavior::Allow,
                    source,
                })
                .collect()
        })
        .unwrap_or_default();

    SettingsTierRecon {
        source,
        auto_mode_entry_count,
        dangerous_allow: find_dangerous_classifier_permissions(&rules),
    }
}

/// The verbatim rule strings the removal offer carries — every destructive allow
/// rule across `tiers`, de-duplicated in first-seen order (the
/// `remove_from_permissions_allow` array the proposal/review UI offers to remove).
#[must_use]
pub fn removal_offer(tiers: &[SettingsTierRecon]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for tier in tiers {
        for info in &tier.dangerous_allow {
            if seen.insert(info.rule_display.clone()) {
                out.push(info.rule_display.clone());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn vocabulary_is_byte_exact() {
        assert_eq!(RECON_SCAN_DESCRIPTION, "environment scan for /auto-mode-setup");
        assert_eq!(RECON_FAILED_CODE, "recon_failed");
        assert_eq!(PARSE_FAILED_CODE, "parse_failed");
        assert_eq!(PARSE_REPAIRED_CODE, "parse_repaired");
        assert_eq!(
            gather_failed_message("EACCES"),
            "auto-mode-setup gather failed: EACCES"
        );
    }

    #[test]
    fn counts_existing_automode_entries() {
        let settings = json!({
            "autoMode": { "allow": ["x"], "environment": ["y"], "soft_deny": [] }
        });
        let r = scan_settings_tier(&settings, PermissionRuleSource::LocalSettings);
        assert_eq!(r.auto_mode_entry_count, 3);
        assert!(r.dangerous_allow.is_empty());
    }

    #[test]
    fn no_automode_block_is_zero() {
        let r = scan_settings_tier(&json!({"model": "opus"}), PermissionRuleSource::UserSettings);
        assert_eq!(r.auto_mode_entry_count, 0);
        // A non-object autoMode also counts as zero (nothing to observe).
        let r2 = scan_settings_tier(&json!({"autoMode": "nope"}), PermissionRuleSource::UserSettings);
        assert_eq!(r2.auto_mode_entry_count, 0);
    }

    #[test]
    fn enumerates_dangerous_allow_rules() {
        let settings = json!({
            "permissions": { "allow": ["Bash(*)", "Read", "Bash(rm:*)", "Edit(src/**)"] }
        });
        let r = scan_settings_tier(&settings, PermissionRuleSource::UserSettings);
        let displays: Vec<&str> = r.dangerous_allow.iter().map(|d| d.rule_display.as_str()).collect();
        // The wildcard Bash rules are destructive; Read / scoped Edit are not.
        assert!(displays.contains(&"Bash(*)"), "got {displays:?}");
        assert!(displays.iter().all(|d| d.starts_with("Bash")));
        // Source display is byte-exact ("user settings").
        assert_eq!(r.dangerous_allow[0].source_display, "user settings");
    }

    #[test]
    fn removal_offer_dedups_across_tiers() {
        let user = scan_settings_tier(
            &json!({"permissions": {"allow": ["Bash(*)"]}}),
            PermissionRuleSource::UserSettings,
        );
        let project = scan_settings_tier(
            &json!({"permissions": {"allow": ["Bash(*)", "Bash(curl:*)"]}}),
            PermissionRuleSource::ProjectSettings,
        );
        let offer = removal_offer(&[user, project]);
        // `Bash(*)` appears in both tiers but is offered once, first-seen order.
        assert_eq!(offer.first().map(String::as_str), Some("Bash(*)"));
        assert_eq!(offer.iter().filter(|r| *r == "Bash(*)").count(), 1);
    }

    #[test]
    fn empty_and_missing_sections_are_safe() {
        let r = scan_settings_tier(&json!({}), PermissionRuleSource::UserSettings);
        assert_eq!(r.auto_mode_entry_count, 0);
        assert!(r.dangerous_allow.is_empty());
        assert!(removal_offer(&[r]).is_empty());
    }
}
