# Harness 独立仓迁移

运行时位于独立仓库 `lingxi-coder/harness-runtime`。LingXi 的 Rust workspace 保留产品入口，通过根 `workspace.dependencies` 和 `Cargo.lock` 固定上游提交；所有共享类型来自同一 Cargo package identity。

## 归属与接口

- 从 `01dcc428ba0c12522f6ee29b6474f3a4e0891c47` 提取 78 个 workspace package，包含运行时依赖闭包、`mock_stdio_mcp` 与 `apply-seccomp`；另迁移四个 vendored Git/TLS Cargo package 和对应原生源码。
- LingXi 保留 `cli`、`bridge-server`、`ios-framework`、`android-aar`、`tui`、`tui-core`、`config-requirements`、`platform-android-shellbin`、`platform-ios-ish-runtime`、`tool-ios-use`。mksh/toybox、应用资源、原生回调和发行包装仍属于宿主。
- 上游根目录为 Cargo workspace，源码和内部插件位于 `crates/`，资源契约、门禁及第三方源码有独立归属。来源路径、原始 Git blob 与 SHA-256 记录在上游 `docs/migration/source-manifest.json`。
- `api`、`models`、`desktop`、`mobile`、既有 feature 和线协议保持不变。宿主通过 Rust `BuildInfo` 注入自己的版本及提交号，`runtime_build_info()` 单独表示运行时身份。
- `llm-client` 继续固定在 `9b0323f10f76c5834acafedaa04470c4e96346f2`。迁移没有升级 registry dependency；保留了所有宿主依赖的 feature、optional 和 default-features 语义。

当前宿主固定运行时代码提交 `36dd3c1295ae13bde59f6131705dbf86a244ae52`。上游 PR 为 https://github.com/lingxi-coder/harness-runtime/pull/1，后续 `0fe594a610bc46e7db11a19bdcaa9f39bee61680` 与 `36436caf643efcaa5b3fd58b81f37ac30beb86ba` 只调整 CI 和验证脚本，没有遗漏未被宿主引用的运行时修复。

## 使用固定来源

在 `lingxi-code/` 执行：

```sh
python3 scripts/runtime_source.py --json
python3 scripts/check-runtime-dependency.py
```

解析器使用 `cargo metadata --locked --all-features`，校验统一 Git URL、完整 SHA、包身份和源码包含关系。它不扫描缓存猜测版本，也不回退到相邻本地 checkout。当前宿主依赖图含 79 个来自上游的运行时与 vendor package；其余迁出包属于独立测试或辅助程序。

宿主测试中确需编译期嵌入的三个 oracle 资源以 `scripts/runtime-fixture-mirrors.json` 声明，并由协议门禁逐字节对照锁定上游。Swift/Kotlin/Electron 的其他协议 fixture 通过固定来源解析或构建时 staging 获取。

## 构建与资源

运行时内置插件、模板、技能镜像、预算、pins、SBOM 与通用供应链门禁属于上游。宿主的 Android launcher、iSH patch、原生源码 pin、Swift 注入和资源安装验证继续在 LingXi 运行。

资源脚本只读使用 Cargo Git checkout；产物与缓存使用显式宿主目录。原生生成绑定必须与对应新库一起使用。现有客户端 Generated 中的旧 `engine_mobile` 文件不能与新 `harness_runtime` 库混用。

macOS 包装继续使用 `npm run package:mac:flare`，保留路径重映射、由内至外签名和 `verify:package`。

## 迁移验证记录

已完成的验证包括：

