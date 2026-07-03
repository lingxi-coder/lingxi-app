//! Resolve an agent definition's model preference to a concrete wire model id.
//!
//! 1:1 port of claude-code's `getAgentModel` (`src/utils/model/agent.ts:37-95`)
//! plus the helpers it leans on (`getRuntimeMainLoopModel`, `getDefault*Model`,
//! `aliasMatchesParentTier`, `getCanonicalName`, `parseUserSpecifiedModel`, the
//! Bedrock region-prefix helpers, and `getAPIProvider`'s Bedrock arm). Without
//! this, the runner's `resolve_model` emits the bare definition string
//! (`"inherit"` / `"haiku"` / `"sonnet"`), which is not a valid Anthropic model
//! id, so built-in subagent spawns error at the provider on a default install
//! (the provider router only substitutes configured `routing.aliases`). See
//! [`resolve_agent_model`].
//!
//! ## Precedence (`getAgentModel`, agent.ts:43-94)
//! 1. **`LINGXI_SUBAGENT_MODEL`** env override (HIGHEST) → resolved via
//!    `parseUserSpecifiedModel`, bypassing the Bedrock region prefix (the TS
//!    early-return at agent.ts:43-45 returns before `applyParentRegionPrefix`).
//!    The TS guard `if (process.env.LINGXI_SUBAGENT_MODEL)` is falsy for
//!    BOTH unset and empty-string, hence the `.filter(|s| !s.is_empty())`.
//! 2. **`toolSpecifiedModel`** (agent.ts:70-76) — NOT separately ported here.
//!    The LingXi spawn path (`handle.rs::spawn`) already converts AgentTool's
//!    per-call `model` field into `AgentModel::Alias` and passes it as the
//!    `model` arg, so the alias/tier logic below covers it identically. This is
//!    the one intentional structural deviation from the TS line-for-line shape.
//! 3. **`Inherit`** (agent.ts:78-88) → `getRuntimeMainLoopModel` so an agent on
//!    `inherit` gets `opusplan`→Opus / `haiku`→Sonnet runtime resolution in plan
//!    mode (else the parent model unchanged).
//! 4. **alias / explicit** (agent.ts:90-94) → if the bare family alias matches
//!    the parent's tier (`aliasMatchesParentTier` via `getCanonicalName`)
//!    inherit the parent's EXACT id (no surprising downgrade); else
//!    `applyParentRegionPrefix(parseUserSpecifiedModel(spec), spec)`.
//!
//! ## Bedrock cross-region prefix inheritance (agent.ts:50-67)
//! When the parent model carries a cross-region inference prefix (`eu.`, `us.`,
//! …) and the provider is Bedrock, alias/explicit-resolved ids inherit that
//! prefix — UNLESS the original spec already pins its own region prefix (then it
//! is preserved, to avoid silent data-residency violations).
//!
//! ## Documented smaller deferrals (not modeled here)
//! - **Live session model**: the parent model is the BOOT-time configured model
//!   (a snapshot threaded from `cfg.model`), not the live session model — so a
//!   mid-session `/model` switch is not reflected in subsequently-spawned
//!   subagents. claude-code threads the live `parentModel`.
//! - **Nested spawns**: the parent model is always the main-loop model, not the
//!   immediate parent subagent's (the frozen request carries no parent model).
//! - **`toolSpecifiedModel` double-handle**: see precedence note 2 above — the
//!   spawn path maps AgentTool's per-call model into `AgentModel::Alias` before
//!   this fn, so the alias tier/default logic covers it; a future change that
//!   threads the per-call model straight in as a separate arg must NOT run the
//!   alias logic twice.

use crate::definition::{AgentDefinition, AgentModel, AgentSource};
use permission::PermissionMode;
use traits::env::is_env_truthy;

/// Canonical concrete id for a bare family alias.
///
/// Mirrors the current Rust catalog (orchestrator `DEFAULT_MODEL` +
/// `handle_impl` `available_models`); claude-code sources these from
/// `getModelStrings()`. DRIFT NOTE: keep aligned with those (and with
/// `tools/skill/src/model_override.rs`) when the default model versions change.
fn family_default_id(family_lower: &str) -> Option<&'static str> {
    match family_lower {
        "opus" => Some("claude-opus-4-8"),
        // 2.1.198 alias table: sonnet.default = "claude-sonnet-5" (was 4-6).
        "sonnet" => Some("claude-sonnet-5"),
        "haiku" => Some("claude-haiku-4-5"),
        _ => None,
    }
}

/// The Sonnet family default id for non-firstParty (3P) providers.
///
/// `getDefaultSonnetModel()` (`model.ts:119-128`) returns `getModelStrings().sonnet45`
/// — canonical first-party id `claude-sonnet-4-5-20250929` (`configs.ts:45`) — when
/// `getAPIProvider() !== 'firstParty'` (Bedrock/Vertex/Foundry), since those
/// providers may not yet have Sonnet 5. firstParty is on `claude-sonnet-5`
/// (`family_default_id("sonnet")`; 2.1.198 alias table
/// `sonnet.per_provider = {bedrock/vertex/foundry: "claude-sonnet-4-5"}`).
///
/// Haiku does NOT diverge by provider (`getDefaultHaikuModel` has no provider
/// branch — Haiku 4.5 is on all platforms). Opus DOES in 2.1.193 — see
/// [`OPUS_3P_DEFAULT_ID`] / [`get_default_opus_model`].
const SONNET_3P_DEFAULT_ID: &str = "claude-sonnet-4-5-20250929";

/// The Opus family default id for the non-firstParty (3P) providers the port
/// models (Bedrock/Vertex/Foundry). `getDefaultOpusModel()` returns `opus46`
/// (`claude-opus-4-6`) for `!['firstParty','anthropicAws','gateway']`, while
/// firstParty gets `opus48` (`family_default_id("opus")` = `claude-opus-4-8`).
/// (claude also maps mantle / anthropicAws / gateway to `opus47`; the port
/// detects only Bedrock/Vertex/Foundry via the `CLAUDE_CODE_USE_*` env vars, so
/// those providers are not represented — same 2-way detection as Sonnet.)
const OPUS_3P_DEFAULT_ID: &str = "claude-opus-4-6";

/// `getDefaultOpusModel()` (`model.ts:105-116`): the `ANTHROPIC_DEFAULT_OPUS_MODEL`
/// env override (when non-empty) wins; else provider-aware — `claude-opus-4-8`
/// for firstParty, `claude-opus-4-6` (`OPUS_3P_DEFAULT_ID`) for
/// Bedrock/Vertex/Foundry.
fn get_default_opus_model() -> String {
    if let Some(v) = std::env::var("ANTHROPIC_DEFAULT_OPUS_MODEL")
        .ok()
        .filter(|s| !s.is_empty())
    {
        return v;
    }
    if api_provider_is_first_party() {
        return family_default_id("opus")
            .expect("opus is a known family")
            .to_string();
    }
    OPUS_3P_DEFAULT_ID.to_string()
}

