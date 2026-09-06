# Fusion 多模型审议 — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.

**Goal:** 在 LingXi 增加第五种运行方式 Fusion：同一任务由多个模型并行只读作答，Analyst 结构化评估，再 Pick 或由父模型 Merge，三个入口（Agent / `/fusion` / workflow）在默认关闭下可独立回滚地交付。

**Architecture:** Fusion 是编排层，不是第五套 LLM loop。`platform-api` 放 DTO + `FusionExecutor` trait；新 crate `fusion` 跑状态机；Panel 走现有 `SubagentSpawner`（隐藏 `fusion-panel`）；Analyst 走新的严格 `query_json_schema`；Synthesizer 只在 Merge 时用父 model/profile 调一次 SideQuery。`tool-agent` / `tasks` / `workflow` 只依赖 trait，组合根在 `apps/engine-desktop`。

**Tech Stack:** Rust workspace（lingxi-code）、tokio JoinSet、现有 SubagentSpawner / SideQueryClient / BudgetEnforcer / TaskRegistry / QuickJS workflow / command-api + TUI slash。

**Repo copy:** `docs/plans/2026-08-31-fusion-multi-model-deliberation.md`（与本文件同步）。

---

## 0. 全局约束（全程有效）

- `fusion.enabled` 默认 **false**。每个 PR 合入后 Fusion 对外 inert，直到显式打开。
- **不**新增名为 `Fusion` 的顶层 builtin tool。
- **不**改 `BUILTIN_SUBAGENT_TYPES`（仍 4 项）和 `BUILTIN_COMMAND_NAMES`（仍 108）。
- **不**给 `BuiltinToolContext` 加字段（mobile / 测试用完整 struct literal）。
- `fusion` crate **禁止**依赖 `tool-agent`、`tasks`、`command-core`、`agent`。`fusion-panel` 定义留在 `agent` crate，orchestrator 只传 `subagent_type`。
- 预算单位 **nano_usd: u64**，不要 `Decimal`。
- 移动端不注册 executor；调用返回 `UnavailableOnPlatform`，禁止静默降级。
- 不要跑整个 workspace `cargo fmt`。改动 crate 用 `cargo test -p <crate>`。
- Panel 互不可见；Analyst 匿名；Pick 零 synth 调用；Merge 恰好一次父模型调用。

---

## 1. 锁定的产品决策

| 项 | 决策 |
|---|---|
| 形态 | OpenRouter Fusion 式审议，不在父树写文件 |
| Panel 工具 | Explicit：`Read`, `Grep`, `Glob`, `WebFetch`。当前不开放 `Bash`，因为继承 session permission 无法结构化保证只读；未来只能通过可信的只读 Shell wrapper 恢复 |
| Analyst | 无工具；temp=0；严格 JSON Schema |
| Synthesizer | 仅 Merge；inherit 父 model/profile；失败 → NeedsParent，不换模型 |
| 默认维度 | `evidence_quality`, `coverage`, `reasoning`, `safety`, `actionability` |
| Agent / Workflow | 默认当前 provider/profile |
| `/fusion` | 默认跨 provider；`--same-provider` 强制同源；enabled=false 也可跑（用户逐次授权） |
| 历史 | Agent 只写 tool_result；Slash 一条 `user_meta`；Workflow 不写主历史 |
| 公开 listing | `fusion` 仅当 enabled=true 且注入了 executor |
| 隐藏类型 | `fusion-panel` 永不进 listing，lookup 对标 `fork` |

---

## 2. 数据契约（`platform-api/src/fusion.rs`）

输入（slash args / workflow opts / Agent extra fields）未知字段拒绝。持久化（FusionResult / task spool / history meta）未知可选字段忽略，带 `schema_version: u16 = 1`。

### 2.1 请求

```rust
pub enum FusionOrigin { Agent, Slash, Workflow }
pub enum FusionPreset { Quality, Fast }

pub struct FusionModelRef {
    pub profile: Option<String>,
    pub model: String,
}

pub struct FusionRequest {
    pub schema_version: u16,           // 1
    pub origin: FusionOrigin,
    pub prompt: String,                // 非空
    pub preset: FusionPreset,
    pub models: Option<Vec<FusionModelRef>>, // 显式列表，至少 2
    pub dimensions: Vec<String>,       // 1..=12, snake_case, 去重保序
    pub partial_ok: bool,
    pub max_panel: Option<u8>,         // clamp 到 settings.maxPanel，2..=8
    pub cross_provider: bool,
    pub parent_profile: String,
    pub parent_model: String,
    pub conversation_id: Option<String>,
    pub workflow_run_id: Option<String>,
}
```

调用方 **不** 传 budget handle / permission / client。由 `FusionInheritance`（组合根注入）提供。

### 2.2 PanelReport（Panel 强制 schema）

```rust
pub struct PanelReport {
    pub schema_version: u16,
    pub summary: String,
    pub candidate_answer: String,
    pub claims: Vec<PanelClaim>,          // evidence_refs ⊆ evidence.id
    pub evidence: Vec<PanelEvidence>,     // id 报告内唯一
    pub assumptions: Vec<String>,
    pub risks: Vec<PanelRisk>,
    pub unresolved_questions: Vec<String>,
}
```

