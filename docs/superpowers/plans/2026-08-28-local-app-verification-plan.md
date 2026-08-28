# 本地应用可交互验收 —— 重构后的实施计划

日期：2026-08-28
基线：`7377e95e7` + 运行时 profile 重构（320 个未提交路径，作者 Codex）
取代：[交互式验收设计](../specs/2026-08-23-local-app-interactive-verification-design.md) 的阶段表
（设计本身仍有效，阶段划分按本文执行）
配套评审：[插件方案 verification 侧评审](../specs/2026-08-28-local-app-plugin-verification-review.md)

> ⚠️ **本文所有代码事实核于 2026-08-28。** 这棵树过去 48 小时被持续改写，
> 两轮独立评审 6 条发现里 4 条陈旧，**我自己也贡献了一条**（评审期间 runtime profile
> 落地，使我刚写下的发现当场失效）。**实施前先钉一个 sha。**

---

## 1. 已完成 —— 不要重做

| 能力 | 状态 | 证据 |
|---|---|---|
| **Phase 1a** `inspect_ui` 几何 + canvas rect + runtimeErrors；`capture_ui` 区域裁剪；`image-read` | ✅ **真机验收通过** | iPhone 11 / iOS 18.6.2，`Executed 83 tests, 0 failures`；17 条判据测试全绿 |
| **Phase 1b** `.lingxi` 进构建键跳过表；canvas 构建的 workspace lease + 删除守卫 | ✅ 已落地 | `e1c128b70` |
| **冒烟 spike**（master step 3b） | ✅ 结论已出 | `6b0377784`，四个 `testSpike*` |
| 真机 bridge 准入控制验证 | ✅ 修复并验证 | `6b0377784`（7 个测试此前在真机上根本跑不起来） |

**spike 的三条结论直接约束下面每一个阶段：**

1. **捕获必须走 `drawHierarchy`，不得用 `takeSnapshot`。**
   后者是生产 `capture_ui` 现在走的路径（`LocalAppWebView.swift:572`），
   对离屏 WKWebView **恒返回 rgb(0,0,0)**；`drawHierarchy` 五种安放里四种正常，
   且能捕获 2D canvas 与 **WebGL**。
2. **离屏 `requestAnimationFrame` 照常运行**（~400ms 内 25 帧）⇒
   动画类 profile 停在离屏也继续渲染，门不必自己驱动帧。
3. **冒烟报告必须动协议**：`AppWorkflowStateDto` 是无字段枚举
   （`client-protocol/src/local_apps.rs:41-46`），uniffi 规则禁止给它加带数据变体。

---

## 2. 重构改变了什么

**运行时 profile 成为一等公民。** 这解除了本工作流最大的一个阻塞，也让两条旧结论作废。

| 变化 | 对 verification 的影响 |
|---|---|
| `AppRuntimeProfileBinding { family, revision, contract_sha256 }` 持久化在 manifest（`local-apps/src/manifest.rs:278/388`） | ✅ **「renderer 不持久化」的阻塞解除。** 我和 `lingxi-next-b5` 曾各自独立确认这条并评为「最强」——它现在**已作废**，不要再引用 |
| 五个 profile 落成真实脚手架模板（`local-apps/templates/runtime-profiles/{react-dom,canvas-2d,three-3d,phaser-2d,babylon-3d}`，119 个新文件） | 冒烟门与 `verify` skill 必须覆盖**全部五个**，不是 DOM/Canvas 两个 |
| `local-apps/src/runtime_migration.rs` + 迁移日志 | 存量 app 会被迁到 profile ⇒ **冒烟门必须能处理「刚迁移完」的 app**，其 `contract_sha256` 与源码可能不同步 |
| `AppSurface{Dom,Canvas}` **仍然存在**，与 profile 并存（`manifest.rs:233`） | 是**两根正交的轴**，不是替代关系。判定逻辑不得二选一：`surface` 决定观测形状（DOM 树 vs canvas rect），`profile` 决定构建与技能路由 |
| 十个 skill 重构为 `references/profiles/` + `router.md`，四个旧 checklist 被删 | `verify` skill 的内容要落进新结构，不是旧的 `verification-matrix.md`（已删） |

