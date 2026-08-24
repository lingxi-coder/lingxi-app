# 对话式创建本地应用（create-app conversational flow）

> 取代 2026-08-22 落地的「create-first 表单」方案。旧的两步表单（一句话简介 → 引擎提议名称+形态 → 确认后创建）整体删除，**不考虑向后兼容**。
>
> 本文经过一轮对抗式核验（116 条断言，31 条被证伪并已在此修正）。核验记录见 §I。

## Context

上一版把创建做成了一个两步原生表单：用户写一句话简介，引擎用一次无头 LLM 调用提议名称与形态，用户确认后应用被创建，再打开它自己的会话。它解决了「会话根目录在 `ConversationSource` 构造时就绑定、活的会话无法重新扎根」这个约束，但代价是把需求收集塞进了一个填空框。

真机上暴露的问题不是表单不好看：应用创建完进入会话后，代理会重新搭脚手架、会跳过需求确认直接开工。上一版只能靠在 kickoff 文案里写「不要再创建应用，也不要重新搭建脚手架」来压制。

**核验推翻了对这个现象的归因。** 见 §0——那份写在工作区里、本该每轮约束代理的合约，在 iOS 上从来没有到达模型。代理不是无视合约，是没看见合约。

本方案把整个创建过程移进对话：点「创建应用」立刻得到一个**空壳**应用，会话从第一轮起就扎在它自己的工作区里，代理一步步问清需求、提议名称与形态、经用户确认后才落地脚手架。顺序由**工具层**保证，不靠提示词自觉。

---

## 0. 前置修复（阻塞项）：工作区 `LINGXI.md` 在移动端从未加载

**这是本方案的先决条件，也是一个当前在线的缺陷。必须先修、先有测试，再做后面任何一步。**

### 事实链

1. `apps/engine-mobile/src/host.rs:3291` 起：`model_cwd` 取自 `workspace_mount.guest_path`（GUEST 坐标）。紧邻的注释写着「The engine-internal cwd (`cwd` — transcripts, .lingxi, **memory files**) stays host」——**意图是 host**。
2. `host.rs:3329`：`SessionCwd::new(model_cwd, trusted_dirs)`。于是 `session_cwd` 实际持有的是 **guest** 路径，与上面那句注释相反。
3. `orchestrator/src/conversation.rs:12052-12076`（`build_system_prompt`）走了 PathAtlas S3 的 `prompt_probe_cwd_resolver` 做 guest→host 转换，`memory.load(&probe_cwd)` 是**对的**。
4. `conversation.rs:12270`（`additional_context_message`）**没走**：`self.memory.load(&self.session_cwd.cwd())`，读的是原始 guest 路径。
5. `orchestrator/src/prompt/mod.rs:153`：「R-P1c/R-P1d: the LINGXI.md memory block is NO LONGER spliced into the system prompt」——**`additional_context_message` 是唯一的渲染路径**，第 3 步那次加载不产出内容。
6. iOS 确实装了 provider（`apps/ios-framework/src/lib.rs:503`、`:735` = `orchestrator::prompt::real_provider()`），所以问题不是「移动端没有 memory provider」，纯粹是坐标。

⇒ `apps/<id>/workspace/LINGXI.md` 在 iOS 上从未进入模型上下文。`scaffold_app_value` 写的那整份工作区合约一直空转。

### 修复

把 `additional_context_message` 的 `memory.load` 也经过 `prompt_probe_cwd_resolver`——与 `build_system_prompt` 同一条 guest→host 跳转。桌面端没有该 resolver，`probe_cwd == cwd`，字节不变。

### 测试（缺一不可）

- 一条**钉住坐标**的 orchestrator 测试：session cwd 为 guest 路径、mount 表把它映射到一个真实 host 目录、该目录下放一个 `LINGXI.md`，断言其内容出现在 `additional_context_message` 的产物里。**这条测试必须先对着未修复的代码跑成红色**，否则它测的不是这个 bug。
- 一条桌面端回归：无 resolver 时行为字节不变。

⚠️ 这条修复独立于本方案的其余部分，且价值更高（它同时修好正式版合约）。**可以先单独落地并单独真机验证。**

---

## 已锁定的决策

