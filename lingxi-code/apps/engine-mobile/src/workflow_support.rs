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

#[derive(Debug, Clone)]
struct WorkflowCheckpoint {
    task_id: String,
    workflow_run_id: String,
    workflow_id: String,
    script_path: String,
    script_sha256: Option<String>,
    script_is_verbatim_builtin: Option<bool>,
    args_json: Option<String>,
    description: String,
    start_time: Option<u64>,
    transcript_dir: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LocalAppResumeProvenance {
    /// A pre-marker checkpoint for a local-app workflow. It is trusted as
    /// local-app provenance, but its old script body must not be executed.
    LegacyBuiltin,
    /// A current checkpoint whose launcher marked the script as bundled.
    CurrentBuiltin,
    /// A trusted checkpoint for a custom script that reused a local-app name.
    Custom,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LocalAppResumeResolution {
    provenance: LocalAppResumeProvenance,
    workflow_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WorkflowProvenanceRecord {
    workflow_id: String,
    script_sha256: String,
    script_is_verbatim_builtin: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum WorkflowProvenanceLookup {
    Missing,
    Invalid { workflow_id: Option<String> },
    Valid(WorkflowProvenanceRecord),
}

const WORKFLOW_PROVENANCE_VERSION: u64 = 1;
const WORKFLOW_PROVENANCE_FILE: &str = "provenance.json";

/// Events emitted by a workflow worker before its launcher has finished
/// persisting the task/session ownership checkpoint.  The task registry starts
/// the handler before returning the generated task id, so this short handoff
/// window is real even though the worker waits for registry registration.
#[derive(Debug, Clone)]
enum PendingWorkflowEvent {
    Status(tasks::TaskStatus),
    Progress {
        run_id: String,
        progress: tasks::handlers::local_workflow::WorkflowProgressUpdate,
    },
}

#[derive(Debug, Clone)]
struct WorkflowAdoptFile {
    written_at_ms: u64,
    origin: String,
    shells: Vec<serde_json::Value>,
    cron: Vec<serde_json::Value>,
    workflows: Vec<WorkflowCheckpoint>,
    agents: Vec<serde_json::Value>,
}

/// Session-scoped workflow handoff store. Mobile cannot rely on a graceful
/// process-exit hook, so it keeps Claude Code's `adopt.json` payload current
/// from launch until a terminal status. A later process adopts it as `Paused`.
pub(crate) struct MobileWorkflowCheckpointStore {
    lingxi_home: std::path::PathBuf,
    cwd: std::path::PathBuf,
    lock: std::sync::Mutex<()>,
    task_runs: std::sync::Mutex<std::collections::HashMap<String, (String, String)>>,
    pending_events: std::sync::Mutex<std::collections::HashMap<String, Vec<PendingWorkflowEvent>>>,
}

impl MobileWorkflowCheckpointStore {
    pub(crate) fn new(lingxi_home: std::path::PathBuf, cwd: std::path::PathBuf) -> Self {
        Self {
            lingxi_home,
            cwd,
            lock: std::sync::Mutex::new(()),
            task_runs: std::sync::Mutex::new(std::collections::HashMap::new()),
            pending_events: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    fn session_dir(&self, session_uuid: &str) -> std::path::PathBuf {
        orchestrator::transcript_paths::subagents_dir(
            &self.lingxi_home,
            &self.cwd.to_string_lossy(),
            session_uuid,
        )
        .parent()
        .expect("subagents directory has a session parent")
        .to_path_buf()
    }

    fn path(&self, session_uuid: &str) -> std::path::PathBuf {
        self.session_dir(session_uuid).join("adopt.json")
    }

    fn track_task_owner(
        &self,
        task_id: &str,
        session_uuid: &str,
        run_id: &str,
    ) -> Vec<PendingWorkflowEvent> {
        let mut task_runs = self
            .task_runs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        task_runs.retain(|_, (existing_session, existing_run)| {
            existing_session != session_uuid || existing_run != run_id
        });
        task_runs.insert(
            task_id.to_string(),
            (session_uuid.to_string(), run_id.to_string()),
        );
        self.pending_events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(task_id)
            .unwrap_or_default()
    }

    fn buffer_event(&self, task_id: &str, event: PendingWorkflowEvent) {
        self.pending_events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entry(task_id.to_string())
            .or_default()
            .push(event);
    }

    fn task_owner(&self, task_id: &str) -> Option<(String, String)> {
        self.task_runs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(task_id)
            .cloned()
    }

    /// Classify a resumed script using host-owned provenance from the CURRENT
    /// session. All launch coordinates must match: workflow run id, persisted
    /// script path, session, and recorded script hash. A missing
    /// `scriptIsVerbatimBuiltin` marker is intentionally classified as a legacy
    /// local-app workflow; it must be rejected before execution rather than
    /// relying on the newer JS argument gate. The supplied resume body is not
    /// compared with the recorded hash here: editing a script must preserve
    /// the checkpoint's marker so the caller cannot turn a bundled row into an
    /// untrusted custom workflow by changing its bytes.
    fn local_app_resume_record(
        &self,
        session_uuid: &str,
        run_id: &str,
        script_path: &std::path::Path,
    ) -> Option<WorkflowProvenanceRecord> {
        if session_uuid.is_empty() || !tool_workflow::is_valid_run_id(run_id) {
            return None;
        }
        let Some(script_path) = script_path.to_str() else {
            return None;
        };
        let script_path = std::path::Path::new(script_path);
        self.read(session_uuid)
            .workflows
            .into_iter()
            .find_map(|checkpoint| {
                if checkpoint
                    .script_sha256
                    .as_deref()
                    .is_none_or(|hash| !is_sha256_hex(hash))
                {
                    return None;
                }
                if !(tool_workflow::LOCAL_APP_BUILD_WORKFLOWS
                    .contains(&checkpoint.workflow_id.as_str())
                    && checkpoint.workflow_run_id == run_id
                    && paths_equivalent(script_path, std::path::Path::new(&checkpoint.script_path)))
                {
                    return None;
                }
                Some(WorkflowProvenanceRecord {
                    workflow_id: checkpoint.workflow_id,
                    script_sha256: checkpoint.script_sha256.expect("validated script hash"),
                    script_is_verbatim_builtin: checkpoint.script_is_verbatim_builtin,
                })
            })
    }

    fn local_app_resume_provenance(
        &self,
        session_uuid: &str,
        run_id: &str,
        script_path: &std::path::Path,
    ) -> Option<LocalAppResumeProvenance> {
        self.local_app_resume_record(session_uuid, run_id, script_path)
            .map(|record| {
                let hash_is_current = is_current_local_app_builtin_hash(&record.script_sha256);
                match record.script_is_verbatim_builtin {
                    Some(true) if hash_is_current => LocalAppResumeProvenance::CurrentBuiltin,
                    Some(false) if !hash_is_current => LocalAppResumeProvenance::Custom,
                    Some(true) | Some(false) | None => LocalAppResumeProvenance::LegacyBuiltin,
                }
            })
    }

    /// Compatibility predicate for focused tests and callers that only need to
    /// know whether the checkpoint belongs to a local-app workflow.
    fn is_trusted_local_app_resume(
        &self,
        session_uuid: &str,
        run_id: &str,
        script_path: &std::path::Path,
    ) -> bool {
        self.local_app_resume_provenance(session_uuid, run_id, script_path)
            .is_some()
    }

    fn persisted_workflow_script_path(
        &self,
        session_uuid: &str,
        run_id: &str,
    ) -> std::path::PathBuf {
        self.session_dir(session_uuid)
            .join("workflows")
            .join(format!("{run_id}.js"))
    }

    fn is_host_owned_workflow_script(
        &self,
        session_uuid: &str,
        run_id: &str,
        script_path: &std::path::Path,
    ) -> bool {
        if session_uuid.is_empty() || !tool_workflow::is_valid_run_id(run_id) {
            return false;
        }
        paths_equivalent(
            script_path,
            &self.persisted_workflow_script_path(session_uuid, run_id),
        )
    }

    fn provenance_path(&self, session_uuid: &str, run_id: &str) -> std::path::PathBuf {
        self.session_dir(session_uuid)
            .join("subagents")
            .join("workflows")
            .join(run_id)
            .join(WORKFLOW_PROVENANCE_FILE)
    }

    fn read_provenance_sidecar(
        &self,
        session_uuid: &str,
        run_id: &str,
        script_path: &std::path::Path,
    ) -> WorkflowProvenanceLookup {
        if session_uuid.is_empty() || !tool_workflow::is_valid_run_id(run_id) {
            return WorkflowProvenanceLookup::Missing;
        }
        let path = self.provenance_path(session_uuid, run_id);
        let contents = match std::fs::read_to_string(path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return WorkflowProvenanceLookup::Missing;
            }
            Err(_) => return WorkflowProvenanceLookup::Invalid { workflow_id: None },
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&contents) else {
            return WorkflowProvenanceLookup::Invalid { workflow_id: None };
        };
        let workflow_id = value
            .get("workflowId")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        let Some(recorded_script_path) = value
            .get("scriptPath")
            .and_then(serde_json::Value::as_str)
            .filter(|path| !path.is_empty())
        else {
            return WorkflowProvenanceLookup::Invalid { workflow_id };
        };
        let Some(script_sha256) = value
            .get("scriptSha256")
            .and_then(serde_json::Value::as_str)
            .filter(|hash| is_sha256_hex(hash))
        else {
            return WorkflowProvenanceLookup::Invalid { workflow_id };
        };
        let valid = value.get("version").and_then(serde_json::Value::as_u64)
            == Some(WORKFLOW_PROVENANCE_VERSION)
            && value.get("sessionUuid").and_then(serde_json::Value::as_str) == Some(session_uuid)
            && value
                .get("workflowRunId")
                .and_then(serde_json::Value::as_str)
                == Some(run_id)
            && workflow_id.as_deref().is_some_and(|id| !id.is_empty())
            && paths_equivalent(script_path, std::path::Path::new(recorded_script_path));
        if !valid {
            return WorkflowProvenanceLookup::Invalid { workflow_id };
        }
        WorkflowProvenanceLookup::Valid(WorkflowProvenanceRecord {
            workflow_id: workflow_id.expect("validated workflow id"),
            script_sha256: script_sha256.to_string(),
            script_is_verbatim_builtin: value
                .get("scriptIsVerbatimBuiltin")
                .and_then(serde_json::Value::as_bool),
        })
    }

    fn write_provenance_sidecar(
        &self,
        session_uuid: &str,
        run_id: &str,
        script_path: &std::path::Path,
        script: &str,
        workflow_id: &str,
        script_is_verbatim_builtin: bool,
    ) -> std::io::Result<()> {
        let path = self.provenance_path(session_uuid, run_id);
        let parent = path.parent().expect("provenance path has parent");
        std::fs::create_dir_all(parent)?;
        let recorded_script_path = std::fs::canonicalize(script_path)
            .unwrap_or_else(|_| script_path.to_path_buf())
            .to_string_lossy()
            .into_owned();
        let value = serde_json::json!({
            "version": WORKFLOW_PROVENANCE_VERSION,
            "sessionUuid": session_uuid,
            "workflowRunId": run_id,
            "scriptPath": recorded_script_path,
            "scriptSha256": sha256_hex(script.as_bytes()),
            "workflowId": workflow_id,
            "scriptIsVerbatimBuiltin": script_is_verbatim_builtin,
        });
        let temp = path.with_extension("json.tmp");
        let json = serde_json::to_vec_pretty(&value).map_err(std::io::Error::other)?;
        std::fs::write(&temp, json)?;
        std::fs::rename(temp, path)
    }

    fn read(&self, session_uuid: &str) -> WorkflowAdoptFile {
        let empty = || WorkflowAdoptFile {
            written_at_ms: unix_time_ms(),
            origin: "exit".to_string(),
            shells: Vec::new(),
            cron: Vec::new(),
            workflows: Vec::new(),
            agents: Vec::new(),
        };
        let Some(value) = std::fs::read_to_string(self.path(session_uuid))
            .ok()
            .and_then(|json| serde_json::from_str::<serde_json::Value>(&json).ok())
        else {
            return empty();
        };
        let workflows = value
            .get("workflows")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|checkpoint| {
                Some(WorkflowCheckpoint {
                    task_id: checkpoint.get("taskId")?.as_str()?.to_string(),
                    workflow_run_id: checkpoint.get("workflowRunId")?.as_str()?.to_string(),
                    script_path: checkpoint.get("scriptPath")?.as_str()?.to_string(),
                    script_sha256: checkpoint
                        .get("scriptSha256")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string),
                    script_is_verbatim_builtin: checkpoint
                        .get("scriptIsVerbatimBuiltin")
                        .and_then(serde_json::Value::as_bool),
                    args_json: checkpoint
                        .get("argsJson")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string),
                    workflow_id: checkpoint
                        .get("workflowId")
                        .and_then(serde_json::Value::as_str)
                        // Checkpoints written before workflow identity was
                        // split used description for both fields.
                        .or_else(|| {
                            checkpoint
                                .get("description")
                                .and_then(serde_json::Value::as_str)
                        })?
                        .to_string(),
                    description: checkpoint.get("description")?.as_str()?.to_string(),
                    start_time: checkpoint
                        .get("startTime")
                        .and_then(serde_json::Value::as_u64),
                    transcript_dir: checkpoint.get("transcriptDir")?.as_str()?.to_string(),
                })
            })
            .collect();
        WorkflowAdoptFile {
            written_at_ms: value
                .get("writtenAtMs")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or_else(unix_time_ms),
            origin: value
                .get("origin")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("exit")
                .to_string(),
            shells: value
                .get("shells")
                .and_then(serde_json::Value::as_array)
                .cloned()
                .unwrap_or_default(),
            cron: value
                .get("cron")
                .and_then(serde_json::Value::as_array)
                .cloned()
                .unwrap_or_default(),
            workflows,
            agents: value
                .get("agents")
                .and_then(serde_json::Value::as_array)
                .cloned()
                .unwrap_or_default(),
        }
    }

    fn write(&self, session_uuid: &str, mut file: WorkflowAdoptFile) -> std::io::Result<()> {
        let path = self.path(session_uuid);
        if file.workflows.is_empty()
            && file.shells.is_empty()
            && file.cron.is_empty()
            && file.agents.is_empty()
        {
            return match std::fs::remove_file(path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error),
            };
        }
        file.written_at_ms = unix_time_ms();
        file.origin = "exit".to_string();
        let parent = path.parent().expect("adopt file has parent");
        std::fs::create_dir_all(parent)?;
        let temp = parent.join("adopt.json.tmp");
        let workflows = file
            .workflows
            .into_iter()
            .map(|checkpoint| {
                let mut value = serde_json::Map::new();
                value.insert("taskId".into(), checkpoint.task_id.into());
                value.insert("workflowRunId".into(), checkpoint.workflow_run_id.into());
                value.insert("workflowId".into(), checkpoint.workflow_id.into());
                value.insert("scriptPath".into(), checkpoint.script_path.into());
                if let Some(hash) = checkpoint.script_sha256 {
                    value.insert("scriptSha256".into(), hash.into());
                }
                if let Some(verbatim) = checkpoint.script_is_verbatim_builtin {
                    value.insert("scriptIsVerbatimBuiltin".into(), verbatim.into());
                }
                if let Some(args) = checkpoint.args_json {
                    value.insert("argsJson".into(), args.into());
                }
                value.insert("description".into(), checkpoint.description.into());
                if let Some(start_time) = checkpoint.start_time {
                    value.insert("startTime".into(), start_time.into());
                }
                value.insert("transcriptDir".into(), checkpoint.transcript_dir.into());
                serde_json::Value::Object(value)
            })
            .collect::<Vec<_>>();
        let json = serde_json::to_vec_pretty(&serde_json::json!({
            "writtenAtMs": file.written_at_ms,
            "origin": file.origin,
            "shells": file.shells,
            "cron": file.cron,
            "workflows": workflows,
            "agents": file.agents,
        }))
        .map_err(std::io::Error::other)?;
        std::fs::write(&temp, json)?;
        std::fs::rename(temp, path)
    }

    fn upsert(
        &self,
        session_uuid: &str,
        checkpoint: WorkflowCheckpoint,
    ) -> std::io::Result<Vec<PendingWorkflowEvent>> {
        let _guard = self
            .lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut file = self.read(session_uuid);
        file.workflows
            .retain(|existing| existing.workflow_run_id != checkpoint.workflow_run_id);
        let task_id = checkpoint.task_id.clone();
        let workflow_run_id = checkpoint.workflow_run_id.clone();
        file.workflows.push(checkpoint);
        match self.write(session_uuid, file) {
            Ok(()) => Ok(self.track_task_owner(&task_id, session_uuid, &workflow_run_id)),
            Err(error) => Err(error),
        }
    }

    pub(crate) fn remove_task(&self, task_id: &str) {
        let _guard = self
            .lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some((session_uuid, run_id)) = self
            .task_runs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(task_id)
            .cloned()
        else {
            return;
        };
        let mut file = self.read(&session_uuid);
        file.workflows
            .retain(|checkpoint| checkpoint.workflow_run_id != run_id);
        match self.write(&session_uuid, file) {
            Ok(()) => {
                // Remove the in-memory owner only after the durable file has
                // been updated. If the write failed, a later terminal event
                // or shutdown retry can still find the checkpoint and clean
                // it up instead of silently orphaning it for adoption.
                self.task_runs
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .remove(task_id);
            }
            Err(error) => {
                tracing::warn!(%error, task_id, "could not remove workflow checkpoint; retaining owner for retry");
            }
        }
    }

    pub(crate) async fn adopt_session(
        &self,
        session_uuid: &str,
        registry: &tasks::registry::TaskRegistry,
    ) {
        let checkpoints = {
            let _guard = self
                .lock
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            self.read(session_uuid).workflows
        };
        let expected_root = self
            .session_dir(session_uuid)
            .join("subagents")
            .join("workflows");
        for checkpoint in checkpoints {
            let valid_task_id = checkpoint.task_id.len() == 9
                && checkpoint.task_id.starts_with('w')
                && checkpoint
                    .task_id
                    .bytes()
                    .skip(1)
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit());
            let valid_run_id = tool_workflow::is_valid_run_id(&checkpoint.workflow_run_id);
            let script = std::fs::read(&checkpoint.script_path).ok();
            let valid_hash = match (&script, checkpoint.script_sha256.as_deref()) {
                (Some(script), Some(expected)) => sha256_hex(script) == expected,
                (Some(_), None) => true,
                _ => false,
            };
            let transcript_dir = std::path::PathBuf::from(&checkpoint.transcript_dir);
            let valid_transcript = std::fs::canonicalize(&expected_root)
                .ok()
                .zip(std::fs::canonicalize(&transcript_dir).ok())
                .is_some_and(|(root, directory)| directory.starts_with(root))
                && transcript_dir.join("journal.jsonl").is_file();
            let valid_args = checkpoint
                .args_json
                .as_deref()
                .is_none_or(|json| serde_json::from_str::<serde_json::Value>(json).is_ok());
            if !(valid_task_id && valid_run_id && valid_hash && valid_transcript && valid_args) {
                tracing::warn!(
                    task_id = %checkpoint.task_id,
                    run_id = %checkpoint.workflow_run_id,
                    "ignored invalid workflow checkpoint"
                );
                continue;
            }
            let start_time = checkpoint
                .start_time
                .map(|millis| std::time::UNIX_EPOCH + std::time::Duration::from_millis(millis))
                .unwrap_or_else(std::time::SystemTime::now);
            let adopted = tasks::registry::AdoptedWorkflow {
                task_id: checkpoint.task_id.clone(),
                session_uuid: Some(session_uuid.to_string()),
                workflow_id: checkpoint.workflow_id.clone(),
                run_id: checkpoint.workflow_run_id.clone(),
                script_path: checkpoint.script_path.clone(),
                args: checkpoint.args_json.clone(),
                transcript_dir: checkpoint.transcript_dir.clone(),
                description: checkpoint.description.clone(),
                start_time,
            };
            if let Err(error) = registry.register_adopted_workflow(adopted).await {
                tracing::warn!(%error, "could not register adopted workflow");
                continue;
            }
            self.task_runs
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(
                    checkpoint.task_id,
                    (session_uuid.to_string(), checkpoint.workflow_run_id),
                );
        }
    }
}

