# --dangerously-skip-permissions + bypassPermissions Mode + Startup Notice — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Port claude-code's `--dangerously-skip-permissions` CLI flag, the `bypassPermissions` permission mode, and the permission-mode startup notice to the Rust port, with the full interactive TTY bypass-confirmation dialog.

**Architecture:** A pure mode resolver + an injectable-env safety-guard live in the `permission` crate (no new deps); the CLI adds the two flags and a real `BypassEnv` impl (libc geteuid + `/.dockerenv` + a 1s HTTP HEAD probe via the existing `PosixHttp` transport), resolves the mode pre-REPL, runs the guards (exit 1 on refusal), prints the notice, and threads the mode into `DesktopConfig.permission_mode`; engine-desktop feeds that mode into the policy when enforcement is on; the TUI gains a TTY-only blocking bypass-confirm dialog.

**Tech Stack:** Rust 1.82 / edition 2021, clap-derive (argv), `platform_api::HttpTransport` + `platform-posix-minimal::PosixHttp` (internet probe), `libc` (geteuid), the existing `permission::PermissionPolicy`/`PermissionMode` substrate, telemetry tengu registry.

**Spec:** `docs/superpowers/specs/2026-06-10-dangerously-skip-permissions-design.md` (approved). Reference of truth: `claude-code/` TS — `utils/permissions/permissionSetup.ts:689-812`, `setup.ts:395-443`, `interactiveHelpers.tsx:218-223`, `components/BypassPermissionsModeDialog.tsx`, `types/permissions.ts:16-39`, `utils/permissions/PermissionMode.ts:117-121`.

**Branch:** `parity-skip-permissions` (already created off `main`, spec committed).

**Conventions that bite (read first):**
- Cargo workspace root is `lingxi-code/`, NOT the repo root. Run cargo there; run git from the repo root with `lingxi-code/...`-prefixed paths (running `git add` after `cd lingxi-code` produces a `lingxi-code/lingxi-code/...` pathspec error).
- Workspace enforces rustc `-D missing-docs` (every `pub` item needs a doc comment) + clippy `-D warnings` on `--all-targets` (pedantic on).
- Commit with `git commit -F <file>` (zsh traps backticks/angle brackets). Footer EXACTLY:
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
- NEVER run `cargo test --workspace` (runtime) — fs_watch flake. Use per-crate tests + `cargo test --workspace --no-run`.
- Tests mutating process env (`USER_TYPE`, `IS_SANDBOX`, `CLAUDE_CODE_BUBBLEWRAP`, …) must hold a shared `Mutex` guard — the pure resolver/guard are designed to AVOID env reads (everything is injected) precisely so tests stay env-free; only the CLI's real-impl tests (if any) touch env.

---

## File map

| File | Action | Responsibility |
|---|---|---|
| `lingxi-code/permission/src/cli_mode.rs` | Create | `CliModeSettings`, `initial_permission_mode_from_cli`, `permission_mode_from_cli_string` (pure) |
| `lingxi-code/permission/src/bypass_guard.rs` | Create | `BypassEnv` trait + `enforce_bypass_safety` (pure logic over injected probes) |
| `lingxi-code/permission/src/lib.rs` | Modify | `pub mod cli_mode; pub mod bypass_guard;` + re-exports |
| `lingxi-code/apps/cli/Cargo.toml` | Modify | promote `libc` to a `cfg(unix)` dependency (was dev-only) |
| `lingxi-code/apps/cli/src/argv.rs` | Modify | `--dangerously-skip-permissions`, `--permission-mode <mode>` |
| `lingxi-code/apps/cli/src/bypass_env.rs` | Create | `RealBypassEnv` (geteuid / `/.dockerenv` / env / HTTP HEAD probe) |
| `lingxi-code/apps/cli/src/lib.rs` | Modify | pre-REPL: resolve mode → guards → notice; pass mode to `build_runtime` |
| `lingxi-code/apps/cli/src/init.rs` | Modify | `resolve_desktop_config` takes the resolved mode → `DesktopConfig.permission_mode` |
| `lingxi-code/apps/engine-desktop/src/lib.rs` | Modify | `DesktopConfig.permission_mode` field; `BuiltinToolContext.permission_mode = cfg.permission_mode`; policy mode override |
| `lingxi-code/telemetry/src/tengu/permission.rs` | Create | `tengu_bypass_permissions_mode_dialog_accept` name |
| `lingxi-code/telemetry/src/tengu/mod.rs` | Modify | register block, TOTAL 348→349 |
| `lingxi-code/test-harness/src/parity/fixtures/tengu_events.json` | Modify | append the name (order-locked tail) |
| count-assert sites (telemetry tests, orchestrator diagnostics, tui vim + behavior_palette) | Modify | 348→349 |
| `lingxi-code/tui/src/startup_bypass.rs` | Create | pure `BypassDialogState` + `handle_key` + render + `should_show_bypass_dialog` |
| `lingxi-code/tui/src/lib.rs` | Modify | `pub mod startup_bypass;` |
| `lingxi-code/apps/cli/src/mode.rs` | Modify | mount the bypass dialog before `run_tui_session` on the TUI path |

---

### Task 1: pure mode resolver (`permission/src/cli_mode.rs`)

**Files:**
- Create: `lingxi-code/permission/src/cli_mode.rs`
- Modify: `lingxi-code/permission/src/lib.rs`

- [ ] **Step 1: Declare the module.** In `permission/src/lib.rs`, add `pub mod cli_mode;` near the other `pub mod` lines (after `pub mod classifier;` is fine), and after the existing re-export block add:

```rust
pub use cli_mode::{initial_permission_mode_from_cli, permission_mode_from_cli_string, CliModeSettings};
```

