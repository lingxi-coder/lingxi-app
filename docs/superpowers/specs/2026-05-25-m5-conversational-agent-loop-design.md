# M5 Conversational Agent Loop — Design

**Status:** Draft, awaiting user review (2026-05-25)
**Target release:** v0.6.0
**Predecessor:** v0.5.0 (M4 — 40 tools 全集)
**Successor (planned):** v0.7.0 (M6 — TUI 基础)

---

## §1 Goal & Non-Goals

### Goal

**v0.6.0 是 LingXi Core 第一个真正可日常使用的版本。** 用户安装 `lingxi-cli`,就能像用 `claude` 一样开始 AI 编码协作。

具体可见目标:

- `lingxi-cli "fix the bug in foo.rs"` — 一行命令,完整 turn loop 跑到底,该改的文件改了,该装的依赖装了
- `lingxi-cli` 不带参数 → 进入 line-based REPL,可以多轮对话
- `lingxi-cli --resume <id>` — 恢复之前 `claude` 或 `lingxi-cli` 留下的 session
- 102 个 slash commands 全部可识别,18 个核心可用
- Hooks 在 PreToolUse/PostToolUse 真正触发
- Cost 实时显示,可调 `/cost` 查看
- 流式 token 输出(token 一进 SSE 就 print 到 stdout)

### Non-Goals (M5 明确不做)

- **TUI 渲染** — 留给 M6。M5 是 plain stdio,不做 ratatui shell、不做 tool use 折叠、不做 diff preview UI、不做 spinner/进度条。
- **102 命令全部实现** — M5 只实现 18 核心,其余 84 注册名字 + stub 返回 `<name>: not implemented in v0.6.0 (M5)`。
- **IDE plugins (VS Code / JetBrains)** — TS/Kotlin 代码,不属于 Rust port 范围。
- **Mobile runtime** — 继续 compile-only。
- **新的工具** — 40 tools 在 v0.5.0 已锁,M5 不增不减。
- **多人协作 / 团队功能** — TeamCreate/TeamDelete 在 v0.5.0 已锁 surface,wiring 在 M6+。
- **Subagent UI** — `/agents` 命令只列表 + 描述,不开 agent 编辑界面(那是 TUI 工作)。

### Success Criteria

`v0.6.0` 发布等价于全部成立:

1. `lingxi-cli "<prompt>"` 能完整跑通一次 multi-turn 对话(包含工具调用),exit code = 0
2. `lingxi-cli` REPL 模式可以连续聊天,`/clear /exit` 可用
3. Session 文件用 `claude` 创建,`lingxi-cli --resume` 能继续(JSONL 字节等价)
4. 18 个核心 slash commands 全部 wire 完整(测试覆盖)
5. 其余 84 个 slash commands surface 注册(`/<name>` 不返回 "unknown",而是返回 stub literal)
6. Hooks 四 arm (Builtin/Http/Command/Agent) 都可触发,PreToolUse/PostToolUse 通
7. `cargo test --workspace` 通过,2 个已知 fs-watch flake 接受
8. 工作区版本 0.5.0 → 0.6.0,annotated 标签 `v0.6.0` + `m5.14`
9. Telemetry +约 77 个事件(orchestrator/REPL/session/resume/hooks/commands turn 事件)
10. 跨平台 compile check:macOS (主) + Linux + Windows + 2 个 mobile target

---

## §2 Architecture

### 2.1 新增 / 修改的 crate

```
新建 (2):
  lingxi-orchestrator       — 顶层 ConversationOrchestrator + turn loop + session driver
  lingxi-cli                — 可执行 binary,包含 argv 解析 + REPL 读循环 + 流式 stdout

修改 (扩展现有,7):
  lingxi-agent              — runner.rs stub → 真正的 reduce+pump
  lingxi-commands           — surface 加 102 命令注册 + 18 实现
  lingxi-hooks              — executor.rs Http/Command/Agent 三 arm 接通
  lingxi-session            — JSONL writer/reader (与 claude-code 字节等价)
  lingxi-telemetry          — orchestrator/REPL/session/resume/hooks/commands 事件
  lingxi-tools              — ToolUseContext 加 OrchestratorHandle 字段
  lingxi-traits             — 加 OrchestratorHandle / SlashCommandDispatcher / PromptingGate 等 trait

不动:
  protocol, core, traits 大部分, api-client (M3 已就绪), mcp, lsp, sandbox, cost, permission,
  tools (40 个工具已锁), telemetry tools schema, plugin, sidequery, skills, cron, memory,
  filestate, msgqueue, outputstyles, secret, anthropic-oauth, bridge, compaction
```

### 2.2 数据流(一次完整 turn loop)

