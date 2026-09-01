# LingXi Core M4 — Tools 全集 Implementation Design

> **Status**: DRAFT (awaiting user review)
> **Date**: 2026-05-24
> **Target version**: v0.5.0
> **Predecessor**: M3 / v0.4.0 (Settings + Memory + API client + OAuth + Cost events + Telemetry schema)
> **Successors**: M5 (Agent surface + Commands + Hooks) → M6 (Plugin marketplace + MCP server + /doctor + Cron + UniFFI polish)

---

## §1 Goal & non-goals

### Goal

Implement all **40 concrete tool types** from claude-code's `src/tools/` directory as Rust `impl Tool for XxxTool` implementations, producing v0.5.0:

1. **File operations** (5 tools): FileReadTool, FileWriteTool, FileEditTool, NotebookEditTool, GlobTool
2. **Search** (1 tool): GrepTool (via ripgrep)
3. **Shell** (4 tools): BashTool, PowerShellTool, REPLTool, SleepTool
4. **Web** (2 tools): WebFetchTool, WebSearchTool
5. **Workflow** (5 tools): TodoWriteTool, EnterPlanModeTool, ExitPlanModeTool, EnterWorktreeTool, ExitWorktreeTool
6. **Agent orchestration** (8 tools): AgentTool, TaskCreateTool, TaskGetTool, TaskListTool, TaskUpdateTool, TaskStopTool, TaskOutputTool, SendMessageTool
7. **Team** (2 tools): TeamCreateTool, TeamDeleteTool
8. **MCP + LSP** (5 tools): MCPTool, McpAuthTool, ListMcpResourcesTool, ReadMcpResourceTool, LSPTool
9. **System** (8 tools): AskUserQuestionTool, BriefTool, ConfigTool, SkillTool, ScheduleCronTool, ToolSearchTool, RemoteTriggerTool, SyntheticOutputTool

Every tool ships with byte-aligned input/output schemas, byte-aligned error strings, hermetic test coverage via M1 trait injection, and a `tengu_tool_*` telemetry event triple (`started`/`succeeded`/`failed`).

### Non-goals (deferred to later milestones)

- **Slash commands** (`/clear`, `/compact`, `/memory`, `/init`, `/resume`, `/cost`, `/doctor`, ...) → **M5**
- **Hooks lifecycle** (PreToolUse / PostToolUse / UserPromptSubmit / Stop / SubagentStop / Notification / SessionStart / PreCompact) — M4 emits the trigger surface; M5 wires actual hook execution → **M5**
- **Agent prompt templates** byte-aligned (system message, persona, sub-agent prompts) → **M5** (M4 provides the agent_loop machinery the prompts feed into)
- **Permission UI** (`PromptingGate` real implementation that asks the user) → **M5** (M4 ships `AllowAllGate` default + `PermissionGate` trait surface)
- **Plugin marketplace** (discover / install / update / manifest validation) → **M6**
- **MCP server side** (exposing LingXi as MCP server) → **M6**
- **`/doctor` diagnostic command** → **M6** (uses M4 tool registry to enumerate available tools)
- **UI / Terminal rendering** (out of scope entirely)
- **Mobile real-device binding** (only cross-compile gates remain)
- **claude.com cloud features** (BriefTool upload to claude.com servers, RemoteTrigger cloud-side) — M4 ships **local-only** stubs; cloud wire would be M6+

### What changed since M3

M3 v0.4.0 landed the engine: full Anthropic API client (with retry+jitter+OAuth hook), proactive+reactive OAuth refresh, 4-layer settings merge, CLAUDE.md hierarchy + memdir relevance ranking, cost event emission, 143-event telemetry schema. The engine can _talk to Anthropic_ and _maintain context_, but it has **zero usable tools**. M4 fills in the tool surface so an agent loop has something to dispatch.

---

## §2 Scope decomposition: 9 sub-plans

Single M4 milestone, **strict-order execution** (per user choice):

```
v0.4.0 (now)  ──►  M4-01  ──►  M4-02  ──►  M4-03  ──►  M4-04  ──►  M4-05  ──►  M4-06  ──►  M4-07  ──►  M4-08  ──►  M4-09 (v0.5.0)
                   File +     Shell       Web         Workflow    Agent +     Team        MCP +       System       Release
                   Search                                          Task                    LSP
                   (6 tools)   (4)         (2)         (5)         (8)         (2)         (5)         (8)         tag v0.5.0
```

**Total**: 40 tools across 9 sub-plans, **~12-16 weeks single-developer**.

Each sub-plan is independently committable, independently verifiable, and produces its own milestone tag (`m4.1` through `m4.9`). The final `v0.5.0` release tag annotates the entire chain.

### Why this ordering

Dependency progression:
- **Foundation first** — file/search tools have zero cross-tool dependencies; agent/task tools depend on file ops being present (Agent reads CLAUDE.md, writes task output)
- **Shell next** — Bash uses Process trait (M1), unlocks demos; depends on M2-04 sandbox + M2-06 process polish (all done)
- **Web after shell** — WebFetch uses Http trait (M1); WebSearch routes through api-client (M3-03)
- **Workflow needs file + shell** — TodoWrite persists via session (M1), worktree tools need M2-01 path layout
- **Agent + Task come after foundation** — Agent spawns subagent loops that re-enter the registry (recursion); needs all prior tools available
- **Team is small** — uses memory paths (M3-02)
- **MCP + LSP** — wire through M2-02b/M2-03 clients
- **System last** — most diverse group; some (Skill, Brief) need M1 crate scaffolding present but no cross-tool deps

