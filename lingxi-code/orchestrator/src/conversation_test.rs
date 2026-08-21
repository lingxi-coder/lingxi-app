//! Unit tests for the conversation orchestrator.

use super::*;

impl ConversationOrchestrator {
    async fn restore_post_compact_attachments(&self) -> Vec<protocol::ConversationMessage> {
        self.restore_post_compact_attachments_against(&[]).await
    }
}

#[cfg(test)]
mod ephemeral_tool_result_persistence_tests {
    use super::redact_ephemeral_tool_result_images;
    use protocol::{ContentBlock, ConversationMessage, MessageId, ToolUseId};
    use serde_json::json;

    fn tool_result(
        content: String,
        content_blocks: Option<Vec<serde_json::Value>>,
    ) -> ConversationMessage {
        ConversationMessage::User {
            id: MessageId::new(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: ToolUseId::new(),
                content,
                is_error: false,
                provider_tool_use_id: None,
                content_blocks,
            }],
            is_meta: false,
            is_compact_summary: false,
            is_visible_in_transcript_only: false,
        }
    }

    #[test]
    fn ephemeral_images_are_removed_only_from_the_persisted_clone() {
        let original = tool_result(
            json!({
                "_lingxi_ephemeral": true,
                "summary": "Temporary screenshot omitted."
            })
            .to_string(),
            Some(vec![json!({
                "type": "image",
                "source": {"type": "base64", "media_type": "image/png", "data": "secret"}
            })]),
        );
        let sanitized = redact_ephemeral_tool_result_images(&original);
        let ConversationMessage::User {
            content: sanitized_blocks,
            ..
        } = &sanitized
        else {
            panic!("expected user message");
        };
        let ContentBlock::ToolResult {
            content,
            content_blocks,
            ..
        } = &sanitized_blocks[0]
        else {
            panic!("expected tool result");
        };
        assert_eq!(content, "Temporary screenshot omitted.");
        assert!(content_blocks.is_none());
        let ConversationMessage::User {
            content: original_blocks,
            ..
        } = &original
        else {
            panic!("expected user message");
        };
        let ContentBlock::ToolResult { content_blocks, .. } = &original_blocks[0] else {
            panic!("expected tool result");
        };
        assert!(
            content_blocks.is_some(),
            "live in-memory message must stay intact"
        );
    }

    #[test]
    fn unmarked_tool_results_are_byte_for_byte_unchanged() {
        let original = tool_result(
            "ordinary result".into(),
            Some(vec![json!({"type": "text", "text": "ordinary"})]),
        );
        assert_eq!(redact_ephemeral_tool_result_images(&original), original);
    }
}
#[cfg(test)]
mod generated_session_name_tests {
    use super::parse_generated_session_name;

    #[test]
    fn parses_plain_and_fenced_json_but_rejects_empty_or_prose() {
        assert_eq!(
            parse_generated_session_name(r#"{"name":"fix-login-bug"}"#).as_deref(),
            Some("fix-login-bug")
        );
        assert_eq!(
            parse_generated_session_name("```json\n{\"name\":\"add-auth-feature\"}\n```")
                .as_deref(),
            Some("add-auth-feature")
        );
        assert_eq!(parse_generated_session_name(r#"{"name":"  "}"#), None);
        assert_eq!(parse_generated_session_name("not json"), None);
    }
}
// ============================================================================
// Turn-recovery behaviors (RECOV.1 / RECOV.2 / RECOV.4)
// ============================================================================
//
// In-file integration tests for the three turn-driver recovery behaviors ported
// from claude-code `query.ts`:
//   - RECOV.1 — the streaming driver's blocking-limit preempt
//     (`query.ts:592-648`): a prompt already at the hard blocking limit ends the
//     turn with the byte-exact prompt-too-long message WITHOUT opening the stream.
//   - RECOV.2 — `StopFailure` hooks fire on an api-error turn-end
//     (`query.ts:1174/1181/1263`); the normal `Stop` hooks do NOT.
//   - RECOV.4 — a Stop-hook blocking continuation resets the
//     `max_output_tokens` recovery budget (`query.ts:1291`).
#[cfg(test)]
mod turn_recovery_tests {
    use super::*;
    use crate::test_support::{
        content_block_start_text, content_block_stop, message_delta_stop, message_start,
        message_stop, mock_message_response, noop_hook_executor, text_delta, MockApiClient,
        MockOutputStream, MockStreamingApiClient, NoOpPermissionGate, StaticMemoryProvider,
    };

    #[test]
    fn compact_focus_and_hook_instructions_are_separate_paragraphs() {
        assert_eq!(
            merge_compact_instructions(Some("focus on Rust"), Some("preserve test output"))
                .as_deref(),
            Some("focus on Rust\n\npreserve test output")
        );
    }

    #[test]
    fn sdk_compact_metadata_keys_are_camelized_recursively() {
        let metadata = camelize_json_keys(serde_json::json!({
            "trigger": "manual",
            "pre_tokens": 42,
            "preserved_segment": {
                "head_uuid": "head",
                "anchor_uuid": "anchor",
                "tail_uuid": "tail"
            },
            "pre_compact_discovered_tools": ["Read"]
        }));
        assert_eq!(metadata["trigger"], "manual");
        assert_eq!(metadata["preTokens"], 42);
        assert_eq!(metadata["preservedSegment"]["headUuid"], "head");
        assert_eq!(metadata["preservedSegment"]["anchorUuid"], "anchor");
        assert_eq!(metadata["preservedSegment"]["tailUuid"], "tail");
        assert_eq!(metadata["preCompactDiscoveredTools"][0], "Read");
    }
    use crate::OrchestratorConfig;
    use hooks::definition::{HookDefinition, HookExecutor as DefHookExecutor, HookSource};
    use hooks::events::HookEventType;
    use hooks::executor::BuiltinHookHandler;
    use hooks::registry::HookRegistry;
    use hooks::response::{HookDecision, HookOutcome, HookResponse, HookResult};
    use hooks::HookExecutorImpl;
    use llm_client::ContentBlock as LlmContentBlock;
    use protocol::{HookId, HttpRequest, HttpResponse};
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex as StdMutex;
    use std::time::Duration;
    use tokio::sync::{Notify, RwLock};
    use traits::{HttpError, HttpTransport, OutputEvent, RuntimeError, RuntimeSpawner};

    async fn wait_for_prewarm_capture(
        api: &MockApiClient,
    ) -> Vec<crate::test_support::MockPrewarmCall> {
        for _ in 0..50 {
            let captured = api.captured_prewarm().await;
            if !captured.is_empty() {
                return captured;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        api.captured_prewarm().await
    }

    struct BlockingPrewarmApiClient {
        active: Arc<AtomicBool>,
        started: Notify,
    }

    impl BlockingPrewarmApiClient {
        fn new() -> Self {
            Self {
                active: Arc::new(AtomicBool::new(false)),
                started: Notify::new(),
            }
        }

        async fn wait_started(&self) {
            loop {
                let notified = self.started.notified();
                if self.active.load(Ordering::SeqCst) {
                    return;
                }
                tokio::time::timeout(Duration::from_secs(1), notified)
                    .await
                    .expect("startup prewarm should start");
            }
        }

        async fn wait_inactive(&self) {
            for _ in 0..50 {
                if !self.active.load(Ordering::SeqCst) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            panic!("startup prewarm should have been aborted");
        }
    }

    struct ActivePrewarmGuard(Arc<AtomicBool>);

    impl Drop for ActivePrewarmGuard {
        fn drop(&mut self) {
            self.0.store(false, Ordering::SeqCst);
        }
    }

    #[async_trait]
    impl OrchestratorApiClient for BlockingPrewarmApiClient {
        async fn messages_create(
            &self,
            _model: &str,
            _profile: Option<&str>,
            _system: Option<&str>,
            _msgs: Vec<ConversationMessage>,
            _tools: Vec<serde_json::Value>,
        ) -> Result<LlmResponse, LlmError> {
            Err(LlmError::Transport {
                message: "blocking prewarm api does not serve messages_create".into(),
            })
        }

        async fn prewarm_responses_websocket(
            &self,
            _model: &str,
            _profile: Option<&str>,
            _system: Option<&str>,
            _messages: Vec<ConversationMessage>,
            _tools: Vec<serde_json::Value>,
        ) -> Result<(), LlmError> {
            self.active.store(true, Ordering::SeqCst);
            self.started.notify_waiters();
            let _guard = ActivePrewarmGuard(self.active.clone());
            std::future::pending::<()>().await;
            Ok(())
        }
    }

    // ---- unused HTTP / Runtime stubs (Builtin hooks never touch them) ----
    struct UnusedHttp;
    #[async_trait]
    impl HttpTransport for UnusedHttp {
        async fn request(&self, _r: HttpRequest) -> Result<HttpResponse, HttpError> {
            Err(HttpError::InvalidRequest("unused".into()))
        }
        async fn stream_sse(&self, _r: HttpRequest) -> Result<traits::http::SseStream, HttpError> {
            Err(HttpError::InvalidRequest("unused".into()))
        }
    }
    struct UnusedRuntime;
    #[async_trait]
    impl RuntimeSpawner for UnusedRuntime {
        async fn spawn(
            &self,
            _n: &str,
            _t: Pin<Box<dyn std::future::Future<Output = ()> + Send + 'static>>,
        ) -> Result<traits::BackgroundTaskHandle, RuntimeError> {
            Err(RuntimeError::Internal("unused".into()))
        }
        async fn sleep(&self, _d: Duration) {}
        async fn cancel(&self, _h: &traits::BackgroundTaskHandle) -> Result<(), RuntimeError> {
            Ok(())
        }
    }

    /// Records every `Stop` / `StopFailure` lifecycle event it sees as
    /// `"Stop:<reason>"` / `"StopFailure:<error>"`. Pass-through (no decision).
    struct RecordingLifecycleHandler {
        log: Arc<StdMutex<Vec<String>>>,
    }
    #[async_trait]
    impl BuiltinHookHandler for RecordingLifecycleHandler {
        fn id(&self) -> &str {
            "rec-lifecycle"
        }
        async fn handle(&self, event: &HookEvent, _ctx: &HookContext) -> HookResult {
            match event {
                HookEvent::Stop { reason } => {
                    self.log.lock().unwrap().push(format!("Stop:{reason}"));
                }
                HookEvent::StopFailure { error } => {
                    self.log
                        .lock()
                        .unwrap()
                        .push(format!("StopFailure:{error}"));
                }
                _ => {}
            }
            HookResult {
                outcome: HookOutcome::Success,
                stdout: String::new(),
                stderr: String::new(),
                exit_code: None,
                response: None,
            }
        }
    }

    /// Stop hook that ALWAYS blocks (asks the agent to keep working). The
    /// re-entry guard converts a SECOND block (when `stop_hook_active`) into a
    /// pass so the loop cannot spin forever.
    struct BlockingStopHandler;
    #[async_trait]
    impl BuiltinHookHandler for BlockingStopHandler {
        fn id(&self) -> &str {
            "block-stop"
        }
        async fn handle(&self, event: &HookEvent, _ctx: &HookContext) -> HookResult {
            let response = matches!(event, HookEvent::Stop { .. }).then(|| HookResponse {
                decision: Some(HookDecision::Block),
                reason: Some("keep going".into()),
                system_message: Some("[stop-hook] please continue".into()),
                ..Default::default()
            });
            HookResult {
                outcome: HookOutcome::Success,
                stdout: String::new(),
                stderr: String::new(),
                exit_code: None,
                response,
            }
        }
    }

    /// `PreCompact` hook that ALWAYS blocks — exercises the compaction abort
    /// (TS `xhe` sets `blockedBy` from the blocked result; `VJn` / proactive /
    /// reactive all honor it by aborting the pass).
    struct BlockingPreCompactHandler;
    #[async_trait]
    impl BuiltinHookHandler for BlockingPreCompactHandler {
        fn id(&self) -> &str {
            "block-precompact"
        }
        async fn handle(&self, event: &HookEvent, _ctx: &HookContext) -> HookResult {
            let response = matches!(event, HookEvent::PreCompact { .. }).then(|| HookResponse {
                decision: Some(HookDecision::Block),
                reason: Some("[guard] compaction not allowed".into()),
                ..Default::default()
            });
            HookResult {
                outcome: HookOutcome::Success,
                stdout: String::new(),
                stderr: String::new(),
                exit_code: None,
                response,
            }
        }
    }

    struct InstructingPreCompactHandler;
    #[async_trait]
    impl BuiltinHookHandler for InstructingPreCompactHandler {
        fn id(&self) -> &str {
            "instruct-precompact"
        }

        async fn handle(&self, _event: &HookEvent, _ctx: &HookContext) -> HookResult {
            HookResult {
                outcome: HookOutcome::Success,
                stdout: "preserve the test evidence".into(),
                stderr: String::new(),
                exit_code: Some(0),
                response: None,
            }
        }
    }

    /// `SessionStart` hook that emits `hookSpecificOutput.additionalContext`
    /// (`Some`) or nothing (`None`) — exercises the SESSIONSTART.CTX consumption.
    struct SessionStartCtxHandler {
        ctx: Option<String>,
    }
    #[async_trait]
    impl BuiltinHookHandler for SessionStartCtxHandler {
        fn id(&self) -> &str {
            "sess-ctx"
        }
        async fn handle(&self, event: &HookEvent, _ctx: &HookContext) -> HookResult {
            let response = matches!(event, HookEvent::SessionStart { .. }).then(|| HookResponse {
                additional_context: self.ctx.clone(),
                ..Default::default()
            });
            HookResult {
                outcome: HookOutcome::Success,
                stdout: String::new(),
                stderr: String::new(),
                exit_code: None,
                response,
            }
        }
    }

    async fn exec_session_start_ctx(ctx: Option<String>) -> Arc<HookExecutorImpl> {
        let registry = Arc::new(RwLock::new(HookRegistry::new()));
        registry
            .write()
            .await
            .register(builtin_hook("sess-ctx", HookEventType::SessionStart));
        let mut exec =
            HookExecutorImpl::new(registry, Arc::new(UnusedHttp), Arc::new(UnusedRuntime));
        exec.register_builtin(Arc::new(SessionStartCtxHandler { ctx }));
        Arc::new(exec)
    }

    fn builtin_hook(handler_id: &str, event_type: HookEventType) -> HookDefinition {
        HookDefinition {
            id: HookId::new(),
            name: handler_id.into(),
            events: vec![event_type],
            if_condition: None,
            executor: DefHookExecutor::Builtin {
                handler_id: handler_id.into(),
            },
            source: HookSource::User,
            blocking: true,
            timeout: None,
            priority: 0,
            once: false,
            status_message: None,
            async_rewake: false,
            async_timeout: None,
            rewake_message: None,
        }
    }

    async fn exec_recording(
        log: Arc<StdMutex<Vec<String>>>,
        events: &[HookEventType],
    ) -> Arc<HookExecutorImpl> {
        let registry = Arc::new(RwLock::new(HookRegistry::new()));
        {
            let mut r = registry.write().await;
            for ev in events {
                r.register(builtin_hook("rec-lifecycle", ev.clone()));
            }
        }
        let mut exec =
            HookExecutorImpl::new(registry, Arc::new(UnusedHttp), Arc::new(UnusedRuntime));
        exec.register_builtin(Arc::new(RecordingLifecycleHandler { log }));
        Arc::new(exec)
    }

    async fn exec_blocking_stop() -> Arc<HookExecutorImpl> {
        let registry = Arc::new(RwLock::new(HookRegistry::new()));
        registry
            .write()
            .await
            .register(builtin_hook("block-stop", HookEventType::Stop));
        let mut exec =
            HookExecutorImpl::new(registry, Arc::new(UnusedHttp), Arc::new(UnusedRuntime));
        exec.register_builtin(Arc::new(BlockingStopHandler));
        Arc::new(exec)
    }

    async fn exec_blocking_pre_compact() -> Arc<HookExecutorImpl> {
        let registry = Arc::new(RwLock::new(HookRegistry::new()));
        registry
            .write()
            .await
            .register(builtin_hook("block-precompact", HookEventType::PreCompact));
        let mut exec =
            HookExecutorImpl::new(registry, Arc::new(UnusedHttp), Arc::new(UnusedRuntime));
        exec.register_builtin(Arc::new(BlockingPreCompactHandler));
        Arc::new(exec)
    }

    async fn exec_instructing_pre_compact() -> Arc<HookExecutorImpl> {
        let registry = Arc::new(RwLock::new(HookRegistry::new()));
        registry.write().await.register(builtin_hook(
            "instruct-precompact",
            HookEventType::PreCompact,
        ));
        let mut exec =
            HookExecutorImpl::new(registry, Arc::new(UnusedHttp), Arc::new(UnusedRuntime));
        exec.register_builtin(Arc::new(InstructingPreCompactHandler));
        Arc::new(exec)
    }

    fn compact_orch(hooks: Arc<HookExecutorImpl>) -> ConversationOrchestrator {
        ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(ToolRegistry::new()),
            hooks,
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        )
    }

    // A blocking PreCompact hook aborts compaction in every route (TS `xhe` /
    // `VJn`): `fire_pre_compact` surfaces the block detail so the caller can
    // throw ("Compaction blocked by PreCompact hook: …") / log + skip.
    #[tokio::test]
    async fn pre_compact_block_returns_detail_else_none() {
        let blocked = compact_orch(exec_blocking_pre_compact().await)
            .fire_pre_compact("manual", None)
            .await;
        assert_eq!(
            blocked.blocked_by.as_deref(),
            Some("[guard] compaction not allowed"),
            "a blocking PreCompact hook must surface its blockedBy detail"
        );

        // No PreCompact hook registered → None → compaction proceeds unchanged.
        let proceed = compact_orch(noop_hook_executor())
            .fire_pre_compact("auto", None)
            .await;
        assert_eq!(proceed.blocked_by, None, "no block → compaction proceeds");
    }

    #[tokio::test]
    async fn pre_compact_success_stdout_becomes_summary_instructions() {
        let outcome = compact_orch(exec_instructing_pre_compact().await)
            .fire_pre_compact("manual", Some("focus on Rust"))
            .await;
        assert_eq!(outcome.blocked_by, None);
        assert_eq!(
            outcome.additional_instructions.as_deref(),
            Some("preserve the test evidence")
        );
    }

    /// A Stop hook that blocks EXACTLY ONCE, then passes. Used by tests that need
    /// precisely one stop-hook continuation, isolated from the consecutive-block
    /// CAP (`LINGXI_STOP_HOOK_BLOCK_CAP`, default 8): a block-every-time hook
    /// would now drive up to 8 continuations, so a test asserting a single
    /// continuation must bound the blocking deterministically.
    struct BlockOnceStopHandler {
        blocked: std::sync::atomic::AtomicBool,
    }
    #[async_trait]
    impl BuiltinHookHandler for BlockOnceStopHandler {
        fn id(&self) -> &str {
            "block-stop"
        }
        async fn handle(&self, event: &HookEvent, _ctx: &HookContext) -> HookResult {
            let first = matches!(event, HookEvent::Stop { .. })
                && !self.blocked.swap(true, std::sync::atomic::Ordering::SeqCst);
            let response = first.then(|| HookResponse {
                decision: Some(HookDecision::Block),
                reason: Some("keep going".into()),
                system_message: Some("[stop-hook] please continue".into()),
                ..Default::default()
            });
            HookResult {
                outcome: HookOutcome::Success,
                stdout: String::new(),
                stderr: String::new(),
                exit_code: None,
                response,
            }
        }
    }

    async fn exec_block_once_stop() -> Arc<HookExecutorImpl> {
        let registry = Arc::new(RwLock::new(HookRegistry::new()));
        registry
            .write()
            .await
            .register(builtin_hook("block-stop", HookEventType::Stop));
        let mut exec =
            HookExecutorImpl::new(registry, Arc::new(UnusedHttp), Arc::new(UnusedRuntime));
        exec.register_builtin(Arc::new(BlockOnceStopHandler {
            blocked: std::sync::atomic::AtomicBool::new(false),
        }));
        Arc::new(exec)
    }

    /// A Stop hook that requests `continue:false` (preventContinuation) with a
    /// fixed `stopReason` — terminates the agent loop (FIX C).
    struct PreventStopHandler {
        reason: Option<String>,
    }
    #[async_trait]
    impl BuiltinHookHandler for PreventStopHandler {
        fn id(&self) -> &str {
            "prevent-stop"
        }
        async fn handle(&self, event: &HookEvent, _ctx: &HookContext) -> HookResult {
            let response = matches!(event, HookEvent::Stop { .. }).then(|| HookResponse {
                prevent_continuation: true,
                reason: self.reason.clone(),
                ..Default::default()
            });
            HookResult {
                outcome: HookOutcome::Success,
                stdout: String::new(),
                stderr: String::new(),
                exit_code: None,
                response,
            }
        }
    }

    async fn exec_prevent_stop(reason: Option<String>) -> Arc<HookExecutorImpl> {
        let registry = Arc::new(RwLock::new(HookRegistry::new()));
        registry
            .write()
            .await
            .register(builtin_hook("prevent-stop", HookEventType::Stop));
        let mut exec =
            HookExecutorImpl::new(registry, Arc::new(UnusedHttp), Arc::new(UnusedRuntime));
        exec.register_builtin(Arc::new(PreventStopHandler { reason }));
        Arc::new(exec)
    }

    /// Seed a history far past the hard blocking limit. The default model
    /// (`claude-opus-4-8`) is natively 1M as of 2.1.198 (M1b), so the
    /// blocking limit sits just under 1M tokens; 8M chars ≈ 2M tokens
    /// (estimator is chars/4), comfortably over.
    async fn seed_over_blocking_limit(orch: &ConversationOrchestrator) {
        let session = orch.session();
        let mut s = session.lock().await;
        s.history.push(ConversationMessage::user(
            MessageId::new(),
            "x".repeat(8_000_000),
        ));
    }

    struct RewakeResponses(std::sync::Mutex<Vec<String>>);

    #[async_trait]
    impl crate::prompt::async_hook_response::AsyncHookResponseProvider for RewakeResponses {
        async fn take_pending_responses(&self) -> Vec<String> {
            std::mem::take(&mut *self.0.lock().unwrap())
        }
    }

    #[tokio::test]
    async fn async_hook_rewake_runs_without_persisting_a_synthetic_user_prompt() {
        let streaming = Arc::new(MockStreamingApiClient::with_turns(vec![vec![
            message_start("rewake", "claude-opus-4-7"),
            content_block_start_text(0),
            text_delta(0, "continued"),
            content_block_stop(0),
            message_delta_stop("end_turn"),
            message_stop(),
        ]]));
        let orch = ConversationOrchestrator::new_with_streaming(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            streaming.clone(),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        )
        .with_async_hook_responses(Arc::new(RewakeResponses(std::sync::Mutex::new(vec![
            "background verification finished".into(),
        ]))));

        let outcome = orch.run_async_hook_rewake().await.expect("rewake turn");
        assert_eq!(outcome, TurnOutcome::EndTurn);

        let calls = streaming.captured_calls().await;
        assert_eq!(calls.len(), 1);
        assert!(calls[0].messages.iter().any(|message| {
            message.is_meta()
                && message
                    .text_content()
                    .contains("background verification finished")
        }));
        assert!(
            calls[0]
                .messages
                .iter()
                .all(|message| message.is_meta() || !message.text_content().is_empty()),
            "the provider request must not contain a synthetic empty human prompt"
        );

        let history = orch.session.lock().await.history.clone();
        assert_eq!(history.len(), 1, "only the assistant response is durable");
        assert!(matches!(history[0], ConversationMessage::Assistant { .. }));
    }

    // -------- RECOV.1 — streaming blocking-limit preempt --------

    #[tokio::test]
    async fn recov1_streaming_blocking_limit_preempts_before_opening_stream() {
        // One valid end_turn turn is scripted; if the preempt regresses the
        // stream opens (captured_calls == 1) and the prompt-too-long text is
        // absent — both asserted against below.
        let streaming = Arc::new(MockStreamingApiClient::with_turns(vec![vec![
            message_start("m", "claude-opus-4-7"),
            content_block_start_text(0),
            text_delta(0, "should not be reached"),
            content_block_stop(0),
            message_delta_stop("end_turn"),
            message_stop(),
        ]]));
        let output = Arc::new(MockOutputStream::new());
        let orch = ConversationOrchestrator::new_with_streaming(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            streaming.clone(),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            output.clone(),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        );
        seed_over_blocking_limit(&orch).await;

        let outcome = orch
            .run_turn_streaming("go")
            .await
            .expect("turn ends without a hard error");
        assert!(
            matches!(outcome, ConversationOutcome::EndTurn { .. }),
            "{outcome:?}"
        );

        // The stream was NEVER opened — the preempt short-circuited the API call.
        assert!(
            streaming.captured_calls().await.is_empty(),
            "the blocking-limit preempt must NOT open the stream"
        );

        // The byte-exact prompt-too-long message + an EndTurn("blocking_limit").
        // The PROACTIVE blocking-limit preempt ends with the DISTINCT terminal
        // reason `blocking_limit` (the binary keeps it separate from the
        // reactive-exhausted `prompt_too_long`); the surfaced message text is
        // still the byte-exact "Prompt is too long".
        let events = output.snapshot().await;
        assert!(
            events
                .iter()
                .any(|e| matches!(e, OutputEvent::Text { text } if text == "Prompt is too long")),
            "byte-exact prompt-too-long message must be surfaced; events={events:#?}"
        );
        assert!(
            events.iter().any(
                |e| matches!(e, OutputEvent::EndTurn { stop_reason, .. } if stop_reason == "blocking_limit")
            ),
            "the proactive preempt must end with stop_reason blocking_limit; events={events:#?}"
        );
    }

    #[tokio::test]
    async fn recov1_streaming_blocking_limit_does_not_trigger_budget_continuation() {
        let streaming = Arc::new(MockStreamingApiClient::with_turns(vec![vec![
            message_start("m", "claude-opus-4-7"),
            content_block_start_text(0),
            text_delta(0, "should not be reached"),
            content_block_stop(0),
            message_delta_stop("end_turn"),
            message_stop(),
        ]]));
        let output = Arc::new(MockOutputStream::new());
        let orch = ConversationOrchestrator::new_with_streaming(
            OrchestratorConfig {
                enable_token_budget: true,
                token_budget: Some(500_000),
                ..OrchestratorConfig::default()
            },
            Arc::new(MockApiClient::new(vec![])),
            streaming.clone(),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            output.clone(),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        );
        seed_over_blocking_limit(&orch).await;

        let outcome = orch
            .run_turn_streaming("go")
            .await
            .expect("turn ends without a hard error");
        assert!(
            matches!(outcome, ConversationOutcome::EndTurn { turn_count: 1, .. }),
            "{outcome:?}"
        );
        assert!(
            streaming.captured_calls().await.is_empty(),
            "the blocking-limit preempt must still short-circuit before opening the stream"
        );
        let history = orch.session().lock().await.history.clone();
        assert!(
            !history.iter().any(|m| matches!(
                m,
                protocol::ConversationMessage::User { content, .. }
                    if matches!(
                        content.first(),
                        Some(protocol::ContentBlock::Text { text }) if text.starts_with("Stopped at ")
                    )
            )),
            "terminal API-error ends must not inject a budget-continuation nudge"
        );
        let events = output.snapshot().await;
        let end = events.iter().rev().find_map(|e| match e {
            OutputEvent::EndTurn { stop_reason, .. } => Some(stop_reason.as_str()),
            _ => None,
        });
        assert_eq!(end, Some("blocking_limit"));
    }

    // -------- terminal stop-reason API errors (claude.ts:2266-2292) --------

    #[tokio::test]
    async fn terminal_model_context_window_exceeded_surfaces_api_error() {
        // `model_context_window_exceeded` has no recovery path, so it hits the
        // terminal arm directly and must surface claude-code's byte-locked
        // API-error message (`claude.ts:2279`) before ending the turn.
        let streaming = Arc::new(MockStreamingApiClient::with_turns(vec![vec![
            message_start("m", "claude-opus-4-7"),
            content_block_start_text(0),
            text_delta(0, "partial answer"),
            content_block_stop(0),
            message_delta_stop("model_context_window_exceeded"),
            message_stop(),
        ]]));
        let output = Arc::new(MockOutputStream::new());
        let orch = ConversationOrchestrator::new_with_streaming(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            streaming.clone(),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            output.clone(),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        );
        orch.run_turn_streaming("go").await.expect("turn ends");

        let events = output.snapshot().await;
        assert!(
            events.iter().any(|e| matches!(e, OutputEvent::Text { text }
                if text == "API Error: The model has reached its context window limit.")),
            "byte-exact context-window-exceeded API error must be surfaced; events={events:#?}"
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, OutputEvent::EndTurn { stop_reason, .. }
                if stop_reason == "model_context_window_exceeded")),
            "the turn must end with stop_reason model_context_window_exceeded; events={events:#?}"
        );
    }

    #[tokio::test]
    async fn terminal_refusal_without_fallback_surfaces_safety_message() {
        // A `refusal` with no `refusalFallbackModel` configured hits the terminal
        // arm (the swap arm `continue`s only when a fallback is set), so it must
        // surface claude-code's byte-locked `U2e` refusal message — the model-label
        // branch (resolved via `marketing_name_for_model`), non-interactive suffix
        // (`interactive_permissions` defaults false).
        let streaming = Arc::new(MockStreamingApiClient::with_turns(vec![vec![
            message_start("m", "claude-opus-4-8"),
            content_block_start_text(0),
            text_delta(0, "partial"),
            content_block_stop(0),
            message_delta_stop("refusal"),
            message_stop(),
        ]]));
        let output = Arc::new(MockOutputStream::new());
        let mut cfg = OrchestratorConfig::default();
        cfg.model = "claude-opus-4-8".to_string();
        assert!(
            cfg.refusal_fallback_model.is_none(),
            "default config must have no refusal fallback (else the swap arm runs)"
        );
        let orch = ConversationOrchestrator::new_with_streaming(
            cfg,
            Arc::new(MockApiClient::new(vec![])),
            streaming.clone(),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            output.clone(),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        );
        orch.run_turn_streaming("go").await.expect("turn ends");

        let events = output.snapshot().await;
        let expected = "API Error: Opus 4.8's safeguards flagged this message (https://www.anthropic.com/legal/aup). This sometimes happens with safe, normal conversations. LingXi can't respond to this request with Opus 4.8.\n\nTry rephrasing the request in a new session or change your model.\n\nLearn more: https://support.claude.com/en/articles/15363606";
        assert!(
            events
                .iter()
                .any(|e| matches!(e, OutputEvent::Text { text } if text == expected)),
            "byte-exact U2e refusal message must be surfaced; events={events:#?}"
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, OutputEvent::EndTurn { stop_reason, .. }
                if stop_reason == "refusal")),
            "the turn must end with stop_reason refusal; events={events:#?}"
        );
    }

    // -------- RECOV.2 — StopFailure fires on an api-error turn-end --------

    #[tokio::test]
    async fn recov2_stop_failure_fires_on_api_error_end_and_stop_does_not() {
        // A history over the blocking limit ⇒ the batched proactive preempt ends
        // with terminal reason `blocking_limit`, which is an api-error end (the
        // surfaced message's api-error field is `invalid_request`). `StopFailure`
        // must fire (error == "invalid_request"); the normal `Stop` hooks must NOT.
        let log = Arc::new(StdMutex::new(Vec::<String>::new()));
        let hooks = exec_recording(
            log.clone(),
            &[HookEventType::Stop, HookEventType::StopFailure],
        )
        .await;
        let orch = ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            // Never called — the blocking-limit preempt fires before the API call.
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(ToolRegistry::new()),
            hooks,
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        );
        seed_over_blocking_limit(&orch).await;

        orch.run_turn("go")
            .await
            .expect("turn ends without a hard error");

        let seen = log.lock().unwrap().clone();
        assert!(
            seen.iter().any(|s| s == "StopFailure:invalid_request"),
            "StopFailure must fire on the api-error end with error=invalid_request: {seen:?}"
        );
        assert!(
            !seen.iter().any(|s| s.starts_with("Stop:")),
            "the normal Stop hooks must NOT fire on an api-error end: {seen:?}"
        );
    }

    #[tokio::test]
    async fn recov2_batched_blocking_limit_does_not_trigger_budget_continuation() {
        let api = Arc::new(MockApiClient::new(vec![]));
        let output = Arc::new(MockOutputStream::new());
        let orch = ConversationOrchestrator::new(
            OrchestratorConfig {
                enable_token_budget: true,
                token_budget: Some(500_000),
                ..OrchestratorConfig::default()
            },
            api.clone(),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            output.clone(),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        );
        seed_over_blocking_limit(&orch).await;

        let outcome = orch.run_turn("go").await.expect("turn ends cleanly");
        assert!(
            matches!(outcome, ConversationOutcome::EndTurn { turn_count: 1, .. }),
            "{outcome:?}"
        );
        assert!(
            api.captured_msgs().await.is_empty(),
            "the blocking-limit preempt must short-circuit before any batched API call"
        );
        let history = orch.session().lock().await.history.clone();
        assert!(
            !history.iter().any(|m| matches!(
                m,
                protocol::ConversationMessage::User { content, .. }
                    if matches!(
                        content.first(),
                        Some(protocol::ContentBlock::Text { text }) if text.starts_with("Stopped at ")
                    )
            )),
            "terminal API-error ends must not inject a budget-continuation nudge"
        );
        let events = output.snapshot().await;
        let end = events.iter().rev().find_map(|e| match e {
            OutputEvent::EndTurn { stop_reason, .. } => Some(stop_reason.as_str()),
            _ => None,
        });
        assert_eq!(end, Some("blocking_limit"));
    }

    #[tokio::test]
    async fn startup_responses_websocket_prewarm_uses_current_model_profile_system_and_empty_history(
    ) {
        let api = Arc::new(MockApiClient::new(vec![]));
        let orch = Arc::new(ConversationOrchestrator::new(
            OrchestratorConfig {
                model: "gpt-5".to_string(),
                ..OrchestratorConfig::default()
            },
            api.clone(),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        ));
        {
            let mut session = orch.session.lock().await;
            session.model_profile = Some("openai".to_string());
        }

        orch.spawn_startup_responses_websocket_prewarm();

        let captured = wait_for_prewarm_capture(&api).await;
        assert_eq!(captured.len(), 1);
        let call = &captured[0];
        assert_eq!(call.model, "gpt-5");
        assert_eq!(call.profile.as_deref(), Some("openai"));
        assert!(
            call.messages.is_empty(),
            "startup prewarm uses empty history"
        );
        assert!(
            call.system
                .as_deref()
                .is_some_and(|system| !system.is_empty()),
            "startup prewarm must use the assembled system prompt"
        );
    }

    #[tokio::test]
    async fn system_prompt_model_identity_follows_switch_model() {
        // Regression (reported): /model switched the ROUTED model, but the <env>
        // identity line ("You are powered by the model named …") stayed frozen
        // at config.model, so a switched-to model (e.g. Fable 5) still saw — and
        // reported — the launch model's identity (Opus 4.8). The prompt identity
        // must track the LIVE session.model that switch_model updates.
        let orch = ConversationOrchestrator::new(
            OrchestratorConfig {
                model: "claude-opus-4-8".to_string(),
                ..OrchestratorConfig::default()
            },
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        );

        let before = orch.build_system_prompt().await;
        assert!(
            before.contains(
                "powered by the model named Opus 4.8. The exact model ID is claude-opus-4-8"
            ),
            "launch identity present: {before}"
        );

        <ConversationOrchestrator as traits::OrchestratorHandle>::switch_model(
            &orch,
            "claude-fable-5",
            None,
        )
        .await
        .expect("switch_model");

        let after = orch.build_system_prompt().await;
        assert!(
            after.contains(
                "powered by the model named Fable 5. The exact model ID is claude-fable-5"
            ),
            "identity follows the switch: {after}"
        );
        // The stale identity LINE must be gone. (The static "most recent Claude
        // models … Opus 4.8" catalog sentence is model-independent and stays —
        // so assert on the identity line, not the bare "Opus 4.8" substring.)
        assert!(
            !after.contains("powered by the model named Opus 4.8"),
            "the stale identity line must be gone after switching: {after}"
        );
    }

    #[tokio::test]
    async fn non_claude_switch_uses_the_named_identity_form_not_id_only() {
        // A switched-to NON-Claude model must still get the strong "powered by
        // the model named {name}" form each turn (via the catalog display name),
        // not the weak id-only "the model {id}." — so its current identity is
        // asserted clearly. (Also: no Claude-catalog contamination.)
        let orch = ConversationOrchestrator::new(
            OrchestratorConfig {
                model: "claude-opus-4-8".to_string(),
                ..OrchestratorConfig::default()
            },
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        );
        <ConversationOrchestrator as traits::OrchestratorHandle>::switch_model(
            &orch,
            "deepseek-v4-pro",
            Some("deepseek"),
        )
        .await
        .expect("switch_model");

        let sp = orch.build_system_prompt().await;
        assert!(
            sp.contains(" - You are powered by the model named ")
                && sp.contains("The exact model ID is deepseek-v4-pro."),
            "non-Claude model uses the named form with its exact id: {sp}"
        );
        assert!(
            !sp.contains(" - You are powered by the model deepseek-v4-pro."),
            "must NOT use the weak id-only fallback: {sp}"
        );
        // The prior fix: no Claude model-catalog line for a non-Claude model.
        assert!(
            !sp.contains("claude-fable-5"),
            "no Claude catalog contamination for a non-Claude model: {sp}"
        );
    }

    #[tokio::test]
    async fn system_prompt_reflects_session_cwd_swap() {
        // Task 5 (worktree 206 session-cwd plumbing): `EnterWorktree`/
        // `ExitWorktree` swap the shared `tool_api::SessionCwd` cell the tool
        // layer resolves relative paths through. The NEXT system-prompt
        // render must show the SWAPPED directory's env-block
        // `Primary working directory:` line (and the trailing gitStatus block,
        // which shares the same live cwd) — not the frozen boot cwd.
        let boot_cwd = std::path::PathBuf::from("/tmp/lingxi-session-cwd-boot-fixture");
        let session_cwd = tool_api::SessionCwd::new(boot_cwd.clone(), vec![boot_cwd.clone()]);
        let orch = ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            boot_cwd.clone(),
        )
        .with_session_cwd(session_cwd.clone());

        let before = orch.build_system_prompt().await;
        assert!(
            before.contains(&format!(
                "Primary working directory: {}",
                boot_cwd.display()
            )),
            "boot cwd present before any swap: {before}"
        );

        let worktree_cwd = std::path::PathBuf::from("/tmp/lingxi-session-cwd-worktree-fixture");
        session_cwd.swap(worktree_cwd.clone(), vec![worktree_cwd.clone()]);