```
┌─────────────────────────────────────────────────────────────────────────┐
│ user 输入 "fix bug X"   或   `lingxi-cli` argv                           │
└─────────────────────────────────────────────────────────────────────────┘
                │
                ▼
┌─────────────────────────────────────────────────────────────────────────┐
│ lingxi-cli::main                                                         │
│   1. argv 解析 (one-shot / -p / --resume / --model / ...)                │
│   2. 初始化 ConfigStore + Telemetry + ApiClient + ToolRegistry           │
│   3. 构造 Orchestrator                                                   │
└─────────────────────────────────────────────────────────────────────────┘
                │
                ▼
┌─────────────────────────────────────────────────────────────────────────┐
│ lingxi-orchestrator::ConversationOrchestrator::run(prompt)              │
│                                                                          │
│   ┌─ 是否 /slash 命令? ─→ SlashDispatcher::dispatch ─→ ToolCallResult   │
│   │                                                                      │
│   └─→ assemble_system_prompt() (cwd + git + files + CLAUDE.md + tools) │
│       │                                                                  │
│       │  ┌──────── 内部 turn loop ────────────────────────┐             │
│       │  │ 1. session.append(UserMessage)                  │             │
│       │  │ 2. api_client.stream(messages, system, tools) ──┼──→ SSE    │
│       │  │ 3. parse SSE → emit content_block_delta → stdout│   stream  │
│       │  │ 4. parse tool_use blocks                        │             │
│       │  │ 5. for each tool_use:                           │             │
│       │  │      a. hooks::PreToolUse fire                  │             │
│       │  │      b. permission_gate::check (interactive y/N)│             │
│       │  │      c. tool_registry.call(name, input, ctx)    │             │
│       │  │      d. hooks::PostToolUse fire                 │             │
│       │  │      e. session.append(ToolResultMessage)       │             │
│       │  │ 6. session.append(AssistantMessage)             │             │
│       │  │ 7. stop_reason == "end_turn"? yes → exit loop   │             │
│       │  │                                no  → goto 2     │             │
│       │  └─────────────────────────────────────────────────┘             │
│       │                                                                  │
│       └─→ compaction trigger if context > 95% threshold                 │
│                                                                          │
│   返回最终 conversation_result                                           │
└─────────────────────────────────────────────────────────────────────────┘
                │
                ▼
┌─────────────────────────────────────────────────────────────────────────┐
│ lingxi-cli::main (post-run)                                              │
│   - print final cost summary                                             │
│   - REPL? → 回到 prompt input loop                                       │
│   - one-shot? → exit code 0                                              │
└─────────────────────────────────────────────────────────────────────────┘
```

### 2.3 关键 trait / 类型(新增到 `lingxi-traits`)

```rust
// 顶层会话 orchestrator handle —— slash commands 调用它来 trigger /clear /compact
pub trait OrchestratorHandle: Send + Sync {
    async fn current_session_id(&self) -> SessionId;
    async fn clear_session(&self) -> Result<(), OrchestratorError>;
    async fn force_compact(&self) -> Result<CompactionSummary, OrchestratorError>;
    async fn snapshot_cost(&self) -> CostSnapshot;
    async fn switch_model(&self, model: &str) -> Result<(), OrchestratorError>;
}

// slash command dispatcher —— /<name> → command 执行
pub trait SlashCommandDispatcher: Send + Sync {
    fn known(&self, name: &str) -> bool;          // 全 102 都 known
    fn implemented(&self, name: &str) -> bool;     // 仅 18 实现
    async fn dispatch(&self, name: &str, args: &str, ctx: SlashContext)
        -> Result<SlashOutcome, SlashError>;
}

// 用户交互的 permission gate —— stderr 提示 y/N
pub trait PromptingGate: PermissionGate {
    async fn prompt_user(&self, request: &PermissionRequest) -> PromptDecision;
}

// 流式 token 输出 sink —— orchestrator 把 SSE delta 喂给它
pub trait OutputStream: Send + Sync {
    async fn emit_text(&self, text: &str);                  // streaming token
    async fn emit_tool_call(&self, tool: &str, input: &Value);
    async fn emit_tool_result(&self, tool: &str, result: &Value);
    async fn emit_end_turn(&self, stop_reason: &str, cost: &CostSnapshot);
}
```

### 2.4 关键设计选择

1. **`lingxi-orchestrator` 是新 crate**(不放进 `lingxi-agent`),因为它的责任是"管理对话",而 agent 责任是"运行单个 agent state machine"。Orchestrator 用 agent 作为底层,但不归 agent 管。
2. **`OutputStream` trait 抽象**让 stdio 和(未来 M6 的)TUI 都可以作为 sink 接进去 —— 同一个 orchestrator 不用修改。
3. **`SlashContext`** 持有 `Arc<dyn OrchestratorHandle>`,所以 `/clear /compact /cost /model` 等命令可以直接调到 orchestrator。`/help /version /status` 等不需要 orchestrator 的命令拿默认实现。
4. **Session JSONL byte-equiv** —— `lingxi-session` 加 `JsonlWriter`/`JsonlReader`,严格按 claude-code 的 `~/.claude/projects/<project-hash>/<id>.jsonl` 格式(每行一个 JSON message,parent_uuid 链)。

---

## §3 14 个 Sub-plans 概览

