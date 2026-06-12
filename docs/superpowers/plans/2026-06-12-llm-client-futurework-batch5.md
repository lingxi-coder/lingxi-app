# llm-client Future-Work Batch 5 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the actionable rev2.9 leftovers: live-snapshot retry gates, raw-utilization statusline export, overage-transition notification, 429 limits-specific error copy, and isEnvTruthy consolidation.

**Architecture:** Three strands. (1) `provider_adapter` reads `SharedSubscription` live at drive-time so the 429-retry/is_enterprise gates and the (new) terminal 429 message reflect the background-fetched tier instead of the build-time `false`. (2) A `RawUtilization` track parallel to `RateLimitInfo` — parsed per-window (5h/7d) on every response, emitted on change via a NEW additive `OutputEvent::RawUtilization`, landing in the TUI statusline command JSON (`rate_limits` field). (3) TUI overage-transition notice (the `getUsingOverageText` hook arm) riding the existing RateLimit event channel.

**Tech Stack:** Rust workspace (`lingxi-code/`). Ground truth: vendored claude-code TS at `/Users/luolingfeng/Projects/LingXi-Next/claude-code/` (read-only — primary checkout).

---

## Standing constraints (MUST follow — same as batches 1-4)

- **NEVER touch the primary checkout** `/Users/luolingfeng/Projects/LingXi-Next` working tree. All edits in THIS worktree (`.claude/worktrees/llm-client-futurework-b5`). TS ground truth is read from the primary checkout read-only.
- **NEVER `git add -A` / `git add .`** — named paths only.
- **`traits/` + `protocol/` FROZEN-ADDITIVE**: zero removed/modified lines vs main (`git diff main -- lingxi-code/traits | grep -c '^-[^-]'` → 0). New modules / appended enum variants / new default-bodied trait methods are fine; changing existing variants or signatures is NOT.
- No secret material in errors/logs. Strict TDD with observed RED. Clippy `-D warnings --all-targets --no-deps` per touched crate.
- Commit trailer exactly: `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`.
- Fresh-worktree gotcha: `cargo build -p mock_stdio_mcp` before the first full test run. CLI package name is `cli` (not lingxi-cli).
- Copy strings: `rateLimitMessages.ts` copy is STRAIGHT ASCII; separators U+00B7. Never re-type the curly-quote TSX constants.

## Ground-truth references

| Topic | File (primary checkout, READ ONLY) | Key lines |
|---|---|---|
| rawUtilization extraction + storage | `claude-code/src/services/claudeAiLimits.ts` | 145-179 (`RawWindowUtilization`/`extractRawUtilization` — BOTH `-utilization` and `-reset` headers required per window), 476 + 500 (extracted on every header pass AND on 429 errors) |
| Statusline rate_limits shape | `claude-code/src/components/StatusLine.tsx` | 50-65 (`used_percentage = utilization*100`, `resets_at`; window omitted when absent) |
| Overage-transition notification | `claude-code/src/hooks/notifs/useRateLimitWarningNotification.tsx` | whole file (~80 lines): show once on `isUsingOverage` false→true when `!isTeamOrEnterprise \|\| hasBillingAccess`; reset flag when leaving overage; remote mode skipped |
| getUsingOverageText copy | `claude-code/src/services/rateLimitMessages.ts` | 303-331 (limitName per rate_limit_type incl. pro/enterprise sonnet→weekly; bare fallback `"Now using extra usage"`) |
| 429 limits-specific error message | `claude-code/src/services/api/errors.ts` | 480-536 (on 429: update limits from error headers, FORCE status='rejected', `getRateLimitErrorMessage(limits, model)` → assistant error message; null → silent NO_RESPONSE_REQUESTED) |
| isEnvTruthy | `claude-code/src/utils/envUtils.ts` | 32-37 |

## Survey facts (verified — do NOT re-derive)