---

## §3 Architecture & components

### Crate layout (no new crates; extend `lingxi-tools`)

```
lingxi-code/crates/tools/
├── src/
│   ├── lib.rs                          # (extend) re-export all builtin tools
│   ├── tool_trait.rs                   # (existing M1) Tool trait + ToolCallResult + ToolError
│   ├── registry.rs                     # (existing M1) ToolRegistry
│   ├── dispatcher.rs                   # (existing M1) dispatch by tool name
│   ├── permissions.rs                  # (existing M1) PermissionGate trait + AllowAllGate
│   ├── streaming_exec.rs               # (existing M1)
│   ├── progress.rs                     # (existing M1)
│   ├── result_storage.rs               # (existing M1)
│   ├── content_replacement.rs          # (existing M1)
│   ├── context.rs                      # (existing M1) ToolStaticContext
│   ├── builtin/                        # NEW M4: all 40 tool implementations
│   │   ├── mod.rs                      # register_all_builtin_tools(registry)
│   │   ├── file_read.rs                # M4-01
│   │   ├── file_write.rs               # M4-01
│   │   ├── file_edit.rs                # M4-01
│   │   ├── notebook_edit.rs            # M4-01
│   │   ├── glob.rs                     # M4-01
│   │   ├── grep.rs                     # M4-01
│   │   ├── bash.rs                     # M4-02
│   │   ├── powershell.rs               # M4-02
│   │   ├── repl.rs                     # M4-02
│   │   ├── sleep.rs                    # M4-02
│   │   ├── web_fetch.rs                # M4-03
│   │   ├── web_search.rs               # M4-03
│   │   ├── todo_write.rs               # M4-04
│   │   ├── plan_mode.rs                # M4-04 (EnterPlanMode + ExitPlanMode)
│   │   ├── worktree.rs                 # M4-04 (EnterWorktree + ExitWorktree)
│   │   ├── agent.rs                    # M4-05 (AgentTool)
│   │   ├── task.rs                     # M4-05 (TaskCreate/Get/List/Update/Stop/Output — 6 in one file)
│   │   ├── send_message.rs             # M4-05
│   │   ├── team.rs                     # M4-06 (TeamCreate + TeamDelete)
│   │   ├── mcp.rs                      # M4-07 (MCPTool + McpAuth + ListMcpResources + ReadMcpResource)
│   │   ├── lsp.rs                      # M4-07
│   │   ├── ask_user_question.rs        # M4-08
│   │   ├── brief.rs                    # M4-08 (local-only stub)
│   │   ├── config.rs                   # M4-08
│   │   ├── skill.rs                    # M4-08
│   │   ├── schedule_cron.rs            # M4-08
│   │   ├── tool_search.rs              # M4-08
│   │   ├── remote_trigger.rs           # M4-08 (local-only stub)
│   │   └── synthetic_output.rs         # M4-08
│   └── shared/                         # NEW M4: cross-tool helpers
│       ├── mod.rs
│       ├── file_kit.rs                 # binary detection, encoding, BOM, line-ending
│       ├── path_validation.rs          # trusted dirs check, traversal guard
│       ├── output_truncation.rs        # MAX_TOOL_OUTPUT_LENGTH applier
│       └── ansi_strip.rs               # for Bash output cleaning
```

**Total LOC estimate:** ~12,000 new functional + ~10,000 test = **~22,000 LOC**.

### Cross-crate dependencies

| Tool group | Depends on |
|------------|------------|
| File ops | `lingxi-platform_api::FileSystem` (M1), `lingxi-permission::PermissionGate` (M1) |
| Shell | `lingxi-platform_api::Process` (M1), `lingxi-sandbox` (M2-04), `lingxi-cost::events` (M3-05) |
| Web | `lingxi-platform_api::Http` (M1), `lingxi-api-client` (M3-03 for WebSearch), `lingxi-cost::events` (M3-05), `lingxi-telemetry::tengu::tool` (M3-06) |
| Workflow | `lingxi-session` (M1), `lingxi-platform_api::Worktree` (M2-01) |
| Agent/Task | `lingxi-agent` (M1), `lingxi-coordinator` (M1), `lingxi-tasks` (M1), `lingxi-api-client` (M3-03, for sub-engine API calls), `lingxi-anthropic-oauth` (M3-04, for sub-engine OAuth) |
| MCP | `lingxi-mcp` (M2-02b), `lingxi-jsonrpc` (M2-02a) |
| LSP | `lingxi-lsp` (M2-03) |
| System | `lingxi-skills` (M1), `lingxi-commands` (M1), `lingxi-cron` (M1) |

No new crates created; `lingxi-tools` grows to ~22 KLOC.

---

## §4 Data flow

### Flow A — Generic tool dispatch (every tool)

