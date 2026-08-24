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

这不是新建一条通道，而是承认既有通道**大部分**已经够用——**每个 app 本来就是一个
会话作用域**（`ConversationScope.localApp(id)`，cwd = `apps/<id>/workspace`），
scope 的 cwd、工具绑定、工作区都是真的。缺的是**入口**、**指哪儿**，以及下面那条
必须先修的前置。

⚠️ 本节早期版本还写了「`workspace/LINGXI.md` 自动注入且已含完整编辑契约」。
**那一句是假的**，见 §0。写入端确实写了那个文件（`local_apps_host.rs:3110-3244`），
但它从未到达模型。本设计不依赖它，直到 §0 修完。

## §0 阻塞前置：`LINGXI.md` 在移动端从未加载

**这是当前在线的缺陷，不是本设计新增的工作。必须先修、先有测试，再做后面任何一步。**

| 环节 | 事实 |
|---|---|
| `apps/engine-mobile/src/host.rs:3291-3293` | `model_cwd` 取 `mount.guest_path`；紧邻注释写着「engine-internal cwd（transcripts、.lingxi、**memory files**）stays host」——**意图是 host** |
| `host.rs:3329` | `SessionCwd::new(model_cwd, …)` ⇒ 装进去的是 **guest** 路径 |
| `tool-api/src/session_cwd.rs:63` | `cwd()` 原样返回存的 `PathBuf`，无任何转换 |
| `orchestrator/src/conversation.rs:12059-12075` | `build_system_prompt` **走了** `prompt_probe_cwd_resolver` 做 guest→host，注释明写「memory hierarchy… must read the HOST directory」 |
| `orchestrator/src/conversation.rs:12270` | `additional_context_message` **没走**：`self.memory.load(&self.session_cwd.cwd())` |
| `orchestrator/src/prompt/mod.rs:150-153` | R-P1c/R-P1d：LINGXI.md **不再拼进系统提示**，`additional_context_message` 是**唯一**渲染路径 |

形状：**做对了转换的那条路径已经不再渲染，唯一渲染的那条读的是没转换的 guest 路径。**
移动端 `memory.load()` 收到 `/workspace/local-app-<id>` 这种 guest 坐标，宿主文件系统
上不存在 ⇒ 加载零个文件 ⇒ memory block 为空，**静默无错**。

推翻尝试均失败：`SessionCwd::cwd()` 不返回 host 孪生；memory provider 不自己转换。
两条路径的不对称本身就是证据——若 provider 会转换，`build_system_prompt` 里那个
resolver 就不必存在。

修法：`additional_context_message` 走同一个 `prompt_probe_cwd_resolver`。测试必须
钉住「session_cwd 是 guest 路径时，渲染出的 claudeMd 非空」，而不是只测 resolver
自身——后者在桌面上恒等，测不出这个 bug。

**为什么它阻塞本设计**：整个「验收发生在对话里」的论证建立在「工作区合约每轮约束
代理」上。合约没送达时，agent 在 app scope 里既不知道构建工具名、也不知道 verify
工具清单，标注消息只能靠正文自带全部指令——那就不是「复用既有通道」，而是每轮重发
一份合约。

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

- Android 的 annotation overlay / 副驾驶条 UI（后补）；既有 agent-facing
  `inspect_ui` / `capture_ui` WebView contract 仍须两端一致
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
                                 RootView 关闭预览并切到目标 app scope
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

这里不能从 `LocalAppsStore` 直接调用当前 source 的 `SendPrompt`。协议刻意不让
`SendPrompt` 带 `session_id`；而预览页可能是从 global/project/另一个 app scope
打开的，直接发送会把修复交给错误工作区。提交走一个新的 RootView 回调：

```swift
onSubmitAnnotations(appID, initSessionID, batchID, prompt)
```

RootView 负责关闭 local-apps cover，再按以下规则路由：

1. 当前已经是同一个 `.localApp(appID)` scope：保留当前 session，等当前 turn 空闲后
   在当前 source 发送。
2. 当前是别的 scope 且 `source.model.streaming == true`：**只排队，不调用
   `switchScope`**。现有 `switchScope` 会先 `cancelAndWait()`；直接切会杀掉一个与此
   app 无关的 turn。像 create landing 一样，在 streaming 变 false 后重新尝试。
3. 目标 session 优先使用 `restoredSessionID(scope: .localApp(appID))` 保存的该 app
   最近活跃 session；没有最近值时才回退 `AppRecordDto.init_session_id`；两者都没有
   才 `startNew:true`。三个月后编辑不能被强制拉回最初的 setup conversation。

   ⚠️ **`restoredSessionID` 的返回类型是 `String` 不是 `String?`**，miss 时返回 `""`
   （`RootView.swift:1030-1039`，`?? ""` 在 `:1038`）。所以判空必须用 `.isEmpty`，
   不能用 `== nil`——否则 `switchScope` 同 scope 分支的 `if let resumeSessionID`
   对 `""` 为真（`RootView.swift:887-890`），会去 `requestSessionResume("")`。
   而且它**抹掉了底层刻意保留的区分**：`ProjectScopedPreferences.storedActiveSessionID`
   返回 `String?` 正是为了区分「没有这个 key」和「用户主动选了新会话所以存了空 id」
   （`ProjectScopedPreferences.swift:28-32`）。**曾点过「新会话」的用户**在这里会被读成
   「无最近值」而落回 `init_session_id`——正是本规则要避免的结果。判据必须读
   `storedActiveSessionID` 的可选值，不是 `restoredSessionID` 的塌陷值。

4. 然后调用 `switchScope(to: .localApp(appID), resumeSessionID: targetSessionID,
   startNew: targetSessionID == nil, initialPrompt: prompt)`。scope switch 正在进行时，
   像 `openCreatedAppSession` 一样保留 one-shot payload 并有界重试，不能因为一次
   `false` 丢掉 batch。

   🚨 **`initialPrompt` 这条路只在转录为空时才真的发出。** `RootView.swift:978` 是
   `if source.model.items.isEmpty`，而 `sessionResumed` 在同一个同步 handler 里就把
   `items` 填好了（`ConversationSource.swift:4401`/`:4418`）。**所以恢复一个已有的
   app 会话时，`initialPrompt` 被静默丢弃**——而规则 3 优先选的正是「最近活跃 session」，
   它几乎必然非空。`switchScope(initialPrompt:)` 因此**不能**作为本设计的提交通道。

   改为：`switchScope` 只负责切 scope（不传 `initialPrompt`），切换完成后由提交路径
   自己在新 source 上 `send(prompt)`，并**保留返回的 token**。

5. 🚨 **`send` 的返回值必须留住。** `send` 返回 `ConversationTurnToken?`，在五个守卫下
   返回 `nil`（`ConversationSource.swift:2488-2494`：`streaming` / `isCancelling` /
   `slashCommandPending` / `sessionTransitionPending` / `isSelectedAgentReadOnly`），
   而 `RootView.swift:979` 和 `:1008` 都用 `_ =` 把它丢了。**丢掉的正是
   `TurnStarted` 要回显的那个 correlator**（`clientTurnId`；引擎侧
   `host.rs:5471` `emit_turn_started(turn_id)` → `client-adapter/src/turn.rs:303-305`）。
   所以：`send` 返回 `nil` ⇒ 立即把 batch 标回 `stored(error，可重试)`；返回 token ⇒
   用它的 id 等 `TurnStarted` 再标 `submitted`。

6. 切换或发送失败时，batch 留在客户端并标红。**失败形态是「静默无操作，重试可成功但
   用户发现不了」**，不是数据丢失：`switchScope` 已经成功、`activeScope` 已是
   `.localApp(appID)`，标注仍在盘上和清单里，第二次点提交会走规则 1（纯 `source.send`）
   并成功。所以 UI 必须给出可见的失败态，否则用户只会看到「点了没反应」。

这条回调是提交路径的唯一入口。`LocalAppsStore.configure` 绑定的当前 source 只继续
服务普通库命令，不承担跨 scope prompt 路由。

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

每条标注从创建起有一个客户端生成的 `annotation_id`（UUID）。点击提交时把当时的
标注 ID 冻结成一个 `batch_id`；之后新增的标注进入下一批，不会被当前修复的构建
误清。未成功落盘的标注不进入 prompt，也不进入 `submitted` batch。

## 协议改动

`ResolveAppUiRequest.result_json` 和 `AppUiRequestDto.value` **都是不透明的
`Option<String>`**（`commands.rs:438`、`local_apps.rs:774`），所以绝大部分
改动不触及协议：

| 能力 | 承载 | 动协议 |
|---|---|---|
| `inspect_ui` 返回元素几何 / canvas rect / runtimeErrors | `result_json` 内的 JSON | 否 |
| `capture_ui` 区域裁剪 | host schema 序列化到 `value` JSON | 否 |
| 截图上的光标标记 | 客户端绘制；`result_json` 加 `last_action` | 否 |
| 操作回放条 | 纯客户端，源自既有 `AppUiRequest` | 否 |
| 标注 → 消息 | 复用 `SendPrompt` | 否 |
| **UI 失败的机器可读错误码** | `ResolveAppUiRequest.error_code` | **是（additive）** |
| **存标注** | 新命令 + `AppEventDto` 回执 | **是** |
| workflow 活动关联 app | `AppWorkflowTaskChanged` + snapshot | **是（additive）** |
| 每次成功构建的 ID | `AppEventDto::AppBuildSucceeded` + `AppDetailsDto` 带最新 build_id | **是（additive）** |

⛔ 早期版本在这张表上方写「所以绝大部分改动不触及协议」并据此把阶段 1 标成「无 DTO
改动」。**那个结论已被 `error_code` 推翻**：`kind` 的判别没有它就只能字符串匹配翻译文案
（详见冒烟门一节）。现在是**五处不触及、四处触及**，且其中 `error_code` 落在阶段 1。

### 唯一的新命令

