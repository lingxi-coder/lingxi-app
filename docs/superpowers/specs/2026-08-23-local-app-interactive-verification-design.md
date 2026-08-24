# 本地应用：可交互的验收与编辑

日期：2026-08-23（2026-08-24 第四轮对抗式评审后重写）
平台：iOS（客户端交互）；引擎改动两端共用
协议基线：`7.0.0`（`client-protocol/src/version.rs:56`），blessed major `7`

> 本文经四轮对抗式评审。第四轮砍掉的机制多于新增的：协议改动 5 处 → **1 处**，
> bless 两轮 → **一轮**，workflow VM 新原语 → **不需要**。用户可见的功能没有缩水。
> 修正记录见文末，不要在没读它的情况下「恢复」任何看起来缺失的东西。

## 起因

用户报告 create local app 的验证环节「能力很差、速度很慢、不能和用户互动」。
三条都成立，根因各不相同。

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
- 第一版 render gate 判 `dom_elements_seen === 0`，真实生成的游戏直接走过去（`:94-101`）
- Generate 阶段把 100 轮预算全烧在自我 QA（28 act / 16 inspect / 18 sleep），
  `max_turns_exhausted`，**Verify 一次都没跑**（`:185-191`）

更根本的一条：`verificationMode` 的 `smoke` / `confirmed-targets` / `full-matrix`
**只是三段散文**，schema、gate、工具集完全相同（`local_app_workflow_core.js:262-268`）。
而 `full-matrix` 要求覆盖「iPhone、Android phone、iPad 横竖屏、桌面」——
`apps/engine-mobile/src/lib.rs:196-235` **根本没注册 Browser 工具**，
真机上这段要求是在逼模型编数据。

### 为什么慢

零并行。Design → Generate&Build → Verify → repair → Verify 纯串行，每段一个冷启动
subagent（`max_turns = 100`，`agent/src/builtins.rs:67`）。`balanced` 3 次调用、
`thorough` 最坏 7 次。每次 UI 观测是一次 host↔client 往返（`UI_TIMEOUT = 2min`，
`local_apps_host.rs:42`）。`LocalAppBuild` 是真机 iSH/proot 里的离线 `vite build`，
最坏 6 次。

### 为什么不能互动

不是「不能说话」——`Workflow` 工具 `async_launched` 立刻返回
（`tools/workflow/src/lib.rs:893`），主 turn 在 workflow 开跑那刻就结束了。
问题是**说了进不去那条流水线**：

- workflow VM 的全局只有 `log`/`phase`/`agent`/`parallel`/`pipeline`/`budget`/`args`/
  `workflow`（`workflow/src/lib.rs:29-120`）——**没有 ask / input / pause 原语**
- `AskUserQuestion` 和 `SendUserMessage` 对每个 subagent 平铺禁用
  （`agent/src/tool_resolver.rs:95-113`、`agent/src/builtins.rs:368-376`）
- `ResumeWorkflow` 只带 `task_id`，**没有 payload**（`commands.rs:345-348`）

界面上也没有「指哪儿」的手段：`inspect_ui` 算完 `getBoundingClientRect()`
**只留一个 `visible: bool` 就扔了**（`LocalAppWebView.swift:675`）；`capture_ui`
入参只有 `{app_id}`、不支持裁剪（`local_apps_host.rs:3602-3605` 明确拒绝过）；
`makeUIView` 返回**裸 `WKWebView`**（`:919-930`）；`Sources/LocalApps/` 里
`DragGesture`/`UIPanGestureRecognizer` **零命中**。

## 设计取向

> **流水线只负责把东西造出来并证明它没崩；验收发生在对话里。**

每个 app 本来就是一个会话作用域（`ConversationScope.localApp(id)`，
cwd = `apps/<id>/workspace`），scope 的 cwd、工具绑定、工作区都是真的，
`LocalAppBuild`/`LocalAppRuntime`/`LocalAppInspectUi` 都是普通 builtin、每轮经
`conversation.rs:12085` 的 `all_names()` 喂给模型。缺的只是**入口**和**指哪儿**。

### 已确认的选择（用户）

| 决定 | 选择 |
|---|---|
| 自动 verify 的角色 | 退成便宜的冒烟门；验收交给人 |
| 光标可视化形态 | 静态标注在截图上（非实时动画光标） |
| 反馈提交节奏 | 默认攒清单，单条可「立即修」 |
| 光标回放的位置 | app 界面上的回放条；同时回喂给 LLM |
| 编辑的适用范围 | 库里任何 app、任何时候 |
| 落地平台 | 客户端 UI 仅 iOS；引擎改动两端共用 |
| 进入标注模式 | 悬浮按钮切模式 |
| 界面布局 | 一条三态复用的副驾驶条 |

### 非目标

- Android 的 overlay / 副驾驶条 UI（后补）；agent-facing 的 `inspect_ui`/`capture_ui`
  WebView contract 仍须两端一致
- 修 `SendPrompt.images`（移动端定义了但从不读，`host.rs:6146`）
- 实时动画光标
- 跨 app 引用
- 桌面/Web 客户端

## 架构

```
构建期                                    验收期（对话）
─────────────────────                    ──────────────────────
Design → Generate & Build                 用户玩 app
              │                            │ 点悬浮按钮进标注态
      build_app 成功点 ──► 冒烟门           │ 拖框 → 描述
              │  （宿主，不经模型）          ▼
              ▼                        标注入清单（客户端，落盘）
        workflow 结束                       │ 提交
              │                             ▼
              └──► app 打开 ──────────► StoreAppAnnotation × N
                                            ▼
                              切到 app scope，send(prompt) 并留住 token
                                            ▼
                              agent 改源码 → LocalAppBuild ← 同一道冒烟门
                                   → LocalAppRuntime restart
                                            ▼
                                  热重载 → 回到「用户玩 app」
```

**冒烟门挂在 `build_app` 的可服务成功点上，不是挂在 workflow 里。** 这是本轮最大的
结构改动，理由见下节。右侧一列没有 workflow：它是 app 自己 scope 里的普通对话轮次，
所以「刚创建完就改」与「三个月后再改」走同一条路径。

### 标注（Annotation）

四个部分：**矩形**（CSS 像素，与 `pointer` 同一坐标系）、**描述**、**裁剪图**、
**命中元素**（`elementId`/`role`/`name`/`rect`）。

命中元素让 agent 从「猜」变成「查」：给一张裁剪图它只能猜「你说的那个按钮」，
给 `{elementId:"score-badge", role:"status", rect:[48,100,72,24]}` 它能直接 grep 到源文件。
canvas 应用没有可命中元素，退化为「矩形 + 图 + 描述」，这是可接受的降级。

