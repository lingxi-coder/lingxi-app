// lingxi-code/crates/core/src/settings/schema.rs
//! `SettingsJson` — strongly-typed mirror of claude-code's `settings.json`.
//!
//! Field naming policy: keep camelCase on the wire (claude-code emits it),
//! `snake_case` in Rust via `#[serde(rename_all = "camelCase")]`. New fields
//! land as `Option<T>` so deserializing an older file never fails on a
//! missing key — this matches v3 Event/Effect stability policy.
//!
//! `$schema` is NOT emitted (claude-code @ commit 6a25909 doesn't emit one)
//! but IS tolerated as an opaque ignored field for future-compat.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Per-field merge strategy.
///
/// The merger ([`crate::settings::merger`]) consults [`strategy_for`] to pick
/// the right combinator for a given JSON key. Defaults to [`MergeStrategy::Override`]
/// for any key not explicitly registered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeStrategy {
    /// Concatenate the two arrays and drop duplicates while preserving order.
    ConcatDedup,
    /// Recurse into the two objects, merging key-by-key under their own strategies.
    DeepMerge,
    /// Later source wins. Used for scalars and any field not in the table.
    Override,
}

/// Per-field strategy table. Order does not matter; the lookup is linear.
///
/// Entries here MUST match spec §7 wire identifiers byte-for-byte.
pub const MERGE_STRATEGIES: &[(&str, MergeStrategy)] = &[
    // Array-merge fields (spec §7).
    ("trustedDirectories", MergeStrategy::ConcatDedup),
    ("additionalDirectories", MergeStrategy::ConcatDedup),
    ("enabledTools", MergeStrategy::ConcatDedup),
    ("additionalIncludes", MergeStrategy::ConcatDedup),
    // Object-merge fields (spec §7).
    ("sandbox", MergeStrategy::DeepMerge),
    ("hooks", MergeStrategy::DeepMerge),
    ("permissions", MergeStrategy::DeepMerge),
    ("outputStyle", MergeStrategy::DeepMerge),
    // LingXi extension — deep-merge so multiple settings layers can each
    // declare a subset of provider profiles.
    ("providers", MergeStrategy::DeepMerge),
    // LingXi extension — deep-merge so multiple settings layers can each
    // contribute routing aliases, fallback chains, and retry policy.
    ("routing", MergeStrategy::DeepMerge),
];

/// Look up the merge strategy for a field name.
///
/// Returns `None` for unknown fields; callers default unknown fields to
/// [`MergeStrategy::Override`] (later source wins).
#[must_use]
pub fn strategy_for(field: &str) -> Option<MergeStrategy> {
    MERGE_STRATEGIES
        .iter()
        .find_map(|(k, v)| (*k == field).then_some(*v))
}

/// Mirror of claude-code's `settings.json` shape.
///
/// All fields `Option<T>` so a partial file (one layer of the 4-layer stack)
/// can omit a field without it showing up as `Some(Default::default())` —
/// "absent" must round-trip distinctly from "explicitly set to default".
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SettingsJson {
    /// Forward-compat: claude-code @ 6a25909 does NOT emit this; we tolerate
    /// readers that do (some IDE tooling may inject a `$schema` reference).
    #[serde(rename = "$schema", default, skip_serializing_if = "Option::is_none")]
    pub dollar_schema: Option<String>,

    /// Array-merge field (concat-dedup).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trusted_directories: Option<Vec<String>>,

    /// Array-merge field (concat-dedup).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_directories: Option<Vec<String>>,

    /// Array-merge field (concat-dedup).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled_tools: Option<Vec<String>>,

    /// Array-merge field (concat-dedup).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_includes: Option<Vec<String>>,

    /// Object-merge field (deep-merge).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<BTreeMap<String, Value>>,

    /// Object-merge field (deep-merge).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hooks: Option<BTreeMap<String, Value>>,

    /// Object-merge field (deep-merge). claude-code `permissions` block:
    /// `{ "allow": [...], "deny": [...], "ask": [...], "defaultMode": "...",
    ///    "additionalDirectories": [...] }` (rule strings like `"Bash(npm run *)"`).
    /// Opaque here — projected into typed rules by
    /// `permission::permission_rules_from_settings_json`. Without this field,
    /// `deny_unknown_fields` would REJECT any settings.json carrying a
    /// `permissions` block (breaking the whole load), so declaring it is
    /// required even though enforcement is wired separately.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permissions: Option<BTreeMap<String, Value>>,

    /// Object-merge field (deep-merge).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_style: Option<BTreeMap<String, Value>>,

    /// Scalar field (later source wins). Telemetry on/off toggle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub telemetry_enabled: Option<bool>,

    /// Scalar field (later source wins). Default model alias.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    /// Object-merge field (deep-merge). `LingXi` extension (claude-code has no
    /// such key): named LLM provider profiles, e.g.
    /// `{ "groq": { "type": "openai", "baseUrl": "...", "apiKeyEnv": "GROQ_API_KEY" } }`.
    /// Parsed into typed profiles by `providers::parse_profiles`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub providers: Option<BTreeMap<String, Value>>,

    /// Object-merge field (deep-merge). `LingXi` extension: routing config
    /// for model aliases, fallback chains, and retry policy. Shape:
    /// `{ "aliases": {alias: "provider/model"}, "fallback": {key: ["provider/model", …]},
    ///    "retry": {"maxAttempts": n, "backoffMs": n} }`.
    /// Parsed by `providers::parse_routing`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routing: Option<Value>,
}

