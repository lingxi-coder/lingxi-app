# Chat / Code 上下文与跨端集成交接

## 2026-11-02 独立复审后收口（优先于下方 d4a6 检查点）

iOS native 源码已按清单提交 `ca3113890`，仅 EOF 空行修正为 `cff83c61b`，没有提交 Generated 或证据目录。独立复审冻结的 342 项 Sources/Tests/project 输入聚合 SHA-256 为 `7e57b509cf6d7b525494c5562d195ad7d802258e6e804bf62715ac082d32df06`。真实 actor/存储/SessionHost 宿主套件 206/206；Full 产品、unit 与 UI 源码类型检查通过的检查点，以及最后测试夹具/EOF 差异均在 iOS evidence 中分别记录。它们仍不代表 simulator ABI/link/runtime 已验收。

独立复审继续修复：Android admission/Stop 的精确未提交恢复、Side/Remote 完整失败稿和 ABA、当前空间 Global New Chat、抽屉 exact path；Desktop pending open 的固定 mode 校验、metadata token 显式释放、已接纳写跨 timeout/disconnect/quit 的 drain。CLI 非 fork 恢复需从目标 workspace 加载项目配置，后台 add-dir/config home 按调用来源固定，TUI deferred replay 不能在审批界面启动前等待。上述晚修尚待最终整合验证。

SDK 通用 deferred replay 的来源检查修复已提交为 `bc68dcfa5ebcd78ab23f877a433ca83c81bb4663`（parent d4a6），仅四文件，工作树干净、未推送。每次派发前检查当前 writer 的 authority；新门禁为 deferred 6/6、private Chat 1/1、compaction 1/1。runtime cold replay 1/1 使用本次冻结源码编译、Mach-O/签名与 binary/dep-info/source hashes 验证的二进制直跑；其 Cargo 链接命令曾被 2 GiB 保护中断，不能算 Cargo 命令通过。**d4a6 及其双端主机绑定是中间检查点，App 正重新固定 bc68dcfa、生成并验证协议和绑定，三端真实验证仍须完成。** SDK 未修改 C ABI、协议/FFI schema 或存储格式；不得仅据此跳过 source identity 和实际生成验证。

## 2026-11-02 d4a6 SDK 与原生绑定检查点（历史）

Harness 最终提交为 `d4a6af84d0523102590622472e4d4a1149ba933d`，工作区干净；App 的 47 个 manifest pin 与 locked resolver 的 84 个 SDK crate 均对齐该提交。LLM 固定为 `0c2f041e856265e093e404d9b202c4238f73ef3c`。最终 source identity 校验、官方 TS 生成与 `--check` 均通过，协议为 32。此前 9c8f/186ecf40 均仅为历史基线。

SDK 最终 session 为 487 unit + 177 integration，共 664 项通过；client adapter 416、rooted-fs 36、WriteTool 34 项通过。runtime 受影响定向 7 项通过，复用的可执行文件与最终 production 源码一致，仅两处 `cfg(test)` fixture 有差异，不能描述成最终 diff 的全新 runtime 重编译。原始证据分布在 `sdk/successor-577f1050/`、`sdk/successor-d5641ff1/` 与 `sdk/successor-c623e277/`。修复包含空锚点 metadata、发布后不确定状态的持久 writer fence、FileHistory 原路径/parent authority、captured rewind 与 fork 目标首次 mount 收据。

最终 d4a6 iOS host staticlib 与真实 Swift metadata 生成均退出 0；7 个 Swift/header/modulemap 文件已推广到 `apps/ios/native/Generated`，输入指纹前后一致。这不等于 simulator 目标架构库、Full 链接或 XCTest 已完成。Android host metadata、实际 Direct JNI/Gradle/设备与 iOS Full 仍在闭环。

Desktop 最新完整 default suite 为 3880 pass / 4 fail / 16 skip。提交 `c8c8a3b8e` 修复其中两个 fixture 合同问题，相关文件 29/29 通过；另外两项依赖真实 Bridge binary 与 Rust producer Cron fixture，仍待最终产物。该完整运行使用 provisional pin，不能充当最终原生联调结果。

移动端当前仍有授权范围内未提交实现：Android admission/恢复与异步导航；iOS async send、durable recovery、草稿 revision、原 owner 音频、guarded Project/Cron 存储及 exact selected transcript path。隔离 JVM、宿主 Swift、parse 与类型检查分别记录，不互相替代。独立审查与真实单 flavor 验证继续进行，未推送，未合入 main。

## 2026-11-02 先前合并后续作（历史检查点）

当前 App 包含 main 合并 `0b2c85ceac1568fe76c084eaecaaf64a8dc74766`、Desktop 调度与导航修复 `484104889`、精确目录来源 `c4adeaa69`，以及 metadata writer/连接归属修复 `767993fca`。Harness 当前 SDK pin 为 `9c8f4f6c4c6cffd2d14056225ebd80d2210b0efe`，LLM 为 `0c2f041e856265e093e404d9b202c4238f73ef3c`；App 已更新 pin 与 lockfile。**独立复审发现 rename 空锚点丢失、retry/compaction 发布后错误被吞的缺口，SDK 正在集中修复，9c8f 现仅为 provisional pin。App 仍有授权范围内三端与协议修复待验证提交。未推送。**

新 SDK 冻结源码 session lib 464/464、integration 177/177、fresh Desktop runtime authority/fork 1/1 通过。BridgeClient 全套 360 pass / 5 Firebase emulator skip；最后追加 socket owner guard 后真实 WS 10/10、build/typecheck 通过。metadata subprocess observer 9/9 为注入进程测试，不能代替 Rust writer/lease 验证。最终两端绑定、目标架构库、Desktop Host/UI 和原生设备验证尚在推进。

独立审查进一步确认并正在修复的合同：

