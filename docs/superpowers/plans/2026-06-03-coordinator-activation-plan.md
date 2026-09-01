# Implementation Plan — FULL Coordinator Multi-Agent Activation

**Date:** 2026-06-03
**Workspace:** `/Users/luolingfeng/Projects/LingXi-Next/.claude/worktrees/m10-native-apps/lingxi-code`
**Design reference:** `docs/superpowers/specs/2026-06-02-m10-native-apps-design.md` §0.9 + §5.6 and the FINAL DESIGN JSON carried into this plan.
**Status:** ready to execute (TDD; tests-first per task).

---

## 1. Goal & Scope

Make coordinator multi-agent mode **actually enterable and observable in the production desktop engine** so that `active_workers > 0` in real runs and the count flows live to every client via the reserved-now-live `ClientEvent::CoordinatorStatus { active_workers, team }` DTO.

**In scope (this pass):**

1. A real **`TaskRegistry::spawn(...)`** dispatch method that looks up a registered handler and actually runs it (today `create()` only inserts a `Pending` row and never calls a handler — confirmed at `tasks/src/registry.rs:59-109`; its doc literally says "Spawning the actual worker is done by handlers").
2. Construct **one `Arc<TeamRegistry>` + one `Arc<CoordinatorMode>` per `build()`** in `apps/engine-desktop`, make the registry observable, and wire the shared `MailboxRouter` into `BuiltinToolContext.mailbox_router` (today `None` at `apps/engine-desktop/src/lib.rs:636`).
3. **Build-time mode activation** via a new additive `DesktopConfig.session_started_as_coordinator` flag (default `false`), with a defense-in-depth call()-time gate via `CoordinatorMode::is_enabled`.
4. **Worker registration that reflects real execution:** `TeamCreate` spawns a real `InProcessTeammate` task, captures the handler-generated `task_id`, writes it back onto the `WorkerAgent`, and a `CoordinatorStatusSink` maps the teammate's `TaskStatus` transitions onto `WorkerStatus`.
5. **PUSH live scalar:** additive default-no-op `OutputStream::emit_coordinator_status`, overridden in `AdapterOutputStream` (mirrors the §0.7 `emit_thinking`/`emit_usage` template), fired from the `CoordinatorStatusSink`.
6. A **real end-to-end test** proving `active_workers > 0` flows from a `TeamCreate` call to a `ClientEvent::CoordinatorStatus`, that MUST fail against the hollow `create()`-only implementation.
7. **Reserved→live contract flip** (no `CLIENT_PROTOCOL_VERSION` bump) + snapshot re-bless.

**PHASE 2 (per-worker TUI roster chrome) is structured but deferred** — the GOAL is met by the PUSH scalar alone. PHASE-2 tasks (PULL roster, `ListingKindDto::Coordinator`, roster DTO, TUI feed, bridge poll) are included as designed-but-lower-priority and clearly marked.

**Explicit NON-GOALS (acceptance criteria, not risks):**

- **Mobile coordinator wiring.** `engine-mobile build_mobile_inner` binds `subagent_spawner:None`, `task_registry:None`, `mailbox_router:None`, `budget_enforcer:None` (`apps/engine-mobile/src/host.rs:372-375`) and constructs no `StateMachinePool`. The entire InProcessTeammate dependency set is absent. Do **not** add the `coordinator` dep to `engine-mobile/Cargo.toml` this pass.
- **Richer worker status transitions** (`Idle` / `AwaitingMessage` / per-turn `Completed`). The current `TaskStatusSink` surface cannot derive them (`terminal_status` returns `None` for `Completed` by design — `tasks/src/handlers/in_process_teammate.rs:245-253`). `active_workers` is scoped to the two transitions the sink can drive: `Running → Working` and `Failed`/`Killed → terminal`. A clean-finishing teammate stays `Working` until killed. Richer transitions require a new non-terminal `SubagentEvent` tap (follow-up).

## 2. Guardrails (apply to every task)

- **Additive-only.** Default sessions (`session_started_as_coordinator = false`) MUST be byte-identical: coordinator `TeamCreate`/`TeamDelete` NOT registered, `tool_team` builtins unchanged, mode off, zero teammate tasks spawned, advertised tool list byte-identical.
- **Version guard.** NO `CLIENT_PROTOCOL_VERSION` bump. `classify()` forces a major bump ONLY for remove/rename/retype (`client-protocol/.../version_guard_test.rs:545/565/584`). reserved→live byte-identical doc flip, added optional field, and added `#[non_exhaustive]` variant are all `Compatible` (tests at lines 603/624).
- **No silent tool shadowing.** All four coordinator tool names collide byte-for-byte with already-registered builtins. `register_builtin` is push-no-dedup (`tool-api/src/registry.rs:43`); `find_by_name` returns the FIRST match (`registry.rs:73`). Therefore: **drop** the coordinator `SendMessage`/`SyntheticOutput` tools (the builtins already satisfy them once the mailbox router is wired); register coordinator `TeamCreate`/`TeamDelete` **IN PLACE OF** `tool_team`'s two, decided at **BUILD time**.
- **One file per task ideally**; each task ends with an explicit `cargo test -p <crate>` verify command and a green bar before the next dependent task starts.

---

## 3. Task Table

