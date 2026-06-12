//! Claude.ai subscription-tier snapshot shared between the composition roots
//! (which resolve it from the OAuth profile/roles endpoints) and UI layers
//! (which branch rate-limit copy on it).
//!
//! Mirrors the TS subscription data sources: `getSubscriptionType()` /
//! `getRateLimitTier()` (`utils/auth.ts:1662-1712`, keychain token fields),
//! and `getOauthAccountInfo()` (`hasExtraUsageEnabled` / `billingType` /
//! `organizationRole`, written at login from the profile + roles endpoints).
//! Tier strings use the TS union values verbatim: `"pro" | "max" | "team" |
//! "enterprise"`. Absent / unknown tier ⇒ every predicate is conservative
//! `false`, reproducing the pre-plumbing scope-guard behaviour byte-for-byte.

use std::sync::{Arc, RwLock};

/// Point-in-time view of the signed-in user's subscription. `Default` is the
/// "unknown subscription" state (all predicates `false`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SubscriptionSnapshot {
    /// `isClaudeAISubscriber()` (`auth.ts:1564`) — pre-computed at the
    /// composition root from token scopes + auth-source precedence.
    pub is_subscriber: bool,
    /// `getSubscriptionType()`: `"pro" | "max" | "team" | "enterprise"`.
    pub subscription_type: Option<String>,
    /// `getRateLimitTier()`: e.g. `"default_claude_max_20x"`.
    pub rate_limit_tier: Option<String>,
    /// `oauthAccount.hasExtraUsageEnabled === true`.
    pub has_extra_usage_enabled: bool,
    /// `oauthAccount.billingType`, e.g. `"stripe_subscription"`.
    pub billing_type: Option<String>,
    /// `oauthAccount.organizationRole`, e.g. `"admin"`.
    pub organization_role: Option<String>,
}

impl SubscriptionSnapshot {
    /// `subscriptionType === 'team' || subscriptionType === 'enterprise'`.
    #[must_use]
    pub fn is_team_or_enterprise(&self) -> bool {
        matches!(
            self.subscription_type.as_deref(),
            Some("team" | "enterprise")
        )
    }

    /// `subscriptionType === 'pro' || subscriptionType === 'enterprise'`
    /// (the `seven_day_sonnet` naming gate, `rateLimitMessages.ts:176-181`).
    #[must_use]
    pub fn is_pro_or_enterprise(&self) -> bool {
        matches!(
            self.subscription_type.as_deref(),
            Some("pro" | "enterprise")
        )
    }

    /// `getRateLimitTier() === 'default_claude_max_20x'`
    /// (`RateLimitMessage.tsx:75`).
    #[must_use]
    pub fn is_max20x(&self) -> bool {
        self.rate_limit_tier.as_deref() == Some("default_claude_max_20x")
    }

    /// Port of `isOverageProvisioningAllowed` (`auth.ts:1623-1643`): must be a
    /// subscriber with a billing type that can purchase extra usage (Stripe or
    /// mobile billing).
    #[must_use]
    pub fn is_overage_provisioning_allowed(&self) -> bool {
        if !self.is_subscriber {
            return false;
        }
        matches!(
            self.billing_type.as_deref(),
            Some(
                "stripe_subscription"
                    | "stripe_subscription_contracted"
                    | "apple_subscription"
                    | "google_play_subscription"
            )
        )
    }

    /// Port of `hasClaudeAiBillingAccess` (`billing.ts:53-78`; the
    /// `/mock-limits` override is not ported). Consumer plans always have
    /// billing access; Team/Enterprise gate on the org role.
    #[must_use]
    pub fn has_claude_ai_billing_access(&self) -> bool {
        if !self.is_subscriber {
            return false;
        }
        match self.subscription_type.as_deref() {
            Some("max" | "pro") => true,
            _ => matches!(
                self.organization_role.as_deref(),
                Some("admin" | "billing" | "owner" | "primary_owner")
            ),
        }
    }

    /// Port of `extraUsage.isEnabled()` for the interactive session arm
    /// (`commands/extra-usage/index.ts:6-17`):
    /// `!isEnvTruthy(DISABLE_EXTRA_USAGE_COMMAND) && isOverageProvisioningAllowed()`.
    /// The env read stays at the caller (keeps this type pure).
    #[must_use]
    pub fn is_extra_usage_command_enabled(&self, disable_env_truthy: bool) -> bool {
        !disable_env_truthy && self.is_overage_provisioning_allowed()
    }
}

/// Shared slot the composition root fills asynchronously (a background
/// profile/roles fetch) and UI layers read at compose time. `None` until the
/// fetch lands; readers treat `None` as `SubscriptionSnapshot::default()`.
pub type SharedSubscription = Arc<RwLock<Option<SubscriptionSnapshot>>>;