        let after = orch.build_system_prompt().await;
        assert!(
            after.contains(&format!(
                "Primary working directory: {}",
                worktree_cwd.display()
            )),
            "system prompt must reflect the swapped worktree cwd: {after}"
        );
        assert!(
            !after.contains(&format!(
                "Primary working directory: {}",
                boot_cwd.display()
            )),
            "the stale boot-cwd line must be gone after the swap: {after}"
        );
    }

    #[tokio::test]
    async fn system_prompt_cwd_stays_at_boot_cwd_when_never_swapped() {
        // INERT INVARIANT: a caller that never calls `.with_session_cwd(...)`
        // gets byte-identical behavior to before Task 5 — the prompt always
        // shows the boot cwd handed to the constructor.
        let boot_cwd = std::path::PathBuf::from("/tmp/lingxi-session-cwd-inert-fixture");
        let orch = ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            boot_cwd.clone(),
        );

        let sp = orch.build_system_prompt().await;
        assert!(
            sp.contains(&format!(
                "Primary working directory: {}",
                boot_cwd.display()
            )),
            "no swap ⇒ boot cwd, exactly as before: {sp}"
        );
    }

    #[tokio::test]
    async fn interactive_session_flag_drives_prompt_and_session_flags() {
        let prior = traits::session_flags::is_non_interactive_session();
        let orch = ConversationOrchestrator::new(
            OrchestratorConfig {
                interactive_permissions: false,
                interactive_session: true,
                ..OrchestratorConfig::default()
            },
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        );

        assert!(
            !traits::session_flags::is_non_interactive_session(),
            "interactive-session composition must publish interactive session flags even when permission prompting stays headless"
        );

        let prompt = orch.build_system_prompt().await;
        assert!(
            prompt.contains("If you need the user to run a shell command themselves"),
            "interactive CLI prompt guidance must follow the explicit interactive-session flag: {prompt}"
        );

        traits::session_flags::set_non_interactive_session(prior);
    }

    #[tokio::test]
    async fn streaming_turn_aborts_pending_startup_prewarm_before_opening_stream() {
        let api = Arc::new(BlockingPrewarmApiClient::new());
        let streaming = Arc::new(MockStreamingApiClient::with_turns(vec![vec![
            message_start("m", "claude-opus-4-7"),
            content_block_start_text(0),
            text_delta(0, "ok"),
            content_block_stop(0),
            message_delta_stop("end_turn"),
            message_stop(),
        ]]));
        let output = Arc::new(MockOutputStream::new());
        let orch = Arc::new(ConversationOrchestrator::new_with_streaming(
            OrchestratorConfig::default(),
            api.clone(),
            streaming.clone(),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            output,
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        ));

        orch.spawn_startup_responses_websocket_prewarm();
        api.wait_started().await;

        let outcome = tokio::time::timeout(Duration::from_secs(1), orch.run_turn_streaming("go"))
            .await
            .expect("streaming turn must not wait for startup prewarm")
            .expect("streaming turn completes");

        assert!(
            matches!(outcome, ConversationOutcome::EndTurn { .. }),
            "{outcome:?}"
        );
        api.wait_inactive().await;
        assert!(
            orch.startup_responses_websocket_prewarm
                .lock()
                .expect("startup responses websocket prewarm")
                .is_none(),
            "turn start must clear the pending startup prewarm handle"
        );
        assert_eq!(
            streaming.captured_calls().await.len(),
            1,
            "the real streaming turn should still open normally"
        );
    }

    #[tokio::test]
    async fn clear_session_aborts_startup_prewarm_and_closes_responses_websocket_session() {
        let api = Arc::new(MockApiClient::new(vec![]));
        let orch = Arc::new(ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            api.clone(),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        ));

        orch.post_compact_skill_attachments
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(MessageId::new(), vec!["skill body".to_string()]);
        orch.tools.deferral().mark_loaded(["StaleTool"]);
        orch.last_response_input_tokens
            .store(42, std::sync::atomic::Ordering::Relaxed);
        orch.output_token_pool
            .store(7, std::sync::atomic::Ordering::Relaxed);
        orch.last_api_call_at_ms
            .store(100, std::sync::atomic::Ordering::Relaxed);
        orch.sent_skill_names
            .lock()
            .await
            .insert("old-skill".into());
        tool_api::read_file_state::set(
            &orch.read_state_map,
            std::env::temp_dir().join("old-session-file"),
            tool_api::read_file_state::ReadFileEntry {
                content: "old".into(),
                mtime_ms: 0,
                offset: None,
                limit: None,
                from_read: true,
                seeded_from_context: false,
                is_partial_view: false,
            },
        );
        orch.spawn_startup_responses_websocket_prewarm();
        <ConversationOrchestrator as traits::OrchestratorHandle>::clear_session(&*orch)
            .await
            .expect("clear session");

        assert_eq!(api.close_responses_ws_count().await, 1);
        assert!(orch.tools.deferral().loaded_tool_names().is_empty());
        assert!(orch.sent_skill_names.lock().await.is_empty());
        assert!(orch
            .read_state_map
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty());
        assert_eq!(
            orch.last_response_input_tokens
                .load(std::sync::atomic::Ordering::Relaxed),
            0
        );
        assert_eq!(
            orch.output_token_pool
                .load(std::sync::atomic::Ordering::Relaxed),
            0
        );
        assert_eq!(
            orch.last_api_call_at_ms
                .load(std::sync::atomic::Ordering::Relaxed),
            -1
        );
        assert!(
            orch.post_compact_skill_attachments
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty(),
            "clearing a session must not leak post-compact attachment identity into the new session"
        );
    }

    // -------- SESSIONSTART.CTX — SessionStart additionalContext consumption ----

    #[tokio::test]
    async fn session_start_additional_context_becomes_persistent_meta_history_message() {
        // A `SessionStart` hook that emits `hookSpecificOutput.additionalContext`
        // must surface it as a persistent `hook_additional_context` meta message
        // in the conversation history (claude-code `processSessionStartHooks`,
        // `sessionStart.ts:163-172` → `messages.ts:4117-4128`), so it rides every
        // subsequent turn. The bytes are the exact `wrapInSystemReminder`
        // (`hookName` = `SessionStart`, multi-line content preserved).
        let orch = ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(ToolRegistry::new()),
            exec_session_start_ctx(Some("Project: lingxi\nBranch: main".into())).await,
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        );
        orch.fire_session_start("startup").await;

        let history = orch.session().lock().await.history.clone();
        assert_eq!(
            history.len(),
            1,
            "exactly one hook_additional_context message; got {history:?}"
        );
        let body = match &history[0] {
            ConversationMessage::User { content, .. } => content
                .iter()
                .filter_map(|b| match b {
                    protocol::ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(""),
            other => panic!("expected a user meta message; got {other:?}"),
        };
        assert_eq!(
            body,
            "<system-reminder>\nSessionStart hook additional context: Project: lingxi\nBranch: main\n</system-reminder>",
            "exact hook_additional_context bytes (hookName=SessionStart, content joined by \\n)"
        );
    }

    #[tokio::test]
    async fn session_start_without_additional_context_pushes_nothing() {
        // Strict no-op: a SessionStart hook that emits no additionalContext leaves
        // the history untouched (the aggregate is discarded exactly as before).
        let orch = ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(ToolRegistry::new()),
            exec_session_start_ctx(None).await,
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        );
        orch.fire_session_start("startup").await;

        assert!(
            orch.session().lock().await.history.is_empty(),
            "no additionalContext ⇒ nothing pushed to history"
        );
    }

    // -------- RECOV.4 — recovery budget reset on stop-hook continuation -----

    #[tokio::test]
    async fn recov4_stop_hook_continuation_resets_max_output_tokens_recovery() {
        // Script: max_tokens, max_tokens, end_turn, then max_tokens ×4.
        // With the reset on the stop-hook continuation, the post-continuation
        // episode gets a FRESH budget of MAX_OUTPUT_TOKENS_RECOVERY_LIMIT (3)
        // nudges, so the loop makes exactly 7 API calls and injects 5 nudges.
        // WITHOUT the reset the carried count (2) would exhaust after only 2
        // more calls (5 total, 3 nudges).
        let mt = || {
            mock_message_response(
                vec![LlmContentBlock::Text {
                    text: "partial".into(),
                    cache_control: None,
                }],
                Some("max_tokens"),
            )
        };
        let et = || {
            mock_message_response(
                vec![LlmContentBlock::Text {
                    text: "done".into(),
                    cache_control: None,
                }],
                Some("end_turn"),
            )
        };
        let api = Arc::new(MockApiClient::new(vec![
            mt(),
            mt(),
            et(),
            mt(),
            mt(),
            mt(),
            mt(),
        ]));
        let orch = ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            api.clone(),
            Arc::new(ToolRegistry::new()),
            // Block-ONCE: this test isolates the recovery-reset on a SINGLE
            // stop-hook continuation. A block-every-time hook would now (post
            // #2 cap-counter) also block the final recovery-exhaustion end and
            // drive further continuations up to LINGXI_STOP_HOOK_BLOCK_CAP
            // (default 8), exhausting the scripted responses.
            exec_block_once_stop().await,
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        );

        let outcome = orch.run_turn("go").await.expect("turn ok");
        assert!(
            matches!(outcome, ConversationOutcome::EndTurn { .. }),
            "{outcome:?}"
        );
        assert_eq!(
            api.captured_msgs().await.len(),
            7,
            "the stop-hook continuation must reset the recovery budget (fresh 3 nudges ⇒ 7 API calls)"
        );
        let nudges = orch
            .session()
            .lock()
            .await
            .history
            .iter()
            .filter(|m| m.text_content() == MAX_OUTPUT_TOKENS_RECOVERY_NUDGE)
            .count();
        assert_eq!(
            nudges, 5,
            "5 recovery nudges expected across the two episodes (2 before + 3 after the reset)"
        );
    }

    // -------- FIX C — Stop hook_stopped_continuation meta message -----------

    #[tokio::test]
    async fn fix_c_stop_prevent_continuation_persists_stopped_message() {
        // Parity with claude-code `query/stopHooks.ts:269-280` — a Stop hook's
        // `continue:false` (preventContinuation) yields a
        // `hook_stopped_continuation` attachment (hookName `Stop`), rendered as
        // an isMeta `<system-reminder>\nStop hook stopped continuation:
        // {stopReason}\n</system-reminder>` user message before the turn
        // terminates. Script a single end_turn, then let the Stop hook prevent
        // continuation.
        let et = mock_message_response(
            vec![LlmContentBlock::Text {
                text: "done".into(),
                cache_control: None,
            }],
            Some("end_turn"),
        );
        let orch = ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![et])),
            Arc::new(ToolRegistry::new()),
            exec_prevent_stop(Some("STOP-CONTINUATION".into())).await,
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        );

        let outcome = orch.run_turn("go").await.expect("turn ok");
        assert!(
            matches!(outcome, ConversationOutcome::StopHookPrevented { .. }),
            "continue:false must terminate as StopHookPrevented, got {outcome:?}"
        );

        // The exact meta message is appended to live history. The transcript
        // persists only its typed attachment; cold resume derives this message
        // from that single source of truth.
        let session = orch.session();
        let s = session.lock().await;
        let found = s.history.iter().any(|m| {
            m.text_content()
                == "<system-reminder>\nStop hook stopped continuation: STOP-CONTINUATION\n</system-reminder>"
        });
        assert!(
            found,
            "the Stop hook_stopped_continuation meta message must be in history: {:#?}",
            s.history
                .iter()
                .map(protocol::ConversationMessage::text_content)
                .collect::<Vec<_>>()
        );
    }

    /// O2: the Stop hook's `preventContinuation` also PERSISTS a
    /// `hook_stopped_continuation` attachment line (BIN off 233101239), not
    /// just the meta message. `message` sits SECOND in key order and the
    /// `toolUseID` is the dispatch's `hook-{uuid}`, matching the
    /// `hook_additional_context` record the same Stop dispatch emits.
    #[tokio::test]
    async fn stop_prevent_continuation_persists_a_stopped_continuation_attachment() {
        let et = mock_message_response(
            vec![LlmContentBlock::Text {
                text: "done".into(),
                cache_control: None,
            }],
            Some("end_turn"),
        );
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("session.jsonl");
        let fs: Arc<dyn traits::FileSystem> = Arc::new(platform_posix::fs::PosixFileSystem::new(
            dir.path().to_path_buf(),
        ));
        let writer = Arc::new(session::jsonl::writer::JsonlWriter::new(path.clone(), fs));
        let orch = ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![et])),
            Arc::new(ToolRegistry::new()),
            exec_prevent_stop(Some("STOP-CONTINUATION".into())).await,
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            dir.path().to_path_buf(),
        )
        .with_jsonl_writer(writer);

        orch.run_turn("go").await.expect("turn ok");

        let raw = std::fs::read_to_string(&path).expect("read jsonl");
        let line = raw
            .lines()
            .find(|l| l.contains("hook_stopped_continuation"))
            .unwrap_or_else(|| panic!("no hook_stopped_continuation attachment line in: {raw}"));
        let v: serde_json::Value = serde_json::from_str(line).expect("json line");
        assert_eq!(v["type"], "attachment");
        let a = &v["attachment"];
        // Key ORDER is the contract — serde_json is pinned preserve_order.
        let rendered = serde_json::to_string(a).expect("attachment json");
        let tool_use_id = a["toolUseID"].as_str().expect("toolUseID").to_string();
        assert_eq!(
            rendered,
            format!(
                r#"{{"type":"hook_stopped_continuation","message":"STOP-CONTINUATION","hookName":"Stop","toolUseID":"{tool_use_id}","hookEvent":"Stop"}}"#
            )
        );
        assert!(
            tool_use_id.starts_with("hook-"),
            "Stop mints `hook-${{randomUUID()}}`, got {tool_use_id}"
        );
        let duplicate_rows = raw
            .lines()
            .filter_map(|row| serde_json::from_str::<serde_json::Value>(row).ok())
            .filter(|row| {
                row["type"] == "user"
                    && row["message"]["content"]
                        .as_str()
                        .is_some_and(|content| content.contains("STOP-CONTINUATION"))
            })
            .count();
        assert_eq!(
            duplicate_rows, 0,
            "the normalized meta message must not be persisted beside its attachment: {raw}"
        );
    }

    #[tokio::test]
    async fn fix_c_stop_prevent_continuation_default_reason() {
        // No `stopReason` → claude's default `'Stop hook prevented continuation'`
        // (`query/stopHooks.ts:271`).
        let et = mock_message_response(
            vec![LlmContentBlock::Text {
                text: "done".into(),
                cache_control: None,
            }],
            Some("end_turn"),
        );
        let orch = ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![et])),
            Arc::new(ToolRegistry::new()),
            exec_prevent_stop(None).await,
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        );

        orch.run_turn("go").await.expect("turn ok");

        let session = orch.session();
        let s = session.lock().await;
        let found = s.history.iter().any(|m| {
            m.text_content()
                == "<system-reminder>\nStop hook stopped continuation: Stop hook prevented continuation\n</system-reminder>"
        });
        assert!(found, "default stopReason must be used");
    }
}

// ============================================================================
// OUTSTYLE.3: per-turn, transient output-style reminder.
//
// Proves the byte-exact `<system-reminder>` meta user message is appended to
// EACH turn's OUTGOING model input when a non-default output style is active,
// on BOTH turn drivers (batched `run_turn` + streaming `run_turn_streaming`),
// and that it is NEVER persisted to `session.history` nor the JSONL transcript
// (transient — never accumulates). With the default style the outgoing message
// list is byte-identical (no extra message), keeping the locked parity fixtures
// green.
// ============================================================================
#[cfg(test)]
mod output_style_reminder_tests {
    use super::*;
    use crate::test_support::{
        content_block_start_text, content_block_stop, message_delta_stop, message_start,
        message_stop, mock_message_response, noop_hook_executor, text_delta, MockApiClient,
        MockOutputStream, MockStreamingApiClient, NoOpPermissionGate, StaticMemoryProvider,
    };
    use crate::OrchestratorConfig;
    use llm_client::ContentBlock as LlmContentBlock;
    use protocol::ContentBlock;
    use std::sync::Arc;
    use tool_api::registry::ToolRegistry;

    /// The byte-exact reminder text for the `Explanatory` builtin — 1:1 with TS
    /// `wrapInSystemReminder(`${name} output style is active. …`)`
    /// (`messages.ts:3097-3099` + `3805-3810`).
    const EXPLANATORY_REMINDER: &str = "<system-reminder>\nExplanatory output style is active. \
         Remember to follow the specific guidelines for this style.\n</system-reminder>";
    const LEARNING_REMINDER: &str = "<system-reminder>\nLearning output style is active. \
         Remember to follow the specific guidelines for this style.\n</system-reminder>";

    /// Config with a non-default builtin output style active.
    fn config_with_style(style: &str) -> OrchestratorConfig {
        OrchestratorConfig {
            output_style: Some(style.to_string()),
            ..OrchestratorConfig::default()
        }
    }

    /// Concatenated text of a message's text blocks (for substring checks).
    fn text_of(msg: &ConversationMessage) -> String {
        match msg {
            ConversationMessage::User { content, .. }
            | ConversationMessage::Assistant { content, .. } => content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(""),
            ConversationMessage::System { content, .. } => content.clone(),
        }
    }

    fn is_reminder(msg: &ConversationMessage, expected: &str) -> bool {
        matches!(msg, ConversationMessage::User { .. }) && text_of(msg) == expected
    }

    /// True when `msg` is the leading `additionalContext` (`# claudeMd` /
    /// `# userEmail` / `# currentDate`) meta message prepended each turn
    /// (R-P1c/R-P1d). With `StaticMemoryProvider::empty()` and no `user_email`
    /// it carries only the always-present `# currentDate` entry.
    fn is_additional_context(msg: &ConversationMessage) -> bool {
        matches!(msg, ConversationMessage::User { .. })
            && text_of(msg).starts_with(
                "<system-reminder>\nAs you answer the user's questions, you can use the following context:",
            )
    }

    // ----- direct unit coverage of the reminder builder -----

    #[tokio::test]
    async fn builder_emits_byte_exact_explanatory_reminder() {
        let orch = ConversationOrchestrator::new(
            config_with_style("Explanatory"),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        );
        let msg = orch
            .output_style_reminder_message()
            .await
            .expect("Explanatory resolves to a reminder");
        assert!(matches!(msg, ConversationMessage::User { .. }));
        assert_eq!(text_of(&msg), EXPLANATORY_REMINDER);
        // Spell out the literal bytes once so a drift in the helper const is caught.
        assert_eq!(
            text_of(&msg),
            "<system-reminder>\nExplanatory output style is active. Remember to follow the specific guidelines for this style.\n</system-reminder>"
        );
    }

    #[tokio::test]
    async fn builder_emits_byte_exact_learning_reminder() {
        let orch = ConversationOrchestrator::new(
            config_with_style("Learning"),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        );
        assert_eq!(
            text_of(
                &orch
                    .output_style_reminder_message()
                    .await
                    .expect("Learning resolves")
            ),
            LEARNING_REMINDER
        );
    }

    #[tokio::test]
    async fn builder_returns_none_for_default_and_unknown_styles() {
        for style in [None, Some("default"), Some(""), Some("Nonexistent")] {
            let cfg = OrchestratorConfig {
                output_style: style.map(str::to_string),
                ..OrchestratorConfig::default()
            };
            let orch = ConversationOrchestrator::new(
                cfg,
                Arc::new(MockApiClient::new(vec![])),
                Arc::new(ToolRegistry::new()),
                noop_hook_executor(),
                Arc::new(NoOpPermissionGate),
                Arc::new(MockOutputStream::new()),
                Arc::new(StaticMemoryProvider::empty()),
                std::env::temp_dir(),
            );
            assert!(
                orch.output_style_reminder_message().await.is_none(),
                "style {style:?} must not produce a reminder"
            );
        }
    }

    // ----- batched driver (`run_turn` / `execute_one_turn`) -----

    #[tokio::test]
    async fn batched_active_style_appends_transient_reminder_not_persisted() {
        let dir = tempfile::tempdir().expect("tempdir");
        let session_path = dir.path().join("session.jsonl");
        let fs: Arc<dyn traits::FileSystem> = Arc::new(platform_posix::fs::PosixFileSystem::new(
            dir.path().to_path_buf(),
        ));
        let writer = Arc::new(session::jsonl::writer::JsonlWriter::new(
            session_path.clone(),
            fs,
        ));

        let resp = mock_message_response(
            vec![LlmContentBlock::Text {
                text: "assistant body".into(),
                cache_control: None,
            }],
            Some("end_turn"),
        );
        let api = Arc::new(MockApiClient::new(vec![resp]));
        let orch = ConversationOrchestrator::new(
            config_with_style("Explanatory"),
            api.clone(),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            dir.path().to_path_buf(),
        )
        .with_jsonl_writer(writer);

        orch.run_turn("user prompt body").await.expect("turn");

        // OUTGOING snapshot: [additionalContext(meta), user(prompt), reminder] —
        // the leading additional-context meta message (R-P1c/d) prepends the
        // user prompt; the output-style reminder trails it (TS position).
        let outgoing = api.captured_msgs().await;
        assert_eq!(outgoing.len(), 1, "exactly one batched API call");
        let sent = &outgoing[0];
        assert_eq!(
            sent.len(),
            3,
            "additionalContext + prompt + reminder; got {sent:?}"
        );
        assert!(
            is_additional_context(&sent[0]),
            "leading meta; got {:?}",
            sent[0]
        );
        assert_eq!(text_of(&sent[1]), "user prompt body");
        assert!(
            is_reminder(&sent[2], EXPLANATORY_REMINDER),
            "trailing message must be the byte-exact reminder; got {:?}",
            sent[2]
        );

        // STORED history: [user(prompt), assistant] — the reminder was NOT pushed.
        let history = orch.session.lock().await.history.clone();
        assert_eq!(history.len(), 2, "user + assistant only; got {history:?}");
        assert!(
            history
                .iter()
                .all(|m| !is_reminder(m, EXPLANATORY_REMINDER)),
            "the reminder must never enter stored history; got {history:?}"
        );
        assert_eq!(text_of(&history[0]), "user prompt body");
        assert_eq!(text_of(&history[1]), "assistant body");

        // JSONL transcript: user + assistant only, reminder text absent.
        let on_disk = std::fs::read_to_string(&session_path).expect("read jsonl");
        assert!(on_disk.contains("user prompt body"));
        assert!(on_disk.contains("assistant body"));
        assert!(
            !on_disk.contains("output style is active"),
            "the reminder must never be persisted to JSONL; file:\n{on_disk}"
        );
    }

    #[tokio::test]
    async fn batched_default_style_sends_no_reminder() {
        let resp = mock_message_response(
            vec![LlmContentBlock::Text {
                text: "body".into(),
                cache_control: None,
            }],
            Some("end_turn"),
        );
        let api = Arc::new(MockApiClient::new(vec![resp]));
        let orch = ConversationOrchestrator::new(
            OrchestratorConfig::default(), // output_style: None
            api.clone(),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        );

        orch.run_turn("just the prompt").await.expect("turn");

        let outgoing = api.captured_msgs().await;
        assert_eq!(outgoing.len(), 1);
        // No output-style reminder; the only prepended message is the leading
        // additional-context meta (always present via `# currentDate`).
        assert_eq!(
            outgoing[0].len(),
            2,
            "additionalContext + prompt; got {:?}",
            outgoing[0]
        );
        assert!(
            is_additional_context(&outgoing[0][0]),
            "leading meta; got {:?}",
            outgoing[0][0]
        );
        assert_eq!(text_of(&outgoing[0][1]), "just the prompt");
        assert!(
            !outgoing[0]
                .iter()
                .any(|m| is_reminder(m, EXPLANATORY_REMINDER) || is_reminder(m, LEARNING_REMINDER)),
            "no output-style reminder on the default path; got {:?}",
            outgoing[0]
        );
    }

    // ----- streaming driver (`run_turn_streaming`) -----

    #[tokio::test]
    async fn streaming_active_style_appends_transient_reminder_not_persisted() {
        let dir = tempfile::tempdir().expect("tempdir");
        let session_path = dir.path().join("session.jsonl");
        let fs: Arc<dyn traits::FileSystem> = Arc::new(platform_posix::fs::PosixFileSystem::new(
            dir.path().to_path_buf(),
        ));
        let writer = Arc::new(session::jsonl::writer::JsonlWriter::new(
            session_path.clone(),
            fs,
        ));

        let streaming = Arc::new(MockStreamingApiClient::with_turns(vec![vec![
            message_start("m1", "claude-opus-4-7"),
            content_block_start_text(0),
            text_delta(0, "streamed body"),
            content_block_stop(0),
            message_delta_stop("end_turn"),
            message_stop(),
        ]]));
        let orch = ConversationOrchestrator::new_with_streaming(
            config_with_style("Learning"),
            Arc::new(MockApiClient::new(vec![])),
            streaming.clone(),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            dir.path().to_path_buf(),
        )
        .with_jsonl_writer(writer);

        orch.run_turn_streaming("streaming prompt")
            .await
            .expect("streaming turn");

        // OUTGOING snapshot to the stream: [additionalContext(meta), user(prompt),
        // output-style reminder, total_tokens reminder]. The total-tokens block
        // comes LAST, matching the oracle's fan-out order
        // (`…critical_system_reminder, silent_turn_reminder,
        // total_tokens_reminder`), and it is present because that reminder
        // defaults ON as it does upstream.
        let calls = streaming.captured_calls().await;
        assert_eq!(calls.len(), 1, "exactly one streaming call");
        let sent = &calls[0].messages;
        assert_eq!(
            sent.len(),
            4,
            "additionalContext + prompt + style reminder + total_tokens; got {sent:?}"
        );
        assert!(
            is_additional_context(&sent[0]),
            "leading meta; got {:?}",
            sent[0]
        );
        assert_eq!(text_of(&sent[1]), "streaming prompt");
        assert!(
            is_reminder(&sent[2], LEARNING_REMINDER),
            "the style reminder must be the byte-exact Learning reminder; got {:?}",
            sent[2]
        );
        assert!(
            text_of(&sent[3]).contains("<total_tokens>"),
            "total-tokens reminder trails the batch; got {:?}",
            sent[3]
        );

        // STORED history: reminder absent.
        let history = orch.session.lock().await.history.clone();
        assert!(
            history.iter().all(|m| !is_reminder(m, LEARNING_REMINDER)),
            "the reminder must never enter stored history; got {history:?}"
        );
        assert_eq!(text_of(&history[0]), "streaming prompt");

        // JSONL transcript: reminder text absent.
        let on_disk = std::fs::read_to_string(&session_path).expect("read jsonl");
        assert!(on_disk.contains("streaming prompt"));
        assert!(
            !on_disk.contains("output style is active"),
            "the reminder must never be persisted to JSONL; file:\n{on_disk}"
        );
    }

    #[tokio::test]
    async fn streaming_default_style_sends_no_reminder() {
        let streaming = Arc::new(MockStreamingApiClient::with_turns(vec![vec![
            message_start("m1", "claude-opus-4-7"),
            content_block_start_text(0),
            text_delta(0, "body"),
            content_block_stop(0),
            message_delta_stop("end_turn"),
            message_stop(),
        ]]));
        let orch = ConversationOrchestrator::new_with_streaming(
            OrchestratorConfig::default(), // output_style: None
            Arc::new(MockApiClient::new(vec![])),
            streaming.clone(),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        );

        orch.run_turn_streaming("only prompt")
            .await
            .expect("streaming turn");

        let calls = streaming.captured_calls().await;
        assert_eq!(calls.len(), 1);
        // No output-style reminder. The leading additional-context meta
        // (always present via `# currentDate`) prepends the prompt, and the
        // `total_tokens_reminder` trails it — that reminder defaults ON, as it
        // does in a stock Claude Code session, so it is part of every outgoing
        // list now. See `crate::prompt::total_tokens`.
        assert_eq!(
            calls[0].messages.len(),
            3,
            "additionalContext + prompt + total_tokens_reminder; got {:?}",
            calls[0].messages
        );
        assert!(
            is_additional_context(&calls[0].messages[0]),
            "leading meta; got {:?}",
            calls[0].messages[0]
        );
        assert_eq!(text_of(&calls[0].messages[1]), "only prompt");
        assert!(
            text_of(&calls[0].messages[2]).contains("<total_tokens>"),
            "trailing total-tokens reminder; got {:?}",
            calls[0].messages[2]
        );
    }
}

// ============================================================================
// R-P1c/R-P1d: the leading `additionalContext` (`# claudeMd` / `# userEmail` /
// `# currentDate`) meta message — byte-lock against claude-code `A6n`.
// ============================================================================
#[cfg(test)]
mod additional_context_tests {
    use super::*;
    use crate::prompt::MemoryFile;
    use crate::test_support::{
        mock_message_response, noop_hook_executor, MockApiClient, MockOutputStream,
        NoOpPermissionGate, StaticMemoryProvider,
    };
    use crate::OrchestratorConfig;
    use protocol::ContentBlock;
    use std::sync::Arc;
    use tool_api::registry::ToolRegistry;

    fn orch_with(
        memory: Arc<StaticMemoryProvider>,
        email: Option<&str>,
    ) -> ConversationOrchestrator {
        ConversationOrchestrator::new(
            OrchestratorConfig {
                user_email: email.map(str::to_string),
                ..OrchestratorConfig::default()
            },
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            memory,
            std::env::temp_dir(),
        )
    }

    fn text(msg: &ConversationMessage) -> String {
        match msg {
            ConversationMessage::User { content, .. } => content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect(),
            _ => String::new(),
        }
    }

    fn runtime_message(body: &str) -> ConversationMessage {
        ConversationMessage::user_meta(MessageId::new(), body.to_string())
    }

    fn mobile_environment_with_runtime(
        tool_runtime: traits::MobileToolRuntime,
        cwd: Option<&str>,
    ) -> traits::MobileRuntimeEnvironment {
        traits::MobileRuntimeEnvironment::new(
            traits::MobileHostEnvironment::new(
                traits::MobileHostOs::Ios,
                Some("19.0".into()),
                traits::MobileDeviceClass::Phone,
                traits::MobileExecutionTarget::PhysicalDevice,
                traits::MobileLaunchMode::Interactive,
            ),
            tool_runtime,
            cwd.map(str::to_string),
            Some("/bin/sh".into()),
            Some("Mobile Linux sh".into()),
            traits::MobileNetworkPolicy::PermissionMediated,
            traits::MobileLifecyclePolicy::IosFiniteBackgroundAssertion,
        )
    }

    fn mobile_environment(cwd: &str) -> traits::MobileRuntimeEnvironment {
        mobile_environment_with_runtime(traits::MobileToolRuntime::MobileLinuxGuest, Some(cwd))
    }

    #[tokio::test]
    async fn all_three_keys_byte_exact_order_and_wrapper() {
        // claudeMd + userEmail present; currentDate always present. Insertion
        // order (claude-code `pS`): claudeMd, userEmail, currentDate.
        let mem = Arc::new(StaticMemoryProvider::with_files(vec![MemoryFile {
            path: std::path::PathBuf::from("/proj/LINGXI.md"),
            body: "MD BODY".into(),
            is_local_override: false,
            tier: memory::lingxi_md::LingxiMdTier::Project,
            globs: None,
            raw_content: "MD BODY".into(),
            content_differs_from_disk: false,
        }]));
        let orch = orch_with(mem, Some("u@example.com"));
        let msg = orch.additional_context_message().await.expect("present");
        // It is a META user message (claude-code `isMeta:!0`).
        assert!(msg.is_meta(), "additionalContext must be isMeta");
        let body = text(&msg);

        // Exact wrapper: opens with the header line, closes with the IMPORTANT
        // line indented by 6 spaces + the closing tag + trailing LF.
        assert!(body.starts_with(
            "<system-reminder>\nAs you answer the user's questions, you can use the following context:\n"
        ));
        assert!(body.ends_with(
            "\n\n      IMPORTANT: this context may or may not be relevant to your tasks. \
You should not respond to this context unless it is highly relevant to your task.\n</system-reminder>\n"
        ));

        // Keys in order, each `# key\nvalue`, joined by `\n`.
        let i_md = body.find("# claudeMd\n").expect("claudeMd key");
        let i_email = body.find("# userEmail\n").expect("userEmail key");
        let i_date = body.find("# currentDate\n").expect("currentDate key");
        assert!(
            i_md < i_email && i_email < i_date,
            "key order claudeMd<userEmail<currentDate"
        );

        // claudeMd value = the assembled memory block (preamble + Contents).
        assert!(body.contains("# claudeMd\nCodebase and user instructions are shown below."));
        assert!(body.contains("Contents of /proj/LINGXI.md"));
        assert!(body.contains("MD BODY"));
        // userEmail value.
        assert!(body.contains("# userEmail\nThe user's email address is u@example.com."));
        // currentDate value (ISO local date).
        let today = crate::prompt::env_meta::current_date_string();
        assert!(body.contains(&format!("# currentDate\nToday's date is {today}.")));
    }

    #[tokio::test]
    async fn omits_lingxi_md_and_email_when_absent_keeps_date() {
        // Empty memory + no email → only `# currentDate` remains.
        let orch = orch_with(Arc::new(StaticMemoryProvider::empty()), None);
        let msg = orch
            .additional_context_message()
            .await
            .expect("date always present");
        let body = text(&msg);
        assert!(!body.contains("# claudeMd"));
        assert!(!body.contains("# userEmail"));
        assert!(body.contains("# currentDate\nToday's date is "));
        // The body between the header and the IMPORTANT line is exactly the one
        // currentDate entry (no stray blank lines from empty entries).
        let today = crate::prompt::env_meta::current_date_string();
        let expected = format!(
            "<system-reminder>\n\
As you answer the user's questions, you can use the following context:\n\
# currentDate\nToday's date is {today}.\n\
\n      IMPORTANT: this context may or may not be relevant to your tasks. \
You should not respond to this context unless it is highly relevant to your task.\n\
</system-reminder>\n"
        );
        assert_eq!(body, expected, "single-key wrapper byte-lock");
    }

    #[tokio::test]
    async fn empty_email_string_is_treated_as_absent() {
        // `user_email: Some("")` (or whitespace) is filtered, matching the
        // `...email&&{userEmail:…}` spread + LingXi's non-empty guard.
        let orch = orch_with(Arc::new(StaticMemoryProvider::empty()), Some("   "));
        let body = text(&orch.additional_context_message().await.expect("date"));
        assert!(!body.contains("# userEmail"));
    }

    #[tokio::test]
    async fn runtime_message_is_prepended_before_additional_context() {
        let mut orch = orch_with(Arc::new(StaticMemoryProvider::empty()), None);
        let runtime =
            "<system-reminder>\nMobile runtime environment (version 1)\n</system-reminder>";
        orch.mobile_runtime_environment_message = Some(runtime_message(runtime));
        orch.mobile_runtime_environment = Some(mobile_environment("/workspace/a"));
        let original = ConversationMessage::user(MessageId::new(), "hello".into());
        let mut messages = vec![original.clone()];

        orch.prepend_leading_context(&mut messages).await;

        assert_eq!(text(&messages[0]), runtime);
        assert!(text(&messages[1]).contains("Guest workspace: /workspace/a"));
        assert!(text(&messages[2]).contains("# currentDate\nToday's date is "));
        assert_eq!(messages[3], original);
        assert_eq!(
            orch.mobile_runtime_environment_preview().await.as_deref(),
            Some(runtime)
        );
    }

    #[tokio::test]
    async fn unresolved_native_workspace_path_falls_back_to_guest_coordinate() {
        let host_cwd = std::path::PathBuf::from("/tmp/native-host-worktree");
        let session_cwd = tool_api::SessionCwd::new(host_cwd.clone(), vec![host_cwd]);
        let orch = orch_with(Arc::new(StaticMemoryProvider::empty()), None)
            .with_session_cwd(session_cwd)
            .with_mobile_runtime_environment(mobile_environment("/workspace/a"))
            .with_mobile_workspace_cwd_resolver(Arc::new(|_| None));
        let mut messages = Vec::new();

        orch.prepend_leading_context(&mut messages).await;

        assert!(text(&messages[1]).contains("Guest workspace: /workspace/a"));
        assert!(!text(&messages[1]).contains("native-host-worktree"));
    }

    #[test]
    fn scheduled_mobile_runtime_uses_headless_prompt_guidance() {
        let mut orch = orch_with(Arc::new(StaticMemoryProvider::empty()), None);
        orch.config.interactive_session = true;
        assert!(orch.prompt_is_interactive());

        let mut environment = mobile_environment("/workspace/a");
        environment.host.launch_mode = traits::MobileLaunchMode::ScheduledHeadless;
        orch.mobile_runtime_environment = Some(environment);

        assert!(!orch.prompt_is_interactive());
    }

    #[tokio::test]
    async fn runtime_message_stays_first_when_transient_context_is_reattached() {
        let mut orch = orch_with(Arc::new(StaticMemoryProvider::empty()), None);
        let runtime = "<system-reminder>runtime</system-reminder>";
        let deferred = runtime_message("<system-reminder>deferred</system-reminder>");
        let date = runtime_message("<system-reminder>date</system-reminder>");
        let tail = runtime_message("<system-reminder>tail</system-reminder>");
        orch.mobile_runtime_environment_message = Some(runtime_message(runtime));
        orch.mobile_runtime_environment = Some(mobile_environment("/workspace/a"));
        let original = ConversationMessage::user(MessageId::new(), "hello".into());
        let mut messages = vec![original.clone()];

        orch.reattach_outgoing_context(
            &mut messages,
            Some(&deferred),
            Some(&date),
            std::slice::from_ref(&tail),
        )
        .await;

        assert_eq!(text(&messages[0]), runtime);
        assert!(text(&messages[1]).contains("Guest workspace: /workspace/a"));
        assert_eq!(text(&messages[2]), text(&date));
        assert_eq!(text(&messages[3]), text(&deferred));
        assert!(text(&messages[4]).contains("# currentDate\nToday's date is "));
        assert_eq!(messages[5], original);
        assert_eq!(messages[6], tail);
    }

    #[tokio::test]
    async fn runtime_message_keeps_dynamic_environment_separate() {
        let mut orch = ConversationOrchestrator::new(
            OrchestratorConfig {
                exclude_dynamic_system_prompt_sections: true,
                ..OrchestratorConfig::default()
            },
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        );
        orch.mobile_runtime_environment_message = Some(runtime_message(
            "<system-reminder>\nMobile runtime environment (version 1)\n</system-reminder>",
        ));
        orch.mobile_runtime_environment = Some(mobile_environment("/workspace/a"));

        let body = text(&orch.additional_context_message().await.expect("date"));
        assert!(body.contains("# Environment\n"));
        assert!(body.contains("# currentDate\nToday's date is "));
    }

