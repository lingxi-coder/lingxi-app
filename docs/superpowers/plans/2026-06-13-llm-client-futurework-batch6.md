# llm-client Future-Work Batch 6 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the remaining actionable rev2.10 leftovers: terminal-429 state/emit parity (status-change emit + terminal-only snapshot + 429 raw extraction, one coherent change) and the A6 statusline execution pump (the missing piece that lets `rate_limits` reach user scripts).

**Architecture:** (1) The adapter's per-attempt 429 recording becomes a pending slot promoted to the real caches only at the TERMINAL 429 (matching TS `extractQuotaStatusFromError`, which runs in the catch handler only), now also extracting raw utilization from the error headers; the conversation drivers then fire the existing emit-on-change helpers before returning the enriched error — giving the TS `emitStatusChange` parity (the TUI renders BOTH the terminal copy and the event-driven banner, as TS does). (2) A third pump in the TUI root mirrors the bridge/multiagent pump pattern: TurnEnded sets a dirty flag, a 300ms debounce loop snapshots the payload inputs, runs the statusline command on the blocking pool with a generation guard (TS abort analog), and writes `status_line_text` back.

**Tech Stack:** Rust workspace (`lingxi-code/`). Ground truth: vendored claude-code TS at `/Users/luolingfeng/Projects/LingXi-Next/claude-code/` (read-only — primary checkout).

---

## Standing constraints (same as batches 1-5)

- NEVER touch the primary checkout `/Users/luolingfeng/Projects/LingXi-Next` working tree; all edits in THIS worktree. TS ground truth read-only.
- NEVER `git add -A` / `git add .` — named paths only.
- `traits/` + `protocol/` FROZEN-ADDITIVE (this batch should not need to touch either — if a task seems to require it, stop and reconsider).
- No secrets in errors/logs. Strict TDD with observed RED. Clippy `-D warnings --all-targets --no-deps` per touched crate.
- Commit trailer exactly: `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`.
- Fresh-worktree gotchas: `cargo build -p mock_stdio_mcp` AND `cargo build -p cli` before the first full test run; never trust a `grep|awk` test summary that prints 0 suites (cargo test fail-fast + pipe exit masking).
- CLI package name is `cli`.

## Ground-truth references

| Topic | File (primary checkout, READ ONLY) | Key lines |
|---|---|---|
| Terminal-429 state update | `claude-code/src/services/claudeAiLimits.ts` | 487-515 (`extractQuotaStatusFromError`: terminal catch handler ONLY; updates `rawUtilization` from error headers :500; builds limits via `computeNewLimitsFromHeaders` then FORCES rejected :507; `emitStatusChange` on change :509-511) |
| Where TS calls it | `claude-code/src/services/api/claude.ts` | grep `extractQuotaStatusFromError` (~:2710, :2765 — the terminal catch, NOT the retry loop) |
| Statusline pump | `claude-code/src/components/StatusLine.tsx` | 138-258 (debounce 300ms on lastAssistantMessageId/permissionMode/vimMode/model change; abortable `executeStatusLineCommand`; set statusLineText only when text changed; errors silently ignored) |
| Statusline trust gate | `claude-code/src/utils/hooks.ts` | 286-296 (`shouldSkipHookDueToTrust` — trust dialog state; NO Rust port of `hasTrustDialogAccepted` exists) |

## Survey facts (verified — do NOT re-derive)

