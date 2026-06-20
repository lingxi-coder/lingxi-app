//! `BuiltinToolContext` — the construction-time handle every builtin tool
//! takes in its `new()` constructor.
//!
//! Moved here from `tools/src/builtin/mod.rs` in M8-P5 so the per-category
//! tool crates (`tool-file`, `tool-shell`, …) can construct their tools
//! without depending on the `tools` monolith. The design §5.1 `ToolCtx`
//! reshape (individual `Arc<dyn Platform>` handles, no god-object) will
//! eventually slim this surface.

use crate::anthropic_request::AnthropicRequestBuilder;
use crate::read_file_state::ReadFileStateMap;
use crate::sandbox_runner::SandboxRunner;
use permission::PermissionMode;
use sandbox::runtime_config::{Platform, SandboxRuntimeConfig};
use std::path::PathBuf;
use std::sync::Arc;
use telemetry::AnalyticsBus;
use traits::budget::BudgetEnforcerHandle;
use traits::camera::CameraControl;
use traits::clipboard::Clipboard;
use traits::clock::Clock;
use traits::computer_control::ComputerControl;
use traits::filesystem::FileSystem;
use traits::http::HttpTransport;
use traits::mailbox::MailboxRouterHandle;
use traits::notification::NotificationService;
use traits::permission_gate::PermissionGate;
use traits::process::ProcessRunner;
use traits::sandbox::Sandbox;
use traits::share::SharingService;
use traits::stt::SpeechToText;
use traits::agent_name_registry::AgentNameRegistry;
use traits::subagent_spawn::SubagentSpawner;
use traits::task_registry::TaskRegistryHandle;
use traits::tts::TextToSpeech;
use traits::voice::VoiceRecorder;
use traits::worktree::WorktreeManager;

/// Static surface every builtin tool needs at construction time.
///
/// Cloning is cheap — every field is `Arc` or a small owned vec.
#[derive(Clone)]
pub struct BuiltinToolContext {
    /// Sandboxed FS access (M1).
    pub fs: Arc<dyn FileSystem>,
    /// Telemetry bus (M3-06).
    pub bus: Arc<AnalyticsBus>,
    /// Canonicalised root directories the agent is allowed to read/write.
    pub trusted_dirs: Vec<PathBuf>,
    /// Process runner — backs BashTool/PowerShellTool/REPLTool (M4-02).
    pub process: Arc<dyn ProcessRunner>,
    /// Sandbox seam — provides the `prepare`/`bypass_with_audit` constructors
    /// that turn `ProcessCommand` into `SandboxedCommand` (M4-02).
    pub sandbox: Arc<dyn Sandbox>,
    /// Wall-clock — backs SleepTool + duration measurement (M4-02).
    pub clock: Arc<dyn Clock>,
    /// Sandbox policy runtime config — drives `wrap_with_sandbox` (M4-02).
    pub sandbox_runtime: SandboxRuntimeConfig,
    /// Async sandbox-wrap seam — the shell/skill tools call `wrap` through this
    /// handle instead of `sandbox::wrap::wrap_with_sandbox` directly, so the
    /// host can inject a live `sandbox-runtime`-backed runner. Defaults to
    /// [`crate::sandbox_runner::LegacyWrapRunner`] (byte-identical to today).
    pub sandbox_runner: Arc<dyn SandboxRunner>,
    /// Active permission mode (M4-02).
    pub permission_mode: PermissionMode,
    /// Whether the host has a working sandbox backend right now (M4-02).
    pub sandbox_available: bool,
    /// Project workspace path (M4-02).
    pub workspace: PathBuf,
    /// Detected platform — drives `wrap_with_sandbox` branch (M4-02).
    pub platform: Platform,
    /// HTTP transport for web tools (WebFetch + WebSearch) (M4-03). M1 trait;
    /// tests inject `MockHttpTransport`.
    pub http: Arc<dyn HttpTransport>,
    /// Anthropic request builder for assembling `POST /v1/messages` requests.
    /// `WebSearchTool` uses it to build the HTTP request, then attaches a tool
    /// block + custom `anthropic-beta` header.
    pub provider: Arc<AnthropicRequestBuilder>,
    /// Model used by `WebSearch` when calling `POST /v1/messages` (M4-03).
    /// Sourced from the session's `coordinator_model` at registration time.
    pub default_model: String,
    /// Worktree manager (M2-01 trait) — backs `EnterWorktree` + `ExitWorktree`
    /// (M4-04). Tests inject `MockWorktreeManager`; production uses
    /// `platform_posix::PosixWorktreeManager`.
    pub worktree: Arc<dyn WorktreeManager>,

