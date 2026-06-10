//! The Windows sandbox backend. Ported 1:1 from
//! `@anthropic-ai/sandbox-runtime@0.0.54`.
//!
//! Reference of truth (source-line cites throughout):
//! `docs/superpowers/references/sandbox-runtime-0.0.54/dist/sandbox/windows-sandbox-utils.js`.
//!
//! Network isolation is enforced by `srt-win.exe` — a separate vendored Rust
//! helper (out of scope here, like the macOS/Linux helper binaries) that manages
//! a local discriminator group, a machine-wide WFP filter set keyed on that
//! group's SID, and an `exec` subcommand that spawns the target under a
//! restricted token inside a hardened job. This module is a thin wrapper around
//! the `srt-win` CLI: it resolves the binary, builds the `srt-win exec` argv +
//! env, and (on Windows) shells out for the group/WFP status queries.
//!
//! The pure argv/env/path-resolution logic ([`get_srt_win_path`],
//! [`group_ref_args`], [`wrap_command_with_sandbox_windows`], the status-query
//! argv builders, and the JSON-result parsers) is portable and unit-tested on
//! every host. The `srt-win` subprocess invocation + the live JSON parse in
//! [`check_windows_dependencies`] are gated to `target_os = "windows"`; on other
//! hosts [`check_windows_dependencies`] returns a "Windows-only" error.
//!
//! # Divergences from the TS (documented, intentional)
//!
//! - **Windows path literals are built as byte-faithful strings, not via
//!   `std::path`.** On a macOS/Linux host `std::path::Path::join` uses `/` as the
//!   separator, so it cannot reproduce `C:\Windows\System32\cmd.exe`. The TS
//!   `path.join(...)` runs on Windows where the separator IS `\`. To keep
//!   [`wrap_command_with_sandbox_windows`] byte-faithful when unit-tested on
//!   macOS, the `cmd.exe` / `powershell.exe` paths are assembled with explicit
//!   `\` literals (string concatenation), not `PathBuf`. This matches the bytes
//!   the TS produces on Windows.
//! - **`wrap_command_with_sandbox_windows` returns the GENERATED proxy env only**
//!   (with `TMPDIR` removed), NOT `{...process.env, ...generated}`. The TS merges
//!   the generated vars over `process.env` so the spawned child inherits the
//!   broker's environment plus the proxy overrides
//!   (`windows-sandbox-utils.js:328`). This port keeps the builder pure
//!   (no `std::env::vars()` read), returning only the generated overrides; the
//!   caller is responsible for merging them over the broker environment when it
//!   spawns. The override SEMANTICS (generated wins) are preserved by the caller
//!   applying these last. Flagged.

use std::path::{Path, PathBuf};

use crate::env::{generate_proxy_env_vars, Platform};

// ────────────────────────────────────────────────────────────────────
// Consts
// ────────────────────────────────────────────────────────────────────

/// Default discriminator group name (`windows-sandbox-utils.js:27`). Used when
/// no `group_sid` and no `group_name` override are supplied.
pub const DEFAULT_WINDOWS_GROUP_NAME: &str = "sandbox-runtime-net";

/// Default inclusive `[low, high]` proxy port range
/// (`windows-sandbox-utils.js:28-30`).
pub const DEFAULT_WINDOWS_PROXY_PORT_RANGE: (u16, u16) = (60080, 60089);

// ────────────────────────────────────────────────────────────────────
// Error
// ────────────────────────────────────────────────────────────────────

/// A Windows-backend error carrying the (TS-faithful) message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsError(pub String);

impl std::fmt::Display for WindowsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for WindowsError {}

/// Shorthand result for this module's fallible operations.
pub type WindowsResult<T> = Result<T, WindowsError>;

// ────────────────────────────────────────────────────────────────────
// Types
// ────────────────────────────────────────────────────────────────────

/// A reference to the discriminator group: either by SID (preferred) or by name
/// (`groupRef` in the TS — `windows-sandbox-utils.js:72-76`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WindowsGroupRef {
    /// Group name override. When unset (and `group_sid` is unset) the default
    /// [`DEFAULT_WINDOWS_GROUP_NAME`] is used.
    pub group_name: Option<String>,
    /// Group SID. When set, it takes precedence over `group_name`.
    pub group_sid: Option<String>,
}

