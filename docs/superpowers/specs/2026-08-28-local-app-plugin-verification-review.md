# Local App 创作插件方案：verification 侧评审 + 流程并入

日期：2026-08-28
评审对象：Codex 的《LingXi Local App 创作插件实施方案》
评审人立场：verification 工作流 owner（[交互式验收设计](./2026-08-23-local-app-interactive-verification-design.md)、
[总实施顺序](./2026-08-24-local-app-implementation-order.md)）
基线：`0e71dff34`。**所有行号按 2026-08-28 的树重核**——方案里至少一处已漂。

---

## 结论

**方案的插件化方向成立，但有两条会造成线上回归/静默失效的阻塞项，必须在动工前解决。**
另有一条是 verification 的架构硬约束：**冒烟门不能住在插件里。**

| # | 级别 | 问题 | 影响 |
|---|---|---|---|
| 1 | 🚨 阻塞 | 重命名 workflow id 会静默关掉 workspace lease 与删除守卫 | 构建中的 app 可被删除 = 回归刚修好的线上 bug |
| 2 | 🚨 阻塞 | 冒烟门若住在插件 JS 里，判据 1/3 无法满足，且门本身可被关闭 | 验收门形同虚设 |
| 3 | ⚠️ 重要 | `defaultEnabled:false` + 无迁移 = 所有存量用户升级后**静默失去创建能力** | 产品级回归 |
| 4 | ~~⚠️~~ ✅ 已作废 | ~~`runtime_profile` 不存在~~ —— **评审期间被 Codex 落地了**，见第 4 节 | 无 |
| 5 | 📝 提示 | 方案引用的 `local_app_workflow_core.js:430` 实际在 `:518` | 树在动，评审要钉 sha |

---

## 1. 🚨 重命名 workflow id 会静默关掉两道守卫

方案第 1 节要求「删除未命名空间化的 `local-app-build`、`local-canvas-build`」，
只保留 `lingxi-local-app:local-app-build`。

问题在于这两个名字**不只是 workflow 注册名，它们是两道安全守卫的判据键**：

```rust
// tasks/src/lib.rs:37
pub const LOCAL_APP_BUILD_WORKFLOWS: &[&str] = &["local-app-build", "local-canvas-build"];

// tasks/src/handlers/local_workflow.rs:522 —— workspace lease
fn requires_workspace_lease(workflow_id: &str) -> bool {
    crate::LOCAL_APP_BUILD_WORKFLOWS.contains(&workflow_id)
}

// tasks/src/registry.rs:918 —— 删除守卫
&& crate::LOCAL_APP_BUILD_WORKFLOWS.contains(&workflow.workflow_id.as_str())
```

两处都是**精确字符串相等**。改名后 `contains()` 一律返回 `false` ⇒

- **workspace lease 不再获取** ⇒ 并发构建互相踩踏；
- **删除守卫不再生效** ⇒ **构建进行中的 app 可以被删掉**。

⚠️ 第二条正是 verification Phase 1b（`e1c128b70`）刚修好的线上 bug
（[设计文档「先于本设计存在的在线缺陷」第 3 条](./2026-08-23-local-app-interactive-verification-design.md)）。
按现方案实施 = 把它原样放回去。

🚨 **而且我自己那道防漂移门抓不到这件事。** `local_app_build_workflow_sets_agree`
（`local_apps_build.rs:3492`）比对的是 `tasks::LOCAL_APP_BUILD_WORKFLOWS` 与
`tool_workflow::LOCAL_APP_BUILD_WORKFLOWS` **两个列表是否彼此一致**——两边一起改名，
门照常绿。它防的是「加了第三个 workflow 却漏改一边」，**防不了「两边同时与现实脱节」**。
这正是本仓库反复吃过的形状：*一个绿测试可能正钉着 bug 本身*。

**要求（三选一，按优先级）：**

1. **首选**：守卫改为判定 workflow 的**语义**而非名字——在 `WorkflowDefinition` 上加一个
   `writes_app_workspace: bool`（或等价的 capability 标记），由插件 manifest 声明，
   守卫读这个标记。名字随便改，守卫不受影响。
2. 次选：常量改为**同时**包含新旧两种拼写，并新增一条**反向测试**：构造一个
   `lingxi-local-app:local-app-build` 的运行中任务，断言删除**被拒绝**。
