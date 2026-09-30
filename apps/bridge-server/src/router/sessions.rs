use super::EngineCommandRouter;
use client::adapter::ClientEventSink;
use client::protocol::events::ClientEvent;
use client::protocol::events::ErrorKindDto;
use client::protocol::listings::SessionAgentSummaryDto;
use lingxi_core::host::task_registry::TaskListFilter;

pub(super) fn session_agent_activity(
    messages: &[client::protocol::message::MessageDto],
) -> Option<String> {
    messages
        .iter()
        .rev()
        .flat_map(|message| message.blocks.iter().rev())
        .find_map(|block| {
            let text = match block {
                client::protocol::message::MessageBlockDto::Text { text }
                | client::protocol::message::MessageBlockDto::Thinking { thinking: text, .. } => {
                    text
                }
                client::protocol::message::MessageBlockDto::ToolUse { tool, .. } => tool,
                client::protocol::message::MessageBlockDto::ToolResult { tool, .. } => tool,
                _ => return None,
            };
            let line = text.lines().find(|line| !line.trim().is_empty())?.trim();
            if line.is_empty()
                || matches!(
                    line,
                    "completed" | "cancelled" | "failed" | "idle" | "running"
                )
            {
                return None;
            }
            Some(line.chars().take(160).collect())
        })
}

pub(super) async fn read_session_agent_summary(
    agent_id: String,
    root: &std::path::Path,
    path: &std::path::Path,
    raw: &[u8],
) -> Option<SessionAgentSummaryDto> {
    let messages = harness_runtime::desktop::session_agents::lower_transcript(raw);
    let mut status = "unknown".to_string();
    let mut name = None;
    let mut agent_type = None;
    let mut model = None;
    let mut model_profile = None;
    for line in raw
        .split(|byte| *byte == b'\n')
        .rev()
        .filter(|line| !line.is_empty())
    {
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(line) else {
            continue;
        };
        name = value
            .get("agent_name")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .or(name);
        agent_type = value
            .get("agent_type")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .or(agent_type);
        model = value
            .get("model")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .or(model);
        model_profile = value
            .get("model_profile")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .or(model_profile);
        if let Some(value) = value.get("status").and_then(serde_json::Value::as_str) {
            status = match value {
                // The TRANSCRIPT's own rest marker (`agent_idle`, written by
                // `agent::transcript::record_terminal`) is an engine-internal
                // word. Translate it here rather than letting it reach a
                // client: on the wire a parked agent is `completed`, exactly
                // like claude-code's row. `harness-runtime::mobile` already does this at
                // its own read-back; the desktop path did not, so one agent
                // reached the panel as `idle` here and `completed` from the
                // live observer.
                "idle" => "completed".to_string(),
                "completed" | "failed" | "killed" | "cancelled" | "running" => value.to_string(),
                _ => "unknown".to_string(),
            };
            break;
        }
    }
    if let Some(parent) = path.parent() {
        let row_path = session::agent_rows::row_path(parent, &agent_id);
        let relative = row_path
            .strip_prefix(root)
            .ok()
            .map(std::path::Path::to_path_buf);
        let root = root.to_path_buf();
        let row = if let Some(relative) = relative {
            tokio::task::spawn_blocking(move || {
                lingxi_core::host::rooted_fs::read_to_string_limited(
                    &root,
                    &relative,
                    session::agent_rows::ROW_MAX_BYTES,
                )
                .ok()
                .and_then(|text| {
                    serde_json::from_str::<session::agent_rows::ParkedAgentRow>(&text).ok()
                })
            })
            .await
            .ok()
            .flatten()
        } else {
            None
        };
        if let Some(row) = row {
            name = name.or(row.request.name).or(row.request.description);
            agent_type = agent_type.or_else(|| {
                (!row.request.subagent_type.is_empty()).then_some(row.request.subagent_type)
            });
            model = model.or(row.request.model);
            model_profile = model_profile.or(row.request.model_profile);
            if status == "running" {
                // A row on disk means the agent is parked and resumable, which
                // on the wire is `completed` — not the footer-group word.
                status = "completed".to_string();
            }
        }
    }
    let agent_type = agent_type.unwrap_or_else(|| "unknown".to_string());
    let name = name
        .or_else(|| (agent_type != "unknown").then(|| agent_type.clone()))
        .unwrap_or_else(|| {
            agent_id
                .strip_prefix("agent:")
                .unwrap_or(&agent_id)
                .chars()
                .take(8)
                .collect()
        });
    let updated_at_ms = tokio::fs::symlink_metadata(path)
        .await
        .ok()
        .filter(|metadata| metadata.is_file())
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|duration| u64::try_from(duration.as_millis()).ok());
    Some(SessionAgentSummaryDto {
        agent_id,
        name,
        agent_type,
        model,
        model_profile,
        status,
        latest_activity: session_agent_activity(&messages),
        updated_at_ms,
    })
}