| # | 名称 | 范围 | 触碰 | 关键字节锁 | 新事件 | 预估 tasks |
|---|---|---|---|---|---|---|
| **M5-01** | Engine wiring close-out | v0.5.0 三个 follow-up:(a) `agent::runner::run_subagent` stub → 真 reduce 循环;(b) `TaskRegistryHandle::output` 接 `TaskOutputManager::read` 真读 spool;(c) `RegistryToolInvoker::invoke` 接 `ToolRegistry::call` 真分派。 | `lingxi-agent`, `lingxi-tasks`, `lingxi-tools` | 无新锁;补全 m4.5-wiring 的两个 Arc::ptr_eq 测试链路。 | 0 | ~12 |
| **M5-02** | ConversationOrchestrator core (batched) | 新 crate `lingxi-orchestrator`,先用 non-streaming `messages_create`,完成 `run(prompt) -> ConversationResult` 的 outer turn loop。Mock model 测试。 | new `lingxi-orchestrator`, `lingxi-traits` (新 traits) | `MAX_TURNS_PER_CONVERSATION` 锁(看 claude-code:`maxTurns` 通常 30+) | 3 (`conversation_started/completed/failed`) | ~16 |
| **M5-03** | System prompt 动态组装 | `assemble_system_prompt(ctx) -> String` —— 拼接 cwd / git status / `git diff --stat` / file tree (depth=2) / 全部 CLAUDE.md 内容 / tools schema。1:1 with claude-code's `getSystemPrompt`. | `lingxi-orchestrator::prompt`, `lingxi-memory` (集成读), `lingxi-core` | claude-code 的 system prompt header/footer 模板锁 | 0 | ~14 |
| **M5-04** | Streaming SSE + mid-stream tool dispatch | orchestrator 从 batched 升级到 streaming:接 `api_client::stream` SSE,逐 `content_block_delta` 喂 `OutputStream::emit_text`,tool_use block 完整后立即 dispatch(不等整个 response 结束)。 | `lingxi-orchestrator`, `lingxi-api-client` | claude-code 的 SSE event 类型枚举 + `content_block_start/_delta/_stop` 序列锁 | 2 (`turn_streaming_started/completed`) | ~18 |
| **M5-05** | Permission gate UX | 新 `PromptingGate` impl(in `lingxi-cli` 或 `lingxi-permission`):interactive y/N via stderr,接到 stdin。集成进 orchestrator turn loop:tool dispatch 前调 `permission_gate.check_with_user_prompt(req)`. | `lingxi-permission`, `lingxi-orchestrator`, `lingxi-cli` (stdin handler) | claude-code 的 permission prompt 文本模板锁:`Claude needs your permission to use ${toolName}` | 2 (`permission_prompted/answered`) | ~13 |
| **M5-06** | Hooks runtime 完整 4-arm | `lingxi-hooks::executor` 的 `Http`/`Command`/`Agent` arms 从 stub → 真实现:HttpExecutor 走 SsrfGuard + HttpClient;CommandExecutor 走 ProcessRunner + sandbox;AgentExecutor 走 SubagentSpawner(M4-05 wiring 已就绪)。orchestrator turn loop 在 tool dispatch 前后调 PreToolUse/PostToolUse. | `lingxi-hooks`, `lingxi-orchestrator` | claude-code 的 hook event JSON schema + hook timeout (60s default?) 锁 | 8 (`hook_pre_started/completed/failed`, `hook_post_started/completed/failed`, `hook_http_skipped_ssrf`, `hook_timeout`) | ~17 |
| **M5-07** | Session JSONL byte-equivalent | `lingxi-session::jsonl` writer/reader。路径 `~/.claude/projects/<project-hash>/<session-uuid>.jsonl`(djb2 hash + UUID v4 session id)。每行 = 一个 message,字段 `{type, uuid, parentUuid, sessionId, timestamp, cwd, version, message, ...}`. orchestrator turn loop 每次 user/assistant/tool_result 都 append. | `lingxi-session`, `lingxi-orchestrator` | 文件路径锁、JSONL message schema 锁、djb2 hash 算法锁 | 3 (`session_appended/rotated/corrupted`) | ~15 |
| **M5-08** | Resume command + load logic | `lingxi-cli --resume <session-id>` 或 `--resume` (交互选最近):读 JSONL → 重建 message 历史 → 用旧的 system prompt seed → 进入 turn loop。`/resume` slash command 同样路径。 | `lingxi-orchestrator`, `lingxi-cli`, `lingxi-session` (`load_session`) | resume 完整性检查:`parentUuid` 链 + `sessionId` 一致 | 2 (`resume_started/completed`) | ~12 |
| **M5-09** | Slash commands surface (全 102) | `lingxi-commands::registry` 注册全 102 个 commands,84 个 stub 返回锁定 literal `"<name>: not implemented in v0.6.0 (M5)"`. 18 个 fill stub `Box<dyn BuiltinCommandHandler>` 占位(下一 plan 实现)。 | `lingxi-commands`, `lingxi-orchestrator` | 102 个命令名全锁;stub literal 锁 | 0 | ~10 |
| **M5-10** | Slash commands core batch 1 (基础 6 个) | 实现 `/clear /compact /help /exit /memory /init`. `/clear` 调 `OrchestratorHandle::clear_session`;`/compact` 调 `force_compact`;`/help` 列 102 名 + 标注实现状态;`/exit` 设 `should_exit=true`;`/memory` 打开 `$EDITOR` 编辑 CLAUDE.md;`/init` 生成 CLAUDE.md skeleton. | `lingxi-commands::builtin`, `lingxi-orchestrator` | `/init` 模板字符串字节锁 | 6×3=18 (`command_<name>_started/completed/failed`) | ~16 |
| **M5-11** | Slash commands core batch 2 (12 个) | `/cost /config /model /permissions /mcp /hooks /agents /login /logout /version /status /doctor /resume`. 大多走 read-only(列表/打印),少数(`/config /permissions`)需修改 settings.json,`/login /logout` 走 anthropic-oauth. | `lingxi-commands::builtin`, `lingxi-anthropic-oauth`, `lingxi-mcp` (列表), `lingxi-hooks` (列表) | 各命令输出格式锁 | 12×3=36 | ~22 |
| **M5-12** | CLI binary `lingxi-cli` | 新 binary crate `lingxi-cli`(`lingxi-code/crates/cli/`)。`clap`-based argv:`lingxi-cli [PROMPT]`, `-p/--print`, `--resume [ID]`, `--model <name>`, `--cwd <dir>`, `--no-stream`, `--json`, `--debug`. main 函数:init telemetry + orchestrator + 调 `orchestrator.run()` → exit code. | new `lingxi-cli` crate | argv flag 名全锁 | 0 (CLI 自己不发事件,委托) | ~14 |
| **M5-13** | Stdio REPL mode | `lingxi-cli` 不带 prompt 参数时进入 REPL:`> ` 提示符 → `tokio::stdin` read line → 如果 `/<cmd>` → dispatch to SlashDispatcher; 否则 → `orchestrator.run_turn(prompt)`. `Ctrl+D` (EOF) → 优雅退出。流式 token 写 stdout,SIGINT 取消当前 turn 不退 REPL. | `lingxi-cli`, `lingxi-orchestrator` (run_turn vs run) | REPL 提示符 `"> "` 字节锁;EOF/SIGINT 行为锁 | 2 (`repl_session_started/ended`) | ~12 |
| **M5-14** | Release v0.6.0 — 跨切 parity + tag | Cross-cutting parity fixtures:(a) `parity_orchestrator.rs` — turn loop 完整流程;(b) `parity_slash_commands.rs` — 102 name 全注册 + 18 实现;(c) `parity_session_jsonl.rs` — 字节等价输入输出;(d) `parity_hooks_runtime.rs` — 四 arm fired. 工作区版本 0.5.0→0.6.0(全 39 个 Cargo.toml). CHANGELOG + README + release doc. Tag `m5.14` + `v0.6.0`. | `lingxi-test-harness`, 全部 `Cargo.toml`, `CHANGELOG.md`, `README.md` | release event `lingxi_core_v0_6_0_released` | 1 | ~14 |

