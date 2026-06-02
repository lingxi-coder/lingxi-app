# M10 Native-Apps Foundation — Implementation Plan (TDD)

> **Status**: PLAN v1
> **Date**: 2026-06-02
> **Spec**: `docs/superpowers/specs/2026-06-02-m10-native-apps-design.md`
> **Scope**: M10-F1 (`client-protocol` + `client-adapter`), M10-F2 (`bridge-server` walking skeleton), M10-F3 (UniFFI client surface). **App UI work (A1/A2/A3) is OUT of scope** — this plan only marks where each transport plugs into the existing `clients/electron` and `clients/ios` shells.
> **Workspace**: `lingxi-code/` flat crates at root (`protocol`, `traits`, `permission`, `orchestrator`, `bridge`, `session`, `tui`, `apps/engine-desktop`, `apps/engine-mobile`, `apps/bridge-server`, `apps/ios-framework`, `apps/android-aar`, `apps/cli`).

---

## 0. Governing decisions (locked before any code)

These come from the FINAL DESIGN and are **not** re-litigated here. They are pinned so the DTO freeze (F1) is correct on the first cut.

1. **Hybrid A+B transport**: one versioned `client-protocol` DTO contract + a `client-adapter`, exposed over TWO transports — bridge-server WebSocket/JSON-RPC (Electron, out-of-process) and UniFFI (iOS/Android, in-process).
2. **Two new engine-tier crates** at the flat workspace root: `client-protocol/` (pure contract) and `client-adapter/` (the only engine→DTO bridge). Additive deps in `apps/bridge-server`, `apps/ios-framework`, `apps/android-aar`, `apps/engine-mobile`.
3. **Dependency-rule reality** (verified against `lingxi-code/scripts/check_deps.py`): there is NO `engine` key in `FORBIDDEN` (lines 30-35); `API_CRATES` is literally `{tool-api, skill-api, command-api}` (line 21) and does NOT bind `client-protocol`/`client-adapter`. The only rules binding a root-level engine crate are: (a) no edge to an `apps/*`/`examples/*` leaf (line 99); (b) API-crate purity (irrelevant here). **Net: keeping `client-protocol` minimal is a design preference, not a gate. The ONE rule to assert in CI is: no `client-protocol`/`client-adapter` → `apps/*` edge.** `deny.toml` is supply-chain only — no change needed except the new uniffi entry in F3.
4. **Tool payloads are JSON Strings on the wire** (`input_json`/`result_json`), because `serde_json::Value` is not UniFFI-representable. Same for `effective_json`/`provenance_json`. Locked in F1.
5. **Session lifecycle**: one transport connection owns ONE engine host that SWAPS its inner `ConversationOrchestrator` in place on New/Resume. `session_id` is a CONNECTION ATTRIBUTE (carried in `SessionStarted`/`SessionResumed`), NOT a per-command param. Adapter sinks (OutputStream/PermissionGate/listener) are connection-scoped and survive the swap. **Locked in F1 before DTO freeze.**
6. **Permission**: the orchestrator binds `Arc<dyn PermissionGate>` and calls `check(name, &Value) -> PermissionDecision` (verified `tui/src/permission_bridge.rs:64-117`). The adapter implements `traits::PermissionGate` ONLY (no `PromptingGate`). `check()` can source ONLY `ToolUseConfirm`; `ExitPlanMode`/`BypassPermissionsMode` are DTO-reserved, feed-deferred. `default_allow` is derived from `permission::tool_default(name) -> PromptDefault`.
7. **ThinkingDelta + UsageUpdate** have NO live engine source today and CANNOT be tapped without an engine change (`pump_stream` exposes only `output: Arc<dyn OutputStream>`). **§1 binary decision, made here: ship both DTOs as `#[non_exhaustive]`-reserved, feed-deferred (round-trip only, no live source) for the foundation, and DROP live thinking/usage from the §5.2 parity claim.** If/when the surgical additive `emit_thinking`/`emit_usage` engine hooks are funded, they light up without a DTO change. (Recorded as a deferred follow-up, NOT foundation work — keeps F1/F2/F3 free of engine-behavior edits per spec §1.)
8. **Image input**: `run_turn_streaming_with_images` takes ONLY `&[PathBuf]` and reads from the engine host FS. **Foundation decision: `ImageRefDto` is uniform inline `{media_type, base64}` on the wire (no transport-leaking enum), and the adapter writes inline bytes to a temp file on the engine host for the path transport; mobile inline image input is DEFERRED in §5.12 (no additive engine entry in the foundation).** The sanctioned additive `run_turn_streaming_with_image_sources` is a funded follow-up, not foundation.
9. **Coordinator/team (§5.6)** is BLOCKED ON ENGINE WIRING (no `TeamRegistry` instance is constructed in any assembled runtime — grep-zero) and is PULLED from the foundation lockstep checklist. DTOs are reserved (`#[non_exhaustive]`) but unfed.
10. **Versioning**: `CLIENT_PROTOCOL_VERSION: &str = "1.0.0"` in `client-protocol`, DISTINCT from `bridge::BRIDGE_PROTOCOL_VERSION` (bumped `0.1.0` → `0.2.0` in F2 for the new event frame). Both exchanged independently in the handshake. The version-guard is a STRUCTURAL DIFF (removed/renamed/retyped ⇒ major bump; new variant/new optional field ⇒ no bump), NOT a naive "file changed ⇒ bump".

---

## 1. Task table