- 移动冷恢复必须同时提供 UUID、固定 mode 和持久化的 exact transcript path；路径保留 SDK 给出的 guest/host 坐标，客户端不推导。SDK 负责可信映射、目录组件 no-follow 及 writer authority 校验；缺路径不能退回 UUID 搜索。
- selected-session 调度持久化 ID/path/mode/opaque scope key；new/task-session 显式保存 session mode/scope key。claim、首次绑定及结果必须使用同一执行快照，不能从后来编辑的 task 或 cwd 重建逻辑归属。
- 原始 `Automation.runs` 会被截断或随任务删除，不能充当永久 owner 证据。SDK 正加入独立持久 reservation ledger，以及 before-model owner-confirmation；分配 owner 不等于已执行。ledger 与 task 是分步持久化，不能宣称两个文件同一原子事务。
- App 在短 admission gate 内确认并持久化 owner，确认后立即放开新会话，模型长 turn 不持全局 gate。取消需等待该次 admission 的 native 终结屏障，之后严格重扫 ledger 或冻结新 claims；既有 owner/turn 不停止。
- Desktop/移动的冷、热恢复都必须把首次捕获的文件 authority 贯穿到 writer bootstrap/activation。独立 fork 捕获新 target authority；合法 compaction/relocation 在事务中迁移 authority。

Desktop 这次提交保留跨空间编辑时的可信任务模式，调度历史打开在 Host queue、runtime activation、renderer generation 和页面回调各自校验归属。新建的全部调度会话在首次绑定前持久化空锚点。175 项调度相关回归和前后端类型检查通过；精确目录 remount 与 metadata 相关 145 项定向和 Node types 通过。完整 provisional Desktop 为 3841 pass / 3 fail / 16 skip，其中 Bot test double 已另行修复并整文件 24/24 通过，剩余需最终 Bridge binary 与实际 Rust producer Cron fixture。最终 pin 后还要重新验证真实集成。

验收范围不变：Desktop default、Android Direct、iOS Full，各一个 flavor。最新证据见 `acceptance/conversation-spaces-20261101/validation-index.md`。此前 Desktop 3576/12 skip 与 Web 286/3 skip 是对应源快照的通过结果；新的 Cron DTO 和来源合同改变后仍需重跑受影响检查。iOS host 与 Android 隔离 JVM 有增量通过记录，但不等于真实 FFI/原生设备验收。

旧 pin 的 iOS host staticlib 构建已在尚未到达 FFI crate 时安全停止，没有生成新库或绑定；退出 `-2` 是 superseded-source 中断，不能算通过。SDK `desktop,mobile` lib-test 编译曾通过，但其后又有 source/ledger 修复，因此仍需最终正确 feature 组合的测试。绑定必须等最终 SDK 提交、App locked resolver 后重新生成。当前 TS 生成仅 provisional，用于并行排查源类型，最终必须走 resolver 的官方 `--check`。

## 2026-11-02 main 同步状态（优先于下方历史快照）

按用户要求，先将各仓库的 `origin/main` 合入任务分支，保留主线账户、MCP、界面及其他修复。App 合并前的功能检查点为 `d3a32a21d`；不覆盖常规 checkout，不推送或合入 main。

- llm-client 合并提交：`0c2f041e856265e093e404d9b202c4238f73ef3c`。
- Harness 合并提交：`aad53cf713ba091c0c430d459bc92c792d5d70c4`，协议仍为 32。
- App 三端文本冲突已解决；最终 Cargo 离线锁定依赖解析通过，84 个 SDK 包统一为上述 Harness SHA，5 个 llm 包统一为上述 llm-client SHA；TS 生成结果与暂存文件一致。App 合并提交包含本段状态。
- 合并中修复了账户准入与 realtime handoff 的交互：分别保留捕获的 session profile、Host 选择的逻辑 audio profile、实际账户 profile/scope；不覆盖 session/mode/epoch/history。

下方所有旧 pin 的测试数字均为**合并前基线**，不能作为新合并提交的验收结果。合并后的轻量检查已覆盖 Desktop 429 项通过/1 项跳过、iOS 81 项宿主测试、Web 149 项；完整编译、重生成两端 FFI、Bridge durable scheduled anchor 修复及行为回归、Android Direct 原生设备和 iOS Full 模拟器验证、最终 review 尚待完成。每个平台仅验证一个 flavor。

交接日期：2026-11-01（America/Los_Angeles）。

**当前结论：协议 32 的本地不可变依赖与主机绑定已经固定，最终 Rust、目标架构及设备验证尚未闭环，不能标记为已交付。**

续作已将 Harness 本地提交固定为 `4676b43f7b3aee5b70a7ff2bac684f01643fca60`，并将 Harness 与 App 的 llm-client 固定为 `772612ca8124c87774eeedbfd8df5fe0258779a9`；两个提交均未推送。App 的 locked/offline resolver 与 all-features metadata 校验通过，确认没有混用这些依赖身份。TS 协议按该 SDK 生成，bridge-client 完整测试为 350 通过、5 跳过；Swift 和 Kotlin 已从同 pin 的真实主机 FFI 元数据生成，输出 SHA 和先前绑定备份均已记录。iOS Full 最终源码 typecheck 已重新通过，Android Direct 最终 JVM 测试为 1451 通过；主机绑定不代表 simulator 静态库或 Android ABI 库已验证。当前验收范围为 Android Direct、iOS Full 和 Desktop default。

磁盘压力导致首次 App 四根 crate 检查被安全中断；经确认没有活动使用者，仅任务专用 Rust target 的可再生缓存已清理，源码、生成绑定和证据保留。重跑后四根 crate 的 lib/test 检查及 CLI/Bridge 生产二进制检查均通过；此后又补了 Bridge 冲突模式参数的拒绝，需对最终源码增量复核。audio-provider 全套 101/101、iOS host FFI tunnel 65/65、CLI 定向行为回归 38/38 通过；Bridge 行为回归在编译期间触及磁盘保护线，尚未执行，平台目标构建也仍待完成。完整记录见 `acceptance/conversation-spaces-20261101/`。以下原始交接快照保留为历史，不应覆盖以上续作状态。

