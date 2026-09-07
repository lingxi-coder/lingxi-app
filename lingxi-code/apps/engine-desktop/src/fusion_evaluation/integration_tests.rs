//! Actual app evaluation -> real pool/runner -> ProviderApiAdapter/ApiService ->
//! production registration/quotes -> WAL coordinator. Only provider IO is fake.
//! The receipt gate delays a real committed ACK; it never manufactures one.
use super::*;
use async_trait::async_trait;
use cost::{CostHydrator, CostPersistence};
use platform_api::subagent_spawn::SubagentInheritance;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, Weak};

const MODELS: [&str; 3] = ["gpt-4o", "gpt-4.1", "gpt-4o-mini"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WireStage {
    Child,
    Analyst,
    Synthesis,
}

fn answer() -> Value {
    json!({"output_id":"answer","reported_format_valid":true,"fact_ids":[],
        "citations":[],"proposed_actions":[],"reported_truncated":false})
}

#[derive(Default)]
struct Wire {
    calls: Mutex<Vec<WireStage>>,
    quotes: Mutex<Vec<u64>>,
    budget: OnceLock<Weak<cost::BudgetEnforcer>>,
    block_child: AtomicBool,
    child_blocked: tokio::sync::Notify,
}
struct Frames {
    frames: VecDeque<llm_client::RawStreamFrame>,
    block: Option<Arc<Wire>>,
}
impl llm_client::transport::FrameStream for Frames {
    fn next_frame(
        &mut self,
    ) -> llm_client::transport::BoxFuture<
        '_,
        Result<Option<llm_client::RawStreamFrame>, llm_client::LlmError>,
    > {
        Box::pin(async move {
            if let Some(frame) = self.frames.pop_front() {
                return Ok(Some(frame));
            }
            if let Some(wire) = self.block.take() {
                wire.child_blocked.notify_one();
                // The real runner's cancellation must destroy this transport
                // stream; no test-side model completion is synthesized.
                std::future::pending::<()>().await;
            }
            Ok(None)
        })
    }
}
struct FakeTransport(Arc<Wire>);
impl FakeTransport {
    async fn capture(&self, stage: WireStage) {
        self.0.calls.lock().unwrap().push(stage);
        let budget = self.0.budget.get().unwrap().upgrade().unwrap();
        let held = budget.active_reservation_nano_usd().await;
        assert!(
            held > 0,
            "actual wire must be preceded by a real monetary hold"
        );
        self.0.quotes.lock().unwrap().push(held);
    }
}
fn user_payload(request: &llm_client::ProviderRequest) -> Value {
    let messages = request.body_json["messages"].as_array().unwrap();
    let message = messages
        .iter()
        .rev()
        .find(|message| message["role"] == "user")
        .unwrap();
    let text = message["content"]
        .as_str()
        .or_else(|| {
            message["content"]
                .as_array()
                .and_then(|blocks| blocks.iter().find_map(|block| block["text"].as_str()))
        })
        .unwrap();
    serde_json::from_str(text).unwrap()
}

/// Classify the actual wire request, independently of HTTP versus SSE.
/// ProviderSideQueryClient may stream either judge stage as well as children.
fn scripted_output(request: &llm_client::ProviderRequest) -> (WireStage, Value) {
    if let Some(tool) = request.body_json["tools"].as_array().and_then(|tools| {
        tools
            .iter()
            .find(|tool| tool["function"]["name"] == "StructuredOutput")
    }) {
        let panel = tool["function"]["parameters"]["properties"]
            .get("schema_version")
            .is_some();
        let output = if panel {
            json!({"schema_version":1,"summary":"offline","candidate_answer":answer().to_string(),
                "claims":[],"evidence":[],"assumptions":[],"risks":[],"unresolved_questions":[]})
        } else {
            answer()
        };
        return (WireStage::Child, output);
    }
    if request
        .body_json
        .get("response_format")
        .is_some_and(Value::is_object)
    {
        let payload = user_payload(request);
        let mut scores = serde_json::Map::new();
        for panel in payload["panels"].as_array().unwrap() {
            let row: serde_json::Map<String, Value> = payload["dimensions"]
                .as_array()
                .unwrap()
                .iter()
                .map(|dimension| (dimension.as_str().unwrap().into(), json!(80)))
                .collect();
            scores.insert(
                panel["panel_id"].as_str().unwrap().into(),
                Value::Object(row),
            );
        }
        // The real evaluation decorator must perform the intervention; the
        // provider fixture does not select Pick or Merge on its behalf.
        return (
            WireStage::Analyst,
            json!({"schema_version":1,"consensus":[],"contradictions":[],
            "unique_insights":[],"coverage_gaps":[],"scores":scores,"confidence":80,
            "recommendation":{"type":"needs_parent","reason":"natural fixture policy"}}),
        );
    }
    assert!(
        user_payload(request).get("analysis").is_some(),
        "expected actual synthesis payload"
    );
    (WireStage::Synthesis, answer())
}
impl llm_client::Transport for FakeTransport {
    fn execute<'a>(
        &'a self,
        request: &'a llm_client::ProviderRequest,
    ) -> llm_client::transport::BoxFuture<
        'a,
        Result<llm_client::ProviderResponse, llm_client::LlmError>,
    > {
        Box::pin(async move {
            let (stage, output) = scripted_output(request);
            self.capture(stage).await;
            let message = if stage == WireStage::Child {
                json!({"role":"assistant","tool_calls":[{"id":"structured","type":"function",
                    "function":{"name":"StructuredOutput","arguments":output.to_string()}}]})
            } else {
                json!({"role":"assistant","content":output.to_string()})
            };
            Ok(llm_client::ProviderResponse {
                status: 200,
                headers: Default::default(),
                request_id: Some("offline".into()),
                body_json: json!({"id":"offline","model":request.body_json["model"],"choices":[{"index":0,
                    "message":message,"finish_reason":if stage==WireStage::Child {"tool_calls"}else{"stop"}}],
                    "usage":{"prompt_tokens":5,"completion_tokens":3,"total_tokens":8}}),
            })
        })
    }
    fn open_stream<'a>(
        &'a self,
        request: &'a llm_client::ProviderRequest,
    ) -> llm_client::transport::BoxFuture<
        'a,
        Result<llm_client::transport::StreamingResponse, llm_client::LlmError>,
    > {
        Box::pin(async move {
            let (stage, output) = scripted_output(request);
            self.capture(stage).await;
            let blocked = stage == WireStage::Child && self.0.block_child.load(Ordering::SeqCst);
            let delta = if blocked {
                json!({"role":"assistant","content":"partial"})
            } else if stage == WireStage::Child {
                json!({"role":"assistant","tool_calls":[{"index":0,"id":"structured","type":"function",
                    "function":{"name":"StructuredOutput","arguments":output.to_string()}}]})
            } else {
                json!({"role":"assistant","content":output.to_string()})
            };
            let chunk = json!({"id":"offline-stream","model":request.body_json["model"],"choices":[{"index":0,"delta":delta,
                "finish_reason":if blocked {Value::Null}else if stage==WireStage::Child {json!("tool_calls")}else{json!("stop")}}],
                "usage":{"prompt_tokens":5,"completion_tokens":3,"total_tokens":8}});
            let mut frames = VecDeque::from([llm_client::RawStreamFrame::new(
                serde_json::to_vec(&chunk).unwrap(),
            )]);
            if !blocked {
                frames.push_back(llm_client::RawStreamFrame::new(b"[DONE]".to_vec()));
            }
            Ok(llm_client::transport::StreamingResponse {
                status: 200,
                headers: Default::default(),
                frames: Box::new(Frames {
                    frames,
                    block: blocked.then(|| self.0.clone()),
                }),
            })
        })
    }
}

