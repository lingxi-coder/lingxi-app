# LingXi Core M6 · Plan 05 · Permission Dialogs — 3 modal dialogs + focus-trap

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. **Multi-commit allowed** — every implementation task ends with its own commit. The verification gate (final task) is the workspace-wide guard.

**Goal:** Land the 3 permission modal dialogs the TUI must support — `ToolUseConfirm`, `ExitPlanMode`, `BypassPermissionsMode` — backed by a `PermissionResponse` orchestrator event channel, with strict focus-trap discipline (when a dialog is open, keystrokes never reach `PromptInput`). After this plan, the REPL screen can route a real `PermissionRequest` from `ConversationOrchestrator` → render dialog → collect `(PermissionResponse, persist: bool)` → forward to orchestrator. `1` / `2` / `N` / `Esc` / `Enter` resolve `ToolUseConfirm` and `ExitPlanMode`; `BypassPermissionsMode` adds a literal-input step (user must type `yes` to enable). 2 new telemetry events + 1 new parity fixture.

**Architecture:** A new `permissions/` component subtree under `crates/tui/src/components/` provides three iocraft components — each takes a `PermissionRequest` variant + an `on_resolve` callback. `screens/repl.rs` overlays the active dialog above `PromptInput` whenever `AppState.pending_permission.is_some()`, and the keymap routes ALL keys to the dialog handler in that case (focus-trap). `AppState` grows two fields — `pending_permission: Option<PermissionRequest>` and `pending_permission_resp_tx: Option<oneshot::Sender<PermissionResponse>>` — set when the orchestrator bridge yields a `TuiEvent::OrchestratorPermissionRequest { request, resp_tx }`. A new `PermissionResponse { decision: PermissionDecisionKind, persist: bool }` enum is defined in `lingxi-traits::prompting_gate` and re-exported from `lingxi-permission`. Variants: `AllowOnce` (persist=false), `AllowAlways` (persist=true), `Deny` (persist=false).

**Tech Stack:** Rust 2021. New deps: NONE (uses existing `iocraft = "=0.6"` from M6-01, `tokio` workspace, `serde 1`, `serde_json 1`, `insta` for snapshots). New types live in `lingxi-traits::prompting_gate` (already exists from M5-05). The `PermissionRequest` enum gets new variants — see "Type extension" below.

**References:**

- Parent design spec: `docs/superpowers/specs/2026-05-28-m6-tui-foundation-design.md` (committed; §3 sub-plan M6-05 lines 443-478)
- Predecessors:
  - M6-01: `crates/tui` crate exists with iocraft event loop, `TuiEvent` enum, `OrchestratorBridge` mpsc, panic-safe terminal restore
  - M6-02: `crates/tui/src/screens/repl.rs` exists with 3-zone layout (StatusLine / Scrollback / PromptInput); `AppState` defined in `crates/tui/src/app.rs`
  - M6-03: streaming + spinner integrated; `streaming: Option<{turn_id, partial}>` field on AppState
  - M6-04: tool-use rendering complete; `AssistantToolUseMessage` and `UserToolResultMessage` exist; the orchestrator emits `ToolUseStart` / `ToolUseResult` events
- M5-05 (Permission UX precedent): `docs/superpowers/plans/2026-05-25-m5-05-permission-ux.md` — defined `PermissionRequest` + `PromptDecision` types in `lingxi-traits::prompting_gate`; established the stdin/stderr scripted-I/O test pattern. M6-05 extends those types with `PermissionRequest` variants + adds `PermissionResponse`.
- claude-code byte-locks (verified at plan-writing time via direct reads):
  - `claude-code/src/components/permissions/PermissionRequest.tsx:128-143` — `getNotificationMessage` returns:
    - `"Claude needs your permission to use ${toolName}"` (generic)
    - `"Claude Code needs your approval for the plan"` (ExitPlanMode)
    - `"Claude Code wants to enter plan mode"` (EnterPlanMode — M7 territory, not landed in M6)
  - `claude-code/src/components/permissions/FallbackPermissionRequest.tsx:158-208` — Select options:
    - `{ label: "Yes", value: "yes" }`
    - `{ label: "Yes, and don't ask again for {toolName} commands in {originalCwd}", value: "yes-dont-ask-again" }` (only shown when `shouldShowAlwaysAllowOptions()` returns true)
    - `{ label: "No", value: "no" }`
    - **No keyboard shortcuts `1`/`2`/`N` are wired in claude-code's React `<Select>`** — it uses arrow-keys + Enter only. M6-05 ADDS the number-key shortcuts as a LingXi-locked TUI convenience (documented in §"LingXi divergence" below).
  - `claude-code/src/components/BypassPermissionsModeDialog.tsx:53-66` — claude-code's bypass dialog uses a `<Select>` with two options (`"No, exit"` / `"Yes, I accept"`) — NOT a "type yes" confirmation. The task brief locks LingXi to a typed-`yes` confirmation step for the M6-05 TUI; this is a deliberate divergence (documented below).
  - Title literal: `"WARNING: Claude Code running in Bypass Permissions mode"` (line 73).
  - Body literal: `"In Bypass Permissions mode, Claude Code will not ask for your approval before running potentially dangerous commands.\nThis mode should only be used in a sandboxed container/VM that has restricted internet access and can easily be restored if damaged."` + `"By proceeding, you accept all responsibility for actions taken while running in Bypass Permissions mode."` (lines 53-57).
  - Telemetry literals (in claude-code): `"tengu_bypass_permissions_mode_dialog_shown"` (line 85), `"tengu_bypass_permissions_mode_dialog_accept"` (line 31). LingXi tracks these as `tengu_tui_permission_dialog_shown` / `tengu_tui_permission_dialog_resolved` with `kind = "bypass_permissions"` discriminant.