fn unix_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    format!("{:x}", sha2::Sha256::digest(bytes))
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn paths_equivalent(left: &std::path::Path, right: &std::path::Path) -> bool {
    left == right
        || left
            .canonicalize()
            .ok()
            .zip(right.canonicalize().ok())
            .is_some_and(|(left, right)| left == right)
}

/// Mobile workflow status adapter: persist every worker transition in the task
/// registry, then publish the same transition to the native conversation UI.
///
/// The registry sink remains the source of truth. This wrapper only closes the
/// previously missing engine-to-client leg; without it a completed workflow was
/// visible to `TaskList` but never produced the existing `TaskStatusChanged`
/// event unless the user stopped it manually.
pub(crate) struct MobileWorkflowStatusSink {
    registry: Arc<tasks::registry_status_sink::RegistryStatusSink>,
    event_sink: Arc<dyn client_adapter::ClientEventSink>,
    listener: Arc<dyn client_adapter::ClientEventListener>,
    checkpoints: Arc<MobileWorkflowCheckpointStore>,
    active_session_uuid: Arc<std::sync::Mutex<String>>,
}

impl MobileWorkflowStatusSink {
    pub(crate) fn new(
        listener: Arc<dyn client_adapter::ClientEventListener>,
        checkpoints: Arc<MobileWorkflowCheckpointStore>,
        active_session_uuid: Arc<std::sync::Mutex<String>>,
    ) -> Self {
        Self {
            registry: Arc::new(tasks::registry_status_sink::RegistryStatusSink::new()),
            event_sink: client_adapter::ListenerSink::arc(listener.clone()),
            listener,
            checkpoints,
            active_session_uuid,
        }
    }

    pub(crate) fn bind(&self, registry: Arc<tasks::registry::TaskRegistry>) {
        self.registry.bind(registry);
    }

    fn is_active_origin(&self, origin_session_id: &str) -> bool {
        self.active_session_uuid
            .lock()
            .map(|active| active.as_str() == origin_session_id)
            .unwrap_or(false)
    }

    async fn emit_status_for_owner(
        &self,
        task_id: &str,
        status: tasks::TaskStatus,
        origin_session_id: String,
    ) {
        if !self.is_active_origin(&origin_session_id) {
            return;
        }
        self.event_sink
            .emit(client_protocol::events::ClientEvent::TaskStatusChanged {
                task_id: task_id.to_string(),
                status: client_adapter::lowering::lower_task_status(match status {
                    tasks::TaskStatus::Pending => "pending",
                    tasks::TaskStatus::Running => "running",
                    tasks::TaskStatus::Paused => "paused",
                    tasks::TaskStatus::Completed => "completed",
                    tasks::TaskStatus::Failed => "failed",
                    tasks::TaskStatus::Killed => "killed",
                }),
                origin_session_id: Some(origin_session_id),
            })
            .await;
    }

    async fn emit_progress_for_owner(
        &self,
        task_id: &str,
        run_id: &str,
        origin_session_id: String,
        progress: tasks::handlers::local_workflow::WorkflowProgressUpdate,
    ) {
        if !self.is_active_origin(&origin_session_id) {
            return;
        }
        self.listener
            .on_workflow_progress(
                origin_session_id,
                task_id.to_string(),
                run_id.to_string(),
                client_protocol::listings::WorkflowProgressDto {
                    kind: progress.kind,
                    index: progress.index,
                    title: progress.title,
                    message: progress.message,
                    label: progress.label,
                    phase_index: progress.phase_index,
                    phase_title: progress.phase_title,
                    agent_id: progress.agent_id,
                    agent_type: progress.agent_type,
                    model: progress.model,
                    fallback_model: progress.fallback_model,
                    state: progress.state,
                    error: progress.error,
                    tool_use_id: progress.tool_use_id,
                    queued_at_ms: progress.queued_at_ms,
                    started_at_ms: progress.started_at_ms,
                    last_progress_at_ms: progress.last_progress_at_ms,
                    attempt: progress.attempt,
                    last_attempt_reason: progress.last_attempt_reason,
                    tokens: progress.tokens,
                    tool_calls: progress.tool_calls,
                    last_tool_name: progress.last_tool_name,
                    last_tool_summary: progress.last_tool_summary,
                    prompt_preview: progress.prompt_preview,
                },
            )
            .await;
    }

    async fn flush_pending_events(&self, task_id: &str, pending: Vec<PendingWorkflowEvent>) {
        let Some((origin_session_id, _)) = self.checkpoints.task_owner(task_id) else {
            return;
        };
        for event in pending {
            match event {
                PendingWorkflowEvent::Status(status) => {
                    self.emit_status_for_owner(task_id, status, origin_session_id.clone())
                        .await;
                    if status.is_terminal() {
                        self.checkpoints.remove_task(task_id);
                    }
                }
                PendingWorkflowEvent::Progress { run_id, progress } => {
                    self.emit_progress_for_owner(
                        task_id,
                        &run_id,
                        origin_session_id.clone(),
                        progress,
                    )
                    .await;
                }
            }
        }
    }
}

#[async_trait::async_trait]
impl tasks::handlers::TaskStatusSink for MobileWorkflowStatusSink {
    async fn set_status(&self, task_id: &str, status: tasks::TaskStatus) {
        tasks::handlers::TaskStatusSink::set_status(&*self.registry, task_id, status).await;
        let Some((origin_session_id, _)) = self.checkpoints.task_owner(task_id) else {
            self.checkpoints
                .buffer_event(task_id, PendingWorkflowEvent::Status(status));
            return;
        };
        if status.is_terminal() {
            self.checkpoints.remove_task(task_id);
        }
        self.emit_status_for_owner(task_id, status, origin_session_id)
            .await;
    }

    async fn is_registered(&self, task_id: &str) -> bool {
        tasks::handlers::TaskStatusSink::is_registered(&*self.registry, task_id).await
    }

    async fn is_terminal(&self, task_id: &str) -> bool {
        tasks::handlers::TaskStatusSink::is_terminal(&*self.registry, task_id).await
    }
}