| 决策 | 选择 |
|---|---|
| 空壳创建时机 | 点击「创建应用」的那一刻，记录与工作区目录立即真实存在 |
| 会话根目录 | 从第一轮起就扎在应用工作区，**不引入任何会话重新扎根机制** |
| 未完成应用 | 在应用库里显示为「草稿·创建中」，点进去续上同一个对话，可随时删除 |
| 「基础模版」 | 就是 `dom` / `canvas` 两种形态，**不重新引入模版目录** |
| 顺序保证 | 工具门（事实），提示词只负责措辞与节奏 |
| 空壳判据 | `AppRecord.scaffolded == false`（**新增持久字段**，见 §A——这是核验后相对最初方案的改动） |
| 协议版本 | **8.0.0**（删除命令/事件变体是破坏性改动），re-bless `blessed_major.txt` |

## 非目标

- 不做会话 cwd / Linux 挂载点 / session catalog 的热切换。空壳先行正是为了绕开它。
- 不重新引入模版目录或起手样板。
- 不改 `LocalAppCreate` 工具（普通聊天里代理主动建应用那条路）的既有语义：它继续「创建 + 脚手架」一步到位。两条路共用同一个 `scaffold_app_value` 实现。
- 不做空壳的自动清理。
- 不迁移历史遗留应用（`surface == None` 且已有脚手架的老应用）。它们必须继续按今天的方式被识别，见 §A.1。

---

## A. 状态模型：`AppRecord` 新增一个持久字段

### A.1 为什么不能用 `manifest.surface == None`

最初的设计想用 `manifest.surface == None` 当「还没成形」的判据。**核验证伪了它**：这个值已经有第二批活的持有者——**上一版脚手架拆分之前创建的历史应用**。

- `local-apps/src/manifest.rs:277` 明写：「`None` means the app predates the scaffold split and cannot be rebuilt」
- `local_apps_build.rs:206` 的 `detect_build_target` 整段存在的理由就是对它报「this app was created with a scaffold that has been removed… create a new app to continue development」
- `APPS_SCHEMA_VERSION` 仍是 `1`，没有任何迁移回填 `surface`；`surface` 只有两个写入点（`scaffold_app_value`、`scaffold_workspace`）

若用裸判据，每一个历史应用都会：在库里被改标成「新应用 · 创建中」（真名与简介被隐藏、点击跳去 pin 会话而不是预览，**一个还能跑的应用变得打不开**）；19 个工具全被拒绝；更糟的是 `LocalAppScaffold` 会**接受**它，`scaffold_app_value` 盖上 surface、把新脚手架写到老源码上——正是 `detect_build_target` 的报错文案存在的目的所要防止的那种损坏。

### A.2 判据：`AppRecord.scaffolded: bool`

在 `local-apps/src/types.rs` 的 `AppRecord` 上新增：

```rust
/// 工作区里是否已经落下脚手架。
/// serde 默认为 `true`：历史记录（含脚手架已移除的老应用）一律按「已成形」加载，
/// 不会被误判成刚点出来的空壳。只有 `CreateApp{surface: None}` 写 `false`，
/// 只有 `LocalAppScaffold` 把它翻成 `true`。
#[serde(default = "default_scaffolded")]
pub scaffolded: bool,
```

选它而不是「manifest.surface + 工作区探测」的三条理由：

1. **语义正确**：`surface == None` 是两种含义的并集，`scaffolded` 只有一种。
2. **零额外 IO**：`surface` 只存在于 `AppManifest`（`manifest.rs:287`），**不在 `AppRecord` 上**。要把它送上列表行，`lower_record`（`local_apps_bridge.rs:251`）就得对**每个应用**多做一次 `load_manifest`（读文件 + 反序列化 + validate，无缓存），而 `AppsChanged` 在每次应用变更时都会发。`scaffolded` 随记录本身来，一次都不多读。
3. **客户端可判**：客户端没有文件系统，无法执行「`surface == None` 且工作区没有 `package.json`」这种复合判据；一个布尔字段可以直接下发。

代价（明确接受）：这是一个持久记录的 schema 变更，牵连 `local-apps/tests/serde_compat.rs` 与 `local-apps/tests/fixtures/v1/apps/*/app.json`。必须有一条**从旧 fixture 加载**的测试，断言它得到 `scaffolded == true`。

⚠️ 这一条是核验后相对最初方案的**设计改动**：最初承诺「不新增任何状态字段」，做不到。

四种样子（第四行是本方案必须不打扰的那一批）：

| | `record.scaffolded` | `manifest.surface` | 工作区 | 库里显示 |
|---|---|---|---|---|
| 空壳 | `false` | `None` | 只有 `.lingxi/` 与引导版 `LINGXI.md` | 草稿 · 创建中 |
| 已成形未构建 | `true` | `Some(_)` | 脚手架已落地 | 草稿 |
| 可运行 | `true` | `Some(_)` | 有 `build/store/dist/` | 就绪 |
| **历史遗留** | `true`（serde 默认） | `None` | 老脚手架 | 照旧，**不受本方案影响** |