| id | title | depends_on | verifies |
|---|---|---|---|
| **F1-00** | Scaffold `client-protocol` crate (empty, workspace-wired) | — | `cargo build -p client-protocol` and `--features uniffi` both compile |
| **F1-01** | Core enums skeleton + `CLIENT_PROTOCOL_VERSION` + serde conventions | F1-00 | round-trip test for `ClientCommand`/`ClientEvent` empty/`Error` variant |
| **F1-02** | `MessageDto` + `MessageBlockDto` block schema (shared) | F1-01 | multi-block assistant round-trip = TUI scrollback block set |
| **F1-03** | Live-turn event DTOs (`TextDelta`, `ToolUseStarted`, `ToolUseResult`, `MessageComplete`, `TurnStarted`, `TurnEnded`, `CostUpdate`, `CompactionCompleted`, `Error`) | F1-02 | serde round-trip per variant |
| **F1-04** | Permission DTOs (`PermissionRequest`/`PermissionResolved`/`PermissionKindDto`/`PermissionResponseDto`) + reserved variants | F1-01 | round-trip + reserved-variant presence |
| **F1-05** | Listing/screen DTOs (Sessions, Models, Mcp, Hooks, Agents, SlashCommands, Memory, Status, Settings, Auth, Doctor, Tasks) | F1-01 | round-trip per listing DTO |
| **F1-06** | `ClientCommand` full set + session-lifecycle decision (no `session_id` param) | F1-03, F1-04, F1-05 | round-trip per command; compile-assert no command carries `session_id` |
| **F1-07** | `ClientError` (`#[derive(uniffi::Error)]`-ready, flat) | F1-01 | round-trip + uniffi-feature compile |
| **F1-08** | JSON-schema golden snapshots (every variant + MessageDto block set + RenderedMessage feed-status table) | F1-03..F1-07 | `cargo test -p client-protocol snapshot` — goldens match |
| **F1-09** | Structural version-diff guard test | F1-08 | removed/renamed entry ⇒ test forces major bump; addition ⇒ passes |
| **F1-10** | Scaffold `client-adapter` crate + `ClientEventSink` trait | F1-08 | `cargo build -p client-adapter` |
| **F1-11** | Pure `From<engine type>` lowering fns (the parity surface) | F1-10 | per-fn unit tests (lowering rules) |
| **F1-12** | `AdapterOutputStream impl traits::OutputStream` | F1-11 | each `emit_*` → expected `ClientEvent` on the sink |
| **F1-13** | Live-turn wrapper: `MessageComplete` synthesis + `OrchestratorError` → `Error` mapping | F1-12 | each error variant → correct `Error.kind`; `MessageComplete` from `PumpedTurn` |
| **F1-14** | `AdapterPermissionGate impl traits::PermissionGate` (id-keyed, fail-closed) | F1-11 | mirror `permission_bridge.rs` 4 tests + concurrent ids + drop/timeout = Deny |
| **F1-15** | Listing/screen `From` parity tests reusing TUI render fixtures | F1-11 | fixture in → matching DTO out, structural not re-derived |
| **F1-16** | CI dep-rule assertion test (no `client-protocol`/`client-adapter` → `apps/*`) | F1-10 | `scripts/check-deps.sh` green + explicit assertion test |
| **F2-00** | `DesktopConfig` struct (F2 deliverable-zero) | F1-13, F1-14 | type compiles; field set frozen |
| **F2-01** | Lift `build_runtime` → `engine_desktop::build(DesktopConfig) -> DesktopRuntime` | F2-00 | `build()` constructs a runtime deterministically (no Argv/env) |
| **F2-02** | Bump `BRIDGE_PROTOCOL_VERSION` → `0.2.0`; add tagged `Frame` enum to `bridge::wire`; carry `CLIENT_PROTOCOL_VERSION` in `Capabilities` | F1-08 | wire round-trip for `Frame::{Request,Response,Event}` |
| **F2-03** | Generalize `McpEndpoint` with a frame-pump callback (reuse auth/upgrade) | F2-02 | existing `mcp_endpoint_test.rs` still green; pump callback invoked |
| **F2-04** | Dedicated `~/.claude/bridge/<port>.lock` discovery file + token | F2-03 | lockfile write/read/Drop-cleanup; distinct `ideName` |
| **F2-05** | **WALKING SKELETON**: drive one turn end-to-end over WS (CLI test client) | F2-01, F2-03, F2-04, F1-13 | connect → handshake → `SendPrompt` → streamed `TextDelta` frames → `TurnEnded` |
| **F2-06** | Permission round-trip over WS (gate ↔ `ApprovePermission`/`DenyPermission`) | F2-05, F1-14 | a parked `check()` resolves from an inbound command on the WS read task |
| **F2-07** | Version-mismatch refusal tests (CLIENT + BRIDGE, independently) | F2-02, F2-05 | mismatch in either version refuses the connection |
| **F2-08** | Full command/event routing in `bridge-server` (listings, model, tasks-poll) | F2-05 | each `ClientCommand` reaches the engine; each pull reply framed |
| **F3-00** | **GATE-ZERO**: vendor/pin `uniffi` offline + verify MSRV vs `rust-version=1.82` | — | `--offline` build still green with uniffi in `Cargo.lock`; `deny.toml` updated |
| **F3-01** | Turn DTOs into real UniFFI types under `uniffi` feature (`setup_scaffolding!`) | F3-00, F1-08 | `cargo build -p client-protocol --features uniffi` + bindgen generates |
| **F3-02** | `ClientEventListener` callback-interface trait + register on handle | F3-01, F1-12 | host-fake listener receives a translated `ClientEvent` |
| **F3-03** | `engine_mobile::build_mobile(MobileConfig) -> MobileRuntime` (shared submit/listener/host module) | F3-01, F1-13, F1-14 | `engine-mobile` builds an orchestrator deterministically off-device |
| **F3-04** | Grow `MobileEngineHandle` into a real session host (owns runtime + adapter + listener) | F3-03 | handle holds tokio rt + orchestrator + adapter; re-exported by both FFI crates |
| **F3-05** | `submit(command) -> Result<(), ClientError>` async FFI entry point | F3-04, F3-02 | `SendPrompt` spawns + returns promptly; Cancel/Approve/Deny resolve |
| **F3-06** | **WALKING SKELETON**: prove submit() + listener round-trip from a host unit test | F3-05 | host-fake `Platform` shim: `submit(SendPrompt)` → listener `TextDelta`/`TurnEnded` |
| **F3-07** | Async-over-FFI runtime registration (uniffi `tokio` feature → foreign executor) | F3-05 | async export resolves on the handle-owned tokio rt |
| **INT-01** | Final integration + verification (both transports, dep-gate, snapshots) | F2-08, F3-07 | full workspace `cargo test` + `check-deps.sh` + snapshot freeze + TS-gen link |

---

## 2. Per-task detail

### Phase F1 — `client-protocol` + `client-adapter` (deepest detail; freezes the wire contract first)

---

#### F1-00 — Scaffold `client-protocol` crate

- **Goal**: a compiling, workspace-wired engine-tier crate with the `uniffi` feature gate stubbed (off by default) so the SAME DTOs compile plain for bridge-server and as UniFFI types for mobile.
- **Files**: `lingxi-code/client-protocol/Cargo.toml`, `lingxi-code/client-protocol/src/lib.rs`, root `lingxi-code/Cargo.toml` workspace `members`.
- **Cargo deps**: `protocol` (path), `serde` (derive), `thiserror`. `serde_json` OMITTED (Value must not enter the contract crate). Feature `uniffi = ["dep:uniffi"]` with `uniffi` as an optional dep (the dep itself is added in F3-00; until then the feature is declared but not buildable — that is fine, default build does not need it).
- **Approach**: `#![forbid(unsafe_code)]`. Empty `lib.rs` with module stubs (`commands`, `events`, `message`, `permission`, `listings`, `error`, `version`). Wire into workspace members. Confirm class = `engine` via `python3 scripts/check_deps.py --list` (must print `client-protocol  engine`).
- **Tests FIRST (red→green)**: a trivial `#[test] fn crate_compiles() {}` plus a `version` module test asserting `CLIENT_PROTOCOL_VERSION` exists once F1-01 lands. (At F1-00 the red is "crate not in workspace / does not build".)
- **Verify**: `cargo build -p client-protocol` AND (after F3-00) `cargo build -p client-protocol --features uniffi`. Before F3-00, just the plain build.
- **depends_on**: none.

#### F1-01 — Core enums skeleton + version + serde conventions

- **Goal**: lock the serde conventions and the top-level `ClientCommand`/`ClientEvent` shells + `CLIENT_PROTOCOL_VERSION` and the `Error` event so every later DTO inherits the frozen conventions.
- **Files**: `client-protocol/src/version.rs`, `events.rs`, `commands.rs`, `lib.rs`.
- **Approach**:
  - `pub const CLIENT_PROTOCOL_VERSION: &str = "1.0.0";`
  - `ClientEvent` and `ClientCommand` are `#[non_exhaustive]` (mirrors `OutputEvent` at `traits/src/orchestrator.rs:371`).
  - Every enum: `#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]` + `#[serde(tag = "type", rename_all = "snake_case")]` (matches `protocol::ContentBlock` and api-client `StreamEvent`).
  - Every optional field: `#[serde(default, skip_serializing_if = "Option::is_none")]` (the UsageApi/CostSnapshot forward-compat pattern).
  - `Error { kind: ErrorKindDto, message: String }` with `ErrorKindDto = Transport | Protocol | Server | MaxTurns | Internal`.