每条标注有客户端生成的 `annotation_id`（UUID），提交时冻结成 `batch_id`。

## 冒烟门

替换 workflow 里由 agent 自述的 gate。

### 挂载点：`build_app`，不是 workflow VM

`LocalAppsHostBroker::build_app` 有一个定义得很干净的「可服务」成功点：
`served_index.exists()` 检查通过后 `mark_ready`（`local_apps_host.rs:3477-3492`），
注释明写「"Ready" means SERVABLE, not "the build tool exited 0"」。**它与调用来源无关**。

把门挂在那里，一次解决四件事：

1. **修复轮次的构建自动获得同一道门。** 门若只在 workflow 里，那么验收后的每一次
   修复构建（普通对话轮次）都不过任何检查——而**修复构建恰恰是最可能白屏的**：
   初次构建是从确认过的 spec 从零生成，修复是按一句「分数一直不涨」在既有代码上动刀。
2. **修复代理在同一轮就拿到宿主验证过的反馈**（「你改完之后首屏是空白」），
   而不是等用户再报一次。
3. **不需要新的 workflow VM 原语。** 早期版本要加 `localAppSmoke(appId)` 全局——
   那是 `workflow::run_with_progress` 的**签名变更**（7 个定参，`workflow/src/lib.rs:827`），
   而 `tools/workflow/Cargo.toml:14-21` **没有 `tasks` 依赖边**（依赖方向相反），
   所以 trait 放 `tasks` 这个方案不成立；那 ~16 个端到端跑真脚本的契约测试会以
   `ReferenceError` 而非断言差异挂掉，而「加个 `typeof localAppSmoke === 'function'`
   守卫」会让 16 个全绿却从不执行这道门。
4. 没有 journal 重放问题（`Plan::Cached`，`local_workflow.rs:1533-1544`），
   因为根本不经过 agent 通道。

JS 只需要读构建结果里宿主写的 `smoke`，并用持久化的 `last_build_id` 校验它对应本次构建。

### 判据

| # | 判据 | 阻塞？ |
|---|---|---|
| 1 | 运行时已启动且 `preview_url` 非空 | **阻塞** |
| 2 | `inspect_ui` 成功；document 为 interactive/complete、viewport 非零；canvas 应用要求 `canvases` 至少一个且 rect 非零 | **阻塞** |
| 3 | `capture_ui` 取到两帧（间隔 1.5 秒，**均传 `marker:"none"`**） | **阻塞** |
| 4 | 首帧非纯色（整帧像素方差） | **建议** |
| 5 | canvas 应用：`canvases[].rect` 内两帧不同 | **建议** |
| 6 | 运行时错误账本为空 | **阻塞** |

🚨 **判据 4/5 是建议而非阻塞。** 它们是唯一能让一个**能用的 app 交付不出去**的判据，
而四轮评审里五个独立镜头各自找到了**不同的**误阻塞路径：菜单态 canvas（门不驱动输入，
`local_app_canvas_workflow.js:240` 的「with an input driven between them」这个豁免条件
门根本表达不了）、逐帧 JPEG 质量档位变化被读成全局运动（`LocalAppWebView.swift:510,513-518`
的阶梯）、Android reload 后的 `about:blank` 白帧、WebContent 进程死亡。
两个阈值本来也没有标定数据。降为建议同时**删掉了 engine-mobile 新增 `image` 依赖的需要**。

🚨 **判据 6 必须捕获 `console.error`，否则它在每个模板应用上恒为盲。**
两套脚手架都把树包在 React `ErrorBoundary` 里，而
`local-apps/templates/vite-react-static-v1/app/error-boundary.jsx` **只有
`getDerivedStateFromError`，没有 `componentDidCatch`、没有 `reportError`**；
React 19（`package.json:17` `"react": "19.2.8"`）的默认 `onCaughtError` 路由到
`console.error`，**永远不到 `window.onerror`/`unhandledrejection`**。
于是一个渲染崩溃的 app 会渲染出「出现了意外错误」兜底页——**有 DOM（过判据 2）、
有配色文字（过判据 4）、无未捕获异常（过判据 6）——全绿出厂。**

修法：把 `console.error` 收进同一个有界账本，标 `kind:"console"`。
⚠️ **反向测试的 fixture 必须是「带一个会抛异常的屏幕的、真实出厂的模板」**，
它必须变红。`error-boundary.jsx` **不在 `HOST_MANAGED_FILES`**
（`local_apps_build.rs:239`），所以只改模板是不可强制的。

### 失败分类与交付

- **任何**来自门发出的 UI 请求的 `Err` ⇒ `infrastructure_unavailable`。
  source defect 只从**成功的载荷**里判。这条策略取代了早期版本的
  `ResolveAppUiRequest.error_code`（已删，理由见协议一节）。
- `inspect_ui` 载荷被降级（`truncated` 含 `runtimeErrors`）⇒ 同样
  `infrastructure_unavailable`，**不判通过**——观测不到不等于没异常。
- 无阻塞 finding ⇒ 构建成功。阻塞判据不过 ⇒ **一次**修复轮；再不过 ⇒
  带诊断交付 `delivery_status: "needs_user_review"`。

🚨 **`needs_user_review` 必须有用户可见信号，否则这次改动是净负。**
今天冒烟受阻的构建会 `throw`（`local_app_workflow_core.js:355`）⇒ 任务 `failed`
⇒ iOS 渲染 `chat_task_failed`（`ConversationSource.swift:3953`）。换成
`needs_user_review` 会渲染 `chat_task_completed`（`:3952`）——**与健康构建逐字节相同**，
而模型也看不到（`local_workflow` 落到通用通知臂，`<result>` 段只有 `local_agent` 有，
`prompt/task_notification.rs:232-246` vs `:133-135`）。
必须同时加一个 app 详情页的可疑标记 + 一条把 `smoke_report` 摘要带进任务通知的路径。

### 门的请求会把 app 推上屏——这是对的

controller 只在 `LocalAppPreviewView` 真的挂了 `LocalAppWebView` 之后才注册，
而它只在 `previewURL != nil` 且视图在屏上时挂载（`LocalAppDetailView.swift:586`）。
把 app 推上屏的正是 `requestedPresentationAppID` → `RootView.swift:309-313`。
**抑制它 = 让门在主流程下 100% 失效**（早期版本这么写过，已撤回）。

在本设计的两条流程里这都是预期行为：用户要么刚要求创建这个 app，要么刚提交标注要求
修它——**都在等这个 app**。

