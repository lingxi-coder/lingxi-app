# Dual-LLM Multi-Agent Execution Design

## 目标

在 LingXi Code 现有 agent 系统内新增一个可配置的执行策略，让高风险或用户显式指定的任务可以由两个不同 LLM 后端独立实现、交叉 review、各自修正，最后由仲裁器选择最优方案或合成最终补丁。

本方案不是新增一个独立产品，也不是让多个 agent 同时修改主工作区。它作为当前 Rust workspace 的一个 orchestration strategy 接入：

```text
用户请求
  -> execution router
  -> single-agent path
     或 dual-llm multi-agent path
  -> finalizer 单点应用最终补丁
  -> verification
```

## 当前项目落点

仓库主体在 `lingxi-code/`，已有能力边界如下：

| 现有模块 | 当前职责 | 本方案用法 |
|---|---|---|
| `engine/src/settings` | 4 层 settings 加载、merge、typed schema | 新增 `multiAgent` LingXi-only 配置字段 |
| `provider-config` | 从 settings.providers / routing 组装 `llm_client::ClientConfig` | 复用 provider profile / routing，不重新造 provider 配置 |
| `llm-client` | provider-neutral LLM client，支持 Anthropic/OpenAI/Gemini/OpenAI-compatible 等 | 给用户配置的候选 agent 和 arbiter 发起模型调用 |
| `orchestrator` | 主会话 turn loop、tool dispatch、cost/hooks/memory wiring | 增加执行策略路由入口，决定是否进入 dual-LLM path |
| `agent` | subagent multi-turn loop，`StateMachinePool` | 用于模型内 agent loop；候选实现可以复用 subagent loop 能力 |
| `tasks` / `coordinator` | background work、team/multi-agent、task registry | 记录候选实现、review、revision、verification 阶段状态 |
| `sidequery` | side LLM 和单轮 forked-agent helper | 适合 reviewer/arbiter 的结构化单轮判断，不适合执行型代码修改 |
| `traits::worktree::WorktreeManager` | disposable git worktree 抽象 | 创建 candidate A / candidate B 隔离候选工作区 |
| `tool-worktree` | 用户可调用 EnterWorktree/ExitWorktree 工具 | 不直接暴露给候选流程；host/orchestrator 直接调用 trait |
| `telemetry` / `cost` | 事件与成本统计 | 记录 multi-agent 阶段、provider、token、耗时、winner |

## 架构选择

采用“合入当前 agent 系统，作为独立 execution strategy”的方式。

不建议做独立服务的原因：

- 现有 provider、settings、credential、cost、permission、telemetry 已经在 `lingxi-code` 内成体系。
- 候选实现必须访问同一份 repo、同一套 tool registry、同一套 permission/sandbox 策略。
- 独立服务会重复认证、日志、worktree 管理和配置解析。

不建议把规则散落到每个 agent prompt 的原因：

- prompt 不能保证文件隔离、只读 review、最终单点 apply 等安全约束。
- 多 agent 的生命周期、状态、成本、失败回退需要 host 级别控制。

推荐新增一个 crate：

```text
lingxi-code/multi-agent/
  Cargo.toml
  src/
    lib.rs
    config.rs
    router.rs
    orchestrator.rs
    worktrees.rs
    providers.rs
    prompts.rs
    review.rs
    revision.rs
    arbiter.rs
    finalizer.rs
    verification.rs
    state.rs
```

如果想先做最小改动，也可以先在 desktop composition root 内实现 experimental module，稳定后再抽成 `multi-agent` crate。但最终建议独立 crate，避免污染 `orchestrator` turn loop。

## 配置设计

新增 LingXi-only settings 字段 `multiAgent`。它应和现有 `providers`、`routing` 一样作为扩展字段存在，不影响 claude-code parity 字段。

建议 wire 名称：

```json
{
  "multiAgent": {
    "enabled": false,
    "mode": "auto",
    "strategy": "dualLlmCompetitive",
    "candidates": [
      {
        "id": "candidate-a",
        "label": "User configured candidate A",
        "model": "profile-a/model-a",
        "role": "implementer"
      },
      {
        "id": "candidate-b",
        "label": "User configured candidate B",
        "model": "profile-b/model-b",
        "role": "implementer"
      }
    ],
    "reviewers": {
      "crossReview": true,
      "authorsFixOwnBranch": true,
      "maxReviewRounds": 1
    },
    "arbiter": {
      "model": "profile-arbiter/model-arbiter",
      "allowHybrid": true
    },
    "triggers": {
      "keywords": ["高准确率", "最优解", "双模型", "互相review", "架构", "安全"],
      "minComplexity": "medium",
      "security": true,
      "architecture": true,
      "largeDiff": true
    },
    "limits": {
      "maxIterations": 2,
      "timeoutSeconds": 1800,
      "phaseTimeoutSeconds": {
        "implementation": 900,
        "review": 300,
        "revision": 600,
        "arbitration": 300,
        "verification": 900
      },
      "maxChangedFiles": 50,
      "cleanupWorktrees": "onSuccess"
    }
  }
}
```

