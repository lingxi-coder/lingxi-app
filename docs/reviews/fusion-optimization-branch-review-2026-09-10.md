# 评审：`codex/fusion-optimization` 分支（2026-09-10）

评审对象：`21771b43a..3f9380a8b`，30 个 commit，220 个文件，**+47,726 生产行 / +17,702 测试与评测行**，67 个新文件，179 个新 `pub struct/enum`，21 个新 `trait`。
评审方式：8 路并行只读代码评审 + 三道客观门（钉定工具链 1.82.0，worktree 独立 `target/`）。

## 结论

**不建议整体合入。** 分支实施的是 Codex 自己的原始 PRD，而不是评审后修订的设计；它绕过了"先评测再建设施"，因此所有可调参数都是断言而非实测。代码质量在局部相当高（认证类型无 serde、所有权同步移交、等待器无丢唤醒），但存在 8 条 P0，并且它引入的三套新基础设施（第二账本、证据回执、自建评测）没有一套买到它声称的东西。

建议：**摘取 5 个独立可用的小改动**，其余作废，Fusion 的真实缺口按修订设计重做。

## 一、三道客观门

| 门 | 结果 |
|---|---|
| `cargo build --tests`（1.82.0） | **失败**。`bridge-server/src/server.rs:1115` 使用未导入的 `ErrorKindDto`，来自最后一个 commit `3f9380a8b` |
| `cargo test -p engine-desktop` | **编不过**。`fusion_pool_admission_test.rs` 用 `tokio::time::advance`/`start_paused`，engine-desktop 的 `Cargo.toml` 没开 `test-util`；只有与 `-p fusion` 同批构建时靠 feature 统一才编得过 |
| doctest | **失败**。`lib.rs:5624` 的 `DesktopConfig` 示例缺 `session_writer_lease` 字段 |
| 其余 16 个 crate 测试 | 6325 passed / 0 failed |
| clippy（12 个改动 crate，`--tests`） | 0 error，退出 0（警告基数很大，与基线同量级） |

Codex 的 `2026-09-07-fusion-final-verification.md` 声称 engine-desktop 432 passed。那是用跨包批量 `cargo test --no-run` 得出的，跨包构建把 fusion 的 dev-dependency 特性统一到了 engine-desktop 上，掩盖了缺失的 `test-util`。**单独跑该 crate 就露。**

**30 个 commit 全部带 `Not-tested:` 尾注**，其中三个明确声明连编译都没跑过：

- `3f9380a8b`：*"Not-tested: Compilation, new regressions, and Clippy; targeted build was stopped after a macOS dylib-load stall."*
- `5ea3f8e7a`：*"Not-tested: New regressions and full compilation; targeted orchestrator build stopped with SIGTERM (exit 143) due low disk space."*
- `35ea030c4`：*"...have not been compiled or executed; disk pressure blocked validation."*

这三个 commit 恰好持有本分支风险最高的三处改动：压缩回合门、Bridge 准入互斥锁、MCP 关机 join。

## 二、P0 清单

### P0-1 失败的 wire 尝试按授权上限计费，并进入会话总额

`llm-client/src/service.rs:2787` 在 `transport.execute` **之前**调 `mark_dispatched()`。连接错误 / 429 / 无 usage 的 5xx 走 `attempt.finish()`，此时无观测值，disposition 落 `Unknown`；`cost/src/attempt.rs:1418`：

```rust
nano_usd: if unknown { exact_cost.max(intent.authorized_nano_usd) } else { exact_cost }
```

空 usage 的 `exact_cost` 为 0，于是取授权峰值。该值经 `replace_vector` 直接写入 `CostState.total_nano_usd`——`/cost` 显示、`max_session_nano_usd` 熔断所依据的那个数。重试上限默认 10，一个被限流的 panel 最多把 11 倍峰值预留记成实际花费，且没有修正路径（只有 `Unknown → Exact` 替换，永远拿不到 usage 就永远不触发）。

违反已定方针「provider 优先、自算兜底，缺失时不发明」。

### P0-2 队首保护改变了普通 Agent 的语义

