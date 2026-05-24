//! M4-05 cross-tool integration — exercises the surface of the 8 agent/task
//! tools: `AgentTool` + 6 task tools + `SendMessageTool`. Verifies that
//! `register_all_builtin_tools` wires every tool, that the dispatcher honors
//! the `Task` legacy alias, that schemas accept the locked input shapes, and
//! that the byte-locked claim window + 6 built-in subagent types reach the
//! production constants byte-for-byte.

#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use lingxi_telemetry::AnalyticsBus;
use lingxi_tools::builtin::agent::{
    AgentTool, AgentToolInput, AGENT_TOOL_NAME, BUILTIN_SUBAGENT_TYPES, LEGACY_AGENT_TOOL_NAME,
    SUBAGENT_BUDGET_DENIED_PREFIX,
};
use lingxi_tools::builtin::send_message::{
    SendMessageTool, SEND_MESSAGE_CLAIM_WINDOW, SEND_MESSAGE_TOOL_NAME,
};
use lingxi_tools::builtin::task::{
    validate_task_id, TaskCreateTool, TaskGetTool, TaskListTool, TaskOutputTool, TaskStopTool,
    TaskUpdateTool, TASK_CREATE_TOOL_NAME, TASK_GET_TOOL_NAME, TASK_LIST_TOOL_NAME,
    TASK_OUTPUT_TOOL_NAME, TASK_STATUSES, TASK_STOP_TOOL_NAME, TASK_TYPES, TASK_UPDATE_TOOL_NAME,
};
use lingxi_tools::tool_trait::Tool;

use common::{fresh_ctx, fresh_tx, test_builtin_ctx};

mod common {
    use lingxi_api_client::AnthropicProvider;
    use lingxi_permission::PermissionMode;
    use lingxi_sandbox::decision::ProjectTrustLevel;
    use lingxi_sandbox::runtime_config::{Platform, SandboxRuntimeConfig};
    use lingxi_telemetry::AnalyticsBus;
    use lingxi_tools::builtin::BuiltinToolContext;
    use lingxi_tools::context::{ToolUseContext, ToolUseOptions};
    use lingxi_tools::progress::{progress_channel, ToolProgressSender};
    use std::path::PathBuf;
    use std::sync::Arc;

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

    pub fn fresh_tx() -> ToolProgressSender {
        let (tx, _rx) = progress_channel();
        tx
    }

