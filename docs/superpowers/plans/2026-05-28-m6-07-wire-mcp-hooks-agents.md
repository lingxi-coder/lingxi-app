# M6-07 Engine Wiring 2 — MCP / Hooks / Agents Listings Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the three `OrchestratorHandle::list_mcp_servers / list_hooks / list_agents` stubs (which currently return `vec![]`) with real reads off `Arc<lingxi_mcp::McpRegistry>` / `Arc<RwLock<lingxi_hooks::HookRegistry>>` / `Arc<RwLock<Vec<lingxi_agent::AgentDefinition>>>`, plumb startup loaders for `.mcp.json` + project-local `.claude/agents/` + global `~/.claude/agents/`, and lock the empty-state literals so `/mcp`, `/hooks`, `/agents` show real configured items end-to-end through both the v0.6.0 stdio REPL and the M6-02+ TUI.

**Architecture:** `ConversationOrchestrator` grows three optional `Arc`-shared registry fields (`mcp_registry`, `hook_registry`, `agent_catalog`), each defaulting to `None` (so the v0.6.0 trait surface stays infallible and the parity fixtures keep passing). Three builder methods (`with_mcp_registry`, `with_hook_registry`, `with_agent_catalog`) attach them. `handle_impl.rs` reads each registry inside the existing `list_*` methods and projects into the locked `McpServerInfo` / `HookInfo` / `AgentInfo` shapes from `lingxi_traits::orchestrator`. The CLI binary (`lingxi-cli::init::build_runtime`) constructs all three registries by reading `.mcp.json` (project precedence over `~/.config/lingxi/mcp.json`), settings.json hooks (M3-01 carries the parsed `hooks` map already), and the union of `~/.claude/agents/*.md` + `<cwd>/.claude/agents/*.md`. Empty-state rendering is moved out of the handlers' generic `render_list` helper into a per-command empty-state literal (`"No MCP servers configured"` / `"No hooks configured"` / `"No subagents configured"`), keeping the non-empty layout unchanged.

**Tech Stack:** Rust 2021, `lingxi-mcp = { path = "../mcp" }` (M2-02b — already a workspace dep), `lingxi-hooks = { path = "../hooks" }` (M5-06), `lingxi-agent = { path = "../agent" }` (M4-05), `serde_json` for `.mcp.json` parsing, `gray_matter` for agent frontmatter (already in `lingxi-skills` deps), `tokio::sync::RwLock` for the hook + agent registries (mutated rarely, read often), `tokio::fs::read_dir` for the agent loader.

---

## File Structure

**Modify:**
- `lingxi-code/crates/orchestrator/src/conversation.rs` — add three optional registry fields + three builder methods (`with_mcp_registry`, `with_hook_registry`, `with_agent_catalog`).
- `lingxi-code/crates/orchestrator/src/handle_impl.rs:106-127` — `list_mcp_servers`, `list_hooks`, `list_agents` read the real registries when wired, fall back to `vec![]` when unset.
- `lingxi-code/crates/orchestrator/Cargo.toml` — add `lingxi-mcp = { path = "../mcp" }`, `lingxi-hooks = { path = "../hooks" }`, `lingxi-agent = { path = "../agent" }` to `[dependencies]` (lingxi-hooks may already be transitive; the explicit dep makes the field type compile).
- `lingxi-code/crates/commands/src/builtin/list_render.rs` — extend `render_list` to take an `empty_state: &str` argument so the three handlers can supply distinct empty-state literals.
- `lingxi-code/crates/commands/src/builtin/mcp.rs`, `hooks.rs`, `agents.rs` — pass the empty-state literal into the updated `render_list`. Update the existing 6 `empty_list` tests to the new expected output.
- `lingxi-code/crates/cli/src/init.rs` — construct `McpRegistry`, `HookRegistry`, agent catalog `Arc<RwLock<Vec<AgentDefinition>>>` from disk; wire all three onto the orchestrator via the new builders.
- `lingxi-code/crates/cli/Cargo.toml` — add `lingxi-mcp = { path = "../mcp" }`, `lingxi-hooks = { path = "../hooks" }`, `lingxi-agent = { path = "../agent" }`.

**Create:**
- `lingxi-code/crates/mcp/src/json_config.rs` — parse `.mcp.json` (claude-code-compatible `{ "mcpServers": { name: { command, args, env, … } } }` shape) into `Vec<McpServerConfig>`. Public surface: `parse_mcp_json_string(raw: &str, scope: ConfigScope) -> Result<Vec<McpServerConfig>, McpJsonError>` and `load_mcp_json_with_precedence(project: &Path, global: &Path) -> Vec<McpServerConfig>`.
- `lingxi-code/crates/mcp/src/lib.rs` — re-export `parse_mcp_json_string` + `load_mcp_json_with_precedence`.
- `lingxi-code/crates/hooks/src/loader.rs` — `load_hooks_from_settings(settings: &lingxi_core::Settings) -> Vec<HookDefinition>`. Reads the existing M3-01 settings `hooks` map.
- `lingxi-code/crates/hooks/src/lib.rs` — re-export `load_hooks_from_settings`.
- `lingxi-code/crates/agent/src/catalog.rs` — frontmatter loader: `load_agents_from_dirs(paths: &[PathBuf]) -> Vec<AgentDefinition>` reads every `*.md` under each path, parses `---` YAML frontmatter with `gray_matter`, and projects into `AgentDefinition` (project paths take precedence over global on `agent_type` collision). Public surface: `load_agents_from_dirs` + `parse_agent_markdown(raw: &str, source: AgentSource, base_dir: PathBuf) -> Result<AgentDefinition, AgentLoadError>`.
- `lingxi-code/crates/agent/src/lib.rs` — re-export `load_agents_from_dirs` + `parse_agent_markdown`.
- `lingxi-code/crates/agent/Cargo.toml` — add `gray_matter = "0.2"` (workspace already uses it in `lingxi-skills`) and `serde_yaml = "0.9"` if not present.
- `lingxi-code/crates/orchestrator/tests/list_mcp_real.rs` — behavioural test for `list_mcp_servers`.
- `lingxi-code/crates/orchestrator/tests/list_hooks_real.rs` — behavioural test for `list_hooks`.
- `lingxi-code/crates/orchestrator/tests/list_agents_real.rs` — behavioural test for `list_agents`.
- `lingxi-code/crates/test-harness/src/parity/fixtures/tui_listings.json` — parity fixture locking the three empty-state literals.
- `lingxi-code/crates/test-harness/tests/parity_tui_listings.rs` — driver for the new fixture.

**Key types (locked — unchanged from `lingxi_traits::orchestrator`):**
- `McpServerInfo { name: String, status: McpStatus, transport: String }` — `McpStatus` ∈ `{ Connected, Disconnected, Error(String) }`.
- `HookInfo { name: String, event: String, matcher: Option<String>, timeout_ms: u64 }` — `timeout_ms` defaults to `60_000` when `HookDefinition::timeout` is `None`.
- `AgentInfo { name: String, description: String, tools_allowed: Vec<String> }` — `name` maps to `AgentDefinition::agent_type`; `description` maps to `AgentDefinition::when_to_use`.

**Empty-state literals (locked here; verified against claude-code `cli/handlers/mcp.tsx:151` and `components/hooks/SelectMatcherMode.tsx:70`):**
- MCP: `"No MCP servers configured"` (LingXi form — drops claude-code's `". Use \`claude mcp add\` to add a server."` trailing hint because `claude mcp add` is a claude-code-specific CLI, not LingXi).
- Hooks: `"No hooks configured"` (LingXi form — drops `". To add hooks, edit settings.json directly or ask Claude."` trailing hint for the same reason).
- Agents: `"No subagents configured"` (LingXi form — claude-code uses `"No agents found"` in `cli/handlers/agents.ts:63`, but the prompt locks the M6-07 wording explicitly; this plan honours the prompt).

**Deviation from prompt's naming:** The prompt mentions `lingxi_mcp::ClientRegistry` and `lingxi_agent::AgentCatalog`. The **actual** types in the codebase are `lingxi_mcp::McpRegistry` (re-exported from `mcp::registry`) and there is **no `AgentCatalog` type**; agents live as a `Vec<AgentDefinition>` constructed in-memory. This plan adds `lingxi_agent::catalog::load_agents_from_dirs` (a loader function, not a struct) and uses `Arc<RwLock<Vec<AgentDefinition>>>` as the storage type on the orchestrator. No new `AgentCatalog` newtype — keeps the surface flat and avoids breaking M4-05's existing call sites.

**StatusLine MCP count:** the spec §3 M6-07 row marks this as **optional**. claude-code's `StatusLine.tsx` is a user-configured external shell command, not a built-in MCP-count indicator — so M6-07 ships **without** any StatusLine change. The `n_mcp_total` / `n_mcp_connected` fields on `StatusSnapshot` (used by `/status`, not StatusLine) are populated from the new registries in Task 9, which is the closest claude-code parity gives us.

---

## Task 0: Lock the registry shapes + empty-state literals (no code yet)

**Files:**
- Read: `lingxi-code/crates/mcp/src/registry.rs:17-86` — confirm `McpRegistry` API (`connections: RwLock<HashMap<String, McpConnectionState>>`).
- Read: `lingxi-code/crates/hooks/src/registry.rs:50-101` — confirm `HookRegistry::sources` + `plugin` shape.
- Read: `lingxi-code/crates/agent/src/definition.rs:14-48` — confirm `AgentDefinition` field shape.
- Read: `lingxi-code/crates/traits/src/orchestrator.rs:97-141` — re-confirm `McpServerInfo`, `HookInfo`, `AgentInfo` are unchanged.
- Read: `claude-code/src/cli/handlers/mcp.tsx:151` — confirm `"No MCP servers configured"` is the production string.
- Read: `claude-code/src/components/hooks/SelectMatcherMode.tsx:70` — confirm `"No hooks configured for this event"`.

- [ ] **Step 1: Confirm the registry read paths.**

Run:
```bash
rg -n "pub fn|pub async fn" lingxi-code/crates/mcp/src/registry.rs
rg -n "pub fn|pub async fn" lingxi-code/crates/hooks/src/registry.rs
rg -n "pub struct AgentDefinition" lingxi-code/crates/agent/src/definition.rs
```

Expected (locked here so all later code compiles against the real signatures):

| Symbol | Signature |
|---|---|
| `McpRegistry::connections` | `RwLock<HashMap<String, McpConnectionState>>` — read with `.read().await` |
| `McpConnectionState::name() -> &str` | helper to extract the server name from any variant |
| `HookRegistry::sources` | `HashMap<HookSource, Vec<HookDefinition>>` (private; needs a new public iterator) |
| `HookRegistry::plugin` | `HashMap<PluginId, Vec<HookDefinition>>` (private; same) |
| `AgentDefinition::agent_type` | `String` (→ `AgentInfo::name`) |
| `AgentDefinition::when_to_use` | `String` (→ `AgentInfo::description`) |
| `AgentDefinition::allowed_tools` | `Vec<String>` (→ `AgentInfo::tools_allowed`) |

Notes:
- `HookRegistry` does **not** expose a public iterator today (its fields are `pub(crate)`-style — the only public methods are `new`, `register`, `register_plugin_hooks`, `unregister_plugin`, `match_event`). Task 3 adds a public `pub fn all_hooks(&self) -> Vec<&HookDefinition>` snapshot method.
- `McpRegistry` does **not** expose a public iterator either; Task 2 adds `pub async fn snapshot(&self) -> Vec<McpServerInfo>` that projects each `McpConnectionState` into the trait shape.
- No `AgentCatalog` newtype exists — we add `lingxi_agent::catalog::load_agents_from_dirs` and store `Arc<RwLock<Vec<AgentDefinition>>>` on the orchestrator.

- [ ] **Step 2: Lock the empty-state literals.**

Per the "Empty-state literals" section in the File Structure header above:
- MCP: `"No MCP servers configured"`
- Hooks: `"No hooks configured"`
- Agents: `"No subagents configured"`

These replace the current `"<Label> (0):\n"` output for the empty case, but the non-empty rendering remains `"<Label> ({count}):\n  <row1>\n…"` unchanged.

- [ ] **Step 3: Decide MCP status projection.**

Each `McpConnectionState` variant maps to one of the three `McpStatus` values:

| `McpConnectionState` | `McpStatus` |
|---|---|
| `Connected { .. }` | `Connected` |
| `Disconnected { .. }` (no error) | `Disconnected` |
| `Disconnected { last_error: Some(e), .. }` | `Error(e)` |
| `Connecting`, `HealthChecking`, `Reconnecting`, `AwaitingOAuth` | `Disconnected` (transient) |
| `Failed { error, .. }` | `Error(error)` |
| `Stopped { .. }` | `Disconnected` |

Transport string is derived from `config.spec.kind()`. Add `McpTransportSpec::kind() -> &'static str` in step 2 of Task 2 if it does not yet exist (returns `"stdio"`, `"sse"`, or `"http"`).

- [ ] **Step 4: Decide `.mcp.json` precedence + on-disk locations.**

Precedence (project wins on name collision):
1. `<cwd>/.mcp.json` — `ConfigScope::Project`
2. `~/.config/lingxi/mcp.json` — `ConfigScope::User`

Missing files are silent — empty list. JSON parse errors are reported via `tracing::warn!` and the file is skipped (do not fail the binary on a malformed `.mcp.json`).

- [ ] **Step 5: Decide `~/.claude/agents/` precedence.**

Precedence (project wins on `agent_type` collision):
1. `<cwd>/.claude/agents/*.md` — `AgentSource::Project`
2. `~/.claude/agents/*.md` — `AgentSource::UserDefined`

Files with no frontmatter or invalid YAML are skipped via `tracing::warn!`.

- [ ] **Step 6: Commit Task 0 notes inline (no code yet).**

```bash
git add docs/superpowers/plans/2026-05-28-m6-07-wire-mcp-hooks-agents.md
git commit -m "docs(m6-07): land MCP/hooks/agents wiring contract notes"
```

---

## Task 1: `render_list` gains an `empty_state` argument

**Files:**
- Modify: `lingxi-code/crates/commands/src/builtin/list_render.rs`
- Test: `lingxi-code/crates/commands/src/builtin/list_render.rs` (inline `#[cfg(test)]`)

- [ ] **Step 1: Write the failing test.**

Replace the existing `tests` module in `crates/commands/src/builtin/list_render.rs` with:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_list_renders_empty_state_literal() {
        let s = render_list("MCP servers", vec![], "No MCP servers configured");
        assert_eq!(s, "No MCP servers configured\n");
    }

    #[test]
    fn multi_row_renders_with_leading_two_spaces() {
        let s = render_list(
            "Hooks",
            vec!["fmt PostToolUse 60000ms".into(), "lint Stop 30000ms".into()],
            "No hooks configured",
        );
        assert_eq!(
            s,
            "Hooks (2):\n  fmt PostToolUse 60000ms\n  lint Stop 30000ms\n"
        );
    }

    #[test]
    fn empty_state_is_used_for_zero_rows_only() {
        // Single row stays in the labelled-list shape.
        let s = render_list("Agents", vec!["reviewer  review code".into()], "No subagents configured");
        assert_eq!(s, "Agents (1):\n  reviewer  review code\n");
    }
}
```

- [ ] **Step 2: Run; expect FAIL (signature mismatch).**

```bash
cargo test -p lingxi-commands builtin::list_render::tests --no-run
```
Expected: COMPILE FAIL — `render_list` takes 2 args, test calls with 3.

- [ ] **Step 3: Extend the signature.**

Replace the body of `crates/commands/src/builtin/list_render.rs::render_list` with:

```rust
/// Render a labelled list with the locked layout, or the supplied
/// `empty_state` literal when `rows` is empty.
///
/// Non-empty output: `"<Label> ({count}):\n  <row1>\n  <row2>\n…"`
/// Empty output:     `"<empty_state>\n"`
#[must_use]
pub fn render_list(label: &str, rows: Vec<String>, empty_state: &str) -> String {
    if rows.is_empty() {
        return format!("{empty_state}\n");
    }
    let count = rows.len();
    let mut out = format!("{label} ({count}):\n");
    for row in rows {
        out.push_str("  ");
        out.push_str(&row);
        out.push('\n');
    }
    out
}
```

- [ ] **Step 4: Run; expect PASS.**

```bash
cargo test -p lingxi-commands builtin::list_render::tests
```
Expected: 3 passing.

- [ ] **Step 5: Commit.**

```bash
git add lingxi-code/crates/commands/src/builtin/list_render.rs
git commit -m "feat(commands): render_list takes an empty_state literal"
```

---

## Task 2: Public `McpRegistry::snapshot` projecting state → `McpServerInfo`

**Files:**
- Modify: `lingxi-code/crates/mcp/src/registry.rs`
- Modify: `lingxi-code/crates/mcp/src/connection.rs` — add `pub fn name(&self) -> &str` on `McpConnectionState` if missing; add `pub fn transport_kind(&self) -> &'static str` on `McpTransportSpec` (or wherever the variant lives).
- Modify: `lingxi-code/crates/mcp/Cargo.toml` — add `lingxi-traits = { path = "../traits" }` to `[dependencies]` (already a transitive dep; the `McpServerInfo` import needs the explicit declaration).
- Test: `lingxi-code/crates/mcp/src/registry.rs` (inline `#[cfg(test)]`)

