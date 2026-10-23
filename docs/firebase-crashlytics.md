# Firebase Crashlytics

项目为 `lingxi-5e4b2`。Android Play / Direct（含 Debug）沿用各自的 Firebase app；
iOS Store / Full 沿用对应 bundle ID 的配置，不依赖或修改 APNs 密钥。

## macOS Desktop

Electron 主进程通过 Node-API 8 插件加载进程内的 `LingXiDesktopCrashlytics.framework`，
框架链接官方 Firebase Apple SDK 12.19.2。不是通过独立 helper 捕获主进程崩溃。
签名 macOS 包在创建窗口前初始化 SDK，自动捕获主进程的原生信号/Mach 崩溃；
下次启动由 SDK 处理持久化报告。未打包的开发进程不启用云端采集。

- 开发签名包使用 Apple app `1:55470483519:ios:ee06a2bd809d9f2d57708a`，
  bundle ID 为 `com.lingxi.code.development`，显示名称为 LingXi macOS Desktop Development。
- 生产包 bundle ID 是 `com.lingxi.code`，与现有 iOS Store bundle ID 相同，因此使用该
  bundle ID 对应的现有 Apple app；通过操作系统和 `distribution=desktop-production` 区分。
  不能把现有 Desktop Web Firebase app ID 用于原生 Crashlytics。
- renderer / GPU / Utility 进程崩溃通过 Electron 退出事件记录为带分类和退出码的
  **非致命错误**，不是这些子进程的原生完整堆栈。正常退出和主动终止不上报。
- 主进程 JS 未捕获异常通过 `uncaughtExceptionMonitor` 观察，保留 Node 的默认退出行为；
  不吞掉错误继续运行。当前云端记录只有固定分类，未上传 JS 原始消息或 JS 堆栈。
- Rust bridge-server、Bun worker 不是 Electron 子 renderer，未因本接入而获得原生
  Crashlytics 堆栈采集；它们仍使用现有运行时日志。Windows/Linux 未接入 Apple SDK。
- 本地诊断在 Electron userData 的 `logs/crashes.jsonl`；原生日志和报告位置由 Apple SDK
  管理。只附加静态 `distribution` / `build_type`，不追加账号、提示词、路径或凭据字段。
  SDK 的标准报告仍含系统、设备、随机安装标识和二进制信息。

使用既有 Flare 签名入口，构建额外要求 XcodeGen 和 Node-API 头文件。通常从 Node
安装目录自动查找，也可用 `LINGXI_NODE_API_HEADERS` 指定 `node_api.h` 所在目录。
原生框架通过 Xcode/SPM 构建，依赖锁定在 `native/crashlytics/Package.resolved`。
包验证会检查框架、插件和匹配 Firebase 配置；本地构建失败不能跳过该检查。

```sh
cd apps/electron
npm run package:mac:flare -- --launch
# 每次发布保留该包旁 symbols/ 中的精确 dSYM，再通过官方上传器上传：
node scripts/upload-crashlytics-symbols.mjs development /absolute/package/symbols
# 生产渠道对应 production；上传前须获得对该项目的符号传输授权。
# 显式隔离验收：仅创建和故意终止测试进程，保留临时目录作为证据。
node scripts/verify-crashlytics.mjs '/absolute/LingXi Code.app' --confirm-isolated-test-crash
```

Electron Framework 自身的符号必须来自同一 Electron 版本、架构的官方 dSYM 包，
并以 `dwarfdump --uuid` 核对实际框架。不能用另一次构建或相近版本的符号替代。
Bridge dSYM 上传成功不代表 Electron Framework 已符号化；本地捕获成功也不代表
云端收到报告。隔离崩溃测试必须使用临时 HOME / CFFIXED_USER_HOME 和临时 userData，
不能对用户正在使用的桌面进程执行故意崩溃。

2026-10-19（America/Los_Angeles）验收：Node-API / Apple SDK 原生编译、3 项诊断
策略回归、主进程类型检查，以及 Flare 签名包静态和完整 packaged smoke 通过。
真实签名 Electron 的隔离 `process.crash()` 后，SDK 重启状态从 false 变为 true；
Firebase 云端确认收到最终包测试会话 `53d77ce1a02945b9bd65077548ef2b43` 的 `MAC_OS_X` / `FATAL`
事件，问题 ID 为 `c810e3a23a5bd8facac9b77fbbfbef9c`，堆栈解析为
`electron::ElectronBindings::Crash()` / `electron_bindings.cc:118`。这是故意触发的 `EXC_BAD_ACCESS`，
不是先前用户桌面 SIGTRAP 的根因或补录。
桥接框架、Node-API 插件、官方 Electron 启动器和 Electron Framework 的精确 dSYM
均通过官方上传器上传；Electron Framework UUID 为 `4C4C448F-5555-3144-A1B7-32E3D8CE2AED`。
不把上传成功单独视为所有云端历史事件已经完成源码行号符号化。

