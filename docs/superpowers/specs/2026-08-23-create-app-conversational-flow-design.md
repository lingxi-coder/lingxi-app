# 对话式创建本地应用（create-app conversational flow）

> 取代 2026-08-22 落地的「create-first 表单」方案。旧的两步表单整体删除，**不考虑向后兼容**。
>
> 经过两轮核验：(1) 内部对抗式核验 116 条断言，31 条修正；(2) 一轮外部 code review（3 P0 / 3 P1），5 条采纳、1 条修正后采纳，另有 4 处其自身不实。记录见 §I。

## Context

上一版把创建做成两步原生表单：用户写一句话简介，引擎用一次无头 LLM 调用提议名称与形态，用户确认后创建，再打开应用自己的会话。它绕开了「会话根目录在 `ConversationSource` 构造时绑定、活的会话无法重新扎根」这个约束，代价是把需求收集塞进填空框。

真机上的问题不是表单不好看：进入会话后代理会重新搭脚手架、跳过需求确认直接开工。上一版只能在 kickoff 文案里写「不要再创建应用」来压制。

**核验推翻了对这个现象的归因**（§0）：那份本该每轮约束代理的工作区合约，在 iOS 上从来没有到达模型。代理不是无视合约，是没看见合约。

本方案把创建过程移进对话：点「创建应用」立刻得到一个**空壳**，会话从第一轮起就扎在它自己的工作区里，代理逐步问清需求、提议名称与形态、经用户确认后落地脚手架。

**保证分层**（这是本方案最重要的一条设计原则，见 §C.0）：
- **保证**由不可绕过的机制提供——首次脚手架**覆盖**源文件种子。
- **提示**由权限规则与工具门提供——它们减少代理走错路的概率，但**不被当作正确性依据**。

---

## 0. 前置修复（阻塞项）：工作区 `LINGXI.md` 在移动端从未加载

**必须先修、先有测试，再做后面任何一步。**

### 事实链

1. `apps/engine-mobile/src/host.rs:3291`：`model_cwd` 取自 `workspace_mount.guest_path`（GUEST）。紧邻注释写着「The engine-internal cwd (`cwd` — transcripts, .lingxi, **memory files**) stays host」——**意图是 host**。
2. `host.rs:3329`：`SessionCwd::new(model_cwd, trusted_dirs)`。`session_cwd` 实际持有 **guest** 路径，与上一句注释相反。
3. `orchestrator/src/conversation.rs:12052-12076`（`build_system_prompt`）走了 PathAtlas S3 的 `prompt_probe_cwd_resolver` 做 guest→host 转换，是**对的**。
4. `conversation.rs:12270`（`additional_context_message`）**没走**：`self.memory.load(&self.session_cwd.cwd())`，读原始 guest 路径。
5. `orchestrator/src/prompt/mod.rs:153`：内存块已不再拼进系统提示词 ⇒ **`additional_context_message` 是唯一渲染路径**。
6. iOS 确实装了 provider（`apps/ios-framework/src/lib.rs:503`、`:735`），问题纯粹是坐标。

⇒ `apps/<id>/workspace/LINGXI.md` 在 iOS 上从未进入模型上下文。`scaffold_app_value` 写的整份工作区合约一直空转。

### 修复

把 `additional_context_message` 的 `memory.load` 也经过 `prompt_probe_cwd_resolver`。桌面端没有该 resolver，`probe_cwd == cwd`，字节不变。

### 测试

- **钉坐标**：session cwd 为 guest 路径、mount 表映射到真实 host 目录、该目录放一个 `LINGXI.md`，断言其内容出现在 `additional_context_message` 产物里。**这条必须先对着未修复的代码跑成红色。**
- 桌面端回归：无 resolver 时字节不变。

⚠️ 本修复独立于其余部分且价值更高（它同时修好正式版合约）。**先单独落地、单独真机验证（§G.0）。**

⚠️ **修复顺序是 §0 → §C.0 → 其余。** §0 不修，连引导版合约都到不了模型，「先问用户」这一步比写入约束更早就断了。

---

## 已锁定的决策

| 决策 | 选择 |
|---|---|
| 空壳创建时机 | 点击「创建应用」那一刻，记录与工作区目录立即真实存在 |
| 会话根目录 | 从第一轮起扎在应用工作区，**不引入会话重新扎根机制** |
| 未完成应用 | 库里显示为「草稿·创建中」，点进去续上同一对话，可随时删除 |
| 「基础模版」 | 就是 `dom` / `canvas` 两种形态，**不重新引入模版目录** |
| 空壳判据 | `AppRecord.scaffolded == false`（新增持久字段，§A.2） |
| 脚手架完整性 | **首次脚手架覆盖源文件种子**（§C.0），不依赖「禁止代理写入」 |
| 状态提交点 | `scaffolded = true` 是 **commit point**，最后写，CAS（§C.1） |
| 创建结果关联 | **`request_id`**，由 `CreateApp` 携带、成功与失败事件原样回传（§D.1） |
| 协议版本 | **8.0.0**，本方案**先**落地并 bless；另一份 spec rebase 上来（§B.4） |

## 非目标

- 不做会话 cwd / Linux 挂载点 / session catalog 的热切换。
- 不重新引入模版目录或起手样板。
- 不改 `LocalAppCreate` 工具（普通聊天里代理主动建应用那条路）的既有语义。
- 不做空壳的自动清理。
- 不迁移历史遗留应用（`surface == None` 且已有脚手架），见 §A.1。
- 不开放「创建后改名」。§C.1 的名称写入是**首次命名**，且受 §C.1.4 的 hash 不变量约束。