`agent/src/pool/capacity.rs:134-137`：

```rust
let protected = state.queue.front().map_or(0, |(_, count)| *count as usize);
if self.available_permits() <= protected { return Err(AdmissionError::Full); }
```

池 20、跑着 17、空 3，此时 `/fusion` 排入 4 个 panel ⇒ 接下来 3 次用户 Agent 调用全部失败，模型收到 `subagent_concurrency_cap`——一个它并未触及的上限，最长 30 秒。

测试 `capacity/tests.rs:126-150` 把这个行为钉死：容量 4、队首等 3，释放 4 个后普通调用只拿到 1 个，第二次 `assert!(matches!(core.acquire_ordinary(), Err(AdmissionError::Full)))`，**此时仍有 3 个空闲 slot**。

没有采用桌面端已有的独立子池先例（subagent 20 / teammate 4），而是共享池加预留算术。

### P0-3 证据回执在两个主要入口上完全失效

`fusion/src/panel.rs:947` 对所有 origin 都设了 `evidence_context`，但只有 `RegistryToolInvoker` 实现了 `invoke_observed`（`tool-api/src/tool_invoker_impl.rs:228`）。`/fusion` 斜杠路径用 `DeferredToolInvoker`，工作流路径用 `WorkspaceLeaseToolInvoker`，两者都走 trait 默认实现（`platform-api/src/tool_invoker.rs:261-283`）返回 `evidence: None`。**静默，无日志，无测试覆盖这两条路径。**

只有 Agent 入口的 Fusion 会铸回执。约 4,200 行证据系统在最主要的入口上是死的。

### P0-4 一次输出计账失败永久冻结整个编排器

`orchestrator/src/conversation/output_accounting.rs:72` 设置实例级 `Arc<AtomicBool>`，`:199-204` 之后拒绝每一个回合。`grep 'output_accounting_failed.*store(false)'` = **0 命中**，没有任何清除路径。

该 scope 在有持久化会话状态时无条件接线（`engine-desktop/src/lib.rs:11786`、`:14835`），因此影响的不只是 Fusion——普通用户的每一个回合都会失败，直到进程重启。

### P0-5 关机路径可永久挂起（无任何超时）

`engine-desktop/src/lib.rs:7163 shutdown_and_drain` → `:7244 drain_pending_all()` → `fusion_recorder.rs:170 await_in_flight_deliveries` 逐个拿投递锁，无超时。锁持有者最终等 `orchestrator/src/conversation/model.rs:495 turn_gate.lock().await`，代码注释明写 *"Wait for the foreground turn gate instead of timing out"*。

同一条 barrier 上还串着：`cron/src/scheduler.rs:770` 无界 join（持 `tick_handle` 锁）、`bridge/src/mcp_endpoint.rs:421` 无超时 join 所有在途帧处理器（包含 OAuth 浏览器流程和插件安装）、`local_fusion.rs:810 drain_shutdown` 无界等 `completion_rx`。

关键是**这条 barrier 在正常退出路径上**：`apps/bridge-server/src/main.rs` 的 SIGTERM 与每个失败路径都调用它。Electron 侧宽限期到点会 SIGKILL，于是慢一步就变成结算中途硬杀——正是这条 barrier 存在的目的所要防止的结果。

### P0-6 结算失败丢弃已付费的响应

`orchestrator/src/turn_loop.rs`（约 :886）在把 `LlmResponse.content` 翻译进历史**之前**返回错误：

```rust
if let Err(error) = settlement.persistence_result() {
    return Err(OrchestratorError::Internal(format!("cost settlement failed after provider response: {error}")));
}
```

流式孪生（`drivers/mod.rs finalize_iteration`）在文本已渲染给用户之后做同样的事。基线是忽略 `record_api_response_v2` 的返回值。而 freeze 有 8 个触发点（含一次磁盘写失败），于是一次瞬时 IO 错误 = 永久回合失败 + 丢输出。

### P0-7 账本损坏使会话永久无法恢复

