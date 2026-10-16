# RC3 发布与清机构建验收（2026-10-16）

发布：[v0.1.0-rc.3](https://github.com/lingxi-coder/mobile-linux-runtime/releases/tag/v0.1.0-rc.3)。
SDK / rootfs 二进制源码 `6b033d41a2b6b736fda1e8f996f55a4b20bd885d`；
SwiftPM wrapper / Release tag `d278c1e76b8f7faa41113da72bad50b8347ae170`。
产品继续使用已完成真机验证的 Harness `a9002383d91d743da8bc3f42f3459d005fe615b5`。

## 已发布内容

14 个资产均已公开，GitHub 返回的资产大小与 SHA-256 和本地严格打包结果逐个一致。
没有覆盖 RC1 或 RC2。

- Android `3.24.2-user1` rootfs：arm64-v8a / x86_64，各有完整 evidence ZIP。
- iOS `3.24.2-user1` base fakefs staging bundle（rootfs ZIP、RootfsPatch、vDSO、portable manifest）。
- 完整 Android Maven SDK `0.1.0-rc.3`。
- 产品专用 native-only Maven 包（本地坐标 `0.1.0`）与 Android native-support bundle。
- iOS Release FFI / NativeSupport XCFramework，含 device arm64、simulator arm64/x86_64。
- Swift package ZIP、`SHA256SUMS`、SDK 与 rootfs 两份 provenance JSON。

SDK Release 来自干净源码，`validation_only=false`，绑定与所有目标库的 ABI 元数据一致。
Android rootfs SHA：

- arm64：`043363aa7a21ff3b3095652de2849d9d6a3a2546fc8012c082383dfed32a37a8`。
- x86_64：`d1aeaf2611692032f688c6eee5484234adca332033b0466b4771e7273fc93685`。

iOS fakefs ZIP SHA：`221a297f6a1dea1509657adf20a073d46384959d443ce815298cebf4bbf39138`。

## 实际下载及构建验证

- 空下载缓存、隔离产品工作区执行 `prepare-sdk-release.py --platform all` 成功。
  公共 HTTPS URL 无需 GitHub token；两种 Android ABI 的归档、锁、许可与不可变 inventory 均通过校验。
  原生 Maven、NativeSupport 与 iOS rootfs 安装后再次通过 SDK / 产品来源检查。
- 从 GitHub 新 clone SDK main，SwiftPM 从 RC3 公开 URL 解析两个 XCFramework 成功。
- 独立 SDK iOS 消费者：simulator arm64/x86_64 编译链接成功；device arm64 编译链接成功（关闭签名）。
- 独立 SDK Android 消费者：从公开 URL 安装完整 Maven ZIP，再构建 SDK 示例 APK 成功。
- 完整 Android 产品：在隔离工作区重新编译两个 ABI 的 JNI、生成同源 Kotlin 绑定，APK 构建与最终 runtime closure 检查通过；964 项 JVM 测试，0 失败。
- 完整 iOS 产品：device arm64、simulator arm64/x86_64 的产品 FFI 与同源 Swift 绑定生成成功；完整应用双架构模拟器与 iPhone arm64 的 Xcode 编译链接均通过（产品 FFI 为 Debug，关闭应用签名）。
- 产品脚本回归：26 项 SDK/准备流程测试与 3 项 rootfs 下载测试通过。
- 固定 llm-client `24f49956` 也已从空 Git 对象库直接远端 fetch 成功，无需本机独有对象。

新的独立工作区缺少项目调试 keystore，按构建说明生成新的本地测试密钥后 Android 构建成功。
未复制、读取或发布现有开发私钥。Google 登录仍需为该本机证书注册公开指纹。
iOS 打包首次因本机磁盘不足失败，保留验收日志并清理闲置可再生缓存后重跑。
本记录覆盖产物发布、解析与编译链接；此前真实产品真机运行结果见[默认用户验收](default-user-validation-2026-10-16.md)。

## 本地日志

- `/tmp/lingxi-sdk-release-rc3/remote-assets.json`
- `/tmp/lingxi-sdk-release-rc3/clean-product-prepare.log`
- `/tmp/lingxi-sdk-release-rc3/swift-resolve.log`
- `/tmp/lingxi-sdk-release-rc3/sdk-consumer-simulator.log`
- `/tmp/lingxi-sdk-release-rc3/sdk-consumer-device.log`
- `/tmp/lingxi-sdk-release-rc3/maven-consumer-install.log`
- `/tmp/lingxi-sdk-release-rc3/android-sdk-consumer-build.log`
- `/tmp/lingxi-sdk-release-rc3/clean-product-android-build2.log`
- `/tmp/lingxi-sdk-release-rc3/clean-product-ios-ffi2.log`
- `/tmp/lingxi-sdk-release-rc3/clean-product-ios-simulator-build.log`
- `/tmp/lingxi-sdk-release-rc3/clean-product-ios-device-build.log`
