# 对话式创建本地应用（create-app conversational flow）

> 取代 2026-08-22 落地的「create-first 表单」方案。旧的两步表单整体删除，**不考虑向后兼容**。
>
> 经过多轮内部核验、外部 code review 与用户决策收敛；完整记录见 §J。
>
> 与交互验收 design 的共享补丁、协议 rebase 和实施顺序统一由 [`2026-08-24-local-app-implementation-order.md`](./2026-08-24-local-app-implementation-order.md) 管理。

## Context

上一版把创建做成两步原生表单：用户写一句话简介，引擎用一次无头 LLM 调用提议名称与形态，用户确认后创建，再打开应用自己的会话。它绕开了「会话根目录在 `ConversationSource` 构造时绑定、活的会话无法重新扎根」这个约束，代价是把需求收集塞进填空框。

真机上的问题不是表单不好看：进入会话后代理会重新搭脚手架、跳过需求确认直接开工。上一版只能在 kickoff 文案里写「不要再创建应用」来压制。

**核验推翻了对这个现象的归因**（§0）：那份本该每轮约束代理的工作区合约，在 iOS 上从来没有到达模型。代理不是无视合约，是没看见合约。

本方案把创建过程移进对话：点「创建应用」立刻得到一个**空壳**，会话从第一轮起就扎在它自己的工作区里，代理逐步问清需求、提议名称与形态、经用户确认后落地脚手架。

在这条对话式创建链路稳定之后，§I 以它为唯一入口扩展 Local App 的 2D/3D 运行时：移动端 WebView 先加入五种 profile，桌面 host 再加入 Godot 原生项目。游戏引擎实施**不得**与本方案的 create-local-app 重构并行修改同一技能、工具 schema 或 workflow；必须等本方案合并并通过验收后再开始。

**保证分层**（这是本方案最重要的一条设计原则，见 §C.0）：
- **保证**由不可绕过的机制提供——首次脚手架**清空可编辑面再写入**（§C.0.1）。判据只有一条：**确认之前写的代码，一个字节都不能进入正式应用。**
- **提示**由工具门提供——减少代理走错路的概率，但**不被当作正确性依据**。

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

🚨 **这三行修复的影响面远大于它的体积，必须独立提交、独立浸泡。** 它会给**整个移动平台**
同时打开 memory 加载——不只是本地应用：嵌套 `@import` 展开、外部包含门、read-state 播种、
以及**每条首用户消息里新增的 token**。所以不要把它和 §A 之后的任何改动打进同一个提交，
也不要在同一轮真机验收里和别的变更一起验。
（这条警告原本写在 verification design 里；ownership 迁到本节时它一度在两份文档里同时消失。）

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
| 「基础模版」 | 底层仍只有 `dom` / `canvas` 两种 surface，**不重新引入面向用户的模版目录**；§I 的 runtime profile 选择 host-managed package/lock 与适配器，不新增第三种 WebView surface |
| 空壳判据 | `AppRecord.scaffolded == false`（新增持久字段，§A.2） |
| 脚手架完整性 | **首次脚手架先清空可编辑面再写入**（§C.0.1），不是逐路径覆盖 |
| 创建模式 | **wire 到服务层同一个概念**：`CreateApp.mode` / `CreateMode::{Shell, Scaffolded}`。空 brief 只在 `Shell` 下合法（§A.3），`surface` 只在 `Scaffolded` 下有意义（§B.1） |
| 文件系统互斥 | 从第一次文件写入到提交，全程持 `storage::lock_app_build`（§C.1） |
| 并发预留 | **进程内**（宿主侧），不落盘——所以崩溃重启后自动失效、重试安全（§C.1） |
| 状态提交点 | `scaffolded = true` 是 **commit point**，最后写，CAS（§C.1） |
| 创建结果关联 | **`request_id`**，由 `CreateApp` 携带、成功与失败事件原样回传（§D.1） |
| 协议版本 | **8.0.0**，本方案**先**落地并 bless；另一份 spec rebase 上来（§B.4） |
| 游戏运行时交付 | 两阶段：先移动端 WebView 五种 profile，再在 desktop Local App host 完成后加入 Godot `NativeGame`（§I） |
| profile 最终选择 | host-native picker 直接接收用户点击并签发一次性 receipt；生成代理不能改写选择（§I.3） |
| 引擎依赖 | 不随主应用内置；Full build 只可携带 engine-free base seed；snapshot 按 toolchain + lock digest 复用（§I.4） |
| 实施顺序 | 当前 create-local-app 重构先独立合并；Babylon/Havok 固定 IIFE spike 先过，才允许落 profile 公共接口（§I.1） |
| Godot 实施授权 | desktop host 完成后还必须先通过独立 NativeGame design + threat model；本文件不直接授权 Phase 2 编码（§I.7） |

## 非目标

- 不做会话 cwd / Linux 挂载点 / session catalog 的热切换。
- 不重新引入模版目录或起手样板。
- 当前 create-local-app 重构不改 `LocalAppCreate` 工具；后续仅按 §I.2 为它追加可选 `runtime_profile`，保留 `surface` 兼容路径，不改变创建授权、记录归属或会话语义。
- 不做空壳的自动清理。
- 不做旧本地应用数据迁移。本方案只在 **clean install** 上实现与验收；开发机上的旧 store 由**重装覆盖后在应用内清空本地应用库**移除（§A.1；⛔ 绝不卸载，理由见 §G.-1）。
- 不开放「创建后改名」。§C.1 的名称写入是**首次命名**，且受 §C.1.4 的 hash 不变量约束。
- Phase 1 不提供 Unity、Cocos、Defold、PlayCanvas 编辑器或第三方插件市场；Phase 2 的 Godot 只开放给 desktop host，且需独立安全设计（§I.7）。

---

## A. 状态模型

### A.1 前提：clean install，不做旧 store 迁移

这个 app 尚未发布，仍在开发阶段；本方案的安装与真机验收都以**重装覆盖安装 + 清空本地应用库**为前提（⛔ **绝不卸载**——卸载会连同 app 数据删掉设备上全部本地应用，那是用户的作品且没有第二份副本；清法见 §G.-1）。实现面对的是一个空的 local-app store，**不存在**旧 `AppRecord`、旧 manifest、workspace、data 或 build 输出。

因此本方案：

- 不实现旧记录迁移，也不把「缺 `scaffolded` 字段」解释成某种业务状态；
- 不把既有应用延迟伪装成空壳，更不会在下一次 `LocalAppScaffold` 时清理旧项目；
- 若开发机仍残留旧 store，视为不受支持的安装状态，按 §G.-1 的做法清空后再验收（⛔ 不要用卸载达成）。

这条前提把迁移问题从运行时代码中删除：新记录从创建那一刻起就必须显式携带 `scaffolded`，所有合法状态都由本版代码自己写出。

### A.2 判据：`AppRecord.scaffolded: bool`

`local-apps/src/types.rs` 的 `AppRecord` 新增：

```rust
/// 工作区里是否已经落下脚手架。
///
/// 每一条新记录都显式写入；缺字段是无效的旧 store（§A.1），不是空壳判据。
///
/// 三个写入点，缺一不可：
///   1. `CreateMode::Shell` 在构造记录时写 `false`；
///   2. `CreateMode::Scaffolded`（`LocalAppCreate` 的 create+scaffold 路径）
///      在构造记录时写 `true`；
///   3. `LocalAppScaffold` 的提交点把 `false` 翻成 `true`。
pub scaffolded: bool,
```

两条理由：

1. **零额外 IO**：`surface` 只在 `AppManifest`（`manifest.rs:287`），**不在 `AppRecord` 上**。要把它送上列表行，`lower_record`（`local_apps_bridge.rs:251`）就得对每个应用多做一次 `load_manifest`（读文件+反序列化+validate，无缓存），而 `AppsChanged` 每次应用变更都发。
2. **客户端可判**：`surface` 客户端根本拿不到，而草稿态要在列表行上判定。一个布尔随记录直接下发。

代价：持久记录形状改变，当前 fixtures 与结构体字面量都必须显式补字段。**不加 serde default，不写旧 fixture 迁移测试**；缺字段应当加载失败并提示清除开发数据，而不是静默变成可被清空的 shell。

三种样子：

| | `record.scaffolded` | `manifest.surface` | 工作区 | 库里显示 |
|---|---|---|---|---|
| 空壳 | `false` | `None` | 只有 `.lingxi/` 与引导版 `LINGXI.md` | 草稿 · 创建中 |
| 已成形未构建 | `true` | `Some(_)` | 脚手架已落地 | 草稿 |
| 可运行 | `true` | `Some(_)` | 有 `build/store/dist/` | 就绪 |

### A.3 必须下移的一条旧不变量

`service.rs:911` 目前硬性拒绝空 brief。理由是「问卷要从 brief 生成」——**问卷流水线已删除，理由过期**。

处置：**下移，并且用创建模式把两条路分开**——不能笼统地「创建时允许空 brief」，那会把 full-create 那条路的不变量一起废掉。

`create_app_with_git_and_workflow_model_and_initializer`（`service.rs:898`）今天既没有 `surface` 也没有模式参数，且**无条件**拒绝空 brief。给它加一个：

```rust
pub enum CreateMode {
    /// 「+」按钮建的空壳：brief 可以为空，记录写 `scaffolded: false`。
    Shell,
    /// create + scaffold 一步到位（`LocalAppCreate` 工具那条路）：
    /// brief 必须非空（保留今天的不变量），记录写 `scaffolded: true`。
    Scaffolded,
}
```

现有的四个包装构造函数一律传 `Scaffolded`，行为逐字节不变。⇒ 空 brief 的放宽**只发生在 `Shell` 模式**，`Scaffolded` 那条路的校验一个字都没松。这同时也是 §A.2 里 `scaffolded` 初值的**唯一**决定点，不用再靠调用方记得写对。

✅ **两条现存测试一个字都不用改。** `service.rs:2091` `create_app_enforces_brief_caps` 与 `service.rs:2170` `create_app_rejects_an_empty_brief` 调的都是默认 `create_app(...)` 包装 ⇒ `Scaffolded` 模式 ⇒ 空 brief **继续被拒**，两条继续绿。

⚠️ 初稿要求改写它们，那是 `CreateMode` 之前的遗留，**已作废**——照初稿改会亲手拆掉 `Scaffolded` 路径上唯一钉住该不变量的两条测试。

要新增的是**直接用 `CreateMode::Shell` 的测试**：空 brief 被接受、记录 `scaffolded == false`、名字落到占位常量。

### A.4 占位名，以及它会泄漏到哪里

创建空壳时服务层 `name` 传 `None`、brief 为空，`service.rs:919` 的派生得到空串，回落到固定的**非本地化**占位常量 `"untitled"`。

除库卡片外，以下每一处都无条件渲染 `name`/`brief`，**全部必须按 `scaffolded == false` 分支**：

| 位置 | 现状 |
|---|---|
| `clients/ios/.../LocalAppsDrawerSection.swift:33` | `Text(app.name)` |
| `clients/ios/.../LocalAppDetailView.swift:306` | `LabeledContent("local_apps_brief", value: app.brief)`（空串；§D.4 恰恰把用户往这页引） |
| `clients/android/.../LocalAppsScreen.kt:681` | `LocalAppCard` 同时渲染 `name` 与 `brief` |
| `clients/ios/.../LocalAppsStore.swift:891` | widget 标签 `summary.name.isEmpty ? summary.brief : summary.name` |
| ⚠️ `clients/ios/.../LocalAppsStore.swift:1147` `makeWidgetSnapshot()` | **把每个应用都映射进主屏 widget 快照**——空壳会以 `"untitled"` 出现在用户主屏上 |
| ⚠️ `clients/android/.../LocalAppsViewModel.kt:1032` `publishWidgetSnapshot()` | 同上 |
| 库卡片 | §D.3 |

**widget 快照的处置是「排除」而不是「改文案」**：`scaffolded == false` 的应用**整个不进快照**。这样不需要给 widget 的 DTO 加字段，也不会出现一个点不开的主屏图标。

还有一处**逃出客户端**：`host.rs:9145` / `:9172` 用 `record.name` 作 pin 住的 init 会话**标题**，`"untitled"` 会写进持久化会话目录。处置：`LocalAppScaffold` 提交后仅在标题仍是 mobile-empty 占位记录时**条件重命名**；用户已经 `/rename` 的标题不动（§C.1.5，post-commit 可重试）。

---

## B. 协议

### B.1 `CreateApp`：显式 `mode` 字段 + 追加 `request_id`

`CreateApp` 追加两个字段（都在最后，uniffi 位置编码）：

| 字段 | 含义 |
|---|---|
| `mode: AppCreateModeDto` | `Shell` = 只建空壳（`scaffolded: false`）；`Scaffolded` = 创建并脚手架（今天的行为） |
| `request_id: Option<String>` | §D.1 的关联键 |

`surface` 只在 `Scaffolded` 模式下有意义；`Shell` 模式下必须为 `None`，否则拒绝（形态是脚手架时才定的，见 §C.1）。

⚠️ **初稿的做法（保持结构不变、翻转 `surface: None` 的语义为「不脚手架」）已废弃。** 那是为了「零 wire 变更」而做的将就，代价是一个字段承担两种含义——`§H` 曾把它列为接受的疤。既然删提议变体已经要求 8.0.0 这个破坏性版本，保住 wire 形状换不到任何东西，只换来一个必然被误读的字段。**不考虑旧版本兼容，就该直说。**

