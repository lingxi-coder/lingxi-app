# Harness Runtime

将 Rust 智能体运行时嵌入桌面、移动端或自定义宿主。

<a id="overview"></a>

## 运行时边界

Harness Runtime 是从灵犀提取的可嵌入执行层，组合智能体、会话、模型宿主服务、权限、工具、插件与持久化。Cargo 包名为 harness-runtime，Rust 导入名为 harness_runtime。

宿主负责产品界面、入口、凭据、平台能力和应用生命周期。独立的 llm-client 负责与模型供应商通信。HarnessBuilder 包装宿主已经组装的服务；构建该门面不会启动进程或发起网络请求。

这些页面描述当前工作区。源码基线分别记录 HEAD、未提交文件和产品依赖版本。本地接口并不自动包含在产品固定版本中；编译器解析后的类型和 feature 请查阅 Rustdoc。

<a id="installation"></a>

## 使用唯一源码身份

应用接入时应保留 Harness 声明的依赖 revision。下面的多仓 path overlay 用于联合开发 SDK，要求各仓契约一致。将固定的 LLM Client 替换成任意更新的工作树，可能在编译应用前就因 llm-runtime 的接口不一致而失败；Cargo 包版本号本身不能证明源码兼容。

验证当前工作树示例时，在 harness-runtime、llm-client 与 mobile-linux-runtime 旁创建 sdk-consumer，并使用下方完整本地清单。直接 Harness crate 与显式消费者 patch 选择三个 SDK 工作树；编译复制的示例前应通过 cargo metadata 验证这些源码身份。

### Cargo.toml · 工作树消费者

```toml
[package]
name = "handbook-harness-consumer"
version = "0.1.0"
edition = "2024"
publish = false

[workspace]

[dependencies]
harness-runtime = { path = "../harness-runtime/crates/runtime", default-features = false, features = ["desktop"] }
client = { path = "../harness-runtime/crates/client", features = ["adapter"] }
tool-api = { path = "../harness-runtime/crates/tool-api" }
permission = { path = "../harness-runtime/crates/permission" }
async-trait = "0.1"
tokio = { version = "1", features = ["rt-multi-thread", "macros", "sync"] }

[patch."https://github.com/lingxi-coder/llm-client.git"]
lingxi-llm-client = { path = "../llm-client" }

[patch."https://github.com/lingxi-coder/mobile-linux-runtime.git"]
mobile-linux-api = { path = "../mobile-linux-runtime/crates/mobile-linux-api" }
mobile-linux-core = { path = "../mobile-linux-runtime/crates/mobile-linux-core" }
mobile-linux-android = { path = "../mobile-linux-runtime/crates/mobile-linux-android" }
mobile-linux-ios = { path = "../mobile-linux-runtime/crates/mobile-linux-ios" }
platform-pty = { path = "../mobile-linux-runtime/crates/platform-pty" }

[patch.crates-io]
git2 = { path = "../harness-runtime/third_party/git2-rs/git2" }
```

验证方式: 需环境验证 (harness-working-tree-manifest)

### Source checkout

```bash
git clone --recurse-submodules https://github.com/lingxi-coder/harness-runtime.git
cd harness-runtime
git submodule update --init --recursive
cargo check --locked -p harness-runtime
```

验证方式: 需环境验证

仓库固定 Rust 1.94.0，根目录没有指向相邻 llm-client 的 path patch。这些覆盖显式属于隔离消费者。发布集成时，所有直接 Harness 包使用同一经审查完整 Git 提交，移除 SDK 工作树覆盖并审查锁文件。共享 Rust 类型不能混用 Git 与本地来源。

<a id="features"></a>

## 选择组合配置

- engine — 默认启用，提供 Harness、构建器、会话句柄和共享执行组件。
- desktop — 包含 engine 及桌面组装、工具、客户端适配器、平台服务、Fusion、工作流与协作。
- mobile — 包含 engine 及 iOS/Android 共享 Rust 组合；设备与平台能力由宿主注入。
- uniffi — 包含 mobile 及原生绑定元数据。android-computer-use 同样包含 mobile。
- realtime-websocket — 启用模型运行时的实时 WebSocket 支持。fusion、workflow 和 collaboration 也可单独选择。

当前 Cargo 功能名为 engine。旧 README 中名为 core 的示例不适用于此版本，请以 crates/runtime/Cargo.toml 中的名称为准。

<a id="builder"></a>

## 组装公共门面

### Rust · host-supplied services

```rust
use std::sync::Arc;
use harness_runtime::{
    Harness, HarnessBuilder, LifecycleService, SessionService,
};

fn assemble(
    session: Arc<dyn SessionService>,
    lifecycle: Arc<dyn LifecycleService>,
) -> Harness {
    HarnessBuilder::new(session, lifecycle).build()
}
```

验证方式: 编译检查 (harness-assemble)

### HarnessBuilder::new