```
caller (agent loop / coordinator):
    │
    ▼ ToolUseRequest { name, input, request_id }
    │
    ToolRegistry::dispatch(name, input, ctx):
       │
       ├─► 1. tool = registry.lookup(name) → &dyn Tool   [Err: UnknownTool]
       ├─► 2. tool.validate_input(input)                  [Err: ValidationError]
       ├─► 3. permission_gate.check(name, &input, ctx)   [Err: Denied]
       │     [emits tengu_tool_permission_{requested,granted,denied}]
       ├─► 4. emit tengu_tool_<name>_started { request_id, input_hash }
       ├─► 5. tool.execute(input, ctx).await              ← real work
       │       (side effects via injected traits — FileSystem, Process, Http, etc.)
       ├─► 6a. on Ok: emit tengu_tool_<name>_succeeded { duration_ms, output_bytes }
       │       output_truncation::truncate(result, MAX_TOOL_OUTPUT_LENGTH)
       │       cost_tracker.record_tool_invocation (if applicable)
       ├─► 6b. on Err: emit tengu_tool_<name>_failed { error_kind, duration_ms }
       └─► 7. return ToolCallResult to caller
```

### Flow B — File operations (Read/Write/Edit) byte-aligned

```
FileReadTool.execute({ file_path, offset?, limit? }):
    │
    ├─► path_validation::canonicalize + assert in trusted_directories
    │     (emits tengu_file_path_blocked on reject)
    ├─► fs.read_metadata(path)
    │     ├─► if size > MAX_FILE_READ_SIZE (262_144 bytes / 256 KB):
    │     │     return ToolError::FileTooLarge { size, limit }
    │     └─► if is_binary (first 8KB has NUL byte):
    │           return ToolError::BinaryFile { path, reason }
    ├─► fs.read_text(path, encoding=auto)  ← UTF-8 BOM detection
    ├─► apply offset/limit (1-based line indexing per claude-code semantics)
    └─► return content + line_range + total_lines
```

### Flow C — BashTool (sandbox + cost + output cap)

```
BashTool.execute({ command, timeout_ms?, run_in_background? }):
    │
    ├─► sandbox::should_use_sandbox(&command, &settings)  ← M2-04 decision
    │     ├─► YES: wrap_with_sandbox(command, &policy)
    │     └─► NO:  raw ProcessCommand
    ├─► process_runner.spawn_managed(cmd, timeout)
    │     (StderrRing 64MB cap from M2-06; setsid + killpg tree-kill on timeout)
    ├─► stream stdout/stderr → output_truncation (MAX_TOOL_OUTPUT_LENGTH = 30_000 chars)
    ├─► strip_ansi(output)
    ├─► record exit_code, duration_ms, sandbox_kind
    ├─► cost_tracker.record_tool_invocation (M3-05; tool-level cost is no-op now,
    │     reserved hook for M4-09 cost-of-tools telemetry)
    └─► emits tengu_tool_bash_{started,succeeded,failed}
```

### Flow D — Agent + Task orchestration

```
AgentTool.execute({ subagent_type, prompt }):
    │
    ├─► coordinator.spawn_subagent(subagent_type, prompt) ← M1 coordinator
    │     │  (creates child Engine with own session)
    │     ├─► sub_engine.init() — reads settings (M3-01), loads memory (M3-02)
    │     ├─► sub_engine.run_loop()
    │     │     ├─► api_client.messages_create (M3-03 retry+jitter+oauth)
    │     │     ├─► tool dispatches via THIS same registry (recursion)
    │     │     └─► budget check (M3-05) — inherits parent budget
    │     └─► sub_engine.shutdown() ← M3-04 OAuth task canceled
    └─► return subagent's final ToolCallResult
        (+ tengu_agent_subagent_completed)


TaskCreateTool.execute({ task }):
    └─► tasks::TaskManager.create(task)  ← M1 tasks crate
        (persists to ~/.claude/tasks/<task_id>/)
        emits tengu_tool_task_create_succeeded { task_id }


TaskListTool.execute({}):
    └─► tasks::TaskManager.list().filter(visible_to(session))


// Task{Get,Update,Stop,Output} follow same pattern: thin wrappers over TaskManager
```

---

## §5 Error handling

### Extended `ToolError` enum

```rust
// lingxi-tools/src/tool_trait.rs (extend existing enum)
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ToolError {
    /* existing M1 variants */
    #[error("unknown tool: {0}")]
    UnknownTool(String),
    #[error("validation error: {0}")]
    ValidationError(#[from] ValidationError),
    #[error("permission denied: {0}")]
    Denied(String),

    /* NEW M4 variants */
    #[error("file too large: {size} bytes exceeds limit {limit}")]
    FileTooLarge { size: u64, limit: u64 },
    #[error("path not in trusted directory: {path:?}")]
    PathBlocked { path: PathBuf },
    #[error("binary file detected at {path:?} ({reason})")]
    BinaryFile { path: PathBuf, reason: &'static str },
    #[error("command timed out after {timeout_ms}ms")]
    Timeout { timeout_ms: u64 },
    #[error("output truncated at {limit} chars")]
    OutputTruncated { limit: usize },
    #[error("subagent failed: {0}")]
    SubagentFailed(String),
    #[error("MCP tool error: {server}/{tool}: {detail}")]
    McpFailure { server: String, tool: String, detail: String },
    #[error("LSP tool error: {0}")]
    LspFailure(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("transport error: {0}")]
    Transport(String),
}
```