    #[tokio::test]
    async fn non_guest_mobile_runtime_keeps_environment_re_emission_when_excluded() {
        let mut orch = ConversationOrchestrator::new(
            OrchestratorConfig {
                exclude_dynamic_system_prompt_sections: true,
                ..OrchestratorConfig::default()
            },
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        );
        orch.mobile_runtime_environment = Some(mobile_environment_with_runtime(
            traits::MobileToolRuntime::AndroidLegacy,
            None,
        ));

        let body = text(&orch.additional_context_message().await.expect("date"));
        assert!(body.contains("# Environment\n"));
        assert!(body.contains("# currentDate\nToday's date is "));
    }

    #[tokio::test]
    async fn system_prompt_override_stays_verbatim_while_runtime_message_is_sent() {
        let api = Arc::new(MockApiClient::new(vec![mock_message_response(
            vec![llm_client::ContentBlock::Text {
                text: "ok".into(),
                cache_control: None,
            }],
            Some("end_turn"),
        )]));
        let mut orch = ConversationOrchestrator::new(
            OrchestratorConfig {
                system_prompt_override: Some("CUSTOM PROMPT — no assembler".into()),
                ..OrchestratorConfig::default()
            },
            api.clone(),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        );
        let runtime =
            "<system-reminder>\nMobile runtime environment (version 1)\n</system-reminder>";
        orch.mobile_runtime_environment_message = Some(runtime_message(runtime));
        orch.mobile_runtime_environment = Some(mobile_environment("/workspace/a"));

        orch.run_turn("hi").await.expect("turn");

        assert_eq!(
            api.captured_systems().await,
            vec![Some("CUSTOM PROMPT — no assembler".into())]
        );
        let sent = api.captured_msgs().await;
        assert_eq!(text(&sent[0][0]), runtime);
        assert!(text(&sent[0][1]).contains("Guest workspace: /workspace/a"));
        assert!(text(&sent[0][2]).contains("# currentDate\nToday's date is "));
    }

    // ------------------------------------------------------------------------
    // `date_change` (cc `Cop`): mid-session midnight crossing.
    // ------------------------------------------------------------------------

    /// Rewind the memoized session-start date so the live local date always
    /// differs — the "session started yesterday" setup.
    fn seed_stale_session_date(orch: &ConversationOrchestrator, session_id: protocol::SessionId) {
        let mut state = orch
            .date_change
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.session_id = Some(session_id);
        state.session_date = "2000-01-01".to_string();
        state.delivered_date = None;
    }

    fn expected_date_change_body() -> String {
        let today = crate::prompt::env_meta::current_date_string();
        format!(
            "<system-reminder>\nThe date has changed. Today's date is now {today}. \
No need to announce the new date \u{2014} the user's own clock shows it.\n</system-reminder>"
        )
    }

    #[tokio::test]
    async fn additional_context_keeps_the_session_start_date_after_rollover() {
        let orch = orch_with(Arc::new(StaticMemoryProvider::empty()), None);
        let sid = orch.session.lock().await.session_id;
        seed_stale_session_date(&orch, sid);

        let body = text(
            &orch
                .additional_context_message()
                .await
                .expect("date context"),
        );
        assert!(body.contains("# currentDate\nToday's date is 2000-01-01."));
        assert!(
            orch.date_change_reminder_message(sid).is_some(),
            "rollover is announced only by date_change"
        );
    }

    #[test]
    fn date_change_none_when_date_unchanged() {
        // First producer run seeds the session-start memo (`LGe = Vr(wcs)`), so
        // a same-day session NEVER emits — the locked fixtures stay identical.
        let orch = orch_with(Arc::new(StaticMemoryProvider::empty()), None);
        let sid = protocol::SessionId::new();
        assert!(orch.date_change_reminder_message(sid).is_none());
        assert!(orch.date_change_reminder_message(sid).is_none());
    }

    #[test]
    fn date_change_emits_once_after_midnight() {
        let orch = orch_with(Arc::new(StaticMemoryProvider::empty()), None);
        let sid = protocol::SessionId::new();
        seed_stale_session_date(&orch, sid);
        let msg = orch
            .date_change_reminder_message(sid)
            .expect("date differs from session start");
        // Byte-exact reminder (renderer @238108493) inside the `Ww` wrap.
        assert_eq!(text(&msg), expected_date_change_body());
        // Meta user message (`zr({…, isMeta:!0})`).
        assert!(matches!(
            msg,
            ConversationMessage::User { is_meta: true, .. }
        ));
        // The producer is PURE: without a commit the SAME reminder is still due,
        // so a step that never reaches the model cannot swallow it.
        assert!(orch.date_change_reminder_message(sid).is_some());
        orch.commit_date_change_reminder();
        // Dedupe: once delivered, the following turn (same date) emits nothing.
        assert!(orch.date_change_reminder_message(sid).is_none());
    }

    #[test]
    fn date_change_stays_deduped_after_a_compact_boundary() {
        // Compaction must not reset the session-level reminder. The leading
        // `currentDate` remains the session-start memo and the changed date was
        // already delivered once.
        let orch = orch_with(Arc::new(StaticMemoryProvider::empty()), None);
        let sid = protocol::SessionId::new();
        seed_stale_session_date(&orch, sid);
        assert!(orch.date_change_reminder_message(sid).is_some());
        orch.commit_date_change_reminder();
        assert!(orch.date_change_reminder_message(sid).is_none());
        assert!(orch.date_change_reminder_message(sid).is_none());
    }

    #[test]
    fn date_change_re_seeds_the_session_start_date_on_a_new_session() {
        // `clearSessionCaches` clears BOTH `LGe`'s memo and the emitted date, so
        // a `/clear` (fresh `SessionId`) or in-place resume (adopted id) must
        // NOT fire a reminder into the brand-new conversation.
        let orch = orch_with(Arc::new(StaticMemoryProvider::empty()), None);
        let old = protocol::SessionId::new();
        seed_stale_session_date(&orch, old);
        assert!(orch.date_change_reminder_message(old).is_some());

        let fresh = protocol::SessionId::new();
        assert!(
            orch.date_change_reminder_message(fresh).is_none(),
            "a new session re-seeds the start date to today"
        );
    }
}

// ============================================================================
// SKILLEXEC.3 (model scope): a tool's `context_modifier` switches the session's
// main-loop model, applied POST-BATCH on BOTH turn drivers (batched `run_turn`
// + streaming `run_turn_streaming`). A tool that returns NO modifier (every
// existing tool + skills WITHOUT a `model:` frontmatter) leaves `session.model`
// untouched — byte-identical, keeping the locked turn-loop/streaming fixtures
// green.
// ============================================================================
#[cfg(test)]
mod skill_model_override_tests {
    use super::*;
    use crate::test_support::{
        content_block_start_text, content_block_start_tool_use, content_block_stop,
        input_json_delta, message_delta_stop, message_start, message_stop, mock_message_response,
        noop_hook_executor, text_delta, MockApiClient, MockOutputStream, MockStreamingApiClient,
        NoOpPermissionGate, StaticMemoryProvider,
    };
    use crate::OrchestratorConfig;
    use llm_client::ContentBlock as LlmContentBlock;
    use protocol::ToolUseId;
    use std::sync::Arc;
    use tool_api::context::ToolUseContext;
    use tool_api::progress::ToolProgressSender;
    use tool_api::registry::ToolRegistry;
    use tool_api::tool_trait::{
        ContextModifier, DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError,
        ToolStaticContext, ValidationError,
    };

    /// The model the [`ModelSwitchTool`] switches the session to.
    const SWITCHED_MODEL: &str = "claude-opus-4-9-zzz";

    /// A tool that succeeds AND returns a `context_modifier` setting the turn's
    /// `main_loop_model` to [`SWITCHED_MODEL`] — the orchestrator-side twin of a
    /// Skill tool with a `model:` frontmatter. Sets the model directly (the
    /// skill-specific `[1m]`-resolution logic is unit-tested in the skill crate).
    struct ModelSwitchTool;
    #[async_trait]
    impl Tool for ModelSwitchTool {
        fn name(&self) -> &str {
            "ModelSwitch"
        }
        fn input_schema(&self) -> &serde_json::Value {
            static SCHEMA: once_cell::sync::Lazy<serde_json::Value> = once_cell::sync::Lazy::new(
                || serde_json::json!({ "type": "object", "properties": {} }),
            );
            &SCHEMA
        }
        fn is_enabled(&self, _ctx: &ToolStaticContext) -> bool {
            true
        }
        fn max_result_size_chars(&self) -> usize {
            1024 * 1024
        }
        fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
            true
        }
        fn is_read_only(&self, _input: &serde_json::Value) -> bool {
            true
        }
        async fn validate_input(
            &self,
            _input: &serde_json::Value,
            _ctx: &ToolUseContext,
        ) -> Result<(), ValidationError> {
            Ok(())
        }
        async fn check_permissions(
            &self,
            _input: &serde_json::Value,
            _ctx: &ToolUseContext,
        ) -> permission::PermissionResult {
            permission::PermissionResult::Allow {
                reason: permission::PermissionDecisionReason::Other {
                    reason: "test".into(),
                },
                updated_input: None,
                update_destination: None,
                metadata: permission::result::PermissionMetadata::default(),
            }
        }
        async fn description(
            &self,
            _input: &serde_json::Value,
            _opts: &DescriptionOptions,
        ) -> String {
            "model-switch".into()
        }
        async fn prompt(&self, _opts: &PromptOptions) -> String {
            String::new()
        }
        async fn call(
            &self,
            _input: serde_json::Value,
            _ctx: ToolUseContext,
            _tx: ToolProgressSender,
        ) -> Result<ToolCallResult, ToolError> {
            let modifier: ContextModifier = Box::new(|mut ctx: ToolUseContext| {
                ctx.options.main_loop_model = SWITCHED_MODEL.to_string();
                ctx
            });
            Ok(ToolCallResult {
                data: serde_json::json!({
                    "content": "TOOL-RESULT",
                    "model_content": "Launching skill: switcher",
                }),
                model_content: None,
                new_messages: vec![],
                context_modifier: Some(modifier),
                is_error: false,
                mcp_meta: None,
            })
        }
    }

    /// A tool with NO `context_modifier` (the byte-identical baseline — like
    /// every existing tool).
    struct PlainTool;
    #[async_trait]
    impl Tool for PlainTool {
        fn name(&self) -> &str {
            "Plain"
        }
        fn input_schema(&self) -> &serde_json::Value {
            static SCHEMA: once_cell::sync::Lazy<serde_json::Value> = once_cell::sync::Lazy::new(
                || serde_json::json!({ "type": "object", "properties": {} }),
            );
            &SCHEMA
        }
        fn is_enabled(&self, _ctx: &ToolStaticContext) -> bool {
            true
        }
        fn max_result_size_chars(&self) -> usize {
            1024 * 1024
        }
        fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
            true
        }
        fn is_read_only(&self, _input: &serde_json::Value) -> bool {
            true
        }
        async fn validate_input(
            &self,
            _input: &serde_json::Value,
            _ctx: &ToolUseContext,
        ) -> Result<(), ValidationError> {
            Ok(())
        }
        async fn check_permissions(
            &self,
            _input: &serde_json::Value,
            _ctx: &ToolUseContext,
        ) -> permission::PermissionResult {
            permission::PermissionResult::Allow {
                reason: permission::PermissionDecisionReason::Other {
                    reason: "test".into(),
                },
                updated_input: None,
                update_destination: None,
                metadata: permission::result::PermissionMetadata::default(),
            }
        }
        async fn description(
            &self,
            _input: &serde_json::Value,
            _opts: &DescriptionOptions,
        ) -> String {
            "plain".into()
        }
        async fn prompt(&self, _opts: &PromptOptions) -> String {
            String::new()
        }
        async fn call(
            &self,
            _input: serde_json::Value,
            _ctx: ToolUseContext,
            _tx: ToolProgressSender,
        ) -> Result<ToolCallResult, ToolError> {
            Ok(ToolCallResult {
                data: serde_json::json!({ "content": "PLAIN-RESULT" }),
                model_content: None,
                new_messages: vec![],
                context_modifier: None,
                is_error: false,
                mcp_meta: None,
            })
        }
    }

    fn registry_with(tool: Arc<dyn Tool>) -> Arc<ToolRegistry> {
        let mut reg = ToolRegistry::new();
        reg.register_builtin(tool);
        Arc::new(reg)
    }

    // ----- batched driver (`run_turn`) -----

    #[tokio::test]
    async fn batched_skill_model_override_switches_session_model() {
        let tu = ToolUseId::new();
        let resp1 = mock_message_response(
            vec![LlmContentBlock::ToolCall {
                id: tu.to_string(),
                name: "ModelSwitch".into(),
                input: serde_json::json!({}),
            }],
            Some("tool_use"),
        );
        let resp2 = mock_message_response(
            vec![LlmContentBlock::Text {
                text: "done".into(),
                cache_control: None,
            }],
            Some("end_turn"),
        );
        let orch = ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![resp1, resp2])),
            registry_with(Arc::new(ModelSwitchTool)),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        );
        // Precondition: the session boots on the default model.
        assert_eq!(
            orch.session.lock().await.model,
            crate::config::DEFAULT_MODEL
        );

        orch.run_turn("switch please").await.expect("turn");

        // POST-BATCH the override took effect; the NEXT turn's API call reads it.
        assert_eq!(orch.session.lock().await.model, SWITCHED_MODEL);
    }

    #[tokio::test]
    async fn batched_no_modifier_leaves_session_model_untouched() {
        // Byte-identical guard: a tool with NO context_modifier must not move
        // `session.model`.
        let tu = ToolUseId::new();
        let resp1 = mock_message_response(
            vec![LlmContentBlock::ToolCall {
                id: tu.to_string(),
                name: "Plain".into(),
                input: serde_json::json!({}),
            }],
            Some("tool_use"),
        );
        let resp2 = mock_message_response(
            vec![LlmContentBlock::Text {
                text: "done".into(),
                cache_control: None,
            }],
            Some("end_turn"),
        );
        let orch = ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![resp1, resp2])),
            registry_with(Arc::new(PlainTool)),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        );
        orch.run_turn("no switch").await.expect("turn");
        assert_eq!(
            orch.session.lock().await.model,
            crate::config::DEFAULT_MODEL,
            "no context_modifier → session.model unchanged (byte-identical)"
        );
    }

    // ----- streaming driver (`run_turn_streaming`) -----

    #[tokio::test]
    async fn streaming_skill_model_override_switches_session_and_next_call() {
        let tu = ToolUseId::new();
        let turn1 = vec![
            message_start("m1", crate::config::DEFAULT_MODEL),
            content_block_start_tool_use(0, tu.clone(), "ModelSwitch"),
            input_json_delta(0, "{}"),
            content_block_stop(0),
            message_delta_stop("tool_use"),
            message_stop(),
        ];
        let turn2 = vec![
            message_start("m2", SWITCHED_MODEL),
            content_block_start_text(0),
            text_delta(0, "done"),
            content_block_stop(0),
            message_delta_stop("end_turn"),
            message_stop(),
        ];
        let streaming = Arc::new(MockStreamingApiClient::with_turns(vec![turn1, turn2]));
        let orch = ConversationOrchestrator::new_with_streaming(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            streaming.clone(),
            registry_with(Arc::new(ModelSwitchTool)),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        );

        orch.run_turn_streaming("switch please")
            .await
            .expect("streaming turn");

        // session.model switched POST-BATCH...
        assert_eq!(orch.session.lock().await.model, SWITCHED_MODEL);
        // ...and the NEXT (second) streaming call used the switched model, while
        // the first used the boot default.
        let calls = streaming.captured_calls().await;
        assert_eq!(calls.len(), 2, "two streaming calls (tool turn + end turn)");
        assert_eq!(calls[0].model, crate::config::DEFAULT_MODEL);
        assert_eq!(
            calls[1].model, SWITCHED_MODEL,
            "the NEXT API call must use the switched model"
        );
    }

    #[tokio::test]
    async fn streaming_no_modifier_leaves_session_model_untouched() {
        // Byte-identical guard on the streaming path.
        let tu = ToolUseId::new();
        let turn1 = vec![
            message_start("m1", crate::config::DEFAULT_MODEL),
            content_block_start_tool_use(0, tu.clone(), "Plain"),
            input_json_delta(0, "{}"),
            content_block_stop(0),
            message_delta_stop("tool_use"),
            message_stop(),
        ];
        let turn2 = vec![
            message_start("m2", crate::config::DEFAULT_MODEL),
            content_block_start_text(0),
            text_delta(0, "done"),
            content_block_stop(0),
            message_delta_stop("end_turn"),
            message_stop(),
        ];
        let streaming = Arc::new(MockStreamingApiClient::with_turns(vec![turn1, turn2]));
        let orch = ConversationOrchestrator::new_with_streaming(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            streaming.clone(),
            registry_with(Arc::new(PlainTool)),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        );
        orch.run_turn_streaming("no switch")
            .await
            .expect("streaming turn");
        assert_eq!(
            orch.session.lock().await.model,
            crate::config::DEFAULT_MODEL,
            "no context_modifier → session.model unchanged on the streaming path"
        );
        let calls = streaming.captured_calls().await;
        assert!(
            calls
                .iter()
                .all(|c| c.model == crate::config::DEFAULT_MODEL),
            "every streaming call used the unchanged default model"
        );
    }

    /// Regression guard for the streaming-profile gap: when `session.model_profile`
    /// is set (e.g. `"github-copilot"`) the INITIAL streaming `.stream()` call
    /// must carry the profile, not `None`.  Mirrors the batched
    /// `build_request_sets_profile_when_provided` test in `provider_adapter.rs`.
    #[tokio::test]
    async fn streaming_threads_model_profile_to_stream_call() {
        let turn = vec![
            message_start("m1", crate::config::DEFAULT_MODEL),
            content_block_start_text(0),
            text_delta(0, "hello"),
            content_block_stop(0),
            message_delta_stop("end_turn"),
            message_stop(),
        ];
        let streaming = Arc::new(MockStreamingApiClient::with_turns(vec![turn]));
        let orch = ConversationOrchestrator::new_with_streaming(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            streaming.clone(),
            registry_with(Arc::new(PlainTool)),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        );
        // Set model_profile on the session directly (mirrors what switch_model does).
        {
            let mut s = orch.session.lock().await;
            s.model_profile = Some("github-copilot".to_string());
        }

        orch.run_turn_streaming("hello")
            .await
            .expect("streaming turn");

        let calls = streaming.captured_calls().await;
        assert_eq!(calls.len(), 1, "one streaming call");
        assert_eq!(
            calls[0].profile.as_deref(),
            Some("github-copilot"),
            "streaming path must thread session.model_profile through to the stream() call"
        );
    }
}

// ============================================================================
// Task 7: mid-stream 529 → non-streaming fallback tests
//
// Parity: `claude.ts:2469-2594`, `withRetry.ts:141,186`
// Env gate: `LINGXI_DISABLE_NONSTREAMING_FALLBACK` (claude.ts:2470)
// Error copy: `errors.ts:166` REPEATED_529_ERROR_MESSAGE = "Repeated 529 Overloaded errors"
// ============================================================================
#[cfg(test)]
mod task7_midstream_fallback_tests {
    use super::*;
    use crate::test_support::{
        content_block_start_text, message_start, mock_message_response, noop_hook_executor,
        text_delta, MockApiClient, MockOutputStream, MockStreamingApiClient, NoOpPermissionGate,
        StaticMemoryProvider,
    };
    use crate::OrchestratorConfig;
    use llm_client::ContentBlock as LlmContentBlock;
    use std::sync::Arc;
    use tool_api::registry::ToolRegistry;

    const DISABLE_FALLBACK_ENV: &str = "LINGXI_DISABLE_NONSTREAMING_FALLBACK";

    /// Serializes the two midstream tests that read/write `DISABLE_FALLBACK_ENV`.
    ///
    /// `std::env::set_var` / `remove_var` are not thread-safe when other threads
    /// read the same variable concurrently.  Tokio runs `#[tokio::test]` functions
    /// in the same process and may schedule them in parallel; holding this lock for
    /// the duration of each test makes the pair race-free without any new crate dep.
    static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// Build a one-ContentBlockStart-then-Err(Overloaded) stream: the first
    /// event is yielded successfully (proving partial events arrived), then the
    /// stream errors with `LlmError::Overloaded`.
    fn one_event_then_overloaded() -> Vec<Result<llm_client::LlmEvent, llm_client::LlmError>> {
        vec![
            Ok(message_start("m1", "claude-opus-4-7")),
            Ok(content_block_start_text(0)),
            Ok(text_delta(0, "partial")),
            Err(llm_client::LlmError::Overloaded { repeated: false }),
        ]
    }

    /// Build an `end_turn` non-streaming response for the fallback.
    fn fallback_response() -> llm_client::LlmResponse {
        mock_message_response(
            vec![LlmContentBlock::Text {
                text: "fallback body".into(),
                cache_control: None,
            }],
            Some("end_turn"),
        )
    }

    /// Task 7 Step 1 (a)(b)(c)(d):
    /// A stream that yields one ContentBlockStart then Err(Overloaded):
    /// (a) the stream is NOT replayed (streaming_api called exactly once),
    /// (b) a fresh non-streaming `messages_create_seeded` is issued,
    /// (c) the seed is 1 (streaming 529 counts toward the budget),
    /// (d) the final response is built from the non-streaming reply only.
    ///
    /// Parity: claude.ts:2551-2594, withRetry.ts:186
    /// (`initialConsecutive529Errors: is529Error(streamingError) ? 1 : 0`)
    #[tokio::test]
    async fn midstream_529_triggers_nonstreaming_fallback() {
        // Serialize with the sibling test that also reads/writes DISABLE_FALLBACK_ENV.
        // `set_var`/`remove_var` are not thread-safe; the (tokio) mutex makes the
        // pair race-free without a new crate dependency, and its guard is safe to
        // hold across the .await points below.
        let _guard = ENV_LOCK.lock().await;
        // Ensure fallback is ENABLED for this test.
        std::env::remove_var(DISABLE_FALLBACK_ENV);

        let streaming = Arc::new(MockStreamingApiClient::with_fallible_turns(vec![
            one_event_then_overloaded(),
        ]));
        let api = Arc::new(MockApiClient::new(vec![fallback_response()]));
        let output = Arc::new(MockOutputStream::new());
        let orch = ConversationOrchestrator::new_with_streaming(
            OrchestratorConfig::default(),
            api.clone(),
            streaming.clone(),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            output.clone(),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        );

        let outcome = orch
            .run_turn_streaming("hello")
            .await
            .expect("turn must succeed via fallback");

        // (a) Stream was called exactly once — NOT replayed.
        let stream_calls = streaming.captured_calls().await;
        assert_eq!(
            stream_calls.len(),
            1,
            "(a) stream must be called exactly once"
        );

        // (b) A fresh non-streaming messages_create_seeded was called.
        let seeds = api.captured_seeds().await;
        assert_eq!(
            seeds.len(),
            1,
            "(b) messages_create_seeded must be called exactly once"
        );

        // (c) The seed is 1 (the streaming 529 counts toward the consecutive 529 budget).
        assert_eq!(
            seeds[0], 1,
            "(c) seed must be 1 for a streaming Overloaded error"
        );

        // (d) The final turn outcome is built from the non-streaming reply only.
        // The output must contain "fallback body" (from the non-streaming response),
        // NOT just "partial" (the partial stream events are discarded).
        let events = output.snapshot().await;
        let texts: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                traits::OutputEvent::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            texts.contains(&"fallback body"),
            "(d) output must contain the fallback body; texts={texts:?}"
        );
        // The turn must have ended with end_turn (not an error).
        assert!(
            matches!(outcome, ConversationOutcome::EndTurn { .. }),
            "outcome must be EndTurn after non-streaming fallback; got {outcome:?}"
        );

        // M1 (Task 7 review): the PERSISTED assistant message must contain ONLY the
        // fallback body, not the partial streaming fragments.  TS yields deltas live
        // (claude.ts:2210 `yield m` fires inside the for-await loop at each
        // `content_block_stop`), so partial output reaching callers before the fallback
        // is parity — but the final persisted turn must reflect ONLY the fallback result.
        let session_arc = orch.session();
        let session_guard = session_arc.lock().await;
        let final_assistant = session_guard
            .history
            .iter()
            .filter_map(|msg| match msg {
                ConversationMessage::Assistant { content, .. } => Some(content),
                _ => None,
            })
            .last()
            .expect("session must contain at least one assistant message");
        let persisted_texts: Vec<&str> = final_assistant
            .iter()
            .filter_map(|blk| match blk {
                protocol::ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            persisted_texts,
            vec!["fallback body"],
            "M1: final persisted assistant message must contain ONLY the fallback body; \
             got {persisted_texts:?}"
        );
    }

    /// Task 7 Step 1 (twin with LINGXI_DISABLE_NONSTREAMING_FALLBACK=1):
    /// When the env gate is set, the streaming error propagates instead of
    /// triggering the non-streaming fallback.
    ///
    /// Parity: claude.ts:2476-2501 (disableFallback branch).
    #[tokio::test]
    async fn midstream_529_propagates_when_fallback_disabled() {
        // Serialize with the sibling test that also reads/writes DISABLE_FALLBACK_ENV.
        // `set_var`/`remove_var` are not thread-safe; the (tokio) mutex makes the
        // pair race-free without a new crate dependency, and its guard is safe to
        // hold across the .await points below.
        let _guard = ENV_LOCK.lock().await;
        // Set the disable flag for this test.
        std::env::set_var(DISABLE_FALLBACK_ENV, "1");

        let streaming = Arc::new(MockStreamingApiClient::with_fallible_turns(vec![
            one_event_then_overloaded(),
        ]));
        let api = Arc::new(MockApiClient::new(vec![]));
        let orch = ConversationOrchestrator::new_with_streaming(
            OrchestratorConfig::default(),
            api.clone(),
            streaming.clone(),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        );

        let result = orch.run_turn_streaming("hello").await;

        // Restore env BEFORE assertions to avoid leaking even on panic.
        std::env::remove_var(DISABLE_FALLBACK_ENV);

        // The error MUST propagate — no fallback.
        assert!(
            result.is_err(),
            "error must propagate when fallback is disabled"
        );
        // No non-streaming call was made.
        assert!(
            api.captured_seeds().await.is_empty(),
            "messages_create_seeded must NOT be called when fallback is disabled"
        );
        // The stream was called exactly once.
        assert_eq!(
            streaming.captured_calls().await.len(),
            1,
            "stream was called exactly once"
        );
    }

    /// `is_env_truthy` covers the exact semantics of TS `isEnvTruthy`
    /// (`utils/envUtils.ts:32`): truthy ONLY for the whitelist
    /// `1`/`true`/`yes`/`on`, case-insensitive and trimmed; everything
    /// else (including `no`/`off`/`2`/`enabled`/arbitrary strings) is
    /// falsy.
    #[test]
    fn is_env_truthy_matches_ts_semantics() {
        // Not set → not truthy.
        assert!(!is_env_truthy(None));
        // Empty → not truthy.
        assert!(!is_env_truthy(Some("")));
        // "false" → not truthy.
        assert!(!is_env_truthy(Some("false")));
        // "0" → not truthy.
        assert!(!is_env_truthy(Some("0")));
        // Whitelist members → truthy.
        assert!(is_env_truthy(Some("1")));
        assert!(is_env_truthy(Some("true")));
        assert!(is_env_truthy(Some("yes")));
        assert!(is_env_truthy(Some("on")));
        // Case-insensitive + trimmed.
        assert!(is_env_truthy(Some("ON")));
        assert!(is_env_truthy(Some(" TRUE ")));
        assert!(is_env_truthy(Some("Yes")));
        // Non-whitelist values → NOT truthy (strict whitelist).
        assert!(!is_env_truthy(Some("no")));
        assert!(!is_env_truthy(Some("off")));
        assert!(!is_env_truthy(Some("2")));
        assert!(!is_env_truthy(Some("enabled")));
        assert!(!is_env_truthy(Some("disable")));
        assert!(!is_env_truthy(Some("random")));
    }
}

/// Task 6 (llm-client future-work batch 5): the terminal-429 limits-copy
/// re-map (`enrich_rate_limited_error`). The integration test
/// (`tests/rate_limit_terminal_429_test.rs`) drives the batched `ApiCall`
/// wrapper end-to-end; these cover the `Streaming` wrapper and the
/// pass-through arms directly.
#[cfg(test)]
mod enrich_rate_limited_error_tests {
    use super::*;

    fn rate_limited() -> LlmError {
        LlmError::RateLimited {
            retry_after: None,
            scope: None,
        }
    }

    /// A streaming connect-phase 429 (the wrapper `try_run_turn_streaming`
    /// produces) re-maps onto the composed copy too.
    #[test]
    fn streaming_429_with_copy_maps_to_rate_limit_rejected() {
        let err = enrich_rate_limited_error(
            OrchestratorError::Streaming(rate_limited()),
            Some("You've hit your weekly limit · resets 3pm".to_string()),
        );
        assert!(
            matches!(err, OrchestratorError::RateLimitRejected { .. }),
            "got {err:?}"
        );
        assert_eq!(err.to_string(), "You've hit your weekly limit · resets 3pm");
    }

    /// No composed copy → both wrappers pass through untouched.
    #[test]
    fn rate_limited_without_copy_passes_through() {
        let api = enrich_rate_limited_error(OrchestratorError::ApiCall(rate_limited()), None);
        assert!(matches!(
            api,
            OrchestratorError::ApiCall(LlmError::RateLimited { .. })
        ));
        let stream = enrich_rate_limited_error(OrchestratorError::Streaming(rate_limited()), None);
        assert!(matches!(
            stream,
            OrchestratorError::Streaming(LlmError::RateLimited { .. })
        ));
    }

    /// A non-429 error never consults the copy — even when one is cached.
    #[test]
    fn non_rate_limited_ignores_copy() {
        let err = enrich_rate_limited_error(
            OrchestratorError::StreamEndedWithoutStop,
            Some("You've hit your weekly limit".to_string()),
        );
        assert!(matches!(err, OrchestratorError::StreamEndedWithoutStop));
    }
}

// ============================================================================
// SKILLLIST.1: per-turn, transient `skill_listing` reminder.
//
// Proves the orchestrator method: returns the rendered `<system-reminder>` when
// a provider is wired AND the `Skill` tool is present this turn; returns `None`
// when no provider is wired, or when the `Skill` tool is absent (so we never
// advertise skills the model can't invoke). The byte-level formatting is covered
// in `prompt::skill_listing::tests`.
// ============================================================================
#[cfg(test)]
mod skill_listing_reminder_tests {
    use super::*;
    use crate::prompt::skill_listing::{SkillListingEntry, SkillListingProvider};
    use crate::test_support::{
        noop_hook_executor, MockApiClient, MockOutputStream, NoOpPermissionGate,
        StaticMemoryProvider,
    };
    use crate::OrchestratorConfig;
    use std::sync::Arc;
    use tool_api::context::ToolUseContext;
    use tool_api::progress::ToolProgressSender;
    use tool_api::registry::ToolRegistry;
    use tool_api::tool_trait::{
        DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolStaticContext,
        ValidationError,
    };

    /// Static skill-listing fixture.
    struct FixtureSkills(Vec<SkillListingEntry>);
    #[async_trait]
    impl SkillListingProvider for FixtureSkills {
        async fn skill_entries(&self) -> Vec<SkillListingEntry> {
            self.0.clone()
        }
    }

    /// Minimal tool whose only meaningful behavior is its name — used to put a
    /// `Skill`-named tool (or not) into the registry for the gate test.
    struct NamedTool(&'static str);
    #[async_trait]
    impl Tool for NamedTool {
        fn name(&self) -> &str {
            self.0
        }
        fn input_schema(&self) -> &serde_json::Value {
            static SCHEMA: once_cell::sync::Lazy<serde_json::Value> = once_cell::sync::Lazy::new(
                || serde_json::json!({ "type": "object", "properties": {} }),
            );
            &SCHEMA
        }
        fn is_enabled(&self, _ctx: &ToolStaticContext) -> bool {
            true
        }
        fn max_result_size_chars(&self) -> usize {
            1024
        }
        fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
            true
        }
        fn is_read_only(&self, _input: &serde_json::Value) -> bool {
            true
        }
        async fn validate_input(
            &self,
            _input: &serde_json::Value,
            _ctx: &ToolUseContext,
        ) -> Result<(), ValidationError> {
            Ok(())
        }
        async fn check_permissions(
            &self,
            _input: &serde_json::Value,
            _ctx: &ToolUseContext,
        ) -> permission::PermissionResult {
            permission::PermissionResult::Allow {
                reason: permission::PermissionDecisionReason::Other { reason: "t".into() },
                updated_input: None,
                update_destination: None,
                metadata: permission::result::PermissionMetadata::default(),
            }
        }
        async fn description(
            &self,
            _input: &serde_json::Value,
            _opts: &DescriptionOptions,
        ) -> String {
            String::new()
        }
        async fn prompt(&self, _opts: &PromptOptions) -> String {
            String::new()
        }
        async fn call(
            &self,
            _input: serde_json::Value,
            _ctx: ToolUseContext,
            _tx: ToolProgressSender,
        ) -> Result<ToolCallResult, ToolError> {
            Ok(ToolCallResult {
                data: serde_json::json!({}),
                model_content: None,
                new_messages: vec![],
                context_modifier: None,
                is_error: false,
                mcp_meta: None,
            })
        }
    }

