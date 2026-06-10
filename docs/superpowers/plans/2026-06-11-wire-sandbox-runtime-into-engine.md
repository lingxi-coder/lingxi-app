# Wiring sandbox-runtime into the engine sandbox path — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development.

**Goal:** Route the engine's bash/powershell/skill command sandboxing through the faithful `sandbox-runtime` crate via an injected async `SandboxRunner`, lazy-initialized per session, with the existing sync wrap kept as the default (opt-in live runner). Spec: `docs/superpowers/specs/2026-06-11-wire-sandbox-runtime-into-engine-design.md`.

**Branch:** `wire-sandbox-runtime-engine`. **Conventions:** Cargo root `lingxi-code/`; git from repo root, `lingxi-code/...` paths, **NEVER `git add -A`** (untracked codex/liter-llm/opencode dirs at repo root). `-D missing-docs` + clippy pedantic. `traits/` + `protocol/` are FROZEN — do NOT touch (verify empty diff). Footer `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`. **engine-mobile MUST stay 0-dep on sandbox-runtime AND sandbox-runtime-runner.**

**Key verified facts:** `sandbox_runtime::SandboxManager::initialize(&mut self, cfg, ask, log_monitor)` is **async**; `wrap_with_sandbox(&self, command, bin_shell: Option<&str>, custom_config, cwd: &str) -> Result<(String, Vec<PathBuf>), ManagerError>` is **sync**; `update_config(&mut self, cfg)` + `reset(&mut self)` sync. Engine config = `sandbox::runtime_config::SandboxRuntimeConfig` {network: NetworkRestrictionConfig{allowed_domains, allow_unix_sockets, allow_all_unix_sockets, allow_local_binding, http_proxy_port, socks_proxy_port, ...}, filesystem: FilesystemRestrictionConfig{allow_write, deny_write, deny_read, allow_read, ...}, ripgrep, enable_weaker_nested_sandbox, ...}. Call sites: `tools/shell/src/bash.rs:570`, `tools/shell/src/powershell.rs:219`, `tools/skill/src/skill.rs:292`. Context literals: `tool-api/src/test_support.rs` (2), `apps/engine-desktop/src/lib.rs:1454` + `:2371`, `apps/engine-mobile/src/host.rs:357`, `tools/agent/src/agent.rs:433`, `tools/speech/src/lib.rs:253`, + per-tool test `ctx_with`/`bctx` helpers.

---

### Task 1: `SandboxRunner` trait + `LegacyWrapRunner` + context field

**Files:** Create `tool-api/src/sandbox_runner.rs`; Modify `tool-api/src/builtin_context.rs`, `tool-api/src/lib.rs`; update ALL `BuiltinToolContext { ... }` literal sites.

