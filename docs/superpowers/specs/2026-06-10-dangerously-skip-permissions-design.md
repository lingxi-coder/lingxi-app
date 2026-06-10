# --dangerously-skip-permissions + bypassPermissions Mode + Startup Notice — Design

**Date:** 2026-06-10
**Status:** Approved (user decisions: full interactive TUI dialog; flag sets mode + guards only [keep LINGXI_ENFORCE_PERMISSIONS as the execution switch]; port ALL setup.ts safety guards)
**Reference of truth:** `claude-code/` TS — `utils/permissions/permissionSetup.ts:689-812` (`initialPermissionModeFromCLI`), `setup.ts:395-443` (safety guards), `interactiveHelpers.tsx:218-223` + `components/BypassPermissionsModeDialog.tsx` (dialog), `main.tsx:1388-1411,2879-2888` (notice), `types/permissions.ts:16-39`, `utils/permissions/PermissionMode.ts:117-121`.
**Branch:** new feature branch off `main`, merged locally after gates (established parity pipeline).

## Goal

Port claude-code's `--dangerously-skip-permissions` CLI flag, the `bypassPermissions` permission mode, and the permission-mode startup notice to the Rust port. SECURITY-SENSITIVE (skip-all-permissions posture). Closes parity-remainder item B.

## Current Rust substrate (verified)

- `permission::PermissionMode` enum already has `BypassPermissions` (+ `is_external()` covering exactly the 5 external modes: Default/Plan/AcceptEdits/BypassPermissions/DontAsk — matches TS `EXTERNAL_PERMISSION_MODES`).
- `permission::PermissionPolicy` already enforces mode in `authorize`; `bypass_permissions_disabled_from_settings_json` + `default_mode_from_settings_json` exist; the killswitch (`disableBypassPermissionsMode`) is already wired into the engine-desktop policy build.
- Default gate is `NoOpPermissionGate` (always-allow); `PolicyPermissionGate` only builds behind opt-in `LINGXI_ENFORCE_PERMISSIONS`.
- `apps/engine-desktop/src/lib.rs` hardwires `BuiltinToolContext.permission_mode = PermissionMode::Default` and computes the policy `mode` from settings `defaultMode` only.
- CLI `argv.rs` has no `--dangerously-skip-permissions` / `--permission-mode` flag.
- `skipDangerousModePermissionPrompt` settings key is written by the `migrations` crate but has no consumer.
- TUI: `permission::next_permission_mode` exists but is NOT wired to a Shift+Tab cycle; mid-session permission prompts exist (`tui/src/components/permissions/`); there is no startup-blocking dialog phase.

## Architecture

```
permission/src/cli_mode.rs     # initial_permission_mode_from_cli (pure) + permission_mode_from_cli_string
permission/src/bypass_guard.rs # safety guards (root refusal + ant docker/no-internet); side-effects isolated
apps/cli/src/argv.rs           # --dangerously-skip-permissions, --permission-mode <mode>
apps/cli/src/lib.rs            # pre-REPL: resolve mode → run guards → print notice → thread into DesktopConfig
apps/engine-desktop/src/lib.rs # DesktopConfig.permission_mode replaces the hardwired Default; feeds policy mode
tui/src/startup_bypass.rs (new)+ session.rs  # TTY-only blocking BypassPermissionsModeDialog before the REPL
```

`cli_mode.rs` is a pure function (parsed flags + a settings view in, `(mode, notice)` out — no env, no IO) so the priority logic is exhaustively unit-testable. `bypass_guard.rs` isolates the side-effecting probes (geteuid / `/.dockerenv` / `HEAD 1.1.1.1`) behind a small injectable seam so the matrix is testable without a container.

## Component 1: mode resolution (`cli_mode.rs`)

Port of `initialPermissionModeFromCLI` (permissionSetup.ts:689-812).