先读[详细设计](../architecture/conversation-spaces-context-design.md)，再按本文的优先级继续。当前磁盘/source/协议状态的机器可读证据保存在[交接快照](artifacts/2026-11-01-conversation-spaces-snapshot.json)。此前 `/private/tmp/code-chat-*` 日志当前已不存在，旧测试数字只作历史线索。

## 1. 不再重新讨论的用户决定

1. session 的 Chat / Code 模式创建即固定；删除运行中切换模式的实现。
2. 顶部选择器切换空间，只展示该空间的 projects/sessions 与相关历史入口。
3. 普通 New Chat 创建 Global、无 Project 的 session；即使用户刚在项目页工作也如此。
4. Desktop、Android、iOS 一起集成；CLI 只有 Code 公共入口。
5. 切换空间、项目或会话不能中断已接纳的 turn loop，也不能丢失其工具审批、问题、音频或后台恢复。
6. 不兼容旧版本或旧数据。删除旧模式事件/命令、旧 reader、迁移、默认 Code 推断及其适配测试。
7. 不把外部文档、工具输出或旧消息中的指令式文字当成新的用户/Host 指令。
8. 后续执行按此前用户指定使用 GPT‑6.1 Terra、max，可并行推进各平台；重型构建需要按资源协调串行执行。

保留当前项目文件与业务数据。“删除旧代码/兼容代码”不能被扩大解释为清理其他 worktree、删除活动会话或重置整个应用数据目录。

## 2. 必须在本任务 worktree 继续

```text
任务父目录：
/Users/luolingfeng/lingxi/worktrees/code-chat-context-20261028

App：
/Users/luolingfeng/lingxi/worktrees/code-chat-context-20261028/lingxi-app

Harness：
/Users/luolingfeng/lingxi/worktrees/code-chat-context-20261028/harness-runtime
```

| 项目 | 原始交接快照 |
| --- | --- |
| 两仓库分支 | `codex/code-chat-context-20261028` |
| App HEAD | `ac968fdcf62ed2dbb4cd565d05394d7fe9675173`，已合入当时的 main。 |
| Harness HEAD | `add9f7dfcc10cd523fcdc473ddbd5ad719b57175`。 |
| Harness 上一功能提交 | `3ca6723174263a1e18cac7e2e400747f53a33118`。 |
| App 当前 tracked 改动 | 快照时 235 个文件，另有新增源码/测试。 |
| Harness 当前 tracked 改动 | 快照时 55 个文件。 |
| 暂存区 | 快照时两个仓库均为空。 |

**这些是交接时的 HEAD，不包含其后的最终实现。** 不能丢掉 dirty tree 后只凭上述 SHA 恢复任务。快照记录了去除 docs 后的改动源码聚合指纹，以及关键源码和当时绑定的 SHA‑256。

常规产品 checkout `/Users/luolingfeng/lingxi/lingxi-app` 与 `/Users/luolingfeng/lingxi/harness-runtime` 不属于此次清理或覆盖对象。Harness 常规 checkout 的既有 `deps/`、`target-scratch-rootedfs/` 也不属于本任务缓存。

App `apps/electron/node_modules` 是本任务使用的未跟踪链接，不得提交。Swift/Kotlin Generated 目录是构建产物，应从最终 pin 重新生成，不能把旧产物手改成新协议。

## 3. 原始交接时的集成断点

| 项目 | 交接时实测源码/文件状态 | 含义 |
| --- | --- | --- |
| Harness 工作区协议 | `32.0.0` | 新 DTO 与固定模式格式已写入 dirty tree。 |
| App Cargo Harness pin | `add9f7df…` | 指向的已提交 SDK 仍为协议 `31.0.0`。 |
| bridge-client TS | `32.0.0` | 已与工作区新协议对齐，仍需最终 pin 校验。 |
| Android generated client | 仍含 `SessionModeChanged` | 旧绑定，不能证明当前 Kotlin 集成通过。 |
| iOS Generated/client.swift | 仍含 `sessionModeChanged` | 旧绑定，不能证明当前 Swift 集成通过。 |

继续顺序必须是：**修复 SDK 当前失败 → 用当前源码完成必要回归 → 固化 SDK 提交 → App 改为该完整 SHA → resolver/lockfile 校验 → 生成协议与两端 FFI → 原生编译和回归**。

不要通过 Cargo path patch、邻近 checkout 或手改绑定绕过这条链。

交接快照之后，实时音频输入的驱动边界修复产生了新的 llm-client 本地提交；SDK 与 App 已一起固定到该提交，避免同一进程解析出两个 llm-client 类型身份。该提交尚未推送，其他依赖保持原 pin：

```text
llm-client    772612ca8124c87774eeedbfd8df5fe0258779a9
mobile-linux  bfd17e346f7ebbb2bcd5206f3c05f56b83a1858b
Askama        d47daf8767805c0c4dfb456145fc857bfd9c1d2c
```

## 4. 各层实现进度

“已写入”只说明有源码和相应测试；下表不等于最终编译、运行或设备验收通过。

