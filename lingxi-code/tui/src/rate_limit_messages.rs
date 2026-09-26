//! Rate-limit message composer — port of claude-code
//! `services/rateLimitMessages.ts` `getRateLimitMessage` (lines 45-104) plus
//! the upsell selection from `components/messages/RateLimitMessage.tsx`
//! `getUpsellMessage` (byte-locked strings landed in batch 3; the full
//! subscription-aware arm port landed in batch 4 — see the CLOSED list below).
//!
//! (fix round 1) Carried over 1:1 from the iocraft backend's
//! `tui/src/rate_limit_messages.rs` so [`crate::chat_widget::ChatWidget`] can
//! fold `TurnEvent::RateLimit` header snapshots into
//! `RenderedMessage::RateLimit` notices exactly like the old renderer. The
//! locked upsell literals (previously
//! `tui/src/components/messages/rate_limit.rs`) live in the local [`upsell`]
//! module.
//!
//! Pure function: the nine `TurnEvent::RateLimit` header-derived fields in,
//! `Option<ComposedRateLimit { text, upsell }>` out. Copy strings are
//! byte-identical to the TS source — note that `rateLimitMessages.ts` uses
//! STRAIGHT ASCII apostrophes throughout (`You've hit your…` literals at
//! lines 169/233/238/340/343 are `'`/U+0027, NOT U+2019); the curly
//! apostrophe/ellipsis literals live only in the `getUpsellMessage` strings,
//! already byte-locked in [`upsell`].
//!
//! Reset-time formatting REUSES
//! [`llm_runtime::model::rate_limit::format_reset_time`] — the existing 1:1
//! port of TS `utils/format.ts` `formatResetTime` (en-US, 12-hour, minute
//! omitted at `:00`, lowercased am/pm, `(<tz>)` suffix when requested). The
//! old backend reached the same function through its `orchestrator` re-export.
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

use llm_runtime::model::rate_limit::format_reset_time;
use platform_api::env::is_env_truthy;
use platform_api::subscription::SubscriptionSnapshot;

/// Locked upsell strings. Mirrors claude-code 2.1.206 `getUpsellMessage`
/// (binary `Gid` @221157422: `function
/// Gid({shouldShowUpsell:e,isMax20x:t,isExtraUsageCommandEnabled:r,shouldAutoOpenRateLimitOptionsMenu:n,isTeamOrEnterprise:o,hasBillingAccess:i,serverHidesUpgrade:s,serverHidesOverage:a,spendLimitNudgePath:l}){...}`).
/// Note the curly apostrophe U+2019 in "you’re" and the ellipsis U+2026 in
/// "Opening your options…" — both byte-verified against the real 2.1.206
/// binary's minified-source region (the `/extra-usage` slash-command was
/// renamed to `/usage-credits` in 206; see `Ytr`/`Xte`/`Mhs` nearby in the
/// same binary region).
pub mod upsell {
    /// Max-20x + extra-usage enabled; team/enterprise + billing access;
    /// serverHidesUpgrade + extra-usage enabled. Reused verbatim across
    /// THREE `Gid` branches (byte-verified: the same literal appears 3x in
    /// the binary's `Gid` body).
    pub const USAGE_CREDITS_FINISH: &str =
        "/usage-credits to finish what you\u{2019}re working on.";
    /// Max-20x, extra-usage disabled.
    pub const LOGIN_SWITCH: &str = "/login to switch to an API usage-billed account.";
    /// Auto-open menu (`shouldAutoOpenRateLimitOptionsMenu`); structurally
    /// unreachable in the TUI (no interactive rate-limit options menu).
    pub const OPENING_OPTIONS: &str = "Opening your options\u{2026}";
    /// `spendLimitNudgePath` arm.
    pub const SPEND_LIMIT_NUDGE: &str = "/usage-credits to adjust your monthly spend limit.";
    /// Default (non-team, no extra-usage / serverHidesUpgrade false).
    pub const UPGRADE: &str = "/upgrade to increase your usage limit.";
    /// Team/enterprise, no billing access.
    pub const USAGE_CREDITS_REQUEST_ADMIN: &str =
        "/usage-credits to request more usage from your admin.";
    /// Team/enterprise, extra-usage disabled (`!c`).
    pub const USAGE_CREDITS_ADMIN_ENABLE: &str =
        "Your admin can enable extra usage at claude.ai/admin-settings/usage.";
    /// Fallback (non-team, serverHidesUpgrade false, extra-usage enabled).
    pub const UPGRADE_OR_USAGE_CREDITS: &str =
        "/upgrade or /usage-credits to finish what you\u{2019}re working on.";
}

/// The nine header-derived fields of `TurnEvent::RateLimit`
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
    /// `anthropic-ratelimit-unified-upgrade-paths` (2.1.206), parsed to a
    /// list. Consumed by the upsell (`getUpsellMessage`) server-hide /
    /// spend-nudge derivations (a later task); threaded here so the
    /// composer has the input once that logic lands.
    pub upgrade_paths: Option<Vec<String>>,
    /// 2.1.206 `credits_required` derivation. Consumed by the upsell
    /// suppression gate (`shouldShowUpsell`, a later task).
    pub credits_required: bool,
}

/// A composed rate-limit notice: error/warning text plus the optional dim
/// upsell line rendered under it by
/// [`crate::history_cell::system::RateLimitCell`].
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
pub fn compose_rate_limit(
    info: &RateLimitInfo,
    sub: &SubscriptionSnapshot,
) -> Option<ComposedRateLimit> {
    // 2.1.206 `lhe` (the compiled `formatLimitReachedText`) no longer branches
    // on `process.env.USER_TYPE === 'ant'` — that #briarpatch-cc arm was removed
    // (see `format_limit_reached_text`'s doc comment), so the `USER_TYPE` read
    // and the `is_ant` thread it fed are gone. `extraUsage.isEnabled()`
    // (commands/extra-usage/index.ts:6-17) reads `DISABLE_EXTRA_USAGE_COMMAND`
    // through `isEnvTruthy`; the read happens here so the core stays injectable.
    let disable_extra_usage =
        is_env_truthy(std::env::var("DISABLE_EXTRA_USAGE_COMMAND").ok().as_deref());
    compose_with(
        info,
        sub,
        sub.is_extra_usage_command_enabled(disable_extra_usage),
    )
}

