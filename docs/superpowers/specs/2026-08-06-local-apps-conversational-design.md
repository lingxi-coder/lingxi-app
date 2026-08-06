# 本地应用生成：从模版向导改为对话式设计

日期：2026-08-06
状态：已批准，待实施
影响范围：`lingxi-code/local-apps`、`lingxi-code/client-protocol`、`lingxi-code/apps/engine-mobile`、`clients/ios`、`skills/create-local-app`

## 1. 背景与目标

### 现状

本地应用（local apps）今天是**模版优先**的：

- `AppTemplateKind`（`local-apps/src/types.rs:22`）固定四个模版：`dashboard`、`crud_tracker`、`content_showcase`、`form_utility`。这个枚举同时存在于持久化记录、wire DTO 和 `client-protocol/tests/version_guard_test.rs` 的冻结清单里。
- 每个模版携带一份服务端固定的目录 `AppTemplateDto.steps`（`client-protocol/src/local_apps.rs:280`），iOS 的 `LocalAppDesignerView.swift:13` 照 `template.orderedSteps` 渲染一个多步表单向导。
- 数据集合由模版硬编码（`local-apps/src/manifest.rs:125`：`Dashboard => records`、`CrudTracker => items`）。
- LLM 在设计阶段只能旁观：通过 `propose_design` 提一个 patch 建议，由用户手动 apply / dismiss。
- **代码生成阶段完全没有 LLM 参与。** `engine-mobile/src/local_apps_generation.rs:1607` 的 `generate_source` 调用同文件 `:1863` 的 `render_app_shell_source(name, template, fields)`，把 app 名、模版 tag 和 design spec JSON 塞进 `:88` 的 `APP_SHELL_TEMPLATE`（一段内联在 Rust 源码里的固定 JSX）。产物永远是同一个 `components/AppShell.jsx`，靠运行时读 spec JSON 变形。
- `GenerationJob.prompt` 字段（`local-apps/src/generation.rs:110`）已存在但无人消费；`AppService::begin_revision`（`service.rs:1286`）未暴露给 agent，因此会话里根本发起不了迭代。

### 目标

改成 Claude Code design 式的交互：

1. 用户用一句话描述想要的 app
2. LLM 针对这次请求现场出一份问卷（chips 单选 / 多选 / `Other…` / 「由你决定」）
3. 用户在设计器页作答，看到一份「将创建」的只读摘要，确认
4. LLM 真的写源码，过校验和构建，出预览
5. 用户在对话或 app 详情页用自然语言持续提要求，反复迭代直到满意

### 已确认的产品决策

| 决策点 | 选择 |
|---|---|
| 澄清问答的界面 | 保留独立设计器页，问题由 LLM 动态生成（不是聊天内联卡片） |
| 提问节奏 | 一次性出完整份问卷；用户的回答不反过来改变题目 |
| 生成后的迭代 | 自然语言修改，设计器不再出现 |
| 数据表结构与权限 | LLM 推导，确认页以只读摘要展示 |
| 旧模版枚举与旧数据 | 彻底删除，不做兼容迁移 |
| 架构方案 | Host 托管全部 LLM 调用（`engine-mobile` 在 `uniffi` feature 下已依赖 `llm-client` + `orchestrator`） |

### 非目标

- 不改动运行时（`manage_runtime`）、数据 API（`query_data` / `mutate_data`）、UI 检视（`inspect_ui` / `act_on_ui`）、checkpoint 机制
- 不改动 `next-static-v1` scaffold 本身与静态导出约束
- 不放宽 `source_validator.rs` 的任何安全策略
- 不涉及 claude-code 2.1.x 对齐（mobile 是已确认的 intentional divergence）

## 2. 架构总览

改造分布在两处，不是一处：

| | 现在 | 目标 |
|---|---|---|
| 出题 | 模版固定目录，四套写死的 steps/fields | LLM 按 brief 现场出一份问卷 |
| 计划 | 无（集合由模版硬编码） | LLM 从答案推导数据表 / 权限 / 域名，确认页展示 |
| 写码 | Rust 字符串替换，零 LLM | LLM 写入 `app/ components/ lib/ styles/ public/`，过 `source_validator` + build |
| 改码 | 不存在 | 自然语言 → LLM 改源码 → 重建 |

保留不动的地基：`AppGenerationExecutor` trait、generation job 状态机、单 worker 串行、`source_validator.rs`（453 行策略校验）、checkpoint 回滚、两道人工确认门。

### 关键取舍：生成从确定性变为概率性