| ID | Title | Crate(s) | Phase | depends_on |
|----|-------|----------|-------|------------|
| T01 | `TaskRegistry::spawn()` real handler dispatch | tasks | 1 | — |
| T02 | `TeamRegistry` status mutators + by-task_id index + team_name | coordinator | 1 | — |
| T03 | `coordinator_internal_tools` signature change; drop SendMessage/SyntheticOutput | coordinator | 1 | T02 |
| T04 | `TeamSpawnSeam` trait + `TaskRegistry` impl (decouple coordinator↔tasks) | traits, tasks | 1 | T01 |
| T05 | `TeamCreateTool.call()`: mode gate + real spawn + task_id write-back + team_name | coordinator | 1 | T02, T03, T04 |
| T06 | `TeamDeleteTool.call()`: mode gate + kill backing teammate | coordinator | 1 | T02, T03, T04 |
| T07 | `CoordinatorStatusSink` (TaskStatusSink → WorkerStatus) unit | coordinator | 1 | T02 |
| T08 | `OutputStream::emit_coordinator_status` default no-op (trait) | traits | 1 | — |
| T09 | `AdapterOutputStream::emit_coordinator_status` override + parity test | client-adapter | 1 | T08 |
| T10 | Flip `CoordinatorStatus` reserved→live doc; re-bless `feed_status.json` | client-protocol | 1 | — |
| T11 | `DesktopConfig.session_started_as_coordinator`; `DesktopRuntime` fields | engine-desktop | 1 | — |
| T12 | `register_desktop_tools` mode-exclusive coordinator tool selection | engine-desktop | 1 | T03, T11 |
| T13 | `build()` wiring: registry, mode, mailbox_router, direct teammate handler, sink | engine-desktop | 1 | T01, T02, T05, T06, T07, T11, T12 |
| T14 | Separate teammate `StateMachinePool` + pool-starvation regression | engine-desktop, agent | 1 | T13 |
| T15 | ANTI-HOLLOW end-to-end engine integration (active_workers>0 → ClientEvent) | engine-desktop | 1 | T09, T13, T14 |
| T16 | Workspace regression + version-guard/contract snapshot gate | workspace | 1 | T10, T15 |
| T17 | `platform_api::team_registry::TeamRegistryHandle` + coordinator impl | traits, coordinator | 2 | T02 |
| T18 | Roster DTO + `ListingKindDto::Coordinator` + `lower_worker_agent` + version-guard index | client-protocol, client-adapter | 2 | T10 |
| T19 | `EngineCommandRouter`: optional coordinator handle + `spawn_coordinator_poll` + emit_listing arm | bridge-server | 2 | T17, T18 |
| T20 | TUI `WorkersRefreshed` live feed over `TeamRegistryHandle` + feed-slot resolution | tui | 2 | T17, T18 |
| T21 | Optional `StatusSnapshot.active_workers` surfacing on `/status` | traits, client-protocol, client-adapter | 2 (opt) | T10 |

**Total tasks: 21** (PHASE 1: T01–T16; PHASE 2: T17–T21).

---

## 4. Per-Task Detail

> Convention: every task is **tests-first**. Write the failing test, run the verify command, see RED, then implement to GREEN. Each task's verify command is the gate.

---

### T01 — `TaskRegistry::spawn()` real handler dispatch *(THE load-bearing addition)*

- **Goal:** Add a real dispatch method that looks up the per-type handler and actually runs it, so a registered `InProcessTeammate`/`LocalAgent` handler becomes runnable in production. `create()` (`tasks/src/registry.rs:61-109`) only inserts `Pending` and ignores `_input`; it never calls `self.handlers.get(task_type)` nor `handler.spawn()`. Every existing `handler.spawn()` caller is `#[cfg(test)]`.
- **Files:** `tasks/src/registry.rs` (add method; keep `create()` for the placeholder path), possibly `tasks/src/lib.rs` (re-export `TaskSpawnInput` if not already public).
- **Approach:**
  - Add `pub async fn spawn(&self, task_type: TaskType, input: TaskSpawnInput, description: String) -> Result<String, TaskError>` that:
    1. allocates the spool via `self.output_manager.allocate(&id)` (mirror `create()`),
    2. looks up `self.handlers.get(&task_type)` → `TaskError::NotFound`/`Unsupported` if absent,
    3. inserts the typed `TaskState` for the variant (use the real input fields, not the placeholder `AgentId::nil()`),
    4. calls `handler.spawn(input, TaskContext{fs, runtime, ...})`,
    5. records the returned `BackgroundTaskHandle`/`TaskHandle` in `self.handles`,
    6. returns the **handler-generated** `task_id` (`TaskHandle.task_id` — `in_process_teammate.rs:362`), distinct from `create()`'s `generate_task_id`.
  - Keep `create()` unchanged (placeholder/tool-dispatch path).
- **Tests-first** (`tasks/src/registry.rs` `#[cfg(test)]` or `tasks/tests/`):
  - `spawn_invokes_handler_and_returns_handler_task_id`: register a fake `Task` handler that records that `spawn()` ran and returns a known `task_id`; assert `TaskRegistry::spawn(...)` returns THAT id, that the handler ran exactly once, and that the id differs from a `create()` placeholder id for the same `task_type`.
  - `spawn_unknown_type_errors`: spawn with no registered handler → `TaskError`.
  - `spawn_records_handle_for_kill`: after `spawn`, `kill(task_id)` finds and cancels the recorded handle.
- **Verify:** `cargo test -p tasks spawn_`
- **depends_on:** —

---

### T02 — `TeamRegistry` status mutators + by-task_id index + team_name

