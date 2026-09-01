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
    ("policyHelpers", MergeStrategy::DeepMerge),
    ("spellcheck", MergeStrategy::DeepMerge),
    ("additionalMarketplaces", MergeStrategy::DeepMerge),
    ("enabledPlugins", MergeStrategy::DeepMerge),
    ("pluginConfigs", MergeStrategy::DeepMerge),
    ("extraKnownMarketplaces", MergeStrategy::DeepMerge),
    // NB: `outputStyle` is intentionally NOT here — TS types it as a string and
    // merges it scalar-override (settingsMergeCustomizer special-cases only
    // arrays), so it falls through to the default Override strategy.
    // LingXi extension — deep-merge so multiple settings layers can each
    // declare a subset of provider profiles.
    ("providers", MergeStrategy::DeepMerge),
    // LingXi extension — deep-merge so multiple settings layers can each
    // contribute routing aliases, fallback chains, and retry policy.
    ("routing", MergeStrategy::DeepMerge),
    // LingXi extension — Fusion multi-model deliberation. Deep-merge objects;
    // arrays (panel lists, dimensions, allowedProfiles) replace as a whole.
    ("fusion", MergeStrategy::DeepMerge),
    // Managed model-restriction map — deep-merge (CC `settingsMergeCustomizer`
    // leaves objects to lodash's recursive merge; only specific arrays concat).
    // NB: `availableModels` (array) and `enforceAvailableModels` (scalar) are
    // deliberately NOT here — they fall through to the default Override, matching
    // CC returning the source array for non-concat arrays.
    ("modelOverrides", MergeStrategy::DeepMerge),
    // `vimInsertModeRemaps` — deep-merge, matching `merger::merge`'s
    // `merge_string_map` for this field (a one-level record has no nested
    // structure, so a per-key union with `next` winning IS the deep merge).
    // It was MISSING here while `merge` deep-merged it: the table and the
    // merger disagreed, so `merger::merge_raw_layer` — and through it the
    // desktop settings snapshot — took Override for a field the engine
    // actually unions. (`tracer` was unaffected: it treats `DeepMerge` and
    // `Override` identically, so only a `ConcatDedup` gap would reach it.)
    // Pinned in both directions by
    // `merger::tests::raw_layer_merge_agrees_with_the_typed_merge` and
    // `merger::tests::every_field_merge_combines_is_registered_and_fixtured`.
    ("vimInsertModeRemaps", MergeStrategy::DeepMerge),
    // HTTP-hook security allowlists (H-BIN-12). Both are arrays, and CC's
    // `settingsMergeCustomizer` (`ipe`) concat-dedups EVERY array except
    // `fallbackModel` (`WSm(e,t)=Mo([...e,...t])`); both describe strings say
    // verbatim "Arrays merge across settings sources (same semantics as
    // allowedMcpServers)." So both are ConcatDedup.
    ("allowedHttpHookUrls", MergeStrategy::ConcatDedup),
    ("httpHookAllowedEnvVars", MergeStrategy::ConcatDedup),
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

    /// Object-merge field (deep-merge). Managed helper commands keyed by OS /
    /// fallback scope. Kept opaque until the helper-resolution consumer lands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_helpers: Option<BTreeMap<String, Value>>,

    /// Scalar field (later source wins). Whether Claude.ai skill sync is
    /// enabled. Schema-only until the marketplace sync consumer lands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync_claude_ai_skills: Option<bool>,

    /// Object-merge field (deep-merge). Alias-shaped marketplace declarations
    /// keyed by marketplace name, parallel to `extraKnownMarketplaces`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_marketplaces: Option<BTreeMap<String, Value>>,

    /// Array field (scalar-override merge). Alias-shaped marketplace allowlist,
    /// parallel to `strictKnownMarketplaces`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_marketplaces: Option<Vec<Value>>,

    /// Scalar field (later source wins). Managed gate for command-sourced
    /// plugins. Unset consumer semantics still follow `allowManagedHooksOnly`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disable_command_plugin_sources: Option<bool>,

    /// Object-merge field (deep-merge). Spellcheck configuration block. Kept
    /// opaque until the spellcheck consumer lands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spellcheck: Option<BTreeMap<String, Value>>,

    /// Scalar field (later source wins). `dialogExpiry`: how long permission /
    /// ask-user dialogs stay armed. 2.1.232 `_Vp`:
    /// `["default","60s","5m","10m","never"]`. UI `"default"` is stored as
    /// omitted (`void 0`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dialog_expiry: Option<String>,

    /// Scalar field (later source wins). `modelProposedGoals` enum:
    /// `auto|alwaysAsk|disabled`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_proposed_goals: Option<String>,

    /// Scalar field (later source wins). `keybindingFlavor` enum:
    /// `classic|readline`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keybinding_flavor: Option<String>,

    /// Scalar field (later source wins). Whether the UI may auto-continue at a
    /// usage-limit stop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_continue_at_usage_limit: Option<bool>,

    /// Scalar field (later source wins). `crossSessionInbound`: accept / hold /
    /// refuse messages from other live sessions on this machine. 2.1.232 `bVp`:
    /// `["default","accept","hold","refuse"]`. `"default"` is stored omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cross_session_inbound: Option<String>,

    /// Command prepended to CLI self-spawns. Only user, `--settings`, managed,
    /// and the dedicated environment variables may contribute at the consumer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_wrapper: Option<String>,

    /// Main interactive status-line command configuration. Kept as raw JSON so
    /// forward-compatible Claude fields survive settings composition.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_line: Option<Value>,

    /// Per-task status-line command configuration for running subagents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent_status_line: Option<Value>,

    /// Scalar field (later source wins). `viewMode`: the transcript view mode
    /// applied on startup. CC 2.1.207 settings zod (verbatim):
    /// `viewMode:E.enum(["default","verbose","focus"]).optional().catch(void 0)
    /// .describe("Default transcript view mode on startup")`. Backs the
    /// H-BIN-11 `/focus` command (which toggles `viewMode==="focus"`) and the
    /// `/tui` renderer split. Typed `Option<String>` (like `outputStyle` /
    /// `askUserQuestionTimeout`), tolerant of unknown values via the zod
    /// `.catch(void 0)` — an out-of-enum string round-trips as the raw value and
    /// is treated as `default` by the reader. Scalar-override merge (not in
    /// `MERGE_STRATEGIES`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view_mode: Option<String>,

    /// Scalar field (later source wins). `emojiCompletionEnabled` controls
    /// `:shortcode` suggestions in the interactive composer. Absent defaults
    /// to enabled, matching Claude Code 2.1.217.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emoji_completion_enabled: Option<bool>,

    /// Scalar field (later source wins). When true, retain API-provided
    /// thinking summaries instead of requesting redacted-thinking blocks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_thinking_summaries: Option<bool>,

    /// LingXi extension. When enabled, a primary model without image input
    /// support may use its provider profile's internal vision delegate.
    /// Absence resolves to enabled at the consumer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vision_delegation_enabled: Option<bool>,

    /// Scalar field (later source wins). User opt-in for the feature-gated
    /// proactive agent/mobile push notification surface.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_push_notif_enabled: Option<bool>,

    /// Scalar field (later source wins). When enabled, a literal `ultracode`
    /// token in a submitted prompt emits the Workflow authorization reminder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_keyword_trigger_enabled: Option<bool>,

    /// Scalar field (later source wins). Session-scoped gate for dynamic
    /// workflows. Absence resolves to enabled at the composition root, matching
    /// Claude Code's default-on path when no plan/experiment gate disables it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable_workflows: Option<bool>,

    /// Scalar field (later source wins). `workflowSizeGuideline` controls the
    /// advisory fan-out size appended to the Workflow tool prompt. Valid wire
    /// values are `unrestricted`, `small`, `medium`, and `large`; consumers
    /// normalize unknown values. Absence resolves to `medium` at the use site.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_size_guideline: Option<String>,

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

    /// Scalar field (later source wins). `disableArtifact`: opt out of the
    /// `Artifact` tool (parity 2.1.207 H-BIN-03). CC 2.1.207 settings zod
    /// (verbatim): `disableArtifact:E.boolean().optional().describe("Disable the
    /// Artifact tool (also via CLAUDE_CODE_DISABLE_ARTIFACT).")`. The env half
    /// (`CLAUDE_CODE_DISABLE_ARTIFACT`) is wired in `tool_api::artifact_gate`
    /// (binary `R9i()`); threading this settings value into that gate lands with
    /// the Stage-2 publish pipeline. Scalar-override merge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disable_artifact: Option<bool>,

    /// Scalar field (later source wins). `enableArtifact`: explicitly enable /
    /// disable the `Artifact` tool for this user (parity 2.1.207 H-BIN-03). CC
    /// 2.1.207 settings zod (verbatim): `enableArtifact:E.boolean().optional()
    /// .describe("Enable or disable the Artifact tool for this user. Unset
    /// defaults to enabled once the feature is available.")`. Read by the tool
    /// gate's `P7t() ?? L7t()` tail (binary): when set it wins over the default,
    /// when unset the tool is enabled once the `tengu_cobalt_plinth` gate is
    /// available. Schema-only today (the gate's live settings read is Stage-2);
    /// the key round-trips so it is ACCESSIBLE. Scalar-override merge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable_artifact: Option<bool>,

    /// Scalar field (later source wins). `disableAgentView`: disable the agent
    /// view surface (`claude agents`, `--bg`, `/background`, the on-demand
    /// daemon) and the redefined background-session `/fork` + `/subtask`. CC
    /// 2.1.215 settings zod (verbatim describe): "Disable agent view (`claude
    /// agents`, `--bg`, /background, the on-demand daemon). Typically set in
    /// managed settings. Equivalent to CLAUDE_CODE_DISABLE_AGENT_VIEW=1." Read by
    /// `I2i()` (the `vO()` gate) alongside the `CLAUDE_CODE_DISABLE_AGENT_VIEW`
    /// env var: `settings.disableAgentView === true` disables agent view exactly
    /// like a truthy env var. Threaded into command registration via
    /// [`platform_api::agent_view::is_enabled_with_setting`] (see
    /// `command_core::register_core_batch_8`). Scalar-override merge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disable_agent_view: Option<bool>,

    /// `disableAllHooks` — when true, ALL hooks are disabled (claude hook-dispatch
    /// gate `Ql()`). One half of the `/goal` hooks-restricted gate (`kEt`): a
    /// restricted-hooks session rejects `/goal`. Scalar-override merge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disable_all_hooks: Option<bool>,

    /// `allowManagedHooksOnly` — when true, only managed-policy hooks run. The
    /// other half of the `/goal` hooks-restricted gate (claude
    /// `allowManagedHooksOnly===!0`). Scalar-override merge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_managed_hooks_only: Option<bool>,

    /// Scalar field (later source wins). Default model alias.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    /// Array field (scalar-override merge). `availableModels`: managed allowlist
    /// of models a user may select. CC 2.1.207 settings zod (verbatim describe):
    /// "Allowlist of models that users can select. Accepts family aliases
    /// (\"opus\" allows any opus version), version prefixes (\"opus-4-5\" allows
    /// only that version), and full model IDs. If undefined, all models are
    /// available. If empty array, only the default model is available."
    /// Typically set in managed (`policySettings`) settings by enterprise
    /// administrators. Consumed via [`llm_client::model::allowlist`] — matcher +
    /// policy-provenance for the `enforceAvailableModels` gate. Absent ⇒ None
    /// (no restriction), distinct from `Some(vec![])` (only the default model).
    /// Scalar-override merge (CC `settingsMergeCustomizer` returns the source
    /// array for non-concat arrays), so NOT in `MERGE_STRATEGIES`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub available_models: Option<Vec<String>>,

    /// Scalar field (later source wins). `enforceAvailableModels`: gate on the
    /// managed `availableModels` allowlist. CC 2.1.207 zod (verbatim describe):
    /// "When true and availableModels is a non-empty array, the Default model
    /// selection is also constrained: if the default model for the user tier is
    /// not in availableModels, Default resolves to the first allowed
    /// availableModels entry instead. Has no effect when availableModels is unset
    /// or an empty array. Typically set in managed settings by enterprise
    /// administrators." The flag only binds with a POLICY-owned allowlist
    /// (`llm_client::model::allowlist::resolve_enforcement`). Scalar-override
    /// merge (not in `MERGE_STRATEGIES`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enforce_available_models: Option<bool>,

    /// Object-merge field (deep-merge). `modelOverrides`: managed mapping from
    /// Anthropic model ID to provider-specific model ID. CC 2.1.207 zod
    /// (verbatim describe): "Override mapping from Anthropic model ID (e.g.
    /// \"claude-opus-4-6\") to provider-specific model ID (e.g. a Bedrock
    /// inference profile ARN). Typically set in managed settings by enterprise
    /// administrators." Feeds the same `availableModels` enforcement path
    /// (reverse-mapped in the matcher, `overridesMap`). Deep-merged across tiers
    /// (CC's `settingsMergeCustomizer` leaves objects to lodash's recursive
    /// merge).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_overrides: Option<BTreeMap<String, String>>,

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

    // ── HTTP-hook security allowlists (H-BIN-12) ─────────────────────────────
    // Two CC 2.1.207 settings keys the HTTP hook executor reads live per
    // execution (`PFy()=Wn()`), consumed by `hooks::HttpExecutor` — the URL
    // allowlist gate + the per-hook env-var intersection. Both are array-merge
    // (ConcatDedup, see `MERGE_STRATEGIES`).
    /// Array-merge field (concat-dedup). `allowedHttpHookUrls`: allowlist of URL
    /// patterns HTTP hooks may target. CC 2.1.207 zod (verbatim describe):
    /// "Allowlist of URL patterns that HTTP hooks may target. Supports * as a
    /// wildcard (e.g. \"https://hooks.example.com/*\"). When set, HTTP hooks with
    /// non-matching URLs are blocked. If undefined, all URLs are allowed. If
    /// empty array, no HTTP hooks are allowed. Arrays merge across settings
    /// sources (same semantics as allowedMcpServers)." Consumed by the HTTP hook
    /// executor via [`crate::settings::schema`] → the `hooks` crate's
    /// `HttpExecutor`: `None` ⇒ all URLs allowed; `Some(empty)` ⇒ block ALL HTTP
    /// hooks; `Some(patterns)` ⇒ the hook URL must match ≥1 pattern (CC `NBr`
    /// wildcard matcher) or the request is blocked before dispatch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_http_hook_urls: Option<Vec<String>>,

    /// Array-merge field (concat-dedup). `httpHookAllowedEnvVars`: allowlist of
    /// environment variable names HTTP hooks may interpolate into headers. CC
    /// 2.1.207 zod (verbatim describe): "Allowlist of environment variable names
    /// HTTP hooks may interpolate into headers. When set, each hook's effective
    /// allowedEnvVars is the intersection with this list. If undefined, no
    /// restriction is applied. Arrays merge across settings sources (same
    /// semantics as allowedMcpServers)." Consumed by the HTTP hook executor:
    /// `None` ⇒ per-hook `allowedEnvVars` used as-is; `Some(list)` ⇒ each hook's
    /// effective allowlist is its own `allowedEnvVars` ∩ this global list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_hook_allowed_env_vars: Option<Vec<String>>,

    // ── Enterprise login / version managed-policy keys (H-BIN-09) ────────────
    // Six admin-provisioned `managed-settings.json` keys CC 2.1.207 both schemas
    // AND enforces; consumed via [`crate::settings::enterprise`]. All scalar-
    // override merge (none in `MERGE_STRATEGIES`). See that module for wiring
    // status (version gate LIVE; login-flow method-lock/org-pin DORMANT).
    /// Scalar field (later source wins). `forceLoginMethod`: force a specific
    /// OAuth login method. CC 2.1.207 zod (verbatim describe): "Force a specific
    /// login method: \"claudeai\" for Claude Pro/Max, \"console\" for Console
    /// billing, \"gateway\" for the Cloud gateway OIDC device flow". Enum
    /// `claudeai|console|gateway` with `.catch(void 0)` — an out-of-set value
    /// degrades to "no forced method". Typed `Option<String>` (like
    /// `askUserQuestionTimeout`); the enum + `.catch` degrade live in the typed
    /// accessor [`SettingsJson::force_login_method_parsed`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub force_login_method: Option<String>,

    /// Scalar field (later source wins). `forceLoginGatewayUrl`: Cloud gateway
    /// URL to pre-fill and auto-connect to during login. CC 2.1.207 zod:
    /// `forceLoginGatewayUrl:E.string().url().optional().catch(void 0)`
    /// ("@internal Cloud gateway URL to pre-fill and auto-connect to during
    /// login. Typically set in local managed settings alongside forceLoginMethod:
    /// \"gateway\" so users never type the URL."). Pre-filled into the OAuth
    /// screen by the login flow (DORMANT wiring — see `enterprise`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub force_login_gateway_url: Option<String>,

    /// Scalar field (later source wins). `forceLoginOrgUUID`: pin OAuth login to
    /// an organization (or list). CC 2.1.207 zod:
    /// `forceLoginOrgUUID:E.union([E.string(),E.array(E.string())]).optional()`
    /// ("Organization UUID to require for OAuth login. Accepts a single UUID
    /// string or an array of UUIDs (any one is permitted). When set in managed
    /// settings, login fails if the authenticated account does not belong to a
    /// listed organization."). Kept as opaque `Value` (string | string[]) so a
    /// malformed value is TOLERATED at load and degraded at the consumer
    /// ([`SettingsJson::force_login_org_pin`]) rather than failing the whole
    /// settings file.
    ///
    /// NB explicit `rename`: serde's `camelCase` rule would emit `forceLoginOrgUuid`,
    /// but the CC wire key is the all-caps `forceLoginOrgUUID`.
    #[serde(
        rename = "forceLoginOrgUUID",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub force_login_org_uuid: Option<Value>,

    /// Scalar field (later source wins). `parentSettingsBehavior`: whether the
    /// SDK parent managed-settings tier (`Options.managedSettings` /
    /// `--managed-settings`) layers under this admin tier. CC 2.1.207 zod enum
    /// `first-wins|merge` ("first-wins (default): parent is dropped … merge:
    /// parent's restrictive-only-filtered settings union under the admin
    /// winner."). Consumed by [`crate::settings::enterprise::should_merge_parent_settings`]
    /// (the SDK parent tier itself is not yet modeled in lingxi).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_settings_behavior: Option<String>,

    /// Scalar field (later source wins). `minimumVersion`: USER setting — CC
    /// 2.1.207 zod: "Minimum version to stay on - prevents downgrades when
    /// switching to stable channel". Consumed ONLY by CC's auto-updater
    /// channel-downgrade guard (`Zlo`/`Wn()`); lingxi has no auto-updater, so
    /// this is schema-only (dead-by-missing-subsystem) — do NOT confuse with the
    /// managed startup gate `requiredMinimumVersion`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum_version: Option<String>,

    /// Scalar field (later source wins). `requiredMinimumVersion`: managed
    /// startup exit gate — CC 2.1.207 zod: "Minimum Claude Code version required
    /// to start. If the running version is older, Claude Code exits at startup
    /// with instructions to update. Only enforced from managed (policy)
    /// settings." Consumed by [`crate::settings::enterprise::version_gate`]
    /// (LIVE, wired at CLI boot).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_minimum_version: Option<String>,

    /// Scalar field (later source wins). `requiredMaximumVersion`: managed
    /// startup exit gate — CC 2.1.207 zod: "Maximum Claude Code version allowed
    /// to start. If the running version is newer, Claude Code exits at startup
    /// with instructions to install an approved version. Only enforced from
    /// managed (policy) settings." Ships with `requiredMinimumVersion` in the
    /// same `version_gate`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_maximum_version: Option<String>,

    /// Scalar field (later source wins). `forceRemoteSettingsRefresh`: CC 2.1.207
    /// zod: "When set in managed settings, the CLI blocks startup until remote
    /// managed settings are freshly fetched, and exits if the fetch fails."
    /// Schema-only — lingxi has no remote managed-settings fetcher; the key is
    /// typed so it round-trips and is ACCESSIBLE when that subsystem lands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub force_remote_settings_refresh: Option<bool>,

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

    /// Object-merge field (deep-merge, `MERGE_STRATEGIES`). `vimInsertModeRemaps`: two-key insert-mode sequences
    /// to key names. Claude Code 2.1.208 uses this for common Vim insert-exit
    /// mappings, for example `{ "jj": "Escape" }`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vim_insert_mode_remaps: Option<BTreeMap<String, String>>,

    /// Scalar field (later source wins). `otelHeadersHelper`: path to (or shell
    /// command for) a script whose stdout is a JSON object of OTLP export header
    /// `k:v` strings — used to inject short-lived bearer tokens into the
    /// OpenTelemetry monitoring exporters (parity 2.1.207 H-BIN-06). Read by the
    /// binary `otelHeadersHelper` runner (`RRi()`/`wRi()`), which validates the
    /// output ("must return a JSON object with string key-value pairs"), caches
    /// it, and re-invokes at most once per
    /// `CLAUDE_CODE_OTEL_HEADERS_HELPER_DEBOUNCE_MS` window. The validation +
    /// debounce state machine lives in `telemetry::otel::headers_helper`; wiring
    /// this value into the live export path is the H-BIN-06 egress remainder.
    /// The key round-trips so it is ACCESSIBLE. Scalar-override merge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub otel_headers_helper: Option<String>,

    /// Object-merge field. On-disk enabled plugin allowlist keyed by
    /// `plugin@marketplace`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled_plugins: Option<BTreeMap<String, Value>>,

    /// Object-merge field. Non-sensitive plugin config records keyed by
    /// `plugin@marketplace`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_configs: Option<BTreeMap<String, Value>>,

    /// Object-merge field. Extra marketplace declarations keyed by marketplace
    /// name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra_known_marketplaces: Option<BTreeMap<String, Value>>,

    /// Array field (scalar-override merge). Managed allowlist of known
    /// marketplaces; entries may be strings or structured source objects.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strict_known_marketplaces: Option<Vec<Value>>,

    /// Array field (scalar-override merge). Managed blocklist of marketplace
    /// names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_marketplaces: Option<Vec<String>>,

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

    /// Object-merge field (deep-merge). LingXi extension: Fusion multi-model
    /// deliberation. Absent ⇒ Fusion stays disabled (default `enabled: false`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fusion: Option<FusionSettingsJson>,
}