#[async_trait::async_trait]
impl tasks::handlers::local_workflow::WorkflowProgressSink for MobileWorkflowStatusSink {
    async fn emit_workflow_progress(
        &self,
        task_id: &str,
        run_id: &str,
        progress: tasks::handlers::local_workflow::WorkflowProgressUpdate,
    ) {
        let Some((origin_session_id, _)) = self.checkpoints.task_owner(task_id) else {
            self.checkpoints.buffer_event(
                task_id,
                PendingWorkflowEvent::Progress {
                    run_id: run_id.to_string(),
                    progress,
                },
            );
            return;
        };
        self.emit_progress_for_owner(task_id, run_id, origin_session_id, progress)
            .await;
    }
}

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

    /// Forward the lease token instead of inheriting the trait's delegating
    /// default. This wrapper sits between the lease PRODUCER
    /// (`WorkspaceLeaseToolInvoker`) and the CONSUMER (`RegistryToolInvoker`,
    /// which folds the token into `PermissionCheckContext`), so the default —
    /// which drops the token and calls `invoke` — left
    /// `workspace_lease_token` permanently `None` in production: the lease
    /// ALLOW never fired, and neither did the paired `denies_host_owned_for_token`
    /// hard deny.
    async fn invoke_with_workspace_lease(
        &self,
        name: &str,
        input: serde_json::Value,
        ctx: traits::tool_invoker::SubagentInvocationContext,
        workspace_lease_token: Option<u64>,
    ) -> Result<serde_json::Value, traits::tool_invoker::ToolInvokerError> {
        match self.inner.get() {
            Some(invoker) => {
                invoker
                    .invoke_with_workspace_lease(name, input, ctx, workspace_lease_token)
                    .await
            }
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
    /// Project cwd that owns the persisted session directory; fixed for the session.
    pub(crate) project_cwd: std::path::PathBuf,
    /// Profile root that owns `apps/<app_id>/workspace/.lingxi/app.manifest.json`.
    /// Local-app workflow persistence checks must read this host-materialized
    /// manifest rather than trusting the caller's expected collection list or
    /// the session cwd (which can point at another project during a resume).
    pub(crate) app_data_root: std::path::PathBuf,
    /// Live cwd shared with the session and sampled at each launch.
    pub(crate) current_cwd: Arc<std::sync::Mutex<std::path::PathBuf>>,
    /// The lingxi home (`<app_files_root>/.claude`), anchoring
    /// `transcriptDir = <projectDir>/<sessionId>/subagents/workflows/<runId>`.
    pub(crate) lingxi_home: std::path::PathBuf,
    /// The LIVE current-session uuid (bare uuid, no `sess:` prefix), read at
    /// launch time. Mobile retargets sessions inside ONE engine — New/Resume/
    /// Clear swap the id while the orchestrator and this launcher live on —
    /// so a boot-time snapshot would anchor every later workflow's transcript
    /// under a session the user has already left.
    pub(crate) session_uuid: Arc<std::sync::Mutex<String>>,
    pub(crate) checkpoints: Arc<MobileWorkflowCheckpointStore>,
    pub(crate) status_sink: Arc<MobileWorkflowStatusSink>,
}

/// Return whether a launch resolves to one of the immutable local-app build
/// workflows. A named fresh built-in must have no `scriptPath`, no non-empty
/// inline override, and the resolved body must still equal its bundled
/// descriptor. A fresh script-path launch with today's exact bundled body is
/// also accepted. A resumed script with today's exact bundled bytes is still
/// the current built-in even when its terminal checkpoint was cleaned up. A
/// non-exact resumed script must have host-owned provenance; markerless
/// checkpoints are conservatively treated as legacy and rejected rather than
/// executed. A custom workflow that merely happens to use the same `meta.name`
/// is not treated as a built-in.
fn is_mobile_local_app_builtin(
    spec: &tool_workflow::WorkflowLaunchSpec,
    script: &str,
    trusted_local_app_resume: bool,
) -> bool {
    let is_resume = spec
        .resume_from_run_id
        .as_deref()
        .is_some_and(|run_id| !run_id.is_empty());
    let no_script_path = spec
        .script_path
        .as_deref()
        .filter(|path| !path.is_empty())
        .is_none();
    let no_inline_override = spec
        .script
        .as_deref()
        .filter(|inline| !inline.is_empty())
        .is_none();
    let named_builtin = !is_resume
        && no_script_path
        && no_inline_override
        && spec.name.as_deref().is_some_and(|name| {
            tool_workflow::LOCAL_APP_BUILD_WORKFLOWS.contains(&name)
                && tool_workflow::BUILTIN_WORKFLOWS
                    .get(name)
                    .is_some_and(|descriptor| descriptor.script == script)
        });
    // Exact current bundled bytes are authoritative even for a resume whose
    // terminal checkpoint has already been removed. This is safe because the
    // immutable descriptor body, rather than a caller-controlled name/path,
    // identifies the built-in. Non-exact resumes still require trusted
    // checkpoint provenance below.
    let bundled_script = is_current_local_app_builtin_script(script);
    named_builtin || bundled_script || trusted_local_app_resume
}

fn is_current_local_app_builtin_script(script: &str) -> bool {
    tool_workflow::LOCAL_APP_BUILD_WORKFLOWS.iter().any(|name| {
        tool_workflow::BUILTIN_WORKFLOWS
            .get(name)
            .is_some_and(|descriptor| descriptor.script == script)
    })
}

fn is_current_local_app_builtin_hash(hash: &str) -> bool {
    tool_workflow::LOCAL_APP_BUILD_WORKFLOWS.iter().any(|name| {
        tool_workflow::BUILTIN_WORKFLOWS
            .get(name)
            .is_some_and(|descriptor| sha256_hex(descriptor.script.as_bytes()) == hash)
    })
}

fn local_app_workflow_id_for_hash(hash: &str) -> Option<String> {
    tool_workflow::LOCAL_APP_BUILD_WORKFLOWS
        .iter()
        .find_map(|name| {
            tool_workflow::BUILTIN_WORKFLOWS
                .get(name)
                .filter(|descriptor| sha256_hex(descriptor.script.as_bytes()) == hash)
                .map(|_| (*name).to_string())
        })
}

fn local_app_resume_identity_id(
    spec: &tool_workflow::WorkflowLaunchSpec,
    script: &str,
) -> Option<String> {
    spec.name
        .as_deref()
        .filter(|name| !name.is_empty())
        .filter(|name| tool_workflow::LOCAL_APP_BUILD_WORKFLOWS.contains(name))
        .map(str::to_string)
        .or_else(|| {
            workflow::meta_string_value(script, "name")
                .filter(|name| tool_workflow::LOCAL_APP_BUILD_WORKFLOWS.contains(&name.as_str()))
        })
}

fn local_app_resume_resolution_for_record(
    record: &WorkflowProvenanceRecord,
    local_identity_id: Option<String>,
) -> Option<LocalAppResumeResolution> {
    let record_is_local =
        tool_workflow::LOCAL_APP_BUILD_WORKFLOWS.contains(&record.workflow_id.as_str());
    let hash_is_current = is_current_local_app_builtin_hash(&record.script_sha256);
    let expected_workflow_id = if record_is_local {
        Some(record.workflow_id.clone())
    } else {
        local_app_workflow_id_for_hash(&record.script_sha256).or(local_identity_id)
    };
    if expected_workflow_id.is_none() {
        return None;
    }
    let provenance = if !record_is_local {
        LocalAppResumeProvenance::LegacyBuiltin
    } else {
        let record_matches_descriptor = tool_workflow::BUILTIN_WORKFLOWS
            .get(&record.workflow_id)
            .is_some_and(|descriptor| {
                sha256_hex(descriptor.script.as_bytes()) == record.script_sha256
            });
        match record.script_is_verbatim_builtin {
            Some(true) if record_matches_descriptor => LocalAppResumeProvenance::CurrentBuiltin,
            Some(true) => LocalAppResumeProvenance::LegacyBuiltin,
            Some(false) if hash_is_current => LocalAppResumeProvenance::LegacyBuiltin,
            Some(false) => LocalAppResumeProvenance::Custom,
            None => LocalAppResumeProvenance::LegacyBuiltin,
        }
    };
    Some(LocalAppResumeResolution {
        provenance,
        workflow_id: expected_workflow_id,
    })
}

fn local_app_resume_provenance_for_launch(
    checkpoints: &MobileWorkflowCheckpointStore,
    spec: &tool_workflow::WorkflowLaunchSpec,
    session_uuid: &str,
    run_id: &str,
    script_path: &std::path::Path,
    script: &str,
) -> Option<LocalAppResumeProvenance> {
    local_app_resume_resolution_for_launch(
        checkpoints,
        spec,
        session_uuid,
        run_id,
        script_path,
        script,
    )
    .map(|resolution| resolution.provenance)
}

fn local_app_resume_resolution_for_launch(
    checkpoints: &MobileWorkflowCheckpointStore,
    spec: &tool_workflow::WorkflowLaunchSpec,
    session_uuid: &str,
    run_id: &str,
    script_path: &std::path::Path,
    script: &str,
) -> Option<LocalAppResumeResolution> {
    match checkpoints.read_provenance_sidecar(session_uuid, run_id, script_path) {
        WorkflowProvenanceLookup::Valid(record) => local_app_resume_resolution_for_record(
            &record,
            local_app_resume_identity_id(spec, script),
        ),
        WorkflowProvenanceLookup::Invalid { workflow_id } => {
            let local_workflow_id = workflow_id
                .as_deref()
                .filter(|id| tool_workflow::LOCAL_APP_BUILD_WORKFLOWS.contains(id))
                .map(str::to_string);
            let identity_id =
                local_workflow_id.or_else(|| local_app_resume_identity_id(spec, script));
            (identity_id.is_some()
                || checkpoints.is_host_owned_workflow_script(session_uuid, run_id, script_path))
            .then_some(LocalAppResumeResolution {
                provenance: LocalAppResumeProvenance::LegacyBuiltin,
                workflow_id: identity_id,
            })
        }
        WorkflowProvenanceLookup::Missing => checkpoints
            .local_app_resume_record(session_uuid, run_id, script_path)
            .and_then(|record| {
                local_app_resume_resolution_for_record(
                    &record,
                    local_app_resume_identity_id(spec, script),
                )
            })
            .or_else(|| {
                (local_app_resume_identity_id(spec, script).is_some()
                    && checkpoints.is_host_owned_workflow_script(session_uuid, run_id, script_path))
                .then_some(LocalAppResumeResolution {
                    provenance: LocalAppResumeProvenance::LegacyBuiltin,
                    workflow_id: None,
                })
            }),
    }
}

/// Replace the caller-provided persistence contract with the IDs from the
/// materialized app manifest. The caller's value is intentionally ignored,
/// including `[]`: an empty caller list must not turn off the native
/// round-trip gate for a manifest that declares writable collections.
fn apply_materialized_local_app_collections(
    app_data_root: &std::path::Path,
    spec: &mut tool_workflow::WorkflowLaunchSpec,
    script: &str,
) -> Result<(), tool_workflow::WorkflowLaunchError> {
    apply_materialized_local_app_collections_with_provenance(app_data_root, spec, script, false)
}

fn apply_materialized_local_app_collections_with_provenance(
    app_data_root: &std::path::Path,
    spec: &mut tool_workflow::WorkflowLaunchSpec,
    script: &str,
    trusted_local_app_resume: bool,
) -> Result<(), tool_workflow::WorkflowLaunchError> {
    apply_materialized_local_app_collections_with_identity(
        app_data_root,
        spec,
        script,
        trusted_local_app_resume,
        None,
    )
}

/// The single Local App build workflow authorized for a build target.
///
/// This is the map `apply_materialized_local_app_collections_with_identity`
/// enforces below: it is the "one place a build target maps to its required
/// workflow id" the component-literal allowlist documents. Phase -1 P-1.1
/// wraps its output in a typed handle (`local_app_plugin_binding`) so the
/// seam below stops comparing raw strings itself; P-1.3 is expected to
/// remove this map in favor of typed routing throughout — this function is
/// deliberately still exactly the pre-Phase -1 match, moved rather than
/// rewritten, so that removal has one obvious place to happen.
pub(crate) fn required_workflow_id_for(
    build_target: crate::local_apps_build::LocalAppBuildTarget,
) -> &'static str {
    match build_target {
        crate::local_apps_build::LocalAppBuildTarget::ReactDomR1 => "local-app-build",
        crate::local_apps_build::LocalAppBuildTarget::Canvas2dR1
        | crate::local_apps_build::LocalAppBuildTarget::Three3dR1
        | crate::local_apps_build::LocalAppBuildTarget::Phaser2dR1
        | crate::local_apps_build::LocalAppBuildTarget::Babylon3dR1 => "local-canvas-build",
    }
}

fn apply_materialized_local_app_collections_with_identity(
    app_data_root: &std::path::Path,
    spec: &mut tool_workflow::WorkflowLaunchSpec,
    script: &str,
    trusted_local_app_resume: bool,
    expected_workflow_id: Option<&str>,
) -> Result<(), tool_workflow::WorkflowLaunchError> {
    if let Some(expected_workflow_id) = expected_workflow_id {
        let expected_script = tool_workflow::BUILTIN_WORKFLOWS
            .get(expected_workflow_id)
            .map(|descriptor| descriptor.script);
        if expected_script.is_none() {
            if trusted_local_app_resume {
                return Err(tool_workflow::WorkflowLaunchError(
                    "cannot resume an obsolete local-app build workflow script; start the current named local-app workflow instead of resuming this run"
                        .to_string(),
                ));
            }
        } else if is_current_local_app_builtin_script(script) && expected_script != Some(script) {
            return Err(tool_workflow::WorkflowLaunchError(
                "cannot resume a local-app workflow with a different built-in script; start the matching named local-app workflow instead"
                    .to_string(),
            ));
        } else if trusted_local_app_resume && expected_script != Some(script) {
            return Err(tool_workflow::WorkflowLaunchError(
                "cannot resume an obsolete local-app build workflow script; start the current named local-app workflow instead of resuming this run"
                    .to_string(),
            ));
        }
    } else if trusted_local_app_resume && !is_current_local_app_builtin_script(script) {
        return Err(tool_workflow::WorkflowLaunchError(
            "cannot resume an obsolete local-app build workflow script; start the current named local-app workflow instead of resuming this run"
                .to_string(),
        ));
    }
    if !is_mobile_local_app_builtin(spec, script, trusted_local_app_resume) {
        return Ok(());
    }

    let launched_workflow_id = expected_workflow_id
        .map(str::to_string)
        .or_else(|| local_app_workflow_id_for_hash(&sha256_hex(script.as_bytes())))
        .or_else(|| {
            spec.name
                .as_deref()
                .filter(|name| tool_workflow::LOCAL_APP_BUILD_WORKFLOWS.contains(name))
                .map(str::to_string)
        })
        .ok_or_else(|| {
            tool_workflow::WorkflowLaunchError(
                "cannot identify the local-app built-in workflow being launched".to_string(),
            )
        })?;

    let args = spec.args.as_mut().ok_or_else(|| {
        tool_workflow::WorkflowLaunchError(
            "local-app build workflow requires args.app_id and a materialized manifest".to_string(),
        )
    })?;
    let object = args.as_object_mut().ok_or_else(|| {
        tool_workflow::WorkflowLaunchError(
            "local-app build workflow args must be an object containing app_id".to_string(),
        )
    })?;
    let app_id = object
        .get("app_id")
        .and_then(serde_json::Value::as_str)
        .filter(|app_id| !app_id.trim().is_empty())
        .ok_or_else(|| {
            tool_workflow::WorkflowLaunchError(
                "local-app build workflow requires a non-empty args.app_id".to_string(),
            )
        })?;
    let layout = local_apps::AppLayout::new(app_data_root, app_id).map_err(|error| {
        tool_workflow::WorkflowLaunchError(format!(
            "cannot resolve local-app manifest for app {app_id:?}: {error}"
        ))
    })?;
    // Keep workflow routing behind the same authoritative gate as the build
    // path.  The workflow receives an app id from the model, so loading a
    // manifest alone is not enough: a torn shell can otherwise look like a
    // valid local app, and a caller-controlled binding could select a family
    // that this host cannot actually resolve.  `detect_build_target` checks
    // the record mirror's `scaffolded` bit, requires the manifest surface and
    // binding as a consistent pair, and resolves the binding against the exact
    // published catalog (including its contract hash and availability).
    let build_target = crate::local_apps_build::detect_build_target(&layout).map_err(|error| {
        tool_workflow::WorkflowLaunchError(format!(
            "cannot validate local-app runtime profile for app {app_id:?}: {error}"
        ))
    })?;
    let manifest = local_apps::load_manifest(&layout).map_err(|error| {
        tool_workflow::WorkflowLaunchError(format!(
            "cannot load materialized local-app manifest for app {app_id:?}: {error}"
        ))
    })?;
    let binding = manifest.runtime_profile.as_ref().ok_or_else(|| {
        tool_workflow::WorkflowLaunchError(format!(
            "local-app build workflow requires app {app_id:?} to have a persisted runtime profile"
        ))
    })?;
    if manifest.dependency_snapshot.is_none() {
        return Err(tool_workflow::WorkflowLaunchError(format!(
            "local-app build workflow requires app {app_id:?} to have a verified dependency snapshot"
        )));
    }
    // The Host consumes a typed handle rather than picking or comparing a
    // workflow name itself: the binding resolves the build target's
    // authorized workflow and enforces it against the caller's selection,
    // keeping the pre-Phase -1 refusal semantics (same app id, family, and
    // both workflow ids named on mismatch) behind a composition seam instead
    // of an inline string comparison.
    let plugin_binding =
        crate::local_app_plugin_binding::LocalAppPluginBinding::resolve(build_target);
    plugin_binding.enforce(app_id, binding.family, &launched_workflow_id)?;
    object.insert(
        "runtime_profile".to_string(),
        serde_json::json!({
            "family": binding.family.as_str(),
            "revision": binding.revision,
            "contract_sha256": binding.contract_sha256,
        }),
    );
    let collection_ids = manifest
        .collections
        .into_iter()
        .map(|collection| serde_json::Value::String(collection.id))
        .collect();
    object.insert(
        "expected_writable_collections".to_string(),
        serde_json::Value::Array(collection_ids),
    );
    Ok(())
}