- [ ] **Step 2: Failing tests** — bottom of `cli_mode.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::mode::PermissionMode;

    fn no_settings() -> CliModeSettings {
        CliModeSettings { default_mode: None, bypass_disabled: false }
    }

    #[test]
    fn from_string_accepts_the_five_external_modes() {
        assert_eq!(permission_mode_from_cli_string("default"), PermissionMode::Default);
        assert_eq!(permission_mode_from_cli_string("plan"), PermissionMode::Plan);
        assert_eq!(permission_mode_from_cli_string("acceptEdits"), PermissionMode::AcceptEdits);
        assert_eq!(permission_mode_from_cli_string("bypassPermissions"), PermissionMode::BypassPermissions);
        assert_eq!(permission_mode_from_cli_string("dontAsk"), PermissionMode::DontAsk);
    }

    #[test]
    fn from_string_unknown_falls_to_default() {
        // TS permissionModeFromString: not in PERMISSION_MODES → 'default'.
        // 'auto' is ant-only (TRANSCRIPT_CLASSIFIER) → not external here → default.
        assert_eq!(permission_mode_from_cli_string("auto"), PermissionMode::Default);
        assert_eq!(permission_mode_from_cli_string("bubble"), PermissionMode::Default);
        assert_eq!(permission_mode_from_cli_string("garbage"), PermissionMode::Default);
    }

    #[test]
    fn dangerously_skip_wins_and_yields_bypass() {
        let (mode, notice) = initial_permission_mode_from_cli(None, true, &no_settings());
        assert_eq!(mode, PermissionMode::BypassPermissions);
        assert!(notice.is_none());
    }

    #[test]
    fn cli_flag_used_when_no_skip() {
        let (mode, _) = initial_permission_mode_from_cli(Some("plan"), false, &no_settings());
        assert_eq!(mode, PermissionMode::Plan);
    }

    #[test]
    fn skip_outranks_cli_flag() {
        // ordered_modes pushes bypass first, then the cli mode; first valid wins.
        let (mode, _) = initial_permission_mode_from_cli(Some("plan"), true, &no_settings());
        assert_eq!(mode, PermissionMode::BypassPermissions);
    }

    #[test]
    fn settings_default_mode_used_as_lowest_priority() {
        let s = CliModeSettings { default_mode: Some(PermissionMode::AcceptEdits), bypass_disabled: false };
        let (mode, _) = initial_permission_mode_from_cli(None, false, &s);
        assert_eq!(mode, PermissionMode::AcceptEdits);
    }

    #[test]
    fn killswitch_skips_bypass_and_sets_notice() {
        let s = CliModeSettings { default_mode: None, bypass_disabled: true };
        let (mode, notice) = initial_permission_mode_from_cli(None, true, &s);
        assert_eq!(mode, PermissionMode::Default);
        assert_eq!(notice.as_deref(), Some("Bypass permissions mode was disabled by settings"));
    }

    #[test]
    fn killswitch_falls_through_to_next_valid_mode() {
        // skip → bypass (disabled, skipped+notice) then cli plan is valid → plan,
        // but notice is carried (TS keeps `notification` across the loop).
        let s = CliModeSettings { default_mode: None, bypass_disabled: true };
        let (mode, notice) = initial_permission_mode_from_cli(Some("plan"), true, &s);
        assert_eq!(mode, PermissionMode::Plan);
        assert_eq!(notice.as_deref(), Some("Bypass permissions mode was disabled by settings"));
    }

    #[test]
    fn no_inputs_is_default_no_notice() {
        let (mode, notice) = initial_permission_mode_from_cli(None, false, &no_settings());
        assert_eq!(mode, PermissionMode::Default);
        assert!(notice.is_none());
    }
}
```

- [ ] **Step 3: Run to verify failure.** `cd lingxi-code && cargo test -p permission cli_mode` → FAIL (undefined).

- [ ] **Step 4: Implement** — top of `cli_mode.rs`:

```rust
//! CLI permission-mode resolution — pure port of
//! `initialPermissionModeFromCLI` (claude-code `utils/permissions/
//! permissionSetup.ts:689-812`) and `permissionModeFromString`
//! (`PermissionMode.ts:117-121`).
//!
//! Pure by construction: all inputs (parsed flags + a settings view) are
//! passed in, so the priority logic is exhaustively testable without env or
//! IO. The caller (the CLI) reads the merged settings and process env.
//!
//! Documented omissions vs TS (ant-only / no-substrate in the external build):
//! - Statsig `tengu_disable_bypass_permissions_mode` gate (and its
//!   `"…disabled by your organization policy"` notice) — no Statsig substrate;
//!   only the settings-disable notice is reachable.
//! - `auto` / `TRANSCRIPT_CLASSIFIER` mode — classifier is ant-only; an
//!   `"auto"` CLI value falls to `Default` exactly like TS in a non-ant build.
//! - `CLAUDE_CODE_REMOTE` filtering of settings `defaultMode` (CCR) — LingXi
//!   has no CCR remote entrypoint; the `tengu_ccr_unsupported_default_mode_ignored`
//!   event is not reproduced. The caller passes `default_mode` straight through.

use crate::mode::PermissionMode;

/// The settings inputs the resolver reads. The caller extracts these from the
/// merged raw settings (e.g. via `default_mode_from_settings_json` +
/// `bypass_permissions_disabled_from_settings_json`), keeping this fn pure.
#[derive(Debug, Clone)]
pub struct CliModeSettings {
    /// `settings.permissions.defaultMode`, already validated to an external
    /// mode (else `None`).
    pub default_mode: Option<PermissionMode>,
    /// `settings.permissions.disableBypassPermissionsMode === "disable"` — the
    /// bypass-permissions killswitch.
    pub bypass_disabled: bool,
}

/// `permissionModeFromString` (`PermissionMode.ts:117-121`): the valid set is
/// the five external modes; anything else (incl. ant-only `auto`/`bubble`) →
/// `Default`.
#[must_use]
pub fn permission_mode_from_cli_string(s: &str) -> PermissionMode {
    let mode = match s {
        "default" => PermissionMode::Default,
        "plan" => PermissionMode::Plan,
        "acceptEdits" => PermissionMode::AcceptEdits,
        "bypassPermissions" => PermissionMode::BypassPermissions,
        "dontAsk" => PermissionMode::DontAsk,
        _ => return PermissionMode::Default,
    };
    debug_assert!(mode.is_external());
    mode
}