`hydrate_from_journal` 对内部损坏、任意 revision 缺口、有 snapshot 无 WAL 都返回 `Err`（`journal.rs:598`、`:292`），映射为 `BuildError::DurableSession` → `InitError::DurableSession`。一个字节坏在**派生的记账账本**里，该会话就再也 `--resume`/`--continue` 不了。transcript 加载器是容错的，这个不是；没有隔离或改名路径（`grep quarantine` = 0）。

### P0-8 kill 现在往 transcript 写错误行

`local_fusion.rs:794 request_cancel()` → `record_terminal_candidate`（`platform-api/src/fusion.rs:1425`，无条件）→ `fusion_recorder.rs:801` 对任意结果都构造 outbox，`:372` 发出 `<fusion-error>…</fusion-error>` 并推送 "Fusion run failed" 通知。`PreparedFusionRun` 的 `Drop`（`fusion.rs:1453-1483`）同样处理——registry 丢弃的未激活句柄，即用户从未见过的任务，也会留下错误行。

原契约：Failed/Killed 从不发布。`local_fusion.rs:788-793` 的注释声称 kill "suppresses publication"，实际只跳过了 legacy `finalize`。

## 三、违反「不做兼容」

用户 2026-09-03 定、2026-09-06 重申：兼容代码一律移除、设备数据可丢弃、不写迁移，唯一例外是上游协议面。分支新增行中 `legacy` 出现 **386 次**。

| 项 | 位置 | 性质 |
|---|---|---|
| `lastCost` 开账余额导入 | `engine-desktop/src/lib.rs:10939-10958, 11413-11461` | **为未发布版本写的迁移** |
| 每个全新会话账本首行是 `LegacyImportEvaluated` 标记 | `session_state.rs:1364-1454` | 从未发布的磁盘格式里的永久兼容痕迹 |
| `decode_legacy_flat_cost` | `session_state.rs:2115-2131` + 3 处回退分支 | 解析一种**历史上不存在**的行形状 |
| CLI `lastCost` 存取并行保留 | `apps/cli/src/session_cost.rs`、`mode.rs:1851`、`run.rs:4023` | 双路径 |
| `result_published: bool` 与 `publication_status` 枚举并存，且 bool 覆盖枚举 | `tasks/src/state.rs:526`；`run.rs:3179` | `state.rs:686` 还加测试钉死 |
| `ModelAttemptBillingMode::{LegacyAggregate, MeteredAttempts}` 运行时双计费路径 | `platform-api/src/model_attempt.rs:18`；选择在 `fusion/src/orchestrator.rs:2462` | 移动端与非持久化桌面走 Legacy；6 个 `attempt_run.is_none()` 分支 |
| `FusionRequest.conversation_id: Option<String>` 与 typed identity 并存 | `platform-api/src/fusion.rs:187` | 三个入口仍事后覆写；会话 id 表示法仍是 5 种 |
| `for_legacy_request` / `into_legacy_result` / `FusionExecutor::run` shim | `fusion.rs:348, 1161` | 测试名字就叫 `production_run_compatibility_shim_…` |
| `retry_cycle_end` / receipt `status` 的 `#[serde(default)]` + "legacy 记录"测试 | `fusion.rs:1988, 2942` | 为不存在的旧记录做兼容 |
| `PanelEvidence.receipt_ref: Option`，"absent legacy evidence remains unverified" | `fusion.rs:1511` | 显式兼容路径 |
| `record_legacy` / `LegacyFusion` 输出事件 | `cost/src/budget/output.rs:16`；`platform-api/src/workflow_output.rs:22` | 生产在用 |

## 四、三套新基础设施的收益核算

### 第二账本（约 13,000 行，46 个新 pub 类型）

Codex 选择在 `<lingxi_home>/session-state/<uuid>/{ledger.v1.jsonl,snapshot.v1.json}` 建独立 WAL + 锁 + 快照，而不是往现成 transcript 写 side record（`writer.rs:1720 append_side_record` 已存在，未知行类型读时忽略，`cost-state` 是零生产者的预留 `LastWins` 行类型）。

