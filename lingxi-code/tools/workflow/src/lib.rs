//! `tool-workflow` — the `Workflow` tool: launch a model-authored workflow
//! script as a background task.
//!
//! claude-code's Workflow tool runs the script in the background, returning
//! immediately with `{ status: "async_launched", taskId, taskType:
//! "local_workflow" }`; a `<task-notification>` arrives when it completes. The
//! script orchestrates subagents (`agent()`/`parallel()`/`pipeline()`/…);
//! LingXi runs it on the `workflow` crate's QuickJS runtime via a
//! `LocalWorkflow` background task (`tasks::handlers::local_workflow`).
//!
//! The byte-exact model-facing surface — the tool name, the long-form
//! description ([`Tool::prompt`]), and the input schema — is reproduced from the
//! claude-code v2.1.185 binary. The launch itself goes through the injected
//! [`WorkflowLauncher`] seam, which the composition root wires over the task
//! registry; with no launcher wired the tool serves its surface but `call`
//! reports a clear error.

#![forbid(unsafe_code)]

use std::sync::Arc;

use async_trait::async_trait;
use once_cell::sync::Lazy;
use traits::env::is_env_truthy;
use permission::result::PermissionMetadata;
use permission::{PermissionDecisionReason, PermissionResult};
use serde_json::{json, Value};

use tool_api::context::ToolUseContext;
use tool_api::progress::ToolProgressSender;
use tool_api::tool_trait::{
    DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolStaticContext,
    ValidationError,
};

/// Tool name byte-lock.
pub const TOOL_NAME: &str = "Workflow";

/// The long-form tool description (claude-code v2.1.185 `prompt`), reproduced
/// byte-for-byte. A trailing newline (should an editor add one to the data file)
/// is stripped so the API description matches the binary exactly.
static DESCRIPTION: Lazy<String> = Lazy::new(|| {
    include_str!("workflow_description.txt")
        .trim_end_matches('\n')
        .to_string()
});

/// The input schema (claude-code v2.1.185 `inputSchema`), reproduced from the
/// zod `strictObject` definition (source-order properties, `additionalProperties:
/// false`, no required keys — the "at least one of script/name/scriptPath"
/// constraint is a runtime `.refine`, enforced in [`Tool::validate_input`]).
static INPUT_SCHEMA: Lazy<Value> = Lazy::new(|| {
    serde_json::from_str(include_str!("workflow_input_schema.json"))
        .expect("workflow_input_schema.json is valid JSON")
});

/// What a [`WorkflowLauncher`] needs to start a workflow run. Mirrors the
/// Workflow tool's input minus the `title`/`description` fields (which the tool
/// description marks "Ignored").
#[derive(Debug, Clone, Default)]
pub struct WorkflowLaunchSpec {
    /// Inline self-contained script source.
    pub script: Option<String>,
    /// Name of a predefined/saved workflow.
    pub name: Option<String>,
    /// Path to a script file on disk (takes precedence over `script`/`name`).
    pub script_path: Option<String>,
    /// `args` global value, passed verbatim.
    pub args: Option<Value>,
    /// Resume a prior run by its `wf_…` id.
    pub resume_from_run_id: Option<String>,
}

/// The result of a successful launch.
#[derive(Debug, Clone, Default)]
pub struct WorkflowLaunched {
    /// The background task id (claude-code `taskId`).
    pub task_id: String,
    /// The local run id for `resumeFromRunId` (claude-code `runId`). Minted at
    /// launch so the model can pass it back to resume; `None` if unavailable.
    pub run_id: Option<String>,
    /// Path to the persisted workflow script for this invocation (claude-code
    /// `scriptPath`) — editable, and passable back as `scriptPath` to re-run
    /// without resending the script. `None` if the host did not persist it.
    pub script_path: Option<String>,
    /// `meta.name` from the script (claude-code `workflowName`). `None` if not
    /// extracted.
    pub workflow_name: Option<String>,
}

/// Error launching a workflow.
#[derive(Debug)]
pub struct WorkflowLaunchError(pub String);

impl std::fmt::Display for WorkflowLaunchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for WorkflowLaunchError {}

