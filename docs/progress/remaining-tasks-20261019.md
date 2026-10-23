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
| 版本化模型目录四端刷新 | 四端实现与启动缓存验收完成；本页最新记录补齐 Android 实体手机和 Flare 签名 iPhone 的首次获取、重启及配置保留验证 | 保留发行渠道、逐模型推理及其他平台验收边界，不把目录刷新通过扩大为全部运行时通过 |
| iOS 旧会话恢复与 Side Chat | 2026-10-22 已重新构建最新 main `406879ed2`、Harness `5a2de5b1` 的签名包并通过旧会话链接／普通重启、硬件键盘、独立草稿、隐藏重开、模型隔离及结束生命周期 | 本批当前 main 签名真机验收完成；后续源码、屏幕键盘触控与发行渠道不继承本结果 |
| 深链接发行验收 | 本地实现、网页、AASA/direct 关联已发布；Play 真实 App Signing 指纹、签名 iOS Universal Link、Windows/Linux 实机仍开放 | 复用当前实现做平台验收；不能猜证书或把模拟器/脚本结果标为发行通过 |
| 移动 Crashlytics | 2026-10-22 iOS 真机未捕获 NSException 云端入库并解析到 Swift 源码行；Android NDK 测试库云端入库并解析到 C 源码行 | 本批接入链路通过；首次 Swift SIGTRAP 对照未观察到云端事件，单独保留未通过记录，不扩大为所有原生信号／Rust／第三方库覆盖 |
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


## 2026-10-20：四端模型目录启动与运行时接线

- SDK 固定 `ff71b2ec1139bc67cc8719d78b1735aca967bbe8`，Harness 固定 `1edfd6e2365618052a2bb7096ae1b1dc0430a775`。Product manifest、锁文件和 Cargo metadata 保留 canonical Git URL、完整 SHA，无路径覆盖；由同一组装目录提供实际执行路由、价格、设置目录和选择器列表。Claude Code 目标仍为 2.1.295。
- Desktop 传入 Electron `app.getVersion()`；CLI 使用实际 CLI 包版本；Android 使用 `BuildConfig.VERSION_NAME`；iOS 使用 `CFBundleShortVersionString`。各端分别按平台、精确 App 版本、schema 和 endpoint 隔离缓存。确定性嵌入与测试不配置网络来源；CLI `--bare` 不开启目录请求。
- **先本地、后台刷新**：有效缓存优先，无缓存使用 SDK 内置定义；两种启动均不等待网络。后台 GET 最长 8 秒，验证整个响应、版本范围、源定义 SHA、完整缓存校验和、SDK 解析及执行注册表后，使用非阻塞锁、私有临时文件和 rename 更新缓存。当前运行时及 settings 重载继续持有原快照；下次运行时装配采用新缓存，不中途切换请求。
- 新提供商／模型必须具有完整可执行 SDK 定义；已知定义可显式引用固定 SDK。保留鉴权、协议、endpoint controls、音频服务、分时价格等信息。用户自定义模型、连接、路由别名、端点覆盖、环境密钥名和稳定凭据 ID 在官方快照之后合并；目录代码不写 settings 或凭据。ChatGPT 登录连接保持 Host 所有权。
- 修复新发布模型和用户模型被二进制内置白名单再次隐藏的问题。CLI 的独立行结构保留明确发布标记；连接、弃用及管理员限制继续生效。Desktop 的设置目录组装也改为复用同一个启动快照，避免刷新时回退到内置目录。会话音频复用真实运行时服务；独立音频 Host 继续使用其明确的独立配置入口。
- SDK default 库测试 **622 项通过**；最终 Harness provider-config **87 单元 + 1 集成测试通过**，包括真实 localhost 条件 GET/304、损坏／503 回退、发布回退保护、自定义配置、协议格式退出后的凭据 ID、后台快照隔离，以及服务器不响应时首次启动仍及时使用本地目录。provider-config / llm-runtime 全 targets Clippy（拒绝 warnings）通过。
- 最终只读公共查询四端均接纳 **1274 个模型、47 个提供商**（含 Host 登录连接）；测试版本 Desktop 0.1.0、CLI 0.12.0、Android 0.1.0、iOS 2.4.1。请求无账号、workspace 或 provider 凭据；未改动云端草稿或发布。HTTP 耗时属于显式预热验证，不是本地启动等待时间。
- Product 定向验证：Electron 41 项 Host 测试、Node/Web 类型检查和生产构建通过；CLI 模型列表 11 项、CLI safe/bare 启动配置 1 项、Desktop 参数 10 项、iOS launch config 1 项通过；719 项客户端源码和 3 份协议镜像检查通过。原生生成器依赖隔离测试通过。
- Android / iOS host 原生库已构建并生成匹配的新 Kotlin / Swift 绑定；Android Play Debug 生产及 instrumentation Kotlin 编译通过，Swift 三个绑定模块与带 `appVersion` 的启动配置类型检查通过。这组绑定验证使用 Harness `94675ef8`；后续 `1edfd6e2` 仅调整 provider-config 的网络等待策略、测试、诊断示例和文档，**无 UniFFI 接口或客户端调用变化**。最终固定提交的 Desktop bridge、CLI、Android FFI、iOS FFI 四个生产根 locked 编译检查均已通过；不将旧 JNI 或 XCFramework 改标为新版本。
- 本阶段未重打或安装新版 APK、签名 iOS App、macOS 包，未执行设备／GUI 或真实 provider 推理验收；这些边界与共享契约和调用方编译分开记录。WebRTC 测试继续跳过；其 WIP 不纳入目录提交。chat 与 Computer Use 不接手。
- 所有日志及缓存清理清单位于 `.buildcache/remaining-task-takeover-20261019/catalog-*`。为避免磁盘耗尽，仅退休本任务已完成且无占用的旧构建缓存、测试二进制和临时验收模拟器可重建磁盘；保留源码、用户数据、APK、实际 JNI、符号和验收证据。原 Product checkout 的其他 session Gradle 修改未纳入。