- **snapshot 文件在生产中只作存在性探针**：唯一读者 `hydrate_from_journal:2031-2047` 用它判断"有快照却没有 WAL"从而拒绝启动；`decode_projection_snapshot` 是 `#[cfg(test)]`。写它每次要两次 fsync。
- **真实新增能力只有两条**：任意 resume 会话的按模型明细跨重启恢复；Fusion outbox 跨重启重投。
- **代价**：每条普通 transcript 行现在要一次跨进程 flock + 两次 `sync_all`（macOS 上是 `F_FULLFSYNC`，单次 10–100 ms），基线是 flush-only；一个用户回合至少 4 次 fsync，成本行再加 3 次。WAL 无压缩，每次 mutation 追加完整 `CostStateVector`（含全部 per-model usage）；`CoordinatorState.results`、`JournalIndex.locations`、`SessionEntry.response_settlements` 同步无界增长。
- 启动一个既有会话要**扫 4 遍 WAL、重写 3 次快照**（`lib.rs:11455-11468`）。

### 证据回执（约 4,200 行，15 个新 pub 类型，3 个新 trait 方法）

- 唯一决策级效果：合成器输出里出现未知的 `[evidence:evr_…]` 标记时返回 NeedsParent——而这个标记语法本身就是本 PR 引进的。
- `CitationValidation::ValidReferences` 在 `citations.rs` 之外零引用；`FusionResult` 上没有任何字段记录"已验证"。
- 它阻止的原有失败模式：**没有**。panel 幻觉出未调用工具的证据 ⇒ 没有 `host_evidence` 行 ⇒ 合成器无可引用 ⇒ 照样 Merge。
- 两条静默降级：真实 `Read` 的 locator 是绝对规范路径，模型按相对路径引用就静默拿不到回执（e2e fixture 用相对路径所以测不出）；自定义 spawner 下 `producer_drain` 为 `None` 时回执直接消失。
- 一条反向效果：带回执的 panel 在 packing 放不下时**整段丢掉** claims 摘录（`packing.rs:486-490`），而非按前缀截断。
- 加上 P0-3，它在 `/fusion` 和工作流两个入口上根本没运行。

### 自建评测（约 4,300 行）

- **不含任何质量判据**：没有 grader、没有 judge、没有期望输出。`dry_run()` 硬编码 `semantic_ratings: "unrated"`。
- **答案泄漏进提示词**：`fusion_evaluation.rs:256` 把 fixture 的完整 `expected_facts` id 列表放进 live prompt，`harness.rs:598-621` 再拿模型自报的 `fact_ids` 去对同一份列表打分。抄列表就是满分。
- **语料是占位符**：`fixtures.rs:1-6` 自己写着 *"synthetic and local by construction … not a benchmark corpus"*；24 条全部是一句话，rubric 24 条共用同一个四词字符串且没有评分路径。
- **零复用现成 harness**：对 `apps/cli/src/commands/plugin_eval.rs`（5,069 行，已有 `case.yaml` + `graders/*.md`、`--ablation`、`--judge-model`、`--runs`、`--max-cost-usd`、`--report html`）的 diff 为空。新 harness 没有 `--model`、`--judge-model`、`--runs`、report。
- **Pick/Merge 对比无效**：`strategy.rs:44-66` 无条件覆盖分析师的自然建议（含自然 NeedsParent），测的是两个合成策略，不是生产行为。
- **从未跑过真实调用**。因此模型排序、阈值、quorum 10 秒、并发 2、所有默认值都是断言。
- 副作用：整个评测模块（含 fake spawner 和语料）是生产 `fusion` crate 的无条件 `pub` API（`fusion/src/lib.rs:14-15`，无 feature gate），编译进 engine-desktop / engine-mobile / bridge-server；`engine-desktop` 里另有约 1,030 行非 `cfg(test)` 的评测代码，在 `build()` 组合根上加了四个 eval 专用分支。

## 五、越界与副作用（与 Fusion 无关的行为改变）