    fn orch_with(
        tools: ToolRegistry,
        provider: Option<Arc<dyn SkillListingProvider>>,
    ) -> ConversationOrchestrator {
        let api = Arc::new(MockApiClient::new(vec![]));
        let mut orch = ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            api,
            Arc::new(tools),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        );
        if let Some(p) = provider {
            orch = orch.with_skill_listing(p);
        }
        orch
    }

    fn fixture() -> Arc<dyn SkillListingProvider> {
        Arc::new(FixtureSkills(vec![SkillListingEntry {
            name: "debug".into(),
            description: "Debug a failing test".into(),
            when_to_use: None,
            is_bundled: false,
        }]))
    }

    #[tokio::test]
    async fn reminder_present_when_provider_and_skill_tool_wired() {
        let mut reg = ToolRegistry::new();
        reg.register_builtin(Arc::new(NamedTool("Skill")));
        let orch = orch_with(reg, Some(fixture()));
        let msg = orch
            .skill_listing_reminder_message()
            .await
            .expect("reminder present");
        let text = msg.text_content();
        assert!(text.starts_with("<system-reminder>"), "got: {text}");
        assert!(text.contains("The following skills are available for use with the Skill tool:"));
        assert!(text.contains("- debug: Debug a failing test"));
    }

    #[tokio::test]
    async fn no_reminder_when_skill_tool_absent() {
        // Provider wired, but the Skill tool is not in the registry this turn.
        let orch = orch_with(ToolRegistry::new(), Some(fixture()));
        assert!(orch.skill_listing_reminder_message().await.is_none());
    }

    #[tokio::test]
    async fn no_reminder_when_provider_absent() {
        let mut reg = ToolRegistry::new();
        reg.register_builtin(Arc::new(NamedTool("Skill")));
        let orch = orch_with(reg, None);
        assert!(orch.skill_listing_reminder_message().await.is_none());
    }

    // ── PLANMODE (plan_mode_reminder_message) ──────────────────────────────

    #[tokio::test]
    async fn plan_mode_reminder_none_when_plan_mode_off() {
        // Default session: plan mode OFF ⇒ no reminder (keeps the default build
        // byte-identical).
        let orch = orch_with(ToolRegistry::new(), None);
        assert!(orch.session().lock().await.plan_mode == false);
        assert!(orch.plan_mode_reminder_message().await.is_none());
    }

    #[tokio::test]
    async fn plan_mode_reminder_full_then_sparse() {
        let orch = orch_with(ToolRegistry::new(), None);
        {
            let sess = orch.session();
            let mut s = sess.lock().await;
            s.plan_mode = true;
            // EnterPlanMode resets this; assert the reset default explicitly.
            s.plan_reminder_shown = false;
        }

        // First plan-mode turn ⇒ FULL (206 `LU_`): the aIp banner + the 5-phase
        // workflow scaffold.
        let m0 = orch
            .plan_mode_reminder_message()
            .await
            .expect("plan-mode full reminder");
        // 2.1.238 `Zy`/`NT` envelope + `isMeta:!0` (@296675470 / @296673554).
        assert!(m0.is_meta(), "plan_mode reminder must be isMeta");
        let t0 = m0.text_content();
        assert!(
            t0.starts_with("<system-reminder>\nPlan mode is active. The user indicated"),
            "turn-0 must be the FULL reminder inside the system-reminder envelope, got: {t0}"
        );
        assert!(
            t0.ends_with("\n</system-reminder>"),
            "envelope must close: {t0}"
        );
        assert!(
            t0.contains("## Plan Workflow"),
            "full reminder scaffold: {t0}"
        );
        assert!(
            t0.contains("### Phase 5: Call ExitPlanMode"),
            "full reminder phases: {t0}"
        );
        // No plan file on disk for a fresh temp session.
        assert!(
            t0.contains("No plan file exists yet."),
            "planExists=false: {t0}"
        );
        // Injection armed the sparse flag.
        assert!(orch.session().lock().await.plan_reminder_shown);

        // Second plan-mode turn ⇒ SPARSE (206 `MU_`).
        let t1 = orch
            .plan_mode_reminder_message()
            .await
            .expect("plan-mode sparse reminder")
            .text_content();
        assert!(
            t1.starts_with(
                "<system-reminder>\nPlan mode still active (see full instructions earlier in conversation)."
            ),
            "turn-1 must be the SPARSE reminder, got: {t1}"
        );
        assert!(t1.contains("Follow 5-phase workflow."), "sparse body: {t1}");
    }

    #[tokio::test]
    async fn plan_mode_reminder_reset_replays_full() {
        // After a sparse turn, re-entering plan mode (reset flag) replays FULL.
        let orch = orch_with(ToolRegistry::new(), None);
        orch.session().lock().await.plan_mode = true;
        let _full = orch.plan_mode_reminder_message().await.expect("full");
        let _sparse = orch.plan_mode_reminder_message().await.expect("sparse");
        // Simulate EnterPlanMode / set_plan_mode(true) re-arming the tracker.
        orch.session().lock().await.plan_reminder_shown = false;
        let again = orch
            .plan_mode_reminder_message()
            .await
            .expect("full again after reset")
            .text_content();
        assert!(
            again.starts_with("<system-reminder>\nPlan mode is active. The user indicated"),
            "got: {again}"
        );
    }

    #[tokio::test]
    async fn plan_mode_reminder_uses_custom_instructions() {
        // C5: `--plan-mode-instructions` (config.plan_mode_instructions) replaces
        // the default 5-phase body with the custom "## Plan Workflow" branch.
        let mut orch = orch_with(ToolRegistry::new(), None);
        orch.config.plan_mode_instructions = Some("MY BODY".to_string());
        orch.session().lock().await.plan_mode = true;
        let full = orch
            .plan_mode_reminder_message()
            .await
            .expect("plan-mode custom reminder")
            .text_content();
        assert!(
            full.contains("## Plan Workflow\n\nMY BODY\n\n### Call ExitPlanMode"),
            "custom workflow body: {full}"
        );
        assert!(
            !full.contains("### Phase 1"),
            "default phases suppressed: {full}"
        );
    }

    // ── SKILLLIST.1 delta (sent-tracking) ──────────────────────────────────

    /// A skill provider whose entry set can change between turns (shared
    /// `Arc<Mutex<…>>`), to exercise the "new skill appears later" delta path.
    struct MutableSkills(std::sync::Arc<std::sync::Mutex<Vec<SkillListingEntry>>>);
    #[async_trait]
    impl SkillListingProvider for MutableSkills {
        async fn skill_entries(&self) -> Vec<SkillListingEntry> {
            self.0.lock().unwrap().clone()
        }
    }

    fn skill(name: &str) -> SkillListingEntry {
        SkillListingEntry {
            name: name.into(),
            description: format!("desc for {name}"),
            when_to_use: None,
            is_bundled: false,
        }
    }

    #[tokio::test]
    async fn skill_listing_delta_turn0_full_then_none_when_no_new() {
        // Turn 0 emits the FULL listing; a later turn with the SAME skills (no
        // new names) emits nothing (None).
        let mut reg = ToolRegistry::new();
        reg.register_builtin(Arc::new(NamedTool("Skill")));
        let orch = orch_with(
            reg,
            Some(Arc::new(FixtureSkills(vec![skill("alpha"), skill("beta")]))),
        );

        // Turn 0: both skills present.
        let t0 = orch
            .skill_listing_reminder_message()
            .await
            .expect("turn-0 full listing");
        let t0 = t0.text_content();
        assert!(t0.contains("- alpha:"), "turn-0 missing alpha: {t0}");
        assert!(t0.contains("- beta:"), "turn-0 missing beta: {t0}");

        // Turn 1: no NEW skills since both were already sent → None.
        assert!(
            orch.skill_listing_reminder_message().await.is_none(),
            "turn-1 must emit nothing when no new skill appeared"
        );
    }

    #[tokio::test]
    async fn skill_listing_delta_emits_only_new_skill_on_later_turn() {
        let mut reg = ToolRegistry::new();
        reg.register_builtin(Arc::new(NamedTool("Skill")));
        let shared = std::sync::Arc::new(std::sync::Mutex::new(vec![skill("alpha")]));
        let orch = orch_with(reg, Some(Arc::new(MutableSkills(shared.clone()))));

        // Turn 0: only `alpha`.
        let t0 = orch
            .skill_listing_reminder_message()
            .await
            .expect("turn-0")
            .text_content();
        assert!(t0.contains("- alpha:"));
        assert!(!t0.contains("- gamma:"));

        // A new skill `gamma` appears.
        shared.lock().unwrap().push(skill("gamma"));

        // Turn 1: ONLY the new `gamma` is emitted (alpha was already sent).
        let t1 = orch
            .skill_listing_reminder_message()
            .await
            .expect("turn-1 new-only")
            .text_content();
        assert!(
            t1.contains("- gamma:"),
            "turn-1 must contain the new skill: {t1}"
        );
        assert!(
            !t1.contains("- alpha:"),
            "turn-1 must NOT re-emit the already-sent skill: {t1}"
        );
    }

    struct OnceAsyncResponses(std::sync::Mutex<Vec<String>>);
    #[async_trait::async_trait]
    impl crate::prompt::async_hook_response::AsyncHookResponseProvider for OnceAsyncResponses {
        async fn take_pending_responses(&self) -> Vec<String> {
            std::mem::take(&mut *self.0.lock().unwrap())
        }
    }

    #[tokio::test]
    async fn async_hook_response_reminder_folds_in_then_drains_once() {
        let reg = ToolRegistry::new();
        let orch = orch_with(reg, None).with_async_hook_responses(Arc::new(OnceAsyncResponses(
            std::sync::Mutex::new(vec!["ran background lints: clean".to_string()]),
        )));
        // Turn 0: the completed background-hook response is folded in, wrapped.
        let t0 = orch
            .async_hook_response_reminder_message()
            .await
            .expect("turn-0 async hook response")
            .text_content();
        assert!(t0.contains("<system-reminder>"), "must be wrapped: {t0}");
        assert!(
            t0.contains("ran background lints: clean"),
            "must carry the hook's system_message: {t0}"
        );
        // Turn 1: consume-once — the delivered response must NOT re-appear.
        assert!(
            orch.async_hook_response_reminder_message().await.is_none(),
            "a delivered async-hook response must be drained, not repeated"
        );
    }

    #[tokio::test]
    async fn async_hook_response_reminder_none_without_provider() {
        let reg = ToolRegistry::new();
        let orch = orch_with(reg, None);
        assert!(
            orch.async_hook_response_reminder_message().await.is_none(),
            "no provider wired ⇒ strict no-op"
        );
    }

    // ── hook-bg-fields: Stop / SubagentStop background_tasks + session_crons ──

    /// A [`StopHookSnapshotProvider`] that returns fixed fixtures, so the
    /// orchestrator's `populate_stop_hook_snapshot` wiring is testable without a
    /// live registry / cron file.
    struct FixtureStopSnapshot {
        tasks: Vec<hooks::HookBackgroundTask>,
        crons: Vec<hooks::HookSessionCron>,
    }
    #[async_trait::async_trait]
    impl crate::stop_hook_snapshot::StopHookSnapshotProvider for FixtureStopSnapshot {
        async fn background_tasks(&self) -> Vec<hooks::HookBackgroundTask> {
            self.tasks.clone()
        }
        async fn session_crons(&self) -> Vec<hooks::HookSessionCron> {
            self.crons.clone()
        }
    }

    #[tokio::test]
    async fn populate_stop_hook_snapshot_stamps_both_arrays_when_wired() {
        use hooks::{HookBackgroundTask, HookSessionCron};
        let reg = ToolRegistry::new();
        let orch = orch_with(reg, None).with_stop_hook_snapshot(Arc::new(FixtureStopSnapshot {
            tasks: vec![HookBackgroundTask {
                id: "b1".into(),
                r#type: "shell".into(),
                status: "running".into(),
                description: "build".into(),
                command: Some("cargo build".into()),
                agent_type: None,
                server: None,
                tool: None,
                name: None,
            }],
            crons: vec![HookSessionCron {
                id: "c1".into(),
                schedule: "* * * * *".into(),
                recurring: true,
                prompt: "hi".into(),
            }],
        }));
        // A Stop-firing context (the only path that populates the snapshot)
        // carries BOTH arrays, populated, after the snapshot helper runs.
        let mut ctx = orch.lifecycle_hook_ctx(false).await;
        // Before population the lifecycle ctx leaves both fields None (the
        // default — a non-Stop lifecycle hook omits the keys).
        assert!(ctx.background_tasks.is_none());
        assert!(ctx.session_crons.is_none());
        orch.populate_stop_hook_snapshot(&mut ctx).await;
        let bg = ctx.background_tasks.expect("background_tasks populated");
        assert_eq!(bg.len(), 1);
        assert_eq!(bg[0].id, "b1");
        assert_eq!(bg[0].r#type, "shell");
        let crons = ctx.session_crons.expect("session_crons populated");
        assert_eq!(crons.len(), 1);
        assert_eq!(crons[0].id, "c1");
        assert!(crons[0].recurring);
    }

    #[tokio::test]
    async fn populate_stop_hook_snapshot_noop_without_provider() {
        let reg = ToolRegistry::new();
        let orch = orch_with(reg, None);
        let mut ctx = orch.lifecycle_hook_ctx(false).await;
        orch.populate_stop_hook_snapshot(&mut ctx).await;
        // No provider wired ⇒ both fields stay None ⇒ the executor omits the
        // keys (claude `m = undefined`), byte-identical to the pre-feature build.
        assert!(ctx.background_tasks.is_none());
        assert!(ctx.session_crons.is_none());
    }

    // ── T35: `task-notification` reminder folds in then drains once ──────────

    /// A [`TaskNotificationProvider`] that hands back its fixture exactly once
    /// (the second drain returns empty), mirroring the registry's
    /// take-mark-evict semantics so the consume-once invariant is testable
    /// without a real registry.
    struct OnceTaskNotifications(std::sync::Mutex<Vec<traits::task_registry::TaskNotification>>);
    #[async_trait::async_trait]
    impl crate::prompt::task_notification::TaskNotificationProvider for OnceTaskNotifications {
        async fn take_pending_task_notifications(
            &self,
        ) -> Vec<traits::task_registry::TaskNotification> {
            std::mem::take(&mut *self.0.lock().unwrap())
        }
    }

    #[tokio::test]
    async fn task_notification_reminder_folds_in_then_drains_once() {
        let reg = ToolRegistry::new();
        // One terminal `local_bash` task — the minimal faithful surface.
        let bash = traits::task_registry::TaskNotification {
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
        };
        let orch = orch_with(reg, None).with_task_notifications(Arc::new(OnceTaskNotifications(
            std::sync::Mutex::new(vec![bash]),
        )));
        // Turn 0: the terminal task is folded in as a byte-faithful
        // `<task-notification>` inside one `<system-reminder>`.
        let t0 = orch
            .task_notification_reminder_message()
            .await
            .expect("turn-0 task notification")
            .text_content();
        let body = "<system-reminder>\n\
<task-notification>\n\
<task-id>b12345678</task-id>\n\
<output-file>/tmp/tasks/b12345678.output</output-file>\n\
<status>completed</status>\n\
<summary>Background command \"run tests\" completed (exit code 0)</summary>\n\
</task-notification>\n\
</system-reminder>";
        assert_eq!(
            t0,
            format!(
                "{}{body}",
                crate::prompt::task_notification::NON_USER_INPUT_HEADER
            )
        );
        // Turn 1: consume-once — the notified+evicted task must NOT re-appear.
        assert!(
            orch.task_notification_reminder_message().await.is_none(),
            "a delivered task notification must be drained, not repeated"
        );
    }

    #[tokio::test]
    async fn task_notification_reminder_none_without_provider() {
        let reg = ToolRegistry::new();
        let orch = orch_with(reg, None);
        assert!(
            orch.task_notification_reminder_message().await.is_none(),
            "no provider wired ⇒ strict no-op"
        );
    }
}

// ── `agent_listing_delta`: per-turn, transient agent catalog reminder ─────────
//
// Proves [`ConversationOrchestrator::agent_listing_reminder_message`]:
// - GATE OFF (default): always `None`, and the inline `AgentTool` prompt is
//   unchanged (asserted in `tool-agent` — here we just confirm the orchestrator
//   side stays silent).
// - GATE ON (`LINGXI_AGENT_LIST_IN_MESSAGES=1`, guarded by a process-wide
//   lock): turn-0 full listing + "Available agent types for the Agent tool:"
//   header; a later turn with no new types ⇒ `None`; a newly-added type ⇒ a
//   delta with the "New agent types are now available…" header and ONLY the new
//   line. Also gated on the `Agent` tool's presence + a wired catalog.
#[cfg(test)]
mod agent_listing_reminder_tests {
    use super::*;
    use crate::test_support::{
        noop_hook_executor, MockApiClient, MockOutputStream, NoOpPermissionGate,
        StaticMemoryProvider,
    };
    use crate::OrchestratorConfig;
    use agent::{AgentDefinition, AgentModel, AgentPermissionMode, AgentSource, AgentToolPolicy};
    use std::sync::Arc;
    use tool_api::context::ToolUseContext;
    use tool_api::progress::ToolProgressSender;
    use tool_api::registry::ToolRegistry;
    use tool_api::tool_trait::{
        DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolStaticContext,
        ValidationError,
    };

    /// `LINGXI_AGENT_LIST_IN_MESSAGES` is process-global; serialize the
    /// gate-sensitive tests (every one removes/sets the var under this lock).
    static AGENT_LIST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Minimal tool whose only meaningful behavior is its name — used to put an
    /// `Agent`-named tool (or not) into the registry for the gate test.
    struct NamedTool(&'static str);
    #[async_trait]
    impl Tool for NamedTool {
        fn name(&self) -> &str {
            self.0
        }
        fn input_schema(&self) -> &serde_json::Value {
            static SCHEMA: once_cell::sync::Lazy<serde_json::Value> = once_cell::sync::Lazy::new(
                || serde_json::json!({ "type": "object", "properties": {} }),
            );
            &SCHEMA
        }
        fn is_enabled(&self, _ctx: &ToolStaticContext) -> bool {
            true
        }
        fn max_result_size_chars(&self) -> usize {
            1024
        }
        fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
            true
        }
        fn is_read_only(&self, _input: &serde_json::Value) -> bool {
            true
        }
        async fn validate_input(
            &self,
            _input: &serde_json::Value,
            _ctx: &ToolUseContext,
        ) -> Result<(), ValidationError> {
            Ok(())
        }
        async fn check_permissions(
            &self,
            _input: &serde_json::Value,
            _ctx: &ToolUseContext,
        ) -> permission::PermissionResult {
            permission::PermissionResult::Allow {
                reason: permission::PermissionDecisionReason::Other { reason: "t".into() },
                updated_input: None,
                update_destination: None,
                metadata: permission::result::PermissionMetadata::default(),
            }
        }
        async fn description(
            &self,
            _input: &serde_json::Value,
            _opts: &DescriptionOptions,
        ) -> String {
            String::new()
        }
        async fn prompt(&self, _opts: &PromptOptions) -> String {
            String::new()
        }
        async fn call(
            &self,
            _input: serde_json::Value,
            _ctx: ToolUseContext,
            _tx: ToolProgressSender,
        ) -> Result<ToolCallResult, ToolError> {
            Ok(ToolCallResult {
                data: serde_json::json!({}),
                model_content: None,
                new_messages: vec![],
                context_modifier: None,
                is_error: false,
                mcp_meta: None,
            })
        }
    }

    fn agent_def(agent_type: &str, when_to_use: &str, tools: AgentToolPolicy) -> AgentDefinition {
        AgentDefinition {
            agent_type: agent_type.into(),
            when_to_use: when_to_use.into(),
            tools,
            max_turns: 1,
            model: AgentModel::Inherit,
            permission_mode: AgentPermissionMode::Bubble,
            source: AgentSource::BuiltIn,
            base_dir: "/tmp".into(),
            system_prompt: None,
            mcp_servers: vec![],
            frontmatter_hooks: vec![],
            icon: None,
            allowed_tools: vec![],
            worktree_requirement: None,
            disallowed_tools: vec![],
            skills: vec![],
            required_mcp_servers: vec![],
            background: false,
            isolation: None,
            memory: None,
            effort: None,
            initial_prompt: None,
            color: None,
            observer: None,
        }
    }

    /// Build an orchestrator with the given tools + optional agent catalog.
    fn orch_with(
        tools: ToolRegistry,
        catalog: Option<Arc<tokio::sync::RwLock<Vec<AgentDefinition>>>>,
    ) -> ConversationOrchestrator {
        let api = Arc::new(MockApiClient::new(vec![]));
        let mut orch = ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            api,
            Arc::new(tools),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        );
        if let Some(c) = catalog {
            orch = orch.with_agent_catalog(c);
        }
        orch
    }

    fn reg_with_agent_tool() -> ToolRegistry {
        let mut reg = ToolRegistry::new();
        reg.register_builtin(Arc::new(NamedTool("Agent")));
        reg
    }

    #[tokio::test]
    async fn gate_explicit_off_is_none() {
        let _g = AGENT_LIST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // v2.1.193 default is ON (catalog externalized); the LEGACY inline path
        // (explicit `=false`) keeps the catalog in the description, so the
        // orchestrator emits no reminder.
        std::env::set_var("LINGXI_AGENT_LIST_IN_MESSAGES", "false");

        let catalog = Arc::new(tokio::sync::RwLock::new(vec![agent_def(
            "general-purpose",
            "anything",
            AgentToolPolicy::All {
                use_exact_tools: false,
            },
        )]));
        let orch = orch_with(reg_with_agent_tool(), Some(catalog));
        let got = orch.agent_listing_reminder_message().await;
        std::env::remove_var("LINGXI_AGENT_LIST_IN_MESSAGES");
        assert!(
            got.is_none(),
            "explicit gate OFF ⇒ no reminder (inline path)"
        );
    }

    #[tokio::test]
    async fn gate_on_no_catalog_still_announces_builtins() {
        // Binary `aLe` builds the delta from `activeAgents` (built-ins +
        // user/project), gating ONLY on the Agent tool's presence — NOT on a
        // wired DISK catalog. So a session with the gate ON, the Agent tool
        // present, and NO disk catalog still announces the BUILT-IN agents
        // (e.g. general-purpose). (Previously this early-returned `None`,
        // suppressing built-ins under the gate — a divergence from `aLe`.)
        let _g = AGENT_LIST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::set_var("LINGXI_AGENT_LIST_IN_MESSAGES", "1");
        let orch = orch_with(reg_with_agent_tool(), None);
        let got = orch.agent_listing_reminder_message().await;
        std::env::remove_var("LINGXI_AGENT_LIST_IN_MESSAGES");
        let text = got
            .expect("built-ins must be announced even with no disk catalog")
            .text_content();
        assert!(text.starts_with("<system-reminder>"), "got: {text}");
        assert!(
            text.contains("Available agent types for the Agent tool:"),
            "turn-0 initial header expected; got: {text}"
        );
        assert!(
            text.contains("- general-purpose:"),
            "built-ins must be listed with no disk catalog; got: {text}"
        );
    }

    #[tokio::test]
    async fn gate_on_but_agent_tool_absent_is_none() {
        let _g = AGENT_LIST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::set_var("LINGXI_AGENT_LIST_IN_MESSAGES", "1");
        let catalog = Arc::new(tokio::sync::RwLock::new(vec![agent_def(
            "general-purpose",
            "anything",
            AgentToolPolicy::All {
                use_exact_tools: false,
            },
        )]));
        // Empty registry — the Agent tool is not present this turn.
        let orch = orch_with(ToolRegistry::new(), Some(catalog));
        let got = orch.agent_listing_reminder_message().await;
        std::env::remove_var("LINGXI_AGENT_LIST_IN_MESSAGES");
        assert!(got.is_none(), "Agent tool absent ⇒ no reminder");
    }

    #[tokio::test]
    async fn gate_on_turn0_full_listing_with_initial_header() {
        let _g = AGENT_LIST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::set_var("LINGXI_AGENT_LIST_IN_MESSAGES", "1");
        // Catalog supplies a custom type; built-ins are merged in too.
        let catalog = Arc::new(tokio::sync::RwLock::new(vec![agent_def(
            "custom-agent",
            "a project agent",
            AgentToolPolicy::Explicit(vec!["Read".into()]),
        )]));
        let orch = orch_with(reg_with_agent_tool(), Some(catalog));

        let msg = orch.agent_listing_reminder_message().await;
        std::env::remove_var("LINGXI_AGENT_LIST_IN_MESSAGES");
        let text = msg.expect("turn-0 reminder present").text_content();

        assert!(text.starts_with("<system-reminder>"), "got: {text}");
        assert!(text.ends_with("</system-reminder>"), "got: {text}");
        assert!(
            text.contains("Available agent types for the Agent tool:"),
            "turn-0 must use the is_initial header; got: {text}"
        );
        // formatAgentLine for the catalog entry.
        assert!(
            text.contains("- custom-agent: a project agent (Tools: Read)"),
            "missing catalog line; got: {text}"
        );
        // Built-ins are merged in (e.g. general-purpose).
        assert!(
            text.contains("- general-purpose:"),
            "built-ins must be merged into the listing; got: {text}"
        );
    }

    #[tokio::test]
    async fn gate_on_later_turn_no_new_types_is_none() {
        let _g = AGENT_LIST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::set_var("LINGXI_AGENT_LIST_IN_MESSAGES", "1");
        let catalog = Arc::new(tokio::sync::RwLock::new(vec![agent_def(
            "custom-agent",
            "a project agent",
            AgentToolPolicy::All {
                use_exact_tools: false,
            },
        )]));
        let orch = orch_with(reg_with_agent_tool(), Some(catalog));

        // Turn 0 emits the full listing.
        let t0 = orch.agent_listing_reminder_message().await;
        assert!(t0.is_some(), "turn-0 must emit");
        // Turn 1 with the same catalog ⇒ nothing new ⇒ None.
        let t1 = orch.agent_listing_reminder_message().await;
        std::env::remove_var("LINGXI_AGENT_LIST_IN_MESSAGES");
        assert!(t1.is_none(), "no new types ⇒ no reminder");
    }

    #[tokio::test]
    async fn gate_on_newly_added_type_emits_delta_with_new_header_only() {
        let _g = AGENT_LIST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::set_var("LINGXI_AGENT_LIST_IN_MESSAGES", "1");
        let catalog = Arc::new(tokio::sync::RwLock::new(vec![agent_def(
            "alpha-agent",
            "the alpha agent",
            AgentToolPolicy::All {
                use_exact_tools: false,
            },
        )]));
        let orch = orch_with(reg_with_agent_tool(), Some(catalog.clone()));

        // Turn 0: full listing (contains alpha-agent + built-ins).
        let t0 = orch
            .agent_listing_reminder_message()
            .await
            .expect("turn-0")
            .text_content();
        assert!(t0.contains("- alpha-agent:"));
        assert!(!t0.contains("- gamma-agent:"));

        // A brand-new agent type appears in the catalog.
        catalog.write().await.push(agent_def(
            "gamma-agent",
            "the gamma agent",
            AgentToolPolicy::Explicit(vec!["Read".into(), "Edit".into()]),
        ));

        // Turn 1: ONLY the new type, with the "New agent types…" header.
        let t1 = orch
            .agent_listing_reminder_message()
            .await
            .expect("turn-1 delta")
            .text_content();
        std::env::remove_var("LINGXI_AGENT_LIST_IN_MESSAGES");

        assert!(
            t1.contains("New agent types are now available for the Agent tool:"),
            "delta must use the non-initial header; got: {t1}"
        );
        assert!(
            !t1.contains("Available agent types for the Agent tool:"),
            "delta must NOT use the is_initial header; got: {t1}"
        );
        assert!(
            t1.contains("- gamma-agent: the gamma agent (Tools: Read, Edit)"),
            "delta must contain the new agent line; got: {t1}"
        );
        assert!(
            !t1.contains("- alpha-agent:"),
            "delta must NOT re-emit an already-announced type; got: {t1}"
        );
    }
}

// ── §F: per-turn, transient `conditional_rules` reminder ──────────────────────
//
// Mirrors the `skill_listing_reminder_tests` template: a `StaticMemoryProvider`
// fixture supplies conditional (`paths:`-gated) `MemoryFile`s, the touched-file
// set is seeded directly into the shared `read_state_map`, and
// `conditional_rules_reminder_message` is asserted to inject the matching rule
// once (with sent-tracking dedup) and skip non-matching / already-sent rules.
#[cfg(test)]
mod new_diagnostics_reminder_tests {
    use super::*;
    use crate::test_support::{
        noop_hook_executor, MockApiClient, MockOutputStream, NoOpPermissionGate,
        StaticMemoryProvider,
    };
    use crate::OrchestratorConfig;
    use std::sync::Arc;
    use tool_api::registry::ToolRegistry;

    struct MockDiag(Option<String>);
    #[async_trait::async_trait]
    impl traits::NewDiagnosticsSource for MockDiag {
        async fn take_new_diagnostics_block(&self) -> Option<String> {
            self.0.clone()
        }
    }

    fn orch_with_diag(
        source: Option<Arc<dyn traits::NewDiagnosticsSource>>,
    ) -> ConversationOrchestrator {
        let o = ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::with_files(vec![])),
            std::path::PathBuf::from("/work"),
        );
        match source {
            Some(s) => o.with_new_diagnostics_source(s),
            None => o,
        }
    }

    #[tokio::test]
    async fn injects_block_when_source_has_new_diagnostics() {
        let block = "<new-diagnostics>The following new diagnostic issues were detected:\n\nx.rs:\n  \u{2718} [Line 1:1] boom</new-diagnostics>";
        let orch = orch_with_diag(Some(Arc::new(MockDiag(Some(block.to_string())))));
        let msg = orch
            .new_diagnostics_reminder_message()
            .await
            .expect("a block is injected");
        assert_eq!(msg.text_content(), block);
    }

    #[tokio::test]
    async fn no_reminder_without_source_or_when_empty() {
        // No source wired (the common no-LSP case).
        assert!(orch_with_diag(None)
            .new_diagnostics_reminder_message()
            .await
            .is_none());
        // Source wired but nothing new.
        assert!(orch_with_diag(Some(Arc::new(MockDiag(None))))
            .new_diagnostics_reminder_message()
            .await
            .is_none());
    }
}

#[cfg(test)]
mod conditional_rules_reminder_tests {
    use super::*;
    use crate::prompt::MemoryFile;
    use crate::test_support::{
        noop_hook_executor, MockApiClient, MockOutputStream, NoOpPermissionGate,
        StaticMemoryProvider,
    };
    use crate::OrchestratorConfig;
    use memory::lingxi_md::LingxiMdTier;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tool_api::registry::ToolRegistry;

    /// A Project-tier conditional rule living at `<cwd>/.lingxi/rules/{name}.md`
    /// (so its derived base dir is `<cwd>`) carrying the given `paths:` globs.
    fn project_rule(cwd: &std::path::Path, name: &str, globs: &[&str]) -> MemoryFile {
        MemoryFile {
            path: cwd.join(".lingxi").join("rules").join(format!("{name}.md")),
            body: format!("BODY OF {name}"),
            is_local_override: false,
            tier: LingxiMdTier::Project,
            globs: Some(globs.iter().map(|s| (*s).to_string()).collect()),
            raw_content: format!("BODY OF {name}"),
            content_differs_from_disk: false,
        }
    }

    /// Build an orchestrator whose memory provider returns `rules` and whose cwd
    /// is `cwd`. Conditional rules need no Skill tool / skill provider.
    fn orch_with_rules(cwd: PathBuf, rules: Vec<MemoryFile>) -> ConversationOrchestrator {
        ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::with_files(rules)),
            cwd,
        )
    }

    fn push_touched(orch: &ConversationOrchestrator, path: &std::path::Path) {
        // Seed the ONE shared read-state registry the way a file tool's
        // `readFileState.set` does (content is irrelevant to rule matching,
        // which keys off the path).
        tool_api::read_file_state::set(
            &orch.read_state_map,
            path.to_path_buf(),
            tool_api::read_file_state::ReadFileEntry {
                content: String::new(),
                mtime_ms: 0,
                offset: None,
                limit: None,
                from_read: true,
                seeded_from_context: false,
                is_partial_view: false,
            },
        );
    }

    fn push_host_seed(orch: &ConversationOrchestrator, path: &std::path::Path) {
        tool_api::read_file_state::set_with_model_context(
            &orch.read_state_map,
            path.to_path_buf(),
            tool_api::read_file_state::ReadFileEntry {
                content: String::new(),
                mtime_ms: 0,
                offset: None,
                limit: None,
                from_read: false,
                seeded_from_context: false,
                is_partial_view: false,
            },
            false,
        );
    }

    #[tokio::test]
    async fn matching_touched_file_injects_rule() {
        let cwd = PathBuf::from("/work/repo");
        let orch = orch_with_rules(cwd.clone(), vec![project_rule(&cwd, "scoped", &["src"])]);
        // A touched file under `src/` matches `paths: src/**`.
        push_touched(&orch, &cwd.join("src/x.rs"));

        let msg = orch
            .conditional_rules_reminder_message()
            .await
            .expect("matching rule must be injected");
        let text = msg.text_content();
        assert!(text.starts_with("<system-reminder>"), "got: {text}");
        assert!(
            text.contains("Contents of /work/repo/.lingxi/rules/scoped.md:"),
            "got: {text}"
        );
        assert!(text.contains("BODY OF scoped"), "got: {text}");
        // It is a BARE nested-memory render — no eager-block preamble.
        assert!(!text.contains("Codebase and user instructions"));
    }

    #[tokio::test]
    async fn non_matching_touched_file_does_not_inject() {
        let cwd = PathBuf::from("/work/repo");
        let orch = orch_with_rules(cwd.clone(), vec![project_rule(&cwd, "scoped", &["src"])]);
        // `docs/y.md` does NOT match `paths: src/**`.
        push_touched(&orch, &cwd.join("docs/y.md"));
        assert!(
            orch.conditional_rules_reminder_message().await.is_none(),
            "a non-matching touched file must not activate the rule"
        );
    }

    #[tokio::test]
    async fn host_seeded_file_does_not_activate_conditional_rule() {
        let cwd = PathBuf::from("/work/repo");
        let orch = orch_with_rules(cwd.clone(), vec![project_rule(&cwd, "scoped", &["src"])]);
        push_host_seed(&orch, &cwd.join("src/x.rs"));
        assert!(
            orch.conditional_rules_reminder_message().await.is_none(),
            "host-seeded paths are not model context and must not trigger rules"
        );
    }

    #[tokio::test]
    async fn rule_injected_once_then_not_reinjected() {
        let cwd = PathBuf::from("/work/repo");
        let orch = orch_with_rules(cwd.clone(), vec![project_rule(&cwd, "scoped", &["src"])]);
        push_touched(&orch, &cwd.join("src/x.rs"));

        // Turn 0: injected.
        assert!(
            orch.conditional_rules_reminder_message().await.is_some(),
            "first activation must inject"
        );
        // Turn 1: the same file is still touched, but the rule was already sent →
        // not re-injected (sent-tracking dedup).
        assert!(
            orch.conditional_rules_reminder_message().await.is_none(),
            "an already-sent rule must not be re-injected"
        );
    }

    #[tokio::test]
    async fn no_touched_file_yields_none() {
        let cwd = PathBuf::from("/work/repo");
        let orch = orch_with_rules(cwd.clone(), vec![project_rule(&cwd, "scoped", &["src"])]);
        // read_file_state empty → no rule can match.
        assert!(orch.conditional_rules_reminder_message().await.is_none());
    }

    #[tokio::test]
    async fn no_conditional_rules_yields_none() {
        // Provider returns an unconditional file only (globs == None): the
        // conditional cache is empty, so the reminder is a strict no-op even with
        // a touched file present.
        let cwd = PathBuf::from("/work/repo");
        let unconditional = MemoryFile {
            path: cwd.join("LINGXI.md"),
            body: "always".into(),
            is_local_override: false,
            tier: LingxiMdTier::Project,
            globs: None,
            raw_content: "always".into(),
            content_differs_from_disk: false,
        };
        let orch = orch_with_rules(cwd.clone(), vec![unconditional]);
        push_touched(&orch, &cwd.join("src/x.rs"));
        assert!(orch.conditional_rules_reminder_message().await.is_none());
    }

    #[tokio::test]
    async fn newly_matching_rule_injected_on_later_turn() {
        // Two rules; only one matches initially. After a second file is touched,
        // the second rule activates and is injected (delta across turns).
        let cwd = PathBuf::from("/work/repo");
        let orch = orch_with_rules(
            cwd.clone(),
            vec![
                project_rule(&cwd, "src-rule", &["src"]),
                project_rule(&cwd, "docs-rule", &["docs"]),
            ],
        );
        push_touched(&orch, &cwd.join("src/a.rs"));
        let t0 = orch
            .conditional_rules_reminder_message()
            .await
            .expect("src-rule active")
            .text_content();
        assert!(t0.contains("src-rule.md"));
        assert!(!t0.contains("docs-rule.md"));

        // Now touch a docs file → docs-rule newly activates; src-rule already sent.
        push_touched(&orch, &cwd.join("docs/readme.md"));
        let t1 = orch
            .conditional_rules_reminder_message()
            .await
            .expect("docs-rule newly active")
            .text_content();
        assert!(t1.contains("docs-rule.md"), "got: {t1}");
        assert!(
            !t1.contains("src-rule.md"),
            "already-sent src-rule must not re-inject: {t1}"
        );
    }
}

// Nested memory (`k$o` @237714543 fed by `Rop` @237715260): the per-turn
// reminder that surfaces the LINGXI.md governing a TOUCHED file's directory.
#[cfg(test)]
mod nested_memory_reminder_tests {
    use super::*;
    use crate::test_support::{
        noop_hook_executor, MockApiClient, MockOutputStream, NoOpPermissionGate,
        StaticMemoryProvider,
    };
    use crate::OrchestratorConfig;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use tool_api::registry::ToolRegistry;

    const MEM: &str = branding::MEMORY_FILE;
    const DOT: &str = branding::DOT_DIR;

    fn touch(p: &Path, body: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    /// cwd=`<root>/repo`, trigger `<root>/repo/pkg/api/handler.rs`, and a HOME
    /// under the same temp root so the User tier can never reach the real one.
    struct Fixture {
        _tmp: tempfile::TempDir,
        cwd: PathBuf,
        home: PathBuf,
        trigger: PathBuf,
    }

    fn fixture() -> Fixture {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let cwd = root.join("repo");
        let home = root.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let trigger = cwd.join("pkg").join("api").join("handler.rs");
        touch(&trigger, "fn main(){}");
        Fixture {
            _tmp: tmp,
            cwd,
            home,
            trigger,
        }
    }

    fn orch(f: &Fixture) -> ConversationOrchestrator {
        ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::with_files(vec![])),
            f.cwd.clone(),
        )
        // Hermetic roots: without this the User/Managed pass would probe the
        // developer's real `~/.lingxi/rules`.
        .with_nested_memory_roots(f.home.clone(), None)
    }

    fn push_touched(orch: &ConversationOrchestrator, path: &Path) {
        tool_api::read_file_state::set(
            &orch.read_state_map,
            path.to_path_buf(),
            tool_api::read_file_state::ReadFileEntry {
                content: String::new(),
                mtime_ms: 0,
                offset: None,
                limit: None,
                from_read: true,
                seeded_from_context: false,
                is_partial_view: false,
            },
        );
    }

    #[tokio::test]
    async fn surfaces_ancestor_memory_once_then_never_again() {
        let f = fixture();
        touch(&f.cwd.join("pkg").join(MEM), "pkg guidance");
        let orch = orch(&f);
        push_touched(&orch, &f.trigger);

        let text = orch
            .nested_memory_reminder_message()
            .await
            .expect("the memory governing the touched file must surface")
            .text_content();
        assert!(text.starts_with("<system-reminder>"), "got: {text}");
        assert!(text.contains("pkg guidance"), "got: {text}");

        assert!(
            orch.nested_memory_reminder_message().await.is_none(),
            "`loadedNestedMemoryPaths` must stop a second emission"
        );
    }

    #[tokio::test]
    async fn no_touched_file_yields_none() {
        let f = fixture();
        touch(&f.cwd.join("pkg").join(MEM), "pkg guidance");
        assert!(orch(&f).nested_memory_reminder_message().await.is_none());
    }

    /// The sent-set must survive the read-state entry disappearing.
    ///
    /// In the happy path the seed itself blocks a second emission, which makes
    /// the two guards indistinguishable — dropping `sent_nested_memory` passes
    /// every other test here. But `read_state_map` is an LRU with entry and
    /// byte caps, so a long session evicts; the oracle's
    /// `loadedNestedMemoryPaths` is a plain non-evicting Set precisely so
    /// eviction cannot resurrect an already-sent file.
    #[tokio::test]
    async fn eviction_from_read_state_does_not_resurrect_a_sent_file() {
        let f = fixture();
        let mem = f.cwd.join("pkg").join(MEM);
        touch(&mem, "pkg guidance");
        let orch = orch(&f);
        push_touched(&orch, &f.trigger);
        assert!(orch.nested_memory_reminder_message().await.is_some());

        // Simulate the LRU dropping the seeded entry.
        let canon = std::fs::canonicalize(&mem).unwrap();
        assert!(
            orch.read_state_map.lock().unwrap().remove(&canon).is_some(),
            "the seed must have been there to evict"
        );

        assert!(
            orch.nested_memory_reminder_message().await.is_none(),
            "already-sent memory must stay sent after its read-state entry is evicted"
        );
    }

    #[tokio::test]
    async fn a_file_the_model_already_read_is_not_surfaced() {
        // `k$o`: `if(!t.readFileState.has(i.path))` — a memory file the model
        // already Read is in context verbatim; re-sending it is pure waste.
        let f = fixture();
        let mem = f.cwd.join("pkg").join(MEM);
        touch(&mem, "pkg guidance");
        let orch = orch(&f);
        push_touched(&orch, &f.trigger);
        push_touched(&orch, &mem);

        assert!(orch.nested_memory_reminder_message().await.is_none());
    }

    #[tokio::test]
    async fn surfacing_seeds_read_state_so_a_later_read_dedups() {
        let f = fixture();
        let mem = f.cwd.join("pkg").join(MEM);
        touch(&mem, "pkg guidance");
        let orch = orch(&f);
        push_touched(&orch, &f.trigger);
        assert!(orch.nested_memory_reminder_message().await.is_some());

        let entry = tool_api::read_file_state::get(
            &orch.read_state_map,
            &std::fs::canonicalize(&mem).unwrap(),
        )
        .expect("the surfaced file must be seeded under its CANONICAL path");
        assert!(
            entry.seeded_from_context,
            "`seededFromContext:!0` is unconditional at this site — it is what \
             makes the next Read return the dedup stub"
        );
        assert!(!entry.from_read, "a seed is not a Read");
    }

    #[tokio::test]
    async fn a_rule_already_sent_by_conditional_rules_is_not_resent() {
        // LingXi runs BOTH mechanisms; the oracle has one. They share
        // `sent_conditional_rules` so a `paths:`-gated rule reaches the model
        // at most once, whichever gets there first.
        let f = fixture();
        let rule = f.cwd.join("pkg").join(DOT).join("rules").join("api.md");
        touch(&rule, "---\npaths:\n  - \"api/**\"\n---\napi rule\n");
        let orch = orch(&f);
        push_touched(&orch, &f.trigger);

        orch.sent_conditional_rules
            .lock()
            .await
            .insert(rule.clone());
        assert!(
            orch.nested_memory_reminder_message().await.is_none(),
            "conditional-rules already sent this rule"
        );
    }
}