后两列的 `Draft`/`Ready` 就是今天的 `AppWorkflowState`（`types.rs:27`），不动。

### A.3 必须下移的一条旧不变量

`create_app_with_git_and_workflow_model_and_initializer`（`local-apps/src/service.rs:911`）目前硬性拒绝空 brief。那条规则的理由是「问卷要从 brief 生成」——**问卷流水线已被删除，理由过期了**。

处置：**下移，不是删除**。创建时允许空 brief；`LocalAppScaffold` 时必须非空（`trim` 后）。守的还是同一件事——`LINGXI.md` 里不会出现空简介。

⚠️ 两条现存测试直接钉着旧不变量，会立刻转红，**必须一起改**：
- `service.rs:2091` `create_app_enforces_brief_caps`（首个断言是 `create_app(Some("A"), " ", None)` → `InvalidRequest`）→ 保留 `MAX_BRIEF_BYTES` 那一半，把空串那一半改成「现在被接受」
- `service.rs:2170` `create_app_rejects_an_empty_brief` → 改写成「空 brief 产生一个 `scaffolded == false`、名字为占位常量的空壳」

### A.4 占位名，以及它会泄漏到哪里

创建空壳时服务层 `name` 传 `None`、brief 为空，`service.rs:919` 的派生（`brief.chars().take(24)`）得到空串，需要回落到固定的**非本地化**占位常量 `"untitled"`。

⚠️ **核验证伪了「客户端永远不显示它」**。除库卡片外，至少还有五处无条件渲染 `name`/`brief`，全部必须按 `scaffolded == false` 分支：

| 位置 | 现状 |
|---|---|
| `clients/ios/.../LocalAppsDrawerSection.swift:33` | `Text(app.name)` |
| `clients/ios/.../LocalAppDetailView.swift:306` | `LabeledContent("local_apps_brief", value: app.brief)`（空串；而 §D.4 恰恰把用户往这页引） |
| `clients/android/.../LocalAppsScreen.kt:681` | `LocalAppCard` 同时渲染 `name` 与 `brief` |
| `clients/ios/.../LocalAppsStore.swift:891` | widget 标签 `summary.name.isEmpty ? summary.brief : summary.name` |
| 库卡片 | §D.3 |

⚠️ 还有一处**逃出客户端**：`host.rs:9145` / `:9172` 用 `record.name` 作为 pin 住的 init 会话**标题**（`create_branch_to_cwd(..., Some(&record.name), ..)` / `append_mobile_empty_session(&init_id, &record.name)`），所以 `"untitled"` 会写进持久化的会话目录。

处置：`LocalAppScaffold` 在写回名称后**一并重命名该 pin 会话**。加一条测试。

---

## B. 协议

### B.1 `CreateApp`：结构不动，翻转 `surface` 的语义

`client-protocol/src/commands.rs` 的 `CreateApp` 结构不动。改的是 `surface` 字段的含义：

| `surface` | 新语义 | 谁在用 |
|---|---|---|
| `Some(dom \| canvas)` | 创建记录**并**脚手架（今天的行为） | `LocalAppCreate` 工具那条路、测试 |
| `None` | **只创建空壳**（`scaffolded: false`），不脚手架 | 新的原生「创建应用」按钮 |

今天 `None` 的含义是「调用方没表态，宿主挑路由默认值然后照样脚手架」。翻转它只影响移动客户端，而移动客户端本次本来就要重写。**零 wire 变更。**

代价（明确接受）：这个字段从此有两种含义。字段文档必须写清，否则下一个读者会以为 `None` 仍等于「默认 dom」。

⚠️ **核验修正的两处 payload 事实**：
- `commands.rs:366` 的 `name: String` 是**必填非可选**的 wire 字段。客户端发的是 `name: ""`（空串）**不是 nil**；「传 `None`」只在服务层成立（`create_app_with_git_and_workflow_model_and_initializer(name: Option<&str>)`）。宿主需要一条「wire 空串 → 服务层 `None`」的分支。
- `commands.rs:368` 的 `origin: AppCreateOriginDto` 也是必填，最初的 payload 清单漏了。按钮路径发 `.library`。

### B.2 列表行 DTO 追加 `scaffolded`

