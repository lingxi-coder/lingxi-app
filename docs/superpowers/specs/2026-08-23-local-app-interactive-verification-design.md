# 本地应用：可交互的验收与编辑

日期：2026-08-23
平台：iOS（客户端交互）；引擎与协议改动两端共用
协议基线：`7.0.0`（`client-protocol/src/version.rs:56`），blessed major `7`

## 起因

用户报告 create local app 的验证环节「能力很差、速度很慢、不能和用户互动」。
调研（8 个并行读者覆盖 workflow 脚本 / 引擎设备工具 / 两端 WebView / 协议 /
会话作用域 / 技能与文档 / 既有先例）确认这三条都成立，且根因各不相同。

### 为什么差

验证是一个 subagent 填一份 JSON，gate 读这份 JSON。
`local_app_build_workflow.js:103-111` 自己写着：

> Every field here is still the AGENT's transcription — the workflow VM has no
> tool-calling primitive of its own… A verifier that never looked and reports
> `{status:'not_applicable', canvas_surfaces:0}` is still indistinguishable from
> an honest DOM app; closing that needs a host-side ledger of `LocalAppCaptureUi`
> calls, **which does not exist yet**.

代码注释里记着三次真实翻车：

- 一次真跑的贪吃蛇返回 `ok:true` 全绿 + 十句称赞，驱动了一个修复代理去修一堆
  工作正常的东西；fixture 全写 `[]` 所以从没抓到（`local_app_build_workflow.js:57-66`）
- 第一版 render gate 判 `dom_elements_seen === 0`，真实生成的游戏直接走过去
  （`:94-101`）
- Generate 阶段把 100 轮预算全烧在自我 QA（28 act / 16 inspect / 18 sleep），
  `max_turns_exhausted`，**Verify 一次都没跑**（`:185-191`）

更根本的一条：`verificationMode` 的 `smoke` / `confirmed-targets` /
`full-matrix` **只是三段散文**，schema、gate、工具集完全相同
（`local_app_workflow_core.js:262-268`）。而 `full-matrix` 要求覆盖
「iPhone、Android phone、iPad 横竖屏、桌面」——`apps/engine-mobile/src/lib.rs:196-235`
**根本没注册 Browser 工具**，真机上这段要求是在逼模型编数据。

### 为什么慢

零并行。Design → Generate&Build → Verify → repair → Verify 是纯串行，
每段都是一个冷启动 subagent（`max_turns = 100`，`agent/src/builtins.rs:67`）。
`balanced` 3 次调用、`thorough` 最坏 7 次。每次 UI 观测是一次
host↔client 往返（`UI_TIMEOUT = 2min`，`local_apps_host.rs:42`），
而 `LocalAppsStore.swift:1116` 在**每一次** UI 动作前还要多做一次
`getDetails(appID:)` 引擎往返。`LocalAppBuild` 是真机 iSH/proot 里的离线
`vite build`，最坏 6 次。

### 为什么不能互动

不是「不能说话」——`Workflow` 工具 `async_launched` 立刻返回
（`tools/workflow/src/lib.rs:893`），主 turn 在 workflow 开跑那刻就结束了，
composer 并没被锁。问题是**说了进不去那条流水线**：

- workflow VM 的全局只有 `log` / `phase` / `agent` / `parallel` / `pipeline` /
  `budget` / `args` / `workflow`（`workflow/src/lib.rs:29-120`）——
  **没有任何 ask / input / pause 原语**
- `AskUserQuestion` 和 `SendUserMessage` 对每个 subagent 平铺禁用
  （`agent/src/tool_resolver.rs:95-113`、`agent/src/builtins.rs:368-376`）
- `ResumeWorkflow` 只带 `task_id`，**没有 payload**（`commands.rs:345-348`）
- 用户在此期间打的字会开一个**全新的、和 workflow 无关的 turn**

