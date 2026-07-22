//! PLACE on a RUNNING orchestrator (the symmetric twin of `clear_session`).
//!
//! Verifies that resuming:
//! - replaces the live history with the supplied transcript (so the next turn
//!   sees prior context — observed via `conversation_transcript`),
//! - ADOPTS the named session id (resume does NOT mint a fresh one, unlike
//!   `clear_session`), and
//! - leaves the live model UNCHANGED (resume keeps the running model).
use orchestrator::test_support::{
    noop_hook_executor, MockApiClient, MockOutputStream, NoOpPermissionGate, StaticMemoryProvider,
};
use orchestrator::{ConversationOrchestrator, OrchestratorConfig};
use protocol::{ConversationMessage, MessageId, SessionId};
use std::sync::Arc;
use traits::OrchestratorHandle;

fn make_orch() -> Arc<ConversationOrchestrator> {
    let api = Arc::new(MockApiClient::new(vec![]));
    let tools = Arc::new(tool_api::registry::ToolRegistry::new());
    let hooks = noop_hook_executor();
    let perms = Arc::new(NoOpPermissionGate);
    let output = Arc::new(MockOutputStream::new());
    let memory = Arc::new(StaticMemoryProvider::empty());
    let orch = ConversationOrchestrator::new(
        OrchestratorConfig::default(),
        api,
        tools,
        hooks,
        perms,
        output,
        memory,
        std::env::temp_dir(),
    );
    Arc::new(orch)
}

#[tokio::test]
async fn resume_session_adopts_history_and_named_id_keeping_model() {
    let orch = make_orch();
    let handle: Arc<dyn OrchestratorHandle> = orch.clone();

    // The fresh orchestrator starts with its auto-minted id + default model and
    // an empty history.
    let original_id = handle.current_session_id().await;
    let original_model = handle.get_status_snapshot().await.model;
    assert!(
        handle.conversation_transcript().await.is_empty(),
        "a fresh orchestrator starts with no history"
    );

    // A named target session + a two-message replayed transcript.
    let named = SessionId::new();
    assert_ne!(
        named, original_id,
        "the named target must differ from the live id"
    );
    let history = vec![
        ConversationMessage::user(MessageId::new(), "prior user turn".to_string()),
        ConversationMessage::Assistant {
            id: MessageId::new(),
            content: vec![protocol::ContentBlock::Text {
                text: "prior assistant turn".to_string(),
            }],
            stop_reason: Some("end_turn".to_string()),
        },
    ];

    handle
        .resume_session(
            named,
            history.clone(),
            Some("11111111-1111-4111-8111-111111111111".to_string()),
            None,
            traits::ResumeRuntimeSnapshot::default(),
        )
        .await
        .expect("resume_session must succeed on the production handle");

    // History was adopted in place — the next turn will see prior context.
    let restored = handle.conversation_transcript().await;
    assert_eq!(
        restored, history,
        "resume must adopt the replayed transcript"
    );

    // The NAMED id was adopted (resume does NOT mint a fresh one).
    assert_eq!(
        handle.current_session_id().await,
        named,
        "resume must adopt the named session id, not mint a fresh one"
    );

    // The live model is UNCHANGED (resume keeps the running model).
    assert_eq!(
        handle.get_status_snapshot().await.model,
        original_model,
        "resume must not change the live model"
    );
}