3. 最低要求：若坚持纯改名，必须在同一 commit 里加上述反向测试，并且该测试要
   **先在改名前证明自己会红**。

无论选哪条，**验收标准里必须新增**：「构建进行中的 app（DOM 与 canvas 各一次）
删除请求被拒绝」。现方案的验收清单里没有任何一条覆盖它。

---

## 2. 🚨 冒烟门不能住在插件里

方案把 `verify` 作为插件 skill + `verifier` agent，把验证门禁迁进
`workflows/local-app-build.js`。这与 verification 设计的判据直接冲突。

**判据 3「判定不经模型」**：现状的门是
```javascript
// local_app_workflow_core.js:518
if (build.ok === true && verification.webview_checked !== true) { throw ... }
```
`webview_checked` 是 **verifier 子代理自己填的布尔值**，背后零证据核验。
把它搬进插件只是换了个文件，判据 3 依旧不满足——**门证明的仍然只是「有个模型这么说了」**。

**判据 1「宿主保证触发」**：`promote_build_root`（`local_apps_build.rs:970`，
唯一生产调用点）是新字节变成被服务字节的定义性时刻，三条产出路径全收敛于此。
门必须挂在这里。**插件 JS 无法挂在这里**——它是被 workflow engine 调用的，
而 `restore_checkpoint_value`（`local_apps_host.rs:3279`）根本不经过 workflow。

**新增的第三个理由：插件可以被关闭。** 方案明写「插件关闭时仅禁止创建、更新和 AI 验证」。
若门住在插件里，则**关掉插件 = 关掉验收门**。而门的价值恰恰在于它不可绕过。

**要求 —— 职责一刀切开：**

| 职责 | 归属 | 理由 |
|---|---|---|
| **冒烟门**：宿主侧触发、宿主侧判定、结构化 `SmokeReport` | **host core，不进插件** | 判据 1/3/6；必须不可关闭 |
| **AI 验收**：设计评审、可用性、视觉、交互探索 | **插件 `verifier` agent** | 主观判断，本就该模型做，可关闭 |

冒烟门产出机器事实（文档是否换新、runtime error 账本、canvas 是否有非零 rect），
`verifier` 在这些事实**之上**做主观判断。关掉插件 ⇒ 失去 AI 评审，
**但不失去「构建产物到底能不能加载」这条底线**。

这个切分同时解决方案自己的一个矛盾：它把 `verify` 定义为「不修改源码」的只读操作，
却又让它成为构建成功与否的判据来源——只读的东西不该有否决权，除非它的判定不来自模型。

---

## 3. ⚠️ 默认关闭 = 存量用户静默失去创建能力

方案：`defaultEnabled: false`，且「不提供旧名称 alias、迁移期或双注册」，
「所有现有用户升级后插件均为关闭状态」。

后果：**升级后，用户点「+」创建 app 会得到一个启用提示，而不是他们昨天还在用的功能。**
create-flow 会话刚刚交付的对话式创建流程，落地即默认关闭。

方案已经设计了启用提示（这是对的），但没有回答：

- 存量**已有 app 的用户**是否也要走一次启用？（建议：检测到用户已有 ≥1 个 app ⇒ 首次升级自动启用）
- 提示的文案是否说明**为什么**突然需要启用？
- 关闭状态下 app 会话里发创作指令，模型看到的是什么？方案说「显示启用提示」，
  但**模型侧**会看到工具消失——需要明确它是拿到一个可解释的错误，还是静默少了工具。
  按本仓库判据：*工具消失而模型不知道，会被读成「模型拒绝执行」*。

**建议**：`defaultEnabled: false` 只对**全新安装**成立；升级路径上，
若 `AppRecord` 数量 > 0 则视为已启用。这不是兼容层，是一次性迁移判定。

---

## 4. ✅ ~~`runtime_profile` 不存在~~ —— 本条在评审期间被证伪

**我写下这条时它是真的，写完时已经不是了。原文保留在下方，因为这个失效过程本身是结论。**

原始发现：`runtime_profile` 在全部 Rust 代码中 0 命中；`renderer` 哪儿都不持久化
（`AppSurface` 只有 `Dom | Canvas`）⇒ `operation:"update"` 对已有 canvas app 无法成立。
这条发现由我和 `lingxi-next-b5` 两个会话**独立确认**过，并被双方评为「最强的一条」。

**现在（`0e71dff34` + Codex 的未提交改动）它已经落地了：**