// P0.1: `relevant_memory_reminder_messages` SURFACING tests.
//
// A `MemoryPrefetch::with_fixed_result` (seeded surfaced set) is wired via
// `with_memory_prefetch`; `start_memory_prefetch` arms the per-turn handle and
// the reminder is asserted to render the `relevant_memories` shape, dedup against
// both `surfaced_memory_paths` (across turns) and `read_state_map` (the SHARED
// P3.2 nested-channel guard), and stay a strict no-op when no prefetch is wired.
#[cfg(test)]
mod relevant_memory_reminder_tests {
    use super::*;
    use crate::test_support::{
        noop_hook_executor, MockApiClient, MockOutputStream, NoOpPermissionGate,
        StaticMemoryProvider,
    };
    use crate::OrchestratorConfig;
    use async_trait::async_trait;
    use memory::surfacing::SurfacedMemory;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::SystemTime;
    use tool_api::registry::ToolRegistry;

    /// A runtime that actually RUNS the spawned future on the current tokio
    /// runtime, so the prefetch's one-shot send fires (the shared
    /// `noop_hook_executor` `UnusedRuntime` errors instead, which would leave the
    /// channel unresolved). Cancel/sleep are no-ops — the prefetch task is
    /// instantaneous.
    struct InlineRuntime;
    #[async_trait]
    impl traits::RuntimeSpawner for InlineRuntime {
        async fn spawn(
            &self,
            name: &str,
            task: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'static>>,
        ) -> Result<traits::BackgroundTaskHandle, traits::RuntimeError> {
            tokio::spawn(task);
            Ok(traits::BackgroundTaskHandle {
                task_name: name.to_string(),
                task_id: 0,
            })
        }
        async fn sleep(&self, _d: std::time::Duration) {}
        async fn cancel(
            &self,
            _h: &traits::BackgroundTaskHandle,
        ) -> Result<(), traits::RuntimeError> {
            Ok(())
        }
    }

    fn mem(path: &str, content: &str, age_days: u64) -> SurfacedMemory {
        SurfacedMemory {
            path: PathBuf::from(path),
            content: content.into(),
            age_days,
            mtime: SystemTime::UNIX_EPOCH,
        }
    }

    /// Build an orchestrator with NO prefetch wired (surfacing inert).
    fn orch_bare() -> ConversationOrchestrator {
        ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::with_files(vec![])),
            PathBuf::from("/work/repo"),
        )
    }

    #[tokio::test]
    async fn maybe_extract_session_memory_is_noop_without_handle() {
        // The inert default: no session-memory handle wired (and no cache slot)
        // ⇒ a strict no-op (no panic, nothing spawned), so the locked fixtures
        // stay byte-identical. The enabled path's extract+write is covered by
        // `memory::session_memory` tests; the composition-root wiring is gated
        // behind `LINGXI_SESSION_MEMORY` (default off).
        let orch = orch_bare();
        assert!(orch.session_memory.is_none());
        orch.maybe_extract_session_memory().await;
        assert!(orch.session_memory.is_none());
    }

    /// Build an orchestrator whose prefetch resolves to `seed`.
    fn orch_with_seed(seed: Vec<SurfacedMemory>) -> ConversationOrchestrator {
        let runtime: Arc<dyn traits::RuntimeSpawner> = Arc::new(InlineRuntime);
        let prefetch = Arc::new(memory::prefetch::MemoryPrefetch::with_fixed_result(
            runtime, seed,
        ));
        orch_bare().with_memory_prefetch(prefetch)
    }

    #[tokio::test]
    async fn no_prefetch_wired_yields_none() {
        let orch = orch_bare();
        // Without arming, and with no prefetch, the reminder is a strict no-op.
        orch.start_memory_prefetch().await;
        assert!(orch.relevant_memory_reminder_messages().await.is_empty());
        assert!(!orch.has_memory_prefetch());
    }

    #[tokio::test]
    async fn empty_prefetch_result_yields_none() {
        let orch = orch_with_seed(vec![]);
        orch.start_memory_prefetch().await;
        assert!(orch.relevant_memory_reminder_messages().await.is_empty());
    }

    #[tokio::test]
    async fn not_armed_yields_none() {
        // A wired prefetch that was never armed this turn (slot empty) ⇒ None.
        let orch = orch_with_seed(vec![mem("/m/a.md", "A", 0)]);
        assert!(orch.relevant_memory_reminder_messages().await.is_empty());
    }

    #[tokio::test]
    async fn seeded_prefetch_renders_relevant_memories_block() {
        let orch = orch_with_seed(vec![mem("/m/a.md", "USE FD NOT FIND", 0)]);
        orch.start_memory_prefetch().await;
        let msg = orch
            .relevant_memory_reminder_messages()
            .await
            .pop()
            .expect("seeded prefetch must surface");
        let text = msg.text_content();
        assert!(msg.is_meta(), "relevant memory must be a meta user message");
        assert!(
            text.contains(
                "Retrieved for possible relevance \u{2014} use only if it actually applies"
            ),
            "idx-0 preamble missing: {text}"
        );
        assert!(
            text.contains("Memory: /m/a.md:\n\nUSE FD NOT FIND"),
            "got: {text}"
        );
    }

    #[tokio::test]
    async fn surfaced_once_then_not_reinjected_across_turns() {
        let orch = orch_with_seed(vec![mem("/m/a.md", "A", 0)]);
        // Turn 0: surfaced.
        orch.start_memory_prefetch().await;
        assert!(
            !orch.relevant_memory_reminder_messages().await.is_empty(),
            "first surfacing must inject"
        );
        // Turn 1: same memory ⇒ already in surfaced_memory_paths ⇒ no re-inject.
        orch.start_memory_prefetch().await;
        assert!(
            orch.relevant_memory_reminder_messages().await.is_empty(),
            "an already-surfaced memory must not be re-injected"
        );
    }

    /// Seed the ONE shared read-state registry as a file tool's
    /// `readFileState.set` would (path is all the dedup keys off).
    fn seed_read_state(orch: &ConversationOrchestrator, path: PathBuf) {
        tool_api::read_file_state::set(
            &orch.read_state_map,
            path,
            tool_api::read_file_state::ReadFileEntry {
                content: String::new(),
                mtime_ms: 0,
                offset: None,
                limit: None,
                from_read: true,
                seeded_from_context: false,
                is_partial_view: false,
            },
        );
    }

    fn seed_host_read_state(orch: &ConversationOrchestrator, path: PathBuf) {
        tool_api::read_file_state::set_with_model_context(
            &orch.read_state_map,
            path,
            tool_api::read_file_state::ReadFileEntry {
                content: String::new(),
                mtime_ms: 0,
                offset: None,
                limit: None,
                from_read: false,
                seeded_from_context: false,
                is_partial_view: false,
            },
            false,
        );
    }

    #[tokio::test]
    async fn shared_dedup_skips_memory_already_in_read_state_map() {
        // A memory whose path was already loaded as a nested/conditional (P3.2)
        // attachment / tool read (present in the shared read_state_map) must NOT
        // be double-injected via the surfacing channel.
        let orch = orch_with_seed(vec![mem("/m/a.md", "A", 0)]);
        seed_read_state(&orch, PathBuf::from("/m/a.md"));
        orch.start_memory_prefetch().await;
        assert!(
            orch.relevant_memory_reminder_messages().await.is_empty(),
            "a path already in read_state_map must not be surfaced"
        );
    }

    #[tokio::test]
    async fn partial_dedup_surfaces_only_fresh_memories() {
        // Two memories; one already read. Only the fresh one surfaces, and it
        // carries the idx-0 preamble (it is the first RENDERED memory).
        let orch = orch_with_seed(vec![
            mem("/m/seen.md", "SEEN", 0),
            mem("/m/new.md", "NEW", 0),
        ]);
        seed_read_state(&orch, PathBuf::from("/m/seen.md"));
        orch.start_memory_prefetch().await;
        let messages = orch.relevant_memory_reminder_messages().await;
        let text = messages[0].text_content();
        assert!(text.contains("Memory: /m/new.md:\n\nNEW"), "got: {text}");
        assert!(
            !text.contains("/m/seen.md"),
            "already-read memory leaked: {text}"
        );
    }

    #[tokio::test]
    async fn host_seeded_path_does_not_suppress_relevant_memory() {
        let orch = orch_with_seed(vec![mem("/m/seeded.md", "SEEDED", 0)]);
        seed_host_read_state(&orch, PathBuf::from("/m/seeded.md"));
        orch.start_memory_prefetch().await;
        let messages = orch.relevant_memory_reminder_messages().await;
        let text = messages[0].text_content();
        assert!(
            text.contains("Memory: /m/seeded.md:\n\nSEEDED"),
            "got: {text}"
        );
    }

    #[tokio::test]
    async fn multiple_memories_keep_independent_meta_message_boundaries() {
        let orch = orch_with_seed(vec![mem("/m/a.md", "A", 0), mem("/m/b.md", "B", 0)]);
        orch.start_memory_prefetch().await;
        let messages = orch.relevant_memory_reminder_messages().await;
        assert_eq!(messages.len(), 2);
        assert!(messages.iter().all(ConversationMessage::is_meta));
        assert!(messages[0].text_content().contains("Memory: /m/a.md:"));
        assert!(messages[1].text_content().contains("Memory: /m/b.md:"));
        assert!(!messages[1]
            .text_content()
            .contains("Retrieved for possible relevance"));
    }
}

// ── EXPERIMENTAL_SKILL_SEARCH skill-discovery surfacing (default OFF) ─────────
#[cfg(test)]
mod skill_discovery_reminder_tests {
    use super::*;
    use crate::test_support::{
        noop_hook_executor, MockApiClient, MockOutputStream, NoOpPermissionGate,
        StaticMemoryProvider,
    };
    use crate::OrchestratorConfig;
    use async_trait::async_trait;
    use protocol::ContentBlock;
    use skill_api::DiscoveredSkill;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tool_api::registry::ToolRegistry;

    /// A runtime that actually RUNS the spawned future so the one-shot resolves.
    struct InlineRuntime;
    #[async_trait]
    impl traits::RuntimeSpawner for InlineRuntime {
        async fn spawn(
            &self,
            name: &str,
            task: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'static>>,
        ) -> Result<traits::BackgroundTaskHandle, traits::RuntimeError> {
            tokio::spawn(task);
            Ok(traits::BackgroundTaskHandle {
                task_name: name.to_string(),
                task_id: 0,
            })
        }
        async fn sleep(&self, _d: std::time::Duration) {}
        async fn cancel(
            &self,
            _h: &traits::BackgroundTaskHandle,
        ) -> Result<(), traits::RuntimeError> {
            Ok(())
        }
    }

    fn skill(name: &str, description: &str) -> DiscoveredSkill {
        DiscoveredSkill {
            name: name.into(),
            description: description.into(),
            short_id: None,
        }
    }

    /// Build an orchestrator with NO skill prefetch wired (channel inert).
    fn orch_bare() -> ConversationOrchestrator {
        ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::with_files(vec![])),
            PathBuf::from("/work/repo"),
        )
    }

    /// Build an orchestrator whose skill prefetch resolves to `seed`.
    fn orch_with_seed(seed: Vec<DiscoveredSkill>) -> ConversationOrchestrator {
        let runtime: Arc<dyn traits::RuntimeSpawner> = Arc::new(InlineRuntime);
        let prefetch = Arc::new(skill_api::SkillDiscoveryPrefetch::with_fixed_result(
            runtime, seed,
        ));
        orch_bare().with_skill_discovery_prefetch(prefetch)
    }

    /// Push an assistant message that requested an `Edit` so `find_write_pivot`
    /// reports a write pivot and the prefetch fires.
    async fn push_write_pivot(orch: &ConversationOrchestrator) {
        let msg = ConversationMessage::Assistant {
            id: MessageId::new(),
            content: vec![ContentBlock::ToolUse {
                id: protocol::ToolUseId::new(),
                name: "Edit".into(),
                input: serde_json::json!({}),
                provider_id: None,
            }],
            stop_reason: None,
        };
        orch.session.lock().await.history.push(msg);
    }

    // (D.8) Flag OFF (no prefetch wired) = zero change: both calls are strict
    // no-ops and `has_skill_discovery_prefetch()` is false.
    #[tokio::test]
    async fn no_prefetch_wired_is_inert() {
        let orch = orch_bare();
        push_write_pivot(&orch).await;
        orch.start_skill_discovery_prefetch().await;
        assert!(orch.skill_discovery_reminder_message().await.is_none());
        assert!(!orch.has_skill_discovery_prefetch());
    }

    // (D.9) Flag ON = byte-exact attachment injected.
    #[tokio::test]
    async fn seeded_prefetch_renders_skill_discovery_block() {
        let orch = orch_with_seed(vec![
            skill("git-commit", "Commit staged changes"),
            skill("rebase", "Interactive rebase helper"),
        ]);
        assert!(orch.has_skill_discovery_prefetch());
        push_write_pivot(&orch).await;
        orch.start_skill_discovery_prefetch().await;
        let text = orch
            .skill_discovery_reminder_message()
            .await
            .expect("seeded prefetch must surface")
            .text_content();
        assert_eq!(
            text,
            "<system-reminder>\n\
             Skills relevant to your task:\n\n\
             - git-commit: Commit staged changes\n\
             - rebase: Interactive rebase helper\n\n\
             These skills encode project-specific conventions. \
             Invoke via Skill(\"<name>\") for complete instructions.\n\
             </system-reminder>"
        );
    }

    // (D.4 integration) Non-write iteration (no write-pivot tool) ⇒ inert even
    // with a seeded prefetch.
    #[tokio::test]
    async fn non_write_pivot_is_inert() {
        let orch = orch_with_seed(vec![skill("a", "da")]);
        // No assistant tool-use in history ⇒ find_write_pivot == false.
        orch.start_skill_discovery_prefetch().await;
        assert!(
            orch.skill_discovery_reminder_message().await.is_none(),
            "non-write iteration must surface nothing"
        );
    }

    // Empty result ⇒ None.
    #[tokio::test]
    async fn empty_result_yields_none() {
        let orch = orch_with_seed(vec![]);
        push_write_pivot(&orch).await;
        orch.start_skill_discovery_prefetch().await;
        assert!(orch.skill_discovery_reminder_message().await.is_none());
    }

    // Not armed (slot empty) ⇒ None.
    #[tokio::test]
    async fn not_armed_yields_none() {
        let orch = orch_with_seed(vec![skill("a", "da")]);
        assert!(orch.skill_discovery_reminder_message().await.is_none());
    }

    // (D.10) Dedup across turns: same skill armed turn N and N+1 ⇒ injects once.
    #[tokio::test]
    async fn surfaced_once_then_not_reinjected_across_turns() {
        let orch = orch_with_seed(vec![skill("a", "da")]);
        push_write_pivot(&orch).await;
        // Turn 0: surfaced.
        orch.start_skill_discovery_prefetch().await;
        assert!(
            orch.skill_discovery_reminder_message().await.is_some(),
            "first surfacing must inject"
        );
        // Turn 1: same skill ⇒ already in surfaced_skill_names ⇒ no re-inject.
        orch.start_skill_discovery_prefetch().await;
        assert!(
            orch.skill_discovery_reminder_message().await.is_none(),
            "an already-surfaced skill must not be re-injected"
        );
    }

    // Partial dedup: only the fresh skill surfaces on turn N+1.
    #[tokio::test]
    async fn partial_dedup_surfaces_only_fresh_skills() {
        let orch = orch_with_seed(vec![skill("seen", "ds")]);
        push_write_pivot(&orch).await;
        orch.start_skill_discovery_prefetch().await;
        assert!(orch.skill_discovery_reminder_message().await.is_some());

        // Re-seed the SAME prefetch slot is not possible (fixed_result is fixed);
        // instead assert the surfaced_skill_names set recorded "seen".
        assert!(orch.surfaced_skill_names.lock().await.contains("seen"));
    }
}

// ── Finding #80: refusal → fallback-model swap (maybe_swap_to_refusal_fallback) ──
#[cfg(test)]
mod refusal_fallback_tests {
    use super::*;
    use crate::test_support::{
        noop_hook_executor, MockApiClient, MockOutputStream, NoOpPermissionGate,
        StaticMemoryProvider,
    };
    use crate::OrchestratorConfig;
    use protocol::SessionId;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tool_api::registry::ToolRegistry;

    /// Build an orchestrator whose `refusal_fallback_model` is `cfg_fallback`,
    /// returning the orchestrator + a clone of its `MockOutputStream` (shares the
    /// same event buffer) so the test can inspect emitted warnings.
    fn orch_with_refusal_fallback(
        cfg_fallback: Option<&str>,
    ) -> (ConversationOrchestrator, MockOutputStream) {
        let out = MockOutputStream::new();
        let cfg = OrchestratorConfig {
            refusal_fallback_model: cfg_fallback.map(str::to_string),
            ..OrchestratorConfig::default()
        };
        let orch = ConversationOrchestrator::new(
            cfg,
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(out.clone()),
            Arc::new(StaticMemoryProvider::with_files(vec![])),
            PathBuf::from("/work/repo"),
        );
        (orch, out)
    }

    /// Build an orchestrator over a multi-hop refusal CASCADE.
    fn orch_with_refusal_chain(chain: &[&str]) -> (ConversationOrchestrator, MockOutputStream) {
        let out = MockOutputStream::new();
        let cfg = OrchestratorConfig {
            refusal_fallback_chain: chain.iter().map(|s| (*s).to_string()).collect(),
            ..OrchestratorConfig::default()
        };
        let orch = ConversationOrchestrator::new(
            cfg,
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(out.clone()),
            Arc::new(StaticMemoryProvider::with_files(vec![])),
            PathBuf::from("/work/repo"),
        );
        (orch, out)
    }

    /// The point of the cascade: successive refusals walk the chain instead of
    /// stopping after one hop. The once-per-session latch does NOT apply to a
    /// multi-hop chain — the chain itself is the bound.
    #[tokio::test]
    async fn a_cascade_walks_each_hop_in_order() {
        let (orch, _out) = orch_with_refusal_chain(&["hop-one", "hop-two"]);

        assert!(orch.maybe_swap_to_refusal_fallback().await, "first hop");
        assert_eq!(orch.session.lock().await.model, "hop-one");

        assert!(orch.maybe_swap_to_refusal_fallback().await, "second hop");
        assert_eq!(orch.session.lock().await.model, "hop-two");

        // Chain exhausted: every stage has been tried, so the walk declines
        // rather than looping back onto a model that already refused.
        assert!(
            !orch.maybe_swap_to_refusal_fallback().await,
            "an exhausted chain declines"
        );
        assert_eq!(orch.session.lock().await.model, "hop-two");
    }

    /// END TO END, and the whole point of steps 2-4: a multi-hop cascade emits
    /// ONE notice, naming where the session ended up — not one notice per hop
    /// announcing a model the cascade has already left.
    #[tokio::test]
    async fn a_cascade_emits_one_collapsed_notice_not_one_per_hop() {
        let (orch, out) = orch_with_refusal_chain(&["hop-one", "hop-two", "hop-three"]);

        assert!(orch.maybe_swap_to_refusal_fallback().await);
        assert!(
            out.text_events().await.is_empty(),
            "an intermediate hop is HELD, not announced"
        );

        assert!(orch.maybe_swap_to_refusal_fallback().await);
        assert!(
            out.text_events().await.is_empty(),
            "the second hop is still intermediate"
        );

        // The last hop has nothing remaining, so the episode settles and the
        // accumulated notice goes out — once.
        assert!(orch.maybe_swap_to_refusal_fallback().await);
        let texts = out.text_events().await;
        assert_eq!(texts.len(), 1, "exactly one notice for the whole cascade");
        assert!(
            texts[0].contains("hop-three"),
            "it names where the session ENDED UP: {}",
            texts[0]
        );
        assert!(
            !texts[0].contains("hop-one") && !texts[0].contains("hop-two"),
            "and not the hops it passed through: {}",
            texts[0]
        );
    }

    /// A single-hop fallback announces immediately — there is no later hop that
    /// could withdraw it, so holding it would just delay the user's notice.
    #[tokio::test]
    async fn a_single_hop_announces_immediately() {
        let (orch, out) = orch_with_refusal_chain(&["only"]);
        assert!(orch.maybe_swap_to_refusal_fallback().await);
        let texts = out.text_events().await;
        assert_eq!(texts.len(), 1);
        assert!(texts[0].contains("only"), "{}", texts[0]);
    }

    /// A ONE-element chain behaves exactly like the historical single
    /// `refusal_fallback_model`, latch included — the default path must be
    /// unchanged by the cascade's arrival.
    #[tokio::test]
    async fn a_single_hop_chain_still_latches_once_per_session() {
        let (orch, _out) = orch_with_refusal_chain(&["only"]);
        assert!(orch.maybe_swap_to_refusal_fallback().await);
        assert!(
            orch.refusal_fallback_latched
                .load(std::sync::atomic::Ordering::SeqCst),
            "a single-hop chain still sets the latch"
        );
        assert!(
            !orch.maybe_swap_to_refusal_fallback().await,
            "and the latch stops the second attempt"
        );
    }

    /// An empty chain defers to `refusal_fallback_model`, so a config written
    /// before the cascade existed keeps working and the two never disagree.
    #[tokio::test]
    async fn an_empty_chain_defers_to_the_single_model_field() {
        let (orch, _out) = orch_with_refusal_fallback(Some("legacy-model"));
        assert!(orch.maybe_swap_to_refusal_fallback().await);
        assert_eq!(orch.session.lock().await.model, "legacy-model");
    }

    /// The cascade's tried-models list resets with the latch, so a cleared
    /// session can walk the same chain again from its first hop.
    #[tokio::test]
    async fn clearing_the_session_lets_the_cascade_start_over() {
        let (orch, _out) = orch_with_refusal_chain(&["hop-one", "hop-two"]);
        assert!(orch.maybe_swap_to_refusal_fallback().await);
        assert!(orch.maybe_swap_to_refusal_fallback().await);
        assert!(!orch.maybe_swap_to_refusal_fallback().await);

        orch.refusal_tried_models.lock().await.clear();
        orch.refusal_fallback_latched
            .store(false, std::sync::atomic::Ordering::SeqCst);
        assert!(
            orch.maybe_swap_to_refusal_fallback().await,
            "a reset session walks the chain again"
        );
        assert_eq!(orch.session.lock().await.model, "hop-one");
    }

    #[tokio::test]
    async fn no_fallback_configured_is_a_strict_noop() {
        let (orch, out) = orch_with_refusal_fallback(None);
        let before = orch.session.lock().await.model.clone();
        assert!(
            !orch.maybe_swap_to_refusal_fallback().await,
            "no fallback → false"
        );
        assert_eq!(
            orch.session.lock().await.model,
            before,
            "model must NOT change"
        );
        assert!(out.text_events().await.is_empty(), "no warning emitted");
        assert!(
            !orch
                .refusal_fallback_latched
                .load(std::sync::atomic::Ordering::SeqCst),
            "latch must stay unset when nothing was configured"
        );
    }

    #[tokio::test]
    async fn swaps_once_then_latches() {
        let (orch, _out) = orch_with_refusal_fallback(Some("claude-sonnet-4-6"));
        // First refusal → swap.
        assert!(
            orch.maybe_swap_to_refusal_fallback().await,
            "first call swaps"
        );
        assert_eq!(
            orch.session.lock().await.model,
            "claude-sonnet-4-6",
            "session model must be swapped to the fallback"
        );
        assert!(
            orch.session.lock().await.model_profile.is_none(),
            "fallback model carries no provider profile"
        );
        // Second refusal → latched, no re-swap, terminal behavior preserved.
        assert!(
            !orch.maybe_swap_to_refusal_fallback().await,
            "second call is latched (no re-swap)"
        );
        assert_eq!(
            orch.session.lock().await.model,
            "claude-sonnet-4-6",
            "model unchanged on the second (latched) call"
        );
    }

    #[tokio::test]
    async fn emits_byte_exact_warning_on_swap() {
        let (orch, out) = orch_with_refusal_fallback(Some("claude-sonnet-4-6"));
        assert!(orch.maybe_swap_to_refusal_fallback().await);
        let texts = out.text_events().await;
        assert_eq!(texts.len(), 1, "exactly one warning emitted");
        // Byte-exact reproduction of 2.1.206 `VPn` for category == "other":
        // the generic `$7m`/`hmi` prefix, then "Switched to {Mf(fallback)}" — the
        // fallback's MARKETING NAME ("Sonnet 4.6"), not the raw id — then `bxr`.
        assert_eq!(
            texts[0],
            "This model's safeguards flagged this message. \
This sometimes happens with safe, normal conversations. Switched to Sonnet 4.6. \
Send feedback with /feedback or learn more: https://support.claude.com/en/articles/15363606"
        );
    }

    #[tokio::test]
    async fn clear_session_resets_refusal_fallback_latch() {
        let (orch, _out) = orch_with_refusal_fallback(Some("claude-sonnet-4-6"));
        assert!(
            orch.maybe_swap_to_refusal_fallback().await,
            "first session swaps"
        );

        <ConversationOrchestrator as traits::OrchestratorHandle>::clear_session(&orch)
            .await
            .expect("clear_session succeeds");

        assert!(
            !orch
                .refusal_fallback_latched
                .load(std::sync::atomic::Ordering::SeqCst),
            "clear_session must reset the per-session refusal fallback latch"
        );
        assert!(
            orch.maybe_swap_to_refusal_fallback().await,
            "a fresh cleared session must be able to swap on its first refusal"
        );
    }

    #[tokio::test]
    async fn resume_session_resets_refusal_fallback_latch() {
        let (orch, _out) = orch_with_refusal_fallback(Some("claude-sonnet-4-6"));
        assert!(
            orch.maybe_swap_to_refusal_fallback().await,
            "first session swaps"
        );

        <ConversationOrchestrator as traits::OrchestratorHandle>::resume_session(
            &orch,
            SessionId::new(),
            vec![],
            None,
            None,
            traits::ResumeRuntimeSnapshot::default(),
        )
        .await
        .expect("resume_session succeeds");

        assert!(
            !orch
                .refusal_fallback_latched
                .load(std::sync::atomic::Ordering::SeqCst),
            "resume_session must reset the per-session refusal fallback latch"
        );
        assert!(
            orch.maybe_swap_to_refusal_fallback().await,
            "a resumed session must be able to swap on its first refusal"
        );
    }
}

// ── `persist_message_to_jsonl_with_parent`: explicit parentUuid override ──────
//
// Proves that the streaming executor can parent each tool-result user message to
// the assistant message that REQUESTED the tool (TS `sourceToolAssistantUUID`),
// rather than the linear `last_jsonl_uuid` chain, by calling
// `persist_message_to_jsonl_with_parent(msg, Some(assistant_uuid))`.
//
// Also proves the `None` path (default chain) is byte-identical to the old
// `persist_message_to_jsonl` behaviour.
#[cfg(test)]
mod transcript_persistence_warning_tests {
    use super::*;
    use crate::test_support::{
        noop_hook_executor, MockApiClient, MockOutputStream, NoOpPermissionGate,
        StaticMemoryProvider,
    };
    use platform_posix::fs::PosixFileSystem;
    use std::sync::Arc;
    use tool_api::registry::ToolRegistry;

    fn failing_orchestrator(
        root: &std::path::Path,
    ) -> (ConversationOrchestrator, MockOutputStream) {
        // A directory at the transcript's file path deterministically makes
        // every append fail without relying on platform permission semantics.
        let transcript_path = root.join("transcript.jsonl");
        std::fs::create_dir(&transcript_path).expect("create blocking directory");
        let fs: Arc<dyn traits::FileSystem> = Arc::new(PosixFileSystem::new(root.to_path_buf()));
        let writer = Arc::new(session::jsonl::writer::JsonlWriter::new(
            transcript_path,
            fs,
        ));
        let output = MockOutputStream::new();
        let orch = ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(output.clone()),
            Arc::new(StaticMemoryProvider::empty()),
            root.to_path_buf(),
        )
        .with_jsonl_writer(writer);
        (orch, output)
    }

    #[tokio::test]
    async fn transcript_append_failure_is_silent_to_the_user() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (orch, output) = failing_orchestrator(dir.path());

        let user = ConversationMessage::user(MessageId::new(), "hello".into());
        orch.persist_message_to_jsonl(&user).await;
        orch.persist_active_goal_state_to_jsonl(None).await;

        let (boundary, metadata) = compaction::create_compact_boundary(
            compaction::CompactTrigger::Manual,
            1,
            None,
            None,
            Some(1),
            &[],
        );
        orch.persist_compact_boundary_to_jsonl(&boundary, &metadata)
            .await;

        let per_block = ConversationMessage::Assistant {
            id: MessageId::new(),
            content: vec![
                protocol::ContentBlock::Text {
                    text: "first".into(),
                },
                protocol::ContentBlock::Text {
                    text: "second".into(),
                },
            ],
            stop_reason: Some("end_turn".into()),
        };
        orch.persist_assistant_per_block(&per_block, None, None)
            .await;

        let merged = ConversationMessage::Assistant {
            id: MessageId::new(),
            content: vec![protocol::ContentBlock::Text {
                text: "merged".into(),
            }],
            stop_reason: Some("end_turn".into()),
        };
        orch.persist_assistant_merged(&merged, None, None).await;

        // CC 2.1.218 shows NO user-visible notice for a transcript-append
        // failure — the handling is log + telemetry only. Every persist path
        // above failed against the failing writer; none may surface a
        // SystemNotice to the user.
        let notice_count = output
            .snapshot()
            .await
            .into_iter()
            .filter(|event| matches!(event, traits::OutputEvent::SystemNotice { .. }))
            .count();
        assert_eq!(
            notice_count, 0,
            "transcript-append failures must not surface a user-visible notice"
        );
    }
}

#[cfg(test)]
mod hook_attachment_persistence_tests {
    use super::*;
    use crate::test_support::{
        noop_hook_executor, MockApiClient, MockOutputStream, NoOpPermissionGate,
        StaticMemoryProvider,
    };
    use crate::OrchestratorConfig;
    use platform_posix::fs::PosixFileSystem;
    use std::sync::Arc;
    use tool_api::registry::ToolRegistry;
    /// Frame buffering holds `tool_result` frames until the collection point
    /// releases them, so the SDK sees RECEIVED order rather than completion
    /// order — and so a cancelled tool reports its synthetic instead of the
    /// real outcome the executor discarded.
    ///
    /// These pin the mechanism. They do NOT prove the streaming driver's
    /// ordering end-to-end; that would need two tools whose completion order
    /// differs from their received order.
    mod tool_frame_ordering_tests {
        use super::*;

        fn orch_for_frames(output: Arc<MockOutputStream>) -> ConversationOrchestrator {
            ConversationOrchestrator::new(
                OrchestratorConfig::default(),
                Arc::new(MockApiClient::new(vec![])),
                Arc::new(ToolRegistry::new()),
                noop_hook_executor(),
                Arc::new(NoOpPermissionGate),
                output,
                Arc::new(StaticMemoryProvider::empty()),
                std::env::temp_dir(),
            )
        }

        fn result_ids(events: &[traits::orchestrator::OutputEvent]) -> Vec<String> {
            events
                .iter()
                .filter_map(|e| match e {
                    traits::orchestrator::OutputEvent::ToolResult { id, .. } => {
                        Some(id.to_string())
                    }
                    _ => None,
                })
                .collect()
        }

        /// With buffering OFF the frame goes straight out — the batched driver,
        /// which dispatches in received order anyway, is unchanged.
        #[tokio::test]
        async fn buffering_off_emits_immediately() {
            let output = Arc::new(MockOutputStream::new());
            let orch = orch_for_frames(output.clone());
            let id = protocol::ToolUseId::new();
            orch.emit_tool_result_frame(&id, "Bash", "out", &serde_json::json!({}), None)
                .await;
            assert_eq!(result_ids(&output.snapshot().await), vec![id.to_string()]);
        }

        /// With buffering ON nothing reaches the stream until release.
        #[tokio::test]
        async fn buffered_frames_are_withheld_then_released_in_caller_order() {
            let output = Arc::new(MockOutputStream::new());
            let orch = orch_for_frames(output.clone());
            orch.set_tool_frame_buffering(true).await;

            let first = protocol::ToolUseId::new();
            let second = protocol::ToolUseId::new();
            // Buffered in COMPLETION order: `second` finished first.
            orch.emit_tool_result_frame(&second, "Bash", "b", &serde_json::json!({}), None)
                .await;
            orch.emit_tool_result_frame(&first, "Read", "a", &serde_json::json!({}), None)
                .await;
            assert!(
                result_ids(&output.snapshot().await).is_empty(),
                "nothing may reach the stream while buffering is on"
            );

            // Released in RECEIVED order by the collection point.
            orch.release_tool_frame(&first, "a", false).await;
            orch.release_tool_frame(&second, "b", false).await;
            assert_eq!(
                result_ids(&output.snapshot().await),
                vec![first.to_string(), second.to_string()],
                "release order wins over completion order"
            );
        }

        /// A tool that never dispatched (queued, then cancelled) has no buffered
        /// frame and still gets one. Before the collection point released
        /// frames, this case emitted nothing at all.
        #[tokio::test]
        async fn releasing_an_undispatched_tool_still_emits() {
            let output = Arc::new(MockOutputStream::new());
            let orch = orch_for_frames(output.clone());
            orch.set_tool_frame_buffering(true).await;

            let id = protocol::ToolUseId::new();
            orch.release_tool_frame(&id, "The user doesn't want to proceed", true)
                .await;
            assert_eq!(
                result_ids(&output.snapshot().await),
                vec![id.to_string()],
                "a never-dispatched tool must still report a frame"
            );
        }

        /// The released content is the block's FINAL text, so a synthetic that
        /// replaced a cancelled tool's real outcome wins over what dispatch
        /// buffered.
        #[tokio::test]
        async fn substituted_content_wins_over_the_buffered_result() {
            let output = Arc::new(MockOutputStream::new());
            let orch = orch_for_frames(output.clone());
            orch.set_tool_frame_buffering(true).await;

            let id = protocol::ToolUseId::new();
            orch.emit_tool_result_frame(
                &id,
                "Bash",
                "REAL OUTPUT",
                &serde_json::json!({ "stdout": "REAL OUTPUT" }),
                None,
            )
            .await;
            orch.release_tool_frame(&id, "SYNTHETIC", true).await;

            let events = output.snapshot().await;
            let found = events.iter().any(|e| matches!(
                e,
                traits::orchestrator::OutputEvent::ToolResult { id: gid, .. } if gid.to_string() == id.to_string()
            ));
            assert!(found, "the released frame must be emitted");
            assert!(
                !format!("{events:?}").contains("REAL OUTPUT"),
                "the discarded real outcome must not reach the SDK: {events:?}"
            );
        }
    }