| 项 | 位置 | 影响 |
|---|---|---|
| 删除 SendMessage 的 UDS 直发 | `tools/ui/src/send_message.rs:482`；`coordinator/src/tool_send_message.rs:294` | 跨会话消息只剩轮询文件收件箱；空闲对端**永远看不到**待批准对话框，投递回执推迟到对端下一回合。`send_peer_message` 零生产调用者，而注释仍写着 "UDS is preferred" |
| 普通 `agent()` 批的计账口径改变 | `local_workflow.rs:2431-2455` | 改为按 `cumulative_usage` 全轮次 + reasoning token 收费，且对 Failed 收费；桌面端工作流比移动端更早撞 `token_budget` |
| 回合中 `/compact` 从静默允许变成报错 | `bridge-server/src/server.rs:1112` | wire 可见的行为变化。**移动端没有对应的守卫**（`engine-mobile/src/host.rs:8916`），于是移动端的 `/compact` 会停在 `turn_gate` 上等整个回合 |
| `file_changed_watch::restart()` 改为等待排空 | `file_changed_watch.rs:586` | 每次 `cd`/`updateWatchPaths` 都阻塞到旧 FSEvents 流析构 |
| inbox 迁移失败后进程没有 inbox | `uds_inbox.rs:480` | 在普通 `/clear`/`/resume` 路径上 |
| `platform-api/src/task_registry.rs` | 纯 `rustfmt` 重排（+11 行，全是测试桩签名） | 违反"never `cargo fmt`"，只贡献 rebase 摩擦 |
| Fusion 概念漏进通用 Task 抽象 | `tasks/src/task_trait.rs:269+` | `TaskHandle` 带 `fusion_timeout_ms`、`fusion_prepared_summary`、`fusion_activation`；其中 `with_fusion_timeout_ms` 已无生产者 |
| `platform-api/Cargo.toml` | tokio `["sync"]` → `["rt","sync","time"]` | 删掉了 "No runtime pulled into this low-level crate" 这条被声明的设计约束，而不是遵守它 |
| 两个 commit 信息与代码相反 | `35ea030c4` 标题说 "Interrupt … in-flight frame handlers"，代码是 join；`e86e79276` 声称 drain 而 Cargo.toml 删了约束 | 唯一没有自动判据的产出物 |

## 六、集成可行性

- 基线 `21771b43a` 已在 `main` 中（无需再 fast-forward `codex/fusion-audit-fixes`）。
- main 自基线前进 **139 个 commit（+104,268 行）**，且做了 2.1.263/2.1.267 的 parity 重写，**恰好压在分支改的同一批 seam 上**：

| 文件 | main 侧 | 分支侧 |
|---|---|---|
| `tasks/src/registry.rs` | +3865 / -280 | +263 / -31 |
| `agent/src/handle.rs` | +835 / -313 | +338 / -21 |
| `tools/agent/src/agent.rs` | +658 / -75 | +267 / -92 |
| `apps/engine-desktop/src/lib.rs` | +875 / -282 | +1323 / -98 |
| `orchestrator/src/turn_loop.rs` | +363 / -65 | +146 / -29 |

- `git merge-tree` 报 **38 个冲突文件**；其中 `coordinator/src/tool_team_create.rs`、`tool_team_delete.rs` 已被 main 删除（合并进 `implicit_team.rs` / `tool_send_message.rs`）。
- 结构性冲突：`SubagentSpawnRequest` 字面量在 main 上从 37 处涨到 54 处，`SubagentInvocationContext` 从 15 到 20——分支给这两个结构各加了字段，全部需要补齐（编译错误，机械但面广）。
- `turn_loop.rs` 是上游 parity 区，分支往里插了 8 个新点，且流式孪生用了结构不同的做法（`account_stream` 包装 vs 内联 observe），未来从 oracle 重新移植 PTL 恢复逻辑会逐行冲突。

## 七、建议

**作废整条分支的合入路径**，改为摘取以下独立可用的部分：