- **Tests FIRST**: `version_is_semver()` (parses `1.0.0`); `error_event_round_trips()` (serialize/deserialize `ClientEvent::Error` and assert the JSON tag is `"error"` and snake_case fields).
- **Verify**: `cargo test -p client-protocol version error`.
- **depends_on**: F1-00.

#### F1-02 — `MessageDto` + `MessageBlockDto` (the shared block schema)

- **Goal**: a first-class `MessageDto` (was referenced 3× but never defined — the parity-critic finding). Carries the full block set the ~22 tool-card + diff + thinking renderers need so `MessageComplete` and a resumed scrollback are reproducible.
- **Files**: `client-protocol/src/message.rs`.
- **Approach**:
  - `MessageDto { role: String, blocks: Vec<MessageBlockDto> }`.
  - `MessageBlockDto = Text { text } | Thinking { thinking, signature: Option<String> } | RedactedThinking { data: String } | ToolUse { id, tool, input_json: String } | ToolResult { id, tool, result_json: String, is_error: bool, old_string: Option<String>, new_string: Option<String>, file_path: Option<String> }`.
  - Diff fields (`old_string`/`new_string`/`file_path`) mirror `UserToolResult` carried at `tui/src/state.rs:73-93`. `input_json`/`result_json` are JSON Strings (Value never enters the contract).
- **Tests FIRST**:
  - `message_dto_round_trips()` — full multi-block assistant message round-trips byte-stable.
  - `message_block_set_matches_tui_scrollback()` — construct a message with one of each block kind and assert the variant set equals the block kinds the TUI scrollback renders (Text/Thinking/RedactedThinking/ToolUse/ToolResult). This is the structural parity anchor.
- **Verify**: `cargo test -p client-protocol message`.
- **depends_on**: F1-01.

#### F1-03 — Live-turn event DTOs

- **Goal**: freeze the per-turn streaming events the adapter emits.
- **Files**: `client-protocol/src/events.rs`.
- **Approach** — add these `ClientEvent` variants (engine_source noted from the area maps):
  - `TextDelta { text }` (1:1 `OutputStream::emit_text`).
  - `ToolUseStarted { id, tool, input_json }` (1:1 `emit_tool_call`, Value lowered to JSON String).
  - `ToolUseResult { id, tool, result_json, is_error }` (1:1 `emit_tool_result`; fires in COMPLETION order — clients key by id).
  - `MessageComplete { stop_reason, message: Option<MessageDto> }` (synthesized; no engine message-boundary event).
  - `TurnStarted { turn_id: Option<u64> }` (adapter-synthesized on `SendPrompt` receipt; no engine source).
  - `TurnEnded { outcome: TurnOutcomeDto, stop_reason, cost: CostDto }` (1:1 `emit_end_turn`).
  - `CostUpdate { total_usd, input_tokens, output_tokens, api_calls, session_duration_secs, formatted }`.
  - `CompactionCompleted { messages_before, messages_after, bytes_saved }` (1:1 `emit_compaction_completed`).
  - `ThinkingDelta { thinking, signature: Option<String> }` and `UsageUpdate {…}` — **defined but reserved/feed-deferred** per decision §0.7. Include them in the enum (so they freeze now) and in the feed-status table (F1-08) marked RESERVED.
  - `TurnOutcomeDto = EndTurn | MaxTurns | Cancelled`; `CostDto` mirrors `CostSnapshot` lowered (Duration→secs).
- **Tests FIRST**: one `round_trips_*` test per variant; an `end_turn_outcome_variants()` test enumerating `TurnOutcomeDto`.
- **Verify**: `cargo test -p client-protocol events`.
- **depends_on**: F1-02.

#### F1-04 — Permission DTOs

- **Goal**: freeze the permission request/response shape sourced from `PermissionGate::check`.
- **Files**: `client-protocol/src/permission.rs`.
- **Approach**:
  - `PermissionRequest { request_id: u64, kind: PermissionKindDto, worker: Option<WorkerInfoDto> }`.
  - `PermissionKindDto = ToolUseConfirm { tool_name, tool_input_json: String, default_allow: bool } | ExitPlanMode { plan } | BypassPermissionsMode`. Only `ToolUseConfirm` has a live source; the other two are RESERVED, feed-deferred (decision §0.6).
  - `PermissionResolved { request_id: u64, response: PermissionResponseDto }`.
  - `PermissionResponseDto = AllowOnce | AllowAlways | Deny`.
  - `WorkerInfoDto` reserved (no wire identity today — `WorkerPermissionInfo` is TUI-side only).
- **Tests FIRST**: round-trip each `PermissionKindDto` variant; assert `ToolUseConfirm` carries `default_allow: bool` (the collapsed `PromptDefault`); `worker_is_optional_and_defaults_none()`.
- **Verify**: `cargo test -p client-protocol permission`.
- **depends_on**: F1-01.

#### F1-05 — Listing / screen DTOs

- **Goal**: freeze the pull/reply DTOs for all screens. Reconcile spec §4.1 names against wire names while the snapshot is still unfrozen (e.g. spec §4.1 says `AgentList`; wire name is `Agents` — reconcile NOW).
- **Files**: `client-protocol/src/listings.rs`.
- **Approach** — define (from the events catalog):
  - `SessionList { sessions: Vec<SessionRowDto{uuid, title, modified_rfc3339, message_count: u32, path} } }` (maps `SessionMetadata`; **`path` exists at `session/src/jsonl/loader.rs:33`** — map directly, do NOT synthesize). `SessionStarted`/`SessionEnded`/`SessionResumed` (the lossy-replay note goes in the feed-status doc).
  - `ModelList { models, current }`, `ModelChanged { model }`.
  - `McpServers { servers: Vec<McpServerDto{name, status: McpStatusDto, transport}> }`; `McpStatusDto = Connected | Disconnected | Error { reason }` (Error(String) lowered to struct variant for UniFFI flatness).
  - `Hooks`, `Agents`, `SlashCommandCatalog`, `MemoryEntries`, `StatusSnapshot` (traits shape canonical + status-line fields appended OPTIONAL), `SettingsSnapshot { effective_json, provenance_json }` (JSON Strings), `AuthState`, `DoctorReport`, `TaskRow`, `TaskOutputChunk`, `TaskStatusChanged`, `CoordinatorStatus` (RESERVED — decision §0.9).
- **Tests FIRST**: round-trip per listing DTO; `mcp_status_error_is_struct_variant()`; `session_row_carries_path()`; `status_snapshot_optional_fields_skip_when_none()`.
- **Verify**: `cargo test -p client-protocol listings`.
- **depends_on**: F1-01.

#### F1-06 — `ClientCommand` full set + session-lifecycle lock

- **Goal**: freeze every command, and ENCODE decision §0.5 — no command carries `session_id`.
- **Files**: `client-protocol/src/commands.rs`.
- **Approach** — `ClientCommand` variants:
  - `SendPrompt { text, prompt_mode: Option<PromptModeDto>, images: Vec<ImageRefDto{media_type, base64}>, turn_id: Option<u64> }` (inline image bytes per decision §0.8; `PromptModeDto = Normal | Bash | Memory | Plan`).
  - `Cancel { turn_id: Option<u64> }`, `ApprovePermission { request_id, response }`, `DenyPermission { request_id }`.
  - `SetModel { model }`, `ListModels`, `RunSlashCommand { raw }` (LOSSY at dispatcher — reply is `CommandResultDto { display, injected }`).
  - `RefreshListings { which: Vec<ListingKindDto> }`.
  - `NewSession { cwd: Option<String>, model: Option<String> }`, `ResumeSession { session_id: String, cwd: Option<String> }` (NOTE: `ResumeSession` carries `session_id` because it NAMES a target to resume — this is the ONE allowed occurrence; live commands during a session do not), `ListSessions { limit: Option<u32> }`.
  - `Login`, `Logout`, `ForceCompact`, `ClearSession`, `TaskList { status_filter }`, `TaskOutput { task_id, offset }`, `TaskStop { task_id }`, `RequestExit`.
  - **Explicitly NOT commands** (client-local; adapter does not back them): `SearchMessages`, `JumpToMessage`, `ExportSession`, `SetTheme`, prompt-history. Documented in the module doc-comment, not encoded as variants.