### Per-category error mapping

| Tool group | Trigger | Error variant | User-facing string (byte-aligned w/ claude-code) |
|------------|---------|--------------|--------------------------------------------------|
| File | path traversal | `PathBlocked` | `"File path {path} is outside trusted directories"` |
| File | size > 256 KB | `FileTooLarge` | `"File {path} ({size}B) exceeds 256KB read limit"` |
| File | NUL bytes in first 8KB | `BinaryFile` | `"File {path} appears to be binary (first 8KB contains NUL bytes)"` |
| File | non-UTF-8 | `Io` (wraps utf8 error) | `"File {path} is not valid UTF-8"` |
| Shell | command not found | `Io(NotFound)` | `"Command '{cmd}' not found"` |
| Shell | timeout | `Timeout` | `"Bash command timed out after {N}ms"` |
| Shell | exit ≠ 0 | `ToolCallResult.is_error = true` (NOT `Err`) | result.stderr surfaced normally |
| Shell | sandbox unavailable on Win | `ValidationError` | `"sandbox not supported on this platform"` (M2-04 lock) |
| Web | HTTP 4xx/5xx | `Transport` | `"WebFetch: HTTP {status} from {url}"` |
| Web | DNS fail | `Transport` | `"WebFetch: cannot resolve {host}"` |
| Web | content > 5 MB | `OutputTruncated` | `"\n\n[Content truncated due to length...]"` (suffix) |
| Agent | subagent loop crash | `SubagentFailed` | `"Subagent '{type}' failed: {reason}"` |
| Agent | budget exceeded mid-run | `Denied` (cost gate) | `"Budget exceeded ($X.XX); stopped."` (M3-05 lock) |
| MCP | server timeout | `McpFailure` | `"MCP server \"{server}\" tool \"{tool}\" timed out after {N}s"` (M2-02b lock) |
| MCP | server disconnected | `McpFailure` | `"MCP server '{name}' is not connected"` |
| LSP | server down | `LspFailure` | `"LSP server '{name}' is not running"` |

### claude-code source citations (parity fixture references)

| Tool | Locked string | Source file:line |
|------|---------------|------------------|
| BashTool | `"Bash command timed out after {N}ms"` | `claude-code/src/tools/BashTool/prompt.ts` |
| FileReadTool | `"File {path} ({size}B) exceeds 256KB read limit"` | `claude-code/src/tools/FileReadTool/utils.ts:158` |
| WebFetchTool | `"\n\n[Content truncated due to length...]"` | `claude-code/src/tools/WebFetchTool/utils.ts:529` |
| FileEditTool | `"\n\n... [{N} lines truncated] ..."` | `claude-code/src/tools/FileEditTool/utils.ts:405` |
| MCP timeout | `"MCP server \"{server}\" tool \"{tool}\" timed out after {N}s"` | already locked in M2-02b |
| Budget exceeded | `"Budget exceeded ($X.XX); stopped."` | already locked in M3-05 |

All locks live in `parity_tool_error_strings.json` (cross-cutting M4 fixture).

### Recovery strategy

- **Validation errors** → return immediately; user fixes input.
- **Permission denied** → return immediately; caller asks user separately via `PromptingGate` (M5).
- **Transient transport errors** (HTTP 5xx, MCP server crash) → tool returns `Err`; **caller decides retry**. Tools never self-retry — only `messages_create` retries (M3-03).
- **Cost budget exceeded** → tool returns `Denied` after emitting `tengu_cost_budget_exceeded` (M3-05).
- **Sandbox not-available on Windows** → return `ValidationError` with byte-locked refusal string (M2-04).

### Panic policy

- Tools **never panic** in business logic.
- Tool registration at startup may panic on duplicate name (catches programmer error early).
- Programmer-error invariants use `debug_assert!`; release builds fall through silently.

---

## §6 Testing strategy

### Test pyramid

```
                  ╱╲
                 ╱  ╲    Parity fixtures (~9 new + cross-cutting error strings + smoke)
                ╱────╲
               ╱      ╲
              ╱  Integ ╲  Integration tests (~64 new — at least 1 per tool)
             ╱──────────╲
            ╱            ╲
           ╱   Contract   ╲  Tool contract suite (extends to 40 tools)
          ╱────────────────╲
         ╱                  ╲
        ╱     Unit tests     ╲ ~400 unit tests (avg 10/tool)
       ╱______________________╲ TDD red→green per task
```

### Per sub-plan test budgets

| Sub-plan | Unit | Integ | Parity fixture |
|----------|------|-------|----------------|
| M4-01 Foundation (6 tools) | ~60 | ~10 | `parity_file_tools.json` |
| M4-02 Shell (4) | ~40 | ~8 | `parity_shell_tools.json` |
| M4-03 Web (2) | ~25 | ~6 | `parity_web_tools.json` |
| M4-04 Workflow (5) | ~35 | ~8 | `parity_workflow_tools.json` |
| M4-05 Agent + Task (8) | ~80 | ~12 | `parity_agent_task_tools.json` |
| M4-06 Team (2) | ~15 | ~4 | `parity_team_tools.json` |
| M4-07 MCP + LSP (5) | ~50 | ~6 | `parity_mcp_lsp_tools.json` |
| M4-08 System (8) | ~80 | ~10 | `parity_system_tools.json` |
| **Subtotal new** | **~385** | **~64** | 8 group fixtures + cross-cutting |
| + cross-cutting | | | `parity_tool_error_strings.json` + `parity_tool_full_v0_5_0_smoke.json` |

