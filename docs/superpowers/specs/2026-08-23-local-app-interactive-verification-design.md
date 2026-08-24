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
| ~~光标可视化~~ | **已撤销（2026-08-24 用户决定）** —— 见「被撤销的需求」 |
| 反馈提交节奏 | 默认攒清单，单条可「立即修」 |
| ~~操作回放条~~ | **已撤销（同上）** |
| 编辑的适用范围 | 库里任何 app、任何时候 |
| 落地平台 | 客户端 UI 仅 iOS；引擎改动两端共用 |
| 进入标注模式 | 悬浮按钮切模式 |
| 界面布局 | 一条两态复用的副驾驶条（输入框 / 药丸清单） |

### 非目标

- Android 的 overlay / 副驾驶条 UI（后补）；agent-facing 的 `inspect_ui`/`capture_ui`
  WebView contract 仍须两端一致
- 修 `SendPrompt.images`（移动端定义了但从不读，`host.rs:6146`）
- 任何形式的光标可视化（实时或截图标注）—— 见「被撤销的需求」
- 跨 app 引用
- 桌面/Web 客户端

### 被撤销的需求：光标可视化与操作回放条

2026-08-24 用户决定撤销。原始需求是「LLM 自动操作时显示鼠标位置」，用户澄清其用意是：
**在 LLM 通过 app-use 或 computer-use 控制、模拟点击时，让用户看清楚它到底做了什么操作**。

这是一个**跨子系统的通用诉求**，不是本地应用验收流程特有的——computer-use 是另一套
子系统（`lingxi-code/tools/computer-use/`，其 `:21` 明确把 overlay 划在范围之外），
Android-use 又是一套。把它塞进本设计只会得到一个只在本地应用里生效的半吊子版本。
**记录在此，等它作为独立条目被设计时一并覆盖三个子系统。**

撤销连带删掉的东西（不要因为「看起来缺了」而恢复）：

- `capture_ui` 的 `marker` 入参与 `result_json` 的 `last_action` 字段
- 客户端在 `captureFrame` 里的标记绘制（iOS `UIGraphicsImageRenderer` / Android `Canvas`）
- 副驾驶条的第三态（操作帧横排）、每动作一张 240 px 缩略图、24 项与 6 MiB 双上限
- 「任何程序化像素比对必须传 `marker:"none"`」这条规则——**没有标记就没有这个 bug 类**
- 给 `AppUiRequestDto` 追加 `origin` 字段的必要性——回放条不存在了，门自己的探测
  不会再被显示成「我正在操作你的 app」
- 引导里「第一次看到回放条」那一次 coach mark

净效果：副驾驶条从三态降为两态，`capture_ui` 的 schema 只剩 `rect`，
判据 3 不再有前置条件，客户端少一个 ring buffer 和两条内存预算。

## 架构

```
构建期                                    验收期（对话）
─────────────────────                    ──────────────────────
Design → Generate & Build                 用户玩 app
              │                            │ 点悬浮按钮进标注态
      [挂载点未定 · spike] ──► 冒烟门        │ 拖框 → 描述
              │                            ▼
              ▼                        标注入清单（客户端，落盘）
        workflow 结束                       │ 提交
              │                             ▼
              └──► app 打开 ──────────► StoreAppAnnotation × N
                                            ▼
                              切到 app scope，send(prompt) 并留住 token
                                            ▼
                              agent 改源码 → LocalAppBuild ← 同一道冒烟门
                                   → 一次 document load（机制未定，见冒烟门一节）
                                            ▼
                                  页面刷新 → 回到「用户玩 app」
```

⛔ **冒烟门的挂载点尚未确定，阻塞在一个 spike 上**（四个方案已被逐一证伪，见下节）。
右侧一列没有 workflow：它是 app 自己 scope 里的普通对话轮次，所以「刚创建完就改」与
「三个月后再改」走同一条路径。

### 标注（Annotation）

