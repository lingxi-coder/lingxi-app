//! Model deprecation utilities.
//!
//! Direct port of claude-code `utils/model/deprecation.ts`. Contains the
//! provider-keyed retirement-date table for deprecated models plus the
//! [`model_deprecation_warning`] lookup that maps a configured model id to the
//! one-line startup warning (or `None` when the model is current).
//!
//! ## Surfacing (1:1 fidelity note)
//!
//! In TS the warning is produced by `getModelDeprecationWarning(resolvedInitialModel)`
//! at `main.tsx:2873` and pushed onto the startup notification queue (key
//! `model-deprecation-warning`, `color: 'warning'`, `priority: 'high'`) right
//! beside the permission-mode notice (`main.tsx:2889-2896`). The Rust port has
//! not yet built that startup-notification queue (nor its sibling
//! `permissionModeNotification`, nor the `resolvedInitialModel` resolution
//! chain), so this exposes the faithful **pure lookup** ready to wire when the
//! queue lands — exactly the pattern used for the firstParty model literals in
//! `api-client/opus.rs` / `model_capabilities.rs`, which are ported ahead of
//! full provider wiring. With any current (Claude 4-generation) default model
//! the lookup returns `None`, so startup stays byte-identical until a user
//! configures one of the deprecated Claude 3 ids below.

/// API provider, mirroring `APIProvider` (`utils/model/providers.ts:4`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ApiProvider {
    FirstParty,
    Bedrock,
    Vertex,
    Foundry,
}

/// Port of `isEnvTruthy` (`envUtils.ts:32-37`): truthy iff the lowercased,
/// trimmed value is one of `1`/`true`/`yes`/`on`. Kept module-local to mirror
/// the sibling copy in `model_capabilities.rs`.
fn is_env_truthy(key: &str) -> bool {
    std::env::var(key)
        .is_ok_and(|v| matches!(v.to_lowercase().trim(), "1" | "true" | "yes" | "on"))
}

/// Port of `getAPIProvider()` (`utils/model/providers.ts:6-14`): Bedrock,
/// Vertex, Foundry env flags checked in that order, else first-party.
fn api_provider() -> ApiProvider {
    if is_env_truthy("CLAUDE_CODE_USE_BEDROCK") {
        ApiProvider::Bedrock
    } else if is_env_truthy("CLAUDE_CODE_USE_VERTEX") {
        ApiProvider::Vertex
    } else if is_env_truthy("CLAUDE_CODE_USE_FOUNDRY") {
        ApiProvider::Foundry
    } else {
        ApiProvider::FirstParty
    }
}

/// A deprecated model's display name and its per-provider retirement dates.
/// `None` for a provider means the model is not deprecated there (TS `null`).
struct DeprecationEntry {
    /// Human-readable model name (TS `modelName`).
    model_name: &'static str,
    /// Retirement date for the first-party Anthropic API, or `None`.
    first_party: Option<&'static str>,
    /// Retirement date for Bedrock, or `None`.
    bedrock: Option<&'static str>,
    /// Retirement date for Vertex, or `None`.
    vertex: Option<&'static str>,
    /// Retirement date for Foundry, or `None`.
    foundry: Option<&'static str>,
}

impl DeprecationEntry {
    fn retirement_date(&self, provider: ApiProvider) -> Option<&'static str> {
        match provider {
            ApiProvider::FirstParty => self.first_party,
            ApiProvider::Bedrock => self.bedrock,
            ApiProvider::Vertex => self.vertex,
            ApiProvider::Foundry => self.foundry,
        }
    }
}

/// Deprecated models and their retirement dates by provider, byte-locked to
/// `DEPRECATED_MODELS` (`utils/model/deprecation.ts:33-61`). The first tuple
/// element is the case-insensitive **substring** matched against the model id;
/// to add a new deprecated model, add an entry here.
const DEPRECATED_MODELS: &[(&str, DeprecationEntry)] = &[
    (
        "claude-3-opus",
        DeprecationEntry {
            model_name: "Claude 3 Opus",
            first_party: Some("January 5, 2026"),
            bedrock: Some("January 15, 2026"),
            vertex: Some("January 5, 2026"),
            foundry: Some("January 5, 2026"),
        },
    ),
    (
        "claude-3-7-sonnet",
        DeprecationEntry {
            model_name: "Claude 3.7 Sonnet",
            first_party: Some("February 19, 2026"),
            bedrock: Some("April 28, 2026"),
            vertex: Some("May 11, 2026"),
            foundry: Some("February 19, 2026"),
        },
    ),
    (
        "claude-3-5-haiku",
        DeprecationEntry {
            model_name: "Claude 3.5 Haiku",
            first_party: Some("February 19, 2026"),
            bedrock: None,
            vertex: None,
            foundry: None,
        },
    ),
];

/// Resolved deprecation info for a model id (TS `DeprecatedModelInfo`).
struct DeprecatedModelInfo {
    model_name: &'static str,
    retirement_date: &'static str,
}