/// Resolve a launch spec to a script source. Precedence follows claude-code:
/// `scriptPath` over `script` over `name` (the schema marks `scriptPath` as
/// "Takes precedence over `script` and `name`"). `read` loads a file's contents
/// (the host provides real I/O); `name` resolution looks under
/// `.claude/workflows/<name>` with common script extensions. (LingXi ships no
/// built-in workflow library, so a `name` that isn't a saved file is an error.)
pub fn resolve_script<R>(
    spec: &WorkflowLaunchSpec,
    read: R,
) -> Result<String, WorkflowLaunchError>
where
    R: Fn(&str) -> std::io::Result<String>,
{
    let nonempty = |o: &Option<String>| o.as_deref().filter(|s| !s.is_empty()).map(str::to_string);
    if let Some(path) = nonempty(&spec.script_path) {
        return read(&path)
            .map_err(|e| WorkflowLaunchError(format!("cannot read scriptPath '{path}': {e}")));
    }
    if let Some(script) = nonempty(&spec.script) {
        return Ok(script);
    }
    if let Some(name) = nonempty(&spec.name) {
        for ext in [".js", ".mjs", ".ts", ""] {
            if let Ok(src) = read(&format!(".claude/workflows/{name}{ext}")) {
                return Ok(src);
            }
        }
        return Err(WorkflowLaunchError(format!(
            "no saved workflow named '{name}' under .claude/workflows/"
        )));
    }
    Err(WorkflowLaunchError(
        "Must provide script, name, or scriptPath".into(),
    ))
}

/// Seam that spawns a `LocalWorkflow` background task and returns its id. The
/// composition root wires this over the task registry (keeping this crate
/// decoupled from `tasks`); tests inject a mock.
#[async_trait]
pub trait WorkflowLauncher: Send + Sync {
    /// Launch the workflow described by `spec`, returning the new task id.
    async fn launch(
        &self,
        spec: WorkflowLaunchSpec,
    ) -> Result<WorkflowLaunched, WorkflowLaunchError>;
}

/// `WorkflowTool` — launch a workflow script as a background task.
#[derive(Clone)]
pub struct WorkflowTool {
    launcher: Option<Arc<dyn WorkflowLauncher>>,
}

impl WorkflowTool {
    /// Construct. `launcher` is `None` when the host has not wired the workflow
    /// task seam — the model-facing surface is still served, but `call` errors.
    #[must_use]
    pub fn new(launcher: Option<Arc<dyn WorkflowLauncher>>) -> Self {
        Self { launcher }
    }

    fn spec_from_input(input: &Value) -> WorkflowLaunchSpec {
        let s = |k: &str| input.get(k).and_then(Value::as_str).map(str::to_string);
        WorkflowLaunchSpec {
            script: s("script"),
            name: s("name"),
            script_path: s("scriptPath"),
            args: input.get("args").cloned(),
            resume_from_run_id: s("resumeFromRunId"),
        }
    }
}

#[async_trait]
impl Tool for WorkflowTool {
    fn name(&self) -> &str {
        TOOL_NAME
    }
    fn input_schema(&self) -> &Value {
        &INPUT_SCHEMA
    }
    fn is_enabled(&self, _: &ToolStaticContext) -> bool {
        // Port of `fbn()` + `pA()` from claude-code v2.1.186 (offset 196461282).
        // `fbn()` returns true (= disable) when:
        //   `isEnvTruthy(process.env.CLAUDE_CODE_DISABLE_WORKFLOWS)` OR
        //   `$H()?.settings.disableWorkflows === true`
        //
        // LingXi: the env-var branch is implemented faithfully.
        // ⚠️ managed-setting `disableWorkflows` has no ctx seam:
        //   `ToolStaticContext` carries only `feature_flags`; the managed settings
        //   object is not threaded to `is_enabled`. The setting gate is therefore
        //   not implemented; a future refactor that adds managed-settings to
        //   `ToolStaticContext` should add the second arm.
        //
        // The org/launch (`Xs("allow_workflows")`), GrowthBook
        // (`tengu_workflows_enabled`), and plan-availability gates have no LingXi
        // backing and are treated as permissive (enabled), matching the
        // Max/Team/null-plan default.
        !is_env_truthy(std::env::var("CLAUDE_CODE_DISABLE_WORKFLOWS").ok().as_deref())
    }
    fn max_result_size_chars(&self) -> usize {
        // The result is a tiny `{status, taskId, taskType}` object.
        16384
    }
    fn is_concurrency_safe(&self, _: &Value) -> bool {
        // A workflow fans out many side-effecting agents.
        false
    }
    fn is_read_only(&self, _: &Value) -> bool {
        false
    }