/// `getDefaultSonnetModel()` (`model.ts:118-128`): the
/// `ANTHROPIC_DEFAULT_SONNET_MODEL` env override (when non-empty) wins; else the
/// default is provider-aware — `claude-sonnet-4-5-20250929` for non-firstParty
/// (Bedrock/Vertex/Foundry, which lag), `claude-sonnet-5` for firstParty
/// (2.1.198 alias table).
fn get_default_sonnet_model() -> String {
    if let Some(v) = std::env::var("ANTHROPIC_DEFAULT_SONNET_MODEL")
        .ok()
        .filter(|s| !s.is_empty())
    {
        return v;
    }
    if api_provider_is_first_party() {
        return family_default_id("sonnet")
            .expect("sonnet is a known family")
            .to_string();
    }
    SONNET_3P_DEFAULT_ID.to_string()
}

/// `getDefaultHaikuModel()` (`model.ts:130-138`): env override, else default.
/// Haiku 4.5 is available on all platforms (firstParty/Foundry/Bedrock/Vertex),
/// so the TS has no provider branch — collapsed here.
fn get_default_haiku_model() -> String {
    env_default("ANTHROPIC_DEFAULT_HAIKU_MODEL", "haiku")
}

/// Shared body for the env-override `getDefault*Model` helpers whose defaults do
/// NOT diverge by provider (opus / haiku): env override (when non-empty) wins,
/// else the family default id. Sonnet is provider-aware — see
/// [`get_default_sonnet_model`] — so it does NOT route through here.
fn env_default(env_var: &str, family_lower: &str) -> String {
    if let Some(v) = std::env::var(env_var).ok().filter(|s| !s.is_empty()) {
        return v;
    }
    family_default_id(family_lower)
        .expect("env_default called with a known family")
        .to_string()
}

/// `getRuntimeMainLoopModel` (`model.ts:145-167`): in plan mode, `opusplan`
/// resolves to Opus (without `[1m]`) and `haiku` resolves to Sonnet; otherwise
/// the main-loop model unchanged.
///
/// NOTE the TS keys off `getUserSpecifiedModelSetting()` — the RAW user setting
/// string (e.g. `'opusplan'` / `'haiku'`) — which is why `model_setting` is
/// threaded SEPARATELY from `main_loop_model` (the resolved id). Without it the
/// Inherit branch is byte-identical to returning the parent model.
fn get_runtime_main_loop_model(
    permission_mode: PermissionMode,
    main_loop_model: &str,
    exceeds_200k_tokens: bool,
    model_setting: Option<&str>,
) -> String {
    // opusplan uses Opus in plan mode without [1m] suffix.
    if model_setting == Some("opusplan")
        && permission_mode == PermissionMode::Plan
        && !exceeds_200k_tokens
    {
        return get_default_opus_model();
    }

    // sonnetplan by default
    if model_setting == Some("haiku") && permission_mode == PermissionMode::Plan {
        return get_default_sonnet_model();
    }

    main_loop_model.to_string()
}

/// Cross-region inference profile prefixes for Bedrock (`bedrock.ts:189`).
const BEDROCK_REGION_PREFIXES: [&str; 4] = ["us", "eu", "apac", "global"];

/// Extract the model/inference-profile id from a Bedrock ARN. If the input is
/// not an ARN, returns it unchanged. 1:1 with `extractModelIdFromArn`
/// (`bedrock.ts:199-208`).
///
/// ARN format: `arn:aws:bedrock:<region>:<account>:inference-profile/<profile-id>`
fn extract_model_id_from_arn(model_id: &str) -> &str {
    if !model_id.starts_with("arn:") {
        return model_id;
    }
    match model_id.rfind('/') {
        Some(i) => &model_id[i + 1..],
        None => model_id,
    }
}

/// Extract the region prefix from a Bedrock cross-region inference model id
/// (handles both plain ids and full ARN format). 1:1 with `getBedrockRegionPrefix`
/// (`bedrock.ts:222-235`).
///
/// For example:
/// - `"eu.anthropic.claude-sonnet-4-5-20250929-v1:0"` → `Some("eu")`
/// - `"us.anthropic.claude-3-7-sonnet-20250219-v1:0"` → `Some("us")`
/// - `"arn:aws:bedrock:ap-northeast-2:123:inference-profile/global.anthropic.claude-opus-4-6-v1"` → `Some("global")`
/// - `"anthropic.claude-3-5-sonnet-20241022-v2:0"` → `None` (foundation model)
/// - `"claude-sonnet-4-5-20250929"` → `None` (first-party format)
fn get_bedrock_region_prefix(model_id: &str) -> Option<&'static str> {
    let effective_model_id = extract_model_id_from_arn(model_id);
    for prefix in BEDROCK_REGION_PREFIXES {
        if effective_model_id.starts_with(&format!("{prefix}.anthropic.")) {
            return Some(prefix);
        }
    }
    None
}

/// `true` if a model id is a foundation model (e.g.
/// `"anthropic.claude-sonnet-4-5-20250929-v1:0"`). 1:1 with `isFoundationModel`
/// (`bedrock.ts:181-183`).
fn is_foundation_model(model_id: &str) -> bool {
    model_id.starts_with("anthropic.")
}

/// Apply a region prefix to a Bedrock model id. If the model already has a
/// different region prefix, it is replaced. If the model is a foundation model
/// (`anthropic.*`), the prefix is added. Otherwise returned as-is. 1:1 with
/// `applyBedrockRegionPrefix` (`bedrock.ts:248-265`).
///
/// For example:
/// - `applyBedrockRegionPrefix("us.anthropic.claude-sonnet-4-5-v1:0", "eu")` → `"eu.anthropic.claude-sonnet-4-5-v1:0"`
/// - `applyBedrockRegionPrefix("anthropic.claude-sonnet-4-5-v1:0", "eu")` → `"eu.anthropic.claude-sonnet-4-5-v1:0"`
/// - `applyBedrockRegionPrefix("claude-sonnet-4-5-20250929", "eu")` → `"claude-sonnet-4-5-20250929"` (not a Bedrock model)
fn apply_bedrock_region_prefix(model_id: &str, prefix: &str) -> String {
    // Check if it already has a region prefix and replace it.
    if let Some(existing) = get_bedrock_region_prefix(model_id) {
        return model_id.replacen(&format!("{existing}."), &format!("{prefix}."), 1);
    }
    // Check if it's a foundation model (anthropic.*) and add the prefix.
    if is_foundation_model(model_id) {
        return format!("{prefix}.{model_id}");
    }
    // Not a Bedrock model format, return as-is.
    model_id.to_string()
}

/// `getAPIProvider() === 'bedrock'` (`providers.ts:6-13`): only the Bedrock arm
/// is needed for the region-prefix seam (vertex/foundry never apply the region
/// prefix). Uses the strict-allowlist `is_env_truthy`.
fn api_provider_is_bedrock() -> bool {
    is_env_truthy(std::env::var("CLAUDE_CODE_USE_BEDROCK").ok().as_deref())
}