同时，界面上也没有任何「指哪儿」的手段：`inspect_ui` 算完
`getBoundingClientRect()` **只留一个 `visible: bool` 就把几何扔了**
（`LocalAppWebView.swift:675`），`capture_ui` 的入参只有 `{app_id}`、
不支持裁剪（`local_apps_host.rs:3602-3605` 明确拒绝过），
`makeUIView` 返回的是**裸 `WKWebView`**、没有任何可以画东西的图层
（`LocalAppWebView.swift:919-930`），`Sources/LocalApps/` 里
`DragGesture` / `UIPanGestureRecognizer` **零命中**。

## 设计取向

把「机器造」和「人验收」分开：

> **流水线只负责把东西造出来并证明它没崩；验收发生在对话里。**

这不是新建一条通道，而是承认既有通道已经够用——**每个 app 本来就是一个
会话作用域**（`ConversationScope.localApp(id)`，cwd = `apps/<id>/workspace`），
`workspace/LINGXI.md` 自动注入且已含完整编辑契约与 verify 工具清单
（`local_apps_host.rs:3110-3244`）。缺的只是**入口**和**指哪儿**。

### 已确认的选择

| 决定 | 选择 |
|---|---|
| 自动 verify 的角色 | 退成便宜的冒烟门；验收交给人 |
| 光标可视化形态 | 静态标注在截图上（非实时动画光标） |
| 反馈提交节奏 | 默认攒清单，单条可「立即修」 |
| 光标回放的位置 | app 界面上的回放条；**同时回喂给 LLM** |
| 编辑的适用范围 | 库里任何 app、任何时候 |
| 落地平台 | 客户端 UI 仅 iOS；引擎与协议两端共用 |
| 进入标注模式 | 悬浮按钮切模式 |
| 界面布局 | 一条三态复用的副驾驶条 |

### 非目标

- Android 客户端 UI（引擎与协议改动对其可用，UI 后补）
- 修 `SendPrompt.images`（移动端定义了但从不读，`host.rs:6146`）——
  标注图走落盘，不走消息附件
- 实时动画光标
- 跨 app 引用（会污染 scope 边界，而 scope 正是把 agent 钉在正确工作区的东西）
- 桌面/Web 客户端

## 架构

```
构建期（workflow）                        验收期（对话）
─────────────────────                    ──────────────────────
Design → Generate & Build → 冒烟门        用户玩 app
                             │             │ 点悬浮按钮进标注态
                     宿主自己跑六条判据      │ 拖框 → 描述
                             │             ▼
                        workflow 结束   标注入清单（客户端本地）
                             │             │ 提交
                             └──► app 打开 ─┘
                                           ▼
                                 StoreAppAnnotation × N（引擎落盘）
                                           ▼
                                 SendPrompt（正文含路径 + 矩形 + 命中元素）
                                           ▼
                                 app scope 的会话（已存在）
                                           ▼
                                 agent 改源码 → LocalAppBuild
                                      → LocalAppRuntime restart
                                           ▼
                                 热重载 → 回到「用户玩 app」
```

右侧一列**没有 workflow**：它是 app 自己 scope 里的一次普通对话轮次。
因此「刚创建完就改」与「三个月后再改」走同一条路径，前者只是会话恰好还热着。

### 标注（Annotation）

四个部分：

- **矩形** — CSS 像素，与 `pointer` 同一坐标系
- **描述** — 用户的一句话
- **裁剪图** — 该矩形区域的 JPEG
- **命中元素** — 该矩形内的元素列表（`elementId` / `role` / `name` / `rect`）

命中元素是让 agent 从「猜」变成「查」的关键：给一张裁剪图，它只能猜「你说的
那个按钮」对应哪个组件；给 `{elementId:"score-badge", role:"status",
name:"12 分", rect:[48,100,72,24]}`，它能直接 grep 到源文件。

canvas 应用没有可命中的元素，标注退化为「矩形 + 图 + 描述」，agent 靠矩形在
CSS 坐标系里的位置对照绘制代码。这是可接受的降级，不是失败。

## 协议改动

`ResolveAppUiRequest.result_json` 和 `AppUiRequestDto.value` **都是不透明的
`Option<String>`**（`commands.rs:438`、`local_apps.rs:774`），所以绝大部分
改动不触及协议：

