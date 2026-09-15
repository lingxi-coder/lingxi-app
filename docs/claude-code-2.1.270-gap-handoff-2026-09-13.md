# Handoff：LingXi Code × Claude Code 2.1.270 全量 Gap 审计

> 用途：交接给另一个 LLM 独立审核。
> 审计日期：2026-09-13
> 审计者：LingXi agent（主 agent + 6 路并行子系统子代理）
> Claude Code 基线：**2.1.270**（`npm view @anthropic-ai/claude-code version`；本机 CLI 2.1.269）
> 目标代码：`lingxi-code/`（Rust cargo workspace）
> 上一轮全局审计基线：2.1.208（`docs/claude-code-2.1.208-parity-reaudit-2026-07-14.md`）

---

## 0. 审核者如何用这份文档

**这是差距盘点，不是修复实现。** 请重点独立核验：

1. 标注 **H** 的条目能否在给定 `file:line` 处直接复现；
2. 标注 **M/L** 的条目是否存在别名实现或不同代码路径（子代理可能漏查）；
3. §5「未验证清单」是否需要下钻；
4. 是否有子系统被整体遗漏。

**重要声明：**
- 本轮**未运行测试套件**，结论均为源码阅读 + changelog 交叉比对。
- 本地 `claude-code/src` 是泄露快照（结构约 2.1.8x，**旧于** 2.1.270），只作契约结构参考；2.1.209→2.1.270 行为以官方 CHANGELOG 为准。
- P2/P3 中有「全仓符号零命中」判定，理论上可能以别名存在 → 已标 L。

---

## 1. 审计范围与方法

**覆盖子系统：** orchestrator/turn loop/streaming、session/resume、compaction、memory、agent/subagent、task/coordinator/workflow、hooks、permission、sandbox、tools（file/shell/web/lsp/meta/plan/ui/…）、MCP、plugin、skills、CLI/commands/print/stream-json、cost、cron、config、TUI。

**方法：** 按子系统并行深读 `lingxi-code` 当前树，逐条回归 2.1.208 旧发现，并对 2.1.209→2.1.270 changelog 新增期望做 grep/定向核验。仓库内 `lingxi-code/docs/` 已有更新到 2.1.267/2.1.270 的分模块审计（loop/permission/agent/task/mcp/skills），本文件在其基础上补 2.1.268–2.1.270 增量并做全局汇总。

**判定标准：** 用户可观察语义、安全边界、生命周期/恢复能力、工具契约任一不同即计 gap；实现方式不同不算。排除：多 LLM provider 差异、`.lingxi`/`.claude` 命名、品牌、LingXi 独有扩展。

**Confidence：** H=读当前代码可确认；M=代码路径可推断/依赖 changelog 适用性；L=仅“零命中/未找到消费者”。

**主要参考的仓库内既有审计（建议审核者也读）：**
- `lingxi-code/docs/loop-parity-2.1.270-2026-09-12.md`（最新，loop/cron）
- `lingxi-code/docs/permission-byte-alignment-2.1.263-2026-09-07.md` + `HANDOFF-permission-2026-09-09.md`
- `lingxi-code/docs/agent-byte-alignment-2.1.266-2026-09-08.md`、`task-parity-audit-2026-09-07.md`、`parity-2.1.267-task-kill-family-2026-09-10.md`
- `lingxi-code/docs/parity-2.1.267-mcp-plugin-2026-09-10.md`、`parity-2.1.267-skills-2026-09-10.md`
- `lingxi-code/docs/compact-byte-alignment-2.1.261-2026-09-04.md`

---

## 2. 结论摘要

1. **无 P0**：未发现正常流程中的数据丢失或安全越权。
2. **旧账基本还清**：2.1.208 的多数 P1 已关闭（LSP 契约、后台 attach/resume、persistent runner 泄漏、stream-json 截断、substitution 灾难性删除、Read 状态分裂、compact 冷恢复元数据）。
3. **仍开放的最高优先（P1）**：Read 大文件整文件读入内存；`permission_denials` 恒空；TaskStop 无法用 Agent UUID 寻址；MCP server mode 缺失；memory 默认关闭/非项目隔离/无团队同步/无 auto-dream；`/rewind` 假成功；部分压缩缺失；插件解压保留 world-writable 权限。
4. **P2 主力**是 2.1.228–2.1.270 新增量：`bashOutputMaxChars`、Glob/Grep 权限顺序、WebFetch 上限、1 GB 结果上限、Bash 编辑 diff、`/output-style`、`--permission-prompts none` 等。

