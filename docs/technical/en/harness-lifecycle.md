# Host lifecycle & persistence

Compose host services and shut down through the runtime’s durability barriers.

<a id="host-services"></a>

## Implement host services

SessionService and LifecycleService are Send + Sync async traits. Use services from one runtime composition so the session handle and shutdown owner agree about the resources being drained. Service implementations retain their execution, permission, and persistence rules.

### SessionService::run

```text
async fn run(&self, input: RunInput, cancel: CancellationToken) -> Result<TurnOutcome, HandleError>
```

The host-provided execution method behind SessionHandle::run.

### SessionService::session_id

```text
async fn session_id(&self) -> SessionId
```

Report the session mounted by this composition.

### SessionService::transcript

```text
async fn transcript(&self) -> Vec<ConversationMessage>
```

Read the ordered transcript from the composition.

### SessionService::cost

```text
async fn cost(&self) -> CostSnapshot
```

Read the composition’s cumulative model usage and cost.

### LifecycleService::shutdown

```text
async fn shutdown(&self) -> ShutdownReport
```

Drain resources through the composition’s shutdown coordinator.

<a id="platform-assembly"></a>

## Use the platform composition

desktop::build_harness injects the host’s OutputStream and PermissionGate into the production desktop composition and returns the shared Harness facade. Unlike HarnessBuilder, platform assembly has explicit startup I/O and configuration semantics.

mobile::build_mobile takes MobileConfig, an Arc<dyn Platform>, a ClientEventListener, and a PermissionRequestSink. It returns MobileRuntime; device capabilities and platform services are supplied through Platform.

The desktop wrapper below requires the companion permission crate from the same Harness source identity. The complete consumer manifest includes that dependency. Headless uses the same desktop feature and accepts explicit host services, byte I/O and signals; it does not install signal handlers, take over global stdio or call process::exit.

### Rust · desktop assembly with a host permission gate

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

Verification method: Compile check (harness-desktop-build)

### Rust · headless run with host-owned byte streams

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

Verification method: Compile check (harness-headless-run)

### desktop::build_harness

```text
pub async fn build_harness(config: DesktopConfig, output: Arc<dyn OutputStream>, permissions: Arc<dyn PermissionGate>) -> Result<Harness, BuildError>
```

Assemble an embedded desktop runtime with a host permission gate. Requires desktop.

### mobile::build_mobile

```text
pub async fn build_mobile(cfg: MobileConfig, platform: Arc<dyn Platform>, listener: Arc<dyn ClientEventListener>, permission_sink: Arc<dyn PermissionRequestSink>) -> Result<MobileRuntime, MobileBuildError>
```

Assemble the shared mobile runtime from explicit host inputs. Requires mobile.

HeadlessIo::new takes injected AsyncRead and stdout/stderr AsyncWrite implementations. Text input reads to EOF; use an empty input reader for a prompt-only call, or explicitly mark a terminal input. Keep the sender returned by HeadlessSignals::channel to forward host interrupts/shutdown. Inspect HeadlessExit.execution, shutdown and delivery independently; compilation does not execute a model turn.

<a id="shutdown"></a>

## Drain before closing

First stop admitting new work, then await outstanding turns. Call Harness::shutdown only after that host barrier. Inspect complete and errors; if shutdown is incomplete, handle the failures and retry sequentially. The desktop implementation delegates to its existing session lifecycle coordinator.

### Rust · after active turns have drained

```rust
use harness_runtime::{Harness, ShutdownReport};

async fn shutdown_after_turns(harness: &Harness) -> ShutdownReport {
    harness.shutdown().await
}
```

Verification method: Compile check (harness-shutdown)

### Harness::shutdown

```text
pub async fn shutdown(&self) -> ShutdownReport
```

Request resource drain from the injected lifecycle service.

### ShutdownReport

```text
pub struct ShutdownReport { pub complete: bool, pub errors: Vec<String>, pub publications: Vec<lingxi_core::host::FusionPublicationReceipt> }
```

Report shutdown barriers, retryable failures, and durable Fusion publication receipts.

<a id="persistence"></a>

## Persistence belongs to the composition

The session crate exports JsonlWriter, JsonlReader, SessionStorage, SessionResumer, and RolloutRecorder. The facade does not introduce a separate storage format. SessionStorage appends transcripts under an exclusive flock and fsync; its load method recovers the stored conversation.

SessionResumer restores stored messages with an empty file-state cache so files are read again instead of trusting historical reads. Reattaching permissions, plugins, MCP connections, and other live services remains a host responsibility. The production JsonlWriter and rollout format are separate lower-level surfaces; consult the session source before selecting one.

Transcript snapshots, event delivery, and durable writes are different boundaries. Await the composition’s durability and shutdown barriers before treating resources as closed.

<a id="mock-lifecycle"></a>

## Exercise the copied facade without a provider

Put this complete test module in the consumer library. Add async-trait = "0.1" and tokio = { version = "1", features = ["macros", "rt"] } to dev-dependencies. It tests service injection, mounted identity, cancellation and shutdown; it does not simulate a model or a filesystem.