## 2026-10-21：固定提交的 App 构建与目录启动验收

受测代码：Product `617dbc8501087f2c7218dc266b265bc2615cb7fc`、Harness `1edfd6e2365618052a2bb7096ae1b1dc0430a775`、SDK `ff71b2ec1139bc67cc8719d78b1735aca967bbe8`。三仓重新 fetch 后均与各自远端 main 一致；本轮没有修改产品运行时源码。下述路径均以 `.buildcache/remaining-task-takeover-20261019/` 为基准。

| 平台 | 实际构建与运行结果 | 目录与配置证据 |
| --- | --- | --- |
| macOS ARM64 | Flare 签名包静态及运行 smoke 通过；实际包完成 Keychain 凭据重启回读、localhost 请求、renderer 启动。已启动正常用户桌面 App | 隔离冷启动使用 77 个运行时模型，后台缓存 1327 个公开模型；重启后运行时模型 440 个，其中 432 项与公开目录上下文信息一致。自定义 profile/model 保留 |
| Android ARM64 | 当前 JNI 与匹配 Kotlin 绑定重新构建；Play Debug APK 与 instrumentation APK 构建通过；API 37、16 KiB 页模拟器运行正常；真实 JNI 设置持久化及引擎重建测试 1 项通过 | App 实际写入 1327 模型缓存，重启仍有效，平台 android / App 0.1.0；自定义模型及两条连接保持完整。首次未落盘和测试权限对话框干扰保留原始日志，没有预置目录缓存或删除产品校验 |
| iOS ARM64 simulator | 当前 Rust core、匹配 Swift 绑定、合并 XCFramework 与 StoreDebug App 构建通过；iOS 26.3 独立模拟器实际启动、重启及正常界面截图通过 | 首次无缓存启动后取得 1327 模型，重启使用现有缓存；平台 ios / App 2.4.1；自定义模型与两条连接保持完整，截图显示自定义模型 |
| CLI | 当前实际入口 dev 构建通过；以 Flare 身份签名，复用已验证包中的签名 Broker；两次启动均通过真实 streaming 请求返回 `CATALOG_RUNTIME_OK` | 隔离 HOME 的冷启动写入 1327 模型缓存，热启动已有有效缓存；平台 cli / App 0.12.0；实际请求均保持 `catalog-local` 的自定义 `catalog-model` 路由 |

