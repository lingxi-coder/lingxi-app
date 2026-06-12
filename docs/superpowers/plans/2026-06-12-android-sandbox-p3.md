# Android Sandbox P3 Implementation Plan — the mobile `Shell` tool

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Expose a model-facing, mobile-only `Shell` tool that runs deny-net commands through the P2 in-engine Minijail runner. It is registered ONLY when the device + config gate passes (capability probe OK + `enable_shell` + the D11 secrets gate), declares the mksh dialect + the probed toybox applet inventory in its prompt, and refuses network-intent commands with an advisory pointing at the (future) structured Git tool. Desktop `BashTool` and iOS are untouched.

**Architecture:** Spec r3 `docs/superpowers/specs/2026-06-12-android-sandbox-shell-design.md` §Shell tool + §Registration gates + D9/D10/D11. The tool is a new `tools/shell-mobile` crate implementing the `Tool` trait; its `call()` builds a deny-net `SandboxPolicy`, runs it through `ctx.sandbox.prepare()` → `ctx.process.run()` (the P2 runner). The registration gate + prompt data are android-specific, so they are computed in `android-aar build_android_engine` (the only place holding the concrete `AndroidPlatform` + the probed capability cache) and threaded through `MobileConfig` → `BuiltinToolContext` into `register_mobile_tools`.

**Predecessor:** P0a+P1+P2 merged to main at `1fa6f68b`. The runner executes jailed commands (device-verified); `enable_shell`/D11 gates are defined but **not yet consulted** — P3 consults them. This branch (`android-sandbox-p3`) is cut from main.

**Key findings from recon (shape the plan):**
- The desktop `BashTool::check_permissions` is itself a stub (`"allow-all-gate (M4-02 default)"`, `tools/shell/src/bash.rs:418`). Real allow/ask gating lives in the engine's `AdapterPermissionGate` (already wired on mobile via `PermissionRequestSink` → Kotlin UI; `host.rs`). So the mobile `Shell` tool's `check_permissions` MIRRORS the desktop stub (Allow + reason) — D4 "reuse desktop allowlist/ask rules" is satisfied by the existing engine gate, NOT by tool-level rule code. Do not re-implement a rules engine here.
- `tree-sitter-bash` exists in the `permission` crate but behind the off-by-default `bash-ast` feature; the desktop tool does not use it. The Shell tool's **network-intent advisory** uses a pragmatic shell-segment tokenizer (split on `|`, `&&`, `||`, `;`, `&`, newlines; take each segment's head word), NOT a new tree-sitter dep.
- `BuiltinToolContext` (`tool-api/src/builtin_context.rs`) is constructed in `apps/engine-mobile/src/host.rs:438` from `MobileConfig` + the `Arc<dyn Platform>`. It already carries many `Option<...>` capability fields. The gate/prompt carrier is added there.
- `register_mobile_tools` (`apps/engine-mobile/src/lib.rs:101`) registers the mobile tool set; it is shared shape (iOS calls the same `build_mobile_engine`). Gating the Shell registration on a ctx field keeps iOS/desktop unaffected (field defaults to `None`).
- `Tool` trait (`tool-api/src/tool_trait.rs`): `name`, `input_schema`, `is_enabled(&ToolStaticContext{feature_flags})`, `check_permissions`, `prompt(PromptOptions)`, `call(...)`, `description`, `max_result_size_chars`, `is_concurrency_safe`, etc. `ShellMobileTool { ctx: BuiltinToolContext }` mirrors `ClipboardTool`.

**Spec invariants P3 must not break:**
- Tool is **absent, not erroring** when the gate fails (register-time gate, not a runtime error).
- Shell is **deny-net always** (D10): `call()` builds `NetworkPolicy::Disabled`; network-intent commands get an advisory error, never a network grant.
- The P2 runner's fail-closed behavior is unchanged; the tool just feeds it a policy.
- `platform-android` stays `#![forbid(unsafe_code)]`; no new unsafe in P3 (pure tool/wiring code).