```rust
ClientCommand::StoreAppAnnotation {
    request_id: String,     // 一次命令/回执 correlator
    app_id: String,
    annotation_id: String,  // 客户端 UUID，贯穿清单与 batch
    rect: String,          // "x,y,w,h"，CSS 像素
    viewport: String,      // JSON: width/height/offset/scale
    hit_elements_json: String, // 有界 JSON 数组；引擎解析、校验后落盘
    note: String,
    image_base64: String,  // 区域 JPEG
}
// → ClientEvent::AppEvent {
//      event: AppEventDto::AppAnnotationStored {
//        request_id, app_id, annotation_id, path: Option<String>, error: Option<String>
//      }
//    }
```

引擎写入 `apps/<id>/workspace/.lingxi/annotations/<ts>-<n>.jpg` 与同名
`.json`（含 rect / note / 命中元素 / 视口尺寸），回**工作区相对路径**。
`request_id` 和 `annotation_id` 必须原样回显，因此同一 app 的 N 个写入即使乱序完成，
客户端仍能把 path/error 归到正确药丸。`hit_elements_json` 最多 200 项、每个字段沿用
`inspect_ui` 的既有长度上限；引擎拒绝非法 JSON、非有限/负尺寸矩形和越界 viewport，
而不是把客户端字符串原样写进元数据。

`AppAnnotationStored` 放在 `AppEventDto`，不新增顶层 `ClientEvent`——但**不要用
「元数据快到上限了」当理由**：实测量过，`AppEventDto` 占 15.8%、`ClientEvent` 占
28.7%，这条约束目前**不成立**。放进 `AppEventDto` 的真实理由是它就是 app 生命周期
事件的既有信封（`local_apps.rs:835-836`），客户端已有单一 `case let .appEvent(event)`
分发点（`LocalAppsStore.swift:267`），新增顶层变体会多一条平行路径而无任何收益。

🚨 **必须有「无回执」状态。** `ClientCommand` 是 `#[non_exhaustive]`，而
`EngineMobileHost::submit` 的兜底臂在 `tracing::debug!` 之后返回 `Ok(())`
（`host.rs:7064-7074`）。移动端**没有协议握手**，所以任何版本偏斜或漏写的 match 臂
都会让 `StoreAppAnnotation` **被接受并丢弃**：没有 `AppAnnotationStored`、没有错误、
没有超时 ⇒ 所有标注永远停在 `draft`，「开始修这 N 个问题」点了没反应，任何地方都
没有诊断。

本文的已知约束第 7 条已经写了这个陷阱，早期版本却没把它应用到自己的新命令上。修法：
客户端对每个 `request_id` 设 8 秒超时，超时即把该条标注置为
`draft(error: "no_receipt")` 并在 UI 上可见；状态机因此多一条 `draft --超时--> draft(error)`
边。**乱序回执与超时是两回事**：前者靠 `request_id + annotation_id` 归并，后者靠
per-request 计时器。

（另：早期版本用「N 条并发提交乱序完成」论证 correlator 的必要性。那个前提未经代码
证实——`LocalAppsStore` 是 `@MainActor`、`send` 也是 MainActor 隔离，自然写法
`for … { await send(…) }` 是严格顺序的，而既有的 `ExecuteAppBridgeRequest` 更是
inline await（`host.rs:6789-6791`）。correlator 仍然必须有，理由是**回执经由单一共享
事件流回来**且必须能归到正确药丸，不是因为已证实会乱序。）

### workflow 与构建事件

延迟提交不能监听 `AppWorkflowChanged`。它在第一次 `LocalAppBuild` 把 app 从 draft
标成 ready 时就发出，早于 runtime/冒烟/repair；ready app 后续 rebuild 也不再发。

不要修改既有 `TaskStatusChanged` variant 的字段形状；UniFFI 生成的 mobile constructor
会跟着变化，而移动端没有握手。追加一个 local-app event：

```rust
struct AppWorkflowTaskDto {
    task_id: String,
    workflow_name: String,
    status: TaskStatusDto,
}

AppEventDto::AppWorkflowTaskChanged {
    app_id: String,
    task: AppWorkflowTaskDto, // running | completed | failed | killed
}

AppEventDto::AppWorkflowTasksSnapshot {
    app_id: String,
    tasks: Vec<AppWorkflowTaskDto>, // 仅 nonterminal
}
```

`MobileWorkflowStatusSink` 继续原样发 `TaskStatusChanged`，并额外发
`AppWorkflowTaskChanged`；仅两个内置 local-app workflow、在 running 与 terminal 发。

数据可达性已核实（早期评审曾断言「sink 拿不到 app_id / 分不出 workflow 种类」，**不成立**）：
`WorkflowCheckpoint` 带 `workflow_id` 与 `args_json`，launcher 对**每次**启动都写
（`workflow_support.rs:19-30`、`:889-903`），`workflow_id` 正是那两处硬编码判的同一个值；
`bind`（`:444`）本来就收到 `host.rs:3846-3847` 传进来的**具体** `Arc<TaskRegistry>`。

⚠️ 但**不是**「每个 task 直接握着这两样」：`task_runs`（`workflow_support.rs:61`）是
`HashMap<String, (String, String)>`，只有 session_uuid 和 run_id；`workflow_id`/`args_json`
在盘上的 `adopt.json` 里。所以落地方式是**把 `task_runs` 的元组加宽**，带上 `workflow_id`
和从 `args_json` 解析出的 `app_id`，两个写入点（`:104-107` 与 `:393-396`）都拿得到。

🚨 **顺序陷阱**：`set_status` 在 `:562` 先 `checkpoints.remove_task(task_id)`，`:564` 才
`emit_status_for_owner`（`:535` 同样）。新的 emit **必须复用 `:556` 处已经取出的元组**，
不能在 remove 之后重新查——否则 terminal 事件恒缺 app_id，而这正是排队守卫等的那一条。客户端维护
`activeWorkflowTaskIDsByApp: [AppID: Set<TaskID>]`，提交时若集合非空就排队；同 app
新启动的 workflow 也加入等待。集合回到空且等待期内所有 terminal 都是 completed 才
具备发送资格；还要等当前 source `streaming == false` 才走 RootView 路由。任一
failed/killed 则保留并显示重试。

running event 可能在客户端重连或打开详情页前已经发生，因此 event delta 不是唯一
真相。`GetAppDetails` 和 engine reconnect 都从 task registry 查询该 app 的 nonterminal
local-app workflows，并额外发 `AppWorkflowTasksSnapshot`；客户端收到后替换该 app 的
active set，再继续消费 changed delta。

🚨 **但 registry 的两条读路径都带会话过滤，照字面实现这个 snapshot 会在设备上返回空。**
`tasks/src/handle.rs:328`（`list`）和 `:354`（`list_workflows`）都调
`workflow_visible_in_current_session`，它在 `tasks/src/registry.rs:708-711` 只保留
`workflow.session_uuid == current_session` 的 `LocalWorkflow`。而移动端**每次换会话都会
重指这个过滤器**：`apps/engine-mobile/src/host.rs:5086-5088` 在 `retarget_session_writer`
里，由 ResumeSession（`:5267`、`:5341`）、NewSession（`:6670`）、ClearSession（`:6576`）
到达。

失败场景（本设计自己会触发）：构建 workflow 从 setup 会话 A 启动、仍在跑；提交路径
按上面的规则 4 调 `switchScope(… resumeSessionID:/startNew:)` ⇒ 引擎 retarget 到会话 B
⇒ 过滤器 = B ⇒ snapshot 查询对那个**仍在运行**的构建返回**零行** ⇒ 客户端把 active set
替换成 `{}`、判定构建已结束，**把 queued batch 发进一个正在跑的构建**。
`MobileWorkflowCheckpointStore::adopt_session` 救不了这个——它只收养已经属于**新**会话的
checkpoint（`workflow_support.rs:322-382`）。

更糟的是**这个缺陷会让本文自己的测试变绿**：`workflow_session_filter` 默认是 `None`
（`registry.rs:167`），此时 `registry.rs:705-707` 对一切返回 `true`。所以单测里
snapshot 恢复得好好的，真机上 100% 失效——和本文列为「历史翻车」的那些形状完全一致。

修法比想象的简单：**已经有一个绕过会话过滤的查询存在**——
`TaskRegistry::find_nonterminal_local_app_workflows(app_id)`（`registry.rs:789-813`）
直接读 `self.tasks`，**不经 `workflow_visible_in_current_session`**。它今天被删除守卫
用着（`host.rs:5941`）。snapshot 复用它即可，不必新造绕过。它现在只返回 task_id，
需要加宽成 `(task_id, workflow_id, status)` 以填 `AppWorkflowTaskDto`。

🚨 **但它的谓词硬编码了 `workflow.workflow_id == "local-app-build"`（`registry.rs:796`），
排除了 `local-canvas-build`。这是一个独立的在线 bug，不是本设计引入的**：该函数是
**删除 app 前的守卫**（`host.rs:5938-5952`：这个查询 **或** 活跃 lease），而 lease 那道
门同样硬编码 `local-app-build`（`local_workflow.rs:1987`）⇒ **一个 canvas 应用可以在
构建进行中被删掉**。两处必须一起改成集合判定。

`tool_workflow::LOCAL_APP_BUILD_WORKFLOWS`（`tools/workflow/src/lib.rs:239`）已经**就是**
这个集合，但它是私有的，且 `tasks/Cargo.toml` 依赖的是 `workflow` 而非 `tool_workflow`，
所以要么 `pub` 出来并加依赖边，要么提到两边都看得见的 crate。

测试必须**显式设置** `workflow_session_filter`（而不是留默认 `None`——默认下
`registry.rs:705-707` 对一切返回 `true`，测试会假绿），并覆盖**两个**内置 workflow。