    // ===== M4-05 wiring (Phase 3) =====
    /// Subagent spawner — `AgentTool` dispatches recursive subagent runs
    /// through this seam. `None` when the host has not wired a state-
    /// machine pool yet; in that case `AgentTool::call` surfaces a clear
    /// internal error. Production wires `agent::PoolSubagentSpawner`.
    pub subagent_spawner: Option<Arc<dyn SubagentSpawner>>,
    /// Name → agent-id registry for spawned ASYNC subagents (claude
    /// `AppState.agentNameRegistry`, `AgentTool.tsx:704-711`). `AgentTool`
    /// registers `name → agentId` for a named async spawn so a later
    /// `SendMessage({ to: name })` resolves the running agent. `None` when no
    /// host registry is wired (sync-only paths never register). Distinct from
    /// the coordinator teammate roster.
    pub agent_name_registry: Option<Arc<dyn AgentNameRegistry>>,
    /// Task registry — the 6 `Task*` tools dispatch CRUD through this seam.
    /// Production wires `tasks::TaskRegistry`.
    pub task_registry: Option<Arc<dyn TaskRegistryHandle>>,
    /// Mailbox router — `SendMessageTool` dispatches teammate routing
    /// through this seam. Production wires `coordinator::MailboxRouter`.
    pub mailbox_router: Option<Arc<dyn MailboxRouterHandle>>,
    /// Budget enforcer — `AgentTool` gates spawn calls through this seam.
    /// Production wires `cost::BudgetEnforcer`.
    pub budget_enforcer: Option<Arc<dyn BudgetEnforcerHandle>>,
    /// Permission gate (enforcement 3b) — `AgentTool` threads this into the
    /// `RegistryToolInvoker` it hands the spawner, so a spawned subagent's tool
    /// calls are gated by the SAME policy as the main loop (closing the bypass
    /// where the inherited invoker dispatched any tool unconditionally). `None`
    /// = no enforcement wired (the default; legacy always-dispatch behavior).
    /// Production wires the boot gate (`PolicyPermissionGate` when
    /// `LINGXI_ENFORCE_PERMISSIONS` is set, else the no-op gate).
    pub permission_gate: Option<Arc<dyn PermissionGate>>,

    // ===== M4-07 wiring (Phase 7) =====
    /// MCP registry — the 4 MCP builtin tools (`MCPTool`, `McpAuthTool`,
    /// `ListMcpResourcesTool`, `ReadMcpResourceTool`) dispatch through this
    /// seam. `None` when the host has not wired an MCP layer; tools surface
    /// a "MCP registry not configured" error in that case. Production wires
    /// `mcp::McpRegistry` populated by the platform.
    pub mcp_registry: Option<Arc<::mcp::registry::McpRegistry>>,
    /// LSP registry — `LSPTool` dispatches through this seam. `None` when
    /// the host has not wired an LSP layer. Production wires
    /// `lsp::registry::LspRegistry` populated by plugin registration.
    pub lsp_registry: Option<Arc<::lsp::registry::LspRegistry>>,

    // ===== M8-P11 mobile / device-control capabilities =====
    /// Native camera — `tool-camera`'s `CameraTool` routes here. `None` on
    /// desktop; mobile composition roots wire `platform.camera()` (a Swift /
    /// Kotlin impl via UniFFI).
    pub camera: Option<Arc<dyn CameraControl>>,
    /// Native microphone recorder — `tool-voice`'s `VoiceTool` routes here.
    /// `None` on desktop.
    pub voice: Option<Arc<dyn VoiceRecorder>>,
    /// Native speech-to-text — `tool-speech`'s `SpeechTool` (`transcribe`)
    /// routes here. `None` on desktop; mobile wires `platform.stt()`.
    pub stt: Option<Arc<dyn SpeechToText>>,
    /// Native text-to-speech — `tool-speech`'s `SpeechTool` (`speak`) routes
    /// here. `None` on desktop; mobile wires `platform.tts()`.
    pub tts: Option<Arc<dyn TextToSpeech>>,
    /// Native share sheet — `tool-share`'s `ShareTool` routes here. `None` on
    /// desktop.
    pub share: Option<Arc<dyn SharingService>>,
    /// Native system notifications — `tool-notification`'s `NotificationTool`
    /// routes here. `None` on desktop; mobile wires `platform.notifications()`.
    pub notifications: Option<Arc<dyn NotificationService>>,
    /// Native system clipboard — `tool-clipboard`'s `ClipboardTool` routes
    /// here. `None` on desktop; mobile wires `platform.clipboard()`.
    pub clipboard: Option<Arc<dyn Clipboard>>,
    /// Screen-capture + input automation — the device-control tools
    /// (`computer`/`android_use`/`ios_use`) route here. `None` unless a
    /// desktop automation backend or a mobile UniFFI impl is wired.
    pub computer_control: Option<Arc<dyn ComputerControl>>,

