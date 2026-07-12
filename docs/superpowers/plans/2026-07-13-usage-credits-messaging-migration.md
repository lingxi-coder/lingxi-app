# `/usage-credits` Messaging Migration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Advance `tui/src/rate_limit_messages.rs` (a faithful port of claude-code's rate-limit
message functions) from the 2.1.205 `/extra-usage` baseline to CC 2.1.206's full
`/usage-credits` messaging — byte-exact — including the new server-driven inputs
(`upgradePaths`, `overageInUse`, `overage-period-*` utilization, the expanded
`overageDisabledReason` taxonomy, `seven_day_overage_included`) plumbed end-to-end.

**Architecture:** Three layers. (1) **Data plumbing** — parse 4 new
`anthropic-ratelimit-unified-*` response headers into `llm_client` `RateLimitInfo`, thread
them across `TurnEvent::RateLimit` into the TUI composer's `RateLimitInfo`, and add the
`SubscriptionSnapshot` predicates + `tengu_*` flags the 206 branches consult. (2) **Message
functions** — rewrite the six composer functions (`getRateLimitMessage`/`Fdu`,
`getLimitReachedText`/`Ucg`, `getEarlyWarningText`/`jcg`, `getWarningUpsellText`/`Wcg`,
`getUsingOverageText`/`a7n`, `getUpsellMessage`/`Gid`, plus helpers `qcg`/`lhe`) to their
206 bodies. (3) **Integration + byte-exact golden tests.** The composer is already
Anthropic/claude.ai-gated, so a non-Anthropic build stays byte-identical.

**Tech Stack:** Rust (crates: `llm-client`, `traits`, `tui`, `telemetry`); the oracle is the
2.1.206 binary at `/Users/luolingfeng/.local/share/claude/versions/2.1.206`.

## Global Constraints

- **Byte-exact vs 2.1.206.** Every ported string/branch must match the binary. Oracle-verify
  with `grep -abo` + python latin-1 slice; handle `’` (U+2019) / `…` (U+2026) and `·`
  (U+00B7 middle dot) — the composer uses REAL chars (runtime form), the binary stores them
  as literal chars in the minified source shown here.
- **Never touch the 4 user dirty files:** `llm-client/data/models-dev/openrouter.json`,
  `llm-client/src/catalog/presets.rs`, `tools/agent/src/agent.rs`,
  `tools/agent/src/agent_test.rs`. A task needing one is BLOCKED, not forced.
- **Inert-when-non-Anthropic:** the composer only runs behind `SubscriptionSnapshot` /
  `has_claude_ai_billing_access` / `is_ant`; non-Anthropic builds must stay byte-identical.
- **New `tengu_*` flags default-OFF** via `telemetry::flag_bool(key, false)`; a default build
  must be byte-identical to pre-change for the flag-gated branches
  (`tengu_pewter_summit`, `tengu_idle_amber_finch`, `tengu_coral_beacon`).
- **LingXi rebrand kept:** where CC says "Claude Code", the port keeps "LingXi" (e.g.
  `/upgrade to keep using LingXi`) — rebrand carve-out. MODEL names ("Fable 5 limit",
  "Opus limit", "Sonnet limit") are NOT rebranded (they name models, not the product).
- **No git remote → commit to `main` locally.** Commit trailer ends:
  `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`.
- **Never run `cargo fmt`.**
- **Known unrelated pre-existing failure:** `orchestrator::compaction_hooks_test::failing_or_blocking_pre_compact_hook_does_not_break_compaction` — ignore.

## The decoded 2.1.206 source (authoritative reference for every task)

These are the minified 2.1.206 function bodies extracted from the binary. Tasks port them
verbatim. `Jie(x,!0)` = `format_reset_time(x, true, true)` (the port's `fmt_reset`). `·` below
is U+00B7. Helper predicate mapping (Phase 0 pins the deep ones):

- `A5()` = `Uc()?.billingType==="usage_based"` → `sub.billing_type.as_deref()==Some("usage_based")`
- `Fs()` = subscription type → `sub.subscription_type`
- `tC()` = `Bo() && (Fs()∈{max,pro} || orgRole∈{admin,billing,owner,primary_owner})` → **≈ existing `sub.has_claude_ai_billing_access()`** (Phase 0 Task 1 verifies byte-equivalence)
- `QJe()` = `Bo() && billingType∈{stripe_subscription, stripe_subscription_contracted, apple_subscription, google_play_subscription}` → **existing `sub.is_overage_provisioning_allowed()`**
- `Pee()` = `flag_bool("tengu_idle_amber_finch", false)`
- `Ze("tengu_coral_beacon",!1)` = `flag_bool("tengu_coral_beacon", false)`
- `Ze("tengu_pewter_summit",!1)` = `flag_bool("tengu_pewter_summit", false)`
- `Bo()` = is-claude.ai-OAuth (scopes) → `sub.is_subscriber` (Phase 0 Task 1 confirms)
- `Xte.isEnabled()` = the `/usage-credits`(ex-`/extra-usage`) command enabled → existing
  `sub.is_extra_usage_command_enabled(disable_env_truthy)` path in `compose_rate_limit`
- `ZA(t)`, `WBe()`, `B5()`, `Eyt()`, the overage-included-models set (`O9e()`/`VQ`/`ei`) —
  **deep-internal; Phase 0 Task 2 decodes + pins them.**

```js
// getRateLimitMessage — Fdu(e,t): only the overage allowed_warning text changed vs the port
function Fdu(e,t){
  if(e.isUsingOverage){
    if(e.overageStatus==="allowed_warning")
      return {message:`You're close to your ${A5()?"usage limit":"usage credit limit"}`,severity:"warning"};
    return null}
  if(e.status==="rejected") return {message:Ucg(e,t),severity:"error"};
  if(e.status==="allowed_warning"){ if(e.utilization!==void 0&&e.utilization<0.7)return null; /* → jcg early-warning */ }
}