Rust schema 建议：

```rust
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SettingsJson {
    // existing fields...
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub multi_agent: Option<serde_json::Value>,
}
```

第一阶段可以先用 `Value` 承载，避免一次性引入过多 typed schema。第二阶段再在 `multi-agent/src/config.rs` 中解析成强类型：

```rust
pub enum MultiAgentMode {
    Off,
    Auto,
    Force,
}

pub struct MultiAgentConfig {
    pub enabled: bool,
    pub mode: MultiAgentMode,
    pub strategy: MultiAgentStrategyKind,
    pub candidates: Vec<AgentEndpoint>,
    pub reviewers: ReviewConfig,
    pub arbiter: ArbiterConfig,
    pub triggers: TriggerConfig,
    pub limits: LimitConfig,
}
```

Settings merge：

- `merge()`（`engine/src/settings/merger.rs`）是**逐字段手写**的：新字段只加进 `SettingsJson` struct 而不在 `merge()` 里加一行，默认行为是 “`next` 整体覆盖 `prev`”——project 层一个 `multiAgent` 会把 user 层整块吃掉。
- 因此 `multi_agent`（`Option<Value>`，与 `routing` 同型）**必须在 `merge()` 里显式走 `deep_merge_value_opt`**，与 `routing` 一致：

```rust
// merger.rs merge() 内新增
multi_agent: deep_merge_value_opt(prev.multi_agent, next.multi_agent),
```

- user/project/env 覆盖顺序沿用现有 `Settings::load`。
- 不要写入 Claude Code settings 支持列表，除非要让 `Config` 工具支持读写它。

候选 agent 必须完全来自用户配置。`candidate-a` / `candidate-b` 只是稳定 ID 示例，不代表固定绑定 OpenAI 或 Anthropic。合法配置可以是：

```json
{
  "multiAgent": {
    "candidates": [
      { "id": "fast", "model": "profile-fast/model-fast" },
      { "id": "deep", "model": "profile-deep/model-deep" }
    ],
    "arbiter": { "model": "profile-arbiter/model-arbiter" }
  }
}
```

约束：

- MVP 要求恰好 2 个 `candidates`。
- 后续可以扩展到 N 个候选，但 arbitration / artifact layout 需要先泛化。
- `id` 必须是文件名安全的稳定标识，不得从 provider 名自动推导。
- `model` 走现有 `provider-config` / `llm-client` 解析；未配置或不可用时 fail fast。

环境变量建议：

```text
LINGXI_MULTI_AGENT=off|auto|force
LINGXI_MULTI_AGENT_ENABLED=true|false
LINGXI_MULTI_AGENT_STRATEGY=dualLlmCompetitive
LINGXI_MULTI_AGENT_TIMEOUT_SECONDS=1800
LINGXI_MULTI_AGENT_REVIEW_ROUNDS=1
```

CLI 覆盖建议：

```text
lingxi run --multi-agent "..."
lingxi run --dual-llm "..."
lingxi run --no-multi-agent "..."
```

优先级：

```text
显式 --no-multi-agent
> 显式 --multi-agent / --dual-llm
> env
> settings.multiAgent
> auto trigger
> 默认 single-agent
```

## 路由策略

新增 `multi-agent/src/router.rs`：

```rust
pub enum ExecutionRoute {
    SingleAgent,
    DualLlmCompetitive,
}

pub struct RouteInput<'a> {
    pub user_prompt: &'a str,
    pub explicit_flag: ExplicitMultiAgentFlag,
    pub config: &'a MultiAgentConfig,
    pub estimated_complexity: Complexity,
    pub touched_area_hint: TaskAreaHint,
}
```

路由逻辑：

