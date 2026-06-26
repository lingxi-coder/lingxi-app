//! Rate-limit message composer — port of claude-code
//! `services/rateLimitMessages.ts` `getRateLimitMessage` (lines 45-104) plus
//! the upsell selection from `components/messages/RateLimitMessage.tsx`
//! `getUpsellMessage` (byte-locked strings landed in batch 3; the full
//! subscription-aware arm port landed in batch 4 — see the CLOSED list below).
//!
//! Pure function: the nine `OutputEvent::RateLimit` header-derived fields in,
//! `Option<ComposedRateLimit { text, upsell }>` out. Copy strings are
//! byte-identical to the TS source — note that `rateLimitMessages.ts` uses
//! STRAIGHT ASCII apostrophes throughout (`You've hit your…` literals at
//! lines 169/233/238/340/343 are `'`/U+0027, NOT U+2019); the curly
//! apostrophe/ellipsis literals live only in the `getUpsellMessage` strings,
//! already byte-locked in
//! [`crate::components::messages::rate_limit::upsell`].
//!
//! Reset-time formatting REUSES
//! [`orchestrator::model::rate_limit::format_reset_time`] — the existing 1:1
//! port of TS `utils/format.ts` `formatResetTime` (en-US, 12-hour, minute
//! omitted at `:00`, lowercased am/pm, `(<tz>)` suffix when requested).
//!
//! ## Subscription granularity (batch-4: documented gaps CLOSED)
//!
//! Batches ≤3 had no subscription plumbing, so the TS branches consulting
//! `getSubscriptionType()` / `getRateLimitTier()` / `getOauthAccountInfo()?.
//! hasExtraUsageEnabled` / `hasClaudeAiBillingAccess()` were scope-guarded to
//! unknown-subscription defaults. Batch 4 threads
//! [`SubscriptionSnapshot`] into the composer, closing all four:
//! - `getLimitReachedText` `seven_day_sonnet` naming
//!   (rateLimitMessages.ts:175-182) — `weekly limit` for pro/enterprise;
//! - the team/enterprise early-warning suppression
//!   (rateLimitMessages.ts:80-94);
//! - `getWarningUpsellText` (rateLimitMessages.ts:261-297) — see
//!   [`warning_upsell`];
//! - `getUpsellMessage` (RateLimitMessage.tsx:18-47) — see [`error_upsell`].
//!
//! The default snapshot (unknown subscription, every predicate `false`)
//! reproduces the historical behaviour EXCEPT the error upsell, which is now
//! `None` for non-subscribers (TSX :26 `if (!shouldShowUpsell) return null`,
//! with `shouldShowUpsell = isClaudeAISubscriber()` at :78).

use crate::components::messages::rate_limit::upsell;
use orchestrator::model::rate_limit::format_reset_time;
use traits::env::is_env_truthy;
use traits::subscription::SubscriptionSnapshot;

/// The nine header-derived fields of `traits::OutputEvent::RateLimit`
/// (each `None` when the provider did not send the corresponding
/// `anthropic-ratelimit-unified-*` header). Mirrors the TS `ClaudeAILimits`
/// shape minus the derived `isUsingOverage`, which [`compose_rate_limit`]
/// re-derives TS-faithfully (claudeAiLimits.ts:406-409).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RateLimitInfo {
    /// `anthropic-ratelimit-unified-status`.
    pub status: Option<String>,
    /// `anthropic-ratelimit-unified-representative-claim`.
    pub rate_limit_type: Option<String>,
    /// Representative claim's 0-1 utilization fraction.
    pub utilization: Option<f64>,
    /// `anthropic-ratelimit-unified-reset` (Unix-epoch seconds).
    pub resets_at: Option<u64>,
    /// Per-claim reset (Unix-epoch seconds). Unused by the composer (the TS
    /// `getRateLimitMessage` never reads it) — carried for completeness.
    pub claim_resets_at: Option<u64>,
    /// `anthropic-ratelimit-unified-overage-status`.
    pub overage_status: Option<String>,
    /// `anthropic-ratelimit-unified-overage-reset` (Unix-epoch seconds).
    pub overage_resets_at: Option<u64>,
    /// `anthropic-ratelimit-unified-overage-disabled-reason`.
    pub overage_disabled_reason: Option<String>,
    /// `anthropic-ratelimit-unified-fallback` == `available`. Unused by the
    /// composer (TS parity) — carried for completeness.
    pub fallback_available: Option<bool>,
}

/// A composed rate-limit notice: error/warning text plus the optional dim
/// upsell line rendered under it by
/// [`crate::components::messages::rate_limit::RateLimitMessage`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposedRateLimit {
    /// The notice text (rendered error-colored).
    pub text: String,
    /// Optional dim upsell line. `Some` only for error-severity notices —
    /// in claude-code the `RateLimitMessage` component (which owns the dim
    /// upsell line) renders transcript ERRORS, while warnings go to the UI
    /// footer without it (rateLimitMessages.ts:128-141).
    pub upsell: Option<String>,
}

/// TS `WARNING_THRESHOLD` (rateLimitMessages.ts:72).
const WARNING_THRESHOLD: f64 = 0.7;