struct AckGate {
    hold: AtomicBool,
    committed: tokio::sync::Notify,
    release: tokio::sync::Semaphore,
    pending: AtomicUsize,
}
impl Default for AckGate {
    fn default() -> Self {
        Self {
            hold: AtomicBool::new(false),
            committed: tokio::sync::Notify::new(),
            release: tokio::sync::Semaphore::new(0),
            pending: AtomicUsize::new(0),
        }
    }
}
struct AckPersistence {
    coordinator: Arc<crate::session_state::SessionStateCoordinator>,
    gate: Arc<AckGate>,
}
#[async_trait]
impl CostPersistence for AckPersistence {
    async fn acquire_permit(
        &self,
        session: protocol::SessionId,
    ) -> Result<cost::CostPersistPermit, cost::CostPersistError> {
        self.coordinator.acquire_permit(session).await
    }
    async fn acquire_attempt_permit(
        &self,
        session: protocol::SessionId,
    ) -> Result<cost::AttemptPersistPermit, cost::CostPersistError> {
        let real = self.coordinator.acquire_attempt_permit(session).await?;
        let gate = self.gate.clone();
        Ok(cost::AttemptPersistPermit::new(move |mut request| {
            let receipt = matches!(&request.mutation, cost::AttemptPersistMutation::Receipt(_));
            if !receipt || !gate.hold.load(Ordering::SeqCst) {
                return real.enqueue(request);
            }
            let (real_ack, received) = tokio::sync::oneshot::channel();
            let original = std::mem::replace(&mut request.ack, real_ack);
            real.enqueue(request)?;
            gate.pending.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                let result = received
                    .await
                    .expect("real coordinator must acknowledge accepted receipt");
                assert!(result.is_ok(), "gate holds a real successful WAL ACK");
                gate.committed.notify_one();
                let permit = gate.release.acquire().await.unwrap();
                permit.forget();
                let _ = original.send(result);
                gate.pending.fetch_sub(1, Ordering::SeqCst);
            });
            Ok(())
        }))
    }
}