⚠️ **核验证伪了最初写的 `AppSummaryDto`——仓库里没有这个类型。** 列表行是 `AppRecordDto`（`client-protocol/src/local_apps.rs:231`），由 `lower_record`（`local_apps_bridge.rs:251`）从 `&AppRecord` 直接映射；iOS 侧的视图模型叫 `LocalAppSummary`（`LocalAppsModels.swift:25`，由 `LocalAppsProtocolAdapter.app(_:)` 从 `AppRecordDto` 构造）。

变更：`AppRecordDto` 追加 `scaffolded: bool`。因为 §A.2 把它放在了 `AppRecord` 上，`lower_record` 是**纯映射补齐**，没有额外 IO。

⚠️ uniffi 生成绑定按**位置**编码结构体字段，必须**追加在最后**（同样的位置风险已在 `local_apps.rs:907` 对 `AppEventDto` 的变体顺序记录过）。

⚠️ `client-protocol/tests/version_guard_test.rs` 的 `current_contract_index()` 是一张**手写的字符串字面量表**（`AppRecordDto.*` 在 `:1285-1293` 附近），不更新它，守卫**根本看不见**这次改动。同文件 `:2133/:2204/:2302` 还有三处 `AppRecordDto` 结构体字面量要补字段。

### B.3 删除提议命令/事件 ⇒ 8.0.0

删除 `ClientCommand::ProposeAppIdentity` 与 `ClientEvent::AppIdentityProposed`。删除枚举变体是破坏性改动（governing decision §0.10）。

必须同批改动的文件（⚠️ **最初漏了后四项，其中第一项是守卫本身**）：

- `client-protocol/src/version.rs` → `"8.0.0"`
- `client-protocol/tests/version_test.rs`
- `client-protocol/snapshots/contract_index.json`（重新生成）
- `client-protocol/snapshots/blessed_major.txt` → `8`
- ⚠️ `client-protocol/tests/version_guard_test.rs` — 手写表里 `put("ClientEvent::AppIdentityProposed"…)`（`:337-340`）与 `put("ClientCommand::ProposeAppIdentity"…)`（`:599`）共 7 行。**删除变体后代码照样编过**，覆盖锚点 `contract_index_covers_every_dto` 只是抽样，不构造这两个变体——不删这几行，守卫看不见删除，也就不会强制 bump
- ⚠️ `client-protocol/tests/snapshot_test.rs`
- ⚠️ 删除 `client-protocol/snapshots/command/propose_app_identity.json` 与 `snapshots/event/app_identity_proposed.json`
- ⚠️ `clients/shared/src/protocol.ts` — 版本常量**以及两个 union 成员**

**替代方案（已否决）**：把死变体留在协议里、停在 7.x。否决理由：死表面会烂在原地，而本次明确不考虑向后兼容。

---

## C. 引擎

### C.1 新工具 `LocalAppScaffold`

入参：`{app_id, name, brief, surface, workflow_model?}`，`additionalProperties: false`。

⛔ **命名禁区**：`local_apps_mcp.rs` 有一条断言对**全部工具 schema 的拼接串**做小写子串检查，禁止出现 `template`；`local_app_template_removal_guard_test.rs` 另有四个禁用符号。工具名、参数名、枚举值、description 一律避开。

行为：
1. 校验目标应用存在且 `record.scaffolded == false`；已成形则拒绝，复用现成文案「an app's surface is fixed when the app is created and cannot be changed」。
2. 校验 `brief.trim()` 非空（§A.3）、`name` 非空、长度在既有 `MAX_NAME_BYTES` / `MAX_BRIEF_BYTES` 内。
3. 写回 `AppRecord` 的 `name` / `brief` / `workflow_model`，并置 `scaffolded = true`。
4. 调**已有的** `scaffold_app_value(&record, surface)`（`local_apps_host.rs:3110`）。它本身先盖 `manifest.surface` 再写文件，崩溃安全性已论证过，不动。
5. 重命名 pin 住的 init 会话（§A.4）。
6. 返回工作区合约摘要，提示代理下一步读 `LINGXI.md`。

⚠️ **核验修正**：最初只说 `workflow_model` 没有更新路径。实际上 **`name` 与 `brief` 也没有**——`record.name =` / `record.brief =` 在 `AppState::create_with_git` 之外一处都不存在（`update_manifest` 改的是 manifest 的显示名，不是 `AppRecord.name`）。所以需要的是一条**把四个字段一起写回、持久化并发事件**的服务方法。`git_enabled` 不开放修改。

**权限**：`permission/src/defaults_per_tool.rs` 设 `AllowByDefault`。

