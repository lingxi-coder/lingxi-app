//! Builtin tools — M4 sub-plans 01..08 land 40 implementations under this tree.
//!
//! M4-01 (this file) ships 6 foundation tools (file ops + search) and the
//! `register_all_builtin_tools` entrypoint that future sub-plans extend.

use crate::registry::ToolRegistry;
use lingxi_api_client::AnthropicProvider;
use lingxi_permission::PermissionMode;
use lingxi_sandbox::decision::ProjectTrustLevel;
use lingxi_sandbox::runtime_config::{Platform, SandboxRuntimeConfig};
use lingxi_telemetry::AnalyticsBus;
use lingxi_traits::budget::BudgetEnforcerHandle;
use lingxi_traits::clock::Clock;
use lingxi_traits::filesystem::FileSystem;
use lingxi_traits::http::HttpTransport;
use lingxi_traits::mailbox::MailboxRouterHandle;
use lingxi_traits::process::ProcessRunner;
use lingxi_traits::sandbox::Sandbox;
use lingxi_traits::subagent_spawn::SubagentSpawner;
use lingxi_traits::task_registry::TaskRegistryHandle;
use lingxi_traits::worktree::WorktreeManager;
use std::path::PathBuf;
use std::sync::Arc;

pub mod agent;
pub mod ask_user_question;
pub mod bash;
pub mod brief;
pub mod file_edit;
pub mod file_read;
pub mod file_write;
pub mod glob;
pub mod grep;
pub mod lsp;
pub mod mcp;
pub mod notebook_edit;
pub mod plan_mode;
pub mod powershell;
pub mod repl;
pub mod send_message;
pub mod shell_events;
pub mod sleep;
pub mod task;
pub mod team;
pub mod todo_write;
pub mod web_fetch;
pub mod web_search;
pub mod worktree;

#[cfg(test)]
pub(crate) mod test_support;

/// M4-05 wiring mocks. Public so `tests/agent_task_integration_test.rs`
/// (an external integration crate) can pull in the same fixtures the
/// in-crate unit tests use.
pub mod agent_test_support;

pub use agent::AgentTool;
pub use ask_user_question::AskUserQuestionTool;
pub use bash::BashTool;
pub use brief::BriefTool;
pub use file_edit::FileEditTool;
pub use file_read::FileReadTool;
pub use file_write::FileWriteTool;
pub use glob::GlobTool;
pub use grep::GrepTool;
pub use lsp::LSPTool;
pub use mcp::{ListMcpResourcesTool, MCPTool, McpAuthTool, ReadMcpResourceTool};
pub use notebook_edit::NotebookEditTool;
pub use plan_mode::{EnterPlanModeTool, ExitPlanModeTool};
pub use powershell::PowerShellTool;
pub use repl::REPLTool;
pub use send_message::SendMessageTool;
pub use sleep::SleepTool;
pub use task::{
    TaskCreateTool, TaskGetTool, TaskListTool, TaskOutputTool, TaskStopTool, TaskUpdateTool,
};
pub use team::{TeamCreateTool, TeamDeleteTool};
pub use todo_write::TodoWriteTool;
pub use web_fetch::WebFetchTool;
pub use web_search::WebSearchTool;
pub use worktree::{EnterWorktreeTool, ExitWorktreeTool};

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
    /// Active permission mode (M4-02).
    pub permission_mode: PermissionMode,
    /// Whether the project workspace has been explicitly trusted (M4-02).
    pub project_trust: ProjectTrustLevel,
    /// Whether the host has a working sandbox backend right now (M4-02).
    pub sandbox_available: bool,
    /// Project workspace path (M4-02).
    pub workspace: PathBuf,
    /// Detected platform — drives `wrap_with_sandbox` branch (M4-02).
    pub platform: Platform,
    /// HTTP transport for web tools (WebFetch + WebSearch) (M4-03). M1 trait;
    /// tests inject `MockHttpTransport`.
    pub http: Arc<dyn HttpTransport>,
    /// Anthropic provider for assembling `POST /v1/messages` requests (M3-03).
    /// `WebSearchTool` uses it to build the HTTP request, then attaches a tool
    /// block + custom `anthropic-beta` header.
    pub provider: Arc<AnthropicProvider>,
    /// Model used by `WebSearch` when calling `POST /v1/messages` (M4-03).
    /// Sourced from the session's `coordinator_model` at registration time.
    pub default_model: String,
    /// Worktree manager (M2-01 trait) — backs `EnterWorktree` + `ExitWorktree`
    /// (M4-04). Tests inject `MockWorktreeManager`; production uses
    /// `lingxi_platform_posix::PosixWorktreeManager`.
    pub worktree: Arc<dyn WorktreeManager>,

    // ===== M4-05 wiring (Phase 3) =====
    /// Subagent spawner — `AgentTool` dispatches recursive subagent runs
    /// through this seam. `None` when the host has not wired a state-
    /// machine pool yet; in that case `AgentTool::call` surfaces a clear
    /// internal error. Production wires `lingxi_agent::PoolSubagentSpawner`.
    pub subagent_spawner: Option<Arc<dyn SubagentSpawner>>,
    /// Task registry — the 6 `Task*` tools dispatch CRUD through this seam.
    /// Production wires `lingxi_tasks::TaskRegistry`.
    pub task_registry: Option<Arc<dyn TaskRegistryHandle>>,
    /// Mailbox router — `SendMessageTool` dispatches teammate routing
    /// through this seam. Production wires `lingxi_coordinator::MailboxRouter`.
    pub mailbox_router: Option<Arc<dyn MailboxRouterHandle>>,
    /// Budget enforcer — `AgentTool` gates spawn calls through this seam.
    /// Production wires `lingxi_cost::BudgetEnforcer`.
    pub budget_enforcer: Option<Arc<dyn BudgetEnforcerHandle>>,

    // ===== M4-07 wiring (Phase 7) =====
    /// MCP registry — the 4 MCP builtin tools (`MCPTool`, `McpAuthTool`,
    /// `ListMcpResourcesTool`, `ReadMcpResourceTool`) dispatch through this
    /// seam. `None` when the host has not wired an MCP layer; tools surface
    /// a "MCP registry not configured" error in that case. Production wires
    /// `lingxi_mcp::McpRegistry` populated by the platform.
    pub mcp_registry: Option<Arc<lingxi_mcp::registry::McpRegistry>>,
    /// LSP registry — `LSPTool` dispatches through this seam. `None` when
    /// the host has not wired an LSP layer. Production wires
    /// `lingxi_lsp::registry::LspRegistry` populated by plugin registration.
    pub lsp_registry: Option<Arc<lingxi_lsp::registry::LspRegistry>>,
}