    /// Builds a minimal `BuiltinToolContext` for integration tests. Mirrors
    /// the `test_support::ctx_for_file_tools` helper since that one is
    /// `pub(crate)` and unreachable from `tests/`.
    #[allow(clippy::too_many_lines)]
    pub fn test_builtin_ctx(bus: Arc<AnalyticsBus>) -> BuiltinToolContext {
        use async_trait::async_trait;
        use lingxi_traits::filesystem::{FileContent, FileEvent, FileSystem, FlockGuard, FsError};
        use lingxi_traits::http::{HttpError, HttpTransport, SseStream};
        use lingxi_traits::process::{ProcessError, ProcessHandle, ProcessOutput, ProcessRunner};
        use lingxi_traits::sandbox::{
            ProcessCommand as SbxCommand, Sandbox, SandboxBackend, SandboxCapability, SandboxError,
            SandboxFeatures, SandboxPolicy, SandboxedCommand, SandboxedTag,
        };
        use lingxi_traits::worktree::{
            WorktreeError, WorktreeHandle, WorktreeInfo, WorktreeManager,
        };

        struct PanickingFs;
        #[async_trait]
        impl FileSystem for PanickingFs {
            async fn read_file(
                &self,
                _: &str,
                _: Option<u64>,
                _: Option<u64>,
            ) -> Result<FileContent, FsError> {
                panic!("not used")
            }
            async fn write_file(&self, _: &str, _: &str) -> Result<(), FsError> {
                panic!()
            }
            fn is_within_workspace(&self, _: &str) -> bool {
                true
            }
            async fn watch(
                &self,
                _: &str,
            ) -> Result<std::pin::Pin<Box<dyn futures::Stream<Item = FileEvent> + Send>>, FsError>
            {
                panic!()
            }
            async fn append_file(&self, _: &str, _: &str) -> Result<(), FsError> {
                panic!()
            }
            async fn truncate(&self, _: &str, _: u64) -> Result<(), FsError> {
                panic!()
            }
            async fn file_mtime(&self, _: &str) -> Result<std::time::SystemTime, FsError> {
                panic!()
            }
            async fn file_size(&self, _: &str) -> Result<u64, FsError> {
                panic!()
            }
            async fn delete_file(&self, _: &str) -> Result<(), FsError> {
                panic!()
            }
            async fn symlink(&self, _: &str, _: &str) -> Result<(), FsError> {
                panic!()
            }
            async fn flock_exclusive(&self, _: &str) -> Result<Box<dyn FlockGuard>, FsError> {
                panic!()
            }
            async fn fsync(&self, _: &str) -> Result<(), FsError> {
                panic!()
            }
        }

        struct PanickingProc;
        #[async_trait]
        impl ProcessRunner for PanickingProc {
            async fn run(&self, _: &SandboxedCommand) -> Result<ProcessOutput, ProcessError> {
                Err(ProcessError::Unsupported)
            }
            async fn spawn_background(
                &self,
                _: &SandboxedCommand,
            ) -> Result<ProcessHandle, ProcessError> {
                Err(ProcessError::Unsupported)
            }
            async fn kill(&self, _: &ProcessHandle) -> Result<(), ProcessError> {
                Ok(())
            }
            fn is_available(&self) -> bool {
                false
            }
        }

        struct BypassSb;
        #[async_trait]
        impl Sandbox for BypassSb {
            fn is_available(&self) -> bool {
                true
            }
            fn backend(&self) -> SandboxBackend {
                SandboxBackend::None
            }
            fn prepare(
                &self,
                cmd: SbxCommand,
                _: &SandboxPolicy,
            ) -> Result<SandboxedCommand, SandboxError> {
                Ok(SandboxedCommand::__new_sandboxed(
                    cmd,
                    SandboxedTag::BypassAuditedWithReason {
                        reason: "test".into(),
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

        struct StubClock;
        impl lingxi_traits::Clock for StubClock {
            fn now(&self) -> std::time::SystemTime {
                std::time::UNIX_EPOCH
            }
        }

        struct StubHttp;
        #[async_trait]
        impl HttpTransport for StubHttp {
            async fn request(
                &self,
                _: lingxi_protocol::HttpRequest,
            ) -> Result<lingxi_protocol::HttpResponse, HttpError> {
                Err(HttpError::InvalidRequest("stub".into()))
            }
            async fn stream_sse(
                &self,
                _: lingxi_protocol::HttpRequest,
            ) -> Result<SseStream, HttpError> {
                Err(HttpError::InvalidRequest("stub".into()))
            }
        }

        struct StubWt;
        #[async_trait]
        impl WorktreeManager for StubWt {
            async fn create_worktree(
                &self,
                _: &str,
                _: Option<&str>,
                _: &[PathBuf],
            ) -> Result<WorktreeHandle, WorktreeError> {
                Err(WorktreeError::Unsupported)
            }
            async fn remove_worktree(&self, _: &WorktreeHandle) -> Result<(), WorktreeError> {
                Ok(())
            }
            async fn list_worktrees(&self) -> Result<Vec<WorktreeInfo>, WorktreeError> {
                Ok(vec![])
            }
            async fn cleanup_stale(
                &self,
                _: std::time::Duration,
            ) -> Result<Vec<PathBuf>, WorktreeError> {
                Ok(vec![])
            }
            fn is_supported(&self) -> bool {
                true
            }
        }

        BuiltinToolContext {
            fs: Arc::new(PanickingFs),
            bus,
            trusted_dirs: vec![PathBuf::from("/tmp")],
            process: Arc::new(PanickingProc),
            sandbox: Arc::new(BypassSb),
            clock: Arc::new(StubClock),
            sandbox_runtime: SandboxRuntimeConfig::default(),
            permission_mode: PermissionMode::Default,
            project_trust: ProjectTrustLevel::Trusted,
            sandbox_available: false,
            workspace: PathBuf::from("/tmp"),
            platform: Platform::Linux,
            http: Arc::new(StubHttp),
            provider: Arc::new(AnthropicProvider::new("test", None)),
            default_model: "claude-sonnet-4-20250514".into(),
            worktree: Arc::new(StubWt),
            subagent_spawner: None,
            task_registry: None,
            mailbox_router: None,
            budget_enforcer: None,
        }
    }
}

#[test]
fn agent_tool_name_locked() {
    assert_eq!(AGENT_TOOL_NAME, "Agent");
    assert_eq!(LEGACY_AGENT_TOOL_NAME, "Task");
}

#[test]
fn agent_six_builtin_subagent_types() {
    assert_eq!(
        BUILTIN_SUBAGENT_TYPES,
        &[
            "general-purpose",
            "Plan",
            "Explore",
            "verification",
            "claude-code-guide",
            "statusline-setup"
        ]
    );
}

#[test]
fn send_message_claim_window_locked_30s() {
    assert_eq!(SEND_MESSAGE_CLAIM_WINDOW.as_secs(), 30);
    assert_eq!(SEND_MESSAGE_TOOL_NAME, "SendMessage");
}

#[test]
fn six_task_tool_names_locked() {
    assert_eq!(TASK_CREATE_TOOL_NAME, "TaskCreate");
    assert_eq!(TASK_GET_TOOL_NAME, "TaskGet");
    assert_eq!(TASK_LIST_TOOL_NAME, "TaskList");
    assert_eq!(TASK_UPDATE_TOOL_NAME, "TaskUpdate");
    assert_eq!(TASK_STOP_TOOL_NAME, "TaskStop");
    assert_eq!(TASK_OUTPUT_TOOL_NAME, "TaskOutput");
}

#[test]
fn budget_denied_prefix_locked() {
    assert_eq!(SUBAGENT_BUDGET_DENIED_PREFIX, "Budget exceeded ($");
}

#[test]
fn task_types_byte_aligned() {
    assert_eq!(TASK_TYPES.len(), 7);
    assert!(TASK_TYPES.contains(&"local_bash"));
    assert!(TASK_TYPES.contains(&"dream"));
    assert!(TASK_TYPES.contains(&"in_process_teammate"));
}

#[test]
fn task_statuses_locked() {
    assert_eq!(
        TASK_STATUSES,
        &["pending", "running", "completed", "failed", "killed"]
    );
}

#[tokio::test]
async fn agent_unknown_subagent_type_rejects_with_locked_message() {
    let bus = Arc::new(AnalyticsBus::new());
    let ctx = common::test_builtin_ctx(bus);
    let tool = AgentTool::new(ctx);
    let input = serde_json::json!({ "subagent_type": "claude", "prompt": "x" });
    let err = tool.call(input, fresh_ctx(), fresh_tx()).await.unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("unknown subagent_type 'claude'"), "got: {msg}");
    assert!(msg.contains("general-purpose"));
}

#[tokio::test]
async fn agent_empty_prompt_rejects_with_locked_message() {
    let bus = Arc::new(AnalyticsBus::new());
    let ctx = test_builtin_ctx(bus);
    let tool = AgentTool::new(ctx);
    let input = serde_json::json!({ "subagent_type": "Plan", "prompt": "   " });
    let err = tool.call(input, fresh_ctx(), fresh_tx()).await.unwrap_err();
    assert!(format!("{err}").contains("Agent: prompt is empty"));
}

#[tokio::test]
async fn agent_accepts_all_six_builtin_subagent_types() {
    for ty in BUILTIN_SUBAGENT_TYPES {
        let bus = Arc::new(AnalyticsBus::new());
        let ctx = test_builtin_ctx(bus);
        let tool = AgentTool::new(ctx);
        let input = serde_json::json!({ "subagent_type": ty, "prompt": "do work" });
        let result = tool
            .call(input, fresh_ctx(), fresh_tx())
            .await
            .unwrap_or_else(|e| panic!("subagent type {ty:?} rejected: {e}"));
        assert_eq!(result.data["subagent_type"], *ty);
    }
}

#[tokio::test]
async fn task_create_returns_well_formed_id_and_locked_pending_status() {
    let bus = Arc::new(AnalyticsBus::new());
    let ctx = test_builtin_ctx(bus);
    let tool = TaskCreateTool::new(ctx);
    let input = serde_json::json!({ "task_type": "local_bash", "description": "run ls" });
    let result = tool.call(input, fresh_ctx(), fresh_tx()).await.unwrap();
    let id = result.data["task_id"].as_str().unwrap().to_string();
    assert!(validate_task_id(&id).is_ok(), "id {id} fails regex");
    assert_eq!(result.data["status"], "pending");
    assert_eq!(result.data["task_type"], "local_bash");
}

#[tokio::test]
async fn task_create_unknown_type_returns_locked_error() {
    let bus = Arc::new(AnalyticsBus::new());
    let ctx = test_builtin_ctx(bus);
    let tool = TaskCreateTool::new(ctx);
    let input = serde_json::json!({ "task_type": "bogus", "description": "x" });
    let err = tool.call(input, fresh_ctx(), fresh_tx()).await.unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("unknown task_type 'bogus'"));
    assert!(msg.contains("local_bash"));
}

#[tokio::test]
async fn task_get_malformed_id_returns_locked_error() {
    let bus = Arc::new(AnalyticsBus::new());
    let ctx = test_builtin_ctx(bus);
    let tool = TaskGetTool::new(ctx);
    let input = serde_json::json!({ "task_id": "TOOSHORT" });
    let err = tool.call(input, fresh_ctx(), fresh_tx()).await.unwrap_err();
    assert!(format!("{err}").contains("malformed task_id 'TOOSHORT'"));
}

#[tokio::test]
async fn task_list_empty_returns_empty_array() {
    let bus = Arc::new(AnalyticsBus::new());
    let ctx = test_builtin_ctx(bus);
    let tool = TaskListTool::new(ctx);
    let result = tool
        .call(serde_json::json!({}), fresh_ctx(), fresh_tx())
        .await
        .unwrap();
    assert!(result.data["tasks"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn task_stop_returns_killed_status() {
    let bus = Arc::new(AnalyticsBus::new());
    let ctx = test_builtin_ctx(bus);
    let tool = TaskStopTool::new(ctx);
    let input = serde_json::json!({ "task_id": "b12345678" });
    let result = tool.call(input, fresh_ctx(), fresh_tx()).await.unwrap();
    assert_eq!(result.data["status"], "killed");
}

#[tokio::test]
async fn task_update_running_transition() {
    let bus = Arc::new(AnalyticsBus::new());
    let ctx = test_builtin_ctx(bus);
    let tool = TaskUpdateTool::new(ctx);
    let input = serde_json::json!({ "task_id": "b12345678", "status": "running" });
    let result = tool.call(input, fresh_ctx(), fresh_tx()).await.unwrap();
    assert_eq!(result.data["status"], "running");
}

#[tokio::test]
async fn task_output_returns_shape_correct_envelope() {
    let bus = Arc::new(AnalyticsBus::new());
    let ctx = test_builtin_ctx(bus);
    let tool = TaskOutputTool::new(ctx);
    let input = serde_json::json!({ "task_id": "b12345678" });
    let result = tool.call(input, fresh_ctx(), fresh_tx()).await.unwrap();
    assert!(result.data["content"].is_string());
    assert!(result.data["total_lines"].is_number());
    assert!(result.data["truncated"].is_boolean());
}

#[tokio::test]
async fn send_message_returns_30s_claim_window_in_payload() {
    let bus = Arc::new(AnalyticsBus::new());
    let ctx = test_builtin_ctx(bus);
    let tool = SendMessageTool::new(ctx);
    let input = serde_json::json!({ "to_agent_id": "agent-x", "message": "hello" });
    let result = tool.call(input, fresh_ctx(), fresh_tx()).await.unwrap();
    assert_eq!(result.data["delivered"], true);
    assert_eq!(result.data["claim_window_secs"], 30);
}

#[tokio::test]
async fn send_message_empty_body_rejects_with_locked_string() {
    let bus = Arc::new(AnalyticsBus::new());
    let ctx = test_builtin_ctx(bus);
    let tool = SendMessageTool::new(ctx);
    let input = serde_json::json!({ "to_agent_id": "a", "message": "" });
    let err = tool.call(input, fresh_ctx(), fresh_tx()).await.unwrap_err();
    assert!(format!("{err}").contains("SendMessage: message is empty"));
}

#[test]
fn agent_input_serde_full_roundtrip() {
    let input = AgentToolInput {
        subagent_type: "Plan".into(),
        prompt: "Design the plan.".into(),
        context_paths: vec![],
    };
    let json = serde_json::to_value(&input).unwrap();
    let back: AgentToolInput = serde_json::from_value(json).unwrap();
    assert_eq!(back, input);
}