struct NoExternal;
#[async_trait]
impl platform_api::tool_invoker::ToolInvoker for NoExternal {
    async fn invoke(
        &self,
        _: &str,
        _: Value,
        _: platform_api::tool_invoker::SubagentInvocationContext,
    ) -> Result<Value, platform_api::tool_invoker::ToolInvokerError> {
        panic!("fixture must not invoke external tools")
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

struct Harness {
    directory: tempfile::TempDir,
    setup: Arc<EvaluationSetup>,
    host: Arc<HostInputs>,
    service: Arc<llm_client::ApiService>,
    wire: Arc<Wire>,
    gate: Arc<AckGate>,
    tracker: Arc<cost::CostTracker>,
    budget: Arc<cost::BudgetEnforcer>,
    coordinator: Arc<crate::session_state::SessionStateCoordinator>,
    recorder: Arc<crate::fusion_recorder::DesktopFusionRecorder>,
    worker: tokio::task::JoinHandle<()>,
    pool: Arc<agent::StateMachinePool>,
}
impl Harness {
    async fn new(calls: u32, money: u64) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let session = protocol::SessionId::new();
        let lease =
            platform_api::live_sessions::LiveSessionDir::at_live(directory.path().join("sessions"))
                .claim_session_id(&session.to_string(), std::process::id())
                .unwrap()
                .into_shared();
        let coordinator =
            crate::session_state::SessionStateCoordinator::open(directory.path(), session, lease)
                .unwrap();
        let worker = coordinator.start().await.unwrap();
        let pricing = Arc::new(cost::PricingCatalog::builtin_reference());
        let gate = Arc::new(AckGate::default());
        let (tx, _) = tokio::sync::mpsc::channel(1);
        let tracker = Arc::new(
            cost::CostTracker::new(session, pricing.clone(), tx)
                .try_with_durable_persistence(
                    coordinator.hydrate(session).await.unwrap(),
                    Arc::new(AckPersistence {
                        coordinator: coordinator.clone(),
                        gate: gate.clone(),
                    }),
                    coordinator.writer_lease(),
                    coordinator.durability_gate(),
                )
                .unwrap(),
        );
        let budget = Arc::new(cost::BudgetEnforcer::new(
            cost::BudgetConfig {
                max_session_nano_usd: Some(money),
                max_turn_nano_usd: None,
                max_turn_tokens: None,
                warning_thresholds: vec![],
                on_exceed: cost::BudgetExceedPolicy::Halt,
            },
            tracker.clone(),
        ));
        let outputs = budget.workflow_output_scopes();
        outputs
            .ensure_current(session, protocol::MessageId::new(), Some(100_000))
            .await
            .unwrap();
        let wire = Arc::new(Wire::default());
        wire.budget.set(Arc::downgrade(&budget)).unwrap();
        let profile = llm_client::ProviderProfile {
            provider_id: llm_client::ProviderId::OpenAI,
            profile_name: "openai".into(),
            base_url: "https://unused.invalid/v1".into(),
            protocol: llm_client::ProtocolFamily::OpenAiChat,
            auth: llm_client::AuthStrategy::None,
            credential: llm_client::CredentialConfig::None,
            models: MODELS
                .iter()
                .map(|model| llm_client::ModelProfile {
                    display_model: (*model).into(),
                    request_model: (*model).into(),
                    billing_model: (*model).into(),
                    aliases: vec![],
                    description: None,
                    metadata: platform_api::ModelMetadata {
                        context_window_tokens: Some(200_000),
                        max_input_tokens: Some(180_000),
                        max_output_tokens: Some(32_000),
                        ..Default::default()
                    },
                    capabilities: llm_client::Capabilities {
                        streaming: true,
                        tools: true,
                        structured_output: true,
                        ..Default::default()
                    },
                })
                .collect(),
            pricing: Default::default(),
            signing: None,
            azure: None,
            supports_websockets: false,
            supports_websocket_compression: false,
            websocket_connect_timeout_ms: None,
            vision_delegate: None,
        };
        let service = Arc::new(llm_client::ApiService::new(
            Arc::new(
                llm_client::DefaultLlmClient::from_config(llm_client::ClientConfig {
                    providers: vec![profile],
                })
                .unwrap(),
            ),
            Arc::new(FakeTransport(wire.clone())),
            Default::default(),
            Default::default(),
            "offline-evaluation",
            None,
            None,
        ));
        service.require_registered_model_attempts();
        let attempts = crate::desktop_fusion_attempts(
            service.clone(),
            budget.clone(),
            tracker.clone(),
            pricing.clone(),
            Some(outputs),
        )
        .unwrap();
        let setup = Arc::new(EvaluationSetup {
            selection: LiveSelection::validate(
                &LiveOptions {
                    paid_opt_in: true,
                    run_count: Some(3),
                    budget_nano_usd: Some(money),
                },
                Some(calls),
                &[0, 2, 4],
            )
            .unwrap(),
            quota: OnceLock::new(),
            host: OnceLock::new(),
        });
        setup
            .install_quota(service.as_ref(), attempts.clone())
            .unwrap();
        let pool = Arc::new(agent::StateMachinePool::new(
            Arc::new(test_harness::mocks::MockRuntimeSpawner::default()),
            3,
        ));
        let spawner = Arc::new(
            agent::PoolSubagentSpawner::new(pool.clone()).with_api_client(Arc::new(
                orchestrator::provider_adapter::ProviderApiAdapter::new(service.clone()),
            )),
        );
        let mut cfg = crate::DesktopConfig::default();
        cfg.cwd = directory.path().join("workspace");
        std::fs::create_dir_all(&cfg.cwd).unwrap();
        cfg.lingxi_home = directory.path().to_path_buf();
        cfg.isolated_credential_storage = true;
        cfg.setting_source_scope = (false, false);
        cfg.flag_settings=Some(serde_json::from_value(json!({"fusion":{"enabled":true,"qualityPanelCount":3,"panelMaxTurns":1,
            "panelMaxOutputTokensPerTurn":512,"panelReservedInputTokensPerTurn":32768,"analystMaxOutputTokens":512,
            "synthesizerMaxOutputTokens":512,"analysisProtocolRetries":0}})).unwrap());
        let catalog = MODELS
            .iter()
            .map(|model| fusion::CatalogModel {
                profile: "openai".into(),
                model: (*model).into(),
                hints: platform_api::FusionModelHints {
                    eligible: true,
                    judge_eligible: true,
                    quality_rank: 90,
                    ..Default::default()
                },
                structured_output: true,
                limits: fusion::ModelLimits {
                    context_window_tokens: Some(200_000),
                    max_input_tokens: Some(180_000),
                    max_output_tokens: Some(32_000),
                },
            })
            .collect::<Vec<_>>();
        let path = session::jsonl::path::session_path(
            directory.path(),
            &cfg.cwd.to_string_lossy(),
            &session.as_uuid().to_string(),
        );
        let fs: Arc<dyn platform_api::FileSystem> = Arc::new(
            platform_posix::fs::PosixFileSystem::new(directory.path().to_path_buf()),
        );
        let durable = Arc::new(session::jsonl::DurableTranscriptWriter::from_pinned(
            coordinator.journal().root().to_path_buf(),
            coordinator.journal().root_identity(),
        ));
        let writer =
            Arc::new(session::jsonl::writer::JsonlWriter::new(path, fs).with_durable_lock(durable));
        writer
            .activate_session_target(session, writer.path().to_path_buf(), cfg.cwd.clone())
            .unwrap();
        let recorder = Arc::new(crate::fusion_recorder::DesktopFusionRecorder::new(
            coordinator.clone(),
            Some(crate::fusion_recorder::FusionTranscriptTarget::new(writer).for_session(session)),
        ));
        setup
            .install_host(HostInputs {
                cfg,
                session,
                parent_model: MODELS[0].into(),
                parent_profile: Some("openai".into()),
                spawner,
                query: Arc::new(sidequery::ProviderSideQueryClient::from_service(
                    service.clone(),
                )),
                attempts,
                catalog: Arc::new(catalog),
                pricing,
                bus: Arc::new(telemetry::AnalyticsBus::new()),
                inheritance: SubagentInheritance {
                    tool_invoker: Arc::new(NoExternal),
                    budget: budget.clone(),
                },
                recorder: Some(recorder.clone()),
            })
            .unwrap();
        let host = setup.host.get().unwrap().clone();
        Self {
            directory,
            setup,
            host,
            service,
            wire,
            gate,
            tracker,
            budget,
            coordinator,
            recorder,
            worker,
            pool,
        }
    }
    async fn case(
        &self,
        mode: ComparisonMode,
        cancel: tokio_util::sync::CancellationToken,
    ) -> CaseReport {
        run_case(self.host.clone(), mode, cancel).await
    }
    async fn close(self) -> tempfile::TempDir {
        assert_eq!(
            self.pool.slot_count().await,
            0,
            "real pool producers must be gone"
        );
        self.tracker.drain_owned_settlements().await.unwrap();
        self.recorder.retry_pending().await;
        assert_eq!(self.budget.active_reservation_nano_usd().await, 0);
        self.service.clear_model_attempt_hooks();
        self.coordinator.close_and_drain().await.unwrap();
        self.worker.await.unwrap();
        self.directory
    }
}

async fn run_case(
    host: Arc<HostInputs>,
    mode: ComparisonMode,
    cancel: tokio_util::sync::CancellationToken,
) -> CaseReport {
    let comparison = evaluation::harness::dry_run()
        .unwrap()
        .comparisons
        .into_iter()
        .find(|comparison| {
            comparison.mode == mode && comparison.completion_policy == CompletionPolicy::WaitAll
        })
        .unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        host.run_case(comparison, &evaluation::fixtures::all_fixtures()[0], cancel),
    )
    .await
    .expect("offline live-host case must terminate")
    .expect("normal registration/prepare")
}