---

## File structure

```text
lingxi-code/
├── tools/shell-mobile/                 CREATE: new crate `tool-shell-mobile`
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs                      ShellMobileTool + register_all (gated) + Tool impl
│       └── net_intent.rs               network-intent segment tokenizer + denylist (host-tested)
├── tool-api/src/builtin_context.rs     MODIFY: + `android_shell: Option<AndroidShellToolCtx>` (+ the carrier type)
├── apps/engine-mobile/src/
│   ├── host.rs                         MODIFY: MobileConfig +android_shell; set BuiltinToolContext.android_shell
│   └── lib.rs                          MODIFY: register_mobile_tools → tool_shell_mobile::register_all
├── apps/android-aar/src/lib.rs         MODIFY: build_android_engine computes the gate + prompt info into MobileConfig
├── lingxi-code/Cargo.toml              MODIFY: workspace member + (engine-mobile) dep
└── clients/android/app/src/androidTest/java/com/lingxi/code/
    └── ShellToolTest.kt                CREATE (device): the registered Shell tool runs end-to-end
```

---

### Task 1: `AndroidShellToolCtx` carrier on `BuiltinToolContext`

**Files:** Modify `tool-api/src/builtin_context.rs`.

- [ ] **Step 1: Failing test** — in `builtin_context.rs` tests (or a new test): assert `BuiltinToolContext` has an `android_shell: Option<AndroidShellToolCtx>` field defaulting to `None` in the existing test constructor/helper, and that `AndroidShellToolCtx { enabled, prompt_info, applets }` constructs. (If the crate has no test ctx builder, add a `#[cfg(test)]` one or test the carrier type alone.)

- [ ] **Step 2: Run → FAIL.**

- [ ] **Step 3: Implement** — add the carrier type and field:

```rust
/// Android-only `Shell` tool wiring (spec r3 §Shell tool). `None` on desktop /
/// iOS. Built by `android-aar` from the probed capability cache + the
/// `AndroidShellConfig` gate; consumed by `tool-shell-mobile::register_all`
/// (registration gate) and the tool's prompt.
#[derive(Debug, Clone)]
pub struct AndroidShellToolCtx {
    /// The full registration gate result: capability-probe OK + `enable_shell`
    /// + D11 secrets gate all satisfied. When `false`, the Shell tool is NOT
    /// registered (absent, not erroring).
    pub enabled: bool,
    /// Probed toybox applet inventory (for the tool prompt; may be empty).
    pub applets: Vec<String>,
    /// System sh version string (`KSH_VERSION`) when probed, for the prompt.
    pub sh_version: Option<String>,
}
```

Add `pub android_shell: Option<AndroidShellToolCtx>,` to `BuiltinToolContext` with a doc comment. Update EVERY `BuiltinToolContext { .. }` construction site in the workspace to add `android_shell: None,` (grep: `rg "BuiltinToolContext \{" --type rust -l` — engine-desktop, engine-mobile host, tool-api test_support, any tool tests). This is the bulk of the task; `cargo check --workspace` finds them all.

- [ ] **Step 4: Run** `cargo test -p tool-api && cargo check --workspace` → clean.

- [ ] **Step 5: Commit**

```bash
git add tool-api/src/builtin_context.rs <other touched ctx sites>
git commit -m "feat(tool-api): AndroidShellToolCtx carrier on BuiltinToolContext (P3 seam)"
```

---

### Task 2: network-intent tokenizer (host-tested, pure)

**Files:** Create `tools/shell-mobile/Cargo.toml` + `src/net_intent.rs` (+ stub `src/lib.rs` so the crate builds).

- [ ] **Step 1: Cargo.toml + workspace member.** New crate `tool-shell-mobile`; deps: `tool-api`, `traits`, `permission` (for `PermissionResult`), `serde_json`, `async-trait`, `serde`. Add `"tools/shell-mobile"` to the workspace `members`. Minimal `lib.rs`: `pub mod net_intent;` (the tool itself lands in Task 3).