```text
1. 用户显式关闭 -> SingleAgent
2. 用户显式开启 -> DualLlmCompetitive
3. config.enabled=false 或 mode=off -> SingleAgent
4. mode=force -> DualLlmCompetitive
5. mode=auto:
   前置门槛（必须先满足，否则纯讨论类 prompt 会误触发昂贵路径）：
   - 本轮确实计划写文件（有 edit/write 意图，而非纯问答 / 解释 / review）。
   满足前置门槛后，再命中以下任一升级条件：
   - 命中 trigger keyword
   - 安全/权限/认证/支付/数据删除相关
   - 架构或 shared abstraction
   - 预计跨多个 crate 或大 diff
   - 上一次 single-agent verification 失败
   -> DualLlmCompetitive
6. 否则 SingleAgent
```

`estimated_complexity` 第一阶段可以用启发式，不需要模型判断：

- prompt 包含 “架构/重构/security/auth/permission/最优/准确率/compare”
- 计划修改超过 N 个文件
- 目标路径涉及 `permission`、`secret`、`llm-client`、`provider-config`、`orchestrator`、`tasks`、`coordinator`

## 执行状态机

新增 `multi-agent/src/state.rs`：

```rust
pub enum DualLlmPhase {
    BuildTaskBrief,
    CreateWorktrees,
    ImplementCandidates,
    CollectSelfReports,
    CrossReview,
    AuthorRevision,
    Arbitration,
    ApplyFinalPatch,
    Verification,
    Cleanup,
    Complete,
    Failed,
}
```

每次 run 保存状态到：

```text
.lingxi/multi-agent/runs/<run_id>/
  task-brief.md
  state.json
  candidate-a/
    self-report.md
    patch.diff
    verification.log
  candidate-b/
    self-report.md
    patch.diff
    verification.log
  reviews/
    candidate-a-on-candidate-b-round-1.md
    candidate-b-on-candidate-a-round-1.md
  arbitration.md
  final.patch
  final-verification.log
```

也可以放到 `.claude/` 下，但推荐 `.lingxi/`，因为这是 LingXi-only orchestration 记录，不是 claude-code parity state。

## Worktree 隔离

复用 `traits::worktree::WorktreeManager`，不要让两个候选 agent 在主工作区写代码。

⚠️ **不要复用 `agent::worktree_policy::create_worktree_or_degrade`。** 该 helper 在无法创建 worktree 时**降级返回 `None`（= 不用 worktree，在当前 cwd 就地跑）**。对单 agent 这是合理回退；对 dual-LLM 则等于“两个候选同时写主工作区”，正好踩穿隔离保证。multi-agent 必须直接调用 `WorktreeManager::create_worktree`，把 `None` / 失败一律当 **fatal**（`MultiAgentError::Worktree`），绝不就地跑。

**两个 worktree 串行创建，候选并行执行。** 同一 repo 下并发 `git worktree add` 会争主 repo 的 `.git` 锁；`CreateWorktrees` 阶段应顺序创建 candidate-a、candidate-b，创建完成后再并行启动两个候选（每个 worktree 有独立 index，并行写文件是安全的）。

Worktree slug：

```text
multi-agent/<run_id>/<candidate_id>
```

现有 `tool-worktree` 的 slug 限制是总长 64 且 `/` 会 flatten 成 `+`，因此 run id 应该短，例如 ULID 前 12 位：

```text
multi-agent/01HX9ABCD12/candidate-a
multi-agent/01HX9ABCD12/candidate-b
```

对应路径：

```text
<repo>/.claude/worktrees/multi-agent+01HX9ABCD12+candidate-a
<repo>/.claude/worktrees/multi-agent+01HX9ABCD12+candidate-b
```

安全规则：

- candidate agent 只能写自己的 worktree。
- reviewer 阶段只读对方 diff 和 self-report，不允许写文件。
- author revision 只能改自己的 worktree。
- finalizer 是唯一可以修改主工作区的阶段。
- cleanup 前必须检查 worktree dirty summary；失败时保留 worktree 并报告路径。

## Provider 接入

不要为任何具体厂商、产品或 provider 写硬编码调用。候选 agent 和 arbiter 都通过 `llm-client` 的 model routing 走现有 provider 配置。

模型引用使用当前项目已有形式：

```text
profile-a/model-a
profile-b/model-b
profile-arbiter/model-arbiter
```

`provider-config` 已经把 `settings.providers` 和 `settings.routing` 汇总为 `llm_client::ClientConfig`，multi-agent 只需要：

- 从 config 读取 `model` 字符串。
- 调用现有 resolver/route 构造 API client。
- 把 usage 交给 `cost` 和 `telemetry`。

## Agent 运行方式

候选实现需要 tool loop，因此不能只用 `sidequery::SideQueryClient`。

推荐分层：

