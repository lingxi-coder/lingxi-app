# Tools & permissions

Implement tools using the shared contract and register their capabilities.

<a id="tool-contract"></a>

## The Tool contract

tool-api defines the abstract contract used by tool implementations and execution components. It exposes Tool, ToolUseContext, ToolRegistry, ToolCallResult, validation errors, progress channels, and dispatch support. These are companion crate exports, rather than methods on Harness.

A tool advertises its canonical name, JSON input schema, enablement, result size, and scheduling hints. Before execution, the dispatcher validates input and computes permissions. Hooks may affect that permission decision. Implementing call alone is not the complete Tool implementation.

### Tool::name

```text
fn name(&self) -> &str
```

Canonical dispatcher identity for the tool.

### Tool::input_schema

```text
fn input_schema(&self) -> &Utf16JsonProjection
```

Advertise the tool’s input JSON Schema.

### Tool::validate_input

```text
async fn validate_input(&self, input: &Utf16JsonProjection, ctx: &ToolUseContext) -> Result<(), ValidationError>
```

Validate tool-specific conditions before side effects; the default implementation succeeds.

### Tool::check_permissions

```text
async fn check_permissions(&self, input: &Utf16JsonProjection, ctx: &ToolUseContext) -> permission::PermissionResult
```

Compute the invocation-specific permission decision before dispatch.

### Tool::call

```text
async fn call(&self, input: Utf16JsonProjection, ctx: ToolUseContext, progress_tx: ToolProgressSender) -> Result<ToolCallResult, ToolError>
```

Execute validated, permission-checked input and return the model-facing result.

<a id="registry"></a>

## Register tools before sharing

ToolRegistry stores builtin, MCP, LSP, and plugin partitions. Share it with Arc instead of cloning the registry. Builtins are registered through &mut self; MCP registration uses interior synchronization so a server can refresh its tools after the registry is shared.

### Rust · existing Tool implementation required

```rust
use std::sync::Arc;
use tool_api::{Tool, ToolRegistry};

fn registry_with(tool: Arc<dyn Tool>) -> Arc<ToolRegistry> {
    let mut registry = ToolRegistry::new();
    registry.register_builtin(tool);
    Arc::new(registry)
}
```

Verification method: Compile check (harness-tool-registry)

### ToolRegistry::register_builtin

```text
pub fn register_builtin(&mut self, tool: Arc<dyn Tool>)
```

Add a builtin before sharing the registry.

### ToolRegistry::available_tools

```text
pub fn available_tools(&self, ctx: &ToolStaticContext) -> Vec<Arc<dyn Tool>>
```

Read the enabled, filtered tool pool in its deterministic advertised order.

### ToolRegistry::register_mcp_tools

```text
pub fn register_mcp_tools(&self, conn_id: McpConnectionId, tools: Vec<Arc<dyn Tool>>)
```

Register or refresh the tools attached to an MCP connection.

### ToolRegistry::unregister_mcp_tools

```text
pub fn unregister_mcp_tools(&self, conn_id: McpConnectionId)
```

Remove the tool partition for a disconnected MCP connection.

<a id="invocation-context"></a>

## Invocation context & progress

ToolUseContext carries invocation options, conversation history, tool-use and assistant-message identities, optional session state, origin-session attribution, and an optional per-call cancellation token. Respect the supplied cancellation and host capabilities when implementing operations.

### progress_channel

```text
pub fn progress_channel() -> (ToolProgressSender, ToolProgressReceiver)
```

Create a bounded 64-slot channel for tool-defined progress JSON.

### ToolProgress

```text
pub struct ToolProgress { pub tool_use_id: lingxi_core::types::ToolUseId, pub data: Utf16JsonProjection }
```

Correlate one progress payload with its originating tool use.

<a id="capability-filtering"></a>

## Filter session capabilities

### ToolRegistry::set_session_tool_allowlist

```text
pub fn set_session_tool_allowlist(&mut self, tools: &[String])
```

Set an explicit builtin capability allowlist and hide all dynamic partitions. Canonical builtin names govern the gate; aliases do not bypass it.

Tool visibility and per-call permission checks are separate controls. Desktop embedded hosts inject PermissionGate during assembly; client transports can resolve interactive requests through AdapterPermissionGate.

<a id="dispatch-failures"></a>

## Validation, denial and failed side effects

The orchestrator resolves canonical identity, validates/coerces input, runs tool policy and hooks, obtains the permission decision, and invokes the tool through tracked dispatch. Do not call a Tool body directly from a provider adapter: that would bypass host authority and the invocation journal.

Schema rejection, permission denial, call failure and interruption are different outcomes. Use ToolError for failed execution and keep observable input redacted through backfill_observable_input; ToolCallResult supplies the model-facing payload. A failed call may have already changed an external system, so retry policy must belong to the operation.

### Existing registry and tool-contract tests

```bash
cargo test --locked -p tool-api --lib
cargo test --locked -p test-harness --test tools_hooks_e2e
```

Verification method: Environment validation

- [Extension lifecycle](extensions-lifecycle.md)

