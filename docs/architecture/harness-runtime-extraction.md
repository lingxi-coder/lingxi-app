# Harness 独立仓迁移

运行时位于独立仓库 `lingxi-coder/harness-runtime`。LingXi 的 Rust workspace 保留产品入口，通过根 `workspace.dependencies` 和 `Cargo.lock` 固定上游提交；所有共享类型来自同一 Cargo package identity。

## 归属与接口

- 从 `01dcc428ba0c12522f6ee29b6474f3a4e0891c47` 提取 78 个 workspace package，包含运行时依赖闭包、`mock_stdio_mcp` 与 `apply-seccomp`；另迁移四个 vendored Git/TLS Cargo package 和对应原生源码。
- LingXi 保留 `cli`、`bridge-server`、`ios-framework`、`android-aar`、`tui`、`tui-core`、`config-requirements`、`platform-android-shellbin`、`platform-ios-ish-runtime`、`tool-ios-use`。mksh/toybox、应用资源、原生回调和发行包装仍属于宿主。
- 上游根目录为 Cargo workspace，源码和内部插件位于 `crates/`，资源契约、门禁及第三方源码有独立归属。来源路径、原始 Git blob 与 SHA-256 记录在上游 `docs/migration/source-manifest.json`。
- `api`、`models`、`desktop`、`mobile`、既有 feature 和线协议保持不变。宿主通过 Rust `BuildInfo` 注入自己的版本及提交号，`runtime_build_info()` 单独表示运行时身份。
- `llm-client` 继续固定在 `9b0323f10f76c5834acafedaa04470c4e96346f2`。迁移没有升级 registry dependency；保留了所有宿主依赖的 feature、optional 和 default-features 语义。

当前宿主固定运行时代码提交 `36dd3c1295ae13bde59f6131705dbf86a244ae52`。上游 PR 为 https://github.com/lingxi-coder/harness-runtime/pull/1，后续 `0fe594a610bc46e7db11a19bdcaa9f39bee61680` 只调整 CI，没有遗漏未被宿主引用的运行时修复。

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
- 从真实 SSH 远端重新克隆上游，在系统沙箱禁止读取 LingXi 和本地 harness-runtime checkout 的条件下，全 workspace、all-targets、all-features 编译通过。
- 真实 GitHub Actions 验证六组运行时 feature、四组 Android ABI/feature、Linux/macOS/Windows 核心包、Linux/macOS desktop 和 iOS device/simulator 构建通过。Windows desktop 的 POSIX 编译错误在原始源码中已存在，保留原平台能力差异。
- Android play/direct JNI 两种 ABI、mksh/toybox 与 Kotlin 绑定生成通过。12 个 ELF 架构、296 个 Kotlin FFI 声明与四个 JNI 库导出均匹配；签名 macOS 包装仍在最终验证。
- iOS arm64 simulator XCFramework、Swift SDK 类型检查、生成接口对照、FFI 初始化和模拟器静态库链接通过。本机完整设备包装因已有 Podman VM 无法启动而阻塞；远端设备编译通过不等同于真机验收。

全量门禁仍保留已复现的原有失败：上游品牌扫描 82 个历史命中及 114 条失效基线，技能描述长度超限；宿主品牌扫描 6 个历史命中及 2 条失效基线，12 个缺失翻译键、6 个过期生成文件和 2 个孤立 XML。没有通过刷新全部基线、删除门禁或修改 oracle 内容掩盖它们。

Android 完整原生供应链验收还受原有 PRoot checkout/pin 不匹配影响；没有修改该宿主第三方树来掩盖差异。iOS/Android 真机启动、恢复、权限、取消和关闭冒烟尚未执行。

并发的桌面包装任务曾为旧源目录补充 BuildInfo。迁移前对照其修改后，确认其版本注入及 Debug 展示能力已由上游实现承接，再移除旧副本；没有丢弃该任务的功能修复。

## 回退与维护

上游 Git 提交和宿主依赖切换分别记录。回退宿主迁移提交即可恢复原有依赖布局，不需要迁移用户数据。原有 329 条未提交删除路径全部排除在本次暂存范围之外。并发期间 OpenMinis gitlink 对应目录恢复为原始提交，其余 328 条删除保持未暂存；未擅自删除来源不明的恢复目录。

GitHub 推送按仓库账号使用 SSH Host 别名；此仓库使用 `github.com-lingxi-coder`，IdentityFile 以当前 `~/.ssh/config` 为准。公共 Cargo 依赖使用可供干净构建环境读取的固定 Git 来源。