Host 再验证：confidence 0..=100；字符串 NUL 清理 + `subagent_output_guard`；超条目/字节上限 = 协议失败。

### 2.3 Analyst / 结果

```rust
pub enum FusionRecommendation {
    Pick { panel_id: String, reason: String },
    Merge { reason: String },
    NeedsParent { reason: String },
}

pub enum FusionStatus { Completed, NeedsParent }
pub enum FusionDecision {
    Picked { panel_id: String },
    Merged,
    NeedsParent { reason: FusionNeedsParentReason },
}

pub struct FusionResult {
    pub schema_version: u16,
    pub run_id: String,                 // `fu_` + ulid，不是 task id
    pub status: FusionStatus,
    pub decision: FusionDecision,
    pub final_text: String,
    pub analysis: Option<FusionAnalysis>,
    pub panels: Vec<PanelOutcome>,      // 匿名 id；无完整 PanelReport
    pub usage: FusionUsage,
    pub timing: FusionTiming,
    pub egress_profiles: Vec<String>,
}
```

NeedsParent 是 **Ok 成功状态**，不是 ToolError。宿主决策：

- Pick：panel_id 必须存在且有 `candidate_answer`
- Merge：`confidence >= 60` 且无未解决 critical contradiction，否则强制 NeedsParent
- scores 的 panel id / dimension / 0..=100 必须与请求完全匹配

> **落地更新（F004）：** `FusionNeedsParentReason` 落地时比本节最初列的 `AnalystRequested` / `AnalysisParseFailed` / `CriticalContradiction` / `LowConfidence` / `SynthesisFailed` 多一个变体 `AnalysisFailed { category: String }`——覆盖 analyst 调用本身失败（超时、传输/4xx/5xx、或面板跑完后才发现 analyst 路由不支持结构化输出）而不是"JSON 解码/校验失败"的情形；旧实现把这类失败也贴上 `AnalysisParseFailed` 标签，误导排障。`category` 是 sanitize 过的分类字符串（如 `"timeout"`、`"structured_output_unsupported"`），从不携带原始 provider 错误体。`SynthesisTimedOut` 同理是 `SynthesisFailed` 之外单独区分超时的变体。

### 2.4 错误

预检类（零 provider 调用）：`Disabled`, `UnavailableOnPlatform`, `InvalidConfiguration`, `InvalidRequest`, `TooFewModels`, `InvalidCustomModels`, `CrossProviderDenied`, `NoJudgeModel`, `StructuredOutputUnsupported`, `BudgetReservationUnavailable`, `BudgetExceeded`, `SpawnLimitExceeded`。

运行类：`AllPanelsFailed`, `MinPanelsNotMet`, `PanelSetIncomplete`, `TimedOutEmpty`, `Cancelled`, `Internal`。

### 2.5 Trait

```rust
#[async_trait]
pub trait FusionExecutor: Send + Sync {
    async fn run(
        &self,
        request: FusionRequest,
        inherit: FusionInheritance,
        progress: Option<tokio::sync::mpsc::Sender<FusionProgress>>,
    ) -> Result<FusionResult, FusionError>;
}
```

---

## 3. Settings（`core/src/settings/schema.rs`）

`MERGE_STRATEGIES` 增加 `("fusion", MergeStrategy::DeepMerge)`。对象 deep-merge，数组整体替换。

```json
{
  "fusion": {
    "enabled": false,
    "preset": "quality",
    "qualityPanelCount": 3,
    "fastPanelCount": 2,
    "maxPanel": 8,
    "minSuccessfulPanels": 2,
    "partialOk": true,
    "panelMaxTurns": 12,
    "panelMaxOutputTokensPerTurn": 8192,
    "panelReservedInputTokensPerTurn": 32768,
    "maxReservedNanoUsd": null,
    "analystMaxOutputTokens": 8192,
    "synthesizerMaxOutputTokens": 16384,
    "panelIdleTimeoutMs": 180000,
    "panelTotalTimeoutMs": 600000,
    "analystTimeoutMs": 120000,
    "synthesizerTimeoutMs": 180000,
    "totalTimeoutMs": 1200000,
    "analysisProtocolRetries": 1,
    "slashCrossProviderDefault": true,
    "allowCrossProviderForAgent": false,
    "allowCrossProviderForWorkflow": false,
    "allowedProfiles": [],
    "workflowFusionCallCap": 20
  }
}
```

> **落地更新（F004）：** `totalTimeoutMs` 默认值由 900000 提到 **1,200,000**（上方 JSON 已同步）——原来的 900000 连自身的阶段默认值都装不下（600000+120000*2+180000=1,020,000），会被本节新增的阶段和校验直接判非法。`FusionSettingsJson::validate`（`core/src/settings/schema.rs`）与 `fusion::FusionRuntimeConfig::defaults()`（`fusion/src/config.rs`）的默认值刻意保持锁步，改一处必须改另一处，否则一个缺省的 `fusion.totalTimeoutMs` 会在校验器和运行时配置之间读到两个不同的值。

