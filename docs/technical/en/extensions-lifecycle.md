# Tools, skills, hooks, MCP and LSP

Discover extensions through trusted registries and retain authority across activation, refresh, invocation and teardown.

<a id="extension-kinds"></a>

## Extension responsibilities

| Surface | Runtime responsibility |
| --- | --- |
| Tool | Canonical operation, schema, enablement, invocation permission, scheduling hints, progress and result. |
| Skill/command | Discovered instruction content and invocation semantics. A skill is not an independent provider network transport. |
| Hook | Configured event behavior that can inject context, enforce decisions or run host-owned actions. |
| Plugin | Validated package contributions across tool/command/agent/skill/hook/MCP/LSP/workflow registries. |

<a id="discovery-activation"></a>

## Discover and activate once

Desktop composition loads applicable commands/skills and constructs the production PluginManager with shared registries. ToolRegistry advertises deterministic partitions for builtins and dynamic sources. Session allowlists, tool-pool selection, plugin trust and managed restrictions participate before capabilities reach the model.

A package manifest can describe components without proving successful activation. Inspect loaded state and diagnostics. Reload updates the owning partition rather than duplicating capabilities; disable tears down contributed components and retains explicit failure state when teardown cannot complete.

<a id="hook-flow"></a>

## Hooks and permission-aware invocation

Events include PreToolUse, PostToolUse, PostToolUseFailure, SessionStart/End, PermissionRequest, TaskCompleted, ConfigChange and PostToolBatch. Command, HTTP, prompt and agent executors have different dependencies and error semantics. Use the executor registry rather than interpreting hook JSON inside a UI client.

Hook output is parsed through the shared response contract and may affect dispatch/context. HTTP hooks apply the explicit SSRF guard. Hook-initiated MCP calls use the already-live MCP registry; the desktop late-binding bridge avoids a construction cycle and does not grant hooks independent server discovery authority.

<a id="mcp-and-lsp"></a>

## MCP and LSP lifecycle

McpRegistry owns connections, authentication state, startup loading, catalog refresh and reconnect/backoff. The relevant host transport supplies stdio, HTTP, SSE or WebSocket support; mobile has a separate injected transport. A disconnected server is unavailable with diagnostic state, not a successful empty catalog. Elicitation and input-required states return through host request routing.

LSP registration accepts verified plugin servers; user/project settings cannot inject arbitrary LSP servers. The registry tracks activation, connections and restart policy (default maximum three restarts). Mobile Auto/Off/On activation uses trusted workspace classification and Mobile Linux transport, so native process availability remains a prerequisite.

<a id="extension-checks"></a>

## Reproducible lifecycle checks

1. Verify discovery with an explicit fixture, activate it, check advertised canonical names and exercise one permission-gated invocation.
2. Simulate transport disconnect and catalog refresh; ensure old tools disappear and pending requests resolve with the appropriate failure.
3. Disable the plugin, inspect owned process cleanup and retry a retained teardown failure. Test genuine third-party processes and OAuth separately.

### MCP and LSP registry tests

```bash
cargo test --locked -p mcp --test compile_smoke
cargo test --locked -p lsp --test plugin_only_registration_test
cargo test --locked -p lsp --test spawn_handshake_test
cargo test --locked -p test-harness --test mcp_lifecycle
```

Verification method: Environment validation

These commands identify existing contracts; they do not certify remote server behavior or third-party plugins. Keep external service versions and target-host evidence with the acceptance record.

## Sources and documentation baseline

[Source repository](https://github.com/lingxi-coder/harness-runtime)

- [harness-runtime/crates/tool-api/src/registry.rs](https://github.com/lingxi-coder/harness-runtime/blob/bb8be1b7434045b0a85507315c1df9a5ddc27875/crates/tool-api/src/registry.rs) — Committed source (bb8be1b7434045b0a85507315c1df9a5ddc27875)
- [harness-runtime/crates/skill/src/lib.rs](https://github.com/lingxi-coder/harness-runtime/blob/bb8be1b7434045b0a85507315c1df9a5ddc27875/crates/skill/src/lib.rs) — Committed source (bb8be1b7434045b0a85507315c1df9a5ddc27875)
- [harness-runtime/crates/runtime/src/desktop/skill_loader.rs](https://github.com/lingxi-coder/harness-runtime/blob/bb8be1b7434045b0a85507315c1df9a5ddc27875/crates/runtime/src/desktop/skill_loader.rs) — Committed source (bb8be1b7434045b0a85507315c1df9a5ddc27875)
- [harness-runtime/crates/hooks/src/events.rs](https://github.com/lingxi-coder/harness-runtime/blob/bb8be1b7434045b0a85507315c1df9a5ddc27875/crates/hooks/src/events.rs) — Committed source (bb8be1b7434045b0a85507315c1df9a5ddc27875)
- [harness-runtime/crates/hooks/src/executor.rs](https://github.com/lingxi-coder/harness-runtime/blob/bb8be1b7434045b0a85507315c1df9a5ddc27875/crates/hooks/src/executor.rs) — Committed source (bb8be1b7434045b0a85507315c1df9a5ddc27875)
- [harness-runtime/crates/hooks/src/ssrf_guard.rs](https://github.com/lingxi-coder/harness-runtime/blob/bb8be1b7434045b0a85507315c1df9a5ddc27875/crates/hooks/src/ssrf_guard.rs) — Committed source (bb8be1b7434045b0a85507315c1df9a5ddc27875)
- [harness-runtime/crates/plugin/src/manager.rs](https://github.com/lingxi-coder/harness-runtime/blob/bb8be1b7434045b0a85507315c1df9a5ddc27875/crates/plugin/src/manager.rs) — Committed source (bb8be1b7434045b0a85507315c1df9a5ddc27875)
- [harness-runtime/crates/mcp/src/registry.rs](https://github.com/lingxi-coder/harness-runtime/blob/bb8be1b7434045b0a85507315c1df9a5ddc27875/crates/mcp/src/registry.rs) — Committed source (bb8be1b7434045b0a85507315c1df9a5ddc27875)
- [harness-runtime/crates/mcp/src/registry/catalog_refresh.rs](https://github.com/lingxi-coder/harness-runtime/blob/bb8be1b7434045b0a85507315c1df9a5ddc27875/crates/mcp/src/registry/catalog_refresh.rs) — Committed source (bb8be1b7434045b0a85507315c1df9a5ddc27875)
- [harness-runtime/crates/lsp/src/registry.rs](https://github.com/lingxi-coder/harness-runtime/blob/bb8be1b7434045b0a85507315c1df9a5ddc27875/crates/lsp/src/registry.rs) — Committed source (bb8be1b7434045b0a85507315c1df9a5ddc27875)
- [harness-runtime/crates/runtime/src/mobile/mobile_lsp.rs](https://github.com/lingxi-coder/harness-runtime/blob/bb8be1b7434045b0a85507315c1df9a5ddc27875/crates/runtime/src/mobile/mobile_lsp.rs) — Committed source (bb8be1b7434045b0a85507315c1df9a5ddc27875)

- harness-runtime: HEAD bb8be1b7434045b0a85507315c1df9a5ddc27875; Referenced input fingerprint sha256:e9079905b280000c8013dd7f16954f3709e464fdcf67fca25de7e73176e1230a; Product pin bb8be1b7434045b0a85507315c1df9a5ddc27875

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