- 上游所有 workspace target 与 feature 编译；minimal、core、desktop、mobile、mobile+uniffi、android-computer-use 六种独立配置编译。
- 2,110 个插件、模板、技能及第三方资源文件与来源逐字节一致。补入十个生产构建需要、但原仓忽略的策略文件，内容不变。
- 上游完整测试运行报告 16,870 passed、27 failed、8 ignored；另有 agent 测试进程栈溢出，未产生完整结果。13 个失败 target 中，12 个在原始提交的隔离基线中复现。唯一确认的迁移回归是 `DesktopConfig` 文档示例缺少新字段，已修复并单独通过 doctest。
- 单独 Harness 测试中的暂停用例曾出现时序失败；基线和迁移后各独立运行五次均通过，后续 workspace 运行也通过。没有将其标记为已修复。
- 依赖身份七个正反例、依赖方向四个反例、品牌扫描器反例、资源 inventory、供应链及 rootfs 工具测试通过。
- Electron 相关测试 54 项、Android JVM 相关测试 6 项、Swift 协议样例 505 项通过。删除旧源码后的宿主完整 Rust workspace 测试为 2,780 passed、0 failed、4 ignored。

- 宿主在禁止访问本地 harness-runtime checkout、禁止写入固定 Cargo Git checkout 的沙箱中通过 all-targets/all-features 编译及依赖、客户端协议门禁。
- 从真实 SSH 远端重新克隆上游，在系统沙箱禁止读取 LingXi 和本地 harness-runtime checkout 的条件下，全 workspace、all-targets、all-features 编译通过；七项会话恢复、取消及权限测试、依赖和资源门禁通过。负向读取探针确认两个原目录确实不可访问。
- 真实 GitHub Actions 验证六组运行时 feature、四组 Android ABI/feature、Linux/macOS/Windows 核心包、Linux/macOS desktop 和 iOS device/simulator 构建通过。Windows desktop 的 POSIX 编译错误在原始源码中已存在，保留原平台能力差异。
- Android play/direct JNI 两种 ABI、mksh/toybox 与 Kotlin 绑定生成通过。12 个 ELF 架构、296 个 Kotlin FFI 声明与四个 JNI 库导出均匹配；签名 macOS 包装结果见下方。
- iOS 完整 XCFramework 流程已使用 Rust 1.94 通过：设备 arm64、模拟器 arm64/x86_64 三个切片成功合并，必需的 Node/rootfs/iSH 源码构建、宿主辅助程序及根文件系统逐项验证全部通过。最终 Swift SDK 类型检查无诊断，四组公开绑定归一化对照、宿主 FFI 初始化和模拟器静态链接通过。验证产物位于 `/tmp/harness-ios-validation/Frameworks/LingxiCodeFFI.xcframework`；根文件系统 ZIP SHA-256 为 `c7c4f3f7fcc8b7f646e19856d4ebee4b4e89fb4d8561ae2d029092445dbb5b54`。构建 VM 正常停止，iSH 构建期间补丁已恢复。未声称模拟器或真机应用运行验收。

