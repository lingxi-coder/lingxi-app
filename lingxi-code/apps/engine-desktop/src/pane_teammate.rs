//! Session-owned transport for persistent teammates in terminal panes.
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use coordinator::{CoordinatorStatusSink, SendMessageTool, TeamRegistry};
use platform_api::runtime::RuntimeSpawner;
use platform_api::subagent_spawn::{SubagentInheritance, SubagentSpawnRequest};
use platform_api::swarm::{PaneId, PanePosition, SwarmBackend};
use platform_api::team_spawn::{PaneLaunchMetadata, TeamSpawnError, TeamSpawnSeam};
use platform_api::teammate_worker::{PaneTeammateManifest, ParentToWorker, WorkerToParent};
use platform_api::OutputStream;
use protocol::{AgentId, SessionId};
use tasks::handlers::TaskStatusSink;
use tasks::registry::TaskRegistry;
use tasks::{TaskSpawnInput, TaskStatus, TaskType};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::OwnedWriteHalf;
use tokio::net::UnixListener;
use tokio::sync::Mutex;
use tool_api::context::ToolUseContext;
use tool_api::tool_trait::Tool;

const MAX_FRAME_BYTES: usize = 1024 * 1024;
const CONTROL_WRITE_TIMEOUT: Duration = Duration::from_secs(2);
const BACKEND_CLEANUP_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_EARLY_FRAMES: usize = 128;
const MAX_EARLY_BYTES: usize = 4 * 1024 * 1024;

/// Keep partial line bytes across select cancellation and cap every frame
/// before allocating an unbounded JSON line from the worker socket.
struct WorkerReader<R> {
    reader: R,
    partial: Vec<u8>,
}

impl<R: AsyncBufRead + Unpin> WorkerReader<R> {
    fn new(reader: R) -> Self {
        Self {
            reader,
            partial: Vec::new(),
        }
    }

    async fn next_frame(&mut self) -> Result<Option<WorkerToParent>, TeamSpawnError> {
        let remaining = MAX_FRAME_BYTES.saturating_sub(self.partial.len()) + 1;
        let read = (&mut self.reader)
            .take(remaining as u64)
            .read_until(b'\n', &mut self.partial)
            .await
            .map_err(internal)?;
        if self.partial.len() > MAX_FRAME_BYTES {
            return Err(internal("Teammate frame exceeds 1 MiB"));
        }
        if read == 0 && self.partial.is_empty() {
            return Ok(None);
        }
        if !self.partial.ends_with(b"\n") {
            return Err(internal("Incomplete teammate frame"));
        }
        let frame = serde_json::from_slice(&self.partial).map_err(internal)?;
        self.partial.clear();
        Ok(Some(frame))
    }
}

async fn authenticate_until_ready<R: AsyncBufRead + Unpin>(
    reader: &mut WorkerReader<R>,
    token: &str,
) -> Result<VecDeque<WorkerToParent>, TeamSpawnError> {
    match reader.next_frame().await? {
        Some(WorkerToParent::Hello { token: received }) if received == token => {}
        _ => return Err(internal("Teammate authentication failed")),
    }
    let mut early = VecDeque::new();
    let mut bytes = 0;
    loop {
        let message = reader
            .next_frame()
            .await?
            .ok_or_else(|| internal("Teammate closed before Ready"))?;
        match message {
            WorkerToParent::Ready { .. } => return Ok(early),
            WorkerToParent::Hello { .. } => {
                return Err(internal("Repeated teammate authentication"))
            }
            WorkerToParent::State {
                ref status,
                ref error,
                ..
            } if matches!(status.as_str(), "failed" | "killed") => {
                return Err(internal(format!(
                    "Teammate terminated before Ready ({status}): {}",
                    error.as_deref().unwrap_or("no failure detail")
                )));
            }
            _ => {
                bytes += serde_json::to_vec(&message).map_err(internal)?.len();
                if early.len() >= MAX_EARLY_FRAMES || bytes > MAX_EARLY_BYTES {
                    return Err(internal("Teammate exceeded pre-Ready message buffer"));
                }
                early.push_back(message);
            }
        }
    }
}

struct PaneTask {
    writer: Arc<Mutex<OwnedWriteHalf>>,
    pane: PaneId,
    directory: PathBuf,
    metadata: PaneLaunchMetadata,
    backend: Arc<dyn SwarmBackend>,
}

/// Live backend selection resolved for one new teammate launch.
pub(crate) struct PaneBackendSelection {
    pub(crate) backend: Option<Arc<dyn SwarmBackend>>,
    pub(crate) explicit: bool,
    pub(crate) error: Option<String>,
}

