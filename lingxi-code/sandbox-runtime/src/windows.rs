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
}