| 范围 | 已写入内容 | 尚需关闭 |
| --- | --- | --- |
| SDK 固定模式 | 固定 header、只读 mode snapshot、required new/resume mode、旧事件删除。 | 新 parser/UUID 用例及最终协议合约重跑。 |
| 上下文 | Chat/Code profile、manifest、Skill 来源/epoch、模型排除、静态 prompt snapshot 边界。 | 新 fork/恢复路径与现有 compaction 的最终回归。 |
| SDK owner | UUID claim、冷恢复、精确 transcript source、调度借用与 drain。 | cron 隔离进程及 Desktop durable anchor 实际运行。 |
| Desktop | 独立空间存储、Global New Chat、UUID runtime、后台 lease、handoff 与调度创建。 | 最终 SDK pin 的 Bridge 联调；签名包与真实界面验收。 |
| Android | SessionHost/OwnerRegistry、导航 fencing、草稿/附件、多 owner recovery、engine-scoped CU/audio。 | 新 FFI 下 Direct 编译、最终 JVM 与设备用例。 |
| iOS | MainActor SessionHost、按 owner 保留 source/草稿、release receipt、通知/Live Activity 与远程 gate。 | 新 FFI 下 Full 编译、测试、链接及设备用例。 |
| CLI/Bridge | 公共 Code-only、expected-mode 校验、background fork epoch、私有 scheduled anchor flag。 | 最终 pin 下 Rust production/test 编译及回归。 |
| 远程共享层 | mode 确认/冲突拒绝、写入 Result 传播、Team 单次审批例外。 | 两个遗漏 fixture、共享 crate 与 FFI tunnel 测试。 |
| realtime | local input admission、typed Agent terminal、OpenAI/Gemini 完成证据、采集 gate。 | 最新源码实际回归；真实 provider 和设备音频。 |

## 5. 本次重新确认的待修复项

### P0-1：依赖与生成产物不是同一协议

见第 3 节。SDK 未提交改动先完成检查再固化；App 的完整 pin 与 lockfile 同步，随后从解析出的真实源码重新生成。不能把“TS 可编译”当作 native runtime 已更新。

### P0-2：跨空间 handoff retry 测试仍使用旧 fixture

文件：Harness `crates/runtime/src/handoff.rs`。

测试：`explicit_retry_uses_the_original_capture_and_prepare_never_retries`。

当前 fixture 直接以 user 消息开头，缺少当前必须的 `fixed-session-mode` 首行。前序执行记录报告 mock provider 调用次数为 0，而断言期望 1；当前源码仍可确认这一 fixture 缺陷。

处理：给测试 source 添加真实 UUID、Code、epoch 0 的固定首行；保留测试原本“Prepare 不暗中重试、Retry 使用原 cutoff”的断言。然后重跑整个 `handoff::tests`，不能降低断言或恢复缺 header 默认 Code。

### P0-3：远程集成测试遗漏 mode

文件与入口：

- App `packages/remote-control-client/tests/e2e.rs`：snapshot begin 的 fixture。
- App `packages/remote-control-client/tests/bots.rs`：`hold_records`。

这两处当前仍不给普通会话提供 mode，新 gate 会保持 Loading，原 Live 断言或等待将无法成立。补入当前 wire 字段的明确 Code mode，保持原测试语义；参考 `tests/session.rs` 已更新的 fixture。

**不能修改生产 gate 让未知 mode 自动变成 Code。**

### P0-4：最新补丁缺少最终运行证据

严格 header、independent fork、cron stack、Desktop anchor、最新 realtime frame-tail、远程冲突/Team capability 和移动端最后一轮提交保护均有晚于部分历史测试的改动。需要以最终源码重跑，不能直接继承旧通过数。

## 6. 已处理的 code review 问题

以下修复已进入源码；仍由第 5 节及验证清单决定是否关闭：

| 原问题 | 当前处理 |
| --- | --- |
| 切换空间销毁正在运行的 source | 提升到进程级 owner host/runtime manager。 |
| `/cd` 被当作更换逻辑项目 | UUID/scope/mode 固定，cwd 单独更新。 |
| slash/Fusion 尚未显示 streaming 时被回收 | 发送前建立 pending lease，并按相关终态/权威 roster 移交。 |
| AttachTurn 的同步事件丢失 | Android 先完成订阅 barrier。 |
| 旧终态清除新 turn recovery | checkpoint 带 owner/turn，精确匹配后清理。 |
| Computer Use 或音频回调落到当前屏幕 | engine/operation owner 注册，不使用 currentEngine 兜底。 |
| 独立 fork 继承 Skill 权威和计算 prompt | 新 epoch、退役复制激活、丢弃 computed snapshot；exact handoff 保留。 |
| JSON 对象重复键被普通 map 解析吞掉 | 固定首行强类型 deny-unknown-fields，验证重复键与 UUID。 |
| 绑定调度 UUID 后尚无可发现 transcript | 固定首行后持久化空锚点，再确认 bind。 |
| cron worker 栈溢出 | 8 MiB stack 与大型 future 装箱，增加隔离进程回归。 |
| Provider complete 提前结束 Agent | typed local terminal，核对 response/frame/interaction status。 |
| 远程未知模式就允许输入 | 等待确认，冲突保持 Error；FFI 传播写入失败并保留草稿。 |
| Team 审批因没有 Chat/Code 被阻断 | 精确保留 ID 分类，仅放行单次 permission.respond。 |

## 7. 建议的并行责任边界

快照时没有仍可复用的活动子 agent；需重新建立工作分工。所有 worker 都必须知道共享 dirty tree 不可回滚他人改动。

| 工作线 | 文件/责任 | 不应修改 |
| --- | --- | --- |
| SDK / CLI | Harness core/session/client/runtime 与 App CLI/Bridge 的相关测试。 | Electron、Android、iOS UI。 |
| Desktop | `apps/electron/**`；Bridge 接口变动先协调。 | SDK realtime 与两端 native。 |
| Android | `apps/android/**`，新绑定之后的 Direct 与测试。 | Swift、共享 remote controller。 |
| iOS | `apps/ios/**`，新绑定之后的 Full 与测试。 | Kotlin、共享 remote controller。 |
| realtime | Harness realtime driver/test 与 `apps/audio-provider/**`。 | 空间存储和导航 UI。 |
| shared remote / review | `packages/remote-control-client/**`、`packages/mobile-ffi/src/remote*`。 | 两端页面；需要跨端改变时先说明合约。 |

