# llm-client Future-Work Batch 6 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the remaining actionable rev2.10 leftovers: terminal-429 state/emit parity (status-change emit + terminal-only snapshot + 429 raw extraction, one coherent change) and the A6 statusline execution pump (the missing piece that lets `rate_limits` reach user scripts).

**Architecture:** (1) The adapter's per-attempt 429 recording becomes a pending slot promoted to the real caches only at the TERMINAL 429 (matching TS `extractQuotaStatusFromError`, which on the drive path runs in the terminal catch handler only); raw utilization is extracted from the error headers UNCONDITIONALLY (TS extracts raw for any error headers, independent of the limits gate); the conversation drivers then fire the existing emit-on-change helpers before returning the enriched error — giving the TS `emitStatusChange` parity (the TUI renders BOTH the terminal copy and the event-driven banner, as TS does). (2) A third pump in the TUI root mirrors the bridge/multiagent pump pattern: TurnEnded sets a dirty flag, a 300ms debounce loop snapshots the payload inputs, runs the statusline command on the blocking pool (single-flight — no generation guard needed: the loop awaits each command inline, and re-trigger rides the dirty flag), and writes `status_line_text` back.

**Pre-implementation plan-review:** this plan was adversarially pressure-tested (4-lens workflow) against the TS and live code before implementation. Load-bearing corrections already folded in: the headerless-429 emit is a DOCUMENTED DIVERGENCE (not a faithful `extractQuotaStatusFromError` port — see B1 in Task 1); the settings-wiring uses `read_settings_map` raw JSON (statusLine is NOT a typed `SettingsJson` field — see Task 2); the emit helpers are no-arg `&self` async methods reached by de-sugaring sync `map_err` sites; promotion needs drive-entry reset + `matches!(RateLimited)` gating; raw is uncoupled from the limits gate. Frozen-crate-safe: both tasks reuse existing `OutputEvent::RateLimit`/`RawUtilization`, `RateLimitInfo::from_429_error_headers`, `RawUtilization::from_headers`, `enrich_rate_limited_error` — NO new variants, NO traits/protocol edits.

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
| Terminal-429 state update (TS REFERENCE — Rust ports a NARROWER gate, see B1) | `claude-code/src/services/claudeAiLimits.ts` | 487-515 (`extractQuotaStatusFromError`: updates `rawUtilization` from error headers :500 for ANY headers; builds limits via `computeNewLimitsFromHeaders` only when `error.headers` present; then FORCES `status='rejected'` at :507 **UNCONDITIONALLY — outside the `if (error.headers)` block**; `emitStatusChange` if `!isEqual` :509-511 — so a HEADERLESS terminal 429 still emits a bare rejected). DIVERGENCE the Rust takes: our `last_rate_limit` cache has no "bare rejected, no windows" representation, so the Rust path promotes/emits only when the unified-header gate (`from_429_error_headers`/errors.ts:480) passes; a headerless terminal 429 emits no RateLimit event (the terminal error copy already conveys rejection). |
| The MESSAGE gate the Rust actually ports | `claude-code/src/services/api/errors.ts` | 480-516 (builds a local limits object from error headers, gate `rateLimitType \|\| overageStatus`, status forced rejected) — this is `RateLimitInfo::from_429_error_headers`, already in the tree (batch 5) |
| Where TS calls `extractQuotaStatusFromError` | `claude-code/src/services/api/claude.ts` | ~:2710, :2765 — on the drive path only in the terminal catch (post-retry), NOT the retry loop. (A 3rd call exists in `checkQuotaStatus` pre-flight, claudeAiLimits.ts:246 — not modeled by the Rust adapter.) |
| Statusline pump | `claude-code/src/components/StatusLine.tsx` | 138-258 (debounce 300ms on lastAssistantMessageId/permissionMode/vimMode/model change; abortable `executeStatusLineCommand`; set statusLineText only when text changed; errors silently ignored) |
| Statusline trust gate | `claude-code/src/utils/hooks.ts` | 286-296 (`shouldSkipHookDueToTrust` — trust dialog state; NO Rust port of `hasTrustDialogAccepted` exists) |

## Survey facts (verified — do NOT re-derive)