⚠️ 但 `AppUiRequestDto` **没有 origin 字段**（`local_apps.rs:767-775`），所以副驾驶条
无法区分「agent 在操作」和「宿主的门在探测」，门自己的 3–6 次探测会显示成
「我正在操作你的 app」。落地时二选一：给 `AppUiRequestDto` 追加一个可选 `origin`
字段（进本设计唯一那次 bless），或副驾驶条只在有 agent turn 在飞时显示。
**倾向后者**（不加协议字段）。

## 协议改动：只有一处

`ResolveAppUiRequest.result_json` 和 `AppUiRequestDto.value` 都是不透明
`Option<String>`（`commands.rs:438`、`local_apps.rs:774`），所以绝大部分能力不触及协议。

| 能力 | 承载 | 动协议 |
|---|---|---|
| `inspect_ui` 元素几何 / canvas rect / runtimeErrors | `result_json` 内 JSON | 否 |
| `capture_ui` 区域裁剪 + marker | host schema 序列化到 `value` JSON | 否 |
| 冒烟门报告 | 宿主写盘 + 构建结果 | 否 |
| 标注 → 消息 | 复用 `SendPrompt` | 否 |
| 清单清理触发 | 复用既有 `AppRecordChanged` | 否 |
| **存标注** | 新命令 + `AppEventDto` 回执 | **是（唯一）** |

### 被删掉的三处协议改动，以及为什么

- ⛔ **`ResolveAppUiRequest.error_code`**：没有任何按它分支的消费者
  （`local_apps_host.rs:2113-2141` 五个失败返回没有一个是 source defect），
  且**在 Android 上不可实现**——spec 点名的失败点在那边不存在
  （`LocalAppWebView.kt:192` 是离屏截图错误），而 `local_apps_error_ui_not_open`
  是个**死资源**：五个语种的 catalog 都有、**Kotlin 零引用**。
  替换成上面那条一行的门策略。**这一删让阶段 1 完全不碰协议。**
- ⛔ **`AppWorkflowTaskChanged` / `AppWorkflowTasksSnapshot` / `AppWorkflowTaskDto`
  + `task_runs` 加宽 + 客户端 active-set 状态机**：整族删掉。它防的是「构建中提交标注」，
  但 **`TaskRegistry` 是每个引擎一个**（`host.rs:3383`，在 `build_mobile_inner_with_ask`
  `:2329` 里），而提交路径每次切 scope 都新建引擎（`RootView.swift:915/928`）⇒
  新 registry 是空的，**跟会话过滤无关，修不好**。而 build-vs-build 本来就被进程互斥锁 +
  per-app 文件锁排除了（`local_apps_build.rs:626,634`；`storage.rs:185-216`）。
  提交直接发，让锁去串行化。
- ⛔ **`AppEventDto::AppBuildSucceeded` + `AppDetailsDto` 单独的 build id**：
  `AppRecordChanged` 已经是 blessed 事件且客户端已在处理（`LocalAppsStore.swift:911`）；
  `pin_init_session` 就是从 `with_app` 里发 `AppEvent::RecordChanged` 的现成先例
  （`service.rs:707-712` → `local_apps_bridge.rs:216-220`）。清理改成监听
  `AppRecordChanged` 且 `record.lastBuildId != batch.baselineBuildId`。

### `StoreAppAnnotation`

```rust
ClientCommand::StoreAppAnnotation {
    app_id: String,
    annotation_id: String,   // 客户端 UUID，贯穿清单/batch/回执
    rect: String,            // "x,y,w,h"，CSS 像素
    viewport: String,        // JSON: width/height/offset/scale
    hit_elements_json: String,
    note: String,
    image_base64: String,
}
// → AppEventDto::AppAnnotationStored { app_id, annotation_id, path: Option<String>, error: Option<String> }
```

⛔ 早期版本还有一个 `request_id`。删掉：它的论据（并发提交会乱序回执）**未经代码证实**
——`LocalAppsStore` 是 `@MainActor`、`send` 也是 MainActor 隔离，自然写法是顺序的；
`annotation_id` 本来就唯一、就是药丸的 key，一个 correlator 够了。

⛔ 早期版本还有一个 8 秒「无回执」计时器。删掉：那是为 `#[non_exhaustive]` 兜底臂
（`host.rs:7064-7074` 静默 `Ok(())`）定制的单命令活性协议，而那个陷阱对全部 40+ 命令
一视同仁。本仓库既有的答案是**兜底臂加 `debug_assert!` + 两半一起发版**。

放在 `AppEventDto` 而非新增顶层 `ClientEvent`：它就是 app 生命周期事件的既有信封
（`local_apps.rs:835-836`），客户端已有单一 `case let .appEvent(event)` 分发点
（`LocalAppsStore.swift:267`）。**不要用「uniffi 元数据快到上限」当理由——实测
`AppEventDto` 15.8%、`ClientEvent` 28.7%，不成立。**

### 落盘位置：`apps/<id>/annotations/`，不在 workspace 里

🚨 **不能写进 `workspace/.lingxi/`**，两个独立原因：

1. **`.lingxi` 是构建键的输入。** `collect_workspace_inputs` 的跳过表只有
   `.git | .lingxi-build-state | node_modules | dist`（`local_apps_build.rs:766-769`）。
   往 `workspace/.lingxi/` 里写任何可变内容 ⇒ **这个 app 的构建缓存永远不再命中**
   ⇒ 每轮修复都是一次真机上的完整离线 `vite build`。本设计的出发点之一是「慢」。
   （模块自己那份包含 `.lingxi` 的「非构建输入」清单 `:1470-1481` 只有 `#[cfg(test)]`
   到得了。）
2. **checkpoint restore 会把整个 workspace 读进内存两次、把未跟踪文件删掉再重写**
   （`checkpoints.rs:210-214, 297-360`）。restore 期间存下的标注会被 ack 一个**已经不存在
   的路径**。

所以标注落在 `apps/<id>/annotations/`（workspace 之外，`storage.rs` 的 app 目录下），
引擎回工作区**之外**的相对路径。同时**仍要把 `".lingxi"` 加进 `:766-769` 的跳过表**——
`spec.md` 和将来任何写进去的东西都会踩同一个坑，且这是独立的在线缺陷
（`restore_host_managed_files` 在 `:637-639` 于键计算 `:642` **之前**重新钉住
`.lingxi/source-policy.json`，所以它的字节是常量，加进跳过表是安全的）。