- 四端公共目录的受测发布为 `e65c83d9-2d38-4244-ad45-535d730f5f43`，响应 SHA-256 `ae2b83c151099484784637577930a51ce9c159ba02ef49dbe19fb967304fa8f1`；逐端回执保留实际 App 版本及缓存校验和。与上一阶段 1274 模型的查询相比，公开发布已更新，未修改云端目录。
- 产物索引：`catalog-app-acceptance-617dbc850/app-acceptance-index.json`。macOS ZIP：`catalog-app-acceptance-617dbc850/macos/LingXi-Code-0.1.0-mac-arm64.zip`，SHA-256 `6823a8d121e5a4506c3deb40df7c69204b310ca446a29e6ed3b56992d3d45f41`。Android APK 位于本工作树 `apps/android/native/app/build/outputs/apk/play/debug/app-play-debug.apk`，SHA-256 `627c9121da225e3cf961f693c24792d80a6cffe6c84b01a93578a580e0906f12`。iOS App：`catalog-ios-xcode/Build/Products/StoreDebug-iphonesimulator/LingxiCode.app`，完整内容哈希清单为 `catalog-app-acceptance-617dbc850/ios-app-files.json`。
- 原始回执及截图分别在 `catalog-app-acceptance-617dbc850/desktop-cold-warm-r2/`、`android-cold-warm/`、`ios-cold-warm/`。macOS 官方包验证日志 `catalog-macos-package-verification.log`；Android 构建与 JNI 运行日志 `catalog-android-apk-build.log` / `catalog-android-native-roundtrip.log`；iOS 最终编译日志 `catalog-ios-app-build-arm64.log`。
- CLI 回执及原始请求记录在 `catalog-app-acceptance-617dbc850/cli-cold-warm-r3/`，保留二进制在 `catalog-app-acceptance-617dbc850/cli-runtime/lingxi-cli`。这属于实际 dev 入口运行验收，不是 CLI release 包验收。首次未携带 Broker 的开发入口按安全策略拒绝启动；补齐签名及真实 Broker 后才执行测试，没有改用伪凭据存储。另一次后台目录响应体读取失败时，推理仍成功且未生成错误缓存；错误日志在 `cli-cold-warm-r2/cold.stderr` 原样保留。独立 GET 与后续完整冷、热启动通过，不把该失败改写为首次成功。夹具主动延迟 localhost 响应 12 秒，回执的 22.42 / 19.60 秒是端到端测试耗时，不是目录阻塞启动时间。
- macOS bridge dSYM 与包内二进制 UUID 匹配，完整符号已无损归档；dsymutil 对部分临时 LTO 对象发出警告，第三方源码行信息完整性不作保证。Android 未裁剪符号独立无损归档并验证原始 SHA，保留运行 payload 与绑定。仅清理已完成、本任务独占的可重建中间缓存和隔离 Android 测试磁盘，逐项清单保留；其他 session 的源文件、构建和模拟器不动。
- iOS native-support 使用已验证、与固定 `mobile-linux-runtime` `6b033d41` 匹配的原生支持产物；Rust core 未复用旧版本。Swift 6.4 下支持库构建脚本的独立语言模式修复已在该 SDK main 的 `9eb37cf4` 发布并通过两种 simulator 架构构建；本次未把 Product/Harness 升级到包含其他 SDK 改动的新版本。
- 最终来源检查通过：77 个 Harness 包固定到 `1edfd6e2`，5 个 mobile-linux 包固定到 `6b033d41`，未使用路径覆盖。四端均取得同一发布的有效缓存；完整执行目录增量在 Desktop 实际 `list_models` 路径验证，移动端补充了真实 App 启动、缓存及配置保留证据。没有把移动端截图当作逐模型推理验证。
- 边界：Android 是 ARM64 Play Debug 模拟器验收，未覆盖 x86_64、物理设备或 Play 发布包；iOS 是 ARM64 simulator App，`CODE_SIGNING_ALLOWED=NO`，不代表签名真机、TestFlight 或 App Store 验收。隔离目录测试无真实 provider 凭据，未执行云端真实模型推理。WebRTC 继续跳过，Chat 与 Computer Use 不接手；canonical Product 的 Gradle WIP 及 Harness 的 WebRTC WIP 保持未提交。

## 2026-10-21：移动端竖屏、首个凭据路由修复与签名真机目录验收