`TaskStatusChanged` 现有的 active-origin filter 保持不变；`AppWorkflowTaskChanged` **不能**
套这个 filter。用户可能在构建时切 scope/打开 app，若 terminal 随旧 conversation
owner 被抑制，queued batch 会永远卡住。app workflow activity 走全局 local-app event
sink，按 `(task_id,status)` 去重。

另外，每次 `LocalAppBuild` 真正发布了新的 servable output 后都发：

```rust
AppEventDto::AppBuildSucceeded {
    app_id: String,
    build_id: String,       // 每次成功调用唯一，即使复用相同 output
    output_digest: String,  // build provenance/output digest
}
```

这不是 workflow 状态；普通对话里的修复 build 也会发。客户端只清除已经
`submitted`、且事件 `build_id != batch.baseline_build_id` 的那些 annotation IDs。
`baseline_build_id` 在收到该 prompt 的 `TurnStarted` 时从 per-app 最新 build ID 取值；
新建、落盘失败、排队未发和后续新增的标注都不受影响。

### `build_id` 必须是持久化的 UUID，落在 `AppRecord` 上

`baseline_build_id` 目前无处可取，而且三条显而易见的替代方案都不行：

- **不能放进 build provenance。** `BuildProvenance` 只有 `{version, buildKey, outputSha256}`
  （`local_apps_build.rs:42-49`），而 `build.json` 就在 `build/store` 目录里
  （`:865-867`），`promote_build_root` **把整个目录 rename 走**再把新树换进来
  （`:1136-1140`）⇒ 每次 promote 都先销毁再于下一条语句重写（`:663`→`:664`），
  中间有一个**活着的可服务产物没有 id** 的窗口。且 `write_build_provenance`
  （`:896-925`）只有 temp+rename、**无 fsync**。
- **不能用 `buildKey`/`outputSha256`。** 两者都是内容派生的，相同源码重建后不变——
  满足不了「每次成功调用唯一，即使复用相同 output」。
- 🚨 **绝不能用进程内计数器。** `RootView.swift:915/922` **每次切 scope 都新建引擎并替换**，
  而本设计的提交路径自己就会切 scope ⇒ 新引擎重新发 `build-1`，从旧引擎取的基线
  **永远比较相等**，清理一次都不会触发。
- 另：**cache-hit 路径在写任何 provenance 之前就 return 了**（`:642-644`），而
  `build_app` 仍视其为成功并 `mark_ready` ⇒ 存在「一次成功构建没碰任何持久化字节」。

落地：给 `AppRecord`（`local-apps/src/types.rs:142`）加
`#[serde(default, skip_serializing_if = "Option::is_none")] pub last_build_id: Option<String>`，
**照抄 `init_session_id` 的 additive 形状**（`types.rs:170-176`）——`skip_serializing_if`
是硬要求，否则 `AppManifest::hash()` 一变，每个既有应用立刻 `database manifest mismatch`。
在 `AppService` 里 `mark_ready` 旁边加一个走同一 `with_app(app_id, |app, now| …)` 闭包的
mutator（拿到原子持久化 + 事件顺序锁），但**不能像 `mark_ready` 那样在「没变化」时提前
返回**（`service.rs:721-723`）——每次都铸一个新的 v4 UUID。调用点在
`LocalAppsHostBroker::build_app` 的 `served_index.exists()` 检查**之后**
（`local_apps_host.rs:3477-3487`），那是唯一把「成功构建」定义成「可服务」的地方，
且能覆盖 cache-hit 提前返回。

`AppDetailsDto` 完全由持久化状态组装（`host.rs:5607-5622` → `lower_details`
（`local_apps_bridge.rs:441-452`）），**没有内存通道**，所以不落盘的 id 根本进不去。

⚠️ **`AppBuildSucceeded` 不带会话/轮次溯源**，而排队守卫看的是只覆盖两个内置 workflow
的 `AppWorkflowTaskChanged`，清理器却对**普通对话里的 build** 也会触发。这个不对称是
有意的（修复 build 正是普通对话里发生的），但意味着：同一 app 上一个后台任务的 build
会清掉一个与它无关的 batch。可接受，因为清理只作用于**已 submitted** 的条目，最坏
后果是清早了一点，不丢草稿。
（早期版本还担心「另一个客户端的 build」——那条不成立：`RootView.makeSource` 每次切
scope 新建引擎并在 `:929` 替换旧的，fanout 按设计弱持有观察者
（`local-apps/src/events.rs:80-84`），移动端同 profile 无第二个进程。一个进程一个 host。）

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
（`local_apps.rs:726-742` 有记录）。既有 pointer/key 的 wire spelling 保持逗号字符串；
capture 的新增结构序列化成现有 `value` 里的 JSON，不改 action enum。

### bless 四步（command 与 event 必须一起做）

1. `current_contract_index()` 补 command、`AppWorkflowTaskDto`、四个 `AppEventDto` 变体，
   **以及 `ResolveAppUiRequest.error_code` 和 `AppRecordDto/AppDetailsDto` 的 build id**
   的 `put(...)`。不能只补 AppUi/command 段。

   🚨 **这两个新字段的失败模式完全相反，必须分别对待：**

   - **`error_code` 是全静默的**——漏掉索引条目，四道门全绿：
     `current_contract_matches_index_or_version_bumped`（`version_guard_test.rs:1710`）
     只 diff 手写索引与 golden，两边都没变；`contract_index_covers_every_dto`
     （`:1796`）只构造 **10 个** `ClientCommand` 变体而
     **`ResolveAppUiRequest` 不在其中**，所以没有编译中断；
     `snapshots/command/resolve_app_ui_request.json` 里 `None` 配
     `skip_serializing_if` 序列化后**逐字节相同**，snapshot 也绿。
     ⇒ **必须手动加索引条目 + 一个带 `error_code` 的失败态 golden**，没有任何自动化会提醒。
   - **build id 不是静默的**：`version_guard_test.rs:2203` 构造穷尽的
     `AppDetailsDto { app, manifest, runtime, checkpoints }` 字面量，加字段就是**硬编译错误**。

   这正是已知约束第 9 条（契约索引是手写的 ⇒ 不加条目静默放行）的一次实例化——
   而它这次只对两个字段中的一个成立。
2. `snapshots/command/` 加 `StoreAppAnnotation` golden；`snapshots/event/` 加
   `AppAnnotationStored`、`AppWorkflowTaskChanged`、`AppWorkflowTasksSnapshot`、
   `AppBuildSucceeded` golden，
   并全部加入 `snapshot_test.rs` 表项。
3. `clients/shared/src/protocol.ts` 同步 TS command union、`ClientEvent`、
   `AppEventDto` **与手写运行时守卫**
   （`clients/shared/test/snapshots.test.ts:1324, 1351, 1374, 1384`），并跑
   `clients/shared` 自己的完整 `npm test`。
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

同时把原来的 `canvasCount` 扩成 `canvases: [{rect}]`（最多 64 个）并保留
`canvasCount` 兼容字段；增加 `documentState`、`viewport`（含
`offsetLeft/offsetTop/scale`）与上节的 `runtimeErrors`。canvas rect 是宿主只比较
画布像素、避免被外围 DOM 动画骗过的依据。

### 🚨 256 KiB 是硬失败，本节新增的字段必须被预算住

`LocalAppWebView.swift:407-411` 在 `resultJSON.utf8.count > 256 * 1_024` 时返回
`.failure(local_apps_error_ui_invalid_result)`——**它不截断，它失败**。既有最坏情况
本来就不宽裕：`clean()` 每串截到 500 字符（`:579`），`snapshot()` 最多 200 个元素、
每个带 `elementId`/`role`/`name`/`value`（`:674-686`）。

早期版本估的增量是「约 200 × 4 个整数」（约 5 KB）——**漏掉了最大的一块**：
`runtimeErrors` 32 条、`message`/`source` 各沿用 500 字符上限 ⇒ 最坏约 33 KB，
是估值的十倍。加上 `canvases`/`viewport`/`documentState` 后总增量约 40 KB。

**后果是自指的**：文本密集的 Ionic 应用 + 一个在抛异常的页面会把载荷推过 256 KiB，
`inspect_ui` 返回一个既不是「未挂载」也不是超时的错误 ⇒ 不属于
`infrastructure_unavailable` ⇒ 阻塞判据 2 失败 ⇒ 对一个健康的 app 烧掉一轮修复，
第二次再失败就交付 `needs_user_review`。**app 抛的异常越多，判据 6 越可能因为超限而
根本观测不到——这个账本会打败它自己服务的那条判据。**

因此定死预算，且预算必须在**客户端组装时**执行而不是事后检查：

| 段 | 上限 |
|---|---|
| `runtimeErrors` | 8 条（不是 32），`message` 200 字符、`source` 120 字符，超出只留计数 `runtimeErrorsDropped` |
| `canvases` | 16 条（不是 64）；超出只留 `canvasCount` |
| `elements` | 200 条不变，但**新增的 `rect` 不受 500 字符规则影响**，4 个整数 |

组装完成后若仍超 200 KiB（留 56 KiB 余量给 JSON 信封与 base64 之外的字段），
按 `elements` → `canvases` → `runtimeErrors` 的顺序逐段降级并在结果里写明
`truncated: [<段名>]`。**宿主冒烟门看到 `truncated` 含 `runtimeErrors` 时，判据 6 必须
判为 `infrastructure_unavailable` 而不是通过**——观测不到不等于没有异常。

### `capture_ui` 区域裁剪

不要再给 `value` 发明逗号与 `clean` 混排的第二套语法。对模型暴露的
`capture_ui` schema additive 增加：

```json
{
  "app_id": "…",
  "rect": {"x": 10, "y": 20, "width": 120, "height": 80},
  "marker": "last_action" // 或 "none"；默认 last_action
}
```