四个部分：**矩形**（CSS 像素，与 `pointer` 同一坐标系）、**描述**、**裁剪图**、
**命中元素**（`elementId`/`role`/`name`/`rect`）。

命中元素让 agent 从「猜」变成「查」：给一张裁剪图它只能猜「你说的那个按钮」，
给 `{elementId:"score-badge", role:"status", rect:[48,100,72,24]}` 它能直接 grep 到源文件。
canvas 应用没有可命中元素，退化为「矩形 + 图 + 描述」，这是可接受的降级。

每条标注有客户端生成的 `annotation_id`（UUID），提交时冻结成 `batch_id`。

## 冒烟门

替换 workflow 里由 agent 自述的 gate。

### ⛔ 挂载点未定：阻塞在一个 spike 上

**四轮纸面设计，四个挂载点，四次被杀，四次死因互不相同。本节不再给出方案。**

| 提案 | 死因（均在代码上验证） |
|---|---|
| ① workflow VM 新原语 `localAppSmoke()` | 是 `workflow::run_with_progress` 的签名变更（`workflow/src/lib.rs:827`），而 `tools/workflow/Cargo.toml` **没有 `tasks` 依赖边**（方向相反）⇒ trait 归属不成立；~16 个端到端契约测试会以 `ReferenceError` 挂掉 |
| ② `build_app` 的可服务成功点 | **`build_app` 不启动 runtime**（`local_apps_host.rs:3500-3504` 只回一句 hint）⇒ 初次构建时没有 runtime、没有 WebView；修复构建时看的是**还没 reload 的旧页面** |
| ③ `manage_runtime` start/restart | **restart 不重新加载页面**：端口刻意稳定 ⇒ URL 逐字节相同 ⇒ `LocalAppWebView.swift:938` 的 `guard loadedURL != url` 直接返回。另有四条独立失败（`start` 对运行中的 runtime 是成功的 no-op；agent 可以不调；`sceneWillEnterForeground` 会对每个先前运行的 app 调 `start`） |
| ④ ②+ 门自己发 `reload` | **`.reload` 不在自动放行集**（`LocalAppsStore.swift:980` 只放行 `.inspect`/`.captureView`）⇒ 弹权限模态，无人应答则 `UI_TIMEOUT` 120 秒后 `Err` ⇒ **好构建被判失败**；且 reload 的结果在 `webView.reload()` **之后立即返回**（`LocalAppWebView.swift:384-387`），**没有任何 settle 信号**可等 |

### 任何方案必须同时满足的五条（这才是本节的实际产出）

1. **宿主保证触发**：不依赖模型记得调用什么。
   ⚠️ 注意 `restore_checkpoint_value` **不走 `build_app`**，它直接调 `build_workspace`
   （`local_apps_host.rs:3032-3036`、`:3044`）——**它是三个「产出被服务字节」的路径里唯一
   会静默逃过门的那个**。早期版本称 `build_app` 「与调用来源无关」，不成立。
2. **新鲜文档**：被观测的必须是刚构建出来的那份。静态服务器直接对着
   `build/store/dist` 服务（`:2966-2967`）、`promote_build_root` 原地换目录
   （`local_apps_build.rs:1136-1140`）⇒ **服务器不用重启，下一次 HTTP 请求就吐新字节；
   缺的只是一次 document load**。缓存不是障碍：`index.html` 走 `no-cache` + ETag
   （`:4618-4626`、`:4746-4755`），hashed asset 改名。
3. **判定不经模型**。🚨 **这条四个方案全部没兑现**：`build_app` 的 `Err` 变成无结构的
   `tool_error` 文本块（`local_apps_mcp.rs:1445` → `:620-626`），而 workflow runtime
   **只观察工具名、从不检查结果**（`local_workflow.rs:1152-1156`）⇒ 判定只活在 subagent
   自己的上下文里。而且 `requirePreviewOnSuccess` 拦不住：`start_runtime` 从不读
   `workflow_state`，唯一前提是 `dist/index.html` 存在（`:2665-2675`）——**正是门的触发点
   刚刚证明过的**⇒ 门失败后 agent 照样拿得到 URL 并返回 `{ok:true}`。
