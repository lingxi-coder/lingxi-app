# 宿主生命周期与持久化

组合宿主服务，并通过运行时持久化屏障完成关闭。

<a id="host-services"></a>

## 实现宿主服务

SessionService 与 LifecycleService 是 Send + Sync 异步 trait。服务应来自同一运行时组合，让会话句柄与关闭负责人对需要排空的资源保持一致。服务实现保留自身的执行、权限和持久化规则。

### SessionService::run

```text
async fn run(&self, input: RunInput, cancel: CancellationToken) -> Result<TurnOutcome, HandleError>
```

SessionHandle::run 背后的宿主执行方法。

### SessionService::session_id

```text
async fn session_id(&self) -> SessionId
```

报告该组合当前挂载的会话。

### SessionService::transcript

```text
async fn transcript(&self) -> Vec<ConversationMessage>
```

读取组合的有序对话记录。

### SessionService::cost

```text
async fn cost(&self) -> CostSnapshot
```

读取组合的累计模型用量与成本。

### LifecycleService::shutdown

```text
async fn shutdown(&self) -> ShutdownReport
```

通过组合的关闭协调器排空资源。

<a id="platform-assembly"></a>

## 使用平台组合

desktop::build_harness 将宿主的 OutputStream 和 PermissionGate 注入生产桌面组合，并返回共享 Harness 门面。平台组装具有明确的启动 I/O 与配置行为，与 HarnessBuilder 的包装过程不同。

mobile::build_mobile 接收 MobileConfig、Arc<dyn Platform>、ClientEventListener 和 PermissionRequestSink，返回 MobileRuntime。设备能力与平台服务通过 Platform 注入。

下方桌面包装器需要来自同一 Harness 源码身份的配套 permission crate；完整消费者清单已包含它。Headless 使用同一 desktop feature，接收显式宿主服务、字节 I/O 与信号，不安装信号处理器、接管全局 stdio 或调用 process::exit。

### Rust · 带宿主权限门的桌面组装

```rust
use std::sync::Arc;
use harness_runtime::{
    desktop::{self, BuildError, DesktopConfig},
    Harness, OutputStream,
};
use permission::gate::PermissionGate;

async fn desktop_harness(
    config: DesktopConfig,
    output: Arc<dyn OutputStream>,
    permissions: Arc<dyn PermissionGate>,
) -> Result<Harness, BuildError> {
    desktop::build_harness(config, output, permissions).await
}
```

验证方式: 编译检查 (harness-desktop-build)

### Rust · 使用宿主字节流的 Headless 执行

```rust
use std::sync::Arc;
use harness_runtime::{
    desktop::DesktopConfig,
    headless::{self, HeadlessConfig, HeadlessExit, HeadlessHostServices,
        HeadlessIo, HeadlessOptions, HeadlessSignals, SessionStart},
};

async fn headless_prompt(
    desktop: DesktopConfig,
    services: Arc<dyn HeadlessHostServices>,
    io: HeadlessIo,
    signals: HeadlessSignals,
    prompt: String,
) -> HeadlessExit {
    headless::run(
        HeadlessConfig {
            desktop,
            options: HeadlessOptions { prompt: Some(prompt), ..Default::default() },
            session_start: SessionStart::New,
        },
        services,
        io,
        signals,
    ).await
}
```

验证方式: 编译检查 (harness-headless-run)

### desktop::build_harness

```text
pub async fn build_harness(config: DesktopConfig, output: Arc<dyn OutputStream>, permissions: Arc<dyn PermissionGate>) -> Result<Harness, BuildError>
```

使用宿主权限门组装嵌入式桌面运行时，需启用 desktop。

### mobile::build_mobile

```text
pub async fn build_mobile(cfg: MobileConfig, platform: Arc<dyn Platform>, listener: Arc<dyn ClientEventListener>, permission_sink: Arc<dyn PermissionRequestSink>) -> Result<MobileRuntime, MobileBuildError>
```

从明确的宿主输入组装共享移动运行时，需启用 mobile。

HeadlessIo::new 接收注入的 AsyncRead 和 stdout／stderr AsyncWrite 实现。文本输入读取至 EOF；仅提示词调用使用空输入 reader，终端输入则显式标记。保留 HeadlessSignals::channel 返回的 sender，转发宿主中断／关闭。分别检查 HeadlessExit.execution、shutdown 与 delivery；编译示例不会执行模型轮次。

<a id="shutdown"></a>

## 排空后关闭