---

## 3. 已验证关闭的旧 Gap（回归结论）

| 旧 ID | 项目 | 状态 | 当前证据（LingXi） |
|---|---|---|---|
| P1-01 | LSP `filePath`/`file_path` 契约断裂 | FIXED | `tools/lsp/src/lsp_tool.rs:1146-1151` 优先 `filePath` 回退 `file_path`；`get_path:1213`、`validate_input:1224`、`call:1255`；schema `:1162` |
| P1-02 | 后台无 live attach / 假 resume / 空 env | FIXED | PTY attach v2：`apps/cli/src/commands/attach.rs:27-59`、`agents.rs:976-993`；daemon fail-closed 不重放 `daemon.rs:20-21,1274-1275,1832-1834`，测试 `:4320`；resume 载精确 transcript `bg_worker.rs:757,788-828`；env 传递 `background_dispatch.rs:888-994` |
| P1-03 | Agent UUID/name 与 Task ID 不可互换 | 基本 FIXED，残留 AG-1 | alias 解析器 `tasks/src/registry.rs:626-666`，别名 `:5346-5366`；`kill/get/output/SendMessage` 可解析；仅 TaskStop 工具级解析器未接入 |
| P1-04 | persistent Agent 停止/失败泄漏 runner | FIXED | cleanup 保存并执行 `registry.rs:2152,2245-2246`；`SpawnDeallocGuard` `agent/src/handle.rs:2953-2979`；测试 `kill_deallocates_persistent_inner_runner` |
| P1-05 | `apiKeyHelper` 未进入鉴权链 | FIXED | `provider-config/src/credentials.rs:101-114` 在 OS store 前调用，回显 `apiKeyHelper failed: {e}` |
| P1-06 | print/stream-json 大结果截断 | FIXED | FIFO 屏障 `apps/cli/src/stream_json.rs:537-543,173-176`；所有 result 路径 await `run.rs:836,854,2801,2827,2833`；入口先 join `main.rs:47-48` |
| P1-07 | substitution 内灾难性删除可被自动放行 | FIXED | 守卫前移到 sandbox auto-allow 与 exact-allow **之前** `permission/src/policy.rs:1211-1266`；substitution 扫描 `dangerous_removal.rs:563`；调用 `policy.rs:1223` |
| P1-10 | 旧 Vec 与新 LRU read state 并存 | FIXED | 单一 `ReadFileStateMap` `orchestrator/src/conversation/runtime.rs:250,462` |
| P2-09 | runtime `async:true` hook eventual output 丢失 | FIXED | `hooks/src/async_registry.rs`；测试 `hooks/src/executor_test.rs:3370-3374` |
| P2-10 | compact 元数据 / invoked-skill 状态不全 | FIXED | `orchestrator/src/conversation/compaction.rs:335-336,1250,616-623` |
| — | Glob/Grep null-byte/整数守卫 | FIXED | `tools/file/src/grep.rs:851-866`、`glob.rs:246-254` |
| — | read/edit cache 25MB→16MB | FIXED | `tool-api/src/read_file_state.rs:54-56` |
| — | MultiEdit 缺失 | CLOSED（正确） | 有意不注册 `tools/file/src/lib.rs:255-259`；2.1.270 上游亦无 MultiEdit |

---

## 4. 仍开放 — P1

### 4.1 Orchestrator / 输出契约

| ID | Gap | CC 2.1.270 期望 | LingXi 现状 | 影响 | Conf |
|---|---|---|---|---|---|
| OR-1 | stream-json `result.permission_denials` 恒为 `[]` | 2.1.269：结果帧须含被 path-scoped deny 拦截的 Read/Edit/Write | 三 builder 硬编码 `apps/cli/src/stream_json.rs:976,1124,1218`；`run.rs:2805-2835` 从不注入；事件级 `permission_denied` 存在（`control_plane.rs:112`）但无聚合成 result | SDK/桌面结果摘要永远看不到拒绝 | H |

