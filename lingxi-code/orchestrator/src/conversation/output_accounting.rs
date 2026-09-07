//! Captured main-response participation in the host's shared output book.
use super::*;
use platform_api::{WorkflowOutputEventId, WorkflowOutputScope};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone)]
pub(crate) struct OutputTurn {
    scope: WorkflowOutputScope,
    failed: Arc<AtomicBool>,
}

pub(crate) struct MainOutputObservation {
    turn: OutputTurn,
    event: MessageId,
    visible: u64,
    reasoning: u64,
    observed: bool,
    finished: bool,
}

impl MainOutputObservation {
    pub(crate) fn observe(&mut self, usage: &llm_client::Usage) {
        // Provider-normalized cumulative buckets are disjoint. Partial frames
        // may omit a bucket; never replace previously known counts with zero.
        self.visible = self.visible.max(usage.billable_tokens.output);
        self.reasoning = self.reasoning.max(usage.billable_tokens.reasoning_output);
        self.observed = true;
    }

    fn observe_completed(&mut self, usage: &llm_client::Usage) {
        // A completed output report replaces provisional bucket splits;
        // max-per-bucket would double count reclassified reasoning tokens.
        // An absent/default Completed usage must not erase retained partials.
        // Speed/context/diagnostic metadata alone is not an output report.
        let has_output = usage.billable_tokens.output != 0
            || usage.billable_tokens.reasoning_output != 0
            || ["output_tokens", "completion_tokens", "candidatesTokenCount"]
                .iter()
                .any(|key| {
                    usage
                        .provider_metadata
                        .get(key)
                        .and_then(serde_json::Value::as_u64)
                        .is_some()
                });
        if has_output {
            self.visible = usage.billable_tokens.output;
            self.reasoning = usage.billable_tokens.reasoning_output;
            self.observed = true;
        }
    }

    pub(crate) fn finish(&mut self) -> Result<(), OrchestratorError> {
        if self.finished {
            return Ok(());
        }
        self.finished = true;
        if !self.observed {
            return Ok(());
        }
        let result = self
            .visible
            .checked_add(self.reasoning)
            .ok_or_else(|| "main response output overflow".to_string())
            .and_then(|tokens| {
                self.turn
                    .scope
                    .record_legacy(WorkflowOutputEventId::MainResponse(self.event), tokens)
                    .map_err(|error| error.to_string())
            });
        result.map_err(|error| {
            self.turn.failed.store(true, Ordering::Release);
            OrchestratorError::Internal(format!("output accounting failed: {error}"))
        })
    }
}

impl Drop for MainOutputObservation {
    fn drop(&mut self) {
        // Cancellation retains already observed real usage, never estimated
        // text lengths. A failed drop publication blocks subsequent dispatch.
        let _ = self.finish();
    }
}

pub(crate) fn account_stream(
    stream: futures::stream::BoxStream<'static, Result<llm_client::LlmEvent, llm_client::LlmError>>,
    observation: Option<MainOutputObservation>,
) -> futures::stream::BoxStream<'static, Result<llm_client::LlmEvent, llm_client::LlmError>> {
    let Some(observation) = observation else {
        return stream;
    };
    Box::pin(OutputStream {
        stream,
        observation,
    })
}

struct OutputStream {
    stream: futures::stream::BoxStream<'static, Result<llm_client::LlmEvent, llm_client::LlmError>>,
    observation: MainOutputObservation,
}

impl futures::Stream for OutputStream {
    type Item = Result<llm_client::LlmEvent, llm_client::LlmError>;
    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        use llm_client::LlmEvent;
        use std::task::Poll;
        let this = self.get_mut();
        let polled = this.stream.as_mut().poll_next(cx);
        let terminal = match &polled {
            Poll::Ready(Some(Ok(event))) => {
                match event {
                    LlmEvent::MessageStart { response } => {
                        this.observation.observe(&response.usage)
                    }
                    LlmEvent::Completed { response } => {
                        this.observation.observe_completed(&response.usage)
                    }
                    LlmEvent::MessageDelta {
                        usage: Some(usage), ..
                    } => this.observation.observe(usage),
                    _ => {}
                }
                matches!(event, LlmEvent::Completed { .. })
            }
            Poll::Ready(None | Some(Err(_))) => true,
            Poll::Pending => false,
        };
        if terminal {
            // Preserve provider usage AND its original terminal error/EOF.
            // The driver observes the latch only after handing retained usage
            // to the existing cost owner; replacing this item loses evidence.
            let _ = this.observation.finish();
        }
        polled
    }
}

