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
impl traits::filesystem::FileSystem for PanickingFs {
    async fn read_file(
        &self,
        _: &str,
        _: Option<u64>,
        _: Option<u64>,
    ) -> Result<traits::filesystem::FileContent, traits::filesystem::FsError> {
        panic!("M4-01 builtin tools do not call FileSystem::read_file");
    }
    async fn write_file(&self, _: &str, _: &str) -> Result<(), traits::filesystem::FsError> {
        panic!("M4-01 builtin tools do not call FileSystem::write_file")
    }
    fn is_within_workspace(&self, _: &str) -> bool {
        true
    }
    async fn watch(
        &self,
        _: &str,
    ) -> Result<
        std::pin::Pin<Box<dyn futures::Stream<Item = traits::filesystem::FileEvent> + Send>>,
        traits::filesystem::FsError,
    > {
        panic!("not called")
    }
    async fn append_file(&self, _: &str, _: &str) -> Result<(), traits::filesystem::FsError> {
        panic!("not called")
    }
    async fn truncate(&self, _: &str, _: u64) -> Result<(), traits::filesystem::FsError> {
        panic!("not called")
    }
    async fn file_mtime(
        &self,
        _: &str,
    ) -> Result<std::time::SystemTime, traits::filesystem::FsError> {
        panic!("not called")
    }
    async fn file_size(&self, _: &str) -> Result<u64, traits::filesystem::FsError> {
        panic!("not called")
    }
    async fn delete_file(&self, _: &str) -> Result<(), traits::filesystem::FsError> {
        panic!("not called")
    }
    async fn symlink(&self, _: &str, _: &str) -> Result<(), traits::filesystem::FsError> {
        panic!("not called")
    }
    async fn flock_exclusive(
        &self,
        _: &str,
    ) -> Result<Box<dyn traits::filesystem::FlockGuard>, traits::filesystem::FsError> {
        panic!("not called")
    }
    async fn fsync(&self, _: &str) -> Result<(), traits::filesystem::FsError> {
        panic!("not called")
    }
}

/// Convenience: return a `PanickingFs` wrapped as `Arc<dyn FileSystem>`.
pub fn make_dummy_fs() -> Arc<dyn traits::filesystem::FileSystem> {
    Arc::new(PanickingFs) as _
}

/// Process-wide HOME lock for tests that mutate `$HOME` via `std::env::set_var`.
///
/// M4-08 builtin tools (Brief, Config, ScheduleCron, RemoteTrigger) each
/// resolve their on-disk dir from `$HOME`. Their unit tests redirect HOME to
/// a `tempfile::TempDir`; serialize them through this single lock so parallel
/// test threads from different modules don't race on the same env var.
///
/// `tokio::sync::Mutex` (not `std::sync::Mutex`) so the guard can be safely
/// held across `await` points in `#[tokio::test]` bodies.
pub static HOME_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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
        session: None,
        subagent_registry: None,
    }
}

/// Build a fresh progress sender wired to a dropped receiver.
pub fn fresh_tx() -> ToolProgressSender {
    let (tx, _rx) = progress_channel();
    tx
}

// ===== M4-02 shell-tool test stubs ==========================================

use std::sync::Mutex;
use traits::process::{ProcessError, ProcessHandle, ProcessOutput, ProcessRunner};
use traits::sandbox::{
    ProcessCommand as SbxCommand, Sandbox, SandboxBackend, SandboxCapability, SandboxError,
    SandboxFeatures, SandboxPolicy, SandboxedCommand, SandboxedTag,
};

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

impl traits::Clock for StubClock {
    fn now(&self) -> std::time::SystemTime {
        *self.now.lock().unwrap()
    }
}

/// Convenience: wrap a fresh `StubClock` in `Arc<dyn Clock>`.
#[must_use]
pub fn make_stub_clock() -> Arc<dyn traits::Clock> {
    Arc::new(StubClock::new())
}

/// A `HttpTransport` stub that always errors. Satisfies the
/// `BuiltinToolContext::http` field for tests that don't exercise it.
pub struct PanickingHttp;