🚨 **`engine-mobile` 必须开 `image-read`。** 现在是
`tool-file = { path = "../../tools/file" }` **裸依赖**（`Cargo.toml:216`），
而 `image-read` 默认关闭、注释明写「engine-mobile/minimal stay lean and never compile
the image codecs」（`tools/file/Cargo.toml:47-49`）；engine-desktop 开了（`:121`）。
不开的话 `Read` 打开标注 JPEG 命中 NUL 扫描返回 `format_binary`，**不是图片**——
「把你框的那块给 agent 看」这个核心动作直接不工作。
**这条落地前，整个裁剪管线不要发版。**

## 引擎改动

### `inspect_ui`：几何 + canvas rect + 运行时错误账本

`LocalAppWebView.swift:675` 与 `LocalAppWebView.kt:636-645` 已在算
`getBoundingClientRect()`，只留了 `visible`。改为同时输出 `rect: [x,y,w,h]`（CSS 像素取整）。
`canvasCount` 扩成 `canvases: [{rect}]`（保留 `canvasCount` 兼容字段），
增加 `documentState`、`viewport`（含 `offsetLeft/offsetTop/scale`）与 `runtimeErrors`。

两端在 document-start bootstrap 安装 `error`、`unhandledrejection` **和 `console.error`**
监听（见判据 6），有界记录 `{message, source, line, column, at_ms, kind}`。
页面 reload 时清空；同一 document 内重复 inspect 不清空。

#### 256 KiB 是硬失败，新增字段必须预算住

`LocalAppWebView.swift:407-411` 在 `resultJSON.utf8.count > 256 * 1_024` 时返回
`.failure(local_apps_error_ui_invalid_result)`——**不截断，直接失败**。
既有最坏情况已不宽裕（`clean()` 每串 500 字符 `:579`，200 个元素各带四个字符串 `:674-686`）。

| 段 | 上限 |
|---|---|
| `runtimeErrors` | 8 条，`message` 200 字符、`source` 120 字符，超出只留 `runtimeErrorsDropped` |
| `canvases` | 16 条；超出只留 `canvasCount` |
| `elements` | 200 条不变；新增 `rect` 是 4 个整数 |

组装后若仍超 200 KiB，按 `elements` → `canvases` → `runtimeErrors` 逐段降级并写
`truncated: [<段名>]`。

### `capture_ui`：区域裁剪 + marker

对模型暴露的 schema additive 增加：

```json
{ "app_id": "…", "rect": {"x":10,"y":20,"width":120,"height":80}, "marker": "last_action" }
```

host 校验有限数字/正尺寸后序列化进既有 `AppUiRequestDto.value`，**DTO 不变**。

🚨 **裁剪的长边上限必须自己算，两端都错在相反方向：**

- iOS：既有 `snapshotWidth` 是从**整个 view 的 bounds** 推的
  （`LocalAppWebView.swift:482-488`），直接复用会把裁剪区**放大**。而早期修正写的
  `snapshotWidth = min(rect.width, cap)` **只钳了宽**：一个 120×800 pt 的竖条裁剪
  → 360×2400 px，超出声称的长边上限 2.34 倍，然后被压到 q0.3 或直接拒绝。
  正确写法：`scale = min(1, capPoints / max(rect.width, rect.height));
  snapshotWidth = rect.width * scale`。
- Android：**不能**「先 PixelCopy 整帧再裁」——那是从一个已经被 1024 钳过的帧里取样，
  约 2.3× 分辨率损失。要把 CSS→window 的 rect 作为 `PixelCopy` 的**源** rect，
  目标尺寸按裁剪区定，钳裁剪区，不放大。
  ⚠️ **`× density` 这一步必须在文档里写出来**——本仓库已经栽过一次
  （`LocalAppWebView.kt:344-353`）。

### 光标标记（用户需求 #3）

客户端在 `captureFrame` 里画标记圈再编码：iOS `UIGraphicsImageRenderer`，
Android 在 PixelCopy/fallback bitmap 上用平台 `Canvas`。
**仓库里没有任何服务端图像绘制库**（`imageproc`/`tiny-skia`/`resvg`/`ab_glyph` 都不是
依赖），所以必须在客户端画。

标记位置来自紧邻的上一次带坐标动作（客户端记 `{kind,x,y,at}`），仅当 5 秒内才画。
`result_json` 加 `last_action: {kind,x,y,age_ms}`。

🚨 **任何程序化像素比对都必须传 `marker:"none"`**：门自己不发坐标动作，5 秒过期规则会让
两帧一有标记一无，标记若落在 canvas 矩形内会让**冻结的 canvas 假过**判据 5。
标记是给人和给模型看的证据，不是给比较器看的数据。

### workflow 脚本删减（同一个 commit 里必须一起做）

- 删 model Verify 阶段；保留一份只由宿主 smoke finding 触发的 repair prompt。
  agent 调用数从 3–7 降到 **2**（Design + Generate&Build），DOM `fast` 降到 **1**；
  触发修复各 +1。
- 删 `STRATEGY_POLICY.verificationMode` 与三段散文。`runDesign` 保留
  （`local-canvas-build` 拒绝 `fast` 的理由仍成立）。`maxRepairRounds` 保留，
  三个 strategy 归一为 1。
- 删 `args.revision_prompt`（唯一写入者是测试辅助 `builtins.rs:1059-1078`）。
- 终态结果固定为 `{ok, strategy, complexity, agent_calls, preview_url, repair_rounds,
  delivery_status, smoke_report, summary}`；`delivery_status ∈
  {ready, needs_user_review, verification_unavailable}`。
- 标记 `SHAPE.verifyPrompt`、`SHAPE.extraBlockingFindings`、`VERIFICATION_RESULT_SCHEMA` 死。

🚨 **第四次「删掉承重副作用」——必须同时改的四处：**

1. **两个 shape 文件给 Generate 代理的那句话。**
   `local_app_build_workflow.js:191` 和 `local_app_canvas_workflow.js:225` 写着
   *"STOP once the build succeeds… **A separate Verify stage does all of that afterwards
   with a budget of its own.**"* ——它正上方的注释（`:185-190`）记录了它为什么承重：
   *那次 100 轮预算被自我 QA 烧光、`max_turns_exhausted`、Verify 一次没跑*。
   **本文把那条注释当作删 Verify 的头号证据引用，却差点把指令留在原地。**
   删完之后：这句话的理由变成假的、它禁止的四个工具仍无条件注册
   （`local_apps_tools.rs:66-79` → `host.rs:3734`）、`maxTurns` 仍是 100，
   而下游已没有会被饿死的 Verify ⇒ 预算烧光直接撞 `requirePreviewOnSuccess`
   （`local_app_workflow_core.js:167`，调用于 `:254`）抛异常，**整个构建失败**。
   改写为：*「宿主会在本阶段之后自动跑一次冒烟检查；你自己走 UI 不属于它、也不算数。」*