4. **不需要用户在场**：`approvedUIAutomation`（`LocalAppsStore.swift:150`）是纯进程内存、
   从不持久化 ⇒ 每次冷启后的第一次都会弹窗。后台/无人值守构建必须能过。
5. **不越权**：`LocalAppBuild` 是 `AllowByDefault`，而 `LocalAppInspectUi`/`CaptureUi`/
   `ActOnUi` 都是 `DenyByDefault`，`defaults_per_tool.rs:128-130` 写明了威胁模型
   （*a pixel capture cannot redact anything it renders*）。**让免提示的工具把 DOM 读取和
   像素捕获做成未经提示的副作用，是权限表上的洞，不是设计取舍。**

### 下一步是 spike，不是第六轮纸面推演

四次死因分别是**时序、页面加载语义、权限表、结果不可观测**——没有一条是靠更仔细地读代码
能提前发现的，四次都是评审在事后从另一个角度撞出来的。按本仓库自己的判据
（*重建视口的判据必然逃过单测；实测语义 + 真机取证*），继续在纸上迭代的期望收益是负的。

**spike 的问题**：宿主能否在真机上，对一个刚构建完的 app，拿到一次**新鲜且不可伪造**的
观测，且不需要用户在场、不越权、不抢占屏幕？

三位评审收敛到同一个候选（**但它本身未经验证，不要当成结论**）：
store 持有的**离屏 `WKWebView`**（按 `makeUIView` 同款配置挂到 key window、零尺寸），
配一条**专用的、非 agent 的**宿主事件通道（与 `.inspect`/`.captureView` 同理由自动放行，
且**绝不写入 `approvedUIAutomation`**），用显式
`load(URLRequest(cachePolicy: .reloadIgnoringLocalAndRemoteCacheData))` 而非 `reload()`，
从 `didFinish`/`didFailProvisionalNavigation` 结算。它同时绕开 ①②③④ 的全部死因，
并且**顺带消除了「门会把用户从聊天里拽走」这个 UX 问题**。

spike 的验收标准就是上面那五条，每条都要在**真机**上被证伪或证实——尤其第 3 条，
它是本设计存在的全部理由。

**在 spike 有结论之前，阶段 2 不可计划。** 阶段 1 与阶段 3 不依赖它，可以先做。

### 顺带确认的一个结构性缺口：热重载不存在

本文数据流最后一步「热重载 → 回到用户玩 app」**当前没有任何机制实现**。客户端只有三条
reload 路径：首次挂载（`LocalAppWebView.swift:927-929`）、URL **变化**时的
`updateUIView`（`:938-941`）、以及 agent 主动发的 `reload` 动作（`:384-387`）。
**没有任何东西响应 runtime 状态变化**，而 restart 又不改 URL。

所以今天：用户报问题 → agent 改好 → 重建 → 重启 → **WebView 仍显示旧页面**。
`docs/local-apps/HANDOFF.md:397-401` 那条「Preview shows old code」记的就是这个，
但它被写成了提示词纪律（「repair build 必须 restart」），而 restart 根本不解决它。

这个缺口与门共用同一个解（一次 document load），所以由同一个 spike 覆盖。

### 判据

| # | 判据 | 阻塞？ |
|---|---|---|
| 1 | 运行时已启动且 `preview_url` 非空 | **阻塞** |
| 2 | `inspect_ui` 成功；document 为 interactive/complete、viewport 非零；canvas 应用要求 `canvases` 至少一个且 rect 非零 | **阻塞** |
| 3 | `capture_ui` 取到两帧（间隔 1.5 秒） | **阻塞** |
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

## 协议改动：只有一处

`ResolveAppUiRequest.result_json` 和 `AppUiRequestDto.value` 都是不透明
`Option<String>`（`commands.rs:438`、`local_apps.rs:774`），所以绝大部分能力不触及协议。