**仍然不存在的（本计划要建的全部）：** `SmokeReport`(0)、`verification_unavailable`(0)、
`StoreAppAnnotation`/`AppAnnotation`(0)、`needs_user_review`(0)、build generations(0)。
`webview_checked` 门仍是 verifier **自填的布尔值**（`local_app_workflow_core.js`，3 处）。

---

## 3. 阶段计划

### P0 —— 守卫语义化（前置，阻塞插件化）

**为什么先做**：插件方案要把 workflow 改名为 `lingxi-local-app:local-app-build`。
而 `LOCAL_APP_BUILD_WORKFLOWS`（`tasks/src/lib.rs:37`）是 **workspace lease 与删除守卫的
精确匹配键**（`local_workflow.rs:522`；`registry.rs:918-919`，⚠️ 跨两行，单行 grep 查不到）。改名 ⇒ 两道守卫静默失效 ⇒
**构建中的 app 可被删除**，即回归 Phase 1b 刚修好的线上 bug。

🚨 **我自己那道防漂移门抓不到它**：它比对两个列表**彼此**是否一致，两边一起改名照常绿。

**任务**
- P0.1 `WorkflowDefinition` 增加 `writes_app_workspace: bool` 能力标记，由 manifest 声明。
- P0.2 `requires_workspace_lease` 与删除守卫改读该标记，不再匹配名字。
- P0.3 **反向测试**：构造运行中的 `local-app-build` 与 `local-canvas-build` 各一，
  断言删除**被拒绝**；再构造一个改名后的 id，断言**同样被拒绝**。

**退出门**：P0.3 必须**先在 P0.1 之前证明自己会红**（把标记写死为 false，测试必须失败并
点名"构建中的 app 被删除"）。绿而未先红的门不算门。

---

### P1 —— 冒烟门落 host core

**这是本工作流的核心，也是唯一不可放进插件的部分。** 三条理由：
判据 1 要求宿主保证触发，而插件 JS 挂不到 `promote_build_root`；
判据 3 要求判定不经模型，而 `webview_checked` 是模型自述；
**插件可以被关闭 ⇒ 门也就可以被关闭**。

**挂载点**：`promote_build_root`，调用于 `build_workspace_locked`
（`local_apps_build.rs:1147` —— **唯一生产调用点**；`:3349` 在 `#[cfg(test)]` 模块内）——
新字节变成被服务字节的定义性时刻，三条产出路径全收敛于此，
包括 `restore_checkpoint_value` 这条唯一会绕过 `build_app` 的路径。

**任务**
- P1.1 iOS：离屏 WKWebView 宿主通道。非零固定 viewport，挂 key window 可布局容器，
  容器移出可见边界。**必须绕开 `approvedUIAutomation`（`LocalAppsStore.swift:148`）
  与 `requestedPresentationAppID`（`:1089`）** —— 后者对**每一个** UI 请求都会设置，
  且在只读自动放行分支之前，所以任何复用现有 UI 请求通道的门都会把用户从聊天里拽走。
- P1.2 registry 按 `(appID, role)` 键控，`role ∈ {visible, smoke}`。
  `resolveBridge`/`deliverStreamFrame` **按 requestID 路由**（各 broker 的 `inFlight`
  集合互不相交，`LocalAppWebView.swift:156`），只有 `execute(request:)` 需要 role。
  **必须解决**：`close(appID:)` 与数据存储删除时谁关掉 smoke 视图。
- P1.3 取证：`drawHierarchy`（**不是 `takeSnapshot`**）+ `inspect_ui` 的
  runtimeErrors / documentState / canvas rect。
- P1.4 `SmokeReport{build_id, output_change_id, criteria[], verdict}`，宿主侧计算判定。
- P1.5 Android 无能力 ⇒ `verification_unavailable`，**绝不静默判通过**。
- P1.6 删除 `local_app_workflow_core.js` 里的 `webview_checked` 自述门。