// formatLimitReachedText — lhe(e,t,r): NO USER_TYPE==='ant' branch in 206 (r unused)
function lhe(e,t,r){return `You've hit your ${e}${t}`}

// limit-name helper — qcg(e,t,r)
function qcg(e,t,r){
  if(e.rateLimitType==="seven_day_sonnet"){let n=Fs();return lhe(n==="pro"||n==="enterprise"?"weekly limit":"Sonnet limit",t,r)}
  if(e.rateLimitType==="seven_day_opus")return lhe("Opus limit",t,r);
  if(e.rateLimitType==="seven_day_overage_included")return lhe("Fable 5 limit",t,r);
  if(e.rateLimitType==="seven_day")return lhe("weekly limit",t,r);
  if(e.rateLimitType==="five_hour")return lhe("session limit",t,r);
  return null}

// getLimitReachedText — Ucg(e,t)   (Hqi = {"org_level_disabled_until","org_spend_cap_reached"})
function Ucg(e,t){
  let r=A5(),n=tC(),o=n?"":" · contact your admin to increase it",
      i=e.resetsAt,s=i?Jie(i,!0):void 0,a=e.overageResetsAt?Jie(e.overageResetsAt,!0):void 0,
      l=s?` · resets ${s}`:"",c=qcg(e,l,t);
  if(!r&&e.overageDisabledReason&&c&&!Hqi.has(e.overageDisabledReason)&&(e.rateLimitType==="seven_day_overage_included"||!(ZA(t)&&WBe()&&!B5())))return c;
  if(!r&&e.overageDisabledReason&&Hqi.has(e.overageDisabledReason)){
    let u=Fs();
    if(u==="team"||u==="enterprise")return lhe("org's monthly spend limit",n?" · visit claude.ai/admin-settings/usage":" · run /usage-credits to ask your admin for a higher limit",t);
    return lhe(n?"monthly spend limit":"org's monthly spend limit",n?" · raise it at claude.ai/settings/usage":" · ask your admin to raise it at claude.ai/settings/usage",t)}
  if(e.overageStatus==="rejected"){
    let u="";
    if(i&&e.overageResetsAt){if(i<e.overageResetsAt)u=` · resets ${s}`;else u=` · resets ${a}`}
    else if(s)u=` · resets ${s}`;else if(a)u=` · resets ${a}`;
    if(e.overageDisabledReason==="out_of_credits"){
      if(r)return n?"Your org is out of usage · add funds to continue":"Your org is out of usage · contact your admin";
      return `You're out of usage credits${u}`}
    if(e.overageDisabledReason&&Hqi.has(e.overageDisabledReason)){let d=a?` · resets ${a}`:"";return lhe("org's monthly usage limit",d,t)}
    if(e.overageDisabledReason==="seat_tier_level_disabled"||e.overageDisabledReason==="seat_tier_zero_credit_limit")return `Your seat type doesn't include ${r?"usage":"usage credits"}`;
    if(e.overageDisabledReason==="org_service_level_disabled")return "This service is disabled for your org";
    if(e.overageDisabledReason==="member_level_disabled"||e.overageDisabledReason==="member_zero_credit_limit")return "Your usage allocation has been disabled by your admin · run /usage-credits to ask your admin for a higher limit";
    if(e.overageDisabledReason==="group_zero_credit_limit")return "Your group's usage limit is set to $0 · run /usage-credits to ask your admin for a higher limit";
    if(r)return lhe("usage limit",o,t);
    return lhe("limit",u,t)}
  if(c)return c;
  if(r)return lhe("usage limit",o,t);
  return lhe("usage limit",l,t)}

// getEarlyWarningText — jcg(e)
function jcg(e){
  let t=null;
  switch(e.rateLimitType){
    case"seven_day":t="weekly limit";break; case"five_hour":t="session limit";break;
    case"seven_day_opus":t="Opus limit";break; case"seven_day_sonnet":t="Sonnet limit";break;
    case"seven_day_overage_included":t="Fable 5 limit";break;
    case"overage":t=A5()?"usage":"usage credits";break; case void 0:return null}
  let r=e.utilization?Math.floor(e.utilization*100):void 0,
      n=e.rateLimitType==="overage"&&A5(),
      o=e.resetsAt&&!n?Jie(e.resetsAt,!0):void 0,
      i=Wcg(e.rateLimitType);
  if(r&&o){let a=`You've used ${r}% of your ${t} · resets ${o}`;return i?`${a} · ${i}`:a}
  if(r){let a=`You've used ${r}% of your ${t}`;return i?`${a} · ${i}`:a}
  if(e.rateLimitType==="overage")t=A5()?"usage limit":"usage credit limit";
  if(o){let a=`Approaching ${t} · resets ${o}`;return i?`${a} · ${i}`:a}
  let s=`Approaching ${t}`;return i?`${s} · ${i}`:s}