host 校验有限数字/正尺寸后，把它序列化进既有 `AppUiRequestDto.value` 的 JSON 字符串；
因此 DTO 不变。iOS 侧解析后，把 CSS rect 转回 view rect，再设置
`WKSnapshotConfiguration.rect`。Android 在
`PixelCopy`/fallback 得到 bitmap 后按同一 CSS→view 换算裁剪。无 `rect` 时仍为
整帧；`marker:"none"` 明确表示干净图。

🚨 **不能直接沿用既有的长边上限计算，它会把裁剪区放大。** `LocalAppWebView.swift:482-488`
的 `snapshotWidth` 是从**整个 view 的 bounds** 推的，不是从 `configuration.rect`：
`longEdge = max(bounds.width, bounds.height)`，`snapshotWidth = bounds.width * (targetPointEdge / longEdge)`。

算一遍：393×852 pt 的 view、3× 屏。`targetPointEdge = 341.3`，`snapshotWidth = 157.4` pt。
此时把 `configuration.rect` 设成本文自己的示例区域（118×74）——WebKit 会把这块
**放大到 157.4 pt 宽**，即 118 pt 的内容被拉成约 472 px，1.33× 的模糊放大，还把
170 KiB 的 JPEG 预算花在放大出来的像素上（`:510`），而「长边 1024 px」这个上限
**从未真正作用于裁剪区**。

所以裁剪路径要自己算：`snapshotWidth` 从 `rect.width` 推，长边上限对 `rect` 生效，
质量阶梯 `[0.7, 0.5, 0.3]` 与 170 KiB 预算沿用。裁剪区小于目标尺寸时**不放大**
（`snapshotWidth = min(rect.width, cap)`）。

越界钳制发生在持有真实 viewport 的客户端；host 只做形状/数值校验。完全在
viewport 外返回错误，不能静默改成整帧。

### 运行时错误账本

`read_logs(log="runtime")` 当前只是读文件，文件不存在还会返回空 tail，不能证明
页面没有异常。两端 WebView 在 document-start bootstrap 安装 `error` 与
`unhandledrejection` listener，保存最近 32 条有界记录：

```json
{"message":"…","source":"…","line":12,"column":4,"at_ms":1787543300}
```

不记录 rejection value 的任意对象展开，只取安全字符串；每字段沿用 500 字符上限。
`inspect_ui.result_json` 增加 `runtimeErrors`，宿主冒烟门读这个账本。页面 reload 时
清空；同一 document 内重复 inspect 不清空。`read_logs` 继续用于 build/runtime
server 诊断，但不再承担浏览器异常判据。

### 光标标记

客户端在 `captureFrame` 里画标记圈，再编码 JPEG：iOS 用
`UIGraphicsImageRenderer`，Android 在 PixelCopy/fallback bitmap 上用平台 `Canvas`。
**仓库中没有任何服务端图像绘制库**——`imageproc` /
`tiny-skia` / `resvg` / `ab_glyph` 都不是依赖，`image` 只用于
decode/encode/resize 和一处 `crop_imm`——所以绘制必须在客户端。

标记位置来自**紧邻的上一次带坐标动作**（客户端本地记住 `{kind, x, y, at}`）。
仅当该动作发生在 5 秒内才绘制。`result_json` 增加
`last_action: {kind, x, y, age_ms}`，让 agent 知道图上那个圈是什么。
`marker:"none"` 时不绘制。

同一个标记帧进操作回放条。**这满足「回喂给 LLM」**：agent 请求截图时拿到的
就是带标记的那张，能看见「我点的地方和我以为的地方不一样」。

### 冒烟门：专用 host primitive

替换 workflow 里由 agent 自述的 gate。构建完成后**由宿主发起**一组固定调用，
agent 不参与：

1. 运行时已启动且 `preview_url` 非空
2. `inspect_ui` 成功，document 为 interactive/complete、viewport 非零；DOM 应用
   不再要求 `elements` 非空（纯阅读页可以没有交互控件），canvas 应用要求
   `canvases` 至少一个且 rect 非零
3. `capture_ui` 取两帧，间隔 1.5 秒
4. 首帧非纯色 —— 判据是**整帧像素方差低于阈值**，即「整屏只有一个颜色」。
   这是白屏/黑屏的形状；一个真实设计即使极简也有文字与边框，不会命中。
   若快照同时有可见文本/图片/canvas 结构证据，本条降为 warning；否则阻塞
5. canvas 应用：只比较 `canvases[].rect` 覆盖的像素，两帧必须不同；不能让 DOM
   spinner、光标或回放 UI 的变化替冻结 canvas 过关

   🚨 **门的两次 capture 必须显式传 `marker: "none"`。** schema 默认是
   `last_action`，而标记只在「上一次带坐标动作发生在 5 秒内」时绘制。门自己只发
   inspect 和 capture、**不发任何坐标动作**，所以那个「上一次动作」来自更早的 agent
   轮次：若它发生在 T，门在 T+4s 与 T+5.5s 取两帧 ⇒ **第一帧有标记、第二帧没有**。
   标记是客户端画在合成快照之上的，可以落在 canvas 矩形内 ⇒ 判据 5 看到「变化」⇒
   **冻结的 canvas 通过**。这正是本判据存在的意义所要挡住的东西。

   推广成规则：**任何程序化的像素比对都必须走 `marker:"none"`。** 标记是给人和给
   模型看的证据，不是给比较器看的数据。
6. `inspect_ui.runtimeErrors` 为空

无 blocking finding → 构建成功。任一阻塞判据不过 → **一次**修复轮；再不过 →
**带诊断把 app 交给用户**，而不是像现在这样 `throw` 掉整个构建结果
（`local_app_workflow_core.js:351-360`）。

这六条**没有一条经过模型**：宿主发的调用、宿主读的返回、宿主做的判断。
agent 无法「声称」自己看过。这正面回应了 `local_app_build_workflow.js:111`
自己记下的 TODO。

执行 seam 固定如下，不能让 executor 现场选择：

- workflow runtime 增加一个窄 primitive：`localAppSmoke(appId)`；不增加 generic
  tool-calling primitive。

  实现约束（不是可选项）：
  - **必须是一个新 global，不能路由进 agent 通道。** 所有经
    `local_workflow.rs:1526-1546` 的调用都拿到 journal 链式 key，命中即返回
    `Plan::Cached` 从磁盘重放（resume 时加载，`:2216-2219`）；只有 `__wf_resolve`
    通过 `:1492` 的 early `continue` 豁免。而 `local_app_workflow_core.js:155`
    **主动教用户用 `resumeFromRunId` 重试** ⇒ 一次 resume 会重放一份陈旧
    `SmokeReport`，对宿主本轮根本没看过的 app 报 `ready`。**那正是本机制存在的意义
    所要消灭的失败模式。** 新 global 与 journal 无任何交互，天然免疫；若实现时改走
    agent 通道，必须同时拿到 `__wf_resolve` 那样的豁免。
  - **只能是同步阻塞调用。** 宿主 global 全部是同步 `rquickjs::Function::new` 闭包
    （`workflow/src/lib.rs:897-1000`），**没有 Rust future ↔ JS promise 桥**
    （`workflow/Cargo.toml` 因 MSRV pin 显式关掉 `futures`）。同步→异步的接缝已存在：
    handler 已在专用 `std::thread` 上跑 `run_with_progress` 并用
    `blocking_send`/`blocking_recv`（`local_workflow.rs:11-31`、`:1413-1453`）。
  - `check_determinism` 不会拦新 global——它只标 `Date.now`/`Math.random`/裸
    `new Date()`（`workflow/src/lib.rs:426-456`）。
  - 像素判定要 `image` crate。`tool-api/Cargo.toml:41` 无条件带它（含 `jpeg`），
    但 **`tool-api` 不 re-export `image`**（`grep "pub use image"` 零命中），
    所以 engine-mobile 要加自己的直接依赖边。
- `LocalWorkflowHandler` 注入 `Arc<dyn LocalAppSmokeGate>`。仅内置
  `local-app-build` / `local-canvas-build` 可调用；`appId` 必须等于该 run 的
  `args.app_id` 且匹配 workspace lease，否则拒绝。
- trait 与默认 `UnavailableLocalAppSmokeGate` 放在 `tasks`，避免 tasks 反向依赖
  engine-mobile。依赖方向已核实：`tasks/Cargo.toml` 无 engine-mobile 边，
  `apps/engine-mobile/Cargo.toml:172` 有 `tasks = { path = "../../tasks", optional = true }`。
  先例也在：`NoopStatusSink` / `TaskStatusSink`（`local_workflow.rs:59` 再导出）。

  🚨 **绑定顺序按字面写法编不过。** `LocalWorkflowHandler::new` 在
  `host.rs:3520`（位于 `build_mobile_inner_with_ask`，`:2329`），而 local-apps host 是在
  `build_mobile_engine_inner`（`:9186`）里产出的（`:9297` 或 fallback broker `:9309`），
  **而后者 `:9223` 才去调前者**。broker 严格晚于 handler 存在。必须用为这个问题发明的
  延迟单元：`host.rs:3506` 的 `DeferredToolInvoker`（注释：*"filled with the real
  `RegistryToolInvoker` once `tools` exists"*）或 `:3529` 的 `with_output_pool_cell`
  那种 `OnceLock`。先例齐备，早期版本只是没点明这一步。

  ⚠️ **engine-desktop 的 `verification_unavailable` 只有编译期那一半是真的。**
  `grep -rn "local_apps" apps/engine-desktop/src/` **零命中**——local-app 工具只在
  `engine-mobile/src/host.rs:3734` 注册。桌面上 Generate & Build 代理连 `LocalAppBuild`
  都没有，run 会在 `requirePreviewOnSuccess` 就 throw，根本走不到门。绑定 unavailable
  实现的价值是**让共享 handler 编译**，不是「桌面会得到一个诚实的 unavailable 结果」。
  不要为这条路径写行为测试。
