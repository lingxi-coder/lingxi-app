# Harness 结构重构验证记录

验证日期：2026-09-26。范围是迁移现有实现、调整依赖和宿主接线，不替换业务行为或存储。工作区在本次重构前已有其他未提交改动；下述基线是开始重构时保存的源码，不是 Git HEAD。

## 编译与依赖

在 `lingxi-code` 下使用仓库固定的 Rust 1.94：

```sh
cargo check --offline --locked --workspace --all-targets \
  --features harness-runtime/desktop,harness-runtime/mobile,harness-runtime/uniffi
cargo check --offline --locked -p harness-runtime
cargo check --offline --locked -p harness-runtime --no-default-features --features mobile
```

三项均通过，编译错误为零。工作区仍有既有 `missing_docs`、条件编译下的未使用代码等警告，不能表述为整个项目零警告。当前工具环境没有 `lsp_diagnostics`；这里记录的是 Rust 编译器检查结果。

默认核心配置的依赖树不包含 `tui`、`tui-core`、`client-adapter`、`client-protocol` 或 `uniffi`。移动配置不启用 UniFFI 也可编译并运行原有宿主测试。依赖门禁覆盖 89 个工作区 crate，通过。

原生目标检查均通过：

```sh
IPHONEOS_DEPLOYMENT_TARGET=17.0 CARGO_PROFILE_DEV_DEBUG=0 \
  cargo check --offline --locked -p ios-framework --target aarch64-apple-ios

# 使用本机 Android NDK 29.0.14206865，设置 ANDROID_NDK_HOME。
CARGO_PROFILE_DEV_DEBUG=0 cargo ndk -t arm64-v8a check --offline --locked -p android-aar
CARGO_PROFILE_DEV_DEBUG=0 cargo ndk -t arm64-v8a check --offline --locked \
  -p android-aar --features android-computer-use
```

这些结果证明目标代码可编译，不代表真机运行验收。

## 已运行的测试

以下统计按测试目标计数，不重复计算集成测试启动的子进程或单项重跑。

| 测试范围 | 通过 | 失败 | 忽略 |
|---|---:|---:|---:|
| `harness-api` 原有交互契约 | 7 | 0 | 0 |
| `model-runtime` 决策契约 | 6 | 0 | 0 |
| 桌面装配单元测试 | 494 | 3 | 0 |
| 桌面装配集成测试 | 25 | 2 | 0 |
| 移动装配单元测试，无 UniFFI | 821 | 7 | 1 |
| 移动装配集成测试，无 UniFFI | 49 | 1 | 0 |
| Android AAR 原有宿主测试 | 12 | 0 | 0 |
| iOS Framework 原有宿主测试 | 17 | 0 | 0 |
| 合计 | 1431 | 13 | 1 |

主要复现命令：

```sh
CARGO_PROFILE_TEST_DEBUG=0 cargo test --offline --locked -p harness-api -p model-runtime --lib --tests
CARGO_PROFILE_TEST_DEBUG=0 cargo test --offline --locked -p harness-runtime \
  --no-default-features --features desktop --lib --tests --no-fail-fast
CARGO_PROFILE_TEST_DEBUG=0 cargo test --offline --locked -p harness-runtime \
  --no-default-features --features mobile --lib --tests --no-fail-fast
CARGO_PROFILE_TEST_DEBUG=0 cargo test --offline --locked -p android-aar -p ios-framework --lib
```

需要允许测试创建本地 Unix socket。最初受沙箱限制的两项桌面 socket 测试，在具有本地 socket 权限的环境中通过。

## 失败项与原源码对照

13 项失败均用重构开始时保存的原始桌面或移动装配源码重新运行，并复现相同断言或超时。临时基线只调整依赖位置和已迁出的契约导入；没有改动测试条件或业务实现。

