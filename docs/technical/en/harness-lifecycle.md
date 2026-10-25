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

<a id="headless-options"></a>

## Configure a batch or streaming worker

headless::run is an embeddable application entry point with injected I/O. Set the input/output contract before handing readers and writers to the runtime; closing an input stream and cancelling a turn are different operations.

| HeadlessOptions field | Default | Use it for |
| --- | --- | --- |
| prompt / prompt_images | None / empty | Initial text and images supplied directly by the host. |
| input_format / output_format | Text / Text | Choose plain text or the supported structured stream formats; use the corresponding reader/writer. |
| no_stream | false | Select the batched provider path for a one-shot prompt. |
| include_partial_messages / include_hook_events | false / false | Opt into partial assistant frames or hook frames in stream-json output. |
| max_budget_usd / json_schema | None / None | Set an invocation budget or a source-preserving structured-output schema. |
| permission_prompts_none | false | Disable interactive prompts when the host intentionally has no prompt transport; this is not a blanket permission grant. |

<a id="shutdown-result"></a>

## Treat execution, persistence and delivery separately

| Inspect | What it establishes | What to retain on failure |
| --- | --- | --- |
| HeadlessExit.execution | Whether model/runtime execution started and how it ended. | reason, errors and session_id when available. |
| HeadlessExit.shutdown | Whether resource drain and persistence barriers completed. | shutdown_errors and the optional cleanup handle. |
| HeadlessExit.delivery | Whether final output and writer shutdown completed. | delivery_errors; do not report missing output as successful delivery. |
| ShutdownReport.complete | Whether the injected lifecycle service finished every required barrier. | errors and durable publications, then retry shutdown sequentially. |

A successful model response can coexist with failed output delivery. An incomplete shutdown is also meaningful even when the final text was delivered. Keep those states visible to the embedding application.

<a id="resume-checklist"></a>

## Resume a conversation after restarting the host

1. Select the session through the host’s existing resume path, not by changing the value returned from SessionHandle::id.
2. Restore the durable ordered transcript and its message identities. Reattach the model service, output destination, permission policy and extension owners for this new host lifetime.
3. Treat historical file reads according to the selected resume implementation. The lower-level SessionResumer documented above starts with an empty cache; do not copy a cache from another workspace to make an edit pass.
4. Mount the restored session before admitting the next prompt. A new transcript snapshot should come from that mounted runtime.

- [Session snapshots](harness-sessions.md#snapshots)

## Sources and documentation baseline

[Source repository](https://github.com/lingxi-coder/harness-runtime)

- [harness-runtime/crates/runtime/src/api.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/runtime/src/api.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/runtime/src/desktop/sdk.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/runtime/src/desktop/sdk.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/runtime/src/desktop/session_state.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/runtime/src/desktop/session_state.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/runtime/src/mobile/host.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/runtime/src/mobile/host.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/runtime/src/mobile/host/assembly.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/runtime/src/mobile/host/assembly.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/runtime/src/headless/mod.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/runtime/src/headless/mod.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/runtime/src/headless/config.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/runtime/src/headless/config.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/runtime/src/headless/host.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/runtime/src/headless/host.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/runtime/src/headless/io.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/runtime/src/headless/io.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/runtime/src/headless/session.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/runtime/src/headless/session.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/runtime/src/desktop/configuration.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/runtime/src/desktop/configuration.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/permission/src/gate.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/permission/src/gate.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/session/src/storage.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/session/src/storage.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/session/src/resumer.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/session/src/resumer.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)

- harness-runtime: HEAD f975ab5804bb3af0e72beb3974e5c911ece44d53; Referenced input fingerprint sha256:92c2108dc63215bc69ea9c11ea081795da883fdcb1c9c28b060aa9221a1b95a2; Product pin 0e28910fd624638e6c5d1f92eca4899a7938d188

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