- engine-mobile 的实现直接调用 `LocalAppsHost` 的 runtime/inspect/capture 请求并做
  像素判定，不走 model tool result，也不经过 `LocalAppCaptureUi` 的 agent permission
  gate。用户主动启动这个、且 lease 已绑定精确 app 的 build workflow，是这次只读
  冒烟检查的授权边界；图片只在内存中参与判定，不写 conversation history。
- primitive 返回有界
  `SmokeReport {status, findings:[{kind,code,evidence}], warnings, metrics}`，其中
  `kind` 只能是 `source_defect` 或 `infrastructure_unavailable`；模型只能在 repair
  prompt 中消费 findings，不能写或覆盖 report。
- JS 在 Generate & Build 后调用一次。失败且尚有 repair budget 时，以 report 生成
  一次 repair agent prompt，rebuild/restart 后再调用一次。**只有**
  `source_defect` 触发 repair；WebView 未挂载、app 退到后台、UI request timeout 属于
  `infrastructure_unavailable`，直接返回 `delivery_status:"verification_unavailable"`，
  不能让模型改一份没有证据表明出错的源码。最后仍有 source defect 时不 throw：
  app 已经 servable，workflow 返回 `delivery_status:"needs_user_review"` 和诊断；
  `LocalAppSmokeGate` 每次调用都把自己的最终 report 原子覆盖写到
  `workspace/.lingxi/smoke-report.json`，不是让 JS/agent 转写。构建/运行时本身失败
  仍然 throw。
- smoke UI request 使用 20 秒单次、45 秒整门 deadline，不沿用普通 agent UI path 的
  2 分钟 × N 最坏等待。deadline 到达后取消未决请求并清理**宿主侧** `pending_ui`
  （`local_apps_host.rs:2115-2119`）。注意客户端 registry 自己在 **10 秒**就放弃并
  返回 failure（`LocalAppWebView.swift:109-115`），所以「未挂载」这一类根本走不到
  20 秒；20 秒只对「已挂载但页面不响应」有意义。

- 🚨 **`kind` 的判别需要一个机器可读的错误码，这是本设计的第四处协议改动。**
  `ResolveAppUiRequest { request_id, decision, result_json, error: Option<String> }`
  （`commands.rs:431-442`）**没有错误码**，而 spec 归类为 infra 的那个确切失败
  （「WebView 未挂载」）回来的是**本地化字符串**——iOS
  `String(localized: "local_apps_error_ui_not_open")`（`LocalAppWebView.swift:115`，
  5 个语种都有），Android 是它自己硬编码的英文（`LocalAppWebView.kt:192`）；宿主侧
  `request_ui` 把一切塌成 `Err(String)`（`local_apps_host.rs:2126-2140`）。
  **靠字符串匹配一个会被翻译的 catalog 值来决定要不要派修复代理，是不可接受的。**

  仓库里已经解决过同一件事：`AppBridgeResponseDto.error_code`
  （`local_apps.rs:479-483`），注释原话 *"Stable machine-readable failure code … so
  page code can branch without parsing `error` prose."* 照抄这个形状，给
  `ResolveAppUiRequest` 加 `error_code: Option<String>`，两端在既有失败点填稳定码
  （`ui_not_open` / `ui_timeout` / `ui_result_too_large` / `ui_invalid_result` / …）。

  **这条推翻了两处早期结论**：协议表里的「绝大部分改动不触及协议」要改成
  「五处不触及、四处触及」（见协议一节的表）；阶段 1 不再是「无 DTO 改动」，
  bless 必须进阶段 1。

- 🚨 **冒烟门的 UI 请求会把用户的屏幕全屏劫持。** 每条 `AppUiRequest` 都设
  `requestedPresentationAppID`（`LocalAppsStore.swift:971`），`RootView.swift:309-313`
  据此 `navigation.openLocalApps(appID:)` → `presentedRoute` 驱动 `.fullScreenCover`
  （`RootView.swift:411-418`）。一次构建至少 3 条请求（1 inspect + 2 capture，有修复轮
  ×2），**每条都往用户正在做的事情上盖一层全屏**，而且没有任何东西负责关掉门打开的
  那层 cover。这与本文「流水线在后台跑」的前提直接冲突，也与提交路径（规定要**关掉**
  这层 cover）互相打架。

  ⛔ **早期版本在这里决定「走一条不触发 presentation 的静默旁路」。那是错的，已撤回。**

  **presentation 就是唯一挂载 WebView 的机制。** controller 只在
  `LocalAppPreviewView` / `LocalAppEmbeddedPreview` 真的挂了 `LocalAppWebView` 之后
  才注册，而它只在 `previewURL != nil` **且该视图在屏上**时挂载
  （`LocalAppDetailView.swift:586`）。把 app 推上屏的正是
  `requestedPresentationAppID` → `RootView.swift:309-313`。抑制掉它 ⇒ 没有 controller
  ⇒ `ui_not_open` ⇒ **主流程（新建）下冒烟门 100% 跑不起来**。

  这是同一个错误的第三次（前两次：删 `getDetails`、以为 Verify 阶段是纯成本）：
  **把一个看起来只有成本的机制删掉，而它正是让功能能跑的那一环。**

  **决定：presentation 保留，不加旁路，也不做 origin 推断。** 理由不是妥协，而是它在
  本设计的流程里本来就正确：每一次构建要么是（a）用户刚要求创建这个 app，要么是
  （b）用户刚提交标注要求修它——**两种情况下用户都正在等这个 app**，被带到它面前是
  预期行为，不是劫持。配上副驾驶条，用户看到的是「门正在检查我的 app」，这是本设计
  想要的效果。

  ⚠️ 同时撤回「origin 信号」那套：客户端只收到 `AppUiRequestDto`，靠「无 agent turn
  在飞 + 已有 controller」去推断「这是冒烟请求」**是有竞态的**，而且现在不需要了。

  剩下的真实缺口只有一个：**由其他会话里的代理触发的后台构建**会把用户拉到那个 app。
  本轮不为它建抑制机制（YAGNI；本设计的两条流程都不产生这种情况）。若实测确认它讨厌，
  再做——届时正确解法是宿主侧的离屏 WebView 宿主，而不是抑制 presentation，因为抑制
  等于让门失效。

这样 `maxRepairRounds` 仍由 JS 消费、repair prompt 仍与 `SHAPE` 放在一起，宿主只
拥有不可伪造的观察与判定。`LocalWorkflowHandler` 不需要重新实现 agent prompt。

### workflow 脚本删减

- 删 model Verify 阶段，保留一份只由 host smoke finding 触发的 repair prompt
  （`local_app_workflow_core.js:260-312` 的现有自述 verify loop 删除）。
  balanced/thorough 的 agent 调用数从 3–7 降到 **2**（Design + Generate&Build），
  DOM fast 从 2 降到 **1**（Generate&Build）；冒烟门触发修复时各加 1。
- 删 `STRATEGY_POLICY` 中的 `verificationMode` 字段与三段散文。
  `runDesign` 保留（`local-canvas-build` 拒绝 `fast` 的理由仍成立：
  `fast` 跳过 Design，而画面应用的难点全在机制与帧循环上）。
  `maxRepairRounds` 保留，由 JS 围绕 `localAppSmoke()` 消费，不再由 verify 的自述
  结果消费。当前范围无论 strategy 最多一次 smoke repair；若保留配置字段，三个
  strategy 的值都归一为 1，避免文案继续暗示 thorough 会做更强的人类验收。
- 删 `args.revision_prompt`（只到 Design、`fast` 下被丢弃的死线；
  唯一写入者是测试辅助 `builtins.rs:1059-1078`）。其职责由标注承担。
- workflow terminal result 删 `verification_mode` / agent 自述的 `verification`，固定为
  `{ok, strategy, complexity, agent_calls, preview_url, repair_rounds,
  delivery_status, smoke_report, summary}`；`delivery_status` 只允许
  `ready | needs_user_review | verification_unavailable`。
- `builtins.rs` 中断言阶段序列与 agent 计数的测试同步更新
  （`:280-286`、`:272-278`、`:294-300`、`:441`、`:456-465`、`:817-822`）。

  🚨 **早期清单漏了两个必红的合约测试**：
  `local_app_build_is_model_invocable_and_pins_its_contract`（`builtins.rs:125`）断言
  `descriptor.script.contains("title: 'Verify'")`（`:133-138`）、三个 phase 按序出现
  （`:139-148`），以及脚本文本含锚点 `"verificationMode"`（`:159`）与
  `"verification_mode"`（`:163`）；canvas 孪生在 `builtins.rs:1100`。
  ⚠️ 修它们时**不要顺手把 phase 顺序断言一起删掉**——那是剩下两个 phase 唯一的顺序守卫。
  正确做法是把 `Verify` 锚点换成对新 `delivery_status` 取值的断言，保留顺序检查。

### 顺带修掉的既有缺陷

| 缺陷 | 位置 |
|---|---|
| `local-canvas-build` 拿不到 workspace lease（字面量比较） | `tasks/src/handlers/local_workflow.rs:1987` |
| 确认过的 spec 从不落盘 | task handler 在 workflow 启动时写 `workspace/.lingxi/spec.md`，`LINGXI.md` 增加指向它的一行 |

### ⛔ 撤回：不要删每次 UI 动作前的 `getDetails(appID:)`

早期版本把它列成「纯粹的延迟税」。**它是承重的，而且删了正好打断本设计的冒烟门。**
代码在 `LocalAppsStore.swift:1111-1115` 写明了理由：

> an inspect can reach iOS before the `running` event that carries its loopback URL.
> Refreshing the authoritative details first lets the preview route mount its WebView
> while the registry waits for it, instead of stranding the request on the not-ready
> placeholder.

机制：`AppDetailsChanged` 的回复才填 `runtimes[appID]`（`LocalAppsStore.swift:896-907`），
而 `LocalAppPreviewView` 只在 `store.runtimes[appID]?.url != nil` 时才挂
`LocalAppWebView`（`LocalAppDetailView.swift:571-595`）。没有挂载就没有 controller
注册，`LocalAppWebViewRegistry.execute` 轮询 50 × 200 ms 后失败
（`LocalAppWebView.swift:109-115`）。

