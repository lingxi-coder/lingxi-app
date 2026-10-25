# Continue, resume, fork and cancel

Choose the right session operation and distinguish cancellation, saved history and active execution ownership.

<a id="choose-operation"></a>

## Match the operation to user intent

| Intent | Operation | Identity behavior |
| --- | --- | --- |
| Ask another question now | Run again on the mounted SessionHandle | Keep the conversation; wait for the preceding turn. |
| Start an independent conversation | NewSession | The host creates/mounts a session and emits SessionStarted. |
| Open saved history | ResumeSession | Name a target; SessionResumed confirms the mounted id and messages. |
| Explore another branch | ForkSession on MobileHost | MobileHost creates a new branch; creation alone does not mount it. Desktop Bridge does not currently handle this command. |
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

This recipe follows the current MobileHost implementation. Desktop Bridge does not route ForkSession, so a desktop client must not offer this command or wait for SessionForked until its host implements it. The shared protocol type alone does not establish support.

ForkSession accepts SessionModeDto chat or code and keeps the current session’s workspace according to its command contract. Treat its result as a new conversation identity. A source id in a lifecycle command does not change the rule that normal live commands target the mounted connection.

- Retain the source entry until the host confirms the new branch.
- Do not fork by editing saved JSONL in place or reusing the source id.

1. On MobileHost, send ForkSession and wait for SessionForked; retain the returned new branch id.
2. Mount that id with ResumeSession, or open the host connection for that branch. Wait for SessionResumed or the host’s authoritative mounted-session confirmation.
3. Only then send a follow-up prompt. Sending a prompt immediately after creation can still target the original mounted session.

### JSON · MobileHost fork into code mode

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

Verification method: Environment validation (harness-guide-cooperative-cancellation)

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

- [harness-runtime/crates/runtime/src/api.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/runtime/src/api.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/client/src/protocol/commands.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/client/src/protocol/commands.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/client/src/protocol/events.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/client/src/protocol/events.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/client/src/protocol/listings.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/client/src/protocol/listings.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/runtime/src/desktop/sdk.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/runtime/src/desktop/sdk.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/runtime/src/mobile/host.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/runtime/src/mobile/host.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/session/src/jsonl/loader.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/session/src/jsonl/loader.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [lingxi-app/apps/bridge-server/src/server.rs](https://github.com/lingxi-coder/lingxi-app/blob/caf679253971034bc202c566a81c10d4c5af31c4/apps/bridge-server/src/server.rs) — Committed source (caf679253971034bc202c566a81c10d4c5af31c4)
- [lingxi-app/apps/bridge-server/src/router.rs](https://github.com/lingxi-coder/lingxi-app/blob/caf679253971034bc202c566a81c10d4c5af31c4/apps/bridge-server/src/router.rs) — Committed source (caf679253971034bc202c566a81c10d4c5af31c4)

- lingxi-app: HEAD caf679253971034bc202c566a81c10d4c5af31c4; Referenced input fingerprint sha256:5172e409e969106eb5149a400160fbc916294195f79f186ce9f63e32444c54b5
- harness-runtime: HEAD f975ab5804bb3af0e72beb3974e5c911ece44d53; Referenced input fingerprint sha256:92c2108dc63215bc69ea9c11ea081795da883fdcb1c9c28b060aa9221a1b95a2; Product pin e04c1837508014d68ea042d5c9fe3c8c15af5aa0

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
