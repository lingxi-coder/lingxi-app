# Chat / Code 集成：当前审查记录

状态记录：2026-11-03（America/Los_Angeles）。这是正在进行的审查台账；完整产品契约见[设计文档](../architecture/conversation-spaces-context-design.md)，当前工作区、来源和接续顺序见[交接文档](2026-11-01-conversation-spaces-handoff.md)。合并前 App iOS 源码 checkpoint 为 `6bc55fe9f30b5f510b8a1255e661f97222fa9423`；测试时为其父提交 `e5bb2ac98614a4f481c937b7ac5c0b60ee156ec0` 加冻结工作树输入，未以提交后 HEAD 重新运行；该阶段独立 SDK checkout 固定 `55ddd4cf880ed3de4696319c51b8429dcef80a9e`，父提交为 `46fb13b3449d02e595ec9fd49145d5de23bf66b1`。原 SDK 工作树 `f286ff5cd05c720b82451a66dc01e88bca213ade` 的未提交改动须保留。最终合同要求 Global Chat `~/.品牌/chat`、Global Code `~/.品牌/code`、配置保留 `~/.品牌`。[locked 离线来源核验](../../acceptance/conversation-spaces-20261101/protocol/post-merge-20261102/app-55ddd-locked-source-proof-20261103.json)通过；Android 和 Desktop 当时的产物及测试见下方，设备和完整平台验收尚未完成。

## 2026-11-03 本地 main 同步记录

App 合并的双方来源是功能分支 `58eb629b1965435a79fc0278875091ae443cd36d` 和本地 main `46049b8ec3f386cb4e00e206da6ec652e89fca4b`；回到本地 main 的最终 ref 与保全结果以任务目录 `merge-main-20261103/final-receipt.json` 为准。保留 main 的 Teams workspace、新 Bots 侧栏及未保存修改确认，也保留固定 session mode、普通 Global New Chat、原 owner 和 Local App Code 准入；Bot 侧栏通过所选行捕获的精确 transcript path 打开历史记录，待宿主校验。恢复重试保留原 owner、同一 runtime 和精确 transcript；未保存的 Bot 修改仍通过 main 的确认交互处理。

Harness SDK 的本地 main 与功能分支现为 `38ea730f503f318283cd4e26684f1abded10b6ec`；双方父提交是 `55ddd4cf880ed3de4696319c51b8429dcef80a9e` 和 `e0a8ae09607d361fc0b94d92fe51756238347302`。定向测试 122 pass、1 个 cache performance probe ignored，测试时的暂存树与合并提交树同为 `e54249690af5e4c2934c9de2bae2f55c0e4118df`；[快进和保全回执](../../../merge-main-20261103/harness/canonical-main-fast-forward.json)保留在任务目录。[App locked/offline/all-features 来源证明](../../../merge-main-20261103/app-pin-proof.json)共 1,052 packages，84 个 Harness、5 个 llm-client、5 个 Mobile Linux 包分别单源锁定 `38ea`、`3cb37e3`、`43382a`。这是源码依赖解析，不是新原生包或设备结果。

本轮 App [Bridge 恢复与 runtime manager 组合测试](../../../merge-main-20261103/bridge-manager-final.log) 238/238、0 skip；[转录/工具定向测试](../../../merge-main-20261103/transcript-focused-final.log) 104/104；Electron [web](../../../merge-main-20261103/electron-web-typecheck-final.log) 和 [node](../../../merge-main-20261103/electron-node-typecheck-final.log) TypeScript 检查通过。Bots 侧栏单测 15/15、相关 UI/导航 SSR 54/54、host fixture 的 Electron 侧栏交互 1/1 通过。翻译生成检查覆盖 2,752 keys / 5 locales，生成器测试 21/21；`check_i18n_pairing.py` 仍报告已有同名参数资源、`provider_accounts.xml` 和其他文案缺项，未记为通过。

SDK `55ddd4cf` → `38ea` 的 client/mobile/core types 和 UDL 公共 ABI 核对无差异；llm-client message 结构没有 schema 差异，Rust API 和持久化行为则包含新的 logical message ID。此次未发现必须重新生成绑定的差异，但已有 native 二进制仍是旧 `55ddd4cf`，不证明新 SDK 行为；要运行新 SDK，仍须重新编译 native runtime。

`llm-client` 已在本地合并并将 main 快进到 `3cb37e3a3b840b54a4e92994fdc8b1edf7f592d3`，[定向测试](../../../merge-main-20261103/llm/cargo-targeted.log)151/151；Mobile Linux 本地 main 快进到 `25cf8a273bcd572b9be48e18fc9807e01dcdfce1`，该提交只更改测试路径，App 的产品 pin 仍为其父提交 `43382a`。本次仅做 main 同步与必要的源码/定向检查，没有以新 SDK 合并 pin 重新跑完整原生构建或设备验收，也没有 push。