#[tokio::test]
async fn actual_live_host_single_pick_merge_use_registered_wire_and_durable_totals() {
    let harness = Harness::new(16, 1_000_000_000).await;
    let mut total = 0;
    for (mode, expected_calls) in [
        (ComparisonMode::Single, 1),
        (ComparisonMode::PanelPick, 4),
        (ComparisonMode::PanelMerge, 5),
    ] {
        let before = harness.wire.calls.lock().unwrap().len();
        let report = harness
            .case(mode, tokio_util::sync::CancellationToken::new())
            .await;
        assert!(
            report.computation_error.is_none(),
            "{:?}",
            report.computation_error
        );
        assert!(report.format_error.is_none(), "{:?}", report.format_error);
        let result = report.result.unwrap();
        assert_eq!(result.status, platform_api::FusionStatus::Completed);
        assert!(report.saved_run.is_some());
        assert_eq!(
            harness.wire.calls.lock().unwrap().len() - before,
            expected_calls
        );
        {
            let calls = harness.wire.calls.lock().unwrap();
            let count = |stage| {
                calls[before..]
                    .iter()
                    .filter(|actual| **actual == stage)
                    .count()
            };
            let expected = match mode {
                ComparisonMode::Single => (1, 0, 0),
                ComparisonMode::PanelPick => (3, 1, 0),
                ComparisonMode::PanelMerge => (3, 1, 1),
            };
            assert_eq!((count(WireStage::Child), count(WireStage::Analyst), count(WireStage::Synthesis)), expected,
                "stage counts come from actual HTTP/SSE request schemas, not logical comparison labels");
        }
        assert_eq!(
            report.facts.usage.as_ref().unwrap().provider_requests as usize,
            expected_calls
        );
        assert!(matches!(
            report.facts.attempt_settlement,
            Some(platform_api::FusionAttemptSettlementStatus::Settled)
        ));
        if mode != ComparisonMode::Single {
            assert!(matches!(
                report.natural_recommendations.as_slice(),
                [platform_api::FusionRecommendation::NeedsParent { .. }]
            ));
        }
        assert_eq!(
            matches!(result.decision, platform_api::FusionDecision::Merged),
            mode == ComparisonMode::PanelMerge
        );
        total += report.facts.usage.unwrap().realized_nano_usd;
        assert_eq!(
            harness.tracker.snapshot().await.total_nano_usd,
            total,
            "same durable session accumulates every comparison"
        );
    }
    assert_eq!(harness.setup.quota.get().unwrap().claimed(), 10);
    let before = harness.wire.calls.lock().unwrap().len();
    harness.recorder.retry_pending().await;
    assert_eq!(
        harness.wire.calls.lock().unwrap().len(),
        before,
        "local publication replay never calls model transport"
    );
    harness.close().await;
}