#[async_trait]
impl traits::http::HttpTransport for PanickingHttp {
    async fn request(
        &self,
        _: protocol::HttpRequest,
    ) -> Result<protocol::HttpResponse, traits::http::HttpError> {
        Err(traits::http::HttpError::InvalidRequest(
            "stub PanickingHttp: not configured for this test".into(),
        ))
    }
    async fn stream_sse(
        &self,
        _: protocol::HttpRequest,
    ) -> Result<traits::http::SseStream, traits::http::HttpError> {
        Err(traits::http::HttpError::InvalidRequest(
            "stub PanickingHttp: stream_sse not supported".into(),
        ))
    }
}

/// Convenience: wrap [`PanickingHttp`] in `Arc<dyn HttpTransport>`.
#[must_use]
pub fn make_stub_http() -> Arc<dyn traits::http::HttpTransport> {
    Arc::new(PanickingHttp)
}

// ===== M4-04 workflow-tool test stubs =======================================

use std::path::PathBuf;
use std::time::Duration;
use traits::worktree::{WorktreeError, WorktreeHandle, WorktreeInfo, WorktreeManager};

/// In-memory `WorktreeManager` for hermetic tests. Tracks every call so tests
/// can assert on `created`, `removed`, `listed`. Reuses the M2-01 slug helpers
/// (`validate_worktree_slug` + `flatten_slug`) so its outputs are byte-aligned
/// with production.
#[allow(dead_code)] // M4-04 Tasks 10/11/13 use these helpers
#[derive(Default)]
pub struct MockWorktreeManager {
    inner: std::sync::Mutex<MockWtInner>,
}

#[derive(Default)]
struct MockWtInner {
    created: Vec<(String, WorktreeHandle)>,
    removed: Vec<WorktreeHandle>,
    next_path_root: Option<PathBuf>,
    scripted_create_error: Option<WorktreeError>,
    scripted_remove_error: Option<WorktreeError>,
}

#[allow(dead_code)] // M4-04 Tasks 10/11/13 use these helpers
impl MockWorktreeManager {
    /// Build a fresh `MockWorktreeManager`. Defaults to creating worktrees
    /// under `/tmp/mock-repo/.claude/worktrees/<flatten(slug)>`.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Override the root used to assemble new worktree paths.
    #[must_use]
    pub fn with_root(root: impl Into<PathBuf>) -> Self {
        let s = Self::default();
        s.inner.lock().unwrap().next_path_root = Some(root.into());
        s
    }

    /// Force the next `create_worktree` call to return `err`.
    pub fn script_create_error(&self, err: WorktreeError) {
        self.inner.lock().unwrap().scripted_create_error = Some(err);
    }

    /// Force the next `remove_worktree` call to return `err`.
    #[allow(dead_code)]
    pub fn script_remove_error(&self, err: WorktreeError) {
        self.inner.lock().unwrap().scripted_remove_error = Some(err);
    }

    /// Inspect created worktrees.
    #[must_use]
    pub fn created(&self) -> Vec<(String, WorktreeHandle)> {
        self.inner.lock().unwrap().created.clone()
    }

    /// Inspect removed worktrees.
    #[must_use]
    pub fn removed(&self) -> Vec<WorktreeHandle> {
        self.inner.lock().unwrap().removed.clone()
    }
}

#[async_trait]
impl WorktreeManager for MockWorktreeManager {
    async fn create_worktree(
        &self,
        slug: &str,
        _base_branch: Option<&str>,
        _copy_includes: &[PathBuf],
    ) -> Result<WorktreeHandle, WorktreeError> {
        // Drain scripted error first.
        if let Some(err) = self.inner.lock().unwrap().scripted_create_error.take() {
            return Err(err);
        }
        // Mirror the M2-01 validation locally — keeps the mock byte-aligned
        // with `EnterWorktreeTool`'s pre-flight check without pulling in
        // `lingxi-platform-posix` (which would form a dep cycle).
        validate_slug_inline(slug)?;
        let flat = flatten_slug_inline(slug);
        let root = self
            .inner
            .lock()
            .unwrap()
            .next_path_root
            .clone()
            .unwrap_or_else(|| PathBuf::from("/tmp/mock-repo"));
        let path = root.join(".claude").join("worktrees").join(&flat);
        let handle = WorktreeHandle {
            path,
            branch_name: format!("worktree-{flat}"),
        };
        self.inner
            .lock()
            .unwrap()
            .created
            .push((slug.to_string(), handle.clone()));
        Ok(handle)
    }