✅ 顺带：`AppCreateModeDto` 与 §A.3 服务层的 `CreateMode` 是**同一个概念、同一套名字**，从 wire 一路贯到 `AppRecord.scaffolded` 的初值，中间没有转译，也没有第二处需要人记住的映射。

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
- ⚠️ 本批同时落地 §D.1 的两个关联字段（它们与删变体共享同一次 bless）：`AppEventDto::AppCreated.request_id`（`src/local_apps.rs:917`）与 `ClientEvent::AppOperationFailed.request_id`（`src/events.rs:320`）
- `snapshots/contract_index.json`（重新生成）；`snapshots/blessed_major.txt` → `8`
- ⚠️ `tests/version_guard_test.rs` — 手写表里 `put("ClientEvent::AppIdentityProposed"…)`（`:337-340`）与 `put("ClientCommand::ProposeAppIdentity"…)`（`:599`）共 7 行。**删变体后代码照样编过**，覆盖锚点只是抽样 ⇒ 不删这几行，守卫看不见删除，8.0.0 不会被强制
- ⚠️ `tests/snapshot_test.rs`；删除 `snapshots/command/propose_app_identity.json`、`snapshots/event/app_identity_proposed.json`
- `clients/shared/src/protocol.ts` — 版本常量**以及两个 union 成员**（`:1213-1214` 附近的 `app_created` / `app_record_changed` 不动，删的是提议那两条）

### B.4 协议所有权与顺序（跨计划）

`2026-08-23-local-app-interactive-verification-design.md` 也有 annotation/build-generation protocol 追加。**两份计划不能从各自的旧基线独立 bless。**

定案顺序：
1. **本方案先落地并 bless 8.0.0。**
2. verification Phase 3 rebase 到 8.0.0 与 profile-aware 代码之后，再按实际 contract diff 决定下一版本并只生成一次两端 bindings。

完整顺序与共享文件唯一 owner 见 [`2026-08-24-local-app-implementation-order.md`](./2026-08-24-local-app-implementation-order.md)。实施时若 verification 仍从 7.0.0 开始修改协议，立即停止并 rebase；不得人工拼接两个 contract snapshot。

以后若本方案继续修改 `scaffolded` commit point、surface/profile、workspace/session pin、build workflow 集合、pnpm/profile lock 或 create protocol anchors，**同一变更必须更新 master order 与 verification 的「上游 create-flow 契约与防漂移门」**。只改本文件而把下游留给以后，视为 design 未完成。

---

## C. 引擎

### C.0 脚手架完整性：保证靠清空，不靠禁止

**核验发现的洞**：`scaffolded == false` 期间代理完全可以往 `app/`、`src/` 写文件，而首次脚手架**不会覆盖它们**。三个机制叠加：

1. `scaffold_workspace_initialized` 对源文件用 `write_file(…, overwrite=false)`；`write_file` 在目标已是普通文件时**直接返回 Ok**。
2. `permission/src/workspace_lease.rs:611` 的 `host_owned_relative` 拒的是 `.lingxi/**`、`LINGXI.md`、各种 config、`package.json`、`pnpm-lock.yaml`、`node_modules`、`index.html`——**`app/**` 与 `src/**` 不在其中**。
3. 每应用授权模板里写着 `"Edit(./**)"`（`local-apps/templates/vite-react-static-v1/.lingxi/settings.local.json`）。

#### C.0.1 保证（不可绕过）

**`scaffolded == false` 的首次脚手架，先把工作区可编辑面清空，再写锁定文件与种子。**

保留的只有三项：`.lingxi/`、`LINGXI.md`、`node_modules/`。其余一律删除。空壳按定义没有任何合法的应用源码，所以清空是安全的；这也是 §C.1 失败重试安全的前提——重试敢重跑，正是因为它每次都从干净的地面开始。

⚠️ **初稿写的「源文件种子用 `overwrite=true`」不够。** 覆盖只作用于种子**自己那九个路径**，种子之外的文件照样进构建：Vite 8.2.1 的 `DEFAULT_EXTENSIONS` 把 `.js` 排在 `.jsx` **之前**，模板没有 `resolve.extensions` 覆盖，16 处 import 全是无扩展名的，`copy_workspace_tree` 会把多余文件一并拷进构建根 ⇒ 预先写下的 `app/app.js` 在解析时**赢过**种子的 `app/app.jsx`，种子沦为死代码；`lib/lingxi-provider.js` 同理顶替 host-managed 的 `.jsx`。逐路径覆盖挡不住这一类，**清空可以**。

⚠️ **也不要把它当「继承来的既有隐患」放过**（初稿如此，是错的）。今天的创建在**创建事务内部**就完成脚手架（`host.rs:5731` 的 initializer、`local_apps_mcp.rs:1352` 的 `create` 闭包），**根本不存在「工作区已建、尚未脚手架」的可写窗口**。那个窗口是本方案开的，所以这个顺序破口也是本方案自己的。判据只有一条：**确认之前写的代码，一个字节都不能进入正式应用。** 逐路径覆盖过不了。

实现：`scaffold_workspace_initialized` 接收 `first_scaffold: bool`（或等价枚举）。`true` 时先清空（保留上述三项）再写；锁定文件与种子此时都是 `overwrite=true`。已成形应用的重新钉合（`restore_host_managed_files`）路径**不受影响**——它绝不清空。

#### C.0.2 提示（减少走错，不作正确性依据）

**工具门**（§C.2）拦住构建/安装/运行时那一类。**这是提示层的全部内容**——下面两条是本轮核验砍掉的方案，留在这里是为了让后来者不要再走一遍：

⛔ **不要收紧空壳的 `.lingxi/settings.local.json`。** 初稿提议创建时写一份不含 `Edit(./**)` 的变体、脚手架后改回。**行不通**：移动端只在**引擎构造时读一次**该文件（`host.rs:2978` 读 `<cwd>/.lingxi/settings.local.json`，`:3084` 一次性构造 `PermissionPolicy::from_rules` 并 `Arc` 包好）。所以「提交后改写」对**正在进行的那个会话完全无效**，而收紧的规则会一直生效到会话结束——恰恰卡死脚手架之后那段真正要写源码的时间。要么做一条实时规则通道（不值得），要么不收紧。选后者。

⛔ **不要写「Shell 绕得过一切写入禁止」。** 初稿这句话是错的，且它曾是「保证必须靠覆盖」的主论据。事实：移动端 shell 工具名为 **`Shell`**（`tools/shell-mobile/src/lib.rs:47`），而 `.localApp` 会话里 `deny_workspace_host_owned`（`permission/src/policy.rs:939`、`:950`，位于通用 allow 分支**之前**）已经硬拒了非只读巡检类的 shell 命令。**保证要靠覆盖，理由是 §C.0.3 的路径集边界，不是 shell。**

#### C.0.3 清空之后仍然留下的（登记在案，本期不修）

§C.0.1 的清空只覆盖**首次脚手架那一刻**。应用一旦成形，`Edit(./**)` 授权就回来了，代理仍可写一个 `app/app.js` 影子掉 `app/app.jsx`——**这一半才是真正的既有隐患**：今天任何已成形应用都可以，本方案既不引入也不扩大。

修它需要给 Vite 钉 `resolve.extensions`，或让每次构建前的钉合清掉锁定/种子路径的同名异扩展兄弟文件。**单独立项，不在本期。**

分清两半：**确认之前**的写入由 §C.0.1 的清空彻底解决（本方案的责任）；**成形之后**的影子写入是既有面（不是）。

### C.1 新工具 `LocalAppScaffold`

入参：`{app_id, name, brief, surface, workflow_model?}`，`additionalProperties: false`。

⛔ **命名禁区**：`local_apps_mcp.rs` 有一条断言对**全部工具 schema 的拼接串**做小写子串检查禁止 `template`；`local_app_template_removal_guard_test.rs` 另有四个禁用符号。工具名、参数名、枚举值、description 一律避开。

#### 步骤（顺序是规范的一部分）

1. **预留：进程内，不落盘。** 宿主侧一个 `Mutex<HashSet<AppId>>`（或等价物），进入时插入、离开时无条件移除（RAII guard，走 panic 路径也要还）。已在集合中 ⇒ 立刻拒绝。

   ⚠️ **不要照 `set_init_session` 做预留。** 那个范式是往**持久字段**上做 set-once 写入，而预留一旦落盘，进程被杀就再也清不掉——草稿永久变砖，直接和第 4 步的「失败可重试」互斥。`with_app` 也守不住：它的守卫只活到自身的完成任务结束，第 2-4 步全在锁外跑（`service.rs:308`）。引擎在设备上是**单进程**，所以进程内预留就够；重启后集合自然为空、`scaffolded` 仍是 `false`，重试照常成功。
   （`set_init_session` 仍是**第 4 步提交写**的范式——那一步确实是持久字段上的 set-once CAS。）
2. **校验**：`brief.trim()` 非空（§A.3）、`name` 非空、长度在 `MAX_NAME_BYTES` / `MAX_BRIEF_BYTES` 内；`surface` 可解析。
3. **落地**（全部完成后才提交）。⚠️ **记录在这一步里只是「暂存」，不落盘**：

   0. 取 `storage::lock_app_build(root, app_id)`（见下方警告），**持到第 4 步结束**
   1. 构造一份**未持久化**的 `proposed_record`（`record.clone()` 后套上确认的 `name` / `brief` / `workflow_model`）
   2. manifest：`surface` + `name`（见 C.1.4）
   3. 清空可编辑面 + 写锁定文件 + 写种子（§C.0.1）
   4. 正式版 `LINGXI.md`（覆盖引导版），**用 `proposed_record` 渲染**

   ⚠️ **不要在这一步就把 `name` / `brief` 写进库。** 初稿让第 1 步直接持久化，落地失败就会留下一个「名字已经是『打飞机』、却仍然是空壳」的记录——用户在库里看到一个像样的名字，点进去却回到引导对话。四个字段必须**在第 4 步一次性提交**。

   ⚠️ **必须全程持 `storage::lock_app_build`。** 它的文档原话是「Callers must hold this lock for the **complete** operation that mutates or removes an app's workspace/build tree」，**物理删除走的是同一把锁**（`storage.rs:899` 经 `lock_app_build_if_present`）。进程内预留（步骤 1）只排斥另一个 `LocalAppScaffold`，**挡不住 `DeleteApp`**：并发删除会把应用目录 rename 进 trash，而脚手架还在往里写 ⇒ 写进一个已被摘除的目录，留下永不回收的孤儿。现有的构建路径（`local_apps_build.rs:634`）和宿主路径（`local_apps_host.rs:1370`）都已经这么做，照它们写。

   ⚠️ **`LINGXI.md` 渲染顺序**：`scaffold_app_value` 的合约文本是 `format!("# Local App: {name} ({id})\n\nBrief: {brief}…", name = record.name, brief = record.brief)`（`local_apps_host.rs:3219`）。渲染时必须传 `proposed_record` 而不是创建时那份，否则写出来的是 `# Local App: untitled` + 空 Brief——而 `LINGXI.md` **只写这一次**（`restore_host_managed_files` 不含它，二次 `LocalAppScaffold` 被拒），且按 §0 它是**唯一**每轮到达模型的通道 ⇒ 整个对话问出来的需求会在唯一的长期载体里永久丢失。
4. **提交点**：**一个** `with_app` 闭包内一次性持久化 `name` / `brief` / `workflow_model` / `scaffolded = true`（照 `set_init_session` 的 set-once CAS）。任何一步失败 ⇒ 四个字段一个都没落盘、`scaffolded` 仍是 `false`，预留随 guard 析构释放，构建锁随之释放，允许安全重试（重试安全正是因为 §C.0.1 每次都清空重来）。
5. **post-commit**：对 pin 住的 init 会话做一次条件重命名（§A.4）。**只有最新生效的 `custom-title` 仍是带 `mobileEmptySession: 1` 的初始占位记录时**，才追加应用真名；如果用户已用 `/rename` 或 hook 写过普通 `custom-title`，立即尊重用户、不改名。失败只记日志，不回滚。

   ⚠️ **「可重试」必须有真正的触发器，否则只是措辞。** 现有的 boot backfill sweep（`host.rs:9354`）遍历每条记录、自愈目录漂移、补缺失的 pin，但**不碰已存在会话的标题**——所以一次失败的重命名今天永远不会被修好。⇒ 在那个 sweep 里加一条**标题对账**。

⚠️ **即时重命名和 boot 对账必须共用同一判据，不能只在 sweep 里保护用户标题。** 「标题不等于 `record.name` 就改」会抹掉用户自己改的标题。`/rename`（`orchestrator/src/handle_impl.rs:632` 的 `append_custom_title`）、hook 的 `sessionTitle` 和 mobile 的初始占位**写的是同一条 `custom-title` 通道**，光看标题分不出来。唯一的分辨依据是 `append_mobile_empty_session`（`session/src/jsonl/writer.rs:516`）多带的一个字段：

```json
{"type":"custom-title","customTitle":"…","sessionId":"…","mobileEmptySession":1}
```

即时重命名与 boot 对账条件统一为：**该会话最新生效的 `custom-title` 记录仍然带 `mobileEmptySession: 1`** —— 即用户从未改过名 —— 且 `record.scaffolded == true` 且标题与 `record.name` 不符。一旦后面出现过普通 `custom-title`（没有该标记），**尊重用户，不动**。

测试要有两个反例：intake 期间先 `/rename` 再调用 `LocalAppScaffold`，即时重命名不得覆盖；`/rename` 之后跑 sweep，标题也必须**不变**。

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

