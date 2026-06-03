//! Resolve an agent definition's model preference to a concrete wire model id.
//!
//! Mirrors claude-code's `getAgentModel` (`src/utils/model/agent.ts`) for the
//! default Anthropic / 3P path. Without this, the runner's `resolve_model`
//! emits the bare definition string (`"inherit"` / `"haiku"` / `"sonnet"`),
//! which is not a valid Anthropic model id, so built-in subagent spawns error
//! at the provider on a default install (the provider router only substitutes
//! configured `routing.aliases`). See [`resolve_agent_model`].
//!
//! ## What is modeled (the default-path core of `getAgentModel`)
//! - **`Inherit`** → the parent / main-loop model (`getDefaultSubagentModel()`
//!   returns `'inherit'`, which `getAgentModel` resolves to `parentModel`).
//! - **bare family alias** (`opus` / `sonnet` / `haiku`): if it matches the
//!   parent's tier, inherit the parent's EXACT id (claude-code's
//!   `aliasMatchesParentTier`, which prevents a surprising downgrade); else
//!   resolve to that family's default concrete id.
//! - **`Explicit("claude-…")`** → passed through verbatim.
//! - **non-family alias** → passed through (a configured `routing.aliases` may
//!   still map it).
//!
//! ## Documented smaller deferrals (not modeled here)
//! - **Live session model**: the parent model is the BOOT-time configured model
//!   (a snapshot threaded from `cfg.model`), not the live session model — so a
//!   mid-session `/model` switch is not reflected in subsequently-spawned
//!   subagents. claude-code threads the live `parentModel`.
//! - **`CLAUDE_CODE_SUBAGENT_MODEL`** env override (highest precedence in
//!   `getAgentModel`) — not honored, to keep this function pure/deterministic.
//! - **Plan-mode runtime resolution** (`getRuntimeMainLoopModel`: `opusplan`→Opus,
//!   `haiku`→Sonnet in plan mode), **Bedrock** cross-region prefix inheritance,
//!   and the per-call `toolSpecifiedModel` (absent from the frozen
//!   `SubagentSpawnRequest`) — host-specific / out of seam.
//! - **Nested spawns**: the parent model is always the main-loop model, not the
//!   immediate parent subagent's (the frozen request carries no parent model).
//! - **`in_process_teammate` path**: the persistent-teammate spawner
//!   (`tasks::handlers::in_process_teammate`) now folds in [`resolve_agent_model`]
//!   too — via `InProcessTeammateHandler::with_default_model` (wired from
//!   `cfg.model` at boot) + its `build_context` — so a wired teammate resolves
//!   its model the same way as a `PoolSubagentSpawner` spawn. Only the shared
//!   boot-snapshot vs live-`/model` caveat (above) remains.

use crate::definition::AgentModel;

/// Canonical concrete id for a bare family alias.
///
/// Mirrors the current Rust catalog (orchestrator `DEFAULT_MODEL` +
/// `handle_impl` `available_models`); claude-code sources these from
/// `getModelStrings()`. DRIFT NOTE: keep aligned with those when the default
/// model versions change.
fn family_default_id(family_lower: &str) -> Option<&'static str> {
    match family_lower {
        "opus" => Some("claude-opus-4-7"),
        "sonnet" => Some("claude-sonnet-4-6"),
        "haiku" => Some("claude-haiku-4-5"),
        _ => None,
    }
}

/// `getCanonicalName`-lite: does `parent_model` belong to `family_lower`'s tier?
/// A concrete id contains its family name (e.g. `claude-opus-4-7` → `opus`).
///
/// APPROXIMATION: a raw substring match, NOT claude-code's
/// `getCanonicalName(...).includes(...)` (which strips provider/ARN noise
/// first). It assumes `parent_model` is a real catalog wire id (the boot
/// snapshot of `cfg.model`), so it agrees with `getCanonicalName` for every
/// Anthropic / Bedrock / Vertex id — including older `claude-3-5-sonnet`-style
/// names, where a `claude-<family>`-anchored check would wrongly miss — and is
/// correctly `false` for openai/gemini parents. The only divergence is a
/// free-form alias that merely *mentions* a family word (e.g. a custom
/// deployment named `opus-prod-pool`), which cannot reach this seam today.
fn parent_is_tier(parent_model: &str, family_lower: &str) -> bool {
    parent_model.to_lowercase().contains(family_lower)
}