- [ ] **Step 2: Failing tests** in `net_intent.rs`:

```rust
//! Network-intent advisory (spec r3 §Shell tool, D10): the Shell tool is
//! deny-net, so a command whose intent is networking is refused up-front with
//! a pointer to the structured Git tool rather than executed to a confusing
//! seccomp EPERM. This is UX guidance, not a security boundary — the boundary
//! is the runner's net-deny seccomp filter.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_network_command_heads() {
        for c in [
            "git clone https://x",
            "git fetch origin",
            "git push",
            "curl https://x",
            "wget http://x",
            "nc 10.0.0.1 80",
            "ssh host",
            "scp a b:c",
            "echo hi && git pull",   // any segment counts
            "ls | curl x",
        ] {
            assert!(network_intent(c).is_some(), "{c:?} should be flagged");
        }
    }

    #[test]
    fn allows_local_commands() {
        for c in [
            "echo hi",
            "ls -la",
            "git status",
            "git diff",
            "git commit -m x",      // local git is allowed (P4 git runs deny-net)
            "grep -r foo .",
            "cat file | sed s/a/b/",
        ] {
            assert!(network_intent(c).is_none(), "{c:?} should be allowed");
        }
    }

    #[test]
    fn parse_failure_is_not_a_network_grant() {
        // Unparseable / weird input must NOT silently pass as "local"; the
        // caller treats `None` as "no detected net intent, run deny-net anyway"
        // — which is safe because the runner is deny-net regardless. Document
        // that the advisory is best-effort and the seccomp filter is the real
        // stop. (No assertion beyond: does not panic.)
        let _ = network_intent("$(");
    }
}
```

- [ ] **Step 3: Implement** — segment on shell operators, match each segment head against a network-command set; for `git`, only specific subcommands are network:

```rust
/// Shell command heads that always imply network use.
const NET_HEADS: &[&str] = &["curl", "wget", "nc", "ncat", "ssh", "scp", "sftp", "rsync", "telnet", "ftp"];
/// `git` subcommands that hit the network.
const GIT_NET_SUBCMDS: &[&str] = &["clone", "fetch", "pull", "push", "ls-remote", "remote", "submodule"];

/// If the command shows network intent, return a human advisory string
/// (for the tool error). `None` = no detected intent (still run deny-net).
#[must_use]
pub fn network_intent(command: &str) -> Option<String> {
    for seg in split_segments(command) {
        let mut words = seg.split_whitespace();
        let Some(head) = words.next() else { continue };
        let head = head.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_' && c != '/');
        let base = head.rsplit('/').next().unwrap_or(head); // strip any path
        if NET_HEADS.contains(&base) {
            return Some(format!(
                "`{base}` needs network access, which the Shell tool does not allow. \
                 Use a structured network tool (e.g. Git) for remote operations."
            ));
        }
        if base == "git" {
            if let Some(sub) = words.next() {
                if GIT_NET_SUBCMDS.contains(&sub) {
                    return Some(format!(
                        "`git {sub}` needs network access, which the Shell tool does not allow. \
                         Use the Git tool for remote git operations; local git (status/diff/commit/log) \
                         works here."
                    ));
                }
            }
        }
    }
    None
}

/// Split a command line into top-level segments on `|`, `&&`, `||`, `;`, `&`,
/// and newlines. Best-effort (ignores quoting/`$()` — the runner's seccomp is
/// the real boundary; this is advisory).
fn split_segments(command: &str) -> Vec<String> {
    // implement a simple scan splitting on the operator bytes; collapse `&&`/`||`
    // ...
}
```

Implement `split_segments` (a simple byte scan; `&&`/`||` collapse to one boundary; bare `&`/`;`/`|`/`\n` are boundaries). Keep it small; the tests pin behavior.

- [ ] **Step 4: Run** `cargo test -p tool-shell-mobile net_intent` → PASS.