#[tokio::test]
async fn live_host_cumulative_quota_rejects_later_case_before_any_extra_wire() {
    let harness = Harness::new(1, 1_000_000_000).await;
    let first = harness
        .case(
            ComparisonMode::Single,
            tokio_util::sync::CancellationToken::new(),
        )
        .await;
    assert!(first.computation_error.is_none());
    let second = harness
        .case(
            ComparisonMode::Single,
            tokio_util::sync::CancellationToken::new(),
        )
        .await;
    assert!(second.computation_error.is_some());
    assert_eq!(harness.wire.calls.lock().unwrap().len(), 1);
    assert_eq!(harness.setup.quota.get().unwrap().claimed(), 1);
    assert_eq!(
        harness.tracker.snapshot().await.total_nano_usd,
        first.facts.usage.unwrap().realized_nano_usd
    );
    harness.close().await;
}

#[tokio::test]
async fn live_host_cumulative_money_is_refused_by_real_budget_before_wire() {
    let calibration = Harness::new(16, 1_000_000_000).await;
    let first = calibration
        .case(
            ComparisonMode::Single,
            tokio_util::sync::CancellationToken::new(),
        )
        .await;
    assert!(first.computation_error.is_none());
    // Observe the real upper-bound hold, do not reconstruct pricing in tests.
    let quote = calibration.wire.quotes.lock().unwrap()[0];
    calibration.close().await;
    let harness = Harness::new(16, quote).await;
    let first = harness
        .case(
            ComparisonMode::Single,
            tokio_util::sync::CancellationToken::new(),
        )
        .await;
    assert!(first.computation_error.is_none());
    let second = harness
        .case(
            ComparisonMode::Single,
            tokio_util::sync::CancellationToken::new(),
        )
        .await;
    assert!(second.computation_error.is_some());
    assert_eq!(harness.wire.calls.lock().unwrap().len(), 1);
    assert!(harness.tracker.snapshot().await.total_nano_usd > 0);
    harness.close().await;
}