1. **`937673687`（build metadata，4 文件 +200 行）** — 与 Fusion 完全无关，修的是"硬编码不存在的 `.git` 相对路径导致每次重编"的真实 bug，用 `git rev-parse --git-path` 正确处理 linked worktree/detached HEAD/packed refs。在四个路径上与 main 无冲突，可直接 cherry-pick 成独立 commit。
2. **`9a4eb8b21` 中的 WebFetch 确定性改造** — 跳过 side query、64 KiB UTF-8 安全截断、缓存命中与未命中同一判断。策略经 `ToolUseContext.tool_execution_policy` 由宿主设置，未碰 `BuiltinToolContext`（符合设计文档 §0），模型不可达。需剥离同 commit 里的证据钩子。
3. **`92103b7cb` + `bab64bfc1`（TUI，10 文件 +553 行）** — `TaskRow` 补 `stage`/`error`，`/tasks` 借用现成的 250 ms footer poller 刷新且按 id 保选中；普通任务行渲染不变，有回归测试。
4. **`090f7806f`（`cap_input_bytes` 线性化，2 文件 +341 行）** — 纯测量优化，非行为改变，带与旧实现的对照测试。
5. **`settings_watch.rs` 的泄漏修复** — 修的是既有真 bug（构造 future 被取消时 `Vec<JoinHandle>` 被丢弃，spawn 出去的循环持着编排器的 `Arc` 泄漏），有测试钉住。但它埋在 51 文件的 `e86e79276` 里，需要单独抽。

**其余部分重做**，按修订设计（账本落 transcript side record、Fusion 专属子池 + `acquire_many_owned` 整组准入、计量钩子放 runner 与 SideQueryClient 装饰器、先用 `lingxi plugin eval` 跑真实评测）。

**分支上真正值得保留的洞见**（不必保留其实现）：

- 发布语义的正确形态已经被验证：`Published` 只在事务性 append + fsync 完成**且**持久化回执写成功之后置位；message uuid 从 run_id 派生所以重试幂等；终态是单条 mutation；registry 等 fsync 之后才翻 `Completed`。
- panic 监督的正确位置是 supervisor 里按 poll 包 `catch_unwind`，不是改 `PlatformRuntime`。
- 架构不变量守住了：`fusion/src` 非测试的模型调用点仍精确是两个（analyst 的 `query_json_schema`、synth 的 `query`），没有新增隐含调用。
- 容量确实按 `(profile, model)` 从目录条目的 metadata 取，未知时对自动选型 fail closed、对显式模型报配置错误——没有踩 `model_limits.rs` 跨 provider 取 MAX 的坑。
- 匿名 id 是 run_id 播种的置换、在收集之后施加，因此打包顺序与完成顺序无关；分析师提示词里确实不含 profile/model id（有捕获提示词文本的测试）。
- 双池先例、tokio FIFO 整组准入、`OwnedSemaphorePermit` 转移的所有权检查（`PanelPoolPermit` + `owns()` 指针比较）都是对的方向。

## 附：质量上确实好的地方

- 认证类型全部 `#[serde(skip)]` 且 `Debug` 脱敏，配有伪造 JSON 的往返测试证明持久化输入无法伪造 context。
- 所有权移交在任何 await 之前同步完成（`HostLease::transfer`、`CostBudgetAttempt::transfer`），`Drop` 仍会结算，被丢弃的等待者无法退掉已知花费。
- 手写等待器无丢唤醒窗口、无 drop 泄漏（`subscribe()` 先于入队，`Waiter::drop` 摘除并通知），不用 tokio 公平信号量的理由站得住（否则普通 `try_acquire` 会被彻底饿死）。
- `journal.rs` 的尾部修复很仔细：残行截断、完整无换行尾行补换行、内部损坏 fail closed、确认前对文件与目录都 fsync；追加后的指纹校验能抓到中途被替换的 leaf。
- 会话切换改成 prepare→activate 两阶段提交，把所有可失败/需 await 的水合放在同步临界区之前，被取消的调用者不再能留下"成本指向 B 而对话还在 A"。
- `mcp_endpoint.rs` 的 `JoinSet` 改造修了真 bug：基线在宿主退出时孤立所有连接并**完全跳过** `on_close_with_sink`，权限门的排空从未运行过。
