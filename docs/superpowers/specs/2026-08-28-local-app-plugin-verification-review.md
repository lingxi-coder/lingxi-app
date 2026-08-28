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

---

## 9. verification 的 skills / agents 具体内容（本节补上文缺的部分）

第 7 节给的是**架构与顺序**，没有给插件里那几个文件**写什么**。本节补齐。

### 9.1 先更正方案的 skill 清单

方案说「把根级十个 local-app skills 迁入插件」，并列出 11 个目录。实际（2026-08-28）：

| 插件目录 | 来源 | 状态 |
|---|---|---|
| `create/` | `skills/create-local-app` | ✅ 存在 |
| `verify/` | `skills/frontend-qa` | ✅ 存在 |
| `design/` | `skills/frontend-design` | ✅ 存在 |
| `accessibility/` | `skills/accessibility` | ✅ 存在 |
| `react/` | `skills/react-best-practices` | ✅ 存在 |
| `dom/` | `skills/ionic-react-local-app` | ✅ 存在 |
| `canvas-2d/` | `skills/canvas-2d-local-app` | ✅ 存在 |
| `threejs/` | `skills/threejs-local-app` | ✅ 存在 |
| `phaser/` | `skills/phaser-2d-local-app` | ✅ 存在 |
| `babylon/` | `skills/babylon-3d-local-app` | ✅ 存在 |
| **`update/`** | — | 🚨 **不存在，必须新写** |

⇒ 是「迁移 10 + 新写 1」，不是「迁移 10」。`update/` 是方案新引入的入口
（`operation:"update"`），它承载 revision 语义，也是 Phase 4 里**标注（annotation）
转成修改指令**的落点。方案的工作量估计需要据此调整。

📝 十个 skill 全部存在这件事本身也是新的——`phaser-2d-local-app` 与
`babylon-3d-local-app` 是评审期间出现的。再次印证第 5 条。

### 9.2 `skills/verify/` 必须携带的内容（今天的 `frontend-qa` 没有）

**🚨 最重要的一条：捕获仪器。** `frontend-qa` 现在要求
「A claim without a screenshot, log … 」并让 verifier「capture two frames」，
但**全篇没有一个字提到用哪个 API 取帧**。而 spike 实测：

- `takeSnapshot`（= 生产 `capture_ui` 现在走的路径，`LocalAppWebView.swift:572`）
  对**离屏** WKWebView **恒返回 rgb(0,0,0)**；
- `drawHierarchy` 正常，且能捕获 2D canvas 与 WebGL。

⇒ 若冒烟门用离屏视图，而 skill 让 verifier「截两帧比对」，verifier 会拿到
**两张全黑的图**，然后按 `frontend-qa` 现有的「motion」判据报告「无变化」——
一个**假的失败**，且看起来像 app 坏了。skill 必须写明：
**在离屏语境下不得使用 `takeSnapshot`**，帧证据由宿主侧 `drawHierarchy` 提供。

**其余必须进 `verify/` 的内容**（均来自 verification 设计，今天散落在设计文档里）：

| 内容 | 为什么必须在 skill 里 |
|---|---|
| 六条判据表（1/2/6 阻塞，4/5 建议） | verifier 需要知道**哪些会否决交付**，否则它会把建议项当阻塞项报 |
| **判据 4/5 是建议不是阻塞**，及其四条误阻塞路径（菜单态 canvas、JPEG 质量档位被读成运动、Android reload 的 `about:blank` 白帧、WebContent 进程死亡） | 这四条各由不同评审镜头发现；不写进 skill，verifier 会重新踩 |
| 判据 6 必须覆盖 `console.error` | React 19 的 `onCaughtError` 路由到 `console.error`，**永不到 `window.onerror`**；模板的 `error-boundary.jsx` 只有 `getDerivedStateFromError` ⇒ 渲染崩溃的 app 会出兜底页：**有 DOM、有配色文字、无未捕获异常 —— 三条判据全绿出厂** |
| 载荷降级语义 | `inspect_ui` 结果被降级（`truncated` 含 `runtimeErrors`）⇒ 判 `infrastructure_unavailable`，**不判通过**。观测不到 ≠ 没异常 |
| 失败分类 | 任何来自门发出的 UI 请求的 `Err` ⇒ `infrastructure_unavailable`；source defect **只从成功的载荷**里判 |
| **Android 无冒烟能力 ⇒ `verification_unavailable`** | 绝不能把 iOS-only 的成功推广到 Android，也绝不静默判通过 |
| ⚠️ 载荷预算降级**两端都只钉了拼写、从未执行过** | Phase 1a 真机验收发现：`testSnapshotDegradesInAFixedOrderAndSaysSo` 断言的是注入 JS 的**文本**。超 256 KiB 是**硬失败不是截断** ⇒ 对一个真实的忙页面，`inspect_ui` 可能返回错误而非降级载荷。skill 必须让 verifier 把这种情况报成基础设施问题，而不是 app 缺陷 |