以下审查和产物结果属于合并前 `55ddd4cf` 阶段；它们说明当时实际测试的来源与范围，不是新合并 pin 的产物或设备通过记录。合并前其他工作树 dirty 内容由[保全清单](../../../merge-main-20261103/baseline.json)记录，须按实际状态保留。

## 合并前 `55ddd4cf` 审查和验证记录

独立 `mobile-linux-runtime` 的 App 锁定产品来源为 `43382a20161e371a0a31526412164b3d35545ab5`，其目录、managed mount 和 rootfs profile 改动已由 SDK 和 App 的锁定解析引用。该 runtime 的五包 Cargo 为 166 pass / 1 ignored，installer 离线 JVM 为 10/10；旧的真实 ARM64 staging 曾在磁盘阈值处中断。后续 Android 隔离包已核对两架构 rootfs，但设备执行仍待验证。

Mobile Linux iOS 的 [NativeRegression 首轮](../../acceptance/conversation-spaces-20261101/ios/git55-native-run/native-regression-43382-r1.result.json)在 `43382a` 原测试源上为 42 tests / 7 断言失败：两组旧 fixture 的 host 路径仍使用 `lingxi/projects`，与新 guest `/.lingxi/projects` 身份不对应。仅将这两个测试 fixture 路径改为隐藏目录后，[25cf 测试源重跑](../../acceptance/conversation-spaces-20261101/ios/git55-native-run/native-regression-25cf-r2.result.json)42/42 通过，xcodebuild exit 0；[来源清单](../../../build/ios-native-regression-25cf-20261104/test-source-provenance.json)记录 `25cf8a273bcd572b9be48e18fc9807e01dcdfce1` 的父提交为 `43382a` 且生产树无差异。App 的 runtime pin 仍为 `43382a`；此模拟器 stub 回归不能替代物理 iSH/设备执行。

## 已定位：移动 Cron 预准入 busy

真实 iOS simulator 用例发现：旧 SDK `mobile/host.rs` 三个 public Cron 入口在 `begin_owner` 返回 false 时产生 `busy: Scheduled session owner could not be reserved`，随后 `fired_cron_dto` 将其标为 `retryable=false`。拒绝发生在 claim、ledger、transcript 和工具执行之前。callback 的 Bool 既可能表示暂时占用，也可能表示 catalog/prepare 失效；它只证明该 occurrence 尚未执行，不保证拒绝是暂态。在锁内重新校验 occurrence/authority 后可安全重试，后续仍受 Host 验证，本轮不扩 callback ABI。修复还须区分 callback 期间已经发生的取消，不能把 cancel 加 false 误报成可重试 busy。e7 阶段已通过 locked/offline [Cron crate 169/169](../../acceptance/conversation-spaces-20261101/protocol/post-merge-20261102/e7-cron-lib.log)；修复已进入独立 SDK `46fb13b`；当前 `55ddd4cf` 的 [iOS simulator native 选定场景](../../acceptance/conversation-spaces-20261101/ios/git55-native-run/native-sequence-r3.json)覆盖 scheduler 的准入、callback 期间取消与 cwd/ledger，三个 mobile 入口和设备仍须按实际覆盖分别核对。

修复不应改变另一条边界：若原生执行已经准入、或提交/终态结果不确定，客户端不能假定未执行并自动重放。bc68 的全部通过结果及其 native artifacts 都要按原 source/pin 引用；后续新 SDK 只继承经过实际来源和受影响范围验证的证据。

## 已定位：Android 重连 media drain，最终设备回归待完成

真机分进程结果及独立复审确认：旧 Android FFI 在 engine 最后引用释放后异步 revoke media attachment，`waitForMobileSessionRelease` 只确认 session owner 释放，不证明 media 已 drain。重连执行 close → 等 session → build 时可能因旧 attachment 的 drain 未确认而拒绝新 engine。最后引用仍由 detached priming 持有时，旧 attachment 可能尚为 Active，随后才进入 Revoking。当前实现对精确旧 binding 同步关闭准入并等待 drain，不能关闭其他活跃 owner；UNKNOWN/TIMED_OUT 保持 fail closed。`55ddd4cf` 的 JNI、含空索引 fixture 的 test APK 和 app APK 均已构建，手机暂不可用，设备复测仍待完成。

## 当前审查：Android 目录授权与终端

独立审查发现，调度清单须严格匹配 SDK schema，不能因缺少 `task_id` 或字段类型错误仍授权内容目录；Project 索引不可用时，保留的旧快照也不能继续作为已验证写权限。清单的父目录读取须绑定无符号链接的实际目录，避免先检查后打开时目录被替换。Android 当前源码和 JVM 回归已包含这些边界；最终设备行为仍待验证。