| 能力 | 承载 | 动协议 |
|---|---|---|
| `inspect_ui` 返回元素几何 | `result_json` 内的 JSON | 否 |
| `capture_ui` 区域裁剪 | `value = "x,y,w,h"` | 否 |
| 截图上的光标标记 | 客户端绘制；`result_json` 加 `last_action` | 否 |
| 操作回放条 | 纯客户端，源自既有 `AppUiRequest` | 否 |
| 标注 → 消息 | 复用 `SendPrompt` | 否 |
| **存标注** | 新命令 | **是** |

### 唯一的新命令

```rust
ClientCommand::StoreAppAnnotation {
    app_id: String,
    rect: String,          // "x,y,w,h"，CSS 像素
    note: String,
    image_base64: String,  // 区域 JPEG
}
// → ClientEvent::AppAnnotationStored { app_id, path, error }
```

引擎写入 `apps/<id>/workspace/.lingxi/annotations/<ts>-<n>.jpg` 与同名
`.json`（含 rect / note / 命中元素 / 视口尺寸），回**工作区相对路径**。

**为什么不让客户端直接写这个目录**：仓库已因「一个目录两套推导」栽过一次——
ios-ish runtime 自己推 `app_sandbox_root`、引擎从 Swift 拿，两条推导差三段
目录，`FileManager` 建的 guest 目录对 guest 根本不存在。目录布局是引擎的知识
（`AppLayout::workspace_rel()`，`local-apps/src/manifest.rs:504-508`）；客户端
一旦开始自己拼这条路径，就多了一份迟早要漂的副本。这条命令同时是尺寸闸口：
超标在引擎侧降质或拒绝，不必在两侧各写一遍。

### 变体追加位置与 uniffi 风险

新变体必须**追加到枚举末尾**（uniffi 序数是位置相关的）。
本设计不给 `AppUiActionKindDto` 增加任何带数据的变体——给一个原本无字段的
枚举加带数据变体会让 uniffi 生成 Kotlin `sealed class` 并把现有全部常量
重命名（`CLICK`→`Click`），这是只在 Android 上炸的破坏性变更
（`local_apps.rs:726-742` 有记录）。坐标一律沿用逗号打包字符串。

### bless 四步（缺一步 version guard 必红）

1. `current_contract_index()` 补 `put(...)`（AppUi 段在 `version_guard_test.rs:1428-1449`）
2. `snapshots/command/` 加 golden + `snapshot_test.rs` 表项
3. `clients/shared/src/protocol.ts` 加 TS union 成员 **与手写运行时守卫**
   （`clients/shared/test/snapshots.test.ts:1324, 1351, 1374, 1384`），并跑
   `clients/shared` 自己的 37 条 `npm test`
4. `BLESS=1 cargo test -p client-protocol --test version_guard_test --test snapshot_test`

第 3 步最易漏。上一轮只改了 Rust 侧、TS 镜像仍是旧版本号，桌面 BridgeClient
握手直接硬失败。**且没有任何测试断言 TS 常量等于 Rust 常量**——漂移是静默的。

⚠️ 移动端**没有协议握手**（`grep client_protocol_version` 在 engine-mobile /
clients/ios / clients/android 中 0 命中），版本偏斜不可检测，只有 append-only
纪律在保护它。

## 引擎改动

### `inspect_ui` 返回元素几何

`LocalAppWebView.swift:675` 与 `LocalAppWebView.kt:636-645` 已经在算
`getBoundingClientRect()`，只留了 `visible`。改为同时输出
`rect: [x, y, w, h]`（CSS 像素，取整）。元素上限仍为 200，`deepQuery` 上限 400
不变。快照体积增加约 200 × 4 个整数。

### `capture_ui` 区域裁剪

`value` 可选带 `"x,y,w,h"`。iOS 侧把 `WKSnapshotConfiguration.rect` 设为该
区域（点单位 = CSS 像素），沿用既有的长边 1024 像素上限与
`[0.7, 0.5, 0.3]` 质量阶梯（`LocalAppWebView.swift:475-520`）。无 `value`
时行为完全不变。

### 光标标记