### 9.3 `agents/verifier.md` 的权限形状

方案已经说对了一半（「不得编辑源码」「输出固定 `qa_report`」）。要补两条：

1. **verifier 不得自报门禁结论。** 今天的 `webview_checked` 就是 verifier 自填的布尔值
   （`local_app_workflow_core.js:518`），背后零证据。插件化之后，
   **`qa_report` 里不得包含任何等价于「我验过了」的字段**——
   机器事实由 host core 的 `SmokeReport` 提供，verifier 只在其**之上**做主观判断。
   这是判据 3 在 agent 定义层面的落地。
2. **工具白名单必须显式排除写工具**，且要有一条测试断言它排除了
   （方案的验收标准写了「verifier 无源码写工具」，但没说怎么证明）。
   建议：断言 verifier 的解析后工具集与 builder 的**交集为空**（写工具侧）。

### 9.4 明确**不**进插件的部分

| verification 能力 | 归属 | 理由 |
|---|---|---|
| `inspect_ui` / `capture_ui` / `act_on_ui`（Phase 1a） | **host core** | 是 WebView 能力，不是创作能力；插件关闭时仍存在，只是 agent 侧被 gate |
| `image-read`（Phase 1a） | **host core** | 同上 |
| 冒烟门 + `SmokeReport`（Phase 2） | **host core** | 见第 2 条 |
| `StoreAppAnnotation` / `DeleteAppAnnotation` / build generations（Phase 3） | **host core，动协议** | 协议与存储是 host 的 |
| iOS overlay / 标注状态机 / 副驾驶条（Phase 4/5） | **host core 客户端** | 是原生 UI，插件装不下 |
| 主观 QA 评审（设计、可用性、视觉、交互探索） | **插件 `verifier`** | 可关闭 |
| renderer 专属的验证要点（canvas 帧率、three.js 场景图等） | **插件 renderer skills** | 随 profile 走 |

一句话：**插件承载「怎么判断好不好」，host core 承载「能不能加载、有没有报错」。**

### 9.5 `skills/update/` 与标注的衔接（Phase 4 的落点）

`update/` 是新写的，它同时是 verification Phase 4 的接口：用户在 app 上框选问题、
写一句描述 ⇒ 组装成 `revision_prompt` ⇒ 走 `operation:"update"`。所以它必须写明：

- `revision_prompt` 可能携带**标注上下文**（区域截图 + 用户描述 + 该区域的 DOM 摘要）；
- 标注的生命周期由 host core 管（`annotations/` 原子目录、build generation 清理），
  skill 只消费不管理；
- **最小修改原则**：`update` 不得借机重写无关文件——这条今天由
  `local_app_workflow_core.js` 的 repair 语义隐含表达，迁移时必须显式写进 skill。

---

## 10. 🚨 「使用已有 app」这一整类完全没被覆盖（方案与本评审第 9 节都漏了）

方案的 11 个 skill 全是**创作**技能（create/update/verify/design/accessibility +
5 个 renderer）。**没有任何一个 skill 讲「怎么使用一个已经存在的 app」。**

这不是可有可无的补充——它正好落在方案自己划的那条禁用边界上。

### 10.1 禁用后的状态是「有工具、没说明书」