// getWarningUpsellText — Wcg(e)   (r=Uc()?.hasExtraUsageEnabled===true, n=tC(), QJe()=overage-provisioning, Pee=tengu_idle_amber_finch)
function Wcg(e){
  let t=Fs(),r=Uc()?.hasExtraUsageEnabled===!0,n=tC();
  if(t==="team"||t==="enterprise"){
    if(!r&&QJe())return n?"Run /usage-credits to turn on extra usage for your org":"Run /usage-credits to ask your admin for more";
    if(r&&e==="overage")return n?"Run /usage-credits to raise the cap":"Run /usage-credits to ask your admin for more";
    return null}
  if(e==="five_hour"&&(t==="pro"||t==="max")&&!Pee())return "/upgrade to keep using Claude Code";  // → LingXi
  return null}

// getUsingOverageText — a7n(e,t)   (t = model; O9e()=overage-included models set, VQ/ei map a model)
function a7n(e,t){
  let r=e.resetsAt?Jie(e.resetsAt,!0):"",n="";
  if(e.rateLimitType==="five_hour")n="session limit";
  else if(e.rateLimitType==="seven_day")n="weekly limit";
  else if(e.rateLimitType==="seven_day_opus")n="Opus limit";
  else if(e.rateLimitType==="seven_day_sonnet"){let a=Fs();n=a==="pro"||a==="enterprise"?"weekly limit":"Sonnet limit"}
  let o=A5();
  if(!n&&!o&&t){let a=VQ(ei(t));if(a&&O9e().includes(a)){let l=r?` · Your ${a} limit resets ${r}`:"";return `Now using usage credits for ${a}${l}`}}
  let i=o?"your usage allocation":"usage credits";
  if(!n)return `Now using ${i}`;
  let s=r&&!o?` · Your ${n} resets ${r}`:"";
  return `You're now using ${i}${s}`}

// getUpsellMessage — Gid({shouldShowUpsell:e,isMax20x:t,isExtraUsageCommandEnabled:r,shouldAutoOpenRateLimitOptionsMenu:n,isTeamOrEnterprise:o,hasBillingAccess:i,serverHidesUpgrade:s,serverHidesOverage:a,spendLimitNudgePath:l})
function Gid({...}){
  if(!e)return null;
  if(n)return "Opening your options…";                                   // menu arm — structurally false in the TUI
  if(l)return "/usage-credits to adjust your monthly spend limit.";
  let c=r&&!a;                                                           // isExtraUsageCommandEnabled && !serverHidesOverage
  if(t){ if(c)return "/usage-credits to finish what you’re working on."; return "/login to switch to an API usage-billed account." }
  if(o){ if(!c)return "Your admin can enable extra usage at claude.ai/admin-settings/usage."; if(i)return "/usage-credits to finish what you’re working on."; return "/usage-credits to request more usage from your admin." }
  if(s){ if(c)return "/usage-credits to finish what you’re working on."; return null }
  if(!c)return "/upgrade to increase your usage limit.";
  return "/upgrade or /usage-credits to finish what you’re working on." }

// Gid input derivations at the call site (Tdo), from ClaudeAILimits `mle=upgradePaths`:
//   serverHidesUpgrade(param) = (mle!==void 0 && !mle.includes("upgrade_plan")) || Pee()
//   serverHidesOverage        =  mle!==void 0 && !mle.includes("overage")
//   spendLimitNudgePath       =  flag_bool("tengu_pewter_summit") && !isTeamOrEnterprise && overageDisabledReason==="org_level_disabled_until" && tC() && isExtraUsageCommandEnabled
//   isMax20x                  =  subscription_type==="max" && rate_limit_tier==="default_claude_max_20x"
//   hasBillingAccess          =  tC()
//   shouldAutoOpenRateLimitOptionsMenu = false in the TUI (no interactive options menu)
//   shouldShowUpsell          =  Eyt() || Bo()   (Phase 0 Task 2 pins Eyt; today ≈ sub.is_subscriber)

// ClaudeAILimits header parse (new 206 fields):
//   overageInUse            = header "anthropic-ratelimit-unified-overage-in-use" === "true"
//   upgradePaths            = header "anthropic-ratelimit-unified-upgrade-paths".split(",").map(trim)  (undefined if absent)
//   overagePeriodMonthly    = Number(header "anthropic-ratelimit-unified-overage-period-monthly-utilization")  (if finite → {utilization})
//   overagePeriodChannel    = Number(header "anthropic-ratelimit-unified-overage-period-channel-utilization")  (if finite → {utilization})
//   credits_required error body: error.error.details.error_code==="credits_required" → overageDisabledReason = details.disabled_reason
```

---

## File Structure

- `llm-client/src/model/rate_limit.rs` — add 4 header fields to `RateLimitInfo` + parse them; add `credits_required` error-body → `overage_disabled_reason`.
- `traits/src/orchestrator.rs` — add the 4 fields to the `TurnEvent::RateLimit` variant.
- The `llm_client::RateLimitInfo → TurnEvent::RateLimit` bridge (whichever crate maps it — Task 3 locates it) — thread the 4 fields.
- `traits/src/subscription.rs` — add `is_usage_based_billing()` (`A5`), confirm/adjust `has_claude_ai_billing_access` (`tC`) and `is_overage_provisioning_allowed` (`QJe`), and (Phase 0 Task 2) any new predicate/state the deep branches need.
- `tui/src/rate_limit_messages.rs` — add the 4 fields to the composer `RateLimitInfo`; rewrite the six functions + helpers; new upsell string consts; byte-exact tests. (Primary file.)
- `tui/src/chat_widget.rs:988-999` — populate the 4 new composer `RateLimitInfo` fields from `TurnEvent::RateLimit`.

---

## Phase 0 — Foundations (predicates, flags, deep-internal decode)

### Task 1: Confirm `A5`/`tC`/`QJe`/`Bo` ↔ existing `SubscriptionSnapshot` mapping; add `is_usage_based_billing`

**Files:**
- Modify: `traits/src/subscription.rs`
- Test: inline `#[cfg(test)]` in `traits/src/subscription.rs`