- **Tests FIRST**: round-trip per command; `no_live_command_carries_session_id()` — a test that pattern-matches every variant EXCEPT `ResumeSession` and asserts no `session_id` field (the lifecycle lock); `prompt_mode_variants_round_trip()`.
- **Verify**: `cargo test -p client-protocol commands`.
- **depends_on**: F1-03, F1-04, F1-05.

#### F1-07 — `ClientError`

- **Goal**: a flat, `#[derive(uniffi::Error)]`-ready error type composing with `thiserror`.
- **Files**: `client-protocol/src/error.rs`.
- **Approach**: `ClientError` flat variants mirroring command failure modes (`Transport`, `Protocol`, `Rejected`, `NotFound`, `Internal(String)`). No nested non-FFI payloads. The `uniffi::Error` derive is feature-gated (`#[cfg_attr(feature = "uniffi", derive(uniffi::Error))]`).
- **Tests FIRST**: round-trip; `error_variants_are_ffi_flat()` (compile-time — under `--features uniffi` in F3-01 this becomes the real check; here assert the Display strings).
- **Verify**: `cargo test -p client-protocol error`.
- **depends_on**: F1-01.

#### F1-08 — JSON-schema golden snapshots (FRONT-LOADED — freezes the wire contract)

- **Goal**: serialize one canonical instance of EVERY `ClientEvent`/`ClientCommand` variant + the `MessageDto` block set + the **RenderedMessage feed-status table** into checked-in `client-protocol/snapshots/*.json` goldens. The snapshot IS the frozen wire format and the auditable feed-status record. This is the single most important F1 deliverable — it is why the protocol DTOs are front-loaded.
- **Files**: `client-protocol/snapshots/` (checked in), `client-protocol/tests/snapshot_test.rs`.
- **Approach**:
  - One golden per variant (canonical instance with stable field values).
  - A `feed_status.json` golden enumerating the RenderedMessage feed-status table: LIVE-FED (~6: `UserText`, `AssistantText`, `AssistantToolUse`, `UserToolResult`, `CompactBoundary`; `AssistantThinking` only if §0.7 hook is taken — for the foundation it is RESERVED) and RESERVED/feed-deferred (~22: the full list from the events catalog incl. `ThinkingDelta`, `UsageUpdate`, `CoordinatorStatus`, `ExitPlanMode`, etc.). This makes "feed deferred" auditable so §5.3 "~22 renderers parity" is not overstated.
  - Snapshot framework: `insta` if already a workspace dev-dep, else a hand-rolled `assert_eq!(serde_json::to_string_pretty(&x), include_str!(golden))` (no new prod dep — dev-only).
- **Tests FIRST**: write the snapshot harness asserting each golden matches BEFORE generating goldens (red: no goldens → fail; green: generate + review goldens, re-run).
- **Verify**: `cargo test -p client-protocol --test snapshot_test`.
- **depends_on**: F1-03, F1-04, F1-05, F1-06, F1-07.

#### F1-09 — Structural version-diff guard

- **Goal**: enforce decision §0.10's structural rule, NOT a naive file-changed check.
- **Files**: `client-protocol/tests/version_guard_test.rs`, a small `client-protocol/snapshots/contract_index.json` (the set of variant tags + field names + types).
- **Approach**: the guard diffs the current contract index against the checked-in one and classifies each change: a REMOVED entry or a RENAMED/RETYPED tag/field ⇒ requires a MAJOR `CLIENT_PROTOCOL_VERSION` bump (test fails unless major changed); a NEW variant or NEW optional field ⇒ additive, no bump required (test passes). The index is regenerated by the same test in a `BLESS=1` mode.
- **Tests FIRST**: `removing_a_field_requires_major_bump()` (simulate by feeding a doctored index — assert the classifier returns `Breaking`); `adding_optional_field_is_compatible()` (assert `Compatible`); `current_contract_matches_index_or_version_bumped()`.
- **Verify**: `cargo test -p client-protocol --test version_guard_test`.
- **depends_on**: F1-08.

#### F1-10 — Scaffold `client-adapter` + `ClientEventSink`

- **Goal**: the engine→DTO bridge crate exists and defines the transport-agnostic sink.
- **Files**: `lingxi-code/client-adapter/Cargo.toml`, `src/lib.rs`, `src/sink.rs`; workspace members.
- **Cargo deps**: `client-protocol` (path), `protocol` (path), `traits` (path), `permission` (path — REQUIRED for `tool_default` + the re-exported `PermissionRequest`/`PermissionResponse` from `permission::gate`), `async-trait`, `tokio` (`mpsc`/`oneshot`/`Mutex`/`sync`), `tokio-util` (`CancellationToken`), `serde_json` (Value→String lowering — this is where Value is ALLOWED, NOT in client-protocol). **It must NOT import `tui`** — `BridgeOutputStream`/`TuiPermissionGate` are reference templates to COPY.
- **Approach**: `#[async_trait] pub trait ClientEventSink: Send + Sync { async fn emit(&self, ev: ClientEvent); }`. `lib.rs` re-exports the adapter types. Confirm class = `engine`.
- **Tests FIRST**: a `MockSink` capturing emitted events (in `client-adapter/src/test_support.rs`); `sink_trait_object_compiles()`.
- **Verify**: `cargo build -p client-adapter && cargo test -p client-adapter sink`.
- **depends_on**: F1-08.

#### F1-11 — Pure `From<engine type>` lowering fns

- **Goal**: the parity-test surface — pure mapping fns shared by both transports and tests.
- **Files**: `client-adapter/src/lowering.rs`.
- **Approach** — implement and unit-test each lowering rule:
  - `serde_json::Value` → JSON String (`input_json`/`result_json`).
  - `SystemTime` → RFC3339 String; `Duration` → `u64` secs; `usize` → `u32`.
  - `McpStatus::Error(String)` → `McpStatusDto::Error{reason}`; `PromptDefault` → `bool`; `CostSnapshot` → `CostDto`.
  - `SessionMetadata` → `SessionRowDto` (map `.path` directly).
  - `McpServerInfo`/`HookInfo`/`AgentInfo`/`StatusSnapshot`/`DoctorReport`/`TaskRecord`/`TaskOutputChunk` → their DTOs.
  - `GroupedToolUse`/`CollapsedReadSearch` folding stays CLIENT-SIDE (adapter emits raw `ToolUseStarted`/`ToolUseResult`).
- **Tests FIRST**: one unit test per lowering rule (e.g. `value_lowers_to_json_string()`, `system_time_to_rfc3339()`, `mcp_error_to_struct_variant()`, `cost_snapshot_to_dto()`).
- **Verify**: `cargo test -p client-adapter lowering`.
- **depends_on**: F1-10.

#### F1-12 — `AdapterOutputStream impl traits::OutputStream`

