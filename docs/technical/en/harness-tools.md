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
fn input_schema(&self) -> &serde_json::Value
```

Advertise the tool’s input JSON Schema.

### Tool::validate_input

```text
async fn validate_input(&self, input: &serde_json::Value, ctx: &ToolUseContext) -> Result<(), ValidationError>
```

Validate tool-specific conditions before side effects; the default implementation succeeds.

### Tool::check_permissions

```text
async fn check_permissions(&self, input: &serde_json::Value, ctx: &ToolUseContext) -> permission::PermissionResult
```

Compute the invocation-specific permission decision before dispatch.

### Tool::call

```text
async fn call(&self, input: serde_json::Value, ctx: ToolUseContext, progress_tx: ToolProgressSender) -> Result<ToolCallResult, ToolError>
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
pub struct ToolProgress { pub tool_use_id: lingxi_core::types::ToolUseId, pub data: serde_json::Value }
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

## Sources and documentation baseline

[Source repository](https://github.com/lingxi-coder/harness-runtime)

- [harness-runtime/crates/tool-api/src/tool_trait.rs](https://github.com/lingxi-coder/harness-runtime/blob/bb8be1b7434045b0a85507315c1df9a5ddc27875/crates/tool-api/src/tool_trait.rs) — Committed source (bb8be1b7434045b0a85507315c1df9a5ddc27875)
- [harness-runtime/crates/tool-api/src/context.rs](https://github.com/lingxi-coder/harness-runtime/blob/bb8be1b7434045b0a85507315c1df9a5ddc27875/crates/tool-api/src/context.rs) — Committed source (bb8be1b7434045b0a85507315c1df9a5ddc27875)
- [harness-runtime/crates/tool-api/src/registry.rs](https://github.com/lingxi-coder/harness-runtime/blob/bb8be1b7434045b0a85507315c1df9a5ddc27875/crates/tool-api/src/registry.rs) — Committed source (bb8be1b7434045b0a85507315c1df9a5ddc27875)
- [harness-runtime/crates/tool-api/src/progress.rs](https://github.com/lingxi-coder/harness-runtime/blob/bb8be1b7434045b0a85507315c1df9a5ddc27875/crates/tool-api/src/progress.rs) — Committed source (bb8be1b7434045b0a85507315c1df9a5ddc27875)
- [harness-runtime/crates/orchestrator/src/turn_loop/tool_dispatch.rs](https://github.com/lingxi-coder/harness-runtime/blob/bb8be1b7434045b0a85507315c1df9a5ddc27875/crates/orchestrator/src/turn_loop/tool_dispatch.rs) — Committed source (bb8be1b7434045b0a85507315c1df9a5ddc27875)

- harness-runtime: HEAD bb8be1b7434045b0a85507315c1df9a5ddc27875; Referenced input fingerprint sha256:e9079905b280000c8013dd7f16954f3709e464fdcf67fca25de7e73176e1230a; Product pin bb8be1b7434045b0a85507315c1df9a5ddc27875

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