受测 Product 来源为 `3ec6ae04b1ec4cf095796ba1889a34eee0c7011b` 加本批五个移动端文件；原始源码补丁 `catalog-device-acceptance-3ec6ae04b/mobile-source.patch` 的 SHA-256 为 `a1d0cb9efb04fece0e93253684867bb43d0d3f7cb87e5c605622ebffbd4ef022`。Harness `1edfd6e2365618052a2bb7096ae1b1dc0430a775`、SDK `ff71b2ec1139bc67cc8719d78b1735aca967bbe8` 与 mobile-linux-runtime `6b033d41a2b6b736fda1e8f996f55a4b20bd885d` 的固定来源未变。以下证据相对 `.buildcache/remaining-task-takeover-20261019/`。

- **竖屏**：Android 主 Activity 固定 `portrait`，覆盖共享 Manifest 的 Play / Direct；最终 APK 的 Manifest 已核实。iOS 共享 Info.plist 的 iPhone、iPad 方向数组均仅保留 Portrait，启用 `UIRequiresFullScreen`，Store / Full 共用这份配置。实体 Android 请求系统横向旋转后 App 截图仍为 1080×2400，测试结束恢复原系统旋转设置；iPhone 实际界面保持竖屏。iPad、Android Direct 与平板窗口模式未做设备实测。
- **Android 配置 DeepSeek 后仍无模型**：源码确认首次凭据保存会改变 profile 的可路由性，但此前未将 `credentialConfigured` 变化纳入引擎重建判断；异步 Save and apply 回调还读取了保存前的 Compose 状态。现在按保存后的 Store 状态重建引擎，并在凭据可用性变化时标记配置待应用。新增回归先红（14 项中该项失败）后绿，最终设置相关 **133 项 JVM 测试全部通过**，`assemblePlayDebug` 通过。原生 JNI / 绑定沿用上一节实际固定提交的匹配产物，没有用旧 JNI 代替当前版本。

| 真机 | 实际产物与启动 | 目录、配置及界面结果 |
| --- | --- | --- |
| Xiaomi 24117RK2CC / Android 16 / API 36 / ARM64 / 4 KiB 页 | 当前 Play Debug 更新安装，App 0.1.0；完成首次网络获取及随后重启 | 两次均校验 1327 个模型；DeepSeek 已在聊天选择器可用，未再出现 No model 提示。用户 provider、模型、URL、连接保持完整，既有加密凭据文件逐字节哈希不变 |
| iPhone 11 / iOS 18.6.2 / ARM64 | 从当前固定 Rust core 构建 device-only XCFramework，生成的 Swift metadata 与当前绑定逐字节一致；StoreDebug 2.4.1 / 8721 通过 Flare `AZ4AX7J833` 签名和设备授权检查，保留数据更新安装 | 切换至可访问目录的网络后，首次获取及重启均取得 1327 模型；实际进程正常存活。整个 provider-settings JSON 与更新前一致，未改写或导出 Keychain 凭据；界面显示 Flash |

- 两端实际缓存同一公开发布 `e65c83d9-2d38-4244-ad45-535d730f5f43`，body SHA-256 `ae2b83c151099484784637577930a51ce9c159ba02ef49dbe19fb967304fa8f1`。Android / iOS 回执分别位于 `catalog-device-acceptance-3ec6ae04b/android/receipt.json` 与 `ios-physical-receipt.json`；旋转回执为 `android/portrait-rotation-receipt.json`。首次网络超时和 Safari 无法加载的证据保留；用户恢复网络后才通过，没有预置目录数据或绕过校验。回执耗时属于端到端操作，不是启动等待网络的时间。
- 最终 APK：`catalog-device-acceptance-3ec6ae04b/android/app-play-debug-portrait-credential.apk`，SHA-256 `91b1e4f75170196e1023128429814a6666e7fa811d7132526fa3589d5501534e`。签名 iPhone App：`catalog-ios-xcode/Build/Products/StoreDebug-iphoneos/LingxiCode.app`；匹配 dSYM、完整 App 文件哈希、来源及签名回执保留在本批产物索引中。没有上传新符号或报告。
- **单独记录的旧会话边界**：最终 iPhone 截图显示既有会话的 `reporting admission rejected`，并非目录获取失败。只读核查确认该会话的 handback sidecar 是 v1、pending/admitted 均为空；当前源码只接受 v2。手机另有当前启动创建的 v2 sidecar。按用户不兼容旧版本的要求，本批不加入格式迁移、不删除旧会话或其数据，也不把该会话的推理标为通过。后续读取完整错误事件时 CoreDevice 隧道断开，原始失败保留；不影响此前已取得的冷／热目录回执。
- 本节关闭的是物理设备上的目录首次获取／重启读取、用户模型及配置保留和受测手机竖屏。它不代表所有真实 provider 推理、旧会话恢复、Play 发行、TestFlight / App Store 或物理 iPad 已通过。WebRTC 继续按用户要求跳过；Chat、Computer Use 与其他 session 的源文件不纳入。canonical Product 的 Android / Electron / iOS UI 及 Gradle 未提交修改、Harness WebRTC WIP 均保留。
- 推送前接入并发进入 main 的 `423baf3ba8eee488d3668b53e5fec0e59cd749b9`（可视化 UI 与 Android bootstrap session），与本批五个文件无重叠。合并后 Android 生产 Kotlin 编译与设置／bootstrap **138 项测试通过**；720 个客户端源码和 3 份协议镜像检查、iOS plist 校验及被接入 Swift 文件的语法解析通过。此处 Swift 检查仅为语法解析，未重做合并后完整 iOS 构建／GUI；上表设备安装包对应修复提交 `5cf957c9f02aa63b777d18e39cd8a7d3d182658c` 的源码组合，不标记为包含新可视化提交的重新打包产物。整合日志：`catalog-mobile-post-main-integration.log`。

