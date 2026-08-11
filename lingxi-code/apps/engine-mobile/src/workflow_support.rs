//! Workflow-on-mobile composition pieces (plan v3 Phase 1).
//!
//! Everything the `Workflow` tool needs to run on the mobile engine — a real
//! `TaskRegistry`, a `PoolSubagentSpawner`, and the launcher that spawns
//! `LocalWorkflow` background tasks — adapted from the desktop composition
//! root (`engine-desktop/src/lib.rs`; the launcher mirrors
//! `TaskRegistryWorkflowLauncher`, the invoker mirrors `DeferredToolInvoker`).
//! Deliberately omitted desktop seams: worktree isolation (an
//! `isolation:"worktree"` agent degrades to a plain spawn — the documented
//! fallback), LSP, and the coordinator. Subagents keep upstream interactivity
//! semantics: a one-shot spawn is `is_async=false`, so `AskUserQuestion`
//! inside a workflow agent rides the SAME shared `Arc<ToolRegistry>` (and
//! therefore the same `TuiBridgeResolver` → broker → client channel) the
//! main session uses.

use std::sync::Arc;

/// Late-bound [`traits::tool_invoker::ToolInvoker`] resolving the composition
/// cycle: the `LocalWorkflowHandler` is registered into the `TaskRegistry`
/// (needs `&mut` — BEFORE the registry is `Arc`-wrapped), yet must dispatch
/// tools through the parent's `Arc<ToolRegistry>`, which is assembled AFTER
/// the task registry exists (its `BuiltinToolContext` carries
/// `task_registry.clone()`). Constructed empty, filled exactly once with the
/// real `RegistryToolInvoker` after `tools` is built; no workflow can
/// dispatch a tool before the build returns. Mirror of the desktop
/// `DeferredToolInvoker`.
pub(crate) struct DeferredToolInvoker {
    inner: std::sync::OnceLock<Arc<dyn traits::tool_invoker::ToolInvoker>>,
}

impl DeferredToolInvoker {
    pub(crate) fn new() -> Self {
        Self {
            inner: std::sync::OnceLock::new(),
        }
    }

    /// Fill the cell with the real invoker. A second call is a no-op (the
    /// first binding wins), matching the build-once semantics.
    pub(crate) fn set(&self, invoker: Arc<dyn traits::tool_invoker::ToolInvoker>) {
        let _ = self.inner.set(invoker);
    }
}