/// Routes launches to a terminal backend, retaining the in-process registry as
/// the automatic-mode fallback and lifecycle owner of ordinary teammate tasks.
#[derive(Clone)]
pub(crate) struct PaneTeammateSpawner {
    registry: Arc<TaskRegistry>,
    team: Arc<TeamRegistry>,
    runtime: Arc<dyn RuntimeSpawner>,
    output: Arc<dyn OutputStream>,
    session_id: SessionId,
    private_dir: PathBuf,
    backend: Option<Arc<dyn SwarmBackend>>,
    explicit: bool,
    backend_error: Option<String>,
    fallback_warned: Arc<AtomicBool>,
    backend_selector: Option<Arc<dyn Fn() -> PaneBackendSelection + Send + Sync>>,
    executable: PathBuf,
    tasks: Arc<Mutex<HashMap<String, PaneTask>>>,
}

impl PaneTeammateSpawner {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        registry: Arc<TaskRegistry>,
        team: Arc<TeamRegistry>,
        runtime: Arc<dyn RuntimeSpawner>,
        output: Arc<dyn OutputStream>,
        session_id: SessionId,
        private_dir: PathBuf,
        backend: Option<Arc<dyn SwarmBackend>>,
        explicit: bool,
        worker_executable: PathBuf,
    ) -> Self {
        Self {
            registry,
            team,
            runtime,
            output,
            session_id,
            private_dir,
            backend,
            explicit,
            backend_error: None,
            fallback_warned: Arc::new(AtomicBool::new(false)),
            backend_selector: None,
            executable: worker_executable,
            tasks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub(crate) fn with_backend_selector(
        mut self,
        selector: Arc<dyn Fn() -> PaneBackendSelection + Send + Sync>,
    ) -> Self {
        self.backend_selector = Some(selector);
        self
    }

    async fn write(
        writer: &Mutex<OwnedWriteHalf>,
        value: ParentToWorker,
    ) -> Result<(), TeamSpawnError> {
        let mut bytes = serde_json::to_vec(&value).map_err(internal)?;
        bytes.push(b'\n');
        // Include mutex acquisition: another blocked control write must not
        // prevent Stop from reaching backend termination.
        tokio::time::timeout(CONTROL_WRITE_TIMEOUT, async {
            writer.lock().await.write_all(&bytes).await
        })
        .await
        .map_err(|_| internal("Teammate control write timed out"))?
        .map_err(internal)
    }

    async fn cleanup(&self, task_id: &str) {
        let task = self.tasks.lock().await.remove(task_id);
        if let Some(task) = task {
            match tokio::time::timeout(BACKEND_CLEANUP_TIMEOUT, task.backend.kill_pane(&task.pane))
                .await
            {
                Ok(Ok(())) => {}
                Ok(Err(error)) => tracing::warn!("Teammate pane termination failed: {error}"),
                Err(_) => tracing::warn!("Teammate pane termination timed out"),
            }
            // Private launch material is removed even when the backend fails.
            let _ = tokio::fs::remove_dir_all(&task.directory).await;
        }
    }

    async fn launch_owned(
        &self,
        agent_id: AgentId,
        name: String,
        team_name: String,
        request: SubagentSpawnRequest,
    ) -> Result<String, TeamSpawnError> {
        // Keep the bounded handshake alive if its calling tool is interrupted:
        // it must still reap the pane and private files on every failure.
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let owner = self.clone();
        self.runtime
            .spawn(
                "pane-teammate-launch",
                Box::pin(async move {
                    let result = owner.launch_pane(agent_id, name, team_name, request).await;
                    if let Err(Ok(task_id)) = sender.send(result) {
                        let _ = TeamSpawnSeam::kill(&owner, &task_id).await;
                    }
                }),
            )
            .await
            .map_err(internal)?;
        receiver.await.map_err(internal)?
    }

    async fn launch_pane(
        &self,
        agent_id: AgentId,
        name: String,
        team_name: String,
        request: SubagentSpawnRequest,
    ) -> Result<String, TeamSpawnError> {
        let backend = self
            .backend
            .as_ref()
            .ok_or_else(|| internal("No terminal backend available"))?;
        if !self.executable.is_file() {
            return Err(internal(
                "LingXi CLI executable is unavailable for a terminal teammate",
            ));
        }
        // Each directory has a random identity and is created owner-only before
        // the manifest or socket exists. An existing path is never reused.
        let directory = self
            .private_dir
            .join(format!("lingxi-tm-{}", AgentId::new()));
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .map_err(internal)?;
        let result = async {
            let socket_path = directory.join("ipc");
            let listener = UnixListener::bind(&socket_path).map_err(internal)?;
            std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600)).map_err(internal)?;
            let token = format!("{}{}", AgentId::new(), AgentId::new());
            let manifest_path = directory.join("launch.json");
            let manifest = PaneTeammateManifest { socket_path, token: token.clone(), agent_id,
                name: name.clone(), team_name: team_name.clone(), parent_session_id: self.session_id,
                request: request.clone() };
            {
                use std::io::Write;
                let mut file = std::fs::OpenOptions::new().write(true).create_new(true)
                    .mode(0o600).open(&manifest_path).map_err(internal)?;
                file.write_all(&serde_json::to_vec(&manifest).map_err(internal)?).map_err(internal)?;
            }
            let pane = backend.create_teammate_pane(&agent_id, PanePosition::Right).await.map_err(internal)?;
            let launched = async {
                let metadata = backend.pane_metadata(&pane).await.map_err(internal)?;
                let mut command = format!("{} --teammate-launch-file {}", shell_quote(&self.executable), shell_quote(&manifest_path));
                let color = request.teammate_color.as_deref().unwrap_or("blue");
                for (flag, value) in [
                    ("--agent-id", format!("{name}@{team_name}")),
                    ("--agent-name", name.clone()),
                    ("--team-name", team_name.clone()),
                    ("--agent-color", color.to_owned()),
                    ("--parent-session-id", self.session_id.to_string()),
                    ("--agent-type", request.subagent_type.clone()),
                ] {
                    command.push(' '); command.push_str(flag); command.push(' ');
                    command.push_str(&shell_quote(Path::new(&value)));
                }
                if request.mode.as_deref() == Some("plan") { command.push_str(" --plan-mode-required"); }
                if let Some(mode) = request.mode.as_deref() {
                    command.push_str(" --permission-mode ");
                    command.push_str(&shell_quote(Path::new(mode)));
                }
                backend.send_command_to_pane(&pane, &command).await.map_err(internal)?;
                let handshake = async {
                    let (stream, _) = listener.accept().await.map_err(internal)?;
                    let (read, write) = stream.into_split();
                    let mut reader = WorkerReader::new(BufReader::new(read));
                    let early = authenticate_until_ready(&mut reader, &token).await?;
                    Ok((reader, write, early))
                };
                let (mut reader, write, mut early) = tokio::time::timeout(Duration::from_secs(30), handshake)
                    .await.map_err(|_| internal("Teammate did not become ready within 30 seconds"))??;
                let task_id = self.registry.create(TaskType::InProcessTeammate,
                    TaskSpawnInput::InProcessTeammate { agent_id, name: name.clone(), team_name: team_name.clone(),
                        description: request.prompt.clone(), spawn_request: Some(request.clone()), inheritance: None },
                    request.description.clone().unwrap_or_default()).await.map_err(internal)?;
                let writer = Arc::new(Mutex::new(write));
                self.tasks.lock().await.insert(task_id.clone(), PaneTask {
                    writer: writer.clone(), pane: pane.clone(), directory: directory.clone(), metadata, backend: backend.clone(),
                });
                let _ = self.registry.set_status(&task_id, TaskStatus::Running).await;
                let owner = self.clone();
                let watched_id = task_id.clone();
                let result = self.runtime.spawn("pane-teammate-reader", Box::pin(async move {
                    let sink = CoordinatorStatusSink::new(owner.team.clone(), owner.output.clone());
                    sink.set_status(&watched_id, TaskStatus::Running).await;
                    let tool = SendMessageTool::new(owner.team.clone(), tool_ui::send_message::truncate_preview).with_spawn_seam(Arc::new(owner.clone()));
                    let mut ctx = ToolUseContext::model_seed(request.model.clone().unwrap_or_default());
                    ctx.agent_id = Some(agent_id); ctx.agent_name = Some(name);
                    ctx.team_name = Some(team_name); ctx.origin_session_id = Some(owner.session_id);
                    let mut ended = false;
                    // Keep this deadline across frames: continuously arriving
                    // output must not defer terminal-state cleanup forever.
                    let mut status_poll = tokio::time::interval(Duration::from_millis(200));
                    status_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                    loop {
                        tokio::select! {
                            frame = async {
                                if let Some(message) = early.pop_front() { Ok(Some(message)) }
                                else { reader.next_frame().await }
                            } => {
                                let message = match frame {
                                    Ok(Some(message)) => message,
                                    _ => break,
                                };
                                match message {
                                    WorkerToParent::Output { text } => {
                                        if let Ok(path) = owner.registry.output_manager.path_for(&watched_id) {
                                            let _ = owner.registry.output_manager.append(&path, &text).await;
                                        }
                                    }
                                    WorkerToParent::State { status, error, awaiting_plan_approval } => {
                                        let _ = owner.registry.set_awaiting_plan_approval(&watched_id, awaiting_plan_approval).await;
                                        sink.set_awaiting_plan_approval(&watched_id, awaiting_plan_approval).await;
                                        let status = match status.as_str() {
                                            "running" => TaskStatus::Running,
                                            "completed" => TaskStatus::Completed,
                                            "failed" => TaskStatus::Failed,
                                            "killed" => TaskStatus::Killed,
                                            "idle" => {
                                                sink.set_teammate_idle(&watched_id).await;
                                                continue;
                                            }
                                            _ => continue,
                                        };
                                        let _ = owner.registry.set_status(&watched_id, status).await;
                                        if let Some(error) = error { sink.set_failed(&watched_id, &error).await; }
                                        else { sink.set_status(&watched_id, status).await; }
                                        if status.is_terminal() { ended = true; break; }
                                    }
                                    WorkerToParent::CoordinatorMessage { message } => {
                                        if let Ok(mut message) = serde_json::from_value::<coordinator::mailbox::TeammateMessage>(message) {
                                            // Attribute the authenticated channel to its member,
                                            // never to the untrusted payload's supplied sender.
                                            message.from = coordinator::mailbox::MessageSender::Teammate(agent_id);
                                            message.from_name = ctx.agent_name.clone().unwrap_or_default();
                                            let _ = owner.team.mailbox_router.route(&owner.team.coordinator_id, message).await;
                                        }
                                    }
                                    WorkerToParent::SendMessage { id, input } => {
                                        let (tx, rx) = tokio::sync::mpsc::channel(16);
                                        drop(rx);
                                        let (result, is_error) = match tool.call(input, ctx.clone(), tx).await {
                                            Ok(result) => (result.data, result.is_error),
                                            Err(error) => (serde_json::json!({"error":error.to_string()}), true),
                                        };
                                        if Self::write(&writer, ParentToWorker::SendMessageResult { id, result, is_error }).await.is_err() { break; }
                                    }
                                    _ => break,
                                }
                            }
                            _ = status_poll.tick() => {
                                if owner.registry.get(&watched_id).await.is_none_or(|state| state.base().status.is_terminal()) {
                                    let _ = Self::write(&writer, ParentToWorker::Shutdown).await;
                                    ended = true;
                                    break;
                                }
                            }
                        }
                    }
                    if !ended {
                        let _ = owner.registry.set_status(&watched_id, TaskStatus::Failed).await;
                        sink.set_failed(&watched_id, "Teammate transport closed").await;
                    }
                    owner.cleanup(&watched_id).await;
                })).await;
                if let Err(error) = result {
                    let _ = self.registry.set_status(&task_id, TaskStatus::Failed).await;
                    self.cleanup(&task_id).await;
                    return Err(internal(error));
                }
                Ok(task_id)
            }.await;
            if launched.is_err() {
                let _ = tokio::time::timeout(BACKEND_CLEANUP_TIMEOUT, backend.kill_pane(&pane)).await;
            }
            launched
        }.await;
        if result.is_err() {
            let _ = tokio::fs::remove_dir_all(&directory).await;
        }
        result
    }
}