---

## A. 状态模型

### A.1 为什么不能用 `manifest.surface == None`

`manifest.surface == None` 已经有第二批活的持有者——**脚手架拆分之前创建的历史应用**：

- `local-apps/src/manifest.rs:277`：「`None` means the app predates the scaffold split and cannot be rebuilt」
- `local_apps_build.rs:206` 的 `detect_build_target` 整段存在的理由就是对它报「create a new app to continue development」
- `APPS_SCHEMA_VERSION` 仍是 `1`，没有迁移回填 `surface`；`surface` 只有两个写入点

裸用它，每个历史应用都会：库里被改标成「创建中」（真名与简介被隐藏、点击跳去 pin 会话，**一个还能跑的应用变得打不开**）；工具全被拒；且 `LocalAppScaffold` 会**接受**它，把新脚手架写到老源码上——正是那条报错文案要防止的损坏。

### A.2 判据：`AppRecord.scaffolded: bool`

`local-apps/src/types.rs` 的 `AppRecord` 新增：

```rust
/// 工作区里是否已经落下脚手架。
/// serde 默认为 `true`：历史记录一律按「已成形」加载，不会被误判成刚点出来的空壳。
/// 只有 `CreateApp{surface: None}` 写 `false`；只有 `LocalAppScaffold` 的提交点翻成 `true`。
#[serde(default = "default_scaffolded")]
pub scaffolded: bool,
```

三条理由：

1. **语义正确**：`surface == None` 是两种含义的并集，`scaffolded` 只有一种。
2. **零额外 IO**：`surface` 只在 `AppManifest`（`manifest.rs:287`），**不在 `AppRecord` 上**。要把它送上列表行，`lower_record`（`local_apps_bridge.rs:251`）就得对每个应用多做一次 `load_manifest`（读文件+反序列化+validate，无缓存），而 `AppsChanged` 每次应用变更都发。
3. **客户端可判**：客户端没有文件系统，无法执行「`surface == None` 且工作区没有 `package.json`」这种复合判据。

代价：持久记录 schema 变更，牵连 `local-apps/tests/serde_compat.rs` 与 `local-apps/tests/fixtures/v1/apps/*/app.json`。必须有一条**从旧 fixture 加载**的测试断言 `scaffolded == true`。

四种样子（第四行是本方案必须不打扰的那一批）：

| | `record.scaffolded` | `manifest.surface` | 工作区 | 库里显示 |
|---|---|---|---|---|
| 空壳 | `false` | `None` | 只有 `.lingxi/` 与引导版 `LINGXI.md` | 草稿 · 创建中 |
| 已成形未构建 | `true` | `Some(_)` | 脚手架已落地 | 草稿 |
| 可运行 | `true` | `Some(_)` | 有 `build/store/dist/` | 就绪 |
| **历史遗留** | `true`（serde 默认） | `None` | 老脚手架 | 照旧，**不受本方案影响** |

### A.3 必须下移的一条旧不变量

`service.rs:911` 目前硬性拒绝空 brief。理由是「问卷要从 brief 生成」——**问卷流水线已删除，理由过期**。

处置：**下移**。创建时允许空 brief；`LocalAppScaffold` 时必须非空。

⚠️ 两条现存测试直接钉着旧不变量，**必须一起改**：
- `service.rs:2091` `create_app_enforces_brief_caps`（首个断言 `create_app(Some("A"), " ", None)` → `InvalidRequest`）→ 保留 `MAX_BRIEF_BYTES` 那一半，空串那一半改成「被接受」
- `service.rs:2170` `create_app_rejects_an_empty_brief` → 改写成「空 brief 产生 `scaffolded == false`、名字为占位常量的空壳」

### A.4 占位名，以及它会泄漏到哪里

创建空壳时服务层 `name` 传 `None`、brief 为空，`service.rs:919` 的派生得到空串，回落到固定的**非本地化**占位常量 `"untitled"`。

除库卡片外，至少五处无条件渲染 `name`/`brief`，**全部必须按 `scaffolded == false` 分支**：

| 位置 | 现状 |
|---|---|
| `clients/ios/.../LocalAppsDrawerSection.swift:33` | `Text(app.name)` |
| `clients/ios/.../LocalAppDetailView.swift:306` | `LabeledContent("local_apps_brief", value: app.brief)`（空串；§D.4 恰恰把用户往这页引） |
| `clients/android/.../LocalAppsScreen.kt:681` | `LocalAppCard` 同时渲染 `name` 与 `brief` |
| `clients/ios/.../LocalAppsStore.swift:891` | widget 标签 `summary.name.isEmpty ? summary.brief : summary.name` |
| 库卡片 | §D.3 |

还有一处**逃出客户端**：`host.rs:9145` / `:9172` 用 `record.name` 作 pin 住的 init 会话**标题**，`"untitled"` 会写进持久化会话目录。处置：`LocalAppScaffold` 提交后**重命名该会话**（§C.1.5，post-commit 可重试）。

---

## B. 协议

### B.1 `CreateApp`：翻转 `surface` 语义 + 追加 `request_id`

| `surface` | 新语义 | 谁在用 |
|---|---|---|
| `Some(dom \| canvas)` | 创建记录**并**脚手架（今天的行为） | `LocalAppCreate` 工具那条路、测试 |
| `None` | **只创建空壳**（`scaffolded: false`），不脚手架 | 新的原生「创建应用」按钮 |

