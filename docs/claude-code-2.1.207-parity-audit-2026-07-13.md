# LingXi Code 与 Claude Code 2.1.207 全局 Parity 审计

> 审计日期：2026-07-13  
> Claude Code 基线：`2.1.207`  
> 审计对象：`lingxi-code`  
> 结论：**当前仍不能声明与 Claude Code 2.1.207 达到 1:1 parity**

## 1. 摘要 

本轮是在项目完成大量修复后的再次全局审计。审计覆盖：

- orchestrator、turn loop、conversation；
- agent、subagent、workflow、background session；
- sandbox、hooks、tools、MCP；
- memory、compaction、tasks；
- LSP、file、web；
- CLI daemon、resume、JSONL persistence。

按约定排除了以下项目特定差异：

- 多 LLM provider；
- LingXi 自定义配置目录和文件名；
- branding、产品命名和非 Claude Code 的扩展能力。

基线通过本机 `claude --version`、npm dist-tags 和 Anthropic 官方 changelog 交叉确认：本机与 npm `latest`/`next` 均为 `2.1.207`。

本轮未发现 P0，但仍存在多项 P1：包括 Agent 隔离与权限模式失效、managed policy 未执行、compaction 冷恢复错误、后台任务 ID 不可寻址、partial output 丢失、ToolSearch/MCP/LSP 生产链断裂，以及后台 daemon/任务存储的多进程一致性问题。

## 2. 严重级别

- **P0**：可直接导致广泛数据破坏、任意代码执行或系统不可用；必须立即阻断发布。
- **P1**：核心功能、安全边界或持久化语义在正常生产路径可复现地失效；应阻断 parity 声明。
- **P2**：重要功能不完整、生命周期泄漏、竞态或较窄条件下的行为偏差。
- **P3**：标识、边界格式、低频兼容性或证明体系不足。

## 3. P1 Findings

### P1-01 默认异步 Agent 绕过 worktree isolation

#### 证据链

