# LingXi control-protocol integration map (read-only survey)

Scope: where to wire the bidirectional SDK **control protocol** into LingXi's
`apps/cli` print path. All file:line refs are absolute under
`/Users/luolingfeng/Projects/LingXi-Next/lingxi-code/`. The oracle is the
minified binary at
`/opt/homebrew/.../claude-code-darwin-arm64/claude`; the readable (older) TS
canonical reference is `/Users/luolingfeng/Projects/LingXi-Next/claude-code/src/`
(notably `cli/print.ts`, `cli/structuredIO.ts`, `bridge/*`).

---

## 0. Authoritative subtype set (from the binary)

`LC_ALL=C strings $BIN | grep -E '^(...)$'` confirms the full inbound
`control_request.request.subtype` enumeration the host can send:

```
initialize            set_permission_mode   set_model
interrupt             can_use_tool          mcp_message
hook_callback         get_session_cost      get_usage
rewind_files          cancel_async_message  control_cancel_request
apply_flag_settings
```
Plus (seen in print.ts switch but not in the bare-string grep):
`end_session`, `set_max_thinking_tokens`, `mcp_status`, `get_context_usage`,
`seed_read_state`, `mcp_set_servers`, `reload_plugins`, `mcp_reconnect`.

`can_use_tool` is **outbound** (engine→host); the rest are **inbound**
(host→engine). The binary also exposes `"type":"interrupt"` and an
`$tengu_tool_use_can_use_tool_rejected` telemetry anchor confirming the
can_use_tool path is live.

The TS switch in `cli/print.ts:2830-3140` is the canonical dispatch body and is
the spec the LingXi reader loop must mirror.

---

## (a) stdin reader + stdout writer

### Current control_request dispatch — REJECT-UNSUPPORTED stub

`apps/cli/src/stream_json_input.rs:159-168` — the `"control_request"` arm of
`process_line`:
- requires the `request` field (fatal `InputError::MissingRequest` if absent,
  line 161-163), matching the binary's `Missing request on control_request`.
- then prints to stderr `"control_request received but full control protocol is
  not yet implemented (P5 deferred)"` and returns `FrameAction::Consumed`
  (line 166-167). **Nothing is dispatched, nothing is answered.**

`"control_response"` arm (line 170-173): silently `Consumed` — no pending-request
resolution. `normalize_control_message_keys` (line 74-89) already does the
`requestId`→`request_id` rename (top-level + nested `response.requestId`) that
the protocol needs, so the key-normalization plumbing is DONE.

### The stdin reader loop — BLOCKING, DRAIN-TO-EOF (the central architectural gap)

`apps/cli/src/stream_json_input.rs:287-320` `read_input_turns(reader, replay,
session_id) -> Result<Vec<UserTurn>, InputError>`:
- a **synchronous** `for line in reader.lines()` loop that processes EVERY line
  and returns a `Vec<UserTurn>` only after stdin reaches **EOF**.

Driven from `apps/cli/src/run.rs:322-326` inside
`run_stream_json_input_loop`:
```rust
let turns = tokio::task::spawn_blocking(move || {
    let stdin = std::io::stdin();
    read_input_turns(stdin.lock(), replay, &session_id_for_replay)
}).await;
```
Then turns are run sequentially (`run.rs:354-376`).

**Consequence / gap:** the reader collects *all* turns up front, then runs them.
There is no concurrent inbound channel while a turn is in flight. A control
protocol REQUIRES the reader to run **concurrently with** the turn loop so that
`interrupt` / `set_model` / `set_permission_mode` / `control_response` (the
answer to an outbound `can_use_tool`) arrive *during* a turn. This is the single
biggest structural change: `read_input_turns` must become a streaming async task
that pushes frames onto channels (user turns → a turn queue; control frames →
a control dispatcher; control_responses → the pending-request map) rather than
draining to a `Vec`.