/// `initialPermissionModeFromCLI` (`permissionSetup.ts:689-812`): resolve the
/// session permission mode from CLI flags + settings, returning the mode plus
/// an optional user-facing notice (set when the bypass killswitch suppresses a
/// requested bypass).
#[must_use]
pub fn initial_permission_mode_from_cli(
    permission_mode_cli: Option<&str>,
    dangerously_skip: bool,
    settings: &CliModeSettings,
) -> (PermissionMode, Option<String>) {
    // Modes in order of priority (TS `orderedModes`).
    let mut ordered: Vec<PermissionMode> = Vec::new();
    if dangerously_skip {
        ordered.push(PermissionMode::BypassPermissions);
    }
    if let Some(cli) = permission_mode_cli {
        ordered.push(permission_mode_from_cli_string(cli));
    }
    if let Some(default_mode) = settings.default_mode {
        ordered.push(default_mode);
    }

    let mut notification: Option<String> = None;
    for mode in ordered {
        if mode == PermissionMode::BypassPermissions && settings.bypass_disabled {
            // TS: skip this mode, carry the notice forward.
            notification = Some("Bypass permissions mode was disabled by settings".to_string());
            continue;
        }
        return (mode, notification);
    }
    (PermissionMode::Default, notification)
}
```

- [ ] **Step 5: Run → PASS. Gate + commit.**

```bash
cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-code
cargo test -p permission cli_mode
cargo clippy -p permission --all-targets --no-deps -- -D warnings
cd /Users/luolingfeng/Projects/LingXi-Next
git add lingxi-code/permission/src/cli_mode.rs lingxi-code/permission/src/lib.rs
git commit -F /tmp/msg.txt   # "feat(permission): initial_permission_mode_from_cli resolver (pure)" + footer
```

---

### Task 2: safety guard (`permission/src/bypass_guard.rs`)

**Files:**
- Create: `lingxi-code/permission/src/bypass_guard.rs`
- Modify: `lingxi-code/permission/src/lib.rs`

- [ ] **Step 1: Declare + re-export.** In `permission/src/lib.rs`: `pub mod bypass_guard;` and `pub use bypass_guard::{enforce_bypass_safety, BypassEnv};`.

- [ ] **Step 2: Failing tests** — bottom of `bypass_guard.rs` (a fake `BypassEnv` drives the matrix; no process env):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct FakeEnv {
        windows: bool,
        euid: u32,
        docker: bool,
        internet: bool,
        vars: HashMap<String, String>,
    }
    impl Default for FakeEnv {
        fn default() -> Self {
            Self { windows: false, euid: 1000, docker: false, internet: false, vars: HashMap::new() }
        }
    }
    #[async_trait::async_trait]
    impl BypassEnv for FakeEnv {
        fn is_windows(&self) -> bool { self.windows }
        fn effective_uid(&self) -> u32 { self.euid }
        fn env(&self, key: &str) -> Option<String> { self.vars.get(key).cloned() }
        fn is_docker(&self) -> bool { self.docker }
        async fn has_internet(&self) -> bool { self.internet }
    }

    #[tokio::test]
    async fn non_root_passes() {
        let e = FakeEnv::default();
        assert!(enforce_bypass_safety(&e).await.is_ok());
    }

    #[tokio::test]
    async fn root_without_sandbox_is_refused() {
        let e = FakeEnv { euid: 0, ..FakeEnv::default() };
        let err = enforce_bypass_safety(&e).await.unwrap_err();
        assert_eq!(err, "--dangerously-skip-permissions cannot be used with root/sudo privileges for security reasons");
    }

    #[tokio::test]
    async fn root_with_is_sandbox_passes_check_one() {
        let mut e = FakeEnv { euid: 0, ..FakeEnv::default() };
        e.vars.insert("IS_SANDBOX".into(), "1".into());
        assert!(enforce_bypass_safety(&e).await.is_ok());
    }

    #[tokio::test]
    async fn root_with_bubblewrap_passes_check_one() {
        let mut e = FakeEnv { euid: 0, ..FakeEnv::default() };
        e.vars.insert("CLAUDE_CODE_BUBBLEWRAP".into(), "1".into());
        assert!(enforce_bypass_safety(&e).await.is_ok());
    }

    #[tokio::test]
    async fn ant_not_sandboxed_is_refused() {
        let mut e = FakeEnv::default();
        e.vars.insert("USER_TYPE".into(), "ant".into());
        let err = enforce_bypass_safety(&e).await.unwrap_err();
        assert_eq!(err, "--dangerously-skip-permissions can only be used in Docker/sandbox containers with no internet access but got Docker: false, Bubblewrap: false, IS_SANDBOX: false, hasInternet: false");
    }

    #[tokio::test]
    async fn ant_sandboxed_no_internet_passes() {
        let mut e = FakeEnv { docker: true, ..FakeEnv::default() };
        e.vars.insert("USER_TYPE".into(), "ant".into());
        assert!(enforce_bypass_safety(&e).await.is_ok());
    }

    #[tokio::test]
    async fn ant_sandboxed_with_internet_is_refused() {
        let mut e = FakeEnv { docker: true, internet: true, ..FakeEnv::default() };
        e.vars.insert("USER_TYPE".into(), "ant".into());
        let err = enforce_bypass_safety(&e).await.unwrap_err();
        assert!(err.contains("Docker: true") && err.contains("hasInternet: true"));
    }

    #[tokio::test]
    async fn ant_local_agent_entrypoint_skips_check_two() {
        let mut e = FakeEnv::default(); // not sandboxed, no internet
        e.vars.insert("USER_TYPE".into(), "ant".into());
        e.vars.insert("CLAUDE_CODE_ENTRYPOINT".into(), "local-agent".into());
        assert!(enforce_bypass_safety(&e).await.is_ok());
    }

    #[tokio::test]
    async fn non_ant_skips_check_two() {
        let e = FakeEnv::default(); // not sandboxed; non-ant → check 2 skipped
        assert!(enforce_bypass_safety(&e).await.is_ok());
    }
}
```

- [ ] **Step 3: Verify failure**, then **Step 4: Implement** — top of `bypass_guard.rs`:

```rust
//! `--dangerously-skip-permissions` environment safety guards — port of
//! claude-code `setup.ts:395-443`. Runs only when bypass is requested/resolved.
//!
//! Pure decision logic over an injected [`BypassEnv`] so the matrix is testable
//! without root / Docker / network. The real impl lives in the CLI
//! (`apps/cli/src/bypass_env.rs`): `geteuid`, `/.dockerenv`, env, and a 1s
//! HTTP HEAD probe.

/// Host-environment probes the guard needs. Injected so tests can drive every
/// combination.
#[async_trait::async_trait]
pub trait BypassEnv: Send + Sync {
    /// `process.platform === 'win32'`.
    fn is_windows(&self) -> bool;
    /// `process.getuid()` (effective uid). Non-unix hosts return a non-zero
    /// sentinel so check 1 is a no-op.
    fn effective_uid(&self) -> u32;
    /// Read a process env var.
    fn env(&self, key: &str) -> Option<String>;
    /// `getIsDocker()` (`envDynamic.ts:11`): linux && `/.dockerenv` exists.
    fn is_docker(&self) -> bool;
    /// `hasInternetAccess()` (`env.ts:28`): a 1s HEAD to `http://1.1.1.1`.
    async fn has_internet(&self) -> bool;
}

/// `isEnvTruthy` (`envUtils.ts:32-37`): unset/empty ⇒ false; else
/// lowercase-trim ∈ {1, true, yes, on}.
fn env_truthy(v: Option<String>) -> bool {
    v.is_some_and(|s| matches!(s.trim().to_lowercase().as_str(), "1" | "true" | "yes" | "on"))
}