校验：counts ≥ 2；maxPanel 2..=8；counts ≤ maxPanel；minSuccessful ≤ panel count；每个 timeout 阶段单独 ≤ total；**阶段之和 ≤ total**（`panelTotalTimeoutMs + analystTimeoutMs*(1+analysisProtocolRetries) + synthesizerTimeoutMs ≤ totalTimeoutMs`，F004——否则全部面板成功完成的一次 run 仍可能因为 analyst 重试 + synthesizer 的耗时把端到端截止时间挤爆，被误报成"未产出任何面板前超时"）；retries 0 或 1。这两条校验分别在两处生效并互为兜底：`FusionSettingsJson::validate` 只看单个 settings 分层文件自身出现的字段（分层文件只设置其中一侧时不误判整体），`FusionRuntimeConfig::from_settings`（`fusion/src/config.rs`）在所有分层合并、每个字段取到最终值之后对**合并结果**重跑同一条阶段和不变量以及 `minSuccessfulPanels ≤ min(qualityPanelCount, fastPanelCount)`。无效配置不 panic，入口返回 `InvalidConfiguration`。

**预留公式（覆盖 Codex 1byte=1token 峰值）：**

```
reserved_input  = panelCount * panelMaxTurns * panelReservedInputTokensPerTurn
reserved_output = panelCount * panelMaxTurns * panelMaxOutputTokensPerTurn
                + analystMaxOutputTokens * (1 + analysisProtocolRetries)
                + synthesizerMaxOutputTokens
reserved_usd    = price(input)*reserved_input + price(output)*reserved_output
                  + per-request fees * max_calls
if maxReservedNanoUsd is Some: reserved_usd = min(reserved_usd, maxReservedNanoUsd)
                  若理论峰值仍大于 session 剩余 → BudgetExceeded，零调用
```

1 byte = 1 token **只**用于单次调用 usage 缺失时的结算兜底，不用于预留。

> **落地更新（F001 / G003 / G001）：** 上面的公式此前只在纸面上成立——`FusionOrchestrator` 组装时价格表恒为 `()`（`FusionPriceBook for ()` 永远 `rates_for → None`），按 token 计费的模型在有硬 `--max-budget` 时预检必拒（`"has no price"`），没有硬上限时每次预留又都报价 $0，硬预算不变量没有接到任何真实数字上。落地做法：
> - `apps/engine-desktop` 新增 `DesktopFusionPriceBook`，包一层会话本就在用的 `cost::PricingCatalog`（`CostTracker` 计费的同一张表），用主循环 `record_api_response_v2` 同款的 `orchestrator::cost_wiring::model_ref_from_string` 做 `(profile, model)` 解析，`desktop_fusion_executor` 组装时 `.with_price_book(Arc::new(book))`。定价目录没有独立的按次费率字段，`per_request_nano_usd` 固定填 0（"没有固定费"而不是"没定价"，不影响硬预算门靠 token 费率生效）。
> - `realized_nano_usd` 不再是"运行前后 session 级 CostTracker 差值"这种启发式（旧写法会把同一会话里父模型自己产生的花费也记到 Fusion 头上，G001）；改为 `fusion::orchestrator` 在 run 结束时用同一张价格表，对**这一次 run 自己的** usage（每个 Panel 的 cumulative usage + Analyst + Synthesizer）单独计价；任何一个组件缺费率就把整个结果标 `estimated = true`，不是悄悄按 0 记账。
> - `cost::budget::BudgetEnforcer::commit_reservation` 此前 `let _ = actual_nano_usd;`——预留的钱在 commit 时直接消失，不进 `CostTracker`，一个设了 `--max-budget` 的会话可以在 Fusion 上无限超支。落地后 `commit_reservation` 先 `cost_tracker.record_external_cost(actual_nano_usd)` 再释放持有；这是 Fusion usage 唯一进入会话总花费的入口，因为 Fusion 的 provider 调用全部走 `ProviderApiAdapter` / `ProviderSideQueryClient`，从不经过 `record_api_response_v2`。
> - `platform-api::task_registry::stop_background_agents_for_budget` 此前只认 `local_agent` / `local_workflow`，超预算时后台 `/fusion` 任务不会被停；加了 `"local_fusion" => true`（G003）。

---

## 4. 模型解析

`llm-client` `ModelProfile` / `platform-api` `ModelListing` 增加可选 `FusionModelHints { eligible, quality_rank, latency_class, cost_class, judge_eligible }`。

自动预设只收 `eligible=true`。Analyst 额外要求 `judge_eligible && capabilities.structured_output`。**禁止**用模型名、catalog 顺序、provider 顺序当质量。

Hint 表是 checked-in policy；**ID 必须从当前 catalog preset 逐条抄**，抄不到则该行不生效（默认 ineligible）。禁止模糊匹配。

- Quality：跨源时每 profile 先取最高 quality 各 1，再按 rank 补齐；同分：cost 低 → latency 快 → profile 字典序 → model 字典序。
- Fast：latency → quality → cost → 字典序；默认 2 个。
- 少于 2 个可用 → `TooFewModels`。无合格 Analyst → `NoJudgeModel`。均在任何 Panel 调用前失败。
- 自定义 `models` 可含未标 eligible 的模型，但仍要存在、可用、≥2 个不同 ref、不违反 allowlist/跨源策略。按 token 计费却无价格且有硬预算 → 拒绝。

---

## 5. 状态机

```
Created → ResolvingModels → ReservingBudget → RunningPanels
       → Analyzing → Deciding
            ├ Pick → Completed
            ├ NeedsParent → NeedsParent
            └ Merge → Synthesizing → Completed | NeedsParent
Cancel / 预检失败 / 0 成功 / 低于 min → Failed | Cancelled
```