/// Port of `getRateLimitMessage` (rateLimitMessages.ts:45-104), branch order
/// preserved. Returns `None` when no message should be shown.
#[must_use]
pub fn compose_rate_limit(info: &RateLimitInfo, sub: &SubscriptionSnapshot) -> Option<ComposedRateLimit> {
    // TS `formatLimitReachedText` branches on `process.env.USER_TYPE ===
    // 'ant'` (rateLimitMessages.ts:339); `extraUsage.isEnabled()`
    // (commands/extra-usage/index.ts:6-17) reads
    // `DISABLE_EXTRA_USAGE_COMMAND` through `isEnvTruthy`. Both env reads
    // happen here so the core stays injectable for tests.
    let disable_extra_usage = is_env_truthy(
        std::env::var("DISABLE_EXTRA_USAGE_COMMAND")
            .ok()
            .as_deref(),
    );
    compose_with(
        info,
        std::env::var("USER_TYPE").as_deref() == Ok("ant"),
        sub,
        sub.is_extra_usage_command_enabled(disable_extra_usage),
    )
}

/// Testable core of [`compose_rate_limit`] with the `USER_TYPE === 'ant'`
/// flag and `extraUsage.isEnabled()` injected.
fn compose_with(
    info: &RateLimitInfo,
    is_ant: bool,
    sub: &SubscriptionSnapshot,
    extra_usage_cmd_enabled: bool,
) -> Option<ComposedRateLimit> {
    let status = info.status.as_deref();
    let overage_status = info.overage_status.as_deref();

    // "Check overage scenarios first (when subscription is rejected but
    // overage is available)" — rateLimitMessages.ts:49-60.
    if is_using_overage(info) {
        if overage_status == Some("allowed_warning") {
            return Some(ComposedRateLimit {
                text: "You're close to your extra usage spending limit".to_owned(),
                upsell: None,
            });
        }
        return None;
    }

    // "ERROR STATES - when limits are rejected" — rateLimitMessages.ts:62-65.
    if status == Some("rejected") {
        return Some(ComposedRateLimit {
            text: limit_reached_text(info, is_ant, sub),
            upsell: error_upsell(sub, extra_usage_cmd_enabled),
        });
    }

    // "WARNING STATES - when approaching limits" — rateLimitMessages.ts:67-100.
    if status == Some("allowed_warning") {
        // "Only show warnings when utilization is above threshold (70%)" —
        // TS guards `utilization !== undefined && utilization <
        // WARNING_THRESHOLD` (:73-78), so an ABSENT utilization falls through.
        if let Some(u) = info.utilization {
            if u < WARNING_THRESHOLD {
                return None;
            }
        }

        // TS :80-94: don't warn non-billing Team/Enterprise users with extra
        // usage enabled — they roll into overage seamlessly.
        if sub.is_team_or_enterprise()
            && sub.has_extra_usage_enabled
            && !sub.has_claude_ai_billing_access()
        {
            return None;
        }

        if let Some(text) = early_warning_text(info, sub) {
            return Some(ComposedRateLimit { text, upsell: None });
        }
    }

    None
}

/// `isUsingOverage` derivation, ported verbatim from claudeAiLimits.ts:406-409:
///
/// ```text
/// const isUsingOverage =
///   status === 'rejected' &&
///   (overageStatus === 'allowed' || overageStatus === 'allowed_warning')
/// ```
///
/// `pub` so the streaming layer's overage-transition notice
/// (`useRateLimitWarningNotification.tsx`) shares the one derivation instead
/// of re-deriving it.
#[must_use]
pub fn is_using_overage(info: &RateLimitInfo) -> bool {
    info.status.as_deref() == Some("rejected")
        && matches!(
            info.overage_status.as_deref(),
            Some("allowed" | "allowed_warning")
        )
}

/// TS `formatResetTime(ts, true)` — showTimezone, showTime defaulted true.
fn fmt_reset(ts: Option<u64>) -> Option<String> {
    format_reset_time(ts.and_then(|t| i64::try_from(t).ok()), true, true)
}

/// Port of `getUsingOverageText` (rateLimitMessages.ts:303-331) — the
/// transient notice shown once when the session rolls into extra usage
/// (`useRateLimitWarningNotification.tsx` fires it on the overage
/// transition).
#[must_use]
pub fn using_overage_text(info: &RateLimitInfo, sub: &SubscriptionSnapshot) -> String {
    // TS :304-306: `resetTime = limits.resetsAt ? formatResetTime(resetsAt,
    // true) : ''` — the falsy-0 guard lives inside `format_reset_time`.
    let reset_time = fmt_reset(info.resets_at);
    // TS :308-321 limitName chain.
    let limit_name = match info.rate_limit_type.as_deref() {
        Some("five_hour") => "session limit",
        Some("seven_day") => "weekly limit",
        Some("seven_day_opus") => "Opus limit",
        Some("seven_day_sonnet") => {
            // "For pro and enterprise, Sonnet limit is the same as weekly"
            // — TS :316-320.
            if sub.is_pro_or_enterprise() {
                "weekly limit"
            } else {
                "Sonnet limit"
            }
        }
        _ => "",
    };
    // TS :323-325: no limitName → the bare copy, BEFORE any reset suffix.
    if limit_name.is_empty() {
        return "Now using extra usage".to_owned();
    }
    // TS :327-330: `resetMessage = resetTime ? ` · Your ${limitName} resets
    // ${resetTime}` : ''` — straight ASCII apostrophe, U+00B7 separator.
    match reset_time {
        Some(t) => format!("You're now using extra usage \u{b7} Your {limit_name} resets {t}"),
        None => "You're now using extra usage".to_owned(),
    }
}

