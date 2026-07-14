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
    ("lingxiMdExcludes", MergeStrategy::ConcatDedup),
    // `companyAnnouncements` is an array field; CC's `settingsMergeCustomizer`
    // concat-dedups all arrays across settings tiers, so it merges the same way
    // as the other array keys (later-tier entries appended, dupes dropped).
    ("companyAnnouncements", MergeStrategy::ConcatDedup),
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

    /// `lingxiMdExcludes` (array-merge, concat-dedup): glob patterns or absolute
    /// paths of `LINGXI.md` files to exclude from loading (claude-code
    /// `settings/types.ts:1053`, gate `isLingxiMdExcluded`, `claudemd.ts:547`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lingxi_md_excludes: Option<Vec<String>>,

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

    /// Scalar field (later source wins). `askUserQuestionTimeout`: the idle
    /// window before an `AskUserQuestion` prompt auto-continues with the
    /// answers selected so far. Oracle 2.1.201 zod:
    /// `askUserQuestionTimeout:E.enum(["60s","5m","10m","never"]).catch(void 0)`
    /// with default `never` ("auto-continue only runs when explicitly set to
    /// 60s/5m/10m"). Typed `Option<String>` (like `outputStyle`), tolerant of
    /// unknown values via the zod `.catch` (parsed into the typed
    /// `tool_ui::ask_user_question::AskUserQuestionTimeout` at the tool layer).
    /// Scalar-override merge (not in `MERGE_STRATEGIES`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ask_user_question_timeout: Option<String>,

    /// Scalar field (later source wins). Telemetry on/off toggle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub telemetry_enabled: Option<bool>,

    /// Scalar field (later source wins). `axScreenReader`: accessibility
    /// screen-reader mode (2.1.201 settings schema `screenReader` group:
    /// "Render screen-reader friendly output (flat text, no decorative borders
    /// or animations). Overridden by the CLAUDE_AX_SCREEN_READER env var and
    /// the --ax-screen-reader CLI flag."). Read by the `ax_screen_reader` gate
    /// as the lowest-precedence source (below the env var and CLI flag). Key
    /// stays `axScreenReader` verbatim (config wire key).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ax_screen_reader: Option<bool>,

    /// Scalar field (later source wins). `alwaysThinkingEnabled`: when set to
    /// `false`, disables extended thinking for the session UNLESS a fixed budget
    /// is pinned via the `MAX_THINKING_TOKENS` env var or the
    /// `--max-thinking-tokens` flag (both pre-empt this). Consumed by the boot
    /// session `ThinkingConfig` resolver (`llm_client::model::thinking::
    /// session_thinking_from_env`, binary `qIe()`:
    /// `if(e.alwaysThinkingEnabled===!1)return!1;return!0`). Key stays
    /// `alwaysThinkingEnabled` verbatim (config wire key).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub always_thinking_enabled: Option<bool>,

    /// Scalar field (later source wins). `skipWebFetchPreflight`: skip the
    /// `WebFetch` domain-blocklist preflight for enterprise hosts whose network
    /// policy blocks outbound connections to `claude.ai`/`api.anthropic.com`.
    /// CC 2.1.207 settings zod (verbatim):
    /// `skipWebFetchPreflight:E.boolean().optional().describe("Skip the WebFetch
    /// blocklist check for enterprise environments with restrictive security
    /// policies")`. Consumed by `WebFetchTool` via
    /// `BuiltinToolContext::skip_web_fetch_preflight` — when true the tool skips
    /// the blocklist check entirely (binary
    /// `if(!Mi().skipWebFetchPreflight)switch((await DSd(g)).status){…}`;
    /// leaked `utils.ts:423-424` `if(!settings.skipWebFetchPreflight){…}`).
    /// Scalar-override merge (not in `MERGE_STRATEGIES`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skip_web_fetch_preflight: Option<bool>,

    /// Scalar field (later source wins). Default model alias.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    /// Scalar field (later source wins). `awsAuthRefresh`: path to a script
    /// that refreshes AWS authentication (2.1.198 settings schema: "Path to a
    /// script that refreshes AWS authentication"). Consumed by the llm-client
    /// AWS auth-refresh flow (`llm_client::aws_auth`, binary fn `ZBd`) when a
    /// Bedrock-style request fails with an expired-STS auth error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aws_auth_refresh: Option<String>,

    /// Scalar field (later source wins). `awsCredentialExport`: path to a
    /// script that exports AWS credentials (2.1.198 settings schema: "Path to
    /// a script that exports AWS credentials"; binary fn `t2d` parses its
    /// stdout as STS JSON — `{Credentials:{AccessKeyId,SecretAccessKey,
    /// SessionToken,Expiration}}` or the flat equivalent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aws_credential_export: Option<String>,

    /// Scalar field (later source wins). `gcpAuthRefresh`: command to refresh
    /// GCP authentication (2.1.198 settings schema: "Command to refresh GCP
    /// authentication (e.g., gcloud auth application-default login)").
    /// Schema-only today — the GCP refresh runtime (binary `zzr`/`Yzr`) is not
    /// ported; the key is typed so it round-trips and is ACCESSIBLE when the
    /// Vertex flow lands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gcp_auth_refresh: Option<String>,

    /// Array-merge field (concat-dedup). `companyAnnouncements`: strings shown
    /// in the TUI startup header (one selected per process). CC 2.1.207 zod
    /// (verbatim): `companyAnnouncements:E.array(E.string()).optional()
    /// .describe("Company announcements to display at startup (one will be
    /// randomly selected if multiple are provided)")`. Consumed by the selection
    /// helper [`crate::settings::company_announcements::select_company_announcement`]
    /// (port of binary `jxo`/`oip`): non-empty entries only, `[0]` when
    /// `numStartups==1` else uniform-random, memoized once per process. Merged
    /// concat-dedup, matching CC's `settingsMergeCustomizer` array customizer
    /// (`Array.isArray(t)&&Array.isArray(e)?(t.forEach(a=>{e.includes(a)||e.push(a)}))`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub company_announcements: Option<Vec<String>>,

    /// Scalar field (later source wins). `plansDirectory`: custom directory for
    /// plan-mode plan files, relative to the project root. CC 2.1.207 zod
    /// (verbatim): `plansDirectory:E.string().optional().describe("Custom
    /// directory for plan files, relative to project root. If not set, defaults
    /// to ~/.claude/plans/")`. Consumed by the orchestrator plan-file resolver
    /// (`ConversationOrchestrator::plan_file_path`, port of binary `iT`): when
    /// set, resolved against the project root with a within-root containment
    /// check; on failure the byte-exact error `plansDirectory must be within
    /// project root: {r}` is logged and the resolver falls back to the default
    /// `<config-home>/plans`. Scalar-override merge (not in `MERGE_STRATEGIES`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plans_directory: Option<String>,

    /// Scalar field (later source wins). `apiKeyHelper`: path to (or command
    /// line of) a script whose stdout is the Anthropic auth value. CC 2.1.207
    /// zod (verbatim): `apiKeyHelper:E.string().optional().describe("Path to a
    /// script that outputs authentication values")`. Consumed by the auth
    /// executor (`llm_client::oauth::anthropic::run_api_key_helper`, port of
    /// binary `LTh`) with the TTL cache (`api_key_helper_ttl_ms`, port of `obc`).
    /// Scalar-override merge (not in `MERGE_STRATEGIES`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_helper: Option<String>,

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
            ("lingxiMdExcludes", self.lingxi_md_excludes.as_deref()),
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
        let parsed: SettingsJson =
            serde_json::from_str(json).expect("permissions block must parse");
        assert!(parsed.permissions.is_some());
        assert!(
            strategy_for("permissions").is_some(),
            "permissions has a merge strategy"
        );
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
        assert!(
            parsed.model.is_some(),
            "sibling fields must survive the load"
        );
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
            "lingxiMdExcludes",
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
    fn skip_web_fetch_preflight_parses_roundtrips_and_is_scalar_override() {
        // CC 2.1.207 zod: `skipWebFetchPreflight:E.boolean().optional()`. A
        // settings.json carrying `"skipWebFetchPreflight": true` must parse into
        // the typed `Some(true)` (unknown-key tolerance alone would strip it, so
        // the value would never be ACCESSIBLE to the WebFetch tool).
        let json = r#"{ "skipWebFetchPreflight": true, "model": "claude-sonnet-4-5" }"#;
        let parsed: SettingsJson =
            serde_json::from_str(json).expect("skipWebFetchPreflight bool must parse");
        assert_eq!(parsed.skip_web_fetch_preflight, Some(true));
        assert!(
            parsed.model.is_some(),
            "sibling fields must survive the load"
        );
        // camelCase on the wire; round-trips.
        let back = serde_json::to_string(&parsed).expect("serialize");
        assert!(back.contains("\"skipWebFetchPreflight\":true"), "{back}");
        // Absent ⇒ None (distinct from `Some(false)`), and NOT emitted on
        // re-serialize (skip_serializing_if).
        let absent: SettingsJson = serde_json::from_str(r#"{"model":"x"}"#).unwrap();
        assert_eq!(absent.skip_web_fetch_preflight, None);
        assert!(!serde_json::to_string(&absent)
            .unwrap()
            .contains("skipWebFetchPreflight"));
        // Scalar-override merge (later layer wins) — must NOT be a registered
        // ConcatDedup/DeepMerge field (falls through to the default Override).
        assert!(
            strategy_for("skipWebFetchPreflight").is_none(),
            "skipWebFetchPreflight must be scalar-override (later source wins)"
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
    fn aws_gcp_auth_refresh_keys_parse_and_roundtrip() {
        // 2.1.198 settings schema: awsAuthRefresh / awsCredentialExport /
        // gcpAuthRefresh are plain string keys (script paths / commands).
        let json = r#"{
            "awsAuthRefresh": "aws sso login --profile myprofile",
            "awsCredentialExport": "/usr/local/bin/export-aws-creds.sh",
            "gcpAuthRefresh": "gcloud auth application-default login"
        }"#;
        let parsed: SettingsJson = serde_json::from_str(json).expect("parse");
        assert_eq!(
            parsed.aws_auth_refresh.as_deref(),
            Some("aws sso login --profile myprofile")
        );
        assert_eq!(
            parsed.aws_credential_export.as_deref(),
            Some("/usr/local/bin/export-aws-creds.sh")
        );
        assert_eq!(
            parsed.gcp_auth_refresh.as_deref(),
            Some("gcloud auth application-default login")
        );
        // camelCase on the wire (claude-code emits camelCase; scalar-override
        // merge — none of the three is in MERGE_STRATEGIES).
        let back = serde_json::to_string(&parsed).expect("serialize");
        assert!(back.contains("\"awsAuthRefresh\""));
        assert!(back.contains("\"awsCredentialExport\""));
        assert!(back.contains("\"gcpAuthRefresh\""));
        for key in ["awsAuthRefresh", "awsCredentialExport", "gcpAuthRefresh"] {
            assert!(
                strategy_for(key).is_none(),
                "{key} must be scalar-override (later source wins)"
            );
        }
    }

    #[test]
    fn ask_user_question_timeout_parses_and_is_scalar_override() {
        // 2.1.201 settings schema: askUserQuestionTimeout is an enum string
        // (60s|5m|10m|never), scalar-override merge. Default is `never`.
        let json = r#"{ "askUserQuestionTimeout": "5m", "model": "claude-sonnet-4-5" }"#;
        let parsed: SettingsJson =
            serde_json::from_str(json).expect("askUserQuestionTimeout must parse");
        assert_eq!(parsed.ask_user_question_timeout.as_deref(), Some("5m"));
        // Sibling survives the load.
        assert!(parsed.model.is_some());
        // camelCase on the wire + scalar-override (not registered DeepMerge).
        let back = serde_json::to_string(&parsed).expect("serialize");
        assert!(back.contains("\"askUserQuestionTimeout\""));
        assert!(
            strategy_for("askUserQuestionTimeout").is_none(),
            "askUserQuestionTimeout must be scalar-override (later source wins)"
        );
    }

    #[test]
    fn ask_user_question_timeout_absent_is_none() {
        // Absent key ⇒ None (the tool layer resolves absent ⇒ default `never`).
        let parsed: SettingsJson = serde_json::from_str(r#"{"model":"opus"}"#).expect("parse");
        assert!(parsed.ask_user_question_timeout.is_none());
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
    fn company_announcements_parses_roundtrips_and_is_concat_dedup() {
        // CC 2.1.207 zod: `companyAnnouncements:E.array(E.string()).optional()`.
        let json = r#"{ "companyAnnouncements": ["hi", "there"], "model": "x" }"#;
        let parsed: SettingsJson =
            serde_json::from_str(json).expect("companyAnnouncements array must parse");
        assert_eq!(
            parsed.company_announcements.as_deref(),
            Some(&["hi".to_string(), "there".to_string()][..])
        );
        // Sibling survives.
        assert!(parsed.model.is_some());
        // camelCase on the wire; round-trips.
        let back = serde_json::to_string(&parsed).expect("serialize");
        assert!(back.contains("\"companyAnnouncements\":[\"hi\",\"there\"]"), "{back}");
        // Absent ⇒ None, not emitted.
        let absent: SettingsJson = serde_json::from_str(r#"{"model":"x"}"#).unwrap();
        assert_eq!(absent.company_announcements, None);
        assert!(!serde_json::to_string(&absent)
            .unwrap()
            .contains("companyAnnouncements"));
        // Array-merge (concat-dedup), matching CC's array customizer.
        assert!(
            matches!(strategy_for("companyAnnouncements"), Some(MergeStrategy::ConcatDedup)),
            "companyAnnouncements must be ConcatDedup"
        );
    }

    #[test]
    fn plans_directory_and_api_key_helper_parse_roundtrip_and_are_scalar_override() {
        // CC 2.1.207 zod: both are `E.string().optional()`, scalar-override merge.
        let json = r#"{ "plansDirectory": "docs/plans", "apiKeyHelper": "/usr/local/bin/get-key.sh", "model": "x" }"#;
        let parsed: SettingsJson = serde_json::from_str(json).expect("must parse");
        assert_eq!(parsed.plans_directory.as_deref(), Some("docs/plans"));
        assert_eq!(
            parsed.api_key_helper.as_deref(),
            Some("/usr/local/bin/get-key.sh")
        );
        assert!(parsed.model.is_some());
        // camelCase on the wire.
        let back = serde_json::to_string(&parsed).expect("serialize");
        assert!(back.contains("\"plansDirectory\":\"docs/plans\""), "{back}");
        assert!(back.contains("\"apiKeyHelper\":\"/usr/local/bin/get-key.sh\""), "{back}");
        // Absent ⇒ None, not emitted.
        let absent: SettingsJson = serde_json::from_str(r#"{"model":"x"}"#).unwrap();
        assert_eq!(absent.plans_directory, None);
        assert_eq!(absent.api_key_helper, None);
        // Scalar-override merge (not registered ConcatDedup/DeepMerge).
        assert!(strategy_for("plansDirectory").is_none());
        assert!(strategy_for("apiKeyHelper").is_none());
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