| 阶段 | 运行方式 |
|---|---|
| task brief build | 主 orchestrator 内部生成，或 `SideQueryClient` 单轮 |
| independent implementation | 复用 `agent` subagent multi-turn loop 或现有 orchestrator runner，工作目录指向候选 worktree |
| self-report | candidate agent 最后一轮输出，或 host 从 git diff + logs 生成 |
| cross-review | `SideQueryClient` 单轮即可，输入为 task brief + 对方 diff + self-report |
| author revision | 原 candidate agent 在自己的 worktree 内再跑一轮 |
| arbitration | `SideQueryClient` 单轮结构化输出 |
| finalizer | host 代码，不是 LLM 直接改主工作区；需要合成时可调用一个 executor agent，但写入点仍是 finalizer 控制 |

如果第一阶段不想碰 `agent::StateMachinePool`，可以用外部进程适配：

```text
candidate adapter: 按用户配置的 model/provider 调用内部 provider 或外部 CLI wrapper
```

但长期应收敛到内部 `llm-client + agent loop`，否则权限、日志、成本和取消语义会分裂。

## Cross-review 规则

默认采用“review document only，然后作者修自己的分支”。

```text
1. candidate-a 实现 A
2. candidate-b 实现 B
3. candidate-a review B，只写 reviews/candidate-a-on-candidate-b-round-1.md
4. candidate-b review A，只写 reviews/candidate-b-on-candidate-a-round-1.md
5. candidate-a 根据对 A 的 review 修 A
6. candidate-b 根据对 B 的 review 修 B
7. Arbiter 选 A/B/hybrid/reject both
```

review 轮数必须受配置限制：

```text
review_round = 0
while review_round < maxReviewRounds:
  cross-review both candidates
  each author revises its own branch
  if both reviews contain no blocking findings:
    break
  review_round += 1
```

停止条件：

- 达到 `reviewers.maxReviewRounds`。
- 两边 review 都没有 blocking findings。
- 任一候选超时、失败或超过 changed-files 限制，直接进入 arbitration。
- `maxReviewRounds=0` 表示跳过 cross-review，直接仲裁初版候选。

review 轮数只有 `reviewers.maxReviewRounds` 一个旋钮（单一真相源）。不要在 `limits` 里再放一个同名硬上限——一个语义两处配置会让用户改了一个、另一个没动而困惑。`limits` 只承载 timeout、`maxChangedFiles`、`maxIterations` 这类硬约束。

禁止：

- A 直接改 B 的 worktree。
- B 直接改 A 的 worktree。
- review 阶段运行 destructive command。
- 两个候选实现同时写主工作区。

允许：

- reviewer 建议“移植对方测试”。
- arbiter 选择 hybrid。
- finalizer 在主工作区应用 hybrid patch。

## 超时与错误处理

multi-agent workflow 必须把错误作为一等状态处理，不能只依赖 LLM 文本自报。

### Timeout model

配置分两层：

```text
limits.timeoutSeconds                整个 multi-agent run 的墙钟上限
limits.phaseTimeoutSeconds.*         单阶段上限
```

阶段超时默认值：

| 阶段 | 默认超时 | 超时后动作 |
|---|---:|---|
| `BuildTaskBrief` | 120s | fallback 到 host 生成的简短 task brief |
| `CreateWorktrees` | 120s | fail run；不允许降级到主工作区并行写入 |
| `ImplementCandidates` | 900s | 标记超时 candidate 为 failed；若至少一个 candidate 成功则继续 |
| `CrossReview` | 300s | 标记对应 review 缺失；继续进入 revision/arbitration |
| `AuthorRevision` | 600s | 保留该 candidate 的 revision 前版本；继续 arbitration |
| `Arbitration` | 300s | 启用 deterministic fallback arbitration |
| `ApplyFinalPatch` | 300s | fail run；不强行 partial apply |
| `Verification` | 900s | 标记 verification inconclusive；不得 claim complete |
| `Cleanup` | 120s | best-effort；失败时保留 worktree 并报告路径 |

实现建议：

```rust
pub struct TimeoutPolicy {
    pub run_timeout: Duration,
    pub phase_timeout: BTreeMap<DualLlmPhase, Duration>,
}
```

每个 phase 用 runtime 的 cancellation token 包裹。run-level timeout 触发时：

1. cancel 所有 candidate/reviewer/reviser 子任务。
2. flush 已有 logs / patch / review artifacts。
3. 检查 worktree dirty summary。
4. 返回 `MultiAgentError::RunTimedOut { run_id, phase }`。
5. 不清理 dirty 或 unknown-state worktree。

### Error taxonomy

新增错误类型：