/// Port of `getLimitReachedText` (rateLimitMessages.ts:143-197).
fn limit_reached_text(info: &RateLimitInfo, is_ant: bool, sub: &SubscriptionSnapshot) -> String {
    let reset_time = fmt_reset(info.resets_at);
    let overage_reset_time = fmt_reset(info.overage_resets_at);
    let reset_message = reset_time
        .as_deref()
        .map_or_else(String::new, |t| format!(" \u{b7} resets {t}"));

    // "if BOTH subscription (checked before this method) and overage are
    // exhausted" — TS :152-173.
    if info.overage_status.as_deref() == Some("rejected") {
        // "Show the earliest reset time" — TS :154-166. The first branch
        // gates on the RAW timestamps (`resetsAt && limits.overageResetsAt`,
        // 0 falsy), the fallbacks on the formatted strings.
        let earliest = match (
            info.resets_at.filter(|t| *t != 0),
            info.overage_resets_at.filter(|t| *t != 0),
        ) {
            (Some(r), Some(o)) => {
                if r < o {
                    reset_time
                } else {
                    overage_reset_time
                }
            }
            _ => reset_time.or(overage_reset_time),
        };
        let overage_reset_message = earliest
            .as_deref()
            .map_or_else(String::new, |t| format!(" \u{b7} resets {t}"));

        if info.overage_disabled_reason.as_deref() == Some("out_of_credits") {
            return format!("You're out of extra usage{overage_reset_message}");
        }

        return format_limit_reached_text("limit", &overage_reset_message, is_ant);
    }

    let limit = match info.rate_limit_type.as_deref() {
        // "For pro and enterprise, Sonnet limit is the same as weekly" —
        // TS :175-182.
        Some("seven_day_sonnet") => {
            if sub.is_pro_or_enterprise() {
                "weekly limit"
            } else {
                "Sonnet limit"
            }
        }
        Some("seven_day_opus") => "Opus limit",
        Some("seven_day") => "weekly limit",
        Some("five_hour") => "session limit",
        _ => "usage limit",
    };
    format_limit_reached_text(limit, &reset_message, is_ant)
}

/// Port of `getEarlyWarningText` (rateLimitMessages.ts:199-254).
fn early_warning_text(info: &RateLimitInfo, sub: &SubscriptionSnapshot) -> Option<String> {
    let limit_name = match info.rate_limit_type.as_deref() {
        Some("seven_day") => "weekly limit",
        Some("five_hour") => "session limit",
        Some("seven_day_opus") => "Opus limit",
        Some("seven_day_sonnet") => "Sonnet limit",
        Some("overage") => "extra usage",
        // TS `case undefined: return null` (:217-218); unknown strings can't
        // occur in the typed union — treated alike.
        _ => return None,
    };

    // TS :222-224: `used = limits.utilization ? Math.floor(u * 100) :
    // undefined`, then truthiness-gated (`if (used && ...)`) — so both a
    // falsy utilization (0) and a floored 0% behave as absent.
    #[allow(
        clippy::cast_possible_truncation,
        reason = "TS Math.floor of a 0-1 utilization fraction times 100; the floored value fits trivially"
    )]
    let used = info
        .utilization
        .filter(|u| *u != 0.0)
        .map(|u| (u * 100.0).floor() as i64)
        .filter(|n| *n != 0);
    let reset_time = fmt_reset(info.resets_at);

    // "Get upsell command based on subscription type and limit type" —
    // TS :229-230.
    let upsell = warning_upsell(info.rate_limit_type.as_deref(), sub);
    let with_upsell = |base: String| match upsell {
        Some(u) => format!("{base} \u{b7} {u}"),
        None => base,
    };

    if let Some(used) = used {
        let base = if let Some(reset_time) = &reset_time {
            format!("You've used {used}% of your {limit_name} \u{b7} resets {reset_time}")
        } else {
            format!("You've used {used}% of your {limit_name}")
        };
        return Some(with_upsell(base));
    }

    // "For the 'Approaching <x>' verbiage, 'extra usage limit' makes more
    // sense than 'extra usage'" — TS :242-245.
    let limit_name = if info.rate_limit_type.as_deref() == Some("overage") {
        format!("{limit_name} limit")
    } else {
        limit_name.to_owned()
    };

    let base = if let Some(reset_time) = reset_time {
        format!("Approaching {limit_name} \u{b7} resets {reset_time}")
    } else {
        format!("Approaching {limit_name}")
    };
    Some(with_upsell(base))
}

/// Port of `formatLimitReachedText` (rateLimitMessages.ts:333-344). The TS
/// `model` parameter is unused there (`_model`) and omitted here.
fn format_limit_reached_text(limit: &str, reset_message: &str, is_ant: bool) -> String {
    if is_ant {
        // FEEDBACK_CHANNEL_ANT = '#briarpatch-cc' (rateLimitMessages.ts:15).
        return format!(
            "You've hit your {limit}{reset_message}. If you have feedback about \
             this limit, post in #briarpatch-cc. You can reset your limits with \
             /reset-limits"
        );
    }
    format!("You've hit your {limit}{reset_message}")
}

/// Warning-upsell copy (rateLimitMessages.ts:274, :282 — straight ASCII).
const EXTRA_USAGE_REQUEST: &str = "/extra-usage to request more";
const UPGRADE_KEEP_USING: &str = "/upgrade to keep using LingXi";