## 采集范围

- Android 使用 Firebase BoM 管理 `firebase-crashlytics-ndk`，采集 JVM 崩溃、ANR 和原生崩溃；
  Gradle 插件固定为 3.0.8。iOS 使用现有 Firebase SPM 版本中的 `FirebaseCrashlytics`。
- Release 默认自动上报，Debug 默认关闭自动上报。Android 由合并后的 Manifest 决定；
  iOS 由构建时展开的 `CRASHLYTICS_COLLECTION_ENABLED` 决定，在 Firebase 初始化前生效。
  不写入跨构建保留的运行时采集开关。
- 仅添加 `distribution`、`build_type` 两个静态自定义字段。没有 `setUserId`，
  不添加邮箱、账号 UID、推送安装密钥、FID、提示词、会话内容或 provider 凭据。
  SDK 本身仍会收集标准崩溃堆栈、设备/系统/版本信息及随机安装标识。
- 不新增 Google Analytics、自动业务日志或捕获所有业务异常的处理器。
  两端原有 `telemetry` 字段只是未连接的历史内存状态，不作为持久化授权开关。

## Android 原生符号

Rust/JNI 的构建脚本启用行号信息，并在 strip 前保存每个渠道、ABI 的精确未剥离库：
`app/build/nativeSymbols/{play,direct}/{arm64-v8a,x86_64}/libandroid_aar.so`。
这些库和 JNI / Kotlin bindings 参与同一发布/回滚流程，不能用其他构建的同名库替代。
目录均被 Git 忽略；第三方原生库仍需要其发布方提供的匹配符号。

```sh
cd apps/android/native
./scripts/build-jni.sh --variant play
# 按既有 MobileLinux 发布流程准备原生资产后，构建并上传同一次构建的符号：
./gradlew :app:assemblePlayRelease :app:uploadCrashlyticsSymbolFilePlayRelease
# Direct 对应 build-jni.sh --variant direct 和 DirectRelease 两个任务。
```

Crashlytics 的 NDK 上传任务必须显式执行；普通 Debug 编译不上传符号。
发布产物应同时归档匹配的未剥离库，保留 ELF GNU build ID；后续重新编译不能还原旧版本符号。

## iOS 符号

`project.yml` 链接 Crashlytics，并为四个构建配置生成 dSYM。应用的最后一个构建脚本使用
SPM 中官方 `Crashlytics/run`，仅在 iPhoneOS Release 构建时上传；输入包括 dSYM、
所选 app 的 `GoogleService-Info.plist` 和实际可执行文件。
本地 Debug / 模拟器构建会跳过上传，保留原有签名及 APNs 设置。

```sh
cd apps/ios/native
xcodegen generate
# 按既有签名流程构建 StoreRelease / FullRelease 或 Archive。
```

## 显式真机验证

`CrashlyticsDeviceTest` 的两个测试默认跳过。先备份应用数据，保留数据安装 Play Debug
及对应 AndroidTest APK，再明确选择以下方法；第一个方法会有意终止应用和测试进程。
不要对正在执行的会话运行此检查。

```sh
adb -s <serial> shell am instrument -w -r \
  -e class 'com.lingxi.code.diagnostics.CrashlyticsDeviceTest#captureFatalCrash' \
  -e liveCrashlyticsProject lingxi-5e4b2 -e verificationId <随机UUID> \
  com.lingxi.code.debug.test/androidx.test.runner.AndroidJUnitRunner
adb -s <serial> shell am instrument -w -r \
  -e class 'com.lingxi.code.diagnostics.CrashlyticsDeviceTest#sendCapturedReport' \
  -e liveCrashlyticsProject lingxi-5e4b2 \
  com.lingxi.code.debug.test/androidx.test.runner.AndroidJUnitRunner
```

第二步通过 `sendUnsentReports()` 一次性发送已捕获报告，不打开后续 Debug 自动采集。
应核对 SDK 的上传成功日志以及 Firebase 控制台中该 UUID 对应的事件；本地崩溃文件和
编译成功不代表云端已收到报告。`verification_id` 仅由显式测试添加。

