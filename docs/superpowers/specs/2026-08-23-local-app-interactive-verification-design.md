# 本地应用：可交互的验收与编辑

日期：2026-08-23（2026-08-24 第六轮对抗式评审后修订）
平台：iOS（客户端交互）；引擎改动两端共用
协议：**代码当前是 `7.0.0` / blessed major `7`**（`client-protocol/src/version.rs:52`、
`snapshots/blessed_major.txt`）。create-flow §B 计划先 bless **`8.0.0`**；**那次 bless 落地之后**，
本文 Phase 3 在 8.0.0 与 profile-aware 代码上 rebase，不再从 7.0.0 独立 bless。
⚠️ 8.0.0 目前是**计划**不是既成事实——引用它时不要写成现状。

> 跨计划实施顺序、共享补丁 owner 与协议 rebase 统一由 [`2026-08-24-local-app-implementation-order.md`](./2026-08-24-local-app-implementation-order.md) 管理。本文保持交互验收的行为权威，不复制 create-flow/runtime-profile 设计。

> 本文经六轮对抗式评审。第五轮把冒烟门收束为必须真机取证的 spike；
> 第六轮补齐 annotation 原子持久化、build outcome 到 iOS 的传播、scope 切换后
> one-shot send，并清理了所有已撤回方案的残留契约。修正记录见文末。
>
> 📌 **本文的两条编辑纪律**（多轮评审各抓到一次同形状的问题后加的）。
>
> **纪律一（防漏）：**
> 任何决定都必须同时落到**四处**——正文、**阶段表**、**测试清单**、**状态/转换表**。
> 第四处是后补的：`discarded` 那轮改完正文和测试后，权威状态图里仍缺五个状态。
> 只写正文的决定在这份文档里已经被漏掉过三次（阶段 3 的必做项、Android 的
> `verification_unavailable` 分支、token 交接与重启降级）：写正文的人认为已经定了，
> 而按阶段表和测试清单干活的人看不到它。
>
> **纪律二（防复活）：把一个既有结构「补成全量」之前，先列出本文所有 ⛔ 标记的
> 已删除机制，当作排除清单。** 补全量表时人是**凭记忆重建**的，而记忆里留下的是
> 「这个机制存在过」，不是「它为什么被删」——于是被明确删掉的东西会作为正式条目
> 写回来，还因为进了「权威」结构而比原来更难被发现。
>
> 实例：`storing` 的 8 秒无回执计时器在 `StoreAppAnnotation` 一节被显式删除，
> 却在状态转换表里作为正式转换复活；同一轮里，测试清单还保留着比正文早一轮的
> 「丢弃最后一条即移除 token 关联」，与正文新规则直接冲突。
> 两条都不是读代码读错，是**没有回读这份文档**。
>
> 这两条纪律是一对：纪律一防「改了正文没改结构」，纪律二防「改了结构没回读正文」。

## 上游 create-flow 契约与防漂移门

本文是 create-flow 的**下游行为设计**，不是一份冻结在 2026-08-23 代码形状上的独立实现说明。verification 不依赖创建页长什么样，但依赖以下 post-create 契约：

| 上游不变量 | verification 的依赖 |
|---|---|
| `AppRecord.scaffolded == true` 是首次脚手架 commit point | 只对已经成形的 app 启用 build/smoke/annotation；空壳只回创建会话 |
| manifest 有合法 surface；新应用还有不可变 runtime profile | DOM/canvas 观测与 workflow 路由从持久事实读取，不从源码猜 |
| app 有稳定 workspace、pin/init session 与 app scope | annotation prompt 必须进入目标 workspace/session，不能重新扎根 |
| `local-app-build` / `local-canvas-build` 是完整 build workflow 集合 | lease、删除保护、smoke 与 source-vs-source 判定必须覆盖二者 |
| client protocol 已包含 create-flow 的 mode/request/scaffolded 契约 | Phase 3 只能追加 annotation/build-generation 字段，不能拿旧 golden 覆盖它们 |
| host-owned pnpm/build/restore 读取已经提交的 profile/lock | verification rebuild 不得重选 profile 或恢复 v1 package/lock |

**任何 create-flow 变更只要触及上表一项，就必须在同一变更中：**

1. 更新 [`2026-08-24-local-app-implementation-order.md`](./2026-08-24-local-app-implementation-order.md) 的顺序/owner；
2. 更新本节、阶段表、测试清单与状态/转换表中受影响的 verification 契约；
3. 在实现 PR/commit 的 `Related:` 或等价记录中写明 verification 所基于的 create-flow baseline commit；
4. 跑一条 downstream compatibility test：分别通过当前 direct-create/profile 路径与 shell→host-confirmation→Scaffold 路径创建应用，再执行 verification 的 build/inspect/capture 基础流程。禁止只用手写旧 fixture 证明兼容。

每个 verification 阶段开工前都要重新 rebase 并核对上表；spec 中的旧行号只能作调查入口。若代码与上游契约不符，先更新设计，不能让 executor 临场猜测。

Phase 3 另有机器门：contract index **必须包含**（这些由 create-flow §B 引入，**今天代码里还没有**——
`CreateApp.mode` 与 `AppRecordDto.scaffolded` 现在 grep 为 0 命中）`CreateApp.mode`、
`CreateApp.request_id`、`AppRecordDto.scaffolded`、创建成功/失败的 `request_id`，
并且旧 identity-proposal 命令/事件已删除；缺任一项就拒绝 verification bless，
防止用 major-7 snapshot 覆盖 create-flow。
⚠️ 这道门**在 create-flow §B 落地之前必然不通过**，这是设计意图（它就是用来挡「在 §B 之前
抢先 bless」的），不是待修的红。

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

### 任何方案必须同时满足的六条（这才是本节的实际产出）

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
6. **结果可观测**：宿主产生的 `SmokeReport` 必须以结构化数据到达 workflow
   的 repair 决策和 iOS 的 app 详情标记，不得要求 subagent 转写工具文本。
   报告至少带 `build_id` 与 `output_change_id`，让客户端能证明它验的是
   哪一代被服务字节；
   spike 必须明确这个载体是既有 DTO 字段、新 app event，还是进程级服务状态；
   在载体未定之前，不得承诺「冒烟报告不动协议」。

### 下一步是 spike，不是第六轮纸面推演

四次死因分别是**时序、页面加载语义、权限表、结果不可观测**——没有一条是靠更仔细地读代码
能提前发现的，四次都是评审在事后从另一个角度撞出来的。按本仓库自己的判据
（*重建视口的判据必然逃过单测；实测语义 + 真机取证*），继续在纸上迭代的期望收益是负的。

**spike 的问题**：宿主能否在真机上，对一个刚构建完的 app，拿到一次**新鲜且不可伪造**的
观测，且不需要用户在场、不越权、不抢占屏幕？

三位评审收敛到同一个候选（**但它本身未经验证，不要当成结论**）：
store 持有的**离屏 `WKWebView`**（按 `makeUIView` 同款配置，用与目标设备一致的
**非零固定 viewport** 挂到 key window 的可布局容器，容器移到可见边界之外；
**不得**用零尺寸、`isHidden=true` 或 `alpha=0` 伪装离屏，这三者都可能让 WebKit
不布局/不绘制，而现有 capture 路径已明确拒绝零宽高），
配一条**专用的、非 agent 的**宿主事件通道（与 `.inspect`/`.captureView` 同理由自动放行，
且**绝不写入 `approvedUIAutomation`**），用显式
`load(URLRequest(cachePolicy: .reloadIgnoringLocalAndRemoteCacheData))` 而非 `reload()`，
从 `didFinish`/`didFailProvisionalNavigation` 结算。它同时绕开 ①②③④ 的全部死因，
并且**顺带消除了「门会把用户从聊天里拽走」这个 UX 问题**。

🚨 **spike 还有一个比「离屏能不能截图」更硬的子问题：路由基座是单例。**
`LocalAppWebViewRegistry.controllers` 是 `[String: WeakController]`，**只以 `appID` 为键**
（`LocalAppWebView.swift:43`），而 `register(_:appID:)` 会**不可逆地关掉在位者**：
`if let replaced = controllers[appID]?.value, replaced !== controller { replaced.close() }`
（`:48-56`），`close()` 会 `detach` broker、`stopLoading`、摘掉全部 script message handler
并把 `webView` 置 nil（`:348-359`），**没有任何东西会把它重开**。
而 `makeUIView` 本身就是注册点（`:927`）。

三条出路全是死的：
1. 离屏视图注册 ⇒ **当场关掉用户正在看的预览**；
2. 不注册 ⇒ `execute` 只查 `controllers[appId]`（`:108-116`），
   判据 2/3/6 就是 `inspect_ui`/`capture_ui`，**没有任何路由能到达页面**；
3. 走私有路由绕开 registry ⇒ 可见预览随时可能挂载（用户点开 app，或任何 agent UI 请求
   触发 `requestedPresentationAppID`），`makeUIView` → `register` 又把在位者关掉。

而**共存正是标注流程的常态**：用户看着 app、提交药丸、agent 重建、门开火。

同轴的第二个问题：`dataStore(appID:)`（`:1314`）给两个视图**同一个 identified store**，
而 `:70-73` 的注释明写「WebKit requires every view using an identified data store to be
released before `remove(forIdentifier:)` runs」，`close(appID:)`（`:75-78`）只关**注册过的**
那个——它正是 `removeDataForDeletedApps`（`:1366`）删应用前调用的。一个不在册的离屏
WKWebView 会**静默破坏应用删除的清理**，而且它启动时的写入会落进用户自己的
`localStorage`/IndexedDB。

