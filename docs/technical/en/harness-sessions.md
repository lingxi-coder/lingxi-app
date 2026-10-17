# Sessions & turns

Run prompts, observe output, cancel work, and read session state.

<a id="run"></a>

## Run a turn

RunInput contains the user prompt and a Vec<PathBuf> of image paths. The existing workspace and model implementation resolves those images. SessionHandle delegates execution, permission decisions, and persistence to the injected SessionService.

### Rust · shared session interface

```rust
use harness_runtime::{
    CancellationToken, HandleError, Harness, RunInput, TurnOutcome,
};

async fn run_turn(
    harness: &Harness,
    prompt: String,
    cancel: CancellationToken,
) -> Result<TurnOutcome, HandleError> {
    harness.session().run(
        RunInput { prompt, images: Vec::new() },
        cancel,
    ).await
}
```

Verification method: Compile check (harness-run-turn)

### RunInput

```text
pub struct RunInput { pub prompt: String, pub images: Vec<PathBuf> }
```

Cloneable, defaultable input for the existing agent execution path.

### SessionHandle::run

```text
pub async fn run(&self, input: RunInput, cancel: CancellationToken) -> Result<TurnOutcome, HandleError>
```

Execute a turn through the composition’s existing agent loop.

<a id="outcomes"></a>

## Cancellation & outcomes

CancellationToken is re-exported from tokio-util. Keep a clone in the host, pass the token into run, and call cancel() on the host’s clone when the user interrupts. Await the run future so the existing execution path can unwind before releasing its resources.

### TurnOutcome

```text
pub enum TurnOutcome { EndTurn, MaxTurns, Cancelled }
```

Distinguish a natural model stop, the turn budget being reached, and cancellation.

### HandleError

```text
pub enum HandleError { ActionFailed(String), Unimplemented(String) }
```

Coarse host-handle failures. The string carries the human-readable reason.

Cancellation is part of the runtime execution contract. A cancellation outcome is distinct from proof that every external side effect has been rolled back.

<a id="output"></a>

## Observe streamed output

run returns a terminal outcome; streaming output goes to the OutputStream supplied when the runtime is composed. The interface includes text, tool call/result, message boundary, notices, and lifecycle callbacks. Hosts that need client DTOs can use client::adapter::AdapterOutputStream.

### OutputStream::emit_text

```text
async fn emit_text(&self, text: &str, utf16_code_units: Option<&[u16]>)
```

Deliver assistant text to the host’s output destination.

<a id="snapshots"></a>

## Read session snapshots

### SessionHandle::id

```text
pub async fn id(&self) -> SessionId
```

Identity of the currently mounted session.

### SessionHandle::transcript

```text
pub async fn transcript(&self) -> Vec<ConversationMessage>
```

Read the existing ordered conversation transcript.

### SessionHandle::cost

```text
pub async fn cost(&self) -> CostSnapshot
```

Read cumulative usage and cost, including token counts and API duration.

The facade exposes the mounted session. Creating, listing, resuming, or switching sessions belongs to the host composition and client commands.

<a id="session-errors"></a>

## Diagnose a failed turn

1. Record the mounted session ID and host turn_id before admission. Keep model/provider profile and request IDs as separate identities.
2. Consume output as it arrives; terminal outcomes do not reconstruct discarded deltas. Correlate a tool result with its tool_use_id rather than its displayed name.
3. On interruption, cancel the owned token and await run. Preserve partial output and the returned outcome or error before permitting another session operation.
4. For cold resume, load durable session state through the host composition. An ID plus an empty new runtime is not a restored transcript.

Streaming protocol checks include truncated streams and tool-result pairing. Separate tests must cover output delivery, permission response routing and durable publication; TurnOutcome alone establishes none of those barriers.

## Sources and documentation baseline

[Source repository](https://github.com/lingxi-coder/harness-runtime)

- [harness-runtime/crates/runtime/src/api.rs](https://github.com/lingxi-coder/harness-runtime/blob/dc6ce1217143b693d97ab6278ff66568beea20d3/crates/runtime/src/api.rs) — Committed source (dc6ce1217143b693d97ab6278ff66568beea20d3)
- [harness-runtime/crates/runtime/src/desktop/sdk.rs](https://github.com/lingxi-coder/harness-runtime/blob/dc6ce1217143b693d97ab6278ff66568beea20d3/crates/runtime/src/desktop/sdk.rs) — Committed source (dc6ce1217143b693d97ab6278ff66568beea20d3)
- [harness-runtime/crates/core/src/host/orchestrator.rs](https://github.com/lingxi-coder/harness-runtime/blob/dc6ce1217143b693d97ab6278ff66568beea20d3/crates/core/src/host/orchestrator.rs) — Committed source (dc6ce1217143b693d97ab6278ff66568beea20d3)
- [harness-runtime/crates/client/src/adapter/output_stream.rs](https://github.com/lingxi-coder/harness-runtime/blob/dc6ce1217143b693d97ab6278ff66568beea20d3/crates/client/src/adapter/output_stream.rs) — Committed source (dc6ce1217143b693d97ab6278ff66568beea20d3)

- harness-runtime: HEAD dc6ce1217143b693d97ab6278ff66568beea20d3; Referenced input fingerprint sha256:a83d44fb79099d3a923f8fd33bfd18789edd02c2b1d5c683a16c589fffaf2578; Product pin dc6ce1217143b693d97ab6278ff66568beea20d3

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