TS reference model: `cli/structuredIO.ts` `read()` is an `AsyncGenerator`
consumed by the `for await (const message of structuredIO.structuredInput)` loop
in `print.ts:2816`, which handles `control_request` **inline** (no queue) while
`user` frames feed the turn `run()`. `control_response` is resolved by
`StructuredIO.processLine` against `pendingRequests` (structuredIO.ts:374-408),
NOT surfaced to the loop.

### The stdout writer — `StreamJsonStream`

`apps/cli/src/stream_json.rs:179-206`. Owns `out: Arc<Mutex<std::io::Stdout>>`
(line 180). Every emit takes `self.out.lock().await` then calls
`emit_line(&mut out, &frame)` (line 38-44) which writes compact JSON + `\n`,
escaping U+2028/U+2029. **It CAN already write arbitrary frames** — every
`emit_*` method is just "lock stdout, write one JSON line." Adding
`emit_control_request` / `emit_control_response` is mechanically trivial
(a new method that builds the frame and calls `emit_line`).

Why a dedicated control-plane writer is still needed:
- The `out` mutex makes each *individual* line atomic, but it does NOT impose a
  total order between a control frame and the data frames an in-flight turn is
  emitting. Two tasks (the turn's `OutputStream` callbacks vs. a control
  responder) racing for the lock can **interleave at line granularity** — fine
  for correctness (each line is whole) but it means a control frame can
  "overtake" a queued data frame.
- The binary/TS design forbids overtaking. `cli/structuredIO.ts:160-162` is the
  exact design lock:
  > `// sendRequest() and print.ts both enqueue here; the drain loop is the only
  > writer. Prevents control_request from overtaking queued stream_events.`
  i.e. **one `outbound` Stream queue, one drain task is the sole writer**; every
  producer (turn output + control requests + control responses) `enqueue`s, and
  a single consumer serializes to stdout in FIFO order.

**Recommendation / cleanest seam:** replace `Arc<Mutex<Stdout>>` with a single
writer task fed by an `mpsc` channel (an `outbound` queue). `StreamJsonStream`
becomes a producer that pushes `Value`s onto the channel instead of locking
stdout; a new `ControlPlaneWriter` (same channel) pushes control frames. This is
a localized refactor of `stream_json.rs` (the `out` field + `emit_line` call
sites) and matches the TS `Stream<StdoutMessage>` + single drain loop 1:1. The
`escape_line_terminators` + `\n` framing stays unchanged.

### Wiring fact that makes this easy

`apps/cli/src/lib.rs:146-170`: the SAME `Arc<StreamJsonStream>` is
(1) installed as the orchestrator's `OutputStream`
(`let adapter: Arc<dyn platform_api::OutputStream> = stream.clone()` → `build_runtime`)
AND (2) handed by value into `run_stream_json_input_loop(... stream ...)`. So the
loop already holds the writer handle — a control responder built on the same
shared stream needs no new plumbing to reach stdout.

---

## (b) The permission gate — where a can_use_tool-over-stdio decider plugs in

### Gate trait surface (the workspace-authoritative seam)

`platform-api/src/permission_gate.rs` — `PermissionGate` (line 116). The orchestrator
holds exactly one: `orchestrator/src/conversation.rs:563`
`pub(crate) perms: Arc<dyn PermissionGate>`. The turn loop consults it at:
- `orchestrator/src/turn_loop.rs:2329` `orch.perms.resolve_detailed(name,
  &effective_input).await` → returns `PermissionResolution::{Allow, Deny{..},
  Ask}` (the source-gated path that fires `PermissionRequest`/`PermissionDenied`
  hooks).
- `orchestrator/src/turn_loop.rs:2427` `orch.perms.check(name,
  &effective_input).await` (and `check_in_plan_mode`, `check_after_hook_allow`,
  `check_with_worker`).

`permission/src/gate.rs` is a thin re-export shim of the traits-crate types.

### How the gate is built at boot (and why it's currently static)