### Tool contract suite (parameterized over impl)

```rust
// lingxi-test-harness/src/contracts/tool.rs (extends existing scaffolding)
pub async fn run_tool_contract<T: Tool>(
    tool: &T,
    happy_input: T::Input,
    invalid_input: T::Input,
    denied_gate: Box<dyn PermissionGate>,
) {
    // 1. happy path → Ok with non-empty output
    // 2. validate_input(invalid_input) → Err(ValidationError)
    // 3. execute with denied gate → Err(ToolError::Denied)
    // 4. cancellation: spawn execute(), drop future → no panic, no leak
    // 5. telemetry: tengu_tool_<name>_{started,succeeded|failed} all emitted
}
```

Every M4 tool driver test runs this contract against the real impl.

### Hermetic testing — dependency injection patterns

| Tool | Real dependency | Mock substitution |
|------|----------------|-------------------|
| File ops | `FileSystem` (M1) | `MockFileSystem` (in-memory tempdir) |
| Bash | `Process` (M1) | `MockProcess` (canned stdout/stderr/exit) |
| WebFetch | `Http` (M1) | axum mock server |
| AgentTool | `AnthropicProvider` (M3-03) | `MockApiProvider` (canned messages.create) |
| MCPTool | `McpClient` (M2-02b) | mock_mcp.rs harness |
| LSPTool | `LspClient` (M2-03) | duplex pipe + mock LSP server |
| TodoWriteTool | `SessionState` (M1) | in-memory session |
| ScheduleCronTool | `CronScheduler` (M1) | mock cron |

### Expected test counts at v0.5.0

- Existing M3 baseline: 753 (all stay green)
- New M4 unit tests: ~385
- New M4 contract drivers: 40 (one per tool)
- New M4 integration tests: ~64
- New M4 parity drivers: 9 (1 per sub-plan + 1 cross-cutting error strings + 1 smoke = 10 total)
- **Total at v0.5.0: ~1,250 tests**

### Cross-platform CI

Continues M3 matrix:
- `x86_64-apple-darwin`, `aarch64-apple-darwin`, `x86_64-unknown-linux-gnu`, `x86_64-unknown-linux-musl` — all build + test
- `x86_64-pc-windows-msvc` — Windows CI runner
- `x86_64-pc-windows-gnu` — `cargo check` only (local host limit)
- Tools that are Windows-only (PowerShellTool) gated by `cfg(target_os = "windows")`; tests skipped on other platforms.

### Test runtime targets

- Per-crate `cargo test -p lingxi-tools` < 15s (relaxed from 10s; contract suite spans 40 tools)
- Full workspace `cargo test --workspace` < 120s
- With `--ignored` (real network, real bash) < 300s

### Test discipline

- Each tool: **≥1 happy path, ≥1 permission-denied path, ≥1 validation-error path, byte-literal assertion on every error string from §5**.
- Wire identifier literals appear byte-for-byte in test assertions (parity fixtures enforce).
- No flaky tests — timing-sensitive tests use mocked `Clock` (M1 trait).

---

## §7 Wire identifiers locked (byte-alignment table)

These literals must appear byte-for-byte in code AND in at least one test assertion.

### Cross-cutting tool constants

| Item | Literal | Lock site |
|------|---------|-----------|
| Max tool output | `30_000` chars (`MAX_TOOL_OUTPUT_LENGTH`) | `shared/output_truncation.rs` |
| Tool output truncation suffix | `"\n\n[Output truncated due to length]"` | same |
| Tool full-name prefix (MCP) | `mcp__<server>__<tool>` | reuses M2-02b lock |
| Telemetry event prefix | `tengu_tool_<name>_{started,succeeded,failed}` | reuses M3-06 schema |

### File ops (M4-01)

| Item | Literal |
|------|---------|
| Max file read | `262_144` bytes (256 KB, `MAX_FILE_READ_SIZE`) |
| Binary detection | first `8 * 1024` bytes scanned for NUL byte |
| Default encoding | UTF-8 (BOM-aware); reject non-UTF-8 |
| Glob result cap | `100` matches (`MAX_GLOB_MATCHES`); excess → `truncated: true` flag |
| Grep result cap | `100` matches per file |
| Line indexing | **1-based** in tool input/output; LSP wire 0-based (M2-03 lock) |
| Trusted dir error | `"File path {path} is outside trusted directories"` |
| File too large error | `"File {path} ({size}B) exceeds 256KB read limit"` |
| Binary file error | `"File {path} appears to be binary (first 8KB contains NUL bytes)"` |
| Edit patch truncation suffix | `"\n\n... [{N} lines truncated] ..."` |

### Shell (M4-02)