### Rust · complete mock lifecycle test

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

Verification method: Mock run (harness-lifecycle-mock)

In production, stop new admissions and await every active run future before shutdown. A failed durability barrier keeps ShutdownReport.complete false; retry the same owner sequentially. Do not replace the live runtime just to hide an incomplete report.

## Sources and documentation baseline

[Source repository](https://github.com/lingxi-coder/harness-runtime)

- [harness-runtime/crates/runtime/src/api.rs](https://github.com/lingxi-coder/harness-runtime/blob/bb8be1b7434045b0a85507315c1df9a5ddc27875/crates/runtime/src/api.rs) — Committed source (bb8be1b7434045b0a85507315c1df9a5ddc27875)
- [harness-runtime/crates/runtime/src/desktop/sdk.rs](https://github.com/lingxi-coder/harness-runtime/blob/bb8be1b7434045b0a85507315c1df9a5ddc27875/crates/runtime/src/desktop/sdk.rs) — Committed source (bb8be1b7434045b0a85507315c1df9a5ddc27875)
- [harness-runtime/crates/runtime/src/desktop/session_state.rs](https://github.com/lingxi-coder/harness-runtime/blob/bb8be1b7434045b0a85507315c1df9a5ddc27875/crates/runtime/src/desktop/session_state.rs) — Committed source (bb8be1b7434045b0a85507315c1df9a5ddc27875)
- [harness-runtime/crates/runtime/src/mobile/host.rs](https://github.com/lingxi-coder/harness-runtime/blob/bb8be1b7434045b0a85507315c1df9a5ddc27875/crates/runtime/src/mobile/host.rs) — Committed source (bb8be1b7434045b0a85507315c1df9a5ddc27875)
- [harness-runtime/crates/runtime/src/mobile/host/assembly.rs](https://github.com/lingxi-coder/harness-runtime/blob/bb8be1b7434045b0a85507315c1df9a5ddc27875/crates/runtime/src/mobile/host/assembly.rs) — Committed source (bb8be1b7434045b0a85507315c1df9a5ddc27875)
- [harness-runtime/crates/runtime/src/headless/mod.rs](https://github.com/lingxi-coder/harness-runtime/blob/bb8be1b7434045b0a85507315c1df9a5ddc27875/crates/runtime/src/headless/mod.rs) — Committed source (bb8be1b7434045b0a85507315c1df9a5ddc27875)
- [harness-runtime/crates/runtime/src/headless/config.rs](https://github.com/lingxi-coder/harness-runtime/blob/bb8be1b7434045b0a85507315c1df9a5ddc27875/crates/runtime/src/headless/config.rs) — Committed source (bb8be1b7434045b0a85507315c1df9a5ddc27875)
- [harness-runtime/crates/runtime/src/headless/host.rs](https://github.com/lingxi-coder/harness-runtime/blob/bb8be1b7434045b0a85507315c1df9a5ddc27875/crates/runtime/src/headless/host.rs) — Committed source (bb8be1b7434045b0a85507315c1df9a5ddc27875)
- [harness-runtime/crates/runtime/src/headless/io.rs](https://github.com/lingxi-coder/harness-runtime/blob/bb8be1b7434045b0a85507315c1df9a5ddc27875/crates/runtime/src/headless/io.rs) — Committed source (bb8be1b7434045b0a85507315c1df9a5ddc27875)
- [harness-runtime/crates/runtime/src/headless/session.rs](https://github.com/lingxi-coder/harness-runtime/blob/bb8be1b7434045b0a85507315c1df9a5ddc27875/crates/runtime/src/headless/session.rs) — Committed source (bb8be1b7434045b0a85507315c1df9a5ddc27875)
- [harness-runtime/crates/permission/src/gate.rs](https://github.com/lingxi-coder/harness-runtime/blob/bb8be1b7434045b0a85507315c1df9a5ddc27875/crates/permission/src/gate.rs) — Committed source (bb8be1b7434045b0a85507315c1df9a5ddc27875)
- [harness-runtime/crates/session/src/storage.rs](https://github.com/lingxi-coder/harness-runtime/blob/bb8be1b7434045b0a85507315c1df9a5ddc27875/crates/session/src/storage.rs) — Committed source (bb8be1b7434045b0a85507315c1df9a5ddc27875)
- [harness-runtime/crates/session/src/resumer.rs](https://github.com/lingxi-coder/harness-runtime/blob/bb8be1b7434045b0a85507315c1df9a5ddc27875/crates/session/src/resumer.rs) — Committed source (bb8be1b7434045b0a85507315c1df9a5ddc27875)

- harness-runtime: HEAD bb8be1b7434045b0a85507315c1df9a5ddc27875; Referenced input fingerprint sha256:e9079905b280000c8013dd7f16954f3709e464fdcf67fca25de7e73176e1230a; Product pin bb8be1b7434045b0a85507315c1df9a5ddc27875

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