fn internal(error: impl std::fmt::Display) -> TeamSpawnError {
    TeamSpawnError::Internal(error.to_string())
}
fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

#[async_trait]
impl TeamSpawnSeam for PaneTeammateSpawner {
    async fn spawn_teammate(
        &self,
        agent_id: AgentId,
        name: String,
        team_name: String,
        description: String,
    ) -> Result<String, TeamSpawnError> {
        self.registry
            .spawn_teammate(agent_id, name, team_name, description)
            .await
    }
    async fn spawn_teammate_request(
        &self,
        agent_id: AgentId,
        name: String,
        team_name: String,
        request: SubagentSpawnRequest,
        inherit: SubagentInheritance,
    ) -> Result<String, TeamSpawnError> {
        if let Some(selector) = &self.backend_selector {
            let selection = selector();
            let mut selected = self.clone();
            selected.backend = selection.backend;
            selected.explicit = selection.explicit;
            selected.backend_error = selection.error;
            selected.backend_selector = None;
            return selected
                .spawn_teammate_request(agent_id, name, team_name, request, inherit)
                .await;
        }
        if let Some(error) = &self.backend_error {
            if self.explicit {
                return Err(internal(error));
            }
            tracing::warn!(
                "[handleSpawn] No pane backend available, falling back to in-process: {error}"
            );
            if !self.fallback_warned.swap(true, Ordering::SeqCst) {
                let hint = if std::env::var("TERM_PROGRAM").as_deref() == Ok("iTerm.app") {
                    "To force iTerm2 panes, set teammateMode: \"iterm2\" in settings and enable the iTerm2 Python API (Preferences > General > Magic)."
                } else {
                    "To use terminal panes, set teammateMode: \"tmux\" in settings."
                };
                self.output
                    .emit_system_notice(
                        &format!(
                            "Couldn't open a teammate pane — running in-process instead. {hint}"
                        ),
                        false,
                    )
                    .await;
            }
        } else if self.backend.is_some() {
            return self.launch_owned(agent_id, name, team_name, request).await;
        } else if self.explicit {
            return Err(internal("No terminal backend available"));
        }
        self.registry
            .spawn_teammate_request(agent_id, name, team_name, request, inherit)
            .await
    }
    async fn pane_metadata(&self, task_id: &str) -> Option<PaneLaunchMetadata> {
        self.tasks
            .lock()
            .await
            .get(task_id)
            .map(|task| task.metadata.clone())
    }
    async fn send_message(&self, task_id: &str, message: String) -> Result<(), TeamSpawnError> {
        let writer = self
            .tasks
            .lock()
            .await
            .get(task_id)
            .map(|task| task.writer.clone());
        if let Some(writer) = writer {
            Self::write(&writer, ParentToWorker::Message { text: message }).await
        } else {
            self.registry.send_message(task_id, message).await
        }
    }
    async fn apply_plan_approval(
        &self,
        task_id: &str,
        response: platform_api::teammate_plan::PlanApprovalResponse,
    ) -> Result<(), TeamSpawnError> {
        let writer = self
            .tasks
            .lock()
            .await
            .get(task_id)
            .map(|task| task.writer.clone());
        if let Some(writer) = writer {
            Self::write(&writer, ParentToWorker::PlanApprovalResponse { response }).await
        } else {
            self.registry.apply_plan_approval(task_id, response).await
        }
    }
    async fn kill(&self, task_id: &str) -> Result<(), TeamSpawnError> {
        let writer = self
            .tasks
            .lock()
            .await
            .get(task_id)
            .map(|task| task.writer.clone());
        if let Some(writer) = writer {
            // Mark terminal first so the independent reader also owns cleanup
            // if this caller is cancelled during the cooperative shutdown.
            let _ = self.registry.set_status(task_id, TaskStatus::Killed).await;
            let _ = Self::write(&writer, ParentToWorker::Shutdown).await;
            self.cleanup(task_id).await;
            CoordinatorStatusSink::new(self.team.clone(), self.output.clone())
                .set_status(task_id, TaskStatus::Killed)
                .await;
            Ok(())
        } else {
            self.registry.kill(task_id).await.map_err(internal)
        }
    }
    async fn is_alive(&self, task_id: &str) -> bool {
        TeamSpawnSeam::is_alive(self.registry.as_ref(), task_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use platform_api::swarm::{SwarmError, SwarmHandle, SwarmLayout};
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn transcript(frames: Vec<WorkerToParent>) -> Vec<u8> {
        let mut bytes = Vec::new();
        for frame in frames {
            bytes.extend(serde_json::to_vec(&frame).unwrap());
            bytes.push(b'\n');
        }
        bytes
    }

    #[tokio::test]
    async fn ready_handshake_preserves_early_rpc_and_output_order() {
        let bytes = transcript(vec![
            WorkerToParent::Hello {
                token: "secret".into(),
            },
            WorkerToParent::SendMessage {
                id: 7,
                input: serde_json::json!({"to":"main","message":"early"}),
            },
            WorkerToParent::Output {
                text: "first output".into(),
            },
            WorkerToParent::Ready {
                task_id: "child-id".into(),
            },
            WorkerToParent::State {
                status: "running".into(),
                error: None,
                awaiting_plan_approval: false,
            },
        ]);
        let mut reader = WorkerReader::new(BufReader::new(bytes.as_slice()));
        let mut early = authenticate_until_ready(&mut reader, "secret")
            .await
            .unwrap();
        assert!(
            matches!(early.pop_front(), Some(WorkerToParent::SendMessage { id:7, input }) if input["message"] == "early")
        );
        assert!(
            matches!(early.pop_front(), Some(WorkerToParent::Output { text }) if text == "first output")
        );
        assert!(early.is_empty());
        assert!(
            matches!(reader.next_frame().await.unwrap(), Some(WorkerToParent::State { status, .. }) if status == "running")
        );
    }

    #[tokio::test]
    async fn worker_reader_keeps_partial_frame_when_status_poll_cancels_read() {
        let (mut writer, stream) = tokio::io::duplex(1024);
        let mut reader = WorkerReader::new(BufReader::new(stream));
        writer
            .write_all(b"{\"type\":\"output\",\"text\":\"part")
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(10), reader.next_frame())
                .await
                .is_err()
        );
        writer.write_all(b"ial\"}\n").await.unwrap();
        assert!(
            matches!(reader.next_frame().await.unwrap(), Some(WorkerToParent::Output { text }) if text == "partial")
        );
    }