今天 `None` 的含义是「宿主挑路由默认值然后照样脚手架」。翻转只影响移动客户端，而它本次要重写。代价（明确接受）：这个字段从此有两种含义，字段文档必须写清。

**追加 `request_id: Option<String>`**（§D.1 要求）。⚠️ 因此 `CreateApp` **不再是零 wire 变更**——初稿这句话作废。追加在最后（uniffi 位置编码）。

⚠️ **payload 事实**：
- `commands.rs:366` 的 `name: String` 是**必填非可选**。客户端发 `name: ""`（空串）**不是 nil**；「传 `None`」只在服务层成立。宿主需要一条「wire 空串 → 服务层 `None`」的分支。
- `commands.rs:368` 的 `origin: AppCreateOriginDto` 也是必填。按钮路径发 `.library`。

### B.2 列表行 DTO 追加 `scaffolded`

列表行是 `AppRecordDto`（`client-protocol/src/local_apps.rs:231`），由 `lower_record`（`local_apps_bridge.rs:251`）从 `&AppRecord` 直接映射；iOS 视图模型是 `LocalAppSummary`（`LocalAppsModels.swift:25`）。⛔ 仓库里**没有** `AppSummaryDto` 这个类型（初稿的杜撰）。

`AppRecordDto` 追加 `scaffolded: bool`。因为 §A.2 把它放在 `AppRecord` 上，`lower_record` 是**纯映射补齐**，无额外 IO。

⚠️ uniffi 按**位置**编码结构体字段，必须**追加在最后**。
⚠️ `client-protocol/tests/version_guard_test.rs` 的 `current_contract_index()` 是**手写字符串字面量表**（`AppRecordDto.*` 在 `:1285-1293` 附近），不更新它守卫**看不见**改动；同文件 `:2133/:2204/:2302` 三处结构体字面量要补字段。

TS 侧（`clients/shared/src/protocol.ts:851`）同步加字段。⛔ **该文件没有任何 runtime guard / zod 校验**，纯 interface——外部 review 提出的「TypeScript runtime guards」在仓库里不存在，无需改动。

### B.3 删除提议命令/事件

删 `ClientCommand::ProposeAppIdentity` 与 `ClientEvent::AppIdentityProposed`。删枚举变体是破坏性改动（§0.10）。

必须同批改动：
- `client-protocol/src/version.rs` → `"8.0.0"`；`tests/version_test.rs`
- `snapshots/contract_index.json`（重新生成）；`snapshots/blessed_major.txt` → `8`
- ⚠️ `tests/version_guard_test.rs` — 手写表里 `put("ClientEvent::AppIdentityProposed"…)`（`:337-340`）与 `put("ClientCommand::ProposeAppIdentity"…)`（`:599`）共 7 行。**删变体后代码照样编过**，覆盖锚点只是抽样 ⇒ 不删这几行，守卫看不见删除，8.0.0 不会被强制
- ⚠️ `tests/snapshot_test.rs`；删除 `snapshots/command/propose_app_identity.json`、`snapshots/event/app_identity_proposed.json`
- `clients/shared/src/protocol.ts` — 版本常量**以及两个 union 成员**（`:1213-1214` 附近的 `app_created` / `app_record_changed` 不动，删的是提议那两条）

### B.4 协议所有权与顺序（跨计划）

`docs/superpowers/specs/2026-08-23-local-app-interactive-verification-design.md:5` 声明基线 `7.0.0` / blessed major 7，且自身也有多项 protocol 追加。**两份计划不能各自独立 bless。**

定案顺序：
1. **本方案先落地并 bless 8.0.0。**
2. 另一份 rebase 到 8.0.0 之后再走它自己的 bless。

⚠️ 该文件当前是**另一个会话正在改的未提交工作**，本方案**不修改它**；顺序需要人工在两个会话间协调。实施本方案时若发现它已抢先 bless，停下来问，不要自行合并。

---

## C. 引擎

### C.0 脚手架完整性：保证靠覆盖，不靠禁止

**核验发现的洞**：`scaffolded == false` 期间代理完全可以往 `app/`、`src/` 写文件，而首次脚手架**不会覆盖它们**。三个机制叠加：

1. `scaffold_workspace_initialized` 对源文件用 `write_file(…, overwrite=false)`；`write_file` 在目标已是普通文件时**直接返回 Ok**。
2. `permission/src/workspace_lease.rs:611` 的 `host_owned_relative` 拒的是 `.lingxi/**`、`LINGXI.md`、各种 config、`package.json`、`pnpm-lock.yaml`、`node_modules`、`index.html`——**`app/**` 与 `src/**` 不在其中**。
3. 每应用授权模板里写着 `"Edit(./**)"`（`local-apps/templates/vite-react-static-v1/.lingxi/settings.local.json`）。

#### C.0.1 保证（不可绕过）

**`scaffolded == false` 的首次脚手架，源文件种子用 `overwrite=true`。**

空壳按定义没有合法的应用源码，覆盖是安全的；这样**不论代理在此之前写了什么**，脚手架完整性都成立。这也是 §C.1 失败重试安全的前提。

实现：`scaffold_workspace_initialized` 接收一个 `first_scaffold: bool`（或等价的枚举），`true` 时源文件也走 `overwrite=true`。锁定文件本来就是 `true`，不变。已成形应用的重新钉合（`restore_host_managed_files`）路径**不受影响**。