- **Goal**: the live-turn feed — the direct analog of `BridgeOutputStream` (`tui/src/events/orchestrator_bridge.rs`), pushing `ClientEvent` DTOs to the `ClientEventSink` instead of an mpsc `TurnEvent`.
- **Files**: `client-adapter/src/output_stream.rs`.
- **Approach**: implement the 5 `OutputStream` methods (verified at `traits/src/orchestrator.rs:423-456`): `emit_text` → `TextDelta`; `emit_tool_call` → `ToolUseStarted` (lower input via F1-11); `emit_tool_result` → `ToolUseResult`; `emit_end_turn` → `CostUpdate` + `TurnEnded`; `emit_compaction_completed` → `CompactionCompleted`. Holds an `Arc<dyn ClientEventSink>`.
- **Tests FIRST**: feed each callback with a fixture arg and assert the matching DTO appears on the `MockSink`. `emit_end_turn_produces_cost_then_turn_ended()` (asserts BOTH events, in order).
- **Verify**: `cargo test -p client-adapter output_stream`.
- **depends_on**: F1-11.

#### F1-13 — Live-turn wrapper: `MessageComplete` synthesis + error mapping

- **Goal**: synthesize the message-boundary event and translate the turn `Result`.
- **Files**: `client-adapter/src/turn.rs`.
- **Approach**:
  - Synthesize `MessageComplete` from `PumpedTurn{assistant_blocks, stop_reason}` (`streaming_loop.rs:40-50`) into a `MessageDto`.
  - Synthesize `TurnStarted` on `SendPrompt` receipt (engine never emits it).
  - Wrap the `run_turn` future and translate `Err(OrchestratorError)` → `Error` event: `Streaming(ApiError)` → `Transport`; `StreamingProtocol` (incl. mid-stream server error, mapped at `streaming_loop.rs:93`) → `Protocol`/`Server`; `StreamEndedWithoutStop`/`MaxTurnsReached` → `MaxTurns`/`Internal`.
- **Tests FIRST**: `each_orchestrator_error_maps_to_error_kind()` (table-driven over the variants); `message_complete_synthesized_from_pumped_turn()`; `turn_started_emitted_on_send()`.
- **Verify**: `cargo test -p client-adapter turn`.
- **depends_on**: F1-12.

#### F1-14 — `AdapterPermissionGate impl traits::PermissionGate` (id-keyed, fail-closed)

