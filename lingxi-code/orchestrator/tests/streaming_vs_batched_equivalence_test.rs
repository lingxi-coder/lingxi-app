//! streaming (M5-04 Task 17).
//!
//! Asserts:
//!   - both paths produce `ConversationOutcome::EndTurn { turn_count: 1, .. }`
//!   - both paths append identical assistant message bodies to the session.
use llm_client::{ContentBlock as LlmContentBlock, LlmResponse, Usage};
use orchestrator::test_support::{
    content_block_start_text, content_block_stop, message_delta_stop, message_start, message_stop,
    text_delta, MockApiClient, MockOutputStream, MockStreamingApiClient, NoOpPermissionGate,
    StaticMemoryProvider,
};
use orchestrator::{scripted, ConversationOrchestrator, ConversationOutcome, OrchestratorConfig};
use protocol::{ContentBlock, ConversationMessage};
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use tool_api::registry::ToolRegistry;

fn run_with_large_stack<F, Fut>(build: F)
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = ()>,
{
    let handle = std::thread::Builder::new()
        .name("orchestrator-equivalence-test".into())
        .stack_size(64 * 1024 * 1024)
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build test runtime");
            runtime.block_on(build());
        })
        .expect("spawn large-stack test thread");
    handle.join().expect("large-stack test thread panicked");
}

fn batched_response(text: &str) -> LlmResponse {
    LlmResponse {
        id: "msg_eq".into(),
        model: "claude-opus-4-7".into(),
        content: vec![LlmContentBlock::Text {
            text: text.into(),
            cache_control: None,
        }],
        stop_reason: Some("end_turn".into()),
        stop_details: None,
        usage: Usage::default(),
        cost: None,
        provider_metadata: serde_json::Value::Null,
    }
}

#[test]
fn batched_and_streaming_produce_same_assistant_text() {
    run_with_large_stack(|| async {
        // ── Batched path
        let batched_mock = Arc::new(MockApiClient::new(vec![batched_response("hello world")]));
        let streaming_stub = Arc::new(MockStreamingApiClient::empty());
        let output_b = Arc::new(MockOutputStream::new());
        let orch_b = ConversationOrchestrator::new_with_streaming(
            OrchestratorConfig::default(),
            batched_mock,
            streaming_stub,
            Arc::new(ToolRegistry::new()),
            orchestrator::test_support::noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            output_b.clone(),
            Arc::new(StaticMemoryProvider::empty()),
            PathBuf::from("/tmp"),
        );
        let outcome_b = orch_b.run_turn("ping").await.expect("batched");

        // ── Streaming path
        let stream_script = scripted![
            message_start("m1", "claude-opus-4-7"),
            content_block_start_text(0),
            text_delta(0, "hello world"),
            content_block_stop(0),
            message_delta_stop("end_turn"),
            message_stop(),
        ];
        let streaming_mock = Arc::new(MockStreamingApiClient::with_turns(vec![stream_script]));
        let batched_stub = Arc::new(MockApiClient::new(Vec::new()));
        let output_s = Arc::new(MockOutputStream::new());
        let orch_s = ConversationOrchestrator::new_with_streaming(
            OrchestratorConfig::default(),
            batched_stub,
            streaming_mock,
            Arc::new(ToolRegistry::new()),
            orchestrator::test_support::noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            output_s.clone(),
            Arc::new(StaticMemoryProvider::empty()),
            PathBuf::from("/tmp"),
        );
        let outcome_s = orch_s.run_turn_streaming("ping").await.expect("stream");

        // Same outcome shape.
        match (outcome_b, outcome_s) {
            (
                ConversationOutcome::EndTurn {
                    turn_count: tc_b, ..
                },
                ConversationOutcome::EndTurn {
                    turn_count: tc_s, ..
                },
            ) => {
                assert_eq!(tc_b, 1);
                assert_eq!(tc_s, 1);
            }
            _ => panic!("unexpected outcome variants"),
        }

        // Same assistant text in the session history.
        let session_b = orch_b.session();
        let session_s = orch_s.session();
        let s_b = session_b.lock().await;
        let s_s = session_s.lock().await;
        let extract_text = |hist: &[ConversationMessage]| -> Option<String> {
            for m in hist {
                if let ConversationMessage::Assistant { content, .. } = m {
                    for blk in content {
                        if let ContentBlock::Text { text } = blk {
                            return Some(text.clone());
                        }
                    }
                }
            }
            None
        };
        assert_eq!(extract_text(&s_b.history), Some("hello world".into()));
        assert_eq!(extract_text(&s_s.history), Some("hello world".into()));
    });
}

// ── #3 (main-loop parity): per-turn reminder symmetry across paths ───────────
//
// Both the batched (`run_turn` / `try_run_turn_cancelable`) and streaming
// (`run_turn_streaming`) drivers must inject the SAME per-turn reminders in the
// canonical order. Historically the batched path injected `<new-diagnostics>`
// but NOT `<task-notification>`, and the streaming path the reverse — both are
// consume-once drains, so the bytes sent to the model diverged purely by entry
// path. These two tests pin the symmetry: each reminder, when its source is
// wired, now appears on BOTH paths.

struct OnceTaskNotifications(std::sync::Mutex<Vec<traits::task_registry::TaskNotification>>);
#[async_trait::async_trait]
impl orchestrator::prompt::task_notification::TaskNotificationProvider for OnceTaskNotifications {
    async fn take_pending_task_notifications(
        &self,
    ) -> Vec<traits::task_registry::TaskNotification> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }
}

struct MockDiag(Option<String>);
#[async_trait::async_trait]
impl traits::NewDiagnosticsSource for MockDiag {
    async fn take_new_diagnostics_block(&self) -> Option<String> {
        self.0.clone()
    }
}

