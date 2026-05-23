// lingxi-core/crates/core/src/settings/schema.rs
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
    ("outputStyle", MergeStrategy::DeepMerge),
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
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
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

    /// Object-merge field (deep-merge).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_style: Option<BTreeMap<String, Value>>,

    /// Scalar field (later source wins). Telemetry on/off toggle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub telemetry_enabled: Option<bool>,

    /// Scalar field (later source wins). Default model alias.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
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
        // The three spec §7 object-merge fields MUST be registered as DeepMerge.
        for field in ["sandbox", "hooks", "outputStyle"] {
            let strat =
                strategy_for(field).unwrap_or_else(|| panic!("missing strategy for {field}"));
            assert!(
                matches!(strat, MergeStrategy::DeepMerge),
                "{field} should be DeepMerge, got {strat:?}"
            );
        }
    }
}
