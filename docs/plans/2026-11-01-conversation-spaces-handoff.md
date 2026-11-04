# Chat / Code 会话空间：当前交接

状态记录：2026-11-03（America/Los_Angeles）。本页描述正在进行的集成及其验收边界，**不是最终通过声明**。产品契约详见[设计文档](../architecture/conversation-spaces-context-design.md)，独立审查和未关闭问题见[审查记录](2026-11-02-conversation-spaces-review.md)。先前 `55ddd4cf` 来源的源码 checkpoint 和模拟器重跑已完成；后续 main 同步的来源和证据边界见本页开头的同步记录。

## 2026-11-03 本地 main 同步

这次任务是把各仓库本地 main 与功能分支合并并回到本地 main，保留其他工作树的未提交内容；未 push。App 首次同步的双方来源是功能分支 `58eb629b1965435a79fc0278875091ae443cd36d` 和本地 main `46049b8ec3f386cb4e00e206da6ec652e89fca4b`；后者包含新的 Teams workspace、Bots 侧栏和未保存修改确认。开始时本地 main 领先 `origin/main` 的 `1f73edb020e7f35590e03818f43bbe01d31369a8`；回到本地 main 的最终 ref 与保全结果以任务目录 `merge-main-20261103/final-receipt.json` 为准。本地证据文件不随源码提交自动发布。

合并期间，本地 main 又推进至输入历史提交 `1fc322bd43059f6c46073f94172782e12044f60e`。已在首次合并提交 `264a88644b1ab20fa10a905e9ab2ea8a47e0b3aa` 之上继续纳入该提交；13 个文件自动合并无冲突。新增的真实 Electron 输入历史交互 1/1 和合并后的 web TypeScript 检查均通过，见 `prompt-history-latest-main.log` 与 `electron-web-typecheck-latest-main.log`。并行任务的内容以已提交 main 历史保留；原三项未提交改动继续排除。

Harness SDK 的功能分支和本地 main 已合至 `38ea730f503f318283cd4e26684f1abded10b6ec`，双亲为 `55ddd4cf880ed3de4696319c51b8429dcef80a9e` 和 `e0a8ae09607d361fc0b94d92fe51756238347302`，见[本地快进回执](../../../merge-main-20261103/harness/canonical-main-fast-forward.json)。SDK 合并工作树定向测试 122 pass、1 个 cache performance probe ignored；测试时暂存树与合并提交树均为 `e54249690af5e4c2934c9de2bae2f55c0e4118df`。App 47 处 Harness 引用指向 `38ea`；[locked/offline/all-features 来源证明](../../../merge-main-20261103/app-pin-proof.json)记录 1,052 packages 中 84 个 Harness 包均为 `38ea`、5 个 llm-client 包均为 `3cb37e3a3b840b54a4e92994fdc8b1edf7f592d3`、5 个 Mobile Linux 包均为 `43382a20161e371a0a31526412164b3d35545ab5`。该证明是依赖解析，不能代替产品编译或设备测试。

`llm-client` 本地 main 已与功能分支合为 `3cb37e3a3b840b54a4e92994fdc8b1edf7f592d3`，定向测试 151/151，见[测试日志](../../../merge-main-20261103/llm/cargo-targeted.log)。Mobile Linux 的本地 main 已快进至 `25cf8a273bcd572b9be48e18fc9807e01dcdfce1`；其仅修改两处测试 fixture，产品源码与父提交 `43382a` 一致，App 的 runtime pin 仍为 `43382a`。

本轮 App 侧相关检查：[Bridge 恢复和 runtime manager 组合测试](../../../merge-main-20261103/bridge-manager-final.log) 238/238、0 skip；Electron [web](../../../merge-main-20261103/electron-web-typecheck-final.log) 和 [node](../../../merge-main-20261103/electron-node-typecheck-final.log) TypeScript 检查通过；[转录/工具定向测试](../../../merge-main-20261103/transcript-focused-final.log) 104/104。Bots 侧栏单测 15/15、相关 UI/导航 SSR 54/54、使用 host fixture 的 Electron 侧栏交互 1/1 通过。翻译生成检查覆盖 2,752 keys / 5 locales，生成器测试 21/21；`check_i18n_pairing.py` 本轮仍报已有同名参数资源、`provider_accounts.xml` 和其他文案缺项，不能记为通过。