所以 spike 必须回答：registry 是否要改成按 `(appID, role)` 键控（`role ∈ {visible, smoke}`），
若是，只带 `appId` 的 `resolveBridge`/`deliverStreamFrame` 怎么办；以及谁负责在
`close(appID:)`/数据存储删除时关掉 smoke 视图。**这是对一个共享单例的客户端改造，
不是「按 `makeUIView` 同款配置」。**

spike 的验收标准是上面那六条**加上这个子问题**，每条都要在**真机**上被证伪或证实——
尤其第 3 条，它是本设计存在的全部理由。

**在 spike 有结论之前，阶段 2 不可计划。** 阶段 1a/1b 与阶段 3 不依赖它，可以先做。

🚨 **阶段 2 的门只覆盖 iOS，这必须写进契约而不是默认。** workflow 与引擎的改动两端共用，
但 spike 的候选是 iOS 的离屏 `WKWebView`，而 Android 的 WebView registry 是**另一套独立
实现**（`LocalAppWebView.kt`、`LocalAppsViewModel.kt`），不会自动复用。所以：

- 引擎侧必须显式表达「本平台没有冒烟能力」，并据此返回
  `delivery_status: "verification_unavailable"`——**绝不能在没有能力的平台上静默判通过**；
- Android 的对等能力是**独立的一次 spike + 独立的验收门**，不在本设计的阶段 2 里；
- 非目标一节里「Android UI 后补」指的是 overlay 与副驾驶条；
  **agent-facing 的 `inspect_ui`/`capture_ui` contract 仍然两端一致**（阶段 1），
  两者不要混为一谈。

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

### 可见预览路由与宿主冒烟路由必须分开

既有 agent UI 请求仍由 `requestedPresentationAppID` → `RootView.swift:309-313`
打开可见预览，这是它的权限与用户反馈边界。冒烟 spike 的验收标准则是
**不需要用户在场、不抢占屏幕**，所以它不得复用 `requestedPresentationAppID`
来挂载 WebView。若离屏候选在真机上不成立，spike 应判该方案失败，而不是
回退到把 app 强制推上屏。

## 协议改动：已确定两组，冒烟载体待 spike

`ResolveAppUiRequest.result_json` 和 `AppUiRequestDto.value` 都是不透明
`Option<String>`（`commands.rs:438`、`local_apps.rs:774`），所以绝大部分能力不触及协议。

| 能力 | 承载 | 动协议 |
|---|---|---|
| `inspect_ui` 元素几何 / canvas rect / runtimeErrors | `result_json` 内 JSON | 否 |
| `capture_ui` 区域裁剪 | host schema 序列化到 `value` JSON | 否 |
| 冒烟门报告 / `needs_user_review` | **待 spike 决定** | **待定，不得预判为否** |
| 标注 → 消息 | 复用 `SendPrompt` | 否 |
| 清单清理触发 | 复用 `AppRecordChanged`，但 `AppRecordDto` 新增 build 字段 | **是** |
| **存标注 / 删标注** | 两条新命令 + `AppEventDto` 回执 | **是** |

已确定的协议工作必须在**同一个结构更新批次**中完成：

- `ClientCommand::StoreAppAnnotation` 与 `AppEventDto::AppAnnotationStored`；
- `ClientCommand::DeleteAppAnnotation` 与 `AppEventDto::AppAnnotationDeleted`
  （`cleared` 的终态；理由见「标注清单的持久化与状态机」）；
- `AppRecordDto.last_build_id` 与 `AppRecordDto.last_output_change_id`；
- Rust `lower_record()`、TS protocol mirror/runtime guards、iOS/Android 绑定与 adapter/model、
  contract index/goldens/snapshots 同步更新。

本项目不要求兼容旧客户端，但「不兼容」不等于可以遗漏当前版本的
DTO lowering 或生成绑定。

⚠️ 本批不是从本文原始的 7.0.0 snapshot 开始。按 master order，create-flow 先 bless 8.0.0；本节 Phase 3 只在其上 rebase，并在 runtime-profile Phase 1 合并后修改 `AppRecord`/host/adapter。版本号由 rebase 后的实际 contract diff 与版本守卫决定，不在本文预写成第二个 8.0.0，也不复用旧 major-7 golden。

### 被删掉的三处协议改动，以及为什么

- ⛔ **`ResolveAppUiRequest.error_code`**：没有任何按它分支的消费者
  （`local_apps_host.rs:2113-2141` 五个失败返回没有一个是 source defect），
  且**在 Android 上不可实现**——spec 点名的失败点在那边不存在
  （`LocalAppWebView.kt:192` 是离屏截图错误），而 `local_apps_error_ui_not_open`
  是个**死资源**：五个语种的 catalog 都有、**Kotlin 零引用**。
  替换成上面那条一行的门策略。**这一删让阶段 1a/1b 都完全不碰协议。**
- ⛔ **`AppWorkflowTaskChanged` / `AppWorkflowTasksSnapshot` / `AppWorkflowTaskDto`
  + `task_runs` 加宽 + 客户端 active-set 状态机**：整族删掉。它防的是「构建中提交标注」，
  但 **`TaskRegistry` 是每个引擎一个**（`host.rs:3383`，在 `build_mobile_inner_with_ask`
  `:2329` 里），而提交路径每次切 scope 都新建引擎（`RootView.swift:915/928`）⇒
  新 registry 是空的，**跟会话过滤无关，修不好**。而 build-vs-build 本来就被进程互斥锁 +
  per-app 文件锁排除了（`local_apps_build.rs:626,634`；`storage.rs:185-216`）。
  ⚠️ **但「所以提交直接发、让锁去串行化」这个后续结论是错的，已删**——那两把锁
  **不覆盖 agent 的源码编辑**，见「source-vs-source 竞态」。提交时机待阶段 2 决定。
- ⛔ **`AppEventDto::AppBuildSucceeded` + `AppDetailsDto` 单独的 build id**：
  `AppRecordChanged` 已经是 blessed 事件且客户端已在处理（`LocalAppsStore.swift:911`）；
  `pin_init_session` 就是从 `with_app` 里发 `AppEvent::RecordChanged` 的现成先例
  （`service.rs:707-712` → `local_apps_bridge.rs:216-220`）。清理改成监听
  `AppRecordChanged`，用 `last_build_id` 对应一次构建、用 `last_output_change_id`
  对应最近一次**真正改变被服务字节**的构建。这复用既有 event 变体，
  但仍需要扩展 `AppRecordDto`。

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
// → AppEventDto::AppAnnotationStored {
//      app_id, annotation_id,
//      path: Option<String>, // ".lingxi/annotations/<id>/image.jpg"
//      error: Option<String>
//    }
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

### 落盘位置：`workspace/.lingxi/annotations/<annotation_id>/`

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
2. ⚠️ **「checkpoint restore 会删掉它」——按当时的表述是错的，但有一个真实的窄窗口。**
   （先说结论：**加上下面那条豁免之后，两种情况都安全**；不加豁免则窗口内的标注会丢。）
   `.lingxi` **本来就被整目录递归特殊处理**：`exclude_service_documents`
   （`checkpoints.rs:257-274`）往 `.git/info/exclude` 写的是目录模式 `/.lingxi/`，
   且每次 `open_or_init` 都重写；`untrack_service_documents`（`:290-302`）
   `index.remove_dir(APP_STATE_DIR, 0)` 整目录；`read_service_documents`（`:314-347`）
   **只递归 `.lingxi/`**，不是整个 workspace；而 restore 的 checkout 请求的是
   `remove_untracked(true)` 而**不是** `remove_ignored`——`checkpoints.rs:249-251` 明确
   记录了这个区分。**reset 之前就已存在的**标注因此能扛过 restore。

   🚨 **但 `restore_service_documents`（`:371-386`）还会 `remove_file` 掉「reset 之后存在、
   却不在 reset 之前那份快照里」的每一个 `.lingxi` 文件**——所以在 restore 窗口**之内**
   发布的标注会被删掉，而且是在引擎已经回执成功之后。这就是下文那条豁免要解决的窗口，
   两处必须一起读。

可读性也已核实为三层独立成立：租约层 Reader 被**刻意豁免**
（`workspace_lease.rs:303-306, 355-358`，还有一条按名字钉住它的测试
`host_owned_metadata_is_readable_but_never_writable` `:807-822`，断言
`allows("Read", ".lingxi/settings.local.json") == true`）；模板种下的
`settings.local.json` 授 `Read(./**)`、只 deny `Edit(./.lingxi/**)`；
glob 走 gitignore 语义（`filesystem.rs:521`），`permission/src` 里**没有任何隐藏文件规则**。

🚨 **`annotation_id` 在任何 `Path::join` 之前必须先校验**，恢复时再查非法 UUID 已经太迟。
命令收到的是裸 `String`，随后**直接当目录名用**——这是一条路径穿越面。

照抄 `local-apps/src/ids.rs` 既有的形状（`is_valid_app_id` `:46` / `validate_app_id` `:61`
→ `AppError`），加一对 `is_valid_annotation_id` / `validate_annotation_id`：
必须是**规范 UUID 且是单一路径组件**（无分隔符、无 `.`/`..`、非绝对路径），
在处理器最开头校验，失败即回 `invalid_annotation_id`，**在拼任何路径之前**。