- **Goal**: the hardest mapping — replicate `TuiPermissionGate` but KEYED BY ID to multiplex concurrent worker+main requests, with an explicit fail-closed owner.
- **Files**: `client-adapter/src/permission_gate.rs`.
- **Approach** (mirror `tui/src/permission_bridge.rs:64-118`, COPY don't import):
  - `check(name, &Value) -> PermissionDecision`: consult session rules; call `permission::tool_default(name) -> PromptDefault`; build `PermissionKindDto::ToolUseConfirm` (collapse `PromptDefault`→`default_allow: bool`); assign a `request_id` via `AtomicU64`; emit `PermissionRequest`; park a `oneshot::Sender<PermissionResponse>` in `Mutex<HashMap<u64, oneshot::Sender>>`; `await` the oneshot.
  - `resolve(request_id, PermissionResponseDto)` (called by transport on `ApprovePermission`/`DenyPermission` from a DIFFERENT task): look up the id, send on the oneshot. `AllowAlways` appends a session `PermissionRule`.
  - **Fail-closed owner**: a connection-scoped guard owns the HashMap; on transport teardown/app-background it DRAINS the map (dropping every parked Sender ⇒ Deny); plus a per-request timeout that resolves Deny. So a vanished resolving task never hangs the turn future.
- **Tests FIRST** (mirror permission_bridge.rs's 4 gate tests, against the REAL `PermissionDecision`, not the DTO shorthand):
  - `gate_emits_request_and_resolves_allow_once()` — emit id N, `resolve(N, AllowOnce)`, assert `check()` returns `PermissionDecision::Allow`.
  - `gate_skips_dialog_when_session_rule_matches()`.
  - `gate_persists_allow_always()`.
  - `gate_denies_on_user_deny()`.
  - **NEW**: `concurrent_worker_and_main_ids_resolve_independently()`; `drop_resolves_deny()` (drain the map); `timeout_resolves_deny()`.
  - `tool_use_confirm_constructed_with_default_allow()` (asserts the `tool_default`→bool collapse).
- **Verify**: `cargo test -p client-adapter permission_gate`.
- **depends_on**: F1-11.

#### F1-15 — Listing/screen `From` parity tests reusing TUI render fixtures

- **Goal**: prove structural parity — for each engine event a TUI renderer consumes, feed the IDENTICAL input to the adapter's `From` fn and assert the matching DTO.
- **Files**: `client-adapter/tests/parity_test.rs` (loads fixtures from `tui/src/components/messages/` and the listing fixtures).
- **Approach**: drive `McpServerInfo`/`HookInfo`/`AgentInfo`/`StatusSnapshot`/`DoctorReport`/`TaskRecord`/`SessionMetadata` fixtures through the F1-11 fns; assert the DTO. Parity is structural, not re-derived. Reuse the M9 fixture corpus where the engine is live; deterministic fixtures where stubbed.
- **Tests FIRST**: one parity test per listing DTO (`mcp_info_parity()`, `agent_info_parity()`, `status_snapshot_parity()`, …).
- **Verify**: `cargo test -p client-adapter --test parity_test`.
- **depends_on**: F1-11.

#### F1-16 — CI dep-rule assertion (the ONE binding rule)

- **Goal**: gate the only rule that binds — no `client-protocol`/`client-adapter` → `apps/*` edge — and confirm `check-deps.sh` stays green after the two new crates land.
- **Files**: `client-adapter/tests/dep_rule_test.rs` (or a small entry in the existing CI script).
- **Approach**: shell out to `cargo metadata --no-deps` (or parse `scripts/check_deps.py --list`) and assert neither new crate lists any `apps/*` dependency. Run `scripts/check-deps.sh` and assert exit 0. (No `deny.toml` change in F1 — supply-chain only.)
- **Tests FIRST**: `client_protocol_has_no_app_edge()`, `client_adapter_has_no_app_edge()`, `check_deps_sh_green()`.
- **Verify**: `bash lingxi-code/scripts/check-deps.sh && cargo test -p client-adapter --test dep_rule_test`.
- **depends_on**: F1-10.

---

### Phase F2 — `bridge-server` walking skeleton (Electron transport)

> Relabeled from "bridge-server completion" to "bridge wire v0.2 + bridge-server" — it is net-new protocol work plus a shared-crate refactor.
> **Plug-in point**: `clients/electron` reads the dedicated `~/.claude/bridge/<port>.lock` (F2-04), connects over WS, handshakes (F2-02), and sends `ClientCommand` frames. No Electron UI rework in this plan.

---

#### F2-00 — `DesktopConfig` (deliverable-zero)

- **Goal**: define the deterministic, env/argv-free config so bridge-server AND the F2 e2e test can construct a runtime without `Argv`/`std::env`.
- **Files**: `apps/engine-desktop/src/lib.rs` (new `DesktopConfig` struct).
- **Approach**: `DesktopConfig { api_base, api_key, cwd, claude_home, default_model, provider_profiles, mcp_paths, use_noop_permission_gate: bool }`. Every field the ~270-line `build_runtime` (`apps/cli/src/init.rs:148`) reads from env/argv becomes an explicit field. `use_noop_permission_gate` lets the CLI opt into `NoOpPermissionGate` (init.rs:245) while bridge-server uses `AdapterPermissionGate`.
- **Tests FIRST**: `desktop_config_default_is_constructible()`; a doc-test showing field-by-field construction.
- **Verify**: `cargo build -p engine-desktop`.
- **depends_on**: F1-13, F1-14.

#### F2-01 — Lift `build_runtime` → `engine_desktop::build(DesktopConfig)`

- **Goal**: the single largest F2 item — move the runtime wiring out of `apps/cli` (which bridge-server CANNOT depend on: app→app forbidden, `check_deps.py:99`) into `engine-desktop`.
- **Files**: `apps/engine-desktop/src/lib.rs` (new `build`), `apps/engine-desktop/Cargo.toml` (deps the lifted code needs), `apps/cli/src/init.rs` (refactor `build_runtime` to call `engine_desktop::build` with a `DesktopConfig` derived from `Argv`/env).
- **Approach**: `pub async fn build(cfg: DesktopConfig) -> Result<DesktopRuntime, BuildError>` where `DesktopRuntime { orchestrator, dispatcher, auth, task_registry }`. Construct the orchestrator with `AdapterOutputStream` + `AdapterPermissionGate` (gated by `use_noop_permission_gate`). No `Argv`/`std::env` reads inside `build()` — those stay in `apps/cli` and feed the config. Preserve byte-equivalent engine behavior (spec §1 non-goal).
- **Tests FIRST**: `build_constructs_runtime_deterministically()` (no env/argv); `build_with_noop_gate_uses_noop()` and `build_default_uses_adapter_gate()`; an `apps/cli` regression test that the refactored `build_runtime` still produces an equivalent runtime.
- **Verify**: `cargo test -p engine-desktop build && cargo test -p lingxi-cli` (or the CLI crate's name).
- **depends_on**: F2-00.

#### F2-02 — Wire v0.2: tagged `Frame` + version carry

- **Goal**: add the server-push event frame the existing request/response wire lacks, and carry `CLIENT_PROTOCOL_VERSION` in the handshake.
- **Files**: `bridge/src/wire.rs`.
- **Approach**: add `pub enum Frame { Request(BridgeRequest) | Response(BridgeResponse) | Event(ClientEvent) }` (tagged, snake_case). `Request.params` = `ClientCommand` JSON; `Response.result`/`Event` = `ClientEvent` JSON; `id` only on command→reply correlation; events carry no id. Bump `BRIDGE_PROTOCOL_VERSION` `0.1.0` → `0.2.0` (wire.rs:16). Add a `client_protocol_version` field to `Capabilities` carrying `client_protocol::CLIENT_PROTOCOL_VERSION`. (`bridge` adds a `client-protocol` path dep — legal, both engine-tier.)
- **Tests FIRST**: extend `bridge/tests/protocol_roundtrip_test.rs` — `frame_request_round_trips()`, `frame_event_round_trips()`, `frame_response_round_trips()`; `capabilities_carry_client_protocol_version()`.
- **Verify**: `cargo test -p bridge --test protocol_roundtrip_test`.
- **depends_on**: F1-08.

#### F2-03 — Generalize `McpEndpoint` with a frame-pump callback

- **Goal**: build the real read/write pump while keeping the proven auth/upgrade code shared (recommended over a parallel WS variant).
- **Files**: `bridge/src/mcp_endpoint.rs`.
- **Approach**: REUSE verbatim `start_on_ephemeral_port` (binds `127.0.0.1:0`, accept loop, mcp_endpoint.rs:51), the constant-time auth check + 401-before-upgrade (mcp_endpoint.rs:128-142, `UNAUTHORIZED_BODY` :30), the `mcp` subprotocol echo. REPLACE the post-upgrade stub (`let _ = ws;` at mcp_endpoint.rs:167) with a frame pump: a `FramePump` callback/trait that the caller (bridge-server) supplies — deserialize inbound text frames as `Frame`, hand `ClientCommand` to the engine, push outbound `ClientEvent` frames. Keep the change additive (existing callers that pass no pump keep today's hold-open behavior, so `mcp_endpoint_test.rs` stays green).
- **Tests FIRST**: keep all of `bridge/tests/mcp_endpoint_test.rs` green (401-on-bad-token, upgrade-on-good-token, subprotocol echo); add `frame_pump_invoked_on_inbound_frame()` (a stub pump echoes a frame back and the test client receives it).
- **Verify**: `cargo test -p bridge --test mcp_endpoint_test`.
- **depends_on**: F2-02.

#### F2-04 — Dedicated bridge discovery lockfile

- **Goal**: avoid the IDE-peer collision — write `~/.claude/bridge/<port>.lock` with its OWN `ideName`, not the `~/.claude/ide/` + `IDE_NAME = "LingXi"` file (verified at `bridge/src/lockfile.rs:15`) which the real IDE peer scans.
- **Files**: `bridge/src/lockfile.rs` (a `for_bridge` constructor / dedicated dir) or a new `bridge/src/bridge_lockfile.rs`.
- **Approach**: reuse the `LockfileBody` shape + `LockfileGuard` Drop-cleanup + `generate_auth_token` (lockfile.rs:54) but root at `~/.claude/bridge/` with a distinct `ideName` (e.g. `"LingXi-Bridge"`). The Electron app reads this lockfile, connects, and echoes the token in the `X-Claude-Code-Ide-Authorization` header. (`AuthChallenge`/`AuthResponse` in wire.rs are M8 placeholders for the deprecated "mobile drives desktop" use case — out of scope, left unused.)
- **Tests FIRST**: `bridge_lockfile_writes_to_bridge_dir()`; `bridge_lockfile_uses_distinct_ide_name()`; `bridge_lockfile_drop_cleans_up()`.
- **Verify**: `cargo test -p bridge --test lockfile_test` (extend it).
- **depends_on**: F2-03.

#### F2-05 — WALKING SKELETON: one turn end-to-end over WS

- **Goal**: prove the whole transport with the minimum viable path — connect, handshake, run one turn, stream assistant text to a CLI test client. **This is the F2 gate before full coverage.**
- **Files**: `apps/bridge-server/src/main.rs` + `apps/bridge-server/src/server.rs` (server loop), `apps/bridge-server/Cargo.toml` (add `client-protocol`, `client-adapter`, `engine-desktop` path deps; keep `bridge`, `tokio`, `tracing`, `anyhow`), `apps/bridge-server/tests/e2e_one_turn_test.rs` (CLI test client harness).
- **Approach**: bridge-server builds `engine_desktop::build(DesktopConfig)` (deterministic — the reason F2-00 is deliverable-zero), starts the generalized `McpEndpoint` with a frame pump that routes `SendPrompt` → adapter-wired orchestrator turn (spawned, returns promptly), and pushes the adapter's `ClientEvent`s out as `Frame::Event`. Single Electron child per server; model single-client; multi-client is later.
- **Tests FIRST**: `drive_one_turn_over_ws()` — using `connect_async` + an `http::Request` with the auth header from the lockfile: handshake (assert `ServerHello` + version), send `Frame::Request(SendPrompt{text})`, assert a sequence of `Frame::Event(TextDelta)` then `Frame::Event(TurnEnded)`. The engine is driven against a stubbed `StreamingApiClient` returning a deterministic token stream (M9 "real where live, fixtures where stubbed").
- **Verify**: `cargo test -p bridge-server --test e2e_one_turn_test`.
- **depends_on**: F2-01, F2-03, F2-04, F1-13.

#### F2-06 — Permission round-trip over WS

- **Goal**: prove the inverted blocking permission handshake crosses the WS boundary without deadlock.
- **Files**: `apps/bridge-server/src/server.rs`, `apps/bridge-server/tests/e2e_permission_test.rs`.
- **Approach**: `AdapterPermissionGate::check()` blocks a tool dispatch on a oneshot (on the engine task); the WS read task (independent task) resolves it on inbound `ApprovePermission`/`DenyPermission(request_id)`. The connection-scoped guard drains the HashMap on disconnect (fail-closed). No deadlock as long as the gate awaits the oneshot (not a blocking lock).
- **Tests FIRST**: `permission_request_event_then_approve_resolves_check()` — drive a turn whose tool triggers `check()`, assert a `Frame::Event(PermissionRequest{request_id})` arrives, send `Frame::Request(ApprovePermission{request_id, AllowOnce})`, assert the tool proceeds and `TurnEnded` arrives. `disconnect_mid_permission_denies()` (drain ⇒ Deny).
- **Verify**: `cargo test -p bridge-server --test e2e_permission_test`.
- **depends_on**: F2-05, F1-14.

#### F2-07 — Version-mismatch refusal

- **Goal**: a client/engine disagreeing on a breaking version refuses to proceed — independently for both versions.
- **Files**: `apps/bridge-server/tests/version_mismatch_test.rs`.
- **Approach**: the handshake checks BOTH `BRIDGE_PROTOCOL_VERSION` (wire envelope) and `CLIENT_PROTOCOL_VERSION` (in `Capabilities`). A major mismatch in either refuses.
- **Tests FIRST**: `bridge_version_mismatch_refuses()`; `client_protocol_version_mismatch_refuses()` (one test each).
- **Verify**: `cargo test -p bridge-server --test version_mismatch_test`.
- **depends_on**: F2-02, F2-05.

#### F2-08 — Full command/event routing

- **Goal**: extend the skeleton to the full `ClientCommand` set + pull replies + the task poll loop.
- **Files**: `apps/bridge-server/src/router.rs`.
- **Approach**: route each command to its engine entry (HANDLE-backed: `switch_model`, `list_*`, `run_doctor_checks`, `get_status_snapshot`, `snapshot_cost`, `force_compact`, `clear_session`, `request_exit`, `TaskRegistryHandle::list/output/kill`, `AuthHandle::login/logout`; ENGINE-TIER/HOST reads: `SlashCommandCatalog` via `desktop_command_registry`, `SettingsSnapshot` via `Settings::load`, `MemoryEntries` via `memory::claude_md` walk, `SessionList` via `list_recent_sessions`). The adapter OWNS a task poll loop (matches the TUI; `TaskRegistryHandle::list` on an interval). `Cancel`/`ClearSession` mid-turn semantics enforced (reject `ClearSession` mid-turn).
- **Tests FIRST**: a routing test per command family (`set_model_routes()`, `list_mcp_routes()`, `slash_command_routes_to_registry()`, `task_list_poll_emits_task_row()`, `clear_session_rejected_mid_turn()`).
- **Verify**: `cargo test -p bridge-server`.
- **depends_on**: F2-05.

---

### Phase F3 — UniFFI client surface (mobile transport)

> **Plug-in point**: `clients/ios` consumes the generated `.xcframework` bindings — `LingxiCodeEngine.make(appSandboxRoot:)` builds `PlatformImpls` and calls `buildMobileEngine`; F3 adds `submit()` + a `ClientEventListener` the Swift side registers. No SwiftUI rework in this plan.

---

#### F3-00 — GATE-ZERO: uniffi offline-clearance + MSRV

- **Goal**: clear the supply-chain + MSRV blocker that ALL of F3 depends on. `uniffi` has ZERO occurrences in `Cargo.lock` (verified) — pulling it needs network and may violate the `--offline` build policy.
- **Files**: `lingxi-code/Cargo.toml` (workspace dep + possibly `rust-version` bump), `lingxi-code/Cargo.lock`, `lingxi-code/deny.toml`, the offline-allow/vendor mechanism.
- **Approach**: (1) pick an async-capable `uniffi` version (>= ~0.28); (2) VERIFY its MSRV against `rust-version = "1.82"` (Cargo.toml:159) — bump the pin if the chosen version requires it; (3) vendor/pin `uniffi` + `uniffi-bindgen` into `Cargo.lock` under the offline-allow mechanism; (4) add the licenses uniffi pulls in to `deny.toml` so the supply-chain gate stays green. Enable the uniffi `tokio` feature (for F3-07).
- **Tests FIRST**: `cargo build --offline` (whole workspace) must stay green WITH uniffi in the lock; `cargo deny check` green.
- **Verify**: `cargo build --offline -p client-protocol --features uniffi && cargo deny check`.
- **depends_on**: none (but blocks all F3 code).

#### F3-01 — DTOs as real UniFFI types

- **Goal**: light up the `uniffi` feature so the SAME `client-protocol` DTOs compile as UniFFI types.
- **Files**: `client-protocol/src/*.rs` (add `#[cfg_attr(feature = "uniffi", derive(uniffi::Enum/Record/Error))]`), `client-protocol/src/lib.rs` (`#[cfg(feature = "uniffi")] uniffi::setup_scaffolding!();`).
- **Approach**: every DTO field is already UniFFI-representable (the JSON-String-for-Value decision §0.4 is why). `ClientError` gets `uniffi::Error`. Confirm the F1-08 snapshots are unchanged (the derives are additive, not shape-changing).
- **Tests FIRST**: `cargo build -p client-protocol --features uniffi`; a smoke test that `uniffi-bindgen generate` produces a `.swift`/`.kt` without error; re-run F1-08 snapshots (must still pass — proves no shape drift).
- **Verify**: `cargo test -p client-protocol --features uniffi && cargo run -p uniffi-bindgen -- generate …` (or the project's bindgen invocation).
- **depends_on**: F3-00, F1-08.

#### F3-02 — `ClientEventListener` callback interface

- **Goal**: the OUTBOUND analog of the existing inbound `CameraControl`/`VoiceRecorder`/`SharingService` callbacks.
- **Files**: `client-adapter/src/listener.rs` (or a shared FFI-visible crate), `apps/ios-framework/src/lib.rs`, `apps/android-aar/src/lib.rs`.
- **Approach**: `#[uniffi::export(callback_interface)] #[async_trait] pub trait ClientEventListener: Send + Sync { async fn on_event(&self, event: ClientEvent); }`. The adapter holds `Arc<dyn ClientEventListener>` as its `ClientEventSink` and calls `on_event` for every translated DTO — mirroring `BridgeOutputStream`→`TurnEvent` but pushing to the listener. Register via `build_mobile_engine` arg or `set_event_listener`.
- **Tests FIRST**: a host-only fake listener (a `Mutex<Vec<ClientEvent>>` collector); `listener_receives_translated_event()` (feed the adapter sink, assert the fake listener captured it).
- **Verify**: `cargo test -p ios-framework listener` (host build).
- **depends_on**: F3-01, F1-12.

#### F3-03 — `engine_mobile::build_mobile(MobileConfig)` + shared host module

- **Goal**: the mobile-side equivalent of the `engine_desktop::build` lift — `engine-mobile` currently has EMPTY `[dependencies]` and builds NO orchestrator (verified). This is a sized item alongside F2-01, not an afterthought.
- **Files**: `apps/engine-mobile/src/lib.rs` (new `MobileConfig` + `build_mobile() -> MobileRuntime` + shared submit/listener/host module), `apps/engine-mobile/Cargo.toml` (add `tokio` rt-multi-thread, `client-adapter`, `client-protocol`).
- **Approach**: `MobileConfig` analog of `DesktopConfig`. `build_mobile()` constructs `BuiltinToolContext` from `Arc<dyn Platform>`, builds the `ConversationOrchestrator` via the mobile registries (`mobile_tool_registry`/`mobile_skill_registry`/`mobile_command_registry`, engine-mobile lib.rs:45-89), binds `AdapterOutputStream` + `AdapterPermissionGate`, stores the listener. The shared submit/listener/host logic lives HERE and is re-exported by both FFI crates (prevents iOS/Android drift). Off-device, gate the real `Platform` behind `cfg(target_os)` and provide a host-test shim.
- **Tests FIRST**: `build_mobile_constructs_orchestrator()` (host build with a fake `Platform` shim — see F3-06); `mobile_runtime_binds_adapter_sinks()`.
- **Verify**: `cargo test -p engine-mobile build_mobile` (host).
- **depends_on**: F3-01, F1-13, F1-14.

#### F3-04 — `MobileEngineHandle` becomes a real session host

- **Goal**: grow the stub (`create_session` returns `Internal` error; holds only `Arc<dyn Platform>` + `skill_count`, verified `apps/ios-framework/src/lib.rs:64-93`) into a real host.
- **Files**: `apps/ios-framework/src/lib.rs`, `apps/android-aar/src/lib.rs`.
- **Approach**: `MobileEngineHandle` (becomes `#[derive(uniffi::Object)]`) owns the handle-owned tokio runtime, the `MobileRuntime` from F3-03, the adapter, and the registered `Arc<dyn ClientEventListener>`. Both FFI crates re-export the shared host from `engine-mobile`. `build_mobile_engine` accepts/stores the listener.
- **Tests FIRST**: `handle_holds_runtime_and_listener()` (host build with shim); `create_session_no_longer_stubbed()`.
- **Verify**: `cargo test -p ios-framework && cargo test -p android-aar` (host).
- **depends_on**: F3-03.

#### F3-05 — `submit(command)` async FFI entry point

- **Goal**: the inbound command path — async `#[uniffi::export]`.
- **Files**: `apps/ios-framework/src/lib.rs`, `apps/android-aar/src/lib.rs` (thin wrappers over the engine-mobile shared `submit`).
- **Approach**: `async fn submit(&self, command: ClientCommand) -> Result<(), ClientError>`. `SendPrompt` spawns the streaming turn on the handle's tokio runtime and returns promptly (results stream via the listener — MUST NOT block the FFI call for the whole turn); `Cancel` fires the `CancellationToken`; `ApprovePermission`/`DenyPermission` resolve the parked oneshot (F1-14); `SetModel` → `switch_model`; `RunSlashCommand` → `mobile_command_registry` dispatch; `RefreshListings` → `list_*` + engine-tier reads.
- **Tests FIRST**: `submit_send_prompt_returns_promptly()` (assert the call returns before `TurnEnded`); `submit_cancel_fires_token()`; `submit_approve_resolves_oneshot()`.
- **Verify**: `cargo test -p engine-mobile submit` (host).
- **depends_on**: F3-04, F3-02.

#### F3-06 — WALKING SKELETON: prove submit() + listener from a host unit test

- **Goal**: the F3 equivalent of F2-05 — prove the skeleton without a device. **Gate before full coverage.**
- **Files**: `apps/engine-mobile/src/test_support.rs` (host-only fake `Platform` shim — `platform-ios`/`platform-android` are `cfg(target_os)`-gated so off-device `build_mobile_engine` returns `NotOnIos`/`NotOnAndroid`), `apps/engine-mobile/tests/skeleton_test.rs`.
- **Approach**: a host-only fake `Platform` (fs/http/clock stubs) lets `build_mobile()` run on CI. Build the runtime, register a fake `ClientEventListener`, `submit(SendPrompt)` against a stubbed `StreamingApiClient`, assert the listener receives `TextDelta` then `TurnEnded`. This is exactly the spec §8 "prove from a Swift/Kotlin unit test" smoke test, runnable on the host.
- **Tests FIRST**: `submit_send_prompt_drives_listener_text_then_turn_ended()`.
- **Verify**: `cargo test -p engine-mobile --test skeleton_test`.
- **depends_on**: F3-05.

#### F3-07 — Async-over-FFI runtime registration

- **Goal**: name the registration mechanism explicitly (not merely assert one exists) — UniFFI async export requires a registered foreign async runtime.
- **Files**: `client-protocol`/`engine-mobile` (enable uniffi `tokio` feature), `apps/ios-framework`/`apps/android-aar` (register the handle-owned tokio rt-multi-thread as the foreign executor).
- **Approach**: enable uniffi's `tokio` feature (F3-00 pinned it) and register the `MobileEngineHandle`-owned tokio runtime as the foreign async executor so `submit()`/`on_event` resolve on it. Verify the async export actually awaits on that runtime.
- **Tests FIRST**: `async_submit_resolves_on_handle_runtime()` (host); confirm bindgen emits the async-callback scaffolding.
- **Verify**: `cargo test -p engine-mobile async && cargo build -p ios-framework --features uniffi`.
- **depends_on**: F3-05.

---

### INT-01 — Final integration + verification

- **Goal**: prove both transports carry the SAME contract, the dep-gate is green, and the snapshot/TS-gen link holds.
- **Files**: a top-level `lingxi-code/tests/m10_foundation_smoke.rs` or a CI step; no new source.
- **Approach**:
  1. `cargo test` across the whole workspace (all F1/F2/F3 suites green).
  2. `bash lingxi-code/scripts/check-deps.sh` exit 0 (the two new crates introduce no forbidden edge); re-run the F1-16 assertion.
  3. `cargo build --offline` green WITH uniffi in the lock (F3-00 held).
  4. `cargo deny check` green.
  5. Re-freeze: run the F1-08 snapshot + F1-09 version-guard one more time; confirm the bridge-server e2e (F2-05) and the mobile skeleton (F3-06) both stream `TextDelta`→`TurnEnded` from the SAME `client-protocol` DTOs.
  6. Confirm the TS SDK types in `clients/shared` are GENERATED from the F1-08 JSON snapshots (the real link — no off-the-shelf serde↔Swift/Kotlin differ; the Swift/Kotlin contract is covered by F3-06, not a three-way file differ).
- **Tests FIRST**: the verification is the test — each command above must pass; capture the exact commands in the CI step.
- **Verify**: `cargo test && bash lingxi-code/scripts/check-deps.sh && cargo build --offline && cargo deny check`.
- **depends_on**: F2-08, F3-07.

---

## 3. Sequencing notes

- **F1 is fully front-loaded**: the DTO freeze (F1-08) + version guard (F1-09) land before ANY adapter logic depends on it, so the wire contract is frozen and auditable early. The adapter parity tests (F1-12..F1-15) are the core F1 deliverable.
- **F2-00 is deliverable-zero**: `DesktopConfig` + the `build_runtime` lift unblock everything downstream in F2 (the e2e test cannot build a runtime deterministically without it).
- **F3-00 is gate-zero**: the uniffi offline/MSRV clearance blocks all F3 code; it has no code dependency and can be done in parallel with F1/F2.
- **Walking skeletons (F2-05, F3-06) precede full coverage** (F2-08 / the rest of F3) per spec §8.
- **Deferred (NOT foundation, recorded for follow-up)**: live `ThinkingDelta`/`UsageUpdate` (needs additive `emit_thinking`/`emit_usage`), mobile inline image input (needs `run_turn_streaming_with_image_sources`), coordinator/team §5.6 (needs a runtime `TeamRegistry` + worker identity on `PermissionRequest`), programmatic settings/memory write (the `$EDITOR`-only gap blocking §5.8 full editing). All four are §1-respecting engine changes outside the foundation.