- [ ] **Step 1: Write the failing test.**

Append to `crates/mcp/src/registry.rs`:

```rust
#[cfg(test)]
mod snapshot_tests {
    use super::*;
    use crate::connection::{ConfigScope, McpServerConfig};
    use lingxi_traits::{McpServerInfo, McpStatus, McpTransportSpec};
    use std::sync::Arc;

    fn stdio_cfg(name: &str) -> McpServerConfig {
        McpServerConfig {
            name: name.into(),
            spec: McpTransportSpec::Stdio {
                command: "echo".into(),
                args: vec![],
                env: Default::default(),
                cwd: None,
            },
            scope: ConfigScope::Project,
            disabled: false,
        }
    }

    #[tokio::test]
    async fn snapshot_empty_registry() {
        let t = Arc::new(crate::test_support::MockMcpTransport::default());
        let r = McpRegistry::new(t);
        assert_eq!(r.snapshot().await, Vec::<McpServerInfo>::new());
    }

    #[tokio::test]
    async fn snapshot_disconnected_server_no_error() {
        let t = Arc::new(crate::test_support::MockMcpTransport::default());
        let r = McpRegistry::new(t);
        let cfg = stdio_cfg("memory");
        r.connections.write().await.insert(
            "memory".into(),
            McpConnectionState::Disconnected { config: cfg, last_error: None },
        );
        let snap = r.snapshot().await;
        assert_eq!(
            snap,
            vec![McpServerInfo {
                name: "memory".into(),
                status: McpStatus::Disconnected,
                transport: "stdio".into(),
            }]
        );
    }
}
```

(Note: `crates/mcp/src/test_support.rs` does not yet exist; if there's no `MockMcpTransport` reachable from the mcp crate's unit tests, replace it with a minimal anonymous mock declared inline in the test. The test-harness has `mocks::mock_mcp::MockMcpTransport` available for integration tests in `crates/test-harness/`.)

If no in-crate mock exists, replace the two-test module above with:

```rust
#[cfg(test)]
mod snapshot_tests {
    use super::*;
    use crate::connection::{ConfigScope, McpServerConfig};
    use lingxi_traits::{McpServerInfo, McpStatus, McpTransportSpec};
    use std::sync::Arc;

    // Minimal mock — only `connections` matters for snapshot.
    struct StubTransport;
    #[async_trait::async_trait]
    impl lingxi_traits::McpTransport for StubTransport {
        async fn connect(&self, _spec: &McpTransportSpec) -> Result<lingxi_traits::McpConnection, lingxi_traits::McpError> { unreachable!() }
        async fn initialize(&self, _c: &lingxi_traits::McpConnection) -> Result<lingxi_traits::ServerCapabilitiesDto, lingxi_traits::McpError> { unreachable!() }
        async fn list_tools(&self, _c: &lingxi_traits::McpConnection) -> Result<Vec<lingxi_traits::McpToolDto>, lingxi_traits::McpError> { unreachable!() }
        async fn list_resources(&self, _c: &lingxi_traits::McpConnection) -> Result<Vec<lingxi_traits::McpResourceDto>, lingxi_traits::McpError> { unreachable!() }
        async fn list_prompts(&self, _c: &lingxi_traits::McpConnection) -> Result<Vec<lingxi_traits::McpPromptDto>, lingxi_traits::McpError> { unreachable!() }
        async fn disconnect(&self, _id: lingxi_protocol::McpConnectionId) -> Result<(), lingxi_traits::McpError> { unreachable!() }
    }

    #[tokio::test]
    async fn snapshot_empty_registry() {
        let r = McpRegistry::new(Arc::new(StubTransport));
        assert_eq!(r.snapshot().await, Vec::<McpServerInfo>::new());
    }

    #[tokio::test]
    async fn snapshot_disconnected_server_no_error() {
        let r = McpRegistry::new(Arc::new(StubTransport));
        let cfg = McpServerConfig {
            name: "memory".into(),
            spec: McpTransportSpec::Stdio {
                command: "echo".into(),
                args: vec![],
                env: Default::default(),
                cwd: None,
            },
            scope: ConfigScope::Project,
            disabled: false,
        };
        r.connections.write().await.insert(
            "memory".into(),
            McpConnectionState::Disconnected { config: cfg, last_error: None },
        );
        let snap = r.snapshot().await;
        assert_eq!(
            snap,
            vec![McpServerInfo {
                name: "memory".into(),
                status: McpStatus::Disconnected,
                transport: "stdio".into(),
            }]
        );
    }
}
```

(If the actual `McpTransport` trait signature in `lingxi_traits` differs, run `rg -n "pub trait McpTransport" lingxi-code/crates/traits/src/` and adjust.)

- [ ] **Step 2: Run; expect FAIL (`snapshot` method missing).**

```bash
cargo test -p lingxi-mcp snapshot_tests:: --no-run
```
Expected: COMPILE FAIL — `no method named 'snapshot'`.

- [ ] **Step 3: Add the transport-kind helper if missing.**

In `crates/traits/src/` (find the file declaring `McpTransportSpec`):

```rust
impl McpTransportSpec {
    /// Short transport-kind label for display (`"stdio"`, `"sse"`, `"http"`).
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Stdio { .. } => "stdio",
            Self::Sse { .. } => "sse",
            Self::Http { .. } => "http",
        }
    }
}
```

(If `McpTransportSpec` has additional variants, extend the match. If `kind()` already exists, skip this step.)

- [ ] **Step 4: Add `snapshot` on `McpRegistry`.**

In `crates/mcp/src/registry.rs`, append to `impl McpRegistry`:

```rust
    /// Project every known connection into the trait-facing
    /// [`lingxi_traits::McpServerInfo`] shape. Used by
    /// `OrchestratorHandle::list_mcp_servers` (M6-07).
    pub async fn snapshot(&self) -> Vec<lingxi_traits::McpServerInfo> {
        use lingxi_traits::{McpServerInfo, McpStatus};
        let conns = self.connections.read().await;
        let mut out: Vec<McpServerInfo> = conns
            .values()
            .map(|s| McpServerInfo {
                name: s.name().to_string(),
                status: project_status(s),
                transport: s.transport_kind().to_string(),
            })
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }
```

And below the impl block, add the projector + a `transport_kind` helper on `McpConnectionState`:

```rust
fn project_status(state: &McpConnectionState) -> lingxi_traits::McpStatus {
    use lingxi_traits::McpStatus;
    match state {
        McpConnectionState::Connected { .. } => McpStatus::Connected,
        McpConnectionState::Disconnected { last_error: Some(e), .. } => McpStatus::Error(e.clone()),
        McpConnectionState::Disconnected { .. }
        | McpConnectionState::Connecting { .. }
        | McpConnectionState::AwaitingOAuth { .. }
        | McpConnectionState::HealthChecking { .. }
        | McpConnectionState::Reconnecting { .. }
        | McpConnectionState::Stopped { .. } => McpStatus::Disconnected,
        McpConnectionState::Failed { error, .. } => McpStatus::Error(error.clone()),
    }
}

impl McpConnectionState {
    /// Transport-kind label of the connection's config (`"stdio"`/`"sse"`/`"http"`).
    #[must_use]
    pub fn transport_kind(&self) -> &'static str {
        let cfg = match self {
            Self::Disconnected { config, .. }
            | Self::Connecting { config, .. }
            | Self::AwaitingOAuth { config, .. }
            | Self::Connected { config, .. }
            | Self::HealthChecking { config, .. }
            | Self::Reconnecting { config, .. }
            | Self::Failed { config, .. }
            | Self::Stopped { config } => config,
        };
        cfg.spec.kind()
    }
}
```

(Note: `McpConnectionState::name()` likely already exists per `connection.rs:120`. If `transport_kind()` shape clashes with an existing helper, prefer the existing one.)

