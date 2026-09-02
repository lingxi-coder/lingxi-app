//! Session-scoped subagent observation and transcript helpers for Desktop.
//!
//! The renderer's Agents view is deliberately backed by the same structured
//! observer and append-only JSONL transcripts used by the engine.  Keeping the
//! lowering here means the desktop bridge never has to scrape progress text or
//! invent a second agent lifecycle.

use async_trait::async_trait;
use client_adapter::ClientEventSink;
use client_protocol::events::ClientEvent;
use client_protocol::listings::SessionAgentSummaryDto;
use protocol::ConversationMessage;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

fn complete_transcript_lines(raw: &[u8]) -> impl Iterator<Item = &[u8]> {
    raw.split_inclusive(|byte| *byte == b'\n')
        .filter(|line| line.ends_with(b"\n"))
        .map(|line| {
            let line = line.strip_suffix(b"\n").unwrap_or(line);
            line.strip_suffix(b"\r").unwrap_or(line)
        })
}

/// Count valid message records in a transcript prefix.  The raw count is a
/// monotonic watermark even when a visible compact summary is replaced, so a
/// revision-aware renderer cannot mistake a changed snapshot for a duplicate.
#[must_use]
pub fn transcript_revision(raw: &[u8]) -> u64 {
    complete_transcript_lines(raw)
        .filter(|line| !line.is_empty())
        .filter_map(|line| serde_json::from_slice::<serde_json::Value>(line).ok())
        .filter_map(|value| value.get("message").cloned())
        .filter_map(|message| serde_json::from_value::<ConversationMessage>(message).ok())
        .count() as u64
}

/// Only user-visible conversation records belong in the Agents transcript.
#[must_use]
pub fn conversation_is_visible(message: &ConversationMessage) -> bool {
    !matches!(
        message,
        ConversationMessage::User { is_meta: true, .. }
            | ConversationMessage::User {
                is_compact_summary: true,
                ..
            }
            | ConversationMessage::User {
                is_visible_in_transcript_only: true,
                ..
            }
    )
}

/// Parse one append-only agent transcript, dropping malformed lines and
/// engine-only lifecycle messages.  Truncated final lines are intentionally
/// ignored so a crash during a write does not make the agent unreadable.
#[must_use]
pub fn parse_transcript_messages(raw: &[u8]) -> Vec<ConversationMessage> {
    complete_transcript_lines(raw)
        .filter(|line| !line.is_empty())
        .filter_map(|line| serde_json::from_slice::<serde_json::Value>(line).ok())
        .filter_map(|value| value.get("message").cloned())
        .filter_map(|message| serde_json::from_value::<ConversationMessage>(message).ok())
        .filter(|message| {
            !matches!(
                message,
                ConversationMessage::System {
                    subtype: Some(subtype),
                    ..
                } if subtype.starts_with("agent_")
            )
        })
        .filter(conversation_is_visible)
        .collect()
}

/// Return the one-based line number of the first malformed COMPLETE record.
/// An unterminated final line is a recoverable interrupted append and is not
/// reported as corruption.
#[must_use]
pub fn first_corrupt_transcript_line(raw: &[u8]) -> Option<usize> {
    complete_transcript_lines(raw)
        .enumerate()
        .find_map(|(index, line)| {
            if line.is_empty() {
                return None;
            }
            let valid = serde_json::from_slice::<serde_json::Value>(line)
                .ok()
                .and_then(|value| value.get("message").cloned())
                .and_then(|message| serde_json::from_value::<ConversationMessage>(message).ok())
                .is_some();
            (!valid).then_some(index + 1)
        })
}

/// Extract an `agent:<uuid>` from a transcript file name.  This rejects
/// arbitrary files and path components before any file content is read.
#[must_use]
pub fn agent_id_from_path(path: &Path) -> Option<String> {
    path.file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix("agent-"))
        .and_then(|id| id.strip_suffix(".jsonl"))
        .and_then(protocol::AgentId::parse_prefixed)
        .map(|id| id.to_string())
}