#[async_trait::async_trait]
impl tool_workflow::WorkflowLauncher for MobileWorkflowLauncher {
    async fn launch(
        &self,
        mut spec: tool_workflow::WorkflowLaunchSpec,
    ) -> Result<tool_workflow::WorkflowLaunched, tool_workflow::WorkflowLaunchError> {
        let cwd = self
            .current_cwd
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let abs = |p: &str| -> std::path::PathBuf {
            let path = std::path::Path::new(p);
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                cwd.join(path)
            }
        };
        let script =
            tool_workflow::resolve_script_at(&cwd, &spec, |p| std::fs::read_to_string(abs(p)))?;
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
        if let Err(error) = workflow::validate_body(&script) {
            let error = match error {
                workflow::WorkflowError::Engine(message)
                | workflow::WorkflowError::Script(message) => message,
            };
            let run_id = tool_workflow::mint_run_id(spec.resume_from_run_id.as_deref());
            let workflow_name = workflow::meta_string_value(&script, "name");
            let summary = workflow::meta_string_value(&script, "description");
            return Ok(tool_workflow::WorkflowLaunched {
                task_id: tasks::generate_task_id(tasks::TaskType::LocalWorkflow),
                run_id: Some(run_id),
                workflow_name,
                summary,
                error: Some(error),
                ..Default::default()
            });
        }
        // One launch belongs to exactly one session. Capture the live session
        // before consulting resume provenance so a concurrent retarget cannot
        // split its task row, checkpoint, and transcript directory across two
        // conversations.
        let session_uuid = spec
            .session_uuid
            .take()
            .filter(|session| !session.is_empty())
            .or_else(|| self.session_uuid.lock().ok().map(|guard| guard.clone()))
            .unwrap_or_default();
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
            if !tool_workflow::is_valid_run_id(rid) {
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
        // Mobile is the host authority for local-app persistence. Rewrite the
        // task-local list before serializing args into the TaskRegistry so the
        // QuickJS workflow can never receive a caller-supplied `[]` for an app
        // whose materialized manifest declares collections. Current bundled
        // bytes identify the built-in even after a terminal checkpoint has
        // been cleaned up. A non-exact resume is classified from the
        // host-owned transcript sidecar first, then its legacy checkpoint,
        // and finally the same-session scratch path when no durable row
        // remains. These checks bind the workflow id, run id, script path,
        // session, recorded script identity, and provenance marker. A
        // markerless legacy script is rejected before it can execute. Non-local
        // and direct custom workflows remain byte-for-byte unchanged.
        let resume_resolution = match (
            spec.resume_from_run_id
                .as_deref()
                .filter(|run_id| !run_id.is_empty()),
            spec.script_path
                .as_deref()
                .filter(|path| !path.is_empty())
                .map(|path| abs(path)),
        ) {
            (Some(run_id), Some(script_path)) => local_app_resume_resolution_for_launch(
                &self.checkpoints,
                &spec,
                &session_uuid,
                run_id,
                &script_path,
                &script,
            ),
            _ => None,
        };
        let trusted_local_app_resume = matches!(
            resume_resolution
                .as_ref()
                .map(|resolution| resolution.provenance),
            Some(
                LocalAppResumeProvenance::LegacyBuiltin | LocalAppResumeProvenance::CurrentBuiltin
            )
        );
        let expected_workflow_id = resume_resolution
            .as_ref()
            .and_then(|resolution| resolution.workflow_id.as_deref());
        apply_materialized_local_app_collections_with_identity(
            &self.app_data_root,
            &mut spec,
            &script,
            trusted_local_app_resume,
            expected_workflow_id,
        )?;
        // Mint the run id at launch (fresh) or reuse the resume id. Host
        // clock use is fine — only the workflow SCRIPT is barred from the
        // clock. Shape: `wf_` + 8 hex + `-` + 3 hex.
        let run_id = tool_workflow::mint_run_id(spec.resume_from_run_id.as_deref());
        let workflow_name = workflow::meta_string_value(&script, "name");
        tool_workflow::apply_local_app_build_default_model(
            &cwd,
            workflow_name.as_deref(),
            &mut spec.args,
        )?;
        let summary = workflow::meta_string_value(&script, "description");
        let task_description = summary
            .clone()
            .unwrap_or_else(|| "Dynamic workflow".to_string());
        // Reserve before the first run-id-derived filesystem write. The async
        // block below collects every later error so the reservation is always
        // released exactly once.
        let reservation = self
            .registry
            .try_reserve_workflow_run_id(&run_id)
            .await
            .map_err(|error| tool_workflow::WorkflowLaunchError(error.to_string()))?;
        let launch_result = async {
            let subagents = orchestrator::transcript_paths::subagents_dir(
                &self.lingxi_home,
                &self.project_cwd.to_string_lossy(),
                &session_uuid,
            );
            // Persist the script so it is editable + re-runnable via `scriptPath`.
            // A `scriptPath` input is already on disk → returned as-is; an
            // inline/`name` script is written under the session-owned workflow dir.
            let script_path = if let Some(p) = spec.script_path.as_deref().filter(|s| !s.is_empty())
            {
                abs(p).to_str().map(str::to_string).ok_or_else(|| {
                    tool_workflow::WorkflowLaunchError(
                        "workflow script path is not valid UTF-8".to_string(),
                    )
                })?
            } else {
                let session_dir = subagents.parent().ok_or_else(|| {
                    tool_workflow::WorkflowLaunchError(
                        "cannot derive workflow session directory".to_string(),
                    )
                })?;
                let dir = session_dir.join("workflows");
                let file = dir.join(format!("{run_id}.js"));
                std::fs::create_dir_all(&dir).map_err(|error| {
                    tool_workflow::WorkflowLaunchError(format!(
                        "cannot create workflow script directory '{}': {error}",
                        dir.display()
                    ))
                })?;
                std::fs::write(&file, &script).map_err(|error| {
                    tool_workflow::WorkflowLaunchError(format!(
                        "cannot persist workflow script '{}': {error}",
                        file.display()
                    ))
                })?;
                file.to_str().map(str::to_string).ok_or_else(|| {
                    tool_workflow::WorkflowLaunchError(
                        "workflow script path is not valid UTF-8".to_string(),
                    )
                })?
            };
            let transcript_dir = { subagents.join("workflows").join(&run_id) };
            std::fs::create_dir_all(&transcript_dir).map_err(|error| {
                tool_workflow::WorkflowLaunchError(format!(
                    "cannot create workflow transcript directory '{}': {error}",
                    transcript_dir.display()
                ))
            })?;
            let journal_path = transcript_dir.join("journal.jsonl");
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&journal_path)
                .map_err(|error| {
                    tool_workflow::WorkflowLaunchError(format!(
                        "cannot create workflow journal '{}': {error}",
                        journal_path.display()
                    ))
                })?;
            let transcript_dir_wire = transcript_dir.to_str().map(str::to_string);
            let has_script_path = spec
                .script_path
                .as_deref()
                .filter(|s| !s.is_empty())
                .is_some();
            let has_name = spec.name.as_deref().filter(|s| !s.is_empty()).is_some();
            let named_source = spec
                .name
                .as_deref()
                .filter(|s| !s.is_empty())
                .and_then(|name| tool_workflow::workflow_source_for_name(&cwd, name));
            let script_is_verbatim_builtin =
                is_mobile_local_app_builtin(&spec, &script, trusted_local_app_resume);
            let (invocation_mode, workflow_source) = if has_script_path {
                ("scriptPath".to_string(), "scriptPath".to_string())
            } else if has_name {
                (
                    "named".to_string(),
                    if named_source.is_some() {
                        named_source.unwrap_or("custom").to_string()
                    } else {
                        "custom".to_string()
                    },
                )
            } else {
                ("inline".to_string(), "inline".to_string())
            };
            let script_sha256 = sha256_hex(script.as_bytes());
            let workflow_id = workflow_name
                .clone()
                .filter(|name| !name.is_empty())
                .or_else(|| spec.name.clone())
                .unwrap_or_default();
            // Fresh local-app launches leave a durable provenance record beside
            // the journal. Resumes never rewrite this record: a mismatched
            // caller path must not be able to replace a trusted record with a
            // new custom identity.
            if spec
                .resume_from_run_id
                .as_deref()
                .filter(|run_id| !run_id.is_empty())
                .is_none()
                && !session_uuid.is_empty()
                && tool_workflow::LOCAL_APP_BUILD_WORKFLOWS.contains(&workflow_id.as_str())
            {
                self.checkpoints
                    .write_provenance_sidecar(
                        &session_uuid,
                        &run_id,
                        std::path::Path::new(&script_path),
                        &script,
                        &workflow_id,
                        script_is_verbatim_builtin,
                    )
                    .map_err(|error| {
                        tool_workflow::WorkflowLaunchError(format!(
                            "cannot persist workflow provenance: {error}"
                        ))
                    })?;
            }
            let task_id = self
                .registry
                .spawn(
                    tasks::TaskType::LocalWorkflow,
                    tasks::TaskSpawnInput::LocalWorkflow {
                        session_uuid: Some(session_uuid.clone()),
                        workflow_id: workflow_id.clone(),
                        script,
                        resume_from_run_id: spec.resume_from_run_id.clone(),
                        args: spec
                            .args
                            .as_ref()
                            .map(|v| serde_json::to_string(v).unwrap_or_default()),
                        run_id: Some(run_id.clone()),
                        invocation_mode: Some(invocation_mode),
                        workflow_source: Some(workflow_source),
                        script_is_verbatim_builtin: Some(script_is_verbatim_builtin),
                        transcript_subdir: Some(transcript_dir.clone()),
                        launched_from_subagent: spec.launched_from_subagent,
                        tool_use_id: spec.tool_use_id.clone(),
                        creator_teammate_name: spec.creator_teammate_name.clone(),
                        creator_team_name: spec.creator_team_name.clone(),
                        creator_agent_id: spec
                            .creator_agent_id
                            .as_deref()
                            .and_then(protocol::AgentId::parse_prefixed),
                    },
                    task_description,
                )
                .await
                .map_err(|e| tool_workflow::WorkflowLaunchError(e.to_string()))?;
            self.registry
                .set_workflow_resume_metadata(&task_id, script_path.clone(), transcript_dir.clone())
                .await
                .map_err(|error| tool_workflow::WorkflowLaunchError(error.to_string()))?;
            let args_json = spec
                .args
                .as_ref()
                .map(|value| serde_json::to_string(value).unwrap_or_default());
            let pending_events = match self.checkpoints.upsert(
                &session_uuid,
                WorkflowCheckpoint {
                    task_id: task_id.clone(),
                    workflow_run_id: run_id.clone(),
                    workflow_id: workflow_id.clone(),
                    script_path: script_path.clone(),
                    script_sha256: Some(script_sha256),
                    script_is_verbatim_builtin: Some(script_is_verbatim_builtin),
                    args_json,
                    description: summary
                        .clone()
                        .or_else(|| workflow_name.clone())
                        .unwrap_or_else(|| "Workflow".to_string()),
                    start_time: Some(unix_time_ms()),
                    transcript_dir: transcript_dir.to_string_lossy().into_owned(),
                },
            ) {
                Ok(pending_events) => pending_events,
                Err(error) => {
                    let _ = self.registry.kill(&task_id).await;
                    return Err(tool_workflow::WorkflowLaunchError(format!(
                        "cannot persist workflow checkpoint: {error}"
                    )));
                }
            };
            self.status_sink
                .flush_pending_events(&task_id, pending_events)
                .await;
            if self
                .registry
                .get(&task_id)
                .await
                .is_some_and(|state| state.base().status.is_terminal())
            {
                self.checkpoints.remove_task(&task_id);
            }
            if spec.resume_from_run_id.is_some() {
                self.registry
                    .remove_paused_workflow_by_run_id(&session_uuid, &run_id)
                    .await;
            }
            Ok(tool_workflow::WorkflowLaunched {
                task_id,
                run_id: Some(run_id.clone()),
                script_path: Some(script_path),
                workflow_name,
                summary,
                transcript_dir: transcript_dir_wire,
                error: None,
            })
        }
        .await;
        drop(reservation);
        launch_result
    }
}

#[cfg(test)]
mod run_id_tests {
    use std::sync::Arc;

    use tool_workflow::is_valid_run_id;

    use crate::test_support::FakeListener;

    fn stamp_profile(
        manifest: &mut local_apps::AppManifest,
        family: local_apps::AppRuntimeProfile,
    ) {
        let binding = crate::local_app_runtime_profiles::current_binding_for_family(family)
            .expect("published runtime profile");
        manifest.surface = Some(family.surface());
        manifest.runtime_profile = Some(binding.clone());
        manifest.dependency_snapshot = Some(local_apps::AppDependencySnapshot {
            requested_sha256: "0".repeat(64),
            package_sha256: "1".repeat(64),
            lockfile_sha256: "2".repeat(64),
            dependency_tree_sha256: "3".repeat(64),
            sbom_sha256: "4".repeat(64),
            toolchain_key: crate::local_app_runtime_profiles::RUNTIME_PROFILE_TOOLCHAIN_KEY
                .to_string(),
            verified_profile_contract_sha256: binding.contract_sha256,
        });
    }

    fn stamp_record_mirror(layout: &local_apps::AppLayout, scaffolded: bool) {
        let mut record = local_apps::AppState::create_with_git(
            layout.app_id().to_string(),
            "Fixture".to_string(),
            "A workflow-support fixture".to_string(),
            None,
            false,
            1_700_000_000_000,
        )
        .record;
        record.scaffolded = scaffolded;
        let relative = local_apps::storage::metadata_rel(layout.app_id());
        let path = layout.root().join(relative);
        std::fs::create_dir_all(path.parent().expect("metadata parent")).expect("metadata dir");
        let mut body = serde_json::to_vec_pretty(&local_apps::storage::AppMetadataFile {
            schema_version: local_apps::APPS_SCHEMA_VERSION,
            app: record,
        })
        .expect("metadata json");
        body.push(b'\n');
        std::fs::write(path, body).expect("metadata mirror");
    }

    #[tokio::test]
    async fn checkpoint_round_trip_adopts_a_paused_workflow() {
        let root = tempfile::tempdir().expect("tempdir");
        let home = root.path().join(".claude");
        let session = "00000000-0000-0000-0000-000000000001";
        let store = Arc::new(super::MobileWorkflowCheckpointStore::new(
            home,
            root.path().to_path_buf(),
        ));
        let transcript_dir = store
            .session_dir(session)
            .join("subagents")
            .join("workflows")
            .join("wf_abcdef");
        std::fs::create_dir_all(&transcript_dir).unwrap();
        std::fs::write(transcript_dir.join("journal.jsonl"), "").unwrap();
        let script_path = root.path().join("build.js");
        let script = b"return 1;";
        std::fs::write(&script_path, script).unwrap();
        store
            .upsert(
                session,
                super::WorkflowCheckpoint {
                    task_id: "wabc12345".into(),
                    workflow_run_id: "wf_abcdef".into(),
                    workflow_id: "local-app-build".into(),
                    script_path: script_path.to_string_lossy().into_owned(),
                    script_sha256: Some(super::sha256_hex(script)),
                    script_is_verbatim_builtin: None,
                    args_json: Some(r#"{"app_id":"demo"}"#.into()),
                    description: "Build local app".into(),
                    start_time: Some(1234),
                    transcript_dir: transcript_dir.to_string_lossy().into_owned(),
                },
            )
            .unwrap();

        let fs: Arc<dyn traits::FileSystem> = Arc::new(
            platform_posix_minimal::PosixFileSystem::new(root.path().to_path_buf()),
        );
        let output = Arc::new(tasks::output_manager::TaskOutputManager::new(
            root.path().join("task-output"),
            fs.clone(),
        ));
        std::fs::create_dir_all(root.path().join("task-output")).unwrap();
        let registry = tasks::registry::TaskRegistry::new(
            Arc::new(platform_posix_minimal::PosixRuntime::new()),
            fs,
            output,
        );
        store.adopt_session(session, &registry).await;

        let state = registry.get("wabc12345").await.expect("adopted task");
        assert_eq!(state.base().status, tasks::TaskStatus::Paused);
        let tasks::state::TaskState::LocalWorkflow(workflow) = state else {
            panic!("expected workflow")
        };
        assert_eq!(workflow.run_id.as_deref(), Some("wf_abcdef"));
        assert_eq!(workflow.workflow_id, "local-app-build");
        assert_eq!(workflow.script_path.as_deref(), script_path.to_str());

        store.remove_task("wabc12345");
        assert!(
            !store.path(session).exists(),
            "terminal cleanup removes the last handoff file"
        );
    }

    #[test]
    fn adopt_json_omits_absent_fields_and_preserves_other_handoffs() {
        let root = tempfile::tempdir().expect("tempdir");
        let session = "00000000-0000-0000-0000-000000000002";
        let store = super::MobileWorkflowCheckpointStore::new(
            root.path().join(".claude"),
            root.path().to_path_buf(),
        );
        store
            .upsert(
                session,
                super::WorkflowCheckpoint {
                    task_id: "wabc12345".into(),
                    workflow_run_id: "wf_abcdef".into(),
                    workflow_id: "local-app-build".into(),
                    script_path: "/workspace/build.js".into(),
                    script_sha256: None,
                    script_is_verbatim_builtin: None,
                    args_json: None,
                    description: "Build local app".into(),
                    start_time: None,
                    transcript_dir: "/workspace/transcript".into(),
                },
            )
            .expect("upsert checkpoint");

        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(store.path(session)).expect("read adopt file"))
                .expect("parse adopt file");
        let workflow = &value["workflows"][0];
        assert_eq!(workflow["workflowId"], "local-app-build");
        assert!(workflow.get("scriptSha256").is_none());
        assert!(workflow.get("argsJson").is_none());
        assert!(workflow.get("startTime").is_none());

        let mut file = store.read(session);
        file.shells.push(serde_json::json!({ "taskId": "b123" }));
        store.write(session, file).expect("add shell handoff");
        store.remove_task("wabc12345");

        let value: serde_json::Value = serde_json::from_slice(
            &std::fs::read(store.path(session)).expect("shell handoff must remain"),
        )
        .expect("parse preserved adopt file");
        assert_eq!(value["workflows"], serde_json::json!([]));
        assert_eq!(value["shells"], serde_json::json!([{ "taskId": "b123" }]));
    }

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

