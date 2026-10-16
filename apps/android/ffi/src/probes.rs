#[cfg(feature = "uniffi")]
use super::callbacks::AndroidGitCredentialProvider;
#[cfg(all(feature = "uniffi", target_os = "android"))]
use super::callbacks::AndroidGitCredentialProviderBridge;
#[cfg(all(feature = "uniffi", any(target_os = "android", test)))]
use lingxi_core::types::utf16_json::Utf16JsonProjection;

#[cfg(all(feature = "uniffi", any(target_os = "android", test)))]
fn git_probe_result_json(data: &Utf16JsonProjection) -> String {
    data.to_json_string().unwrap_or_else(|error| {
        serde_json::json!({ "error": format!("serialize: {error}") }).to_string()
    })
}

/// P4d acceptance probe: drive the REAL [`tool_git_mobile::GitTool`] path for one
/// structured git operation, and return the `ToolCallResult` data (or the error)
/// as JSON. This is the on-device proof that libgit2's OpenSSL TLS + the Android
/// system cacerts (`/system/etc/security/cacerts`) verify a real HTTPS clone —
/// the one thing the host `file://` tests (P4b/c) cannot exercise.
///
/// It is NOT a bypass: it builds a [`tool_api::BuiltinToolContext`] with
/// `mobile_git = Some(MobileGitToolCtx { enabled: true, has_token: false,
/// workspace_root })` + `mobile_git_secret = Some(MobileGitSecret {
/// credential_provider: None, ca_dir: Some(ca_cert_dir), .. })`, constructs the
/// `GitTool`, parses
/// `operation_json` into a source-preserving JSON projection, and runs
/// `GitTool::call(..)` on a transient current-thread runtime — exactly the engine's path. No token is
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
        use tool_api::{MobileGitSecret, MobileGitToolCtx};

        // Parse the structured operation input. A malformed payload is a probe
        // error, not a tool failure.
        let input = match Utf16JsonProjection::parse(&operation_json) {
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
        // handles are inert here; only `mobile_git` + `mobile_git_secret`
        // matter. No token (public repo); the CA dir points at the system store.
        let mut ctx = shell_test_ctx(ProcessOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        });
        ctx.mobile_git = Some(MobileGitToolCtx {
            enabled: true,
            has_token: false,
            workspace_root: workspace,
        });
        ctx.mobile_git_secret = Some(MobileGitSecret {
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
            Ok(result) => git_probe_result_json(&result.data),
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
        use tool_api::{MobileGitSecret, MobileGitToolCtx};

        let input = match Utf16JsonProjection::parse(&operation_json) {
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
        ctx.mobile_git = Some(MobileGitToolCtx {
            enabled: true,
            has_token: provider.is_some(),
            workspace_root: workspace,
        });
        ctx.mobile_git_secret = Some(MobileGitSecret {
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
            Ok(result) => git_probe_result_json(&result.data),
            Err(e) => format!(
                "{{\"error\":\"{}\"}}",
                e.to_string().replace('"', "'").replace('\n', " ")
            ),
        }
    }
}

#[cfg(all(test, feature = "uniffi"))]
mod projection_result_tests {
    use super::{git_probe_result_json, Utf16JsonProjection};

    #[test]
    fn git_probe_returns_the_documented_json_object() {
        let data =
            Utf16JsonProjection::parse(r#"{"ok":true,"head":"abc123","files":["README.md"]}"#)
                .unwrap();
        let decoded: serde_json::Value =
            serde_json::from_str(&git_probe_result_json(&data)).unwrap();
        assert_eq!(
            decoded,
            serde_json::json!({"ok":true,"head":"abc123","files":["README.md"]})
        );
    }

    #[test]
    fn git_probe_preserves_unpaired_utf16_in_json_output() {
        let data = Utf16JsonProjection::parse(r#"{"path":"\ud800"}"#).unwrap();
        let json = git_probe_result_json(&data).to_ascii_lowercase();
        assert_eq!(json, r#"{"path":"\ud800"}"#);
    }
}