先停止接收新任务，再等待正在执行的轮次。完成宿主屏障后调用 Harness::shutdown。检查 complete 和 errors；关闭未完成时处理失败并顺序重试。桌面实现委托给既有会话生命周期协调器。

### Rust · after active turns have drained

```rust
use harness_runtime::{Harness, ShutdownReport};

async fn shutdown_after_turns(harness: &Harness) -> ShutdownReport {
    harness.shutdown().await
}
```

验证方式: 编译检查 (harness-shutdown)

### Harness::shutdown

```text
pub async fn shutdown(&self) -> ShutdownReport
```

请求注入的生命周期服务排空资源。

### ShutdownReport

```text
pub struct ShutdownReport { pub complete: bool, pub errors: Vec<String>, pub publications: Vec<lingxi_core::host::FusionPublicationReceipt> }
```

报告关闭屏障、可重试失败及持久化 Fusion 发布回执。

<a id="persistence"></a>

## 持久化由组合负责

session crate 导出 JsonlWriter、JsonlReader、SessionStorage、SessionResumer 和 RolloutRecorder。公共门面不会引入独立存储格式。SessionStorage 在独占 flock 下追加对话记录并执行 fsync；load 方法恢复已保存的对话。

SessionResumer 恢复已保存消息时创建空文件状态缓存，让文件重新读取而不是信任历史读取。重新挂载权限、插件、MCP 连接等实时服务仍是宿主职责。生产 JsonlWriter 与 rollout 格式是独立的底层接口，选择前请查阅 session 源码。

对话快照、事件交付和持久化写入是不同边界。确认资源关闭前，应等待组合的持久化与关闭屏障。

<a id="mock-lifecycle"></a>

## 无需供应商即可验证复制的门面示例

将以下完整测试模块放入消费者库。在 dev-dependencies 中加入 async-trait = "0.1" 与 tokio = { version = "1", features = ["macros", "rt"] }。它验证服务注入、挂载标识、取消和关闭，不模拟模型或文件系统。

### Rust · 完整模拟生命周期测试

```rust
#[cfg(test)]
mod lifecycle_example {
    use std::sync::Arc;
    use harness_runtime::{CancellationToken, ConversationMessage, CostSnapshot,
        HandleError, HarnessBuilder, LifecycleService, RunInput, SessionId,
        SessionService, ShutdownReport, TurnOutcome};

    struct Host { id: SessionId }

    #[async_trait::async_trait]
    impl SessionService for Host {
        async fn run(&self, _input: RunInput, cancel: CancellationToken)
            -> Result<TurnOutcome, HandleError> {
            Ok(if cancel.is_cancelled() { TurnOutcome::Cancelled }
                else { TurnOutcome::EndTurn })
        }
        async fn session_id(&self) -> SessionId { self.id }
        async fn transcript(&self) -> Vec<ConversationMessage> { Vec::new() }
        async fn cost(&self) -> CostSnapshot { CostSnapshot::default() }
    }

    #[async_trait::async_trait]
    impl LifecycleService for Host {
        async fn shutdown(&self) -> ShutdownReport {
            ShutdownReport { complete: true, ..Default::default() }
        }
    }

    #[tokio::test]
    async fn facade_cancellation_and_shutdown() {
        let host = Arc::new(Host { id: SessionId::new() });
        let harness = HarnessBuilder::new(host.clone(), host.clone()).build();
        assert_eq!(harness.session().id().await, host.id);
        let cancel = CancellationToken::new();
        cancel.cancel();
        let outcome = harness.session().run(RunInput::default(), cancel).await.unwrap();
        assert!(matches!(outcome, TurnOutcome::Cancelled));
        assert!(harness.shutdown().await.complete);
    }
}
```

验证方式: 模拟运行 (harness-lifecycle-mock)

生产中先停止新准入并等待所有活动 run future，再关闭。持久化屏障失败时 ShutdownReport.complete 保持 false，应对同一负责人顺序重试。不能为了隐藏未完成报告而替换活动 runtime。

## 源码与文档基线