impl EngineCommandRouter {
    /// Enumerate the real persisted JSONL catalog. A missing/empty store is a
    /// successful empty listing; other loader failures are surfaced as a
    /// recoverable client error without leaking filesystem details.
    pub(super) async fn emit_session_list(&self, limit: usize, sink: &dyn ClientEventSink) {
        let Some(store) = self.session_store.as_ref() else {
            sink.emit(ClientEvent::SessionList {
                sessions: Vec::new(),
            })
            .await;
            return;
        };

        match session::jsonl::list_recent_sessions_with_diagnostics(
            &store.lingxi_home,
            &store.session_cwd,
            limit,
            store.fs.clone(),
        )
        .await
        {
            Ok(catalog) => {
                let sessions = catalog
                    .sessions
                    .iter()
                    .map(client::adapter::lowering::lower_session_metadata)
                    .collect();
                sink.emit(ClientEvent::SessionList { sessions }).await;
                if catalog.skipped_files > 0 {
                    tracing::warn!(
                        skipped_files = catalog.skipped_files,
                        "bridge-server: unreadable sessions skipped"
                    );
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Internal,
                        message: "Some unreadable sessions were skipped. Repair or remove damaged session files, then retry.".to_string(),
                    })
                    .await;
                }
            }
            Err(session::jsonl::LoaderError::EmptyDirectory) => {
                sink.emit(ClientEvent::SessionList {
                    sessions: Vec::new(),
                })
                .await;
            }
            Err(error) => {
                tracing::warn!(%error, "bridge-server: session catalog unavailable");
                sink.emit(ClientEvent::Error {
                    kind: ErrorKindDto::Internal,
                    message: "session catalog is unavailable. Repair or remove unreadable session files, then retry.".to_string(),
                })
                .await;
            }
        }
    }
    /// Emit the real session-agent roster. `main` is included for protocol
    /// parity with mobile; Electron filters it from the Agents section because
    /// the main conversation already occupies the central Stage.
    pub(super) async fn emit_session_agent_list(&self, sink: &dyn ClientEventSink) {
        let Some(store) = self.session_store.as_ref() else {
            sink.emit(ClientEvent::Error {
                kind: ErrorKindDto::Internal,
                message:
                    "session agent listing unavailable: persisted session store is unavailable"
                        .to_string(),
            })
            .await;
            return;
        };
        let session_id = self.handle.current_session_id().await;
        let status = if self.is_turn_active() {
            "running"
        } else {
            "idle"
        };
        let model_snapshot = self.handle.get_status_snapshot().await;
        let mut agents = vec![SessionAgentSummaryDto {
            agent_id: "main".to_string(),
            name: "Main agent".to_string(),
            agent_type: "main".to_string(),
            model: Some(model_snapshot.model),
            model_profile: model_snapshot.model_profile,
            status: status.to_string(),
            latest_activity: None,
            updated_at_ms: None,
        }];
        let dir = orchestrator::transcript_paths::subagents_dir(
            &store.lingxi_home,
            &store.session_cwd,
            &session_id.as_uuid().to_string(),
        );
        let mut unreadable = 0usize;
        match harness_runtime::desktop::session_agents::collect_transcript_paths(&dir).await {
            Ok(paths) => {
                for path in paths {
                    let Some(agent_id) =
                        harness_runtime::desktop::session_agents::agent_id_from_path(&path)
                    else {
                        continue;
                    };
                    let raw = match harness_runtime::desktop::session_agents::read_transcript(
                        &dir, &path,
                    )
                    .await
                    {
                        Ok(raw) => raw,
                        Err(error) => {
                            unreadable = unreadable.saturating_add(1);
                            tracing::warn!(%error, "bridge-server: session agent transcript unreadable");
                            continue;
                        }
                    };
                    if let Some(line) =
                        harness_runtime::desktop::session_agents::first_corrupt_transcript_line(
                            &raw,
                        )
                    {
                        unreadable = unreadable.saturating_add(1);
                        tracing::warn!(
                            line,
                            "bridge-server: corrupt session agent transcript skipped"
                        );
                        continue;
                    }
                    if let Some(summary) =
                        read_session_agent_summary(agent_id, &dir, &path, &raw).await
                    {
                        agents.push(summary);
                    }
                }
            }
            Err(error) => {
                unreadable = unreadable.saturating_add(1);
                tracing::warn!(%error, "bridge-server: session agent transcript directory unreadable");
            }
        }
        // Claude 2.1.269 background_tasks_changed is a process-scoped level:
        // restarting clears liveness; reconnecting to this process retains it.
        // Read live facts AFTER the disk walk so terminal pushes win over an
        // older running JSONL record. Listing never rewrites transcript bytes.
        let tasks = self.tasks.list(TaskListFilter::default()).await;
        let observed = self
            .session_agent_observer
            .as_ref()
            .map(|observer| observer.snapshot(&session_id.as_uuid().to_string()))
            .unwrap_or_default();
        // Allocation can precede its first JSONL append.
        for current in observed.values() {
            if !agents
                .iter()
                .any(|agent| agent.agent_id == current.agent_id)
            {
                agents.push(current.clone());
            }
        }
        for agent in &mut agents[1..] {
            let current = observed.get(&agent.agent_id);
            if let Some(current) = current {
                // Observer is sampled after the task read. This includes idle:
                // Completed can reach the observer before the task consumer
                // parks its row. The runner publishes resumed input before a
                // new provider query, so that edge also has an observation.
                *agent = current.clone();
                continue;
            }
            let task = tasks.as_ref().ok().and_then(|rows| {
                rows.iter().find(|row| {
                    row.task_type == "local_agent"
                        && row.owner_agent_id.as_deref() == Some(agent.agent_id.as_str())
                })
            });
            if let Some(task) = task {
                // Hosts without the observer still use the current registry,
                // never historical transcript status, for live workers.
                agent.status = if task.is_parked {
                    // Parked is `completed` on the wire, same as the two
                    // read-back paths above.
                    "completed"
                } else {
                    task.status.as_str()
                }
                .to_string();
            } else if matches!(agent.status.as_str(), "running" | "pending") {
                if tasks.is_ok() {
                    agent.status = "cancelled".to_string();
                    agent.latest_activity =
                        Some("Interrupted when the engine stopped.".to_string());
                } else {
                    agent.status = "unknown".to_string();
                    agent.latest_activity =
                        Some("Unable to verify whether this agent is still running.".to_string());
                }
            }
        }
        agents[1..].sort_by(|left, right| right.updated_at_ms.cmp(&left.updated_at_ms));
        sink.emit(ClientEvent::SessionAgentList {
            session_id: session_id.as_uuid().to_string(),
            agents,
        })
        .await;
        if unreadable > 0 {
            sink.emit(ClientEvent::Error {
                kind: ErrorKindDto::Internal,
                message: format!(
                    "{unreadable} unreadable or corrupt session agent transcript(s) were skipped"
                ),
            })
            .await;
        }
    }
    /// Read one agent's complete transcript with an append-only revision. The
    /// requested id is validated before path lookup and the result is dropped
    /// if the session changed while the filesystem read was in flight.
    pub(super) async fn emit_session_agent_transcript(
        &self,
        agent_id: String,
        sink: &dyn ClientEventSink,
    ) {
        let Some(store) = self.session_store.as_ref() else {
            sink.emit(ClientEvent::Error {
                kind: ErrorKindDto::Internal,
                message:
                    "session agent transcript unavailable: persisted session store is unavailable"
                        .to_string(),
            })
            .await;
            return;
        };
        let requested_session_id = self.handle.current_session_id().await;
        let (messages, revision) = if agent_id == "main" {
            let uuid = requested_session_id.as_uuid();
            match orchestrator::replay_session_state(
                &store.lingxi_home,
                &store.session_cwd,
                uuid,
                store.fs.clone(),
            )
            .await
            {
                Ok(replayed) => {
                    let path = orchestrator::transcript_paths::main_transcript_path(
                        &store.lingxi_home,
                        &store.session_cwd,
                        &uuid.to_string(),
                    );
                    let raw = match tokio::fs::read(path).await {
                        Ok(raw) => raw,
                        Err(error) => {
                            tracing::warn!(%error, "bridge-server: main transcript unreadable");
                            sink.emit(ClientEvent::Error {
                                kind: ErrorKindDto::Internal,
                                message:
                                    "load main agent transcript failed: transcript is unreadable"
                                        .to_string(),
                            })
                            .await;
                            return;
                        }
                    };
                    (
                        // Same transcript the resume path emits, so it must be
                        // lowered the same way — a `main` row whose spawn
                        // results were dropped here would re-open the very
                        // anchoring gap the resume path closes.
                        client::adapter::lowering::lower_transcript_with_tool_results(
                            &replayed.display_history,
                            &replayed.client_state_tool_results,
                        ),
                        harness_runtime::desktop::session_agents::transcript_revision(&raw),
                    )
                }
                Err(error) => {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Internal,
                        message: format!("load main agent transcript failed: {error}"),
                    })
                    .await;
                    return;
                }
            }
        } else {
            let Some(parsed) = lingxi_core::types::AgentId::parse_prefixed(&agent_id) else {
                sink.emit(ClientEvent::Error {
                    kind: ErrorKindDto::Protocol,
                    message: format!("malformed session agent id: {agent_id:?}"),
                })
                .await;
                return;
            };
            let dir = orchestrator::transcript_paths::subagents_dir(
                &store.lingxi_home,
                &store.session_cwd,
                &requested_session_id.as_uuid().to_string(),
            );
            let path = match harness_runtime::desktop::session_agents::find_transcript_path(
                &dir,
                &parsed.to_string(),
            )
            .await
            {
                Ok(path) => path,
                Err(error) => {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Internal,
                        message: format!("load session agent transcript failed: {error}"),
                    })
                    .await;
                    return;
                }
            };
            let raw = match path {
                Some(path) => {
                    match harness_runtime::desktop::session_agents::read_transcript(&dir, &path)
                        .await
                    {
                        Ok(raw) => raw,
                        Err(error) => {
                            tracing::warn!(%error, "bridge-server: session agent transcript unreadable");
                            sink.emit(ClientEvent::Error {
                                kind: ErrorKindDto::Internal,
                                message:
                                    "load session agent transcript failed: transcript is unreadable"
                                        .to_string(),
                            })
                            .await;
                            return;
                        }
                    }
                }
                None => {
                    // A persistent runner can be visible in the live roster
                    // before its first JSONL append reaches disk. Treat that
                    // narrow startup window as a quiet retry; surfacing a
                    // rejected command here makes the renderer's poll loop
                    // append the same "not found" row every tick. Historical
                    // ids still take the explicit error path below.
                    if self.has_live_session_agent_task(&agent_id).await {
                        tracing::debug!(
                            %agent_id,
                            "session agent transcript is not on disk yet; retrying"
                        );
                        return;
                    }
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Rejected,
                        message: "session agent transcript was not found".to_string(),
                    })
                    .await;
                    return;
                }
            };
            if let Some(line) =
                harness_runtime::desktop::session_agents::first_corrupt_transcript_line(&raw)
            {
                tracing::warn!(
                    line,
                    "bridge-server: corrupt session agent transcript rejected"
                );
                sink.emit(ClientEvent::Error {
                    kind: ErrorKindDto::Internal,
                    message: "load session agent transcript failed: transcript is corrupt"
                        .to_string(),
                })
                .await;
                return;
            }
            (
                harness_runtime::desktop::session_agents::lower_transcript(&raw),
                harness_runtime::desktop::session_agents::transcript_revision(&raw),
            )
        };
        if self.handle.current_session_id().await != requested_session_id {
            return;
        }
        sink.emit(ClientEvent::SessionAgentTranscript {
            session_id: requested_session_id.as_uuid().to_string(),
            agent_id,
            next_message_index: messages.len() as u64,
            messages,
            revision,
        })
        .await;
    }
    /// Whether a missing transcript belongs to a currently live background
    /// agent. The task registry is the authoritative bridge-side view during
    /// the short allocation→first-write window; a parked agent remains
    /// resumable even though its task status is terminal.
    pub(super) async fn has_live_session_agent_task(&self, agent_id: &str) -> bool {
        self.tasks
            .list(TaskListFilter::default())
            .await
            .ok()
            .is_some_and(|rows| {
                rows.into_iter().any(|row| {
                    row.task_type == "local_agent"
                        && row.owner_agent_id.as_deref() == Some(agent_id)
                        && (row.is_parked
                            || matches!(row.status.as_str(), "pending" | "running" | "paused"))
                })
            })
    }
}