    fn orch_with_writer(
        dir: &std::path::Path,
        path: std::path::PathBuf,
    ) -> ConversationOrchestrator {
        let fs: Arc<dyn traits::FileSystem> = Arc::new(PosixFileSystem::new(dir.to_path_buf()));
        let writer = Arc::new(session::jsonl::writer::JsonlWriter::new(path, fs));
        ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            dir.to_path_buf(),
        )
        .with_jsonl_writer(writer)
    }

    /// A hook-run attachment lands as its own `type:"attachment"` transcript
    /// line whose payload rides in the `attachment` key BEFORE `type`, and it
    /// advances the chain so the next line parents to it.
    #[tokio::test]
    async fn hook_attachment_is_persisted_as_an_attachment_line() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("session.jsonl");
        let orch = orch_with_writer(dir.path(), path.clone());

        let payload = hooks::success_attachment(
            &hooks::HookAttachmentIdentity {
                hook_name: "PostToolUse:Bash".into(),
                hook_event: "PostToolUse".into(),
                tool_use_id: "toolu_01ApkBwAZMCAza47B5nAWiGS".into(),
            },
            "formatted",
            "formatted\n",
            "",
            0,
            "./hooks/fmt.sh",
            37,
        );
        orch.persist_hook_attachment_to_jsonl(payload.clone()).await;

        let raw = std::fs::read_to_string(&path).expect("read jsonl");
        let lines: Vec<&str> = raw.lines().filter(|l| !l.trim().is_empty()).collect();
        assert_eq!(lines.len(), 1, "one line: {raw}");
        let v: serde_json::Value = serde_json::from_str(lines[0]).expect("json");
        assert_eq!(v["type"], "attachment");
        assert_eq!(v["attachment"], payload);
        assert!(v.get("message").is_none(), "no inner message: {}", lines[0]);
        // Payload precedes the discriminator on real 2.1.220 attachment lines.
        let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(
            &keys[..4],
            ["parentUuid", "isSidechain", "attachment", "type"]
        );
        // Chain advanced.
        assert_eq!(
            orch.last_jsonl_uuid.lock().await.as_deref(),
            v["uuid"].as_str()
        );
    }

    /// O3: attachments queued during a tool dispatch are flushed — in
    /// production order, as `attachment` lines — AFTER the `tool_result` they
    /// follow, matching claude's stream order (`insertMessageChain` writes the
    /// yielded attachment message right after the yielded tool_result), and the
    /// queue is drained so a second flush is a no-op.
    #[tokio::test]
    async fn queued_hook_attachments_flush_after_the_tool_result_in_order() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("session.jsonl");
        let orch = orch_with_writer(dir.path(), path.clone());

        let tuid = protocol::ToolUseId::new();
        orch.queue_hook_attachment(
            &tuid,
            hooks::additional_context_attachment(
                "PostToolUse:Edit",
                tuid.as_str(),
                "PostToolUse",
                &["FIRST".to_string()],
            ),
        )
        .await;
        orch.queue_hook_attachment(
            &tuid,
            hooks::error_during_execution_attachment(
                "SECOND",
                "PostToolUse:Edit",
                tuid.as_str(),
                "PostToolUse",
            ),
        )
        .await;

        let msg = ConversationMessage::User {
            id: protocol::MessageId::new(),
            content: vec![protocol::ContentBlock::ToolResult {
                tool_use_id: tuid.clone(),
                content: "ok".into(),
                is_error: false,
                provider_tool_use_id: None,
                content_blocks: None,
            }],
            is_meta: false,
            is_compact_summary: false,
            is_visible_in_transcript_only: false,
        };
        orch.persist_message_to_jsonl(&msg).await;
        orch.flush_hook_attachments(&tuid).await;
        // Draining: a second flush writes nothing.
        orch.flush_hook_attachments(&tuid).await;

        let raw = std::fs::read_to_string(&path).expect("read jsonl");
        let lines: Vec<&str> = raw.lines().filter(|l| !l.trim().is_empty()).collect();
        assert_eq!(lines.len(), 3, "tool_result + 2 attachments: {raw}");
        let v0: serde_json::Value = serde_json::from_str(lines[0]).expect("json");
        let v1: serde_json::Value = serde_json::from_str(lines[1]).expect("json");
        let v2: serde_json::Value = serde_json::from_str(lines[2]).expect("json");
        assert_eq!(v0["type"], "user");
        assert_eq!(v1["attachment"]["type"], "hook_additional_context");
        assert_eq!(v1["attachment"]["content"][0], "FIRST");
        assert_eq!(v2["attachment"]["type"], "hook_error_during_execution");
        assert_eq!(v2["attachment"]["content"], "SECOND");
        // Linear chain: result → first attachment → second attachment.
        assert_eq!(v1["parentUuid"], v0["uuid"]);
        assert_eq!(v2["parentUuid"], v1["uuid"]);
    }

    /// END-TO-END: a real `HookExecutorImpl` wired with the real sink and a
    /// real orchestrator writes ONE `attachment` transcript line for the hook
    /// run — the whole publish chain (executor → sink → JSONL writer), not just
    /// each half. Guards against the value being computed but never persisted.
    #[tokio::test]
    async fn executor_run_reaches_the_transcript_through_the_real_sink() {
        use async_trait::async_trait;
        use hooks::executor::BuiltinHookHandler;
        use hooks::registry::{HookContext, HookRegistry};
        use hooks::{HookOutcome, HookResult};

        struct Ok0;
        #[async_trait]
        impl BuiltinHookHandler for Ok0 {
            async fn handle(&self, _event: &hooks::HookEvent, _ctx: &HookContext) -> HookResult {
                HookResult {
                    outcome: HookOutcome::Success,
                    stdout: "linted".into(),
                    stderr: String::new(),
                    exit_code: Some(0),
                    response: None,
                }
            }
            fn id(&self) -> &str {
                "lint"
            }
        }

        struct UnusedHttp;
        #[async_trait]
        impl traits::HttpTransport for UnusedHttp {
            async fn request(
                &self,
                _req: protocol::HttpRequest,
            ) -> Result<protocol::HttpResponse, traits::HttpError> {
                Err(traits::HttpError::InvalidRequest("unused".into()))
            }
            async fn stream_sse(
                &self,
                _req: protocol::HttpRequest,
            ) -> Result<traits::http::SseStream, traits::HttpError> {
                Err(traits::HttpError::InvalidRequest("unused".into()))
            }
        }
        struct UnusedRuntime;
        #[async_trait]
        impl traits::RuntimeSpawner for UnusedRuntime {
            async fn spawn(
                &self,
                _name: &str,
                _task: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'static>>,
            ) -> Result<traits::BackgroundTaskHandle, traits::RuntimeError> {
                Err(traits::RuntimeError::Internal("unused".into()))
            }
            async fn sleep(&self, _d: std::time::Duration) {}
            async fn cancel(
                &self,
                _h: &traits::BackgroundTaskHandle,
            ) -> Result<(), traits::RuntimeError> {
                Ok(())
            }
        }

        let mut registry = HookRegistry::new();
        registry.register(hooks::HookDefinition {
            id: protocol::HookId::new(),
            name: "lint".into(),
            events: vec![hooks::HookEventType::PostToolUse],
            if_condition: None,
            executor: hooks::HookExecutor::Builtin {
                handler_id: "lint".into(),
            },
            source: hooks::HookSource::User,
            blocking: true,
            timeout: None,
            priority: 0,
            once: false,
            status_message: None,
            async_rewake: false,
            async_timeout: None,
            rewake_message: None,
        });

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("session.jsonl");
        let sink = Arc::new(crate::JsonlHookAttachmentSink::new());

        let mut exec = hooks::HookExecutorImpl::new(
            Arc::new(tokio::sync::RwLock::new(registry)),
            Arc::new(UnusedHttp),
            Arc::new(UnusedRuntime),
        )
        .with_attachment_sink(sink.clone() as Arc<dyn hooks::HookAttachmentSink>);
        exec.register_builtin(Arc::new(Ok0));

        let orch = Arc::new(orch_with_writer(dir.path(), path.clone()));
        sink.attach(&orch);

        exec.execute(
            hooks::HookEvent::PostToolUse {
                tool_name: "Edit".into(),
                tool_input: serde_json::json!({}),
                tool_output: serde_json::json!({}),
                tool_use_id: protocol::ToolUseId::from("toolu_e2e".to_string()),
                duration_ms: None,
            },
            HookContext::default(),
        )
        .await;

        let raw = std::fs::read_to_string(&path).expect("transcript written");
        let lines: Vec<&str> = raw.lines().filter(|l| !l.trim().is_empty()).collect();
        assert_eq!(lines.len(), 1, "exactly one attachment line: {raw}");
        let v: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(v["type"], "attachment");
        assert_eq!(v["attachment"]["type"], "hook_success");
        assert_eq!(v["attachment"]["hookName"], "PostToolUse:Edit");
        assert_eq!(v["attachment"]["hookEvent"], "PostToolUse");
        assert_eq!(v["attachment"]["toolUseID"], "toolu_e2e");
        assert_eq!(v["attachment"]["content"], "linted");
        assert_eq!(v["attachment"]["exitCode"], 0);
        assert_eq!(v["attachment"]["command"], "lint");
    }

    /// The sink adapter forwards to the orchestrator once attached, and is an
    /// inert no-op before that (composition order: the hook executor is built
    /// before the orchestrator exists).
    #[tokio::test]
    async fn sink_forwards_to_the_attached_orchestrator() {
        use hooks::HookAttachmentSink;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("session.jsonl");
        let sink = Arc::new(crate::JsonlHookAttachmentSink::new());

        // Unattached: must not panic, must not write.
        sink.record(serde_json::json!({"type": "hook_success"}))
            .await;
        assert!(!path.exists(), "unattached sink writes nothing");

        let orch = Arc::new(orch_with_writer(dir.path(), path.clone()));
        sink.attach(&orch);
        sink.record(serde_json::json!({"type": "hook_cancelled"}))
            .await;

        let raw = std::fs::read_to_string(&path).expect("read jsonl");
        assert!(
            raw.contains(r#""attachment":{"type":"hook_cancelled"}"#),
            "sink persisted the payload: {raw}"
        );
    }

    #[tokio::test]
    async fn sink_atomically_persists_oversized_hook_output_in_session_storage() {
        use hooks::HookAttachmentSink;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("session.jsonl");
        let sink = Arc::new(crate::JsonlHookAttachmentSink::new());
        let orch =
            Arc::new(orch_with_writer(dir.path(), path).with_config_home(dir.path().to_path_buf()));
        let session_uuid = orch.session.lock().await.session_id.as_uuid().to_string();
        sink.attach(&orch);
        let body = "x".repeat(hooks::attachment::HOOK_OUTPUT_INLINE_LIMIT + 1);

        let reference = sink
            .persist_large_output(&body)
            .await
            .expect("persisted reference");
        let output_dir = session::jsonl::path::tool_results_dir(
            dir.path(),
            &orch.current_cwd().to_string_lossy(),
            &session_uuid,
        );
        let files = std::fs::read_dir(output_dir)
            .expect("tool-results directory")
            .collect::<Result<Vec<_>, _>>()
            .expect("tool-results entries");
        assert_eq!(files.len(), 1);
        let saved = files[0].path();
        assert!(reference.contains(&saved.to_string_lossy().to_string()));
        assert_eq!(std::fs::read_to_string(saved).expect("full output"), body);
    }
}

#[cfg(test)]
mod persist_with_parent_tests {
    use super::*;
    use crate::test_support::{
        noop_hook_executor, MockApiClient, MockOutputStream, NoOpPermissionGate,
        StaticMemoryProvider,
    };
    use crate::OrchestratorConfig;
    use platform_posix::fs::PosixFileSystem;
    use session::jsonl::schema::JsonlMessage;
    use std::sync::Arc;
    use tool_api::registry::ToolRegistry;