终端的显式关闭需要等待正在发生的 PTY open 取得结果后关闭，不能取消协程后遗失已创建的 native handle；只有确认尚未提交 native open 的失败或取消可以重试，open/close 结果未知时保留原 owner 的屏障。后台输出订阅必须在 PTY 轮询前就绪，否则首条输出或 VT 查询可能丢失。当前源码已进入 `55ddd4cf` 隔离包并通过 JVM 回归；JNI 在设备上的终端路径仍待验证，不以旧 bc68 测试覆盖。

## 已定位并修复：iOS owner gate 生命周期

[真实 simulator r6 日志](../../acceptance/conversation-spaces-20261101/ios/final-ui-nav-r6.log)显示，从 Project 普通新建 Global 时，当前 `RootView` 的 gate 与保留的 `SessionHost` 捕获的 gate 曾是不同实例；失败时后者已为 nil，导致 `conflictingOwner`，Global 未被打开。原 `RootView` 中普通属性 gate 可随 view 重建，而 SessionHost、ProjectStore 和 CronRepository 由 `@State` 保留；正式路径即使仍强持旧 gate，也须保证导航、目录变更和 Cron 使用同一 gate，不能只为 UI fixture 增加引用。现已将 gate 与 preferences 改为 `@State`，[r7 模拟器 UI](../../acceptance/conversation-spaces-20261101/ios/final-global-chat-ui-r7.log)的 Project → Global、mode 和草稿检查 1/1 通过；r7 使用 bc68 和旧目录合同。当前 pin 的 [UI 重建后共享 gate](../../acceptance/conversation-spaces-20261101/ios/git55-native-run/ui-03-r2.summary.json)、[Cron 准入与取消 native 场景](../../acceptance/conversation-spaces-20261101/ios/git55-native-run/native-sequence-r3.json)及下方 Full 批次已执行；实际覆盖和阶段仍以各自回执为准。

## 当前修复：移动 Stop 准入与 iOS 项目筛选

独立审查发现两处不同的 Stop 边界。iOS 的 follow-up 在等待 durable ledger 期间，Stop 可已开始而 `streaming` 和原 turn token 仍有效；旧检查会继续消费输入并异步提交。源码现由同步 `cancel()` 先失效准入 generation，再开始异步取消；原 owner 的文本、附件和 chip 使用实际 admission 记录及 revision，避免用户在点击与异步开始之间移除 chip 后又被恢复。重连移交未发送项之前重新校验相同 scope/mode/UUID、native workspace 与精确 catalog transcript。独立静态复核已完成，[前驱绑定源码 typecheck](../../acceptance/conversation-spaces-20261101/ios/stop-picker-typecheck-46fb-diagnostic-r2/result.json)通过；当前 pin 的[受控 XCTest 批次](../../acceptance/conversation-spaces-20261101/ios/git55-native-run/app-host-batch-r4.summary.json)已运行，其中包含 follow-up Stop、两项精确回执和草稿/chip 交错用例。

SDK `46fb13b` 曾有 FFI 之后的窗口：Send 尚未 reserve 时指定该 ID 的 Cancel 先返回，随后 Send 仍可能启动；Side Chat 也有同类首 turn 排序。后继 `55ddd4cf` 在 exact session/turn 或 side/turn 上协调 Cancel 与 reserve/enqueue，并保留 Cancel 绕过长 transition 的能力。只有原 Send 的固定 `turn_cancelled_before_admission` 拒绝与同 engine/session/side/turn 的 explicit Cancel 成功共同证明未准入；已 reserve/enqueue 使用真实输入的 terminal，不造空 checkpoint，其他错误和未知结果不自动恢复。Pause 与显式 Stop 区分，`/cd`、`/add-dir` 沿既有导航合同；[35/35 SDK 实测](../../acceptance/conversation-spaces-20261101/protocol/post-merge-20261102/successor-turn-cancel-r5-validation-receipt.json)覆盖受控准入、Side、Cron、Pause、Bash 和 fence，Android 当前 pin 包已构建、设备待跑；iOS 当前 pin 的选定 Full、FFI 和 UI 测试见下方分阶段结果。

SDK 的 Pause 持久化与事件发送之间仍可能先送达同一 turn 的 Cancelled、再送达旧 Paused。独立审查确认 Android 原 reducer 会将后者当新恢复并重新阻塞 composer；修复以 source-local 的规范化 session/turn 记录真实 terminal，并在同一锁内决定与入队，拒绝该 owner 的迟到非终态，保留其他 owner 和正常 Pause → Running。此前 [r8 JVM](../../../build/android-native-validation/successor-jni-git46fb-20261103-r1/direct-jvm-r8-summary.json)使用 `46fb13b` 前驱 JNI；当前 `55ddd4cf` 包构建重新执行的 JVM 为 1,653/1,653，见[核验](../../../build/android-native-validation/successor-jni-git55-20261103-r1/isolated-apk-validation.json)。不将此规则推广为所有外部 listener 的全序保证。