SDK `55ddd4cf` 到 `38ea` 的 client/mobile/core types 和 UDL 公共 ABI 检查无差异；llm-client message 结构无 schema 变化，Rust API 及持久化行为包含新的 logical message ID。未发现此次合并要求重新生成绑定，但现有 native 二进制仍源自 `55ddd4cf`，不能代表 `38ea` 的行为；运行新 SDK 仍须重新编译相应 native runtime，本次合并没有重新发包。

下文保留合并前 `55ddd4cf` 快照的完整 Android APK、iOS 模拟器、Desktop Bridge 和 SDK 验收记录，**不能将这些结果当作新 SDK 合并 pin 的重新打包或设备验收**。本次范围是本地 main 同步和必要的源码、定向检查，不额外重复全部平台原生构建；Android 新来源真机、iOS 物理设备/真实 provider 和签名包仍无相应新证据。

合并前[工作树保全清单](../../../merge-main-20261103/baseline.json)记录了任务 App 的两项 ProviderAccounts 未提交文件，以及主 App 工作树的 `Cargo.lock`、`ConversationDetailHost.kt`、`modUiAbovePrompt.tsx` 和其他目录；后续检查应按清单核实，不覆盖它们。

## 合并前 `55ddd4cf` 工作区快照

| 内容 | 当前快照 |
| --- | --- |
| 任务根目录 | `/Users/luolingfeng/lingxi/worktrees/code-chat-context-20261028` |
| App | 上述目录下 `lingxi-app`；iOS 源码 checkpoint `6bc55fe9f30b5f510b8a1255e661f97222fa9423` 的父提交是测试时的 `e5bb2ac98614a4f481c937b7ac5c0b60ee156ec0`。测试使用父提交加冻结工作树输入，源码 checkpoint 收录 11 项 iOS 修正与测试；不要把测试说成从提交后 HEAD 重新构建。两项无关 ProviderAccounts 文件在 Android 包和 JVM 测试时仍为 dirty，须原样保留。 |
| Harness SDK | 同级独立 checkout `harness-runtime-cron-validation-20261102` 已提交且干净，HEAD `55ddd4cf880ed3de4696319c51b8429dcef80a9e`，父提交 `46fb13b3449d02e595ec9fd49145d5de23bf66b1`。后继包含明确 ID 的 Stop/准入修复；[协议交接](../../acceptance/conversation-spaces-20261101/protocol/post-merge-20261102/protocol-55ddd-handoff-20261103.md)记录源码、测试与来源边界。原 `harness-runtime` HEAD `f286ff5cd05c720b82451a66dc01e88bca213ade` 的五处未提交改动须保留，不与独立 checkout 混用。 |
| Mobile Linux runtime | 独立 `mobile-linux-runtime` checkout 的产品来源为 `43382a20161e371a0a31526412164b3d35545ab5`；57 项源码清单 `build/rootfs-mode-workspaces-20261102/source-commit.json` 的 SHA-256 为 `c3b1bc8b3d77182cf724cd5bf425c6caaa2c4d63b4492e355e7891ff4f1fda30`。SDK 和 App 的锁定解析均指向该 SHA；独立测试提交 `25cf8a` 只修正两处 iOS 测试 fixture，生产树相同，见下方回归。Android 隔离包已包含核验过的双架构 rootfs，设备执行仍待验证。 |
| llm-client | 上述目录下 `llm-client`，当前提交 `0c2f041e856265e093e404d9b202c4238f73ef3c`。 |
| 证据 | App 内 [acceptance 验证索引](../../acceptance/conversation-spaces-20261101/validation-index.md)及同目录各平台记录；任务构建产物在同级 `build/`。这些 acceptance 与 build 材料保留在本地工作区，未纳入上述源码 checkpoint；链接用于当前工作树交接，源码提交本身不携带这些证据。 |