- [ ] **Step 5: Commit**

```bash
git add tools/shell-mobile lingxi-code/Cargo.toml lingxi-code/Cargo.lock
git commit -m "feat(tool-shell-mobile): network-intent advisory tokenizer (host)"
```

---

### Task 3: `ShellMobileTool` — the Tool impl

**Files:** `tools/shell-mobile/src/lib.rs`.

- [ ] **Step 1: Failing tests** (host — the runner returns enforcement_failed on host, so assert the tool plumbs prepare→run and maps results/advisory correctly):

```rust
    // A ctx with android_shell enabled + a host sandbox/process that lets us
    // observe prepare()+run() being called. Reuse tool-api's test_support ctx
    // builder; set android_shell = Some(enabled:true,...). The mobile
    // AndroidMinijailSandbox/runner aren't host-runnable, so use a fake
    // Sandbox+ProcessRunner (record prepared policy; return a canned output)
    // OR assert against the real ones' host behavior. Simplest: a MockSandbox
    // that records the SandboxPolicy.network and returns a wrapped command, and
    // a MockProcess returning a known ProcessOutput.

    #[tokio::test]
    async fn call_builds_deny_net_policy_and_runs() { /* network == Disabled; stdout passthrough */ }

    #[tokio::test]
    async fn network_intent_command_is_refused_without_running() { /* "git clone x" → ToolError, process.run NOT called */ }

    #[test]
    fn name_is_shell_and_schema_has_command() { /* name()=="Shell"; schema has command:string */ }

    #[tokio::test]
    async fn prompt_declares_mksh_dialect_and_lists_applets() { /* prompt contains "mksh" + an applet from ctx */ }
```

- [ ] **Step 2: Run → FAIL.**

- [ ] **Step 3: Implement** `ShellMobileTool { ctx: BuiltinToolContext }`:
  - `name()` → `"Shell"`; `input_schema()` → `{ command: string (required), timeout?: integer ms, description?: string }`.
  - `is_enabled(_)` → `self.ctx.android_shell.as_ref().map_or(false, |a| a.enabled)` (defensive double-gate; registration already filters).
  - `check_permissions(...)` → mirror the desktop `BashTool` stub: `PermissionResult::Allow { reason: Other{ "android-shell deny-net" }, .. }` (real allow/ask is the engine gate). Cite the recon finding in a comment.
  - `prompt(opts)` → a concise prompt: declares this is a **mksh** shell (no bash process substitution `<(...)`, no `${var,,}`, no `mapfile`/`readarray`), deny-net (no network — use the Git tool for remote ops), workspace-rooted; lists the probed applet inventory from `self.ctx.android_shell.applets` (or "system toybox" when empty) and the sh version when present.
  - `call(input, ...)`:
    1. extract `command` (ValidationError if missing/empty), `timeout` (cap like desktop), `description`.
    2. `if let Some(advice) = net_intent::network_intent(&command) { return Err(ToolError::InvalidInput(advice)); }` — refuse BEFORE building/running.
    3. build `ProcessCommand { command: "/system/bin/sh", args: vec!["-c", command], cwd: Some(ctx.workspace), env: {}, timeout, stdin: None }`.
    4. build the deny-net default `SandboxPolicy { network: Disabled, writable_paths: [], denied_paths: [], allow_subprocess: true, limits: default }`.
    5. `let sandboxed = ctx.sandbox.prepare(pcmd, &policy).map_err(|e| ToolError::...)?;` (prepare can fail closed → surface as a tool error naming the guarantee).
    6. `let out = ctx.process.run(&sandboxed).await.map_err(map_process_err)?;` — map `Timeout` → a timeout message, `SandboxEnforcementFailed`/`PolicyUnsupported`/`MalformedSandboxPlan` → InvalidInput/Internal with the named reason, `Unsupported` → "shell unavailable".
    7. on `Ok(out)`: build a `ToolCallResult` with stdout/stderr (truncate to `max_result_size_chars`), exit code; if `out.timed_out` surface the timeout message. Mirror desktop BashTool's result shape where reasonable.
  - `register_all(reg, ctx)`: register `ShellMobileTool::new(ctx)` ONLY when `ctx.android_shell.as_ref().is_some_and(|a| a.enabled)`; otherwise no-op (absent, not erroring). Doc-comment the gate.