| Item | Literal |
|------|---------|
| Bash default timeout | `120_000` ms (2 min); max `600_000` ms (10 min) |
| Bash shell | `/bin/bash` (Linux), `/bin/zsh` (macOS) |
| PowerShell shell | `powershell.exe` (Win), `pwsh` (other) |
| Bash timeout error | `"Bash command timed out after {N}ms"` |
| Stderr ring cap | 64 MB (M2-06 lock) |
| ANSI strip regex | `\x1b\[[0-9;]*[a-zA-Z]` |

### Web (M4-03)

| Item | Literal |
|------|---------|
| WebFetch max content | `5 * 1024 * 1024` bytes (5 MB) |
| WebFetch truncation suffix | `"\n\n[Content truncated due to length...]"` |
| User-Agent (tool side) | `claude-code-tool/<CARGO_PKG_VERSION>` (distinct from api-client UA) |
| Allowed schemes | `https`, `http` only |
| WebSearch wire name | `web_search_20250305` (matches anthropic-beta lock from M3-03) |

### Workflow (M4-04)

| Item | Literal |
|------|---------|
| TodoWrite states | `pending` / `in_progress` / `completed` (NOT `done`, NOT `todo`) |
| Plan mode markers | `[PLAN MODE]` (Enter), `[EXIT PLAN MODE]` (Exit) |
| Worktree branch prefix | `worktree-<flatten(slug)>` (M2-01 lock) |
| Worktree path | `<repo>/.claude/worktrees/<flatten(slug)>` (M2-01 lock) |

### Agent + Task (M4-05)

| Item | Literal |
|------|---------|
| AgentTool subagent types | from `agentTypes.json` (general-purpose, claude, plan, ...) |
| Task storage | `~/.claude/tasks/<task_id>/` (M2-06 task_output_path lock) |
| Task ID format | 9-char `[bartwmd][0-9a-z]{8}` (matches `lingxi_tasks::id::generate_task_id`) |
| SendMessage claim window | `Duration::from_secs(30)` |
| Subagent budget | inherits parent `BudgetEnforcer` (M3-05) — single shared instance |

### Team (M4-06)

| Item | Literal |
|------|---------|
| Team directory | `~/.claude/team-mem/<team_name>/` (M3-02 lock) |
| Team config | `~/.claude/team-mem/<team_name>/config.json` |
| Default team | `"default"` |

### MCP + LSP (M4-07)

| Item | Literal |
|------|---------|
| MCPTool dispatch | `mcp__<server>__<tool>` parsed → `McpClient::call_tool` (M2-02b) |
| McpAuthTool config | settings.json `mcpServers.<name>.transport.auth` |
| LSPTool operations | `hover` / `completion` / `definition` / `references` (M2-03 lock) |
| LSP position | 1-based on input; 0-based on wire (M2-03 conversion lock) |

### System (M4-08)

| Item | Literal |
|------|---------|
| AskUserQuestion max options | `4` |
| AskUserQuestion label cap | `60` chars |
| BriefTool upload path | `~/.claude/brief/<uuid>.txt` (LOCAL only in M4; cloud is M6) |
| ConfigTool fields | `model`, `outputStyle`, `theme`, `verbose` |
| SkillTool descriptor cap | `1024` chars per skill |
| ScheduleCronTool format | 5-field cron (`* * * * *`); via `lingxi-cron::parse` |
| ToolSearchTool top-k | `20` matches |
| RemoteTrigger credentials path | `~/.claude/.credentials.json` (legacy) |
| SyntheticOutputTool | local stub (echo input) in M4; cloud impl in M6+ |

### Telemetry events per tool (40 × 3 = 120 events)

Every tool emits via M3-06 schema:
- `tengu_tool_<name>_started` — `{ request_id: Verified, input_hash: Verified }`
- `tengu_tool_<name>_succeeded` — `{ request_id, duration_ms: u64, output_bytes: u64 }`
- `tengu_tool_<name>_failed` — `{ request_id, error_kind: Verified, duration_ms: u64 }`
- `tengu_tool_permission_requested` / `_granted` / `_denied` — `{ tool: Verified, decision_source: Verified }`

The 120 `tengu_tool_*` events map to M3-06's `tengu/tool.rs` 40-event registry (M3-06 schema is forward-compatible — events were declared with shape but emitter binding lands here in M4).

---

## §8 Sub-plan outline (M4-01 through M4-09)

Each sub-plan = separate document under `docs/superpowers/plans/2026-05-24-m4-XX-*.md`. Tasks expand per writing-plans skill (bite-sized TDD: write test → run-fail → impl → run-pass → commit).

### M4-01 — Foundation (6 file/search tools)

**Phases** (estimated ~22 bite-sized TDD tasks):
1. Shared `file_kit.rs` — binary detection, BOM, encoding (replaces ad-hoc in M1)
2. Shared `path_validation.rs` — trusted dir check, traversal guard
3. Shared `output_truncation.rs` — MAX_TOOL_OUTPUT_LENGTH applier
4. FileReadTool impl + contract test + parity fixture row
5. FileWriteTool impl + diff vs existing
6. FileEditTool impl + patch-format engine
7. NotebookEditTool impl + cell editing (Jupyter format)
8. GlobTool impl + 100-match cap
9. GrepTool impl + ripgrep integration
10. Cross-tool integration test (`read → edit → read` roundtrip)
11. Parity fixture `parity_file_tools.json` + driver
12. Verification gate + tag `m4.1`