该验收快照中 App 的 47 处 Harness 引用锁定 `55ddd4cf880ed3de4696319c51b8429dcef80a9e`，4 处 Mobile Linux 引用锁定 `43382a`；`Cargo.lock` SHA-256 为 `4128060603475df74961172454291f2bc2d0b939a9f9995893ff2e436c89378e`。`--locked --offline --all-features` metadata 在 App 父提交上使用随后原样提交的 pin 文件通过；[来源核验](../../acceptance/conversation-spaces-20261101/protocol/post-merge-20261102/app-55ddd-locked-source-proof-20261103.json)记录 84 个 Harness、5 个 Mobile Linux 和 5 个 llm-client 包的精确提交。协议仍为 32；新 JNI、Bridge 和隔离包有下方各自的产物证据，不能把解析通过当成设备或全部平台验收。原始[历史快照](artifacts/2026-11-01-conversation-spaces-snapshot.json)只保存旧时点数据，已标注 superseded，不能作为当前 pin 或待办。

这是一组共享的 dirty worktree。继续之前先查看各仓库 HEAD、暂存区、未提交改动及活动构建，不能重置或覆盖他人的改动。常规 checkout、现有用户项目和业务数据均不在清理范围；`apps/electron/node_modules` 链接、Generated、target、构建产物和 acceptance 日志不因交接自动提交。大型 Cargo、Gradle、Xcode 任务需协调资源，停止线和实际磁盘余量重新核实；清理只限经路径、来源和使用状态确认的本任务可重建缓存，先保留日志与指纹。

合并前发现的品牌隐藏根目录改动已获用户明确授权纳入：最终合同为 Global Chat `~/.品牌/chat`、Global Code `~/.品牌/code`，配置保留 `~/.品牌`；项目显式目录不变，项目和 scheduled 的管理根仍在品牌容器内，CLI 公共默认 Code。SDK `55ddd4cf` 和 App 已锁定新目录来源；各平台仅以其实际产物及测试结果为准。不把混合工作树结果当作 immutable pin 的证据，也不称原 SDK 工作树干净。

## 不变的产品与身份契约

- Chat / Code 是独立导航空间，session mode 创建时固定；空间切换不是切换模型、权限或修改现有 session。普通 New Chat 使用当前空间的 mode，创建无 Project 的 Global 会话；只有显式项目内创建才绑定 Project。
- 空间、项目和历史入口按 mode 过滤，再做数量限制。切换页面只改变当前展示；已经准入的 turn、工具、审批、问题、source、草稿和音频继续属于原 owner。
- owner 至少由宿主命名空间、UUID、logical scope、fixed mode 与 source/engine incarnation 确定；turn、音频、导航操作还要携带各自的 token 或 generation。后台回调、迟到选择和通知不能改投当前屏幕的 owner；重复 claim 必须复用原 owner 或明确拒绝。
- logical scope 与运行 cwd 独立。`/cd` 不改变 Global/Project 或 mode；选择历史、恢复、handoff 和冷启动必须延续选中行捕获的精确 transcript path 和 authority，不能凭当前 cwd 或相同 UUID 的其他副本重新猜测。
- 当前格式只接受有效的 fixed-session-mode header 和当前 schema；未知/冲突 mode、损坏文件或缺失身份须拒绝，不默认 Code、不把坏目录当空状态覆盖。删除旧 API/reader/迁移并不授权删除用户数据。App 中的旧命令文本只用于拒绝旧命令的负向测试。
- 跨空间继续是从原已提交 cutoff 产生带来源链接、可编辑的 Global 摘要草稿，目标发送后才运行。独立 fork 使用已捕获的不可变来源快照与新 instruction epoch；之后原会话继续写入不会使已发布的分支失效。认证后台 handoff 保持原 owner/epoch，不混同于 fork。
- CLI 公共入口只提供 Code；私有已认证后台恢复可继续原捕获身份。配置、权限和后台派生按目标及捕获来源确定，不能由后来切换的 UI 推断。

## 持久化、准入与回执