If `connections` is `pub(crate)`-private, also bump its visibility to `pub(crate)` (it likely already is); the test uses `r.connections.write().await` which requires the same crate's tests to see it. The test module is `#[cfg(test)] mod snapshot_tests` inside the same file — same-crate visibility is sufficient.

- [ ] **Step 5: Run; expect PASS.**

```bash
cargo test -p lingxi-mcp snapshot_tests::
```
Expected: 2 passing.

- [ ] **Step 6: Commit.**

```bash
git add lingxi-code/crates/mcp/src/registry.rs lingxi-code/crates/mcp/Cargo.toml lingxi-code/crates/traits/src/
git commit -m "feat(mcp): McpRegistry::snapshot projects state into McpServerInfo"
```

---

## Task 3: Public `HookRegistry::all_hooks` snapshot

**Files:**
- Modify: `lingxi-code/crates/hooks/src/registry.rs`
- Test: same file (inline `#[cfg(test)]`)

- [ ] **Step 1: Write the failing test.**

Append to `crates/hooks/src/registry.rs`:

```rust
#[cfg(test)]
mod all_hooks_tests {
    use super::*;
    use crate::definition::{HookExecutor, HookSource};
    use crate::events::HookEventType;
    use lingxi_protocol::HookId;

    fn hk(name: &str, event: HookEventType, source: HookSource) -> HookDefinition {
        HookDefinition {
            id: HookId::new(),
            name: name.into(),
            events: vec![event],
            if_condition: None,
            executor: HookExecutor::Builtin { handler_id: "noop".into() },
            source,
            blocking: true,
            timeout: None,
            priority: 0,
        }
    }

    #[test]
    fn all_hooks_returns_empty_for_fresh_registry() {
        let r = HookRegistry::new();
        assert!(r.all_hooks().is_empty());
    }

    #[test]
    fn all_hooks_unions_source_and_plugin_buckets() {
        let mut r = HookRegistry::new();
        r.register(hk("user-fmt", HookEventType::PostToolUse, HookSource::User));
        r.register(hk("project-lint", HookEventType::Stop, HookSource::Project));
        r.register_plugin_hooks(
            lingxi_protocol::PluginId::new(),
            vec![hk("plugin-x", HookEventType::PreToolUse, HookSource::Plugin)],
        );

        let names: Vec<&str> = r.all_hooks().iter().map(|h| h.name.as_str()).collect();
        assert!(names.contains(&"user-fmt"));
        assert!(names.contains(&"project-lint"));
        assert!(names.contains(&"plugin-x"));
        assert_eq!(names.len(), 3);
    }
}
```

(If `HookEventType` variants differ, run `rg -n "pub enum HookEventType" lingxi-code/crates/hooks/src/events.rs` and adjust. If `HookId::new` / `PluginId::new` don't exist, look up the actual constructors.)

- [ ] **Step 2: Run; expect FAIL (method missing).**

```bash
cargo test -p lingxi-hooks all_hooks_tests:: --no-run
```

- [ ] **Step 3: Add `all_hooks`.**

In `crates/hooks/src/registry.rs`, append to `impl HookRegistry`:

```rust
    /// Snapshot every registered hook across all sources (user / project /
    /// local / managed / plugin / frontmatter / session / skill).
    ///
    /// Used by `OrchestratorHandle::list_hooks` (M6-07). Returned in
    /// unspecified order — callers that need stable order should sort by
    /// `name`.
    #[must_use]
    pub fn all_hooks(&self) -> Vec<&HookDefinition> {
        let mut out: Vec<&HookDefinition> = self.sources.values().flatten().collect();
        out.extend(self.plugin.values().flatten());
        out.extend(self.frontmatter.values().flatten());
        out
    }
```

- [ ] **Step 4: Run; expect PASS.**

```bash
cargo test -p lingxi-hooks all_hooks_tests::
```

- [ ] **Step 5: Commit.**

```bash
git add lingxi-code/crates/hooks/src/registry.rs
git commit -m "feat(hooks): HookRegistry::all_hooks snapshot across all sources"
```

---

## Task 4: `lingxi_hooks::loader::load_hooks_from_settings`

**Files:**
- Create: `lingxi-code/crates/hooks/src/loader.rs`
- Modify: `lingxi-code/crates/hooks/src/lib.rs` — `pub mod loader; pub use loader::load_hooks_from_settings;`
- Test: `lingxi-code/crates/hooks/src/loader.rs` (inline `#[cfg(test)]`)

- [ ] **Step 1: Find the existing settings hooks schema.**

```bash
rg -n "hooks.*HashMap\|hooks: \|pub hooks" lingxi-code/crates/core/src/settings/ | head -10
```

Expected: `lingxi_core::Settings` (or equivalent) carries a `hooks: HashMap<HookEventType, Vec<HookSettingEntry>>` or similar shape parsed in M3-01. Read the exact field on `Settings` and adapt the loader accordingly.

If the codebase's settings type currently has NO `hooks` field (M3-01 only parsed the section but did not surface it), instead read the raw JSON from `~/.config/lingxi/settings.json` (and `<cwd>/.claude/settings.json`) and parse just the `hooks` block locally. The fallback variant uses `serde_json::Value` to avoid coupling to the full `Settings` struct.

- [ ] **Step 2: Write the failing test.**

Create `crates/hooks/src/loader.rs`:

```rust
//! Hook loader (M6-07) — projects parsed settings into a
//! `Vec<HookDefinition>` ready to feed into [`crate::HookRegistry::register`].
//!
//! Settings shape (claude-code compatible):
//! ```json
//! {
//!   "hooks": {
//!     "PreToolUse": [
//!       { "matcher": "Write|Edit", "hooks": [
//!         { "type": "command", "command": "./fmt.sh", "timeout": 60 }
//!       ]}
//!     ]
//!   }
//! }
//! ```
//! Missing or empty `hooks` block yields `vec![]`.

use crate::definition::{HookCondition, HookDefinition, HookExecutor, HookSource};
use crate::events::HookEventType;
use lingxi_protocol::HookId;
use serde::Deserialize;
use std::collections::HashMap;
use std::time::Duration;

/// Top-level settings shape consumed by [`parse_hooks_from_settings_json`].
#[derive(Debug, Deserialize)]
struct SettingsTop {
    #[serde(default)]
    hooks: HashMap<String, Vec<MatcherGroup>>,
}

#[derive(Debug, Deserialize)]
struct MatcherGroup {
    #[serde(default)]
    matcher: Option<String>,
    #[serde(default)]
    hooks: Vec<HookEntry>,
}

#[derive(Debug, Deserialize)]
struct HookEntry {
    #[serde(default, rename = "type")]
    kind: Option<String>,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    timeout: Option<u64>,
}

/// Parse the raw JSON string of a settings file into hook definitions.
///
/// Unknown event names are silently skipped. Entries without a `command`
/// field are skipped. Returns `Ok(vec![])` when the input has no `hooks`
/// block at all.
pub fn parse_hooks_from_settings_json(
    raw: &str,
    source: HookSource,
) -> Result<Vec<HookDefinition>, serde_json::Error> {
    let top: SettingsTop = serde_json::from_str(raw)?;
    let mut out = Vec::new();
    for (event_name, groups) in top.hooks {
        let Some(event_type) = parse_event_type(&event_name) else { continue };
        for group in groups {
            for entry in group.hooks {
                let Some(command) = entry.command else { continue };
                if entry.kind.as_deref() != Some("command") {
                    continue;
                }
                let condition = group.matcher.as_ref().map(|m| HookCondition {
                    pattern: m.clone(),
                    match_tool_name: true,
                    match_input: false,
                });
                out.push(HookDefinition {
                    id: HookId::new(),
                    name: command.clone(),
                    events: vec![event_type],
                    if_condition: condition,
                    executor: HookExecutor::Command {
                        command,
                        args: vec![],
                        env: HashMap::new(),
                        cwd: None,
                    },
                    source,
                    blocking: true,
                    timeout: entry.timeout.map(Duration::from_secs),
                    priority: 0,
                });
            }
        }
    }
    Ok(out)
}

fn parse_event_type(name: &str) -> Option<HookEventType> {
    match name {
        "PreToolUse" => Some(HookEventType::PreToolUse),
        "PostToolUse" => Some(HookEventType::PostToolUse),
        "Stop" => Some(HookEventType::Stop),
        "Notification" => Some(HookEventType::Notification),
        "UserPromptSubmit" => Some(HookEventType::UserPromptSubmit),
        "SubagentStop" => Some(HookEventType::SubagentStop),
        "PreCompact" => Some(HookEventType::PreCompact),
        "SessionStart" => Some(HookEventType::SessionStart),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_settings_yields_no_hooks() {
        let hooks = parse_hooks_from_settings_json("{}", HookSource::User).unwrap();
        assert!(hooks.is_empty());
    }

    #[test]
    fn one_pretooluse_command_hook() {
        let raw = r#"{
          "hooks": {
            "PreToolUse": [
              { "matcher": "Write|Edit", "hooks": [
                { "type": "command", "command": "./fmt.sh", "timeout": 30 }
              ]}
            ]
          }
        }"#;
        let hooks = parse_hooks_from_settings_json(raw, HookSource::Project).unwrap();
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0].name, "./fmt.sh");
        assert_eq!(hooks[0].events, vec![HookEventType::PreToolUse]);
        assert_eq!(hooks[0].timeout, Some(Duration::from_secs(30)));
        assert_eq!(hooks[0].source, HookSource::Project);
        let cond = hooks[0].if_condition.as_ref().expect("matcher present");
        assert_eq!(cond.pattern, "Write|Edit");
    }

    #[test]
    fn unknown_event_is_skipped() {
        let raw = r#"{ "hooks": { "Bogus": [{ "hooks": [{ "type": "command", "command": "x" }]}]}}"#;
        let hooks = parse_hooks_from_settings_json(raw, HookSource::User).unwrap();
        assert!(hooks.is_empty());
    }

    #[test]
    fn missing_command_field_is_skipped() {
        let raw = r#"{ "hooks": { "Stop": [{ "hooks": [{ "type": "command" }]}]}}"#;
        let hooks = parse_hooks_from_settings_json(raw, HookSource::User).unwrap();
        assert!(hooks.is_empty());
    }
}
```

- [ ] **Step 3: Run; expect FAIL (module not in lib.rs).**

```bash
cargo test -p lingxi-hooks loader::tests --no-run
```
Expected: COMPILE FAIL — `unresolved module 'loader'`.

- [ ] **Step 4: Re-export from `lib.rs`.**

In `crates/hooks/src/lib.rs`, find the existing `pub mod` lines and add:
```rust
pub mod loader;
pub use loader::parse_hooks_from_settings_json;
```

- [ ] **Step 5: Run; expect PASS.**

```bash
cargo test -p lingxi-hooks loader::tests
```
Expected: 4 passing.

- [ ] **Step 6: Commit.**

```bash
git add lingxi-code/crates/hooks/src/loader.rs lingxi-code/crates/hooks/src/lib.rs
git commit -m "feat(hooks): parse_hooks_from_settings_json loader (M6-07)"
```

---

## Task 5: `lingxi_mcp::json_config::parse_mcp_json_string` + precedence loader

**Files:**
- Create: `lingxi-code/crates/mcp/src/json_config.rs`
- Modify: `lingxi-code/crates/mcp/src/lib.rs`
- Test: `lingxi-code/crates/mcp/src/json_config.rs` (inline `#[cfg(test)]`)

- [ ] **Step 1: Write the failing test.**

Create `crates/mcp/src/json_config.rs`:

```rust
//! `.mcp.json` parser (M6-07).
//!
//! Reads the claude-code-compatible shape:
//! ```json
//! {
//!   "mcpServers": {
//!     "memory":     { "command": "mcp-memory",     "args": [], "env": {} },
//!     "filesystem": { "command": "mcp-filesystem", "args": ["/tmp"], "env": {} }
//!   }
//! }
//! ```
//! Each entry is projected into a [`crate::McpServerConfig`] with
//! [`crate::ConfigScope`] supplied by the caller. URL-based ("http"/"sse")
//! entries are accepted too via the `url` field.

use crate::connection::{ConfigScope, McpServerConfig};
use lingxi_traits::McpTransportSpec;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

/// Errors raised while parsing a `.mcp.json` file.
#[derive(Debug, thiserror::Error)]
pub enum McpJsonError {
    /// Invalid JSON.
    #[error("invalid .mcp.json: {0}")]
    Json(#[from] serde_json::Error),
    /// An entry had neither `command` nor `url` set.
    #[error("server '{0}' missing both command and url")]
    UnknownTransport(String),
}

#[derive(Debug, Deserialize)]
struct McpJsonTop {
    #[serde(default, rename = "mcpServers")]
    mcp_servers: HashMap<String, McpJsonEntry>,
}

#[derive(Debug, Deserialize)]
struct McpJsonEntry {
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: HashMap<String, String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default, rename = "type")]
    transport_type: Option<String>, // "http" | "sse" — only when `url` is set
    #[serde(default)]
    disabled: bool,
}

