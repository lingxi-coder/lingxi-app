// lingxi-code/crates/core/src/settings/schema.rs
//! `SettingsJson` — strongly-typed mirror of claude-code's `settings.json`.
//!
//! Field naming policy: keep camelCase on the wire (claude-code emits it),
//! `snake_case` in Rust via `#[serde(rename_all = "camelCase")]`. New fields
//! land as `Option<T>` so deserializing an older file never fails on a
//! missing key — this matches v3 Event/Effect stability policy.
//!
//! Unknown keys are tolerated-and-ignored across the board: claude-code's
//! zod `SettingsSchema` is explicitly `.passthrough()` (`types.ts:1072`,
//! consumed via `safeParse` at `settings.ts:219`), so unknown keys never fail
//! a TS load either. Typed fields exist so the engine can ACCESS known
//! values — they are not a load-time acceptance gate.
//!
//! KNOWN DIVERGENCE (safe while nothing round-trips this struct to disk):
//! TS `.passthrough()` RETAINS unknown keys in the parsed/merged in-memory
//! settings; this typed struct DROPS them, so a merged `SettingsJson`
//! re-serialized to disk would lose them. No Rust path does that today —
//! `/effort`, `ConfigTool`, and the migrations crate all write settings via
//! raw `serde_json::Map` read-modify-write, which preserves unknown keys.
//!
//! `$schema` is NOT emitted (claude-code @ commit 6a25909 doesn't emit one)
//! but is kept as a typed field so a file that carries one round-trips it.

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
    ("claudeMdExcludes", MergeStrategy::ConcatDedup),
    // Object-merge fields (spec §7).
    ("sandbox", MergeStrategy::DeepMerge),
    ("hooks", MergeStrategy::DeepMerge),
    ("permissions", MergeStrategy::DeepMerge),
    // NB: `outputStyle` is intentionally NOT here — TS types it as a string and
    // merges it scalar-override (settingsMergeCustomizer special-cases only
    // arrays), so it falls through to the default Override strategy.
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
///
/// Unknown keys are tolerated-and-ignored, matching claude-code's zod
/// `SettingsSchema` `.passthrough()` (`types.ts:1072`; `safeParse` at
/// `settings.ts:219`). Known fields keep their typed parses. Tolerance also
/// un-breaks settings files carrying keys written by `ConfigTool`, `/effort`
/// (`effortLevel`), or the migrations subsystem (`env`,
/// `skipDangerousModePermissionPrompt`, `enableAllProjectMcpServers`, …) —
/// under the previous `deny_unknown_fields` strictness any such key made the
/// whole-file load fail, and production callers' `.ok()` then silently
/// dropped the entire settings layer.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SettingsJson {
    /// Forward-compat: claude-code @ 6a25909 does NOT emit this; we keep it
    /// typed so a `$schema` reference injected by IDE tooling round-trips
    /// instead of being stripped on re-serialize.
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

    /// `claudeMdExcludes` (array-merge, concat-dedup): glob patterns or absolute
    /// paths of `LINGXI.md` files to exclude from loading (claude-code
    /// `settings/types.ts:1053`, gate `isClaudeMdExcluded`, `claudemd.ts:547`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_md_excludes: Option<Vec<String>>,

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
    /// `permission::permission_rules_from_settings_json`. The typed field
    /// exists for ACCESS (rule projection + the `DeepMerge` strategy), not as a
    /// load gate — unknown keys are tolerated-and-ignored anyway.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permissions: Option<BTreeMap<String, Value>>,

    /// Scalar field (later source wins). TS types this `outputStyle:
    /// z.string().optional()` (settings/types.ts:639; `type OutputStyle =
    /// string`, config.ts:181), and `settingsMergeCustomizer` special-cases
    /// only arrays, so a string `outputStyle` takes scalar-override
    /// (settings.ts:538-547) — NOT deep-merge. Typing it as a map made a real
    /// `"outputStyle": "Explanatory"` settings.json fail the whole load.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_style: Option<String>,

    /// Scalar field (later source wins). Telemetry on/off toggle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub telemetry_enabled: Option<bool>,

    /// Scalar field (later source wins). Default model alias.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    /// Object-merge field (deep-merge). `LingXi` extension (claude-code has no
    /// such key): named LLM provider profiles. Each entry has the shape:
    /// `{ "type": "openai"|"openai-responses"|"anthropic"|"gemini"|"azure-openai"
    ///           |"bedrock-claude"|"vertex-claude"|"vertex-gemini", "baseUrl": "...",
    ///    "apiKeyEnv": "GROQ_API_KEY",
    ///    "models": [{ "id": "model-id", "aliases": ["alias"]?,
    ///                 "capabilities": {...}? }] }`.
    ///
    /// **Wired (3c-T2):** parsed by `platform_common::apply_settings_providers`
    /// and appended to `llm_client::ClientConfig` in `build()`.  `models` is
    /// REQUIRED per entry; an absent or empty list is an error at engine startup.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub providers: Option<BTreeMap<String, Value>>,

    /// Object-merge field (deep-merge). `LingXi` extension: routing config
    /// for model aliases, fallback chains, and retry policy. Shape:
    /// `{ "aliases": {alias: "profile/model"},
    ///    "fallback": {"<primary-model-or-alias>": ["profile/model", …]},
    ///    "retry": {"maxAttempts": n, "backoffMs": n} }`.
    ///
    /// **Wiring status (batch-1 T1):**
    /// - `aliases`: **WIRED** — each alias is pushed onto the target
    ///   `ModelProfile` (resolved across all configured + builtin profiles).
    /// - `fallback`: **WIRED** — parsed by `platform_common::parse_routing_overrides`
    ///   and threaded into the adapter as per-model fallback overrides.
    ///   Per-model entry wins over the global `DesktopConfig.fallback_model` /
    ///   argv fallback.  Only chain[0] is used; longer chains log a warning.
    /// - `retry.maxAttempts`: **WIRED** — sets `RetryControl.max_retries`.
    ///   Precedence: `LINGXI_MAX_RETRIES` env > `maxAttempts` > default (10).
    /// - `retry.backoffMs`: **WIRED** — sets the exponential-backoff ladder's
    ///   first rung (overrides the default base of 500ms). Ladder is
    ///   `min(backoffMs * 2^attempt, 32000)`; `backoffMs=1000` →
    ///   `[1000, 2000, 4000, 8000, 16000, 32000, …]`. Additive jitter
    ///   `+ rand(0, 0.25) * base` still applies. Server-sent `Retry-After` is
    ///   never scaled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routing: Option<Value>,
}