/// Recursively find ordinary, non-symlink agent transcripts.  Workflow runs
/// are nested below `subagents/workflows/<run-id>` and therefore need the
/// recursive walk; symlinks are ignored rather than followed.
pub async fn collect_transcript_paths(root: &Path) -> std::io::Result<Vec<PathBuf>> {
    match tokio::fs::symlink_metadata(root).await {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "session agent transcript root must not be a symlink",
            ));
        }
        Ok(metadata) if !metadata.is_dir() => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "session agent transcript root is not a directory",
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    }
    let mut dirs = vec![root.to_path_buf()];
    let mut paths = Vec::new();
    while let Some(dir) = dirs.pop() {
        let mut entries = match tokio::fs::read_dir(&dir).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            let metadata = tokio::fs::symlink_metadata(&path).await?;
            let file_type = metadata.file_type();
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                dirs.push(path);
            } else if file_type.is_file() && agent_id_from_path(&path).is_some() {
                paths.push(path);
            }
        }
    }
    paths.sort();
    Ok(paths)
}

/// Securely read one transcript relative to its engine-owned root. The rooted
/// filesystem walks every component without following symlinks, closing the
/// check/open race that a later plain `tokio::fs::read(path)` would re-open.
pub async fn read_transcript(root: &Path, path: &Path) -> std::io::Result<Vec<u8>> {
    let relative = path.strip_prefix(root).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "session agent transcript escaped its root",
        )
    })?;
    let root = root.to_path_buf();
    let relative = relative.to_path_buf();
    tokio::task::spawn_blocking(move || {
        platform_api::rooted_fs::read_to_string(&root, &relative)
            .map(String::into_bytes)
            .map_err(|error| std::io::Error::other(error.to_string()))
    })
    .await
    .map_err(|error| std::io::Error::other(error.to_string()))?
}

/// Locate one validated transcript below the engine-owned root.
pub async fn find_transcript_path(root: &Path, agent_id: &str) -> std::io::Result<Option<PathBuf>> {
    Ok(collect_transcript_paths(root)
        .await?
        .into_iter()
        .find(|path| agent_id_from_path(path).as_deref() == Some(agent_id)))
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}

fn activity(message: &ConversationMessage) -> Option<String> {
    let blocks = match message {
        ConversationMessage::Assistant { content, .. }
        | ConversationMessage::User { content, .. } => Some(content.as_slice()),
        ConversationMessage::System { content, .. } => {
            return (!content.is_empty()).then(|| content.chars().take(160).collect());
        }
    }?;
    blocks.iter().find_map(|block| match block {
        protocol::ContentBlock::Text { text } if !text.is_empty() => {
            Some(text.chars().take(160).collect())
        }
        protocol::ContentBlock::ToolUse { name, .. } => Some(name.clone()),
        protocol::ContentBlock::ToolResult { content, .. } if !content.is_empty() => {
            Some(content.chars().take(160).collect())
        }
        _ => None,
    })
}

#[derive(Clone)]
struct BoundAgent {
    session_id: String,
    name: String,
    agent_type: String,
    model: String,
    model_profile: Option<String>,
    persistent: bool,
}

/// Desktop observer installed by the bridge composition root.  The observer
/// receives the actual typed messages emitted by `PoolSubagentSpawner`; no
/// demo rows or synthetic transcript text enter the client event stream.
pub struct DesktopSessionAgentObserver {
    event_sink: Arc<dyn ClientEventSink>,
    session_id: String,
    bound_agents: tokio::sync::Mutex<HashMap<String, BoundAgent>>,
    tool_indexes: tokio::sync::Mutex<HashMap<String, client_adapter::turn::ToolUseIndex>>,
    message_indexes: tokio::sync::Mutex<HashMap<String, u64>>,
}

impl DesktopSessionAgentObserver {
    #[must_use]
    pub fn new(event_sink: Arc<dyn ClientEventSink>, session_id: impl Into<String>) -> Self {
        Self {
            event_sink,
            session_id: session_id.into(),
            bound_agents: tokio::sync::Mutex::new(HashMap::new()),
            tool_indexes: tokio::sync::Mutex::new(HashMap::new()),
            message_indexes: tokio::sync::Mutex::new(HashMap::new()),
        }
    }