/// Port of `getDeprecatedModelInfo` (`deprecation.ts:66-83`): scan the table in
/// declaration order; the first entry whose key is a substring of the
/// lowercased model id AND has a non-null retirement date for the active
/// provider wins. Returns `None` (TS `{ isDeprecated: false }`) otherwise.
fn deprecated_model_info(model_id: &str) -> Option<DeprecatedModelInfo> {
    let lowercase = model_id.to_lowercase();
    let provider = api_provider();
    for (key, value) in DEPRECATED_MODELS {
        let Some(retirement_date) = value.retirement_date(provider) else {
            continue;
        };
        if !lowercase.contains(key) {
            continue;
        }
        return Some(DeprecatedModelInfo {
            model_name: value.model_name,
            retirement_date,
        });
    }
    None
}

/// Get a deprecation warning message for a model, or `None` if not deprecated.
///
/// Direct port of `getModelDeprecationWarning` (`deprecation.ts:88-101`). A
/// `None`/empty model id yields `None` (TS `if (!modelId) return null`). The
/// returned string is byte-identical to TS, including the leading `⚠ ` glyph.
#[must_use]
pub fn model_deprecation_warning(model_id: Option<&str>) -> Option<String> {
    let model_id = model_id.filter(|m| !m.is_empty())?;
    let info = deprecated_model_info(model_id)?;
    Some(format!(
        "⚠ {} will be retired on {}. Consider switching to a newer model.",
        info.model_name, info.retirement_date
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Env access in these tests is process-global; serialize them so the
    /// provider flags one test sets can't leak into another running in
    /// parallel.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn clear_provider_env() {
        std::env::remove_var("CLAUDE_CODE_USE_BEDROCK");
        std::env::remove_var("CLAUDE_CODE_USE_VERTEX");
        std::env::remove_var("CLAUDE_CODE_USE_FOUNDRY");
    }

    #[test]
    fn none_or_empty_model_yields_no_warning() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_provider_env();
        assert_eq!(model_deprecation_warning(None), None);
        assert_eq!(model_deprecation_warning(Some("")), None);
    }

    #[test]
    fn current_model_yields_no_warning() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_provider_env();
        // The Rust port's default first-party model set is the Claude 4
        // generation — none of these match a deprecation key, so startup is
        // byte-identical (no warning) in the common case.
        for current in [
            "claude-opus-4-6",
            "claude-opus-4-7",
            "claude-sonnet-4-6",
            "claude-haiku-4-5",
            "claude-haiku-4-5-20251001",
            "gpt-4o",
        ] {
            assert_eq!(
                model_deprecation_warning(Some(current)),
                None,
                "{current} should not warn"
            );
        }
    }

    #[test]
    fn deprecated_first_party_models_warn() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_provider_env();
        assert_eq!(
            model_deprecation_warning(Some("claude-3-opus-20240229")).as_deref(),
            Some("⚠ Claude 3 Opus will be retired on January 5, 2026. Consider switching to a newer model.")
        );
        assert_eq!(
            model_deprecation_warning(Some("claude-3-7-sonnet-20250219")).as_deref(),
            Some(
                "⚠ Claude 3.7 Sonnet will be retired on February 19, 2026. Consider switching to a newer model."
            )
        );
        assert_eq!(
            model_deprecation_warning(Some("claude-3-5-haiku-20241022")).as_deref(),
            Some(
                "⚠ Claude 3.5 Haiku will be retired on February 19, 2026. Consider switching to a newer model."
            )
        );
    }

    #[test]
    fn substring_match_is_case_insensitive() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_provider_env();
        assert_eq!(
            model_deprecation_warning(Some("CLAUDE-3-OPUS-20240229")).as_deref(),
            Some("⚠ Claude 3 Opus will be retired on January 5, 2026. Consider switching to a newer model.")
        );
        // Bedrock-prefixed id still matches the substring.
        assert!(model_deprecation_warning(Some("anthropic.claude-3-opus-20240229-v1:0")).is_some());
    }

    #[test]
    fn provider_gates_retirement_date() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_provider_env();

        // Bedrock has its own Opus date and no Haiku 3.5 date (null).
        std::env::set_var("CLAUDE_CODE_USE_BEDROCK", "1");
        assert_eq!(
            model_deprecation_warning(Some("claude-3-opus-20240229")).as_deref(),
            Some("⚠ Claude 3 Opus will be retired on January 15, 2026. Consider switching to a newer model.")
        );
        // claude-3-5-haiku is null on Bedrock -> no warning.
        assert_eq!(model_deprecation_warning(Some("claude-3-5-haiku-20241022")), None);
        clear_provider_env();

        // Vertex 3.7 Sonnet date differs from first-party.
        std::env::set_var("CLAUDE_CODE_USE_VERTEX", "1");
        assert_eq!(
            model_deprecation_warning(Some("claude-3-7-sonnet-20250219")).as_deref(),
            Some("⚠ Claude 3.7 Sonnet will be retired on May 11, 2026. Consider switching to a newer model.")
        );
        clear_provider_env();
    }
}