/// Port of `getWarningUpsellText` (rateLimitMessages.ts:261-297).
fn warning_upsell(
    rate_limit_type: Option<&str>,
    sub: &SubscriptionSnapshot,
) -> Option<&'static str> {
    match rate_limit_type {
        // "5-hour session limit warning" — TS :268-284.
        Some("five_hour") => {
            if sub.is_team_or_enterprise() {
                // "Teams/Enterprise with overages disabled: prompt to request
                // extra usage. Only show if overage provisioning is allowed
                // for this org type (e.g., not AWS marketplace)" — TS :270-275.
                if !sub.has_extra_usage_enabled && sub.is_overage_provisioning_allowed() {
                    return Some(EXTRA_USAGE_REQUEST);
                }
                // "Teams/Enterprise with overages enabled or unsupported
                // billing type don't need upsell" — TS :276-277.
                return None;
            }
            // "Pro/Max users: prompt to upgrade" — TS :280-283.
            if matches!(sub.subscription_type.as_deref(), Some("pro" | "max")) {
                return Some(UPGRADE_KEEP_USING);
            }
            None
        }
        // "Overage warning (approaching spending limit)" — TS :286-293.
        Some("overage") => {
            if sub.is_team_or_enterprise()
                && !sub.has_extra_usage_enabled
                && sub.is_overage_provisioning_allowed()
            {
                return Some(EXTRA_USAGE_REQUEST);
            }
            None
        }
        // "Weekly limit warnings don't show upsell per spec" — TS :295-296.
        _ => None,
    }
}