```rust
/// The settings inputs the resolver reads (caller extracts from the merged
/// raw settings map; keeps this fn pure / env-free).
pub struct CliModeSettings {
    /// `settings.permissions.defaultMode` (already validated to an external mode, else None).
    pub default_mode: Option<PermissionMode>,
    /// `settings.permissions.disableBypassPermissionsMode === "disable"`.
    pub bypass_disabled: bool,
}

pub fn initial_permission_mode_from_cli(
    permission_mode_cli: Option<&str>,
    dangerously_skip: bool,
    settings: &CliModeSettings,
) -> (PermissionMode, Option<String>) { … }
```

Logic (1:1, ordered-modes then first-valid):
1. Build `ordered_modes`: if `dangerously_skip` → push `BypassPermissions`; if `permission_mode_cli` → push `permission_mode_from_cli_string(..)`; if `settings.default_mode` → push it.
2. Walk `ordered_modes`; for a `BypassPermissions` candidate when `bypass_disabled`, set `notification = "Bypass permissions mode was disabled by settings"` and `continue`; otherwise take it as the result and stop.
3. No valid mode → `(Default, notification)`.

`permission_mode_from_cli_string(s) -> PermissionMode` ports `permissionModeFromString` (PermissionMode.ts:117-121): the valid set is the 5 external modes (`acceptEdits`/`bypassPermissions`/`default`/`dontAsk`/`plan`); anything else → `Default`. Reuse `PermissionMode::is_external` for the membership check.