删除 `render_app_shell_source` 后，产物不再必然可运行，改为依赖「模型 + validator + build」三重兜底。第一版可能需要若干轮 revise 才令人满意。这是所选交互形态的固有代价，已明确接受。

## 3. 数据模型与协议

### 3.1 观察：渲染协议本就是通用的

`AppDesignStepDto` / `AppDesignFieldDto` / `AppDesignFieldOptionDto` / `DesignValueDto`（`client-protocol/src/local_apps.rs:228-275`）不含任何模版特定语义——step 有 order/title/fields，field 有 id/label/field_type/required/options。它们唯一的问题是挂在 `AppTemplateDto.steps` 上跟着模版走。

因此本设计**不重写渲染协议**，只把这组类型从「模版目录」改嫁到「每个 app 自己的问卷」。iOS 的 `DesignerStepCard` 渲染逻辑不动。

### 3.2 删除清单

```
local-apps/src/types.rs           AppTemplateKind 枚举（含 as_str / Display impl）
                                  AppRecord.template
                                  AppDesignDraft.template
local-apps/src/manifest.rs        :125 模版→集合硬映射
engine-mobile/local_apps_generation.rs
                                  :88   APP_SHELL_TEMPLATE
                                  :1863 render_app_shell_source
                                        default_collection_fields
client-protocol/src/local_apps.rs AppTemplateKindDto
                                  AppTemplateDto
                                  AppRecordDto.template
                                  list 响应中的 templates: Vec<AppTemplateDto>
client-protocol/tests/version_guard_test.rs
                                  上述所有 put(…) 条目
clients/ios/…/LocalAppsModels.swift        LocalAppTemplate
clients/ios/…/LocalAppsLibraryView.swift   LocalAppCreateSheet 的模版选择器
skills/create-local-app/SKILL.md           模版目录相关的全部指引
```

`AppDataCollectionDto.enabled_by_default` 的 doc comment 提到 "this template"，改写为不引用模版的措辞。

### 3.3 新增与修改的核心类型

`AppDesignDraft` 从「一张字段表」升级为「问卷 + 答案 + 计划」三段，仍持久化在同一个 `workspace/.lingxi/design-spec.json`（一个文件、一把锁、一个 revision 计数器，问卷与答案不会不同步）：

```rust
pub struct AppDesignDraft {
    pub schema_version: u32,
    pub revision: u64,
    /// LLM 出的题。authoring 成功后不可变。
    pub questionnaire: Vec<AppDesignStep>,
    /// 用户答案，沿用现状的 field_id → DesignValue 映射。
    pub fields: BTreeMap<String, DesignValue>,
    /// 确认页展示的「将创建」摘要。
    pub plan: Option<AppPlan>,
    /// plan 是针对哪个 revision 算出来的。
    pub plan_for_revision: Option<u64>,
    pub pending_suggestion: Option<AppDesignSuggestion>,
    pub confirmed_revision: Option<u64>,
}

pub struct AppPlan {
    /// 复用 manifest::DataCollectionSchema。
    pub collections: Vec<DataCollectionSchema>,
    pub capabilities: Vec<AppCapabilityKind>,
    /// 外部 HTTPS 域名。
    pub domains: Vec<String>,
    /// 给用户读的一段人话，必须点明每个 Deferred 字段被定成了什么。
    pub summary: String,
}
```

`AppRecord` 新增 `brief: String` 字段，删除 `template` 字段。

**brief 只存一份，存在 `AppRecord` 上**（`apps/index.json` + `workspace/.lingxi/app.json`），不进 draft。理由：列表页要展示它，出题失败后重试要读它，而 `generate_source` 本来就已经在调 `service.record(&app_id)`（`local_apps_generation.rs:1616`）。存两份必然分叉。

修改 brief 通过新增的 `AppService::update_brief(app_id, brief)`，**仅在 `questionnaire_failed` 与 `collecting_spec` 两个状态下允许**。改 brief 会清空 `questionnaire`、`fields`、`plan` 并重新出题——问卷要重出，旧答案的 field id 已不适用。

**`plan_for_revision` 的作用**：沿用仓库既有的 `AppDesignSuggestion.based_on_revision` 防陈旧机制（见 `types.rs:245` 的 LOAD-BEARING 注释）。`confirm_design` 必须校验 `plan_for_revision == Some(current_revision)`，否则返回 typed `revision_conflict`。用户改了任一答案，计划立即作废，必须重新出计划才能确认。

### 3.4 两个字段级增补

对应设计参考里的两种 chip：

- `AppDesignFieldDto` / `AppDesignField` 新增 `allows_custom: bool` —— 渲染 `Other…` 自由文本框。值直接写进现有的 `SingleChoice(String)`，不需要新类型。
- `AppDesignFieldDto` / `AppDesignField` 新增 `allows_defer: bool`，配套 `DesignValue::Deferred` 新变体 —— 渲染「由你决定」。