**Tag**: `m4.1`

### M4-02 — Shell (4 tools)

**Phases** (estimated ~18 TDD tasks):
1. BashTool impl with sandbox wiring (M2-04 dispatch)
2. ANSI escape stripping
3. Background spawn (M2-06 spawn_background hook)
4. Timeout + tree-kill verification (M2-06 killpg)
5. PowerShellTool impl (Windows-gated)
6. REPLTool impl (interactive subprocess)
7. SleepTool impl (trivial — uses Clock trait)
8. Cost-of-tools telemetry hook (no-op in M4; reserved for future)
9. Parity fixture `parity_shell_tools.json` + driver
10. Verification gate + tag `m4.2`

**Tag**: `m4.2`

### M4-03 — Web (2 tools)

**Phases** (estimated ~10 TDD tasks):
1. WebFetchTool impl + 5 MB cap + scheme validation
2. WebSearchTool impl + routes via api-client (M3-03)
3. URL validation (no file://, no data:)
4. Retry policy (transient HTTP 5xx → return Err, agent decides)
5. Parity fixture `parity_web_tools.json` + driver
6. Verification gate + tag `m4.3`

**Tag**: `m4.3`

### M4-04 — Workflow (5 tools)

**Phases** (estimated ~16 TDD tasks):
1. TodoWriteTool — state machine (`pending`/`in_progress`/`completed`)
2. TodoWriteTool — persists via SessionState (M1)
3. EnterPlanModeTool + ExitPlanModeTool (in `plan_mode.rs`)
4. EnterWorktreeTool — uses M2-01 worktree manager
5. ExitWorktreeTool — removes via M2-01 worktree manager
6. Parity fixture `parity_workflow_tools.json` + driver
7. Verification gate + tag `m4.4`

**Tag**: `m4.4`

### M4-05 — Agent + Task (8 tools)

**Phases** (estimated ~32 TDD tasks):
1. AgentTool — coordinator.spawn_subagent integration (recursive registry dispatch)
2. AgentTool — subagent budget propagation (M3-05 inheritance)
3. AgentTool — telemetry events (subagent_started/completed/failed)
4. TaskCreateTool — TaskManager.create + persistence to `~/.claude/tasks/`
5. TaskGetTool + TaskListTool + TaskUpdateTool — read/list/mutate
6. TaskStopTool — graceful + force-kill (M2-06 tree-kill)
7. TaskOutputTool — reads task_output_path (M2-06 lock)
8. SendMessageTool — `lingxi-msgqueue` integration
9. Cross-tool integration test — agent spawns subagent that uses TodoWrite
10. Parity fixture `parity_agent_task_tools.json` + driver
11. Verification gate + tag `m4.5`

**Tag**: `m4.5`

### M4-06 — Team (2 tools)

**Phases** (estimated ~8 TDD tasks):
1. TeamCreateTool — writes `~/.claude/team-mem/<team>/config.json`
2. TeamDeleteTool — removes team dir (with safety check)
3. Parity fixture `parity_team_tools.json` + driver
4. Verification gate + tag `m4.6`

**Tag**: `m4.6`

### M4-07 — MCP + LSP (5 tools)

**Phases** (estimated ~14 TDD tasks):
1. MCPTool — `mcp__<server>__<tool>` parse + dispatch via M2-02b McpClient
2. McpAuthTool — reads settings auth config
3. ListMcpResourcesTool — wraps McpClient::list_resources
4. ReadMcpResourceTool — wraps McpClient::read_resource
5. LSPTool — operation dispatcher (hover/completion/definition/references)
6. LSPTool — 1-based ↔ 0-based position conversion (M2-03 lock)
7. Parity fixture `parity_mcp_lsp_tools.json` + driver
8. Verification gate + tag `m4.7`

**Tag**: `m4.7`

### M4-08 — System (8 tools)

**Phases** (estimated ~28 TDD tasks):
1. AskUserQuestionTool — 4-option cap + 60-char label cap
2. BriefTool — local stub (writes to `~/.claude/brief/<uuid>.txt`)
3. ConfigTool — reads/writes 4 fields via Settings (M3-01)
4. SkillTool — loads skill descriptors (M1 lingxi-skills)
5. ScheduleCronTool — uses M1 lingxi-cron parse + schedule
6. ToolSearchTool — searches registry by name/description; top-20
7. RemoteTriggerTool — local stub (no cloud)
8. SyntheticOutputTool — echoes input (claude.com internal — M4 stub)
9. Parity fixture `parity_system_tools.json` + driver
10. Verification gate + tag `m4.8`

**Tag**: `m4.8`

### M4-09 — Release v0.5.0

**Phases** (estimated ~12 TDD tasks):
1. Cross-cutting parity fixture `parity_tool_error_strings.json` (all locked strings from §5)
2. Cross-cutting parity fixture `parity_tool_full_v0_5_0_smoke.json` (40-tool e2e)
3. Drivers for both above
4. Workspace verification (cargo test/clippy/fmt/cross-target)
5. Update CHANGELOG.md `[0.5.0]` section
6. Update docs/ARCHITECTURE.md crate map (40 builtin tools)
7. Update docs/PLATFORMS.md (tool availability matrix)
8. Update README.md (v0.5.0 + 40-tool snapshot)
9. Tag `m4.9`
10. Tag `v0.5.0`

**Tag**: `m4.9` + `v0.5.0`

---

## §9 Release plan & timeline

Single-developer, **~12-16 weeks estimated** (recognizing M3 was rebaselined 5-7 → 8-10 weeks):

| Week | Sub-plan | Notes |
|------|---------|-------|
| 1-2 | M4-01 Foundation | Largest single sub-plan (6 tools + shared helpers) |
| 3 | M4-02 Shell | sandbox wiring is critical-path |
| 4 | M4-03 Web | smallest sub-plan |
| 5-6 | M4-04 Workflow | TodoWrite has state-machine complexity |
| 7-9 | M4-05 Agent + Task | 8 tools, recursive registry dispatch, biggest functional complexity |
| 10 | M4-06 Team | small |
| 11-12 | M4-07 MCP + LSP | 5 tools, mostly thin wrappers over M2 clients |
| 13-14 | M4-08 System | 8 tools, diverse |
| 15-16 | M4-09 Release | verification + docs + tag |

Buffer: ±2 weeks. If parallel via subagent-driven-development (as M2/M3 were), 8-10 weeks possible.

---

## §10 Open questions & deferred items

1. **Permission UI** — M4 ships `AllowAllGate` default + `PromptingGate` trait surface. The actual prompting UI (terminal-side input loop) lives in M5 (which depends on UI scope being decided). M4 contract tests use `AllowAllGate` for happy path + `DenyAllGate` for permission-denied path.

2. **`BriefTool` cloud upload** — Real claude-code BriefTool uploads to claude.com. M4 ships local-only stub (writes file to `~/.claude/brief/`). Cloud wire would land in M6 alongside other claude.ai integrations.

3. **`SyntheticOutputTool`** — Internal claude.com tool. M4 ships an echo-input stub. No cloud functionality.

4. **`RemoteTriggerTool`** — Reads legacy credentials.json but otherwise stubbed. Cloud delivery deferred to M6.

5. **Tool-level cost telemetry** — `tengu_tool_<name>_cost_recorded` is reserved in M3-06 schema but not emitted in M4. Will be emitted in M4-02 Shell's BashTool (long-running shell commands consume resources worth tracking). Actual binding is a no-op hook in M4; M5 wires real cost attribution.

6. **Agent prompt templates** — AgentTool spawns subagents that use M3-03 api-client. The actual system prompt / persona / `agentTypes.json` byte-aligned wiring is M5. M4 ships the machinery; M5 supplies the prompt text.

These do NOT block v0.5.0 release.

---

## §11 References

- **claude-code source (read-only):** `/Users/luolingfeng/Projects/LingXi-Next/claude-code/src/tools/`
  - 40 tool subdirectories — each contains tool implementation + utils + prompt + (often) tests
  - Key shared files: `claude-code/src/Tool.ts`, `claude-code/src/tools/utils.ts`, `claude-code/src/tools/shared/`
- **M3 spec:** `docs/superpowers/specs/2026-05-23-m3-engine-completion-design.md`
- **M2 spec:** `docs/superpowers/specs/2026-05-23-m2-claude-code-parity-design.md`
- **M3 sub-plans:** `docs/superpowers/plans/2026-05-23-m3-*.md` (7 files)
- **Existing LingXi crates:**
  - `lingxi-code/crates/tools/` — framework (Tool trait, registry, dispatcher, permissions, streaming, results)
  - `lingxi-code/crates/permission/` — M1 PermissionGate trait
  - `lingxi-code/crates/agent/`, `coordinator/`, `tasks/`, `session/`, `memory/` — M1/M3 supporting crates
  - `lingxi-code/crates/mcp/`, `lsp/` — M2 protocol clients
  - `lingxi-code/crates/api-client/`, `anthropic-oauth/`, `cost/`, `telemetry/`, `settings/` — M3 engine
- **Existing parity protocol:** v3 §32.6 — every fixture has `_source` + `_note` claude-code citations

---

## §12 Out of scope (explicit)

To prevent scope creep:

- **UI / Terminal rendering** — React Ink hooks (`useToolUseConfirm`, progress UI, animated spinners) NOT ported. Rust-side progress callback trait is provided (M1 `progress.rs`); M5 wires actual UI.
- **Mobile real-device binding** — UniFFI compile-only gates stay.
- **Slash commands** — M5.
- **Hooks lifecycle execution** — M5 (M4 only ships the trigger emission points).
- **Agent prompt templates byte-aligned** — M5.
- **Plugin marketplace** — M6.
- **MCP server side** — M6.
- **`/doctor` command** — M6.
- **Cron user-level workflow** — M6 (M4 only ships ScheduleCronTool surface).
- **UniFFI API stabilization** — M6.
- **claude.com cloud features** (BriefTool real upload, SyntheticOutputTool cloud, RemoteTrigger cloud) — M6+ or never (depends on claude.com cooperation).

---

**End of M4 design.**