2. **`throw` → `needs_user_review` 会删掉唯一的用户可见信号**（见冒烟门一节）。
3. **CI 门**：`scripts/mobile-linux/verify-local-app-supply-chain.py:995` 对拼装后的脚本做
   子串检查 `"verification_mode"`（`"verificationMode"` **匹配不上**），
   `:1018-1019` `fail()`；canvas token `:1029-1035` 钉着
   `"only a difference proves the loop is running"`，只存在于将死的 `verifyPrompt` 里。
   CI 入口 `.github/workflows/clients.yml:139` → `verify-mobile-linux-pins.sh:67`。
   **必须换成 `delivery_status`/`smoke_report` 锚点。**
4. **`builtins.rs` 的合约测试**：`local_app_build_is_model_invocable_and_pins_its_contract`
   （`:125`）断言 `contains("title: 'Verify'")`（`:133-138`）、三个 phase 按序（`:139-148`）、
   锚点 `"verificationMode"`（`:159`）与 `"verification_mode"`（`:163`）；canvas 孪生 `:1100`。
   ⚠️ 修它们时**不要顺手删掉 phase 顺序断言**——那是剩下两个 phase 唯一的顺序守卫。
5. **`SKILL.md`** 是编译进二进制的模型可见合约，仍在描述已删除的管线（`:308-322`），
   且它的过时是**静默的**。

## iOS 客户端

### 图层

`makeUIView` 目前返回裸 `WKWebView`（`LocalAppWebView.swift:919-930`）。改为返回容器
`UIView`，`WKWebView` 铺满，上叠透明的 `LocalAppAnnotationOverlayView`；非标注态
`isUserInteractionEnabled = false`，触摸原样落到 WebView。

不用页面内注入的 overlay：CSP 允许，但注入元素会进 app 自己的 DOM、被 `deepQuery` 扫到，
污染验证快照。

### 标注模式

- 悬浮按钮切换；进入时 overlay 接管交互、整层压暗、顶部蓝色模式条。
- `UIPanGestureRecognizer` 拉矩形，四角手柄可调，最小 24×24 点。
- 松手 → 副驾驶条变输入框，不弹模态。
- **不能假设点坐标恒等于 CSS 像素**：模板只设 `initial-scale=1`，没禁用 pinch zoom。
  松手时从同一 document 读 `visualViewport {width,height,offsetLeft,offsetTop,scale}` 换算。

🚨 **`makeAnnotation` 的三步必须串行，而 `LocalAppWebViewController` 现在没有任何串行原语**
（`LocalAppWebView.swift:331-332` 类里无锁无队列），每个 UI 请求各跑一个 detached `Task`
（`LocalAppsStore.swift:1095-1109`）。后果：`rect` 算在布局 A、`hitElements` 算在布局 B、
用户看到的裁剪是布局 C。**今天用一轮里两个并行 `act_on_ui` 就能复现。**
修法：controller 上一个 `private var inFlight: Task<Void, Never>?` 链，
`execute` 和 `makeAnnotation` 都走它。

### 提交路由

RootView 的 `onSubmitAnnotations(appID, batchID, prompt)` 是唯一入口。

1. 已在同一 `.localApp(appID)` scope：直接在当前 source 发。
2. 在别的 scope 且正 streaming：**只排队，不调 `switchScope`**——`switchScope` 会先
   `previousSource.cancelAndWait()`（`RootView.swift:909`），直接切会杀掉无关的 turn。
3. 目标 session 优先该 app 最近活跃 session。⚠️ **`restoredSessionID` 返回 `String` 不是
   `String?`**，miss 时返回 `""`（`RootView.swift:1030-1039`），判空要用 `.isEmpty`；
   而且它**抹掉了底层刻意保留的区分**——`ProjectScopedPreferences.storedActiveSessionID`
   返回 `String?` 正是为了区分「没有 key」和「用户选了新会话所以存了空 id」
   （`ProjectScopedPreferences.swift:28-32`）。必须读那个可选值。
4. 🚨 **不能用 `switchScope(initialPrompt:)`**：它只在转录为空时才发
   （`RootView.swift:978` 的 `if source.model.items.isEmpty`，而 `sessionResumed` 在同一个
   同步 handler 里就填好了 `items`，`ConversationSource.swift:4401/4418`）——
   而规则 3 选的恰恰是几乎必然非空的会话。改为切完 scope 自己 `send(prompt)`。
5. 🚨 **必须留住 `send` 的返回值。** 它返回 `ConversationTurnToken?`，五个守卫下返回 `nil`
   （`ConversationSource.swift:2488-2494`），而 `RootView.swift:979`/`:1008` 用 `_ =` 丢了——
   丢掉的正是 `TurnStarted` 要回显的 correlator。
   ⚠️ 但 **`sessionTransitionPending` 要排除在「nil ⇒ 失败」之外**：`switchScope` 同步置位
   它（`RootView.swift:928,937,951,953`），那是规则 4 自己的成功路径。
6. 🚨 **`ConversationTurnCompletion` 到不了 `LocalAppsStore`。** 它是 `@Published`
   （`ConversationSource.swift:397`，发布于 `:1996-2006`），唯一消费者是
   `ChatView.swift:225`；而 `LocalAppsStore` 只接了 `subscribe { handle(event:) }`
   （`RootView.swift:811`、`LocalAppsStore.swift:209`）。需要在 **RootView** 加一个显式
   钩子（挨着 `:300` 那个 `.onReceive(source.model.$streaming)`）——**不能放在详情/预览视图里**，
   它们会被提交路径自己的关 cover 动作卸载。

### 副驾驶条（用户需求 #3）与引导

三态：agent 操作中显示带标记的操作帧；刚框完变输入框；有待提交标注时变药丸清单。

⚠️ **本节与引导一节（coach mark）未经任何对抗性评审覆盖** —— 第四轮的七个镜头没有一个
读过它们。它们是用户明确要求的功能（需求 #3），所以保留在设计里，但**落地前需要单独一轮
评审**，且排在最后一个阶段。已知的一个问题见「门的请求会把 app 推上屏」一节末尾
（`AppUiRequest` 没有 origin 字段）。