⚠️ 另有**三处**硬同步点，漏一处就撞红：
1. `local_apps_tools.rs` 的 `LOCAL_APP_TOOLS` 表
2. `local_apps_mcp.rs:2057` 的 `catalog_is_fixed_and_exposes_no_arbitrary_execution_surface`——一条**精确名字列表的 `assert_eq!`**
3. ⚠️ `permission/src/defaults_per_tool.rs:143` 的 `debug_assert_eq!(m.len(), 66, "tool defaults table must list all 66 tools")` → 改 `67`，连同上一行注释 `// 44 oracle-parity tools + 22 mobile local-app builtins` 与模块文档里的同类计数。**它就在 §C.1.6 刚点名的那个文件里**，且只在 debug 构建里 panic——release 真机不发作，本地 `cargo test` 必炸。

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

**引导版**——在 `CreateApp{mode: Shell}` 的 initializer 里写入 `workspace/LINGXI.md`（该模式下 `surface` 必须为 `None` 且不调用 `scaffold_app_value`，见 §B.1）。此时 `layout.initialize()` 已跑过，工作区目录存在。

内容要点（正式版会整体覆盖）：
- 这个应用刚创建，**还没有形态**，工作区是空的。
- 你现在的任务是引导用户，不是写代码。**现在写下的任何源文件都会在脚手架落地时被删除**（§C.0.1 清空可编辑面），写了也是白写。
- 本地应用工具里，此刻只有 `LocalAppScaffold` 对你有意义；构建、安装依赖、运行时、界面检查那一类都会拒绝你并告诉你原因。
- 步骤：**用普通对话文本**开放式地问用户想做什么并等他回答（这一轮**不用** `AskUserQuestion`）→ 读描述、能推断的自己推断，只对仍然悬着且会改变产物的可枚举选择用 `AskUserQuestion` 问一轮 → 用 `AskUserQuestion` 把提议的**名称**与**形态**（`dom` 多屏界面 / `canvas` 单一绘制面：游戏、3D、可视化）交给用户确认或修改 → 确认后调 `LocalAppScaffold` → 重读本文件，按新合约继续。
- 形态一旦落地不可更改，所以必须让用户确认，不要自作主张。

⛔ **开场那一轮必须是普通文本，不是选择器。** `AskUserQuestion` 弹的是原生选择器，而开场时模型只知道「用户想要一个应用」，能填进选项里的全是对用户想法的猜测；选择器于是收上来一个「在你的猜测之间的选择」，**顶替掉**后面每一步都要读的那段自由描述。所以：开场用文本要描述，从第二轮起才是 `AskUserQuestion` 的地盘（它擅长的是在**可列举的选项**之间做决定）。技能文档 (`skills/create-local-app/SKILL.md`) 里那条「Never ask unresolved questions in ordinary assistant text」必须**在原地**写出这条例外并限定只覆盖开场那一轮，否则两条规则互相矛盾，读到后一条的模型会把开场改回选择器。

✅ `AskUserQuestion` 在移动端确实注册且有 resolver（`apps/engine-mobile/src/lib.rs:201`、`:254`），本设计对它的依赖成立。

⚠️ 上述 `dom` / `canvas` 确认是 §0-§H 首次合并的基线。Phase 1 开始后必须由 §I.3 **整体替换**：`AskUserQuestion` 只做需求澄清，最终 profile 由 `LocalAppConfirmRuntimeProfile` 的 host-native picker 决定，`LocalAppScaffold` 只接收 receipt；不要让两套确认同时存在。

**正式版**——§0-§H 首次合并时，`scaffold_app_value` 现有的两套合约文本（dom / canvas）不动。Phase 1 在对应 surface 合约上追加已提交 profile、唯一允许的 adapter/frame-loop 所有者和 workflow 路由；核心 bridge/host-owned 文件约束继续共享，不能复制成五份会漂移的完整合约。

⚠️ **「代理改不了它」的理由**（结论对、初稿的理由错）：该文件在 `apps/<id>/workspace/LINGXI.md`，**就在可写根之内**。真正拦住写入的是 `permission/src/workspace_lease.rs` host-owned 拒绝名单里的 `LINGXI.md`。做 §C.0.2 时若碰了那条规则，等于把合约的编辑权交给代理。

### C.4 `detect_build_target` 的错误分支

以 `record.scaffolded` 分家，**不是**嗅探 `package.json`（§A.2 已把这个区分做成持久事实，再探一次文件系统就是第二套推导）：

| 条件 | 结论 |
|---|---|
| 有 `next.config.mjs` | 保持现有的 legacy 拒绝 |
| `scaffolded == false` 且 `surface == None` | 空壳 → 「这个应用还没有形态，先用 `LocalAppScaffold` 定下形态」 |
| `scaffolded == true` 且 `surface == Some(_)` | 对应目标 |
| `false + Some(_)` 或 `true + None` | 本版代码不可产生 → 视为存储损坏报错，不要静默 |

判断顺序要以 `scaffolded` 与 `surface` 的**组合**为准，不能先看到 `surface == Some(_)` 就直接返回目标；否则一个撕裂/损坏记录会绕过空壳门。clean install 前提意味着这里不承担任何旧状态迁移（§A.1）。

⚠️ **`detect_build_target(layout: &AppLayout)` 拿不到 `AppRecord`，但不用改签名也不用穿 `AppService`**：`storage.rs` 的 `AppMetadataFile` 就是 `apps/<id>/workspace/.lingxi/app.json`——整个 `AppRecord` 的镜像，只凭 `layout` 就能读（`metadata_rel(app_id)`），且 `repair_torn_commit` 明确它在撕裂提交时**优先于索引**。读它就是读 §A.2 那个持久事实，**不违反**本节「不要嗅探文件系统」的禁令——那条禁的是 `package.json` / `vite.config.mjs` 这类**被脚手架自己重写**的文件（循环论证），不是记录镜像。

⚠️ `detect_build_target` 还有不经过 MCP 的调用点。实现时**逐个数清并各自加测试**。

### C.5 删除

`handle_propose_app_identity` / `propose_app_identity`（`host.rs`）、`APP_IDENTITY_SYSTEM_PROMPT`、`parse_app_identity`、`fallback_app_name`，及其全部测试。

---

## D. 客户端

### D.1 入口：删掉整个表单，用 `request_id` 关联

「+」按钮不再 push 创建路由，直接三步：
1. 生成一个 `request_id`（客户端 UUID），发
   `CreateApp{request_id, mode: .shell, brief: "", name: "", origin: .library, surface: nil, git_enabled: 默认, workflow_model: nil, conversation_id: 当前会话}`
2. 按 `request_id` 认领结果，再等 `AppRecordChanged` 带回 pin
   （⚠️ `AppCreated` 永远不带 `init_session_id`——它在创建事务内发出，pin 之后才铸。）
3. 打开 `.localApp(appID)` 会话并自动发 kickoff

⚠️ **一次性布尔不可用（已否决）**：引擎对**两条创建路径**都发 `AppCreated`，布尔在并发创建时会把代理在别处建的应用错认成本次「+」的结果，把用户劫持进错误的会话。今天的客户端正是用**精确 brief 比对**规避这个竞态（`LocalAppsStore.swift:857`、`LocalAppsViewModel.kt:806`），而本方案要删掉那套认领 ⇒ 必须有真正的关联键。

**`request_id` 契约**：
- `CreateApp` 携带（`Option<String>`，追加在最后）
- **成功与失败事件都原样回传**

⚠️ **成功那一半必须穿到领域层，不能只改 `host.rs`。** `AppCreated` **不是** `host.rs` 发的：`AppService` 在自己的创建完成任务里发出（`service.rs:1053`，早于 `create_app_*` 返回），再经 `lower_app_event`（`local_apps_bridge.rs:211`，一个纯映射函数，由分离的转发器调用）到达 wire。`handle_create_app` 根本看不到那个事件，也没有装饰它的钩子。⇒ 必须：
- `local-apps/src/events.rs` 的 `AppEvent::AppCreated` 追加 `request_id: Option<String>`
- `create_app_with_git_and_workflow_model_and_initializer` 及其**四个包装构造函数**加一个 `request_id` 参数（`LocalAppCreate` 那条路传 `None`）
- `local_apps_bridge.rs` 的 `lower_app_event` 透传

⛔ 只按初稿改 `events.rs` 的 DTO + `host.rs`，成功路径上这个字段**永远是 `None`**，客户端「只认领匹配的事件」就永远不匹配 ⇒ 每次创建都超时，而 §D.5 又删掉了旧的 brief 认领 ⇒ **「+」按钮再也进不去新应用的会话**。

⚠️ **失败那一半今天没有载体。** 创建失败唯一的通道是 `ClientEvent::AppOperationFailed { app_id, code, message }`（`client-protocol/src/events.rs:320`），**没有 `request_id` 字段**。给它追加一个 `request_id: Option<String>`，并把它加进 §B.3 的「必须同批改动」清单——含 `version_guard_test.rs` 的手写表与受影响的 `snapshots/event/*.json`。
- 客户端只认领 `request_id` 匹配的事件；不匹配的一律忽略
- **pending 清理**：客户端为每个 pending `request_id` 设 **30 秒**的 `createResultTimeout`，超时或断线重连后清空 pending 并向用户报「创建结果未知，请在应用库确认」——**不得**在重连后认领任何未带匹配 `request_id` 的事件

  ⚠️ 这是一个**新常量，要自己定**。仓库里今天唯一相关的超时是 `identityProposalTimeout = .seconds(20)`（`LocalAppsStore.swift:414`，Android 对应 `LocalAppsViewModel.kt:1109`），而它是为**一次模型调用**设的、且随本方案一起删除——不要拿它当基准。30 秒的依据：创建本身是本地文件操作，但 pin 会铸一个会话，chat-origin 的还要 fork 源对话的历史，冷设备上可能不快；超时只是止损，应用其实已经在库里。

### D.2 kickoff 文案

换成不带占位符的一句话（用户视角：「我想做一个新的本地应用。」）。

⚠️ **i18n 必须改 `clients/translations/*.json`，生成产物碰都不碰**（iOS 的 `Localizable.xcstrings`、Android 的 `values*/strings.xml` 都是 `generate.py` 的产物）。新文案不带占位符，两端**共用一个 key**。

同批删除：`local_apps_init_kickoff %@` 与 Android 孪生 `local_apps_init_kickoff`，以及创建表单的全部文案 key。⚠️ Android 另有一份**不由生成器管理**的 `values*/strings_local_apps_v3.xml`，一并核对。

### D.3 草稿态

`AppRecordDto.scaffolded == false` 的卡片：标题渲染本地化的「新应用」、副标题「创建中」，**不显示** `name`/`brief`；点击进 pin 会话而非预览；删除照旧。§A.4 表里**其余每一处**渲染点用同一判据分支（widget 快照除外——那里是整个排除，不是改文案）。

### D.4 Widget 入口搬家（两端都要，Android 更麻烦）

⚠️ widget 请求目前**两端都只能从创建表单进入**：
- iOS：`pendingWidgetSetup` 只有一个置位点，在创建落地路径上
- Android：`LocalAppsViewModel.kt:322` 的 `pendingWidgetPin = addWidget`，由 `LocalAppsScreen.kt` 创建弹窗里的 `addWidget` 开关驱动

⇒ 删掉表单等于**两端都回归**这个功能。

- **iOS**：挪到 `clients/ios/Sources/LocalApps/LocalAppDetailView.swift`。⚠️ 该文件今天 `grep -i widget` **零命中**，是净新增 UI。
- **Android**：✅ **有现成承载点，初稿说「没有详情页」是错的。** `LocalAppsDestination.Details`（`LocalAppsContract.kt:214`）是一等目的地，在 `LocalAppsScreen.kt:143` 分发到 `LocalAppDetailsScreen`；`LocalAppCard` 自己还带一个溢出 `DropdownMenu`（`LocalAppsScreen.kt:695`）。⇒ 加一个 `LocalAppsAction` 并在其中一处置 `pendingWidgetPin` 即可，**不是新建屏幕**。

### D.5 删除

- iOS：`LocalAppCreateView`（⚠️ **不是独立文件**，声明在 `LocalAppsLibraryView.swift` 内）；`AppIdentityProposal`、`proposeIdentity`、`identityProposalAnswers`、`creationBrief`
- Android：⚠️ 组件名是 **`CreateAppDialog`**（`LocalAppsScreen.kt:303`，`:270` 调用），不是 `CreateAppSheet`；连同 `onProposeIdentity` 与 `LocalAppsViewModel` 对应状态

---

## E. 需要改动的文件

### 前置（§0）
- `lingxi-code/orchestrator/src/conversation.rs` — `additional_context_message` 的 memory 加载走 probe resolver + 对应测试

### 协议
- `client-protocol/src/commands.rs` — `CreateApp` 追加 `mode: AppCreateModeDto` 与 `request_id`；新增 `AppCreateModeDto` 枚举
- `client-protocol/src/local_apps.rs` — `AppRecordDto.scaffolded`；⚠️ **`AppEventDto::AppCreated` 的 `request_id` 也在这里**（`:917`），不在 `events.rs`
- `client-protocol/src/events.rs` — 删两个变体；⚠️ `ClientEvent::AppOperationFailed.request_id`（`:320`）
- `client-protocol/src/version.rs`
- `client-protocol/tests/version_test.rs`、⚠️ `tests/version_guard_test.rs`、⚠️ `tests/snapshot_test.rs`
- `snapshots/contract_index.json`、`snapshots/blessed_major.txt`
- ⚠️ 删除 `snapshots/command/propose_app_identity.json`、`snapshots/event/app_identity_proposed.json`
- `clients/shared/src/protocol.ts`、`clients/shared/test/snapshots.test.ts`

