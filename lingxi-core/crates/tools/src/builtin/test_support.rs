//! Shared test helpers for builtin tool unit tests.
//!
//! Cleanly avoids re-declaring a 100-line `PanickingFs` impl in every tool's
//! test module. The 6 M4-01 tools all use `tokio::fs` directly; the
//! `Arc<dyn FileSystem>` field in `BuiltinToolContext` is required for future
//! M5 sandbox wiring but never touched by the tools themselves.

use crate::context::{ToolUseContext, ToolUseOptions};
use crate::progress::{progress_channel, ToolProgressSender};
use async_trait::async_trait;
use std::sync::Arc;

/// A stub `FileSystem` that panics on every method.
///
/// All 6 M4-01 builtin tools go through `tokio::fs` directly and never call
/// the FS trait, so it's safe to hand them a panicking stub. M5 sandbox
/// wiring will swap this for a real `FileSystem` impl.
pub struct PanickingFs;

#[async_trait]
impl lingxi_traits::filesystem::FileSystem for PanickingFs {
    async fn read_file(
        &self,
        _: &str,
        _: Option<u64>,
        _: Option<u64>,
    ) -> Result<lingxi_traits::filesystem::FileContent, lingxi_traits::filesystem::FsError> {
        panic!("M4-01 builtin tools do not call FileSystem::read_file");
    }
    async fn write_file(&self, _: &str, _: &str) -> Result<(), lingxi_traits::filesystem::FsError> {
        panic!("M4-01 builtin tools do not call FileSystem::write_file")
    }
    fn is_within_workspace(&self, _: &str) -> bool {
        true
    }
    async fn watch(
        &self,
        _: &str,
    ) -> Result<
        std::pin::Pin<Box<dyn futures::Stream<Item = lingxi_traits::filesystem::FileEvent> + Send>>,
        lingxi_traits::filesystem::FsError,
    > {
        panic!("not called")
    }
    async fn append_file(
        &self,
        _: &str,
        _: &str,
    ) -> Result<(), lingxi_traits::filesystem::FsError> {
        panic!("not called")
    }
    async fn truncate(&self, _: &str, _: u64) -> Result<(), lingxi_traits::filesystem::FsError> {
        panic!("not called")
    }
    async fn file_mtime(
        &self,
        _: &str,
    ) -> Result<std::time::SystemTime, lingxi_traits::filesystem::FsError> {
        panic!("not called")
    }
    async fn file_size(&self, _: &str) -> Result<u64, lingxi_traits::filesystem::FsError> {
        panic!("not called")
    }
    async fn delete_file(&self, _: &str) -> Result<(), lingxi_traits::filesystem::FsError> {
        panic!("not called")
    }
    async fn symlink(&self, _: &str, _: &str) -> Result<(), lingxi_traits::filesystem::FsError> {
        panic!("not called")
    }
    async fn flock_exclusive(
        &self,
        _: &str,
    ) -> Result<Box<dyn lingxi_traits::filesystem::FlockGuard>, lingxi_traits::filesystem::FsError>
    {
        panic!("not called")
    }
    async fn fsync(&self, _: &str) -> Result<(), lingxi_traits::filesystem::FsError> {
        panic!("not called")
    }
}

/// Convenience: return a `PanickingFs` wrapped as `Arc<dyn FileSystem>`.
pub fn make_dummy_fs() -> Arc<dyn lingxi_traits::filesystem::FileSystem> {
    Arc::new(PanickingFs) as _
}

/// Build a fresh, minimal [`ToolUseContext`] for unit tests.
pub fn fresh_ctx() -> ToolUseContext {
    ToolUseContext {
        options: ToolUseOptions {
            debug: false,
            verbose: false,
            main_loop_model: "test".into(),
            max_budget_nano_usd: None,
            mcp_clients: vec![],
            is_non_interactive_session: false,
            custom_system_prompt: None,
            append_system_prompt: None,
        },
        messages: vec![],
        tool_use_id: None,
        agent_id: None,
        content_replacement_state: None,
    }
}

/// Build a fresh progress sender wired to a dropped receiver.
pub fn fresh_tx() -> ToolProgressSender {
    let (tx, _rx) = progress_channel();
    tx
}

// ===== M4-02 shell-tool test stubs ==========================================

use lingxi_traits::process::{ProcessError, ProcessHandle, ProcessOutput, ProcessRunner};
use lingxi_traits::sandbox::{
    ProcessCommand as SbxCommand, Sandbox, SandboxBackend, SandboxCapability, SandboxError,
    SandboxFeatures, SandboxPolicy, SandboxedCommand, SandboxedTag,
};
use std::sync::Mutex;

/// In-test `ProcessRunner` that returns a canned [`ProcessOutput`] for each
/// `run` call. Panics on `spawn_background` (most tests don't need it; the
/// bash background test wires up its own bespoke stub).
pub struct StubProcess {
    queued: Mutex<Vec<ProcessOutput>>,
}

impl StubProcess {
    /// Construct a stub that will return `outputs[i]` on the i-th `run` call.
    #[must_use]
    pub fn with(outputs: Vec<ProcessOutput>) -> Self {
        Self {
            queued: Mutex::new(outputs),
        }
    }

    /// Single-output convenience constructor.
    #[must_use]
    pub fn single(output: ProcessOutput) -> Self {
        Self::with(vec![output])
    }
}

#[async_trait]
impl ProcessRunner for StubProcess {
    async fn run(&self, _cmd: &SandboxedCommand) -> Result<ProcessOutput, ProcessError> {
        let mut q = self.queued.lock().unwrap();
        if q.is_empty() {
            return Err(ProcessError::Io("stub exhausted".into()));
        }
        Ok(q.remove(0))
    }

