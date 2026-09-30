# Hooks Permission Runtime Parity Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Close the highest-priority claude-code parity gap by proving and wiring real hook decision execution and permission-gate interactions instead of relying on no-op/test-only stubs.

**Architecture:** The `hooks` crate already exposes `HookExecutorImpl` with Builtin/Http/Command/Agent/Prompt arms and injection seams (`with_process_runner`, `with_agent_spawner`, `with_prompt_runner`, `with_async_registry`). The orchestrator already calls PreToolUse/PostToolUse/PermissionRequest/PermissionDenied at the dispatch chokepoint in `turn_loop.rs`, and `permission` already provides enforcing gates. This plan adds missing positive-path tests, hardens hook decision aggregation into orchestrator behavior, and wires real desktop composition seams while preserving documented stub behavior where claude-code itself or current host infrastructure is intentionally deferred.

**Tech Stack:** Rust 2021, Tokio async tests, `hooks`, `orchestrator`, `permission`, `client-adapter`, `engine-desktop`, `test-harness`. Verification uses focused crate tests plus `cargo clippy -p hooks -p orchestrator -p permission -p engine-desktop -- -D warnings` and targeted parity tests.

---

## Starting Context

- Audit source: global parity audit found Hooks runtime + Permission Gate as highest-risk parity gap.
- Existing positive coverage:
  - `lingxi-code/test-harness/tests/parity_hooks_runtime.rs`
  - `lingxi-code/orchestrator/tests/permission_hooks_test.rs`
  - `lingxi-code/permission/tests/prompting_gate_e2e_test.rs`
  - `lingxi-code/test-harness/tests/parity_mcp_permission_gate.rs`
- Existing implementation seams:
  - `hooks::HookExecutorImpl::{new, with_async_registry, with_agent_spawner, with_prompt_runner, with_process_runner}`
  - `hooks::PromptExecutor` + `HookPromptRunner`
  - `hooks::AgentExecutor` + `platform_api::subagent_spawn::SubagentSpawner`
  - `hooks::HttpExecutor` + `platform_api::HttpTransport`
  - `permission::PolicyPermissionGate`
  - `client_adapter::AdapterPermissionGate`
  - `orchestrator::turn_loop` PreToolUse + PermissionRequest/Denied chokepoint
- Known production composition gap:
  - `apps/engine-desktop/src/lib.rs` still imports `orchestrator::test_support::NoOpPermissionGate`.

## Constraints

- Do not remove test support types; existing tests still use no-op gates and mock executors.
- Do not broaden to unrelated hook events unless required for the tests below.
- Preserve documented deferred behavior for hook `Command` stdin/env gaps unless a task explicitly tests it.
- Do not rewrite `HookExecutorImpl` wholesale; use the existing injection seams.
- Keep exact parity tests passing.

---

### Task 1: Prompt Hook Decision Aggregation

**Files:**
- Test: `lingxi-code/hooks/tests/prompt_executor_decision_parity.rs`
- Modify: `lingxi-code/hooks/src/prompt_executor.rs` only if the failing test exposes a real gap
- Modify: `lingxi-code/hooks/src/executor.rs` only if the aggregate decision is not propagated

**Step 1: Write the failing test**

Create `lingxi-code/hooks/tests/prompt_executor_decision_parity.rs`:

```rust
use std::{path::PathBuf, sync::Arc, time::Duration};

use async_trait::async_trait;
use hooks::{
    definition::{HookDefinition, HookExecutor, HookSource},
    events::{HookEvent, HookEventType},
    prompt_executor::{HookPromptRunner, PromptHookRequest},
    registry::{HookContext, HookRegistry},
    response::HookDecision,
    HookExecutorImpl,
};
use protocol::{HookId, SessionId, ToolUseId};
use serde_json::json;
use tokio::sync::RwLock;

struct BlockingPromptRunner;

#[async_trait]
impl HookPromptRunner for BlockingPromptRunner {
    async fn run(
        &self,
        req: PromptHookRequest,
    ) -> Result<String, hooks::prompt_executor::PromptHookError> {
        assert!(req.prompt.contains("Bash"));
        Ok(r#"{"ok":false,"reason":"prompt says no"}"#.to_string())
    }
}

#[tokio::test]
async fn prompt_hook_block_decision_reaches_aggregate() {
    let mut registry = HookRegistry::new();
    registry.register(HookDefinition {
        id: HookId::new(),
        name: "prompt-blocker".to_string(),
        events: vec![HookEventType::PreToolUse],
        if_condition: None,
        executor: HookExecutor::Prompt {
            prompt: "Evaluate $ARGUMENTS".to_string(),
            model: Some("claude-haiku-4-5".to_string()),
        },
        source: HookSource::User,
        blocking: true,
        timeout: Some(Duration::from_secs(1)),
        priority: 0,
        once: false,
        status_message: None,
    });

    let exec = HookExecutorImpl::new(
        Arc::new(RwLock::new(registry)),
        Arc::new(hooks::test_support::UnusedHttp),
        Arc::new(hooks::test_support::UnusedRuntime),
    )
    .with_prompt_runner(Arc::new(BlockingPromptRunner));

    let aggregate = exec
        .execute(
            HookEvent::PreToolUse {
                tool_name: "Bash".to_string(),
                tool_input: json!({"command":"rm -rf /tmp/nope"}),
                tool_use_id: ToolUseId::new(),
            },
            HookContext {
                session_id: SessionId::new(),
                cwd: PathBuf::from("/work"),
                ..Default::default()
            },
        )
        .await;

    assert_eq!(aggregate.decision, Some(HookDecision::Block));
    assert_eq!(aggregate.reason.as_deref(), Some("prompt says no"));
}
```

**Step 2: Run test to verify RED**

Run:

```bash
cargo test -p hooks --test prompt_executor_decision_parity
```

Expected: FAIL if `hooks::test_support` helpers are not exported or prompt decisions do not reach aggregate. If it already passes, keep the test as regression coverage and move to Step 5.

**Step 3: Minimal implementation**

If the test fails because no test helpers exist, add test-only helpers under `hooks/src/test_support.rs` and export them behind `#[cfg(test)]` or `#[cfg(any(test, feature = "test-support"))]`:

```rust
pub struct UnusedHttp;
pub struct UnusedRuntime;
```

Implement the required traits by returning clear errors. Do not change production behavior.

If the test fails because the decision is dropped, inspect `hooks/src/prompt_executor.rs` and `hooks/src/executor.rs`:

- `PromptExecutor::execute` must parse `{"ok":false,"reason":"..."}` into `HookResponse { decision: Some(HookDecision::Block), reason, prevent_continuation: true, ... }`.
- `HookExecutorImpl::merge` must fold that response into `AggregateHookResult.decision` and `reason`.

**Step 4: Run test to verify GREEN**

Run:

```bash
cargo test -p hooks --test prompt_executor_decision_parity
cargo test -p test-harness --test parity_hooks_runtime
```

Expected: PASS.

**Step 5: Commit**

```bash
GIT_MASTER=1 git add lingxi-code/hooks/src lingxi-code/hooks/tests/prompt_executor_decision_parity.rs
GIT_MASTER=1 git commit -m "test(hooks): cover prompt hook decisions" \
  -m "Ultraworked with [Sisyphus](https://github.com/code-yeongyu/oh-my-openagent)" \
  -m "Co-authored-by: Sisyphus <clio-agent@sisyphuslabs.ai>"
```

---

### Task 2: Command Hook Positive Runner Path

**Files:**
- Test: `lingxi-code/hooks/tests/command_executor_process_runner_integration.rs`
- Modify: `lingxi-code/hooks/src/executor.rs` only if positive command runner path fails

**Step 1: Write the failing test**