    fn allocated_session_id(&self) -> String {
        // Workflow spawns write under the actual parent session's
        // `subagents/workflows/<run>` directory. Prefer that id so nested
        // workers remain fenced to the transcript the bridge opened.
        if let Some(session_id) = agent::workflow_transcript_subdir_override()
            .and_then(|path| path.ancestors().nth(3).map(Path::to_path_buf))
            .and_then(|path| path.file_name().map(|name| name.to_owned()))
            .and_then(|name| name.to_str().map(str::to_owned))
        {
            return session_id;
        }
        self.session_id.clone()
    }
}

#[async_trait]
impl platform_api::subagent_spawn::SubagentSpawnObserver for DesktopSessionAgentObserver {
    async fn on_event(&self, event: platform_api::subagent_spawn::SubagentObservation) {
        use platform_api::subagent_spawn::SubagentObservation;
        match event {
            SubagentObservation::Allocated {
                agent_id,
                agent_type,
                name,
                model,
                model_profile,
                persistent,
                initial_message_index,
            } => {
                let session_id = self.allocated_session_id();
                let name = name.unwrap_or_else(|| agent_type.clone());
                self.bound_agents.lock().await.insert(
                    agent_id.to_string(),
                    BoundAgent {
                        session_id: session_id.clone(),
                        name: name.clone(),
                        agent_type: agent_type.clone(),
                        model: model.clone(),
                        model_profile: model_profile.clone(),
                        persistent,
                    },
                );
                self.message_indexes
                    .lock()
                    .await
                    .insert(agent_id.to_string(), initial_message_index);
                self.event_sink
                    .emit(ClientEvent::SessionAgentUpdated {
                        session_id,
                        agent: SessionAgentSummaryDto {
                            agent_id: agent_id.to_string(),
                            name,
                            agent_type,
                            model: Some(model),
                            model_profile,
                            status: "running".to_string(),
                            latest_activity: None,
                            updated_at_ms: Some(unix_time_ms()),
                        },
                    })
                    .await;
            }
            SubagentObservation::Message { agent_id, message } => {
                if !conversation_is_visible(&message) {
                    return;
                }
                let key = agent_id.to_string();
                let Some(bound) = self.bound_agents.lock().await.get(&key).cloned() else {
                    // An observer event from a stale/foreign runtime must not
                    // leak into this session's feed.
                    return;
                };
                let dto = {
                    let mut indexes = self.tool_indexes.lock().await;
                    let index = indexes.entry(key.clone()).or_default();
                    client_adapter::lowering::lower_conversation_message_with(&message, index)
                };
                let message_index = {
                    let mut indexes = self.message_indexes.lock().await;
                    let next = indexes.entry(key.clone()).or_default();
                    let current = *next;
                    *next = next.saturating_add(1);
                    current
                };
                self.event_sink
                    .emit(ClientEvent::SessionAgentMessage {
                        session_id: bound.session_id.clone(),
                        agent_id: key.clone(),
                        message_index,
                        message: dto,
                    })
                    .await;
                self.event_sink
                    .emit(ClientEvent::SessionAgentUpdated {
                        session_id: bound.session_id,
                        agent: SessionAgentSummaryDto {
                            agent_id: key,
                            name: bound.name,
                            agent_type: bound.agent_type,
                            model: Some(bound.model),
                            model_profile: bound.model_profile,
                            status: "running".to_string(),
                            latest_activity: activity(&message),
                            updated_at_ms: Some(unix_time_ms()),
                        },
                    })
                    .await;
            }
            SubagentObservation::Completed { agent_id, .. } => {
                let persistent = self
                    .bound_agents
                    .lock()
                    .await
                    .get(&agent_id.to_string())
                    .is_some_and(|bound| bound.persistent);
                if persistent {
                    self.emit_update(agent_id, "idle", None, false).await;
                } else {
                    self.emit_terminal(agent_id, "completed").await;
                }
            }
            SubagentObservation::Failed { agent_id, error } => {
                self.emit_terminal_with_activity(agent_id, "failed", Some(error))
                    .await;
            }
            SubagentObservation::Killed { agent_id } => {
                self.emit_terminal(agent_id, "killed").await
            }
            SubagentObservation::Progress { .. } | SubagentObservation::Retry { .. } => {}
        }
    }
}