/// Testable core of [`compose_rate_limit`] with `extraUsage.isEnabled()` injected.
fn compose_with(
    info: &RateLimitInfo,
    sub: &SubscriptionSnapshot,
    extra_usage_cmd_enabled: bool,
) -> Option<ComposedRateLimit> {
    let status = info.status.as_deref();
    let overage_status = info.overage_status.as_deref();

    // "Check overage scenarios first (when subscription is rejected but
    // overage is available)" — rateLimitMessages.ts:49-60.
    if is_using_overage(info) {
        if overage_status == Some("allowed_warning") {
            // 2.1.206 `Fdu` (binary @217902246): `` `You're close to your
            // ${A5()?"usage limit":"usage credit limit"}` `` — straight
            // ASCII apostrophe, byte-verified against the real binary. This
            // replaces the pre-206 "extra usage spending limit" wording.
            let limit_name = if sub.is_usage_based_billing() {
                "usage limit"
            } else {
                "usage credit limit"
            };
            return Some(ComposedRateLimit {
                text: format!("You're close to your {limit_name}"),
                upsell: None,
            });
        }
        return None;
    }

    // "ERROR STATES - when limits are rejected" — rateLimitMessages.ts:62-65.
    if status == Some("rejected") {
        // 2.1.206 `Tdo` (binary @221157422 area): `jid =
        // fit.rateLimitType==="seven_day_overage_included" ||
        // fit.errorCode==="credits_required"` — when `jid`, the upsell
        // region is hard-nulled (`if(jid){Uhs=null;break bb0}`) BEFORE `Gid`
        // is ever invoked, regardless of what `Gid`'s own inputs would
        // otherwise produce.
        let jid = info.rate_limit_type.as_deref() == Some("seven_day_overage_included")
            || info.credits_required;
        let upsell = if jid {
            None
        } else {
            error_upsell(&UpsellInputs {
                should_show_upsell: should_show_upsell(sub),
                is_max20x: sub.is_max20x(),
                is_extra_usage_command_enabled: extra_usage_cmd_enabled,
                // `Tdo` derives this from pending-menu-open UI state
                // (`_do=j$C&&yly==="pending"&&_ly&&!jid&&ydo`) that has no
                // TUI counterpart — there is no interactive rate-limit
                // options menu here, so the arm is structurally always
                // false (see [`upsell::OPENING_OPTIONS`]'s doc comment).
                should_auto_open_rate_limit_options_menu: false,
                is_team_or_enterprise: sub.is_team_or_enterprise(),
                has_billing_access: sub.has_claude_ai_billing_access(),
                server_hides_upgrade: server_hides_upgrade(info),
                server_hides_overage: server_hides_overage(info),
                spend_limit_nudge_path: spend_limit_nudge_path(info, sub, extra_usage_cmd_enabled),
            })
        };
        return Some(ComposedRateLimit {
            text: limit_reached_text(info, sub),
            upsell,
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
/// `pub` so the chat widget's overage-transition notice
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

/// Port of `getUsingOverageText` (2.1.206 binary `a7n` @217907540) — the
/// transient notice shown once when the session rolls into extra usage
/// (`useRateLimitWarningNotification.tsx` fires it on the overage
/// transition). Decoded body (byte-verified against the real 2.1.206 binary;
/// straight ASCII apostrophe in "You're", U+00B7 middot separator):
///
/// ```text
/// function a7n(e, t) {   // t = model
///   r = e.resetsAt ? Jie(e.resetsAt, !0) : ""
///   n = ""
///   if (e.rateLimitType === "five_hour") n = "session limit"
///   else if (e.rateLimitType === "seven_day") n = "weekly limit"
///   else if (e.rateLimitType === "seven_day_opus") n = "Opus limit"
///   else if (e.rateLimitType === "seven_day_sonnet") {
///     a = Fs(); n = a === "pro" || a === "enterprise" ? "weekly limit" : "Sonnet limit"
///   }
///   o = A5()
///   // model-specific arm — see COLLAPSE note below, omitted here
///   i = o ? "your usage allocation" : "usage credits"
///   if (!n) return `Now using ${i}`
///   s = r && !o ? ` · Your ${n} resets ${r}` : ""
///   return `You're now using ${i}${s}`
/// }
/// ```
///
/// Key 206 changes vs the prior (205) port: "extra usage" →
/// `o?"your usage allocation":"usage credits"`; the no-limit-name case is now
/// `"Now using {i}"` (was "Now using extra usage"); the reset suffix is
/// suppressed entirely when `o` (usage-based billing) is true.
///
/// 206 `a7n` has a model-specific arm `if(!n && !o && model &&
/// O9e().includes(VQ(ei(model)))) return "Now using usage credits for
/// ${model}…"`. Per Task 2's pinning `overage_included_models()` is empty (no
/// `tengu_usage_overage_included_models` source), so `O9e().includes(...)` is
/// always false and the arm never fires; it is omitted and the model param is
/// not threaded.
#[must_use]
pub fn using_overage_text(info: &RateLimitInfo, sub: &SubscriptionSnapshot) -> String {
    // JS `r = e.resetsAt ? formatResetTime(resetsAt, true) : ""` — an empty
    // STRING (not an absent Option) when resetsAt is unset; the `s` guard
    // below checks `r` truthiness (`r != ""`), matching the JS exactly.
    let r: String = fmt_reset(info.resets_at).unwrap_or_default();
    // limitName chain.
    let n: &str = match info.rate_limit_type.as_deref() {
        Some("five_hour") => "session limit",
        Some("seven_day") => "weekly limit",
        Some("seven_day_opus") => "Opus limit",
        Some("seven_day_sonnet") => {
            // "For pro and enterprise, Sonnet limit is the same as weekly".
            if sub.is_pro_or_enterprise() {
                "weekly limit"
            } else {
                "Sonnet limit"
            }
        }
        _ => "",
    };
    let o = sub.is_usage_based_billing();
    let i = if o {
        "your usage allocation"
    } else {
        "usage credits"
    };
    // `!n` → the bare copy, BEFORE any reset suffix.
    if n.is_empty() {
        return format!("Now using {i}");
    }
    // `s = r && !o ? ` · Your ${n} resets ${r}` : ""` — the reset suffix is
    // suppressed outright when usage-based billing (`o`) is true, regardless
    // of whether a reset time is present.
    let s = if !r.is_empty() && !o {
        format!(" \u{b7} Your {n} resets {r}")
    } else {
        String::new()
    };
    format!("You're now using {i}{s}")
}

/// `Hqi` (2.1.206 binary, string table @87692608:
/// `Hqi=new Set(["org_level_disabled_until","org_spend_cap_reached"])`) — the
/// org-level overage-disabled reasons that route to the "monthly spend
/// limit" copy ahead of the seat/member/group taxonomy. Byte-verified
/// against the real 2.1.206 binary.
const HQI_REASONS: [&str; 2] = ["org_level_disabled_until", "org_spend_cap_reached"];

/// Port of `getLimitReachedText` (2.1.206 binary `Ucg` @217902843), the
/// usage-credits + org/seat/member/group taxonomy rewrite. Decoded body
/// (straight ASCII apostrophes; `\xB7` middot escapes in the minified JS
/// source, a literal U+00B7 byte in the binary's separate Latin-1 string
/// table @87692608 — both regions cross-checked) byte-verified against the
/// real 2.1.206 binary:
///
/// ```text
/// function Ucg(e,t){
///   let r=A5(),n=tC(),
///       o=n?"":" · contact your admin to increase it",
///       i=e.resetsAt, s=i?Jie(i,!0):void 0,
///       a=e.overageResetsAt?Jie(e.overageResetsAt,!0):void 0,
///       l=s?` · resets ${s}`:"",
///       c=qcg(e,l,t);
///   if(!r&&e.overageDisabledReason&&c&&!Hqi.has(e.overageDisabledReason)
///        &&(e.rateLimitType==="seven_day_overage_included"||!(ZA(t)&&WBe()&&!B5())))
///     return c;
///   if(!r&&e.overageDisabledReason&&Hqi.has(e.overageDisabledReason)){
///     let u=Fs();
///     if(u==="team"||u==="enterprise")
///       return lhe("org's monthly spend limit",
///         n?" · run /usage-credits to raise it, or visit claude.ai/admin-settings/usage"
///          :" · run /usage-credits to ask your admin for a higher limit",t);
///     return lhe(n?"monthly spend limit":"org's monthly spend limit",
///       n?" · raise it at claude.ai/settings/usage"
///        :" · ask your admin to raise it at claude.ai/settings/usage",t)
///   }
///   if(e.overageStatus==="rejected"){
///     let u="";
///     if(i&&e.overageResetsAt) if(i<e.overageResetsAt)u=` · resets ${s}`;else u=` · resets ${a}`;
///     else if(s)u=` · resets ${s}`; else if(a)u=` · resets ${a}`;
///     if(e.overageDisabledReason==="out_of_credits"){
///       if(r)return n?"Your org is out of usage · add funds to continue"
///                    :"Your org is out of usage · contact your admin";
///       return `You're out of usage credits${u}`
///     }
///     if(e.overageDisabledReason&&Hqi.has(e.overageDisabledReason)){
///       let d=a?` · resets ${a}`:"";
///       return lhe("org's monthly usage limit",d,t)
///     }
///     if(e.overageDisabledReason==="seat_tier_level_disabled"
///        ||e.overageDisabledReason==="seat_tier_zero_credit_limit")
///       return `Your seat type doesn't include ${r?"usage":"usage credits"}`;
///     if(e.overageDisabledReason==="org_service_level_disabled")
///       return"This service is disabled for your org";
///     if(e.overageDisabledReason==="member_level_disabled"
///        ||e.overageDisabledReason==="member_zero_credit_limit")
///       return"Your usage allocation has been disabled by your admin · run /usage-credits to ask your admin for a higher limit";
///     if(e.overageDisabledReason==="group_zero_credit_limit")
///       return"Your group's usage limit is set to $0 · run /usage-credits to ask your admin for a higher limit";
///     if(r)return lhe("usage limit",o,t);
///     return lhe("limit",u,t)
///   }
///   if(c)return c;
///   if(r)return lhe("usage limit",o,t);
///   return lhe("usage limit",l,t)
/// }
/// ```
///
/// Byte-note: the "org's monthly spend limit" team/enterprise-with-billing
/// variant is `" · run /usage-credits to raise it, or visit
/// claude.ai/admin-settings/usage"` in the real binary — longer than an
/// earlier draft's abbreviated `" · visit claude.ai/admin-settings/usage"`;
/// this port uses the byte-verified binary text (confirmed both in the
/// minified-source region @217902843 and the separate Latin-1 string table
/// @87692608, which agree byte-for-byte).
///
/// `r`=[`SubscriptionSnapshot::is_usage_based_billing`] (`A5`),
/// `n`=[`SubscriptionSnapshot::has_claude_ai_billing_access`] (`tC`),
/// `Fs()`=`sub.subscription_type`, `qcg`=[`qcg_limit_name`] composed with
/// [`format_limit_reached_text`] (`lhe`), `Jie(x,!0)`=[`fmt_reset`].
///
/// FIRST-GUARD COLLAPSE: the guard's final disjunct
/// `(rateLimitType==="seven_day_overage_included"||!(ZA(t)&&WBe()&&!B5()))`
/// is omitted below. Per Task 2's pinning, [`overage_consent_required`]
/// (`WBe`) is a documented `false` in this port, so `ZA(t)&&WBe()&&!B5()` is
/// always `false`, making `!(…)` always `true` — the whole disjunct
/// collapses to a hard `true` regardless of `rateLimitType`, `ZA`, or `B5`.
/// `ZA(t)` is the only consumer of the model parameter `t` in this function,
/// so the model is not threaded here; see [`is_fable_model`] /
/// [`overage_consent_required`] for the pinned leaves.
fn limit_reached_text(info: &RateLimitInfo, sub: &SubscriptionSnapshot) -> String {
    let r = sub.is_usage_based_billing();
    let n = sub.has_claude_ai_billing_access();
    let o: &str = if n {
        ""
    } else {
        " \u{b7} contact your admin to increase it"
    };

    let reset_time = fmt_reset(info.resets_at);
    let overage_reset_time = fmt_reset(info.overage_resets_at);
    let reset_message = reset_time
        .as_deref()
        .map_or_else(String::new, |t| format!(" \u{b7} resets {t}"));

    let c = qcg_limit_name(info.rate_limit_type.as_deref(), sub)
        .map(|name| format_limit_reached_text(name, &reset_message));

    let reason = info.overage_disabled_reason.as_deref();
    let is_hqi_reason = reason.is_some_and(|x| HQI_REASONS.contains(&x));

    // 206 Ucg first guard (collapsed — see doc comment above).
    if !r && reason.is_some() && c.is_some() && !is_hqi_reason {
        return c.unwrap();
    }

    // Hqi (org-level spend-cap) reasons OUTSIDE the overage-rejected branch —
    // this fires regardless of `overageStatus` and is checked BEFORE the
    // `overageStatus === "rejected"` block below, so it takes priority over
    // the Hqi arm nested inside that block (which is reachable only when `r`
    // is true, since this outer arm always intercepts `!r` cases first).
    if !r && is_hqi_reason {
        let u = sub.subscription_type.as_deref();
        if matches!(u, Some("team" | "enterprise")) {
            return format_limit_reached_text(
                "org's monthly spend limit",
                if n {
                    " \u{b7} run /usage-credits to raise it, or visit claude.ai/admin-settings/usage"
                } else {
                    " \u{b7} run /usage-credits to ask your admin for a higher limit"
                },
            );
        }
        return format_limit_reached_text(
            if n {
                "monthly spend limit"
            } else {
                "org's monthly spend limit"
            },
            if n {
                " \u{b7} raise it at claude.ai/settings/usage"
            } else {
                " \u{b7} ask your admin to raise it at claude.ai/settings/usage"
            },
        );
    }

    // "if BOTH subscription (checked before this method) and overage are
    // exhausted".
    if info.overage_status.as_deref() == Some("rejected") {
        // "Show the earliest reset time". The first branch gates on the RAW
        // timestamps (`resetsAt && overageResetsAt`, 0 falsy), the fallbacks
        // on the formatted strings.
        let earliest = match (
            info.resets_at.filter(|t| *t != 0),
            info.overage_resets_at.filter(|t| *t != 0),
        ) {
            (Some(rt), Some(ot)) => {
                if rt < ot {
                    reset_time.clone()
                } else {
                    overage_reset_time.clone()
                }
            }
            _ => reset_time.clone().or_else(|| overage_reset_time.clone()),
        };
        let u = earliest
            .as_deref()
            .map_or_else(String::new, |t| format!(" \u{b7} resets {t}"));

        if reason == Some("out_of_credits") {
            if r {
                return if n {
                    "Your org is out of usage \u{b7} add funds to continue".to_owned()
                } else {
                    "Your org is out of usage \u{b7} contact your admin".to_owned()
                };
            }
            return format!("You're out of usage credits{u}");
        }
        if is_hqi_reason {
            let d = overage_reset_time
                .as_deref()
                .map_or_else(String::new, |t| format!(" \u{b7} resets {t}"));
            return format_limit_reached_text("org's monthly usage limit", &d);
        }
        if matches!(
            reason,
            Some("seat_tier_level_disabled" | "seat_tier_zero_credit_limit")
        ) {
            return format!(
                "Your seat type doesn't include {}",
                if r { "usage" } else { "usage credits" }
            );
        }
        if reason == Some("org_service_level_disabled") {
            return "This service is disabled for your org".to_owned();
        }
        if matches!(
            reason,
            Some("member_level_disabled" | "member_zero_credit_limit")
        ) {
            return "Your usage allocation has been disabled by your admin \u{b7} run /usage-credits to ask your admin for a higher limit".to_owned();
        }
        if reason == Some("group_zero_credit_limit") {
            return "Your group's usage limit is set to $0 \u{b7} run /usage-credits to ask your admin for a higher limit".to_owned();
        }
        if r {
            return format_limit_reached_text("usage limit", o);
        }
        return format_limit_reached_text("limit", &u);
    }

    if let Some(c) = c {
        return c;
    }
    if r {
        return format_limit_reached_text("usage limit", o);
    }
    format_limit_reached_text("usage limit", &reset_message)
}

/// Port of `qcg` (2.1.206 binary @217905040) limit-name mapping:
///
/// ```text
/// function qcg(e,t,r){
///   if(e.rateLimitType==="seven_day_sonnet"){let n=Fs();
///     return lhe(n==="pro"||n==="enterprise"?"weekly limit":"Sonnet limit",t,r)}
///   if(e.rateLimitType==="seven_day_opus")return lhe("Opus limit",t,r);
///   if(e.rateLimitType==="seven_day_overage_included")return lhe("Fable 5 limit",t,r);
///   if(e.rateLimitType==="seven_day")return lhe("weekly limit",t,r);
///   if(e.rateLimitType==="five_hour")return lhe("session limit",t,r);
///   ...
/// }
/// ```
///
/// `qcg` itself calls straight through to `lhe`
/// ([`format_limit_reached_text`]) with the resolved name; this helper
/// isolates just the name resolution so callers can reuse it —
/// [`limit_reached_text`] (`Ucg`) is the first non-test caller.
///
/// `seven_day_overage_included` → `"Fable 5 limit"` is NEW in 2.1.206
/// ("Fable 5" is a model name — not rebranded). Byte-verified against the
/// real 2.1.206 binary (string table @87697088).
#[must_use]
pub(crate) fn qcg_limit_name(
    rate_limit_type: Option<&str>,
    sub: &SubscriptionSnapshot,
) -> Option<&'static str> {
    match rate_limit_type {
        Some("seven_day_sonnet") => Some(if sub.is_pro_or_enterprise() {
            "weekly limit"
        } else {
            "Sonnet limit"
        }),
        Some("seven_day_opus") => Some("Opus limit"),
        Some("seven_day_overage_included") => Some("Fable 5 limit"),
        Some("seven_day") => Some("weekly limit"),
        Some("five_hour") => Some("session limit"),
        _ => None,
    }
}

/// Port of `getEarlyWarningText` (2.1.206 binary `jcg`, decoded body
/// byte-verified against the real 2.1.206 binary @217905476). Key 206
/// changes vs the prior (205) port: (1) a NEW
/// `seven_day_overage_included` → `"Fable 5 limit"` case; (2) the `overage`
/// limit name is now `A5()`-gated (`"usage"`/`"usage credits"` pre-Approaching,
/// `"usage limit"`/`"usage credit limit"` in the Approaching branch),
/// replacing the old `"extra usage"`/`"extra usage limit"` wording; (3) the
/// reset time is suppressed when `rateLimitType==="overage" && A5()` (JS `n`);
/// (4) the upsell suffix now comes from the 206 `Wcg` ([`warning_upsell`]).
fn early_warning_text(info: &RateLimitInfo, sub: &SubscriptionSnapshot) -> Option<String> {
    let is_overage = info.rate_limit_type.as_deref() == Some("overage");
    let usage_based = sub.is_usage_based_billing();

    let limit_name = match info.rate_limit_type.as_deref() {
        Some("seven_day") => "weekly limit",
        Some("five_hour") => "session limit",
        Some("seven_day_opus") => "Opus limit",
        Some("seven_day_sonnet") => "Sonnet limit",
        Some("seven_day_overage_included") => "Fable 5 limit",
        Some("overage") => {
            if usage_based {
                "usage"
            } else {
                "usage credits"
            }
        }
        // TS `case void 0: return null`; unknown strings can't occur in the
        // typed union — treated alike.
        _ => return None,
    };

    // TS `r = e.utilization?Math.floor(e.utilization*100):void 0`, then
    // truthiness-gated (`if(r&&...)`) — so both a falsy utilization (0) and a
    // floored 0% behave as absent. Kept as an already-floored `f64` rendered
    // with `{:.0}` (no int cast): identical digits for every in-range
    // fraction, and non-finite values (JS-falsy `NaN`) are filtered like the
    // TS truthiness gate.
    let used = info
        .utilization
        .filter(|u| *u != 0.0)
        .map(|u| (u * 100.0).floor())
        .filter(|n| *n != 0.0 && n.is_finite());

    // 206 `n = e.rateLimitType==="overage" && A5()`: suppress the reset time
    // for usage-based-billing overage notices even when `resetsAt` is set.
    let reset_suppressed = is_overage && usage_based;
    let reset_time = if reset_suppressed {
        None
    } else {
        fmt_reset(info.resets_at)
    };

    // "Get upsell command based on subscription type and limit type" —
    // now the 206 `Wcg` ([`warning_upsell`]).
    let upsell = warning_upsell(info.rate_limit_type.as_deref(), sub);
    let with_upsell = |base: String| match upsell {
        Some(u) => format!("{base} \u{b7} {u}"),
        None => base,
    };

    if let Some(used) = used {
        let base = if let Some(reset_time) = &reset_time {
            format!("You've used {used:.0}% of your {limit_name} \u{b7} resets {reset_time}")
        } else {
            format!("You've used {used:.0}% of your {limit_name}")
        };
        return Some(with_upsell(base));
    }

    // 206 `if(e.rateLimitType==="overage")t=A5()?"usage limit":"usage credit
    // limit"` — the Approaching-only reassignment of the overage limit name.
    let limit_name = if is_overage {
        if usage_based {
            "usage limit"
        } else {
            "usage credit limit"
        }
    } else {
        limit_name
    };

    let base = if let Some(reset_time) = reset_time {
        format!("Approaching {limit_name} \u{b7} resets {reset_time}")
    } else {
        format!("Approaching {limit_name}")
    };
    Some(with_upsell(base))
}

/// Port of 2.1.206 `lhe(e,t,r)` = `You've hit your ${e}${t}` — the
/// `formatLimitReachedText` (`rateLimitMessages.ts`). The pre-206
/// `USER_TYPE==='ant'` #briarpatch-cc/reset-limits branch (and the `model`/`_model`
/// arg) were removed in 206, so this is a pure two-part concatenation.
fn format_limit_reached_text(limit: &str, reset_message: &str) -> String {
    format!("You've hit your {limit}{reset_message}")
}

/// Warning-upsell copy (2.1.206 binary `Wcg`, string literals at
/// @87701824 / @217906477 — straight ASCII, byte-verified against the real
/// binary). `USAGE_CREDITS_ASK_ADMIN`
/// is shared by both team/enterprise branches below (the `!hasBillingAccess`
/// arm of each).
const USAGE_CREDITS_TURN_ON: &str = "Run /usage-credits to turn on extra usage for your org";
const USAGE_CREDITS_ASK_ADMIN: &str = "Run /usage-credits to ask your admin for more";
const USAGE_CREDITS_RAISE_CAP: &str = "Run /usage-credits to raise the cap";
/// 2.1.206 says `"/upgrade to keep using Claude Code"`; kept as the LingXi
/// product name per this port's rebrand convention (byte-locked pre-206).
const UPGRADE_KEEP_USING: &str = "/upgrade to keep using LingXi";

/// Port of `getWarningUpsellText` (2.1.206 binary `Wcg`):
///
/// ```text
/// function Wcg(e){
///   let t=Fs(), r=Uc()?.hasExtraUsageEnabled===!0, n=tC();
///   if(t==="team"||t==="enterprise"){
///     if(!r&&QJe())return n?"Run /usage-credits to turn on extra usage for your org"
///                          :"Run /usage-credits to ask your admin for more";
///     if(r&&e==="overage")return n?"Run /usage-credits to raise the cap"
///                                 :"Run /usage-credits to ask your admin for more";
///     return null
///   }
///   if(e==="five_hour"&&(t==="pro"||t==="max")&&!Pee())
///     return"/upgrade to keep using Claude Code";
///   return null
/// }
/// ```
///
/// `t`=`sub.subscription_type`, `r`=`sub.has_extra_usage_enabled`,
/// `n`=`sub.has_claude_ai_billing_access()` (`tC`), `QJe`=
/// `sub.is_overage_provisioning_allowed()`, `Pee`=[`flags::idle_amber_finch`]
/// (documented `false` in this port ⇒ the five_hour pro/max branch always
/// fires).
fn warning_upsell(
    rate_limit_type: Option<&str>,
    sub: &SubscriptionSnapshot,
) -> Option<&'static str> {
    let has_billing_access = sub.has_claude_ai_billing_access();
    if sub.is_team_or_enterprise() {
        if !sub.has_extra_usage_enabled && sub.is_overage_provisioning_allowed() {
            return Some(if has_billing_access {
                USAGE_CREDITS_TURN_ON
            } else {
                USAGE_CREDITS_ASK_ADMIN
            });
        }
        if sub.has_extra_usage_enabled && rate_limit_type == Some("overage") {
            return Some(if has_billing_access {
                USAGE_CREDITS_RAISE_CAP
            } else {
                USAGE_CREDITS_ASK_ADMIN
            });
        }
        return None;
    }
    if rate_limit_type == Some("five_hour")
        && matches!(sub.subscription_type.as_deref(), Some("pro" | "max"))
        && !flags::idle_amber_finch()
    {
        return Some(UPGRADE_KEEP_USING);
    }
    None
}

/// The nine `Gid` inputs (2.1.206 `getUpsellMessage`, binary `Gid`
/// @221157422). Field names are the camelCase JS destructure keys
/// snake_cased 1:1 so the mapping to the decoded body stays obvious. See
/// [`error_upsell`] for the derivation of each from `compose_with`'s
/// `RateLimitInfo` / `SubscriptionSnapshot` / `extra_usage_cmd_enabled`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UpsellInputs {
    /// `e` — `Eyt()||Bo()`, reduces to `sub.is_subscriber` via
    /// [`should_show_upsell`]. Gates every other arm.
    pub should_show_upsell: bool,
    /// `t` — `sub.is_max20x()`.
    pub is_max20x: bool,
    /// `r` — the `extra_usage_cmd_enabled` param threaded through
    /// `compose_with`.
    pub is_extra_usage_command_enabled: bool,
    /// `n` — structurally `false` in the TUI (no interactive rate-limit
    /// options menu).
    pub should_auto_open_rate_limit_options_menu: bool,
    /// `o` — `sub.is_team_or_enterprise()`.
    pub is_team_or_enterprise: bool,
    /// `i` — `sub.has_claude_ai_billing_access()`.
    pub has_billing_access: bool,
    /// `s` — see [`server_hides_upgrade`].
    pub server_hides_upgrade: bool,
    /// `a` — see [`server_hides_overage`].
    pub server_hides_overage: bool,
    /// `l` — see [`spend_limit_nudge_path`].
    pub spend_limit_nudge_path: bool,
}

/// Port of `getUpsellMessage` (2.1.206 binary `Gid` @221157422), byte-verified
/// against the real binary:
///
/// ```text
/// function Gid({shouldShowUpsell:e,isMax20x:t,isExtraUsageCommandEnabled:r,
///               shouldAutoOpenRateLimitOptionsMenu:n,isTeamOrEnterprise:o,
///               hasBillingAccess:i,serverHidesUpgrade:s,serverHidesOverage:a,
///               spendLimitNudgePath:l}){
///   if(!e)return null;
///   if(n)return"Opening your options…";
///   if(l)return"/usage-credits to adjust your monthly spend limit.";
///   let c=r&&!a;
///   if(t){
///     if(c)return"/usage-credits to finish what you’re working on.";
///     return"/login to switch to an API usage-billed account."
///   }
///   if(o){
///     if(!c)return"Your admin can enable extra usage at claude.ai/admin-settings/usage.";
///     if(i)return"/usage-credits to finish what you’re working on.";
///     return"/usage-credits to request more usage from your admin."
///   }
///   if(s){
///     if(c)return"/usage-credits to finish what you’re working on.";
///     return null
///   }
///   if(!c)return"/upgrade to increase your usage limit.";
///   return"/upgrade or /usage-credits to finish what you’re working on."
/// }
/// ```
///
/// `shouldAutoOpenRateLimitOptionsMenu` (`n`) is structurally false in this
/// port: the TUI has no interactive rate-limit options menu, so the
/// `OPENING_OPTIONS` arm is unreachable (the constant stays byte-locked for
/// when the menu lands). `shouldShowUpsell` (`e`) is derived by callers via
/// [`should_show_upsell`] (`Eyt()||Bo()`, reducing to `sub.is_subscriber`).
#[must_use]
fn error_upsell(inputs: &UpsellInputs) -> Option<String> {
    let &UpsellInputs {
        should_show_upsell,
        is_max20x,
        is_extra_usage_command_enabled,
        should_auto_open_rate_limit_options_menu,
        is_team_or_enterprise,
        has_billing_access,
        server_hides_upgrade,
        server_hides_overage,
        spend_limit_nudge_path,
    } = inputs;
    if !should_show_upsell {
        return None;
    }
    if should_auto_open_rate_limit_options_menu {
        return Some(upsell::OPENING_OPTIONS.to_owned());
    }
    if spend_limit_nudge_path {
        return Some(upsell::SPEND_LIMIT_NUDGE.to_owned());
    }
    let c = is_extra_usage_command_enabled && !server_hides_overage;
    if is_max20x {
        return Some(
            if c {
                upsell::USAGE_CREDITS_FINISH
            } else {
                upsell::LOGIN_SWITCH
            }
            .to_owned(),
        );
    }
    if is_team_or_enterprise {
        if !c {
            return Some(upsell::USAGE_CREDITS_ADMIN_ENABLE.to_owned());
        }
        return Some(
            if has_billing_access {
                upsell::USAGE_CREDITS_FINISH
            } else {
                upsell::USAGE_CREDITS_REQUEST_ADMIN
            }
            .to_owned(),
        );
    }
    if server_hides_upgrade {
        return if c {
            Some(upsell::USAGE_CREDITS_FINISH.to_owned())
        } else {
            None
        };
    }
    if !c {
        return Some(upsell::UPGRADE.to_owned());
    }
    Some(upsell::UPGRADE_OR_USAGE_CREDITS.to_owned())
}

/// `serverHidesUpgrade` (`Tdo`'s `hkt = B$C||Uid`, 2.1.206 binary): the
/// server's `upgrade_paths` list is present and excludes `"upgrade_plan"`, OR
/// the `tengu_idle_amber_finch` flag (`Pee()`/[`flags::idle_amber_finch`]) is
/// set. `mle!==void 0&&!mle.includes("upgrade_plan")` — an ABSENT
/// `upgrade_paths` (`None`) is `false`, matching JS `mle!==void 0`.
#[must_use]
fn server_hides_upgrade(info: &RateLimitInfo) -> bool {
    info.upgrade_paths
        .as_ref()
        .is_some_and(|paths| !paths.iter().any(|p| p == "upgrade_plan"))
        || flags::idle_amber_finch()
}

/// `serverHidesOverage` (`Tdo`'s `Bid = lly`, 2.1.206 binary):
/// `mle!==void 0&&!mle.includes("overage")` — an absent `upgrade_paths` is
/// `false`.
#[must_use]
fn server_hides_overage(info: &RateLimitInfo) -> bool {
    info.upgrade_paths
        .as_ref()
        .is_some_and(|paths| !paths.iter().any(|p| p == "overage"))
}

/// `spendLimitNudgePath` (`Tdo`'s `Bhs = gly`, 2.1.206 binary):
/// `Ze("tengu_pewter_summit",!1)&&!$id&&fit.overageDisabledReason==="org_level_disabled_until"&&mly&&qid`
/// — the spend-nudge flag AND non-team/enterprise AND the org-level
/// spend-cap disabled reason AND billing access AND the extra-usage command
/// enabled.
#[must_use]
fn spend_limit_nudge_path(
    info: &RateLimitInfo,
    sub: &SubscriptionSnapshot,
    extra_usage_cmd_enabled: bool,
) -> bool {
    flags::spend_limit_nudge_enabled()
        && !sub.is_team_or_enterprise()
        && info.overage_disabled_reason.as_deref() == Some("org_level_disabled_until")
        && sub.has_claude_ai_billing_access()
        && extra_usage_cmd_enabled
}

// ── 2.1.206 deep-internal predicates (Task 2) ────────────────────────────────
//
// These pin the leaves that gate three 2.1.206 branches:
//   (a) `Ucg`'s first guard  `!(ZA(t) && WBe() && !B5())`
//   (b) `a7n`'s per-model arm `O9e().includes(VQ(ei(t)))`
//   (c) `Gid`'s upsell gate   `shouldShowUpsell = Eyt() || Bo()`
// The subscription-consuming leaves (`B5`/`DBe`/`Bo` via `is_subscriber`,
// `x5` via `rate_limit_tier`) live on [`SubscriptionSnapshot`]. The
// model-based / flag-based / module-global leaves are pinned here. Each
// documented default follows the byte-locked convention used by
// [`upsell::OPENING_OPTIONS`]. SCOPE: every "collapses" / "never fires"
// observation below is about THIS task's three rate-limit-message branches
// only. These predicates have many OTHER 2.1.206 consumers (ZA ~18 sites,
// WBe ~7, B5 ~10, O9e ~4 — e.g. `WCt(limits, O9e())` filtering weekly-scoped
// model-limit rows, the `Qfi`/`Gfi` Fable annotations) that are out of scope
// here and where the same defaults are NOT no-ops; each must be analyzed
// independently.

/// `Eyt()` (2.1.206 binary @212992273: `function Eyt(){return!1}`) — a hard
/// `false` constant in 2.1.206. It is one input to `Gid`'s
/// `shouldShowUpsell = Eyt() || Bo()`; with `Eyt()==false`, `shouldShowUpsell`
/// reduces to `Bo()` = [`SubscriptionSnapshot::is_subscriber`].
const EYT: bool = false;

/// Composer default for `Rn()!=="firstParty"` inside `B5()`
/// ([`SubscriptionSnapshot::is_saffron_credits_only`]).
///
// 206 Rn(): the port is not Anthropic's first-party Claude Code binary and has
// no deployment discriminator wired into the subscription layer, so
// `deployment_first_party` defaults to `false` (⇒ `Rn()!=="firstParty"` is
// `true`). Byte-locked like the OPENING_OPTIONS note. For THIS task's `Ucg`
// branch the `B5()` result is masked because `overage_consent_required()`
// (WBe) already zeroes the `ZA&&WBe` factor; `B5()`'s other 2.1.206 consumers
// are out of scope (see the block header) and this default is not a no-op for
// them. `Rn()` itself (binary @212986032) is an env-derived deployment tag
// (gateway/bedrock/foundry/anthropicAws/mantle/vertex/firstParty); the
// composer intentionally does not thread that env state into this claude.ai
// saffron gate.
pub const DEPLOYMENT_FIRST_PARTY: bool = false;

/// `Gid` `shouldShowUpsell = Eyt() || Bo()` (2.1.206). `Bo()` (binary
/// @214252997: `bS() && GW(scopes)`) is the claude.ai-subscriber check =
/// [`SubscriptionSnapshot::is_subscriber`].
#[must_use]
pub fn should_show_upsell(sub: &SubscriptionSnapshot) -> bool {
    EYT || sub.is_subscriber
}

/// `WBe()` (2.1.206 binary @213282129: `function WBe(){return bIe()||Vqo()}`).
///
/// - `bIe()` (binary @213282164) reads the `tengu_saffron_lattice` flag config
///   via `n7m()`: `if(cfg.enabled===false) return false; return
///   cfg.overageConsentRequired===true || <planLimitsEndDate elapsed>`. The
///   port has no such flag config source and the flag is absent by default
///   (`enabled` unset) ⇒ `bIe()==false`.
/// - `Vqo()` (binary @210856387: `return Pt.fableCreditsRequired`) returns a
///   module-global set only by the (unported) fable-bridge consent dialog flow
///   ⇒ `false`.
///
// 206 WBe: `false` because the port has neither a `tengu_saffron_lattice` flag
// config nor fable-credits module state. For THIS task's `Ucg` first guard
// `!(ZA(t) && WBe() && !B5())`, a `false` WBe zeroes the `ZA&&WBe&&!B5`
// suppression factor, so the suppression path never triggers here and `ZA`/`B5`
// are not evaluated for this branch (the outer `return c` still fires on its
// own conditions — only the ZA/WBe/B5 suppression is inert). WBe has ~7 other
// 2.1.206 consumers (out of scope, see block header) where this default is NOT
// a no-op.
#[must_use]
pub fn overage_consent_required() -> bool {
    false
}

/// Minimal faithful port of `Ns(so(id))` model-id normalization used by `ZA`:
/// lowercases, drops a trailing `[1m]` context suffix, and strips any
/// provider-prefix path segment (`anthropic/claude-fable-5-1` → `claude-fable-5-1`).
fn normalize_model_id(model_id: &str) -> String {
    let lower = model_id.trim().to_lowercase();
    let no_suffix = lower.strip_suffix("[1m]").unwrap_or(&lower);
    no_suffix
        .rsplit('/')
        .next()
        .unwrap_or(no_suffix)
        .to_string()
}

/// `ZA(t)` (2.1.206 binary @213182958:
/// `Ns(so(e))==="claude-fable-5-1" || KQ(e)`) — is the model the Fable model.
/// `KQ` (binary @213182868) compares the normalized id to
/// `ANTHROPIC_DEFAULT_FABLE_MODEL`. In THIS task's `Ucg` first guard, `ZA(t)`
/// is ANDed with [`overage_consent_required`] (`WBe`, documented `false`), so
/// it has no effect on that branch; pinned faithfully. `ZA` has ~18 other
/// 2.1.206 consumers (out of scope, see block header) where it is live.
#[must_use]
pub fn is_fable_model(model_id: &str) -> bool {
    let norm = normalize_model_id(model_id);
    if norm == "claude-fable-5-1" {
        return true;
    }
    // KQ(e): `t = ANTHROPIC_DEFAULT_FABLE_MODEL; if(!t) return false;
    //         return Ns(e)===Ns(t)`.
    std::env::var("ANTHROPIC_DEFAULT_FABLE_MODEL")
        .ok()
        .filter(|t| !t.is_empty())
        .is_some_and(|t| normalize_model_id(&t) == norm)
}

/// `O9e()` (2.1.206 binary @217901083:
/// `Ze("tengu_usage_overage_included_models", []).filter(isString)`) — the set
/// of model DISPLAY NAMES for which `a7n` swaps to the
/// "Now using usage credits for `${model}`" copy
/// (`O9e().includes(VQ(ei(t)))`). `VQ(ei(t))` (binaries @213199290 / @213199704)
/// is the resolved model's `display_name`.
///
// 206 O9e: empty because the port has no `tengu_usage_overage_included_models`
// flag source (statsig default `[]`). With an empty set the membership test is
// always false ⇒ THIS task's per-model usage-credits arm (`a7n`) never fires.
// O9e has ~4 other 2.1.206 consumers (out of scope, e.g. `WCt(limits, O9e())`
// filtering weekly-scoped model-limit rows) where an empty set is NOT inert;
// analyze independently.
#[must_use]
pub fn overage_included_models() -> Vec<String> {
    Vec::new()
}

/// `O9e().includes(VQ(ei(t)))` — whether the resolved model's display name is in
/// the overage-included set. Always `false` in the port (empty set).
#[must_use]
pub fn model_in_overage_included_set(model_display_name: &str) -> bool {
    overage_included_models()
        .iter()
        .any(|m| m == model_display_name)
}

/// 2.1.206 claude.ai statsig flag gates consulted by the rate-limit message
/// composer (`tengu_pewter_summit`, `tengu_idle_amber_finch`,
/// `tengu_coral_beacon`). The port has no statsig backend and no realizable
/// claude.ai experiment state, so the statsig default (`false`) is
/// byte-faithful for every port state — same documented-`false` convention as
/// [`EYT`] / [`DEPLOYMENT_FIRST_PARTY`] / [`overage_consent_required`] above.
/// Each is kept as a named helper (rather than inlined `false`) so the branch
/// structure matches 2.1.206 and it's a single edit-point if a flag source is
/// ever wired.
pub mod flags {
    /// `tengu_pewter_summit` — spend-limit nudge gate. Statsig default
    /// `false`; no flag source in the port ⇒ always `false`.
    #[must_use]
    pub fn spend_limit_nudge_enabled() -> bool {
        false
    }

    /// `tengu_idle_amber_finch` (`Pee()`). Statsig default `false`; no flag
    /// source in the port ⇒ always `false`.
    #[must_use]
    pub fn idle_amber_finch() -> bool {
        false
    }

    /// `tengu_coral_beacon`. Statsig default `false`; no flag source in the
    /// port ⇒ always `false`.
    #[must_use]
    pub fn coral_beacon() -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Legacy shorthand: unknown subscription (default snapshot), extra-usage
    /// command disabled — the pre-batch-4 composer behaviour.
    fn compose(info: &RateLimitInfo) -> Option<ComposedRateLimit> {
        compose_with(info, &SubscriptionSnapshot::default(), false)
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
    /// `format_reset_time` stays on the time-only branch). std-only (no
    /// chrono dev-dependency in this crate).
    fn ts_in(delta: i64) -> u64 {
        let now = i64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("epoch")
                .as_secs(),
        )
        .unwrap();
        u64::try_from(now + delta).unwrap()
    }

    /// Expected `formatResetTime(ts, true)` output (showTimezone = true,
    /// showTime defaulted true) — computed through the SAME reused
    /// llm-runtime port so the assertions are deterministic.
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
        assert_eq!(compose(&RateLimitInfo::default()), None);
    }

    #[test]
    fn status_allowed_composes_nothing() {
        let info = RateLimitInfo {
            status: Some("allowed".into()),
            ..RateLimitInfo::default()
        };
        assert_eq!(compose(&info), None);
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
        assert_eq!(compose(&info), None);
    }

    #[test]
    fn rejected_with_overage_allowed_warning_warns_spending_limit() {
        // isUsingOverage + overageStatus allowed_warning → close-to-limit
        // warning (TS :53-58). 2.1.206 `Fdu` swapped the wording to
        // `You're close to your ${A5()?"usage limit":"usage credit
        // limit"}` (binary @217902246) — straight apostrophe, byte parity.
        // Default (non-usage-based) subscription → "usage credit limit".
        let info = RateLimitInfo {
            status: Some("rejected".into()),
            overage_status: Some("allowed_warning".into()),
            ..RateLimitInfo::default()
        };
        let got = compose(&info).unwrap();
        assert_eq!(got.text, "You're close to your usage credit limit");
        assert!(!got.text.contains('\u{2019}'));
        assert_eq!(got.upsell, None, "warnings carry no dim upsell line");
    }

    // ── rejected (error) branches (rateLimitMessages.ts:63-65, 143-197) ───

    #[test]
    fn rejected_five_hour_hits_session_limit_with_reset() {
        let ts = ts_in(3600);
        let got = compose(&rejected(Some("five_hour"), Some(ts))).unwrap();
        assert_eq!(
            got.text,
            format!("You've hit your session limit \u{b7} resets {}", reset(ts))
        );
        assert!(
            !got.text.contains('\u{2019}'),
            "TS uses straight apostrophes"
        );
    }

    #[test]
    fn rejected_seven_day_hits_weekly_limit() {
        let ts = ts_in(3600);
        let got = compose(&rejected(Some("seven_day"), Some(ts))).unwrap();
        assert_eq!(
            got.text,
            format!("You've hit your weekly limit \u{b7} resets {}", reset(ts))
        );
    }

    #[test]
    fn rejected_seven_day_opus_hits_opus_limit() {
        let got = compose(&rejected(Some("seven_day_opus"), None)).unwrap();
        assert_eq!(got.text, "You've hit your Opus limit");
    }

    #[test]
    fn rejected_seven_day_sonnet_hits_sonnet_limit() {
        // Unknown subscription → the non-pro/enterprise default 'Sonnet
        // limit' (TS :175-182; gap documented in the module docs).
        let got = compose(&rejected(Some("seven_day_sonnet"), None)).unwrap();
        assert_eq!(got.text, "You've hit your Sonnet limit");
    }

    #[test]
    fn rejected_unknown_type_hits_usage_limit() {
        let got = compose(&rejected(None, None)).unwrap();
        assert_eq!(got.text, "You've hit your usage limit");
    }

    #[test]
    fn rejected_ant_user_gets_plain_text_206() {
        // 2.1.206 `lhe` dropped the USER_TYPE === 'ant' #briarpatch-cc branch
        // entirely — the output is now plain regardless of user type.
        let got = compose(&rejected(Some("five_hour"), None)).unwrap();
        assert_eq!(got.text, "You've hit your session limit");
    }

    #[test]
    fn lhe_206_has_no_ant_branch() {
        assert_eq!(
            format_limit_reached_text("session limit", " · resets 3pm"),
            "You've hit your session limit · resets 3pm"
        );
        assert_eq!(
            format_limit_reached_text("weekly limit", ""),
            "You've hit your weekly limit"
        );
    }

    // ── rejected + overage rejected (TS :152-173) ─────────────────────────

    #[test]
    fn both_rejected_out_of_credits_personal_says_out_of_usage_credits() {
        // 2.1.206 `Ucg`: `overageDisabledReason==="out_of_credits"` with
        // `!A5()` (non-usage-based billing, the default snapshot) →
        // `` `You're out of usage credits${u}` `` — straight apostrophe,
        // replacing the pre-206 "You're out of extra usage" wording.
        //
        // rate_limit_type is left unmapped (None) here: the first guard
        // (`!r && overageDisabledReason && c && !Hqi.has(...)`) would
        // otherwise short-circuit on `c` (e.g. "session limit" for
        // "five_hour") BEFORE this `overageStatus === "rejected"` taxonomy is
        // ever reached — that guard only lets `r === false` cases through to
        // this block when `c` is null (an unmapped rate-limit type).
        let ts = ts_in(1800);
        let info = RateLimitInfo {
            overage_status: Some("rejected".into()),
            overage_disabled_reason: Some("out_of_credits".into()),
            overage_resets_at: Some(ts),
            ..rejected(None, None)
        };
        let got = compose(&info).unwrap();
        assert_eq!(
            got.text,
            format!("You're out of usage credits \u{b7} resets {}", reset(ts))
        );
        assert!(!got.text.contains('\u{2019}'));
    }

    #[test]
    fn both_rejected_without_reason_hits_plain_limit() {
        // overageStatus rejected, no disabled reason → the bare word
        // 'limit' (TS :172).
        let info = RateLimitInfo {
            overage_status: Some("rejected".into()),
            ..rejected(Some("five_hour"), None)
        };
        let got = compose(&info).unwrap();
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
        let got = compose(&info).unwrap();
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
        let got = compose(&info).unwrap();
        assert_eq!(
            got.text,
            format!("You've hit your limit \u{b7} resets {}", reset(early))
        );
    }

    // ── 2.1.206 `Ucg` usage-credits + org/seat/member/group taxonomy ──────
    //
    // Byte-verified against the real 2.1.206 binary: both the minified JS
    // source (`function Ucg(e,t){...}` @217902843) and the separate Latin-1
    // string table (@87692608) were decoded and cross-checked to agree
    // byte-for-byte, including the straight ASCII apostrophes and the
    // U+00B7 middot separators.

    fn usage_based(sub: SubscriptionSnapshot) -> SubscriptionSnapshot {
        SubscriptionSnapshot {
            billing_type: Some("usage_based".into()),
            ..sub
        }
    }

    #[test]
    fn out_of_credits_org_usage_based_with_billing_access_says_add_funds() {
        // `Ucg`: overageStatus rejected, reason out_of_credits, A5()==true
        // (usage-based billing) and tC()==true (billing access, e.g. an
        // admin) → "Your org is out of usage · add funds to continue".
        let sub = usage_based(SubscriptionSnapshot {
            is_subscriber: true,
            subscription_type: Some("team".into()),
            organization_role: Some("admin".into()),
            ..SubscriptionSnapshot::default()
        });
        let info = RateLimitInfo {
            overage_status: Some("rejected".into()),
            overage_disabled_reason: Some("out_of_credits".into()),
            ..rejected(Some("five_hour"), None)
        };
        let got = compose_with(&info, &sub, false).unwrap();
        assert_eq!(
            got.text,
            "Your org is out of usage \u{b7} add funds to continue"
        );
        assert!(!got.text.contains('\u{2019}'));
    }

    #[test]
    fn out_of_credits_org_usage_based_without_billing_access_says_contact_admin() {
        // Same branch, no billing access (e.g. a member role) →
        // "Your org is out of usage · contact your admin".
        let sub = usage_based(SubscriptionSnapshot {
            is_subscriber: true,
            subscription_type: Some("team".into()),
            organization_role: Some("member".into()),
            ..SubscriptionSnapshot::default()
        });
        let info = RateLimitInfo {
            overage_status: Some("rejected".into()),
            overage_disabled_reason: Some("out_of_credits".into()),
            ..rejected(Some("five_hour"), None)
        };
        let got = compose_with(&info, &sub, false).unwrap();
        assert_eq!(
            got.text,
            "Your org is out of usage \u{b7} contact your admin"
        );
    }

    #[test]
    fn seat_tier_disabled_personal_says_doesnt_include_usage_credits() {
        // `overageDisabledReason ∈ {seat_tier_level_disabled,
        // seat_tier_zero_credit_limit}` → `` `Your seat type doesn't include
        // ${r?"usage":"usage credits"}` ``. Default (non-usage-based)
        // subscription → "usage credits". rate_limit_type left unmapped
        // (None) so the first-guard `c` short-circuit doesn't intercept
        // before this taxonomy runs (see the out_of_credits test above).
        for reason in ["seat_tier_level_disabled", "seat_tier_zero_credit_limit"] {
            let info = RateLimitInfo {
                overage_status: Some("rejected".into()),
                overage_disabled_reason: Some(reason.into()),
                ..rejected(None, None)
            };
            let got = compose(&info).unwrap();
            assert_eq!(got.text, "Your seat type doesn't include usage credits");
        }
    }

    #[test]
    fn seat_tier_disabled_usage_based_says_doesnt_include_usage() {
        // Same reasons, A5()==true (usage-based billing) → "usage" (no
        // "credits" suffix).
        let sub = usage_based(SubscriptionSnapshot::default());
        let info = RateLimitInfo {
            overage_status: Some("rejected".into()),
            overage_disabled_reason: Some("seat_tier_zero_credit_limit".into()),
            ..rejected(Some("five_hour"), None)
        };
        let got = compose_with(&info, &sub, false).unwrap();
        assert_eq!(got.text, "Your seat type doesn't include usage");
    }

    #[test]
    fn org_service_level_disabled_says_service_disabled() {
        // rate_limit_type unmapped (None) so the first guard doesn't
        // intercept (see the out_of_credits test above).
        let info = RateLimitInfo {
            overage_status: Some("rejected".into()),
            overage_disabled_reason: Some("org_service_level_disabled".into()),
            ..rejected(None, None)
        };
        let got = compose(&info).unwrap();
        assert_eq!(got.text, "This service is disabled for your org");
    }

    #[test]
    fn member_disabled_reasons_say_allocation_disabled_by_admin() {
        // rate_limit_type unmapped (None) so the first guard doesn't
        // intercept (see the out_of_credits test above).
        for reason in ["member_level_disabled", "member_zero_credit_limit"] {
            let info = RateLimitInfo {
                overage_status: Some("rejected".into()),
                overage_disabled_reason: Some(reason.into()),
                ..rejected(None, None)
            };
            let got = compose(&info).unwrap();
            assert_eq!(
                got.text,
                "Your usage allocation has been disabled by your admin \u{b7} run /usage-credits to ask your admin for a higher limit"
            );
        }
    }

    #[test]
    fn group_zero_credit_limit_says_group_limit_set_to_zero() {
        // rate_limit_type unmapped (None) so the first guard doesn't
        // intercept (see the out_of_credits test above).
        let info = RateLimitInfo {
            overage_status: Some("rejected".into()),
            overage_disabled_reason: Some("group_zero_credit_limit".into()),
            ..rejected(None, None)
        };
        let got = compose(&info).unwrap();
        assert_eq!(
            got.text,
            "Your group's usage limit is set to $0 \u{b7} run /usage-credits to ask your admin for a higher limit"
        );
    }

    #[test]
    fn hqi_team_enterprise_spend_limit_by_billing_access() {
        // Hqi reasons (org_level_disabled_until / org_spend_cap_reached),
        // NOT inside the overage-rejected branch (overageStatus absent) —
        // `Fs()` team/enterprise → "org's monthly spend limit", suffix gated
        // on `tC()` (billing access): admin → the "raise it, or visit
        // admin-settings" copy; member → the "ask your admin" copy.
        for reason in ["org_level_disabled_until", "org_spend_cap_reached"] {
            let admin = SubscriptionSnapshot {
                is_subscriber: true,
                subscription_type: Some("team".into()),
                organization_role: Some("admin".into()),
                ..SubscriptionSnapshot::default()
            };
            let info = RateLimitInfo {
                overage_disabled_reason: Some(reason.into()),
                ..rejected(Some("five_hour"), None)
            };
            let got = compose_with(&info, &admin, false).unwrap();
            assert_eq!(
                got.text,
                "You've hit your org's monthly spend limit \u{b7} run /usage-credits to raise it, or visit claude.ai/admin-settings/usage"
            );

            let member = SubscriptionSnapshot {
                organization_role: Some("member".into()),
                ..admin
            };
            let got = compose_with(&info, &member, false).unwrap();
            assert_eq!(
                got.text,
                "You've hit your org's monthly spend limit \u{b7} run /usage-credits to ask your admin for a higher limit"
            );
        }
    }

    #[test]
    fn hqi_personal_spend_limit_by_billing_access() {
        // Same Hqi reasons, non-team/enterprise `Fs()` (e.g. pro, or unknown)
        // → `tC()` (billing access) selects "monthly spend limit" (raise it
        // yourself) vs "org's monthly spend limit" (ask your admin).
        let billed = SubscriptionSnapshot {
            is_subscriber: true,
            subscription_type: Some("pro".into()),
            ..SubscriptionSnapshot::default()
        };
        let info = RateLimitInfo {
            overage_disabled_reason: Some("org_level_disabled_until".into()),
            ..rejected(Some("five_hour"), None)
        };
        let got = compose_with(&info, &billed, false).unwrap();
        assert_eq!(
            got.text,
            "You've hit your monthly spend limit \u{b7} raise it at claude.ai/settings/usage"
        );

        // Unknown/no billing access (default snapshot) → the org-facing copy.
        let got = compose(&info).unwrap();
        assert_eq!(
            got.text,
            "You've hit your org's monthly spend limit \u{b7} ask your admin to raise it at claude.ai/settings/usage"
        );
    }

    #[test]
    fn hqi_inside_overage_rejected_reads_org_monthly_usage_limit() {
        // The Hqi arm NESTED inside `overageStatus==="rejected"` is reachable
        // only when `A5()` (`r`) is true — the outer `!r && Hqi.has(...)`
        // check above always intercepts non-usage-based cases first.
        let sub = usage_based(SubscriptionSnapshot::default());
        let ts = ts_in(3600);
        let info = RateLimitInfo {
            overage_status: Some("rejected".into()),
            overage_disabled_reason: Some("org_spend_cap_reached".into()),
            overage_resets_at: Some(ts),
            ..rejected(Some("five_hour"), None)
        };
        let got = compose_with(&info, &sub, false).unwrap();
        assert_eq!(
            got.text,
            format!(
                "You've hit your org's monthly usage limit \u{b7} resets {}",
                reset(ts)
            )
        );

        // No overageResetsAt → no reset suffix.
        let info_no_reset = RateLimitInfo {
            overage_resets_at: None,
            ..info
        };
        let got = compose_with(&info_no_reset, &sub, false).unwrap();
        assert_eq!(got.text, "You've hit your org's monthly usage limit");
    }

    #[test]
    fn final_fallback_usage_limit_by_billing_and_reset() {
        // Bottom of `Ucg`: no `c` (unmapped rate-limit type), no overage
        // rejection, no Hqi reason → `r ? lhe("usage limit", o) :
        // lhe("usage limit", l)`.
        let usage_billed_admin = usage_based(SubscriptionSnapshot {
            is_subscriber: true,
            subscription_type: Some("team".into()),
            organization_role: Some("admin".into()),
            ..SubscriptionSnapshot::default()
        });
        let info = rejected(Some("overage"), None); // "overage" is unmapped by qcg.
        let got = compose_with(&info, &usage_billed_admin, false).unwrap();
        assert_eq!(got.text, "You've hit your usage limit");

        let usage_billed_member = SubscriptionSnapshot {
            organization_role: Some("member".into()),
            ..usage_billed_admin
        };
        let got = compose_with(&info, &usage_billed_member, false).unwrap();
        assert_eq!(
            got.text,
            "You've hit your usage limit \u{b7} contact your admin to increase it"
        );

        // Non-usage-based, with a reset time → the `l` (reset_message) arm.
        let ts = ts_in(1800);
        let info_with_reset = rejected(Some("overage"), Some(ts));
        let got = compose(&info_with_reset).unwrap();
        assert_eq!(
            got.text,
            format!("You've hit your usage limit \u{b7} resets {}", reset(ts))
        );
    }

    #[test]
    fn first_guard_c_passthrough_bypasses_overage_rejected_branch() {
        // The first guard (`!r && overageDisabledReason && c &&
        // !Hqi.has(...)`) must intercept BEFORE the
        // `overageStatus === "rejected"` block — proven by giving both an
        // opus rate-limit type (so `c` = "You've hit your Opus limit") AND
        // overageStatus "rejected" with a non-taxonomy disabled reason (so,
        // absent the guard, control would instead fall into the rejected
        // block and produce the very different "You've hit your limit").
        let info = RateLimitInfo {
            overage_status: Some("rejected".into()),
            overage_disabled_reason: Some("some_unrecognized_reason".into()),
            ..rejected(Some("seven_day_opus"), None)
        };
        let got = compose(&info).unwrap();
        assert_eq!(got.text, "You've hit your Opus limit");
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
        let got = compose(&rejected(Some("five_hour"), None)).unwrap();
        assert_eq!(got.upsell, None);
    }

    #[test]
    fn rejected_unknown_subscription_with_overage_header_has_no_upsell() {
        // Pre-batch-4 the overage-status header proxied a generic
        // team/enterprise fallthrough upsell; the real gate is the
        // subscriber check (TSX :26 + :78); the header no longer drives the
        // upsell.
        let info = RateLimitInfo {
            overage_status: Some("rejected".into()),
            ..rejected(Some("five_hour"), None)
        };
        let got = compose(&info).unwrap();
        assert_eq!(got.upsell, None);
    }

    #[test]
    fn unknown_subscription_keeps_rev28_copy() {
        // Default snapshot reproduces the rev2.8 (batch-3) composition:
        // non-pro 'Sonnet limit' naming and no error upsell.
        let got = compose(&rejected(Some("seven_day_sonnet"), None)).unwrap();
        assert_eq!(got.text, "You've hit your Sonnet limit");
        assert_eq!(got.upsell, None);
    }

    #[test]
    fn pro_subscriber_sonnet_limit_reads_weekly_limit() {
        // "For pro and enterprise, Sonnet limit is the same as weekly" —
        // rateLimitMessages.ts:175-182.
        let got = compose_with(&rejected(Some("seven_day_sonnet"), None), &pro(), false).unwrap();
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
            &enterprise,
            false,
        )
        .unwrap();
        assert_eq!(got.text, "You've hit your weekly limit");
    }

    /// Builds the 9 `Gid` inputs the same way `compose_with` derives them
    /// from a subscription + the extra-usage-command flag, with both
    /// `RateLimitInfo`-sourced server-hide fields defaulted `false` (no
    /// `upgrade_paths`) and `spend_limit_nudge_path` defaulted `false` — the
    /// tests below override individual fields with struct-update syntax to
    /// pin the branches those fields gate.
    fn inputs(sub: &SubscriptionSnapshot, extra_usage_cmd_enabled: bool) -> UpsellInputs {
        UpsellInputs {
            should_show_upsell: should_show_upsell(sub),
            is_max20x: sub.is_max20x(),
            is_extra_usage_command_enabled: extra_usage_cmd_enabled,
            should_auto_open_rate_limit_options_menu: false,
            is_team_or_enterprise: sub.is_team_or_enterprise(),
            has_billing_access: sub.has_claude_ai_billing_access(),
            server_hides_upgrade: false,
            server_hides_overage: false,
            spend_limit_nudge_path: false,
        }
    }

    #[test]
    fn gid_not_a_subscriber_returns_none() {
        // `Gid` :`if(!e)return null` — shouldShowUpsell gates everything
        // else, including the auto-open-menu and spend-nudge arms that
        // otherwise run BEFORE it in source order.
        assert_eq!(
            error_upsell(&UpsellInputs {
                should_show_upsell: false,
                should_auto_open_rate_limit_options_menu: true,
                spend_limit_nudge_path: true,
                ..UpsellInputs::default()
            }),
            None
        );
    }

    #[test]
    fn gid_auto_open_menu_returns_opening_options() {
        // `Gid`: `if(n)return"Opening your options…"` — checked before the
        // spend-nudge arm. Structurally unreachable from `compose_with`
        // (always passes `false`), but `Gid` itself must still honor it.
        assert_eq!(
            error_upsell(&UpsellInputs {
                should_show_upsell: true,
                should_auto_open_rate_limit_options_menu: true,
                spend_limit_nudge_path: true,
                ..UpsellInputs::default()
            })
            .as_deref(),
            Some(upsell::OPENING_OPTIONS)
        );
    }

    #[test]
    fn gid_spend_limit_nudge_path_returns_adjust_spend_limit() {
        // `Gid`: `if(l)return"/usage-credits to adjust your monthly spend
        // limit."` — tested by passing `spendLimitNudgePath` directly as an
        // input, since the real `tengu_pewter_summit` flag
        // ([`flags::spend_limit_nudge_enabled`]) is documented `false` in
        // this port and can never drive this branch through `compose_with`.
        assert_eq!(
            error_upsell(&UpsellInputs {
                should_show_upsell: true,
                spend_limit_nudge_path: true,
                ..UpsellInputs::default()
            })
            .as_deref(),
            Some(upsell::SPEND_LIMIT_NUDGE)
        );
    }

    #[test]
    fn max20x_error_upsell_login_switch_without_extra_usage_cmd() {
        // `Gid`: `if(t){if(c)return FINISH; return"/login to switch..."}`,
        // `c = false` (extra-usage command disabled).
        assert_eq!(
            error_upsell(&inputs(&max20x(), false)).as_deref(),
            Some(upsell::LOGIN_SWITCH)
        );
    }

    #[test]
    fn max20x_error_upsell_usage_credits_finish_with_cmd() {
        // Max-20x with the extra-usage command enabled → `c = true`.
        assert_eq!(
            error_upsell(&inputs(&max20x(), true)).as_deref(),
            Some(upsell::USAGE_CREDITS_FINISH)
        );
    }

    #[test]
    fn pro_error_upsell_upgrade_without_cmd() {
        // `Gid`: non-team, `serverHidesUpgrade` false, `!c` → UPGRADE.
        assert_eq!(
            error_upsell(&inputs(&pro(), false)).as_deref(),
            Some(upsell::UPGRADE)
        );
    }

    #[test]
    fn pro_with_cmd_enabled_falls_through_to_upgrade_or_usage_credits() {
        // A pro user WITH the command enabled skips the team block and
        // lands on the final fallthrough — `c = true` → UPGRADE_OR_USAGE_CREDITS.
        assert_eq!(
            error_upsell(&inputs(&pro(), true)).as_deref(),
            Some(upsell::UPGRADE_OR_USAGE_CREDITS)
        );
    }

    #[test]
    fn team_error_upsell_admin_enable_without_cmd_regardless_of_role() {
        // `Gid`: `if(o){if(!c)return"Your admin can enable extra usage
        // ..."; ...}` — the `!c` check runs BEFORE the `hasBillingAccess`
        // (`i`) check, so admin and member alike get the admin-enable copy
        // when the extra-usage command is disabled. This is a REAL 206
        // behavior change: pre-206 this arm returned `null`.
        assert_eq!(
            error_upsell(&inputs(&team(false, Some("admin")), false)).as_deref(),
            Some(upsell::USAGE_CREDITS_ADMIN_ENABLE)
        );
        assert_eq!(
            error_upsell(&inputs(&team(false, Some("member")), false)).as_deref(),
            Some(upsell::USAGE_CREDITS_ADMIN_ENABLE)
        );
    }

    #[test]
    fn team_error_upsell_finish_vs_request_admin_with_cmd_by_billing_access() {
        // `Gid`: `if(i)return FINISH; return REQUEST_ADMIN` — `c = true`
        // (extra-usage command enabled), billing access picks FINISH,
        // member picks REQUEST_ADMIN.
        assert_eq!(
            error_upsell(&inputs(&team(false, Some("admin")), true)).as_deref(),
            Some(upsell::USAGE_CREDITS_FINISH)
        );
        assert_eq!(
            error_upsell(&inputs(&team(false, Some("member")), true)).as_deref(),
            Some(upsell::USAGE_CREDITS_REQUEST_ADMIN)
        );
        // Enterprise rides the same is_team_or_enterprise() predicate; pin
        // one variant so the arm isn't team-only-tested.
        let enterprise = SubscriptionSnapshot {
            subscription_type: Some("enterprise".into()),
            ..team(false, Some("member"))
        };
        assert_eq!(
            error_upsell(&inputs(&enterprise, true)).as_deref(),
            Some(upsell::USAGE_CREDITS_REQUEST_ADMIN)
        );
    }

    #[test]
    fn gid_server_hides_upgrade_finish_when_c_else_none() {
        // `Gid`: `if(s){if(c)return FINISH; return null}` — non-team,
        // `serverHidesUpgrade` true.
        let with_cmd = UpsellInputs {
            server_hides_upgrade: true,
            ..inputs(&pro(), true)
        };
        assert_eq!(
            error_upsell(&with_cmd).as_deref(),
            Some(upsell::USAGE_CREDITS_FINISH)
        );
        let without_cmd = UpsellInputs {
            server_hides_upgrade: true,
            ..inputs(&pro(), false)
        };
        assert_eq!(error_upsell(&without_cmd), None);
    }

    // ── server-hides-* / spend-nudge derivations (`Tdo`, 2.1.206 binary) ──

    #[test]
    fn server_hides_overage_derives_from_upgrade_paths_membership() {
        // `mle!==void 0&&!mle.includes("overage")`.
        assert!(
            !server_hides_overage(&RateLimitInfo::default()),
            "absent upgrade_paths"
        );
        assert!(!server_hides_overage(&RateLimitInfo {
            upgrade_paths: Some(vec!["overage".into()]),
            ..RateLimitInfo::default()
        }));
        assert!(server_hides_overage(&RateLimitInfo {
            upgrade_paths: Some(vec!["upgrade_plan".into()]),
            ..RateLimitInfo::default()
        }));
        assert!(server_hides_overage(&RateLimitInfo {
            upgrade_paths: Some(vec![]),
            ..RateLimitInfo::default()
        }));
    }

    #[test]
    fn server_hides_upgrade_derives_from_upgrade_paths_membership() {
        // `mle!==void 0&&!mle.includes("upgrade_plan")` OR `Pee()`
        // (idle_amber_finch — documented false, so this reduces to the
        // upgrade_paths membership test alone in this port).
        assert!(
            !server_hides_upgrade(&RateLimitInfo::default()),
            "absent upgrade_paths"
        );
        assert!(!server_hides_upgrade(&RateLimitInfo {
            upgrade_paths: Some(vec!["upgrade_plan".into()]),
            ..RateLimitInfo::default()
        }));
        assert!(server_hides_upgrade(&RateLimitInfo {
            upgrade_paths: Some(vec!["overage".into()]),
            ..RateLimitInfo::default()
        }));
    }

    #[test]
    fn spend_limit_nudge_path_predicate_requires_every_conjunct() {
        // `Ze("tengu_pewter_summit",!1)&&!$id&&reason==="org_level_disabled_until"&&mly&&qid`.
        // The real flag ([`flags::spend_limit_nudge_enabled`]) is documented
        // `false` in this port, so the full predicate is always `false`
        // through `compose_with` EVEN when every other conjunct holds.
        assert!(!flags::spend_limit_nudge_enabled());
        let billed_personal = SubscriptionSnapshot {
            is_subscriber: true,
            subscription_type: Some("pro".into()),
            organization_role: Some("admin".into()),
            ..SubscriptionSnapshot::default()
        };
        let info = RateLimitInfo {
            overage_disabled_reason: Some("org_level_disabled_until".into()),
            ..RateLimitInfo::default()
        };
        assert!(!spend_limit_nudge_path(&info, &billed_personal, true));
        // Wrong reason, team/enterprise, or the cmd disabled each
        // independently keep it false too (exercising the AND chain).
        assert!(!spend_limit_nudge_path(
            &RateLimitInfo::default(),
            &billed_personal,
            true
        ));
        assert!(!spend_limit_nudge_path(
            &info,
            &team(false, Some("admin")),
            true
        ));
        assert!(!spend_limit_nudge_path(&info, &billed_personal, false));
    }

    // ── `jid` upsell-suppression gate (2.1.206 `Tdo`) ──────────────────────

    #[test]
    fn jid_seven_day_overage_included_suppresses_upsell_end_to_end() {
        // `jid = rateLimitType==="seven_day_overage_included" ||
        // errorCode==="credits_required"` hard-nulls the upsell BEFORE `Gid`
        // ever runs, regardless of subscription state.
        let info = rejected(Some("seven_day_overage_included"), None);
        let got = compose_with(&info, &pro(), true).unwrap();
        assert_eq!(got.upsell, None);
    }

    #[test]
    fn jid_credits_required_suppresses_upsell_end_to_end() {
        let info = RateLimitInfo {
            credits_required: true,
            ..rejected(Some("five_hour"), None)
        };
        let got = compose_with(&info, &pro(), true).unwrap();
        assert_eq!(got.upsell, None);
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
        assert_eq!(compose(&info), None);
    }

    #[test]
    fn warning_at_threshold_composes() {
        // TS gate is `utilization < WARNING_THRESHOLD` → exactly 0.7 warns.
        let ts = ts_in(3600);
        let got = compose(&warning(Some("seven_day"), Some(0.7), Some(ts))).unwrap();
        assert_eq!(
            got.text,
            format!(
                "You've used 70% of your weekly limit \u{b7} resets {}",
                reset(ts)
            )
        );
        assert_eq!(got.upsell, None);
    }

    #[test]
    fn warning_used_percentage_floors() {
        // Math.floor(0.857 * 100) = 85 (TS :223).
        let got = compose(&warning(Some("seven_day"), Some(0.857), None)).unwrap();
        assert_eq!(got.text, "You've used 85% of your weekly limit");
    }

    #[test]
    fn warning_without_utilization_says_approaching() {
        // TS only short-circuits when utilization !== undefined (:73-78);
        // absent utilization falls through to the 'Approaching' copy (:247-253).
        let ts = ts_in(3600);
        let got = compose(&warning(Some("five_hour"), None, Some(ts))).unwrap();
        assert_eq!(
            got.text,
            format!("Approaching session limit \u{b7} resets {}", reset(ts))
        );
    }

    #[test]
    fn warning_overage_type_appends_limit_to_approaching() {
        // 206 `jcg`: `t=A5()?"usage limit":"usage credit limit"` on the
        // Approaching path. Default (unknown) subscription → A5()==false →
        // "usage credit limit" (was "extra usage limit" pre-206).
        let got = compose(&warning(Some("overage"), None, None)).unwrap();
        assert_eq!(got.text, "Approaching usage credit limit");
    }

    #[test]
    fn warning_overage_type_with_utilization_keeps_extra_usage() {
        // 206 `jcg`: `t=A5()?"usage":"usage credits"` pre-Approaching.
        // Default (unknown) subscription → A5()==false → "usage credits"
        // (was "extra usage" pre-206).
        let got = compose(&warning(Some("overage"), Some(0.9), None)).unwrap();
        assert_eq!(got.text, "You've used 90% of your usage credits");
    }

    #[test]
    fn jcg_overage_usage_based_billing_says_usage_and_suppresses_reset() {
        // 206: A5()==true (usage-based billing) → "usage" naming, and the
        // reset time is suppressed (`n=rateLimitType==="overage"&&A5()`)
        // even though resetsAt is present.
        let sub = SubscriptionSnapshot {
            billing_type: Some("usage_based".into()),
            ..SubscriptionSnapshot::default()
        };
        let ts = ts_in(3600);
        let got =
            compose_with(&warning(Some("overage"), Some(0.9), Some(ts)), &sub, false).unwrap();
        assert_eq!(got.text, "You've used 90% of your usage");
    }

    #[test]
    fn jcg_overage_approaching_usage_based_says_usage_limit() {
        let sub = SubscriptionSnapshot {
            billing_type: Some("usage_based".into()),
            ..SubscriptionSnapshot::default()
        };
        let got = compose_with(&warning(Some("overage"), None, None), &sub, false).unwrap();
        assert_eq!(got.text, "Approaching usage limit");
    }

    #[test]
    fn jcg_seven_day_overage_included_reads_fable_5_limit() {
        // NEW in 206: `case"seven_day_overage_included":t="Fable 5 limit"`.
        let got = compose(&warning(
            Some("seven_day_overage_included"),
            Some(0.75),
            None,
        ))
        .unwrap();
        assert_eq!(got.text, "You've used 75% of your Fable 5 limit");
    }

    #[test]
    fn warning_zero_utilization_composes_nothing() {
        // `0 !== undefined` and `0 < 0.7` → the TS threshold guard returns
        // null (TS :73-78).
        let info = warning(Some("seven_day"), Some(0.0), Some(ts_in(3600)));
        assert_eq!(compose(&info), None);
    }

    #[test]
    fn warning_unknown_type_composes_nothing() {
        // TS getEarlyWarningText returns null for `undefined` (:217-218);
        // unknown strings can't occur in the typed union — treated alike.
        assert_eq!(compose(&warning(None, Some(0.9), None)), None);
    }

    // ── team/enterprise warning suppression (TS :80-94) ───────────────────

    #[test]
    fn team_with_extra_usage_and_no_billing_access_suppresses_warning() {
        // "Don't warn non-billing Team/Enterprise users about approaching
        // plan limits if overages are enabled" — rateLimitMessages.ts:80-94.
        // Role None → no billing access.
        let info = warning(Some("five_hour"), Some(0.8), Some(ts_in(3600)));
        assert_eq!(compose_with(&info, &team(true, None), false), None);
    }

    #[test]
    fn team_admin_still_sees_warning() {
        // Billing access (admin role) defeats the suppression (TS :91).
        let ts = ts_in(3600);
        let info = warning(Some("five_hour"), Some(0.8), Some(ts));
        let got = compose_with(&info, &team(true, Some("admin")), false).unwrap();
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
        let got = compose_with(&info, &pro(), false).unwrap();
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
        let got = compose_with(&info, &max, false).unwrap();
        assert!(got.text.ends_with(" \u{b7} /upgrade to keep using LingXi"));
    }

    #[test]
    fn five_hour_warning_team_without_extra_usage_appends_request_upsell() {
        // 206 `Wcg`: team/enterprise, !hasExtraUsageEnabled &&
        // isOverageProvisioningAllowed() (Stripe) → billing-access-gated
        // "Run /usage-credits to …" copy. Admin role → hasBillingAccess ==
        // true → the "turn on … for your org" wording (was
        // '/extra-usage to request more' pre-206).
        let got = compose_with(
            &warning(Some("five_hour"), Some(0.8), None),
            &team(false, Some("admin")),
            false,
        )
        .unwrap();
        assert_eq!(
            got.text,
            "You've used 80% of your session limit \u{b7} Run /usage-credits to turn on extra usage for your org"
        );
    }

    #[test]
    fn five_hour_warning_team_member_without_billing_access_asks_admin() {
        // Same branch, member role → hasBillingAccess == false → the shared
        // "ask your admin for more" wording.
        let got = compose_with(
            &warning(Some("five_hour"), Some(0.8), None),
            &team(false, Some("member")),
            false,
        )
        .unwrap();
        assert_eq!(
            got.text,
            "You've used 80% of your session limit \u{b7} Run /usage-credits to ask your admin for more"
        );
    }

    #[test]
    fn five_hour_warning_team_with_extra_usage_has_no_upsell() {
        // "Teams/Enterprise with overages enabled ... don't need upsell" —
        // TS :276-277. Admin role so the :80-94 suppression doesn't apply.
        let got = compose_with(
            &warning(Some("five_hour"), Some(0.8), None),
            &team(true, Some("admin")),
            false,
        )
        .unwrap();
        assert_eq!(got.text, "You've used 80% of your session limit");
    }

    #[test]
    fn weekly_warning_never_has_upsell() {
        // "Weekly limit warnings don't show upsell per spec" — TS :295-296.
        let got =
            compose_with(&warning(Some("seven_day"), Some(0.8), None), &pro(), false).unwrap();
        assert_eq!(got.text, "You've used 80% of your weekly limit");
    }

    #[test]
    fn overage_warning_team_without_extra_usage_appends_request_upsell() {
        // 206: the overage-name Approaching reassignment (`t=A5()?"usage
        // limit":"usage credit limit"`) happens BEFORE the `Wcg` upsell is
        // appended. team() billing_type is stripe (not usage-based) → A5()
        // == false → "usage credit limit"; admin role → billing-access-gated
        // "turn on … for your org" (was '/extra-usage to request more'
        // pre-206, on the old 'extra usage limit' wording).
        let got = compose_with(
            &warning(Some("overage"), None, None),
            &team(false, Some("admin")),
            false,
        )
        .unwrap();
        assert_eq!(
            got.text,
            "Approaching usage credit limit \u{b7} Run /usage-credits to turn on extra usage for your org"
        );
    }

    #[test]
    fn overage_warning_team_with_extra_usage_raises_cap_upsell() {
        // 206 `Wcg` second team/enterprise branch: hasExtraUsageEnabled &&
        // rateLimitType=="overage" → billing-access-gated "raise the cap".
        // Only the admin (billing-access) variant is reachable through the
        // full composer here: the TS :80-94 suppression
        // (`isTeamOrEnterprise && hasExtraUsageEnabled &&
        // !hasClaudeAiBillingAccess`) fires FIRST for any non-billing team
        // member with extra usage enabled and returns null before `jcg`/`Wcg`
        // ever run (see `team_with_extra_usage_and_no_billing_access_suppresses_warning`);
        // the member/"ask your admin for more" arm of `Wcg` itself is pinned
        // directly in `wcg_team_raise_cap_vs_ask_admin_by_billing_access`.
        let got = compose_with(
            &warning(Some("overage"), None, None),
            &team(true, Some("admin")),
            false,
        )
        .unwrap();
        assert_eq!(
            got.text,
            "Approaching usage credit limit \u{b7} Run /usage-credits to raise the cap"
        );
    }

    // ── getUsingOverageText / `a7n` (2.1.206 binary @217907540) ───────────
    //
    // `using_overage_text` reads only `rate_limit_type`/`resets_at` plus the
    // subscription, so the `rejected(...)` constructor doubles as its input.
    // 206 rewrote the copy from "extra usage" to `A5()`-gated "usage
    // credits"/"your usage allocation" and suppresses the reset suffix
    // outright when usage-based billing (`A5()`) is true.

    #[test]
    fn using_overage_text_per_limit_type() {
        let unknown = SubscriptionSnapshot::default();
        let ts = ts_in(3600);
        // five_hour → 'session limit'; separator placement is
        // ` · Your {limitName} resets {resetTime}`, U+00B7.
        assert_eq!(
            using_overage_text(&rejected(Some("five_hour"), Some(ts)), &unknown),
            format!(
                "You're now using usage credits \u{b7} Your session limit resets {}",
                reset(ts)
            )
        );
        // seven_day → 'weekly limit'.
        assert_eq!(
            using_overage_text(&rejected(Some("seven_day"), Some(ts)), &unknown),
            format!(
                "You're now using usage credits \u{b7} Your weekly limit resets {}",
                reset(ts)
            )
        );
        // seven_day_opus → 'Opus limit'.
        assert_eq!(
            using_overage_text(&rejected(Some("seven_day_opus"), Some(ts)), &unknown),
            format!(
                "You're now using usage credits \u{b7} Your Opus limit resets {}",
                reset(ts)
            )
        );
        // seven_day_sonnet: "For pro and enterprise, Sonnet limit is the same
        // as weekly"; everyone else keeps 'Sonnet limit'.
        assert_eq!(
            using_overage_text(&rejected(Some("seven_day_sonnet"), Some(ts)), &pro()),
            format!(
                "You're now using usage credits \u{b7} Your weekly limit resets {}",
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
                "You're now using usage credits \u{b7} Your weekly limit resets {}",
                reset(ts)
            )
        );
        assert_eq!(
            using_overage_text(&rejected(Some("seven_day_sonnet"), Some(ts)), &unknown),
            format!(
                "You're now using usage credits \u{b7} Your Sonnet limit resets {}",
                reset(ts)
            )
        );
        // No limit type → the bare "Now using {i}" copy, EVEN with resetsAt
        // set: the JS checks `!n` before the reset message is ever built.
        assert_eq!(
            using_overage_text(&rejected(None, Some(ts)), &unknown),
            "Now using usage credits"
        );
        // Usage-based billing, no limit type → "your usage allocation".
        assert_eq!(
            using_overage_text(&rejected(None, Some(ts)), &usage_based(unknown.clone())),
            "Now using your usage allocation"
        );
        // five_hour without a reset → no ` · Your …` suffix (empty resetTime
        // ⇒ empty resetMessage).
        assert_eq!(
            using_overage_text(&rejected(Some("five_hour"), None), &unknown),
            "You're now using usage credits"
        );
    }

    #[test]
    fn using_overage_text_usage_based_billing_suppresses_reset() {
        // 206: `s = r && !o ? " · Your {n} resets {r}" : ""` — when `o`
        // (usage-based billing / A5()) is true, the reset suffix is
        // suppressed even though a reset time IS present.
        let ts = ts_in(3600);
        let sub = usage_based(SubscriptionSnapshot::default());
        assert_eq!(
            using_overage_text(&rejected(Some("seven_day"), Some(ts)), &sub),
            "You're now using your usage allocation"
        );
    }

    #[test]
    fn using_overage_text_is_straight_ascii() {
        // TS uses straight ASCII apostrophes (U+0027), never the curly
        // U+2019; the only non-ASCII byte allowed is the U+00B7 separator.
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
        // 2.1.206 `Wcg` copy uses plain ASCII (no curly apostrophes, unlike
        // the TSX getUpsellMessage strings), byte-verified against the real
        // binary @217906477-217906649.
        assert_eq!(
            USAGE_CREDITS_TURN_ON,
            "Run /usage-credits to turn on extra usage for your org"
        );
        assert_eq!(
            USAGE_CREDITS_ASK_ADMIN,
            "Run /usage-credits to ask your admin for more"
        );
        assert_eq!(
            USAGE_CREDITS_RAISE_CAP,
            "Run /usage-credits to raise the cap"
        );
        assert_eq!(UPGRADE_KEEP_USING, "/upgrade to keep using LingXi");
        for s in [
            USAGE_CREDITS_TURN_ON,
            USAGE_CREDITS_ASK_ADMIN,
            USAGE_CREDITS_RAISE_CAP,
            UPGRADE_KEEP_USING,
        ] {
            assert!(s.is_ascii(), "warning upsell copy must be straight ASCII");
            assert!(!s.contains('\u{2019}'));
        }
    }

    // ── warning_upsell (Wcg) direct unit tests ────────────────────────────

    #[test]
    fn wcg_team_turn_on_vs_ask_admin_by_billing_access() {
        // Team/enterprise, !hasExtraUsageEnabled && isOverageProvisioningAllowed():
        // hasBillingAccess (admin) → "turn on … for your org"; without it
        // (member) → the shared "ask your admin for more".
        assert_eq!(
            warning_upsell(Some("five_hour"), &team(false, Some("admin"))),
            Some(USAGE_CREDITS_TURN_ON)
        );
        assert_eq!(
            warning_upsell(Some("five_hour"), &team(false, Some("member"))),
            Some(USAGE_CREDITS_ASK_ADMIN)
        );
    }

    #[test]
    fn wcg_team_raise_cap_vs_ask_admin_by_billing_access() {
        // Team/enterprise, hasExtraUsageEnabled && rateLimitType=="overage":
        // hasBillingAccess (admin) → "raise the cap"; without it (member) →
        // the shared "ask your admin for more".
        assert_eq!(
            warning_upsell(Some("overage"), &team(true, Some("admin"))),
            Some(USAGE_CREDITS_RAISE_CAP)
        );
        assert_eq!(
            warning_upsell(Some("overage"), &team(true, Some("member"))),
            Some(USAGE_CREDITS_ASK_ADMIN)
        );
    }

    #[test]
    fn wcg_team_extra_usage_enabled_non_overage_falls_through_to_none() {
        // hasExtraUsageEnabled but rateLimitType != "overage" → neither team
        // branch matches → null.
        assert_eq!(
            warning_upsell(Some("five_hour"), &team(true, Some("admin"))),
            None
        );
    }

    #[test]
    fn wcg_pro_max_five_hour_upgrade_fires_since_idle_amber_finch_is_false() {
        // `!Pee()`: idle_amber_finch() is documented `false` in this port ⇒
        // the pro/max five_hour upgrade upsell always fires.
        assert!(!flags::idle_amber_finch());
        assert_eq!(
            warning_upsell(Some("five_hour"), &pro()),
            Some(UPGRADE_KEEP_USING)
        );
        let max = SubscriptionSnapshot {
            subscription_type: Some("max".into()),
            ..pro()
        };
        assert_eq!(
            warning_upsell(Some("five_hour"), &max),
            Some(UPGRADE_KEEP_USING)
        );
    }

    #[test]
    fn wcg_weekly_limit_type_never_has_upsell() {
        // "Weekly limit warnings don't show upsell per spec" — unaffected by
        // the 206 rewrite: neither the team/enterprise nor the five_hour
        // pro/max arm matches a non-five_hour, non-overage rate limit type.
        assert_eq!(warning_upsell(Some("seven_day"), &pro()), None);
        assert_eq!(warning_upsell(None, &pro()), None);
    }

    // ── locked upsell literals (RateLimitMessage.tsx getUpsellMessage) ────

    #[test]
    fn usage_credits_upsell_uses_curly_apostrophe() {
        assert!(upsell::USAGE_CREDITS_FINISH.contains('\u{2019}'));
        assert!(upsell::UPGRADE_OR_USAGE_CREDITS.contains('\u{2019}'));
    }

    #[test]
    fn opening_options_uses_ellipsis() {
        assert!(upsell::OPENING_OPTIONS.ends_with('\u{2026}'));
    }

    // ── 2.1.206 deep-internal predicates (Task 2) ─────────────────────────

    #[test]
    fn eyt_is_hard_false() {
        // binary @212992273: `function Eyt(){return!1}`.
        assert!(!EYT);
    }

    #[test]
    fn deployment_first_party_defaults_not_first_party() {
        // 206 Rn(): documented default — the port is not the first-party
        // binary, so `Rn()!=="firstParty"` is true ⇒ the flag is false.
        assert!(!DEPLOYMENT_FIRST_PARTY);
        // Threaded through B5 it forces the credits-only gate on regardless of
        // the subscription (for this task's Ucg branch the `!B5()` suppression
        // term is masked by WBe==false; B5's other consumers are out of scope).
        let sub = SubscriptionSnapshot {
            is_subscriber: true,
            subscription_type: Some("pro".into()),
            rate_limit_tier: Some("default_claude_max_20x".into()),
            ..SubscriptionSnapshot::default()
        };
        assert!(sub.is_saffron_credits_only(DEPLOYMENT_FIRST_PARTY));
    }

    #[test]
    fn should_show_upsell_reduces_to_is_subscriber() {
        // Gid: shouldShowUpsell = Eyt() || Bo(); Eyt()==false ⇒ == is_subscriber.
        assert!(!should_show_upsell(&SubscriptionSnapshot::default()));
        let sub = SubscriptionSnapshot {
            is_subscriber: true,
            ..SubscriptionSnapshot::default()
        };
        assert!(should_show_upsell(&sub));
    }

    #[test]
    fn overage_consent_required_is_documented_false() {
        // WBe() = bIe() || Vqo(); both leaves have no port source ⇒ false.
        assert!(!overage_consent_required());
    }

    #[test]
    fn is_fable_model_matches_normalized_fable_id() {
        assert!(is_fable_model("claude-fable-5-1"));
        assert!(is_fable_model("Claude-Fable-5-1"));
        assert!(is_fable_model("anthropic/claude-fable-5-1"));
        assert!(is_fable_model("claude-fable-5-1[1m]"));
        assert!(!is_fable_model("claude-sonnet-4-5"));
        assert!(!is_fable_model("claude-mythos-5-1"));
    }

    #[test]
    fn overage_included_set_is_empty_arm_never_fires() {
        // O9e() default [] ⇒ membership always false, for any display name.
        assert!(overage_included_models().is_empty());
        assert!(!model_in_overage_included_set("Claude Fable 5"));
        assert!(!model_in_overage_included_set(""));
    }

    #[test]
    fn rate_limit_flags_default_off() {
        // With no override env/config, all three 206 flags default to false so
        // the default build is byte-identical to pre-change.
        assert!(!flags::spend_limit_nudge_enabled());
        assert!(!flags::idle_amber_finch());
        assert!(!flags::coral_beacon());
    }

    // ── 2.1.206 `Fdu` overage close-to-your ${limitName} (Task 8) ─────────

    #[test]
    fn overage_close_to_your_uses_limit_name() {
        // 2.1.206 binary @217902246: `` `You're close to your
        // ${A5()?"usage limit":"usage credit limit"}` `` — straight ASCII
        // apostrophe, byte-verified against the real 2.1.206 binary.
        let info = RateLimitInfo {
            status: Some("rejected".into()),
            overage_status: Some("allowed_warning".into()),
            ..RateLimitInfo::default()
        };
        // not usage_based → "usage credit limit" (A5() == false).
        let sub = SubscriptionSnapshot::default();
        assert_eq!(
            compose_with(&info, &sub, true).unwrap().text,
            "You're close to your usage credit limit"
        );
        // A5() == true → "usage limit".
        let sub = SubscriptionSnapshot {
            billing_type: Some("usage_based".into()),
            ..SubscriptionSnapshot::default()
        };
        assert_eq!(
            compose_with(&info, &sub, true).unwrap().text,
            "You're close to your usage limit"
        );
        assert!(
            !compose_with(&info, &sub, true)
                .unwrap()
                .text
                .contains('\u{2019}'),
            "TS source uses a straight ASCII apostrophe"
        );
    }

    #[test]
    fn qcg_maps_206_limit_names() {
        // Port of `qcg` (2.1.206 binary @217905040), byte-verified against
        // the real binary. Pin every arm, including the NEW
        // `seven_day_overage_included` → "Fable 5 limit" case (206-only;
        // "Fable 5" is a model name, not rebranded).
        let unknown = SubscriptionSnapshot::default();
        assert_eq!(
            qcg_limit_name(Some("seven_day_sonnet"), &unknown),
            Some("Sonnet limit")
        );
        assert_eq!(
            qcg_limit_name(Some("seven_day_sonnet"), &pro()),
            Some("weekly limit")
        );
        assert_eq!(
            qcg_limit_name(Some("seven_day_opus"), &unknown),
            Some("Opus limit")
        );
        assert_eq!(
            qcg_limit_name(Some("seven_day_overage_included"), &unknown),
            Some("Fable 5 limit")
        );
        assert_eq!(
            qcg_limit_name(Some("seven_day"), &unknown),
            Some("weekly limit")
        );
        assert_eq!(
            qcg_limit_name(Some("five_hour"), &unknown),
            Some("session limit")
        );
        assert_eq!(qcg_limit_name(None, &unknown), None);
        assert_eq!(qcg_limit_name(Some("overage"), &unknown), None);
    }

    // ── Task 14: end-to-end byte-exact matrix through `compose_rate_limit`
    // ────────────────────────────────────────────────────────────────────
    //
    // Every test above drives the composer through the env-injectable
    // `compose_with` seam. These drive the REAL public entry point,
    // `compose_rate_limit(&RateLimitInfo, &SubscriptionSnapshot)` — the one
    // `chat_widget` actually calls — so the `DISABLE_EXTRA_USAGE_COMMAND`
    // env-var plumbing is pinned end-to-end too, not just the pure core. No
    // other test in this crate touches that var, so clearing it here is
    // deterministic and race-free under the parallel test harness.

    /// Clear the var AND hold the crate-wide env lock for the caller's whole
    /// body — the returned guard must be bound (`let _env = …`).
    ///
    /// The previous comment here claimed this was "deterministic and race-free"
    /// because no other test touches this variable. That reasoning is about the
    /// VARIABLE; the race is on the environ BLOCK, which `chat_widget`'s
    /// agent-view test mutates concurrently. Reads here could tear.
    #[must_use]
    fn clear_rate_limit_env() -> std::sync::MutexGuard<'static, ()> {
        let guard = crate::ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        std::env::remove_var("DISABLE_EXTRA_USAGE_COMMAND");
        guard
    }

    #[test]
    fn e2e_overage_allowed_warning_close_to_credit_limit() {
        let _env = clear_rate_limit_env();
        // rejected + overageStatus allowed_warning → isUsingOverage → the
        // 2.1.206 close-to-limit copy, non-usage-based default snapshot.
        let info = RateLimitInfo {
            status: Some("rejected".into()),
            overage_status: Some("allowed_warning".into()),
            ..RateLimitInfo::default()
        };
        let got = compose_rate_limit(&info, &SubscriptionSnapshot::default()).unwrap();
        assert_eq!(got.text, "You're close to your usage credit limit");
        assert_eq!(got.upsell, None, "warnings carry no dim upsell line");
    }

    #[test]
    fn e2e_rejected_out_of_credits_personal_upsell_wired_through_gid() {
        let _env = clear_rate_limit_env();
        // overageStatus rejected, reason out_of_credits, non-usage-based
        // (pro, stripe billing) → "You're out of usage credits · resets
        // {t}", and — unlike the pure-core tests above — the upsell here is
        // computed by the REAL `Gid` wiring inside `compose_rate_limit`
        // (pro + extra-usage-command-enabled + no server-hide headers →
        // UPGRADE_OR_USAGE_CREDITS).
        let ts = ts_in(1800);
        let info = RateLimitInfo {
            status: Some("rejected".into()),
            overage_status: Some("rejected".into()),
            overage_disabled_reason: Some("out_of_credits".into()),
            overage_resets_at: Some(ts),
            ..RateLimitInfo::default()
        };
        let got = compose_rate_limit(&info, &pro()).unwrap();
        assert_eq!(
            got.text,
            format!("You're out of usage credits \u{b7} resets {}", reset(ts))
        );
        assert_eq!(
            got.upsell.as_deref(),
            Some(upsell::UPGRADE_OR_USAGE_CREDITS)
        );

        // Absent reset (`overage_resets_at: None`) drops the suffix entirely.
        let info_no_reset = RateLimitInfo {
            overage_resets_at: None,
            ..info
        };
        let got = compose_rate_limit(&info_no_reset, &pro()).unwrap();
        assert_eq!(got.text, "You're out of usage credits");
    }

    #[test]
    fn e2e_rejected_out_of_credits_org_usage_based_billing_add_funds() {
        let _env = clear_rate_limit_env();
        // Team admin, usage-based org billing, out_of_credits →
        // "Your org is out of usage · add funds to continue"; the Gid
        // upsell for this exact shape is the team admin-enable copy (the
        // org's billing_type isn't Stripe/Apple/Google, so the extra-usage
        // command itself is disabled).
        let sub = usage_based(team(false, Some("admin")));
        let info = RateLimitInfo {
            status: Some("rejected".into()),
            overage_status: Some("rejected".into()),
            overage_disabled_reason: Some("out_of_credits".into()),
            ..rejected(Some("five_hour"), None)
        };
        let got = compose_rate_limit(&info, &sub).unwrap();
        assert_eq!(
            got.text,
            "Your org is out of usage \u{b7} add funds to continue"
        );
        assert_eq!(
            got.upsell.as_deref(),
            Some(upsell::USAGE_CREDITS_ADMIN_ENABLE)
        );
    }

    #[test]
    fn e2e_seven_day_overage_included_fable5_jid_suppresses_upsell() {
        let _env = clear_rate_limit_env();
        // `jid = rateLimitType==="seven_day_overage_included" ||
        // errorCode==="credits_required"` hard-nulls the upsell BEFORE
        // `Gid` ever runs — even for a pro subscriber who would otherwise
        // get a real upsell line. Text uses the NEW 206 "Fable 5 limit"
        // naming (`qcg`/`Ucg`).
        let info = RateLimitInfo {
            status: Some("rejected".into()),
            rate_limit_type: Some("seven_day_overage_included".into()),
            ..RateLimitInfo::default()
        };
        let got = compose_rate_limit(&info, &pro()).unwrap();
        assert_eq!(got.text, "You've hit your Fable 5 limit");
        assert_eq!(
            got.upsell, None,
            "jid gate hard-nulls the upsell before Gid runs"
        );
    }

    #[test]
    fn e2e_credits_required_jid_suppresses_upsell() {
        let _env = clear_rate_limit_env();
        // Same `jid` gate, driven by `credits_required` instead of the
        // rate-limit-type disjunct.
        let info = RateLimitInfo {
            credits_required: true,
            ..rejected(Some("five_hour"), None)
        };
        let got = compose_rate_limit(&info, &pro()).unwrap();
        assert_eq!(got.text, "You've hit your session limit");
        assert_eq!(
            got.upsell, None,
            "jid gate hard-nulls the upsell before Gid runs"
        );
    }

    #[test]
    fn e2e_approaching_five_hour_pro_early_warning_with_upgrade_upsell() {
        let _env = clear_rate_limit_env();
        // allowed_warning, utilization >= 0.7, five_hour, pro subscriber →
        // the early-warning text (getEarlyWarningText / `jcg`) with the
        // `/upgrade to keep using LingXi` suffix appended by
        // getWarningUpsellText (`Wcg`). Warnings never populate the
        // separate `upsell` field (only error-severity notices do — this
        // suffix lives INSIDE `text`).
        let ts = ts_in(3600);
        let info = RateLimitInfo {
            status: Some("allowed_warning".into()),
            rate_limit_type: Some("five_hour".into()),
            utilization: Some(0.8),
            resets_at: Some(ts),
            ..RateLimitInfo::default()
        };
        let got = compose_rate_limit(&info, &pro()).unwrap();
        assert_eq!(
            got.text,
            format!(
                "You've used 80% of your session limit \u{b7} resets {} \u{b7} /upgrade to keep using LingXi",
                reset(ts)
            )
        );
        assert_eq!(got.upsell, None);
    }

    #[test]
    fn e2e_inert_non_subscriber_rejected_has_no_upsell_leakage() {
        let _env = clear_rate_limit_env();
        // `Gid` `shouldShowUpsell = Eyt()||Bo()` reduces to
        // `sub.is_subscriber` (Eyt() is a hard `false` constant in 2.1.206).
        // A default (non-subscriber / non-Anthropic-session) snapshot on a
        // rejected status must therefore compose with NO `/usage-credits`
        // upsell — proving non-subscriber sessions get no upsell leakage
        // from the 206 migration.
        let info = rejected(Some("five_hour"), None);
        let got = compose_rate_limit(&info, &SubscriptionSnapshot::default()).unwrap();
        assert_eq!(got.upsell, None);
    }
}