- `SubscriberState { is_subscriber, is_enterprise }` is copied at adapter construction: `orchestrator/src/provider_adapter.rs:33-37`, consumed at `:280`/`:298` (`apply_beta_header_with_auth` is_subscriber) and `:497-498` (`RetryState { is_subscriber, is_enterprise }`). Built at `apps/engine-desktop/src/lib.rs:1182` (`is_enterprise: false` hard-coded) and `apps/engine-mobile/src/host.rs:362` (both false).
- `DesktopRuntime.subscription: traits::subscription::SharedSubscription` exists (batch 4); slot seeded synchronously, background fetch fills tier/billing/role. Readers: `.read().ok()` → clone → `unwrap_or_default()`.
- `OrchestratorConfig.is_subscriber/is_enterprise` (`config.rs:127/:140`) feed the adapter via the composition root; the `config.rs:134` PARITY-GAP comment says the profile fetch isn't performed — now it IS (batch 4); the comment is stale either way.
- Batch-3 precedent for additive events: `traits::OutputEvent::RateLimit` (9-field variant appended) + `OutputStream::emit_rate_limit` default no-op; orchestrator emit-on-change via `tokio::sync::Mutex<Option<RateLimitInfo>>` dedupe at two seams (`conversation.rs` streaming ~:2129 helper `emit_rate_limit_if_changed` at ~:639, `turn_loop.rs` batched funnel ~:375); TUI bridge `tui/src/events/orchestrator_bridge.rs:206` maps variant → `TurnEvent::RateLimit`; `tui/src/streaming.rs:115-156` apply_event arm composes + dedupes via `state.last_rate_limit_text`.
- `RateLimitInfo::from_headers_at` (orchestrator/src/model/rate_limit.rs) parses per-claim headers already; tolerant helpers `parse_epoch_secs`/`parse_fraction` are module fns.
- TUI statusline command: `tui/src/components/status_line_command.rs:115-147` `build_status_line_input(8 args) -> Value` builds the stdin JSON (NO rate_limits today); config/text plumbed via `AppState.status_line_config` (state.rs:820) + `status_line_text`; invocation site — grep `build_status_line_input(` in `tui/src` (repl.rs/app.rs region).
- TUI has NO TS-style notification system; rate-limit notices render as `RenderedMessage::RateLimit { text, upsell }` pushed from the streaming arm. The overage notice rides the same channel.
- `rate_limit_error_message(info, reset, sub)` + `SubscriptionContext` (orchestrator/src/model/rate_limit.rs:496-525) exist with ZERO call sites — built in batch 3 for exactly this wiring.
- `is_env_truthy` copies: `tui/src/rate_limit_messages.rs:98` (TS-faithful, batch 4), `migrations/src/context.rs:14` (pub), `tools/skill/src/model_override.rs:183`, `tools/shell/src/prompt.rs:100` (takes name, reads env itself), `tools/meta/src/repl_gate.rs:67`, `tools/task/src/task.rs:148`, `compaction/src/thresholds.rs:254`, plus a documented-divergent one in `orchestrator/src/conversation.rs:2758`.
- `/mock-limits` is 1,028 lines of TS (rateLimitMocking.ts + mockRateLimits.ts) — OUT OF SCOPE this batch; record in spec.

## Closed / deferred items (no code task — record in spec, Task 7)

- **`/mock-limits` + interactive rate-limit options menu**: deferred (1k+ TS lines, test-tooling value only; would unlock OPENING_OPTIONS + the shouldShowUpsell mock arm).
- **OpenAiResponses real-traffic validation**: still blocked (no OpenAI key in this environment).

---

### Task 1: traits additive — `OutputEvent::RawUtilization` + `emit_raw_utilization` + `traits::env::is_env_truthy` (FROZEN-ADDITIVE)

**Files:**
- Modify: `lingxi-code/traits/src/orchestrator.rs` (append variant + default method — find the batch-3 `RateLimit` variant and `emit_rate_limit` and mirror their style/doc conventions EXACTLY)
- Create: `lingxi-code/traits/src/env.rs`
- Modify: `lingxi-code/traits/src/lib.rs` (+1 `pub mod env;` line)

- [ ] **Step 1: Read** `traits/src/orchestrator.rs` — the `OutputEvent::RateLimit` variant block and `emit_rate_limit` default no-op + their tests (batch 3). Confirm appending a variant + a default-bodied method is additive (no existing lines change).