    // ===== file-tools-remainder Batch B: read-state registry =====
    /// Per-path read-state map (`path → {content, mtime_ms, offset, limit}`).
    /// 1:1 port of claude-code's `readFileState`
    /// (`FileReadTool.ts:1032`): a successful `Read` records the file's
    /// content, floor-truncated mtime (ms), and the `offset`/`limit` it read
    /// with. The orchestrator shares ONE `Arc<Mutex<HashMap<…>>>` across the
    /// file tools it constructs so a write from one tool is visible to the
    /// others (and to the future staleness guards / Read dedup that will read
    /// this map). Cheap to clone (`Arc`).
    pub read_file_state: ReadFileStateMap,

    // ===== Read(deny) search-exclusion seam =====
    /// Ripgrep `--glob` exclude strings (WITHOUT the leading `!`) derived from
    /// the active `Read`-`deny` permission rules — 1:1 with claude-code
    /// `F4e(U4e(toolPermissionContext), cwd)`, computed once at engine boot via
    /// [`permission::read_deny_exclude_globs`]. The `Grep` and `Glob` tools turn
    /// each entry into a negated `ignore`-crate override so a denied/sensitive
    /// path never appears in search results (`GrepTool.ts:417-427`, `glob.ts`
    /// `lLa()`). Empty by default — every non-live construction site (tests,
    /// other tools) leaves it `vec![]`, so behavior is unchanged when no
    /// `Read`-deny rule applies. Populated only at the two live engine roots
    /// (`engine-desktop` + `engine-mobile`) from the boot `PermissionPolicy`.
    pub read_deny_exclude_globs: Vec<String>,

    // ===== Android-sandbox P3 seam =====
    /// Android-only `Shell` tool wiring (spec r3 §Shell tool). `None` on
    /// desktop / iOS. Built by `android-aar` from the probed capability cache +
    /// the `AndroidShellConfig` gate; consumed by `tool-shell-mobile::register_all`
    /// (registration gate) and the tool's prompt.
    pub android_shell: Option<AndroidShellToolCtx>,

    // ===== Android-git P4 seam =====
    /// Android-only `Git` tool wiring (spec §G5 gate). `None` on desktop /
    /// iOS. Built by `android-aar` from the enable flag, workspace readiness,
    /// and CA-store reachability; consumed by `tool-git-mobile::register_all`
    /// (registration gate) and the tool's prompt.
    pub android_git: Option<AndroidGitToolCtx>,

    /// Android-only Git **secret** seam (spec §G3 auth). Carries the HTTPS
    /// token + CA directory used by the network ops (clone/fetch/pull). Held
    /// SEPARATELY from the public [`AndroidGitToolCtx`] so the token never
    /// enters the broadly-cloned public carrier (which only exposes
    /// `has_token: bool`). `None` on desktop / iOS and whenever no token /
    /// CA dir is configured. Populated by `android-aar` (T10) and consumed by
    /// `tool-git-mobile`'s network ops. Never logged or persisted.
    pub android_git_secret: Option<AndroidGitSecret>,

    // ===== V2 task lifecycle BLOCKING hooks (TaskCreate/TaskUpdate tool path) =====
    /// BLOCKING `TaskCreated` / `TaskCompleted` lifecycle-hook firer for the V2
    /// `Task*` tool path. `None` (the default) → the tool never fires these
    /// hooks (non-blocking); the desktop composition root wires
    /// `Some(orchestrator::OrchestratorTaskLifecycleHookFirer)` over the shared
    /// `Arc<hooks::HookExecutorImpl>`.
    ///
    /// This is a SEPARATE seam from the fire-and-forget
    /// `hooks::TaskCreatedFirer` / `hooks::TaskCompletedFirer` the `TaskRegistry`
    /// holds (which "MUST NOT propagate hook failures"). It reproduces
    /// claude-code's BLOCKING paths — `executeTaskCreatedHooks`
    /// (`TaskCreateTool.ts:93-113`, on a blocking error `deleteTask` + throw) and
    /// `executeTaskCompletedHooks` (`TaskUpdateTool.ts:232-265`, on a blocking
    /// error return `success:false` and DO NOT apply the status). `tool-api`
    /// deliberately does NOT depend on `hooks`, so the trait is defined here and
    /// the orchestrator implements it.
    pub task_lifecycle_hooks: Option<Arc<dyn TaskLifecycleHookFirer>>,
}