- Batch-5 T6 layout (`orchestrator/src/provider_adapter.rs`): `record_rate_limit_from_429(headers)` called at the two 429 decode sites (non-stream `decode_err` arm ~:757-region; stream connect ≥400 ~:960-region); it currently writes `last_rate_limit` PER-ATTEMPT (documented divergence, this batch fixes it) and composes/caches `last_429_message`; `last_429_message` cleared on success at both success seams; `last_raw_utilization` assigned only by `record_rate_limit_from_headers` (success paths).
- Conversation drivers: `enrich_rate_limited_error` applied at 4 sites covering all 6 public run_turn drivers; `emit_rate_limit_if_changed` + `emit_raw_utilization_if_changed` helpers exist on `ConversationOrchestrator` and are called at the two SUCCESS seams (`turn_loop.rs:376-378`, streaming `conversation.rs:~2262`); both read `self.api` accessors (`last_rate_limit_full()` / `last_raw_utilization()`).
- Tests pinning current behavior: `orchestrator/tests/rate_limit_terminal_429_test.rs` (3), `rate_limit_emit_test.rs` (10), adapter unit tests in provider_adapter — terminal-only promotion will require updating any test that asserted the per-attempt `last_rate_limit` write; that change is THE SPEC (cite claudeAiLimits.ts:487 terminal-only), document in the test.
- TUI pump pattern (`tui/src/root.rs:1855-1870`): `hooks.use_future` + `Arc<Mutex<AppState>>` + drain loop + `tick.set(tick+1)` redraw; a 100ms `tokio::time::interval` ticker exists nearby (:1924); `spawn_blocking` precedent at :1366/:1407.
- `apply_event` (`tui/src/streaming.rs:27`) handles `TurnEvent::TurnEnded` (:290 region); takes `&mut AppState` + `&Notify`.
- Statusline pieces ALL exist but unwired: `StatusLineConfig::from_settings_value` / `should_run(trusted)` (fail-closed bool param), `build_status_line_input(9 args incl. raw_utilization)`, `run_status_line_command(command, stdin_json, timeout)` (sync, std::process), `format_custom_status_line`, `AppState.status_line_text` (rendered via app.rs:1068 → repl.rs:212), `AppState.status_line_config` + `load_status_line_setting(&settings_json)` (NO caller yet), `AppState.raw_utilization`.
- NO Rust trust-store port exists (`hasTrustDialogAccepted` unported; `hooks/src/executor.rs:457` comment says workspace trust is handled "upstream"). Settings `trustedDirectories` exists but is a different mechanism (claude-code gates the statusline on the trust DIALOG, not trustedDirectories).
- `AppState.status` is a `StatusSnapshot` (model, cwd, cost string, context pct — read its fields in state.rs when wiring payload inputs); `parse_cost_usd` exists for the cost string.
- TS statusline timeout: claude-code uses a 5s default in `executeStatusLineCommand` (VERIFY in utils/hooks.ts when implementing — grep `STATUS_LINE` constants; use whatever the TS uses).

## Deferred (record in spec, Task 3)

- `/mock-limits` + interactive rate-limit options menu (1k+ TS lines, test tooling) — still deferred.
- OpenAiResponses real-traffic validation — still key-blocked.
- Trust-dialog store port (`hasTrustDialogAccepted`) — the pump launches with `trusted = true` + documented rationale (below); the fail-closed `should_run(trusted)` parameter stays for when the store lands.

---

### Task 1: Terminal-429 promotion + emit parity (orchestrator)

**Files:**
- Modify: `lingxi-code/orchestrator/src/provider_adapter.rs`
- Modify: `lingxi-code/orchestrator/src/conversation.rs`
- Modify: `lingxi-code/orchestrator/src/model/rate_limit.rs` (only if a helper is needed; prefer none)
- Tests: `lingxi-code/orchestrator/tests/rate_limit_terminal_429_test.rs` (extend), adapter unit tests

**Design (port of `extractQuotaStatusFromError`, claudeAiLimits.ts:487-515):**

1. **Pending slot.** `record_rate_limit_from_429` no longer writes `last_rate_limit`. Instead it stores `pending_429: Mutex<Option<Pending429>>` where
```rust
/// 429-attempt state held until the retry loop declares the error TERMINAL —
/// TS only updates module state in the catch handler
/// (`extractQuotaStatusFromError`, claudeAiLimits.ts:487), never on retried
/// attempts. Promoted by [`Self::promote_pending_429`]; discarded on any
/// subsequent success (the existing success seams overwrite/clear).
struct Pending429 {
    /// Forced-rejected limits snapshot (`from_429_error_headers`).
    info: RateLimitInfo,
    /// Raw per-window utilization from the SAME error headers
    /// (`extractRawUtilization` runs on the error pass too, ts:500).
    raw: RawUtilization,
}
```
`last_429_message` composition stays exactly as-is (per-attempt compose+cache is unobservable — it is only read after a terminal RateLimited error; do NOT touch its tests).
A 429 whose headers fail the gate (`from_429_error_headers` → None) clears the pending slot (mirror of the current message-clearing behavior) — a later headerless 429 must not promote an earlier attempt's snapshot.