## 2026-10-21：最新 main 包、会话存储修复与真机验收（进行中）

- 隔离工作树先接入 `74a6c9744`，再快进整合 main 的 `20ecfee5e`、`5234a15b0` 和 `811473f9a`。当前本地代码提交为 `a5b3dfac54b99e7c5fe5be826ba6d73a00823cf4`；本轮只新增 iOS 会话目录准备代码、引擎启动接线和回归。Harness `1edfd6e2365618052a2bb7096ae1b1dc0430a775`、SDK `ff71b2ec1139bc67cc8719d78b1735aca967bbe8` 及 mobile-linux 来源保持既有固定组合。没有将 canonical 的其他 session 修改纳入提交。
- 「Fix iOS error」session 已对 20 个空 v1 inbox 作一次性数据修复。本轮只读复核修复前后原件：pending/admitted 均为空，仅版本字段改变，另外 20 个原有 inbox 字节相同；初次真机备份的 42 个 inbox 全部为 v2。本轮没有再改写这些版本字段，也没有增加旧协议读取、迁移分支或兼容 facade。
- 真机复现了更新后旧会话链接显示“不在当前本地工作区”：数据容器 UUID 变化后，旧 transcript 留在原 CWD 的目录，列表只读取当前 CWD。新增准备流程在引擎构建前复制同一 app 的当前格式目录，保留原件、identity sidecar 和工具结果。字节前缀相容时选择完整副本；仅允许真实会话替代具有相同原始空会话锚点的占位副本。分叉冲突或 symlink 会明确失败，不覆盖原数据；被替换的当前副本另行备份。
- 首次真机复核还发现 Foundation 把路径化为 `/var/...`，而 Runtime 的目录键使用 POSIX `/private/var/...`，因此补改为与 Runtime 一致的 `realpath`。最终 **7 项 Darwin XCTest 全部通过**，覆盖原始 UTF-16 转义字节、前缀、空占位、幂等、冲突、工作区、长路径和真实 `/var` 身份差异。真实 iPhone 备份夹具中 **16 个原文件哈希不变、6 个 transcript 重定位、目标完整历史字节保留**；该夹具结果与最终真机运行验收分开记录。
- Android 最新 main 的 Play Debug 主 APK 和测试 APK 构建通过；相关设置、bootstrap、消息元数据与折叠策略 **144 项 JVM 回归通过**。主 APK SHA-256 为 `5aa17094c3b285f7b8eec1292c0c1ebe84afd445018e66cf7cd2297596f4bb2c`。标准非流式安装解决了 Xiaomi 的流式安装拒绝；两份 APK 均已保留数据更新成功。升级及重启两阶段的真实 App 启动、1327 模型缓存、DeepSeek 无障碍标签、1080×2400 竖屏均通过，provider/连接字段保留，加密凭据字节哈希从升级前至重启后相同。该 Android 源码与当前提交之间无 Android 或共享依赖变更。
- 额外 Android 组件 instrumentation 尚未通过：MIUI 日志明确拒绝测试 `ComponentActivity` 的后台启动，10 项测试停在第一项。已结束本次测试进程，没有关闭安全保护或更改后台权限；普通主 App 启动和界面观察与该环境阻挡分开报告。
- 最终 iOS `a5b3dfac5` StoreDebug **2.4.1 / 8721** 已完整构建、以 Flare `AZ4AX7J833` 签名并保留数据更新安装。签名、设备授权、iPhone/iPad 竖屏 mask 和 dSYM 匹配通过：主可执行 UUID 为 `AA9B9320-50FD-3C2E-9B79-EA609C2F99A7`，debug dylib UUID 为 `CD604E38-3CCB-324C-8896-33AB089740A3`；整个 App 文件清单 SHA-256 为 `ca1f060882f22562bd85e015e09e205ccc7dcdbae803b318d16ad1f841ee0b67`。724 个客户端源码及 3 份协议镜像检查通过。
- **当前未关闭：最终 iOS 旧会话打开、普通重启恢复及 GUI 验收。** 早期候选复核暴露了上述规范化缺口；修复后新包尚未取得运行通过结果。当前 CoreDevice 的正常及不激活屏幕启动均被系统以 `Locked` 拒绝，镜像显示 `Connection Interrupted`，最新显示清单只剩 LCD，无可用无线显示。已请求用户将 iPhone 放在 Mac 附近接电、密码解锁一次后再锁屏。不能将签名安装、离线夹具或前一候选的缓存记为最终版本运行通过；本次按用户随后明确要求先提交、合并并推送 main，保留该项未完成状态。
- 当前证据集中于 `.buildcache/remaining-task-takeover-20261019/mobile-main-acceptance-811473f9a/`；原始修复备份、首次失败、真实数据夹具和 7 项回归位于相邻 `mobile-main-acceptance-74a6c9744/`。各轮已签名 App、匹配 dSYM、APK 与数据原件均保留；仅退休已完成、无占用的本任务可重建中间产物。WebRTC 继续跳过；Chat、Computer Use 和其他 session WIP 不纳入。两个修复提交随本次进度报告一起整合到 main；最终远端提交以交付回执为准。

