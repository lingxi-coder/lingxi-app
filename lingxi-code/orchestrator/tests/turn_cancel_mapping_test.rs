//! §3.2, the late-cancel rows: a token that fires AFTER the model step already
//! succeeded.
//!
//! The two entries answer differently, and §3.2 records both:
//!
//!   Batched: 成功返回后晚到取消 — 不追加一次 token 检查，保留已完成的
//!            EndTurn，不覆盖成 Cancelled
//!   Streaming: 成功返回后晚到取消 — 主 streaming 包装可将成功 EndTurn /
//!              StopHookPrevented 映射为 Cancelled
//!
//! §8's risk table states the direction of the danger outright: "Streaming 包装
//! 把晚到取消映射为 Cancelled，batched 没有 … 禁止在 batched 最终 return 探测
//! token." PR 5 unifies the entry wrappers, and a single shared "was the token
//! cancelled?" check at the end is the obvious way to write one. It would hand
//! batched a mapping it has never had, and the symptom is a turn that finished
//! its work being reported as cancelled — the work is done, persisted, and the
//! caller is told it did not happen.
//!
//! "Late" is made deterministic rather than raced: the provider cancels the
//! token as a side effect and THEN returns a successful response, so by the time
//! the step resolves the token is already cancelled. No sleeps, no ordering
//! assumptions.

use async_trait::async_trait;
use llm_client::{ContentBlock as LlmContentBlock, LlmError, LlmResponse};
use orchestrator::test_support::{
    mock_message_response, MockOutputStream, NoOpPermissionGate, StaticMemoryProvider,
};
use orchestrator::{
    ConversationOrchestrator, OrchestratorApiClient, OrchestratorConfig, TurnOutcome,
};
use protocol::ConversationMessage;
use std::path::PathBuf;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tokio_util::sync::CancellationToken as Token;
use tool_api::registry::ToolRegistry;

/// A provider that cancels the turn's token and then answers successfully.
///
/// This is what makes the test deterministic: the cancellation is observable
/// before the model step resolves, so the step completing and the token being
/// cancelled are not racing.
struct CancelsThenSucceeds {
    token: Token,
    calls: std::sync::atomic::AtomicUsize,
}

#[async_trait]
impl OrchestratorApiClient for CancelsThenSucceeds {
    async fn messages_create(
        &self,
        _model: &str,
        _profile: Option<&str>,
        _system: Option<&str>,
        _msgs: Vec<ConversationMessage>,
        _tools: Vec<serde_json::Value>,
    ) -> Result<LlmResponse, LlmError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        // Fire the cancel BEFORE answering, so the step resolves into an
        // already-cancelled turn.
        self.token.cancel();
        Ok(mock_message_response(
            vec![LlmContentBlock::Text {
                text: "finished the work".into(),
                cache_control: None,
            }],
            Some("end_turn"),
        ))
    }
}

/// BATCHED: a cancel that arrives after the step succeeded does NOT rewrite the
/// outcome.
///
/// `try_run_turn_cancelable` races the step against the token, and when the step
/// wins it proceeds to the disposition without consulting the token again. The
/// turn produced an answer and persisted it; reporting `Cancelled` would tell
/// the caller that work never happened.
#[test]
fn a_cancel_arriving_after_the_batched_step_succeeded_keeps_the_end_turn() {
    let handle = std::thread::Builder::new()
        .name("late-cancel-mapping".into())
        .stack_size(64 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime")
                .block_on(async {
                    let token = CancellationToken::new();
                    let api = Arc::new(CancelsThenSucceeds {
                        token: token.clone(),
                        calls: std::sync::atomic::AtomicUsize::new(0),
                    });
                    let orch = ConversationOrchestrator::new(
                        OrchestratorConfig::default(),
                        api.clone(),
                        Arc::new(ToolRegistry::new()),
                        orchestrator::test_support::noop_hook_executor(),
                        Arc::new(NoOpPermissionGate),
                        Arc::new(MockOutputStream::new()),
                        Arc::new(StaticMemoryProvider::empty()),
                        PathBuf::from("/tmp"),
                    );

                    let outcome = orch
                        .run_turn_with_cancel("ping", token.clone())
                        .await
                        .expect("turn");

                    assert!(
                        token.is_cancelled(),
                        "the fixture must actually have cancelled the token, or this test \
                         proves nothing"
                    );
                    assert_eq!(
                        api.calls.load(std::sync::atomic::Ordering::SeqCst),
                        1,
                        "exactly one model step, and it succeeded"
                    );
                    assert_eq!(
                        outcome,
                        TurnOutcome::EndTurn,
                        "a cancel that lands after the step succeeded must NOT rewrite the \
                         outcome. §3.2: batched adds no final token probe, and §8 forbids \
                         giving it one. A shared entry wrapper that ends with 'was the token \
                         cancelled?' reports a finished, persisted turn as Cancelled."
                    );

                    // And the answer really did land, which is what makes the
                    // wrong mapping a lie rather than a cosmetic difference.
                    let session = orch.session();
                    let history = session.lock().await.history.clone();
                    assert!(
                        history
                            .iter()
                            .any(|m| m.text_content().contains("finished the work")),
                        "the assistant's answer must be in history — the turn did its work"
                    );
                });
        })
        .expect("spawn");
    handle.join().expect("test thread panicked");
}
