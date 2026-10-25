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
async fn emit_text(&self, text: &Utf16JsonProjection)
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

<a id="run-parameters"></a>

## Run input and return values

| Value | Meaning | Integration rule |
| --- | --- | --- |
| RunInput.prompt: String | User text passed to the mounted execution path. | Supply the actual user request; system instructions belong to host configuration. |
| RunInput.images: Vec<PathBuf> | Image paths resolved by the workspace/model implementation. | Keep the files readable on the execution host while the request is in use. |
| CancellationToken | A shared cancellation signal, not a timeout duration. | Keep a clone for the stop button or host deadline; await run after cancelling. |
| Result<TurnOutcome, HandleError> | A terminal execution result, rather than streamed assistant text. | Read output from OutputStream and branch separately on outcome versus error. |

Cloning SessionHandle shares the mounted service. It does not create another session, fork history, or grant concurrent turn admission; the composition owns those decisions.

<a id="cancellation-recipe"></a>

## Connect a stop button without losing the result

Give the caller a token clone before starting this function. The caller can cancel that clone from its UI or deadline handler. This function waits for cleanup in the existing run path and returns the original outcome, including a normal completion that won a race with cancellation.

- EndTurn means the execution path ended naturally; MaxTurns means its turn budget ended; Cancelled is an explicit cancellation outcome.
- An error or cancellation does not undo a tool’s completed file write, command, or external request. Inspect the operation before retrying it.
- Keep partial text as partial text until the host receives the final message or error boundary. A disconnected UI is not proof that the runtime stopped.

### Rust · complete function, assembled host required

```rust
use harness_runtime::{CancellationToken, HandleError, Harness, RunInput, TurnOutcome};

async fn submit(
    harness: &Harness,
    prompt: String,
    cancel: CancellationToken,
) -> Result<TurnOutcome, HandleError> {
    let session = harness.session();
    let session_id = session.id().await;
    let result = session.run(RunInput { prompt, images: vec![] }, cancel).await;
    let cost = session.cost().await;
    println!("session={session_id:?}, api_calls={}", cost.api_calls);
    result
}

// Before submit: let cancel = CancellationToken::new();
// Keep cancel.clone() in the host; call cancel.cancel() when the user stops.
```

Verification method: Compile check (harness-cancellable-turn)

<a id="stream-rendering"></a>

## Render a stream and reconcile the final message

1. Append text deltas to the active assistant presentation. Preserve source text through the supplied UTF-16 JSON projection instead of round-tripping arbitrary external payloads through lossy strings.
2. Create tool activity rows from the tool call identity. Update the same row when its matching result arrives; multiple calls can use the same tool name.
3. Use final message boundaries or MessageComplete DTOs to settle the assistant row. Do not append a second copy of the same completed message after already rendering its deltas.
4. Read cost() when you need cumulative accounting. A transcript snapshot, a text delta and a cost update serve different purposes.

- [Event fields and permission response routing](harness-client.md#event-consumer)

## Sources and documentation baseline

[Source repository](https://github.com/lingxi-coder/harness-runtime)

- [harness-runtime/crates/runtime/src/api.rs](https://github.com/lingxi-coder/harness-runtime/blob/0e28910fd624638e6c5d1f92eca4899a7938d188/crates/runtime/src/api.rs) — Committed source (0e28910fd624638e6c5d1f92eca4899a7938d188)
- [harness-runtime/crates/runtime/src/desktop/sdk.rs](https://github.com/lingxi-coder/harness-runtime/blob/0e28910fd624638e6c5d1f92eca4899a7938d188/crates/runtime/src/desktop/sdk.rs) — Committed source (0e28910fd624638e6c5d1f92eca4899a7938d188)
- [harness-runtime/crates/core/src/host/orchestrator.rs](https://github.com/lingxi-coder/harness-runtime/blob/0e28910fd624638e6c5d1f92eca4899a7938d188/crates/core/src/host/orchestrator.rs) — Committed source (0e28910fd624638e6c5d1f92eca4899a7938d188)
- [harness-runtime/crates/client/src/adapter/output_stream.rs](https://github.com/lingxi-coder/harness-runtime/blob/0e28910fd624638e6c5d1f92eca4899a7938d188/crates/client/src/adapter/output_stream.rs) — Committed source (0e28910fd624638e6c5d1f92eca4899a7938d188)

- harness-runtime: HEAD 0e28910fd624638e6c5d1f92eca4899a7938d188; Referenced input fingerprint sha256:b6b36966a8f4c48b8d0813ea03f73cd617387ddaf2531d8254e4a5ac81f48255; Product pin 0e28910fd624638e6c5d1f92eca4899a7938d188

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