🚨 **大小写必须按「校验时不敏感、落盘时归一为小写」处理，不能只收小写。**
两端的原生 UUID 大小写**相反**：Swift 的 `UUID().uuidString` 是**大写**
（`7C725956-…`），Kotlin 的 `UUID.randomUUID().toString()` 是**小写**。
只接受小写会让 iOS 生成的每一个 id 被引擎拒绝。

- 引擎：`validate_annotation_id` 大小写不敏感地校验规范 UUID 形状，
  **归一为小写之后**再拼路径；回执里回归一后的值。
- iOS：仍应写 `UUID().uuidString.lowercased()`——这是**本仓库既有约定**
  （`LXISHNativeBridge.swift:1605`、`:1732`，`LXISHNativeRootfs.swift:104`、`:286`），
  让日志、prompt 正文和目录名三处一致。

测试必须覆盖 `../`、绝对路径、含 `/` 与 `\\`、**大写 UUID（必须被接受并归一）**、
超长串、空串，以及**同一 UUID 的大小写两种写法映射到同一个目录**。

🚨 **目录名必须是 `annotation_id`，不是时间戳序号。** 每条标注是一个原子发布单元：

```
workspace/.lingxi/annotations/<annotation_id>/
├── image.jpg
└── annotation.json  # annotation_id/rect/viewport/hit_elements/note/schema_version
```

引擎先在同父目录写 `.tmp-<annotation_id>-<nonce>/`，两个文件都校验并写完后，
用一次 directory rename 发布为 `<annotation_id>/`；`AppAnnotationStored` 只能在 rename
成功后回执。这使「disk-only 恢复」既能找回图，也能找回药丸和 prompt 需要的
`note/rect/viewport/hit_elements`。详见「标注清单的持久化与状态机」。
同一 `annotation_id` 重试时：已存在的 `annotation.json` 与本次规范化输入一致则
幂等回成功；不一致则回 `annotation_id_conflict`，不覆盖已存证据。

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

1. 回调进入时立即在 RootView 保留一个按 `batchID` 键的
   `PendingAnnotationSubmission { appID, batchID, prompt, targetSessionID, state }`。
   `state` 只能是 `queued | switching | awaitingTurnStarted(token)`；
   任意重入都先按 `batchID` 去重。
   ⚠️ 早期版本还列了一个 `readyToSend`——**没有任何转换产生或消费它**，删掉。
   **pending 项在三个出口上都必须被移除**，之后由药丸 FSM 独占
   `stored`/`submitted`。

   🚨 **但 token 必须在移除 pending 之前先交出去，否则 completion 无从匹配。**
   本文后面要求用完整 `ConversationTurnToken` 匹配 `ConversationTurnCompletion`，
   而 token 原本只存在于 pending 项里。RootView 需要一份**非持久化**的
   `batchTurnCorrelation: [BatchID: ConversationTurnToken]`：
   - 在移除 pending 的**同一次 MainActor 执行里**原子写入；
   - 收到该 token 的 completion（任意 outcome）后移除；
   - 进程重启后天然为空——这与「重启把 `submitted`/`buildObserved` 降级为
     `stored(recovered)`」是同一条推论的两面，两处必须一起改。

   三个出口：**token 匹配的 `TurnStarted` 到达** ⇒ 移除 + 药丸转 `submitted`
   （**不是** `send()` 返回 token 那一刻——那时只进 `awaitingTurnStarted`）；
   read-only 返回 `nil` ⇒ 移除 + 药丸转 `stored(error)`；turn cancelled/failed ⇒
   移除 + 药丸转 `stored(可重试)`。**不写移除规则的话**，`awaitingTurnStarted`
   没有出边，而「后续重试边沿看到该状态必须 no-op」+「按 `batchID` 去重」会
   **静默吞掉用户对同一批次的重新提交**——边界表承诺的重试路径是死的。
2. 只要当前 source 正在 `streaming/isCancelling/sessionTransitionPending`，或 RootView
   `projectSwitching == true`，就**只排队，不调 `switchScope`**。尤其是
   `switchScope` 会先 `previousSource.cancelAndWait()`（`RootView.swift:909`），
   直接切会杀掉无关的 turn。
3. 目标 session 优先该 app 最近活跃 session。⚠️ **`restoredSessionID` 返回 `String` 不是
   `String?`**，miss 时返回 `""`（`RootView.swift:1030-1039`），判空要用 `.isEmpty`；
   而且它**抹掉了底层刻意保留的区分**——`ProjectScopedPreferences.storedActiveSessionID`
   返回 `String?` 正是为了区分「没有 key」和「用户选了新会话所以存了空 id」
   （`ProjectScopedPreferences.swift:28-32`）。必须读那个可选值。
4. 🚨 **不能用 `switchScope(initialPrompt:)`**：它只在转录为空时才发
   （`RootView.swift:978` 的 `if source.model.items.isEmpty`，而 `sessionResumed` 在同一个
   同步 handler 里就填好了 `items`，`ConversationSource.swift:4401/4418`）——
   而规则 3 选的恰恰是几乎必然非空的会话。`switchScope` 只负责切换，
   pending 项进入 `switching`，**返回 `true` 不等于可以立即 send**。
5. RootView 新增唯一 `flushPendingAnnotationSubmission()`。它只在以下边沿重试：
   `sourceGeneration` 变化、`projectSwitching` 变为 `false`、`$streaming` 投递
   `false`、`$sessionTransitionPending` 投递 `false`。

   🚨 **但「投递 false 之后再调 `send`」这个写法必然永远失败，必须改。**
   `@Published` 从 **willSet** 发布，所以 sink 里读存储属性拿到的还是**旧值**；
   而 `send()` 在内部**自己重读**那五个守卫（`ConversationSource.swift:2488-2494`，
   `@Published var streaming` `:388`、`sessionTransitionPending` `:463`），
   **没有任何入口能把投递值递给它**。于是在 true→false 那一沿，`send` 读到的仍是
   `true`、返回 `nil`，本文又把 `nil` 规定为「保持 queued 等下一个边沿」——
   而其余三个边沿都没动 ⇒ **永远发不出去**。

   ⚠️ 这个陷阱**就记录在本文让你贴着写的那个修饰符上面四行**：
   `RootView.swift:296-300` 原话「`@Published` publishes from `willSet` … on the
   true→false edge it reads `true` and the gate below would refuse forever.
   **Pass the DELIVERED value instead of re-reading the property.**」
   `landCreatedAppIfReady(streaming:)` 正是靠把投递值作为**参数**接进去才逃掉的
   （`RootView.swift:621`），而 `send()` 没有这个缝。

   **定死用这一种**（不留实现期选择）：sink 里**只**把投递值记进 pending 项并标脏，
   然后 `Task { @MainActor in flushPendingAnnotationSubmission() }` 跳到下一个 main-actor
   调度点再读属性——属性赋值是同步完成的，所以那时 `didSet` 已结束、读到的是新值。
   ⛔ **不要**给 `ConversationSource` 加接受投递值的 `send` 变体：那是一个被大量调用的
   共享 API，为一个调用方改它的代价远大于一次 runloop 跳转。
   ⛔ **不要**照字面实现「投递 false 后立刻重新检查五个守卫再 send」。

   ⚠️ 跳转后重新检查时可能发现**另一个**守卫已经置位（比如新一轮 turn 刚开始）——
   此时保持 queued 等下一个边沿，这是正确行为，不是失败。

   每次重试都重新检查：
   `activeScope == .localApp(appID)`、目标 session 已 adopt、source 不在五个
   `send` 守卫中。满足后才调 `send(prompt)`。
   - 返回 token：先把状态原子改为 `awaitingTurnStarted(token)`，再等
     token 匹配的 `TurnStarted`；后续重试边沿看到该状态必须 no-op；
   - 因为 streaming/cancelling/slash/transition 返回 `nil`：保持 queued，等下一个边沿；
   - 因 read-only 返回 `nil`：改为 `stored(error，可见且可重试)`，不自动循环。
6. 🚨 **TurnStarted/completion 的完整 token 都要到 RootView。**
   `ClientEventCenter` 里的原始 `TurnStarted` 只有线上 `turn_id`，没有 `sessionEpoch`
   或 source generation，不能拿它直接对全局 batch。`ConversationSource` 应在
   `acceptTurnEvent` 通过后发布 `turnStartedToken: ConversationTurnToken?`，RootView 观察
   这个值，把 `awaitingTurnStarted(token)` 改成 `submitted`，并记录当时的
   build baselines。
   **`ConversationTurnCompletion` 同样到不了 `LocalAppsStore`。** 它是 `@Published`
   （`ConversationSource.swift:397`，发布于 `:1996-2006`），唯一消费者是
   `ChatView.swift:225`；而 `LocalAppsStore` 只接了 `subscribe { handle(event:) }`
   （`RootView.swift:811`、`LocalAppsStore.swift:209`）。需要在 **RootView** 加一个显式
   钩子（挨着 `:300` 那个 `.onReceive(source.model.$streaming)`）——**不能放在详情/预览视图里**，
   它们会被提交路径自己的关 cover 动作卸载。两个钩子都用完整
   `ConversationTurnToken(clientTurnId + sessionEpoch)` 匹配 batch，不得只比线上的
   `turn_id`。

### 副驾驶条与引导

两态：刚框完变输入框；有待提交标注时变药丸清单。非标注态时整条收起。

