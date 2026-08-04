# iOS Linux 运行时设置页重构

日期：2026-08-04
状态：已批准，待实现

## 背景

真机安装 `com.lingxi.code.full` 后，用户在「设置 → Linux 运行时」页反馈两点：看不懂如何配置；点击行没有反应。

页面当前为 `clients/ios/Sources/Settings/LinuxRuntimePage.swift`，691 行，同时承载 FFI 桥接（`LinuxRuntimeBridge`）、状态映射与视图，平铺 8 个分区：后端选择、状态、维护、终端、任务、挂载骨架、工作区与终端、安全说明。

## 问题诊断

### 1. 点击无响应（真实缺陷，调用处）

`SettingsRow` 组件本身**支持** `onTap`：泛型（带 trailing 闭包）版本的结构体持有 `var onTap`，init 接收它，`body` 在 `onTap != nil` 时把 `content` 包进 `Button`。组件无缺陷。

缺陷在调用处：`LinuxRuntimePage` 的 `actionRow`（第 607 行）与 `actionButtonRow`（第 579 行）从不传 `onTap`，仅在 trailing 槽内放置一个 12pt 的 `Button(action.label)`。因此整行不可点，唯一触摸目标是右侧一小块文字。行的视觉语言与设置内其它可点条目一致，用户自然点击行主体，无反应。

修复只涉及该页调用处，不改共享组件。

### 2. 信息面向工程师

术语（rootfs、fakefs、ABI、PTY、挂载骨架、unavailable stub）未翻译，且页面不回答「我现在该做什么」。禁用态副标题仅显示「未启用」「当前构建未开放该操作」，未说明原因。

### 3. 数据源约束

`MobileLinuxStatusFfi` 仅暴露 `state / backend / mode / platform / abi / version / managed_root / active_root / staged_root / archive_sha256 / installed_size_bytes / writable_guest_paths / last_error`；`MobileLinuxCapabilityFfi` 仅暴露能力布尔位。**均不含已安装软件包版本**，因此 node/git/python 版本今天无法取得。

## 设计

### 定位

默认简单、可展开高级。默认视图服务普通用户，全部诊断字段保留在折叠区内，信息不丢失。

### 组件一：健康卡（新增）

`LinuxRuntimeHealthCard` 展示一个**派生**的呈现状态，由 `(mode, rootfsState)` 计算得出。二者是不同维度：`LinuxRuntimeMode` 为 `legacy | mobileLinux`，而 `LinuxRuntimeRootfsState` 有 8 个 case（`missing / installing / ready / corrupt / repairing / resetting / unsupported / blockedByLicense`）。健康卡必须覆盖全部组合，不得只处理其中四个。

判定顺序（先匹配者生效）：

| 条件 | 标题 | 主操作 |
|---|---|---|
| `mode == .legacy` | ○ 已禁用 Linux | 启用 |
| `.unsupported` | ○ 当前环境不支持 | 无（说明 `capability.reason`） |
| `.blockedByLicense` | ▲ 授权阻塞 | 无（说明原因） |
| `.installing` / `.repairing` / `.resetting` | 进行中（复用 `state.label`） | 无，显示进度指示 |
| `.ready` | ● 环境就绪 | 打开终端 |
| `.missing` | ○ 尚未安装 | 安装 |
| `.corrupt` | ▲ 需要修复 | 修复 |

- 第二行：`Alpine <version> · <installed size>`
- 第三行：钉版工具链 `node <v> · git <v> · python <v>`（仅 `.ready` 且版本匹配时显示）

### 组件二：版本数据源

`clients/ios/scripts/build-linux-runtime.sh` 生成 `manifest.json` 时，把
`docs/mobile-linux/local-app-runtime-pins.json` 的 `runtime_packages` 一并写入。
该文件在 Xcode Run Script 阶段已被拷贝为 bundle 内的 `linux-runtime-manifest.json`，无需新增打包步骤。

`LXISHRuntimeBundleManifest` 增加 `runtimePackages: [String: String]` 字段并解析。

**展示条件**：仅当 `status.version == manifest.rootfsVersion` 时显示工具链行。旧 rootfs 配新二进制时不显示，而非谎报——钉版清单描述的是「本次构建打包了什么」，不是「设备上装了什么」。

### 组件三：渐进披露

- 默认可见：健康卡、终端、工作区
- 「高级与诊断」折叠区：后端选择、状态明细、维护、任务、挂载骨架、安全说明

后端选择（Mobile Linux / Legacy）移入折叠区，并补一句后果说明。选择 Legacy 会禁用整个 Linux 后端，普通用户误触即全功能不可用；保留能力但不再随手可及。

### 组件四：文件拆分

691 行拆为三个文件，各自单一职责：

- `LinuxRuntimeBridge.swift` — FFI 句柄缓存、config 构造、load/verify/repair/reset、状态映射
- `LinuxRuntimePage.swift` — 视图组合与分区
- `LinuxRuntimeHealthCard.swift` — 健康卡

FFI 桥接与视图挤在同一文件，是该页难以修改的结构性原因。

### 交互修复

`actionRow` / `actionButtonRow` 传 `onTap:`，整行成为触摸目标；移除 trailing 槽内冗余按钮。禁用态传 `onTap: nil`，副标题写明**具体原因**（例如「rootfs 未安装，无法校验」），替代泛化的「未启用」。

## 测试

- 现有 220 个 iOS 单测保持全绿
- 新增：
  - 派生呈现状态的映射，**覆盖全部 8 个 `LinuxRuntimeRootfsState` 与 2 个 mode 的组合**，而非只测 ready/missing/corrupt
  - `mode == .legacy` 优先于任何 rootfs 状态
  - manifest 的 `runtime_packages` 解析
  - 版本不匹配时不展示工具链行（断言「不显示」，而非只断言匹配时显示）
  - 启用态行携带 `onTap`、禁用态不携带

## 约束

**不得运行 `xcodegen generate`。** 用户已在 Xcode 中手工配置签名（Team: Flare App, Inc. / `AZ4AX7J833`，Bundle ID `com.lingxi.code.full`，Xcode Managed Profile）。`xcodegen` 会从 `project.yml` 重新生成 `.xcodeproj` 并冲掉该配置。新增的三个 Swift 文件位于 `Sources/` 下，已被 `project.yml` 的 `path: Sources` 覆盖，无需重新生成工程。

## 不在范围内

- 不改 `SettingsComponents.swift`（组件无缺陷）
- 不改 FFI 契约（`MobileLinuxStatusFfi` 保持不变；工具链版本走打包清单而非新增 FFI 字段）
- 不做首次进入的分步向导