协议、Cargo pin、FFI 生成由一人统一协调。静态检查与轻量测试可并行；Cargo codegen、Gradle 与 Swift/Xcode 大型构建避免同时占满磁盘与内存。

## 8. 恢复工作后的第一轮检查

以下为待执行命令，不代表本次文档工作已运行这些构建。

```bash
export TASK_ROOT=/Users/luolingfeng/lingxi/worktrees/code-chat-context-20261028
export APP_ROOT="$TASK_ROOT/lingxi-app"
export SDK_ROOT="$TASK_ROOT/harness-runtime"
export EVIDENCE_ROOT="$APP_ROOT/acceptance/conversation-spaces-20261101"
mkdir -p "$EVIDENCE_ROOT"

cd "$APP_ROOT"
git status --short --branch
git diff --check

cd "$SDK_ROOT"
git status --short --branch
git diff --check

df -h "$TASK_ROOT"
```

再检查任务目录下实际运行的 Cargo、rustc、Gradle、Xcode 和测试进程；不能根据旧会话的 session id 假设构建还在运行。不要直接调用旧工具 session id。

默认 `python3` 曾是 3.9，缺少 `tomllib`；相关脚本使用 `python3.11`，或为构建建立任务专用 PATH shim，不替换系统 Python。

## 9. SDK 测试优先级

先修第 5 节 fixture，再运行。资源有限时使用：

```bash
cd "$SDK_ROOT"
export CARGO_PROFILE_DEV_DEBUG=0
export CARGO_PROFILE_TEST_DEBUG=0
export CARGO_INCREMENTAL=0
export CARGO_BUILD_JOBS=2
export CARGO_TARGET_DIR="$TASK_ROOT/validation-runtime-target"

cargo test --locked --offline -p session --lib -- --test-threads=1
cargo test --locked --offline -p client
cargo test --locked --offline -p orchestrator --test realtime_agent_test -- --test-threads=1
```

runtime Mobile 的定向组使用相同参数前缀，例如：

```bash
cargo test --locked --offline -p harness-runtime --lib \
  --no-default-features --features mobile \
  handoff::tests -- --test-threads=1
```

同样覆盖这些 filter：

```text
mobile::host::tests::initial_profile_tests
mobile::host::tests::session_owner_tests
mobile::transcript::owner_tests
mobile::host::shutdown_tests
mobile::host::automation::owned_cron_tests
mobile::host::cron_automation_tests::cancelled_automation_waits_for_non_abortable_cleanup_before_persisting
committed_source_tests
idle_scheduled_run_survives_default_thread_stack_in_isolated_process
```

最后一个隔离进程用例必须验证实际调度 assembly，不只是 thread builder 配置。若 feature 或测试名称调整，先用 `-- --list` 确认目标存在；**0 tests 不算通过**。

Desktop anchor 在 mobile-only build 中不会被编译。需要启用 desktop 的 SDK lib 测试，并实际运行：

```text
scheduled_draft_is_discoverable_before_desktop_build_returns
scheduled_anchor_is_idempotent_and_rejects_foreign_owner
```

同时执行 orchestrator 的 profile recovery、session append、Skill/compaction、fork epoch 与 prompt snapshot 相关回归。保留每条命令、源码身份、退出码、实际通过/失败数量，写入持久 acceptance 目录。

## 10. 固化 SDK、更新 App pin

1. 审核 Harness 当前 dirty manifest，只提交本任务源码/测试/协议快照；不得包含 target 或凭据。
2. 获取新的完整 40 位 SHA。它应包含协议 32 与本轮最终修复。
3. App `Cargo.toml` 中本 SDK 的 47 处 rev 要统一，lockfile 同步到同一 source identity；实际数量以重新检索为准。
4. 必要时将本地提交导入 Cargo 使用的 Git object cache；这是本地 fetch，不是向远程发布。
5. 运行锁定的 resolver，确认 App 真正使用该提交。

```bash
cd "$APP_ROOT"
CARGO_NET_OFFLINE=true python3.11 scripts/checks/check-runtime-dependency.py
cargo metadata --locked --offline --format-version 1 > "$EVIDENCE_ROOT/cargo-metadata.json"
```

前序解析范围为 79 个 Harness packages 和 5 个 mobile-linux packages；若最终数量有合理变化，应记录原因。禁止混用两个 Harness rev，也不引入 `[patch]` 本地 override。

本地 Cargo Git DB 的已知位置为：

```text
/Users/luolingfeng/.cargo/git/db/harness-runtime-8d2473fb8c2fadbd
```

使用前重新验证该 DB 对应 URL 和权限。需要 fetch/push 外部远程时使用仓库 owner 对应 SSH alias `github.com-lingxi-coder`，不能假设默认 GitHub 身份。

本次文档不包含新的 push、合并 main 或发布授权；不要因写好交接就执行发布。

## 11. 协议与 bridge-client

在完成 pin 后，从 App resolver 选定的不可变源码生成：

```bash
cd "$APP_ROOT"
CARGO_NET_OFFLINE=true python3.11 scripts/checks/generate-client-protocol.py
CARGO_NET_OFFLINE=true python3.11 scripts/checks/generate-client-protocol.py --check
CARGO_NET_OFFLINE=true python3.11 scripts/checks/generate-client-protocol.py --test
CARGO_NET_OFFLINE=true python3.11 scripts/checks/test_bridge_protocol_generator.py
CARGO_NET_OFFLINE=true python3.11 scripts/checks/check_client_protocol.py

cd "$APP_ROOT/packages/bridge-client"
npm run build
npm test
```

检查当前源码、validator、序列化快照与消费端，不应留下旧模式行为：

