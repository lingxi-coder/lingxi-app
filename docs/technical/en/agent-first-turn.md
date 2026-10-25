# Run your first Agent turn

Go from an assembled Harness to a prompt, a terminal result, follow-up context and a clean shutdown.

<a id="prerequisites"></a>

## Prepare a real runtime

This recipe starts with &Harness from your host. HarnessBuilder wraps injected services; it does not choose a model, discover tools or create a workspace. Desktop build_harness(config, output, permissions).await assembles the existing desktop runtime and returns the facade.

1. Use the complete consumer manifest and the same SDK source identities as the host.
2. Connect output and permission services before building the composition.
3. Configure the workspace and an available provider/model. Begin with text and no image paths.

- [Complete consumer manifest](harness.md#installation)
- [Desktop composition example](harness-lifecycle.md#platform-assembly)

<a id="submit-and-inspect"></a>

## Submit a prompt and inspect state

| Value | Meaning |
| --- | --- |
| RunInput.prompt | User text passed to the agent execution path. |
| RunInput.images | Host-side paths resolved by the composition, not URLs or inline base64. |
| Result<TurnOutcome, HandleError> | Terminal execution status; streaming text and tools use the output destination. |
| session.transcript() | Ordered snapshot of the mounted conversation, useful for inspection or rebuilding a view. |

### Rust · function for an assembled host

```rust
use harness_runtime::{CancellationToken, HandleError, Harness, RunInput, TurnOutcome};

async fn run_and_inspect(
    harness: &Harness,
    prompt: String,
) -> Result<(TurnOutcome, usize), HandleError> {
    let session = harness.session();
    let outcome = session.run(
        RunInput { prompt, images: Vec::new() },
        CancellationToken::new(),
    ).await?;
    let message_count = session.transcript().await.len();
    Ok((outcome, message_count))
}
```

Verification method: Environment validation (harness-guide-first-turn)

<a id="classify-completion"></a>

## Handle every terminal case

| Result | Application response |
| --- | --- |
| Ok(EndTurn) | Finish this interaction and retain the session for a follow-up. |
| Ok(MaxTurns) | Explain that the configured turn budget was reached; preserve partial work. |
| Ok(Cancelled) | Clear the busy state after run unwinds; retain already produced output. |
| Err(ActionFailed(reason)) | Show the failure reason rather than an empty successful answer. |
| Err(Unimplemented(reason)) | Check selected host capabilities and wiring before retrying. |

<a id="continue-context"></a>

## Ask a follow-up sequentially

Call run again on the same mounted session only after the prior run completes. The composition retains conversation context. Cloning SessionHandle shares access to the same session; it neither creates another conversation nor establishes safe overlapping turns.

### Rust · sequential follow-up excerpt

```rust
let session = harness.session();
session.run(RunInput {
    prompt: "Explain the repository layout".into(),
    images: Vec::new(),
}, CancellationToken::new()).await?;

session.run(RunInput {
    prompt: "Which component owns persistence?".into(),
    images: Vec::new(),
}, CancellationToken::new()).await?;
```

Verification method: API excerpt

<a id="close-the-host"></a>

## Finish the host lifecycle

Do not shut down after each prompt when the application needs follow-ups. At actual exit, stop admission, finish or cancel active work and await it, then call harness.shutdown(). An incomplete ShutdownReport retains errors and publication receipts for a sequential retry.

### Rust · shutdown report excerpt

```rust
let report = harness.shutdown().await;
if !report.complete {
    // Retain report.errors and report.publications.
    // Resolve the outstanding barrier before a sequential shutdown retry.
}
```

Verification method: API excerpt

- [Cancellation and recovery](agent-session-workflows.md)
- [Shutdown contract](harness-lifecycle.md#shutdown)

## Sources and documentation baseline

[Source repository](https://github.com/lingxi-coder/harness-runtime)

- [harness-runtime/crates/runtime/src/api.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/runtime/src/api.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/runtime/src/lib.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/runtime/src/lib.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/runtime/src/desktop/sdk.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/runtime/src/desktop/sdk.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/core/src/host/orchestrator.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/core/src/host/orchestrator.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)

- harness-runtime: HEAD f975ab5804bb3af0e72beb3974e5c911ece44d53; Referenced input fingerprint sha256:92c2108dc63215bc69ea9c11ea081795da883fdcb1c9c28b060aa9221a1b95a2; Product pin e04c1837508014d68ea042d5c9fe3c8c15af5aa0

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