- [`tools/agent/src/agent.rs:1466`](../crates/tools/agent/src/agent.rs#L1466) 将本地 Agent 默认设为异步执行。
- 同文件约 1499–1521 行在 worktree 创建逻辑之前直接进入 `dispatch_async` 并返回。
- 同文件约 856–876 行只把原始 `isolation` 和 `cwd` 搬运到 `SubagentSpawnRequest`。
- 真正的 `create_worktree` 仅位于 [`tools/agent/src/agent.rs:1582`](../crates/tools/agent/src/agent.rs#L1582) 之后的同步路径。
- [`agent/src/handle.rs:971`](../lingxi-code/agent/src/handle.rs#L971) 只消费 `request.cwd`，没有消费 `request.isolation`。

#### 触发条件

调用默认后台 Agent，并传入 `isolation: "worktree"`，包括没有显式传入 `run_in_background` 的普通调用。

#### 影响

子 Agent 实际可能在父 checkout 中写文件，与模型和调用方看到的隔离契约不一致，可能造成并行 Agent 修改互相覆盖。

#### 上游差异

Claude Code 2.1.203 已修复 worktree-isolated subagent 在父 checkout 执行命令的问题；官方 subagent 文档要求 isolated Agent 的 Bash/PowerShell 在其 worktree 中执行。

#### 必需回归测试

异步 spawn 必须创建 worktree、将 child cwd 设置为该 worktree，并验证父 checkout 没有任何修改；同时覆盖成功清理、保留脏 worktree 和失败回滚。

### P1-02 Agent `mode: "plan"` 被接受但未执行

#### 证据链

- [`tools/agent/src/agent.rs:216`](../crates/tools/agent/src/agent.rs#L216) 的模型可见 schema 声明支持 `mode: "plan"`。
- mode 被搬运到 spawn request，但 [`agent/src/handle.rs:845`](../lingxi-code/agent/src/handle.rs#L845) 构建 child context 时没有读取 `request.mode`。
- [`agent/src/tool_resolver.rs:215`](../lingxi-code/agent/src/tool_resolver.rs#L215) 只按静态 `AgentDefinition.permission_mode` 缩窄工具。
- `SubagentSpawnRequest` 的注释仍将 permission-mode application 标记为 deferred。

#### 影响

调用者认为 child 处于只读 plan mode，但该 Agent 仍可能拥有 Write、Edit、Bash 等写能力。这是安全边界和模型工具契约的直接失效。

#### 必需回归测试

使用同一默认 Agent definition，分别以 `mode: "plan"` 和 `mode: "default"` spawn；断言 plan child 没有写工具并由 plan permission gate 约束。

### P1-03 TaskStop/TaskOutput 无法使用 Agent 返回的 UUID 或 name

#### 证据链

- [`apps/engine-desktop/src/background_agent.rs:97`](../crates/apps/engine-desktop/src/background_agent.rs#L97) 创建对外 Agent UUID。
- Task registry handler 又在 [`tasks/src/handlers/local_agent.rs:232`](../lingxi-code/tasks/src/handlers/local_agent.rs#L232) 生成独立 `aXXXXXXXX` task ID。
- Agent tool 只把对外 UUID 返回给模型。
- TaskStop 在 [`tools/task/src/task.rs:1981`](../crates/tools/task/src/task.rs#L1981) 附近直接用输入字符串查 registry。
- TaskOutput 在 [`tools/task/src/task.rs:2487`](../crates/tools/task/src/task.rs#L2487) 附近使用同样的直接 lookup。
- 生产路径不存在 UUID/name 到 registry task ID 的 alias resolver。

#### 影响

模型拿到的唯一 ID 不能用于停止 Agent 或读取 Agent 输出；按 Agent name 调用也失败。跨 Agent、嵌套 Agent 场景同样受影响。

#### 上游差异

Claude Code 2.1.203 明确修复了 TaskStop/TaskOutput 无法寻址其他 Agent 创建的后台任务的问题。

#### 必需回归测试

异步 spawn 后分别使用对外 UUID、name 和底层 task ID 调用 TaskStop/TaskOutput，并覆盖 nested/cross-agent 场景；miss 时应列出有效 running Agent ID 和描述。

### P1-04 主会话、subagent 和 hosted WebSearch 丢失 mid-stream partial work

#### 主会话

- [`orchestrator/src/streaming_loop.rs:297`](../lingxi-code/orchestrator/src/streaming_loop.rs#L297) 遇到 stream error 时只返回 error 和 content-start flag，不携带已累计 blocks。
- [`orchestrator/src/conversation.rs:5954`](../lingxi-code/orchestrator/src/conversation.rs#L5954) 最终只持久化 synthetic model error。

#### Subagent

- [`agent/src/accumulator.rs:402`](../lingxi-code/agent/src/accumulator.rs#L402) 对 stream item 使用 `?`，错误发生时丢弃已有 content。
- [`agent/src/runner.rs:912`](../lingxi-code/agent/src/runner.rs#L912) 只上报 `Failed(error)`。
- Agent tool 最终只返回 hard error，不包含 child 已完成的 partial work。

#### Hosted WebSearch

- `tools/web/src/web_search.rs` 的 streaming accumulator 同样在中途错误后直接返回 `ToolError`，已收到的 result/progress blocks 不会进入 tool result。

#### 影响

用户已经看见的输出不会进入 transcript/resume；parent Agent 也拿不到 subagent 已完成的分析或代码片段。

#### 上游差异

Claude Code 2.1.199 要求在 mid-stream overloaded/server/rate-limit 错误后保留 partial response，并附 incomplete-response notice。

#### 必需回归测试

构造 `MessageStart -> TextDelta("useful") -> Overloaded/RateLimited/Transport`，断言主会话 JSONL、resume、subagent result 和 WebSearch tool result 均保留 partial 内容及 incomplete 标记。

### P1-05 Compaction 只热更新内存，冷 resume 恢复压缩前历史

#### 证据链

- [`orchestrator/src/conversation.rs:2403`](../lingxi-code/orchestrator/src/conversation.rs#L2403) 构造 compact boundary 后丢弃完整 metadata。
- 同文件约 2452–2471 行只在内存中构造 `boundary + summary + preserved tail + attachments`。
- [`orchestrator/src/conversation.rs:2478`](../lingxi-code/orchestrator/src/conversation.rs#L2478) 只把 boundary marker 写入 JSONL，没有持久化 summary、tail 和 restored attachments。
- [`session/src/jsonl/loader.rs:989`](../lingxi-code/session/src/jsonl/loader.rs#L989) 的 cold loader 仍沿旧 `parentUuid` 链回溯。
- [`orchestrator/src/resume.rs:104`](../lingxi-code/orchestrator/src/resume.rs#L104) 跳过 boundary/system 行，但继续回放旧链上的 user/assistant 消息。

#### 触发条件

会话发生自动或手动 compaction，随后退出进程并 cold resume。

#### 影响

Summary、preserved tail、恢复附件和 metadata 消失，压缩前 raw history 重新进入模型上下文；token 节省、长期会话连续性和热/冷行为一致性全部失效。

#### 必需回归测试

使用生产 JSONL writer 强制 compact，销毁 orchestrator 后通过生产 loader resume；断言冷恢复历史与热会话完全一致，并且 summarized prefix 不再进入模型。

### P1-06 生产环境存在三份互不共享的 read-file state

#### 证据链

- [`apps/engine-desktop/src/lib.rs:4886`](../crates/apps/engine-desktop/src/lib.rs#L4886) 为文件工具创建一份 rich read-state map。
- [`orchestrator/src/conversation.rs:1270`](../lingxi-code/orchestrator/src/conversation.rs#L1270) 又创建独立 legacy Vec 和另一份 rich map。
- [`orchestrator/src/turn_loop.rs:122`](../lingxi-code/orchestrator/src/turn_loop.rs#L122) 成功执行 Read/Edit/Write 后只更新 legacy Vec。
- 文件工具内部只更新 `BuiltinToolContext.read_file_state`。
- [`orchestrator/src/conversation.rs:2345`](../lingxi-code/orchestrator/src/conversation.rs#L2345) post-compact drain 的是 Conversation 自己从未被生产文件工具填充的 rich map。

#### 影响

- 生产路径的 post-compact 文件恢复恒为空；
- `/files`、memory relevance/dedup、LRU/staleness 各自使用不同状态；
- fork/subagent 也无法可靠继承主会话的文件读取状态。

#### 必需回归测试

从 engine composition root 执行真实 Read 后 compact，断言 Conversation 与文件工具持有同一个 Arc，并验证恢复附件、LRU 次序、容量限制和 `/files` 输出一致。

### P1-07 ToolSearch 和 deferred-tool pipeline 两端均未接通

#### 证据链

- [`tools/meta/src/tool_search.rs:105`](../crates/tools/meta/src/tool_search.rs#L105) 的生产构造器安装空 `StaticRegistryView`。
- [`tools/meta/src/lib.rs:21`](../crates/tools/meta/src/lib.rs#L21) 的生产注册路径使用该空构造器。
- 生产代码没有使用 `with_view` 注入真实 registry；它只出现在测试。
- `Tool::should_defer()` 被多个工具实现，但没有生产 consumer。
- [`tool-api/src/registry.rs:69`](../lingxi-code/tool-api/src/registry.rs#L69) 仍返回所有 enabled builtin/dynamic tools。
- [`tool-api/src/wire.rs:155`](../lingxi-code/tool-api/src/wire.rs#L155) 仍把完整工具集合序列化到模型请求。

#### 影响

延迟工具实际上全部预加载，但 ToolSearch 又搜索空集合：既没有节省上下文，也无法按需发现工具。

#### 上游差异

Claude Code 默认启用 Tool Search；MCP tools 应 deferred，并在需要时通过 ToolSearch 动态进入上下文。

#### 必需回归测试

从真实 engine registry 发起模型请求，断言 deferred tool schema 首轮不在 wire 中；随后通过 ToolSearch 搜索并选择后，该 schema 才进入下一轮上下文。

### P1-08 `--add-dir`、文件 roots 和 MCP roots 没有形成闭环

#### 证据链

- [`apps/engine-desktop/src/lib.rs:3993`](../crates/apps/engine-desktop/src/lib.rs#L3993) 只把 `--add-dir` 合并到 permission policy。
- 文件工具的 `trusted_dirs` 仍只有启动 cwd。
- Bash cwd containment 也只认静态 workspace。
- [`mcp/src/inbound.rs:17`](../lingxi-code/mcp/src/inbound.rs#L17) 的 `roots/list` 固定返回单 cwd。
- 生产代码不存在 `notifications/roots/list_changed`。

#### 影响

UI/permission 看似允许额外目录，但 Read/Edit/Write/Glob/Grep/Bash/MCP 对该目录的认知并不一致。

#### 上游差异

Claude Code 2.1.203 要求 MCP roots 包含 additional working directories，并在集合变化时发送 `roots/list_changed`。

#### 必需回归测试

以 `--add-dir /tmp/extra` 启动，验证 Read、Edit、Glob、Grep、Bash `cd` 均成功，MCP `roots/list` 返回 cwd 和 extra，并在动态增删时发送通知。

### P1-09 LSP fallback 和多文件 route 都存在生产错误

#### 证据链

- [`lsp/src/registry.rs:147`](../lingxi-code/lsp/src/registry.rs#L147) 从 `HashMap` 选择第一个同扩展名 server。
- 第一个 server 启动或 initialize 失败时直接向上传播，不尝试其他候选。
- 已初始化 server 命中时直接返回 connection ID，没有为当前文件写 route cache。
- [`lsp/src/registry.rs:213`](../lingxi-code/lsp/src/registry.rs#L213) 的 `ensure_client_for_file` 随后要求当前精确 path 已存在 route，第二个同扩展文件因此可能返回 unavailable。

#### 影响

一个失效插件 LSP 会阻塞健康 LSP；即使首个文件成功，第二个相同扩展名文件也可能无法取得 client。

#### 上游差异

Claude Code 2.1.205 已明确修复“一个插件 LSP 初始化失败阻止另一个同扩展名 LSP”的问题。

#### 必需回归测试

1. 注册两个处理同扩展名的 server，第一个失败、第二个成功，断言第二个被尝试。
2. 依次打开 `a.rs` 和 `b.rs`，断言都路由到同一个健康 client。

### P1-10 Managed permission rules 没有进入生产策略

#### 证据链

- [`apps/engine-desktop/src/lib.rs:3957`](../crates/apps/engine-desktop/src/lib.rs#L3957) 只解析 user/project/local 三层 permission rules。
- [`apps/engine-desktop/src/lib.rs:4024`](../crates/apps/engine-desktop/src/lib.rs#L4024) 明确说明 managed raw settings 仅用于 sandbox derivation。
- 同处注释明确写明 managed permission rules 未加载。
- `PermissionRuleSource::PolicySettings` 虽存在，但没有进入该生产构建链。

#### 影响

企业 managed deny/ask/defaultMode、disableBypass 等策略可能不生效，用户或项目配置可以执行本应被组织策略禁止的工具操作。

#### 上游差异

Claude Code 规定 managed permission rules 具有最高优先级，且不能被 CLI、用户或项目设置覆盖。

#### 必需回归测试

使用临时 managed-settings 注入 deny rule、defaultMode 和 bypass killswitch；同时在 CLI/project/user 层尝试覆盖，断言 managed 策略始终胜出。

### P1-11 后台 task notification 缺少 non-human provenance

#### 证据链

- [`orchestrator/src/prompt/task_notification.rs:112`](../lingxi-code/orchestrator/src/prompt/task_notification.rs#L112) 的 Agent note 只解释 task notification 可重复触发。
- local bash、monitor 和 generic notification 没有任何说明“未发生用户输入”的 provenance note。

#### 影响

后台结果中的 `approved`、`continue`、`the user asked` 等内容可能被模型误当作真实用户授权，形成跨消息权限混淆。

#### 上游差异

Claude Code 2.1.205 专门修改后台 task notification，要求明确声明没有发生 human input。

#### 必需回归测试

所有 notification 类型都必须包含 non-human provenance；构造结果文本 `user approved this`，断言其不能被权限层或对话 provenance 当成用户授权。

### P1-12 CLI `--bg` 仍是一次性 worker，而非可恢复 live session

#### 证据链

- [`apps/cli/src/background_dispatch.rs:19`](../crates/apps/cli/src/background_dispatch.rs#L19) 明确没有 control socket/live background protocol。
- [`apps/cli/src/commands/bg_worker.rs:31`](../crates/apps/cli/src/commands/bg_worker.rs#L31) 明确没有 PTY、IPC、respawn、watchdog 和 upgrade takeover。
- worker 只执行一次 `run_turn`。
- [`apps/cli/src/commands/daemon.rs:314`](../crates/apps/cli/src/commands/daemon.rs#L314) 将 worker crash 直接标记为永久 failed，不恢复。
- [`apps/cli/src/commands/agents.rs:484`](../crates/apps/cli/src/commands/agents.rs#L484) 的 attach 实际启动第二个 `--resume` CLI 进程，而不是连接原 worker。
- [`apps/cli/src/background_dispatch.rs:200`](../crates/apps/cli/src/background_dispatch.rs#L200) 将 dispatch env 固定为空 map。

#### 影响

- attach/reply/stop/update 不是真正作用于原 worker；
- 原 worker 和第二个 resume 进程可能并发写同一 JSONL；
- PATH、`ANTHROPIC_BASE_URL`、`CLAUDE_CODE_EXTRA_BODY` 不随发起 shell 传递；
- daemon/worker 升级、重启或失去 cwd 后不能恢复原工作。

#### 上游差异

Claude Code 2.1.203–2.1.207 连续修复了后台 daemon token 恢复、attach/reply/stop、dispatch shell env、后台升级和 worktree cold reopen。

#### 必需回归测试

启动长时间 `--bg` 任务，在 Working 状态 attach 并发送新消息；断言仍为同一 worker、turn 串行、JSONL 只写一次。再覆盖 daemon restart、CLI update、cwd 删除和 dispatch env 继承。

### P1-13 TodoStore 的单进程锁假设已经失效

#### 证据链

- [`tools/task/src/todo_store.rs:11`](../crates/tools/task/src/todo_store.rs#L11) 明确将 Claude Code 的跨进程文件锁替换为进程内 mutex。
- [`tools/task/src/todo_store.rs:189`](../crates/tools/task/src/todo_store.rs#L189) 的 high-water mark 是普通 read/write。
- [`tools/task/src/todo_store.rs:282`](../crates/tools/task/src/todo_store.rs#L282) 的 update 是非原子 read-modify-write。
- 当前项目已经存在 daemon、后台 worker、attach/resume 和多个进程访问同一 session/task directory 的路径。

#### 影响

并发进程可能生成重复 task ID、覆盖其他进程更新、读取半写入 JSON 或把 task 文件截断。

#### 必需回归测试

启动多个独立进程并发 create/update/delete 同一个 task list，断言 ID 唯一、JSON 始终可解析、所有更新可串行化且无 lost update。

## 4. P2/P3 Residual Gaps

| 级别 | Gap | 证据与影响 |
|---|---|---|
| P2 | MCP 最新配置未闭环 | `.mcp.json` parser 缺少 `request_timeout_ms` 和 server-level `alwaysLoad`：[`mcp/src/json_config.rs:71`](../lingxi-code/mcp/src/json_config.rs#L71)。Timeout 仍只读全局 env：[`mcp/src/client.rs:903`](../lingxi-code/mcp/src/client.rs#L903)。tools/resources/prompts list-changed 也没有动态刷新。 |
| P2 | `--agent` 实际为 no-op | [`apps/engine-desktop/src/lib.rs:5723`](../crates/apps/engine-desktop/src/lib.rs#L5723) 只 resolve/log，注释明确 main-thread application pending；没有替换主线程 system prompt、tool restrictions、model、hooks 或 MCP。 |
| P2 | `.worktreeinclude` 未实现 | Agent 创建 worktree 固定传 `&[]`：[`tools/agent/src/agent.rs:1594`](../crates/tools/agent/src/agent.rs#L1594)。没有 parser 或 composition wiring。 |
| P2 | Hook 返回值被解析但不应用 | `displayContent`、`suppressOriginalPrompt` 均保留 TODO：[`hooks/src/response.rs:123`](../lingxi-code/hooks/src/response.rs#L123)。MessageDisplay aggregate 被丢弃：[`orchestrator/src/conversation.rs:4264`](../lingxi-code/orchestrator/src/conversation.rs#L4264)。Setup/PostCompact 也因缺少 trigger 而跳过 matcher。 |
| P2 | 内部 meta 消息被持久化为真实 user | [`orchestrator/src/conversation.rs:3105`](../lingxi-code/orchestrator/src/conversation.rs#L3105) 调用 `ConversationMessage::user`，其 `is_meta=false`；正确构造器位于 [`protocol/src/messages.rs:230`](../lingxi-code/protocol/src/messages.rs#L230)。会污染 title、first/last prompt、branch 和 resume。 |
| P2 | Plugin userConfig 不生效 | Sensitive 值只生成 Null：[`plugin/src/loader.rs:27`](../lingxi-code/plugin/src/loader.rs#L27)；解析结果在 [`plugin/src/manager.rs:483`](../lingxi-code/plugin/src/manager.rs#L483) 被丢弃。`pluginConfigs` scopes、secure storage、subprocess env 和 `${user_config.*}` substitution 未闭环。 |
| P2 | Sandbox live conversion 丢字段 | Engine 已解析 `allowPty`/`allowAppleEvents`，但 runner 在 [`sandbox-runtime-runner/src/convert.rs:108`](../lingxi-code/sandbox-runtime-runner/src/convert.rs#L108) 强制转换为 `None`。 |
| P2 | cwd split-brain | Bash 维护独立可变 cwd；file/Glob/Grep/LSP/memory/system-prompt/git-tree 等仍大量读取启动 cwd。`cd` 后不同子系统对“当前目录”的认知不一致。 |
| P2 | LSP 生命周期不完整 | 首次并发请求可重复启动同一 server；`Starting`/`Failed` state 没有真正用于串行化；unregister 删除映射但不 shutdown server process。 |
| P2 | Watcher 生命周期和事件分类 | POSIX watcher 的 blocking receiver 没有取消通道，静默目录下可能泄漏 task/FD；Created 事件被按“当前文件是否存在”归类，实际不可达；hook 动态 `watchPaths` 和 cwd rebinding 未实现。 |
| P2 | Background Agent lifecycle 泄漏 | mailbox route/pump 终止后不 unregister；fast-completing local Agent 存在 handle 完成先于注册竞态；永久删除 `claude rm`/agents Ctrl-X 生命周期缺失。 |
| P2 | Post-compact runtime context 不完整 | Invoked skills 未恢复；即使 read-state 共享被修复，当前文件恢复仍使用旧 snapshot，而不是 compact 时 fresh reread。 |
| P2 | Hosted WebSearch partial result | 已收到 result/progress block 后发生 stream error，所有 partial blocks 被丢弃。 |
| P2 | `skipWebFetchPreflight` settings 未接线 | 目前只支持私有环境变量，settings JSON 中的配置不会进入 WebFetch runtime。 |
| P3 | WebFetch 二进制响应有损 | HTTP transport body 是 `String`，Reqwest 使用 `.text()`；PDF、图片或无效 UTF-8 body 在保存 artifact 前已经损坏。 |
| P3 | Runtime 版本标识落后 | [`platform-api/src/lib.rs:11`](../lingxi-code/platform-api/src/lib.rs#L11) 仍硬编码 `2.1.206`，WebFetch UA 和 child `AI_AGENT` 也使用旧版本。 |
| P3 | Parity 证明基线落后 | 唯一集中 parity harness 仍是 [`test-harness/tests/parity_claude_2_1_198.rs`](../lingxi-code/test-harness/tests/parity_claude_2_1_198.rs)；没有覆盖 2.1.199–2.1.207 的 release matrix。 |

## 5. 已确认关闭或显著改善的旧 Gap

以下旧问题已在本轮确认关闭或显著改善：

- Subagent 未传 `run_in_background` 时已默认后台运行。
- max-turn guard 前会 drain mid-turn input，不再静默丢失消息。
- 完整 background spawn request 与 immediate-parent inheritance 已跨 task boundary。
- In-process persistent Agent 已支持每轮 rest/resume。
- Workflow progress drain 不再 hang。
- API error 已正确映射为 failed/tool error，不再误报成功。
- 生产 JSONL writer 已接入。
- Compaction 热内存路径已保留 summary 后的 suffix/tail。
- WebFetch 已实现手动 redirect policy 和 blocklist preflight。
- Hosted WebSearch 已具备 streaming/progress。
- Malformed glob 的 permission/rules/file-read 主路径已避免 crash。
- File/settings watcher 的上层 async forwarding handle 已由 runtime 持有；剩余问题位于底层 blocking watcher。
- Plugin LSP register/unregister 已有映射清理；剩余问题是 fallback、route、并发初始化与 process shutdown。

## 6. 验证结果

### 核心 crate 单元测试

```text
agent             231 passed
engine-desktop    531 passed
cli               122 passed
orchestrator      534 passed
lsp                 8 passed
mcp               172 passed
tool-meta          57 passed
tool-task         143 passed
```

合计：`1,798 passed, 0 failed`。

### 现有 parity harness

```text
parity_claude_2_1_198: 6 passed, 0 failed
```

本轮总计：`1,804 passed, 0 failed`。

### 测试全绿仍不能证明 parity 的原因

- 多个测试只验证字段被 parse 或搬运，没有验证它最终被 runtime 消费。
- ToolSearch 测试使用手工注入的 registry view，没有覆盖生产空 registry 构造器。
- LSP 测试只覆盖首个文件的 `ensure_server_for_file`，没有覆盖第二文件的生产 client lookup。
- Read-state 测试验证 Arc 可以共享，但 engine composition root 实际没有共享该 Arc。
- Mid-stream fallback 测试锁定 fallback body，却没有断言 partial assistant output 被保存。
- 当前没有 2.1.199–2.1.207 的 changelog-to-regression-test matrix。

## 7. 建议修复顺序

### Wave 1：安全边界与可控性

1. Managed permission rules；
2. Agent `mode`；
3. 异步 Agent worktree isolation；
4. Agent UUID/name 与 TaskStop/TaskOutput alias；
5. Task notification non-human provenance。

### Wave 2：持久化与上下文正确性

1. Compaction JSONL schema、metadata 和 cold resume；
2. Read-file state 单一共享 Arc；
3. 主会话/subagent/WebSearch partial work preservation；
4. TodoStore 跨进程锁和原子写。

### Wave 3：动态工具与协议

1. ToolSearch/deferred tool production wiring；
2. MCP roots、list-changed、request timeout、alwaysLoad；
3. LSP fallback、多文件 route、并发初始化和 shutdown。

### Wave 4：后台 session 架构

1. Live worker control/PTY/messaging protocol；
2. Attach/reply/stop 串行化；
3. Dispatch shell environment；
4. Crash/upgrade/cold reopen recovery；
5. Permanent task deletion和 mailbox lifecycle。

### Wave 5：P2/P3 收尾

Hooks、plugin userConfig、sandbox field conversion、cwd state、watcher、WebFetch/WebSearch 边界和版本标识。

## 8. 1:1 Parity 完成门槛

在满足以下条件前，不建议把项目标记为 Claude Code 2.1.207 1:1 parity：

- 所有 P1 均关闭并有生产 composition-root 回归测试；
- 增加 2.1.199–2.1.207 的逐版本行为矩阵；
- 完成真实 background daemon、cold resume、compact/resume 和 multi-process task stress tests；
- 验证 managed policy 无法被 CLI/user/project 覆盖；
- 验证模型可见 tool schema 与实际 runtime 权限/隔离完全一致；
- runtime 版本标识、WebFetch UA 和 child `AI_AGENT` 更新到目标版本；
- 在干净 worktree 上通过 lint、format、全 workspace tests、静态分析和端到端 parity suite。

## 9. 官方参考资料

- [Claude Code CHANGELOG](https://github.com/anthropics/claude-code/blob/main/CHANGELOG.md)
- [Create custom subagents](https://code.claude.com/docs/en/sub-agents)
- [Run parallel sessions with worktrees](https://code.claude.com/docs/en/worktrees)
- [Connect Claude Code to tools via MCP](https://code.claude.com/docs/en/mcp)
- [Configure permissions](https://code.claude.com/docs/en/permissions)
- [Claude Code settings](https://code.claude.com/docs/en/settings)
- [Hooks reference](https://code.claude.com/docs/en/hooks)
- [How Claude remembers your project](https://code.claude.com/docs/en/memory)
- [Plugins reference](https://code.claude.com/docs/en/plugins-reference)
- [Sandboxing](https://code.claude.com/docs/en/sandboxing)

## 10. 审计备注

- 本轮是只读审计，没有修改产品源码。
- 审计前已存在的 untracked assets/docs 未被修改或删除。
- 由于 Claude Code 为闭源分发物，本报告的“parity”依据为：本机最新 binary 的可观测行为、Anthropic 官方文档/changelog、仓库自身的 Claude parity contract，以及 Rust 生产 composition root 的静态调用链。