Create `lingxi-code/hooks/tests/command_executor_process_runner_integration.rs` with a mock `ProcessRunner` and mock `Sandbox` that records `ProcessCommand.stdin` and returns stdout containing a valid hook JSON response:

```rust
// Test shape, not all imports shown:
// - build HookDefinition { executor: HookExecutor::Command { command: "hook", args: vec![], env: Default::default(), cwd: None } }
// - attach MockProcessRunner + MockSandbox via HookExecutorImpl::with_process_runner
// - run PreToolUse event
// - assert process saw stdin ending with '\n'
// - assert aggregate.decision == Some(HookDecision::Approve) for stdout:
//   {"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow"}}
```

**Step 2: Run test to verify RED**

Run:

```bash
cargo test -p hooks --test command_executor_process_runner_integration
```

Expected: FAIL if command output parsing or process/sandbox wiring is incomplete.

**Step 3: Minimal implementation**

In `hooks/src/executor.rs`, preserve current `Command arm not wired` behavior when either `process` or `sandbox` is absent. When both are present:

- Build `ProcessCommand` with command/args/cwd/env/timeout/stdin.
- Inject `CLAUDE_PROJECT_DIR` from `HookContext.project_dir.unwrap_or(ctx.cwd)`.
- Add trailing newline to stdin.
- Use `sandbox.bypass_with_audit(pcmd, "hook_command")`.
- Call `process.run(&sandboxed).await`.
- Parse stdout with `parse_response` using the expected hook event.
- Map non-zero exit to `HookOutcome::Error` while still surfacing stdout/stderr.

**Step 4: Run tests to verify GREEN**

Run:

```bash
cargo test -p hooks --test command_executor_process_runner_integration
cargo test -p test-harness --test parity_hooks_runtime
```

Expected: PASS. Existing parity test for missing process runner must still pass.

**Step 5: Commit**

```bash
GIT_MASTER=1 git add lingxi-code/hooks/src/executor.rs lingxi-code/hooks/tests/command_executor_process_runner_integration.rs
GIT_MASTER=1 git commit -m "test(hooks): cover command hook process runner" \
  -m "Ultraworked with [Sisyphus](https://github.com/code-yeongyu/oh-my-openagent)" \
  -m "Co-authored-by: Sisyphus <clio-agent@sisyphuslabs.ai>"
```

---

### Task 3: Agent Hook Positive Spawner Path

**Files:**
- Test: `lingxi-code/hooks/tests/agent_executor_with_spawner_success.rs`
- Modify: `lingxi-code/hooks/src/agent_executor.rs` only if positive path fails
- Modify: `lingxi-code/hooks/src/executor.rs` only if `with_agent_spawner` or inheritance plumbing fails

**Step 1: Write the failing test**

Create `lingxi-code/hooks/tests/agent_executor_with_spawner_success.rs` with a mock `SubagentSpawner` returning:

```rust
SubagentResult::Completed {
    content: serde_json::Value::String(
        r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"agent says no"}}"#.to_string()
    ),
    usage: Default::default(),
}
```

Assert:

- spawner received prompt containing hook prompt and event payload JSON
- aggregate decision is `Some(HookDecision::Block)`
- aggregate reason is `Some("agent says no")`

**Step 2: Run test to verify RED**

Run:

```bash
cargo test -p hooks --test agent_executor_with_spawner_success
```

Expected: FAIL if inheritance/test helper seam is missing.

**Step 3: Minimal implementation**

If missing helper types make the test verbose, add a small `hooks/tests/support.rs` module inside tests only. Do not change production code unless `AgentExecutor::execute` fails to:

- use injected spawner
- pass `ctx.inherit`
- parse `SubagentResult::Completed.content`
- propagate parsed hook decision

**Step 4: Run tests to verify GREEN**

Run:

```bash
cargo test -p hooks --test agent_executor_with_spawner_success
cargo test -p test-harness --test parity_hooks_runtime
```

Expected: PASS.

**Step 5: Commit**