    /// Build an orchestrator wired with a `JsonlWriter` backed by `path`.
    fn orch_with_writer(
        dir: &std::path::Path,
        path: std::path::PathBuf,
    ) -> ConversationOrchestrator {
        let fs: Arc<dyn traits::FileSystem> = Arc::new(PosixFileSystem::new(dir.to_path_buf()));
        let writer = Arc::new(session::jsonl::writer::JsonlWriter::new(path, fs));
        ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            dir.to_path_buf(),
        )
        .with_jsonl_writer(writer)
    }

    /// Read all JSONL lines back from disk and deserialize.
    fn read_jsonl(path: &std::path::Path) -> Vec<JsonlMessage> {
        let raw = std::fs::read_to_string(path).expect("read jsonl");
        raw.lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str::<JsonlMessage>(l).expect("deserialize jsonl line"))
            .collect()
    }

    // ── test 1: explicit parent_override ─────────────────────────────────────

    #[tokio::test]
    async fn tool_result_parents_to_explicit_assistant_uuid() {
        let dir = tempfile::tempdir().expect("tempdir");
        let session_path = dir.path().join("session.jsonl");
        let orch = orch_with_writer(dir.path(), session_path.clone());

        // Persist an assistant message first (linear chain — no override).
        let asst_msg = ConversationMessage::Assistant {
            id: protocol::MessageId::new(),
            content: vec![protocol::ContentBlock::Text {
                text: "I will call a tool".into(),
            }],
            stop_reason: Some("tool_use".into()),
        };
        orch.persist_message_to_jsonl(&asst_msg).await;

        // Capture the assistant line's uuid from disk.
        let lines_after_asst = read_jsonl(&session_path);
        assert_eq!(
            lines_after_asst.len(),
            1,
            "expected 1 line (the assistant message)"
        );
        let assistant_uuid = lines_after_asst[0].uuid.clone();

        // Persist a tool-result user message via the override variant, passing
        // the assistant's uuid explicitly — simulates streaming executor parenting.
        let tool_result_msg =
            ConversationMessage::user(protocol::MessageId::new(), "tool result body".into());
        orch.persist_message_to_jsonl_with_parent(&tool_result_msg, Some(assistant_uuid.clone()))
            .await;

        // Read back both lines.
        let lines = read_jsonl(&session_path);
        assert_eq!(lines.len(), 2, "expected 2 lines (assistant + tool_result)");
        let tool_result_line = &lines[1];

        // THE KEY ASSERTION: the tool-result line's parentUuid must equal the
        // assistant's uuid, NOT the prior last_jsonl_uuid (which also happens to
        // be the assistant uuid here, but the next test distinguishes them).
        assert_eq!(
            tool_result_line.parent_uuid.as_deref(),
            Some(assistant_uuid.as_str()),
            "tool_result parentUuid must equal the explicit assistant uuid override"
        );
    }

    // ── test 2: override bypasses last_jsonl_uuid ─────────────────────────────
    //
    // Three messages: user → assistant → tool_result(override=user_uuid).
    // Without the override, the tool_result would parent to the assistant.
    // With the override it must parent to the user uuid instead, proving the
    // override takes effect independent of what `last_jsonl_uuid` holds.

    #[tokio::test]
    async fn override_bypasses_last_jsonl_uuid_chain() {
        let dir = tempfile::tempdir().expect("tempdir");
        let session_path = dir.path().join("session.jsonl");
        let orch = orch_with_writer(dir.path(), session_path.clone());

        // 1. Persist a user message (no override).
        let user_msg = ConversationMessage::user(protocol::MessageId::new(), "user prompt".into());
        orch.persist_message_to_jsonl(&user_msg).await;
        let lines = read_jsonl(&session_path);
        let user_uuid = lines[0].uuid.clone();

        // 2. Persist an assistant message (no override → chains off user).
        let asst_msg = ConversationMessage::Assistant {
            id: protocol::MessageId::new(),
            content: vec![protocol::ContentBlock::Text {
                text: "ok calling tool".into(),
            }],
            stop_reason: Some("tool_use".into()),
        };
        orch.persist_message_to_jsonl(&asst_msg).await;
        let lines = read_jsonl(&session_path);
        assert_eq!(lines[1].parent_uuid.as_deref(), Some(user_uuid.as_str()));
        let _asst_uuid = lines[1].uuid.clone();

        // 3. Persist a tool-result user message with an EXPLICIT override pointing
        //    back to the user_uuid (unusual, but proves the override wins over
        //    last_jsonl_uuid which currently holds the assistant uuid).
        let tool_result_msg =
            ConversationMessage::user(protocol::MessageId::new(), "tool result".into());
        orch.persist_message_to_jsonl_with_parent(&tool_result_msg, Some(user_uuid.clone()))
            .await;

        let lines = read_jsonl(&session_path);
        assert_eq!(lines.len(), 3, "expected 3 lines");
        assert_eq!(
            lines[2].parent_uuid.as_deref(),
            Some(user_uuid.as_str()),
            "override must win over last_jsonl_uuid (which holds the assistant uuid)"
        );
    }

    // ── test 3: None path advances last_jsonl_uuid (regression) ──────────────
    //
    // Proves `persist_message_to_jsonl_with_parent(msg, None)` is byte-identical
    // to the old `persist_message_to_jsonl`: two messages with None form a
    // monotonic chain where msg2.parentUuid == msg1.uuid.

    #[tokio::test]
    async fn none_override_chains_off_last_jsonl_uuid() {
        let dir = tempfile::tempdir().expect("tempdir");
        let session_path = dir.path().join("session.jsonl");
        let orch = orch_with_writer(dir.path(), session_path.clone());

        let msg1 = ConversationMessage::user(protocol::MessageId::new(), "first message".into());
        orch.persist_message_to_jsonl_with_parent(&msg1, None).await;

        let msg2 = ConversationMessage::user(protocol::MessageId::new(), "second message".into());
        orch.persist_message_to_jsonl_with_parent(&msg2, None).await;

        let lines = read_jsonl(&session_path);
        assert_eq!(lines.len(), 2, "expected 2 JSONL lines");
        // First entry: root of chain → parent_uuid is None.
        assert_eq!(
            lines[0].parent_uuid, None,
            "first entry must have no parent"
        );
        // Second entry: must chain off the first.
        assert_eq!(
            lines[1].parent_uuid.as_deref(),
            Some(lines[0].uuid.as_str()),
            "second entry parentUuid must equal first entry uuid (linear chain)"
        );
    }

    // ── test 4: last_jsonl_uuid advances after override ──────────────────────
    //
    // After an overridden persist, `last_jsonl_uuid` is still advanced to the
    // newly-persisted line's uuid. A subsequent non-overridden line must chain
    // off the overridden line (not off whatever the override pointed to).

    #[tokio::test]
    async fn last_jsonl_uuid_advances_after_override() {
        let dir = tempfile::tempdir().expect("tempdir");
        let session_path = dir.path().join("session.jsonl");
        let orch = orch_with_writer(dir.path(), session_path.clone());

        // 1. First message (no override) — root.
        let msg1 = ConversationMessage::user(protocol::MessageId::new(), "root".into());
        orch.persist_message_to_jsonl(&msg1).await;
        let lines = read_jsonl(&session_path);
        let root_uuid = lines[0].uuid.clone();

        // 2. Overridden message pointing back to root — simulates a tool result.
        let msg2 = ConversationMessage::user(protocol::MessageId::new(), "overridden".into());
        orch.persist_message_to_jsonl_with_parent(&msg2, Some(root_uuid.clone()))
            .await;
        let lines = read_jsonl(&session_path);
        let overridden_uuid = lines[1].uuid.clone();
        // Verify the override took effect.
        assert_eq!(
            lines[1].parent_uuid.as_deref(),
            Some(root_uuid.as_str()),
            "overridden line must parent to root, not to itself"
        );

        // 3. Third message (no override) — must chain off msg2 (the overridden line),
        //    not off msg1 (root). This confirms last_jsonl_uuid was advanced.
        let msg3 = ConversationMessage::user(protocol::MessageId::new(), "subsequent".into());
        orch.persist_message_to_jsonl(&msg3).await;
        let lines = read_jsonl(&session_path);
        assert_eq!(lines.len(), 3, "expected 3 JSONL lines");
        assert_eq!(
            lines[2].parent_uuid.as_deref(),
            Some(overridden_uuid.as_str()),
            "subsequent non-overridden line must chain off the overridden line"
        );
    }

    /// A denied tool's persisted `user` line carries `toolDenialKind`, in
    /// claude's slot (after `timestamp`, before the `userType` trailer).
    ///
    /// The kind is recorded against the `tool_use_id` at the permission
    /// decision and stamped here, mirroring claude's message-level field. The
    /// exactly-one-`tool_result` guard is claude's own (`Tpr`): a message
    /// carrying several tool_results cannot attribute one kind, so it gets none.
    #[tokio::test]
    async fn denied_tool_result_line_carries_tool_denial_kind() {
        let dir = tempfile::tempdir().expect("tempdir");
        let session_path = dir.path().join("session.jsonl");
        let orch = orch_with_writer(dir.path(), session_path.clone());

        let tuid = protocol::ToolUseId::new();
        orch.record_tool_denial_kind(&tuid, "permission-rule").await;

        let msg = ConversationMessage::User {
            id: protocol::MessageId::new(),
            content: vec![protocol::ContentBlock::ToolResult {
                tool_use_id: tuid.clone(),
                content: "Permission to use Bash has been denied.".into(),
                is_error: true,
                provider_tool_use_id: None,
                content_blocks: None,
            }],
            is_meta: false,
            is_compact_summary: false,
            is_visible_in_transcript_only: false,
        };
        orch.persist_message_to_jsonl(&msg).await;

        let raw = std::fs::read_to_string(&session_path).expect("session file");
        let line = raw.lines().next().expect("one line");
        assert!(
            line.contains(r#""toolDenialKind":"permission-rule""#),
            "denied line must carry the kind, got: {line}"
        );
        let i_kind = line.find("toolDenialKind").expect("kind present");
        let i_trailer = line.find("userType").expect("trailer present");
        assert!(
            i_kind < i_trailer,
            "toolDenialKind must precede the common trailer, got: {line}"
        );
    }

    /// O1: a tool_result `user` line carries the tool's STRUCTURED result as
    /// `toolUseResult` plus `sourceToolAssistantUUID` (== `parentUuid`).
    ///
    /// Oracle: the success arm at 2.1.220 BIN off **235420375** builds
    /// `zr({content:Ft, …, toolUseResult: gt, …, sourceToolAssistantUUID:
    /// i.uuid})` where `gt = se.data` is the tool's raw structured result
    /// (NOT the model-facing string). The writer `insertMessageChain`
    /// (BIN off **237862200**) then DERIVES `parentUuid` from
    /// `sourceToolAssistantUUID`, which is why the two are equal on all
    /// 96 794 real 2.1.220 lines carrying the field.
    #[tokio::test]
    async fn tool_result_line_carries_structured_result_and_source_assistant_uuid() {
        let dir = tempfile::tempdir().expect("tempdir");
        let session_path = dir.path().join("session.jsonl");
        let orch = orch_with_writer(dir.path(), session_path.clone());

        let tuid = protocol::ToolUseId::new();
        // 1. The assistant line owning this tool_use.
        let assistant = ConversationMessage::Assistant {
            id: protocol::MessageId::new(),
            content: vec![protocol::ContentBlock::ToolUse {
                id: tuid.clone(),
                name: "Bash".into(),
                input: serde_json::json!({"command":"ls"}),
                provider_id: None,
            }],
            stop_reason: None,
        };
        let map = orch
            .persist_assistant_per_block(&assistant, None, None)
            .await;
        let assistant_uuid = map
            .get(&tuid)
            .cloned()
            .expect("tool_use line uuid recorded");

        // 2. The tool's structured result, recorded at dispatch.
        orch.record_tool_use_result(
            &tuid,
            serde_json::json!({"stdout":"a\n","stderr":"","interrupted":false}),
        )
        .await;

        let msg = ConversationMessage::User {
            id: protocol::MessageId::new(),
            content: vec![protocol::ContentBlock::ToolResult {
                tool_use_id: tuid.clone(),
                content: "a".into(),
                is_error: false,
                provider_tool_use_id: None,
                content_blocks: None,
            }],
            is_meta: false,
            is_compact_summary: false,
            is_visible_in_transcript_only: false,
        };
        orch.persist_message_to_jsonl_with_parent(&msg, Some(assistant_uuid.clone()))
            .await;

        let raw = std::fs::read_to_string(&session_path).expect("session file");
        let line = raw.lines().nth(1).expect("tool_result line");
        assert!(
            line.contains(r#""toolUseResult":{"stdout":"a\n","stderr":"","interrupted":false}"#),
            "structured result must ride verbatim, got: {line}"
        );
        assert!(
            line.contains(&format!(r#""sourceToolAssistantUUID":"{assistant_uuid}""#)),
            "source assistant uuid must be the tool_use's own line uuid, got: {line}"
        );
        assert!(
            line.contains(&format!(r#""parentUuid":"{assistant_uuid}""#)),
            "parentUuid must equal sourceToolAssistantUUID, got: {line}"
        );
        let i_res = line.find("toolUseResult").expect("result present");
        let i_src = line
            .find("sourceToolAssistantUUID")
            .expect("source present");
        let i_trailer = line.find("userType").expect("trailer present");
        assert!(
            i_res < i_src && i_src < i_trailer,
            "head order must be toolUseResult < sourceToolAssistantUUID < trailer, got: {line}"
        );
    }

    /// O1: a FAILED tool's `toolUseResult` is the plain string
    /// `` `Error: ${message}` ``, NOT the `{"error":…}` object the port sends
    /// on its stream-json wire (2.1.220 BIN off **235424595**).
    #[tokio::test]
    async fn error_tool_result_persists_the_bare_error_string() {
        let dir = tempfile::tempdir().expect("tempdir");
        let session_path = dir.path().join("session.jsonl");
        let orch = orch_with_writer(dir.path(), session_path.clone());

        let tuid = protocol::ToolUseId::new();
        orch.record_tool_use_result(&tuid, serde_json::Value::String("Error: boom".into()))
            .await;

        let msg = ConversationMessage::User {
            id: protocol::MessageId::new(),
            content: vec![protocol::ContentBlock::ToolResult {
                tool_use_id: tuid.clone(),
                content: "Error: boom".into(),
                is_error: true,
                provider_tool_use_id: None,
                content_blocks: None,
            }],
            is_meta: false,
            is_compact_summary: false,
            is_visible_in_transcript_only: false,
        };
        orch.persist_message_to_jsonl(&msg).await;

        let raw = std::fs::read_to_string(&session_path).expect("session file");
        assert!(
            raw.contains(r#""toolUseResult":"Error: boom""#),
            "error result must be the bare string, got: {raw}"
        );
        assert!(
            !raw.contains(r#""toolUseResult":{"error""#),
            "the {{error:…}} object is the stream-json wire, not the transcript"
        );
    }

    /// O1: an MCP tool's `mcpMeta` is a TOP-LEVEL sibling between
    /// `toolDenialKind` and `sourceToolAssistantUUID`, never nested inside
    /// `toolUseResult` (2.1.220 BIN off **232969604**: on the main chain
    /// `Uks(undefined, meta)` returns the raw meta verbatim).
    #[tokio::test]
    async fn mcp_result_line_carries_mcp_meta_as_a_top_level_sibling() {
        let dir = tempfile::tempdir().expect("tempdir");
        let session_path = dir.path().join("session.jsonl");
        let orch = orch_with_writer(dir.path(), session_path.clone());

        let tuid = protocol::ToolUseId::new();
        orch.record_tool_use_result(&tuid, serde_json::json!([{"type":"text","text":"hi"}]))
            .await;
        orch.record_tool_use_mcp_meta(&tuid, serde_json::json!({"_meta":{"claude/endTurn":true}}))
            .await;

        let msg = ConversationMessage::User {
            id: protocol::MessageId::new(),
            content: vec![protocol::ContentBlock::ToolResult {
                tool_use_id: tuid.clone(),
                content: "hi".into(),
                is_error: false,
                provider_tool_use_id: None,
                content_blocks: None,
            }],
            is_meta: false,
            is_compact_summary: false,
            is_visible_in_transcript_only: false,
        };
        orch.persist_message_to_jsonl(&msg).await;

        let raw = std::fs::read_to_string(&session_path).expect("session file");
        let line = raw.lines().next().expect("one line");
        assert!(
            line.contains(r#""mcpMeta":{"_meta":{"claude/endTurn":true}}"#),
            "mcpMeta must ride verbatim, got: {line}"
        );
        let i_res = line.find(r#""toolUseResult""#).expect("result present");
        let i_mcp = line.find(r#""mcpMeta""#).expect("meta present");
        let i_trailer = line.find("userType").expect("trailer present");
        assert!(
            i_res < i_mcp && i_mcp < i_trailer,
            "mcpMeta is a sibling AFTER toolUseResult and before the trailer, got: {line}"
        );
    }

    /// O1: the exactly-one-`tool_result` guard (claude's `Tpr`) applies to
    /// EVERY tool-result head key, not just `toolDenialKind` — a batched user
    /// message carrying two results cannot attribute one message-level value.
    #[tokio::test]
    async fn two_tool_results_in_one_user_message_get_no_head_keys() {
        let dir = tempfile::tempdir().expect("tempdir");
        let session_path = dir.path().join("session.jsonl");
        let orch = orch_with_writer(dir.path(), session_path.clone());

        let a = protocol::ToolUseId::new();
        let b = protocol::ToolUseId::new();
        orch.record_tool_use_result(&a, serde_json::json!({"stdout":"a"}))
            .await;
        orch.record_tool_use_result(&b, serde_json::json!({"stdout":"b"}))
            .await;
        orch.record_source_tool_assistant_uuid(&a, "aaa".into())
            .await;

        let mk = |id: protocol::ToolUseId| protocol::ContentBlock::ToolResult {
            tool_use_id: id,
            content: "x".into(),
            is_error: false,
            provider_tool_use_id: None,
            content_blocks: None,
        };
        let msg = ConversationMessage::User {
            id: protocol::MessageId::new(),
            content: vec![mk(a), mk(b)],
            is_meta: false,
            is_compact_summary: false,
            is_visible_in_transcript_only: false,
        };
        orch.persist_message_to_jsonl(&msg).await;

        let raw = std::fs::read_to_string(&session_path).expect("session file");
        assert!(!raw.contains("toolUseResult"), "got: {raw}");
        assert!(!raw.contains("sourceToolAssistantUUID"), "got: {raw}");
    }

    /// An ALLOWED tool's line is byte-unchanged — no stray key.
    #[tokio::test]
    async fn allowed_tool_result_line_has_no_denial_kind() {
        let dir = tempfile::tempdir().expect("tempdir");
        let session_path = dir.path().join("session.jsonl");
        let orch = orch_with_writer(dir.path(), session_path.clone());

        let msg = ConversationMessage::User {
            id: protocol::MessageId::new(),
            content: vec![protocol::ContentBlock::ToolResult {
                tool_use_id: protocol::ToolUseId::new(),
                content: "ok".into(),
                is_error: false,
                provider_tool_use_id: None,
                content_blocks: None,
            }],
            is_meta: false,
            is_compact_summary: false,
            is_visible_in_transcript_only: false,
        };
        orch.persist_message_to_jsonl(&msg).await;

        let raw = std::fs::read_to_string(&session_path).expect("session file");
        assert!(!raw.contains("toolDenialKind"));
    }

    #[tokio::test]
    async fn persist_message_to_jsonl_uses_the_supplied_message_uuid() {
        let dir = tempfile::tempdir().expect("tempdir");
        let session_path = dir.path().join("session.jsonl");
        let orch = orch_with_writer(dir.path(), session_path.clone());

        let raw_uuid = uuid::Uuid::new_v4();
        let msg = ConversationMessage::user(
            protocol::MessageId::from_uuid(raw_uuid),
            "sdk replay prompt".into(),
        );
        orch.persist_message_to_jsonl(&msg).await;

        let lines = read_jsonl(&session_path);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].uuid, raw_uuid.to_string());
    }

    #[tokio::test]
    async fn session_contains_message_uuid_accepts_bare_and_prefixed_ids() {
        let dir = tempfile::tempdir().expect("tempdir");
        let session_path = dir.path().join("session.jsonl");
        let orch = orch_with_writer(dir.path(), session_path);

        let id = protocol::MessageId::new();
        {
            let mut session = orch.session.lock().await;
            session
                .history
                .push(ConversationMessage::user(id, "seed".into()));
        }

        assert!(
            orch.session_contains_message_uuid(&id.as_uuid().to_string())
                .await
        );
        assert!(orch.session_contains_message_uuid(&id.to_string()).await);
        assert!(
            !orch
                .session_contains_message_uuid(&uuid::Uuid::new_v4().to_string())
                .await
        );
    }

    #[tokio::test]
    async fn assistant_envelope_carries_full_betamessage_shape() {
        let dir = tempfile::tempdir().expect("tempdir");
        let orch = orch_with_writer(dir.path(), dir.path().join("s.jsonl"));
        let msg = ConversationMessage::Assistant {
            id: protocol::MessageId::new(),
            content: vec![protocol::ContentBlock::Text { text: "hi".into() }],
            stop_reason: Some("end_turn".into()),
        };
        let usage = serde_json::json!({ "input_tokens": 5, "output_tokens": 3 });

        // Real path (model + usage supplied) → full BetaMessage envelope, in the
        // claude-code / golden-fixture key order.
        let jmsg = orch.to_jsonl_message_with_inner_id(
            &msg,
            "sess",
            None,
            None,
            None,
            None,
            Some("inner-abc"),
            Some("claude-opus-4-8"),
            Some(&usage),
            Some("req_test123"),
            None,
        );
        // The real-response path stamps the top-level `requestId` (via `extra`).
        assert_eq!(
            jmsg.extra.get("requestId").and_then(|v| v.as_str()),
            Some("req_test123"),
            "real assistant line carries the top-level requestId"
        );
        let inner = jmsg.message.as_object().expect("inner is an object");
        let keys: Vec<&str> = inner.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            vec![
                "id",
                "type",
                "role",
                "content",
                "model",
                "stop_reason",
                "stop_sequence",
                "usage"
            ],
            "BetaMessage envelope key order"
        );
        assert_eq!(inner["id"], serde_json::json!("inner-abc"));
        assert_eq!(inner["type"], serde_json::json!("message"));
        assert_eq!(inner["role"], serde_json::json!("assistant"));
        assert_eq!(inner["model"], serde_json::json!("claude-opus-4-8"));
        assert_eq!(inner["stop_reason"], serde_json::json!("end_turn"));
        assert_eq!(inner["stop_sequence"], serde_json::Value::Null);
        assert_eq!(inner["usage"], usage);

        // Synthetic path (no model/usage) → the synthetic BetaMessage envelope
        // (baseCreateAssistantMessage `QBl` → createAssistantAPIErrorMessage `tc`,
        // binary @205978440): model "<synthetic>", `usage` OMITTED, `stop_reason`
        // hardcoded "stop_sequence" (NOT the message's own "end_turn"), with
        // container/stop_details/context_management = null.
        let plain = orch.to_jsonl_message_with_inner_id(
            &msg,
            "sess",
            None,
            None,
            None,
            None,
            Some("inner-abc"),
            None,
            None,
            None,
            None,
        );
        // No request_id supplied → no top-level `requestId` (the synthetic case).
        assert!(
            !plain.extra.contains_key("requestId"),
            "synthetic line omits requestId"
        );
        let pinner = plain.message.as_object().unwrap();
        let pkeys: Vec<&str> = pinner.keys().map(String::as_str).collect();
        assert_eq!(
            pkeys,
            vec![
                "id",
                "container",
                "model",
                "role",
                "stop_details",
                "stop_reason",
                "stop_sequence",
                "type",
                "content",
                "context_management"
            ],
            "synthetic BetaMessage envelope key order"
        );
        assert_eq!(pinner["id"], serde_json::json!("inner-abc"));
        assert_eq!(pinner["model"], serde_json::json!("<synthetic>"));
        assert_eq!(pinner["container"], serde_json::Value::Null);
        assert_eq!(pinner["role"], serde_json::json!("assistant"));
        assert_eq!(pinner["stop_details"], serde_json::Value::Null);
        // Hardcoded "stop_sequence", NOT the message's own "end_turn".
        assert_eq!(pinner["stop_reason"], serde_json::json!("stop_sequence"));
        assert_eq!(pinner["stop_sequence"], serde_json::json!(""));
        assert_eq!(pinner["type"], serde_json::json!("message"));
        assert_eq!(pinner["context_management"], serde_json::Value::Null);
        // `usage` is omitted (tc calls QBl without a usage arg).
        assert!(
            !pinner.contains_key("usage"),
            "synthetic envelope must omit usage"
        );
        assert_eq!(pinner["content"][0]["type"], serde_json::json!("text"));
        assert_eq!(pinner["content"][0]["text"], serde_json::json!("hi"));
    }

    #[tokio::test]
    async fn synthetic_api_error_envelope_stamps_top_level_fields() {
        let dir = tempfile::tempdir().expect("tempdir");
        let orch = orch_with_writer(dir.path(), dir.path().join("s.jsonl"));
        let msg = ConversationMessage::Assistant {
            id: protocol::MessageId::new(),
            content: vec![protocol::ContentBlock::Text {
                text: "API Error: boom".into(),
            }],
            stop_reason: Some("model_error".into()),
        };

        // 1. No-category builder (top-level `model_error` catch / malformed
        //    terminal): `isApiErrorMessage:true`, `error`/`apiErrorStatus` OMITTED,
        //    inner `stop_reason` stays `"stop_sequence"`.
        let bare = orch.to_jsonl_message_with_inner_id(
            &msg,
            "sess",
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(&ApiErrorEnvelope::default()),
        );
        assert_eq!(
            bare.extra.get("isApiErrorMessage"),
            Some(&serde_json::Value::Bool(true)),
            "isApiErrorMessage is always stamped"
        );
        assert!(!bare.extra.contains_key("error"), "no error category");
        assert!(
            !bare.extra.contains_key("apiErrorStatus"),
            "no apiErrorStatus"
        );
        assert_eq!(
            bare.message["stop_reason"],
            serde_json::json!("stop_sequence"),
            "no-override keeps the synthetic stop_sequence"
        );

        // 2. `max_output_tokens` category (max_tokens / context-window cap).
        let cap = orch.to_jsonl_message_with_inner_id(
            &msg,
            "sess",
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(&ApiErrorEnvelope {
                error: Some("max_output_tokens"),
                api_error_status: None,
                inner_stop_reason: None,
            }),
        );
        assert_eq!(
            cap.extra.get("error").and_then(|v| v.as_str()),
            Some("max_output_tokens")
        );
        assert_eq!(
            cap.extra.get("isApiErrorMessage"),
            Some(&serde_json::Value::Bool(true))
        );

        // 3. Refusal: `error:"invalid_request"` + inner `stop_reason:"refusal"`.
        let refusal = orch.to_jsonl_message_with_inner_id(
            &msg,
            "sess",
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(&ApiErrorEnvelope {
                error: Some("invalid_request"),
                api_error_status: None,
                inner_stop_reason: Some("refusal"),
            }),
        );
        assert_eq!(
            refusal.extra.get("error").and_then(|v| v.as_str()),
            Some("invalid_request")
        );
        assert_eq!(
            refusal.message["stop_reason"],
            serde_json::json!("refusal"),
            "refusal overrides the inner stop_reason"
        );

        // 4. With an HTTP status → `apiErrorStatus` is a JSON number.
        let with_status = orch.to_jsonl_message_with_inner_id(
            &msg,
            "sess",
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(&ApiErrorEnvelope {
                error: Some("rate_limit"),
                api_error_status: Some(429),
                inner_stop_reason: None,
            }),
        );
        assert_eq!(
            with_status.extra.get("apiErrorStatus"),
            Some(&serde_json::Value::Number(429.into()))
        );

        // 5. A normal (non-api-error) assistant line stamps NOTHING.
        let normal = orch.to_jsonl_message(&msg, "sess", None, None, None, None);
        assert!(!normal.extra.contains_key("isApiErrorMessage"));
        assert!(!normal.extra.contains_key("error"));
    }

    // ── per-request api-error classifier (`Flp`/`KNn`) ────────────────────────

    /// `classify_api_error` maps each typed [`LlmError`] semantic variant to the
    /// canonical (category, status) pair recovered from the 2.1.195 `Flp`/`KNn`
    /// classifier + the on-disk transcript aggregate. Status is OMITTED (`None`)
    /// where the port has no confident canonical HTTP status (mirrors claude
    /// omitting `apiErrorStatus` when the error is not an `APIError`-with-status).
    #[test]
    fn classify_api_error_prefers_the_true_status_over_the_canonical_table() {
        use llm_client::LlmError;
        // A REAL 422 used to be persisted as 400: `InvalidRequest` mapped to its
        // canonical status because the raw one was gone by then. The provider
        // decoders now store the SDK's `${status} ${body}` text, so the true
        // status survives into the transcript's `apiErrorStatus`.
        for (status, expected) in [(422u16, 422u16), (424, 424), (409, 409)] {
            let e = OrchestratorError::ApiCall(LlmError::InvalidRequest {
                message: format!("{status} {{\"type\":\"error\"}}"),
            });
            let env = classify_api_error(&e);
            assert_eq!(env.api_error_status, Some(expected), "status {status}");
            // The CATEGORY still comes from the variant — only the status is
            // sharpened, so the byte-locked file-format value is untouched.
            assert_eq!(env.error, Some("invalid_request"));
        }

        // No prefix (internal validation, not a provider decode) → the canonical
        // table still applies, exactly as before.
        let internal = OrchestratorError::ApiCall(LlmError::InvalidRequest {
            message: "invalid model name".to_string(),
        });
        assert_eq!(classify_api_error(&internal).api_error_status, Some(400));

        // A variant carrying no message can never gain a prefix, so its
        // canonical status is unaffected.
        let auth = OrchestratorError::ApiCall(LlmError::Authentication {
            message: String::new(),
        });
        assert_eq!(classify_api_error(&auth).api_error_status, Some(401));
    }

    #[test]
    fn classify_api_error_maps_llm_variants_to_category_and_status() {
        use llm_client::LlmError;
        let cases: Vec<(LlmError, Option<&'static str>, Option<u16>)> = vec![
            (
                LlmError::RateLimited {
                    retry_after: None,
                    scope: None,
                },
                Some("rate_limit"),
                Some(429),
            ),
            // On-disk 529 lines tag `server_error` (NOT the `YNn` statusline
            // `"overloaded"`).
            (
                LlmError::Overloaded { repeated: false },
                Some("server_error"),
                Some(529),
            ),
            (
                LlmError::Authentication {
                    message: String::new(),
                },
                Some("authentication_failed"),
                Some(401),
            ),
            (
                LlmError::PermissionDenied {
                    message: String::new(),
                },
                Some("authentication_failed"),
                Some(403),
            ),
            // Billing is an Error-message match in `Flp`, not a status branch.
            (LlmError::QuotaExceeded, Some("billing_error"), None),
            // PTL/context-window: `invalid_request` with NO status.
            (
                LlmError::ContextOverflow { token_gap: 12 },
                Some("invalid_request"),
                None,
            ),
            // 2.1.212 413 request-too-large: SAME `invalid_request` category as
            // the context-window branch, no `apiErrorStatus` on the `su` call.
            (LlmError::RequestTooLarge, Some("invalid_request"), None),
            (
                LlmError::InvalidRequest {
                    message: "bad".into(),
                },
                Some("invalid_request"),
                Some(400),
            ),
            (
                LlmError::ModelUnavailable,
                Some("model_not_found"),
                Some(404),
            ),
            (LlmError::ProviderInternal, Some("server_error"), Some(500)),
            // Timeout/transport tail → `server_error`, no status.
            (
                LlmError::Transport {
                    message: "t".into(),
                },
                Some("server_error"),
                None,
            ),
            (
                LlmError::StreamInterrupted {
                    message: "s".into(),
                },
                Some("server_error"),
                None,
            ),
            // Generic `Error` fallthrough → `unknown`.
            (
                LlmError::CostUnavailable {
                    message: "c".into(),
                },
                Some("unknown"),
                None,
            ),
            (
                LlmError::UnsupportedCapability {
                    capability: "x".into(),
                },
                Some("unknown"),
                None,
            ),
        ];
        for (inner, cat, status) in cases {
            // Both wrapping variants classify identically.
            for wrapped in [
                OrchestratorError::ApiCall(inner.clone()),
                OrchestratorError::Streaming(inner.clone()),
            ] {
                let env = classify_api_error(&wrapped);
                assert_eq!(env.error, cat, "category for {inner:?}");
                assert_eq!(env.api_error_status, status, "status for {inner:?}");
                // The `ql` path never overrides the inner stop_reason.
                assert_eq!(
                    env.inner_stop_reason, None,
                    "inner stop_reason for {inner:?}"
                );
            }
        }
    }

    /// The 2.1.212 `$Vi()` request-too-large notice is byte-exact and its tail
    /// switches on interactivity (`un()===!Ht.isInteractive`).
    #[test]
    fn request_too_large_notice_is_byte_exact() {
        // Non-interactive (print) session: generic advice.
        assert_eq!(
            super::request_too_large_notice(false),
            "Request too large (max 32MB). Accumulated images and attachments in the conversation pushed the request over the limit. Remove older images or compact the conversation."
        );

        // Interactive (TUI) session: `/compact` + double-esc actions.
        assert_eq!(
            super::request_too_large_notice(true),
            "Request too large (max 32MB). Accumulated images and attachments in the conversation pushed the request over the limit. Run /compact, or double press esc to go back and remove attachments."
        );
    }

    /// Orchestrator-internal / generic-Error variants fall through to `unknown`
    /// with NO status (the `Flp` generic-`Error` tail).
    #[test]
    fn classify_api_error_generic_variants_are_unknown_no_status() {
        for e in [
            OrchestratorError::Internal("boom".into()),
            OrchestratorError::StreamingProtocol("bad".into()),
            OrchestratorError::StreamEndedWithoutStop,
            OrchestratorError::RepeatedOverloaded,
            OrchestratorError::PermissionAbort {
                message: "Agent aborted: too many classifier denials in headless mode".into(),
            },
            OrchestratorError::MaxTurnsReached { max_turns: 30 },
            OrchestratorError::MaxBudgetReached {
                budget_nano_usd: 5_000_000_000,
            },
        ] {
            let env = classify_api_error(&e);
            assert_eq!(env.error, Some("unknown"), "{e:?}");
            assert_eq!(env.api_error_status, None, "{e:?}");
            assert_eq!(env.inner_stop_reason, None, "{e:?}");
        }
    }

    /// End-to-end: a classified envelope drives the persisted JSONL line's
    /// top-level `error`/`isApiErrorMessage`/`apiErrorStatus` fields with
    /// presence + values 1:1 with the classifier output.
    #[test]
    fn classified_envelope_stamps_jsonl_top_level_fields() {
        use llm_client::LlmError;
        let dir = tempfile::tempdir().expect("tempdir");
        let orch = orch_with_writer(dir.path(), dir.path().join("s.jsonl"));
        let msg = ConversationMessage::Assistant {
            id: protocol::MessageId::new(),
            content: vec![protocol::ContentBlock::Text {
                text: "invalid request: bad".into(),
            }],
            stop_reason: Some("model_error".into()),
        };

        let env = classify_api_error(&OrchestratorError::ApiCall(LlmError::InvalidRequest {
            message: "bad".into(),
        }));
        let line = orch.to_jsonl_message_with_inner_id(
            &msg,
            "sess",
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(&env),
        );
        assert_eq!(
            line.extra.get("error").and_then(|v| v.as_str()),
            Some("invalid_request")
        );
        assert_eq!(
            line.extra.get("isApiErrorMessage"),
            Some(&serde_json::Value::Bool(true))
        );
        assert_eq!(
            line.extra.get("apiErrorStatus"),
            Some(&serde_json::Value::Number(400.into()))
        );

        // A no-status category (server_error from transport) OMITS apiErrorStatus.
        let env2 = classify_api_error(&OrchestratorError::ApiCall(LlmError::Transport {
            message: "t".into(),
        }));
        let line2 = orch.to_jsonl_message_with_inner_id(
            &msg,
            "sess",
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(&env2),
        );
        assert_eq!(
            line2.extra.get("error").and_then(|v| v.as_str()),
            Some("server_error")
        );
        assert!(
            !line2.extra.contains_key("apiErrorStatus"),
            "no-status category must omit apiErrorStatus"
        );
    }

    // ── transcript per-line cwd reflects the LIVE (post-`cd`) session cwd ──────

    #[tokio::test]
    async fn transcript_line_cwd_tracks_live_cwd_after_cd() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Share a `current_cwd` cell with the orchestrator — the same `Arc` the
        // desktop composition root hands to `OrchestratorCwdChangedFirer`, which
        // a Bash `cd` mutates. Start it at the init cwd.
        let init_cwd = dir.path().to_path_buf();
        let cell = Arc::new(std::sync::Mutex::new(init_cwd.clone()));
        let orch =
            orch_with_writer(dir.path(), dir.path().join("s.jsonl")).with_current_cwd(cell.clone());

        // First persisted line is stamped with the init cwd.
        let m1 = ConversationMessage::user(protocol::MessageId::new(), "before cd".into());
        let line1 = orch.to_jsonl_message(&m1, "sess", None, None, None, None);
        assert_eq!(
            line1.cwd,
            init_cwd.to_string_lossy(),
            "pre-`cd` line carries the init cwd"
        );

        // Simulate a Bash `cd` advancing the shared cell (what the CwdChanged
        // firer does on every `cd`).
        let new_cwd = dir.path().join("subdir");
        *cell.lock().unwrap() = new_cwd.clone();

        // The NEXT persisted line must reflect the advanced cwd, not the init.
        let m2 = ConversationMessage::user(protocol::MessageId::new(), "after cd".into());
        let line2 = orch.to_jsonl_message(&m2, "sess", None, None, None, None);
        assert_eq!(
            line2.cwd,
            new_cwd.to_string_lossy(),
            "post-`cd` line must carry the advanced live cwd, not the init cwd"
        );
        assert_ne!(
            line2.cwd, line1.cwd,
            "the cwd readback must move with the live session cwd"
        );
    }

    // ── test 5: per-content_block_stop single-block assistant lines ───────────
    //
    // claude.ts:2171-2211: a streaming assistant turn emits ONE JSONL line per
    // content block — same inner `message.id`, distinct top-level `uuid`, one
    // block each. sessionStorage.ts:1028: each tool_result parents to ITS
    // tool_use's line uuid (`sourceToolAssistantUUID`), NOT a shared per-turn
    // parent.
    //
    // This drives `persist_assistant_per_block` directly: an assistant turn with
    // content [text, tool_use A, tool_use B] must persist THREE single-block
    // assistant lines that (a) share one inner `message.id`, (b) have three
    // DISTINCT top-level uuids, (c) carry exactly one block each; then a
    // tool_result for A parents to A's line uuid and a tool_result for B parents
    // to B's line uuid.
    #[tokio::test]
    async fn assistant_turn_persists_one_line_per_content_block_with_per_tool_reparenting() {
        let dir = tempfile::tempdir().expect("tempdir");
        let session_path = dir.path().join("session.jsonl");
        let orch = orch_with_writer(dir.path(), session_path.clone());
        let session = orch.session();
        session.lock().await.model_profile = Some("deepseek".to_string());

        let id_a = protocol::ToolUseId::from("toolu_A");
        let id_b = protocol::ToolUseId::from("toolu_B");

        let assistant_id = protocol::MessageId::new();
        let assistant_msg = ConversationMessage::Assistant {
            id: assistant_id,
            content: vec![
                protocol::ContentBlock::Text {
                    text: "let me call two tools".into(),
                },
                protocol::ContentBlock::ToolUse {
                    id: id_a.clone(),
                    name: "Alpha".into(),
                    input: serde_json::json!({}),
                    provider_id: None,
                },
                protocol::ContentBlock::ToolUse {
                    id: id_b.clone(),
                    name: "Bravo".into(),
                    input: serde_json::json!({}),
                    provider_id: None,
                },
            ],
            stop_reason: Some("tool_use".into()),
        };

        let map = orch
            .persist_assistant_per_block(&assistant_msg, None, None)
            .await;

        let lines = read_jsonl(&session_path);
        // (c) THREE single-block assistant lines.
        let asst_lines: Vec<&JsonlMessage> = lines
            .iter()
            .filter(|l| l.message_type == "assistant")
            .collect();
        assert_eq!(
            asst_lines.len(),
            3,
            "expected 3 single-block assistant lines (one per content block), got {}",
            asst_lines.len()
        );
        for (i, l) in asst_lines.iter().enumerate() {
            let blocks = l
                .message
                .get("content")
                .and_then(|c| c.as_array())
                .unwrap_or_else(|| panic!("line {i} content must be an array"));
            assert_eq!(blocks.len(), 1, "line {i} must carry exactly one block");
            assert_eq!(
                l.extra.get("modelProfile").and_then(|value| value.as_str()),
                Some("deepseek"),
                "line {i} must persist the provider profile used for the response"
            );
        }

        // (a) all three share ONE inner `message.id`.
        let inner_ids: Vec<&str> = asst_lines
            .iter()
            .map(|l| {
                l.message
                    .get("id")
                    .and_then(|v| v.as_str())
                    .expect("inner message.id present")
            })
            .collect();
        assert_eq!(
            inner_ids[0], inner_ids[1],
            "all blocks must share the same inner message.id"
        );
        assert_eq!(inner_ids[1], inner_ids[2]);
        assert_eq!(
            inner_ids[0],
            assistant_id.as_uuid().to_string(),
            "shared inner message.id must be the turn's logical id"
        );

        // (b) three DISTINCT top-level uuids.
        let uuids: std::collections::HashSet<&str> =
            asst_lines.iter().map(|l| l.uuid.as_str()).collect();
        assert_eq!(
            uuids.len(),
            3,
            "the three lines must have distinct top-level uuids"
        );

        // map must hold A and B -> their respective line uuids (text block none).
        let a_uuid = map.get(&id_a).expect("A in map").clone();
        let b_uuid = map.get(&id_b).expect("B in map").clone();
        assert_ne!(a_uuid, b_uuid, "A and B must map to different line uuids");

        // Persist a tool_result for A and for B; each must parent to ITS line.
        let tr_a = ConversationMessage::User {
            id: protocol::MessageId::new(),
            content: vec![protocol::ContentBlock::ToolResult {
                tool_use_id: id_a.clone(),
                content: "result-A".into(),
                is_error: false,
                provider_tool_use_id: None,
                content_blocks: None,
            }],
            is_meta: false,
            is_compact_summary: false,
            is_visible_in_transcript_only: false,
        };
        orch.persist_message_to_jsonl_with_parent(&tr_a, Some(a_uuid.clone()))
            .await;
        let tr_b = ConversationMessage::User {
            id: protocol::MessageId::new(),
            content: vec![protocol::ContentBlock::ToolResult {
                tool_use_id: id_b.clone(),
                content: "result-B".into(),
                is_error: false,
                provider_tool_use_id: None,
                content_blocks: None,
            }],
            is_meta: false,
            is_compact_summary: false,
            is_visible_in_transcript_only: false,
        };
        orch.persist_message_to_jsonl_with_parent(&tr_b, Some(b_uuid.clone()))
            .await;

        let lines = read_jsonl(&session_path);
        let tr_lines: Vec<&JsonlMessage> = lines
            .iter()
            .filter(|l| {
                l.message_type == "user"
                    && serde_json::to_string(&l.message)
                        .map(|s| s.contains("tool_result"))
                        .unwrap_or(false)
            })
            .collect();
        assert_eq!(tr_lines.len(), 2, "expected 2 tool_result user lines");
        // tr_a parents to A's line; tr_b parents to B's line — NOT one shared parent.
        assert_eq!(
            tr_lines[0].parent_uuid.as_deref(),
            Some(a_uuid.as_str()),
            "tool_result A must parent to A's tool_use line uuid"
        );
        assert_eq!(
            tr_lines[1].parent_uuid.as_deref(),
            Some(b_uuid.as_str()),
            "tool_result B must parent to B's tool_use line uuid"
        );
        assert_ne!(
            tr_lines[0].parent_uuid, tr_lines[1].parent_uuid,
            "the two tool_results must NOT share one parent (per-tool reparenting)"
        );
    }

    #[tokio::test]
    async fn merged_assistant_persists_model_profile_for_resume() {
        let dir = tempfile::tempdir().expect("tempdir");
        let session_path = dir.path().join("session.jsonl");
        let orch = orch_with_writer(dir.path(), session_path.clone());
        let session = orch.session();
        session.lock().await.model_profile = Some("openrouter".to_string());
        let assistant = ConversationMessage::Assistant {
            id: protocol::MessageId::new(),
            content: vec![protocol::ContentBlock::Text {
                text: "done".to_string(),
            }],
            stop_reason: Some("end_turn".to_string()),
        };

        orch.persist_assistant_merged(&assistant, None, None).await;

        let lines = read_jsonl(&session_path);
        assert_eq!(lines.len(), 1);
        assert_eq!(
            lines[0]
                .extra
                .get("modelProfile")
                .and_then(|value| value.as_str()),
            Some("openrouter")
        );
    }
}

#[cfg(test)]
mod prefix_overflow_block_count_tests {
    use super::count_document_and_image_blocks;
    use protocol::{ContentBlock, ConversationMessage, DocumentSource, ImageSource, MessageId};

    fn image_block() -> ContentBlock {
        ContentBlock::Image {
            source: ImageSource::Base64 {
                media_type: "image/png".into(),
                data: "AAAA".into(),
            },
        }
    }

    fn document_block() -> ContentBlock {
        ContentBlock::Document {
            source: DocumentSource::Base64 {
                media_type: "application/pdf".into(),
                data: "AAAA".into(),
            },
        }
    }

    #[test]
    fn counts_documents_and_images_across_user_and_assistant() {
        // #55 a3p documentBlockCount / imageBlockCount.
        let msgs = vec![
            ConversationMessage::User {
                id: MessageId::new(),
                content: vec![
                    ContentBlock::Text { text: "hi".into() },
                    image_block(),
                    document_block(),
                ],
                is_meta: false,
                is_compact_summary: false,
                is_visible_in_transcript_only: false,
            },
            ConversationMessage::Assistant {
                id: MessageId::new(),
                content: vec![image_block()],
                stop_reason: None,
            },
            // System messages carry a flat string — never counted.
            ConversationMessage::System {
                id: MessageId::new(),
                content: "system".into(),
                subtype: None,
                compact_metadata: None,
            },
        ];
        let (docs, imgs) = count_document_and_image_blocks(&msgs);
        assert_eq!(docs, 1, "one document block across the messages");
        assert_eq!(imgs, 2, "two image blocks across the messages");
    }

    #[test]
    fn counts_zero_when_no_media_blocks() {
        let msgs = vec![ConversationMessage::user(
            MessageId::new(),
            "plain text".into(),
        )];
        assert_eq!(count_document_and_image_blocks(&msgs), (0, 0));
    }
}

// #78: unit coverage for the streaming-path "visible output" predicate. The
// streaming driver's thinking-only nudge (`conversation.rs` `Some("end_turn")`
// / `Some("stop_sequence")` / `None` arms) gates on this exact function; the
// batched twin's branch transitions are covered in
// `turn_loop::malformed_and_thinking_only_tests`.
#[cfg(test)]
mod pumped_visible_text_tests {
    use super::pumped_has_visible_text;
    use protocol::ContentBlock;

    fn text(s: &str) -> ContentBlock {
        ContentBlock::Text {
            text: s.to_string(),
        }
    }
    fn thinking(s: &str) -> ContentBlock {
        ContentBlock::Thinking {
            thinking: s.to_string(),
            signature: None,
        }
    }

    #[test]
    fn empty_blocks_have_no_visible_text() {
        assert!(!pumped_has_visible_text(&[]));
    }

    #[test]
    fn thinking_only_has_no_visible_text() {
        assert!(!pumped_has_visible_text(&[thinking("reasoning")]));
    }

    #[test]
    fn whitespace_only_text_is_not_visible() {
        assert!(!pumped_has_visible_text(&[text("   \n\t ")]));
    }

    #[test]
    fn non_empty_text_is_visible() {
        assert!(pumped_has_visible_text(&[text("hello")]));
    }

    #[test]
    fn thinking_plus_real_text_is_visible() {
        assert!(pumped_has_visible_text(&[
            thinking("reasoning"),
            text("answer")
        ]));
    }
}

// ============================================================================
// Finding #73: per-turn `todo_reminder` (V1) / `task_reminder` (V2).
//
// Proves [`ConversationOrchestrator::todo_reminder_message`] +
// [`ConversationOrchestrator::bump_reminder_turn_counters`] +
// [`ConversationOrchestrator::note_todo_reminder_tool_call`]:
// - counters increment once per turn and reset on the relevant tool call;
// - the reminder fires only when BOTH counters reach the thresholds, the
//   relevant tool is present, the Brief tool is absent, history is non-empty,
//   and the killswitch is not "off";
// - the body is byte-exact (V1 with/without items; V2 with items) and emitted
//   inside a `<system-reminder>` envelope as a META user message (oracle
//   `Zy([kn({content:o,isMeta:!0})])`, 2.1.238 @296690005).
// The byte-level renderer is additionally covered in `tool_task::reminder::tests`.
// ============================================================================
#[cfg(test)]
mod todo_reminder_tests {
    use super::*;
    use crate::prompt::todo_reminder::{TaskReminderItem, TodoReminderTaskProvider};
    use crate::test_support::{
        noop_hook_executor, MockApiClient, MockOutputStream, NoOpPermissionGate,
        StaticMemoryProvider,
    };
    use crate::OrchestratorConfig;
    use engine::{TodoItem, TodoState};
    use std::sync::Arc;
    use tool_api::context::ToolUseContext;
    use tool_api::progress::ToolProgressSender;
    use tool_api::registry::ToolRegistry;
    use tool_api::tool_trait::{
        DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolStaticContext,
        ValidationError,
    };

    /// `std::env::set_var`/`remove_var` are not thread-safe; serialize the
    /// env-mutating tests (selection + killswitch) behind this lock.
    static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// Minimal name-only tool for the tool-presence gates.
    struct NamedTool(&'static str);
    #[async_trait]
    impl Tool for NamedTool {
        fn name(&self) -> &str {
            self.0
        }
        fn input_schema(&self) -> &serde_json::Value {
            static SCHEMA: once_cell::sync::Lazy<serde_json::Value> = once_cell::sync::Lazy::new(
                || serde_json::json!({ "type": "object", "properties": {} }),
            );
            &SCHEMA
        }
        fn is_enabled(&self, _ctx: &ToolStaticContext) -> bool {
            true
        }
        fn max_result_size_chars(&self) -> usize {
            1024
        }
        fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
            true
        }
        fn is_read_only(&self, _input: &serde_json::Value) -> bool {
            true
        }
        async fn validate_input(
            &self,
            _input: &serde_json::Value,
            _ctx: &ToolUseContext,
        ) -> Result<(), ValidationError> {
            Ok(())
        }
        async fn check_permissions(
            &self,
            _input: &serde_json::Value,
            _ctx: &ToolUseContext,
        ) -> permission::PermissionResult {
            permission::PermissionResult::Allow {
                reason: permission::PermissionDecisionReason::Other { reason: "t".into() },
                updated_input: None,
                update_destination: None,
                metadata: permission::result::PermissionMetadata::default(),
            }
        }
        async fn description(
            &self,
            _input: &serde_json::Value,
            _opts: &DescriptionOptions,
        ) -> String {
            String::new()
        }
        async fn prompt(&self, _opts: &PromptOptions) -> String {
            String::new()
        }
        async fn call(
            &self,
            _input: serde_json::Value,
            _ctx: ToolUseContext,
            _tx: ToolProgressSender,
        ) -> Result<ToolCallResult, ToolError> {
            Ok(ToolCallResult {
                data: serde_json::json!({}),
                model_content: None,
                new_messages: vec![],
                context_modifier: None,
                is_error: false,
                mcp_meta: None,
            })
        }
    }

    /// Static V2 task source.
    struct StaticTasks(Vec<TaskReminderItem>);
    #[async_trait]
    impl TodoReminderTaskProvider for StaticTasks {
        async fn task_items(&self, _session_id: protocol::SessionId) -> Vec<TaskReminderItem> {
            self.0.clone()
        }
    }

    fn orch_with(tools: ToolRegistry) -> ConversationOrchestrator {
        let api = Arc::new(MockApiClient::new(vec![]));
        ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            api,
            Arc::new(tools),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        )
    }

    fn reg_with(names: &[&'static str]) -> ToolRegistry {
        let mut reg = ToolRegistry::new();
        for n in names {
            reg.register_builtin(Arc::new(NamedTool(n)));
        }
        reg
    }

    /// Make the session non-empty (the binary `!e||e.length===0 ⇒ []` gate) and
    /// optionally arm both counters at their thresholds.
    async fn prime_session(orch: &ConversationOrchestrator, write_c: u32, reminder_c: u32) {
        let mut s = orch.session.lock().await;
        s.history.push(ConversationMessage::user(
            MessageId::new(),
            "hi".to_string(),
        ));
        s.turns_since_last_todo_write = write_c;
        s.turns_since_last_reminder = reminder_c;
    }

    // ── counters ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn bump_increments_both_counters_each_turn() {
        let orch = orch_with(reg_with(&["TodoWrite"]));
        orch.bump_reminder_turn_counters().await;
        orch.bump_reminder_turn_counters().await;
        let s = orch.session.lock().await;
        assert_eq!(s.turns_since_last_todo_write, 2);
        assert_eq!(s.turns_since_last_reminder, 2);
    }

    #[tokio::test]
    async fn note_tool_call_resets_write_counter_on_todowrite_v1() {
        let _g = ENV_LOCK.lock().await;
        std::env::set_var("LINGXI_ENABLE_TASKS", "off"); // ⇒ V1 selected
        let orch = orch_with(reg_with(&["TodoWrite"]));
        {
            let mut s = orch.session.lock().await;
            s.turns_since_last_todo_write = 7;
            s.turns_since_last_reminder = 7;
        }
        orch.note_todo_reminder_tool_call(&["TodoWrite".to_string()])
            .await;
        {
            let s = orch.session.lock().await;
            assert_eq!(
                s.turns_since_last_todo_write, 0,
                "TodoWrite resets write ctr"
            );
            assert_eq!(s.turns_since_last_reminder, 7, "reminder ctr untouched");
        }
        // A non-qualifying tool (Read) does NOT reset.
        {
            let mut s = orch.session.lock().await;
            s.turns_since_last_todo_write = 5;
        }
        orch.note_todo_reminder_tool_call(&["Read".to_string()])
            .await;
        assert_eq!(orch.session.lock().await.turns_since_last_todo_write, 5);
        std::env::remove_var("LINGXI_ENABLE_TASKS");
    }

    #[tokio::test]
    async fn note_tool_call_resets_on_taskupdate_v2_default() {
        let _g = ENV_LOCK.lock().await;
        std::env::remove_var("LINGXI_ENABLE_TASKS"); // default ⇒ V2
        let orch = orch_with(reg_with(&["TaskUpdate"]));
        {
            let mut s = orch.session.lock().await;
            s.turns_since_last_todo_write = 9;
        }
        // V2 resets on TaskCreate or TaskUpdate, NOT on TodoWrite.
        orch.note_todo_reminder_tool_call(&["TodoWrite".to_string()])
            .await;
        assert_eq!(
            orch.session.lock().await.turns_since_last_todo_write,
            9,
            "V2 mode does not reset on TodoWrite"
        );
        orch.note_todo_reminder_tool_call(&["TaskUpdate".to_string()])
            .await;
        assert_eq!(orch.session.lock().await.turns_since_last_todo_write, 0);
    }

    // ── firing gates ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn no_fire_below_threshold() {
        let _g = ENV_LOCK.lock().await;
        std::env::remove_var("LINGXI_TODO_REMINDER_MODE");
        std::env::set_var("LINGXI_ENABLE_TASKS", "off"); // V1
        let orch = orch_with(reg_with(&["TodoWrite"]));
        prime_session(&orch, 9, 10).await; // write ctr one short
        assert!(orch.todo_reminder_message().await.is_none());
        // Now both at threshold ⇒ fires.
        {
            let mut s = orch.session.lock().await;
            s.turns_since_last_todo_write = 10;
        }
        assert!(orch.todo_reminder_message().await.is_some());
        std::env::remove_var("LINGXI_ENABLE_TASKS");
    }

    #[tokio::test]
    async fn no_fire_when_history_empty() {
        let _g = ENV_LOCK.lock().await;
        std::env::set_var("LINGXI_ENABLE_TASKS", "off");
        let orch = orch_with(reg_with(&["TodoWrite"]));
        // counters armed but NO history.
        {
            let mut s = orch.session.lock().await;
            s.turns_since_last_todo_write = 10;
            s.turns_since_last_reminder = 10;
        }
        assert!(
            orch.todo_reminder_message().await.is_none(),
            "empty history suppresses the reminder"
        );
        std::env::remove_var("LINGXI_ENABLE_TASKS");
    }

    #[tokio::test]
    async fn no_fire_when_tool_absent() {
        let _g = ENV_LOCK.lock().await;
        std::env::set_var("LINGXI_ENABLE_TASKS", "off"); // V1 needs TodoWrite
        let orch = orch_with(reg_with(&["Read"])); // no TodoWrite
        prime_session(&orch, 10, 10).await;
        assert!(orch.todo_reminder_message().await.is_none());
        std::env::remove_var("LINGXI_ENABLE_TASKS");
    }

    #[tokio::test]
    async fn no_fire_when_brief_present() {
        let _g = ENV_LOCK.lock().await;
        std::env::set_var("LINGXI_ENABLE_TASKS", "off");
        // TodoWrite present AND Brief (SendUserMessage) present ⇒ skip.
        let orch = orch_with(reg_with(&["TodoWrite", "SendUserMessage"]));
        prime_session(&orch, 10, 10).await;
        assert!(orch.todo_reminder_message().await.is_none());
        std::env::remove_var("LINGXI_ENABLE_TASKS");
    }

    #[tokio::test]
    async fn killswitch_off_suppresses() {
        let _g = ENV_LOCK.lock().await;
        std::env::set_var("LINGXI_ENABLE_TASKS", "off");
        std::env::set_var("LINGXI_TODO_REMINDER_MODE", "off");
        let orch = orch_with(reg_with(&["TodoWrite"]));
        prime_session(&orch, 10, 10).await;
        assert!(
            orch.todo_reminder_message().await.is_none(),
            "killswitch \"off\" suppresses the reminder"
        );
        std::env::remove_var("LINGXI_TODO_REMINDER_MODE");
        std::env::remove_var("LINGXI_ENABLE_TASKS");
    }

    // ── exact text + reminder-counter reset on fire ──────────────────────────

    #[tokio::test]
    async fn v1_fires_with_exact_text_no_items_and_resets_reminder_ctr() {
        let _g = ENV_LOCK.lock().await;
        std::env::remove_var("LINGXI_TODO_REMINDER_MODE");
        std::env::set_var("LINGXI_ENABLE_TASKS", "off"); // V1
        let orch = orch_with(reg_with(&["TodoWrite"]));
        prime_session(&orch, 10, 10).await;
        let msg = orch.todo_reminder_message().await.expect("fires");
        // `Zy`/`NT` envelope + `isMeta:!0` (2.1.238 @296690005). NOTE the body's
        // own trailing `\n` sits directly before the wrapper's, exactly as the
        // oracle's `` `<system-reminder>\n${o}\n</system-reminder>` `` produces.
        assert!(msg.is_meta(), "todo_reminder must be isMeta");
        assert_eq!(
            msg.text_content(),
            "<system-reminder>\nThe TodoWrite tool hasn't been used recently. If you're working on tasks that would benefit from tracking progress, consider using the TodoWrite tool to track progress. Also consider cleaning up the todo list if has become stale and no longer matches what you are working on. Only use it if it's relevant to the current work. This is just a gentle reminder - ignore if not applicable.\n\n</system-reminder>"
        );
        // The reminder counter reset to 0 on fire.
        assert_eq!(orch.session.lock().await.turns_since_last_reminder, 0);
        std::env::remove_var("LINGXI_ENABLE_TASKS");
    }

    #[tokio::test]
    async fn v1_fires_with_items_byte_exact() {
        let _g = ENV_LOCK.lock().await;
        std::env::remove_var("LINGXI_TODO_REMINDER_MODE");
        std::env::set_var("LINGXI_ENABLE_TASKS", "off"); // V1
        let orch = orch_with(reg_with(&["TodoWrite"]));
        prime_session(&orch, 10, 10).await;
        {
            let mut s = orch.session.lock().await;
            s.todos.push(TodoItem {
                id: "t1".into(),
                content: "first".into(),
                status: TodoState::Pending,
                active_form: "Doing first".into(),
            });
            s.todos.push(TodoItem {
                id: "t2".into(),
                content: "second".into(),
                status: TodoState::InProgress,
                active_form: "Doing second".into(),
            });
        }
        let msg = orch.todo_reminder_message().await.expect("fires");
        assert!(msg.text_content().ends_with(
            "\n\nHere are the existing contents of your todo list:\n\n[1. [pending] first\n2. [in_progress] second]\n</system-reminder>"
        ), "got: {:?}", msg.text_content());
        std::env::remove_var("LINGXI_ENABLE_TASKS");
    }

    #[tokio::test]
    async fn v2_fires_with_items_from_provider_byte_exact() {
        let _g = ENV_LOCK.lock().await;
        std::env::remove_var("LINGXI_TODO_REMINDER_MODE");
        std::env::remove_var("LINGXI_ENABLE_TASKS"); // default ⇒ V2
        let orch = orch_with(reg_with(&["TaskUpdate"])).with_todo_reminder_tasks(Arc::new(
            StaticTasks(vec![
                TaskReminderItem {
                    id: "1".into(),
                    status: TodoState::Completed,
                    subject: "alpha".into(),
                },
                TaskReminderItem {
                    id: "2".into(),
                    status: TodoState::Pending,
                    subject: "beta".into(),
                },
            ]),
        ));
        prime_session(&orch, 10, 10).await;
        let msg = orch.todo_reminder_message().await.expect("fires");
        let text = msg.text_content();
        assert!(msg.is_meta(), "task_reminder must be isMeta");
        assert!(
            text.starts_with("<system-reminder>\nThe task tools haven't been used recently."),
            "got: {text}"
        );
        assert!(
            text.ends_with(
                "\n\nHere are the existing tasks:\n\n#1. [completed] alpha\n#2. [pending] beta\n</system-reminder>"
            ),
            "got: {text:?}"
        );
        std::env::remove_var("LINGXI_ENABLE_TASKS");
    }

    #[tokio::test]
    async fn v2_fires_base_only_without_provider() {
        let _g = ENV_LOCK.lock().await;
        std::env::remove_var("LINGXI_TODO_REMINDER_MODE");
        std::env::remove_var("LINGXI_ENABLE_TASKS"); // V2
        let orch = orch_with(reg_with(&["TaskUpdate"])); // no task provider
        prime_session(&orch, 10, 10).await;
        let msg = orch.todo_reminder_message().await.expect("fires");
        assert_eq!(
            msg.text_content(),
            "<system-reminder>\nThe task tools haven't been used recently. If you're working on tasks that would benefit from tracking progress, consider using TaskCreate to add new tasks and TaskUpdate to update task status (set to in_progress when starting, completed when done). Also consider cleaning up the task list if it has become stale. Only use these if relevant to the current work. This is just a gentle reminder - ignore if not applicable.\n\n</system-reminder>"
        );
        std::env::remove_var("LINGXI_ENABLE_TASKS");
    }
}

// ── P2-12: post-compact FILE restoration re-reads from disk ───────────────────
//
// `restore_post_compact_attachments` must RE-READ each selected file from disk
// (the binary's `eRg`/`XQn` behaviour) rather than reusing the stale
// `readFileState` snapshot content: a file that changed after its last read is
// restored with FRESH content, a deleted/unreadable file is dropped, and each
// re-read attempt fires the `tengu_post_compact_file_restore_{success,error}`
// telemetry (empty payload).
#[cfg(test)]
mod post_compact_file_restore_tests {
    use super::*;
    use crate::test_support::{
        noop_hook_executor, MockApiClient, MockOutputStream, NoOpPermissionGate,
        StaticMemoryProvider,
    };
    use crate::OrchestratorConfig;
    use std::sync::Arc;
    use tool_api::read_file_state::{set, ReadFileEntry};
    use tool_api::registry::ToolRegistry;

    fn stale_entry(content: &str) -> ReadFileEntry {
        ReadFileEntry {
            content: content.to_string(),
            mtime_ms: 1,
            offset: None,
            limit: None,
            from_read: true,
            seeded_from_context: false,
            is_partial_view: false,
        }
    }

    /// Serialize + clean the process-global invoked-skill registry so these
    /// tests (which now exercise the skill arm too) don't see rows registered by
    /// a parallel test. Resets on acquire AND on drop (under the lock) so no row
    /// leaks past the test. Hold the returned guard for the whole test body.
    struct RegistryGuard(#[allow(dead_code)] std::sync::MutexGuard<'static, ()>);
    impl Drop for RegistryGuard {
        fn drop(&mut self) {
            compaction::invoked_skills::reset_for_test();
        }
    }
    fn registry_guard() -> RegistryGuard {
        let g = compaction::invoked_skills::TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        compaction::invoked_skills::reset_for_test();
        RegistryGuard(g)
    }

    async fn orch_with_bus(
        cwd: std::path::PathBuf,
        map: tool_api::read_file_state::ReadFileStateMap,
        sink: Arc<telemetry::InMemorySink>,
    ) -> ConversationOrchestrator {
        let bus = Arc::new(telemetry::AnalyticsBus::new());
        bus.attach_sink(sink).await;
        let config = OrchestratorConfig {
            plans_directory: Some("plans".to_string()),
            ..OrchestratorConfig::default()
        };
        ConversationOrchestrator::new(
            config,
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::with_files(vec![])),
            cwd,
        )
        .with_analytics_bus(bus)
        .with_read_state_map(map)
    }

    fn restore_names(events: &[telemetry::RecordedEvent]) -> Vec<String> {
        events
            .iter()
            .filter(|e| e.name.starts_with("tengu_post_compact_file_restore"))
            .map(|e| e.name.clone())
            .collect()
    }

    #[tokio::test]
    async fn reread_restores_fresh_content_not_stale_snapshot() {
        let _rg = registry_guard();
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("live.txt");
        // Snapshot recorded "OLD"; on disk the file now holds "NEW CONTENT".
        std::fs::write(&path, "NEW CONTENT").expect("write file");
        let map = tool_api::read_file_state::new_read_file_state_map();
        set(&map, path.clone(), stale_entry("OLD STALE SNAPSHOT"));

        let sink = Arc::new(telemetry::InMemorySink::new());
        let orch = orch_with_bus(dir.path().to_path_buf(), map, sink.clone()).await;

        let restored = orch.restore_post_compact_attachments().await;
        assert_eq!(restored.len(), 1, "the live file is restored");
        let body = restored[0].text_content();
        assert!(
            body.contains("NEW CONTENT"),
            "must restore FRESH disk content; got: {body}"
        );
        assert!(
            !body.contains("OLD STALE SNAPSHOT"),
            "must NOT restore the stale snapshot content; got: {body}"
        );

        // Exactly one success event fired, no error event.
        assert_eq!(
            restore_names(&sink.events().await),
            vec!["tengu_post_compact_file_restore_success".to_string()]
        );
    }

    #[tokio::test]
    async fn preserved_file_attachment_is_not_restored_twice() {
        let _rg = registry_guard();
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("already.txt");
        std::fs::write(&path, "current").expect("write file");
        let map = tool_api::read_file_state::new_read_file_state_map();
        set(&map, path.clone(), stale_entry("stale"));
        let sink = Arc::new(telemetry::InMemorySink::new());
        let orch = orch_with_bus(dir.path().to_path_buf(), map, sink.clone()).await;
        let boundary = protocol::ConversationMessage::user_meta(
            protocol::MessageId::new(),
            format!(
                "<system-reminder>\nReferenced file {} (restored after compaction):\ncurrent\n</system-reminder>",
                path.display()
            ),
        );

        let restored = orch
            .restore_post_compact_attachments_against(&[boundary])
            .await;
        assert!(restored.is_empty());
        assert!(restore_names(&sink.events().await).is_empty());
    }

    #[tokio::test]
    async fn plan_file_is_excluded_from_post_compact_restore() {
        let _rg = registry_guard();
        let dir = tempfile::tempdir().expect("tempdir");
        let map = tool_api::read_file_state::new_read_file_state_map();
        let sink = Arc::new(telemetry::InMemorySink::new());
        let orch = orch_with_bus(dir.path().to_path_buf(), map.clone(), sink.clone()).await;
        let session_id = orch.session.lock().await.session_id;
        let path = std::path::PathBuf::from(ConversationOrchestrator::plan_file_path(
            &session_id,
            dir.path(),
            Some("plans"),
        ));
        std::fs::create_dir_all(path.parent().expect("plans parent")).expect("create plans dir");
        std::fs::write(&path, "secret plan").expect("write plan");
        set(&map, path, stale_entry("stale plan"));

        let restored = orch.restore_post_compact_attachments().await;
        assert!(restored.is_empty());
        assert!(restore_names(&sink.events().await).is_empty());
    }

    #[tokio::test]
    async fn deleted_file_is_dropped_and_fires_error_event() {
        let _rg = registry_guard();
        let dir = tempfile::tempdir().expect("tempdir");
        // A path recorded in the snapshot but never written to disk (deleted).
        let missing = dir.path().join("gone.txt");
        let map = tool_api::read_file_state::new_read_file_state_map();
        set(
            &map,
            missing,
            stale_entry("content the model saw before deletion"),
        );

        let sink = Arc::new(telemetry::InMemorySink::new());
        let orch = orch_with_bus(dir.path().to_path_buf(), map, sink.clone()).await;

        let restored = orch.restore_post_compact_attachments().await;
        assert!(
            restored.is_empty(),
            "an unreadable/deleted file must be dropped, not restored from the stale snapshot"
        );
        assert_eq!(
            restore_names(&sink.events().await),
            vec!["tengu_post_compact_file_restore_error".to_string()]
        );
    }

    #[tokio::test]
    async fn host_seed_snapshot_is_not_restored_and_remains_cached() {
        let _rg = registry_guard();
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("seeded.txt");
        std::fs::write(&path, "seeded on disk").expect("write file");
        let map = tool_api::read_file_state::new_read_file_state_map();
        tool_api::read_file_state::set_with_model_context(
            &map,
            path.clone(),
            stale_entry("host seed snapshot"),
            false,
        );

        let sink = Arc::new(telemetry::InMemorySink::new());
        let orch = orch_with_bus(dir.path().to_path_buf(), map.clone(), sink.clone()).await;

        let restored = orch.restore_post_compact_attachments().await;
        assert!(
            restored.is_empty(),
            "host-seeded snapshots must not be restored into model context"
        );
        assert!(restore_names(&sink.events().await).is_empty());
        assert!(
            tool_api::read_file_state::get(&map, &path).is_some(),
            "host-seeded snapshot must stay cached for staleness/dedup after compaction"
        );
    }

    #[tokio::test]
    async fn success_and_error_events_fire_per_file() {
        let _rg = registry_guard();
        let dir = tempfile::tempdir().expect("tempdir");
        let live = dir.path().join("a.txt");
        std::fs::write(&live, "alive").expect("write file");
        let gone = dir.path().join("b.txt"); // never created

        let map = tool_api::read_file_state::new_read_file_state_map();
        // Higher mtime → selected/re-read first (DESC), but ordering of the two
        // telemetry events is not asserted — only the multiset.
        set(&map, live.clone(), stale_entry("stale-a"));
        set(&map, gone, stale_entry("stale-b"));

        let sink = Arc::new(telemetry::InMemorySink::new());
        let orch = orch_with_bus(dir.path().to_path_buf(), map, sink.clone()).await;

        let restored = orch.restore_post_compact_attachments().await;
        assert_eq!(restored.len(), 1, "only the live file survives");
        assert!(restored[0].text_content().contains("alive"));

        let mut names = restore_names(&sink.events().await);
        names.sort();
        assert_eq!(
            names,
            vec![
                "tengu_post_compact_file_restore_error".to_string(),
                "tengu_post_compact_file_restore_success".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn empty_read_state_restores_nothing_and_fires_no_events() {
        let _rg = registry_guard();
        let dir = tempfile::tempdir().expect("tempdir");
        let map = tool_api::read_file_state::new_read_file_state_map();
        let sink = Arc::new(telemetry::InMemorySink::new());
        let orch = orch_with_bus(dir.path().to_path_buf(), map, sink.clone()).await;

        let restored = orch.restore_post_compact_attachments().await;
        assert!(restored.is_empty());
        assert!(restore_names(&sink.events().await).is_empty());
    }

    // ── P2-12: post-compact SKILL restoration (`rRg`) ─────────────────────────

    #[tokio::test]
    async fn invoked_skill_reappears_as_attachment_post_compact() {
        let _rg = registry_guard();
        let dir = tempfile::tempdir().expect("tempdir");
        // No files read this turn — the skill arm must still run.
        let map = tool_api::read_file_state::new_read_file_state_map();
        let sink = Arc::new(telemetry::InMemorySink::new());
        let orch = orch_with_bus(dir.path().to_path_buf(), map, sink.clone()).await;

        // A skill was invoked before the compaction (main thread → agentId None).
        compaction::invoked_skills::register(
            "deploy",
            std::path::Path::new("/skills/deploy"),
            "Deploy guidelines: run the pipeline.",
            None,
        );

        let restored = orch.restore_post_compact_attachments().await;
        assert_eq!(restored.len(), 1, "one invoked_skills meta message");
        let body = restored[0].text_content();
        assert!(
            body.contains("The following skills were invoked EARLIER in this session"),
            "byte-faithful invoked_skills preamble; got: {body}"
        );
        assert!(body.contains("### Skill: deploy"));
        assert!(body.contains("Path: /skills/deploy"));
        assert!(body.contains("Deploy guidelines: run the pipeline."));

        // The registry SURVIVES compaction (not cleared) — a second restore still
        // sees the skill (documented no-clear rationale).
        let again = orch.restore_post_compact_attachments().await;
        assert_eq!(again.len(), 1, "registry survives compaction");
        assert!(again[0].text_content().contains("### Skill: deploy"));
    }

    #[tokio::test]
    async fn invoked_skill_already_in_preserved_attachment_is_not_duplicated() {
        let _rg = registry_guard();
        let dir = tempfile::tempdir().expect("tempdir");
        let map = tool_api::read_file_state::new_read_file_state_map();
        let sink = Arc::new(telemetry::InMemorySink::new());
        let orch = orch_with_bus(dir.path().to_path_buf(), map, sink).await;

        let content = "Deploy guidelines: run the pipeline.";
        compaction::invoked_skills::register(
            "deploy",
            std::path::Path::new("/skills/deploy"),
            content,
            None,
        );
        let preserved = orch.restore_post_compact_attachments().await;
        assert_eq!(preserved.len(), 1, "first compaction restores the skill");

        let restored = orch
            .restore_post_compact_attachments_against(&preserved)
            .await;
        assert!(
            restored.is_empty(),
            "preserved invoked-skills content must not be emitted twice"
        );
    }

    #[tokio::test]
    async fn invoked_skill_with_markdown_separator_is_deduped_without_parsing_its_content() {
        let _rg = registry_guard();
        let dir = tempfile::tempdir().expect("tempdir");
        let map = tool_api::read_file_state::new_read_file_state_map();
        let sink = Arc::new(telemetry::InMemorySink::new());
        let orch = orch_with_bus(dir.path().to_path_buf(), map, sink).await;

        let content = "Deploy the first stage.\n\n---\n\nThen deploy the second stage.";
        compaction::invoked_skills::register(
            "deploy",
            std::path::Path::new("/skills/deploy"),
            content,
            None,
        );

        let first = orch.restore_post_compact_attachments().await;
        assert_eq!(first.len(), 1, "first compaction restores the skill");

        let persisted = orch.to_jsonl_message(&first[0], "session", None, None, None, None);
        let persisted: session::JsonlMessage = serde_json::from_str(
            &serde_json::to_string(&persisted).expect("serialize attachment metadata"),
        )
        .expect("round-trip attachment metadata");
        assert_eq!(
            persisted
                .extra
                .get("invokedSkillContents")
                .and_then(serde_json::Value::as_array)
                .and_then(|contents| contents.first())
                .and_then(serde_json::Value::as_str),
            Some(content),
            "the opaque body must be persisted without delimiter parsing"
        );

        let resumed = orch_with_bus(
            dir.path().to_path_buf(),
            tool_api::read_file_state::new_read_file_state_map(),
            Arc::new(telemetry::InMemorySink::new()),
        )
        .await;
        resumed
            .restore_resume_runtime_metadata(std::slice::from_ref(&persisted))
            .await;
        let replayed =
            crate::state_from_messages(uuid::Uuid::new_v4(), std::slice::from_ref(&persisted));
        let restored = resumed
            .restore_post_compact_attachments_against(&replayed.history)
            .await;
        assert!(
            restored.is_empty(),
            "resume followed by compact must preserve structural dedup even when Markdown contains the renderer separator"
        );
    }

    #[tokio::test]
    async fn no_invoked_skills_restores_no_skill_message() {
        let _rg = registry_guard();
        let dir = tempfile::tempdir().expect("tempdir");
        let map = tool_api::read_file_state::new_read_file_state_map();
        let sink = Arc::new(telemetry::InMemorySink::new());
        let orch = orch_with_bus(dir.path().to_path_buf(), map, sink.clone()).await;

        // Empty registry → no skill attachment (and no file snapshot → nothing).
        let restored = orch.restore_post_compact_attachments().await;
        assert!(restored.is_empty());
    }

    #[tokio::test]
    async fn subagent_skill_not_restored_on_main_thread() {
        let _rg = registry_guard();
        let dir = tempfile::tempdir().expect("tempdir");
        let map = tool_api::read_file_state::new_read_file_state_map();
        let sink = Arc::new(telemetry::InMemorySink::new());
        let orch = orch_with_bus(dir.path().to_path_buf(), map, sink.clone()).await;

        // A skill invoked under a subagent (agentId Some) must NOT surface on the
        // main-thread (agentId None) restore — `kGo` filters by agentId.
        compaction::invoked_skills::register(
            "child-skill",
            std::path::Path::new("/skills/child"),
            "child body",
            Some("agent:child"),
        );

        let restored = orch.restore_post_compact_attachments().await;
        assert!(
            restored.is_empty(),
            "subagent skill must not restore on the main thread"
        );
    }
}

#[cfg(test)]
mod seed_read_state_from_host_tests {
    use super::*;
    use crate::test_support::{
        noop_hook_executor, MockApiClient, MockOutputStream, NoOpPermissionGate,
        StaticMemoryProvider,
    };
    use crate::OrchestratorConfig;
    use std::sync::Arc;
    use tool_api::registry::ToolRegistry;

    fn test_orchestrator(cwd: std::path::PathBuf) -> ConversationOrchestrator {
        ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::with_files(vec![])),
            cwd,
        )
    }

    #[tokio::test]
    async fn seed_read_state_normalizes_bom_and_crlf_to_lf() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("seeded.txt");
        std::fs::write(&path, b"\xEF\xBB\xBFalpha\r\nbeta\rgamma\n").expect("write fixture");
        let orch = test_orchestrator(dir.path().to_path_buf());
        let host_mtime_ms = std::fs::metadata(&path)
            .expect("metadata")
            .modified()
            .expect("mtime")
            .duration_since(std::time::UNIX_EPOCH)
            .expect("post epoch")
            .as_millis() as f64
            + 1.0;

        assert!(
            orch.seed_read_state_from_host("seeded.txt", host_mtime_ms)
                .await
        );

        let entry = tool_api::read_file_state::get(&orch.read_state_map, &path)
            .expect("seeded entry present");
        assert_eq!(entry.content, "alpha\nbeta\ngamma\n");
        assert!(!entry.from_read);
        assert!(
            orch.read_state_map
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .model_context_keys()
                .is_empty(),
            "host-seeded content must not appear as model-visible context"
        );
    }
}

/// P2-02 (cc2.1.207): `--agent` adopts a main-thread agent — its system prompt
/// becomes the main-loop system prompt (claude-code `nre`, `--system-prompt`
/// still winning) and its `agentType` rides every main-thread lifecycle hook
/// payload (`bde`/`MB()`, base builder `wf` `?? MB()`).
#[cfg(test)]
mod main_thread_agent_tests {
    use super::*;
    use crate::test_support::{
        noop_hook_executor, MockApiClient, MockOutputStream, NoOpPermissionGate,
        StaticMemoryProvider,
    };
    use crate::OrchestratorConfig;
    use async_trait::async_trait;
    use serde_json::json;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tool_api::context::ToolUseContext;
    use tool_api::progress::ToolProgressSender;
    use tool_api::registry::ToolRegistry;
    use tool_api::tool_trait::{
        DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolStaticContext,
        ValidationError,
    };

    /// The "no `tools:` frontmatter" policy (claude `s===undefined`) — keeps the
    /// whole tool pool. Used as the default in the system-prompt-focused tests.
    fn keep_all_tools() -> agent::AgentToolPolicy {
        agent::AgentToolPolicy::All {
            use_exact_tools: false,
        }
    }

    fn orch_with_config(config: OrchestratorConfig) -> ConversationOrchestrator {
        ConversationOrchestrator::new(
            config,
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::with_files(vec![])),
            PathBuf::from("/work/repo"),
        )
    }

    struct LiveModeGate(std::sync::RwLock<String>);

    #[async_trait]
    impl PermissionGate for LiveModeGate {
        async fn check(
            &self,
            _tool_name: &str,
            _input: &serde_json::Value,
        ) -> traits::PermissionDecision {
            traits::PermissionDecision::Allow
        }

        async fn set_permission_mode(&self, mode: &str) -> Result<(), String> {
            *self.0.write().expect("live mode write lock") = mode.to_string();
            Ok(())
        }

        fn permission_mode(&self) -> Option<String> {
            Some(self.0.read().expect("live mode read lock").clone())
        }
    }

    /// A minimal builtin tool exposing a fixed `name()` — enough for the wire
    /// serializer (`build_wire_tools`) to advertise it and for the main-thread
    /// agent filter to inspect its name.
    struct NamedTool(&'static str);

    #[async_trait]
    impl Tool for NamedTool {
        fn name(&self) -> &str {
            self.0
        }
        fn input_schema(&self) -> &serde_json::Value {
            static SCHEMA: std::sync::OnceLock<serde_json::Value> = std::sync::OnceLock::new();
            SCHEMA.get_or_init(|| json!({ "type": "object", "properties": {} }))
        }
        fn is_enabled(&self, _ctx: &ToolStaticContext) -> bool {
            true
        }
        fn max_result_size_chars(&self) -> usize {
            1024
        }
        fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
            true
        }
        fn is_read_only(&self, _input: &serde_json::Value) -> bool {
            true
        }
        async fn validate_input(
            &self,
            _input: &serde_json::Value,
            _ctx: &ToolUseContext,
        ) -> Result<(), ValidationError> {
            Ok(())
        }
        async fn check_permissions(
            &self,
            _input: &serde_json::Value,
            _ctx: &ToolUseContext,
        ) -> permission::PermissionResult {
            permission::PermissionResult::Allow {
                reason: permission::PermissionDecisionReason::Other {
                    reason: "test".into(),
                },
                updated_input: None,
                update_destination: None,
                metadata: permission::result::PermissionMetadata::default(),
            }
        }
        async fn description(
            &self,
            _input: &serde_json::Value,
            _opts: &DescriptionOptions,
        ) -> String {
            self.0.into()
        }
        async fn prompt(&self, _opts: &PromptOptions) -> String {
            String::new()
        }
        async fn call(
            &self,
            _input: serde_json::Value,
            _ctx: ToolUseContext,
            _tx: ToolProgressSender,
        ) -> Result<ToolCallResult, ToolError> {
            Ok(ToolCallResult {
                data: json!({ "content": "ok" }),
                model_content: None,
                new_messages: vec![],
                context_modifier: None,
                is_error: false,
                mcp_meta: None,
            })
        }
    }

    /// Build an orchestrator whose registry advertises the named builtin tools.
    fn orch_with_tools(names: &[&'static str]) -> ConversationOrchestrator {
        let mut registry = ToolRegistry::new();
        for n in names {
            registry.register_builtin(Arc::new(NamedTool(n)) as Arc<dyn Tool>);
        }
        ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(registry),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::with_files(vec![])),
            PathBuf::from("/work/repo"),
        )
    }

    /// The set of `name` fields the wire tool array advertises.
    async fn wire_tool_names(orch: &ConversationOrchestrator) -> Vec<String> {
        orch.build_wire_tools()
            .await
            .into_iter()
            .filter_map(|t| {
                t.get("name")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            })
            .collect()
    }

    /// A dead OAuth session must actually REACH the user as "Login expired",
    /// not as the variant's bare `Display`. This is the wiring half of
    /// `api_error_copy::oauth_refresh_dead_text` — the copy constants have their
    /// own byte-exact tests, but a `match` arm that is never taken renders
    /// nothing, and a unit test of the constant cannot detect that.
    #[tokio::test]
    async fn a_dead_oauth_session_renders_the_login_expired_copy() {
        let mut config = OrchestratorConfig::default();
        config.interactive_session = true;
        let orch = orch_with_config(config);
        assert_eq!(
            orch.model_error_text(&LlmError::OAuthRefreshDead).await,
            "Login expired \u{b7} Please run /connect"
        );

        // Same error, no TTY: the copy must stop naming a command the caller
        // cannot run.
        let mut headless = OrchestratorConfig::default();
        headless.interactive_session = false;
        let orch = orch_with_config(headless);
        assert_eq!(
            orch.model_error_text(&LlmError::OAuthRefreshDead).await,
            "Failed to authenticate: OAuth session expired and could not be refreshed"
        );
    }

    /// A real provider 403 now KEEPS its message, so the auth-copy family is
    /// reachable. Before `Authentication`/`PermissionDenied` carried a message,
    /// the decoder dropped it at the provider boundary and every branch below
    /// gated on `Display` ("permission denied") — none could ever match.
    #[tokio::test]
    async fn a_real_403_keeps_the_message_the_auth_branches_gate_on() {
        // Exactly what `providers::map_error_status(403, …)` now produces.
        let revoked = LlmError::PermissionDenied {
            message: "403 OAuth token has been revoked".to_string(),
        };
        assert_eq!(revoked.http_status(), Some(403), "prefix survives");
        assert!(crate::api_error_copy::is_oauth_revoked(
            revoked.http_status(),
            revoked.provider_message().unwrap_or_default()
        ));

        let orch = orch_with_config(OrchestratorConfig::default());
        assert_eq!(
            orch.model_error_text(&revoked).await,
            "Your account does not have access to Claude. Please login again or \
             contact your administrator."
        );

        // The org-level OAuth block reaches its own copy too.
        let org_block = LlmError::PermissionDenied {
            message: "403 OAuth authentication is currently not allowed for this organization"
                .to_string(),
        };
        assert_eq!(
            orch.model_error_text(&org_block).await,
            crate::api_error_copy::OAUTH_ORG_NOT_ALLOWED
        );

        // A 403 with unrelated text reaches the oracle's TERMINAL arm, which
        // still carries the provider detail — NOT the bare `Display`.
        let plain = LlmError::PermissionDenied {
            message: "403 forbidden".to_string(),
        };
        assert_eq!(
            orch.model_error_text(&plain).await,
            "Failed to authenticate. API Error: 403 forbidden",
            "default config is non-interactive"
        );

        // The COMMON shape: the SDK stringifies the whole body into the
        // message, and the oracle unwraps it rather than showing raw JSON.
        let json_body = LlmError::PermissionDenied {
            message: r#"403 {"type":"error","error":{"message":"quota gone"}}"#.to_string(),
        };
        assert_eq!(
            orch.model_error_text(&json_body).await,
            "Failed to authenticate. API Error: 403 quota gone"
        );
    }

    /// `/status` warnings must reflect the CURRENT memory set, not a launch
    /// snapshot: a LINGXI.md that grows past the limit mid-session is exactly
    /// the case the panel exists to report.
    #[tokio::test]
    async fn large_memory_warnings_are_recomputed_from_the_live_memory_set() {
        use traits::OrchestratorHandle as _;

        // The default model is `claude-opus-4-8`, a 1M-context model, so the
        // threshold is 200_000 chars — NOT the 40_000 floor. Sizing the fixture
        // against the floor would have made this pass for the wrong reason.
        let big = "x".repeat(250_000);
        let file = crate::prompt::MemoryFile {
            path: std::path::PathBuf::from("/work/repo/LINGXI.md"),
            body: big,
            is_local_override: false,
            tier: crate::prompt::LingxiMdTier::Project,
            globs: None,
            raw_content: String::new(),
            content_differs_from_disk: false,
        };
        let orch = ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::with_files(vec![file])),
            PathBuf::from("/work/repo"),
        );
        let rows = orch
            .large_memory_warnings()
            .await
            .expect("production orchestrator can recompute memory warnings");
        assert_eq!(
            rows.len(),
            1,
            "the oversized file must be reported: {rows:?}"
        );
        assert!(
            rows[0].starts_with("Large ") && rows[0].contains("will impact performance"),
            "oracle row shape: {}",
            rows[0]
        );

        // An empty memory set reports nothing — the panel stays byte-identical.
        let empty = ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::with_files(vec![])),
            PathBuf::from("/work/repo"),
        );
        assert_eq!(empty.large_memory_warnings().await, Some(Vec::new()));
    }

    /// The org-level OAuth block must reach the user as its own copy, and must
    /// NOT be confused with the API-key disablement — they prescribe opposite
    /// remedies.
    #[tokio::test]
    async fn an_org_oauth_block_tells_the_user_to_use_an_api_key() {
        let orch = orch_with_config(OrchestratorConfig::default());
        let err = LlmError::InvalidRequest {
            message: "403 OAuth authentication is currently not allowed for this organization"
                .to_string(),
        };
        // The gate keys on Authentication/PermissionDenied, so route it the way
        // a real decode would.
        let denied = LlmError::PermissionDenied {
            message: String::new(),
        };
        assert!(
            !crate::api_error_copy::is_oauth_org_not_allowed(
                denied.http_status(),
                &denied.to_string()
            ),
            "a bare PermissionDenied carries no message and must not match"
        );
        assert!(crate::api_error_copy::is_oauth_org_not_allowed(
            Some(403),
            &err.to_string()
        ));
        let text = orch
            .model_error_text(&LlmError::PermissionDenied {
                message: String::new(),
            })
            .await;
        assert!(
            !text.contains("disabled Claude subscription access"),
            "a bare 403 with no message must not claim an org block: {text}"
        );
    }

    /// The sibling failures must NOT claim the login expired: a plain auth
    /// failure keeps its own text, so the new arm cannot swallow them.
    #[tokio::test]
    async fn an_ordinary_auth_failure_is_not_reported_as_an_expired_login() {
        let mut config = OrchestratorConfig::default();
        config.interactive_session = true;
        let orch = orch_with_config(config);
        let text = orch
            .model_error_text(&LlmError::Authentication {
                message: String::new(),
            })
            .await;
        assert!(
            !text.contains("Login expired"),
            "a generic auth failure must not be rendered as an expired login: {text}"
        );
    }

    /// A resolved `--agent` with a prompt REPLACES the assembled default system
    /// prompt on every query (claude-code `nre` uses `agentDef.getSystemPrompt()`
    /// as the whole system prompt, exactly like `--system-prompt`).
    #[tokio::test]
    async fn main_thread_agent_prompt_replaces_default() {
        let orch = orch_with_config(OrchestratorConfig::default());
        let default = orch.assemble_system_prompt_preview().await;
        orch.set_main_thread_agent(
            "code-reviewer".to_string(),
            Some("You are a meticulous code reviewer.".to_string()),
            keep_all_tools(),
            Vec::new(),
            None,
        )
        .await;
        let after = orch.assemble_system_prompt_preview().await;
        assert_eq!(after, "You are a meticulous code reviewer.");
        assert_ne!(
            after, default,
            "the agent prompt must replace the assembled default"
        );
    }

    /// `--system-prompt` (`system_prompt_override` / claude `overrideSystemPrompt`)
    /// beats the main-thread agent's prompt — `nre` returns `Zu([overrideSystemPrompt])`
    /// before ever consulting the agent definition.
    #[tokio::test]
    async fn system_prompt_override_beats_main_thread_agent() {
        let mut config = OrchestratorConfig::default();
        config.system_prompt_override = Some("EXPLICIT --system-prompt wins".to_string());
        let orch = orch_with_config(config);
        orch.set_main_thread_agent(
            "code-reviewer".to_string(),
            Some("agent prompt should be ignored".to_string()),
            keep_all_tools(),
            Vec::new(),
            None,
        )
        .await;
        assert_eq!(
            orch.assemble_system_prompt_preview().await,
            "EXPLICIT --system-prompt wins"
        );
    }

    /// An adopted agent that declares NO prompt falls through to the assembled
    /// default (claude `getSystemPrompt()` -> undefined -> default path).
    #[tokio::test]
    async fn main_thread_agent_without_prompt_uses_default() {
        let orch = orch_with_config(OrchestratorConfig::default());
        let default = orch.assemble_system_prompt_preview().await;
        orch.set_main_thread_agent(
            "promptless".to_string(),
            None,
            keep_all_tools(),
            Vec::new(),
            None,
        )
        .await;
        assert_eq!(orch.assemble_system_prompt_preview().await, default);
    }

    /// The adopted agent's `agentType` rides main-thread lifecycle hook payloads
    /// (`expansion_hook_context` shares the `lifecycle_hook_ctx` builder that
    /// `SessionStart` / `UserPromptSubmit` / `Stop` use). `None` before any
    /// `--agent` is applied.
    #[tokio::test]
    async fn lifecycle_hook_ctx_carries_main_thread_agent_type() {
        let orch = orch_with_config(OrchestratorConfig::default());
        assert_eq!(orch.expansion_hook_context().await.agent_type, None);
        orch.set_main_thread_agent(
            "code-reviewer".to_string(),
            None,
            keep_all_tools(),
            Vec::new(),
            None,
        )
        .await;
        assert_eq!(
            orch.expansion_hook_context().await.agent_type,
            Some("code-reviewer".to_string())
        );
    }

    #[tokio::test]
    async fn lifecycle_hook_ctx_reports_live_permission_mode() {
        let orch = ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(LiveModeGate(std::sync::RwLock::new("default".to_string()))),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::with_files(vec![])),
            PathBuf::from("/work/repo"),
        );

        orch.set_permission_mode("acceptEdits")
            .await
            .expect("set live permission mode");
        assert_eq!(
            orch.expansion_hook_context().await.permission_mode,
            Some("acceptEdits".to_string())
        );

        orch.session.lock().await.plan_mode = true;
        assert_eq!(
            orch.expansion_hook_context().await.permission_mode,
            Some("plan".to_string()),
            "explicit /plan state takes precedence over the gate's last mode"
        );
    }

    #[tokio::test]
    async fn lifecycle_hook_ctx_uses_trimmed_last_assistant_text() {
        let orch = ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::with_files(vec![])),
            PathBuf::from("/work/repo"),
        );

        {
            let session_handle = orch.session();
            let mut session = session_handle.lock().await;
            session.history.push(protocol::ConversationMessage::user(
                protocol::MessageId::new(),
                "earlier user".to_string(),
            ));
            session
                .history
                .push(protocol::ConversationMessage::Assistant {
                    id: protocol::MessageId::new(),
                    content: vec![
                        protocol::ContentBlock::Text {
                            text: " first line ".to_string(),
                        },
                        protocol::ContentBlock::Thinking {
                            thinking: "hidden".to_string(),
                            signature: None,
                        },
                        protocol::ContentBlock::Text {
                            text: "second line ".to_string(),
                        },
                    ],
                    stop_reason: Some("end_turn".to_string()),
                });
        }

        assert_eq!(
            orch.expansion_hook_context().await.last_assistant_message,
            Some("first line \nsecond line".to_string()),
            "hook context must join the last assistant's text blocks with newlines and trim outer whitespace"
        );

        orch.session()
            .lock()
            .await
            .history
            .push(protocol::ConversationMessage::Assistant {
                id: protocol::MessageId::new(),
                content: vec![protocol::ContentBlock::Text {
                    text: "   ".to_string(),
                }],
                stop_reason: Some("end_turn".to_string()),
            });
        assert_eq!(
            orch.expansion_hook_context().await.last_assistant_message,
            None,
            "an all-whitespace final assistant message must surface as None"
        );
    }

    /// An adopted agent with a `tools:` allow-list narrows the advertised main-
    /// loop tool pool to the named tools (claude `HJ(agentDef,to,!1,!0)` with an
    /// explicit `s`: only listed tools survive). Tools not in the list are
    /// dropped; a listed name that does not exist is simply absent.
    #[tokio::test]
    async fn main_thread_agent_explicit_tools_narrow_the_pool() {
        let orch = orch_with_tools(&["Read", "Write", "Bash", "Grep"]);
        // No agent yet ⇒ every registered tool is advertised.
        let before = wire_tool_names(&orch).await;
        assert_eq!(before, vec!["Bash", "Grep", "Read", "Write"]);

        orch.set_main_thread_agent(
            "reviewer".to_string(),
            None,
            agent::AgentToolPolicy::Explicit(vec!["Read".to_string(), "Grep".to_string()]),
            Vec::new(),
            None,
        )
        .await;
        let after = wire_tool_names(&orch).await;
        assert_eq!(after, vec!["Grep", "Read"]);
    }

    /// `AgentToolPolicy::All` (no `tools:` frontmatter, claude `s===undefined`)
    /// keeps the WHOLE pool — including tools the SUBAGENT filter would strip as
    /// "always-disallowed" (ExitPlanMode / AskUserQuestion). The main-thread
    /// filter runs `HJ` with `n=true`, which bypasses that strip. Regression
    /// guard against accidentally reusing the subagent resolver here.
    #[tokio::test]
    async fn main_thread_agent_all_policy_keeps_always_disallowed_tools() {
        let orch = orch_with_tools(&["Read", "ExitPlanMode", "AskUserQuestion"]);
        orch.set_main_thread_agent(
            "planner".to_string(),
            None,
            keep_all_tools(),
            Vec::new(),
            None,
        )
        .await;
        let after = wire_tool_names(&orch).await;
        assert_eq!(after, vec!["AskUserQuestion", "ExitPlanMode", "Read"]);
    }

    /// The agent's per-definition `disallowedTools` subtracts from the pool
    /// (base tool name; a trailing `(rule)` is stripped) BEFORE the `tools:`
    /// projection — claude `HJ` `g=u.filter(P=>!isToolDisallowed(P))`.
    #[tokio::test]
    async fn main_thread_agent_disallowed_tools_subtract() {
        let orch = orch_with_tools(&["Read", "Write", "Bash"]);
        orch.set_main_thread_agent(
            "safe".to_string(),
            None,
            keep_all_tools(),
            vec!["Write".to_string(), "Bash(rm -rf)".to_string()],
            None,
        )
        .await;
        let after = wire_tool_names(&orch).await;
        assert_eq!(after, vec!["Read"]);
    }

    #[tokio::test]
    async fn mobile_runtime_reminder_is_stable_across_agent_tool_filters() {
        let orch = orch_with_tools(&["Read", "Shell"]);
        let orch = orch.with_mobile_runtime_environment(traits::MobileRuntimeEnvironment::new(
            traits::MobileHostEnvironment::new(
                traits::MobileHostOs::Ios,
                Some("19.0".into()),
                traits::MobileDeviceClass::Phone,
                traits::MobileExecutionTarget::PhysicalDevice,
                traits::MobileLaunchMode::Interactive,
            ),
            traits::MobileToolRuntime::MobileLinuxGuest,
            Some("/workspace/a".into()),
            Some("/bin/sh".into()),
            Some("Mobile Linux sh".into()),
            traits::MobileNetworkPolicy::PermissionMediated,
            traits::MobileLifecyclePolicy::IosFiniteBackgroundAssertion,
        ));

        let before = orch
            .mobile_runtime_environment_preview()
            .await
            .expect("runtime reminder");
        orch.set_main_thread_agent(
            "reviewer".to_string(),
            None,
            agent::AgentToolPolicy::Explicit(vec!["Read".to_string()]),
            Vec::new(),
            None,
        )
        .await;

        let after = orch
            .mobile_runtime_environment_preview()
            .await
            .expect("runtime reminder");
        assert_eq!(after, before);
        assert!(after.contains("per-agent availability is defined by registered tool schemas"));
        assert!(!after.contains("available to this agent"));
    }

    /// A resolved `--agent` model (`Some(resolved_id)`) replaces the session
    /// model (claude `jb(Zo(y.model))`); the caller has already gated it on
    /// `!userSpecifiedModel` and resolved the alias to a wire id. The profile is
    /// cleared (agent frontmatter carries a bare id).
    #[tokio::test]
    async fn main_thread_agent_model_override_replaces_session_model() {
        let mut config = OrchestratorConfig::default();
        config.model = "base-model".to_string();
        let orch = orch_with_config(config);
        assert_eq!(orch.session().lock().await.model, "base-model");

        orch.set_main_thread_agent(
            "fast".to_string(),
            None,
            keep_all_tools(),
            Vec::new(),
            Some("claude-agent-model".to_string()),
        )
        .await;
        let session = orch.session();
        let s = session.lock().await;
        assert_eq!(s.model, "claude-agent-model");
        assert_eq!(s.model_profile, None);
    }

    /// `model_override == None` (agent `model: inherit`, or the user passed
    /// `--model` so the caller gated it out) leaves the session model untouched.
    #[tokio::test]
    async fn main_thread_agent_no_model_override_leaves_session_model() {
        let mut config = OrchestratorConfig::default();
        config.model = "base-model".to_string();
        let orch = orch_with_config(config);
        orch.set_main_thread_agent(
            "inheritor".to_string(),
            None,
            keep_all_tools(),
            Vec::new(),
            None,
        )
        .await;
        assert_eq!(orch.session().lock().await.model, "base-model");
    }
}