```text
set_session_mode / SetSessionMode
session_mode_changed / SessionModeChanged / sessionModeChanged
SessionModeTransition
pendingMode / pending_mode（模式选择语义）
协议 31 的当前版本声明或 fixture
```

负向测试中的旧字符串可以用于断言拒绝；历史归档或其他独立语义中的 pending 不能机械删除。Generated 中的旧 event 必须通过重新生成消失。

## 12. App Rust production 与 test 编译

统一 pin 后检查四个根 crate 的非测试和测试目标。此前 `--tests` 没有提供所需的非测试 FFI metadata；不要只跑 tests 然后假定 production 已编译。

```bash
cd "$APP_ROOT"
cargo check --locked --offline \
  -p ios-framework -p android-aar -p bridge-server -p cli \
  --lib --tests

cargo test --locked --offline -p remote-control-client
```

Mobile FFI 源码由两端 packager 编入，不存在独立 `packages/mobile-ffi/Cargo.toml`。其 remote tunnel 测试应通过 `ios-framework` / `android-aar` 相应 test target 运行。

App 关键行为测试定位：

```text
CLI / background fork：
independent_background_fork_retires_copied_skill_authority
independent_fork_rebuilds_computed_prompt_but_handoff_retains_it
exhausted_fork_epoch_fails_before_filesystem_publication

Bridge / scheduled draft：
scheduled_durable_anchor_requires_a_new_owned_draft
scheduled_draft_is_discoverable_and_resumable_before_first_turn
scheduled_draft_cannot_adopt_an_existing_conversation

audio-provider：
pending_admission_blocks_capture_even_if_playback_unpauses
interruptible_capture_stays_paused_until_deferred_input_is_admitted
gemini_audio_resumes_after_interrupting_thinking_without_provider_start
```

还应运行 CLI public Chat 拒绝、exact admitted Chat background recovery、mode mismatch 无副作用、busy handoff 与 hot-resume owner 校验的已修改测试。只检查 Cargo 编译不能代替执行。

## 13. FFI 生成与原生验证

### 13.1 先生成同一版本

标准入口：

- Android：`apps/android/native/scripts/build-jni.sh`，以及当前项目配置的 UniFFI Kotlin 生成步骤。
- iOS：`apps/ios/native/scripts/build-xcframework.sh`。

iOS 现有脚本负责三份 Swift 绑定的 runtime/scaffolding 去重、callback 初始化和 modulemap。不能手动覆盖其中一份文件后跳过后处理。

前序使用过的 UniFFI 0.32.2 generator：

```text
/Users/luolingfeng/lingxi/acceptance/mobile-local-app-20261023/uniffi-bindgen
SHA256 335f0fdf986055bd5c57ab5b5b2bf80b93f8a95846fbf54201c2749ea33f1570
```

复用前验证文件存在与 SHA。generator 可复用不代表输入 metadata 可复用；当前旧协议 metadata/静态库全部不构成协议 32 的证明。

### 13.2 仅用于绑定检查的资源受限路径

已有脚本支持：

```bash
cd "$APP_ROOT"
LINGXI_FFI_BINDINGS_ONLY=1 \
LINGXI_FFI_METADATA_LIBRARY=/absolute/path/to/current-ios-metadata.a \
LINGXI_BINDGEN_PATH=/absolute/path/to/verified-uniffi-bindgen \
CARGO_NET_OFFLINE=true \
bash apps/ios/native/scripts/build-xcframework.sh
```

`current-ios-metadata.a` 必须来自最终 pin 的真实编译产物，包含 client、harness_runtime 和 ios_framework 的 UniFFI metadata；Android archive 则使用 android_aar root。不能把两端 root 无差别混在一个 archive。

前序尝试已证明可从编码 MIR 的 cargo-check 记录生成 metadata object，但临时 wrapper/helper 当前不可依赖。若重建这条优化路径，必须重新保存工具源码与 provenance，验证每个 `_UNIFFI_META` 符号及 const section 字节与原 object 一致。

资源经验：完整单个 runtime object 会导致严重 LLVM 内存/交换压力。分 CGU 的对象输出可使用较高 `codegen-units`、`--emit=obj --out-dir` 且不带单一 `-o`，再提取确实包含 metadata 的 const sections。此路径只证明生成绑定，**不证明平台静态链接、包内 native ELF 或设备 ABI**。

### 13.3 Android

用户最新指定每个平台只验证一个 flavor：Android Direct、iOS Full、Desktop default。以下最终验收只统计该范围；历史其他 flavor 检查不表示已按最终源码验证。

生成后至少完成 DirectDebug Kotlin 编译和当前 JVM 测试。必要时先用 Gradle `--dry-run` 核对 native 依赖任务。

```bash
cd "$APP_ROOT/apps/android/native"
./gradlew --offline --max-workers=1 \
  :app:compileDirectDebugKotlin
./gradlew --offline --max-workers=1 :app:testDirectDebugUnitTest
```

只做 Kotlin/JVM 检查时，如排除 `stageDirectMobileLinuxAssets` 或对应 native verification 任务，必须把排除项与原因写入证据，不能把结果叫作 APK build。先核对当前 task graph，再选择具体 `-x`，不要盲目复制历史命令。

已知依赖为 Gradle 9.6.1、匹配当前 manifest 的 mobile-linux-native-support 本地 Maven 工件与精确 pin 的 sherpa。缓存缺失时明确报告，不替换为邻近版本。

重点测试：SessionOwnerRegistry、OrderedSelectionWriter、recovery routing、draft/attachment restore、RemoteConversationSource、TeamApproval UI、ComputerUseOwnerRegistry、AudioOwnerBindingRegistry、FlowVoiceController、VoiceSpeechPlayer 与 realtime lane。

### 13.4 iOS

新绑定之后至少覆盖 FullDebug 的 product source、生成 Swift 和受影响 XCTest target。前序检查范围为约 215 个 product source 加 3 份 generated Swift，最终数量以当前工程为准。