**冒烟门恰好在这个窗口里开火**（`LocalAppBuild` + `LocalAppRuntime restart` 之后立刻）。
删掉 `getDetails` ⇒ `runtimes[appID].url` 仍为空 ⇒ 无 WebView 挂载 ⇒ 10 秒后
`local_apps_error_ui_not_open` ⇒ 每一次构建都是 `verification_unavailable`，**门一次
都跑不起来**。

连带修正两处：registry 那个 **10 秒**上限**先于**本文的「20 秒单次」deadline 生效，
所以对「未挂载」这一类失败，20 秒 deadline 永远不会触发；而真正会泄漏的条目是宿主
侧的 `pending_ui`（`local_apps_host.rs:2115-2119`），不是客户端的
`pendingUIRequestAppIDs`——后者已在 `LocalAppsStore.swift:1092` 自清。

deadline 本身**不是**跨 crate 改动：`request_ui` 的生产调用者只有三个
（`local_apps_host.rs:3605`、`:3621`、`:3669`），全在同文件、是 `LocalAppsHostBroker`
的私有固有方法，加一个 `deadline` 参数或在调用点包 `tokio::time::timeout` 并清理
`pending_ui` 即可（约 4 行）。早期版本称其需要签名级重构，论据错引了一个位于
`#[cfg(test)]` 里的调用点（`local_apps_host_agent.rs:1357`）。

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
- 视图坐标 → CSS 像素：**不能假设点坐标恒等于 CSS 像素**。模板只设置
  `initial-scale=1`，没有禁用 pinch zoom。松手时从同一个 WebView document 读取
  `visualViewport {width,height,offsetLeft,offsetTop,scale}`，按当前 WebView bounds
  比例换算 selection rect；滚动与缩放 offset 一并写入 annotation viewport。
  裁剪仍用原始 view rect，命中测试与落盘 rect 用换算后的 CSS rect。两端
  `pointer`/`inspect_ui` 共享这份 viewport 定义，不各自推导。

pan 结束后由 `LocalAppWebViewController.makeAnnotation(viewRect:)` 生成一个原子业务
结果，而不是让 SwiftUI 分别猜三份数据：

1. 在 page JS 中读取上述 viewport，把 view rect 换成 CSS rect；
2. 沿用 `deepQuery` walk，筛选 `visible` 且 rect 与 selection 相交的元素，输出
   `elementId/role/name/rect`；不读取已被 inspect 规则标成 sensitive 的 value；
3. 用原始 view rect 做 `WKSnapshotConfiguration.rect` 裁剪并编码 JPEG；
4. 一次返回 `annotation_id/rect/viewport/hitElements/imageBase64` 给清单。

JS 几何与 native snapshot 之间可能跨一帧；动画 app 已明确接受“松手附近的帧”，但
两者必须由同一个 controller 串行执行，不能穿插另一次 navigation/UI request。

### 副驾驶条（三态）

| 状态 | 触发 | 内容 |
|---|---|---|
| agent 操作中 | 收到 `AppUiRequest` | 横排操作帧；坐标动作带标记圈，点击放大 |
| 刚框完 | pan 结束 | 输入框「这里怎么了？」 |
| 有待提交标注 | 清单非空且非标注态 | 药丸清单 + 「开始修这 N 个问题」 |

`AppUiRequest` 本身没有图片，普通 click/pointer/fill 的 result 也是 JSON；只有
显式 `captureView` 才返回 frame。因此回放帧不能只靠 request 数据假装存在。

具体实现：

- injected action result 增加 `action_point`（pointer 直接用输入坐标；元素动作取最终
  resolved target rect 中心）和 `resolved_rect`。key/back/reload 可以没有坐标。
- `LocalAppsStore.executeUIRequest` 先正常 resolve engine request，不把 agent 往返绑在
  截图耗时上；随后让同一个 `LocalAppWebViewController` 异步产生一张**仅客户端本地**
  的 240 px 长边、JPEG 0.55 thumbnail，画上 action marker 后加入回放。
- 显式 `captureView` 返回给 agent 的完整 frame 同时加入回放，不再重复抓一张。
- 截图失败只把该 action 显示成文字药丸，不得反过来让已成功的 UI action 失败。

`pendingUIRequestAppIDs` 继续只表示 engine request 在飞；新增独立的 per-app replay
ring buffer。回放图不进入 `result_json`、session 或 engine，所以不受 256 KiB 通道
限制；客户端总内存上限 6 MiB，达到上限时即使不足 24 条也先丢最旧图。

回放项上限 24 条（与既有 workflow 日志上限一致，
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

复用既有一次性引导的持久化模式，但**不用全屏向导**——要教的是一个就地手势，
脱离真实界面讲了记不住。

⚠️ **那个模式不是 `@AppStorage`。** `@AppStorage` 在 `clients/ios/Sources` 里**零命中**
（全树 grep）。`SetupWizardView.swift:26-31` 只有 `@State`，自己不持久化任何东西；
一次性标志住在 `AppState` 上，而 `AppState` 是 `@Observable @MainActor final class`
（`Theme.swift:25-27`），用的是朴素
`var setupDone: Bool { didSet { defaults.set(setupDone, forKey: "setupDone") } }`
（`Theme.swift:48`）。**`@AppStorage` 是 SwiftUI `DynamicProperty`，根本没法用在那个
class 上**——照字面写不出来。新标志沿用 `didSet + defaults` 这个形状。

🚨 **`Theme.swift:60-70` 在 `LINGXI_UI_TESTING == "1"` 时强制 `setupDone = true`**，
即首启引导在 XCUITest 下被抑制（除非 `LINGXI_FORCE_ONBOARDING` 这类显式 override）。
本文要求新的 coach mark 有 UI 测试覆盖，所以**必须为新标志加同形状的 override**，
否则那些测试跑的是「引导已被跳过」的路径，永远绿且永远没测到东西。

**第一次进验收态**：真实界面上覆半透明遮罩，手指从左上拖到右下拉出矩形的
动画，一行字「发现问题？在界面上拖一个框，告诉我哪里不对。」，一个「知道了」。
**若用户直接上手拖框，引导立即消失**——教学目的达成即退场。

**第一次看到回放条**：旁边浮一行小字「我正在操作你的 app，这里能看到我点了
哪儿」，3 秒自渐隐，无需点击。用独立标志，因为两件事可能相隔很久。

可重放入口放进 app 详情页的 `ellipsis.circle` 菜单——该菜单目前**只有
「重启」一项**（`LocalAppDetailView.swift:108-128`）。

⚠️ 文案写进 `clients/translations/*.json`（真源），**不是
`Localizable.xcstrings`**（`generate.py` 的生成产物，手改下次全还原）。
缺 key **不报错不崩，直接把 key 本身当文案显示**。iOS 占位符在 KEY 里、Android 在
值里，常需两个 key。

落地时跑一遍审计：扫 `String(localized: "...")`，把 `\(...)` 归一成 `%@`，**按 ` %`
之前的前缀**比对 catalog（直接比全 key 会因 `%lld`/`%1$@` 产生大量假阳性）。
（早期版本引 `local_apps_create_intake_seed %@` 作为踩坑先例，但全树 grep 这个 key
只命中 spec 自己——教训成立，那条证据在本树不可复现，故删除。）

## 边界与错误处理

| 场景 | 行为 |
|---|---|
| 构建 workflow 仍在跑时用户提交标注 | 冻结当前 ID 为 queued batch，按钮显示「构建中，稍后自动提交」；监听 app id 匹配的 `AppWorkflowTaskChanged`，直到 active task set 为空。等待期间新增项进入下一批 |
| workflow `failed/killed` | 不发送 queued batch，保留并显示「构建未完成，点此重试提交」；不能因为任意 task 结束就发 |
| 当前另一个 scope 的普通 turn 正在 streaming | 保留 batch 并显示「当前回复结束后提交」；不调用 `switchScope`，不取消无关 turn |
| `StoreAppAnnotation` 失败（磁盘满、超限） | 按回显的 `annotation_id` 把该条留在草稿并标红；其余照常提交，消息正文只列成功落盘的条目 |
| 裁剪图超尺寸 | 引擎按既有质量阶梯降质；仍超则拒绝该条并回 `error` |
| 矩形内无命中元素 | 正常提交，元素段写「（无 DOM 元素，canvas 区域）」 |
| 标注模式下 app 仍在自行动画 | 允许——截图取的是松手瞬间的帧；矩形是坐标不是快照 |
| `capture_ui` 区域越界 | 持有真实 viewport 的客户端钳位；完全在视口外则回错误，不静默重定向 |
| 冒烟门第 4 条误判（应用本来就是纯色设计） | 快照同时有可见文本/图片/canvas 结构证据时降为 warning 并写进诊断；否则阻塞 |
| `StoreAppAnnotation` 提交后 8 秒无回执 | 该条置 `draft(error: "no_receipt")` 并在 UI 可见。`submit` 的兜底臂会静默吞掉未识别命令（`host.rs:7064-7074`）且移动端无握手，所以「没回音」必须是一个显式状态，不能靠等 |
| 提交时 `send` 返回 `nil`（五个守卫之一命中） | batch 回到 `stored(error，可重试)`；**不能**用 `_ =` 丢掉返回值，那个 token 是 `TurnStarted` 的 correlator |
| 冒烟门开火时 app 的 WebView 未挂载 | 门的请求经既有路由触发 presentation，预览挂载后 controller 注册、请求继续。若 45 秒整门 deadline 内仍未挂载（app 已删、运行时没起来），报 `infrastructure_unavailable` |
| `inspect_ui` 载荷被降级（`truncated` 含 `runtimeErrors`） | 判据 6 判为 `infrastructure_unavailable`，**不判通过**——观测不到不等于没异常 |
| 用户在 agent 操作中途进标注模式 | 允许。overlay 接管触摸不影响 `act_on_ui`（后者是注入 JS 合成事件，不经过 UIKit 触摸链） |
| 同一 app 多条 annotation 回执乱序 | 只按 `request_id + annotation_id` 归并，不按数组位置或最后一次请求猜测 |
| 修复期间用户又新增标注 | 新标注保持 draft；`AppBuildSucceeded` 只清除已 submitted batch 中的 IDs |