    async fn remove_worktree(&self, handle: &WorktreeHandle) -> Result<(), WorktreeError> {
        if let Some(err) = self.inner.lock().unwrap().scripted_remove_error.take() {
            return Err(err);
        }
        self.inner.lock().unwrap().removed.push(handle.clone());
        Ok(())
    }

    async fn list_worktrees(&self) -> Result<Vec<WorktreeInfo>, WorktreeError> {
        Ok(Vec::new())
    }

    async fn cleanup_stale(&self, _max_age: Duration) -> Result<Vec<PathBuf>, WorktreeError> {
        Ok(Vec::new())
    }

    fn is_supported(&self) -> bool {
        true
    }
}

/// Convenience: wrap a fresh `MockWorktreeManager` in `Arc<dyn WorktreeManager>`.
#[must_use]
pub fn make_mock_worktree() -> Arc<dyn WorktreeManager> {
    Arc::new(MockWorktreeManager::new())
}

const MAX_SLUG_LEN: usize = 64;

fn validate_slug_inline(slug: &str) -> Result<(), WorktreeError> {
    if slug.is_empty() {
        return Err(WorktreeError::InvalidSlug("slug is empty".into()));
    }
    if slug.len() > MAX_SLUG_LEN {
        return Err(WorktreeError::InvalidSlug(format!(
            "slug exceeds {MAX_SLUG_LEN} chars (got {})",
            slug.len()
        )));
    }
    for segment in slug.split('/') {
        if segment.is_empty() {
            return Err(WorktreeError::InvalidSlug(format!(
                "slug contains empty segment: {slug:?}"
            )));
        }
        for ch in segment.chars() {
            let allowed = ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-';
            if !allowed {
                return Err(WorktreeError::InvalidSlug(format!(
                    "slug contains invalid character {ch:?} in segment {segment:?}"
                )));
            }
        }
    }
    Ok(())
}

fn flatten_slug_inline(slug: &str) -> String {
    slug.replace('/', "+")
}

/// Build a [`super::BuiltinToolContext`] for M4-01 file-tool unit tests
/// (process/sandbox/clock get stubbed defaults so the M4-02 fields satisfy
/// the struct shape without affecting file-tool behavior).
#[must_use]
pub fn ctx_for_file_tools(
    fs: Arc<dyn traits::filesystem::FileSystem>,
    bus: Arc<telemetry::AnalyticsBus>,
    trusted_dirs: Vec<std::path::PathBuf>,
) -> super::BuiltinToolContext {
    use permission::PermissionMode;
    use sandbox::decision::ProjectTrustLevel;
    use sandbox::runtime_config::{Platform, SandboxRuntimeConfig};

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
        http: make_stub_http(),
        provider: Arc::new(api_client::AnthropicProvider::new("test-key", None)),
        default_model: "claude-sonnet-4-20250514".to_string(),
        worktree: make_mock_worktree(),
        subagent_spawner: None,
        task_registry: None,
        mailbox_router: None,
        budget_enforcer: None,
        permission_gate: None,
        mcp_registry: None,
        lsp_registry: None,
        camera: None,
        voice: None,
        stt: None,
        tts: None,
        share: None,
        computer_control: None,
    }
}

/// Build a full [`super::BuiltinToolContext`] for shell-tool unit tests.
#[must_use]
#[allow(dead_code)] // used by M4-02 shell-tool tests (bash/powershell/repl/sleep)
pub fn shell_test_ctx(out: ProcessOutput) -> super::BuiltinToolContext {
    use permission::PermissionMode;
    use sandbox::decision::ProjectTrustLevel;
    use sandbox::runtime_config::{Platform, SandboxRuntimeConfig};
    use std::path::PathBuf;
    use telemetry::AnalyticsBus;

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
        http: make_stub_http(),
        provider: Arc::new(api_client::AnthropicProvider::new("test-key", None)),
        default_model: "claude-sonnet-4-20250514".to_string(),
        worktree: make_mock_worktree(),
        subagent_spawner: None,
        task_registry: None,
        mailbox_router: None,
        budget_enforcer: None,
        permission_gate: None,
        mcp_registry: None,
        lsp_registry: None,
        camera: None,
        voice: None,
        stt: None,
        tts: None,
        share: None,
        computer_control: None,
    }
}