🚨 **最小可用的副驾驶条是阶段 4 的功能门，不是阶段 5 的视觉附件。**
阶段 4 必须同时交付：输入态、药丸清单、提交、重试、丢弃，以及
`annotation_quota_exceeded` 的可见错误和恢复入口。没有这个最小界面，阶段 3 的配额会把
用户锁死在“请先清理”，却无处清理的状态。

⚠️ **阶段 5 只保留副驾驶条的视觉/交互打磨和引导。** 这部分未经对抗性评审覆盖，
落地前需要单独一轮评审；但它不得承载提交/重试/丢弃/配额恢复这四个功能性入口。

引导复用 `AppState` 上的 `didSet + defaults` 形状——**不是 `@AppStorage`**
（全树零命中；`AppState` 是 `@Observable @MainActor final class`，`Theme.swift:25-27,48`，
`@AppStorage` 是 SwiftUI `DynamicProperty`，用不上去）。
⚠️ `Theme.swift:60-70` 在 `LINGXI_UI_TESTING == "1"` 时强制 `setupDone = true`，
新标志需要同形状的 override，否则 UI 测试跑的是「引导已跳过」的路径。
文案写进 `clients/translations/*.json`（真源），不是 `Localizable.xcstrings`（生成产物）。

### 标注清单的持久化与状态机

清单必须落盘：进程在 `StoreAppAnnotation` 成功后终止会丢掉清单却留下孤儿文件。
照抄 `LocalAppWebsiteDataStoreRegistry`（`LocalAppWebView.swift:1302` 起）——带版本的
`Codable` 日志。用户创建药丸时就先把完整 `draft` 写入日志，然后才发
`StoreAppAnnotation`；状态更新也都用 temp + rename 替换整份日志。

⛔ **早期版本写「盘上有清单里没有的就删掉」。那会删掉它本来要保护的证据，已撤回。**
本节承认的窗口正是「引擎写完图、客户端还没提交日志就崩」，而在那个窗口里删除
= **删掉刚存好的截图**。而且我引的先例被我用反了：`removeDataForDeletedApps`
（`:1358`）是**日志驱动的删除**——只删日志点名**且**被权威快照确认的条目
（`:1356-1360`），并且在破坏性操作**之前**先记录意图（`:1334` 的注释原话是
Journals the exact store identifier BEFORE the engine is asked to delete，
`prepareForDeletion` `:1337`，引擎拒绝时还有 `cancelDeletion` `:1350` 回滚）。
我把「日志驱动」写成了「日志差集」。

恢复时以引擎原子发布的 annotation 目录为权威事实，以客户端日志为 UI/
batch 状态：

- 盘上有完整 `<annotation_id>/annotation.json + image.jpg`、清单没有 ⇒
  解析 sidecar，恢复完整药丸为 `stored(recovered)`，不是删除；
- 清单是 `draft/storing`、盘上已有同 id 的完整目录 ⇒ 升为 `stored(recovered)`；
- 清单是 `stored/submitted/buildObserved`、盘上没有 ⇒ 降回 `draft(error)`；
- 🚨 **清单是 `submitted` 或 `buildObserved`、盘上有 ⇒ 一律降到 `stored(recovered)`。**
  这两个状态的推进依赖 `ConversationTurnToken`，而 token 是**进程内的**，重启后
  live turn 已经不存在、无法再对账。**降级是安全的**（最坏是用户重新提交一次，
  证据都还在），而**留在 `submitted` 是不可恢复的**：没有任何边沿能再推动它，
  药丸永久卡住，且 `batchID` 去重会静默吞掉用户的重新提交。
  ⛔ 不要试图持久化 token 来「续上」——`sessionEpoch` 与 source generation 在重启后
  必然变化，续上的对账是假的。
- `.tmp-*` 或缺任一正式文件的目录不算标注；仅在没有对应 in-flight Store
  且超过有界宽限期后回收；
- **目录名不是合法 UUID** 的目录**不进药丸列表**——引擎只发布合法 UUID 名（rename 原子，
  不存在半个名字），所以它一定是外来垃圾，由宽限期清扫回收并记日志；
- **目录名合法但 `annotation.json` schema/字段校验失败**的目录进入可见的
  `recovery_error`（图可能还是好的，属于用户证据，且 id 合法所以可丢弃），
  不静默删除。

🚨 **必须补一条终态：目前整个引擎侧标注接口是只写的，没有任何删除路径。**
`cleared` 只是客户端状态，而恢复规则以**盘为权威**⇒ 两种读法都坏：
日志若丢弃 `cleared` 条目，则下次启动时**每一条历史标注都会作为
`stored(recovered)` 药丸回来**（每一次成功修复之后的每一次启动）；日志若永久保留
墓碑，则日志与盘上目录**无界增长**（每张图上限 170 KiB，`LocalAppWebView.swift:510`），
而且长在**构建工作区里**、没有任何 GC。参考的先例本身是一个带版本的
`UserDefaults` 键（`:1305`），换 `.v2` 就会把墓碑全孤立掉、退回第一种读法。

**定死用带回执的删除命令**，进阶段 3 的同一批协议改动：

```rust
ClientCommand::DeleteAppAnnotation { app_id, annotation_id }
// → AppEventDto::AppAnnotationDeleted { app_id, annotation_id, error: Option<String> }
```

⛔ **不用「引擎在 `build_and_record` 里 GC」那个候选**：`build_and_record` 手里
**没有 batch、没有 turn、没有 `SmokeReport`**，它无法判断哪些标注真的可以删——
它唯一知道的是「输出变了」，而输出变化和「这批反馈已被处理」不是一回事
（用户可能在修复途中又加了新标注）。

- `cleared` 的 `annotation_id` **以墓碑形式留在日志里，直到收到该 id 的
  `AppAnnotationDeleted` 且无 `error`**；墓碑在收到回执后才移除。
- 删除命令**可重试且幂等**：目录已不存在 ⇒ 回成功。
- 重启时若日志里还有墓碑 ⇒ 重发删除命令（这条要有测试）。
- `annotations/` 的数量与字节上限用**拒绝**而非静默回收：超限时
  `StoreAppAnnotation` 回 `annotation_quota_exceeded`，客户端提示用户先提交或清理。
  ⛔ 不要在超限时自动删最旧的——那会在用户没看见的情况下丢掉证据。
- **上限取 64 条 / 16 MiB，先到先算**（per app）。**两个上限都统计 `annotations/` 下的
  全部目录项，含 `.tmp-*`**——早期版本只让字节数含 tmp、条数仍只数正式目录，
  于是「反复创建空临时目录后崩溃」可以在 16 MiB 以下耗尽目录项/inode。
  依据：待提交集合是「一个人在一次验收里能记住的问题数」量级，64 已经很宽；
  16 MiB 同时给上面那条 checkpoint 豁免留出确定的最坏值。
- 超限时**不得产生任何目录**——连 `.tmp-*` 都不许留，配额检查在写第一个字节之前。
- 🚨 **配额必须统计 `annotations/` 下的全部字节，包括 `.tmp-*`**，否则它根本不限制磁盘：
  崩溃遗留和并发写入的临时目录在宽限期内不计数，可以反复绕过上限。
  顺序固定为：**先回收超过宽限期的 `.tmp-*`，再统计全部剩余字节，最后判配额**。

🚨 **必须有用户主动丢弃的入口，否则配额会把用户永久锁死。**
上面写「提示用户先提交或清理」，但设计里唯一的删除路径是「修复成功 ⇒ `cleared` ⇒
`DeleteAppAnnotation`」。一个标了 64 条却不想提交、或者其中若干条落盘失败的用户，
**没有任何办法把它们清掉**——配额永远满，标注功能永久不可用。

补一个终态 `discarded`（药丸上左滑/长按 ⇒ 丢弃）。**丢弃按当前状态分派，逐状态列全**——
早期版本用一句「`draft` 从未成功 Store ⇒ 纯本地删除」概括，漏掉了在飞和不可寻址两类：

| 丢弃时的状态 | 盘上有东西吗 | 动作 |
|---|---|---|
| `draft`（**从未发出** Store） | 否 | 纯本地删除，不发命令、不留墓碑 |
| `storing`（**已发出、回执未到**） | **未知** | 标 `discardPending` 并移出列表；**等 Store 回执**：成功 ⇒ 墓碑 + `DeleteAppAnnotation`；失败 ⇒ 纯本地删除 |
| `draft(error)` / `stored` / `stored(recovered)` | 是（除非 Store 失败） | 墓碑 + `DeleteAppAnnotation`，回执无 `error` 后移除墓碑 |
| `submitted` / `buildObserved` | 是 | 标 `discardPending` 并移出列表；**等该 batch 的 turn 走到终态 completion 之后**才发 Delete |
| `recovery_error`（sidecar 校验失败，**目录名是合法 UUID**） | 是 | 同 `stored`：墓碑 + `DeleteAppAnnotation` |

🚨 **`storing` 不能按 `draft` 处理。** 用户在 Store 已发出、回执未到时丢弃，若只删本地记录，
随后 Store 成功 ⇒ 盘上留下**无人认领的目录**，重启时又被恢复成 `stored(recovered)`——
用户丢弃过的东西自己回来了。等回执再分派是确定的，且不依赖两条命令的到达顺序。

🚨 **`submitted`/`buildObserved` 的 Delete 必须延后到 turn 终态。**
提交出去的 prompt 正文里**已经带着截图路径**，agent 可能正在读它、或正在按它修复。
立刻删 ⇒ agent 读到不存在的文件。同理**不能提前移除 `batchTurnCorrelation`**：
completion 还要靠它匹配。所以 batch 变空时**只在 turn 已终态的前提下**才移除关联；
turn 仍在飞就保留，等 completion 到达时一并清理。

