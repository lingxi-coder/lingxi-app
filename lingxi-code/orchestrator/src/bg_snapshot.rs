//! Conversation-snapshot writer for the 2.1.212 `/fork` (`vAd`) background
//! session copy.
//!
//! [`history_to_jsonl_lines`] converts a live in-memory conversation into the
//! `<uuid>.jsonl` transcript line shape the RESUME loader
//! ([`crate::resume::replay_session_state`] → `session::jsonl::load_session`)
//! reads back, so a backgrounded worker that resumes the copied session sees
//! the parent conversation.
//!
//! This is a self-contained, loader-compatible projection — NOT the full
//! byte-golden `ConversationOrchestrator::to_jsonl_message` envelope (which
//! needs live per-turn state: response usage, request-id, effort, api-error
//! flags). The resume loader reconstructs `ConversationMessage`s from the line
//! `type` + inner `message.{role,content}` + the `uuid`/`parentUuid` chain, all
//! of which this projection emits faithfully. The richer per-turn usage/error
//! metadata is irrelevant to a fresh COPY (there is no in-flight response to
//! attribute), so reproducing it here would only duplicate that method's logic.
//!
//! One per-turn field is NOT metadata-only: the assistant line's `model`. The
//! resume loader ([`crate::resume::state_from_messages`]) restores the session's
//! active model from the LAST assistant line's `message.model` (real transcripts
//! carry it — `conversation.rs`'s `to_jsonl_message` writes it), and
//! `seed_orchestrator_session` copies that into `session.model`. Emitting the
//! assistant lines WITHOUT `model` therefore makes a forked background session
//! silently resume on `DEFAULT_MODEL` (`claude-opus-4-8`), losing the parent's
//! active model (e.g. after `/model sonnet`, or a cross-provider model — whose
//! provider profile the resume path re-derives from the model id, so restoring
//! the id restores the routing too). `ConversationMessage::Assistant` carries no
//! per-line model, so the caller threads the parent's CURRENT session model in
//! and this writer stamps it onto every assistant line — the last one is what
//! the loader reads, so the copy resumes on the parent's model, not the default.
//!
//! The composition root (`apps/cli`) owns the actual write (it holds the
//! `session::jsonl::JsonlWriter` + resolved `projects/<sanitize(cwd)>/…` path);
//! this helper lives in `orchestrator` so the `protocol::ConversationMessage`
//! → `session::JsonlMessage` mapping stays next to `to_jsonl_message`.

use protocol::ConversationMessage;
use session::JsonlMessage;

