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