/// Parameters for [`wrap_command_with_sandbox_windows`]
/// (`windows-sandbox-utils.js:303`).
#[derive(Debug, Clone)]
pub struct WindowsWrapParams {
    /// The command string to run inside the sandbox (lands as a single argv
    /// element passed through to the inner shell).
    pub command: String,
    /// The discriminator group reference for `srt-win exec`.
    pub group: WindowsGroupRef,
    /// HTTP proxy port for the generated proxy env vars.
    pub http_proxy_port: Option<u16>,
    /// SOCKS proxy port for the generated proxy env vars.
    pub socks_proxy_port: Option<u16>,
    /// Inner shell selector (`binShell`); defaults to `cmd` when `None`.
    /// `pwsh` → `pwsh.exe`; `*powershell*` → Windows `PowerShell` 1.0; else cmd.
    pub bin_shell: Option<String>,
    /// `SystemRoot` (defaults to `C:\Windows` when `None`) — sourced from the
    /// process env by the thin wrapper.
    pub system_root: Option<String>,
    /// The resolved `srt-win.exe` path (sourced via [`get_srt_win_path`] by the
    /// thin wrapper; a param here to keep this builder pure/portable).
    pub srt_win_path: String,
}

/// The spawn descriptor returned by [`wrap_command_with_sandbox_windows`]: the
/// argv plus the generated proxy env overrides (`{argv, env}` in the TS).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsInvocation {
    /// The full argv: `[srt_win, "exec", ...group_ref, "--", <shell argv>]`.
    pub argv: Vec<String>,
    /// The generated proxy env vars (ordered `(key, value)` pairs), with
    /// `TMPDIR` removed. The caller merges these OVER the broker environment
    /// when spawning (see the module-level divergence note).
    pub env: Vec<(String, String)>,
}

// ────────────────────────────────────────────────────────────────────
// Binary resolution
// ────────────────────────────────────────────────────────────────────

/// Locate `srt-win.exe` (`getSrtWinPath`, `windows-sandbox-utils.js:51-68`).
/// Resolution order:
///   1. `SRT_WIN_PATH` env var, if it points at an existing file.
///   2. `<repo>/vendor/srt-win/target/release/srt-win.exe`.
///   3. `<repo>/dist/vendor/srt-win/target/release/srt-win.exe`.
///
/// `env_srt_win_path` is the value of `SRT_WIN_PATH` (`None` if unset);
/// `repo_root` is the resolved repo root. Both are params to keep the function
/// pure/testable; a thin wrapper reads the real env + derives the root.
///
/// `exists` is the predicate used to test candidate paths (so tests can inject a
/// fake filesystem); production passes `Path::exists`.
///
/// # Errors
/// Returns a [`WindowsError`] with the exact `srt-win.exe not found…` message
/// (listing the looked-in paths) when no candidate exists.
pub fn get_srt_win_path_with(
    env_srt_win_path: Option<&str>,
    repo_root: &Path,
    exists: impl Fn(&Path) -> bool,
) -> WindowsResult<PathBuf> {
    if let Some(env_path) = env_srt_win_path {
        if !env_path.is_empty() && exists(Path::new(env_path)) {
            return Ok(PathBuf::from(env_path));
        }
    }
    let candidates = [
        repo_root.join("vendor").join("srt-win").join("target").join("release").join("srt-win.exe"),
        repo_root
            .join("dist")
            .join("vendor")
            .join("srt-win")
            .join("target")
            .join("release")
            .join("srt-win.exe"),
    ];
    for c in &candidates {
        if exists(c) {
            return Ok(c.clone());
        }
    }
    // TS: `Looked in: ${[envPath, ...candidates].filter(Boolean).join(', ')}` —
    // envPath is included only when it was set (truthy), candidates always.
    let mut looked: Vec<String> = Vec::new();
    if let Some(env_path) = env_srt_win_path {
        if !env_path.is_empty() {
            looked.push(env_path.to_string());
        }
    }
    for c in &candidates {
        looked.push(c.display().to_string());
    }
    Err(WindowsError(format!(
        "srt-win.exe not found. Set SRT_WIN_PATH or build with \
         `cargo build --release --manifest-path vendor/srt-win/Cargo.toml`. \
         Looked in: {}",
        looked.join(", ")
    )))
}

/// Thin wrapper: read `SRT_WIN_PATH` from the real env and resolve `srt-win.exe`
/// under `repo_root` using `Path::exists`. See [`get_srt_win_path_with`].
///
/// # Errors
/// Propagates the [`WindowsError`] from [`get_srt_win_path_with`].
pub fn get_srt_win_path(repo_root: &Path) -> WindowsResult<PathBuf> {
    let env_path = std::env::var("SRT_WIN_PATH").ok();
    get_srt_win_path_with(env_path.as_deref(), repo_root, Path::exists)
}

// ────────────────────────────────────────────────────────────────────
// Group-ref argv
// ────────────────────────────────────────────────────────────────────