- Batch-5 T6 layout (`orchestrator/src/provider_adapter.rs`): `record_rate_limit_from_429(headers)` called at the two 429 decode sites (non-stream `decode_err` arm ~:757-region; stream connect ≥400 ~:960-region); it currently writes `last_rate_limit` PER-ATTEMPT (documented divergence, this batch fixes it) and composes/caches `last_429_message`; `last_429_message` cleared on success at both success seams; `last_raw_utilization` assigned only by `record_rate_limit_from_headers` (success paths).
- Conversation drivers: `enrich_rate_limited_error` (aka `enrich_api_error`) applied at 4 sites covering all 6 public run_turn drivers — `conversation.rs:1376`, `:1957`, `:2495` (all `.map_err(|e| self.enrich_api_error(e))` SYNC closures), and `:2660` (sync match arm `Err(e) => Err(self.enrich_api_error(e))`). `:2495` and `:2660` are tail-position. The emit helpers are `async fn emit_rate_limit_if_changed(&self)` / `emit_raw_utilization_if_changed(&self)` — **NO args** (conversation.rs:709, :755), read `self.api`/`self.output`. They are called at the two SUCCESS seams (`turn_loop.rs:376-378`, streaming `conversation.rs:~2262`). ⇒ You CANNOT `.await` inside the sync `map_err` closures; each site must be de-sugared (see Task 1 step 3).
- Tests: NO existing test pins a PER-ATTEMPT `last_rate_limit` write. The only post-429 snapshot assertion is `terminal_429_with_unified_headers_records_limits_copy` (provider_adapter.rs:2104) — it drives a TERMINAL 429 (FakeTransport), so it SURVIVES terminal-only promotion UNCHANGED and becomes the regression guard that `promote_pending_429` fires. Do NOT weaken it. `rate_limit_emit_test.rs` uses `MockApiClient` + `set_rate_limit_full` (stubs `last_rate_limit_full()` directly, bypassing `promote_pending_429`) — so an "emit" test there does NOT exercise promotion (see Task 1 test split, I4).
- TUI pump pattern (`tui/src/root.rs:1855-1870`): `hooks.use_future` + `state: Arc<tokio::sync::Mutex<AppState>>` (TOKIO mutex — `let mut st = state.lock().await`) + drain loop + `tick.set(tick.get().wrapping_add(1))` redraw; a 100ms `tokio::time::interval` ticker exists nearby (:1924, `MissedTickBehavior::Skip`); `spawn_blocking` precedent at :1366/:1407 (its `.await` yields a `Result<T, JoinError>` — unwrap via `.unwrap_or_default()` per :1370).
- `apply_event` (`tui/src/streaming.rs:27`) — the PRODUCTION `TurnEvent::TurnEnded(_outcome)` handler is at **streaming.rs:92** (line 290 is a test fixture); takes `&mut AppState` + `&Notify`. Set `status_line_dirty = true` here.
- Statusline pieces ALL exist but unwired: `StatusLineConfig::from_settings_value` / `should_run(trusted)` (fail-closed bool param), `build_status_line_input(9 args incl. `raw_utilization: Option<&RawUtilizationSnapshot>`)`, `run_status_line_command(command, stdin_json, timeout)` (sync, std::process — **already runs stdout through `format_custom_status_line` and returns FORMATTED text**, status_line_command.rs:251-256; do NOT format again), `STATUS_LINE_TIMEOUT = Duration::from_secs(5)` const (status_line_command.rs:64 — matches TS hooks.ts 5000ms; reuse it, don't re-derive), `format_custom_status_line` (lives in status_line.rs:131 — not needed if you assign `run_status_line_command`'s output directly), `AppState.status_line_text` (rendered via app.rs:1068 → repl.rs:212), `AppState.status_line_config` + `load_status_line_setting(&self, settings: &serde_json::Value)` (state.rs:928 — NO caller yet; needs a raw `Value`), `AppState.raw_utilization`.
- **Settings wiring (B2):** `statusLine` is NOT a typed `SettingsJson` field (schema.rs drops unknown keys), so `Settings::load`/`load_merged_output_style` CANNOT carry it. Use the raw-map precedent `migrations::settings_update::read_settings_map` (returns `BTreeMap<String, Value>` per tier; already used at apps/cli/src/mode.rs:254-265). Read User+Local(+project), merge the `statusLine` key in claude-code precedence (Local over User), pass that `Value` to `load_status_line_setting`.
- NO Rust trust-store port exists (`hasTrustDialogAccepted` unported; `hooks/src/executor.rs:457` comment says workspace trust is handled "upstream"). Settings `trustedDirectories` is a DIFFERENT mechanism (claude-code gates the statusline on the trust DIALOG, not trustedDirectories).
- `AppState.status` is a `StatusSnapshot` (state.rs:421) with `model: String` (one string — reuse for both `model_id`+`model_display_name`) and `cwd: PathBuf` (reuse for both `current_dir`+`project_dir`; document the divergence); cost string + context pct also there; `parse_cost_usd` recovers the dollar value.

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

