# LingXi 遗留任务接手清单

2026-10-19，America/Los_Angeles。用户指定接手 LingXi 遗留工作；**chat 项目与 Computer Use 暂不接手**。Claude Code 运行时目标保持 2.1.295，不增加旧协议兼容分支。Remote 保持 LingXi 自建服务和通信所有权。

## 基线与核查

- SDK：`e986d0841e5cc6b5151bdaeabca1c6dd156771df`。
- Harness：`f928124df1035eb861c1d5f056d977cef8756369`。
- Product：`bf24677c3e470b3acb06157495b54cdd9e288ae0`。
- 在两个隔离的 `codex/remaining-tasks-20261019` 分支推进，不重置原工作区。
- 已读取相关 session 的最近报告并核对 main 祖先。旧报告中的“未提交”不能直接视为当前未合并；Activity、Side Chat、模型路由、FCM、Crashlytics、Fusion、slash 提示等已有实现进入 main，不重新重放历史补丁。
- 本轮前的 iOS 设置构建与 5 项定向测试已通过。它们不替代真机、签名 Universal Link 或系统广播验收。

## 当前仍开放的交付

| 工作 | 当前证据与缺口 | 下一步 |
| --- | --- | --- |
| WebRTC iOS 系统广播 | 提前帧下溢回归、Flare 签名及安装通过；CUA 点击路由不稳定，尚无当前广播回执。2026-10-20 用户明确要求先跳过此项测试 | 保留未验证状态、源码与证据，不阻塞其他工作；不以 Ready 或构建成功代替跨 App 帧、停止与恢复验收 |
| WebRTC 产品接入与 Remote | 独立 transport/media/FFI 与 localhost TURN 已验证；生产产品没有接入 | 在媒体边界验证后接入现有 host/control-plane；认证、设备与权限仍归项目自有服务，不另造执行所有者 |
| WebRTC 其他媒体边界 | 限制性 NAT、TCP/TLS、手机互通、长期刷新、热状态与能耗尚无完整证据 | 逐项实施和验证，区分共享测试与真实设备结果 |
| 版本化模型目录四端刷新 | Firebase API、已发布目录及 Web 管理页已存在；当前文档明确尚未接入四端启动同步 | 闭合 SDK 官方目录定义、Harness 校验/缓存/刷新、Desktop/CLI/mobile 启动和当前选择器消费者；保留用户模型、连接和凭据 |
| iOS 真机界面与 Side Chat | 源码与模拟器回归已存在，旧 session 的真机检查因镜像/锁屏未完成 | 用匹配绑定和库的当前签名 App 验证键盘、独立草稿、隐藏重开与模型设置 |
| 深链接发行验收 | 本地实现、网页、AASA/direct 关联已发布；Play 真实 App Signing 指纹、签名 iOS Universal Link、Windows/Linux 实机仍开放 | 复用当前实现做平台验收；不能猜证书或把模拟器/脚本结果标为发行通过 |
| 移动 Crashlytics | Android JVM 测试云端入库已有记录；iOS 真机上报及 NDK 云端符号化未完成 | 准备匹配产物和隔离测试；云端上传遵守既有授权范围，不对正在使用的会话故意触发崩溃 |
| Desktop production 凭据稳定性 | 旧任务等待 production 通道保存凭据，未完成约定的 15 次刷新、3 次启动 | 先确认当前签名通道及凭据状态；不跨通道复制密钥，不记录明文凭据 |
| 其他验收边界 | macOS 公证/跨机器 Gatekeeper、长期稳定性等没有因 main 合并自动完成 | 只在对应环境与实际操作完成后关闭 |

Local App 原产品代码不是 Claude 对齐项。独立市场的旧“后端 503 未配置”报告已被后续 Firebase 部署替代；本轮实际读取当前市场 `/api/v1/health` 与 `/api/v1/capabilities`，均为 JSON / HTTP 200，可关闭那条旧阻塞。模型公共 API 实际返回 46 个提供商、1327 个模型，未登录管理接口返回 401；这证明后台可用，不代表四端启动刷新已经实现。生产服务变更和证书发布须保留具体可审内容与相应授权。