    #[tokio::test]
    async fn handshake_reports_early_failure_and_bounds_buffer() {
        let failure = transcript(vec![
            WorkerToParent::Hello {
                token: "secret".into(),
            },
            WorkerToParent::State {
                status: "failed".into(),
                error: Some("startup failure".into()),
                awaiting_plan_approval: false,
            },
        ]);
        let mut reader = WorkerReader::new(BufReader::new(failure.as_slice()));
        assert!(authenticate_until_ready(&mut reader, "secret")
            .await
            .unwrap_err()
            .to_string()
            .contains("Teammate terminated before Ready (failed): startup failure"));
        let mut frames = vec![WorkerToParent::Hello {
            token: "secret".into(),
        }];
        frames.extend((0..=MAX_EARLY_FRAMES).map(|_| WorkerToParent::Output {
            text: "early".into(),
        }));
        let bytes = transcript(frames);
        let mut reader = WorkerReader::new(BufReader::new(bytes.as_slice()));
        assert!(authenticate_until_ready(&mut reader, "secret")
            .await
            .unwrap_err()
            .to_string()
            .contains("pre-Ready message buffer"));
        let oversized = vec![b'x'; MAX_FRAME_BYTES + 1];
        let mut reader = WorkerReader::new(BufReader::new(oversized.as_slice()));
        assert!(reader
            .next_frame()
            .await
            .unwrap_err()
            .to_string()
            .contains("frame exceeds 1 MiB"));
    }