引导复用 `AppState` 上的 `didSet + defaults` 形状——**不是 `@AppStorage`**
（全树零命中；`AppState` 是 `@Observable @MainActor final class`，`Theme.swift:25-27,48`，
`@AppStorage` 是 SwiftUI `DynamicProperty`，用不上去）。
⚠️ `Theme.swift:60-70` 在 `LINGXI_UI_TESTING == "1"` 时强制 `setupDone = true`，
新标志需要同形状的 override，否则 UI 测试跑的是「引导已跳过」的路径。
文案写进 `clients/translations/*.json`（真源），不是 `Localizable.xcstrings`（生成产物）。

### 标注清单的持久化与状态机

清单必须落盘：进程在 `StoreAppAnnotation` 成功后终止会丢掉清单却留下孤儿文件。
照抄 `LocalAppWebsiteDataStoreRegistry`（`LocalAppWebView.swift:1302` 起）——带版本的
`Codable` 日志 + 与权威状态对账（`:1358`）。重启时与 `apps/<id>/annotations/` 的实际文件
对账：盘上有清单里没有的删掉，清单里有盘上没有的降回 `draft(error)`。

```
draft ──Store 成功──► stored ──TurnStarted──► submitted ──output_digest 变化──► cleared
  │                      │                        │
  └──Store 失败──►draft(error)                     └──turn cancelled/failed──► stored(可重试)
```

🚨 **清理必须比 `build_id`。** `build_cache_hit` 在写任何 provenance 之前就
`return Ok(())`（`local_apps_build.rs:642-644`），而 `build_app` 仍视其为成功——
所以 agent 为了复现问题先跑一次 `LocalAppBuild`（无改动）就会清掉你整批标注。
判据用 **`output_digest` 变化**（`:659,661`，只在 miss 路径产生），`build_id` 只作 correlator。

### `build_id`：持久化 UUID，落在 `AppRecord`

三条显而易见的替代方案都不行：

- **不能放 build provenance**：`build.json` 在 `build/store` 里（`local_apps_build.rs:865-867`），
  而 `promote_build_root` **把整个目录 rename 走**再换新树（`:1136-1140`）⇒ 每次 promote
  先销毁再于下一条语句重写（`:663`→`:664`），中间有「活的可服务产物没有 id」的窗口；
  且 `write_build_provenance` 只有 temp+rename、**无 fsync**。
- **不能用 `buildKey`/`outputSha256` 当 id**：内容派生，相同源码重建后不变。
- 🚨 **绝不能用进程内计数器**：`RootView.swift:915/928` 每次切 scope 都新建引擎，
  而提交路径自己就会切 ⇒ 新引擎重发 `build-1`，基线永远相等，清理一次都不触发。

落地：`AppRecord` 加
`#[serde(default, skip_serializing_if = "Option::is_none")] pub last_build_id: Option<String>`
（`local-apps/src/types.rs:142`），**照抄 `init_session_id` 的 additive 形状**（`:170-176`）
——`skip_serializing_if` 是硬要求，否则 `AppManifest::hash()` 一变，既有应用立刻
`database manifest mismatch`。在 `AppService` 里加走同一 `with_app` 闭包的 mutator，
**不能像 `mark_ready` 那样在「没变化」时提前返回**（`service.rs:721-723`），每次铸新 UUID，
并从 `with_app` 里发 `AppEvent::RecordChanged`（`pin_init_session` 就是这个先例，`:707-712`）。
调用点在 `build_app` 的 `served_index.exists()` 检查之后（`local_apps_host.rs:3477-3492`）。

## 提交时组装的消息

```
我在运行「贪吃蛇」时标了 3 个问题。视口 393×852 CSS 像素。

1. 分数一直不涨
   区域 x=44 y=96 w=118 h=74
   截图 ../annotations/1787543300-1.jpg
   区域内元素:
     #score-badge  role=status  name="12 分"  rect=[48,100,72,24]

2. 按钮太小
   ...

请逐条修复，然后 LocalAppBuild + LocalAppRuntime restart。
改完告诉我改了什么，我再看一遍。
```

这段格式是与 agent 的实际契约，应有快照测试。

## 边界与错误处理

| 场景 | 行为 |
|---|---|
| 构建仍在跑时提交标注 | **直接提交**。build-vs-build 已被进程互斥锁 + per-app 文件锁排除（`local_apps_build.rs:626,634`），不再建客户端排队机制 |
| `StoreAppAnnotation` 失败 | 按回显 `annotation_id` 把该条留草稿标红；其余照常提交，正文只列成功落盘的 |
| 裁剪图超尺寸 | 引擎按质量阶梯降质；仍超则拒绝该条并回 `error` |
| 矩形内无命中元素 | 正常提交，元素段写「（无 DOM 元素，canvas 区域）」 |
| `capture_ui` 区域越界 | 持有真实 viewport 的客户端钳位；完全在视口外回错误，不静默改整帧 |
| 提交时 `send` 返回 `nil` | batch 回 `stored(error，可重试)`；**`sessionTransitionPending` 除外**（那是成功路径） |
| 修复轮 turn 被取消/失败 | 经 RootView 的显式钩子把 batch 退回 `stored(可重试)` |
| 门开火时 WebView 未挂载 | 经既有路由触发 presentation；45 秒整门 deadline 内仍未挂载则 `infrastructure_unavailable` |
| 门的 UI 请求返回任何 `Err` | `infrastructure_unavailable`，不判 source defect |
| `inspect_ui` 载荷被降级 | 判据 6 判 `infrastructure_unavailable`，不判通过 |
| 用户在 agent 操作中途进标注模式 | 允许；overlay 接管触摸不影响 `act_on_ui`（注入 JS 合成事件，不经 UIKit 触摸链） |

**并发写入风险（明确记录，未消除）**：另一条会话仍可能在同一时刻对同一工作区发起编辑
轮次。`WorkspacePermissionLeaseRegistry` 是**授权**而非**互斥**机制
（`permission/src/workspace_lease.rs:60-83`）。若实测出现冲突再引入 app 级编辑互斥。

## 测试策略

### 引擎 / Rust

- 冒烟门六条判据各有独立测试，**每条都要有反向用例**。
  🚨 **判据 6 的反向 fixture 必须是「真实出厂的模板 + 一个会抛异常的屏幕」**，
  它必须变红——用手写的 `throw` 页面测不出 `ErrorBoundary` 那条路径。
- 门只能由 `build_app` 的成功点触发；JS 无法注入一个 `status:"passed"` 覆盖宿主报告。
- WebView 未挂载/timeout 返回 `verification_unavailable` 且 `agent_calls` 不变，
  确认 `pending_ui`（`local_apps_host.rs:2115-2119`）被清理。