### 3.1 总计

- **14 sub-plans**,~213 tasks
- **新事件:** 约 77 个(orchestrator+streaming+permission+hooks+session+resume+commands+repl+release)
- **依赖顺序:** 严格线性 M5-01 → M5-14
- **估时:** 单 claw 8-10 周

### 3.2 依赖图(简化)

```
M5-01 (engine wiring)
   ↓
M5-02 (orchestrator core) ── 依赖 M5-01 (subagent pump)
   ↓
M5-03 (system prompt) ── 依赖 M5-02
   ↓
M5-04 (streaming) ── 依赖 M5-02 (turn loop)
   ↓
M5-05 (permission UX) ── 依赖 M5-02
   ↓
M5-06 (hooks runtime) ── 依赖 M5-05 (permission flows before hooks)
   ↓
M5-07 (session JSONL) ── 依赖 M5-02
   ↓
M5-08 (resume) ── 依赖 M5-07
   ↓
M5-09 (commands surface) ── 依赖 M5-02 (dispatcher needs orchestrator)
   ↓
M5-10 (commands batch 1) ── 依赖 M5-09
   ↓
M5-11 (commands batch 2) ── 依赖 M5-10, M5-07 (resume cmd), M5-08
   ↓
M5-12 (CLI binary) ── 依赖 M5-02..11
   ↓
M5-13 (REPL mode) ── 依赖 M5-12
   ↓
M5-14 (release) ── 依赖全部
```

---

## §4 关键 Wire Identifiers / 字节锁清单

按 sub-plan 分组,每项标注:**已确认**(从 claude-code 源码 grep 出)/ **待 reverse-engineer**(M5-XX task 0 第一步确定)。

### 4.1 Session 存储 (M5-07)

| 锁 | 值 | 状态 |
|---|---|---|
| Session 文件根路径 | `getClaudeConfigHomeDir()/projects/<project-hash>/<session-uuid>.jsonl` | ✅ 已确认 `src/utils/sessionStoragePortable.ts:7-13` |
| `project-hash` 算法 | **djb2 hash** of canonical cwd path | ✅ 已确认 `import { djb2Hash } from './hash.js'` |
| Session UUID 格式 | `^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$` (UUID v4, lowercase) | ✅ 已确认 line 23 |
| JSONL message schema | `{type, uuid, parentUuid, sessionId, timestamp, cwd, version, message: {...}}` 各字段名锁 | ⚠️ 待 reverse-engineer:`type` 枚举值、`message` 内嵌 schema |
| `LITE_READ_BUF_SIZE` | `65_536` (64 KB) — 用于 metadata 快速读取 | ✅ 已确认 line 17 |
| `unescapeJsonString` / `extractJsonStringField` 算法 | 不完全 JSON parse,逐字符扫描 — 需要 1:1 port | ✅ 算法已找到 |
| `validateUuid` regex | 与上面 UUID 格式一致 | ✅ 已确认 |

### 4.2 Turn loop 限制 (M5-02)

| 锁 | 值 | 状态 |
|---|---|---|
| `maxTurns` 字段名 (per-agent) | camelCase `maxTurns` | ✅ 已确认 `QueryEngine.ts:146` |
| Default `maxTurns` (主 agent) | **无全局 default** — pulled from agent frontmatter 或调用者传入 | ⚠️ 待:主 conversation 的默认值需 reverse-engineer(可能是 `Infinity` 或 30) |
| "Reached maximum turns" 错误模板 | `"Reached maximum number of turns (${maxTurns})"` | ✅ 已确认 `QueryEngine.ts:870` |
| Streaming SSE event 名 | `message_start`, `content_block_start`, `content_block_delta`, `content_block_stop`, `message_delta`, `message_stop`, `ping` | ⚠️ 待:从 `src/services/api/` reverse-engineer 实际订阅列表 |

### 4.3 System prompt 模板 (M5-03)