客户端在 `captureFrame` 里，用 `UIGraphicsImageRenderer` 在快照上画一个
标记圈，再编码 JPEG。**仓库中没有任何服务端图像绘制库**——`imageproc` /
`tiny-skia` / `resvg` / `ab_glyph` 都不是依赖，`image` 只用于
decode/encode/resize 和一处 `crop_imm`——所以绘制必须在客户端。

标记位置来自**紧邻的上一次带坐标动作**（客户端本地记住 `{kind, x, y, at}`）。
仅当该动作发生在 5 秒内才绘制。`result_json` 增加
`last_action: {kind, x, y, age_ms}`，让 agent 知道图上那个圈是什么。
`value` 含 `"clean"` 时不绘制。

同一个标记帧进操作回放条。**这满足「回喂给 LLM」**：agent 请求截图时拿到的
就是带标记的那张，能看见「我点的地方和我以为的地方不一样」。

### 冒烟门：宿主账本

替换 workflow 里由 agent 自述的 gate。构建完成后**由宿主发起**一组固定调用，
agent 不参与：

1. 运行时已启动且 `preview_url` 非空
2. `inspect_ui` 取到快照：DOM 应用 `elements` 非空；canvas 应用 `canvasCount ≥ 1`
3. `capture_ui` 取两帧，间隔 1.5 秒
4. 首帧非纯色 —— 判据是**整帧像素方差低于阈值**，即「整屏只有一个颜色」。
   这是白屏/黑屏的形状；一个真实设计即使极简也有文字与边框，不会命中。
   若某应用确实整屏单色，由第 2 条（元素或 canvas 存在）承担该场景，
   本条在诊断里注明后放行。
5. canvas 应用：两帧必须不同
6. `read_logs` 无未捕获异常

全过 → 构建成功。任一不过 → **一次**修复轮；再不过 →
**带诊断把 app 交给用户**，而不是像现在这样 `throw` 掉整个构建结果
（`local_app_workflow_core.js:351-360`）。

这六条**没有一条经过模型**：宿主发的调用、宿主读的返回、宿主做的判断。
agent 无法「声称」自己看过。这正面回应了 `local_app_build_workflow.js:111`
自己记下的 TODO。

### workflow 脚本删减

- 删 Verify 与 repair 阶段（`local_app_workflow_core.js:260-312`）。
  agent 调用数从 3–7 降到 **2**（Design + Generate&Build），
  冒烟门触发修复时 **3**。
- 删 `STRATEGY_POLICY` 中的 `verificationMode` 字段与三段散文。
  `runDesign` 保留（`local-canvas-build` 拒绝 `fast` 的理由仍成立：
  `fast` 跳过 Design，而画面应用的难点全在机制与帧循环上）。
  `maxRepairRounds` 保留但改由**冒烟门**消费，不再由 verify 的自述结果消费。
- 删 `args.revision_prompt`（只到 Design、`fast` 下被丢弃的死线；
  唯一写入者是测试辅助 `builtins.rs:1059-1078`）。其职责由标注承担。
- `builtins.rs` 中断言阶段序列与 agent 计数的测试同步更新
  （`:280-286`、`:272-278`、`:294-300`、`:441`、`:456-465`、`:817-822`）。

### 顺带修掉的既有缺陷

| 缺陷 | 位置 |
|---|---|
| `local-canvas-build` 拿不到 workspace lease（字面量比较） | `tasks/src/handlers/local_workflow.rs:1987` |
| 每次 UI 动作前多一次 `getDetails(appID:)` 引擎往返 | `LocalAppsStore.swift:1116-1121` |
| 确认过的 spec 从不落盘 | task handler 在 workflow 启动时写 `workspace/.lingxi/spec.md`，`LINGXI.md` 增加指向它的一行 |

spec 落盘由**宿主**而非 agent 执行：`args.spec` 在启动参数里已是确定值，
让模型转写只会引入失真。

## iOS 客户端

### 图层

`makeUIView` 目前返回裸 `WKWebView`（`LocalAppWebView.swift:919-930`）。
改为返回一个容器 `UIView`，`WKWebView` 铺满，上方叠一个透明的
`LocalAppAnnotationOverlayView`。非标注态时 `isUserInteractionEnabled = false`，
所有触摸原样落到 WebView，**app 的交互零影响**。