| 原有失败 | 原源码复现结果 |
|---|---|
| 桌面 `build_persists_and_restores_agent_setting_on_resume` | 会话仍被活动 writer 占用 |
| 桌面 `build_resume_missing_catalog_agent_uses_persisted_snapshot` | 同上 |
| 桌面 `build_resume_restores_agent_frontmatter_permission_mode` | 同上 |
| 桌面音频注入的 recognizer-only、recorder-only 两项 | 单项音频依赖的测试断言与现有统一音频工具注册不一致 |
| 移动默认模型：`double_qualified_ref_falls_back_to_the_anthropic_boot_default` | 保留的模型引用与测试预期不一致 |
| 移动默认模型：`fallback_is_taken_from_the_listings_when_anthropic_is_not_registered` | 同上 |
| 移动默认模型：`unknown_qualified_ref_falls_back` | 同上 |
| 移动 `mobile_provider_catalog_matches_engine_presets_without_secrets` | 模型目录排序预期不一致 |
| 移动 `model_preference_disabled_providers_never_restore_or_accept_selection` | 已禁用提供商的选择预期不一致 |
| 移动 `model_preference_unavailable_saved_provider_falls_back_to_config_without_overwrite` | 不可用提供商的回退预期不一致 |
| 移动 `mobile_human_message_reconstructs_stopped_agent_with_original_identity_and_history` | 原源码和迁移后源码单独重跑均超时 |
| 移动工具集合快照 | 原测试环境注册的工具不含预期的 `voice`，在快照比较前失败 |

本次没有调整这些业务逻辑，也没有修改失败断言使其通过。测试不是全绿；上述对照用于区分已有失败与本次迁移回归，不构成对既有行为正确性的背书。

迁移中发现并修复的实际问题：移动测试的插件根目录、组件扫描 allowlist 路径，以及两个集成测试错误依赖 UniFFI 的入口开关。修复后技能加载 9 项、组件扫描 38 项、移动持久化 skeleton 2 项通过，全量移动测试已复跑。

## 资源与源码保留

- 桌面和移动源文件、测试及资源共 84 个文件迁入共享 crate。
- 原装配源文件的函数声明数量保持一致：桌面 1703、移动 2784，包含原有测试函数。此项是迁移完整性辅助检查，不替代编译和行为测试。
- 对 67 个迁移的 Rust 源文件进行命名空间、feature 和格式归一对照后，剩余差异仅涉及资源相对路径、UniFFI 根 scaffolding 的移动、音频导出的条件属性和测试代码换行；未发现额外业务函数体改动。
- 原工具集合快照和资源 fixture 内容保留；没有更新快照来隐藏差异。
- Phase 2、Phase 7 插件门禁通过：27 Skills、5 Agents、2 Workflows、5 Schemas、112 profile assets、6 MCP widget assets，打包 inventory 完整匹配。
- Local Apps 供应链校验通过；修改脚本的 Python/Bash 语法检查通过。
- 新共享 crate 的格式检查及 `git diff --check` 通过。

## 原生绑定与交付边界

Swift 绑定由新构建的 `ios-framework` 动态库生成，使用仓库原有脚本中的 modulemap、重复 scaffolding 清理和回调初始化步骤处理。全部生成 Swift 与应用中的 `EngineModule.swift` 一起通过类型检查，零错误、零警告；链接并执行探针返回 `Harness UniFFI contract version: 26`。

Kotlin 绑定由新构建的 `android-aar` 宿主动态库生成，沿用项目自己的 UniFFI 合并生成器和配置。生成文件保持 `com.lingxi.code.bindings` 包名；内部共享 FFI 命名空间为 `harness_runtime`。使用项目对应的 Kotlin 2.2.20、JNA 5.19.0 和 coroutines 1.11.0 编译生成文件通过，零错误、零警告。

绑定验证产物位于临时目录，不覆盖移动客户端已有的 Generated/Frameworks 或发布包；重新构建移动客户端时仍需执行其正常包装脚本。

未完成的验收项：Android/iOS 真机运行、前后台切换、真实凭据与 shell 进程回收、完整 AAR/XCFramework 发布产物验证，以及性能基线。编译、宿主测试和绑定探针不能替代这些检查。

## macOS 签名包装

已按仓库要求从 `clients/electron` 执行：

```sh
npm run package:mac:flare -- --check
npm run package:mac:flare
```

Flare 团队 `AZ4AX7J833` 的 development 签名预检通过。包装完成了带路径重映射的 Rust release 构建、Electron 构建、Audio Helper / Credential Broker / 主应用由内至外签名，以及 `verify:package:static`。ZIP 为 185431921 字节，SHA-256 为 `311496fcf3f57ebd368b90054c581037170eef56a89ffbf7f63a6e06a738f427`。

包装命令最终退出码为 1：`verify:package:smoke` 检测到已有 LingXi 实例运行，按其保护条件拒绝启动第二个实例。本次未自行关闭当前应用，因此尚不能将 macOS 包标记为完成全部包装验收。完成运行验证仍需退出当前实例后执行 `npm run verify:package`。