每个 terminal 只跑一次 finalize：取消子任务、释放 Panel slot、释放 spawn reservation 未使用部分、释放预算余额、flush usage/telemetry、只发一次完成通知。迟到事件不得覆盖 terminal。

Panel：JoinSet + CancellationToken。idle watchdog 作用于 provider stream 建连与每个响应事件，并在每个事件后重置；另有 end-to-end panel total timeout。成功数 ≥ min 且 partialOk → 进 Analyst。

> **落地更新（F002）：** Panel 的子代理 spawn request 使用 `structured_output_mode: StructuredOutputMode::WhenDone`（`fusion/src/panel.rs`），不是对整轮循环恒定的 `Forced`——runner 在还有只读工具可用、且未到最后一轮时用普通（auto）`tool_choice`，只在最后一轮或模型连续无工具调用时才强制 `StructuredOutput`。这条能力是 runner 层新加的 additive 字段（`SubagentSpawnRequest::structured_output_mode`，serde default 仍是 `Forced`，保证既有 `agent({schema})` workflow 的字节级行为不变），Fusion 是唯一把它设为 `WhenDone` 的调用方。

> **落地更新（F007）：** `FusionOrchestrator` 不在构造时把 `FusionRuntimeConfig` 冻结一份——`run()` 一开始就通过 `FusionConfigSource::load()` 重新读一次当前生效配置（`fusion/src/config.rs`），`agent_surface()` / `workflow_fusion_call_cap()` 同理。见 §11 kill switch 的对应说明。

父 provider/profile：优先使用 session 显式身份。legacy/resume 只有 bare model 时，仅当 live catalog 中该 model 唯一对应一个 profile 才回填；多 profile 重名必须 fail-closed，禁止按 catalog 顺序猜测 same-provider 路由。

Analyst 输入：匿名 P1..Pn，`run_id` 派生稳定洗牌。无 provider/model。非法 JSON 同模型重试 1 次，再失败 → NeedsParent(AnalysisParseFailed)。禁止从文本里抠 JSON。

---

## 6. PR 与任务

每 PR 保持可编译、默认关闭、有测试。提交信息用 `feat(fusion): ...`。

### PR1 — 类型、配置、hints、strict SideQuery（inert）

#### Task 1.1 公共 DTO + trait

**Files:**
- Create: `lingxi-code/platform-api/src/fusion.rs`
- Modify: `lingxi-code/platform-api/src/lib.rs`（`mod fusion; pub use fusion::*`）

**Steps:**
- [ ] 写入 §2 全部类型、`FusionExecutor`、`FusionInheritance`（budget / spawner / side_query / cancel / settings snapshot 的 Arc 句柄）。
- [ ] 单测：dimensions snake_case / 去重 / 1..=12；recommendation 反序列化；schema_version 默认 1。
- [ ] `cargo test -p platform-api fusion::`

#### Task 1.2 Settings

**Files:**
- Modify: `lingxi-code/core/src/settings/schema.rs`（`FusionSettingsJson`、`SettingsJson.fusion`、`MERGE_STRATEGIES`）
- Modify: 现有 settings 单测模块

**Steps:**
- [ ] 默认 `enabled=false`。
- [ ] 测 deep-merge、数组替换、边界值、无效 timeout → 诊断而非 panic。
- [ ] `cargo test -p core settings::`

#### Task 1.3 QuerySource 三分

**Files:**
- Modify: `lingxi-code/sidequery/src/purposes.rs`

```rust
FusionPanel, FusionAnalyst, FusionSynthesizer
// as_str: "fusion_panel" | "fusion_analyst" | "fusion_synthesizer"
```

- [ ] roundtrip 单测。现有 variant 行为不变。
- [ ] `cargo test -p sidequery purposes::`

#### Task 1.4 FusionModelHints

**Files:**
- Modify: `lingxi-code/llm-client/src/config.rs` `ModelProfile`
- Modify: `lingxi-code/platform-api` 的 `ModelListing`（若 listing 独立）
- Create: `lingxi-code/llm-client/src/fusion_hints.rs`（checked-in 表）
- Modify: `lingxi-code/llm-client/src/catalog/presets.rs` 在组装 ModelProfile 时按 **精确 request_model** 填 hints

**Steps:**
- [ ] 表内 ID 必须 `catalog` 里存在，否则该行 skip（单测：未知 ID 不 panic）。
- [ ] 无 hint 的模型 `eligible=false`。
- [ ] `cargo test -p llm-client fusion_hints::`

#### Task 1.5 strict JSON SideQuery

**Files:**
- Modify: `lingxi-code/sidequery/src/side_query.rs`（新方法，**不改** `output_format` 语义）
- Modify: `lingxi-code/sidequery/src/provider_side_query.rs`（走 `ApiService::stream_json_schema`）

```rust
async fn query_json_schema<T: DeserializeOwned>(
    &self,
    request: StrictStructuredQueryRequest,
    schema: serde_json::Value,
) -> Result<StrictStructuredQueryResponse<T>, SideQueryError>;
```

- [ ] provider 不支持 → `StructuredOutputUnsupported`。
- [ ] 非 JSON / 截断 / 额外文本 → 协议失败，不抠子串。
- [ ] 现有 `output_format` 单测仍过（宽松路径）。
- [ ] `cargo test -p sidequery`