不使用页面内注入的 overlay：CSP 虽允许（`script-src 'self' 'unsafe-inline'`，
`LocalAppWebView.swift:1076`），但注入元素会进入 app 自己的 DOM，被
`inspect_ui` 的 `deepQuery` 扫到，污染验证快照，也可能被 app 的样式影响。

### 标注模式

- 悬浮按钮（右下）切换。进入时：overlay 接管交互、整层压暗
  `rgba(8,10,15,0.55)`、顶部出现蓝色模式条。
- `UIPanGestureRecognizer` 拉矩形；四角手柄可再调整；最小 24×24 点，
  小于此视为误触取消。
- 松手 → 副驾驶条变成输入框。**不弹模态**，避免遮住刚框的区域。
- 视图坐标 → CSS 像素：WKWebView 的点坐标即 CSS 像素（缩放固定），
  但仍以 `capture_ui` 回报的 `viewport` 为准做一次校正，与
  `skills/frontend-qa/SKILL.md:27-34` 记录的换算保持同一套。

### 副驾驶条（三态）

| 状态 | 触发 | 内容 |
|---|---|---|
| agent 操作中 | 收到 `AppUiRequest` | 横排操作帧，每帧带标记圈；点击放大 |
| 刚框完 | pan 结束 | 输入框「这里怎么了？」 |
| 有待提交标注 | 清单非空且非标注态 | 药丸清单 + 「开始修这 N 个问题」 |

数据源：`LocalAppsStore` 已收到每一条 `AppUiRequest`
（`LocalAppsStore.swift:964-1005`），并已有从未渲染过的在飞状态
`pendingUIRequestAppIDs`（`:86`，唯一消费者是 `LocalAppsLibraryView.swift:47`
的路由）。回放条不需要引擎提供任何新信息。

回放帧上限 24 条（与既有 workflow 日志上限一致，
`ConversationSource.swift:3057-3060`），超出丢弃最旧的。

### 提交时组装的消息

```
我在运行「贪吃蛇」时标了 3 个问题。视口 393×852 CSS 像素。

1. 分数一直不涨
   区域 x=44 y=96 w=118 h=74
   截图 .lingxi/annotations/1787543300-1.jpg
   区域内元素:
     #score-badge  role=status  name="12 分"  rect=[48,100,72,24]

2. 按钮太小
   ...

请逐条修复，然后 LocalAppBuild + LocalAppRuntime restart。
改完告诉我改了什么，我再看一遍。
```

这段格式是与 agent 的实际契约，应有测试钉住。

### SwiftUI 约束

`RootView.body` 加到约 30 个修饰符即触发
`unable to type-check this expression in reasonable time`，且**超时期间该
表达式从未被类型检查**（真错会一直藏着）。副驾驶条与 overlay 的接线不要
堆进 `RootView`，放在 `LocalAppDetailView` / `LocalAppPreviewView` 层，
并按既有做法拆成多个声明。

`@Published` 从 `willSet` 发布：任何 `.onReceive($x)` 必须用**投递值**，
不能回头读属性。

## 引导

复用既有一次性引导设施的持久化模式（`Sources/Onboarding/SetupWizardView.swift`
的 `@AppStorage` on `AppState`），但**不用全屏向导**——要教的是一个就地手势，
脱离真实界面讲了记不住。

**第一次进验收态**：真实界面上覆半透明遮罩，手指从左上拖到右下拉出矩形的
动画，一行字「发现问题？在界面上拖一个框，告诉我哪里不对。」，一个「知道了」。
**若用户直接上手拖框，引导立即消失**——教学目的达成即退场。

**第一次看到回放条**：旁边浮一行小字「我正在操作你的 app，这里能看到我点了
哪儿」，3 秒自渐隐，无需点击。用独立标志，因为两件事可能相隔很久。

可重放入口放进 app 详情页的 `ellipsis.circle` 菜单——该菜单目前**只有
「重启」一项**（`LocalAppDetailView.swift:108-128`）。