/// Typed `settings.fusion` object. Every field is `Option` so a partial layer
/// can set a subset. Runtime defaults are applied by the Fusion orchestrator;
/// [`SettingsJson::validate`] rejects values that would be dangerous if used.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FusionSettingsJson {
    /// Master switch for Agent listing + workflow `fusion()`. Default false.
    /// When false, workflow `fusion()` rejects before any provider call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// `quality` or `fast`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    /// Quality preset panel count (min 2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality_panel_count: Option<u8>,
    /// Fast preset panel count (min 2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fast_panel_count: Option<u8>,
    /// Hard cap 2..=8.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_panel: Option<u8>,
    /// Minimum successful panels before analysis.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_successful_panels: Option<u8>,
    /// Continue when some panels fail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partial_ok: Option<bool>,
    /// Per-panel turn cap (1..=32).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub panel_max_turns: Option<u32>,
    /// Per-turn output token cap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub panel_max_output_tokens_per_turn: Option<u32>,
    /// Per-turn reserved input tokens (not 1 byte = 1 token).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub panel_reserved_input_tokens_per_turn: Option<u32>,
    /// Optional hard reservation ceiling in nano-USD. This caps reserved spend
    /// up front; realized cost may end lower.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_reserved_nano_usd: Option<u64>,
    /// Analyst output cap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub analyst_max_output_tokens: Option<u32>,
    /// Synthesizer output cap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub synthesizer_max_output_tokens: Option<u32>,
    /// Panel idle timeout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub panel_idle_timeout_ms: Option<u64>,
    /// Panel total timeout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub panel_total_timeout_ms: Option<u64>,
    /// Analyst timeout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub analyst_timeout_ms: Option<u64>,
    /// Synthesizer timeout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub synthesizer_timeout_ms: Option<u64>,
    /// End-to-end timeout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_timeout_ms: Option<u64>,
    /// Analyst protocol retries (0 or 1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub analysis_protocol_retries: Option<u8>,
    /// `/fusion` default cross-provider. `true` allows prompt egress beyond the
    /// parent provider/profile unless the caller explicitly requests same-provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slash_cross_provider_default: Option<bool>,
    /// Agent may request cross-provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_cross_provider_for_agent: Option<bool>,
    /// Workflow may request cross-provider. When false, an explicit
    /// `fusion(..., { crossProvider: true })` must reject instead of silently
    /// downgrading to same-provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_cross_provider_for_workflow: Option<bool>,
    /// Hard allowlist of profile names. Empty = no extra restriction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_profiles: Option<Vec<String>>,
    /// Per-workflow `fusion()` call cap. Hard-clamped to `1..=20`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_fusion_call_cap: Option<u32>,
}