`Deferred` 必须是真变体而非「留空」：确认门要能区分「用户还没答」（阻止推进）和「用户明确说你决定」（放行，交由出计划的 LLM 调用定值）。`DesignValueDto` 已标 `#[non_exhaustive]`，加变体是兼容的。

`allows_defer` 为 true 的字段，`required` 语义变为「必须显式作答或显式 Deferred」。

## 4. 三段 LLM 调用

新增 `engine-mobile/src/local_apps_llm.rs`，与 `local_apps_*` 家族一致标注 `#[cfg(feature = "uniffi")]`（`llm-client` 是该 feature 才拉入的依赖）。三个调用均使用结构化输出，**产物一律先过 host 校验再落盘——LLM 的输出是提议，校验器的判定才是事实**。

### 4.1 `author_questionnaire(brief) -> QuestionnaireDraft`

**触发**：`create_app(name, brief)` 落盘后立即入队。

**返回**：`{ suggested_name: String, steps: Vec<AppDesignStep> }`。`suggested_name` 让用户不必在创建前先想名字。

**校验**（任一条不过则整份拒收）：

- `steps.len() <= 5`
- 每个 step 的 `fields.len() <= 8`，总 field 数 `<= 24`
- step id 与 field id 均匹配 `^[a-z][a-z0-9_]{0,39}$`，且 field id 全局唯一
- `field_type` 属于既有白名单
- `single_choice` / `multiple_choice` 的 `options` 非空且 `<= 12`；option `value` 唯一
- `label <= 80` 字符，`description <= 240` 字符，`suggested_name <= 60` 字符
- `default_value` 的 `DesignValue` 变体必须与 `field_type` 匹配

**失败**：落 `questionnaire_failed`，用户可重试或修改 brief 重来。

### 4.2 `plan(brief, questionnaire, answers) -> AppPlan`

**触发**：用户在设计器走完最后一步。

**行为**：所有标记为 `Deferred` 的字段在此定值，并且必须在 `summary` 中如实写明——用户点「由你决定」不等于放弃知情权。

**校验**：

- collection id / field id 匹配 `^[a-z][a-z0-9_]{0,39}$`，collection 数 `<= 8`，每 collection field 数 `<= 24`
- `DataFieldKind::Enum` 必须携带非空 `enum_options`
- `capabilities` 属于 `AppCapabilityKind` 白名单
- 每个 domain 必须是合法主机名、不是 IP 字面量、不是 loopback/私网地址；数量 `<= 8`
- `summary <= 1200` 字符

**失败**：落 `plan_failed`，可重试。

### 4.3 `generate_sources(brief, answers, plan, tree, revision_prompt) -> Vec<FileWrite>`

**触发**：`AppGenerationExecutor::generate_source`，替换 `render_app_shell_source` 调用点。

**输入按 job kind 分**：

- `Initial`：`tree` 为空，`revision_prompt` 为 `None`
- `Revision`：`tree` 为受限根下的现有文件（路径清单 + 内容，受字节预算约束），`revision_prompt` 为用户的自然语言要求
- `Restore`：不调用 LLM，维持现状直接返回 `Ok(())`

**写盘闸门**（逐条 `FileWrite` 独立检查，任一条不过则整批拒绝，不做部分写入）：

- 路径必须落在 `WRITABLE_ROOTS`（`source_validator.rs:17`：`app` / `components` / `lib` / `styles` / `public`）之下
- 拒绝 `..`、绝对路径、符号链接
- 拒绝写 `package.json` 与 npm lockfile
- 文件数 `<= 60`，单文件 `<= 256 KiB`，总计 `<= 4 MiB`（比 `source_validator.rs:14-16` 的 5000 / 4 MiB / 128 MiB 更严，贴合 LLM 实际输出量级）

全批写完后交给现有的 `validate_workspace_source` 做完整策略校验。

**修复循环**：validate 失败时，把 validator 的错误原文回喂 LLM 重新生成，**最多 2 次**（复用已有的 `GenerationJob.attempt` 计数）。仍失败则落 `ValidationFailed`，用户可在对话中用自然语言要求修正后重试。

### 4.4 Prompt 归属

三段 prompt 以 `include_str!` 从 `engine-mobile/assets/prompts/{author_questionnaire,plan,generate_sources}.md` 读入，走 git diff 可审。

**不放进 `SKILL.md`**：本方案的立足点是入口对称——App 库的「+」不经过会话，会话侧 skill 拿不到的东西不能是唯一真相源。