#[tokio::test]
async fn cancelled_live_authority_retains_claim_while_real_ack_and_drain_are_pending() {
    let harness = Harness::new(16, 1_000_000_000).await;
    harness.gate.hold.store(true, Ordering::SeqCst);
    harness.wire.block_child.store(true, Ordering::SeqCst);
    let cancel = tokio_util::sync::CancellationToken::new();
    let case = tokio::spawn(run_case(
        harness.host.clone(),
        ComparisonMode::Single,
        cancel.clone(),
    ));
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        harness.wire.child_blocked.notified(),
    )
    .await
    .unwrap();
    cancel.cancel();
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        harness.gate.committed.notified(),
    )
    .await
    .unwrap();
    assert!(
        !case.is_finished(),
        "actual run finalizer must await the retained WAL ACK"
    );
    assert_eq!(harness.gate.pending.load(Ordering::SeqCst), 1);
    let mut settlement_drain = Box::pin(harness.tracker.drain_owned_settlements());
    std::future::poll_fn(|cx| {
        assert!(
            std::future::Future::poll(settlement_drain.as_mut(), cx).is_pending(),
            "the public real settlement drain must wait for the held WAL ACK"
        );
        std::task::Poll::Ready(())
    })
    .await;
    // Composite live-authority exclusion only: this fixture intentionally
    // retains its host/coordinator/lease pins as well as the unacknowledged
    // settlement. Therefore None does NOT isolate the ACK's own retention
    // pin, and does not execute the private 64-session victim-selection scan.
    assert!(harness
        .coordinator
        .durability_gate()
        .retention_gate()
        .try_begin_retirement()
        .unwrap()
        .is_none());
    assert!(platform_api::live_sessions::LiveSessionDir::at_live(
        harness.directory.path().join("sessions")
    )
    .claim_session_id(&harness.host.session.to_string(), std::process::id())
    .is_err());
    assert_eq!(harness.wire.calls.lock().unwrap().len(), 1);
    harness.gate.release.add_permits(1);
    let report = case.await.unwrap();
    settlement_drain.await.unwrap();
    assert_eq!(harness.gate.pending.load(Ordering::SeqCst), 0);
    assert!(report.computation_error.is_some());
    assert_eq!(report.facts.allocated_panels, Some(1));
    assert!(
        report.facts.usage.unwrap().input_tokens >= 5,
        "known provider usage survives cancellation"
    );
    assert_eq!(harness.pool.slot_count().await, 0);
    let session = harness.host.session;
    let directory = harness.close().await;
    let reclaimed =
        platform_api::live_sessions::LiveSessionDir::at_live(directory.path().join("sessions"))
            .claim_session_id(&session.to_string(), std::process::id());
    assert!(
        reclaimed.is_ok(),
        "claim is released only after ACK and full fixture drain"
    );
}