/// `getAPIProvider() === 'firstParty'` (`providers.ts:6-13`): the falsy tail of
/// the Bedrock > Vertex > Foundry > firstParty precedence chain — first-party iff
/// none of `CLAUDE_CODE_USE_BEDROCK` / `_VERTEX` / `_FOUNDRY` is env-truthy. Uses
/// the strict-allowlist `is_env_truthy` (same as the TS `isEnvTruthy`), so a
/// non-allowlisted value (e.g. `"0"` / `"false"`) does NOT switch providers.
fn api_provider_is_first_party() -> bool {
    !is_env_truthy(std::env::var("CLAUDE_CODE_USE_BEDROCK").ok().as_deref())
        && !is_env_truthy(std::env::var("CLAUDE_CODE_USE_VERTEX").ok().as_deref())
        && !is_env_truthy(std::env::var("CLAUDE_CODE_USE_FOUNDRY").ok().as_deref())
}

/// Check if a bare family alias (`opus`/`sonnet`/`haiku`) matches the parent
/// model's tier. When it does, the subagent inherits the parent's EXACT model
/// string instead of resolving the alias to a provider default. 1:1 with
/// `aliasMatchesParentTier` (`agent.ts:110-122`).
///
/// Prevents surprising downgrades: a Vertex user on Opus 4.6 (via `/model`) who
/// spawns a subagent with `model: opus` should get Opus 4.6, not whatever
/// `getDefaultOpusModel()` returns for 3P.
///
/// Only bare family aliases match. `opus[1m]`, `best`, `opusplan` fall through
/// (the default arm) since they carry semantics beyond "same tier as parent".
/// CRITICAL: it uses `getCanonicalName(parentModel)` (which strips dates / ARN /
/// provider noise), NOT a raw substring on the full id.
fn alias_matches_parent_tier(alias: &str, parent_model: &str) -> bool {
    let canonical = canonical_name(parent_model);
    match alias.to_lowercase().as_str() {
        "opus" => canonical.contains("opus"),
        "sonnet" => canonical.contains("sonnet"),
        "haiku" => canonical.contains("haiku"),
        _ => false,
    }
}

/// Resolve a full model id to a shorter canonical family name. Faithful copy of
/// `tools/skill/src/model_override.rs::canonical_name` (ports
/// `firstPartyNameToCanonical`, `model.ts:217-270`); the
/// `resolveOverriddenModel`/Bedrock-ARN indirection (`getCanonicalName`,
/// `model.ts:279-283`) is a no-op for these substring checks, so it is folded
/// in (identical to the skill copy's note).
fn canonical_name(model: &str) -> String {
    let name = model.to_lowercase();
    // Order matters: check more specific versions first (4-6 before 4-5 before 4).
    if name.contains("claude-opus-4-6") {
        return "claude-opus-4-6".to_string();
    }
    if name.contains("claude-opus-4-5") {
        return "claude-opus-4-5".to_string();
    }
    if name.contains("claude-opus-4-1") {
        return "claude-opus-4-1".to_string();
    }
    if name.contains("claude-opus-4") {
        return "claude-opus-4".to_string();
    }
    // sonnet-5 before the sonnet-4-x arms (2.1.198; mutually exclusive —
    // "claude-sonnet-4-5" does NOT contain "sonnet-5").
    if name.contains("claude-sonnet-5") {
        return "claude-sonnet-5".to_string();
    }
    if name.contains("claude-sonnet-4-6") {
        return "claude-sonnet-4-6".to_string();
    }
    if name.contains("claude-sonnet-4-5") {
        return "claude-sonnet-4-5".to_string();
    }
    if name.contains("claude-sonnet-4") {
        return "claude-sonnet-4".to_string();
    }
    if name.contains("claude-haiku-4-5") {
        return "claude-haiku-4-5".to_string();
    }
    if name.contains("claude-3-7-sonnet") {
        return "claude-3-7-sonnet".to_string();
    }
    if name.contains("claude-3-5-sonnet") {
        return "claude-3-5-sonnet".to_string();
    }
    if name.contains("claude-3-5-haiku") {
        return "claude-3-5-haiku".to_string();
    }
    if name.contains("claude-3-opus") {
        return "claude-3-opus".to_string();
    }
    if name.contains("claude-3-sonnet") {
        return "claude-3-sonnet".to_string();
    }
    if name.contains("claude-3-haiku") {
        return "claude-3-haiku".to_string();
    }
    // Fall back to the lowercased input when no pattern matches (the TS regex
    // only narrows the unmatched case; substring checks are equivalent here).
    name
}

/// Resolve a user/agent-specified model string to a concrete id — the
/// alias-resolution subset of `parseUserSpecifiedModel` (`model.ts:445-505`),
/// EXACTLY as in `tools/skill/src/model_override.rs`. Bare family aliases
/// (`opus` / `sonnet` / `haiku` / `opusplan` / `best`) map to their default id
/// (preserving a trailing `[1m]`, except `best` which `getBestModel` ignores
/// `[1m]` for); every other string passes through with only `[1m]` normalized.
///
/// The alias arms route through the env-aware `get_default_*_model` helpers
/// (matching the TS `getDefaultSonnetModel()` / `getDefaultHaikuModel()` /
/// `getDefaultOpusModel()` calls at `model.ts:459-465`), so the
/// `ANTHROPIC_DEFAULT_{OPUS,SONNET,HAIKU}_MODEL` env overrides take effect.
///
/// The ant-model registry, the legacy Opus 4.0/4.1 first-party remap, and the
/// Foundry deployment-id passthrough branches of `parseUserSpecifiedModel` are
/// NOT modeled (unreachable from this seam — see the skill copy's note).
///
/// Public entry point — claude's `getMainLoopModel()` → `parseUserSpecifiedModel`.
/// The composition root calls this to resolve the user/CLI model setting (a bare
/// alias like `opusplan`/`sonnet`, or a full id) to the concrete main-loop wire
/// id BEFORE handing it to a subagent/teammate spawner as the `parent_model`.
/// Without this the `AgentModel::Inherit` default-mode branch returns the raw
/// alias (a bogus wire id); claude passes the RESOLVED `mainLoopModel` as the
/// child's parent. The raw alias is still passed separately as `model_setting`
/// so the plan-mode `opusplan→Opus` swap can fire.
#[must_use]
pub fn resolve_user_specified_model(model_input: &str) -> String {
    parse_user_specified_model(model_input)
}

fn parse_user_specified_model(model_input: &str) -> String {
    let trimmed = model_input.trim();
    let normalized = trimmed.to_lowercase();
    let has_1m_tag = has_1m_context(&normalized);
    let base = strip_1m_suffix(&normalized);
    let suffix = if has_1m_tag { "[1m]" } else { "" };

    match base.as_str() {
        // `opusplan` → Sonnet default (Opus only in plan mode), 1:1 with TS.
        "opusplan" | "sonnet" => format!("{}{suffix}", get_default_sonnet_model()),
        "haiku" => format!("{}{suffix}", get_default_haiku_model()),
        "opus" => format!("{}{suffix}", get_default_opus_model()),
        // `getBestModel` returns the Opus default and ignores any [1m] suffix.
        "best" => get_default_opus_model(),
        _ => {
            // Non-alias: preserve the original case, normalizing only `[1m]`.
            if has_1m_tag {
                format!("{}[1m]", strip_1m_suffix(trimmed))
            } else {
                trimmed.to_string()
            }
        }
    }
}