**PR1 验收:** 默认 inert；无公开入口；无新 tool 名。

---

### PR2 — fusion crate 状态机（无公开入口）

#### Task 2.1 crate 骨架

**Files:**
- Create: `lingxi-code/fusion/Cargo.toml`, `src/lib.rs`, `config.rs`, `model_resolver.rs`, `panel.rs`, `analyst.rs`, `decision.rs`, `synthesizer.rs`, `budget.rs`, `orchestrator.rs`, `progress.rs`
- Modify: `lingxi-code/Cargo.toml` workspace `members` + `default-members` 加 `"fusion"`

依赖：`platform-api`, `sidequery`, `protocol`, `cost`, `core`, `tokio`, `async-trait`, `serde`, `thiserror`, `tracing`。**不要**依赖 `agent` / `tool-agent` / `tasks`。

#### Task 2.2 hidden fusion-panel

**Files:**
- Modify: `lingxi-code/agent/src/builtins.rs` 增加 `fusion_panel_definition()`（对标 `fork_agent_definition`）
- Modify: `lingxi-code/agent/src/handle.rs` `lookup_definition`：在 fork 之后、catalog 之前解析 `fusion-panel`，用户同名 agent 不能覆盖
- Modify: `lingxi-code/agent/src/handle.rs` `agent_listing_entries`：**过滤** `fusion-panel`

定义：
- `AgentToolPolicy::Explicit`（不是 Except）：`Read, Grep, Glob, WebFetch`
- 不开放 `Bash`：提示词不能阻止继承父 session 权限的 shell 写入工作区；在有可信的只读 Shell wrapper 前保持结构化只读
- `permission_mode: Bubble`
- `max_turns: 12`（可被 request 降低，不能超 settings）
- `schema`: PanelReport JSON Schema 字符串
- `model: Inherit`（spawn 时被 per-panel override）

- [ ] 单测：listing 不含 fusion-panel；lookup("fusion-panel") 返回 Explicit 四个只读工具。
- [ ] `BUILTIN_SUBAGENT_TYPES` 测试仍是 4。

#### Task 2.3 SubagentSpawnRequest / 累计 usage

**Files:**
- Modify: `lingxi-code/platform-api/src/subagent_spawn.rs` additive：

```rust
pub max_turns_override: Option<u32>,
pub max_output_tokens_per_turn: Option<u32>,
pub max_input_bytes_per_turn: Option<u64>,
pub query_source: Option<sidequery::QuerySource>, // 若 cycle，用 string label
pub correlation_id: Option<String>,
```

`SubagentResult::Completed` additive `cumulative_usage: SubagentUsage`（serde default）。

- Modify: `lingxi-code/agent/src/runner.rs`：每轮累加 `llm_client::Usage`；执行 per-turn output/input ceiling；`query_source` 传到 API 调用。
- [ ] 两轮 fake client：final-turn usage ≠ cumulative；ceiling 截断。

> QuerySource 在 platform-api 可能造成 cycle。优先在 spawn request 上放 `Option<String>` COGS label（`"fusion_panel"`），由 runner 映射到 sidequery::QuerySource。

#### Task 2.4 Orchestrator + fake 测试

**Files:** `lingxi-code/fusion/src/*.rs` + `fusion/src/orchestrator_test.rs`

用 fake `SubagentSpawner` + fake `SideQueryClient`：

- [ ] 3 Panel 并发，互不可见（各 spawn prompt 不含其它 panel 输出）
- [ ] 1 失败 + partialOk + min=2 → 进 Analyst
- [ ] min 不足 → MinPanelsNotMet
- [ ] Pick：synth 调用次数 0，final_text = sanitized candidate_answer
- [ ] Merge：synth 恰好 1 次，parent model/profile
- [ ] Analyst 非法 JSON 重试 1 次成功
- [ ] 两次非法 → NeedsParent(AnalysisParseFailed)
- [ ] Synth 失败 → NeedsParent(SynthesisFailed)，final_text 为宿主确定性摘要
- [ ] critical contradiction → 强制 NeedsParent
- [ ] cancel 各阶段无 slot 泄漏（JoinHandle 全 join）
- [ ] PanelReport 伪造 system/tool 指令被 guard 清掉后再进 Analyst
- [ ] `cargo test -p fusion`

**PR2 验收:** 无 Agent listing 变化；无 slash；无 workflow 全局。

---

### PR3 — 硬预算（仍无公开入口）

#### Task 3.1 reservation API

**Files:**
- Modify: `lingxi-code/platform-api/src/budget.rs`

```rust
async fn reserve_nano_usd(&self, nano_usd: u64) -> Result<BudgetReservationId, BudgetError>;
async fn commit_reservation(&self, id: BudgetReservationId, actual_nano_usd: u64) -> Result<(), BudgetError>;
async fn release_reservation(&self, id: BudgetReservationId);
```

默认 impl：无 max budget 时 reserve 成功（id 可 no-op）；有 max 但未实现 → `Internal`，Fusion 映射 `BudgetReservationUnavailable`。

- Modify: `lingxi-code/cost/src/budget.rs`：原子 `realized + active_reservations <= max_session_nano_usd`。`check_and_charge` / `check_pre_api_call` 必须看见 reservations。
- Drop 只发异步 release 信号；正常路径必须显式 release。