#### C.0.2 提示（减少走错，不作正确性依据）

- **收紧空壳的 `.lingxi/settings.local.json`**：创建空壳时写一份不含 `Edit(./**)`、不含 `LocalAppBuild` / `LocalAppRuntime` 许可的变体，`LocalAppScaffold` 提交后改写成正常版。这个文件本来就是每应用一份、创建时从模板写入（`local-apps/src/permissions.rs:36`），是现成的seam。
  ✅ **一条规则覆盖四个工具**：`permissions.rs:25` 上方那段（对 2.1.235 二进制核过）说明 `Write` / `MultiEdit` / `NotebookEdit` **全部查 `Edit` 名下的规则**。所以不需要为它们各写一条。
- **工具门**（§C.2）拦住构建/安装/运行时那一类。

⚠️ **Bash 封不住，别假装封住了。** `gate_mobile_shell_ctx`（`host.rs:1650`）只在 mobile-linux **不可用**时关闭 shell，而本地应用会话恰恰挂着它 ⇒ shell 能往工作区写，权限规则管不着。这正是 C.0.1 必须存在的理由：把正确性押在「禁止」上，就永远留着这个缺口。

### C.1 新工具 `LocalAppScaffold`

入参：`{app_id, name, brief, surface, workflow_model?}`，`additionalProperties: false`。

⛔ **命名禁区**：`local_apps_mcp.rs` 有一条断言对**全部工具 schema 的拼接串**做小写子串检查禁止 `template`；`local_app_template_removal_guard_test.rs` 另有四个禁用符号。工具名、参数名、枚举值、description 一律避开。

#### 步骤（顺序是规范的一部分）

1. **预留（CAS）**：在**一个** `with_app` 闭包内检查 `record.scaffolded == false` 并标记「正在脚手架」。范式照 `set_init_session`（`service.rs:693`）——它就是在 `with_app` 里做 set-once 检查+写入，天然原子，**不需要新的锁**。并发的第二个调用在这里就被拒。
2. **校验**：`brief.trim()` 非空（§A.3）、`name` 非空、长度在 `MAX_NAME_BYTES` / `MAX_BRIEF_BYTES` 内；`surface` 可解析。**历史遗留应用（`scaffolded == true`、`surface == None`）必须在第 1 步就被拒。**
3. **落地**（全部完成后才提交）：
   - manifest：`surface` + `name`（见 C.1.4）
   - 锁定文件 + 源文件种子（**`overwrite=true`**，§C.0.1）
   - 正式版 `LINGXI.md`（覆盖引导版）
   - `AppRecord` 的 `name` / `brief` / `workflow_model`
4. **提交点**：最后才写 `scaffolded = true`。任何一步失败 ⇒ 保持 `false`，允许安全重试（重试安全正是因为 C.0.1 覆盖）。
5. **post-commit 可重试**：重命名 pin 住的 init 会话（§A.4）。失败只记日志，不回滚。

⚠️ **两个标志的写入顺序相反，且都是有意的，不要「统一」它们。**
`scaffold_app_value` 现有注释**刻意**先盖 `manifest.surface` 再写文件：「Stamping first means a crash between the two steps leaves an app that can be scaffolded again, not one that cannot」——那条性质保留。而 `record.scaffolded` 是**外层**的提交点，必须最后写。一个是「文件层可重入」，一个是「记录层已完成」，各自守各自的东西。

⚠️ **不需要在成功后排队 dependency install。** `queue_dependency_install` 位于 `ensure_dependency_install`（`local_apps_host.rs:1208`），由**构建路径**惰性触发，创建路径不碰。

#### C.1.4 改 `manifest.name` 的 hash 不变量（外部 review 未覆盖）

`AppManifest::hash()`（`manifest.rs:460`）序列化**整个结构体**，**含 `name`**；`AppDataStore::ensure_manifest`（`data.rs:597`）拿它与 SQLite `_lingxi_schema.manifest_hash` 比较，不等就对**所有数据读写**报「database manifest mismatch」。

对刚创建的空壳是安全的：它没有任何 collection，`AppDataStore::open`（`data.rs:263`，open 时才写入那行 schema）从未被调用过，库还不存在。

⇒ **写成显式不变量 + 测试**：`manifest.name` 只允许在 `LocalAppScaffold` 的首次落地中写入，且必须先断言该应用尚无数据库。**永远不要**把这条路径推广成「允许改名」——那会直接损坏用户数据。这也是 §非目标里「不开放创建后改名」的真正理由。

#### C.1.5 服务层需要的写入方法

⚠️ `record.name` / `record.brief` / `record.workflow_model` **三者今天都没有更新路径**（`record.name =` / `record.brief =` 在 `AppState::create_with_git` 之外一处都没有；`update_manifest` 改的是 manifest 显示名，不是 `AppRecord.name`）。需要一条**把四个字段（含 `scaffolded`）按 §C.1 的顺序写回、持久化并发事件**的服务方法。`git_enabled` 不开放修改。

#### C.1.6 权限

`permission/src/defaults_per_tool.rs` 设 `AllowByDefault`。

⚠️ **不够**：`local_apps_tools.rs:187` 的手写子集 `requires_bound_session_for_auto_allow` 会把 allow-by-default 工具在 `bound_app_id(ctx)` 为 `None` 时降级为 `Ask`。builtin 对每个会话都注册，**全局会话里也看得见 `LocalAppScaffold`**。必须加进这张表：扎在该应用工作区的会话免弹框，全局会话仍要问。