### 4.2 Agent / Task

| ID | Gap | CC 期望 | LingXi 现状 | 影响 | Conf |
|---|---|---|---|---|---|
| AG-1 | TaskStop 不能用 Agent 返回的 UUID 寻址，TaskOutput 可以 | async agent 的 task id 即 agent id：`LocalAgentTask.tsx:488`、`AgentTool.tsx:734,758`、`TaskStopTool.ts:72` | `Agent` 返回 `agentId:<uuid>`（`tools/agent/src/agent.rs:820-830`），纯异步 `task_id=None`（`:3277`）；TaskStop 经 `resolve_stop_target`（`tools/task/src/task.rs:2406`→`tasks/src/handle.rs:533-550`→`tasks/src/resolve.rs:9-120`）只匹配 task_id/teammate/命名，不查 alias map；TaskOutput 经 `registry.get`（`task.rs:3164`→`registry.rs:2292-2295`）会解析别名 | 用同一 id，Stop 失败、Output 成功；模型被明确告知该 id 可用 | M |

### 4.3 Memory / Session / Compaction

| ID | Gap | CC 期望 | LingXi 现状 | 影响 | Conf |
|---|---|---|---|---|---|
| MEM-1 | auto-memory 默认关闭 | 默认 true（`memdir/paths.ts:30-58`） | `# Memory` 段仅 prefetch 接线时输出；prefetch 受 `LINGXI_MEMDIR_PREFETCH` 控制默认 OFF：`orchestrator/src/conversation/prompt.rs:699-704`、`apps/engine-desktop/src/lib.rs:15285-15299`、`orchestrator/src/prompt/mod.rs:299-304` | 开箱无记忆指令、无召回 | H/M |
| MEM-2 | auto-memory 目录用户全局，未按项目隔离 | `<base>/projects/<sanitized-git-root>/memory/`（`memdir/paths.ts:223-233`） | 固定 `<config-home>/memdir`：`memory/src/memdir/paths.rs:43-48`、`orchestrator/src/prompt/memory_section.rs:230,239` | 跨仓库记忆串扰 | M |
| MEM-3 | 团队记忆无服务端同步 | 双向 GET/PUT + checksum 增量 + per-repo + 上传跳过密钥（`services/teamMemorySync/index.ts`、`memdir/teamMemPaths.ts:73-94,228-284`） | 只读本地轮询 watcher；加载时脱敏而非上传跳过；无路径穿越防护：`memory/src/team_memory.rs:15-91`、`memory/src/memdir/team_paths.rs:11-19` | 团队笔记不传播 | H/M |
| MEM-4 | auto-dream 无调度器 | 24h+5 session 门 + consolidation lock（`services/autoDream/autoDream.ts`） | handler 存在但无调度；自述 deferred `tasks/src/handlers/dream.rs:41-43,477-481`；`autoDreamEnabled` 无消费者 `tools/meta/src/config.rs:223-225` | 后台巩固不自动运行 | H |
| SES-1 | `/rewind` / `--rewind-files` 假报成功 | 2.1.222：备份缺失不报成功；symlink/hardlink 恢复被跳过并计数 | 缺备份静默 no-op `session/src/file_history.rs:479-495`；空也返回 `Ok` `:289-338`；CLI 见 Ok 即打印成功 `apps/cli/src/run.rs:3749-3754`；`fs::copy` 跟随链接 `:328-334,488-494` | 数据恢复假成功；symlink 可写穿 | H |
| CMP-1 | 部分压缩（从某条消息起摘要）缺失 | `services/compact/compact.ts:772,840` | 仅尾部选择 `compaction/src/partial.rs:65`；无 partial prompt `compaction/src/prompt.rs:20-27`；picker deferred `tui/src/bottom_pane/rewind_picker_view.rs:20-23` | 无法保留选定近期前缀 | H |

### 4.4 MCP / Plugin