- 发布整合：按用户明确要求接入最新 main `dff63c360`（包含 Local Apps、消息间距及 iOS FFI 生成修复），无合并冲突；相对 main 最终只新增上述 3 个 iOS 文件和本报告。合并候选 `553b3fed2` 上重新编译并执行 7 项 Darwin XCTest，全部通过；725 个客户端源码和 3 份协议镜像检查通过，差异空白检查通过。没有将之前签名包记为该合并候选已重新打包／真机通过，iOS 最终 GUI 和额外 Android instrumentation 仍按上述边界待验收。


## 2026-10-22：旧会话恢复与 Side Chat 真机验收

- 已恢复 iPhone 11 / iOS 18.6.2 的镜像连接，用现有 Flare 签名 `a5b3dfac54b99e7c5fe5be826ba6d73a00823cf4` App 完成生产入口验证，没有启用 UI-test fixture、修改凭据或发送新的模型请求。本批无新增源码修复，也无旧版兼容层；原修复已在 `7c9fe7f7c` 进入 main。
- **旧会话恢复通过**：公开链接打开原会话后，v2 inbox 激活序号由 4 升为 5；随后普通重启、不再传入链接，仍恢复同一会话，序号升为 6。两个阶段均确认当前 `/private/var/...` 目录键、18 个原 catalog 文件哈希不变、完整历史字节前缀保留、provider-settings 字节不变、1327 模型缓存校验通过及进程存活。镜像实际显示原始鹈鹕动画对话，无新的会话不可用或 admission 错误；其中历史 `[Request interrupted by user]` 保留，不作为当前失败。
- **Side Chat 真机交互通过**：从上述真实主会话打开旁聊，用镜像硬件键盘输入 `2026102201`；隐藏后主会话草稿仍为空，重开旁聊保留输入。创建第二条旁聊，其草稿初始为空，输入 `2026102202` 后经真实 Side 1 / Side 2 列表切回第一条，原草稿保持 `2026102201`。第一条旁聊选择 DeepSeek V4 Pro 后草稿仍保留，隐藏旁聊后主会话仍为 Flash。操作后再次从真机读取 provider-settings，与测试前字节完全一致。两条测试草稿均未发送；本次不把镜像硬件键盘测试扩大为手机屏幕键盘触控验收。
- 镜像输入曾出现 `noWindowsAvailable`、短暂暂停和重连。一次自动审批因工具报告暂停而拒绝继续按键；随后先只读确认 Mirroir Connected 和 CoreDevice 无线显示仍可用，才恢复正常交互，没有绕过锁屏或降低系统保护。测试最后镜像再次重连，未完成测试旁聊清理；这不撤销此前已观察到的草稿隔离、隐藏重开和模型隔离结果。
- 原始设备回执：`.buildcache/remaining-task-takeover-20261019/mobile-main-acceptance-811473f9a/ios-device-receipt-connected-20261022.json`；两阶段文件、配置和进程证据位于 `ios-explicit-old-session-realpath-connected-20261022/`、`ios-restart-old-session-realpath-connected-20261022/`。GUI 观察汇总为同目录 `sidechat-physical-receipt-20261022.json`，原始截图由本会话 CUA 工具记录保存。
- 整合时接入 main `1418b97a4`。会话目录 helper、EngineConversationSource、SideChatSheet 和 ChatView 与受测 `a5b3dfac5` 源码字节相同；后续 main 的 Harness pin、时间线间距及其他功能不属于本次签名包，不能据此声称最新 main 全包真机验收完成。WebRTC 继续跳过；Android 额外 instrumentation、其他平台/发行项及其他 session WIP 不纳入本阶段。