**退出门**
- 五个 profile 各跑一次真机闭环，`SmokeReport` 落到客户端；
- **埋雷测试**：故意让构建产出一个抛异常的首屏，断言门**变红**且报告点名该异常；
- **绕过测试**：走 `restore_checkpoint_value` 路径，断言门**同样触发**；
- Android 返回 `verification_unavailable` 而非 pass。

---

### P2 —— `verify` skill 与 `verifier` agent 的内容

**归属**：内容进插件（可关闭），但**机器事实来自 P1 的 `SmokeReport`**。
插件承载「怎么判断好不好」，host core 承载「能不能加载、有没有报错」。

**任务**
- P2.1 `verify` skill 必须写明**捕获仪器**：离屏语境下不得用 `takeSnapshot`。
  🚨 今天的 `frontend-qa` 让 verifier「capture two frames」却**全篇不说用哪个 API** ⇒
  离屏下会拿到两张全黑图，报「无变化」——一个**假失败**，且看起来像 app 坏了。
- P2.2 写入六条判据表；**判据 4/5 是建议不是阻塞**，附四条误阻塞路径
  （菜单态 canvas、JPEG 质量档位被读成运动、Android reload 的 `about:blank` 白帧、
  WebContent 进程死亡）。
- P2.3 判据 6 必须覆盖 `console.error`：React 19 的 `onCaughtError` 路由到
  `console.error`，**永不到 `window.onerror`**；模板的 `error-boundary.jsx` 只有
  `getDerivedStateFromError` ⇒ 渲染崩溃的 app 出兜底页：**有 DOM、有配色文字、
  无未捕获异常 —— 三条判据全绿出厂**。
- P2.4 载荷降级 ⇒ `infrastructure_unavailable`，**不判通过**（观测不到 ≠ 没异常）。
- P2.5 `verifier` **不得自报门禁结论**：`qa_report` 里不得有任何等价于「我验过了」的字段。
- P2.6 工具白名单：verifier 与 builder 的**写工具交集为空**，并有测试断言。

**退出门**：`local-apps/templates/runtime-profiles/` 五个模板各造一个故意有缺陷的变体，
verifier 必须点名缺陷；以及一个**健康**变体，必须不报阻塞项（防过杀）。

---

### P3 —— `use` skill（host core，不进插件）

**用户指出的整类缺口。** 24 个 `LocalApp*` 工具里，「使用」那一半在插件关闭时仍可用，
**但全仓库唯一描述 `LocalAppQueryData`/`LocalAppMutateData` 语义的文本在
`create-local-app/SKILL.md` 里**——一个即将进插件并默认关闭的文件。
⇒ 关掉插件 = **工具还在，说明书没了**，模型会照自己猜的形状调用。

**任务**
- P3.1 给出 24 个工具的**完整二分表**。判据：**「它会改变 app 的源码或 manifest 吗？」**
  会 ⇒ 创作（随插件关闭）；不会 ⇒ 使用（始终可用）。
  按此 `MutateData` 属使用，`CheckpointRestore` 属创作。
- P3.2 `use` skill 内容：数据读写边界（collection 必须已在 manifest 声明，
  否则 `collection "x" is not declared by the app manifest` —— 2026-08-07 真机 QA 的 F1）；
  runtime 启停语义（`start` 对运行中的 runtime 是成功 no-op；
  **restart 不重新加载页面**，端口刻意稳定 ⇒ URL 逐字节相同 ⇒
  `LocalAppWebView.swift:1104` 的 `guard loadedURL != url` 直接返回）。
- P3.3 从 `create-local-app` **抽出**而非复制这些内容。

**退出门**：插件关闭状态下，模型能正确完成一次「查数据 → 改数据」，且不触碰创作工具。

---

### P4 —— 标注与 build generations（动协议）

**依赖**：P1（`SmokeReport` 的协议载体）。**与插件方案的协议 bless 合并成一次**，
避免两轮 uniffi 绑定重生成。