/// `true` if `model` carries an explicit `[1m]` suffix (case-insensitive).
/// Mirrors `has1mContext` reduced to the substring test (the
/// `CLAUDE_CODE_DISABLE_1M_CONTEXT` gate is not modeled here — `[1m]` passthrough
/// is a no-op for the family-default resolution this seam performs).
fn has_1m_context(model: &str) -> bool {
    model.to_lowercase().contains("[1m]")
}

/// Strip a single trailing `[1m]` (case-insensitive) and trim. Mirrors the TS
/// `replace(/\[1m]$/i, '').trim()`.
fn strip_1m_suffix(s: &str) -> String {
    if s.to_lowercase().ends_with("[1m]") {
        s[..s.len() - 4].trim().to_string()
    } else {
        s.trim().to_string()
    }
}

/// The Explore model-cap ladder — claude-code 2.1.198 `Kyl`
/// (`["haiku","sonnet","opus"]`). `obm` slices it up to and including
/// [`EXPLORE_MODEL_CAP`] (`Kyl.slice(0, Kyl.indexOf(Yyl)+1)` — the whole array,
/// since opus is last) and asks whether the session model names ANY of these
/// families.
const EXPLORE_MODEL_CAP_LADDER: [&str; 3] = ["haiku", "sonnet", "opus"];

/// The alias the built-in Explore agent is capped at — claude-code 2.1.198
/// `Yyl` (`"opus"`).
const EXPLORE_MODEL_CAP: &str = "opus";

/// 1:1 port of `dPn(e,t)` (2.1.198): `true` iff the lowercased model string
/// contains ANY of the (non-empty) needles, case-insensitively.
fn model_contains_any(model: &str, needles: &[&str]) -> bool {
    let lower = model.to_lowercase();
    needles
        .iter()
        .any(|n| !n.is_empty() && lower.contains(&n.to_lowercase()))
}

/// 1:1 port of `obm(e)` (2.1.198): `true` iff the provider is firstParty AND
/// the session model names NONE of the haiku/sonnet/opus families (i.e. a
/// fable/mythos-class session model, "above" the opus cap).
///
/// `session_provider_first_party` is LingXi's multi-provider extension of the
/// `fr() !== "firstParty"` gate: the composition root passes `false` when the
/// session's default model routes to a non-Anthropic provider profile
/// (OpenAI/Gemini/…), which behaves exactly like the TS non-firstParty branch
/// (→ `false` → Explore inherits). The env half (`CLAUDE_CODE_USE_BEDROCK` /
/// `_VERTEX` / `_FOUNDRY`) is checked here, same as `fr()`.
fn session_model_exceeds_explore_cap(
    session_model: &str,
    session_provider_first_party: bool,
) -> bool {
    if !session_provider_first_party || !api_provider_is_first_party() {
        return false;
    }
    let cap_idx = EXPLORE_MODEL_CAP_LADDER
        .iter()
        .position(|m| *m == EXPLORE_MODEL_CAP)
        .expect("the cap is in the ladder");
    let ladder = &EXPLORE_MODEL_CAP_LADDER[..=cap_idx];
    !model_contains_any(session_model, ladder)
}

/// 1:1 port of `GAe(e,t)` (2.1.198): the built-in `Explore` agent's model is
/// derived from the SESSION model instead of its (now `"inherit"`) frontmatter.
///
/// ```js
/// function GAe(e,t){if(e.agentType!==qme.agentType||e.source!=="built-in")return e.model;
///   return obm(t)?Yyl:"inherit"}
/// ```
///
/// - Any non-Explore or non-built-in definition: `def.model` unchanged (a
///   user/project agent literally named "Explore" keeps its own model).
/// - Built-in Explore on a firstParty session whose model names none of
///   haiku/sonnet/opus (fable/mythos-class): the `"opus"` alias — Explore
///   inherits the session model CAPPED at opus.
/// - Otherwise (haiku/sonnet/opus session, or any non-firstParty provider):
///   `"inherit"` — Explore runs on the session model.
#[must_use]
pub fn resolve_builtin_explore_model(
    def: &AgentDefinition,
    session_model: &str,
    session_provider_first_party: bool,
) -> AgentModel {
    if def.agent_type != "Explore" || !matches!(def.source, AgentSource::BuiltIn) {
        return def.model.clone();
    }
    if session_model_exceeds_explore_cap(session_model, session_provider_first_party) {
        AgentModel::Alias(EXPLORE_MODEL_CAP.to_string())
    } else {
        AgentModel::Inherit
    }
}

