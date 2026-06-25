//! Startup sandbox gating tests for `engine_desktop::build()`.
//!
//! Faithful to claude-code's startup sandbox flow:
//!   - `isPlatformInEnabledList()` (sandbox-adapter.ts:505) — the
//!     `sandbox.enabledPlatforms` allowlist gate, ported to the pure free fn
//!     [`engine_desktop::platform_in_enabled_list`].
//!   - `isSandboxRequired()` (sandbox-adapter.ts:479) + `getSandboxUnavailableReason()`
//!     (:562) — when the user sets `sandbox.enabled` AND `failIfUnavailable`, an
//!     unavailable sandbox is a HARD build failure
//!     ([`engine_desktop::BuildError::SandboxUnavailable`]); without
//!     `failIfUnavailable` the build degrades to no-sandbox execution but warns.

use std::sync::Arc;

use engine_desktop::{build, BuildError, DesktopConfig};
use sandbox::runtime_config::Platform;

// ── A no-op permission sink (the `use_noop_permission_gate: true` path never
//    actually pushes through it, so it only needs to satisfy the trait). ───────
#[derive(Default)]
struct NoopPermissionSink;

#[async_trait::async_trait]
impl client_adapter::PermissionRequestSink for NoopPermissionSink {
    async fn emit_request(
        &self,
        _request: client_protocol::permission::PermissionRequest,
    ) {
    }
}

/// Pure-unit: the `enabledPlatforms` gate. `None` (unset) ⇒ all supported
/// platforms allowed; `[]` ⇒ none allowed; otherwise the list must contain the
/// current platform. Mirrors claude-code `isPlatformInEnabledList()`.
#[test]
fn enabled_platforms_macos_only_disables_on_linux() {
    // macOS-only list rejects a Linux host.
    assert!(!engine_desktop::platform_in_enabled_list(
        Some(&[Platform::Linux]),
        Platform::Mac
    ));
    // Empty list ⇒ disabled everywhere.
    assert!(!engine_desktop::platform_in_enabled_list(Some(&[]), Platform::Mac));
    // Unset ⇒ all supported platforms allowed.
    assert!(engine_desktop::platform_in_enabled_list(None, Platform::Mac));
    // Matching single-platform list ⇒ allowed.
    assert!(engine_desktop::platform_in_enabled_list(
        Some(&[Platform::Mac]),
        Platform::Mac
    ));
}

/// The platform value that is GUARANTEED NOT to be the current host, so a
/// `sandbox.enabledPlatforms` list of just that value makes `in_enabled_list`
/// deterministically `false` regardless of where the test runs.
fn other_platform_wire() -> &'static str {
    if cfg!(target_os = "macos") {
        "linux"
    } else {
        "macos"
    }
}

/// Build a deterministic, env/argv-free `DesktopConfig` rooted at `cwd`, with a
/// `~/.claude/settings.json` written from `settings_json`.
fn config_with_settings(settings_json: &str) -> (tempfile::TempDir, DesktopConfig) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cwd = tmp.path().to_path_buf();
    let claude_home = cwd.join(".claude");
    std::fs::create_dir_all(&claude_home).expect("mkdir .claude");
    std::fs::write(claude_home.join("settings.json"), settings_json).expect("write settings.json");

    let cfg = DesktopConfig {
        api_base: "https://api.anthropic.com".to_string(),
        api_key: String::new(),
        cwd: cwd.clone(),
        claude_home,
        default_model: "claude-sonnet-4-20250514".to_string(),
        fallback_model: None,
        provider_profiles: None,
        routing: None,
        mcp_paths: vec![cwd.join(".mcp.json")],
        use_noop_permission_gate: true,
        deny_unresolved_ask: false,
        injected_permission_gate: None,
        session_started_as_coordinator: false,
        memory_provider: None,
        permission_mode: permission::PermissionMode::Default,
        connect_prompt: None,
        max_turns: None,
        max_budget_usd: None,
        json_schema: None,
        system_prompt_override: None,
        append_system_prompt: None,
        session_id_override: None,
        disable_slash_commands: false,
        add_dir: Vec::new(),
        cli_mcp_servers: Vec::new(),
        exclude_dynamic_system_prompt_sections: false,
        multi_agent: None,
        explicit_multi_agent: None,
    };
    (tmp, cfg)
}

async fn run_build(cfg: DesktopConfig) -> Result<engine_desktop::DesktopRuntime, BuildError> {
    let output: Arc<dyn traits::OutputStream> =
        Arc::new(orchestrator::test_support::MockOutputStream::new());
    let perm_sink: Arc<dyn client_adapter::PermissionRequestSink> =
        Arc::new(NoopPermissionSink);
    build(cfg, output, perm_sink).await
}

/// `sandbox.enabled` + `failIfUnavailable` + an `enabledPlatforms` list that
/// excludes the current host ⇒ `in_enabled_list == false` reproducibly ⇒ the
/// build is REJECTED with `BuildError::SandboxUnavailable`, and the reason names
/// the `enabledPlatforms` rejection.
#[tokio::test]
async fn fail_if_unavailable_with_missing_deps_rejects_build() {
    let settings = format!(
        r#"{{ "sandbox": {{ "enabled": true, "failIfUnavailable": true, "enabledPlatforms": ["{}"] }} }}"#,
        other_platform_wire()
    );
    let (_tmp, cfg) = config_with_settings(&settings);

    // `DesktopRuntime` is not `Debug`, so match the result directly rather than
    // via `expect_err`.
    match run_build(cfg).await {
        Err(BuildError::SandboxUnavailable(reason)) => {
            assert!(
                reason.contains("is not in sandbox.enabledPlatforms"),
                "reason must name the enabledPlatforms rejection, got: {reason}"
            );
        }
        Err(other) => panic!("expected BuildError::SandboxUnavailable, got {other:?}"),
        Ok(_) => panic!("sandbox required but unavailable must reject the build"),
    }
}

/// Same unavailable condition but WITHOUT `failIfUnavailable` ⇒ the build SUCCEEDS
/// (degrade-not-reject), and the wired sandbox is not available.
#[tokio::test]
async fn fail_if_unavailable_false_degrades_not_rejects() {
    let settings = format!(
        r#"{{ "sandbox": {{ "enabled": true, "enabledPlatforms": ["{}"] }} }}"#,
        other_platform_wire()
    );
    let (_tmp, cfg) = config_with_settings(&settings);

    let rt = run_build(cfg)
        .await
        .expect("without failIfUnavailable the build must degrade, not reject");
    // The sandbox is not available (the current platform is excluded by the
    // single-element enabledPlatforms list), so commands run unsandboxed.
    drop(rt);
}