/// Build the `--group-sid <sid>` / `--name <name>` argv fragment
/// (`groupRefArgs`, `windows-sandbox-utils.js:72-76`). A set `group_sid` wins;
/// otherwise the `group_name` override or [`DEFAULT_WINDOWS_GROUP_NAME`].
#[must_use]
pub fn group_ref_args(group: &WindowsGroupRef) -> Vec<String> {
    if let Some(sid) = &group.group_sid {
        return vec!["--group-sid".to_string(), sid.clone()];
    }
    vec![
        "--name".to_string(),
        group
            .group_name
            .clone()
            .unwrap_or_else(|| DEFAULT_WINDOWS_GROUP_NAME.to_string()),
    ]
}

// ────────────────────────────────────────────────────────────────────
// Wrap
// ────────────────────────────────────────────────────────────────────

/// Build the spawn descriptor for running `command` inside the Windows sandbox
/// (`wrapCommandWithSandboxWindows`, `windows-sandbox-utils.js:303-330`):
/// `argv = [srt_win, "exec", ...group_ref, "--", <inner shell argv>]`.
///
/// Shell dispatch on `bin_shell.to_lowercase()` (default `cmd`):
///   - `pwsh` → `["pwsh.exe", "-NoProfile", "-Command", command]`
///   - `*powershell*` →
///     `[<SystemRoot>\System32\WindowsPowerShell\v1.0\powershell.exe,
///       "-NoProfile", "-Command", command]`
///   - else (cmd) →
///     `[<SystemRoot>\System32\cmd.exe, "/d", "/s", "/c", command]`
///
/// `SystemRoot` defaults to `C:\Windows`. The Windows path literals are built
/// with explicit `\` so they are byte-faithful even when this runs on macOS (see
/// the module-level divergence note).
///
/// `env` is `generate_proxy_env_vars(http, socks, None, Platform::Windows, "")`
/// with the `TMPDIR` pair removed (TS `delete generated.TMPDIR`). `Platform::Windows`
/// skips the `GIT_SSH_COMMAND` branch (matching `getPlatform()==='windows'`).
#[must_use]
pub fn wrap_command_with_sandbox_windows(p: &WindowsWrapParams) -> WindowsInvocation {
    let mut argv = vec![p.srt_win_path.clone(), "exec".to_string()];
    argv.extend(group_ref_args(&p.group));
    argv.push("--".to_string());

    let system_root = p.system_root.as_deref().unwrap_or("C:\\Windows");
    let shell = p.bin_shell.as_deref().unwrap_or("cmd").to_lowercase();

    if shell == "pwsh" || shell.contains("powershell") {
        let ps_exe = if shell == "pwsh" {
            "pwsh.exe".to_string()
        } else {
            // Byte-faithful Windows path (explicit backslashes).
            format!("{system_root}\\System32\\WindowsPowerShell\\v1.0\\powershell.exe")
        };
        argv.push(ps_exe);
        argv.push("-NoProfile".to_string());
        argv.push("-Command".to_string());
        argv.push(p.command.clone());
    } else {
        // cmd /d (no AutoRun) /s (strip first+last quote) /c (run-then-exit).
        argv.push(format!("{system_root}\\System32\\cmd.exe"));
        argv.push("/d".to_string());
        argv.push("/s".to_string());
        argv.push("/c".to_string());
        argv.push(p.command.clone());
    }

    // Generated proxy vars, TMPDIR removed (the POSIX tmp path serves no purpose
    // on Windows and breaks msys2 tools). `tmpdir` is passed empty because the
    // TMPDIR pair is immediately stripped.
    let mut env =
        generate_proxy_env_vars(p.http_proxy_port, p.socks_proxy_port, None, Platform::Windows, "");
    env.retain(|(k, _)| k != "TMPDIR");

    WindowsInvocation { argv, env }
}

// ────────────────────────────────────────────────────────────────────
// Status / dependency queries
// ────────────────────────────────────────────────────────────────────

/// Parsed `srt-win group status` JSON (`getWindowsGroupStatus`,
/// `windows-sandbox-utils.js:114-116`). `state` is `ready` /
/// `created-not-on-token` / `absent` etc.; `sid`/`warning` are optional.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct WindowsGroupStatus {
    /// Group state (`ready`, `created-not-on-token`, `absent`, …).
    pub state: String,
    /// The group SID, when known.
    #[serde(default)]
    pub sid: Option<String>,
    /// An optional warning surfaced to the caller.
    #[serde(default)]
    pub warning: Option<String>,
}

/// Parsed `srt-win wfp status` JSON (`getWindowsWfpStatus`,
/// `windows-sandbox-utils.js:123-133`). `port_range` (TS `portRange`) is
/// optional.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct WindowsWfpStatus {
    /// WFP state (`installed`, `absent`, …).
    pub state: String,
    /// Number of srt-win-tagged filters found under the sublayer.
    #[serde(default)]
    pub filters: Option<u32>,
    /// The configured inclusive `[low, high]` proxy port range, when present.
    #[serde(default)]
    pub port_range: Option<(u16, u16)>,
}

