# Mobile Linux 默认用户验收（2026-10-16）

按照确认方案，Android PRoot 与 iOS iSH 均使用 `lingxi`（UID/GID 1000），
`HOME=/home/lingxi`，默认工作目录 `~/lingxi`。
项目映射为 `/home/lingxi/lingxi/<id>`；宿主项目存储位置不变。
镜像提供免密码 `sudo`（doas shim），管理员 home 为 `/root`。
用户 home 与管理员 home 持久化，已有用户文件以不覆盖方式迁移。

## 来源与产物

- 产品 SDK 固定提交：`6b033d41a2b6b736fda1e8f996f55a4b20bd885d`。
- 产品 Harness 固定提交：`a9002383d91d743da8bc3f42f3459d005fe615b5`。
- Cargo 来源检查：两者均为唯一 canonical Git 来源，无 path override。
- Android rootfs：`3.24.2-user1`，arm64 与 x86_64 均完成归档、清单、APK closure 校验。
- ARM 归档 SHA-256：`043363aa7a21ff3b3095652de2849d9d6a3a2546fc8012c082383dfed32a37a8`。
- x86_64 归档 SHA-256：`d1aeaf2611692032f688c6eee5484234adca332033b0466b4771e7273fc93685`。
- SDK 与默认用户 rootfs 通过 `v0.1.0-rc.3` 发布；固定依赖提交已交付 main。
  产品下载入口与清机构建说明见 [从发布构建](build-from-release.md)。

## 已通过的验证

- SDK Rust workspace：135 项通过；另补跑需真实归档的解包、完整性与激活测试通过。
- SDK Python：59 项；iOS NativeSupport：26 项。
- Android 安装器：真实 doas 4755 与私有配置 600 的模式保留测试通过。
- Android 真机（Samsung SM_S9310，arm64，API 36）：11 类 SDK smoke 检查通过，
  包含默认身份、HOME/PWD、免密提权、raw stdio、PTY、后台任务、网络策略及重启持久化。
- iPhone 11（iOS 18.6.2）：默认身份、HOME/PWD、sudo、项目挂载、raw stdio、
  后台命令与登录 PTY 的 SDK 默认用户测试通过。
  该补充测试还覆盖了旧辅助进程退出事件误关闭新 PTY 的修复。
- Harness：平台回归、9 项移动环境上下文和 6 项工具可用性/工作目录测试通过。
- 产品 FFI：Android 11 项、iOS 24 项通过。
- Android 产品：964 项 DirectDebug JVM 测试通过。
- 产品 SDK 接入脚本：22 项通过。
- iOS 产品模拟器专项：41 项通过，覆盖终端、项目目录与 runtime 清单。

## 完整产品补充验证

- 修复 iOS 两处小栈崩溃：引擎构造在专用 8 MiB Rust 线程完成；异步 submit 调度在引擎拥有的 Tokio runtime 执行，并保留取消语义。
- 两个 512 KiB 调用线程回归用例均先复现栈溢出，再验证修复通过；Harness mobile host 231 项通过。
- 修复过期的 Swift 测试 DTO、认证预期与 LocalAppHost 测试替身。
- 原始完整 iOS 工程、真实启动路径：814 项测试，0 失败、1 项真机专用测试跳过。未设置 `LINGXI_UI_TESTING`。
- Android 完整产品：Samsung SM_S9310（arm64，API 36），包名 `com.lingxi.code.direct.debug`。
  - 最终 APK、两种 ABI JNI 与同源生成的 Kotlin 绑定构建成功；APK runtime closure 校验通过。
  - 默认用户/runtime、真实引擎回调、设置持久化、管理接口共 5 项真机测试通过。
  - 强制结束产品进程后，独立冷启动验收 1 项通过，`PRODUCT_COLD_START_ACCEPTANCE PASS android`。
  - 公开终端深链接走真实 MainActivity、TerminalViewModel、PTY、渲染视图，UI 验收 1 项通过。
  - 实际终端截图显示 `PRODUCT_UI:1000:/home/lingxi:/home/lingxi/lingxi`，连接状态为 connected。
  - Android JVM 回归 964 项，0 失败、0 跳过。