| ID | Gap | CC 期望 | LingXi 现状 | 影响 | Conf |
|---|---|---|---|---|---|
| MCP-1 | MCP server mode 整体缺失 | `src/entrypoints/mcp.ts` 暴露 CC 工具（`tools/list`/`tools/call`）+ `mcp-server/` | `apps/cli` 无 `mcp serve`；`mcp/src/inbound.rs` 仅客户端 inbound，无 `tools/list` handler | 无法把 LingXi 当 MCP server | M |
| MCP-2 | 插件压缩包解压保留 world-writable / 不清理陈旧文件 | 2.1.269 修复归档可读、权限继承、陈旧文件残留 | `configuration-admin/src/plugin_download.rs:197-216`→`plugin::unpack_plugin_archive`；`plugin/src/mcpb.rs` 无 `set_permissions`/umask；`plugin_install.rs` 无相关逻辑 | 多用户机器可读/篡改插件 | M |

---

## 5. 仍开放 — P2

### 5.1 Tools

| ID | Gap | CC 期望 | LingXi 现状 | Conf |
|---|---|---|---|---|
| TL-1 | Read `offset/limit` 大文件仍整文件 `read_to_end`（P1-08 残余） | ranged read 流式 | 流式分支仅 `input_limit.is_some() && size>MAX`：`tools/file/src/read.rs:2144-2157`；数据 `platform-api/src/rooted_fs.rs:1275-1277`；逐字节扫描 `read.rs:386-413` | H |
| TL-2 | Write 无按模型的 read-before-write 门 | 2.1.228：新模型可覆盖未读文件 | 无条件 `FILE_NOT_READ_ERROR`：`tools/file/src/lib.rs:180-193`；prompt 仍宣称会失败 `write.rs:40-42` | M |
| TL-3 | 缺 `bashOutputMaxChars` 设置 | 2.1.270：与 `taskOutputMaxChars` 一并生效，上限 128K | 仅 `task_output_max_chars` `core/src/settings/schema.rs:341`；Bash 上限只来自 env `tools/shell/src/bash.rs:307-332` | H |
| TL-4 | Glob/Grep 权限判定前探测磁盘 | 2.1.259：先判权限再报不存在 | `validate_input` 先于 `check_permissions` `tool-api/src/tool_invoker_impl.rs:290-297`；stat `glob.rs:258`、`grep.rs:867` | M |
| TL-5 | WebFetch 无整体 300s deadline | 2.1.269 `CLAUDE_CODE_WEBFETCH_DEADLINE_MS` | 仅每请求 60s `tools/web/src/web_fetch.rs:86-89,1167` | M |
| TL-6 | 工具结果落盘 1 GB 上限缺失 | 2.1.266 | 全仓无常量；最近为后台 spool 5GB/8MB `tasks/src/output_manager.rs:35,67` | M |
| TL-7 | Todo/Task 工具门是 denylist 而非 allowlist | 2.1.268 | 反选 `tool-api/src/todo_tools_gate.rs:9-23` | L |

### 5.2 Orchestrator / Side-query

| ID | Gap | CC 期望 | LingXi 现状 | Conf |
|---|---|---|---|---|
| OR-2 | `/btw` 缺“不得编造工具调用”护栏 | 2.1.269 | 旧文案 `orchestrator/src/conversation/model.rs:911`；答案原样返回 `:925-971`；`commands/core/src/side_question.rs:76-101` | H |
| OR-3 | 中断回合自动续跑 + 6h 上限缺失 | 2.1.211 / 2.1.269 | env 仅在 scrub 名单 `platforms/posix/src/process/runner.rs:116`，无消费者 | M |
| OR-4 | `/goal` 遇错无退避重试 | 2.1.269 | 仅时间/idle 阶梯 `orchestrator/src/conversation/hooks.rs:1004,1009-1025` | L |
| OR-5 | 版本常量落后 | 2.1.270 | `platform-api/src/lib.rs:43` = `"2.1.267"` | H |

### 5.3 Session / Compact / Memory（其余）

