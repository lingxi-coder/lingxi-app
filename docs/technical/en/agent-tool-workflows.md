# Add a tool and handle approval

Take a tool from schema and host registration through a real permission round-trip, result presentation and teardown.

<a id="design-a-small-tool"></a>

## Define one operation and its contract

A custom tool is a Rust Tool implementation registered with the host. Start with a narrow operation, canonical name, source-owned input schema and explicit result. Keep validation and authorization separate from the side-effecting call.

| Tool surface | Responsibility |
| --- | --- |
| name / input_schema / input_repair_metadata | Declare canonical identity, accepted shape and required streamed-input repair facts. |
| is_enabled / is_concurrency_safe | Declare capability availability and whether this input may overlap other tool work. |
| validate_input | Check operation-specific requirements before side effects. |
| check_permissions | Compute an input/context-specific result. Being listed is not permission. |
| call / ToolProgressSender | Execute with ToolUseContext, cancellation, progress and a structured result. |

- [Tool contract and signatures](harness-tools.md#tool-contract)

<a id="register-with-owner"></a>

## Register with the lifecycle owner

Register builtins while the host still owns a mutable ToolRegistry. MCP tools belong to McpConnectionId partitions; plugin tools belong to PluginId partitions. Use owner-scoped removal/replacement so disconnected services or disabled plugins cannot leave stale capabilities advertised.

### Rust · register an implemented builtin

```rust
use std::sync::Arc;
use tool_api::{Tool, ToolRegistry};

fn register_host_tool(registry: &mut ToolRegistry, tool: Arc<dyn Tool>) {
    registry.register_builtin(tool);
}
```

Verification method: API excerpt

Registration does not assemble the tool’s host dependencies or bypass session capability filters.

<a id="approval-round-trip"></a>

## Complete an approval round-trip

1. Forward PermissionRequest through PermissionRequestSink and show the real tool/input.
2. Echo request_id on the reply. The transport receive task must remain responsive while the turn waits.
3. Let AdapterPermissionGate::resolve find the parked request; false means unknown or already resolved.
4. Remove the prompt on PermissionRequestResolved, including expired or cancelled outcomes.

### JSON · outbound permission DTO

```json
{
  "request_id": 17,
  "kind": {
    "type": "tool_use_confirm",
    "tool_name": "Bash",
    "tool_input_json": "{\"command\":\"git status --short\"}",
    "default_allow": true
  }
}
```

Verification method: API excerpt

### JSON · approve once

```json
{"type":"approve_permission","request_id":17,"response":{"type":"allow_once"}}
```

Verification method: API excerpt

### JSON · deny

```json
{"type":"deny_permission","request_id":17}
```

Verification method: API excerpt

<a id="allowed-choices"></a>

## Offer only host-approved choices

The adapter defaults to a 300-second request timeout and supports with_timeout. The host can cancel captured owners or drain all pending requests on teardown. A UI default selection is not proof of a user decision.

| Choice/state | Required behavior |
| --- | --- |
| AllowOnce | Approve one parked invocation without creating a durable rule. |
| AllowAlways | Let the gate derive a rule from the captured call. Durable persistence also needs configured paths and enabled persistence. |
| suppress_always_allow_rule | Hide persistent allow; the gate also downgrades unsuitable responses. |
| auto_mode_prompt | Render the engine-provided optional action; do not infer eligibility from the tool name. |
| Timeout / teardown | Resolve pending work through the gate; never leave it parked indefinitely. |

<a id="test-the-call"></a>

## Test dispatch, approval and cleanup

- Advertise and invoke through the real dispatcher; approve once and verify one result with the same tool-use id.
- Reject malformed input and deny a valid call; confirm neither performs side effects.
- Cancel during approval, then answer the old request; it must not revive execution.
- Disconnect MCP or disable the owner plugin and verify removal from the capability view.

- [Tool API and capability filtering](harness-tools.md)
- [Streamed UI events](agent-ui-streaming.md)

## Sources and documentation baseline

[Source repository](https://github.com/lingxi-coder/harness-runtime)

- [harness-runtime/crates/tool-api/src/tool_trait.rs](https://github.com/lingxi-coder/harness-runtime/blob/a39da75f0985d61d90fffa255f7b4d90901b0e27/crates/tool-api/src/tool_trait.rs) — Committed source (a39da75f0985d61d90fffa255f7b4d90901b0e27)
- [harness-runtime/crates/tool-api/src/registry.rs](https://github.com/lingxi-coder/harness-runtime/blob/a39da75f0985d61d90fffa255f7b4d90901b0e27/crates/tool-api/src/registry.rs) — Committed source (a39da75f0985d61d90fffa255f7b4d90901b0e27)
- [harness-runtime/crates/tool-api/src/context.rs](https://github.com/lingxi-coder/harness-runtime/blob/a39da75f0985d61d90fffa255f7b4d90901b0e27/crates/tool-api/src/context.rs) — Committed source (a39da75f0985d61d90fffa255f7b4d90901b0e27)
- [harness-runtime/crates/client/src/adapter/permission_gate.rs](https://github.com/lingxi-coder/harness-runtime/blob/a39da75f0985d61d90fffa255f7b4d90901b0e27/crates/client/src/adapter/permission_gate.rs) — Committed source (a39da75f0985d61d90fffa255f7b4d90901b0e27)
- [harness-runtime/crates/client/src/protocol/permission.rs](https://github.com/lingxi-coder/harness-runtime/blob/a39da75f0985d61d90fffa255f7b4d90901b0e27/crates/client/src/protocol/permission.rs) — Committed source (a39da75f0985d61d90fffa255f7b4d90901b0e27)
- [harness-runtime/crates/client/src/protocol/commands.rs](https://github.com/lingxi-coder/harness-runtime/blob/a39da75f0985d61d90fffa255f7b4d90901b0e27/crates/client/src/protocol/commands.rs) — Committed source (a39da75f0985d61d90fffa255f7b4d90901b0e27)
- [harness-runtime/crates/client/src/protocol/events.rs](https://github.com/lingxi-coder/harness-runtime/blob/a39da75f0985d61d90fffa255f7b4d90901b0e27/crates/client/src/protocol/events.rs) — Committed source (a39da75f0985d61d90fffa255f7b4d90901b0e27)

- harness-runtime: HEAD a39da75f0985d61d90fffa255f7b4d90901b0e27; Referenced input fingerprint sha256:92c2108dc63215bc69ea9c11ea081795da883fdcb1c9c28b060aa9221a1b95a2; Product pin a39da75f0985d61d90fffa255f7b4d90901b0e27

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