    async fn check_permissions(&self, _: &Value, _: &ToolUseContext) -> PermissionResult {
        // The Workflow tool itself needs no permission gate — the subagents it
        // spawns are individually permissioned (claude-code surfaces no
        // `canUseTool` prompt for Workflow).
        PermissionResult::Allow {
            reason: PermissionDecisionReason::Other {
                reason: "Workflow launch — spawned agents are individually permissioned".into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }

    async fn description(&self, _: &Value, _: &DescriptionOptions) -> String {
        "Running a workflow".into()
    }

    async fn prompt(&self, _: &PromptOptions) -> String {
        DESCRIPTION.clone()
    }

    async fn validate_input(
        &self,
        input: &Value,
        _: &ToolUseContext,
    ) -> Result<(), ValidationError> {
        // zod `.refine(e => e.script || e.name || e.scriptPath, { message: … })`.
        let present =
            |k: &str| input.get(k).and_then(Value::as_str).is_some_and(|s| !s.is_empty());
        if !present("script") && !present("name") && !present("scriptPath") {
            return Err(ValidationError(
                "Must provide script, name, or scriptPath".into(),
            ));
        }
        Ok(())
    }

    async fn call(
        &self,
        input: Value,
        _ctx: ToolUseContext,
        _progress: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        let launcher = self.launcher.as_ref().ok_or_else(|| {
            ToolError::Internal("Workflow launching is not available in this host".into())
        })?;
        let spec = Self::spec_from_input(&input);
        let launched = launcher
            .launch(spec)
            .await
            .map_err(|e| ToolError::Internal(e.to_string()))?;
        // claude-code result shape (output schema `sUp`, local path): status,
        // taskId, taskType, plus the optional workflowName / runId / scriptPath.
        // (The "remote_launched"/"remote_agent" + sessionUrl variants are the
        // CCR/remote path, out of scope for the single-process build.) The
        // optional fields are omitted when the host did not supply them.
        let mut data = json!({
            "status": "async_launched",
            "taskId": launched.task_id,
            "taskType": "local_workflow",
        });
        let obj = data.as_object_mut().expect("json object");
        if let Some(name) = launched.workflow_name {
            obj.insert("workflowName".into(), Value::String(name));
        }
        if let Some(run_id) = launched.run_id {
            obj.insert("runId".into(), Value::String(run_id));
        }
        if let Some(path) = launched.script_path {
            obj.insert("scriptPath".into(), Value::String(path));
        }
        Ok(ToolCallResult {
            data,
            new_messages: vec![],
            context_modifier: None,
            mcp_meta: None,
        })
    }
}

/// Register the `Workflow` tool against `reg`. Without a wired launcher the
/// model-facing surface is served but `call` errors; the composition root
/// constructs [`WorkflowTool::new(Some(launcher))`] directly once the workflow
/// task seam is available.
pub fn register_all(reg: &mut tool_api::ToolRegistry, _ctx: tool_api::BuiltinToolContext) {
    reg.register_builtin(Arc::new(WorkflowTool::new(None)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    struct MockLauncher {
        task_id: String,
        seen: StdMutex<Option<WorkflowLaunchSpec>>,
    }
    impl MockLauncher {
        fn new(task_id: &str) -> Arc<Self> {
            Arc::new(Self {
                task_id: task_id.to_string(),
                seen: StdMutex::new(None),
            })
        }
    }
    #[async_trait]
    impl WorkflowLauncher for MockLauncher {
        async fn launch(
            &self,
            spec: WorkflowLaunchSpec,
        ) -> Result<WorkflowLaunched, WorkflowLaunchError> {
            *self.seen.lock().unwrap() = Some(spec);
            Ok(WorkflowLaunched {
                task_id: self.task_id.clone(),
                ..Default::default()
            })
        }
    }

    fn tool(launcher: Option<Arc<dyn WorkflowLauncher>>) -> WorkflowTool {
        WorkflowTool::new(launcher)
    }

    #[test]
    fn name_is_workflow() {
        assert_eq!(tool(None).name(), "Workflow");
    }

    #[test]
    fn resolve_script_precedence_and_name_lookup() {
        use std::collections::HashMap;
        let files: HashMap<&str, &str> = HashMap::from([
            ("/abs/wf.js", "FROM_PATH"),
            (".claude/workflows/review.js", "FROM_NAME"),
        ]);
        let read = |p: &str| {
            files
                .get(p)
                .map(|s| (*s).to_string())
                .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "nope"))
        };

        // scriptPath wins over an inline script.
        let spec = WorkflowLaunchSpec {
            script: Some("INLINE".into()),
            script_path: Some("/abs/wf.js".into()),
            ..Default::default()
        };
        assert_eq!(resolve_script(&spec, &read).unwrap(), "FROM_PATH");

        // inline script when there is no scriptPath.
        let spec = WorkflowLaunchSpec {
            script: Some("INLINE".into()),
            ..Default::default()
        };
        assert_eq!(resolve_script(&spec, &read).unwrap(), "INLINE");

        // name → .claude/workflows/<name>.js.
        let spec = WorkflowLaunchSpec {
            name: Some("review".into()),
            ..Default::default()
        };
        assert_eq!(resolve_script(&spec, &read).unwrap(), "FROM_NAME");

        // unknown name + nothing-provided → errors.
        let spec = WorkflowLaunchSpec {
            name: Some("missing".into()),
            ..Default::default()
        };
        assert!(resolve_script(&spec, &read).is_err());
        assert!(resolve_script(&WorkflowLaunchSpec::default(), &read).is_err());
    }

    #[test]
    fn description_matches_the_binary_byte_for_byte() {
        // v2.1.185 runtime length of the Workflow tool description.
        assert_eq!(DESCRIPTION.len(), 18961, "description byte length drifted");
        assert!(DESCRIPTION.starts_with(
            "Execute a workflow script that orchestrates multiple subagents deterministically."
        ));
        assert!(DESCRIPTION.ends_with("hand-author a continuation script."));
        // The ${r1e} interpolation resolved to the ▸ group marker.
        assert!(DESCRIPTION.contains("\"▸ name\" group in /workflows"));
        // No leftover raw escape sequences.
        assert!(!DESCRIPTION.contains("\\u2014"));
    }

    #[test]
    fn input_schema_is_byte_exact() {
        let s = &*INPUT_SCHEMA;
        assert_eq!(s["type"], "object");
        assert_eq!(s["additionalProperties"], false);
        assert!(s.get("required").is_none(), "no required keys");
        // Source-order properties (the zod definition order).
        let props = s["properties"].as_object().unwrap();
        let order: Vec<&String> = props.keys().collect();
        assert_eq!(
            order,
            vec![
                "script",
                "name",
                "description",
                "title",
                "args",
                "scriptPath",
                "resumeFromRunId"
            ]
        );
        assert_eq!(props["script"]["maxLength"], 524288);
        assert_eq!(props["resumeFromRunId"]["pattern"], "^wf_[a-z0-9-]{6,}$");
        // E.unknown() → `args` has no `type` constraint.
        assert!(props["args"].get("type").is_none());
        assert!(props["script"]["description"]
            .as_str()
            .unwrap()
            .starts_with("Self-contained workflow script."));
    }

    #[tokio::test]
    async fn validate_requires_one_of_script_name_or_script_path() {
        let t = tool(None);
        let ctx = tool_api::test_support::fresh_ctx();
        assert!(t.validate_input(&json!({}), &ctx).await.is_err());
        assert!(t
            .validate_input(&json!({ "title": "x" }), &ctx)
            .await
            .is_err());
        assert!(t
            .validate_input(&json!({ "script": "log('hi')" }), &ctx)
            .await
            .is_ok());
        assert!(t
            .validate_input(&json!({ "name": "review" }), &ctx)
            .await
            .is_ok());
        assert!(t
            .validate_input(&json!({ "scriptPath": "/tmp/wf.js" }), &ctx)
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn call_launches_and_returns_async_launched() {
        let launcher = MockLauncher::new("w_abc123");
        let t = tool(Some(launcher.clone()));
        let res = t
            .call(
                json!({ "script": "return 1;", "args": ["a.ts", "b.ts"] }),
                tool_api::test_support::fresh_ctx(),
                tool_api::test_support::fresh_tx(),
            )
            .await
            .expect("call ok");
        assert_eq!(res.data["status"], "async_launched");
        assert_eq!(res.data["taskId"], "w_abc123");
        assert_eq!(res.data["taskType"], "local_workflow");
        // The launcher saw the parsed spec (script + args).
        let spec = launcher.seen.lock().unwrap().clone().expect("launched");
        assert_eq!(spec.script.as_deref(), Some("return 1;"));
        assert_eq!(spec.args, Some(json!(["a.ts", "b.ts"])));
    }

    #[tokio::test]
    async fn call_without_a_launcher_errors() {
        let t = tool(None);
        let err = t
            .call(
                json!({ "script": "return 1;" }),
                tool_api::test_support::fresh_ctx(),
                tool_api::test_support::fresh_tx(),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::Internal(_)));
    }

    // is_enabled gate tests — port of `fbn()` / `pA()` local-deterministic subset.
    //
    // Env vars are process-global; all tests that touch CLAUDE_CODE_DISABLE_WORKFLOWS
    // must hold ENV_LOCK so they don't race with each other.
    use std::sync::Mutex;
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Default: no env var set → tool is enabled.
    #[test]
    fn is_enabled_default() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::remove_var("CLAUDE_CODE_DISABLE_WORKFLOWS");
        let t = tool(None);
        assert!(
            t.is_enabled(&ToolStaticContext::default()),
            "Workflow must be enabled by default"
        );
    }

    /// CLAUDE_CODE_DISABLE_WORKFLOWS=1 → tool is disabled.
    #[test]
    fn is_enabled_disabled_by_env_1() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("CLAUDE_CODE_DISABLE_WORKFLOWS", "1");
        let t = tool(None);
        let enabled = t.is_enabled(&ToolStaticContext::default());
        std::env::remove_var("CLAUDE_CODE_DISABLE_WORKFLOWS");
        assert!(!enabled, "Workflow must be disabled when env var is '1'");
    }

    /// CLAUDE_CODE_DISABLE_WORKFLOWS=true → tool is disabled.
    #[test]
    fn is_enabled_disabled_by_env_true() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("CLAUDE_CODE_DISABLE_WORKFLOWS", "true");
        let t = tool(None);
        let enabled = t.is_enabled(&ToolStaticContext::default());
        std::env::remove_var("CLAUDE_CODE_DISABLE_WORKFLOWS");
        assert!(!enabled, "Workflow must be disabled when env var is 'true'");
    }

    /// CLAUDE_CODE_DISABLE_WORKFLOWS=yes → tool is disabled.
    #[test]
    fn is_enabled_disabled_by_env_yes() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("CLAUDE_CODE_DISABLE_WORKFLOWS", "yes");
        let t = tool(None);
        let enabled = t.is_enabled(&ToolStaticContext::default());
        std::env::remove_var("CLAUDE_CODE_DISABLE_WORKFLOWS");
        assert!(!enabled, "Workflow must be disabled when env var is 'yes'");
    }

    /// CLAUDE_CODE_DISABLE_WORKFLOWS=0 (falsy) → tool remains enabled.
    #[test]
    fn is_enabled_falsy_env_value_stays_enabled() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("CLAUDE_CODE_DISABLE_WORKFLOWS", "0");
        let t = tool(None);
        let enabled = t.is_enabled(&ToolStaticContext::default());
        std::env::remove_var("CLAUDE_CODE_DISABLE_WORKFLOWS");
        assert!(enabled, "Workflow must stay enabled when env var is '0' (falsy)");
    }
}