/// Build the argv for the `srt-win group status` query
/// (`windows-sandbox-utils.js:115`): `["group", "status", ...group_ref]`.
#[must_use]
pub fn group_status_args(ref_: &WindowsGroupRef) -> Vec<String> {
    let mut args = vec!["group".to_string(), "status".to_string()];
    args.extend(group_ref_args(ref_));
    args
}

/// Build the argv for the `srt-win wfp status` query
/// (`windows-sandbox-utils.js:124-126`): `["wfp", "status"]` plus
/// `--sublayer-guid <guid>` when supplied.
#[must_use]
pub fn wfp_status_args(sublayer_guid: Option<&str>) -> Vec<String> {
    let mut args = vec!["wfp".to_string(), "status".to_string()];
    if let Some(g) = sublayer_guid {
        args.push("--sublayer-guid".to_string());
        args.push(g.to_string());
    }
    args
}

/// Parse `srt-win group status` stdout (one JSON line) into a
/// [`WindowsGroupStatus`].
///
/// # Errors
/// Returns a [`WindowsError`] when the stdout is not parseable JSON.
pub fn parse_group_status(stdout: &str) -> WindowsResult<WindowsGroupStatus> {
    serde_json::from_str(stdout.trim()).map_err(|e| {
        WindowsError(format!(
            "srt-win group status: unparseable JSON output {:?}: {e}",
            stdout.trim()
        ))
    })
}

/// Parse `srt-win wfp status` stdout (one JSON line) into a [`WindowsWfpStatus`].
///
/// # Errors
/// Returns a [`WindowsError`] when the stdout is not parseable JSON.
pub fn parse_wfp_status(stdout: &str) -> WindowsResult<WindowsWfpStatus> {
    serde_json::from_str(stdout.trim()).map_err(|e| {
        WindowsError(format!(
            "srt-win wfp status: unparseable JSON output {:?}: {e}",
            stdout.trim()
        ))
    })
}

/// Install instructions surfaced verbatim in dependency-error messages
/// (`windowsInstallInstructions`, `windows-sandbox-utils.js:354-373`).
/// `group_state == "created-not-on-token"` yields the logout-only message.
#[must_use]
pub fn windows_install_instructions(
    ref_: &WindowsGroupRef,
    sublayer_guid: Option<&str>,
    group_state: &str,
) -> String {
    if group_state == "created-not-on-token" {
        return "The discriminator group exists but is not yet in this session's \
                token. LOG OUT and back in to pick up the new group membership \
                (it enters TokenGroups at logon). Network is not disrupted \
                meanwhile — WFP filter-0 PERMITs traffic while the group is absent \
                from your token."
            .to_string();
    }
    let g = if let Some(sid) = &ref_.group_sid {
        format!("--group-sid {sid}")
    } else {
        format!(
            "--name {}",
            ref_.group_name.as_deref().unwrap_or(DEFAULT_WINDOWS_GROUP_NAME)
        )
    };
    let sl = sublayer_guid.map_or(String::new(), |g| format!(" --sublayer-guid {g}"));
    format!(
        "Windows sandbox needs a one-time install (one UAC prompt):\n\
         \u{20}\u{20}npx sandbox-runtime windows-install\n\
         \u{20}\u{20}— or call installWindowsSandbox(), or run \
         `srt-win.exe install {g}{sl}` directly —\n\
         then LOG OUT and back in (the group SID enters TokenGroups at logon).\n\
         Network is not disrupted before the logout: while the group is absent \
         from your token, WFP filter-0 PERMITs all traffic."
    )
}

/// The result of [`check_windows_dependencies`]: blocking `errors` plus
/// informational `warnings` (`{errors, warnings}`,
/// `windows-sandbox-utils.js:378`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WindowsDependencyReport {
    /// Blocking errors (presence blocks `initialize()`).
    pub errors: Vec<String>,
    /// Non-blocking informational warnings.
    pub warnings: Vec<String>,
}