#[async_trait::async_trait]
impl traits::tool_invoker::ToolInvoker for DeferredToolInvoker {
    async fn invoke(
        &self,
        name: &str,
        input: serde_json::Value,
        ctx: traits::tool_invoker::SubagentInvocationContext,
    ) -> Result<serde_json::Value, traits::tool_invoker::ToolInvokerError> {
        match self.inner.get() {
            Some(invoker) => invoker.invoke(name, input, ctx).await,
            None => Err(traits::tool_invoker::ToolInvokerError::Internal(
                "DeferredToolInvoker: tool dispatch attempted before build() bound the registry"
                    .to_string(),
            )),
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// The mobile [`tool_workflow::WorkflowLauncher`]: resolves + validates the
/// script, mints/reuses the run id, persists the script for
/// re-runnability, and spawns the `LocalWorkflow` task through the mobile
/// `TaskRegistry`. Adapted from the desktop `TaskRegistryWorkflowLauncher`
/// with mobile path anchors (`std::fs` is fine here — every path is inside
/// the app sandbox).
pub(crate) struct MobileWorkflowLauncher {
    pub(crate) registry: Arc<tasks::registry::TaskRegistry>,
    pub(crate) cwd: std::path::PathBuf,
    /// The lingxi home (`<app_files_root>/.claude`), anchoring
    /// `transcriptDir = <projectDir>/<sessionId>/subagents/workflows/<runId>`.
    pub(crate) lingxi_home: std::path::PathBuf,
    /// The LIVE current-session uuid (bare uuid, no `sess:` prefix), read at
    /// launch time. Mobile retargets sessions inside ONE engine — New/Resume/
    /// Clear swap the id while the orchestrator and this launcher live on —
    /// so a boot-time snapshot would anchor every later workflow's transcript
    /// under a session the user has already left.
    pub(crate) session_uuid: Arc<std::sync::Mutex<String>>,
}

#[async_trait::async_trait]
impl tool_workflow::WorkflowLauncher for MobileWorkflowLauncher {
    async fn launch(
        &self,
        spec: tool_workflow::WorkflowLaunchSpec,
    ) -> Result<tool_workflow::WorkflowLaunched, tool_workflow::WorkflowLaunchError> {
        let cwd = self.cwd.clone();
        let abs = |p: &str| -> std::path::PathBuf {
            let path = std::path::Path::new(p);
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                cwd.join(path)
            }
        };
        let script = tool_workflow::resolve_script(&spec, |p| std::fs::read_to_string(abs(p)))?;
        // Reject a malformed `meta` block at the tool boundary; the byte-exact
        // message surfaces to the model as the tool error (desktop parity).
        workflow::validate_meta(&script).map_err(|e| {
            let msg = match e {
                workflow::WorkflowError::Script(m) => m,
                other => other.to_string(),
            };
            tool_workflow::WorkflowLaunchError(msg)
        })?;
        // Determinism gate: an INLINE `script` may not use
        // Date.now()/Math.random()/new Date() (breaks resume);
        // author-controlled `scriptPath`/`name` files are exempt.
        let is_inline = spec.script.as_deref().is_some_and(|s| !s.is_empty())
            && spec
                .script_path
                .as_deref()
                .filter(|s| !s.is_empty())
                .is_none();
        if is_inline {
            if let Err(workflow::WorkflowError::Script(m)) = workflow::check_determinism(&script) {
                return Err(tool_workflow::WorkflowLaunchError(m));
            }
        }
        // Resume gate (errorCode 3): a `resumeFromRunId` naming a
        // STILL-RUNNING workflow is rejected — two runs sharing a run id
        // would race on the same journal.
        if let Some(rid) = spec.resume_from_run_id.as_deref().filter(|s| !s.is_empty()) {
            // The tool schema advertises `^wf_[a-z0-9-]{6,}$`, but nothing in
            // the workspace validates JSON-Schema `pattern` — and this id
            // becomes a FILENAME (the persisted script below, and the journal
            // in the task handler). An unchecked `../…` or absolute value
            // would write outside the scratch dir, e.g. over the workspace's
            // host-managed `lib/lingxi-bridge.js`.
            if !is_valid_run_id(rid) {
                return Err(tool_workflow::WorkflowLaunchError(format!(
                    "resumeFromRunId {rid:?} is not a workflow run id (expected wf_ followed by \
                     at least 6 lowercase alphanumerics or dashes)"
                )));
            }
            if let Some(task_id) = self.registry.find_running_workflow_by_run_id(rid).await {
                return Err(tool_workflow::WorkflowLaunchError(format!(
                    "Workflow {rid} is still running (task {task_id}). Stop it first with \
                     TaskStop({{taskId: \"{task_id}\"}}) before resuming."
                )));
            }
        }
        // Mint the run id at launch (fresh) or reuse the resume id. Host
        // clock use is fine — only the workflow SCRIPT is barred from the
        // clock. Shape: `wf_` + 8 hex + `-` + 3 hex.
        let run_id = spec
            .resume_from_run_id
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                use std::sync::atomic::{AtomicU64, Ordering};
                static WF_SEQ: AtomicU64 = AtomicU64::new(0);
                let seq = WF_SEQ.fetch_add(1, Ordering::Relaxed);
                let nanos = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos() as u64)
                    .unwrap_or(0);
                let v = nanos ^ seq.wrapping_mul(0x9e37_79b9_7f4a_7c15);
                format!("wf_{:08x}-{:03x}", (v >> 32) as u32, (v as u32) & 0xfff)
            });
        // Persist the script so it is editable + re-runnable via `scriptPath`.
        // A `scriptPath` input is already on disk → returned as-is; an
        // inline/`name` script is written under the app-sandbox scratch dir.
        let script_path = if let Some(p) = spec.script_path.as_deref().filter(|s| !s.is_empty()) {
            abs(p).to_str().map(str::to_string)
        } else {
            let dir = cwd.join(".lingxi-scratch").join("workflows");
            let file = dir.join(format!("{run_id}.js"));
            (std::fs::create_dir_all(&dir).is_ok() && std::fs::write(&file, &script).is_ok())
                .then(|| file.to_str().map(str::to_string))
                .flatten()
        };
        let workflow_name = workflow::meta_string_value(&script, "name");
        let summary = workflow::meta_string_value(&script, "description");
        let transcript_dir = {
            // Read the live cell HERE, not at construction: the session the
            // user is in when they launch a workflow is the one its
            // transcript belongs under.
            let session_uuid = self
                .session_uuid
                .lock()
                .map(|guard| guard.clone())
                .unwrap_or_default();
            let subagents = orchestrator::transcript_paths::subagents_dir(
                &self.lingxi_home,
                &self.cwd.to_string_lossy(),
                &session_uuid,
            );
            subagents
                .join("workflows")
                .join(&run_id)
                .to_str()
                .map(str::to_string)
        };
        let (invocation_mode, workflow_source) =
            if let Some(p) = spec.script_path.as_deref().filter(|s| !s.is_empty()) {
                ("scriptPath".to_string(), p.to_string())
            } else if let Some(n) = spec.name.as_deref().filter(|s| !s.is_empty()) {
                ("named".to_string(), n.to_string())
            } else {
                ("inline".to_string(), "inline".to_string())
            };
        let task_id = self
            .registry
            .spawn(
                tasks::TaskType::LocalWorkflow,
                tasks::TaskSpawnInput::LocalWorkflow {
                    workflow_id: workflow_name
                        .clone()
                        .filter(|s| !s.is_empty())
                        .or_else(|| spec.name.clone())
                        .unwrap_or_default(),
                    script,
                    resume_from_run_id: spec.resume_from_run_id.clone(),
                    args: spec
                        .args
                        .as_ref()
                        .map(|v| serde_json::to_string(v).unwrap_or_default()),
                    run_id: Some(run_id.clone()),
                    invocation_mode: Some(invocation_mode),
                    workflow_source: Some(workflow_source),
                    launched_from_subagent: false,
                },
                "Workflow".to_string(),
            )
            .await
            .map_err(|e| tool_workflow::WorkflowLaunchError(e.to_string()))?;
        Ok(tool_workflow::WorkflowLaunched {
            task_id,
            run_id: Some(run_id),
            script_path,
            workflow_name,
            summary,
            transcript_dir,
        })
    }
}

