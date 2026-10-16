# 从 main 构建默认用户版本

发布页：[Mobile Linux SDK v0.1.0-rc.3](https://github.com/lingxi-coder/mobile-linux-runtime/releases/tag/v0.1.0-rc.3)。
SDK 二进制源码固定为 `6b033d41a2b6b736fda1e8f996f55a4b20bd885d`，与产品 Cargo.lock 一致。
Android rootfs 为 `3.24.2-user1`，支持 arm64-v8a / x86_64；iOS 提供相同默认用户的 base fakefs 镜像。
默认用户 `lingxi`（UID/GID 1000），HOME `/home/lingxi`，工作目录 `~/lingxi`。

准备脚本从公开 Release 下载，核对 main 中提交的 SHA-256、SDK 来源 SHA、完整文件清单和原生支持元数据。
输出在忽略的 build / Frameworks 目录，下载缓存默认在 `target/mobile-linux-release-cache`。
无需复制其他开发机器的 `.so`、XCFramework、Maven 仓库或 rootfs。
通用工具：Python 3.11+、Node.js（构建脚本使用）、Git、C/C++ 编译工具、CMake 和 pkg-config。

## Android

前提：JDK 21、Android SDK/NDK、仓库固定的 Rust toolchain，以及 `cargo-ndk`。设置 `ANDROID_HOME`（或 `ANDROID_SDK_ROOT`）指向 SDK；脚本会选择其 `ndk` 目录中已安装的版本，也可显式设置 `ANDROID_NDK_HOME`。安装所需组件后，从产品仓库根目录执行：

```sh
# 首次在本机构建时生成项目调试密钥（已有密钥则保留）。
if [ ! -f apps/android/native/.signing/lingxi-5e4b2-debug.keystore ]; then
  mkdir -p apps/android/native/.signing
  keytool -genkeypair -keystore apps/android/native/.signing/lingxi-5e4b2-debug.keystore \
    -storepass android -keypass android -alias androiddebugkey -keyalg RSA \
    -keysize 2048 -validity 10000 -dname 'CN=Android Debug,O=Android,C=US' -noprompt
fi
python3 scripts/mobile-linux/prepare-sdk-release.py --platform android
bash apps/android/native/scripts/build-jni.sh --variant direct
cd apps/android/native
./gradlew :app:assembleDirectDebug
```

新生成密钥可用于本地构建；Google 登录还需要按 [Firebase 配置](../firebase-auth.md) 注册该证书的公开指纹。私钥不随 Release 或 Git 发布。

构建 Store 变体时，将后两条命令的 `direct` / `DirectDebug` 换为 `play` / `PlayDebug`。
准备脚本安装原生支持和 installer 的 Maven 包，下载两种 ABI 的 rootfs，并为两个变体暂存资源。
产品 JNI 由固定 Harness 和 SDK 的 Rust 源码构建，保持 Kotlin/JNI 同源；不链接独立 SDK 的第二份 Rust FFI。
构建结束会检查最终 APK 的 rootfs、原生库和来源闭包。

## iOS

前提：macOS、Xcode（iOS 18+ SDK）、XcodeGen、仓库固定的 Rust toolchain 与 iOS targets。
从产品仓库根目录执行：

```sh
python3 scripts/mobile-linux/prepare-sdk-release.py --platform ios
LINGXI_REUSE_STAGED_LINUX_RUNTIME=1 LINGXI_FFI_PROFILE=debug \
  bash apps/ios/native/scripts/build-xcframework.sh
bash apps/ios/native/scripts/install-sherpa-runtime.sh
cd apps/ios/native
xcodegen generate
xcodebuild -project LingxiCode.xcodeproj -scheme LingxiCode \
  -destination 'generic/platform=iOS Simulator' build CODE_SIGNING_ALLOWED=NO
```

准备脚本安装 device arm64、simulator arm64/x86_64 的 NativeSupport，以及已验证的 rootfs、RootfsPatch 和 vDSO。
`build-xcframework.sh` 校验并使用这些原生资源，再从固定 Rust 源码生成产品引擎和匹配的 Swift 绑定。
真机安装需要本机配置的 Apple 签名；模拟器可完成编译和链接，Linux guest 执行需要物理 iPhone。

## 独立 SDK 消费者

SDK main / `v0.1.0-rc.3` 的根 `Package.swift` 已固定 Release URL 和校验和，可直接通过 SwiftPM 使用。
独立 Android SDK Maven 包为 `mobile-linux-maven-0.1.0-rc.3.zip`，内部版本 `0.1.0-rc.3`。
通过 SDK 的 `scripts/release/install-maven-bundle.py` 安装，并传入发布的 SHA-256 和完整源码 SHA。
产品专用的 `mobile-linux-native-maven-0.1.0-rc.3.zip` 保持本地坐标版本 `0.1.0`，只含 installer/native-support。

发布提供 `SHA256SUMS`、`release-provenance.json`、`rootfs-provenance.json`、每个 Android ABI 的完整 evidence。
iOS rootfs ZIP 是 staging bundle，包含 portable manifest 和 `resources/alpine-rootfs.zip`，按准备脚本暂存。
Rust 来源不变；产品此前的真机记录见 [默认用户验收](default-user-validation-2026-10-16.md)。

本次实际下载与编译结果见 [RC3 发布验收](rc3-release-validation-2026-10-16.md)。