    async fn spawn_background(
        &self,
        _cmd: &SandboxedCommand,
    ) -> Result<ProcessHandle, ProcessError> {
        Err(ProcessError::Unsupported)
    }

    async fn kill(&self, _handle: &ProcessHandle) -> Result<(), ProcessError> {
        Ok(())
    }

    fn is_available(&self) -> bool {
        true
    }
}

/// Auditing-bypass-only `Sandbox` impl. Every command is tagged as a bypass
/// with the supplied reason — convenient for unit tests that don't care
/// about sandbox semantics.
pub struct BypassSandbox;

#[async_trait]
impl Sandbox for BypassSandbox {
    fn is_available(&self) -> bool {
        true
    }
    fn backend(&self) -> SandboxBackend {
        SandboxBackend::None
    }
    fn prepare(
        &self,
        cmd: SbxCommand,
        _policy: &SandboxPolicy,
    ) -> Result<SandboxedCommand, SandboxError> {
        Ok(SandboxedCommand::__new_sandboxed(
            cmd,
            SandboxedTag::BypassAuditedWithReason {
                reason: "test_bypass".into(),
            },
        ))
    }
    fn bypass_with_audit(&self, cmd: SbxCommand, reason: &str) -> SandboxedCommand {
        SandboxedCommand::__new_sandboxed(
            cmd,
            SandboxedTag::BypassAuditedWithReason {
                reason: reason.into(),
            },
        )
    }
    async fn probe_capability(&self) -> SandboxCapability {
        SandboxCapability {
            available: true,
            reason: None,
            features: SandboxFeatures::default(),
        }
    }
}

/// Convenience: wrap [`StubProcess::single`] in `Arc<dyn ProcessRunner>`.
#[must_use]
pub fn make_stub_process(out: ProcessOutput) -> Arc<dyn ProcessRunner> {
    Arc::new(StubProcess::single(out))
}

/// Convenience: wrap [`BypassSandbox`] in `Arc<dyn Sandbox>`.
#[must_use]
pub fn make_bypass_sandbox() -> Arc<dyn Sandbox> {
    Arc::new(BypassSandbox)
}

/// Minimal in-test wall-clock that ticks deterministically.
pub struct StubClock {
    now: Mutex<std::time::SystemTime>,
}

impl StubClock {
    /// Construct anchored at the Unix epoch.
    #[must_use]
    pub fn new() -> Self {
        Self {
            now: Mutex::new(std::time::UNIX_EPOCH),
        }
    }
}

impl Default for StubClock {
    fn default() -> Self {
        Self::new()
    }
}

impl lingxi_traits::Clock for StubClock {
    fn now(&self) -> std::time::SystemTime {
        *self.now.lock().unwrap()
    }
}

/// Convenience: wrap a fresh `StubClock` in `Arc<dyn Clock>`.
#[must_use]
pub fn make_stub_clock() -> Arc<dyn lingxi_traits::Clock> {
    Arc::new(StubClock::new())
}

/// Build a [`super::BuiltinToolContext`] for M4-01 file-tool unit tests
/// (process/sandbox/clock get stubbed defaults so the M4-02 fields satisfy
/// the struct shape without affecting file-tool behavior).
#[must_use]
pub fn ctx_for_file_tools(
    fs: Arc<dyn lingxi_traits::filesystem::FileSystem>,
    bus: Arc<lingxi_telemetry::AnalyticsBus>,
    trusted_dirs: Vec<std::path::PathBuf>,
) -> super::BuiltinToolContext {
    use lingxi_permission::PermissionMode;
    use lingxi_sandbox::decision::ProjectTrustLevel;
    use lingxi_sandbox::runtime_config::{Platform, SandboxRuntimeConfig};

    let workspace = trusted_dirs
        .first()
        .cloned()
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"));

    super::BuiltinToolContext {
        fs,
        bus,
        trusted_dirs,
        process: make_stub_process(ProcessOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        }),
        sandbox: make_bypass_sandbox(),
        clock: make_stub_clock(),
        sandbox_runtime: SandboxRuntimeConfig::default(),
        permission_mode: PermissionMode::Default,
        project_trust: ProjectTrustLevel::Trusted,
        sandbox_available: false,
        workspace,
        platform: if cfg!(target_os = "macos") {
            Platform::Mac
        } else {
            Platform::Linux
        },
    }
}

/// Build a full [`super::BuiltinToolContext`] for shell-tool unit tests.
#[must_use]
#[allow(dead_code)] // used by M4-02 shell-tool tests (bash/powershell/repl/sleep)
pub fn shell_test_ctx(out: ProcessOutput) -> super::BuiltinToolContext {
    use lingxi_permission::PermissionMode;
    use lingxi_sandbox::decision::ProjectTrustLevel;
    use lingxi_sandbox::runtime_config::{Platform, SandboxRuntimeConfig};
    use lingxi_telemetry::AnalyticsBus;
    use std::path::PathBuf;

    super::BuiltinToolContext {
        fs: make_dummy_fs(),
        bus: Arc::new(AnalyticsBus::new()),
        trusted_dirs: vec![PathBuf::from("/tmp")],
        process: make_stub_process(out),
        sandbox: make_bypass_sandbox(),
        clock: make_stub_clock(),
        sandbox_runtime: SandboxRuntimeConfig::default(),
        permission_mode: PermissionMode::Default,
        project_trust: ProjectTrustLevel::Trusted,
        sandbox_available: false,
        workspace: PathBuf::from("/tmp"),
        platform: if cfg!(target_os = "macos") {
            Platform::Mac
        } else {
            Platform::Linux
        },
    }
}