- iOS 完整产品：iPhone 11（iOS 18.6.2），包名 `com.lingxi.code`。
  - 原始完整工程及所有测试 target 真机构建、签名成功，使用最终 Harness 固定提交。
  - 真实引擎回调、默认用户/runtime、设置持久化、管理接口共 10 项真机测试通过。
  - 终止已有产品进程后，通过产品公开 `lingxi://open_terminal` 路由再次启动应用。
  - 手机镜像实际终端显示 `PRODUCT_UI:1000:/home/lingxi:/home/lingxi/lingxi` 和 `PRODUCT_COLD_OK`。
    后者读取并校验前一次真机 XCTest 写入的 home 文件，证明跨完整应用进程冷启动保留。
  - 未设置 mock 或 `LINGXI_UI_TESTING`，运行真实产品 FFI、打包 rootfs 与 NativeSupport。

## 验收边界

此前 SDK 验收应用的结果与完整产品真机结果分开记录。新产品真机测试覆盖真实打包资源、FFI、默认身份、sudo、项目目录读写、后台命令、PTY 和 home 持久化；另以进程重启验证持久化。
iOS XCUITest 启动器在用例执行前与 Xcode 握手失败（DTX / runner code 74），两版本 Xcode 交叉检查仍复现；该自动 UI 用例没有通过。实际产品冷启动与终端界面改由公开深链接加 iPhone Mirroring 检查完成，身份与跨进程持久化输出均已目视确认。镜像鼠标注入也报 noWindowsAvailable，因此不声称通过侧栏逐步点击或键盘输入验收。

真实引擎回调测试使用空凭据，验证本地命令、目录、配置与终止事件；本记录不声称在线 LLM 生成或所有产品功能已全面回归。
此前本地产物用于开发与真机验收；随后以同一 SDK 源码构建 Release SDK，连同默认用户 rootfs 发布为 RC3。源代码通过 main 交付。iOS 本地 FFI framework 包含 device arm64 与 simulator arm64，NativeSupport 保留完整平台切片；未生成新的 Intel simulator FFI 发行切片。

## 本地证据

- iOS 全量：`/tmp/lingxi-ios-full-unit-tests-final2.log`（814 / 0 失败 / 1 跳过）。
- iOS 真机：`/tmp/lingxi-ios-product-device-tests.log`（10 / 0 失败）。
- iOS 真机构建：`/tmp/lingxi-ios-product-device-build.log`。
- iOS UI 自动化阻断：`/tmp/lingxi-ios-product-device-ui2.log`，`/tmp/lingxi-ios-product-device-ui-xcode26b.log`。
- Android 构建与 JVM：`/tmp/lingxi-product-android-final-gradle.log`。
- Android 真机：`/tmp/lingxi-android-product-device-tests-final.log`（5 / 0 失败）。
- Android 冷启动：`/tmp/lingxi-android-product-cold-start.log`（1 / 0 失败）。
- Android 终端界面：`/tmp/lingxi-android-product-ui-test-final.log`（1 / 0 失败）。
- Android 截图：`/tmp/lingxi-default-user/android-product-terminal.png`。
- Harness 栈修复回归：`/tmp/lingxi-both-small-stack-after.log`、`/tmp/lingxi-mobile-dispatch-final-regression.log`。


## Git 交付范围

- SDK：默认用户实现 `6b033d41`，另提交真实归档验收的清单数量修正；该补充只改测试，产品继续固定已验证的实现 SHA。
- Harness：默认用户接入 `8bf50f1a`、异步调度小栈修复 `13f583b7`、构造小栈修复 `a9002383`；合并最新远端 main，产品继续固定已完成真机验证的 `a9002383`。
- 产品：仅提交默认用户路径、SDK/Harness 固定依赖、iOS 崩溃相关测试修复与真机验收；排除当前工作区其他 Web/Firebase/infra 改动。
- 隔离交付检查：SDK 真实归档校验 6 项通过；产品 SDK 接入/下载脚本 25 项通过。此前完整 App 构建与真机证据见上文。