/// Resolve `model` to a concrete wire model string given the parent / main-loop
/// model, the live permission mode, and the RAW user model setting. 1:1 port of
/// `getAgentModel` (`agent.ts:37-95`). See the module docs for the precedence and
/// the deferred nuances.
///
/// `permission_mode` is the live/boot permission mode anchor (Inherit routes
/// through `getRuntimeMainLoopModel`). `model_setting` is the RAW user model
/// setting string (`getUserSpecifiedModelSetting()`, e.g. `"opusplan"` /
/// `"haiku"` / `None`) — NOT the resolved id; without it the Inherit branch
/// returns the parent model unchanged (faithful: a non-opusplan setting never
/// triggers the plan-mode swap).
#[must_use]
pub fn resolve_agent_model(
    model: &AgentModel,
    parent_model: &str,
    permission_mode: PermissionMode,
    model_setting: Option<&str>,
) -> String {
    // 1. LINGXI_SUBAGENT_MODEL env override (HIGHEST). The TS guard is falsy
    //    for both unset AND empty-string. This branch bypasses the Bedrock prefix
    //    (the TS early-return precedes applyParentRegionPrefix).
    if let Some(v) = std::env::var("LINGXI_SUBAGENT_MODEL")
        .ok()
        .filter(|s| !s.is_empty())
    {
        return parse_user_specified_model(&v);
    }

    // Extract Bedrock region prefix from the parent model to inherit for
    // subagents (so subagents use the same cross-region inference profile).
    let parent_region_prefix = get_bedrock_region_prefix(parent_model);

    // Apply the parent region prefix for Bedrock models. `original_spec` is the
    // raw model string before resolution (alias or full id). If the user
    // explicitly specified a full model id that already carries its own region
    // prefix (e.g. "eu.anthropic.…"), we preserve it instead of overwriting with
    // the parent's prefix (prevents silent data-residency violations).
    let apply_parent_region_prefix = |resolved: &str, original_spec: &str| -> String {
        if let Some(prefix) = parent_region_prefix {
            if api_provider_is_bedrock() {
                if get_bedrock_region_prefix(original_spec).is_some() {
                    return resolved.to_owned();
                }
                return apply_bedrock_region_prefix(resolved, prefix);
            }
        }
        resolved.to_owned()
    };

    // 2. toolSpecifiedModel (agent.ts:70-76) — NOT separately ported; the spawn
    //    path maps AgentTool's per-call model into AgentModel::Alias upstream, so
    //    the alias/tier tail below covers it (one intentional deviation).

    match model {
        // 3. Inherit → runtime main-loop resolution (opusplan→Opus / haiku→Sonnet
        //    in plan mode; else the parent model unchanged).
        AgentModel::Inherit => {
            get_runtime_main_loop_model(permission_mode, parent_model, false, model_setting)
        }
        // 4. Explicit / Alias share the same tail: if the bare family alias
        //    matches the parent's tier, inherit the parent's EXACT id; else
        //    resolve + apply the parent region prefix. Real explicit ids are
        //    full `claude-…` strings, which parse_user_specified_model passes
        //    through unchanged (case preserved, only [1m] normalized), so this is
        //    byte-identical to the old verbatim passthrough for them.
        AgentModel::Explicit(spec) | AgentModel::Alias(spec) => {
            if alias_matches_parent_tier(spec, parent_model) {
                return parent_model.to_string();
            }
            let resolved = parse_user_specified_model(spec);
            apply_parent_region_prefix(&resolved, spec)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- env-var serialization -------------------------------------------
    //
    // LINGXI_SUBAGENT_MODEL / CLAUDE_CODE_USE_BEDROCK / ANTHROPIC_DEFAULT_*
    // mutate process-global env. To avoid cross-test races (cargo runs tests in
    // parallel within a crate), all env-mutating tests share one Mutex and clean
    // up after themselves. (migrations/src/context.rs sets env directly; we add a
    // guard since multiple tests here touch the same vars.)
    use std::sync::Mutex;
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvGuard {
        key: &'static str,
        prev: Option<String>,
    }
    impl EnvGuard {
        fn set(key: &'static str, val: &str) -> Self {
            let prev = std::env::var(key).ok();
            std::env::set_var(key, val);
            Self { key, prev }
        }
    }
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.prev {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }

    const DEFAULT: PermissionMode = PermissionMode::Default;

    #[test]
    fn inherit_resolves_to_parent_model() {
        assert_eq!(
            resolve_agent_model(&AgentModel::Inherit, "claude-opus-4-7", DEFAULT, None),
            "claude-opus-4-7"
        );
    }

    #[test]
    fn explicit_passes_through_verbatim() {
        // Real explicit ids are full `claude-…` strings → parse passes them
        // through unchanged.
        assert_eq!(
            resolve_agent_model(
                &AgentModel::Explicit("claude-sonnet-4-6".to_string()),
                "claude-opus-4-7",
                DEFAULT,
                None,
            ),
            "claude-sonnet-4-6"
        );
    }

    #[test]
    fn family_alias_of_different_tier_resolves_to_default_id() {
        // parent is opus; agent asks for haiku/sonnet → family default concrete id.
        // The `sonnet` default is provider-aware (#18) so pin firstParty under the
        // ENV_LOCK to assert the 1P id deterministically.
        let _lock = ENV_LOCK.lock().unwrap();
        let _g = clear_provider_env();
        assert_eq!(
            resolve_agent_model(
                &AgentModel::Alias("haiku".to_string()),
                "claude-opus-4-7",
                DEFAULT,
                None,
            ),
            "claude-haiku-4-5"
        );
        assert_eq!(
            resolve_agent_model(
                &AgentModel::Alias("sonnet".to_string()),
                "claude-opus-4-7",
                DEFAULT,
                None,
            ),
            "claude-sonnet-5"
        );
    }

    #[test]
    fn family_alias_matching_parent_tier_inherits_exact_parent() {
        // parent IS opus → "opus" inherits the parent's exact id, not a default.
        assert_eq!(
            resolve_agent_model(
                &AgentModel::Alias("opus".to_string()),
                "claude-opus-4-9-future",
                DEFAULT,
                None,
            ),
            "claude-opus-4-9-future"
        );
        assert_eq!(
            resolve_agent_model(
                &AgentModel::Alias("sonnet".to_string()),
                "claude-sonnet-4-6",
                DEFAULT,
                None,
            ),
            "claude-sonnet-4-6"
        );
    }

    #[test]
    fn family_alias_is_case_insensitive() {
        assert_eq!(
            resolve_agent_model(
                &AgentModel::Alias("Haiku".to_string()),
                "claude-opus-4-7",
                DEFAULT,
                None,
            ),
            "claude-haiku-4-5"
        );
    }

    #[test]
    fn non_family_alias_passes_through() {
        // A custom alias the user may have mapped via routing.aliases.
        assert_eq!(
            resolve_agent_model(
                &AgentModel::Alias("my-fast-model".to_string()),
                "claude-opus-4-7",
                DEFAULT,
                None,
            ),
            "my-fast-model"
        );
    }

    #[test]
    fn family_alias_with_non_anthropic_parent_resolves_to_default_id() {
        // A 3P / non-Anthropic parent MODEL is not any Claude tier, so a family
        // alias resolves to the family default (NOT the parent). Pin firstParty
        // under the ENV_LOCK since the `sonnet` default is provider-aware (#18);
        // the parent model id (gemini/…) is unrelated to the API provider env.
        let _lock = ENV_LOCK.lock().unwrap();
        let _g = clear_provider_env();
        assert_eq!(
            resolve_agent_model(
                &AgentModel::Alias("opus".to_string()),
                "openai/gpt-4o",
                DEFAULT,
                None,
            ),
            "claude-opus-4-8"
        );
        assert_eq!(
            resolve_agent_model(
                &AgentModel::Alias("sonnet".to_string()),
                "gemini/gemini-2.0-flash",
                DEFAULT,
                None,
            ),
            "claude-sonnet-5"
        );
    }

    #[test]
    fn cross_tier_family_alias_resolves_to_that_familys_default() {
        // parent is sonnet, agent asks for opus (different tier) → opus default id.
        // The opus default is provider-aware (#2), so pin firstParty deterministically.
        let _lock = ENV_LOCK.lock().unwrap();
        let _g = clear_provider_env();
        assert_eq!(
            resolve_agent_model(
                &AgentModel::Alias("opus".to_string()),
                "claude-sonnet-4-6",
                DEFAULT,
                None,
            ),
            "claude-opus-4-8"
        );
    }

    // ---- G10: new edge cases ----------------------------------------------

    #[test]
    fn subagent_model_env_override_wins_over_everything() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _g = EnvGuard::set("LINGXI_SUBAGENT_MODEL", "haiku");
        // Override beats Inherit, Alias, and Explicit alike — resolved via
        // parse_user_specified_model.
        assert_eq!(
            resolve_agent_model(&AgentModel::Inherit, "claude-opus-4-7", DEFAULT, None),
            "claude-haiku-4-5"
        );
        assert_eq!(
            resolve_agent_model(
                &AgentModel::Alias("sonnet".to_string()),
                "claude-opus-4-7",
                DEFAULT,
                None,
            ),
            "claude-haiku-4-5"
        );
        assert_eq!(
            resolve_agent_model(
                &AgentModel::Explicit("claude-sonnet-4-6".to_string()),
                "claude-opus-4-7",
                DEFAULT,
                None,
            ),
            "claude-haiku-4-5"
        );
    }

    #[test]
    fn subagent_model_env_override_empty_string_is_ignored() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _g = EnvGuard::set("LINGXI_SUBAGENT_MODEL", "");
        // Empty-string is falsy in the TS guard → not honored; Inherit falls
        // through to the parent.
        assert_eq!(
            resolve_agent_model(&AgentModel::Inherit, "claude-opus-4-7", DEFAULT, None),
            "claude-opus-4-7"
        );
    }

    #[test]
    fn inherit_opusplan_plan_mode_resolves_to_opus() {
        // opusplan in plan mode resolves to the opus default, which is
        // provider-aware (#2) → pin firstParty deterministically.
        let _lock = ENV_LOCK.lock().unwrap();
        let _g = clear_provider_env();
        assert_eq!(
            resolve_agent_model(
                &AgentModel::Inherit,
                "claude-sonnet-4-6",
                PermissionMode::Plan,
                Some("opusplan"),
            ),
            "claude-opus-4-8"
        );
    }

    #[test]
    fn inherit_opusplan_default_mode_returns_parent_unchanged() {
        assert_eq!(
            resolve_agent_model(
                &AgentModel::Inherit,
                "claude-sonnet-4-6",
                PermissionMode::Default,
                Some("opusplan"),
            ),
            "claude-sonnet-4-6"
        );
    }

    #[test]
    fn inherit_haiku_plan_mode_resolves_to_sonnet() {
        // Resolves to the Sonnet default, which is provider-aware (#18) → pin
        // firstParty under the ENV_LOCK to assert the 1P id deterministically.
        let _lock = ENV_LOCK.lock().unwrap();
        let _g = clear_provider_env();
        assert_eq!(
            resolve_agent_model(
                &AgentModel::Inherit,
                "claude-opus-4-7",
                PermissionMode::Plan,
                Some("haiku"),
            ),
            "claude-sonnet-5"
        );
    }

    #[test]
    fn inherit_no_setting_plan_mode_returns_parent_unchanged() {
        assert_eq!(
            resolve_agent_model(
                &AgentModel::Inherit,
                "claude-opus-4-7",
                PermissionMode::Plan,
                None
            ),
            "claude-opus-4-7"
        );
    }

    // ---- G10: Bedrock cross-region prefix inheritance ----------------------

    #[test]
    fn bedrock_family_default_is_not_a_foundation_model_so_prefix_is_noop() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _g = EnvGuard::set("CLAUDE_CODE_USE_BEDROCK", "1");
        // parent has a us. prefix; on Bedrock (non-firstParty) the `sonnet` alias
        // resolves to the 3P Sonnet default (claude-sonnet-4-5-20250929, #18),
        // which is NOT a foundation (anthropic.*) id, so apply_bedrock_region_prefix
        // is a no-op.
        assert_eq!(
            resolve_agent_model(
                &AgentModel::Alias("sonnet".to_string()),
                "us.anthropic.claude-opus-4-6-v1:0",
                DEFAULT,
                None,
            ),
            "claude-sonnet-4-5-20250929"
        );
    }

    #[test]
    fn bedrock_foundation_model_inherits_parent_prefix() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _g = EnvGuard::set("CLAUDE_CODE_USE_BEDROCK", "1");
        // An explicit foundation id (anthropic.*) gets the parent's us. prefix.
        assert_eq!(
            resolve_agent_model(
                &AgentModel::Explicit("anthropic.claude-sonnet-4-6-v1:0".to_string()),
                "us.anthropic.claude-opus-4-6-v1:0",
                DEFAULT,
                None,
            ),
            "us.anthropic.claude-sonnet-4-6-v1:0"
        );
    }

    #[test]
    fn bedrock_original_spec_with_own_prefix_is_preserved() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _g = EnvGuard::set("CLAUDE_CODE_USE_BEDROCK", "1");
        // The explicit spec already pins eu. → the parent's us. prefix does NOT
        // overwrite it.
        assert_eq!(
            resolve_agent_model(
                &AgentModel::Explicit("eu.anthropic.claude-sonnet-4-6-v1:0".to_string()),
                "us.anthropic.claude-opus-4-6-v1:0",
                DEFAULT,
                None,
            ),
            "eu.anthropic.claude-sonnet-4-6-v1:0"
        );
    }

    #[test]
    fn bedrock_prefix_not_applied_when_not_bedrock_provider() {
        // No CLAUDE_CODE_USE_BEDROCK → api_provider_is_bedrock() is false → the
        // foundation id is returned unchanged even though the parent has a us.
        // prefix. (Guarded by the lock to avoid a concurrent test setting the var.)
        let _lock = ENV_LOCK.lock().unwrap();
        let _g = EnvGuard {
            key: "CLAUDE_CODE_USE_BEDROCK",
            prev: std::env::var("CLAUDE_CODE_USE_BEDROCK").ok(),
        };
        std::env::remove_var("CLAUDE_CODE_USE_BEDROCK");
        assert_eq!(
            resolve_agent_model(
                &AgentModel::Explicit("anthropic.claude-sonnet-4-6-v1:0".to_string()),
                "us.anthropic.claude-opus-4-6-v1:0",
                DEFAULT,
                None,
            ),
            "anthropic.claude-sonnet-4-6-v1:0"
        );
    }

    #[test]
    fn alias_matches_parent_tier_via_canonical_inherits_exact_parent() {
        // parent is a Bedrock-prefixed opus id; canonical_name strips the prefix
        // so Alias("opus") tier-matches and inherits the EXACT parent id.
        assert_eq!(
            resolve_agent_model(
                &AgentModel::Alias("opus".to_string()),
                "us.anthropic.claude-opus-4-6-v1:0",
                DEFAULT,
                None,
            ),
            "us.anthropic.claude-opus-4-6-v1:0"
        );
    }

    #[test]
    fn non_bare_aliases_do_not_tier_match() {
        // opus[1m] / best / opusplan are NOT bare family aliases → default arm
        // false, so they do NOT inherit the parent even when same family.
        assert!(!alias_matches_parent_tier("opus[1m]", "claude-opus-4-7"));
        assert!(!alias_matches_parent_tier("best", "claude-opus-4-7"));
        assert!(!alias_matches_parent_tier("opusplan", "claude-opus-4-7"));
        assert!(alias_matches_parent_tier("opus", "claude-opus-4-7"));
    }

    #[test]
    fn default_opus_model_env_override() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _g = EnvGuard::set("ANTHROPIC_DEFAULT_OPUS_MODEL", "custom-opus-id");
        assert_eq!(get_default_opus_model(), "custom-opus-id");
    }

    // ---- #18: 3P-provider Sonnet family default (Bedrock/Vertex/Foundry) ------
    //
    // claude-code's getDefaultSonnetModel() (model.ts:118-128) returns
    // getModelStrings().sonnet45 (canonical claude-sonnet-4-5-20250929) for
    // non-firstParty providers, and sonnet46 (claude-sonnet-4-6) for firstParty.
    // getDefaultOpusModel/getDefaultHaikuModel do NOT differ by provider today.
    //
    // Each test scopes ALL THREE provider env vars under the shared ENV_LOCK so a
    // concurrent test setting CLAUDE_CODE_USE_* cannot leak in.

    /// Clear all three provider env vars (restored on Drop), pinning firstParty.
    fn clear_provider_env() -> [EnvGuard; 3] {
        let mk = |k: &'static str| {
            let g = EnvGuard {
                key: k,
                prev: std::env::var(k).ok(),
            };
            std::env::remove_var(k);
            g
        };
        [
            mk("CLAUDE_CODE_USE_BEDROCK"),
            mk("CLAUDE_CODE_USE_VERTEX"),
            mk("CLAUDE_CODE_USE_FOUNDRY"),
        ]
    }

    #[test]
    fn sonnet_default_is_5_on_first_party() {
        // 2.1.198 alias table: sonnet.default = claude-sonnet-5.
        let _lock = ENV_LOCK.lock().unwrap();
        let _g = clear_provider_env();
        assert!(api_provider_is_first_party());
        assert_eq!(get_default_sonnet_model(), "claude-sonnet-5");
        // The bare `sonnet` alias resolves through the same default on firstParty.
        assert_eq!(
            resolve_agent_model(
                &AgentModel::Alias("sonnet".to_string()),
                "openai/gpt-4o",
                DEFAULT,
                None,
            ),
            "claude-sonnet-5"
        );
    }

    #[test]
    fn sonnet_default_is_4_5_on_bedrock() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _g = clear_provider_env();
        let _b = EnvGuard::set("CLAUDE_CODE_USE_BEDROCK", "1");
        assert!(!api_provider_is_first_party());
        assert_eq!(get_default_sonnet_model(), "claude-sonnet-4-5-20250929");
    }

    #[test]
    fn sonnet_default_is_4_5_on_vertex() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _g = clear_provider_env();
        let _v = EnvGuard::set("CLAUDE_CODE_USE_VERTEX", "1");
        assert!(!api_provider_is_first_party());
        assert_eq!(get_default_sonnet_model(), "claude-sonnet-4-5-20250929");
        // The bare `sonnet` alias resolves to the 3P default on Vertex. Parent is
        // non-Anthropic so no tier-match early-return can shadow it.
        assert_eq!(
            resolve_agent_model(
                &AgentModel::Alias("sonnet".to_string()),
                "openai/gpt-4o",
                DEFAULT,
                None,
            ),
            "claude-sonnet-4-5-20250929"
        );
        // `opusplan` also resolves to the Sonnet default outside plan mode, so it
        // too gets the 3P id.
        assert_eq!(
            resolve_agent_model(
                &AgentModel::Alias("opusplan".to_string()),
                "openai/gpt-4o",
                DEFAULT,
                None,
            ),
            "claude-sonnet-4-5-20250929"
        );
    }

    #[test]
    fn sonnet_default_is_4_5_on_foundry() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _g = clear_provider_env();
        let _f = EnvGuard::set("CLAUDE_CODE_USE_FOUNDRY", "1");
        assert!(!api_provider_is_first_party());
        assert_eq!(get_default_sonnet_model(), "claude-sonnet-4-5-20250929");
    }

    #[test]
    fn sonnet_env_override_wins_over_provider_default() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _g = clear_provider_env();
        let _b = EnvGuard::set("CLAUDE_CODE_USE_BEDROCK", "1");
        // ANTHROPIC_DEFAULT_SONNET_MODEL (non-empty) beats the 3P provider branch.
        let _o = EnvGuard::set("ANTHROPIC_DEFAULT_SONNET_MODEL", "custom-sonnet-id");
        assert_eq!(get_default_sonnet_model(), "custom-sonnet-id");
    }

    #[test]
    fn non_truthy_provider_env_stays_first_party() {
        // A non-allowlisted value ("0") is NOT truthy under the strict allowlist,
        // so the provider stays firstParty and Sonnet stays 4.6 — matching the TS
        // isEnvTruthy semantics.
        let _lock = ENV_LOCK.lock().unwrap();
        let _g = clear_provider_env();
        let _b = EnvGuard::set("CLAUDE_CODE_USE_BEDROCK", "0");
        assert!(api_provider_is_first_party());
        assert_eq!(get_default_sonnet_model(), "claude-sonnet-5");
    }

    #[test]
    fn opus_differs_by_provider_haiku_does_not() {
        // #2: Opus is provider-aware (firstParty→claude-opus-4-8,
        // Bedrock/Vertex/Foundry→claude-opus-4-6); Haiku has no provider branch
        // (same id on all platforms), like the existing Sonnet split.
        let _lock = ENV_LOCK.lock().unwrap();
        let _g = clear_provider_env();
        assert_eq!(get_default_opus_model(), "claude-opus-4-8");
        let haiku_fp = get_default_haiku_model();
        let _b = EnvGuard::set("CLAUDE_CODE_USE_BEDROCK", "1");
        assert_eq!(get_default_opus_model(), "claude-opus-4-6");
        assert_eq!(get_default_haiku_model(), haiku_fp);
    }

    // ---- M2 Part B: 2.1.198 registry pins (anthropicAws provider identity) --
    //
    // Verified against the REAL 2.1.198 binary registry (extracted 2026-07-02):
    // - alias table `sonnet.per_provider.anthropic_aws = "claude-sonnet-4-6"`
    //   (bedrock/vertex/foundry/mantle → "claude-sonnet-4-5",
    //   gateway → "claude-sonnet-4-6"; default "claude-sonnet-5").
    // - alias table `opus.per_provider.anthropic_aws = "claude-opus-4-7"`
    //   (bedrock/vertex/foundry → "claude-opus-4-6"; mantle/gateway →
    //   "claude-opus-4-7"; default "claude-opus-4-8").
    // - sonnet-5 `provider_ids.anthropic_aws = "claude-sonnet-5"` (same string
    //   as first_party — the id does not diverge for anthropicAws).
    //
    // LingXi's provider detection is 2-way (firstParty vs the env-detected
    // Bedrock/Vertex/Foundry), so the anthropic_aws / mantle / gateway arms
    // have no runtime representation — the ids they'd resolve to are pinned
    // here so a future anthropicAws-aware host wires the RIGHT values (and any
    // upstream alias-table change shows up as a deliberate test edit).

    #[test]
    fn registry_2_1_198_pins_match_binary() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _g = clear_provider_env();
        // Defaults (firstParty arm) — shared with the anthropic_aws sonnet-5
        // provider id, which is byte-identical to first_party.
        assert_eq!(family_default_id("sonnet"), Some("claude-sonnet-5"));
        assert_eq!(family_default_id("opus"), Some("claude-opus-4-8"));
        assert_eq!(family_default_id("haiku"), Some("claude-haiku-4-5"));
        // 3P arms the port models (bedrock/vertex/foundry per_provider).
        assert_eq!(OPUS_3P_DEFAULT_ID, "claude-opus-4-6");
        assert_eq!(SONNET_3P_DEFAULT_ID, "claude-sonnet-4-5-20250929");
    }

    #[test]
    fn registry_2_1_198_anthropic_aws_alias_targets_pass_through() {
        // The anthropic_aws per_provider alias targets (sonnet →
        // claude-sonnet-4-6, opus → claude-opus-4-7) are real catalog ids: an
        // explicit request for either must pass through verbatim so an
        // anthropicAws-configured profile can route them unmodified.
        for id in ["claude-sonnet-4-6", "claude-opus-4-7"] {
            assert_eq!(
                resolve_agent_model(
                    &AgentModel::Explicit(id.to_string()),
                    "claude-opus-4-8",
                    DEFAULT,
                    None,
                ),
                id
            );
        }
    }

    // ---- Tier3#14: alias arms honor ANTHROPIC_DEFAULT_*_MODEL env overrides --
    //
    // claude-code's parseUserSpecifiedModel routes its alias arms through
    // getDefaultSonnetModel() / getDefaultHaikuModel() / getDefaultOpusModel()
    // (model.ts:459-465), so the ANTHROPIC_DEFAULT_{OPUS,SONNET,HAIKU}_MODEL env
    // overrides win for the corresponding family alias. The full resolve path
    // (resolve_agent_model → parse_user_specified_model) must reflect that.
    //
    // Parent is non-Anthropic ("openai/gpt-4o") so no tier-match early-return
    // can shadow the alias resolution.

    #[test]
    fn opus_alias_honors_env_override() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _g = EnvGuard::set("ANTHROPIC_DEFAULT_OPUS_MODEL", "custom-opus-id");
        assert_eq!(
            resolve_agent_model(
                &AgentModel::Alias("opus".to_string()),
                "openai/gpt-4o",
                DEFAULT,
                None,
            ),
            "custom-opus-id"
        );
        // The [1m] suffix is preserved on top of the env override.
        assert_eq!(
            resolve_agent_model(
                &AgentModel::Alias("opus[1m]".to_string()),
                "openai/gpt-4o",
                DEFAULT,
                None,
            ),
            "custom-opus-id[1m]"
        );
    }

    #[test]
    fn sonnet_alias_honors_env_override() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _g = EnvGuard::set("ANTHROPIC_DEFAULT_SONNET_MODEL", "custom-sonnet-id");
        assert_eq!(
            resolve_agent_model(
                &AgentModel::Alias("sonnet".to_string()),
                "openai/gpt-4o",
                DEFAULT,
                None,
            ),
            "custom-sonnet-id"
        );
        // `opusplan` resolves to the Sonnet default outside plan mode, so it too
        // honors ANTHROPIC_DEFAULT_SONNET_MODEL.
        assert_eq!(
            resolve_agent_model(
                &AgentModel::Alias("opusplan".to_string()),
                "openai/gpt-4o",
                DEFAULT,
                None,
            ),
            "custom-sonnet-id"
        );
    }

    #[test]
    fn haiku_alias_honors_env_override() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _g = EnvGuard::set("ANTHROPIC_DEFAULT_HAIKU_MODEL", "custom-haiku-id");
        assert_eq!(
            resolve_agent_model(
                &AgentModel::Alias("haiku".to_string()),
                "openai/gpt-4o",
                DEFAULT,
                None,
            ),
            "custom-haiku-id"
        );
    }

    #[test]
    fn best_alias_honors_opus_env_override() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _g = EnvGuard::set("ANTHROPIC_DEFAULT_OPUS_MODEL", "custom-opus-id");
        // `best` → getBestModel → Opus default (ignoring any [1m] suffix).
        assert_eq!(
            resolve_agent_model(
                &AgentModel::Alias("best".to_string()),
                "openai/gpt-4o",
                DEFAULT,
                None,
            ),
            "custom-opus-id"
        );
    }

    // ── GAe/obm/dPn (2.1.198): built-in Explore model from the session model ──

    /// A built-in Explore stand-in matching the fields `GAe` consults.
    fn builtin_explore_def() -> AgentDefinition {
        let mut def = crate::builtins::builtin_agent_definitions()
            .into_iter()
            .find(|d| d.agent_type == "Explore")
            .expect("Explore is a built-in");
        // The 2.1.198 frontmatter is `model:"inherit"` (qme); assert it here so
        // the GAe tests below exercise the real definition.
        assert!(matches!(def.model, AgentModel::Inherit));
        def.model = AgentModel::Inherit;
        def
    }

    #[test]
    fn explore_on_fable_class_session_caps_at_opus() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _g = clear_provider_env();
        let def = builtin_explore_def();
        // fable/mythos-class session models name none of haiku/sonnet/opus →
        // obm true → the "opus" alias (Yyl).
        for session in ["claude-fable-5", "claude-mythos-5-20260101", "CLAUDE-FABLE-5[1m]"] {
            assert!(
                matches!(
                    resolve_builtin_explore_model(&def, session, true),
                    AgentModel::Alias(ref a) if a == "opus"
                ),
                "{session} → opus cap"
            );
        }
    }

    #[test]
    fn explore_on_claude_family_session_inherits() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _g = clear_provider_env();
        let def = builtin_explore_def();
        // dPn is a case-insensitive substring test over ["haiku","sonnet","opus"].
        for session in [
            "claude-sonnet-5",
            "claude-opus-4-8-20260115",
            "claude-haiku-4-5",
            "CLAUDE-OPUS-4-6",
        ] {
            assert!(
                matches!(resolve_builtin_explore_model(&def, session, true), AgentModel::Inherit),
                "{session} → inherit"
            );
        }
    }

    #[test]
    fn explore_on_non_first_party_env_provider_inherits() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _g = clear_provider_env();
        let def = builtin_explore_def();
        // fr() !== "firstParty" (Bedrock/Vertex/Foundry) → obm false → inherit,
        // even for a fable-class session model.
        for var in ["CLAUDE_CODE_USE_BEDROCK", "CLAUDE_CODE_USE_VERTEX", "CLAUDE_CODE_USE_FOUNDRY"] {
            let _p = EnvGuard::set(var, "1");
            assert!(
                matches!(
                    resolve_builtin_explore_model(&def, "claude-fable-5", true),
                    AgentModel::Inherit
                ),
                "{var} → inherit"
            );
        }
    }

    #[test]
    fn explore_on_non_anthropic_profile_inherits() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _g = clear_provider_env();
        let def = builtin_explore_def();
        // LingXi multi-provider: a session routed to a non-Anthropic provider
        // profile (OpenAI/Gemini/…) behaves like the non-firstParty branch.
        assert!(matches!(
            resolve_builtin_explore_model(&def, "gpt-4o", false),
            AgentModel::Inherit
        ));
    }

    #[test]
    fn non_explore_and_non_builtin_defs_keep_their_model() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _g = clear_provider_env();
        // Non-Explore built-in: model untouched (GAe early-return on agentType).
        let plan = crate::builtins::builtin_agent_definitions()
            .into_iter()
            .find(|d| d.agent_type == "Plan")
            .unwrap();
        assert!(matches!(
            resolve_builtin_explore_model(&plan, "claude-fable-5", true),
            AgentModel::Inherit
        ));
        let guide = crate::builtins::builtin_agent_definitions()
            .into_iter()
            .find(|d| d.agent_type == "claude-code-guide")
            .unwrap();
        assert!(matches!(
            resolve_builtin_explore_model(&guide, "claude-fable-5", true),
            AgentModel::Alias(ref a) if a == "haiku"
        ));
        // A USER-DEFINED agent literally named "Explore": source != built-in →
        // untouched (GAe early-return on source).
        let mut user_explore = builtin_explore_def();
        user_explore.source = AgentSource::UserDefined;
        user_explore.model = AgentModel::Alias("sonnet".to_string());
        assert!(matches!(
            resolve_builtin_explore_model(&user_explore, "claude-fable-5", true),
            AgentModel::Alias(ref a) if a == "sonnet"
        ));
    }
}