- Existing surfaces consumed by this plan:
  - `lingxi-core/crates/traits/src/prompting_gate.rs` — already exports `PermissionRequest { tool_name, tool_input, default_decision }` (M5-05). Task 2 step 1 of this plan **REPLACES the existing struct with an enum** (preserves stdio path via a `ToolUseConfirm { tool_name, tool_input, default_decision }` variant — bit-identical to the M5-05 struct's three fields).
  - `lingxi-core/crates/orchestrator/src/conversation.rs` — orchestrator currently consults `Arc<dyn PermissionGate>` synchronously (M5-05). Task 7 adds an out-of-band `permission_event_tx: Option<mpsc::Sender<PermissionExchange>>` field; when set, the orchestrator uses a `TuiPermissionGate` that sends the request to the TUI and awaits the response via oneshot.
  - `lingxi-core/crates/tui/src/events/mod.rs` — `TuiEvent` enum (M6-01). Task 7 step 2 adds an `OrchestratorPermissionRequest { request: PermissionRequest, resp_tx: oneshot::Sender<PermissionResponse> }` variant.
  - `lingxi-core/crates/tui/src/app.rs` — `AppState` (M6-02). Task 7 step 4 adds `pending_permission: Option<PermissionRequest>` + `pending_permission_resp_tx: Option<oneshot::Sender<PermissionResponse>>`.
  - `lingxi-core/crates/tui/src/screens/repl.rs` — Task 8 step 1 overlays the dialog above `PromptInput` when `pending_permission.is_some()`.
  - `lingxi-core/crates/tui/src/events/keymap.rs` — Task 8 step 3 adds a focus-trap branch: when `pending_permission.is_some()`, ALL keys route to the dialog handler; PromptInput state is never touched.
  - `lingxi-core/crates/telemetry/src/tengu/` — Task 13 appends 2 new constants. The post-M6-04 baseline count is locked in M6-04 (TBD by M6-04 — for the purposes of this plan, refer to it as `<M6_04_TOTAL>`; Task 13 step 5 computes `<M6_04_TOTAL> + 2` and updates the `event_name_completeness_test.rs`).
  - `lingxi-core/crates/test-harness/src/parity/fixtures/` — Task 12 creates `tui_permission_dialogs.json` (new parity fixture).

- Repo conventions:
  - Tests live in `#[cfg(test)] mod tests { ... }` blocks adjacent to production code; integration tests live under `crates/<crate>/tests/<name>_test.rs`.
  - `crates/tui/tests/snapshots/` holds insta `.snap` files (set up in M6-02).
  - All component files start with `#![forbid(unsafe_code)]`.
  - Iocraft component naming: `mod foo` → `pub fn Foo(props: &FooProps) -> impl Into<AnyElement<'static>>`.
  - Telemetry events: `tengu_<category>_<verb>_<noun>` (here: `tengu_tui_permission_dialog_shown`, `tengu_tui_permission_dialog_resolved`).
  - Insta snapshot policy: redact escape sequences via `[filters]` so layout assertions are stable across terminals.

---

## Reverse-engineered byte-locks

| Lock id | Value | Source |
|---|---|---|
| Generic header | `"Claude needs your permission to use {tool_name}"` | `claude-code/src/components/permissions/PermissionRequest.tsx:142` |
| ExitPlanMode header | `"Claude Code needs your approval for the plan"` | `claude-code/src/components/permissions/PermissionRequest.tsx:131` |
| BypassPermissions title | `"WARNING: Claude Code running in Bypass Permissions mode"` | `claude-code/src/components/BypassPermissionsModeDialog.tsx:73` |
| BypassPermissions body 1 | `"In Bypass Permissions mode, Claude Code will not ask for your approval before running potentially dangerous commands.\nThis mode should only be used in a sandboxed container/VM that has restricted internet access and can easily be restored if damaged."` | Same file, line 53 |
| BypassPermissions body 2 | `"By proceeding, you accept all responsibility for actions taken while running in Bypass Permissions mode."` | Same file, line 53 |
| Bypass confirmation word | `"yes"` (case-insensitive — `lowercased == "yes"`) | LingXi divergence — claude-code uses Select, not literal-input. **Locked here** because the M6-05 task brief calls for it. |

### LingXi divergence (documented, not a bug)

claude-code's three dialogs use React `<Select>` with arrow-key navigation. LingXi M6-05 locks **three numeric keyboard shortcuts** for `ToolUseConfirm` and `ExitPlanMode` (`1` = AllowOnce, `2` = AllowAlways, `N` = Deny) **in addition to** arrow-key + Enter navigation. The shortcuts are LingXi-locked and tested in this plan; they don't have a claude-code source line because they don't exist there. Esc → Deny matches claude-code's `Dialog onCancel` handler (`BypassPermissionsModeDialog.tsx:81-83`: `gracefulShutdownSync(0)` on Esc — LingXi maps to `Deny` rather than exiting the process, because the orchestrator owns the deny path).

For `BypassPermissionsMode`, LingXi requires the user to literally type `yes` + Enter to enable; this is a deliberate friction step beyond claude-code's Select. The task brief explicitly calls for it (it matches the spirit of claude-code's banner severity and is a more typical CLI safety idiom than React's Select).

### Button labels (LingXi-locked)

LingXi-locked labels chosen for the M6-05 dialogs (NOT byte-equivalent to claude-code — claude-code's Select labels are `Yes` / `Yes, and don't ask again for X commands in Y` / `No`):

| LingXi label | LingXi response | Maps to claude-code |
|---|---|---|
| `[1] Allow Once` | `PermissionResponse::AllowOnce` | `Yes` (without `dont-ask-again`) |
| `[2] Allow Always` | `PermissionResponse::AllowAlways` | `Yes, and don't ask again for {tool} commands in {cwd}` |
| `[N] Deny` | `PermissionResponse::Deny` | `No` |

These labels are **byte-locked at the LingXi project level** (Task 12's parity fixture locks them). The literal-lock list in M6-09 will record the divergence from claude-code and the rationale (TUI-shortcut affordance).

---

## Design locks

- **`PermissionRequest` becomes an enum (replacing the M5-05 struct).** Three variants:
  ```rust
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub enum PermissionRequest {
      /// Generic tool-use confirmation (the M5-05 case, preserved).
      ToolUseConfirm {
          tool_name: String,
          tool_input: serde_json::Value,
          default_decision: PromptDefault,
      },
      /// ExitPlanMode — user must approve the proposed plan before execution.
      ExitPlanMode {
          /// Plan markdown body. Rendered as multi-line block in the dialog.
          plan: String,
      },
      /// BypassPermissionsMode — user must confirm the dangerous mode toggle.
      BypassPermissionsMode,
  }
  ```
  The M5-05 stdio path uses only `ToolUseConfirm`; M6-05 adds `ExitPlanMode` and `BypassPermissionsMode`. Task 2 step 4 of this plan refactors the M5-05 `InteractivePromptingGate` to construct `PermissionRequest::ToolUseConfirm { .. }` for all stdio prompts — its behavior is unchanged.

- **`PermissionResponse` is new.** Three variants:
  ```rust
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum PermissionResponse {
      /// Allow this single tool call. `persist = false`.
      AllowOnce,
      /// Allow this tool for the rest of the session (and write a session rule).
      AllowAlways,
      /// Reject the tool call.
      Deny,
  }
  ```
  Stored in `lingxi-traits::prompting_gate`. Re-exported from `lingxi-permission::gate` (the existing M5-05 shim).

- **Orchestrator → TUI bridge** uses a `tokio::sync::oneshot::Sender<PermissionResponse>` paired with each request. When the orchestrator's `TuiPermissionGate::check` is called, it:
  1. Builds a `PermissionRequest` variant.
  2. Creates `let (tx, rx) = oneshot::channel()`.
  3. Sends `TuiEvent::OrchestratorPermissionRequest { request, resp_tx: tx }` over the TUI mpsc.
  4. `rx.await` blocks until the TUI dialog resolves (or the channel is dropped → `Cancelled`).
  5. Maps `PermissionResponse` → `PermissionDecision` (Allow* → Allow, Deny → Deny with reason `"user denied via dialog"`).

  **`AllowAlways`** in M6-05 emits a session-scoped allow rule via the orchestrator's session-rules sink (the `PermissionUpdate` mechanism — already wired through `lingxi-permission::PermissionRule`). M6 ships the wire path; the actual persistence to `localSettings` is M7 territory (`/permissions` command). M6-05 implements the **in-memory session rule** path only — the rule lives until the orchestrator's session ends.

- **Focus-trap discipline (CRITICAL).** When `AppState.pending_permission.is_some()`:
  - Keymap handler matches keys against the active dialog FIRST.
  - PromptInput's text buffer is NOT mutated regardless of what key is pressed.
  - Scrollback `j`/`k` navigation is also disabled.
  - This is enforced at the keymap dispatch site (`crates/tui/src/events/keymap.rs::handle_key`), NOT inside individual components — components don't know whether they're focused.

- **Dialog rendering** uses iocraft's `Box` with `border_style: BorderStyle::Single` + `padding: 1` to draw the dialog frame. The REPL screen overlays the dialog using iocraft's z-index (highest z), so it visually sits above the StatusLine / Scrollback / PromptInput. Behind the dialog, the rest of the screen is rendered with `dim_color: true` to indicate it's inactive.

- **`AllowAlways` session-rule sink.** `ConversationOrchestrator` grows an `Arc<Mutex<Vec<lingxi_permission::PermissionRule>>>` field `session_allow_rules`. The `TuiPermissionGate` appends `PermissionRule { tool: tool_name, behavior: Allow, source: Session }` when the user picks `AllowAlways`. The existing M5-05 `PolicyGate` (or whatever the M5-05 wiring placed at the top of the gate chain) consults this list before consulting the prompting gate. M6-05 Task 7 step 8 adds the lookup. M7's `/permissions` command surfaces these rules; M6-05 just plumbs the list.

- **Test pattern** for the dialogs mirrors M5-05's `tokio::io::duplex` approach but uses iocraft's headless test harness. iocraft exposes `iocraft::testing::render_to_buffer` (or equivalent — verified by M6-01) which returns the rendered ANSI bytes for a given props snapshot. For behavior tests, we drive the dialog's keymap via a synthesized `crossterm::event::KeyEvent`. The dialog component exposes a `pub fn handle_key(props: &Props, state: &mut DialogState, key: KeyEvent) -> Option<DialogResolution>` helper that's pure (no async, no I/O) and unit-testable in isolation. Task 3 step 2 implements this helper.

- **Telemetry events** (LingXi-locked):
  - `tengu_tui_permission_dialog_shown` — emitted when `pending_permission` transitions from `None` to `Some(_)`. Payload: `{ kind: PiiTagged("tool_use" | "exit_plan_mode" | "bypass_permissions"), tool_name: Option<PiiTagged> }`.
  - `tengu_tui_permission_dialog_resolved` — emitted when the user resolves the dialog. Payload: `{ kind: PiiTagged(<same>), decision: Verified("allow_once" | "allow_always" | "deny"), persist: Verified("true" | "false"), elapsed_ms: Verified(u64) }`.
  - Both events appended to `lingxi-core/crates/telemetry/src/tengu/tui.rs` (created in M6-01; Task 13 extends the `NAMES` slice).

---

## Files this plan touches

**Creates (new files):**

- `lingxi-core/crates/tui/src/components/permissions/mod.rs` — module barrel + `PermissionDialogProps` shared types + `DialogResolution` enum.
- `lingxi-core/crates/tui/src/components/permissions/tool_use_confirm.rs` — `ToolUseConfirm` iocraft component + `handle_key` helper.
- `lingxi-core/crates/tui/src/components/permissions/exit_plan_mode.rs` — `ExitPlanMode` iocraft component + `handle_key` helper.
- `lingxi-core/crates/tui/src/components/permissions/bypass_permissions.rs` — `BypassPermissionsMode` iocraft component + `handle_key` helper (with `yes`-typing state machine).
- `lingxi-core/crates/tui/src/components/permissions/tests/snapshot_tool_use_confirm.rs` — insta snapshot test.
- `lingxi-core/crates/tui/src/components/permissions/tests/snapshot_exit_plan_mode.rs` — insta snapshot test.
- `lingxi-core/crates/tui/src/components/permissions/tests/snapshot_bypass_permissions.rs` — insta snapshot test.
- `lingxi-core/crates/tui/tests/behavior_permission_dialogs.rs` — 6 behavior tests (5 covered in §"Tests required", plus 1 focus-trap test).
- `lingxi-core/crates/test-harness/src/parity/fixtures/tui_permission_dialogs.json` — locks labels + key bindings + decision discriminants.
- `lingxi-core/crates/test-harness/tests/parity_tui_permission_dialogs.rs` — driver that loads the fixture and replays each scenario against the dialog components.

**Modifies:**

- `lingxi-core/crates/traits/src/prompting_gate.rs` — REPLACE struct `PermissionRequest` with enum (3 variants) + ADD `PermissionResponse` enum.
- `lingxi-core/crates/permission/src/gate.rs` — re-export `PermissionResponse` (1-line addition to the `pub use lingxi_traits::prompting_gate::{..}` list).
- `lingxi-core/crates/permission/src/prompting_gate.rs` — update `InteractivePromptingGate::prompt_user` to construct `PermissionRequest::ToolUseConfirm { .. }` (M5-05 stdio path). Pattern-match on the enum to keep behavior identical; the other two variants return `PromptError::Cancelled { reason: "unsupported in stdio".into() }` because the M5-05 stdio gate doesn't know how to render multiline plans or warnings — the TUI gate is the only consumer for those.
- `lingxi-core/crates/orchestrator/src/conversation.rs` — ADD `permission_event_tx: Option<mpsc::Sender<PermissionExchange>>` field + `session_allow_rules: Arc<Mutex<Vec<PermissionRule>>>` field; update `ConversationOrchestrator::new` + `new_with_perms` to accept them.
- `lingxi-core/crates/orchestrator/src/handle_impl.rs` — ADD `TuiPermissionGate { event_tx, session_allow_rules }` struct + `impl PermissionGate for TuiPermissionGate` that consults session rules first, then sends the request to the TUI and awaits the oneshot.
- `lingxi-core/crates/tui/src/events/mod.rs` — ADD `OrchestratorPermissionRequest { request: PermissionRequest, resp_tx: oneshot::Sender<PermissionResponse> }` variant to `TuiEvent`.
- `lingxi-core/crates/tui/src/events/keymap.rs` — ADD focus-trap branch (lines documented in Task 8).
- `lingxi-core/crates/tui/src/app.rs` — ADD `pending_permission: Option<PermissionRequest>` + `pending_permission_resp_tx: Option<oneshot::Sender<PermissionResponse>>` fields; ADD `pending_permission_started_at: Option<Instant>` for the telemetry `elapsed_ms`.
- `lingxi-core/crates/tui/src/screens/repl.rs` — overlay dialog on top of the 3-zone layout when `pending_permission.is_some()`; dim the backdrop.
- `lingxi-core/crates/tui/src/components/mod.rs` — ADD `pub mod permissions;`.
- `lingxi-core/crates/tui/src/telemetry.rs` — extend the TUI telemetry module with the 2 new events.
- `lingxi-core/crates/telemetry/src/tengu/tui.rs` — APPEND `PERMISSION_DIALOG_SHOWN` + `PERMISSION_DIALOG_RESOLVED` constants + 2 payload structs.
- `lingxi-core/crates/telemetry/src/tengu/mod.rs` — bump the `tui` submodule's count by 2 in `TOTAL` formula.
- `lingxi-core/crates/telemetry/tests/event_name_completeness_test.rs` — bump expected count by 2 (assumes the M6-04 baseline of `<M6_04_TOTAL>`; Task 13 computes `<M6_04_TOTAL> + 2`).

**Deletes:** none.

---

## Tasks

> 13 TDD tasks. Tasks 1-2 update shared types. Tasks 3-5 build the three dialog components (one per task, TDD). Tasks 6-7 wire orchestrator → TUI bridge. Task 8 wires the REPL screen + keymap focus-trap. Tasks 9-11 add behavior tests. Task 12 ships the parity fixture. Task 13 is the workspace verification gate + telemetry registration + tag.

### Task 1: Extend `PermissionRequest` to an enum + add `PermissionResponse`

**Files:**
- Modify: `lingxi-core/crates/traits/src/prompting_gate.rs`
- Modify: `lingxi-core/crates/permission/src/gate.rs`
- Modify: `lingxi-core/crates/permission/src/lib.rs`

**Steps:**

- [ ] **Step 1: Read current state.** Open `lingxi-core/crates/traits/src/prompting_gate.rs`. Confirm `pub struct PermissionRequest { tool_name, tool_input, default_decision }` exists (M5-05). If it doesn't, STOP — M5-05 prerequisite missing.

- [ ] **Step 2: Write failing tests for the new enum + response.**

  Append to `lingxi-core/crates/traits/src/prompting_gate.rs` `#[cfg(test)] mod tests`:
  ```rust
  #[test]
  fn permission_request_enum_tool_use_confirm_variant() {
      let req = PermissionRequest::ToolUseConfirm {
          tool_name: "Bash".to_string(),
          tool_input: serde_json::json!({ "command": "ls" }),
          default_decision: PromptDefault::DenyByDefault,
      };
      match req {
          PermissionRequest::ToolUseConfirm { tool_name, default_decision, .. } => {
              assert_eq!(tool_name, "Bash");
              assert_eq!(default_decision, PromptDefault::DenyByDefault);
          }
          _ => panic!("wrong variant"),
      }
  }

  #[test]
  fn permission_request_enum_exit_plan_mode_variant() {
      let req = PermissionRequest::ExitPlanMode {
          plan: "1. Foo\n2. Bar".to_string(),
      };
      match req {
          PermissionRequest::ExitPlanMode { plan } => assert!(plan.contains("Foo")),
          _ => panic!("wrong variant"),
      }
  }

  #[test]
  fn permission_request_enum_bypass_permissions_variant() {
      let req = PermissionRequest::BypassPermissionsMode;
      matches!(req, PermissionRequest::BypassPermissionsMode);
  }

  #[test]
  fn permission_response_three_variants() {
      assert_eq!(PermissionResponse::AllowOnce, PermissionResponse::AllowOnce);
      assert_ne!(PermissionResponse::AllowOnce, PermissionResponse::AllowAlways);
      assert_ne!(PermissionResponse::AllowOnce, PermissionResponse::Deny);
  }
  ```

- [ ] **Step 3: Run tests to verify they fail.**

  Run: `cargo test -p lingxi-traits prompting_gate --lib 2>&1 | tail -20`
  Expected: compilation error — `PermissionRequest::ToolUseConfirm` doesn't exist, `PermissionResponse` doesn't exist.

- [ ] **Step 4: Replace the struct with the enum + add response.**

  In `lingxi-core/crates/traits/src/prompting_gate.rs`, REPLACE the existing `pub struct PermissionRequest { .. }` (lines 26-35 in the M5-05 file) with:
  ```rust
  /// A single permission prompt — three variants:
  /// - `ToolUseConfirm` is the M5-05 stdio-prompt case (preserved).
  /// - `ExitPlanMode` asks the user to approve a plan-mode exit.
  /// - `BypassPermissionsMode` asks the user to opt in to dangerous mode.
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub enum PermissionRequest {
      /// Tool-use confirmation — equivalent to the M5-05 struct.
      ToolUseConfirm {
          /// Canonical tool name (e.g. `"Bash"`, `"Agent"`).
          tool_name: String,
          /// The model's `tool_input` JSON.
          tool_input: serde_json::Value,
          /// Default decision when the user presses Enter only.
          default_decision: PromptDefault,
      },
      /// Plan-mode exit — user must approve a proposed plan.
      ExitPlanMode {
          /// Plan markdown body, rendered as a multi-line block.
          plan: String,
      },
      /// Dangerous-mode toggle — user must explicitly type `yes` to enable.
      BypassPermissionsMode,
  }

  /// Outcome of a dialog round-trip in the TUI (M6-05).
  ///
  /// Maps to `PermissionDecision` via `PermissionResponse::into_decision`:
  /// `AllowOnce` and `AllowAlways` both → `Allow`; `Deny` → `Deny { reason: "user denied" }`.
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum PermissionResponse {
      /// Allow this single tool call.
      AllowOnce,
      /// Allow this tool for the rest of the session (write a session rule).
      AllowAlways,
      /// Reject the tool call.
      Deny,
  }

  impl PermissionResponse {
      /// Whether the response should be persisted as a session rule.
      pub fn persist(self) -> bool {
          matches!(self, Self::AllowAlways)
      }
  }
  ```

- [ ] **Step 5: Run tests to verify they pass.**

  Run: `cargo test -p lingxi-traits prompting_gate --lib 2>&1 | tail -10`
  Expected: all tests pass (4 new + 4 existing from M5-05).

- [ ] **Step 6: Update permission re-export.**

  In `lingxi-core/crates/permission/src/gate.rs`, extend the existing `pub use lingxi_traits::prompting_gate::{..}` block to include `PermissionResponse`. Specifically, the existing line:
  ```rust
  pub use lingxi_traits::prompting_gate::{
      PermissionRequest, PromptDecision, PromptDefault, PromptError, PromptingGate,
  };
  ```
  becomes:
  ```rust
  pub use lingxi_traits::prompting_gate::{
      PermissionRequest, PermissionResponse, PromptDecision, PromptDefault, PromptError,
      PromptingGate,
  };
  ```

- [ ] **Step 7: Update permission lib.rs re-export.**

  In `lingxi-core/crates/permission/src/lib.rs`, add `PermissionResponse` to the existing re-export line (find the line that re-exports `PermissionDecision, PermissionGate, PermissionRequest, PromptDecision, PromptDefault, PromptError, PromptingGate` from `gate::*` and add `PermissionResponse` to it).

- [ ] **Step 8: Verify M5-05 stdio path still compiles.**

  Run: `cargo build -p lingxi-permission -p lingxi-orchestrator 2>&1 | tail -30`
  Expected: compilation FAILS because `InteractivePromptingGate::prompt_user` constructs `PermissionRequest { .. }` as a struct (M5-05 code) — but the type is now an enum. This failure is expected and is fixed in Task 2.

- [ ] **Step 9: Commit.**

  ```bash
  git add lingxi-core/crates/traits/src/prompting_gate.rs lingxi-core/crates/permission/src/gate.rs lingxi-core/crates/permission/src/lib.rs
  git commit -m "feat(m6-05 task 1): PermissionRequest enum + PermissionResponse"
  ```

---

### Task 2: Update M5-05 stdio gate to the new enum

**Files:**
- Modify: `lingxi-core/crates/permission/src/prompting_gate.rs`

**Steps:**

- [ ] **Step 1: Locate the M5-05 stdio gate.** Open `lingxi-core/crates/permission/src/prompting_gate.rs`. Find `impl InteractivePromptingGate` — the `prompt_user` method constructs and matches on `PermissionRequest`.

- [ ] **Step 2: Refactor `prompt_user` to pattern-match on the enum.**

  The current M5-05 method signature (per the precedent plan) is:
  ```rust
  async fn prompt_user(
      &self,
      request: &PermissionRequest,
  ) -> Result<PromptDecision, PromptError> {
      // M5-05 code accesses request.tool_name, request.tool_input, request.default_decision
      // …
  }
  ```

  Update to pattern-match. Replace the body so it dispatches on the enum:
  ```rust
  async fn prompt_user(
      &self,
      request: &PermissionRequest,
  ) -> Result<PromptDecision, PromptError> {
      match request {
          PermissionRequest::ToolUseConfirm {
              tool_name,
              tool_input,
              default_decision,
          } => {
              // [Existing M5-05 stdio prompt body — moved verbatim into this arm.
              // Touches `tool_name`, `tool_input`, `default_decision` instead of
              // `request.tool_name` etc.]
              self.prompt_tool_use(tool_name, tool_input, *default_decision).await
          }
          PermissionRequest::ExitPlanMode { .. } | PermissionRequest::BypassPermissionsMode => {
              // The M5-05 stdio gate cannot render multiline plan bodies or the
              // bypass warning — that's the TUI gate's job (Task 7). Return a
              // structured cancellation so the orchestrator falls through to
              // Deny + a clear reason.
              Err(PromptError::Cancelled {
                  reason: "stdio gate cannot render multiline permission dialogs"
                      .to_string(),
              })
          }
      }
  }
  ```

- [ ] **Step 3: Extract the body into `prompt_tool_use` helper.**

  Above the trait impl, add:
  ```rust
  impl InteractivePromptingGate {
      async fn prompt_tool_use(
          &self,
          tool_name: &str,
          _tool_input: &serde_json::Value,
          default_decision: PromptDefault,
      ) -> Result<PromptDecision, PromptError> {
          // [The exact M5-05 prompt body — format the line, write to stderr,
          // read from stdin, parse, retry up to 3 times. Move ALL lines from
          // the old `prompt_user` body here, replacing `request.tool_name`
          // with `tool_name`, `request.default_decision` with
          // `default_decision`, and `request.tool_input` with `_tool_input`.]
          //
          // The byte-locked prompt format from M5-05:
          //   "Claude needs your permission to use {tool_name}\n[Y/n] "  (AllowByDefault)
          //   "Claude needs your permission to use {tool_name}\n[y/N] "  (DenyByDefault)
          //   "Agent tool requires permission to spawn sub-agents.\n[Y/n] "  (Agent override)
          //
          // Tests in M5-05 `permission/tests/prompting_gate_format_test.rs`
          // and `permission/tests/prompting_gate_parse_test.rs` already cover
          // this path — Task 2 step 4 verifies they still pass after the
          // refactor.
          // …
      }
  }
  ```

  (The plan engineer fills in the body by literally moving the M5-05 lines from the old `prompt_user` into this helper — this is a mechanical refactor, no logic changes.)

- [ ] **Step 4: Run M5-05 tests to verify no regression.**

  Run: `cargo test -p lingxi-permission --tests 2>&1 | tail -20`
  Expected: all M5-05 tests pass (the format, parse, retry, and orchestrator interactive-perms tests). If any fail, the helper extraction has a bug — diff against the M5-05 source.

- [ ] **Step 5: Write a new test for the ExitPlanMode + Bypass arms.**

  In `lingxi-core/crates/permission/src/prompting_gate.rs`, append to `#[cfg(test)] mod tests`:
  ```rust
  #[tokio::test]
  async fn stdio_gate_returns_cancelled_for_exit_plan_mode() {
      use crate::gate::PermissionRequest;
      // Use the existing M5-05 test scaffolding to construct an InteractivePromptingGate.
      let gate = make_test_gate();  // helper from M5-05 tests
      let req = PermissionRequest::ExitPlanMode { plan: "x".to_string() };
      let err = gate.prompt_user(&req).await.unwrap_err();
      assert!(matches!(err, PromptError::Cancelled { .. }));
  }

  #[tokio::test]
  async fn stdio_gate_returns_cancelled_for_bypass_permissions() {
      use crate::gate::PermissionRequest;
      let gate = make_test_gate();
      let req = PermissionRequest::BypassPermissionsMode;
      let err = gate.prompt_user(&req).await.unwrap_err();
      assert!(matches!(err, PromptError::Cancelled { .. }));
  }
  ```

- [ ] **Step 6: Run the new tests.**

  Run: `cargo test -p lingxi-permission prompting_gate --lib 2>&1 | tail -10`
  Expected: both new tests pass.

- [ ] **Step 7: Commit.**

  ```bash
  git add lingxi-core/crates/permission/src/prompting_gate.rs
  git commit -m "refactor(m6-05 task 2): stdio gate pattern-matches PermissionRequest enum"
  ```

---

### Task 3: `ToolUseConfirm` dialog component + `handle_key` helper

**Files:**
- Create: `lingxi-core/crates/tui/src/components/permissions/mod.rs`
- Create: `lingxi-core/crates/tui/src/components/permissions/tool_use_confirm.rs`
- Modify: `lingxi-core/crates/tui/src/components/mod.rs` (add `pub mod permissions;`)

**Steps:**

- [ ] **Step 1: Add the module barrel.**

  Create `lingxi-core/crates/tui/src/components/permissions/mod.rs`:
  ```rust
  //! Permission modal dialogs — 3 variants per M6-05 plan.
  #![forbid(unsafe_code)]

  pub mod bypass_permissions;
  pub mod exit_plan_mode;
  pub mod tool_use_confirm;

  use lingxi_permission::gate::PermissionResponse;

  /// What the dialog produces when the user resolves it.
  ///
  /// Two-tuple: the response variant + whether the resolution carries a session
  /// scope (when `AllowAlways` is picked, the orchestrator persists a session
  /// rule). `Deny` always has `persist == false`.
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub struct DialogResolution {
      pub response: PermissionResponse,
      pub persist: bool,
  }

  impl DialogResolution {
      pub fn allow_once() -> Self {
          Self { response: PermissionResponse::AllowOnce, persist: false }
      }
      pub fn allow_always() -> Self {
          Self { response: PermissionResponse::AllowAlways, persist: true }
      }
      pub fn deny() -> Self {
          Self { response: PermissionResponse::Deny, persist: false }
      }
  }

  /// Which of the three buttons is currently highlighted (arrow-key state).
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum DialogFocus {
      AllowOnce,
      AllowAlways,
      Deny,
  }

  impl Default for DialogFocus {
      /// Per the M6-05 task brief: "Allow Once highlighted by default".
      fn default() -> Self { Self::AllowOnce }
  }

  impl DialogFocus {
      pub fn next(self) -> Self {
          match self {
              Self::AllowOnce => Self::AllowAlways,
              Self::AllowAlways => Self::Deny,
              Self::Deny => Self::AllowOnce,
          }
      }
      pub fn prev(self) -> Self {
          match self {
              Self::AllowOnce => Self::Deny,
              Self::AllowAlways => Self::AllowOnce,
              Self::Deny => Self::AllowAlways,
          }
      }
      pub fn resolve(self) -> DialogResolution {
          match self {
              Self::AllowOnce => DialogResolution::allow_once(),
              Self::AllowAlways => DialogResolution::allow_always(),
              Self::Deny => DialogResolution::deny(),
          }
      }
  }
  ```

- [ ] **Step 2: Add module reference.**

  In `lingxi-core/crates/tui/src/components/mod.rs`, append:
  ```rust
  pub mod permissions;
  ```

- [ ] **Step 3: Write the failing test for `handle_key`.**

  Create `lingxi-core/crates/tui/src/components/permissions/tool_use_confirm.rs` with:
  ```rust
  //! `ToolUseConfirm` dialog — generic per-tool permission prompt.
  #![forbid(unsafe_code)]

  // (production code added in step 4)

  #[cfg(test)]
  mod tests {
      use super::*;
      use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
      use lingxi_permission::gate::PermissionResponse;

      fn k(code: KeyCode) -> KeyEvent {
          KeyEvent::new(code, KeyModifiers::NONE)
      }

      #[test]
      fn key_1_returns_allow_once() {
          let mut state = ToolUseConfirmState::default();
          let res = handle_key(&mut state, k(KeyCode::Char('1')));
          assert_eq!(res.unwrap().response, PermissionResponse::AllowOnce);
      }

      #[test]
      fn key_2_returns_allow_always() {
          let mut state = ToolUseConfirmState::default();
          let res = handle_key(&mut state, k(KeyCode::Char('2')));
          assert_eq!(res.unwrap().response, PermissionResponse::AllowAlways);
          assert!(res.unwrap().persist);
      }

      #[test]
      fn key_lowercase_n_returns_deny() {
          let mut state = ToolUseConfirmState::default();
          let res = handle_key(&mut state, k(KeyCode::Char('n')));
          assert_eq!(res.unwrap().response, PermissionResponse::Deny);
      }

      #[test]
      fn key_uppercase_n_returns_deny() {
          let mut state = ToolUseConfirmState::default();
          let res = handle_key(&mut state, k(KeyCode::Char('N')));
          assert_eq!(res.unwrap().response, PermissionResponse::Deny);
      }

      #[test]
      fn key_esc_returns_deny() {
          let mut state = ToolUseConfirmState::default();
          let res = handle_key(&mut state, k(KeyCode::Esc));
          assert_eq!(res.unwrap().response, PermissionResponse::Deny);
      }

      #[test]
      fn key_enter_resolves_on_highlighted_button() {
          let mut state = ToolUseConfirmState::default();
          // Default focus = AllowOnce
          let res = handle_key(&mut state, k(KeyCode::Enter));
          assert_eq!(res.unwrap().response, PermissionResponse::AllowOnce);
      }

      #[test]
      fn key_down_arrow_advances_focus() {
          let mut state = ToolUseConfirmState::default();
          assert_eq!(state.focus, DialogFocus::AllowOnce);
          let res = handle_key(&mut state, k(KeyCode::Down));
          assert!(res.is_none());  // no resolution, just focus change
          assert_eq!(state.focus, DialogFocus::AllowAlways);
      }

      #[test]
      fn key_text_does_not_resolve() {
          let mut state = ToolUseConfirmState::default();
          let res = handle_key(&mut state, k(KeyCode::Char('x')));
          assert!(res.is_none());
      }
  }
  ```

- [ ] **Step 4: Run tests to verify they fail.**

  Run: `cargo test -p lingxi-tui tool_use_confirm --lib 2>&1 | tail -20`
  Expected: compilation failure — `handle_key`, `ToolUseConfirmState`, `DialogFocus` not yet defined in this module's scope.

- [ ] **Step 5: Implement production code.**

  Above `#[cfg(test)] mod tests` in the same file:
  ```rust
  use crossterm::event::{KeyCode, KeyEvent};
  use iocraft::prelude::*;

  use super::{DialogFocus, DialogResolution};

  /// Mutable state carried across renders for this dialog.
  #[derive(Debug, Clone, Default)]
  pub struct ToolUseConfirmState {
      pub focus: DialogFocus,
  }

  /// Props passed by the parent (REPL screen).
  #[derive(Default, Props)]
  pub struct ToolUseConfirmProps {
      pub tool_name: String,
      pub tool_input_pretty: String,
      pub focus: DialogFocus,
  }

  /// Pure key handler. Returns `Some(resolution)` when the user picks an option,
  /// `None` otherwise (focus moved, ignored key).
  pub fn handle_key(state: &mut ToolUseConfirmState, key: KeyEvent) -> Option<DialogResolution> {
      match key.code {
          KeyCode::Char('1') => Some(DialogResolution::allow_once()),
          KeyCode::Char('2') => Some(DialogResolution::allow_always()),
          KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => Some(DialogResolution::deny()),
          KeyCode::Enter => Some(state.focus.resolve()),
          KeyCode::Down | KeyCode::Tab => {
              state.focus = state.focus.next();
              None
          }
          KeyCode::Up | KeyCode::BackTab => {
              state.focus = state.focus.prev();
              None
          }
          _ => None,
      }
  }

  /// iocraft component rendering the dialog frame + 3 buttons.
  #[component]
  pub fn ToolUseConfirm(props: &ToolUseConfirmProps) -> impl Into<AnyElement<'static>> {
      let header = format!("Claude needs your permission to use {}", props.tool_name);
      let button_label = |focus: DialogFocus, label: &str| -> String {
          if focus == props.focus {
              format!("> {}", label)
          } else {
              format!("  {}", label)
          }
      };
      element! {
          Box(
              flex_direction: FlexDirection::Column,
              border_style: BorderStyle::Single,
              padding: 1,
          ) {
              Text(content: header.clone())
              Text(content: format!("Input: {}", props.tool_input_pretty))
              Box(flex_direction: FlexDirection::Column, padding_top: 1) {
                  Text(content: button_label(DialogFocus::AllowOnce, "[1] Allow Once"))
                  Text(content: button_label(DialogFocus::AllowAlways, "[2] Allow Always"))
                  Text(content: button_label(DialogFocus::Deny, "[N] Deny"))
              }
          }
      }
  }
  ```

  (Note: iocraft API names may need slight adjustment based on the exact version pinned in M6-01 — engineers should verify `BorderStyle::Single` and `FlexDirection::Column` against the cached `iocraft = "=0.6"` docs. If iocraft uses `border_color` or different prop names, use the M6-02 `StatusLine` precedent as a template.)

- [ ] **Step 6: Run tests to verify they pass.**

  Run: `cargo test -p lingxi-tui tool_use_confirm --lib 2>&1 | tail -10`
  Expected: 8 tests pass.

- [ ] **Step 7: Commit.**

  ```bash
  git add lingxi-core/crates/tui/src/components/permissions/ lingxi-core/crates/tui/src/components/mod.rs
  git commit -m "feat(m6-05 task 3): ToolUseConfirm dialog component + handle_key"
  ```

---

### Task 4: `ExitPlanMode` dialog component + `handle_key` helper

**Files:**
- Create: `lingxi-core/crates/tui/src/components/permissions/exit_plan_mode.rs`

**Steps:**

- [ ] **Step 1: Write the failing tests.**

  Create the file with `#[cfg(test)] mod tests` containing the exact same key-handling tests as Task 3 (with `ExitPlanModeState` substituted for `ToolUseConfirmState` and `exit_plan_mode::handle_key` for `tool_use_confirm::handle_key`). The 8 tests are byte-for-byte the same — the key bindings are identical between the two dialogs. Copy the test block from Task 3 step 3 verbatim, renaming the imports.

  In particular, include these tests:
  - `key_1_returns_allow_once`
  - `key_2_returns_allow_always`
  - `key_lowercase_n_returns_deny`
  - `key_uppercase_n_returns_deny`
  - `key_esc_returns_deny`
  - `key_enter_resolves_on_highlighted_button`
  - `key_down_arrow_advances_focus`
  - `key_text_does_not_resolve`

- [ ] **Step 2: Run tests to verify failure.**

  Run: `cargo test -p lingxi-tui exit_plan_mode --lib 2>&1 | tail -10`
  Expected: compilation failure (`ExitPlanModeState`, `handle_key` not defined).

- [ ] **Step 3: Implement production code.**

  Above `#[cfg(test)]`:
  ```rust
  //! `ExitPlanMode` dialog — user approves a plan-mode exit.
  #![forbid(unsafe_code)]

  use crossterm::event::{KeyCode, KeyEvent};
  use iocraft::prelude::*;

  use super::{DialogFocus, DialogResolution};

  #[derive(Debug, Clone, Default)]
  pub struct ExitPlanModeState {
      pub focus: DialogFocus,
  }

  #[derive(Default, Props)]
  pub struct ExitPlanModeProps {
      pub plan: String,
      pub focus: DialogFocus,
  }

  pub fn handle_key(state: &mut ExitPlanModeState, key: KeyEvent) -> Option<DialogResolution> {
      // Bit-identical handler to ToolUseConfirm (the dialog grammar is the same).
      match key.code {
          KeyCode::Char('1') => Some(DialogResolution::allow_once()),
          KeyCode::Char('2') => Some(DialogResolution::allow_always()),
          KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => Some(DialogResolution::deny()),
          KeyCode::Enter => Some(state.focus.resolve()),
          KeyCode::Down | KeyCode::Tab => {
              state.focus = state.focus.next();
              None
          }
          KeyCode::Up | KeyCode::BackTab => {
              state.focus = state.focus.prev();
              None
          }
          _ => None,
      }
  }

  #[component]
  pub fn ExitPlanMode(props: &ExitPlanModeProps) -> impl Into<AnyElement<'static>> {
      let header = "Claude Code needs your approval for the plan".to_string();
      let button_label = |focus: DialogFocus, label: &str| -> String {
          if focus == props.focus { format!("> {}", label) } else { format!("  {}", label) }
      };
      element! {
          Box(
              flex_direction: FlexDirection::Column,
              border_style: BorderStyle::Single,
              padding: 1,
          ) {
              Text(content: header)
              Box(flex_direction: FlexDirection::Column, padding_top: 1) {
                  Text(content: props.plan.clone())
              }
              Box(flex_direction: FlexDirection::Column, padding_top: 1) {
                  Text(content: button_label(DialogFocus::AllowOnce, "[1] Allow Once"))
                  Text(content: button_label(DialogFocus::AllowAlways, "[2] Allow Always"))
                  Text(content: button_label(DialogFocus::Deny, "[N] Deny"))
              }
          }
      }
  }
  ```

- [ ] **Step 4: Run tests to verify they pass.**

  Run: `cargo test -p lingxi-tui exit_plan_mode --lib 2>&1 | tail -10`
  Expected: 8 tests pass.

- [ ] **Step 5: Commit.**

  ```bash
  git add lingxi-core/crates/tui/src/components/permissions/exit_plan_mode.rs
  git commit -m "feat(m6-05 task 4): ExitPlanMode dialog component + handle_key"
  ```

---

### Task 5: `BypassPermissionsMode` dialog component (yes-typing state machine)

**Files:**
- Create: `lingxi-core/crates/tui/src/components/permissions/bypass_permissions.rs`

**Steps:**

- [ ] **Step 1: Write the failing tests.**

  Create the file with `#[cfg(test)] mod tests`:
  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;
      use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
      use lingxi_permission::gate::PermissionResponse;

      fn k(code: KeyCode) -> KeyEvent { KeyEvent::new(code, KeyModifiers::NONE) }
      fn kc(c: char) -> KeyEvent { k(KeyCode::Char(c)) }

      #[test]
      fn typed_yes_then_enter_returns_allow_once() {
          let mut state = BypassPermissionsState::default();
          assert!(handle_key(&mut state, kc('y')).is_none());
          assert_eq!(state.typed, "y");
          assert!(handle_key(&mut state, kc('e')).is_none());
          assert_eq!(state.typed, "ye");
          assert!(handle_key(&mut state, kc('s')).is_none());
          assert_eq!(state.typed, "yes");
          let res = handle_key(&mut state, k(KeyCode::Enter));
          assert_eq!(res.unwrap().response, PermissionResponse::AllowOnce);
      }

      #[test]
      fn enter_without_typed_yes_does_not_resolve() {
          let mut state = BypassPermissionsState::default();
          let res = handle_key(&mut state, k(KeyCode::Enter));
          assert!(res.is_none());
      }

      #[test]
      fn esc_returns_deny_even_with_partial_input() {
          let mut state = BypassPermissionsState::default();
          handle_key(&mut state, kc('y'));
          let res = handle_key(&mut state, k(KeyCode::Esc));
          assert_eq!(res.unwrap().response, PermissionResponse::Deny);
      }

      #[test]
      fn key_uppercase_n_returns_deny() {
          let mut state = BypassPermissionsState::default();
          let res = handle_key(&mut state, kc('N'));
          assert_eq!(res.unwrap().response, PermissionResponse::Deny);
      }

      #[test]
      fn typing_wrong_letters_does_not_resolve() {
          let mut state = BypassPermissionsState::default();
          handle_key(&mut state, kc('y'));
          handle_key(&mut state, kc('o'));  // wrong — expected 'e'
          // Buffer keeps the wrong letter; user can backspace.
          assert_eq!(state.typed, "yo");
          let res = handle_key(&mut state, k(KeyCode::Enter));
          assert!(res.is_none());
      }

      #[test]
      fn backspace_pops_typed_buffer() {
          let mut state = BypassPermissionsState::default();
          handle_key(&mut state, kc('y'));
          handle_key(&mut state, kc('e'));
          handle_key(&mut state, k(KeyCode::Backspace));
          assert_eq!(state.typed, "y");
      }

      #[test]
      fn typed_is_case_insensitive() {
          let mut state = BypassPermissionsState::default();
          handle_key(&mut state, kc('Y'));
          handle_key(&mut state, kc('E'));
          handle_key(&mut state, kc('S'));
          let res = handle_key(&mut state, k(KeyCode::Enter));
          assert_eq!(res.unwrap().response, PermissionResponse::AllowOnce);
      }
  }
  ```

- [ ] **Step 2: Run tests to verify failure.**

  Run: `cargo test -p lingxi-tui bypass_permissions --lib 2>&1 | tail -10`
  Expected: compilation failure.

- [ ] **Step 3: Implement production code.**

  Above `#[cfg(test)]`:
  ```rust
  //! `BypassPermissionsMode` dialog — user must type `yes` to enable.
  #![forbid(unsafe_code)]

  use crossterm::event::{KeyCode, KeyEvent};
  use iocraft::prelude::*;

  use super::DialogResolution;

  /// State carried across renders.
  #[derive(Debug, Clone, Default)]
  pub struct BypassPermissionsState {
      /// Letters typed so far (lowercased internally). When `typed == "yes"`,
      /// Enter resolves to `AllowOnce`.
      pub typed: String,
  }

  #[derive(Default, Props)]
  pub struct BypassPermissionsProps {
      pub typed: String,
  }

  /// Pure key handler.
  ///
  /// The dialog has two completion paths:
  /// 1. User types `yes` (case-insensitive) then Enter → `AllowOnce`.
  /// 2. User presses Esc or `N`/`n` → `Deny`.
  ///
  /// Backspace pops the last char. Wrong letters are accepted into the buffer
  /// so the user sees the typo and can correct it (this matches typical CLI
  /// confirm-by-typing patterns).
  pub fn handle_key(
      state: &mut BypassPermissionsState,
      key: KeyEvent,
  ) -> Option<DialogResolution> {
      match key.code {
          KeyCode::Esc => Some(DialogResolution::deny()),
          KeyCode::Char('n') | KeyCode::Char('N') => Some(DialogResolution::deny()),
          KeyCode::Enter => {
              if state.typed.to_ascii_lowercase() == "yes" {
                  Some(DialogResolution::allow_once())
              } else {
                  None
              }
          }
          KeyCode::Backspace => {
              state.typed.pop();
              None
          }
          KeyCode::Char(c) => {
              // Only letters; ignore digits/punctuation.
              if c.is_ascii_alphabetic() {
                  state.typed.push(c.to_ascii_lowercase());
              }
              None
          }
          _ => None,
      }
  }

  #[component]
  pub fn BypassPermissionsMode(
      props: &BypassPermissionsProps,
  ) -> impl Into<AnyElement<'static>> {
      // Byte-locked literals from claude-code/src/components/BypassPermissionsModeDialog.tsx:53,73.
      let title = "WARNING: Claude Code running in Bypass Permissions mode";
      let body1 = "In Bypass Permissions mode, Claude Code will not ask for your approval before running potentially dangerous commands.\nThis mode should only be used in a sandboxed container/VM that has restricted internet access and can easily be restored if damaged.";
      let body2 = "By proceeding, you accept all responsibility for actions taken while running in Bypass Permissions mode.";
      let prompt_line = format!("Type \"yes\" + Enter to enable, Esc to cancel: {}", props.typed);
      element! {
          Box(
              flex_direction: FlexDirection::Column,
              border_style: BorderStyle::Single,
              padding: 1,
          ) {
              Text(content: title.to_string())
              Box(flex_direction: FlexDirection::Column, padding_top: 1) {
                  Text(content: body1.to_string())
                  Text(content: body2.to_string())
              }
              Box(flex_direction: FlexDirection::Column, padding_top: 1) {
                  Text(content: prompt_line)
              }
          }
      }
  }
  ```

  Note: claude-code's React component takes "Yes, I accept" / "No, exit" through a Select widget. LingXi diverges by using a typed-`yes` confirmation. The two body strings ARE byte-locked from claude-code; the prompt line ("Type ...") is LingXi-locked. Both are tracked in the M6-09 literal-lock list.

- [ ] **Step 4: Run tests to verify they pass.**

  Run: `cargo test -p lingxi-tui bypass_permissions --lib 2>&1 | tail -10`
  Expected: 7 tests pass.

- [ ] **Step 5: Commit.**

  ```bash
  git add lingxi-core/crates/tui/src/components/permissions/bypass_permissions.rs
  git commit -m "feat(m6-05 task 5): BypassPermissionsMode dialog component"
  ```

---

### Task 6: Snapshot tests for all 3 dialogs (insta)

**Files:**
- Create: `lingxi-core/crates/tui/src/components/permissions/tests/snapshot_tool_use_confirm.rs`
- Create: `lingxi-core/crates/tui/src/components/permissions/tests/snapshot_exit_plan_mode.rs`
- Create: `lingxi-core/crates/tui/src/components/permissions/tests/snapshot_bypass_permissions.rs`
- Modify: `lingxi-core/crates/tui/src/components/permissions/mod.rs` (add `#[cfg(test)] pub mod tests;` if iocraft test harness exposed via subdir)

**Steps:**

- [ ] **Step 1: Verify iocraft has a render-to-string helper.**

  Open the M6-02 `crates/tui/tests/render_status_line.rs` (which lands snapshot scaffolding in M6-02). Confirm the pattern — typically:
  ```rust
  use iocraft::testing::render_element_to_string;
  let frame = render_element_to_string(element! { StatusLine(/*..*/) });
  insta::assert_snapshot!(frame);
  ```

  If iocraft's helper is named differently, adapt accordingly. The exact API is locked at M6-02 — Task 6 follows the precedent.

- [ ] **Step 2: Write the `ToolUseConfirm` snapshot.**

  Create `lingxi-core/crates/tui/src/components/permissions/tests/snapshot_tool_use_confirm.rs`:
  ```rust
  use iocraft::testing::render_element_to_string;
  use iocraft::prelude::*;
  use lingxi_tui::components::permissions::{
      tool_use_confirm::{ToolUseConfirm, ToolUseConfirmProps},
      DialogFocus,
  };

  #[test]
  fn snapshot_tool_use_confirm_default_state() {
      let frame = render_element_to_string(element! {
          ToolUseConfirm(
              tool_name: "Bash".to_string(),
              tool_input_pretty: "{\"command\":\"ls -la\"}".to_string(),
              focus: DialogFocus::AllowOnce,
          )
      });
      insta::assert_snapshot!(frame);
  }
  ```

- [ ] **Step 3: Write the `ExitPlanMode` snapshot.**

  Create `lingxi-core/crates/tui/src/components/permissions/tests/snapshot_exit_plan_mode.rs`:
  ```rust
  use iocraft::testing::render_element_to_string;
  use iocraft::prelude::*;
  use lingxi_tui::components::permissions::{
      exit_plan_mode::{ExitPlanMode, ExitPlanModeProps},
      DialogFocus,
  };

  #[test]
  fn snapshot_exit_plan_mode_with_5_line_plan() {
      let plan = "1. Read foo.rs\n2. Refactor bar()\n3. Add tests\n4. Run cargo test\n5. Commit"
          .to_string();
      let frame = render_element_to_string(element! {
          ExitPlanMode(plan: plan, focus: DialogFocus::AllowOnce)
      });
      insta::assert_snapshot!(frame);
  }
  ```

- [ ] **Step 4: Write the `BypassPermissionsMode` snapshot.**

  Create `lingxi-core/crates/tui/src/components/permissions/tests/snapshot_bypass_permissions.rs`:
  ```rust
  use iocraft::testing::render_element_to_string;
  use iocraft::prelude::*;
  use lingxi_tui::components::permissions::bypass_permissions::{
      BypassPermissionsMode, BypassPermissionsProps,
  };

  #[test]
  fn snapshot_bypass_permissions_empty_typed() {
      let frame = render_element_to_string(element! {
          BypassPermissionsMode(typed: String::new())
      });
      insta::assert_snapshot!(frame);
  }

  #[test]
  fn snapshot_bypass_permissions_partial_typed() {
      let frame = render_element_to_string(element! {
          BypassPermissionsMode(typed: "ye".to_string())
      });
      insta::assert_snapshot!(frame);
  }
  ```

- [ ] **Step 5: Run snapshots in review mode.**

  Run: `cargo insta test -p lingxi-tui --accept 2>&1 | tail -20`
  Expected: 4 new `.snap` files created under `crates/tui/tests/snapshots/`. (If `cargo insta` is not installed, run `cargo install cargo-insta` first.)

- [ ] **Step 6: Manually inspect the snapshots.**

  Open each `.snap` file. Confirm:
  - The `ToolUseConfirm` snapshot contains `Claude needs your permission to use Bash`, `> [1] Allow Once`, `  [2] Allow Always`, `  [N] Deny`.
  - The `ExitPlanMode` snapshot contains `Claude Code needs your approval for the plan` + all 5 plan lines + the 3 buttons.
  - The `BypassPermissionsMode` snapshots both contain the WARNING title + both body literals + the prompt line.

  If a snapshot diverges, FIX the production code (Tasks 3-5) rather than accepting wrong output.

- [ ] **Step 7: Re-run to verify they pass without `--accept`.**

  Run: `cargo test -p lingxi-tui --tests 2>&1 | tail -10`
  Expected: all snapshot tests pass.

- [ ] **Step 8: Commit.**

  ```bash
  git add lingxi-core/crates/tui/src/components/permissions/tests/ lingxi-core/crates/tui/tests/snapshots/
  git commit -m "test(m6-05 task 6): snapshot tests for 3 permission dialogs"
  ```

---

### Task 7: Orchestrator → TUI bridge — `TuiPermissionGate` + `PermissionExchange`

**Files:**
- Modify: `lingxi-core/crates/tui/src/events/mod.rs`
- Modify: `lingxi-core/crates/orchestrator/src/handle_impl.rs` (or `conversation.rs` — depending on where the M5-05 gate was wired)
- Modify: `lingxi-core/crates/orchestrator/src/conversation.rs`
- Modify: `lingxi-core/crates/tui/src/app.rs`
- Modify: `lingxi-core/crates/tui/src/lib.rs` (if exposing `TuiPermissionGate` to the cli crate)

**Steps:**

- [ ] **Step 1: Define `PermissionExchange` type.**

  In `lingxi-core/crates/orchestrator/src/handle_impl.rs`, add at the top:
  ```rust
  use lingxi_permission::gate::{PermissionRequest, PermissionResponse};
  use tokio::sync::{mpsc, oneshot, Mutex};

  /// One in-flight permission round-trip between the orchestrator and the TUI.
  /// Constructed by `TuiPermissionGate::check` and sent over the mpsc to the TUI.
  /// The TUI fills `resp_tx` when the user resolves the dialog.
  #[derive(Debug)]
  pub struct PermissionExchange {
      pub request: PermissionRequest,
      pub resp_tx: oneshot::Sender<PermissionResponse>,
  }
  ```

- [ ] **Step 2: Add `TuiEvent::OrchestratorPermissionRequest` variant.**

  In `lingxi-core/crates/tui/src/events/mod.rs`, find the existing `TuiEvent` enum (M6-01 lands this). Add a new variant:
  ```rust
  use lingxi_orchestrator::handle_impl::PermissionExchange;

  #[derive(Debug)]
  pub enum TuiEvent {
      // … existing variants from M6-01..04 (Key, Resize, Tick, OrchestratorMessage, etc.)
      OrchestratorPermissionRequest(PermissionExchange),
  }
  ```

  If `lingxi-tui` doesn't yet depend on `lingxi-orchestrator`, add it to `crates/tui/Cargo.toml`:
  ```toml
  lingxi-orchestrator = { workspace = true }
  lingxi-permission = { workspace = true }
  ```

- [ ] **Step 3: Write a failing test for `TuiPermissionGate::check`.**

  Create or append to `lingxi-core/crates/orchestrator/src/handle_impl.rs::tests`:
  ```rust
  #[cfg(test)]
  mod tui_permission_gate_tests {
      use super::*;
      use lingxi_traits::permission_gate::{PermissionDecision, PermissionGate};
      use serde_json::json;

      #[tokio::test]
      async fn tui_gate_sends_request_and_receives_response() {
          let (event_tx, mut event_rx) = mpsc::channel::<PermissionExchange>(4);
          let rules: Arc<Mutex<Vec<lingxi_permission::PermissionRule>>> =
              Arc::new(Mutex::new(Vec::new()));
          let gate = TuiPermissionGate {
              event_tx: event_tx.clone(),
              session_allow_rules: rules.clone(),
          };

          // Spawn a "TUI" that responds AllowOnce to whatever comes in.
          let tui_task = tokio::spawn(async move {
              let ex = event_rx.recv().await.unwrap();
              let _ = ex.resp_tx.send(PermissionResponse::AllowOnce);
          });

          let decision = gate.check("Bash", &json!({"command": "ls"})).await;
          assert_eq!(decision, PermissionDecision::Allow);
          tui_task.await.unwrap();
      }

      #[tokio::test]
      async fn tui_gate_skips_dialog_when_session_rule_matches() {
          let (event_tx, mut event_rx) = mpsc::channel::<PermissionExchange>(4);
          let rules = Arc::new(Mutex::new(vec![
              lingxi_permission::PermissionRule::allow_tool("Bash"),
          ]));
          let gate = TuiPermissionGate {
              event_tx: event_tx.clone(),
              session_allow_rules: rules.clone(),
          };

          let decision = gate.check("Bash", &json!({"command": "ls"})).await;
          assert_eq!(decision, PermissionDecision::Allow);
          // No event should have been sent.
          assert!(event_rx.try_recv().is_err());
      }

      #[tokio::test]
      async fn tui_gate_persists_allow_always_into_session_rules() {
          let (event_tx, mut event_rx) = mpsc::channel::<PermissionExchange>(4);
          let rules = Arc::new(Mutex::new(Vec::new()));
          let gate = TuiPermissionGate {
              event_tx: event_tx.clone(),
              session_allow_rules: rules.clone(),
          };
          let tui_task = tokio::spawn(async move {
              let ex = event_rx.recv().await.unwrap();
              let _ = ex.resp_tx.send(PermissionResponse::AllowAlways);
          });
          let _ = gate.check("Bash", &json!({})).await;
          tui_task.await.unwrap();
          assert_eq!(rules.lock().await.len(), 1);
      }
  }
  ```

- [ ] **Step 4: Run tests to verify failure.**

  Run: `cargo test -p lingxi-orchestrator tui_permission_gate_tests 2>&1 | tail -20`
  Expected: compilation failure — `TuiPermissionGate`, `PermissionRule::allow_tool` not yet defined.

- [ ] **Step 5: Implement `TuiPermissionGate`.**

  Add to `lingxi-core/crates/orchestrator/src/handle_impl.rs`:
  ```rust
  use async_trait::async_trait;
  use lingxi_traits::permission_gate::{PermissionDecision, PermissionGate};

  /// Orchestrator-side permission gate that forwards requests to the TUI.
  ///
  /// On `check()`:
  /// 1. Look up `session_allow_rules` — if the tool has an Allow rule, skip dialog.
  /// 2. Otherwise, build a `PermissionRequest::ToolUseConfirm` and send over `event_tx`.
  /// 3. Await the oneshot reply.
  /// 4. If `AllowAlways`, push a new rule into `session_allow_rules`.
  pub struct TuiPermissionGate {
      pub event_tx: mpsc::Sender<PermissionExchange>,
      pub session_allow_rules: Arc<Mutex<Vec<lingxi_permission::PermissionRule>>>,
  }

  #[async_trait]
  impl PermissionGate for TuiPermissionGate {
      async fn check(&self, name: &str, input: &serde_json::Value) -> PermissionDecision {
          // Step 1: consult session rules.
          {
              let rules = self.session_allow_rules.lock().await;
              if rules.iter().any(|r| r.matches_tool(name)) {
                  return PermissionDecision::Allow;
              }
          }

          // Step 2: build request.
          let default_decision = lingxi_permission::defaults_per_tool::tool_default(name);
          let req = PermissionRequest::ToolUseConfirm {
              tool_name: name.to_string(),
              tool_input: input.clone(),
              default_decision,
          };

          // Step 3: send + await.
          let (tx, rx) = oneshot::channel();
          let ex = PermissionExchange { request: req, resp_tx: tx };
          if self.event_tx.send(ex).await.is_err() {
              // TUI is gone — fail closed.
              return PermissionDecision::Deny {
                  reason: "TUI permission bridge closed".to_string(),
              };
          }
          let resp = match rx.await {
              Ok(r) => r,
              Err(_) => return PermissionDecision::Deny {
                  reason: "TUI permission response dropped".to_string(),
              },
          };

          // Step 4: persist if AllowAlways.
          if matches!(resp, PermissionResponse::AllowAlways) {
              self.session_allow_rules
                  .lock()
                  .await
                  .push(lingxi_permission::PermissionRule::allow_tool(name));
          }

          match resp {
              PermissionResponse::AllowOnce | PermissionResponse::AllowAlways => {
                  PermissionDecision::Allow
              }
              PermissionResponse::Deny => PermissionDecision::Deny {
                  reason: "user denied via dialog".to_string(),
              },
          }
      }
  }
  ```

  If `lingxi_permission::PermissionRule::allow_tool` and `matches_tool` don't yet exist, ADD them as 5-line helpers in `crates/permission/src/lib.rs` (the `PermissionRule` type was added by M1.3). The helpers:
  ```rust
  // In lingxi-permission src — under the PermissionRule impl block.
  impl PermissionRule {
      pub fn allow_tool(tool: &str) -> Self {
          Self { tool_name: tool.to_string(), behavior: PermissionBehavior::Allow, /* … */ }
      }
      pub fn matches_tool(&self, tool: &str) -> bool {
          self.tool_name == tool && self.behavior == PermissionBehavior::Allow
      }
  }
  ```
  (Adapt to the existing `PermissionRule` field names — the engineer reads the M1.3 type definition first.)

- [ ] **Step 6: Add `AppState` fields.**

  In `lingxi-core/crates/tui/src/app.rs`, find the `AppState` struct (M6-02). Add:
  ```rust
  pub pending_permission: Option<PermissionRequest>,
  pub pending_permission_resp_tx: Option<oneshot::Sender<PermissionResponse>>,
  pub pending_permission_started_at: Option<std::time::Instant>,
  ```
  Plus:
  ```rust
  // Per-dialog state.
  pub tool_use_dialog_state:
      crate::components::permissions::tool_use_confirm::ToolUseConfirmState,
  pub exit_plan_dialog_state:
      crate::components::permissions::exit_plan_mode::ExitPlanModeState,
  pub bypass_dialog_state:
      crate::components::permissions::bypass_permissions::BypassPermissionsState,
  ```
  And in the `AppState::default()` impl (or wherever default values are constructed), initialize all three to `Default::default()` and the `pending_*` fields to `None`.

- [ ] **Step 7: Wire `OrchestratorPermissionRequest` event into AppState.**

  In `lingxi-core/crates/tui/src/app.rs` (or wherever the M6-02 event-loop dispatch lives), add a match arm for `TuiEvent::OrchestratorPermissionRequest(ex)`:
  ```rust
  TuiEvent::OrchestratorPermissionRequest(ex) => {
      state.pending_permission = Some(ex.request.clone());
      state.pending_permission_resp_tx = Some(ex.resp_tx);
      state.pending_permission_started_at = Some(std::time::Instant::now());
      // Reset per-dialog state.
      state.tool_use_dialog_state = Default::default();
      state.exit_plan_dialog_state = Default::default();
      state.bypass_dialog_state = Default::default();
      // Telemetry.
      let kind = match &ex.request {
          PermissionRequest::ToolUseConfirm { .. } => "tool_use",
          PermissionRequest::ExitPlanMode { .. } => "exit_plan_mode",
          PermissionRequest::BypassPermissionsMode => "bypass_permissions",
      };
      crate::telemetry::permission_dialog_shown(kind);
  }
  ```

  (`crate::telemetry::permission_dialog_shown` is implemented in Task 13.)

- [ ] **Step 8: Run tests.**

  Run: `cargo test -p lingxi-orchestrator tui_permission_gate_tests 2>&1 | tail -10`
  Expected: 3 tests pass.

- [ ] **Step 9: Commit.**

  ```bash
  git add lingxi-core/crates/orchestrator/src/handle_impl.rs lingxi-core/crates/orchestrator/Cargo.toml lingxi-core/crates/tui/src/events/mod.rs lingxi-core/crates/tui/src/app.rs lingxi-core/crates/tui/Cargo.toml lingxi-core/crates/permission/src/lib.rs
  git commit -m "feat(m6-05 task 7): TuiPermissionGate + orchestrator-TUI bridge"
  ```

---

### Task 8: Focus-trap keymap + REPL screen overlay

**Files:**
- Modify: `lingxi-core/crates/tui/src/events/keymap.rs`
- Modify: `lingxi-core/crates/tui/src/screens/repl.rs`

**Steps:**

- [ ] **Step 1: Write failing focus-trap behavior test.**

  Create `lingxi-core/crates/tui/tests/focus_trap_test.rs`:
  ```rust
  use lingxi_tui::app::AppState;
  use lingxi_tui::events::keymap::handle_key;
  use lingxi_permission::gate::PermissionRequest;
  use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
  use serde_json::json;

  fn k(c: char) -> KeyEvent {
      KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
  }

  #[test]
  fn dialog_open_prompt_input_not_mutated() {
      let mut state = AppState::default();
      state.prompt_text = "hello".to_string();
      state.pending_permission = Some(PermissionRequest::ToolUseConfirm {
          tool_name: "Bash".to_string(),
          tool_input: json!({}),
          default_decision: lingxi_permission::gate::PromptDefault::DenyByDefault,
      });
      // user types 'h' — must NOT append to prompt_text.
      handle_key(&mut state, k('h'));
      assert_eq!(state.prompt_text, "hello");
  }

  #[test]
  fn no_dialog_open_prompt_input_accepts_keys() {
      let mut state = AppState::default();
      state.prompt_text = "hel".to_string();
      handle_key(&mut state, k('l'));
      assert_eq!(state.prompt_text, "hell");
  }
  ```

- [ ] **Step 2: Run test to verify it fails.**

  Run: `cargo test -p lingxi-tui --test focus_trap_test 2>&1 | tail -10`
  Expected: compilation error OR the test fails because the keymap has no focus-trap branch.

- [ ] **Step 3: Implement focus-trap in `keymap.rs`.**

  Open `lingxi-core/crates/tui/src/events/keymap.rs`. At the TOP of `handle_key`, before any other dispatch, add:
  ```rust
  pub fn handle_key(state: &mut AppState, key: KeyEvent) {
      // === FOCUS TRAP (M6-05) ===
      // When a permission dialog is open, ALL keys route to its handler.
      // PromptInput / scrollback / slash-command keys are inert.
      if let Some(req) = state.pending_permission.clone() {
          let resolution = match req {
              PermissionRequest::ToolUseConfirm { .. } => {
                  crate::components::permissions::tool_use_confirm::handle_key(
                      &mut state.tool_use_dialog_state,
                      key,
                  )
              }
              PermissionRequest::ExitPlanMode { .. } => {
                  crate::components::permissions::exit_plan_mode::handle_key(
                      &mut state.exit_plan_dialog_state,
                      key,
                  )
              }
              PermissionRequest::BypassPermissionsMode => {
                  crate::components::permissions::bypass_permissions::handle_key(
                      &mut state.bypass_dialog_state,
                      key,
                  )
              }
          };
          if let Some(resolution) = resolution {
              resolve_pending_permission(state, resolution);
          }
          return;
      }
      // === end focus trap ===

      // (existing M6-02..04 key handling continues below)
      // …
  }

  fn resolve_pending_permission(
      state: &mut AppState,
      resolution: crate::components::permissions::DialogResolution,
  ) {
      let kind = match &state.pending_permission {
          Some(PermissionRequest::ToolUseConfirm { .. }) => "tool_use",
          Some(PermissionRequest::ExitPlanMode { .. }) => "exit_plan_mode",
          Some(PermissionRequest::BypassPermissionsMode) => "bypass_permissions",
          None => return,
      };
      let elapsed_ms = state
          .pending_permission_started_at
          .map(|t| t.elapsed().as_millis() as u64)
          .unwrap_or(0);
      crate::telemetry::permission_dialog_resolved(
          kind, resolution.response, resolution.persist, elapsed_ms,
      );
      if let Some(tx) = state.pending_permission_resp_tx.take() {
          let _ = tx.send(resolution.response);
      }
      state.pending_permission = None;
      state.pending_permission_started_at = None;
  }
  ```

- [ ] **Step 4: Add the REPL screen overlay.**

  In `lingxi-core/crates/tui/src/screens/repl.rs`, find the existing render layout (M6-02 lands the 3-zone layout). Add an overlay branch:
  ```rust
  use crate::components::permissions::{
      bypass_permissions::{BypassPermissionsMode, BypassPermissionsProps},
      exit_plan_mode::{ExitPlanMode, ExitPlanModeProps},
      tool_use_confirm::{ToolUseConfirm, ToolUseConfirmProps},
      DialogFocus,
  };
  use lingxi_permission::gate::PermissionRequest;

  // Inside the Repl component render block:
  let dialog: Option<AnyElement<'static>> = match &state.pending_permission {
      Some(PermissionRequest::ToolUseConfirm {
          tool_name, tool_input, ..
      }) => Some(element! {
          ToolUseConfirm(
              tool_name: tool_name.clone(),
              tool_input_pretty: serde_json::to_string_pretty(tool_input)
                  .unwrap_or_default(),
              focus: state.tool_use_dialog_state.focus,
          )
      }.into()),
      Some(PermissionRequest::ExitPlanMode { plan }) => Some(element! {
          ExitPlanMode(
              plan: plan.clone(),
              focus: state.exit_plan_dialog_state.focus,
          )
      }.into()),
      Some(PermissionRequest::BypassPermissionsMode) => Some(element! {
          BypassPermissionsMode(typed: state.bypass_dialog_state.typed.clone())
      }.into()),
      None => None,
  };

  // Wrap the existing layout (StatusLine + Scrollback + PromptInput) and overlay the dialog:
  element! {
      Box(flex_direction: FlexDirection::Column) {
          // … existing layout …
          #(dialog)
      }
  }
  ```

  (Iocraft's z-order or "absolute positioning" pattern depends on the exact 0.6 API. If iocraft 0.6 doesn't support overlay directly, fall back to rendering the dialog INSTEAD OF the 3-zone layout when `pending_permission.is_some()` — the dialog becomes the entire screen. Document the chosen approach in Task 8 step 5's commit message.)

- [ ] **Step 5: Run tests to verify they pass.**

  Run: `cargo test -p lingxi-tui --test focus_trap_test 2>&1 | tail -10`
  Expected: both tests pass.

- [ ] **Step 6: Commit.**

  ```bash
  git add lingxi-core/crates/tui/src/events/keymap.rs lingxi-core/crates/tui/src/screens/repl.rs lingxi-core/crates/tui/tests/focus_trap_test.rs
  git commit -m "feat(m6-05 task 8): focus-trap keymap + REPL dialog overlay"
  ```

---

### Task 9: Behavior test — `1` resolves to `AllowOnce` end-to-end

**Files:**
- Create: `lingxi-core/crates/tui/tests/behavior_permission_dialogs.rs`

**Steps:**

- [ ] **Step 1: Write the failing behavior test.**

  ```rust
  // crates/tui/tests/behavior_permission_dialogs.rs
  use lingxi_tui::app::AppState;
  use lingxi_tui::events::keymap::handle_key;
  use lingxi_permission::gate::{
      PermissionRequest, PermissionResponse, PromptDefault,
  };
  use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
  use serde_json::json;
  use tokio::sync::oneshot;

  fn k(code: KeyCode) -> KeyEvent { KeyEvent::new(code, KeyModifiers::NONE) }

  fn open_tool_use_dialog(state: &mut AppState) -> oneshot::Receiver<PermissionResponse> {
      let (tx, rx) = oneshot::channel();
      state.pending_permission = Some(PermissionRequest::ToolUseConfirm {
          tool_name: "Bash".to_string(),
          tool_input: json!({"command": "ls"}),
          default_decision: PromptDefault::DenyByDefault,
      });
      state.pending_permission_resp_tx = Some(tx);
      state.pending_permission_started_at = Some(std::time::Instant::now());
      rx
  }

  #[tokio::test]
  async fn feed_1_sends_allow_once() {
      let mut state = AppState::default();
      let rx = open_tool_use_dialog(&mut state);
      handle_key(&mut state, k(KeyCode::Char('1')));
      let resp = rx.await.unwrap();
      assert_eq!(resp, PermissionResponse::AllowOnce);
      assert!(state.pending_permission.is_none());
  }
  ```

- [ ] **Step 2: Run test to verify it passes.**

  Run: `cargo test -p lingxi-tui --test behavior_permission_dialogs feed_1_sends_allow_once 2>&1 | tail -10`
  Expected: PASS (Tasks 7-8 already wired the resolve path).

- [ ] **Step 3: Commit.**

  ```bash
  git add lingxi-core/crates/tui/tests/behavior_permission_dialogs.rs
  git commit -m "test(m6-05 task 9): behavior test — 1 resolves to AllowOnce"
  ```

---

### Task 10: Behavior tests — `2` → AllowAlways, `N`/`n` → Deny, `Esc` → Deny

**Files:**
- Modify: `lingxi-core/crates/tui/tests/behavior_permission_dialogs.rs`

**Steps:**

- [ ] **Step 1: Append failing tests.**

  In the same file:
  ```rust
  #[tokio::test]
  async fn feed_2_sends_allow_always() {
      let mut state = AppState::default();
      let rx = open_tool_use_dialog(&mut state);
      handle_key(&mut state, k(KeyCode::Char('2')));
      let resp = rx.await.unwrap();
      assert_eq!(resp, PermissionResponse::AllowAlways);
  }

  #[tokio::test]
  async fn feed_lowercase_n_sends_deny() {
      let mut state = AppState::default();
      let rx = open_tool_use_dialog(&mut state);
      handle_key(&mut state, k(KeyCode::Char('n')));
      let resp = rx.await.unwrap();
      assert_eq!(resp, PermissionResponse::Deny);
  }

  #[tokio::test]
  async fn feed_uppercase_n_sends_deny() {
      let mut state = AppState::default();
      let rx = open_tool_use_dialog(&mut state);
      handle_key(&mut state, k(KeyCode::Char('N')));
      let resp = rx.await.unwrap();
      assert_eq!(resp, PermissionResponse::Deny);
  }

  #[tokio::test]
  async fn feed_esc_sends_deny() {
      let mut state = AppState::default();
      let rx = open_tool_use_dialog(&mut state);
      handle_key(&mut state, k(KeyCode::Esc));
      let resp = rx.await.unwrap();
      assert_eq!(resp, PermissionResponse::Deny);
  }
  ```

- [ ] **Step 2: Run tests.**

  Run: `cargo test -p lingxi-tui --test behavior_permission_dialogs 2>&1 | tail -20`
  Expected: all 5 behavior tests pass (the new 4 + the one from Task 9).

- [ ] **Step 3: Commit.**

  ```bash
  git add lingxi-core/crates/tui/tests/behavior_permission_dialogs.rs
  git commit -m "test(m6-05 task 10): behavior tests — 2/n/N/Esc resolutions"
  ```

---

### Task 11: Behavior test — BypassPermissions requires typed `yes`

**Files:**
- Modify: `lingxi-core/crates/tui/tests/behavior_permission_dialogs.rs`

**Steps:**

- [ ] **Step 1: Append failing test.**

  In the same file:
  ```rust
  fn open_bypass_dialog(state: &mut AppState) -> oneshot::Receiver<PermissionResponse> {
      let (tx, rx) = oneshot::channel();
      state.pending_permission = Some(PermissionRequest::BypassPermissionsMode);
      state.pending_permission_resp_tx = Some(tx);
      state.pending_permission_started_at = Some(std::time::Instant::now());
      rx
  }

  #[tokio::test]
  async fn bypass_requires_typed_yes_then_enter() {
      let mut state = AppState::default();
      let rx = open_bypass_dialog(&mut state);

      // Step 1: feed `y`, `e`, `s` — no resolution yet.
      handle_key(&mut state, k(KeyCode::Char('y')));
      assert!(state.pending_permission.is_some());
      handle_key(&mut state, k(KeyCode::Char('e')));
      assert!(state.pending_permission.is_some());
      handle_key(&mut state, k(KeyCode::Char('s')));
      assert!(state.pending_permission.is_some());

      // Step 2: feed Enter — now resolves to AllowOnce.
      handle_key(&mut state, k(KeyCode::Enter));
      let resp = rx.await.unwrap();
      assert_eq!(resp, PermissionResponse::AllowOnce);
      assert!(state.pending_permission.is_none());
  }

  #[tokio::test]
  async fn bypass_enter_before_yes_does_not_resolve() {
      let mut state = AppState::default();
      let _rx = open_bypass_dialog(&mut state);
      handle_key(&mut state, k(KeyCode::Char('y')));
      handle_key(&mut state, k(KeyCode::Enter));
      assert!(state.pending_permission.is_some(),
          "Enter before full 'yes' must not resolve");
  }
  ```

- [ ] **Step 2: Run tests.**

  Run: `cargo test -p lingxi-tui --test behavior_permission_dialogs bypass 2>&1 | tail -10`
  Expected: both new tests pass.

- [ ] **Step 3: Commit.**

  ```bash
  git add lingxi-core/crates/tui/tests/behavior_permission_dialogs.rs
  git commit -m "test(m6-05 task 11): behavior — BypassPermissions requires typed yes"
  ```

---

### Task 12: Parity fixture — `tui_permission_dialogs.json`

**Files:**
- Create: `lingxi-core/crates/test-harness/src/parity/fixtures/tui_permission_dialogs.json`
- Create: `lingxi-core/crates/test-harness/tests/parity_tui_permission_dialogs.rs`

**Steps:**

- [ ] **Step 1: Write the fixture JSON.**

  Create `lingxi-core/crates/test-harness/src/parity/fixtures/tui_permission_dialogs.json`:
  ```json
  {
    "_claude_code_version": "2026-05-28-snapshot",
    "labels": {
      "tool_use_confirm": {
        "header_template": "Claude needs your permission to use {tool_name}",
        "button_allow_once": "[1] Allow Once",
        "button_allow_always": "[2] Allow Always",
        "button_deny": "[N] Deny"
      },
      "exit_plan_mode": {
        "header": "Claude Code needs your approval for the plan",
        "button_allow_once": "[1] Allow Once",
        "button_allow_always": "[2] Allow Always",
        "button_deny": "[N] Deny"
      },
      "bypass_permissions": {
        "title": "WARNING: Claude Code running in Bypass Permissions mode",
        "body_1": "In Bypass Permissions mode, Claude Code will not ask for your approval before running potentially dangerous commands.\nThis mode should only be used in a sandboxed container/VM that has restricted internet access and can easily be restored if damaged.",
        "body_2": "By proceeding, you accept all responsibility for actions taken while running in Bypass Permissions mode.",
        "confirm_word": "yes"
      }
    },
    "key_bindings": {
      "tool_use_confirm": [
        { "key": "1", "expects": "AllowOnce" },
        { "key": "2", "expects": "AllowAlways" },
        { "key": "n", "expects": "Deny" },
        { "key": "N", "expects": "Deny" },
        { "key": "Esc", "expects": "Deny" },
        { "key": "Enter@AllowOnce", "expects": "AllowOnce" }
      ],
      "exit_plan_mode": [
        { "key": "1", "expects": "AllowOnce" },
        { "key": "2", "expects": "AllowAlways" },
        { "key": "n", "expects": "Deny" },
        { "key": "Esc", "expects": "Deny" }
      ],
      "bypass_permissions": [
        { "keys": ["y", "e", "s", "Enter"], "expects": "AllowOnce" },
        { "keys": ["Esc"], "expects": "Deny" },
        { "keys": ["N"], "expects": "Deny" },
        { "keys": ["y", "Enter"], "expects": "no-resolution" }
      ]
    }
  }
  ```

- [ ] **Step 2: Write the parity driver.**

  Create `lingxi-core/crates/test-harness/tests/parity_tui_permission_dialogs.rs`:
  ```rust
  use lingxi_test_harness::parity::load_fixture;

  #[test]
  fn parity_tui_permission_dialogs_labels_locked() {
      let fixture: serde_json::Value = load_fixture("tui_permission_dialogs.json");
      // Header literal locks.
      assert_eq!(
          fixture["labels"]["tool_use_confirm"]["header_template"],
          "Claude needs your permission to use {tool_name}"
      );
      assert_eq!(
          fixture["labels"]["exit_plan_mode"]["header"],
          "Claude Code needs your approval for the plan"
      );
      assert_eq!(
          fixture["labels"]["bypass_permissions"]["title"],
          "WARNING: Claude Code running in Bypass Permissions mode"
      );
      // Button labels.
      assert_eq!(
          fixture["labels"]["tool_use_confirm"]["button_allow_once"],
          "[1] Allow Once"
      );
      assert_eq!(
          fixture["labels"]["tool_use_confirm"]["button_allow_always"],
          "[2] Allow Always"
      );
      assert_eq!(
          fixture["labels"]["tool_use_confirm"]["button_deny"],
          "[N] Deny"
      );
      // Bypass-confirmation word locked.
      assert_eq!(
          fixture["labels"]["bypass_permissions"]["confirm_word"],
          "yes"
      );
  }

  #[tokio::test]
  async fn parity_tui_permission_dialogs_key_bindings_round_trip() {
      use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
      use lingxi_permission::gate::{PermissionRequest, PermissionResponse, PromptDefault};
      use lingxi_tui::app::AppState;
      use lingxi_tui::events::keymap::handle_key;
      use serde_json::json;
      use tokio::sync::oneshot;

      let fixture: serde_json::Value = load_fixture("tui_permission_dialogs.json");

      // Walk tool_use_confirm bindings.
      for binding in fixture["key_bindings"]["tool_use_confirm"].as_array().unwrap() {
          let key_str = binding["key"].as_str().unwrap();
          let expects = binding["expects"].as_str().unwrap();
          let (tx, rx) = oneshot::channel();
          let mut state = AppState::default();
          state.pending_permission = Some(PermissionRequest::ToolUseConfirm {
              tool_name: "Bash".to_string(),
              tool_input: json!({}),
              default_decision: PromptDefault::DenyByDefault,
          });
          state.pending_permission_resp_tx = Some(tx);

          let key = match key_str {
              "1" => KeyEvent::new(KeyCode::Char('1'), KeyModifiers::NONE),
              "2" => KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE),
              "n" => KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE),
              "N" => KeyEvent::new(KeyCode::Char('N'), KeyModifiers::NONE),
              "Esc" => KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
              "Enter@AllowOnce" => KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
              other => panic!("unknown key in fixture: {other}"),
          };
          handle_key(&mut state, key);
          let resp = rx.await.unwrap();
          let actual = match resp {
              PermissionResponse::AllowOnce => "AllowOnce",
              PermissionResponse::AllowAlways => "AllowAlways",
              PermissionResponse::Deny => "Deny",
          };
          assert_eq!(actual, expects, "key {key_str}");
      }

      // (Similar walks for exit_plan_mode and bypass_permissions — omitted for brevity here
      // in the plan, but engineer adds them following the same pattern.)
  }
  ```

  Note: `load_fixture` is the existing test-harness helper used by every other parity test (it reads from `crates/test-harness/src/parity/fixtures/`). If it doesn't have a generic JSON variant, add a 5-line one — most M5 parity drivers already use it.

- [ ] **Step 3: Run the parity driver.**

  Run: `cargo test -p lingxi-test-harness parity_tui_permission_dialogs 2>&1 | tail -20`
  Expected: both tests pass. If the second test omits coverage for `exit_plan_mode` or `bypass_permissions` arrays, ADD those walks before tagging.

- [ ] **Step 4: Commit.**

  ```bash
  git add lingxi-core/crates/test-harness/src/parity/fixtures/tui_permission_dialogs.json lingxi-core/crates/test-harness/tests/parity_tui_permission_dialogs.rs
  git commit -m "test(m6-05 task 12): parity fixture — tui_permission_dialogs"
  ```

---

### Task 13: Telemetry registration + verification gate + tag

**Files:**
- Modify: `lingxi-core/crates/telemetry/src/tengu/tui.rs`
- Modify: `lingxi-core/crates/telemetry/src/tengu/mod.rs`
- Modify: `lingxi-core/crates/telemetry/tests/event_name_completeness_test.rs`
- Modify: `lingxi-core/crates/test-harness/src/parity/fixtures/tengu_events.json`
- Create: `lingxi-core/crates/tui/src/telemetry.rs` (or extend the M6-01 one)

**Steps:**

- [ ] **Step 1: Add the two telemetry constants.**

  Open `lingxi-core/crates/telemetry/src/tengu/tui.rs` (created in M6-01 with the 4 baseline TUI events). Append:
  ```rust
  /// Emitted when a permission dialog opens in the TUI.
  pub const PERMISSION_DIALOG_SHOWN: &str = "tengu_tui_permission_dialog_shown";

  /// Emitted when a permission dialog is resolved (Allow/Deny/persist + elapsed_ms).
  pub const PERMISSION_DIALOG_RESOLVED: &str = "tengu_tui_permission_dialog_resolved";
  ```
  Then extend the existing `NAMES` slice by 2 entries — appended after whatever the M6-04 baseline includes. Example:
  ```rust
  pub const NAMES: &[&str] = &[
      // … existing M6-01..04 entries
      PERMISSION_DIALOG_SHOWN,
      PERMISSION_DIALOG_RESOLVED,
  ];
  ```

- [ ] **Step 2: Bump the `TOTAL` formula.**

  In `lingxi-core/crates/telemetry/src/tengu/mod.rs`, find the `TOTAL = ... + <tui_count> + ...` formula. Increment the `tui` slot by 2. Example: if M6-04 ended at `tui = 8`, this becomes `tui = 10`.

- [ ] **Step 3: Add payload structs + emitter helpers.**

  In `lingxi-core/crates/tui/src/telemetry.rs`, append:
  ```rust
  use lingxi_permission::gate::PermissionResponse;
  use lingxi_telemetry::tengu::tui::{PERMISSION_DIALOG_RESOLVED, PERMISSION_DIALOG_SHOWN};

  pub fn permission_dialog_shown(kind: &str) {
      tracing::info!(target: "lingxi.tengu", event = PERMISSION_DIALOG_SHOWN, kind = kind);
  }

  pub fn permission_dialog_resolved(
      kind: &str,
      response: PermissionResponse,
      persist: bool,
      elapsed_ms: u64,
  ) {
      let decision = match response {
          PermissionResponse::AllowOnce => "allow_once",
          PermissionResponse::AllowAlways => "allow_always",
          PermissionResponse::Deny => "deny",
      };
      tracing::info!(
          target: "lingxi.tengu",
          event = PERMISSION_DIALOG_RESOLVED,
          kind = kind,
          decision = decision,
          persist = persist,
          elapsed_ms = elapsed_ms,
      );
  }
  ```

- [ ] **Step 4: Insert into `tengu_events.json` parity fixture.**

  Open `lingxi-core/crates/test-harness/src/parity/fixtures/tengu_events.json`. Find the section where M6-04's last TUI events were appended (registration order). Insert `tengu_tui_permission_dialog_shown` and `tengu_tui_permission_dialog_resolved` IMMEDIATELY after the last M6-04 entry and BEFORE any later category.

- [ ] **Step 5: Update event-count test.**

  Open `lingxi-core/crates/telemetry/tests/event_name_completeness_test.rs`. Locate the registry-size assertion (e.g. `registry_is_exactly_NNN_entries`). Bump by 2 (`NNN → NNN+2`). Update the test name + comment to mention M6-05.

- [ ] **Step 6: Run the workspace verification gate.**

  ```bash
  cargo fmt --all --check
  cargo clippy --workspace --all-targets -- -D warnings
  cargo test --workspace
  ```

  Expected:
  - fmt passes
  - clippy passes (no new warnings)
  - all tests pass — including:
    - `lingxi-traits` tests
    - `lingxi-permission` tests (incl. M5-05 regression suite)
    - `lingxi-orchestrator::handle_impl::tui_permission_gate_tests` (3 tests)
    - `lingxi-tui::components::permissions::*` (8 + 8 + 7 = 23 unit tests)
    - `lingxi-tui` snapshot tests (4)
    - `lingxi-tui` behavior tests — `focus_trap_test.rs` (2), `behavior_permission_dialogs.rs` (7)
    - `lingxi-test-harness::parity_tui_permission_dialogs` (2 tests)
    - `lingxi-telemetry::event_name_completeness_test` (bumped count)

- [ ] **Step 7: Run cross-platform compile check.**

  ```bash
  cargo check --workspace --target x86_64-apple-darwin
  cargo check --workspace --target x86_64-unknown-linux-gnu
  cargo check --workspace --target x86_64-pc-windows-gnu
  ```
  Expected: all 3 green. (Android/iOS targets are checked in M6-09 — not required here per the spec's per-sub-plan gate.)

- [ ] **Step 8: Create the milestone tag.**

  ```bash
  git tag -a m6.5 -m "M6-05: 3 permission dialogs (ToolUseConfirm, ExitPlanMode, BypassPermissionsMode) + focus-trap"
  ```
  Confirm with `git tag -l 'm6.*'` — expect `m6.1`, `m6.2`, `m6.3`, `m6.4`, `m6.5`.

  Do NOT push the tag. The user pushes on release per the M5-14 / v0.6.0 convention.

- [ ] **Step 9: Commit any remaining changes.**

  ```bash
  git add lingxi-core/crates/telemetry/ lingxi-core/crates/test-harness/src/parity/fixtures/tengu_events.json lingxi-core/crates/tui/src/telemetry.rs
  git commit -m "chore(m6-05 task 13): register 2 TUI permission-dialog telemetry events + tag m6.5"
  ```

---

## Self-Review Checklist

Before declaring M6-05 done, re-walk this list:

1. **Spec coverage (parent design §3 sub-plan M6-05):**
   - 3 dialog components shipped (Tasks 3, 4, 5) ✓
   - Keys 1/2/N/Esc/Enter wired (Tasks 3, 4, 9, 10) ✓
   - BypassPermissionsMode requires typed `yes` (Tasks 5, 11) ✓
   - Focus-trap works (Task 8) ✓
   - REPL overlays dialog above PromptInput (Task 8 step 4) ✓
   - 2 telemetry events registered (Task 13) ✓
   - Parity fixture locks labels + bindings (Task 12) ✓

2. **Placeholder scan:** searched the plan for "TBD", "TODO", "fill in details", "implement later". None present. (One acceptable note: Task 7 step 5's `PermissionRule::allow_tool` fallback "if helper doesn't exist, add it" — this is a defensive note, not a placeholder.)

3. **Type consistency:**
   - `PermissionRequest::ToolUseConfirm` field names (`tool_name`, `tool_input`, `default_decision`) match between Task 1, Task 2, and Task 7.
   - `PermissionResponse` variants (`AllowOnce`, `AllowAlways`, `Deny`) match across Tasks 1, 3, 7, 9, 10, 11, 12.
   - `DialogResolution::{response, persist}` consistent across Tasks 3, 4, 5, 7, 8.
   - `DialogFocus` consistent across Tasks 3, 4 (Task 5 doesn't use it — Bypass has its own state).
   - Telemetry constant names (`PERMISSION_DIALOG_SHOWN` / `PERMISSION_DIALOG_RESOLVED`) match between Task 7 step 7, Task 13, and the fixture.

4. **Open gaps (documented, not blockers):**
   - `PermissionRule::allow_tool` / `matches_tool` helpers may need fields adapted to the M1.3 `PermissionRule` shape — Task 7 step 5 notes this.
   - iocraft 0.6 overlay API: if `z-index` overlay isn't supported, Task 8 step 4 falls back to "dialog replaces 3-zone layout" — engineer decides at execution time.
   - The post-M6-04 baseline event count `<M6_04_TOTAL>` is unknown at plan-writing time. Task 13 step 5 uses `<M6_04_TOTAL> + 2` symbolically — the engineer reads the actual count from `event_name_completeness_test.rs` and bumps it accordingly.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-05-28-m6-05-permission-dialogs.md`.

Two execution options:

1. **Subagent-Driven (recommended)** — dispatch a fresh subagent per task, review between tasks, fast iteration.
2. **Inline Execution** — execute tasks in this session using executing-plans, batch execution with checkpoints.