- 完整 Swift typecheck 需要实际 Swift Observation 宏及真实第三方 module，parser-only 不算。
- 使用缓存时核对 Firebase 12.19.2、GoogleSignIn 9.2.0、ZIPFoundation 0.9.20 等当前 `project.yml` pin。
- 曾使用 `teams-sdk-cache-20261030` Products；必须补充正确的 `GoogleUtilities-NSData` modulemap，且排除旧 FFI include/modulemap。
- 该缓存是否仍存在需要重新检查，不能把另一个 worktree 的旧 FFI 当作依赖。
- 独立的宿主 XCTest/stub 测试可验证纯业务逻辑；不得代替 iOS Simulator/真机链接和交互测试。

重点测试：NewSessionModeReadiness、SessionDraftOwnership、SessionResume、ProjectScopedPreferences、ConversationSourceReplacement、BackgroundSupport、RemoteModeAdmission/RemoteControl、VoiceInteraction 与 IOSRealtimeAudioLifecycle。

## 14. Desktop 验证

bridge-client build 完成后，再检查 Electron：

```bash
cd "$APP_ROOT/apps/electron"
npm run typecheck:node
npm run typecheck:web
```

使用项目现有 Node/tsx runner 运行相关测试，至少覆盖：

```text
conversation-spaces.test.ts
session-mode-runtime.test.ts
session-workspace-runtime.test.ts
session-workspace-host.test.ts
session-catalog.test.ts / session-catalog-workspace.test.ts
session-archive.test.ts / submitted-session-catalog.test.ts
runtime-command-readiness.test.ts / resume-ownership.test.ts
scheduled-service.test.ts / scheduled-replay.test.ts / scheduled-reconnect.test.ts
remote-snapshot.test.ts / remote-runtime.test.ts
runtime-lifecycle-interaction.test.mjs
```

真实 Electron lifecycle fixture 涉及 loopback listener，前序 sandbox EPERM 后在允许的环境下通过。若当前受限，准确记录权限或运行条件，不将其归因于业务逻辑。

签名打包只用仓库要求的 `npm run package:mac:flare`。未执行签名和包内静态/运行验证之前，不声明 Desktop package 完成。开发进程也不能证明 Credential Broker 持久性。

## 15. 验证记录：历史与本次分开

### 本次文档核对

已读取实际工作区并确认：

- 两仓库 HEAD、branch、dirty manifest、暂存区状态。
- SDK 工作区 32 / App pinned SDK 31 / TS 32 的版本断点。
- Kotlin 与 Swift 生成 client 中的旧 mode event。
- handoff 与两个 remote fixture 的当前缺字段问题。
- 两仓库 `git diff --check` 通过。
- 选取的旧 `/private/tmp` 日志均缺失；旧 artifact manifest 不能视为当前测试二进制证明。

完整结果见机器快照。本次未重新运行下列历史测试。

### 前序执行记录，仅供定位

| 范围 | 前序报告 | 使用限制 |
| --- | --- | --- |
| Session lib | 437/437。 | 晚于该次运行的 strict header/UUID 测试需重跑。 |
| Runtime mobile | 协议 32 no-run 编译通过；定向组中 handoff fixture 有失败。 | 不代表完整 runtime suite 通过；原日志缺失。 |
| realtime | 一轮 24/24。 | 早于最新 completion/frame-tail/no-playback 改动。 |
| bridge-client | 协议 32 build 通过；更早全套 350 pass / 5 skip。 | 全套数字来自此前状态，不能移作最终 32。 |
| Electron 32 | 分组 251、193、150，通过；另有 model 11、disposition 4、remote 7、lifecycle 1。 | 分组可能重叠，不合计独立测试数；3 个 Firebase emulator 用例跳过。 |
| Android | 旧绑定下 Direct/Play 编译，部分 recovery 组 88/88。 | 发生在最终 clean-break/音频/remote 修改前，需新绑定重跑。 |
| iOS | 旧绑定下 Full/Store typecheck；realtime mock 10/10；存储/偏好隔离测试 37/37；remote 纯 Swift 6/6。 | 含 mock/stub 与宿主检查，不是当前 FFI32 或真机验收。 |
| App Rust | 旧 SDK pin 下四根 crate production/test check 通过。 | 已被协议 32 源码变更取代。 |

这些数字来自前序执行记录，不是此次重新验证的结果。应为最终版本重新生成原始日志、退出码和 source/ABI 身份。

## 16. 三端验收矩阵