impl SettingsJson {
    /// Run cross-field semantic checks beyond what `deny_unknown_fields` catches.
    ///
    /// # Errors
    ///
    /// Returns [`SettingsError::SchemaViolation`] with a human-readable trail
    /// pointing at the offending field path (e.g. `trustedDirectories[1]`).
    ///
    /// [`SettingsError::SchemaViolation`]: crate::settings::SettingsError::SchemaViolation
    pub fn validate(&self) -> Result<(), crate::settings::SettingsError> {
        for (field_name, array) in [
            ("trustedDirectories", self.trusted_directories.as_deref()),
            (
                "additionalDirectories",
                self.additional_directories.as_deref(),
            ),
            ("enabledTools", self.enabled_tools.as_deref()),
            ("additionalIncludes", self.additional_includes.as_deref()),
        ] {
            if let Some(arr) = array {
                for (i, s) in arr.iter().enumerate() {
                    if s.is_empty() {
                        return Err(crate::settings::SettingsError::SchemaViolation(format!(
                            "{field_name}[{i}] is empty"
                        )));
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserializes_minimal_settings() {
        let json = r#"{"trustedDirectories": ["/foo"]}"#;
        let parsed: SettingsJson = serde_json::from_str(json).unwrap();
        assert_eq!(
            parsed.trusted_directories.as_deref(),
            Some(&["/foo".to_string()][..])
        );
    }

    #[test]
    fn rejects_unknown_fields() {
        let json = r#"{"trustedDirectories": [], "bogusField": 1}"#;
        let err = serde_json::from_str::<SettingsJson>(json).unwrap_err();
        assert!(
            err.to_string().contains("unknown field"),
            "expected unknown-field error, got: {err}"
        );
    }

    #[test]
    fn accepts_permissions_block() {
        // Regression: before the `permissions` field existed, `deny_unknown_fields`
        // REJECTED any settings.json carrying a claude-code permissions block,
        // breaking the entire load. It must now parse (the rules are projected
        // out separately by `permission::permission_rules_from_settings_json`).
        let json = r#"{
            "permissions": { "allow": ["Bash(npm run *)"], "deny": ["Read(./secrets/**)"], "ask": [], "defaultMode": "default" }
        }"#;
        let parsed: SettingsJson = serde_json::from_str(json).expect("permissions block must parse");
        assert!(parsed.permissions.is_some());
        assert!(strategy_for("permissions").is_some(), "permissions has a merge strategy");
    }

    #[test]
    fn tolerates_dollar_schema_field_for_future_compat() {
        // claude-code @ 6a25909 does NOT emit "$schema". We tolerate it for forward-compat.
        let json = r#"{"$schema": "https://example.com/schema.json", "trustedDirectories": []}"#;
        let parsed = serde_json::from_str::<SettingsJson>(json);
        assert!(parsed.is_ok(), "should tolerate $schema field: {parsed:?}");
    }

    #[test]
    fn merge_strategies_table_covers_array_fields() {
        // The four spec §7 array-merge fields MUST be registered as ConcatDedup.
        for field in [
            "trustedDirectories",
            "additionalDirectories",
            "enabledTools",
            "additionalIncludes",
        ] {
            let strat =
                strategy_for(field).unwrap_or_else(|| panic!("missing strategy for {field}"));
            assert!(
                matches!(strat, MergeStrategy::ConcatDedup),
                "{field} should be ConcatDedup, got {strat:?}"
            );
        }
    }

    #[test]
    fn merge_strategies_table_covers_object_fields() {
        // The spec §7 object-merge fields MUST be registered as DeepMerge
        // (plus the `permissions` block — claude-code parity).
        for field in ["sandbox", "hooks", "permissions", "outputStyle"] {
            let strat =
                strategy_for(field).unwrap_or_else(|| panic!("missing strategy for {field}"));
            assert!(
                matches!(strat, MergeStrategy::DeepMerge),
                "{field} should be DeepMerge, got {strat:?}"
            );
        }
    }

    #[test]
    fn validate_rejects_empty_string_in_trusted_dirs() {
        let json = r#"{"trustedDirectories": ["/foo", ""]}"#;
        let parsed: SettingsJson = serde_json::from_str(json).unwrap();
        let err = parsed.validate().unwrap_err();
        assert!(
            matches!(err, crate::settings::SettingsError::SchemaViolation(ref s) if s.contains("trustedDirectories[1]")),
            "expected SchemaViolation pointing at index 1, got: {err:?}"
        );
    }

    #[test]
    fn validate_accepts_well_formed_settings() {
        let json = r#"{"trustedDirectories": ["/foo"], "telemetryEnabled": true}"#;
        let parsed: SettingsJson = serde_json::from_str(json).unwrap();
        assert!(parsed.validate().is_ok());
    }

    #[test]
    fn providers_field_roundtrips() {
        let raw = r#"{"providers":{"groq":{"type":"openai","baseUrl":"https://api.groq.com/openai/v1","apiKeyEnv":"GROQ_API_KEY"}}}"#;
        let parsed: SettingsJson = serde_json::from_str(raw).expect("parse");
        assert!(parsed.providers.is_some());
        let back = serde_json::to_string(&parsed).expect("serialize");
        assert!(back.contains("\"providers\""));
    }

    #[test]
    fn routing_field_roundtrips() {
        let raw = r#"{"routing":{"aliases":{"fast":"openai/gpt-4o"},"retry":{"maxAttempts":2,"backoffMs":100}}}"#;
        let parsed: SettingsJson = serde_json::from_str(raw).expect("parse");
        assert!(parsed.routing.is_some());
        let back = serde_json::to_string(&parsed).expect("serialize");
        assert!(back.contains("\"routing\""));
    }
}
