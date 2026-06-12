# llm-client Future-Work Batch 4 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the remaining low-priority items from spec rev2.8: TUI subscription-granularity upsell arms (with the OAuth profile/roles plumbing they need), the surpassed-threshold early-warning header port, and a host-side Gemini upload polling convenience.

**Architecture:** Three independent strands. (1) A `SubscriptionSnapshot` value type in `traits` (additive), resolved at the desktop composition root by a background OAuth profile + roles fetch and threaded CLI → TUI so the rate-limit composer can take the real TS subscription branches instead of its documented scope-guards. (2) The TS `getEarlyWarningFromHeaders` two-stage logic (header-based `surpassed-threshold` detection + time-relative fallback) ported into `orchestrator::model::rate_limit::RateLimitInfo::from_headers`, including the `computeNewLimitsFromHeaders` final-status semantics. (3) A `wait_for_file_active` polling driver on `DefaultLlmClient` (tokio `time` feature is already in the workspace dep).

**Tech Stack:** Rust workspace (`lingxi-code/`), tokio, serde. Ground truth: vendored claude-code TS at `/Users/luolingfeng/Projects/LingXi-Next/claude-code/` (read-only — it lives in the PRIMARY checkout, not this worktree).

---

## Standing constraints (MUST follow — carried from all prior batches)

- **NEVER touch the primary checkout** working tree `/Users/luolingfeng/Projects/LingXi-Next` (another session works there). All edits happen in THIS worktree (`.claude/worktrees/llm-client-futurework-b4`). The TS ground-truth files are read from the primary checkout by absolute path — READ ONLY.
- **NEVER `git add -A` or `git add .`** — stage named paths only (untracked `codex/`, `liter-llm/`, `opencode/`, `claude-code/`, `.omo/`, `third_party/` dirs exist at repo root in the primary checkout).
- **`traits/` and `protocol/` crates are FROZEN-ADDITIVE**: zero removed/modified lines vs main. Verify with `git diff main -- lingxi-code/traits | grep -c '^-[^-]'` → must print `0` (same for `lingxi-code/protocol`). New files/modules/`pub use` lines are fine.
- **No secret material** (tokens, API keys, header values) in error messages, logs, or test fixtures' assertions.
- Strict **TDD with observed RED**: write the test, run it, SEE it fail, then implement.
- Clippy gate: `cargo clippy -p <crate> --all-targets --no-deps -- -D warnings` (workspace sets pedantic=warn).
- Commit trailer exactly: `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`.
- Fresh-worktree gotcha: run `cargo build -p mock_stdio_mcp` once before the first full test run.
- Copy strings: TS `rateLimitMessages.ts` uses STRAIGHT ASCII apostrophes; `RateLimitMessage.tsx` upsell strings use CURLY `\u{2019}` (already byte-locked in `tui/src/components/messages/rate_limit.rs`). Separators are U+00B7 `·`. Never "fix" these.

## Ground-truth references (read before implementing the matching task)

| Topic | File (primary checkout, READ ONLY) | Key lines |
|---|---|---|
| Early-warning configs + header detection + final-status semantics | `claude-code/src/services/claudeAiLimits.ts` | 38-77 (configs/claim map), 98-103 (`computeTimeProgress`), 255-294 (`getHeaderBasedEarlyWarning`), 301-340 (`getTimeRelativeEarlyWarning`), 347-374 (`getEarlyWarningFromHeaders`), 376-436 (`computeNewLimitsFromHeaders`) |
| Warning suppression + sonnet naming + warning upsell | `claude-code/src/services/rateLimitMessages.ts` | 68-100 (`getRateLimitMessage` warning path), 175-182 (`seven_day_sonnet`), 261-297 (`getWarningUpsellText`), 303-331 (`getUsingOverageText`) |
| Error-severity upsell arms | `claude-code/src/components/messages/RateLimitMessage.tsx` | 18-47 (`getUpsellMessage`), 52-160 (component wiring incl. `isMax20x`, `shouldShowUpsell`) |
| Subscription/billing predicates | `claude-code/src/utils/auth.ts` 1623-1643 (`isOverageProvisioningAllowed`), 1662-1677 (`getSubscriptionType`), 1703-1712 (`getRateLimitTier`); `claude-code/src/utils/billing.ts` 53-78 (`hasClaudeAiBillingAccess`) | |
| `/extra-usage` enablement | `claude-code/src/commands/extra-usage/index.ts` | 6-17 (`isExtraUsageAllowed`, `isEnabled`) |
| Roles fetch | `claude-code/src/services/oauth/client.ts` 276-309 (`fetchAndStoreUserRoles`); `claude-code/src/constants/oauth.ts` 93 (`ROLES_URL: 'https://api.anthropic.com/api/oauth/claude_cli/roles'`) | |

## Survey facts (already verified — do NOT re-derive)

