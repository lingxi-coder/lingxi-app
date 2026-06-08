//! SKILLEXEC.3 (model scope): resolve a skill's `model:` frontmatter against the
//! session's current main-loop model, carrying the `[1m]` 1M-context suffix over
//! when the target family supports it.
//!
//! 1:1 port of claude-code `resolveSkillModelOverride`
//! (`src/utils/model/model.ts:523-536`) and the helpers it leans on:
//!   - `has1mContext` / `modelSupports1M` (`src/utils/context.ts:35-49`),
//!   - the alias-resolution subset of `parseUserSpecifiedModel`
//!     (`src/utils/model/model.ts:445-505`) reachable from this seam.
//!
//! ## Why `[1m]` must be carried over
//! A skill author writing `model: opus` means "use opus-class reasoning" — not
//! "downgrade to 200K". If the user is on `opus[1m]` and invokes a skill with a
//! bare `model: opus`, passing the alias through unchanged would drop the
//! effective window from 1M to 200K and trip autocompact at ~23% apparent usage.
//! So when the CURRENT model carries `[1m]`, the SKILL model does NOT, and the
//! skill's resolved family supports 1M, we re-append `[1m]`.
//!
//! ## DRIFT NOTES (deliberate, documented deferrals vs. the full TS)
//! - `model_supports_1m` mirrors the LITERAL TS check (`claude-sonnet-4` /
//!   `opus-4-6` substrings). When the catalog default opus moves past 4-6 this
//!   check intentionally stops re-appending `[1m]` for the bare `opus` alias —
//!   exactly as the unchanged TS would, until its `@[MODEL LAUNCH]` pattern is
//!   bumped. Keep this aligned with `compaction::context_window`.
//! - the family default ids (`opus`→`claude-opus-4-7`, …) mirror
//!   `agent::model_resolution::family_default_id` + orchestrator `DEFAULT_MODEL`.
//!   Keep them aligned when the catalog versions change.
//! - the ant-model registry, the legacy Opus 4.0/4.1 first-party remap, and the
//!   Foundry deployment-id passthrough branches of `parseUserSpecifiedModel` are
//!   NOT modeled — none is reachable from a skill `model:` string in this port.

use std::env;

/// Resolve `skill_model` against `current_model`, carrying `[1m]` over when the
/// target family supports it. 1:1 with `resolveSkillModelOverride`
/// (`model.ts:523-536`).
#[must_use]
pub fn resolve_skill_model_override(skill_model: &str, current_model: &str) -> String {
    // Already explicitly 1M, or the current model is not 1M → pass through.
    if has_1m_context(skill_model) || !has_1m_context(current_model) {
        return skill_model.to_string();
    }
    // The current model is 1M but the skill model is a bare alias / id without
    // the suffix: re-append `[1m]` iff the resolved family supports it.
    if model_supports_1m(&parse_user_specified_model(skill_model)) {
        return format!("{skill_model}[1m]");
    }
    skill_model.to_string()
}

/// Mirrors `is1mContextDisabled` (`utils/context.ts:31-33`).
fn is_1m_context_disabled() -> bool {
    is_env_truthy(env::var("CLAUDE_CODE_DISABLE_1M_CONTEXT").ok().as_deref())
}

/// `true` if `model` carries an explicit `[1m]` suffix (case-insensitive),
/// unless 1M context is disabled. Mirrors `has1mContext` (`context.ts:35-40`).
fn has_1m_context(model: &str) -> bool {
    if is_1m_context_disabled() {
        return false;
    }
    // TS uses /\[1m\]/i — a case-insensitive substring of the literal "[1m]".
    model.to_lowercase().contains("[1m]")
}

/// `true` if the canonical model family supports 1M context (sonnet-4 family or
/// opus-4-6), unless 1M context is disabled. Mirrors `modelSupports1M`
/// (`context.ts:43-49`).
fn model_supports_1m(model: &str) -> bool {
    if is_1m_context_disabled() {
        return false;
    }
    let canonical = canonical_name(model);
    canonical.contains("claude-sonnet-4") || canonical.contains("opus-4-6")
}

/// Family default concrete id for a bare alias. Mirrors `getDefault*Model()`
/// resolution; see the module DRIFT NOTE on keeping the catalog aligned.
fn family_default_id(family_lower: &str) -> Option<&'static str> {
    match family_lower {
        "opus" => Some("claude-opus-4-7"),
        "sonnet" => Some("claude-sonnet-4-6"),
        "haiku" => Some("claude-haiku-4-5"),
        _ => None,
    }
}