- [ ] **`SandboxRunner` trait** (`#[async_trait]`, in tool-api): `async fn wrap(&self, command: &str, cfg: &sandbox::runtime_config::SandboxRuntimeConfig, platform: sandbox::runtime_config::Platform, bin_shell: Option<&str>, cwd: Option<&std::path::Path>) -> Result<String, sandbox::wrap::SandboxWrapError>`; `async fn cleanup_after_command(&self) {}` (default no-op); `async fn reset(&self) {}` (default no-op).
- [ ] **`LegacyWrapRunner`** (unit struct in tool-api): `wrap` calls `sandbox::wrap::wrap_with_sandbox(command, cfg, platform)` (the existing sync fn — ignores bin_shell/cwd, faithful to today). cleanup/reset are the no-op defaults. `pub fn default_sandbox_runner() -> std::sync::Arc<dyn SandboxRunner>` → `Arc::new(LegacyWrapRunner)`.
- [ ] **`BuiltinToolContext`**: add `pub sandbox_runner: std::sync::Arc<dyn SandboxRunner>`. Update EVERY construction literal to add `sandbox_runner: default_sandbox_runner(),` (test_support, engine-desktop ×2, engine-mobile, agent, speech, and each tool's test `ctx_with`/`bctx`). Re-export `SandboxRunner`/`LegacyWrapRunner`/`default_sandbox_runner` from tool-api's lib.
- [ ] **Tests:** `LegacyWrapRunner::wrap` returns the same string as `sandbox::wrap::wrap_with_sandbox` for a sample config (parity); a fake `SandboxRunner` records calls. Gate (`cargo test -p tool-api` + clippy) + commit (`feat(tool-api): SandboxRunner trait + LegacyWrapRunner + context field`).

### Task 2: `sandbox-runtime-runner` crate (config conversion + live runner)

**Files:** Create `sandbox-runtime-runner/{Cargo.toml, src/lib.rs, src/convert.rs}`; add to workspace members (NOT default-members if it shouldn't build on mobile targets — but it's desktop; add to members + default-members is fine since it builds on macOS/Linux). Deps: `sandbox-runtime`, `sandbox`, `tool-api`, `async-trait`, `tokio`.

- [ ] **`convert.rs`** — `pub fn to_runtime_config(engine: &sandbox::runtime_config::SandboxRuntimeConfig) -> sandbox_runtime::SandboxRuntimeConfig`: map `network.allowed_domains`→`network.allowed_domains`; engine has no `denied_domains` on NetworkRestrictionConfig (verify — if absent, `denied_domains: vec![]`); `allow_unix_sockets`/`allow_all_unix_sockets`/`allow_local_binding`/`http_proxy_port`/`socks_proxy_port` → the matching `NetworkConfig` fields; `filesystem.{allow_write→allow_write, deny_write→deny_write, deny_read→deny_read, allow_read→allow_read}` → `FilesystemConfig`; `ripgrep`→`ripgrep`; `enable_weaker_nested_sandbox`→`Some(..)`; `enable_weaker_network_isolation`→`Some(..)`; `bwrap_path`/`socat_path` if present; `mandatory_deny_search_depth` if present. Unit-test the mapping (domains, fs paths, ports, empty/defaults).
- [ ] **`SandboxRuntimeRunner`** (impl `tool_api::SandboxRunner`): `tokio::sync::Mutex<RunnerState{ manager: Option<sandbox_runtime::SandboxManager>, active_key: Option<u64>, mount_points: Vec<PathBuf> }>`.
  - `wrap`: lock; `let rt = to_runtime_config(cfg)`; `let key = hash(&rt)`; if `manager` is None → `let mut m = SandboxManager::new(); m.initialize(rt, None, false).await.map_err(map_err)?; state.manager = Some(m); state.active_key = Some(key)`; else if `key != active_key` → if only allow/deny domains differ, `m.update_config(rt)` (live); else `m.reset(); m.initialize(rt, None, false).await?`; set active_key. Then `let (wrapped, mounts) = m.wrap_with_sandbox(command, bin_shell, None, cwd.unwrap_or(".").to_str()...).map_err(map_err)?; state.mount_points = mounts; Ok(wrapped)`. (Distinguishing structural-vs-domain change can be simplified: always re-init unless ONLY the network domain vectors differ; keep it correct + documented.)
  - `cleanup_after_command`: lock; `sandbox_runtime::linux::cleanup_bwrap_mount_points(&state.mount_points)`; clear.
  - `reset`: lock; `if let Some(m) = &mut state.manager { m.reset(); }` (reset is sync); `state.manager = None`.
  - `map_err(ManagerError) -> SandboxWrapError::Unsupported(msg)` (so a broken host degrades like today).
- [ ] **Tests:** convert mapping (Task above); runner `wrap` on macOS → returns a sandbox-exec string (host-runnable; initialize starts proxies on localhost ports) for `allowedDomains=["github.com"]`; a 2nd `wrap` reuses (manager not re-created — assert via a call counter or that the port is stable); `reset` tears down. Linux/bridge bits Docker-gated or skipped on macOS. Gate (`cargo test -p sandbox-runtime-runner` + clippy) + commit (`feat(sandbox-runtime-runner): live SandboxRunner backed by SandboxManager + config conversion`).

### Task 3: wire the tool call sites

**Files:** Modify `tools/shell/src/bash.rs`, `tools/shell/src/powershell.rs`, `tools/skill/src/skill.rs`.

- [ ] Replace `match wrap_with_sandbox(&spawn_cmd, &self.ctx.sandbox_runtime, self.ctx.platform) { ... }` with `match self.ctx.sandbox_runner.wrap(&spawn_cmd, &self.ctx.sandbox_runtime, self.ctx.platform, Some(&shell), cwd_opt).await { Ok(wrapped) => ..., Err(SandboxWrapError::Unsupported(s)) => ... (same error handling) }`. `cwd_opt` = the command's working dir (`Some(&self.ctx.workspace)` or the per-command cwd — use what the tool already has). After the wrapped command finishes spawning/executing, call `self.ctx.sandbox_runner.cleanup_after_command().await` (find the post-exec point; for skill/powershell mirror bash).
- [ ] Keep the `SandboxDecision::NoSandbox` branch unchanged; only the `Sandbox { .. }` branch changes. Keep `should_use_sandbox` + `allow_unsandboxed_commands` logic intact.
- [ ] Update the tools' tests that asserted on `wrap_with_sandbox` to go through an injected runner (or keep the default legacy runner so output is unchanged — the legacy path is byte-identical). Gate (`cargo test -p tool-shell -p tool-skill` + clippy) + commit (`feat(tools): route bash/powershell/skill wrap through ctx.sandbox_runner`).

### Task 4: inject the live runner in engine-desktop

**Files:** Modify `apps/engine-desktop/src/lib.rs` (the `:1454` BuiltinToolContext build); `apps/engine-desktop/Cargo.toml` (+`sandbox-runtime-runner`).

- [ ] At the desktop context build, set `sandbox_runner: std::sync::Arc::new(sandbox_runtime_runner::SandboxRuntimeRunner::new())` instead of the default. Call `reset()` on the runner at session teardown (find where the context/session drops; if there's no clean hook, rely on `SandboxManager`'s `Drop`/`LinuxBridge` Drop teardown + document). engine-mobile (`host.rs:357`) and the desktop test stub (`:2371`) keep `default_sandbox_runner()`.
- [ ] Gate + commit (`feat(engine-desktop): inject live SandboxRuntimeRunner into the tool context`).

### Task 5: final gates

```bash
cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-code
cargo test -p tool-api -p sandbox-runtime-runner -p tool-shell -p tool-skill
cargo clippy -p tool-api -p sandbox-runtime-runner -p tool-shell -p tool-skill --all-targets --no-deps -- -D warnings
cargo test --workspace --no-run
cargo build -p engine-desktop
cargo build -p engine-mobile
cargo tree -p engine-mobile -e normal | grep -cE "sandbox-runtime($|[^-])"   # 0 (sandbox-runtime)
cargo tree -p engine-mobile -e normal | grep -c "sandbox-runtime-runner"      # 0
```
Frozen check: `git diff main -- lingxi-code/traits lingxi-code/protocol` empty.

## Final verification
1. `SandboxRunner` seam: bash/powershell/skill wrap through `ctx.sandbox_runner`; LegacyWrapRunner default = byte-identical current behavior; injected live runner = sandbox-runtime stack.
2. `SandboxRuntimeRunner`: lazy session manager (proxy/bridge/MITM), config conversion faithful, cleanup + reset; macOS wrap host-runnable.
3. engine-mobile pulls 0 `sandbox-runtime` AND 0 `sandbox-runtime-runner`; engine-desktop builds + injects the live runner; frozen surfaces untouched.
4. The engine's sandboxed bash commands now use the faithful proxy domain-filter + MITM + seccomp stack (the deferred socat companion is now live).