`apps/engine-desktop/src/lib.rs:2532-2560` selects the **inner transport gate**:
- `cfg.injected_permission_gate` present → the TUI's `TuiPermissionGate` (interactive).
- else `cfg.use_noop_permission_gate` (the `--print` default) → `DenyOnAskGate`
  (when `deny_unresolved_ask`) or `NoOpPermissionGate`.
- else → `AdapterPermissionGate` (the bridge transport's prompt gate).

Then `apps/engine-desktop/src/lib.rs:2784-2808` wraps it: when enforcement is on
(`LINGXI_ENFORCE_PERMISSIONS` default on) it builds
`PermissionPolicy::from_rules(mode, rules)` (rule/mode/read-only/sandbox logic)
into an `Arc<PermissionPolicy>` and constructs
`PolicyPermissionGate::new(policy, perms)` — `policy` resolves rule/mode/deny
first, and only an unresolved `Ask` delegates to the inner `perms` transport.

### Where the can_use_tool-over-stdio decider goes

The cleanest plug-in point is a **new inner transport `PermissionGate` impl** —
call it `StdioControlPermissionGate` — substituted for
`NoOpPermissionGate`/`DenyOnAskGate`/`AdapterPermissionGate` at
`engine-desktop/src/lib.rs:2539-2550`. It would:
- on `check`/`resolve_detailed`/`check_with_worker`, **emit an outbound
  `control_request{subtype:"can_use_tool", tool_name, input, tool_use_id, ...}`**
  through the control-plane writer, register a pending promise keyed by
  `request_id`, and `.await` the matching inbound `control_response`, mapping its
  `{behavior:"allow"|"deny", message, updatedInput?}` onto
  `PermissionDecision::{Allow, Deny{reason}}`.
- Keeping `PolicyPermissionGate` as the OUTER wrapper is correct and matches the
  TS `canUseTool` (`cli/print.ts:4152-4176`): rule/mode deny+allow resolve
  locally first (`hasPermissionsToUseTool`), and only an `ask` behavior delegates
  to the permission-prompt transport — here the can_use_tool round-trip.
- `check_with_worker` already threads `PromptWorker` (subagent attribution,
  permission_gate.rs:30-37,138-146) — map it into the request's worker fields.

The TS outbound mechanism to mirror (`cli/structuredIO.ts`):
- `sendRequest<Response>(...)` (line 469): mint `request_id`, `outbound.enqueue`
  the control_request, register `pendingRequests.set(requestId, {resolve,...})`
  (line 512), `return new Promise(...)` (line 511).
- inbound `control_response` (structuredIO.ts:374-408): `pendingRequests.get(
  response.request_id)` → resolve → `pendingRequests.delete`. Dedup via
  `resolvedToolUseIds` for `can_use_tool` (line 176-187) so a late/duplicate
  response can't double-resolve a tool_use (would otherwise 400 on non-unique
  tool_use ids).
- `can_use_tool` callers: structuredIO.ts:590, 737. Sandbox network-ask uses the
  same path via `createSandboxAskCallback` (print.ts:617-620).

LingXi gap: NO `pendingRequests` map, NO outbound `control_request` emitter, NO
`control_response` resolver exists today (`stream_json_input.rs:170-173`
discards `control_response`). All three are net-new and belong next to the
control-plane writer.

---

## (c) Interrupt (control_request `interrupt`) → cancel an in-flight turn

### The CancellationToken plumbing EXISTS — but the print path doesn't use it

The orchestrator has cancellable entry points:
- `conversation.rs:4902` `run_turn_with_cancel(&self, prompt, cancel:
  CancellationToken)` (batched) → `try_run_turn_cancelable` (line 4917). It races
  each API round-trip against `cancel.cancelled()` via `tokio::select!`
  (line 4987-5000) and, on cancel, injects the `INTERRUPT_MESSAGE`
  (`"[Request interrupted by user]"`, const at conversation.rs:542) and returns
  `TurnOutcome::Cancelled`. Pre-cancel at loop top is also handled (line 4967-4973).