⚠️ **不够**。`local_apps_tools.rs:187` 还有一张手写子集 `requires_bound_session_for_auto_allow`，它把 allow-by-default 的工具在 `bound_app_id(ctx)` 为 `None` 时降级为 `Ask`。builtin 对每个会话都注册，所以**全局会话里也能看到 `LocalAppScaffold`**。必须把它加进这张表：扎在该应用工作区的会话免弹框，全局会话仍然要问。

⚠️ 另有两处硬同步点：
- `local_apps_tools.rs` 的 `LOCAL_APP_TOOLS` 表
- `local_apps_mcp.rs:2057` 的 `catalog_is_fixed_and_exposes_no_arbitrary_execution_surface` —— 一条 **22 个名字的 `assert_eq!` 精确列表**，新增任何 provider operation 都会撞红

### C.2 工具门

位置：`local_apps_mcp.rs` 的 `call()`（`:1146`）**最顶端**，紧随 `validate_input` 之后、**在 `:1151` 的 `parse_dynamic_tool` 分支之前**。

⚠️ **核验推翻了「照 `runtime_api_compatible()` 写」这条指示。** 那个检查在 `:1173`，位于动态分支**内部**（分支 `:1152` 开、`:1192` 关），只覆盖应用自有 MCP 命名空间；静态 `match tool` 那条路**根本没有 runtime-api 检查**。仓库里**不存在**一个同时覆盖两条分支的先例。门必须自己写在 `:1151` 之上，app_id 来源分两种：`parse_dynamic_tool(tool)` 命中时取绑定值，否则取 `input["app_id"]`。

规则：目标应用 `record.scaffolded == false` 且工具不在放行名单内 → `tool_error`：

> 应用 `<id>` 还没有形态。先与用户确认要做什么，再用 `LocalAppScaffold` 定下名称、简介与形态。

⚠️ **门必须按工具名判定，不能按「入参里有没有 `app_id`」判定。** 核验发现：`create` 的 schema（`local_apps_mcp.rs:797`）确实没有 `app_id`，但 `LocalAppTool::call`（`local_apps_tools.rs:367`）在分发**之前**会从会话 cwd 注入 `app_id`。所以「没有 app_id 就天然不受门管」是错的，最初据此算出的 22/19 也随之作废。

放行名单（按名字）：`LocalAppScaffold`、`LocalAppList`、`LocalAppGet`、`LocalAppCreate`。

`LocalAppCreate` 放行是**有意的**：在一个空壳会话里代理若真要另建一个应用，这不是本门要防的错误；本门防的是「在空工作区上构建/安装/跑运行时」。`LOCAL_APP_TOOLS` 共 22 条，受门管的是 **18** 条。

### C.3 两份 `LINGXI.md`

**先决条件：§0 必须已经修好。** 否则本节写的任何文件都到不了模型手里。

**引导版**——在 `CreateApp{surface: None}` 的 initializer 里写入 `workspace/LINGXI.md`。此时 `layout.initialize()` 已跑过，工作区目录存在。

内容要点（正式版会整体覆盖）：
- 这个应用刚创建，**还没有形态**，工作区是空的。
- 你现在的任务是引导用户，不是写代码。
- 你现在只有 `AskUserQuestion` 和 `LocalAppScaffold` 可用；其余本地应用工具会拒绝你并告诉你原因。
- 步骤：先问用户想做什么 → 据回答推断意图，用 `AskUserQuestion` 把提议的**名称**与**形态**（`dom` 多屏界面 / `canvas` 单一绘制面：游戏、3D、可视化）交给用户确认或修改 → 确认后调 `LocalAppScaffold` → 重读本文件，按新合约继续。
- 形态一旦落地不可更改，所以必须让用户确认，不要自作主张。

**正式版**——`scaffold_app_value` 现有的两套合约文本（dom / canvas），不动。它插值 `record.name` 与 `record.brief`，此刻已由 `LocalAppScaffold` 写成真值。

⚠️ **核验修正了「代理改不了它」的理由**（结论对，理由错，且那个错理由会误导后来人）：该文件位于 `apps/<id>/workspace/LINGXI.md`，**就在可写根之内**（`host.rs:3282` 把挂载 host 路径加进 `trusted_dirs`，工作区正是唯一可写的 `LocalAppBuild` 挂载）。真正拦住写入的是 `permission/src/workspace_lease.rs:669` 的 host-owned 拒绝名单里有 `LINGXI.md`。做 §C.2 时若碰了那条规则，就等于把合约的编辑权交给了代理。

### C.4 `detect_build_target` 的错误分支

`detect_build_target` 目前在 `manifest.surface == None` 时统一返回「此应用由已移除的脚手架创建，请新建一个应用」。对空壳来说这句话是主动误导。