- `StoreAppAnnotation` 路径推导测试钉住**真实设备路径形状**（`apps/<id>/annotations/`），
  而非重复实现里的推导；另测非法 base64/JSON/rect、尺寸上限、原子写。
- **构建键回归测试**：往 `workspace/.lingxi/<任意>` 写内容**不得**改变 `workspace_build_key`。
- **`Read` 一个工作区外的 `.jpg` 必须返回 image 结果**（守住 `image-read` feature）。
- 协议：command + event goldens、contract index、TS union/guards、`clients/shared` 完整
  `npm test`；测试明确断言 `AppAnnotationStored` 位于 `AppEventDto`。
  ⚠️ **契约索引是手写的**，漏条目时 version guard 静默放行——新字段必须手动加 `put(...)`。
- workflow 脚本：阶段序列与 agent 计数断言更新；**同时更新
  `verify-local-app-supply-chain.py` 的 token 清单**，否则 CI 每个 PR 红。
- 跑测试**必须捕获完整输出到文件再 grep**（只 grep `FAILED` 会丢 `failures:` 块里的测试名），
  用 `--no-fail-fast`；**测试总数下降即使 0 failures 也是红旗**。

### iOS

- 注入 JS 的几何输出：扩展 `LocalAppsStoreTests` 既有的 shadow-DOM walk 测试
  （`executionSource` 是 internal 正为此）；覆盖 element/canvas rect、runtime error ring 的
  8 条上限与 reload 清空、**`console.error` 被收进账本**。
- 坐标换算至少覆盖 1x、pinch zoom、滚动 offset、旋转后 viewport 四条。
- **裁剪尺寸**：竖长条裁剪（如 120×800 pt）不得超过长边上限，不得放大。
- prompt 路由：从 global/project/另一个 app scope 提交都进目标 workspace；
  已在同 scope 时保留当前 session；无关 source 正 streaming 时不 `cancelAndWait`；
  `sessionTransitionPending` 不被误判成失败；turn cancelled/failed 让 batch 可重试。
- **controller 串行**：慢 `execute` 与并发 `makeAnnotation` 观察到同一个 document generation。
- 清单持久化：杀进程后重启，清单与 `annotations/` 目录对账正确。
- 清理：两次无改动的连续 build ⇒ `build_id` 不同、`output_digest` 相同、**batch 存活**。
- ⚠️ WebView 与预览路由**当前没有 `accessibilityIdentifier`**，overlay 需要自己的 id；
  **SwiftUI 容器上的 `accessibilityIdentifier` 会覆盖所有子元素的 id**，
  需 `.accessibilityElement(children: .contain)`。UI 测试要 `-testLanguage zh-Hans`。
- 引擎是预编译 xcframework，**`xcodebuild` 不重编 Rust**：判据是
  `strings -a …/libios_framework.a | grep <新字符串>`，装机后验 `LingxiCode.debug.dylib`
  （主二进制只有 91 KB，grep 它得 0 = 假阴性）；重建需 `LINGXI_REUSE_STAGED_LINUX_RUNTIME=1`。

### 真机

冒烟门必须上机验证，不接受单测绿即交付：重建视口/几何的判据历史上必然逃过单测。
DOM 应用与 canvas 应用各走一遍完整流程（框选、提交、修复、热重载）。
Android 虽不落 overlay，阶段 1 的共享 UI tool contract 仍需设备或 instrumentation 覆盖；
若该轮上不了 Android 设备，**必须记为阶段 1 未完成**，不能用「Android UI 非目标」把
agent-facing contract 判绿。

## 分阶段落地

| 阶段 | 内容 | 依赖 |
|---|---|---|
| 1 | 两端 `inspect_ui` 几何/canvas rect/runtimeErrors（含 `console.error` 与载荷预算）+ `capture_ui` 区域（修正后的裁剪数学）+ 光标标记 + `image-read` + `.lingxi` 进构建键跳过表 + 两处 `local-app-build` 字面量改集合判定 | 无。**完全不碰协议** |
| 2 | 冒烟门挂 `build_app` + 判据 1/2/6 阻塞、4/5 建议 + workflow 脚本删减（含那五处同 commit 必改）+ `needs_user_review` 的可见信号 | 1 |
| 3 | `StoreAppAnnotation` + `AppRecord.last_build_id` + 唯一一次 bless | 1 |
| 4 | iOS overlay + controller 串行 + 标注状态机与持久化 + 提交路由 | 1 与 3 |
| 5 | 副驾驶条 + 引导（**先单独评审**，见该节警告） | 4 |

阶段 1 与 3 可并行。阶段 1 是 2 的硬前置（判据依赖 1 新增的 rect 与 runtimeErrors）。

## 不属于本设计的在线缺陷（建议单独开条目）

四轮评审顺带确认的、与本功能无关的现存 bug：

1. **`.lingxi` 不在构建键跳过表**（`local_apps_build.rs:766-769`）——任何写入都让缓存永不命中。
2. **`Paused` 的收养检查点让 app 永久不可删**：`registry.rs:903` 把收养的检查点登记为
   `TaskStatus::Paused`，而 `:794` 的判据是 `!is_terminal()`，`Paused` 不是终态 ⇒
   删除守卫（`host.rs:5938-5956`）永远认为有活的构建。改为
   `matches!(status, Running | Queued)`。
3. **canvas 构建既没有 workspace lease、也不被删除守卫保护**：
   `local_workflow.rs:1987` 与 `registry.rs:796` 都硬编码 `"local-app-build"`
   ⇒ canvas 应用可以在构建进行中被删掉。
4. **`LINGXI.md` 在移动端从未加载**：`session_cwd` 持 guest 路径
   （`host.rs:3291-3293, 3329`），`build_system_prompt` 走了 `prompt_probe_cwd_resolver`
   做 guest→host（`conversation.rs:12059-12075`）但**那条路径已不再渲染**
   （`prompt/mod.rs:150-153`），唯一渲染的 `additional_context_message`（`:12270`）
   读的是未转换的 guest 路径 ⇒ 加载零个文件、静默无错。
   ⚠️ **它不阻塞本设计**（工具名每轮经 `all_names()` 喂给模型，标注消息正文自带指令），
   但它是平台级缺陷：修复会给**整个移动平台**同时打开 memory 加载（嵌套 `@import` 展开、
   外部包含门、read-state 播种、每条首用户消息的新 token），所以应独立提交、独立浸泡。

## 已知约束

1. **协议**：`7.0.0`，blessed major 7；additive 也需重新 bless。移动端**无握手**，
   偏斜不可检测，只有 append-only 纪律在保护。
