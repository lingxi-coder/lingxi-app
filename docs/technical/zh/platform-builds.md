# 原生构建、绑定与发布验收

构建匹配的 Rust／原生产物、选择分发变体，并区分编译、模拟器与实体设备验收。

<a id="build-matrix"></a>

## 平台与分发矩阵

公开产品能力由宿主、目标 OS、原生回调就绪状态与分发 flavor 共同选择。产品版本字段属于各产物；iOS marketing／build 版本、Android versionName／versionCode、Electron package 版本与 Rust crate 版本不能被当作统一 SDK 修订。源码基线与产品依赖 pin 才提供可复现身份。

| 目标 | 当前构建契约 | 验收边界 |
| --- | --- | --- |
| Android 产品 | minSdk 26；compileSdk 37；targetSdk 35；arm64-v8a 与 x86_64。 | JNI 打包及设备 instrumentation 行为。 |
| Android play / direct | play 为 store，关闭高风险原生 offload；direct 为 full，使用独立应用 ID。 | 编译时 flavor 选择加运行时权限决定。 |
| iOS 产品 | iOS 18.0；真机 arm64；arm64／x86_64 模拟器；Store／Full 配置。 | 签名真机 guest 执行；模拟器仅 API／链接检查。 |
| Desktop macOS | 通过 package:mac:flare 构建 Electron、Rust sidecar 与签名凭证 broker。 | 静态包验证及打包后运行 smoke。 |
| CLI / Web | Rust 二进制／静态 React-Vite 官网。 | 终端／provider 行为／浏览器文档行为。 |

<a id="binding-artifacts"></a>

## 生成一组匹配的绑定／库

Android build-jni.sh 为两种受支持 ABI 构建 android-aar，按配置生成各 UniFFI 组件，并在替换前暂存归属 Kotlin 包及 libandroid_aar.so，保留手写 Java 与其他原生辅助程序。play／direct 选择不同 JNI 输出。不要向正在运行的 Gradle 构建只复制新绑定或只复制新库。

iOS build-xcframework.sh 生成 Swift／C 头文件／modulemap、构建真机及模拟器 Rust 切片，再组装 LingxiCodeFFI.xcframework。MobileLinuxNativeSupport 含 iSH glue，不含另一份 Rust core；静态框架链接进应用，并非嵌入 dylib。可选隔离输出根目录先暂存匹配框架与绑定，再发布到构建目录。

LINGXI_SIM_ARM64_ONLY=1 生成仅供本地测试的模拟器框架，不能分发；LINGXI_DEVICE_ONLY=1 选择真机路径。只有构建脚本验证源码及产物匹配后才可复用生成绑定或暂存原生运行时；便利 flag 不能替代发布来源验证。

### 产品原生构建入口

```bash
bash apps/android/native/scripts/build-jni.sh --variant play
bash apps/android/native/scripts/build-jni.sh --variant direct
bash apps/ios/native/scripts/build-xcframework.sh
(cd apps/ios/native && xcodegen generate)
```

验证方式: 需环境验证

<a id="release-flow"></a>

## 发布装配与资源身份

将产品锁定 SDK revision 与对应 native-support 发布、压缩包 SHA-256、manifest、SBOM 及平台资源集配套使用。Android 打包提取后的可执行辅助库；iOS 使用转换的 fakefs ZIP、RootfsPatch.bundle、VDSO 与稳定工作区挂载。修改源码 pin 后须刷新所有依赖原生产物并验证包清单，不能只编辑 manifest。

Android store／full 标志描述编译能力，运行时仍须获得权限。iOS XcodeGen 从 project.yml 保留 target、分发与签名设置。macOS 打包须保留已配置 team 及正确 development 应用／broker profile；flare 包装负责预检与经过验证的签名。凭证属于运行时输入，不能编译到 framework 或 APK 资源。

1. 选择源码／依赖 revision 及分发配置。
2. 生成匹配绑定、Rust 库及 native support；验证资源哈希与 ABI 清单。
3. 通过平台入口构建并签名最终产品。
4. 分别运行静态包、模拟器／宿主及实体设备检查。

<a id="release-checks"></a>

## 测试层级与常见故障

宿主 Rust 编译验证 facade feature 与 FFI 类型；Swift／Kotlin 编译验证生成转换器；模拟器测试验证 UI 与链接；设备 smoke 验证实际原生执行。压缩包检查在执行前建立资源信任，打包 smoke 验证启动后的 sidecar／服务身份。记录实际通过的层级，不能把构建绿灯扩大成硬件验收。