## 2026-10-22：最新 main 签名包与真机复验完成

- **受测代码固定为 Product `406879ed2871983fd22fd56cc198f020ff7db1d0`**（开始及提交报告前两次 fetch 均为该 main），Harness `5a2de5b14e6c0d8d2e5bac9155c9c39accda86c7`、SDK `ff71b2ec1139bc67cc8719d78b1735aca967bbe8`、mobile-linux `6b033d41a2b6b736fda1e8f996f55a4b20bd885d`。没有复用旧 Harness 原生核心：官方脚本重新构建 Host 元数据库、生成 7 个 Swift/FFI 文件，再构建 ARM64 device release 库并组装 XCFramework。生成文件恰与旧绑定字节相同，但新核心来源独立记录，不能用它推断旧库可替代。既有 native-support/rootfs 仅在同一 immutable SDK 和哈希校验通过后复用。
- **构建／签名通过**：Xcode 27.1 `27A9275`、Rust 1.94.0；当前 StoreDebug App `2.4.1 / 8721` 完整构建成功，Flare Team `AZ4AX7J833` 签名、设备授权、iPhone/iPad 竖屏及 dSYM 匹配通过。新 debug dylib UUID 为 `D7979A20-00F6-35FD-A077-CB974D2D176E`；完整 App 文件清单 SHA-256 为 `726a3b6d091dc05ee5712195cec3ea9be5e6e85b91a16d21641c5cadad187c9d`。725 个客户端源码及 3 份协议镜像检查通过。未上传符号或故意触发崩溃。
- **保留数据升级与旧会话恢复通过**：先备份 provider-settings、preferences、inboxes 和 projects，再安装新包。iPhone 11 / iOS 18.6.2 上公开链接打开原会话 `9d838fee-9184-40fe-8604-2f2d43e9577a`，激活序号由 6 升为 7；普通重启不传链接仍选中同一会话，序号升为 8。两个阶段均确认 v2 身份、当前 POSIX 目录、32 个原 catalog 文件哈希不变、完整目标历史字节前缀保留、provider-settings 字节不变、1327 模型缓存校验和有效及进程存活。GUI 观察原对话正常，无新的 admission／会话不可用错误；历史 interruption 记录保留。
- **当前包 Side Chat 真机通过**：真实生产入口创建第一条旁聊，硬件键盘输入 `2026102203`，隐藏后主会话输入框仍为空，重开保留该草稿；第二条旁聊初始为空，输入 `2026102204` 后切回第一条，仍为 `2026102203`。第一条选择 V4 Pro 后草稿保留，主会话仍为 Flash。结束第一条后自动选中第二条，`2026102204` 与 Flash 均保持；结束第二条显示空 Side chats 列表，再关闭面板返回原会话。两条测试旁聊均已清理，未向模型发送测试草稿；最后再次核对 provider-settings 与升级前字节相同、原会话仍选中、序号至少 8 且进程存活。
- 镜像曾短暂暂停，按其公开 Resume 操作恢复。多显示器下 CUA 报 `noWindowsAvailable`，通过镜像自身 Window 菜单把窗口移到 S27 PRO 后恢复正常点击，不需要修改产品、绕过锁屏或调整安全权限。本次记录镜像硬件键盘，不扩大为手机屏幕键盘触控验收；GUI 原始截图与操作由本会话 CUA 工具记录保留。
- **证据和产物**：`.buildcache/remaining-task-takeover-20261019/ios-main-406879ed2-20261022/acceptance-index.json` 汇总 source、native archive 哈希、签名清单、旧会话两阶段回执和 Side Chat 回执；同目录保存原始构建日志、安装日志、升级前备份及旧 `a5b3dfac5` App/dSYM/native pair。新 App 为 `.buildcache/remaining-task-takeover-20261019/catalog-ios-xcode/Build/Products/StoreDebug-iphoneos/LingxiCode.app`，匹配 dSYM 在其旁边，已安装并留在原会话。此次无新增源码修复、无兼容层；只提交验收报告。WebRTC、其他平台、真实云端推理及发行渠道继续按既有范围排除；其他 session 未提交源码保留。

