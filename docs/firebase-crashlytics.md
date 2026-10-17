# Firebase Crashlytics

项目为 `lingxi-5e4b2`。Android Play / Direct（含 Debug）沿用各自的 Firebase app；
iOS Store / Full 沿用对应 bundle ID 的配置，不依赖或修改 APNs 密钥。

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