impl DesktopSessionAgentObserver {
    async fn emit_terminal(&self, agent_id: protocol::AgentId, status: &str) {
        self.emit_terminal_with_activity(agent_id, status, None)
            .await;
    }

    async fn emit_terminal_with_activity(
        &self,
        agent_id: protocol::AgentId,
        status: &str,
        latest_activity: Option<String>,
    ) {
        self.emit_update(agent_id, status, latest_activity, true)
            .await;
    }

    async fn emit_update(
        &self,
        agent_id: protocol::AgentId,
        status: &str,
        latest_activity: Option<String>,
        clear_state: bool,
    ) {
        let key = agent_id.to_string();
        let Some(bound) = self.bound_agents.lock().await.get(&key).cloned() else {
            return;
        };
        self.event_sink
            .emit(ClientEvent::SessionAgentUpdated {
                session_id: bound.session_id,
                agent: SessionAgentSummaryDto {
                    agent_id: key.clone(),
                    name: bound.name,
                    agent_type: bound.agent_type,
                    model: Some(bound.model),
                    model_profile: bound.model_profile,
                    status: status.to_string(),
                    latest_activity,
                    updated_at_ms: Some(unix_time_ms()),
                },
            })
            .await;
        if clear_state {
            // Terminal rows remain in the client roster, but one-shot/failed/
            // killed children must not retain per-agent indexes forever.
            self.bound_agents.lock().await.remove(&key);
            self.tool_indexes.lock().await.remove(&key);
            self.message_indexes.lock().await.remove(&key);
        }
    }
}

/// Lower a complete transcript to the event's message DTO shape.
#[must_use]
pub fn lower_transcript(raw: &[u8]) -> Vec<client_protocol::message::MessageDto> {
    client_adapter::lowering::lower_transcript(&parse_transcript_messages(raw))
}

#[cfg(test)]
mod tests {
    use super::*;
    use platform_api::subagent_spawn::{SubagentObservation, SubagentSpawnObserver, SubagentUsage};

    async fn allocate(
        observer: &DesktopSessionAgentObserver,
        agent_id: protocol::AgentId,
        persistent: bool,
        initial_message_index: u64,
    ) {
        observer
            .on_event(SubagentObservation::Allocated {
                agent_id,
                agent_type: "researcher".to_string(),
                name: Some("Research".to_string()),
                model: "test-model".to_string(),
                model_profile: Some("test-profile".to_string()),
                persistent,
                initial_message_index,
            })
            .await;
    }

    fn completed(agent_id: protocol::AgentId) -> SubagentObservation {
        SubagentObservation::Completed {
            agent_id,
            content: serde_json::json!("done"),
            usage: SubagentUsage::default(),
            total_tool_use_count: 0,
            total_duration_ms: 0,
            assistant_message_count: 0,
            last_request_id: None,
        }
    }

    #[test]
    fn agent_id_parser_rejects_non_agent_files() {
        assert!(agent_id_from_path(Path::new("agent-nope.jsonl")).is_none());
        assert!(agent_id_from_path(Path::new(
            "agent-aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa.jsonl"
        ))
        .is_some());
        assert!(agent_id_from_path(Path::new(
            "agent-aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa.task.json"
        ))
        .is_none());
    }

    #[test]
    fn revision_counts_hidden_records_while_lowering_only_visible() {
        let visible = ConversationMessage::user(protocol::MessageId::new(), "hello".to_string());
        let hidden = ConversationMessage::User {
            id: protocol::MessageId::new(),
            content: vec![protocol::ContentBlock::Text {
                text: "summary".into(),
            }],
            is_meta: false,
            is_compact_summary: true,
            is_visible_in_transcript_only: false,
        };
        let raw = format!(
            "{}\n{}\n",
            serde_json::json!({"message": visible}),
            serde_json::json!({"message": hidden})
        );
        assert_eq!(transcript_revision(raw.as_bytes()), 2);
        assert_eq!(parse_transcript_messages(raw.as_bytes()).len(), 1);
    }