**并发写入风险（明确记录，未完全消除）**：客户端延迟提交只是 UX 层的护栏，
另一个客户端或另一条会话仍可能在 workflow 跑的同时对同一工作区发起编辑轮次。
`WorkspacePermissionLeaseRegistry` 是**授权**而非**互斥**机制
（`permission/src/workspace_lease.rs:60-83`），不构成保护。若实测出现真实
冲突，再引入 app 级的编辑互斥；本设计不预先建造它。

### 清单必须落盘，`submitted` 必须有出口

**标注清单只在内存里是不够的。** 进程在 `StoreAppAnnotation` 成功之后终止，会丢掉
清单与批次，却在工作区留下**孤儿图片文件**。仓库里已有两个可照抄的先例：
`LocalAppsStore.swift:788` 的 `UserDefaults` 键 `local-apps.running-before-suspension`
（重载时在 `:791-802` 与权威状态对账），以及 `LocalAppWebsiteDataStoreRegistry`——
一个完整的带版本 `Codable` 日志（键 `local-apps.pending-web-data-cleanup.v1`，
`LocalAppWebView.swift:1302`，写在 `:1344/1351/1373`，**并在 `:1358`
`removeDataForDeletedApps(activeAppIDs:)` 里与引擎的权威快照对账**）。

照第二个先例做：per-app 的带版本 Codable 清单，重启时加载，并与工作区
`.lingxi/annotations/` 的实际文件对账——**盘上有文件而清单里没有的，删掉；清单里有而
盘上没有的，降回 `draft(error)`**。

🚨 **`submitted` 目前是死胡同**：状态机唯一的出边是「新 build_id → cleared」，所以修复
轮次被**取消 / 失败 / 完成但没产生构建**时，那批标注永远停在 `submitted`，既不会清也
不能重试。出口已经现成：`ConversationTurnCompletion`（`ConversationSource.swift:155-168`）
带 `Outcome.{completed, maxTurns, cancelled, failed}`，**键正是规则 5 要求留住的那个
`ConversationTurnToken`**，发布在 `:1996-2006`。

而且不需要新增任何管道：`ConversationSource.apply` 在自己的 switch **之前**就把每条原始
事件转给 `externalEventHandler`（`:3785`），`RootView.swift:813/918` 泵进
`clientEventCenter.publish`，`RootView.swift:192` 无过滤地订阅 `localApps.handle(event:)`
⇒ `LocalAppsStore.handle(event:)`（`:209`）**本来就收得到**。

新增边：`submitted --Outcome != completed--> stored(error，可重试)`；
`submitted --completed 但超时未见 post-baseline build--> stored(可重试)`。

客户端 annotation 状态机是：

```
draft ──Store 成功──► stored ──TurnStarted──► submitted ──新 build_id──► cleared
  │                      │
  └──Store 失败──► draft(error)
                         └──route/send 失败──► stored(error，可重试)
```

queued 是 `stored` 上的等待原因，不是另一个会把新标注一起清掉的全局布尔值。清理
必须按 annotation IDs 做差集。

## 测试策略

### 引擎 / Rust

- `localAppSmoke()` 只能由两个内置 local-app workflow 调用；错误 workflow、错误
  app id、无/不匹配 lease 各有拒绝测试。报告由 host callback 返回，JS 无法注入
  一个 `status:"passed"` 覆盖它。WebView 未挂载/timeout 返回
  `verification_unavailable`、agent_calls 保持 2，并确认 pending request 被清理。
- 冒烟门六条判据各有独立测试，**每条都要有反向用例**：空白 DOM 第 4 条红、
  纯阅读 DOM（无 interactive elements）通过、静止 canvas 第 5 条红、只有外围 DOM
  spinner 在动而 canvas 冻结仍红、`error` 与 `unhandledrejection` 各让第 6 条红。
- 反向用例不得写成 `if cmd; then 报错; fi`——`if` 会暂停 `set -e`，把脚本
  崩溃判为通过。用「退出码恰为 1 且输出无 Traceback」的判据。
- `StoreAppAnnotation` 的路径推导测试必须钉住**真实设备路径形状**
  （`apps/<id>/workspace/.lingxi/annotations/`），而非重复实现里的推导；另测非法
  base64/JSON/rect、尺寸上限、原子写、同 app 并发乱序回执仍回显正确 ID。
- 协议：command 与 event goldens、version index、TS union/guards/snapshots 和
  `clients/shared` 完整 `npm test`；测试明确断言 `AppAnnotationStored` 位于
  `AppEventDto`，不是新增顶层 event。
- `AppWorkflowTaskChanged`：普通 task 与非 local-app workflow 不发；两个内置 workflow
  的 running/completed/failed/killed 都带正确 app id/task id/name，重复事件去重。
  同 app 两个 workflow 重叠时，第一条 completed 不触发提交，集合清空后才决策。
  客户端错过 running delta 后，`GetAppDetails`/reconnect snapshot 能恢复 active set。
  构建中切换 active session 后仍收到 app terminal（但旧 owner 的普通
  `TaskStatusChanged` 仍按原规则过滤）。
  首次构建、ready app rebuild、复用相同 output 的成功 build 都会各发一条
  `AppBuildSucceeded`，且 `build_id` 唯一。
- workflow 脚本：`builtins.rs` 中阶段序列与 agent 计数断言更新为
  balanced/thorough 的 `["Design","Generate & Build"]` / `agent_calls: 2`，以及 DOM
  fast 的 `["Generate & Build"]` / `agent_calls: 1`；host smoke fail 时各恰好 +1。
  第二次仍失败返回 `needs_user_review`，不能伪装 smoke passed 或 throw 掉 servable app。
- 跑测试**必须捕获完整输出到文件再 grep**——只 grep `FAILED` 会丢掉
  `failures:` 块里的测试名。用 `--no-fail-fast`：cargo 在第一个失败的
  **二进制**处停止，测试总数下降即使 0 failures 也是红旗。

### iOS

- 注入 JS 的几何输出：扩展 `LocalAppsStoreTests` 中已有的 shadow-DOM walk
  测试（`executionSource` 是 internal 正是为此）；覆盖 element/canvas rect、
  `action_point`、runtime error ring 的 32 条上限与 reload 清空。
- 消息组装格式：快照测试。
- 坐标换算：视图点 → CSS 像素 → `capture_ui` 裁剪，至少覆盖 1x、pinch zoom、滚动
  offset、旋转后 viewport 四条；不能只测初始 scale=1。
- prompt 路由：从 global/project/另一个 app scope 提交都进入目标 app workspace；
  已在同 app scope 时保留当前 session；目标 app 有最近活跃 session 时优先于 init；
  当前无关 source 正在 streaming 时不调用 `cancelAndWait`、不取消该 turn，结束后才切；
  switch 正忙时 payload 重试且只发送一次。
- annotation 状态机：N 条 Store 回执乱序、部分失败、workflow failed/completed、
  submitted batch 后新增草稿、下一次成功 build 只清 submitted IDs。
- 回放：普通 pointer/click 成功后本地异步抓 thumbnail；显式 capture 不重复抓；
  thumbnail 失败不改变原 UI request 的成功结果；24 条与 6 MiB 两个上限都测。
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

Android 虽不落 overlay/副驾驶条，阶段 1 的共享 UI tool contract 仍需设备或
instrumentation 覆盖：element/canvas rect、runtimeErrors、区域 PixelCopy crop、
`last_action` marker 与 `marker:"none"`。若该轮无法上 Android 设备，必须把缺口
记为 phase 1 未完成，不能用“Android UI 非目标”把 agent-facing tool contract 判绿。

## 分阶段落地

| 阶段 | 内容 | 依赖 |
|---|---|---|
| **0** | **`additional_context_message` 走 `prompt_probe_cwd_resolver`，让 `LINGXI.md` 真正到达移动端模型** | 无；**阻塞其后一切** |
| 1 | 两端 `inspect_ui` 几何/canvas rect/runtimeErrors（含载荷预算）+ `capture_ui` 区域（自算裁剪尺寸）/光标标记 + `ResolveAppUiRequest.error_code` + 该项的 bless | 0 |
| 2 | `localAppSmoke` primitive（新 global，非 agent 通道）+ 宿主判定（capture 一律 `marker:"none"`）+ host-evidenced repair loop + `request_ui` deadline + **两处 `local-app-build` 字面量改集合判定**（lease `local_workflow.rs:1987` 与删除守卫 `registry.rs:796`）+ spec 落盘 | 1 |
| 3 | `StoreAppAnnotation` + workflow/build 事件 + `AppDetailsDto` build_id + snapshot 绕过会话过滤 + 完整 command/event bless | 1（仅 bless 流程复用） |
| 4 | iOS overlay + 标注状态机（含无回执超时）+ app-scope 路由（不用 `initialPrompt`）+ 副驾驶条三态 | 1 与 3 |
| 5 | 引导 + 文案 + UI 测试 override | 4 |

- **阶段 0 是全局阻塞项**，且它是一个既有在线缺陷、不属于本功能的新增工作。
- 阶段 1 是 2 的硬前置：2 的 canvas 局部 diff、异常判据、`kind` 判别分别依赖 1 新增的
  rect、runtimeErrors、`error_code`。
- 阶段 1 **不再是「无 DTO 改动」**：`error_code` 把一次 bless 拉进了阶段 1。
- 阶段 3 可与 2 并行（都只依赖 1）。
- ⛔ **「删 `getDetails(appID:)`」已从计划中撤回**——它是冒烟门能开火的前提，见上文。