⚠️ 文案写进 `clients/translations/*.json`（真源），**不是
`Localizable.xcstrings`**（`generate.py` 的生成产物，手改下次全还原）。
缺 key **不报错不崩，直接把 key 本身当文案显示**——此坑已踩过一次
（`local_apps_create_intake_seed %@` 从没进过 catalog，意图对话实际收到的是
key 字面量）。iOS 占位符在 KEY 里、Android 在值里，常需两个 key。

## 边界与错误处理

| 场景 | 行为 |
|---|---|
| 构建 workflow 仍在跑时用户提交标注 | 客户端**保持**提交，按钮显示「构建中，稍后自动提交」，收到该 app 的 workflow 结束事件后自动发出。标注是客户端本地状态，等待期间可继续增删。 |
| `StoreAppAnnotation` 失败（磁盘满、超限） | 该条标注留在清单里并标红，其余照常提交；消息正文只列成功落盘的条目 |
| 裁剪图超尺寸 | 引擎按既有质量阶梯降质；仍超则拒绝该条并回 `error` |
| 矩形内无命中元素 | 正常提交，元素段写「（无 DOM 元素，canvas 区域）」 |
| 标注模式下 app 仍在自行动画 | 允许——截图取的是松手瞬间的帧；矩形是坐标不是快照 |
| `capture_ui` 区域越界 | 引擎钳到视口内；完全在视口外则回错误，不静默重定向 |
| 冒烟门第 4 条误判（应用本来就是纯色设计） | 该条降为警告而非阻塞，写进交付诊断 |
| 用户在 agent 操作中途进标注模式 | 允许。overlay 接管触摸不影响 `act_on_ui`（后者是注入 JS 合成事件，不经过 UIKit 触摸链） |

**并发写入风险（明确记录，未完全消除）**：客户端保持提交只是 UX 层的护栏，
另一个客户端或另一条会话仍可能在 workflow 跑的同时对同一工作区发起编辑轮次。
`WorkspacePermissionLeaseRegistry` 是**授权**而非**互斥**机制
（`permission/src/workspace_lease.rs:60-83`），不构成保护。若实测出现真实
冲突，再引入 app 级的编辑互斥；本设计不预先建造它。

## 测试策略

### 引擎 / Rust

- 冒烟门六条判据各有独立测试，**每条都要有反向用例**：白屏应用第 4 条红、
  静止的 canvas 应用第 5 条红、抛异常的应用第 6 条红。
- 反向用例不得写成 `if cmd; then 报错; fi`——`if` 会暂停 `set -e`，把脚本
  崩溃判为通过。用「退出码恰为 1 且输出无 Traceback」的判据。
- `StoreAppAnnotation` 的路径推导测试必须钉住**真实设备路径形状**
  （`apps/<id>/workspace/.lingxi/annotations/`），而非重复实现里的推导。
- 协议：golden + TS 快照 + `clients/shared` 的 37 条 `npm test`。
- workflow 脚本：`builtins.rs` 中阶段序列与 agent 计数断言更新为
  `["Design","Generate & Build"]` / `agent_calls: 2`。
- 跑测试**必须捕获完整输出到文件再 grep**——只 grep `FAILED` 会丢掉
  `failures:` 块里的测试名。用 `--no-fail-fast`：cargo 在第一个失败的
  **二进制**处停止，测试总数下降即使 0 failures 也是红旗。

### iOS

- 注入 JS 的几何输出：扩展 `LocalAppsStoreTests` 中已有的 shadow-DOM walk
  测试（`executionSource` 是 internal 正是为此）。
- 消息组装格式：快照测试。
- 坐标换算：视图点 → CSS 像素 → `capture_ui` 裁剪，端到端一条。
- ⚠️ WebView 与预览路由**当前没有 `accessibilityIdentifier`**，XCUITest
  无法寻址。overlay 需要自己的 id。
  **SwiftUI 容器上的 `accessibilityIdentifier` 会覆盖所有子元素的 id** ——
  需 `.accessibilityElement(children: .contain)`，否则源码里 grep 到的 id
  在运行时根本不存在。