## 调度与证据

首个修复阶段的 Harness 已推送 main：`b054a3bfe094d8a588d76706ca90212d62381111`，远端一致、分歧 `0 0`。Product 的全部 Harness manifest/lock 来源同步固定到该提交；SDK 仍为 `e986d0841e5cc6b5151bdaeabca1c6dd156771df`。新旧 Harness 的生产 `crates` tree 均为 `5407db7a705a9a1305e4f0a6bac8710ab6e0b76c`，本批改动仅在独立 WebRTC 源码与文档；因此不将旧包重标为新构建，也不重复未受影响的 tokenizer/生产 FFI 全量构建。

源码与最小可复现优先，采集仅用于剩余语义疑点和关键路径。一次只运行必要的重量构建，避免磁盘再次耗尽。每批验证通过后合并推送 main；只清理已交付且无占用的本任务分支、工作树与可重建缓存，保留仍作为 Mods 注册源的目录。

接手证据在工作区 `.buildcache/remaining-task-takeover-20261019/`：`task-inventory.json` 包含原 session 标题、报告及已核实的 main 祖先；每项新验证保留独立日志。该清单是继续推进的入口，不是“所有遗留任务已完成”的声明。

## 2026-10-20：跳过广播测试后的推进

- Product 已快进整合 main 到 `ff2797e03`，包含 Android Gradle 升级和 FFI 调用方修复；Harness main 仍为 `b054a3bf`。未重置原 checkout，保留本任务尚未提交的 WebRTC 修复。
- 当前 Android 验收从这组固定来源重新准备 Release 原生支持和 rootfs，校验已完成部分的哈希与来源；JNI/绑定从当前提交重新生成。旧 `98130b21` JNI 的 Harness 源码与当前 `b054a3bf` 不同，不能复制旧库冒充当前组合通过。
- JNI 构建脚本的 Play / Direct 交叉编译补齐 `--locked`。新增依赖漂移回归先失败后通过：锁文件需要更新时立即失败，不改锁文件，不发布 JNI、绑定或符号。16 项现有/新增发布边界测试通过，随后补跑两个变体的依赖漂移用例均通过；shell 语法与 diff 检查通过。这些脚本验证不代表 APK 或真机验收通过。
- 当前 main 的来源检查确认 canonical Git Harness `b054a3bf`、无开发路径覆盖；719 项客户端来源和 3 份协议镜像检查通过。Android JNI / Kotlin / APK 完整门禁仍在进行，不能借用旧产物的结论。
- 版本化目录尚未接入。源码入口已确定：后台 `apps/firebase/src/catalog.mjs` / `scripts/generate-seed.py`；SDK `configuration/merge.rs::Definitions::get`；Harness `llm-runtime/src/catalog/upstream.rs`、`provider-config/src/assemble.rs`、desktop `credentials.rs` 和 mobile `host/assembly.rs`。后台四个布尔能力与价格投影不包含完整 SDK wire 定义，不能直接替换协议、凭据、推理控制或定价时段。后续必须闭合官方定义快照与启动缓存，再由同次配置装配刷新执行目录与 UI；保留用户覆盖，不能只接选择器。
- WebRTC 产品与 Remote 保留开放状态，但不再以本轮广播测试为其他独立任务的前置条件。系统确认、设备验收和生产凭据等环境边界继续分别记录。

## 2026-10-20：Android Play JNI / APK 阶段通过