iOS Side Chat 30 分钟自动过期会让 parent 收到 `SideChatEnded` 并移除 entry，原有逻辑清空草稿/附件，也会使已确认未准入或仍在途的原始输入失去入口。当前实现将输入保存在原 side 的只读值记录并释放 child 的 native 引用；在途结果未知时不称未准入或自动重发，且只允许经同 parent 的精确来源验证后随重连移交。首次 Side 创建在预准入时过期须保留 parent 原草稿，handoff 后过期则保留原 side 的未知输入。最终复审又发现普通 Send 的 transport 未知错误会清除保存完整 payload 的 turn 记录；修正后每次未知错误各自保留原文、附件、UTF-16 和当时消费的 chip，后续发送不能覆盖或混入新 chip。slash 不发送或消费该 chip，独立保留当前草稿。相关新回归和受控事件交错已在 Full 批次通过；显式 End 仍按用户动作处理。前驱绑定的[诊断记录](../../acceptance/conversation-spaces-20261101/ios/side-expiry-typecheck-46fb-diagnostic-r1/result.json)曾在产品编译后触发磁盘 guard，不能记为测试通过。

iOS workspace picker 曾展示未加入当前 mode 的 Project，选择后会给该 Project 增加当前空间 membership；这与列表按 mode 过滤的合同不符。候选在排序前按捕获 mode 过滤并在点击时复核 owner 与 membership，[当前 Full 批次](../../acceptance/conversation-spaces-20261101/ios/git55-native-run/app-host-batch-r4.tests.json)中的 `testWorkspacePickerShowsOnlyProjectsInTheCapturedMode` 已通过；普通 Global New Chat 的既有通过结果不能替代该场景。

## 当前审查：移动 Local App owner

Chat 空间禁止发起 Local App 创建、源码编辑、预览、发布、Remix 和开发恢复，须在写入 intent、源码或 Project 前明确提示用户切到 Code，不自动切换并继续；已安装应用的浏览和运行仍可用。Code owner A 已接纳的操作在用户切到 Chat B 后仍由 A 完成，不能因为当前空间改变而取消、改投或事后拒绝。三端 UI 入口和 SDK 工具入口已有相应实现；Desktop 当前 pin 真实 Bridge、Android 新包/JVM 已有分层结果，iOS Full 已覆盖下方列明的选定用例，移动设备仍需按实际执行范围验证。

模型工具还须区分普通应用列表和开发者身份。已发现移动 SDK 的 `LocalApp list` 曾直接返回完整 `AppRecord`，其中不仅有 `authoring`，历史 publication receipt 内也可能包含来源路径和 owner；模型结果须使用普通应用信息的显式投影，原生 Apps 页面仍需完整记录。移动端模型拒绝提示的本地化及 SDK 模型结果投影均已有实现；Android 当前绑定及包已构建；iOS Full 的结果及适用路径见下方，移动设备路径仍待验证。

首建受管源码只能在固定 Project、session 和 source 上登记；异步完成、草稿和语音操作须保留原 entry。冷恢复以确切的原生 owner receipt 为权限；receipt 缺失时只允许验证 source proof 后构造发送准入关闭的恢复 owner，确认原生会话身份后才可补登记并开放发送。SDK checkpoint 的 receipt 不可变且可保留多个 owner，使后续 current pointer 变化不会撤销已运行的 engine。独立审查发现：首次发布不能只按最新 source pointer 选择 owner，否则 A 的工具在 B 登记之后发布会归属于 B；即使 tuple 相同，原源码目录已被替换时也不能将替代目录发布为旧 owner 的内容。已有应用更新也须遵循受管源码与捕获的 engine owner，不能仅凭 app ID 和 revision 接受另一来源。SDK 已持久化 sourceId affinity 并在发布边界核验原源码和调用者；普通未绑定来源保持原发布能力。应用被移除后 tombstone 仍保留其 sourceId 归属，旧 source、数据和 receipt 保留；另一个应用必须使用可信流程创建的新 sourceId 副本。真实目标集成仍待完成。