/// Register every M4-01 foundation tool against `registry`.
///
/// Idempotent insertion: caller MUST pass an empty registry (or accept
/// duplicate registrations). Later sub-plans add Shell / Web / Workflow /
/// Agent / Team / MCP+LSP / System tools by extending this function.
///
/// Each tool's `impl Tool` is wired in Tasks 5/7/9/11/13/15; as the impls
/// land, the matching line below is uncommented incrementally so the crate
/// remains buildable between tasks. Task 17 verifies that all 6 are wired.
pub fn register_all_builtin_tools(registry: &mut ToolRegistry, ctx: BuiltinToolContext) {
    // M4-01 — file/search tools.
    registry.register_builtin(Arc::new(FileReadTool::new(ctx.clone())));
    registry.register_builtin(Arc::new(FileWriteTool::new(ctx.clone())));
    registry.register_builtin(Arc::new(FileEditTool::new(ctx.clone())));
    registry.register_builtin(Arc::new(NotebookEditTool::new(ctx.clone())));
    registry.register_builtin(Arc::new(GlobTool::new(ctx.clone())));
    registry.register_builtin(Arc::new(GrepTool::new(ctx.clone())));
    // M4-02 — shell tools.
    registry.register_builtin(Arc::new(BashTool::new(ctx.clone())));
    registry.register_builtin(Arc::new(PowerShellTool::new(ctx.clone())));
    registry.register_builtin(Arc::new(REPLTool::new(ctx.clone())));
    registry.register_builtin(Arc::new(SleepTool::new(ctx.clone())));
    // M4-03 — web tools.
    registry.register_builtin(Arc::new(WebFetchTool::new(ctx.clone())));
    registry.register_builtin(Arc::new(WebSearchTool::new(ctx.clone())));
    // M4-04 — workflow tools.
    registry.register_builtin(Arc::new(TodoWriteTool::new(ctx.clone())));
    registry.register_builtin(Arc::new(EnterPlanModeTool::new(ctx.clone())));
    registry.register_builtin(Arc::new(ExitPlanModeTool::new(ctx.clone())));
    registry.register_builtin(Arc::new(EnterWorktreeTool::new(ctx.clone())));
    registry.register_builtin(Arc::new(ExitWorktreeTool::new(ctx.clone())));
    // M4-05 — agent + task + send_message tools.
    registry.register_builtin(Arc::new(AgentTool::new(ctx.clone())));
    registry.register_builtin(Arc::new(TaskCreateTool::new(ctx.clone())));
    registry.register_builtin(Arc::new(TaskGetTool::new(ctx.clone())));
    registry.register_builtin(Arc::new(TaskListTool::new(ctx.clone())));
    registry.register_builtin(Arc::new(TaskUpdateTool::new(ctx.clone())));
    registry.register_builtin(Arc::new(TaskStopTool::new(ctx.clone())));
    registry.register_builtin(Arc::new(TaskOutputTool::new(ctx.clone())));
    registry.register_builtin(Arc::new(SendMessageTool::new(ctx.clone())));
    // M4-06 — team tools.
    registry.register_builtin(Arc::new(TeamCreateTool::new(ctx.clone())));
    registry.register_builtin(Arc::new(TeamDeleteTool::new(ctx.clone())));
    // M4-07 — MCP + LSP tools.
    registry.register_builtin(Arc::new(MCPTool::new(ctx.clone())));
    registry.register_builtin(Arc::new(McpAuthTool::new(ctx.clone())));
    registry.register_builtin(Arc::new(ListMcpResourcesTool::new(ctx.clone())));
    registry.register_builtin(Arc::new(ReadMcpResourceTool::new(ctx.clone())));
    registry.register_builtin(Arc::new(LSPTool::new(ctx)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin::test_support::make_dummy_fs;
    use crate::registry::ToolRegistry;
    use lingxi_telemetry::AnalyticsBus;
    use std::path::PathBuf;

    fn dummy_ctx() -> BuiltinToolContext {
        use crate::builtin::test_support::{
            make_bypass_sandbox, make_stub_clock, make_stub_http, make_stub_process,
        };
        use lingxi_traits::process::ProcessOutput;
        BuiltinToolContext {
            fs: make_dummy_fs(),
            bus: Arc::new(AnalyticsBus::new()),
            trusted_dirs: vec![PathBuf::from("/tmp")],
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
            workspace: PathBuf::from("/tmp"),
            platform: if cfg!(target_os = "macos") {
                Platform::Mac
            } else {
                Platform::Linux
            },
            http: make_stub_http(),
            provider: Arc::new(AnthropicProvider::new("test-key", None)),
            default_model: "claude-sonnet-4-20250514".to_string(),
            worktree: crate::builtin::test_support::make_mock_worktree(),
            subagent_spawner: None,
            task_registry: None,
            mailbox_router: None,
            budget_enforcer: None,
            mcp_registry: None,
            lsp_registry: None,
        }
    }

    #[test]
    fn register_all_inserts_thirty_two_tools_after_m4_07() {
        let mut registry = ToolRegistry::new();
        register_all_builtin_tools(&mut registry, dummy_ctx());
        let ctx = crate::tool_trait::ToolStaticContext::default();
        let tools = registry.available_tools(&ctx);
        // M4-01 (6) + M4-02 (4) + M4-03 (2) + M4-04 (5) + M4-05 (8) + M4-06 (2) + M4-07 (5) = 32.
        assert_eq!(tools.len(), 32);
        let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
        for n in [
            "Read",
            "Write",
            "Edit",
            "NotebookEdit",
            "Glob",
            "Grep",
            "Bash",
            "PowerShell",
            "REPL",
            "Sleep",
            "WebFetch",
            "WebSearch",
            "TodoWrite",
            "EnterPlanMode",
            "ExitPlanMode",
            "EnterWorktree",
            "ExitWorktree",
            "Agent",
            "TaskCreate",
            "TaskGet",
            "TaskList",
            "TaskUpdate",
            "TaskStop",
            "TaskOutput",
            "SendMessage",
            "TeamCreate",
            "TeamDelete",
            "MCP",
            "McpAuth",
            "ListMcpResources",
            "ReadMcpResource",
            "LSP",
        ] {
            assert!(names.contains(&n), "missing tool {n}: {names:?}");
        }
    }

    #[test]
    fn find_by_name_works_for_every_tool() {
        let mut registry = ToolRegistry::new();
        register_all_builtin_tools(&mut registry, dummy_ctx());
        for name in [
            "Read",
            "Write",
            "Edit",
            "NotebookEdit",
            "Glob",
            "Grep",
            "Bash",
            "PowerShell",
            "REPL",
            "Sleep",
            "WebFetch",
            "WebSearch",
            "TodoWrite",
            "EnterPlanMode",
            "ExitPlanMode",
            "EnterWorktree",
            "ExitWorktree",
            "Agent",
            "TaskCreate",
            "TaskGet",
            "TaskList",
            "TaskUpdate",
            "TaskStop",
            "TaskOutput",
            "SendMessage",
            "TeamCreate",
            "TeamDelete",
            "MCP",
            "McpAuth",
            "ListMcpResources",
            "ReadMcpResource",
            "LSP",
        ] {
            assert!(
                registry.find_by_name(name).is_some(),
                "registry missing {name}"
            );
        }
    }

    #[test]
    fn agent_tool_resolvable_by_legacy_task_alias() {
        let mut registry = ToolRegistry::new();
        register_all_builtin_tools(&mut registry, dummy_ctx());
        // The dispatcher must honor `aliases()` — "Task" → AgentTool.
        let tool = registry
            .find_by_name("Task")
            .expect("legacy 'Task' alias must resolve to AgentTool");
        assert_eq!(tool.name(), "Agent");
    }
}