| 能力 | 承载 | 动协议 |
|---|---|---|
| `inspect_ui` 元素几何 / canvas rect / runtimeErrors | `result_json` 内 JSON | 否 |
| `capture_ui` 区域裁剪 | host schema 序列化到 `value` JSON | 否 |
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

### 落盘位置：`workspace/.lingxi/annotations/<annotation_id>.jpg`

⛔ **早期版本把它移到 `apps/<id>/annotations/`（workspace 之外）。那是错的，已撤回。**
移出去的两条理由，一条被证伪、一条是独立 bug 本来就要修；而移出去**引入了一个致命
新问题：agent 根本读不到那张图**——guest 只挂了 `/workspace/local-app-<id>`
（`LocalAppCodeBrowser.swift:56`，就是 workspace 目录本身），而
`escapes_local_app_workspace`（`workspace_lease.rs:202`）注释明写
「deliberately evaluated **before** generic allow rules」。路径够不着，权限也拒。

逐条清算当初那两个理由：

1. **「`.lingxi` 是构建键输入」——真的，但它是一个独立在线缺陷，修它就完了。**
   跳过表只有 `.git | .lingxi-build-state | node_modules | dist`
   （`local_apps_build.rs:766-769`），按 `file_name` 在任意深度匹配。加 `".lingxi"` 安全：
   `source-policy.json` 由 `restore_host_managed_files` 在 `:639` 于键计算 `:642` **之前**
   从编译进二进制的字节重新钉住，恒定；`app.manifest.json` 唯一与构建相关的字段
   `surface` 已经通过 `target.cache_tag()`（`:735`）独立折进键里；其余
   （`settings.local.json`/`app.json`/`design-spec.json`）是纯服务状态——**它们正是今天在
   churn 这个键的东西，跳过它们本身就是那个 bug 的修复**。
2. ⛔ **「checkpoint restore 会删掉它」——假的，我把代码读反了。**
   `.lingxi` **本来就被整目录递归特殊处理**：`exclude_service_documents`
   （`checkpoints.rs:257-274`）往 `.git/info/exclude` 写的是目录模式 `/.lingxi/`，
   且每次 `open_or_init` 都重写；`untrack_service_documents`（`:290-302`）
   `index.remove_dir(APP_STATE_DIR, 0)` 整目录；`read_service_documents`（`:314-347`）
   **只递归 `.lingxi/`**，不是整个 workspace；而 restore 的 checkout 请求的是
   `remove_untracked(true)` 而**不是** `remove_ignored`——`checkpoints.rs:249-251` 明确
   记录了这个区分。标注放在 `.lingxi/` 下能扛过 restore 两重。

可读性也已核实为三层独立成立：租约层 Reader 被**刻意豁免**
（`workspace_lease.rs:303-306, 355-358`，还有一条按名字钉住它的测试
`host_owned_metadata_is_readable_but_never_writable` `:807-822`，断言
`allows("Read", ".lingxi/settings.local.json") == true`）；模板种下的
`settings.local.json` 授 `Read(./**)`、只 deny `Edit(./.lingxi/**)`；
glob 走 gitignore 语义（`filesystem.rs:521`），`permission/src` 里**没有任何隐藏文件规则**。

🚨 **文件名必须是 `annotation_id`，不是时间戳序号。** 这是重启对账能做对的前提，
见「标注清单的持久化与状态机」。

**仍然要做**：把 `".lingxi"` 加进 `:766-769` 的跳过表——它是独立在线缺陷，
且 `spec.md` 与将来任何写进去的东西都会踩同一个坑。

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

### `capture_ui`：区域裁剪

对模型暴露的 schema additive 增加：

