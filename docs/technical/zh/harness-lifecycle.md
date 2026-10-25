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

<a id="headless-options"></a>

## 配置批处理或流式 worker

headless::run 是注入 I/O 的可嵌入应用入口。把读写流交给运行时前先确定输入／输出契约；关闭输入流与取消轮次是不同操作。

| HeadlessOptions 字段 | 默认值 | 用途 |
| --- | --- | --- |
| prompt / prompt_images | None／空 | 宿主直接提供的初始文本与图像。 |
| input_format / output_format | Text／Text | 选择纯文本或支持的结构化流格式，配套相应读写器。 |
| no_stream | false | 为一次性提示词选择供应商批量响应路径。 |
| include_partial_messages / include_hook_events | false／false | 在 stream-json 输出中按需包含助手部分消息或 hook 帧。 |
| max_budget_usd / json_schema | None／None | 设置本次调用预算，或保留源信息的结构化输出 schema。 |
| permission_prompts_none | false | 宿主明确没有提示传输时禁用交互提示；它不代表授予全部权限。 |

<a id="shutdown-result"></a>

## 分别处理执行、持久化与交付

| 检查项 | 能说明什么 | 失败时保留什么 |
| --- | --- | --- |
| HeadlessExit.execution | 模型／运行时执行是否开始，以及如何结束。 | reason、errors 与可用时的 session_id。 |
| HeadlessExit.shutdown | 资源排空与持久化屏障是否完成。 | shutdown_errors 与可选 cleanup 句柄。 |
| HeadlessExit.delivery | 最终输出与 writer 关闭是否完成。 | delivery_errors；输出缺失时不要报告交付成功。 |
| ShutdownReport.complete | 注入的生命周期服务是否完成全部必要屏障。 | errors 与持久化 publications，再顺序重试关闭。 |

模型响应成功时仍可能发生输出交付失败；最终文本已交付时，关闭未完成也仍有意义。嵌入应用应分别展示这些状态。

<a id="resume-checklist"></a>

## 宿主重启后恢复对话

1. 通过宿主既有恢复路径选择会话，不要修改 SessionHandle::id 的返回值来模拟恢复。
2. 恢复持久化的有序对话及消息标识。为新宿主生命周期重新接入模型服务、输出目标、权限策略与扩展负责人。
3. 按照所选恢复实现处理历史文件读取。上文的底层 SessionResumer 从空缓存开始；不要复制另一工作空间的缓存来让编辑通过。
4. 接收下一条提示词前，先挂载恢复后的会话。新的对话快照应来自该已挂载运行时。

- [会话快照](harness-sessions.md#snapshots)

## 源码与文档基线

[源码仓库](https://github.com/lingxi-coder/harness-runtime)

- [harness-runtime/crates/runtime/src/api.rs](https://github.com/lingxi-coder/harness-runtime/blob/0e28910fd624638e6c5d1f92eca4899a7938d188/crates/runtime/src/api.rs) — 已提交源码 (0e28910fd624638e6c5d1f92eca4899a7938d188)
- [harness-runtime/crates/runtime/src/desktop/sdk.rs](https://github.com/lingxi-coder/harness-runtime/blob/0e28910fd624638e6c5d1f92eca4899a7938d188/crates/runtime/src/desktop/sdk.rs) — 已提交源码 (0e28910fd624638e6c5d1f92eca4899a7938d188)
- [harness-runtime/crates/runtime/src/desktop/session_state.rs](https://github.com/lingxi-coder/harness-runtime/blob/0e28910fd624638e6c5d1f92eca4899a7938d188/crates/runtime/src/desktop/session_state.rs) — 已提交源码 (0e28910fd624638e6c5d1f92eca4899a7938d188)
- [harness-runtime/crates/runtime/src/mobile/host.rs](https://github.com/lingxi-coder/harness-runtime/blob/0e28910fd624638e6c5d1f92eca4899a7938d188/crates/runtime/src/mobile/host.rs) — 已提交源码 (0e28910fd624638e6c5d1f92eca4899a7938d188)
- [harness-runtime/crates/runtime/src/mobile/host/assembly.rs](https://github.com/lingxi-coder/harness-runtime/blob/0e28910fd624638e6c5d1f92eca4899a7938d188/crates/runtime/src/mobile/host/assembly.rs) — 已提交源码 (0e28910fd624638e6c5d1f92eca4899a7938d188)
- [harness-runtime/crates/runtime/src/headless/mod.rs](https://github.com/lingxi-coder/harness-runtime/blob/0e28910fd624638e6c5d1f92eca4899a7938d188/crates/runtime/src/headless/mod.rs) — 已提交源码 (0e28910fd624638e6c5d1f92eca4899a7938d188)
- [harness-runtime/crates/runtime/src/headless/config.rs](https://github.com/lingxi-coder/harness-runtime/blob/0e28910fd624638e6c5d1f92eca4899a7938d188/crates/runtime/src/headless/config.rs) — 已提交源码 (0e28910fd624638e6c5d1f92eca4899a7938d188)
- [harness-runtime/crates/runtime/src/headless/host.rs](https://github.com/lingxi-coder/harness-runtime/blob/0e28910fd624638e6c5d1f92eca4899a7938d188/crates/runtime/src/headless/host.rs) — 已提交源码 (0e28910fd624638e6c5d1f92eca4899a7938d188)
- [harness-runtime/crates/runtime/src/headless/io.rs](https://github.com/lingxi-coder/harness-runtime/blob/0e28910fd624638e6c5d1f92eca4899a7938d188/crates/runtime/src/headless/io.rs) — 已提交源码 (0e28910fd624638e6c5d1f92eca4899a7938d188)
- [harness-runtime/crates/runtime/src/headless/session.rs](https://github.com/lingxi-coder/harness-runtime/blob/0e28910fd624638e6c5d1f92eca4899a7938d188/crates/runtime/src/headless/session.rs) — 已提交源码 (0e28910fd624638e6c5d1f92eca4899a7938d188)
- [harness-runtime/crates/runtime/src/desktop/configuration.rs](https://github.com/lingxi-coder/harness-runtime/blob/0e28910fd624638e6c5d1f92eca4899a7938d188/crates/runtime/src/desktop/configuration.rs) — 已提交源码 (0e28910fd624638e6c5d1f92eca4899a7938d188)
- [harness-runtime/crates/permission/src/gate.rs](https://github.com/lingxi-coder/harness-runtime/blob/0e28910fd624638e6c5d1f92eca4899a7938d188/crates/permission/src/gate.rs) — 已提交源码 (0e28910fd624638e6c5d1f92eca4899a7938d188)
- [harness-runtime/crates/session/src/storage.rs](https://github.com/lingxi-coder/harness-runtime/blob/0e28910fd624638e6c5d1f92eca4899a7938d188/crates/session/src/storage.rs) — 已提交源码 (0e28910fd624638e6c5d1f92eca4899a7938d188)
- [harness-runtime/crates/session/src/resumer.rs](https://github.com/lingxi-coder/harness-runtime/blob/0e28910fd624638e6c5d1f92eca4899a7938d188/crates/session/src/resumer.rs) — 已提交源码 (0e28910fd624638e6c5d1f92eca4899a7938d188)

- harness-runtime: HEAD 0e28910fd624638e6c5d1f92eca4899a7938d188; 所据文件指纹 sha256:b6b36966a8f4c48b8d0813ea03f73cd617387ddaf2531d8254e4a5ac81f48255; 产品固定版本 0e28910fd624638e6c5d1f92eca4899a7938d188

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