**Interfaces:**
- Produces: `SubscriptionSnapshot::is_usage_based_billing(&self) -> bool` (`A5` = `billing_type=="usage_based"`).

- [ ] **Step 1: Write the failing test**
```rust
#[test]
fn is_usage_based_billing_matches_a5() {
    let mut s = SubscriptionSnapshot::default();
    assert!(!s.is_usage_based_billing());
    s.billing_type = Some("usage_based".into());
    assert!(s.is_usage_based_billing());
    s.billing_type = Some("stripe_subscription".into());
    assert!(!s.is_usage_based_billing());
}
```
- [ ] **Step 2: Run to verify it fails** — `cargo test -p traits is_usage_based_billing_matches_a5` → FAIL (method missing).
- [ ] **Step 3: Implement**
```rust
/// Port of `A5()` (`Uc()?.billingType==="usage_based"`) — selects "usage limit"
/// vs "usage credit limit" wording in the 2.1.206 rate-limit messages.
#[must_use]
pub fn is_usage_based_billing(&self) -> bool {
    self.billing_type.as_deref() == Some("usage_based")
}
```
Then, in a doc comment on `has_claude_ai_billing_access`, record: "Also serves as
`tC()` / `hasBillingAccess` in the 2.1.206 `getUpsellMessage` (`Bo()&&(max|pro || orgRole∈…)`);
byte-equivalent given `Bo()≈is_subscriber`." Confirm `is_overage_provisioning_allowed`
already lists all four `QJe` billing types (it does: stripe/stripe_contracted/apple/google_play).
- [ ] **Step 4: Run to verify pass** — `cargo test -p traits` → green.
- [ ] **Step 5: Commit**
```bash
git add traits/src/subscription.rs
git commit -m "feat(subscription): is_usage_based_billing (A5) + document tC/QJe mapping for 206 rate-limit msgs

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

### Task 2: Decode & pin the deep-internal predicates (`ZA`,`WBe`,`B5`,`Eyt`, overage-included-model set)

**Files:**
- Modify: `traits/src/subscription.rs` (add the pinned predicates/state) OR
  `tui/src/rate_limit_messages.rs` (if a predicate is composer-local) — decided by what each consumes.
- Test: inline unit tests for each pinned predicate.

These predicates gate two branches only: (a) `Ucg`'s first guard
(`!(ZA(t)&&WBe()&&!B5())`), and (b) `a7n`'s "Now using usage credits for `${model}`" arm
(`O9e().includes(VQ(ei(t)))`), and (c) `Gid`'s `shouldShowUpsell` (`Eyt()||Bo()`).

- [ ] **Step 1: Decode each from the binary** (record the exact body in the commit message).
Run, for each symbol, against `/Users/luolingfeng/.local/share/claude/versions/2.1.206`:
```bash
python3 - <<'PY'
d=open("/Users/luolingfeng/.local/share/claude/versions/2.1.206","rb").read().decode("latin-1")
for sig in ["function ZA(","function WBe(){","function B5(){","function Eyt(","function O9e(","function VQ(","function ei(","function bIe(","function Vqo(","function Rn(","function DBe(","function x5("]:
    i=d.find(sig); print(f"\n--- {sig} @{i} ---\n{d[i:i+240] if i>=0 else 'NOT FOUND'}")
PY
```
Known so far: `B5()=Rn()!=="firstParty"||!Bo()||DBe()||x5()==="default_claude_zero"`;
`WBe()=bIe()||Vqo()`. Map each leaf (`Rn`=deployment/first-party, `DBe`, `x5`=rate_limit_tier,
`bIe`/`Vqo`=overage-consent flags) to either an existing `SubscriptionSnapshot` field, a
`telemetry::flag_bool`, or a documented faithful default when the port has no source (record
the default + why, exactly like the existing `OPENING_OPTIONS` unreachable-menu note).
- [ ] **Step 2: Write failing tests** — one per pinned predicate, asserting the decoded truth
table (e.g. `first_party_default → B5()==false path`, `usage-included model in set → arm fires`).
- [ ] **Step 3: Implement** the predicates as `SubscriptionSnapshot` methods / composer-local
fns with the decoded bodies. For any leaf with no port data source, implement the documented
faithful default and add a `// 206 <sym>: <default> because <the port lacks X>` comment.
- [ ] **Step 4: Run** — `cargo test -p traits -p tui <the new tests>` → green.
- [ ] **Step 5: Commit** (`feat(subscription): pin ZA/WBe/B5/Eyt/overage-included predicates for 206 …`).