新判据（**以 `record.scaffolded` 分家，不是嗅探 `package.json`**——§A.2 已把这个区分做成持久事实，再探一次文件系统是第二套推导）：

| 条件 | 结论 |
|---|---|
| `manifest.surface == Some(_)` | 对应目标 |
| `scaffolded == false` | 空壳 → 「这个应用还没有形态，先用 `LocalAppScaffold` 定下形态」 |
| `scaffolded == true` 且 `surface == None` | 历史遗留 → 保持现有的「请新建应用」 |
| 有 `next.config.mjs` | 保持现有的 legacy 拒绝 |

⚠️ 这不是「门已经挡住了」就能省的：`detect_build_target` 还有不经过 MCP 的调用点。实现时**逐个数清并各自加测试**。

### C.5 删除

- `handle_propose_app_identity` / `propose_app_identity`（`apps/engine-mobile/src/host.rs`）
- `APP_IDENTITY_SYSTEM_PROMPT`、`parse_app_identity`、`fallback_app_name`
- 它们的全部测试

---

## D. 客户端

### D.1 入口：删掉整个表单

「+」按钮不再 push 创建路由，直接三步：
1. 发 `CreateApp{brief: "", name: "", origin: .library, surface: nil, git_enabled: 默认, workflow_model: nil, conversation_id: 当前会话}`
2. 等 `AppRecordChanged` 带回 pin，再打开会话
   （⚠️ `AppCreated` 永远不带 `init_session_id`——它在创建事务内发出，pin 之后才铸。这个坑上一版已解决。）
3. 打开 `.localApp(appID)` 会话并自动发 kickoff

⚠️ **核验修正**：「机制原样复用」只对**等 pin** 那一半成立。**布防那一半不能复用**——`landingAwaitingPin` 今天由 brief 认领布防（`LocalAppsStore.swift:877` `guard let claimed = creationBrief, claimed == record.brief`；Android `LocalAppsViewModel.kt:806`），而 §D.5 要删掉 `creationBrief`。引擎对**两条创建路径**都发 `AppCreated`，所以删掉认领后布防就没有关联键：代理在别处调 `LocalAppCreate` 会把用户劫持进另一个应用的会话。

替代关联键（实现时二选一并写进测试）：「+」点击置位的一次性布尔（首个未被其它路径认领的 `AppCreated` 消费掉），或在 `CreateApp` 上带一个客户端生成的 request id。**后者更稳，但要动 wire**——既然已经是 8.0.0，代价很低。

### D.2 kickoff 文案

换成不带占位符的一句话，用户视角：「我想做一个新的本地应用。」

⚠️ **i18n 必须改 `clients/translations/*.json`，生成产物碰都不碰**（iOS 的 `Localizable.xcstrings`、Android 的 `values*/strings.xml` 都是 `generate.py` 的产物，手改下次全还原）。新文案不带占位符，iOS 与 Android **共用一个 key** 即可。

同批删除：`local_apps_init_kickoff %@` 与 Android 孪生 `local_apps_init_kickoff`，以及创建表单的全部文案 key。⚠️ Android 另有一份**不由生成器管理**的 `values*/strings_local_apps_v3.xml`，要一并核对。

### D.3 草稿态

`AppRecordDto.scaffolded == false` 的卡片：标题渲染本地化的「新应用」、副标题「创建中」，**不显示** `name`/`brief`；点击进 pin 会话而非预览；删除照旧。

§A.4 列出的另外四处渲染点用同一判据分支。

### D.4 Widget 入口搬家

⚠️ 「创建后添加到主屏幕 Widget」目前**只能从创建表单进入**：`pendingWidgetSetup` 只有一个置位点，就在创建落地路径上。删掉表单等于删掉这个功能。

处置：挪到应用详情页（`clients/ios/Sources/LocalApps/LocalAppDetailView.swift`，⚠️ 该文件今天 `grep -i widget` **零命中**，是净新增 UI）。

### D.5 删除

- iOS：`LocalAppCreateView`（⚠️ 它**不是独立文件**，声明在 `LocalAppsLibraryView.swift` 内）；`AppIdentityProposal`、`proposeIdentity`、`identityProposalAnswers`、`creationBrief`
- Android：⚠️ 组件名是 **`CreateAppDialog`**（`LocalAppsScreen.kt:303`，`:270` 处调用），不是 `CreateAppSheet`；连同 `onProposeIdentity` 与 `LocalAppsViewModel` 对应状态

---

## E. 需要改动的文件