🚨 **目录名不是合法 UUID 的目录不做成 `recovery_error`，由引擎在清扫里回收。**
早期版本把它显示成用户可丢弃的 `recovery_error`，但丢弃走的是
`DeleteAppAnnotation`，而该命令**要求合法 UUID** ⇒ 必然回 `invalid_annotation_id`
⇒ **这条药丸永远删不掉**。而且它本来就不可能是用户证据：引擎**只发布合法 UUID 名的目录**
（目录 rename 是原子的，不存在「改了一半」的名字），所以非 UUID 名一定是外来垃圾。
由宽限期清扫回收并记日志即可。

⇒ **`recovery_error` 只保留一种成因：目录名合法、但 `annotation.json` schema/字段校验失败。**
那种情况图可能还是好的，属于用户证据，且 id 合法所以丢弃走得通。

`discarded` 与 `cleared` 只在**成因**上不同（用户丢弃 vs 修复完成），终态机制共用一套——
不要为它另造一条删除路径。

🚨 **`StoreAppAnnotation` 不取 per-app 构建锁，而 checkpoint restore 会删掉窗口内新建的目录。**
`restore_service_documents`（`checkpoints.rs:371-386`）会 `remove_file` 掉**每一个
reset 之后存在、但不在 reset 之前快照里**的 `.lingxi` 文件。一次 restore 是完整的
git hard reset，不是瞬时的；在那个窗口里发布的标注会在引擎**已经回执
`AppAnnotationStored{path}` 之后**被删掉，随后恢复规则把它降成 `draft(error)`
——用户的截图没了。其他写这棵树的路径都持锁（`service.rs:760` 建 checkpoint、
`:797` restore、`local_apps_build.rs:633-634` 构建）。
**定死用豁免，不用锁**：把 `annotations/` 同时从
`read_service_documents` 的递归**和** `restore_service_documents` 的删多余项那一趟里
豁免掉。那一趟的用途是防止 reset 把**曾被跟踪过的** `.lingxi` blob 重新物化，
跟从未被跟踪的标注目录无关。

⛔ **不用 `storage::lock_app_build`**：那会让一次提交被一整次 Vite 构建（可达数分钟）
挡住，而提交是用户正在等的交互动作；而且它与「source-vs-source 竞态」那节的未决
决定耦合，会把一个已经能定的问题绑到一个还不能定的问题上。

豁免之后，标注在 restore 中的行为是**完全不被触碰**：它们已因 `/.lingxi/` 进了
`.git/info/exclude`，而 checkout 请求的是 `remove_untracked` 而非 `remove_ignored`。

**并发 restore 测试是阶段 3 的硬门**：在 restore 进行中发布一条标注，
restore 结束后该标注必须完整存在且回执有效。

⚠️ **顺带：把 `annotations/` 从 `read_service_documents` 的递归里跳过。**
它的注释是「**Read every file** under `workspace/.lingxi/`」，对每个文件
`std::fs::read` 进 `Vec<u8>`（`checkpoints.rs:317-352`），**无大小无数量上限**；
`restore()` 会同时持有**两份**完整拷贝（`:211` 的 `preserved` 与 `:371` 的第二次读），
外加 `:388` 每文件第三次整读做字节比对，rollback 路径再来一遍
（`local_apps_host.rs:3028`、`:3038`）。跳过之后标注反而**更安全**：
它们已因 `/.lingxi/` 进了 `.git/info/exclude`，而 checkout 请求的是
`remove_untracked` 而非 `remove_ignored` ⇒ **不碰它们 = 自动存活**。

**happy path**（只是导航用，**不是**契约）：

```
draft → storing → stored → submitted → buildObserved → cleared
```

**契约是下面这张全量转换表。** 每个状态把它能收到的每个事件都列全——
早期版本只画了 happy path 的箭头，于是 `storing` / `discardPending` /
`discarded` / `stored(recovered)` / `recovery_error` **五个状态从没出现在图里**，
只活在散文里；而散文允许作者只写想到的那几个分支，正是三条 P1 的成因。

| 状态 | 事件 | 结果 |
|---|---|---|
| 尚未进入 annotation 状态机：`record.scaffolded == false` | 用户尝试进入标注模式 | 不创建 `draft`；回到该 app 的创建会话并显示「先完成应用创建」 |
| 尚未进入 annotation 状态机：`scaffolded == true` 但 surface/profile 持久组合非法 | 任意 verification 入口 | 不创建 `draft`；显示 storage-corrupt 错误，禁止从源码猜 profile 后继续 |
| `draft`（本地，未发 Store） | 提交 / 立即修 | 发 Store ⇒ `storing` |
| | 用户丢弃 | 纯本地删除（盘上无物） |
| | 重启：盘上有同 id 完整目录 | `stored(recovered)` |
| | 重启：盘上无 | 保持 `draft` |
| `storing`（已发 Store，回执未到） | 回执 ok | `stored` |
| | 回执 err | `draft(error)` |
| | **一直没有回执** | **保持 `storing` 并可见地显示「保存中」——本设计没有超时计时器**（见下） |
| | 用户丢弃 | `discardPending(storing)` |
| | 重启 | 盘上有 ⇒ `stored(recovered)`；盘上无 ⇒ `draft` |
| `stored` / `stored(recovered)` | token 匹配的 `TurnStarted` | `submitted` |
| | `send` 返回 nil（read-only） | `stored(error，可重试)` |
| | 用户丢弃 | 墓碑 + `DeleteAppAnnotation` ⇒ `discarded` |
| | 重启 | 盘上有 ⇒ `stored(recovered)`；盘上无 ⇒ `draft(error)` |
| `draft(error)` | 用户重试 | 发 Store ⇒ `storing` |
| | 用户丢弃 | 盘上无 ⇒ 本地删除；盘上有 ⇒ 墓碑 + Delete ⇒ `discarded` |
| `submitted` | output-change generation 变化 | `buildObserved` |
| | turn cancelled / failed / 无构建 | `stored(可重试)` |
| | 用户丢弃 | `discardPending(turn)` |
| | **重启** | **降级 `stored(recovered)`**（token 是进程内的，无法续上） |
| `buildObserved` | turn completed 且门通过 且 `SmokeReport.output_change_id` 匹配 | `cleared` |
| | turn completed 但**门未通过** / 报告缺失 / `verification_unavailable` / `output_change_id` **不匹配** | `stored(可重试)`，药丸上显示具体原因 |
| | turn cancelled / failed | `stored(可重试)` |
| | 用户丢弃 | `discardPending(turn)` |
| | **重启** | **降级 `stored(recovered)`** |
| `discardPending(storing)` | Store 回执 ok | 墓碑 + Delete ⇒ `discarded` |
| | Store 回执 err | 纯本地删除 |
| | 重启 | 盘上有 ⇒ 墓碑 + Delete；盘上无 ⇒ 本地删除 |
| `discardPending(turn)` | 该 batch 的 turn 走到终态 completion | 墓碑 + Delete ⇒ `discarded` |
| | 重启 | 直接墓碑 + Delete（turn 不可能还在飞，安全） |
| `recovery_error`（名字合法、sidecar 校验失败） | 用户丢弃 | 墓碑 + Delete ⇒ `discarded` |
| | 重启 | 保持 `recovery_error` |
| `cleared` / `discarded`（终态） | 墓碑存在 | 发 `DeleteAppAnnotation` |
| | Delete 回执无 `error` | 移除墓碑（真正终结） |
| | **Delete 回执带 `error`** | **保留墓碑**，药丸区显示可见错误；按**有界退避**重试（1s/4s/16s，共 3 次），仍失败则停在可见错误并等用户手动重试或下次启动 |
| | 重启且墓碑仍在 | 重发 Delete（退避计数重置） |

⚠️ **目录名不是合法 UUID 的目录不是本表的任何状态**——它不进药丸列表，
由宽限期清扫回收（见上文）。

🚨 **`storing` 没有超时计时器，这是刻意的。** 早期版本有过一个 8 秒「无回执」计时器，
已在「`StoreAppAnnotation`」一节明确删除——它是为 `#[non_exhaustive]` 兜底臂
（`host.rs:7064-7074` 静默 `Ok(())`）定制的单命令活性协议，而那个陷阱对全部 40+ 命令
一视同仁，本仓库的答案是**兜底臂加 `debug_assert!` + 两半一起发版**。
⚠️ 写这张表时我把它当成正式转换又写了回来——**一个慢但会成功的 Store 会被提前判成
`draft(error)`，随后回执到达、目录落盘，变成迟到的孤儿**。已删。

没有计时器时 `storing` 的出口是：**回执**（正常路径）、**用户丢弃**（走
`discardPending(storing)`）、以及**重启对账**（按盘上有无分派）。在一个会话内
一直收不到回执就一直显示「保存中」——这是诚实的，比编造一个失败结论好。

🚨 **`submitted` 不能直接跳到 `cleared`。** 构建成功严格早于 runtime 重启，重启严格早于
turn 结束，而 turn 在成功构建之后仍有**六个失败发布点**：`ConversationSource.swift:2385`
（用户 Stop / `cancelAndWait`）、`:4233/:4237/:4241/:4248`（`turnEnded` 四种映射）、
`:5306`（传输/协议/服务端/拒绝/内部错误）、`:3820`（斜杠命令）。而清理事件
（`AppEvent::RecordChanged` → `local_apps_bridge.rs:216-220` → `LocalAppsStore.swift:911`）
**完全不与 turn 耦合**——构建打戳那一刻就到了。