fn one_task_notification() -> traits::task_registry::TaskNotification {
    traits::task_registry::TaskNotification {
        task_id: "b12345678".into(),
        task_type: "local_bash".into(),
        status: "completed".into(),
        description: "run tests".into(),
        tool_use_id: None,
        output_path: Some("/tmp/tasks/b12345678.output".into()),
        exit_code: Some(0),
        error: None,
        result: None,
        usage: None,
        killed_by: None,
        worktree_path: None,
        worktree_branch: None,
        workflow_failures: Vec::new(),
        workflow_agent_count: None,
        workflow_total_tokens: None,
        workflow_total_tool_calls: None,
        workflow_duration_ms: None,
        ..Default::default()
    }
}

/// BATCHED path must inject the `<task-notification>` reminder (parity with
/// streaming). Pre-fix the batched snapshot omitted it entirely.
#[test]
fn batched_turn_injects_task_notification_reminder() {
    run_with_large_stack(|| async {
        let batched_mock = Arc::new(MockApiClient::new(vec![batched_response("ok")]));
        let streaming_stub = Arc::new(MockStreamingApiClient::empty());
        let orch = ConversationOrchestrator::new_with_streaming(
            OrchestratorConfig::default(),
            batched_mock.clone(),
            streaming_stub,
            Arc::new(ToolRegistry::new()),
            orchestrator::test_support::noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            PathBuf::from("/tmp"),
        )
        .with_task_notifications(Arc::new(OnceTaskNotifications(std::sync::Mutex::new(
            vec![one_task_notification()],
        ))));
        orch.run_turn("ping").await.expect("batched");
        let calls = batched_mock.captured_msgs().await;
        let first = format!("{:?}", calls[0]);
        assert!(
        first.contains("<task-notification>"),
        "batched turn must inject the task-notification reminder (parity with streaming); got: {first}"
    );
    });
}

/// STREAMING path must inject the `<new-diagnostics>` reminder (parity with
/// batched). Pre-fix the streaming snapshot omitted it entirely.
#[test]
fn streaming_turn_injects_new_diagnostics_reminder() {
    run_with_large_stack(|| async {
        let stream_script = scripted![
            message_start("m1", "claude-opus-4-7"),
            content_block_start_text(0),
            text_delta(0, "ok"),
            content_block_stop(0),
            message_delta_stop("end_turn"),
            message_stop(),
        ];
        let streaming_mock = Arc::new(MockStreamingApiClient::with_turns(vec![stream_script]));
        let batched_stub = Arc::new(MockApiClient::new(Vec::new()));
        let block = "<new-diagnostics>The following new diagnostic issues were detected:\n\nx.rs:\n  \u{2718} [Line 1:1] boom</new-diagnostics>";
        let orch = ConversationOrchestrator::new_with_streaming(
            OrchestratorConfig::default(),
            batched_stub,
            streaming_mock.clone(),
            Arc::new(ToolRegistry::new()),
            orchestrator::test_support::noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            PathBuf::from("/tmp"),
        )
        .with_new_diagnostics_source(Arc::new(MockDiag(Some(block.to_string()))));
        orch.run_turn_streaming("ping").await.expect("stream");
        let calls = streaming_mock.captured_calls().await;
        let first = format!("{:?}", calls[0].messages);
        assert!(
        first.contains("<new-diagnostics>"),
        "streaming turn must inject the new-diagnostics reminder (parity with batched); got: {first}"
    );
    });
}

/// #1 (main-loop parity): a connect-phase prompt-too-long on the STREAMING path
/// (the adapter returns `Err(ContextOverflow)` from `stream()`) must trigger the
/// reactive PTL recovery — the SAME `call_api_with_ptl_recovery` helper the
/// batched path uses — recovering via a non-streaming call, NOT bubbling a hard
/// `OrchestratorError::Streaming` error (the prior documented divergence).
#[test]
fn streaming_connect_413_recovers_via_reactive_ptl() {
    run_with_large_stack(|| async {
        use llm_client::LlmError;
        // The stream OPEN returns a connect-phase 413/ContextOverflow once.
        let streaming_mock = Arc::new(MockStreamingApiClient::with_open_error(
            LlmError::ContextOverflow { token_gap: 100 },
            Vec::new(),
        ));
        // Reactive recovery issues a batched messages_create that SUCCEEDS.
        let batched_mock = Arc::new(MockApiClient::new(vec![batched_response("recovered")]));
        let orch = ConversationOrchestrator::new_with_streaming(
            OrchestratorConfig::default(),
            batched_mock.clone(),
            streaming_mock.clone(),
            Arc::new(ToolRegistry::new()),
            orchestrator::test_support::noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            PathBuf::from("/tmp"),
        );
        let outcome = orch
            .run_turn_streaming("ping")
            .await
            .expect("streaming 413 must RECOVER, not surface a hard error");
        match outcome {
            ConversationOutcome::EndTurn { turn_count, .. } => assert_eq!(turn_count, 1),
            other => panic!("expected EndTurn after recovery, got {other:?}"),
        }
        // One stream-open attempt; the recovery then used the batched API once.
        assert_eq!(
            streaming_mock.captured_calls().await.len(),
            1,
            "exactly one stream-open attempt"
        );
        assert_eq!(
            batched_mock.captured_msgs().await.len(),
            1,
            "reactive recovery issued exactly one batched call"
        );
        // The recovered assistant text reached the session history.
        let session = orch.session();
        let s = session.lock().await;
        let has_recovered = s.history.iter().any(|m| {
        matches!(m, ConversationMessage::Assistant { content, .. }
            if content.iter().any(|b| matches!(b, ContentBlock::Text { text } if text == "recovered")))
    });
        assert!(
            has_recovered,
            "recovered assistant text must be appended to session history"
        );
    });
}
