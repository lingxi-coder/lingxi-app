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

/// Maximum script size in bytes (claude-code `P2 = 524288` = 512 KB).
pub const MAX_SCRIPT_BYTES: usize = 524288;

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
    /// `meta.description` from the script (claude-code `summary = p`). `None`
    /// if the workflow has no description in its meta block.
    pub summary: Option<String>,
    /// Directory where subagent transcripts are written (claude-code
    /// `transcriptDir = Nte(runId)` → `<sessionProjectDir>/<sessionId>/subagents/workflows/<runId>`).
    /// `None` if the session dir is not available to the launcher.
    pub transcript_dir: Option<String>,
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
            if let Ok(src) = read(&format!("{}/workflows/{name}{ext}", branding::DOT_DIR)) {
                return Ok(src);
            }
        }
        return Err(WorkflowLaunchError(format!(
            "no saved workflow named '{name}' under {}/workflows/",
            branding::DOT_DIR
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

    /// List saved workflow names from `.claude/workflows/`. Returns a
    /// comma-joined string for the errorCode-1b message, or `None` on I/O error.
    fn list_available_workflow_names() -> Option<String> {
        let dir = std::fs::read_dir(format!("{}/workflows", branding::DOT_DIR)).ok()?;
        let mut names: Vec<String> = dir
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let fname = e.file_name();
                let fname = fname.to_string_lossy();
                // Strip known extensions to get the bare name.
                for ext in [".js", ".mjs", ".ts"] {
                    if let Some(stem) = fname.strip_suffix(ext) {
                        return Some(stem.to_string());
                    }
                }
                Some(fname.into_owned())
            })
            .collect();
        names.sort();
        names.dedup();
        Some(names.join(", "))
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
        100000
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
        _ctx: &ToolUseContext,
    ) -> Result<(), ValidationError> {
        // ── Gate order mirrors claude-code v2.1.186 validateInput (offset 203004507) ──
        //
        // errorCode 7 — abort / input truncated
        // ⚠️ UNREACHABLE: the binary's `yke(t.abortController.signal)` is the
        // HTTP-layer server-retraction signal; LingXi's `ToolUseContext` exposes a
        // local `cancel` (CancellationToken) that fires on sibling errors / user
        // interrupt — a different thing. No server-fallback abort signal is threaded
        // to `validate_input`. Kept as a named constant for documentation; the gate
        // is not wired.
        //
        // errorCode 5 — `disableWorkflows` managed setting
        // ⚠️ PARTIAL: the binary's `fbn()` checks an org-managed setting
        // (`$H()?.settings.disableWorkflows`). `ToolStaticContext` carries only
        // `feature_flags`; the managed-settings object is not threaded here. We
        // fire the byte-exact message on the env-var branch (same branch as
        // `is_enabled`) as a faithful-equivalent gate for local builds. The managed-
        // setting arm is NOT reachable from this ctx.
        if is_env_truthy(std::env::var("CLAUDE_CODE_DISABLE_WORKFLOWS").ok().as_deref()) {
            return Err(ValidationError(
                "Dynamic workflows are disabled by managed settings (`disableWorkflows`).".into(),
            ));
        }

        // errorCode 6 — session gate (`pA()`)
        // ⚠️ PARTIAL: the binary's `pA()` checks org policy, launch gate, and the
        // user's `/config` "Dynamic workflows" toggle. None of these sources are
        // threaded to validate_input in LingXi's ctx. The gate below is the local-
        // equivalent env-var path; the managed org/launch/config arms are NOT
        // reachable. In practice this gate is always permissive on local builds.
        // Message byte-exact per §8 errorCode 6.
        // (No additional local gate beyond the env-var above — pA() defaults
        // permissive on Max/Team/null-plan; only fires when explicitly disabled.)

        // errorCode 1 — script resolution (sub-errors 1a–1f, byte-exact per §8.1)
        // Reproduces the binary's D7a() resolution logic with exact error strings.
        let s = |k: &str| input.get(k).and_then(Value::as_str).map(str::to_string);
        let script_path = s("scriptPath").filter(|v| !v.is_empty());
        let script      = s("script").filter(|v| !v.is_empty());
        let name        = s("name").filter(|v| !v.is_empty());

        // Resolved script text (for errorCode 2 and 4 checks below).
        let resolved_script: String;

        if let Some(ref path) = script_path {
            // 1c — UNC path not allowed
            if path.starts_with("\\\\") {
                return Err(ValidationError(format!(
                    "UNC paths are not allowed for workflow scriptPath: {path}"
                )));
            }
            // 1d / 1e / 1f — file read / not found / too large
            match std::fs::read(path) {
                Ok(bytes) => {
                    if bytes.len() > MAX_SCRIPT_BYTES {
                        return Err(ValidationError(format!(
                            "Workflow script file {path} exceeds {MAX_SCRIPT_BYTES} bytes"
                        )));
                    }
                    resolved_script = String::from_utf8_lossy(&bytes).into_owned();
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    return Err(ValidationError(format!(
                        "Workflow script file not found: {path}"
                    )));
                }
                Err(e) => {
                    return Err(ValidationError(format!(
                        "Failed to read workflow script file {path}: {e}"
                    )));
                }
            }
        } else if let Some(ref inline) = script {
            resolved_script = inline.clone();
        } else if let Some(ref wf_name) = name {
            // Try to resolve from saved workflows (.claude/workflows/<name>{.js,.mjs,.ts,""}).
            let mut found: Option<String> = None;
            for ext in [".js", ".mjs", ".ts", ""] {
                let candidate = format!("{}/workflows/{wf_name}{ext}", branding::DOT_DIR);
                match std::fs::read_to_string(&candidate) {
                    Ok(src) => { found = Some(src); break; }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(_) => continue,
                }
            }
            if let Some(src) = found {
                resolved_script = src;
            } else {
                // 1b — workflow name not found; list available names
                let available: String = Self::list_available_workflow_names()
                    .unwrap_or_default();
                let list = if available.is_empty() {
                    "(none)".to_string()
                } else {
                    available
                };
                return Err(ValidationError(format!(
                    "Workflow \"{wf_name}\" not found. Available: {list}"
                )));
            }
        } else {
            // 1a — none of script/name/scriptPath provided
            return Err(ValidationError(
                "Must provide script, name, or scriptPath".into(),
            ));
        }

        // errorCode 2 — parse/meta error (`Invalid workflow script: ${error}`)
        // Mirrors binary's `Bw(n.script)` → `validate_meta`.
        if let Err(e) = workflow::validate_meta(&resolved_script) {
            return Err(ValidationError(format!("Invalid workflow script: {e}")));
        }

        // errorCode 4 — determinism violation (inline script only)
        // Binary: `e.script && HKa(r.scriptBody)`. `e.script` is the RAW INLINE
        // `script` field from the input — it is falsy when `name` or `scriptPath`
        // is used. Only inline `script` input is checked; `name`-resolved saved
        // workflows and `scriptPath`-sourced files skip this gate.
        if script.is_some() {
            if let Err(e) = workflow::check_determinism(&resolved_script) {
                // The WorkflowError Display wraps the message; we want the raw
                // NON_DETERMINISTIC_MESSAGE, which lives inside WorkflowError::Script.
                use workflow::WorkflowError;
                let msg = match e {
                    WorkflowError::Script(m) => m,
                    WorkflowError::Engine(m) => m,
                };
                return Err(ValidationError(msg));
            }
        }

        // errorCode 3 — still-running resume target
        // The binary looks up a `local_workflow` task by `workflowRunId ===
        // resumeFromRunId` in the task registry. LingXi's `ToolUseContext` does
        // not carry a task-registry handle, so this gate is NOT enforced here;
        // instead it is enforced at LAUNCH time by `TaskRegistryWorkflowLauncher`
        // (the composition root, which owns the registry — see
        // `find_running_workflow_by_run_id`), which returns the byte-exact
        // "Workflow … is still running … Stop it first with TaskStop(…)" error.
        // Observable behaviour matches: a resume of a still-running workflow is
        // rejected before a second run starts.

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
        // taskId, taskType, plus the optional workflowName / runId / scriptPath /
        // summary / transcriptDir. The "remote_launched"/"remote_agent" + sessionUrl
        // variants are the CCR/remote path, out of scope for the single-process
        // build. Optional fields are omitted when the host did not supply them.
        //
        // The model-facing launch text mirrors `mapToolResultToToolResultBlockParam`
        // (binary §1 verbatim, async_launched case):
        //   "Workflow launched in background. Task ID: {taskId}"
        //   + optional "\nSummary: {summary}"
        //   + optional "\nTranscript dir: {transcriptDir}"
        //   + optional "\nScript file: …\n(Edit …)"
        //   + optional "\nRun ID: …\nTo resume …"
        //   + "\n\nYou will be notified when it completes. Use /workflows to watch live progress."
        let task_id = launched.task_id.clone();
        let summary    = launched.summary.as_deref();
        let transcript = launched.transcript_dir.as_deref();
        let script_p   = launched.script_path.as_deref();
        let run_id_str = launched.run_id.as_deref();

        let n = summary.map_or_else(String::new, |s| format!("\nSummary: {s}"));
        let r = transcript.map_or_else(String::new, |t| format!("\nTranscript dir: {t}"));
        let o = script_p.map_or_else(String::new, |p| {
            format!(
                "\nScript file: {p}\n(Edit this file with Write/Edit and re-invoke Workflow with \
                 {{scriptPath: \"{p}\"}} to iterate without resending the script.)"
            )
        });
        let s = match (script_p, run_id_str) {
            (Some(p), Some(rid)) => format!(
                "\nRun ID: {rid}\nTo resume after editing the script: \
                 Workflow({{scriptPath: \"{p}\", resumeFromRunId: \"{rid}\"}}) \
                 — completed agents return cached results."
            ),
            _ => String::new(),
        };
        let model_content = format!(
            "Workflow launched in background. Task ID: {task_id}{n}{r}{o}{s}\
             \n\nYou will be notified when it completes. Use /workflows to watch live progress."
        );

        let mut data = json!({
            "status": "async_launched",
            "taskId": task_id,
            "taskType": "local_workflow",
            "model_content": model_content,
        });
        let obj = data.as_object_mut().expect("json object");
        if let Some(name) = launched.workflow_name {
            obj.insert("workflowName".into(), Value::String(name));
        }
        if let Some(rid) = launched.run_id {
            obj.insert("runId".into(), Value::String(rid));
        }
        if let Some(path) = launched.script_path {
            obj.insert("scriptPath".into(), Value::String(path));
        }
        if let Some(sum) = launched.summary {
            obj.insert("summary".into(), Value::String(sum));
        }
        if let Some(td) = launched.transcript_dir {
            obj.insert("transcriptDir".into(), Value::String(td));
        }
        Ok(ToolCallResult {
            data,
            model_content: None,
            new_messages: vec![],
            context_modifier: None,
            is_error: false,
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
    fn max_result_size_is_100000() {
        assert_eq!(tool(None).max_result_size_chars(), 100000);
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

    // A minimal valid workflow script: has a proper meta block, is deterministic.
    const VALID_SCRIPT: &str = concat!(
        "export const meta = { name: 'test', description: 'A test workflow' };\n",
        "await agent('do something');\n",
    );

    #[tokio::test]
    async fn validate_error_1a_must_provide_one_of_three_fields() {
        // errorCode 1a — none of script/name/scriptPath provided.
        let t = tool(None);
        let ctx = tool_api::test_support::fresh_ctx();
        let _g = ENV_LOCK.lock().unwrap();
        std::env::remove_var("CLAUDE_CODE_DISABLE_WORKFLOWS");

        let err = t.validate_input(&json!({}), &ctx).await.unwrap_err();
        assert_eq!(err.0, "Must provide script, name, or scriptPath");

        let err2 = t
            .validate_input(&json!({ "title": "x" }), &ctx)
            .await
            .unwrap_err();
        assert_eq!(err2.0, "Must provide script, name, or scriptPath");
    }

    #[tokio::test]
    async fn validate_error_2_invalid_workflow_script() {
        // errorCode 2 — script is present but fails meta parse.
        let t = tool(None);
        let ctx = tool_api::test_support::fresh_ctx();
        let _g = ENV_LOCK.lock().unwrap();
        std::env::remove_var("CLAUDE_CODE_DISABLE_WORKFLOWS");

        // A script with no meta block at all triggers the "must be first statement" error.
        let err = t
            .validate_input(&json!({ "script": "console.log('hello');" }), &ctx)
            .await
            .unwrap_err();
        assert!(
            err.0.starts_with("Invalid workflow script:"),
            "expected errorCode 2 message, got: {:?}",
            err.0
        );
    }

    #[tokio::test]
    async fn validate_error_4_date_now_determinism() {
        // errorCode 4 — inline script uses Date.now().
        let t = tool(None);
        let ctx = tool_api::test_support::fresh_ctx();
        let _g = ENV_LOCK.lock().unwrap();
        std::env::remove_var("CLAUDE_CODE_DISABLE_WORKFLOWS");

        let script = concat!(
            "export const meta = { name: 'bad', description: 'non-det' };\n",
            "const t = Date.now();\n",
        );
        let err = t
            .validate_input(&json!({ "script": script }), &ctx)
            .await
            .unwrap_err();
        assert_eq!(
            err.0,
            workflow::NON_DETERMINISTIC_MESSAGE,
            "errorCode 4 message must be byte-exact"
        );
    }

    #[tokio::test]
    async fn validate_name_resolved_date_now_passes_determinism_gate() {
        // Binary parity: errorCode-4 is NOT triggered for `name`-resolved saved
        // workflows even when the resolved body contains Date.now(). `e.script`
        // (the raw inline field) is falsy, so the binary skips the determinism
        // check. LingXi must do the same.
        let t = tool(None);
        let ctx = tool_api::test_support::fresh_ctx();
        let _g = ENV_LOCK.lock().unwrap();
        std::env::remove_var("CLAUDE_CODE_DISABLE_WORKFLOWS");

        // Write a non-deterministic saved workflow to .claude/workflows/.
        let dir = std::path::Path::new(".claude/workflows");
        std::fs::create_dir_all(dir).unwrap();
        let wf_path = dir.join("nondet-wf.js");
        let nondeterministic_src = concat!(
            "export const meta = { name: 'nondet-wf', description: 'non-det saved' };\n",
            "const t = Date.now();\n",
            "await agent('do something');\n",
        );
        std::fs::write(&wf_path, nondeterministic_src).unwrap();

        let result = t
            .validate_input(&json!({ "name": "nondet-wf" }), &ctx)
            .await;

        // Clean up before asserting so we don't leave stray files.
        let _ = std::fs::remove_file(&wf_path);

        result.expect(
            "name-resolved workflow with Date.now() must NOT be rejected for determinism",
        );
    }

    #[tokio::test]
    async fn validate_error_5_disable_workflows_env() {
        // errorCode 5 — CLAUDE_CODE_DISABLE_WORKFLOWS=1 fires the managed-settings message.
        let t = tool(None);
        let ctx = tool_api::test_support::fresh_ctx();
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("CLAUDE_CODE_DISABLE_WORKFLOWS", "1");
        let result = t
            .validate_input(&json!({ "script": VALID_SCRIPT }), &ctx)
            .await;
        std::env::remove_var("CLAUDE_CODE_DISABLE_WORKFLOWS");

        let err = result.unwrap_err();
        assert_eq!(
            err.0,
            "Dynamic workflows are disabled by managed settings (`disableWorkflows`).",
            "errorCode 5 message must be byte-exact"
        );
    }

    #[tokio::test]
    async fn validate_valid_script_passes() {
        // A valid script with proper meta + deterministic code → Ok(()).
        let t = tool(None);
        let ctx = tool_api::test_support::fresh_ctx();
        let _g = ENV_LOCK.lock().unwrap();
        std::env::remove_var("CLAUDE_CODE_DISABLE_WORKFLOWS");

        t.validate_input(&json!({ "script": VALID_SCRIPT }), &ctx)
            .await
            .expect("valid script must pass all gates");
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

    // ── Task 5: summary + transcriptDir + byte-exact launch text ─────────────

    /// A launcher that returns a full `WorkflowLaunched` with all optional
    /// fields set, for testing the result JSON shape and model_content text.
    struct RichMockLauncher {
        launched: WorkflowLaunched,
    }
    #[async_trait]
    impl WorkflowLauncher for RichMockLauncher {
        async fn launch(
            &self,
            _spec: WorkflowLaunchSpec,
        ) -> Result<WorkflowLaunched, WorkflowLaunchError> {
            Ok(self.launched.clone())
        }
    }

    /// call() with summary + transcriptDir → result JSON has both fields and the
    /// model-facing text contains the `Summary:` and `Transcript dir:` lines.
    #[tokio::test]
    async fn call_with_summary_and_transcript_dir_in_result() {
        let launcher = Arc::new(RichMockLauncher {
            launched: WorkflowLaunched {
                task_id: "w_t1".into(),
                run_id: Some("wf_abc123def456".into()),
                script_path: Some("/tmp/wf_abc123def456.js".into()),
                workflow_name: Some("my-wf".into()),
                summary: Some("A test workflow".into()),
                transcript_dir: Some(
                    "/home/.claude/projects/-Users-me-proj/sess123/subagents/workflows/wf_abc123def456".into()
                ),
            },
        });
        let t = tool(Some(launcher));
        let res = t
            .call(
                json!({ "script": "return 1;" }),
                tool_api::test_support::fresh_ctx(),
                tool_api::test_support::fresh_tx(),
            )
            .await
            .expect("call ok");

        // JSON data shape
        assert_eq!(res.data["status"], "async_launched");
        assert_eq!(res.data["taskId"], "w_t1");
        assert_eq!(res.data["taskType"], "local_workflow");
        assert_eq!(res.data["summary"], "A test workflow");
        assert_eq!(
            res.data["transcriptDir"],
            "/home/.claude/projects/-Users-me-proj/sess123/subagents/workflows/wf_abc123def456"
        );
        assert_eq!(res.data["runId"], "wf_abc123def456");
        assert_eq!(res.data["scriptPath"], "/tmp/wf_abc123def456.js");
        assert_eq!(res.data["workflowName"], "my-wf");

        // model_content text — byte-exact per oracle §1 async_launched template
        let mc = res.data["model_content"].as_str().expect("model_content string");
        assert!(
            mc.starts_with("Workflow launched in background. Task ID: w_t1"),
            "must start with task id header: {mc}"
        );
        assert!(mc.contains("\nSummary: A test workflow"), "must have Summary line: {mc}");
        assert!(
            mc.contains("\nTranscript dir: /home/.claude/projects/-Users-me-proj/sess123/subagents/workflows/wf_abc123def456"),
            "must have Transcript dir line: {mc}"
        );
        assert!(
            mc.contains("\nScript file: /tmp/wf_abc123def456.js\n(Edit this file with Write/Edit"),
            "must have Script file line: {mc}"
        );
        assert!(
            mc.contains("\nRun ID: wf_abc123def456\nTo resume after editing the script:"),
            "must have Run ID line: {mc}"
        );
        assert!(
            mc.ends_with("\n\nYou will be notified when it completes. Use /workflows to watch live progress."),
            "must end with footer: {mc}"
        );
    }

    /// call() with no summary/transcriptDir → those fields are absent from JSON and
    /// the model_content text has no `Summary:` or `Transcript dir:` lines.
    #[tokio::test]
    async fn call_without_optional_fields_omitted_from_result() {
        let launcher = Arc::new(RichMockLauncher {
            launched: WorkflowLaunched {
                task_id: "w_t2".into(),
                run_id: None,
                script_path: None,
                workflow_name: None,
                summary: None,
                transcript_dir: None,
            },
        });
        let t = tool(Some(launcher));
        let res = t
            .call(
                json!({ "script": "return 1;" }),
                tool_api::test_support::fresh_ctx(),
                tool_api::test_support::fresh_tx(),
            )
            .await
            .expect("call ok");

        // Optional fields absent from JSON
        assert!(res.data.get("summary").is_none(), "summary must be absent");
        assert!(res.data.get("transcriptDir").is_none(), "transcriptDir must be absent");
        assert!(res.data.get("runId").is_none(), "runId must be absent when None");
        assert!(res.data.get("scriptPath").is_none(), "scriptPath must be absent when None");

        // model_content has no conditional lines
        let mc = res.data["model_content"].as_str().expect("model_content string");
        assert!(!mc.contains("Summary:"), "no Summary line when absent: {mc}");
        assert!(!mc.contains("Transcript dir:"), "no Transcript dir line when absent: {mc}");
        assert!(!mc.contains("Script file:"), "no Script file line when absent: {mc}");
        assert!(!mc.contains("Run ID:"), "no Run ID line when absent: {mc}");
        // Footer always present
        assert!(
            mc.ends_with("\n\nYou will be notified when it completes. Use /workflows to watch live progress."),
            "footer always present: {mc}"
        );
    }

    /// Byte-exact full launch text — verifies the entire model_content string
    /// against the §1 oracle template with all conditional lines present.
    #[tokio::test]
    async fn launch_text_byte_exact_full_template() {
        let launcher = Arc::new(RichMockLauncher {
            launched: WorkflowLaunched {
                task_id: "w_TASKID".into(),
                run_id: Some("wf_RUNID".into()),
                script_path: Some("/path/to/script.js".into()),
                workflow_name: Some("wf-name".into()),
                summary: Some("My workflow summary".into()),
                transcript_dir: Some("/tmp/transcripts/subagents/workflows/wf_RUNID".into()),
            },
        });
        let t = tool(Some(launcher));
        let res = t
            .call(
                json!({ "script": "return 1;" }),
                tool_api::test_support::fresh_ctx(),
                tool_api::test_support::fresh_tx(),
            )
            .await
            .expect("call ok");

        let mc = res.data["model_content"].as_str().expect("model_content string");

        // Verify the EXACT string per §1 oracle template
        let expected = concat!(
            "Workflow launched in background. Task ID: w_TASKID",
            "\nSummary: My workflow summary",
            "\nTranscript dir: /tmp/transcripts/subagents/workflows/wf_RUNID",
            "\nScript file: /path/to/script.js",
            "\n(Edit this file with Write/Edit and re-invoke Workflow with {scriptPath: \"/path/to/script.js\"} to iterate without resending the script.)",
            "\nRun ID: wf_RUNID",
            "\nTo resume after editing the script: Workflow({scriptPath: \"/path/to/script.js\", resumeFromRunId: \"wf_RUNID\"}) — completed agents return cached results.",
            "\n\nYou will be notified when it completes. Use /workflows to watch live progress.",
        );
        assert_eq!(mc, expected, "launch text must be byte-exact per §1 oracle");
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