未定义 FFI 符号表示绑定／库／组件不匹配；Android helper 缺失或 ELF ABI 错误属于打包；模拟器 guest 不可用是预期行为；RestartRequired 对应 iOS 进程身份；profile 拒绝属于签名；只有源码检查成功而没有匹配发布来源的产物不能视为可分发。设备网络／内存、系统权限、挂起／恢复及音频归属清理仍须明确验收。

- [原生回调 facade](platform-integration.md)
- [独立 Android SDK](mobile-android.md)
- [独立 iOS SDK](mobile-ios.md)

## 源码与文档基线

[源码仓库](https://github.com/lingxi-coder/lingxi-app/tree/main/apps/ios/native/scripts/build-xcframework.sh)

- [lingxi-app/apps/ios/native/scripts/build-xcframework.sh](https://github.com/lingxi-coder/lingxi-app/blob/bf12d8217b18d91a37ae51ed90a9be0b975fb9b0/apps/ios/native/scripts/build-xcframework.sh) — 已提交源码 (bf12d8217b18d91a37ae51ed90a9be0b975fb9b0)
- [lingxi-app/apps/ios/native/scripts/build-linux-runtime.sh](https://github.com/lingxi-coder/lingxi-app/blob/bf12d8217b18d91a37ae51ed90a9be0b975fb9b0/apps/ios/native/scripts/build-linux-runtime.sh) — 已提交源码 (bf12d8217b18d91a37ae51ed90a9be0b975fb9b0)
- [lingxi-app/apps/ios/native/scripts/verify-linux-runtime.sh](https://github.com/lingxi-coder/lingxi-app/blob/bf12d8217b18d91a37ae51ed90a9be0b975fb9b0/apps/ios/native/scripts/verify-linux-runtime.sh) — 已提交源码 (bf12d8217b18d91a37ae51ed90a9be0b975fb9b0)
- [lingxi-app/apps/ios/native/project.yml](https://github.com/lingxi-coder/lingxi-app/blob/bf12d8217b18d91a37ae51ed90a9be0b975fb9b0/apps/ios/native/project.yml) — 已提交源码 (bf12d8217b18d91a37ae51ed90a9be0b975fb9b0)
- [lingxi-app/apps/android/native/scripts/build-jni.sh](https://github.com/lingxi-coder/lingxi-app/blob/bf12d8217b18d91a37ae51ed90a9be0b975fb9b0/apps/android/native/scripts/build-jni.sh) — 已提交源码 (bf12d8217b18d91a37ae51ed90a9be0b975fb9b0)
- [lingxi-app/apps/android/native/app/build.gradle.kts](https://github.com/lingxi-coder/lingxi-app/blob/bf12d8217b18d91a37ae51ed90a9be0b975fb9b0/apps/android/native/app/build.gradle.kts) — 已提交源码 (bf12d8217b18d91a37ae51ed90a9be0b975fb9b0)
- [lingxi-app/apps/android/ffi/uniffi.toml](https://github.com/lingxi-coder/lingxi-app/blob/bf12d8217b18d91a37ae51ed90a9be0b975fb9b0/apps/android/ffi/uniffi.toml) — 已提交源码 (bf12d8217b18d91a37ae51ed90a9be0b975fb9b0)
- [lingxi-app/apps/ios/ffi/uniffi.toml](https://github.com/lingxi-coder/lingxi-app/blob/bf12d8217b18d91a37ae51ed90a9be0b975fb9b0/apps/ios/ffi/uniffi.toml) — 已提交源码 (bf12d8217b18d91a37ae51ed90a9be0b975fb9b0)
- [lingxi-app/apps/electron/scripts/package-macos-flare.sh](https://github.com/lingxi-coder/lingxi-app/blob/bf12d8217b18d91a37ae51ed90a9be0b975fb9b0/apps/electron/scripts/package-macos-flare.sh) — 已提交源码 (bf12d8217b18d91a37ae51ed90a9be0b975fb9b0)
- [mobile-linux-runtime/scripts/checks/check-resource-contracts.sh](https://github.com/lingxi-coder/mobile-linux-runtime/blob/6b033d41a2b6b736fda1e8f996f55a4b20bd885d/scripts/checks/check-resource-contracts.sh) — 已提交源码 (6b033d41a2b6b736fda1e8f996f55a4b20bd885d)

- lingxi-app: HEAD bf12d8217b18d91a37ae51ed90a9be0b975fb9b0; 所据文件指纹 sha256:be15e9524b52d312f415a56fb611a59c4c0e73f5a858b8c268772f4532863b2d
- mobile-linux-runtime: HEAD 6b033d41a2b6b736fda1e8f996f55a4b20bd885d; 所据文件指纹 sha256:435a888b1a310f357f18311df5f96418fa72b1a900d78e987af282bb3e91d7ca; 产品固定版本 6b033d41a2b6b736fda1e8f996f55a4b20bd885d

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
