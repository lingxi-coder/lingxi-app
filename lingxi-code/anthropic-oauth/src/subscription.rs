//! Subscription resolution from OAuth scopes and the profile response.
//!
//! Ports the two claude-code predicates the retry / 429 / beta batches consume:
//! - [`subscription_from_scopes`] — `auth.ts:1564-1571` `isClaudeAISubscriber`,
//!   which delegates to `services/oauth/client.ts:38-40`
//!   `shouldUseClaudeAIAuth(scopes) = Boolean(scopes?.includes('user:inference'))`.
//! - [`is_enterprise`] — `auth.ts:1694` `isEnterpriseSubscriber` =
//!   `getSubscriptionType() === 'enterprise'`.
//!
//! Plus [`apply_profile`], which folds an [`OAuthProfileResponse`] into a
//! [`ClaudeAiLimitsState`] by populating `subscription_type` from the org-type
//! discriminant (`client.ts:370-387`).
//!
//! **Divergence (documented):** TS `isClaudeAISubscriber` first checks
//! `isAnthropicAuthEnabled()` and returns `false` when Anthropic auth is off
//! (env API keys / third-party providers). That gate depends on global config
//! we do not see here, so callers must apply it; [`subscription_from_scopes`]
//! ports only the scope check. `isEnterpriseSubscriber` reads
//! `getSubscriptionType()`, which can come from a mock (ant-only), the stored
//! token's `subscriptionType`, or the profile org-type — we resolve from the
//! tier we *have* and conservatively treat an absent / ambiguous tier as
//! non-enterprise and non-subscriber.

use crate::limits::{ClaudeAiLimitsState, SubscriptionType};
use crate::profile::OAuthProfileResponse;

/// `CLAUDE_AI_INFERENCE_SCOPE` — `constants/oauth.ts:33`. Locked byte-for-byte.
/// Presence of this scope is what distinguishes a real Claude.ai login token
/// from an inference-only / API-key session.
pub const CLAUDE_AI_INFERENCE_SCOPE: &str = "user:inference";

/// `CLAUDE_AI_PROFILE_SCOPE` — `constants/oauth.ts:34`. Gates profile-scoped
/// endpoint calls so service-key sessions don't 403-storm (`hasProfileScope`,
/// `auth.ts:1580-1584`).
pub const CLAUDE_AI_PROFILE_SCOPE: &str = "user:profile";

/// Port of `shouldUseClaudeAIAuth(scopes)` (`client.ts:38-40`): the user is a
/// Claude.ai subscriber iff the token's scopes include `user:inference`.
///
/// Mirrors `Boolean(scopes?.includes(CLAUDE_AI_INFERENCE_SCOPE))`. An empty
/// scope vector → `false`.
#[must_use]
pub fn subscription_from_scopes(scopes: &[String]) -> bool {
    scopes.iter().any(|s| s == CLAUDE_AI_INFERENCE_SCOPE)
}

/// Port of `hasProfileScope()` (`auth.ts:1580-1584`): whether the token carries
/// the `user:profile` scope (required to hit `/api/oauth/profile`).
#[must_use]
pub fn has_profile_scope(scopes: &[String]) -> bool {
    scopes.iter().any(|s| s == CLAUDE_AI_PROFILE_SCOPE)
}

/// Port of `isEnterpriseSubscriber()` (`auth.ts:1694`): the resolved tier is
/// Enterprise.
///
/// TS reads `getSubscriptionType() === 'enterprise'`. We read the tier already
/// resolved into `state.subscription_type`; absent / ambiguous → `false`
/// (conservative). Team-with-PAYG is NOT enterprise in TS (`isTeamPremiumSubscriber`
/// is a distinct predicate), so we do not fold it in here.
#[must_use]
pub fn is_enterprise(state: &ClaudeAiLimitsState) -> bool {
    state.subscription_type == Some(SubscriptionType::Enterprise)
}

/// Whether the resolved tier is a paid Claude.ai subscription
/// (`max`/`pro`/`team`/`enterprise`). Useful for the api-client batches that
/// gate on subscriber-ness when scopes are unavailable but a profile was
/// fetched. `Free`/`Unknown`/absent → `false`.
#[must_use]
pub fn is_subscriber_tier(state: &ClaudeAiLimitsState) -> bool {
    matches!(
        state.subscription_type,
        Some(
            SubscriptionType::Max
                | SubscriptionType::Pro
                | SubscriptionType::Team
                | SubscriptionType::Enterprise
        )
    )
}