2. **Promotion at terminal.** New `fn promote_pending_429(&self)` on the adapter: takes the pending slot (`.take()`); if Some: `*last_rate_limit.lock() = Some(info)`; `*last_raw_utilization.lock() = Some(raw)` ONLY when `raw != RawUtilization::default()` (TS assigns unconditionally — but our raw cache convention never stores/emits the empty snapshot; keep convention, document the line). Call sites: in BOTH drive fns (`drive_non_stream_seeded_with_chain`, `drive_stream`), at every `return Err(e)` path where `e` matches `LlmError::RateLimited { .. }` — find them all (the terminal next_step Fail branch + any early-return). Prefer ONE choke point per drive fn if the code shape allows (e.g. wrap the final error return); do not call it on retried attempts.
Also: SUCCESS must discard a stale pending slot — add `*pending_429.lock() = None` next to the existing `last_429_message` clearing at both success seams.

3. **Emit at the terminal error (conversation.rs).** Where `enrich_rate_limited_error` runs (the 4 driver sites), when the error IS the rate-limited one (i.e. enrichment matched — both `ApiCall(RateLimited)` and the enriched `RateLimitRejected` count), call `self.emit_rate_limit_if_changed(output).await` and `self.emit_raw_utilization_if_changed(output).await` BEFORE returning the error (adapt to the helpers' actual signatures). This is the `emitStatusChange` parity (ts:509-511): the promoted rejected snapshot flows out as an `OutputEvent::RateLimit`, so the TUI shows the banner + the T5 overage notice when applicable, IN ADDITION to the terminal error copy — exactly what TS renders (assistant error message + RateLimitMessage component). Document this at the call site.
If the emit helpers take no args / different shape, hook in however the success seams do it — same calls, same order.

4. **Doc updates**: `record_rate_limit_from_429`'s divergence comment (per-attempt → now terminal-only, divergence CLOSED), `last_raw_utilization`'s "NOT recorded on 429" sentence (now it is, at terminal), spec follow-ups list shrinks (Task 3).

**Tests (TDD; extend the terminal-429 + emit integration suites + adapter units):**
- `retried_429_does_not_update_limits_snapshot` — a 429 followed by a SUCCESS within the same drive: `last_rate_limit_full()` reflects the success headers (or None if success headerless), NOT the 429; pending slot cleared. (Drive-level test using whatever fake transport the adapter unit tests use — read them first.)
- `terminal_429_promotes_snapshot_and_raw` — drive exhausts retries on 429-with-headers: `last_rate_limit_full()` has status rejected; `last_raw_utilization()` has the windows from the error headers.
- `terminal_429_emits_rate_limit_event` — end-to-end (rate_limit_emit_test.rs style): run_turn fails on terminal 429 → exactly one `OutputEvent::RateLimit` with status rejected AND (if windows present) one `RawUtilization` event; the turn error is still the enriched copy.
- `headerless_terminal_429_emits_nothing_new` — no unified headers on the 429: no RateLimit event from the error path (the pending slot was None), generic error string surfaces.
- UPDATE existing tests that pinned per-attempt `last_rate_limit` writes (cite ts:487 terminal-only in the update).

**Verify:** `cargo test -p orchestrator` full; clippy; `cargo check --workspace`.
**Commit:** `feat(orchestrator): terminal-only 429 state promotion + status-change emit parity`

---

### Task 2: A6 statusline execution pump (tui)

**Files:**
- Modify: `lingxi-code/tui/src/root.rs` (third pump block)
- Modify: `lingxi-code/tui/src/streaming.rs` (dirty flag on TurnEnded)
- Modify: `lingxi-code/tui/src/state.rs` (dirty flag field + settings wiring call site if needed)
- Modify: `lingxi-code/tui/src/session.rs` and/or `lingxi-code/apps/cli/src/mode.rs` — wherever merged settings JSON is available at mount to call `load_status_line_setting` (INVESTIGATE: find where the TUI/CLI loads merged settings; `engine::settings::Settings::load` is the seam the CLI uses — see `engine_desktop::load_merged_output_style` for the pattern; the TUI needs the `statusLine` value once at mount. If no settings JSON reaches the TUI today, load it in `build_tui_runtime` (cli/mode.rs) via the same `Settings::load` pattern and pass the parsed `StatusLineConfig` through a new `Runtime::with_status_line_config` builder — mirroring `with_subscription` from batch 4/5.)
- Modify: `lingxi-code/tui/src/components/status_line_command.rs` (module-doc "pump is a follow-up" note → wired; `should_run` doc)

**Design (port of StatusLine.tsx:138-258):**

1. **Trigger + debounce.** `AppState.status_line_dirty: bool` (doc: TS re-runs on lastAssistantMessageId/permissionMode/vimMode/model change; the TUI's analog is end-of-turn — set in `apply_event` on `TurnEvent::TurnEnded`; permission-mode/model changes mid-session may be added when those mutate state — note where). Pump loop (new `use_future` block in root.rs, mirroring the bridge pump's structure and comments):
```text
loop every 300ms (tokio interval, MissedTickBehavior::Skip — the TS debounce analog):
  lock state;
  if !state.status_line_dirty → drop, continue;
  let Some(cfg) = state.status_line_config.clone() else { dirty=false; drop; continue };
  if !cfg.should_run(true) → dirty=false; drop; continue;   // trusted=true, see note
  snapshot inputs (model id/display from state.status, cwd, project_dir, added_dirs (empty — not tracked in TUI state; document), version env!("CARGO_PKG_VERSION"), parse_cost_usd(state.status.cost), context_pct, state.raw_utilization);
  state.status_line_dirty = false;
  generation += 1; let my_gen = generation;            // TS AbortController analog
  drop(state lock);
  let text = tokio::task::spawn_blocking(move || run_status_line_command(&cfg.command, &input_json, TIMEOUT)).await;
  lock state;
  if my_gen == generation && text.is_some() {           // stale results discarded
      let formatted = format_custom_status_line(&text.unwrap());
      if state.status_line_text.as_deref() != Some(formatted.as_str()) {
          state.status_line_text = Some(formatted);
          tick redraw;
      }
  }
  drop;
```
(Adapt to the actual pump idioms in root.rs — the multiagent pump shows how a ticker-driven loop locks/mutates/ticks. Errors from the command → `run_status_line_command` returns None → leave the previous text, matching TS's silent catch. CHECK the TS timeout constant in utils/hooks.ts `executeStatusLineCommand` and use it.)
2. **Trust note** (doc at the `should_run(true)` call):
```rust
// trusted=true: claude-code gates the statusline on the trust DIALOG
// (hooks.ts:286-296 shouldSkipHookDueToTrust); lingxi has no
// hasTrustDialogAccepted port — the hooks executor takes the same
// upstream-trust stance (hooks/src/executor.rs:457). should_run keeps its
// fail-closed `trusted` parameter for when a trust store lands.
```
3. **Settings wiring**: per the INVESTIGATE note above — minimal faithful path; if `load_status_line_setting` ends up unused after wiring through a builder instead, delete it or use it — no dead code.

**Tests (TDD):**
- streaming.rs: `turn_ended_sets_status_line_dirty`.
- status_line_command.rs or state tests: pure pieces already tested; add a `should_run` doc-behavior test only if behavior changes (it shouldn't).
- The pump loop itself is render-loop glue — cover its pure core: extract a testable `fn build_pump_payload(state: &AppState) -> Option<(String /*command*/, String /*stdin json*/)>` that does the gate+snapshot+build, unit-tested for: no config → None; non-command/should_run-false → None; armed config → Some with rate_limits present when raw_utilization set. The loop calls this under the lock.
- If a Runtime builder was added: a session.rs/state default test mirroring batch-5 T5's.

**Verify:** `cargo test -p tui` full; clippy; `cargo build -p cli`.
**Commit:** `feat(tui): wire the A6 statusline command pump (debounced, generation-guarded)`

---

### Task 3: Spec rev2.11 + final verification

**Files:** `docs/superpowers/specs/2026-06-10-llm-client-engine-adoption-design.md`

- rev2.11 entry (mirror rev2.10's format; cite the batch-6 commit SHAs from git log): terminal-429 trio closed (promotion + raw extraction + status-change emit; the TUI now double-renders terminal 429s exactly as TS does — banner + terminal copy); A6 statusline pump wired (debounce 300ms, generation guard, trusted=true stance documented, payload incl. rate_limits live end-to-end); the `should_run` trusted parameter retained for a future trust-store port.
- Remaining list (honest): /mock-limits + options menu; OpenAiResponses real-traffic (key-blocked); trust-dialog store port; anything Task 1/2 surfaced.
- Commit: `docs(spec): rev2.11 — future-work batch 6 (terminal-429 parity, statusline pump)`

## Final verification

1. `cargo build -p mock_stdio_mcp && cargo build -p cli`, then `cargo test --workspace > log 2>&1; echo exit:$?` and tally `grep '^test result'` — expect 0 failures and a suite count ≥ the batch-5 count (532); investigate ANY shortfall (fail-fast masking).
2. Clippy battery: orchestrator, tui, cli, client-adapter, bridge-server, engine-desktop (+ any other touched crate).
3. Frozen checks: traits + protocol diffs EMPTY (this batch touches neither).
4. Trailer audit → exactly one value.