```rust
pub enum MultiAgentError {
    Config(ConfigError),
    ProviderUnavailable { candidate_id: String, model: String, reason: String },
    Worktree(WorktreeError),
    CandidateFailed { candidate_id: String, phase: DualLlmPhase, reason: String },
    CandidateTimedOut { candidate_id: String, phase: DualLlmPhase },
    ReviewFailed { reviewer_id: String, target_id: String, round: u32, reason: String },
    ReviewTimedOut { reviewer_id: String, target_id: String, round: u32 },
    ArbitrationFailed { reason: String },
    FinalizerFailed { reason: String },
    VerificationFailed { command: String, exit_code: Option<i32>, log_path: PathBuf },
    VerificationTimedOut { command: String, log_path: PathBuf },
    ArtifactIo { path: PathBuf, reason: String },
    Cancelled,
    RunTimedOut { run_id: String, phase: DualLlmPhase },
}
```

错误分类：

| 分类 | 例子 | 是否继续 |
|---|---|---|
| Recoverable candidate error | 一个 candidate API 失败、实现超时、测试失败 | 若另一个 candidate 成功，继续 arbitration |
| Recoverable review error | 一份 review 超时或格式坏 | 记录缺失，继续 |
| Recoverable revision error | 作者修正失败 | 使用 revision 前 patch 继续 |
| Arbiter recoverable error | arbiter JSON 解析失败 | 重试一次；仍失败则 deterministic fallback |
| Fatal config error | candidates 少于 2、model 不可解析 | fail fast |
| Fatal worktree error | 无法创建隔离 worktree | fail run |
| Fatal finalizer error | patch 无法干净应用主工作区 | fail run |
| Fatal verification error | 验证失败 | 不 claim complete；可进入 fix iteration |
| User cancellation | 用户停止 | cancel children，保留 artifacts |

### Candidate failure handling

候选实现阶段的规则：

```text
if both candidates fail:
  fail run, keep artifacts

if one candidate fails and one succeeds:
  skip failed candidate's revision
  ask arbiter to compare "successful candidate" vs "no viable competitor"
  finalizer may accept the successful candidate only after verification

if candidate produces no diff:
  mark CandidateFailed(reason="empty diff")

if candidate exceeds maxChangedFiles:
  mark CandidateFailed(reason="diff too large")
  continue only if another candidate is viable
```

候选失败不应自动 fallback 到主工作区单 agent，除非用户显式允许。否则 multi-agent 的隔离保证会被绕开。

### Review/revision failure handling

Cross-review 是质量增强，不是必需的 correctness gate。

```text
for each review:
  if timeout or provider error:
    write reviews/<reviewer>-on-<target>-round-N.error.md
    continue

for each revision:
  if timeout or provider error:
    keep candidate's pre-revision patch
    record revision failure
```

如果某个 candidate 的 reviewer 失败，不允许让另一个 candidate 直接修改它的分支。作者自修规则仍然成立。

### Arbitration fallback

arbiter 正常输出结构化 decision。若 arbiter 失败：

1. 使用同一 arbiter model 重试一次，输入压缩版 reports + scores。
2. 仍失败时启用 deterministic fallback：

```text
if only one candidate has valid patch and passed candidate verification:
  accept that candidate
else if both have valid patches:
  choose the one with:
    1. verification passed
    2. fewer blocking review findings
    3. smaller changed-files count
    4. lower touched-crate count
  if still tied:
    reject_both and ask user
else:
  reject_both
```

fallback arbitration 必须写入 `arbitration.md`，并标注 `decision_source=deterministic_fallback`。

### Finalizer failure handling

finalizer 修改主工作区前必须：

1. 检查主工作区是否有与目标文件冲突的用户改动。
2. 生成 `final.patch`。
3. dry-run apply。
4. dry-run 成功后才 apply。

失败处理：

```text
patch conflict:
  do not partial apply
  write finalizer-error.md
  keep winner worktree
  report conflict files

dirty main workspace conflict:
  do not overwrite user changes
  fail run with FinalizerFailed

hybrid synthesis incomplete:
  reject hybrid
  either accept single winner or fail
```

### Verification failure handling

verification 失败不是 multi-agent 成功。

```text
if verification command fails:
  write final-verification.log
  if limits.maxIterations remaining:
    send failure to winner/fixer in the final workspace or winner worktree
    rerun verification
  else:
    mark failed
```

verification timeout 与 verification failure 区分：

- timeout: `VerificationTimedOut`，结果不确定，不能 claim complete。
- non-zero exit: `VerificationFailed`，有明确失败证据。

### Artifact write failures

artifact 写入失败会降低可审计性，处理策略：