- **Goal:** Add the genuinely-new mutation/lookup surface so `active_workers` can be computed and change over time. (`task_id`/`parent_id`/`last_active_at` fields ALREADY exist on `WorkerAgent` — `team_registry.rs:29/25/33`; do NOT re-add them.)
- **Files:** `coordinator/src/team_registry.rs`.
- **Approach (all additive):**
  - `pub async fn update_status(&self, agent_id: &AgentId, status: WorkerStatus)` — set status + touch `last_active_at = SystemTime::now()`.
  - `pub async fn set_task_id(&self, agent_id: &AgentId, task_id: String)` — write back the handler-generated id.
  - `pub async fn find_by_task_id(&self, task_id: &str) -> Option<WorkerAgent>` + a by-task_id index (or a linear scan over `workers` — acceptable at this scale; document the choice).
  - `pub async fn set_team_name(&self, name: Option<String>)` backed by a new `team_name: RwLock<Option<String>>` field on the struct (the reserved DTO's `team: Option<String>` source; we do NOT multiplex concurrent teams this pass).
  - `pub async fn team_name(&self) -> Option<String>` reader.
  - Helper: `pub async fn active_worker_count(&self) -> u32` = count of workers whose status is non-terminal (`Idle` | `Working{..}` | `AwaitingMessage`).
- **Tests-first** (`coordinator/src/team_registry.rs` tests):
  - `spawn_then_update_status_transitions`: spawn → `Idle`; `update_status(Working)` → `list()` reflects `Working` and `last_active_at` advanced.
  - `update_status_failed_and_killed`: terminal statuses reflected.
  - `find_by_task_id_roundtrip`: `set_task_id` then `find_by_task_id` returns the worker.
  - `active_worker_count_excludes_terminal`: mix of `Working`/`Failed`/`Killed` → count is the non-terminal count only.
  - `team_name_set_get`.
- **Verify:** `cargo test -p coordinator team_registry`
- **depends_on:** —

---

### T03 — `coordinator_internal_tools` signature change; drop SendMessage/SyntheticOutput

- **Goal:** Change the factory so it returns ONLY `TeamCreate` + `TeamDelete` (the two carrying net-new behavior) and threads the new dependencies. Today `internal_tools.rs:22` takes only `Arc<TeamRegistry>` and returns all four.
- **Files:** `coordinator/src/internal_tools.rs`.
- **Approach:**
  - New signature: `pub fn coordinator_internal_tools(team: Arc<TeamRegistry>, mode: Arc<CoordinatorMode>, spawn_seam: Arc<dyn platform_api::team_spawn::TeamSpawnSeam>) -> Vec<Arc<dyn Tool>>` (the `TeamSpawnSeam` trait lands in T04).
  - Return `vec![TeamCreateTool::new(team.clone(), mode.clone(), spawn_seam.clone()), TeamDeleteTool::new(team, mode, spawn_seam)]`.
  - **Drop** `SendMessageTool`/`SyntheticOutputTool` from the returned vec (satisfied by the in-tree builtins once the mailbox router is wired — see T13). Leave the tool source files in place (not deleted) but no longer assembled here; update the module doc comment accordingly.
- **Tests-first** (`coordinator/src/internal_tools.rs` tests):
  - `factory_returns_exactly_team_create_and_delete`: assert returned vec has length 2 and the two `name()`s are `"TeamCreate"`, `"TeamDelete"`.
- **Verify:** `cargo test -p coordinator internal_tools`
- **depends_on:** T02 (constructors take the new deps)

---

### T04 — `TeamSpawnSeam` trait + `TaskRegistry` impl

- **Goal:** Give `TeamCreateTool` a typed spawn seam to start a real `InProcessTeammate` without a coordinator→tasks dependency cycle (the existing `TaskRegistryHandle::create` cannot carry `agent_id`/`name` — it builds a placeholder with `AgentId::nil()` + empty name at `tasks/src/handle.rs:118-121`). Mirror the `TaskRegistryHandle` decoupling pattern.
- **Files:** `platform-api/src/team_spawn.rs` (NEW narrow trait), `platform-api/src/lib.rs` (module export), `tasks/src/registry.rs` or `tasks/src/handle.rs` (impl on `TaskRegistry`).
- **Approach:**
  - Define `#[async_trait] pub trait TeamSpawnSeam: Send + Sync { async fn spawn_teammate(&self, agent_id: protocol::AgentId, name: String, description: String) -> Result<String, TeamSpawnError>; async fn kill(&self, task_id: &str) -> Result<(), TeamSpawnError>; }` returning the handler-generated `task_id`.
  - Impl `TeamSpawnSeam for tasks::registry::TaskRegistry` by calling the new `TaskRegistry::spawn(TaskType::InProcessTeammate, TaskSpawnInput::InProcessTeammate{ agent_id, name }, description)` (T01) and `kill`.
  - Keep the trait in `traits` so `coordinator` (which already depends on `traits` for `Tool`/`MailboxRouterHandle`) and `tasks` both reference it without a cycle.
- **Tests-first:**
  - `traits` compile-only doc test that the trait is object-safe (`Arc<dyn TeamSpawnSeam>`).
  - `tasks/tests` (or registry tests): `team_spawn_seam_spawns_real_teammate`: with an `InProcessTeammate` handler registered, `TeamSpawnSeam::spawn_teammate` returns a non-empty handler-generated id and the handler ran (reuse the T01 fake/recording handler).
- **Verify:** `cargo test -p tasks team_spawn` && `cargo test -p platform-api team_spawn`
- **depends_on:** T01

---

### T05 — `TeamCreateTool.call()`: mode gate + real spawn + task_id write-back + team_name

- **Goal:** Replace the metadata-only behavior (today `team.spawn_worker(agent_type, team_name, task_id="")` with an EMPTY task_id at `tool_team_create.rs:208`) with a real teammate spawn whose handler-generated id is reconciled back onto the `WorkerAgent`.
- **Files:** `coordinator/src/tool_team_create.rs`.
- **Approach — `call()` must, in order:**
  1. **Mode early-return:** if `!self.mode.is_enabled()` return `ToolError::InvalidInput("coordinator mode not active")` (defense-in-depth; lets a future `/coordinator exit()` neutralize the tool without a registry rebuild). NOTE: `TeamCreateTool::is_enabled` today consults the static feature flag `agent_swarms_enabled` (`tool_team_create.rs:92-99`) — the mode early-return is NET-NEW code in `call()`, not a flag flip.
  2. `let agent_id = self.team.spawn_worker(agent_type, team_name.clone(), String::new()).await?;` (mint `WorkerAgent` + `AgentId` + mailbox).
  3. `let task_id = self.spawn_seam.spawn_teammate(agent_id, team_name.clone(), description).await?;` (start the real `InProcessTeammate`, returns the handler-generated id).
  4. `self.team.set_task_id(&agent_id, task_id.clone()).await;` (reconcile the two id spaces by keying the worker↔task link on the handler-returned id).
  5. `self.team.set_team_name(Some(team_name)).await;`
  6. Return the worker `agent_id` + the real `task_id` in the tool result (replace the `task_id == agent_id` placeholder return).
- **Tests-first** (`coordinator/src/tool_team_create.rs` tests, with a fake `TeamSpawnSeam` returning a known id):
  - `call_when_mode_disabled_errors`: mode off → `InvalidInput`, no worker spawned.
  - `call_spawns_worker_and_writes_back_task_id`: mode on → exactly one worker in `list()`; its `task_id` equals the seam-returned id (NOT empty, NOT the agent_id); `team_name` set.
  - `call_invokes_spawn_seam_once`.
- **Verify:** `cargo test -p coordinator tool_team_create`
- **depends_on:** T02, T03, T04

---

### T06 — `TeamDeleteTool.call()`: mode gate + kill backing teammate

- **Goal:** On delete, also kill the running teammate task (today it only probes `list()` then `delete_worker()`).
- **Files:** `coordinator/src/tool_team_delete.rs`.
- **Approach — `call()` must:**
  1. Mode early-return (same as T05).
  2. Look up the `WorkerAgent` by `agent_id`, read its `task_id`.
  3. If `task_id` non-empty: `self.spawn_seam.kill(&task_id).await` (best-effort; map error to a tool warning, do not abort the delete).
  4. `self.team.delete_worker(&agent_id).await` (removes worker + unregisters mailbox).
- **Tests-first** (`coordinator/src/tool_team_delete.rs` tests):
  - `delete_when_mode_disabled_errors`.
  - `delete_kills_backing_task_then_removes_worker`: with a fake seam recording `kill(task_id)`, after spawn+delete the seam saw the right id and `list()` is empty.
- **Verify:** `cargo test -p coordinator tool_team_delete`
- **depends_on:** T02, T03, T04

---

### T07 — `CoordinatorStatusSink` (TaskStatusSink → WorkerStatus)

- **Goal:** Bridge the teammate's `TaskStatusSink` transitions onto `TeamRegistry::update_status`, scoped to the transitions the sink can actually drive. This is the status-flow seam injected via `.with_status_sink` in T13.
- **Files:** `coordinator/src/status_sink.rs` (NEW), `coordinator/src/lib.rs` (export).
- **Approach:**
  - `pub struct CoordinatorStatusSink { team: Arc<TeamRegistry>, output: Arc<dyn OutputStream> /* optional emit hook, see T13 */ }` implementing `tasks::handlers::TaskStatusSink`.
  - `set_status(task_id, TaskStatus)`:
    - `Running` → `find_by_task_id(task_id)` → `update_status(agent_id, WorkerStatus::Working{ activity: "running".into() })` (fixed string — `WorkerStatus` derives `PartialEq` on `activity` and the sink has no per-turn text; richer text is a follow-up).
    - `Failed{..}` → `WorkerStatus::Failed{ error }`.
    - `Killed` → `WorkerStatus::Killed`.
    - `Completed` → **no transition** (documents the lossy surface: `terminal_status` returns `None` for `Completed` by design — a persistent teammate emits `Completed` per turn-set yet keeps running — `in_process_teammate.rs:245-253`).
  - After each transition that changes state, fire `output.emit_coordinator_status(team.active_worker_count(), team.team_name().as_deref())` (the PUSH wiring; uses T08/T09).
- **Tests-first** (`coordinator/src/status_sink.rs` tests; reuse the `in_process_teammate.rs:597` `RecordingSink` style for assertions where helpful):
  - `running_maps_to_working`: spawn worker, `set_status(task_id, Running)` → `list()` shows `Working{activity:"running"}`.
  - `completed_does_not_transition`: after `Running`, `set_status(task_id, Completed)` leaves status `Working` (documents the gap).
  - `failed_and_killed_map_through`.
  - `unknown_task_id_is_noop` (no panic).
  - `emit_fires_with_active_count`: inject a spy `OutputStream`; assert it received the active-worker count after a `Running` transition.
- **Verify:** `cargo test -p coordinator status_sink`
- **depends_on:** T02 (and references T08's trait method, which is default-no-op so compiles even before T09)

---

### T08 — `OutputStream::emit_coordinator_status` default no-op (trait)

- **Goal:** Add the additive PUSH hook to the orchestrator-facing trait, exactly alongside the verified default-no-op `emit_thinking` (`platform-api/src/orchestrator.rs:550`) / `emit_usage` (`:565`).
- **Files:** `platform-api/src/orchestrator.rs`.
- **Approach:** `async fn emit_coordinator_status(&self, _active_workers: u32, _team: Option<&str>) {}` (default no-op so every existing `OutputStream` impl — TUI/CLI/Mock — keeps compiling unchanged).
- **Tests-first:** compile-gate is the test; add a tiny unit asserting the default does nothing for a unit struct impl (no panic, returns).
- **Verify:** `cargo test -p platform-api orchestrator`
- **depends_on:** —

---

### T09 — `AdapterOutputStream::emit_coordinator_status` override + parity test

- **Goal:** Override the new method in the adapter to push `ClientEvent::CoordinatorStatus`, mirroring `emit_thinking` at `client-adapter/src/output_stream.rs:184`.
- **Files:** `client-adapter/src/output_stream.rs`.
- **Approach:**
  ```rust
  async fn emit_coordinator_status(&self, active_workers: u32, team: Option<&str>) {
      self.sink.emit(ClientEvent::CoordinatorStatus {
          active_workers,
          team: team.map(str::to_string),
      }).await;
  }
  ```
  Downstream is already universal (the single `Arc<dyn ClientEventSink>` fans out to bridge WS + mobile UniFFI) — NO transport changes.
- **Tests-first** (`client-adapter` tests; mirror `emit_thinking_produces_thinking_delta` at `output_stream.rs:428` with `MockSink`):
  - `emit_coordinator_status_produces_one_event`: drive `emit_coordinator_status(3, Some("alpha"))` → exactly one `ClientEvent::CoordinatorStatus{active_workers:3, team:Some("alpha")}`.
  - `emit_coordinator_status_none_team`: `team:None` round-trips.
- **Verify:** `cargo test -p client-adapter output_stream`
- **depends_on:** T08

---

### T10 — Flip `CoordinatorStatus` reserved→live doc; re-bless `feed_status.json`

- **Goal:** Lift the feed-deferred invariant on the DTO (no field change → no wire break) and re-bless the feed-status snapshot. The DTO `{active_workers:u32, team:Option<String>}` at `client-protocol/src/events.rs:280` stays byte-identical.
- **Files:** `client-protocol/src/events.rs` (doc only), `client-protocol/snapshots/feed_status.json` (re-bless).
- **Approach:**
  - Replace the doc invariant at `events.rs:275-279` ("MUST NOT be wired to a live source … always 0") with a LIVE-FED note matching the `ThinkingDelta` note at `events.rs:288`.
  - Re-bless `feed_status.json` entry at line ~48-49: `status: "reserved"` → `"live"` (NOT gated by version-major logic). Run with `BLESS=1`.
- **Tests-first:**
  - Existing round-trip/snapshot tests for `CoordinatorStatus` must still pass (no field change).
  - Add/extend an assertion that `feed_status.json` lists `CoordinatorStatus` as `"live"`.
- **Verify:** `BLESS=1 cargo test -p client-protocol feed_status` then `cargo test -p client-protocol feed_status` (clean, no BLESS).
- **depends_on:** —

---

### T11 — `DesktopConfig.session_started_as_coordinator`; `DesktopRuntime` fields

- **Goal:** Add the build-time activation flag and surface the registry + mode on the runtime so the status feed and (PHASE 2) command router can read them.
- **Files:** `apps/engine-desktop/src/lib.rs` (`DesktopConfig` ~`:180`, `DesktopRuntime` ~`:267`), `apps/engine-desktop/Cargo.toml` (add `coordinator` dep — today only `test-harness` depends on it).
- **Approach:**
  - `DesktopConfig`: add `pub session_started_as_coordinator: bool` with `Default = false` (additive to the field set). Ensure `Default`/builder paths set `false`.
  - `DesktopRuntime`: add `pub coordinator: Arc<coordinator::TeamRegistry>` and `pub coordinator_mode: Arc<coordinator::CoordinatorMode>` (alongside `task_registry` at `:267`).
  - Add `coordinator` to `apps/engine-desktop/Cargo.toml` `[dependencies]`. **Do NOT** add it to `engine-mobile`.
  - Optional: thread the flag from `session/src/metadata.rs` (already carries a coordinator-mode field) when constructing config — note as a follow-up wire, not required for T15.
- **Tests-first** (`engine-desktop` tests):
  - `default_config_is_not_coordinator`: `DesktopConfig::default().session_started_as_coordinator == false`.
  - `runtime_exposes_coordinator_handles`: after a minimal `build()` (default config) `runtime.coordinator` and `runtime.coordinator_mode` exist and `coordinator_mode.is_enabled() == false`.
- **Verify:** `cargo test -p engine-desktop config` (or the crate's package name — confirm with `cargo metadata`)
- **depends_on:** —

---

### T12 — `register_desktop_tools` mode-exclusive coordinator tool selection

- **Goal:** Register coordinator `TeamCreate`/`TeamDelete` IN PLACE OF `tool_team`'s two when coordinator-capable, at BUILD time, to avoid the silent-shadow collision. `tool_team::register_all` is unconditional today at `lib.rs:108`; the registry is built-once-and-moved, so mode-exclusivity MUST be decided at build time.
- **Files:** `apps/engine-desktop/src/lib.rs` (`register_desktop_tools` / `desktop_tool_registry`, ~`:85-112`).
- **Approach:**
  - Change `register_desktop_tools` / `desktop_tool_registry` to take `coordinator: Option<Arc<TeamRegistry>>`, `spawn_seam: Option<Arc<dyn TeamSpawnSeam>>`, `mode: Arc<CoordinatorMode>` (or a single `Option<CoordinatorWiring>` struct bundling the three).
  - When `coordinator.is_some()` (coordinator-capable session): **suppress** `tool_team::register_all` for the `TeamCreate`/`TeamDelete` names (call a variant that registers tool_team's *other* tools only, or skip its two and push the coordinator pair), then push `coordinator_internal_tools(team, mode, spawn_seam)` (T03).
  - When `None` (default): unchanged — `tool_team::register_all` as today; coordinator tools absent.
  - Verify there is exactly ONE `TeamCreate` and ONE `TeamDelete` in `all_names()` in BOTH modes (no duplicate names emitted into the system prompt).
- **Tests-first** (`engine-desktop` tests over the assembled registry):
  - `default_mode_registers_tool_team_create`: with `coordinator: None`, `all_names()` contains exactly one `"TeamCreate"` and it is `tool_team`'s (probe by a behavior marker or `is_enabled` semantics).
  - `coordinator_mode_registers_coordinator_create`: with `coordinator: Some`, `all_names()` contains exactly one `"TeamCreate"` and it is the coordinator's; no duplicate names anywhere in `all_names()`.
- **Verify:** `cargo test -p engine-desktop tool_registry`
- **depends_on:** T03, T11

---

### T13 — `build()` wiring: registry, mode, mailbox_router, direct teammate handler, status sink

- **Goal:** The composition-root change. Construct the coordinator subsystem and wire it.
- **Files:** `apps/engine-desktop/src/lib.rs` (`build()`).
- **Approach (in `build()`):**
  1. **Mint coordinator id** (stable for the session): `let coordinator_id = protocol::AgentId::new();` (lowest-coupling additive choice vs reusing the session id). Store on `DesktopRuntime` if useful for SendMessage from-id (see T15 / risk).
  2. **Construct subsystem:** `let team = Arc::new(coordinator::TeamRegistry::new(coordinator_id)); let mode = Arc::new(coordinator::CoordinatorMode::default());`
  3. **Direct teammate handler with sink** — between `task_registry_inner` being built (`:590`) and Arc-wrapped (`:608`), because `register_handler` takes `&mut self`. Build the handler DIRECTLY (NOT `register_agent_handlers`, per the `registry.rs:227-229` escape-hatch doc) so the status sink is attached:
     ```rust
     let coordinator_sink = Arc::new(coordinator::CoordinatorStatusSink::new(team.clone(), output.clone()));
     let teammate_pool = Arc::new(StateMachinePool::new(PosixRuntime::new(), TEAMMATE_POOL_CAP)); // SEPARATE pool — see T14
     let teammate_handler = InProcessTeammateHandler::new(teammate_pool, task_output_manager.clone(), subagent_api.clone())
         .with_tool_invoker(parent_invoker)
         .with_status_sink(coordinator_sink.clone());
     task_registry_inner.register_handler(TaskType::InProcessTeammate, Arc::new(teammate_handler));
     ```
     (`subagent_api` exists; `budget_enforcer`/`subagent_spawner` exist at `:445/:455`.) **Definition resolver:** rely on the handler default `DefaultTeammateDefinition` (permissive) — state this explicitly; a catalog-backed resolver returning `None` for the team-lead name would silently fail every spawn at `in_process_teammate.rs:293`. Do NOT attach `.with_definitions` this pass.
  4. **Spawn seam:** after `task_registry = Arc::new(task_registry_inner)` (`:608`), build `let spawn_seam: Arc<dyn TeamSpawnSeam> = task_registry.clone();` (T04 impl).
  5. **Mailbox router:** set `mailbox_router: Some(team.mailbox_router.clone() as Arc<dyn platform_api::mailbox::MailboxRouterHandle>)` in `BuiltinToolContext` (was `None` at `:636`) — `MailboxRouter` already impls the trait (`coordinator/src/handle.rs:30`), so the builtin `SendMessage` (`tools/ui/src/send_message.rs`) and coordinator routing share the SAME mailboxes.
  6. **Tool registry:** call `desktop_tool_registry(tool_ctx, coordinator_wiring)` (T12) passing `Some(team)`/`Some(spawn_seam)`/`mode` when `config.session_started_as_coordinator`, else `None`/`None`/`mode`.
  7. **Activate mode at build:** if `config.session_started_as_coordinator` → `mode.enter()` and set `CoordinatorMode.session_started_as_coordinator = true` (note the field is `pub` and constructed via `default()` then mutated, or via a constructor — confirm `mode.rs:11-15`).
  8. **Surface on runtime:** set `DesktopRuntime { coordinator: team, coordinator_mode: mode, .. }`.
  9. (Optional, lower priority) `/coordinator` (alias `/cowork`) slash command in `desktop_command_registry` (`:235/:660`) calling `mode.enter()/exit()` returning `ModeSwitchResult` — flips the call()-gate live but CANNOT change the registered tool set mid-session. Note only; not required for T15.
- **Tests-first:** covered by T15 (the anti-hollow integration) + a focused unit:
  - `build_coordinator_session_enters_mode`: `build()` with `session_started_as_coordinator=true` → `runtime.coordinator_mode.is_enabled()`; `build()` default → not enabled.
  - `mailbox_router_is_wired_when_coordinator`: assert the builtin SendMessage path has a router (behavioral; or assert ctx field non-None via a test seam).
- **Verify:** `cargo test -p engine-desktop build_coordinator`
- **depends_on:** T01, T02, T05, T06, T07, T11, T12

---

### T14 — Separate teammate `StateMachinePool` + pool-starvation regression

- **Goal:** Avoid deadlocking `AgentTool` subagent spawning. `subagent_pool` (`lib.rs:444`) is moved into `PoolSubagentSpawner` (`:446`) with `max_concurrent=4`; persistent teammates park on `wait_for_message` and NEVER free their slot, so sharing one pool risks starvation.
- **Files:** `apps/engine-desktop/src/lib.rs` (the `teammate_pool` introduced in T13), a regression test in `engine-desktop` (or `agent` if the harness lives there).
- **Approach:** Use a SEPARATE `StateMachinePool` with its own cap (`TEAMMATE_POOL_CAP`, e.g. 4) for teammates, distinct from `subagent_pool`. Document the cap choice.
- **Tests-first:**
  - `parked_teammates_do_not_starve_agent_tool`: with `N == TEAMMATE_POOL_CAP` parked teammates occupying the teammate pool, assert an `AgentTool` subagent still spawns through `subagent_pool` (proves the separate-pool decision). Use the existing pool/subagent harness; scripted `SubagentApiClient`.
- **Verify:** `cargo test -p engine-desktop pool_starvation`
- **depends_on:** T13

---

### T15 — ANTI-HOLLOW end-to-end engine integration *(the key real-run gate)*

- **Goal:** Prove `active_workers > 0` flows from a real `TeamCreate` call to a `ClientEvent::CoordinatorStatus`, and that the test FAILS against the hollow `create()`-only implementation.
- **Files:** `apps/engine-desktop/tests/coordinator_activation.rs` (NEW), reuse `test-harness` scaffolding where available.
- **Approach / assertions (coordinator session, `session_started_as_coordinator=true`):** invoke the coordinator `TeamCreate` tool and assert:
  - (a) a `WorkerAgent` exists in `runtime.coordinator.list()`;
  - (b) a REAL `InProcessTeammate` ran — assert the **handler actually ran** (e.g. a scripted `SubagentApiClient` round-trip count `> 0`, or a spool line written), NOT merely that a `Pending` row exists;
  - (c) `WorkerAgent.task_id == the handler-returned id` (non-empty, not the agent_id);
  - (d) the worker transitioned `Idle → Working` via the sink;
  - (e) an `OutputStream` spy (or `MockSink` behind `AdapterOutputStream`) received `ClientEvent::CoordinatorStatus{ active_workers > 0 }`.
  - **This test MUST fail against the hollow `create()`-only implementation** (i.e. it would fail at (b)/(d)/(e) if `TaskRegistry::spawn` were not wired). Optionally include an inverted assertion documenting that.
- **Default-session no-regression (same file or sibling):** with `session_started_as_coordinator=false`:
  - `tool_team`'s `TeamCreate` is the registered one;
  - coordinator `TeamCreate`/`TeamDelete` are NOT in `all_names()`;
  - no teammate task spawned;
  - the advertised tool list is **byte-identical** to a pre-change baseline (snapshot the sorted `all_names()`).
- **Verify:** `cargo test -p engine-desktop coordinator_activation`
- **depends_on:** T09, T13, T14

---

### T16 — Workspace regression + version-guard/contract snapshot gate

- **Goal:** Prove additive-only and no `CLIENT_PROTOCOL_VERSION` bump across the whole workspace; confirm AgentTool/subagent merged tests untouched.
- **Files:** none new beyond snapshot re-bless from T10 (and T18 in PHASE 2).
- **Approach:**
  - Run the version-guard tests: confirm `classify()` reports `Compatible` (reserved→live byte-identical flip is not a contract change requiring a major bump — verified tests at `version_guard_test.rs:603/624`).
  - Confirm the `feed_status.json` re-bless from T10 is clean.
  - Full workspace build+test. Known FSEvents fs-watch posix flake (per MEMORY) is unrelated — re-run those tests in isolation if they flap; do not treat as a code failure.
- **Tests-first:** these ARE the gate tests (already exist); this task asserts they pass post-change.
- **Verify:** `cargo test -p client-protocol version_guard` && `cargo test --workspace`
- **depends_on:** T10, T15

---

## PHASE 2 — Per-worker roster chrome (deferred; GOAL already met by PHASE 1)

> The PUSH scalar alone (T01–T16) satisfies "active_workers>0 fed live to all clients, reserved→live". PHASE 2 is required ONLY for per-worker TUI `WorkerRow` chrome.

### T17 — `platform_api::team_registry::TeamRegistryHandle` + coordinator impl

- **Goal:** Narrow trait so the bridge/TUI PULL path reads workers without depending on the concrete `coordinator` crate (mirrors `platform_api::task_registry::TaskRegistryHandle`).
- **Files:** `platform-api/src/team_registry.rs` (NEW), `platform-api/src/lib.rs` (export), `coordinator/src/handle.rs` (impl for `TeamRegistry`).
- **Approach:** `#[async_trait] pub trait TeamRegistryHandle: Send + Sync { async fn list_workers(&self) -> Vec<WorkerInfo>; async fn team_name(&self) -> Option<String>; }` with a `WorkerInfo` POD `{agent_id, agent_type, name, status}` in `traits`. Impl on `coordinator::TeamRegistry` lowering `WorkerAgent`→`WorkerInfo`.
- **Tests-first:** `team_registry_handle_lists_workers` in `coordinator` (spawn 2, assert 2 `WorkerInfo`s with mapped status).
- **Verify:** `cargo test -p coordinator team_registry_handle` && `cargo test -p platform-api team_registry`
- **depends_on:** T02

### T18 — Roster DTO + `ListingKindDto::Coordinator` + `lower_worker_agent` + version-guard index

- **Goal:** Add a distinct per-worker roster DTO (NOT the existing `permission.rs:97` `WorkerInfoDto{name,color,team}`, which is reserved for permission requests and does not match `WorkerRow`) and the listing kind.
- **Files:** `client-protocol/src/events.rs` (or `listings.rs`) NEW `CoordinatorWorkerDto{agent_id, name, agent_type, status}`; `client-protocol/src/commands.rs` add `ListingKindDto::Coordinator` (enum is `#[non_exhaustive]` — compatible per `version_guard_test.rs:624`); `client-adapter/src/lowering.rs` `lower_worker_agent`; `client-protocol/.../version_guard_test.rs` add the new DTO to `current_contract_index()` constructor (else `contract_index_covers_every_dto` at `:726` fails).
- **Approach:** DTO carries `{agent_id, name, agent_type, status}` lowering 1:1 to the TUI `WorkerRow` (`tui/src/multiagent/state.rs:25`). Regenerate contract_index / JSON-schema / round-trip snapshots with `BLESS=1`.
- **Tests-first:**
  - `coordinator_worker_dto_roundtrip` + `listing_kind_coordinator_roundtrip` + `classify_reports_compatible` (no major bump).
  - `lower_worker_agent_matches_worker_row_fixture` (client-adapter parity against a `WorkerRow` fixture).
  - the version-guard contract-index test compiles+passes with the new DTO present.
- **Verify:** `BLESS=1 cargo test -p client-protocol` then `cargo test -p client-protocol` && `cargo test -p client-adapter lower_worker`
- **depends_on:** T10

### T19 — `EngineCommandRouter`: optional coordinator handle + `spawn_coordinator_poll` + emit_listing arm

- **Goal:** PULL roster on the bridge, mirroring the EXISTING task PULL pattern (NOT `OrchestratorHandle` — it holds no `TeamRegistry`; that was the wrong seam in the original design).
- **Files:** `apps/bridge-server/src/router.rs`.
- **Approach:** add optional `coordinator: Option<Arc<dyn platform_api::team_registry::TeamRegistryHandle>>` to `EngineCommandRouter::new` (`:125`); add a `ListingKindDto::Coordinator` arm in `emit_listing` (`:186`/`:235`); add `spawn_coordinator_poll(sink, interval)` mirroring `spawn_task_poll` (`:158`) + `emit_task_rows` (`:259`/`:235`) emitting one roster DTO per worker. Like `spawn_task_poll`, it needs a deliberate connection-lifecycle start point — note it is NOT auto-started in prod today either.
- **Tests-first:** `spawn_coordinator_poll_emits_worker_rows` + `emit_listing_coordinator_arm` (mirror the `spawn_task_poll` test; mock handle returning 2 workers → 2 roster DTOs on the sink).
- **Verify:** `cargo test -p bridge-server coordinator_poll`
- **depends_on:** T17, T18

### T20 — TUI `WorkersRefreshed` live feed over `TeamRegistryHandle` + feed-slot resolution

- **Goal:** Populate `MultiAgentState.workers` live. `PollerFeed` (`tui/src/multiagent/poller.rs:50`) emits ONLY `TasksRefreshed`; there is a single `multiagent_feed` slot.
- **Files:** `tui/src/multiagent/poller.rs` (or a new `workers_poller.rs`), `tui/src/multiagent/` feed composition.
- **Approach:** add a `TeamRegistryHandle`-backed feed mapping `WorkerInfo`→`WorkerRow` emitting `WorkersRefreshed`; resolve the single feed slot via a composite feed that emits BOTH `TasksRefreshed` + `WorkersRefreshed` (or a second feed/pump pair). Define the canonical `WorkerStatus → WorkerRow.status` label mapping fn.
- **Tests-first:** extend `tui/tests/coordinator_chrome.rs` — live feed → `apply_multiagent_event` → `MultiAgentState.workers` populated; assert the status-label mapping and that `WorkersRefreshed` + `TasksRefreshed` coexist in the resolved feed.
- **Verify:** `cargo test -p tui coordinator_chrome`
- **depends_on:** T17, T18

### T21 — Optional `StatusSnapshot.active_workers` surfacing on `/status`

- **Goal:** Surface the count on `/status` (adding-optional-field is compatible — `version_guard_test.rs:603`).
- **Files:** `platform-api/src/orchestrator.rs` (`StatusSnapshot`), `client-protocol/src/listings.rs` (`StatusSnapshotDto`), `client-adapter/src/lowering.rs` (`lower_status_snapshot`).
- **Approach:** append optional `active_workers: u32` (mirrors the optional `status_line` append); lower it through; re-bless snapshots with `BLESS=1`.
- **Tests-first:** `status_snapshot_carries_active_workers_roundtrip` + classify compatible.
- **Verify:** `BLESS=1 cargo test -p client-protocol status_snapshot` then `cargo test -p client-protocol status_snapshot`
- **depends_on:** T10

---

## 5. Acceptance Criteria (definition of done — PHASE 1)

1. With `session_started_as_coordinator=true`, calling `TeamCreate` spawns a REAL `InProcessTeammate` (handler ran), reconciles the handler-returned `task_id` onto the `WorkerAgent`, transitions it `Idle→Working`, and emits `ClientEvent::CoordinatorStatus{active_workers>0}` to the client sink (T15 green, and FAILS against the hollow impl).
2. Default sessions are byte-identical: tool list unchanged, no teammate spawned, mode off (T15 no-regression assertions green).
3. No `CLIENT_PROTOCOL_VERSION` bump; `classify()` reports `Compatible`; `feed_status.json` shows `CoordinatorStatus: "live"` (T10, T16 green).
4. Separate teammate pool: parked teammates do not starve `AgentTool` (T14 green).
5. Full workspace tests pass (T16), modulo the unrelated FSEvents posix fs-watch flake.

**Explicit out-of-scope (acceptance):** mobile coordinator wiring; richer `Idle`/`AwaitingMessage`/per-turn-`Completed` transitions; PHASE-2 roster chrome is optional follow-up.