产物位置：`clients/electron/dist/LingXi-Code-0.1.0-mac-arm64/LingXi Code.app`；日志：`/tmp/lingxi-harness-mac-package.log`。

## 本机日志

构建与测试日志位于 `/tmp/lingxi-harness-*.log`；绑定生成、类型检查和链接探针位于 `/tmp/lingxi-harness-bindings/`。原源码对照日志以 `lingxi-harness-baseline-` 开头。这些是本次会话的临时证据，不是仓库依赖或长期发布记录。


## Review 修复验证（2026-09-26）

- 凭据刷新改为 `SharedCredentialStack` 显式拥有的 `FusionCatalogRegistry`，覆盖 CLI、Bridge、登录和退出路径，移除进程级刷新列表。
- 提供 `HarnessBuilder`、`Harness`、`SessionHandle` 及注入式会话/关闭服务。`desktop::build_harness` 复用原装配和权限接口，保持 Agent、存储和关闭协调实现；客户端产品接入继续使用原适配层。
- 将共享显示派生、差异和样式实现迁到 `client-presentation`，终端 I/O 与自动主题检测保留在 `tui-core`。移动 normal 依赖树不含 `tui`、`tui-core`、`crossterm`；依赖门验证传递路径，注入反向依赖的负向检查被拒绝。
- `cargo check --offline --locked -p harness-runtime --features desktop,mobile,uniffi -p cli -p bridge-server --all-targets` 通过；默认 core 独立检查通过。没有新增编译错误，应用测试 fixture 的既有 missing-docs 警告仍存在。
- client-adapter 111、client-presentation 226、tool-ui 140、tui-core 154，共 631 项测试通过。工具套件首次被沙箱禁止 Unix socket 绑定，使用允许本地 socket 的环境重跑全部通过。
- Harness 凭据相关 6 项、作用域通知及退出回调 2 项、两个嵌入实例的会话身份和关闭测试 1 项通过；本次定向验证合计 640 项。未重新执行全部项目测试或移动真机验收。
- 格式、依赖门和 `git diff --check` 通过。当前没有可用的 `lsp_diagnostics`，类型验证使用 Rust 编译器完成。
- 品牌扫描未通过：工作区其他路径有 86 项新命中、213 项失效基线，主要涉及既有 Local Apps 模板和测试文件。此次只同步 classify 源码迁移对应的三个基线路径；没有扩大白名单或覆盖其他基线问题。

详细日志：`/tmp/lingxi-fix-check-final.log`、`/tmp/lingxi-sdk-core-check.log`、`/tmp/lingxi-presentation-tests-native.log`、`/tmp/lingxi-credential-scope-tests.log`、`/tmp/lingxi-credential-callback-tests.log`、`/tmp/lingxi-sdk-embedding-tests.log`、`/tmp/lingxi-fix-brand-check.log`。


## API 归属收敛验证（2026-09-26）

本轮删除独立 `harness-api` crate。SDK 公共入口从 `harness-runtime::sdk` 收到 `harness-runtime::api`；提问与交互命令契约迁入 `tool-api`，设备访问授权契约迁入 `permission::computer_access`。此前记录中的 `harness-api` 测试结果属于迁移前阶段，当前以本节命令和路径为准。

- 迁移前后源码对照：三组交互契约和 SDK 服务入口，去除注释与空白后代码一致。消费者只调整导入、依赖声明及格式；不提供旧 crate 或旧模块路径的兼容转发。
- Workspace/lockfile 不再含 `harness-api`。89 个 crate 的依赖门通过，负向检查能拒绝 `tool-api -> harness-runtime`。core normal 依赖不含客户端协议、适配器、UniFFI 或 TUI；mobile normal 依赖不含终端实现。
- 全工作区 `cargo check --offline --locked --workspace --all-targets --features harness-runtime/desktop,harness-runtime/mobile,harness-runtime/uniffi` 通过；`cargo check --offline --locked -p harness-runtime --no-default-features --features core` 通过。原有 missing-docs 警告保留，类型检查为零错误。Bridge Server 的 `tool-api` 从测试依赖提升为生产依赖，以满足新的契约引用。
- 已有测试：提问契约 5、设备访问契约 2、客户端适配器 111、Decision 集成契约 6、嵌入式 SDK 1，共 125 项通过。没有新增测试实现或重新运行整套产品测试。
- 本轮修改的 30 个 Rust 文件 rustfmt 检查与 `git diff --check` 通过。当前无 `lsp_diagnostics` 工具，使用 Rust 编译器验证。
- `agent` 与 `llm-runtime` 保留不同职责；Decision 保持 `model-runtime::decision`，不新建空的 Runtime crate 或启用新模型流程。本轮未进行移动真机或 macOS 包装验证。