| ID | Gap | CC 期望 | LingXi 现状 | Conf |
|---|---|---|---|---|
| CMP-2 | cached microcompact（cache-editing）缺失 | `services/compact/microCompact.ts:336` | `cached_microcompact.rs` 仅同输入结果缓存；`microcompact.rs:65-67` 未建模 | M |
| CMP-3 | compact 后不告知 REPL VM 变量已清空 | `UOt` 的 `o` 参数 | `compaction/src/prompt.rs:315-316` deferral | M |
| MEM-5 | MEMORY.md 上限按原始内容计；frontmatter 无 `modified` | 排除 frontmatter/注释；ISO `modified` | `memory/src/index_cap.rs:88-95`；`memory/src/file.rs:28-39` | M |
| MEM-6 | session-memory 提取器默认不跑 | 回合末接线 | composition root 默认 OFF `apps/engine-desktop/src/lib.rs:15285-15299`、`orchestrator/src/prompt/memory_block.rs:352-386` | M |
| SES-2 | `queue-operation` 从不写入 | 策略 `always` | 策略表 `session/src/jsonl/transcript_compact.rs:191,1021`，无生产者 | M |
| SES-3 | `content-replacement` 仅 fork/branch 写入 | 普通会话亦持久化 | `session/src/branch.rs:166-224` | M |
| SES-4 | orchestrator 层不恢复后台 worker 实时状态 | resume 重连后台会话 | `orchestrator/src/resume.rs` 无 live attach（与 CLI 层已修复的 P1-02 不同层） | M |

### 5.4 Agent / Task（其余）

| ID | Gap | CC 期望 | LingXi 现状 | Conf |
|---|---|---|---|---|
| AG-2 | 无 per-run workflow 并发上限 | 2.1.269 `CLAUDE_CODE_WORKFLOW_MAX_CONCURRENT_AGENTS` | 仅全局 `CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS` `platform-api/src/subagent_spawn.rs:839-881`；按批整组派发 `workflow/src/lib.rs:1331,1515` | H |
| AG-3 | 后台 agent 运行中仍可能报 waiting for input | 2.1.269；`CLAUDE_CODE_BG_TASKS_REPORT_RUNNING=0` 回退 | 空闲提示 60s 定时不查后台 `apps/cli/src/idle_notify.rs:43,66-90`；无条件武装 `repl_loop.rs:122` | M |
| AG-4 | `TeamCreate`/`TeamDelete` 缺失 | CC 提供（`coordinatorMode.ts:28-33`） | `tools/team/` 空；测试钉死不存在 `apps/engine-desktop/tests/coordinator_activation.rs:427,470`；改用隐式 team。**可能有意** | M |
| AG-5 | 2.1.268 teammate-respawn 信任修复无法确认 | respawn 不得取不可信同名 agent 文件 | 无 `respawn_teammate` 路径，危害未验证 | L |
| AG-6 | 2.1.265/267 prompt-cache 前缀稳定性 | SubagentStart 上下文/preload skill 保持前缀 | 本轮未验证 | L |

### 5.5 Hooks / Permission / Sandbox

| ID | Gap | CC 期望 | LingXi 现状 | Conf |
|---|---|---|---|---|
| HP-1 | `permission_denials`（同 OR-1） | 2.1.269 | 见 OR-1 | H |
| HP-2 | `--permission-prompts none` 缺失 | 2.1.259 无人值守自动拒绝 | `apps/cli` 零命中 | H |
| HP-3 | plan-mode 审批/同意下限整套缺失（28 条） | plan×artifacts×teammates×auto-mode | crate 无 substrate；`HANDOFF-permission-2026-09-09.md` §4.4 | H |
| HP-4 | session 级 `BK` 允许项缺失 | 会话目录放行 | 无会话目录 plumbing；**使 read block 更严格，不更松** | H |
| HP-5 | MCP 审批文案 13 / remote-control·--print 权限宿主 4 / host-asserted `classifierContext` 2 | 对应上游 | HANDOFF §4.5 记为 P2 上限；substrate 已有 | M |
| HP-6 | sandbox 凭据 `sandbox.credentials.awsPairs` 缺失 | 2.1.257 | 全仓 `awsPairs`/`aws_pairs` 零命中 | M |
| HP-7 | `!` 前缀 deny/ask 规则仅本 source 生效 + 裸 `!` 忽略 | 2.1.269 | 未找到 `!` 规则否定解析（偏 fail-safe） | L |