### Task 3: `tengu_*` flag gates (`tengu_pewter_summit`, `tengu_idle_amber_finch`, `tengu_coral_beacon`)

**Files:**
- Modify: `tui/src/rate_limit_messages.rs` (a small `flags` sub-module of thin wrappers)
- Test: inline.

- [ ] **Step 1: Write the failing test**
```rust
#[test]
fn rate_limit_flags_default_off() {
    // With no override env/config, all three 206 flags default to false so the
    // default build is byte-identical to pre-change.
    assert!(!flags::spend_limit_nudge_enabled());
    assert!(!flags::idle_amber_finch());
    assert!(!flags::coral_beacon());
}
```
- [ ] **Step 2: Run to verify it fails** — module missing.
- [ ] **Step 3: Implement**
```rust
/// 2.1.206 statsig flags consulted by the rate-limit message composer. All
/// default-OFF (`telemetry::flag_bool(_, false)`) so a default build is
/// byte-identical to the pre-migration behaviour for the flag-gated branches.
mod flags {
    pub fn spend_limit_nudge_enabled() -> bool { telemetry::flag_bool("tengu_pewter_summit", false) }
    pub fn idle_amber_finch() -> bool { telemetry::flag_bool("tengu_idle_amber_finch", false) }   // Pee()
    pub fn coral_beacon() -> bool { telemetry::flag_bool("tengu_coral_beacon", false) }
}
```
- [ ] **Step 4: Run** — `cargo test -p tui rate_limit_flags_default_off` → PASS.
- [ ] **Step 5: Commit** (`feat(tui): default-off tengu flags for 206 rate-limit composer`).

---

## Phase 1 — Data plumbing (new header fields end-to-end)

### Task 4: `llm_client::RateLimitInfo` — 4 new header fields + parse

**Files:**
- Modify: `llm-client/src/model/rate_limit.rs` (struct ~line 372 + the parse fn)
- Test: inline `#[cfg(test)]`.

**Interfaces:**
- Produces (on `llm_client::model::rate_limit::RateLimitInfo`): `pub overage_in_use: bool`,
  `pub upgrade_paths: Option<Vec<String>>`, `pub overage_period_monthly_utilization: Option<f64>`,
  `pub overage_period_channel_utilization: Option<f64>`.

- [ ] **Step 1: Write the failing test** — build a headers map with the 4 new headers and
assert the parse:
```rust
#[test]
fn parses_206_overage_headers() {
    let h = /* header map builder used by sibling tests */
        .with("anthropic-ratelimit-unified-overage-in-use", "true")
        .with("anthropic-ratelimit-unified-upgrade-paths", "upgrade_plan, overage")
        .with("anthropic-ratelimit-unified-overage-period-monthly-utilization", "0.42")
        .with("anthropic-ratelimit-unified-overage-period-channel-utilization", "0.10");
    let info = parse_rate_limit_info(&h).unwrap();   // use the crate's actual parse entry point
    assert!(info.overage_in_use);
    assert_eq!(info.upgrade_paths.as_deref(), Some(&["upgrade_plan".to_string(), "overage".to_string()][..]));
    assert_eq!(info.overage_period_monthly_utilization, Some(0.42));
    assert_eq!(info.overage_period_channel_utilization, Some(0.10));
}
```
(Find the exact parse entry point + header-map helper by reading the existing
`#[cfg(test)]` tests in `rate_limit.rs` — reuse their pattern.)
- [ ] **Step 2: Run to verify it fails** — fields missing.
- [ ] **Step 3: Implement**
  1. Add the 4 fields to `RateLimitInfo` (doc each with its header name; `#[serde(default)]` if the struct is serde).
  2. In the parser, after the existing header reads, add (byte-faithful to the JS parse):
```rust
    let overage_in_use =
        header_value(headers, "anthropic-ratelimit-unified-overage-in-use") == Some("true");
    let upgrade_paths = header_value(headers, "anthropic-ratelimit-unified-upgrade-paths")
        .map(|v| v.split(',').map(|s| s.trim().to_string()).collect());
    let overage_period_monthly_utilization =
        header_value(headers, "anthropic-ratelimit-unified-overage-period-monthly-utilization")
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|n| n.is_finite());
    let overage_period_channel_utilization =
        header_value(headers, "anthropic-ratelimit-unified-overage-period-channel-utilization")
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|n| n.is_finite());
```
  (Match the exact `header_value`/accessor helper the file already uses.)
- [ ] **Step 4: Run** — `cargo test -p llm-client parses_206_overage_headers` then `cargo test -p llm-client` → green.
- [ ] **Step 5: Commit** (`feat(llm-client): parse 206 overage-in-use/upgrade-paths/period-utilization headers`).

### Task 5: `credits_required` error-body → `overage_disabled_reason`

**Files:**
- Modify: `llm-client/src/model/rate_limit.rs` (or the error-classification site — Task locates it via `credits_required` / `disabled_reason`)
- Test: inline.

Port `Nqi(e)`: `if error.error.details.error_code==="credits_required"` → set
`overage_disabled_reason = details.disabled_reason` (when a string). 