移动端还须处理冷启动授权期间 settings 重绑、首建失败的原 owner 重试，以及进程退出后无法仅凭普通 Project 路径推断受信任创建意图的问题。已裁定使用持久 sourceId 证明与 owner intent 恢复；恢复副本和 Remix 创建的源码也必须由可信 SDK 流程登记同样的证明，重试沿用用户捕获的 release revision。缺失 receipt 的冷恢复须在发送准入关闭时由原生确认 Code、UUID、transcript 和来源工作区后再补登记。iOS 的项目页必须提供可实际到达的精确意图恢复入口，普通 New Chat 仍保持原语义；Android 的未入目录意图恢复须保留原 UUID/path 且不自动选择或重建已完成后删除的会话。iOS 同步准备草稿，避免晚到结果覆盖用户编辑；Android 的 Side 和 source 会话变更也须受同一 owner 准入限制。相关修复已有下方的宿主与模拟器分层结果；尚未覆盖的 native 集成和移动设备路径仍待验证。

最新源码审查还覆盖三处受控交错：Android 复用 authoring owner 时，选择检查须与原 source 正在进行的 workspace 修改互斥；iOS 普通会话和 authoring 的原生 source 已准备但尚未 retain 时若失败，必须强持对应 claim，按该 source 单飞退役，等待精确 media 与 session 两项 release；SDK 的 Code 子代理和 streaming fallback 发布须使用该工具调用所属 transcript 中已持久化且重放稳定的身份。iOS 候选的 [host 17/17](../../acceptance/conversation-spaces-20261101/ios/ordinary-preparation-market-zip-host-20261103-r3.log)含超时、并发和 ZIP 暂存，但使用 stub；不能将它或旧 pin 当作真实 FFI 通过。

iOS 的候选 media drain 还须在初次等待超时后继续处理迟到确认。独立审查发现，确认若发生在超时返回和 phase 检查之间，旧分支会遗漏 confirmed callback，导致 Swift 侧及 native route 未清理；SDK 已确认的 attachment 会自行从 host map 移除。当前候选按实际 phase 继续等待或完成回调，并加入该交错的 route 回归；当前 `55ddd4cf` simulator FFI 已用于下方选定 native 测试；该媒体 drain 交错的真实 FFI/设备验证不在已列范围内。

Desktop 发布恢复须保留宿主接管之前的精确暂存输入。preparing intent 重命名后若目录同步失败，原暂存可能丢失；候选已仅在确认尚未准入 SDK 且身份、摘要一致时处理。SDK tool 的准备记录一旦存在，即使部分 stage 已删除也不能同 ID 重采变化后的源码；ready 则保留原 owner、输入和摘要。实际 [tool-ui 24/24](../../acceptance/conversation-spaces-20261101/protocol/post-merge-20261102/successor-toolui-staging-replay-r2.log)覆盖中断、取消、`/cd` 和 deferred 重放。已提交或结果不确定的发布仍须按原权威回执处理。

Desktop 的应用会话创建在 Code A 已准入后允许导航到 Chat B：宿主继续原 owner 的登记，仅在原导航仍当前时展示。恢复须先核对现存 runtime 的实际 transcript，同 UUID 的 B 已驻留时不能先改它的重连路径；路径别名须指向现存同一普通文件，并保留原 catalog path/scope。`/cd A→B` 的 pending 重放沿原 A，preparing 和新 native 请求仍重验；[Desktop 定向测试 60/60](../../acceptance/conversation-spaces-20261101/desktop/desktop-authoring-replay-final-20261103.log)及新来源的[真实 Electron/Bridge 2/2](../../acceptance/conversation-spaces-20261101/desktop/desktop-local-app-real-bridge-combined-r2-20261103.log)已通过。iOS 已有精确 intent 的驻留 owner 恢复须核验 sourceId；无 intent 但已有有效权威 receipt 的正常恢复仍允许。

Android retained turn replay 的旧字符串 `outcome` fallback 与当前 SDK 的 tagged-object 协议及 clean-break 合同不符；SDK 当前将 `ClientEvent` 直接序列化到 journal，Android 旧分支却可把裸 `"end_turn"` 结算为完成。源码候选已移除此 fallback，并将旧兼容测试改为拒绝；保留当前 SDK fixture 正例。[真实 Direct JVM r6](../../../build/android-native-validation/successor-jni-git46fb-20261103-r1/direct-jvm-r6-result.json)已复测通过，不删除旧用户数据。

SDK generic 发布检查已按 source proof 的规范路径及祖先校验，并为 Windows verbatim/UNC、大小写与 physical identity 设置边界；包含卷根和 source 子目录的候选已进入 checkpoint。其平台无关源码测试已运行，Windows 专属真实文件系统用例未在本机执行。iOS Market Remix 曾被旧安装解包器的 dotfile 规则误拒；候选现在只保存已核大小和 SHA 的原 ZIP，由 SDK 验证完整 inventory 与路径。[实际 SDK mobile handler 2/2](../../acceptance/conversation-spaces-20261101/protocol/post-merge-20261102/successor-native-market-remix-mobile-r1.log)覆盖 `.gitignore`、`.well-known` 和 traversal 拒绝，仍不等于 iOS FFI 测试。

