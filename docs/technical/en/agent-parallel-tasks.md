# Observe parallel agents and tasks

Give independent work clear ownership and build progress/cancellation controls around authoritative agent and task state.

<a id="split-by-owner"></a>

## Split independent work with clear owners

Parallel work is useful when subtasks can progress independently, such as an API review and a UI review. Give each worker an input, owned output and completion condition. Shared files, registries and external side effects still need coordination.

| Identifier | Meaning and use |
| --- | --- |
| session_id | Conversation/workspace owner; scopes history and client views. |
| agent_id | Agent instance and its own transcript; use host-returned ids for inspection. |
| task_id | Long-lived task state/output owner; use it for output reads and stop requests. |
| turn_id | Optional client correlator for one admitted foreground action and targeted cancellation. |

<a id="admission-before-execution"></a>

## Treat capacity as admission

StateMachinePool has an explicit max_concurrent capacity. Ordinary slot admission fails fast when physical capacity is full; group admission reserves all requested slots or none. Distinguish capacity refusal from provider timeout and tool failure before choosing a retry policy.

- Keep cancellation responsive during admission and execution.
- Do not hold a partial group allocation while waiting for the remaining slots.
- A cloned session handle is not a new worker; use the host’s actual subagent/task composition.

- [Agent pool and ownership](agents-background.md)

<a id="background-owner"></a>

## Use a real background-task owner

Desktop’s background-agent decorator connects detached Agent work to LocalAgent task state, output and mailbox handling. A bare pool spawner is not a replacement for that durable composition. Closing a view, spawning a Tokio future or retaining an in-memory agent record does not establish restart-safe background execution.

1. Choose a host with task handlers and background admission wired.
2. Retain the admitted task id and owner; read status and output through that owner.
3. Distinguish mailbox delivery from the recipient actually consuming a message.
4. After restart, verify restored ownership and execution state before offering Resume.

<a id="inspect-progress"></a>

## Build a monitor using the protocol

Desktop Bridge returns TaskRow events and, when request_id is supplied, an authoritative TaskListComplete receipt carrying that id. Current MobileHost ignores request_id and emits only TaskRow events; it has no task-list completion receipt. Keep receipt-based snapshot reconciliation specific to a host that implements it, and do not wait for TaskListComplete on MobileHost.

TaskOutput returns TaskOutputChunk with content, total_lines and truncated. Follow the host’s offset contract for later reads; do not derive an offset from JavaScript string length or assume a truncated result delivered every line. Agent listings/transcripts differ from task output and are scoped to the mounted session.

### JSON · independent monitoring commands

```json
{"type":"task_list","request_id":"tasks-panel-1"}
{"type":"task_output","task_id":"<task-id>","offset":0}
{"type":"list_session_agents"}
{"type":"load_session_agent_transcript","agent_id":"<agent-id>"}
```

Verification method: API excerpt

Use ids returned by the host. Placeholder ids are not executable identities. An empty output chunk or partial transcript is not a completion signal; inspect task/agent state.

<a id="stop-and-collect"></a>

## Stop, then collect terminal state

1. Send TaskStop for the selected host-returned id, not an unrelated foreground turn.
2. Keep the row visible as stopping/pending until the host reports a terminal state.
3. Retain partial output, failure reason and durable completion/publication receipts.
4. Release observers and task/workspace resources through the lifecycle owner.

### JSON · stop the selected task

```json
{"type":"task_stop","task_id":"<task-id>"}
```

Verification method: API excerpt

- [Background execution and recovery](agents-background.md)
- [Validation boundaries](validation-evidence.md)

The existence of collaboration/task types does not prove desktop, mobile or cross-device execution parity. Check actual host wiring for the platform you ship.

## Sources and documentation baseline