- [ ] **Step 1: Write the failing test** — feed an error body
`{"error":{"error":{"details":{"error_code":"credits_required","disabled_reason":"out_of_credits"}}}}`
and assert the derived `overage_disabled_reason == Some("out_of_credits")` and an
`error_code`/flag marking `credits_required` (add a `pub credits_required: bool` field to
`RateLimitInfo` if the composer's `jid` gate needs it — see Task 11's `jid`).
- [ ] **Step 2–5:** run-fail → implement the JSON dig (reuse the crate's error-body type) → run-pass → commit.

### Task 6: Thread the new fields through `TurnEvent::RateLimit` + the TUI composer `RateLimitInfo`

**Files:**
- Modify: `traits/src/orchestrator.rs` (`TurnEvent::RateLimit` variant ~line 1287)
- Modify: the `llm_client::RateLimitInfo → TurnEvent::RateLimit` bridge (locate with
  `grep -rn "TurnEvent::RateLimit {" --include=*.rs` in `client-adapter`/`orchestrator`)
- Modify: `tui/src/rate_limit_messages.rs` (add the 4 fields to the composer `RateLimitInfo`,
  `#[derive(Default)]` covers them) + `tui/src/chat_widget.rs:999` (populate them)
- Test: `traits/src/orchestrator_test.rs` round-trip + a `chat_widget` apply test.

**Interfaces:**
- Produces (on `tui::rate_limit_messages::RateLimitInfo`): `pub overage_in_use: bool`,
  `pub upgrade_paths: Option<Vec<String>>`, `pub overage_period_monthly_utilization: Option<f64>`,
  `pub overage_period_channel_utilization: Option<f64>`, `pub credits_required: bool`.

- [ ] **Step 1: Write the failing test** — extend the `orchestrator_test.rs` `TurnEvent::RateLimit`
round-trip to carry the new fields and assert they survive; add/extend a `chat_widget`
`apply_rate_limit` test that a `TurnEvent::RateLimit` with `upgrade_paths` reaches the composer input.
- [ ] **Step 2: Run to verify it fails.**
- [ ] **Step 3: Implement** — add the fields to the variant, the bridge mapping, and the
`chat_widget.rs:999` `RateLimitInfo { … }` literal; add the 5 fields to the composer struct.
- [ ] **Step 4: Run** — `cargo test -p traits -p tui` → green (+ `cargo build --workspace` to catch every `TurnEvent::RateLimit { … }` construction site; fill new fields with defaults at test/mocks).
- [ ] **Step 5: Commit** (`feat: thread 206 overage header fields to the rate-limit composer`).

---

## Phase 2 — Message function rewrites (byte-exact)

> Each task rewrites ONE composer function to its 206 body (from the reference block above),
> with byte-exact golden tests. Order respects call-deps: helpers (`lhe`,`qcg`) → leaf-users.
> The composer's `is_ant` parameter: 206 `lhe` has **no** `USER_TYPE==='ant'` branch — Task 7
> removes it from `format_limit_reached_text` (keep the param threading only if other call
> sites need it; otherwise drop it and its plumbing).

### Task 7: `format_limit_reached_text` (`lhe`) — drop the `ant` branch

**Files:** Modify `tui/src/rate_limit_messages.rs`; Test inline.
- [ ] **Step 1: Failing test**
```rust
#[test]
fn lhe_206_has_no_ant_branch() {
    assert_eq!(format_limit_reached_text("session limit", " · resets 3pm", true),  "You've hit your session limit · resets 3pm");
    assert_eq!(format_limit_reached_text("weekly limit", "", false), "You've hit your weekly limit");
}
```
- [ ] **Step 2: Run — FAIL** (current code appends the `#briarpatch-cc` text when `is_ant`).
- [ ] **Step 3: Implement**
```rust
fn format_limit_reached_text(limit: &str, reset_message: &str, _is_ant: bool) -> String {
    // 2.1.206 `lhe(e,t,r)` = `You've hit your ${e}${t}` — the USER_TYPE==='ant'
    // #briarpatch-cc/reset-limits branch was removed in 206.
    format!("You've hit your {limit}{reset_message}")
}
```
- [ ] **Step 4: Run — PASS**; `cargo test -p tui`.
- [ ] **Step 5: Commit** (`feat(tui): 206 formatLimitReachedText — drop ant #briarpatch branch`).

### Task 8: overage `allowed_warning` text (`Fdu`) + `qcg` limit-name helper

**Files:** Modify `tui/src/rate_limit_messages.rs`; Test inline.
- [ ] **Step 1: Failing tests**
```rust
#[test]
fn overage_close_to_your_uses_limit_name() {
    let mut info = RateLimitInfo { status: Some("rejected".into()), overage_status: Some("allowed_warning".into()), ..Default::default() };
    let mut sub = SubscriptionSnapshot::default();               // not usage_based → "usage credit limit"
    assert_eq!(compose_with(&info, false, &sub, true).unwrap().text, "You're close to your usage credit limit");
    sub.billing_type = Some("usage_based".into());               // A5 → "usage limit"
    assert_eq!(compose_with(&info, false, &sub, true).unwrap().text, "You're close to your usage limit");
    let _ = &mut info;
}
#[test]
fn qcg_maps_206_limit_names() {
    // seven_day_overage_included → "Fable 5 limit"
    assert_eq!(qcg_limit_name(Some("seven_day_overage_included"), &SubscriptionSnapshot::default()), Some("Fable 5 limit"));
}
```
- [ ] **Step 2: Run — FAIL.**
- [ ] **Step 3: Implement** — in `compose_with`, replace the overage `allowed_warning` text with
`format!("You're close to your {}", if sub.is_usage_based_billing() {"usage limit"} else {"usage credit limit"})`; add a `qcg_limit_name(rate_limit_type, sub) -> Option<&'static str>` helper per `qcg` (incl. `seven_day_overage_included → "Fable 5 limit"`).
- [ ] **Step 4: Run — PASS**; `cargo test -p tui`.
- [ ] **Step 5: Commit** (`feat(tui): 206 overage close-to-your ${limitName} + qcg helper`).