```rust
// local-apps/src/manifest.rs:278
pub struct AppRuntimeProfileBinding {
    pub family: AppRuntimeProfile,   // ReactDom | Canvas2d | Three3d | Phaser2d | Babylon3d
    pub revision: u32,
    pub contract_sha256: String,
}

// local-apps/src/manifest.rs:388 —— 持久化在 manifest 上
pub runtime_profile: Option<AppRuntimeProfileBinding>,
```

而且带了两道门：`scaffolded_apps_require_a_runtime_profile`（`:1087`）与
`dependency_snapshot_requires_a_matching_runtime_profile_contract`（`:1094`）。
协议侧 `AppRuntimeProfileDto`、`AppRuntimeProfileBindingDto`、
`ResolveAppRuntimeProfileSelection` 命令均已就位。

⇒ **方案第 3 节的 `runtime_profile: { family, revision, contract_sha256 }` 逐字对得上，
`operation:"update"` 成立，本条对方案没有任何要求。**

📝 **这条的教训比它本身重要。** 我在同一份文档的第 5 条里建议「先钉一个 sha 再评审」，
理由是过去 24 小时内两轮评审 6 条发现里 4 条陈旧。**然后我自己在写这份评审的过程中
又贡献了第 5 条陈旧发现**——从查证到落笔之间，Codex 把它修好了。
第 5 条的建议因此从「建议」升级为**必须**。

## 5. 📝 方案引用的行号已漂

方案引用 `local_app_workflow_core.js:430` 的验证门禁，实际在 **`:518`**。
过去 24 小时内两轮独立评审对这份 diff 产生了 **6 条发现里 4 条陈旧**（双向都有），
根因是 Codex 的写手仍在改这棵树。

**建议**：本方案实施前先钉一个 sha，评审与实施都基于该 sha。

---

## 6. 方案里已经正确的部分（核过，不要改）

- **`agentType` 是真的通了**，不是只写在文档里：`local_workflow.rs:1259`
  读取 `opts.agentType` 并覆盖 `subagent_type`，`:638` 把它纳入 resume chain-key。
  方案第 3 节的 `agent(prompt, { agentType: "lingxi-local-app:designer" })` 可用。
- **`PluginComponents` 加 `workflows` 是干净的扩展**：现有 7 个槽位
  （`manifest.rs:81` 起）都是同构的 `Vec<ComponentPath>`，加第 8 个不破坏任何东西。
- **`default_enabled` 已存在**（`discovery.rs`，9 处），不需要新造概念。
- **运行 lease + `plugin_in_use`** 与既有 workspace lease 模型一致，方向正确。
- **all-or-nothing materialization** 是对的：部分注册的插件是本仓库最难查的一类状态。

---

## 7. verification 流程并入插件方案

按上文第 2 条的切分，verification 的五个阶段这样落进方案的实施步骤。
⚠️ 注意 runtime profile 已在评审期间落地（第 4 节），所以下表中原先「等 step 4」的
阻塞已经解除——**verification Phase 2/3 的前置现在是齐的**。

### 已完成，插件方案必须继承而非重做

| verification 阶段 | 状态 | 插件方案的义务 |
|---|---|---|
| **Phase 1a** agent UI contract（`inspect_ui` 几何/canvas rect/runtimeErrors、`capture_ui` 区域裁剪、`image-read`） | ✅ 已落地并**真机验收通过**（2026-08-27，iPhone 11/iOS 18.6.2，83 tests 0 failures） | 这些是 **host core 能力，不进插件**。`verifier` agent 调用它们；插件关闭时它们仍存在（只是 agent 侧工具被 gate 掉） |
| **Phase 1b** `.lingxi` 进构建键跳过表 + canvas 构建的 lease/删除守卫 | ✅ 已落地（`e1c128b70`） | **见第 1 条——改名会毁掉它** |
| **smoke spike**（master step 3b） | ✅ 已完成（`6b0377784`） | 结论直接约束插件设计，见下 |

### spike 结论对插件方案的三条硬约束

1. **捕获必须走 `drawHierarchy`，不能用 `takeSnapshot`。**
   实测：`takeSnapshot`（= 生产 `capture_ui` 现在用的路径，`LocalAppWebView.swift:572`）
   对离屏 WKWebView **恒返回 rgb(0,0,0)**；`drawHierarchy` 五种安放里四种正常绘制，
   且能捕获 2D canvas 与 **WebGL**。