所以中间态 `buildObserved` 是必须的。每个 batch 在对应 `TurnStarted` 到达时同时记下
`baseline_build_id` 与 `baseline_output_change_id`；只有后续 `AppRecordChanged`
的 `last_output_change_id != baseline_output_change_id` 才进入
`buildObserved(observedOutputChangeID: record.last_output_change_id)`。
**只有在 token 匹配的 `ConversationTurnCompletion.completed` 到达、且通过的
`SmokeReport.output_change_id == observedOutputChangeID` 时才清理**。这允许后续无改动
build 铸新 `last_build_id`，又不会把门报告错绑到别的输出上。
任一失败都回到 `stored(可重试)`；
重试 turn 在新的 `TurnStarted` 重新取当时的两个 baseline。

🚨 **build outcome 必须用 `output_digest` 判定字节是否真变，而
`build_workspace` 今天不告诉调用方它变没变。**
`build_cache_hit` 在写任何 provenance 之前就 `return Ok(())`（`local_apps_build.rs:642-644`），
而签名是 `async fn build_workspace(...) -> Result<(), AppError>`（`:589`）——**缓存命中与
真实重建对调用方逐字节相同**。所以 agent 为了复现问题先跑一次无改动的 `LocalAppBuild`
就会清掉整批标注。

修法不需要把 digest 本身送上线，但**需要把「最近一次输出变化」持久化并
通过 `AppRecordDto` 送到客户端**。digest 本来就在每次真构建时算出来，
只是从没离开过 `local_apps_build.rs`。

- 在 `build_workspace` 里把**上一版**被服务树的 digest 提前取出——`digest_tree(dist)`
  已经在 `build_cache_hit`（`:889`）里算了。**这个顺序是硬要求**：`promote_build_root`
  （`:1136-1140`）先把旧 `build_root` rename 走，`write_build_provenance` 在下一条语句
  （`:664`）才写新的 `build.json`，所以 `build_workspace` 一返回旧 digest 就没了。
- 返回类型加宽成
  `Result<BuildOutcome { output_changed: bool }, AppError>`：缓存命中处（`:643`）
  为 `false`；miss 路径比较新旧 `output_sha256`。两个 digest 直接可比——
  `validate_build_output`（`:979-989`）与缓存路径算的是同一棵树。
- `LocalAppsHostBroker` 增加一个唯一的 `build_and_record` 包装，所有会发布
  被服务字节的生产路径都走它：`build_app`、`restore_checkpoint_value` 的正向
  rebuild，以及 restore 失败时的 rollback rebuild。不得只在 `build_app`
  里打戳，否则 checkpoint restore 会产生新字节却留下旧 record。

### build generations：两个持久化 UUID，落在 `AppRecord` 并传到 DTO

三条显而易见的替代方案都不行：

- **不能放 build provenance**：`build.json` 在 `build/store` 里（`local_apps_build.rs:865-867`），
  而 `promote_build_root` **把整个目录 rename 走**再换新树（`:1136-1140`）⇒ 每次 promote
  先销毁再于下一条语句重写（`:663`→`:664`），中间有「活的可服务产物没有 id」的窗口；
  且 `write_build_provenance` 只有 temp+rename、**无 fsync**。
- **不能用 `buildKey`/`outputSha256` 当 id**：内容派生，相同源码重建后不变。
- 🚨 **绝不能用进程内计数器**：`RootView.swift:915/928` 每次切 scope 都新建引擎，
  而提交路径自己就会切 ⇒ 新引擎重发 `build-1`，基线永远相等，清理一次都不触发。

落地：`AppRecord` 与 `AppRecordDto` 同时增加：

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub last_build_id: Option<String>,
#[serde(default, skip_serializing_if = "Option::is_none")]
pub last_output_change_id: Option<String>,
```

🚨 **这对 serde 属性是硬要求，漏了会让整个 app 库读不出来。**
（第五轮给这条规则配的理由——「否则 `AppManifest::hash()` 一变」——**是错的**：
`AppManifest::hash()` 序列化的是 `AppManifest`，字段里没有 `AppRecord`。
第六轮把错理由和对规则一起删掉了。正确理由如下。）

`AppRecord` 持久化在 `apps/index.json`，并镜像到 `workspace/.lingxi/app.json`
（`types.rs:138-139`、`storage.rs:364`）。这个仓库**没有迁移函数**——
`storage.rs:34-36` 原话：*"the legacy-state serde aliases … **IS the on-disk migration**"*。
serde 对 `Option` **没有隐式默认**，所以裸字段会让
`serde_json::from_str::<AppIndexFile>` 报 `missing field`，`load_all`（`storage.rs:474`）
整体返回 `Err` ⇒ **不是某个 app 打不开，是整个库打不开**。
升 schema 也不是出路：`ensure_schema_version`（`:451-459`）对任何非
`APPS_SCHEMA_VERSION` 的值硬失败。

照抄 `init_session_id` 的形状（`types.rs:167-176`），它的 docstring 把理由写死了：
*"Same default+skip serde shape as `conversation_id`, **so old stores load unchanged and
the goldens stay byte-identical**"*。签入的 golden
`local-apps/tests/fixtures/v1/apps/index.json` 两个键都没有，
`serde_compat.rs:14-27` 钉住再持久化的字节——不用跑就能证明。

- 每次 `build_and_record` 得到可服务结果都铸新 `last_build_id`；
- 仅当 `BuildOutcome.output_changed == true` 时，把 `last_output_change_id`
  设为同一个新 build id；缓存命中/无改动构建保持旧值；
- 两个字段都由 `AppService` 的单个 `with_app` mutator 持久化，然后发
  `AppEvent::RecordChanged`（`pin_init_session` 先例，`:707-712`）；
- `lower_record()`、`AppRecordDto`、TS mirror/guards、iOS `LocalAppSummary`/adapter/store、
  Android adapter、contract index/goldens 和生成绑定必须同批更新。

两个字段为 `Option` 是因为新建 app 在首次成功构建前没有 generation，
不是为了在当前项目里兼容旧客户端。

## 提交时组装的消息

```
我在运行「贪吃蛇」时标了 3 个问题。视口 393×852 CSS 像素。

1. 分数一直不涨
   区域 x=44 y=96 w=118 h=74
   截图 .lingxi/annotations/550e8400-e29b-41d4-a716-446655440000/image.jpg
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
| 提交时遇到 send 守卫 | streaming/cancelling/slash/transition/project switch 期间不调 `send`，保持 queued 并由 RootView 在下一个状态边缘 one-shot flush；read-only 则回 `stored(error，可重试)` |
| 修复轮 turn 被取消/失败 | 经 RootView 的显式钩子把 batch 退回 `stored(可重试)` |
| 门开火时宿主 WebView 未挂载 | 仅走 spike 选定的非可见宿主路由；不得回退到 `requestedPresentationAppID`。有界 deadline 内未完成非零 viewport 挂载则 `infrastructure_unavailable` |
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

- **上游兼容门**：测试 app 不得只靠手写 fixture 构造。分别通过当前 direct-create/profile 路径与 shell→host-native profile confirmation→`LocalAppScaffold` 路径创建一个 app，断言 `scaffolded`、surface/profile、workspace/session pin、workflow 路由正确，再跑 build/inspect/capture 基础流程。
- **协议防回退门（Phase 3）**：contract index/golden 必须保留 create-flow 的 `CreateApp.mode`、两处创建 `request_id`、`AppRecordDto.scaffolded`，并继续缺少已删除的 identity-proposal 命令/事件；从 major-7 fixture 生成 snapshot 必须失败。
- **共享补丁唯一性**：Phase 1 直接测试 `local-app-build` 与 `local-canvas-build` 共用 lease/delete guard，以及 workspace `LINGXI.md` 已由上游 resolver 加载；不得为 verification 新增第二套实现路径。
- 冒烟门六条判据各有独立测试，**每条都要有反向用例**。
  🚨 **判据 6 的反向 fixture 必须是「真实出厂的模板 + 一个会抛异常的屏幕」**，
  它必须变红——用手写的 `throw` 页面测不出 `ErrorBoundary` 那条路径。
- 🚨 **无冒烟能力的平台必须红**：在没有注册冒烟能力的宿主上跑完整 workflow，
  终态必须是 `delivery_status: "verification_unavailable"`，**不得是 `ready`**。
  这条是防止共享的 workflow 在 Android 上静默判通过的唯一机器守卫——
  正文写了不算，必须有测试。
- 门的触发点与事件载体由 spike 决定；测试只钉住**宿主必然触发**、
  覆盖 `build_app` 与 checkpoint restore/rollback 三条会改变被服务字节的路径，
  且 JS/subagent 无法注入 `status:"passed"` 覆盖宿主报告。不得在 spike
  前将测试写死为 `build_app` 触发。
- 🚨 **smoke 视图与可见预览的共存必须进测试门**（不是只写在 spike 问题里）：
  (i) 可见预览挂载时冒烟门跑一遍，**用户的预览不得被 `close()`**、页面状态不丢；
  (ii) 反向：冒烟视图存在时用户打开 app，两者都能收到各自的消息，
  `resolveBridge`/`deliverStreamFrame` 不串台；
  (iii) 删除 app 时**两个视图都被关闭**，`remove(forIdentifier:)` 不因残留引用失败
  （`LocalAppWebView.swift:70-78`、`:1366`）。