impl FusionSettingsJson {
    /// Reject values that would make Fusion unusable or unbounded.
    ///
    /// Absent fields are not errors — the orchestrator fills runtime defaults.
    /// Invalid *present* values fail the settings load.
    pub fn validate(&self) -> Result<(), crate::settings::SettingsError> {
        use crate::settings::SettingsError::SchemaViolation;
        if let Some(preset) = self.preset.as_deref() {
            if preset != "quality" && preset != "fast" {
                return Err(SchemaViolation(format!(
                    "fusion.preset `{preset}` must be quality or fast"
                )));
            }
        }
        let max_panel = self.max_panel.unwrap_or(8);
        if !(2..=8).contains(&max_panel) {
            return Err(SchemaViolation("fusion.maxPanel must be in 2..=8".into()));
        }
        for (name, value) in [
            ("fusion.qualityPanelCount", self.quality_panel_count),
            ("fusion.fastPanelCount", self.fast_panel_count),
            ("fusion.minSuccessfulPanels", self.min_successful_panels),
        ] {
            if let Some(n) = value {
                if n < 2 {
                    return Err(SchemaViolation(format!("{name} must be at least 2")));
                }
                if n > max_panel {
                    return Err(SchemaViolation(format!(
                        "{name} must not exceed fusion.maxPanel"
                    )));
                }
            }
        }
        if let Some(turns) = self.panel_max_turns {
            if !(1..=32).contains(&turns) {
                return Err(SchemaViolation(
                    "fusion.panelMaxTurns must be in 1..=32".into(),
                ));
            }
        }
        for (name, value) in [
            (
                "fusion.panelMaxOutputTokensPerTurn",
                self.panel_max_output_tokens_per_turn,
            ),
            (
                "fusion.panelReservedInputTokensPerTurn",
                self.panel_reserved_input_tokens_per_turn,
            ),
            (
                "fusion.analystMaxOutputTokens",
                self.analyst_max_output_tokens,
            ),
            (
                "fusion.synthesizerMaxOutputTokens",
                self.synthesizer_max_output_tokens,
            ),
        ] {
            if let Some(n) = value {
                if n == 0 {
                    return Err(SchemaViolation(format!("{name} must be positive")));
                }
            }
        }
        if let Some(retries) = self.analysis_protocol_retries {
            if retries > 1 {
                return Err(SchemaViolation(
                    "fusion.analysisProtocolRetries must be 0 or 1".into(),
                ));
            }
        }
        let total = self.total_timeout_ms;
        for (name, value) in [
            ("fusion.panelIdleTimeoutMs", self.panel_idle_timeout_ms),
            ("fusion.panelTotalTimeoutMs", self.panel_total_timeout_ms),
            ("fusion.analystTimeoutMs", self.analyst_timeout_ms),
            ("fusion.synthesizerTimeoutMs", self.synthesizer_timeout_ms),
        ] {
            if let (Some(total), Some(stage)) = (total, value) {
                if stage > total {
                    return Err(SchemaViolation(format!(
                        "{name} must not exceed fusion.totalTimeoutMs"
                    )));
                }
            }
            if let Some(stage) = value {
                if stage == 0 {
                    return Err(SchemaViolation(format!("{name} must be positive")));
                }
            }
        }
        if let Some(cap) = self.workflow_fusion_call_cap {
            if cap == 0 || cap > 20 {
                return Err(SchemaViolation(
                    "fusion.workflowFusionCallCap must be in 1..=20".into(),
                ));
            }
        }
        Ok(())
    }
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
        validate_enum(
            "modelProposedGoals",
            self.model_proposed_goals.as_deref(),
            &["auto", "alwaysAsk", "disabled"],
        )?;
        validate_enum(
            "keybindingFlavor",
            self.keybinding_flavor.as_deref(),
            &["classic", "readline"],
        )?;
        if let Some(fusion) = self.fusion.as_ref() {
            fusion.validate()?;
        }
        Ok(())
    }

    /// Effective `disableCommandPluginSources` value. When unset, this follows
    /// the managed hook restriction flag exactly like Claude Code 2.1.238.
    #[must_use]
    pub fn effective_disable_command_plugin_sources(&self) -> bool {
        self.disable_command_plugin_sources
            .unwrap_or(self.allow_managed_hooks_only == Some(true))
    }

    /// Effective extra marketplace declarations, honoring the 2.1.238 alias
    /// key `additionalMarketplaces`. When both spellings are present, the old
    /// canonical key wins and the alias is ignored with a warning.
    pub fn effective_extra_known_marketplaces<F>(
        &self,
        mut warn: F,
    ) -> Option<BTreeMap<String, Value>>
    where
        F: FnMut(&str),
    {
        match (
            self.extra_known_marketplaces.as_ref(),
            self.additional_marketplaces.as_ref(),
        ) {
            (Some(canonical), Some(_alias)) => {
                warn(
                    "additionalMarketplaces is ignored because extraKnownMarketplaces is also set",
                );
                Some(canonical.clone())
            }
            (Some(canonical), None) => Some(canonical.clone()),
            (None, Some(alias)) => Some(alias.clone()),
            (None, None) => None,
        }
    }

    /// Effective strict marketplace allowlist, honoring the 2.1.238 alias key
    /// `allowedMarketplaces`. When both spellings are present, the old
    /// canonical key wins and the alias is ignored with a warning.
    pub fn effective_strict_known_marketplaces<F>(&self, mut warn: F) -> Option<Vec<Value>>
    where
        F: FnMut(&str),
    {
        match (
            self.strict_known_marketplaces.as_ref(),
            self.allowed_marketplaces.as_ref(),
        ) {
            (Some(canonical), Some(_alias)) => {
                warn("allowedMarketplaces is ignored because strictKnownMarketplaces is also set");
                Some(canonical.clone())
            }
            (Some(canonical), None) => Some(canonical.clone()),
            (None, Some(alias)) => Some(alias.clone()),
            (None, None) => None,
        }
    }
}