## 5. 状态机与门禁

`AppWorkflowState` 新增四个变体：`AuthoringQuestionnaire`、`QuestionnaireFailed`、`Planning`、`PlanFailed`。

```
create(name?, brief)
      │
      │        ┌──重试 / update_brief──┐
      ▼        ▼                       │
authoring_questionnaire ──失败──▶ questionnaire_failed
      │ 成功
      ▼
 collecting_spec
      │        ┌──重试──┐
      │ 最后一步 │        │
      ▼        ▼        │
   planning ──失败──▶ plan_failed
      │ 成功
      ▼
awaiting_spec_confirmation
      │  confirm_design(interaction_id, revision)     ◀ 人工门 ①
      ▼
 generating → validating → building → starting_preview
      ▼
awaiting_preview_confirmation                         ◀ 人工门 ②
      │ 批准                          │ 提出新要求
      ▼                               ▼
    ready ────────────────────────▶ revising → generating → …
```

要点：

- `ready` **不是终态**。任何时候对该 app 提出要求即回到 `revising`，迭代轮次不设上限，每轮照常存 checkpoint，不满意可回滚。
- 两道人工门一个不少：设计确认（`confirm_design`）、预览批准（`approve_preview`）。
- `collecting_spec` 期间修改任一答案，`plan` 与 `plan_for_revision` 一并清空。
- `questionnaire_failed` 与 `plan_failed` 的「重试」回到各自的执行态（`authoring_questionnaire` / `planning`），不是直接跳到下一步。
- `AuthoringQuestionnaire` 与 `Planning` 期间设计器为只读，避免用户在 LLM 往返途中改答案造成竞态。
- 触发 `planning` 的动作是设计器最后一步的主按钮，文案为「生成方案」而非「下一步」——它会产生一次 LLM 往返，用户需要预期到等待。

**修订链路已经存在，不要重复建设。** 核查代码后确认（本节初稿曾错误地写成需要改 `begin_revision` 签名）：

- `AppService::request_revision(app_id, prompt)`（`service.rs:1312`）与 `AppState::request_revision(prompt, now)`（`state.rs:540`）**已存在且已携带 prompt**，转移是 `awaiting_preview_confirmation | ready → revising`。
- prompt **已经全链路接通**：`request_revision` 入队 `RevisionRequested` continuation → 协调器在 `generation.rs:889-900` 取出 `prompt` 建 job → `generation.rs:558` 传进 `GenerationRequest.prompt`。
- 客户端命令已有（`host.rs:3130` `handle_request_app_revision`），iOS 也已能在预览门里提反馈（`LocalAppDetailView.swift:490` 的 `store.requestRevision`）。
- `begin_revision`（`state.rs:503`）是**另一个**转移（`validation_failed → revising`），不带 prompt 是正确的，不动。

因此本设计在修订链路上只需补两处缺口：

1. **MCP 未暴露** —— 12 个工具里没有修订入口（`local_apps_mcp.rs:171-290`），会话中的 agent 发起不了迭代。
2. **唯一的消费者丢弃了它** —— `generate_source` 渲染模版，从不读 `request.prompt`。这正是「命名了、算出来了、从没接上」的典型。§4.3 接上它。

## 6. MCP 工具表变更

`engine-mobile/src/local_apps_mcp.rs:181` 起：

| 工具 | 变更 |
|---|---|
| `create` | 参数 `template`（四选一枚举）改为 `brief`（string, 1..=2000）；`name` 改为可选 |
| `revise` | **新增**。`(app_id, prompt)` → 调用**已存在的** `AppService::request_revision`（`service.rs:1312`）。服务层与 prompt 链路都是现成的，缺的只是 MCP 这一层暴露——12 个工具里没有任何修订入口 |
| `get` | 响应携带 questionnaire / plan / brief，不再有 template |
| `list` | 响应删除 `templates` 数组 |
| `propose_design` | 不变。其语义（给用户提字段建议、由用户手动 apply）在动态问卷上同样成立 |

其余工具（`manage_runtime`、`query_data`、`mutate_data`、`inspect_ui`、`act_on_ui`、`read_logs`、`list_checkpoints`、`restore_checkpoint`）不变。

`update_brief` **不进 MCP 工具表**，只作为客户端命令暴露。改 brief 会丢弃用户已填的全部答案，属于必须由用户本人在界面上发起的破坏性操作；agent 需要不同的 app 时应当新建一个。

## 7. iOS 客户端