- task brief / state 写入失败：fatal。
- candidate log 写入失败：fatal，避免丢失实现证据。
- review 文档写入失败：fatal for review phase；可跳过 review 进入 arbitration，但必须把错误写入 state。
- telemetry 发送失败：non-fatal。

### User-visible result states

最终状态必须明确：

```text
complete
  final patch applied and verification passed

complete_with_warnings
  final patch applied; non-critical review/cleanup/telemetry failed; verification passed

failed
  no final patch applied, or final patch applied but verification failed and could not be fixed

cancelled
  user cancelled; children stopped; artifacts/worktrees retained as needed

inconclusive
  verification timed out or arbiter/finalizer could not prove a safe winner
```

只有 `complete` 和 `complete_with_warnings` 可以向用户报告任务完成。

## Prompt 模板

建议放在：

```text
lingxi-code/multi-agent/prompts/
  implementer.md
  reviewer.md
  reviser.md
  arbiter.md
```

`implementer.md`：

```md
You are an independent implementation agent.

Implement the task in your assigned isolated worktree.
Do not inspect the competing implementation.
Follow existing project patterns.
Keep the diff minimal unless the task requires architectural change.
Add or update tests when correctness risk justifies it.

Return:
- approach summary
- changed files
- design decisions
- tests run
- known risks
```

`reviewer.md`：

```md
Review the competing implementation.

Do not edit files.
Produce a review document only.

Prioritize:
- correctness
- missing requirements
- regressions
- security/privacy risks
- maintainability
- test coverage
- unnecessary complexity

Classify findings:
- blocking
- non-blocking
- suggestion
```

`reviser.md`：

```md
You are revising your own implementation based on the competing agent's review.

Only modify your assigned worktree.
Address blocking issues first.
Keep a short note for each review item:
- fixed
- rejected with reason
- not applicable with reason

Run relevant verification again.
```

`arbiter.md`：

```md
You are the arbiter.

Compare Implementation A and Implementation B by code evidence only.
Do not prefer either provider by identity.

Score 1-5:
- correctness
- completeness
- simplicity
- maintainability
- project fit
- test coverage
- regression risk
- security risk

Choose exactly one:
- accept A
- accept B
- synthesize hybrid
- reject both

Return:
- decision
- score table
- critical issues in each implementation
- rejected alternatives
- final patch strategy
- required verification commands
- remaining risks
```

## Finalizer

Finalizer 是 host 控制的最终写入阶段。

输入：

- `task-brief.md`
- A/B final diff
- A/B self-report
- cross-review docs
- arbitration result

输出：

- `final.patch`
- 主工作区修改
- `final-verification.log`

策略：

```text
accept A:
  apply A patch to main workspace

accept B:
  apply B patch to main workspace

hybrid:
  根据 arbitration.md 生成 patch plan
  优先手工/结构化 apply 指定文件块
  必要时调用单一 executor agent 修改主工作区

reject both:
  保留两个 worktree 和 review docs
  请求用户决策；不静默回退 single-agent
  （与“候选失败不自动 fallback 单 agent”规则保持一致，避免绕过隔离语义）
```

不要直接 `git merge` 两个候选分支。候选分支可能包含互斥设计、重复改动或临时调试文件。

## Verification

verification command 来源顺序：

```text
1. task brief 显式指定
2. candidate self-report 提供
3. arbiter.requiredVerification
4. repo 默认验证
```

本仓库建议默认：

```text
cargo test -p <affected-crate>
cargo clippy -p <affected-crate> --all-targets -- -D warnings
```

⚠️ **不要把 `cargo fmt --check` 放进默认 verification。** 本仓库不保持 fmt-clean（项目约定不跑 `cargo fmt`），`cargo fmt --check` 会 non-zero exit，使得即便候选实现完全正确，verification 也必然落入 `VerificationFailed` → 永远 claim 不了 `complete`。格式检查若确有需要，只能作为某个 candidate 自己 scope 内的 opt-in 命令，绝不进默认套件。

如果修改跨 workspace 或无法确定 affected crate：

```text
cargo test --workspace
```

实际命令应由 finalizer 根据 changed files 收窄，避免每次 dual-LLM 都跑完整 workspace。

“changed files → affected crate”映射本身是一个独立的、可单测的单元：输入一组 changed paths，输出受影响的 crate 集合（解析 workspace 成员路径前缀）。必须显式实现并测试，否则会默默退化成 `cargo test --workspace`，拖垮每次 run 的时延与成本。

验证失败处理：