- Desktop 的空间状态位于 `userData/conversation-spaces.v2.json`；Global 默认 workspace 按 mode 分别为 `~/.lingxi/chat` 与 `~/.lingxi/code`，配置仍在 `~/.lingxi`。Desktop 显式 `LINGXI_CONFIG_DIR` 可覆盖品牌配置容器，两个 workspace 应随容器解析；移动端 `~` 指 Linux 用户主目录，项目位于品牌容器的 `projects` 子目录。目录合同已进入 SDK checkpoint 和 App；新来源 Android 包已构建，各设备执行仍待验证。Android 使用 `filesDir/scope-state.json` version 2；iOS 的 `ProjectScopedPreferences` 按空间/scope/UUID 保存导航和草稿，项目及 Global 会话索引分别使用 `session-spaces.json` 和 `global-session-spaces.json`。Project 索引是否有效取决于完整 schema 与必要字段，不能仅凭 version 1 判为旧数据。
- Mobile Linux 新 rootfs manifest 必须含与运行 profile 精确一致的 `brand_profile`；缺失或不匹配时拒绝安装/启动。旧 release manifest 只能作为显式转换生成新产物的输入，不能在运行时默认为 LingXi。相应源码及派生物已有分层测试，Android 隔离包中双架构 rootfs 已核对；设备行为仍待验证。
- 当前 transcript 先写严格的固定 mode/UUID/epoch 首行；消息保持原有文本保真。冷恢复先捕获并验证 root/leaf、mode 和文件身份，再取得对应 writer/UUID claim。可观测替换必须拒绝或报告结果不确定。合作 SDK writer 受 lease 与各 scope 事务保护；不声称跨所有 scope 的原子 claim、跨文件 WAL 或抵御绕过锁的同用户进程在最后检查后替换命名空间。
- Desktop metadata 对话框和归档预检携带选中来源的 opaque receipt；热来源走原连接 writer，冷来源走持 lease 的私有进程。写入已接纳后，timeout、断线或窗口关闭不证明完成；应保留原 runtime/lease，等待权威 drain 或原进程退出，不重投到新来源。
- 移动端恢复账本按 session/turn 精确拥有。同步检查在消费 composer 前执行，异步准备与 Stop 必须在 native 提交边界作出同一准入决定；确定未准入时将草稿、附件与上下文 chip 恢复或保存在原 owner，并用 revision 防止 ABA 覆盖。ledger/witness 缺失、损坏、不匹配或写入结果不确定时保持 fenced；仅凭本进程已同步的精确回执进行允许的重试，不以文件形状猜测修复。
- Cron 新目标先预留 UUID/owner，再发布持久空锚点及 reservation，App 确认完整 receipt 后方可运行模型、hook 或 deferred tools。Selected/TaskSession 和 run receipt 必须带精确 UUID、path、mode、scope key；任务历史裁剪或删除不删除独立 owner reservation。取消等待原生 admission 清理并严格 reconciliation，结果未知不能伪装成未执行或自动重跑。
- Local App 的创建、源码编辑、预览、发布、Remix 和开发恢复只允许在 Code 空间发起。Chat 空间须在写入 intent、源码或 Project 之前拒绝并提示先切换到 Code，不能自动切换后继续原动作；浏览和运行已安装应用仍可用。Code owner 已接纳的操作在用户切到 Chat 后仍属于原 owner，应继续完成，不能取消、改投或事后仅因当前空间变化而拒绝。
- 移动 Local App 的首次创建和冷恢复须延续已登记的 authoring source、Project 与固定 Code session owner。SDK checkpoint 已实现不可变 owner receipt：新 owner 的登记或 current pointer 变化不撤销已有 owner，也不把运行中的 engine 改投其他会话；单次查询本身不是跨返回后的 lease。受管应用的首次发布及更新均须核对持久 sourceId、原源码身份和捕获的 engine owner，普通未绑定来源的应用保留现有发布能力。移除应用后其 tombstone 仍保留 sourceId 与原应用的唯一归属，源码、数据和 owner receipt 保留；创建另一个应用须通过可信流程取得新 sourceId 的副本，不能自动认领旧来源。恢复或 Remix 使用用户选中时捕获的 release revision；首建失败、结果不确定和冷启动中断须有可到达、可验证的恢复路径，最终 native 集成仍待验证。
- 普通 Remote 会话必须先确认固定 mode；未知或冲突时停止投影及控制。Team 单次审批是无会话 mode 的独立 capability，只允许对捕获的 request 做 allow-once 或 deny，不确定结果不得重新投相反答案。
- Desktop/CLI FileHistory 使用原始文件路径与父目录 authority；移动端没有 FileHistory UI/还原能力，本轮不扩展该功能。上下文中的外部资料、工具输出和旧消息不提升为新 Host/用户指令。