impl ConversationOrchestrator {
    pub(crate) async fn prepare_output_session(
        &self,
        session: SessionId,
    ) -> Result<Option<OutputTurn>, OrchestratorError> {
        let Some(provider) = &self.model_runtime.output_scopes else {
            return Ok(None);
        };
        self.check_output_accounting()?;
        let scope = provider
            .ensure_current(session, MessageId::new(), self.config.token_budget)
            .await
            .map_err(|error| {
                OrchestratorError::Internal(format!("output session preparation failed: {error}"))
            })?;
        if scope.session_id() != session {
            return Err(OrchestratorError::Internal(
                "prepared output session identity mismatch".into(),
            ));
        }
        Ok(Some(OutputTurn {
            scope,
            failed: self.model_runtime.output_accounting_failed.clone(),
        }))
    }

    pub(crate) fn install_output_session(&self, prepared: Option<OutputTurn>) {
        *self
            .model_runtime
            .output_turn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = prepared;
    }

    pub(crate) fn check_output_accounting(&self) -> Result<(), OrchestratorError> {
        if self
            .model_runtime
            .output_accounting_failed
            .load(Ordering::Acquire)
        {
            Err(OrchestratorError::Internal(
                "output accounting is unavailable".into(),
            ))
        } else {
            Ok(())
        }
    }
    pub(crate) async fn begin_output_turn(
        &self,
        generation: MessageId,
    ) -> Result<(), OrchestratorError> {
        let Some(provider) = &self.model_runtime.output_scopes else {
            return Ok(());
        };
        // Conservative instance-wide latch: a custom provider must not regain
        // dispatch authority merely by rotating turns after a failed write.
        if self
            .model_runtime
            .output_accounting_failed
            .load(Ordering::Acquire)
        {
            return Err(OrchestratorError::Internal(
                "output accounting is unavailable".into(),
            ));
        }
        let session = self.session.lock().await.session_id;
        let scope = provider
            .begin_turn(session, generation, self.config.token_budget)
            .await
            .map_err(|error| {
                OrchestratorError::Internal(format!("output turn unavailable: {error}"))
            })?;
        if scope.session_id() != session || scope.generation_id() != generation {
            return Err(OrchestratorError::Internal(
                "output turn identity mismatch".into(),
            ));
        }
        *self
            .model_runtime
            .output_turn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(OutputTurn {
            scope,
            failed: self.model_runtime.output_accounting_failed.clone(),
        });
        Ok(())
    }