| 锁 | 值 | 状态 |
|---|---|---|
| System prompt section 顺序 | 1. header (`You are Claude Code, Anthropic's...`) 2. environment (`<env>...</env>`) 3. tools schema 4. memory (CLAUDE.md) 5. footer | ⚠️ 全部待 reverse-engineer from `src/services/prompts.ts` |
| `<env>` 内容 | cwd / git status / `git diff --stat` / file tree depth=2 / platform | ⚠️ 待 reverse-engineer |
| Memory hierarchy 拼接 | `~/.claude/CLAUDE.md` → `<repo>/CLAUDE.md` → `<repo>/CLAUDE.local.md` (与 M3-02 锁定的 hierarchy 一致) | ✅ 已与 M3-02 对齐 |

### 4.4 Permission gate UX (M5-05)

| 锁 | 值 | 状态 |
|---|---|---|
| 通用 prompt | `"Claude needs your permission to use ${toolName}"` | ✅ 已确认 `src/components/permissions/PermissionRequest.tsx:142` |
| AgentTool 特殊 prompt | `"Agent tool requires permission to spawn sub-agents."` | ✅ 已确认 `src/tools/AgentTool/AgentTool.tsx:1290` |
| `[Y/n]` vs `[y/N]` | claude-code 用 `[Y/n]`(default yes) 或 `[y/N]`(default no)按 tool 区分 | ⚠️ 待 reverse-engineer:各 tool 的 default Y/N |
| stdin 读取 → 接受 `y/Y/yes` / `n/N/no` / 空回车走 default | 标准 UX | 实现细节 |

### 4.5 Hooks runtime (M5-06)

| 锁 | 值 | 状态 |
|---|---|---|
| Hook event JSON schema | `{event_type, tool_name, tool_input, tool_response, ...}` | ⚠️ 待 reverse-engineer from `src/hooks/` |
| Hook timeout (default) | 通常 60s,SSRF 5s | ⚠️ 待确认 |
| HTTP hook SSRF block list | localhost / 127.x / 169.254.x / private ranges | ✅ 已在 `lingxi-hooks::ssrf_guard.rs`(M1.7) |
| Hook executor 错误格式 | `"Hook ${id} failed: ${reason}"` | ⚠️ 待 reverse-engineer |

### 4.6 Slash commands (M5-09, M5-10, M5-11)

| 锁 | 值 | 状态 |
|---|---|---|
| 102 命令名 | 见 `src/commands/` 目录(已枚举出全部) | ✅ 全部已枚举 |
| Stub literal (84 未实现) | `"${name}: not implemented in v0.6.0 (M5)"` | 🆕 我们定义(我们的锁) |
| `/help` 输出格式 | `Commands:\n  /<name>  <description>\n  ...` | ⚠️ 待 reverse-engineer from `src/commands/help/` |
| `/init` 生成的 CLAUDE.md 模板 | 几百行 markdown | ⚠️ 待 reverse-engineer from `src/commands/init.ts` |
| 18 核心命令各自 dispatch/output 格式 | 每个 1-3 行字节锁 | ⚠️ 待 reverse-engineer per command |

### 4.7 CLI flags (M5-12)

| 锁 | 值 | 状态 |
|---|---|---|
| `-p / --print` (one-shot 模式) | exit 0 after first end_turn | ✅ 已确认 `src/cli/print.ts` |
| `--resume [<id>]` | 不带 id → 交互选最近 5 | ⚠️ 待 reverse-engineer 行为 |
| `--model <name>` | 与 `/model` 同 | ⚠️ 待确认接受的 model 字符串 |
| `--cwd <dir>` | 修改 cwd before init | 通用 |
| `--no-stream` | batched 输出 | ⚠️ claude-code 是否有此 flag 待确认 |
| `--json` | 输出结构化 JSON(stdout 替代人类可读) | ⚠️ 待 reverse-engineer |
| `--debug` | 打开 verbose 日志到 stderr | 通用 |

### 4.8 REPL (M5-13)

| 锁 | 值 | 状态 |
|---|---|---|
| REPL prompt 字符串 | `"> "` 或 claude-code 用的具体串 | ⚠️ 待 reverse-engineer(`src/screens/REPL.tsx`) |
| EOF 行为 | `Ctrl+D` 优雅退出 + 持久化 session | 通用 |
| SIGINT 行为 | `Ctrl+C` 取消当前 turn,不退 REPL | 通用 |
| Multi-line input | TBD — claude-code 用 backslash continuation? | ⚠️ 待确认 |

### 4.9 Telemetry 新事件(累计 ~77 个)

| Plan | 新增 |
|---|---|
| M5-02 | 3 — `conversation_started/completed/failed` |
| M5-04 | 2 — `turn_streaming_started/completed` |
| M5-05 | 2 — `permission_prompted/answered` |
| M5-06 | 8 — `hook_pre_started/completed/failed`, `hook_post_started/completed/failed`, `hook_http_skipped_ssrf`, `hook_timeout` |
| M5-07 | 3 — `session_appended/rotated/corrupted` |
| M5-08 | 2 — `resume_started/completed` |
| M5-10 | 18 — 6 命令 × `started/completed/failed` |
| M5-11 | 36 — 12 命令 × 3 |
| M5-13 | 2 — `repl_session_started/ended` |
| M5-14 | 1 — `lingxi_core_v0_6_0_released` |
| **总计** | **77** 新事件 |

预计 `ALL_EVENT_NAMES` 从 v0.5.0 的 238 → v0.6.0 的 ~315(实际数字 reconcile 时确定)。

### 4.10 工作策略(关于 ⚠️ 待 reverse-engineer 项)

每个 sub-plan 的 Task 0(scaffold)第一步 = **grep + read claude-code source for the byte-locks listed here**,把待办项的实际值锁进 plan 的"Critical 1:1 fidelity items"段。这样 plan 写完每个具体串都已经验证过来源。