impl SettingsJson {
    /// Run cross-field semantic checks beyond what serde's typed
    /// deserialization catches (unknown keys are tolerated, not validated).
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
            ("claudeMdExcludes", self.claude_md_excludes.as_deref()),
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
    fn ignores_unknown_fields_zod_passthrough_parity() {
        // zod `SettingsSchema` is `.passthrough()` (types.ts:1072; safeParse
        // at settings.ts:219) — unknown keys never fail a TS load; known
        // siblings must still parse typed.
        let json = r#"{"trustedDirectories": ["/foo"], "bogusField": 1}"#;
        let parsed: SettingsJson =
            serde_json::from_str(json).expect("unknown keys must be ignored, not rejected");
        assert_eq!(
            parsed.trusted_directories.as_deref(),
            Some(&["/foo".to_string()][..])
        );
    }

    #[test]
    fn accepts_permissions_block() {
        // Regression history: under the (since-removed) `deny_unknown_fields`
        // strictness, a settings.json carrying a claude-code permissions block
        // broke the entire load until the field was declared. Today unknown
        // keys are tolerated anyway; the typed field exists for ACCESS — rule
        // projection via `permission::permission_rules_from_settings_json`
        // and the DeepMerge strategy — not load-gating.
        let json = r#"{
            "permissions": { "allow": ["Bash(npm run *)"], "deny": ["Read(./secrets/**)"], "ask": [], "defaultMode": "default" }
        }"#;
        let parsed: SettingsJson = serde_json::from_str(json).expect("permissions block must parse");
        assert!(parsed.permissions.is_some());
        assert!(strategy_for("permissions").is_some(), "permissions has a merge strategy");
    }

    #[test]
    fn accepts_string_output_style() {
        // OUTSTYLE.1 regression history: `output_style` was once typed as a
        // map, so a real claude-code settings.json carrying the documented
        // `"outputStyle": "Explanatory"` (a string) failed deserialization
        // under the then-strict schema and broke the ENTIRE settings load
        // (dropping model / permissions / hooks in that layer). TS types it
        // `z.string()` — it must parse as a string so the value is ACCESSIBLE
        // typed (unknown-key tolerance alone would strip it, not surface it).
        let json = r#"{ "outputStyle": "Explanatory", "model": "claude-sonnet-4-5" }"#;
        let parsed: SettingsJson =
            serde_json::from_str(json).expect("string outputStyle must parse");
        assert_eq!(parsed.output_style.as_deref(), Some("Explanatory"));
        assert!(parsed.model.is_some(), "sibling fields must survive the load");
        // Scalar-override, not deep-merge.
        assert!(
            strategy_for("outputStyle").is_none(),
            "outputStyle must be scalar-override"
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
            "claudeMdExcludes",
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
        for field in ["sandbox", "hooks", "permissions"] {
            let strat =
                strategy_for(field).unwrap_or_else(|| panic!("missing strategy for {field}"));
            assert!(
                matches!(strat, MergeStrategy::DeepMerge),
                "{field} should be DeepMerge, got {strat:?}"
            );
        }
        // OUTSTYLE.1: `outputStyle` is a scalar string in claude-code, so it
        // must NOT be a registered DeepMerge field — it falls through to the
        // default Override strategy (later layer wins).
        assert!(
            strategy_for("outputStyle").is_none(),
            "outputStyle must default to Override (scalar), not be registered DeepMerge"
        );
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