**任务**
- P4.1 `StoreAppAnnotation` / `DeleteAppAnnotation`；`annotations/` 原子目录；
  `annotation_id` 落盘前校验/归一；read/delete 双豁免。
- P4.2 `AppRecord`/`AppRecordDto` 两个 build generation 字段
  （`last_build_id` / `last_output_change_id`，**两者今天都不存在**）。
- P4.3 `SmokeReport` 与 `PluginSummaryDto` 同一次 bless。

**硬门**：并发 restore 测试；两次无改动构建 ⇒ `last_build_id` 变、
`last_output_change_id` 不变。
⚠️ **该验收在 `.lingxi` 未进跳过表的坏状态下也会通过** —— 所以
`workspace_build_key` 那条回归测试必须**同时**在此门内（Phase 1b 已落地，此处纳入基线断言）。

---

### P5 —— iOS 标注 UI（host core 客户端）

**依赖**：P4。overlay + controller 串行 + 标注状态机与持久化 + RootView one-shot 提交路由
+ 最小可用副驾驶条。`skills/update/`（**不存在，需新写**）是标注转成修改指令的落点。

🚨 **`needs_user_review` 必须有用户可见信号，否则这次改动是净负**：
它会渲染 `chat_task_completed`，**与健康构建逐字节相同**，模型也看不到。
必须同时加 app 详情页的可疑标记 + 把 `smoke_report` 摘要带进任务通知。

---

## 4. 依赖顺序

```
P0 守卫语义化 ──┬─► 插件改名（Codex 的方案，非本计划）
                │
P1 冒烟门 ──────┼─► P2 verify skill 内容
   (host core)  │
                └─► P4 协议/标注 ──► P5 iOS 标注 UI
P3 use skill ───┘   (与插件 bless 合并)
```

- **P0 必须先于任何改名**，且与改名在同一 commit 切换。
- **P1 与 P3 可与插件化并行**：它们只动 host core，不碰插件文件。
- **P2 依赖 P1**：没有 `SmokeReport` 提供机器事实，verifier 只能自述。
- P4/P5 串行。

---

## 5. 每个阶段都必须自带的防漏门

本仓库反复吃过的四种假绿，每阶段交付前逐条自检：

1. **退出码不是证据。** `xcodebuild` 在三种真机失败形态下都退出 0；
   判据只能落在输出**点名了具体的东西**。
2. **绿测试要先证明自己会红。** 且红的那次必须点名缺失的内容。
3. **两个列表彼此一致 ≠ 与现实一致**（P0 那道门就是这么瞎的）。
4. **一个仪器给出的否定结论不可信。** spike 里 `takeSnapshot` 单独给出的
   「离屏不能绘制」是错的，加第二个仪器当场反转。
   任何关于渲染/合成/截图的否定结论必须跑两个独立仪器。

以及真机专属的两条：

5. **真机安装 9 次失败 5 次**（`MIInstallerErrorDomain` 增量补丁，
   以及首次启动 `runner exit 74`）。**卸载重装不能解决**（已实测，
   代价是清空设备数据）。可行做法是重试到跑通，判据是日志里有
   `Executed N tests` 那一行。
6. **`#if canImport(engine_mobileFFI)` 会静默吞掉整族测试。** 判绿前先断言执行数。

---

## 6. 与插件方案的边界

| | host core（不可关闭） | 插件（可关闭） |
|---|---|---|
| 观测能力 | `inspect_ui`/`capture_ui`/`act_on_ui`/`image-read` | — |
| 判定 | **冒烟门 + `SmokeReport`**（机器事实） | AI 主观评审 |
| 数据 | 标注存储、build generations、协议 | — |
| 客户端 | 标注 overlay、副驾驶条 | — |
| 知识 | **`use` skill** | `create`/`update`/`verify`/`design` + 5 个 renderer skill |

一句话：**能不能加载、有没有报错 —— host core；好不好 —— 插件。**