/// Resolve a user/skill-specified model string to a concrete id — the
/// alias-resolution subset of `parseUserSpecifiedModel` (`model.ts:445-505`)
/// that this seam can reach. Bare family aliases (`opus` / `sonnet` / `haiku` /
/// `opusplan`) map to their default id (preserving a trailing `[1m]`); every
/// other string passes through with only `[1m]` normalized. See module DRIFT
/// NOTE for the unmodeled branches.
fn parse_user_specified_model(model_input: &str) -> String {
    let trimmed = model_input.trim();
    let normalized = trimmed.to_lowercase();
    let has_1m_tag = has_1m_context(&normalized);
    let base = strip_1m_suffix(&normalized);
    let suffix = if has_1m_tag { "[1m]" } else { "" };

    match base.as_str() {
        // `opusplan` → Sonnet default (Opus only in plan mode), 1:1 with TS.
        "opusplan" | "sonnet" => format!("{}{suffix}", family_default_id("sonnet").unwrap()),
        "haiku" => format!("{}{suffix}", family_default_id("haiku").unwrap()),
        "opus" => format!("{}{suffix}", family_default_id("opus").unwrap()),
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

/// Strip a single trailing `[1m]` (case-insensitive) and trim. Mirrors the TS
/// `replace(/\[1m]$/i, '').trim()`.
fn strip_1m_suffix(s: &str) -> String {
    if s.to_lowercase().ends_with("[1m]") {
        s[..s.len() - 4].trim().to_string()
    } else {
        s.trim().to_string()
    }
}

/// Resolve a full model id to a shorter canonical family name. Faithful copy of
/// `compaction::context_window::canonical_name` (ports `firstPartyNameToCanonical`,
/// `model.ts:217-270`); the Bedrock-ARN indirection is a no-op for our substring
/// checks, so it is folded in.
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

/// Mirrors `isEnvTruthy` (`utils/envUtils.ts:32-37`): `1` / `true` / `yes` /
/// `on` (case-insensitive, trimmed) are truthy.
fn is_env_truthy(value: Option<&str>) -> bool {
    match value {
        Some(v) => matches!(v.to_lowercase().trim(), "1" | "true" | "yes" | "on"),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_model_already_1m_passes_through() {
        // The skill explicitly asked for 1M → never touched.
        assert_eq!(
            resolve_skill_model_override("claude-sonnet-4-6[1m]", "claude-opus-4-6[1m]"),
            "claude-sonnet-4-6[1m]"
        );
    }

    #[test]
    fn current_model_not_1m_passes_through() {
        // Session is plain 200K → no [1m] to carry over → unchanged.
        assert_eq!(
            resolve_skill_model_override("claude-sonnet-4-6", "claude-opus-4-6"),
            "claude-sonnet-4-6"
        );
        assert_eq!(
            resolve_skill_model_override("opus", "claude-opus-4-6"),
            "opus"
        );
    }

    #[test]
    fn carries_1m_when_current_is_1m_and_family_supports_it() {
        // Full id whose family supports 1M → re-append [1m].
        assert_eq!(
            resolve_skill_model_override("claude-sonnet-4-6", "claude-opus-4-6[1m]"),
            "claude-sonnet-4-6[1m]"
        );
    }

    #[test]
    fn bare_alias_resolves_then_carries_1m() {
        // `sonnet` resolves to claude-sonnet-4-6 (supports 1M) → [1m] re-appended,
        // matching the TS `parseUserSpecifiedModel` pre-resolution step.
        assert_eq!(
            resolve_skill_model_override("sonnet", "claude-opus-4-6[1m]"),
            "sonnet[1m]"
        );
    }

    #[test]
    fn family_without_1m_variant_does_not_carry() {
        // haiku has no 1M variant → downgrade stands, no [1m] re-appended.
        assert_eq!(
            resolve_skill_model_override("haiku", "claude-opus-4-6[1m]"),
            "haiku"
        );
        assert_eq!(
            resolve_skill_model_override("claude-haiku-4-5", "claude-sonnet-4-6[1m]"),
            "claude-haiku-4-5"
        );
    }

    #[test]
    fn parse_resolves_known_aliases() {
        assert_eq!(parse_user_specified_model("opus"), "claude-opus-4-7");
        assert_eq!(parse_user_specified_model("sonnet"), "claude-sonnet-4-6");
        assert_eq!(parse_user_specified_model("haiku"), "claude-haiku-4-5");
        assert_eq!(parse_user_specified_model("opusplan"), "claude-sonnet-4-6");
        // Unknown / full id passes through (case preserved).
        assert_eq!(
            parse_user_specified_model("claude-sonnet-4-6"),
            "claude-sonnet-4-6"
        );
        assert_eq!(parse_user_specified_model("My-Custom-ID"), "My-Custom-ID");
    }
}