2. **uniffi**：变体追加到末尾（序数密集且位置相关）；不给无字段枚举加带数据变体
   （会让 Kotlin 生成 `sealed class` 并重命名所有常量）。`AppEventDto` 的 docstring 用
   `//` 而非 `///`。
3. **生成绑定两端都是 gitignored 构建产物**（Android `clients/android/.gitignore:7`；
   iOS `clients/ios/.gitignore:6-7`）。改 wire DTO 后必须先重新生成再声称客户端编得过；
   `cargo test` 全绿对两个客户端零信息量。iOS：
   `LINGXI_REUSE_STAGED_LINUX_RUNTIME=1 clients/ios/scripts/build-xcframework.sh`；
   Android：`clients/android/scripts/build-jni.sh`（或只生成绑定的两条命令，
   **stock `uniffi-bindgen` 会 panic，要用 `ios-framework --features cli` 那个打过补丁的 bin**）。
   gradle 任务带 flavor：`:app:compilePlayDebugKotlin`。
   ⚠️ 陈旧绑定**只表现为编译失败，没有任何测试或 CI 会提前抓到**。
4. **CSP**：单一无条件常量（`local_apps_host.rs:100`），三处同源。
   ⛔ 「wasm 与 Worker 被拦死」**已作废**——`'wasm-unsafe-eval'` 与 `worker-src 'self' blob:`
   现在是 **DEFAULTS, not a grant**（`:80-81`），2026-08-21 的实测是**改策略之前**的事
   （`:96-99` 作为历史保留，只 grep 到 "both blocks were real" 会读成现状）。
5. **Ionic**：`iife` 下无法按需引入，~1.4 MB 固定底盘；重度 Shadow DOM，两端 `deepQuery`
   已穿透，几何输出必须走同一条 walk。
6. **设备**：真机 rootfs 无 Node 工具链；`AppManifest::hash()` 序列化整个结构体且绑定
   SQLite schema ⇒ 新字段必须 `skip_serializing_if`。
7. **`ClientCommand` 是 `#[non_exhaustive]` 且分发有兜底臂**（`host.rs:7064-7074` 静默
   `Ok(())`）⇒ 新命令加了也编译通过但被静默忽略，必须显式加处理臂。
   本仓库的答案是兜底臂加 `debug_assert!` + 两半一起发版，不是给单个命令定制活性协议。
8. **allow-by-default 名单有两份手写副本**（`permission/src/defaults_per_tool.rs` 与
   `local_apps_tools.rs:674`）。
9. **契约索引 `current_contract_index()` 是手写的** ⇒ 不加条目版本守卫静默放行。
10. **构建环境**：cargo target 曾涨到 84G 撑爆磁盘（`errno 28`，表现为随机
    `could not compile`）。`rm -rf target/debug/incremental` 安全回收。
11. 🚨 **任何「顺带删掉 X」的条目，落地前必须回答「谁依赖 X 的副作用」，而不只是
    「X 做了什么」。** 本设计**四次**栽在同一形状上：删 model Verify 阶段（副作用是保证
    app 在交付前被打开过一次，且是 Generate 阶段那条 STOP 指令的理由）、删
    `getDetails(appID:)`（副作用是触发预览路由挂载 WebView）、抑制 presentation
    （同样是挂载 WebView，且是唯一的那个）、删 `verification_mode`（副作用是一个 CI 门的
    子串锚点）。四个副作用**都不在各自函数的名字里，也不在直接调用链上**。
    判据：**先看谁在时序上紧跟着它、依赖它建立的状态**；如果它的注释解释的是「为什么在
    这里」而不只是「做什么」，那条注释就是答案。四次里四次是评审抓的。
    落地条目格式：**「删 X；已确认依赖 X 副作用的是 {…}，它们改为 {…}」**——
    写不出前半句就还没做完调研。

## 未决问题

- 冒烟门判据 4/5 的阈值：**本轮不需要**（已降为建议）。真要转阻塞时用真实应用标定。
- 副驾驶条与引导的具体形态：见该节警告，需单独一轮评审。

## 评审修正记录

四轮对抗式评审。第一轮 Codex 7 条 + 双评审；第二轮无命题扫描（11 条）；
第三轮 Codex 6 条 + 双评审；第四轮七个盲镜头 + 逐批打回 + 完整性/计划双批评（18 条）。

**第四轮砍掉的**（净简化）：`ResolveAppUiRequest.error_code`（无消费者、Android 不可实现、
名点的失败点是死资源）；`AppWorkflowTask*` 整族（registry 是每引擎一个，修不好；
且它防的竞态已被构建互斥锁排除）；`AppEventDto::AppBuildSucceeded`（`AppRecordChanged`
已经带且客户端已处理）；`StoreAppAnnotation.request_id`（论据未经证实）；8 秒无回执计时器
（为全局陷阱定制单命令协议）；workflow VM 新原语（改成挂 `build_app`，同时让修复构建
获得同一道门）。协议改动 5→1，bless 2→1。

**第四轮新增的必改项**：判据 6 的 `console.error` 捕获（否则每个模板应用恒为盲）；
判据 4/5 降为建议（五个镜头各自找到不同的误阻塞路径）；`.lingxi` 进构建键跳过表 +
标注移出 workspace（否则每轮修复全量重建，且 checkpoint restore 会删掉它）；
engine-mobile 开 `image-read`（否则 agent 根本读不了标注图）；controller 串行原语；
裁剪数学两端都错；`ConversationTurnCompletion` 的钩子位置；
`verify-local-app-supply-chain.py` 的 CI 门；`SKILL.md` 的静默过时。

**被评审推翻的、本文早期的论断**：「§0 阻塞其后一切」（不成立，工具名每轮经
`all_names()` 喂给模型）；「snapshot 复用 `find_nonterminal_local_app_workflows` 即可」
（registry 是每引擎一个，跟会话过滤无关）；「deadline 需要跨 crate 签名重构」
（论据错引 `#[cfg(test)]` 里的调用点）；「并发提交会乱序回执」（`@MainActor` 顺序）；
「uniffi 元数据已接近上限」（实测 15.8%/28.7%）；「Android 绑定是签入的」（gitignored）。

**四轮之后仍未验证的**（诚实列出，不是「大概没事」）：副驾驶条与引导两节没有任何镜头
读过；边界表 20 行里 17 行未被检查；**没有任何一轮跑过任何东西**——没有 `cargo test`、
没有 `xcodebuild`、没有真机，所有关于测试行为的判断都是静态阅读。
