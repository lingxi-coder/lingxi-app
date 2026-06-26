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
    /// The process uid checked against 0. claude-code calls `process.getuid()`
    /// — the REAL uid (`setup.ts:404`), so the production impl returns
    /// `getuid()`, NOT the effective uid (the method name is a slight misnomer
    /// kept for the trait contract). Non-unix hosts return a non-zero sentinel
    /// so check 1 is a no-op.
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
        && !env_truthy(env.env("LINGXI_BUBBLEWRAP"))
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
        let is_bubblewrap = env_truthy(env.env("LINGXI_BUBBLEWRAP"));
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
        e.vars.insert("LINGXI_BUBBLEWRAP".into(), "1".into());
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