⚠️ 另有两处硬同步点：`local_apps_tools.rs` 的 `LOCAL_APP_TOOLS` 表；`local_apps_mcp.rs:2057` 的 `catalog_is_fixed_and_exposes_no_arbitrary_execution_surface`——一条**精确名字列表的 `assert_eq!`**，新增任何 provider operation 都会撞红。

### C.2 工具门

位置：`local_apps_mcp.rs` 的 `call()`（`:1146`）**最顶端**，紧随 `validate_input`、**在 `:1151` 的 `parse_dynamic_tool` 分支之前**。

⚠️ **仓库里不存在同时覆盖两条分支的先例。** `runtime_api_compatible()`（`:1173`）在动态分支**内部**（分支 `:1152` 开、`:1192` 关），只覆盖应用自有 MCP 命名空间；静态 `match tool` 那条路**根本没有 runtime-api 检查**。门必须自己写在 `:1151` 之上，app_id 来源分两种：`parse_dynamic_tool(tool)` 命中时取绑定值，否则取 `input["app_id"]`。

#### ⚠️ 按 operation 判定，不是按 builtin 名

`call()` 收到的是 **provider operation**：`LocalAppTool::call` 调 `transport.call_host_operation(self.operation, input)`，静态 match 的臂是 `"list"` / `"get"` / `"build"` / `"create"`。`LOCAL_APP_TOOLS`（`local_apps_tools.rs:53`）的第二个元素就是 operation。

⇒ 放行名单写 `LocalAppScaffold` 会**连 scaffold 自己一起拦掉**。放行名单（**operation**）：`scaffold`、`list`、`get`、`create`。

也**不能**按「入参里有没有 `app_id`」判定：`LocalAppTool::call`（`local_apps_tools.rs:367`）在分发**之前**会从会话 cwd 注入 `app_id`。

`create` 放行是有意的：空壳会话里代理若真要另建一个应用，不是本门要防的错误；本门防的是「在空工作区上构建/安装/跑运行时」。

规则：目标应用 `record.scaffolded == false` 且 operation 不在放行名单内 → `tool_error`：

> 应用 `<id>` 还没有形态。先与用户确认要做什么，再用 `LocalAppScaffold` 定下名称、简介与形态。

数量：`LOCAL_APP_TOOLS` 今天 **22** 条，加本方案的 1 条为 **23**，放行 4 ⇒ 受门管 **19**。⚠️ **测试必须从工具表派生这个集合，不要硬编码数量**——初稿写 18 就是硬编码算错的结果。

### C.3 两份 `LINGXI.md`

**先决条件：§0 必须已修。** 否则本节写的文件到不了模型手里。

**引导版**——在 `CreateApp{surface: None}` 的 initializer 里写入 `workspace/LINGXI.md`。此时 `layout.initialize()` 已跑过，工作区目录存在。

内容要点（正式版会整体覆盖）：
- 这个应用刚创建，**还没有形态**，工作区是空的。
- 你现在的任务是引导用户，不是写代码。**现在写的任何源文件都会在脚手架落地时被覆盖**（§C.0.1）。
- 本地应用工具里，此刻只有 `LocalAppScaffold` 对你有意义；构建、安装依赖、运行时、界面检查那一类都会拒绝你并告诉你原因。问需求用 `AskUserQuestion`。
- 步骤：先问用户想做什么 → 据回答推断意图，用 `AskUserQuestion` 把提议的**名称**与**形态**（`dom` 多屏界面 / `canvas` 单一绘制面：游戏、3D、可视化）交给用户确认或修改 → 确认后调 `LocalAppScaffold` → 重读本文件，按新合约继续。
- 形态一旦落地不可更改，所以必须让用户确认，不要自作主张。

✅ `AskUserQuestion` 在移动端确实注册且有 resolver（`apps/engine-mobile/src/lib.rs:201`、`:254`），本设计对它的依赖成立。

**正式版**——`scaffold_app_value` 现有的两套合约文本（dom / canvas），不动。

⚠️ **「代理改不了它」的理由**（结论对、初稿的理由错）：该文件在 `apps/<id>/workspace/LINGXI.md`，**就在可写根之内**。真正拦住写入的是 `permission/src/workspace_lease.rs` host-owned 拒绝名单里的 `LINGXI.md`。做 §C.0.2 时若碰了那条规则，等于把合约的编辑权交给代理。

### C.4 `detect_build_target` 的错误分支

以 `record.scaffolded` 分家，**不是**嗅探 `package.json`（§A.2 已把这个区分做成持久事实，再探一次文件系统就是第二套推导）：

| 条件 | 结论 |
|---|---|
| `manifest.surface == Some(_)` | 对应目标 |
| `scaffolded == false` | 空壳 → 「这个应用还没有形态，先用 `LocalAppScaffold` 定下形态」 |
| `scaffolded == true` 且 `surface == None` | 历史遗留 → 保持现有的「请新建应用」 |
| 有 `next.config.mjs` | 保持现有的 legacy 拒绝 |

⚠️ `detect_build_target` 还有不经过 MCP 的调用点。实现时**逐个数清并各自加测试**。

### C.5 删除

`handle_propose_app_identity` / `propose_app_identity`（`host.rs`）、`APP_IDENTITY_SYSTEM_PROMPT`、`parse_app_identity`、`fallback_app_name`，及其全部测试。

---

## D. 客户端

### D.1 入口：删掉整个表单，用 `request_id` 关联