/// Fold a fetched [`OAuthProfileResponse`] into a [`ClaudeAiLimitsState`],
/// populating `subscription_type` from the org-type discriminant
/// (`client.ts:370-387` via [`OAuthProfileResponse::subscription_type`]).
///
/// Conservative: an unknown / absent org type leaves `subscription_type`
/// untouched (we do NOT clobber a previously-known tier with `None`), matching
/// the TS coalescing `profileInfo?.subscriptionType ?? existing?.subscriptionType`.
pub fn apply_profile(state: &mut ClaudeAiLimitsState, profile: &OAuthProfileResponse) {
    if let Some(tier) = profile.subscription_type() {
        state.subscription_type = Some(tier);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scopes(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn inference_scope_makes_subscriber() {
        assert!(subscription_from_scopes(&scopes(&[
            "user:profile",
            "user:inference",
            "org:create_api_key"
        ])));
    }

    #[test]
    fn missing_inference_scope_is_not_subscriber() {
        assert!(!subscription_from_scopes(&scopes(&["user:profile"])));
        assert!(!subscription_from_scopes(&scopes(&[])));
    }

    #[test]
    fn inference_only_token_is_subscriber() {
        // Long-lived inference-only tokens hardcode ['user:inference'].
        assert!(subscription_from_scopes(&scopes(&["user:inference"])));
    }

    #[test]
    fn profile_scope_detection() {
        assert!(has_profile_scope(&scopes(&["user:inference", "user:profile"])));
        assert!(!has_profile_scope(&scopes(&["user:inference"])));
    }

    #[test]
    fn enterprise_tier_is_enterprise() {
        let state = ClaudeAiLimitsState {
            subscription_type: Some(SubscriptionType::Enterprise),
            ..Default::default()
        };
        assert!(is_enterprise(&state));
        assert!(is_subscriber_tier(&state));
    }

    #[test]
    fn non_enterprise_tiers_are_not_enterprise() {
        for tier in [
            SubscriptionType::Free,
            SubscriptionType::Pro,
            SubscriptionType::Max,
            SubscriptionType::Team,
            SubscriptionType::Unknown,
        ] {
            let state = ClaudeAiLimitsState {
                subscription_type: Some(tier),
                ..Default::default()
            };
            assert!(!is_enterprise(&state), "{tier:?} must not be enterprise");
        }
    }

    #[test]
    fn ambiguous_absent_tier_is_conservative() {
        let state = ClaudeAiLimitsState::default();
        assert!(!is_enterprise(&state));
        assert!(!is_subscriber_tier(&state));
    }

    #[test]
    fn subscriber_tier_excludes_free_and_unknown() {
        for tier in [SubscriptionType::Free, SubscriptionType::Unknown] {
            let state = ClaudeAiLimitsState {
                subscription_type: Some(tier),
                ..Default::default()
            };
            assert!(!is_subscriber_tier(&state), "{tier:?} must not be a subscriber");
        }
        for tier in [
            SubscriptionType::Pro,
            SubscriptionType::Max,
            SubscriptionType::Team,
            SubscriptionType::Enterprise,
        ] {
            let state = ClaudeAiLimitsState {
                subscription_type: Some(tier),
                ..Default::default()
            };
            assert!(is_subscriber_tier(&state), "{tier:?} must be a subscriber");
        }
    }

    #[test]
    fn apply_profile_populates_tier() {
        let profile = OAuthProfileResponse {
            organization: Some(crate::profile::OAuthOrganization {
                organization_type: Some("claude_team".into()),
                ..Default::default()
            }),
            account: None,
        };
        let mut state = ClaudeAiLimitsState::default();
        apply_profile(&mut state, &profile);
        assert_eq!(state.subscription_type, Some(SubscriptionType::Team));
        assert!(is_subscriber_tier(&state));
    }

    #[test]
    fn apply_profile_does_not_clobber_known_tier_with_unknown() {
        let mut state = ClaudeAiLimitsState {
            subscription_type: Some(SubscriptionType::Max),
            ..Default::default()
        };
        let unknown = OAuthProfileResponse {
            organization: Some(crate::profile::OAuthOrganization {
                organization_type: Some("claude_galaxy".into()),
                ..Default::default()
            }),
            account: None,
        };
        apply_profile(&mut state, &unknown);
        // Known Max tier survives an unknown-org-type profile.
        assert_eq!(state.subscription_type, Some(SubscriptionType::Max));
    }
}