方案说插件关闭时「List/Get/Runtime/Logs/QueryData、已有 app 列表和原生
preview/runtime 控件继续可用」。同时方案第 9 步要「删除根 `skills/` 下旧 local-app skills」。

而**全仓库唯一描述 `LocalAppQueryData` / `LocalAppMutateData` 语义的文本，
就在 `skills/create-local-app/SKILL.md` 里**（`:280`、`:304`、`:481`、`:520`）——
一个即将被迁进插件、并默认关闭的文件。

⇒ **插件关闭后：工具还在，唯一的说明书没了。**
用户在 app 会话里说「把我记账 app 里那笔 100 改成 200」，模型手里有
`LocalAppMutateData`，但仓库里已经没有任何东西告诉它这个工具的操作形状约束
（`:304` 那条「每个 operation 必须精确是……」）。这比「工具和说明一起消失」更糟，
因为它是**静默降级**：模型会尝试，然后以它自己猜的形状调用。

### 10.2 工具分类不完整

`defaults_per_tool.rs` 里共 **24 个** `LocalApp*` 工具。方案的禁用清单
（Create、Scaffold、Manifest、Build、dependency update、Inspect/Capture/Act、
checkpoint mutation、background mutation）与保留清单
（List/Get/Runtime/Logs/QueryData）**加起来不等于 24**，中间有一批没有归属，例如：

| 工具 | 默认 | 属于创作还是使用？方案未答 |
|---|---|---|
| `LocalAppMutateData` | Deny | **使用**——改自己 app 里的数据不是创作 |
| `LocalAppEvents` | Deny | 使用（app 作用域事件流） |
| `LocalAppCheckpointList` | **Allow** | 使用（只读历史）——但 checkpoint *restore* 是创作？ |
| `LocalAppBackgroundList` / `Status` | — | 使用（看后台任务） |
| `LocalAppTool` / `LocalApp` | — | 未分类 |

**要求**：方案必须给出 **24 个工具的完整二分表**，而不是两个示例性列表。
判据建议用一句话表述并逐个套用：**「它会改变 app 的源码或 manifest 吗？」——
会 ⇒ 创作（随插件关闭）；不会 ⇒ 使用（始终可用）。**
按这条，`MutateData`（改数据不改源码）属于使用，`CheckpointRestore`（回滚源码）属于创作。

### 10.3 建议：`skills/use/` 留在 host core，不进插件

新增一个**不属于插件**的 `use` skill（或并入 host core 的常驻上下文），内容是：

- 24 个工具里「使用」那一半的操作形状与约束（从 `create-local-app` 里**抽出**而非复制）；
- 数据读写的边界：`QueryData` 只读、`MutateData` 的 operation 形状、collection 必须已在
  manifest 声明（否则报 `collection "x" is not declared by the app manifest`——
  这是 2026-08-07 真机 QA 里最严重的那条 F1）；
- runtime 的启停语义：`start` 对运行中的 runtime 是成功的 no-op；
  **restart 不重新加载页面**（端口刻意稳定 ⇒ URL 逐字节相同 ⇒
  `LocalAppWebView.swift:1104` 的 `guard loadedURL != url` 直接返回）——
  这条是 spike 顺带证实的，今天写在 `docs/local-apps/HANDOFF.md` 里当作提示词纪律，
  但它其实是**使用者需要知道的运行时事实**；
- 插件关闭时**它仍然在**——这正是它必须留在 host core 的理由。

这样禁用插件的语义才是干净的：**失去「造和改」，保留「用」，而且「用」是有说明书的。**

### 10.4 对 verification 的直接影响

`verify` / `verifier` 需要调用的**恰恰是使用类工具**（跑起来、查数据、看日志、
读 runtime 状态）。所以：

- verifier 的工具白名单 = 「使用」那一半 + `Inspect/Capture/Act`（观测类），
  **不含任何写源码的工具**——这与第 9.3 节的要求一致，现在有了明确的划分依据；
- 若 `use` skill 进了插件并随之关闭，则**插件关闭时 host core 的冒烟门仍要工作，
  但描述其所用工具的文本没了**。这是第 2 条（冒烟门必须留 host core）之外
  又一个「门的依赖不能住在可关闭的容器里」的实例。