「+」按钮不再 push 创建路由，直接三步：
1. 生成一个 `request_id`（客户端 UUID），发
   `CreateApp{request_id, brief: "", name: "", origin: .library, surface: nil, git_enabled: 默认, workflow_model: nil, conversation_id: 当前会话}`
2. 按 `request_id` 认领结果，再等 `AppRecordChanged` 带回 pin
   （⚠️ `AppCreated` 永远不带 `init_session_id`——它在创建事务内发出，pin 之后才铸。）
3. 打开 `.localApp(appID)` 会话并自动发 kickoff

⚠️ **一次性布尔不可用（已否决）**：引擎对**两条创建路径**都发 `AppCreated`，布尔在并发创建时会把代理在别处建的应用错认成本次「+」的结果，把用户劫持进错误的会话。今天的客户端正是用**精确 brief 比对**规避这个竞态（`LocalAppsStore.swift:857`、`LocalAppsViewModel.kt:806`），而本方案要删掉那套认领 ⇒ 必须有真正的关联键。

**`request_id` 契约**：
- `CreateApp` 携带（`Option<String>`，追加在最后）
- **成功与失败事件都原样回传**（`AppCreated` 追加 `request_id: Option<String>`；创建失败的错误响应同样带回）
- 客户端只认领 `request_id` 匹配的事件；不匹配的一律忽略
- **pending 清理**：客户端为每个 pending `request_id` 设超时（与既有创建超时一致），超时或断线重连后清空 pending 并向用户报「创建结果未知，请在应用库确认」——**不得**在重连后认领任何未带匹配 `request_id` 的事件

### D.2 kickoff 文案

换成不带占位符的一句话（用户视角：「我想做一个新的本地应用。」）。

⚠️ **i18n 必须改 `clients/translations/*.json`，生成产物碰都不碰**（iOS 的 `Localizable.xcstrings`、Android 的 `values*/strings.xml` 都是 `generate.py` 的产物）。新文案不带占位符，两端**共用一个 key**。

同批删除：`local_apps_init_kickoff %@` 与 Android 孪生 `local_apps_init_kickoff`，以及创建表单的全部文案 key。⚠️ Android 另有一份**不由生成器管理**的 `values*/strings_local_apps_v3.xml`，一并核对。

### D.3 草稿态

`AppRecordDto.scaffolded == false` 的卡片：标题渲染本地化的「新应用」、副标题「创建中」，**不显示** `name`/`brief`；点击进 pin 会话而非预览；删除照旧。§A.4 列出的另外四处渲染点用同一判据分支。

### D.4 Widget 入口搬家（两端都要，Android 更麻烦）

⚠️ widget 请求目前**两端都只能从创建表单进入**：
- iOS：`pendingWidgetSetup` 只有一个置位点，在创建落地路径上
- Android：`LocalAppsViewModel.kt:322` 的 `pendingWidgetPin = addWidget`，由 `LocalAppsScreen.kt` 创建弹窗里的 `addWidget` 开关驱动

⇒ 删掉表单等于**两端都回归**这个功能。

- **iOS**：挪到 `clients/ios/Sources/LocalApps/LocalAppDetailView.swift`。⚠️ 该文件今天 `grep -i widget` **零命中**，是净新增 UI。
- **Android**：⚠️ **没有应用详情页可搬**——`localapps/` 下只有 `LocalAppsScreen.kt` 等文件，不存在 detail 屏。必须**新建承载点**（应用卡片的溢出菜单，或一个详情 bottom sheet）。这是本方案里唯一一处净新增的 Android UI，工作量不要按「移动一个开关」估。

### D.5 删除

- iOS：`LocalAppCreateView`（⚠️ **不是独立文件**，声明在 `LocalAppsLibraryView.swift` 内）；`AppIdentityProposal`、`proposeIdentity`、`identityProposalAnswers`、`creationBrief`
- Android：⚠️ 组件名是 **`CreateAppDialog`**（`LocalAppsScreen.kt:303`，`:270` 调用），不是 `CreateAppSheet`；连同 `onProposeIdentity` 与 `LocalAppsViewModel` 对应状态

---

## E. 需要改动的文件

### 前置（§0）
- `lingxi-code/orchestrator/src/conversation.rs` — `additional_context_message` 的 memory 加载走 probe resolver + 对应测试

### 协议
- `client-protocol/src/commands.rs`（`surface` 语义 + `request_id`）、`src/events.rs`（删变体 + `AppCreated.request_id`）、`src/local_apps.rs`（`AppRecordDto.scaffolded`）、`src/version.rs`
- `client-protocol/tests/version_test.rs`、⚠️ `tests/version_guard_test.rs`、⚠️ `tests/snapshot_test.rs`
- `snapshots/contract_index.json`、`snapshots/blessed_major.txt`
- ⚠️ 删除 `snapshots/command/propose_app_identity.json`、`snapshots/event/app_identity_proposed.json`
- `clients/shared/src/protocol.ts`、`clients/shared/test/snapshots.test.ts`

