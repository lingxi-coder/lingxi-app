# Wiring `sandbox-runtime` into the engine sandbox path — design

**Date:** 2026-06-11 · **Status:** approved (scope + lifecycle decisions made via AskUserQuestion)

## Goal

Route the engine's real command-sandboxing path — the bash / powershell / skill tools, which today call the synchronous `sandbox::wrap_with_sandbox(cmd, &SandboxRuntimeConfig, platform)` — through the faithful, security-reviewed `sandbox-runtime` crate (the complete 1:1 port of `@anthropic-ai/sandbox-runtime`). This replaces today's simpler wrap (which **defers the socat domain-filter companion**, so a domain-allowlist policy gets no external egress) with the full live stack: the host HTTP/SOCKS forward proxy + socat netns bridge + optional TLS-MITM + the exact bwrap argv (Linux) / SBPL profile (macOS) + the AF_UNIX seccomp block.

## Decisions (user-approved)

1. **Scope:** Full live stack (running proxy + bridge + MITM), not wrap-only.
2. **Wiring point:** the desktop path uses `sandbox-runtime` directly (via an injected runner); no new standalone adapter crate fronting it.
3. **Lifecycle:** the session `SandboxManager` initializes **lazily on the first wrap**, then is reused for the session; `reset()` on teardown.
4. **Default/fallback:** a `LegacyWrapRunner` (today's sync `sandbox::wrap_with_sandbox`) remains the default `Arc<dyn SandboxRunner>`; the live runner is **opt-in via injection** (mobile, tests, and hosts without bwrap/socat keep current behavior — no regressions).

## Why this shape

- The wrap call sites (`tools/shell/src/bash.rs:570`, `tools/shell/src/powershell.rs:219`, `tools/skill/src/skill.rs:292`) are already inside `async fn`s → the wrap can become `async` with no `block_on` bridge.
- `tool-shell` and the runner crate do **not** reach `engine-mobile` (verified: `cargo tree -p engine-mobile` shows 0 edges to `tool-shell`), so depending on `sandbox-runtime` there keeps mobile 0-dep.
- `traits/` (frozen) is untouched — the new `SandboxRunner` trait is additive in `tool-api`.
- The existing coarse `traits::sandbox::Sandbox`/`NetworkPolicy` path (`PosixSandbox::prepare`) is a *separate*, generic process-spawn mechanism; it is NOT the bash path and is left as-is (a possible later unification, out of scope here).

## Architecture

### 1. The seam — `SandboxRunner` trait (additive, in `tool-api`)

```rust
#[async_trait]
pub trait SandboxRunner: Send + Sync {
    /// Wrap a command for sandboxed execution. Returns the spawn-ready shell
    /// string (bwrap/sandbox-exec invocation) referencing any live proxy
    /// sockets/ports. `cwd` drives fs glob-expansion + the mandatory-deny scan.
    async fn wrap(
        &self,
        command: &str,
        cfg: &sandbox::runtime_config::SandboxRuntimeConfig,
        platform: sandbox::runtime_config::Platform,
        bin_shell: Option<&str>,
        cwd: Option<&std::path::Path>,
    ) -> Result<String, sandbox::wrap::SandboxWrapError>;

    /// Called by the tool AFTER the wrapped command exits (mount-point cleanup,
    /// scrub). No-op for the legacy runner.
    async fn cleanup_after_command(&self) {}

    /// Tear down session resources (proxies/bridge/CA). Called on session end.
    async fn reset(&self) {}
}
```

`BuiltinToolContext` gains `pub sandbox_runner: Arc<dyn SandboxRunner>` (defaulted to `LegacyWrapRunner`). The three tools replace `wrap_with_sandbox(&spawn_cmd, &self.ctx.sandbox_runtime, self.ctx.platform)` with
`self.ctx.sandbox_runner.wrap(&spawn_cmd, &self.ctx.sandbox_runtime, self.ctx.platform, Some(&shell), cwd).await` and call `cleanup_after_command()` after the spawn completes.

### 2. `LegacyWrapRunner` (default, `tool-api` or `sandbox`)

Thin wrapper over the existing sync `sandbox::wrap_with_sandbox` — identical current behavior; `cleanup_after_command`/`reset` are no-ops. This is the default so nothing changes unless the live runner is injected.

### 3. `SandboxRuntimeRunner` (new desktop-only crate `sandbox-runtime-runner`)

Depends on `sandbox-runtime` + `sandbox` (for the engine config types). Owns the session manager:

```rust
pub struct SandboxRuntimeRunner {
    state: tokio::sync::Mutex<RunnerState>,
}
struct RunnerState {
    manager: Option<sandbox_runtime::SandboxManager>,
    active_config_key: Option<ConfigKey>,  // detects structural changes
    mount_points: Vec<std::path::PathBuf>,  // from the last wrap, for cleanup
}
```

- **`wrap`:** lock state; convert the engine `SandboxRuntimeConfig` → `sandbox_runtime::SandboxRuntimeConfig` (Task: the field mapping). If `manager` is `None` → `SandboxManager::initialize(rt_cfg, ask=None, log_monitor=false).await` (starts proxies + bridge + optional MITM CA). If the config's **structural** parts changed (ports/MITM/fs) → `reset()` + re-init; if only allow/deny **domains** changed → `manager.update_config(...)` (live). Then `let (wrapped, mounts) = manager.wrap_with_sandbox(command, bin_shell, custom_config=None, cwd).await?;` store `mounts`, return `wrapped`.
- **`cleanup_after_command`:** `cleanup_bwrap_mount_points(&mounts)` + clear.
- **`reset`:** `manager.reset().await`, drop it.

### 4. Config conversion (`sandbox::SandboxRuntimeConfig` → `sandbox_runtime::SandboxRuntimeConfig`)

Faithful field mapping (a dedicated function + unit tests):
- `network.allowed_domains` → `network.allowedDomains`; `denied_domains` → `deniedDomains`; carry `httpProxyPort`/`socksProxyPort` if the engine sets external ports; `tlsTerminate`/`parentProxy`/unix-socket flags if present (else `None`).
- `filesystem.allow_write`/`deny_write`/`deny_read`/`allow_read` → the matching fields; `allow_git_config`.
- `ripgrep` → `ripgrep`; `mandatory_deny_search_depth`; `bwrap_path`/`socat_path`; `enable_weaker_nested_sandbox`.
- `allow_unsandboxed_commands` is consumed by the tool's `should_use_sandbox` decision (NOT a sandbox-runtime concept) — not mapped.

### 5. Engine wiring

The desktop engine/platform constructs `Arc::new(SandboxRuntimeRunner::new())` and injects it into `BuiltinToolContext.sandbox_runner` at session/context build time (where `sandbox_runtime`, `platform`, `sandbox` etc. are already populated). `reset()` is called on session teardown (wherever the context/session is dropped). Mobile/test contexts keep the `LegacyWrapRunner` default.

## Error handling

- `SandboxManager::initialize` failure (deps missing, proxy bind fail) → map to `SandboxWrapError::Unsupported` (the tool already handles this → `sandbox_refused` + `ToolError::InvalidInput`), so a broken sandbox host degrades exactly like today.
- The runner never panics the tool: lock poisoning / proxy errors map to `SandboxWrapError`.

## Testing

- `LegacyWrapRunner`: behavior identical to `wrap_with_sandbox` (a parity test).
- Config conversion: unit tests (each field, empty/defaults, domains).
- `SandboxRuntimeRunner` (Linux/Docker-gated where the bridge runs; pure parts portable): first wrap initializes + returns a bwrap string with the proxy sockets; second wrap reuses (no re-init); a domain change live-updates; `cleanup_after_command` removes the mount points; `reset` tears down. macOS: wrap returns a sandbox-exec string (host-runnable).
- The three tool call sites: an injected fake `SandboxRunner` records the wrap call (unit) + the real runner in an integration test.
- Gates: `cargo test` on touched crates; clippy `-D warnings`; `cargo test --workspace --no-run`; **`engine-mobile` pulls 0 `sandbox-runtime` AND 0 `sandbox-runtime-runner`**; frozen `traits`/`protocol` empty.

## Out of scope (tracked follow-ups)

- Unifying the coarse `traits::sandbox::Sandbox`/`PosixSandbox::prepare` path with this rich path.
- Surfacing `sandbox-runtime`'s violation store / ask-callback into the engine UI (the runner passes `ask=None` for now; domain prompts stay deny-on-unmatched).
- Windows live wiring (the engine bash path is posix/macOS; Windows uses `wrap_with_sandbox`'s windows branch — left on the legacy runner).