### 引擎
- `local-apps/src/types.rs` — `AppRecord.scaffolded`
- ⚠️ `local-apps/tests/serde_compat.rs` + `local-apps/tests/fixtures/v1/apps/*/app.json` — 当前 fixtures 显式补 `scaffolded`；不增加缺字段迁移/default
- `local-apps/src/service.rs` — ⚠️ `CreateMode::{Shell, Scaffolded}`（§A.3，同时决定 `scaffolded` 初值）；占位名；§C.1.5 的写入方法（提交写照 `set_init_session` 的 set-once CAS）；⚠️ `create_app_*` 及其四个包装构造函数加 `request_id` 参数
- ⚠️ `local-apps/src/events.rs` — `AppEvent::AppCreated` 追加 `request_id`（§D.1：`AppCreated` 由服务层发出，不是 `host.rs`）
- `apps/engine-mobile/src/local_apps_build.rs` — `scaffold_workspace_initialized` 的 `first_scaffold` **清空+写入**开关（§C.0.1）；`detect_build_target` 分支
- `apps/engine-mobile/src/local_apps_mcp.rs` — `LocalAppScaffold` schema + dispatch；`call()` 顶端的门；⚠️ `:2057` 的精确名字列表
- `apps/engine-mobile/src/local_apps_tools.rs` — `LOCAL_APP_TOOLS`；⚠️ `requires_bound_session_for_auto_allow`
- `permission/src/defaults_per_tool.rs` — 新条目 + ⚠️ `:143` 的 `debug_assert_eq!(m.len(), 66)` → `67` 及其两处计数注释
- `apps/engine-mobile/src/local_apps_host.rs` — `LocalAppScaffold` 实现；引导版 `LINGXI.md`
- `apps/engine-mobile/src/local_apps_bridge.rs` — `lower_record` 补 `scaffolded`；⚠️ `lower_app_event` 透传 `request_id`（⚠️ `lower_manifest` 也在这个文件，但它构造的 `AppManifestDto` 只经 `GetAppDetails` 到达，**不在列表流上**，本方案不用它）
- `apps/engine-mobile/src/host.rs` — `CreateApp` 分叉；wire 空 `name` → 服务层 `None`；`request_id` 回传；pin 会话重命名；删提议命令

### 技能与文档
- ⚠️ `skills/create-local-app/SKILL.md` — **被 `include_str!` 编进引擎**（`skill-api/src/builtin/bundled.rs:31`），其「Entry and confirmation」段落逐字写着旧契约
- ⚠️ `skills/create-local-app/agents/openai.yaml`
- ⚠️ `docs/local-apps/HANDOFF.md` — `:11`、`:45`、`:95` 三处陈述的正是本方案反转的不变量
- ⚠️ `apps/engine-mobile/src/host.rs` 的 boot backfill sweep（`:9354`）— 增加 pin 会话**标题对账**（§C.1 步骤 5）

### 客户端
- `clients/ios/Sources/LocalApps/` — `LocalAppsLibraryView.swift`、`LocalAppsStore.swift`、`LocalAppsModels.swift`、`LocalAppsProtocolAdapter.swift`、⚠️ `LocalAppDetailView.swift`、⚠️ `LocalAppsDrawerSection.swift`
- `clients/ios/Sources/App/RootView.swift`
- ⚠️ widget 快照排除空壳：`clients/ios/.../LocalAppsStore.swift` 的 `makeWidgetSnapshot()`、`clients/android/.../LocalAppsViewModel.kt` 的 `publishWidgetSnapshot()`
- `clients/android/.../localapps/` — `LocalAppsScreen.kt`（widget 动作加进**现有的** `LocalAppDetailsScreen` 或卡片溢出菜单）、`LocalAppsViewModel.kt`、`LocalAppsContract.kt`；`RootScreen.kt`
- `clients/translations/*.json`（5 语言）+ 跑 `generate.py`
- 测试：`clients/ios/Tests/LocalAppsStoreTests.swift`、`clients/android/.../LocalAppsViewModelTest.kt`

---

## F. 测试

### 前置（§0）
见 §0。**那条坐标测试必须先见红。**

### Rust
- **持久字段必填**：当前 fixtures 显式携带 `scaffolded`；缺字段的旧记录加载失败并提示清除开发数据，不得静默变成 shell（§A.1）
- **空壳创建**：`scaffolded == false`；工作区只有 `.lingxi/` 与 `LINGXI.md`；引导版合约命中关键指令；空 brief 被接受
- **`CreateMode` 分界**：`Scaffolded` 模式下空 brief **仍被拒**（旧不变量没被连坐废掉）、且记录 `scaffolded == true`；`Shell` 模式下空 brief 被接受、记录 `scaffolded == false`
- **`Shell` 模式带 `surface` 被拒**（§B.1：形态只在脚手架时定）
- **§C.0.1 覆盖保证**（最重要的一条）：先在空壳工作区写一个与种子同路径的文件（例如 `app/screens/home-screen.jsx`），再调 `LocalAppScaffold`，断言该文件**被种子内容覆盖**。⛔ 这条测试必须对着 `overwrite=false` 的当前实现**先跑成红色**，否则它测的不是这个洞。
- **门**：**从 `LOCAL_APP_TOOLS` 表派生**受门管集合，逐项断言拒绝且文案指向 `LocalAppScaffold`；放行的四个 operation 能过。⛔ 不硬编码数量、不写 19 个函数
- **门的两条路径**：静态 `match` 与 `parse_dynamic_tool` 各一条。只测一条等于没测到 §C.2 的要害
- **门按 operation 而非 builtin 名**：一条专门断言 `scaffold` 自己不被拦
- **`LocalAppScaffold` 原子性**：
  - 成功路径写回四个字段、正式合约覆盖引导版、pin 会话被重命名
  - ⚠️ **写出的 `workspace/LINGXI.md` 含用户确认的名称与简介，不是 `untitled` / 空 Brief**（钉住 §C.1 用 `proposed_record` 渲染；这是初稿的真 bug）
  - ⚠️ **落地失败后 `name` / `brief` 也没落盘**——库里不能出现「名字对了但还是空壳」的记录（钉住四字段一次性提交）
  - ⚠️ **§C.0.1 的清空**：在空壳工作区预先写下 `app/app.js`、`lib/lingxi-provider.js` 和一个种子里没有的 `app/screens/rogue.jsx`，脚手架后断言**三个都不在了**、且 `.lingxi/` 与 `node_modules/` 还在。⛔ 这条必须先对着当前实现跑成红色
  - ⚠️ **Scaffold 与 DeleteApp 并发**：删除在脚手架持锁期间发起，断言不产生「写进已被摘除目录」的孤儿（钉住 `lock_app_build`）
  - **落地阶段失败 ⇒ `scaffolded` 仍为 `false`，且重试成功**（这条钉住 §C.1 的提交点顺序）
  - 并发两次调用，第二次在**进程内预留**处被拒
  - **预留在失败路径上被释放**：第一次调用在落地阶段失败后，第二次调用能进到落地（钉住 guard 析构；预留若落了盘这条必红）
  - 二次调用（已成形）被拒；空 brief 被拒；未知 surface 被拒
- **§C.1.4 hash 不变量**：断言首次落地前该应用没有数据库；并有一条测试证明「若数据库已存在则拒绝改 `manifest.name`」
- **`detect_build_target`**：空壳给「先定形态」；`scaffolded == true` 且 `surface == None` 报存储损坏（各自独立断言）
- **session 标题条件改名**：正例——最新 `custom-title` 仍带 `mobileEmptySession: 1` 时，Scaffold 成功后改为应用名；失败后由 boot sweep 用同一判据补偿。⚠️ 反例——intake 期间先 `/rename` 再 Scaffold，及 `/rename` 后再跑 sweep，标题都保持用户值（钉住即时与启动补偿都不抹用户标题）
- ⛔ **不要改** `create_app_enforces_brief_caps` 与 `create_app_rejects_an_empty_brief`（§A.3）：它们走默认包装 = `Scaffolded`，必须继续绿。改了就等于拆掉该路径上唯一钉住空-brief 不变量的两条测试

⚠️ 跑法：`cargo test --workspace --all-features --no-fail-fast`。engine-mobile 的 local-apps 模块是 `#[cfg(feature = "uniffi")]`，不加 `--all-features` 整块被跳过。全量输出落文件再 grep（只 grep `FAILED` 会丢掉 `failures:` 块里的测试名），并盯**测试总数**是否下降。

### 客户端
⚠️ brief 认领机制**纯粹在客户端**，不属于 Rust 段：`clients/ios/Tests/LocalAppsStoreTests.swift`（`:1571-1656`）与 `clients/android/.../LocalAppsViewModelTest.kt`（`:302-528`），**每端各约 9-12 条**，不是总共 9 条。

另需：`request_id` 认领（含**不匹配的事件必须被忽略**、超时清理、断线重连不误认领）；草稿卡片渲染；§A.4 表里**每一处**渲染点不泄漏占位串；**widget 快照不含 `scaffolded == false` 的应用**；「+」不再弹表单；kickoff 文案；两端的 widget 常驻入口。

⚠️ 改 wire 之后**必须先重新生成 uniffi 绑定再编客户端**：
- `bash clients/ios/scripts/build-xcframework.sh && (cd clients/ios && xcodegen generate)`
- `bash clients/android/scripts/build-jni.sh --variant play`

## G. 真机验收

**不接受「编过了」。**

**G.-1（clean install 前提）**：**重装覆盖安装**（在既有 app 上直接装新包）。⛔ **绝不先卸载再装。**

**为什么**：本地应用整个存放在 app 的私有数据里。卸载会把这份数据一并删掉，也就是删掉设备上**所有**本地应用——那是用户自己做出来的东西，除这台设备外没有第二份副本，删掉不可恢复。「先卸载求个干净」在这条链路上是**破坏性操作**，不是清理手段。

**确实需要一个空的本地应用库时**（只在开发机上这么做）：

- **两端通用（首选）**：重装覆盖后，在 app 的本地应用库里把残留应用逐个删掉。
- **Android**：系统设置 → 应用 → 本 app → 存储 → **清除数据**（清数据 ≠ 卸载，app 本身保留）。
- **iOS**：系统里没有等价的「清除数据」——iOS 上删除 App 就等于卸载，所以**只能**用上面那条应用内删除；不要为了清库去删 App。

启动后确认本地应用库为空。**不拿残留旧 store 验收本方案。**

**G.0（§0 单独验收，先做）**：在干净安装上先创建一个测试用本地应用，再进入它的会话，让代理复述 `LINGXI.md` 里只在该文件出现过的一条约束。修复前它应当答不出来。

1. 点「+」→ **不弹任何表单**，直接进对话，代理第一句在问你想做什么
2. 回答「打飞机」→ 代理提议名称，并用**原生选项**让你确认形态，且它选的是 `canvas`
3. 确认后应用成形、草稿标记消失、一路能构建出可玩的东西
4. 反向用例：第 1 步就退出 → 库里留一张「创建中」卡片，点回去续上**同一个**对话
5. **两端 widget 入口**：iOS 详情页、Android 新承载点各加一次成功

## H. 风险与已知留口

- **工具门只是提示，正确性由 §C.0.1 的清空提供。** 判据：确认之前写的代码，一个字节都不能进入正式应用。
- **成形之后**的同名异扩展影子文件（`app/app.js` 在 Vite 解析里赢过 `app/app.jsx`）**仍然可行**——那是既有面，今天任何已成形应用都可以，本方案既不引入也不扩大（§C.0.3）。**本期不修，单独立项。**
- ⛔ 初稿写的「Bash 能绕过一切写入禁止」**是错的**，已删：移动端 shell 工具名为 `Shell`，且 `.localApp` 会话里 `deny_workspace_host_owned` 已硬拒非只读巡检类命令。别再把它当作论据。
- **门保证不了「代理真的问过用户」。** 它可能问完第一句就自作主张调 `LocalAppScaffold`。只能靠提示词，验收 G.2 就是在验它。若真机反复不过，下一步是让 `LocalAppScaffold` 要求一个「用户已确认」的证据字段——同样可被编造，本期不做。
- **空壳会堆积**，没有自动清理，靠用户删。
- **`AppRecord` 多了一个持久字段**，相对最初「不新增状态」的承诺是一次让步（§A.2）。
- **旧 store 不受支持**（§A.1）。这是未发布开发版的 clean-install 决策，不实现迁移，也绝不把缺字段记录静默解释成可被清空的 shell；开发机残留数据按 §G.-1 清除（⛔ 不要卸载）。
- **8.0.0 的破坏面**：只有**桌面端**（Electron / `clients/shared`）在握手时硬失败。**iOS/Android 走 UniFFI 进程内调用，既没有握手也没有版本交换**（`grep CLIENT_PROTOCOL_VERSION` 在 `engine-mobile` / `ios-framework` / `android-aar` 零命中），失败形式是重新生成绑定后**编译不过**。
- **跨计划协议与共享文件冲突**按 §B.4 的 master order 串行处理；不得另开一份临时顺序或重复实现共享补丁。

## I. Local App 2D/3D 游戏引擎实施方案

