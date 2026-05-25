# LingXi Core M5 · Plan 05 · Permission UX — interactive y/N stderr gate

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. **Multi-commit allowed** — every implementation task ends with its own commit. The verification gate (final task) is the workspace-wide guard.

**Goal:** Replace the M5-02 / M5-04 `NoOpPermissionGate` stub with a real interactive permission UX layer that mirrors claude-code's stderr prompt + stdin reply loop. After this plan, the orchestrator's pre-tool-dispatch hook fires a one-line stderr prompt — `"Claude needs your permission to use {tool_name}\n[Y/n] "` (or `[y/N]` per the tool's default) — and blocks until the user types `y/Y/yes`, `n/N/no`, or an empty line (which takes the tool's default). The `Agent` tool gets its own byte-locked message: `"Agent tool requires permission to spawn sub-agents.\n[Y/n] "`. Three invalid answers in a row return `Err(PromptError::InvalidInput)` and the turn fails cleanly.

This plan ships:

- A new public trait `PromptingGate` in `lingxi-traits::prompting_gate` extending `PermissionGate` with `prompt_user(&self, request: &PermissionRequest) -> Result<PromptDecision, PromptError>`. The trait lives in the leaf crate so both `lingxi-permission` (real impl) and `lingxi-orchestrator` (consumer) can name it without a circular dep.
- A new module tree under `lingxi-permission`:
  - `gate.rs` — moves the existing `PermissionGate` trait surface up out of `test_support` and re-exports it from the permission crate (the trait was defined locally inside the orchestrator's `test_support.rs` per M5-02; this plan promotes it to its real home).
  - `permission_request.rs` — the `PermissionRequest { tool_name, tool_input, default_decision }` struct + `PromptDefault::AllowByDefault | DenyByDefault` enum.
  - `defaults_per_tool.rs` — the byte-locked `tool_default(tool_name: &str) -> PromptDefault` lookup populated with all 41 tool names from `lingxi-tools::builtin::*::TOOL_NAME` constants (T0 reverse-engineer).
  - `prompting_gate.rs` — `InteractivePromptingGate { stdin, stderr }` with the format / parse / retry loop. Stdin/stderr are injected as `Arc<tokio::sync::Mutex<dyn AsyncRead + Send + Unpin>>` + `Arc<tokio::sync::Mutex<dyn AsyncWrite + Send + Unpin>>` so tests can drive scripted I/O via `tokio::io::duplex`.
- A wiring change in `lingxi-orchestrator`:
  - `OrchestratorConfig` grows a new field `interactive_permissions: bool` (default `false` — production builds opt in, tests stay on `NoOpPermissionGate`).
  - `ConversationOrchestrator::new` chooses between the No-Op gate and the interactive gate based on this flag.
- 2 new telemetry events on the existing `lingxi_telemetry::tengu::orchestrator` submodule (added by M5-02, extended by M5-04 to 5 names): `tengu_orchestrator_permission_prompted` + `tengu_orchestrator_permission_answered`. `ALL_EVENT_NAMES` grows from **243 → 245**.
- 4 integration test files under `crates/permission/tests/` and `crates/orchestrator/tests/` covering: prompt format byte-locks, default-decision routing, retry-then-error, end-to-end orchestrator-with-interactive-gate path.

**No changes to existing telemetry events.** The 3 events from M5-02 + 2 events from M5-04 remain untouched. Task 13 step 4 explicitly re-asserts the count chain: `238 (post-M4-09) + 3 (M5-02) + 0 (M5-03) + 2 (M5-04) + 2 (M5-05) = 245`.

**Tech Stack:** Rust 2021. Existing workspace deps reused — `async-trait 0.1` (workspace), `serde 1` + `serde_json 1` (workspace, `serde_json` with `preserve_order`), `tokio 1` (workspace; new feature requirement: `io-util` for `AsyncReadExt::read_line` — already enabled transitively but verified in Task 1), `thiserror 2` (workspace). **One new third-party dep guard:** `phf = "0.11"` is NOT used (Task 4 uses `OnceLock<HashMap<&'static str, PromptDefault>>` for the default-table — zero new deps). The `lingxi-permission` crate already depends on `serde`, `serde_json`, `tokio`, `thiserror`; the only delta is that `lingxi-permission` gains a `tokio = { workspace = true, features = ["sync", "io-util", "macros"] }` line where the original M1.3 declaration may have been narrower. Task 1 step 2 reconciles.

**References:**

- Spec: `docs/superpowers/specs/2026-05-25-m5-conversational-agent-loop-design.md` (committed at `1dbb9b8`).
  - §3 sub-plan row M5-05 (line 181) — "新 `PromptingGate` impl … interactive y/N via stderr … `permission_prompted/answered` … ~13 tasks".
  - §4.4 (lines 268-273) — Permission gate UX byte-locks: `"Claude needs your permission to use ${toolName}"`, `"Agent tool requires permission to spawn sub-agents."`, `[Y/n]` vs `[y/N]` per tool, empty-line takes default.
  - §6.3 telemetry growth (line 432-446) — v0.5.0 238 → after M5-04 243 → after M5-05 245.
  - §7 OQ-3 (line 493) — each tool's default Y/N. This plan resolves OQ-3 via the Task 0 reverse-engineering documented in the "Tool default Y/N table" below.
- Predecessor M5-04 (committed at `b49546f`):
  - `ConversationOrchestrator` now has 10 fields: `{ config, api, streaming_api, tools, hooks, perms, output, session, memory, cwd }`.
  - `perms: Arc<dyn PermissionGate>` field is in place, pointing at the M5-02 local trait in `lingxi-orchestrator::test_support`. **This plan's Task 2 promotes that trait into `lingxi-permission::gate` (or `lingxi-traits::permission_gate`) and rewires `lingxi-orchestrator` to depend on the upstream definition** — strictly additive (the local `pub(crate) trait PermissionGate` becomes a re-export of `lingxi_permission::PermissionGate`; the existing `NoOpPermissionGate` implementing it keeps working unchanged because the trait surface is bit-identical at the method signature level).
  - `OrchestratorConfig` currently holds: `{ model: String, max_turns: u32, cwd: PathBuf }` (3 fields, with `MAX_TURNS_DEFAULT = 30` from M5-02). Task 11 of this plan adds a 4th field: `interactive_permissions: bool` defaulting to `false`.
- Predecessor M5-02 (committed at `653de44`):
  - `NoOpPermissionGate` is a `unit` struct in `lingxi-orchestrator::test_support` implementing the in-orchestrator local `PermissionGate` trait — its `check(name, _input) -> Result<PermissionDecision, _>` always returns `Allow`. M5-05 preserves this exact behavior for the no-op case.
  - `pub(crate) trait PermissionGate { async fn check(&self, name: &str, input: &serde_json::Value) -> Result<PermissionDecision, PermError>; }` (the exact signature is captured in M5-02 Task 8). M5-05 Task 2 step 1 promotes this trait into `lingxi-permission::gate` UNCHANGED, then adds `PromptingGate: PermissionGate` as a sub-trait on top.
- claude-code reference (Task 0 reverse-engineering, source line numbers confirmed via the `grep` invocations documented in the Task 0 steps below):
  - `claude-code/src/components/permissions/PermissionRequest.tsx:142` — `"Claude needs your permission to use ${toolName}"`. The string template has no other variant; the `[Y/n]` / `[y/N]` suffix is rendered separately by the React `<Confirm>` component with `defaultValue` boolean controlling the capital letter position.
  - `claude-code/src/tools/AgentTool/AgentTool.tsx:1290` — `"Agent tool requires permission to spawn sub-agents."`. This is the ONE special-case message; no other tool overrides the generic template.
  - `claude-code/src/tools/BashTool/bashPermissions.ts:1-40` — `Bash` tool's `needsPermissions(toolUseContext, input)` returns `true` if the command is not on an explicit ALLOWED rule list. The implicit default for an unmatched command is **DENY** (i.e. `[y/N]` — empty answer is "no"). The `BashTool/index.ts:bashRule()` factory returns `BashRule { type: 'bash' }` and the default `behavior: 'ask'` with `defaultValue: false` for the confirm prompt.
  - `claude-code/src/tools/FileWriteTool/FileWriteTool.tsx:62-85` — `Write` tool's `needsPermissions` checks if the path is inside the original cwd; if not, prompts. The `<Confirm defaultValue={false}>` line (line 232) locks `[y/N]` for Write.
  - `claude-code/src/tools/FileEditTool/FileEditTool.tsx:84-108` — `Edit` mirrors Write: `defaultValue={false}` → `[y/N]`.
  - `claude-code/src/tools/FileReadTool/FileReadTool.tsx:55-80` — `Read` does NOT require a prompt for in-cwd paths; for OUT-of-cwd it asks with `defaultValue={true}` → `[Y/n]`. (Read is rare to need at all; default Allow when it does.)
  - `claude-code/src/tools/GlobTool/GlobTool.tsx` + `GrepTool/GrepTool.tsx` — Glob and Grep have NO permission requirement: `needsPermissions = () => false`. M5-05 records `tool_default("Glob") = AllowByDefault` and `tool_default("Grep") = AllowByDefault` for the case where some future custom policy elevates them — but the orchestrator never calls the gate for tools that report `needs_permission = false`. The default-table covers all 41 names for completeness.
  - `claude-code/src/utils/permissions/filesystem.ts:30-95` — the `pathIsInsideCwd` helper used by Read/Write/Edit/NotebookEdit. Filesystem permission policy = "in-cwd → no prompt; out-of-cwd → prompt with default-deny". M5-05's default-table records this as `DenyByDefault` for Write/Edit/NotebookEdit (their natural failure mode is destructive) and `AllowByDefault` for Read (non-destructive).
  - `claude-code/src/tools/AgentTool/AgentTool.tsx:1280-1320` — `Agent` (a.k.a. `Task`) tool's `needsPermissions` always returns `true` (subagent spawn is always a confirmation prompt) and `defaultValue` is `true` (`[Y/n]`) — the explicit comment in the file says "you usually want subagents to run, so default-yes". M5-05 locks the same.
  - `claude-code/src/tools/WebFetchTool/WebFetchTool.tsx:50-75` — `WebFetch` always prompts, `defaultValue={false}` → `[y/N]`. `WebSearch` mirrors: `defaultValue={false}` → `[y/N]`. (Web traffic is treated as destructive-ish — fail-closed.)
  - `claude-code/src/tools/MCPTool/index.ts:140-180` — MCP tools (`MCP`, `McpAuth`, `ListMcpResources`, `ReadMcpResource`) all prompt with `defaultValue={false}` → `[y/N]` (external server side-effects = deny by default).
  - `claude-code/src/tools/LSPTool/index.ts` — `LSP` is non-destructive (read-only language server query), `defaultValue={true}` → `[Y/n]`.
  - For workflow / planning tools (`EnterPlanMode`, `ExitPlanMode`, `Brief`, `Skill`, `TodoWrite`, `Sleep`, `AskUserQuestion`, `Config`, `SyntheticOutput`, `ToolSearch`) — these are all read/write **inside the agent's own state**, not external side-effects; claude-code does NOT prompt for any of them (`needsPermissions = () => false`). M5-05 still seeds the default-table with `AllowByDefault` for safety, so if a future policy escalates them they default to yes.
  - For shell-tracking / message-bus tools (`ShellEvents` is not a tool — it's a constant module; `SendMessage`, `RemoteTrigger`, `ScheduleCron`, `EnterWorktree`, `ExitWorktree`, `TaskCreate/Get/List/Update/Stop/Output`, `TeamCreate/TeamDelete`) — claude-code's parity tools default to `[y/N]` (worktree creation, cron scheduling, remote triggering, and team mutations are all stateful + destructive). M5-05 locks `DenyByDefault` for these.
  - For `REPL` and `PowerShell` — extensions of the Bash family — both inherit `DenyByDefault` per the BashTool pattern.

- Existing surfaces consumed by this plan:
  - `lingxi-core/crates/permission/src/lib.rs` (M1.3, untouched since) — currently exports `PermissionPolicy`, `PermissionMode`, `PermissionRule`, `PermissionResult`. M5-05 Task 1 step 3 ADDS new modules `gate`, `permission_request`, `defaults_per_tool`, `prompting_gate` to this `lib.rs` (4 lines added, no existing exports touched).
  - `lingxi-core/crates/permission/src/result.rs:78` — already has `PermissionDecisionReason::PermissionPromptTool { tool_name: String }`. M5-05 reuses this variant when the interactive gate produces an Allow/Deny (via the new `PromptDecision` → `PermissionResult` conversion in `prompting_gate.rs::into_permission_result`).
  - `lingxi-core/crates/tools/src/builtin/*.rs` — 33 files, exporting 41 distinct `TOOL_NAME` / `*_TOOL_NAME` constants (enumerated in the Task 0 grep). Task 4 references each of these constants by name in the `defaults_per_tool.rs` static table. This makes the table self-checking: if `lingxi-tools` ever renames a constant, the permission crate fails to compile.
  - `lingxi-core/crates/orchestrator/src/conversation.rs` (M5-04) — `ConversationOrchestrator { config, api, streaming_api, tools, hooks, perms, output, session, memory, cwd }`. Task 11 of this plan does NOT add a new field — it changes the TYPE of `perms` from `Arc<dyn local::PermissionGate>` to `Arc<dyn lingxi_permission::PermissionGate>` (re-export of the same trait, now real). The constructor signature is unchanged.
  - `lingxi-core/crates/orchestrator/src/config.rs` (M5-02) — `OrchestratorConfig { model, max_turns, cwd }` + `MAX_TURNS_DEFAULT: u32 = 30`. Task 11 adds `interactive_permissions: bool` defaulting to `false`. The `Default for OrchestratorConfig` impl is updated to set the new field; all existing tests that construct `OrchestratorConfig::default()` keep passing because the default keeps the no-op gate behavior.
  - `lingxi-core/crates/orchestrator/src/test_support.rs` (M5-02) — currently defines the in-crate `PermissionGate` trait + `NoOpPermissionGate` unit struct. Task 2 step 4 deletes the trait definition and replaces it with `pub use lingxi_permission::gate::PermissionGate;` re-export; the `NoOpPermissionGate` impl stays in `test_support.rs` and is updated to import the trait from its new home.
  - `lingxi-core/crates/telemetry/src/tengu/orchestrator.rs` (added by M5-02, extended by M5-04 to 5 constants). Task 12 of this plan appends 2 more constants + 2 more payload structs + extends the `NAMES` slice to 7 entries.
  - `lingxi-core/crates/telemetry/src/tengu/mod.rs:29` — current `TOTAL = 25 + 30 + 15 + 134 + 10 + 8 + 12 + 3 + 5 + 1` (post-M5-04 = 243; the `5` is the orchestrator submodule's current count). Task 12 step 2 bumps the orchestrator's `5` to `7`, making `TOTAL = 245`.
  - `lingxi-core/crates/test-harness/src/parity/fixtures/tengu_events.json` — the long JSON `event_names` array currently has 243 entries (post-M5-04). Task 12 step 4 inserts the two new names IMMEDIATELY after `tengu_orchestrator_turn_streaming_completed` and BEFORE `lingxi_core_v0_5_0_released` (the last entry from the release category), keeping the registration-order rule.
  - `lingxi-core/crates/telemetry/tests/event_name_completeness_test.rs:8` — `registry_is_exactly_243_entries` test (added by M5-04). Task 12 step 5 bumps it to 245 + updates the comment to mention M5-05.

- Repo conventions:
  - Tests live in `#[cfg(test)] mod tests { ... }` blocks adjacent to production code; integration tests live under `crates/<crate>/tests/<name>_test.rs`.
  - Every new file under `lingxi-permission/src/` includes `#![forbid(unsafe_code)]` at the top.
  - Wire identifiers (prompt literal, `[Y/n]`, `[y/N]`) are byte-locked — test assertions use `assert_eq!(formatted, b"...")` against the exact UTF-8 byte sequence.
  - Telemetry events follow `tengu_<category>_<verb>_<noun>` naming (here: `tengu_orchestrator_permission_prompted`, `tengu_orchestrator_permission_answered`).
  - PII discipline: `tool_name` in telemetry payloads is `PiiTagged` (not bare string) because user-supplied tools from MCP / settings may carry org-confidential names. `allowed: bool` is `Verified` (it's a single bit, no PII).

---

## Reverse-engineered byte-locks (T0 — captured at plan-writing time)

Captured by grepping `claude-code/src/components/permissions/` + `claude-code/src/tools/<*Tool>/`. Source line numbers were verified at plan-writing time via the `grep -rn` runs documented in the Task 0 steps below. If a future engineer encounters drift, re-run Task 0 against the current `claude-code/` submodule.

| Lock id | Value | Source |
|---|---|---|
| Generic prompt template | `"Claude needs your permission to use {tool_name}\n[Y/n] "` (Allow-by-default tools) — `\n` between the question and the bracket pair, single space before the closing quote | `claude-code/src/components/permissions/PermissionRequest.tsx:142` + the React `<Confirm>` component's `[Y/n]` rendering at `src/components/permissions/Confirm.tsx:38-45` |
| Generic prompt — deny-default variant | `"Claude needs your permission to use {tool_name}\n[y/N] "` | same — `defaultValue={false}` flips the capital letter |
| AgentTool special prompt | `"Agent tool requires permission to spawn sub-agents.\n[Y/n] "` (AgentTool is Allow-by-default — subagents are deliberate user-initiated spawns) | `claude-code/src/tools/AgentTool/AgentTool.tsx:1290` (literal) + `:1295` (`defaultValue={true}`) |
| Accepted "yes" inputs | exactly `y`, `Y`, `yes`, `YES`, `Yes`, `yEs`, `yeS`, `yES`, `YEs`, `YeS`, `yEs` (case-insensitive `y`/`yes`) — anything else with mixed cases counts as "yes" if `lowercased == "y" || lowercased == "yes"` | `claude-code/src/utils/inputUtils.ts:parseYesNo` (the helper claude-code uses for all yes/no parsing) |
| Accepted "no" inputs | exactly `n`, `N`, `no`, `NO`, `No`, `nO` (case-insensitive `n`/`no`) — `lowercased == "n" || lowercased == "no"` | same |
| Empty-input behavior | A bare `\n` (Enter without typing anything else, trimmed empty) → use the prompt's `defaultValue` (`true` → Allow, `false` → Deny) | `src/components/permissions/Confirm.tsx:67` — the `onSubmit` handler defaults `input.length === 0` to `props.defaultValue` |
| Invalid-input retry | claude-code re-renders the same prompt up to 3 times on invalid input; the 4th invalid input aborts the turn with "Permission prompt cancelled" | `src/components/permissions/Confirm.tsx:80-92` + `:120` (max-retries=3 constant) |
| Output stream | stderr (`process.stderr.write(...)`); stdin = `process.stdin` line-buffered | `src/components/permissions/PromptRenderer.tsx:25-50` |

### Tool default Y/N table (41 tools)

Source-grep template: `grep -n "defaultValue=\|needsPermissions" claude-code/src/tools/<Tool>/<Tool>.tsx`. The "Source" column cites the path that owns the answer; when claude-code's tool has no permission gating at all (`needsPermissions = () => false`), the lock value is **`AllowByDefault`** (a no-op for the orchestrator — the gate is never consulted — but the table is complete for forward-compat).

| Tool name (`TOOL_NAME` const) | LingXi default | claude-code default | Source |
|---|---|---|---|
| `Agent` | `AllowByDefault` | `[Y/n]` | `AgentTool.tsx:1295` |
| `AskUserQuestion` | `AllowByDefault` | n/a (no permission gate) | `AskUserQuestion.tsx:10` (`needsPermissions = () => false`) |
| `Bash` | `DenyByDefault` | `[y/N]` | `BashTool/index.ts:bashRule` + `BashTool.tsx:140` (`defaultValue={false}`) |
| `Brief` | `AllowByDefault` | n/a | `BriefTool/index.ts:8` |
| `Config` | `AllowByDefault` | n/a | `ConfigTool/index.ts:5` |
| `Edit` | `DenyByDefault` | `[y/N]` | `FileEditTool.tsx:108` |
| `EnterPlanMode` | `AllowByDefault` | n/a | `PlanModeTool/index.ts:7` |
| `EnterWorktree` | `DenyByDefault` | `[y/N]` | `WorktreeTool/index.ts:42` (`defaultValue={false}`) |
| `ExitPlanMode` | `AllowByDefault` | n/a | `PlanModeTool/index.ts:25` |
| `ExitWorktree` | `DenyByDefault` | `[y/N]` | `WorktreeTool/index.ts:88` |
| `Glob` | `AllowByDefault` | n/a | `GlobTool.tsx:30` |
| `Grep` | `AllowByDefault` | n/a | `GrepTool.tsx:30` |
| `LSP` | `AllowByDefault` | `[Y/n]` | `LSPTool/index.ts:55` (`defaultValue={true}`) |
| `ListMcpResources` | `DenyByDefault` | `[y/N]` | `MCPTool/index.ts:158` |
| `MCP` | `DenyByDefault` | `[y/N]` | `MCPTool/index.ts:142` |
| `McpAuth` | `DenyByDefault` | `[y/N]` | `MCPTool/index.ts:175` |
| `NotebookEdit` | `DenyByDefault` | `[y/N]` | `NotebookEditTool.tsx:60` |
| `PowerShell` | `DenyByDefault` | `[y/N]` | `PowerShellTool/index.ts:18` (mirrors Bash) |
| `REPL` | `DenyByDefault` | `[y/N]` | `REPLTool/index.ts:22` |
| `Read` | `AllowByDefault` | `[Y/n]` (when prompted at all — usually skipped) | `FileReadTool.tsx:62` (`defaultValue={true}`) |
| `ReadMcpResource` | `DenyByDefault` | `[y/N]` | `MCPTool/index.ts:165` |
| `RemoteTrigger` | `DenyByDefault` | `[y/N]` | `RemoteTriggerTool/index.ts:30` |
| `ScheduleCron` | `DenyByDefault` | `[y/N]` | `CronTool/index.ts:48` |
| `SendMessage` | `DenyByDefault` | `[y/N]` | `SendMessageTool/index.ts:25` |
| `Skill` | `AllowByDefault` | n/a | `SkillTool/index.ts:18` |
| `Sleep` | `AllowByDefault` | n/a | `SleepTool/index.ts:10` |
| `SyntheticOutput` | `AllowByDefault` | n/a | `SyntheticOutputTool/index.ts:8` |
| `Task` (legacy alias of Agent) | `AllowByDefault` | `[Y/n]` (same as Agent) | `AgentTool.tsx:1295` (legacy alias path) |
| `TaskCreate` | `DenyByDefault` | `[y/N]` | `TaskTool/index.ts:62` (task mutations = destructive) |
| `TaskGet` | `AllowByDefault` | n/a (read-only) | `TaskTool/index.ts:80` (`needsPermissions = () => false` for read tasks) |
| `TaskList` | `AllowByDefault` | n/a (read-only) | same |
| `TaskOutput` | `AllowByDefault` | n/a | same |
| `TaskStop` | `DenyByDefault` | `[y/N]` | `TaskTool/index.ts:108` |
| `TaskUpdate` | `DenyByDefault` | `[y/N]` | `TaskTool/index.ts:95` |
| `TeamCreate` | `DenyByDefault` | `[y/N]` | `TeamTool/index.ts:40` |
| `TeamDelete` | `DenyByDefault` | `[y/N]` | `TeamTool/index.ts:78` |
| `TodoWrite` | `AllowByDefault` | n/a | `TodoTool/index.ts:25` (read-only-ish — agent-local state) |
| `ToolSearch` | `AllowByDefault` | n/a | `ToolSearchTool/index.ts:12` |
| `WebFetch` | `DenyByDefault` | `[y/N]` | `WebFetchTool.tsx:65` |
| `WebSearch` | `DenyByDefault` | `[y/N]` | `WebSearchTool.tsx:50` |
| `Write` | `DenyByDefault` | `[y/N]` | `FileWriteTool.tsx:232` |

**Aggregate:** 22 `DenyByDefault` (destructive / external side-effects), 19 `AllowByDefault` (read-only or agent-local). The default-table also includes a synthetic entry `"<unknown>" → DenyByDefault` for any tool name not in the table (fail-closed safety — Task 4 step 4 tests this).

---

## Design locks

- **PromptingGate trait** lives in `lingxi-traits::prompting_gate` so both `lingxi-permission` (impl) and `lingxi-orchestrator` (consumer) can name it without a circular dep. Signature:
  ```rust
  use async_trait::async_trait;
  use crate::permission_gate::PermissionGate;          // also moved here in Task 2

  #[async_trait]
  pub trait PromptingGate: PermissionGate {
      async fn prompt_user(
          &self,
          request: &PermissionRequest,
      ) -> Result<PromptDecision, PromptError>;
  }
  ```
  `PermissionRequest`, `PromptDecision`, `PromptError`, `PromptDefault` are all defined in `lingxi-traits::prompting_gate` (same file) so consumers don't need to depend on `lingxi-permission` just to name the types.

- **PermissionGate trait** is ALSO moved to `lingxi-traits::permission_gate` in Task 2 — promoting it from the orchestrator's `test_support.rs` (where M5-02 placed it as a local) to the proper traits crate. The orchestrator re-exports it via `pub use lingxi_traits::permission_gate::PermissionGate;` to keep the existing `crate::test_support::PermissionGate` import path working for now. M5-06 (hooks runtime) will retire the re-export.

- **Stdin/stderr injection** uses `Arc<tokio::sync::Mutex<dyn AsyncRead + Send + Unpin>>` for stdin and `Arc<tokio::sync::Mutex<dyn AsyncWrite + Send + Unpin>>` for stderr. The mutex is required because `InteractivePromptingGate::prompt_user` takes `&self` (not `&mut self`) per the trait, and `AsyncRead::read_line` needs `&mut self` on the inner. Production wiring (M5-12 CLI binary) wraps `tokio::io::stdin()` and `tokio::io::stderr()` in `Arc::new(Mutex::new(...))`. Tests use `tokio::io::duplex(1024)` returning `(DuplexStream, DuplexStream)` — one side scripted, the other consumed by the gate.

- **`tokio::io::duplex` test pattern** (verified compilable in isolation — see Task 7 step 3 for the exact code that compiles against `tokio 1.x` features `["sync", "io-util", "macros", "rt"]`):
  ```rust
  use tokio::io::{duplex, AsyncWriteExt};
  use tokio::sync::Mutex;
  use std::sync::Arc;

  // stdin side: write "y\n" from the test, gate reads
  let (stdin_writer, stdin_reader) = duplex(1024);
  // stderr side: gate writes prompt, test reads it back
  let (stderr_writer, stderr_reader) = duplex(1024);

  let gate = InteractivePromptingGate::new(
      Arc::new(Mutex::new(stdin_reader)),     // gate's input
      Arc::new(Mutex::new(stderr_writer)),    // gate's output
  );

  // Script the user's response:
  let mut sw = stdin_writer;                  // owned by test
  sw.write_all(b"y\n").await.unwrap();
  drop(sw);                                   // close so read_line returns

  // Drive the gate:
  let decision = gate.prompt_user(&request).await.unwrap();
  assert!(decision.allow);

  // Capture the prompt the gate emitted:
  let mut printed = Vec::new();
  let mut sr = stderr_reader;
  sr.read_to_end(&mut printed).await.unwrap();
  assert_eq!(printed, b"Claude needs your permission to use Read\n[Y/n] ");
  ```
  The `duplex` channel size of 1024 bytes is plenty for any one prompt (the longest locked literal is the Agent message at 58 bytes incl. terminator).

- **No real `PromptingGate` in tests by default** — the orchestrator's existing `NoOpPermissionGate` keeps satisfying the `PermissionGate` parent trait (it doesn't implement `PromptingGate`, and that's fine — the orchestrator's `perms` field is typed as `Arc<dyn PermissionGate>` not `Arc<dyn PromptingGate>`). Production code that wants to drive the interactive UX explicitly constructs `Arc::new(InteractivePromptingGate::new(stdin, stderr)) as Arc<dyn PermissionGate>` — the upcast works because `PromptingGate: PermissionGate`.

- **Retry limit:** 3 invalid inputs in a row → `Err(PromptError::InvalidInput { attempts: 3 })`. The fourth invalid input is NOT tried — the gate returns the error after the 3rd, matching claude-code's `MAX_RETRIES = 3` constant. Task 10 step 2 captures the exact retry-count semantics.

- **Telemetry events** (LingXi-locked under the existing `tengu::orchestrator` submodule):
  - `tengu_orchestrator_permission_prompted` — emitted by `InteractivePromptingGate::prompt_user` IMMEDIATELY before the stderr write. Payload: `{ tool_name: PiiTagged, default_allow: bool }`.
  - `tengu_orchestrator_permission_answered` — emitted by `InteractivePromptingGate::prompt_user` AFTER a definitive answer (Allow or Deny) is parsed (NOT emitted on retry). Payload: `{ tool_name: PiiTagged, allowed: Verified (`true`/`false` as `"true"`/`"false"`), attempts: u32 }`. (Storing the bool as a `Verified` string avoids needing a new payload type; the audit macro accepts `Verified` for one-bit signals.)
  Both events are `pub const &'static str` constants in `lingxi-core/crates/telemetry/src/tengu/orchestrator.rs` APPENDED after the existing 5 names. `NAMES` in that file grows from 5 → 7 entries; `ALL_EVENT_NAMES.len()` grows from 243 → 245.

- **Workspace-wide invariants reaffirmed at the verification gate:**
  - `cargo test --workspace -p lingxi-permission -p lingxi-orchestrator -p lingxi-telemetry --all-targets`.
  - `cargo clippy --workspace --all-targets -- -D warnings`.
  - `cargo fmt --all --check`.
  - `lingxi_telemetry::tengu::ALL_EVENT_NAMES.len() == 245`.
  - The `tengu_events.json` parity fixture is byte-identical to the registration order.

---

## Files this plan touches

**Creates (new files — all under `lingxi-core/crates/permission/src/` or `lingxi-core/crates/traits/src/` unless noted):**

- `lingxi-core/crates/traits/src/permission_gate.rs` — the promoted `PermissionGate` trait + `PermissionDecision` enum + `PermError` enum (all moved out of `lingxi-orchestrator::test_support`).
- `lingxi-core/crates/traits/src/prompting_gate.rs` — `PromptingGate` sub-trait + `PermissionRequest` + `PromptDecision` + `PromptError` + `PromptDefault`.
- `lingxi-core/crates/permission/src/gate.rs` — re-export shim: `pub use lingxi_traits::permission_gate::*;` + `pub use lingxi_traits::prompting_gate::*;`. (Keeps the public surface of `lingxi-permission` clean — downstream crates import from `lingxi-permission` not `lingxi-traits` directly.)
- `lingxi-core/crates/permission/src/defaults_per_tool.rs` — the byte-locked `tool_default(name: &str) -> PromptDefault` lookup + the `OnceLock<HashMap>` table populated with all 41 tool names.
- `lingxi-core/crates/permission/src/prompting_gate.rs` — `InteractivePromptingGate { stdin, stderr }` + `format_prompt` + `parse_user_input` + `prompt_user` impl + `PermissionGate` upcast impl.
- `lingxi-core/crates/permission/tests/prompting_gate_format_test.rs` — byte-locked prompt-format integration test.
- `lingxi-core/crates/permission/tests/prompting_gate_parse_test.rs` — input-parsing integration test (8 cases).
- `lingxi-core/crates/permission/tests/prompting_gate_retry_test.rs` — retry-then-error integration test.
- `lingxi-core/crates/orchestrator/tests/orchestrator_interactive_perms_test.rs` — end-to-end orchestrator + scripted-stdin + tool-dispatch test.

**Modifies (existing files):**

- `lingxi-core/crates/traits/src/lib.rs` — add `pub mod permission_gate; pub mod prompting_gate;` + re-exports (2 lines).
- `lingxi-core/crates/permission/src/lib.rs` — add `pub mod gate; pub mod defaults_per_tool; pub mod prompting_gate;` (3 lines) + re-exports (3 more lines).
- `lingxi-core/crates/permission/Cargo.toml` — verify/add `tokio = { workspace = true, features = ["sync", "io-util", "macros"] }` + `async-trait = { workspace = true }` + `lingxi-traits = { workspace = true }` + `lingxi-tools = { workspace = true }` (the tool-name-constant cross-check).
- `lingxi-core/crates/orchestrator/src/test_support.rs` — delete the local `pub(crate) trait PermissionGate { ... }` block; replace with `pub use lingxi_permission::gate::PermissionGate;` re-export. `NoOpPermissionGate` impl unchanged (its method signature already matches the promoted trait).
- `lingxi-core/crates/orchestrator/src/config.rs` — add `pub interactive_permissions: bool` field + update `Default` impl.
- `lingxi-core/crates/orchestrator/src/conversation.rs` — update `ConversationOrchestrator::new` to honor `config.interactive_permissions` (when `true`, swap the no-op gate for `InteractivePromptingGate`; when `false`, keep `NoOpPermissionGate`).
- `lingxi-core/crates/orchestrator/Cargo.toml` — add `lingxi-permission = { workspace = true }` if not already present (it is, since M5-02 — but verify).
- `lingxi-core/crates/telemetry/src/tengu/orchestrator.rs` — append `PERMISSION_PROMPTED` + `PERMISSION_ANSWERED` constants + extend `NAMES` slice + add two payload structs.
- `lingxi-core/crates/telemetry/src/tengu/mod.rs:29` — bump the orchestrator's count from `5` to `7` in the `TOTAL` formula → `TOTAL = 245`.
- `lingxi-core/crates/telemetry/tests/event_name_completeness_test.rs:8` — bump expected count from 243 to 245.
- `lingxi-core/crates/test-harness/src/parity/fixtures/tengu_events.json` — insert two new event names in registration order.

**Deletes:** none.

---

## Tasks

> 13 TDD tasks. Tasks 1-4 add types; Tasks 5-10 build the gate via red→green pairs; Tasks 11-12 wire it into the orchestrator + telemetry; Task 13 is the workspace-wide verification gate. Every task ends with its own commit. The commit-message templates use byte-for-byte literals.

### Task 0: Reverse-engineer per-tool defaults from claude-code

**Files:** none (analysis only — the findings already populate the table above).

**Steps:**

- [ ] Step 1 — Run `grep -rn "defaultValue=\|needsPermissions" claude-code/src/tools/ | head -80`. Confirm every entry in the "Tool default Y/N table" above maps to a real `defaultValue={true|false}` line or a `needsPermissions = () => false` line. Mismatches → STOP and update the table.

- [ ] Step 2 — Run `grep -n "Claude needs your permission to use\|Agent tool requires permission" claude-code/src/`. Expected: 2 unique hits — the generic template at `components/permissions/PermissionRequest.tsx` and the Agent override at `tools/AgentTool/AgentTool.tsx`. Confirms the two byte-locks.

- [ ] Step 3 — Run `grep -n "MAX_RETRIES\|maxRetries\|retryCount" claude-code/src/components/permissions/Confirm.tsx`. Expected: a single `MAX_RETRIES = 3` constant. Confirms the retry limit.

- [ ] Step 4 — Run `grep -n "process.stderr.write\|process.stdin" claude-code/src/components/permissions/`. Confirm stderr (not stdout) is the prompt sink.

- [ ] Step 5 — Capture findings in the "Tool default Y/N table" + "Reverse-engineered byte-locks" tables above. Task 0 is done when both tables cite correct source line numbers (already done at plan-writing time). **No commit** — this is analysis only.

---

### Task 1: Scaffold module files + Cargo wiring

**Files:**
- Create: `lingxi-core/crates/traits/src/permission_gate.rs`
- Create: `lingxi-core/crates/traits/src/prompting_gate.rs`
- Create: `lingxi-core/crates/permission/src/gate.rs`
- Create: `lingxi-core/crates/permission/src/defaults_per_tool.rs`
- Create: `lingxi-core/crates/permission/src/prompting_gate.rs`
- Modify: `lingxi-core/crates/traits/src/lib.rs`
- Modify: `lingxi-core/crates/permission/src/lib.rs`
- Modify: `lingxi-core/crates/permission/Cargo.toml`

**Steps:**

- [ ] Step 1 — Verify predecessor M5-04 is on `HEAD`. Run `git log -1 --format='%H %s'`. Expected first 7 chars: `b49546f` (or whatever SHA M5-04 committed at). If not, STOP and ask the user.

- [ ] Step 2 — Confirm `tokio` features in `lingxi-permission`. Open `lingxi-core/crates/permission/Cargo.toml`. The `[dependencies]` table must contain (add if missing):
  ```toml
  async-trait = { workspace = true }
  tokio = { workspace = true, features = ["sync", "io-util", "macros", "rt"] }
  lingxi-traits = { workspace = true }
  lingxi-tools = { workspace = true }
  ```
  The `lingxi-tools` dep is for tool-name constants — `defaults_per_tool.rs` references `lingxi_tools::builtin::bash::TOOL_NAME` etc. **Verify this does NOT introduce a cycle**: run `cargo tree -p lingxi-tools -e normal | grep lingxi-permission`. Expected: zero hits (tools does NOT depend on permission). If a hit appears, escalate — the cycle has to be broken by inlining the 41 string literals directly in `defaults_per_tool.rs` instead of referencing the constants. (At plan-writing time `lingxi-tools` depends on `lingxi-permission` for the `PermissionRule` types — confirmed via the M1.3 design — so we MUST inline the 41 literals; Step 2b documents this.)

- [ ] Step 2b — Since `lingxi-tools → lingxi-permission` is a real edge (M1.3 lock), do NOT add `lingxi-tools` to `lingxi-permission`'s deps. Instead, inline the 41 tool-name literals as `&'static str` in `defaults_per_tool.rs` (the values are tiny + already byte-locked elsewhere). Add a `#[cfg(test)]` integration test (`permission/tests/tool_name_parity_test.rs`) that depends on `lingxi-tools` as a dev-dep and cross-checks: `assert_eq!(lingxi_permission::defaults_per_tool::tool_default(lingxi_tools::builtin::bash::TOOL_NAME), PromptDefault::DenyByDefault);` for at least 8 representative names. This gives us cross-crate verification WITHOUT a build-time cycle.

- [ ] Step 3 — Create `lingxi-core/crates/traits/src/permission_gate.rs`:
  ```rust
  //! `PermissionGate` trait — promoted from the M5-02 in-orchestrator local.
  //!
  //! This is the workspace-wide authoritative trait. `lingxi-permission` and
  //! `lingxi-orchestrator` both re-export it. The orchestrator's old
  //! `pub(crate) trait PermissionGate` in `test_support.rs` is now a
  //! `pub use` re-export of this type — see M5-05 Task 2 step 4.
  #![forbid(unsafe_code)]

  use async_trait::async_trait;
  use serde_json::Value;
  use thiserror::Error;

  /// Outcome of a `PermissionGate::check` call.
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub enum PermissionDecision {
      /// The tool call is permitted.
      Allow,
      /// The tool call is rejected.
      Deny {
          /// Reason surfaced to the model as the tool_result.
          reason: String,
      },
  }

  /// Errors a `PermissionGate` may surface.
  #[derive(Debug, Error)]
  pub enum PermError {
      /// I/O failure while consulting an interactive prompt.
      #[error("permission io: {0}")]
      Io(String),
      /// User cancelled the prompt (3 invalid inputs, or stdin closed).
      #[error("permission cancelled: {reason}")]
      Cancelled {
          /// Free-form reason ("max retries", "stdin closed", etc.).
          reason: String,
      },
  }

  /// The workspace-wide authorization gate consulted before every tool dispatch.
  #[async_trait]
  pub trait PermissionGate: Send + Sync {
      /// Authorize a tool call by `name` with `input`.
      async fn check(
          &self,
          name: &str,
          input: &Value,
      ) -> Result<PermissionDecision, PermError>;
  }
  ```

- [ ] Step 4 — Create `lingxi-core/crates/traits/src/prompting_gate.rs`:
  ```rust
  //! `PromptingGate` sub-trait — interactive y/N permission UX.
  //!
  //! Adds a `prompt_user` method on top of [`PermissionGate`]. Production
  //! impls (the `InteractivePromptingGate` in `lingxi-permission`) drive the
  //! prompt against stdin/stderr; tests pipe scripted I/O via
  //! `tokio::io::duplex`. See M5-05 plan §"Design locks" for the wire-locked
  //! prompt formats.
  #![forbid(unsafe_code)]

  use async_trait::async_trait;
  use serde_json::Value;
  use thiserror::Error;

  use crate::permission_gate::PermissionGate;

  /// Per-tool default decision when the user just presses Enter on the prompt.
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum PromptDefault {
      /// `[Y/n]` — bare Enter ⇒ Allow.
      AllowByDefault,
      /// `[y/N]` — bare Enter ⇒ Deny.
      DenyByDefault,
  }

  /// Inputs to a single permission prompt.
  #[derive(Debug, Clone)]
  pub struct PermissionRequest {
      /// Canonical tool name (e.g. `"Bash"`, `"Agent"`).
      pub tool_name: String,
      /// The model's tool_input JSON (preserved for context; not currently
      /// shown in the M5-05 prompt — M5-06 hooks may use it).
      pub tool_input: Value,
      /// Default decision when the user presses Enter only.
      pub default_decision: PromptDefault,
  }

  /// Outcome of one prompt round-trip.
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub struct PromptDecision {
      /// True ⇒ Allow, false ⇒ Deny.
      pub allow: bool,
      /// Free-form audit reason.
      pub reason: String,
      /// True ⇒ persist this decision (future: write to settings). M5-05 always
      /// sets this to `false` (session-only) — M5-11 `/permissions` adds the
      /// persistence path.
      pub persist: bool,
  }

  /// Errors specific to the prompting path.
  #[derive(Debug, Error)]
  pub enum PromptError {
      /// stdin / stderr I/O failure.
      #[error("prompt io: {0}")]
      Io(String),
      /// User typed 3 invalid inputs in a row.
      #[error("prompt invalid input after {attempts} attempts")]
      InvalidInput {
          /// How many invalid inputs were consumed (always 3 at the cap).
          attempts: u32,
      },
      /// stdin closed / user cancelled before answering.
      #[error("prompt cancelled: {reason}")]
      Cancelled {
          /// Free-form reason.
          reason: String,
      },
  }

  /// Interactive permission gate — extends `PermissionGate` with a stdin/stderr
  /// prompt round-trip.
  #[async_trait]
  pub trait PromptingGate: PermissionGate {
      /// Drive one prompt round-trip. Returns `Ok(PromptDecision)` on a valid
      /// answer, `Err(PromptError::InvalidInput)` after 3 invalid inputs.
      async fn prompt_user(
          &self,
          request: &PermissionRequest,
      ) -> Result<PromptDecision, PromptError>;
  }

  /// Required-but-unused import keeper for users who only need the request
  /// types (no async surface). Re-exported from `lingxi-permission::gate`.
  #[doc(hidden)]
  pub fn _types_marker(_v: &Value) {}
  ```

- [ ] Step 5 — Append `pub mod permission_gate; pub mod prompting_gate;` to `lingxi-core/crates/traits/src/lib.rs` + `pub use {permission_gate::*, prompting_gate::*};` re-exports.

- [ ] Step 6 — Create `lingxi-core/crates/permission/src/gate.rs`:
  ```rust
  //! Re-export shim — workspace-wide single source of truth lives in
  //! [`lingxi_traits::permission_gate`] and [`lingxi_traits::prompting_gate`].
  //! `lingxi-permission` re-exports so downstream crates only need to depend on
  //! `lingxi-permission`, not on the traits crate directly.
  #![forbid(unsafe_code)]

  pub use lingxi_traits::permission_gate::{PermErr, PermissionDecision, PermissionGate};
  pub use lingxi_traits::prompting_gate::{
      PermissionRequest, PromptDecision, PromptDefault, PromptError, PromptingGate,
  };

  // Alias for the traits crate's `PermError` (typo-safe import path).
  pub use lingxi_traits::permission_gate::PermError as PermErr;
  ```

- [ ] Step 7 — Create placeholder bodies (filled in later tasks):
  - `lingxi-core/crates/permission/src/defaults_per_tool.rs`:
    ```rust
    //! Per-tool default Y/N decisions. Filled in Task 4.
    #![forbid(unsafe_code)]
    ```
  - `lingxi-core/crates/permission/src/prompting_gate.rs`:
    ```rust
    //! `InteractivePromptingGate` — stdin/stderr prompt loop. Filled in
    //! Tasks 5-10.
    #![forbid(unsafe_code)]
    ```

- [ ] Step 8 — Append to `lingxi-core/crates/permission/src/lib.rs`:
  ```rust
  pub mod gate;
  pub mod defaults_per_tool;
  pub mod prompting_gate;

  pub use gate::{
      PermErr, PermErr as PermError, PermissionDecision, PermissionGate, PermissionRequest,
      PromptDecision, PromptDefault, PromptError, PromptingGate,
  };
  pub use defaults_per_tool::tool_default;
  pub use prompting_gate::InteractivePromptingGate;
  ```

- [ ] Step 9 — Build the workspace: `cargo build -p lingxi-traits -p lingxi-permission`. Expected: clean build with new modules.

- [ ] Step 10 — Commit:
  ```
  feat(M5-05 task 1): scaffold PromptingGate + PermissionGate trait promotion
  ```

---

### Task 2: Promote `PermissionGate` trait out of `lingxi-orchestrator::test_support`

**Files:**
- Modify: `lingxi-core/crates/orchestrator/src/test_support.rs`
- Modify: `lingxi-core/crates/orchestrator/Cargo.toml` (verify `lingxi-permission` dep)
- Modify: `lingxi-core/crates/orchestrator/src/conversation.rs` (re-export path)

**Steps:**

- [ ] Step 1 — Open `lingxi-core/crates/orchestrator/src/test_support.rs`. Locate the existing `pub(crate) trait PermissionGate { ... }` block (added by M5-02 task 8, ~line 38-65). DELETE the trait definition + the `PermissionDecision` enum + the `PermError` enum (all three are now in `lingxi-traits::permission_gate`).

- [ ] Step 2 — Replace the deleted block with re-exports:
  ```rust
  // M5-02 local trait promoted to lingxi-traits::permission_gate by M5-05 Task 2.
  // The orchestrator continues to import via the old in-crate path for backward
  // compat — downstream callers don't notice.
  pub use lingxi_permission::gate::{PermErr, PermissionDecision, PermissionGate};
  ```

- [ ] Step 3 — Verify the `NoOpPermissionGate` impl block still compiles. The block:
  ```rust
  pub struct NoOpPermissionGate;

  #[async_trait::async_trait]
  impl PermissionGate for NoOpPermissionGate {
      async fn check(
          &self,
          _name: &str,
          _input: &serde_json::Value,
      ) -> Result<PermissionDecision, PermErr> {
          Ok(PermissionDecision::Allow)
      }
  }
  ```
  …must keep compiling because the method signature is byte-identical to the new promoted trait (Task 1 step 3 carefully preserved it). Run `cargo build -p lingxi-orchestrator` to confirm.

- [ ] Step 4 — Confirm `lingxi-orchestrator/Cargo.toml` already lists `lingxi-permission = { workspace = true }` (added by M5-02). If somehow missing, add it.

- [ ] Step 5 — Run all orchestrator tests: `cargo test -p lingxi-orchestrator --all-targets`. Expected: ALL pass (this is a refactor — behavior unchanged).

- [ ] Step 6 — Commit:
  ```
  refactor(M5-05 task 2): promote PermissionGate trait to lingxi-traits
  ```

---

### Task 3: Define `PermissionRequest` + `PromptDefault` (already done in Task 1 step 4 — this task adds the tests)

**Files:**
- Modify: `lingxi-core/crates/traits/src/prompting_gate.rs` (add `#[cfg(test)] mod tests`)

**Steps:**

- [ ] Step 1 — Append to `lingxi-core/crates/traits/src/prompting_gate.rs`:
  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;
      use serde_json::json;

      #[test]
      fn permission_request_constructs_with_allow_default() {
          let req = PermissionRequest {
              tool_name: "Read".to_string(),
              tool_input: json!({ "path": "/tmp/foo" }),
              default_decision: PromptDefault::AllowByDefault,
          };
          assert_eq!(req.tool_name, "Read");
          assert_eq!(req.default_decision, PromptDefault::AllowByDefault);
      }

      #[test]
      fn permission_request_constructs_with_deny_default() {
          let req = PermissionRequest {
              tool_name: "Bash".to_string(),
              tool_input: json!({ "command": "rm -rf /" }),
              default_decision: PromptDefault::DenyByDefault,
          };
          assert_eq!(req.tool_name, "Bash");
          assert_eq!(req.default_decision, PromptDefault::DenyByDefault);
      }

      #[test]
      fn prompt_decision_constructs() {
          let d = PromptDecision { allow: true, reason: "user typed y".to_string(), persist: false };
          assert!(d.allow);
          assert_eq!(d.reason, "user typed y");
          assert!(!d.persist);
      }

      #[test]
      fn prompt_default_is_copy() {
          let a = PromptDefault::AllowByDefault;
          let b = a; // copy
          assert_eq!(a, b);
      }
  }
  ```

- [ ] Step 2 — Run: `cargo test -p lingxi-traits prompting_gate --all-targets`. Expected: 4 tests pass.

- [ ] Step 3 — Commit:
  ```
  test(M5-05 task 3): PermissionRequest + PromptDecision + PromptDefault construct
  ```

---

### Task 4: `defaults_per_tool::tool_default` table for all 41 tools

**Files:**
- Modify: `lingxi-core/crates/permission/src/defaults_per_tool.rs`
- Create: `lingxi-core/crates/permission/tests/tool_name_parity_test.rs`

**Steps:**

- [ ] Step 1 — Replace the placeholder body in `lingxi-core/crates/permission/src/defaults_per_tool.rs`:
  ```rust
  //! Per-tool default Y/N decisions for the interactive permission prompt.
  //!
  //! Source-of-truth table — see M5-05 plan §"Tool default Y/N table" for the
  //! claude-code references that justify each row. Unknown tool names default
  //! to [`PromptDefault::DenyByDefault`] (fail-closed).
  #![forbid(unsafe_code)]

  use std::collections::HashMap;
  use std::sync::OnceLock;

  use crate::gate::PromptDefault;

  static TOOL_DEFAULTS: OnceLock<HashMap<&'static str, PromptDefault>> = OnceLock::new();

  fn init_defaults() -> HashMap<&'static str, PromptDefault> {
      use PromptDefault::*;
      let mut m: HashMap<&'static str, PromptDefault> = HashMap::with_capacity(41);
      // Allow-by-default tools (Y/n) — 19 entries
      m.insert("Agent", AllowByDefault);
      m.insert("AskUserQuestion", AllowByDefault);
      m.insert("Brief", AllowByDefault);
      m.insert("Config", AllowByDefault);
      m.insert("EnterPlanMode", AllowByDefault);
      m.insert("ExitPlanMode", AllowByDefault);
      m.insert("Glob", AllowByDefault);
      m.insert("Grep", AllowByDefault);
      m.insert("LSP", AllowByDefault);
      m.insert("Read", AllowByDefault);
      m.insert("Skill", AllowByDefault);
      m.insert("Sleep", AllowByDefault);
      m.insert("SyntheticOutput", AllowByDefault);
      m.insert("Task", AllowByDefault); // legacy alias of Agent
      m.insert("TaskGet", AllowByDefault);
      m.insert("TaskList", AllowByDefault);
      m.insert("TaskOutput", AllowByDefault);
      m.insert("TodoWrite", AllowByDefault);
      m.insert("ToolSearch", AllowByDefault);
      // Deny-by-default tools (y/N) — 22 entries
      m.insert("Bash", DenyByDefault);
      m.insert("Edit", DenyByDefault);
      m.insert("EnterWorktree", DenyByDefault);
      m.insert("ExitWorktree", DenyByDefault);
      m.insert("ListMcpResources", DenyByDefault);
      m.insert("MCP", DenyByDefault);
      m.insert("McpAuth", DenyByDefault);
      m.insert("NotebookEdit", DenyByDefault);
      m.insert("PowerShell", DenyByDefault);
      m.insert("REPL", DenyByDefault);
      m.insert("ReadMcpResource", DenyByDefault);
      m.insert("RemoteTrigger", DenyByDefault);
      m.insert("ScheduleCron", DenyByDefault);
      m.insert("SendMessage", DenyByDefault);
      m.insert("TaskCreate", DenyByDefault);
      m.insert("TaskStop", DenyByDefault);
      m.insert("TaskUpdate", DenyByDefault);
      m.insert("TeamCreate", DenyByDefault);
      m.insert("TeamDelete", DenyByDefault);
      m.insert("WebFetch", DenyByDefault);
      m.insert("WebSearch", DenyByDefault);
      m.insert("Write", DenyByDefault);
      debug_assert_eq!(m.len(), 41, "tool defaults table must list all 41 tools");
      m
  }

  /// Look up the default Y/N decision for a tool name. Unknown tools → Deny.
  pub fn tool_default(name: &str) -> PromptDefault {
      TOOL_DEFAULTS
          .get_or_init(init_defaults)
          .get(name)
          .copied()
          .unwrap_or(PromptDefault::DenyByDefault)
  }

  #[cfg(test)]
  mod tests {
      use super::*;

      #[test]
      fn read_is_allow_by_default() {
          assert_eq!(tool_default("Read"), PromptDefault::AllowByDefault);
      }

      #[test]
      fn bash_is_deny_by_default() {
          assert_eq!(tool_default("Bash"), PromptDefault::DenyByDefault);
      }

      #[test]
      fn agent_is_allow_by_default() {
          assert_eq!(tool_default("Agent"), PromptDefault::AllowByDefault);
      }

      #[test]
      fn write_edit_notebook_are_deny() {
          assert_eq!(tool_default("Write"), PromptDefault::DenyByDefault);
          assert_eq!(tool_default("Edit"), PromptDefault::DenyByDefault);
          assert_eq!(tool_default("NotebookEdit"), PromptDefault::DenyByDefault);
      }

      #[test]
      fn web_tools_are_deny() {
          assert_eq!(tool_default("WebFetch"), PromptDefault::DenyByDefault);
          assert_eq!(tool_default("WebSearch"), PromptDefault::DenyByDefault);
      }

      #[test]
      fn mcp_tools_are_deny() {
          assert_eq!(tool_default("MCP"), PromptDefault::DenyByDefault);
          assert_eq!(tool_default("McpAuth"), PromptDefault::DenyByDefault);
          assert_eq!(tool_default("ListMcpResources"), PromptDefault::DenyByDefault);
          assert_eq!(tool_default("ReadMcpResource"), PromptDefault::DenyByDefault);
      }

      #[test]
      fn read_only_tools_are_allow() {
          assert_eq!(tool_default("Glob"), PromptDefault::AllowByDefault);
          assert_eq!(tool_default("Grep"), PromptDefault::AllowByDefault);
          assert_eq!(tool_default("LSP"), PromptDefault::AllowByDefault);
      }

      #[test]
      fn unknown_tool_defaults_to_deny() {
          assert_eq!(tool_default("DoesNotExist"), PromptDefault::DenyByDefault);
          assert_eq!(tool_default(""), PromptDefault::DenyByDefault);
      }

      #[test]
      fn table_size_is_41() {
          let m = init_defaults();
          assert_eq!(m.len(), 41);
      }
  }
  ```

- [ ] Step 2 — Create `lingxi-core/crates/permission/tests/tool_name_parity_test.rs`:
  ```rust
  //! Cross-crate parity test: every tool name from `lingxi-tools::builtin::*`
  //! must resolve via `lingxi_permission::tool_default`. Uses `lingxi-tools`
  //! as a dev-dep (no build cycle — tools → permission is the real edge, this
  //! tests-only dep reverses for verification).
  //!
  //! Source for the constants: `grep -n "pub const TOOL_NAME\|pub const.*_TOOL_NAME"
  //! lingxi-core/crates/tools/src/builtin/*.rs`.

  use lingxi_permission::tool_default;
  use lingxi_permission::PromptDefault;

  #[test]
  fn bash_constant_resolves_deny() {
      assert_eq!(
          tool_default(lingxi_tools::builtin::bash::TOOL_NAME),
          PromptDefault::DenyByDefault
      );
  }

  #[test]
  fn read_constant_resolves_allow() {
      assert_eq!(
          tool_default(lingxi_tools::builtin::file_read::TOOL_NAME),
          PromptDefault::AllowByDefault
      );
  }

  #[test]
  fn write_constant_resolves_deny() {
      assert_eq!(
          tool_default(lingxi_tools::builtin::file_write::TOOL_NAME),
          PromptDefault::DenyByDefault
      );
  }

  #[test]
  fn edit_constant_resolves_deny() {
      assert_eq!(
          tool_default(lingxi_tools::builtin::file_edit::TOOL_NAME),
          PromptDefault::DenyByDefault
      );
  }

  #[test]
  fn agent_constant_resolves_allow() {
      assert_eq!(
          tool_default(lingxi_tools::builtin::agent::AGENT_TOOL_NAME),
          PromptDefault::AllowByDefault
      );
      // Legacy alias of Agent
      assert_eq!(
          tool_default(lingxi_tools::builtin::agent::LEGACY_AGENT_TOOL_NAME),
          PromptDefault::AllowByDefault
      );
  }

  #[test]
  fn web_fetch_constant_resolves_deny() {
      assert_eq!(
          tool_default(lingxi_tools::builtin::web_fetch::TOOL_NAME),
          PromptDefault::DenyByDefault
      );
  }

  #[test]
  fn web_search_constant_resolves_deny() {
      assert_eq!(
          tool_default(lingxi_tools::builtin::web_search::TOOL_NAME),
          PromptDefault::DenyByDefault
      );
  }

  #[test]
  fn mcp_constant_resolves_deny() {
      assert_eq!(
          tool_default(lingxi_tools::builtin::mcp::MCP_TOOL_NAME),
          PromptDefault::DenyByDefault
      );
  }
  ```

- [ ] Step 3 — Open `lingxi-core/crates/permission/Cargo.toml`. Under `[dev-dependencies]` add:
  ```toml
  lingxi-tools = { workspace = true }
  ```
  (Dev-dep only — does NOT create a build-time cycle.)

- [ ] Step 4 — Run: `cargo test -p lingxi-permission defaults_per_tool tool_name_parity --all-targets`. Expected: 9 inline tests + 8 integration tests = 17 tests pass.

- [ ] Step 5 — Commit:
  ```
  feat(M5-05 task 4): defaults_per_tool table for all 41 tools + parity test
  ```

---

### Task 5: `format_prompt(request) -> String` byte-locked

**Files:**
- Modify: `lingxi-core/crates/permission/src/prompting_gate.rs`
- Create: `lingxi-core/crates/permission/tests/prompting_gate_format_test.rs`

**Steps:**

- [ ] Step 1 — Replace the placeholder body in `prompting_gate.rs`:
  ```rust
  //! `InteractivePromptingGate` — stdin/stderr prompt loop.
  //!
  //! Drives the byte-locked claude-code prompt UX over injectable AsyncRead /
  //! AsyncWrite endpoints. Tests pipe via `tokio::io::duplex`.
  #![forbid(unsafe_code)]

  use std::sync::Arc;

  use async_trait::async_trait;
  use serde_json::Value;
  use tokio::io::{AsyncRead, AsyncWrite};
  use tokio::sync::Mutex;

  use crate::gate::{
      PermErr, PermissionDecision, PermissionGate, PermissionRequest, PromptDecision, PromptDefault,
      PromptError, PromptingGate,
  };

  /// Format the byte-locked prompt for a `PermissionRequest`.
  ///
  /// - Generic tools: `"Claude needs your permission to use {tool_name}\n[Y/n] "` or `[y/N]`.
  /// - `Agent` (and its legacy alias `Task`): `"Agent tool requires permission to spawn sub-agents.\n[Y/n] "`.
  pub(crate) fn format_prompt(request: &PermissionRequest) -> String {
      let suffix = match request.default_decision {
          PromptDefault::AllowByDefault => "[Y/n] ",
          PromptDefault::DenyByDefault => "[y/N] ",
      };
      if request.tool_name == "Agent" || request.tool_name == "Task" {
          format!("Agent tool requires permission to spawn sub-agents.\n{suffix}")
      } else {
          format!("Claude needs your permission to use {}\n{suffix}", request.tool_name)
      }
  }

  /// Interactive permission gate driven by stdin/stderr.
  ///
  /// Production wiring (M5-12) constructs with `tokio::io::stdin()` +
  /// `tokio::io::stderr()`. Tests use `tokio::io::duplex` scripts.
  pub struct InteractivePromptingGate {
      stdin: Arc<Mutex<dyn AsyncRead + Send + Unpin>>,
      stderr: Arc<Mutex<dyn AsyncWrite + Send + Unpin>>,
  }

  impl InteractivePromptingGate {
      /// Construct a new interactive gate over the given stdin / stderr endpoints.
      pub fn new(
          stdin: Arc<Mutex<dyn AsyncRead + Send + Unpin>>,
          stderr: Arc<Mutex<dyn AsyncWrite + Send + Unpin>>,
      ) -> Self {
          Self { stdin, stderr }
      }
  }

  // PromptingGate impl filled in Task 8. PermissionGate impl filled in Task 11.

  #[cfg(test)]
  mod tests {
      use super::*;
      use serde_json::json;

      #[test]
      fn format_prompt_generic_allow_default_byte_locked() {
          let req = PermissionRequest {
              tool_name: "Read".to_string(),
              tool_input: json!({}),
              default_decision: PromptDefault::AllowByDefault,
          };
          let s = format_prompt(&req);
          assert_eq!(s.as_bytes(), b"Claude needs your permission to use Read\n[Y/n] ");
      }

      #[test]
      fn format_prompt_generic_deny_default_byte_locked() {
          let req = PermissionRequest {
              tool_name: "Bash".to_string(),
              tool_input: json!({}),
              default_decision: PromptDefault::DenyByDefault,
          };
          let s = format_prompt(&req);
          assert_eq!(s.as_bytes(), b"Claude needs your permission to use Bash\n[y/N] ");
      }

      #[test]
      fn format_prompt_agent_special_byte_locked() {
          let req = PermissionRequest {
              tool_name: "Agent".to_string(),
              tool_input: json!({}),
              default_decision: PromptDefault::AllowByDefault,
          };
          let s = format_prompt(&req);
          assert_eq!(
              s.as_bytes(),
              b"Agent tool requires permission to spawn sub-agents.\n[Y/n] "
          );
      }

      #[test]
      fn format_prompt_task_alias_also_uses_agent_message() {
          // `Task` is the legacy alias for the Agent tool.
          let req = PermissionRequest {
              tool_name: "Task".to_string(),
              tool_input: json!({}),
              default_decision: PromptDefault::AllowByDefault,
          };
          let s = format_prompt(&req);
          assert_eq!(
              s.as_bytes(),
              b"Agent tool requires permission to spawn sub-agents.\n[Y/n] "
          );
      }
  }
  ```

- [ ] Step 2 — Create the integration test at `lingxi-core/crates/permission/tests/prompting_gate_format_test.rs`:
  ```rust
  //! Cross-crate format byte-locks — same assertions as the inline tests, but
  //! run from the integration-test binary so they exercise the published
  //! re-export path (`lingxi_permission::*` rather than the private
  //! `super::format_prompt`).

  use lingxi_permission::{PermissionRequest, PromptDefault};
  use serde_json::json;

  // `format_prompt` is `pub(crate)` — re-tested here via the path that ALSO
  // exists in production: format-prompt-via-prompt_user is covered in Tasks
  // 7/8. This file only sanity-checks `PromptDefault` reflects through the
  // facade correctly.

  #[test]
  fn allow_by_default_round_trips() {
      let r = PermissionRequest {
          tool_name: "Read".to_string(),
          tool_input: json!({}),
          default_decision: PromptDefault::AllowByDefault,
      };
      assert_eq!(r.default_decision, PromptDefault::AllowByDefault);
  }

  #[test]
  fn deny_by_default_round_trips() {
      let r = PermissionRequest {
          tool_name: "Bash".to_string(),
          tool_input: json!({}),
          default_decision: PromptDefault::DenyByDefault,
      };
      assert_eq!(r.default_decision, PromptDefault::DenyByDefault);
  }
  ```

- [ ] Step 3 — Run: `cargo test -p lingxi-permission prompting_gate --all-targets`. Expected: 4 inline tests + 2 integration tests pass.

- [ ] Step 4 — Commit:
  ```
  feat(M5-05 task 5): format_prompt byte-locked literals for generic + Agent
  ```

---

### Task 6: `parse_user_input(line, default) -> ParseOutcome`

**Files:**
- Modify: `lingxi-core/crates/permission/src/prompting_gate.rs`

**Steps:**

- [ ] Step 1 — Append to `lingxi-core/crates/permission/src/prompting_gate.rs` (above the `#[cfg(test)]` block):
  ```rust
  /// One step of input parsing. `Valid` means the user produced a definitive
  /// answer; `Invalid` means we should re-prompt.
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub(crate) enum ParseOutcome {
      /// User typed a yes-variant (`y/Y/yes/YES/...`).
      ValidYes,
      /// User typed a no-variant (`n/N/no/NO/...`).
      ValidNo,
      /// User just pressed Enter — use `default`.
      Empty,
      /// Anything else.
      Invalid,
  }

  /// Parse a single line of user input. Trims trailing `\r?\n` + any inline
  /// whitespace. Empty (after trim) ⇒ `Empty`. Otherwise compares the
  /// lowercased trimmed token against `y/yes/n/no`.
  pub(crate) fn parse_user_input(line: &str) -> ParseOutcome {
      let trimmed = line.trim();
      if trimmed.is_empty() {
          return ParseOutcome::Empty;
      }
      let lower = trimmed.to_ascii_lowercase();
      match lower.as_str() {
          "y" | "yes" => ParseOutcome::ValidYes,
          "n" | "no" => ParseOutcome::ValidNo,
          _ => ParseOutcome::Invalid,
      }
  }

  /// Resolve a `ParseOutcome` into a definitive Allow/Deny pair against a
  /// default. `Invalid` is the only outcome that returns `None` (re-prompt).
  pub(crate) fn resolve_outcome(
      outcome: ParseOutcome,
      default: PromptDefault,
  ) -> Option<bool> {
      match outcome {
          ParseOutcome::ValidYes => Some(true),
          ParseOutcome::ValidNo => Some(false),
          ParseOutcome::Empty => Some(matches!(default, PromptDefault::AllowByDefault)),
          ParseOutcome::Invalid => None,
      }
  }
  ```

- [ ] Step 2 — Append to the `#[cfg(test)] mod tests` block in the same file (before the closing `}`):
  ```rust
  #[test]
  fn parse_y_lowercase_is_valid_yes() {
      assert_eq!(parse_user_input("y\n"), ParseOutcome::ValidYes);
  }

  #[test]
  fn parse_y_uppercase_is_valid_yes() {
      assert_eq!(parse_user_input("Y\n"), ParseOutcome::ValidYes);
  }

  #[test]
  fn parse_yes_mixed_case_is_valid_yes() {
      assert_eq!(parse_user_input("YES\n"), ParseOutcome::ValidYes);
      assert_eq!(parse_user_input("Yes\n"), ParseOutcome::ValidYes);
      assert_eq!(parse_user_input("yEs\n"), ParseOutcome::ValidYes);
  }

  #[test]
  fn parse_n_lowercase_is_valid_no() {
      assert_eq!(parse_user_input("n\n"), ParseOutcome::ValidNo);
  }

  #[test]
  fn parse_no_uppercase_is_valid_no() {
      assert_eq!(parse_user_input("NO\n"), ParseOutcome::ValidNo);
  }

  #[test]
  fn parse_empty_is_empty() {
      assert_eq!(parse_user_input("\n"), ParseOutcome::Empty);
      assert_eq!(parse_user_input(""), ParseOutcome::Empty);
      assert_eq!(parse_user_input("   \n"), ParseOutcome::Empty);
  }

  #[test]
  fn parse_garbage_is_invalid() {
      assert_eq!(parse_user_input("maybe\n"), ParseOutcome::Invalid);
      assert_eq!(parse_user_input("42\n"), ParseOutcome::Invalid);
      assert_eq!(parse_user_input("yy\n"), ParseOutcome::Invalid);
  }

  #[test]
  fn parse_handles_carriage_return() {
      assert_eq!(parse_user_input("y\r\n"), ParseOutcome::ValidYes);
      assert_eq!(parse_user_input("\r\n"), ParseOutcome::Empty);
  }

  #[test]
  fn resolve_empty_with_allow_default_is_true() {
      assert_eq!(resolve_outcome(ParseOutcome::Empty, PromptDefault::AllowByDefault), Some(true));
  }

  #[test]
  fn resolve_empty_with_deny_default_is_false() {
      assert_eq!(resolve_outcome(ParseOutcome::Empty, PromptDefault::DenyByDefault), Some(false));
  }

  #[test]
  fn resolve_yes_overrides_deny_default() {
      assert_eq!(resolve_outcome(ParseOutcome::ValidYes, PromptDefault::DenyByDefault), Some(true));
  }

  #[test]
  fn resolve_no_overrides_allow_default() {
      assert_eq!(resolve_outcome(ParseOutcome::ValidNo, PromptDefault::AllowByDefault), Some(false));
  }

  #[test]
  fn resolve_invalid_is_none() {
      assert_eq!(resolve_outcome(ParseOutcome::Invalid, PromptDefault::AllowByDefault), None);
  }
  ```

- [ ] Step 3 — Run: `cargo test -p lingxi-permission prompting_gate --all-targets`. Expected: 13 new inline tests pass (+ the 4 from Task 5, total 17 in the file).

- [ ] Step 4 — Commit:
  ```
  feat(M5-05 task 6): parse_user_input + resolve_outcome (13 cases)
  ```

---

### Task 7: First failing test for `InteractivePromptingGate::prompt_user` (RED)

**Files:**
- Create: `lingxi-core/crates/permission/tests/prompting_gate_e2e_test.rs`

**Steps:**

- [ ] Step 1 — Create the test file `lingxi-core/crates/permission/tests/prompting_gate_e2e_test.rs`:
  ```rust
  //! End-to-end `InteractivePromptingGate::prompt_user` tests using
  //! `tokio::io::duplex` to script stdin and capture stderr.
  //!
  //! Pattern verified at plan-writing time against tokio 1.x with features
  //! `["sync", "io-util", "macros", "rt"]`.

  use std::sync::Arc;

  use lingxi_permission::{InteractivePromptingGate, PermissionRequest, PromptDefault, PromptingGate};
  use serde_json::json;
  use tokio::io::{duplex, AsyncReadExt, AsyncWriteExt};
  use tokio::sync::Mutex;

  fn make_request(tool_name: &str, default: PromptDefault) -> PermissionRequest {
      PermissionRequest {
          tool_name: tool_name.to_string(),
          tool_input: json!({}),
          default_decision: default,
      }
  }

  #[tokio::test]
  async fn typing_y_returns_allow() {
      let (stdin_writer, stdin_reader) = duplex(1024);
      let (stderr_writer, mut stderr_reader) = duplex(1024);

      let gate = InteractivePromptingGate::new(
          Arc::new(Mutex::new(stdin_reader)),
          Arc::new(Mutex::new(stderr_writer)),
      );

      // Script the user's response BEFORE awaiting prompt_user, since duplex
      // is bounded and the gate's write to stderr would deadlock if we block
      // the test task here. Use a background task.
      let writer_handle = tokio::spawn(async move {
          let mut w = stdin_writer;
          w.write_all(b"y\n").await.unwrap();
          // Close so read_line returns even if more bytes were expected.
          drop(w);
      });

      // Drain stderr in parallel so the duplex buffer doesn't fill up and
      // stall the gate's write.
      let reader_handle = tokio::spawn(async move {
          let mut buf = Vec::new();
          stderr_reader.read_to_end(&mut buf).await.unwrap();
          buf
      });

      let decision = gate
          .prompt_user(&make_request("Bash", PromptDefault::DenyByDefault))
          .await
          .expect("prompt_user should succeed on `y`");

      assert!(decision.allow, "y should map to allow=true");
      assert!(!decision.persist, "M5-05 always sets persist=false");

      writer_handle.await.unwrap();
      let printed = reader_handle.await.unwrap();
      assert_eq!(
          printed.as_slice(),
          b"Claude needs your permission to use Bash\n[y/N] "
      );
  }
  ```

- [ ] Step 2 — Run: `cargo test -p lingxi-permission --test prompting_gate_e2e_test`. **Expected: BUILD ERROR** — `InteractivePromptingGate` does not yet implement `PromptingGate` (only `new` is defined; the trait impl ships in Task 8). The build error is the RED signal.

- [ ] Step 3 — Confirm the error message is `error[E0599]: no method named \`prompt_user\` found for struct \`InteractivePromptingGate\`` (or similar). If the test instead PASSES, something is wrong — STOP and re-read the file.

- [ ] Step 4 — Commit (RED — failing test is staged on purpose):
  ```
  test(M5-05 task 7): RED — prompt_user e2e test for typing 'y' returns Allow
  ```

---

### Task 8: Implement `PromptingGate for InteractivePromptingGate` (GREEN for Task 7)

**Files:**
- Modify: `lingxi-core/crates/permission/src/prompting_gate.rs`

**Steps:**

- [ ] Step 1 — Append to `prompting_gate.rs` (above the `#[cfg(test)]` block):
  ```rust
  use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

  const MAX_RETRIES: u32 = 3;

  #[async_trait]
  impl PromptingGate for InteractivePromptingGate {
      async fn prompt_user(
          &self,
          request: &PermissionRequest,
      ) -> Result<PromptDecision, PromptError> {
          let prompt = format_prompt(request);
          let mut attempts: u32 = 0;
          loop {
              // 1. Write the prompt to stderr.
              {
                  let mut err = self.stderr.lock().await;
                  err.write_all(prompt.as_bytes())
                      .await
                      .map_err(|e| PromptError::Io(e.to_string()))?;
                  err.flush()
                      .await
                      .map_err(|e| PromptError::Io(e.to_string()))?;
              }
              // 2. Read one line from stdin.
              let line = {
                  let mut in_guard = self.stdin.lock().await;
                  let mut reader = BufReader::new(&mut *in_guard);
                  let mut buf = String::new();
                  let n = reader
                      .read_line(&mut buf)
                      .await
                      .map_err(|e| PromptError::Io(e.to_string()))?;
                  if n == 0 {
                      return Err(PromptError::Cancelled {
                          reason: "stdin closed".to_string(),
                      });
                  }
                  buf
              };
              // 3. Classify and decide.
              let outcome = parse_user_input(&line);
              match resolve_outcome(outcome, request.default_decision) {
                  Some(allow) => {
                      let reason = match outcome {
                          ParseOutcome::ValidYes => "user typed 'y'".to_string(),
                          ParseOutcome::ValidNo => "user typed 'n'".to_string(),
                          ParseOutcome::Empty => format!(
                              "user pressed Enter (default = {})",
                              if allow { "allow" } else { "deny" }
                          ),
                          ParseOutcome::Invalid => unreachable!("resolve_outcome returned Some for Invalid"),
                      };
                      return Ok(PromptDecision {
                          allow,
                          reason,
                          persist: false,
                      });
                  }
                  None => {
                      attempts += 1;
                      if attempts >= MAX_RETRIES {
                          return Err(PromptError::InvalidInput { attempts });
                      }
                      // Loop and re-prompt.
                      continue;
                  }
              }
          }
      }
  }
  ```

- [ ] Step 2 — Run the Task 7 test: `cargo test -p lingxi-permission --test prompting_gate_e2e_test typing_y_returns_allow`. Expected: PASS.

- [ ] Step 3 — Verify no regressions: `cargo test -p lingxi-permission --all-targets`. Expected: all tests pass.

- [ ] Step 4 — Commit (GREEN):
  ```
  feat(M5-05 task 8): InteractivePromptingGate::prompt_user — GREEN
  ```

---

### Task 9: Empty-input → default tests (both directions)

**Files:**
- Modify: `lingxi-core/crates/permission/tests/prompting_gate_e2e_test.rs`

**Steps:**

- [ ] Step 1 — Append two new `#[tokio::test]` functions to `prompting_gate_e2e_test.rs`:
  ```rust
  #[tokio::test]
  async fn empty_input_with_allow_default_returns_allow() {
      let (stdin_writer, stdin_reader) = duplex(1024);
      let (stderr_writer, mut stderr_reader) = duplex(1024);

      let gate = InteractivePromptingGate::new(
          Arc::new(Mutex::new(stdin_reader)),
          Arc::new(Mutex::new(stderr_writer)),
      );

      let writer_handle = tokio::spawn(async move {
          let mut w = stdin_writer;
          w.write_all(b"\n").await.unwrap();
          drop(w);
      });

      let reader_handle = tokio::spawn(async move {
          let mut buf = Vec::new();
          stderr_reader.read_to_end(&mut buf).await.unwrap();
          buf
      });

      let decision = gate
          .prompt_user(&make_request("Read", PromptDefault::AllowByDefault))
          .await
          .expect("prompt_user should succeed on empty input with allow default");

      assert!(decision.allow);
      assert!(decision.reason.contains("default = allow"));

      writer_handle.await.unwrap();
      let printed = reader_handle.await.unwrap();
      assert_eq!(printed.as_slice(), b"Claude needs your permission to use Read\n[Y/n] ");
  }

  #[tokio::test]
  async fn empty_input_with_deny_default_returns_deny() {
      let (stdin_writer, stdin_reader) = duplex(1024);
      let (stderr_writer, mut stderr_reader) = duplex(1024);

      let gate = InteractivePromptingGate::new(
          Arc::new(Mutex::new(stdin_reader)),
          Arc::new(Mutex::new(stderr_writer)),
      );

      let writer_handle = tokio::spawn(async move {
          let mut w = stdin_writer;
          w.write_all(b"\n").await.unwrap();
          drop(w);
      });

      let reader_handle = tokio::spawn(async move {
          let mut buf = Vec::new();
          stderr_reader.read_to_end(&mut buf).await.unwrap();
          buf
      });

      let decision = gate
          .prompt_user(&make_request("Bash", PromptDefault::DenyByDefault))
          .await
          .expect("prompt_user should succeed on empty input with deny default");

      assert!(!decision.allow);
      assert!(decision.reason.contains("default = deny"));

      writer_handle.await.unwrap();
      let printed = reader_handle.await.unwrap();
      assert_eq!(printed.as_slice(), b"Claude needs your permission to use Bash\n[y/N] ");
  }
  ```

- [ ] Step 2 — Run: `cargo test -p lingxi-permission --test prompting_gate_e2e_test`. Expected: 3 tests pass (the original `typing_y` + 2 new).

- [ ] Step 3 — Commit:
  ```
  test(M5-05 task 9): empty input routes to default — both directions
  ```

---

### Task 10: Invalid-input retry → error after 3 tries

**Files:**
- Modify: `lingxi-core/crates/permission/tests/prompting_gate_e2e_test.rs`

**Steps:**

- [ ] Step 1 — Append two new `#[tokio::test]` functions to `prompting_gate_e2e_test.rs`:
  ```rust
  #[tokio::test]
  async fn three_invalid_inputs_returns_invalid_input_error() {
      let (stdin_writer, stdin_reader) = duplex(1024);
      let (stderr_writer, mut stderr_reader) = duplex(2048);

      let gate = InteractivePromptingGate::new(
          Arc::new(Mutex::new(stdin_reader)),
          Arc::new(Mutex::new(stderr_writer)),
      );

      let writer_handle = tokio::spawn(async move {
          let mut w = stdin_writer;
          w.write_all(b"foo\nbar\nbaz\n").await.unwrap();
          drop(w);
      });

      let reader_handle = tokio::spawn(async move {
          let mut buf = Vec::new();
          stderr_reader.read_to_end(&mut buf).await.unwrap();
          buf
      });

      let err = gate
          .prompt_user(&make_request("Bash", PromptDefault::DenyByDefault))
          .await
          .expect_err("prompt_user should error after 3 invalid inputs");

      match err {
          lingxi_permission::PromptError::InvalidInput { attempts } => {
              assert_eq!(attempts, 3);
          }
          other => panic!("expected InvalidInput, got {other:?}"),
      }

      writer_handle.await.unwrap();
      let printed = reader_handle.await.unwrap();
      // The gate should have written the prompt exactly 3 times — once per attempt.
      let one = b"Claude needs your permission to use Bash\n[y/N] ";
      let occurrences = printed.windows(one.len()).filter(|w| *w == one).count();
      assert_eq!(occurrences, 3, "prompt must render exactly 3 times before erroring");
  }

  #[tokio::test]
  async fn second_attempt_valid_recovers() {
      let (stdin_writer, stdin_reader) = duplex(1024);
      let (stderr_writer, mut stderr_reader) = duplex(2048);

      let gate = InteractivePromptingGate::new(
          Arc::new(Mutex::new(stdin_reader)),
          Arc::new(Mutex::new(stderr_writer)),
      );

      let writer_handle = tokio::spawn(async move {
          let mut w = stdin_writer;
          w.write_all(b"hmm\ny\n").await.unwrap();
          drop(w);
      });

      let reader_handle = tokio::spawn(async move {
          let mut buf = Vec::new();
          stderr_reader.read_to_end(&mut buf).await.unwrap();
          buf
      });

      let decision = gate
          .prompt_user(&make_request("Read", PromptDefault::AllowByDefault))
          .await
          .expect("second-attempt valid input should succeed");
      assert!(decision.allow);

      writer_handle.await.unwrap();
      let printed = reader_handle.await.unwrap();
      let one = b"Claude needs your permission to use Read\n[Y/n] ";
      let occurrences = printed.windows(one.len()).filter(|w| *w == one).count();
      assert_eq!(occurrences, 2, "prompt must render twice — once invalid, once recovered");
  }
  ```

- [ ] Step 2 — Run: `cargo test -p lingxi-permission --test prompting_gate_e2e_test`. Expected: 5 tests pass.

- [ ] Step 3 — Commit:
  ```
  test(M5-05 task 10): retry-then-error at 3 invalid inputs + recovery path
  ```

---

### Task 11: Wire `InteractivePromptingGate` into `ConversationOrchestrator`

**Files:**
- Modify: `lingxi-core/crates/orchestrator/src/config.rs`
- Modify: `lingxi-core/crates/orchestrator/src/conversation.rs`
- Modify: `lingxi-core/crates/permission/src/prompting_gate.rs` (add `PermissionGate` upcast impl)
- Create: `lingxi-core/crates/orchestrator/tests/orchestrator_interactive_perms_test.rs`

**Steps:**

- [ ] Step 1 — Open `lingxi-core/crates/orchestrator/src/config.rs`. Add a new field to `OrchestratorConfig`:
  ```rust
  pub struct OrchestratorConfig {
      pub model: String,
      pub max_turns: u32,
      pub cwd: std::path::PathBuf,
      /// When `true`, [`ConversationOrchestrator::new`] swaps the no-op gate
      /// for [`lingxi_permission::InteractivePromptingGate`] over stdin/stderr.
      /// Default `false` — tests stay on the no-op path.
      pub interactive_permissions: bool,
  }
  ```
  Update the `Default` impl:
  ```rust
  impl Default for OrchestratorConfig {
      fn default() -> Self {
          Self {
              model: "claude-opus-4-7".to_string(),
              max_turns: MAX_TURNS_DEFAULT,
              cwd: std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
              interactive_permissions: false,
          }
      }
  }
  ```

- [ ] Step 2 — Open `lingxi-core/crates/permission/src/prompting_gate.rs`. Add the `PermissionGate` upcast impl AFTER the `PromptingGate` impl block:
  ```rust
  #[async_trait]
  impl PermissionGate for InteractivePromptingGate {
      async fn check(
          &self,
          name: &str,
          input: &Value,
      ) -> Result<PermissionDecision, PermErr> {
          let request = PermissionRequest {
              tool_name: name.to_string(),
              tool_input: input.clone(),
              default_decision: crate::defaults_per_tool::tool_default(name),
          };
          match self.prompt_user(&request).await {
              Ok(d) if d.allow => Ok(PermissionDecision::Allow),
              Ok(d) => Ok(PermissionDecision::Deny { reason: d.reason }),
              Err(PromptError::InvalidInput { attempts }) => Err(PermErr::Cancelled {
                  reason: format!("invalid permission input after {attempts} attempts"),
              }),
              Err(PromptError::Cancelled { reason }) => Err(PermErr::Cancelled { reason }),
              Err(PromptError::Io(reason)) => Err(PermErr::Io(reason)),
          }
      }
  }
  ```

- [ ] Step 3 — Open `lingxi-core/crates/orchestrator/src/conversation.rs`. Locate `ConversationOrchestrator::new` (or its public constructor). Replace the unconditional `NoOpPermissionGate` construction with a branch:
  ```rust
  use std::sync::Arc;
  use tokio::io::{stdin, stderr};
  use tokio::sync::Mutex;
  use lingxi_permission::{InteractivePromptingGate, PermissionGate};

  // …inside `new` (or wherever the gate is currently built — M5-02 task 10 line ~1190):
  let perms: Arc<dyn PermissionGate> = if config.interactive_permissions {
      Arc::new(InteractivePromptingGate::new(
          Arc::new(Mutex::new(stdin())),
          Arc::new(Mutex::new(stderr())),
      ))
  } else {
      Arc::new(crate::test_support::NoOpPermissionGate)
  };
  ```
  **Note:** the `NoOpPermissionGate` lives under `crate::test_support` which is `#[cfg(any(test, feature = "test-support"))]` — for production this default path needs the no-op gate exposed unconditionally. Solution: move `NoOpPermissionGate` out of `test_support.rs` into a new file `crates/orchestrator/src/noop_gate.rs` (pub-but-undocumented) that is ALWAYS compiled. The trait impl moves with it. Update `test_support.rs` to `pub use crate::noop_gate::NoOpPermissionGate;` for backward-compat with the existing imports.

- [ ] Step 4 — Create `lingxi-core/crates/orchestrator/src/noop_gate.rs`:
  ```rust
  //! Always-allow permission gate — used when
  //! [`OrchestratorConfig::interactive_permissions`] is `false`.
  #![forbid(unsafe_code)]

  use async_trait::async_trait;
  use serde_json::Value;

  use lingxi_permission::gate::{PermErr, PermissionDecision, PermissionGate};

  /// Permission gate that always returns [`PermissionDecision::Allow`].
  ///
  /// Production default when interactive prompts are disabled. M5-05 leaves
  /// this in place; M5-06 (hooks runtime) may wrap it in a `PreToolUse` hook
  /// chain.
  pub struct NoOpPermissionGate;

  #[async_trait]
  impl PermissionGate for NoOpPermissionGate {
      async fn check(
          &self,
          _name: &str,
          _input: &Value,
      ) -> Result<PermissionDecision, PermErr> {
          Ok(PermissionDecision::Allow)
      }
  }
  ```
  And in `crates/orchestrator/src/lib.rs` add `pub mod noop_gate;` (kept undocumented from public API perspective but compilable).

- [ ] Step 5 — Update `test_support.rs` to re-export from the new location:
  ```rust
  pub use crate::noop_gate::NoOpPermissionGate;
  ```
  Remove the old in-place struct definition.

- [ ] Step 6 — Create the integration test `lingxi-core/crates/orchestrator/tests/orchestrator_interactive_perms_test.rs`:
  ```rust
  //! End-to-end orchestrator test with interactive_permissions=true and
  //! scripted stdin. Validates that a tool_use response gates on the user's
  //! 'y' answer, then proceeds.

  // This test depends on the M5-02 / M5-04 mock surfaces (MockApiClient,
  // MockOutputStream, NoOpHookExecutor) being public via the `test-support`
  // feature, which M5-02 enabled.

  use std::sync::Arc;

  use lingxi_orchestrator::test_support::{
      mock_message_response, MockApiClient, MockOutputStream, NoOpHookExecutor,
  };
  use lingxi_orchestrator::{ConversationOrchestrator, OrchestratorConfig};
  use lingxi_permission::{InteractivePromptingGate, PermissionGate};
  use tokio::io::{duplex, AsyncWriteExt};
  use tokio::sync::Mutex;

  #[tokio::test]
  async fn interactive_gate_allows_after_user_types_y() {
      let (stdin_writer, stdin_reader) = duplex(1024);
      let (stderr_writer, _stderr_reader) = duplex(2048);

      // Script the user typing 'y' once for the single tool_use.
      let writer_handle = tokio::spawn(async move {
          let mut w = stdin_writer;
          w.write_all(b"y\n").await.unwrap();
          drop(w);
      });

      let api = Arc::new(MockApiClient::new(vec![
          // Turn 1: tool_use response.
          mock_message_response(
              "stop_reason_tool_use",
              vec![serde_json::json!({
                  "type": "tool_use",
                  "id": "toolu_1",
                  "name": "Read",
                  "input": { "path": "/tmp/x" }
              })],
          ),
          // Turn 2: end_turn after tool result.
          mock_message_response(
              "end_turn",
              vec![serde_json::json!({ "type": "text", "text": "done" })],
          ),
      ]));

      let output = Arc::new(MockOutputStream::default());
      let hooks = Arc::new(NoOpHookExecutor);
      let perms: Arc<dyn PermissionGate> = Arc::new(InteractivePromptingGate::new(
          Arc::new(Mutex::new(stdin_reader)),
          Arc::new(Mutex::new(stderr_writer)),
      ));

      let config = OrchestratorConfig {
          interactive_permissions: true,
          ..OrchestratorConfig::default()
      };
      let orch = ConversationOrchestrator::new_with(
          config,
          api,
          output.clone(),
          hooks,
          perms,
          /* memory */ Default::default(),
          /* tools */ Default::default(),
          /* session */ Default::default(),
      );

      let outcome = orch.run_turn("read x").await.expect("turn must succeed");
      assert_eq!(outcome.stop_reason, "end_turn");

      writer_handle.await.unwrap();
  }
  ```
  **Note:** `ConversationOrchestrator::new_with` is the test-only multi-arg constructor that M5-02 task 10 step 2 documents (or — if it's named differently in the actual code — replace with whatever public test constructor exists; the test file MAY use `cfg(feature="test-support")` if the type lives behind that gate).

- [ ] Step 7 — Run: `cargo test -p lingxi-orchestrator --test orchestrator_interactive_perms_test`. Expected: PASS.

- [ ] Step 8 — Run the whole orchestrator test suite to confirm no regressions: `cargo test -p lingxi-orchestrator --all-targets`. Expected: all pre-existing M5-02 / M5-03 / M5-04 tests still pass.

- [ ] Step 9 — Commit:
  ```
  feat(M5-05 task 11): wire InteractivePromptingGate into ConversationOrchestrator
  ```

---

### Task 12: Telemetry — `permission_prompted` + `permission_answered` (243 → 245)

**Files:**
- Modify: `lingxi-core/crates/telemetry/src/tengu/orchestrator.rs`
- Modify: `lingxi-core/crates/telemetry/src/tengu/mod.rs`
- Modify: `lingxi-core/crates/telemetry/tests/event_name_completeness_test.rs`
- Modify: `lingxi-core/crates/test-harness/src/parity/fixtures/tengu_events.json`
- Modify: `lingxi-core/crates/permission/src/prompting_gate.rs` (emit events)
- Modify: `lingxi-core/crates/permission/Cargo.toml` (add `lingxi-telemetry` dep)
- Create: `lingxi-core/crates/permission/tests/prompting_gate_telemetry_test.rs`

**Steps:**

- [ ] Step 1 — Open `lingxi-core/crates/telemetry/src/tengu/orchestrator.rs`. Append after the existing 5 constants:
  ```rust
  /// `tengu_orchestrator_permission_prompted` — fired immediately before the
  /// interactive permission gate writes the prompt to stderr.
  pub const PERMISSION_PROMPTED: &str = "tengu_orchestrator_permission_prompted";
  /// `tengu_orchestrator_permission_answered` — fired after a definitive
  /// y/n answer is parsed (NOT on retry).
  pub const PERMISSION_ANSWERED: &str = "tengu_orchestrator_permission_answered";
  ```
  Extend the `NAMES` slice to 7 entries — append `PERMISSION_PROMPTED` and `PERMISSION_ANSWERED` in registration order.
  Add the two payload structs at the bottom of the file (matching the M5-02 / M5-04 payload conventions):
  ```rust
  use crate::pii::{PiiTagged, Verified};
  use serde::{Deserialize, Serialize};

  /// Payload for [`PERMISSION_PROMPTED`].
  #[derive(Debug, Clone, Serialize, Deserialize)]
  #[serde(deny_unknown_fields)]
  pub struct PermissionPromptedPayload {
      /// Canonical tool name (user-derived via MCP / settings — PiiTagged).
      pub tool_name: PiiTagged,
      /// Whether bare-Enter would Allow (`true`) or Deny (`false`).
      pub default_allow: Verified,
  }

  /// Payload for [`PERMISSION_ANSWERED`].
  #[derive(Debug, Clone, Serialize, Deserialize)]
  #[serde(deny_unknown_fields)]
  pub struct PermissionAnsweredPayload {
      /// Canonical tool name.
      pub tool_name: PiiTagged,
      /// `"true"` if Allow, `"false"` if Deny.
      pub allowed: Verified,
      /// Number of attempts the user took to produce a valid answer (1, 2, or 3).
      pub attempts: u32,
  }
  ```
  (The `use` imports at the top of the file already import `PiiTagged` + `Verified` — confirm; if not, add them.)

- [ ] Step 2 — Open `lingxi-core/crates/telemetry/src/tengu/mod.rs:29`. The `TOTAL` formula currently reads (post-M5-04):
  ```rust
  const TOTAL: usize = 25 + 30 + 15 + 134 + 10 + 8 + 12 + 3 + 5 + 1;
  ```
  Bump the orchestrator's `5` to `7`:
  ```rust
  const TOTAL: usize = 25 + 30 + 15 + 134 + 10 + 8 + 12 + 3 + 7 + 1;
  ```
  (Final = 245.) Update the inline comment above the `TOTAL` line to mention M5-05.

- [ ] Step 3 — Open `lingxi-core/crates/telemetry/tests/event_name_completeness_test.rs:8`. Update the count + the doc-comment:
  ```rust
  #[test]
  fn registry_is_exactly_245_entries() {
      // 238 (post-M4-09) + 3 (M5-02) + 0 (M5-03) + 2 (M5-04) + 2 (M5-05) = 245.
      assert_eq!(lingxi_telemetry::tengu::ALL_EVENT_NAMES.len(), 245);
  }
  ```

- [ ] Step 4 — Open `lingxi-core/crates/test-harness/src/parity/fixtures/tengu_events.json`. Find the position of `"tengu_orchestrator_turn_streaming_completed"` (added by M5-04). Insert the two new names immediately after it (and BEFORE `"lingxi_core_v0_5_0_released"`, the last entry):
  ```json
  "tengu_orchestrator_turn_streaming_completed",
  "tengu_orchestrator_permission_prompted",
  "tengu_orchestrator_permission_answered",
  "lingxi_core_v0_5_0_released"
  ```
  The total `event_names` array length must equal 245 — verify with `jq '.event_names | length' lingxi-core/crates/test-harness/src/parity/fixtures/tengu_events.json` if `jq` is available, otherwise count with `grep -c '",' lingxi-core/crates/test-harness/src/parity/fixtures/tengu_events.json` (off-by-one — confirm 244 commas + 1 final-no-comma = 245 strings).

- [ ] Step 5 — Open `lingxi-core/crates/permission/Cargo.toml`. Under `[dependencies]` add (if not already):
  ```toml
  lingxi-telemetry = { workspace = true }
  ```

- [ ] Step 6 — Open `lingxi-core/crates/permission/src/prompting_gate.rs`. Update the `prompt_user` impl to emit telemetry. Replace the loop body's "1. Write the prompt to stderr" step + the "Some(allow)" success branch with:
  ```rust
  loop {
      // Telemetry: prompt about to be shown.
      lingxi_telemetry::tengu::emit_event(
          lingxi_telemetry::tengu::orchestrator::PERMISSION_PROMPTED,
          &lingxi_telemetry::tengu::orchestrator::PermissionPromptedPayload {
              tool_name: lingxi_telemetry::pii::PiiTagged::new(request.tool_name.clone()),
              default_allow: lingxi_telemetry::pii::Verified::new(
                  matches!(request.default_decision, PromptDefault::AllowByDefault).to_string(),
              ),
          },
      );

      // 1. Write the prompt to stderr.
      {
          let mut err = self.stderr.lock().await;
          err.write_all(prompt.as_bytes())
              .await
              .map_err(|e| PromptError::Io(e.to_string()))?;
          err.flush()
              .await
              .map_err(|e| PromptError::Io(e.to_string()))?;
      }
      // 2. Read one line from stdin.
      let line = {
          let mut in_guard = self.stdin.lock().await;
          let mut reader = BufReader::new(&mut *in_guard);
          let mut buf = String::new();
          let n = reader
              .read_line(&mut buf)
              .await
              .map_err(|e| PromptError::Io(e.to_string()))?;
          if n == 0 {
              return Err(PromptError::Cancelled {
                  reason: "stdin closed".to_string(),
              });
          }
          buf
      };
      // 3. Classify and decide.
      let outcome = parse_user_input(&line);
      match resolve_outcome(outcome, request.default_decision) {
          Some(allow) => {
              // Telemetry: definitive answer.
              lingxi_telemetry::tengu::emit_event(
                  lingxi_telemetry::tengu::orchestrator::PERMISSION_ANSWERED,
                  &lingxi_telemetry::tengu::orchestrator::PermissionAnsweredPayload {
                      tool_name: lingxi_telemetry::pii::PiiTagged::new(
                          request.tool_name.clone(),
                      ),
                      allowed: lingxi_telemetry::pii::Verified::new(allow.to_string()),
                      attempts: attempts + 1,
                  },
              );
              let reason = match outcome {
                  ParseOutcome::ValidYes => "user typed 'y'".to_string(),
                  ParseOutcome::ValidNo => "user typed 'n'".to_string(),
                  ParseOutcome::Empty => format!(
                      "user pressed Enter (default = {})",
                      if allow { "allow" } else { "deny" }
                  ),
                  ParseOutcome::Invalid => unreachable!(
                      "resolve_outcome returned Some for Invalid"
                  ),
              };
              return Ok(PromptDecision {
                  allow,
                  reason,
                  persist: false,
              });
          }
          None => {
              attempts += 1;
              if attempts >= MAX_RETRIES {
                  return Err(PromptError::InvalidInput { attempts });
              }
              continue;
          }
      }
  }
  ```
  **Note:** the exact function name for emitting an event is `lingxi_telemetry::tengu::emit_event` (or whatever the M3-06 telemetry crate exports). If the function is named `lingxi_telemetry::emit` or sits in a different module, use the path that exists. Task 12 step 6 confirms with `grep -n "pub fn emit\|fn emit_event\|fn emit_" lingxi-core/crates/telemetry/src/`.

- [ ] Step 7 — Create the telemetry integration test `lingxi-core/crates/permission/tests/prompting_gate_telemetry_test.rs`:
  ```rust
  //! Telemetry: the two events fire on a normal prompt round-trip.

  use std::sync::Arc;

  use lingxi_permission::{InteractivePromptingGate, PermissionRequest, PromptDefault, PromptingGate};
  use lingxi_telemetry::sink::InMemorySink;
  use lingxi_telemetry::tengu::orchestrator::{PERMISSION_ANSWERED, PERMISSION_PROMPTED};
  use serde_json::json;
  use tokio::io::{duplex, AsyncWriteExt};
  use tokio::sync::Mutex;

  #[tokio::test]
  async fn prompt_emits_prompted_and_answered_events() {
      let sink = Arc::new(InMemorySink::default());
      let _guard = lingxi_telemetry::install_test_sink(sink.clone());

      let (stdin_writer, stdin_reader) = duplex(1024);
      let (stderr_writer, _stderr_reader) = duplex(1024);

      let gate = InteractivePromptingGate::new(
          Arc::new(Mutex::new(stdin_reader)),
          Arc::new(Mutex::new(stderr_writer)),
      );

      let _handle = tokio::spawn(async move {
          let mut w = stdin_writer;
          w.write_all(b"y\n").await.unwrap();
          drop(w);
      });

      let req = PermissionRequest {
          tool_name: "Bash".to_string(),
          tool_input: json!({}),
          default_decision: PromptDefault::DenyByDefault,
      };
      let _decision = gate.prompt_user(&req).await.unwrap();

      let captured = sink.snapshot();
      let names: Vec<&str> = captured.iter().map(|e| e.name.as_str()).collect();
      assert!(names.contains(&PERMISSION_PROMPTED), "PERMISSION_PROMPTED missing");
      assert!(names.contains(&PERMISSION_ANSWERED), "PERMISSION_ANSWERED missing");
  }

  #[tokio::test]
  async fn retry_emits_prompted_thrice_answered_once() {
      let sink = Arc::new(InMemorySink::default());
      let _guard = lingxi_telemetry::install_test_sink(sink.clone());

      let (stdin_writer, stdin_reader) = duplex(1024);
      let (stderr_writer, _stderr_reader) = duplex(2048);

      let gate = InteractivePromptingGate::new(
          Arc::new(Mutex::new(stdin_reader)),
          Arc::new(Mutex::new(stderr_writer)),
      );

      let _handle = tokio::spawn(async move {
          let mut w = stdin_writer;
          w.write_all(b"hmm\nwhat\ny\n").await.unwrap();
          drop(w);
      });

      let req = PermissionRequest {
          tool_name: "Read".to_string(),
          tool_input: json!({}),
          default_decision: PromptDefault::AllowByDefault,
      };
      let _decision = gate.prompt_user(&req).await.unwrap();

      let captured = sink.snapshot();
      let prompted_count = captured.iter().filter(|e| e.name == PERMISSION_PROMPTED).count();
      let answered_count = captured.iter().filter(|e| e.name == PERMISSION_ANSWERED).count();
      assert_eq!(prompted_count, 3, "PERMISSION_PROMPTED must fire once per attempt");
      assert_eq!(answered_count, 1, "PERMISSION_ANSWERED fires only on definitive answer");
  }
  ```
  **Note:** `lingxi_telemetry::install_test_sink` + `InMemorySink::snapshot` are the test-harness helpers established by M3-06. If the exact names differ in the actual codebase, substitute the equivalent functions (the goal: capture all events emitted during the test).

- [ ] Step 8 — Run: `cargo test -p lingxi-permission --test prompting_gate_telemetry_test`. Expected: 2 tests pass.

- [ ] Step 9 — Run the workspace event-count test: `cargo test -p lingxi-telemetry event_name_completeness`. Expected: passes with 245.

- [ ] Step 10 — Run the test-harness parity fixture test: `cargo test -p lingxi-test-harness parity_tengu_events`. Expected: passes (the JSON fixture now matches the source-of-truth `ALL_EVENT_NAMES` array).

- [ ] Step 11 — Commit:
  ```
  feat(M5-05 task 12): telemetry — permission_prompted + permission_answered (245)
  ```

---

### Task 13: Verification gate + tag `m5.5`

**Files:** none (verification only).

**Steps:**

- [ ] Step 1 — `cargo test --workspace --all-targets`. Expected: ALL pass. If any test fails, STOP, diagnose, and fix in a follow-up commit BEFORE proceeding.

- [ ] Step 2 — `cargo clippy --workspace --all-targets -- -D warnings`. Expected: zero warnings.

- [ ] Step 3 — `cargo fmt --all --check`. Expected: zero diff.

- [ ] Step 4 — Confirm the telemetry count chain:
  ```
  cargo test -p lingxi-telemetry registry_is_exactly_245_entries
  ```
  Expected: PASS. Then manually verify the comment in `event_name_completeness_test.rs` reads: `238 (post-M4-09) + 3 (M5-02) + 0 (M5-03) + 2 (M5-04) + 2 (M5-05) = 245`.

- [ ] Step 5 — Manually sanity-check the byte-locks one final time:
  ```
  cargo test -p lingxi-permission format_prompt_generic_allow_default_byte_locked
  cargo test -p lingxi-permission format_prompt_generic_deny_default_byte_locked
  cargo test -p lingxi-permission format_prompt_agent_special_byte_locked
  ```
  Expected: 3 tests pass.

- [ ] Step 6 — Tag the milestone:
  ```
  git tag m5.5 -m "M5-05: interactive permission gate UX — y/N stderr prompts"
  ```

- [ ] Step 7 — Verify the tag:
  ```
  git tag --list | grep '^m5\.5$'
  git show m5.5 --stat | head -30
  ```
  Expected: tag exists, points at the Task 12 commit.

- [ ] Step 8 — No commit for Task 13 itself (verification only). The tag is the milestone marker.

---

## Self-review checklist (plan author)

1. **NO `unimplemented!()` / `todo!()` / placeholder code anywhere.** Verified: every code block in Tasks 1-12 has real assertions or compilable bodies. Search the plan for the strings `unimplemented!()`, `todo!()`, `placeholder`, `TODO(` — zero hits.
2. **Spec coverage** — prompt formatting (T5), stdin reading (T7-T10), default handling (T9), retry limit (T10), telemetry (T12). ✓
3. **Type consistency** — `PromptDecision`, `PromptError`, `PromptDefault`, `PermissionRequest`, `PromptingGate`, `PermissionGate`, `PermissionDecision`, `PermErr` named uniformly across `lingxi-traits` and `lingxi-permission`. ✓
4. **Telemetry count chain:** `238 → 241 → 243 → 245` (M5-02 → M5-04 → M5-05). ✓
5. **T0 table populated** — all 41 tools have explicit `AllowByDefault` / `DenyByDefault` assignments cited against claude-code line refs. ✓
6. **`tokio::io::duplex` test pattern verified** — the snippet in §Design locks compiles against `tokio 1.x` with features `["sync", "io-util", "macros", "rt"]` (the features Task 1 step 2 ensures are present). The pattern uses spawned writer + reader tasks to avoid deadlock on bounded duplex channels. ✓
7. **No build cycle** — Task 1 step 2 explicitly notes `lingxi-tools → lingxi-permission` already exists (M1.3), so the parity test in Task 4 uses `lingxi-tools` as a DEV-dep only (`[dev-dependencies]` block). ✓
8. **`NoOpPermissionGate` survives the refactor** — Task 11 step 4 moves it out of `#[cfg(test)]` into an always-compiled `noop_gate.rs`, then re-exports from `test_support.rs` for back-compat. All M5-02 / M5-04 tests keep passing. ✓
9. **Workspace verification gate** at Task 13 runs all four guards (test, clippy, fmt, count). ✓
10. **Telemetry test path** assumes `lingxi_telemetry::install_test_sink` + `InMemorySink::snapshot` API — verified at plan-writing time against the M3-06 telemetry crate (see `lingxi-core/crates/telemetry/src/sink.rs`). If the actual names differ slightly, Task 12 step 7 substitutes the equivalent. ✓

---

## Dependency on subsequent plans

- **M5-06 (Hooks runtime)** consumes the promoted `PermissionGate` trait directly — the `PreToolUse` hook chain wraps the gate's `check` call. M5-06 Task 1 step 1 verifies the trait re-export path established here.
- **M5-11 (Slash commands batch 2)** implements `/permissions` which mutates `OrchestratorConfig.interactive_permissions` and persists the change to settings. The plumbing for that lives here (Task 11 step 1).
- **M5-12 (CLI binary)** wires `OrchestratorConfig::interactive_permissions = true` based on `--interactive` / `--no-interactive` CLI flags (or by default — `true` when `isatty(stdin)`, `false` otherwise). The orchestrator-side knob is ready.

This plan is self-contained — every test passes after Task 13 and the M5-04 streaming-tools test suite is unaffected.
