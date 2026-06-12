//! Rate-limit message composer — port of claude-code
//! `services/rateLimitMessages.ts` `getRateLimitMessage` (lines 45-104) plus
//! the upsell selection from `components/messages/RateLimitMessage.tsx`
//! `getUpsellMessage` (llm-client future-work batch 3, Task 9).
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
//! ## Documented gaps (no subscription granularity in the TUI)
//!
//! The TS composer consults `getSubscriptionType()` / `getRateLimitTier()` /
//! `getOauthAccountInfo()?.hasExtraUsageEnabled` / `hasClaudeAiBillingAccess()`
//! — none of which are plumbed into the TUI event. Per the batch-3 plan's
//! scope-guard the port treats the subscription as UNKNOWN:
//! - `getLimitReachedText` `seven_day_sonnet` (rateLimitMessages.ts:175-182):
//!   pro/enterprise would say `weekly limit`; the unknown-subscription port
//!   uses the default `Sonnet limit`.
//! - `getRateLimitMessage` team/enterprise early-warning suppression
//!   (rateLimitMessages.ts:80-94) is skipped (treated as non-team).
//! - `getEarlyWarningText`'s inline `getWarningUpsellText`
//!   (rateLimitMessages.ts:261-297): every arm requires pro/max/team/
//!   enterprise knowledge, so the unknown-subscription port resolves to
//!   `None` (no inline warning upsell).
//! - `getUpsellMessage` (RateLimitMessage.tsx): `isMax20x`, the auto-open
//!   options menu, and the team/enterprise arms are unreachable; only the two
//!   generic arms ([`upsell::UPGRADE`] / [`upsell::UPGRADE_OR_EXTRA`]) are
//!   used. See [`error_upsell`].

use crate::components::messages::rate_limit::upsell;
use orchestrator::model::rate_limit::format_reset_time;

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
pub fn compose_rate_limit(info: &RateLimitInfo) -> Option<ComposedRateLimit> {
    // TS `formatLimitReachedText` branches on `process.env.USER_TYPE ===
    // 'ant'` (rateLimitMessages.ts:339); read it here so the core stays
    // injectable for tests.
    compose_with(info, std::env::var("USER_TYPE").as_deref() == Ok("ant"))
}

/// Testable core of [`compose_rate_limit`] with the `USER_TYPE === 'ant'`
/// flag injected.
fn compose_with(info: &RateLimitInfo, is_ant: bool) -> Option<ComposedRateLimit> {
    let status = info.status.as_deref();
    let overage_status = info.overage_status.as_deref();

    // isUsingOverage derivation, ported verbatim from claudeAiLimits.ts:406-409:
    //   const isUsingOverage =
    //     status === 'rejected' &&
    //     (overageStatus === 'allowed' || overageStatus === 'allowed_warning')
    let is_using_overage = status == Some("rejected")
        && matches!(overage_status, Some("allowed" | "allowed_warning"));

    // "Check overage scenarios first (when subscription is rejected but
    // overage is available)" — rateLimitMessages.ts:49-60.
    if is_using_overage {
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
            text: limit_reached_text(info, is_ant),
            upsell: Some(error_upsell(info)),
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

        // TS :80-94 suppresses the warning for non-billing team/enterprise
        // users with extra usage enabled — subscription info the TUI doesn't
        // have (see module docs); treated as non-team.

        if let Some(text) = early_warning_text(info) {
            return Some(ComposedRateLimit { text, upsell: None });
        }
    }

    None
}

/// TS `formatResetTime(ts, true)` — showTimezone, showTime defaulted true.
fn fmt_reset(ts: Option<u64>) -> Option<String> {
    format_reset_time(ts.and_then(|t| i64::try_from(t).ok()), true, true)
}

/// Port of `getLimitReachedText` (rateLimitMessages.ts:143-197).
fn limit_reached_text(info: &RateLimitInfo, is_ant: bool) -> String {
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
        // TS :175-182 says 'weekly limit' for pro/enterprise — subscription
        // unknown in the TUI (see module docs), so the non-pro default
        // 'Sonnet limit' is used.
        Some("seven_day_sonnet") => "Sonnet limit",
        Some("seven_day_opus") => "Opus limit",
        Some("seven_day") => "weekly limit",
        Some("five_hour") => "session limit",
        _ => "usage limit",
    };
    format_limit_reached_text(limit, &reset_message, is_ant)
}