```json
{ "app_id": "…", "rect": {"x":10,"y":20,"width":120,"height":80} }
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

### 副驾驶条与引导

两态：刚框完变输入框；有待提交标注时变药丸清单。非标注态时整条收起。

⚠️ **本节与引导一节未经任何对抗性评审覆盖** —— 第四轮的七个镜头没有一个读过它们，
落地前需要单独一轮评审，且排在最后一个阶段。

引导复用 `AppState` 上的 `didSet + defaults` 形状——**不是 `@AppStorage`**
（全树零命中；`AppState` 是 `@Observable @MainActor final class`，`Theme.swift:25-27,48`，
`@AppStorage` 是 SwiftUI `DynamicProperty`，用不上去）。
⚠️ `Theme.swift:60-70` 在 `LINGXI_UI_TESTING == "1"` 时强制 `setupDone = true`，
新标志需要同形状的 override，否则 UI 测试跑的是「引导已跳过」的路径。
文案写进 `clients/translations/*.json`（真源），不是 `Localizable.xcstrings`（生成产物）。

### 标注清单的持久化与状态机

清单必须落盘：进程在 `StoreAppAnnotation` 成功后终止会丢掉清单却留下孤儿文件。
照抄 `LocalAppWebsiteDataStoreRegistry`（`LocalAppWebView.swift:1302` 起）——带版本的
`Codable` 日志。

⛔ **早期版本写「盘上有清单里没有的就删掉」。那会删掉它本来要保护的证据，已撤回。**
本节承认的窗口正是「引擎写完图、客户端还没提交日志就崩」，而在那个窗口里删除
= **删掉刚存好的截图**。而且我引的先例被我用反了：`removeDataForDeletedApps`
（`:1358`）是**日志驱动的删除**——只删日志点名**且**被权威快照确认的条目
（`:1356-1360`），并且在破坏性操作**之前**先记录意图（`:1334` 的注释原话是
Journals the exact store identifier BEFORE the engine is asked to delete，
`prepareForDeletion` `:1337`，引擎拒绝时还有 `cancelDeletion` `:1350` 回滚）。
我把「日志驱动」写成了「日志差集」。

正确做法**不需要 WAL、不需要 sidecar、不需要隔离区**——**让文件名就是 `annotation_id`**
（命令本来就带这个参数，它本来就贯穿清单/batch/回执）。于是：

- 盘上有、清单没有 ⇒ **按文件名里的 id 恢复成 `stored`**，不是删除
- 清单有、盘上没有 ⇒ 降回 `draft(error)`
- 真正无法归属的文件（id 不是合法 UUID）才回收，且只在该 app 无待提交批次时

（早期版本的文件名是 `1787543300-1.jpg` 这种时间戳序号，**不携带任何归属信息**——
那才是逼出「只能靠差集猜」的根源。）

```
draft ──Store 成功──► stored ──TurnStarted──► submitted
  │                                             │
  └──Store 失败──► draft(error)                  ├──digest 变化──► buildObserved
                                                │                     │
                                                │        turn completed 且门通过 ──► cleared
                                                │                     │
                                                └──turn cancelled/failed/无构建──► stored(可重试)
```

🚨 **`submitted` 不能直接跳到 `cleared`。** 构建成功严格早于 runtime 重启，重启严格早于
turn 结束，而 turn 在成功构建之后仍有**六个失败发布点**：`ConversationSource.swift:2385`
（用户 Stop / `cancelAndWait`）、`:4233/:4237/:4241/:4248`（`turnEnded` 四种映射）、
`:5306`（传输/协议/服务端/拒绝/内部错误）、`:3820`（斜杠命令）。而清理事件
（`AppEvent::RecordChanged` → `local_apps_bridge.rs:216-220` → `LocalAppsStore.swift:911`）
**完全不与 turn 耦合**——构建打戳那一刻就到了。

所以中间态 `buildObserved` 是必须的：digest 变化只记录「有新产物」，**只有在 token 匹配的
`ConversationTurnCompletion.completed` 到达、且门通过之后才清理**。

🚨 **清理必须比 `output_digest`，而 `build_workspace` 今天不告诉调用方它变没变。**
`build_cache_hit` 在写任何 provenance 之前就 `return Ok(())`（`local_apps_build.rs:642-644`），
而签名是 `async fn build_workspace(...) -> Result<(), AppError>`（`:589`）——**缓存命中与
真实重建对调用方逐字节相同**。所以 agent 为了复现问题先跑一次无改动的 `LocalAppBuild`
就会清掉整批标注。

修法**不需要任何协议工作**（早期评审建议加持久化 digest 字段 + DTO + bless，那是错的）：
digest 本来就在每次构建时算出来，只是从没离开过 `local_apps_build.rs`。

- 在 `build_workspace` 里把**上一版**被服务树的 digest 提前取出——`digest_tree(dist)`
  已经在 `build_cache_hit`（`:889`）里算了。**这个顺序是硬要求**：`promote_build_root`
  （`:1136-1140`）先把旧 `build_root` rename 走，`write_build_provenance` 在下一条语句
  （`:664`）才写新的 `build.json`，所以 `build_workspace` 一返回旧 digest 就没了。
- 返回类型加宽成 `Result<bool, AppError>`（或两变体的 `BuildOutcome`），语义是
  **被服务的 `dist` digest 变了**：缓存命中处（`:643`）为 `false`；miss 路径比
  `output_sha256`。两个 digest 直接可比——`validate_build_output`（`:979-989`）与缓存
  路径算的是同一棵树。

`build_id` 仍然只作 correlator。

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
| 构建仍在跑时提交标注 | **未定，见「source-vs-source 竞态」** |
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

### 🚨 source-vs-source 竞态：砍掉排队机制的理由不成立

早期版本砍掉客户端排队，理由是「build-vs-build 已被锁排除」。**锁只包住构建，不包住编辑。**
`local_apps_build.rs:626-627` 的进程级 `Mutex` 与 `:633-634` 的 per-app `flock` 都在
`build_workspace` **内部**，只覆盖 restore-host-files → build-key → Vite → promote；
任何 agent 的 `Edit`/`Write` 都不取这两把锁。唯一的另一个串行器 `reserve_turn`
（`host.rs:5361-5366`）是 **turn-vs-turn**，而 **workflow 不是 turn**。

交错路径**是本设计自己的 happy path，不需要用户做任何刁钻操作**：

1. app 会话里 agent 启动 `local-app-build`，工具返回 `async_launched`
   （`tools/workflow/src/lib.rs:1-9`、`:943`），**turn 结束，turn 槽位空出来**；
2. workflow 在 runtime-spawned worker 里继续跑（`local_workflow.rs:2036`；`:2059` 明说基线
   「fixed for this workflow's life **even as later turns update** the orchestrator's live
   baseline」），generate 阶段已起 runtime，app 已可预览；
3. 用户开始标注并提交 ⇒ 一个**新 turn** 编辑源码，而 workflow 的修复轮**同时**在编辑源码。

`WorkspacePermissionLeaseRegistry` 救不了：它是**授权**不是**互斥**
（`permission/src/workspace_lease.rs:60-83`）。

**本轮不给方案**——候选（进程级 per-app 编辑租约 / 把活动状态放进 process-wide app service /
恢复排队但改用可靠的终态信号）各有代价，而上一轮凭「读起来像能行」就砍掉排队机制正是这条
缺陷的来源。**与冒烟门的 spike 一并决定**：两者都取决于同一个问题——宿主对某个 app 的活动
状态究竟有没有一个可靠的、进程级的真相源。

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
- 清单持久化：**必须进入 in-flight 窗口**——在引擎写完图之后、客户端提交日志之前杀进程，
  重启后那条标注必须以 `stored` 恢复且**图还在**。
  ⚠️ 早期版本的测试是「重启后清单与目录对账正确」，那是**自我实现的**：它断言的正是
  规则本身，永远进不了那个窗口。
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
| 1 | 两端 `inspect_ui` 几何/canvas rect/runtimeErrors（含 `console.error` 与载荷预算）+ `capture_ui` 区域（修正后的裁剪数学）+ `image-read` + `.lingxi` 进构建键跳过表 + 两处 `local-app-build` 字面量改集合判定 | 无。**完全不碰协议** |
| **spike** | 宿主能否拿到新鲜且不可伪造的观测（五条验收，真机） | 1 |
| 2 | 冒烟门 + 判据 1/2/6 阻塞、4/5 建议 + workflow 脚本删减（含那五处同 commit 必改）+ `needs_user_review` 可见信号 + source-vs-source 的决定 | **spike** |
| 3 | `StoreAppAnnotation` + `AppRecord.last_build_id` + 唯一一次 bless | 1 |
| 4 | iOS overlay + controller 串行 + 标注状态机与持久化 + 提交路由 | 1 与 3 |
| 5 | 副驾驶条（两态）+ 引导（**先单独评审**，见该节警告） | 4 |

阶段 1 与 3 可并行。阶段 1 是 spike 与 2 的硬前置。
**阶段 2 在 spike 出结论前不可计划**；阶段 4、5 不依赖它。

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
- 副驾驶条（两态）与引导的具体形态：见该节警告，需单独一轮评审。

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

### 第五轮（Codex 6 条 + 定向核验 + 三角度攻击）

七条全部成立。**其中两条 P0 是第四轮我自己的修复造成的**：把门挪到 `build_app`
（那里没有 runtime），把标注挪出 workspace（那里 agent 读不到）。搬迁是两个改动，
我只审了搬走那半。

- **标注位置搬回 `workspace/.lingxi/annotations/`**，文件名改成 `annotation_id`。
  当初搬走的两条理由：构建键那条是真的但**是独立在线缺陷，修它就完了**；
  checkpoint 那条**是我把代码读反了**（`.lingxi` 本来就被整目录递归排除，
  restore 用 `remove_untracked` 而非 `remove_ignored`）。
- **重启对账规则整条撤回**：我引 `removeDataForDeletedApps` 作先例却把它的规则用反了
  ——它是**日志驱动**的删除，我写成了**日志差集**删除，那会删掉它本要保护的证据。
  对应的验收测试也是自我实现的，一并改。
- **digest 通道**：Codex 的补救（加持久化字段 + DTO + bless）是错的，**零协议工作**即可。
- **清理时机**：新增 `buildObserved` 中间态。
- **source-vs-source 竞态**：第四轮砍排队机制的理由不成立，与 spike 一并重定。
- ⛔ **冒烟门的挂载点回到未决**，并升级为「阻塞在 spike 上」。四个方案、四种死因
  （时序 / 页面加载语义 / 权限表 / 结果不可观测），没有一条是靠更仔细读代码能提前发现的。
  **其中第三条尤其重要：四个方案没有一个真正兑现过「判定不经模型」**——那是本设计存在的
  全部理由。
- 顺带确认**热重载机制不存在**（restart 不改 URL，而客户端 `guard loadedURL != url`）。

**方法论结论**：这个机制上五轮纸面推演产出了五个「读起来对、实际不通」的答案。
继续纸面迭代的期望收益为负，下一步是 spike。

**四轮之后仍未验证的**（诚实列出，不是「大概没事」）：副驾驶条与引导两节没有任何镜头
读过；边界表 20 行里 17 行未被检查；**没有任何一轮跑过任何东西**——没有 `cargo test`、
没有 `xcodebuild`、没有真机，所有关于测试行为的判断都是静态阅读。

**2026-08-24 撤销**：光标可视化与操作回放条整体移出本设计，见「被撤销的需求」。
评审曾以「未经评审」为由建议砍掉这两节，那不是砍需求的正当理由（那是评审自己的覆盖
缺口）；实际撤销依据是用户澄清了它的用意——它属于 app-use / computer-use 的通用能力，
不该在本地应用里做一个半吊子版本。