- [ ] **Step 4: Run** `cargo test -p tool-shell-mobile && cargo clippy -p tool-shell-mobile --all-targets -- -D warnings && cargo fmt -p tool-shell-mobile --check && cargo check --workspace`.

- [ ] **Step 5: Commit**

```bash
git add tools/shell-mobile/src/lib.rs
git commit -m "feat(tool-shell-mobile): ShellMobileTool — deny-net prepare+run, mksh prompt, net-intent refusal (D9/D10)"
```

---

### Task 4: register the tool in engine-mobile + thread the gate through MobileConfig

**Files:** `apps/engine-mobile/src/lib.rs`, `apps/engine-mobile/src/host.rs`, `apps/engine-mobile/Cargo.toml` (+ dep on `tool-shell-mobile`), workspace `Cargo.toml`.

- [ ] **Step 1:** Add `tool-shell-mobile` as an engine-mobile dependency.

- [ ] **Step 2:** `MobileConfig` gains `pub android_shell: Option<tool_api::AndroidShellToolCtx>` (default `None` in `Default`). In `host.rs:438`, set `android_shell: cfg.android_shell.clone()` on the `BuiltinToolContext`.

- [ ] **Step 3:** In `register_mobile_tools` add `tool_shell_mobile::register_all(reg, ctx.clone());` among the mobile tools (it self-gates on `ctx.android_shell`). Order: after the cross-platform subset, with the mobile-exclusive tools.

- [ ] **Step 4: Test** — extend the engine-mobile snapshot/registration test (`apps/engine-mobile/tests/mobile_tool_list_snapshot.rs` exists): with `android_shell: None` (default), `Shell` is ABSENT from the registry; with `android_shell: Some(enabled:true,..)`, `Shell` is PRESENT. (Build the registry via `mobile_tool_registry(ctx)` with each ctx.)

- [ ] **Step 5:** `cargo test -p engine-mobile && cargo check --workspace && cargo clippy -p engine-mobile -p tool-shell-mobile --all-targets -- -D warnings`.

- [ ] **Step 6: Commit**

```bash
git add apps/engine-mobile lingxi-code/Cargo.toml lingxi-code/Cargo.lock
git commit -m "feat(engine-mobile): register the mobile Shell tool, gated on android_shell ctx (P3)"
```

---

### Task 5: compute the gate + prompt info in android-aar

**Files:** `apps/android-aar/src/lib.rs` (the `build_android_engine` android branch — it already runs the eager probe from P2/T7 and holds the concrete `AndroidPlatform`).

- [ ] **Step 1:** After the eager probe populates the cache (P2/T7 code) and before `engine_mobile::build_mobile_engine`, compute the gate from the concrete `AndroidShellConfig` (available via the platform inputs — thread it if not already reachable; the `AndroidShellConfig` is in `AndroidPlatformInputs.shell`) + the probed `AndroidSandboxCapabilities`:

```rust
let android_shell = shell_cfg.as_ref().map(|cfg| {
    let caps = cache.get(); // the just-probed capabilities
    let enabled = cfg.enable_shell
        && cfg.secrets_gate_satisfied()
        && caps.available()
        && caps.seccomp_filter
        && caps.net_deny_verified;
    tool_api::AndroidShellToolCtx {
        enabled,
        applets: caps.toybox_applets.clone(),
        sh_version: caps.system_sh_version.clone(),
    }
});
mobile_cfg.android_shell = android_shell;
```