/// Port of `getEarlyWarningText` (rateLimitMessages.ts:199-254).
fn early_warning_text(info: &RateLimitInfo) -> Option<String> {
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

    // getWarningUpsellText (TS :261-297) — every arm needs subscription
    // type / hasExtraUsageEnabled, none of which the TUI has (see module
    // docs); the unknown-subscription port resolves to no inline upsell.
    let upsell: Option<&str> = None;
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

/// Dim upsell line for error-severity notices — port of
/// `RateLimitMessage.tsx` `getUpsellMessage` for the context the TUI HAS.
///
/// GAP (batch-3 plan scope-guard, documented in the module docs): the TUI
/// doesn't plumb subscription granularity, so vs the TS:
/// - `isMax20x` (rate-limit tier) is unknown → the Max-20x arms
///   (`EXTRA_USAGE_FINISH` / `LOGIN_SWITCH`) are unreachable;
/// - there is no auto-opening options menu → `OPENING_OPTIONS` unreachable;
/// - team/enterprise + billing access are unknown → those arms unreachable;
/// - `shouldShowUpsell` (`isClaudeAISubscriber()`) is treated as `true`
///   because the unified rate-limit headers only arrive on claude.ai
///   subscription accounts;
/// - `extraUsage.isEnabled()` is proxied by the presence of the
///   overage-status header (the account-level overage signal the event DOES
///   carry).
///
/// That leaves exactly the two generic arms (the TS `null` returns are all
/// inside unreachable branches, so the port's return is non-optional — the
/// caller wraps it for `ComposedRateLimit::upsell`):
/// - no overage header → `'/upgrade to increase your usage limit.'`
///   (TSX `!isTeamOrEnterprise && !isExtraUsageCommandEnabled`);
/// - overage header present → the final fallthrough
///   `'/upgrade or /extra-usage to finish what you\u{2019}re working on.'`.
fn error_upsell(info: &RateLimitInfo) -> String {
    if info.overage_status.is_none() {
        return upsell::UPGRADE.to_owned();
    }
    upsell::UPGRADE_OR_EXTRA.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(compose_with(&RateLimitInfo::default(), false), None);
    }

    #[test]
    fn status_allowed_composes_nothing() {
        let info = RateLimitInfo {
            status: Some("allowed".into()),
            ..RateLimitInfo::default()
        };
        assert_eq!(compose_with(&info, false), None);
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
        assert_eq!(compose_with(&info, false), None);
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
        let got = compose_with(&info, false).unwrap();
        assert_eq!(got.text, "You're close to your extra usage spending limit");
        assert!(!got.text.contains('\u{2019}'));
        assert_eq!(got.upsell, None, "warnings carry no dim upsell line");
    }

    // ── rejected (error) branches (rateLimitMessages.ts:63-65, 143-197) ───

    #[test]
    fn rejected_five_hour_hits_session_limit_with_reset() {
        let ts = ts_in(3600);
        let got = compose_with(&rejected(Some("five_hour"), Some(ts)), false).unwrap();
        assert_eq!(
            got.text,
            format!("You've hit your session limit \u{b7} resets {}", reset(ts))
        );
        assert!(!got.text.contains('\u{2019}'), "TS uses straight apostrophes");
    }

    #[test]
    fn rejected_seven_day_hits_weekly_limit() {
        let ts = ts_in(3600);
        let got = compose_with(&rejected(Some("seven_day"), Some(ts)), false).unwrap();
        assert_eq!(
            got.text,
            format!("You've hit your weekly limit \u{b7} resets {}", reset(ts))
        );
    }

    #[test]
    fn rejected_seven_day_opus_hits_opus_limit() {
        let got = compose_with(&rejected(Some("seven_day_opus"), None), false).unwrap();
        assert_eq!(got.text, "You've hit your Opus limit");
    }

    #[test]
    fn rejected_seven_day_sonnet_hits_sonnet_limit() {
        // Unknown subscription → the non-pro/enterprise default 'Sonnet
        // limit' (TS :175-182; gap documented in the module docs).
        let got = compose_with(&rejected(Some("seven_day_sonnet"), None), false).unwrap();
        assert_eq!(got.text, "You've hit your Sonnet limit");
    }

    #[test]
    fn rejected_unknown_type_hits_usage_limit() {
        let got = compose_with(&rejected(None, None), false).unwrap();
        assert_eq!(got.text, "You've hit your usage limit");
    }

    #[test]
    fn rejected_ant_user_appends_feedback_and_reset_hint() {
        // TS formatLimitReachedText USER_TYPE === 'ant' branch (:339-341).
        let got = compose_with(&rejected(Some("five_hour"), None), true).unwrap();
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
        let got = compose_with(&info, false).unwrap();
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
        let got = compose_with(&info, false).unwrap();
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
        let got = compose_with(&info, false).unwrap();
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
        let got = compose_with(&info, false).unwrap();
        assert_eq!(
            got.text,
            format!("You've hit your limit \u{b7} resets {}", reset(early))
        );
    }

    // ── error upsell (RateLimitMessage.tsx getUpsellMessage) ─────────────

    #[test]
    fn rejected_without_overage_header_upsells_upgrade() {
        let got = compose_with(&rejected(Some("five_hour"), None), false).unwrap();
        assert_eq!(got.upsell.as_deref(), Some(upsell::UPGRADE));
    }

    #[test]
    fn rejected_with_overage_header_upsells_upgrade_or_extra() {
        let info = RateLimitInfo {
            overage_status: Some("rejected".into()),
            ..rejected(Some("five_hour"), None)
        };
        let got = compose_with(&info, false).unwrap();
        assert_eq!(got.upsell.as_deref(), Some(upsell::UPGRADE_OR_EXTRA));
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
        assert_eq!(compose_with(&info, false), None);
    }

    #[test]
    fn warning_at_threshold_composes() {
        // TS gate is `utilization < WARNING_THRESHOLD` → exactly 0.7 warns.
        let ts = ts_in(3600);
        let got = compose_with(&warning(Some("seven_day"), Some(0.7), Some(ts)), false).unwrap();
        assert_eq!(
            got.text,
            format!("You've used 70% of your weekly limit \u{b7} resets {}", reset(ts))
        );
        assert_eq!(got.upsell, None);
    }

    #[test]
    fn warning_used_percentage_floors() {
        // Math.floor(0.857 * 100) = 85 (TS :223).
        let got = compose_with(&warning(Some("seven_day"), Some(0.857), None), false).unwrap();
        assert_eq!(got.text, "You've used 85% of your weekly limit");
    }

    #[test]
    fn warning_without_utilization_says_approaching() {
        // TS only short-circuits when utilization !== undefined (:73-78);
        // absent utilization falls through to the 'Approaching' copy (:247-253).
        let ts = ts_in(3600);
        let got = compose_with(&warning(Some("five_hour"), None, Some(ts)), false).unwrap();
        assert_eq!(
            got.text,
            format!("Approaching session limit \u{b7} resets {}", reset(ts))
        );
    }

    #[test]
    fn warning_overage_type_appends_limit_to_approaching() {
        // TS :242-245 — 'extra usage' becomes 'extra usage limit' on the
        // Approaching path.
        let got = compose_with(&warning(Some("overage"), None, None), false).unwrap();
        assert_eq!(got.text, "Approaching extra usage limit");
    }

    #[test]
    fn warning_overage_type_with_utilization_keeps_extra_usage() {
        let got = compose_with(&warning(Some("overage"), Some(0.9), None), false).unwrap();
        assert_eq!(got.text, "You've used 90% of your extra usage");
    }

    #[test]
    fn warning_zero_utilization_composes_nothing() {
        // `0 !== undefined` and `0 < 0.7` → the TS threshold guard returns
        // null (TS :73-78).
        let info = warning(Some("seven_day"), Some(0.0), Some(ts_in(3600)));
        assert_eq!(compose_with(&info, false), None);
    }

    #[test]
    fn warning_unknown_type_composes_nothing() {
        // TS getEarlyWarningText returns null for `undefined` (:217-218);
        // unknown strings can't occur in the typed union — treated alike.
        assert_eq!(compose_with(&warning(None, Some(0.9), None), false), None);
    }
}