**已确认具备（非 gap）：** `PreModelSwitch`/`PostModelSwitch`（`hooks/src/events.rs:54`、`orchestrator/src/handle_impl.rs:121`、`orchestrator/src/conversation/hooks.rs:1646`）；SessionEnd 超时 env（`hooks/src/executor.rs`）；`allowedHttpHookUrls`/`httpHookAllowedEnvVars`/`allowedChannelPlugins` fail-closed（`core/src/settings/schema.rs`、`hooks/src/http_executor.rs`）；sandbox 域名尾点归一（`sandbox-runtime/src/host.rs:56`）；`allowUnsandboxedCommands`（`sandbox/src/runtime_config.rs:245`）；2.1.270 auto-mode containment-escape 模板（`permission/src/bundled/auto_mode_270_*.txt`）。

### 5.6 MCP / Plugin / Skills（其余）

| ID | Gap | CC 期望 | LingXi 现状 | Conf |
|---|---|---|---|---|
| MP-1 | 内联 skill `disallowed-tools` 无消费者 | 激活期间移除工具，下条用户消息清除 | 解析于 `command-api/src/model.rs:224`、`markdown_loader.rs:99-100,799`；dispatcher 只读 `allowed_tools` `dispatcher.rs:594`；Skill 仅 fork 路径用 `tools/skill/src/skill.rs:140,414` | H |
| MP-2 | marketplace `command` 源与 `mode:"link"` 缺失 | 新增 command 源 | `plugin/src/source.rs:25-43`；`marketplace.rs:93-151` 无 command 分支 | H |
| MP-3 | “Unknown skill” 不提示 plugin skill 全名 | 2.1.269 | 固定文案 `tools/skill/src/skill.rs:841,921` | M |
| MP-4 | MCP 重连身份用原始 URL | 2.1.269 query 重排不重连 | `mcp/src/discovery_cache.rs` 只规范化 JSON key | M |
| MP-5 | GitLab marketplace / `additionalMarketplaces` 别名不支持 | 裸 gitlab URL；别名键 | 全仓 `gitlab` 零命中；`plugin/src/git.rs:143-201` github 中心 | M |
| MP-6 | `--sparse` / `skipLfs` 缺失 | 注册源携带并复用 | 全仓无实现命中 | M |
| MP-7 | MCP/plugin 遥测词表与 scaffold 缺失 | — | 项目 251 审计 §20b/§20c/§24a/§25b；非用户可观察 | M |

### 5.7 CLI / Commands / Cost / Config

| ID | Gap | CC 期望 | LingXi 现状 | Conf |
|---|---|---|---|---|
| CLI-1 | `/output-style [name]` 缺失 | 2.1.269 重新加入 | 基于 2.1.183 判定为已移除：`command-api/src/builtin_support/names.rs:8-14`；`tui/src/command.rs` 未注册 | H |
| CLI-2 | `--system-prompt-snapshot off` 缺失 | 每次请求重渲染 | `apps/cli/src/argv.rs` 零命中 | H |
| CLI-3 | `--append-subagent-system-prompt-file` 缺失（仅 inline） | 从文件读过大的 subagent 提示 | 仅 `--append-subagent-system-prompt` `apps/cli/src/argv.rs:1091` | H |
| CLI-4 | `/cost` 缺 prompt-cache 行；无 `prompt_cache` 状态字段；`modelPricing` 被忽略 | 每会话 cache 命中/未命中/重缓存/冷热 + 状态行 + 组织合同价 | 旧块 `cost/src/render.rs:179-209`；`cost/`、`tui/src/status_line.rs`、`core/src/settings/schema.rs` 无 `prompt_cache`/`model_pricing` | M |
| CLI-5 | `bashEditDiffEnabled` / Bash 文件 diff 缺失 | 2.1.269 | 全仓零命中 | M |
| CLI-6 | `CLAUDE_CODE_BG_TASKS_REPORT_RUNNING` 缺失（同 AG-3） | 2.1.269 | 零命中 | M |
| CLI-7 | resumed session 只持久化 cost 总额 | `cost-tracker.ts` 存 `lastModelUsage`/`lastAPIDuration` | `apps/cli/src/session_cost.rs:19-24` 只写 `lastCost`/`lastSessionId` | H |
| CLI-8 | `--settings` 超大文件守卫缺失 | >2 MiB 启动报错 | `apps/cli/src/lib.rs`/`init.rs` 无守卫 | L/M |