- `conversation.rs:5080` `run_turn_streaming_with_cancel` (+ `_images` /
  `_image_sources` at 5093/5119) — the streaming twin. Here the token is threaded
  INTO `try_run_turn_streaming(prompt, images, user_cancel: Option<Cancellation
  Token>)` (conversation.rs:3750-3761), not raced. The top-of-loop guard
  (line 3864-3871) and post-tools guard (line 4616+) honor it; the
  `StreamingToolExecutor` is built via `new_with_user_cancel` (streaming_executor.rs:230,
  child token at 234) so in-flight Cancel-behavior tools get the synthetic
  reject + persist, ending the turn gracefully (`aborted_streaming`).
- `OrchestratorHandle` exposes these: `handle_impl.rs:297`
  `run_turn_streaming_with_cancel` and `:315` `run_turn_streaming_with_images`.

### The gap

The print/stream-json paths call the **non-cancellable** `run_turn(&prompt)`:
- `run.rs:57` (`run_oneshot`), `run.rs:185` (`run_stream_json_print`),
  `run.rs:367` (`run_stream_json_input_loop`). `run_turn` (conversation.rs:2947)
  delegates to `try_run_turn` (line 3580) which has NO cancel token.

So to support `interrupt`: switch the stream-json loop to drive turns via
`run_turn_streaming_with_cancel` (or `run_turn_with_cancel`), holding a
per-turn `CancellationToken`. The control dispatcher, on an inbound
`control_request{subtype:"interrupt"}`, calls `cancel.cancel()` and answers
`sendControlResponseSuccess`. This is exactly the TS handler
(`cli/print.ts:2831-2849`: `abortController.abort()` then
`sendControlResponseSuccess(message)`).

Because the cancel token lives in the (currently blocking) reader's caller, this
ALSO depends on (a) — the reader must run concurrently so `interrupt` can arrive
mid-turn. The token must be shared between the turn driver and the control
dispatcher (an `Arc`-wrapped or freshly-minted-per-turn `CancellationToken`,
mintable from `tokio_util::sync::CancellationToken`).

`control_cancel_request` (binary-confirmed; print.ts `setOnControlRequestResolved`
→ `sendControlCancelRequest`) cancels a STALE *outbound* request (e.g. a
can_use_tool the host superseded) — it maps to dropping/aborting the pending
entry in the `pendingRequests` map, distinct from `interrupt` (which aborts the
turn).

---

## (d) session / model / permission-mode mutation seams

### session() — direct mutable handle (works for model + plan_mode + history)

`conversation.rs:6048` `pub fn session(&self) -> Arc<Mutex<SessionState>>`.
`SessionState` carries `model`, `model_profile`, `plan_mode`, `session_id`,
`history`, `usage`. The resume path already mutates it in place
(`run.rs:660-672` `seed_orchestrator_session`). The lifecycle hook reads
`s.plan_mode` (conversation.rs:2996-2998).

### set_model → `switch_model` EXISTS

`handle_impl.rs:106-111`:
```rust
async fn switch_model(&self, model: &str, profile: Option<&str>) -> Result<(),HandleError> {
    let mut s = self.session.lock().await;
    s.model = model.to_string();
    s.model_profile = profile.map(str::to_string);
    Ok(())
}
```
So `control_request{subtype:"set_model", model}` maps cleanly to
`orchestrator.switch_model(model, None)` then `sendControlResponseSuccess`. TS
parity: `print.ts:2933-2944` (`"default"` → `getDefaultMainLoopModel()`; here
LingXi's `switch_model` accepts any string; resolve `"default"` to LingXi's
configured default before calling). NOTE: `switch_model` mutates session state
only; whether the live `api`/router re-resolves the model per turn from
`s.model` needs confirming (the turn builds the request from session model each
turn, so this should take effect on the next turn — matches TS
`setMainLoopModelOverride`).