- spike 候选的 WebView 必须使用非零 viewport；未挂载/didFail/timeout
  按门内判定记为 `infrastructure_unavailable`，workflow 终态记为 `verification_unavailable`
  （两个名字**不是同义词**：前者是单条判据的分类，后者是 `delivery_status` 的取值），必须释放专用 pending request 与离屏 WebView，
  且 `agent_calls` 不变。
- `StoreAppAnnotation` 路径测试钉住
  `apps/<id>/workspace/.lingxi/annotations/<annotation_id>/{annotation.json,image.jpg}`；
  另测非法 base64/JSON/rect、尺寸上限、temp-directory rename 发布，以及
  图写完/目录 rename 前崩溃不得产生正式 annotation。
- **删除生命周期**：`DeleteAppAnnotation` 幂等（目录已不存在 ⇒ 成功）；重启后日志里的
  墓碑会**重发**删除命令并在收到回执后消失；超配额时 `StoreAppAnnotation` 被拒且
  `annotations/` 下**不留任何目录（含 `.tmp-*`）**。
- **并发 restore（阶段 3 硬门）**：restore 进行中发布一条标注，restore 结束后该标注
  **完整存在**且回执有效。
- **构建键回归测试**：往 `workspace/.lingxi/<任意>` 写内容**不得**改变 `workspace_build_key`。
- **`Read .lingxi/annotations/<id>/image.jpg` 必须返回 image 结果**（守住
  `image-read` feature 和 host-owned metadata 可读边界）。
- build outcome：缓存命中和无字节变化的 miss 都使 `last_build_id` 变、
  `last_output_change_id` 不变；真正改变 dist 时两者同步变到同一新 id。
- 协议：command/event/record goldens、contract index、TS union/guards、`clients/shared`
  完整 `npm test`；断言 `AppAnnotationStored` **与 `AppAnnotationDeleted`** 都位于 `AppEventDto`，且
  `AppRecordDto` 的两个 build generation 字段经 lowering 到达两端客户端。
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
  `sessionTransitionPending` 期间不发、变为 false 后由 RootView one-shot flush **恰好发一次**；
  重复 state edge 不重发；turn cancelled/failed 让 token 匹配的 batch 可重试。
- **controller 串行**：慢 `execute` 与并发 `makeAnnotation` 观察到同一个 document generation。
- **token 交接**：`TurnStarted` 到达 ⇒ pending 被移除**且** `batchTurnCorrelation` 里同时
  出现该 batch 的完整 token；收到该 token 的 completion（任意 outcome）后关联被移除；
  丢弃某 batch 的最后一条标注**只在该 batch 的 turn 已走到终态时**才移除关联；
  turn 仍在飞时关联必须保留（否则 completion 无从匹配）。
- **重启降级**：日志为 `submitted` 或 `buildObserved`、目录完整、进程重启 ⇒ 药丸必须回到
  `stored(recovered)` 且**可再次提交**（验证 `batchID` 去重不会吞掉重新提交）；
  `batchTurnCorrelation` 重启后为空。
- **用户丢弃（逐状态，按转换表全覆盖）**：
  `draft` 不发命令；`storing` 中丢弃后 Store **成功** ⇒ 目录被删且重启后**不会**复活，
  Store **失败** ⇒ 纯本地删除；`submitted` 丢弃后在 turn 终态**之前不得**发 Delete
  （断言截图在 turn 期间一直可读），终态后才删；`batchTurnCorrelation` 在 turn 仍在飞时
  **不被提前移除**；`recovery_error`（合法 UUID）可丢弃成功。
  丢弃到配额以下后 `StoreAppAnnotation` 重新可用。
- **`discardPending` 的两条重启分支**：`discardPending(storing)` 重启后，盘上**有**目录 ⇒
  墓碑 + Delete，盘上**无** ⇒ 纯本地删除；`discardPending(turn)` 重启后**直接**墓碑 + Delete
  （不再等 completion）。
- **`buildObserved` 下用户丢弃** ⇒ 进 `discardPending(turn)`，且在 turn 终态**之前**
  不得发 Delete。
- **`buildObserved` 的非通过出口**：门未通过 / 报告缺失 / `verification_unavailable` /
  `output_change_id` 不匹配，四种都必须回到 `stored(可重试)` 且药丸显示原因——
  **不得停在 `buildObserved`**。
- **Delete 失败**：回执带 `error` ⇒ 墓碑保留、显示可见错误、按有界退避重试；
  退避耗尽后不静默循环。
- **阶段 4 最小副驾驶条（硬门）**：药丸清单必须实际暴露提交/重试/丢弃；
  触发 `annotation_quota_exceeded` 后错误可见，用户丢弃到配额以下后能继续创建标注。
  没有这条验收，阶段 4 不完成，不得以“阶段 5 会做 UI”为由交付。
- **非法目录名**：手工放一个非 UUID 名的目录进 `annotations/` ⇒ 它**不出现在药丸列表里**，
  且被宽限期清扫回收（有日志）。
- **配额条数**：只创建空 `.tmp-*` 目录也会计入条数上限。
- 清单持久化：**必须进入 in-flight 窗口**——在引擎发布 annotation
  目录之后、客户端记录 Store 回执之前杀进程，重启后必须从
  `annotation.json` 恢复 `note/rect/viewport/hitElements` 和图片，状态为
  `stored(recovered)`。
  ⚠️ 早期版本的测试是「重启后清单与目录对账正确」，那是**自我实现的**：它断言的正是
  规则本身，永远进不了那个窗口。
- 清理：两次无改动的连续 build ⇒ `last_build_id` 不同、
  `last_output_change_id` 相同、**batch 存活**；输出变化但 turn/runtime/门后续失败
  ⇒ 停在/`buildObserved` 回退到 `stored`，不清理。
- ⚠️ WebView 与预览路由**当前没有 `accessibilityIdentifier`**，overlay 需要自己的 id；
  **SwiftUI 容器上的 `accessibilityIdentifier` 会覆盖所有子元素的 id**，
  需 `.accessibilityElement(children: .contain)`。UI 测试要 `-testLanguage zh-Hans`。
- 引擎是预编译 xcframework，**`xcodebuild` 不重编 Rust**：判据是
  `strings -a …/libios_framework.a | grep <新字符串>`，装机后验 `LingxiCode.debug.dylib`
  （主二进制只有 91 KB，grep 它得 0 = 假阴性）；重建需 `LINGXI_REUSE_STAGED_LINUX_RUNTIME=1`。

### 真机

冒烟门必须上机验证，不接受单测绿即交付：重建视口/几何的判据历史上必然逃过单测。
DOM 应用与 canvas 应用各走一遍完整流程（框选、提交、修复、热重载）。
Android 虽不落 overlay，阶段 **1a** 的共享 UI tool contract 仍需设备或 instrumentation 覆盖；
若该轮上不了 Android 设备，**必须记为阶段 1a 未完成**，不能用「Android UI 非目标」把
agent-facing contract 判绿。

## 分阶段落地

| 阶段 | 内容 | 依赖 |
|---|---|---|
| **1a** | 两端 `inspect_ui` 几何/canvas rect/runtimeErrors（含 `console.error` 与载荷预算）+ `capture_ui` 区域（修正后的裁剪数学）+ `image-read` | **无依赖，今天即可开工**。这些只动 `LocalAppWebView.swift`/`.kt` 与 `engine-mobile/Cargo.toml`——按 create-flow 全文 grep，`LocalAppWebView` 与 `image-read` 命中数**均为 0**，零文件重叠。**完全不碰协议** |
| **1b** | `.lingxi` 进构建键跳过表 + 两处 `local-app-build` 字面量改集合判定（lease 与删除守卫） | master step 2 的写入窗口——这三处落在 `local_apps_build.rs` / `local_workflow.rs` / `registry.rs`，与 create-flow 有文件级重叠，按 master order 串行 |
| **spike** | 宿主能否拿到新鲜且不可伪造的观测（六条验收，真机） | 1a |
| 2 | 冒烟门 + 判据 1/2/6 阻塞、4/5 建议 + workflow 脚本删减（含那五处同 commit 必改）+ `needs_user_review` 可见信号 + source-vs-source 的决定。**iOS only**：引擎必须显式表达「本平台无冒烟能力」并返回 `verification_unavailable`，Android 对等能力是独立 spike | **spike + create-flow Web runtime profile Phase 1**；所有 workflow 判定必须覆盖最终 profile 集合 |
| 3 | `StoreAppAnnotation` 原子目录 + **`DeleteAppAnnotation`** + **`annotation_id` 落盘前校验/归一** + **`annotations/` 的 read/delete 双豁免** + `AppRecord`/`AppRecordDto` 两个 build generation 字段 + `BuildOutcome`/`build_and_record` + 协议 bless/绑定生成。**硬门：并发 restore 测试** | 1 + create-flow 协议 8.0.0 + runtime-profile Phase 1；按 master order rebase 后再 bless |
| 4 | iOS overlay + controller 串行 + 标注状态机与持久化 + RootView one-shot 提交路由 + **最小可用副驾驶条**（输入/药丸清单/提交/重试/丢弃/quota 错误） | **2 与 3** |
| 5 | 副驾驶条的视觉/交互打磨 + 引导（**先单独评审**，不承载功能性恢复入口） | 4 |