### 前置（§0）
- `lingxi-code/orchestrator/src/conversation.rs` — `additional_context_message` 的 memory 加载走 probe resolver
- 对应的 orchestrator 测试文件

### 协议
- `client-protocol/src/commands.rs`、`src/events.rs`、`src/local_apps.rs`、`src/version.rs`
- `client-protocol/tests/version_test.rs`、⚠️ `tests/version_guard_test.rs`、⚠️ `tests/snapshot_test.rs`
- `client-protocol/snapshots/contract_index.json`、`snapshots/blessed_major.txt`
- ⚠️ 删除 `snapshots/command/propose_app_identity.json`、`snapshots/event/app_identity_proposed.json`
- `clients/shared/src/protocol.ts`（版本 + 两个 union 成员）、`clients/shared/test/snapshots.test.ts`

### 引擎
- `local-apps/src/types.rs` — `AppRecord.scaffolded`
- ⚠️ `local-apps/tests/serde_compat.rs` + `local-apps/tests/fixtures/v1/apps/*/app.json`
- `local-apps/src/service.rs` — 允许空 brief；占位名；写回 name/brief/workflow_model/scaffolded 的方法
- `apps/engine-mobile/src/local_apps_mcp.rs` — `LocalAppScaffold` schema + dispatch；`call()` 顶端的门；⚠️ `:2057` 的 22 名精确列表
- `apps/engine-mobile/src/local_apps_tools.rs` — `LOCAL_APP_TOOLS`；⚠️ `requires_bound_session_for_auto_allow`
- `permission/src/defaults_per_tool.rs`
- `apps/engine-mobile/src/local_apps_host.rs` — `LocalAppScaffold` 实现；引导版 `LINGXI.md`
- ⚠️ `apps/engine-mobile/src/local_apps_bridge.rs` — `lower_record` 补 `scaffolded`（`lower_manifest` **在这个文件**，不在 `local_apps_host.rs`；但它构造的 `AppManifestDto` 只经 `GetAppDetails` 到达，**不在列表流上**，本方案不用它）
- `apps/engine-mobile/src/host.rs` — `CreateApp` 分叉；wire 空 `name` → 服务层 `None`；pin 会话重命名；删提议命令
- `apps/engine-mobile/src/local_apps_build.rs` — `detect_build_target` 分支

### 技能与文档（⚠️ 最初整段遗漏）
- ⚠️ `skills/create-local-app/SKILL.md` — **被 `include_str!` 编进引擎**（`skill-api/src/builtin/bundled.rs:31`，注册于 `builtin/mod.rs:81`），其「Entry and confirmation」段落逐字写着旧契约
- ⚠️ `skills/create-local-app/agents/openai.yaml` — `default_prompt` 也调用该技能
- ⚠️ `docs/local-apps/HANDOFF.md` — `:11`、`:45`、`:95` 三处陈述的正是本方案反转的不变量

### 客户端
- `clients/ios/Sources/LocalApps/` — `LocalAppsLibraryView.swift`、`LocalAppsStore.swift`、`LocalAppsModels.swift`、`LocalAppsProtocolAdapter.swift`、⚠️ `LocalAppDetailView.swift`、⚠️ `LocalAppsDrawerSection.swift`
- `clients/ios/Sources/App/RootView.swift`
- `clients/android/.../localapps/` — `LocalAppsScreen.kt`、`LocalAppsViewModel.kt`、`LocalAppsContract.kt`；`RootScreen.kt`
- `clients/translations/*.json`（5 语言）+ 跑 `generate.py`
- ⚠️ 测试：`clients/ios/Tests/LocalAppsStoreTests.swift`、`clients/android/.../LocalAppsViewModelTest.kt` 等

---

## F. 测试

### 前置（§0）
见 §0「测试」。**那条坐标测试必须先见红。**

### Rust
- **旧 fixture 加载** → `scaffolded == true`（历史应用不被误判成空壳）
- **空壳创建**：`scaffolded == false`；工作区只有 `.lingxi/` 与 `LINGXI.md`；引导版合约命中关键指令；空 brief 被接受
- **门**：**表驱动**断言那 18 个工具在空壳上全部拒绝且文案指向 `LocalAppScaffold`；放行的四个能过。⛔ 不要写 18 个函数
- **门的两条路径**：静态 `match` 与 `parse_dynamic_tool` 各一条用例。只测一条等于没测到 §C.2 的要害
- **`LocalAppScaffold`**：写回四个字段；正式合约覆盖引导版；pin 会话被重命名；二次调用被拒；空 brief 被拒；未知 surface 被拒；**对历史遗留应用（`scaffolded == true`、`surface == None`）必须拒绝**
- **`detect_build_target`**：空壳 / 历史遗留 **各自独立断言**。共用一条断言等于没测
- **改写**：`create_app_enforces_brief_caps`、`create_app_rejects_an_empty_brief`（§A.3）