- 发布前并发整合 main 的 `fd23050c0`、`34fb5bd55`、`1a2950901`（仅 6 个 Electron 文件）。已逐项核对 iOS 源码、生成输入和所有依赖未变化；本批签名包覆盖最终 main 的 iOS 代码组合，无需为桌面改动重建手机包。其他 session 的 11 个未提交路径及 staged/unstaged 状态逐项保留。

## 2026-10-22：移动 Crashlytics 真机上报与原生符号化

- 基于 main `45d15a7e4`，SDK、Harness 和 mobile-linux 来源不变。iOS 仅新增 DEBUG 显式诊断入口；Android 仅扩展默认跳过的 instrumentation 和测试 APK 原生 fixture，不向主 APK 添加崩溃入口。没有打开 Debug 持续采集，没有旧协议兼容层。
- **Android 真机通过**：Samsung SM-S9310 上有意的 NDK SIGSEGV 被 SDK 捕获，既有一次性发送测试通过；Firebase 中的测试 UUID 精确匹配。问题 `b944655aca5f522a545072b293497e69`、事件 `2278137104560685610` 的崩溃线程解析到 `CrashlyticsNdkVerificationLeaf` / `crashlytics_verification.c:6`，与匹配 ELF 的 GNU build ID 和本地 addr2line 一致。测试 APK 使用该设备既有 Debug 身份签名，未卸载主 App 或清除数据。
- **iOS 真机通过**：Flare StoreDebug 2.4.1 / 8721 签名构建、dSYM 上传及保留数据安装通过。iPhone 11 / iOS 18.6.2 的未捕获 `CrashlyticsDeviceVerification` NSException 云端入库，问题 `925eb6051ce7dd7c5069d43d8d07482d`；完整事件名中的 SDK session `152959a7ac6c491e97551c2dd841af8e` 与发送前原始捕获目录一致。云端解析到 `CrashlyticsDeviceVerification.swift:51`，匹配 debug dylib dSYM `0E02F537-EB9C-3B90-B76E-97F121A3E638`。SDK 在等待 Debug 采集同意时延迟写自定义字段，本次采用精确 SDK session ID 关联，不把本地 UUID 误写为云端已有字段。
- **保留失败与边界**：首次 Swift SIGTRAP 有本地捕获／一次性发送回执，但未观察到云端事件；没有把它改写为通过。后续仅增加一次不同异常类型的有界对照，未大规模重采。测试库符号化不代表每个生产 Rust／第三方堆栈均已验证，也未做新的 Release 发行验收。
- **恢复与保护**：两端原 provider 配置逐字节一致。Android 主 App 已正常重新打开；iOS 已无诊断环境正常重启，原会话选择仍在、进程存活。签名、构建日志、原始 SDK 捕获文件、云端测试事件、匹配符号和数据备份保留在 `.buildcache/remaining-task-takeover-20261019/mobile-crashlytics-20261022/`。WebRTC 继续跳过，Chat、Computer Use 与其他 session 修改不纳入。
- Desktop production 凭据稳定性仍在本轮后续执行；不会将 development 通道的结果代替 production 验收。