#### Task 3.2 预留公式 + 结算

**Files:** `lingxi-code/fusion/src/budget.rs`

- [ ] fixture：按 Codex 原文 1byte=1token×256KiB×12×3 会超的 session，用修正公式可通过。防止有人改回去。
- [ ] 预留失败 → provider call count = 0。
- [ ] 两个并发 Fusion 不能一起越过余额。
- [ ] 每轮 Panel commit 实际 usage；缺失 usage 用保守上界并标 estimated。
- [ ] 所有 terminal 路径 reservation 归零；不双记。
- [ ] 按 token 计费无价格 + 有 max budget → 拒绝。
- [ ] Subscription 美元预留 0，仍受 token/请求/超时限制。
- [ ] `cargo test -p cost budget::` 与 `cargo test -p fusion budget::`

**PR3 验收:** 仍无公开入口。

---

### PR4 — Agent 入口

#### Task 4.1 AgentToolInput + intercept

**Files:**
- Modify: `lingxi-code/tools/agent/src/agent.rs`

`AgentTool` 增加 `fusion: Option<Arc<dyn FusionExecutor>>`。

```rust
impl AgentTool {
    pub fn with_fusion(self, executor: Arc<dyn FusionExecutor>) -> Self { ... }
}
```

`AgentToolInput` additive serde default：

```rust
pub preset: Option<String>,
pub models: Option<Vec<String>>,      // "profile:model" 或 "model"
pub dimensions: Option<Vec<String>>,
pub max_panel: Option<u8>,
pub partial_ok: Option<bool>,
pub cross_provider: Option<bool>,
```

旧 payload 无这些字段必须仍能反序列化。

`call` 顺序（在普通 spawn 之前）：
1. 解析 input
2. `subagent_type == "fusion"`？
3. 无 executor 或 `!enabled` → 当未知类型 / Disabled（listing 不含则 "Agent type 'fusion' not found"）

   > **落地更新（统一 §3 与本条的矛盾）：** §3 说无效的 `fusion.*` 配置要在入口处返回 `InvalidConfiguration`，但本条字面上只区分"没有 executor"和"`enabled=false`"两种通用消息——一份非法配置（例如阶段超时之和超过 total）如果只是让组合根装不出 orchestrator，会被这里当成普通 Disabled 吞掉，调用方看到的还是泛泛的"未找到/已禁用"，看不出是配置错误。落地做法是 `FusionExecutor` trait 加一个 `preflight_error(&self) -> Option<FusionError>`（默认 `None`，不影响任何正常 executor），组合根在设置校验失败时不再让 `desktop_fusion_executor` 直接 panic 或返回不可用，而是构造一个 `RejectedFusionExecutor` 把校验失败的 `InvalidConfiguration` 钉在这个方法上。`call` 顺序里第 3 步在检查 `enabled` **之前**先查 `executor.preflight_error()`——有值就直接把它当错误返回，绕开"未知类型/Disabled"这条通用分支；workflow 的 `fusion()` 桥（`tasks/src/handlers/local_workflow.rs`）在同一个位置消费同一个方法，两个入口对同一个非法配置给出同一个 `InvalidConfiguration`，不再各说各话。
4. 校验 options → `FusionRequest { origin: Agent, cross_provider: 仅当 allowCrossProviderForAgent && 显式 true }`
5. batch spawn reserve N（见 4.2）
6. `executor.run`
7. `Ok(ToolCallResult)`：`model_content = final_text`；`data` 含 runId/status/decision/panel summary/usage/timing/egress。NeedsParent 仍是 Ok。

- Modify: `lingxi-code/tools/agent/src/lib.rs` 保持 `register_all` 不注入 fusion。
- Modify: `lingxi-code/apps/engine-desktop/src/lib.rs`：构造 `AgentTool::new(ctx).with_fusion(orch)` 再 register（不要改 `register_all` 签名以免 mobile/测试全炸）。

#### Task 4.2 listing + spawn 计数

**Files:**
- Modify: `lingxi-code/tools/agent/src/agent.rs` prompt/listing 路径：executor.is_some() && settings.enabled 时追加 `SubagentListingEntry { agent_type: "fusion", when_to_use: "...并行多模型审议...约 4–5× 成本...", tools_description: "..." }`
- Modify: `lingxi-code/platform-api/src/task_registry.rs`

```rust
fn try_reserve_total_agent_spawns(&self, n: u64, cap: u64) -> Result<u64, u64>;
fn release_total_agent_spawn_reservations(&self, n: u64);
```

Fusion wrapper **不计** 1；每个实际 Panel 计 1。预检失败归还未启动的 N。

- [ ] disabled：listing 无 fusion；显式调用 not found 或 Disabled。
- [ ] enabled：listing 有 fusion；fusion-panel 无。
- [ ] `BUILTIN_SUBAGENT_TYPES.len()==4`。
- [ ] spawn counts：1 次 fusion + 3 panel → total_agent_spawns += 3。
- [ ] `cargo test -p tool-agent`
- [ ] `cargo test -p test-harness four_builtin_subagent_types`

#### Task 4.3 进度

把 `FusionProgress` 映射到现有 `spawn_with_progress` 字符串 / nested Task cell。阶段名用 §7。