```text
pub fn new(session: Arc<dyn SessionService>, lifecycle: Arc<dyn LifecycleService>) -> Self
```

接收来自同一已组装运行时的会话与生命周期服务。

### HarnessBuilder::build

```text
pub fn build(self) -> Harness
```

将注入服务保存在 SDK 的不透明句柄后。

### Harness::session

```text
pub fn session(&self) -> SessionHandle
```

返回指向当前组合中已挂载会话的轻量句柄副本。

<a id="embedding-contract"></a>

## 宿主准入、资源与失败

使用工作线程栈至少为 harness_runtime::RUNTIME_THREAD_STACK_SIZE（8 MiB）的 Tokio runtime。平台组装加载配置、构建注册表并启动受其管理的服务，可能在会话可用前失败。仅构建门面无法验证凭据或平台就绪状态。

Rust 宿主自己提供权限界面时选择 desktop::build_harness；显式设备服务选择 mobile::build_mobile；按字节进行非交互执行选择 headless::run。Headless 由 desktop 启用，分别报告执行、持久化关闭和输出交付。

- [Agent 执行与恢复](agent-execution.md)

现有 sdk_surface 测试检查不透明 SDK 契约；平台组装与真实供应商执行需要各自验证。构建结果不是模型服务或设备验收结果。

<a id="choose-entrypoint"></a>

## 为你的应用选择入口

先确定要构建哪一种宿主。聊天窗口、批处理 worker 与移动应用需要不同的输入、权限和关闭方式，即使它们使用同一个 Agent 循环。

| 目标 | 使用入口 | 宿主需要提供 |
| --- | --- | --- |
| 嵌入 Rust 聊天或桌面界面 | desktop::build_harness | DesktopConfig、OutputStream 与 PermissionGate |
| 运行非交互任务 | headless::run | HeadlessHostServices、字节读写流与宿主信号 |
| 集成 Android 或 iOS | mobile::build_mobile | MobileConfig、Platform、事件监听器与权限请求 sink |
| 包装已组装的运行时 | HarnessBuilder::new | 来自同一组合的 SessionService 与 LifecycleService |

- [平台组装与关闭](harness-lifecycle.md#platform-assembly)

<a id="first-agent-workflow"></a>

## 接通第一个 Agent 工作流

1. 准备应用工作空间与模型／供应商配置。选择组合配置，让直接依赖的 Harness 包保持相同源码版本。
2. 先创建输出目标，再组装运行时。如果界面消费 ClientEvent DTO，用 AdapterOutputStream 包装一个共享 ClientEventSink。
3. 提供能联系用户、或执行明确非交互策略的权限门。宿主挂载会话后再接收提示词。
4. 向 session().run 传入 RunInput 与宿主持有的 CancellationToken。等待终态结果的同时，从输出流接收文本和工具活动。
5. 保留返回结果、会话标识与成本快照。关闭应用时停止新提示词，等待活动轮次排空，再检查 shutdown().complete。

- [复制完整服务注入示例](harness.md#builder)
- [执行、取消与检查轮次](harness-sessions.md)
- [连接客户端事件 sink](harness-client.md#output-adapter)

<a id="startup-troubleshooting"></a>

## 排查启动问题

| 现象 | 先检查 | 下一步 |
| --- | --- | --- |
| 构建器成功，但没有执行任务 | HarnessBuilder 只保留服务，不会提交提示词。 | 调用已挂载的 SessionHandle::run，并消费它的输出目标。 |
| 界面收不到文本 | OutputStream 或 ClientEventSink 的归属与交付。 | 确认启动前接入同一个 sink，避免在执行回调里阻塞传输写入。 |
| 操作一直等待授权 | 宿主权限请求与响应路由。 | 响应准确的 request_id，或在运行前提供预期的非交互策略。 |
| 共享 Rust 类型不兼容 | cargo metadata 中重复的 Git／path 身份。 | 使用完整消费者清单，并让配套 crate 采用同一经审查源码身份。 |

## 源码与文档基线

[源码仓库](https://github.com/lingxi-coder/harness-runtime)

- [harness-runtime/crates/runtime/src/lib.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/runtime/src/lib.rs) — 已提交源码 (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/runtime/src/api.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/runtime/src/api.rs) — 已提交源码 (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/runtime/Cargo.toml](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/runtime/Cargo.toml) — 已提交源码 (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/Cargo.toml](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/Cargo.toml) — 已提交源码 (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/rust-toolchain.toml](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/rust-toolchain.toml) — 已提交源码 (f975ab5804bb3af0e72beb3974e5c911ece44d53)

- harness-runtime: HEAD f975ab5804bb3af0e72beb3974e5c911ece44d53; 所据文件指纹 sha256:92c2108dc63215bc69ea9c11ea081795da883fdcb1c9c28b060aa9221a1b95a2; 产品固定版本 0e28910fd624638e6c5d1f92eca4899a7938d188

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
