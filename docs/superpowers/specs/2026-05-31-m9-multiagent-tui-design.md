# M9 Multi-Agent TUI Surface — Design

**Status:** Draft, awaiting user review (2026-05-31)
**Target release:** v0.10.0
**Predecessor:** v0.8.0 (M7 — TUI Surface) → M8 (Composable Engine + Mobile; CHANGELOG `[0.9.0]`, crate versions un-bumped)
**Successor (planned):** M10 (MCP UI + Pickers) → M11 (Permission & dialog fidelity) → M12 (Terminal-protocol + misc)
**Author:** luolingfeng + Claude Opus 4.8

---

## §0 Decisions Locked (from brainstorm)

| # | Question | Answer |
|---|---|---|
| Q1 | Parity boundary for "1:1 copy of claude-code's TUI" | **Agent-experience parity.** Reproduce every surface a user sees while *using* the agent; exclude Anthropic product-internal plumbing (auto-updaters, feedback/surveys, Sentry, Bedrock auth, dev bars, Claude-in-Chrome onboarding) and the remote/teleport surface that is gated on the bridge engine (P13 skeleton). The spec gives an explicit exclude list per milestone. |
| Q2 | Which deferred-TUI milestone first | **M9 — Multi-Agent / Team / Coordinator UI.** Largest, most-visible gap vs claude-code; engine building-blocks exist. Sequence after: M10 (MCP+pickers) → M11 (permissions+dialogs) → M12 (terminal-protocol+misc). |
| Q3 | How M9 handles the engine dependency | **UI-first + adapter layer.** Define a TUI-side `MultiAgentState` presentation model + extend the orchestrator→TUI bridge contract; build the entire UI surface against it, fully snapshot/behavior/parity tested. Render real data where the engine is live (`TaskRegistryHandle`), deterministic fixtures where the pool is stubbed. Same posture M7 took with its 5 deferred engine items ("render against the engine as-is"). |
| Q4 | Agent-management UI depth in M9 | **Read-only list + detail.** `AgentsList` + `AgentDetail` over the `AgentDefinition` catalog. Authoring (CreateAgentWizard / AgentEditor / Color·Model·ToolSelector) defers to a later slice (write-back risk class). The *running-conversation* multi-agent surface is fully in scope regardless. |
| C1 | Crate / TUI library / validation conventions | **Carried from M6/M7 unchanged:** `iocraft = "=0.8.3"`; hybrid validation (insta snapshots + behavior tests + parity fixtures); literal-lock discipline; one `handle_live_key` dispatcher; per-sub-plan annotated tags; no remote push from Claude. No new workspace crate — extend `tui` (+ minimal `tasks` wiring in the desktop app). |

---

## §1 Goal & Non-Goals

### Goal

**v0.10.0 completes the multi-agent TUI surface.** Where v0.8.0 (M7) finished the *single-user* experience, M9 makes the TUI render the claude-code **multi-agent** experience: the message types, status chrome, dialogs, and read-only agent discovery a user sees when subagents, in-process teammates, and background tasks are active.

It is built **UI-first against a presentation adapter** (Q3): the full surface is implemented and tested now, renders real data where the engine is already live (the `TaskRegistryHandle` task path — spool output round-trip is proven by `tasks/tests/handle_spool_round_trip.rs`), and uses a deterministic fixture feed where the execution pool is still stubbed. When the pool is un-stubbed (a separate engine milestone) the UI lights up with **no UI changes** — the adapter swaps its feed from fixture to real.

Concrete deliverables — must all ship for v0.10.0:

1. **Bridge contract + presentation model**: `MultiAgentEvent` variants on the existing orchestrator→TUI event path; a `MultiAgentState` aggregate in `AppState`; an adapter trait with a **real poller** impl (over `traits::task_registry::TaskRegistryHandle` + the coordinator mailbox) and a **fixture** impl.
2. **6 team message renderers**: `TaskAssignmentMessage`, `UserTeammateMessage` (+ plan-approval / shutdown / task-completed / idle sub-types), `UserAgentNotificationMessage`, `UserChannelMessage`, `teamMemCollapsed`, `teamMemSaved`.
3. **Status chrome**: `TeamStatus` (footer count + hint), `CoordinatorAgentStatus` (background-agent panel), `TeammateViewHeader` (viewing-teammate header), `AgentProgressLine` (per-agent tree progress line).
4. **Background-task surface**: `BackgroundTaskStatus` footer, `BackgroundTasksDialog` (list↔detail), one row renderer per `TaskState` variant (7), `ShellProgress`, and **live output tailing** via `TaskRegistryHandle::output`. Local detail dialogs (Shell / InProcessTeammate / AsyncAgent).
5. **Worker permission chrome**: `WorkerBadge` + `WorkerPendingPermission`, integrated into the M6 permission focus-trap.
6. **Agent discovery (read-only)**: `AgentsList` + `AgentDetail` over the `AgentDefinition` catalog (all `AgentSource`s; tools / model / color / permission-mode / file path); a `/agents` command + screen.
7. **Shared primitives**: `AgentColor`→theme `Style` map; task status icon/color map (`taskStatusUtils` parity); a reusable select-list/detail-nav helper (reuse M7's if present).
8. **Minimal real engine wiring**: construct the already-implemented `tasks::TaskRegistry` in the desktop composition root and set `task_registry: Some(..)` in `BuiltinToolContext` so the `Agent`/`Task`/`TeamCreate` tools have a registry and the poller has a data source. **The execution pool is NOT un-stubbed.**
9. Test surface: snapshots for every renderer/status/dialog, behavior tests for interactive flows + cross-state seams, ≥1 new parity fixture (`parity_tui_multiagent`).
10. Version reconciliation: crate `version` 0.8.0 → 0.10.0; CHANGELOG note that M8's `[0.9.0]` never bumped crate versions; annotated tags `m9.N` per sub-plan + `v0.10.0`.
11. Cross-platform compile gate green (5 targets; same posture as v0.7.0/v0.8.0).

### Non-Goals (M9 explicitly does NOT do)

- **Agent authoring** — `CreateAgentWizard`, `AgentEditor`, `ColorPicker`, `ModelSelector`, `ToolSelector`. Write-back to definition files is a distinct risk class (the M7 R7 lesson) → a later TUI slice.
- **Un-stubbing the engine pool / real teammate execution** — `StateMachinePool::allocate` is an "M1.14 stub"; making subagents and in-process teammates actually *run* to completion is an **engine** milestone. M9 defines the contract and renders against it; the fixture feed isolates the gap. **Hard line — see §4 R4.**
- **Distributed-only surfaces** — `RemoteSessionDetailDialog` / `RemoteSessionProgress`, `TeamsDialog` teleport backends, the rich `DreamDetailDialog`, `DesktopHandoff`. These need the remote/teleport/bridge engine (P13 skeleton) and are excluded by the Q1 boundary. M9 renders remote/dream/workflow/monitor task **rows** as read-only status lines for list completeness; their detail dialogs are minimal "not available in this build" placeholders.
- **The other deferred-TUI milestones** — MCP UI + pickers (M10), per-tool permission dialogs + dialog pile (M11), terminal-protocol cluster + misc surfaces (M12).
- **Byte-identical color output** — `AgentColor`/status colors map to the active theme; parity means equivalent look (carried from M7 §0 Q3).

### Success Criteria

v0.10.0 release equivalent to all of these passing:

1. A fixture multi-agent session renders all 6 team message types (and `UserTeammateMessage`'s 4 sub-types) correctly — snapshot-locked.
2. A background-bash task launched via the **real** `TaskRegistryHandle` shows a live `ShellProgress` row, appears in the `BackgroundTaskStatus` footer, opens in `BackgroundTasksDialog`, and tails its output correctly in `ShellDetailDialog`.
3. `CoordinatorAgentStatus` panel + `AgentProgressLine` render a multi-agent fixture (tree chars, tool/token counts, per-agent color); `TeammateViewHeader` + `TeamStatus` reflect roster state; enter/esc into a teammate view works.
4. `WorkerPendingPermission` integrates with the existing focus-trap — the cross-state seam (worker permission while a normal permission is pending) is tested and correct.
5. `/agents` opens `AgentsList`; selecting opens `AgentDetail` with correct tools/model/color/source/path.
6. Multi-agent state flows through the **single** bridge/adapter path — no parallel subscription (M7 §2.5 discipline); a contract test asserts the real poller's output shape matches the fixture schema.
7. `cargo test --workspace` passes; cross-platform compile gate green for 5 targets.
8. Crate versions are 0.10.0; annotated tag `v0.10.0` exists, points to the release commit.

---

## §2 Architecture

### 2.1 Crate layout — extend `tui`, plus minimal `tasks` wiring in the app

No new workspace member. `tui` grows new modules; `apps/engine-desktop` (and the `cli` init path) gain a `tasks` dependency to construct the real registry.

```
tui/src/
├── multiagent/                    ← NEW: presentation model + adapter (M9-01)
│   ├── mod.rs
│   ├── state.rs                   ← MultiAgentState (tasks/workers/coordinator_view/viewing_teammate)
│   ├── event.rs                   ← MultiAgentEvent (the single output type)
│   ├── adapter.rs                 ← MultiAgentFeed trait
│   ├── poller.rs                  ← real feed: TaskRegistryHandle tick + mailbox drain
│   └── fixture.rs                 ← deterministic scripted feed (tests + pool-stubbed parts)
├── components/
│   ├── messages/                  ← EXPANDED: +6 team renderers (M9-03)
│   │   ├── task_assignment.rs, user_teammate.rs, user_agent_notification.rs,
│   │   │   user_channel.rs, team_mem_collapsed.rs, team_mem_saved.rs
│   │   └── mod.rs                 ← dispatch table extended
│   ├── tasks/                     ← NEW: background-task surface (M9-04/05)
│   │   ├── mod.rs, row.rs (7 variants), shell_progress.rs,
│   │   ├── status_footer.rs (BackgroundTaskStatus),
│   │   ├── tasks_dialog.rs (BackgroundTasksDialog list↔detail),
│   │   └── detail/{shell.rs, in_process_teammate.rs, async_agent.rs, placeholder.rs}
│   ├── coordinator/               ← NEW: status chrome (M9-06)
│   │   ├── agent_status.rs (CoordinatorAgentStatus), agent_progress_line.rs,
│   │   ├── teammate_view_header.rs, team_status.rs
│   ├── permissions/               ← EXPANDED: worker chrome (M9-07)
│   │   ├── worker_badge.rs, worker_pending.rs
│   └── agents/                    ← NEW: read-only discovery (M9-08)
│       ├── mod.rs, agents_list.rs, agent_detail.rs
├── render/ or theme.rs            ← EXPANDED: AgentColor→Style + status icon/color maps (M9-02)
├── screens/                       ← agents screen route + tasks-dialog route via active_screen
└── telemetry.rs                   ← EXPANDED: screen/panel events (M9-09)
```

### 2.2 New / changed dependencies

| Crate | Where | Why |
|---|---|---|
| `tasks` (path dep) | `apps/engine-desktop`, `apps/cli` | construct `TaskRegistry` in the composition root |
| `traits::task_registry::*` | `tui` (already transitively available) | the `TaskRegistryHandle` the poller consumes |
| `coordinator` (path dep, optional) | `tui` or app | mailbox drain for teammate messages (confirm DAG at M9-01; may route through a handle to avoid a `tui→coordinator` edge — see §4 R5) |

No third-party crates. The check-deps gate (`scripts/check_deps.py`) permits apps depending on `tasks`/`coordinator` (apps are composition roots). A `tui → coordinator` edge is evaluated at M9-01 against the gate; if disallowed, the mailbox is surfaced through an existing handle/trait instead (§4 R5).

### 2.3 Data-flow — three layers, one path

```
ENGINE (as-is)                       ADAPTER (new)                    TUI (new)
──────────────                       ─────────────                    ─────────
TaskRegistryHandle ─poll(list/get/output)─┐
 (REAL — spool round-trip proven)         │
coordinator mailbox ─drain(TeammateMessage)┤
                                          ▼
SubagentEvent (pool = STUBBED) ─┐  MultiAgentFeed ─MultiAgentEvent─▶ MultiAgentState ─▶ renderers
fixtures (deterministic) ───────┴─▶ (drained by the SAME loop          (in AppState; mutated     + status chrome
                                     that drains TurnEvent)             only by apply_event)       + dialogs
```

1. **Extend the single channel — never add a parallel one.** The M6 ship-blocker was a parallel key path; M7 §2.5 fixed it with one dispatcher. **Default representation:** a sibling `MultiAgentEvent` enum drained by the *same* render-loop `select!` that drains `TurnEvent` (keeps `TurnEvent` focused on the turn lifecycle). The concrete channel wiring is settled in M9-01, but the invariant is fixed regardless: **one drain loop, one mutation seam (`apply_event`), and no renderer subscribes to the engine directly.**
2. **`MultiAgentState` is a pure presentation model** in `AppState`, mutated only by the existing `streaming::apply_event` seam. Every renderer is a pure function of it → snapshot-testable exactly like the M7 renderers.
3. **One `MultiAgentFeed` trait, two impls, one output type.** *Real* (`poller.rs`): a tick over `TaskRegistryHandle::{list,get,output}` + a mailbox drain. *Fixture* (`fixture.rs`): a scripted deterministic feed for tests **and** for the pool-stubbed agent-execution parts. Both emit `MultiAgentEvent` → swappable; a contract test (M9-01) asserts the real impl's output shape against the fixture schema.
4. **Minimal real engine wiring (cheap, low-risk):** construct `tasks::TaskRegistry::new(runtime, fs, output_manager)` (all three inputs already exist in the desktop root) and set `task_registry: Some(..)` in `BuiltinToolContext`. The `Agent`/`Task`/`TeamCreate`/`TeamDelete` tools then have a registry; the poller has a data source. **The pool stays stubbed.**
5. **Keymap routing extends the §2.5 priority ladder** — no new top-level path:
   ```
   1. pending_permission.is_some()   → permission dialog (M6) [+ WorkerPendingPermission, M9-07]
   2. active_screen.is_some()        → active screen (M7) [+ BackgroundTasksDialog, AgentsList, M9-05/08]
   3. input overlay (palette/…)      → (M7)
   4. teammate-view filter mode      → transcript filter (M9-06)
   5. prompt_input / scrollback      → (M7)
   ```

### 2.4 Mapping claude-code data shapes → LingXi engine types

The renderers are literal-locked to claude-code's *output* (strings/layout), but read LingXi's *types*. Key correspondences:

| claude-code concept | LingXi engine type |
|---|---|
| background task (local_bash / local_agent / remote_agent / in_process_teammate / workflow / monitor / dream) | `tasks::state::TaskState` (7 variants) + `TaskStatus` |
| task status icon/color | derived from `TaskStatus` (Pending/Running/Completed/Failed/Killed) |
| teammate / worker roster | `coordinator::team_registry::{WorkerAgent, WorkerStatus}` |
| teammate message (assignment / completion / approval / idle) | `coordinator::mailbox::{TeammateMessage, MessageSender}` |
| agent type definition | `agent::definition::AgentDefinition` + `AgentSource` |
| per-agent color | `agent::display::AgentColor` (10 colors) + `AgentColorManager` |
| subagent progress (tool count / tokens) | `agent::runner::SubagentEvent::Progress` |

Where a claude-code field has no LingXi equivalent (e.g. teleport backend type), the renderer omits it or shows the placeholder — never invents engine state.

### 2.5 What does NOT change

- The orchestrator turn loop, `OrchestratorHandle`, the existing `TurnEvent` semantics for single-user flows.
- All M7 renderers, screens, vim/palette/completion, theme system, VirtualMessageList.
- `-p` print mode and `--no-tui` stdio REPL.
- The execution pool (`StateMachinePool`) stays as-is (stubbed); M9 adds zero engine-loop behavior.

### 2.6 Literal-lock discipline (carried from M6/M7 §2.8)

Every user-visible string matches claude-code's source byte-for-byte unless there is an explicit reason to diverge. The implementer for each renderer/dialog reads the equivalent claude-code `.tsx` first and copies the exact literal. M9-09 extends the literal-lock catalog (`docs/superpowers/literals/`) with the multi-agent renderers + dialogs.

### 2.7 Telemetry

Additive only (new names, no renames). Final set + count audited and locked in M9-09 (the M6 "330→326" lesson):
- Screen/panel lifecycle: `tengu_tui_background_tasks_opened`/`_closed`, `tengu_tui_agents_screen_opened`/`_closed`, `tengu_tui_teammate_view_entered`.
- A release marker `lingxi_core_v0_10_0_released` (once-guarded; follows the prior pattern).

Every registered name gets a real emit site (M6 discipline).

---

## §3 Per-Sub-Plan Deliverables

9 sub-plans, sequential, each ending with annotated tag `m9.N` and a two-stage review before the next.

### M9-01 — Bridge contract + `MultiAgentState` + adapter
**Goal:** the data path. **Lands:** `tui/src/multiagent/` (`state.rs`, `event.rs`, `adapter.rs` `MultiAgentFeed` trait, `poller.rs` real impl, `fixture.rs`); `MultiAgentEvent` wired into the existing render-loop drain + `streaming::apply_event`; `apps/engine-desktop` + `cli` init construct `tasks::TaskRegistry` → `task_registry: Some(..)`. DAG check for any `tui→coordinator` edge (§4 R5). **Tests:** adapter→state transitions; fixture determinism; **contract test** (real-poller output shape == fixture schema); desktop builds with the registry wired. **Tag:** `m9.1`

### M9-02 — Shared primitives
**Goal:** the maps every renderer needs. **Lands:** `AgentColor`→theme `Style` map; task status icon/color map (`taskStatusUtils` parity — icons + colors for Pending/Running/Completed/Failed/Killed); reusable select-list/detail-nav helper (reuse M7's if one exists; else extract). **Tests:** color/icon snapshot tables (all variants). **Tag:** `m9.2`

### M9-03 — Team message renderers (6)
**Goal:** the multi-agent transcript. **Lands:** `task_assignment.rs`, `user_teammate.rs` (+ plan-approval / shutdown / task-completed / idle sub-types), `user_agent_notification.rs`, `user_channel.rs`, `team_mem_collapsed.rs`, `team_mem_saved.rs` + dispatch entries; each reads claude-code TSX first (literal lock). **Tests:** snapshot per renderer + per sub-type. **Tag:** `m9.3`

### M9-04 — Background-task rows + progress + output tail
**Goal:** per-task rendering. **Lands:** row renderer per `TaskState` variant (7); `ShellProgress` (icon, elapsed, exit/signal); read-only rows for remote/dream/workflow/monitor; the live output-tail view backed by `TaskRegistryHandle::output` (incremental offset reads). **Tests:** snapshot per type; behavior (output tailing advances on new spool bytes). **Tag:** `m9.4`

### M9-05 — Task status footer + dialog
**Goal:** the task management surface. **Lands:** `BackgroundTaskStatus` footer (visible-count, selected highlight, eviction tick, hide-when-only-teammates rule); `BackgroundTasksDialog` (list↔detail nav); local detail dialogs (`ShellDetailDialog`, `InProcessTeammateDetailDialog`, `AsyncAgentDetailDialog`); remote/dream detail = placeholder; routed via `active_screen` (priority 2). **Tests:** behavior (open/nav/select/detail/close, tick eviction); snapshot. **Tag:** `m9.5`

### M9-06 — Coordinator status chrome
**Goal:** the team/coordinator UI chrome. **Lands:** `CoordinatorAgentStatus` panel (visible local-agent tasks, run/stop hints); `AgentProgressLine` (tree chars ├─/└─, tool-use + token counts, per-agent color, Initializing/Done/Running-in-background states); `TeammateViewHeader` (viewing-@name · esc to return); `TeamStatus` footer (teammate count + enter-to-view hint); teammate-view transcript filter mode (priority 4). **Tests:** snapshot; behavior (panel nav, enter/esc teammate view). **Tag:** `m9.6`

### M9-07 — Worker permission chrome
**Goal:** swarm-worker permission visuals into the focus-trap. **Lands:** `WorkerBadge` (colored @name); `WorkerPendingPermission` (waiting-for-lead, tool/action lines); integrated into the M6 permission priority (priority 1). **Tests:** behavior — **cross-state seam** (worker permission while a normal permission is pending; routing stays correct); snapshot. **Tag:** `m9.7`

### M9-08 — Agent discovery (read-only)
**Goal:** browse the agent catalog. **Lands:** `AgentsList` (all `AgentSource`s, grouped; model/memory/override badges); `AgentDetail` (type, tools incl. wildcard/invalid handling, model, color, permission mode, file path); `/agents` command + screen route (`active_screen`, priority 2). **Tests:** behavior (list/select/detail/close); catalog read; snapshot. **Tag:** `m9.8`

### M9-09 — Parity fixture + release v0.10.0
**Goal:** cross-cutting validation, version reconciliation, release. **Lands:** `parity_tui_multiagent.json` + driver (high-value strings, sub-type markers, layout structure for renderers/chrome/dialogs); literal-lock catalog extended; telemetry count audited + locked; crate versions 0.8.0 → 0.10.0 (+ CHANGELOG note reconciling M8's un-bumped `[0.9.0]`); release doc + CHANGELOG + README; annotated `m9.9` + `v0.10.0`. **Final review** explicitly probes cross-state seams (§5.6): worker-permission while a screen is open; background-tasks dialog while a permission is pending; teammate-view while a task completes. **Tests:** full cumulative suite; M7 fixtures still pass; release marker emits once. **Tag:** `m9.9` + `v0.10.0`

### Summary

| Sub-plan | Lands | Tag |
|---|---|---|
| M9-01 | Bridge contract + MultiAgentState + adapter | m9.1 |
| M9-02 | Shared primitives (color/status maps, select-list) | m9.2 |
| M9-03 | Team message renderers (6) | m9.3 |
| M9-04 | Background-task rows + progress + output tail | m9.4 |
| M9-05 | Task status footer + dialog | m9.5 |
| M9-06 | Coordinator status chrome | m9.6 |
| M9-07 | Worker permission chrome | m9.7 |
| M9-08 | Agent discovery (read-only) | m9.8 |
| M9-09 | Parity fixture + release v0.10.0 | m9.9 + v0.10.0 |

**Estimate:** 9 sub-plans, ~90–110 tasks, ~3–4 calendar weeks at sustained pace. Variance ±1 week (the adapter contract + cross-state seams are the drivers; no vim/VirtualList-class risk).

---

## §4 Risk Register

### High-impact

| # | Risk | Impact | Probability | Mitigation |
|---|---|---|---|---|
| R1 | **Fixture/real divergence** — UI looks right in tests but wrong against the live poller | High | Medium | Single `MultiAgentEvent` output type for both impls; a **contract test** (M9-01) asserts the real poller's output shape against the fixture schema; the real task path is exercised end-to-end in M9-04/05 (it is genuinely live). |
| R2 | **Cross-state seam regressions** — the recurring M6/M7 ship-blocker; worker-perm × tasks-dialog × teammate-view × existing focus-trap all contend for live keys | High | Medium-high | Single `handle_live_key` dispatcher; strict priority *extension* (§2.3); explicit seam tests in M9-07 + the M9-09 final review. |
| R3 | **Literal drift** across 6 renderers + many dialogs (M7 R6 echo) | Medium | High | Implementer reads TSX first; literal-lock catalog extended M9-09; parity fixture locks high-value strings. |
| R4 | **Engine-liveness creep** — temptation to un-stub the pool mid-M9 turns a TUI milestone into an engine milestone | High (schedule) | Medium | **Hard line:** M9 wires only the already-complete `TaskRegistry`; pool un-stub is explicitly a separate engine milestone; the fixture feed isolates the gap. Any pool change → STOP, defer, document. |

### Medium / low

| # | Risk | Impact | Mitigation |
|---|---|---|---|
| R5 | **DAG edge** — a `tui → coordinator` dependency for the mailbox drain may violate the check-deps gate | Medium | Evaluate at M9-01; if disallowed, surface the mailbox through an existing handle/trait (e.g. an `OutputStream`-style callback the orchestrator already owns) rather than a direct crate edge. |
| R6 | **Output-tail performance** — naive re-read of a large spool file each tick | Low | Use the existing incremental `output_offset` (byte-offset) reads `TaskRegistryHandle::output` already supports; only fetch new bytes. |
| R7 | **Version reconciliation confusion** — M8's `[0.9.0]` CHANGELOG vs un-bumped 0.8.0 crates | Low | M9-09 bumps crates 0.8.0 → 0.10.0 in one step and adds a CHANGELOG note; no attempt to retro-tag v0.9.0. |
| R8 | **`UserTeammateMessage` sub-type fan-out** — 4 nested types (plan-approval/shutdown/task-completed/idle) parsed from one XML-tagged payload | Medium | Snapshot each sub-type; literal-lock the tag parsing against the TSX. |

### Hard gates

1. **End of M9-01:** the real poller produces `MultiAgentEvent`s whose shape matches the fixture schema (contract test green), and the desktop app builds with `TaskRegistry` wired. If the `tui→coordinator` edge is disallowed → reroute the mailbox (R5) before proceeding.
2. **End of M9-05:** a real background-bash task renders end-to-end (footer → dialog → tailing detail) against the live `TaskRegistryHandle`. This is the proof that the "UI-first + adapter" bet pays off for the live half.

None block the *milestone* — each degrades scope, not schedule.

---

## §5 Verification

### 5.1 Test categories (hybrid, unchanged from M6/M7)

| Layer | Tool | Scope |
|---|---|---|
| Unit | `cargo test` | color/icon maps, status formatting, adapter state transitions, output-offset math |
| Behavior | `cargo test` + driver | dialog nav, focus-trap seams, output tailing, teammate-view mode, poller→state |
| Snapshot | `insta` | every renderer / status line / dialog × multiple states |
| Parity | fixture + driver | `parity_tui_multiagent` — high-value strings + sub-type markers + layout structure |
| Integration | PTY (`expectrl`/`rexpect`) | open/close BackgroundTasksDialog + AgentsList; terminal restore |

### 5.2 Per-area test budget

| Area | Snapshots | Behavior |
|---|---|---|
| Adapter + MultiAgentState | — | 8+ (transitions, fixture determinism, **contract test**) — **critical** |
| Shared primitives (color/icon) | 4+ | — |
| Team renderers (M9-03) | ~12 (6 renderers + sub-types) | a few (folding) |
| Task rows + progress + tail | ~9 | 4+ (tailing) |
| Task footer + dialog | 2 | 6+ (nav, tick, seam) |
| Coordinator chrome | 4 | 4+ (panel nav, teammate view) |
| Worker permission | 2 | 4+ (**cross-state seam**) — **critical** |
| Agents read-only | 2 | 4+ |

**Targets:** ~35 snapshots, ~30 behavior tests, ~2 PTY smokes by M9-09.

### 5.3 Parity fixture (≥1 new in M9-09)

- **`parity_tui_multiagent.json` + driver** — the 6 team renderers' high-value strings + sub-type markers; task-status icons/labels; `AgentProgressLine` structure (tree chars, counts); `BackgroundTasksDialog` layout; `AgentDetail` field labels. M7 fixtures (`parity_tui_renderers`, `parity_tui_screens`, etc.) continue passing.

### 5.4 Workspace verification gate (every sub-plan)

Run **from inside `lingxi-code/`** (toolchain pins rust 1.82.0):

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo check --workspace --target {x86_64-unknown-linux-gnu, x86_64-apple-darwin, x86_64-pc-windows-gnu, aarch64-linux-android, aarch64-apple-ios}
scripts/check-deps.sh
```

Known flakes (allowed rerun): the M7-documented set (`rapid_writes_collapse_to_single_event`, `streaming_concurrent_tools_test`, posix fs_watch FSEvents timing).

### 5.5 Manual verification checklist

Headless agents can't drive a real terminal. Each sub-plan lists a human smoke item; the **final human smoke** before v0.10.0 covers: a fixture multi-agent session renders all team message types; a real background bash task tails in the dialog; coordinator panel + progress lines render; `/agents` opens and navigates; worker-permission seam behaves. Run on tmux + a truecolor terminal.

### 5.6 Final-review discipline (carried from M6/M7)

The M9-09 final review explicitly probes cross-state seams: **worker-permission while a screen is open; background-tasks dialog while a permission is pending; teammate-view while a task completes.** Named step, not an afterthought.

---

## §6 Schedule

### 6.1 Cadence

Single-Claude pace, sequential. Calibrated from M6 (9 sub-plans ~3.5wk) and M7 (16 ~5.5wk).

| Sub-plan | Tasks | Calendar | Cumulative |
|---|---|---|---|
| M9-01 | 12-16 | 3-4d | 4d |
| M9-02 | 6-8 | 1-2d | 6d |
| M9-03 | 12-14 | 2-3d | 9d |
| M9-04 | 10-12 | 2d | 11d |
| M9-05 | 12-14 | 2-3d | 14d |
| M9-06 | 10-12 | 2d | 16d |
| M9-07 | 8-10 | 1-2d | 18d |
| M9-08 | 8-10 | 1-2d | 20d |
| M9-09 | 12-14 | 2-3d | 23d |

**Total: ~90–110 tasks, ~3–4 calendar weeks. Variance ±1 week** (M9-01 adapter contract + cross-state seams drive it).

### 6.2 Dependencies

```
M9-01 (bridge+state+adapter) ──▶ everything
M9-02 (primitives) ──▶ M9-03..08
M9-03 renderers ┐
M9-04 task rows ┼─ independent after 01/02
M9-06 chrome    ┘
M9-05 task dialogs ← M9-04
M9-07 worker perms ← M9-01 + existing permission system
M9-08 agents read-only ← AgentDefinition catalog (independent)
M9-09 ← everything
```

Critical path: M9-01 → M9-02 → breadth → M9-09.

### 6.3 Slip handling

>1.5× estimate → (1) scope creep: defer slice to M10, document; (2) real blocker: BLOCKED, escalate; (3) hidden dependency: precursor sub-plan M9-Na. R4 (pool un-stub) and R5 (DAG edge) carry pre-authorized degrade paths so they slip scope, not schedule.

### 6.4 Tag and release policy

Per-sub-plan annotated tags `m9.1`…`m9.9`, local only. Release tag `v0.10.0` in M9-09. **No remote push from Claude.** No force-push / skip-hooks / amends.

### 6.5 Worktree strategy

Dedicated worktree `m9-execution` on branch `m9-execution`, created at M9-01 via `superpowers:using-git-worktrees`. After M9-09 lands `v0.10.0`, fast-forward merge to the integration branch via `superpowers:finishing-a-development-branch`, with the cross-state-seam review (§5.6) before merge.

### 6.6 After M9

1. **Pause for review** — no auto-progression.
2. **M10 brainstorm** — MCP UI + pickers (the next roadmap milestone).
3. The roadmap continues: M11 (permission & dialog fidelity) → M12 (terminal-protocol + misc). The gated layer (remote/teleport/bridge, voice/grove, IDE) waits on its engine support and gets its own brainstorm when ready.

---

## References

- M7 design (predecessor): `docs/superpowers/specs/2026-05-29-m7-tui-surface-design.md`
- M6 design: `docs/superpowers/specs/2026-05-28-m6-tui-foundation-design.md`
- M8 design: `docs/superpowers/specs/2026-05-29-m8-composable-engine-mobile-design.md`
- Engine multi-agent types:
  - `tasks/src/state.rs` — `TaskState` (7 variants), `TaskStatus`, `TaskStateBase`
  - `coordinator/src/team_registry.rs` — `WorkerAgent`, `WorkerStatus`, `TeamRegistry`
  - `coordinator/src/mailbox.rs` — `TeammateMessage`, `MessageSender`, `MailboxRouter`
  - `agent/src/definition.rs` — `AgentDefinition`, `AgentSource`, `AgentToolPolicy`
  - `agent/src/display.rs` — `AgentColor`; `agent/src/color_manager.rs` — `AgentColorManager`
  - `agent/src/runner.rs` — `SubagentEvent`; `agent/src/pool.rs` — `StateMachinePool` (M1.14 stub)
  - `traits::task_registry::TaskRegistryHandle` (consumed by the poller); `tasks/tests/handle_spool_round_trip.rs` (proves live output)
  - `tui/src/events/orchestrator_bridge.rs` — `TurnEvent`, `BridgeOutputStream` (the path M9 extends)
- claude-code reference source: `/Users/luolingfeng/Projects/LingXi-Next/claude-code/src/`
  - `components/messages/` — the 6 team renderers
  - `components/teams/`, `CoordinatorAgentStatus.tsx`, `TeammateViewHeader.tsx`, `AgentProgressLine.tsx` — status chrome
  - `components/tasks/` — background-task dialogs + progress + `taskStatusUtils`
  - `components/permissions/WorkerBadge.tsx`, `WorkerPendingPermission.tsx`
  - `components/agents/AgentsList.tsx`, `AgentDetail.tsx`

---

**End of design.**