/// Enforce the bypass safety preconditions. `Err(message)` means the
/// environment is unsafe — the caller prints it to stderr and exits 1 (TS
/// `console.error` + `process.exit(1)`).
///
/// # Errors
/// Returns the byte-exact TS refusal message for a root/sudo session or, in an
/// ant build, a non-sandboxed-or-internet-connected session.
pub async fn enforce_bypass_safety(env: &dyn BypassEnv) -> Result<(), String> {
    // Check 1 — root/sudo refusal (all builds; setup.ts:402-414).
    if !env.is_windows()
        && env.effective_uid() == 0
        && env.env("IS_SANDBOX").as_deref() != Some("1")
        && !env_truthy(env.env("CLAUDE_CODE_BUBBLEWRAP"))
    {
        return Err(
            "--dangerously-skip-permissions cannot be used with root/sudo privileges for security reasons"
                .to_string(),
        );
    }

    // Check 2 — ant-only Docker/no-internet (setup.ts:416-442). The USER_TYPE
    // gate keeps it dead in external builds; ported faithfully per the spec.
    let entrypoint = env.env("CLAUDE_CODE_ENTRYPOINT");
    if env.env("USER_TYPE").as_deref() == Some("ant")
        && entrypoint.as_deref() != Some("local-agent")
        && entrypoint.as_deref() != Some("claude-desktop")
    {
        let is_docker = env.is_docker();
        let is_bubblewrap = env_truthy(env.env("CLAUDE_CODE_BUBBLEWRAP"));
        let is_sandbox = env.env("IS_SANDBOX").as_deref() == Some("1");
        let sandboxed = is_docker || is_bubblewrap || is_sandbox;
        let has_internet = env.has_internet().await;
        if !sandboxed || has_internet {
            return Err(format!(
                "--dangerously-skip-permissions can only be used in Docker/sandbox containers with no internet access but got Docker: {is_docker}, Bubblewrap: {is_bubblewrap}, IS_SANDBOX: {is_sandbox}, hasInternet: {has_internet}"
            ));
        }
    }
    Ok(())
}
```

- [ ] **Step 5: Run → PASS. Gate + commit** (`feat(permission): enforce_bypass_safety guard (root refusal + ant docker/no-internet)`). Clippy `-p permission`.

---

### Task 3: CLI flags + real BypassEnv (`argv.rs`, `bypass_env.rs`, `Cargo.toml`)

**Files:**
- Modify: `lingxi-code/apps/cli/Cargo.toml`
- Modify: `lingxi-code/apps/cli/src/argv.rs`
- Create: `lingxi-code/apps/cli/src/bypass_env.rs`
- Modify: `lingxi-code/apps/cli/src/lib.rs` (module decl `mod bypass_env;`)

- [ ] **Step 1: Promote `libc` to a real dep.** In `apps/cli/Cargo.toml`, ADD a `cfg(unix)` dependency section (keep the dev-dep too if other tests use it — check; if the dev-dep was the only `libc` line, replace it):

```toml
[target.'cfg(unix)'.dependencies]
libc = "0.2"
```

- [ ] **Step 2: argv flags + failing test.** In `argv.rs`, add to the struct (after `pub no_tui: bool,`):

```rust
    /// SECURITY-SENSITIVE: bypass all permission prompts for the session
    /// (claude-code `--dangerously-skip-permissions`). Resolves to
    /// `PermissionMode::BypassPermissions` subject to the safety guards
    /// (root refusal; ant sandbox/no-internet) in `permission::bypass_guard`.
    #[arg(long = "dangerously-skip-permissions")]
    pub dangerously_skip_permissions: bool,

    /// Initial permission mode (`--permission-mode <mode>`): one of
    /// `default`/`plan`/`acceptEdits`/`bypassPermissions`/`dontAsk`. Unknown
    /// values resolve to `default` (claude-code `permissionModeFromString`).
    #[arg(long = "permission-mode", value_name = "MODE")]
    pub permission_mode: Option<String>,
```

Add tests in the `argv.rs` tests module:

```rust
    #[test]
    fn dangerously_skip_permissions_flag_parses() {
        let a = Argv::from_iter(["lingxi-cli", "--dangerously-skip-permissions"]).unwrap();
        assert!(a.dangerously_skip_permissions);
    }

    #[test]
    fn permission_mode_flag_parses() {
        let a = Argv::from_iter(["lingxi-cli", "--permission-mode", "plan"]).unwrap();
        assert_eq!(a.permission_mode.as_deref(), Some("plan"));
        let b = Argv::from_iter(["lingxi-cli"]).unwrap();
        assert!(b.permission_mode.is_none());
        assert!(!b.dangerously_skip_permissions);
    }
```

- [ ] **Step 3: Run** `cargo test -p cli argv` → the new tests FAIL (fields absent), then they pass once the fields are added. (clap-derive needs no parse-loop edits.)

- [ ] **Step 4: Real BypassEnv** — `apps/cli/src/bypass_env.rs`:

```rust
//! `RealBypassEnv` — the production [`permission::bypass_guard::BypassEnv`]
//! impl: real `geteuid`, `/.dockerenv`, process env, and a 1s HTTP HEAD probe
//! to `http://1.1.1.1` via the posix HTTP transport (`platform-posix-minimal`).

use std::sync::Arc;
use std::time::Duration;

use permission::bypass_guard::BypassEnv;
use protocol::transport::{HttpMethod, HttpRequest};
use platform_api::HttpTransport;

/// Production environment probe for the bypass safety guard.
pub struct RealBypassEnv {
    http: Arc<dyn HttpTransport>,
}

impl RealBypassEnv {
    /// Build with the posix HTTP transport for the internet probe.
    #[must_use]
    pub fn new() -> Self {
        Self { http: Arc::new(platform_posix_minimal::http::PosixHttp::new()) }
    }
}

impl Default for RealBypassEnv {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl BypassEnv for RealBypassEnv {
    fn is_windows(&self) -> bool {
        cfg!(windows)
    }

    fn effective_uid(&self) -> u32 {
        // Non-unix: return a non-zero sentinel so the root check is a no-op.
        #[cfg(unix)]
        {
            // SAFETY: geteuid is always safe; it only reads the calling
            // process's effective uid and cannot fail.
            unsafe { libc::geteuid() as u32 }
        }
        #[cfg(not(unix))]
        {
            u32::MAX
        }
    }

    fn env(&self, key: &str) -> Option<String> {
        std::env::var(key).ok()
    }

