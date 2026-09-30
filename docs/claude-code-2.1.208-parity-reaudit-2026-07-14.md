# LingXi Code 与 Claude Code 2.1.208 全局 Parity 再审计

> Historical milestone/audit record. Paths and validation describe the recorded version; use [the current product layout](development/multi-repo-workflow.md) for navigation and build instructions.

> 审计日期：2026-07-14  
> Claude Code 基线：`2.1.208`  
> LingXi 源码基线：`a5f0e8f3c`  
> 上一版报告：[`claude-code-2.1.207-parity-audit-2026-07-13.md`](./claude-code-2.1.207-parity-audit-2026-07-13.md)  
> 结论：**当前仍不能声明与 Claude Code 2.1.208 达到 1:1 parity**

## 1. 结论摘要

本轮是在 Claude Code 对项目进行大量修复后的生产链路再审计。相比上一版报告，显著进展包括：显式 Agent worktree isolation、child permission-mode gate、主会话与 subagent partial-output 保留、compaction 冷恢复、ToolSearch/deferred tools、`--add-dir`/MCP roots、managed permission、task-notification provenance，以及普通 task locking 等核心路径已经关闭或明显改善。

但重新沿生产调用链检查后，仍确认 **32 组未关闭 gap：11 个 P1、14 个 P2、7 个 P3**。这里的数字按“可独立修复和验收的缺陷簇”统计，不等同于代码行数或单个函数数量。

最需要优先处理的是：

1. LSP 对外 schema 使用 `filePath`，但调用前校验读取 `file_path`，导致合法生产调用在执行前被拒绝；现有单测反而只测试了错误的 snake_case 输入。
2. `claude agents`/background daemon 仍不是可 attach、可原位恢复的 live session；daemon crash 后可能重新执行原 prompt，而不是从 transcript 继续。
3. 异步 Agent 返回的 UUID 与 TaskRegistry 的 `aXXXXXXXX` ID 是两套身份，`TaskStop`/`TaskOutput` 无法用 Agent 返回值寻址。
4. persistent Agent 的 TaskStop/失败路径只终止外层 pump，没有 deallocate 内层 StateMachinePool runner；`TaskHandle.cleanup` 又在 registry 中被直接丢弃。
5. `apiKeyHelper` 只有 resolver/执行器定义，没有进入实际鉴权请求链。
6. print/stream-json 输出 writer 是 detached task，主流程入队后立即 `process::exit`，大结果仍可能被截断。
7. shell substitution 中的灾难性删除没有获得与直接 `rm -rf` 相同的不可自动批准保护。
8. Read 的 offset/limit 路径仍先读取、解码整文件，超长单行可造成内存暴涨。

因此，当前状态适合表述为“2.1.207 多项核心 gap 已修复，2.1.208 parity 正在收敛”，不适合对外声称“1:1 copy”或“full parity”。

## 2. 范围、排除项与判定标准

覆盖范围：

- orchestrator、turn loop、conversation、streaming；
- agent、subagent、background agent、workflow、worktree；
- sandbox、permission、hooks；
- tools、MCP、memory、compaction、tasks；
- LSP、file、web；
- CLI print/stream-json、daemon、resume、session/file-history。

按约定排除以下项目特定差异：

- 多 LLM provider 与其 provider-specific 适配；
- `.lingxi` 与 `.claude` 等自定义目录、文件名本身；
- branding、产品命名；
- LingXi 额外扩展能力。

但“实现方式不同”不构成排除理由。只要用户可观察语义、安全边界、生命周期、恢复能力或工具契约与 Claude Code 不同，仍计为 gap。