```text
1. 记录 failure log。
2. 若 limits.maxIterations 未用完，把失败交给 winner 作者或专门 debugger agent。
3. 修复仍在最终主工作区或 winner worktree 中单点进行。
4. 再次运行 verification。
5. 超限后标记 failed，保留 artifacts。
```

`limits.maxIterations` 控制“最终实现修复 + verification”循环次数；`maxReviewRounds` 只控制候选之间的互审轮数。两者不能混用，否则一个 noisy reviewer 会耗尽最终修复预算。

## Telemetry 与成本

新增事件建议：

```text
tengu_multi_agent_started
tengu_multi_agent_candidate_started
tengu_multi_agent_candidate_completed
tengu_multi_agent_review_completed
tengu_multi_agent_revision_completed
tengu_multi_agent_arbitration_completed
tengu_multi_agent_finalizer_completed
tengu_multi_agent_verification_completed
tengu_multi_agent_failed
```

字段：

```text
run_id
strategy
phase
provider_id
model
winner
decision
duration_ms
changed_files_count
verification_passed
cost_nano_usd
```

敏感字段：

- prompt、diff、file path 默认 PII tagged，不进安全列。
- provider credential 绝不落日志。
- review 文档和 patch 只落本地 `.lingxi/multi-agent/runs`。

## 安全约束

这些必须在代码层 enforce，不能只靠 prompt：

1. worktree 隔离：candidate 的 cwd 必须是自己的 worktree。
2. reviewer read-only：review 阶段使用只读 sandbox 或不给 ToolInvoker 写权限。
3. 主工作区单写者：只有 finalizer 可写主工作区。
4. permission 继承：candidate agent 使用与主会话一致或更严格的 permission mode。
5. destructive commands：默认不允许 `git reset --hard`、删除主工作区、清理对方 worktree。
6. cleanup fail-closed：worktree dirty summary 不可用时保留 worktree。
7. cancellation：用户取消时终止所有候选 background tasks，保留 artifacts。

## 与现有 Team/Coordinator 的关系

`coordinator` 当前更像“用户可见的团队/teammate 协作模式”，包含 mailbox、team registry、SendMessage、TeamCreate 等。

本方案的 dual-LLM workflow 是“host-managed competitive execution”，默认不暴露为用户可见 teammate。建议：

- 不复用 `TeamCreate` 作为入口，避免用户团队语义和候选竞争语义混淆。
- 可以复用 `tasks::TaskRegistry` 跟踪 background lifecycle。
- 可以复用 `coordinator::status_sink` 这类状态展示基础设施。
- 未来如果 UI 要展示候选 agent，可把 candidate 映射成只读 task row，而不是普通 teammate。

## 文件与 crate 改动计划

### Phase 1: 文档与配置骨架

改动：

- `docs/multi-agent-dual-llm-design.md`：本设计文档。
- `lingxi-code/engine/src/settings/schema.rs`：新增 `multi_agent: Option<Value>`。
- `lingxi-code/engine/src/settings/merger.rs`：新增 `multiAgent` deep-merge。
- `lingxi-code/engine/src/settings/tracer.rs`：记录 `multiAgent` provenance。
- `lingxi-code/engine/src/settings/env_parser.rs`：解析 `LINGXI_MULTI_AGENT*`。

测试：

- settings schema roundtrip。
- `multiAgent` deep-merge：project 层与 user 层的 `multiAgent` 子键**逐键合并而非整块覆盖**（回归 `deep_merge_value_opt` 是否真的接上）。
- env 覆盖 settings。

### Phase 2: 新增 `multi-agent` crate

改动：

- `lingxi-code/multi-agent/Cargo.toml`
- `lingxi-code/multi-agent/src/config.rs`
- `lingxi-code/multi-agent/src/router.rs`
- `lingxi-code/multi-agent/src/state.rs`
- `lingxi-code/multi-agent/src/prompts.rs`

测试：

- route precedence。
- keyword/complexity trigger。
- invalid config diagnostics。
- `timeoutSeconds` / `phaseTimeoutSeconds` 解析与默认值。

### Phase 3: Worktree + artifact manager

改动：

- `multi-agent/src/worktrees.rs`：包装 `WorktreeManager`。
- `multi-agent/src/artifacts.rs`：写 `.lingxi/multi-agent/runs/<run_id>`。

测试：

- slug 长度与 flatten 兼容。
- run artifact layout。
- cleanup fail-closed。
- artifact 写入失败时返回 fatal error。
- worktree 创建失败 / 返回 `None` 一律当 fatal，不降级到主工作区（不复用 `create_worktree_or_degrade`）。
- 两个 worktree 串行创建（不并发 `git worktree add`）。