本节建立在 §0-§H 的对话式创建链路上，不另造一条创建入口。交付分成两个阶段：先为 iOS/Android WebView Local App 增加可选 Web runtime profile；待 desktop Local App host 能管理非 WebView 项目后，再增加 Godot 原生项目。

### I.1 实施门与交付顺序

1. **先合并当前 create-local-app 重构。** §0-§H、协议 8.0.0、`LocalAppScaffold`、新创建入口行为和两条现有 workflow 必须先独立落地并通过 §F/§G。
2. **游戏引擎工作不得与该重构并行修改** `skills/create-local-app/**`、`LocalAppCreate` / `LocalAppScaffold` schema、`local-app-build` 或 `local-canvas-build`。Phase 1 只能 rebase 到合并后的接口；若基线接口仍在变，停止引擎实现，不维护两套临时兼容层。
3. **Phase 1.0：IIFE 可行性门。** 在新增 enum、manifest 字段或工具 schema **之前**，先用固定 pnpm lock 和现有 `format: "iife"` / `inlineDynamicImports: true` 配置完成 Babylon core + glTF loader + Havok 的真实 production build，并在 iOS/Android WebView 各启动一次、载入 glTF、初始化 Havok、捕获两帧。只验证 import 不算通过。若失败，停止 Phase 1，回到设计评审；不得先落公共接口，也不得静默把公共 Vite 配置改成 ESM/code-splitting。
4. **Phase 1：WebView profile。** Phase 1.0 通过后，保持 `AppSurface::{Dom, Canvas}`，增加五个持久 runtime profile、profile-aware scaffold/lock/cache、创建推荐与 WebView 生命周期适配器。
5. **Phase 2：Godot NativeGame。** 只有 desktop Local App host 已能创建、打开、运行和删除非 WebView 项目，且 §I.7 要求的独立 design/security review 已通过后才启动；不把 Godot 塞进 `AppSurface::Canvas`。

Phase 1 是当前 design 的后续里程碑，不是 §0-§H 的合并阻塞项；Phase 2 又以 Phase 1、desktop host 和独立安全设计为前置。每一阶段单独提交、单独验收，禁止半落地的 profile 字段先于对应 locked scaffold 上线。

### I.2 持久化模型、公共接口与兼容性

新增持久枚举：

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AppRuntimeProfile {
    #[serde(rename = "react_dom")]
    ReactDom,
    #[serde(rename = "canvas_2d")]
    Canvas2d,
    #[serde(rename = "three_3d")]
    Three3d,
    #[serde(rename = "phaser_2d")]
    Phaser2d,
    #[serde(rename = "babylon_3d")]
    Babylon3d,
}
```

⚠️ **必须逐变体 `rename`，不能用 `rename_all = "snake_case"`。** serde 的 `SnakeCase` 只在**大写字母**前插下划线，**数字不断词** ⇒ `Canvas2d` → `canvas2d`、`Three3d` → `three3d`、`Phaser2d` → `phaser2d`、`Babylon3d` → `babylon3d`，五个里四个与本节全篇（映射表、推荐表、依赖表、workflow 路由、测试）用的 `canvas_2d` / `three_3d` / `phaser_2d` / `babylon_3d` **对不上**；只有 `ReactDom → react_dom` 碰巧正确。现有 `AppSurface` 的 `Dom` / `Canvas` 不含数字，所以这个坑至今没暴露过。

两种走法都会坏：直接 serde 反序列化则按表写的 `"canvas_2d"` 被拒为 unknown variant，五分之四的 profile 创建全挂；另写手写 `parse()/as_str()`（`AppSurface` 就是这个范式）则**入参/receipt/SBOM 用 `canvas_2d`、manifest 落盘字节是 `canvas2d`**，同一个值两套拼写。

⛔ **发现晚了就改不动了**：profile 进 `AppManifest::hash()`（`manifest.rs:460` 序列化整个结构体）且首次 scaffold 后不可修改，改拼写会改掉每个带 profile 应用的 hash，`AppDataStore::ensure_manifest`（`data.rs:597`）随即对**所有数据读写**报 `database manifest mismatch`——正是 §C.1.4 要防的那类损坏。

**持久拼写以本节映射表为准。** §I.7.1 的 `godot_2d` / `godot_3d` 落 Rust enum 时同一个坑，一并逐变体 `rename`。

`AppManifest` 追加以下字段；serde 形状是兼容性契约的一部分，不能省略：

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub runtime_profile: Option<AppRuntimeProfile>,
```

它与 `AppRecord.scaffolded` 的兼容策略不同：

- Phase 1 创建的新 WebView 应用必须持久化 `Some(profile)`；
- 缺字段或显式 `None` 且 `surface == Some(_)` 表示 **legacy v1 scaffold**，继续使用现有 v1 host-managed 文件和 lockfile；`surface == None` 仍走当前 pre-scaffold-split 的不可重建拒绝，不因新增 profile 获得新语义；
- 读取 legacy manifest 时不推断、不回填，也不从源码、依赖树或 `surface` 猜 profile；
- 这条兼容只保护无 profile 的 DOM、Canvas 与 Three.js v1 应用/fixtures，不撤销 §A.1 对旧 `AppRecord.scaffolded` store 的 clean-install 决策。

profile 到 surface 的映射是公共不变量：

| profile | surface |
|---|---|
| `react_dom` | `dom` |
| `canvas_2d` | `canvas` |
| `three_3d` | `canvas` |
| `phaser_2d` | `canvas` |
| `babylon_3d` | `canvas` |

`LocalAppCreate` 追加可选 `runtime_profile`，保留现有 `surface`：

| 输入 | 行为 |
|---|---|
| 有 profile、无 surface | 由上表派生 surface，走对应 profile scaffold |
| profile 与 surface 都有且一致 | 接受，profile 是权威值 |
| profile 与 surface 冲突 | 在写记录/manifest 前返回校验错误 |
| 只有 surface | **按映射表反查到 engine-free 的 profile**（`dom` → `react_dom`，`canvas` → `canvas_2d`）并持久化 |
| profile 与 surface 都缺省 | 同上：归一成 `surface = dom` ⇒ `react_dom` |

⚠️ **切法是「新建 vs 既存」，不是「新客户端 vs 旧客户端」。** 初稿把 surface-only 归给「旧客户端兼容路径 ⇒ 创建 legacy v1 scaffold」，有两处错：

1. **默认调用就走这条路。** `LocalAppCreate{brief:"一个待办清单"}` 里 `runtime_profile` 可选、`surface` 也可选且 schema 写着 `Defaults to "dom"`，dispatch 归一成 `AppSurface::Dom`（`local_apps_mcp.rs:1316`）。于是**每一次默认创建**都落进 legacy v1；而 §I.4 规定 Full 包只带 engine-free base seed、v1 lock 不再拥有 bundled seed ⇒ 首次构建必须联网解析全套依赖。`scripts/mobile-linux/build-local-app-node-modules.sh` 开头写明它存在的理由正是消灭这个回归（「every device resolved the same 169 packages over the network on first `create_local_app`」）。
2. **`LocalAppCreate` 没有「旧客户端」。** 它是编进同一个二进制的 builtin，schema 与 `include_str!` 打包的 `SKILL.md` 随引擎一起升级，不存在版本落后的调用方。

「不能把 surface-only 的 `canvas` 自动升级」这条理由只对**已经装好依赖树的既存应用**成立——新建应用根本没有旧 lockfile 可被替换。因此：**Phase 1 之后任何新建都必须落一个 profile**；「继续读 v1 lock、不推断不回填」只保留给**已经存在**的无 profile 应用（§I.2 的 legacy 读取规则）。

⇒ §I.4 的 schema 描述同批修正：`surface` 的 `Defaults to "dom"` 要改成说明它只在缺 `runtime_profile` 时用于反查 profile。

Phase 1 不让生成代理直接决定对话式创建的最终 profile。§C.1 的 `LocalAppScaffold` 入参改为 `{app_id, name, brief, profile_confirmation_id, workflow_model?}`；host 用 `profile_confirmation_id` 读取用户在 §I.3 亲自选择的 profile 并派生 surface，schema 不再接受可覆盖它的 `runtime_profile` / `surface`。`LocalAppScaffold` 提交时一次性写入 receipt 绑定的 profile 和派生 surface，仍以 `record.scaffolded = true` 为最后 commit point。`LocalAppCreate` 的直接创建兼容矩阵保持上表行为，不复用这张对话 receipt。

profile 创建后不可修改：

- `update_manifest`、重建、依赖安装与 workflow 都不得接受替换 profile 的参数；
- host-managed restore 和 build 始终读取 manifest 中已经提交的 profile；
- 需要换引擎时创建新应用，避免 package、lockfile、模板适配器与依赖 snapshot 被隐式替换；
- manifest 有 profile 时，若 surface 与映射表不一致，按存储损坏拒绝，不自动修复。

`Some(runtime_profile)` 和 `surface` 都会进入 `AppManifest::hash()`，因此沿用 §C.1.4 的数据库不变量：两者必须在首次 scaffold、任何 data store 打开之前一起写定，此后禁止修改。`None` 必须因 `skip_serializing_if` 完全不进入序列化字节；测试要用一份无 profile 的 v1 manifest 证明 load 前后的 JSON/hash 字节不变、已有 SQLite 不发生 mismatch，并证明已有数据库时替换 profile 被拒，而不是等到下一次数据访问才报错。

`detect_build_target` 在 Phase 1 扩展为 profile-aware：先保留 §C.4 的 `scaffolded + surface` 完整性检查，再检查 profile/surface 映射；`Some(profile)` 选择对应 locked file set，`None` 选择 legacy v1。绝不通过 `package.json` 或 import 反推 profile。

### I.3 智能推荐、用户覆盖与确认边界

`skills/create-local-app/SKILL.md` 负责从已确认规格中评分并推荐 profile，固定规则如下：

| 需求信号 | 推荐 profile |
|---|---|
| 表单、列表、页面导航 | `react_dom` |
| 简单绘制、模拟或极小游戏 | `canvas_2d` |
| 自定义着色器或轻量 3D | `three_3d` |
| 精灵、Tilemap、2D 动画或碰撞 | `phaser_2d` |
| 3D 模型、场景、动画或刚体物理 | `babylon_3d` |

混合需求不能靠隐藏优先级静默决胜：若两类信号都会实质改变引擎选择，代理先追问主玩法/渲染需求，再给一个推荐与理由；用户始终可以覆盖推荐。

需求澄清仍发生在本方案的对话里，不重新引入被 §D 删除的两步创建表单；普通追问继续用 `AskUserQuestion`。「创建前可以修改」在这里指 **`LocalAppScaffold` commit 前**；空壳记录和 pin 会话已经存在。

最终 profile 选择改由新的 host-mediated `LocalAppConfirmRuntimeProfile`（operation `confirm_runtime_profile`）完成。生成代理只提交 `{app_id, recommended_profile, reason}`；host 自己渲染原生 picker，用户的点击不经过模型改写。确认界面必须显示：

- 推荐 profile 与基于用户需求的理由；
- 该 profile 当前的依赖准备状态；
- 五个可选 profile 和用户最终选择。

为避免代理猜测缓存状态，Phase 1 新增只读 `LocalAppRuntimeProfiles` 工具（operation `runtime_profiles`）：返回 profile、surface、固定引擎版本与下面的 `download_status`，不触发网络或安装：

| 状态 | 含义 / UI 文案 |
|---|---|
| `ready_snapshot` | 已有验证过的 snapshot，不需要下载 |
| `ready_bundled_base` | 该 engine-free lock 与主应用的 base seed 精确匹配，不需要下载 |
| `download_may_be_required` | 没有可验证的离线完整树，首次准备可能下载；用户确认即授权网络 |

共享 pnpm store 只作为 `--prefer-offline` 的机会性加速：除非 host 能在不修改 store/应用的前提下证明完整 lock 可离线解析，否则不能把 `download_may_be_required` 降成「不需要下载」。确认 UI 在安装前再次读取状态，所以 snapshot/seed 在对话期间发生变化不会显示旧结论。

`LocalAppConfirmRuntimeProfile` 返回 `{selected_profile, profile_confirmation_id}`。receipt 是 host 进程内保存的不可预测随机 id，绑定 `app_id + selected_profile`，有效期固定 10 分钟：

- **每个 `app_id` 同时只允许一张未消费 receipt**：签发新 receipt 时**原子作废**该 app 此前所有未消费的 receipt（实现成 per-app 单槽，而不是 id → receipt 的映射）；
- `LocalAppScaffold` 在取得 §C.1 的进程内 scaffold 预留后校验并 claim receipt；过期、跨 app、未知、已消费或**已被更新的确认取代（superseded）**的 receipt 一律拒绝；
- manifest/profile 提交成功后消费 receipt；落地失败则释放 claim，只要仍在 TTL 内即可重试；
- receipt 不落盘，进程重启后失效，用户重新确认即可；这不会把草稿变砖；
- schema 不接受另一个 profile，所以代理无法把用户选的 `phaser_2d` 换成 `babylon_3d`。

⚠️ **单槽那条是「代理不能改写选择」成立的必要条件，不是锦上添花。** 没有它，重放旧回执就能绕过整条防线：用户先选 `phaser_2d`（receipt A），改主意再选 `babylon_3d`（receipt B），代理提交仍在 TTL 内的 A —— A 既没过期、也没跨 app、更没被消费，四条拒绝理由一条都不占，**被接受**，应用永久钉成用户已经否决的引擎（profile 提交后不可修改）。「schema 不接受另一个 profile」只堵住了**参数注入**，堵不住**回执选择**：代理根本不需要伪造 profile 字符串。