/// Port of `getUpsellMessage` (RateLimitMessage.tsx:18-47).
///
/// `shouldAutoOpenRateLimitOptionsMenu` is structurally false: the TUI has no
/// interactive rate-limit options menu, so the `OPENING_OPTIONS` arm (TSX
/// :33-35) is unreachable (the constant stays byte-locked for when the menu
/// lands). `shouldShowUpsell` is `sub.is_subscriber` — the TS
/// `shouldProcessMockLimits()` arm (TSX :78) is the unported `/mock-limits`
/// test command.
fn error_upsell(sub: &SubscriptionSnapshot, extra_usage_cmd_enabled: bool) -> Option<String> {
    // TSX :26.
    if !sub.is_subscriber {
        return None;
    }
    // TSX :27-32.
    if sub.is_max20x() {
        return Some(
            if extra_usage_cmd_enabled {
                upsell::EXTRA_USAGE_FINISH
            } else {
                upsell::LOGIN_SWITCH
            }
            .to_owned(),
        );
    }
    // TSX :36-38.
    if !sub.is_team_or_enterprise() && !extra_usage_cmd_enabled {
        return Some(upsell::UPGRADE.to_owned());
    }
    // TSX :39-45.
    if sub.is_team_or_enterprise() {
        if !extra_usage_cmd_enabled {
            return None;
        }
        return Some(
            if sub.has_claude_ai_billing_access() {
                upsell::EXTRA_USAGE_FINISH
            } else {
                upsell::EXTRA_USAGE_ADMIN
            }
            .to_owned(),
        );
    }
    // TSX :46.
    Some(upsell::UPGRADE_OR_EXTRA.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Legacy shorthand: unknown subscription (default snapshot), extra-usage
    /// command disabled — the pre-batch-4 composer behaviour.
    fn compose(info: &RateLimitInfo, is_ant: bool) -> Option<ComposedRateLimit> {
        compose_with(info, is_ant, &SubscriptionSnapshot::default(), false)
    }

    /// Pro subscriber on Stripe billing.
    fn pro() -> SubscriptionSnapshot {
        SubscriptionSnapshot {
            is_subscriber: true,
            subscription_type: Some("pro".into()),
            billing_type: Some("stripe_subscription".into()),
            ..SubscriptionSnapshot::default()
        }
    }

    /// Team subscriber on Stripe billing; extra usage + org role injectable.
    fn team(has_extra_usage: bool, role: Option<&str>) -> SubscriptionSnapshot {
        SubscriptionSnapshot {
            is_subscriber: true,
            subscription_type: Some("team".into()),
            billing_type: Some("stripe_subscription".into()),
            has_extra_usage_enabled: has_extra_usage,
            organization_role: role.map(str::to_owned),
            ..SubscriptionSnapshot::default()
        }
    }

    /// Max subscriber on the 20x rate-limit tier.
    fn max20x() -> SubscriptionSnapshot {
        SubscriptionSnapshot {
            is_subscriber: true,
            subscription_type: Some("max".into()),
            rate_limit_tier: Some("default_claude_max_20x".into()),
            billing_type: Some("stripe_subscription".into()),
            ..SubscriptionSnapshot::default()
        }
    }

    /// Unix-epoch seconds `delta` seconds from now (kept well within 24h so
    /// `format_reset_time` stays on the time-only branch).
    fn ts_in(delta: i64) -> u64 {
        u64::try_from(chrono::Utc::now().timestamp() + delta).unwrap()
    }

    /// Expected `formatResetTime(ts, true)` output (showTimezone = true,
    /// showTime defaulted true) — computed through the SAME reused
    /// orchestrator port so the assertions are deterministic.
    fn reset(ts: u64) -> String {
        format_reset_time(Some(i64::try_from(ts).unwrap()), true, true).unwrap()
    }

    fn rejected(rate_limit_type: Option<&str>, resets_at: Option<u64>) -> RateLimitInfo {
        RateLimitInfo {
            status: Some("rejected".into()),
            rate_limit_type: rate_limit_type.map(str::to_owned),
            resets_at,
            ..RateLimitInfo::default()
        }
    }

    // ── missing fields ───────────────────────────────────────────────────

    #[test]
    fn all_fields_absent_composes_nothing() {
        assert_eq!(compose(&RateLimitInfo::default(), false), None);
    }

    #[test]
    fn status_allowed_composes_nothing() {
        let info = RateLimitInfo {
            status: Some("allowed".into()),
            ..RateLimitInfo::default()
        };
        assert_eq!(compose(&info, false), None);
    }

    // ── isUsingOverage derivation (claudeAiLimits.ts:406-409) ──────────────

    #[test]
    fn rejected_with_overage_allowed_is_using_overage_no_message() {
        // status rejected + overageStatus allowed → isUsingOverage → the
        // overage branch returns null unless allowed_warning (TS :51-60).
        let info = RateLimitInfo {
            status: Some("rejected".into()),
            overage_status: Some("allowed".into()),
            ..RateLimitInfo::default()
        };
        assert_eq!(compose(&info, false), None);
    }

    #[test]
    fn rejected_with_overage_allowed_warning_warns_spending_limit() {
        // isUsingOverage + overageStatus allowed_warning → spending-limit
        // warning (TS :53-58). Straight apostrophe — byte parity.
        let info = RateLimitInfo {
            status: Some("rejected".into()),
            overage_status: Some("allowed_warning".into()),
            ..RateLimitInfo::default()
        };
        let got = compose(&info, false).unwrap();
        assert_eq!(got.text, "You're close to your extra usage spending limit");
        assert!(!got.text.contains('\u{2019}'));
        assert_eq!(got.upsell, None, "warnings carry no dim upsell line");
    }

    // ── rejected (error) branches (rateLimitMessages.ts:63-65, 143-197) ───

    #[test]
    fn rejected_five_hour_hits_session_limit_with_reset() {
        let ts = ts_in(3600);
        let got = compose(&rejected(Some("five_hour"), Some(ts)), false).unwrap();
        assert_eq!(
            got.text,
            format!("You've hit your session limit \u{b7} resets {}", reset(ts))
        );
        assert!(!got.text.contains('\u{2019}'), "TS uses straight apostrophes");
    }

    #[test]
    fn rejected_seven_day_hits_weekly_limit() {
        let ts = ts_in(3600);
        let got = compose(&rejected(Some("seven_day"), Some(ts)), false).unwrap();
        assert_eq!(
            got.text,
            format!("You've hit your weekly limit \u{b7} resets {}", reset(ts))
        );
    }

    #[test]
    fn rejected_seven_day_opus_hits_opus_limit() {
        let got = compose(&rejected(Some("seven_day_opus"), None), false).unwrap();
        assert_eq!(got.text, "You've hit your Opus limit");
    }

    #[test]
    fn rejected_seven_day_sonnet_hits_sonnet_limit() {
        // Unknown subscription → the non-pro/enterprise default 'Sonnet
        // limit' (TS :175-182; gap documented in the module docs).
        let got = compose(&rejected(Some("seven_day_sonnet"), None), false).unwrap();
        assert_eq!(got.text, "You've hit your Sonnet limit");
    }

    #[test]
    fn rejected_unknown_type_hits_usage_limit() {
        let got = compose(&rejected(None, None), false).unwrap();
        assert_eq!(got.text, "You've hit your usage limit");
    }

    #[test]
    fn rejected_ant_user_appends_feedback_and_reset_hint() {
        // TS formatLimitReachedText USER_TYPE === 'ant' branch (:339-341).
        let got = compose(&rejected(Some("five_hour"), None), true).unwrap();
        assert_eq!(
            got.text,
            "You've hit your session limit. If you have feedback about this limit, \
             post in #briarpatch-cc. You can reset your limits with /reset-limits"
        );
    }

    // ── rejected + overage rejected (TS :152-173) ─────────────────────────

    #[test]
    fn both_rejected_out_of_credits_says_out_of_extra_usage() {
        let ts = ts_in(1800);
        let info = RateLimitInfo {
            overage_status: Some("rejected".into()),
            overage_disabled_reason: Some("out_of_credits".into()),
            overage_resets_at: Some(ts),
            ..rejected(Some("five_hour"), None)
        };
        let got = compose(&info, false).unwrap();
        assert_eq!(
            got.text,
            format!("You're out of extra usage \u{b7} resets {}", reset(ts))
        );
    }

    #[test]
    fn both_rejected_without_reason_hits_plain_limit() {
        // overageStatus rejected, no disabled reason → the bare word
        // 'limit' (TS :172).
        let info = RateLimitInfo {
            overage_status: Some("rejected".into()),
            ..rejected(Some("five_hour"), None)
        };
        let got = compose(&info, false).unwrap();
        assert_eq!(got.text, "You've hit your limit");
    }

    #[test]
    fn both_rejected_picks_earlier_of_two_resets() {
        let early = ts_in(600);
        let late = ts_in(7200);
        // subscription resets first
        let info = RateLimitInfo {
            overage_status: Some("rejected".into()),
            overage_resets_at: Some(late),
            ..rejected(Some("five_hour"), Some(early))
        };
        let got = compose(&info, false).unwrap();
        assert_eq!(
            got.text,
            format!("You've hit your limit \u{b7} resets {}", reset(early))
        );
        // overage resets first
        let info = RateLimitInfo {
            overage_status: Some("rejected".into()),
            overage_resets_at: Some(early),
            ..rejected(Some("five_hour"), Some(late))
        };
        let got = compose(&info, false).unwrap();
        assert_eq!(
            got.text,
            format!("You've hit your limit \u{b7} resets {}", reset(early))
        );
    }

    // ── error upsell (RateLimitMessage.tsx getUpsellMessage :18-47) ───────
    //
    // Batch ≤3 proxied the upsell off the overage-status header and ALWAYS
    // returned one of the two generic arms; that was a documented
    // scope-guard. The real TSX gates everything on `shouldShowUpsell =
    // isClaudeAISubscriber()` (:26 + :78), so the unknown-subscription
    // default is now NO upsell.

    #[test]
    fn rejected_unknown_subscription_has_no_upsell() {
        // TSX :26 `if (!shouldShowUpsell) return null` with
        // `shouldShowUpsell = isClaudeAISubscriber()` (:78) — the default
        // snapshot is not a subscriber. (Pre-batch-4 this asserted the
        // generic `upsell::UPGRADE` proxy.)
        let got = compose(&rejected(Some("five_hour"), None), false).unwrap();
        assert_eq!(got.upsell, None);
    }

    #[test]
    fn rejected_unknown_subscription_with_overage_header_has_no_upsell() {
        // Pre-batch-4 the overage-status header proxied
        // `upsell::UPGRADE_OR_EXTRA`; the real gate is the subscriber check
        // (TSX :26 + :78); the header no longer drives the upsell.
        let info = RateLimitInfo {
            overage_status: Some("rejected".into()),
            ..rejected(Some("five_hour"), None)
        };
        let got = compose(&info, false).unwrap();
        assert_eq!(got.upsell, None);
    }

    #[test]
    fn unknown_subscription_keeps_rev28_copy() {
        // Default snapshot reproduces the rev2.8 (batch-3) composition:
        // non-pro 'Sonnet limit' naming and no error upsell.
        let got = compose(&rejected(Some("seven_day_sonnet"), None), false).unwrap();
        assert_eq!(got.text, "You've hit your Sonnet limit");
        assert_eq!(got.upsell, None);
    }

    #[test]
    fn pro_subscriber_sonnet_limit_reads_weekly_limit() {
        // "For pro and enterprise, Sonnet limit is the same as weekly" —
        // rateLimitMessages.ts:175-182.
        let got = compose_with(&rejected(Some("seven_day_sonnet"), None), false, &pro(), false)
            .unwrap();
        assert_eq!(got.text, "You've hit your weekly limit");
        // End-to-end wiring pin: a subscriber's rejected notice carries the
        // error upsell out of compose_with (TSX :36-38 — pro without the
        // extra-usage command → UPGRADE), not just from error_upsell directly.
        assert_eq!(got.upsell.as_deref(), Some(upsell::UPGRADE));

        let enterprise = SubscriptionSnapshot {
            subscription_type: Some("enterprise".into()),
            ..pro()
        };
        let got = compose_with(
            &rejected(Some("seven_day_sonnet"), None),
            false,
            &enterprise,
            false,
        )
        .unwrap();
        assert_eq!(got.text, "You've hit your weekly limit");
    }

    #[test]
    fn max20x_error_upsell_login_switch_without_extra_usage_cmd() {
        // TSX :27-31: Max-20x without the extra-usage command → /login arm.
        assert_eq!(
            error_upsell(&max20x(), false).as_deref(),
            Some(upsell::LOGIN_SWITCH)
        );
    }

    #[test]
    fn max20x_error_upsell_extra_usage_finish_with_cmd() {
        // TSX :28-30: Max-20x with the extra-usage command enabled.
        assert_eq!(
            error_upsell(&max20x(), true).as_deref(),
            Some(upsell::EXTRA_USAGE_FINISH)
        );
    }

    #[test]
    fn pro_error_upsell_upgrade_without_cmd() {
        // TSX :36-38: !isTeamOrEnterprise && !isExtraUsageCommandEnabled.
        assert_eq!(error_upsell(&pro(), false).as_deref(), Some(upsell::UPGRADE));
    }

    #[test]
    fn pro_with_cmd_enabled_falls_through_to_upgrade_or_extra() {
        // TSX :36 requires BOTH !team && !cmd — a pro user WITH the command
        // enabled skips the UPGRADE arm and the team block (:39) and lands
        // on the final fallthrough (:46). This pins the subtle TS ordering.
        assert_eq!(
            error_upsell(&pro(), true).as_deref(),
            Some(upsell::UPGRADE_OR_EXTRA)
        );
    }

    #[test]
    fn team_error_upsell_admin_vs_member() {
        // TSX :39-45: team/enterprise — null without the command; with it,
        // billing access picks FINISH, member picks ADMIN.
        assert_eq!(error_upsell(&team(false, Some("admin")), false), None);
        assert_eq!(
            error_upsell(&team(false, Some("admin")), true).as_deref(),
            Some(upsell::EXTRA_USAGE_FINISH)
        );
        assert_eq!(
            error_upsell(&team(false, Some("member")), true).as_deref(),
            Some(upsell::EXTRA_USAGE_ADMIN)
        );
        // Enterprise rides the same is_team_or_enterprise() predicate
        // (TSX :74); pin one variant so the arm isn't team-only-tested.
        let enterprise = SubscriptionSnapshot {
            subscription_type: Some("enterprise".into()),
            ..team(false, Some("member"))
        };
        assert_eq!(
            error_upsell(&enterprise, true).as_deref(),
            Some(upsell::EXTRA_USAGE_ADMIN)
        );
    }

    // ── allowed_warning branches (TS :68-100, 199-254) ───────────────────

    fn warning(
        rate_limit_type: Option<&str>,
        utilization: Option<f64>,
        resets_at: Option<u64>,
    ) -> RateLimitInfo {
        RateLimitInfo {
            status: Some("allowed_warning".into()),
            rate_limit_type: rate_limit_type.map(str::to_owned),
            utilization,
            resets_at,
            ..RateLimitInfo::default()
        }
    }

    #[test]
    fn warning_below_threshold_composes_nothing() {
        let info = warning(Some("seven_day"), Some(0.69), Some(ts_in(3600)));
        assert_eq!(compose(&info, false), None);
    }

    #[test]
    fn warning_at_threshold_composes() {
        // TS gate is `utilization < WARNING_THRESHOLD` → exactly 0.7 warns.
        let ts = ts_in(3600);
        let got = compose(&warning(Some("seven_day"), Some(0.7), Some(ts)), false).unwrap();
        assert_eq!(
            got.text,
            format!("You've used 70% of your weekly limit \u{b7} resets {}", reset(ts))
        );
        assert_eq!(got.upsell, None);
    }

    #[test]
    fn warning_used_percentage_floors() {
        // Math.floor(0.857 * 100) = 85 (TS :223).
        let got = compose(&warning(Some("seven_day"), Some(0.857), None), false).unwrap();
        assert_eq!(got.text, "You've used 85% of your weekly limit");
    }

    #[test]
    fn warning_without_utilization_says_approaching() {
        // TS only short-circuits when utilization !== undefined (:73-78);
        // absent utilization falls through to the 'Approaching' copy (:247-253).
        let ts = ts_in(3600);
        let got = compose(&warning(Some("five_hour"), None, Some(ts)), false).unwrap();
        assert_eq!(
            got.text,
            format!("Approaching session limit \u{b7} resets {}", reset(ts))
        );
    }

    #[test]
    fn warning_overage_type_appends_limit_to_approaching() {
        // TS :242-245 — 'extra usage' becomes 'extra usage limit' on the
        // Approaching path.
        let got = compose(&warning(Some("overage"), None, None), false).unwrap();
        assert_eq!(got.text, "Approaching extra usage limit");
    }

    #[test]
    fn warning_overage_type_with_utilization_keeps_extra_usage() {
        let got = compose(&warning(Some("overage"), Some(0.9), None), false).unwrap();
        assert_eq!(got.text, "You've used 90% of your extra usage");
    }

    #[test]
    fn warning_zero_utilization_composes_nothing() {
        // `0 !== undefined` and `0 < 0.7` → the TS threshold guard returns
        // null (TS :73-78).
        let info = warning(Some("seven_day"), Some(0.0), Some(ts_in(3600)));
        assert_eq!(compose(&info, false), None);
    }

    #[test]
    fn warning_unknown_type_composes_nothing() {
        // TS getEarlyWarningText returns null for `undefined` (:217-218);
        // unknown strings can't occur in the typed union — treated alike.
        assert_eq!(compose(&warning(None, Some(0.9), None), false), None);
    }

    // ── team/enterprise warning suppression (TS :80-94) ───────────────────

    #[test]
    fn team_with_extra_usage_and_no_billing_access_suppresses_warning() {
        // "Don't warn non-billing Team/Enterprise users about approaching
        // plan limits if overages are enabled" — rateLimitMessages.ts:80-94.
        // Role None → no billing access.
        let info = warning(Some("five_hour"), Some(0.8), Some(ts_in(3600)));
        assert_eq!(compose_with(&info, false, &team(true, None), false), None);
    }

    #[test]
    fn team_admin_still_sees_warning() {
        // Billing access (admin role) defeats the suppression (TS :91).
        let ts = ts_in(3600);
        let info = warning(Some("five_hour"), Some(0.8), Some(ts));
        let got = compose_with(&info, false, &team(true, Some("admin")), false).unwrap();
        assert_eq!(
            got.text,
            format!(
                "You've used 80% of your session limit \u{b7} resets {}",
                reset(ts)
            )
        );
    }

    // ── warning upsell (getWarningUpsellText, TS :261-297) ────────────────

    #[test]
    fn five_hour_warning_pro_appends_upgrade_upsell() {
        // TS :281-283: pro/max → '/upgrade to keep using Claude Code',
        // appended ` · {upsell}` to the base copy (:232-234).
        let ts = ts_in(3600);
        let info = warning(Some("five_hour"), Some(0.8), Some(ts));
        let got = compose_with(&info, false, &pro(), false).unwrap();
        assert_eq!(
            got.text,
            format!(
                "You've used 80% of your session limit \u{b7} resets {} \u{b7} /upgrade to keep using LingXi",
                reset(ts)
            )
        );
        assert!(got.text.ends_with(" \u{b7} /upgrade to keep using LingXi"));
        // TS :281 matches subscription_type directly ('pro' || 'max') — pin
        // the max arm too (max here is NOT the 20x tier; tier is separate).
        let max = SubscriptionSnapshot {
            subscription_type: Some("max".into()),
            ..pro()
        };
        let got = compose_with(&info, false, &max, false).unwrap();
        assert!(got.text.ends_with(" \u{b7} /upgrade to keep using LingXi"));
    }

    #[test]
    fn five_hour_warning_team_without_extra_usage_appends_request_upsell() {
        // TS :272-275: team/enterprise without extra usage, overage
        // provisioning allowed (Stripe) → '/extra-usage to request more'.
        let got = compose_with(
            &warning(Some("five_hour"), Some(0.8), None),
            false,
            &team(false, Some("admin")),
            false,
        )
        .unwrap();
        assert_eq!(
            got.text,
            "You've used 80% of your session limit \u{b7} /extra-usage to request more"
        );
    }

    #[test]
    fn five_hour_warning_team_with_extra_usage_has_no_upsell() {
        // "Teams/Enterprise with overages enabled ... don't need upsell" —
        // TS :276-277. Admin role so the :80-94 suppression doesn't apply.
        let got = compose_with(
            &warning(Some("five_hour"), Some(0.8), None),
            false,
            &team(true, Some("admin")),
            false,
        )
        .unwrap();
        assert_eq!(got.text, "You've used 80% of your session limit");
    }

    #[test]
    fn weekly_warning_never_has_upsell() {
        // "Weekly limit warnings don't show upsell per spec" — TS :295-296.
        let got = compose_with(
            &warning(Some("seven_day"), Some(0.8), None),
            false,
            &pro(),
            false,
        )
        .unwrap();
        assert_eq!(got.text, "You've used 80% of your weekly limit");
    }

    #[test]
    fn overage_warning_team_without_extra_usage_appends_request_upsell() {
        // TS :287-293 (overage arm) + the `limitName += ' limit'`
        // Approaching adjustment (:242-245) happening BEFORE the upsell
        // append (:247-249).
        let got = compose_with(
            &warning(Some("overage"), None, None),
            false,
            &team(false, Some("admin")),
            false,
        )
        .unwrap();
        assert_eq!(
            got.text,
            "Approaching extra usage limit \u{b7} /extra-usage to request more"
        );
    }

    // ── getUsingOverageText (rateLimitMessages.ts:303-331) ────────────────
    //
    // `using_overage_text` reads only `rate_limit_type`/`resets_at` plus the
    // subscription, so the `rejected(...)` constructor doubles as its input.

    #[test]
    fn using_overage_text_per_limit_type() {
        let unknown = SubscriptionSnapshot::default();
        let ts = ts_in(3600);
        // five_hour → 'session limit' (TS :309-310); separator placement is
        // ` · Your {limitName} resets {resetTime}` (TS :328), U+00B7.
        assert_eq!(
            using_overage_text(&rejected(Some("five_hour"), Some(ts)), &unknown),
            format!(
                "You're now using extra usage \u{b7} Your session limit resets {}",
                reset(ts)
            )
        );
        // seven_day → 'weekly limit' (TS :311-312).
        assert_eq!(
            using_overage_text(&rejected(Some("seven_day"), Some(ts)), &unknown),
            format!(
                "You're now using extra usage \u{b7} Your weekly limit resets {}",
                reset(ts)
            )
        );
        // seven_day_opus → 'Opus limit' (TS :313-314).
        assert_eq!(
            using_overage_text(&rejected(Some("seven_day_opus"), Some(ts)), &unknown),
            format!(
                "You're now using extra usage \u{b7} Your Opus limit resets {}",
                reset(ts)
            )
        );
        // seven_day_sonnet: "For pro and enterprise, Sonnet limit is the same
        // as weekly" (TS :315-320); everyone else keeps 'Sonnet limit'.
        assert_eq!(
            using_overage_text(&rejected(Some("seven_day_sonnet"), Some(ts)), &pro()),
            format!(
                "You're now using extra usage \u{b7} Your weekly limit resets {}",
                reset(ts)
            )
        );
        let enterprise = SubscriptionSnapshot {
            subscription_type: Some("enterprise".into()),
            ..pro()
        };
        assert_eq!(
            using_overage_text(&rejected(Some("seven_day_sonnet"), Some(ts)), &enterprise),
            format!(
                "You're now using extra usage \u{b7} Your weekly limit resets {}",
                reset(ts)
            )
        );
        assert_eq!(
            using_overage_text(&rejected(Some("seven_day_sonnet"), Some(ts)), &unknown),
            format!(
                "You're now using extra usage \u{b7} Your Sonnet limit resets {}",
                reset(ts)
            )
        );
        // No limit type → the bare copy, EVEN with resetsAt set: TS :323
        // checks `!limitName` before the reset message is ever built.
        assert_eq!(
            using_overage_text(&rejected(None, Some(ts)), &unknown),
            "Now using extra usage"
        );
        // five_hour without a reset → no ` · Your …` suffix (TS :327-329:
        // empty resetTime ⇒ empty resetMessage).
        assert_eq!(
            using_overage_text(&rejected(Some("five_hour"), None), &unknown),
            "You're now using extra usage"
        );
    }

    #[test]
    fn using_overage_text_is_straight_ascii() {
        // TS :324/:330 use straight ASCII apostrophes (U+0027), never the
        // curly U+2019; the only non-ASCII byte allowed is the U+00B7
        // separator.
        let unknown = SubscriptionSnapshot::default();
        for info in [
            rejected(Some("five_hour"), Some(ts_in(3600))),
            rejected(Some("five_hour"), None),
            rejected(None, None),
        ] {
            let got = using_overage_text(&info, &unknown);
            assert!(
                !got.contains('\u{2019}'),
                "curly apostrophe in {got:?} — TS uses straight ASCII"
            );
        }
    }

    #[test]
    fn warning_upsell_copy_is_straight_ascii() {
        // rateLimitMessages.ts:274 + :282 use plain ASCII (no curly
        // apostrophes, unlike the TSX getUpsellMessage strings).
        assert_eq!(EXTRA_USAGE_REQUEST, "/extra-usage to request more");
        assert_eq!(UPGRADE_KEEP_USING, "/upgrade to keep using LingXi");
        for s in [EXTRA_USAGE_REQUEST, UPGRADE_KEEP_USING] {
            assert!(s.is_ascii(), "warning upsell copy must be straight ASCII");
            assert!(!s.contains('\u{2019}'));
        }
    }
}