基线通过本机 `claude --version`、npm dist-tag 与 Anthropic 官方 [`CHANGELOG.md`](https://github.com/anthropics/claude-code/blob/main/CHANGELOG.md) 交叉确认；审计时最新版本为 `2.1.208`。Subagent、permission、worktree 和 agent-view 语义还与官方文档交叉核对：[`sub-agents`](https://code.claude.com/docs/en/sub-agents)、[`permissions`](https://code.claude.com/docs/en/permissions)、[`worktrees`](https://code.claude.com/docs/en/worktrees)、[`agents`](https://code.claude.com/docs/en/agents)。

## 3. P1 Findings

| ID | Gap | 生产证据 | 影响 |
|---|---|---|---|
| P1-01 | LSP `filePath`/`file_path` 契约断裂 | schema 和 `call()` 读取 camelCase：[`tools/lsp/src/lsp_tool.rs#L1118`](../crates/tools/lsp/src/lsp_tool.rs#L1118)、[`#L1221`](../crates/tools/lsp/src/lsp_tool.rs#L1221)；`get_path`/`validate_input` 却读取 snake_case：[`#L1173`](../crates/tools/lsp/src/lsp_tool.rs#L1173)；turn loop 在 `call()` 前校验 raw input：[`orchestrator/src/turn_loop.rs#L2313`](../lingxi-code/orchestrator/src/turn_loop.rs#L2313)。 | 模型按 schema 发出的合法 LSP 请求会在生产调用前失败；definition/hover/references 等工具整体不可可靠使用。 |
| P1-02 | Background service 不具备 live attach/真实 resume | live job 明确拒绝 attach：[`apps/cli/src/commands/agents.rs#L523`](../apps/cli/host/src/commands/agents.rs#L523)；worker 没有 PTY/rendezvous：[`apps/cli/src/commands/daemon.rs#L521`](../apps/cli/host/src/commands/daemon.rs#L521)；crash respawn：[`#L450`](../apps/cli/host/src/commands/daemon.rs#L450)；重启后重新构造 prompt/runtime：[`apps/cli/src/commands/bg_worker.rs#L140`](../apps/cli/host/src/commands/bg_worker.rs#L140)；dispatch env 为空：[`apps/cli/src/background_dispatch.rs#L189`](../apps/cli/host/src/background_dispatch.rs#L189)。 | 不能边运行边 attach/reply/stop；失败投递无法持久化；daemon 重启可能重复原始副作用，且 shell 环境丢失。 |
| P1-03 | Agent UUID/name 与 Task ID 不可互换 | Agent 对外返回 UUID：[`apps/engine-desktop/src/background_agent.rs#L96`](../crates/apps/engine-desktop/src/background_agent.rs#L96)、[`tools/agent/src/agent.rs#L991`](../crates/tools/agent/src/agent.rs#L991)；handler 另生成 `a...` ID：[`tasks/src/handlers/local_agent.rs#L266`](../lingxi-code/tasks/src/handlers/local_agent.rs#L266)；TaskStop/TaskOutput 直接 `registry.get(input)`：[`tools/task/src/task.rs#L2005`](../crates/tools/task/src/task.rs#L2005)、[`#L2494`](../crates/tools/task/src/task.rs#L2494)。 | 模型拿到的 Agent 返回值不能用于停止或读取该 Agent；name 也没有统一 alias resolver。 |
| P1-04 | Persistent Agent 停止/失败泄漏真实 runner | persistent spawn 只返回 pool receiver、没有 deallocation owner：[`agent/src/handle.rs#L1039`](../lingxi-code/agent/src/handle.rs#L1039)；runner 会驻留等待新消息：[`agent/src/runner.rs#L1394`](../lingxi-code/agent/src/runner.rs#L1394)；TaskStop 仅 cancel 外层 worker：[`tasks/src/handlers/local_agent.rs#L572`](../lingxi-code/tasks/src/handlers/local_agent.rs#L572)；registry 只取 `handle.task_id`，丢弃 cleanup：[`tasks/src/registry.rs#L240`](../lingxi-code/tasks/src/registry.rs#L240)。 | 内层 Agent 可能继续持有 pool slot、进程或工具副作用；多次 stop/fail 后可耗尽并发槽。 |
| P1-05 | `apiKeyHelper` 没有进入请求鉴权链 | 文件注释直接标明 live per-request auth 尚未接线：[`llm-client/src/oauth/anthropic/api_key_helper.rs#L24`](../lingxi-code/llm-client/src/oauth/anthropic/api_key_helper.rs#L24)；`fetch_api_key` 除定义/测试外无生产调用。 | 配置看似被接受但实际不工作；2.1.208 的“3 次内显示 helper 自身错误”也不可能实现。 |
| P1-06 | 大型 print/stream-json 最终结果可能被截断 | drain task 被 detached spawn 且不保存 JoinHandle：[`apps/cli/src/stream_json.rs#L85`](../apps/cli/host/src/stream_json.rs#L85)、[`#L259`](../apps/cli/host/src/stream_json.rs#L259)；run 只把 result 入队就返回：[`apps/cli/src/run.rs#L1029`](../apps/cli/host/src/run.rs#L1029)；main 随即 `process::exit`：[`apps/cli/src/main.rs#L9`](../apps/cli/host/src/main.rs#L9)。 | OS 来不及 flush 时丢失尾部或整个 result message，命中 2.1.208 明确修复的问题。 |
| P1-07 | substitution 内灾难性删除仍可被自动放行 | dangerous-removal 只检查 split 后首 token 为 `rm`/`rmdir`：[`permission/src/dangerous_removal.rs#L426`](../lingxi-code/permission/src/dangerous_removal.rs#L426)；sandbox auto-allow 先于该 guard：[`permission/src/policy.rs#L495`](../lingxi-code/permission/src/policy.rs#L495)；generic substitution safety 是 classifier-approvable，且 exact allow 在它之前返回：[`#L623`](../lingxi-code/permission/src/policy.rs#L623)；现有测试明确锁定 exact allow 可绕过 substitution safety：[`permission/src/policy_test.rs#L2641`](../lingxi-code/permission/src/policy_test.rs#L2641)。 | `$(rm -rf ~)`、backticks、process substitution 不能保证像直接 `rm -rf ~` 一样强制人工确认；属于数据破坏安全边界。 |
| P1-08 | Read offset/limit 的长单行路径仍全量加载 | explicit limit 绕过普通 cap：[`tools/file/src/read.rs#L1734`](../crates/tools/file/src/read.rs#L1734)；随后读取并解码整个文件：[`#L1785`](../crates/tools/file/src/read.rs#L1785)；最后才做行切片：[`#L1895`](../crates/tools/file/src/read.rs#L1895)。 | 极长单行或大文件可造成内存暴涨/进程 OOM，未达到 2.1.208 的 clean-error 行为。 |
| P1-09 | Agent definition/workflow 的 worktree isolation 只解析不执行 | Agent tool 代码明确注明 definition frontmatter isolation 未线程化：[`tools/agent/src/agent.rs#L1614`](../crates/tools/agent/src/agent.rs#L1614)；workflow 只把 `isolation` 写入 request，`cwd/worktree` 仍为空：[`tasks/src/handlers/local_workflow.rs#L408`](../lingxi-code/tasks/src/handlers/local_workflow.rs#L408)；child 只消费 `request.cwd`：[`agent/src/handle.rs#L976`](../lingxi-code/agent/src/handle.rs#L976)。 | `isolation: worktree` 自定义 Agent 和 workflow agent 仍可能写父 checkout。显式 Agent tool 参数路径已修复，但不是完整语义。 |
| P1-10 | Read 状态仍有 legacy Vec 与 rich LRU 两套真相 | 两份状态并存：[`orchestrator/src/conversation.rs#L986`](../lingxi-code/orchestrator/src/conversation.rs#L986)；turn-loop dispatch 写 legacy state 且使用固定 `orch.cwd`：[`orchestrator/src/turn_loop.rs#L122`](../lingxi-code/orchestrator/src/turn_loop.rs#L122)；`/files`、conditional rules、memory dedup 分别读取不同集合：[`orchestrator/src/handle_impl.rs#L625`](../lingxi-code/orchestrator/src/handle_impl.rs#L625)、[`orchestrator/src/conversation.rs#L8541`](../lingxi-code/orchestrator/src/conversation.rs#L8541)、[`#L8649`](../lingxi-code/orchestrator/src/conversation.rs#L8649)。 | 已读文件、规则激活、memory 去重和 UI 结果可互相矛盾；现有测试甚至断言两者独立。 |
| P1-11 | 已完成后台 task 被自动通知后立即删除 | next-turn notification 遍历 terminal task 并删除：[`tasks/src/registry.rs#L545`](../lingxi-code/tasks/src/registry.rs#L545)；`mark_notified` 同样 remove：[`#L458`](../lingxi-code/tasks/src/registry.rs#L458)；通知只带 output path，未完整保留 agent result/usage。 | `/tasks` 中完成项提前消失，随后 TaskOutput 得到 NotFound；与 2.1.208“保留到 cleanup/显式读取”相反。 |

### P1-01 为什么现有测试没有发现

[`tools/lsp/src/lsp_tool.rs#L2005`](../crates/tools/lsp/src/lsp_tool.rs#L2005) 的校验测试构造的是 `file_path`，恰好满足错误实现；模型可见 schema 和真实 `call()` 却要求 `filePath`。需要新增一个穿过 `TurnLoop -> validate_input -> call` 的 camelCase 集成测试，而不是继续分别测试两半。

### P1-02 恢复语义不是“重启同一 prompt”

Claude Code 的 agent view 是可观察、可 attach 的后台会话面；官方 [`agents` 文档](https://code.claude.com/docs/en/agents) 明确描述查看、attach、回复和停止正在运行的工作。当前 daemon 只保存 job 元数据并重启 headless worker，`bg_worker` 没有从 JSONL 恢复到中断点的生产链。这样做在只读 prompt 上看似可用，但在已经执行 Write/Bash/外部 API 后会重复副作用。

### P1-04 需要一个统一 lifecycle owner

目前存在三层异步对象：TaskRegistry record、LocalAgent 外层 event pump、StateMachinePool runner。停止和终止必须由一个 owner 原子地完成：发 `UserExit` 或 cancel runner、deallocate pool slot、撤销 mailbox/name alias、处理 worktree、保留 terminal result、再更新 task 状态。只 abort 外层 future 不能满足这一契约。

## 4. P2 Findings

| ID | Gap | 证据/说明 |
|---|---|---|
| P2-01 | 同扩展名 LSP fallback 仍只尝试第一个 server | registry 选择 `names.first()`：[`lsp/src/registry.rs#L258`](../lingxi-code/lsp/src/registry.rs#L258)；首 server 初始化/请求失败直接返回：[`#L445`](../lingxi-code/lsp/src/registry.rs#L445)。未覆盖 Claude Code 2.1.205 的 plugin LSP fallback 修复。 |
| P2-02 | LSP open documents 无 50-doc LRU/didClose | tracker 是无界 `HashSet`：[`lsp/src/open_file_tracker.rs#L16`](../lingxi-code/lsp/src/open_file_tracker.rs#L16)，只 insert/显式 clear：[`#L35`](../lingxi-code/lsp/src/open_file_tracker.rs#L35)；tool operation 持续 open：[`lsp/src/tool_operations.rs#L161`](../lingxi-code/lsp/src/tool_operations.rs#L161)。 |
| P2-03 | Agent `tools` 解析为空时静默启动无工具 Agent | resolver 对未知名只 `filter`：[`agent/src/tool_resolver.rs#L162`](../lingxi-code/agent/src/tool_resolver.rs#L162)，随后允许空 schemas/allow-list：[`#L253`](../lingxi-code/agent/src/tool_resolver.rs#L253)。2.1.208 要求返回包含未识别项的明确错误。 |
| P2-04 | stream-json whitespace/non-string `set_model` 仍不符合 2.1.208 | 输入只去 CR 并检查 `is_empty()`，不 trim 空白：[`apps/cli/src/stream_json_input.rs#L352`](../apps/cli/host/src/stream_json_input.rs#L352)、[`#L441`](../apps/cli/host/src/stream_json_input.rs#L441)；非字符串 `set_model` 被静默替换为默认模型：[`apps/cli/src/run.rs#L286`](../apps/cli/host/src/run.rs#L286)。 |
| P2-05 | Grep count pagination 低报 total | 先正确累计 `total_matches`：[`tools/file/src/grep.rs#L720`](../crates/tools/file/src/grep.rs#L720)，count mode 却在分页后用有限 rows 重算 total/file count：[`#L855`](../crates/tools/file/src/grep.rs#L855)。 |
| P2-06 | Markdown table 没有 200 行上限 | parser 收集全部 row：[`tui-core/src/render/markdown.rs#L473`](../apps/cli/tui-core/src/render/markdown.rs#L473)，renderer 遍历全部：[`tui-core/src/render/markdown_table.rs#L311`](../apps/cli/tui-core/src/render/markdown_table.rs#L311)。 |
| P2-07 | file-history 只裁内存索引，不清理旧 backup 文件 | snapshot 上限 100：[`session/src/file_history.rs#L29`](../lingxi-code/session/src/file_history.rs#L29)；prune 只 drain Vec：[`#L235`](../lingxi-code/session/src/file_history.rs#L235)；backup 持续落盘：[`#L393`](../lingxi-code/session/src/file_history.rs#L393)。 |
| P2-08 | MCP 空 URL 仍被当成 remote config | `Some("")` 直接构建 remote：[`mcp/src/json_config.rs#L226`](../lingxi-code/mcp/src/json_config.rs#L226)；diagnostic 的 `has_url` 只检查类型，不检查非空：[`mcp/src/config_diagnostics.rs#L183`](../lingxi-code/mcp/src/config_diagnostics.rs#L183)。 |
| P2-09 | hook runtime 输出中的 `{"async":true}` 丢失 eventual output | executor 收到 Backgrounded 后直接返回空 Success：[`hooks/src/executor.rs#L1066`](../lingxi-code/hooks/src/executor.rs#L1066)；POSIX runner 把剩余 stdout/stderr drain 到 sink 后丢弃：[`platforms/posix/src/process/runner.rs#L329`](../lingxi-code/platforms/posix/src/process/runner.rs#L329)。配置级 `blocking:false` 的 AsyncHookRegistry 已接通，本项只针对 runtime marker 路径。 |
| P2-10 | Compact 冷恢复主体已修复，但 metadata/skill state 不完整 | `preTokens=0`、logical parent/userContext/messagesSummarized/discoveredTools 使用空值：[`orchestrator/src/conversation.rs#L2610`](../lingxi-code/orchestrator/src/conversation.rs#L2610)；invoked skills restore 明确未接线：[`#L2472`](../lingxi-code/orchestrator/src/conversation.rs#L2472)。 |
| P2-11 | hosted WebSearch 自然 EOF 无 incomplete notice | parser 在没有 `message_stop` 的 EOF 上仍成功返回已有内容：[`tools/web/src/web_search.rs#L1211`](../crates/tools/web/src/web_search.rs#L1211)。主模型/subagent/显式 stream error 的 partial-output 已修复。 |
| P2-12 | 科学计数法 env 值仍按 mantissa 解析 | context-window env parser：[`llm-client/src/model/context_window.rs#L204`](../lingxi-code/llm-client/src/model/context_window.rs#L204)；数字扫描只取开头 digits：[`#L336`](../lingxi-code/llm-client/src/model/context_window.rs#L336)，`1e6` 变 `1`。 |
| P2-13 | 缺少 `CLAUDE_CODE_PROCESS_WRAPPER` 等价能力 | repo 无对应配置；daemon/background/agent-view self-spawn 直接 `Command::new(current_exe)`：[`apps/cli/src/commands/daemon.rs#L81`](../apps/cli/host/src/commands/daemon.rs#L81)、[`apps/cli/src/background_dispatch.rs#L245`](../apps/cli/host/src/background_dispatch.rs#L245)、[`apps/cli/src/commands/agents.rs#L540`](../apps/cli/host/src/commands/agents.rs#L540)。企业 launcher/policy 环境会被绕过。 |
| P2-14 | per-spawn plan mode 只在 permission gate 生效，工具 schema 仍暴露写工具 | `resolve_tools` 在 effective child mode 计算前运行：[`agent/src/handle.rs#L964`](../lingxi-code/agent/src/handle.rs#L964)；resolver 只按静态 definition plan mode 缩窄：[`agent/src/tool_resolver.rs#L215`](../lingxi-code/agent/src/tool_resolver.rs#L215)。运行时 gate 能阻挡，但模型仍看到并尝试 Write/Bash，和 Claude 的 plan tool surface 不同。 |

## 5. P3 Findings

| ID | Gap | 证据/说明 |
|---|---|---|
| P3-01 | permission rule matcher 每次重新编译 regex | shell glob 在 match 热路径 `Regex::new`：[`permission/src/shell_rule_matching.rs#L162`](../lingxi-code/permission/src/shell_rule_matching.rs#L162)；domain wildcard 同样逐次编译：[`permission/src/policy.rs#L1621`](../lingxi-code/permission/src/policy.rs#L1621)。 |
| P3-02 | MCP tool-pool 每 turn 重建 | 代码注释明确“no session-level toolSchemaCache analog yet”：[`orchestrator/src/conversation.rs#L8777`](../lingxi-code/orchestrator/src/conversation.rs#L8777)，且 prompt 逐个 await：[`tool-api/src/wire.rs#L156`](../lingxi-code/tool-api/src/wire.rs#L156)。 |
| P3-03 | file read/edit cache 仍为 25 MB，而非 2.1.208 的 16 MB | 常量和 constructor：[`tool-api/src/read_file_state.rs#L44`](../lingxi-code/tool-api/src/read_file_state.rs#L44)、[`#L222`](../lingxi-code/tool-api/src/read_file_state.rs#L222)。 |
| P3-04 | 缺少 `vimInsertModeRemaps` | settings/schema/TUI 全局无对应字段或 two-key insert remap state machine。 |
| P3-05 | workflow user-scope save UX 缺失 | loader 只加载项目相对 workflow 路径：[`tools/workflow/src/lib.rs#L119`](../crates/tools/workflow/src/lib.rs#L119)；当前 `/workflows` 是运行浏览器，没有 Claude 2.1.208 对应的 user-scope save dialog。目录名差异不计，缺少能力本身计入。 |
| P3-06 | parity target/harness 仍停在 2.1.207 | 常量：[`platform-api/src/lib.rs#L11`](../lingxi-code/platform-api/src/lib.rs#L11)；测试仅有 [`test-harness/tests/parity_claude_2_1_207.rs`](../lingxi-code/test-harness/tests/parity_claude_2_1_207.rs)。 |
| P3-07 | OAuth callback test 存在并行竞态 | 测试先探测 ephemeral port、drop 后再 bind，并自述 race acceptable：[`llm-client/src/oauth/anthropic/callback.rs#L269`](../lingxi-code/llm-client/src/oauth/anthropic/callback.rs#L269)。全 crate 运行稳定失败，单测独立运行通过，说明 test isolation/ready handshake 不足。 |

## 6. 上一版 P1 的复核状态

| 旧 ID | 状态 | 本轮判断 |
|---|---|---|
| 2.1.207 P1-01 默认 async Agent 绕过显式 worktree | **主体关闭** | Agent tool 的显式 `isolation:"worktree"` 已在 async/sync 分支前创建并线程化；definition/workflow 仍有 P1-09。 |
| P1-02 `mode:"plan"` 完全未执行 | **部分关闭** | effective child mode 已进入运行时 permission gate；模型可见工具集合仍未按 per-spawn mode 缩窄，见 P2-14。 |
| P1-03 TaskStop/Output 无法寻址 Agent | **未关闭** | 仍是 UUID、task ID、name 三套地址空间，见 P1-03。 |
| P1-04 partial work 丢失 | **主体关闭** | 主模型、subagent 和显式 WebSearch stream error 已保留 partial；自然 EOF 仍见 P2-11。 |
| P1-05 compact 冷 resume 回到压缩前 | **主体关闭** | summary/replacement history 已持久化恢复；metadata/skill state 仍见 P2-10。 |
| P1-06 三份 read-file state | **部分关闭** | rich state 已共享，但 legacy Vec 与 rich LRU 仍分裂，见 P1-10。 |
| P1-07 ToolSearch/deferred pipeline | **关闭** | 生产 resolver、advertisement 与 dispatch 已形成链路。 |
| P1-08 `--add-dir`/roots/MCP roots | **关闭** | additional roots 已进入 file permission 和 MCP roots/list/change 通知。 |
| P1-09 LSP fallback/multi-file route | **部分关闭** | multi-file/route 有修复；当前新增 P1-01 参数契约错误，same-extension fallback 仍见 P2-01。 |
| P1-10 managed permission 未进生产策略 | **关闭** | managed/remote rules 已进入生产 policy merge 与 precedence。 |
| P1-11 task notification 缺 non-human provenance | **关闭** | main、subagent 和 notification prompt 已注入明确 provenance。 |
| P1-12 `--bg` 一次性 worker | **未关闭** | 仍缺 live attach、可靠投递、真实 resume、version-safe handover，见 P1-02。 |
| P1-13 TodoStore 单进程锁假设 | **主体关闭** | 正常 task create/update locking 已补；失败时 fail-open/stale recovery 仍建议作为下一轮并发 fault-injection 项。 |

## 7. Claude Code 2.1.208 Delta 覆盖矩阵

以下是与本项目核心相关的 2.1.208 changelog 项逐项核对。provider-specific、billing、browser UI 等已按排除项跳过。

### 已覆盖或基本覆盖

- AX screen reader mode；
- fast mode 在切回支持模型后恢复；
- Edit 在 read 后文件变化、但目标仍唯一时恢复；
- Read empty file；
- Grep invalid regex 明确报错；
- MCP stdio stderr 64 MB ring cap；
- config `blocking:false` async hook registry/output foldback；
- `/release-notes` 不注入模型上下文；
- 主模型/subagent mid-stream partial work；
- explicit Agent worktree isolation；
- task notification non-human provenance。

### 未覆盖或部分覆盖

- `vimInsertModeRemaps`：P3-04；
- process wrapper：P2-13；
- background reply persistence/attach/update-safe resume：P1-02；
- large JSON/stream-json flush：P1-06；
- scientific notation env：P2-12；
- table 200-row cap：P2-06；
- Grep count pagination：P2-05；
- Glob NUL：当前没有显式 NUL guard；pattern/path/cwd 的错误类型尚缺一条端到端 regression，本报告暂列证明缺口而非单独 blocker；
- `apiKeyHelper` error：P1-05；
- whitespace stream-json/non-string `set_model`：P2-04；
- empty Agent tools：P2-03；
- workflow user-scope save：P3-05；
- LSP 50-doc LRU：P2-02；
- Read long-line bound：P1-08；
- permission matcher cache：P3-01；
- MCP tool-pool cache：P3-02；
- 16 MB read/edit cache：P3-03；
- file-history backup pruning：P2-07；
- completed task retention：P1-11；
- catastrophic removal in substitutions：P1-07；
- MCP empty URL：P2-08。

## 8. 按用户指定领域的结论

| 领域 | 状态 | 主要未关闭项 |
|---|---|---|
| orchestrator / turn_loop / conversation | 部分 parity | LSP pre-validation、Read 双状态、compact metadata、MCP tool-pool rebuild |
| agent / subagent | 不满足 parity | 双 ID、persistent runner cleanup、definition isolation、empty tools、plan schema |
| workflow | 部分 parity | isolation 未兑现、user-scope save UX 缺失 |
| sandbox / permissions | 高风险部分 parity | substitution catastrophic removal；matcher cache |
| hooks | 主体可用、仍有窄 gap | runtime `async:true` eventual output 丢失 |
| tools | 不满足 parity | LSP 工具断裂、Read long-line、Grep count、large output |
| MCP | 部分 parity | empty URL、tool-pool cache；stderr cap 已修复 |
| memory / compact | 主体恢复可用 | compact metadata/invoked skills、Read state split |
| tasks | 不满足 parity | Agent address space、runner lifecycle、terminal retention |
| LSP | 不满足 parity | camel/snake contract、fallback、open-doc LRU |
| file | 部分 parity | long-line、Grep count、history pruning、cache bound |
| web | 接近 parity | hosted WebSearch natural EOF incomplete notice |
| background/session | 不满足 parity | live attach、reply durability、resume、env/version handover |

## 9. 动态验证结果

执行并读取了以下结果：

```text
cargo test -q \
  -p tool-lsp -p lsp -p permission -p tool-file -p tasks \
  -p tool-web -p agent -p mcp -p orchestrator -p cli --lib
=> 3,114 passed; 0 failed

cargo test -q -p test-harness --test parity_claude_2_1_207
=> 5 passed; 0 failed

cargo test -q -p hooks --lib
=> 330 passed; 0 failed

cargo test -q -p tui-core --lib
=> 252 passed; 0 failed

cargo test -q -p session --lib
=> 101 passed; 0 failed

cargo test -q -p llm-client --lib
=> 664 passed; 1 failed

cargo test -q -p llm-client --lib \
  oauth::anthropic::callback::tests::await_callback_wrapper_binds_explicit_port -- --exact
=> 1 passed; 0 failed
```

`llm-client` 的失败不是 parity 结论的推测，而是可重复的 suite-level test race：整 crate 两次分别出现 `Connection reset by peer` 和 `Connection refused`，单独执行则通过。它已列为 P3-07。

这些测试为绿也不能反证上述 gap，因为多个缺陷位于跨层组合处：

- LSP 单测分别接受了错误 snake_case，没有穿过模型 schema；
- Task handler 测试直接持有 cleanup closure，生产 registry 却丢弃它；
- read-state 测试明确允许 rich/legacy 独立；
- 现有 parity harness 仍只覆盖 2.1.207 的 5 个窄场景。

## 10. 建议修复顺序与验收门槛

### 第一批：阻断数据损坏与不可控副作用

1. 修复 P1-07：在任何 allow/sandbox/auto/bypass 分支前，对 `$(...)`、backticks、`<(...)` 内递归检测 catastrophic removal，并标记不可 classifier approve。
2. 修复 P1-04：让 persistent spawn 返回 RAII lifecycle handle；stop/fail/drop 均 deallocate pool、撤销 route、处理 worktree。
3. 修复 P1-02：禁止“从原 prompt 伪恢复”；在真正 transcript resume 前，crash 应 fail closed，避免重复 side effects。
4. 修复 P1-08 与 P1-06：流式 bounded Read；print writer 显式 close/flush/join 后才能退出。

### 第二批：恢复核心工具和 Agent 契约

1. 统一 LSP 字段为 `filePath`，删除双命名；补 production turn-loop 集成测试。
2. 建立一个 AgentAddressResolver，使 UUID、task ID、name 都指向同一 lifecycle record。
3. 将 Agent definition/workflow isolation 下沉到唯一 spawn resolver，所有入口共用。
4. 把 `apiKeyHelper` 接入每次 Anthropic auth resolution，并按 helper 错误分类/重试。
5. 合并 legacy/rich read state，所有消费者只读同一 canonical store。
6. terminal tasks 只在显式 cleanup/delete 后移除；读取输出不应破坏列表状态。

### 第三批：2.1.208 行为和性能收口

- stream-json control、Grep count、table cap、LSP LRU/fallback、MCP empty URL；
- compact metadata/skills、file-history backup pruning；
- permission/MCP matcher 和 tool-pool cache；
- process wrapper、vim remap、workflow save；
- 把 parity 常量和 harness 升到 2.1.208。

### 声明 1:1 parity 前必须新增的端到端测试

- schema 真实字段经过 turn loop 调用 LSP；
- async Agent 返回的 ID 立即用于 TaskOutput/TaskStop/name stop；
- stop/fail/cancel 后 pool slot、mailbox、runtime handle、worktree 均为零泄漏；
- daemon 在 Write/Bash 之后 crash，不重复原 turn；attach/reply/stop 可跨重启；
- 50+ MB stream-json result 完整落盘/pipe，含最终 result record；
- 超长单行 Read 返回 bounded error，RSS 不随整行增长；
- substitution 中的 `rm -rf ~` 在 auto/bypass/exact allow/sandbox 下全部强制人工确认；
- completed task 在通知后仍可 list/output，直到 cleanup；
- 2.1.208 changelog 的每个非排除项至少一条 parity regression。

## 11. 最终判断

大量修复是真实且有效的，尤其是上一轮最严重的 managed rules、ToolSearch、roots、partial-output、compact 冷恢复和 provenance 问题已经明显收敛。但当前剩余项仍覆盖安全、后台恢复、Agent 生命周期、LSP 可用性和输出完整性，不能被视为边缘 UI 差异。

**最终 verdict：NO-GO for “Claude Code 2.1.208 1:1 parity” claim。**

建议在 P1 全清、P2 有明确接受/排除记录、2.1.208 harness 升级并加入跨层故障注入测试后，再进行一次 release-candidate 审计。