**Design.** TS reference is `extractQuotaStatusFromError` (claudeAiLimits.ts:487-515) but the Rust ports a NARROWER gate (the `from_429_error_headers` message gate) — see the ground-truth table and B1 below. Read the existing `record_rate_limit_from_429` (provider_adapter.rs:~564-593) and both drive fns FIRST.

1. **Pending slot.** `record_rate_limit_from_429` no longer writes `last_rate_limit`. Instead it stores `pending_429: Mutex<Option<Pending429>>`. **Split the existing `.map(|info| {...})` closure (provider_adapter.rs:570-593):** that closure currently couples the snapshot write WITH `last_429_message` composition — keep the message composition exactly as-is (per-attempt compose+cache is unobservable; do NOT touch its tests), but move ONLY the snapshot into the pending slot. **Uncouple `raw` from the limits gate (I3):** TS runs `extractRawUtilization(headersToUse)` for ANY error headers (ts:500), independent of the `computeNewLimitsFromHeaders` gate. So compute `raw = RawUtilization::from_headers(&hvec)` UNCONDITIONALLY (the closure already builds `hvec`), and store the pending slot whenever EITHER the limits gate passes OR raw is non-empty:
```rust
/// 429-attempt state held until the retry loop declares the error TERMINAL —
/// TS updates module state only in the terminal catch handler
/// (`extractQuotaStatusFromError`, claudeAiLimits.ts:487), never on retried
/// attempts. Promoted by [`Self::promote_pending_429`]; discarded on
/// drive-entry and on any subsequent success.
struct Pending429 {
    /// Forced-rejected limits snapshot (`from_429_error_headers`); `None`
    /// when the unified-header limits gate did not pass but raw windows did.
    info: Option<RateLimitInfo>,
    /// Raw per-window utilization from the SAME error headers, computed
    /// UNCONDITIONALLY (`extractRawUtilization`, ts:500 — independent of the
    /// limits gate).
    raw: RawUtilization,
}
```
A 429 whose headers yield NEITHER a gated `info` NOR any raw window clears the pending slot (→ `None`) — a later headerless 429 must not leave a stale earlier-attempt snapshot.