    #[test]
    fn transcript_validation_ignores_only_an_unterminated_tail() {
        let message = ConversationMessage::user(protocol::MessageId::new(), "hello".to_string());
        let valid = serde_json::json!({"message": message}).to_string();
        let raw = format!("{valid}\nnot-json\n");
        assert_eq!(first_corrupt_transcript_line(raw.as_bytes()), Some(2));
        assert_eq!(
            first_corrupt_transcript_line(format!("{valid}\nnot-json").as_bytes()),
            None,
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn transcript_walk_and_read_reject_symlink_escape() {
        use std::os::unix::fs::symlink;

        let root_parent = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root = root_parent.path().join("subagents");
        std::fs::create_dir(&root).unwrap();
        let agent_id = protocol::AgentId::new();
        let outside_file = outside.path().join(format!("agent-{agent_id}.jsonl"));
        std::fs::write(&outside_file, "secret").unwrap();
        symlink(&outside_file, root.join(format!("agent-{agent_id}.jsonl"))).unwrap();

        assert!(collect_transcript_paths(&root).await.unwrap().is_empty());
        assert!(
            read_transcript(&root, &root.join(format!("agent-{agent_id}.jsonl")),)
                .await
                .is_err()
        );

        let linked_root = root_parent.path().join("linked-subagents");
        symlink(outside.path(), &linked_root).unwrap();
        assert!(collect_transcript_paths(&linked_root).await.is_err());
    }

    #[tokio::test]
    async fn one_shot_completion_emits_completed_and_releases_all_observer_state() {
        let sink = client_adapter::MockSink::arc();
        let observer = DesktopSessionAgentObserver::new(sink.clone(), "session-a");
        let agent_id = protocol::AgentId::new();
        allocate(&observer, agent_id, false, 0).await;

        observer.on_event(completed(agent_id)).await;

        assert!(sink.events().await.iter().any(|event| matches!(
            event,
            ClientEvent::SessionAgentUpdated { agent, .. }
                if agent.agent_id == agent_id.to_string() && agent.status == "completed"
        )));
        assert!(observer.bound_agents.lock().await.is_empty());
        assert!(observer.tool_indexes.lock().await.is_empty());
        assert!(observer.message_indexes.lock().await.is_empty());
    }

    #[tokio::test]
    async fn persistent_completion_is_idle_and_keeps_restored_index_for_resume() {
        let sink = client_adapter::MockSink::arc();
        let observer = DesktopSessionAgentObserver::new(sink.clone(), "session-a");
        let agent_id = protocol::AgentId::new();
        allocate(&observer, agent_id, true, 4).await;

        observer.on_event(completed(agent_id)).await;
        assert!(sink.events().await.iter().any(|event| matches!(
            event,
            ClientEvent::SessionAgentUpdated { agent, .. }
                if agent.agent_id == agent_id.to_string() && agent.status == "idle"
        )));
        assert!(observer
            .bound_agents
            .lock()
            .await
            .contains_key(&agent_id.to_string()));
        assert_eq!(
            observer
                .message_indexes
                .lock()
                .await
                .get(&agent_id.to_string()),
            Some(&4)
        );

        observer
            .on_event(SubagentObservation::Message {
                agent_id,
                message: ConversationMessage::Assistant {
                    id: protocol::MessageId::new(),
                    content: vec![protocol::ContentBlock::Text {
                        text: "resumed output".to_string(),
                    }],
                    stop_reason: None,
                },
            })
            .await;
        assert!(sink.events().await.iter().any(|event| matches!(
            event,
            ClientEvent::SessionAgentMessage { agent_id: emitted, message_index: 4, .. }
                if emitted == &agent_id.to_string()
        )));
        assert!(sink.events().await.iter().any(|event| matches!(
            event,
            ClientEvent::SessionAgentUpdated { agent, .. }
                if agent.agent_id == agent_id.to_string() && agent.status == "running"
        )));

        observer
            .on_event(SubagentObservation::Killed { agent_id })
            .await;
        assert!(observer.bound_agents.lock().await.is_empty());
        assert!(observer.message_indexes.lock().await.is_empty());
    }
}
