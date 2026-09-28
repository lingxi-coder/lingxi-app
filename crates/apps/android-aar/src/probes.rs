#[cfg(feature = "uniffi")]
use super::callbacks::AndroidGitCredentialProvider;
#[cfg(all(feature = "uniffi", target_os = "android"))]
use super::callbacks::AndroidGitCredentialProviderBridge;
#[cfg(target_os = "android")]
use super::host::bootstrap_bundled_shell;
#[cfg(all(feature = "uniffi", target_os = "android"))]
use std::sync::Arc;

/// P0a gate probe: run the on-device minijail smoke and return it as JSON
/// (`{"ok":bool,"no_new_privs":bool,"child_exit_zero":bool,"reason":...}`).
/// Keys are serde's default `snake_case` (`SmokeResult` has no `rename_all`).
/// Host builds report the structural reason.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
#[must_use]
pub fn android_sandbox_smoke() -> String {
    #[cfg(target_os = "android")]
    {
        serde_json::to_string(&platform_android_minijail::minijail_smoke())
            .unwrap_or_else(|e| format!("{{\"ok\":false,\"reason\":\"serialize: {e}\"}}"))
    }
    #[cfg(not(target_os = "android"))]
    {
        "{\"ok\":false,\"reason\":\"host build\"}".to_string()
    }
}

/// P2 acceptance probe: exercise the REAL prepare→runner→`run_jailed` path for
/// one `command` rooted at `workspace`, and return the outcome as JSON
/// (`{"stdout","stderr","exit_code","timed_out","enforcement_failed"}`).
///
/// This is NOT a bypass: it constructs a probed [`CapabilityCache`], an
/// [`AndroidMinijailSandbox`] + [`AndroidMinijailProcessRunner`] over that SAME
/// cache, `prepare()`s the command under a deny-net policy, and `run()`s it
/// through the same jailed fork/exec the engine uses. The wall-clock timeout is
/// hardcoded to **2 seconds** so a `sleep 10` probe reliably trips the watchdog
/// (`timed_out=true`) without making the instrumentation test slow.
///
/// `enforcement_failed` is `null` on success; on the host build (no Android
/// device) it is `"host build"` and the rest are empty/zero — so a JVM-host run
/// fails loudly rather than silently passing.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
#[allow(clippy::needless_pass_by_value)] // FFI export: UniFFI marshals owned `String`.
#[must_use]
pub fn android_sandbox_run_probe(command: String, workspace: String) -> String {
    #[cfg(not(target_os = "android"))]
    {
        let _ = (command, workspace);
        "{\"enforcement_failed\":\"host build\"}".to_string()
    }
    #[cfg(target_os = "android")]
    {
        use mobile_linux_api::{NetworkPolicy, ResourceLimits};
        use platform_android::{
            capabilities::{probe_android_capabilities, CapabilityCache},
            AndroidMinijailProcessRunner, AndroidMinijailSandbox, AndroidShellConfig,
        };
        use platform_api::{ProcessCommand, ProcessRunner, Sandbox, SandboxPolicy};
        use std::collections::HashMap;

        // The probe runtime: capability probe + the jailed run are independent
        // of the engine runtime, so spin up a transient current-thread runtime
        // and drop it (mirrors `build_android_engine`'s eager-probe seam).
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                return format!("{{\"enforcement_failed\":\"probe runtime build: {e}\"}}");
            }
        };

        // Probe REAL capabilities and share ONE cache across sandbox + runner —
        // exactly as `AndroidPlatform::new` does.
        let cache = Arc::new(CapabilityCache::new());
        cache.set(rt.block_on(probe_android_capabilities()));

        let ws = std::path::PathBuf::from(&workspace);
        let cfg = AndroidShellConfig {
            native_library_dir: ws.join("native-lib"),
            shell_workspace_root: ws.clone(),
            app_cache_root: ws.join("cache"),
            package_name: "com.lingxi.code".to_string(),
            package_version_code: 1,
            app_writable_roots: vec![ws.clone()],
            enable_shell: true,
            secrets_in_keystore: true,
            shell_data_exposure_accepted: true,
            bundled_mksh_path: None,
            bundled_mksh_hash: None,
            bundled_applet_dir: None,
        };
        let sandbox = AndroidMinijailSandbox::new(cfg, cache.clone());
        let runner = AndroidMinijailProcessRunner::new(cache);

        // Deny-net policy (the P2 acceptance default). 2s wall-clock timeout so
        // `sleep 10` trips the watchdog.
        // Request NO filesystem confinement: Android's fs boundary is the app
        // UID, not Landlock (shipping kernels disable it), so `plan_from_policy`
        // rejects any non-empty writable/denied path set. Net-deny is the only
        // active confinement here.
        let policy = SandboxPolicy {
            network: NetworkPolicy::Disabled,
            writable_paths: vec![],
            denied_paths: vec![],
            allow_subprocess: true,
            limits: ResourceLimits::default(),
        };
        let proc_cmd = ProcessCommand {
            command: "/system/bin/sh".to_string(),
            args: vec!["-c".to_string(), command],
            cwd: Some(ws),
            env: HashMap::new(),
            timeout: Some(std::time::Duration::from_secs(2)),
            stdin: None,
        };

        let prepared = match sandbox.prepare(proc_cmd, &policy) {
            Ok(p) => p,
            Err(e) => {
                return format!(
                    "{{\"enforcement_failed\":\"prepare: {}\"}}",
                    e.to_string().replace('"', "'")
                );
            }
        };
        match rt.block_on(runner.run(&prepared)) {
            Ok(out) => serde_json::json!({
                "stdout": out.stdout,
                "stderr": out.stderr,
                "exit_code": out.exit_code,
                "timed_out": out.timed_out,
                "enforcement_failed": serde_json::Value::Null,
            })
            .to_string(),
            Err(e) => format!(
                "{{\"enforcement_failed\":\"run: {}\"}}",
                e.to_string().replace('"', "'")
            ),
        }
    }
}