## 合并前 `55ddd4cf` 验证与限制

| 层级 | 本次可引用的事实 | 尚不能据此声明 |
| --- | --- | --- |
| SDK / 协议 | `55ddd4cf` 的 Stop/准入及相关回归 [35/35](../../acceptance/conversation-spaces-20261101/protocol/post-merge-20261102/successor-turn-cancel-r5-validation-receipt.json)实际通过：17 个 admission/Side/Cron/Pause 场景、5 个 fence、5 个原有 cancel、4 个 Bash、3 个 Pause 和 1 个 skill 冷恢复；7,777 项输入前后一致。前驱 `46fb13b` 的 [Local App 175/175](../../acceptance/conversation-spaces-20261101/protocol/post-merge-20261102/successor-localapp-clean-break-r2.log)、[tool-ui 24/24](../../acceptance/conversation-spaces-20261101/protocol/post-merge-20261102/successor-toolui-staging-replay-r2.log)及 mobile handler 测试按原来源保留。 | [Desktop scheduled draft 构建](../../acceptance/conversation-spaces-20261101/protocol/post-merge-20261102/successor-desktop-scheduled-draft-r1.result.json)曾触及 3 GiB 停止线，测试未运行；Windows 专属 alias/volume 用例未在本机执行。Android 当前包设备验证未完成；iOS 仅有下表列出的 simulator 测试。 |
| Mobile Linux runtime | 生产源码经单 lane 五包 Cargo 测试 166 pass / 1 ignored；installer 离线 JVM 10/10，见[五包结果](../../acceptance/conversation-spaces-20261101/protocol/post-merge-20261102/successor-mobile-linux-final-five-packages.result.json)及[installer 结果](../../acceptance/conversation-spaces-20261101/protocol/post-merge-20261102/successor-mobile-linux-final-installer-jvm-r2.result.json)。Mobile Linux iOS simulator [NativeRegression 42/42](../../acceptance/conversation-spaces-20261101/ios/git55-native-run/native-regression-25cf-r2.result.json)在产品源码 `43382a` 加仅更改两处测试路径的测试提交 `25cf8a273bcd572b9be48e18fc9807e01dcdfce1` 上通过；[来源](../../../build/ios-native-regression-25cf-20261104/test-source-provenance.json)记录生产树相同。 | [原 `43382a` 测试源首次执行](../../acceptance/conversation-spaces-20261101/ios/git55-native-run/native-regression-43382-r1.result.json)为 42 tests / 7 断言失败，不能写成原样通过；测试源修正后才通过。模拟器 stub 不是物理 iSH 启动或设备验收。 |
| Desktop default / Web | `55ddd4cf` 的[生产 Bridge 来源](../../acceptance/conversation-spaces-20261101/protocol/post-merge-20261102/successor-app-git55-bridge-production-provenance-r1.json)和[桌面综合回执](../../acceptance/conversation-spaces-20261101/desktop/sdk55-desktop-final-provenance-20261103.json)记录 binary SHA-256 `6786dea2ab6b4caaaf4ffba60b274b0303ee719b88eac4b3e576f379d3205fe7`；Local App、scheduled、durable draft 的私有 Bridge 测试分别 8/8、5/5、3/3，真实 Electron + Bridge 2/2，真实 Rust handler fixture 经 Desktop CoreClient/CoreStore 解码 1/1。前驱路径别名源码测试 108/108 按原来源保留。 | CoreStore 测试使用相关联的 fixture 重放，不是新的 live Rust handler；Electron 输入清单未枚举 Vite 全部运行依赖。正式签名打包、发布、真实 provider 和物理音频仍未由此证明；此前触及磁盘 guard 中止的构建也不能记为通过。 |
| Android Direct | `55ddd4cf` 真正的 ARM64 JNI 和三个生成绑定已用于隔离 Direct 包；[包核验](../../../build/android-native-validation/successor-jni-git55-20261103-r1/isolated-apk-validation.json)记录 193 suites / 1,653 JVM tests 全通过、0 failure/error/skip，instrumentation 编译完成。app APK SHA-256 `f0280b21bd13e65bbef8ea6dbf7fb2b80c80c3bd319e1af98fad01c03c09bcfb`，test APK `e4fd3b29c1ca8fbd1c639856c30df982cb8bf994c942641818704f7adca92a9f`；两包均无 INTERNET 权限，签名一致。构建时两项无关 ProviderAccounts dirty 源码仍在，见[回执](../../../build/android-native-validation/successor-jni-git55-20261103-r1/postpackage-validation-receipt.json)。 | 手机暂不可用；新包未安装或执行，设备 JNI、UI、音频及真实 provider 均未验证。旧 bc68 [设备分进程](../../acceptance/conversation-spaces-20261101/android/postfinal-direct-device-per-process-bc68-summary.log)为 2 pass / 3 fail，不套用新产物。 |
| iOS Full | `55ddd4cf` simulator FFI archive 和七个生成绑定已生成；原组装 wrapper 中断，随后以同一不可变输入手动执行 simulator-only 组装步骤，89 项 native 输入核对一致，见[组装回执](../../../build/ios-ffi-git55-20261103-r1/manual-assembly-sim-r1/receipt.json)。包含 Factory 和 mock 恢复修复的 [r4 Full 重建](../../acceptance/conversation-spaces-20261101/ios/git55-native-run/full-build-for-testing-r4.result.json)及[24 类 XCTest 批次](../../acceptance/conversation-spaces-20261101/ios/git55-native-run/app-host-batch-r4.summary.json)通过：535 pass、0 fail、1 预期 Store-only skip。另有 16 个独立 native 用例和 6 个 UI 用例按下方分阶段结果通过。build 基于 App `e5bb2ac` 加冻结工作树输入，[479 项清单](../../acceptance/conversation-spaces-20261101/ios/git55-native-run/full-build-inputs-r4-before.json)的内容聚合 SHA-256 `8640f838e445af43979f73b8610edfee8acdfceae5f2a54ae074e6894ebd680f` 且构建前后一致。 | 手动 simulator slice 不等于完整 wrapper、真机或签名包；运行中导航 UI 使用 mock，不能证明真实 provider。UI01 通过结果记录 1 个 SwiftUI reparenting runtime warning。bc68 首批 XCTest 424 pass / 9 fail / 1 预期 Store-only skip 与单项 [Project → Global UI](../../acceptance/conversation-spaces-20261101/ios/final-global-chat-ui-r7.log)均为旧来源。 |
| CLI | 本轮静态核对 [Code 初始化和固定 mode 校验](../../apps/cli/host/src/init.rs)、[headless 恢复](../../apps/cli/host/src/headless_host.rs)及 [Code 列表过滤](../../apps/cli/host/src/run/resume.rs)。 | 这不是新的 CLI 编译或运行；旧二进制和测试仍按各自历史来源引用。 |