---

## §5 Testing 策略

### 5.1 测试层次(沿用 M4 模式)

每个 sub-plan 至少产出 4 种测试:

| 层次 | 位置 | 目的 |
|---|---|---|
| **Unit** | `src/<file>.rs::tests` | TDD per step,组件内部正确性 |
| **Integration** | `crates/<crate>/tests/*.rs` | 跨 module / 跨 trait 流程 |
| **Parity** | `crates/test-harness/tests/parity_*.rs` + `fixtures/*.json` | 字节锁验证,1:1 with claude-code |
| **End-to-end** | `crates/cli/tests/cli_e2e_*.rs` via `assert_cmd` | 真二进制 + stdin/stdout 全链路(M5-12, M5-13) |

### 5.2 关键测试基础设施

| 名称 | 用于 plans | 来源 |
|---|---|---|
| `MockHttpTransport` + `ScriptedResponse` | M5-02, M5-04 | 已有 from M3-03 / M4-03 |
| `MockBudgetEnforcerHandle` + `MockSubagentSpawner` | M5-01, M5-06 | 已有 from M4-05 wiring |
| **NEW: `MockOutputStream`** | M5-04 流式输出测试 | 新增 `lingxi-test-harness` |
| **NEW: `ScriptedSseStream`** | M5-04 流式响应模拟 | 新增,产 `message_start → content_block_delta × N → message_stop` |
| **NEW: `MockStdin` + `CaptureStdout`** | M5-13 REPL | 新增 to `lingxi-cli` test_support |
| `tempfile::TempDir` | M5-07, M5-08 session JSONL | 已有 (stdlib) |

### 5.3 Golden file fixtures(M5-07 / M5-08 关键)

Session JSONL 是 v0.6.0 最 byte-critical 的输出。测试策略:

1. **Snapshot 法:** 用真 `claude` CLI 跑一个简单 turn(`claude -p "say hi"`),抓 `~/.claude/projects/<hash>/<uuid>.jsonl`,sanitize 时间戳/UUID(替换为占位符 `<TS>` / `<UUID>`),存到 `crates/test-harness/src/parity/fixtures/golden_sessions/`(新目录)。
2. **写入测试:** 给 lingxi-session::JsonlWriter 喂相同 message 序列,生成的 file 跟 golden(replace 占位符回真值)字节等价。
3. **读取测试:** 让 lingxi-session::JsonlReader 读 golden,reconstruct 出的 message 序列与 ground truth 一致。
4. **互通测试:** lingxi-cli 写一段 → claude resume 读它 ✅;claude 写一段 → lingxi-cli resume 读它 ✅。(M5-08 acceptance)

需要 3-5 个 golden sessions 覆盖:

- 单 turn 简单对话
- Multi-turn + tool calls
- Resumed session(parent_uuid 链跨进程)
- Compacted session(包含 SDKCompactBoundaryMessage)

### 5.4 CLI end-to-end (M5-12, M5-13)

新增 `crates/cli/tests/`:

- `cli_help_test.rs` — `lingxi-cli --help` 与 `claude --help` 输出对照(byte-diff 允许 prefix/version 差异)
- `cli_print_mode_test.rs` — `lingxi-cli -p "hello"` 完整 turn(mock API 后端)
- `cli_resume_test.rs` — `--resume <id>` 加载 golden + 接续 turn
- `cli_repl_test.rs` — `assert_cmd` driver,scripted stdin → assert stdout segments
- `cli_signals_test.rs` — Ctrl+C / EOF 行为

E2E 用 `MockHttpTransport` 通过环境变量 `ANTHROPIC_API_BASE=<mock-url>` 注入(api-client 已支持)。

### 5.5 跨平台