/// `plansDirectory` (206 `iT`) resolution + within-root containment.
#[cfg(test)]
mod plans_dir_tests {
    use super::*;
    use std::path::Path;
    use tempfile::TempDir;

    fn repo_root() -> TempDir {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join(".git")).unwrap();
        tmp
    }

    #[test]
    fn none_setting_falls_back_to_default_config_home_plans() {
        let root = Path::new("/home/u/project");
        let got = ConversationOrchestrator::plans_dir(root, None);
        // Default: `<config-home>/plans` (never under the project root).
        assert_eq!(got, ConversationOrchestrator::default_plans_dir());
        assert!(got.ends_with("plans"));
    }

    #[test]
    fn empty_setting_is_treated_as_absent() {
        let root = Path::new("/home/u/project");
        let got = ConversationOrchestrator::plans_dir(root, Some(""));
        assert_eq!(got, ConversationOrchestrator::default_plans_dir());
    }

    #[test]
    fn relative_within_root_is_accepted_and_resolved() {
        let repo = repo_root();
        let expected = repo.path().join("docs/plans");
        let got = ConversationOrchestrator::plans_dir(repo.path(), Some("docs/plans"));
        assert_eq!(got, expected);
    }

    #[test]
    fn dot_segments_normalize_but_stay_within_root() {
        let repo = repo_root();
        let expected = repo.path().join("plans");
        let got = ConversationOrchestrator::plans_dir(repo.path(), Some("./sub/../plans"));
        assert_eq!(got, expected);
    }

    #[test]
    fn parent_escape_is_rejected_and_falls_back_to_default() {
        // `../outside` normalizes to `/home/u/outside`, which is NOT within the
        // project root → reject, log the error, use the default.
        let root = Path::new("/home/u/project");
        let got = ConversationOrchestrator::plans_dir(root, Some("../outside"));
        assert_eq!(got, ConversationOrchestrator::default_plans_dir());
    }

    #[test]
    fn absolute_outside_root_is_rejected() {
        let root = Path::new("/home/u/project");
        let got = ConversationOrchestrator::plans_dir(root, Some("/etc/evil"));
        assert_eq!(got, ConversationOrchestrator::default_plans_dir());
    }

    #[test]
    fn absolute_inside_root_is_accepted() {
        // `path.resolve` uses an absolute value verbatim; if it happens to be
        // within the project root it is accepted.
        let repo = repo_root();
        let inside = repo.path().join("plans");
        let got = ConversationOrchestrator::plans_dir(
            repo.path(),
            Some(inside.to_string_lossy().as_ref()),
        );
        assert_eq!(got, inside);
    }

    #[test]
    fn project_root_itself_is_within_root() {
        // `o === n` branch of W5_ — the plans dir equal to the root is accepted.
        let repo = repo_root();
        let got = ConversationOrchestrator::plans_dir(repo.path(), Some("."));
        assert_eq!(got, repo.path());
    }

    #[test]
    fn sibling_prefix_is_not_confused_for_containment() {
        // Component-wise containment: `/home/u/project-evil` must NOT count as
        // within `/home/u/project` (a naive string prefix would wrongly accept).
        let root = Path::new("/home/u/project");
        let got = ConversationOrchestrator::plans_dir(root, Some("/home/u/project-evil"));
        assert_eq!(got, ConversationOrchestrator::default_plans_dir());
    }

    #[test]
    fn windows_containment_comparison_is_case_insensitive() {
        let relative = path_relative_components(
            Path::new("/Repo/Project"),
            Path::new("/repo/project/docs/plans"),
            true,
        )
        .expect("Windows-style comparison should accept path casing differences");
        assert_eq!(
            relative,
            vec![
                std::ffi::OsString::from("docs"),
                std::ffi::OsString::from("plans")
            ]
        );
        assert!(
            path_relative_components(
                Path::new("/Repo/Project"),
                Path::new("/repo/project-evil/plans"),
                true,
            )
            .is_none(),
            "component comparison must still reject sibling prefixes"
        );
    }

    #[test]
    fn protected_directory_component_is_rejected() {
        let repo = repo_root();
        let got = ConversationOrchestrator::plans_dir(repo.path(), Some(".git/plans"));
        assert_eq!(got, ConversationOrchestrator::default_plans_dir());
    }

    #[test]
    fn nested_repository_boundary_is_rejected() {
        let repo = repo_root();
        std::fs::create_dir_all(repo.path().join("nested/.git")).unwrap();
        std::fs::create_dir_all(repo.path().join("nested/plans")).unwrap();
        let got = ConversationOrchestrator::plans_dir(repo.path(), Some("nested/plans"));
        assert_eq!(got, ConversationOrchestrator::default_plans_dir());
    }

    #[test]
    fn existing_file_component_is_rejected() {
        let repo = repo_root();
        std::fs::write(repo.path().join("README.md"), "x").unwrap();
        let got = ConversationOrchestrator::plans_dir(repo.path(), Some("README.md/plans"));
        assert_eq!(got, ConversationOrchestrator::default_plans_dir());
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_component_is_rejected() {
        let repo = repo_root();
        let outside = TempDir::new().unwrap();
        std::fs::create_dir_all(outside.path().join("plans")).unwrap();
        std::os::unix::fs::symlink(outside.path(), repo.path().join("linked")).unwrap();
        let got = ConversationOrchestrator::plans_dir(repo.path(), Some("linked/plans"));
        assert_eq!(got, ConversationOrchestrator::default_plans_dir());
    }

    #[test]
    fn plan_file_path_joins_uuid_md_under_resolved_dir() {
        let sid = SessionId::new();
        let repo = repo_root();
        let path = ConversationOrchestrator::plan_file_path(&sid, repo.path(), Some("docs/plans"));
        let expected = repo
            .path()
            .join("docs/plans")
            .join(format!("{}.md", sid.as_uuid()));
        assert_eq!(path, expected.to_string_lossy());
    }
}