These commands are reproducible checks, not a claim that every registered tool is safe on every device. Device input, shell isolation and external service acceptance are checked in their host.

<a id="custom-tool-checklist"></a>

## Implement a useful custom tool

A custom tool needs a complete Tool implementation and a host composition that registers it. Begin with a small operation whose input, result and side effects you can explain to both the model and the user.

| Implement | Contract | Common mistake |
| --- | --- | --- |
| name, input_schema and input_repair_metadata | A canonical identity, source-preserving schema and explicit input-repair facts. | Registering aliases as separate capabilities or advertising an unknown repair contract. |
| is_enabled, is_read_only and is_concurrency_safe | Visibility and scheduling facts for this actual invocation. | Marking a mutating operation read-only to avoid approval or serialization. |
| description, prompt and max_result_size_chars | Model-facing guidance and an explicit result-size policy. | Returning unbounded raw output without considering the model-facing result. |
| validate_input and check_permissions | Tool-specific validation and a decision before side effects. | Starting network/filesystem work during validation or permission checks. |
| call and ToolCallResult | Execution under the dispatcher with an explicit model result. | Calling the tool body directly from a transport or provider adapter. |

Current tool inputs and schemas use lingxi_core::types::utf16_json::Utf16JsonProjection. JSON is still the data format, but arbitrary external schemas and strings should retain their source projection instead of being treated as ordinary serde_json::Value.

<a id="tool-results-and-errors"></a>

## Return results that the model can act on

ToolCallResult separates data, an optional faithful model-facing string and error state. The dispatcher can derive model text from data, or use model_content when supplied. MCP error results can be returned as accepted results with is_error set; a transport failure remains a failed call.

| Failure | Meaning | Handle it by |
| --- | --- | --- |
| InvalidInput / ValidationError | The request does not satisfy the tool contract. | Return actionable validation detail; ask for corrected input before retrying. |
| PermissionDenied | Policy or the user refused this invocation. | Surface the denial and let the agent choose an allowed alternative. |
| InteractionRequired | This host cannot provide the required live interaction channel. | Supply the intended host interaction or choose a suitable noninteractive operation. |
| Aborted / Io | Execution was interrupted or the underlying operation failed. | Keep partial state and check idempotency before reissuing a side-effecting operation. |
| EditWithoutRead | The session has no accepted read state for the edit target. | Read the current target in the mounted session, then construct a fresh edit. |

### Rust · result construction excerpt

```rust
use tool_api::{source_text_projection, ToolCallResult};

fn completed_summary() -> ToolCallResult {
    ToolCallResult::from_data(source_text_projection("Saved the report."))
}
```

Verification method: API excerpt

<a id="hooks-and-mcp"></a>

## Add hooks and MCP without bypassing dispatch

Use hooks to observe or control lifecycle boundaries, and MCP to add tools owned by an external server connection. These are different extension mechanisms: a hook does not replace the tool registry, and connecting a server does not authorize all its tool calls.

| Need | Source-backed seam | Lifecycle rule |
| --- | --- | --- |
| Validate or annotate a tool invocation | HookEventType::PreToolUse / PostToolUse / PostToolUseFailure | Choose the correct event and respect its control result; do not run the side effect from an observation hook. |
| Update user/project hooks | HookRegistry::replace_source_hooks | Replace one source bucket after validation; retain plugin and session hook ownership. |
| Add server tools | McpRegistry::connect and ToolRegistry::register_mcp_tools | Retain connection identity for refresh/disconnect. Let the production composition coordinate connection and tool registration. |
| Remove a server | ToolRegistry::unregister_mcp_tools | Remove only that connection partition and complete the owned transport teardown. |

- [Hook and extension lifecycle](extensions-lifecycle.md)

HookDefinition includes blocking, timeout, priority and source. Blocking hooks are awaited; nonblocking hooks use the asynchronous registry. A timeout or failed hook is not a successful policy decision. Configure failure behavior at the supported event boundary.

## Sources and documentation baseline

[Source repository](https://github.com/lingxi-coder/harness-runtime)

- [harness-runtime/crates/tool-api/src/tool_trait.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/tool-api/src/tool_trait.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/tool-api/src/context.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/tool-api/src/context.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/tool-api/src/registry.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/tool-api/src/registry.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/tool-api/src/progress.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/tool-api/src/progress.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/orchestrator/src/turn_loop/tool_dispatch.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/orchestrator/src/turn_loop/tool_dispatch.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/core/src/host/permission_gate.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/core/src/host/permission_gate.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/hooks/src/definition.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/hooks/src/definition.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/hooks/src/events.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/hooks/src/events.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/hooks/src/registry.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/hooks/src/registry.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/mcp/src/registry.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/mcp/src/registry.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)

- harness-runtime: HEAD f975ab5804bb3af0e72beb3974e5c911ece44d53; Referenced input fingerprint sha256:92c2108dc63215bc69ea9c11ea081795da883fdcb1c9c28b060aa9221a1b95a2; Product pin e04c1837508014d68ea042d5c9fe3c8c15af5aa0

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