[源码仓库](https://github.com/lingxi-coder/harness-runtime)

- [harness-runtime/crates/runtime/src/api.rs](https://github.com/lingxi-coder/harness-runtime/blob/1c08d9265d9a4fbf20d59c0776ddbfb96cb03e4d/crates/runtime/src/api.rs) — 已提交源码 (1c08d9265d9a4fbf20d59c0776ddbfb96cb03e4d)
- [harness-runtime/crates/runtime/src/desktop/sdk.rs](https://github.com/lingxi-coder/harness-runtime/blob/1c08d9265d9a4fbf20d59c0776ddbfb96cb03e4d/crates/runtime/src/desktop/sdk.rs) — 已提交源码 (1c08d9265d9a4fbf20d59c0776ddbfb96cb03e4d)
- [harness-runtime/crates/runtime/src/desktop/session_state.rs](https://github.com/lingxi-coder/harness-runtime/blob/1c08d9265d9a4fbf20d59c0776ddbfb96cb03e4d/crates/runtime/src/desktop/session_state.rs) — 已提交源码 (1c08d9265d9a4fbf20d59c0776ddbfb96cb03e4d)
- [harness-runtime/crates/runtime/src/mobile/host.rs](https://github.com/lingxi-coder/harness-runtime/blob/1c08d9265d9a4fbf20d59c0776ddbfb96cb03e4d/crates/runtime/src/mobile/host.rs) — 已提交源码 (1c08d9265d9a4fbf20d59c0776ddbfb96cb03e4d)
- [harness-runtime/crates/runtime/src/mobile/host/assembly.rs](https://github.com/lingxi-coder/harness-runtime/blob/1c08d9265d9a4fbf20d59c0776ddbfb96cb03e4d/crates/runtime/src/mobile/host/assembly.rs) — 已提交源码 (1c08d9265d9a4fbf20d59c0776ddbfb96cb03e4d)
- [harness-runtime/crates/runtime/src/headless/mod.rs](https://github.com/lingxi-coder/harness-runtime/blob/1c08d9265d9a4fbf20d59c0776ddbfb96cb03e4d/crates/runtime/src/headless/mod.rs) — 已提交源码 (1c08d9265d9a4fbf20d59c0776ddbfb96cb03e4d)
- [harness-runtime/crates/runtime/src/headless/config.rs](https://github.com/lingxi-coder/harness-runtime/blob/1c08d9265d9a4fbf20d59c0776ddbfb96cb03e4d/crates/runtime/src/headless/config.rs) — 已提交源码 (1c08d9265d9a4fbf20d59c0776ddbfb96cb03e4d)
- [harness-runtime/crates/runtime/src/headless/host.rs](https://github.com/lingxi-coder/harness-runtime/blob/1c08d9265d9a4fbf20d59c0776ddbfb96cb03e4d/crates/runtime/src/headless/host.rs) — 已提交源码 (1c08d9265d9a4fbf20d59c0776ddbfb96cb03e4d)
- [harness-runtime/crates/runtime/src/headless/io.rs](https://github.com/lingxi-coder/harness-runtime/blob/1c08d9265d9a4fbf20d59c0776ddbfb96cb03e4d/crates/runtime/src/headless/io.rs) — 已提交源码 (1c08d9265d9a4fbf20d59c0776ddbfb96cb03e4d)
- [harness-runtime/crates/runtime/src/headless/session.rs](https://github.com/lingxi-coder/harness-runtime/blob/1c08d9265d9a4fbf20d59c0776ddbfb96cb03e4d/crates/runtime/src/headless/session.rs) — 已提交源码 (1c08d9265d9a4fbf20d59c0776ddbfb96cb03e4d)
- [harness-runtime/crates/permission/src/gate.rs](https://github.com/lingxi-coder/harness-runtime/blob/1c08d9265d9a4fbf20d59c0776ddbfb96cb03e4d/crates/permission/src/gate.rs) — 已提交源码 (1c08d9265d9a4fbf20d59c0776ddbfb96cb03e4d)
- [harness-runtime/crates/session/src/storage.rs](https://github.com/lingxi-coder/harness-runtime/blob/1c08d9265d9a4fbf20d59c0776ddbfb96cb03e4d/crates/session/src/storage.rs) — 已提交源码 (1c08d9265d9a4fbf20d59c0776ddbfb96cb03e4d)
- [harness-runtime/crates/session/src/resumer.rs](https://github.com/lingxi-coder/harness-runtime/blob/1c08d9265d9a4fbf20d59c0776ddbfb96cb03e4d/crates/session/src/resumer.rs) — 已提交源码 (1c08d9265d9a4fbf20d59c0776ddbfb96cb03e4d)

- harness-runtime: HEAD 1c08d9265d9a4fbf20d59c0776ddbfb96cb03e4d; 所据文件指纹 sha256:d77506e15375f8653d0ae9198567f3dbeba4e931dce8226b58971350a635ae8a; 产品固定版本 1c08d9265d9a4fbf20d59c0776ddbfb96cb03e4d

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