- [真实 Linux helper CI](https://github.com/lingxi-coder/harness-runtime/actions/runs/36291747832/job/108543030207) 构建并执行 `apply-seccomp`，验证自定义 target 定位、Unix socket/socketpair 拦截、IPv4 socket 放行、NO_NEW_PRIVS 和子进程退出码。
- 宿主验证构建提交 `0102773438de56fad21d1ce7d88fab18e96f82d2` 通过 `npm run package:mac:flare -- --launch`。运行时本地 checkout 在整个包装期间临时移开，包装后恢复。Flare 团队 `AZ4AX7J833` 签名、路径重映射、静态校验及真实应用冒烟全部通过，应用重新启动。固定 Git checkout 的 4,684 个文件在构建前后 SHA-256 全部一致。ZIP 摘要为 `63893a8dfd5934d66150a913bef5a5481a5b1c9ff9a286ac89d1933d101fdd52`。

全量门禁仍保留已复现的原有失败：上游品牌扫描 82 个历史命中及 114 条失效基线，技能描述长度超限；宿主品牌扫描 6 个历史命中及 2 条失效基线，12 个缺失翻译键、6 个过期生成文件和 2 个孤立 XML。没有通过刷新全部基线、删除门禁或修改 oracle 内容掩盖它们。

Android 完整 APK 包装未通过原生资源预检：两种 ABI 均缺 `libproot.so`、`libproot-loader.so`、`libmobile_linux_policy_launcher.so` 和 `libpty_bridge.so`。现有 iOS 脚本的递归初始化已使 PRoot 处于要求的 `8cf13e997cdc9472997aae19df8050c073c9a86c`，但两种变体在隔离宿主 worktree 中补建缺失库时，都被原有 iSH 桥接头文件/实现文件摘要门禁阻塞。原始 `01dcc428` 的 pins 与当前 pins 完全相同；实际文件与原始 OpenMinis gitlink `9cf3a855fecd27bb5735b84cacbd56852a3ab8dd` 的 blob 逐字节相同，证明此不一致早于迁移。没有刷新 pins 或输出缺失资源的 APK。iOS/Android 真机启动、恢复、权限、取消和关闭冒烟尚未执行。

Linux CI 完整测试报告 17 个失败 target；已完成的测试汇总为 16,845 passed、48 failed、8 ignored，另有 agent 栈溢出中止。不能把本地 12 个基线复现结论扩大为远端全部失败均已复现；额外的 workflow 时序、缺少 bwrap/socat 等问题保留在 [CI 验证记录](harness-runtime-ci-validation.json)。

CI 还报告未改变依赖锁定所带来的 cargo-deny 许可证/公告失败、原有 Clippy 和 Windows ConPTY 失败。Linux 提示词快照失败已在原始提交与迁移后分别复现：测试匹配 `OS version:`，而生产文本为 `OS Version:`，导致内核版本未被归一化；没有修改提示词字节或刷新快照。

并发的桌面包装任务曾为旧源目录补充 BuildInfo。迁移前对照其修改后，确认其版本注入及 Debug 展示能力已由上游实现承接，再移除旧副本；没有丢弃该任务的功能修复。

## 回退与维护

上游 Git 提交和宿主依赖切换分别记录。资源准备提交 `e78020129f4d383538fbe8de5725e4fbe62d9b64` 将原先被忽略的十个必需策略文件逐字节纳入 Git，并只登记它们四十项已有品牌清单键。随后的一次完整迁移提交包含依赖切换、源码移除及原生构建修复。已在隔离工作树中实际执行回退，核对恢复后的完整 Git tree 与资源准备提交完全一致、十个文件摘要全部匹配；回退不需要迁移用户数据。原有 329 条未提交删除路径全部排除在本次暂存范围之外。并发期间 OpenMinis gitlink 对应目录恢复为原始提交，其余 328 条删除保持未暂存；未擅自删除来源不明的恢复目录。

原生构建修复还包括：仅在构建 macOS rootfs 辅助程序时移除 iOS 部署目标变量；让 iOS/Android 的所有 Cargo/bindgen 调用继承工作区解析出的 Rust 工具链，消除调用目录导致的全局工具链误选。Android 已用 Rust 1.94 重新生成 Kotlin，与两个既有验证变体逐字节一致。较早的 iOS 全局工具链构建不作为最终 Rust 1.94 验收证据。

早期构建引用 `0102773`、`e86b37e`、`b4bef864` 保留在 `codex/extract-harness-runtime-validated-builds`，用于核验实际产物来源。最终原生代码验证使用 `e672b8a6d0bfad00d057af57c2228c375ce3233e`，该代码验证提交保留在 `codex/extract-harness-runtime-native-validation`，交付提交仅在相同代码上补齐验证文档。

GitHub 推送按仓库账号使用 SSH Host 别名；此仓库使用 `github.com-lingxi-coder`，IdentityFile 以当前 `~/.ssh/config` 为准。公共 Cargo 依赖保留可供干净 CI 环境读取的标准 URL；本机 Git 将 `https://github.com/lingxi-coder/` 精确按账号路由到 `git@github.com-lingxi-coder:lingxi-coder/`，Cargo 使用 Git CLI。已从空 Cargo Git 缓存实际拉取固定提交，trace 确认走该 SSH 别名；此规则不改变其他 GitHub 账号的路由。