- [ ] **Step 2: Failing tests** (RED): in orchestrator.rs's test mod, mirror the existing RateLimit-variant tests:

```rust
#[test]
fn raw_utilization_variant_constructs() {
    let e = OutputEvent::RawUtilization {
        five_hour_utilization: Some(0.42),
        five_hour_resets_at: Some(1_750_000_000),
        seven_day_utilization: None,
        seven_day_resets_at: None,
    };
    assert!(matches!(e, OutputEvent::RawUtilization { .. }));
}

#[tokio::test]
async fn emit_raw_utilization_default_is_noop() {
    // Whatever minimal OutputStream impl the existing emit_rate_limit
    // default-noop test uses — REUSE it; the default body must be callable
    // without an override.
}
```

And in `env.rs`:

```rust
#[test]
fn env_truthy_matrix() {
    for v in ["1", "true", "TRUE", " yes ", "On"] {
        assert!(is_env_truthy(Some(v)), "{v}");
    }
    for v in ["", "0", "false", "off ", "no", "2", "enabled"] {
        assert!(!is_env_truthy(Some(v)), "{v}");
    }
    assert!(!is_env_truthy(None));
}
```

- [ ] **Step 3: Implement.**

`env.rs`:
```rust
//! Shared TS-faithful env-truthiness helper.
//!
//! Port of claude-code `isEnvTruthy` (`utils/envUtils.ts:32-37`): unset/empty
//! ⇒ false; otherwise the lowercased, trimmed value must be one of
//! `1`/`true`/`yes`/`on`. The workspace previously carried ~7 private copies;
//! TS-faithful ones consolidate here (batch 5). Copies with deliberately
//! different semantics stay local and documented.

/// `isEnvTruthy(envVar)` — see module docs.
#[must_use]
pub fn is_env_truthy(value: Option<&str>) -> bool {
    let Some(v) = value else { return false };
    matches!(v.to_lowercase().trim(), "1" | "true" | "yes" | "on")
}
```

Variant (append at the END of `OutputEvent`, doc style copied from `RateLimit`):
```rust
/// Raw per-window unified rate-limit utilization — claude-code
/// `rawUtilization` (`claudeAiLimits.ts:145-179`), tracked on every API
/// response (unlike the warning-gated `RateLimit` fields) and consumed by
/// the statusline command input (`StatusLine.tsx:50-65`). A window is
/// `None` when the response lacked either of its two headers.
RawUtilization {
    /// `anthropic-ratelimit-unified-5h-utilization` (0-1 fraction).
    five_hour_utilization: Option<f64>,
    /// `anthropic-ratelimit-unified-5h-reset` (unix epoch seconds).
    five_hour_resets_at: Option<u64>,
    /// `anthropic-ratelimit-unified-7d-utilization` (0-1 fraction).
    seven_day_utilization: Option<f64>,
    /// `anthropic-ratelimit-unified-7d-reset` (unix epoch seconds).
    seven_day_resets_at: Option<u64>,
},
```