## 本轮审查覆盖的核心契约

| 交界 | 已审查的要求 | 仍需最终来源验证 |
| --- | --- | --- |
| Session / 导航 | 创建固定 mode；普通新会话为当前空间 Global；项目内明确新建才绑定项目。保留后台 source，异步完成只回原 UUID/scope/mode/incarnation。 | 三端最终 UI 中，运行、审批和 audio 时切换及返回。 |
| 恢复 / writer | 所选行的精确 transcript path/authority 从 catalog 贯穿 cold/hot resume、fork 和首次 writer mount；同 UUID 其他文件不能接管。Global Chat/Code 的默认 cwd 分别在品牌容器的 `chat` / `code`，不能据 cwd 推断 logical scope。 | 新 mode 目录接口及 successor 下受影响 native/host 回归；外部不合作写入只按设计文档的可观测替换边界声明。 |
| Desktop metadata | 对话框/归档使用捕获 receipt；已接纳写跨 timeout/断连持续持有 owner，直到权威 drain/原进程退出。 | 最终变更下的真实 Bridge 与关闭、重连、runtime 生命周期。 |
| 移动 send / recovery | 持久准入先于消费输入与 native 提交；Stop、失败稿、附件和 revision 防止误恢复或覆盖另一个 owner；ledger/witness 不确定时 fenced。 | Direct JVM 和 Full 选定回归已运行；移动设备行为仍待验证。 |
| Cron | 独立 reservation 先于绑定和执行；receipt 含 UUID/path/mode/scope，取消等待原生 admission 终结并 reconciliation。 | 关闭上述 busy 分类问题后重验三个真实入口；非 busy / 未知结果不能误重试。 |
| Remote / Team | 普通 session 未确认 mode 时 fail closed；Team capability 仅一次 allow-once/deny；附件与审批绑定捕获的 source。 | 最终协议和跨端集成；先前 Web 源码测试不代替实际设备或 provider。 |
| Audio | Flow、dictation、realtime 和自动播放捕获原 owner、operation 和配置；导航不把旧音频/审批交给新 owner。 | iOS Full 中的受控音频测试已运行；Android 真机及真实音频/provider 的适用范围仍待验证。 |

移动端当前没有 FileHistory UI 或还原入口；本轮不为补齐 Desktop/CLI 能力而新增入口。fork 在 source snapshot 已成功捕获后，源继续写入不使该合法快照失效。归属保证以合作 writer 和可观测替换为界，不引入跨文件 WAL 或 inode CAS 作为本轮要求。

## 已有验证证据，不等于最终验收