iOS [最终回执](../../acceptance/conversation-spaces-20261101/ios/git55-native-run/final-receipt.json) SHA-256 为 `c09d2cdc9a1105daf8a5c1615683d67c7a0ff731c1c7445a279b0cd0cd5ddf79`，逐项记录来源、日志和结果。16 个独立新进程 native 用例中，01–06 和 11 取 [r3 序列](../../acceptance/conversation-spaces-20261101/ios/git55-native-run/native-sequence-r3.json)，07–10 和 12–16 取 [r2 序列](../../acceptance/conversation-spaces-20261101/ios/git55-native-run/native-sequence-r2.json)，各自实际通过，不重复计数。6 个 UI 用例分别取 [01 r4](../../acceptance/conversation-spaces-20261101/ios/git55-native-run/ui-01-r4.summary.json)、[04 r3](../../acceptance/conversation-spaces-20261101/ios/git55-native-run/ui-04-r3.summary.json)、[06 r3](../../acceptance/conversation-spaces-20261101/ios/git55-native-run/ui-06-r3.summary.json)及 02/03/05 r2；它们是分阶段来源，并非同一次完整 suite。01 为 mock 流与真实 owner 导航，06 通过实际 Side FFI。

历史 bc68 Android 隔离 app APK SHA-256 `29363f6972fe83b36e1b517227045279f70dce18dd4645f64303cc7f3a3f81c2`，test APK `132843e18dc4936f92334e9ba6393677ea188cd57c667158e881bedb2000fe49`。当时的 657 项构建输入清单见 `android/postfinal-bc68-android-source-before-v4.sha256`；这两包不代表当前来源。iOS 运行日志和命令记录在 acceptance/ios；部分旧 xcresult 与生成文件已按授权清理，其原始测试摘要和清理清单保留在 [iOS 清理证据](../../acceptance/conversation-spaces-20261101/ios/old-test-output-cleanup-20261103/removal-result.json)，不能假定所有原路径仍存在。其他历史数字和资源受限的直接二进制运行只按原记录所属层级引用，不能改写为完整 Cargo/原生测试通过。