### Phase 4: Candidate implementation runner

改动：

- `multi-agent/src/orchestrator.rs`
- `multi-agent/src/providers.rs`
- 接入 `llm-client` / `agent` loop 或先接外部 provider adapter。

测试：

- mock provider 并行执行。
- candidate 只能写自己的 cwd。
- timeout/cancel 清理。
- 单个 candidate 超时后另一个 candidate 可继续。
- 两个 candidate 都失败时 run failed 且 artifacts 保留。

### Phase 5: Cross-review + revision + arbitration

改动：

- `multi-agent/src/review.rs`
- `multi-agent/src/revision.rs`
- `multi-agent/src/arbiter.rs`
- prompt 模板。

测试：

- reviewer read-only。
- revision 只改 author worktree。
- `maxReviewRounds=0` 跳过 review。
- 达到 `maxReviewRounds` 后强制进入 arbitration。
- review timeout 写 `.error.md` 并继续。
- revision timeout 保留 revision 前 patch。
- arbiter structured decision parsing。
- arbiter 失败后重试一次，再 fallback deterministic arbitration。

### Phase 6: Finalizer + verification

改动：

- `multi-agent/src/finalizer.rs`
- `multi-agent/src/verification.rs`
- execution router 接入 `orchestrator` / composition root。

测试：

- accept A/B patch apply。
- hybrid plan requires explicit patch strategy。
- verification failure loop。
- finalizer patch dry-run 失败时不 partial apply。
- verification timeout 进入 `inconclusive`，不得 claim complete。
- finalizer 是唯一主工作区写入者。
- changed-files → affected-crate 映射：给定一组 changed paths 解析出正确的 crate 集合（默认套件**不含** `cargo fmt --check`）。

### Phase 7: UI/CLI 控制

改动：

- CLI flags：`--multi-agent`、`--dual-llm`、`--no-multi-agent`。
- TUI status：显示 `dual-llm: implementing/reviewing/arbitrating/verifying`。
- `/config` 可选支持 `multiAgent.enabled`、`multiAgent.mode`。

测试：

- CLI flag precedence。
- TUI render snapshot。
- `/config` 写 settings 时保留未知字段。

## 最小可行版本

MVP 不需要一次实现 hybrid 和完整 UI。

MVP 范围：

```text
1. settings + CLI 开关
2. 两个 worktree
3. 两个 candidate 并行实现
4. 生成 self-report + patch.diff
5. 可选 cross-review，受 maxReviewRounds 限制
6. arbiter 只做 accept A / accept B / reject both
7. finalizer 应用 winner patch
8. 运行 verification
```

MVP 暂不做：

- hybrid synthesis
- candidate author revision
- TUI rich comparison
- 多于两个 provider
- 分布式执行

## 风险与取舍

| 风险 | 处理 |
|---|---|
| 成本和延迟显著增加（≈ 4–6× 单 agent：2× 实现 loop + 2× review + 2× revision loop + 1× arbiter + N× verification 修复迭代，而非 2×） | 默认 `enabled=false`；`mode=auto` 触发器收紧（见路由策略），只有显式 / 高风险触发 |
| 两个候选方案都错误 | arbiter 支持 `reject_both`，保留 artifacts，回退 single-agent 或请求用户 |
| review 文档噪音过多 | reviewer prompt 强制 blocking/non-blocking/suggestion 分类 |
| cross-review 循环失控 | `maxReviewRounds` 硬限制，达到上限必须进入 arbitration |
| hybrid 合成引入新 bug | MVP 禁用 hybrid；后续必须由 finalizer + verification 收口 |
| worktree 清理误删 | dirty summary fail-closed，未知则保留 |
| parity 字段污染 | `multiAgent` 明确标注 LingXi-only，不进入 claude-code parity guarantee |

## 验收标准

1. 用户可以通过 config 或 CLI 显式开启/关闭 dual-LLM。
2. auto 模式只在 trigger 命中时启用。
3. 两个用户配置的 candidate 实现运行在不同 worktree。
4. reviewer 阶段没有写权限。
5. 作者修正阶段只能修改自己的 worktree。
6. cross-review 轮数受 `maxReviewRounds` 限制，达到上限后必须仲裁。
7. arbiter 输出结构化 decision。
8. finalizer 是唯一修改主工作区的阶段。
9. verification 失败不会报告完成。
10. artifacts 完整保存 task brief、patch、review、arbitration、verification log。
11. 取消或失败后不会丢失候选 worktree 中的未合并工作。