- Android：`55ddd4cf` ARM64 JNI 和三个 target-derived Kotlin 绑定已进入当前隔离 Direct app/test APK；193 suites / 1,653 JVM tests 全通过、0 failure/error/skip，instrumentation 编译完成。app APK SHA-256 `f0280b21bd13e65bbef8ea6dbf7fb2b80c80c3bd319e1af98fad01c03c09bcfb`，test APK `e4fd3b29c1ca8fbd1c639856c30df982cb8bf994c942641818704f7adca92a9f`，见[包核验](../../../build/android-native-validation/successor-jni-git55-20261103-r1/isolated-apk-validation.json)和[源码/构建回执](../../../build/android-native-validation/successor-jni-git55-20261103-r1/postpackage-validation-receipt.json)。两项无关 dirty ProviderAccounts 源码当时仍在。手机暂不可用，新包未安装或执行；旧 bc68 [分进程结果](../../acceptance/conversation-spaces-20261101/android/postfinal-direct-device-per-process-bc68-summary.log)为 2 pass / 3 fail，不能套用新产物。
- iOS：`55ddd4cf` simulator FFI archive 和七个生成绑定已有产物；原组装 wrapper 中断，使用相同不可变输入手动组装 simulator-only slice，89 项 native 输入核对一致，见[组装回执](../../../build/ios-ffi-git55-20261103-r1/manual-assembly-sim-r1/receipt.json)。`43382a` 产品和 `25cf` 测试源的 NativeRegression 42/42 见上文。包含 Factory root 和 mock 恢复修复的 [r4 Full 重建](../../acceptance/conversation-spaces-20261101/ios/git55-native-run/full-build-for-testing-r4.result.json)和[24 类 XCTest 批次](../../acceptance/conversation-spaces-20261101/ios/git55-native-run/app-host-batch-r4.summary.json)通过 535、失败 0、预期 Store-only skip 1。16 个独立 native 用例取 01–06/11 的 r3 与其余 9 项 r2；6 个 UI 用例取 01 r4、04/06 r3、02/03/05 r2，各自实际通过且不重复计数。输入以 `e5bb2ac` 加 479 项冻结工作树清单为准，内容聚合 SHA-256 `8640f838e445af43979f73b8610edfee8acdfceae5f2a54ae074e6894ebd680f`，r4 构建前后一致。[最终回执](../../acceptance/conversation-spaces-20261101/ios/git55-native-run/final-receipt.json)逐项保存来源和结果，SHA-256 `c09d2cdc9a1105daf8a5c1615683d67c7a0ff731c1c7445a279b0cd0cd5ddf79`。UI01 使用 mock stream、结果含一条 SwiftUI reparenting runtime warning；设备、真实 provider 和签名包未由此证明。
- Desktop / Bridge / CLI：`55ddd4cf` 的生产 Bridge、真实 Electron/Bridge 2/2 和 Rust handler fixture 的 CoreClient/CoreStore 解码 1/1 见[综合回执](../../acceptance/conversation-spaces-20261101/desktop/sdk55-desktop-final-provenance-20261103.json)；Bridge private Local App、scheduled、durable draft 分别为 [8/8](../../acceptance/conversation-spaces-20261101/protocol/post-merge-20261102/successor-app-git55-bridge-local-app-tests-r2.log)、[5/5](../../acceptance/conversation-spaces-20261101/protocol/post-merge-20261102/successor-app-git55-bridge-scheduled-tests-r1.log)、[3/3](../../acceptance/conversation-spaces-20261101/protocol/post-merge-20261102/successor-app-git55-bridge-durable-tests-r1.log)。CoreStore 为 fixture 重放，Electron 输入清单未枚举所有 Vite 运行依赖；早前磁盘 guard 中止的 SDK Desktop 构建不记为通过。CLI 本轮仅静态核对 [Code 创建/恢复校验](../../apps/cli/host/src/init.rs)、[headless 恢复](../../apps/cli/host/src/headless_host.rs)和 [Code 过滤](../../apps/cli/host/src/run/resume.rs)，没有新的 CLI 编译或运行；正式打包、发布与未覆盖 provider 场景仍待验证。
- SDK / Local App：后继 `55ddd4cf` 的 exact-ID Stop/准入 [35/35](../../acceptance/conversation-spaces-20261101/protocol/post-merge-20261102/successor-turn-cancel-r5-validation-receipt.json)通过；`46fb13b` 的对应源码闭包通过 [Local App 175/175](../../acceptance/conversation-spaces-20261101/protocol/post-merge-20261102/successor-localapp-clean-break-r2.log)、tool-ui 24/24 和 mobile handler 定向测试。真实 handler fixture 被 Android [host JUnit 3/3](../../acceptance/conversation-spaces-20261101/android/native-authoring-fixture-candidate-r2b-junit.log)与 iOS [host Swift 6/6](../../acceptance/conversation-spaces-20261101/ios/native-handler-fixture-r2-ios-host-r1.log)消费；stub 及前驱证据不能相加成当前原生设备通过数。
- 历史检查点与被替代的 pin/计划保留在原始 acceptance 记录和标为 superseded 的[历史快照](artifacts/2026-11-01-conversation-spaces-snapshot.json)，不再作为当前任务清单。

Android 离线隔离包的手动设备观察覆盖了 Global picker、Chat/Code 切换和 Chat 下新建仍为 Chat/Global；见 [Device follow-up](../../acceptance/conversation-spaces-20261101/android/postfinal-direct-validation-status-20261102.md)。随后按用户授权创建隔离空项目，[Project → 普通 New Chat → Global](../../acceptance/conversation-spaces-20261101/android/postfinal-device-manual-project-to-global-bc68.txt) 的手动检查也保持 Code mode。这些是旧目录策略下的 bc68 观察，最终 SDK 新目录须重验；观察不覆盖真实模型或运行中的 turn。分进程错误也暴露了上述 media drain 边界。

当前 `55ddd4cf` 的首批 Full simulator 运行日志报告 533 项、1 skip、7 次断言失败；runner 在测试输出后以 -15 结束，不能算通过。失败集中在三个 ProjectStore 测试的旧 guest cwd fixture 和两项把任意 rejected 误当确定未准入的 Side 测试；对应 fixture 已按新目录与 exact Send+Cancel 合同调整，并在后续 r2、r4 批次重跑通过。此批包含的多次未知发送/chip 新用例已通过；随后补充的 listener error 先于 Send FFI 结果的交错不在首批中，但已包含在 r2 通过批次。详见[首批日志](../../acceptance/conversation-spaces-20261101/ios/git55-native-run/app-host-batch-r1.log)和[runner 结果](../../acceptance/conversation-spaces-20261101/ios/git55-native-run/app-host-batch-r1.result.json)。