/// The `resumeFromRunId` shape the Workflow tool's schema advertises
/// (`^wf_[a-z0-9-]{6,}$`) — enforced in code because the id becomes a file
/// name and nothing in this workspace validates JSON-Schema `pattern`.
fn is_valid_run_id(value: &str) -> bool {
    let Some(rest) = value.strip_prefix("wf_") else {
        return false;
    };
    rest.len() >= 6
        && rest
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

#[cfg(test)]
mod run_id_tests {
    use super::is_valid_run_id;

    /// The minted shape is accepted; every escape shape a resume id could
    /// carry into `dir.join(format!("{run_id}.js"))` is refused.
    #[test]
    fn run_id_validation_refuses_path_escapes() {
        assert!(is_valid_run_id("wf_1a2b3c4d-0ff"));
        assert!(is_valid_run_id("wf_abcdef"));

        for bad in [
            "",
            "wf_",
            "wf_abc",
            "../../lib/lingxi-bridge",
            "wf_../../lib/lingxi-bridge",
            "/tmp/anywhere",
            "wf_/tmp/anywhere",
            "wf_ABCDEF",
            "wf_abc def",
            "wf_abc.def",
        ] {
            assert!(!is_valid_run_id(bad), "must refuse {bad:?}");
        }
    }
}