### Android NDK 真机验证

`captureNativeFatalCrash` 也默认跳过，仅在上述项目 opt-in 参数存在时执行。
先运行 `scripts/build-crashlytics-test-library.sh`，再构建 `assemblePlayDebugAndroidTest`。
脚本使用本机 NDK 29 为 ARM64 编译带 DWARF、GNU build ID 和 16 KiB 对齐的测试库，
仅放入测试 APK 的 assets；不进入主 App。使用与设备主 App 相同的既有 Debug 签名，
不要为解决签名不一致而卸载主 App 或清除数据。

显式选择 `CrashlyticsDeviceTest#captureNativeFatalCrash` 并传入新的 `verificationId`；
测试拒绝已有待发送报告，加载测试库后执行一次有意的 SIGSEGV。随后使用上述
`sendCapturedReport` 一次性发送，并正常重新打开主 App。用官方 Firebase CLI 上传
`app/build/crashlytics-verification/arm64-v8a` 中的匹配符号：

```sh
firebase crashlytics:symbols:upload --app <当前 Play Debug Firebase app ID> \
  app/build/crashlytics-verification/arm64-v8a
```

验收要求云端测试 UUID、FATAL、函数名和 C 源码行同时匹配。测试库通过不代表每个
Rust 或第三方库的堆栈均已覆盖；生产 JNI 仍必须保留并上传各自精确匹配的符号。

### iOS 显式验证入口

仅 DEBUG 构建包含 `CrashlyticsDeviceVerification`。使用不附加调试器的设备启动入口，
同时传入 `LINGXI_CRASHLYTICS_PROJECT=lingxi-5e4b2`、新的
`LINGXI_CRASHLYTICS_VERIFICATION_ID=<UUID>`、`LINGXI_CRASHLYTICS_PHASE=capture` 和
`LINGXI_CRASHLYTICS_KIND=swift-sigtrap`（Swift fatalError）或 `objc-exception`（未捕获 NSException）。
缺少任一有效参数或自动采集已打开时不执行。capture 拒绝已有待发送报告，同一 UUID
不能重复触发；SDK 初始化后有界等待，再触发明确选择的异常类型。
诊断进程临时启用 SDK debug 日志，不持久化采集或日志覆盖。

重启时保留相同项目、UUID 和 KIND，改为 `LINGXI_CRASHLYTICS_PHASE=send`，仅在本地
capture-ready 回执、异常类型和前次崩溃状态均匹配时调用一次性发送。回执位于
`Library/Caches/CrashlyticsDeviceVerification/receipt.json`。SDK 12.19.2 在等待 Debug
采集同意时会延后自定义字段写入，因此不能只依赖云端 verification_id；发送前保留
原始捕获目录，以其精确 SDK session ID、时间和匹配 dSYM 交叉核对云端事件。
完成后不带这些环境变量正常启动，不改变持续采集开关。

2026-10-22（America/Los_Angeles）补充验收：

- Samsung SM-S9310 的原生测试库事件 `2278137104560685610` 已在 Android 云端入库，
  问题 ID `b944655aca5f522a545072b293497e69`；测试 UUID 精确匹配，崩溃线程解析为
  `CrashlyticsNdkVerificationLeaf`、`crashlytics_verification.c:6`。测试库 GNU build ID 为
  `fc6d919af9bc6e0c2a21ae765852b26be95a316a`，本地 addr2line 与云端一致。
- iPhone 11 / iOS 18.6.2、StoreDebug 2.4.1 / 8721 的未捕获 NSException 已云端入库，
  问题 ID `925eb6051ce7dd7c5069d43d8d07482d`，事件
  `152959a7ac6c491e97551c2dd841af8e_2278143465221146841` 与发送前原始 SDK session ID
  精确一致；云端解析到 `CrashlyticsDeviceVerification.swift:51`，匹配 debug dylib dSYM
  UUID `0E02F537-EB9C-3B90-B76E-97F121A3E638`。此前 Swift SIGTRAP 对照尚未在云端
  观察到，不将其记为通过，也不据此声称所有原生信号均已验证。
- 两端 Debug 自动采集保持关闭，provider 配置字节保留；未上传业务会话、提示词或凭据。
  原始证据、匹配符号和回执保留在
  `.buildcache/remaining-task-takeover-20261019/mobile-crashlytics-20261022/`。

## 当前验收记录

2026-10-16（America/Los_Angeles）：