⛔ **阶段 3 不得先于阶段 1b 的「`.lingxi` 进构建键跳过表」交付。**
`AppRecord` 就存在 `workspace/.lingxi/app.json`（`storage.rs:364`，`MetadataMirror`
每次持久化都重写，`:829-835`），所以每次构建都铸新 `last_build_id` **本身就在 churn
一个构建键输入** ⇒ `build_cache_hit`（`local_apps_build.rs:642-644`）永远不再命中，
每次真机构建都是完整 Vite 重建。而阶段 3 自己的验收测试（两次无改动构建 ⇒
`last_build_id` 变、`last_output_change_id` 不变）**在这个坏状态下照样通过**，
本阶段没有任何东西会发现它。`workspace_build_key` 那条回归测试必须**同时**进
阶段 1b 和阶段 3 的门。

阶段 1b 与 3 在 verification 自身的数据依赖上可以并行，**但集成上不得并行写入**：master order 将阶段 3 排在 runtime-profile Phase 1 之后，以避免 `AppRecord`、DTO、host、bindings 和客户端 adapter 两轮冲突修改。阶段 **1a** 是 spike 与 2 的硬前置（判据依赖 1a 新增的 rect 与 runtimeErrors）；
**1b 不是** spike 的前置，它只在阶段 3 之前必须落地（构建键那条）。
**阶段 2 在 spike 出结论前不可计划；阶段 4 不得在 2 之前交付**，因为提交路由的
source-vs-source 互斥策略与 `buildObserved → cleared` 的门结果都由阶段 2 确定。

## 先于本设计存在的在线缺陷

四轮评审顺带确认的现存 bug。⚠️ **其中第 1、3 条由本设计 Phase 1b 唯一实现；第 4 条由 create-flow §0 唯一实现**——
列在这里是为了说明「它们不是本设计引入的、也应当独立于本设计被修」，
不是说它们在范围之外。第 2 条与本设计无关，建议单独开条目。

1. **`.lingxi` 不在构建键跳过表**（`local_apps_build.rs:766-769`）——任何写入都让缓存永不命中。
2. **`Paused` 的收养检查点让 app 永久不可删**：`registry.rs:903` 把收养的检查点登记为
   `TaskStatus::Paused`，而 `:794` 的判据是 `!is_terminal()`，`Paused` 不是终态 ⇒
   删除守卫（`host.rs:5938-5956`）永远认为有活的构建。改为
   `matches!(status, Running | Queued)`。
3. **canvas 构建既没有 workspace lease、也不被删除守卫保护**：
   `local_workflow.rs:1987` 与 `registry.rs:796` 都硬编码 `"local-app-build"`
   ⇒ canvas 应用可以在构建进行中被删掉。按 master order，本条的唯一实现 owner 是
   verification Phase 1b；create-flow runtime-profile 只保留进入断言与回归测试。
4. **`LINGXI.md` 在移动端从未加载**：`session_cwd` 持 guest 路径
   （`host.rs:3291-3293, 3329`），`build_system_prompt` 走了 `prompt_probe_cwd_resolver`
   做 guest→host（`conversation.rs:12059-12075`）但**那条路径已不再渲染**
   （`prompt/mod.rs:150-153`），唯一渲染的 `additional_context_message`（`:12270`）
   读的是未转换的 guest 路径 ⇒ 加载零个文件、静默无错。
   它由 create-flow §0 在 master step 1 单独提交、单独浸泡；verification 不再复制实现。
   ⚠️ **「必须独立提交、独立浸泡」的理由不能跟着 ownership 一起丢**：这个三行修复会给
   **整个移动平台**同时打开 memory 加载（嵌套 `@import` 展开、外部包含门、read-state 播种、
   每条首用户消息的新 token）。完整表述见 create-flow §0；本文只保留指针。
   （早期一次 ownership 迁移把这段警告从两份文档里同时删掉了——迁移是两个动作，
   附带约束必须跟着走，见顶部纪律二。）
   本文已核实**它不阻塞 Phase 1a**（工具名每轮经 `all_names()` 喂给模型、标注消息正文
   自带指令）；**Phase 4** 的提交路由依赖工作区合约真正到达模型，开工时要跑 upstream
   compatibility test 证明它已加载。

## 已知约束

1. **协议**：代码今天是 `7.0.0` / blessed 7；实施基线将是 create-flow §B bless 后的 `8.0.0`。
   **本项目不兼容旧客户端/旧引擎，两半必须同时发版**；Phase 3 必须先验证 create-flow
   contract anchors 仍在，再 bless rebase 后的实际 diff、重生成两端绑定并编译当前客户端。
   移动端无握手不是跳过这些步骤的理由。
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
6. **设备**：真机 rootfs 无 Node 工具链。
6a. **给 `AppManifest` 加字段**：`AppManifest::hash()`（`manifest.rs:460-465`）序列化整个
   结构体且绑定 SQLite schema ⇒ 新字段必须 `skip_serializing_if`，否则既有应用立刻
   `database manifest mismatch`。
6b. **给 `AppRecord` 加字段**：理由**不是** `AppManifest::hash()`——它不序列化 `AppRecord`。
   真实理由是这个仓库**没有迁移函数**（`storage.rs:34-36`：*serde aliases … IS the on-disk
   migration*），serde 对 `Option` 无隐式默认 ⇒ 裸字段让 `load_all` 整体 `Err`，
   **整个 app 库打不开**。必须 `#[serde(default, skip_serializing_if = "Option::is_none")]`，
   照抄 `init_session_id`（`types.rs:167-176`）。
   ⚠️ 这两条**规则相同、理由不同、适用的结构体不同**。早期版本把 6b 挂在 6a 的理由上，
   评审核出理由是假的、把规则一并删掉，险些造成整库读不出来。
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

- **阻塞项**：冒烟 spike 的六条验收是否能在真机上同时成立，以及
  `SmokeReport` 的结构化载体。未决前不计划阶段 2。
- **阻塞项**：source-vs-source 的进程级互斥/排队真相源。未决前不交付
  阶段 4 的提交路由。
- 冒烟门判据 4/5 的阈值：**本轮不需要**（已降为建议）。真要转阻塞时用真实应用标定。
- 副驾驶条（两态）与引导的具体形态：见该节警告，需单独一轮评审。

## 评审修正记录

六轮对抗式评审。第一轮 Codex 7 条 + 双评审；第二轮无命题扫描（11 条）；
第三轮 Codex 6 条 + 双评审；第四轮七个盲镜头 + 逐批打回 + 完整性/计划双批评（18 条）；
第五轮把冒烟门收束为 spike；第六轮修正数据通道与执行契约。

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

- **标注位置搬回 `workspace/.lingxi/annotations/`**，归属名改成 `annotation_id`。
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

### 第六轮（Codex 7 条回归 + 代码图门）

- 冒烟门仍保持 spike 未决，但候选 WebView 改为非零 viewport，删掉了
  `build_app` 专属测试和「必须把 app 推上屏」的旧结论；
- annotation 改为 `<annotation_id>/{annotation.json,image.jpg}` 的原子目录，
  disk-only 恢复能重建完整药丸，不只是找回一张无上下文的图；
- build 传播改为 `last_build_id + last_output_change_id`，补全
  `AppRecordDto`/lowering/两端 adapter/contract 工作，撤回「零协议工作」的错误结论；
- RootView 增加按 batch 去重的 one-shot flush，明确在 session transition 完成后
  恰好发送一次；
- 阶段 4 改为依赖 2 + 3，不再把尚未决定的 source-vs-source 竞态带进实现。

**六轮之后仍未验证的**（诚实列出，不是「大概没事」）：副驾驶条与引导两节没有任何镜头
读过；边界表 20 行里 17 行未被检查；**没有任何一轮跑过任何东西**——没有 `cargo test`、
没有 `xcodebuild`、没有真机，所有关于测试行为的判断都是静态阅读。

**2026-08-24 撤销**：光标可视化与操作回放条整体移出本设计，见「被撤销的需求」。
评审曾以「未经评审」为由建议砍掉这两节，那不是砍需求的正当理由（那是评审自己的覆盖
缺口）；实际撤销依据是用户澄清了它的用意——它属于 app-use / computer-use 的通用能力，
不该在本地应用里做一个半吊子版本。

### 第七轮（跨计划 rebase 与 create-flow 防漂移，2026-08-24）

- 协议实施基线从原始写作时的 7.0.0 改为 create-flow bless 后的 8.0.0；Phase 3 必须保留 mode/request/scaffolded contract anchors，并在 profile-aware 代码上决定下一次 bless。
- 新增「上游 create-flow 契约与防漂移门」：verification 明确消费 scaffolded、surface/profile、workspace/session、workflow 集合、pnpm/build 与 create protocol 六组 post-create seam。
- happy-path 测试禁止只靠旧手写 fixture，必须分别从 direct-create/profile 与 shell→host-confirmation→Scaffold 两条当前创建路径进入 verification。
- 状态表增加两个前置 guard：空壳不能进入 annotation 状态机；非法 surface/profile 组合必须报存储损坏，不能从源码猜测后继续。
- 共享补丁只留一个 owner：`LINGXI.md` resolver 属于 create-flow §0；canvas workflow lease/delete guard 与 `.lingxi` build-key 属于 verification Phase 1。
- 新增 `2026-08-24-local-app-implementation-order.md`，规定两份 design 的串行写入、spike 与协议 rebase 顺序；create-flow 以后改动 post-create seam 时，必须同一变更更新本文和 master order。
