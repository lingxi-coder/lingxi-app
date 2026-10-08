//! CLI entry point for a real persistent teammate hosted in a terminal pane.
//! The parent authenticates a private Unix connection; the ordinary engine
//! owns credentials, tool permissions, hooks and the persistent task loop.
use crate::argv::Argv;

pub async fn run(argv: &Argv, mode: permission::PermissionMode) -> i32 {
    #[cfg(unix)]
    match unix::run(argv, mode).await {
        Ok(()) => crate::exit_codes::SUCCESS,
        Err(error) => {
            eprintln!("Teammate worker: {error}");
            crate::exit_codes::RUNTIME_ERROR
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (argv, mode);
        eprintln!("Terminal teammate workers require a Unix host.");
        crate::exit_codes::RUNTIME_ERROR
    }
}

#[cfg(unix)]
mod unix {
    use super::*;
    use async_trait::async_trait;
    use lingxi_core::host::team_spawn::TeamSpawnSeam;
    use lingxi_core::host::teammate_worker::{
        PaneMessageForwarder, PaneMessageResult, PaneTeammateManifest, ParentToWorker,
        WorkerToParent,
    };
    use permission::gate::{
        PermissionCheckContext, PermissionDecision, PermissionGate, PermissionOutcome,
    };
    use std::collections::HashMap;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    };
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    use tokio::net::unix::OwnedWriteHalf;
    use tokio::sync::{mpsc, oneshot, Mutex};

    type Replies = Arc<std::sync::Mutex<HashMap<u64, oneshot::Sender<PaneMessageResult>>>>;
    const WRITE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
    const RPC_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);
    const MAX_FRAME_BYTES: usize = 1024 * 1024;

    #[derive(Debug)]
    enum TaskCommand {
        Message(String),
        PlanApproval(lingxi_core::host::teammate_plan::PlanApprovalResponse),
    }

    async fn dispatch_task_command(
        seam: &dyn TeamSpawnSeam,
        task_id: &str,
        command: TaskCommand,
    ) -> Result<(), lingxi_core::host::team_spawn::TeamSpawnError> {
        match command {
            TaskCommand::Message(text) => seam.send_message(task_id, text).await,
            TaskCommand::PlanApproval(response) => {
                seam.apply_plan_approval(task_id, response).await
            }
        }
    }

    struct Connection {
        writer: Mutex<OwnedWriteHalf>,
        replies: Replies,
        next_id: AtomicU64,
        poisoned: AtomicBool,
        cancelled: tokio_util::sync::CancellationToken,
    }
    struct WriteAttempt<'a> {
        connection: &'a Connection,
        complete: bool,
    }
    impl Drop for WriteAttempt<'_> {
        fn drop(&mut self) {
            if !self.complete {
                self.connection.poisoned.store(true, Ordering::Release);
                self.connection.cancelled.cancel();
            }
        }
    }
    struct PendingReply<'a> {
        replies: &'a Replies,
        id: u64,
    }
    impl Drop for PendingReply<'_> {
        fn drop(&mut self) {
            self.replies
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .remove(&self.id);
        }
    }
    impl Connection {
        fn new(writer: OwnedWriteHalf, cancelled: tokio_util::sync::CancellationToken) -> Self {
            Self {
                writer: Mutex::new(writer),
                replies: Arc::new(std::sync::Mutex::new(HashMap::new())),
                next_id: AtomicU64::new(1),
                poisoned: AtomicBool::new(false),
                cancelled,
            }
        }
        async fn write(&self, frame: &WorkerToParent) -> Result<(), String> {
            if self.poisoned.load(Ordering::Acquire) || self.cancelled.is_cancelled() {
                return Err("Parent teammate connection closed".into());
            }
            let mut bytes = serde_json::to_vec(frame).map_err(|e| e.to_string())?;
            bytes.push(b'\n');
            if bytes.len() > MAX_FRAME_BYTES {
                return Err("Teammate frame exceeds 1 MiB".into());
            }
            let deadline = tokio::time::Instant::now() + WRITE_TIMEOUT;
            let mut writer = tokio::select! {
                biased;
                _ = self.cancelled.cancelled() => return Err("Parent teammate connection closed".into()),
                writer = tokio::time::timeout_at(deadline, self.writer.lock()) => writer.map_err(|_| "Parent teammate writer lock timed out".to_owned())?,
            };
            if self.poisoned.load(Ordering::Acquire) {
                return Err("Parent teammate connection closed".into());
            }
            // Dropping this future after a partial write closes the logical
            // connection; no subsequent frame may reuse its truncated JSON.
            let mut attempt = WriteAttempt {
                connection: self,
                complete: false,
            };
            let result = tokio::select! {
                biased;
                _ = self.cancelled.cancelled() => Err("Parent teammate connection closed".into()),
                result = tokio::time::timeout_at(deadline, writer.write_all(&bytes)) => match result {
                    Ok(Ok(())) => Ok(()),
                    Ok(Err(error)) => Err(error.to_string()),
                    Err(_) => Err("Parent teammate write timed out".into()),
                },
            };
            if result.is_ok() {
                attempt.complete = true;
            }
            result
        }
    }
    #[async_trait]
    impl PaneMessageForwarder for Connection {
        async fn send_message(
            &self,
            input: serde_json::Value,
        ) -> Result<PaneMessageResult, String> {
            let deadline = tokio::time::Instant::now() + RPC_TIMEOUT;
            let id = self.next_id.fetch_add(1, Ordering::Relaxed);
            let (tx, rx) = oneshot::channel();
            self.replies
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .insert(id, tx);
            let _pending = PendingReply {
                replies: &self.replies,
                id,
            };
            tokio::select! {
                biased;
                _ = self.cancelled.cancelled() => Err("Parent teammate connection closed".into()),
                result = tokio::time::timeout_at(deadline, async {
                    self.write(&WorkerToParent::SendMessage { id, input }).await?;
                    rx.await.map_err(|_| "Parent teammate connection closed".to_string())
                }) => result.map_err(|_| "Parent SendMessage timed out".to_owned())?,
            }
        }
    }

    /// The engine selects the injected terminal gate before constructing an
    /// adapter gate. Reaching the unused adapter sink is a wiring error.
    struct InjectedPermissionSink;
    #[async_trait]
    impl client::adapter::PermissionRequestSink for InjectedPermissionSink {
        async fn emit_request(&self, _: client::protocol::permission::PermissionRequest) {
            unreachable!("pane workers always inject a terminal permission gate");
        }
    }

    /// A nonblocking terminal reader avoids Tokio stdin's uninterruptible
    /// background read keeping the child process alive after parent shutdown.
    // Darwin's /dev/tty proxy cannot be registered with kqueue (EINVAL).
    // Poll the nonblocking descriptor with a timer instead; neither reads nor
    // cancellation require an uninterruptible blocking helper thread.
    struct TerminalReader {
        file: std::fs::File,
        retry: Option<std::pin::Pin<Box<tokio::time::Sleep>>>,
    }
    impl tokio::io::AsyncRead for TerminalReader {
        fn poll_read(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            if buf.remaining() == 0 {
                return std::task::Poll::Ready(Ok(()));
            }
            loop {
                if let Some(retry) = self.retry.as_mut() {
                    std::task::ready!(std::future::Future::poll(retry.as_mut(), cx));
                    self.retry = None;
                }
                match std::io::Read::read(&mut self.file, buf.initialize_unfilled()) {
                    Ok(read) => {
                        buf.advance(read);
                        return std::task::Poll::Ready(Ok(()));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        self.retry = Some(Box::pin(tokio::time::sleep(
                            std::time::Duration::from_millis(20),
                        )));
                    }
                    Err(error) => return std::task::Poll::Ready(Err(error)),
                }
            }
        }
    }

    struct PanePermissionGate {
        inner: permission::InteractivePromptingGate,
        output: Arc<Mutex<dyn tokio::io::AsyncWrite + Send + Unpin>>,
        prompt_lock: Mutex<()>,
        cancelled: tokio_util::sync::CancellationToken,
    }
    impl PanePermissionGate {
        fn terminal(cancelled: tokio_util::sync::CancellationToken) -> Result<Self, String> {
            let file = std::fs::OpenOptions::new()
                .read(true)
                .open("/dev/tty")
                .map_err(|e| {
                    format!("Unable to open teammate terminal for permission prompts: {e}")
                })?;
            let flags = rustix::fs::fcntl_getfl(&file).map_err(|e| e.to_string())?;
            rustix::fs::fcntl_setfl(&file, flags | rustix::fs::OFlags::NONBLOCK)
                .map_err(|e| e.to_string())?;
            let input = TerminalReader { file, retry: None };
            Ok(Self::new(
                Arc::new(Mutex::new(BufReader::new(input))),
                Arc::new(Mutex::new(tokio::io::stderr())),
                cancelled,
            ))
        }
        fn new(
            input: Arc<Mutex<dyn tokio::io::AsyncBufRead + Send + Unpin>>,
            output: Arc<Mutex<dyn tokio::io::AsyncWrite + Send + Unpin>>,
            cancelled: tokio_util::sync::CancellationToken,
        ) -> Self {
            Self {
                inner: permission::InteractivePromptingGate::new(input, output.clone()),
                output,
                prompt_lock: Mutex::new(()),
                cancelled,
            }
        }
    }
    #[async_trait]
    impl PermissionGate for PanePermissionGate {
        async fn check(&self, name: &str, input: &serde_json::Value) -> PermissionDecision {
            tokio::select! {
                biased;
                _ = self.cancelled.cancelled() => PermissionDecision::Deny { reason: "Teammate permission prompt cancelled".into() },
                decision = async {
                    let _prompt = self.prompt_lock.lock().await;
                    // Show the actual operation before asking; the shared REPL
                    // prompt itself displays only the tool name.
                    let details = format!("\n{}\n", serde_json::to_string_pretty(input).unwrap_or_default());
                    if let Err(error) = self.output.lock().await.write_all(details.as_bytes()).await {
                        return PermissionDecision::Deny { reason: format!("Permission prompt output failed: {error}") };
                    }
                    self.inner.check(name, input).await
                } => decision,
            }
        }
        async fn check_exit_plan_mode(
            &self,
            plan: &str,
            ctx: &PermissionCheckContext,
        ) -> PermissionOutcome {
            tokio::select! {
                biased;
                _ = self.cancelled.cancelled() => PermissionOutcome::Deny { reason: "Teammate permission prompt cancelled".into() },
                decision = self.inner.check_exit_plan_mode(plan, ctx) => decision,
            }
        }
    }

    fn load_manifest(argv: &Argv) -> Result<PaneTeammateManifest, String> {
        let path = argv
            .teammate_launch_file
            .as_ref()
            .ok_or("Missing teammate launch file")?;
        let metadata = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
        if !metadata.file_type().is_file() || metadata.permissions().mode() & 0o077 != 0 {
            return Err("Teammate launch file must be a private regular file".into());
        }
        let manifest: PaneTeammateManifest =
            serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        validate_identity(argv, &manifest)?;
        std::fs::remove_file(path).map_err(|e| e.to_string())?;
        Ok(manifest)
    }

    fn validate_identity(argv: &Argv, manifest: &PaneTeammateManifest) -> Result<(), String> {
        if manifest.token.is_empty() || manifest.name.is_empty() || manifest.team_name.is_empty() {
            return Err("Incomplete teammate launch identity".into());
        }
        if argv.agent_id.as_deref().is_some_and(|id| {
            lingxi_core::types::AgentId::parse_prefixed(id) != Some(manifest.agent_id)
                && id != format!("{}@{}", manifest.name, manifest.team_name)
        }) || argv
            .agent_name
            .as_deref()
            .is_some_and(|name| name != manifest.name)
            || argv
                .team_name
                .as_deref()
                .is_some_and(|name| name != manifest.team_name)
            || argv.parent_session_id.as_deref().is_some_and(|id| {
                lingxi_core::types::SessionId::parse_prefixed(id)
                    != Some(manifest.parent_session_id)
            })
            || argv
                .agent_color
                .as_deref()
                .is_some_and(|color| Some(color) != manifest.request.teammate_color.as_deref())
            || argv
                .agent_type
                .as_deref()
                .is_some_and(|kind| kind != manifest.request.subagent_type)
        {
            return Err("Teammate command identity does not match launch context".into());
        }
        Ok(())
    }

    pub async fn run(argv: &Argv, mode: permission::PermissionMode) -> Result<(), String> {
        run_with_config(argv, mode, None).await
    }

    // Production always passes None; only the ignored test harness supplies
    // an isolated engine config. No environment switch weakens CLI storage.
    async fn run_with_config(
        argv: &Argv,
        mode: permission::PermissionMode,
        config: Option<harness_runtime::desktop::DesktopConfig>,
    ) -> Result<(), String> {
        let manifest = load_manifest(argv)?;
        std::env::set_var("LINGXI_CODE_TEAMMATE_BACKEND", "split-pane");
        let socket = tokio::net::UnixStream::connect(&manifest.socket_path)
            .await
            .map_err(|e| e.to_string())?;
        let (read, write) = socket.into_split();
        let cancelled = tokio_util::sync::CancellationToken::new();
        let connection = Arc::new(Connection::new(write, cancelled.clone()));
        connection
            .write(&WorkerToParent::Hello {
                token: manifest.token.clone(),
            })
            .await?;
        let (input_tx, mut input_rx) = mpsc::channel(64);
        let replies = connection.replies.clone();
        let reader_cancel = cancelled.clone();
        // Replies are consumed independently of task message delivery: a busy
        // teammate may itself be awaiting a SendMessage reply from its parent.
        let reader = tokio::spawn(async move {
            let mut lines = BufReader::new(read).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let Ok(frame) = serde_json::from_str::<ParentToWorker>(&line) else {
                    break;
                };
                match frame {
                    ParentToWorker::SendMessageResult {
                        id,
                        result,
                        is_error,
                    } => {
                        if let Some(reply) = replies
                            .lock()
                            .unwrap_or_else(|error| error.into_inner())
                            .remove(&id)
                        {
                            let _ = reply.send(PaneMessageResult { result, is_error });
                        }
                    }
                    ParentToWorker::Message { text } => {
                        // Bound pending delivery; a full queue terminates the
                        // connection visibly instead of deadlocking RPC replies.
                        if input_tx.try_send(TaskCommand::Message(text)).is_err() {
                            break;
                        }
                    }
                    ParentToWorker::PlanApprovalResponse { response } => {
                        if input_tx
                            .try_send(TaskCommand::PlanApproval(response))
                            .is_err()
                        {
                            break;
                        }
                    }
                    ParentToWorker::Shutdown => break,
                }
            }
            replies
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .clear();
            reader_cancel.cancel();
        });

        let result = drive(
            argv,
            mode,
            manifest,
            connection.clone(),
            &mut input_rx,
            cancelled,
            config,
        )
        .await;
        if let Err(error) = &result {
            let _ = connection
                .write(&WorkerToParent::State {
                    status: "failed".into(),
                    error: Some(error.clone()),
                    awaiting_plan_approval: false,
                })
                .await;
        }
        reader.abort();
        result
    }

    async fn drive(
        argv: &Argv,
        mode: permission::PermissionMode,
        mut manifest: PaneTeammateManifest,
        connection: Arc<Connection>,
        input_rx: &mut mpsc::Receiver<TaskCommand>,
        cancelled: tokio_util::sync::CancellationToken,
        config: Option<harness_runtime::desktop::DesktopConfig>,
    ) -> Result<(), String> {
        let mut cfg = match config {
            Some(config) => config,
            None => match &manifest.request.cwd {
                Some(cwd) => crate::init::resolve_desktop_config_at(argv, mode, cwd.into()),
                None => crate::init::resolve_desktop_config(argv, mode),
            },
        };
        cfg.initial_teammate_team_name = Some(manifest.team_name.clone());
        cfg.injected_permission_gate =
            Some(Arc::new(PanePermissionGate::terminal(cancelled.clone())?));
        manifest
            .request
            .origin_session_id
            .get_or_insert(manifest.parent_session_id);
        if argv.plan_mode_required {
            manifest.request.mode = Some("plan".into());
        }
        if let Some(model) = &manifest.request.model {
            cfg.default_model = model.clone();
        }
        let output = Arc::new(harness_runtime::headless::output_adapter::SinkAdapter::new(
            Arc::new(harness_runtime::headless::output::PlainSink::new(
                crate::headless_host::process_stdout(),
                crate::headless_host::process_stderr(),
            )),
        ));
        let runtime =
            harness_runtime::desktop::build(cfg, output, Arc::new(InjectedPermissionSink))
                .await
                .map_err(|e| e.to_string())?;
        runtime
            .coordinator
            .set_team_name(Some(manifest.team_name.clone()))
            .await;
        runtime
            .coordinator
            .set_message_forwarder(connection.clone())
            .await;
        runtime
            .coordinator
            .register_worker(
                manifest.agent_id,
                manifest.request.subagent_type.clone(),
                manifest.name.clone(),
                String::new(),
            )
            .await
            .map_err(|e| e.to_string())?;
        if let Some(color) = manifest.request.teammate_color.as_deref() {
            runtime
                .coordinator
                .mailbox_router
                .set_color(&manifest.name, color)
                .await;
        }
        let description = manifest.request.description.clone().unwrap_or_default();
        let task_id = runtime
            .task_registry
            .spawn(
                tasks::id::TaskType::InProcessTeammate,
                tasks::task_trait::TaskSpawnInput::InProcessTeammate {
                    agent_id: manifest.agent_id,
                    name: manifest.name,
                    team_name: manifest.team_name,
                    description: manifest.request.prompt.clone(),
                    spawn_request: Some(manifest.request),
                    inheritance: None,
                },
                description,
            )
            .await
            .map_err(|e| e.to_string())?;
        runtime
            .coordinator
            .set_task_id(&manifest.agent_id, task_id.clone())
            .await;
        connection
            .write(&WorkerToParent::Ready {
                task_id: task_id.clone(),
                selection: lingxi_core::host::team_spawn::TeamSpawnSeam::resolved_model_selection(
                    runtime.task_registry.as_ref(),
                    &task_id,
                )
                .await
                .map_err(|error| error.to_string())?,
            })
            .await?;
        let mut tick = tokio::time::interval(std::time::Duration::from_millis(100));
        let mut spool: Option<tokio::fs::File> = None;
        let mut pending_output = Vec::new();
        let mut last_state = None;
        let mut terminal = false;
        let outcome = async {
            loop {
                tokio::select! {
                    _ = cancelled.cancelled() => break,
                    message = input_rx.recv() => {
                        let Some(command) = message else { break; };
                        tokio::select! {
                            _ = cancelled.cancelled() => break,
                            result = dispatch_task_command(runtime.task_registry.as_ref(), &task_id, command) => result.map_err(|e| e.to_string())?,
                        }
                    }
                    _ = tick.tick() => {
                        if let Some(mailbox) = runtime.coordinator.mailbox_router.get(&runtime.coordinator.coordinator_id).await {
                            for message in mailbox.drain() {
                                connection.write(&WorkerToParent::CoordinatorMessage { message: serde_json::to_value(message).map_err(|e| e.to_string())? }).await?;
                            }
                        }
                        let state = runtime.task_registry.get(&task_id).await.ok_or("Teammate task disappeared")?;
                        if spool.is_none() { spool = tokio::fs::File::open(&state.base().output_file).await.ok(); }
                        if let Some(file) = &mut spool {
                            drain_output(file, &connection, &mut pending_output).await?;
                        }
                        let worker = runtime.coordinator.find_by_agent_id(&manifest.agent_id).await;
                        let worker_status = worker.and_then(|w| serde_json::to_value(w.status).ok());
                        let status = reported_status(state.base().status, worker_status.as_ref());
                        let awaiting_plan_approval = matches!(&state, tasks::state::TaskState::InProcessTeammate(row) if row.awaiting_plan_approval);
                        let error = worker_status.as_ref().and_then(|v| v["Failed"]["error"].as_str()).map(str::to_owned);
                        if let Some(frame) = changed_state_frame(&mut last_state, status, error, awaiting_plan_approval) {
                            connection.write(&frame).await?;
                        }
                        if state.base().status.is_terminal() { terminal = true; break; }
                    }
                }
            }
            Ok::<(), String>(())
        }.await;
        if !terminal {
            let _ = runtime.task_registry.kill(&task_id).await;
            let _ = connection
                .write(&WorkerToParent::State {
                    status: "killed".into(),
                    error: None,
                    awaiting_plan_approval: false,
                })
                .await;
        }
        outcome
    }

    fn changed_state_frame(
        last: &mut Option<(String, bool)>,
        status: String,
        error: Option<String>,
        awaiting_plan_approval: bool,
    ) -> Option<WorkerToParent> {
        let current = (status.clone(), awaiting_plan_approval);
        if last.as_ref() == Some(&current) {
            return None;
        }
        *last = Some(current);
        Some(WorkerToParent::State {
            status,
            error,
            awaiting_plan_approval,
        })
    }

    fn reported_status(
        task: tasks::state::TaskStatus,
        worker: Option<&serde_json::Value>,
    ) -> String {
        if !task.is_terminal()
            && worker
                .and_then(serde_json::Value::as_str)
                .is_some_and(|status| matches!(status, "Idle" | "AwaitingMessage"))
        {
            "idle".into()
        } else {
            format!("{task:?}").to_lowercase()
        }
    }

    async fn drain_output(
        file: &mut tokio::fs::File,
        connection: &Connection,
        pending: &mut Vec<u8>,
    ) -> Result<(), String> {
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let read = file.read(&mut buffer).await.map_err(|e| e.to_string())?;
            if read == 0 {
                return Ok(());
            }
            pending.extend_from_slice(&buffer[..read]);
            let valid = match std::str::from_utf8(pending) {
                Ok(_) => pending.len(),
                Err(error) if error.error_len().is_none() => error.valid_up_to(),
                Err(error) => return Err(error.to_string()),
            };
            if valid != 0 {
                let text = std::str::from_utf8(&pending[..valid])
                    .map_err(|e| e.to_string())?
                    .to_owned();
                print!("{text}");
                connection.write(&WorkerToParent::Output { text }).await?;
                pending.drain(..valid);
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// Exercises the production socket/task/permission loop with only the
        /// engine's existing isolated credential store substituted. The smoke
        /// driver supplies a private manifest, controlling PTY and mock API.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        #[ignore = "requires the external pane smoke driver and its private manifest"]
        async fn offline_worker_engine_smoke() {
            let path =
                std::env::var_os("LINGXI_PANE_SMOKE_MANIFEST").expect("smoke driver manifest");
            let argv = Argv::from_iter([
                std::ffi::OsString::from("lingxi"),
                std::ffi::OsString::from("--teammate-launch-file"),
                path,
            ])
            .unwrap();
            let launch: PaneTeammateManifest = serde_json::from_slice(
                &std::fs::read(argv.teammate_launch_file.as_ref().unwrap()).unwrap(),
            )
            .unwrap();
            let mut config = match launch.request.cwd {
                Some(cwd) => crate::init::resolve_desktop_config_at(
                    &argv,
                    permission::PermissionMode::Default,
                    cwd.into(),
                ),
                None => {
                    crate::init::resolve_desktop_config(&argv, permission::PermissionMode::Default)
                }
            };
            config.isolated_credential_storage = true;
            run_with_config(&argv, permission::PermissionMode::Default, Some(config))
                .await
                .unwrap();
        }

        #[test]
        fn approval_completion_clears_pending_ui_flag_even_while_status_stays_idle() {
            let mut last = None;
            let pending = changed_state_frame(&mut last, "idle".into(), None, true).unwrap();
            assert_eq!(
                serde_json::to_value(pending).unwrap()["awaiting_plan_approval"],
                true
            );
            assert!(changed_state_frame(&mut last, "idle".into(), None, true).is_none());
            let approved = changed_state_frame(&mut last, "idle".into(), None, false).unwrap();
            assert_eq!(
                serde_json::to_value(approved).unwrap()["awaiting_plan_approval"],
                false
            );
        }

        #[derive(Default)]
        struct PlanTransportSpy(Mutex<Vec<serde_json::Value>>);
        #[async_trait]
        impl TeamSpawnSeam for PlanTransportSpy {
            async fn spawn_teammate(
                &self,
                _: lingxi_core::types::AgentId,
                _: String,
                _: String,
                _: String,
            ) -> Result<String, lingxi_core::host::team_spawn::TeamSpawnError> {
                unreachable!()
            }
            async fn kill(
                &self,
                _: &str,
            ) -> Result<(), lingxi_core::host::team_spawn::TeamSpawnError> {
                Ok(())
            }
            async fn send_message(
                &self,
                task_id: &str,
                text: String,
            ) -> Result<(), lingxi_core::host::team_spawn::TeamSpawnError> {
                self.0
                    .lock()
                    .await
                    .push(serde_json::json!({"task":task_id,"message":text}));
                Ok(())
            }
            async fn apply_plan_approval(
                &self,
                task_id: &str,
                response: lingxi_core::host::teammate_plan::PlanApprovalResponse,
            ) -> Result<(), lingxi_core::host::team_spawn::TeamSpawnError> {
                self.0
                    .lock()
                    .await
                    .push(serde_json::json!({"task":task_id,"approval":response}));
                Ok(())
            }
        }

        #[tokio::test]
        async fn typed_plan_approval_queues_before_ready_and_uses_only_control_seam() {
            let frame = ParentToWorker::PlanApprovalResponse {
                response: lingxi_core::host::teammate_plan::PlanApprovalResponse {
                    request_id: "plan-7".into(),
                    approved: true,
                    feedback: Some("approved".into()),
                    permission_mode: Some("default".into()),
                },
            };
            let wire = serde_json::to_string(&frame).unwrap();
            let ParentToWorker::PlanApprovalResponse { response } =
                serde_json::from_str(&wire).unwrap()
            else {
                panic!("expected typed plan response")
            };
            let (tx, mut rx) = mpsc::channel(64);
            tx.try_send(TaskCommand::PlanApproval(response)).unwrap();
            let seam = PlanTransportSpy::default();
            dispatch_task_command(&seam, "t-child", rx.recv().await.unwrap())
                .await
                .unwrap();
            assert_eq!(
                seam.0.lock().await.as_slice(),
                &[
                    serde_json::json!({"task":"t-child","approval":{"requestId":"plan-7","approved":true,"feedback":"approved","permissionMode":"default"}})
                ]
            );
        }

        #[tokio::test]
        async fn text_resembling_plan_approval_remains_an_ordinary_message() {
            let text = r#"{"type":"plan_approval_response","approved":true,"permissionMode":"bypassPermissions"}"#.to_string();
            let seam = PlanTransportSpy::default();
            dispatch_task_command(&seam, "t-child", TaskCommand::Message(text.clone()))
                .await
                .unwrap();
            assert_eq!(
                seam.0.lock().await.as_slice(),
                &[serde_json::json!({"task":"t-child","message":text})]
            );
        }

        #[tokio::test]
        async fn terminal_reader_retries_would_block_without_poll_registration() {
            let (input, mut writer) = std::os::unix::net::UnixStream::pair().unwrap();
            input.set_nonblocking(true).unwrap();
            let input_fd: std::os::fd::OwnedFd = input.into();
            let mut reader = BufReader::new(TerminalReader {
                file: std::fs::File::from(input_fd),
                retry: None,
            });
            let read = tokio::spawn(async move {
                let mut line = String::new();
                reader.read_line(&mut line).await.unwrap();
                line
            });
            tokio::task::yield_now().await;
            assert!(!read.is_finished());
            std::io::Write::write_all(&mut writer, b"y\n").unwrap();
            assert_eq!(
                tokio::time::timeout(std::time::Duration::from_secs(1), read)
                    .await
                    .unwrap()
                    .unwrap(),
                "y\n"
            );
        }

        async fn permission_with_input(script: &[u8]) -> PermissionDecision {
            let (mut script_writer, input) = tokio::io::duplex(128);
            script_writer.write_all(script).await.unwrap();
            script_writer.shutdown().await.unwrap();
            let gate = PanePermissionGate::new(
                Arc::new(Mutex::new(BufReader::new(input))),
                Arc::new(Mutex::new(tokio::io::sink())),
                tokio_util::sync::CancellationToken::new(),
            );
            gate.check("Bash", &serde_json::json!({"command":"touch result.txt"}))
                .await
        }

        #[tokio::test]
        async fn terminal_permission_approval_releases_the_tool() {
            assert!(matches!(
                permission_with_input(b"y\n").await,
                PermissionDecision::Allow
            ));
        }

        #[tokio::test]
        async fn terminal_permission_denial_blocks_the_tool() {
            assert!(matches!(
                permission_with_input(b"n\n").await,
                PermissionDecision::Deny { .. }
            ));
        }

        #[tokio::test]
        async fn terminal_permission_eof_denies_instead_of_waiting_for_timeout() {
            let result = tokio::time::timeout(
                std::time::Duration::from_secs(1),
                permission_with_input(b""),
            )
            .await
            .unwrap();
            assert!(
                matches!(result, PermissionDecision::Deny { reason } if reason.contains("stdin closed"))
            );
        }

        #[tokio::test]
        async fn parent_disconnect_cancels_an_open_permission_prompt() {
            let (_keep_input_open, input) = tokio::io::duplex(128);
            let cancelled = tokio_util::sync::CancellationToken::new();
            let gate = PanePermissionGate::new(
                Arc::new(Mutex::new(BufReader::new(input))),
                Arc::new(Mutex::new(tokio::io::sink())),
                cancelled.clone(),
            );
            let pending = tokio::spawn(async move {
                gate.check("Bash", &serde_json::json!({"command":"touch result.txt"}))
                    .await
            });
            tokio::task::yield_now().await;
            cancelled.cancel();
            let result = tokio::time::timeout(std::time::Duration::from_secs(1), pending)
                .await
                .unwrap()
                .unwrap();
            assert!(
                matches!(result, PermissionDecision::Deny { reason } if reason.contains("cancelled"))
            );
        }
        #[test]
        fn idle_roster_state_is_reported_without_overriding_terminal_task_status() {
            let idle = serde_json::json!("Idle");
            assert_eq!(
                reported_status(tasks::state::TaskStatus::Running, Some(&idle)),
                "idle"
            );
            assert_eq!(
                reported_status(tasks::state::TaskStatus::Failed, Some(&idle)),
                "failed"
            );
            assert_eq!(
                reported_status(
                    tasks::state::TaskStatus::Running,
                    Some(&serde_json::json!({"Working":{"activity":"running"}}))
                ),
                "running"
            );
        }
        #[test]
        fn worker_flags_carry_identity_without_prompt() {
            let argv = Argv::from_iter([
                "lingxi",
                "--agent-id",
                "buddy@team",
                "--agent-name",
                "buddy",
                "--team-name",
                "team",
                "--agent-color",
                "blue",
                "--agent-type",
                "general-purpose",
                "--plan-mode-required",
                "--teammate-launch-file",
                "/tmp/private-launch.json",
            ])
            .unwrap();
            assert_eq!(argv.agent_name.as_deref(), Some("buddy"));
            assert!(argv.plan_mode_required);
            assert!(argv.teammate_launch_file.is_some());
        }

        #[tokio::test]
        async fn rpc_write_lock_wait_is_bounded_before_reply_timeout() {
            let (child, _parent_not_reading) = tokio::net::UnixStream::pair().unwrap();
            let (_, writer) = child.into_split();
            let connection = Connection::new(writer, tokio_util::sync::CancellationToken::new());
            let _held = connection.writer.lock().await;
            let error = tokio::time::timeout(
                WRITE_TIMEOUT + std::time::Duration::from_secs(1),
                connection.send_message(serde_json::json!({"to":"main","message":"hello"})),
            )
            .await
            .expect("write lock wait must not consume the 120s reply deadline")
            .unwrap_err();
            assert!(error.contains("writer lock timed out"));
            assert!(connection.replies.lock().unwrap().is_empty());
            assert!(!connection.poisoned.load(Ordering::Acquire));
        }

        #[tokio::test]
        async fn cancellation_interrupts_child_writer_lock_and_releases_pending_rpc() {
            let (child, _parent) = tokio::net::UnixStream::pair().unwrap();
            let (_, writer) = child.into_split();
            let cancellation = tokio_util::sync::CancellationToken::new();
            let connection = Arc::new(Connection::new(writer, cancellation.clone()));
            let _held = connection.writer.lock().await;
            let caller = connection.clone();
            let pending = tokio::spawn(async move {
                caller
                    .send_message(serde_json::json!({"to":"main","message":"hello"}))
                    .await
            });
            tokio::task::yield_now().await;
            cancellation.cancel();
            assert!(
                tokio::time::timeout(std::time::Duration::from_secs(1), pending)
                    .await
                    .unwrap()
                    .unwrap()
                    .is_err()
            );
            assert!(connection.replies.lock().unwrap().is_empty());
        }

        #[tokio::test]
        async fn child_partial_write_timeout_closes_transport_and_releases_rpc() {
            let (child, _parent_not_reading) = tokio::net::UnixStream::pair().unwrap();
            let (_, writer) = child.into_split();
            let connection = Connection::new(writer, tokio_util::sync::CancellationToken::new());
            let error = tokio::time::timeout(
                WRITE_TIMEOUT + std::time::Duration::from_secs(1),
                connection
                    .send_message(serde_json::json!({"to":"main","message":"x".repeat(900_000)})),
            )
            .await
            .unwrap()
            .unwrap_err();
            assert!(error.contains("write timed out"));
            assert!(connection.poisoned.load(Ordering::Acquire));
            assert!(connection.cancelled.is_cancelled());
            assert!(connection.replies.lock().unwrap().is_empty());
            assert!(connection
                .write(&WorkerToParent::Hello {
                    token: "never append another frame".into()
                })
                .await
                .is_err());
        }

        #[tokio::test]
        async fn aborted_partial_child_frame_poisoning_is_cancellation_safe() {
            let (child, mut parent) = tokio::net::UnixStream::pair().unwrap();
            let (_, writer) = child.into_split();
            let connection = Arc::new(Connection::new(
                writer,
                tokio_util::sync::CancellationToken::new(),
            ));
            let writing = connection.clone();
            let pending = tokio::spawn(async move {
                writing
                    .write(&WorkerToParent::Output {
                        text: "x".repeat(900_000),
                    })
                    .await
            });
            let mut bytes = [0_u8; 16];
            parent.read_exact(&mut bytes).await.unwrap();
            pending.abort();
            let _ = pending.await;
            assert!(connection.poisoned.load(Ordering::Acquire));
            assert!(connection.cancelled.is_cancelled());
        }

        #[tokio::test]
        async fn cancelling_a_waiting_rpc_removes_its_reply_without_poisoning_complete_frame() {
            let (child, parent) = tokio::net::UnixStream::pair().unwrap();
            let (_, writer) = child.into_split();
            let connection = Arc::new(Connection::new(
                writer,
                tokio_util::sync::CancellationToken::new(),
            ));
            let caller = connection.clone();
            let pending = tokio::spawn(async move {
                caller
                    .send_message(serde_json::json!({"to":"main","message":"hello"}))
                    .await
            });
            let mut lines = BufReader::new(parent).lines();
            assert!(lines.next_line().await.unwrap().is_some());
            pending.abort();
            let _ = pending.await;
            assert!(connection.replies.lock().unwrap().is_empty());
            assert!(!connection.poisoned.load(Ordering::Acquire));
        }

        #[tokio::test]
        async fn message_rpc_preserves_structured_result_and_error_flag() {
            let (child, parent) = tokio::net::UnixStream::pair().unwrap();
            let (_, writer) = child.into_split();
            let connection = Arc::new(Connection::new(
                writer,
                tokio_util::sync::CancellationToken::new(),
            ));
            let caller = connection.clone();
            let expected =
                serde_json::json!({"to":"peer", "message":"hello", "summary":"greeting"});
            let input = expected.clone();
            let pending = tokio::spawn(async move { caller.send_message(input).await });
            let mut lines = BufReader::new(parent).lines();
            let frame: WorkerToParent =
                serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            let WorkerToParent::SendMessage { id, input } = frame else {
                panic!("expected message frame")
            };
            assert_eq!(input, expected);
            connection
                .replies
                .lock()
                .unwrap()
                .remove(&id)
                .unwrap()
                .send(PaneMessageResult {
                    result: serde_json::json!({"success":false,"message":"recipient unavailable"}),
                    is_error: true,
                })
                .unwrap();
            let reply = pending.await.unwrap().unwrap();
            assert!(reply.is_error);
            assert_eq!(reply.result["message"], "recipient unavailable");
            assert!(connection.replies.lock().unwrap().is_empty());
        }

        #[test]
        fn manifest_rejects_command_identity_mismatch() {
            let argv = Argv::from_iter(["lingxi", "--agent-name", "impostor"]).unwrap();
            let manifest = PaneTeammateManifest {
                socket_path: "/tmp/socket".into(),
                token: "private-token".into(),
                agent_id: lingxi_core::types::AgentId::new(),
                name: "buddy".into(),
                team_name: "team".into(),
                parent_session_id: lingxi_core::types::SessionId::new(),
                request: Default::default(),
            };
            assert!(validate_identity(&argv, &manifest).is_err());
        }

        #[test]
        fn manifest_color_matches_assigned_command_color() {
            let matching = Argv::from_iter(["lingxi", "--agent-color", "blue"]).unwrap();
            let wrong = Argv::from_iter(["lingxi", "--agent-color", "cyan"]).unwrap();
            let manifest = PaneTeammateManifest {
                socket_path: "/tmp/socket".into(),
                token: "token".into(),
                agent_id: lingxi_core::types::AgentId::new(),
                name: "worker".into(),
                team_name: "session-12345678".into(),
                parent_session_id: lingxi_core::types::SessionId::new(),
                request: lingxi_core::host::SubagentSpawnRequest {
                    teammate_color: Some("blue".into()),
                    ..Default::default()
                },
            };
            assert!(validate_identity(&matching, &manifest).is_ok());
            assert!(validate_identity(&wrong, &manifest).is_err());
            let encoded = serde_json::to_vec(&manifest).unwrap();
            let decoded: PaneTeammateManifest = serde_json::from_slice(&encoded).unwrap();
            assert_eq!(decoded.request.teammate_color.as_deref(), Some("blue"));
        }

        #[tokio::test]
        async fn output_preserves_unicode_across_chunks_and_drains_terminal_tail() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("output");
            let expected = format!("{}消息{}", "x".repeat(65_535), "tail".repeat(20_000));
            tokio::fs::write(&path, &expected).await.unwrap();
            let (child, parent) = tokio::net::UnixStream::pair().unwrap();
            let (_, writer) = child.into_split();
            let connection = Connection::new(writer, tokio_util::sync::CancellationToken::new());
            let drain = tokio::spawn(async move {
                let mut file = tokio::fs::File::open(path).await.unwrap();
                let mut pending = Vec::new();
                drain_output(&mut file, &connection, &mut pending)
                    .await
                    .unwrap();
                assert!(pending.is_empty());
            });
            let mut actual = String::new();
            let mut lines = BufReader::new(parent).lines();
            while let Some(line) = lines.next_line().await.unwrap() {
                let WorkerToParent::Output { text } = serde_json::from_str(&line).unwrap() else {
                    panic!("expected output frame")
                };
                actual.push_str(&text);
            }
            drain.await.unwrap();
            assert_eq!(actual, expected);
        }
    }
}
