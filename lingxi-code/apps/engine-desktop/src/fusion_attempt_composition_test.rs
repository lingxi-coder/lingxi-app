//! Offline checks of the production composition callpoints, not a parallel
//! test-only registrar. The durable fixture uses the real session coordinator.
use super::*;
use cost::CostHydrator;
use platform_api::subagent_spawn::{SubagentInheritance, SubagentSpawner};
use platform_api::tool_invoker::{SubagentInvocationContext, ToolInvoker, ToolInvokerError};
use platform_api::{FusionExecutor, ModelAttemptBillingMode};

const MODELS: [&str; 2] = ["claude-sonnet-5", "claude-opus-4-7"];

pub(super) struct RetirementProbe;

#[async_trait]
impl llm_client::ModelAttemptHooks for RetirementProbe {
    async fn begin(
        &self,
        _: &platform_api::ModelAttemptContext,
        _: &llm_client::LlmRequest,
        _: &llm_client::PreparedLlmCall,
    ) -> Result<Box<dyn llm_client::ModelAttemptLease>, llm_client::LlmError> {
        panic!("shutdown test must not dispatch")
    }
}

struct Offline;
impl llm_client::Transport for Offline {
    fn execute<'a>(
        &'a self,
        _: &'a llm_client::ProviderRequest,
    ) -> llm_client::transport::BoxFuture<
        'a,
        Result<llm_client::ProviderResponse, llm_client::LlmError>,
    > {
        Box::pin(async { panic!("composition tests must not send provider traffic") })
    }
    fn open_stream<'a>(
        &'a self,
        _: &'a llm_client::ProviderRequest,
    ) -> llm_client::transport::BoxFuture<
        'a,
        Result<llm_client::transport::StreamingResponse, llm_client::LlmError>,
    > {
        Box::pin(async { panic!("composition tests must not stream provider traffic") })
    }
}
#[async_trait]
impl SubagentSpawner for Offline {
    async fn spawn(
        &self,
        _: platform_api::subagent_spawn::SubagentSpawnRequest,
        _: SubagentInheritance,
    ) -> Result<
        platform_api::subagent_spawn::SubagentResult,
        platform_api::subagent_spawn::SubagentSpawnError,
    > {
        panic!("whole-panel admission must precede spawning")
    }
}
#[async_trait]
impl sidequery::SideQueryClient for Offline {
    async fn query(
        &self,
        _: sidequery::SideQueryRequest,
    ) -> Result<sidequery::SideQueryResponse, sidequery::SideQueryError> {
        panic!("composition tests must not issue side queries")
    }
}
#[async_trait]
impl ToolInvoker for Offline {
    async fn invoke(
        &self,
        _: &str,
        _: serde_json::Value,
        _: SubagentInvocationContext,
    ) -> Result<serde_json::Value, ToolInvokerError> {
        panic!("composition tests must not invoke tools")
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

fn service() -> Arc<llm_client::ApiService> {
    let profile = llm_client::ProviderProfile {
        provider_id: llm_client::ProviderId::AnthropicFirstParty,
        profile_name: "anthropic".into(),
        base_url: "https://unused.invalid".into(),
        protocol: llm_client::ProtocolFamily::AnthropicMessages,
        auth: llm_client::AuthStrategy::None,
        credential: llm_client::CredentialConfig::None,
        models: MODELS
            .into_iter()
            .map(|model| llm_client::ModelProfile {
                display_model: model.into(),
                request_model: model.into(),
                billing_model: model.into(),
                aliases: vec![],
                description: None,
                metadata: Default::default(),
                capabilities: llm_client::Capabilities {
                    streaming: true,
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
    Arc::new(llm_client::ApiService::new(
        Arc::new(
            llm_client::DefaultLlmClient::from_config(llm_client::ClientConfig {
                providers: vec![profile],
            })
            .unwrap(),
        ),
        Arc::new(Offline),
        Default::default(),
        Default::default(),
        "test",
        None,
        None,
    ))
}

fn executor(
    cfg: &DesktopConfig,
    host: Arc<fusion_attempts::DesktopFusionAttempts>,
    pricing: Arc<cost::PricingCatalog>,
) -> Arc<dyn FusionExecutor> {
    let catalog = MODELS
        .into_iter()
        .map(|model| fusion::CatalogModel {
            profile: "anthropic".into(),
            model: model.into(),
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
    desktop_fusion_executor(
        Arc::new(Offline),
        Arc::new(Offline),
        cfg,
        host,
        Arc::new(catalog),
        Arc::new(telemetry::AnalyticsBus::new()),
        pricing,
    )
}

fn submission(
    session: protocol::SessionId,
    budget: Arc<cost::BudgetEnforcer>,
) -> platform_api::FusionSubmission {
    let request = platform_api::FusionRequest {
        schema_version: 1,
        origin: platform_api::FusionOrigin::Slash,
        prompt: "Review offline".into(),
        preset: platform_api::FusionPreset::Quality,
        models: Some(
            MODELS
                .into_iter()
                .map(|model| platform_api::FusionModelRef {
                    profile: Some("anthropic".into()),
                    model: model.into(),
                })
                .collect(),
        ),
        dimensions: vec!["correctness".into()],
        partial_ok: true,
        max_panel: None,
        cross_provider: false,
        parent_profile: "anthropic".into(),
        parent_model: MODELS[0].into(),
        conversation_id: Some(session.to_string()),
        workflow_run_id: None,
    };
    platform_api::FusionSubmission::new(
        request,
        platform_api::FusionInheritance::new(
            SubagentInheritance {
                tool_invoker: Arc::new(Offline),
                budget,
            },
            tokio_util::sync::CancellationToken::new(),
        ),
        platform_api::FusionRunIdentity::new(
            platform_api::FusionRunId::generated(),
            Some(session),
            platform_api::FusionOrigin::Slash,
            None,
        ),
    )
    .unwrap()
}

fn budget(tracker: Arc<cost::CostTracker>) -> Arc<cost::BudgetEnforcer> {
    Arc::new(cost::BudgetEnforcer::new(
        cost::BudgetConfig {
            max_session_nano_usd: None,
            max_turn_nano_usd: None,
            max_turn_tokens: None,
            warning_thresholds: vec![],
            on_exceed: cost::BudgetExceedPolicy::Halt,
        },
        tracker,
    ))
}

#[tokio::test]
async fn desktop_fusion_composition_durable_uses_same_host_for_wire_and_prepare() {
    let (tmp, mut cfg) = tests::test_config(true);
    cfg.flag_settings = Some(
        serde_json::from_value(serde_json::json!({
            "fusion": {"enabled": true}
        }))
        .unwrap(),
    );
    let session = protocol::SessionId::new();
    let lease = platform_api::live_sessions::LiveSessionDir::at_live(tmp.path().join("sessions"))
        .claim_session_id(&session.to_string(), std::process::id())
        .unwrap()
        .into_shared();
    let coordinator =
        session_state::SessionStateCoordinator::open(tmp.path(), session, lease).unwrap();
    let worker = coordinator.start().await.unwrap();
    let pricing = Arc::new(cost::PricingCatalog::builtin_reference());
    let (tx, _) = tokio::sync::mpsc::channel(1);
    let tracker = Arc::new(
        cost::CostTracker::new(session, pricing.clone(), tx)
            .try_with_durable_persistence(
                coordinator.hydrate(session).await.unwrap(),
                coordinator.clone(),
                coordinator.writer_lease(),
                coordinator.durability_gate(),
            )
            .unwrap(),
    );
    let budget = budget(tracker.clone());
    let outputs = budget.workflow_output_scopes();
    let scope = outputs
        .ensure_current(session, protocol::MessageId::new(), Some(100_000))
        .await
        .unwrap();
    let service = service();
    let host = desktop_fusion_attempts(
        service.clone(),
        budget.clone(),
        tracker.clone(),
        pricing.clone(),
        outputs,
    );
    assert_eq!(
        Arc::strong_count(&host),
        2,
        "factory must install this exact host in ApiService"
    );
    let mut rollback = cfg.clone();
    rollback.flag_settings = Some(
        serde_json::from_value(serde_json::json!({
            "fusion": {"enabled": true, "workflowConcurrency": 1}
        }))
        .unwrap(),
    );
    assert_eq!(
        executor(&rollback, host.clone(), pricing.clone()).workflow_batch_concurrency(),
        1
    );
    let executor = executor(&cfg, host.clone(), pricing);
    assert_eq!(executor.workflow_batch_concurrency(), 2);
    assert_eq!(
        Arc::strong_count(&host),
        3,
        "executor must retain the same host, not a second registry"
    );
    let prepared = executor
        .clone()
        .prepare(submission(session, budget.clone()))
        .unwrap();
    assert_eq!(
        prepared.control().billing_mode(),
        ModelAttemptBillingMode::MeteredAttempts
    );
    assert_eq!(scope.spent(), 0);
    assert_eq!(
        budget.active_reservation_nano_usd().await,
        0,
        "prepare cannot acquire a hold"
    );
    let outcome = prepared
        .activate(platform_api::FusionActivation::now(), None)
        .await;
    assert!(matches!(
        outcome.result,
        Err(platform_api::FusionError::PanelAdmissionRejected(_))
    ));
    assert_eq!(outcome.facts.allocated_panels, Some(0));
    assert_eq!(outcome.facts.attempts, Some(0));
    assert_eq!(
        outcome.facts.attempt_settlement,
        Some(platform_api::FusionAttemptSettlementStatus::Settled)
    );
    tracker.drain_owned_settlements().await.unwrap();
    drop(executor);
    // Registration finalizers retain run authority, never the hook registry.
    assert_eq!(Arc::strong_count(&host), 2);
    drop(service);
    assert_eq!(
        Arc::strong_count(&host),
        1,
        "host must not retain ApiService strongly"
    );
    coordinator.close_and_drain().await.unwrap();
    worker.await.unwrap();
}

/// An ephemeral-transcript host is not an unmetered host. It gets a real
/// ledger under a temporary home, so every attempt is registered and billed
/// exactly as for a persistent host; what it does not get is a transcript on
/// disk. Before the disposable ledger existed this host had no registrar at
/// all, which both clamped it to one workflow batch and billed it as an
/// unverifiable aggregate.
#[tokio::test]
async fn desktop_fusion_composition_ephemeral_still_meters_and_requires_pool_admission() {
    let (tmp, mut cfg) = tests::test_config(true);
    // No transcript, but the ledger below still lands under a real directory:
    // production roots it at a temporary `LINGXI_HOME` removed at shutdown.
    cfg.session_persistence = false;
    cfg.flag_settings = Some(
        serde_json::from_value(serde_json::json!({
            "fusion": {"enabled": true, "workflowConcurrency": 2}
        }))
        .unwrap(),
    );
    let session = protocol::SessionId::new();
    let lease = platform_api::live_sessions::LiveSessionDir::at_live(tmp.path().join("sessions"))
        .claim_session_id(&session.to_string(), std::process::id())
        .unwrap()
        .into_shared();
    let coordinator =
        session_state::SessionStateCoordinator::open(tmp.path(), session, lease).unwrap();
    let _worker = coordinator.start().await.unwrap();
    let pricing = Arc::new(cost::PricingCatalog::builtin_reference());
    let (tx, _) = tokio::sync::mpsc::channel(1);
    let tracker = Arc::new(
        cost::CostTracker::new(session, pricing.clone(), tx)
            .try_with_durable_persistence(
                coordinator.hydrate(session).await.unwrap(),
                coordinator.clone(),
                coordinator.writer_lease(),
                coordinator.durability_gate(),
            )
            .unwrap(),
    );
    let budget = budget(tracker.clone());
    let outputs = budget.workflow_output_scopes();
    outputs
        .ensure_current(session, protocol::MessageId::new(), Some(100_000))
        .await
        .unwrap();
    // The host keeps only a weak handle to the service, so this binding is
    // what keeps the attempt registrar reachable for the whole test.
    let service = service();
    let host = desktop_fusion_attempts(
        service.clone(),
        budget.clone(),
        tracker,
        pricing.clone(),
        outputs,
    );
    let executor = executor(&cfg, host, pricing);
    // The clamp to one batch belonged to the unregistered path, not to the
    // ephemeral host, so this now reads the configured value.
    assert_eq!(executor.workflow_batch_concurrency(), 2);
    let prepared = executor
        .prepare(submission(session, budget.clone()))
        .unwrap();
    assert_eq!(
        prepared.control().billing_mode(),
        ModelAttemptBillingMode::MeteredAttempts
    );
    // Offline does not implement the admitted-pool seam. Activation must fail
    // closed before reaching its panicking legacy spawn/side-query methods.
    let outcome = prepared
        .activate(platform_api::FusionActivation::now(), None)
        .await;
    assert!(matches!(
        outcome.result,
        Err(platform_api::FusionError::PanelAdmissionRejected(_))
    ));
    assert_eq!(outcome.facts.allocated_panels, Some(0));
    assert_eq!(outcome.facts.attempts, Some(0));
    assert_eq!(budget.active_reservation_nano_usd().await, 0);
}

#[tokio::test]
async fn desktop_fusion_composition_refuses_ephemeral_tracker_even_with_output_scope() {
    let (_tmp, cfg) = tests::test_config(true);
    let session = protocol::SessionId::new();
    let pricing = Arc::new(cost::PricingCatalog::builtin_reference());
    let (tx, _) = tokio::sync::mpsc::channel(1);
    let tracker = Arc::new(cost::CostTracker::new(session, pricing.clone(), tx));
    let budget = budget(tracker.clone());
    let outputs = budget.workflow_output_scopes();
    outputs
        .ensure_current(session, protocol::MessageId::new(), None)
        .await
        .unwrap();
    let service = service();
    let host = desktop_fusion_attempts(
        service.clone(),
        budget.clone(),
        tracker,
        pricing.clone(),
        outputs,
    );
    let error = executor(&cfg, host, pricing)
        .prepare(submission(session, budget.clone()))
        .err()
        .expect("ephemeral authority must not register physical attempts");
    assert!(
        error.to_string().contains("originating durable session"),
        "{error}"
    );
    assert_eq!(budget.active_reservation_nano_usd().await, 0);
}
