//! Resume entrypoint — load a prior session, validate the chain, replay its
//! messages into a fresh [`SessionState`], and continue the turn loop.
//!
//! Spec §3 M5-08 row + §4.x resume completeness checks.
//!
//! Plan adaptation: the M5-07 `load_session` surface takes
//! `(claude_home, cwd, session_id, fs)` (rather than the plan-doc's
//! `(session_id, cwd)`), so [`replay_session_state`] mirrors that signature.
//! The CLI/REPL callers already have a `claude_home: PathBuf` and an
//! `Arc<dyn FileSystem>` from M3-01 + M4-01, so threading them through is
//! cheap and avoids hard-coding `dirs::home_dir()` inside the orchestrator.

use crate::config::OrchestratorConfig;
use crate::conversation::{ConversationOrchestrator, NoStreamingApiClient, OrchestratorApiClient};
use crate::test_support::{HookExecutor, PermissionGate};
use lingxi_core::SessionState;
use lingxi_protocol::{ContentBlock, ConversationMessage, MessageId, SessionId};
use lingxi_session::jsonl::{load_session, JsonlMessage, JsonlWriter, LoaderError};
use lingxi_telemetry::tengu::session::{RESUME_COMPLETED, RESUME_STARTED};
use lingxi_tools::registry::ToolRegistry;
use lingxi_traits::{FileSystem, OutputStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;
use uuid::Uuid;

/// Errors raised by the resume path. Forwards loader errors verbatim.
#[derive(Debug, thiserror::Error)]
pub enum ResumeError {
    /// Bubbled up from [`load_session`].
    #[error(transparent)]
    Loader(#[from] LoaderError),
}

/// Result of [`replay_session_state`]: the rebuilt session, the UUID of the
/// last replayed message (`None` only if zero messages were replayed — i.e.
/// the loader returned an empty `Vec`, which is structurally rejected
/// upstream but kept as `Option` for type safety), and the raw replayed
/// messages (callers may want to inspect them, e.g. interop tests).
#[derive(Debug)]
pub struct ReplayedSession {
    /// Replayed session state with `history` populated from the JSONL.
    pub state: SessionState,
    /// UUID of the last replayed message — used to seed the orchestrator's
    /// `last_jsonl_uuid` so the next append chains via `parent_uuid`.
    pub last_message_uuid: Option<Uuid>,
    /// Raw replayed messages (file-order), for callers that need them.
    pub messages: Vec<JsonlMessage>,
}

/// Load + replay a session by UUID. Emits
/// [`RESUME_STARTED`] before the disk read and
/// [`RESUME_COMPLETED`] after a successful replay.
///
/// Errors: any [`LoaderError`] from `load_session` is wrapped in
/// [`ResumeError::Loader`].
pub async fn replay_session_state(
    claude_home: &Path,
    cwd: &str,
    session_id: Uuid,
    fs: Arc<dyn FileSystem>,
) -> Result<ReplayedSession, ResumeError> {
    let sid_str = session_id.to_string();
    tracing::info!(
        event = RESUME_STARTED,
        session_id = %sid_str,
    );
    let messages = load_session(claude_home, cwd, session_id, fs).await?;
    let (state, last_uuid) = build_state_from_jsonl(session_id, &messages);
    tracing::info!(
        event = RESUME_COMPLETED,
        session_id = %sid_str,
        message_count = messages.len() as u64,
    );
    Ok(ReplayedSession {
        state,
        last_message_uuid: last_uuid,
        messages,
    })
}

/// Convert the replayed JSONL into a fresh [`SessionState`] + the UUID of
/// the tail message. `type: "user" | "assistant"` lines are appended to
/// `history`; `type: "system"` / `compact_boundary` / sidechain entries
/// are reconstructed at runtime from settings + memory and are NOT
/// replayed.
fn build_state_from_jsonl(
    session_id: Uuid,
    messages: &[JsonlMessage],
) -> (SessionState, Option<Uuid>) {
    let mut state = SessionState::empty(
        SessionId::from_uuid(session_id),
        crate::config::DEFAULT_MODEL.to_string(),
    );
    let mut last_uuid: Option<Uuid> = None;
    for m in messages {
        let msg_uuid = Uuid::parse_str(&m.uuid).unwrap_or_else(|_| Uuid::nil());
        match m.message_type.as_str() {
            "user" => {
                let content_blocks = extract_content_blocks(&m.message);
                state.history.push(ConversationMessage::User {
                    id: MessageId::from_uuid(msg_uuid),
                    content: content_blocks,
                });
                last_uuid = Some(msg_uuid);
            }
            "assistant" => {
                let content_blocks = extract_content_blocks(&m.message);
                state.history.push(ConversationMessage::Assistant {
                    id: MessageId::from_uuid(msg_uuid),
                    content: content_blocks,
                    stop_reason: None,
                });
                last_uuid = Some(msg_uuid);
            }
            _ => {
                // system / compact_boundary / sidechain / agent-internal — skip
                // for history replay, but still advance the chain pointer so
                // the next append's parent_uuid is anchored to the last
                // *persisted* line in the file (matching claude-code's
                // chain semantics).
                if !m.uuid.is_empty() {
                    last_uuid = Some(msg_uuid);
                }
            }
        }
    }
    (state, last_uuid)
}

/// Best-effort extraction of `content` from a JSONL `message` payload.
///
/// claude-code stores `message.content` as either:
/// - a string (for simple text-only turns), or
/// - an array of content blocks.
///
/// We accept both: a string becomes a single `ContentBlock::Text`; an array
/// is deserialized as `Vec<ContentBlock>` directly. If neither shape
/// matches, the result is an empty `Vec` — the message is still appended
/// so the chain is preserved, but the inner content is empty.
fn extract_content_blocks(message: &serde_json::Value) -> Vec<ContentBlock> {
    let Some(content) = message.get("content") else {
        return Vec::new();
    };
    if let Some(s) = content.as_str() {
        return vec![ContentBlock::Text {
            text: s.to_string(),
        }];
    }
    if content.is_array() {
        if let Ok(blocks) = serde_json::from_value::<Vec<ContentBlock>>(content.clone()) {
            return blocks;
        }
    }
    Vec::new()
}

impl ConversationOrchestrator {
    /// Construct an orchestrator pre-populated with a replayed session.
    ///
    /// 1. Calls [`replay_session_state`] to load + validate the JSONL.
    /// 2. Constructs the orchestrator (via the existing
    ///    [`Self::new_with_streaming`]) and overrides its internal session +
    ///    `last_jsonl_uuid` with the replayed values.
    /// 3. Sets `config.resume_session_id = Some(session_id)` so downstream
    ///    code can introspect the resume status.
    ///
    /// The new appends emitted by the M5-07 writer chain off
    /// `last_jsonl_uuid` so the parent-uuid chain continues unbroken.
    #[allow(clippy::too_many_arguments)]
    pub async fn with_resume(
        mut config: OrchestratorConfig,
        session_id: Uuid,
        claude_home: PathBuf,
        cwd_str: String,
        fs: Arc<dyn FileSystem>,
        api: Arc<dyn OrchestratorApiClient>,
        tools: Arc<ToolRegistry>,
        hooks: Arc<HookExecutor>,
        perms: Arc<dyn PermissionGate>,
        output: Arc<dyn OutputStream>,
        memory: Arc<dyn crate::prompt::MemoryHierarchyProvider>,
        cwd: std::path::PathBuf,
        jsonl_writer: Option<Arc<JsonlWriter>>,
    ) -> Result<Self, ResumeError> {
        config.resume_session_id = Some(session_id);
        let replayed = replay_session_state(&claude_home, &cwd_str, session_id, fs).await?;

        let mut orch = Self::new_with_streaming(
            config,
            api,
            Arc::new(NoStreamingApiClient),
            tools,
            hooks,
            perms,
            output,
            memory,
            cwd,
        );
        // Override the auto-generated session + chain pointer with the
        // replayed values. Both fields are `pub(crate)` so this is allowed
        // from a sibling module in the same crate.
        orch.session = Arc::new(Mutex::new(replayed.state));
        orch.last_jsonl_uuid = Mutex::new(replayed.last_message_uuid.map(|u| u.to_string()));
        if let Some(writer) = jsonl_writer {
            orch.jsonl_writer = Some(writer);
        }
        Ok(orch)
    }
}
