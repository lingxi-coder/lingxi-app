# Compose skills, hooks and MCP

Choose the right extension, configure a scoped lifecycle check and verify an external capability is connected and callable.

<a id="choose-extension"></a>

## Choose by responsibility

A skill adds instructions; it does not bypass tool permissions or install process transport. A hook handles specific events; it should not replace a normal tool. MCP needs a live, authenticated connection rather than a settings entry alone.

| Need | Mechanism | Example |
| --- | --- | --- |
| Teach a repeatable procedure | Skill / command | Repository review steps and a required report format. |
| Run code or decide at a lifecycle event | Hook | Validate a pending write or record completion metadata. |
| Expose external tools/data | MCP server | A project service with its own connection and authentication. |
| Distribute several extensions together | Plugin | Versioned skills, hooks and servers under one lifecycle owner. |

<a id="write-a-skill"></a>

## Write a small, testable skill

parse_skill_markdown parses frontmatter/body into Skill; SkillRegistry handles registration/discovery. The host loader selects trusted roots and source tiers. Verify the skill appears in the host listing, then invoke it on a small fixture with an observable expected result.

### Markdown · skill source example

```markdown
---
name: inspect-change
description: Review a scoped change and report reproducible findings.
---

1. Read the requested diff and its owning source.
2. Check one normal path and one failure path.
3. Report actionable findings with file locations.
4. State which checks were actually run.

```

Verification method: Environment validation

This is file content; placement follows the host’s configured discovery roots. Another product’s configuration directory is not automatically active in every embedded host.

<a id="add-a-hook"></a>

## Add a scoped pre-execution check

Use parse_hook_layer_strict for strict editor/service validation of the event-map layer. parse_hooks_from_settings_json reads the enclosing settings object. Parsing establishes a definition, not a ready executor: command, HTTP, prompt, agent and MCP handlers need their host dependencies. Test an allow path and a blocked/error path.

| Field | Purpose |
| --- | --- |
| PreToolUse / matcher | Select the pre-tool event and canonical tool names to check. |
| type / command | Use the command executor and a host-resolved program. Supply the script yourself. |
| timeout | Bound foreground execution in seconds. |
| onFailure: block | Supported pre-action consumers block the action when this hook fails. |

### JSON · settings hook example

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Write|Edit",
        "hooks": [
          {
            "type": "command",
            "command": "python3 scripts/check-write.py",
            "timeout": 5,
            "onFailure": "block"
          }
        ]
      }
    ]
  }
}
```

Verification method: Environment validation

<a id="connect-mcp"></a>

## Check MCP readiness explicitly

1. Resolve the intended server configuration and authentication in the host.
2. Connect using the platform’s available transport; inspect connection/auth state.
3. Publish accepted tools into the server’s owning partition.
4. Refresh/remove the partition on catalog changes or disconnect.

### McpRegistry::connect

```text
pub async fn connect(&self, config: McpServerConfig) -> Result<McpConnectionId, McpError>
```

Connect through the host-supplied transport, returning the connection id or concrete failure.

### McpRegistry::has_callable_server

```text
pub async fn has_callable_server(&self, name: &str) -> bool
```

Check current callability rather than the mere existence of saved configuration.

### McpRegistry::subscribe_catalog_changes

```text
pub fn subscribe_catalog_changes(&self) -> broadcast::Receiver<McpCatalogChanged>
```

Observe catalog changes and replace the owning capability partition.

<a id="package-and-diagnose"></a>

## Package the workflow and diagnose activation

For a workflow spanning skills, hooks and MCP tools, use plugin lifecycle ownership instead of unrelated UI-side installation steps. Inspect loaded plugin state and component diagnostics. A manifest entry or parsed hook is not proof its runner, process or credential is ready.

| Symptom | First check |
| --- | --- |
| Skill is absent | Discovery roots, source tier, frontmatter and active-session loader. |
| Listed hook never runs | Event, matcher, managed policy and injected executor/runner. |
| Configured MCP has no callable tools | Connection/auth state and catalog refresh diagnostics. |
| Removed plugin tools remain visible | Owner-scoped teardown and next capability snapshot. |

- [Plugin activation](harness-plugins.md)
- [Extension architecture](extensions-lifecycle.md)
- [Claude hooks topic reference](https://code.claude.com/docs/en/hooks)

## Sources and documentation baseline

[Source repository](https://github.com/lingxi-coder/harness-runtime)

- [harness-runtime/crates/skill/src/frontmatter.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/skill/src/frontmatter.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/skill/src/model.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/skill/src/model.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/skill/src/registry.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/skill/src/registry.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/hooks/src/loader.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/hooks/src/loader.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/hooks/src/settings_layer.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/hooks/src/settings_layer.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/hooks/src/definition.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/hooks/src/definition.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/hooks/src/response.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/hooks/src/response.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/mcp/src/registry.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/mcp/src/registry.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/plugin/src/manager.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/plugin/src/manager.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/runtime/src/desktop/skill_loader.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/runtime/src/desktop/skill_loader.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)

- harness-runtime: HEAD f975ab5804bb3af0e72beb3974e5c911ece44d53; Referenced input fingerprint sha256:92c2108dc63215bc69ea9c11ea081795da883fdcb1c9c28b060aa9221a1b95a2; Product pin 0e28910fd624638e6c5d1f92eca4899a7938d188

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