    fn is_docker(&self) -> bool {
        cfg!(target_os = "linux") && std::path::Path::new("/.dockerenv").exists()
    }

    async fn has_internet(&self) -> bool {
        // TS: axios HEAD http://1.1.1.1, 1s timeout, any success ⇒ true.
        let req = HttpRequest {
            method: HttpMethod::Head,
            url: "http://1.1.1.1".to_string(),
            headers: Vec::new(),
            body: None,
            timeout: Some(Duration::from_secs(1)),
        };
        self.http.request(req).await.is_ok()
    }
}
```

(Verify `platform_posix_minimal::http::PosixHttp::new()` is the right path — check `platforms/posix-minimal/src/http.rs`; adjust the path/constructor to match. If `PosixHttp::new` is private or differently named, use the crate's public constructor.)

- [ ] **Step 5: Wire the module.** In `apps/cli/src/lib.rs`, add `mod bypass_env;` next to the other `mod` decls.

- [ ] **Step 6: Build + commit.** `cargo build -p cli` (no direct test for `RealBypassEnv` — it's IO; the guard logic is covered in Task 2). `cargo clippy -p cli --all-targets --no-deps -- -D warnings`. Commit (`feat(cli): --dangerously-skip-permissions/--permission-mode flags + RealBypassEnv`).

---

### Task 4: engine-desktop `DesktopConfig.permission_mode`

**Files:**
- Modify: `lingxi-code/apps/engine-desktop/src/lib.rs`

- [ ] **Step 1: Add the field + doc.** In the `DesktopConfig` struct (after `use_noop_permission_gate`), add:

```rust
    /// CLI-resolved session permission mode (claude-code
    /// `initialPermissionModeFromCLI`). Replaces the previously hardwired
    /// `BuiltinToolContext.permission_mode = Default`. When
    /// `LINGXI_ENFORCE_PERMISSIONS` enforcement is ON, this OVERRIDES the
    /// settings `defaultMode` as the highest-priority source; `BypassPermissions`
    /// makes the policy allow-all (unless the bypass killswitch is set).
    ///
    /// EXECUTION-SEMANTICS NOTE: with the default `NoOpPermissionGate`
    /// (enforcement OFF) this field is execution-neutral — tools already
    /// all-allow — but it still drives `BuiltinToolContext.permission_mode`
    /// state. The CLI flag deliberately does NOT switch enforcement on.
    pub permission_mode: permission::PermissionMode,
```

- [ ] **Step 2: Default.** Ensure `DesktopConfig`'s `Default` impl (or the `#[derive(Default)]`/manual builder) sets `permission_mode: permission::PermissionMode::Default`. If `DesktopConfig` derives `Default`, add a manual default only if `PermissionMode` lacks one — `PermissionMode` is `Copy` but check it has `Default`; if not, the `DesktopConfig` Default must be manual for this field. (Find the existing `impl Default for DesktopConfig` or the `#[derive]`; match the established pattern. If it derives Default and `PermissionMode` has no `Default`, add `impl Default for PermissionMode { fn default() -> Self { Self::Default } }` in `permission/src/mode.rs` with a doc comment, since `Default` mode is the natural default — and add a one-line unit test `assert_eq!(PermissionMode::default(), PermissionMode::Default)`.)

- [ ] **Step 3: Feed BuiltinToolContext.** Find `permission_mode: PermissionMode::Default,` in the `BuiltinToolContext { … }` construction (around lib.rs:1445) and change it to `permission_mode: cfg.permission_mode,`.

- [ ] **Step 4: Override the policy mode.** In the `LINGXI_ENFORCE_PERMISSIONS` block, AFTER the per-tier loop that computes `mode` from settings `defaultMode` (around lib.rs:1123) and BEFORE `PermissionPolicy::from_rules(mode, rules)`, add:

```rust
            // CLI-resolved mode is the highest-priority source (TS orderedModes:
            // CLI flag/--permission-mode outranks settings defaultMode). Apply
            // it only when the CLI actually requested a non-default mode, so an
            // unset CLI keeps the settings defaultMode.
            if cfg.permission_mode != permission::PermissionMode::Default {
                mode = cfg.permission_mode;
            }
```

- [ ] **Step 5: Test (policy feed).** Add a unit test in engine-desktop's test module (or wherever `DesktopConfig`/build is integration-tested) that asserts a `DesktopConfig { permission_mode: BypassPermissions, .. }` under enforcement yields a policy whose `authorize` of an unmatched tool returns allow (and, with the killswitch set, falls back to Ask). If a direct build-level seam is awkward, assert at minimum that `BuiltinToolContext.permission_mode` reflects `cfg.permission_mode` via the lightest available seam, and document why a fuller assertion is deferred. (Reuse the existing engine-desktop test patterns; do NOT spin up a live provider.)

- [ ] **Step 6: Gate + commit.** `cargo test -p engine-desktop` (+ `-p permission` if you added the `Default` impl); `cargo clippy -p engine-desktop --all-targets --no-deps -- -D warnings`. Commit (`feat(engine-desktop): thread DesktopConfig.permission_mode into context + policy`).

---

### Task 5: run_cli wiring (resolve → guard → notice → thread)

**Files:**
- Modify: `lingxi-code/apps/cli/src/lib.rs`
- Modify: `lingxi-code/apps/cli/src/init.rs`

- [ ] **Step 1: Thread the mode into `resolve_desktop_config`.** In `init.rs`, change `fn resolve_desktop_config(argv: &Argv) -> DesktopConfig` to `fn resolve_desktop_config(argv: &Argv, permission_mode: permission::PermissionMode) -> DesktopConfig`, and in the returned struct add `permission_mode,`. Update `build_runtime` to take + forward the mode: `pub async fn build_runtime(argv: &Argv, output: Arc<dyn OutputStream>, permission_mode: permission::PermissionMode) -> Result<Runtime, InitError>` → `let cfg = resolve_desktop_config(argv, permission_mode);`. (Add `permission` to `apps/cli/Cargo.toml` deps if not already present — check; the CLI already depends on it transitively via engine but needs a direct path dep for the type. Add `permission = { path = "../../permission" }`.)

- [ ] **Step 2: Resolve + guard in `run_cli`.** In `lib.rs`, AFTER `cwd::apply_cwd(...)` (line ~94) and BEFORE `build_runtime` (line ~115), insert:

```rust
    // (Item B) Resolve the session permission mode from CLI flags + settings,
    // run the bypass safety guards, and capture the startup notice. This must
    // happen BEFORE build_runtime so a refused bypass exits before the runtime
    // is constructed, and so the resolved mode threads into DesktopConfig.
    let (permission_mode, permission_notice) = {
        let settings = read_cli_mode_settings(&parsed);
        let (mode, notice) = permission::initial_permission_mode_from_cli(
            parsed.permission_mode.as_deref(),
            parsed.dangerously_skip_permissions,
            &settings,
        );
        // Guards run when bypass is requested OR resolved (setup.ts:396).
        if mode == permission::PermissionMode::BypassPermissions
            || parsed.dangerously_skip_permissions
        {
            if let Err(msg) = permission::enforce_bypass_safety(&bypass_env::RealBypassEnv::new()).await {
                eprintln!("{msg}");
                return exit_codes::INVALID_USAGE; // TS process.exit(1)
            }
        }
        (mode, notice)
    };
```

(Use the actual `exit_codes` constant for a `1` exit — check `apps/cli/src/exit_codes.rs` for the right name, e.g. `INVALID_USAGE`/`RUNTIME_ERROR`/`GENERAL`; pick the one whose value is `1`. If none is `1`, use the closest established "error exit" constant and note it.)

- [ ] **Step 3: Helper `read_cli_mode_settings`.** Add to `lib.rs` (or `init.rs` and re-import) a small helper that loads the merged settings and derives `CliModeSettings`:

```rust
/// Build `CliModeSettings` from the merged user+project settings (the same
/// `lingxi_core::settings::Settings::load` seam the CLI uses elsewhere). On any
/// load failure, returns the no-op default (None mode, killswitch off) — a
/// faithful degrade (TS `getSettings_DEPRECATED() || {}`).
fn read_cli_mode_settings(parsed: &Argv) -> permission::CliModeSettings {
    let project_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let _ = parsed; // reserved (CLI has no settings-path override flag today)
    // Read the user + project settings.json raw and derive the two fields via
    // the existing permission helpers. `default_mode_from_settings_json` and
    // `bypass_permissions_disabled_from_settings_json` read a single file's raw
    // text; project (read last) wins on defaultMode; bypass-disable is sticky.
    let mut default_mode = None;
    let mut bypass_disabled = false;
    let home = dirs::home_dir().map(|h| h.join(".claude").join("settings.json"));
    let proj = project_dir.join(".claude").join("settings.json");
    for path in [home, Some(proj)].into_iter().flatten() {
        if let Ok(raw) = std::fs::read_to_string(&path) {
            if let Some(m) = permission::default_mode_from_settings_json(&raw) {
                default_mode = Some(m); // project read last → wins
            }
            if permission::bypass_permissions_disabled_from_settings_json(&raw) {
                bypass_disabled = true; // sticky
            }
        }
    }
    permission::CliModeSettings { default_mode, bypass_disabled }
}
```

(Confirm `default_mode_from_settings_json` + `bypass_permissions_disabled_from_settings_json` are re-exported at `permission::` root — they are used by engine-desktop as `permission::default_mode_from_settings_json`, so yes. Read order user→project matches engine-desktop's ascending-priority loop.)

- [ ] **Step 4: Pass the mode to build_runtime + print the notice.** Change the existing `build_runtime(&parsed, adapter)` call (line ~115) to `build_runtime(&parsed, adapter, permission_mode)`. Then, in the existing pre-REPL notice block (next to `startup_deprecation_notice`, ~line 153), add:

```rust
    // (Item B) Permission-mode startup notice (e.g. bypass disabled by
    // settings). Same bounded stderr channel as the deprecation/migration
    // notices (no UI notification-queue substrate).
    if let Some(notice) = &permission_notice {
        eprintln!("{notice}");
    }
```

- [ ] **Step 5: Build + smoke test.** `cargo build -p cli`. Manual smoke (non-root dev box, no bypass): `cargo run -p cli -- --permission-mode plan -p "hi" </dev/null` reaches build (mode resolves to Plan, no guard since not bypass). `cargo run -p cli -- --dangerously-skip-permissions -p "hi" </dev/null` — on a non-root, internet-connected, non-ant dev box: check 1 skipped (non-root), check 2 skipped (non-ant) → proceeds (then fails later on no API key, fine). Report what happened.

- [ ] **Step 6: Gate + commit.** `cargo clippy -p cli --all-targets --no-deps -- -D warnings`. Commit (`feat(cli): resolve permission mode + run bypass guards + notice pre-REPL`).

---

### Task 6: telemetry — `tengu_bypass_permissions_mode_dialog_accept` (348→349)

**This is the W36/W38 shared-constant hazard task — follow the sweep exactly.**

**Files:**
- Create: `lingxi-code/telemetry/src/tengu/permission.rs`
- Modify: `lingxi-code/telemetry/src/tengu/mod.rs`
- Modify: `lingxi-code/test-harness/src/parity/fixtures/tengu_events.json`
- Modify: every `348` count-assert site

- [ ] **Step 1: New module** `telemetry/src/tengu/permission.rs`:

```rust
//! Permission-flow events. Appended as their own registry block (after the
//! config-migration block).

/// `tengu_bypass_permissions_mode_dialog_accept`
/// (`BypassPermissionsModeDialog.tsx` accept branch).
pub const BYPASS_PERMISSIONS_MODE_DIALOG_ACCEPT: &str = "tengu_bypass_permissions_mode_dialog_accept";

/// Registry block.
pub const NAMES: [&str; 1] = [BYPASS_PERMISSIONS_MODE_DIALOG_ACCEPT];
```

- [ ] **Step 2: Register in `tengu/mod.rs`.** Add `pub mod permission;` next to the other module decls; append a history comment `// Bypass-permissions dialog: +1 (permission::NAMES) → 349 total.`; change `const TOTAL: usize = … + 13 + 3 + 9;` to `… + 13 + 3 + 9 + 1;`; and add the copy loop in `concat_all()` AFTER the migration-block loop:

```rust
        let mut i = 0;
        while i < permission::NAMES.len() {
            out[idx] = permission::NAMES[i];
            idx += 1;
            i += 1;
        }
```

- [ ] **Step 3: Fixture append.** In `test-harness/src/parity/fixtures/tengu_events.json`, append `"tengu_bypass_permissions_mode_dialog_accept"` at the very end of the `event_names` array and extend the `_note` with a `"+1 bypass dialog → 349"` sentence (open the file first to match its exact shape).

- [ ] **Step 4: THE SWEEP.**

```bash
cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-code
grep -rn "348" --include="*.rs" . | grep -v target | grep -i "event\|tengu\|ALL_EVENT"
```

Known sites to update 348 → 349 (re-grep regardless):
- `telemetry/src/tengu/mod.rs` (TOTAL + history comment)
- `telemetry/tests/event_name_completeness_test.rs:52` (`len() == 348`), `:245-247` (the `[339..348]` slice → add a `[348..349]` permission-block slice assert, leave the migration slice intact), `:51` history comment
- `telemetry/tests/settings_schema_test.rs:24-25` (assert + running-sum string)
- `orchestrator/src/diagnostics.rs:189` (`expected = 348`) + the `:188` history comment + the test name `:212` (`..._at_348` → `..._at_349`)
- `tui/src/components/prompt_input/vim.rs:1603` and `:1615` (+ their `:1602`/`:1614` comments)
- `tui/tests/behavior_palette.rs:148` (+ `:146` comment)

- [ ] **Step 5: RUN (not `--no-run`) the dependent crates.**

```bash
cargo test -p telemetry
cargo test -p orchestrator diagnostics
cargo test -p tui vim
cargo test -p tui --test behavior_palette
cargo test -p test-harness --test parity_tengu_events
```

All PASS.

- [ ] **Step 6: Gate + commit** (`feat(telemetry): register tengu_bypass_permissions_mode_dialog_accept (348→349)`). Clippy `-p telemetry`.

---

### Task 7: bypass dialog state + render (`tui/src/startup_bypass.rs`)

**Files:**
- Create: `lingxi-code/tui/src/startup_bypass.rs`
- Modify: `lingxi-code/tui/src/lib.rs` (`pub mod startup_bypass;`)

- [ ] **Step 1: Failing tests** — bottom of `startup_bypass.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_highlights_decline() {
        // Select order is [No, exit] then [Yes, I accept]; default highlight on
        // the first (decline-first), matching the TS Select option order.
        let s = BypassDialogState::default();
        assert_eq!(s.selected, BypassChoice::Decline);
    }

    #[test]
    fn arrows_toggle_choice() {
        let mut s = BypassDialogState::default();
        assert_eq!(handle_key(&mut s, key(KeyCode::Down)), None);
        assert_eq!(s.selected, BypassChoice::Accept);
        handle_key(&mut s, key(KeyCode::Up));
        assert_eq!(s.selected, BypassChoice::Decline);
    }

    #[test]
    fn enter_on_accept_returns_accept() {
        let mut s = BypassDialogState { selected: BypassChoice::Accept };
        assert_eq!(handle_key(&mut s, key(KeyCode::Enter)), Some(BypassDialogOutcome::Accept));
    }

    #[test]
    fn enter_on_decline_returns_decline() {
        let mut s = BypassDialogState::default();
        assert_eq!(handle_key(&mut s, key(KeyCode::Enter)), Some(BypassDialogOutcome::Decline));
    }

    #[test]
    fn esc_declines() {
        let mut s = BypassDialogState::default();
        assert_eq!(handle_key(&mut s, key(KeyCode::Esc)), Some(BypassDialogOutcome::Decline));
    }

    #[test]
    fn render_lines_are_byte_exact() {
        let lines = render_lines();
        assert_eq!(lines[0], "WARNING: Claude Code running in Bypass Permissions mode");
        assert!(lines.iter().any(|l| l == "In Bypass Permissions mode, Claude Code will not ask for your approval before running potentially dangerous commands."));
        assert!(lines.iter().any(|l| l == "This mode should only be used in a sandboxed container/VM that has restricted internet access and can easily be restored if damaged."));
        assert!(lines.iter().any(|l| l == "By proceeding, you accept all responsibility for actions taken while running in Bypass Permissions mode."));
        assert!(lines.iter().any(|l| l == "https://code.claude.com/docs/en/security"));
        assert!(lines.iter().any(|l| l == "No, exit"));
        assert!(lines.iter().any(|l| l == "Yes, I accept"));
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }
}
```

- [ ] **Step 2: Verify failure**, then **Step 3: Implement** — top of `startup_bypass.rs` (mirror the crossterm key types the rest of the TUI uses — check `tui/src/components/permissions/tool_use_confirm.rs` for the exact `KeyEvent`/`KeyCode` import path and the `DialogResolution` pattern):

```rust
//! Startup bypass-permissions confirmation — pure state machine + render for
//! the TTY-only `BypassPermissionsModeDialog` (claude-code
//! `components/BypassPermissionsModeDialog.tsx`, shown by `interactiveHelpers
//! showSetupScreens` before the REPL when bypass is resolved and
//! `skipDangerousModePermissionPrompt` is not yet set).
//!
//! Pure: `handle_key` drives a two-option Select; the terminal mount loop
//! lives in the CLI (`apps/cli/src/mode.rs`) and is thin (cannot be driven
//! headless, same caveat as `run_tui_session`).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// The two dialog choices (Select order: decline first, accept second).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BypassChoice {
    /// `No, exit`.
    Decline,
    /// `Yes, I accept`.
    Accept,
}

/// Dialog state — just the highlighted choice.
#[derive(Debug, Clone, Copy)]
pub struct BypassDialogState {
    /// Currently highlighted option.
    pub selected: BypassChoice,
}

impl Default for BypassDialogState {
    fn default() -> Self {
        // Decline-first highlight (matches the TS Select option order).
        Self { selected: BypassChoice::Decline }
    }
}

/// Terminal outcome of the dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BypassDialogOutcome {
    /// User accepted: persist `skipDangerousModePermissionPrompt` + proceed.
    Accept,
    /// User declined (or pressed Esc): exit 1.
    Decline,
}