```bash
GIT_MASTER=1 git add lingxi-code/hooks/src/agent_executor.rs lingxi-code/hooks/src/executor.rs lingxi-code/hooks/tests/agent_executor_with_spawner_success.rs
GIT_MASTER=1 git commit -m "test(hooks): cover agent hook spawner" \
  -m "Ultraworked with [Sisyphus](https://github.com/code-yeongyu/oh-my-openagent)" \
  -m "Co-authored-by: Sisyphus <clio-agent@sisyphuslabs.ai>"
```

---

### Task 4: Orchestrator Blocks Tool Execution on Hook Block

**Files:**
- Test: `lingxi-code/orchestrator/tests/hook_decision_blocks_tool.rs`
- Modify: `lingxi-code/orchestrator/src/turn_loop.rs` only if behavior fails

**Step 1: Write the failing test**

Create `lingxi-code/orchestrator/tests/hook_decision_blocks_tool.rs`:

- Register a `BuiltinHookHandler` for `PreToolUse` returning `HookResponse { decision: Some(HookDecision::Block), reason: Some("blocked by test"), ... }`.
- Register a tool that panics or records if called.
- Run a turn where assistant emits that tool.
- Assert:
  - tool was not called
  - output has `ToolResult` with error containing `Hook blocked: blocked by test`
  - no permission gate check occurred

**Step 2: Run test to verify RED**

Run:

```bash
cargo test -p orchestrator --test hook_decision_blocks_tool
```

Expected: FAIL if orchestrator still calls tool or permission gate.

**Step 3: Minimal implementation**

In `orchestrator/src/turn_loop.rs`, around the PreToolUse aggregate handling:

- If `pre_agg.decision == Some(HookDecision::Block)`, produce error `ToolResult`, emit output, and `continue` before permission gate and tool call.
- Preserve `pre_agg.prevent_continuation` OR-fold behavior.
- Preserve folding of `pre_agg.system_messages` into blocked tool-result content.

**Step 4: Run tests to verify GREEN**

Run:

```bash
cargo test -p orchestrator --test hook_decision_blocks_tool
cargo test -p orchestrator --test permission_hooks_test
```

Expected: PASS.

**Step 5: Commit**

```bash
GIT_MASTER=1 git add lingxi-code/orchestrator/src/turn_loop.rs lingxi-code/orchestrator/tests/hook_decision_blocks_tool.rs
GIT_MASTER=1 git commit -m "test(orchestrator): cover hook block enforcement" \
  -m "Ultraworked with [Sisyphus](https://github.com/code-yeongyu/oh-my-openagent)" \
  -m "Co-authored-by: Sisyphus <clio-agent@sisyphuslabs.ai>"
```

---

### Task 5: Non-Builtin Hook Approval Bypasses Permission Gate

**Files:**
- Test: `lingxi-code/orchestrator/tests/hook_decision_non_builtin_approval.rs`
- Modify: `lingxi-code/orchestrator/src/turn_loop.rs` only if behavior fails

**Step 1: Write the failing tests**

Create `lingxi-code/orchestrator/tests/hook_decision_non_builtin_approval.rs` with two cases:

1. HTTP PreToolUse hook returns:
   `{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow"}}`
2. Prompt PreToolUse hook returns `{"ok": true}` or `{"ok": false}` depending on desired approve/block shape. For approval specifically, use a Prompt runner response that maps to an approval if the schema supports it; if prompt hooks only support block/no-decision, document that and keep HTTP/Agent approval cases.

For approval cases, assert:

- permission gate was not consulted
- tool was called
- PermissionRequest/Denied hooks did not fire

**Step 2: Run tests to verify RED**

Run:

```bash
cargo test -p orchestrator --test hook_decision_non_builtin_approval
```

Expected: FAIL if orchestrator bypass is hardcoded only for builtin paths or if the selected non-builtin hook cannot produce approval.

**Step 3: Minimal implementation**

If the aggregate already exposes `HookDecision::Approve`, no orchestrator change should be needed. If it fails:

- Ensure `HookExecutorImpl::merge` preserves approval decisions independent of executor arm.
- Ensure `turn_loop.rs` checks aggregate decision only, not executor kind.

**Step 4: Run tests to verify GREEN**

Run:

```bash
cargo test -p orchestrator --test hook_decision_non_builtin_approval
cargo test -p orchestrator --test permission_hooks_test
cargo test -p test-harness --test parity_hooks_runtime
```

Expected: PASS.

**Step 5: Commit**

```bash
GIT_MASTER=1 git add lingxi-code/orchestrator/src/turn_loop.rs lingxi-code/orchestrator/tests/hook_decision_non_builtin_approval.rs lingxi-code/hooks/src/executor.rs
GIT_MASTER=1 git commit -m "test(orchestrator): cover hook approval bypass" \
  -m "Ultraworked with [Sisyphus](https://github.com/code-yeongyu/oh-my-openagent)" \
  -m "Co-authored-by: Sisyphus <clio-agent@sisyphuslabs.ai>"
```

---

### Task 6: Desktop Composition Uses Policy Permission Gate Instead of NoOp

**Files:**
- Test: `crates/apps/engine-desktop/tests/permission_wiring_test.rs`
- Modify: `crates/apps/engine-desktop/src/lib.rs`

**Step 1: Write the failing test**

Create `crates/apps/engine-desktop/tests/permission_wiring_test.rs`:

- Use the desktop build entrypoint or the smallest exposed helper that constructs orchestrator dependencies.
- Assert the permission gate is not `NoOpPermissionGate` in default desktop composition.
- If direct type inspection is impossible, configure a deny policy and assert a known denied tool produces a deny result rather than Allow.

**Step 2: Run test to verify RED**

Run:

```bash
cargo test -p engine-desktop --test permission_wiring_test
```

Expected: FAIL if desktop still injects `NoOpPermissionGate`.

**Step 3: Minimal implementation**

In `apps/engine-desktop/src/lib.rs`:

- Stop importing `orchestrator::test_support::NoOpPermissionGate` for production build wiring.
- Construct `permission::PermissionPolicy` from loaded settings tiers already available in desktop build.
- Wrap it in `permission::PolicyPermissionGate`.
- Use `client_adapter::AdapterPermissionGate` or `permission::InteractivePromptingGate` as the inner gate depending on the existing composition seam:
  - For CLI/headless, if no UI approval channel exists, use a fail-closed adapter or configured default policy.
  - Do not block non-interactive tests on stdin.

If current desktop build function does not construct the orchestrator directly, introduce a pure helper:

```rust
pub fn desktop_permission_gate_from_policy(
    policy: Arc<permission::PermissionPolicy>,
    inner: Arc<dyn PermissionGate>,
) -> Arc<dyn PermissionGate> {
    Arc::new(permission::PolicyPermissionGate::new(policy, inner))
}
```

Test that helper first, then wire it at the real construction site in the CLI/host binary follow-up.

**Step 4: Run tests to verify GREEN**

Run:

```bash
cargo test -p engine-desktop --test permission_wiring_test
cargo test -p permission
```

Expected: PASS.

**Step 5: Commit**

```bash
GIT_MASTER=1 git add crates/apps/engine-desktop/src/lib.rs crates/apps/engine-desktop/tests/permission_wiring_test.rs
GIT_MASTER=1 git commit -m "feat(engine-desktop): wire policy permission gate" \
  -m "Ultraworked with [Sisyphus](https://github.com/code-yeongyu/oh-my-openagent)" \
  -m "Co-authored-by: Sisyphus <clio-agent@sisyphuslabs.ai>"
```

---

### Task 7: Desktop Composition Wires Hook Prompt Runner and Async Registry

**Files:**
- Test: `crates/apps/engine-desktop/tests/hook_executor_wiring_test.rs`
- Modify: `crates/apps/engine-desktop/src/lib.rs`
- Modify: `apps/cli/host/src/main.rs` only if the actual orchestrator construction lives there