/// BLOCKING `TaskCreated` / `TaskCompleted` lifecycle-hook firer for the V2
/// `Task*` TOOL path.
///
/// Distinct from the fire-and-forget `hooks::TaskCreatedFirer` /
/// `hooks::TaskCompletedFirer` the `TaskRegistry` holds (whose contract is "MUST
/// NOT propagate hook failures"). This seam REPORTS a blocking decision so the
/// tool can roll back / refuse, reproducing claude-code's blocking paths:
///
/// - [`fire_task_created`](Self::fire_task_created) mirrors `executeTaskCreatedHooks`
///   (`TaskCreateTool.ts:93-113`): on a blocking error claude-code `deleteTask`s
///   the just-created task and throws.
/// - [`fire_task_completed`](Self::fire_task_completed) mirrors
///   `executeTaskCompletedHooks` (`TaskUpdateTool.ts:232-265`): on a blocking
///   error claude-code returns `success:false` and does NOT apply the status.
///
/// Both methods default to a no-op `Ok(())` (allow) so unwired contexts never
/// block — an absent firer (`None`) and a wired-but-defaulted impl behave
/// identically: creation/completion proceeds. `tool-api` must NOT depend on
/// `hooks`, so the trait lives here and the orchestrator (which depends on both
/// `tool-api` and `hooks`) provides the real impl over its
/// `Arc<hooks::HookExecutorImpl>`.
#[async_trait::async_trait]
pub trait TaskLifecycleHookFirer: Send + Sync {
    /// claude-code `executeTaskCreatedHooks` BLOCKING path. `Ok(())` = allow
    /// creation; `Err(reason)` = a hook BLOCKED creation (the tool rolls the
    /// just-created task back and surfaces `reason`).
    ///
    /// `teammate_name` / `team_name` carry the creating teammate's identity
    /// (claude-code `getAgentName()` / `getTeamName()`, `TaskCreateTool.ts:97-98`)
    /// into the `TaskCreated` hook payload; `None` for the main thread / leader.
    async fn fire_task_created(
        &self,
        task_id: &str,
        subject: &str,
        description: Option<&str>,
        teammate_name: Option<&str>,
        team_name: Option<&str>,
    ) -> Result<(), String> {
        let _ = (task_id, subject, description, teammate_name, team_name);
        Ok(())
    }
    /// claude-code `executeTaskCompletedHooks` BLOCKING path. `Ok(())` = allow
    /// completion; `Err(reason)` = a hook BLOCKED completion (the tool returns
    /// `success:false` carrying `reason` and does NOT apply the status).
    async fn fire_task_completed(
        &self,
        task_id: &str,
        status: &str,
        subject: &str,
        description: Option<&str>,
    ) -> Result<(), String> {
        let _ = (task_id, status, subject, description);
        Ok(())
    }
}

/// Android-only `Shell` tool wiring (spec r3 §Shell tool). `None` on desktop /
/// iOS. Built by `android-aar` from the probed capability cache + the
/// `AndroidShellConfig` gate; consumed by `tool-shell-mobile::register_all`
/// (registration gate) and the tool's prompt.
#[derive(Debug, Clone)]
pub struct AndroidShellToolCtx {
    /// The full registration gate result: capability-probe OK + `enable_shell`
    /// + D11 secrets gate all satisfied.
    ///
    /// When `false`, the Shell tool is NOT registered (absent, not erroring).
    pub enabled: bool,
    /// Probed toybox applet inventory (for the tool prompt; may be empty).
    pub applets: Vec<String>,
    /// System sh version string (`KSH_VERSION`) when probed, for the prompt.
    pub sh_version: Option<String>,
    /// When true, the Shell runs the BUNDLED version-locked mksh + a fixed
    /// locked toybox applet inventory (not the device's system sh). Drives the
    /// tool prompt wording; the actual exec switch is in
    /// `AndroidMinijailSandbox::prepare`.
    pub bundled: bool,
}