/// The pure dependency-evaluation core (`checkWindowsDependencies` body,
/// `windows-sandbox-utils.js:378-432`) given already-resolved status. Takes the
/// `srt-win` outcomes as inputs so it is portable + unit-testable; the Windows
/// subprocess glue ([`check_windows_dependencies`]) feeds it the live results.
///
/// `srt_win_err` short-circuits with a single binary-resolution error (TS step
/// 1). `group` / `wfp` are the parsed statuses; a `None` represents a failed
/// query whose error message is in `group_err` / `wfp_err`.
#[must_use]
pub fn evaluate_windows_dependencies(
    ref_: &WindowsGroupRef,
    sublayer_guid: Option<&str>,
    srt_win_err: Option<&str>,
    group: Result<&WindowsGroupStatus, &str>,
    wfp: Result<&WindowsWfpStatus, &str>,
) -> WindowsDependencyReport {
    let mut report = WindowsDependencyReport::default();

    // 1. Binary present.
    if let Some(e) = srt_win_err {
        report.errors.push(e.to_string());
        return report;
    }

    // 2. Group ready (exists AND enabled in the caller's token).
    let gs = match group {
        Ok(gs) => gs,
        Err(e) => {
            report.errors.push(format!("srt-win group status failed: {e}"));
            return report;
        }
    };
    if gs.state != "ready" {
        let sid_suffix = gs.sid.as_ref().map_or(String::new(), |s| format!(" (sid={s})"));
        report.errors.push(format!(
            "Discriminator group is {}{}. {}",
            gs.state,
            sid_suffix,
            windows_install_instructions(ref_, sublayer_guid, &gs.state)
        ));
    }
    if let Some(w) = &gs.warning {
        report.warnings.push(w.clone());
    }

    // 3. WFP filters installed under the sublayer.
    let ws = match wfp {
        Ok(ws) => ws,
        Err(e) => {
            report.errors.push(format!("srt-win wfp status failed: {e}"));
            return report;
        }
    };
    if ws.state != "installed" {
        // Only surface a separate WFP error when the group IS ready (otherwise
        // the group error already gave the right instruction).
        if gs.state == "ready" {
            report.errors.push(format!(
                "WFP filters not installed under sublayer {}. {}",
                sublayer_guid.unwrap_or("(default)"),
                windows_install_instructions(ref_, sublayer_guid, "absent")
            ));
        }
    }

    report
}

/// Check the Windows backend is ready to sandbox (`checkWindowsDependencies`,
/// `windows-sandbox-utils.js:378-432`). Resolves the binary, runs the
/// `srt-win group status` + `wfp status` queries, and evaluates them via
/// [`evaluate_windows_dependencies`].
///
/// The `srt-win` subprocess invocation + JSON parse is `target_os = "windows"`
/// only; on other hosts this returns a single "Windows-only" error.
///
/// `repo_root` resolves `srt-win.exe`.
#[must_use]
pub fn check_windows_dependencies(
    ref_: &WindowsGroupRef,
    sublayer_guid: Option<&str>,
    repo_root: &Path,
) -> WindowsDependencyReport {
    #[cfg(target_os = "windows")]
    {
        // 1. Binary present.
        let exe = match get_srt_win_path(repo_root) {
            Ok(p) => p,
            Err(e) => {
                return WindowsDependencyReport { errors: vec![e.0], warnings: Vec::new() };
            }
        };

        // 2 + 3. Live status queries.
        let group = run_srt_win_group_status(&exe, ref_);
        let wfp = run_srt_win_wfp_status(&exe, sublayer_guid);

        evaluate_windows_dependencies(
            ref_,
            sublayer_guid,
            None,
            group.as_ref().map_err(String::as_str),
            wfp.as_ref().map_err(String::as_str),
        )
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (ref_, sublayer_guid, repo_root);
        WindowsDependencyReport {
            errors: vec!["Windows sandbox backend is Windows-only".to_string()],
            warnings: Vec::new(),
        }
    }
}

// ────────────────────────────────────────────────────────────────────
// Windows-only subprocess glue
// ────────────────────────────────────────────────────────────────────

/// Run `srt-win group status` and parse its JSON output.
#[cfg(target_os = "windows")]
fn run_srt_win_group_status(exe: &Path, ref_: &WindowsGroupRef) -> Result<WindowsGroupStatus, String> {
    let out = run_srt_win(exe, &group_status_args(ref_))?;
    parse_group_status(&out).map_err(|e| e.0)
}

/// Run `srt-win wfp status` and parse its JSON output.
#[cfg(target_os = "windows")]
fn run_srt_win_wfp_status(
    exe: &Path,
    sublayer_guid: Option<&str>,
) -> Result<WindowsWfpStatus, String> {
    let out = run_srt_win(exe, &wfp_status_args(sublayer_guid))?;
    parse_wfp_status(&out).map_err(|e| e.0)
}

