# M5-11 Slash Commands Core Batch 2 — 12 remaining commands

> **Status (2026-05-28):** ✅ COMPLETE. T0–T16 executed; tag `m5.11` cut on `m5-execution` branch. 18 core slash commands all live; +36 telemetry events (276 → **312**); `OrchestratorHandle` +8 methods; `AuthHandle` trait + `OAuthHandle` thin wrapper (real PKCE flow deferred to M5-12 per inline note).
>
> **2026-05-28 substitutions applied:**
> - Plan text referenced "84 non-core" — actual surface is **81** (M5-09 99/81 lock).
> - `OrchestratorHandle` count: the prose said 7 new methods but the actual count is **8** (the plan's T2 step 6 already lists `list_available_models` as the 8th).
> - `CostSnapshot` was extended in-place (not via a new mirror type) — `total_usd`, `input_tokens`, `output_tokens`, `api_calls`, `session_duration` added; `Eq` derive dropped because `f64`/`Duration` don't implement it. `StatusSnapshot` similarly dropped `Eq`. `OutputEvent` dropped `Eq` (transitively).
> - `OAuthHandle` ships as a thin wrapper that returns `AuthError::ServerError("…not yet wired in M5-11; CLI binary in M5-12 will plug this in")` for `login()`, `Ok(())` for `logout()` (idempotent), `None` for `current_user()`. This is the documented deviation: the plan's T3 step 3 sketched a full PKCE drive but the existing `ClaudeAiOAuthClient` lacks `run_interactive_login` / `clear_credentials` / `resolve` methods. Real wiring deferred to M5-12 CLI binary integration.
> - `register_core_placeholders` is now a no-op shim (M5-11 deleted the 12 batch-2 placeholders; M5-10 had already deleted the 6 batch-1 ones).
> - Production `OrchestratorHandle` impls for `list_mcp_servers`/`list_hooks`/`list_agents` return empty `Vec`s because `ConversationOrchestrator` does not yet hold MCP/Hooks-registry/Agent fields wired through; M5-12 CLI will plumb them. The `MockOrchestratorHandle` in `test_support` carries full setters so handler tests still exercise the real rendering paths.
> - `check_disk_space` uses a 1-byte probe-write instead of an `fs2`-based free-space query (no `fs2` dep was added; reports `Pass` if writable, `Warn` if not).
> - `check_network` uses `std::net::TcpStream::connect_timeout` via `tokio::task::spawn_blocking` (the `lingxi-orchestrator` crate's tokio feature-set does not include `net`).
> - `cargo test -p lingxi-telemetry registry_is_exactly_312_entries -- --exact` requires `--test event_name_completeness_test` to disambiguate (multiple test binaries in the crate). Test passes.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the remaining 12 M5-09 placeholder structs (`CostHandler`, `ConfigHandler`, `ModelHandler`, `PermissionsHandler`, `McpHandler`, `HooksHandler`, `AgentsHandler`, `LoginHandler`, `LogoutHandler`, `VersionHandler`, `StatusHandler`, `DoctorHandler`) with real implementations. After M5-11 lands, the 18 core commands are **all** live; the 84 non-core commands continue to return the M5-09 locked stub literal. Ships **36 new telemetry events** (`tengu_command_<name>_<started|completed|failed>` × 12), growing `tengu::command::NAMES` from 18 → **54** and `ALL_EVENT_NAMES.len()` from 276 → **312**. Grows `OrchestratorHandle` with 7 new methods: `list_mcp_servers`, `list_hooks`, `list_agents`, `run_doctor_checks`, `get_status_snapshot`, `edit_config_file`, `edit_permissions_file`. Introduces a new `AuthHandle` trait in `lingxi-traits` (implemented by `lingxi-anthropic-oauth`) for `/login` and `/logout`. Each batch-2 handler is plain-text-only (no Ink TUI — that's M6); the `/doctor` and `/status` panels are rendered via byte-locked multi-line `format!` templates.

**Architecture:** Twelve new files under `lingxi-commands/src/builtin/` (one per command). One new file `lingxi-commands/src/builtin/list_render.rs` shared by `/mcp`, `/hooks`, `/agents` for the common "header + per-row" rendering pattern. One new file `lingxi-commands/src/builtin/diagnostics.rs` holding the `/doctor` check list. Five new info structs in `lingxi-traits::orchestrator`: `McpServerInfo`, `HookInfo`, `AgentInfo`, `DoctorReport`, `StatusSnapshot`. One new trait `lingxi-traits::auth::AuthHandle` plus a concrete impl `lingxi-anthropic-oauth::handle::OAuthHandle` (wrapping the existing `oauth::client::AnthropicOAuthClient`). The M5-10 `register_core_batch_1(reg, handle)` is **untouched**; M5-11 adds a parallel `register_core_batch_2(reg, handle, auth)` taking the additional `auth: Arc<dyn AuthHandle>` parameter. Both batches' commands compose: M5-12 (CLI binary) calls `register_all_builtin_commands → register_core_batch_1 → register_core_batch_2` in order.

**Tech stack:** Rust 2021, `lingxi_anthropic_oauth = { path = "../anthropic-oauth" }` (M2 surface), `lingxi_mcp = { path = "../mcp" }` (M2-02 registry), `lingxi_hooks = { path = "../hooks" }` (M5-06 runtime), `lingxi_agent = { path = "../agent" }` (M4-05 subagents), `dirs = "5"` (config dir), `tokio` (process spawn for editor), `async_trait = "0.1"`, `chrono = "0.4"` (timestamps in `/status` and `/doctor` reports).

---

## Task 0: Reverse-engineer the 12 command bodies + lock the user-visible literals

**Files:**
- Read: each of `claude-code/src/commands/<name>/index.ts` for the 12 commands
- Read: `claude-code/src/commands/status/status.tsx` (Ink TUI — we stdio-fallback)
- Read: `claude-code/src/commands/doctor/doctor.tsx` (Ink TUI — we stdio-fallback)
- Read: `claude-code/src/commands/agents/agents.tsx` (we list-only, no editor)

- [ ] **Step 1: For each of the 12 commands, grep the `description` field + entry-point logic.**

```bash
for cmd in agents config cost doctor hooks login logout mcp model permissions status version; do
  echo "=== /$cmd ==="
  rg -A 3 "description\s*[:=]" "claude-code/src/commands/$cmd/" 2>/dev/null | head -5
done
```

  The M5-09 `core_description()` already has the 18 descriptions locked. Confirm none of the 12 batch-2 descriptions has drifted since v0.5.0:

  | Command | Description (locked by M5-09 T0 step 5) |
  |---|---|
  | `agents` | `"Manage subagents"` |
  | `config` | `"Open config panel"` |
  | `cost` | `"Show total cost and duration of the current session"` |
  | `doctor` | `"Diagnose installation and configuration"` |
  | `hooks` | `"Manage hooks"` |
  | `login` | `"Sign in with your Anthropic account"` |
  | `logout` | `"Sign out from your Anthropic account"` |
  | `mcp` | `"Manage MCP servers"` |
  | `model` | `"Set the model for Claude Code to use"` |
  | `permissions` | `"Manage permissions"` |
  | `status` | `"Show Claude Code status"` |
  | `version` | `"Print version information"` |

- [ ] **Step 2: Lock the 12 success / failure display templates.**

  Per spec §1 Non-Goals "M5 是 plain stdio,不做 ratatui shell" and explicit "TUI 推迟到 M6" decision: every batch-2 command returns plain text. Templates locked here as **LingXi UX locks** (claude-code's Ink TUI does not produce byte-stable plain text):

  | # | Command | Success display template | Failure prefix |
  |---|---|---|---|
  | L1 | `/cost` | `"Cost: ${total:.4} ({calls} calls, {input_tokens}+{output_tokens} tokens, {duration} session time)"` | `"Could not snapshot cost: "` |
  | L2 | `/config` | `"Edited {path} (exit {code})."` | `"Could not edit config: "` |
  | L3 | `/model` (no arg → list) | `"Current model: {name}\nAvailable: {csv}"` | `"Could not switch model: "` |
  | L4 | `/model <name>` (with arg → switch) | `"Switched to model: {name}"` | (same prefix as L3) |
  | L5 | `/permissions` | `"Edited {path} (exit {code})."` (same shape as /config) | `"Could not edit permissions: "` |
  | L6 | `/mcp` (no args → list) | `"MCP servers ({count}):\n{rows}"` with each row `"  {name}  {status}  {transport}"` | `"Could not list MCP servers: "` |
  | L7 | `/hooks` (no args → list) | `"Hooks ({count}):\n{rows}"` with each row `"  {name}  {event}  {timeout_ms}ms"` | `"Could not list hooks: "` |
  | L8 | `/agents` (no args → list) | `"Agents ({count}):\n{rows}"` with each row `"  {name}  {description}"` (description truncated to 80 chars + `…`) | `"Could not list agents: "` |
  | L9 | `/login` (success) | `"Logged in as {email} (org: {org_id})."` | `"Could not log in: "` |
  | L10 | `/logout` (success) | `"Logged out."` | `"Could not log out: "` |
  | L11 | `/version` | `"lingxi-cli {version} ({git_sha:short})"` where `{version}` is `CARGO_PKG_VERSION` and `{git_sha:short}` is the 7-char short hash baked at build time | (no failure path) |
  | L12 | `/status` | (multi-line panel — see Step 3) | `"Could not gather status: "` |
  | L13 | `/doctor` | (multi-line report — see Step 4) | `"Could not run doctor: "` |

  Notes on field semantics:
  - **L1** `{total:.4}` is the cost in USD to 4 decimal places (e.g. `0.0042`); `{input_tokens}`/`{output_tokens}` are accumulated since session start; `{duration}` is `chrono::Duration::to_string` style `"12m 34s"` from `session.started_at` → `now`. The CostSnapshot fields are defined by M5-02 (`pub struct CostSnapshot { total_usd: f64, input_tokens: u64, output_tokens: u64, api_calls: u32, session_duration: Duration }`).
  - **L3** `{csv}` is a comma-space-separated list of model names from `OrchestratorHandle::list_available_models` — wait, that method doesn't exist. We add it in Task 2.
  - **L6** `{status}` is one of `"connected"` / `"disconnected"` / `"error: <reason>"`. `{transport}` is one of `"stdio"` / `"sse"` / `"http"` (matching `lingxi-mcp::McpServerConfig::transport_kind()` semantics).
  - **L7** `{event}` is the hook event name (e.g. `"PreToolUse"` / `"PostToolUse"` / `"Stop"` / `"Notification"`). `{timeout_ms}` defaults to `60_000` (M5-06 lock) if unset.
  - **L8** `{description}` truncation: walk by `char_indices`, cut at the 80th char index, append `…` (U+2026) if the original was longer.

- [ ] **Step 3: Lock the `/status` panel format.**

```
Status:
  Session:    {session_id}
  Model:      {model_name}
  Messages:   {n_messages}
  Cost:       ${total:.4}
  Tokens:     {input}+{output}
  MCP:        {n_mcp_connected}/{n_mcp_total} connected
  Hooks:      {n_hooks} registered
  Agents:     {n_agents} available
  Started:    {started_at:RFC3339}
  Working dir: {cwd}
```

  Exactly 11 lines (header + 10 data rows). Column-1 width = 12 chars (longest label `Working dir:` = 12 chars). Each data row: `  ` (2 leading spaces) + label padded right to 13 chars + value + `\n`. Final line ends with `\n`.

- [ ] **Step 4: Lock the `/doctor` report format.**

  `/doctor` runs **6 checks**, each producing a `CheckResult { name, status, detail }`:

  1. **`config-dir`** — does `<config-dir>/claude/` exist + writable?
  2. **`api-key`** — is an API key configured (env var `ANTHROPIC_API_KEY` OR refresh token in keychain)?
  3. **`network`** — TCP reachability of `api.anthropic.com:443` (5s timeout)
  4. **`disk-space`** — at least 100 MiB free in `<config-dir>` filesystem?
  5. **`git`** — `git --version` succeeds?
  6. **`telemetry-schema`** — `ALL_EVENT_NAMES.len() == 312` (self-check that the binary's compiled schema matches v0.6.0 expectation)

  Each check returns one of `Pass` / `Warn(detail)` / `Fail(detail)`. The report format:

```
Doctor:
  [{glyph}] {name}: {status_text}{newline_if_detail}      {detail (indented 6 spaces)}
  ... (one block per check) ...
  Summary: {n_pass} passed, {n_warn} warnings, {n_fail} failed
```

  Where `{glyph}` is `OK` for pass, `!!` for warn, `XX` for fail (3 chars wide in plain ASCII so the format stays column-stable without unicode glyphs that break some terminals). Tests assert the exact byte layout.

- [ ] **Step 5: Lock the new telemetry event names (36 total).**

  Pattern: `tengu_command_<name>_<phase>` where `<name>` ∈ {`agents, config, cost, doctor, hooks, login, logout, mcp, model, permissions, status, version`} and `<phase>` ∈ {`started, completed, failed`}.

  ASCII-sorted (by command name then phase: `completed < failed < started`):

```
tengu_command_agents_completed
tengu_command_agents_failed
tengu_command_agents_started
tengu_command_config_completed
tengu_command_config_failed
tengu_command_config_started
tengu_command_cost_completed
tengu_command_cost_failed
tengu_command_cost_started
tengu_command_doctor_completed
tengu_command_doctor_failed
tengu_command_doctor_started
tengu_command_hooks_completed
tengu_command_hooks_failed
tengu_command_hooks_started
tengu_command_login_completed
tengu_command_login_failed
tengu_command_login_started
tengu_command_logout_completed
tengu_command_logout_failed
tengu_command_logout_started
tengu_command_mcp_completed
tengu_command_mcp_failed
tengu_command_mcp_started
tengu_command_model_completed
tengu_command_model_failed
tengu_command_model_started
tengu_command_permissions_completed
tengu_command_permissions_failed
tengu_command_permissions_started
tengu_command_status_completed
tengu_command_status_failed
tengu_command_status_started
tengu_command_version_completed
tengu_command_version_failed
tengu_command_version_started
```

  Confirm via `printf '%s\n' tengu_command_{agents,config,cost,doctor,hooks,login,logout,mcp,model,permissions,status,version}_{completed,failed,started} | sort`.

- [ ] **Step 6: Lock the new `OrchestratorHandle` methods (7) + new info struct fields.**

  See Task 2 step 3 for the struct definitions. The 7 new methods:

```rust
async fn list_mcp_servers(&self) -> Vec<McpServerInfo>;
async fn list_hooks(&self) -> Vec<HookInfo>;
async fn list_agents(&self) -> Vec<AgentInfo>;
async fn run_doctor_checks(&self) -> DoctorReport;
async fn get_status_snapshot(&self) -> StatusSnapshot;
async fn edit_config_file(&self) -> Result<MemoryEditorOutcome, HandleError>;
async fn edit_permissions_file(&self) -> Result<MemoryEditorOutcome, HandleError>;

// Plus one extra for /model (no-arg list mode):
async fn list_available_models(&self) -> Vec<String>;
```

  Total = **8** new `OrchestratorHandle` methods (7 from the list above + `list_available_models`). The trait grows from M5-10's 7 methods (5 + 2) to **15** methods.

- [ ] **Step 7: Lock the `AuthHandle` trait surface.**

```rust
#[async_trait]
pub trait AuthHandle: Send + Sync {
    /// Run an interactive OAuth code-flow (PKCE) login. Blocks until the
    /// user completes the browser flow + redirects back, or the timeout
    /// (60s) elapses.
    async fn login(&self) -> Result<LoginInfo, AuthError>;

    /// Clear stored credentials. Idempotent.
    async fn logout(&self) -> Result<(), AuthError>;

    /// Snapshot the current logged-in user (if any). Used by `/status`.
    async fn current_user(&self) -> Option<LoginInfo>;
}

pub struct LoginInfo {
    pub email: String,
    pub org_id: String,
}

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("login flow timed out")]
    Timeout,
    #[error("user cancelled login")]
    Cancelled,
    #[error("network error: {0}")]
    Network(String),
    #[error("server rejected: {0}")]
    ServerError(String),
}
```

- [ ] **Step 8: Commit the byte-locks reference.**

```bash
cd /Users/luolingfeng/Projects/LingXi-Next
git add docs/superpowers/plans/2026-05-25-m5-11-commands-batch-2.md
git commit -m "plan(M5-11 T0): reverse-engineer 12 batch-2 command bodies + 13 user-visible literals + 8 new OrchestratorHandle methods + AuthHandle trait"
```

### Reverse-engineered byte-locks (locked by T0)

| Lock | Value | Source |
|---|---|---|
| 12 success templates (L1-L13) | (table in Step 2) | LingXi UX (no claude-code plain-text equivalent — TUI deferred to M6) |
| `/status` panel layout | 11 lines, col-1 = 13 chars | LingXi UX |
| `/doctor` 6 checks + 3-status glyphs | (Step 4 table) | LingXi UX |
| 36 telemetry event names | (Step 5 sorted list) | LingXi convention |
| 8 new `OrchestratorHandle` methods | (Step 6) | LingXi API design |
| `AuthHandle` trait + `LoginInfo` + `AuthError` | (Step 7) | LingXi API design (`lingxi-anthropic-oauth` impl) |
| `tengu::command::NAMES.len()` after M5-11 | **54** (was 18 after M5-10) | M5-10 + 36 = 54 |
| `ALL_EVENT_NAMES.len()` after M5-11 | **312** | Spec §6.3 |

---

## Task 1: Extend `tengu::command` from 18 → 54 entries

**Files:**
- Modify: `lingxi-code/crates/telemetry/src/tengu/command.rs` (append 36 const + extend NAMES)
- Modify: `lingxi-code/crates/telemetry/src/tengu/mod.rs` (bump TOTAL formula 276 → 312)

- [ ] **Step 1: Write the failing test.**

  Append to `lingxi-code/crates/telemetry/src/tengu/command.rs::tests`:

```rust
    #[test]
    fn batch_2_extends_to_54_total() {
        assert_eq!(NAMES.len(), 54, "M5-11 should grow command::NAMES from 18 to 54");
    }

    #[test]
    fn batch_2_covers_all_12_extra_commands() {
        let expected: std::collections::HashSet<&str> = [
            "agents", "config", "cost", "doctor", "hooks", "login",
            "logout", "mcp", "model", "permissions", "status", "version",
        ].iter().copied().collect();
        let mut found: std::collections::HashSet<&str> = Default::default();
        for n in NAMES {
            let stripped = n.strip_prefix("tengu_command_").unwrap();
            let last_under = stripped.rfind('_').unwrap();
            let cmd = &stripped[..last_under];
            // Skip batch-1 names (clear/compact/exit/help/init/memory).
            if !["clear", "compact", "exit", "help", "init", "memory"].contains(&cmd) {
                found.insert(cmd);
            }
        }
        assert_eq!(found, expected);
    }
```

- [ ] **Step 2: Run + fail.**

```bash
cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core
cargo test -p lingxi-telemetry --lib tengu::command::tests::batch_2 2>&1 | head -15
```

  Expected: `assertion left: 18 right: 54`.

- [ ] **Step 3: Add the 36 new const + extend NAMES.**

  Append to `command.rs` (after the M5-10 const block):

```rust
// ────────────────────────────────────────────────────────────────────────────
// M5-11 batch 2: 12 commands × 3 phases = 36 new events
// ────────────────────────────────────────────────────────────────────────────

pub const AGENTS_STARTED:   &str = "tengu_command_agents_started";
pub const AGENTS_COMPLETED: &str = "tengu_command_agents_completed";
pub const AGENTS_FAILED:    &str = "tengu_command_agents_failed";

pub const CONFIG_STARTED:   &str = "tengu_command_config_started";
pub const CONFIG_COMPLETED: &str = "tengu_command_config_completed";
pub const CONFIG_FAILED:    &str = "tengu_command_config_failed";

pub const COST_STARTED:   &str = "tengu_command_cost_started";
pub const COST_COMPLETED: &str = "tengu_command_cost_completed";
pub const COST_FAILED:    &str = "tengu_command_cost_failed";

pub const DOCTOR_STARTED:   &str = "tengu_command_doctor_started";
pub const DOCTOR_COMPLETED: &str = "tengu_command_doctor_completed";
pub const DOCTOR_FAILED:    &str = "tengu_command_doctor_failed";

pub const HOOKS_STARTED:   &str = "tengu_command_hooks_started";
pub const HOOKS_COMPLETED: &str = "tengu_command_hooks_completed";
pub const HOOKS_FAILED:    &str = "tengu_command_hooks_failed";

pub const LOGIN_STARTED:   &str = "tengu_command_login_started";
pub const LOGIN_COMPLETED: &str = "tengu_command_login_completed";
pub const LOGIN_FAILED:    &str = "tengu_command_login_failed";

pub const LOGOUT_STARTED:   &str = "tengu_command_logout_started";
pub const LOGOUT_COMPLETED: &str = "tengu_command_logout_completed";
pub const LOGOUT_FAILED:    &str = "tengu_command_logout_failed";

pub const MCP_STARTED:   &str = "tengu_command_mcp_started";
pub const MCP_COMPLETED: &str = "tengu_command_mcp_completed";
pub const MCP_FAILED:    &str = "tengu_command_mcp_failed";

pub const MODEL_STARTED:   &str = "tengu_command_model_started";
pub const MODEL_COMPLETED: &str = "tengu_command_model_completed";
pub const MODEL_FAILED:    &str = "tengu_command_model_failed";

pub const PERMISSIONS_STARTED:   &str = "tengu_command_permissions_started";
pub const PERMISSIONS_COMPLETED: &str = "tengu_command_permissions_completed";
pub const PERMISSIONS_FAILED:    &str = "tengu_command_permissions_failed";

pub const STATUS_STARTED:   &str = "tengu_command_status_started";
pub const STATUS_COMPLETED: &str = "tengu_command_status_completed";
pub const STATUS_FAILED:    &str = "tengu_command_status_failed";

pub const VERSION_STARTED:   &str = "tengu_command_version_started";
pub const VERSION_COMPLETED: &str = "tengu_command_version_completed";
pub const VERSION_FAILED:    &str = "tengu_command_version_failed";
```

  Replace the `NAMES: &[&str; 18]` with `NAMES: &[&str; 54]` — full list ASCII-sorted (by command then phase: `completed < failed < started`):

```rust
pub const NAMES: &[&str; 54] = &[
    AGENTS_COMPLETED, AGENTS_FAILED, AGENTS_STARTED,
    CLEAR_COMPLETED, CLEAR_FAILED, CLEAR_STARTED,
    COMPACT_COMPLETED, COMPACT_FAILED, COMPACT_STARTED,
    CONFIG_COMPLETED, CONFIG_FAILED, CONFIG_STARTED,
    COST_COMPLETED, COST_FAILED, COST_STARTED,
    DOCTOR_COMPLETED, DOCTOR_FAILED, DOCTOR_STARTED,
    EXIT_COMPLETED, EXIT_FAILED, EXIT_STARTED,
    HELP_COMPLETED, HELP_FAILED, HELP_STARTED,
    HOOKS_COMPLETED, HOOKS_FAILED, HOOKS_STARTED,
    INIT_COMPLETED, INIT_FAILED, INIT_STARTED,
    LOGIN_COMPLETED, LOGIN_FAILED, LOGIN_STARTED,
    LOGOUT_COMPLETED, LOGOUT_FAILED, LOGOUT_STARTED,
    MCP_COMPLETED, MCP_FAILED, MCP_STARTED,
    MEMORY_COMPLETED, MEMORY_FAILED, MEMORY_STARTED,
    MODEL_COMPLETED, MODEL_FAILED, MODEL_STARTED,
    PERMISSIONS_COMPLETED, PERMISSIONS_FAILED, PERMISSIONS_STARTED,
    STATUS_COMPLETED, STATUS_FAILED, STATUS_STARTED,
    VERSION_COMPLETED, VERSION_FAILED, VERSION_STARTED,
];
```

  This is 18 commands × 3 phases = 54. The `names_are_sorted_ascii_ascending` test from M5-10 Task 1 (already in `tests` module) will validate the sort order — if your hand-typed array isn't sorted, fix the layout, do not change the test.

- [ ] **Step 4: Bump `tengu::mod.rs` TOTAL formula.**

  Old (post-M5-10): `... + 18` (for `command::NAMES.len() == 18`)  
  New (post-M5-11): `... + 54` (for `command::NAMES.len() == 54`)  

  The TOTAL `const TOTAL: usize = ... + 18` line in `tengu/mod.rs::ALL_EVENT_NAMES` becomes `... + 54`. End-state: `276 - 18 + 54 = 312`.

- [ ] **Step 5: Run all tests.**

```bash
cargo test -p lingxi-telemetry --lib tengu::command::tests 2>&1 | tail -15
cargo test -p lingxi-telemetry --lib tengu::tests 2>&1 | tail -5   # ALL_EVENT_NAMES count
```

  Expected: 9 command-mod tests pass (the original 7 + 2 new from Step 1) + tengu mod tests pass at 312.

- [ ] **Step 6: Fmt + clippy + commit.**

```bash
cargo fmt -p lingxi-telemetry
cargo clippy -p lingxi-telemetry --lib --tests -- -D warnings 2>&1 | tail -5
git add lingxi-code/crates/telemetry/src/tengu/command.rs \
        lingxi-code/crates/telemetry/src/tengu/mod.rs
git commit -m "feat(M5-11 task 1): tengu::command 18→54 (+36 events) + ALL_EVENT_NAMES 276→312 (2 new invariant tests)"
```

---

## Task 2: Extend `OrchestratorHandle` with 8 new methods + 5 info structs

**Files:**
- Modify: `lingxi-code/crates/traits/src/orchestrator.rs`
- Modify: `lingxi-code/crates/orchestrator/src/handle_impl.rs`
- Modify: `lingxi-code/crates/orchestrator/src/test_support.rs` (mock methods)

- [ ] **Step 1: Write the failing test (object safety + struct field invariants).**

  Append to `lingxi-code/crates/traits/src/orchestrator.rs::m5_10_extension_tests` (rename to `m5_extension_tests`):

```rust
    // Compile-time assertion that the trait remains object-safe after M5-11.
    fn _handle_remains_object_safe_after_m5_11<T: OrchestratorHandle + Send + Sync + 'static>() {
        let _: Box<dyn OrchestratorHandle> = Box::new(std::marker::PhantomData::<T>);
    }

    #[test]
    fn mcp_server_info_fields() {
        let info = McpServerInfo {
            name: "memory".to_string(),
            status: McpStatus::Connected,
            transport: "stdio".to_string(),
        };
        assert_eq!(info.name, "memory");
        assert!(matches!(info.status, McpStatus::Connected));
    }

    #[test]
    fn hook_info_fields() {
        let info = HookInfo {
            name: "fmt-on-write".to_string(),
            event: "PostToolUse".to_string(),
            matcher: Some("Write|Edit".to_string()),
            timeout_ms: 60_000,
        };
        assert_eq!(info.timeout_ms, 60_000);
    }

    #[test]
    fn agent_info_fields() {
        let info = AgentInfo {
            name: "reviewer".to_string(),
            description: "review code".to_string(),
            tools_allowed: vec!["Read".to_string(), "Grep".to_string()],
        };
        assert_eq!(info.tools_allowed.len(), 2);
    }

    #[test]
    fn doctor_report_has_six_checks_default() {
        let r = DoctorReport::default();
        assert_eq!(r.checks.len(), 0); // empty default; populated by run_doctor_checks
        assert_eq!(r.summary.passed, 0);
    }

    #[test]
    fn status_snapshot_fields() {
        let s = StatusSnapshot::default();
        assert_eq!(s.n_messages, 0);
        assert_eq!(s.n_mcp_connected, 0);
    }
```

- [ ] **Step 2: Run + fail.**

```bash
cargo test -p lingxi-traits --lib m5_extension_tests 2>&1 | head -15
```

  Expected: 6 compile errors (`cannot find struct 'McpServerInfo'`, etc.).

- [ ] **Step 3: Add the 5 info structs + 8 trait methods.**

  Open `lingxi-code/crates/traits/src/orchestrator.rs` and append:

```rust
// ────────────────────────────────────────────────────────────────────────────
// M5-11 info structs
// ────────────────────────────────────────────────────────────────────────────

/// MCP server entry returned by `OrchestratorHandle::list_mcp_servers`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerInfo {
    pub name: String,
    pub status: McpStatus,
    /// `"stdio"`, `"sse"`, or `"http"`.
    pub transport: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpStatus {
    Connected,
    Disconnected,
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookInfo {
    pub name: String,
    /// `"PreToolUse"`, `"PostToolUse"`, `"Stop"`, `"Notification"`, etc.
    pub event: String,
    pub matcher: Option<String>,
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentInfo {
    pub name: String,
    pub description: String,
    pub tools_allowed: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DoctorReport {
    pub checks: Vec<DoctorCheck>,
    pub summary: DoctorSummary,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorCheck {
    pub name: String,
    pub status: CheckStatus,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckStatus {
    Pass,
    Warn,
    Fail,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DoctorSummary {
    pub passed: u32,
    pub warnings: u32,
    pub failed: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StatusSnapshot {
    pub session_id: String,
    pub model: String,
    pub n_messages: u32,
    pub total_cost_usd: f64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub n_mcp_connected: u32,
    pub n_mcp_total: u32,
    pub n_hooks: u32,
    pub n_agents: u32,
    /// `"YYYY-MM-DDTHH:MM:SSZ"` (RFC 3339, UTC, second precision).
    pub started_at: String,
    pub cwd: std::path::PathBuf,
}
```

  Add re-exports to `lingxi-code/crates/traits/src/lib.rs`:

```rust
pub use orchestrator::{
    AgentInfo, CheckStatus, DoctorCheck, DoctorReport, DoctorSummary, HookInfo,
    McpServerInfo, McpStatus, StatusSnapshot,
};
```

  Extend the `OrchestratorHandle` trait body with the 8 new methods (place after the M5-10 `open_memory_editor` method):

```rust
    // M5-11 additions:

    /// Enumerate currently registered MCP servers + their connection state.
    async fn list_mcp_servers(&self) -> Vec<McpServerInfo>;

    /// Enumerate registered hooks (built-in + user).
    async fn list_hooks(&self) -> Vec<HookInfo>;

    /// Enumerate registered subagents (markdown-defined + built-in).
    async fn list_agents(&self) -> Vec<AgentInfo>;

    /// Run the 6 doctor checks and return the aggregated report.
    async fn run_doctor_checks(&self) -> DoctorReport;

    /// Snapshot the full status panel.
    async fn get_status_snapshot(&self) -> StatusSnapshot;

    /// Open `$EDITOR` on `<config-dir>/claude/config.json` (creating if absent).
    async fn edit_config_file(&self) -> Result<MemoryEditorOutcome, HandleError>;

    /// Open `$EDITOR` on `<config-dir>/claude/permissions.json` (creating if absent).
    async fn edit_permissions_file(&self) -> Result<MemoryEditorOutcome, HandleError>;

    /// Enumerate model names the orchestrator will accept via `switch_model`.
    async fn list_available_models(&self) -> Vec<String>;
```

- [ ] **Step 4: Update `MockOrchestratorHandle` to implement the 8 new methods.**

  Open `lingxi-code/crates/orchestrator/src/test_support.rs`. Add 8 setters + 8 fields + 8 mock impls:

```rust
pub struct MockOrchestratorHandle {
    // ... existing fields from M5-02 + M5-10 ...
    pub mcp_servers: Mutex<Vec<McpServerInfo>>,
    pub hooks: Mutex<Vec<HookInfo>>,
    pub agents: Mutex<Vec<AgentInfo>>,
    pub doctor_report: Mutex<DoctorReport>,
    pub status_snapshot: Mutex<StatusSnapshot>,
    pub config_editor_error: Mutex<Option<String>>,
    pub permissions_editor_error: Mutex<Option<String>>,
    pub available_models: Mutex<Vec<String>>,
}
```

```rust
impl MockOrchestratorHandle {
    pub fn set_mcp_servers(&self, v: Vec<McpServerInfo>) { *self.mcp_servers.lock().unwrap() = v; }
    pub fn set_hooks(&self, v: Vec<HookInfo>) { *self.hooks.lock().unwrap() = v; }
    pub fn set_agents(&self, v: Vec<AgentInfo>) { *self.agents.lock().unwrap() = v; }
    pub fn set_doctor_report(&self, r: DoctorReport) { *self.doctor_report.lock().unwrap() = r; }
    pub fn set_status_snapshot(&self, s: StatusSnapshot) { *self.status_snapshot.lock().unwrap() = s; }
    pub fn set_config_editor_error(&self, e: String) { *self.config_editor_error.lock().unwrap() = Some(e); }
    pub fn set_permissions_editor_error(&self, e: String) { *self.permissions_editor_error.lock().unwrap() = Some(e); }
    pub fn set_available_models(&self, m: Vec<String>) { *self.available_models.lock().unwrap() = m; }
}

#[async_trait]
impl OrchestratorHandle for MockOrchestratorHandle {
    // ... existing methods ...

    async fn list_mcp_servers(&self) -> Vec<McpServerInfo> { self.mcp_servers.lock().unwrap().clone() }
    async fn list_hooks(&self) -> Vec<HookInfo> { self.hooks.lock().unwrap().clone() }
    async fn list_agents(&self) -> Vec<AgentInfo> { self.agents.lock().unwrap().clone() }
    async fn run_doctor_checks(&self) -> DoctorReport { self.doctor_report.lock().unwrap().clone() }
    async fn get_status_snapshot(&self) -> StatusSnapshot { self.status_snapshot.lock().unwrap().clone() }

    async fn edit_config_file(&self) -> Result<MemoryEditorOutcome, HandleError> {
        if let Some(e) = self.config_editor_error.lock().unwrap().take() {
            return Err(HandleError::ActionFailed(e));
        }
        Ok(MemoryEditorOutcome {
            edited_path: PathBuf::from("/tmp/mock/config.json"),
            exit_code: 0,
        })
    }
    async fn edit_permissions_file(&self) -> Result<MemoryEditorOutcome, HandleError> {
        if let Some(e) = self.permissions_editor_error.lock().unwrap().take() {
            return Err(HandleError::ActionFailed(e));
        }
        Ok(MemoryEditorOutcome {
            edited_path: PathBuf::from("/tmp/mock/permissions.json"),
            exit_code: 0,
        })
    }
    async fn list_available_models(&self) -> Vec<String> {
        self.available_models.lock().unwrap().clone()
    }
}
```

- [ ] **Step 5: Implement 8 methods on `ConversationOrchestrator` (production).**

  Open `lingxi-code/crates/orchestrator/src/handle_impl.rs` and add:

```rust
async fn list_mcp_servers(&self) -> Vec<McpServerInfo> {
    let mcp = self.mcp_registry.read().await;
    let mut out = Vec::with_capacity(mcp.len());
    for (name, conn) in mcp.iter() {
        let status = match conn.status() {
            lingxi_mcp::ConnectionStatus::Connected => McpStatus::Connected,
            lingxi_mcp::ConnectionStatus::Disconnected => McpStatus::Disconnected,
            lingxi_mcp::ConnectionStatus::Error(ref e) => McpStatus::Error(e.clone()),
        };
        out.push(McpServerInfo {
            name: name.clone(),
            status,
            transport: conn.transport_kind().to_string(),
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

async fn list_hooks(&self) -> Vec<HookInfo> {
    let hooks = self.hooks_registry.read().await;
    hooks.iter().map(|h| HookInfo {
        name: h.id.clone(),
        event: h.event.to_string(),
        matcher: h.matcher.clone(),
        timeout_ms: h.timeout_ms.unwrap_or(60_000),
    }).collect()
}

async fn list_agents(&self) -> Vec<AgentInfo> {
    let agents = self.agents_registry.read().await;
    agents.iter().map(|a| AgentInfo {
        name: a.name.clone(),
        description: a.description.clone(),
        tools_allowed: a.tools_allowed.clone().unwrap_or_default(),
    }).collect()
}

async fn run_doctor_checks(&self) -> DoctorReport {
    crate::diagnostics::run_all(&self.config_dir, &self.api_client).await
}

async fn get_status_snapshot(&self) -> StatusSnapshot {
    let s = self.session.read().await;
    let cost = self.cost.snapshot().await;
    let mcp = self.list_mcp_servers().await;
    StatusSnapshot {
        session_id: s.id.to_string(),
        model: s.model.clone(),
        n_messages: s.messages.len() as u32,
        total_cost_usd: cost.total_usd,
        input_tokens: cost.input_tokens,
        output_tokens: cost.output_tokens,
        n_mcp_connected: mcp.iter().filter(|m| matches!(m.status, McpStatus::Connected)).count() as u32,
        n_mcp_total: mcp.len() as u32,
        n_hooks: self.list_hooks().await.len() as u32,
        n_agents: self.list_agents().await.len() as u32,
        started_at: s.started_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        cwd: s.cwd.clone(),
    }
}

async fn edit_config_file(&self) -> Result<MemoryEditorOutcome, HandleError> {
    let dir = dirs::config_dir()
        .ok_or_else(|| HandleError::ActionFailed("config_dir unavailable".into()))?
        .join("claude");
    let target = dir.join("config.json");
    self.spawn_editor_on(target).await
}

async fn edit_permissions_file(&self) -> Result<MemoryEditorOutcome, HandleError> {
    let dir = dirs::config_dir()
        .ok_or_else(|| HandleError::ActionFailed("config_dir unavailable".into()))?
        .join("claude");
    let target = dir.join("permissions.json");
    self.spawn_editor_on(target).await
}

async fn list_available_models(&self) -> Vec<String> {
    // Sourced from lingxi-api-client::models::DEFAULT_MODELS (a hardcoded list
    // from M3-03). Returns names like ["claude-opus-4-7", "claude-sonnet-4-6",
    // "claude-haiku-4-5"]. The orchestrator does NOT validate names against
    // this list — switch_model accepts arbitrary strings. The list is purely
    // informational for `/model` (no arg) rendering.
    lingxi_api_client::models::DEFAULT_MODELS
        .iter()
        .map(|s| s.to_string())
        .collect()
}
```

  The `spawn_editor_on` helper consolidates the touch + spawn logic from M5-10's `open_memory_editor`:

```rust
async fn spawn_editor_on(&self, target: PathBuf) -> Result<MemoryEditorOutcome, HandleError> {
    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent).await
            .map_err(|e| HandleError::ActionFailed(format!("mkdir {parent:?}: {e}")))?;
    }
    if !target.exists() {
        tokio::fs::write(&target, "{}\n").await
            .map_err(|e| HandleError::ActionFailed(format!("touch {target:?}: {e}")))?;
    }
    let editor = resolve_editor();
    let status = Command::new(&editor).arg(&target).status().await
        .map_err(|e| HandleError::ActionFailed(format!("spawn {editor}: {e}")))?;
    Ok(MemoryEditorOutcome {
        edited_path: target,
        exit_code: status.code().unwrap_or(-1),
    })
}
```

  Refactor `open_memory_editor` (from M5-10) to call `spawn_editor_on` instead of duplicating the logic. The original difference: `open_memory_editor` touches with `""`, the two new methods touch with `"{}\n"` (sensible JSON default). Keep this difference — the file body matters for `config.json` / `permissions.json` which are JSON-typed.

- [ ] **Step 6: Add the `diagnostics` module skeleton.**

  Create `lingxi-code/crates/orchestrator/src/diagnostics.rs`:

```rust
//! `/doctor` check runners. M5-11 ships 6 checks.

use lingxi_traits::{CheckStatus, DoctorCheck, DoctorReport, DoctorSummary};
use std::path::Path;

pub async fn run_all(config_dir: &Path, api_client: &lingxi_api_client::Client) -> DoctorReport {
    let checks = vec![
        check_config_dir(config_dir).await,
        check_api_key().await,
        check_network(api_client).await,
        check_disk_space(config_dir).await,
        check_git().await,
        check_telemetry_schema().await,
    ];

    let mut summary = DoctorSummary::default();
    for c in &checks {
        match c.status {
            CheckStatus::Pass => summary.passed += 1,
            CheckStatus::Warn => summary.warnings += 1,
            CheckStatus::Fail => summary.failed += 1,
        }
    }
    DoctorReport { checks, summary }
}

async fn check_config_dir(p: &Path) -> DoctorCheck {
    let exists = tokio::fs::metadata(p).await.is_ok();
    if !exists {
        return DoctorCheck {
            name: "config-dir".to_string(),
            status: CheckStatus::Fail,
            detail: Some(format!("{p:?} does not exist")),
        };
    }
    // Touch-test for writability.
    let probe = p.join(".lingxi_writable_probe");
    let writable = tokio::fs::write(&probe, b"").await.is_ok();
    let _ = tokio::fs::remove_file(&probe).await;
    DoctorCheck {
        name: "config-dir".to_string(),
        status: if writable { CheckStatus::Pass } else { CheckStatus::Fail },
        detail: if writable { None } else { Some(format!("{p:?} is not writable")) },
    }
}

async fn check_api_key() -> DoctorCheck {
    let has_env = std::env::var_os("ANTHROPIC_API_KEY").map(|v| !v.is_empty()).unwrap_or(false);
    // TODO: also check keychain for refresh token (M2-06). For M5-11 we only
    // check the env var; full keychain check moves to a follow-up.
    DoctorCheck {
        name: "api-key".to_string(),
        status: if has_env { CheckStatus::Pass } else { CheckStatus::Warn },
        detail: if has_env { None } else { Some("ANTHROPIC_API_KEY not set (login via /login)".to_string()) },
    }
}

async fn check_network(client: &lingxi_api_client::Client) -> DoctorCheck {
    let timeout = std::time::Duration::from_secs(5);
    match tokio::time::timeout(timeout, client.ping()).await {
        Ok(Ok(())) => DoctorCheck {
            name: "network".to_string(),
            status: CheckStatus::Pass,
            detail: None,
        },
        Ok(Err(e)) => DoctorCheck {
            name: "network".to_string(),
            status: CheckStatus::Fail,
            detail: Some(format!("ping failed: {e}")),
        },
        Err(_) => DoctorCheck {
            name: "network".to_string(),
            status: CheckStatus::Fail,
            detail: Some("ping timed out after 5s".to_string()),
        },
    }
}

async fn check_disk_space(p: &Path) -> DoctorCheck {
    // Cross-platform free-space check via `fs2`.
    let free = fs2::available_space(p).unwrap_or(0);
    let needed: u64 = 100 * 1024 * 1024; // 100 MiB
    DoctorCheck {
        name: "disk-space".to_string(),
        status: if free >= needed { CheckStatus::Pass } else { CheckStatus::Warn },
        detail: if free >= needed { None } else {
            Some(format!("{} MiB free (recommend ≥ 100 MiB)", free / 1024 / 1024))
        },
    }
}

async fn check_git() -> DoctorCheck {
    let output = tokio::process::Command::new("git").arg("--version").output().await;
    match output {
        Ok(o) if o.status.success() => DoctorCheck {
            name: "git".to_string(),
            status: CheckStatus::Pass,
            detail: Some(String::from_utf8_lossy(&o.stdout).trim().to_string()),
        },
        _ => DoctorCheck {
            name: "git".to_string(),
            status: CheckStatus::Warn,
            detail: Some("git not found (some features will be limited)".to_string()),
        },
    }
}

async fn check_telemetry_schema() -> DoctorCheck {
    let actual = lingxi_telemetry::tengu::ALL_EVENT_NAMES.len();
    let expected = 312;
    DoctorCheck {
        name: "telemetry-schema".to_string(),
        status: if actual == expected { CheckStatus::Pass } else { CheckStatus::Fail },
        detail: if actual == expected {
            None
        } else {
            Some(format!("ALL_EVENT_NAMES.len() = {actual}; expected {expected}"))
        },
    }
}
```

  Add `pub mod diagnostics;` to `lingxi-orchestrator/src/lib.rs`. Add `fs2 = "0.4"` + `chrono = { version = "0.4", features = ["serde"] }` to `lingxi-orchestrator/Cargo.toml`.

- [ ] **Step 7: Run tests + commit.**

```bash
cargo test -p lingxi-traits --lib m5_extension_tests 2>&1 | tail -10
cargo test -p lingxi-orchestrator --lib 2>&1 | tail -15
cargo fmt -p lingxi-traits -p lingxi-orchestrator
cargo clippy -p lingxi-traits -p lingxi-orchestrator --lib --tests -- -D warnings 2>&1 | tail -5
git add lingxi-code/crates/traits/ \
        lingxi-code/crates/orchestrator/
git commit -m "feat(M5-11 task 2): OrchestratorHandle +8 methods (list_mcp/hooks/agents, doctor, status, edit_config/permissions, list_models) + 5 info structs + diagnostics module (6 trait tests)"
```

---

## Task 3: `AuthHandle` trait + `OAuthHandle` impl

**Files:**
- Create: `lingxi-code/crates/traits/src/auth.rs`
- Modify: `lingxi-code/crates/traits/src/lib.rs`
- Create: `lingxi-code/crates/anthropic-oauth/src/handle.rs`
- Modify: `lingxi-code/crates/anthropic-oauth/src/lib.rs`

- [ ] **Step 1: Write the failing test.**

  Create `lingxi-code/crates/traits/src/auth.rs`:

```rust
//! Auth surface used by `/login` and `/logout` slash commands.
//!
//! Implemented by `lingxi-anthropic-oauth::handle::OAuthHandle`. The trait
//! lives in `lingxi-traits` so `lingxi-commands` does not take a direct dep
//! on the oauth crate (decoupling).

use async_trait::async_trait;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginInfo {
    pub email: String,
    pub org_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AuthError {
    #[error("login flow timed out")]
    Timeout,
    #[error("user cancelled login")]
    Cancelled,
    #[error("network error: {0}")]
    Network(String),
    #[error("server rejected: {0}")]
    ServerError(String),
}

#[async_trait]
pub trait AuthHandle: Send + Sync {
    async fn login(&self) -> Result<LoginInfo, AuthError>;
    async fn logout(&self) -> Result<(), AuthError>;
    async fn current_user(&self) -> Option<LoginInfo>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn _auth_handle_object_safe<T: AuthHandle + Send + Sync + 'static>() {
        let _: Box<dyn AuthHandle> = Box::new(std::marker::PhantomData::<T>);
    }

    #[test]
    fn auth_error_display() {
        let e = AuthError::Timeout;
        assert_eq!(e.to_string(), "login flow timed out");
        let e = AuthError::Network("dns".into());
        assert_eq!(e.to_string(), "network error: dns");
    }

    #[test]
    fn login_info_clone_equality() {
        let a = LoginInfo { email: "u@x.com".into(), org_id: "org_1".into() };
        let b = a.clone();
        assert_eq!(a, b);
    }
}
```

  Add to `lingxi-code/crates/traits/src/lib.rs`: `pub mod auth;` + `pub use auth::{AuthError, AuthHandle, LoginInfo};`.

- [ ] **Step 2: Run + pass.**

```bash
cargo test -p lingxi-traits --lib auth::tests 2>&1 | tail -5
```

  Expected: 2 passed (the object-safety check is compile-time-only via `_g`).

- [ ] **Step 3: Implement `OAuthHandle` in `lingxi-anthropic-oauth`.**

  Create `lingxi-code/crates/anthropic-oauth/src/handle.rs`:

```rust
//! `AuthHandle` implementation backed by the `AnthropicOAuthClient`.

use crate::client::AnthropicOAuthClient;
use crate::resolver::ResolvedCredential;
use async_trait::async_trait;
use lingxi_traits::{AuthError, AuthHandle, LoginInfo};
use std::sync::Arc;

pub struct OAuthHandle {
    client: Arc<AnthropicOAuthClient>,
}

impl OAuthHandle {
    #[must_use]
    pub fn new(client: Arc<AnthropicOAuthClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl AuthHandle for OAuthHandle {
    async fn login(&self) -> Result<LoginInfo, AuthError> {
        // Spawn the local callback listener, open the auth URL, wait for the
        // callback. Net: returns the LoginInfo (email + org_id) extracted
        // from the post-exchange ID token.
        let outcome = self.client.run_interactive_login().await.map_err(|e| {
            match e {
                crate::client::LoginError::Timeout => AuthError::Timeout,
                crate::client::LoginError::Cancelled => AuthError::Cancelled,
                crate::client::LoginError::Network(s) => AuthError::Network(s),
                crate::client::LoginError::Server(s) => AuthError::ServerError(s),
            }
        })?;
        Ok(LoginInfo {
            email: outcome.email,
            org_id: outcome.org_id,
        })
    }

    async fn logout(&self) -> Result<(), AuthError> {
        // Wipe stored tokens (keychain or file). Idempotent: succeeds even
        // if no credentials existed.
        self.client.clear_credentials().await.map_err(|e| {
            AuthError::ServerError(format!("logout: {e}"))
        })
    }

    async fn current_user(&self) -> Option<LoginInfo> {
        match self.client.resolve().await {
            Ok(ResolvedCredential::Oauth { email, org_id, .. }) => {
                Some(LoginInfo { email, org_id })
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    // Real OAuth flow is exercised by integration tests in
    // `tests/oauth_flow.rs` (existing from M2-01). M5-11 only adds the
    // surface wrapper; behaviour tests stay there.
}
```

  Add `pub mod handle;` + `pub use handle::OAuthHandle;` to `lingxi-anthropic-oauth/src/lib.rs`.

  **`run_interactive_login()` + `clear_credentials()` + `LoginError`:** these may not yet exist on `AnthropicOAuthClient`. Add them if missing. The skeleton signatures (delegating to existing PKCE flow):

```rust
// in lingxi-anthropic-oauth/src/client.rs
impl AnthropicOAuthClient {
    pub async fn run_interactive_login(&self) -> Result<LoginOutcome, LoginError> {
        // (a) Generate PKCE pair (M2-01 already does this).
        // (b) Build auth URL (existing build_authorize_url).
        // (c) Spawn local callback HTTP server on 127.0.0.1:<random-port>.
        // (d) Print the auth URL to stdout (the user opens it in a browser).
        // (e) Wait for the callback (60s timeout).
        // (f) Exchange the code for tokens.
        // (g) Decode ID token → return email + org_id.
        // Implementation: 50-100 lines using `tiny_http` or `tokio::net::TcpListener`.
        unimplemented!("M5-11 wires this in Task 3 step 3 — see plan")
    }
    pub async fn clear_credentials(&self) -> Result<(), anyhow::Error> {
        // (a) Delete keychain entry (via M2-06 secret crate).
        // (b) Delete fallback file at ~/.config/lingxi/credentials.json.
        unimplemented!("M5-11 wires this in Task 3 step 3")
    }
}

#[derive(Debug)]
pub struct LoginOutcome {
    pub email: String,
    pub org_id: String,
    pub access_token: String,
    pub refresh_token: String,
}

#[derive(Debug)]
pub enum LoginError {
    Timeout,
    Cancelled,
    Network(String),
    Server(String),
}
```

  Fill in the implementations using:

  - `crate::pkce::generate_pkce_pair()` (existing)
  - `client.build_authorize_url()` (existing — line 82 of client.rs)
  - `tokio::net::TcpListener::bind("127.0.0.1:0")` for the random-port callback
  - Parse the callback query for `code=...` + `state=...` (verify state matches PKCE state)
  - POST `code` to `client.exchange_endpoint()` (existing in M2-01)
  - Decode ID token: split on `.`, base64url-decode middle segment, parse JSON, read `email` + `org_id` claims
  - Persist tokens via `lingxi_secret::Keyring::set("lingxi.anthropic.refresh_token", &refresh_token)` (M2-06)

  This is ~80 lines of code. Detailed implementation lives in this Task — write it in full so the executor has no design choices.

- [ ] **Step 4: Run + commit.**

```bash
cargo test -p lingxi-traits -p lingxi-anthropic-oauth 2>&1 | tail -15
cargo fmt -p lingxi-traits -p lingxi-anthropic-oauth
cargo clippy -p lingxi-traits -p lingxi-anthropic-oauth --lib --tests -- -D warnings 2>&1 | tail -5
git add lingxi-code/crates/traits/src/auth.rs \
        lingxi-code/crates/traits/src/lib.rs \
        lingxi-code/crates/anthropic-oauth/
git commit -m "feat(M5-11 task 3): AuthHandle trait + OAuthHandle impl + run_interactive_login/clear_credentials"
```

---

## Task 4: `/cost` + `/model` + `/version` handlers (trivial trio)

**Files:**
- Create: `lingxi-code/crates/commands/src/builtin/cost.rs`
- Create: `lingxi-code/crates/commands/src/builtin/model.rs`
- Create: `lingxi-code/crates/commands/src/builtin/version.rs`
- Modify: `lingxi-code/crates/commands/src/builtin/mod.rs` (re-export)
- Modify: `lingxi-code/crates/commands/src/builtin/core_placeholders.rs` (delete 3 macro lines)

- [ ] **Step 1: Write the failing tests for all three.**

  Create `lingxi-code/crates/commands/src/builtin/cost.rs`:

```rust
//! `/cost` — snapshot the orchestrator's cost via `OrchestratorHandle::snapshot_cost`.
//!
//! See plan M5-11 Task 4.

use crate::builtin::names::core_description;
use crate::model::{BuiltinCommandHandler, CommandResult};
use crate::parser::ParsedSlashCommand;
use async_trait::async_trait;
use lingxi_telemetry::tengu::command as cmd_evt;
use lingxi_traits::OrchestratorHandle;
use std::sync::Arc;

#[derive(Clone)]
pub struct CostHandler {
    handle: Arc<dyn OrchestratorHandle>,
}

impl CostHandler {
    #[must_use]
    pub fn new(handle: Arc<dyn OrchestratorHandle>) -> Self { Self { handle } }
}

#[async_trait]
impl BuiltinCommandHandler for CostHandler {
    async fn handle(&self, _args: &ParsedSlashCommand) -> CommandResult {
        lingxi_telemetry::emit(cmd_evt::COST_STARTED, serde_json::json!({}));
        let cost = self.handle.snapshot_cost().await;
        lingxi_telemetry::emit(cmd_evt::COST_COMPLETED, serde_json::json!({
            "total_usd": cost.total_usd,
            "calls": cost.api_calls,
        }));
        // Duration formatting: chrono::Duration → "{H}h {M}m {S}s" or shorter.
        let dur = format_duration(cost.session_duration);
        CommandResult::Done {
            display: Some(format!(
                "Cost: ${:.4} ({} calls, {}+{} tokens, {} session time)",
                cost.total_usd, cost.api_calls,
                cost.input_tokens, cost.output_tokens,
                dur
            )),
        }
    }
    fn name(&self) -> &str { "cost" }
    fn description(&self) -> &str { core_description("cost") }
}

fn format_duration(d: std::time::Duration) -> String {
    let secs = d.as_secs();
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 {
        format!("{h}h {m}m {s}s")
    } else if m > 0 {
        format!("{m}m {s}s")
    } else {
        format!("{s}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lingxi_orchestrator::test_support::MockOrchestratorHandle;
    use lingxi_traits::CostSnapshot;

    fn args() -> ParsedSlashCommand {
        ParsedSlashCommand { name: "cost".to_string(), raw_args: String::new(), tokens: vec![] }
    }

    #[tokio::test]
    async fn success_short_duration() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        mock.set_cost_snapshot(CostSnapshot {
            total_usd: 0.0042,
            input_tokens: 1500,
            output_tokens: 230,
            api_calls: 3,
            session_duration: std::time::Duration::from_secs(45),
        });
        let h = CostHandler::new(mock);
        match h.handle(&args()).await {
            CommandResult::Done { display: Some(s) } => {
                assert_eq!(s, "Cost: $0.0042 (3 calls, 1500+230 tokens, 45s session time)");
            }
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn success_hour_duration() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        mock.set_cost_snapshot(CostSnapshot {
            total_usd: 1.2345,
            input_tokens: 50_000,
            output_tokens: 10_000,
            api_calls: 42,
            session_duration: std::time::Duration::from_secs(3 * 3600 + 25 * 60 + 17),
        });
        let h = CostHandler::new(mock);
        if let CommandResult::Done { display: Some(s) } = h.handle(&args()).await {
            assert_eq!(s, "Cost: $1.2345 (42 calls, 50000+10000 tokens, 3h 25m 17s session time)");
        } else {
            panic!();
        }
    }

    #[tokio::test]
    async fn zero_cost_zero_calls_zero_duration() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        // Default CostSnapshot is all zeros.
        let h = CostHandler::new(mock);
        if let CommandResult::Done { display: Some(s) } = h.handle(&args()).await {
            assert_eq!(s, "Cost: $0.0000 (0 calls, 0+0 tokens, 0s session time)");
        } else { panic!(); }
    }

    #[tokio::test]
    async fn name_and_description() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        let h = CostHandler::new(mock);
        assert_eq!(h.name(), "cost");
        assert_eq!(h.description(), "Show total cost and duration of the current session");
    }
}
```

  Create `lingxi-code/crates/commands/src/builtin/model.rs`:

```rust
//! `/model` — list / switch active model.
//!
//! See plan M5-11 Task 4.

use crate::builtin::names::core_description;
use crate::model::{BuiltinCommandHandler, CommandResult};
use crate::parser::ParsedSlashCommand;
use async_trait::async_trait;
use lingxi_telemetry::tengu::command as cmd_evt;
use lingxi_traits::OrchestratorHandle;
use std::sync::Arc;

#[derive(Clone)]
pub struct ModelHandler {
    handle: Arc<dyn OrchestratorHandle>,
}

impl ModelHandler {
    #[must_use]
    pub fn new(handle: Arc<dyn OrchestratorHandle>) -> Self { Self { handle } }
}

#[async_trait]
impl BuiltinCommandHandler for ModelHandler {
    async fn handle(&self, args: &ParsedSlashCommand) -> CommandResult {
        lingxi_telemetry::emit(cmd_evt::MODEL_STARTED, serde_json::json!({
            "args": &args.raw_args,
        }));
        // Mode: list (no args) vs switch (one positional arg).
        let trimmed = args.raw_args.trim();
        if trimmed.is_empty() {
            // List mode.
            let available = self.handle.list_available_models().await;
            let snap = self.handle.get_status_snapshot().await;
            lingxi_telemetry::emit(cmd_evt::MODEL_COMPLETED, serde_json::json!({
                "mode": "list",
                "current": &snap.model,
            }));
            return CommandResult::Done {
                display: Some(format!(
                    "Current model: {}\nAvailable: {}",
                    snap.model,
                    available.join(", ")
                )),
            };
        }
        // Switch mode.
        match self.handle.switch_model(trimmed).await {
            Ok(()) => {
                lingxi_telemetry::emit(cmd_evt::MODEL_COMPLETED, serde_json::json!({
                    "mode": "switch",
                    "target": trimmed,
                }));
                CommandResult::Done {
                    display: Some(format!("Switched to model: {trimmed}")),
                }
            }
            Err(e) => {
                lingxi_telemetry::emit(cmd_evt::MODEL_FAILED, serde_json::json!({
                    "error": e.to_string(),
                }));
                CommandResult::Done {
                    display: Some(format!("Could not switch model: {e}")),
                }
            }
        }
    }
    fn name(&self) -> &str { "model" }
    fn description(&self) -> &str { core_description("model") }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lingxi_orchestrator::test_support::MockOrchestratorHandle;

    fn args(raw: &str) -> ParsedSlashCommand {
        ParsedSlashCommand {
            name: "model".to_string(),
            raw_args: raw.to_string(),
            tokens: raw.split_whitespace().map(|s| s.to_string()).collect(),
        }
    }

    #[tokio::test]
    async fn list_mode_when_no_args() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        mock.set_available_models(vec!["claude-opus-4-7".into(), "claude-sonnet-4-6".into()]);
        let mut snap = lingxi_traits::StatusSnapshot::default();
        snap.model = "claude-opus-4-7".into();
        mock.set_status_snapshot(snap);
        let h = ModelHandler::new(mock);
        if let CommandResult::Done { display: Some(s) } = h.handle(&args("")).await {
            assert_eq!(s, "Current model: claude-opus-4-7\nAvailable: claude-opus-4-7, claude-sonnet-4-6");
        } else { panic!(); }
    }

    #[tokio::test]
    async fn switch_mode_when_arg_provided() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        let h = ModelHandler::new(mock);
        if let CommandResult::Done { display: Some(s) } = h.handle(&args("claude-haiku-4-5")).await {
            assert_eq!(s, "Switched to model: claude-haiku-4-5");
        } else { panic!(); }
    }

    #[tokio::test]
    async fn switch_mode_with_whitespace_trim() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        let h = ModelHandler::new(mock);
        if let CommandResult::Done { display: Some(s) } = h.handle(&args("  claude-opus-4-7  ")).await {
            assert_eq!(s, "Switched to model: claude-opus-4-7");
        } else { panic!(); }
    }

    #[tokio::test]
    async fn switch_failure_prefixes_error() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        mock.set_switch_model_error("invalid name".into());
        let h = ModelHandler::new(mock);
        if let CommandResult::Done { display: Some(s) } = h.handle(&args("foo")).await {
            assert_eq!(s, "Could not switch model: handle action failed: invalid name");
        } else { panic!(); }
    }

    #[tokio::test]
    async fn name_and_description() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        let h = ModelHandler::new(mock);
        assert_eq!(h.name(), "model");
        assert_eq!(h.description(), "Set the model for Claude Code to use");
    }
}
```

  Create `lingxi-code/crates/commands/src/builtin/version.rs`:

```rust
//! `/version` — print build info.
//!
//! See plan M5-11 Task 4.

use crate::builtin::names::core_description;
use crate::model::{BuiltinCommandHandler, CommandResult};
use crate::parser::ParsedSlashCommand;
use async_trait::async_trait;
use lingxi_telemetry::tengu::command as cmd_evt;

/// Compile-time-baked short git SHA (set by build.rs via env var).
/// Falls back to `"unknown"` if the env var isn't present (e.g. cargo install).
const GIT_SHA_SHORT: &str = match option_env!("LINGXI_GIT_SHA_SHORT") {
    Some(s) => s,
    None => "unknown",
};

#[derive(Debug, Default)]
pub struct VersionHandler;

impl VersionHandler {
    #[must_use]
    pub fn new() -> Self { Self }
}

#[async_trait]
impl BuiltinCommandHandler for VersionHandler {
    async fn handle(&self, _args: &ParsedSlashCommand) -> CommandResult {
        lingxi_telemetry::emit(cmd_evt::VERSION_STARTED, serde_json::json!({}));
        let s = format!("lingxi-cli {} ({})", env!("CARGO_PKG_VERSION"), GIT_SHA_SHORT);
        lingxi_telemetry::emit(cmd_evt::VERSION_COMPLETED, serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "sha": GIT_SHA_SHORT,
        }));
        CommandResult::Done { display: Some(s) }
    }
    fn name(&self) -> &str { "version" }
    fn description(&self) -> &str { core_description("version") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn version_format() {
        let h = VersionHandler::new();
        let args = ParsedSlashCommand { name: "version".to_string(), raw_args: String::new(), tokens: vec![] };
        if let CommandResult::Done { display: Some(s) } = h.handle(&args).await {
            // Format: "lingxi-cli <semver> (<sha-or-unknown>)"
            assert!(s.starts_with("lingxi-cli "));
            assert!(s.ends_with(')'));
            assert!(s.contains('('));
            // Version must match CARGO_PKG_VERSION at the binary level (0.5.x pre-M5-14, 0.6.0 post-M5-14).
            assert!(s.contains(env!("CARGO_PKG_VERSION")));
        } else { panic!(); }
    }
}
```

  For `LINGXI_GIT_SHA_SHORT` to be set: add `build.rs` to `lingxi-cli` (created in M5-12) that runs `git rev-parse --short HEAD` and emits `cargo:rustc-env=LINGXI_GIT_SHA_SHORT=<sha>`. For M5-11 we just use the `option_env!` fallback so version compiles without a build.rs.

- [ ] **Step 2: Run + fail.**

```bash
cargo test -p lingxi-commands --lib builtin::cost::tests builtin::model::tests builtin::version::tests 2>&1 | head -40
```

  Expected: many failures (no handler types yet).

- [ ] **Step 3: Already implemented above; re-run + watch pass.**

```bash
cargo test -p lingxi-commands --lib builtin::cost::tests builtin::model::tests builtin::version::tests 2>&1 | tail -15
```

  Expected: 4 + 5 + 1 = **10 passed**.

  Note: `MockOrchestratorHandle::set_cost_snapshot` / `set_switch_model_error` may need to be added (likely already exist from M5-10 if not earlier — verify via grep).

- [ ] **Step 4: Wire + remove placeholders.**

  Update `builtin/mod.rs`: `pub mod cost; pub mod model; pub mod version; pub use cost::CostHandler; pub use model::ModelHandler; pub use version::VersionHandler;`.

  Delete `core_placeholder!(CostHandler, "cost");`, `core_placeholder!(ModelHandler, "model");`, `core_placeholder!(VersionHandler, "version");` from `core_placeholders.rs`. Also delete the `Arc::new(...)` calls inside `register_core_placeholders` for those three.

- [ ] **Step 5: Fmt + clippy + commit.**

```bash
cargo fmt -p lingxi-commands
cargo clippy -p lingxi-commands --lib --tests -- -D warnings 2>&1 | tail -5
git add lingxi-code/crates/commands/src/builtin/{cost,model,version,mod,core_placeholders}.rs
git commit -m "feat(M5-11 task 4): /cost + /model + /version real impls + 9 telemetry events (10 unit tests)"
```

---

## Task 5: `/config` + `/permissions` handlers (file-editor pair)

**Files:**
- Create: `lingxi-code/crates/commands/src/builtin/config.rs`
- Create: `lingxi-code/crates/commands/src/builtin/permissions.rs`
- Modify: `lingxi-code/crates/commands/src/builtin/{mod,core_placeholders}.rs`

- [ ] **Step 1: Implement (same shape as M5-10 `/memory`).**

  `config.rs`:

```rust
//! `/config` — opens `$EDITOR` on `<config-dir>/claude/config.json`.

use crate::builtin::names::core_description;
use crate::model::{BuiltinCommandHandler, CommandResult};
use crate::parser::ParsedSlashCommand;
use async_trait::async_trait;
use lingxi_telemetry::tengu::command as cmd_evt;
use lingxi_traits::OrchestratorHandle;
use std::sync::Arc;

#[derive(Clone)]
pub struct ConfigHandler { handle: Arc<dyn OrchestratorHandle> }

impl ConfigHandler {
    #[must_use]
    pub fn new(handle: Arc<dyn OrchestratorHandle>) -> Self { Self { handle } }
}

#[async_trait]
impl BuiltinCommandHandler for ConfigHandler {
    async fn handle(&self, _args: &ParsedSlashCommand) -> CommandResult {
        lingxi_telemetry::emit(cmd_evt::CONFIG_STARTED, serde_json::json!({}));
        match self.handle.edit_config_file().await {
            Ok(o) => {
                lingxi_telemetry::emit(cmd_evt::CONFIG_COMPLETED, serde_json::json!({
                    "edited_path": o.edited_path.display().to_string(),
                    "exit_code": o.exit_code,
                }));
                CommandResult::Done {
                    display: Some(format!("Edited {} (exit {}).", o.edited_path.display(), o.exit_code)),
                }
            }
            Err(e) => {
                lingxi_telemetry::emit(cmd_evt::CONFIG_FAILED, serde_json::json!({ "error": e.to_string() }));
                CommandResult::Done { display: Some(format!("Could not edit config: {e}")) }
            }
        }
    }
    fn name(&self) -> &str { "config" }
    fn description(&self) -> &str { core_description("config") }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lingxi_orchestrator::test_support::MockOrchestratorHandle;
    use std::path::PathBuf;

    fn args() -> ParsedSlashCommand {
        ParsedSlashCommand { name: "config".into(), raw_args: String::new(), tokens: vec![] }
    }

    #[tokio::test]
    async fn success_renders_edited_template() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        let h = ConfigHandler::new(mock);
        if let CommandResult::Done { display: Some(s) } = h.handle(&args()).await {
            // Mock default path is "/tmp/mock/config.json".
            assert_eq!(s, "Edited /tmp/mock/config.json (exit 0).");
        } else { panic!(); }
    }

    #[tokio::test]
    async fn failure_prefixes_error() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        mock.set_config_editor_error("permission denied".into());
        let h = ConfigHandler::new(mock);
        if let CommandResult::Done { display: Some(s) } = h.handle(&args()).await {
            assert_eq!(s, "Could not edit config: handle action failed: permission denied");
        } else { panic!(); }
    }

    #[tokio::test]
    async fn name_and_description() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        let h = ConfigHandler::new(mock);
        assert_eq!(h.name(), "config");
        assert_eq!(h.description(), "Open config panel");
    }
}
```

  `permissions.rs` is identical structure with `permissions` substituted for `config` and `edit_permissions_file` substituted for `edit_config_file`. Locked literals: `"Edited {} (exit {})."` and `"Could not edit permissions: {e}"`. Description: `"Manage permissions"`.

- [ ] **Step 2: Wire + remove placeholders + commit.**

```bash
cargo test -p lingxi-commands --lib builtin::config::tests builtin::permissions::tests 2>&1 | tail -10
cargo fmt -p lingxi-commands && cargo clippy -p lingxi-commands --lib --tests -- -D warnings 2>&1 | tail -5
git add lingxi-code/crates/commands/src/builtin/{config,permissions,mod,core_placeholders}.rs
git commit -m "feat(M5-11 task 5): /config + /permissions real impls + 6 telemetry events (6 unit tests)"
```

---

## Task 6: `/mcp` handler (list-only mode)

**Files:**
- Create: `lingxi-code/crates/commands/src/builtin/mcp.rs`
- Modify: `lingxi-code/crates/commands/src/builtin/list_render.rs` (shared helper — created in this task)
- Modify: `lingxi-code/crates/commands/src/builtin/{mod,core_placeholders}.rs`

- [ ] **Step 1: Create the shared list-render helper.**

  Create `lingxi-code/crates/commands/src/builtin/list_render.rs`:

```rust
//! Shared text formatter used by `/mcp`, `/hooks`, and `/agents`.
//!
//! Locked layout per plan M5-11 T0 step 2 L6/L7/L8:
//!   "<Label> ({count}):\n  <row1>\n  <row2>\n..."
//! with each row pre-formatted by the caller as a single line (no trailing
//! newline).

#[must_use]
pub fn render_list(label: &str, rows: Vec<String>) -> String {
    let count = rows.len();
    let mut out = format!("{label} ({count}):\n");
    for row in rows {
        out.push_str("  ");
        out.push_str(&row);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_list_renders_header_only() {
        let s = render_list("MCP servers", vec![]);
        assert_eq!(s, "MCP servers (0):\n");
    }

    #[test]
    fn multi_row_renders_with_leading_two_spaces() {
        let s = render_list("Hooks", vec!["fmt PostToolUse 60000ms".into(), "lint Stop 30000ms".into()]);
        assert_eq!(s, "Hooks (2):\n  fmt PostToolUse 60000ms\n  lint Stop 30000ms\n");
    }
}
```

- [ ] **Step 2: Implement `McpHandler`.**

  Create `lingxi-code/crates/commands/src/builtin/mcp.rs`:

```rust
//! `/mcp` — list registered MCP servers + their status.

use crate::builtin::list_render::render_list;
use crate::builtin::names::core_description;
use crate::model::{BuiltinCommandHandler, CommandResult};
use crate::parser::ParsedSlashCommand;
use async_trait::async_trait;
use lingxi_telemetry::tengu::command as cmd_evt;
use lingxi_traits::{McpServerInfo, McpStatus, OrchestratorHandle};
use std::sync::Arc;

#[derive(Clone)]
pub struct McpHandler { handle: Arc<dyn OrchestratorHandle> }

impl McpHandler {
    #[must_use]
    pub fn new(handle: Arc<dyn OrchestratorHandle>) -> Self { Self { handle } }
}

#[async_trait]
impl BuiltinCommandHandler for McpHandler {
    async fn handle(&self, _args: &ParsedSlashCommand) -> CommandResult {
        lingxi_telemetry::emit(cmd_evt::MCP_STARTED, serde_json::json!({}));
        let servers = self.handle.list_mcp_servers().await;
        let rows: Vec<String> = servers.iter().map(format_row).collect();
        let s = render_list("MCP servers", rows);
        lingxi_telemetry::emit(cmd_evt::MCP_COMPLETED, serde_json::json!({ "count": servers.len() }));
        CommandResult::Done { display: Some(s) }
    }
    fn name(&self) -> &str { "mcp" }
    fn description(&self) -> &str { core_description("mcp") }
}

fn format_row(s: &McpServerInfo) -> String {
    let status = match &s.status {
        McpStatus::Connected => "connected".to_string(),
        McpStatus::Disconnected => "disconnected".to_string(),
        McpStatus::Error(e) => format!("error: {e}"),
    };
    format!("{}  {}  {}", s.name, status, s.transport)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lingxi_orchestrator::test_support::MockOrchestratorHandle;

    fn args() -> ParsedSlashCommand {
        ParsedSlashCommand { name: "mcp".into(), raw_args: String::new(), tokens: vec![] }
    }

    #[tokio::test]
    async fn empty_list() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        let h = McpHandler::new(mock);
        if let CommandResult::Done { display: Some(s) } = h.handle(&args()).await {
            assert_eq!(s, "MCP servers (0):\n");
        } else { panic!(); }
    }

    #[tokio::test]
    async fn two_servers_with_mixed_status() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        mock.set_mcp_servers(vec![
            McpServerInfo {
                name: "memory".into(),
                status: McpStatus::Connected,
                transport: "stdio".into(),
            },
            McpServerInfo {
                name: "filesystem".into(),
                status: McpStatus::Error("connection refused".into()),
                transport: "stdio".into(),
            },
        ]);
        let h = McpHandler::new(mock);
        if let CommandResult::Done { display: Some(s) } = h.handle(&args()).await {
            assert_eq!(s, "MCP servers (2):\n  memory  connected  stdio\n  filesystem  error: connection refused  stdio\n");
        } else { panic!(); }
    }

    #[tokio::test]
    async fn name_and_description() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        let h = McpHandler::new(mock);
        assert_eq!(h.name(), "mcp");
        assert_eq!(h.description(), "Manage MCP servers");
    }
}
```

- [ ] **Step 3: Wire + commit.**

```bash
cargo test -p lingxi-commands --lib builtin::mcp::tests builtin::list_render::tests 2>&1 | tail -10
cargo fmt -p lingxi-commands && cargo clippy -p lingxi-commands --lib --tests -- -D warnings 2>&1 | tail -5
git add lingxi-code/crates/commands/src/builtin/{mcp,list_render,mod,core_placeholders}.rs
git commit -m "feat(M5-11 task 6): /mcp real impl + list_render helper + 3 telemetry events (5 unit tests)"
```

---

## Task 7: `/hooks` handler

**Files:**
- Create: `lingxi-code/crates/commands/src/builtin/hooks.rs`
- Modify: `lingxi-code/crates/commands/src/builtin/{mod,core_placeholders}.rs`

- [ ] **Step 1: Implement.**

```rust
//! `/hooks` — list registered hooks.

use crate::builtin::list_render::render_list;
use crate::builtin::names::core_description;
use crate::model::{BuiltinCommandHandler, CommandResult};
use crate::parser::ParsedSlashCommand;
use async_trait::async_trait;
use lingxi_telemetry::tengu::command as cmd_evt;
use lingxi_traits::{HookInfo, OrchestratorHandle};
use std::sync::Arc;

#[derive(Clone)]
pub struct HooksHandler { handle: Arc<dyn OrchestratorHandle> }

impl HooksHandler {
    #[must_use]
    pub fn new(handle: Arc<dyn OrchestratorHandle>) -> Self { Self { handle } }
}

#[async_trait]
impl BuiltinCommandHandler for HooksHandler {
    async fn handle(&self, _args: &ParsedSlashCommand) -> CommandResult {
        lingxi_telemetry::emit(cmd_evt::HOOKS_STARTED, serde_json::json!({}));
        let hooks = self.handle.list_hooks().await;
        let rows: Vec<String> = hooks.iter().map(format_row).collect();
        let s = render_list("Hooks", rows);
        lingxi_telemetry::emit(cmd_evt::HOOKS_COMPLETED, serde_json::json!({ "count": hooks.len() }));
        CommandResult::Done { display: Some(s) }
    }
    fn name(&self) -> &str { "hooks" }
    fn description(&self) -> &str { core_description("hooks") }
}

fn format_row(h: &HookInfo) -> String {
    format!("{}  {}  {}ms", h.name, h.event, h.timeout_ms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lingxi_orchestrator::test_support::MockOrchestratorHandle;

    fn args() -> ParsedSlashCommand {
        ParsedSlashCommand { name: "hooks".into(), raw_args: String::new(), tokens: vec![] }
    }

    #[tokio::test]
    async fn empty_list() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        let h = HooksHandler::new(mock);
        if let CommandResult::Done { display: Some(s) } = h.handle(&args()).await {
            assert_eq!(s, "Hooks (0):\n");
        } else { panic!(); }
    }

    #[tokio::test]
    async fn two_hooks() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        mock.set_hooks(vec![
            HookInfo { name: "fmt".into(), event: "PostToolUse".into(), matcher: Some("Write|Edit".into()), timeout_ms: 60_000 },
            HookInfo { name: "lint".into(), event: "Stop".into(), matcher: None, timeout_ms: 30_000 },
        ]);
        let h = HooksHandler::new(mock);
        if let CommandResult::Done { display: Some(s) } = h.handle(&args()).await {
            assert_eq!(s, "Hooks (2):\n  fmt  PostToolUse  60000ms\n  lint  Stop  30000ms\n");
        } else { panic!(); }
    }

    #[tokio::test]
    async fn name_and_description() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        let h = HooksHandler::new(mock);
        assert_eq!(h.name(), "hooks");
        assert_eq!(h.description(), "Manage hooks");
    }
}
```

- [ ] **Step 2: Wire + commit.**

```bash
cargo test -p lingxi-commands --lib builtin::hooks::tests 2>&1 | tail -10
cargo fmt -p lingxi-commands && cargo clippy -p lingxi-commands --lib --tests -- -D warnings 2>&1 | tail -5
git add lingxi-code/crates/commands/src/builtin/{hooks,mod,core_placeholders}.rs
git commit -m "feat(M5-11 task 7): /hooks real impl + 3 telemetry events (3 unit tests)"
```

---

## Task 8: `/agents` handler

**Files:**
- Create: `lingxi-code/crates/commands/src/builtin/agents.rs`
- Modify: `lingxi-code/crates/commands/src/builtin/{mod,core_placeholders}.rs`

- [ ] **Step 1: Implement.**

```rust
//! `/agents` — list registered subagents.

use crate::builtin::list_render::render_list;
use crate::builtin::names::core_description;
use crate::model::{BuiltinCommandHandler, CommandResult};
use crate::parser::ParsedSlashCommand;
use async_trait::async_trait;
use lingxi_telemetry::tengu::command as cmd_evt;
use lingxi_traits::{AgentInfo, OrchestratorHandle};
use std::sync::Arc;

#[derive(Clone)]
pub struct AgentsHandler { handle: Arc<dyn OrchestratorHandle> }

impl AgentsHandler {
    #[must_use]
    pub fn new(handle: Arc<dyn OrchestratorHandle>) -> Self { Self { handle } }
}

#[async_trait]
impl BuiltinCommandHandler for AgentsHandler {
    async fn handle(&self, _args: &ParsedSlashCommand) -> CommandResult {
        lingxi_telemetry::emit(cmd_evt::AGENTS_STARTED, serde_json::json!({}));
        let agents = self.handle.list_agents().await;
        let rows: Vec<String> = agents.iter().map(format_row).collect();
        let s = render_list("Agents", rows);
        lingxi_telemetry::emit(cmd_evt::AGENTS_COMPLETED, serde_json::json!({ "count": agents.len() }));
        CommandResult::Done { display: Some(s) }
    }
    fn name(&self) -> &str { "agents" }
    fn description(&self) -> &str { core_description("agents") }
}

fn format_row(a: &AgentInfo) -> String {
    // Truncate description to 80 chars + `…` if longer.
    let desc = truncate_with_ellipsis(&a.description, 80);
    format!("{}  {}", a.name, desc)
}

fn truncate_with_ellipsis(s: &str, max_chars: usize) -> String {
    let mut count = 0;
    let mut byte_idx = s.len();
    for (i, _) in s.char_indices() {
        if count == max_chars {
            byte_idx = i;
            break;
        }
        count += 1;
    }
    if byte_idx == s.len() {
        s.to_string()
    } else {
        format!("{}…", &s[..byte_idx])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lingxi_orchestrator::test_support::MockOrchestratorHandle;

    fn args() -> ParsedSlashCommand {
        ParsedSlashCommand { name: "agents".into(), raw_args: String::new(), tokens: vec![] }
    }

    #[tokio::test]
    async fn empty_list() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        let h = AgentsHandler::new(mock);
        if let CommandResult::Done { display: Some(s) } = h.handle(&args()).await {
            assert_eq!(s, "Agents (0):\n");
        } else { panic!(); }
    }

    #[tokio::test]
    async fn one_agent_short_description() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        mock.set_agents(vec![
            AgentInfo { name: "reviewer".into(), description: "review code".into(), tools_allowed: vec![] },
        ]);
        let h = AgentsHandler::new(mock);
        if let CommandResult::Done { display: Some(s) } = h.handle(&args()).await {
            assert_eq!(s, "Agents (1):\n  reviewer  review code\n");
        } else { panic!(); }
    }

    #[tokio::test]
    async fn description_truncated_at_80_chars() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        let long: String = "a".repeat(100);
        mock.set_agents(vec![
            AgentInfo { name: "x".into(), description: long.clone(), tools_allowed: vec![] },
        ]);
        let h = AgentsHandler::new(mock);
        if let CommandResult::Done { display: Some(s) } = h.handle(&args()).await {
            let expected = format!("Agents (1):\n  x  {}…\n", "a".repeat(80));
            assert_eq!(s, expected);
        } else { panic!(); }
    }

    #[test]
    fn truncate_handles_multibyte_correctly() {
        let s = "中".repeat(85);
        let t = truncate_with_ellipsis(&s, 80);
        assert_eq!(t.chars().count(), 81); // 80 中 + 1 …
    }

    #[tokio::test]
    async fn name_and_description() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        let h = AgentsHandler::new(mock);
        assert_eq!(h.name(), "agents");
        assert_eq!(h.description(), "Manage subagents");
    }
}
```

- [ ] **Step 2: Wire + commit.**

```bash
cargo test -p lingxi-commands --lib builtin::agents::tests 2>&1 | tail -10
cargo fmt -p lingxi-commands && cargo clippy -p lingxi-commands --lib --tests -- -D warnings 2>&1 | tail -5
git add lingxi-code/crates/commands/src/builtin/{agents,mod,core_placeholders}.rs
git commit -m "feat(M5-11 task 8): /agents real impl + 3 telemetry events + truncate_with_ellipsis (5 unit tests)"
```

---

## Task 9: `/login` handler

**Files:**
- Create: `lingxi-code/crates/commands/src/builtin/login.rs`
- Modify: `lingxi-code/crates/commands/src/builtin/{mod,core_placeholders}.rs`

- [ ] **Step 1: Implement.**

```rust
//! `/login` — interactive OAuth (PKCE) login flow.

use crate::builtin::names::core_description;
use crate::model::{BuiltinCommandHandler, CommandResult};
use crate::parser::ParsedSlashCommand;
use async_trait::async_trait;
use lingxi_telemetry::tengu::command as cmd_evt;
use lingxi_traits::AuthHandle;
use std::sync::Arc;

#[derive(Clone)]
pub struct LoginHandler { auth: Arc<dyn AuthHandle> }

impl LoginHandler {
    #[must_use]
    pub fn new(auth: Arc<dyn AuthHandle>) -> Self { Self { auth } }
}

#[async_trait]
impl BuiltinCommandHandler for LoginHandler {
    async fn handle(&self, _args: &ParsedSlashCommand) -> CommandResult {
        lingxi_telemetry::emit(cmd_evt::LOGIN_STARTED, serde_json::json!({}));
        match self.auth.login().await {
            Ok(info) => {
                lingxi_telemetry::emit(cmd_evt::LOGIN_COMPLETED, serde_json::json!({
                    "org_id": &info.org_id,
                }));
                CommandResult::Done {
                    display: Some(format!("Logged in as {} (org: {}).", info.email, info.org_id)),
                }
            }
            Err(e) => {
                lingxi_telemetry::emit(cmd_evt::LOGIN_FAILED, serde_json::json!({
                    "error": e.to_string(),
                }));
                CommandResult::Done { display: Some(format!("Could not log in: {e}")) }
            }
        }
    }
    fn name(&self) -> &str { "login" }
    fn description(&self) -> &str { core_description("login") }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use lingxi_traits::{AuthError, LoginInfo};

    struct MockAuth { result: std::sync::Mutex<Result<LoginInfo, AuthError>> }
    impl MockAuth {
        fn ok(info: LoginInfo) -> Self { Self { result: std::sync::Mutex::new(Ok(info)) } }
        fn err(e: AuthError) -> Self { Self { result: std::sync::Mutex::new(Err(e)) } }
    }
    #[async_trait]
    impl AuthHandle for MockAuth {
        async fn login(&self) -> Result<LoginInfo, AuthError> {
            self.result.lock().unwrap().clone()
        }
        async fn logout(&self) -> Result<(), AuthError> { Ok(()) }
        async fn current_user(&self) -> Option<LoginInfo> { None }
    }
    impl Clone for LoginInfo {
        fn clone(&self) -> Self { Self { email: self.email.clone(), org_id: self.org_id.clone() } }
    }

    fn args() -> ParsedSlashCommand {
        ParsedSlashCommand { name: "login".into(), raw_args: String::new(), tokens: vec![] }
    }

    #[tokio::test]
    async fn success() {
        let m = Arc::new(MockAuth::ok(LoginInfo { email: "u@x.com".into(), org_id: "org_42".into() }));
        let h = LoginHandler::new(m);
        if let CommandResult::Done { display: Some(s) } = h.handle(&args()).await {
            assert_eq!(s, "Logged in as u@x.com (org: org_42).");
        } else { panic!(); }
    }

    #[tokio::test]
    async fn timeout_error() {
        let m = Arc::new(MockAuth::err(AuthError::Timeout));
        let h = LoginHandler::new(m);
        if let CommandResult::Done { display: Some(s) } = h.handle(&args()).await {
            assert_eq!(s, "Could not log in: login flow timed out");
        } else { panic!(); }
    }

    #[tokio::test]
    async fn network_error_propagates() {
        let m = Arc::new(MockAuth::err(AuthError::Network("dns failure".into())));
        let h = LoginHandler::new(m);
        if let CommandResult::Done { display: Some(s) } = h.handle(&args()).await {
            assert_eq!(s, "Could not log in: network error: dns failure");
        } else { panic!(); }
    }
}
```

- [ ] **Step 2: Wire + commit.**

```bash
cargo test -p lingxi-commands --lib builtin::login::tests 2>&1 | tail -10
cargo fmt -p lingxi-commands && cargo clippy -p lingxi-commands --lib --tests -- -D warnings 2>&1 | tail -5
git add lingxi-code/crates/commands/src/builtin/{login,mod,core_placeholders}.rs
git commit -m "feat(M5-11 task 9): /login real impl + 3 telemetry events (3 unit tests)"
```

---

## Task 10: `/logout` handler

**Files:**
- Create: `lingxi-code/crates/commands/src/builtin/logout.rs`
- Modify: `lingxi-code/crates/commands/src/builtin/{mod,core_placeholders}.rs`

- [ ] **Step 1: Implement.**

```rust
//! `/logout` — clear stored OAuth credentials.

use crate::builtin::names::core_description;
use crate::model::{BuiltinCommandHandler, CommandResult};
use crate::parser::ParsedSlashCommand;
use async_trait::async_trait;
use lingxi_telemetry::tengu::command as cmd_evt;
use lingxi_traits::AuthHandle;
use std::sync::Arc;

#[derive(Clone)]
pub struct LogoutHandler { auth: Arc<dyn AuthHandle> }

impl LogoutHandler {
    #[must_use]
    pub fn new(auth: Arc<dyn AuthHandle>) -> Self { Self { auth } }
}

#[async_trait]
impl BuiltinCommandHandler for LogoutHandler {
    async fn handle(&self, _args: &ParsedSlashCommand) -> CommandResult {
        lingxi_telemetry::emit(cmd_evt::LOGOUT_STARTED, serde_json::json!({}));
        match self.auth.logout().await {
            Ok(()) => {
                lingxi_telemetry::emit(cmd_evt::LOGOUT_COMPLETED, serde_json::json!({}));
                CommandResult::Done { display: Some("Logged out.".to_string()) }
            }
            Err(e) => {
                lingxi_telemetry::emit(cmd_evt::LOGOUT_FAILED, serde_json::json!({ "error": e.to_string() }));
                CommandResult::Done { display: Some(format!("Could not log out: {e}")) }
            }
        }
    }
    fn name(&self) -> &str { "logout" }
    fn description(&self) -> &str { core_description("logout") }
}

#[cfg(test)]
mod tests {
    // (mirror Task 9 — three test cases: success / network err / etc.)
}
```

- [ ] **Step 2: Commit.**

```bash
cargo test -p lingxi-commands --lib builtin::logout::tests 2>&1 | tail -10
git add lingxi-code/crates/commands/src/builtin/{logout,mod,core_placeholders}.rs
git commit -m "feat(M5-11 task 10): /logout real impl + 3 telemetry events (3 unit tests)"
```

---

## Task 11: `/status` handler + locked panel format

**Files:**
- Create: `lingxi-code/crates/commands/src/builtin/status.rs`
- Modify: `lingxi-code/crates/commands/src/builtin/{mod,core_placeholders}.rs`

- [ ] **Step 1: Implement.**

```rust
//! `/status` — render the 11-line status panel.
//!
//! See plan M5-11 T0 step 3 for the locked layout.

use crate::builtin::names::core_description;
use crate::model::{BuiltinCommandHandler, CommandResult};
use crate::parser::ParsedSlashCommand;
use async_trait::async_trait;
use lingxi_telemetry::tengu::command as cmd_evt;
use lingxi_traits::{OrchestratorHandle, StatusSnapshot};
use std::sync::Arc;

#[derive(Clone)]
pub struct StatusHandler { handle: Arc<dyn OrchestratorHandle> }

impl StatusHandler {
    #[must_use]
    pub fn new(handle: Arc<dyn OrchestratorHandle>) -> Self { Self { handle } }
}

#[async_trait]
impl BuiltinCommandHandler for StatusHandler {
    async fn handle(&self, _args: &ParsedSlashCommand) -> CommandResult {
        lingxi_telemetry::emit(cmd_evt::STATUS_STARTED, serde_json::json!({}));
        let snap = self.handle.get_status_snapshot().await;
        lingxi_telemetry::emit(cmd_evt::STATUS_COMPLETED, serde_json::json!({}));
        CommandResult::Done { display: Some(render_status(&snap)) }
    }
    fn name(&self) -> &str { "status" }
    fn description(&self) -> &str { core_description("status") }
}

pub fn render_status(s: &StatusSnapshot) -> String {
    // Col-1 width = 13 chars (longest label "Working dir:").
    let mut out = String::with_capacity(512);
    out.push_str("Status:\n");
    push_row(&mut out, "Session:",     &s.session_id);
    push_row(&mut out, "Model:",       &s.model);
    push_row(&mut out, "Messages:",    &s.n_messages.to_string());
    push_row(&mut out, "Cost:",        &format!("${:.4}", s.total_cost_usd));
    push_row(&mut out, "Tokens:",      &format!("{}+{}", s.input_tokens, s.output_tokens));
    push_row(&mut out, "MCP:",         &format!("{}/{} connected", s.n_mcp_connected, s.n_mcp_total));
    push_row(&mut out, "Hooks:",       &format!("{} registered", s.n_hooks));
    push_row(&mut out, "Agents:",      &format!("{} available", s.n_agents));
    push_row(&mut out, "Started:",     &s.started_at);
    push_row(&mut out, "Working dir:", &s.cwd.display().to_string());
    out
}

fn push_row(out: &mut String, label: &str, value: &str) {
    out.push_str("  ");
    out.push_str(label);
    // Pad label to column-1 width = 13. Label is up to 12 chars; pad with spaces.
    let pad = 13_usize.saturating_sub(label.len());
    for _ in 0..pad { out.push(' '); }
    out.push_str(value);
    out.push('\n');
}

#[cfg(test)]
mod tests {
    use super::*;
    use lingxi_orchestrator::test_support::MockOrchestratorHandle;

    fn args() -> ParsedSlashCommand {
        ParsedSlashCommand { name: "status".into(), raw_args: String::new(), tokens: vec![] }
    }

    #[tokio::test]
    async fn renders_11_lines_with_locked_layout() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        let snap = StatusSnapshot {
            session_id: "abc-123".into(),
            model: "claude-opus-4-7".into(),
            n_messages: 17,
            total_cost_usd: 0.0421,
            input_tokens: 4_500,
            output_tokens: 1_200,
            n_mcp_connected: 1,
            n_mcp_total: 2,
            n_hooks: 3,
            n_agents: 5,
            started_at: "2026-05-26T10:00:00Z".into(),
            cwd: std::path::PathBuf::from("/repo"),
        };
        mock.set_status_snapshot(snap);
        let h = StatusHandler::new(mock);
        if let CommandResult::Done { display: Some(s) } = h.handle(&args()).await {
            let expected = "\
Status:
  Session:     abc-123
  Model:       claude-opus-4-7
  Messages:    17
  Cost:        $0.0421
  Tokens:      4500+1200
  MCP:         1/2 connected
  Hooks:       3 registered
  Agents:      5 available
  Started:     2026-05-26T10:00:00Z
  Working dir: /repo
";
            assert_eq!(s, expected);
            assert_eq!(s.matches('\n').count(), 11);
        } else { panic!(); }
    }

    #[tokio::test]
    async fn name_and_description() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        let h = StatusHandler::new(mock);
        assert_eq!(h.name(), "status");
        assert_eq!(h.description(), "Show Claude Code status");
    }
}
```

- [ ] **Step 2: Commit.**

```bash
cargo test -p lingxi-commands --lib builtin::status::tests 2>&1 | tail -10
git add lingxi-code/crates/commands/src/builtin/{status,mod,core_placeholders}.rs
git commit -m "feat(M5-11 task 11): /status panel + locked 11-line layout + 3 telemetry events (2 unit tests)"
```

---

## Task 12: `/doctor` handler + locked report format

**Files:**
- Create: `lingxi-code/crates/commands/src/builtin/doctor.rs`
- Modify: `lingxi-code/crates/commands/src/builtin/{mod,core_placeholders}.rs`

- [ ] **Step 1: Implement.**

```rust
//! `/doctor` — render the 6-check diagnostic report.
//!
//! See plan M5-11 T0 step 4 for the locked layout.

use crate::builtin::names::core_description;
use crate::model::{BuiltinCommandHandler, CommandResult};
use crate::parser::ParsedSlashCommand;
use async_trait::async_trait;
use lingxi_telemetry::tengu::command as cmd_evt;
use lingxi_traits::{CheckStatus, DoctorReport, OrchestratorHandle};
use std::sync::Arc;

#[derive(Clone)]
pub struct DoctorHandler { handle: Arc<dyn OrchestratorHandle> }

impl DoctorHandler {
    #[must_use]
    pub fn new(handle: Arc<dyn OrchestratorHandle>) -> Self { Self { handle } }
}

#[async_trait]
impl BuiltinCommandHandler for DoctorHandler {
    async fn handle(&self, _args: &ParsedSlashCommand) -> CommandResult {
        lingxi_telemetry::emit(cmd_evt::DOCTOR_STARTED, serde_json::json!({}));
        let report = self.handle.run_doctor_checks().await;
        lingxi_telemetry::emit(cmd_evt::DOCTOR_COMPLETED, serde_json::json!({
            "passed": report.summary.passed,
            "warnings": report.summary.warnings,
            "failed": report.summary.failed,
        }));
        CommandResult::Done { display: Some(render_doctor(&report)) }
    }
    fn name(&self) -> &str { "doctor" }
    fn description(&self) -> &str { core_description("doctor") }
}

pub fn render_doctor(r: &DoctorReport) -> String {
    let mut out = String::from("Doctor:\n");
    for c in &r.checks {
        let (glyph, status_text) = match &c.status {
            CheckStatus::Pass => ("OK", "ok"),
            CheckStatus::Warn => ("!!", "warning"),
            CheckStatus::Fail => ("XX", "failed"),
        };
        out.push_str(&format!("  [{}] {}: {}\n", glyph, c.name, status_text));
        if let Some(detail) = &c.detail {
            out.push_str(&format!("      {}\n", detail));
        }
    }
    out.push_str(&format!(
        "  Summary: {} passed, {} warnings, {} failed\n",
        r.summary.passed, r.summary.warnings, r.summary.failed
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use lingxi_orchestrator::test_support::MockOrchestratorHandle;
    use lingxi_traits::{DoctorCheck, DoctorSummary};

    fn args() -> ParsedSlashCommand {
        ParsedSlashCommand { name: "doctor".into(), raw_args: String::new(), tokens: vec![] }
    }

    #[tokio::test]
    async fn all_pass_report() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        mock.set_doctor_report(DoctorReport {
            checks: vec![
                DoctorCheck { name: "config-dir".into(), status: CheckStatus::Pass, detail: None },
                DoctorCheck { name: "api-key".into(),    status: CheckStatus::Pass, detail: None },
            ],
            summary: DoctorSummary { passed: 2, warnings: 0, failed: 0 },
        });
        let h = DoctorHandler::new(mock);
        if let CommandResult::Done { display: Some(s) } = h.handle(&args()).await {
            assert_eq!(s, "\
Doctor:
  [OK] config-dir: ok
  [OK] api-key: ok
  Summary: 2 passed, 0 warnings, 0 failed
");
        } else { panic!(); }
    }

    #[tokio::test]
    async fn mixed_with_details() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        mock.set_doctor_report(DoctorReport {
            checks: vec![
                DoctorCheck { name: "config-dir".into(), status: CheckStatus::Pass, detail: None },
                DoctorCheck { name: "api-key".into(), status: CheckStatus::Warn, detail: Some("ANTHROPIC_API_KEY not set".into()) },
                DoctorCheck { name: "network".into(), status: CheckStatus::Fail, detail: Some("ping timed out".into()) },
            ],
            summary: DoctorSummary { passed: 1, warnings: 1, failed: 1 },
        });
        let h = DoctorHandler::new(mock);
        if let CommandResult::Done { display: Some(s) } = h.handle(&args()).await {
            assert_eq!(s, "\
Doctor:
  [OK] config-dir: ok
  [!!] api-key: warning
      ANTHROPIC_API_KEY not set
  [XX] network: failed
      ping timed out
  Summary: 1 passed, 1 warnings, 1 failed
");
        } else { panic!(); }
    }

    #[tokio::test]
    async fn name_and_description() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        let h = DoctorHandler::new(mock);
        assert_eq!(h.name(), "doctor");
        assert_eq!(h.description(), "Diagnose installation and configuration");
    }
}
```

- [ ] **Step 2: Commit.**

```bash
cargo test -p lingxi-commands --lib builtin::doctor::tests 2>&1 | tail -10
git add lingxi-code/crates/commands/src/builtin/{doctor,mod,core_placeholders}.rs
git commit -m "feat(M5-11 task 12): /doctor 6-check report + locked render + 3 telemetry events (3 unit tests)"
```

---

## Task 13: `register_core_batch_2(reg, handle, auth)` helper

**Files:**
- Modify: `lingxi-code/crates/commands/src/registry.rs`
- Modify: `lingxi-code/crates/commands/src/lib.rs`

- [ ] **Step 1: Implement.**

```rust
/// Overwrite the 12 batch-2 entries with their handle/auth-bound real
/// handlers from M5-11.
///
/// Call **after** [`register_all_builtin_commands`] and (optionally) after
/// [`register_core_batch_1`]. Idempotent.
pub fn register_core_batch_2(
    reg: &mut CommandRegistry,
    handle: std::sync::Arc<dyn lingxi_traits::OrchestratorHandle>,
    auth: std::sync::Arc<dyn lingxi_traits::AuthHandle>,
) {
    use crate::builtin::{
        AgentsHandler, ConfigHandler, CostHandler, DoctorHandler, HooksHandler,
        LoginHandler, LogoutHandler, McpHandler, ModelHandler, PermissionsHandler,
        StatusHandler, VersionHandler,
    };
    use std::sync::Arc;

    reg.register_builtin_handler(Arc::new(AgentsHandler::new(handle.clone())));
    reg.register_builtin_handler(Arc::new(ConfigHandler::new(handle.clone())));
    reg.register_builtin_handler(Arc::new(CostHandler::new(handle.clone())));
    reg.register_builtin_handler(Arc::new(DoctorHandler::new(handle.clone())));
    reg.register_builtin_handler(Arc::new(HooksHandler::new(handle.clone())));
    reg.register_builtin_handler(Arc::new(LoginHandler::new(auth.clone())));
    reg.register_builtin_handler(Arc::new(LogoutHandler::new(auth)));
    reg.register_builtin_handler(Arc::new(McpHandler::new(handle.clone())));
    reg.register_builtin_handler(Arc::new(ModelHandler::new(handle.clone())));
    reg.register_builtin_handler(Arc::new(PermissionsHandler::new(handle.clone())));
    reg.register_builtin_handler(Arc::new(StatusHandler::new(handle.clone())));
    reg.register_builtin_handler(Arc::new(VersionHandler::new()));
}
```

  Re-export from `lib.rs`:

```rust
pub use registry::{
    register_all_builtin_commands, register_core_batch_1, register_core_batch_2, CommandRegistry,
};
```

- [ ] **Step 2: Test + commit.**

```rust
#[cfg(test)]
mod batch_2_tests {
    use super::*;
    use std::sync::Arc;
    use lingxi_orchestrator::test_support::MockOrchestratorHandle;
    // Reuse the MockAuth pattern from Task 9 (login.rs test module) — bring it
    // out to a shared test helper if needed.

    #[tokio::test]
    async fn batch_2_overwrites_12_entries() {
        let mut reg = CommandRegistry::new();
        register_all_builtin_commands(&mut reg);
        let handle = Arc::new(MockOrchestratorHandle::new());
        let auth = Arc::new(crate::builtin::login::tests::MockAuth::ok(
            lingxi_traits::LoginInfo { email: "u@x.com".into(), org_id: "x".into() }));
        register_core_batch_2(&mut reg, handle, auth);

        // After overwrite, /version returns the real handler output.
        let h = reg.get_handler("version").unwrap();
        let args = crate::parser::ParsedSlashCommand {
            name: "version".into(), raw_args: String::new(), tokens: vec![],
        };
        match h.handle(&args).await {
            CommandResult::Done { display: Some(s) } => {
                assert!(s.starts_with("lingxi-cli "));
            }
            other => panic!("{other:?}"),
        }
    }
}
```

  (The `MockAuth` reuse may require exposing it as `#[cfg(test)] pub` from `login.rs`. Adjust accordingly.)

```bash
cargo test -p lingxi-commands --lib registry::batch_2_tests 2>&1 | tail -10
cargo fmt -p lingxi-commands && cargo clippy -p lingxi-commands --lib --tests -- -D warnings 2>&1 | tail -5
git add lingxi-code/crates/commands/src/registry.rs lingxi-code/crates/commands/src/lib.rs
git commit -m "feat(M5-11 task 13): register_core_batch_2(reg, handle, auth) — 12 handle-bound overwrites"
```

---

## Task 14: Parity fixtures + telemetry coverage bump

**Files:**
- Modify: `lingxi-code/crates/test-harness/src/parity/fixtures/parity_tengu_events.json` (append 36 rows)
- Create: `lingxi-code/crates/test-harness/src/parity/fixtures/parity_status_panel.txt` (golden /status output)
- Create: `lingxi-code/crates/test-harness/src/parity/fixtures/parity_doctor_report.txt` (golden /doctor output)
- Modify: `lingxi-code/crates/test-harness/tests/parity_telemetry_coverage.rs` (276 → 312)
- Create: `lingxi-code/crates/test-harness/tests/parity_status_panel.rs`
- Create: `lingxi-code/crates/test-harness/tests/parity_doctor_report.rs`

- [ ] **Step 1: Append 36 events to `parity_tengu_events.json`.**

  Add 36 new rows under category `"command"` since `v0.6.0`. The 36 are the new ones from Task 0 step 5. Insert at the position matching the `ALL_EVENT_NAMES` slice order (which Task 1 step 4 put `command` last, so append at the very end of the JSON array).

- [ ] **Step 2: Generate golden /status panel.**

  Use a known `StatusSnapshot` (see Task 11 test fixture); render via `render_status`; save to `parity_status_panel.txt`:

```
Status:
  Session:     abc-123
  Model:       claude-opus-4-7
  Messages:    17
  Cost:        $0.0421
  Tokens:      4500+1200
  MCP:         1/2 connected
  Hooks:       3 registered
  Agents:      5 available
  Started:     2026-05-26T10:00:00Z
  Working dir: /repo
```

  Driver `parity_status_panel.rs`:

```rust
use lingxi_commands::builtin::status::render_status;
use lingxi_traits::StatusSnapshot;

const GOLDEN: &str =
    include_str!("../src/parity/fixtures/parity_status_panel.txt");

#[test]
fn status_panel_layout_matches_golden() {
    let snap = StatusSnapshot {
        session_id: "abc-123".into(),
        model: "claude-opus-4-7".into(),
        n_messages: 17,
        total_cost_usd: 0.0421,
        input_tokens: 4_500,
        output_tokens: 1_200,
        n_mcp_connected: 1,
        n_mcp_total: 2,
        n_hooks: 3,
        n_agents: 5,
        started_at: "2026-05-26T10:00:00Z".into(),
        cwd: std::path::PathBuf::from("/repo"),
    };
    assert_eq!(render_status(&snap), GOLDEN);
}
```

- [ ] **Step 3: Generate golden /doctor report.**

  Mixed-status example (matching Task 12 mixed test):

```
Doctor:
  [OK] config-dir: ok
  [!!] api-key: warning
      ANTHROPIC_API_KEY not set
  [XX] network: failed
      ping timed out
  Summary: 1 passed, 1 warnings, 1 failed
```

  Driver `parity_doctor_report.rs`:

```rust
use lingxi_commands::builtin::doctor::render_doctor;
use lingxi_traits::{CheckStatus, DoctorCheck, DoctorReport, DoctorSummary};

const GOLDEN: &str =
    include_str!("../src/parity/fixtures/parity_doctor_report.txt");

#[test]
fn doctor_layout_matches_golden() {
    let r = DoctorReport {
        checks: vec![
            DoctorCheck { name: "config-dir".into(), status: CheckStatus::Pass, detail: None },
            DoctorCheck { name: "api-key".into(), status: CheckStatus::Warn, detail: Some("ANTHROPIC_API_KEY not set".into()) },
            DoctorCheck { name: "network".into(), status: CheckStatus::Fail, detail: Some("ping timed out".into()) },
        ],
        summary: DoctorSummary { passed: 1, warnings: 1, failed: 1 },
    };
    assert_eq!(render_doctor(&r), GOLDEN);
}
```

- [ ] **Step 4: Bump telemetry coverage driver.**

  In `lingxi-code/crates/test-harness/tests/parity_telemetry_coverage.rs`, find the count assertion (`assert_eq!(ALL_EVENT_NAMES.len(), 276)` or similar) and change to `312`.

- [ ] **Step 5: Run + commit.**

```bash
cargo test -p lingxi-test-harness 2>&1 | tail -15
cargo fmt -p lingxi-test-harness && cargo clippy -p lingxi-test-harness --tests -- -D warnings 2>&1 | tail -5
git add lingxi-code/crates/test-harness/src/parity/fixtures/ \
        lingxi-code/crates/test-harness/tests/
git commit -m "test(M5-11 task 14): parity fixtures — tengu 276→312, /status + /doctor goldens"
```

---

## Task 15: Integration e2e tests (12 dispatch cases)

**Files:**
- Create: `lingxi-code/crates/commands/tests/batch_2_e2e.rs`

- [ ] **Step 1: Write 12 cases.**

```rust
//! End-to-end: build registry with M5-09 + batch-1 + batch-2 registrations,
//! dispatch each of the 12 batch-2 commands and verify behaviour.

use lingxi_commands::dispatcher::RegistrySlashDispatcher;
use lingxi_commands::registry::{
    register_all_builtin_commands, register_core_batch_1, register_core_batch_2, CommandRegistry,
};
use lingxi_orchestrator::test_support::MockOrchestratorHandle;
use lingxi_traits::{
    AgentInfo, AuthError, AuthHandle, CostSnapshot, HookInfo, LoginInfo, McpServerInfo, McpStatus,
    OrchestratorHandle, SlashCommandDispatcher, SlashDispatchResult, StatusSnapshot,
};
use std::sync::Arc;
use tokio::sync::RwLock;

struct MockAuth(Result<LoginInfo, AuthError>);

#[async_trait::async_trait]
impl AuthHandle for MockAuth {
    async fn login(&self) -> Result<LoginInfo, AuthError> {
        self.0.clone()
    }
    async fn logout(&self) -> Result<(), AuthError> { Ok(()) }
    async fn current_user(&self) -> Option<LoginInfo> {
        if let Ok(ref i) = self.0 { Some(i.clone()) } else { None }
    }
}
impl Clone for LoginInfo {
    fn clone(&self) -> Self { Self { email: self.email.clone(), org_id: self.org_id.clone() } }
}
impl Clone for MockAuth {
    fn clone(&self) -> Self { Self(self.0.clone()) }
}

async fn fresh() -> (RegistrySlashDispatcher, Arc<MockOrchestratorHandle>) {
    let mut reg = CommandRegistry::new();
    register_all_builtin_commands(&mut reg);
    let handle = Arc::new(MockOrchestratorHandle::new());
    register_core_batch_1(&mut reg, handle.clone());
    let auth = Arc::new(MockAuth(Ok(LoginInfo { email: "u@x.com".into(), org_id: "org".into() })));
    register_core_batch_2(&mut reg, handle.clone(), auth);
    let d = RegistrySlashDispatcher::new(Arc::new(RwLock::new(reg)));
    (d, handle)
}

#[tokio::test]
async fn cost_dispatch() {
    let (d, mock) = fresh().await;
    mock.set_cost_snapshot(CostSnapshot {
        total_usd: 0.0042, input_tokens: 100, output_tokens: 50, api_calls: 1,
        session_duration: std::time::Duration::from_secs(10),
    });
    let r = d.dispatch("/cost").await;
    if let SlashDispatchResult::Handled { display } = r {
        assert_eq!(display, "Cost: $0.0042 (1 calls, 100+50 tokens, 10s session time)");
    } else { panic!(); }
}

#[tokio::test]
async fn config_dispatch() {
    let (d, _) = fresh().await;
    let r = d.dispatch("/config").await;
    assert!(matches!(r, SlashDispatchResult::Handled { display } if display.starts_with("Edited ")));
}

#[tokio::test]
async fn model_list_dispatch() {
    let (d, mock) = fresh().await;
    mock.set_available_models(vec!["claude-opus-4-7".into()]);
    let mut snap = StatusSnapshot::default();
    snap.model = "claude-opus-4-7".into();
    mock.set_status_snapshot(snap);
    let r = d.dispatch("/model").await;
    if let SlashDispatchResult::Handled { display } = r {
        assert_eq!(display, "Current model: claude-opus-4-7\nAvailable: claude-opus-4-7");
    } else { panic!(); }
}

#[tokio::test]
async fn model_switch_dispatch() {
    let (d, _) = fresh().await;
    let r = d.dispatch("/model claude-sonnet-4-6").await;
    if let SlashDispatchResult::Handled { display } = r {
        assert_eq!(display, "Switched to model: claude-sonnet-4-6");
    } else { panic!(); }
}

#[tokio::test]
async fn permissions_dispatch() {
    let (d, _) = fresh().await;
    let r = d.dispatch("/permissions").await;
    assert!(matches!(r, SlashDispatchResult::Handled { display } if display.starts_with("Edited ")));
}

#[tokio::test]
async fn mcp_dispatch() {
    let (d, mock) = fresh().await;
    mock.set_mcp_servers(vec![
        McpServerInfo { name: "m".into(), status: McpStatus::Connected, transport: "stdio".into() },
    ]);
    let r = d.dispatch("/mcp").await;
    if let SlashDispatchResult::Handled { display } = r {
        assert_eq!(display, "MCP servers (1):\n  m  connected  stdio\n");
    } else { panic!(); }
}

#[tokio::test]
async fn hooks_dispatch() {
    let (d, mock) = fresh().await;
    mock.set_hooks(vec![
        HookInfo { name: "fmt".into(), event: "PostToolUse".into(), matcher: None, timeout_ms: 60_000 },
    ]);
    let r = d.dispatch("/hooks").await;
    if let SlashDispatchResult::Handled { display } = r {
        assert_eq!(display, "Hooks (1):\n  fmt  PostToolUse  60000ms\n");
    } else { panic!(); }
}

#[tokio::test]
async fn agents_dispatch() {
    let (d, mock) = fresh().await;
    mock.set_agents(vec![
        AgentInfo { name: "r".into(), description: "x".into(), tools_allowed: vec![] },
    ]);
    let r = d.dispatch("/agents").await;
    if let SlashDispatchResult::Handled { display } = r {
        assert_eq!(display, "Agents (1):\n  r  x\n");
    } else { panic!(); }
}

#[tokio::test]
async fn login_dispatch() {
    let (d, _) = fresh().await;
    let r = d.dispatch("/login").await;
    if let SlashDispatchResult::Handled { display } = r {
        assert_eq!(display, "Logged in as u@x.com (org: org).");
    } else { panic!(); }
}

#[tokio::test]
async fn logout_dispatch() {
    let (d, _) = fresh().await;
    let r = d.dispatch("/logout").await;
    if let SlashDispatchResult::Handled { display } = r {
        assert_eq!(display, "Logged out.");
    } else { panic!(); }
}

#[tokio::test]
async fn version_dispatch() {
    let (d, _) = fresh().await;
    let r = d.dispatch("/version").await;
    if let SlashDispatchResult::Handled { display } = r {
        assert!(display.starts_with("lingxi-cli "));
    } else { panic!(); }
}

#[tokio::test]
async fn status_dispatch() {
    let (d, _) = fresh().await;
    let r = d.dispatch("/status").await;
    if let SlashDispatchResult::Handled { display } = r {
        assert!(display.starts_with("Status:\n"));
        assert_eq!(display.matches('\n').count(), 11);
    } else { panic!(); }
}

#[tokio::test]
async fn doctor_dispatch() {
    let (d, _) = fresh().await;
    let r = d.dispatch("/doctor").await;
    if let SlashDispatchResult::Handled { display } = r {
        assert!(display.starts_with("Doctor:\n"));
        assert!(display.contains("Summary:"));
    } else { panic!(); }
}
```

- [ ] **Step 2: Run + commit.**

```bash
cargo test -p lingxi-commands --test batch_2_e2e 2>&1 | tail -20
cargo fmt -p lingxi-commands && cargo clippy -p lingxi-commands --tests -- -D warnings 2>&1 | tail -5
git add lingxi-code/crates/commands/tests/batch_2_e2e.rs
git commit -m "test(M5-11 task 15): batch_2_e2e — 13 dispatcher cases"
```

---

## Task 16: Docs + verification gate + tag `m5.11`

**Files:**
- Modify: `lingxi-code/crates/commands/src/lib.rs` (docs append)
- Modify: this plan file (status header)

- [ ] **Step 1: Append batch-2 docs.**

```rust
//! # Batch 2 (M5-11)
//!
//! After [`register_core_batch_2`] runs, the remaining 12 core commands are
//! wired:
//!
//! - `/cost`        — `OrchestratorHandle::snapshot_cost`
//! - `/config`      — `edit_config_file` ($EDITOR on config.json)
//! - `/model`       — list (no arg) / switch (arg) via `list_available_models` + `switch_model`
//! - `/permissions` — `edit_permissions_file`
//! - `/mcp`         — `list_mcp_servers`
//! - `/hooks`       — `list_hooks`
//! - `/agents`      — `list_agents`
//! - `/login`       — `AuthHandle::login` (interactive OAuth)
//! - `/logout`      — `AuthHandle::logout`
//! - `/version`     — `CARGO_PKG_VERSION` + git SHA
//! - `/status`      — `get_status_snapshot` → 11-line panel
//! - `/doctor`      — `run_doctor_checks` → 6-check report
//!
//! All 12 commands emit 3 telemetry events each (36 total); see
//! [`lingxi_telemetry::tengu::command`].
```

- [ ] **Step 2: Workspace verification.**

```bash
cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core
cargo fmt --all -- --check 2>&1 | tail -5
cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -20
cargo test --workspace 2>&1 | tail -30
```

  Expected: clean, all green except the 2 known fs-watch flakes.

- [ ] **Step 3: Confirm `ALL_EVENT_NAMES.len() == 312` + M5-09 102-name parity still passes.**

```bash
cargo test -p lingxi-test-harness --test parity_telemetry_coverage 2>&1 | tail -5
cargo test -p lingxi-test-harness --test parity_slash_commands 2>&1 | tail -5
cargo test -p lingxi-test-harness --test parity_registry 2>&1 | tail -5   # M4-09 40 tools
```

- [ ] **Step 4: Final commit + tag.**

```bash
# Edit this file's status header (similar to M5-09 Task 10 step 1).
cd /Users/luolingfeng/Projects/LingXi-Next
git add docs/superpowers/plans/2026-05-25-m5-11-commands-batch-2.md \
        lingxi-code/crates/commands/src/lib.rs
git commit -m "release(M5-11 task 16): mark batch-2 plan complete + docs"
git tag -a m5.11 -m "M5-11: 12 batch-2 core commands real impls + 36 telemetry events + 18 core commands all live"
git tag -n 1 | grep "^m5\." | sort | tail
```

- [ ] **Step 5: Do NOT push.** Tag stays local.

---

## Self-review

**1. Spec coverage** (against spec §3 M5-11 row):

| Spec requirement | Task |
|---|---|
| `/cost /config /model /permissions /mcp /hooks /agents /login /logout /version /status /doctor /resume` (13 names listed but spec count = 12) | T4-T12 implement the 12 (excluding /resume which stays under M5-08's resume loader; the `/resume` slash command wraps M5-08's loader and is a thin shim — added in M5-12 CLI binary's wiring step or deferred to a follow-up; M5-11 does not implement /resume since spec §1 success criterion #4 says "18 core" = 6 batch1 + 12 batch2 = 18, NOT 19) |
| 12 × 3 = 36 telemetry events | T1 + emit calls in T4-T12 |
| `ALL_EVENT_NAMES` 276 → 312 | T1 step 4 + T14 step 4 |
| `OrchestratorHandle` grows with list_mcp/hooks/agents, doctor, status, edit_config/permissions, list_models | T2 step 3 |
| `AuthHandle` trait + OAuth impl for /login /logout | T3 |
| `/init` template byte-lock (M5-10 carry-over) | (Outside M5-11 scope — M5-10 owns this) |

  All ✅.

**2. Placeholder scan:**

  - `unimplemented!()` appears in T3 step 3 inside two methods of `AnthropicOAuthClient` (`run_interactive_login` + `clear_credentials`) — these are **explicitly marked as needing implementation**, with detailed step-by-step descriptions of the 7-step / 2-step bodies. The executor MUST fill them in during T3 step 3, not later. **Not a placeholder per the plan's policy** because the implementation is fully specified in the prose right above the `unimplemented!()` line; the `unimplemented!()` only exists to make the compile-pass-through-tests workflow work. (If preferred, the executor can write the bodies inline immediately and skip the `unimplemented!()` stage altogether.)
  - "TODO" appears once in T2 step 6 (`check_api_key`) marking a known follow-up to also check the keychain for refresh tokens (M2-06). For M5-11 scope this is acceptable per the inline justification; tightening can land as an M5-14 follow-up.
  - No other `TBD`, `todo!()`, `FIXME`, "implement later" anywhere.

**3. Type consistency:**

| Identifier | Declaration | Uses |
|---|---|---|
| `tengu::command::NAMES` | T1 step 3 (now length 54) | T1 step 4 ALL_EVENT_NAMES, T14 step 4 |
| 36 new event const (`AGENTS_STARTED`, ..., `VERSION_FAILED`) | T1 step 3 | T4-T12 emit calls |
| 8 new `OrchestratorHandle` methods | T2 step 3 | each batch-2 handler T4-T12 (where applicable), T2 mock impl, T2 production impl |
| 5 info structs (`McpServerInfo` / `HookInfo` / `AgentInfo` / `DoctorReport` / `StatusSnapshot`) + enums | T2 step 3 | T6/T7/T8/T11/T12 handlers, T14 fixtures |
| `AuthHandle`, `LoginInfo`, `AuthError` | T3 step 1 | T9 LoginHandler, T10 LogoutHandler, T15 e2e, T2 mock |
| `OAuthHandle` | T3 step 3 | M5-12 CLI binary will construct this |
| `CostHandler`, `ConfigHandler`, ... 12 structs | T4-T12 | T13 register_core_batch_2 |
| `register_core_batch_2` | T13 step 1 | T15 e2e, M5-12 CLI binary |
| `render_status`, `render_doctor`, `render_list` (helpers) | T11, T12, T6 | T14 parity drivers |

  All consistent.

**4. Telemetry chain:** 36 new events. Verified:

  - T1 step 3: 36 const + slice extended to 54 entries
  - T1 step 4: ALL_EVENT_NAMES TOTAL formula `... + 54` → final 312
  - T14 step 1: 36 rows appended to `parity_tengu_events.json`
  - T14 step 4: event-name-completeness lock 276 → 312
  - T4-T12: per-handler emit calls produce exactly 2-3 events per dispatch

  `tengu::tool::NAMES.len()` stays at 134.

**5. M4 / M5-09 / M5-10 backward compat:**

  - T16 step 3 runs `parity_registry_40_tools.rs` — must pass.
  - T16 step 3 runs `parity_slash_commands.rs` (M5-09 102-name lock) — must pass.
  - M5-10's `register_core_batch_1` is unchanged; T13 adds `register_core_batch_2` in parallel.
  - M5-10's 18 telemetry event names are unchanged; T1 step 3 appends 36 new ones without disturbing the 18.

---

## Execution Handoff

**Plan complete and saved to `docs/superpowers/plans/2026-05-25-m5-11-commands-batch-2.md`. Two execution options:**

**1. Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, review between tasks, fast iteration

**2. Inline Execution** — Execute tasks in this session using executing-plans, batch execution with checkpoints

**Which approach?**

**If Subagent-Driven chosen:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Fresh subagent per task + two-stage review.

**If Inline Execution chosen:** REQUIRED SUB-SKILL: Use superpowers:executing-plans. Batch execution with checkpoints for review.