    struct QuietOutput;
    #[async_trait]
    impl OutputStream for QuietOutput {
        async fn emit_text(&self, _: &str) {}
        async fn emit_tool_call(&self, _: &protocol::ToolUseId, _: &str, _: &serde_json::Value) {}
        async fn emit_tool_result(
            &self,
            _: &protocol::ToolUseId,
            _: &str,
            _: &str,
            _: &serde_json::Value,
        ) {
        }
        async fn emit_end_turn(&self, _: &str, _: &platform_api::CostSnapshot) {}
    }

    struct FailingBackend {
        killed: AtomicUsize,
        root: PathBuf,
        invalid_auth: bool,
    }
    #[async_trait]
    impl SwarmBackend for FailingBackend {
        async fn start_swarm(&self, _: SwarmLayout) -> Result<SwarmHandle, SwarmError> {
            Err(SwarmError::Unsupported)
        }
        async fn create_teammate_pane(
            &self,
            _: &AgentId,
            _: PanePosition,
        ) -> Result<PaneId, SwarmError> {
            Ok(PaneId { raw: "%9".into() })
        }
        async fn send_command_to_pane(&self, _: &PaneId, command: &str) -> Result<(), SwarmError> {
            use std::os::unix::fs::PermissionsExt;
            let directory = std::fs::read_dir(&self.root)
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path();
            assert_eq!(
                std::fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
                0o700
            );
            let manifest = directory.join("launch.json");
            assert_eq!(
                std::fs::metadata(&manifest).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert!(!command.contains("private prompt"));
            assert!(command.contains("--teammate-launch-file"));
            if self.invalid_auth {
                let manifest: PaneTeammateManifest =
                    serde_json::from_slice(&std::fs::read(manifest).unwrap()).unwrap();
                let mut stream = tokio::net::UnixStream::connect(&manifest.socket_path)
                    .await
                    .unwrap();
                stream
                    .write_all(b"{\"type\":\"hello\",\"token\":\"wrong\"}\n")
                    .await
                    .unwrap();
                Ok(())
            } else {
                Err(SwarmError::Tmux("injected dispatch failure".into()))
            }
        }
        async fn pane_metadata(&self, pane: &PaneId) -> Result<PaneLaunchMetadata, SwarmError> {
            Ok(PaneLaunchMetadata {
                session_name: "current".into(),
                window_name: "current".into(),
                pane_id: pane.raw.clone(),
                backend_type: "tmux".into(),
            })
        }
        async fn kill_pane(&self, _: &PaneId) -> Result<(), SwarmError> {
            self.killed.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        async fn destroy_swarm(&self, _: SwarmHandle) -> Result<(), SwarmError> {
            Ok(())
        }
        fn is_available(&self) -> bool {
            true
        }
    }

    struct EarlyBackend {
        root: PathBuf,
        runtime: Arc<dyn RuntimeSpawner>,
        reply: Arc<Mutex<Option<ParentToWorker>>>,
    }
    #[async_trait]
    impl SwarmBackend for EarlyBackend {
        async fn start_swarm(&self, _: SwarmLayout) -> Result<SwarmHandle, SwarmError> {
            Err(SwarmError::Unsupported)
        }
        async fn create_teammate_pane(
            &self,
            _: &AgentId,
            _: PanePosition,
        ) -> Result<PaneId, SwarmError> {
            Ok(PaneId { raw: "%12".into() })
        }
        async fn pane_metadata(&self, pane: &PaneId) -> Result<PaneLaunchMetadata, SwarmError> {
            Ok(PaneLaunchMetadata {
                backend_type: "tmux".into(),
                session_name: "current".into(),
                window_name: "current".into(),
                pane_id: pane.raw.clone(),
            })
        }
        async fn send_command_to_pane(&self, _: &PaneId, _: &str) -> Result<(), SwarmError> {
            let directory = std::fs::read_dir(&self.root)
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path();
            let manifest: PaneTeammateManifest =
                serde_json::from_slice(&std::fs::read(directory.join("launch.json")).unwrap())
                    .unwrap();
            let mut stream = tokio::net::UnixStream::connect(manifest.socket_path)
                .await
                .unwrap();
            stream
                .write_all(&transcript(vec![
                    WorkerToParent::Hello {
                        token: manifest.token,
                    },
                    WorkerToParent::Output {
                        text: "before Ready\n".into(),
                    },
                    WorkerToParent::SendMessage {
                        id: 7,
                        input: serde_json::json!({"to":"main","message":"early RPC"}),
                    },
                    WorkerToParent::Ready {
                        task_id: "child-task".into(),
                    },
                ]))
                .await
                .unwrap();
            let reply = self.reply.clone();
            self.runtime
                .spawn(
                    "fake-pane-worker",
                    Box::pin(async move {
                        let mut lines = BufReader::new(stream).lines();
                        while let Ok(Some(line)) = lines.next_line().await {
                            let value: ParentToWorker = serde_json::from_str(&line).unwrap();
                            if matches!(value, ParentToWorker::Shutdown) {
                                break;
                            }
                            *reply.lock().await = Some(value);
                        }
                    }),
                )
                .await
                .unwrap();
            Ok(())
        }
        async fn kill_pane(&self, _: &PaneId) -> Result<(), SwarmError> {
            Ok(())
        }
        async fn destroy_swarm(&self, _: SwarmHandle) -> Result<(), SwarmError> {
            Ok(())
        }
        fn is_available(&self) -> bool {
            true
        }
    }

    #[tokio::test]
    async fn unix_worker_early_rpc_is_replied_to_after_ready() {
        let root = tempfile::Builder::new()
            .prefix("lxt-")
            .tempdir_in("/tmp")
            .unwrap();
        let spool = tempfile::tempdir().unwrap();
        let runtime = Arc::new(platform_posix::PosixRuntime::new());
        let fs = Arc::new(platform_posix::PosixFileSystem::new(
            spool.path().to_owned(),
        ));
        let registry = Arc::new(TaskRegistry::new(
            runtime.clone(),
            fs.clone(),
            Arc::new(tasks::output_manager::TaskOutputManager::new(
                spool.path().to_owned(),
                fs,
            )),
        ));
        let team = Arc::new(TeamRegistry::new(AgentId::new()));
        team.mailbox_router
            .register(
                team.coordinator_id,
                Arc::new(coordinator::mailbox::TeammateMailbox::new(
                    team.coordinator_id,
                )),
            )
            .await;
        let reply = Arc::new(Mutex::new(None));
        let backend = Arc::new(EarlyBackend {
            root: root.path().to_owned(),
            runtime: runtime.clone(),
            reply: reply.clone(),
        });
        let spawner = PaneTeammateSpawner::new(
            registry.clone(),
            team,
            runtime.clone(),
            Arc::new(QuietOutput),
            SessionId::new(),
            root.path().to_owned(),
            Some(backend),
            true,
            std::env::current_exe().unwrap(),
        );
        let task_id = spawner
            .launch_pane(
                AgentId::new(),
                "scout".into(),
                "session".into(),
                SubagentSpawnRequest::default(),
            )
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if reply.lock().await.is_some() {
                    break;
                }
                runtime.sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("early RPC must be replayed and receive a response");
        assert!(matches!(
            reply.lock().await.as_ref(),
            Some(ParentToWorker::SendMessageResult { id: 7, .. })
        ));
        let text = tokio::fs::read_to_string(registry.output_manager.path_for(&task_id).unwrap())
            .await
            .unwrap();
        assert!(text.contains("before Ready"));
        TeamSpawnSeam::kill(&spawner, &task_id).await.unwrap();
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn stop_reaps_pane_and_private_material_when_writer_lock_is_stuck() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("private-launch");
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join("launch.json"), "private token").unwrap();
        let spool = tempfile::tempdir().unwrap();
        let runtime = Arc::new(platform_posix::PosixRuntime::new());
        let fs = Arc::new(platform_posix::PosixFileSystem::new(
            spool.path().to_owned(),
        ));
        let registry = Arc::new(TaskRegistry::new(
            runtime.clone(),
            fs.clone(),
            Arc::new(tasks::output_manager::TaskOutputManager::new(
                spool.path().to_owned(),
                fs,
            )),
        ));
        let agent_id = AgentId::new();
        let task_id = registry
            .create(
                TaskType::InProcessTeammate,
                TaskSpawnInput::InProcessTeammate {
                    agent_id,
                    name: "scout".into(),
                    team_name: "session".into(),
                    description: "work".into(),
                    spawn_request: None,
                    inheritance: None,
                },
                "work".into(),
            )
            .await
            .unwrap();
        let backend = Arc::new(FailingBackend {
            killed: AtomicUsize::new(0),
            root: root.path().to_owned(),
            invalid_auth: false,
        });
        let spawner = PaneTeammateSpawner::new(
            registry.clone(),
            Arc::new(TeamRegistry::new(AgentId::new())),
            runtime,
            Arc::new(QuietOutput),
            SessionId::new(),
            root.path().to_owned(),
            Some(backend.clone()),
            true,
            std::env::current_exe().unwrap(),
        );
        let (stream, _peer) = tokio::net::UnixStream::pair().unwrap();
        let (_read, write) = stream.into_split();
        let writer = Arc::new(Mutex::new(write));
        let _held = writer.lock().await;
        spawner.tasks.lock().await.insert(
            task_id.clone(),
            PaneTask {
                writer: writer.clone(),
                pane: PaneId { raw: "%9".into() },
                directory: directory.clone(),
                metadata: PaneLaunchMetadata {
                    session_name: "current".into(),
                    window_name: "current".into(),
                    pane_id: "%9".into(),
                    backend_type: "tmux".into(),
                },
                backend: backend.clone(),
            },
        );
        tokio::time::timeout(
            CONTROL_WRITE_TIMEOUT + Duration::from_secs(2),
            TeamSpawnSeam::kill(&spawner, &task_id),
        )
        .await
        .expect("stop must not wait for the held writer lock")
        .unwrap();
        assert_eq!(backend.killed.load(Ordering::SeqCst), 1);
        assert!(!directory.exists());
        assert!(spawner.tasks.lock().await.is_empty());
        assert_eq!(
            registry.get(&task_id).await.unwrap().base().status,
            TaskStatus::Killed
        );
    }