/// ISO-8601 UTC timestamp with millisecond precision (`new Date().toISOString()`
/// shape) — matches [`ConversationOrchestrator::to_jsonl_message`]'s timestamp
/// format so the copied lines are indistinguishable from live-written ones.
fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Project `history` into resume-loader-compatible JSONL lines for a NEW
/// session `session_uuid` (the bare uuid, no `sess:` prefix), stamping `cwd`
/// and `version` on every line. Lines are `parentUuid`-chained in arrival
/// order (first line's `parentUuid` is `null`), mirroring the live append
/// chain the loader's branch-aware DAG walk expects.
///
/// `model` is the parent session's CURRENT active model, stamped onto every
/// assistant line's `message.model` (an empty string ⇒ omitted). The resume
/// loader restores `session.model` from the LAST assistant line's `model`
/// (see [`crate::resume::state_from_messages`]), so passing it here makes the
/// forked background session resume on the parent's model instead of the
/// `DEFAULT_MODEL` seed — matching how ordinary `--resume` preserves the model
/// from real (model-carrying) assistant lines.
#[must_use]
pub fn history_to_jsonl_lines(
    history: &[ConversationMessage],
    session_uuid: &str,
    cwd: &str,
    version: &str,
    model: &str,
) -> Vec<JsonlMessage> {
    let ts = now_iso();
    let mut lines = Vec::with_capacity(history.len());
    let mut parent_uuid: Option<String> = None;
    for msg in history {
        let (kind, inner) = match msg {
            ConversationMessage::User { content, .. } => {
                ("user", serde_json::json!({ "role": "user", "content": content }))
            }
            ConversationMessage::Assistant { content, .. } => {
                let mut inner = serde_json::json!({ "role": "assistant", "content": content });
                // Stamp the parent's active model so `state_from_messages`
                // restores it on resume (real assistant lines carry `model`;
                // `ConversationMessage::Assistant` has none to recover per-line,
                // so the caller threads the live session model in). An empty
                // model would resume as `<synthetic>`-style noise, so omit it.
                if !model.is_empty() {
                    inner["model"] = serde_json::Value::String(model.to_string());
                }
                ("assistant", inner)
            }
            ConversationMessage::System { content, .. } => (
                "system",
                serde_json::json!({ "role": "system", "content": content }),
            ),
        };
        // Bare 8-4-4-4-12 lowercase uuid (NOT the `msg:`-prefixed display form)
        // — the JSONL schema + `validate_uuid` regex require the raw uuid.
        let uuid = msg.id().as_uuid().to_string();
        let mut extra = serde_json::Map::new();
        if msg.is_meta() {
            extra.insert("isMeta".to_string(), serde_json::Value::Bool(true));
        }
        let line = JsonlMessage {
            message_type: kind.to_string(),
            uuid: uuid.clone(),
            parent_uuid: parent_uuid.take(),
            session_id: session_uuid.to_string(),
            timestamp: ts.clone(),
            cwd: cwd.to_string(),
            version: version.to_string(),
            message: inner,
            is_sidechain: false,
            user_type: Some("external".to_string()),
            git_branch: None,
            entrypoint: Some("cli".to_string()),
            slug: None,
            prompt_id: None,
            logical_parent_uuid: None,
            extra,
        };
        parent_uuid = Some(uuid);
        lines.push(line);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::{ContentBlock, ConversationMessage};

    fn user(text: &str) -> ConversationMessage {
        ConversationMessage::User {
            id: protocol::MessageId::new(),
            content: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
            is_meta: false,
        }
    }

    fn assistant(text: &str) -> ConversationMessage {
        ConversationMessage::Assistant {
            id: protocol::MessageId::new(),
            content: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
            stop_reason: Some("end_turn".to_string()),
        }
    }

    #[test]
    fn chains_parent_uuids_in_order() {
        let history = vec![user("hi"), assistant("hello"), user("more")];
        let lines = history_to_jsonl_lines(&history, "sess-uuid", "/tmp/proj", "9.9.9", "");
        assert_eq!(lines.len(), 3);
        // First line has no parent.
        assert_eq!(lines[0].parent_uuid, None);
        // Each subsequent line chains to the prior line's uuid.
        assert_eq!(lines[1].parent_uuid.as_deref(), Some(lines[0].uuid.as_str()));
        assert_eq!(lines[2].parent_uuid.as_deref(), Some(lines[1].uuid.as_str()));
        // Kinds + trailer fields.
        assert_eq!(lines[0].message_type, "user");
        assert_eq!(lines[1].message_type, "assistant");
        assert_eq!(lines[0].session_id, "sess-uuid");
        assert_eq!(lines[0].cwd, "/tmp/proj");
        assert_eq!(lines[0].version, "9.9.9");
        assert_eq!(lines[0].user_type.as_deref(), Some("external"));
    }

    #[test]
    fn empty_history_yields_no_lines() {
        assert!(history_to_jsonl_lines(&[], "s", "/c", "1", "").is_empty());
    }

    #[test]
    fn inner_message_carries_role_and_content() {
        let lines = history_to_jsonl_lines(&[user("hi")], "s", "/c", "1", "");
        assert_eq!(lines[0].message["role"], "user");
        assert!(lines[0].message["content"].is_array());
    }

    /// Regression (BGF-1): the parent's active model is stamped onto assistant
    /// lines so the fork copy resumes on THAT model, not `DEFAULT_MODEL`. With no
    /// `model` field the resume loader falls back to the launch default,
    /// silently downgrading a `/model sonnet` (or cross-provider) session.
    #[test]
    fn assistant_lines_carry_the_parent_model() {
        let history = vec![user("hi"), assistant("hello")];
        let lines = history_to_jsonl_lines(
            &history,
            "s",
            "/c",
            "1",
            "us.anthropic.claude-sonnet-4-5-20250929-v1:0",
        );
        // Assistant line carries the parent's model; user line does not.
        assert_eq!(
            lines[1].message["model"],
            "us.anthropic.claude-sonnet-4-5-20250929-v1:0"
        );
        assert!(lines[0].message.get("model").is_none());
    }

    /// The stamped model survives a full snapshot → resume round-trip:
    /// `state_from_messages` restores it as the resumed session's active model
    /// (the exact path a backgrounded `/fork` copy takes on boot), so a
    /// non-default parent model is NOT overwritten by `DEFAULT_MODEL`.
    #[test]
    fn resume_restores_forked_parent_model_not_default() {
        let model = "us.anthropic.claude-sonnet-4-5-20250929-v1:0";
        let history = vec![user("hi"), assistant("hello")];
        let lines = history_to_jsonl_lines(&history, "s", "/c", "1", model);
        let sid = uuid::Uuid::nil();
        let state = crate::resume::state_from_messages(sid, &lines);
        assert_eq!(state.model, model, "resume must restore the parent's model");
        assert_ne!(
            state.model,
            crate::config::DEFAULT_MODEL,
            "must NOT fall back to the launch default"
        );
    }

    /// An empty parent model (defensive) omits the field entirely — the resume
    /// loader then keeps its own `DEFAULT_MODEL` seed rather than resuming onto
    /// a `""`/`<synthetic>`-style id that would fail `resolve_in`.
    #[test]
    fn empty_model_omits_the_field() {
        let history = vec![assistant("hello")];
        let lines = history_to_jsonl_lines(&history, "s", "/c", "1", "");
        assert!(lines[0].message.get("model").is_none());
    }
}