若落地在 `record.scaffolded = true` 之前失败且 receipt 随后过期/重启失效，用户可以重新确认其他 profile；此时必须再次断言 `scaffolded == false` 且不存在 data store，并按 §C.0 清空后重写暂存 manifest/files。只有 `scaffolded = true` 的 commit 之后 profile 才进入不可变期。

`runtime_profiles` 与 `confirm_runtime_profile` 都加入空壳工具门的放行集合；工具 catalog、默认权限计数和精确名字测试按 Phase 1 的实际表重新生成，不沿用 §C.1.6 的旧硬编码数。

用户确认后的 profile 由 `LocalAppScaffold` 从 receipt 持久化。后续 `local-app-build` / `local-canvas-build` **只能读取已提交 profile**，workflow 参数不允许另带一个可覆盖值；正式 `LINGXI.md` 同步写入 profile。安装或构建失败也不改变 profile，用户只能重试或新建应用。

用户确认 profile 即授权该 profile 的首次按需依赖下载；确认页不承诺下载耗时。真正安装前仍由 host 在 snapshot 锁内重新检查 seed/snapshot，避免确认期间缓存状态变化产生竞态。

### I.4 Phase 1：依赖、pnpm 构建与内容寻址缓存

host-managed 文件拆成两层：

1. 所有 Web profile 共用的固定 Vite / React / Ionic 基础；
2. profile 专属的 `package.json`、`pnpm-lock.yaml`、入口种子与运行时适配器。

每个 profile 有独立、完全锁定的 package/lock：

| profile | 额外引擎依赖 |
|---|---|
| `react_dom` | 无 |
| `canvas_2d` | 无 |
| `three_3d` | `three@0.185.1` |
| `phaser_2d` | `phaser@4.2.1` |
| `babylon_3d` | `@babylonjs/core@9.22.1`、`@babylonjs/loaders@9.22.1`、`@babylonjs/havok@1.3.14` |

「独立」指每个 profile 都有受审计的 package/lock 所有权，不要求内容刻意不同。`react_dom` 与 `canvas_2d` 若依赖集合完全相同，可以拥有字节一致的 lock，并安全共享同一个 dependency snapshot；adapter/source 不进入依赖树身份。

**包管理器锁死为 pnpm。** 依赖解析、安装、lockfile 和 snapshot 发布全部复用现有 host-owned `pnpm install` 路径；production build 继续调用由该 pnpm 安装物化出的固定 Vite entry。应用工作区、技能、workflow 和文档都不得建议或执行 `npm install`、`npm ci`、`npx`、Yarn，也不运行 package-manager scaffold 命令。

所有引擎依赖均按需下载，不随 LingXi 主应用内置。**Phase 1 必须替换当前 bundled seed，不能继续从含 `three@0.185.1` 的 v1 lock 构建它：**

- Full iOS/Android runtime 只允许携带一个 engine-free base dependency seed；producer 以 `react_dom` / `canvas_2d` 的 engine-free lock 为输入；
- staged `node_modules`、runtime pins、许可证清单和 bundled SPDX SBOM 都必须断言不存在 `three`、`phaser`、`@babylonjs/*`、Havok/WASM；
- surface-only legacy v1 app 仍读取原 v1 lock，但该 lock 不再拥有 bundled seed；没有 snapshot 时走正常 pnpm 按需安装；
- `three_3d` 即使与今天的 v1 依赖集合接近，也不得命中旧的主应用 seed；旧 seed producer/output 必须在同一变更中被 engine-free base seed 取代。

`install_dependencies` 扩展为 profile-aware，但不新增下载服务：

1. 读取已持久化 profile 和该 profile 的固定 package/lock；
2. 沿用现有内容寻址：cache root 由固定 pnpm/toolchain 版本分区，snapshot key 就是 `pnpm-lock.yaml` 的 SHA-256；**profile 不进入 key**，因为相同 lock 产生相同 dependency tree；
3. snapshot ready 时直接物化到应用工作区，不访问网络；
4. 未命中时在 host-owned 环境执行 `pnpm install --frozen-lockfile`，验证后原子发布 snapshot；
5. 相同 lock 与 toolchain 的后续应用复用 snapshot，即使它们属于两个依赖完全相同的 profile。

DOM 与 Canvas2D 的 lockfile、依赖树证明和构建产物不得出现 Three、Phaser、Babylon 或 Havok。各 profile 的依赖集合互不并入「大一统」lock；否则普通应用会为游戏引擎付下载、供应链与产物体积成本。

无网络且 snapshot 不存在、下载中断或摘要不符时：保留应用记录、manifest、源码和已确认 profile，返回**可重试的依赖失败**；绝不降级到 Canvas2D、Three 或另一引擎。失败 snapshot 不发布，临时目录由现有依赖 staging 清理。

供给链验证按 profile 生成并校验：

- package 与 `pnpm-lock.yaml` SHA-256；
- pnpm 实际依赖树证明；
- 许可证清单；
- SPDX SBOM；
- snapshot marker 中的版本、toolchain、lock digest 与 tree digest；
- 每应用 provenance/SBOM 中的 profile→lock 映射。profile 用于审计和诊断，不改变 snapshot 内容地址。

### I.5 Phase 1：模板适配器与 WebView 生命周期

Canvas workflow 不再统一要求 `createFrameLoop`。各 profile 只使用自己的生命周期所有者：

- **React DOM**：保留现有 Vite + React/Ionic routed UI；不创建游戏帧循环。
- **Canvas2D**：保留现有 DPR、resize、visibility、最大模拟步长与 teardown-safe `createFrameLoop`。
- **Three**：adapter 统一 `WebGLRenderer` 的 size/pixel ratio、WebGL context lost/restored、animation loop 停止与 geometry/material/texture/renderer dispose。
- **Phaser**：Phaser 自己创建并拥有 canvas、scene 与 frame loop；React 不再包一层逐帧循环。销毁时调用引擎 lifecycle，解除输入和 resize listener。
- **Babylon**：Babylon 自己拥有 engine、scene 与 render loop。默认 WebGL2；WebGPU 只在 feature detect 成功后显式启用，失败仍按 WebGL2 路径创建，而不是运行中偷偷换 profile。
- **Babylon physics**：profile 固定包含 Havok 包，但仅需求包含物理时初始化 Havok WASM；初始化失败返回可重试错误，不替换物理实现。场景销毁时释放 physics plugin、scene、engine、WASM/渲染资源和 listener。

React/Ionic 只管理菜单、暂停、设置、Game Over 与 LingXi bridge。逐帧位置、速度、动画、粒子和碰撞状态禁止写入 React state 或 Zustand；游戏引擎/adapter 是高频状态的唯一所有者。

游戏资源随应用写入 `public/`，不依赖外部 CDN；单文件继续受现有 32 MiB 静态资源限制。引擎自带的 host-managed WASM/运行时资产由 locked dependency build 管理，不允许生成代理用 CDN 绕开 pnpm lock 与 SBOM。

### I.6 Phase 1：技能、workflow 与运行中保护

重构合并后修改 `create-local-app`：规格收集完成后执行 §I.3 的评分、读取 profile/cache catalog、展示原生确认，再调用扩展后的 `LocalAppScaffold`。确认之前仍遵守 §C.0 的空壳清空保证。

workflow 路由保持两条：

- `react_dom` → `local-app-build`；
- `canvas_2d`、`three_3d`、`phaser_2d`、`babylon_3d` → `local-canvas-build`。

`local-canvas-build` 根据 manifest profile 生成对应 adapter/引擎代码，不从 spec 重新猜引擎，也不接受覆盖 profile 的 workflow args。共享的 repair/verify core 继续复用；profile 专属形状、证据和 lifecycle 检查留在 Canvas workflow 侧。

必须先修现有 workflow 身份遗漏：

- `tasks/src/handlers/local_workflow.rs` 当前只对 `workflow_id == "local-app-build"` 获取 workspace lease；改为同时识别两条 Local App build workflow；
- `tasks/src/registry.rs::find_nonterminal_local_app_workflows` 当前也只识别 `local-app-build`；删除保护必须同时覆盖 `local-canvas-build`；
- `tools/workflow/src/lib.rs` 已有两条 workflow 的身份集合，所有相关判定改成集合/公共 predicate，禁止再写单个字符串相等；
- 表驱动测试逐条证明两条 workflow 都只能绑定目标应用 workspace、运行期间都阻止删除，非 Local App workflow 不受影响。

⚠️ 这组修复的**唯一实现 owner** 是 master order 的 verification Phase 1。本文把它列作 runtime-profile 的进入条件和回归测试，不允许 Phase 1 再提交第二份同义补丁；若上游尚未落地，按 master order 先完成共享基础步骤。

生产构建继续使用固定 IIFE Vite 配置。§I.1 的 Phase 1.0 是公共接口开工门，不是最终 QA 才跑的测试；通过后，Phaser、Babylon、glTF loader 与 Havok WASM 的真实 frozen pnpm install + production build + WebView launch 仍作为持续回归。只做 TypeScript/import 验证不算完成。若后续升级重新撞到 IIFE/code-splitting/WASM 构建限制，先停止对应 profile 发布并重新设计 build contract；不得顺手改动所有 profile 共用的 Vite 配置。重复 build/restore 必须继续使用 manifest 中的同一 profile 与 lock。

### I.7 Phase 2：Godot 原生模式

前置条件：desktop Local App host 已能创建、打开、运行、终止、删除并恢复**非 WebView**项目；移动 host 不进入本阶段。**本节只锁定 Phase 2 的产品边界，不授权直接编码。** 在任何 Godot 代码落地前，必须先提交并通过一份独立的 NativeGame design + threat model，至少完整定义下面两组契约。

#### I.7.1 最小持久化与公共接口

- desktop project catalog 有持久 discriminator：`WebView` 与 `NativeGame`。`NativeGame` 不复用 `AppSurface::Canvas`，也不进入 Web profile 枚举。
- NativeGame manifest 至少持久化 `{project_kind: native_game, runtime: godot, profile: godot_2d|godot_3d, engine_version, toolchain_digest}`；profile 与 engine version 创建后不可隐式替换。
- desktop-only create request/DTO 明确携带 `godot_2d` / `godot_3d`；移动协议不能构造该项目类型。
- desktop host 提供创建、查询 toolchain/download、打开 editor、run、stop、headless export 和删除操作；每个 operation 的错误/重试语义、进程 pid/ownership 与事件 DTO 必须在独立 spec 中 bless。
- 生命周期至少区分 `creating`、`downloading_toolchain`、`ready`、`running`、`exporting`、`failed`；下载或导出失败保留项目并能重试。
- editor、run、export、checkpoint/restore 与物理删除共用同一项目级 lease/存储锁；运行中删除和重复并发 export 被拒或有序等待。

若 desktop host 合并后的领域模型无法自然承载这些字段，Phase 2 应创建独立 NativeGame service/manifest，而不是向 mobile `AppManifest` 或 `AppSurface` 追加 Godot 分支。没有以上持久化/API 设计，§I.8.3 的验收不允许启动。

#### I.7.2 工具链与项目执行信任边界