### 引擎
- `local-apps/src/types.rs` — `AppRecord.scaffolded`
- ⚠️ `local-apps/tests/serde_compat.rs` + `local-apps/tests/fixtures/v1/apps/*/app.json`
- `local-apps/src/service.rs` — 允许空 brief；占位名；§C.1.5 的写入方法（CAS，照 `set_init_session`）
- ⚠️ `local-apps/src/permissions.rs` + `local-apps/templates/vite-react-static-v1/.lingxi/settings.local.json` — 空壳的收紧授权变体（§C.0.2）
- `apps/engine-mobile/src/local_apps_build.rs` — `scaffold_workspace_initialized` 的 `first_scaffold` 覆盖开关（§C.0.1）；`detect_build_target` 分支
- `apps/engine-mobile/src/local_apps_mcp.rs` — `LocalAppScaffold` schema + dispatch；`call()` 顶端的门；⚠️ `:2057` 的精确名字列表
- `apps/engine-mobile/src/local_apps_tools.rs` — `LOCAL_APP_TOOLS`；⚠️ `requires_bound_session_for_auto_allow`
- `permission/src/defaults_per_tool.rs`
- `apps/engine-mobile/src/local_apps_host.rs` — `LocalAppScaffold` 实现；引导版 `LINGXI.md`
- `apps/engine-mobile/src/local_apps_bridge.rs` — `lower_record` 补 `scaffolded`（⚠️ `lower_manifest` 也在这个文件，但它构造的 `AppManifestDto` 只经 `GetAppDetails` 到达，**不在列表流上**，本方案不用它）
- `apps/engine-mobile/src/host.rs` — `CreateApp` 分叉；wire 空 `name` → 服务层 `None`；`request_id` 回传；pin 会话重命名；删提议命令

### 技能与文档
- ⚠️ `skills/create-local-app/SKILL.md` — **被 `include_str!` 编进引擎**（`skill-api/src/builtin/bundled.rs:31`），其「Entry and confirmation」段落逐字写着旧契约
- ⚠️ `skills/create-local-app/agents/openai.yaml`
- ⚠️ `docs/local-apps/HANDOFF.md` — `:11`、`:45`、`:95` 三处陈述的正是本方案反转的不变量

### 客户端
- `clients/ios/Sources/LocalApps/` — `LocalAppsLibraryView.swift`、`LocalAppsStore.swift`、`LocalAppsModels.swift`、`LocalAppsProtocolAdapter.swift`、⚠️ `LocalAppDetailView.swift`、⚠️ `LocalAppsDrawerSection.swift`
- `clients/ios/Sources/App/RootView.swift`
- `clients/android/.../localapps/` — `LocalAppsScreen.kt`、`LocalAppsViewModel.kt`、`LocalAppsContract.kt`（⚠️ 含新建的 widget 承载点）；`RootScreen.kt`
- `clients/translations/*.json`（5 语言）+ 跑 `generate.py`
- 测试：`clients/ios/Tests/LocalAppsStoreTests.swift`、`clients/android/.../LocalAppsViewModelTest.kt`

---

## F. 测试

### 前置（§0）
见 §0。**那条坐标测试必须先见红。**

### Rust
- **旧 fixture 加载** → `scaffolded == true`（历史应用不被误判成空壳）
- **空壳创建**：`scaffolded == false`；工作区只有 `.lingxi/` 与 `LINGXI.md`；引导版合约命中关键指令；空 brief 被接受；授权文件是收紧变体
- **§C.0.1 覆盖保证**（最重要的一条）：先在空壳工作区写一个与种子同路径的文件（例如 `app/screens/home-screen.jsx`），再调 `LocalAppScaffold`，断言该文件**被种子内容覆盖**。⛔ 这条测试必须对着 `overwrite=false` 的当前实现**先跑成红色**，否则它测的不是这个洞。
- **门**：**从 `LOCAL_APP_TOOLS` 表派生**受门管集合，逐项断言拒绝且文案指向 `LocalAppScaffold`；放行的四个 operation 能过。⛔ 不硬编码数量、不写 19 个函数
- **门的两条路径**：静态 `match` 与 `parse_dynamic_tool` 各一条。只测一条等于没测到 §C.2 的要害
- **门按 operation 而非 builtin 名**：一条专门断言 `scaffold` 自己不被拦
- **`LocalAppScaffold` 原子性**：
  - 成功路径写回四个字段、正式合约覆盖引导版、pin 会话被重命名
  - **落地阶段失败 ⇒ `scaffolded` 仍为 `false`，且重试成功**（这条钉住 §C.1 的提交点顺序）
  - 并发两次调用，第二次在 CAS 处被拒
  - 二次调用（已成形）被拒；空 brief 被拒；未知 surface 被拒
  - **历史遗留应用（`scaffolded == true`、`surface == None`）被拒**
- **§C.1.4 hash 不变量**：断言首次落地前该应用没有数据库；并有一条测试证明「若数据库已存在则拒绝改 `manifest.name`」
- **`detect_build_target`**：空壳 / 历史遗留 **各自独立断言**
- **改写**：`create_app_enforces_brief_caps`、`create_app_rejects_an_empty_brief`（§A.3）

⚠️ 跑法：`cargo test --workspace --all-features --no-fail-fast`。engine-mobile 的 local-apps 模块是 `#[cfg(feature = "uniffi")]`，不加 `--all-features` 整块被跳过。全量输出落文件再 grep（只 grep `FAILED` 会丢掉 `failures:` 块里的测试名），并盯**测试总数**是否下降。

### 客户端
⚠️ brief 认领机制**纯粹在客户端**，不属于 Rust 段：`clients/ios/Tests/LocalAppsStoreTests.swift`（`:1571-1656`）与 `clients/android/.../LocalAppsViewModelTest.kt`（`:302-528`），**每端各约 9-12 条**，不是总共 9 条。