当前 SDK 的 16 个独立进程 native 用例首轮有 15 项通过；[模型切换用例](../../acceptance/conversation-spaces-20261101/ios/git55-native-run/fresh-native-11-r2.log)在被拒绝时仍断言旧 `Authentication failed` 文案，实际返回 `all provider accounts are unavailable`，测试失败后 runner 在 finalization guard 处以 -15 结束。调整后以账户不可用语义、原模型不变及同 handle 后续响应验证；[r3 重跑](../../acceptance/conversation-spaces-20261101/ios/git55-native-run/fresh-native-11-r3.summary.json)通过，首轮不能记为通过。UI 用例也以各自实际结果为准。

UI 的[运行中导航首轮](../../acceptance/conversation-spaces-20261101/ios/git55-native-run/ui-01-r2.summary.json)失败：返回原 owner 时测试期望 streaming/token 未变，但 80×300ms mock 流已自然结束，观测为 idle。该 UI 用例本身使用 mock，不能证明 FFI/provider；专用 mock 流改为等待显式 Stop，继续检查原 owner、token、隐藏期间 tail 前进和返回显示。[r3](../../acceptance/conversation-spaces-20261101/ios/git55-native-run/ui-01-r3.log)又因目标 Code 历史 UUID 与 mock 固定 `ui-session` 冲突而被真实 SessionHost 拒绝；仅测试 probe 现以所选 catalog 的精确 ID、workspace 和 transcript 初始化，不绕过 owner 校验。[r4 重跑](../../acceptance/conversation-spaces-20261101/ios/git55-native-run/ui-01-r4.summary.json)已通过原 owner、turn、隐藏输出与返回显示检查。

同轮 UI 的 [Local App 拒绝文案](../../acceptance/conversation-spaces-20261101/ios/git55-native-run/ui-04-r2.log)测试因 app-wide 查询同时命中主对话和已打开摘要中的工具按钮而失败；截图树确认它们属于不同视图，测试限定在摘要中并继续断言完整文案，[r3 重跑](../../acceptance/conversation-spaces-20261101/ios/git55-native-run/ui-04-r3.summary.json)通过。[Side Chat UI](../../acceptance/conversation-spaces-20261101/ios/git55-native-run/ui-06-r2.log)首轮启动失败暴露实际 Factory 缺口：runtime 选择隔离 managed root 时，native config home 仍取进程默认根，FFI 正确拒绝两个 authority 不一致。现已在 interactive/scheduled 构造前统一从选中 root 派生 account directories 与默认 scheduled cwd，保留 FFI 严格检查；配置回归、受影响 native 与 [Side UI r3](../../acceptance/conversation-spaces-20261101/ios/git55-native-run/ui-06-r3.summary.json)重跑通过。两次失败后的结果包未完成 finalization，原日志和 runner 结果保留，不能记为 UI 通过。

首次 iOS 失败中，独立只读诊断确认：重连测试需要等待隔离恢复 ledger 的查询完成；隐藏 Flow 的英文句点尚不形成语音 segment；清理期间导航测试的 fake 只提供一个 barge-in session，却提交了两个 Flow。对应受控测试已在当前 Full XCTest 批次执行；这些判断不覆盖其他失败，也不以宿主 stub 排除 native 问题。

Global New Chat 的 UI 失败中还发现两个测试条件：初始可能恢复到旧 Project，而 DEBUG mock 在新建时复用固定 session ID，触发真实 SessionHost 的跨 scope 保护。当前修正为每个新 mock 分配独立身份，并由实际 workspaceKey 提供 scope 标识；测试验证 Project → Global、原 mode 和草稿状态。上文的 gate 生命周期问题有实际日志证据，不能将首次失败整体归为夹具问题；修正后单项模拟器 UI 已通过。

## 关闭准则

1. SDK 对明确的预准入 busy 正确返回可重试并通过真实回归，未知/已准入结果保持保守；Android 重连等待权威 media drain 回执并复测未知/超时；iOS 保持同一 owner gate 跨 SwiftUI 重建并验证 Cron/导航互斥。SDK checkpoint 已固定，选定的 iOS simulator Cron/gate 回归已完成；Android 设备及未覆盖的目标组合仍须回归。
2. App resolver、生成协议、目标架构 FFI 和产品二进制的来源一致；对 successor 重新运行受影响的 SDK、Bridge、Desktop 与移动端检查。
3. iOS 首轮失败得到复测结果，Android/iOS 当前授权的设备或模拟器交互有独立记录；跳过、环境限制和未跑场景明确列出。
4. source、日志、包、设备、真实 provider 等验收层级分别报告；当前范围的 source review 和本地源码 checkpoint 已完成。acceptance 与 build 是本地工作区证据，未纳入该源码 checkpoint；未授权 push、merge 或发布。