### Task 9: `early_warning_text` (`jcg`) rewrite

**Files:** Modify `tui/src/rate_limit_messages.rs`; Test inline.
Port `jcg` verbatim (from the reference block): `seven_day_overage_included → "Fable 5 limit"`;
`overage → A5()?"usage":"usage credits"` (used%) but `A5()?"usage limit":"usage credit limit"` in the
`Approaching` branch; `n = overage && A5()` suppresses the reset for the used% branch; upsell via `Wcg` (Task 10).
- [ ] **Step 1: Failing tests** — cover: `seven_day` used% + reset + upsell; `overage`+A5 used% (no reset, `"…of your usage"`); `overage` not-A5 Approaching (`"Approaching usage credit limit"`); `seven_day_overage_included` (`"…of your Fable 5 limit"`).
- [ ] **Step 2–5:** run-fail → implement `jcg` → run-pass → `cargo test -p tui` → commit.

### Task 10: `warning_upsell` (`Wcg`) rewrite

**Files:** Modify `tui/src/rate_limit_messages.rs`; Test inline.
Port `Wcg`: team/enterprise → `!hasExtraUsageEnabled && QJe() → tC()? "Run /usage-credits to turn on extra usage for your org" : "Run /usage-credits to ask your admin for more"`; `hasExtraUsageEnabled && overage → tC()? "Run /usage-credits to raise the cap" : "Run /usage-credits to ask your admin for more"`; else null. `five_hour && (pro|max) && !idle_amber_finch() → "/upgrade to keep using LingXi"` (rebrand). Replace the module consts `EXTRA_USAGE_REQUEST`/`UPGRADE_KEEP_USING` accordingly (keep `LingXi`).
- [ ] **Step 1: Failing tests** — the four team/ent variants (tC true/false × turn-on/raise-cap) + the pro five_hour upgrade + the flag-on suppression + weekly→None.
- [ ] **Step 2–5:** run-fail → implement `Wcg` (consuming `tC`=`has_claude_ai_billing_access`, `QJe`=`is_overage_provisioning_allowed`, `has_extra_usage_enabled`, `idle_amber_finch` flag) → run-pass → commit.

### Task 11: `limit_reached_text` (`Ucg`) rewrite — the big one

**Files:** Modify `tui/src/rate_limit_messages.rs`; Test inline. Consumes `qcg_limit_name`
(Task 8), `lhe` (Task 7), `A5`/`tC` (Task 1), the deep guard predicates (Task 2), and the
`Hqi` reason set `{"org_level_disabled_until","org_spend_cap_reached"}`.
- [ ] **Step 1: Failing tests** — one per `Ucg` return path (13 paths in the reference block):
`out_of_credits` personal (`"You're out of usage credits · resets …"`), org add-funds, org
contact-admin; `seat_tier_*` (`"Your seat type doesn't include usage credits"` and the A5
`"…usage"` variant); `org_service_level_disabled`; `member_*`; `group_zero_credit_limit`;
the two `Hqi` monthly-spend paths (team/ent visit-admin-settings vs personal raise-it); the
final `r?lhe("usage limit",o,t):lhe("usage limit",l,t)`; and the `c`/first-guard `qcg`
passthrough. Use the exact byte strings from the reference block.
- [ ] **Step 2: Run — FAIL.**
- [ ] **Step 3: Implement** `limit_reached_text` as a verbatim port of `Ucg`. Add a
`const HQI_REASONS: [&str;2] = ["org_level_disabled_until","org_spend_cap_reached"];`. Wire the
first guard's `!(ZA(t)&&WBe()&&!B5())` via the Task-2 predicates (the `t` model param: thread
the composer's active model, or the Task-2-documented default if unavailable).
- [ ] **Step 4: Run — PASS**; `cargo test -p tui`.
- [ ] **Step 5: Commit** (`feat(tui): 206 getLimitReachedText (Ucg) — usage-credits + org/seat/member/group taxonomy`).

### Task 12: `using_overage_text` (`a7n`) rewrite

**Files:** Modify `tui/src/rate_limit_messages.rs`; Test inline.
Port `a7n`: `i = A5()?"your usage allocation":"usage credits"`; `!n → "Now using ${i}"`; else
`"You're now using ${i}${s}"` where `s = resetTime && !A5() ? " · Your ${n} resets ${r}" : ""`;
the `!n && !A5() && model && model∈O9e()` arm → `"Now using usage credits for ${model}${…}"` (Task-2 model set).
- [ ] **Step 1: Failing tests** — `usage credits` no-reset; `you're now using usage credits · Your weekly limit resets …`; A5 → `your usage allocation` (reset suppressed); the model-specific arm.
- [ ] **Step 2–5:** run-fail → implement → run-pass → commit.

### Task 13: `error_upsell` (`Gid`) rewrite + input derivations