/// Android-only `Git` tool wiring (spec §G5 gate). `None` on desktop / iOS.
/// Built by `android-aar` from the enable flag + workspace readiness +
/// CA-store reachability; consumed by `tool-git-mobile::register_all`
/// (registration gate) and the tool's prompt.
#[derive(Debug, Clone)]
pub struct AndroidGitToolCtx {
    /// The full registration gate result: `enable_git` + workspace-ready +
    /// CA-store-reachable all satisfied.
    ///
    /// When `false`, the Git tool is NOT registered (absent, not erroring).
    pub enabled: bool,
    /// Whether a credential provider is configured for the host. When `false`,
    /// network operations (clone/fetch/pull) are unavailable; the tool prompt
    /// notes that credential configuration is required for network ops. This
    /// drives the prompt only — the registration gate does not use it.
    pub has_token: bool,
    /// App-private repository root (absolute path). All git operations are
    /// anchored to this directory; paths escaping it are rejected.
    pub workspace_root: String,
}

/// Android-only Git **secret** seam (spec §G3 auth). Carries the in-memory
/// HTTPS token + CA-certificate directory used by the network git ops
/// (clone/fetch/pull). Deliberately held outside the public
/// [`AndroidGitToolCtx`] — which only exposes `has_token: bool` — so the token
/// never enters the broadly-cloned public tool carrier. `tool-git-mobile`
/// converts this into its own `GitNetConfig` at call time.
///
/// The token is never written to disk, an env var, or a child-process argv
/// (libgit2 is in-process), and is never logged. `android-aar` (T10) builds
/// this from the Keystore-backed host token + the system cacerts dir.
#[derive(Clone, Default)]
pub struct AndroidGitSecret {
    /// Per-op secret provider (HTTPS token + SSH passphrase). `None` → no secrets
    /// available (anonymous/public remotes only). Replaces the former resident
    /// `token`/`ssh_passphrase` fields — secrets are no longer held resident.
    pub credential_provider: Option<std::sync::Arc<dyn GitCredentialProvider>>,
    /// CA-certificate directory for TLS verification (Android system cacerts),
    /// or `None` to use the libgit2/OpenSSL defaults.
    pub ca_dir: Option<String>,
    /// Filesystem path to the SSH private key (spec §G7), or `None` for
    /// HTTPS-only. Host-supplied and validated to stay inside the app sandbox by
    /// `android-aar` before reaching this seam. A non-secret path.
    pub ssh_private_key_path: Option<String>,
    /// Optional path to the matching SSH public key (libssh2 can derive it from
    /// the private key when `None`). A non-secret path.
    pub ssh_public_key_path: Option<String>,
    /// Pinned SSH host-key fingerprints (lowercase-hex SHA-256). The remote's
    /// host key is accepted only if its SHA-256 is a member; an empty list
    /// rejects every host key (fail-closed). Non-secret hashes.
    pub ssh_known_hosts_sha256_hex: Vec<String>,
}

/// Per-operation Git credential provider (spec: per-op credential FFI). Supplies
/// the two true Git secrets — the HTTPS token and the SSH key passphrase —
/// fetched lazily by `tool-git-mobile` inside libgit2's credentials callback,
/// once per network op. Implemented by the host (android-aar bridges a UniFFI
/// `AndroidGitCredentialProvider` onto this); the secrets are never held resident
/// between ops. Sync (libgit2's cred callback is synchronous).
pub trait GitCredentialProvider: Send + Sync {
    /// The HTTPS token (PAT) for `userpass_plaintext`, or `None` for anonymous.
    fn https_token(&self) -> Option<String>;
    /// The SSH private-key passphrase, or `None` if the key is unencrypted.
    fn ssh_passphrase(&self) -> Option<String>;
}

// A manual `Debug` that omits the secret-bearing credential provider so secrets
// can never leak via a debug print of the context. The key/public-key paths and
// pinned host hashes are non-secret and shown normally.
impl std::fmt::Debug for AndroidGitSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AndroidGitSecret")
            .field("credential_provider", &self.credential_provider.as_ref().map(|_| "<provider>"))
            .field("ca_dir", &self.ca_dir)
            .field("ssh_private_key_path", &self.ssh_private_key_path)
            .field("ssh_public_key_path", &self.ssh_public_key_path)
            .field("ssh_known_hosts_sha256_hex", &self.ssh_known_hosts_sha256_hex)
            .finish()
    }
}