    #[test]
    fn persisted_app_model_is_injected_into_local_app_build_args_only_when_missing() {
        let root = tempfile::tempdir().expect("tempdir");
        let state_dir = root.path().join(".lingxi");
        std::fs::create_dir_all(&state_dir).expect("state dir");
        std::fs::write(
            state_dir.join("app.json"),
            serde_json::json!({
                "schemaVersion": 1,
                "app": { "workflowModel": "deepseek/deepseek-v4-flash" }
            })
            .to_string(),
        )
        .expect("app metadata");
        let mut args = Some(serde_json::json!({
            "app_id": "habits-1234",
            "spec": "confirmed"
        }));

        tool_workflow::apply_local_app_build_default_model(
            root.path(),
            Some("local-app-build"),
            &mut args,
        )
        .expect("inject persisted model");

        assert_eq!(
            args.as_ref()
                .and_then(|value| value.get("model"))
                .and_then(serde_json::Value::as_str),
            Some("deepseek/deepseek-v4-flash")
        );

        let mut explicit = Some(serde_json::json!({
            "app_id": "habits-1234",
            "spec": "confirmed",
            "model": "anthropic/claude-opus-4-7"
        }));
        tool_workflow::apply_local_app_build_default_model(
            root.path(),
            Some("local-app-build"),
            &mut explicit,
        )
        .expect("preserve explicit model");
        assert_eq!(
            explicit
                .as_ref()
                .and_then(|value| value.get("model"))
                .and_then(serde_json::Value::as_str),
            Some("anthropic/claude-opus-4-7")
        );
    }

    #[test]
    fn local_app_builtin_overwrites_expected_collections_from_materialized_manifest() {
        let root = tempfile::tempdir().expect("tempdir");
        let layout = local_apps::AppLayout::new(root.path(), "demo1234").expect("layout");
        let mut manifest = local_apps::AppManifest::for_new_app("demo1234", "Demo");
        stamp_profile(&mut manifest, local_apps::AppRuntimeProfile::ReactDom);
        manifest.collections.push(local_apps::DataCollectionSchema {
            id: "items".into(),
            name: "Items".into(),
            fields: Vec::new(),
        });
        local_apps::save_manifest(&layout, &manifest).expect("manifest");
        stamp_record_mirror(&layout, true);

        let descriptor = tool_workflow::BUILTIN_WORKFLOWS
            .get("local-app-build")
            .expect("local-app built-in");
        let mut spec = tool_workflow::WorkflowLaunchSpec {
            name: Some("local-app-build".into()),
            args: Some(serde_json::json!({
                "app_id": "demo1234",
                "expected_writable_collections": [],
            })),
            ..Default::default()
        };

        super::apply_materialized_local_app_collections(root.path(), &mut spec, descriptor.script)
            .expect("manifest collection ids should be authoritative");

        assert_eq!(
            spec.args
                .as_ref()
                .and_then(|args| args.get("expected_writable_collections")),
            Some(&serde_json::json!(["items"]))
        );
        assert_eq!(
            spec.args
                .as_ref()
                .and_then(|args| args.pointer("/runtime_profile/family"))
                .and_then(serde_json::Value::as_str),
            Some("react_dom")
        );
    }

    /// §19.3: workflow launch context is injected only by the Host, and a
    /// caller-supplied value is rejected or overridden. `object.insert`
    /// silently overwrites an existing key, which is easy to break with a
    /// refactor (`entry().or_insert_with(...)`, or an early return that
    /// skips the insert) while every other test in this module still stays
    /// green, because none of them put a hostile value in the args first.
    ///
    /// This does not just check that the caller's value is GONE — a Host
    /// that injected garbage would also make it gone. It independently
    /// recomputes the app's PINNED binding via
    /// `local_app_runtime_profiles::current_binding_for_family` (the same
    /// authority `stamp_profile` used to seed the manifest) and asserts the
    /// injected object equals it field-by-field.
    #[test]
    fn caller_supplied_runtime_profile_is_overridden_by_the_host() {
        let root = tempfile::tempdir().expect("tempdir");
        let layout = local_apps::AppLayout::new(root.path(), "demo1234").expect("layout");
        let mut manifest = local_apps::AppManifest::for_new_app("demo1234", "Demo");
        stamp_profile(&mut manifest, local_apps::AppRuntimeProfile::ReactDom);
        manifest.collections.push(local_apps::DataCollectionSchema {
            id: "items".into(),
            name: "Items".into(),
            fields: Vec::new(),
        });
        local_apps::save_manifest(&layout, &manifest).expect("manifest");
        stamp_record_mirror(&layout, true);

        let descriptor = tool_workflow::BUILTIN_WORKFLOWS
            .get("local-app-build")
            .expect("local-app built-in");
        // A hostile caller-supplied runtime_profile: a different family, a
        // revision far beyond anything published, and a contract hash that
        // cannot correspond to any real catalog entry. If any of this
        // survives, a caller could point the workflow at a
        // collection/persistence contract the Host never verified.
        let mut spec = tool_workflow::WorkflowLaunchSpec {
            name: Some("local-app-build".into()),
            args: Some(serde_json::json!({
                "app_id": "demo1234",
                "runtime_profile": {
                    "family": "three_3d",
                    "revision": 999,
                    "contract_sha256": "attacker-supplied-not-a-real-sha256",
                },
                "expected_writable_collections": [],
            })),
            ..Default::default()
        };

        super::apply_materialized_local_app_collections(root.path(), &mut spec, descriptor.script)
            .expect("materialized manifest should still resolve through a hostile args block");

        let pinned = crate::local_app_runtime_profiles::current_binding_for_family(
            local_apps::AppRuntimeProfile::ReactDom,
        )
        .expect("published runtime profile");

        let injected = spec
            .args
            .as_ref()
            .and_then(|args| args.get("runtime_profile"))
            .expect("host must inject runtime_profile");
        assert_eq!(
            injected.get("family").and_then(serde_json::Value::as_str),
            Some(pinned.family.as_str()),
            "family must equal the app's PINNED family, not merely differ from the caller's: {injected}"
        );
        assert_eq!(
            injected.get("revision").and_then(serde_json::Value::as_u64),
            Some(u64::from(pinned.revision)),
            "revision must equal the app's PINNED revision, not merely differ from the caller's: {injected}"
        );
        assert_eq!(
            injected
                .get("contract_sha256")
                .and_then(serde_json::Value::as_str),
            Some(pinned.contract_sha256.as_str()),
            "contract_sha256 must equal the app's PINNED contract, not merely differ from the caller's: {injected}"
        );

        // The caller's hostile values must specifically be gone too, not
        // just "replaced by some other value that happens to equal pinned".
        assert_ne!(
            injected.get("family").and_then(serde_json::Value::as_str),
            Some("three_3d"),
            "caller-supplied family must not survive"
        );
        assert_ne!(
            injected.get("revision").and_then(serde_json::Value::as_u64),
            Some(999),
            "caller-supplied revision must not survive"
        );
        assert_ne!(
            injected
                .get("contract_sha256")
                .and_then(serde_json::Value::as_str),
            Some("attacker-supplied-not-a-real-sha256"),
            "caller-supplied contract_sha256 must not survive"
        );
    }

    /// §19.3, the `expected_writable_collections` half: a caller-supplied
    /// collection id naming something the manifest never declared must be
    /// replaced wholesale, not merged with the Host's list.
    #[test]
    fn caller_supplied_expected_writable_collections_is_overridden_by_the_host() {
        let root = tempfile::tempdir().expect("tempdir");
        let layout = local_apps::AppLayout::new(root.path(), "demo1234").expect("layout");
        let mut manifest = local_apps::AppManifest::for_new_app("demo1234", "Demo");
        stamp_profile(&mut manifest, local_apps::AppRuntimeProfile::ReactDom);
        manifest.collections.push(local_apps::DataCollectionSchema {
            id: "items".into(),
            name: "Items".into(),
            fields: Vec::new(),
        });
        local_apps::save_manifest(&layout, &manifest).expect("manifest");
        stamp_record_mirror(&layout, true);

        let descriptor = tool_workflow::BUILTIN_WORKFLOWS
            .get("local-app-build")
            .expect("local-app built-in");
        // A hostile caller-supplied collection the manifest never declared.
        // If it survives, the workflow could be granted write access to a
        // collection the Host never authorized — worse than the `[]` case
        // the existing overwrite test covers, which never asserted a
        // non-empty caller value loses.
        let mut spec = tool_workflow::WorkflowLaunchSpec {
            name: Some("local-app-build".into()),
            args: Some(serde_json::json!({
                "app_id": "demo1234",
                "expected_writable_collections": ["attacker_secrets"],
            })),
            ..Default::default()
        };

        super::apply_materialized_local_app_collections(root.path(), &mut spec, descriptor.script)
            .expect("materialized manifest should still resolve through a hostile args block");

        assert_eq!(
            spec.args
                .as_ref()
                .and_then(|args| args.get("expected_writable_collections")),
            Some(&serde_json::json!(["items"])),
            "the Host must overwrite the caller's collection list with the manifest's, not merge it"
        );
    }

    /// §19.3, the key-SET half. The two tests above each pin ONE Host-injected
    /// key against a hostile caller value. Nothing pins the *set*, and that is
    /// the gap Phase 2 walks into: the canvas script branches on shape, so
    /// `surface` (or the next context key like it) gets added to these args,
    /// and the natural spelling for an author who thinks "the caller may have
    /// already computed it" is `object.entry(k).or_insert_with(...)`. That
    /// form honours a caller-supplied value, the whole suite stays green
    /// because no existing test puts that key in the args first, and a
    /// caller-controlled value reaches the workflow script.
    ///
    /// ## How the Host-owned / caller-owned line is drawn here
    ///
    /// This test does NOT classify keys by name, because a name list cannot
    /// know about a key that does not exist yet. It snapshots the caller's
    /// args, runs the seam, and treats exactly the keys the seam ADDED as
    /// Host-injected. Consequences:
    ///
    /// - a caller-owned key can never trip this, whatever it is named — it is
    ///   in the snapshot, so it is never in the delta. The caller args below
    ///   deliberately carry the full caller-owned surface the workflow scripts
    ///   document (`tools/workflow/src/local_app_workflow_core.js:13`:
    ///   `app_id`, `spec`, `strategy`, `complexity`, `revision_prompt`, plus
    ///   `model`, which is a Host DEFAULT the caller legitimately wins) so
    ///   that property is exercised, not just asserted.
    /// - the two known Host-injected keys are deliberately ABSENT from the
    ///   caller args, so they land in the delta and are pinned by name. That
    ///   they stay Host-owned even when the caller DOES supply them is what
    ///   the two override tests above prove; this test proves no THIRD key
    ///   joined them unnoticed.
    ///
    /// So: adding a Host-injected key here fails loudly and names it, and the
    /// only way to make it pass is to add the key to `expected` — at which
    /// point the reviewer of that diff is looking straight at the two
    /// override tests it must be accompanied by.
    #[test]
    fn host_injected_arg_keys_are_exactly_the_expected_set() {
        let root = tempfile::tempdir().expect("tempdir");
        let layout = local_apps::AppLayout::new(root.path(), "demo1234").expect("layout");
        let mut manifest = local_apps::AppManifest::for_new_app("demo1234", "Demo");
        stamp_profile(&mut manifest, local_apps::AppRuntimeProfile::ReactDom);
        manifest.collections.push(local_apps::DataCollectionSchema {
            id: "items".into(),
            name: "Items".into(),
            fields: Vec::new(),
        });
        local_apps::save_manifest(&layout, &manifest).expect("manifest");
        stamp_record_mirror(&layout, true);

        let descriptor = tool_workflow::BUILTIN_WORKFLOWS
            .get("local-app-build")
            .expect("local-app built-in");
        let caller_args = serde_json::json!({
            "app_id": "demo1234",
            "spec": "a caller-authored spec",
            "revision_prompt": "a caller-authored revision prompt",
            "strategy": "balanced",
            "complexity": {"screens": 2},
            "model": "a-caller-chosen-model",
        });
        let caller_keys: std::collections::BTreeSet<String> = caller_args
            .as_object()
            .expect("caller args object")
            .keys()
            .cloned()
            .collect();
        let mut spec = tool_workflow::WorkflowLaunchSpec {
            name: Some("local-app-build".into()),
            args: Some(caller_args),
            ..Default::default()
        };

        super::apply_materialized_local_app_collections(root.path(), &mut spec, descriptor.script)
            .expect("materialized manifest should resolve");

        let after = spec
            .args
            .as_ref()
            .and_then(serde_json::Value::as_object)
            .expect("args survive the seam as an object");
        let injected: std::collections::BTreeSet<String> = after
            .keys()
            .filter(|key| !caller_keys.contains(*key))
            .cloned()
            .collect();
        let expected: std::collections::BTreeSet<String> =
            ["expected_writable_collections", "runtime_profile"]
                .into_iter()
                .map(str::to_string)
                .collect();

        let unexpected: Vec<&String> = injected.difference(&expected).collect();
        assert!(
            unexpected.is_empty(),
            "the Host injected workflow arg key(s) {unexpected:?} that §19.3 has no \
             caller-override test for. Every Host-injected key needs a companion test \
             proving a hostile caller value loses (see \
             `caller_supplied_runtime_profile_is_overridden_by_the_host` above) and an \
             `object.insert` — NOT `entry().or_insert_with`, which honours the caller. \
             Add that test, then list the key in `expected` here. \
             Host-injected keys seen: {injected:?}"
        );
        let missing: Vec<&String> = expected.difference(&injected).collect();
        assert!(
            missing.is_empty(),
            "the Host stopped injecting the §19.3 key(s) {missing:?}; the workflow script \
             would then run on whatever the caller supplied. \
             Host-injected keys seen: {injected:?}"
        );

        // Listing a key in `expected` must not be a way to silence this gate.
        // Re-run the seam with a sentinel already sitting on EVERY expected
        // Host key: an `object.insert` key comes back with the same value it
        // had when the caller supplied nothing, while an
        // `entry().or_insert_with` key comes back holding the sentinel. So a
        // key added to `expected` without an accompanying override test fails
        // here instead of passing quietly.
        let mut hostile = serde_json::Map::new();
        for key in caller_keys.iter() {
            hostile.insert(
                key.clone(),
                after.get(key).cloned().expect("caller key survives"),
            );
        }
        for key in expected.iter() {
            hostile.insert(key.clone(), serde_json::json!("caller-sentinel"));
        }
        let mut hostile_spec = tool_workflow::WorkflowLaunchSpec {
            name: Some("local-app-build".into()),
            args: Some(serde_json::Value::Object(hostile)),
            ..Default::default()
        };
        super::apply_materialized_local_app_collections(
            root.path(),
            &mut hostile_spec,
            descriptor.script,
        )
        .expect("materialized manifest should resolve through a hostile args block");
        let hostile_after = hostile_spec
            .args
            .as_ref()
            .and_then(serde_json::Value::as_object)
            .expect("args survive the seam as an object");
        for key in expected.iter() {
            assert_eq!(
                hostile_after.get(key),
                after.get(key),
                "the Host key {key:?} is listed as Host-injected but did not OVERRIDE the \
                 caller's value — it is written with `entry().or_insert_with` (or an \
                 equivalent) instead of `object.insert`, so a caller controls it"
            );
        }

        // The caller-owned keys are untouched — this gate must not be
        // mistaken for "the Host owns the whole args map".
        assert_eq!(
            after.get("model").and_then(serde_json::Value::as_str),
            Some("a-caller-chosen-model"),
            "`model` is a Host default the caller legitimately wins"
        );
        assert_eq!(
            after.get("strategy").and_then(serde_json::Value::as_str),
            Some("balanced"),
            "`strategy` is caller-owned by design"
        );
    }