| 文件 | 变更 |
|---|---|
| `LocalAppsLibraryView.swift:375` `LocalAppCreateSheet` | 删除模版选择器，只留一句话描述框。提交后进入等待出题态 |
| `LocalAppDesignerView.swift:13` | steps 来源由 `store.template(app)` 改为 `designer.questionnaire`；渲染逻辑不变 |
| `LocalAppDesignerView` | 新增 `Other…` 与「由你决定」两种 chip |
| `LocalAppDesignerView` | 新增四个中间态界面：出题中、出题失败（重试 / 改描述）、出计划中、出计划失败 |
| 新增确认 sheet | 只读展示 `AppPlan`（数据表 / 权限 / 域名 / summary），出口只有「返回修改」与「确认并生成」。**做成独立 sheet 而非问卷的第 N 步**——它不是问卷的一部分，不应继承步骤条的编辑语义 |
| `LocalAppDetailView.swift:490` | 已有的 `store.requestRevision(appID:feedback:)` 目前只挂在预览门的反馈 sheet 上。改为 `ready` 态也常驻一条底部输入框，复用同一个调用——不是新建能力，是把入口从一次性的门扩展成持续可用 |
| `LocalAppsStore.swift` | 删除 templates 缓存，新增 questionnaire / plan |

会话中提要求与详情页输入框走同一个服务方法、同一条 revision 链——用户可以看着预览直接改，不必来回切页面。

uniffi 绑定需重新生成。删除 `AppTemplateKindDto` 会让 Swift 侧编译报错，这些报错点即为准确的改动清单。

## 8. 错误处理

模版删除后，系统中**不再存在静默降级路径**，这是刻意的。

| 情形 | 处理 |
|---|---|
| 无可用模型 / 离线 / 鉴权失败 | typed `llm_unavailable`；落对应失败态；界面明示「需要联网 / 需要配置模型」 |
| LLM 超时 | 同上，附阶段名 |
| 输出结构不合法 / 超上限 | typed `llm_output_rejected`，携带首个违规项；落对应失败态 |
| 写盘闸门拒绝 | 整批拒绝，不做部分写入；计入修复循环 |
| validate 失败 | 回喂错误重生成，最多 2 次；仍失败落 `ValidationFailed` |
| 旧 `apps/index.json` | 反序列化失败时在 storage 层返回可读错误（明示「此版本不再支持模版时代的 app 记录」），而非 serde 原始报错 |

首次外部域名访问、首次数据 / UI 控制权限授予的 prompt 一律不变。单 worker 串行不变：两个 app 同时生成仍然排队。

## 9. 测试策略

**`local-apps` crate（纯单测，零 LLM 调用）**

- 问卷校验器：每条上限与格式规则各一个拒绝用例 + 一个通过用例
- plan 校验器：同上，重点覆盖 domain 的 IP / loopback / 私网拒绝
- `Deferred` 语义：required + allows_defer 字段留空被拒、显式 Deferred 放行
- `plan_for_revision`：答案变更后确认被拒（typed `revision_conflict`）
- 新状态机完整转移表，含四个新变体的合法与非法转移
- 改 brief 清空答案与问卷

**`engine-mobile`（mock LLM）**

用现有的 `test-harness/src/mocks/mock_http.rs` 打三段调用的 fixture：

- 正常路径（出题 → 出计划 → 写码 → validate 通过）
- 输出超上限被拒
- 非法写盘路径（`..` / 越出 WRITABLE_ROOTS / 符号链接 / 改 package.json）
- validate 失败 → 回喂修复 → 第二次通过
- validate 连续失败 2 次 → 落 `ValidationFailed`
- `Restore` job 不触发任何 LLM 调用

**跑测试必须带 `--all-features`**：local-apps 全家在 `#[cfg(feature = "uniffi")]` 门控内，`cargo test --workspace` 编译不到其中任何一个测试，绿了不说明任何问题。

**协议与客户端**

- `version_guard_test.rs`：删除模版相关条目，新增 `AppDesignFieldDto.allows_custom` / `allows_defer`、`DesignValueDto::Deferred`、`AppPlan`、`AppRecordDto.brief` 等条目
- `clients/ios/Tests/LocalAppsStoreTests.swift`：更新为问卷驱动
- 设计器渲染的 snapshot 测试覆盖 `Other…` 与「由你决定」两种 chip

## 10. 已知代价

1. **旧数据不可用。** 模版时代的 `apps/index.json` 会反序列化失败。已确认接受，CHANGELOG 标注 breaking。
2. **生成结果不再确定。** 详见 §2 的取舍说明。
3. **首次创建多一次 LLM 往返。** 用户输入描述后需等待出题；走完问卷后需等待出计划。两处都需要明确的加载态与可取消入口。