**PR4 验收:** Desktop/CLI Agent 可调；mobile 无 executor。

---

### PR5 — `/fusion` + LocalFusion

#### Task 5.1 TaskType

**Files:**
- Modify: `lingxi-code/tasks/src/id.rs` 加 `LocalFusion`，`id_prefix = 'f'` → id `/^f[0-9a-z]{8}$/`
- Modify: `lingxi-code/tasks/src/task_trait.rs` `TaskSpawnInput::LocalFusion { request, conversation_id }`
- Modify: 所有 `TaskType` / `TaskSpawnInput` exhaustive match（registry、persist、notification、CLI/TUI renderer）
- Create: `lingxi-code/tasks/src/handlers/local_fusion.rs`
- Modify: `lingxi-code/test-harness/tests/parity_agent_task_tools.rs` 若锁了 prefix 表，把 `f` 加进去（那是 task prefix，不是 108 commands）

Handler 持有 `Arc<dyn FusionExecutor>` + `Arc<dyn FusionCompletionSink>`（组合根注入，**不要 OnceLock**）。

完成：
1. TaskNotification 展示 final_text 或 NeedsParent 摘要
2. spool 写 sanitized FusionResult
3. sink.publish(conversation_id, result) — 失败不把 Fusion 改成 Failed
4. 幂等键 `(conversation_id, run_id)`

sink 实现：展示 + `ConversationMessage::user_meta` 一条 fusion-result（XML builder + escape，禁止拼接）。默认不含 PanelReport / 完整 analysis JSON / provider raw error。`is_meta=true` 不进模型上下文。

**不**自动触发父模型下一 turn。

#### Task 5.2 slash 命令

**Files:**
- Create: `lingxi-code/commands/core/src/fusion.rs`（或 `apps/engine-desktop` 扩展 handler，只要 **不** 写入 `BUILTIN_COMMAND_NAMES`）
- Modify: `lingxi-code/commands/core/src/lib.rs` / desktop 扩展注册
- Modify: `lingxi-code/tui/src/command.rs` 广告 `/fusion`（否则 CLI 有、TUI 补全没有）
- 移动端：不注册，或 unavailable handler

语法：

```
/fusion [--quality|--fast]
        [--same-provider|--cross-provider]
        [--models profile:model,...]
        [--dimensions evidence_quality,coverage,...]
        [--partial-ok|--no-partial]
        [--max-panel N]
        PROMPT
```

quality/fast 互斥；same/cross 互斥；未传 provider flag 用 `slashCrossProviderDefault`。立即 `Done { display: task id + preset + egress }`，不注入普通 user message。

- [ ] `BUILTIN_COMMAND_NAMES.len()==108` 仍过
- [ ] parser 互斥 / 空 prompt
- [ ] 默认跨 provider；`--same-provider` 同源
- [ ] 立即返回 `f` + 8 base36
- [ ] 完成只追加一条 meta；重放不重复
- [x] 取消：`TaskStop` → `tasks::handlers::local_fusion::cancel_fusion_worker` 触发 `inherit.cancel`，等一小段宽限期收 worker 自己的完成信号，收不到再 hard-abort 兜底（F012）
- [x] read：任务 DTO（`LocalFusionTaskState.stage` + 终态字段）+ 落盘的 sanitized `FusionResult` spool 体
- [ ] **resume：未实现**（落地更新——`TaskType::LocalFusion` 没有会话续跑路径；一个 `/^f[0-9a-z]{8}$/` 任务不能像其它任务类型那样被恢复重放，只能读它已经落盘的终态或重新发起一次 `/fusion`。这是已知残留，不是本节最初勾选清单暗示的"已实现"）
- [ ] `cargo test -p command-core` `cargo test -p tui` `cargo test -p tasks`

**PR5 验收:** `/fusion` 在 enabled=false 时也能跑。

---

### PR6 — Workflow、观测、mobile、文档

#### Task 6.1 workflow `fusion()`

**Files:**
- Modify: `lingxi-code/workflow/src/lib.rs` prelude **追加** `globalThis.fusion`，**不要**改 `__wf_dispatch_batch` 签名或 agent 队列字节

```javascript
globalThis.fusion = (prompt, opts) => new Promise((res, rej) => {
  globalThis.__wf_fusion_queue.push({ prompt: String(prompt), opts: opts || {}, res, rej });
});
```

Native：`__wf_dispatch_fusion(prompt, optsJson)`。未知字段拒绝。

返回紧凑对象（无原始 PanelReport）。enabled=false → 立即拒绝，零 provider 调用。`crossProvider=true` 未允许 → 拒绝，不暗改 false。每 workflow 最多 `workflowFusionCallCap`（硬上限 20）。

- Modify: `lingxi-code/tasks/src/handlers/local_workflow.rs` host 接到 FusionExecutor。
- [ ] 既有 `parallel_*` / agent batch 单测字节级仍过
- [ ] fusion() 基本 / cap / disabled / unknown field
- [ ] `cargo test -p workflow` `cargo test -p tasks local_workflow`

#### Task 6.2 telemetry

`tengu_fusion_{started,panel_started,panel_completed,panel_failed,analysis_completed,analysis_failed,synthesis_completed,synthesis_failed,completed,failed,cancelled}`