⚠️ 跑法：`cargo test --workspace --all-features --no-fail-fast`。engine-mobile 的 local-apps 模块是 `#[cfg(feature = "uniffi")]`，不加 `--all-features` 整块被跳过。全量输出落文件再 grep（只 grep `FAILED` 会丢掉 `failures:` 块里的测试名），并盯**测试总数**是否下降。

### 客户端
⚠️ **核验修正**：brief 认领机制**纯粹在客户端**，那批测试不属于 Rust 段——它们在 `clients/ios/Tests/LocalAppsStoreTests.swift`（`:1571-1656`）与 `clients/android/.../LocalAppsViewModelTest.kt`（`:302-528`），**每端各约 9-12 条**，不是总共 9 条。

另需：草稿卡片渲染、`name`/`brief` 五处渲染点不泄漏占位串、「+」不再弹表单、kickoff 文案、widget 常驻入口、新的落地关联键（§D.1）。

⚠️ 改了 wire 之后**必须先重新生成 uniffi 绑定再编客户端**（绑定是 gitignored 构建产物）：
- `bash clients/ios/scripts/build-xcframework.sh && (cd clients/ios && xcodegen generate)`
- `bash clients/android/scripts/build-jni.sh --variant play`

## G. 真机验收

**不接受「编过了」。**

**G.0（§0 单独验收，先做）**：在一个**已有**的本地应用会话里，确认代理确实读到了工作区合约——例如让它复述 `LINGXI.md` 里只在该文件出现过的一条约束。修复前它应当答不出来。

1. 点「+」→ **不弹任何表单**，直接进对话，代理第一句在问你想做什么
2. 回答「打飞机」→ 代理提议名称，并用**原生选项**让你确认形态，且它选的是 `canvas`
3. 确认后应用成形、草稿标记消失、一路能构建出可玩的东西
4. 反向用例：第 1 步就退出 → 库里留一张「创建中」卡片，点回去续上**同一个**对话
5. **历史遗留应用回归**：打开一个 `surface == None` 的老应用，确认它**没有**被标成「创建中」、工具没有被门拒绝、`LocalAppBuild` 仍给「请新建应用」

## H. 风险与已知留口

- **门只保证顺序，保证不了「代理真的问过用户」。** 它可能问完第一句就自作主张调 `LocalAppScaffold`。只能靠提示词，验收 G.2 就是在验它。若真机反复不过，下一步是让 `LocalAppScaffold` 要求一个「用户已确认」的证据字段——但那同样可被编造，本期不做。
- **空壳会堆积**，没有自动清理，靠用户删。
- **`CreateApp.surface` 一个字段两种含义**。靠文档与测试压制，不引入第二个命令。
- **`AppRecord` 多了一个持久字段**，相对最初「不新增状态」的承诺是一次让步。理由见 §A.2。
- ⚠️ **8.0.0 的破坏面被核验缩小了**：只有**桌面端**（Electron / `clients/shared`）在握手时硬失败。**iOS/Android 走 UniFFI 进程内调用，既没有握手也没有版本交换**——`grep client_protocol_version|CLIENT_PROTOCOL_VERSION` 在 `engine-mobile` / `ios-framework` / `android-aar` 三个 crate 里**零命中**。移动端的失败形式是重新生成绑定后**编译不过**，不是运行时握手拒绝。

## I. 核验记录

2026-08-23，对本文初稿跑了一轮对抗式核验：9 个查证代理按断言簇分工，凡判定「spec 写错」的再交给一个**默认反驳**的代理去证伪。共 53 个代理、116 条断言，**31 条被确认写错并已在上文修正，13 条被反驳代理推翻（判定为查证代理自己看错）**。

其中改变了设计而非仅措辞的四条：
1. §0 —— 工作区 `LINGXI.md` 在移动端从未加载。这不是本方案的问题，是当前在线的缺陷，且**推翻了对上一版真机现象的归因**。
2. §A.2 —— `manifest.surface == None` 与历史遗留应用重载，被迫在 `AppRecord` 上新增持久字段。
3. §C.2 —— `LocalAppCreate` 的 `app_id` 由 builtin 在分发前注入，「没有 app_id 就不受门管」不成立；门改为按工具名判定。
4. §D.1 —— 落地布防依赖的 brief 认领会被本方案删掉，需要新的关联键。