    #[test]
    fn resumed_local_app_builtin_uses_manifest_but_non_local_workflows_do_not() {
        let root = tempfile::tempdir().expect("tempdir");
        let layout = local_apps::AppLayout::new(root.path(), "demo1234").expect("layout");
        let mut manifest = local_apps::AppManifest::for_new_app("demo1234", "Demo");
        stamp_profile(&mut manifest, local_apps::AppRuntimeProfile::Canvas2d);
        manifest.collections.push(local_apps::DataCollectionSchema {
            id: "progress".into(),
            name: "Progress".into(),
            fields: Vec::new(),
        });
        local_apps::save_manifest(&layout, &manifest).expect("manifest");
        stamp_record_mirror(&layout, true);

        let descriptor = tool_workflow::BUILTIN_WORKFLOWS
            .get("local-canvas-build")
            .expect("local-canvas built-in");
        let mut resumed = tool_workflow::WorkflowLaunchSpec {
            script_path: Some("persisted-workflow.js".into()),
            resume_from_run_id: Some("wf_resume1".into()),
            args: Some(serde_json::json!({
                "app_id": "demo1234",
                "expected_writable_collections": [],
            })),
            ..Default::default()
        };
        super::apply_materialized_local_app_collections(
            root.path(),
            &mut resumed,
            descriptor.script,
        )
        .expect("resumed built-in should use manifest ids");
        assert_eq!(
            resumed
                .args
                .as_ref()
                .and_then(|args| args.get("expected_writable_collections")),
            Some(&serde_json::json!(["progress"]))
        );

        let non_local_descriptor = tool_workflow::BUILTIN_WORKFLOWS
            .get("deep-research")
            .expect("deep-research built-in");
        let mut custom = tool_workflow::WorkflowLaunchSpec {
            name: Some("deep-research".into()),
            args: Some(serde_json::json!({
                "expected_writable_collections": [],
            })),
            ..Default::default()
        };
        super::apply_materialized_local_app_collections(
            root.path(),
            &mut custom,
            non_local_descriptor.script,
        )
        .expect("non-local workflows should not read app manifests");
        assert_eq!(
            custom
                .args
                .as_ref()
                .and_then(|args| args.get("expected_writable_collections")),
            Some(&serde_json::json!([]))
        );
    }