另需：`request_id` 认领（含**不匹配的事件必须被忽略**、超时清理、断线重连不误认领）；草稿卡片渲染；`name`/`brief` 五处渲染点不泄漏占位串；「+」不再弹表单；kickoff 文案；两端的 widget 常驻入口。

⚠️ 改 wire 之后**必须先重新生成 uniffi 绑定再编客户端**：
- `bash clients/ios/scripts/build-xcframework.sh && (cd clients/ios && xcodegen generate)`
- `bash clients/android/scripts/build-jni.sh --variant play`

## G. 真机验收

**不接受「编过了」。**

**G.0（§0 单独验收，先做）**：在一个**已有**的本地应用会话里，让代理复述 `LINGXI.md` 里只在该文件出现过的一条约束。修复前它应当答不出来。

1. 点「+」→ **不弹任何表单**，直接进对话，代理第一句在问你想做什么
2. 回答「打飞机」→ 代理提议名称，并用**原生选项**让你确认形态，且它选的是 `canvas`
3. 确认后应用成形、草稿标记消失、一路能构建出可玩的东西
4. 反向用例：第 1 步就退出 → 库里留一张「创建中」卡片，点回去续上**同一个**对话
5. **历史遗留应用回归**：打开一个 `surface == None` 的老应用，确认它**没有**被标成「创建中」、工具没被门拒、`LocalAppBuild` 仍给「请新建应用」
6. **两端 widget 入口**：iOS 详情页、Android 新承载点各加一次成功

## H. 风险与已知留口

- **门与授权规则只是提示。** 正确性由 §C.0.1 的覆盖提供。**Bash 能绕过一切写入禁止**（`gate_mobile_shell_ctx` 只在 mobile-linux 不可用时关 shell），这是接受的事实而非待修项。
- **门保证不了「代理真的问过用户」。** 它可能问完第一句就自作主张调 `LocalAppScaffold`。只能靠提示词，验收 G.2 就是在验它。若真机反复不过，下一步是让 `LocalAppScaffold` 要求一个「用户已确认」的证据字段——同样可被编造，本期不做。
- **空壳会堆积**，没有自动清理，靠用户删。
- **`CreateApp.surface` 一个字段两种含义**。靠文档与测试压制，不引入第二个命令。
- **`AppRecord` 多了一个持久字段**，相对最初「不新增状态」的承诺是一次让步（§A.2）。
- **8.0.0 的破坏面**：只有**桌面端**（Electron / `clients/shared`）在握手时硬失败。**iOS/Android 走 UniFFI 进程内调用，既没有握手也没有版本交换**（`grep CLIENT_PROTOCOL_VERSION` 在 `engine-mobile` / `ios-framework` / `android-aar` 零命中），失败形式是重新生成绑定后**编译不过**。
- **跨计划协议冲突**（§B.4）需要人工协调，本方案不动另一份 spec。

## I. 核验记录

### 第一轮（内部对抗式，2026-08-23）
9 个查证代理按断言簇分工，凡判「spec 写错」的再交给**默认反驳**的代理证伪。53 个代理、116 条断言，**31 条确认写错并修正，13 条被反驳代理推翻**。改变设计的四条：§0（工作区 `LINGXI.md` 在移动端从未加载，**推翻了对上一版真机现象的归因**）；§A.2（`surface == None` 与历史遗留应用重载）；§C.2（`app_id` 由 builtin 注入，门的判据不成立）；§D.1（落地布防依赖将被删除的 brief 认领）。

### 第二轮（外部 code review，2026-08-23）
6 条 findings（3 P0 / 3 P1），**5 条采纳、1 条修正后采纳**：

| findings | 处置 |
|---|---|
| P0 工具门不能保证 scaffold 是首次写入 | **成立**。但修法改为 §C.0.1 的**覆盖**而非「动态禁止 Write/Edit/Bash」——Bash 封不住，把正确性押在禁止上永远留缺口。禁止层降级为 §C.0.2 的提示 |
| P0 状态转换不原子 | **成立** → §C.1 的 CAS + 提交点。⚠️ 补充了它未察觉的冲突：`scaffold_app_value` 现有注释刻意让 `manifest.surface` **先**写，两个标志顺序相反且都有意 |
| P0 创建结果关联留给实现者 | **成立** → §D.1 定案 `request_id` + pending 清理 |
| P1 门用错命名层级、数量不对 | **成立**，且比它指出的更基础（写 builtin 名会拦掉 scaffold 自己）→ §C.2，23/19，表驱动 |
| P1 两份 spec 的 protocol baseline 冲突 | **成立** → §B.4 定序 |
| P1 删表单回归 Android widget | **成立**，且比它说的麻烦：Android **没有详情页可搬**，是净新增 UI → §D.4 |

**该 review 自身的四处不实**（executor 不要照抄它的引用）：
1. 路径 `lingxi-code/crates/local-apps/…` 不存在（没有 `crates/` 层）
2. Android 包名是 `com.lingxi.code.localapps`，不是 `com.lingxi.localapps`
3. 「TypeScript runtime guards」在仓库里不存在——`clients/shared/src/protocol.ts` 是纯 interface，无 zod/校验函数
4. 「成功后再排队 dependency install」不需要——`queue_dependency_install` 由构建路径惰性触发（`local_apps_host.rs:1208`）

**两轮都未覆盖的一处，由本轮补上**：改 `manifest.name` 会改动 `AppManifest::hash()`，而该 hash 绑着 SQLite `_lingxi_schema`（§C.1.4）。对空壳安全，但必须写成不变量，否则将来的「允许改名」会损坏用户数据。