## 已知约束

1. **协议**：`7.0.0`，blessed major 7。additive 也需重新 bless。
   移动端无握手，偏斜不可检测。
2. **uniffi**：变体追加到末尾；不给无字段枚举加带数据变体。
   `ClientEvent` / `AppEventDto` 的 docstring 会被烤进定容元数据缓冲区且
   **已接近上限**——新事件注释用 `//` 而非 `///`。
3. **生成绑定：两端都是 gitignored 的构建产物，不是签入文件。**
   Android `clients/android/.gitignore:7` 忽略整个
   `app/src/main/java/com/lingxi/code/bindings/`（`git ls-files` 对 `android_aar.kt`
   报 "did not match any file(s) known to git"）；iOS 同理
   （`clients/ios/.gitignore:6-7`，`Generated/` + `Frameworks/`）。
   ⛔ 早期版本写「Android 是签入的构建产物」——错的。（错误来源值得记：我的记忆正文
   写对了，但那条记忆的文件名保留了它第一版的错误结论，我按文件名认的。）

   **改 wire DTO 后必须先重新生成再声称客户端编得过**，`cargo test` 全绿对两个客户端
   零信息量，「我写了对应的 when 臂」同样零信息量——只有重新生成的绑定能把两者接上。
   - iOS：`LINGXI_REUSE_STAGED_LINUX_RUNTIME=1 clients/ios/scripts/build-xcframework.sh`
     （不带该环境变量会重建 Alpine rootfs 并需要 Podman 运行）
   - Android：`clients/android/scripts/build-jni.sh`（会先构建全部 NDK ABI）。
     只要绑定的话约 1 分钟：`cargo build -p android-aar --features uniffi` 然后
     `cargo run -p ios-framework --features cli --bin uniffi-bindgen -- generate
     --library target/debug/libandroid_aar.dylib --language kotlin
     --config apps/android-aar/uniffi.toml --out-dir ../clients/android/app/src/main/java`
     （**stock `uniffi-bindgen` 在这个 surface 上会 panic**，必须用
     `ios-framework --features cli` 里那个打过补丁的 bin）
   - Android gradle 任务是带 flavor 的：`:app:compilePlayDebugKotlin` /
     `compileDirectDebugKotlin`；裸 `:app:compileDebugKotlin` 报 ambiguous——
     看着像构建挂了，其实是拼错。
   - ⚠️ 陈旧绑定**只表现为编译失败，没有任何测试或 CI 步骤会提前抓到它**。
4. **CSP**：单一无条件常量 `LOCAL_APP_CONTENT_SECURITY_POLICY`
   （`local_apps_host.rs:100`），三处同源（该常量、iOS `LocalAppWebView.swift:1076`
   注入的 meta、Android `LocalAppWebView.kt:1434`）。
   ⛔ **早期版本写「`wasm` 与 `Worker` 被实测拦死」是错的，而且它引的那一行说的正相反。**
   实际策略是 `script-src 'self' 'unsafe-inline' 'wasm-unsafe-eval'; … worker-src 'self' blob:`，
   且 `local_apps_host.rs:80-81` 明写这两项是 **DEFAULTS, not a grant**（理由：策略本来就
   带 `'unsafe-inline'`，页面能跑任何自己打包的 JS；wasm 严格弱于此，blob: worker 跑的是
   同源同一份 JS）。2026-08-21 的「实测被拦」是**改策略之前**的事，注释 `:96-99` 作为历史
   保留——**只 grep 到 "both blocks were real" 会读成现状**。`code_generation` 那个能力
   二选一分支已不存在。
   真正的边界没变：`default-src 'self'` + `connect-src 'self'`，设备/宿主能力在 bridge
   逐个门控。
   ⇒ 注入 overlay 是 CSP-legal（本文本来就不注入，见 iOS 图层一节），Worker 帧循环和
   wasm 模块也**不该**因这条约束被拒。
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

11. 🚨 **任何「顺带删掉 X」的条目，落地前必须回答「谁依赖 X 的副作用」，而不只是
    「X 做了什么」。** 本设计三次栽在同一个形状上：
    - 删 model Verify 阶段 —— 副作用是**保证 app 在交付前被打开过一次**
    - 删 `getDetails(appID:)` 往返 —— 副作用是**触发预览路由挂载 WebView**
    - 抑制 presentation —— 副作用同样是**触发 WebView 挂载**（而且是唯一的那个）

    三个副作用都不在各自函数的名字里，也不在它们的直接调用链上；靠读那段代码
    「做了什么」永远看不出来。判据：**先看谁在时序上紧跟着它、依赖它建立的状态**，
    再决定它是不是纯成本。三次里三次都是评审抓的，没有一次是自查发现的。

## 未决问题

无架构阻塞项。以下参数在实现中用 fixture 标定，不改变数据流或阶段依赖：

- 冒烟门第 4 条「非纯色」的方差阈值具体取值——需要用几个真实生成的应用标定。
- canvas 两帧像素差的最小比例——必须在 canvas rect 内标定，并对 JPEG 噪声设下限。

回放 thumbnail 已定为长边 240 px / JPEG 0.55 / 24 项与 6 MiB 双上限；它只在
客户端内存中，不走 256 KiB `result_json`。标注清单不再按「app 下一次成功构建」
整体清空，而是按 `batch_id + annotation_id + build_id` 清理 submitted 子集。

## 评审修正记录

本文经两轮对抗式核验：Codex 提 7 条 findings 并自行修订，随后 7 条各由两名独立评审
在代码上复核（refute-by-default），另有一轮无命题扫描。裁决与修正：

**Codex 的 7 条**：6 条成立、1 条（冒烟门编排）**结论对但命名错**——它说「没有组件
拥有那个循环」，而循环确实在编译期嵌入的 workflow 脚本里（`local_app_workflow_core.js:260-310`），
租约也覆盖整个脚本运行期（`local_workflow.rs:2016` → `:2104` → `:2341`）。真实缺口是
「VM 没有宿主背书的观测原语」，即它修的东西。修复方向全部保留。

**被评审推翻的三条论断（已在正文修正）**：
- 「deadline 需要跨 crate 签名重构」——论据错引了 `#[cfg(test)]` 里的调用点
  （`local_apps_host_agent.rs:1357`）；生产调用者只有三个、全在同文件，约 4 行改动。
- 「journal 重放是修复引入的新缺陷」——只在原语走 agent 通道时成立，而 spec 写的是
  新 global。降级为实现约束并写进正文。
- 「并发提交会乱序回执」——未经代码证实（`@MainActor` + 顺序 await）。correlator 仍
  必需，但理由换成「回执经单一共享事件流回来」。
- 「uniffi 元数据已接近上限」——**实测 15.8% / 28.7%，不成立**。`AppEventDto` 的选择
  保留，理由换成既有信封与单一分发点。

**扫描抓出、两轮 findings 都没提的（已在正文修正）**：`AppWorkflowTasksSnapshot` 的
registry 查询带会话过滤且单测因默认 `None` 而假绿；`getDetails` 删除是打断冒烟门的
回归；冒烟门 UI 请求全屏劫持；`@AppStorage` 不存在且 `LINGXI_UI_TESTING` 抑制引导；
CSP 约束写反；256 KiB 是硬失败且体积估算漏了十倍；capture 阶梯会放大裁剪区；
`builtins.rs` 漏两个合约测试；`StoreAppAnnotation` 无「无回执」态；`initialPrompt`
只在空转录时发出且 `send` 的 token 被丢弃；桌面 unavailable 路径不可达；
`AppDetailsDto` 没有 build_id 可作基线；`tool-api` 不 re-export `image`。

### 第三轮（Codex 6 条 + 双评审）

- **P0 冒烟门验不了新建流程：成立。** 早期版本为了不劫持屏幕而抑制 presentation，
  但 presentation **就是唯一挂载 WebView 的机制**，抑制它 = 让门在主流程下 100% 失效。
  已撤回，连同那套有竞态的 origin 推断。见已知约束 11。
- **绑定是 gitignored 不是签入：成立，早期版本写反了。** 错误来源：那条记忆的正文
  写对了，文件名却保留了它第一版的错误结论。
- **门必须传 `marker:"none"`：成立。** 门自己不发坐标动作，5 秒过期规则会让两帧一有
  标记一无 ⇒ 冻结 canvas 假过。
- **workflow sink 拿不到 app_id：headline 不成立**（`WorkflowCheckpoint` 带
  `workflow_id`/`args_json`，`bind` 本来就收到具体 `Arc<TaskRegistry>`），
  **但 `registry.rs:796` 的硬编码成立**，且它是**删除守卫**——canvas 应用可以在构建中
  被删掉，这是本文之外的在线 bug。同时发现 snapshot 不必新造绕过：
  `find_nonterminal_local_app_workflows` 已经不走会话过滤。
- **批次持久化 / `submitted` 死胡同：成立**，且两个先例与出口（`ConversationTurnCompletion`）
  都已存在，无需新管道。
- **`build_id` 无持久归属：成立**，且三条显而易见的替代方案（provenance / `buildKey` /
  进程内计数器）**逐条不可用**，理由见正文。
- **bless 清单不全：一半成立。** 索引条目确实漏了；绑定那半在上一轮已修正。
  新发现 `error_code` 的漏项是**四道门全绿的静默漏**，而 build id 是硬编译错误。

**最重要的一条来自本文之外**：§0 的 `LINGXI.md` 缺陷。它击穿了本文最初「既有对话
通道已经够用」的论证——写入端确实写了文件，但唯一渲染路径读的是未转换的 guest 路径。
教训记在 `[[lingxi-md-never-reaches-model-on-mobile]]`：**文件被写出来 ≠ 文件被送达
模型；查这类前提要 grep 唯一渲染路径，不是写入点。**