Default method on `OutputStream` (mirror `emit_rate_limit`'s exact arg-list style):
```rust
/// Push a raw-utilization snapshot. Default: no-op (hosts that don't
/// render a statusline ignore it).
async fn emit_raw_utilization(
    &self,
    five_hour_utilization: Option<f64>,
    five_hour_resets_at: Option<u64>,
    seven_day_utilization: Option<f64>,
    seven_day_resets_at: Option<u64>,
) {
    let _ = (five_hour_utilization, five_hour_resets_at, seven_day_utilization, seven_day_resets_at);
}
```
(If `emit_rate_limit` instead takes the variant or a struct — MATCH whatever it does.)

- [ ] **Step 4: GREEN + clippy + frozen check** (`cargo test -p traits`, clippy, `git diff main -- lingxi-code/traits | grep -c '^-[^-]'` → 0). NOTE: appending a variant may break exhaustive `match`es in OTHER crates (tui bridge, client-adapter). `cargo check --workspace` and fix those matches with explicit no-op arms IN THE CONSUMING CRATES (not traits) — those crates are not frozen. Keep the fixes minimal (`OutputEvent::RawUtilization { .. } => {}` style with a one-line comment).
- [ ] **Step 5: Commit** — `git add lingxi-code/traits/src/env.rs lingxi-code/traits/src/lib.rs lingxi-code/traits/src/orchestrator.rs` + any consuming-crate match fixes by name; message `feat(traits): additive RawUtilization event + emit hook + shared is_env_truthy`.

---

### Task 2: Orchestrator — RawUtilization parse + emit-on-change

**Files:**
- Modify: `lingxi-code/orchestrator/src/model/rate_limit.rs` (new struct + parser)
- Modify: `lingxi-code/orchestrator/src/conversation.rs` + `lingxi-code/orchestrator/src/turn_loop.rs` (emit at the same two seams as batch-3 `emit_rate_limit_if_changed` — read that helper first)
- Test: extend `lingxi-code/orchestrator/tests/rate_limit_emit_test.rs`

- [ ] **Step 1: Failing tests** (rate_limit.rs test mod):

```rust
#[test]
fn raw_utilization_requires_both_headers_per_window() {
    // 5h has both → Some; 7d has only utilization → None (ts:174 `util !== null && reset !== null`).
    let headers = h(&[
        ("anthropic-ratelimit-unified-5h-utilization", "0.42"),
        ("anthropic-ratelimit-unified-5h-reset", "1750000000"),
        ("anthropic-ratelimit-unified-7d-utilization", "0.9"),
    ]);
    let raw = RawUtilization::from_headers(&headers);
    assert_eq!(raw.five_hour, Some(RawWindow { utilization: 0.42, resets_at: 1_750_000_000 }));
    assert_eq!(raw.seven_day, None);
}

#[test]
fn raw_utilization_empty_headers_is_default() {
    assert_eq!(RawUtilization::from_headers(&[]), RawUtilization::default());
}
```

- [ ] **Step 2: Implement** (rate_limit.rs, near `RateLimitInfo` — reuse `parse_fraction`/`parse_epoch_secs`):

```rust
/// One window of raw utilization (`RawWindowUtilization`, claudeAiLimits.ts:150-153).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RawWindow {
    /// 0-1 utilization fraction.
    pub utilization: f64,
    /// Unix epoch seconds when the window resets.
    pub resets_at: u64,
}

/// Raw per-window utilization — `extractRawUtilization`
/// (claudeAiLimits.ts:164-179). Tracked on EVERY response with unified
/// headers, independent of warning gating. A window needs BOTH its
/// `-utilization` and `-reset` headers (ts:174).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RawUtilization {
    /// 5-hour window.
    pub five_hour: Option<RawWindow>,
    /// 7-day window.
    pub seven_day: Option<RawWindow>,
}

impl RawUtilization {
    /// Parse from response headers. Empty/absent → `Self::default()`.
    #[must_use]
    pub fn from_headers(headers: &[(String, String)]) -> Self {
        let window = |abbrev: &str| -> Option<RawWindow> {
            let utilization = parse_fraction(headers, &format!("anthropic-ratelimit-unified-{abbrev}-utilization"))?;
            let resets_at = parse_epoch_secs(headers, &format!("anthropic-ratelimit-unified-{abbrev}-reset"))?;
            Some(RawWindow { utilization, resets_at })
        };
        Self { five_hour: window("5h"), seven_day: window("7d") }
    }
}
```
(Adapt the helper-fn signatures to the real `parse_fraction`/`parse_epoch_secs` — check whether they take `(headers, name)` or a pre-fetched value.)

- [ ] **Step 3: Emit seams.** Read `emit_rate_limit_if_changed` (conversation.rs ~:639) and its two call sites. Add a parallel `last_emitted_raw_utilization: tokio::sync::Mutex<Option<RawUtilization>>` slot + `emit_raw_utilization_if_changed(&self, headers, output)` helper: parse, compare-and-set under the mutex, call `output.emit_raw_utilization(...)` only on change AND only when the parse found at least one window (`raw != RawUtilization::default()` — TS replaces `rawUtilization` wholesale including clearing it, BUT clearing only happens via `shouldProcessRateLimits` false which we don't model; document: we never emit the empty snapshot). Call it at BOTH existing seams right next to `emit_rate_limit_if_changed`.
- [ ] **Step 4: Integration test** (extend `tests/rate_limit_emit_test.rs` following its existing fixtures): a response with 5h+7d headers emits once; identical second response does NOT re-emit; changed utilization re-emits.
- [ ] **Step 5: GREEN + clippy**: `cargo test -p orchestrator rate_limit`, `cargo test -p orchestrator --test rate_limit_emit_test`, clippy.
- [ ] **Step 6: Commit** — `feat(orchestrator): raw per-window utilization tracking + emit-on-change`.

---

### Task 3: Live subscription in the adapter (retry gates) + stale PARITY-GAP comment

**Files:**
- Modify: `lingxi-code/orchestrator/src/provider_adapter.rs`
- Modify: `lingxi-code/orchestrator/src/config.rs` (comment only, :129-140)
- Modify: `lingxi-code/apps/engine-desktop/src/lib.rs` (pass the slot)
- Modify: `lingxi-code/apps/engine-mobile/src/host.rs` (pass None)

- [ ] **Step 1: Read** provider_adapter.rs :33-37 (SubscriberState), :130-230 (constructors), :280/:298/:497 (read sites). Plan: add `subscription: Option<traits::subscription::SharedSubscription>` field + builder method `with_subscription(mut self, slot) -> Self` (do NOT change existing constructor signatures — additive builder keeps every existing call site compiling). Add a private resolver:

```rust
/// Effective subscriber state: the live shared snapshot when provided and
/// resolved (closes the retry-gate half of the OrchestratorConfig
/// PARITY-GAP — `is_enterprise` was build-time `false` because the profile
/// fetch lands after construction), else the static build-time state.
/// Poisoned/empty slot → static fallback (conservative, pre-batch-5
/// behavior).
fn effective_subscriber(&self) -> SubscriberState {
    let Some(slot) = &self.subscription else { return self.subscriber; };
    let Ok(guard) = slot.read() else { return self.subscriber; };
    let Some(snap) = guard.as_ref() else { return self.subscriber; };
    SubscriberState {
        is_subscriber: snap.is_subscriber,
        is_enterprise: snap.subscription_type.as_deref() == Some("enterprise"),
    }
}
```

Replace the three `self.subscriber.is_subscriber` / `RetryState { is_subscriber: self.subscriber.is_subscriber, is_enterprise: self.subscriber.is_enterprise, ... }` reads with `self.effective_subscriber()` (compute ONCE per drive call, not per loop iteration — hoist a `let sub = self.effective_subscriber();` at the top of each drive fn / header-inject fn).

- [ ] **Step 2: Failing test first** (provider_adapter test mod or wherever its unit tests live): construct the adapter with a slot containing `subscription_type: Some("enterprise"), is_subscriber: true` → `effective_subscriber()` returns both true; empty slot → static fallback; no slot → static.
- [ ] **Step 3: Wire hosts.** engine-desktop lib.rs:1182 region: after constructing the adapter, chain `.with_subscription(subscription.clone())` (the slot from batch 4 — confirm it's in scope at that point; it's created earlier in build()). engine-mobile host.rs:362: leave as-is (no slot — static state stands; add a one-line comment). Update the stale PARITY-GAP comment at config.rs:129-140: the profile fetch now happens (engine-desktop batch 4) and the adapter reads the live slot (batch 5); `is_enterprise` static field remains as the fallback seed.
- [ ] **Step 4: GREEN + clippy** on orchestrator + engine-desktop + engine-mobile (package name from its Cargo.toml). `cargo check --workspace`.
- [ ] **Step 5: Commit** — `feat(orchestrator): adapter reads live SharedSubscription for 429/enterprise retry gates`.

---

### Task 4: TUI statusline `rate_limits` export

**Files:**
- Modify: `lingxi-code/tui/src/events/orchestrator_bridge.rs` (map the new OutputEvent → TurnEvent)
- Modify: `lingxi-code/tui/src/streaming.rs` (apply_event arm → AppState)
- Modify: `lingxi-code/tui/src/state.rs` (AppState.raw_utilization)
- Modify: `lingxi-code/tui/src/components/status_line_command.rs` (`build_status_line_input` + tests)
- Modify: the `build_status_line_input(` call site (grep in tui/src — repl.rs/app.rs region)

- [ ] **Step 1: Read** how batch-3's RateLimit event crosses bridge→TurnEvent→apply_event, and the `build_status_line_input` call site.
- [ ] **Step 2: Failing tests**:
  - status_line_command.rs: `rate_limits_field_mirrors_ts_shape` — with five_hour {0.42, 1750000000} and seven_day None, JSON contains `rate_limits.five_hour.used_percentage == 42.0` and `rate_limits.five_hour.resets_at == 1750000000`, and `rate_limits.seven_day` is ABSENT (not null); with both None, `rate_limits` is `{}` (TS spreads optional members into an always-present object — StatusLine.tsx:51-65: `rate_limits` itself is always set, windows are conditional).
  - streaming.rs tests: RawUtilization TurnEvent updates `state.raw_utilization`.
- [ ] **Step 3: Implement.**
  - `AppState.raw_utilization: Option<RawUtilizationSnapshot>` — define a small TUI-local struct (4 fields mirroring the event; do NOT import orchestrator types here if the bridge already re-shapes — follow how RateLimit's 9 fields crossed: the TUI has its own mirror struct in rate_limit_messages.rs; put this one in state.rs or status_line_command.rs, wherever the consumer lives).
  - Bridge + streaming arm: mechanical mapping (mirror RateLimit's).
  - `build_status_line_input`: add a 9th param `raw_utilization: Option<&RawUtilizationSnapshot>` (the existing 8-arg allow(too_many_arguments) comment already justifies the flat list — extend it). JSON:
```rust
let mut rate_limits = serde_json::Map::new();
if let Some(raw) = raw_utilization {
    if let (Some(u), Some(r)) = (raw.five_hour_utilization, raw.five_hour_resets_at) {
        rate_limits.insert("five_hour".into(), json!({ "used_percentage": u * 100.0, "resets_at": r }));
    }
    if let (Some(u), Some(r)) = (raw.seven_day_utilization, raw.seven_day_resets_at) {
        rate_limits.insert("seven_day".into(), json!({ "used_percentage": u * 100.0, "resets_at": r }));
    }
}
// then in the json!: "rate_limits": rate_limits,
```
  - Call site passes `st.raw_utilization.as_ref()`.
- [ ] **Step 4: GREEN + clippy**: `cargo test -p tui`, clippy.
- [ ] **Step 5: Commit** — `feat(tui): statusline command input carries rate_limits raw utilization`.

---

### Task 5: TUI overage-transition notice (getUsingOverageText)

**Files:**
- Modify: `lingxi-code/tui/src/rate_limit_messages.rs` (port the text fn)
- Modify: `lingxi-code/tui/src/streaming.rs` (transition logic in the RateLimit arm)
- Modify: `lingxi-code/tui/src/state.rs` (`has_shown_overage_notification: bool`)
- Modify: `lingxi-code/tui/src/app.rs` — decide whether `/clear` should reset the flag: TS state is component-level (`useState` in the hook, NOT reset by transcript clear) — so do NOT reset it on /clear; add a one-line comment at the existing dedupe-slot resets noting the deliberate difference.

- [ ] **Step 1: Read** TS `useRateLimitWarningNotification.tsx` (whole file) + `rateLimitMessages.ts:303-331`. Note: notification fires when `isUsingOverage && !hasShownOverageNotification && (!isTeamOrEnterprise || hasBillingAccess)`; flag resets when `!isUsingOverage`; `getIsRemoteMode()` skip is structurally false for the TUI (document).
- [ ] **Step 2: Failing tests** (rate_limit_messages.rs):

```rust
#[test]
fn using_overage_text_per_limit_type() {
    // five_hour → "You're now using extra usage · Your session limit resets {t}"
    // seven_day → weekly limit; seven_day_opus → Opus limit;
    // seven_day_sonnet + pro → weekly limit; + unknown sub → Sonnet limit;
    // no rate_limit_type (or unmatched) → "Now using extra usage" (bare, no reset suffix);
    // resets_at None → "You're now using extra usage" (no suffix) for known types.
}
```
Port (straight ASCII, U+00B7):
```rust
/// Port of `getUsingOverageText` (rateLimitMessages.ts:303-331).
pub fn using_overage_text(info: &RateLimitInfo, sub: &SubscriptionSnapshot) -> String {
    let reset_time = fmt_reset(info.resets_at); // existing helper
    let limit_name = match info.rate_limit_type.as_deref() {
        Some("five_hour") => "session limit",
        Some("seven_day") => "weekly limit",
        Some("seven_day_opus") => "Opus limit",
        Some("seven_day_sonnet") => {
            if sub.is_pro_or_enterprise() { "weekly limit" } else { "Sonnet limit" }
        }
        _ => "",
    };
    if limit_name.is_empty() {
        return "Now using extra usage".to_owned();
    }
    match reset_time {
        Some(t) => format!("You're now using extra usage \u{b7} Your {limit_name} resets {t}"),
        None => "You're now using extra usage".to_owned(),
    }
}
```
(CHECK the TS: the no-reset branch returns `You're now using extra usage` + empty resetMessage — exact; the bare fallback is `Now using extra usage` WITHOUT "You're". Verify byte-for-byte against ts:303-331 and pin in tests.)
- [ ] **Step 3: Streaming-arm transition logic** (after the existing compose block, same `TurnEvent::RateLimit` arm):
```rust
// Overage-transition notice (useRateLimitWarningNotification.tsx): fire
// once on entering overage when (!team/enterprise || billing access);
// reset the flag on leaving overage. Remote-mode skip is structurally
// false (the TUI is never remote). TS keeps the flag in component state —
// /clear does NOT reset it.
let is_using_overage = info.status.as_deref() == Some("rejected")
    && matches!(info.overage_status.as_deref(), Some("allowed" | "allowed_warning"));
if is_using_overage {
    if !state.has_shown_overage_notification
        && (!sub.is_team_or_enterprise() || sub.has_claude_ai_billing_access())
    {
        state.has_shown_overage_notification = true;
        state.messages.push(RenderedMessage::RateLimit {
            text: crate::rate_limit_messages::using_overage_text(&info, &sub),
            upsell: None,
        });
    }
} else {
    state.has_shown_overage_notification = false;
}
```
(`info` is consumed by compose_rate_limit just above — check borrow order; compose takes `&info` so this is fine. The flag-test in streaming.rs: rejected+overage-allowed event pushes the notice once, a second identical event doesn't, a non-overage event resets, then overage again re-fires.)
- [ ] **Step 4: GREEN + clippy**: `cargo test -p tui`, clippy.
- [ ] **Step 5: Commit** — `feat(tui): overage-transition notice (getUsingOverageText port)`.

---

### Task 6: 429 limits-specific terminal error copy (rate_limit_error_message wiring) — INVESTIGATE-THEN-WIRE

**Files:** TBD by investigation — likely `lingxi-code/orchestrator/src/conversation.rs` / `turn_loop.rs` / `handle_impl.rs`, possibly `lingxi-code/tui/src` error rendering.

TS behavior (errors.ts:480-536): when a turn DIES on a 429 (retries exhausted), the assistant-message error content is the limits-specific copy (`getRateLimitErrorMessage` — the rejected-branch text like `You've hit your weekly limit · resets …`), composed from limits updated off the error headers with status FORCED to 'rejected'.

- [ ] **Step 1: Investigate** — trace what the user currently sees when a turn fails with `OrchestratorError::ApiCall(LlmError::RateLimited)`: grep how run_turn errors are emitted (OutputEvent::Error? eprintln in CLI? TUI turn-error rendering?). Three possible findings:
  1. **A user-facing generic string exists** (e.g. "rate limited" in the transcript) → wire: at that seam, when the error is `RateLimited` and the orchestrator's `last_rate_limit_full()` snapshot has unified headers, compose via `rate_limit_error_message(&info_with_status_rejected, &info.as_reset_times(), SubscriptionContext { is_pro_or_enterprise })` — force `status="rejected"` per errors.ts:507; `is_pro_or_enterprise` from the live slot (Task 3's `effective_subscriber` gives is_enterprise only — for pro you need the snapshot's subscription_type; reuse the Task-3 slot on whatever struct owns the seam). Fall back to the existing generic string when no headers were ever seen.
  2. **The event-driven RateLimit notice already covers the user-visible surface** and the terminal error is swallowed/internal → CLOSE this item as already-rendered-by-design; write the rationale into the spec (Task 7) and do NOT add code.
  3. Something in between → report to the controller (BLOCKED) with the trace; do not guess.
- [ ] **Step 2:** If wiring (finding 1): failing test first at the seam's test surface (orchestrator tests have fakes for output streams — see rate_limit_emit_test.rs), then implement + GREEN + clippy.
- [ ] **Step 3: Commit** (if code) — `feat(orchestrator): limits-specific 429 terminal error copy (getRateLimitErrorMessage wiring)`; if finding 2, no commit here (spec records it in Task 7).

---

### Task 7: isEnvTruthy consolidation (TS-faithful copies only)

**Files:**
- Modify: `lingxi-code/tui/src/rate_limit_messages.rs:94-101` — replace the local fn with `traits::env::is_env_truthy` (tui already deps traits).
- Examine each of: `migrations/src/context.rs:14`, `tools/skill/src/model_override.rs:183`, `tools/shell/src/prompt.rs:100`, `tools/meta/src/repl_gate.rs:67`, `tools/task/src/task.rs:148`, `compaction/src/thresholds.rs:254`. For EACH: read the fn + its tests; switch to the traits helper ONLY IF (a) semantics are byte-identical to the TS port (unset/empty false; lowercase-trim ∈ {1,true,yes,on}) AND (b) the crate already depends on `traits` (check Cargo.toml; do NOT add new dep edges). Keep local wrappers where the copy reads the env itself (e.g. `prompt.rs` takes a name) — the wrapper calls the traits fn for the value test. Anything semantically divergent (e.g. the documented-divergent `orchestrator/src/conversation.rs:2758`) stays local with its existing documentation.
- [ ] **Step 1:** Per-file examination table in your report (file → identical? → traits dep? → switched/kept + why).
- [ ] **Step 2:** Switch the qualifying ones; their existing tests stay and must pass unchanged (they pin behavior, not implementation).
- [ ] **Step 3: GREEN + clippy** on every touched crate. **Step 4: Commit** — `refactor: consolidate TS-faithful is_env_truthy copies onto traits::env`.

---

### Task 8: Spec rev2.10 + final verification

**Files:**
- Modify: `docs/superpowers/specs/2026-06-10-llm-client-engine-adoption-design.md`

- [ ] Spec entry rev2.10 (follow rev2.9's format): batch-5 deliverables (live retry gates closing the config.rs:134 retry half; RawUtilization track → statusline rate_limits; overage-transition notice; Task-6 outcome as wired-or-closed-by-design; isEnvTruthy consolidation table summary); Remaining list updated honestly (deferred /mock-limits + options menu; OpenAiResponses real-traffic still key-blocked; anything Task 6 surfaced).
- [ ] Commit — `docs(spec): rev2.10 — future-work batch 5`.

## Final verification (after all tasks)

1. `cargo build -p mock_stdio_mcp`, then full `cargo test --workspace` — 0 failures (known flake: posix fs_watch under churn).
2. Clippy battery: traits, orchestrator, tui, engine-desktop, engine-mobile package, cli, client-adapter, compaction, migrations + every tools/* crate touched in Task 7.
3. Frozen checks: traits 0 removed lines; protocol empty diff.
4. Trailer audit: `git log main..HEAD --format='%(trailers:key=Co-Authored-By)' | sort -u` → exactly one value.