    #[test]
    fn local_app_builtin_fails_closed_when_manifest_is_missing() {
        let root = tempfile::tempdir().expect("tempdir");
        let descriptor = tool_workflow::BUILTIN_WORKFLOWS
            .get("local-app-build")
            .expect("local-app built-in");
        let mut spec = tool_workflow::WorkflowLaunchSpec {
            name: Some("local-app-build".into()),
            args: Some(serde_json::json!({
                "app_id": "demo1234",
                "expected_writable_collections": [],
            })),
            ..Default::default()
        };
        let error = super::apply_materialized_local_app_collections(
            root.path(),
            &mut spec,
            descriptor.script,
        )
        .expect_err("missing manifest must not trust caller's empty list");
        assert!(
            error.to_string().contains("record mirror"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn local_app_workflow_requires_scaffolded_record_and_complete_manifest() {
        let root = tempfile::tempdir().expect("tempdir");
        let layout = local_apps::AppLayout::new(root.path(), "demo1234").expect("layout");
        let mut manifest = local_apps::AppManifest::for_new_app("demo1234", "Demo");
        stamp_profile(&mut manifest, local_apps::AppRuntimeProfile::ReactDom);
        local_apps::save_manifest(&layout, &manifest).expect("manifest");

        let descriptor = tool_workflow::BUILTIN_WORKFLOWS
            .get("local-app-build")
            .expect("local-app built-in");
        let mut spec = tool_workflow::WorkflowLaunchSpec {
            name: Some("local-app-build".into()),
            args: Some(serde_json::json!({
                "app_id": "demo1234",
                "expected_writable_collections": [],
            })),
            ..Default::default()
        };

        // A manifest/profile pair must not bypass the independent record
        // commit point. This models a crash after the manifest was written but
        // before `record.scaffolded` was persisted.
        stamp_record_mirror(&layout, false);
        let error = super::apply_materialized_local_app_collections(
            root.path(),
            &mut spec,
            descriptor.script,
        )
        .expect_err("an unscaffolded record must not launch a local-app workflow");
        assert!(
            error.to_string().contains("never scaffolded"),
            "unexpected error: {error}"
        );

        // The record alone is not enough either: a scaffold commit without
        // its dependency snapshot is still a partial, non-runnable app.
        stamp_record_mirror(&layout, true);
        let mut partial_manifest = manifest;
        partial_manifest.dependency_snapshot = None;
        local_apps::save_manifest(&layout, &partial_manifest).expect("partial manifest");
        let error = super::apply_materialized_local_app_collections(
            root.path(),
            &mut spec,
            descriptor.script,
        )
        .expect_err("a partial manifest must not launch a local-app workflow");
        assert!(
            error.to_string().contains("dependency snapshot"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn local_app_workflow_rejects_corrupt_runtime_contract_hash() {
        let root = tempfile::tempdir().expect("tempdir");
        let layout = local_apps::AppLayout::new(root.path(), "demo1234").expect("layout");
        let mut manifest = local_apps::AppManifest::for_new_app("demo1234", "Demo");
        stamp_profile(&mut manifest, local_apps::AppRuntimeProfile::ReactDom);
        manifest
            .runtime_profile
            .as_mut()
            .expect("profile binding")
            .contract_sha256 = "0".repeat(64);
        manifest
            .dependency_snapshot
            .as_mut()
            .expect("dependency snapshot")
            .verified_profile_contract_sha256 = "0".repeat(64);
        local_apps::save_manifest(&layout, &manifest).expect("manifest");
        stamp_record_mirror(&layout, true);

        let descriptor = tool_workflow::BUILTIN_WORKFLOWS
            .get("local-app-build")
            .expect("local-app built-in");
        let mut spec = tool_workflow::WorkflowLaunchSpec {
            name: Some("local-app-build".into()),
            args: Some(serde_json::json!({"app_id": "demo1234"})),
            ..Default::default()
        };
        let error = super::apply_materialized_local_app_collections(
            root.path(),
            &mut spec,
            descriptor.script,
        )
        .expect_err("a corrupt profile hash must fail closed");
        assert!(
            error.to_string().contains("expects contract"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn local_app_workflow_rejects_unavailable_babylon_profile() {
        let root = tempfile::tempdir().expect("tempdir");
        let layout = local_apps::AppLayout::new(root.path(), "demo1234").expect("layout");
        let mut manifest = local_apps::AppManifest::for_new_app("demo1234", "Demo");
        manifest.surface = Some(local_apps::AppSurface::Canvas);
        manifest.runtime_profile = Some(local_apps::AppRuntimeProfileBinding {
            family: local_apps::AppRuntimeProfile::Babylon3d,
            revision: 1,
            // The catalog intentionally does not publish a Babylon contract
            // until the real-device spike succeeds; any well-formed digest
            // must still be rejected as unavailable, not selected as a route.
            contract_sha256: "a".repeat(64),
        });
        manifest.dependency_snapshot = Some(local_apps::AppDependencySnapshot {
            requested_sha256: "0".repeat(64),
            package_sha256: "1".repeat(64),
            lockfile_sha256: "2".repeat(64),
            dependency_tree_sha256: "3".repeat(64),
            sbom_sha256: "4".repeat(64),
            toolchain_key: crate::local_app_runtime_profiles::RUNTIME_PROFILE_TOOLCHAIN_KEY
                .to_string(),
            verified_profile_contract_sha256: "a".repeat(64),
        });
        local_apps::save_manifest(&layout, &manifest).expect("manifest");
        stamp_record_mirror(&layout, true);

        let descriptor = tool_workflow::BUILTIN_WORKFLOWS
            .get("local-canvas-build")
            .expect("canvas local-app built-in");
        let mut spec = tool_workflow::WorkflowLaunchSpec {
            name: Some("local-canvas-build".into()),
            args: Some(serde_json::json!({"app_id": "demo1234"})),
            ..Default::default()
        };
        let error = super::apply_materialized_local_app_collections(
            root.path(),
            &mut spec,
            descriptor.script,
        )
        .expect_err("gated Babylon must not launch");
        assert!(
            error
                .to_string()
                .contains("not published in this host build"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn local_app_workflow_routes_each_published_profile_to_its_matching_workflow() {
        for (family, workflow_id) in [
            (local_apps::AppRuntimeProfile::ReactDom, "local-app-build"),
            (
                local_apps::AppRuntimeProfile::Canvas2d,
                "local-canvas-build",
            ),
            (local_apps::AppRuntimeProfile::Three3d, "local-canvas-build"),
            (
                local_apps::AppRuntimeProfile::Phaser2d,
                "local-canvas-build",
            ),
        ] {
            let root = tempfile::tempdir().expect("tempdir");
            let layout = local_apps::AppLayout::new(root.path(), "demo1234").expect("layout");
            let mut manifest = local_apps::AppManifest::for_new_app("demo1234", "Demo");
            stamp_profile(&mut manifest, family);
            local_apps::save_manifest(&layout, &manifest).expect("manifest");
            stamp_record_mirror(&layout, true);

            let descriptor = tool_workflow::BUILTIN_WORKFLOWS
                .get(workflow_id)
                .expect("local-app built-in");
            let mut spec = tool_workflow::WorkflowLaunchSpec {
                name: Some(workflow_id.into()),
                args: Some(serde_json::json!({"app_id": "demo1234"})),
                ..Default::default()
            };
            super::apply_materialized_local_app_collections(
                root.path(),
                &mut spec,
                descriptor.script,
            )
            .unwrap_or_else(|error| panic!("{family} should route to {workflow_id}: {error}"));
            assert_eq!(
                spec.args
                    .as_ref()
                    .and_then(|args| args.pointer("/runtime_profile/family"))
                    .and_then(serde_json::Value::as_str),
                Some(family.as_str()),
                "runtime profile must remain visible to the routed specialist"
            );
        }
    }

    // Characterization tests for the build-target → required-workflow
    // REFUSAL (plan v3 Phase -1, P-1.1). Before this change nothing in the
    // workspace pinned this behaviour — a grep for `refusing caller-selected
    // workflow` and `required_workflow_id` had zero hits outside the
    // production site. P-1.3 replaces the inline match these tests exercise
    // with typed routing through `local_app_plugin_binding`; these tests must
    // keep passing unchanged across that replacement, since they assert on
    // the launcher's observable behaviour (Err vs Ok, and what the error
    // names), not on how the routing is implemented.

    #[test]
    fn react_dom_app_launched_with_canvas_workflow_is_refused() {
        let root = tempfile::tempdir().expect("tempdir");
        let layout = local_apps::AppLayout::new(root.path(), "demo1234").expect("layout");
        let mut manifest = local_apps::AppManifest::for_new_app("demo1234", "Demo");
        stamp_profile(&mut manifest, local_apps::AppRuntimeProfile::ReactDom);
        local_apps::save_manifest(&layout, &manifest).expect("manifest");
        stamp_record_mirror(&layout, true);

        let descriptor = tool_workflow::BUILTIN_WORKFLOWS
            .get("local-canvas-build")
            .expect("canvas local-app built-in");
        let mut spec = tool_workflow::WorkflowLaunchSpec {
            name: Some("local-canvas-build".into()),
            args: Some(serde_json::json!({"app_id": "demo1234"})),
            ..Default::default()
        };
        let error = super::apply_materialized_local_app_collections(
            root.path(),
            &mut spec,
            descriptor.script,
        )
        .expect_err("a react-dom app must refuse a caller-selected canvas workflow");
        let message = error.to_string();
        assert!(
            message.contains("demo1234"),
            "refusal must name the app id: {message}"
        );
        // Ordered fragment, not two independent `contains`: asserting only
        // that both ids appear lets a refactor that SWAPS them ("must use
        // local-canvas-build; refusing caller-selected workflow
        // local-app-build") pass every assertion while telling the operator
        // the exact opposite of the truth. The direction is the security
        // content of this message, so it is what gets pinned.
        let expected_refusal =
            "must use local-app-build; refusing caller-selected workflow local-canvas-build";
        assert!(
            message.contains(expected_refusal),
            "refusal must read {expected_refusal:?}: {message}"
        );
    }

    #[test]
    fn canvas_family_apps_launched_with_the_dom_workflow_are_refused() {
        // Babylon3d is intentionally excluded: its runtime profile is not yet
        // published, so it fails earlier with a different error ("not
        // published in this host build") before this refusal is reached —
        // see the sibling test that pins that behaviour above.
        for family in [
            local_apps::AppRuntimeProfile::Canvas2d,
            local_apps::AppRuntimeProfile::Three3d,
            local_apps::AppRuntimeProfile::Phaser2d,
        ] {
            let root = tempfile::tempdir().expect("tempdir");
            let layout = local_apps::AppLayout::new(root.path(), "demo1234").expect("layout");
            let mut manifest = local_apps::AppManifest::for_new_app("demo1234", "Demo");
            stamp_profile(&mut manifest, family);
            local_apps::save_manifest(&layout, &manifest).expect("manifest");
            stamp_record_mirror(&layout, true);

            let descriptor = tool_workflow::BUILTIN_WORKFLOWS
                .get("local-app-build")
                .expect("dom local-app built-in");
            let mut spec = tool_workflow::WorkflowLaunchSpec {
                name: Some("local-app-build".into()),
                args: Some(serde_json::json!({"app_id": "demo1234"})),
                ..Default::default()
            };
            let error = super::apply_materialized_local_app_collections(
                root.path(),
                &mut spec,
                descriptor.script,
            )
            .expect_err(&format!(
                "a {family} app must refuse a caller-selected dom workflow"
            ));
            let message = error.to_string();
            assert!(
                message.contains("demo1234"),
                "refusal must name the app id (family={family}): {message}"
            );
            // The mirror image of the sibling test's assertion, and the
            // reason both are written as ONE ordered fragment: between the
            // two tests each id appears in both positions, so a refactor
            // that swaps them cannot stay green by symmetry.
            let expected_refusal =
                "must use local-canvas-build; refusing caller-selected workflow local-app-build";
            assert!(
                message.contains(expected_refusal),
                "refusal must read {expected_refusal:?} (family={family}): {message}"
            );
        }
    }

    #[test]
    fn named_local_app_with_custom_inline_script_is_not_a_builtin() {
        let root = tempfile::tempdir().expect("tempdir");
        let mut spec = tool_workflow::WorkflowLaunchSpec {
            name: Some("local-app-build".into()),
            script: Some("export const meta = { name: 'custom' }; return 1".into()),
            args: Some(serde_json::json!({
                "app_id": "demo1234",
                "expected_writable_collections": ["caller_choice"],
            })),
            ..Default::default()
        };
        let script = spec.script.clone().expect("inline script");

        super::apply_materialized_local_app_collections(root.path(), &mut spec, &script)
            .expect("custom inline workflow must not read a missing manifest");

        assert_eq!(
            spec.args
                .as_ref()
                .and_then(|args| args.get("expected_writable_collections")),
            Some(&serde_json::json!(["caller_choice"]))
        );

        let custom_path = root.path().join("custom-local-app.js");
        let custom_path_script = "export const meta = { name: 'local-app-build' };\nreturn 2\n";
        std::fs::write(&custom_path, custom_path_script).expect("custom script path");
        let mut path_spec = tool_workflow::WorkflowLaunchSpec {
            name: Some("local-app-build".into()),
            script_path: Some(custom_path.to_string_lossy().into_owned()),
            args: Some(serde_json::json!({
                "app_id": "demo1234",
                "expected_writable_collections": ["caller_choice"],
            })),
            ..Default::default()
        };
        super::apply_materialized_local_app_collections(
            root.path(),
            &mut path_spec,
            custom_path_script,
        )
        .expect("custom same-meta script path must not read a missing manifest");
        assert_eq!(
            path_spec
                .args
                .as_ref()
                .and_then(|args| args.get("expected_writable_collections")),
            Some(&serde_json::json!(["caller_choice"]))
        );
    }

    #[test]
    fn trusted_legacy_local_app_resume_uses_checkpoint_provenance() {
        let root = tempfile::tempdir().expect("tempdir");
        let layout = local_apps::AppLayout::new(root.path(), "demo1234").expect("layout");
        let mut manifest = local_apps::AppManifest::for_new_app("demo1234", "Demo");
        stamp_profile(&mut manifest, local_apps::AppRuntimeProfile::ReactDom);
        manifest.collections.push(local_apps::DataCollectionSchema {
            id: "progress".into(),
            name: "Progress".into(),
            fields: Vec::new(),
        });
        local_apps::save_manifest(&layout, &manifest).expect("manifest");
        stamp_record_mirror(&layout, true);

        let session = "00000000-0000-0000-0000-000000000003";
        let run_id = "wf_legacy1";
        let script_path = root.path().join("legacy-local-app.js");
        let legacy_script = "export const meta = { name: 'legacy-local-app' };\nreturn 1\n";
        std::fs::write(&script_path, legacy_script).expect("legacy script");
        let checkpoints = super::MobileWorkflowCheckpointStore::new(
            root.path().join(".claude"),
            root.path().to_path_buf(),
        );
        checkpoints
            .upsert(
                session,
                super::WorkflowCheckpoint {
                    task_id: "wlegacy01".into(),
                    workflow_run_id: run_id.into(),
                    workflow_id: "local-app-build".into(),
                    script_path: script_path.to_string_lossy().into_owned(),
                    script_sha256: Some(super::sha256_hex(legacy_script.as_bytes())),
                    script_is_verbatim_builtin: None,
                    args_json: Some(
                        serde_json::json!({
                            "app_id": "demo1234",
                            "expected_writable_collections": [],
                        })
                        .to_string(),
                    ),
                    description: "Legacy local app build".into(),
                    start_time: None,
                    transcript_dir: root
                        .path()
                        .join("transcript")
                        .to_string_lossy()
                        .into_owned(),
                },
            )
            .expect("checkpoint");

        let mut trusted = tool_workflow::WorkflowLaunchSpec {
            script_path: Some(script_path.to_string_lossy().into_owned()),
            resume_from_run_id: Some(run_id.into()),
            session_uuid: Some(session.into()),
            args: Some(serde_json::json!({
                "app_id": "demo1234",
                "expected_writable_collections": [],
            })),
            ..Default::default()
        };
        assert!(checkpoints.is_trusted_local_app_resume(session, run_id, &script_path,));
        let error = super::apply_materialized_local_app_collections_with_provenance(
            root.path(),
            &mut trusted,
            legacy_script,
            true,
        )
        .expect_err("obsolete trusted local-app resume must remain rejected");
        assert!(
            error.to_string().contains("obsolete local-app"),
            "unexpected error: {error}"
        );

        // Before the marker field existed, a custom script could reuse the
        // built-in name and still be indistinguishable from an old bundled
        // checkpoint. Treat that ambiguity as legacy and reject it before
        // execution instead of allowing caller-supplied persistence args.
        let markerless_same_meta_run_id = "wf_legacy-meta1";
        let markerless_same_meta_path = root.path().join("legacy-same-meta.js");
        let markerless_same_meta_script =
            "export const meta = { name: 'local-app-build' };\nreturn 'legacy-custom'\n";
        std::fs::write(&markerless_same_meta_path, markerless_same_meta_script)
            .expect("markerless same-meta script");
        checkpoints
            .upsert(
                session,
                super::WorkflowCheckpoint {
                    task_id: "wlegacy02".into(),
                    workflow_run_id: markerless_same_meta_run_id.into(),
                    workflow_id: "local-app-build".into(),
                    script_path: markerless_same_meta_path.to_string_lossy().into_owned(),
                    script_sha256: Some(super::sha256_hex(markerless_same_meta_script.as_bytes())),
                    script_is_verbatim_builtin: None,
                    args_json: None,
                    description: "Markerless custom local app workflow".into(),
                    start_time: None,
                    transcript_dir: root
                        .path()
                        .join("markerless-custom-transcript")
                        .to_string_lossy()
                        .into_owned(),
                },
            )
            .expect("markerless same-meta checkpoint");
        assert_eq!(
            checkpoints.local_app_resume_provenance(
                session,
                markerless_same_meta_run_id,
                &markerless_same_meta_path,
            ),
            Some(super::LocalAppResumeProvenance::LegacyBuiltin)
        );
        let mut markerless_same_meta = tool_workflow::WorkflowLaunchSpec {
            name: Some("local-app-build".into()),
            script_path: Some(markerless_same_meta_path.to_string_lossy().into_owned()),
            resume_from_run_id: Some(markerless_same_meta_run_id.into()),
            session_uuid: Some(session.into()),
            args: Some(serde_json::json!({
                "app_id": "demo1234",
                "expected_writable_collections": [],
            })),
            ..Default::default()
        };
        let error = super::apply_materialized_local_app_collections_with_provenance(
            root.path(),
            &mut markerless_same_meta,
            markerless_same_meta_script,
            true,
        )
        .expect_err("markerless same-meta custom resume must be rejected as legacy");
        assert!(
            error.to_string().contains("obsolete local-app"),
            "unexpected error: {error}"
        );

        let current_run_id = "wf_current1";
        let current_path = root.path().join("current-local-app.js");
        let current_descriptor = tool_workflow::BUILTIN_WORKFLOWS
            .get("local-app-build")
            .expect("local-app built-in")
            .script;
        std::fs::write(&current_path, current_descriptor).expect("current script");
        checkpoints
            .upsert(
                session,
                super::WorkflowCheckpoint {
                    task_id: "wcurrent1".into(),
                    workflow_run_id: current_run_id.into(),
                    workflow_id: "local-app-build".into(),
                    script_path: current_path.to_string_lossy().into_owned(),
                    script_sha256: Some(super::sha256_hex(current_descriptor.as_bytes())),
                    script_is_verbatim_builtin: Some(true),
                    args_json: None,
                    description: "Current local app build".into(),
                    start_time: None,
                    transcript_dir: root
                        .path()
                        .join("current-transcript")
                        .to_string_lossy()
                        .into_owned(),
                },
            )
            .expect("current checkpoint");
        assert_eq!(
            checkpoints.local_app_resume_provenance(session, current_run_id, &current_path,),
            Some(super::LocalAppResumeProvenance::CurrentBuiltin)
        );
        let mut current = tool_workflow::WorkflowLaunchSpec {
            script_path: Some(current_path.to_string_lossy().into_owned()),
            resume_from_run_id: Some(current_run_id.into()),
            session_uuid: Some(session.into()),
            args: Some(serde_json::json!({
                "app_id": "demo1234",
                "expected_writable_collections": [],
            })),
            ..Default::default()
        };
        super::apply_materialized_local_app_collections_with_provenance(
            root.path(),
            &mut current,
            current_descriptor,
            true,
        )
        .expect("current trusted local-app resume should use manifest ids");
        assert_eq!(
            current
                .args
                .as_ref()
                .and_then(|args| args.get("expected_writable_collections")),
            Some(&serde_json::json!(["progress"]))
        );
        let mut checkpoint_dom_to_canvas = current.clone();
        let checkpoint_resolution = super::local_app_resume_resolution_for_launch(
            &checkpoints,
            &checkpoint_dom_to_canvas,
            session,
            current_run_id,
            &current_path,
            tool_workflow::BUILTIN_WORKFLOWS
                .get("local-canvas-build")
                .expect("canvas local-app built-in")
                .script,
        )
        .expect("checkpoint provenance");
        assert_eq!(
            checkpoint_resolution.workflow_id.as_deref(),
            Some("local-app-build")
        );
        super::apply_materialized_local_app_collections_with_identity(
            root.path(),
            &mut checkpoint_dom_to_canvas,
            tool_workflow::BUILTIN_WORKFLOWS
                .get("local-canvas-build")
                .expect("canvas local-app built-in")
                .script,
            true,
            checkpoint_resolution.workflow_id.as_deref(),
        )
        .expect_err("checkpoint DOM provenance must reject a canvas swap");

        let custom_run_id = "wf_custom1";
        let custom_path = root.path().join("custom-local-app.js");
        let custom_script = "export const meta = { name: 'local-app-build' };\nreturn 'custom'\n";
        std::fs::write(&custom_path, custom_script).expect("custom script");
        checkpoints
            .upsert(
                session,
                super::WorkflowCheckpoint {
                    task_id: "wcustom1".into(),
                    workflow_run_id: custom_run_id.into(),
                    workflow_id: "local-app-build".into(),
                    script_path: custom_path.to_string_lossy().into_owned(),
                    script_sha256: Some(super::sha256_hex(custom_script.as_bytes())),
                    script_is_verbatim_builtin: Some(false),
                    args_json: None,
                    description: "Custom local app workflow".into(),
                    start_time: None,
                    transcript_dir: root
                        .path()
                        .join("custom-transcript")
                        .to_string_lossy()
                        .into_owned(),
                },
            )
            .expect("custom checkpoint");
        assert_eq!(
            checkpoints.local_app_resume_provenance(session, custom_run_id, &custom_path,),
            Some(super::LocalAppResumeProvenance::Custom)
        );
        let mut custom_resume = tool_workflow::WorkflowLaunchSpec {
            name: Some("local-app-build".into()),
            script_path: Some(custom_path.to_string_lossy().into_owned()),
            resume_from_run_id: Some(custom_run_id.into()),
            session_uuid: Some(session.into()),
            args: Some(serde_json::json!({
                "app_id": "demo1234",
                "expected_writable_collections": [],
            })),
            ..Default::default()
        };
        super::apply_materialized_local_app_collections_with_provenance(
            root.path(),
            &mut custom_resume,
            custom_script,
            false,
        )
        .expect("marker=false same-meta custom resume should remain custom");
        assert_eq!(
            custom_resume
                .args
                .as_ref()
                .and_then(|args| args.get("expected_writable_collections")),
            Some(&serde_json::json!([]))
        );

        let mut untrusted = tool_workflow::WorkflowLaunchSpec {
            script_path: Some(script_path.to_string_lossy().into_owned()),
            resume_from_run_id: Some(run_id.into()),
            session_uuid: Some(session.into()),
            args: Some(serde_json::json!({
                "app_id": "demo1234",
                "expected_writable_collections": [],
            })),
            ..Default::default()
        };
        let no_checkpoint = super::MobileWorkflowCheckpointStore::new(
            root.path().join("missing-checkpoint-home"),
            root.path().to_path_buf(),
        );
        assert!(!no_checkpoint.is_trusted_local_app_resume(session, run_id, &script_path,));
        super::apply_materialized_local_app_collections_with_provenance(
            root.path(),
            &mut untrusted,
            legacy_script,
            false,
        )
        .expect("untrusted legacy resume should remain a custom workflow");
        assert_eq!(
            untrusted
                .args
                .as_ref()
                .and_then(|args| args.get("expected_writable_collections")),
            Some(&serde_json::json!([]))
        );

        let wrong_path = root.path().join("wrong-path.js");
        std::fs::write(&wrong_path, legacy_script).expect("wrong-path script");
        assert!(!checkpoints.is_trusted_local_app_resume(session, run_id, &wrong_path,));
        let mut wrong_path_spec = tool_workflow::WorkflowLaunchSpec {
            script_path: Some(wrong_path.to_string_lossy().into_owned()),
            resume_from_run_id: Some(run_id.into()),
            session_uuid: Some(session.into()),
            args: Some(serde_json::json!({
                "app_id": "demo1234",
                "expected_writable_collections": [],
            })),
            ..Default::default()
        };
        super::apply_materialized_local_app_collections_with_provenance(
            root.path(),
            &mut wrong_path_spec,
            legacy_script,
            false,
        )
        .expect("a run id must not authorize a different script path");
        assert_eq!(
            wrong_path_spec
                .args
                .as_ref()
                .and_then(|args| args.get("expected_writable_collections")),
            Some(&serde_json::json!([]))
        );

        let current_descriptor = tool_workflow::BUILTIN_WORKFLOWS
            .get("local-app-build")
            .expect("local-app built-in")
            .script;
        let current_path = root.path().join("current-path.js");
        std::fs::write(&current_path, current_descriptor).expect("current script");
        let mut exact_bytes_untrusted = tool_workflow::WorkflowLaunchSpec {
            script_path: Some(current_path.to_string_lossy().into_owned()),
            resume_from_run_id: Some(run_id.into()),
            session_uuid: Some(session.into()),
            args: Some(serde_json::json!({
                "app_id": "demo1234",
                "expected_writable_collections": [],
            })),
            ..Default::default()
        };
        super::apply_materialized_local_app_collections_with_provenance(
            root.path(),
            &mut exact_bytes_untrusted,
            current_descriptor,
            false,
        )
        .expect("exact current bundled bytes remain authoritative without provenance");
        assert_eq!(
            exact_bytes_untrusted
                .args
                .as_ref()
                .and_then(|args| args.get("expected_writable_collections")),
            Some(&serde_json::json!(["progress"]))
        );
    }

    #[test]
    fn cold_start_provenance_sidecar_gates_legacy_scratch_resume() {
        let root = tempfile::tempdir().expect("tempdir");
        let layout = local_apps::AppLayout::new(root.path(), "demo1234").expect("layout");
        let mut manifest = local_apps::AppManifest::for_new_app("demo1234", "Demo");
        manifest.collections.push(local_apps::DataCollectionSchema {
            id: "progress".into(),
            name: "Progress".into(),
            fields: Vec::new(),
        });
        local_apps::save_manifest(&layout, &manifest).expect("manifest");

        let session = "00000000-0000-0000-0000-000000000004";
        let store = super::MobileWorkflowCheckpointStore::new(
            root.path().join(".claude"),
            root.path().to_path_buf(),
        );
        let cold_store = || {
            super::MobileWorkflowCheckpointStore::new(
                root.path().join(".claude"),
                root.path().to_path_buf(),
            )
        };

        // A pre-sidecar terminal run leaves only its host-owned persisted
        // script. Its local-app identity is enough to fail closed, but an
        // ordinary custom script at another path remains untouched below.
        let legacy_run_id = "wf_legacy-cold1";
        let legacy_path = store.persisted_workflow_script_path(session, legacy_run_id);
        std::fs::create_dir_all(legacy_path.parent().expect("legacy parent"))
            .expect("legacy script directory");
        let legacy_script =
            "export const meta = { name: 'local-app-build' };\nreturn 'legacy body'\n";
        std::fs::write(&legacy_path, legacy_script).expect("legacy script");
        assert!(
            !store.path(session).exists(),
            "no terminal checkpoint remains"
        );
        let mut legacy_spec = tool_workflow::WorkflowLaunchSpec {
            name: Some("local-app-build".into()),
            script_path: Some(legacy_path.to_string_lossy().into_owned()),
            resume_from_run_id: Some(legacy_run_id.into()),
            session_uuid: Some(session.into()),
            args: Some(serde_json::json!({
                "app_id": "demo1234",
                "expected_writable_collections": [],
            })),
            ..Default::default()
        };
        assert_eq!(
            super::local_app_resume_provenance_for_launch(
                &cold_store(),
                &legacy_spec,
                session,
                legacy_run_id,
                &legacy_path,
                legacy_script,
            ),
            Some(super::LocalAppResumeProvenance::LegacyBuiltin)
        );
        let error = super::apply_materialized_local_app_collections_with_provenance(
            root.path(),
            &mut legacy_spec,
            legacy_script,
            true,
        )
        .expect_err("legacy host scratch resume must be rejected");
        assert!(error.to_string().contains("obsolete local-app"));

        // A new custom same-meta workflow records marker=false in the
        // transcript sidecar. A fresh process can therefore resume it without
        // inheriting the local-app manifest gate.
        let custom_run_id = "wf_custom-cold1";
        let custom_path = store.persisted_workflow_script_path(session, custom_run_id);
        let custom_script =
            "export const meta = { name: 'local-app-build' };\nreturn 'custom body'\n";
        std::fs::write(&custom_path, custom_script).expect("custom script");
        store
            .write_provenance_sidecar(
                session,
                custom_run_id,
                &custom_path,
                custom_script,
                "local-app-build",
                false,
            )
            .expect("custom provenance sidecar");
        let mut custom_spec = tool_workflow::WorkflowLaunchSpec {
            name: Some("local-app-build".into()),
            script_path: Some(custom_path.to_string_lossy().into_owned()),
            resume_from_run_id: Some(custom_run_id.into()),
            session_uuid: Some(session.into()),
            args: Some(serde_json::json!({
                "app_id": "demo1234",
                "expected_writable_collections": [],
            })),
            ..Default::default()
        };
        assert_eq!(
            super::local_app_resume_provenance_for_launch(
                &cold_store(),
                &custom_spec,
                session,
                custom_run_id,
                &custom_path,
                custom_script,
            ),
            Some(super::LocalAppResumeProvenance::Custom)
        );
        super::apply_materialized_local_app_collections_with_provenance(
            root.path(),
            &mut custom_spec,
            custom_script,
            false,
        )
        .expect("marker=false custom sidecar should remain custom");
        assert_eq!(
            custom_spec
                .args
                .as_ref()
                .and_then(|args| args.get("expected_writable_collections")),
            Some(&serde_json::json!([]))
        );

        let dom_run_id = "wf_dom-swap1";
        let dom_path = store.persisted_workflow_script_path(session, dom_run_id);
        let dom_descriptor = tool_workflow::BUILTIN_WORKFLOWS
            .get("local-app-build")
            .expect("dom local-app built-in")
            .script;
        std::fs::write(&dom_path, dom_descriptor).expect("dom script");
        store
            .write_provenance_sidecar(
                session,
                dom_run_id,
                &dom_path,
                dom_descriptor,
                "local-app-build",
                true,
            )
            .expect("dom provenance sidecar");
        let mut dom_to_canvas = tool_workflow::WorkflowLaunchSpec {
            name: Some("local-canvas-build".into()),
            script_path: Some(dom_path.to_string_lossy().into_owned()),
            resume_from_run_id: Some(dom_run_id.into()),
            session_uuid: Some(session.into()),
            args: Some(serde_json::json!({
                "app_id": "demo1234",
                "expected_writable_collections": [],
            })),
            ..Default::default()
        };
        let dom_to_canvas_resolution = super::local_app_resume_resolution_for_launch(
            &cold_store(),
            &dom_to_canvas,
            session,
            dom_run_id,
            &dom_path,
            tool_workflow::BUILTIN_WORKFLOWS
                .get("local-canvas-build")
                .expect("canvas local-app built-in")
                .script,
        )
        .expect("dom provenance");
        assert_eq!(
            dom_to_canvas_resolution.workflow_id.as_deref(),
            Some("local-app-build")
        );
        super::apply_materialized_local_app_collections_with_identity(
            root.path(),
            &mut dom_to_canvas,
            tool_workflow::BUILTIN_WORKFLOWS
                .get("local-canvas-build")
                .expect("canvas local-app built-in")
                .script,
            true,
            dom_to_canvas_resolution.workflow_id.as_deref(),
        )
        .expect_err("DOM provenance must reject a canvas script swap");

        let canvas_run_id = "wf_canvas-swap1";
        let canvas_path = store.persisted_workflow_script_path(session, canvas_run_id);
        let canvas_descriptor = tool_workflow::BUILTIN_WORKFLOWS
            .get("local-canvas-build")
            .expect("canvas local-app built-in")
            .script;
        std::fs::write(&canvas_path, canvas_descriptor).expect("canvas script");
        store
            .write_provenance_sidecar(
                session,
                canvas_run_id,
                &canvas_path,
                canvas_descriptor,
                "local-canvas-build",
                true,
            )
            .expect("canvas provenance sidecar");
        let mut canvas_to_dom = tool_workflow::WorkflowLaunchSpec {
            name: Some("local-app-build".into()),
            script_path: Some(canvas_path.to_string_lossy().into_owned()),
            resume_from_run_id: Some(canvas_run_id.into()),
            session_uuid: Some(session.into()),
            args: Some(serde_json::json!({
                "app_id": "demo1234",
                "expected_writable_collections": [],
            })),
            ..Default::default()
        };
        let canvas_to_dom_resolution = super::local_app_resume_resolution_for_launch(
            &cold_store(),
            &canvas_to_dom,
            session,
            canvas_run_id,
            &canvas_path,
            dom_descriptor,
        )
        .expect("canvas provenance");
        assert_eq!(
            canvas_to_dom_resolution.workflow_id.as_deref(),
            Some("local-canvas-build")
        );
        super::apply_materialized_local_app_collections_with_identity(
            root.path(),
            &mut canvas_to_dom,
            dom_descriptor,
            true,
            canvas_to_dom_resolution.workflow_id.as_deref(),
        )
        .expect_err("canvas provenance must reject a DOM script swap");

        let provenance_path = store.provenance_path(session, custom_run_id);
        let mut tampered_value: serde_json::Value = serde_json::from_slice(
            &std::fs::read(&provenance_path).expect("read custom provenance"),
        )
        .expect("parse custom provenance");
        tampered_value["workflowId"] = serde_json::json!("custom-workflow");
        std::fs::write(
            &provenance_path,
            serde_json::to_vec_pretty(&tampered_value).expect("serialize tampered identity"),
        )
        .expect("tamper workflow identity");
        assert_eq!(
            super::local_app_resume_provenance_for_launch(
                &cold_store(),
                &custom_spec,
                session,
                custom_run_id,
                &custom_path,
                custom_script,
            ),
            Some(super::LocalAppResumeProvenance::LegacyBuiltin)
        );
        let mut tampered_identity = custom_spec.clone();
        super::apply_materialized_local_app_collections_with_provenance(
            root.path(),
            &mut tampered_identity,
            custom_script,
            true,
        )
        .expect_err("tampered local-app identity must fail closed");

        let current_descriptor = tool_workflow::BUILTIN_WORKFLOWS
            .get("local-app-build")
            .expect("local-app built-in")
            .script;
        tampered_value["workflowId"] = serde_json::json!("local-app-build");
        tampered_value["scriptSha256"] =
            serde_json::json!(super::sha256_hex(current_descriptor.as_bytes()));
        std::fs::write(
            &provenance_path,
            serde_json::to_vec_pretty(&tampered_value).expect("serialize tampered hash"),
        )
        .expect("tamper script hash");
        assert_eq!(
            super::local_app_resume_provenance_for_launch(
                &cold_store(),
                &custom_spec,
                session,
                custom_run_id,
                &custom_path,
                custom_script,
            ),
            Some(super::LocalAppResumeProvenance::LegacyBuiltin)
        );

        // A path change cannot inherit the sidecar's custom marker; the
        // invalid path is treated as local-app legacy and fails closed.
        let different_path = root.path().join("different.js");
        std::fs::write(&different_path, custom_script).expect("different path script");
        let mut different_path_spec = tool_workflow::WorkflowLaunchSpec {
            name: Some("local-app-build".into()),
            script_path: Some(different_path.to_string_lossy().into_owned()),
            resume_from_run_id: Some(custom_run_id.into()),
            session_uuid: Some(session.into()),
            args: Some(serde_json::json!({
                "app_id": "demo1234",
                "expected_writable_collections": [],
            })),
            ..Default::default()
        };
        assert_eq!(
            super::local_app_resume_provenance_for_launch(
                &cold_store(),
                &different_path_spec,
                session,
                custom_run_id,
                &different_path,
                custom_script,
            ),
            Some(super::LocalAppResumeProvenance::LegacyBuiltin)
        );
        super::apply_materialized_local_app_collections_with_provenance(
            root.path(),
            &mut different_path_spec,
            custom_script,
            true,
        )
        .expect_err("different caller path must not inherit provenance");

        // No sidecar plus a custom body/meta at a non-host path remains a
        // direct custom workflow, even when its args happen to contain app_id.
        let ordinary_path = root.path().join("ordinary-custom.js");
        let ordinary_script = "export const meta = { name: 'custom-workflow' };\nreturn 1\n";
        std::fs::write(&ordinary_path, ordinary_script).expect("ordinary custom script");
        let mut ordinary_spec = tool_workflow::WorkflowLaunchSpec {
            script_path: Some(ordinary_path.to_string_lossy().into_owned()),
            resume_from_run_id: Some("wf_ordinary1".into()),
            session_uuid: Some(session.into()),
            args: Some(serde_json::json!({
                "app_id": "demo1234",
                "expected_writable_collections": ["caller_choice"],
            })),
            ..Default::default()
        };
        assert_eq!(
            super::local_app_resume_provenance_for_launch(
                &cold_store(),
                &ordinary_spec,
                session,
                "wf_ordinary1",
                &ordinary_path,
                ordinary_script,
            ),
            None
        );
        super::apply_materialized_local_app_collections_with_provenance(
            root.path(),
            &mut ordinary_spec,
            ordinary_script,
            false,
        )
        .expect("ordinary custom resume should remain unchanged");
        assert_eq!(
            ordinary_spec
                .args
                .as_ref()
                .and_then(|args| args.get("expected_writable_collections")),
            Some(&serde_json::json!(["caller_choice"]))
        );
    }

    #[tokio::test]
    async fn workflow_progress_sink_calls_structured_listener() {
        let listener = Arc::new(FakeListener::default());
        let root = tempfile::tempdir().expect("tempdir");
        let checkpoints = Arc::new(super::MobileWorkflowCheckpointStore::new(
            root.path().join(".claude"),
            root.path().to_path_buf(),
        ));
        let active_session = Arc::new(std::sync::Mutex::new("session-a".to_string()));
        let sink = super::MobileWorkflowStatusSink::new(
            listener.clone(),
            checkpoints,
            active_session.clone(),
        );

        let progress = tasks::handlers::local_workflow::WorkflowProgressUpdate {
            kind: "workflow_agent".to_string(),
            index: 2,
            title: None,
            message: None,
            label: Some("Design".to_string()),
            phase_index: Some(1),
            phase_title: Some("Design".to_string()),
            agent_id: Some("agent:123".to_string()),
            agent_type: Some("workflow-subagent".to_string()),
            model: Some("gpt-5.4".to_string()),
            fallback_model: None,
            state: Some("running".to_string()),
            error: None,
            tool_use_id: Some("workflow_agent_2_agent:123".to_string()),
            queued_at_ms: Some(10),
            started_at_ms: Some(20),
            last_progress_at_ms: Some(30),
            attempt: Some(1),
            last_attempt_reason: None,
            tokens: Some(40),
            tool_calls: Some(3),
            last_tool_name: Some("Read".to_string()),
            last_tool_summary: Some("Read".to_string()),
            prompt_preview: Some("design screen".to_string()),
        };

        // The worker can emit before launch() has persisted the checkpoint.
        // That event must be retained and replayed once ownership is known.
        tasks::handlers::local_workflow::WorkflowProgressSink::emit_workflow_progress(
            &sink,
            "task_123",
            "wf_abcdef",
            progress.clone(),
        )
        .await;
        assert!(listener.workflow_progress.lock().await.is_empty());

        let pending = sink
            .checkpoints
            .track_task_owner("task_123", "session-a", "wf_abcdef");
        sink.flush_pending_events("task_123", pending).await;

        let events = listener.workflow_progress.lock().await.clone();
        assert!(events
            .iter()
            .any(|(session_id, task_id, run_id, progress)| {
                session_id == "session-a"
                    && task_id == "task_123"
                    && run_id == "wf_abcdef"
                    && progress.kind == "workflow_agent"
                    && progress.index == 2
                    && progress.label.as_deref() == Some("Design")
                    && progress.agent_type.as_deref() == Some("workflow-subagent")
                    && progress.tokens == Some(40)
            }));

        *active_session.lock().unwrap() = "session-b".to_string();
        tasks::handlers::local_workflow::WorkflowProgressSink::emit_workflow_progress(
            &sink,
            "task_123",
            "wf_abcdef",
            tasks::handlers::local_workflow::WorkflowProgressUpdate {
                kind: "workflow_agent".to_string(),
                index: 3,
                title: None,
                message: None,
                label: None,
                phase_index: None,
                phase_title: None,
                agent_id: None,
                agent_type: None,
                model: None,
                fallback_model: None,
                state: Some("running".to_string()),
                error: None,
                tool_use_id: None,
                queued_at_ms: None,
                started_at_ms: None,
                last_progress_at_ms: None,
                attempt: None,
                last_attempt_reason: None,
                tokens: None,
                tool_calls: None,
                last_tool_name: None,
                last_tool_summary: None,
                prompt_preview: None,
            },
        )
        .await;
        assert_eq!(listener.workflow_progress.lock().await.len(), 1);
    }
}

#[cfg(test)]
mod workspace_lease_forwarding_tests {
    use std::sync::{Arc, Mutex as StdMutex};

    use traits::tool_invoker::{SubagentInvocationContext, ToolInvoker, ToolInvokerError};

    /// Terminal invoker that records the lease token it was dispatched with.
    struct RecordingInvoker {
        seen: Arc<StdMutex<Option<Option<u64>>>>,
    }

    #[async_trait::async_trait]
    impl ToolInvoker for RecordingInvoker {
        async fn invoke(
            &self,
            _name: &str,
            _input: serde_json::Value,
            _ctx: SubagentInvocationContext,
        ) -> Result<serde_json::Value, ToolInvokerError> {
            *self.seen.lock().unwrap() = Some(None);
            Ok(serde_json::json!({}))
        }

        async fn invoke_with_workspace_lease(
            &self,
            _name: &str,
            _input: serde_json::Value,
            _ctx: SubagentInvocationContext,
            workspace_lease_token: Option<u64>,
        ) -> Result<serde_json::Value, ToolInvokerError> {
            *self.seen.lock().unwrap() = Some(workspace_lease_token);
            Ok(serde_json::json!({}))
        }

        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    fn bare_ctx() -> SubagentInvocationContext {
        SubagentInvocationContext {
            parent_agent_id: None,
            agent_name: None,
            team_name: None,
            is_async: false,
            is_non_interactive_session: false,
            can_show_permission_prompts: false,
            cwd: None,
            tool_use_id: None,
            depth: 0,
            observer: None,
            parent_model: None,
            parent_model_profile: None,
            mode_override: None,
            frozen_command_denies: Vec::new(),
        }
    }

    /// A lease token handed to the deferred invoker must reach the real
    /// invoker underneath it. The trait's delegating default drops it, which
    /// left `PermissionCheckContext.workspace_lease_token` permanently `None`
    /// in production: the lease ALLOW never fired, and the paired
    /// `denies_host_owned_for_token` hard deny never fired either.
    #[tokio::test]
    async fn deferred_invoker_forwards_the_workspace_lease_token() {
        let seen = Arc::new(StdMutex::new(None));
        let deferred = super::DeferredToolInvoker::new();
        deferred.set(Arc::new(RecordingInvoker { seen: seen.clone() }));

        deferred
            .invoke_with_workspace_lease("Read", serde_json::json!({}), bare_ctx(), Some(77))
            .await
            .expect("dispatch");

        assert_eq!(
            *seen.lock().unwrap(),
            Some(Some(77)),
            "the deferred invoker must forward the lease token, not swallow it"
        );
    }
}