日志：`/tmp/lingxi-api-merged-workspace-final.log`、`/tmp/lingxi-api-merged-core.log`、`/tmp/lingxi-api-merged-tool_api-tests.log`、`/tmp/lingxi-api-merged-permission-tests.log`、`/tmp/lingxi-api-merged-client-adapter-tests.log`、`/tmp/lingxi-api-merged-decision-tests.log`、`/tmp/lingxi-api-merged-embedding-tests.log`。


## model-runtime 合并验证（2026-09-26）

独立 `model-runtime` crate 已删除。SDK 模型入口现在由 `harness_runtime::models` 提供，Decision 实现与原有六项契约测试迁入 Harness；执行组件直接依赖 `llm-runtime`。上文涉及旧 `model-runtime` 的结果与命令是迁移前记录。

- 169 个既有 Rust 消费文件完成源码对照：仅命名空间替换和 rustfmt 变化。Decision 实现逐字节不变，测试仅调整导入路径。LLM 执行、认证、重试和费用逻辑没有改写。
- 旧 package、工作区成员及依赖引用均已移除。88 个 crate 的依赖门通过；agent、orchestrator、compaction 直接依赖 llm-runtime，不反向依赖 Harness。SDK 保留 `models::llm` 对外入口。
- `cargo check --offline --locked --workspace --all-targets --features harness-runtime/desktop,harness-runtime/mobile,harness-runtime/uniffi` 通过，零编译错误；现有 missing-docs 警告保留。
- `cargo check --offline --locked -p harness-runtime --no-default-features` 通过，证明模型入口和 Decision 契约可在无桌面、移动及默认 core 功能的配置下编译。正常依赖树不含旧包装 crate、客户端协议、客户端适配器、UniFFI 或 TUI。
- `CARGO_PROFILE_TEST_DEBUG=0 cargo test --offline --locked -p harness-runtime --all-features --test decision_contract`：6 项通过，0 失败。
- 173 个修改或迁入的 Rust 文件格式检查与 `git diff --check` 通过。类型检查使用 Rust 编译器；没有可用的 `lsp_diagnostics` 工具。本轮未进行真机、生产模型或 macOS 包装验证。

日志：`/tmp/lingxi-model-merged-workspace.log`、`/tmp/lingxi-model-merged-minimal.log`、`/tmp/lingxi-model-merged-decision-tests.log`、`/tmp/lingxi-model-merged-minimal-tree.txt`。


## Review：取消入口修复（2026-09-26）

复查 SDK 入口、依赖/feature 组合、领域契约、凭据刷新与执行调用链后，确认并修复共享执行入口的两类取消问题：

1. **P1：等待会话锁时无法响应取消。** 批处理、流式及队列批量输入直接等待 `turn_gate`；先前轮次一直等待交互时，已取消的后续调用也一直挂起。三个回归用例在修复前均超时失败。现使用带取消的锁获取，只在进入执行前竞争取消信号；获得锁后的模型/工具执行与结果、用量保存沿用原流程。
2. **P2：预取消的图片请求仍读取附件并可能返回文件错误。** 回归用例在修复前返回缺失图片的 I/O 错误。现于图片读取前检查取消，返回 `Cancelled`。

验证结果：

- 新增回归 4 项通过，覆盖等待中取消、预取消及预取消图片。
- 既有 `streaming_cancel_test`、`turn_cancel_mapping_test`、`streaming_partial_finalize_test`、`turn_epilogue_boundary_test` 共 20 项通过，校验运行中取消、部分结果保存及结束流程。
- `cargo check --offline --locked -p harness-runtime --all-features` 通过；修改的执行代码与测试由编译器完成类型验证。既有 permission dead-code 警告保留。
- 依赖门、目标文件 rustfmt 与 `git diff --check` 通过。没有重新运行全量产品测试或真机/包装验收。

日志：`/tmp/lingxi-review-cancel-red.log`、`/tmp/lingxi-review-image-red.log`、`/tmp/lingxi-review-cancel-green.log`、`/tmp/lingxi-review-cancel-compat.log`、`/tmp/lingxi-review-runtime-check.log`。