// =============================================================================
// Tests
// =============================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{ctx_for_file_tools, make_dummy_fs};
    use std::sync::Arc;
    use telemetry::AnalyticsBus;

    /// A mock provider for tests — counts calls and returns fixed secrets.
    struct MockProvider {
        token: Option<String>,
        passphrase: Option<String>,
        token_calls: std::sync::atomic::AtomicUsize,
        pass_calls: std::sync::atomic::AtomicUsize,
    }
    impl GitCredentialProvider for MockProvider {
        fn https_token(&self) -> Option<String> {
            self.token_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.token.clone()
        }
        fn ssh_passphrase(&self) -> Option<String> {
            self.pass_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.passphrase.clone()
        }
    }

    #[test]
    #[allow(clippy::default_trait_access)] // prescribed test body uses Default::default()
    fn android_git_secret_carries_provider_and_debug_has_no_secret() {
        let provider: std::sync::Arc<dyn GitCredentialProvider> = std::sync::Arc::new(MockProvider {
            token: Some("tok".into()), passphrase: Some("pp".into()),
            token_calls: Default::default(), pass_calls: Default::default(),
        });
        let s = AndroidGitSecret {
            credential_provider: Some(provider.clone()),
            ca_dir: Some("/system/etc/security/cacerts".into()),
            ssh_private_key_path: Some("/data/k".into()),
            ..Default::default()
        };
        // The provider is reachable and returns the secret on demand.
        assert_eq!(s.credential_provider.as_ref().unwrap().https_token().as_deref(), Some("tok"));
        // Debug shows NO secret value and an opaque provider marker.
        let dbg = format!("{s:?}");
        assert!(!dbg.contains("tok") && !dbg.contains("pp"), "no secret in Debug: {dbg}");
        assert!(dbg.contains("ca_dir"), "non-secrets still shown: {dbg}");
    }

    /// TDD anchor for Task 7 (P4).
    ///
    /// Asserts:
    /// 1. `AndroidGitToolCtx` constructs with all three fields.
    /// 2. The test-builder `ctx_for_file_tools` defaults `android_git` to
    ///    `None` (i.e. the field exists on `BuiltinToolContext`).
    #[test]
    fn android_git_tool_ctx_constructs_and_defaults_to_none() {
        // Construct the carrier type — enabled with token.
        let carrier = AndroidGitToolCtx {
            enabled: true,
            has_token: true,
            workspace_root: "/x".into(),
        };
        assert!(carrier.enabled);
        assert!(carrier.has_token);
        assert_eq!(carrier.workspace_root, "/x");

        // Disabled variant without token.
        let disabled = AndroidGitToolCtx {
            enabled: false,
            has_token: false,
            workspace_root: String::new(),
        };
        assert!(!disabled.enabled);
        assert!(!disabled.has_token);

        // The test-support builder must produce a ctx with `android_git: None`.
        let ctx = ctx_for_file_tools(
            make_dummy_fs(),
            Arc::new(AnalyticsBus::new()),
            vec![std::path::PathBuf::from("/tmp")],
        );
        assert!(
            ctx.android_git.is_none(),
            "android_git must default to None in test builder"
        );
    }

    /// TDD anchor for Task 1 (P3).
    ///
    /// Asserts:
    /// 1. `AndroidShellToolCtx` constructs with all three fields.
    /// 2. The test-builder `ctx_for_file_tools` defaults `android_shell` to
    ///    `None` (i.e. the field exists on `BuiltinToolContext`).
    #[test]
    fn android_shell_tool_ctx_constructs_and_defaults_to_none() {
        // Construct the carrier type — enabled.
        let carrier = AndroidShellToolCtx {
            enabled: true,
            applets: vec!["grep".into(), "ls".into()],
            sh_version: Some("@(#)MIRBSD KSH R59 2020/01/19".into()),
            bundled: false,
        };
        assert!(carrier.enabled);
        assert_eq!(carrier.applets, vec!["grep", "ls"]);
        assert!(carrier.sh_version.is_some());

        // Disabled variant.
        let disabled = AndroidShellToolCtx {
            enabled: false,
            applets: vec![],
            sh_version: None,
            bundled: false,
        };
        assert!(!disabled.enabled);
        assert!(disabled.applets.is_empty());
        assert!(disabled.sh_version.is_none());

        // The test-support builder must produce a ctx with `android_shell: None`.
        let ctx = ctx_for_file_tools(
            make_dummy_fs(),
            Arc::new(AnalyticsBus::new()),
            vec![std::path::PathBuf::from("/tmp")],
        );
        assert!(
            ctx.android_shell.is_none(),
            "android_shell must default to None in test builder"
        );
    }
}