/// P5c acceptance probe: exercise the REAL prepare→runner→`run_jailed` path for
/// one `command` running through the BUNDLED mksh+toybox (not the device's
/// system sh), and return the outcome as JSON
/// (`{"stdout","stderr","exit_code","timed_out","enforcement_failed"}`).
///
/// This is the on-device end-to-end proof of the P5 bundled-shell chain. It
/// mirrors [`android_sandbox_run_probe`] but first bootstraps the bundled shell
/// via [`bootstrap_bundled_shell`] (stage the toybox applet symlink farm, verify
/// bundled execve + dispatch, sha256 `libmksh.so`), then sets the THREE bundled
/// `AndroidShellConfig` fields from the result. Because those fields are set,
/// `prepare()` selects `ExecTarget::BundledHelper{mksh}` and leads PATH with the
/// applet farm, and the runner content-identity-checks the recorded sha256
/// against the real `libmksh.so` before execve (spec P5b §T4b). So a non-empty
/// `stdout` from a bundled command implicitly proves staging + the hash check +
/// the jailed bundled exec all passed end-to-end.
///
/// The wall-clock timeout is hardcoded to **2 seconds** so a `sleep 10` probe
/// reliably trips the watchdog (`timed_out=true`). The deny-net `SandboxPolicy`
/// is the same one P2 proved (`net_deny_verified`).
///
/// `enforcement_failed` is `null` on success; on the host build (no Android
/// device) it is `"host build"`, and `"bundled bootstrap failed"` if
/// `bootstrap_bundled_shell` returns `None` — so a JVM-host run or a broken
/// bundle fails loudly rather than silently passing.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
#[allow(clippy::needless_pass_by_value)] // FFI export: UniFFI marshals owned `String`.
#[must_use]
pub fn android_bundled_shell_run_probe(
    native_lib_dir: String,
    app_files_root: String,
    command: String,
) -> String {
    #[cfg(not(target_os = "android"))]
    {
        let _ = (native_lib_dir, app_files_root, command);
        "{\"enforcement_failed\":\"host build\"}".to_string()
    }
    #[cfg(target_os = "android")]
    {
        use mobile_linux_api::{NetworkPolicy, ResourceLimits};
        use platform_android::{
            capabilities::{probe_android_capabilities, CapabilityCache},
            AndroidMinijailProcessRunner, AndroidMinijailSandbox, AndroidShellConfig,
        };
        use platform_api::{ProcessCommand, ProcessRunner, Sandbox, SandboxPolicy};
        use std::collections::HashMap;

        // Bootstrap the bundled shell: stage the applet symlink farm, verify
        // bundled execve + dispatch, sha256 libmksh.so. `None` = fail-closed.
        let Some(bundled) = bootstrap_bundled_shell(&native_lib_dir, &app_files_root) else {
            return "{\"enforcement_failed\":\"bundled bootstrap failed\"}".to_string();
        };

        // The probe runtime: capability probe + the jailed run are independent
        // of the engine runtime, so spin up a transient current-thread runtime
        // and drop it (mirrors `android_sandbox_run_probe`).
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                return format!("{{\"enforcement_failed\":\"probe runtime build: {e}\"}}");
            }
        };

        // Probe REAL capabilities and share ONE cache across sandbox + runner.
        let cache = Arc::new(CapabilityCache::new());
        cache.set(rt.block_on(probe_android_capabilities()));

        let root = std::path::PathBuf::from(&app_files_root);
        let cfg = AndroidShellConfig {
            native_library_dir: std::path::PathBuf::from(&native_lib_dir),
            shell_workspace_root: root.clone(),
            app_cache_root: root.join("cache"),
            package_name: "com.lingxi.code".to_string(),
            package_version_code: 1,
            app_writable_roots: vec![root.clone()],
            enable_shell: true,
            secrets_in_keystore: true,
            shell_data_exposure_accepted: true,
            // The three bundled fields drive `prepare` onto BundledHelper{mksh}
            // + the applet-farm PATH, and the runner's content-identity check.
            bundled_mksh_path: Some(bundled.mksh_path),
            bundled_mksh_hash: Some(bundled.mksh_hash),
            bundled_applet_dir: Some(bundled.applet_dir),
        };
        let sandbox = AndroidMinijailSandbox::new(cfg, cache.clone());
        let runner = AndroidMinijailProcessRunner::new(cache);

        // Deny-net policy (the P2 acceptance default, same filter P2 proved via
        // `net_deny_verified`). 2s wall-clock timeout so `sleep 10` trips the
        // watchdog. No filesystem confinement (Android's fs boundary is the app
        // UID, not Landlock); net-deny is the only active confinement here.
        let policy = SandboxPolicy {
            network: NetworkPolicy::Disabled,
            writable_paths: vec![],
            denied_paths: vec![],
            allow_subprocess: true,
            limits: ResourceLimits::default(),
        };
        let proc_cmd = ProcessCommand {
            // `prepare` normalizes the system-sh sentinel onto BundledHelper{mksh}
            // because the bundled fields are set.
            command: "/system/bin/sh".to_string(),
            args: vec!["-c".to_string(), command],
            cwd: Some(root),
            env: HashMap::new(),
            timeout: Some(std::time::Duration::from_secs(2)),
            stdin: None,
        };

        let prepared = match sandbox.prepare(proc_cmd, &policy) {
            Ok(p) => p,
            Err(e) => {
                return format!(
                    "{{\"enforcement_failed\":\"prepare: {}\"}}",
                    e.to_string().replace('"', "'")
                );
            }
        };
        match rt.block_on(runner.run(&prepared)) {
            Ok(out) => serde_json::json!({
                "stdout": out.stdout,
                "stderr": out.stderr,
                "exit_code": out.exit_code,
                "timed_out": out.timed_out,
                "enforcement_failed": serde_json::Value::Null,
            })
            .to_string(),
            Err(e) => format!(
                "{{\"enforcement_failed\":\"run: {}\"}}",
                e.to_string().replace('"', "'")
            ),
        }
    }
}

