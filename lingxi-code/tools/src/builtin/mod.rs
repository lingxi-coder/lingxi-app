//! Builtin tools — M4 sub-plans 01..08 land 40 implementations under this tree.
//!
//! M4-01 (this file) ships 6 foundation tools (file ops + search) and the
//! `register_all_builtin_tools` entrypoint that future sub-plans extend.

use crate::registry::ToolRegistry;
use std::sync::Arc;

pub mod agent;
pub mod mcp;
pub mod shell_events;

// M8-P5: shared test fixture moved to tool-api (feature "test-support").
#[cfg(test)]
pub(crate) use tool_api::test_support;

/// M4-05 wiring mocks. Public so `tests/agent_task_integration_test.rs`
/// (an external integration crate) can pull in the same fixtures the
/// in-crate unit tests use.
pub mod agent_test_support;

pub use agent::AgentTool;
pub use mcp::{ListMcpResourcesTool, MCPTool, McpAuthTool, ReadMcpResourceTool};

// M8-P5: BuiltinToolContext moved to tool-api so per-category tool crates
// can construct tools without depending on this monolith.
pub use tool_api::BuiltinToolContext;
// M8-P5: file/search tools extracted to the tool-file crate; re-export the
// tool types AND the modules (under their old `file_*` names) so
// `tools::builtin::FileReadTool` and `tools::builtin::file_read::TOOL_NAME`
// (parity tests, external callers) keep working.
pub use tool_cron::{remote_trigger, schedule_cron};
pub use tool_cron::{RemoteTriggerTool, ScheduleCronTool};
pub use tool_file::{
    edit as file_edit, glob, grep, notebook_edit, read as file_read, write as file_write,
};
pub use tool_file::{
    FileEditTool, FileReadTool, FileWriteTool, GlobTool, GrepTool, NotebookEditTool,
};
pub use tool_lsp::lsp_tool as lsp;
pub use tool_lsp::LSPTool;
pub use tool_meta::{config, tool_search};
pub use tool_meta::{ConfigTool, ToolSearchTool};
pub use tool_plan::plan_mode;
pub use tool_plan::{EnterPlanModeTool, ExitPlanModeTool};
pub use tool_shell::{bash, powershell, repl};
pub use tool_shell::{BashTool, PowerShellTool, REPLTool};
pub use tool_skill::skill;
pub use tool_skill::SkillTool;
pub use tool_task::{task, todo_write};
pub use tool_task::{
    TaskCreateTool, TaskGetTool, TaskListTool, TaskOutputTool, TaskStopTool, TaskUpdateTool,
    TodoWriteTool,
};
pub use tool_team::team;
pub use tool_team::{TeamCreateTool, TeamDeleteTool};
pub use tool_ui::{ask_user_question, brief, send_message, sleep, synthetic_output};
pub use tool_ui::{
    AskUserQuestionTool, BriefTool, SendMessageTool, SleepTool, SyntheticOutputTool,
};
pub use tool_web::{web_fetch, web_search};
pub use tool_web::{WebFetchTool, WebSearchTool};
pub use tool_worktree::worktree;
pub use tool_worktree::{EnterWorktreeTool, ExitWorktreeTool};

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
    // M4-01 — file/search tools (M8-P5: extracted to the tool-file crate).
    tool_file::register_all(registry, ctx.clone());
    // M4-02 — shell tools.
    tool_shell::register_all(registry, ctx.clone());
    // M4-03 — web tools.
    tool_web::register_all(registry, ctx.clone());
    // M4-04 — workflow tools.
    tool_plan::register_all(registry, ctx.clone());
    tool_worktree::register_all(registry, ctx.clone());
    // M4-05 — agent + task + send_message tools.
    registry.register_builtin(Arc::new(AgentTool::new(ctx.clone())));
    tool_task::register_all(registry, ctx.clone());
    // M4-06 — team tools.
    tool_team::register_all(registry, ctx.clone());
    // M4-07 — MCP + LSP tools.
    registry.register_builtin(Arc::new(MCPTool::new(ctx.clone())));
    registry.register_builtin(Arc::new(McpAuthTool::new(ctx.clone())));
    registry.register_builtin(Arc::new(ListMcpResourcesTool::new(ctx.clone())));
    registry.register_builtin(Arc::new(ReadMcpResourceTool::new(ctx.clone())));
    tool_lsp::register_all(registry, ctx.clone());
    // M4-08 — system tools.
    tool_skill::register_all(registry, ctx.clone());
    tool_cron::register_all(registry, ctx.clone());
    tool_meta::register_all(registry, ctx.clone());
    tool_ui::register_all(registry, ctx);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin::test_support::make_dummy_fs;
    use crate::registry::ToolRegistry;
    use std::path::PathBuf;
    use telemetry::AnalyticsBus;
    // M8-P5: these moved out of the file-level imports when BuiltinToolContext
    // left this module; the dummy_ctx fixture below still constructs one.
    use api_client::AnthropicProvider;
    use permission::PermissionMode;
    use sandbox::decision::ProjectTrustLevel;
    use sandbox::runtime_config::{Platform, SandboxRuntimeConfig};

    fn dummy_ctx() -> BuiltinToolContext {
        use crate::builtin::test_support::{
            make_bypass_sandbox, make_stub_clock, make_stub_http, make_stub_process,
        };
        use traits::process::ProcessOutput;
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
    fn register_all_inserts_forty_tools_after_m4_08() {
        let mut registry = ToolRegistry::new();
        register_all_builtin_tools(&mut registry, dummy_ctx());
        let ctx = crate::tool_trait::ToolStaticContext::default();
        let tools = registry.available_tools(&ctx);
        // M4-01 (6) + M4-02 (4) + M4-03 (2) + M4-04 (5) + M4-05 (8)
        // + M4-06 (2) + M4-07 (5) + M4-08 (8) = 40.
        assert_eq!(tools.len(), 40);
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
            "AskUserQuestion",
            "Brief",
            "Config",
            "Skill",
            "ScheduleCron",
            "ToolSearch",
            "RemoteTrigger",
            "SyntheticOutput",
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
            "AskUserQuestion",
            "Brief",
            "Config",
            "Skill",
            "ScheduleCron",
            "ToolSearch",
            "RemoteTrigger",
            "SyntheticOutput",
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