- 第一版固定使用 [Godot 4.7.2 Standard](https://godotengine.org/download/archive/4.7.2-stable/)，不支持 .NET/C#。
- 按当前 desktop OS/architecture 按需取得官方 Standard editor/runtime artifact 与 export templates；URL、版本、字节大小和 SHA-256 写入受版本控制的 toolchain manifest，下载后按该本地权威验证再进入版本化缓存，不在运行时信任远端最新版本或远端 checksum。若某平台用同一 Standard executable 的 `--headless` 模式运行，则 editor/headless 可以指向同一已验证 binary；不要假设官方一定发布独立 headless 文件。
- archive 解包必须拒绝绝对路径、`..` 穿越、逃逸 symlink/hardlink、重复覆盖和非预期 executable；校验完整 inventory 后才原子发布 toolchain cache。
- **固定 Godot binary 的摘要不等于项目可信。** editor、run 和 export 每次都在 desktop sandbox 内启动：只允许项目根、只读 toolchain cache、专用 user/cache/temp 与本项目 export 根；默认断网；拒绝访问其他 LingXi 应用、会话、用户 home 与任意宿主路径；默认只允许 Godot 自身，若当前 OS export 确实需要 helper，helper 也必须进入 toolchain manifest、固定摘要并显式 allowlist，不能泛化成任意子进程权限。
- 首版项目 preflight 拒绝 `.gdextension`、`.so` / `.dylib` / `.dll`、`addons/**`、EditorPlugin/EditorScript、import plugin 和带 `@tool` 的脚本。preflight 是清晰报错层，**sandbox 才是不可绕过保证**；用户在已打开的 editor 中修改项目也不能突破宿主边界。
- 提供基础 2D/3D 场景、启动编辑器、运行项目和 headless export。首版只导出当前 desktop OS 的可执行物；Android/iOS SDK、签名、商店发布与跨平台 export 作为后续里程碑。
- Godot 项目不使用 `window.lingxi.v2`。需要 LingXi 原生能力时另行设计受审计的 Godot extension；首版不注入隐式 bridge，也不让 WebView workflow 操作 Godot 项目。

所有 Godot artifact 同样按需下载，不随主应用内置。无缓存且离线时保留项目并返回可重试下载错误；摘要不符、inventory 不符或 sandbox 不可用时不得启动 editor/headless/export binary。

### I.8 测试与验收

#### I.8.1 Phase 1 功能与兼容性

- profile 推荐、用户覆盖、profile→surface 映射、冲突校验和 profile 不可变性都有单元测试。
- **Phase 1.0 先行证据**：在任何 profile enum/schema commit 之前保存 Babylon + glTF + Havok 的固定 IIFE production build 日志与 iOS/Android WebView smoke 结果；该证据缺失时 Phase 1 不开工。
- `runtime_profile` 与 `surface` 在 data store 打开前一次性写定；已有数据库时替换任一字段都被拒。
- 无 profile 的 v1 manifest 使用 `skip_serializing_if`：load 后的 canonical JSON/hash 与加字段前 fixture 完全一致，已有 SQLite 继续打开；`surface == None` 仍保持原不可重建拒绝。
- `LocalAppRuntimeProfiles.download_status` 分别覆盖 ready snapshot、精确 engine-free bundled seed 与没有可验证离线树；查询本身不访问网络、不修改 app/snapshot/store。
- **持久拼写钉死**：对五个 profile 各序列化一次 manifest，断言字节里出现的正是映射表中的 `react_dom` / `canvas_2d` / `three_3d` / `phaser_2d` / `babylon_3d`。这条同时钉住 serde 侧与手写 `parse()/as_str()` 侧不漂移——⛔ 用 `rename_all` 会让其中四条直接红（见 §I.2）。
- **默认创建仍命中 engine-free base seed**：不带 `runtime_profile`（`surface` 也缺省）的 `LocalAppCreate` 在**无网络**设备上完成首次依赖安装与构建，并断言落盘 profile 是 `react_dom`；`surface: "canvas"` 无 profile 的同款用例落 `canvas_2d`。
- **superseded receipt 被拒**：对同一个 app 连续确认两次（先 `phaser_2d` 后 `babylon_3d`），提交**第一张** receipt 必须被拒，且成功提交后落盘的是用户第二次的选择。
- host-native picker 的用户选择与 receipt 绑定：跨 app、过期、伪造、重复消费和 receipt/profile 覆盖均被拒；落地失败释放 claim，成功提交后消费；进程重启后要求重新确认。另测一次 pre-commit 失败后重新确认其他 profile：仅在 `scaffolded == false` 且无 data store 时允许清空重写，commit 后永远拒绝。
- 五种 Web profile 均能创建、frozen pnpm install、production build、启动和重建。
- 无 profile 的 legacy DOM、Canvas 和 Three.js v1 fixtures 继续走原 scaffold/lock 并通过原测试；加载不会回填 profile。
- 在清空 snapshot、共享 pnpm store 且不存在 exact bundled seed 的 hermetic 设备上，第一个 `three_3d` / `phaser_2d` / `babylon_3d` 安装发生网络访问；第二个相同 lock/toolchain 的应用命中 snapshot，全程不访问网络。`react_dom` / `canvas_2d` 可以命中 engine-free base seed，不要求伪造一次网络访问。
- DOM/Canvas2D 的 package、lockfile、依赖树、SBOM 和产物不包含 Three、Phaser、Babylon 或 Havok；两端 Full 主应用内的 staged dependency seed、runtime pins、许可证清单和 bundled SBOM 也不包含任何游戏引擎。
- 两个 profile 使用字节相同 lock 时命中同一个 snapshot root；profile 变化但 lock/toolchain 不变不会复制 dependency tree。
- 离线缓存缺失、下载中断、摘要不符、Havok WASM 初始化失败和 WebGL context loss 都产生明确、可重试错误，不静默切换 profile 或引擎。
- Phaser 与 Babylon 的 WebView 测试至少捕获两帧不同图像，并覆盖 pointer、键盘、旋转、后台恢复、context loss 和销毁。
- profile 确认后，workflow 读取 manifest 中的选择；尝试从 workflow 参数换 profile 被拒。
- `local-app-build` 与 `local-canvas-build` 都取得相同语义的 workspace lease，并在运行中阻止删除目标应用。
- 负向测试扫描技能、workflow 与 host 文案，禁止 `npm install`、`npm ci`、`npx`、Yarn；唯一允许的包管理链路是 host-owned pnpm。
- skill validator、workflow tests、Rust workspace tests、Android/iOS Local App tests、供给链验证和 SPDX SBOM 验证全部通过。

#### I.8.2 Phase 1 性能基线

以测试执行时近三代主流 iPhone 与 Android 真机为 60 FPS 基线。设备型号、OS、WebView 版本、电源/温控状态和每个 profile 的版本化 smoke scene 必须随结果记录；没有固定 scene/镜头/输入脚本的数据不可相互比较。

- 标准 Phaser/Babylon smoke scene 预热后中位帧率 ≥55 FPS；
- p95 frame time ≤25 ms；
- 后台恢复后的首个模拟步长 ≤66 ms；
- 输入到视觉响应 p95 ≤80 ms；
- 连续 mount/unmount 20 次后 JS heap 增长 ≤稳定值的 10%；
- DOM profile 相比当前 DOM 基线的 production 产物体积增长 ≤1%；
- 记录每个 profile 的首次依赖安装时间、snapshot 命中率、构建时间、产物大小、运行内存和失败阶段。

heap 与输入延迟通过测试 instrumentation 采集，不把调试探针带进 production bundle。性能 gate 失败时先缩减 smoke scene 或优化 adapter，不通过降低指标、切换 profile 或关闭供应链检查放行。

#### I.8.3 Phase 2 Godot 验收

- 独立 NativeGame design/threat model 已通过；持久 project kind、create/事件 DTO、desktop-only lifecycle tools 与项目级锁都有契约测试，否则本节全部测试不得作为实现开工替代品；
- 无 Godot 缓存时只展示一次下载，完成后验证摘要并原子发布缓存；
- 已缓存时可以离线创建、打开、运行和导出示例项目；
- `godot_2d` 与 `godot_3d` 模板均通过 headless smoke test；
- editor/headless/export 进程崩溃不会损坏项目；临时导出被清理，最后一次成功导出物继续保留；
- 摘要不符、下载中断、运行中删除和重复并发导出都有负向测试；
- 恶意 archive 的路径穿越/symlink、项目内 `.gdextension`/native library/addon/`@tool`/editor plugin 均在启动前拒绝；
- sandbox canary 证明 editor、run 与 export 不能读取项目外文件、写入非专用根、访问网络或启动未批准子进程；sandbox 不可用时 fail closed；
- 首版只验证当前 desktop OS，不把移动签名或商店发布算进完成条件。

### I.9 实施所有权与主要文件

Phase 1 预期影响以下所有权边界；实现时先以合并后的代码图重新核对，不能机械照抄当前行号：

- `local-apps/src/manifest.rs` — `AppRuntimeProfile`、带 `default + skip_serializing_if` 的 optional manifest 字段、映射/不可变性/legacy v1 hash 校验；
- `apps/engine-mobile/src/local_apps_build.rs` — profile-specific locked files、build target 与 production build；
- `apps/engine-mobile/src/local_apps_host.rs` — pnpm install/snapshot、engine-free seed adoption、profile receipt/picker、profile-aware scaffold、正式 `LINGXI.md` 与生命周期资产；
- `apps/engine-mobile/src/local_apps_mcp.rs`、`local_apps_tools.rs` — `LocalAppCreate` / receipt-bound `LocalAppScaffold`、只读 profile catalog、host-native confirmation 与工具门；
- `skills/create-local-app/SKILL.md`、`agents/openai.yaml` — 评分、推荐、下载提示、确认与 workflow 路由；
- `tools/workflow/src/builtins.rs` 的 Canvas shape/core、`tasks/src/handlers/local_workflow.rs`、`tasks/src/registry.rs`、`tools/workflow/src/lib.rs` — profile generation、workspace lease 与删除保护；
- `lingxi-code/scripts/mobile-linux/build-local-app-node-modules.sh`、同目录的 `stage-local-app-runtime.py`、`docs/mobile-linux/local-app-runtime-pins.json`、runtime license/SBOM、iOS/Android staging/build 配置 — bundled seed 从含 Three 的 v1 tree 切到 engine-free base，并加 bundle inventory guard；
- supply-chain、dependency snapshot、receipt、固定 IIFE spike 与 iOS/Android WebView 测试的现有模块和 fixtures。

Phase 2 先新增一份独立 NativeGame design/threat model，再在 desktop host 下建立 project service/manifest、toolchain cache、sandboxed process supervisor 和 export 所有权；不得把 Godot 分支散落进 mobile `local_apps_build.rs` 的 `AppSurface::Canvas` match。

### I.10 明确假设与留口

- 「直接 HTML」指现有 Vite + React/Ionic DOM 应用，不新增纯 HTML/JS scaffold。
- Phase 1 以 WebGL2 为跨 iOS/Android WebView 通用基线；WebGPU 只做 feature-detect，不承诺所有设备可用。
- 用户在 host-native picker 确认 profile 后即授权该 profile 的首次按需依赖下载；receipt 只绑定选择，不代表依赖已经 ready。下载失败可重试，不撤销已提交选择。
- Full 主应用可以携带 engine-free base seed，但不得携带 Three、Phaser、Babylon、Havok 或其他游戏引擎；这不违反「引擎按需下载」。
- Phaser/Babylon/Havok 等包从 registry 获取不等于使用 npm CLI；项目包管理和 lock 权威始终是 pnpm。
- 旧 manifest 无 profile 的兼容是冻结 v1 行为，不是迁移入口；不自动升级依赖，也不生成 profile。
- Phase 1 不提供 Unity、Cocos、Defold、PlayCanvas 编辑器或第三方插件市场。
- Phase 2 的 Godot 仅在 desktop host 开放；移动端继续使用 Phaser/Babylon 等 WebView Local App。本文件只锁定 Phase 2 产品边界，独立 NativeGame design/threat model 才是实施授权。
- Godot extension、移动签名/商店发布、跨 OS export 与第三方 Godot 插件管理均为后续设计；首版明确拒绝 native extension/editor plugin/`@tool`，不在首版隐式补齐。

---

## J. 核验记录

### 第一轮（内部对抗式，2026-08-23）
9 个查证代理按断言簇分工，凡判「spec 写错」的再交给**默认反驳**的代理证伪。53 个代理、116 条断言，**31 条确认写错并修正，13 条被反驳代理推翻**。改变设计的四条：§0（工作区 `LINGXI.md` 在移动端从未加载，**推翻了对上一版真机现象的归因**）；§A.2（`surface == None` 与历史遗留应用重载）；§C.2（`app_id` 由 builtin 注入，门的判据不成立）；§D.1（落地布防依赖将被删除的 brief 认领）。

### 第二轮（外部 code review，2026-08-23）
6 条 findings（3 P0 / 3 P1），**5 条采纳、1 条修正后采纳**：

| findings | 处置 |
|---|---|
| P0 工具门不能保证 scaffold 是首次写入 | **成立**。修法改为 §C.0.1 的**覆盖**而非「动态禁止 Write/Edit/Bash」。⚠️ 当时给的理由（「Bash 封不住」）**在第三轮被证伪**——正确的理由是 §C.0.3 的种子路径集边界。结论不变，论据换了 |
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

**第一、二轮都未覆盖的一处，由第二轮补上**：改 `manifest.name` 会改动 `AppManifest::hash()`，而该 hash 绑着 SQLite `_lingxi_schema`（§C.1.4）。对空壳安全，但必须写成不变量，否则将来的「允许改名」会损坏用户数据。

### 第三轮（对抗式复审改稿，2026-08-24）
七个攻击面（覆盖保证 / 原子性 / request_id / 历史迁移 / 门的完整性 / 新事实核对 / 一致性与缺口），每条指控再交给默认反驳的代理。**32 条指控，11 条存活、21 条被杀**，全部已修：

| 存活项 | 处置 |
|---|---|
| **P0** 预留没有存储介质，且任何落盘写法都与「失败可重试」互斥 | §C.1 步骤 1 改为**进程内**预留（RAII guard），并写明为何**不能**照 `set_init_session`（那是持久 set-once，进程被杀就把草稿变砖）。`set_init_session` 仍是**提交写**的范式 |
| **P1** 正式 `LINGXI.md` 在写回 `AppRecord.name/brief` **之前**渲染 ⇒ 合约永久写着 `untitled` + 空 Brief | §C.1 步骤 3 内部重排序，并加测试。**初稿的真 bug**：`LINGXI.md` 只写一次，而按 §0 它是唯一每轮到达模型的通道 ⇒ 整个对话问出来的需求会在唯一的长期载体里丢失 |
| **P1** `request_id` 挂不到 `AppCreated` 上（服务层发出，`host.rs` 看不到），失败侧也没有载体 | §D.1 改为穿领域层（`AppEvent::AppCreated` + `create_app_*` 签名 + 四个包装），并给 `AppOperationFailed` 追加字段；§E 补齐 |
| **P1** widget 快照把每个应用都映射进主屏，空壳会以 `untitled` 出现 | §A.4 表补两行，处置定为**整个排除** `scaffolded == false` |
| **P1** 收紧 `.lingxi/settings.local.json` 对当前会话无效——权限规则只在引擎构造时读一次 | **整条砍掉**，§C.0.2 留下否决记录 |
| **P2** `defaults_per_tool.rs:143` 的 `debug_assert_eq!(m.len(), 66)` 是第三个硬同步点 | §C.1.6 由两处改三处 |
| **P2** Android **有**详情页与卡片溢出菜单 | §D.4 更正（初稿此处错了两遍，含本表上一版） |
| **P2** 「Bash 绕得过一切写入禁止」是错的 | §C.0.2 / §H 更正：工具名是 `Shell`，工作区里非只读命令已被硬拒 |

**被杀的 21 条里有三条我自己复核过**（不接受转述）：
1. **扩展名影子**（`app/app.js` 顶掉 `app/app.jsx`）——机制**是真的**，端到端验证；反驳只成立在**归属**上（既有隐患）。⇒ 当时定为不修、只重写保证的表述。**第四轮推翻了这个处置**，见下。
2. `detect_build_target` 拿不到记录 —— 被正确驳回：`storage.rs` 的 `AppMetadataFile` 只凭 layout 就能读到整个 `AppRecord`。§C.4 现在点名了它。
3. `scaffolded` 在 `surface: Some(_)` 路径上的初值 —— 驳回理由成立，但攻击者的误读本身说明那句注释有歧义 ⇒ §A.2 改写成三个来源逐条列出，并加了一条测试。

### 第四轮（外部 code review 复审改稿，2026-08-24）
6 条 findings（2 P0 / 3 P1 / 1 P2），**全部成立、全部已修**：

| findings | 处置 |
|---|---|
| **P0** 影子文件不能当「既有隐患」排除 | **推翻第三轮的处置。** 判据不是归属而是「确认之前写的代码不能进入正式应用」，逐路径覆盖过不了这条。⇒ §C.0.1 改为**清空可编辑面再写入**（保留 `.lingxi/`、`LINGXI.md`、`node_modules/`）。关键事实：今天的创建**在创建事务内部**就脚手架（`host.rs:5731`、`local_apps_mcp.rs:1352`），**根本没有可写窗口**——那个窗口是本方案开的，破口就是本方案自己的。§C.3 引导文案同步改成「会被删除」 |
| **P0** shell / full create 缺服务层分界 | §A.3 加 `CreateMode::{Shell, Scaffolded}`。空 brief 的放宽**只在 `Shell`**，`Scaffolded` 保留旧不变量；四个包装函数默认 `Scaffolded`。这同时成为 `scaffolded` 初值的唯一决定点 |
| **P1** 步骤 3 没说记录是暂存还是提前落盘 | 改为 `proposed_record` 暂存渲染 + 第 4 步 `with_app` 一次性提交四个字段。否则落地失败会留下「名字对了但还是空壳」的记录 |
| **P1** 没持 `storage::lock_app_build` | 步骤 3.0 起持锁到提交。进程内预留只排斥另一个 Scaffold，**挡不住 `DeleteApp`**——而删除走同一把锁（`storage.rs:899`），并发会把目录 rename 进 trash 而脚手架继续往里写。加并发测试 |
| **P1** session rename 的「可重试」没有触发器 | boot backfill sweep（`host.rs:9354`）只补缺失的 pin，不碰已存在会话的标题 ⇒ 在该 sweep 里加标题对账。它已经在遍历记录了 |
| **P2** §E 把 `AppCreated.request_id` 放错文件 | `AppEventDto::AppCreated` 在 `client-protocol/src/local_apps.rs:917`；`events.rs` 只管外层事件与 `AppOperationFailed` |

⚠️ 值得记下的教训：第三轮我用「归属」（是不是本方案引入的）来决定修不修，**判据选错了**。正确的判据是这个方案自己许下的保证能不能兑现。同一条机制事实，两轮得出相反结论，差别只在判据。

### 第五轮（外部 code review 复审，2026-08-24）
4 条 findings（2 P1 / 2 P2），**全部成立、全部已修**。两条 P1 都是我自己在第四轮改出来的新矛盾：

| findings | 处置 |
|---|---|
| **P1** `CreateMode` 与测试迁移要求互相矛盾 | 采纳 `CreateMode` 时忘了撤销上一版的「改写两条旧测试」。那两条走默认包装 = `Scaffolded`，**必须继续绿**；照初稿改等于亲手拆掉该路径上唯一钉住空-brief 不变量的测试。§A.3 与 §F 都改成「不要动它们」，另加 `CreateMode::Shell` 的新测试 |
| **P1** boot 标题对账会抹掉用户的 `/rename` | 判据错了。`/rename`（`handle_impl.rs:632`）、hook `sessionTitle` 与 mobile 初始占位**写同一条 `custom-title` 通道**，光比标题分不出来。唯一分辨依据是 `append_mobile_empty_session` 多带的 `"mobileEmptySession": 1`（`session/src/jsonl/writer.rs:525`）。对账收紧为「最新生效的 `custom-title` 仍带该标记」，并加 `/rename` 后不覆盖的反例测试 |
| **P2** 引用了不存在的「既有创建超时」 | 仓库里唯一相关的是 `identityProposalTimeout = 20s`，为一次模型调用而设、且随本方案删除。定名 `createResultTimeout = 30s` 并写明依据 |
| **P2** 三处文案残留 | §C.1 的 `LINGXI.md` 段落重复了一次（第四轮编辑叠加所致）已删；§B.3 补入两个 `request_id` 字段；§D.3 的「另外四处」改为「其余每一处」 |

### 第六轮（用户决策，2026-08-24）
「不需要兼容以前的旧版本」，据此改了两处，都是把此前为了兼容而做的将就撤掉：

1. **§B.1 不再翻转 `surface` 的语义。** 初稿保持 `CreateApp` 结构不变、用 `surface: None` 兼作「不脚手架」，代价是一个字段两种含义（§H 曾把它列为接受的疤）。既然删提议变体本来就要 8.0.0 这个破坏性版本，保住 wire 形状换不到任何东西 ⇒ 改为显式 `mode: AppCreateModeDto`，与 §A.3 服务层的 `CreateMode` **同名同概念**，从 wire 一路贯到 `AppRecord.scaffolded` 的初值。§H 那条疤删除。
2. **§A.1 改为 clean-install 前提。** 产品尚未发布，开发与真机验收都会重装/清数据，因此不实现旧 store 迁移，也不把缺 `scaffolded` 的记录伪装成空壳。字段改为新记录必填；当前 fixtures 显式补值，残留旧 store 直接提示清除开发数据。

⚠️ 该字段本身**保留**：它的理由从「区分老应用」缩成两条仍然成立的——`surface` 只在 `AppManifest` 上（送上列表行要对每个应用多读一次 manifest，而 `AppsChanged` 每次变更都发），且客户端拿不到 `surface`。

### 第七轮（用户澄清后的修复，2026-08-24）

1. 用户明确说明产品尚未发布、会全部重装。此前「把所有旧记录默认成 shell，等下次 Scaffold 再清」的延迟迁移被删除：它既不需要，也会制造 `false + Some(surface)` 与已有数据库无法重建的矛盾。§A.1、§A.2、§C.4、fixtures、测试和真机验收统一到 clean install。
2. session 标题保护从 boot sweep 扩到**即时 post-commit rename**。两条路径都只在最新生效的 `custom-title` 仍带 `mobileEmptySession: 1` 时改名；intake 期间的 `/rename` 或 hook 标题不会被 Scaffold 当场覆盖，也不会在重启后被覆盖。

### 第八轮（2D/3D 游戏引擎扩展，2026-08-24）

1. §I 新增两阶段实施门：当前 create-local-app 重构先合并，Phase 1 再增加五种 WebView runtime profile，Phase 2 等 desktop host 支持非 WebView 项目后加入 Godot `NativeGame`；禁止两批工作并行修改同一技能和 workflow。
2. profile 作为 `AppManifest` 的可选持久字段，与现有 `AppSurface::{Dom, Canvas}` 正交；新创建持久化 profile，旧 manifest 的 `None` 冻结在 v1 scaffold/lock，不推断、不回填。`LocalAppCreate` 保留 surface-only 兼容，冲突 profile/surface 在写入前拒绝。
3. 创建确认采用「固定推荐规则 + 用户覆盖」，新增只读 profile/cache catalog；最终 profile 在 `LocalAppScaffold` 提交后不可变，workflow 只能读取 manifest，不能静默换引擎。
4. 五套依赖全部使用独立 `package.json` / `pnpm-lock.yaml`，复用 host-owned `pnpm install --frozen-lockfile` 与 SHA-256 snapshot；明确禁止 npm/npx/Yarn，并补齐 profile-aware 供应链、SBOM、离线失败和性能验收。
5. 核对代码后登记了现有 workflow 身份遗漏：workspace lease 与运行中删除保护只认 `local-app-build`，Phase 1 必须同时覆盖 `local-canvas-build`。
6. Godot 固定 4.7.2 Standard；按 OS 验证下载与 export templates。headless 允许复用同一 Standard executable 的 `--headless` 模式，不假设每个平台都有独立 headless artifact。

### 第九轮（游戏引擎 plan code review 修复，2026-08-24）

6 条 findings（4 P1 / 2 P2）全部采纳并修复：

1. `runtime_profile` 明确使用 `#[serde(default, skip_serializing_if = "Option::is_none")]`，无 profile 的 v1 manifest 序列化/hash 字节保持不变；`surface == None` 不被误当成 v1 scaffold。
2. 当前含 Three 的 bundled v1 dependency seed 被明确列为 Phase 1 必改项：Full build 只保留 engine-free base seed；legacy v1 和所有游戏引擎按需安装。下载提示改成能表达 snapshot、base seed 与「可能需要下载」的三态，不再把 `!snapshot_ready` 等同于一定访问网络。
3. 对话式 profile 选择不再由代理把字符串从 `AskUserQuestion` 搬进 Scaffold。新增 host-native picker 与 10 分钟、绑定 app/profile、成功后一次性消费的进程内 receipt；`LocalAppScaffold` schema 不接受可覆盖 profile/surface。
4. snapshot identity 回到现有内容寻址边界：toolchain 分区 + lock SHA；profile 只写 provenance/SBOM，依赖相同的 profile 可以共享 tree。
5. Babylon + glTF + Havok 的固定 IIFE 双端 WebView spike 上移为 Phase 1.0 公共接口开工门；失败时停下重新设计 build contract，不能先落 schema。
6. Godot Phase 2 改为独立 NativeGame design + threat model 的后续授权，补齐最小 project discriminator/API/lifecycle/锁要求，以及 archive extraction、project preflight 和 desktop sandbox 的 fail-closed 边界。

### 第十轮（跨计划 master order 与下游防漂移，2026-08-24）

1. create-flow 与 interactive-verification 保持独立 design，不拼成 2400 行单体；新增 `2026-08-24-local-app-implementation-order.md` 作为跨计划顺序和共享文件 owner 的唯一权威。
2. 本方案继续先 bless 8.0.0；verification Phase 3 必须在 8.0.0 与 profile-aware 代码上 rebase，不能从 major 7 独立 bless。
3. `LINGXI.md` resolver 归本方案 §0；canvas workflow lease/delete guard 与 `.lingxi` build-key 归 verification Phase 1。两份文档只允许一个实现 owner，另一份保留进入断言与回归测试。
4. 增加 downstream freshness rule：以后本方案修改 post-create seam 时，同一变更必须更新 master order 和 verification 的上游契约/阶段/测试/状态表，并由真实 create 路径的 compatibility test 防止下游继续使用旧 flow。

### 第十一轮（对抗式复审 Codex 改稿，2026-08-24）
八个攻击面（供应链 / manifest-协议 / workflow 身份 / picker-receipt / Godot-desktop / 跨文档 / §0-§H 回归 / 可执行性），每条指控再交给默认反驳的代理。**34 条指控，4 条存活**，全部已修；§0-§H 只留下一处陈旧引用，其余存活项都在新增的 §I：

| 存活项 | 处置 |
|---|---|
| **P1** `rename_all = "snake_case"` 数字不断词 ⇒ 五个 profile 里四个的持久拼写与全文对不上 | §I.2 改为逐变体 `#[serde(rename = "...")]` 并写明理由；§I.8 加一条「序列化字节等于映射表」的断言（用 `rename_all` 会让其中四条直接红）。⛔ 该值进 `AppManifest::hash()` 且 scaffold 后不可改，发现晚了就会撞上 §C.1.4 那类数据损坏 |
| **P1** 兼容矩阵缺「profile 与 surface 都缺省」一行，而那正是默认 `LocalAppCreate{brief}` | 矩阵切法从「新客户端 vs 旧客户端」改为「新建 vs 既存」：新建一律反查到 engine-free profile，legacy 读取规则只保留给既存无 profile 应用。否则每次默认创建都退回含 `three` 的 v1 lock、首次构建必须联网——正是 `build-local-app-node-modules.sh` 存在的理由要消灭的回归 |
| **P1** 重新确认不作废旧 receipt ⇒ 代理可重放用户已否决的选择 | 改为 per-app 单槽：签发新 receipt 原子作废旧的，拒绝清单加 superseded，§I.8 加连续确认两次的用例。「schema 不接受另一个 profile」只堵参数注入，堵不住回执选择 |
| **P2** §C.3 仍用第六轮已废弃的 `CreateApp{surface: None}` 当判据 | 改为 `CreateApp{mode: Shell}` |

**一条降级为建议、未改文档**：§I 是本文件里的第二个项目，而 §E/§F/§G 通篇只覆盖 §0-§H、§I 另有 §I.8/§I.9 两份平行清单 ⇒「本方案要改哪些文件 / 通过验收 / 先 bless 8.0.0」各有两个答案。反驳代理指出该结构是 §I.1 明确设计的（Phase 1 不是合并阻塞项、§I 自带清单），这条辩护成立，故仅记录建议：可抽成独立 design 并挂进 `2026-08-24-local-app-implementation-order.md`。**待人工决定。**