/// Resolve `model` to a concrete wire model string given the parent / main-loop
/// model. See the module docs for the precedence and the deferred nuances.
#[must_use]
pub fn resolve_agent_model(model: &AgentModel, parent_model: &str) -> String {
    match model {
        AgentModel::Inherit => parent_model.to_string(),
        AgentModel::Explicit(m) => m.clone(),
        AgentModel::Alias(a) => {
            let lower = a.to_lowercase();
            match family_default_id(&lower) {
                // Bare family alias: inherit the parent's EXACT model when same
                // tier (no downgrade), else resolve to the family default id.
                Some(default_id) => {
                    if parent_is_tier(parent_model, &lower) {
                        parent_model.to_string()
                    } else {
                        default_id.to_string()
                    }
                }
                // Custom alias / full id: pass through (routing.aliases may map).
                None => a.clone(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inherit_resolves_to_parent_model() {
        assert_eq!(
            resolve_agent_model(&AgentModel::Inherit, "claude-opus-4-7"),
            "claude-opus-4-7"
        );
    }

    #[test]
    fn explicit_passes_through_verbatim() {
        assert_eq!(
            resolve_agent_model(
                &AgentModel::Explicit("claude-sonnet-4-6".to_string()),
                "claude-opus-4-7"
            ),
            "claude-sonnet-4-6"
        );
    }

    #[test]
    fn family_alias_of_different_tier_resolves_to_default_id() {
        // parent is opus; agent asks for haiku/sonnet → family default concrete id.
        assert_eq!(
            resolve_agent_model(&AgentModel::Alias("haiku".to_string()), "claude-opus-4-7"),
            "claude-haiku-4-5"
        );
        assert_eq!(
            resolve_agent_model(&AgentModel::Alias("sonnet".to_string()), "claude-opus-4-7"),
            "claude-sonnet-4-6"
        );
    }

    #[test]
    fn family_alias_matching_parent_tier_inherits_exact_parent() {
        // parent IS opus → "opus" inherits the parent's exact id, not a default.
        assert_eq!(
            resolve_agent_model(&AgentModel::Alias("opus".to_string()), "claude-opus-4-9-future"),
            "claude-opus-4-9-future"
        );
        assert_eq!(
            resolve_agent_model(&AgentModel::Alias("sonnet".to_string()), "claude-sonnet-4-6"),
            "claude-sonnet-4-6"
        );
    }

    #[test]
    fn family_alias_is_case_insensitive() {
        assert_eq!(
            resolve_agent_model(&AgentModel::Alias("Haiku".to_string()), "claude-opus-4-7"),
            "claude-haiku-4-5"
        );
    }

    #[test]
    fn non_family_alias_passes_through() {
        // A custom alias the user may have mapped via routing.aliases.
        assert_eq!(
            resolve_agent_model(&AgentModel::Alias("my-fast-model".to_string()), "claude-opus-4-7"),
            "my-fast-model"
        );
    }

    #[test]
    fn family_alias_with_non_anthropic_parent_resolves_to_default_id() {
        // The exact scenario this batch fixes: a 3P / non-Anthropic parent is
        // not any Claude tier, so a family alias resolves to the family default
        // (NOT the parent), instead of leaking the raw alias to the provider.
        assert_eq!(
            resolve_agent_model(&AgentModel::Alias("opus".to_string()), "openai/gpt-4o"),
            "claude-opus-4-7"
        );
        assert_eq!(
            resolve_agent_model(&AgentModel::Alias("sonnet".to_string()), "gemini/gemini-2.0-flash"),
            "claude-sonnet-4-6"
        );
    }

    #[test]
    fn cross_tier_family_alias_resolves_to_that_familys_default() {
        // parent is sonnet, agent asks for opus (different tier, upgrade
        // direction) → opus default id, locking the tier asymmetry.
        assert_eq!(
            resolve_agent_model(&AgentModel::Alias("opus".to_string()), "claude-sonnet-4-6"),
            "claude-opus-4-7"
        );
    }
}