(Confirm the exact bindings: `shell_cfg`/`cache` names from the T7 probe code; `cache.get()` returns the probed `AndroidSandboxCapabilities`.) The host (non-android) branch leaves `android_shell: None`.

- [ ] **Step 2: Test** — host build: the off-device path keeps `android_shell: None` (no Shell tool). If feasible add a small unit asserting the gate formula (enabled iff all five conjuncts) — extract the formula into a `fn android_shell_gate(cfg, caps) -> bool` so it is host-testable without a device.

- [ ] **Step 3:** `cargo test -p android-aar && cargo check --workspace`; cross-build `cargo ndk -t arm64-v8a build -p android-aar`; `cargo ndk -t arm64-v8a clippy -p android-aar -- -D warnings`.

- [ ] **Step 4: Commit**

```bash
git add apps/android-aar/src/lib.rs
git commit -m "feat(android-aar): compute Shell-tool registration gate (enable_shell + D11 + caps) into MobileConfig (P3)"
```

---

### Task 6: P3 gate + on-device Shell-tool acceptance

**Files:** `clients/android/app/src/androidTest/java/com/lingxi/code/ShellToolTest.kt` (optional device gate), workspace fixups.

- [ ] **Step 1: workspace gate** — `cargo fmt --all` (revert drift outside P3 crates as in P1/P2); `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace` (the `cwd_persistence`/`pwsh` macOS/env flakes are known — rerun in isolation / note); `cargo ndk -t arm64-v8a build -p android-aar` + `-t x86_64`.

- [ ] **Step 2 (device, best-effort):** if a UniFFI surface exists to drive the engine, assert the `Shell` tool appears in the registered tool list when the gate is on. Simpler durable proof: a host test (Task 4) already proves registration gating; a device test that the registered tool actually executes a command end-to-end is the acceptance bonus. If the emulator (AVD `p0a`, API-34 arm64) is available, build-jni.sh + a small instrumentation assertion; else PENDING-DEVICE with the runbook. The P2 `android_sandbox_run_probe` already proved the run path on-device, so a P3 device test is incremental, not load-bearing — PENDING-DEVICE is acceptable.

- [ ] **Step 3: Commit** any fixups:

```bash
git commit -m "chore(android-sandbox): P3 gate — workspace + android-target clean"
```

---

## Self-review / spec coverage

- Spec §Shell tool: name `Shell` (D9, T3); schema command/timeout/description (T3); deny-net policy always (T3 call step 4, D10); mksh dialect + applet inventory prompt (T3 prompt); network-intent advisory (T2+T3). ✓
- Spec §Registration gates: `cfg(target_os=android)` (android-aar branch) + capability cache available + `enable_shell` + D11 secrets gate (T5 gate formula) → tool absent when unmet (T3 register_all + T4 ctx field). ✓
- D4 permission model: engine `AdapterPermissionGate` (already wired) handles allow/ask; tool `check_permissions` mirrors the desktop stub (T3, with the recon citation). ✓
- Fail-closed: prepare()/run() errors surface as named tool errors, never silent success (T3 call steps 5-6). ✓
- Deferred to P4+ (noted): structured Git tool + bundled helpers + `ExecTarget::BundledHelper` PATH; stdin wiring; bundled mksh/toybox (P5); third_party/minijail vendoring (infra debt — flag at merge); API-29 + physical-device runs.

## Risks specific to P3
- **net-intent tokenizer is best-effort** (no real shell parse): a command that hides networking (`$(echo cur)l`, eval tricks) won't be flagged — but the runner's net-deny seccomp filter is the actual boundary, so the worst case is a confusing EPERM instead of a clean advisory. Documented in `net_intent.rs`.
- **BuiltinToolContext field churn**: adding `android_shell` touches every construction site; `cargo check --workspace` is the safety net — don't miss one (engine-desktop, test_support, tool tests).
- **Gate computed once at build**: the capability probe runs once at engine construction; if device state changed (it can't mid-session) the gate is stale — acceptable per the cache-key design (packageVersionCode + path).