/// Spawn `srt-win <args>` and return trimmed stdout, erroring on spawn failure
/// or non-zero exit (`runSrtWin`/`runSrtWinJson`, `windows-sandbox-utils.js:77-103`).
#[cfg(target_os = "windows")]
fn run_srt_win(exe: &Path, args: &[String]) -> Result<String, String> {
    use std::process::Command;
    let output = Command::new(exe)
        .args(args)
        .output()
        .map_err(|e| format!("srt-win {}: spawn failed: {e}", args.first().map_or("", |s| s)))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !output.status.success() {
        let code = output.status.code().unwrap_or(-1);
        let detail = if stderr.is_empty() { &stdout } else { &stderr };
        return Err(format!("srt-win {} exited {code}: {detail}", args.join(" ")));
    }
    Ok(stdout)
}

// ────────────────────────────────────────────────────────────────────
// Tests (portable — run on macOS)
// ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn has(v: &[(String, String)], k: &str, val: &str) -> bool {
        v.iter().any(|(kk, vv)| kk == k && vv == val)
    }

    // ── consts ──────────────────────────────────────────────────────

    #[test]
    fn consts_match_ts() {
        assert_eq!(DEFAULT_WINDOWS_GROUP_NAME, "sandbox-runtime-net");
        assert_eq!(DEFAULT_WINDOWS_PROXY_PORT_RANGE, (60080, 60089));
    }

    // ── get_srt_win_path ────────────────────────────────────────────

    #[test]
    fn srt_win_path_prefers_env_when_exists() {
        let repo = Path::new("/repo");
        let p = get_srt_win_path_with(Some("/custom/srt-win.exe"), repo, |path| {
            path == Path::new("/custom/srt-win.exe")
        })
        .expect("env path");
        assert_eq!(p, PathBuf::from("/custom/srt-win.exe"));
    }

    #[test]
    fn srt_win_path_falls_back_to_vendor() {
        let repo = Path::new("/repo");
        let vendor =
            repo.join("vendor").join("srt-win").join("target").join("release").join("srt-win.exe");
        let vendor2 = vendor.clone();
        // env set but missing → skip; first candidate exists.
        let p = get_srt_win_path_with(Some("/missing/srt-win.exe"), repo, move |path| {
            path == vendor2
        })
        .expect("vendor fallback");
        assert_eq!(p, vendor);
    }

    #[test]
    fn srt_win_path_falls_back_to_dist() {
        let repo = Path::new("/repo");
        let dist = repo
            .join("dist")
            .join("vendor")
            .join("srt-win")
            .join("target")
            .join("release")
            .join("srt-win.exe");
        let dist2 = dist.clone();
        let p = get_srt_win_path_with(None, repo, move |path| path == dist2).expect("dist fallback");
        assert_eq!(p, dist);
    }

    #[test]
    fn srt_win_path_errors_with_message_and_paths() {
        let repo = Path::new("/repo");
        let err = get_srt_win_path_with(Some("/set/but/missing.exe"), repo, |_| false)
            .expect_err("none exist");
        let msg = err.0;
        assert!(msg.starts_with("srt-win.exe not found. Set SRT_WIN_PATH or build with"));
        assert!(msg.contains(
            "`cargo build --release --manifest-path vendor/srt-win/Cargo.toml`"
        ));
        // env path (truthy) listed, then both candidates.
        assert!(msg.contains("Looked in: /set/but/missing.exe, "));
        assert!(msg.contains("vendor/srt-win/target/release/srt-win.exe"));
        assert!(msg.contains("dist/vendor/srt-win/target/release/srt-win.exe"));
    }

    #[test]
    fn srt_win_path_error_omits_unset_env() {
        let repo = Path::new("/repo");
        let err = get_srt_win_path_with(None, repo, |_| false).expect_err("none exist");
        // No env path → "Looked in: " starts directly with a candidate path.
        assert!(err.0.contains("Looked in: /repo/vendor/srt-win"));
    }

    // ── group_ref_args ──────────────────────────────────────────────

    #[test]
    fn group_ref_sid_wins() {
        let r = WindowsGroupRef {
            group_name: Some("ignored".into()),
            group_sid: Some("S-1-5-21-x".into()),
        };
        assert_eq!(group_ref_args(&r), vec!["--group-sid", "S-1-5-21-x"]);
    }

    #[test]
    fn group_ref_name_override() {
        let r = WindowsGroupRef { group_name: Some("my-group".into()), group_sid: None };
        assert_eq!(group_ref_args(&r), vec!["--name", "my-group"]);
    }

    #[test]
    fn group_ref_default_name() {
        let r = WindowsGroupRef::default();
        assert_eq!(group_ref_args(&r), vec!["--name", "sandbox-runtime-net"]);
    }

    // ── wrap_command_with_sandbox_windows ───────────────────────────

    fn base_params(bin_shell: Option<&str>) -> WindowsWrapParams {
        WindowsWrapParams {
            command: "echo hi".into(),
            group: WindowsGroupRef::default(),
            http_proxy_port: Some(60080),
            socks_proxy_port: Some(60081),
            bin_shell: bin_shell.map(String::from),
            system_root: None,
            srt_win_path: "C:\\srt-win.exe".into(),
        }
    }

    #[test]
    fn wrap_cmd_default_argv_byte_faithful() {
        let inv = wrap_command_with_sandbox_windows(&base_params(None));
        assert_eq!(
            inv.argv,
            vec![
                "C:\\srt-win.exe",
                "exec",
                "--name",
                "sandbox-runtime-net",
                "--",
                "C:\\Windows\\System32\\cmd.exe",
                "/d",
                "/s",
                "/c",
                "echo hi",
            ]
        );
    }

    #[test]
    fn wrap_powershell_argv_byte_faithful() {
        let inv = wrap_command_with_sandbox_windows(&base_params(Some("powershell")));
        assert_eq!(
            inv.argv,
            vec![
                "C:\\srt-win.exe",
                "exec",
                "--name",
                "sandbox-runtime-net",
                "--",
                "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe",
                "-NoProfile",
                "-Command",
                "echo hi",
            ]
        );
    }

    #[test]
    fn wrap_pwsh_argv_byte_faithful() {
        let inv = wrap_command_with_sandbox_windows(&base_params(Some("pwsh")));
        assert_eq!(
            inv.argv,
            vec![
                "C:\\srt-win.exe",
                "exec",
                "--name",
                "sandbox-runtime-net",
                "--",
                "pwsh.exe",
                "-NoProfile",
                "-Command",
                "echo hi",
            ]
        );
    }

    #[test]
    fn wrap_respects_custom_system_root() {
        let mut p = base_params(None);
        p.system_root = Some("D:\\WinDir".into());
        let inv = wrap_command_with_sandbox_windows(&p);
        assert!(inv.argv.contains(&"D:\\WinDir\\System32\\cmd.exe".to_string()));
    }

    #[test]
    fn wrap_shell_case_insensitive() {
        let inv = wrap_command_with_sandbox_windows(&base_params(Some("PowerShell")));
        assert!(inv
            .argv
            .contains(&"C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe".to_string()));
    }

    #[test]
    fn wrap_env_has_proxy_vars_and_no_tmpdir() {
        let inv = wrap_command_with_sandbox_windows(&base_params(None));
        // TMPDIR removed.
        assert!(!inv.env.iter().any(|(k, _)| k == "TMPDIR"));
        // SANDBOX_RUNTIME and proxy vars present.
        assert!(has(&inv.env, "SANDBOX_RUNTIME", "1"));
        assert!(has(&inv.env, "HTTP_PROXY", "http://localhost:60080"));
        assert!(has(&inv.env, "ALL_PROXY", "socks5h://localhost:60081"));
    }

    #[test]
    fn wrap_env_omits_git_ssh_on_windows() {
        // Platform::Windows fires neither GIT_SSH arm even with a socks port.
        let inv = wrap_command_with_sandbox_windows(&base_params(None));
        assert!(!inv.env.iter().any(|(k, _)| k == "GIT_SSH_COMMAND"));
    }

    #[test]
    fn wrap_group_sid_in_argv() {
        let mut p = base_params(None);
        p.group = WindowsGroupRef { group_name: None, group_sid: Some("S-1-5-32".into()) };
        let inv = wrap_command_with_sandbox_windows(&p);
        assert_eq!(&inv.argv[1..5], &["exec", "--group-sid", "S-1-5-32", "--"]);
    }

    // ── status query argv ───────────────────────────────────────────

    #[test]
    fn group_status_args_shape() {
        let r = WindowsGroupRef::default();
        assert_eq!(group_status_args(&r), vec!["group", "status", "--name", "sandbox-runtime-net"]);
    }

    #[test]
    fn wfp_status_args_with_and_without_guid() {
        assert_eq!(wfp_status_args(None), vec!["wfp", "status"]);
        assert_eq!(
            wfp_status_args(Some("GUID-1")),
            vec!["wfp", "status", "--sublayer-guid", "GUID-1"]
        );
    }

    // ── JSON parse ──────────────────────────────────────────────────

    #[test]
    fn parse_group_status_sample() {
        let gs = parse_group_status(r#"{"state":"ready","sid":"S-1-5-21-9"}"#).expect("parse");
        assert_eq!(gs.state, "ready");
        assert_eq!(gs.sid.as_deref(), Some("S-1-5-21-9"));
        assert!(gs.warning.is_none());
    }

    #[test]
    fn parse_group_status_with_warning() {
        let gs = parse_group_status(
            r#"{"state":"created-not-on-token","warning":"log out needed"}"#,
        )
        .expect("parse");
        assert_eq!(gs.state, "created-not-on-token");
        assert_eq!(gs.warning.as_deref(), Some("log out needed"));
    }

    #[test]
    fn parse_group_status_bad_json_errors() {
        let err = parse_group_status("not json").expect_err("bad");
        assert!(err.0.contains("unparseable JSON output"));
    }

    #[test]
    fn parse_wfp_status_sample_with_port_range() {
        let ws = parse_wfp_status(r#"{"state":"installed","filters":3,"port_range":[60080,60089]}"#)
            .expect("parse");
        assert_eq!(ws.state, "installed");
        assert_eq!(ws.filters, Some(3));
        assert_eq!(ws.port_range, Some((60080, 60089)));
    }

    #[test]
    fn parse_wfp_status_minimal() {
        let ws = parse_wfp_status(r#"{"state":"absent"}"#).expect("parse");
        assert_eq!(ws.state, "absent");
        assert!(ws.filters.is_none());
        assert!(ws.port_range.is_none());
    }

    // ── dependency evaluation ───────────────────────────────────────

    #[test]
    fn deps_binary_missing_short_circuits() {
        let r = evaluate_windows_dependencies(
            &WindowsGroupRef::default(),
            None,
            Some("srt-win.exe not found"),
            Err("unused"),
            Err("unused"),
        );
        assert_eq!(r.errors, vec!["srt-win.exe not found"]);
        assert!(r.warnings.is_empty());
    }

    #[test]
    fn deps_all_ready_no_errors() {
        let gs = WindowsGroupStatus { state: "ready".into(), sid: None, warning: None };
        let ws = WindowsWfpStatus {
            state: "installed".into(),
            filters: Some(2),
            port_range: Some((60080, 60089)),
        };
        let r = evaluate_windows_dependencies(
            &WindowsGroupRef::default(),
            None,
            None,
            Ok(&gs),
            Ok(&ws),
        );
        assert!(r.errors.is_empty(), "{:?}", r.errors);
    }

    #[test]
    fn deps_group_not_ready_emits_instructions() {
        let gs = WindowsGroupStatus {
            state: "created-not-on-token".into(),
            sid: Some("S-1-5-21-9".into()),
            warning: None,
        };
        let ws = WindowsWfpStatus { state: "absent".into(), filters: None, port_range: None };
        let r = evaluate_windows_dependencies(
            &WindowsGroupRef::default(),
            None,
            None,
            Ok(&gs),
            Ok(&ws),
        );
        assert_eq!(r.errors.len(), 1, "{:?}", r.errors);
        assert!(r.errors[0].contains("Discriminator group is created-not-on-token"));
        assert!(r.errors[0].contains("(sid=S-1-5-21-9)"));
        assert!(r.errors[0].contains("LOG OUT and back in"));
        // WFP error suppressed because group isn't ready.
        assert!(!r.errors[0].contains("WFP filters not installed"));
    }

    #[test]
    fn deps_group_ready_but_wfp_absent_emits_wfp_error() {
        let gs = WindowsGroupStatus { state: "ready".into(), sid: None, warning: None };
        let ws = WindowsWfpStatus { state: "absent".into(), filters: None, port_range: None };
        let r = evaluate_windows_dependencies(
            &WindowsGroupRef::default(),
            Some("GUID-7"),
            None,
            Ok(&gs),
            Ok(&ws),
        );
        assert_eq!(r.errors.len(), 1, "{:?}", r.errors);
        assert!(r.errors[0].contains("WFP filters not installed under sublayer GUID-7"));
    }

    #[test]
    fn deps_group_query_failure() {
        let r = evaluate_windows_dependencies(
            &WindowsGroupRef::default(),
            None,
            None,
            Err("exit 1"),
            Err("unused"),
        );
        assert_eq!(r.errors, vec!["srt-win group status failed: exit 1"]);
    }

    #[test]
    fn deps_warning_propagated() {
        let gs = WindowsGroupStatus {
            state: "ready".into(),
            sid: None,
            warning: Some("heads up".into()),
        };
        let ws = WindowsWfpStatus {
            state: "installed".into(),
            filters: Some(2),
            port_range: None,
        };
        let r = evaluate_windows_dependencies(
            &WindowsGroupRef::default(),
            None,
            None,
            Ok(&gs),
            Ok(&ws),
        );
        assert!(r.errors.is_empty());
        assert_eq!(r.warnings, vec!["heads up"]);
    }

    #[test]
    fn non_windows_check_returns_windows_only() {
        // On this macOS host the cfg(not(windows)) branch runs.
        #[cfg(not(target_os = "windows"))]
        {
            let r = check_windows_dependencies(&WindowsGroupRef::default(), None, Path::new("/repo"));
            assert_eq!(r.errors, vec!["Windows sandbox backend is Windows-only"]);
        }
    }
}