[Source repository](https://github.com/lingxi-coder/harness-runtime)

- [harness-runtime/crates/agent/src/pool.rs](https://github.com/lingxi-coder/harness-runtime/blob/a39da75f0985d61d90fffa255f7b4d90901b0e27/crates/agent/src/pool.rs) — Committed source (a39da75f0985d61d90fffa255f7b4d90901b0e27)
- [harness-runtime/crates/agent/src/pool/capacity.rs](https://github.com/lingxi-coder/harness-runtime/blob/a39da75f0985d61d90fffa255f7b4d90901b0e27/crates/agent/src/pool/capacity.rs) — Committed source (a39da75f0985d61d90fffa255f7b4d90901b0e27)
- [harness-runtime/crates/agent/src/handle.rs](https://github.com/lingxi-coder/harness-runtime/blob/a39da75f0985d61d90fffa255f7b4d90901b0e27/crates/agent/src/handle.rs) — Committed source (a39da75f0985d61d90fffa255f7b4d90901b0e27)
- [harness-runtime/crates/runtime/src/desktop/background_agent.rs](https://github.com/lingxi-coder/harness-runtime/blob/a39da75f0985d61d90fffa255f7b4d90901b0e27/crates/runtime/src/desktop/background_agent.rs) — Committed source (a39da75f0985d61d90fffa255f7b4d90901b0e27)
- [harness-runtime/crates/runtime/src/mobile/host.rs](https://github.com/lingxi-coder/harness-runtime/blob/a39da75f0985d61d90fffa255f7b4d90901b0e27/crates/runtime/src/mobile/host.rs) — Committed source (a39da75f0985d61d90fffa255f7b4d90901b0e27)
- [harness-runtime/crates/tasks/src/registry.rs](https://github.com/lingxi-coder/harness-runtime/blob/a39da75f0985d61d90fffa255f7b4d90901b0e27/crates/tasks/src/registry.rs) — Committed source (a39da75f0985d61d90fffa255f7b4d90901b0e27)
- [harness-runtime/crates/tasks/src/state.rs](https://github.com/lingxi-coder/harness-runtime/blob/a39da75f0985d61d90fffa255f7b4d90901b0e27/crates/tasks/src/state.rs) — Committed source (a39da75f0985d61d90fffa255f7b4d90901b0e27)
- [harness-runtime/crates/client/src/protocol/commands.rs](https://github.com/lingxi-coder/harness-runtime/blob/a39da75f0985d61d90fffa255f7b4d90901b0e27/crates/client/src/protocol/commands.rs) — Committed source (a39da75f0985d61d90fffa255f7b4d90901b0e27)
- [harness-runtime/crates/client/src/protocol/events.rs](https://github.com/lingxi-coder/harness-runtime/blob/a39da75f0985d61d90fffa255f7b4d90901b0e27/crates/client/src/protocol/events.rs) — Committed source (a39da75f0985d61d90fffa255f7b4d90901b0e27)
- [harness-runtime/crates/client/src/protocol/listings.rs](https://github.com/lingxi-coder/harness-runtime/blob/a39da75f0985d61d90fffa255f7b4d90901b0e27/crates/client/src/protocol/listings.rs) — Committed source (a39da75f0985d61d90fffa255f7b4d90901b0e27)
- [lingxi-app/apps/bridge-server/src/router.rs](https://github.com/lingxi-coder/lingxi-app/blob/abad4d9603ce6a0c032ddf089f69f88e927b76c4/apps/bridge-server/src/router.rs) — Committed source (abad4d9603ce6a0c032ddf089f69f88e927b76c4)
- [lingxi-app/apps/bridge-server/src/router/tasks.rs](https://github.com/lingxi-coder/lingxi-app/blob/abad4d9603ce6a0c032ddf089f69f88e927b76c4/apps/bridge-server/src/router/tasks.rs) — Committed source (abad4d9603ce6a0c032ddf089f69f88e927b76c4)

- lingxi-app: HEAD abad4d9603ce6a0c032ddf089f69f88e927b76c4; Referenced input fingerprint sha256:bbb3c48b6b5761f33d3d66eb68ccaf089f9fd5ffc104138cbe084fb24e3b4f5b
- harness-runtime: HEAD a39da75f0985d61d90fffa255f7b4d90901b0e27; Referenced input fingerprint sha256:92c2108dc63215bc69ea9c11ea081795da883fdcb1c9c28b060aa9221a1b95a2; Product pin a39da75f0985d61d90fffa255f7b4d90901b0e27

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