- Play / Direct Kotlin 及 AndroidTest 编译、四个 Manifest 的采集开关检查通过。
  Play Debug APK 和 AndroidTest APK 打包成功，最终 APK 的既有原生资产校验通过；
  3 项推送策略测试仍通过。Play / Direct Release 的符号生成任务图检查通过。
- Samsung SM-S9310 保留数据更新后，显式主线程崩溃被 SDK 捕获；重新运行一次性发送测试通过。
  云端完整报表在本轮时间范围中确认收到 `CrashlyticsVerificationException`，类型 `FATAL`，
  问题 ID 为 `f70dde07cf9d395d301f544384a5824e`。Debug 自动采集始终关闭。
  应用已重新打开，推送安装 UUID / 密钥保持一致，原账号在重启后成功完成服务端续期。
  SDK 登录缓存可能正常重写，不通过回退加密缓存来“恢复”状态。
- iOS FullDebug 的 arm64 iPhoneOS 应用编译、链接及 dSYM 生成通过；FullRelease 的采集和
  dSYM 设置核对通过；官方 `upload-symbols --validate` 对本轮 app 配置 / dSYM 校验成功。
- 原生符号发布脚本 14 项回归通过，包含 strip 前字节保留及失败后的 JNI / bindings / 符号回滚。
  本轮未重新构建完整 Rust 原生库，也未做 NDK 原生崩溃的云端符号化验收。
  iOS 的真机崩溃上报及实际 Release dSYM 上传仍需在后续构建中验收。

参考：[Android](https://firebase.google.com/docs/crashlytics/android/get-started)、
[NDK 符号](https://firebase.google.com/docs/crashlytics/android/get-started-ndk)、
[iOS dSYM](https://firebase.google.com/docs/crashlytics/ios/get-deobfuscated-reports)。


### Swift SIGTRAP 真机云端复核（2026-10-23 UTC）

- Firebase iOS App 已注册：`com.lingxi.code`，App ID
  `1:55470483519:ios:dd3320da548259cd57708a`；不是缺少注册导致的失败。
- SDK 12.19.2、iPhone 11 / iOS 18.6.2、StoreDebug 2.4.1 (8721)，
  不附加调试器触发一次 `swift-sigtrap`，在发送前保存原始 Mach 报告。
  SDK session `667f3a781dd34c4f8cef6d2735c1b3a3`，异常 `EXC_BREAKPOINT`。
- 重启后显式发送；云端事件
  `667f3a781dd34c4f8cef6d2735c1b3a3_2278168563696168780`，
  issue `5cb41e6bda8a46079760fe5329c3677e`。eventTime 为
  `2026-10-23T04:57:33Z`，receivedTime 为 `04:59:01Z`。
  FATAL 堆栈解析到 `CrashlyticsDeviceVerification.swift:58` 的 Swift fatalError；
  匹配 debug dylib dSYM UUID `100707A9-8923-3CC5-9B80-6AD8514AC681`。
  云端 issue 的归因标题选中 App 入口，但实际 crashed thread 的触发帧已正确符号化。
- 原始 session `63ec06342b1d4c239af5d08522b06c20` 的 Apple IPS 同样确认
  Swift SIGTRAP，匹配旧 dSYM 并解析到当时的触发行；Crashlytics Mach handler
  出现在异常服务器线程。原获批时间窗内的云端查询仍无该事件，且当时未保留完整
  传输回执，无法据此断定历史报告消失的原因。不能把此次复核成功写成已定位历史根因。
- 修复的是验收入口只支持 NSException、无法单独复核 Swift trap 的缺口；没有替换 SDK
  异常处理器或声称修复了未经证实的 Firebase 缺陷。发送入队、本地文件清理与云端确认
  分开记录；后续异常须保留发送前原件及 SDK session。
- 原始证据位于 `.buildcache/remaining-task-takeover-20261019/mobile-crashlytics-20261022/sigtrap-followup-20261022/`，
  包括 `original-sigtrap.ips`、`reports-pre-send/`、`confirmed-swift-cloud-event.json`、
  `build-signature-symbols.json` 及 `symbol-upload.log`。私有日志与配置备份不提交仓库。

依据：[Firebase 测试上报流程](https://firebase.google.com/docs/crashlytics/ios/test-implementation)、
[Apple Swift runtime trap](https://developer.apple.com/documentation/xcode/addressing-crashes-from-swift-runtime-errors)。
相似历史问题 [firebase-ios-sdk #4798](https://github.com/firebase/firebase-ios-sdk/issues/4798)
曾由符号上传解决，但不能据此认定当前 SDK 存在同一问题。