/// Parse a `.mcp.json` payload (file contents) into a list of configs.
pub fn parse_mcp_json_string(
    raw: &str,
    scope: ConfigScope,
) -> Result<Vec<McpServerConfig>, McpJsonError> {
    let top: McpJsonTop = serde_json::from_str(raw)?;
    let mut out = Vec::new();
    for (name, entry) in top.mcp_servers {
        let spec = if let Some(cmd) = entry.command {
            McpTransportSpec::Stdio {
                command: cmd,
                args: entry.args,
                env: entry.env,
                cwd: entry.cwd.map(std::path::PathBuf::from),
            }
        } else if let Some(url) = entry.url {
            match entry.transport_type.as_deref() {
                Some("sse") => McpTransportSpec::Sse { url, headers: Default::default() },
                _ /* default to http */ => McpTransportSpec::Http { url, headers: Default::default() },
            }
        } else {
            return Err(McpJsonError::UnknownTransport(name));
        };
        out.push(McpServerConfig { name, spec, scope, disabled: entry.disabled });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// Load + merge `.mcp.json` configs from the project path (cwd) and the
/// user-global path, with project entries taking precedence on name
/// collision. Missing files yield empty lists; parse errors are returned
/// via the inner `Result` (callers typically `tracing::warn!` and skip).
pub fn load_mcp_json_with_precedence(
    project_path: &Path,
    global_path: &Path,
) -> Vec<McpServerConfig> {
    let mut by_name: HashMap<String, McpServerConfig> = HashMap::new();

    // User-global first (lower precedence).
    if let Ok(raw) = std::fs::read_to_string(global_path) {
        match parse_mcp_json_string(&raw, ConfigScope::User) {
            Ok(cfgs) => {
                for c in cfgs {
                    by_name.insert(c.name.clone(), c);
                }
            }
            Err(e) => tracing::warn!(error = %e, path = %global_path.display(), "skipping malformed user mcp.json"),
        }
    }

    // Project overrides.
    if let Ok(raw) = std::fs::read_to_string(project_path) {
        match parse_mcp_json_string(&raw, ConfigScope::Project) {
            Ok(cfgs) => {
                for c in cfgs {
                    by_name.insert(c.name.clone(), c);
                }
            }
            Err(e) => tracing::warn!(error = %e, path = %project_path.display(), "skipping malformed project .mcp.json"),
        }
    }

    let mut out: Vec<McpServerConfig> = by_name.into_values().collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use std::fs;

    #[test]
    fn parse_empty_json_yields_no_servers() {
        let cfgs = parse_mcp_json_string("{}", ConfigScope::Project).unwrap();
        assert!(cfgs.is_empty());
    }

    #[test]
    fn parse_two_stdio_servers() {
        let raw = r#"{
          "mcpServers": {
            "memory":     { "command": "mcp-memory",     "args": [],         "env": {} },
            "filesystem": { "command": "mcp-filesystem", "args": ["/tmp"],   "env": {} }
          }
        }"#;
        let cfgs = parse_mcp_json_string(raw, ConfigScope::Project).unwrap();
        assert_eq!(cfgs.len(), 2);
        // Sorted by name.
        assert_eq!(cfgs[0].name, "filesystem");
        assert_eq!(cfgs[1].name, "memory");
        match &cfgs[0].spec {
            McpTransportSpec::Stdio { command, args, .. } => {
                assert_eq!(command, "mcp-filesystem");
                assert_eq!(args, &vec!["/tmp".to_string()]);
            }
            other => panic!("expected Stdio, got {other:?}"),
        }
    }

    #[test]
    fn project_overrides_user_on_name_collision() {
        let dir = TempDir::new().unwrap();
        let project = dir.path().join(".mcp.json");
        let global = dir.path().join("user-mcp.json");
        fs::write(&global, r#"{"mcpServers":{"x":{"command":"global-x"}}}"#).unwrap();
        fs::write(&project, r#"{"mcpServers":{"x":{"command":"project-x"}}}"#).unwrap();

        let cfgs = load_mcp_json_with_precedence(&project, &global);
        assert_eq!(cfgs.len(), 1);
        assert_eq!(cfgs[0].scope, ConfigScope::Project);
        match &cfgs[0].spec {
            McpTransportSpec::Stdio { command, .. } => assert_eq!(command, "project-x"),
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn malformed_user_file_is_skipped_silently() {
        let dir = TempDir::new().unwrap();
        let project = dir.path().join(".mcp.json");
        let global = dir.path().join("user-mcp.json");
        fs::write(&global, "{ not json").unwrap();
        fs::write(&project, r#"{"mcpServers":{"y":{"command":"y"}}}"#).unwrap();
        let cfgs = load_mcp_json_with_precedence(&project, &global);
        assert_eq!(cfgs.len(), 1);
        assert_eq!(cfgs[0].name, "y");
    }
}
```

(If `McpTransportSpec::Sse { url, headers }` / `Http { url, headers }` field names differ, run `rg -n "pub enum McpTransportSpec" lingxi-code/crates/traits/src/` and adjust. `tempfile` is already a workspace dev-dep.)

- [ ] **Step 2: Run; expect FAIL.**

```bash
cargo test -p lingxi-mcp json_config::tests --no-run
```

- [ ] **Step 3: Re-export.**

In `crates/mcp/src/lib.rs`:
```rust
pub mod json_config;
pub use json_config::{load_mcp_json_with_precedence, parse_mcp_json_string, McpJsonError};
```

Add to `crates/mcp/Cargo.toml`:
```toml
serde_json = { workspace = true }
thiserror = { workspace = true }
tracing = { workspace = true }
```
(Likely already present — verify with `cargo tree -p lingxi-mcp` before adding duplicates.)

Add to `[dev-dependencies]`:
```toml
tempfile = { workspace = true }
```
(Likely already present.)

- [ ] **Step 4: Run; expect PASS.**

```bash
cargo test -p lingxi-mcp json_config::tests
```
Expected: 4 passing.

- [ ] **Step 5: Commit.**

```bash
git add lingxi-code/crates/mcp/src/json_config.rs lingxi-code/crates/mcp/src/lib.rs lingxi-code/crates/mcp/Cargo.toml
git commit -m "feat(mcp): .mcp.json parser + precedence loader (M6-07)"
```

---

## Task 6: `lingxi_agent::catalog::load_agents_from_dirs`

**Files:**
- Create: `lingxi-code/crates/agent/src/catalog.rs`
- Modify: `lingxi-code/crates/agent/src/lib.rs`
- Modify: `lingxi-code/crates/agent/Cargo.toml` — add `gray_matter = { workspace = true }` and `serde_yaml = { workspace = true }` if missing.
- Test: `lingxi-code/crates/agent/src/catalog.rs` (inline `#[cfg(test)]`)

- [ ] **Step 1: Write the failing test.**

Create `crates/agent/src/catalog.rs`:

```rust
//! Agent catalog loader (M6-07).
//!
//! Reads markdown files from one or more `agents/` directories, parses the
//! YAML frontmatter, and projects each file into an
//! [`crate::AgentDefinition`].
//!
//! Frontmatter shape (claude-code compatible — subset relevant to v0.7.0):
//! ```yaml
//! ---
//! name: reviewer
//! description: Reviews code for security and correctness.
//! tools: [Read, Grep, Bash]
//! model: sonnet
//! ---
//! Body of the system prompt goes here.
//! ```

use crate::definition::{
    AgentDefinition, AgentModel, AgentPermissionMode, AgentSource, AgentToolPolicy,
};
use gray_matter::engine::YAML;
use gray_matter::Matter;
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Errors raised while loading an agent file.
#[derive(Debug, thiserror::Error)]
pub enum AgentLoadError {
    /// File could not be read.
    #[error("read {path}: {source}")]
    Io { path: PathBuf, #[source] source: std::io::Error },
    /// Frontmatter was missing or malformed.
    #[error("no valid frontmatter in {0}")]
    NoFrontmatter(PathBuf),
    /// Frontmatter YAML failed to deserialize.
    #[error("deserialize frontmatter {path}: {source}")]
    Yaml { path: PathBuf, #[source] source: serde_yaml::Error },
}

/// Subset of an agent's YAML frontmatter we read at v0.7.0.
#[derive(Debug, Deserialize)]
struct Frontmatter {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    tools: Vec<String>,
    #[serde(default)]
    model: Option<String>,
}

/// Parse a single agent markdown buffer.
///
/// `source` and `base_dir` are supplied by the caller (so the loader can
/// tag files coming from `~/.claude/agents/` differently from project
/// files). `path_for_error` is used purely for error messages.
pub fn parse_agent_markdown(
    raw: &str,
    source: AgentSource,
    base_dir: PathBuf,
    path_for_error: &Path,
) -> Result<AgentDefinition, AgentLoadError> {
    let matter = Matter::<YAML>::new();
    let parsed = matter.parse(raw);
    let fm_raw = parsed
        .data
        .ok_or_else(|| AgentLoadError::NoFrontmatter(path_for_error.to_path_buf()))?;
    let fm: Frontmatter = fm_raw
        .deserialize()
        .map_err(|e| AgentLoadError::Yaml { path: path_for_error.to_path_buf(), source: e })?;

    let name = fm.name.unwrap_or_else(|| {
        // Fallback: filename stem.
        path_for_error
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("agent")
            .to_string()
    });
    let description = fm.description.unwrap_or_default();
    let tools_policy = if fm.tools.is_empty() {
        AgentToolPolicy::All { use_exact_tools: false }
    } else {
        AgentToolPolicy::Explicit(fm.tools.clone())
    };
    let model = fm
        .model
        .map_or(AgentModel::Inherit, AgentModel::Alias);

    Ok(AgentDefinition {
        agent_type: name,
        when_to_use: description,
        tools: tools_policy,
        max_turns: 100,
        model,
        permission_mode: AgentPermissionMode::Bubble,
        source,
        base_dir,
        system_prompt: Some(parsed.content),
        mcp_servers: Vec::new(),
        frontmatter_hooks: Vec::new(),
        icon: None,
        allowed_tools: fm.tools,
        worktree_requirement: None,
    })
}

/// Load every `*.md` agent file under each path in `paths`, in order.
///
/// Files with no frontmatter or invalid YAML are logged at `warn!` and
/// skipped. On `agent_type` collision, **later paths win** — pass the
/// global path FIRST and the project path SECOND so project agents
/// override user-globals (project files take precedence).
pub async fn load_agents_from_dirs(paths: &[(PathBuf, AgentSource)]) -> Vec<AgentDefinition> {
    use std::collections::HashMap;
    let mut by_name: HashMap<String, AgentDefinition> = HashMap::new();
    for (dir, source) in paths {
        let mut entries = match tokio::fs::read_dir(dir).await {
            Ok(e) => e,
            Err(_) => continue, // missing dir = empty contribution
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let p = entry.path();
            if p.extension().and_then(|s| s.to_str()) != Some("md") {
                continue;
            }
            let raw = match tokio::fs::read_to_string(&p).await {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(error = %e, path = %p.display(), "skipping unreadable agent file");
                    continue;
                }
            };
            match parse_agent_markdown(&raw, *source, dir.clone(), &p) {
                Ok(def) => {
                    by_name.insert(def.agent_type.clone(), def);
                }
                Err(e) => {
                    tracing::warn!(error = %e, path = %p.display(), "skipping malformed agent file");
                }
            }
        }
    }
    let mut out: Vec<AgentDefinition> = by_name.into_values().collect();
    out.sort_by(|a, b| a.agent_type.cmp(&b.agent_type));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn parse_minimal_frontmatter() {
        let raw = "---\nname: reviewer\ndescription: review code\n---\nBody";
        let def = parse_agent_markdown(
            raw,
            AgentSource::UserDefined,
            PathBuf::from("/tmp"),
            Path::new("reviewer.md"),
        )
        .unwrap();
        assert_eq!(def.agent_type, "reviewer");
        assert_eq!(def.when_to_use, "review code");
    }

    #[test]
    fn missing_frontmatter_errors() {
        let raw = "no frontmatter here";
        let err = parse_agent_markdown(
            raw,
            AgentSource::UserDefined,
            PathBuf::from("/tmp"),
            Path::new("x.md"),
        )
        .unwrap_err();
        assert!(matches!(err, AgentLoadError::NoFrontmatter(_)));
    }

    #[test]
    fn frontmatter_tools_become_explicit_policy() {
        let raw = "---\nname: r\ndescription: d\ntools: [Read, Grep]\n---\n";
        let def = parse_agent_markdown(
            raw,
            AgentSource::Project,
            PathBuf::from("/tmp"),
            Path::new("r.md"),
        )
        .unwrap();
        match &def.tools {
            AgentToolPolicy::Explicit(v) => assert_eq!(v, &vec!["Read".to_string(), "Grep".to_string()]),
            other => panic!("expected Explicit, got {other:?}"),
        }
        assert_eq!(def.allowed_tools, vec!["Read".to_string(), "Grep".to_string()]);
    }

    #[tokio::test]
    async fn load_agents_from_dirs_merges_user_and_project() {
        let dir = TempDir::new().unwrap();
        let user = dir.path().join("user");
        let project = dir.path().join("project");
        tokio::fs::create_dir_all(&user).await.unwrap();
        tokio::fs::create_dir_all(&project).await.unwrap();
        tokio::fs::write(
            user.join("alpha.md"),
            "---\nname: alpha\ndescription: from user\n---\n",
        )
        .await
        .unwrap();
        tokio::fs::write(
            project.join("alpha.md"),
            "---\nname: alpha\ndescription: from project\n---\n",
        )
        .await
        .unwrap();
        tokio::fs::write(
            project.join("beta.md"),
            "---\nname: beta\ndescription: project-only\n---\n",
        )
        .await
        .unwrap();

        let defs = load_agents_from_dirs(&[
            (user.clone(), AgentSource::UserDefined),
            (project.clone(), AgentSource::Project),
        ])
        .await;

        assert_eq!(defs.len(), 2);
        // Sorted alphabetically.
        assert_eq!(defs[0].agent_type, "alpha");
        assert_eq!(defs[0].when_to_use, "from project"); // project wins
        assert_eq!(defs[1].agent_type, "beta");
    }
}
```

(If `gray_matter::engine::YAML` import path differs in the version pinned in workspace `Cargo.toml`, run `cargo doc -p gray_matter --no-deps --open` or check `lingxi-skills/Cargo.toml` for the existing version. `Matter::<YAML>::new` is the v0.2 API. If using a different version, adjust per `lingxi-skills/src/frontmatter.rs` which already exercises this dep.)

- [ ] **Step 2: Run; expect FAIL.**

```bash
cargo test -p lingxi-agent catalog::tests --no-run
```

- [ ] **Step 3: Re-export + Cargo deps.**

In `crates/agent/src/lib.rs`:
```rust
pub mod catalog;
pub use catalog::{load_agents_from_dirs, parse_agent_markdown, AgentLoadError};
```

In `crates/agent/Cargo.toml`, ensure under `[dependencies]`:
```toml
gray_matter = { workspace = true }
serde_yaml = { workspace = true }
tokio = { workspace = true, features = ["fs"] }
tracing = { workspace = true }
thiserror = { workspace = true }
```
And under `[dev-dependencies]`:
```toml
tempfile = { workspace = true }
```

(Confirm by reading `lingxi-skills/Cargo.toml` which has the same deps already; copy-paste the version constraints.)

- [ ] **Step 4: Run; expect PASS.**

```bash
cargo test -p lingxi-agent catalog::tests
```
Expected: 4 passing.

- [ ] **Step 5: Commit.**

```bash
git add lingxi-code/crates/agent/src/catalog.rs lingxi-code/crates/agent/src/lib.rs lingxi-code/crates/agent/Cargo.toml
git commit -m "feat(agent): catalog loader for ~/.claude/agents/ + project agents (M6-07)"
```

---

## Task 7: Add registry fields + builders on `ConversationOrchestrator`

**Files:**
- Modify: `lingxi-code/crates/orchestrator/src/conversation.rs`
- Modify: `lingxi-code/crates/orchestrator/Cargo.toml` — add `lingxi-mcp = { path = "../mcp" }`, `lingxi-agent = { path = "../agent" }` (lingxi-hooks already present).
- Test: `lingxi-code/crates/orchestrator/src/conversation.rs` (inline `#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the failing test.**

In `crates/orchestrator/src/conversation.rs` test module (or create `crates/orchestrator/tests/conversation_registries_field.rs`):

```rust
#[tokio::test]
async fn with_mcp_hook_agent_builders_store_fields() {
    use crate::test_support::{
        noop_hook_executor, MockApiClient, MockOutputStream, NoOpPermissionGate,
        StaticMemoryProvider,
    };
    use crate::config::OrchestratorConfig;
    use lingxi_agent::AgentDefinition;
    use lingxi_hooks::HookRegistry;
    use lingxi_mcp::McpRegistry;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    let api = Arc::new(MockApiClient::new());
    let tools = Arc::new(lingxi_tools::registry::ToolRegistry::new());
    let hooks = noop_hook_executor();
    let perms = Arc::new(NoOpPermissionGate);
    let output = Arc::new(MockOutputStream::new());
    let memory = Arc::new(StaticMemoryProvider::empty());

    // Construct empty registries — only the wiring is under test.
    struct StubTransport;
    #[async_trait::async_trait]
    impl lingxi_traits::McpTransport for StubTransport {
        async fn connect(&self, _s: &lingxi_traits::McpTransportSpec) -> Result<lingxi_traits::McpConnection, lingxi_traits::McpError> { unreachable!() }
        async fn initialize(&self, _c: &lingxi_traits::McpConnection) -> Result<lingxi_traits::ServerCapabilitiesDto, lingxi_traits::McpError> { unreachable!() }
        async fn list_tools(&self, _c: &lingxi_traits::McpConnection) -> Result<Vec<lingxi_traits::McpToolDto>, lingxi_traits::McpError> { unreachable!() }
        async fn list_resources(&self, _c: &lingxi_traits::McpConnection) -> Result<Vec<lingxi_traits::McpResourceDto>, lingxi_traits::McpError> { unreachable!() }
        async fn list_prompts(&self, _c: &lingxi_traits::McpConnection) -> Result<Vec<lingxi_traits::McpPromptDto>, lingxi_traits::McpError> { unreachable!() }
        async fn disconnect(&self, _id: lingxi_protocol::McpConnectionId) -> Result<(), lingxi_traits::McpError> { unreachable!() }
    }
    let mcp = Arc::new(McpRegistry::new(Arc::new(StubTransport)));
    let hook_reg = Arc::new(RwLock::new(HookRegistry::new()));
    let agents = Arc::new(RwLock::new(Vec::<AgentDefinition>::new()));

    let orch = ConversationOrchestrator::new(
        OrchestratorConfig::default(), api, tools, hooks, perms, output, memory,
        std::env::temp_dir(),
    )
    .with_mcp_registry(mcp.clone())
    .with_hook_registry(hook_reg.clone())
    .with_agent_catalog(agents.clone());

    assert!(orch.mcp_registry.is_some());
    assert!(orch.hook_registry.is_some());
    assert!(orch.agent_catalog.is_some());
}
```

- [ ] **Step 2: Run; expect FAIL (no fields, no builders).**

```bash
cargo test -p lingxi-orchestrator with_mcp_hook_agent_builders_store_fields --no-run
```

- [ ] **Step 3: Add the fields + builders.**

In `crates/orchestrator/src/conversation.rs`, append to the `ConversationOrchestrator` struct (after `should_exit`):

```rust
    /// MCP registry (M2-02b). `None` when not wired — `list_mcp_servers`
    /// then returns `vec![]`. The CLI binary (M6-07 init.rs) populates
    /// this from `.mcp.json` + `~/.config/lingxi/mcp.json`.
    pub(crate) mcp_registry: Option<Arc<lingxi_mcp::McpRegistry>>,
    /// Hook registry (M5-06). `None` when not wired — `list_hooks` then
    /// returns `vec![]`. CLI binary populates from settings + plugin
    /// sources.
    pub(crate) hook_registry: Option<Arc<tokio::sync::RwLock<lingxi_hooks::HookRegistry>>>,
    /// Subagent catalog (M6-07). `None` when not wired — `list_agents`
    /// then returns `vec![]`. CLI binary populates from `~/.claude/agents/`
    /// + project `.claude/agents/`.
    pub(crate) agent_catalog:
        Option<Arc<tokio::sync::RwLock<Vec<lingxi_agent::AgentDefinition>>>>,
```

In both `new_with_streaming` and `new` constructors, add to the struct literal:
```rust
            mcp_registry: None,
            hook_registry: None,
            agent_catalog: None,
```

Add the three builders on `impl ConversationOrchestrator` (after `with_jsonl_writer`):

```rust
    /// Attach an MCP registry so `list_mcp_servers` reports real data.
    #[must_use]
    pub fn with_mcp_registry(mut self, mcp: Arc<lingxi_mcp::McpRegistry>) -> Self {
        self.mcp_registry = Some(mcp);
        self
    }

    /// Attach a hook registry so `list_hooks` reports real data.
    #[must_use]
    pub fn with_hook_registry(
        mut self,
        hooks: Arc<tokio::sync::RwLock<lingxi_hooks::HookRegistry>>,
    ) -> Self {
        self.hook_registry = Some(hooks);
        self
    }

    /// Attach an agent catalog so `list_agents` reports real data.
    #[must_use]
    pub fn with_agent_catalog(
        mut self,
        agents: Arc<tokio::sync::RwLock<Vec<lingxi_agent::AgentDefinition>>>,
    ) -> Self {
        self.agent_catalog = Some(agents);
        self
    }
```

Add the deps to `crates/orchestrator/Cargo.toml`:
```toml
lingxi-mcp = { path = "../mcp" }
lingxi-agent = { path = "../agent" }
# lingxi-hooks already present
```

- [ ] **Step 4: Run; expect PASS.**

```bash
cargo test -p lingxi-orchestrator with_mcp_hook_agent_builders_store_fields
```

- [ ] **Step 5: Commit.**

```bash
git add lingxi-code/crates/orchestrator/src/conversation.rs lingxi-code/crates/orchestrator/Cargo.toml
git commit -m "feat(orchestrator): add mcp/hook/agent registry fields + builders (M6-07)"
```

---

## Task 8: Real `list_mcp_servers` / `list_hooks` / `list_agents`

**Files:**
- Modify: `lingxi-code/crates/orchestrator/src/handle_impl.rs:106-127`
- Create: `lingxi-code/crates/orchestrator/tests/list_mcp_real.rs`
- Create: `lingxi-code/crates/orchestrator/tests/list_hooks_real.rs`
- Create: `lingxi-code/crates/orchestrator/tests/list_agents_real.rs`

- [ ] **Step 1: Write the failing tests.**

Create `crates/orchestrator/tests/list_mcp_real.rs`:

```rust
//! M6-07 — `list_mcp_servers` reads `Arc<McpRegistry>` when wired.

use lingxi_mcp::{ConfigScope, McpRegistry, McpServerConfig};
use lingxi_orchestrator::test_support::{
    noop_hook_executor, MockApiClient, MockOutputStream, NoOpPermissionGate, StaticMemoryProvider,
};
use lingxi_orchestrator::{ConversationOrchestrator, OrchestratorConfig};
use lingxi_traits::{McpStatus, McpTransportSpec, OrchestratorHandle};
use std::sync::Arc;

struct StubTransport;
#[async_trait::async_trait]
impl lingxi_traits::McpTransport for StubTransport {
    async fn connect(&self, _s: &McpTransportSpec) -> Result<lingxi_traits::McpConnection, lingxi_traits::McpError> { unreachable!() }
    async fn initialize(&self, _c: &lingxi_traits::McpConnection) -> Result<lingxi_traits::ServerCapabilitiesDto, lingxi_traits::McpError> { unreachable!() }
    async fn list_tools(&self, _c: &lingxi_traits::McpConnection) -> Result<Vec<lingxi_traits::McpToolDto>, lingxi_traits::McpError> { unreachable!() }
    async fn list_resources(&self, _c: &lingxi_traits::McpConnection) -> Result<Vec<lingxi_traits::McpResourceDto>, lingxi_traits::McpError> { unreachable!() }
    async fn list_prompts(&self, _c: &lingxi_traits::McpConnection) -> Result<Vec<lingxi_traits::McpPromptDto>, lingxi_traits::McpError> { unreachable!() }
    async fn disconnect(&self, _id: lingxi_protocol::McpConnectionId) -> Result<(), lingxi_traits::McpError> { unreachable!() }
}

fn stdio_cfg(name: &str) -> McpServerConfig {
    McpServerConfig {
        name: name.into(),
        spec: McpTransportSpec::Stdio {
            command: "echo".into(),
            args: vec![],
            env: Default::default(),
            cwd: None,
        },
        scope: ConfigScope::Project,
        disabled: false,
    }
}

fn build_orch_with_registry(reg: Arc<McpRegistry>) -> Arc<ConversationOrchestrator> {
    Arc::new(
        ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new()),
            Arc::new(lingxi_tools::registry::ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        )
        .with_mcp_registry(reg),
    )
}

#[tokio::test]
async fn list_mcp_servers_returns_empty_when_no_registry() {
    let orch = Arc::new(ConversationOrchestrator::new(
        OrchestratorConfig::default(),
        Arc::new(MockApiClient::new()),
        Arc::new(lingxi_tools::registry::ToolRegistry::new()),
        noop_hook_executor(),
        Arc::new(NoOpPermissionGate),
        Arc::new(MockOutputStream::new()),
        Arc::new(StaticMemoryProvider::empty()),
        std::env::temp_dir(),
    ));
    let v = orch.list_mcp_servers().await;
    assert!(v.is_empty());
}

#[tokio::test]
async fn list_mcp_servers_returns_two_when_two_registered() {
    let reg = Arc::new(McpRegistry::new(Arc::new(StubTransport)));
    // Seed two Disconnected servers (no transport call required).
    {
        let mut c = reg.connections.write().await;
        c.insert(
            "memory".into(),
            lingxi_mcp::McpConnectionState::Disconnected {
                config: stdio_cfg("memory"),
                last_error: None,
            },
        );
        c.insert(
            "filesystem".into(),
            lingxi_mcp::McpConnectionState::Disconnected {
                config: stdio_cfg("filesystem"),
                last_error: None,
            },
        );
    }
    let orch = build_orch_with_registry(reg);
    let v = orch.list_mcp_servers().await;
    assert_eq!(v.len(), 2);
    // Sorted by name.
    assert_eq!(v[0].name, "filesystem");
    assert_eq!(v[0].status, McpStatus::Disconnected);
    assert_eq!(v[0].transport, "stdio");
    assert_eq!(v[1].name, "memory");
}
```

Create `crates/orchestrator/tests/list_hooks_real.rs`:

```rust
//! M6-07 — `list_hooks` reads `Arc<RwLock<HookRegistry>>` when wired.

use lingxi_hooks::definition::{HookExecutor, HookSource};
use lingxi_hooks::events::HookEventType;
use lingxi_hooks::{HookDefinition, HookRegistry};
use lingxi_orchestrator::test_support::{
    noop_hook_executor, MockApiClient, MockOutputStream, NoOpPermissionGate, StaticMemoryProvider,
};
use lingxi_orchestrator::{ConversationOrchestrator, OrchestratorConfig};
use lingxi_protocol::HookId;
use lingxi_traits::OrchestratorHandle;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

fn hk(name: &str, event: HookEventType, matcher: Option<&str>, timeout: Option<Duration>) -> HookDefinition {
    HookDefinition {
        id: HookId::new(),
        name: name.into(),
        events: vec![event],
        if_condition: matcher.map(|m| lingxi_hooks::definition::HookCondition {
            pattern: m.into(),
            match_tool_name: true,
            match_input: false,
        }),
        executor: HookExecutor::Builtin { handler_id: "noop".into() },
        source: HookSource::User,
        blocking: true,
        timeout,
        priority: 0,
    }
}

#[tokio::test]
async fn list_hooks_returns_empty_when_no_registry() {
    let orch = Arc::new(ConversationOrchestrator::new(
        OrchestratorConfig::default(),
        Arc::new(MockApiClient::new()),
        Arc::new(lingxi_tools::registry::ToolRegistry::new()),
        noop_hook_executor(),
        Arc::new(NoOpPermissionGate),
        Arc::new(MockOutputStream::new()),
        Arc::new(StaticMemoryProvider::empty()),
        std::env::temp_dir(),
    ));
    assert!(orch.list_hooks().await.is_empty());
}

#[tokio::test]
async fn list_hooks_returns_one_pretooluse_entry() {
    let mut reg = HookRegistry::new();
    reg.register(hk("./fmt.sh", HookEventType::PreToolUse, Some("Write|Edit"), Some(Duration::from_secs(30))));
    let reg = Arc::new(RwLock::new(reg));

    let orch = Arc::new(
        ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new()),
            Arc::new(lingxi_tools::registry::ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        )
        .with_hook_registry(reg),
    );

    let v = orch.list_hooks().await;
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].name, "./fmt.sh");
    assert_eq!(v[0].event, "PreToolUse");
    assert_eq!(v[0].matcher.as_deref(), Some("Write|Edit"));
    assert_eq!(v[0].timeout_ms, 30_000);
}

#[tokio::test]
async fn list_hooks_default_timeout_is_60000ms() {
    let mut reg = HookRegistry::new();
    reg.register(hk("./x.sh", HookEventType::Stop, None, None));
    let reg = Arc::new(RwLock::new(reg));

    let orch = Arc::new(
        ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new()),
            Arc::new(lingxi_tools::registry::ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        )
        .with_hook_registry(reg),
    );

    let v = orch.list_hooks().await;
    assert_eq!(v[0].timeout_ms, 60_000);
}
```

Create `crates/orchestrator/tests/list_agents_real.rs`:

```rust
//! M6-07 — `list_agents` reads `Arc<RwLock<Vec<AgentDefinition>>>` when wired.

use lingxi_agent::definition::{
    AgentDefinition, AgentModel, AgentPermissionMode, AgentSource, AgentToolPolicy,
};
use lingxi_orchestrator::test_support::{
    noop_hook_executor, MockApiClient, MockOutputStream, NoOpPermissionGate, StaticMemoryProvider,
};
use lingxi_orchestrator::{ConversationOrchestrator, OrchestratorConfig};
use lingxi_traits::OrchestratorHandle;
use std::sync::Arc;
use tokio::sync::RwLock;

fn mk(name: &str, desc: &str, tools: Vec<String>) -> AgentDefinition {
    AgentDefinition {
        agent_type: name.into(),
        when_to_use: desc.into(),
        tools: AgentToolPolicy::Explicit(tools.clone()),
        max_turns: 100,
        model: AgentModel::Inherit,
        permission_mode: AgentPermissionMode::Bubble,
        source: AgentSource::UserDefined,
        base_dir: std::path::PathBuf::from("/tmp"),
        system_prompt: None,
        mcp_servers: vec![],
        frontmatter_hooks: vec![],
        icon: None,
        allowed_tools: tools,
        worktree_requirement: None,
    }
}

#[tokio::test]
async fn list_agents_returns_empty_when_no_catalog() {
    let orch = Arc::new(ConversationOrchestrator::new(
        OrchestratorConfig::default(),
        Arc::new(MockApiClient::new()),
        Arc::new(lingxi_tools::registry::ToolRegistry::new()),
        noop_hook_executor(),
        Arc::new(NoOpPermissionGate),
        Arc::new(MockOutputStream::new()),
        Arc::new(StaticMemoryProvider::empty()),
        std::env::temp_dir(),
    ));
    assert!(orch.list_agents().await.is_empty());
}

#[tokio::test]
async fn list_agents_returns_one_entry() {
    let cat = Arc::new(RwLock::new(vec![mk("reviewer", "Reviews code", vec!["Read".into(), "Grep".into()])]));
    let orch = Arc::new(
        ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new()),
            Arc::new(lingxi_tools::registry::ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        )
        .with_agent_catalog(cat),
    );
    let v = orch.list_agents().await;
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].name, "reviewer");
    assert_eq!(v[0].description, "Reviews code");
    assert_eq!(v[0].tools_allowed, vec!["Read".to_string(), "Grep".to_string()]);
}
```

- [ ] **Step 2: Run; expect FAIL.**

```bash
cargo test -p lingxi-orchestrator --test list_mcp_real --test list_hooks_real --test list_agents_real --no-run
```
Expected: COMPILE OK, RUNTIME FAIL — the three `list_*` methods still return `vec![]`.

- [ ] **Step 3: Replace the three `list_*` bodies.**

In `crates/orchestrator/src/handle_impl.rs`, replace lines 106-127 with:

```rust
    async fn list_mcp_servers(&self) -> Vec<McpServerInfo> {
        let Some(reg) = self.mcp_registry.as_ref() else {
            return Vec::new();
        };
        reg.snapshot().await
    }

    async fn list_hooks(&self) -> Vec<HookInfo> {
        let Some(reg) = self.hook_registry.as_ref() else {
            return Vec::new();
        };
        let g = reg.read().await;
        let mut out: Vec<HookInfo> = g
            .all_hooks()
            .into_iter()
            .map(|h| HookInfo {
                name: h.name.clone(),
                event: format!("{:?}", h.events.first().copied().unwrap_or_default()),
                matcher: h.if_condition.as_ref().map(|c| c.pattern.clone()),
                timeout_ms: h
                    .timeout
                    .map_or(60_000_u64, |d| d.as_millis().min(u64::MAX as u128) as u64),
            })
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    async fn list_agents(&self) -> Vec<AgentInfo> {
        let Some(cat) = self.agent_catalog.as_ref() else {
            return Vec::new();
        };
        let g = cat.read().await;
        let mut out: Vec<AgentInfo> = g
            .iter()
            .map(|a| AgentInfo {
                name: a.agent_type.clone(),
                description: a.when_to_use.clone(),
                tools_allowed: a.allowed_tools.clone(),
            })
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }
```

Note: `HookEventType` does not implement `Default` (and `events: vec![]` would be an empty hook anyway — registration always supplies at least one). If `format!("{:?}", e)` does not yield the exact `"PreToolUse"` / `"PostToolUse"` string we need, define a `to_str` helper on `HookEventType` or match manually:

```rust
fn event_str(et: lingxi_hooks::events::HookEventType) -> &'static str {
    use lingxi_hooks::events::HookEventType as E;
    match et {
        E::PreToolUse => "PreToolUse",
        E::PostToolUse => "PostToolUse",
        E::Stop => "Stop",
        E::Notification => "Notification",
        E::UserPromptSubmit => "UserPromptSubmit",
        E::SubagentStop => "SubagentStop",
        E::PreCompact => "PreCompact",
        E::SessionStart => "SessionStart",
    }
}
```

Drop the `format!("{:?}", …)` path in favour of this helper to guarantee a stable string regardless of `Debug` derive output.

- [ ] **Step 4: Run; expect PASS.**

```bash
cargo test -p lingxi-orchestrator --test list_mcp_real
cargo test -p lingxi-orchestrator --test list_hooks_real
cargo test -p lingxi-orchestrator --test list_agents_real
```
Expected: 6 passing across the three test files.

- [ ] **Step 5: Commit.**

```bash
git add lingxi-code/crates/orchestrator/src/handle_impl.rs lingxi-code/crates/orchestrator/tests/list_mcp_real.rs lingxi-code/crates/orchestrator/tests/list_hooks_real.rs lingxi-code/crates/orchestrator/tests/list_agents_real.rs
git commit -m "feat(orchestrator): real list_mcp_servers / list_hooks / list_agents (M6-07)"
```

---

## Task 9: Empty-state literals in `/mcp`, `/hooks`, `/agents` handlers

**Files:**
- Modify: `lingxi-code/crates/commands/src/builtin/mcp.rs`
- Modify: `lingxi-code/crates/commands/src/builtin/hooks.rs`
- Modify: `lingxi-code/crates/commands/src/builtin/agents.rs`
- Modify: existing tests in those files

- [ ] **Step 1: Write the failing test.**

Replace the `empty_list` test in each of `mcp.rs`, `hooks.rs`, `agents.rs` with the new expected literals. In `crates/commands/src/builtin/mcp.rs::tests`:

```rust
    #[tokio::test]
    async fn empty_list() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        let h = McpHandler::new(mock);
        if let CommandResult::Done { display: Some(s) } = h.handle(&args()).await {
            assert_eq!(s, "No MCP servers configured\n");
        } else {
            panic!();
        }
    }
```

In `crates/commands/src/builtin/hooks.rs::tests`:
```rust
    #[tokio::test]
    async fn empty_list() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        let h = HooksHandler::new(mock);
        if let CommandResult::Done { display: Some(s) } = h.handle(&args()).await {
            assert_eq!(s, "No hooks configured\n");
        } else {
            panic!();
        }
    }
```

In `crates/commands/src/builtin/agents.rs::tests`:
```rust
    #[tokio::test]
    async fn empty_list() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        let h = AgentsHandler::new(mock);
        if let CommandResult::Done { display: Some(s) } = h.handle(&args()).await {
            assert_eq!(s, "No subagents configured\n");
        } else {
            panic!();
        }
    }
```

- [ ] **Step 2: Run; expect FAIL (still emit `"<Label> (0):\n"`).**

```bash
cargo test -p lingxi-commands builtin::mcp::tests::empty_list builtin::hooks::tests::empty_list builtin::agents::tests::empty_list
```

- [ ] **Step 3: Update each handler to supply the empty-state literal.**

In `crates/commands/src/builtin/mcp.rs::handle`, change the `render_list` call:
```rust
        let s = render_list("MCP servers", rows, "No MCP servers configured");
```

In `crates/commands/src/builtin/hooks.rs::handle`:
```rust
        let s = render_list("Hooks", rows, "No hooks configured");
```

In `crates/commands/src/builtin/agents.rs::handle`:
```rust
        let s = render_list("Agents", rows, "No subagents configured");
```

- [ ] **Step 4: Run; expect PASS.**

```bash
cargo test -p lingxi-commands builtin::mcp::tests::empty_list builtin::hooks::tests::empty_list builtin::agents::tests::empty_list
# Also rerun the multi-row tests to confirm no regression in the non-empty path.
cargo test -p lingxi-commands builtin::mcp:: builtin::hooks:: builtin::agents::
```
Expected: all green.

- [ ] **Step 5: Commit.**

```bash
git add lingxi-code/crates/commands/src/builtin/mcp.rs lingxi-code/crates/commands/src/builtin/hooks.rs lingxi-code/crates/commands/src/builtin/agents.rs
git commit -m "feat(commands): /mcp /hooks /agents empty-state literals (M6-07)"
```

---

## Task 10: CLI `build_runtime` wires the three registries

**Files:**
- Modify: `lingxi-code/crates/cli/src/init.rs`
- Modify: `lingxi-code/crates/cli/Cargo.toml` — add `lingxi-mcp = { path = "../mcp" }`, `lingxi-hooks = { path = "../hooks" }`, `lingxi-agent = { path = "../agent" }`.
- Test: `lingxi-code/crates/cli/src/init.rs` (existing `build_runtime_with_defaults`)

- [ ] **Step 1: Extend the test.**

Add three accessors on `ConversationOrchestrator` to keep field visibility tight (similar to M6-08's `has_compaction`):

```rust
    #[must_use]
    pub fn has_mcp_registry(&self) -> bool { self.mcp_registry.is_some() }
    #[must_use]
    pub fn has_hook_registry(&self) -> bool { self.hook_registry.is_some() }
    #[must_use]
    pub fn has_agent_catalog(&self) -> bool { self.agent_catalog.is_some() }
```

In `crates/cli/src/init.rs::tests`, replace the existing `build_runtime_with_defaults` body's tail with:

```rust
        let r = build_runtime(&argv, output).await.unwrap();
        assert!(r.orchestrator.has_mcp_registry(),  "build_runtime did not wire McpRegistry");
        assert!(r.orchestrator.has_hook_registry(), "build_runtime did not wire HookRegistry");
        assert!(r.orchestrator.has_agent_catalog(), "build_runtime did not wire agent catalog");
```

- [ ] **Step 2: Run; expect FAIL.**

```bash
cargo test -p lingxi-cli build_runtime_with_defaults
```

- [ ] **Step 3: Wire the registries in `build_runtime`.**

In `crates/cli/src/init.rs`, replace the section between step (5) and step (6) (currently the orchestrator + dispatcher construction). After `let cwd = …` and before the `let orch = Arc::new(ConversationOrchestrator::new(...))`:

```rust
    // M6-07: MCP registry. Honour `.mcp.json` (project) over `~/.config/lingxi/mcp.json` (user).
    let global_mcp_path = dirs::config_dir()
        .map(|d| d.join("lingxi").join("mcp.json"))
        .unwrap_or_else(|| std::path::PathBuf::from("/dev/null"));
    let project_mcp_path = cwd.join(".mcp.json");
    let mcp_configs = lingxi_mcp::load_mcp_json_with_precedence(&project_mcp_path, &global_mcp_path);
    // Build the registry with the production HTTP transport — the registry
    // does NOT auto-connect; it merely registers each config in
    // `Disconnected` state so `/mcp` can list them. Real connect happens
    // when M7 wires the auto-connect loop, OR when a tool invokes the MCP
    // dispatch path.
    let mcp_transport: Arc<dyn lingxi_traits::McpTransport> = http.clone();
    let mcp_registry = Arc::new(lingxi_mcp::McpRegistry::new(mcp_transport));
    {
        // Pre-populate `Disconnected` entries so `/mcp` can list them
        // without firing a real connect.
        let mut conns = mcp_registry.connections.write().await;
        for cfg in mcp_configs {
            let name = cfg.name.clone();
            conns.insert(
                name,
                lingxi_mcp::McpConnectionState::Disconnected {
                    config: cfg,
                    last_error: None,
                },
            );
        }
    }

    // M6-07: Hook registry. Read settings JSON from project + user paths.
    let mut hook_registry = lingxi_hooks::HookRegistry::new();
    let project_settings_path = cwd.join(".claude").join("settings.json");
    let user_settings_path = dirs::config_dir()
        .map(|d| d.join("claude").join("settings.json"))
        .unwrap_or_else(|| std::path::PathBuf::from("/dev/null"));
    for (path, source) in [
        (user_settings_path, lingxi_hooks::definition::HookSource::User),
        (project_settings_path, lingxi_hooks::definition::HookSource::Project),
    ] {
        if let Ok(raw) = tokio::fs::read_to_string(&path).await {
            match lingxi_hooks::parse_hooks_from_settings_json(&raw, source) {
                Ok(hooks) => {
                    for h in hooks {
                        hook_registry.register(h);
                    }
                }
                Err(e) => tracing::warn!(error = %e, path = %path.display(), "skipping malformed settings hooks"),
            }
        }
    }
    let hook_registry = Arc::new(tokio::sync::RwLock::new(hook_registry));

    // M6-07: Agent catalog. Load from project + user agents/.
    let project_agents_dir = cwd.join(".claude").join("agents");
    let user_agents_dir = dirs::home_dir()
        .map(|h| h.join(".claude").join("agents"))
        .unwrap_or_else(|| std::path::PathBuf::from("/dev/null"));
    let agents = lingxi_agent::load_agents_from_dirs(&[
        (user_agents_dir, lingxi_agent::definition::AgentSource::UserDefined),
        (project_agents_dir, lingxi_agent::definition::AgentSource::Project),
    ])
    .await;
    let agent_catalog = Arc::new(tokio::sync::RwLock::new(agents));
```

Then replace the orchestrator construction with the three builders chained:

```rust
    let orch = Arc::new(
        ConversationOrchestrator::new(
            cfg, api_client, tools, hooks, perms, output, memory, cwd,
        )
        .with_mcp_registry(mcp_registry.clone())
        .with_hook_registry(hook_registry.clone())
        .with_agent_catalog(agent_catalog.clone()),
    );
```

(`hooks` here is the existing `noop_hook_executor()` argument — separate from the new `hook_registry` field, which is the **definitions** registry. The `HookExecutor` argument is the runtime dispatcher and stays unchanged in M6-07.)

Add to `crates/cli/Cargo.toml`:
```toml
lingxi-mcp = { path = "../mcp" }
lingxi-hooks = { path = "../hooks" }
lingxi-agent = { path = "../agent" }
dirs = { workspace = true }
```
(`dirs` is likely already a dep; verify before adding.)

- [ ] **Step 4: Run; expect PASS.**

```bash
cargo test -p lingxi-cli build_runtime_with_defaults
```

- [ ] **Step 5: Commit.**

```bash
git add lingxi-code/crates/cli/src/init.rs lingxi-code/crates/cli/Cargo.toml lingxi-code/crates/orchestrator/src/conversation.rs
git commit -m "feat(cli): wire McpRegistry + HookRegistry + agent catalog into build_runtime (M6-07)"
```

---

## Task 11: Parity fixture — `tui_listings.json` locks empty-state literals

**Files:**
- Create: `lingxi-code/crates/test-harness/src/parity/fixtures/tui_listings.json`
- Create: `lingxi-code/crates/test-harness/tests/parity_tui_listings.rs`

- [ ] **Step 1: Write the fixture.**

Create `crates/test-harness/src/parity/fixtures/tui_listings.json`:

```json
{
  "schema_version": 1,
  "milestone": "M6-07",
  "description": "Locks the empty-state literals for /mcp, /hooks, /agents.",
  "empty_states": {
    "mcp":    "No MCP servers configured\n",
    "hooks":  "No hooks configured\n",
    "agents": "No subagents configured\n"
  },
  "non_empty_sample": {
    "mcp": {
      "input": [
        { "name": "filesystem", "status": "Disconnected", "transport": "stdio" },
        { "name": "memory",     "status": "Connected",    "transport": "stdio" }
      ],
      "output": "MCP servers (2):\n  filesystem  disconnected  stdio\n  memory  connected  stdio\n"
    },
    "hooks": {
      "input": [
        { "name": "./fmt.sh", "event": "PreToolUse", "matcher": "Write|Edit", "timeout_ms": 30000 }
      ],
      "output": "Hooks (1):\n  ./fmt.sh  PreToolUse  30000ms\n"
    },
    "agents": {
      "input": [
        { "name": "reviewer", "description": "Reviews code", "tools_allowed": ["Read", "Grep"] }
      ],
      "output": "Agents (1):\n  reviewer  Reviews code\n"
    }
  }
}
```

- [ ] **Step 2: Write the driver test.**

Create `crates/test-harness/tests/parity_tui_listings.rs`:

```rust
//! M6-07 parity — locks `/mcp`, `/hooks`, `/agents` empty-state literals
//! and one non-empty sample for each.

use lingxi_commands::builtin::{
    agents::AgentsHandler, hooks::HooksHandler, mcp::McpHandler,
};
use lingxi_commands::model::{BuiltinCommandHandler, CommandResult};
use lingxi_commands::parser::ParsedSlashCommand;
use lingxi_orchestrator::test_support::MockOrchestratorHandle;
use lingxi_traits::{AgentInfo, HookInfo, McpServerInfo, McpStatus};
use serde_json::Value;
use std::sync::Arc;

fn args(name: &str) -> ParsedSlashCommand {
    ParsedSlashCommand {
        name: name.into(),
        raw_args: String::new(),
        positional_args: vec![],
    }
}

fn load_fixture() -> Value {
    let raw = include_str!("../src/parity/fixtures/tui_listings.json");
    serde_json::from_str(raw).expect("fixture parses")
}

#[tokio::test]
async fn empty_states_match_fixture() {
    let fixture = load_fixture();
    let empty = &fixture["empty_states"];

    let mock = Arc::new(MockOrchestratorHandle::new());
    let r = McpHandler::new(mock.clone()).handle(&args("mcp")).await;
    match r {
        CommandResult::Done { display: Some(s) } => {
            assert_eq!(s, empty["mcp"].as_str().unwrap());
        }
        other => panic!("got {other:?}"),
    }

    let r = HooksHandler::new(mock.clone()).handle(&args("hooks")).await;
    match r {
        CommandResult::Done { display: Some(s) } => {
            assert_eq!(s, empty["hooks"].as_str().unwrap());
        }
        other => panic!("got {other:?}"),
    }

    let r = AgentsHandler::new(mock).handle(&args("agents")).await;
    match r {
        CommandResult::Done { display: Some(s) } => {
            assert_eq!(s, empty["agents"].as_str().unwrap());
        }
        other => panic!("got {other:?}"),
    }
}

#[tokio::test]
async fn non_empty_mcp_matches_fixture() {
    let fixture = load_fixture();
    let mock = Arc::new(MockOrchestratorHandle::new());
    mock.set_mcp_servers(vec![
        McpServerInfo { name: "filesystem".into(), status: McpStatus::Disconnected, transport: "stdio".into() },
        McpServerInfo { name: "memory".into(),     status: McpStatus::Connected,    transport: "stdio".into() },
    ]);
    let r = McpHandler::new(mock).handle(&args("mcp")).await;
    let expected = fixture["non_empty_sample"]["mcp"]["output"].as_str().unwrap();
    match r {
        CommandResult::Done { display: Some(s) } => assert_eq!(s, expected),
        other => panic!("got {other:?}"),
    }
}

#[tokio::test]
async fn non_empty_hooks_matches_fixture() {
    let fixture = load_fixture();
    let mock = Arc::new(MockOrchestratorHandle::new());
    mock.set_hooks(vec![HookInfo {
        name: "./fmt.sh".into(),
        event: "PreToolUse".into(),
        matcher: Some("Write|Edit".into()),
        timeout_ms: 30_000,
    }]);
    let r = HooksHandler::new(mock).handle(&args("hooks")).await;
    let expected = fixture["non_empty_sample"]["hooks"]["output"].as_str().unwrap();
    match r {
        CommandResult::Done { display: Some(s) } => assert_eq!(s, expected),
        other => panic!("got {other:?}"),
    }
}

#[tokio::test]
async fn non_empty_agents_matches_fixture() {
    let fixture = load_fixture();
    let mock = Arc::new(MockOrchestratorHandle::new());
    mock.set_agents(vec![AgentInfo {
        name: "reviewer".into(),
        description: "Reviews code".into(),
        tools_allowed: vec!["Read".into(), "Grep".into()],
    }]);
    let r = AgentsHandler::new(mock).handle(&args("agents")).await;
    let expected = fixture["non_empty_sample"]["agents"]["output"].as_str().unwrap();
    match r {
        CommandResult::Done { display: Some(s) } => assert_eq!(s, expected),
        other => panic!("got {other:?}"),
    }
}
```

(If `lingxi_commands::builtin::{agents::AgentsHandler, hooks::HooksHandler, mcp::McpHandler}` paths differ, run `rg -n "pub struct McpHandler\|pub struct HooksHandler\|pub struct AgentsHandler" lingxi-code/crates/commands/src/` and adjust the import paths.)

- [ ] **Step 3: Run; expect PASS (handlers and fixture align by construction).**

```bash
cargo test -p lingxi-test-harness --test parity_tui_listings
```
Expected: 4 passing.

- [ ] **Step 4: Re-run v0.6.0 parity fixtures to confirm no regression.**

```bash
cargo test -p lingxi-test-harness
```
Expected: every fixture under `crates/test-harness/tests/parity_*.rs` still green.

- [ ] **Step 5: Commit.**

```bash
git add lingxi-code/crates/test-harness/src/parity/fixtures/tui_listings.json lingxi-code/crates/test-harness/tests/parity_tui_listings.rs
git commit -m "test(parity): tui_listings fixture locks /mcp /hooks /agents literals (M6-07)"
```

---

## Task 12: Verification gate + tag `m6.7`

**Files:**
- All files modified in Tasks 1-11
- Tag: `m6.7`

- [ ] **Step 1: Run the M6-07 test set.**

```bash
cargo test -p lingxi-mcp json_config:: snapshot_tests::
cargo test -p lingxi-hooks loader::tests all_hooks_tests::
cargo test -p lingxi-agent catalog::tests
cargo test -p lingxi-orchestrator with_mcp_hook_agent_builders_store_fields
cargo test -p lingxi-orchestrator --test list_mcp_real --test list_hooks_real --test list_agents_real
cargo test -p lingxi-commands builtin::list_render::tests builtin::mcp:: builtin::hooks:: builtin::agents::
cargo test -p lingxi-cli build_runtime_with_defaults
cargo test -p lingxi-test-harness --test parity_tui_listings
```

**Pass criteria:**
- `json_config::tests::parse_two_stdio_servers` → PASS, both names found, project precedence honoured
- `loader::tests::one_pretooluse_command_hook` → PASS, timeout = 30s
- `catalog::tests::load_agents_from_dirs_merges_user_and_project` → PASS, project wins on collision
- `list_mcp_real::list_mcp_servers_returns_two_when_two_registered` → PASS
- `list_hooks_real::list_hooks_returns_one_pretooluse_entry` → PASS
- `list_agents_real::list_agents_returns_one_entry` → PASS
- `mcp::tests::empty_list` → `"No MCP servers configured\n"`
- `hooks::tests::empty_list` → `"No hooks configured\n"`
- `agents::tests::empty_list` → `"No subagents configured\n"`
- `build_runtime_with_defaults` → `has_mcp_registry == true`, `has_hook_registry == true`, `has_agent_catalog == true`
- `parity_tui_listings::empty_states_match_fixture` → PASS
- ALL v0.6.0 parity fixtures (`parity_betas`, `parity_cost_events`, `parity_doctor_report`, `parity_file_tools`, `parity_help_render`, `parity_hooks_runtime`, `parity_init_template`, `parity_orchestrator`, `parity_session_jsonl`, `parity_slash_commands_102`) → all green

- [ ] **Step 2: Workspace verification gate.**

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo check --workspace --target x86_64-unknown-linux-gnu
cargo check --workspace --target x86_64-apple-darwin
cargo check --workspace --target x86_64-pc-windows-gnu
cargo check --workspace --target aarch64-linux-android
cargo check --workspace --target aarch64-apple-ios
```

Known-allowed flakes (allowed to rerun once each):
- `rapid_writes_collapse_to_single_event`
- `writer_output_equals_single_turn_fixture`
- `streaming_concurrent_tools_test`

- [ ] **Step 3: Manual smoke (per spec §5.5 M6-07 row).**

Build the CLI binary:
```bash
cargo build -p lingxi-cli --release
```

Run two scenarios:

**(a) Empty state**

```bash
ANTHROPIC_API_KEY="" target/release/lingxi-cli --no-tui <<EOF
/mcp
/hooks
/agents
/exit
EOF
```

Expected output (verbatim — locked):
```
No MCP servers configured
No hooks configured
No subagents configured
```

**(b) Populated state**

```bash
mkdir -p /tmp/m6_07_smoke/.claude/agents
cd /tmp/m6_07_smoke
cat > .mcp.json <<'JSON'
{"mcpServers":{"memory":{"command":"echo","args":[]},"filesystem":{"command":"echo","args":[]}}}
JSON
cat > .claude/agents/reviewer.md <<'MD'
---
name: reviewer
description: Reviews code
tools: [Read, Grep]
---
Review code carefully.
MD
ANTHROPIC_API_KEY="" target/release/lingxi-cli --no-tui --cwd /tmp/m6_07_smoke <<EOF
/mcp
/agents
/exit
EOF
```

Expected:
- `/mcp` lists `filesystem` and `memory` (both `disconnected`, `stdio`)
- `/agents` lists `reviewer  Reviews code`

(`/hooks` still empty unless `.claude/settings.json` carries a `hooks` block — separate smoke.)

- [ ] **Step 4: Tag `m6.7`.**

```bash
git tag -a m6.7 -m "M6-07 — engine wiring 2: MCP / Hooks / Agents listings"
```

(NO push to remote. Same policy as v0.5.0 / v0.6.0 — see parent spec §6.4.)

- [ ] **Step 5: Verify tag exists locally.**

```bash
git tag -l 'm6.7'
git show --stat m6.7 | head -20
```

---

## Dependency Notes / Cross-task ordering

- **Tasks 1-11** can be executed in linear order. Tasks 2/3/4/5/6 are independent of each other (different crates) — could run in parallel if a subagent-driven executor wants speed. Task 7 depends on Task 1 (Cargo deps land here). Task 8 depends on Tasks 2/3/6/7. Task 9 depends on Task 1 (signature). Task 10 depends on Tasks 4/5/6/7 (consumes all four loaders + the builders). Task 11 depends on Task 9 (handler literals).
- **No TUI changes in M6-07.** The handlers' returned strings already pass through the M6-02 REPL surface via `CommandResult::Done { display: Some(s) }`; the TUI renders them as-is. If the spawned spec includes a StatusLine MCP-count badge, that's deferred to M6-09's release polish (claude-code's StatusLine is user-shell-configured and not a built-in indicator — see "StatusLine MCP count" note in the File Structure header).
- **No new telemetry events.** `/mcp`, `/hooks`, `/agents` already emit `tengu_command_mcp_*`, `tengu_command_hooks_*`, `tengu_command_agents_*` from M5-11. M6-07 changes only the *body* of the listing, not the event surface.

---

## Self-Review

**1. Spec coverage** (mapping the prompt's "WHAT M6-07 SHIPS" list to tasks):

| # | Prompt requirement | Task |
|---|---|---|
| 1 | `list_mcp_servers` reads `lingxi_mcp` registry | Tasks 2, 7, 8, 10 |
| 2 | `list_hooks` reads `lingxi_hooks::HookRegistry` | Tasks 3, 7, 8, 10 |
| 3 | `list_agents` reads agent catalog | Tasks 6, 7, 8, 10 |
| 4 | Startup loading: `.mcp.json` + global mcp.json + settings hooks + project/global agents | Tasks 4, 5, 6, 10 |
| 5 | `/mcp` `/hooks` `/agents` slash commands verify with real data | Task 11 (parity fixture + Task 12 smoke) |
| 6 | Empty-state literals (verbatim) | Tasks 1, 9, 11 |
| 7 | StatusLine optional MCP count | Dropped — claude-code's StatusLine is a user-configured shell command, not an MCP indicator. Documented in File Structure header. |

| Test required (prompt) | Task |
|---|---|
| `.mcp.json` with 2 stdio servers → list_mcp_servers returns 2 | Task 8 (`list_mcp_servers_returns_two_when_two_registered`) + Task 12 smoke (b) |
| No `.mcp.json` → vec![] AND /mcp shows empty literal | Task 8 (`list_mcp_servers_returns_empty_when_no_registry`) + Task 9 (`empty_list`) + Task 11 (fixture) |
| 1 PreToolUse hook → list_hooks returns 1 | Task 8 (`list_hooks_returns_one_pretooluse_entry`) |
| `~/.claude/agents/dummy.md` → list_agents returns 1 | Task 6 (`load_agents_from_dirs_merges_user_and_project`) + Task 8 (`list_agents_returns_one_entry`) |
| Parity fixture `tui_listings.json` round-trips empty-state literals | Task 11 |
| All v0.6.0 fixtures still pass | Task 11 step 4 + Task 12 step 2 |

**2. Placeholder scan:** searched for `TBD`, `TODO`, `implement later`, `fill in details`, `add appropriate`, `similar to Task` — none present. Every step lists the exact code or command to run.

**3. Type consistency:** every reference uses `McpRegistry` (not `ClientRegistry`), `Arc<RwLock<HookRegistry>>` (not `Arc<HookRegistry>` since `register` takes `&mut`), `Arc<RwLock<Vec<AgentDefinition>>>` (not the non-existent `AgentCatalog`). `McpServerInfo` / `HookInfo` / `AgentInfo` field names exactly match `lingxi_traits::orchestrator` definitions. Empty-state literals are uniform across the three handlers, the fixture, and the smoke output.

## Unresolved questions / deferred

1. **Hot reload.** This plan loads the three registries **once at startup**. If a user edits `.mcp.json` or `.claude/agents/` mid-session, the change is not picked up until restart. claude-code's `/reload-plugins` covers some of this. M7 can add a `/reload-config` command + filesystem watcher on these four paths. Surface this in the v0.7.0 release notes as a known limitation.
2. **Hook `event` projection.** Each `HookDefinition` carries `events: Vec<HookEventType>` — multiple subscriptions per hook are possible. This plan projects only `events.first()` into the `HookInfo::event` string. If a hook subscribes to two events, only the first one shows. This matches claude-code's `/hooks` display (which lists one row per matcher group, also one event). Documented inline in `handle_impl.rs::list_hooks`.
3. **`McpRegistry::connections` visibility.** Task 10 writes directly to `mcp_registry.connections.write().await` from the CLI crate. If `connections` is `pub(crate)` today, Task 10 must instead use the existing `register_test_client` or add a `register_disconnected_config(cfg: McpServerConfig)` public method. Add this in Task 2 step 4 if needed — the test in Task 2 currently uses the same direct-write trick under `#[cfg(test)]`, so this only bites at production wiring time. **Mitigation:** if `connections` is private, add `pub async fn register_disconnected(&self, cfg: McpServerConfig)` in Task 2 step 4 alongside `snapshot`. The CLI init then uses that method instead of the raw map.
4. **claude-code's StatusLine.** Dropped from scope (it's a user-shell-configured command, not a built-in indicator). The `/status` command's `n_mcp_total` / `n_mcp_connected` / `n_hooks` / `n_agents` fields still populate from the registries via the M5-11 `StatusSnapshot`; that wiring lands as a small extra to Task 10 only if the `get_status_snapshot` body needs to be updated (currently it hard-codes zeroes — fixing it is one Task 10 step away but is **out of scope for M6-07's listing endpoints**. Track as a follow-up if `/status` numbers feel wrong in dogfooding).
5. **Agent description truncation.** The `/agents` handler already truncates `description` to 80 characters with a Unicode-aware `…`. This is the existing M5-11 behavior — M6-07 inherits it unchanged. If we want to match claude-code's `AgentsList.tsx` (which seems to print descriptions full-width with wrapping), it's a later UI polish.

**End of M6-07 plan.**