/// P2 acceptance probe: run the REAL capability probe and return the matrix as
/// JSON (`{"net_deny_verified":bool,"seccomp_filter":bool,...}`). The strong
/// proof of net-deny enforcement is `net_deny_verified` — the probe forked a
/// child under the net-deny seccomp filter and observed `socket()` ⇒ `EPERM`.
/// Host builds report `{"probed":false,"net_deny_verified":false}`.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
#[must_use]
pub fn android_sandbox_capabilities() -> String {
    #[cfg(not(target_os = "android"))]
    {
        "{\"probed\":false,\"net_deny_verified\":false,\"reason\":\"host build\"}".to_string()
    }
    #[cfg(target_os = "android")]
    {
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => return format!("{{\"probed\":false,\"reason\":\"probe runtime: {e}\"}}"),
        };
        let caps = rt.block_on(platform_android::capabilities::probe_android_capabilities());
        serde_json::json!({
            "probed": caps.probed,
            "minijail_smoke": caps.minijail_smoke,
            "no_new_privs": caps.no_new_privs,
            "seccomp_filter": caps.seccomp_filter,
            "seccomp_tsync": caps.seccomp_tsync,
            "net_deny_verified": caps.net_deny_verified,
            "pgid_kill": caps.pgid_kill,
            "landlock_abi": caps.landlock_abi,
            "system_sh_version": caps.system_sh_version,
            "toybox_applets": caps.toybox_applets,
            "reason": caps.reason,
        })
        .to_string()
    }
}