/// Handle one key. `Some(outcome)` ends the dialog; `None` keeps it open.
#[must_use]
pub fn handle_key(state: &mut BypassDialogState, key: KeyEvent) -> Option<BypassDialogOutcome> {
    match key.code {
        KeyCode::Up | KeyCode::Down | KeyCode::Char('k') | KeyCode::Char('j') | KeyCode::Tab | KeyCode::BackTab => {
            state.selected = match state.selected {
                BypassChoice::Decline => BypassChoice::Accept,
                BypassChoice::Accept => BypassChoice::Decline,
            };
            None
        }
        KeyCode::Enter => Some(match state.selected {
            BypassChoice::Accept => BypassDialogOutcome::Accept,
            BypassChoice::Decline => BypassDialogOutcome::Decline,
        }),
        KeyCode::Esc => Some(BypassDialogOutcome::Decline),
        _ => None,
    }
}

/// The byte-exact display lines (title, body, link, options).
#[must_use]
pub fn render_lines() -> Vec<String> {
    vec![
        "WARNING: Claude Code running in Bypass Permissions mode".to_string(),
        "In Bypass Permissions mode, Claude Code will not ask for your approval before running potentially dangerous commands.".to_string(),
        "This mode should only be used in a sandboxed container/VM that has restricted internet access and can easily be restored if damaged.".to_string(),
        "By proceeding, you accept all responsibility for actions taken while running in Bypass Permissions mode.".to_string(),
        "https://code.claude.com/docs/en/security".to_string(),
        "No, exit".to_string(),
        "Yes, I accept".to_string(),
    ]
}