新 Local App 候选还使用 SDK 真实 mobile handler 输出的同一份 fixture（SHA-256 `9cea0f8bbd647586c0ccc4bf999d5d2a0470f8db549b0d256c3e4e0f46bd0277`）：Android [DTO/store JUnit 3/3](../../acceptance/conversation-spaces-20261101/android/native-authoring-fixture-candidate-r2b-junit.log)，iOS [Swift host 6/6](../../acceptance/conversation-spaces-20261101/ios/native-handler-fixture-r2-ios-host-r1.log)；iOS 另有[失败准备和原 ZIP 暂存 host 17/17](../../acceptance/conversation-spaces-20261101/ios/ordinary-preparation-market-zip-host-20261103-r3.log)。这些测试使用平台或 writer stub，不能替代新 JNI、FFI、模拟器或设备结果。

两份已关闭的历史 bc68 CLI / Bridge 验证二进制已从原路径压缩归档，原路径不再直接可执行；如需复用，先按[归档及恢复元数据](../../acceptance/conversation-spaces-20261101/root-old-host-binary-archive-20261103.json)恢复并核对指纹。压缩文件及各自的 `.restore.json` 位于任务根目录 [retained-bc68-host-binaries-20261103](../../../validation-artifacts/protocol-post-merge-20261102/retained-bc68-host-binaries-20261103/)；临时恢复的字节 SHA 和 codesign 检查均已记录，原有源码、包、日志与 `.d` 文件未随之移除。

为缓解磁盘不足，364 个经 14 次运行记录、SHA、inode 和 `.d` 来源核对的旧 mutable SDK 可重建 `.rlib`/`.rmeta` 已按[清理回执](../../acceptance/conversation-spaces-20261101/protocol/post-merge-20261102/sdk-path-rlib-rmeta-removal-r2-result.json)移除；观察到可用空间增加 2,424,832,000 bytes。清理后源码和依赖信息已复核，`.d`、测试可执行文件、源码、日志和包证据均保留。实际空余和活动进程仍须在后续构建前重新检查。

已完成的四次用户授权清理见[汇总回执](../../acceptance/conversation-spaces-20261101/protocol/post-merge-20261102/root-authorized-build-cache-cleanup-summary-20261103-r2.json)：共 18,873 项，各次观测的空余增量合计 5,115,904,000 bytes；并行文件系统活动使其不等于精确物理回收量。JNI、archive、绑定、包、`.d`、日志、来源证据及各次列出的受保护项保留，旧 Android 外部符号链接目标未递归核验。后续清理以独立回执为准；构建前须重新检查实时空余空间。