/// P4d acceptance probe: drive the REAL [`tool_git_mobile::GitTool`] path for one
/// structured git operation, and return the `ToolCallResult` data (or the error)
/// as JSON. This is the on-device proof that libgit2's OpenSSL TLS + the Android
/// system cacerts (`/system/etc/security/cacerts`) verify a real HTTPS clone —
/// the one thing the host `file://` tests (P4b/c) cannot exercise.
///
/// It is NOT a bypass: it builds a [`tool_api::BuiltinToolContext`] with
/// `android_git = Some(AndroidGitToolCtx { enabled: true, has_token: false,
/// workspace_root })` + `android_git_secret = Some(AndroidGitSecret {
/// credential_provider: None, ca_dir: Some(ca_cert_dir), .. })`, constructs the
/// `GitTool`, parses
/// `operation_json` into the tool input `Value`, and runs `GitTool::call(..)` on
/// a transient current-thread runtime — exactly the engine's path. No token is
/// needed: the acceptance clone targets a PUBLIC repo.
///
/// - `operation_json` — the structured tool input (e.g.
///   `{"operation":"clone","repo_url":"https://github.com/.../x.git","repo":"cloned"}`).
/// - `workspace` — the app-private workspace root all ops are anchored under.
/// - `ca_cert_dir` — the system CA-certificate directory for TLS verification
///   (the device passes `/system/etc/security/cacerts`).
///
/// Returns the `ToolCallResult.data` JSON on success, or `{"error": "..."}` on a
/// parse / tool error. On the host build (no Android device) it returns
/// `{"error":"host build"}` so a JVM-host run fails loudly rather than silently
/// passing.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
#[allow(clippy::needless_pass_by_value)] // FFI export: UniFFI marshals owned `String`.
#[must_use]
pub fn android_git_probe(operation_json: String, workspace: String, ca_cert_dir: String) -> String {
    #[cfg(not(target_os = "android"))]
    {
        let _ = (operation_json, workspace, ca_cert_dir);
        "{\"error\":\"host build\"}".to_string()
    }
    #[cfg(target_os = "android")]
    {
        use mobile_linux_api::ProcessOutput;
        use tool_api::test_support::{fresh_ctx, fresh_tx, shell_test_ctx};
        use tool_api::tool_trait::Tool;
        use tool_api::{AndroidGitSecret, AndroidGitToolCtx};

        // Parse the structured operation input. A malformed payload is a probe
        // error, not a tool failure.
        let input: serde_json::Value = match serde_json::from_str(&operation_json) {
            Ok(v) => v,
            Err(e) => {
                return format!(
                    "{{\"error\":\"parse operation_json: {}\"}}",
                    e.to_string().replace('"', "'")
                );
            }
        };

        // Build the REAL BuiltinToolContext. The git ops drive libgit2 + the
        // filesystem directly, so the test-support stub fs/process/sandbox
        // handles are inert here; only `android_git` + `android_git_secret`
        // matter. No token (public repo); the CA dir points at the system store.
        let mut ctx = shell_test_ctx(ProcessOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        });
        ctx.android_git = Some(AndroidGitToolCtx {
            enabled: true,
            has_token: false,
            workspace_root: workspace,
        });
        ctx.android_git_secret = Some(AndroidGitSecret {
            credential_provider: None,
            ca_dir: if ca_cert_dir.is_empty() {
                None
            } else {
                Some(ca_cert_dir)
            },
            ssh_private_key_path: None,
            ssh_public_key_path: None,
            ssh_known_hosts_sha256_hex: Vec::new(),
        });

        let tool = tool_git_mobile::GitTool::new(ctx);

        // The clone/local ops are sync inside an async `call`; run on a transient
        // current-thread runtime and drop it (mirrors `android_sandbox_run_probe`).
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => return format!("{{\"error\":\"probe runtime build: {e}\"}}"),
        };
        match rt.block_on(tool.call(input, fresh_ctx(), fresh_tx())) {
            Ok(result) => serde_json::to_string(&result.data)
                .unwrap_or_else(|e| format!("{{\"error\":\"serialize: {e}\"}}")),
            Err(e) => format!(
                "{{\"error\":\"{}\"}}",
                e.to_string().replace('"', "'").replace('\n', " ")
            ),
        }
    }
}