/// Whether the bypass dialog must be shown: bypass mode resolved AND no
/// settings tier has `skipDangerousModePermissionPrompt` truthy
/// (`hasSkipDangerousModePermissionPrompt`, claude-code settings.ts:882-889 —
/// user+local here; flag/policy tiers have no Rust substrate).
#[must_use]
pub fn should_show_bypass_dialog(is_bypass_mode: bool, skip_prompt_already_set: bool) -> bool {
    is_bypass_mode && !skip_prompt_already_set
}
```

- [ ] **Step 4: Run → PASS. Gate + commit** (`feat(tui): bypass-permissions confirm dialog state + render`). Clippy `-p tui` (note: tui carries documented pre-existing baseline lints — your new file must add ZERO; the count test you touched in Task 6 is separate).

---

### Task 8: mount the dialog + final gates

**Files:**
- Modify: `lingxi-code/apps/cli/src/mode.rs`
- Modify: `lingxi-code/apps/cli/src/lib.rs` (pass the bypass-mode flag into the TUI path)

- [ ] **Step 1: Thread the resolved mode to the TUI dispatch.** The bypass dialog shows only on the interactive TUI path. In `run_cli`, after resolving `permission_mode`, compute `let is_bypass = permission_mode == permission::PermissionMode::BypassPermissions;` and pass it through `mode::dispatch` to the TUI launch (extend the dispatch signature or stash it on a small struct — match the existing threading style for `parsed`/`runtime`). Read `skipDangerousModePermissionPrompt` from user+local settings via `migrations::settings_update::{settings_path, read_settings_map}` (the `migrations` crate is a sibling; add `migrations = { path = "../../migrations" }` to `apps/cli/Cargo.toml` if not present — it was added for the migrations wiring, confirm).

- [ ] **Step 2: Mount before `run_tui_session`.** In `mode.rs`, in the TUI arm just before `mount_tui_runtime(...)`/`run_tui_session(...)`, add a blocking pre-session gate:

```rust
    // (Item B) TTY-only bypass confirmation (claude-code showSetupScreens).
    if tui::startup_bypass::should_show_bypass_dialog(is_bypass, skip_prompt_already_set) {
        match mount_bypass_dialog().await {
            tui::startup_bypass::BypassDialogOutcome::Accept => {
                // Persist skipDangerousModePermissionPrompt=true (so future
                // launches skip the dialog) + emit the accept event.
                persist_skip_dangerous_prompt();
                bus.log_event(
                    telemetry::tengu::permission::BYPASS_PERMISSIONS_MODE_DIALOG_ACCEPT,
                    std::collections::HashMap::new(),
                ).await;
            }
            tui::startup_bypass::BypassDialogOutcome::Decline => {
                return exit_codes::INVALID_USAGE; // TS gracefulShutdownSync(1)
            }
        }
    }