**Step 1: Write the failing test**

Create `crates/apps/engine-desktop/tests/hook_executor_wiring_test.rs`:

- Build the desktop hook executor helper with mock HTTP/runtime/process/sandbox/agent/prompt seams.
- Assert resulting executor:
  - has prompt runner wired by executing a prompt hook successfully
  - has async registry wired by executing a non-blocking hook and collecting completion
  - preserves missing command/process behavior if the test deliberately omits process runner

**Step 2: Run test to verify RED**

Run:

```bash
cargo test -p engine-desktop --test hook_executor_wiring_test
```

Expected: FAIL if no desktop hook executor builder exists.

**Step 3: Minimal implementation**

In `apps/engine-desktop/src/lib.rs`, add a helper that constructs the real executor:

```rust
pub fn desktop_hook_executor(
    registry: Arc<RwLock<hooks::HookRegistry>>,
    http: Arc<dyn platform_api::HttpTransport>,
    runtime: Arc<dyn platform_api::RuntimeSpawner>,
    prompt_runner: Arc<dyn hooks::HookPromptRunner>,
    async_registry: Arc<hooks::AsyncHookRegistry>,
) -> Arc<hooks::HookExecutorImpl> {
    Arc::new(
        hooks::HookExecutorImpl::new(registry, http, runtime)
            .with_prompt_runner(prompt_runner)
            .with_async_registry(async_registry)
    )
}
```

Add process/agent spawner parameters if production seams are already available.

**Step 4: Run tests to verify GREEN**

Run:

```bash
cargo test -p engine-desktop --test hook_executor_wiring_test
cargo test -p test-harness --test parity_hooks_runtime
```

Expected: PASS.

**Step 5: Commit**

```bash
GIT_MASTER=1 git add crates/apps/engine-desktop/src/lib.rs crates/apps/engine-desktop/tests/hook_executor_wiring_test.rs
GIT_MASTER=1 git commit -m "feat(engine-desktop): wire hook executor seams" \
  -m "Ultraworked with [Sisyphus](https://github.com/code-yeongyu/oh-my-openagent)" \
  -m "Co-authored-by: Sisyphus <clio-agent@sisyphuslabs.ai>"
```

---

### Task 8: Final Parity Verification

**Files:**
- All changed files.

**Step 1: Run hook tests**

Run:

```bash
cargo test -p hooks
cargo test -p test-harness --test parity_hooks_runtime
```

Expected: PASS.

**Step 2: Run permission/orchestrator tests**

Run:

```bash
cargo test -p permission
cargo test -p orchestrator --test permission_hooks_test
cargo test -p orchestrator --test hook_decision_blocks_tool
cargo test -p orchestrator --test hook_decision_non_builtin_approval
cargo test -p test-harness --test parity_mcp_permission_gate
```

Expected: PASS.

**Step 3: Run desktop wiring tests**

Run:

```bash
cargo test -p engine-desktop --test permission_wiring_test
cargo test -p engine-desktop --test hook_executor_wiring_test
```

Expected: PASS.

**Step 4: Run clippy**

Run:

```bash
cargo clippy -p hooks -p orchestrator -p permission -p engine-desktop -- -D warnings
```

Expected: PASS.

**Step 5: Run workspace compile check**

Run:

```bash
cargo test --workspace --no-run
```

Expected: PASS.

---

## Out of Scope

- Do not implement unrelated hook events beyond the tests above.
- Do not change slash command stubs.
- Do not implement MCP server/plugin marketplace.
- Do not change provider codecs.
- Do not require real external HTTP calls or real subagent model calls in tests; use mocks.

## Follow-up After This Plan

- Audit hook async completion UI surfacing and background task rows.
- Add runtime telemetry capture tests for actual `tracing::info!` hook/permission events if the project accepts a test-only tracing subscriber dependency.
- Expand `allowedEnvVars` interpolation for hook HTTP headers if product scope requires it.