**Files:** Modify `tui/src/rate_limit_messages.rs` (rewrite `error_upsell` + new upsell string
consts) and `compose_rate_limit`/`compose_with` (derive the 9 `Gid` inputs); Test inline.
- [ ] **Step 1: Failing tests** — cover each `Gid` return path with the exact strings:
spend-limit-nudge (flag+state), max20x (`c` true/false), team/ent (`!c` admin-enable / `i` finish / request-more), serverHidesUpgrade (`c` finish / null), default (`!c` upgrade / upgrade-or-usage-credits). Assert `shouldAutoOpenRateLimitOptionsMenu` stays false (no `OPENING_OPTIONS`). Include the derivation tests: `serverHidesOverage = upgrade_paths∌"overage"`, `serverHidesUpgrade = (upgrade_paths∌"upgrade_plan") || idle_amber_finch()`, `spendLimitNudgePath` full predicate.
- [ ] **Step 2: Run — FAIL.**
- [ ] **Step 3: Implement**
  1. Replace the `upsell` module consts (migrate `/extra-usage`→`/usage-credits`; add
     `USAGE_CREDITS_FINISH="/usage-credits to finish what you\u{2019}re working on."`,
     `USAGE_CREDITS_ADMIN_ENABLE="Your admin can enable extra usage at claude.ai/admin-settings/usage."`,
     `USAGE_CREDITS_REQUEST_ADMIN="/usage-credits to request more usage from your admin."`,
     `SPEND_LIMIT_NUDGE="/usage-credits to adjust your monthly spend limit."`,
     `UPGRADE_INCREASE="/upgrade to increase your usage limit."`,
     `UPGRADE_OR_USAGE_CREDITS="/upgrade or /usage-credits to finish what you\u{2019}re working on."`,
     keep `LOGIN_SWITCH`, `OPENING_OPTIONS`).
  2. Rewrite `error_upsell` as a verbatim port of `Gid` taking the 9 inputs.
  3. In `compose_with`, derive the inputs (per the reference block's Tdo derivations) from
     `info.upgrade_paths`, `sub`, the flags (Task 3), and `extra_usage_cmd_enabled`; pass
     `shouldAutoOpenRateLimitOptionsMenu=false`. Add the `jid` guard
     (`rate_limit_type=="seven_day_overage_included" || credits_required`) that suppresses the
     upsell (Gid returns null) where 206 does.
- [ ] **Step 4: Run — PASS**; `cargo test -p tui`.
- [ ] **Step 5: Commit** (`feat(tui): 206 getUpsellMessage (Gid) — /usage-credits upsell + server-driven inputs`).

---

## Phase 3 — Integration & verification

### Task 14: End-to-end byte-exact matrix + inert-when-non-Anthropic + oracle re-diff

**Files:** `tui/src/rate_limit_messages.rs` tests; Modify `.omx/state/parity-206-loop.json`
(`RATE_LIMIT_MSG_AUDIT_2026-07-12`) + `lingxi-accepted-divergences` memory.
- [ ] **Step 1: Add an integration test** driving `compose_rate_limit` end-to-end for a
representative matrix (rejected+out_of_credits personal/org; allowed_warning overage; approaching
five_hour pro; team/ent monthly-spend) and assert the full composed `{text, upsell}` byte-exact.
- [ ] **Step 2: Inert test** — a non-subscriber / non-Anthropic `SubscriptionSnapshot` with
absent headers composes exactly what it did pre-migration for the non-billing paths (no
`/usage-credits` leakage). 
- [ ] **Step 3: Oracle re-diff** — `grep -abo` / latin-1 slice every new literal against the
2.1.206 binary; confirm the `·`/`’` bytes.
- [ ] **Step 4: Full verify** — `cargo build --workspace`; `cargo test -p llm-client -p traits -p tui`; `cargo test -p test-harness` if present. Green (modulo the known compaction flake).
- [ ] **Step 5: Update records + commit** — flip `RATE_LIMIT_MSG_AUDIT_2026-07-12` to
"CLOSED (2026-07-13): /usage-credits migration ported byte-exact (full plumbing)"; update the
`lingxi-accepted-divergences` billing-messaging entry to note the reversal (message layer ported;
`/usage-credits` command handler + plan-tier bars still carve-out). Commit
(`feat(tui): 206 /usage-credits messaging migration — integration + close audit`).

---

## Self-Review notes (author)

- **Spec coverage:** command-ref migration (T13), overage rewording+limitName (T8), new upsell
  variants (T10/T13), limitName derivation (T1/T8/T11), fast-mode×usage-credit (folded into the
  header-driven branches; no separate fast-mode surface found in `rate_limit_messages.rs`),
  transition toast (`tengu_pewter_summit` gates the spend-limit-nudge path via T3/T13 — the
  `/extra-usage is now /usage-credits` toast itself is a hidden-command-rename notice, not a
  composer output; recorded as out-of-composer in T14 if it surfaces elsewhere).
- **Deep-internal risk:** T2 is the one research task; if a predicate proves unmappable
  (no port data source), it takes a documented faithful default — surfaced, not guessed.
- **Type consistency:** the composer `RateLimitInfo` gains exactly the 5 fields threaded in T6;
  `A5`=`is_usage_based_billing`, `tC`=`has_claude_ai_billing_access`, `QJe`=`is_overage_provisioning_allowed`
  are used under those names in T8–T13.