```

`mount_bypass_dialog()` is a thin terminal loop: acquire the terminal guard (`tui::terminal`), draw `render_lines()` with the highlighted option, read crossterm key events, feed `startup_bypass::handle_key` until it returns an outcome, restore the terminal, return the outcome. Keep it minimal — it is the one untestable-headless piece (documented, same caveat as `run_tui_session`); all decision logic is in the tested `handle_key`. `persist_skip_dangerous_prompt()` calls `migrations::settings_update::update_settings(&user_settings_path, vec![("skipDangerousModePermissionPrompt".into(), Some(json!(true)))])`. If wiring a `bus` here is awkward (no pre-session bus), emit via the runtime's bus if available else skip the event with a `// TODO`-free doc note (the event is best-effort; prefer wiring the existing runtime bus).

- [ ] **Step 3: Build.** `cargo build -p cli`.

- [ ] **Step 4: FULL GATE RITUAL.**

```bash
cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-code
cargo test -p permission
cargo test -p cli
cargo test -p engine-desktop
cargo test -p tui startup_bypass
cargo test -p telemetry
cargo test -p orchestrator diagnostics
cargo test -p tui vim
cargo test -p tui --test behavior_palette
cargo test -p test-harness --test parity_tengu_events
cargo clippy -p permission -p cli -p engine-desktop -p tui -p telemetry --all-targets --no-deps -- -D warnings
cargo test --workspace --no-run          # struct-trap (~2-3 min)
cargo build -p engine-desktop
cargo build -p engine-mobile
cargo tree -p engine-mobile | grep -c "bypass_guard\|cli_mode" # expect 0 (these live in permission, pulled by mobile?) — see note
```

NOTE on engine-mobile: `cli_mode`/`bypass_guard` live in the `permission` crate, which engine-mobile MAY depend on. That is acceptable — the new code is pure/inert (no libc/http; the real `BypassEnv` impl lives in `apps/cli`, NOT pulled by mobile). Verify mobile pulls NONE of `apps/cli/src/bypass_env.rs` / the CLI flags / the dialog: `cargo tree -p engine-mobile | grep -ci "apps-cli\|^cli "` (expect 0). Document the permission-crate sharing as benign.

- [ ] **Step 5: Frozen-surface check.** `git diff main -- lingxi-code/traits lingxi-code/protocol` → MUST be empty.

- [ ] **Step 6: Commit** (`feat(cli): mount TTY bypass-permissions dialog before the REPL`).

---

## Final verification (whole-branch)

1. `cargo test -p permission -p cli -p engine-desktop -p tui -p telemetry` — all green.
2. The Task 8 gate ritual, all green; engine-mobile pulls no CLI/dialog code.
3. `git diff main -- lingxi-code/traits lingxi-code/protocol` — empty (frozen surfaces).
4. Re-read the spec's "Safety invariants": root refusal unconditional (Task 2 test `root_without_sandbox_is_refused` + the run_cli wiring exits 1); dialog blocks the interactive session (Task 7/8); flag never enables enforcement (Task 4 doc + execution-neutral default); frozen surfaces untouched.
5. Update memory (`parity-1to1-effort.md`): item B (`--dangerously-skip-permissions` + bypassPermissions + permission-mode notice) → DONE; record the bypass dialog, the guards, and the documented deferrals (Shift+Tab cycle, Statsig gate, auto/CCR, mid-session killswitch re-check, enforcement-default-on).
