# Design: rate-limit `/usage-credits` messaging migration (byte-exact port of CC 2.1.206)

**Date:** 2026-07-13
**Status:** Approved (design), pending spec review
**Scope:** gap #1 of the two-gap set (independent spec). gap #2 (cost/usage `i6e` fidelity)
already shipped (`9c0953383..1680341d1`).
**Parity target:** Claude Code 2.1.206 (`/Users/luolingfeng/.local/share/claude/versions/2.1.206`)
**Prior audit:** `RATE_LIMIT_MSG_AUDIT_2026-07-12` in `.omx/state/parity-206-loop.json`

## Problem

CC 2.1.206 migrated its claude.ai billing UX from `/extra-usage` to `/usage-credits`
(binary: a real `name:'usage-credits'` command + 52 `/usage-credits` refs vs only 3
legacy `/extra-usage` remnants) and reworded the overage/limit messaging. The port's
`tui/src/rate_limit_messages.rs` (a port of CC's `getUpsellMessage` +
`claudeAiLimits.ts` + the `RateLimitMessage` component) is pinned at the **2.1.205
`/extra-usage` baseline** and shows the older strings.

This was **user-confirmed as an accepted carve-out on 2026-07-12** (keep legacy
`/extra-usage`, don't port the migration). The user has since chosen to **reverse that
decision** and port the migration. This spec reverses the carve-out **for the message
text only** — see Boundaries for what stays a carve-out.

## Goal

Advance `rate_limit_messages.rs` from the 2.1.205 `/extra-usage` messaging to CC
2.1.206's `/usage-credits` messaging, **byte-exact**, within the file's existing
Anthropic/claude.ai gating. No new surfaces, no new wiring.

## Why this is safe / low-blast-radius

`rate_limit_messages.rs` is **already provider-gated**: every composition path runs
behind `SubscriptionSnapshot` / `has_claude_ai_billing_access()` / `is_ant`
(is-Anthropic). Non-Anthropic (OpenRouter/DeepSeek/Bedrock-non-Claude/etc.) sessions
never reach these strings. So this change is confined to the message content + branch
structure of an already-Anthropic-only composer — a build with a non-Anthropic provider
stays byte-identical to today.

## The 2.1.206 delta to port (grounded in binary extraction)

The following are confirmed present verbatim in the 2.1.206 binary (exact bytes + the
precise `if`-branch conditions are pinned during the plan phase, per the verification
recipe):

1. **Command-ref migration** — `/extra-usage` → `/usage-credits` in every upsell string.
   The port's `upsell` mod consts (`EXTRA_USAGE_FINISH`, `EXTRA_USAGE_ADMIN`,
   `UPGRADE_OR_EXTRA`, and the `EXTRA_USAGE_REQUEST` / `UPGRADE_KEEP_USING` module
   consts) migrate to the `/usage-credits` variants.

2. **Overage warning reworded** — port's
   `"You're close to your extra usage spending limit"` →
   `"You're close to your ${limitName}"`, where **`limitName ∈ {"usage limit",
   "usage credit limit"}`**, selected by billing state. (Binary confirms the
   `allowed_warning` branch carries both `"usage limit"` and `"usage credit limit"`
   alternatives alongside the `warning`/`rejected`/`error`/`team` severity keys.)

3. **New upsell branch variants** — the `if(billingAccess)` branch the port lacks,
   confirmed strings:
   - `"/usage-credits to adjust your monthly spend limit."`
   - `"/usage-credits to request more usage from your admin."`
   - `"Run /usage-credits to continue or switch models with /model."` (monthly-spend-hit
     path; sibling to `"You've hit your monthly spend limit. /model to switch models."`)
   - `"run /usage-credits to raise it, or visit claude.ai/admin-settings/usage"`
   - `"run /usage-credits to ask your admin for a higher limit"`
   - `"/usage-credits to turn them on"` (fast-mode/enable path — see Boundary re: fast mode)

4. **`limitName` derivation** — the selection logic feeding #2/#3 (usage limit vs usage
   credit limit), ported TS-faithfully from CC's 2.1.206 `claudeAiLimits.ts` chain.