| 场景 | Desktop | Android | iOS | 通过条件 |
| --- | --- | --- | --- | --- |
| 项目页点击普通 New Chat | 必测 | 必测 | 必测 | project=null / Global，mode 来自新建时捕获。 |
| 项目内明确新建 | 必测 | 必测 | 必测 | 仅此入口绑定项目。 |
| Chat/Code 往返 | 必测 | 必测 | 必测 | 各自恢复导航与草稿；空项目仍可见。 |
| A 工具运行时切到 B | 必测 | 必测 | 必测 | A 不取消、不重启，返回后结果连续。 |
| A 等审批时切到 B | 必测 | 必测 | 必测 | 审批只回复 A；B 不接收其状态。 |
| 提交后 Started 到来前切换 | 必测 | 必测 | 必测 | admitted/pending owner 被保留，输入不丢。 |
| 相同 UUID 重复挂载 | 必测 | 必测 | 必测 | 复用 owner 或明确拒绝第二次 claim。 |
| `/cd` 与重定位 | 必测 | 必测 | 必测 | 固定 mode/scope 不变，cold resume 路径正确。 |
| recent/search/pin/archive | 必测 | 必测 | 必测 | mode 过滤在 limit 前。 |
| 连接延迟且连续切换 | 必测 | 必测 | 必测 | 旧结果不抢前台，不错写草稿。 |
| 空会话重启恢复 | 必测 | 必测 | 必测 | 使用明确 expected mode 与原 owner。 |
| 调度新目标 ACK 后立刻退出 | 必测 | 按宿主支持 | 按宿主支持 | UUID 有 durable anchor，可发现且可恢复。 |
| 跨空间摘要期间源 turn 继续 | 必测 | 必测 | 必测 | source 不取消，target 仅草稿。 |
| Retry/Cancel 摘要 | 必测 | 必测 | 必测 | cutoff 固定，只影响 summary。 |
| 后台通知与 Stop | 必测 | 必测 | 必测 | 按源 UUID/turn 路由，不操作当前选择。 |
| 两个 owner recovery journal | 按宿主支持 | 必测 | 必测 | 旧终态不清新 turn，A 不清 B。 |
| 语音等待工具时切换 | 必测 | 必测 | 必测 | 音频/审批 lane 仍归原 owner。 |
| OpenAI / Gemini 中断后新输入 | 必测 | 必测 | 必测 | 旧完成帧不结束新输入，采集 gate 正确。 |
| unknown/conflicting remote mode | viewer 按支持 | 必测 | 必测 | 不猜 Code，冲突后停止投影与控制。 |
| Team 单次远程审批 | host 必测 | viewer 必测 | viewer 必测 | 无虚构 mode；仅 allow-once/deny。 |
| 旧数据/缺 header/重复键 | 必测 | 必测 | 必测 | 明确拒绝，原文件不被空状态覆盖。 |

CLI 单独验证公共 list/search/resume/fork 都不暴露 Chat，以及认证的 exact background Chat handoff 仍能继续原任务。

每个“必测”需要实际结果与所在层级。宿主假对象、JVM/XCTest、模拟器、真机和真实 provider 不能互相替代。

## 17. 资源与清理边界

本次核对可用空间约 2.2 GiB，属于瞬时值，开始构建前重新查看。历史上完整 rustc object 与并行编译导致大量 swap，空间曾不足 1 GiB。

当前任务目录可见：

```text
build/
validation-runtime-target/
validation-artifacts/artifact-manifest.json
```

`validation-runtime-target` 仍有构建缓存；其 freshness 要由当前源码和 Cargo 验证。旧 manifest 指向的二进制可能已经删掉；快照逐个记录是否存在，不可仅凭 manifest 文件存在宣称可复现。

仅清理由本任务生成、可重建、无人使用的缓存：先核对路径和 owner、进程与 open file，保留日志/hash/provenance，再按明确 target-dir 清理。保护 Git 对象、源码、lockfile 引用、业务数据、活动会话及其他任务缓存。

日志今后写在持久 acceptance 目录，而不是仅 `/private/tmp`。清理构建产物前先复制需要保存的真实验收证据。

## 18. 已知未覆盖与后续设计

- 移动端驻留 SessionHost 尚无完整容量上限和安全 idle eviction；频繁访问大量会话可能增加进程内存。不要为回收而破坏活动 owner。
- ASR 迟到文本的 JSONL 顺序仍有局限，本轮没有重排历史。不要宣称已验证所有语音历史的时间顺序。
- 最新协议 32 的两端完整 native 链接、签名包、物理设备与真实 provider 尚未验收。
- Firebase emulator 的历史 skip 需要在具备环境时执行，或保留明确未覆盖项。
- catalog 可共享不意味着指令/授权状态可全局共享；后续新增 cache 必须说明 owner、epoch、版本和授权失效边界。

## 19. 最终收口清单

- [ ] 修复 handoff 与 remote 遗留 fixture，相关测试真实运行通过。
- [ ] strict header / fork epoch / cold recovery / cron / realtime / scheduled anchor 最终回归完成。
- [ ] 新 Harness 本地提交固化；App pin 和 lockfile 完全一致。
- [ ] 协议生成、validator、snapshot/ordinal、Swift/Kotlin 绑定一致为 32。
- [ ] App production + test 编译及 CLI/Bridge/audio/shared remote 回归完成。
- [ ] Electron typecheck、相关 host/UI lifecycle 用例完成。
- [ ] Android Direct、iOS Full 和 Desktop default 用当前绑定完成验证。
- [ ] 三端核心交互验收明确记录，包/设备/provider 缺口不冒充通过。
- [ ] 再做一次针对最终 diff 的 code review；修复问题后只重跑受影响检查。
- [ ] 仅按本任务 manifest 做局部提交，排除 node_modules、Generated、target、密钥与无关改动。
- [ ] 更新本文当前状态、最终 SHA 和验收索引；任何发布/合并动作另外按用户授权处理。

## 20. 可直接用于续接的任务说明

```text
继续 /Users/luolingfeng/lingxi/worktrees/code-chat-context-20261028 下的 App 与 Harness dirty worktree。
先读 docs/architecture/conversation-spaces-context-design.md 和本交接，不要丢弃未提交实现。
核心契约：普通 New Chat=Global；session mode 创建固定；切换只改导航且不取消 turn；CLI 公共入口 Code-only。
仅支持当前协议/数据格式，删除旧兼容，保留无关代码与业务数据。
先修 SDK handoff retry fixture、remote e2e/bots 缺 mode fixture；完成最终 SDK 回归后提交并更新 App 完整 pin。
当前 App 仍 pin 协议31，而 SDK/TS源码已32，Swift/Kotlin绑定仍旧，必须重新生成和跑跨端检查。
使用 GPT‑6.1 Terra max 按平台并行，统一协调协议与FFI；重型构建按资源串行。
历史/tmp日志已缺失；所有新结果写入 acceptance/conversation-spaces-20261101，记录 source/ABI身份、命令和退出码。
完成最终 code review 并修复；只提交授权范围，不自动 push、merge、打包安装或删除其他缓存。
```