2. **离屏 `requestAnimationFrame` 照常运行**（~400ms 内 25 帧）⇒
   动画类 app（threejs/phaser/babylon profile）停在离屏也继续渲染，
   门不必自己驱动帧，也不会静默产出空白证据。
3. **冒烟报告的载体必须动协议**：`AppWorkflowStateDto` 是无字段枚举
   （`local_apps.rs:41-46`），uniffi 规则禁止给它加带数据变体 ⇒
   只能新增 `AppRecordDto` 字段或新 app 事件。**插件不能承载它**——
   协议是 host core 的东西。

### 修订后的实施步骤（在方案第 10 步清单上做增量）

方案的 10 步保持不变，插入/修改以下项：

- **步骤 1（补回归测试）新增两条**：
  - 构建进行中的 app 删除被拒绝（DOM + canvas 各一次）——**改名前必须先证明它会红**；
  - `.lingxi` 写入不使构建键失效（Phase 1b 的 `build_key_ignores_service_state_under_dot_lingxi` 已存在，纳入基线）。
- **步骤 3（plugin discovery/manifest）新增**：`WorkflowDefinition` 增加
  `writes_app_workspace` 能力标记，供第 1 条的守卫使用。
- **步骤 4/5（迁移 skills、合并 JS）新增一条断言**：单一 JS 的 profile 分支必须覆盖
  `AppRuntimeProfile` 的**全部五个变体**（ReactDom/Canvas2d/Three3d/Phaser2d/Babylon3d，
  `manifest.rs:278`）。这是枚举，可以穷举——用一条测试钉住「新增变体必须同时更新 JS」，
  否则又是一个「两个列表彼此一致、共同与现实脱节」的形状（见第 1 条）。
- **步骤 6（host workflow ID 统一改名）** 是本方案**风险最高的一步**，
  必须与第 1 条的守卫改造在**同一个 commit**，否则中间态就是「守卫已失效」。
- **新增步骤 6.5 —— 冒烟门落 host core**（不属于插件）：
  在 `promote_build_root` 挂宿主侧触发点；离屏 WKWebView + `drawHierarchy` 取证；
  产出结构化 `SmokeReport{build_id, output_change_id, …}`；
  专用宿主通道必须同时绕开 `approvedUIAutomation`（`LocalAppsStore.swift:148`）
  **和** `requestedPresentationAppID`（`:1033`）——后者对**每一个** UI 请求都会设置，
  且在只读自动放行分支之前，所以任何复用现有 UI 请求通道的门都会把用户从聊天里拽走。
  **iOS only**：Android 无对等能力时引擎必须返回 `verification_unavailable`，
  绝不静默判通过。
- **步骤 8（客户端 Plugin DTO）合并**：`SmokeReport` 的协议载体与
  `PluginSummaryDto` 在**同一次 bless** 里做，避免两轮 uniffi 绑定重生成。

### 插件关闭时 verification 的行为（方案未覆盖，需补进验收标准）

| 能力 | 插件关闭时 |
|---|---|
| 冒烟门（host core） | **仍然运行**——它是构建产物的底线校验，不是 AI 能力 |
| `verifier` agent 的主观评审 | 不可用 |
| `inspect_ui` / `capture_ui` / `act_on_ui`（agent 侧） | 按方案 gate 掉 |
| 已有 app 的预览/启动/停止 | 不受影响（与方案一致） |
| 存量 app 的 `SmokeReport` 历史 | 仍可读——它是 app 数据，不是插件数据 |

---

## 8. 建议的执行顺序

1. **先钉 sha**（第 5 条）。
2. **守卫语义化**（第 1 条）+ 反向测试，**先于**任何改名。
3. **renderer 持久化**（第 4 条），否则 `update`/`verify` 入口是空中楼阁。
4. 插件骨架、`WorkflowRegistry`、迁移 skills/agents（方案步骤 2-5）。
5. **改名 + 守卫切换，同一 commit**（方案步骤 6 + 第 1 条）。
6. **冒烟门落 host core**（新增步骤 6.5）——可与 4/5 并行，因为它不碰插件文件。
7. 迁移判定（第 3 条）+ 客户端 DTO + 协议 bless（方案步骤 7-8，合并 `SmokeReport`）。
8. 清理与文档（方案步骤 9-10）。
