# Continue, resume, fork and cancel

Choose the right session operation and distinguish cancellation, saved history and active execution ownership.

<a id="choose-operation"></a>

## Match the operation to user intent

| Intent | Operation | Identity behavior |
| --- | --- | --- |
| Ask another question now | Run again on the mounted SessionHandle | Keep the conversation; wait for the preceding turn. |
| Start an independent conversation | NewSession | The host creates/mounts a session and emits SessionStarted. |
| Open saved history | ResumeSession | Name a target; SessionResumed confirms the mounted id and messages. |
| Explore another branch | ForkSession | The host creates a new branch; creation alone does not mount it. |
| Stop the current work only | CancellationToken / Cancel | End a turn without deleting history or shutting down the host. |

<a id="resume-over-the-protocol"></a>

## Resume through the host protocol

1. List sessions and let the user choose a returned session id.
2. Send ResumeSession and wait for SessionResumed before submitting follow-up work.
3. Rebuild messages from the resumed snapshot and retain its authoritative identity.
4. SendPrompt targets the mounted connection. Do not add session_id to ordinary live prompt/cancel commands.

### JSON · send commands in this order

```json
{"type":"list_sessions","limit":20}
{"type":"resume_session","session_id":"<saved-session-id>"}
{"type":"send_prompt","text":"Continue the investigation","images":[],"turn_id":21}
```

Verification method: API excerpt

The objects illustrate ordering, not batching. The public Harness facade has no resume(session_id) method; the host owns mounting and persistence.

<a id="fork-a-conversation"></a>

## Fork without overwriting the source

ForkSession accepts SessionModeDto chat or code and keeps the current session’s workspace according to its command contract. Treat its result as a new conversation identity. A source id in a lifecycle command does not change the rule that normal live commands target the mounted connection.

- Retain the source entry until the host confirms the new branch.
- Do not fork by editing saved JSONL in place or reusing the source id.

1. Send ForkSession and wait for SessionForked; retain the returned new branch id.
2. Mount that id with ResumeSession, or open the host connection for that branch. Wait for SessionResumed or the host’s authoritative mounted-session confirmation.
3. Only then send a follow-up prompt. Sending a prompt immediately after creation can still target the original mounted session.

### JSON · fork into code mode

```json
{
  "type": "fork_session",
  "session_id": "<source-session-id>",
  "target_mode": "code"
}
```

Verification method: API excerpt

<a id="cooperative-cancellation"></a>

## Cancel cooperatively and await the run

Create a fresh turn token for the next run: a cancelled token remains cancelled. Over the protocol, include turn_id when cancellation must target the same user action rather than a later active turn. A terminal cancellation outcome does not roll back previously executed tools.

### Rust · connect a stop signal to one run

```rust
use harness_runtime::{CancellationToken, HandleError, Harness, RunInput, TurnOutcome};

async fn run_until_stopped(
    harness: &Harness,
    prompt: String,
    stop: CancellationToken,
) -> Result<TurnOutcome, HandleError> {
    let turn_cancel = CancellationToken::new();
    let session = harness.session();
    let run = session.run(
        RunInput { prompt, images: Vec::new() },
        turn_cancel.clone(),
    );
    tokio::pin!(run);
    tokio::select! {
        result = &mut run => result,
        () = stop.cancelled() => {
            turn_cancel.cancel();
            run.await
        }
    }
}
```

Verification method: Compile check (harness-guide-cooperative-cancellation)

<a id="saved-is-not-running"></a>

## Saved history is not an execution owner

| Observation | What it proves | Next check |
| --- | --- | --- |
| Session appears in a listing | A resumable record exists. | Mount it and inspect SessionResumed. |
| Partial text was saved | Some output was retained. | Inspect terminal state and active/recovered owner. |
| Cancellation was requested | A stop signal reached the host. | Await terminal completion and tool/permission cleanup. |
| ShutdownReport.complete is false | A required barrier is still incomplete. | Retain errors/publications; retry sequentially. |

- [Storage and durable publication](session-storage.md)
- [Host shutdown](harness-lifecycle.md#shutdown)

## Sources and documentation baseline

[Source repository](https://github.com/lingxi-coder/harness-runtime)

- [harness-runtime/crates/runtime/src/api.rs](https://github.com/lingxi-coder/harness-runtime/blob/0e28910fd624638e6c5d1f92eca4899a7938d188/crates/runtime/src/api.rs) — Committed source (0e28910fd624638e6c5d1f92eca4899a7938d188)
- [harness-runtime/crates/client/src/protocol/commands.rs](https://github.com/lingxi-coder/harness-runtime/blob/0e28910fd624638e6c5d1f92eca4899a7938d188/crates/client/src/protocol/commands.rs) — Committed source (0e28910fd624638e6c5d1f92eca4899a7938d188)
- [harness-runtime/crates/client/src/protocol/events.rs](https://github.com/lingxi-coder/harness-runtime/blob/0e28910fd624638e6c5d1f92eca4899a7938d188/crates/client/src/protocol/events.rs) — Committed source (0e28910fd624638e6c5d1f92eca4899a7938d188)
- [harness-runtime/crates/client/src/protocol/listings.rs](https://github.com/lingxi-coder/harness-runtime/blob/0e28910fd624638e6c5d1f92eca4899a7938d188/crates/client/src/protocol/listings.rs) — Committed source (0e28910fd624638e6c5d1f92eca4899a7938d188)
- [harness-runtime/crates/runtime/src/desktop/sdk.rs](https://github.com/lingxi-coder/harness-runtime/blob/0e28910fd624638e6c5d1f92eca4899a7938d188/crates/runtime/src/desktop/sdk.rs) — Committed source (0e28910fd624638e6c5d1f92eca4899a7938d188)
- [harness-runtime/crates/session/src/jsonl/loader.rs](https://github.com/lingxi-coder/harness-runtime/blob/0e28910fd624638e6c5d1f92eca4899a7938d188/crates/session/src/jsonl/loader.rs) — Committed source (0e28910fd624638e6c5d1f92eca4899a7938d188)

- harness-runtime: HEAD 0e28910fd624638e6c5d1f92eca4899a7938d188; Referenced input fingerprint sha256:b6b36966a8f4c48b8d0813ea03f73cd617387ddaf2531d8254e4a5ac81f48255; Product pin 0e28910fd624638e6c5d1f92eca4899a7938d188

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