fn validate_enum(
    field_name: &str,
    value: Option<&str>,
    allowed: &[&str],
) -> Result<(), crate::settings::SettingsError> {
    let Some(value) = value else {
        return Ok(());
    };
    if allowed.contains(&value) {
        return Ok(());
    }
    Err(crate::settings::SettingsError::SchemaViolation(format!(
        "{field_name} must be one of {} (got {value:?})",
        allowed.join(", ")
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_push_notification_setting_parses() {
        let parsed: SettingsJson =
            serde_json::from_str(r#"{"agentPushNotifEnabled":true}"#).unwrap();
        assert_eq!(parsed.agent_push_notif_enabled, Some(true));
    }

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
    fn deserializes_workflow_size_guideline() {
        let parsed: SettingsJson =
            serde_json::from_str(r#"{"workflowSizeGuideline":"medium"}"#).unwrap();
        assert_eq!(parsed.workflow_size_guideline.as_deref(), Some("medium"));
    }

    #[test]
    fn deserializes_enable_workflows() {
        let parsed: SettingsJson = serde_json::from_str(r#"{"enableWorkflows":false}"#).unwrap();
        assert_eq!(parsed.enable_workflows, Some(false));
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
    fn view_mode_parses_roundtrips_and_is_scalar_override() {
        // H-BIN-11 (cc2.1.207) zod:
        // `viewMode:E.enum(["default","verbose","focus"]).optional()
        //  .catch(void 0).describe("Default transcript view mode on startup")`.
        // A settings.json carrying `"viewMode": "focus"` must parse into the
        // typed `Some("focus")` so `/focus`/`/tui` can read it.
        for value in ["default", "verbose", "focus"] {
            let json = format!(r#"{{ "viewMode": "{value}", "model": "claude-sonnet-4-5" }}"#);
            let parsed: SettingsJson =
                serde_json::from_str(&json).expect("viewMode enum value must parse");
            assert_eq!(parsed.view_mode.as_deref(), Some(value));
            assert!(parsed.model.is_some(), "sibling fields must survive");
        }
        // camelCase on the wire; round-trips.
        let parsed: SettingsJson = serde_json::from_str(r#"{"viewMode":"focus"}"#).unwrap();
        let back = serde_json::to_string(&parsed).expect("serialize");
        assert!(back.contains("\"viewMode\":\"focus\""), "{back}");
        // zod `.catch(void 0)` tolerance: an out-of-enum value still parses
        // (the string round-trips; the reader treats it as `default`).
        let odd: SettingsJson =
            serde_json::from_str(r#"{"viewMode":"bogus","model":"x"}"#).expect("catch tolerance");
        assert_eq!(odd.view_mode.as_deref(), Some("bogus"));
        assert!(
            odd.model.is_some(),
            "sibling fields survive an odd viewMode"
        );
        // Absent ⇒ None, and NOT emitted on re-serialize (skip_serializing_if).
        let absent: SettingsJson = serde_json::from_str(r#"{"model":"x"}"#).unwrap();
        assert_eq!(absent.view_mode, None);
        assert!(!serde_json::to_string(&absent).unwrap().contains("viewMode"));
        // Scalar-override merge (later layer wins) — NOT a registered
        // ConcatDedup/DeepMerge field.
        assert!(
            strategy_for("viewMode").is_none(),
            "viewMode must be scalar-override (later source wins)"
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
    fn new_238_settings_keys_parse_roundtrip_and_validate() {
        let json = r#"{
            "policyHelpers": {
                "defaultSettings": { "command": "/usr/local/bin/policy-helper" },
                "darwin": { "command": "/opt/policy-helper" }
            },
            "syncClaudeAiSkills": true,
            "additionalMarketplaces": {
                "corp": { "source": { "source": "directory", "path": "/tmp/corp" } }
            },
            "allowedMarketplaces": [
                "corp",
                { "source": "github", "repo": "acme/plugins" }
            ],
            "disableCommandPluginSources": true,
            "spellcheck": { "enabled": true, "checkFilenames": false },
            "dialogExpiry": "5m",
            "modelProposedGoals": "alwaysAsk",
            "keybindingFlavor": "readline",
            "autoContinueAtUsageLimit": false,
            "crossSessionInbound": "hold"
        }"#;
        let parsed: SettingsJson = serde_json::from_str(json).expect("parse");
        assert_eq!(parsed.sync_claude_ai_skills, Some(true));
        assert_eq!(
            parsed
                .additional_marketplaces
                .as_ref()
                .and_then(|m| m.get("corp")),
            Some(&serde_json::json!({
                "source": { "source": "directory", "path": "/tmp/corp" }
            }))
        );
        assert_eq!(
            parsed.allowed_marketplaces.as_deref(),
            Some(
                &[
                    serde_json::json!("corp"),
                    serde_json::json!({ "source": "github", "repo": "acme/plugins" })
                ][..]
            )
        );
        assert_eq!(parsed.disable_command_plugin_sources, Some(true));
        assert_eq!(parsed.model_proposed_goals.as_deref(), Some("alwaysAsk"));
        assert_eq!(parsed.keybinding_flavor.as_deref(), Some("readline"));
        assert_eq!(parsed.auto_continue_at_usage_limit, Some(false));
        assert!(parsed.policy_helpers.is_some());
        assert!(parsed.spellcheck.is_some());
        parsed.validate().expect("enum values should validate");

        let back = serde_json::to_string(&parsed).expect("serialize");
        for key in [
            "policyHelpers",
            "syncClaudeAiSkills",
            "additionalMarketplaces",
            "allowedMarketplaces",
            "disableCommandPluginSources",
            "spellcheck",
            "dialogExpiry",
            "modelProposedGoals",
            "keybindingFlavor",
            "autoContinueAtUsageLimit",
            "crossSessionInbound",
        ] {
            assert!(
                back.contains(&format!("\"{key}\"")),
                "missing {key}: {back}"
            );
        }
        let ordered_keys = [
            "policyHelpers",
            "syncClaudeAiSkills",
            "additionalMarketplaces",
            "allowedMarketplaces",
            "disableCommandPluginSources",
            "spellcheck",
            "dialogExpiry",
            "modelProposedGoals",
            "keybindingFlavor",
            "autoContinueAtUsageLimit",
            "crossSessionInbound",
        ];
        let positions = ordered_keys
            .iter()
            .map(|key| back.find(&format!("\"{key}\"")).expect("serialized key"))
            .collect::<Vec<_>>();
        assert!(
            positions.windows(2).all(|pair| pair[0] < pair[1]),
            "2.1.238 settings keys must serialize in oracle schema order: {back}"
        );
        assert_eq!(
            strategy_for("policyHelpers"),
            Some(MergeStrategy::DeepMerge)
        );
        assert_eq!(strategy_for("spellcheck"), Some(MergeStrategy::DeepMerge));
        assert_eq!(
            strategy_for("additionalMarketplaces"),
            Some(MergeStrategy::DeepMerge)
        );
        for key in [
            "syncClaudeAiSkills",
            "allowedMarketplaces",
            "disableCommandPluginSources",
            "modelProposedGoals",
            "keybindingFlavor",
            "autoContinueAtUsageLimit",
        ] {
            assert!(
                strategy_for(key).is_none(),
                "{key} must remain scalar-override"
            );
        }
    }

    #[test]
    fn validate_rejects_invalid_new_238_enums() {
        let bad_keybinding: SettingsJson =
            serde_json::from_str(r#"{"keybindingFlavor":"vim"}"#).expect("parse");
        let err = bad_keybinding
            .validate()
            .expect_err("invalid enum must fail");
        assert!(format!("{err:?}").contains("keybindingFlavor"));

        let bad_goals: SettingsJson =
            serde_json::from_str(r#"{"modelProposedGoals":"sometimes"}"#).expect("parse");
        let err = bad_goals.validate().expect_err("invalid enum must fail");
        assert!(format!("{err:?}").contains("modelProposedGoals"));
    }

    #[test]
    fn new_238_alias_helpers_and_disable_command_default_follow_existing_surface() {
        let parsed: SettingsJson = serde_json::from_str(
            r#"{
                "allowManagedHooksOnly": true,
                "additionalMarketplaces": {
                    "alias": { "source": { "source": "directory", "path": "/tmp/alias" } }
                },
                "allowedMarketplaces": ["alias"]
            }"#,
        )
        .expect("parse");
        assert!(parsed.effective_disable_command_plugin_sources());

        let mut warnings = Vec::new();
        let extra =
            parsed.effective_extra_known_marketplaces(|warning| warnings.push(warning.to_string()));
        assert_eq!(
            extra.as_ref().and_then(|m| m.get("alias")),
            Some(&serde_json::json!({
                "source": { "source": "directory", "path": "/tmp/alias" }
            }))
        );
        let allowed = parsed
            .effective_strict_known_marketplaces(|warning| warnings.push(warning.to_string()));
        assert_eq!(allowed, Some(vec![serde_json::json!("alias")]));
        assert!(warnings.is_empty());

        let parsed: SettingsJson = serde_json::from_str(
            r#"{
                "allowManagedHooksOnly": false,
                "disableCommandPluginSources": false,
                "extraKnownMarketplaces": {
                    "canonical": { "source": { "source": "directory", "path": "/tmp/canonical" } }
                },
                "additionalMarketplaces": {
                    "alias": { "source": { "source": "directory", "path": "/tmp/alias" } }
                },
                "strictKnownMarketplaces": ["canonical"],
                "allowedMarketplaces": ["alias"]
            }"#,
        )
        .expect("parse");
        assert!(!parsed.effective_disable_command_plugin_sources());

        let mut warnings = Vec::new();
        let extra =
            parsed.effective_extra_known_marketplaces(|warning| warnings.push(warning.to_string()));
        let allowed = parsed
            .effective_strict_known_marketplaces(|warning| warnings.push(warning.to_string()));
        assert_eq!(
            extra.as_ref().and_then(|m| m.get("canonical")),
            Some(&serde_json::json!({
                "source": { "source": "directory", "path": "/tmp/canonical" }
            }))
        );
        assert_eq!(allowed, Some(vec![serde_json::json!("canonical")]));
        assert_eq!(
            warnings,
            vec![
                "additionalMarketplaces is ignored because extraKnownMarketplaces is also set",
                "allowedMarketplaces is ignored because strictKnownMarketplaces is also set",
            ]
        );
    }

    #[test]
    fn plugin_and_marketplace_settings_parse_roundtrip_and_merge_correctly() {
        let json = r#"{
            "enabledPlugins": { "demo@example": true },
            "pluginConfigs": {
                "demo@example": { "options": { "REGION": "us-east-1" } }
            },
            "extraKnownMarketplaces": {
                "team": { "source": { "type": "directory", "path": "/tmp/team" } }
            },
            "strictKnownMarketplaces": [
                { "source": "github", "repo": "acme/plugins" }
            ],
            "blockedMarketplaces": ["deprecated"],
            "model": "claude-sonnet-4-5"
        }"#;
        let parsed: SettingsJson = serde_json::from_str(json).expect("parse");
        assert!(parsed.enabled_plugins.is_some());
        assert!(parsed.plugin_configs.is_some());
        assert!(parsed.extra_known_marketplaces.is_some());
        assert!(parsed.strict_known_marketplaces.is_some());
        assert_eq!(
            parsed.blocked_marketplaces.as_deref(),
            Some(&["deprecated".to_string()][..])
        );

        let back = serde_json::to_string(&parsed).expect("serialize");
        for key in [
            "enabledPlugins",
            "pluginConfigs",
            "extraKnownMarketplaces",
            "strictKnownMarketplaces",
            "blockedMarketplaces",
        ] {
            assert!(
                back.contains(&format!("\"{key}\"")),
                "missing {key}: {back}"
            );
        }

        for key in ["enabledPlugins", "pluginConfigs", "extraKnownMarketplaces"] {
            assert_eq!(
                strategy_for(key),
                Some(MergeStrategy::DeepMerge),
                "{key} must deep-merge"
            );
        }
        for key in ["strictKnownMarketplaces", "blockedMarketplaces"] {
            assert!(
                strategy_for(key).is_none(),
                "{key} must remain scalar-override"
            );
        }
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
    fn otel_headers_helper_key_parses_and_roundtrips() {
        // 2.1.207 Monitoring schema (H-BIN-06): otelHeadersHelper is a plain
        // string key (script path / shell command). camelCase on the wire.
        let json = r#"{ "otelHeadersHelper": "/opt/otel/get-headers.sh" }"#;
        let parsed: SettingsJson = serde_json::from_str(json).expect("parse");
        assert_eq!(
            parsed.otel_headers_helper.as_deref(),
            Some("/opt/otel/get-headers.sh")
        );
        // Round-trips under the exact camelCase wire key CC emits/reads.
        let back = serde_json::to_string(&parsed).expect("serialize");
        assert!(
            back.contains("\"otelHeadersHelper\""),
            "must serialize back to the byte-exact camelCase key"
        );
        // Scalar-override merge (later source wins; not in MERGE_STRATEGIES).
        assert!(strategy_for("otelHeadersHelper").is_none());
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
    fn managed_model_allowlist_keys_parse_and_roundtrip() {
        // CC 2.1.207 managed model-restriction keys: availableModels (array),
        // enforceAvailableModels (bool), modelOverrides (record). A managed
        // settings.json carrying them must parse into the typed fields (unknown-
        // key tolerance alone would strip them, so the allowlist would be
        // silently dropped and enforcement never bind).
        let json = r#"{
            "availableModels": ["opus", "claude-sonnet-4-5"],
            "enforceAvailableModels": true,
            "modelOverrides": { "claude-opus-4-6": "arn:aws:bedrock:us-east-1::inference-profile/opus" },
            "model": "claude-sonnet-4-5"
        }"#;
        let parsed: SettingsJson = serde_json::from_str(json).expect("managed keys must parse");
        assert_eq!(
            parsed.available_models.as_deref(),
            Some(&["opus".to_string(), "claude-sonnet-4-5".to_string()][..])
        );
        assert_eq!(parsed.enforce_available_models, Some(true));
        assert_eq!(
            parsed
                .model_overrides
                .as_ref()
                .and_then(|m| m.get("claude-opus-4-6"))
                .map(String::as_str),
            Some("arn:aws:bedrock:us-east-1::inference-profile/opus")
        );
        // Sibling survives the load.
        assert!(parsed.model.is_some());
        // camelCase on the wire; round-trips.
        let back = serde_json::to_string(&parsed).expect("serialize");
        assert!(back.contains("\"availableModels\""), "{back}");
        assert!(back.contains("\"enforceAvailableModels\":true"), "{back}");
        assert!(back.contains("\"modelOverrides\""), "{back}");
    }

    #[test]
    fn available_models_absent_vs_empty_are_distinct() {
        // Absent ⇒ None (no restriction); explicit [] ⇒ Some(vec![]) (only the
        // default model available) — the two must round-trip distinctly.
        let absent: SettingsJson = serde_json::from_str(r#"{"model":"opus"}"#).unwrap();
        assert!(absent.available_models.is_none());
        assert!(!serde_json::to_string(&absent)
            .unwrap()
            .contains("availableModels"));
        let empty: SettingsJson = serde_json::from_str(r#"{"availableModels":[]}"#).unwrap();
        assert_eq!(empty.available_models.as_deref(), Some(&[][..]));
    }

    #[test]
    fn model_allowlist_merge_strategies() {
        // modelOverrides deep-merges (CC leaves objects to lodash recursive
        // merge); availableModels/enforceAvailableModels are scalar-override.
        assert!(
            matches!(
                strategy_for("modelOverrides"),
                Some(MergeStrategy::DeepMerge)
            ),
            "modelOverrides must deep-merge"
        );
        assert!(
            strategy_for("availableModels").is_none(),
            "availableModels must be scalar-override (source array wins)"
        );
        assert!(
            strategy_for("enforceAvailableModels").is_none(),
            "enforceAvailableModels must be scalar-override"
        );
    }

    #[test]
    fn enterprise_login_version_keys_parse_and_roundtrip() {
        // CC 2.1.207 enterprise login/version managed-policy keys (H-BIN-09). A
        // managed settings.json carrying them must parse into the typed fields
        // (unknown-key tolerance alone would strip them, so the version gate /
        // login policy would never see them).
        let json = r#"{
            "forceLoginMethod": "gateway",
            "forceLoginGatewayUrl": "https://gw.example.com/oidc",
            "forceLoginOrgUUID": ["11111111-2222-3333-4444-555555555555"],
            "parentSettingsBehavior": "merge",
            "minimumVersion": "2.1.0",
            "requiredMinimumVersion": "2.1.207",
            "requiredMaximumVersion": "3.0.0",
            "forceRemoteSettingsRefresh": true,
            "model": "claude-sonnet-4-5"
        }"#;
        let parsed: SettingsJson =
            serde_json::from_str(json).expect("enterprise policy keys must parse");
        assert_eq!(parsed.force_login_method.as_deref(), Some("gateway"));
        assert_eq!(
            parsed.force_login_gateway_url.as_deref(),
            Some("https://gw.example.com/oidc")
        );
        assert_eq!(
            parsed.force_login_org_uuid,
            Some(Value::Array(vec![Value::String(
                "11111111-2222-3333-4444-555555555555".to_string()
            )]))
        );
        assert_eq!(parsed.parent_settings_behavior.as_deref(), Some("merge"));
        assert_eq!(parsed.minimum_version.as_deref(), Some("2.1.0"));
        assert_eq!(parsed.required_minimum_version.as_deref(), Some("2.1.207"));
        assert_eq!(parsed.required_maximum_version.as_deref(), Some("3.0.0"));
        assert_eq!(parsed.force_remote_settings_refresh, Some(true));
        // Sibling survives the load.
        assert!(parsed.model.is_some());
        // camelCase on the wire; round-trips.
        let back = serde_json::to_string(&parsed).expect("serialize");
        for key in [
            "forceLoginMethod",
            "forceLoginGatewayUrl",
            "forceLoginOrgUUID",
            "parentSettingsBehavior",
            "minimumVersion",
            "requiredMinimumVersion",
            "requiredMaximumVersion",
            "forceRemoteSettingsRefresh",
        ] {
            assert!(
                back.contains(&format!("\"{key}\"")),
                "{key} missing in {back}"
            );
            // All scalar-override merge (later source wins).
            assert!(
                strategy_for(key).is_none(),
                "{key} must be scalar-override (later source wins)"
            );
        }
    }

    #[test]
    fn force_login_org_uuid_accepts_single_string() {
        // The union (string | string[]) — a single string form must parse and
        // NOT fail the load.
        let json = r#"{"forceLoginOrgUUID": "org-abc"}"#;
        let parsed: SettingsJson = serde_json::from_str(json).expect("single-string form parses");
        assert_eq!(
            parsed.force_login_org_uuid,
            Some(Value::String("org-abc".to_string()))
        );
    }

    #[test]
    fn enterprise_keys_absent_are_none_and_not_emitted() {
        let absent: SettingsJson = serde_json::from_str(r#"{"model":"x"}"#).unwrap();
        assert!(absent.force_login_method.is_none());
        assert!(absent.required_minimum_version.is_none());
        assert!(absent.force_remote_settings_refresh.is_none());
        let back = serde_json::to_string(&absent).unwrap();
        for key in [
            "forceLoginMethod",
            "requiredMinimumVersion",
            "forceRemoteSettingsRefresh",
        ] {
            assert!(!back.contains(key), "{key} must not be emitted when absent");
        }
    }

    #[test]
    fn http_hook_security_keys_parse_roundtrip_and_are_concat_dedup() {
        // H-BIN-12: CC 2.1.207 HTTP-hook security allowlists. A settings.json
        // carrying them must parse into the typed fields (unknown-key tolerance
        // alone would strip them, so the HTTP hook executor would never see the
        // policy). Both are array-merge (ConcatDedup) — CC `settingsMergeCustomizer`
        // concat-dedups every array except `fallbackModel`.
        let json = r#"{
            "allowedHttpHookUrls": ["https://hooks.example.com/*"],
            "httpHookAllowedEnvVars": ["HOOK_TOKEN", "TEAM_ID"],
            "model": "claude-sonnet-4-5"
        }"#;
        let parsed: SettingsJson = serde_json::from_str(json).expect("http-hook keys must parse");
        assert_eq!(
            parsed.allowed_http_hook_urls.as_deref(),
            Some(&["https://hooks.example.com/*".to_string()][..])
        );
        assert_eq!(
            parsed.http_hook_allowed_env_vars.as_deref(),
            Some(&["HOOK_TOKEN".to_string(), "TEAM_ID".to_string()][..])
        );
        // Sibling survives the load.
        assert!(parsed.model.is_some());
        // camelCase on the wire; round-trips.
        let back = serde_json::to_string(&parsed).expect("serialize");
        assert!(back.contains("\"allowedHttpHookUrls\""), "{back}");
        assert!(back.contains("\"httpHookAllowedEnvVars\""), "{back}");
        // Both concat-dedup (arrays merge across settings sources).
        for key in ["allowedHttpHookUrls", "httpHookAllowedEnvVars"] {
            assert!(
                matches!(strategy_for(key), Some(MergeStrategy::ConcatDedup)),
                "{key} must be ConcatDedup"
            );
        }
    }

    #[test]
    fn allowed_http_hook_urls_absent_vs_empty_are_distinct() {
        // Absent ⇒ None (all URLs allowed); explicit [] ⇒ Some(vec![]) (block ALL
        // HTTP hooks) — the two must round-trip distinctly.
        let absent: SettingsJson = serde_json::from_str(r#"{"model":"opus"}"#).unwrap();
        assert!(absent.allowed_http_hook_urls.is_none());
        assert!(!serde_json::to_string(&absent)
            .unwrap()
            .contains("allowedHttpHookUrls"));
        let empty: SettingsJson = serde_json::from_str(r#"{"allowedHttpHookUrls":[]}"#).unwrap();
        assert_eq!(empty.allowed_http_hook_urls.as_deref(), Some(&[][..]));
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
        assert!(
            back.contains("\"companyAnnouncements\":[\"hi\",\"there\"]"),
            "{back}"
        );
        // Absent ⇒ None, not emitted.
        let absent: SettingsJson = serde_json::from_str(r#"{"model":"x"}"#).unwrap();
        assert_eq!(absent.company_announcements, None);
        assert!(!serde_json::to_string(&absent)
            .unwrap()
            .contains("companyAnnouncements"));
        // Array-merge (concat-dedup), matching CC's array customizer.
        assert!(
            matches!(
                strategy_for("companyAnnouncements"),
                Some(MergeStrategy::ConcatDedup)
            ),
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
        assert!(
            back.contains("\"apiKeyHelper\":\"/usr/local/bin/get-key.sh\""),
            "{back}"
        );
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

    #[test]
    fn fusion_defaults_disabled_and_deep_merges() {
        assert_eq!(strategy_for("fusion"), Some(MergeStrategy::DeepMerge));
        let absent: SettingsJson = serde_json::from_str(r#"{"model":"x"}"#).unwrap();
        assert!(absent.fusion.is_none());
        absent.validate().expect("absent fusion is valid");

        let parsed: SettingsJson = serde_json::from_str(
            r#"{"fusion":{"enabled":false,"preset":"quality","qualityPanelCount":3}}"#,
        )
        .unwrap();
        assert_eq!(parsed.fusion.as_ref().unwrap().enabled, Some(false));
        assert_eq!(parsed.fusion.as_ref().unwrap().quality_panel_count, Some(3));
        parsed.validate().expect("default-shaped fusion is valid");
        let back = serde_json::to_string(&parsed).unwrap();
        assert!(back.contains("\"fusion\""));
        assert!(back.contains("\"qualityPanelCount\""));
    }

    #[test]
    fn fusion_rejects_out_of_range_counts_and_timeouts() {
        let too_few: SettingsJson =
            serde_json::from_str(r#"{"fusion":{"qualityPanelCount":1}}"#).unwrap();
        assert!(too_few.validate().is_err());

        let max_panel: SettingsJson = serde_json::from_str(r#"{"fusion":{"maxPanel":9}}"#).unwrap();
        assert!(max_panel.validate().is_err());

        let timeout: SettingsJson = serde_json::from_str(
            r#"{"fusion":{"totalTimeoutMs":1000,"panelTotalTimeoutMs":2000}}"#,
        )
        .unwrap();
        assert!(timeout.validate().is_err());

        let retries: SettingsJson =
            serde_json::from_str(r#"{"fusion":{"analysisProtocolRetries":2}}"#).unwrap();
        assert!(retries.validate().is_err());
    }
}
