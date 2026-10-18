# Harness Runtime

将 Rust 智能体运行时嵌入桌面、移动端或自定义宿主。

<a id="overview"></a>

## 运行时边界

Harness Runtime 是从灵犀提取的可嵌入执行层，组合智能体、会话、模型宿主服务、权限、工具、插件与持久化。Cargo 包名为 harness-runtime，Rust 导入名为 harness_runtime。

宿主负责产品界面、入口、凭据、平台能力和应用生命周期。独立的 llm-client 负责与模型供应商通信。HarnessBuilder 包装宿主已经组装的服务；构建该门面不会启动进程或发起网络请求。

这些页面描述当前工作区。源码基线分别记录 HEAD、未提交文件和产品依赖版本。本地接口并不自动包含在产品固定版本中；编译器解析后的类型和 feature 请查阅 Rustdoc。

<a id="installation"></a>

## 使用唯一源码身份

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

## 源码与文档基线

[源码仓库](https://github.com/lingxi-coder/harness-runtime)

- [harness-runtime/crates/runtime/src/lib.rs](https://github.com/lingxi-coder/harness-runtime/blob/4d6d0d6c428e6624a303f5e2a5c96eb4a392f1a1/crates/runtime/src/lib.rs) — 已提交源码 (4d6d0d6c428e6624a303f5e2a5c96eb4a392f1a1)
- [harness-runtime/crates/runtime/src/api.rs](https://github.com/lingxi-coder/harness-runtime/blob/4d6d0d6c428e6624a303f5e2a5c96eb4a392f1a1/crates/runtime/src/api.rs) — 已提交源码 (4d6d0d6c428e6624a303f5e2a5c96eb4a392f1a1)
- [harness-runtime/crates/runtime/Cargo.toml](https://github.com/lingxi-coder/harness-runtime/blob/4d6d0d6c428e6624a303f5e2a5c96eb4a392f1a1/crates/runtime/Cargo.toml) — 已提交源码 (4d6d0d6c428e6624a303f5e2a5c96eb4a392f1a1)
- [harness-runtime/Cargo.toml](https://github.com/lingxi-coder/harness-runtime/blob/4d6d0d6c428e6624a303f5e2a5c96eb4a392f1a1/Cargo.toml) — 已提交源码 (4d6d0d6c428e6624a303f5e2a5c96eb4a392f1a1)
- [harness-runtime/rust-toolchain.toml](https://github.com/lingxi-coder/harness-runtime/blob/4d6d0d6c428e6624a303f5e2a5c96eb4a392f1a1/rust-toolchain.toml) — 已提交源码 (4d6d0d6c428e6624a303f5e2a5c96eb4a392f1a1)

- harness-runtime: HEAD 4d6d0d6c428e6624a303f5e2a5c96eb4a392f1a1; 所据文件指纹 sha256:e9079905b280000c8013dd7f16954f3709e464fdcf67fca25de7e73176e1230a; 产品固定版本 4d6d0d6c428e6624a303f5e2a5c96eb4a392f1a1

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