- UI 测试要 `-testLanguage zh-Hans`。
- 引擎是预编译 xcframework，**`xcodebuild` 不重编 Rust**。引擎侧改动要上机
  验证必须先重建 xcframework，判据是
  `strings -a clients/ios/Frameworks/LingxiCodeFFI.xcframework/ios-arm64/libios_framework.a | grep <新字符串>`；
  装机后验 `LingxiCode.app/LingxiCode.debug.dylib`（主二进制只有 91 KB，
  grep 它会得 0 = 假阴性）。重建需
  `LINGXI_REUSE_STAGED_LINUX_RUNTIME=1`。

### 真机

冒烟门与回放条**必须上机验证**，不接受单测绿即交付：
重建视口/几何的判据历史上必然逃过单测（6 个单测 + 3 轮评审曾放行一个真机
100% 失效的判据）。上机清单：DOM 应用与 canvas 应用各走一遍完整流程，
包含框选、提交、修复、热重载。

## 分阶段落地

| 阶段 | 内容 | 可独立验证 |
|---|---|---|
| 1 | `inspect_ui` 几何 + `capture_ui` 区域 + 光标标记 | 是（纯 result_json/value，无协议改动） |
| 2 | 冒烟门宿主账本 + 删 Verify/repair + 三个既有 bug | 是 |
| 3 | `StoreAppAnnotation` 命令 + bless 四步 | 是 |
| 4 | iOS overlay + 标注模式 + 副驾驶条三态 | 需 1 与 3 |
| 5 | 引导 + 文案 | 需 4 |

阶段 1、2 之间无依赖，可并行；3 是 4 的前置。

## 已知约束

1. **协议**：`7.0.0`，blessed major 7。additive 也需重新 bless。
   移动端无握手，偏斜不可检测。
2. **uniffi**：变体追加到末尾；不给无字段枚举加带数据变体。
   `ClientEvent` 的 docstring 会被烤进定容元数据缓冲区且**已接近上限**——
   新事件注释用 `//` 而非 `///`。
3. **生成绑定**：Android `bindings/android_aar.kt` 是签入的构建产物；
   iOS uniffi 绑定是 gitignored 构建产物。改 wire DTO 不重新生成 ⇒
   客户端不可能编过。
4. **CSP**：`script-src 'self' 'unsafe-inline'`；`wasm` 与 `Worker` 被实测拦死。
5. **Ionic**：`format:"iife"` 下无法按需引入，`@ionic/react` barrel
   ~1.4 MB 固定底盘不 tree-shake；重度使用 Shadow DOM（产物中 `attachShadow`
   73 处），两端的 `deepQuery` 已穿透（`LocalAppWebView.swift:605-637`、
   `LocalAppWebView.kt:562-578`），几何输出必须走同一条 walk。
6. **设备**：真机 rootfs 无 Node 工具链；`AppManifest::hash()` 序列化整个
   结构体且绑定 SQLite schema ⇒ 新增字段必须 `skip_serializing_if`，
   否则每个既有应用立刻 `database manifest mismatch`。
7. **`ClientCommand` 是 `#[non_exhaustive]` 且分发有兜底臂** ⇒ 新命令加了
   也编译通过、但被静默忽略，必须显式加处理臂。
8. **allow-by-default 名单有两份手写副本**：`permission/src/defaults_per_tool.rs`
   与 `local_apps_tools.rs:674`。改一处另一处必红。
9. **契约索引 `current_contract_index()` 是手写的** ⇒ 不加条目版本守卫静默放行。
10. **构建环境**：cargo target 曾涨到 84G 撑爆磁盘（`errno 28`，表现为随机
    `could not compile`，看着像代码错）。`rm -rf target/debug/incremental` 安全回收。

## 未决问题

无阻塞项。以下在实现中定，不影响本设计成立：

- 冒烟门第 4 条「非纯色」的方差阈值具体取值——需要用几个真实生成的应用标定。
- 回放帧缩略图的具体尺寸——受 256 KiB `result_json` 通道与回放条高度共同约束。
（标注清单的存活期已定：保留到该 app 的下一次成功构建为止。图本身已落盘，
清单只是「还没提交的那几条」，一次成功构建即意味着它们要么已被处理、
要么已过时。）