**Documented omissions (ant-only / no-substrate, faithful to the external build):**
- Statsig `tengu_disable_bypass_permissions_mode` gate → its `"…disabled by your organization policy"` notice string is unreachable (no Statsig substrate). Only the settings-disable string is reachable.
- `auto` / `TRANSCRIPT_CLASSIFIER` branches (classifier is ant-only, correctly stubbed) → omitted; `permission_mode_from_cli_string("auto")` falls to `Default` like TS in a non-ant build.
- `CLAUDE_CODE_REMOTE` branch (ignore non-acceptEdits/plan/default settings `defaultMode`): NOT ported — LingXi has no CCR remote entrypoint; the `tengu_ccr_unsupported_default_mode_ignored` event is not reproduced. Documented on the fn. (The caller still passes `default_mode` straight through; CCR's extra filtering is the only divergence.)

## Component 2: safety guards (`bypass_guard.rs`) — port ALL of setup.ts:395-443

Runs only when `mode == BypassPermissions || dangerously_skip`.

```rust
/// Injectable probes so the matrix is testable without root/docker/network.
pub trait BypassEnv {
    fn is_windows(&self) -> bool;       // cfg!(windows)
    fn effective_uid(&self) -> u32;     // libc::geteuid()
    fn env(&self, key: &str) -> Option<String>;
    fn is_docker(&self) -> bool;        // linux && Path::new("/.dockerenv").exists()
    async fn has_internet(&self) -> bool; // HEAD http://1.1.1.1, 1s timeout, via the http trait
}

/// Returns Err(message) when the environment is unsafe — caller prints to
/// stderr and exits 1 (TS console.error + process.exit(1)).
pub async fn enforce_bypass_safety(env: &dyn BypassEnv) -> Result<(), String> { … }
```

Two checks, in TS order:
1. **Root refusal** (security-load-bearing, all builds): `!is_windows && effective_uid()==0 && env("IS_SANDBOX") != "1" && !is_env_truthy(env("CLAUDE_CODE_BUBBLEWRAP"))` → `Err("--dangerously-skip-permissions cannot be used with root/sudo privileges for security reasons")`.
2. **ant docker/no-internet** (USER_TYPE gate keeps it dead in external builds, ported faithfully per user decision): when `env("USER_TYPE")=="ant" && env("CLAUDE_CODE_ENTRYPOINT") != "local-agent" && != "claude-desktop"` — compute `is_bubblewrap` (linux && `is_env_truthy(CLAUDE_CODE_BUBBLEWRAP)`), `is_sandbox` (`IS_SANDBOX=="1"`), `sandboxed = is_docker || is_bubblewrap || is_sandbox`, await `has_internet`. If `!sandboxed || has_internet` → `Err(format!("--dangerously-skip-permissions can only be used in Docker/sandbox containers with no internet access but got Docker: {is_docker}, Bubblewrap: {is_bubblewrap}, IS_SANDBOX: {is_sandbox}, hasInternet: {has_internet}"))` (byte-exact field order/casing).

The real `BypassEnv` impl uses `libc::geteuid()` (libc already a workspace dep on the posix path), `std::env`, `Path::new("/.dockerenv").exists()` gated on `cfg!(target_os="linux")`, and the existing http trait for the 1s HEAD probe.

## Component 3: CLI flags + wiring (`argv.rs`, `lib.rs`)

- `argv.rs`: add `pub dangerously_skip_permissions: bool` and `pub permission_mode: Option<String>` with the long flags `--dangerously-skip-permissions` (bool) and `--permission-mode <mode>` (value). Parse in the existing argv loop following the established pattern; document the security posture on the field.
- `run_cli` pre-REPL (the migrations / deprecation-notice point): read the merged settings (the same `engine::settings::Settings::load` seam the CLI already uses for output-style) to build `CliModeSettings { default_mode, bypass_disabled }`; call `initial_permission_mode_from_cli`; then `enforce_bypass_safety(&RealBypassEnv).await` → on `Err(msg)` print to stderr and exit 1 (`exit_codes`); on success, if `notice` is `Some`, `eprintln!` it (same channel as the deprecation/migration notices); thread the resolved `mode` into `DesktopConfig.permission_mode`.

## Component 4: mode threading + execution semantics (engine-desktop)

- `DesktopConfig` gains `permission_mode: PermissionMode` (default `Default`).
- `BuiltinToolContext.permission_mode` reads `cfg.permission_mode` instead of the hardwired `Default`.
- Policy feed: in the `LINGXI_ENFORCE_PERMISSIONS` block, the CLI-resolved mode OVERRIDES the settings `defaultMode` (it is the highest-priority source, matching TS `orderedModes`). When `cfg.permission_mode == BypassPermissions`, the existing `PermissionPolicy` `authorize` allows all unless the killswitch (`bypass_disabled`) is set — already implemented; this batch only feeds the mode in.
- **Execution-semantics doc (user decision):** `LINGXI_ENFORCE_PERMISSIONS` stays the execution master switch. With enforcement ON, `BypassPermissions` makes the policy allow-all. With enforcement OFF (the `NoOpPermissionGate` default), the flag is execution-neutral (tools already all-allow) but the root guard, the startup notice, and `BuiltinToolContext.permission_mode` state still take effect faithfully. The flag deliberately does NOT silently switch enforcement on. This is documented on `DesktopConfig.permission_mode` and at the wiring site.

## Component 5: startup notice

The `notice` from Component 1 (e.g. `"Bypass permissions mode was disabled by settings"`) is printed to stderr at the pre-REPL point (the bounded stand-in for claude-code's `initialNotifications` priority-high queue — same approach the model-deprecation warning already uses; no UI notification-queue substrate is built).

## Component 6: BypassPermissionsModeDialog (TTY-only blocking confirm)

Faithful to `interactiveHelpers.tsx:218-223` + `BypassPermissionsModeDialog.tsx`: shown ONLY on the interactive TUI path, before the REPL, when `mode == BypassPermissions && !has_skip_dangerous_mode_permission_prompt()` (read user+local settings via the migrations crate's `read_settings_map` + the `settings_path` helpers — `hasSkipDangerousModePermissionPrompt` checks user/local/flag/policy; flag/policy have no Rust substrate, documented).

- New `tui/src/startup_bypass.rs`: a pure `BypassDialogState` + `handle_key` (two options: `No, exit` / `Yes, I accept`; default highlight on "No, exit" to match the decline-first Select order) returning a `BypassDialogOutcome::{Accept, Decline}`, plus a render fn emitting the byte-exact strings:
  - Title: `WARNING: Claude Code running in Bypass Permissions mode`
  - Body line 1: `In Bypass Permissions mode, Claude Code will not ask for your approval before running potentially dangerous commands.` + newline + `This mode should only be used in a sandboxed container/VM that has restricted internet access and can easily be restored if damaged.`
  - Body line 2: `By proceeding, you accept all responsibility for actions taken while running in Bypass Permissions mode.`
  - A docs link line: `https://code.claude.com/docs/en/security`
  - Options: `No, exit` (decline) / `Yes, I accept` (accept).
- `run_tui_session` (or the CLI just before it) mounts the dialog as a blocking pre-session step: `Accept` → write `skipDangerousModePermissionPrompt: true` to userSettings (reuse `migrations::settings_update::update_settings` via the user settings path), emit `tengu_bypass_permissions_mode_dialog_accept`, then proceed; `Decline` → exit 1 (TS `gracefulShutdownSync(1)`). It is NOT a mid-session `Screen` overlay — it is a one-shot startup gate.
- Telemetry: `tengu_bypass_permissions_mode_dialog_accept` registered (1 new event; W36 count-sweep applies — registry 348→349, fixture append, dependent-crate count assertions).

Print / non-TTY paths show no dialog (TS gates it inside `interactiveHelpers`, TTY-only); they rely on the safety guards alone, matching TS.

## Safety invariants

1. Root/sudo refusal is unconditional on all builds (non-Windows, non-sandbox) — the security core; exit 1 before any tool can run.
2. The dialog blocks the interactive session until the user accepts (or exits) — no bypass session starts un-acknowledged unless `skipDangerousModePermissionPrompt` was already accepted.
3. The flag never silently enables the half-built enforcement engine; with the default NoOp gate, tool execution is unchanged (already all-allow), but the guard/notice/state still apply.
4. Frozen surfaces (`traits/`, `protocol/`) untouched; engine-mobile pulls none of this (CLI/engine-desktop/TUI-only).

## Testing strategy

- `cli_mode.rs`: priority matrix (bypass > cli > settings; first-valid), killswitch → `(Default, Some(notice))`, unknown `--permission-mode` → Default, no inputs → `(Default, None)`, `permission_mode_from_cli_string` for all 5 valid + invalid. Pure, no env.
- `bypass_guard.rs`: a fake `BypassEnv` driving the matrix — root+non-sandbox → Err; root+IS_SANDBOX → Ok; root+bubblewrap → Ok; non-root → Ok (skips check 1); ant+not-sandboxed → Err; ant+sandboxed+no-internet → Ok; ant+sandboxed+internet → Err; non-ant → check 2 skipped; byte-exact messages asserted.
- `argv.rs`: both flags parse; `--permission-mode plan` captured.
- engine-desktop: `DesktopConfig.permission_mode` reaches `BuiltinToolContext`; under enforcement, a `BypassPermissions` cfg feeds the policy (allow-all unless killswitch) — assert via the existing policy test seam.
- TUI `startup_bypass.rs`: `handle_key` accept/decline outcomes; render-string exact-match; gate predicate (skipDangerous already set → dialog skipped).
- Gates: `cargo test -p permission -p cli -p engine -p tui` (touched areas) + the telemetry count-sweep RUN set (telemetry/orchestrator-diagnostics/tui vim+behavior_palette/test-harness parity_tengu_events) for the +1 event; `clippy -D warnings` on touched crates; `cargo test --workspace --no-run` struct-trap; both engines build; `cargo tree -p engine-mobile | grep -c` for the new code = 0; `git diff main -- traits protocol` empty.

## Out of scope (documented follow-ups)

- Shift+Tab mid-session permission-mode cycling in the TUI (`next_permission_mode` exists but is unwired) — a separate TUI batch.
- Statsig bypass-disable gate + its org-policy notice string (ant/Statsig substrate).
- `auto`/TRANSCRIPT_CLASSIFIER mode + `CLAUDE_CODE_REMOTE` filtering (ant-only / no CCR substrate).
- Mid-session bypass-killswitch re-check (`checkAndDisableBypassPermissionsIfNeeded`) — startup resolution only here.
- Making `LINGXI_ENFORCE_PERMISSIONS` default-on (the broader "turn on the enforcement engine" decision) — explicitly NOT part of this batch.