另一次已授权的 [Desktop 旧输出清理](../../acceptance/conversation-spaces-20261101/desktop/old-output-cleanup-20261103/result.json)移除三份已验证 ZIP 对应的解包应用，保留三个 ZIP 与 272 项保护材料；内容、模式和符号链接目标已核对，扩展属性未核验。清理前盘点目标的 APFS 独占占用约 2.94 GB；观测可用空间变化包含并发活动，不能等同于该操作的精确回收量。

Android bc68 离线隔离包的手动设备观察确认了 Global picker、Chat/Code 切换，以及 Chat 中 New Chat 保持 Chat/Global；截图和层级见 [Android 状态](../../acceptance/conversation-spaces-20261101/android/postfinal-direct-validation-status-20261102.md)的 Device follow-up。用户又授权在该隔离包私有目录创建空项目 `Context32-NewChat-Verification`：手动观察到选择 Project 后普通 New Chat 回到 Global，Code mode 保持，前后[界面层级](../../acceptance/conversation-spaces-20261101/android/postfinal-device-manual-project-before-newchat-bc68.txt)和[结果层级](../../acceptance/conversation-spaces-20261101/android/postfinal-device-manual-project-to-global-bc68.txt)已保存。这些是旧目录策略下的 bc68 观察，新 SDK 新目录与设备上的 media drain 仍须重验；它们不是 instrumentation 通过，也未覆盖真实模型或运行中的 turn。空索引 fixture 已包含在重新编译的当前 test APK 中，尚未在设备执行。

## 合并前未关闭事项与续作顺序（历史记录）

SDK `55ddd4cf` 的 Stop/准入修复及 App pin 已提交本地 checkpoint；Android 当前 JNI、隔离包与 JVM 和 iOS 分阶段模拟器回归均有上表证据，物理设备和真实 provider 验收尚未完成。具体复现、已测范围与未完成项见[审查记录](2026-11-02-conversation-spaces-review.md)。`46fb13b` 的 JNI、FFI 和旧设备结果均为前驱证据，不能冒充当前 pin 的验证。

1. SDK `mobile/host.rs` 三个 public Cron 入口的 `begin_owner` 若在任何 claim、ledger、transcript 或工具执行前返回 false，bc68 经 `fired_cron_dto` 将其标成 `retryable=false`。这是已在真实 iOS simulator 用例中捕获的预准入分类错误。callback 的 Bool 不区分暂时占用和 catalog/prepare 失效；只证明同一 occurrence 尚未执行。在锁内重新核验 occurrence/authority 后才可安全重试，后续仍受 Host 校验，不保证拒绝是暂态。本轮不扩 callback ABI。修复已进入 SDK `46fb13b`；当前 SDK 的模拟器 native 回归已覆盖选定的 scheduler 准入及 callback 期间取消场景，仍应按实际入口和设备分别验证，不把已准入或结果不确定的失败宽泛改成可重试。
2. 在 REDMI 可用时，对上表的当前隔离包完成已经授权的 USB headless/UI 测试，记录设备、包哈希、命令和结果；优先复核 media drain、重连、目录授权、终端关闭/后台输出、logical scope 和普通 New Chat。`55ddd4cf` 的 JNI 和 APK 已构建，bc68 设备结果仍是历史证据，不冒充当前产物。
3. iOS 的 Cron owner gate 已改为与持久的 SessionHost/ProjectStore/CronRepository 保持同一 SwiftUI 生命周期；当前 pin 的选定 Full、隔离 FFI、UI 与 Mobile Linux simulator 回归结果见上表，按实际阶段、跳过、fixture 来源及硬件限制引用。物理 iOS 设备、iSH、真实 provider 和音频仍待相应目标验证；移动 Local App 的不可变 owner receipt、首建/冷恢复及失败重试须按实际 native 覆盖报告。
4. 按 Desktop default、Android Direct、iOS Full 分别核对固定 mode、Global 新建、运行中导航、审批/音频 owner、exact path 恢复、Cron reservation/cancel 和失败输入保留；用真实目标执行的证据关闭相应场景，区分 mock、host、native、package、device 和 live-provider 层级。
5. 后续操作前核对源码/产物指纹并保留其他 dirty 工作。源码 checkpoint 已在本地完成；未经新的明确授权不 push、merge 到 main 或发布。
