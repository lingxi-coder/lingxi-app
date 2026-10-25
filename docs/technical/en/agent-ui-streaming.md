# Connect streamed output to a UI

Wire one event destination and render text, tool activity and interactive requests without confusing message completion with turn completion.

<a id="wire-before-run"></a>

## Install output before the first run

Harness run does not return a stream iterator. The host supplies OutputStream during composition. For a DTO-based UI, implement ClientEventSink and wrap it with AdapterOutputStream::new. Connect the turn emitter and terminal permission notifications to the same event destination.

### ClientEventSink::emit

```text
async fn emit(&self, event: client::protocol::events::ClientEvent)
```

Receives one owned, lowered event. Enqueue transport work; avoid rendering or blocking network I/O on the agent task.

- [Adapter setup](harness-client.md#output-adapter)

<a id="project-events"></a>

## Project events into stable rows

AdapterOutputStream can emit message boundaries during execution; TurnEventEmitter can also synthesize a complete message snapshot after the pumped turn succeeds. Do not assume every MessageComplete means the run is finished, or that the final snapshot is a new independent assistant answer. Reconcile message identity/snapshots using the host contract.

| Event | UI update | Important distinction |
| --- | --- | --- |
| TurnStarted | Mark the admitted action active; retain its optional turn_id. | Correlate it with the submitted user action. |
| TextDelta | Append to the current assistant message and batch view updates. | A delta is not a complete message. |
| ToolUseStarted / ToolHeartbeat | Create/update activity using the tool-use id. | input_json is a JSON string, not an already parsed object. |
| ToolUseResult | Attach result_json, is_error and display to the matching id. | A failed tool need not terminate the entire turn. |
| MessageComplete | Finish one message; apply the optional full message snapshot. | The agent can continue another model/tool iteration. |
| TurnEnded | Apply terminal outcome, stop reason and cost. | Preserve partial output on cancellation or budget exhaustion. |
| Error | Show its typed kind and message and reconcile the failed interaction. | The turn wrapper emits Error on failure; do not wait forever for a success-only marker. |

<a id="wire-shape"></a>

## Read the wire shape correctly

These are event objects, not a request batch, transport frame or complete turn. Client enums are non-exhaustive and use snake_case type tags. Keep a fallback path for additive events; native bindings must match the host’s protocol layout.

### JSON · independent event objects

```json
{"type":"turn_started","turn_id":12}
{"type":"text_delta","text":"I will inspect the project."}
{"type":"message_complete","stop_reason":"tool_use"}
```

Verification method: API excerpt

<a id="interactive-output"></a>

## Keep approvals and questions interactive

A tool permission request is a dedicated PermissionRequest DTO delivered through PermissionRequestSink, not a PermissionRequest variant of ClientEvent. When configured, the gate emits PermissionRequestResolved via the event sink. AskUserQuestion has its own event and answer command. Render each interaction by request_id and owner rather than appending ordinary assistant text.

1. Display the request without completing the turn.
2. Send the matching decision or answer over the active host connection.
3. Remove the request on authoritative resolution, including expiry or cancellation before the click.

- [Approval round-trip](agent-tool-workflows.md#approval-round-trip)

<a id="reconnect-and-rebuild"></a>

## Restore a snapshot before live projection

A new output adapter does not replay earlier deltas. Rebuild conversation state through the host’s resume/snapshot path, then apply events for the mounted session. Keep session ids, turn ids and tool-use ids separate. If the transport has epoch/sequence metadata, follow its replay rules rather than deduplicating equal text.

- Do not replace the transcript with only the latest delta after reconnect.
- If the UI disappears, notify its lifecycle owner; do not let an unconsumed queue grow indefinitely.
- Exercise tools, pending approvals, transport failure and normal turn completion; text alone misses these transitions.

- [Session recovery](agent-session-workflows.md)

## Sources and documentation baseline

[Source repository](https://github.com/lingxi-coder/harness-runtime)

- [harness-runtime/crates/runtime/src/api.rs](https://github.com/lingxi-coder/harness-runtime/blob/0e28910fd624638e6c5d1f92eca4899a7938d188/crates/runtime/src/api.rs) — Committed source (0e28910fd624638e6c5d1f92eca4899a7938d188)
- [harness-runtime/crates/client/src/adapter/sink.rs](https://github.com/lingxi-coder/harness-runtime/blob/0e28910fd624638e6c5d1f92eca4899a7938d188/crates/client/src/adapter/sink.rs) — Committed source (0e28910fd624638e6c5d1f92eca4899a7938d188)
- [harness-runtime/crates/client/src/adapter/output_stream.rs](https://github.com/lingxi-coder/harness-runtime/blob/0e28910fd624638e6c5d1f92eca4899a7938d188/crates/client/src/adapter/output_stream.rs) — Committed source (0e28910fd624638e6c5d1f92eca4899a7938d188)
- [harness-runtime/crates/client/src/adapter/permission_gate.rs](https://github.com/lingxi-coder/harness-runtime/blob/0e28910fd624638e6c5d1f92eca4899a7938d188/crates/client/src/adapter/permission_gate.rs) — Committed source (0e28910fd624638e6c5d1f92eca4899a7938d188)
- [harness-runtime/crates/client/src/adapter/turn.rs](https://github.com/lingxi-coder/harness-runtime/blob/0e28910fd624638e6c5d1f92eca4899a7938d188/crates/client/src/adapter/turn.rs) — Committed source (0e28910fd624638e6c5d1f92eca4899a7938d188)
- [harness-runtime/crates/client/src/protocol/events.rs](https://github.com/lingxi-coder/harness-runtime/blob/0e28910fd624638e6c5d1f92eca4899a7938d188/crates/client/src/protocol/events.rs) — Committed source (0e28910fd624638e6c5d1f92eca4899a7938d188)
- [harness-runtime/crates/client/src/protocol/permission.rs](https://github.com/lingxi-coder/harness-runtime/blob/0e28910fd624638e6c5d1f92eca4899a7938d188/crates/client/src/protocol/permission.rs) — Committed source (0e28910fd624638e6c5d1f92eca4899a7938d188)

- harness-runtime: HEAD 0e28910fd624638e6c5d1f92eca4899a7938d188; Referenced input fingerprint sha256:b6b36966a8f4c48b8d0813ea03f73cd617387ddaf2531d8254e4a5ac81f48255; Product pin 0e28910fd624638e6c5d1f92eca4899a7938d188

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