禁止记录 prompt / PanelReport / final_text / evidence locator / URL 内容。

#### Task 6.3 mobile + 文档

- engine-mobile 不注入 executor；若有测试调用 → `UnavailableOnPlatform`
- 用户文档（CLI help / settings 注释）：成本、跨 provider 外发、Panel 只读工具边界、NeedsParent
- `docs/architecture-flow.md` Layer 3 加上 `fusion` crate

**PR6 验收:** 三入口齐；默认关闭；相关 crate test + clippy 过。

---

## 7. 进度文案（三入口共用）

```
Resolving models → Reserving budget → Running panels x/N
→ Analyzing reports → Selecting answer | Synthesizing answer
→ Completed | Needs parent | Failed | Cancelled
```

默认 UI 显示匿名 P1..Pn，不显示模型名；详情可显示 egress profile/model。

> **落地更新（F005）：** 本节只定义了文案，没定义谁渲染——落地前 `FusionProgress` 事件确实被发出，但三条入口都没有消费者，用户从 "Resolving models" 到终态之间完全看不到进度。落地后分三条腿，共用同一份 `FusionStage::label()` 文案：
> - **Agent-tool 路径**：`tools/agent` 把每个 `FusionProgress` 转发成既有的 `subagent_activity` 通知（复用父 session 现成的子代理活动展示，不新开一条 UI 通道）。
> - **`/fusion` 任务路径**：`tasks::handlers::local_fusion::run_fusion_worker` 起一个转发协程，把同一串 `FusionProgress` 写进 `LocalFusionTaskState.stage`（task DTO 字段），客户端轮询任务状态时能看到与 Agent-tool 路径相同的阶段文案，而不是"Running"和终态之间的空白。
> - **workflow `fusion()` 路径**：`tasks::handlers::local_workflow.rs` 的 fusion 臂起同样的转发协程，把每个 `FusionProgress` 以 `[workflow_fusion] <FusionStage::label()>` 一行写进 agent() 批次共用的 `worker_progress_tx` 进度通道，`fusion()` 调用在派发到结果之间不再完全静默。

---

## 8. 组合根（engine-desktop）

1. 构 `FusionOrchestrator`（spawner, side_query, budget, resolver, settings）
2. `Arc<dyn FusionExecutor>`
3. `AgentTool::new(ctx).with_fusion(exec.clone())` 替换 `register_all` 的裸 `new`
4. `LocalFusionHandler` + `FusionCompletionSink` 在 orchestrator 可用后注入
5. workflow host 持有同一 exec
6. 注册 `/fusion` handler
7. mobile：跳过 3–6

---

## 9. 测试命令（每个 PR 结束）

```
cargo test -p platform-api fusion::
cargo test -p core settings::
cargo test -p sidequery
cargo test -p llm-client fusion_hints::
cargo test -p fusion
cargo test -p cost budget::
cargo test -p tool-agent
cargo test -p agent fusion_panel
cargo test -p tasks
cargo test -p workflow
cargo test -p command-core
cargo test -p tui
cargo test -p test-harness four_builtin_subagent_types
cargo test -p test-harness -- BUILTIN_COMMAND_NAMES
cargo clippy -p fusion -p tool-agent -p tasks -p workflow -- -D warnings
```

不要 `cargo fmt` 整个 workspace。

并发测试用 barrier / 虚拟时钟 / fake provider，不用 `sleep` 断言时序。

---

## 10. Definition of Done

Codex 验收 1–20 全部成立，并额外：

- 修正后的预留公式 fixture 存在，且「旧 1byte=1token 峰值」不会被重新引入
- `__wf_dispatch_batch` 既有测试仍过
- task id `/^f[0-9a-z]{8}$/`；run_id 与 task id 不混名
- 旧 `AgentToolInput` JSON 无 fusion 字段仍能反序列化
- 真实 provider smoke（同源 + 跨源）若 CI 无凭据，记入发布检查表 Not-tested

---

## 11. 回滚

- `fusion.enabled=false` 关掉 Agent/Workflow
- `/fusion` 可用本地/远端 policy 停注册或让 handler 直接 Disabled
- 历史里已有 fusion-result 当普通 user_meta 显示，不 crash

> **落地更新（F007）：** kill switch **只对新 run 生效**，不是本节最初写的"向运行中 Fusion 发 cancel"——`FusionOrchestrator::run()` 在每次调用开始时才重新 `FusionConfigSource::load()`（见 §5），已经跑在 `run_inner` 里的 run 不会因为设置中途翻转成 `enabled=false` 被主动打断；它按自己已经读到的那份配置继续跑到自然终态（完成 / 各阶段自身超时 / 显式 cancel token）。任何一条终止路径上的资源释放都靠 `Drop`：`fusion::budget::ReservationLease` 没被 `commit()`（成功）就被丢弃时，`Drop` 会 spawn 一次异步 `release_reservation`，是"忘记释放"路径的兜底而不是 kill switch 的机制；已经启动的 Panel 子任务本身的取消仍走 `JoinSet` + `CancellationToken`（`inherit.cancel`），与 `fusion.enabled` 的值无关。真正想打断一个正在跑的 `/fusion` 后台任务要用 `TaskStop`（`tasks::handlers::local_fusion::cancel_fusion_worker`），不是翻转设置。