/// Authenticated variant of [`android_git_probe`] for the G2 (push), G7 (SSH),
/// and per-op credential-provider device-acceptance tests. Identical to
/// `android_git_probe` except it wires the per-op secret path:
///
/// - `credential_provider` — a host [`AndroidGitCredentialProvider`] bridged onto
///   `tool_api::GitCredentialProvider`; supplies the HTTPS token / SSH passphrase
///   lazily per network op (so a Keystore-backed provider's round-trip is
///   exercised on real ops).
/// - `ssh_private_key_path` / `ssh_public_key_path` — on-device key paths for an
///   SSH remote (G7). `None` keeps HTTPS behavior.
/// - `ssh_known_hosts_sha256_hex` — the pinned host-key SHA-256 hex set (G7
///   strict verification; an empty set makes any SSH op fail closed).
///
/// Returns the op `data` JSON or `{"error": "..."}`; host build →
/// `{"error":"host build"}` so a JVM-host run fails loudly.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
#[allow(clippy::needless_pass_by_value)] // FFI export: UniFFI marshals owned values.
#[must_use]
pub fn android_git_probe_authed(
    operation_json: String,
    workspace: String,
    ca_cert_dir: String,
    credential_provider: Option<Box<dyn AndroidGitCredentialProvider>>,
    ssh_private_key_path: Option<String>,
    ssh_public_key_path: Option<String>,
    ssh_known_hosts_sha256_hex: Vec<String>,
) -> String {
    #[cfg(not(target_os = "android"))]
    {
        let _ = (
            operation_json,
            workspace,
            ca_cert_dir,
            credential_provider,
            ssh_private_key_path,
            ssh_public_key_path,
            ssh_known_hosts_sha256_hex,
        );
        "{\"error\":\"host build\"}".to_string()
    }
    #[cfg(target_os = "android")]
    {
        use mobile_linux_api::ProcessOutput;
        use tool_api::test_support::{fresh_ctx, fresh_tx, shell_test_ctx};
        use tool_api::tool_trait::Tool;
        use tool_api::{AndroidGitSecret, AndroidGitToolCtx};

        let input: serde_json::Value = match serde_json::from_str(&operation_json) {
            Ok(v) => v,
            Err(e) => {
                return format!(
                    "{{\"error\":\"parse operation_json: {}\"}}",
                    e.to_string().replace('"', "'")
                );
            }
        };

        // Bridge the host provider onto the engine trait (same as build_android_engine).
        let provider: Option<std::sync::Arc<dyn tool_api::GitCredentialProvider>> =
            credential_provider.map(|p| {
                std::sync::Arc::new(AndroidGitCredentialProviderBridge { inner: p })
                    as std::sync::Arc<dyn tool_api::GitCredentialProvider>
            });

        let mut ctx = shell_test_ctx(ProcessOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        });
        ctx.android_git = Some(AndroidGitToolCtx {
            enabled: true,
            has_token: provider.is_some(),
            workspace_root: workspace,
        });
        ctx.android_git_secret = Some(AndroidGitSecret {
            credential_provider: provider,
            ca_dir: if ca_cert_dir.is_empty() {
                None
            } else {
                Some(ca_cert_dir)
            },
            ssh_private_key_path,
            ssh_public_key_path,
            ssh_known_hosts_sha256_hex,
        });

        let tool = tool_git_mobile::GitTool::new(ctx);
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => return format!("{{\"error\":\"probe runtime build: {e}\"}}"),
        };
        match rt.block_on(tool.call(input, fresh_ctx(), fresh_tx())) {
            Ok(result) => serde_json::to_string(&result.data)
                .unwrap_or_else(|e| format!("{{\"error\":\"serialize: {e}\"}}")),
            Err(e) => format!(
                "{{\"error\":\"{}\"}}",
                e.to_string().replace('"', "'").replace('\n', " ")
            ),
        }
    }
}