Embedded claude.ai/platform URLs that appear inside these strings
(`claude.ai/admin-settings/usage`, `claude.ai/settings/usage`,
`https://platform.claude.com/settings/billing`) are ported as **literal display text**
— they are shown to the user, not called, so reproducing them verbatim is faithful and
carries no backend dependency.

## Boundaries (stay carve-out / out of scope)

- **`/usage-credits` command handler** — CC's real `name:'usage-credits'` command drives
  claude.ai billing (add funds / admin request / org usage-credit management). The port
  has **no `/extra-usage` or `/usage-credits` command handler today** (the messages are
  text-only references), and there is no claude.ai billing backend to drive one. Out of
  scope; the strings reference the command name as display text only.
- **Plan-tier usage bars** — architecturally blocked (needs a claude.ai subscription
  backend). Unchanged carve-out.
- **Fast-mode × usage-credit statusline messages** (`"Fast mode disabled · usage credit
  limit reached"`, `"Fast mode disabled · usage credits turned off for your account"`,
  `"usage credits not available for your plan"`, `"Fast mode requires usage credits ·
  /usage-credits to turn them on"`, etc.) — **the plan phase confirms whether these are
  composed in `rate_limit_messages.rs` or a separate fast-mode/statusline surface.** If
  they live in `rate_limit_messages.rs`, they are in scope for this spec; if they live in
  a separate surface, they are a distinct follow-up recorded for later, NOT force-fit into
  this spec.
- **`"/extra-usage is now /usage-credits"` transition toast** — gated by flag
  `tengu_pewter_summit`. Port as **flag-gated + default-off (inert)** so a default build
  is byte-identical, OR record-and-defer if the flag/trigger cannot be verified. Decided
  in the plan after confirming the flag default and trigger site. Not a blocker.

## Data flow (unchanged)

`RateLimitInfo` + `SubscriptionSnapshot` → `compose_rate_limit` / `using_overage_text` /
`warning_upsell` / `error_upsell` → composed string (dimmed by the consuming
`RateLimitMessage` surface). Only the composition branch logic + string constants change;
the inputs, the entry points, and the provider gating are untouched.

## Testing

- **Byte-exact golden-string tests** for every changed/new variant: the reworded
  overage warning with both `limitName` values, each new `/usage-credits` upsell variant,
  and the monthly-spend-hit path. Assert exact bytes (handle `’`/`…`
  source-vs-runtime escape: the binary stores the escapes, the port uses real
  chars = runtime form).
- **Branch tests** for the `limitName` selection and the `if(billingAccess)` variant
  routing — construct the `RateLimitInfo` + `SubscriptionSnapshot` states that select
  each variant and assert the composed output.
- **Inert-when-non-Anthropic**: a test (or reuse of an existing one) asserting a
  non-billing / non-Anthropic `SubscriptionSnapshot` composes the same output it does
  today (no `/usage-credits` leakage into non-Anthropic paths).

## Verification recipe (per this port's convention)

1. Oracle-diff each ported string/branch against 2.1.206 (`grep -abo` offset + python
   latin-1 slice; handle `${…}` interpolation artifacts and source-vs-runtime
   `’`/`…` escapes).
2. `cargo build -p tui` clean.
3. `cargo test -p tui` (+ the new byte-exact tests) and `cargo test -p test-harness` if
   it covers this surface.
4. Confirm the non-Anthropic / default path stays byte-identical.

## Invariants

- Never touch the 4 user dirty files (`llm-client/data/models-dev/openrouter.json`,
  `llm-client/src/catalog/presets.rs`, `tools/agent/src/agent.rs`,
  `tools/agent/src/agent_test.rs`).
- Default-OFF / inert-when-non-Anthropic: a non-Anthropic build stays byte-identical.
- No git remote → commit to `main` locally; commit trailer
  `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`.
- Never run `cargo fmt`.
- On completion, update `RATE_LIMIT_MSG_AUDIT_2026-07-12` + `lingxi-accepted-divergences`
  to record the carve-out reversal (message text now ported; command/plan-tiers still
  carve-out).