    pub(crate) async fn capture_main_output(
        &self,
    ) -> Result<Option<MainOutputObservation>, OrchestratorError> {
        let Some(provider) = &self.model_runtime.output_scopes else {
            return Ok(None);
        };
        let session = self.session.lock().await.session_id;
        let turn = self
            .model_runtime
            .output_turn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .ok_or_else(|| OrchestratorError::Internal("output turn not initialized".into()))?;
        let current = provider.capture(session).map_err(|error| {
            OrchestratorError::Internal(format!("output scope unavailable: {error}"))
        })?;
        if turn.scope.session_id() != session
            || !turn.scope.shares_account(&current)
            || turn.failed.load(Ordering::Acquire)
        {
            return Err(OrchestratorError::Internal(
                "output scope changed or failed".into(),
            ));
        }
        Ok(Some(MainOutputObservation {
            turn,
            event: MessageId::new(),
            visible: 0,
            reasoning: 0,
            observed: false,
            finished: false,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{
        mock_message_response, noop_hook_executor, MockApiClient, MockOutputStream,
        NoOpPermissionGate, StaticMemoryProvider,
    };
    use futures::StreamExt;
    use platform_api::{BudgetError, WorkflowOutputAccount, WorkflowOutputScopes};
    use std::collections::HashMap;
    use std::sync::Mutex;

    struct Account {
        session: SessionId,
        generation: MessageId,
        events: Mutex<HashMap<WorkflowOutputEventId, u64>>,
    }
    impl WorkflowOutputAccount for Account {
        fn session_id(&self) -> SessionId {
            self.session
        }
        fn generation_id(&self) -> MessageId {
            self.generation
        }
        fn spent(&self) -> u64 {
            self.events.lock().unwrap().values().sum()
        }
        fn record_legacy(
            &self,
            event: WorkflowOutputEventId,
            tokens: u64,
        ) -> Result<(), BudgetError> {
            let mut events = self.events.lock().unwrap();
            if let Some(old) = events.get(&event) {
                if *old != tokens {
                    return Err(BudgetError::Internal("conflict".into()));
                }
            } else {
                events.insert(event, tokens);
            }
            Ok(())
        }
    }
    #[derive(Default)]
    struct Scopes(Mutex<HashMap<SessionId, WorkflowOutputScope>>, AtomicBool);
    #[async_trait]
    impl WorkflowOutputScopes for Scopes {
        async fn ensure_current(
            &self,
            session: SessionId,
            generation: MessageId,
            max: Option<u64>,
        ) -> Result<WorkflowOutputScope, BudgetError> {
            if self.1.load(Ordering::Acquire) {
                return Err(BudgetError::Internal("scope preparation rejected".into()));
            }
            if let Some(scope) = self.0.lock().unwrap().get(&session).cloned() {
                return Ok(scope);
            }
            self.begin_turn(session, generation, max).await
        }
        async fn begin_turn(
            &self,
            session: SessionId,
            generation: MessageId,
            _: Option<u64>,
        ) -> Result<WorkflowOutputScope, BudgetError> {
            let scope = WorkflowOutputScope::new(Arc::new(Account {
                session,
                generation,
                events: Mutex::new(HashMap::new()),
            }));
            self.0.lock().unwrap().insert(session, scope.clone());
            Ok(scope)
        }
        fn capture(&self, session: SessionId) -> Result<WorkflowOutputScope, BudgetError> {
            self.0
                .lock()
                .unwrap()
                .get(&session)
                .cloned()
                .ok_or_else(|| BudgetError::Internal("missing scope".into()))
        }
    }
    fn orch(responses: Vec<llm_client::LlmResponse>) -> ConversationOrchestrator {
        ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(responses)),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        )
    }
    fn usage(visible: u64, reasoning: u64) -> llm_client::Usage {
        llm_client::Usage {
            billable_tokens: llm_client::TokenUsage {
                output: visible,
                reasoning_output: reasoning,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn main_output_clear_resume_preserves_existing_scope_and_failed_prepare_is_inert() {
        let scopes = Arc::new(Scopes::default());
        let orch = orch(vec![]).with_workflow_output_scopes(scopes.clone());
        orch.begin_output_turn(MessageId::new()).await.unwrap();
        let old_session = orch.session.lock().await.session_id;
        let old = scopes.capture(old_session).unwrap();
        old.record_legacy(WorkflowOutputEventId::MainResponse(MessageId::new()), 13)
            .unwrap();
        platform_api::OrchestratorHandle::clear_session(&orch)
            .await
            .unwrap();
        let cleared = orch.session.lock().await.session_id;
        assert_ne!(old_session, cleared);
        assert!(orch.capture_main_output().await.unwrap().is_some());
        assert_eq!(scopes.capture(cleared).unwrap().spent(), 0);
        platform_api::OrchestratorHandle::resume_session(
            &orch,
            old_session,
            vec![],
            None,
            None,
            Default::default(),
        )
        .await
        .unwrap();
        let resumed = scopes.capture(old_session).unwrap();
        assert!(resumed.shares_account(&old));
        assert_eq!(resumed.spent(), 13);
        assert!(orch.capture_main_output().await.unwrap().is_some());

        let marker = ConversationMessage::user(MessageId::new(), "preserve history".to_string());
        orch.session.lock().await.history.push(marker.clone());
        scopes.1.store(true, Ordering::Release);
        assert!(platform_api::OrchestratorHandle::clear_session(&orch)
            .await
            .is_err());
        assert!(platform_api::OrchestratorHandle::resume_session(
            &orch,
            cleared,
            vec![],
            None,
            None,
            Default::default()
        )
        .await
        .is_err());
        let session = orch.session.lock().await;
        assert_eq!(session.session_id, old_session);
        assert_eq!(session.history.last().unwrap().id(), marker.id());
        drop(session);
        assert!(scopes.capture(old_session).unwrap().shares_account(&old));
        assert!(orch.capture_main_output().await.unwrap().is_some());
    }

    #[tokio::test]
    async fn main_output_rejected_completed_preserves_final_provider_usage() {
        struct Reject;
        impl WorkflowOutputAccount for Reject {
            fn session_id(&self) -> SessionId {
                unreachable!()
            }
            fn generation_id(&self) -> MessageId {
                unreachable!()
            }
            fn spent(&self) -> u64 {
                0
            }
            fn record_legacy(&self, _: WorkflowOutputEventId, _: u64) -> Result<(), BudgetError> {
                Err(BudgetError::Internal("storage rejected output".into()))
            }
        }
        let failed = Arc::new(AtomicBool::new(false));
        let observation = MainOutputObservation {
            turn: OutputTurn {
                scope: WorkflowOutputScope::new(Arc::new(Reject)),
                failed: failed.clone(),
            },
            event: MessageId::new(),
            visible: 0,
            reasoning: 0,
            observed: false,
            finished: false,
        };
        let mut response = mock_message_response(vec![], Some("end_turn"));
        response.usage = usage(40, 60);
        let expected = response.usage.clone();
        let stream = futures::stream::iter(vec![
            Ok(llm_client::LlmEvent::Completed {
                response: Box::new(response),
            }),
            Err(llm_client::LlmError::Overloaded { repeated: false }),
        ])
        .boxed();
        let mut wrapped = account_stream(stream, Some(observation));
        let Some(Ok(llm_client::LlmEvent::Completed { response })) = wrapped.next().await else {
            panic!("output failure replaced final paid usage");
        };
        assert_eq!(response.usage, expected);
        assert!(failed.load(Ordering::Acquire));
        assert!(matches!(
            wrapped.next().await,
            Some(Err(llm_client::LlmError::Overloaded { repeated: false }))
        ));
        assert!(wrapped.next().await.is_none());
    }

    #[tokio::test]
    async fn main_output_nonstream_turn_records_disjoint_usage_once() {
        let scopes = Arc::new(Scopes::default());
        let mut response = mock_message_response(
            vec![llm_client::ContentBlock::Text {
                text: "done".into(),
                cache_control: None,
            }],
            Some("end_turn"),
        );
        response.usage = usage(40, 60);
        let orch = orch(vec![response]).with_workflow_output_scopes(scopes.clone());
        orch.run_turn("hello").await.unwrap();
        let session = orch.session.lock().await.session_id;
        assert_eq!(scopes.capture(session).unwrap().spent(), 100);
        assert_eq!(orch.output_token_pool().load(Ordering::Relaxed), 0);
        assert_eq!(
            orch.compaction_runtime
                .last_response_output_tokens
                .load(Ordering::Relaxed),
            40
        );
    }

    #[tokio::test]
    async fn main_output_streaming_turn_participates_without_legacy_double_count() {
        use crate::test_support::{
            content_block_start_text, content_block_stop, message_start, message_stop, text_delta,
            MockStreamingApiClient,
        };
        let scopes = Arc::new(Scopes::default());
        let stream = Arc::new(MockStreamingApiClient::with_turns(vec![vec![
            message_start("m", "claude-opus-4-7"),
            content_block_start_text(0),
            text_delta(0, "done"),
            content_block_stop(0),
            llm_client::LlmEvent::MessageDelta {
                delta: llm_client::MessageDeltaPayload {
                    stop_reason: Some("end_turn".into()),
                    stop_details: None,
                },
                usage: Some(usage(40, 60)),
            },
            message_stop(),
        ]]));
        let orch = ConversationOrchestrator::new_with_streaming(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            stream,
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        )
        .with_workflow_output_scopes(scopes.clone());
        orch.run_turn_streaming("hello").await.unwrap();
        let session = orch.session.lock().await.session_id;
        assert_eq!(scopes.capture(session).unwrap().spent(), 100);
        assert_eq!(orch.output_token_pool().load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn main_output_metadata_only_completion_cannot_erase_retained_output() {
        for explicit_zero in [false, true] {
            let scopes = Arc::new(Scopes::default());
            let orch = orch(vec![]).with_workflow_output_scopes(scopes.clone());
            orch.begin_output_turn(MessageId::new()).await.unwrap();
            let scope = scopes
                .capture(orch.session.lock().await.session_id)
                .unwrap();
            let mut observation = orch.capture_main_output().await.unwrap().unwrap();
            observation.observe(&usage(4, 6));
            let mut completed = llm_client::Usage {
                speed: Some("fast".into()),
                ..Default::default()
            };
            if explicit_zero {
                completed.provider_metadata =
                    serde_json::json!({"input_tokens": 0, "output_tokens": 0});
            }
            observation.observe_completed(&completed);
            observation.finish().unwrap();
            assert_eq!(scope.spent(), if explicit_zero { 0 } else { 10 });
        }
    }

    #[tokio::test]
    async fn main_output_captured_a_survives_b_and_duplicate_finish() {
        let scopes = Arc::new(Scopes::default());
        let orch = orch(vec![]).with_workflow_output_scopes(scopes.clone());
        orch.begin_output_turn(MessageId::new()).await.unwrap();
        let a = scopes
            .capture(orch.session.lock().await.session_id)
            .unwrap();
        let mut observation = orch.capture_main_output().await.unwrap().unwrap();
        orch.session.lock().await.session_id = SessionId::new();
        orch.begin_output_turn(MessageId::new()).await.unwrap();
        let b = scopes
            .capture(orch.session.lock().await.session_id)
            .unwrap();
        observation.observe(&usage(4, 6));
        observation.finish().unwrap();
        observation.finish().unwrap();
        drop(observation);
        assert_eq!(a.spent(), 10);
        assert_eq!(b.spent(), 0);
    }

    #[tokio::test]
    async fn main_output_missing_capture_and_overflow_fail_closed_across_turns() {
        let orch = orch(vec![]).with_workflow_output_scopes(Arc::new(Scopes::default()));
        assert!(orch.capture_main_output().await.is_err());
        orch.begin_output_turn(MessageId::new()).await.unwrap();
        let mut observation = orch.capture_main_output().await.unwrap().unwrap();
        observation.observe(&usage(u64::MAX, 1));
        assert!(observation.finish().is_err());
        assert!(orch.capture_main_output().await.is_err());
        assert!(orch.begin_output_turn(MessageId::new()).await.is_err());
    }

    #[tokio::test]
    async fn main_output_stream_partial_drop_retains_only_known_usage() {
        let scopes = Arc::new(Scopes::default());
        let orch = orch(vec![]).with_workflow_output_scopes(scopes.clone());
        orch.begin_output_turn(MessageId::new()).await.unwrap();
        let scope = scopes
            .capture(orch.session.lock().await.session_id)
            .unwrap();
        let event = llm_client::LlmEvent::MessageDelta {
            delta: llm_client::MessageDeltaPayload {
                stop_reason: None,
                stop_details: None,
            },
            usage: Some(usage(4, 6)),
        };
        let stream = futures::stream::iter(vec![Ok(event)])
            .chain(futures::stream::pending())
            .boxed();
        let mut wrapped = account_stream(stream, orch.capture_main_output().await.unwrap());
        wrapped.next().await.unwrap().unwrap();
        assert_eq!(scope.spent(), 0);
        drop(wrapped);
        assert_eq!(scope.spent(), 10);
    }

    #[tokio::test]
    async fn main_output_stream_completed_retains_final_cumulative_buckets() {
        let scopes = Arc::new(Scopes::default());
        let orch = orch(vec![]).with_workflow_output_scopes(scopes.clone());
        orch.begin_output_turn(MessageId::new()).await.unwrap();
        let scope = scopes
            .capture(orch.session.lock().await.session_id)
            .unwrap();
        let mut response = mock_message_response(vec![], Some("end_turn"));
        response.usage = usage(40, 60);
        let stream = futures::stream::iter(vec![
            Ok(llm_client::LlmEvent::MessageDelta {
                delta: llm_client::MessageDeltaPayload {
                    stop_reason: Some("end_turn".into()),
                    stop_details: None,
                },
                // The final provider snapshot reclassifies provisional visible output.
                usage: Some(usage(100, 0)),
            }),
            Ok(llm_client::LlmEvent::MessageStop),
            Ok(llm_client::LlmEvent::Completed {
                response: Box::new(response),
            }),
        ])
        .boxed();
        let mut wrapped = account_stream(stream, orch.capture_main_output().await.unwrap());
        while wrapped.next().await.is_some() {}
        drop(wrapped);
        assert_eq!(scope.spent(), 100);
    }
}