- 主要平台 macOS。
- Linux:CI 完整测试 + golden session 文件(确认 djb2 hash + path separator 一致)。
- Windows:stdin/stdout 换行符差异(`\r\n` vs `\n`)— 在 `lingxi-cli` 的 stdio adapter 中归一化。Windows golden session 单独存(djb2 hash 同 input 同 output,但 cwd 路径含 `\` 影响)。

---

## §6 跨切关注

### 6.1 Cargo 依赖图

新增依赖关系(箭头方向 = 依赖):

```
lingxi-cli ──→ lingxi-orchestrator ──→ lingxi-agent
                                  ├──→ lingxi-tools
                                  ├──→ lingxi-session
                                  ├──→ lingxi-commands
                                  ├──→ lingxi-hooks
                                  ├──→ lingxi-api-client
                                  ├──→ lingxi-permission
                                  ├──→ lingxi-cost
                                  ├──→ lingxi-memory
                                  └──→ lingxi-telemetry
```

**潜在 cycle risks:**

- ✅ `lingxi-orchestrator` 是新叶节点,只入不出。
- ⚠️ `lingxi-commands` 之前不依赖 `lingxi-orchestrator`,M5-09 加入 `SlashCommandDispatcher` 时:dispatcher 可能需要回调 orchestrator。**解法:** dispatcher trait 在 `lingxi-traits`,orchestrator 实现该 trait;`lingxi-commands` 拿 `Arc<dyn OrchestratorHandle>` 不直接 deps `lingxi-orchestrator`。这就是 §2 architecture 里 `OrchestratorHandle trait` 的设计动机。
- ⚠️ `lingxi-hooks` 之前不调 `lingxi-tools`,M5-06 wiring 需要回调 tool dispatch(`hook → trigger tool`)。**解法:** 同样 trait 反向,hooks 用 `Arc<dyn ToolInvoker>`(已存在 from M4-05 wiring)。

### 6.2 后向兼容(M4 不能 regress)

- ✅ 40 个 builtin tools 全部继续 work,`parity_registry_40_tools.rs` 必须通过。
- ✅ `tengu::tool::NAMES.len() == 134` 不变(只在 `release` namespace 加事件,不动 tool 事件)。
- ✅ M4-05 wiring 的两个 `Arc::ptr_eq` 测试 — M5-01 改 runner pump 后必须仍通过。
- ✅ M3-05 字节锁定的 `"Budget exceeded ($X.YZ); stopped."` denial 串通 — M5-02 orchestrator 加 budget check 时复用。

### 6.3 Telemetry schema 增长

| 阶段 | `ALL_EVENT_NAMES` | `tengu::tool::NAMES` |
|---|---|---|
| v0.5.0 (现在) | 238 | 134 |
| M5-02 后 | 241 (+3) | 134 |
| M5-04 后 | 243 (+2) | 134 |
| M5-05 后 | 245 (+2) | 134 |
| M5-06 后 | 253 (+8) | 134 |
| M5-07 后 | 256 (+3) | 134 |
| M5-08 后 | 258 (+2) | 134 |
| M5-10 后 | 276 (+18) | 134 |
| M5-11 后 | 312 (+36) | 134 |
| M5-13 后 | 314 (+2) | 134 |
| v0.6.0 (M5-14) | **315 (+1)** | 134 |

`tengu_events.json` parity fixture + `event_name_completeness_test.rs` + `settings_schema_test.rs` 每个 sub-plan 同步更新。

### 6.4 新增 telemetry 子模块

`lingxi_telemetry::tengu::orchestrator`(新子模块,持 orchestrator/streaming/permission/hooks/session/resume/repl 事件) + `lingxi_telemetry::tengu::command`(新子模块,持 18 commands × 3 事件)。`release` 子模块继续承担版本 marker。

### 6.5 设置 schema 增长

M3-01 settings.json schema 在 v0.4.0 锁定 143 字段。M5 可能需要新增:

- `orchestrator.max_turns: u32`(默认 30)
- `orchestrator.auto_compact_threshold: f32`(默认 0.95)
- `cli.repl_prompt: String`(默认 `"> "`)
- `cli.color: bool`(TTY auto-detect)
- `cli.json_output: bool`

新字段需在 M5-12 / M5-13 加入 `lingxi_core::settings::SettingsJson` + bump settings schema test counter(152 → 157)。

### 6.6 文档更新

- `CHANGELOG.md` — v0.6.0 section,列 14 sub-plans + 累计字节锁
- `README.md` — status table 加 "M5 Conversational Loop" row,标记完成
- `docs/superpowers/releases/2026-05-XX-v0.6.0.md` — 发布 summary
- 每个 sub-plan 完成时,该 plan markdown 文件存 git history;v0.6.0 release doc 链 14 个 plan files

### 6.7 风险 & 缓解

| 风险 | 影响 | 缓解 |
|---|---|---|
| claude-code system prompt 模板太长/含动态片段 | M5-03 工作量爆 | 接受"行为等价 + 段落顺序锁",不要求每字节锁 |
| Golden session 文件版本飘移(claude-code 更新格式) | M5-07 测试 break | 在 fixture 加 `_claude_code_version: "X.Y.Z"` 标记,M6+ 测试时按当前版本 generate 新 golden |
| `--resume` 跨进程互通失败(djb2 hash 实现差异) | M5-08 acceptance fail | M5-07 task 0 移植 djb2 + 用 claude-code 实例 cross-check 几个 cwd 的 hash |
| Streaming SSE event 顺序与文档不符 | M5-04 实测 fail | task 0 抓真 `claude` 的 SSE log(MITM proxy / 日志)作为 golden;无法抓时回退按官方 docs |
| Hook executor 在 SSRF guard 误报 | M5-06 hook 失败 | 继续用 M1.7 已锁的 SsrfGuard + 加 allow-list 配置 |
| `lingxi-cli` Windows stdin 阻塞 | M5-13 在 Windows REPL hang | 用 `tokio` non-blocking + Windows-specific `winapi` adapter |

---

## §7 Open Questions

这些是在 plan-writing 阶段需要研究/确定的项,不阻塞 design 通过,但每个 sub-plan 的 Task 0 第一步要解决:

| # | 问题 | 影响 plan | 解决方式 |
|---|---|---|---|
| OQ-1 | Main conversation 的 `maxTurns` default 是多少?(claude-code 只在 agent frontmatter 里定义,主 loop 默认值未知) | M5-02 | grep `QueryEngine.ts` + 主 entry,如确无默认则 LingXi 锁 30 |
| OQ-2 | claude-code 的 SSE event 完整订阅列表 + 顺序(`ping` / `error` 等是否处理) | M5-04 | 抓真 `claude -p "hi"` 的 HTTP 日志 OR 读 `src/services/api/` |
| OQ-3 | 各 tool 的 permission default Y/N | M5-05 | 跑 `claude` 实际触发各 tool,看 `[Y/n]` vs `[y/N]` |
| OQ-4 | Hook event JSON schema(各 hook event_type 的 payload 字段) | M5-06 | 读 `src/hooks/` 各 use* hook |
| OQ-5 | Session JSONL `message` 内嵌结构(Anthropic Messages API vs 内部 SDKMessage 多 dialect) | M5-07 | 抓 golden session 文件 + 读 `src/services/api/sessionIngress.ts` |
| OQ-6 | `--resume` 不带 ID 的 interactive 选择 UI(claude-code 是 Ink TUI,我们 stdio fallback) | M5-08 | 简化为 `1-5: <name> [<modified>]` 列表 + 数字选择 |
| OQ-7 | `/init` 生成的 CLAUDE.md 模板内容(可能数百行 markdown) | M5-10 | 直接 cat claude-code `src/commands/init.ts` 内联 template |
| OQ-8 | `/help` 输出格式(列宽 / 分类 / 颜色码) | M5-10 | 跑 `claude /help` 抓 stdout |
| OQ-9 | 12 个 batch 2 命令的输出格式各自 | M5-11 | 逐个 `claude /<cmd>` 抓 stdout |
| OQ-10 | `--json` flag 输出 schema(claude-code 是否在 `-p` 时自动 emit JSON,还是显式 flag) | M5-12 | 读 `src/cli/print.ts` 或运行实测 |
| OQ-11 | REPL 提示符 + 颜色 | M5-13 | 读 `src/screens/REPL.tsx` + 跑 `claude` 实例(可能要在干净 TTY 下) |
| OQ-12 | claude-code 当前版本 → 我们字节锁的 baseline 版本号 | 整体 | 用 `package.json` 标记版本,所有 fixtures 标注 `_claude_code_version` |

---

## §8 Release Plan

### 8.1 Sub-plan 发布节奏

| Plan | 目标周 | 累计周 | 中间状态 |
|---|---|---|---|
| M5-01 Engine wiring close-out | 1 | 1 | v0.5.0 follow-ups 全清,Arc::ptr_eq 闭环 |
| M5-02 Orchestrator core (batched) | 1 | 2 | Mock model 下可跑通最简 turn |
| M5-03 System prompt 动态组装 | 0.5 | 2.5 | Memory 整合 system prompt |
| M5-04 Streaming SSE | 1 | 3.5 | 真 SSE 链路 |
| M5-05 Permission UX | 0.5 | 4 | y/N stderr prompts |
| M5-06 Hooks 4-arm | 1 | 5 | hooks 真触发 |
| M5-07 Session JSONL | 1 | 6 | Golden session 字节等价 |
| M5-08 Resume | 0.5 | 6.5 | 跨 `claude`/`lingxi-cli` 互通 |
| M5-09 Commands surface 102 | 0.5 | 7 | 全部 102 可识别 |
| M5-10 Commands core batch 1 | 0.5 | 7.5 | `/clear /compact /help /exit /memory /init` 通 |
| M5-11 Commands core batch 2 | 1 | 8.5 | 12 命令通 |
| M5-12 CLI binary | 0.5 | 9 | `lingxi-cli -p "X"` 可用 |
| M5-13 REPL mode | 0.5 | 9.5 | `lingxi-cli` 交互可用 |
| M5-14 Release v0.6.0 | 0.5 | 10 | 跨切 parity + tag |

**总估时:10 周**(单 claw,按 M4 实际产出节奏)。

### 8.2 Tag 链

```
v0.5.0  (已锁,prev release)
  ├─ m5.1  Engine wiring close-out
  ├─ m5.2  Orchestrator core
  ├─ m5.3  System prompt
  ├─ m5.4  Streaming
  ├─ m5.5  Permission UX
  ├─ m5.6  Hooks 4-arm
  ├─ m5.7  Session JSONL
  ├─ m5.8  Resume
  ├─ m5.9  Commands surface
  ├─ m5.10 Commands core batch 1
  ├─ m5.11 Commands core batch 2
  ├─ m5.12 CLI binary
  ├─ m5.13 REPL mode
  └─ m5.14 Release v0.6.0
v0.6.0  (M5 done)
```

每个 `m5.N` 是 annotated tag。`v0.6.0` 是最终 annotated release tag。

### 8.3 v0.6.0 发布物清单

- Binary:`target/release/lingxi-cli`(macOS/Linux/Windows)
- 38 个 Cargo.toml 全部 `version = "0.6.0"`
- `CHANGELOG.md` v0.6.0 section
- `README.md` status table 更新
- `docs/superpowers/releases/2026-05-??-v0.6.0.md` 发布 summary
- 14 个 plan markdown 文件(已 commit)
- Annotated tags `m5.14` + `v0.6.0`
- 4 个 cross-cutting parity drivers(parity_orchestrator / parity_slash_commands / parity_session_jsonl / parity_hooks_runtime)

### 8.4 Push 策略

按用户既有偏好:**所有 tag 保留本地,不 push 远端**。用户决定推送时机。

### 8.5 何时进入 M6 brainstorm

v0.6.0 标签设上之后,自然进入 M6(TUI 基础)的 brainstorm。不自动连续推进。

---

## References

- M1 design (foundation): `docs/superpowers/specs/2026-05-22-lingxi-core-rust-engine-design.md`
- M3 design (engine completion): `docs/superpowers/specs/2026-05-23-m3-engine-completion-design.md`
- M4 design (tools 全集): `docs/superpowers/specs/2026-05-24-m4-tools-implementation-design.md`
- M3-02 memory hierarchy lock (used by M5-03 system prompt)
- M3-05 budget format lock (used by M5-02 orchestrator)
- M3-06 telemetry schema lock (extended in §6.3)
- claude-code source: `/Users/luolingfeng/Projects/LingXi-Next/claude-code/src/`
  - `QueryEngine.ts` — turn loop reference
  - `utils/sessionStoragePortable.ts` — session JSONL format
  - `commands/` — 102 slash commands
  - `hooks/` — hooks runtime
  - `screens/REPL.tsx` — REPL UI(M6 reference, M5 stdio fallback)
  - `cli/print.ts` — `-p` print mode