### set_permission_mode → **NO mutation seam (the real gap)**

The active permission mode is **baked into the gate at boot** and held behind an
`Arc`:
- `PermissionPolicy::from_rules(mode, rules)` → `Arc<PermissionPolicy>` →
  `PolicyPermissionGate::new(policy, perms)` (engine-desktop/src/lib.rs:2787-2808).
- `orchestrator.perms: Arc<dyn PermissionGate>` (conversation.rs:563) is then
  immutable. There is **no** `set_permission_mode` / `set_mode` on the
  orchestrator, the handle, or the gate.

The trait doc explicitly calls this out (permission_gate.rs:168-190,
`check_in_plan_mode`): "claude-code reads `toolPermissionContext.mode = 'plan'`
LIVE on every check … LingXi instead builds its `PermissionPolicy` once at boot
with a fixed mode and holds it behind a shared `Arc`." The ONLY runtime-mode
seam today is the binary `plan_mode` flag on `SessionState` (consulted via
`check_in_plan_mode`), i.e. only plan-mode can be toggled live, via session
state, and only because the turn loop special-cases it.

**To support `set_permission_mode` (default/plan/acceptEdits/bypassPermissions
/dontAsk/auto):** the gate's mode must become a runtime cell. Cleanest options:
1. Give `PolicyPermissionGate` an interior `ArcSwap<PermissionMode>` (or
   `Mutex`) it reads on each `authorize_with_mode`, plus a setter; expose an
   orchestrator/handle method `set_permission_mode(mode)`. This is the most
   faithful to TS (live mode read per check).
2. Add a `SessionState.permission_mode` field that the turn loop passes to
   `check_in_plan_mode`-style overrides (extends the existing plan_mode
   precedent). Lighter but only covers what the loop special-cases.

Either way it is a **net-new mutation surface** (struct field + setter +
handle method), unlike set_model. The permission-mode→string mapping for the
init frame / responses already exists: `stream_json.rs:1000-1010`
`permission_mode_str()` (covers default/acceptEdits/bypassPermissions/dontAsk/
plan/auto/bubble). TS parity: `print.ts:2918-2932` `handleSetPermissionMode`
mutates `toolPermissionContext` and sends the response itself.

`apply_flag_settings` (binary-confirmed) is a broader live-settings mutation
(`apply-flag-settings-` anchor) — same class of gap, no seam today.

---

## (e) registries for the `initialize` response

The init payload is already collected for the `system/init` frame, so the
sources are KNOWN and reachable; the `initialize` control_response reuses them.

Sources (seen in `run.rs:97-172` `run_stream_json_print` init population and
`stream_json.rs:926-985` `build_init_params`):

| init field | LingXi source | file:line |
|---|---|---|
| `commands` (name/description/argumentHint) | `runtime.dispatcher.registry()` → `reg.list_all()` (sorted) | run.rs:121-139 |
| `agents` (name/description/model) | `orchestrator.list_agents()` → `AgentInfo{name,description,tools_allowed}` | handle_impl.rs:172-188; run.rs:143-149 |
| `models` / model listings | `orchestrator.list_model_listings()` / `list_available_models()` | handle_impl.rs:261-281 |
| `mcp_servers` (status) | `orchestrator.list_mcp_servers()` → `McpServerInfo` | handle_impl.rs:140-148; run.rs:102-115 |
| `skills` | dispatcher registry, `loaded_from=="skills"` subset | run.rs:121-137 |
| `tools` | `orchestrator.tool_names()` (+ Agent→Task rename) | conversation.rs:6057; stream_json.rs:957-961 |
| `output_style` / available styles | currently hardcoded `"default"` (gap) | run.rs:169 |
| `account`/`apiKeySource` | `detect_api_key_source()` (env-based) | stream_json.rs:989-997 |