- `anthropic-oauth` crate already has: `SubscriptionType` enum (`limits.rs:16` — Free/Pro/Max/Team/Enterprise/Unknown), `OAuthProfileResponse { organization: { organization_type, uuid, rate_limit_tier, billing_type, has_extra_usage_enabled, … }, account }` + `subscription_type()` resolver, `fetch_profile_from_oauth_token(access_token, transport)` (`profile.rs`) — currently NOT called at runtime (documented PARITY-GAP at `orchestrator/src/config.rs:134-138`).
- Composition root `engine_desktop::build` (`apps/engine-desktop/src/lib.rs:923-1013`) reads stored OAuth tokens via `secret::CredentialManager::get_oauth_tokens()` (struct has access_token/refresh_token/expires_at/scopes/email/org_id — NO subscriptionType; lingxi's own credential store never persists tier), computes `is_subscriber`, and inits the refresh driver. `http: Arc<PosixHttp>` is in scope for the background fetch.
- CLI→TUI wiring: `apps/cli/src/mode.rs:166-216 build_tui_runtime` uses the `tui::session::Runtime::with_bridge(...).with_orchestrator(...).with_*` builder; copy the `command_registry` threading pattern.
- TUI composer: `tui/src/rate_limit_messages.rs` — `compose_rate_limit(&info)` (env wrapper) → `compose_with(info, is_ant)` pure core; called from ONE site `tui/src/streaming.rs:142`. Documented scope-guards live at: module docs `:20-38`, warning-suppression comment `:144-148`, sonnet naming `:199`, `let upsell: Option<&str> = None` `:238-241`, `error_upsell` `:286-…`.
- Upsell constants already byte-locked in `tui/src/components/messages/rate_limit.rs:22-38`: `EXTRA_USAGE_FINISH`, `LOGIN_SWITCH`, `OPENING_OPTIONS`, `UPGRADE`, `EXTRA_USAGE_ADMIN`, `UPGRADE_OR_EXTRA` — these are EXACTLY the six `getUpsellMessage` return strings.
- `orchestrator::model::rate_limit::SubscriptionContext` (`rate_limit.rs:496`) has NO call sites outside its own file — the TUI composer is the live rendering path; do NOT plumb subscription into the orchestrator 429-error helper in this batch.
- `RateLimitInfo::from_headers` lives at `orchestrator/src/model/rate_limit.rs:377`; tolerant helpers `parse_epoch_secs`/`parse_fraction` exist nearby. Emit seams (`conversation.rs:2129` streaming, `turn_loop.rs:375` batched) call `from_headers` and dedupe-emit — they need no changes; the early-warning port changes what `from_headers` returns.
- Workspace tokio features: `["rt-multi-thread", "macros", "time", "sync"]` (root `Cargo.toml:208`); `llm-client` inherits via `tokio = { workspace = true }` → `tokio::time` is available.
- `DefaultLlmClient::upload_file` (`llm-client/src/client.rs:225`) shows the request-resolve-send pattern to copy for the status poll; `gemini_files::file_status_request` / `parse_file_status` are the builders; `GeminiFile.state` is a verbatim string (`PROCESSING`/`ACTIVE`/`FAILED`).
- `LlmError` variants available: `InvalidRequest{message}`, `Transport{message}`, `ProviderInternal`, … (`llm-client/src/error.rs`).

## Closed / blocked items (no task — record in spec, Task 8)

- **Mobile-host body_bytes marshaling check: CLOSED by audit.** `platform_posix::PosixHttp` is a re-export alias of `platform_common::http::ReqwestHttp` (`platforms/posix/src/http.rs:16`), which honors `body_bytes` (batch 3). `apps/android-aar/src/lib.rs:1089` constructs `PosixHttp::new()` directly — HTTP never crosses the FFI boundary on Android; `engine-mobile`'s `DynHttp` (`apps/engine-mobile/src/host.rs:80`) forwards the whole `HttpRequest` verbatim. No gap today. Spec note: IF a host-injected (Kotlin/Swift) `HttpTransport` is ever added, its marshaling must carry `body_bytes` verbatim.
- **OpenAiResponses real-traffic validation: still BLOCKED** — no OpenAI API key available in this environment (only Moonshot/DeepSeek, which speak Chat Completions, not the Responses API). Stays on the Remaining list.
- **Orchestrator `is_enterprise` retry-gate PARITY-GAP** (`orchestrator/src/config.rs:134`): NOT closed here. The background profile fetch lands after `OrchestratorConfig` is built; rewiring the retry gates onto a live handle is out of scope. The TUI rendering path (this batch) gets live subscription data; the spec's Remaining list keeps the retry-gate item.

---

### Task 1: `traits::subscription` — SubscriptionSnapshot + predicates (FROZEN-ADDITIVE)

**Files:**
- Create: `lingxi-code/traits/src/subscription.rs`
- Modify: `lingxi-code/traits/src/lib.rs` (add `pub mod subscription;` — additive line only)

`traits` is frozen-additive: a brand-new module + one new `pub mod` line is allowed; do not touch anything else in the crate.

- [ ] **Step 1: Write failing tests** (in `subscription.rs` `#[cfg(test)] mod tests` — unit tests inside the new module file, matching the crate's existing style, e.g. `orchestrator.rs` keeps its tests inline)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn team_snapshot() -> SubscriptionSnapshot {
        SubscriptionSnapshot {
            is_subscriber: true,
            subscription_type: Some("team".to_owned()),
            rate_limit_tier: None,
            has_extra_usage_enabled: false,
            billing_type: Some("stripe_subscription".to_owned()),
            organization_role: None,
        }
    }

    #[test]
    fn default_snapshot_resolves_all_predicates_false() {
        let s = SubscriptionSnapshot::default();
        assert!(!s.is_team_or_enterprise());
        assert!(!s.is_pro_or_enterprise());
        assert!(!s.is_max20x());
        assert!(!s.is_overage_provisioning_allowed());
        assert!(!s.has_claude_ai_billing_access());
        assert!(!s.is_extra_usage_command_enabled(false));
    }

    #[test]
    fn team_and_enterprise_predicates() {
        let mut s = team_snapshot();
        assert!(s.is_team_or_enterprise());
        assert!(!s.is_pro_or_enterprise());
        s.subscription_type = Some("enterprise".to_owned());
        assert!(s.is_team_or_enterprise());
        assert!(s.is_pro_or_enterprise());
        s.subscription_type = Some("pro".to_owned());
        assert!(!s.is_team_or_enterprise());
        assert!(s.is_pro_or_enterprise());
    }

    #[test]
    fn max20x_is_exact_tier_match() {
        let mut s = SubscriptionSnapshot::default();
        s.rate_limit_tier = Some("default_claude_max_20x".to_owned());
        assert!(s.is_max20x());
        s.rate_limit_tier = Some("default_claude_max_5x".to_owned());
        assert!(!s.is_max20x());
    }

    // isOverageProvisioningAllowed (auth.ts:1623-1643): subscriber + one of the
    // four purchasable billing types.
    #[test]
    fn overage_provisioning_requires_subscriber_and_billing_type() {
        let mut s = team_snapshot();
        assert!(s.is_overage_provisioning_allowed());
        for bt in [
            "stripe_subscription",
            "stripe_subscription_contracted",
            "apple_subscription",
            "google_play_subscription",
        ] {
            s.billing_type = Some(bt.to_owned());
            assert!(s.is_overage_provisioning_allowed(), "{bt}");
        }
        s.billing_type = Some("aws_marketplace".to_owned());
        assert!(!s.is_overage_provisioning_allowed());
        s.billing_type = None;
        assert!(!s.is_overage_provisioning_allowed());
        let mut t = team_snapshot();
        t.is_subscriber = false;
        assert!(!t.is_overage_provisioning_allowed());
    }

    // hasClaudeAiBillingAccess (billing.ts:53-78): pro/max → always true;
    // otherwise org role gate. Non-subscriber → false.
    #[test]
    fn billing_access_pro_max_always_team_by_role() {
        let mut s = team_snapshot();
        assert!(!s.has_claude_ai_billing_access());
        for role in ["admin", "billing", "owner", "primary_owner"] {
            s.organization_role = Some(role.to_owned());
            assert!(s.has_claude_ai_billing_access(), "{role}");
        }
        s.organization_role = Some("member".to_owned());
        assert!(!s.has_claude_ai_billing_access());
        s.subscription_type = Some("max".to_owned());
        assert!(s.has_claude_ai_billing_access());
        s.subscription_type = Some("pro".to_owned());
        assert!(s.has_claude_ai_billing_access());
        s.is_subscriber = false;
        assert!(!s.has_claude_ai_billing_access());
    }

    // extra-usage/index.ts:6-17 interactive arm: env-disabled kills it, else
    // provisioning gate.
    #[test]
    fn extra_usage_command_gate() {
        let s = team_snapshot();
        assert!(s.is_extra_usage_command_enabled(false));
        assert!(!s.is_extra_usage_command_enabled(true));
        let mut t = team_snapshot();
        t.billing_type = None;
        assert!(!t.is_extra_usage_command_enabled(false));
    }
}
```

- [ ] **Step 2: Run to observe RED**: `cargo test -p traits subscription` → compile error (module missing). That counts as RED for a new module.

- [ ] **Step 3: Implement** `lingxi-code/traits/src/subscription.rs`:

```rust
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
        matches!(self.subscription_type.as_deref(), Some("team" | "enterprise"))
    }

    /// `subscriptionType === 'pro' || subscriptionType === 'enterprise'`
    /// (the `seven_day_sonnet` naming gate, `rateLimitMessages.ts:176-181`).
    #[must_use]
    pub fn is_pro_or_enterprise(&self) -> bool {
        matches!(self.subscription_type.as_deref(), Some("pro" | "enterprise"))
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
```

Add to `lingxi-code/traits/src/lib.rs` (additive — put next to the existing `pub mod` lines): `pub mod subscription;`

- [ ] **Step 4: GREEN + clippy**: `cargo test -p traits subscription` → all pass. `cargo clippy -p traits --all-targets --no-deps -- -D warnings` → clean.
- [ ] **Step 5: Frozen check**: `git diff main -- lingxi-code/traits | grep -c '^-[^-]'` → prints `0`.
- [ ] **Step 6: Commit**

```bash
git add lingxi-code/traits/src/subscription.rs lingxi-code/traits/src/lib.rs
git commit -m "feat(traits): additive SubscriptionSnapshot + TS billing/upsell predicates

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 2: Orchestrator early-warning port (`getEarlyWarningFromHeaders` + final-status semantics)

**Files:**
- Modify: `lingxi-code/orchestrator/src/model/rate_limit.rs` (`RateLimitInfo` at :304, `from_headers` at :377)

Read `claudeAiLimits.ts:38-77, 98-103, 252-436` first. The port goes INSIDE the `RateLimitInfo` parse so both emit seams (`turn_loop.rs:375`, `conversation.rs:2129`) pick it up without changes.

Semantics to port exactly:
1. `getHeaderBasedEarlyWarning` (:255-294): for each claim in order `5h→five_hour`, `7d→seven_day`, `overage→overage`, if `anthropic-ratelimit-unified-{abbrev}-surpassed-threshold` is present → produce a REPLACEMENT info: `status=allowed_warning`, `rate_limit_type` from the map, `utilization`/`resets_at` from the per-claim headers (absent → None), `fallback_available` carried, `surpassed_threshold` = parsed number, all overage fields cleared.
2. `getTimeRelativeEarlyWarning` (:301-340): fallback, configs in priority order — `five_hour`/`5h`/window 5×60×60/thresholds `[{0.9, 0.72}]`; `seven_day`/`7d`/window 7×24×60×60/thresholds `[{0.75,0.60},{0.5,0.35},{0.25,0.15}]`. Requires BOTH per-claim utilization and reset headers. `time_progress = clamp((now − (resets_at − window)) / window, 0, 1)`; warn when ANY threshold has `utilization >= t.utilization && time_progress <= t.time_pct`.
3. `computeNewLimitsFromHeaders` final-status (:411-424): only when parsed `status` is `allowed` or `allowed_warning`: if an early warning fires → RETURN the replacement info; otherwise force `status = "allowed"` (the server's bare `allowed_warning` with no surpassed threshold is downgraded — that is TS behaviour, port it). `rejected` (and a missing status header → `None`, a deliberate divergence-guard documented in code: TS defaults to `'allowed'`, but fabricating a status with zero unified headers would break `has_unified_headers()` consumers) pass through untouched.
4. Transport-level fields already parsed by `from_headers` that have no TS counterpart in the replacement object (retry-after style fields, if any) keep their parsed values — only the TS-visible fields are replaced. Check the actual struct fields when editing and document the choice inline.

- [ ] **Step 1: Add `surpassed_threshold: Option<f64>` field** to `RateLimitInfo` with doc comment (claudeAiLimits.ts:135). Fix any struct-literal construction sites in this file's tests with `..Default::default()` or explicit `None`.

- [ ] **Step 2: Write failing tests** (same file, `#[cfg(test)]` mod — follow the 15 batch-3 tests' style). Helper: build header vecs as `Vec<(String, String)>`. Use `RateLimitInfo::from_headers_at(&headers, now)` with a fixed `now` built via `SystemTime::UNIX_EPOCH + Duration::from_secs(...)`.

```rust
fn h(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect()
}

#[test]
fn surpassed_threshold_header_forces_allowed_warning_replacement() {
    // status says plain allowed, but the 7d surpassed-threshold header fires.
    let headers = h(&[
        ("anthropic-ratelimit-unified-status", "allowed"),
        ("anthropic-ratelimit-unified-fallback", "available"),
        ("anthropic-ratelimit-unified-7d-surpassed-threshold", "0.5"),
        ("anthropic-ratelimit-unified-7d-utilization", "0.55"),
        ("anthropic-ratelimit-unified-7d-reset", "1750000000"),
        ("anthropic-ratelimit-unified-overage-status", "allowed"),
    ]);
    let info = RateLimitInfo::from_headers_at(&headers, SystemTime::UNIX_EPOCH);
    assert_eq!(info.status.as_deref(), Some("allowed_warning"));
    assert_eq!(info.rate_limit_type.as_deref(), Some("seven_day"));
    assert_eq!(info.utilization, Some(0.55));
    assert_eq!(info.resets_at, Some(1_750_000_000));
    assert_eq!(info.surpassed_threshold, Some(0.5));
    assert!(info.fallback_available);
    // TS returns a FRESH limits object: overage fields cleared.
    assert_eq!(info.overage_status, None);
}

#[test]
fn claim_priority_is_5h_then_7d_then_overage() {
    let headers = h(&[
        ("anthropic-ratelimit-unified-status", "allowed"),
        ("anthropic-ratelimit-unified-7d-surpassed-threshold", "0.5"),
        ("anthropic-ratelimit-unified-5h-surpassed-threshold", "0.9"),
    ]);
    let info = RateLimitInfo::from_headers_at(&headers, SystemTime::UNIX_EPOCH);
    assert_eq!(info.rate_limit_type.as_deref(), Some("five_hour"));
    assert_eq!(info.surpassed_threshold, Some(0.9));
}

#[test]
fn overage_claim_surpassed_threshold_maps_to_overage_type() {
    let headers = h(&[
        ("anthropic-ratelimit-unified-status", "allowed"),
        ("anthropic-ratelimit-unified-overage-surpassed-threshold", "0.8"),
    ]);
    let info = RateLimitInfo::from_headers_at(&headers, SystemTime::UNIX_EPOCH);
    assert_eq!(info.status.as_deref(), Some("allowed_warning"));
    assert_eq!(info.rate_limit_type.as_deref(), Some("overage"));
}

#[test]
fn time_relative_5h_fires_at_high_utilization_early_in_window() {
    // window 18000s; resets_at = now + 9000 → elapsed 9000/18000 = 0.5 <= 0.72;
    // utilization 0.95 >= 0.9 → warn.
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let headers = h(&[
        ("anthropic-ratelimit-unified-status", "allowed"),
        ("anthropic-ratelimit-unified-5h-utilization", "0.95"),
        ("anthropic-ratelimit-unified-5h-reset", "1009000"),
    ]);
    let info = RateLimitInfo::from_headers_at(&headers, now);
    assert_eq!(info.status.as_deref(), Some("allowed_warning"));
    assert_eq!(info.rate_limit_type.as_deref(), Some("five_hour"));
    assert_eq!(info.surpassed_threshold, None); // time-relative path sets none
}

#[test]
fn time_relative_5h_does_not_fire_late_in_window() {
    // elapsed 17000/18000 ≈ 0.944 > 0.72 → no warning; allowed_warning from
    // the server is then DOWNGRADED to allowed (TS :423).
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let headers = h(&[
        ("anthropic-ratelimit-unified-status", "allowed_warning"),
        ("anthropic-ratelimit-unified-5h-utilization", "0.95"),
        ("anthropic-ratelimit-unified-5h-reset", "1001000"),
    ]);
    let info = RateLimitInfo::from_headers_at(&headers, now);
    assert_eq!(info.status.as_deref(), Some("allowed"));
}

#[test]
fn time_relative_7d_middle_threshold() {
    // window 604800; choose elapsed fraction 0.3 (<=0.35) with utilization 0.6
    // (>=0.5, <0.75): middle threshold fires.
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(2_000_000);
    let resets_at = 2_000_000 + (604_800 - 181_440); // elapsed = 181440 = 0.3 window
    let reset_header = resets_at.to_string();
    let headers = h(&[
        ("anthropic-ratelimit-unified-status", "allowed"),
        ("anthropic-ratelimit-unified-7d-utilization", "0.6"),
        ("anthropic-ratelimit-unified-7d-reset", &reset_header),
    ]);
    let info = RateLimitInfo::from_headers_at(&headers, now);
    assert_eq!(info.status.as_deref(), Some("allowed_warning"));
    assert_eq!(info.rate_limit_type.as_deref(), Some("seven_day"));
}

#[test]
fn rejected_status_passes_through_untouched_by_early_warning() {
    let headers = h(&[
        ("anthropic-ratelimit-unified-status", "rejected"),
        ("anthropic-ratelimit-unified-5h-surpassed-threshold", "0.9"),
        ("anthropic-ratelimit-unified-overage-status", "allowed"),
    ]);
    let info = RateLimitInfo::from_headers_at(&headers, SystemTime::UNIX_EPOCH);
    assert_eq!(info.status.as_deref(), Some("rejected"));
    assert_eq!(info.overage_status.as_deref(), Some("allowed"));
}

#[test]
fn missing_status_header_stays_none_no_fabricated_allowed() {
    let info = RateLimitInfo::from_headers_at(&[], SystemTime::UNIX_EPOCH);
    assert_eq!(info.status, None);
}

#[test]
fn from_headers_delegates_with_wall_clock() {
    // Smoke: the legacy entry point still exists and parses statuses.
    let headers = h(&[("anthropic-ratelimit-unified-status", "rejected")]);
    let info = RateLimitInfo::from_headers(&headers);
    assert_eq!(info.status.as_deref(), Some("rejected"));
}
```

- [ ] **Step 3: RED**: `cargo test -p orchestrator rate_limit` → new tests fail to compile (`from_headers_at` missing) — observe it.

- [ ] **Step 4: Implement.** Sketch (adapt names to the file's existing conventions; reuse `parse_epoch_secs`/`parse_fraction`):

```rust
/// One early-warning trigger: warn when usage ≥ `utilization` while ≤
/// `time_pct` of the window has elapsed (claudeAiLimits.ts:38-41).
struct EarlyWarningThreshold {
    utilization: f64,
    time_pct: f64,
}

/// Per-claim fallback config (claudeAiLimits.ts:53-70).
struct EarlyWarningConfig {
    rate_limit_type: &'static str,
    claim_abbrev: &'static str,
    window_seconds: u64,
    thresholds: &'static [EarlyWarningThreshold],
}

const EARLY_WARNING_CONFIGS: &[EarlyWarningConfig] = &[
    EarlyWarningConfig {
        rate_limit_type: "five_hour",
        claim_abbrev: "5h",
        window_seconds: 5 * 60 * 60,
        thresholds: &[EarlyWarningThreshold { utilization: 0.9, time_pct: 0.72 }],
    },
    EarlyWarningConfig {
        rate_limit_type: "seven_day",
        claim_abbrev: "7d",
        window_seconds: 7 * 24 * 60 * 60,
        thresholds: &[
            EarlyWarningThreshold { utilization: 0.75, time_pct: 0.6 },
            EarlyWarningThreshold { utilization: 0.5, time_pct: 0.35 },
            EarlyWarningThreshold { utilization: 0.25, time_pct: 0.15 },
        ],
    },
];

/// Claim-abbrev → rateLimitType for header-based detection
/// (claudeAiLimits.ts:73-77). Iteration order is the TS object order.
const EARLY_WARNING_CLAIM_MAP: &[(&str, &str)] =
    &[("5h", "five_hour"), ("7d", "seven_day"), ("overage", "overage")];
```

`from_headers(headers)` becomes a thin wrapper: `Self::from_headers_at(headers, SystemTime::now())`. `from_headers_at` does the existing parse, then applies step 3 semantics above via two private fns (`header_based_early_warning`, `time_relative_early_warning`) that mirror the TS line-for-line. `computeTimeProgress` port: `((now_secs − (resets_at − window)) / window).clamp(0.0, 1.0)` with saturating/checked arithmetic — no panics on weird header values (resets_at in the past or absurdly far future).

- [ ] **Step 5: GREEN**: `cargo test -p orchestrator rate_limit` all pass; run the batch-3 emit tests too: `cargo test -p orchestrator --test rate_limit_emit_test`.
- [ ] **Step 6: clippy**: `cargo clippy -p orchestrator --all-targets --no-deps -- -D warnings`.
- [ ] **Step 7: Commit**

```bash
git add lingxi-code/orchestrator/src/model/rate_limit.rs
git commit -m "feat(orchestrator): early-warning port — surpassed-threshold headers + time-relative fallback

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 3: `anthropic-oauth` roles fetch (`fetchAndStoreUserRoles` read side)

**Files:**
- Modify: `lingxi-code/anthropic-oauth/src/profile.rs` (append; same file keeps the two fetchers together)
- Modify (only if needed): `lingxi-code/anthropic-oauth/src/lib.rs` re-exports — follow how `fetch_profile_from_oauth_token` is exported.

Ground truth: `services/oauth/client.ts:276-309`; URL constant `constants/oauth.ts:93` → `https://api.anthropic.com/api/oauth/claude_cli/roles` (same `BASE_API_URL` base as the profile fetch). TS throws on failure (login flow); our background-fetch caller swallows — implement error-swallowing → `None`, matching `fetch_profile_from_oauth_token`'s convention, and document the divergence (TS throws because login wants the failure; our read-side caller treats missing roles as unknown).

- [ ] **Step 1: Write failing tests** (append to `profile.rs` tests mod, reusing its existing fake-transport pattern — read the existing tests first and copy the harness):

```rust
#[tokio::test]
async fn fetch_user_roles_parses_role_fields() {
    // Transport scripted to return 200 with the roles payload.
    let transport = fake_transport_returning(
        200,
        r#"{"organization_role":"admin","workspace_role":"workspace_developer","organization_name":"Acme"}"#,
    );
    let roles = fetch_user_roles("test-token", &transport).await.expect("roles");
    assert_eq!(roles.organization_role.as_deref(), Some("admin"));
    assert_eq!(roles.workspace_role.as_deref(), Some("workspace_developer"));
    assert_eq!(roles.organization_name.as_deref(), Some("Acme"));
}

#[tokio::test]
async fn fetch_user_roles_swallows_non_200_and_transport_errors() {
    let transport = fake_transport_returning(403, "{}");
    assert!(fetch_user_roles("test-token", &transport).await.is_none());
    let transport = fake_transport_erroring();
    assert!(fetch_user_roles("test-token", &transport).await.is_none());
}

#[tokio::test]
async fn fetch_user_roles_requests_roles_url_with_bearer() {
    // Capture the request: URL must be {BASE_API_URL}/api/oauth/claude_cli/roles,
    // Authorization: Bearer <token>. Use the capturing transport the profile
    // tests use.
}
```

(`fake_transport_returning` / `fake_transport_erroring` are whatever the existing profile tests call their helpers — REUSE them, do not invent a parallel harness. If the names differ, adapt the test code to the existing helpers.)

- [ ] **Step 2: RED**: `cargo test -p anthropic-oauth profile` → compile failure on `fetch_user_roles`.

- [ ] **Step 3: Implement** (append to `profile.rs`):

```rust
/// Roles endpoint path — `constants/oauth.ts:93` (`ROLES_URL`).
pub const ROLES_URL_PATH: &str = "/api/oauth/claude_cli/roles";

/// Subset of the TS `UserRolesResponse` we consume
/// (`services/oauth/client.ts:283-301`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct UserRolesResponse {
    /// e.g. `"admin" | "billing" | "owner" | "primary_owner" | "member"`.
    #[serde(default)]
    pub organization_role: Option<String>,
    /// Workspace-scoped role.
    #[serde(default)]
    pub workspace_role: Option<String>,
    /// Human-readable organization name.
    #[serde(default)]
    pub organization_name: Option<String>,
}

/// Fetch the signed-in user's org/workspace roles.
///
/// `GET {BASE_API_URL}/api/oauth/claude_cli/roles` with `Authorization:
/// Bearer <token>`, 10s timeout. TS (`fetchAndStoreUserRoles`,
/// `client.ts:276-309`) THROWS on failure because the login flow wants the
/// error; this read-side port swallows everything → `None` (callers treat
/// missing roles as "role unknown", same stance as the profile fetch above).
pub async fn fetch_user_roles(
    access_token: &str,
    transport: &Arc<dyn HttpTransport>,
) -> Option<UserRolesResponse> {
    let req = HttpRequest {
        method: HttpMethod::Get,
        url: format!("{BASE_API_URL}{ROLES_URL_PATH}"),
        headers: vec![(
            "Authorization".into(),
            format!("Bearer {access_token}"),
        )],
        body: None,
        body_bytes: None,
        timeout: Some(PROFILE_TIMEOUT),
    };
    let resp = transport.request(req).await.ok()?;
    if resp.status != 200 {
        return None;
    }
    serde_json::from_slice(&resp.body).ok()
}
```

(Adapt the response-body access to however `fetch_profile_from_oauth_token` reads its body — match it exactly.)

- [ ] **Step 4: GREEN + clippy**: `cargo test -p anthropic-oauth profile` pass; `cargo clippy -p anthropic-oauth --all-targets --no-deps -- -D warnings` clean.
- [ ] **Step 5: Commit**

```bash
git add lingxi-code/anthropic-oauth/src/profile.rs lingxi-code/anthropic-oauth/src/lib.rs
git commit -m "feat(anthropic-oauth): user-roles fetch (claude_cli/roles) for subscription resolution

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 4: Desktop composition root — background subscription resolution

**Files:**
- Modify: `lingxi-code/apps/engine-desktop/src/lib.rs` (`build` at :923, `DesktopRuntime` struct — find it in the same file)

Behavior: `build` creates a `traits::subscription::SharedSubscription` slot. After the token read (:972), seed it with `Some(SubscriptionSnapshot { is_subscriber, ..Default::default() })` (API-key/no-token sessions: seed with `Some(SubscriptionSnapshot::default())` — predicates all false). When the OAuth path is active (`llm_oauth_state` set), `tokio::spawn` a background task that calls `fetch_profile_from_oauth_token` + `fetch_user_roles` with the access token and the existing `http` transport, builds the full snapshot via a pure mapping fn, and writes it into the slot. Expose the slot as a new `pub subscription` field on `DesktopRuntime`.

Secrecy: the access token is cloned ONLY into the spawned task and passed to the two fetchers; never logged, never formatted into errors.

- [ ] **Step 1: Write the pure mapping fn + failing unit test** (same file, tests mod):

```rust
/// Fold the profile + roles responses into the shared snapshot. Pure —
/// unit-tested without IO. Tier mapping mirrors
/// `OAuthProfileResponse::subscription_type()` (TS string union values);
/// `Free`/`Unknown` resolve to `None` (conservative, same as the TS `null`).
fn subscription_snapshot_from(
    is_subscriber: bool,
    profile: Option<&anthropic_oauth::OAuthProfileResponse>,
    roles: Option<&anthropic_oauth::UserRolesResponse>,
) -> traits::subscription::SubscriptionSnapshot {
    use anthropic_oauth::limits::SubscriptionType;
    let org = profile.and_then(|p| p.organization.as_ref());
    let subscription_type = profile.and_then(|p| p.subscription_type()).and_then(|t| match t {
        SubscriptionType::Pro => Some("pro"),
        SubscriptionType::Max => Some("max"),
        SubscriptionType::Team => Some("team"),
        SubscriptionType::Enterprise => Some("enterprise"),
        SubscriptionType::Free | SubscriptionType::Unknown => None,
    });
    traits::subscription::SubscriptionSnapshot {
        is_subscriber,
        subscription_type: subscription_type.map(str::to_owned),
        rate_limit_tier: org.and_then(|o| o.rate_limit_tier.clone()),
        has_extra_usage_enabled: org.and_then(|o| o.has_extra_usage_enabled) == Some(true),
        billing_type: org.and_then(|o| o.billing_type.clone()),
        organization_role: roles.and_then(|r| r.organization_role.clone()),
    }
}
```

Test (RED first — fn missing):

```rust
#[test]
fn subscription_snapshot_maps_profile_and_roles() {
    use anthropic_oauth::{OAuthOrganization, OAuthProfileResponse, UserRolesResponse};
    let profile = OAuthProfileResponse {
        organization: Some(OAuthOrganization {
            organization_type: Some("claude_team".to_owned()),
            rate_limit_tier: Some("default_claude_max_5x".to_owned()),
            billing_type: Some("stripe_subscription".to_owned()),
            has_extra_usage_enabled: Some(true),
            ..Default::default()
        }),
        account: None,
    };
    let roles = UserRolesResponse {
        organization_role: Some("admin".to_owned()),
        ..Default::default()
    };
    let snap = subscription_snapshot_from(true, Some(&profile), Some(&roles));
    assert_eq!(snap.subscription_type.as_deref(), Some("team"));
    assert_eq!(snap.rate_limit_tier.as_deref(), Some("default_claude_max_5x"));
    assert!(snap.has_extra_usage_enabled);
    assert_eq!(snap.billing_type.as_deref(), Some("stripe_subscription"));
    assert_eq!(snap.organization_role.as_deref(), Some("admin"));
    assert!(snap.has_claude_ai_billing_access());
}

#[test]
fn subscription_snapshot_absent_profile_is_conservative() {
    let snap = subscription_snapshot_from(true, None, None);
    assert!(snap.is_subscriber);
    assert_eq!(snap.subscription_type, None);
    assert!(!snap.has_claude_ai_billing_access());
}
```

(Adjust import paths to the crate's actual re-exports — check `anthropic-oauth/src/lib.rs` for whether `OAuthProfileResponse` etc. are re-exported at the root.)

- [ ] **Step 2: RED → implement the fn → GREEN.**

- [ ] **Step 3: Wire the slot into `build` + `DesktopRuntime`.** In `build`: create `let subscription: traits::subscription::SharedSubscription = Arc::new(std::sync::RwLock::new(None));` near the top of step (3). In the `Ok(Some(tokens))` arm, clone the access token string BEFORE it moves into `init_refresh_driver` (`let profile_token = tokens.access_token.clone();` — it's a `Secret<String>`; check how other code exposes it, e.g. `.expose_secret()`, and expose only inside the spawned task). After `is_subscriber` is computed, seed: `*subscription.write().expect("subscription slot") = Some(SubscriptionSnapshot { is_subscriber, ..Default::default() });`. When the refresh driver attaches AND `is_subscriber` (the same `if is_subscriber` block at :997), spawn:

```rust
// Background subscription-tier resolution (closes the rendering half of the
// profile-fetch PARITY-GAP documented at orchestrator/src/config.rs:134):
// fetch profile + roles once, fold into the shared snapshot the TUI reads at
// rate-limit compose time. Errors are swallowed — the seeded
// scopes-only snapshot stays, which renders the same copy as before this
// wiring (conservative predicates).
{
    let slot = subscription.clone();
    let transport: Arc<dyn traits::HttpTransport> = http.clone();
    let token = profile_token.clone();
    tokio::spawn(async move {
        let token = token.expose_secret();
        let profile =
            anthropic_oauth::fetch_profile_from_oauth_token(token, &transport).await;
        let roles = anthropic_oauth::fetch_user_roles(token, &transport).await;
        let snap = subscription_snapshot_from(true, profile.as_ref(), roles.as_ref());
        if let Ok(mut slot) = slot.write() {
            *slot = Some(snap);
        }
    });
}
```

(Adapt: exact `Secret` expose API, exact re-export paths, and whether `http` already coerces to `Arc<dyn HttpTransport>` — the profile fetcher signature is `&Arc<dyn HttpTransport>`.) For the no-token / API-key arms, seed `Some(SubscriptionSnapshot::default())`.

Add to `DesktopRuntime`: `pub subscription: traits::subscription::SharedSubscription,` and set it in the struct literal at the end of `build`. Fix any other `DesktopRuntime` construction sites (grep `DesktopRuntime {` across the workspace — test support included).

- [ ] **Step 4: Build + tests + clippy**: `cargo test -p engine-desktop` and `cargo clippy -p engine-desktop --all-targets --no-deps -- -D warnings`.
- [ ] **Step 5: Commit**

```bash
git add lingxi-code/apps/engine-desktop/src/lib.rs
git commit -m "feat(engine-desktop): background OAuth profile+roles fetch into shared SubscriptionSnapshot

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 5: Thread the snapshot CLI → TUI

**Files:**
- Modify: `lingxi-code/tui/src/session.rs` (Runtime builder)
- Modify: `lingxi-code/tui/src/state.rs` (AppState field)
- Modify: `lingxi-code/tui/src/root.rs` or wherever `Runtime` fields land in `AppState` — follow the `command_registry` threading end-to-end
- Modify: `lingxi-code/apps/cli/src/mode.rs:209-215` (builder call chain)

- [ ] **Step 1: Read the threading pattern.** Trace `with_command_registry` from `session.rs:154` to where the registry reaches `AppState`/the render loop. The subscription handle follows the identical route.

- [ ] **Step 2: Failing test** — in `tui/src/state.rs` tests (or `session.rs` if Runtime fields are tested there):

```rust
#[test]
fn app_state_subscription_defaults_none_and_snapshot_reads_through() {
    let mut st = AppState::new(StatusSnapshot::default());
    assert!(st.subscription_snapshot().is_none());
    let slot: traits::subscription::SharedSubscription =
        std::sync::Arc::new(std::sync::RwLock::new(Some(
            traits::subscription::SubscriptionSnapshot {
                is_subscriber: true,
                ..Default::default()
            },
        )));
    st.subscription = Some(slot);
    assert!(st.subscription_snapshot().expect("snap").is_subscriber);
}
```

- [ ] **Step 3: RED → implement.** `AppState` gains:

```rust
/// Shared subscription slot from the composition root (None in tests /
/// print mode). Read at rate-limit compose time via
/// [`Self::subscription_snapshot`].
pub subscription: Option<traits::subscription::SharedSubscription>,
```

plus an accessor that hides the lock:

```rust
/// Current resolved subscription snapshot, if the composition root provided
/// a slot and the background fetch (or seed) has filled it. A poisoned lock
/// degrades to `None` (conservative copy, never a panic in the render path).
#[must_use]
pub fn subscription_snapshot(&self) -> Option<traits::subscription::SubscriptionSnapshot> {
    self.subscription
        .as_ref()
        .and_then(|s| s.read().ok())
        .and_then(|guard| guard.clone())
}
```

`session.rs`: add `with_subscription(mut self, sub: traits::subscription::SharedSubscription) -> Self` storing into a new `Runtime` field, defaulted `None`; thread it into `AppState` at the same place `command_registry` lands. `mode.rs`: append `.with_subscription(tui_build.runtime.subscription.clone())` to the builder chain at :209-215.

- [ ] **Step 4: GREEN + clippy** on `tui` and `lingxi-cli` builds: `cargo test -p tui state`, `cargo clippy -p tui --all-targets --no-deps -- -D warnings`, `cargo build -p cli` (use the actual CLI package name from `lingxi-code/apps/cli/Cargo.toml` — check it; prior sessions misremembered it as `lingxi-cli`).
- [ ] **Step 5: Commit**

```bash
git add lingxi-code/tui/src/session.rs lingxi-code/tui/src/state.rs lingxi-code/apps/cli/src/mode.rs
# plus root.rs or wherever the threading touched — name files explicitly
git commit -m "feat(tui): thread SharedSubscription from composition root into AppState

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 6: TUI composer — real subscription arms (replaces the rev2.8 scope-guards)

**Files:**
- Modify: `lingxi-code/tui/src/rate_limit_messages.rs` (compose core + upsell fns + module docs)
- Modify: `lingxi-code/tui/src/streaming.rs:142` (call site passes the snapshot)

Ground truth: `rateLimitMessages.ts:68-100, 175-182, 261-297, 303-331`; `RateLimitMessage.tsx:18-47, 74-88, 112-118`. The six error-upsell strings are ALREADY in `components/messages/rate_limit.rs::upsell` — reuse, never re-type them. Two NEW warning-upsell strings (straight ASCII, no trailing period — verified against TS source): `/extra-usage to request more`, `/upgrade to keep using Claude Code`.

Signature changes:
- `compose_rate_limit(info: &RateLimitInfo)` → `compose_rate_limit(info: &RateLimitInfo, sub: &SubscriptionSnapshot)`; env wrapper now also reads `DISABLE_EXTRA_USAGE_COMMAND` (truthiness: match the TS `isEnvTruthy` port already used elsewhere in the workspace — grep `is_env_truthy` / `env_truthy`; if none exists, port TS `isEnvTruthy` locally: trimmed, lowercased value ∈ {"1","true","yes","on"} — verify against the TS util source before assuming).
- `compose_with(info, is_ant)` → `compose_with(info, is_ant, sub: &SubscriptionSnapshot, extra_usage_cmd_enabled: bool)` (pure; env reads stay in the wrapper).
- Call site `streaming.rs:142`: `let sub = st.subscription_snapshot().unwrap_or_default();` then `compose_rate_limit(&info, &sub)`.

Behavior changes (each replaces a documented scope-guard — update the module docs `:20-38` to say the gaps are CLOSED):
1. **Warning suppression** (TS :80-94), placed after the 0.7 gate exactly as in TS: `if sub.is_team_or_enterprise() && sub.has_extra_usage_enabled && !sub.has_claude_ai_billing_access() { return None; }`
2. **`seven_day_sonnet` naming** in `limit_reached_text` (TS :175-182): `if sub.is_pro_or_enterprise() { "weekly limit" } else { "Sonnet limit" }`.
3. **Warning upsell** (TS :261-297) replaces `let upsell: Option<&str> = None` at :238-241:

```rust
/// Port of `getWarningUpsellText` (rateLimitMessages.ts:261-297).
fn warning_upsell(
    rate_limit_type: Option<&str>,
    sub: &SubscriptionSnapshot,
) -> Option<&'static str> {
    match rate_limit_type {
        Some("five_hour") => {
            if sub.is_team_or_enterprise() {
                if !sub.has_extra_usage_enabled && sub.is_overage_provisioning_allowed() {
                    return Some(EXTRA_USAGE_REQUEST);
                }
                // Teams/Enterprise with overages enabled or unsupported
                // billing type don't need upsell (:276-277).
                return None;
            }
            if matches!(sub.subscription_type.as_deref(), Some("pro" | "max")) {
                return Some(UPGRADE_KEEP_USING);
            }
            None
        }
        Some("overage") => {
            if sub.is_team_or_enterprise()
                && !sub.has_extra_usage_enabled
                && sub.is_overage_provisioning_allowed()
            {
                return Some(EXTRA_USAGE_REQUEST);
            }
            None
        }
        // Weekly limit warnings don't show upsell per spec (:295-296).
        _ => None,
    }
}

/// Warning-upsell copy (rateLimitMessages.ts:274, :282 — straight ASCII).
const EXTRA_USAGE_REQUEST: &str = "/extra-usage to request more";
const UPGRADE_KEEP_USING: &str = "/upgrade to keep using Claude Code";
```

4. **Error upsell** — replace the generic-arm `error_upsell(info)` with the full `getUpsellMessage` port (TSX :18-47). `shouldAutoOpenRateLimitOptionsMenu` is structurally `false` (the TUI has no interactive rate-limit options menu — document; `OPENING_OPTIONS` stays byte-locked but unused by this fn). `shouldShowUpsell` = `sub.is_subscriber` (the TS `shouldProcessMockLimits()` arm is the unported `/mock-limits` test command). Return type becomes `Option<String>` (the previous fn always returned a string; `ComposedRateLimit.upsell` is already `Option<String>`):

```rust
/// Port of `getUpsellMessage` (RateLimitMessage.tsx:18-47).
/// `shouldAutoOpenRateLimitOptionsMenu` is structurally false: the TUI has no
/// interactive rate-limit options menu, so the `OPENING_OPTIONS` arm is
/// unreachable (the constant stays byte-locked for when the menu lands).
fn error_upsell(sub: &SubscriptionSnapshot, extra_usage_cmd_enabled: bool) -> Option<String> {
    if !sub.is_subscriber {
        return None; // shouldShowUpsell (mock-limits arm not ported)
    }
    if sub.is_max20x() {
        return Some(
            if extra_usage_cmd_enabled { upsell::EXTRA_USAGE_FINISH } else { upsell::LOGIN_SWITCH }
                .to_owned(),
        );
    }
    if !sub.is_team_or_enterprise() && !extra_usage_cmd_enabled {
        return Some(upsell::UPGRADE.to_owned());
    }
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
    Some(upsell::UPGRADE_OR_EXTRA.to_owned())
}
```

Check how the CURRENT `error_upsell` result is consumed (`upsell: Some(error_upsell(info))` at :128) and adjust to `upsell: error_upsell(&sub, extra_usage_cmd_enabled)`.

5. **Early-warning text upsell append** (TS :229-253): `early_warning_text` gains the upsell parameter and appends `" · {upsell}"` (U+00B7) on every branch, mirroring the four TS return points.

- [ ] **Step 1: Write failing tests** (extend the existing test mod; follow its fixture style — it has `RateLimitInfo` builders already). Cover at minimum:

```rust
// — default snapshot reproduces rev2.8 behavior —
#[test]
fn unknown_subscription_keeps_rev28_copy() {
    // rejected seven_day_sonnet with default snapshot → "Sonnet limit", error
    // upsell None (subscriber unknown ⇒ shouldShowUpsell false).
}

// — sonnet naming —
#[test]
fn pro_subscriber_sonnet_limit_reads_weekly_limit() { /* pro → "weekly limit" */ }

// — warning suppression —
#[test]
fn team_with_extra_usage_and_no_billing_access_suppresses_warning() {
    // status allowed_warning, utilization 0.8, team + has_extra_usage_enabled
    // + role None → compose returns None.
}
#[test]
fn team_admin_still_sees_warning() { /* same but role admin → Some(...) */ }

// — warning upsell —
#[test]
fn five_hour_warning_pro_appends_upgrade_upsell() {
    // pro subscriber (stripe billing), five_hour allowed_warning, util 0.8,
    // resets — text ends with " · /upgrade to keep using Claude Code".
}
#[test]
fn five_hour_warning_team_without_extra_usage_appends_request_upsell() {
    // team + stripe + !has_extra_usage → " · /extra-usage to request more".
}
#[test]
fn five_hour_warning_team_with_extra_usage_has_no_upsell() {}
#[test]
fn weekly_warning_never_has_upsell() {}

// — error upsell arms (getUpsellMessage) —
#[test]
fn max20x_error_upsell_login_switch_without_extra_usage_cmd() {
    // rate_limit_tier default_claude_max_20x, extra_usage_cmd_enabled=false
    // → LOGIN_SWITCH.
}
#[test]
fn max20x_error_upsell_extra_usage_finish_with_cmd() {}
#[test]
fn pro_error_upsell_upgrade_without_cmd() {}
#[test]
fn team_error_upsell_admin_vs_member() {
    // team + cmd enabled: billing access → EXTRA_USAGE_FINISH; member →
    // EXTRA_USAGE_ADMIN; cmd disabled → None.
}
#[test]
fn pro_with_cmd_enabled_falls_through_to_upgrade_or_extra() {
    // !team && cmd enabled → skips the UPGRADE arm → UPGRADE_OR_EXTRA.
}

// — copy byte-locks —
#[test]
fn warning_upsell_copy_is_straight_ascii() {
    assert!(!EXTRA_USAGE_REQUEST.contains('\u{2019}'));
    assert!(!UPGRADE_KEEP_USING.contains('\u{2019}'));
    assert_eq!(EXTRA_USAGE_REQUEST, "/extra-usage to request more");
    assert_eq!(UPGRADE_KEEP_USING, "/upgrade to keep using Claude Code");
}
```

- [ ] **Step 2: RED** (`cargo test -p tui rate_limit_messages`) — observe compile/assert failures.
- [ ] **Step 3: Implement** per the sketches above; update module docs (`:20-38`) to mark the three documented gaps CLOSED with a pointer to this batch; update `streaming.rs:142`.
- [ ] **Step 4: GREEN**: `cargo test -p tui` (full crate — streaming tests must still pass; fix their call sites with `&SubscriptionSnapshot::default()`).
- [ ] **Step 5: clippy**: `cargo clippy -p tui --all-targets --no-deps -- -D warnings`.
- [ ] **Step 6: Commit**

```bash
git add lingxi-code/tui/src/rate_limit_messages.rs lingxi-code/tui/src/streaming.rs
git commit -m "feat(tui): subscription-granular rate-limit upsell arms (getWarningUpsellText + getUpsellMessage ports)

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 7: Gemini `wait_for_file_active` polling driver

**Files:**
- Modify: `lingxi-code/llm-client/src/client.rs` (next to `upload_file` at :225)
- Create: `lingxi-code/llm-client/tests/gemini_files_poll_test.rs`

No TS/codex ground truth exists for polling cadence (batch 3 deliberately left polling to callers). This is a documented CONVENIENCE with our own defaults — NOT byte-pinned. Defaults: poll every 2s, give up after 300s.

- [ ] **Step 1: Write failing tests.** Use `#[tokio::test(start_paused = true)]` so `tokio::time::sleep` auto-advances — tests run instantly. Build a minimal scripted transport in the test file (read `tests/gemini_files_test.rs` first — if its `ScriptedTransport` is reusable via a shared `tests/` helper module, reuse it; otherwise declare a local one returning a queued sequence of responses):

```rust
#[tokio::test(start_paused = true)]
async fn wait_for_file_active_polls_until_active() {
    // Transport scripted: PROCESSING, PROCESSING, ACTIVE.
    // Client configured with the gemini provider exactly as the
    // upload_file tests do (copy their client fixture).
    let file = client
        .wait_for_file_active("gemini-2.0-flash", "files/abc", &transport, FileActivationPoll::default())
        .await
        .expect("active");
    assert_eq!(file.state, "ACTIVE");
    assert_eq!(transport.request_count(), 3);
}

#[tokio::test(start_paused = true)]
async fn wait_for_file_active_fails_fast_on_failed_state() {
    // Sequence: PROCESSING, FAILED → Err(InvalidRequest), exactly 2 requests.
    let err = client
        .wait_for_file_active("gemini-2.0-flash", "files/abc", &transport, FileActivationPoll::default())
        .await
        .expect_err("failed state");
    assert!(matches!(err, LlmError::InvalidRequest { .. }));
}

#[tokio::test(start_paused = true)]
async fn wait_for_file_active_times_out() {
    // Endless PROCESSING; max_wait 10s, interval 2s → Err(Transport) after
    // ~5 polls; paused clock makes this instant.
    let poll = FileActivationPoll { interval: Duration::from_secs(2), max_wait: Duration::from_secs(10) };
    let err = client
        .wait_for_file_active("gemini-2.0-flash", "files/abc", &transport, poll)
        .await
        .expect_err("timeout");
    assert!(matches!(err, LlmError::Transport { .. }));
}

#[tokio::test(start_paused = true)]
async fn wait_for_file_active_rejects_non_gemini_family() {
    // Same guard as upload_file: claude model → InvalidRequest, 0 requests.
}
```

- [ ] **Step 2: RED**: `cargo test -p llm-client --test gemini_files_poll_test` → compile failure.

- [ ] **Step 3: Implement** in `client.rs`:

```rust
/// Polling knobs for [`DefaultLlmClient::wait_for_file_active`]. The defaults
/// (2s interval, 300s budget) are this crate's own convenience choice — there
/// is no claude-code/codex counterpart to pin against; tune per call site.
#[derive(Debug, Clone, Copy)]
pub struct FileActivationPoll {
    /// Delay between consecutive status requests.
    pub interval: Duration,
    /// Total budget before giving up with [`LlmError::Transport`].
    pub max_wait: Duration,
}

impl Default for FileActivationPoll {
    fn default() -> Self {
        Self { interval: Duration::from_secs(2), max_wait: Duration::from_secs(300) }
    }
}

impl DefaultLlmClient {
    /// Poll the Gemini File API until `file_name` leaves `PROCESSING`.
    ///
    /// Convenience layer over [`Self::upload_file`]'s "callers poll
    /// `file_status_request` until `ACTIVE`" contract: same
    /// Gemini-family-only guard, same authenticated request path. Returns
    /// the final [`GeminiFile`] on `ACTIVE`; `FAILED` →
    /// [`LlmError::InvalidRequest`]; budget exhausted →
    /// [`LlmError::Transport`]. State strings are matched verbatim
    /// (tolerant decoder convention — any unknown state keeps polling
    /// until the budget runs out).
    pub async fn wait_for_file_active(
        &self,
        model_or_alias: &str,
        file_name: &str,
        transport: &dyn Transport,
        poll: FileActivationPoll,
    ) -> Result<crate::GeminiFile, LlmError> {
        let deadline = tokio::time::Instant::now() + poll.max_wait;
        loop {
            let file = /* resolve provider + auth exactly like upload_file,
                          send gemini_files::file_status_request, decode with
                          gemini_files::parse_file_status */;
            match file.state.as_str() {
                "ACTIVE" => return Ok(file),
                "FAILED" => {
                    return Err(LlmError::InvalidRequest {
                        message: format!("gemini file processing failed: {file_name}"),
                    })
                }
                _ => {}
            }
            if tokio::time::Instant::now() + poll.interval > deadline {
                return Err(LlmError::Transport {
                    message: format!(
                        "gemini file did not become ACTIVE within {}s",
                        poll.max_wait.as_secs()
                    ),
                });
            }
            tokio::time::sleep(poll.interval).await;
        }
    }
}
```

The `/* resolve provider … */` body: copy `upload_file`'s provider/credential resolution verbatim (same family guard rejecting non-`GeminiGenerateContent` providers, same auth header application) — read `client.rs:225-300` and mirror it; extract a shared private helper if the duplication exceeds ~15 lines.

- [ ] **Step 4: GREEN**: `cargo test -p llm-client --test gemini_files_poll_test` + existing `cargo test -p llm-client --test gemini_files_test`.
- [ ] **Step 5: clippy**: `cargo clippy -p llm-client --all-targets --no-deps -- -D warnings`.
- [ ] **Step 6: Commit**

```bash
git add lingxi-code/llm-client/src/client.rs lingxi-code/llm-client/tests/gemini_files_poll_test.rs
git commit -m "feat(llm-client): wait_for_file_active polling driver for gemini File API uploads

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 8: Spec rev2.9 + closed-item records

**Files:**
- Modify: `docs/superpowers/specs/2026-06-10-llm-client-engine-adoption-design.md`

- [ ] **Step 1: Update the spec** to rev2.9:
  - Batch 4 delivered: subscription plumbing (traits snapshot + desktop background profile/roles fetch + TUI threading) and the full upsell-arm ports; early-warning header port (surpassed-threshold + time-relative fallback + final-status semantics) — note the TS `allowed_warning`-downgrade subtlety and the missing-status divergence-guard; `wait_for_file_active` convenience (defaults documented as non-pinned).
  - Mobile body_bytes item: CLOSED by audit (copy the rationale from this plan's "Closed / blocked items" section verbatim).
  - Remaining list (honest): OpenAiResponses real-traffic validation (blocked: no key); orchestrator `is_enterprise`/`SubscriptionContext` retry-gate + 429-message wiring onto the live snapshot; `/mock-limits` + interactive rate-limit options menu (would unlock the `OPENING_OPTIONS` upsell arm and `shouldShowUpsell` mock arm); TS `rawUtilization` statusline export (not ported — no statusline consumer).
- [ ] **Step 2: Commit**

```bash
git add docs/superpowers/specs/2026-06-10-llm-client-engine-adoption-design.md
git commit -m "docs(spec): rev2.9 — future-work batch 4 (subscription upsell, early warning, upload polling)

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Final verification (after all tasks)

1. `cargo build -p mock_stdio_mcp` (fresh-worktree gotcha), then full `cargo test --workspace` — expect 0 failures (known flakes: posix fs_watch under churn).
2. Clippy battery: `for p in traits orchestrator anthropic-oauth engine-desktop tui llm-client platform-common; do cargo clippy -p $p --all-targets --no-deps -- -D warnings || break; done` (use actual package names from each Cargo.toml).
3. Frozen-crate checks: `git diff main -- lingxi-code/traits | grep -c '^-[^-]'` → 0; `git diff main -- lingxi-code/protocol | grep -c '^-[^-]'` → 0 (protocol untouched this batch — diff should be empty entirely).
4. Commit-trailer audit: `git log main..HEAD --format='%(trailers:key=Co-Authored-By)' | sort -u` → exactly one trailer value.