**Cron / loop：** 按 `lingxi-code/docs/loop-parity-2.1.270-2026-09-12.md` 已闭合；接受差异为 per-task `expiresAt` 取代全局 7 天过期。**TUI：** vim/键位/六套主题齐备，无重大缺失。

---

## 6. 未验证清单（建议审核者下钻）

- 2.1.268 PermissionRequest hooks 在 `--print` 模式触发；
- 2.1.268 `env -C`/`eval` 等不可分析命令同行时 Read/Edit deny 生效；
- 2.1.268/269 符号链接目录按真实拼写的 deny/ask 规则；
- 2.1.269 `Edit()` deny 与 Bash `tee` 写路径联动（工具侧已见 `tools/shell/src/command_semantics.rs:126-153,305-309`，权限侧未验证）；
- 2.1.257 `permissions.ask` 在 compound/subshell 不再被跳过；
- 2.1.257 项目 settings 中 `defaultMode:"bypassPermissions"` 被忽略；
- 2.1.260 zsh REPORTTIME/DIRSTACKSIZE 藏 substitution、算术赋值 `OPTIND=1/0` 必须询问；
- 2.1.246 悬挂 `&&`/`||` 必须询问；
- 2.1.247 hook 输出过载/无法写文件的内存与溢出保护；
- 2.1.248 hook stdout 非法 JSON 对象要报 hook error；
- 2.1.243 hook `if` 条件遇 `$()`/反引号不应误触发；
- 2.1.265 SubagentStart 上下文/preload skill 的 prompt-cache 前缀稳定（同 AG-6）。

---

## 7. 结论矩阵

| 领域 | 状态 | 主要未闭合项 |
|---|---|---|
| orchestrator / turn loop / streaming | 基本 parity | OR-1、OR-2、OR-3、OR-4 |
| session / resume | 基本 parity | SES-1、SES-2、SES-4 |
| compaction | 部分 parity | CMP-1、CMP-2、CMP-3 |
| memory | **不满足 parity** | MEM-1/2/3/4/5/6 |
| agent / subagent | 基本 parity | AG-1、AG-2、AG-3 |
| task / coordinator / workflow | 基本 parity | AG-4、AG-5、AG-6 |
| hooks | 主体可用 | §6 运行时边界 |
| permission / sandbox | 基本 parity，安全边界已修复 | HP-2、HP-3、HP-6、HP-7 |
| tools | 部分 parity | TL-1…TL-7 |
| MCP | 部分 parity | MCP-1、MP-4 |
| plugin | 部分 parity | MCP-2、MP-2、MP-5、MP-6 |
| skills | 基本 parity | MP-1 |
| CLI / commands | 基本 parity | CLI-1…CLI-8 |
| cron / loop | **parity（接受已知差异）** | 无 |
| cost | 部分 parity | CLI-4、CLI-7 |

---

## 8. 建议修复优先级

1. **安全/数据恢复**：SES-1、MCP-2、OR-1/HP-1。
2. **契约断裂**：AG-1、TL-1、MCP-1（若产品需要）。
3. **特性可见性**：MEM-1/2、MEM-4、MEM-3。
4. **新增量对齐**：TL-3、TL-4、TL-5、TL-6、CLI-1/2/3/4/5。
5. **§6 未验证项**走官方 2.1.270 oracle 专项复核。

---

## 9. 局限

- 未运行测试；除 H 外建议以一次驱动测试或 2.1.270 oracle 二进制复核。
- 本地泄露源约 2.1.8x，2.1.209+ 精确字节契约未逐字比对。
- 部分“缺失”由全仓零命中判定，可能以别名实现（已标 L）。
- 本文件只做差距盘点，不含修复实现。