2. **Promotion at terminal + drive-entry reset (I2).** New `fn promote_pending_429(&self)` on the adapter: `.take()` the slot; if `Some`: when `info` is `Some`, `*last_rate_limit.lock() = Some(info)`; when `raw != RawUtilization::default()`, `*last_raw_utilization.lock() = Some(raw)` (our raw cache convention never stores the empty snapshot — document the line; TS assigns unconditionally). Promotion is idempotent (`.take()` empties the slot). **Hook points — NAMED, gated on `matches!(e, LlmError::RateLimited { .. })`** (do NOT promote on non-RateLimited terminals like `RepeatedOverloaded`):
   - non-stream `drive_non_stream_seeded_with_chain`: the `DriveStep::Terminal` arm, right before `return Err(decode_err)` (~:845).
   - stream `drive_stream`: the post-RetryAfter terminal `return Err(decode_err)` (~:995).
   (Verify these are the actual decode-terminal returns when you read the fns; gate each on the RateLimited match.)
   **Drive-entry reset:** add `*pending_429.lock() = None` at the TOP of BOTH drive fns. Rationale (verified): the slot is a cross-call Mutex; the Fallback/AdjustMaxTokens arms `continue` without clearing, the transport-error arm (~:713) surfaces a RateLimited that never recorded headers, and `RepeatedOverloaded` returns a non-RateLimited terminal leaving `pending=Some` — so without a per-drive reset a later turn could promote a stale earlier-turn snapshot. TS has no such window (`extractQuotaStatusFromError` reads only the current error's headers).
   **Success-clear:** add `*pending_429.lock() = None` next to the existing `last_429_message` clearing at BOTH success seams (covers the retried-429-then-success case).

3. **Emit at the terminal error (conversation.rs) — de-sugar the 4 sync sites (I1).** The emit helpers are no-arg `async fn …(&self)` reading `self.api`/`self.output`; the 4 enrich sites are SYNC (`.map_err`/match-arm), so you must restructure each to bind-then-await. The shape (apply at all 4: `:1376`, `:1957`, `:2495`, `:2660`):
```rust
// before:  RESULT.map_err(|e| self.enrich_api_error(e))
// after:
let result = RESULT;                  // bind the Result<_, OrchestratorError-or-LlmError>
if let Err(e) = &result {
    if matches!(e, /* the RateLimited / RateLimitRejected discriminant this site carries */) {
        // emitStatusChange parity (claudeAiLimits.ts:509-511): the drive fn
        // already promoted the pending 429 into self.api's caches, so these
        // emit-on-change helpers flow the rejected snapshot out as an
        // OutputEvent::RateLimit (+ RawUtilization), giving the TUI the banner
        // + the T5 overage notice ALONGSIDE the terminal error copy — exactly
        // what TS renders (assistant error message + RateLimitMessage).
        self.emit_rate_limit_if_changed().await;
        self.emit_raw_utilization_if_changed().await;
    }
}
result.map_err(|e| self.enrich_api_error(e))
```
   For `:2495`/`:2660` (tail-position) use the same bind-then-match-then-return scaffold. Check each site's exact error type (some carry `LlmError`, some already `OrchestratorError`) and match the right discriminant — the emit must fire for the rate-limited terminal whether it surfaces as `ApiCall(RateLimited)` or enriched `RateLimitRejected`. The emits run AFTER the drive fn returned (so promotion already happened) and BEFORE `enrich_api_error` rewrites the message.

4. **Doc updates**: `record_rate_limit_from_429` divergence comment (per-attempt → now terminal-only via the pending slot; divergence CLOSED); `last_raw_utilization` "NOT recorded on 429" sentence → now recorded at terminal; B1 divergence comment (headerless-429 emits no RateLimit event — see test); spec follow-ups shrink (Task 3).

**B1 — headerless-429 is a DOCUMENTED DIVERGENCE, not parity.** Do NOT claim a faithful `extractQuotaStatusFromError` port. TS forces `status='rejected'` and emits even on a headerless terminal 429 (ts:506-507, outside the headers block); the Rust promotes/emits only when the header gate passes, because `last_rate_limit` has no "bare rejected, no windows" representation and the terminal error copy already conveys rejection. State this in code + test comments.

**Tests (TDD; RED first). Two layers that do NOT meet end-to-end with current harnesses (I4) — keep them separate:**

*Adapter-level (FakeTransport drive sequences — read the existing `provider_adapter.rs` FakeTransport tests like `terminal_429_with_unified_headers_records_limits_copy:2104` first and mirror them):*
- `terminal_429_promotes_snapshot_and_raw` — drive exhausts retries on a 429 with unified headers (limits + per-window): `last_rate_limit_full()` status rejected; `last_raw_utilization()` has the windows from the error headers.
- `retried_429_does_not_update_limits_snapshot` — `[429-with-headers, then SUCCESS]` in one drive: `last_rate_limit_full()` reflects the success (or None if headerless), NOT the 429; pending slot cleared.
- `stale_pending_429_not_promoted_across_drives` (the I2 guard) — drive A = `[429-with-headers, then RepeatedOverloaded terminal]` (non-RateLimited terminal, leaves no promotion); drive B = a terminal that is RateLimited but recorded NO fresh 429 headers (transport-surfaced) → assert B does NOT promote A's snapshot (drive-entry reset cleared it).
- `terminal_429_with_unified_headers_records_limits_copy` (existing, :2104) must continue to PASS UNCHANGED — it is the load-bearing guard that `promote_pending_429` fires. Do NOT weaken it.

*Emit-on-change (MockApiClient + `set_rate_limit_full`/`set_raw_utilization` in `rate_limit_emit_test.rs` style) — these stub the snapshot directly and do NOT exercise promotion; they verify the conversation-side de-sugared emit fires on a rate-limited terminal:*
- `terminal_rate_limited_error_emits_rate_limit_event` — a run_turn that fails with a rate-limited terminal, with the mock's `last_rate_limit_full()` set to a rejected snapshot → exactly one `OutputEvent::RateLimit` (status rejected) AND (when raw set) one `RawUtilization`; the returned error is still the enriched copy. (Do NOT call this an "end-to-end promotion" test — it is the emit-seam test.)
- `non_rate_limited_terminal_emits_no_rate_limit_event` — a terminal that is NOT rate-limited (e.g. Overloaded) → no RateLimit event from the error path.
- `headerless_terminal_429_emits_no_rate_limit_event` (B1 divergence) — rate-limited terminal but mock snapshot is `None`/default → no RateLimit event; pin with a comment citing the B1 divergence (TS would emit a bare rejected).

**Verify:** `cargo test -p orchestrator` full; clippy; `cargo check --workspace`.
**Commit:** `feat(orchestrator): terminal-only 429 state promotion + status-change emit parity`

---

### Task 2: A6 statusline execution pump (tui)

**Files:**
- Modify: `lingxi-code/tui/src/root.rs` (third pump block)
- Modify: `lingxi-code/tui/src/streaming.rs` (dirty flag on TurnEnded at streaming.rs:92)
- Modify: `lingxi-code/tui/src/state.rs` (`status_line_dirty` field + `build_pump_payload` pure core)
- Modify: `lingxi-code/tui/src/session.rs` (`Runtime::with_status_line_config` builder, mount-thread into AppState — mirror batch-5 `with_subscription`) + `lingxi-code/apps/cli/src/mode.rs` (`build_tui_runtime`: read+merge `statusLine` via `read_settings_map`, parse `StatusLineConfig`, pass through the builder)
- Modify: `lingxi-code/tui/src/components/status_line_command.rs` (module-doc "pump is a follow-up" note → wired)

**Design (port of StatusLine.tsx:138-258).**

1. **Trigger.** `AppState.status_line_dirty: bool`. Set `= true` ONLY in `apply_event` on `TurnEvent::TurnEnded` (streaming.rs:92). **Do NOT add a RawUtilization/RateLimit dirty trigger (M8):** a terminal 429 emits `ClientEvent::Error`, not `TurnEnded`, so the statusline deliberately does NOT refresh after a terminal 429 — and that is TS-faithful (StatusLine.tsx re-runs on lastAssistantMessageId/mode/model, none of which fire on a terminal 429). (TS also re-runs on permission-mode/vim/model change; the TUI analog for those can be added when those mutate AppState — out of scope here, note it.)

2. **Pure core (testable, in state.rs or status_line_command.rs).** Extract the gate+snapshot+build so the loop is thin:
```rust
/// Build the (command, stdin-json) pair for the statusline pump, or None when
/// the pump should not run. Pure — the render loop calls this under the lock.
fn build_pump_payload(state: &AppState) -> Option<(String, String)> {
    let cfg = state.status_line_config.as_ref()?;
    // trusted=true: claude-code gates the statusline on the trust DIALOG
    // (hooks.ts:286-296 shouldSkipHookDueToTrust); lingxi has no
    // hasTrustDialogAccepted port — the hooks executor takes the same
    // upstream-trust stance (hooks/src/executor.rs:457). should_run keeps its
    // fail-closed `trusted` parameter for when a trust store lands.
    if !cfg.should_run(true) { return None; }
    // StatusSnapshot has one model string + cwd only — reuse model for
    // id+display and cwd for current_dir+project_dir (documented divergence);
    // added_dirs not tracked in TUI state → empty.
    let json = build_status_line_input(
        &state.status.model, &state.status.model,
        &state.status.cwd, &state.status.cwd,
        &[], env!("CARGO_PKG_VERSION"),
        parse_cost_usd(&state.status.cost /* the cost string field */),
        /* context_pct from state.status */,
        state.raw_utilization.as_ref(),
    );
    Some((cfg.command.clone(), json.to_string()))
}
```
(Adapt field accessors to `StatusSnapshot`'s real names.)

3. **Pump loop (new `use_future` block in root.rs, mirroring the bridge pump at :1855 — `state: Arc<tokio::sync::Mutex<AppState>>`, `tokio::time::interval(Duration::from_millis(300))` + `MissedTickBehavior::Skip` as the debounce analog).** Single-flight (awaits each command inline) — NO generation counter (I5: a generation guard would be dead code here; re-trigger rides `status_line_dirty`):
```text
interval 300ms (Skip):
  let payload = { let mut st = state.lock().await;
                  if !st.status_line_dirty { continue }
                  st.status_line_dirty = false;
                  build_pump_payload(&st) };           // lock dropped here
  let Some((command, stdin_json)) = payload else { continue };
  // spawn_blocking: run_status_line_command spawns a child + 5s timeout
  let out = tokio::task::spawn_blocking(move ||
      run_status_line_command(&command, &stdin_json, STATUS_LINE_TIMEOUT))
      .await.unwrap_or_default();                       // JoinError → None (root.rs:1370 precedent)
  if let Some(text) = out {                             // command failure → None → keep previous text (TS silent catch)
      let mut st = state.lock().await;
      // run_status_line_command ALREADY returns format_custom_status_line'd
      // text (status_line_command.rs:251-256) — assign directly, do NOT format again (M3).
      if st.status_line_text.as_deref() != Some(text.as_str()) {
          st.status_line_text = Some(text);
          tick.set(tick.get().wrapping_add(1));         // redraw, bridge-pump style
      }
  }
```
No lock is held across the `spawn_blocking().await` (both locks are scoped blocks dropped before/after). `STATUS_LINE_TIMEOUT` is the existing `status_line_command::STATUS_LINE_TIMEOUT` (5s, matches TS) — do not introduce a new constant.

4. **Settings wiring (B2 — real work, NOT a one-liner).** `statusLine` is not a typed `SettingsJson` field, so route raw JSON: in `build_tui_runtime` (cli/mode.rs) read User+Local(+project) via `migrations::settings_update::read_settings_map`, merge the `statusLine` key in claude-code precedence (Local over User), parse via `StatusLineConfig::from_settings_value`, and pass the `Option<StatusLineConfig>` through a new `Runtime::with_status_line_config` builder threaded into `AppState.status_line_config` at mount (mirror batch-5 `with_subscription` end-to-end). `load_status_line_setting(&mut self, &Value)` may become redundant if you parse in mode.rs — if so, either use it (pass the merged `Value`) or remove it; **no dead code**.

**Tests (TDD; RED first):**
- streaming.rs: `turn_ended_sets_status_line_dirty` — `apply_event(.., TurnEnded, ..)` flips the flag.
- state.rs `build_pump_payload` unit tests: no `status_line_config` → None; config present but `should_run(true)` false (non-command kind / empty command) → None; armed command config → `Some((command, json))` where the json (M6) carries `rate_limits` ONLY when `state.raw_utilization` has at least one FULLY-RESOLVED window (both utilization AND resets_at Some), and OMITS `rate_limits` when windows are unresolved/absent.
- session.rs/state: a `with_status_line_config` default-None + threads-through test (mirror batch-5 T5's `with_subscription` test).
- cli/mode.rs (or a focused unit on the merge helper): a real `{"statusLine":{"type":"command","command":"echo hi"}}` in a settings map reaches `status_line_config = Some(..)` with Local-over-User precedence — this is the B2 RED that fails today.

**Verify:** `cargo test -p tui` full; `cargo test -p cli`; clippy on tui + cli; `cargo build -p cli`.
**Commit:** `feat(tui): wire the A6 statusline command pump (debounced, single-flight)`

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