/// P5a make-or-break gate probe: prove that bundled executables packaged as
/// `lib*.so` under `native_lib_dir` can `execve` under Android 10+ W^X, and
/// decide which toybox applet-resolution mechanism works on-device.
///
/// This is a RAW exec probe — NOT jailed. It only proves the W^X/packaging
/// story (the minijail/deny-net path is unchanged from P2/P3 and proven
/// elsewhere). It returns JSON:
///
/// ```json
/// {"mksh_exec_ok":bool,"applet_symlink_ok":bool,"applet_rewrite_ok":bool,"reason":"..."}
/// ```
///
/// - `mksh_exec_ok`: `<native_lib_dir>/libmksh.so -c 'echo hi'` runs and stdout
///   contains `hi` — proves W^X execve of a bundled executable from
///   nativeLibraryDir works at all.
/// - `applet_symlink_ok`: a symlink `<applet_dir>/grep` → `libtoybox.so`,
///   exec'd as `<applet_dir>/grep foo` over stdin `foo\nbar`, outputs `foo` —
///   proves execve-through-a-symlink-into-nativeLibraryDir + toybox `argv[0]`
///   multicall dispatch under W^X (the PREFERRED applet mechanism for P5b).
/// - `applet_rewrite_ok`: `<native_lib_dir>/libtoybox.so grep foo` over the same
///   stdin outputs `foo` — the command-rewrite FALLBACK mechanism.
///
/// Host builds return `{"error":"host build"}` so a JVM-host run fails loudly
/// rather than silently passing.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
#[allow(clippy::needless_pass_by_value)]
// FFI export: UniFFI marshals owned `String`.
// Single linear probe body (raw mksh exec + symlink-farm + command-rewrite
// applet resolution); the length is intrinsic to the three-mechanism probe, not
// decomposable. Pre-existing P5a debt surfaced by the android-target clippy gate.
#[allow(clippy::too_many_lines)]
#[must_use]
pub fn android_bundled_shell_probe(native_lib_dir: String, applet_dir: String) -> String {
    #[cfg(not(target_os = "android"))]
    {
        let _ = (native_lib_dir, applet_dir);
        "{\"error\":\"host build\"}".to_string()
    }
    #[cfg(target_os = "android")]
    {
        use std::io::Write;
        use std::os::unix::fs::symlink;
        use std::path::Path;
        use std::process::{Command, Stdio};

        let nl = Path::new(&native_lib_dir);
        let mksh = nl.join("libmksh.so");
        let toybox = nl.join("libtoybox.so");
        let mut reason = String::new();

        // (a) RAW mksh execve proof — the make-or-break W^X gate.
        let mksh_exec_ok = match Command::new(&mksh).args(["-c", "echo hi"]).output() {
            Ok(out) => {
                let so = String::from_utf8_lossy(&out.stdout);
                let ok = so.contains("hi");
                if !ok {
                    reason.push_str(&format!(
                        "mksh: status={:?} stdout={:?} stderr={:?}; ",
                        out.status.code(),
                        so,
                        String::from_utf8_lossy(&out.stderr)
                    ));
                }
                ok
            }
            Err(e) => {
                reason.push_str(&format!("mksh spawn: {e}; "));
                false
            }
        };

        // Helper: run a command with stdin "foo\nbar" and assert stdout == "foo".
        let run_grep = |mut cmd: Command, label: &str, reason: &mut String| -> bool {
            cmd.stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            let mut child = match cmd.spawn() {
                Ok(c) => c,
                Err(e) => {
                    reason.push_str(&format!("{label} spawn: {e}; "));
                    return false;
                }
            };
            if let Some(mut sin) = child.stdin.take() {
                let _ = sin.write_all(b"foo\nbar\n");
            }
            match child.wait_with_output() {
                Ok(out) => {
                    let so = String::from_utf8_lossy(&out.stdout);
                    let ok = so.lines().any(|l| l.trim() == "foo");
                    if !ok {
                        reason.push_str(&format!(
                            "{label}: status={:?} stdout={:?} stderr={:?}; ",
                            out.status.code(),
                            so,
                            String::from_utf8_lossy(&out.stderr)
                        ));
                    }
                    ok
                }
                Err(e) => {
                    reason.push_str(&format!("{label} wait: {e}; "));
                    false
                }
            }
        };

        // (b) Symlink-farm applet resolution (PREFERRED).
        let applet_symlink_ok = {
            let dir = Path::new(&applet_dir);
            let link = dir.join("grep");
            let setup_ok = std::fs::create_dir_all(dir)
                .map_err(|e| reason.push_str(&format!("applet_dir mkdir: {e}; ")))
                .is_ok();
            // Refresh the symlink (ignore a pre-existing one from a warm run).
            let _ = std::fs::remove_file(&link);
            if setup_ok {
                match symlink(&toybox, &link) {
                    Ok(()) => {
                        let mut c = Command::new(&link);
                        c.arg("foo");
                        run_grep(c, "applet_symlink", &mut reason)
                    }
                    Err(e) => {
                        reason.push_str(&format!("symlink: {e}; "));
                        false
                    }
                }
            } else {
                false
            }
        };

        // (c) Command-rewrite applet resolution (FALLBACK).
        let applet_rewrite_ok = {
            let mut c = Command::new(&toybox);
            c.args(["grep", "foo"]);
            run_grep(c, "applet_rewrite", &mut reason)
        };

        serde_json::json!({
            "mksh_exec_ok": mksh_exec_ok,
            "applet_symlink_ok": applet_symlink_ok,
            "applet_rewrite_ok": applet_rewrite_ok,
            "reason": if reason.is_empty() { "ok".to_string() } else { reason },
        })
        .to_string()
    }
}