- 固定 Harness `b054a3bfe094d8a588d76706ca90212d62381111`、SDK `e986d0841e5cc6b5151bdaeabca1c6dd156771df`，重新完成 ARM64、x86_64 release JNI 和来自同一库的三套 Kotlin 绑定；没有借用旧版本 JNI。
- 将原来的 UniFFI 生成器等价移动到独立 `native-bindgen` 工具，删除 iOS crate 的旧 CLI feature/入口。Android 生成绑定不再额外编译 iOS 运行时。两端脚本使用同一固定 UniFFI 0.32.2；保留 Kotlin 错误属性、音频 buffer 修复和 JNI/绑定/符号的事务发布。2 项生成器测试、1 项实际依赖闭包/版本边界测试、16 项 JNI 发布回归通过。Swift 生成 smoke 通过，不代表 iOS App 已重新编译。
- 修复当前 Android 调用方：提示建议直接保留 `ClientTextDto`；slash 命令完整传递已有图片列表、原始 UTF-16 及 turn ID。同步当前必填工具展示 DTO 的测试样例，删除已不存在的旧字段；Compose 测试改用具名 `state` 参数。
- `assemblePlayDebug`、全部 **977 项 JVM 测试**（0 失败/跳过）及 `assemblePlayDebugAndroidTest` 通过。最终 APK 原生支持/rootfs/ABI 校验通过；APK v2 签名验证、独立模拟器安装通过。
- Android 37 ARM64、**16 KiB 页**独立模拟器执行真实 JNI 初始化、原生 MCP/Skill/Plugin/Hook 命令往返、Side Chat 原生/Compose 测试，**3 项通过**。Launcher 启动成功，停在系统首次权限界面；测试后 crash buffer 为空。未使用实体手机或真实 provider 凭据。
- canonical Git 来源检查仍无路径覆盖；719 项客户端来源、3 份协议镜像检查通过。证据与 APK 哈希索引：`.buildcache/remaining-task-takeover-20261019/android-play-acceptance-20261020.json`；APK 位于本任务 Product 工作树的 `apps/android/native/app/build/outputs/apk/play/debug/app-play-debug.apk`。
- 本阶段验证 **Play debug**；Direct 的 CU 原生变体、实体设备、Play App Signing/发行和在线模型推理未因此通过。WebRTC 按用户要求跳过。原 Product checkout 的 `gradle/libs.versions.toml` 未提交修改保持原样，不纳入本批。

下一阶段继续版本化模型目录四端启动刷新；当前 Firebase 目录还不能替代完整 SDK 执行定义，不能将选择器刷新单独记为完成。

## 2026-10-20：模型目录执行定义的准备阶段

- Android 阶段提交 `2557ac7b1374dd11faf39d19169515a29828e60b` 已快进推送 Product main，远端一致、分歧 `0 0`。
- 目录生成器为每个提供商保存可选 `executionDefinition`：固定格式 `llm-client-preset-toml`、原始 UTF-8 TOML 和 SHA-256。每个源文件只读取一次，展示投影、原始执行定义与哈希来自同一字节快照。按固定 SDK `e986d084` 重生成 seed，仍为 46 个提供商、1327 个模型；来源与完整 seed 哈希进入 provenance。
- Firebase 严格 DTO、草稿校验、平台筛选、公开 JSON 及 Web 导入导出类型保留该定义；哈希不符会拒绝。没有把四个展示能力布尔值当成完整推理/鉴权/协议/媒体/分时定价配置。此处的校验是格式和内容哈希校验，**不是 SDK 对 TOML 执行语义的校验**。
- 13 项 Firebase 本地测试通过，2 项 Firestore 模拟器用例未运行；39 项 Web 测试、Web 类型检查和生产构建通过。默认系统 Python 3.9 导致的文档测试失败在使用仓库要求的 Python 3.13 后全部通过，无需修改文档测试。seed `--check` 与原始 SDK 文件一致，Unicode/CRLF 往返及篡改拒绝回归通过。
- **四端启动刷新仍未完成**：下一批须在 SDK 入口解析完整定义，按当前 App 版本/平台做有界 GET、ETag/304 和原子缓存；同一启动快照进入 Harness 的 Anthropic/preset 装配、执行路由、价格及客户端列表，并由 settings 刷新继续持有。必须验证自定义模型/连接/凭据不被覆盖，超时/损坏快照回退到有效缓存或内置定义，不以 UI 刷新替代执行链验收。未进行云端部署或覆写现有发布/草稿。