    #[tokio::test]
    async fn control_write_deadline_includes_socket_backpressure() {
        let (stream, _peer_not_reading) = tokio::net::UnixStream::pair().unwrap();
        let (_read, write) = stream.into_split();
        let error = tokio::time::timeout(
            CONTROL_WRITE_TIMEOUT + Duration::from_secs(2),
            PaneTeammateSpawner::write(
                &Mutex::new(write),
                ParentToWorker::Message {
                    text: "x".repeat(16 * 1024 * 1024),
                },
            ),
        )
        .await
        .expect("socket backpressure must have a deadline")
        .unwrap_err();
        assert!(error.to_string().contains("control write timed out"));
    }

    #[tokio::test]
    async fn dispatch_or_authentication_failure_reaps_pane_and_private_launch_material() {
        for invalid_auth in [false, true] {
            let root = tempfile::Builder::new()
                .prefix("lxt-")
                .tempdir_in("/tmp")
                .unwrap();
            let spool = tempfile::tempdir().unwrap();
            let runtime = Arc::new(platform_posix::PosixRuntime::new());
            let fs = Arc::new(platform_posix::PosixFileSystem::new(
                spool.path().to_owned(),
            ));
            let registry = Arc::new(TaskRegistry::new(
                runtime.clone(),
                fs.clone(),
                Arc::new(tasks::output_manager::TaskOutputManager::new(
                    spool.path().to_owned(),
                    fs,
                )),
            ));
            let backend = Arc::new(FailingBackend {
                killed: AtomicUsize::new(0),
                root: root.path().to_owned(),
                invalid_auth,
            });
            let spawner = PaneTeammateSpawner::new(
                registry,
                Arc::new(TeamRegistry::new(AgentId::new())),
                runtime,
                Arc::new(QuietOutput),
                SessionId::new(),
                root.path().to_owned(),
                Some(backend.clone()),
                true,
                std::env::current_exe().unwrap(),
            );
            let error = spawner
                .launch_pane(
                    AgentId::new(),
                    "scout".into(),
                    "session".into(),
                    SubagentSpawnRequest {
                        prompt: "private prompt".into(),
                        ..Default::default()
                    },
                )
                .await
                .unwrap_err();
            assert!(error.to_string().contains(if invalid_auth {
                "authentication failed"
            } else {
                "injected dispatch failure"
            }));
            assert_eq!(backend.killed.load(Ordering::SeqCst), 1);
            assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
            assert!(spawner.tasks.lock().await.is_empty());
        }
    }
}