#[cfg(test)]
mod tests {
    use super::*;

    fn team_snapshot() -> SubscriptionSnapshot {
        SubscriptionSnapshot {
            is_subscriber: true,
            subscription_type: Some("team".to_string()),
            billing_type: Some("stripe_subscription".to_string()),
            ..SubscriptionSnapshot::default()
        }
    }

    #[test]
    fn default_snapshot_resolves_all_predicates_false() {
        let snap = SubscriptionSnapshot::default();
        assert!(!snap.is_team_or_enterprise());
        assert!(!snap.is_pro_or_enterprise());
        assert!(!snap.is_max20x());
        assert!(!snap.is_overage_provisioning_allowed());
        assert!(!snap.has_claude_ai_billing_access());
        assert!(!snap.is_extra_usage_command_enabled(false));
    }

    #[test]
    fn team_and_enterprise_predicates() {
        let team = team_snapshot();
        assert!(team.is_team_or_enterprise());
        assert!(!team.is_pro_or_enterprise());

        let enterprise = SubscriptionSnapshot {
            subscription_type: Some("enterprise".to_string()),
            ..team_snapshot()
        };
        assert!(enterprise.is_team_or_enterprise());
        assert!(enterprise.is_pro_or_enterprise());

        let pro = SubscriptionSnapshot {
            subscription_type: Some("pro".to_string()),
            ..team_snapshot()
        };
        assert!(!pro.is_team_or_enterprise());
        assert!(pro.is_pro_or_enterprise());
    }

    #[test]
    fn max20x_is_exact_tier_match() {
        let max20 = SubscriptionSnapshot {
            rate_limit_tier: Some("default_claude_max_20x".to_string()),
            ..SubscriptionSnapshot::default()
        };
        assert!(max20.is_max20x());

        let max5 = SubscriptionSnapshot {
            rate_limit_tier: Some("default_claude_max_5x".to_string()),
            ..SubscriptionSnapshot::default()
        };
        assert!(!max5.is_max20x());
    }

    #[test]
    fn overage_provisioning_requires_subscriber_and_billing_type() {
        for billing in [
            "stripe_subscription",
            "stripe_subscription_contracted",
            "apple_subscription",
            "google_play_subscription",
        ] {
            let snap = SubscriptionSnapshot {
                billing_type: Some(billing.to_string()),
                ..team_snapshot()
            };
            assert!(
                snap.is_overage_provisioning_allowed(),
                "billing type {billing} should allow overage provisioning"
            );
        }

        let marketplace = SubscriptionSnapshot {
            billing_type: Some("aws_marketplace".to_string()),
            ..team_snapshot()
        };
        assert!(!marketplace.is_overage_provisioning_allowed());

        let no_billing = SubscriptionSnapshot {
            billing_type: None,
            ..team_snapshot()
        };
        assert!(!no_billing.is_overage_provisioning_allowed());

        let non_subscriber = SubscriptionSnapshot {
            is_subscriber: false,
            ..team_snapshot()
        };
        assert!(!non_subscriber.is_overage_provisioning_allowed());
    }

    #[test]
    fn billing_access_pro_max_always_team_by_role() {
        for role in ["admin", "billing", "owner", "primary_owner"] {
            let snap = SubscriptionSnapshot {
                organization_role: Some(role.to_string()),
                ..team_snapshot()
            };
            assert!(
                snap.has_claude_ai_billing_access(),
                "team org role {role} should grant billing access"
            );
        }

        let member = SubscriptionSnapshot {
            organization_role: Some("member".to_string()),
            ..team_snapshot()
        };
        assert!(!member.has_claude_ai_billing_access());

        for consumer in ["max", "pro"] {
            let snap = SubscriptionSnapshot {
                subscription_type: Some(consumer.to_string()),
                organization_role: None,
                ..team_snapshot()
            };
            assert!(
                snap.has_claude_ai_billing_access(),
                "{consumer} should always have billing access"
            );
        }

        let non_subscriber = SubscriptionSnapshot {
            is_subscriber: false,
            subscription_type: Some("max".to_string()),
            ..team_snapshot()
        };
        assert!(!non_subscriber.has_claude_ai_billing_access());
    }

    #[test]
    fn extra_usage_command_gate() {
        let team = team_snapshot();
        assert!(!team.is_extra_usage_command_enabled(true));

        let no_billing = SubscriptionSnapshot {
            billing_type: None,
            ..team_snapshot()
        };
        assert!(!no_billing.is_extra_usage_command_enabled(false));

        assert!(team.is_extra_usage_command_enabled(false));
    }
}
