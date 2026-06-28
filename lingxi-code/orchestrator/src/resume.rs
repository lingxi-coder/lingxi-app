//! Resume entrypoint — load a prior session, validate the chain, replay its
//! messages into a fresh [`SessionState`], and continue the turn loop.
//!
//! Spec §3 M5-08 row + §4.x resume completeness checks.
//!
//! Plan adaptation: the M5-07 `load_session` surface takes
//! `(lingxi_home, cwd, session_id, fs)` (rather than the plan-doc's
//! `(session_id, cwd)`), so [`replay_session_state`] mirrors that signature.
//! The CLI/REPL callers already have a `lingxi_home: PathBuf` and an
//! `Arc<dyn FileSystem>` from M3-01 + M4-01, so threading them through is
//! cheap and avoids hard-coding `dirs::home_dir()` inside the orchestrator.

use crate::config::OrchestratorConfig;
use crate::conversation::{ConversationOrchestrator, NoStreamingApiClient, OrchestratorApiClient};
use crate::test_support::{HookExecutor, PermissionGate};
use engine::SessionState;
use protocol::{ContentBlock, ConversationMessage, MessageId, SessionId};
use session::jsonl::{load_session, JsonlMessage, JsonlWriter, LoaderError};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use telemetry::tengu::session::RESUMED;
use tokio::sync::Mutex;
use tool_api::registry::ToolRegistry;
use traits::{FileSystem, OutputStream};
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

/// Load + replay a session by UUID. Emits a single [`RESUMED`]
/// (`tengu_session_resumed`) after a successful replay — matching claude's
/// single resume event (the started/completed pair is not in the binary).
///
/// Errors: any [`LoaderError`] from `load_session` is wrapped in
/// [`ResumeError::Loader`].
pub async fn replay_session_state(
    lingxi_home: &Path,
    cwd: &str,
    session_id: Uuid,
    fs: Arc<dyn FileSystem>,
) -> Result<ReplayedSession, ResumeError> {
    let sid_str = session_id.to_string();
    let messages = load_session(lingxi_home, cwd, session_id, fs).await?;
    let (state, last_uuid) = build_state_from_jsonl(session_id, &messages);
    // claude emits a SINGLE `tengu_session_resumed` on resume (no started/
    // completed pair — those names have 0 hits in the 2.1.195 binary).
    tracing::info!(
        event = RESUMED,
        session_id = %sid_str,
        message_count = messages.len() as u64,
    );
    Ok(ReplayedSession {
        state,
        last_message_uuid: last_uuid,
        messages,
    })
}

/// Rebuild a replayed [`SessionState`] from transcript messages ALREADY loaded
/// from disk (`session::jsonl::load_session` → `Vec<JsonlMessage>`).
///
/// (M5-13) The CLI's `--resume <uuid>` mount loads the transcript once (for the
/// existence check + the TUI scrollback seed) and must NOT re-read it to seed the
/// engine session: a second `load_session` is both wasteful and a TOCTOU window
/// against a concurrent delete. This thin public wrapper exposes the SAME
/// per-line mapping [`replay_session_state`] uses ([`build_state_from_jsonl`]) so
/// the caller seeds the orchestrator's session directly from the in-hand
/// messages. Returns only the [`SessionState`] (callers seeding an already-built
/// orchestrator through its public `session()` accessor cannot set the
/// `pub(crate)` `last_jsonl_uuid` chain pointer anyway; the desktop/CLI build
/// wires no JSONL writer, so that pointer is inert on this path).
#[must_use]
pub fn state_from_messages(session_id: Uuid, messages: &[JsonlMessage]) -> SessionState {
    build_state_from_jsonl(session_id, messages).0
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
                // Restore the `isMeta` outer-envelope flag (claude-code persists
                // it as a top-level field; we read it back from `extra`) so a
                // resumed Stop-hook-feedback message stays meta/hidden.
                let is_meta = m
                    .extra
                    .get("isMeta")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
                state.history.push(ConversationMessage::User {
                    id: MessageId::from_uuid(msg_uuid),
                    content: content_blocks,
                    is_meta,
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
        lingxi_home: PathBuf,
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
        let replayed = replay_session_state(&lingxi_home, &cwd_str, session_id, fs).await?;

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