TS init response shape to fill (`cli/print.ts:4453-4499`,
`handleInitializeRequest`):
`{commands:[{name,description,argumentHint}], agents:[{name,description,model}],
output_style, available_output_styles, models: modelInfos,
account:{email,organization,subscriptionType,tokenSource,apiKeySource,
apiProvider}, pid, fast_mode_state?}` wrapped in `{type:"control_response",
response:{subtype:"success", request_id, response: initResponse}}`. An
already-initialized re-`initialize` returns `subtype:"error", error:"Already
initialized", pending_permission_requests`.

Gaps vs TS init:
- `description` must be `formatDescriptionWithSource(cmd)` and filtered by
  `userInvocable !== false`; LingXi currently passes bare names for the system/
  init frame (run.rs:124-128) — for the control `initialize` response the richer
  `{name,description,argumentHint}` triple must be produced from the registry's
  `Command` records (the registry has these fields; just not surfaced yet).
- `account` block, `available_output_styles`, `pid`, `fast_mode_state` are not
  collected today (init frame hardcodes `output_style:"default"`,
  `fast_mode_state:"off"`, `plugins:[]`). `plugins` has no clean Runtime accessor
  (run.rs:153-156 notes this).
- `initialize.hooks` / `agents` / `systemPrompt` MERGE-from-stdin (print.ts:4370-
  4449, `createHookCallback` per matcher → `hook_callback` control_request) is a
  whole sub-protocol with no LingXi equivalent.

---

## Summary: cleanest seams vs. net-new gaps

**Already present (reuse):**
- stdout single-line atomic writer + U+2028 escaping (`stream_json.rs:38-44`).
- camelCase→snake_case control-key normalization (`stream_json_input.rs:74-89`).
- `request` field validation + exact error string (`stream_json_input.rs:159-167`).
- the SAME `Arc<StreamJsonStream>` reaches both the OutputStream and the loop
  (`lib.rs:146-170`) — writer handle already in hand.
- `CancellationToken` turn drivers: `run_turn_with_cancel` (conversation.rs:4902)
  + streaming twin (5080); handle methods (handle_impl.rs:297,315).
- `switch_model` mutator (handle_impl.rs:106) for `set_model`.
- `session()` mutable handle (conversation.rs:6048) for model/plan_mode/history.
- inner-transport `PermissionGate` slot (engine-desktop/src/lib.rs:2539-2550) —
  drop-in point for a can_use_tool decider; `check_with_worker` already carries
  subagent attribution.
- all init-response data sources (registries listed in (e)).

**Net-new (the gaps), in rough priority order:**
1. **Concurrent streaming stdin reader** (replace `read_input_turns` drain-to-Vec
   at stream_json_input.rs:287 + run.rs:322). Without it, NO inbound control
   frame can arrive mid-turn. Everything below depends on it.
2. **Single-writer outbound queue** (refactor `StreamJsonStream.out`
   `Arc<Mutex<Stdout>>` → one mpsc-fed drain task) so control frames never
   overtake data frames (TS structuredIO.ts:160-162 design lock).
3. **`pendingRequests` map + outbound control_request emitter + control_response
   resolver** (mirror structuredIO.ts `sendRequest`/processLine 374-408,469-512;
   dedup via resolvedToolUseIds 176-187). Discarded today
   (stream_json_input.rs:170-173).
4. **Control-request dispatcher** replacing the reject stub
   (stream_json_input.rs:159-168) — a switch mirroring print.ts:2830-3140 wiring
   each subtype to the seams above (interrupt→cancel.cancel(); set_model→
   switch_model; initialize→registry collect; etc.).
5. **`can_use_tool` decider gate** (new inner `PermissionGate` at
   engine-desktop:2539) that round-trips over (2)+(3).
6. **`set_permission_mode` runtime mutation** — the one mode-mutation surface
   that does NOT exist (gate mode baked at boot, engine-desktop:2787-2808).
   Requires an interior-mutable mode cell on `PolicyPermissionGate` + a setter
   + a handle method. `apply_flag_settings` is the same class of gap.
