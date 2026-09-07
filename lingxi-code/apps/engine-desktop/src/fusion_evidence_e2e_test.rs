//! Offline end-to-end evidence regression. Only the native tool body and HTTP
//! transport are fakes; pool, runner, registry, normalization, codecs and Fusion
//! analyst/synthesis orchestration are the production implementations.
use ::fusion as fusion_engine;
use async_trait::async_trait;
use llm_client::{
    BoxFuture, FrameStream, LlmError, ProviderRequest, ProviderResponse, RawStreamFrame,
    StreamingResponse, Transport,
};
use permission::{result::PermissionMetadata, PermissionDecisionReason, PermissionResult};
use platform_api::{
    BudgetEnforcerHandle, BudgetError, BudgetReservationId, FusionActivation, FusionDecision,
    FusionExecutor, FusionInheritance, FusionModelHints, FusionModelRef, FusionOrigin,
    FusionPreset, FusionRequest, FusionRunId, FusionRunIdentity, FusionSubmission, FusionUsage,
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex, OnceLock,
};
use std::time::Duration;
use tool_api::{
    DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolProgressSender,
    ToolStaticContext, ToolUseContext,
};

const MODELS: [&str; 2] = ["claude-sonnet-5", "claude-opus-4-7"];
const INPUT: u64 = 701;
const OUTPUT: u64 = 37;

struct ReadFixture {
    name: &'static str,
    capability: bool,
    permissions: AtomicUsize,
    calls: AtomicUsize,
}

#[async_trait]
impl Tool for ReadFixture {
    fn name(&self) -> &str {
        self.name
    }
    fn evidence_capability(&self) -> Option<platform_api::EvidenceCapability> {
        self.capability
            .then_some(platform_api::EvidenceCapability::Read)
    }
    fn input_schema(&self) -> &Value {
        static SCHEMA: OnceLock<Value> = OnceLock::new();
        SCHEMA.get_or_init(|| json!({"type":"object","properties":{"file_path":{"type":"string"}}}))
    }
    fn is_enabled(&self, _: &ToolStaticContext) -> bool {
        true
    }
    fn max_result_size_chars(&self) -> usize {
        4096
    }
    fn is_concurrency_safe(&self, _: &Value) -> bool {
        true
    }
    fn is_read_only(&self, _: &Value) -> bool {
        true
    }
    async fn check_permissions(&self, _: &Value, _: &ToolUseContext) -> PermissionResult {
        self.permissions.fetch_add(1, Ordering::SeqCst);
        PermissionResult::Allow {
            reason: PermissionDecisionReason::Other {
                reason: "offline read fixture".into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }
    async fn description(&self, _: &Value, _: &DescriptionOptions) -> String {
        "Read offline fixture".into()
    }
    async fn prompt(&self, _: &PromptOptions) -> String {
        "Read offline fixture".into()
    }
    async fn call(
        &self,
        _: Value,
        _: ToolUseContext,
        _: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        assert_eq!(
            self.name, "Read",
            "other declared read-only fixtures must not execute"
        );
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(ToolCallResult::from_data(
            json!({"type":"text","file":{"filePath":"a.rs","content":"source evidence","numLines":1,"startLine":1,"totalLines":1}}),
        ))
    }
}

struct Budget;
#[async_trait]
impl BudgetEnforcerHandle for Budget {
    async fn check_and_charge(&self, _: u64) -> Result<(), BudgetError> {
        Ok(())
    }
    async fn snapshot_total_nano_usd(&self) -> u64 {
        0
    }
    async fn reserve_nano_usd(&self, _: u64) -> Result<BudgetReservationId, BudgetError> {
        Ok(BudgetReservationId::NOOP)
    }
}
struct Prices;
impl fusion_engine::FusionPriceBook for Prices {
    fn rates_for(&self, _: &str, _: &str) -> Option<fusion_engine::ModelRates> {
        Some(fusion_engine::ModelRates {
            input_nano_usd_per_token: 1,
            output_nano_usd_per_token: 1,
            per_request_nano_usd: 1,
            cache_read_nano_usd_per_token: 1,
            cache_write_nano_usd_per_token: 1,
            reasoning_nano_usd_per_token: 1,
            cache_write_rate_is_ttl_approximated: false,
        })
    }
}

struct Frames(VecDeque<Vec<u8>>);
impl FrameStream for Frames {
    fn next_frame(&mut self) -> BoxFuture<'_, Result<Option<RawStreamFrame>, LlmError>> {
        let frame = self.0.pop_front().map(RawStreamFrame::new);
        Box::pin(async move { Ok(frame) })
    }
}

fn response_stream(
    model: &str,
    content: Value,
    tool: bool,
    input: u64,
    output: u64,
) -> StreamingResponse {
    // FrameStream is the already-framed transport interface; codecs consume
    // individual JSON payloads, not event:/data: envelope strings.
    let block = if tool {
        json!({"type":"tool_use","id":"tool-response","name":content["name"],"input":{}})
    } else {
        json!({"type":"text","text":""})
    };
    let delta = if tool {
        json!({"type":"input_json_delta","partial_json":content["input"].to_string()})
    } else {
        json!({"type":"text_delta","text":content.as_str().unwrap()})
    };
    let frames = vec![
        json!({"type":"message_start","message":{"id":"msg-offline","model":model,"usage":{"input_tokens":input,"output_tokens":0}}}),
        json!({"type":"content_block_start","index":0,"content_block":block}),
        json!({"type":"content_block_delta","index":0,"delta":delta}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"message_delta","delta":{"stop_reason":if tool { "tool_use" } else { "end_turn" }},"usage":{"output_tokens":output}}),
        json!({"type":"message_stop"}),
    ];
    StreamingResponse {
        status: 200,
        headers: BTreeMap::new(),
        frames: Box::new(Frames(
            frames
                .into_iter()
                .map(|frame| serde_json::to_vec(&frame).unwrap())
                .collect(),
        )),
    }
}

#[derive(Default)]
struct WireState {
    panel_calls: BTreeMap<String, usize>,
    panel_refs: BTreeMap<String, String>,
    analyst: usize,
    synth: usize,
    verified_rows: usize,
}

struct EvidenceTransport {
    state: Mutex<WireState>,
    forged: bool,
    capability: bool,
}

fn judge_payload(body: &Value) -> Option<Value> {
    body.get("messages")?
        .as_array()?
        .iter()
        .rev()
        .filter(|message| message["role"] == "user")
        .flat_map(|message| message["content"].as_array().into_iter().flatten())
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .filter_map(|text| serde_json::from_str::<Value>(text).ok())
        .find(|value| value.get("panels").is_some())
}

fn validate_host_payload(payload: &Value, state: &WireState, capable: bool) -> BTreeSet<String> {
    let mut allowed = BTreeSet::new();
    let panels = payload["panels"].as_array().expect("real Fusion payload");
    assert_eq!(panels.len(), 2);
    for panel in panels {
        let rows = panel.get("host_evidence").and_then(Value::as_array);
        if !capable {
            assert!(
                rows.is_none(),
                "unverified legacy tools cannot gain host metadata"
            );
            continue;
        }
        let rows = rows.expect("actual runner receipt must reach judge");
        assert_eq!(rows.len(), 1);
        let attestation = &rows[0]["attestation"];
        assert_eq!(attestation["fetched"], true);
        assert_eq!(attestation["included_in_request"], true);
        assert_eq!(attestation["source"], "native_tool");
        assert_eq!(attestation["excerpt_status"], "present");
        let reference = attestation["receipt_ref"].as_str().unwrap();
        assert!(state.panel_refs.values().any(|known| known == reference));
        assert!(
            allowed.insert(reference.to_owned()),
            "different panels own different receipts"
        );
        assert_eq!(rows[0]["excerpt"], "source evidence");
        if let Some(report) = panel.get("report") {
            assert_eq!(report["evidence"][0]["receipt_ref"], reference);
        }
    }
    allowed
}

impl Transport for EvidenceTransport {
    fn execute<'a>(
        &'a self,
        request: &'a ProviderRequest,
    ) -> BoxFuture<'a, Result<ProviderResponse, LlmError>> {
        let body = &request.body_json;
        assert!(!body.to_string().contains("lingxi-private-evidence-"));
        let payload = judge_payload(body).expect("only synthesis is nonstreaming");
        let mut state = self.state.lock().unwrap();
        state.synth += 1;
        assert_eq!(state.synth, 1, "no verification or retry model call");
        assert_eq!(state.analyst, 1);
        let allowed = validate_host_payload(&payload, &state, self.capability);
        let text = if self.forged {
            "merged [evidence:evr_00000000000000000000000000000000]".to_owned()
        } else if let Some(reference) = allowed.first() {
            format!("merged [evidence:{reference}]")
        } else {
            "legacy merged answer".into()
        };
        let response = ProviderResponse::json(
            200,
            json!({"id":"msg-synth","type":"message","role":"assistant","model":body["model"],"content":[{"type":"text","text":text}],"stop_reason":"end_turn","usage":{"input_tokens":INPUT,"output_tokens":OUTPUT}}),
        );
        Box::pin(async move { Ok(response) })
    }

    fn open_stream<'a>(
        &'a self,
        request: &'a ProviderRequest,
    ) -> BoxFuture<'a, Result<StreamingResponse, LlmError>> {
        assert_eq!(
            request.stream_transport,
            llm_client::ProviderStreamTransport::Http
        );
        let body = &request.body_json;
        assert!(!body.to_string().contains("lingxi-private-evidence-"));
        let model = body["model"].as_str().unwrap();
        let mut state = self.state.lock().unwrap();
        let response = if let Some(payload) = judge_payload(body) {
            state.analyst += 1;
            assert_eq!(state.analyst, 1);
            let allowed = validate_host_payload(&payload, &state, self.capability);
            state.verified_rows = allowed.len();
            let scores = payload["panels"]
                .as_array()
                .unwrap()
                .iter()
                .map(|panel| {
                    (
                        panel["panel_id"].as_str().unwrap().to_owned(),
                        json!({"correctness":90}),
                    )
                })
                .collect::<serde_json::Map<String, Value>>();
            response_stream(model, Value::String(json!({"schema_version":1,"consensus":["compatible"],"contradictions":[],"unique_insights":[],"coverage_gaps":[],"scores":scores,"confidence":95,"recommendation":{"type":"merge","reason":"combine"}}).to_string()), false, 101, 13)
        } else {
            let calls = state.panel_calls.entry(model.to_owned()).or_default();
            *calls += 1;
            assert!(*calls <= 2, "each real panel gets exactly Read then report");
            if *calls == 1 {
                let advertised = body["tools"]
                    .as_array()
                    .expect("resolved tool schemas must reach wire");
                assert!(
                    advertised.iter().any(|tool| tool["name"] == "Read"),
                    "Read must pass the real tool resolver and be advertised"
                );
                assert!(advertised
                    .iter()
                    .any(|tool| tool["name"] == "StructuredOutput"));
                response_stream(
                    model,
                    json!({"name":"Read","input":{"file_path":"a.rs"}}),
                    true,
                    11,
                    7,
                )
            } else {
                let result = body["messages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .flat_map(|message| message["content"].as_array().into_iter().flatten())
                    .filter(|block| block["type"] == "tool_result")
                    .last()
                    .expect("runner produced real tool result");
                let text = result["content"].as_str().unwrap();
                assert!(text.contains("source evidence"));
                let reference = text
                    .split_once("\nreceipt_ref: ")
                    .map(|(_, tail)| tail.lines().next().unwrap().to_owned());
                assert_eq!(reference.is_some(), self.capability);
                let mut evidence =
                    json!({"id":"e1","kind":"file","locator":"a.rs","excerpt":"source evidence"});
                if let Some(reference) = reference {
                    assert!(reference.starts_with("evr_") && reference.len() == 36);
                    state.panel_refs.insert(model.to_owned(), reference.clone());
                    evidence["receipt_ref"] = json!(reference);
                }
                response_stream(
                    model,
                    json!({"name":"StructuredOutput","input":{"schema_version":1,"summary":"summary","candidate_answer":"candidate","claims":[{"statement":"source exists","evidence_refs":["e1"],"confidence":90}],"evidence":[evidence],"assumptions":[],"risks":[],"unresolved_questions":[]}}),
                    true,
                    11,
                    7,
                )
            }
        };
        Box::pin(async move { Ok(response) })
    }
}

fn service(transport: Arc<EvidenceTransport>) -> Arc<llm_client::ApiService> {
    let provider = llm_client::ProviderProfile {
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
                    tools: true,
                    reasoning: true,
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
    Arc::new(llm_client::ApiService::new(
        Arc::new(
            llm_client::DefaultLlmClient::from_config(llm_client::ClientConfig {
                providers: vec![provider],
            })
            .unwrap(),
        ),
        transport,
        Default::default(),
        Default::default(),
        "offline-test",
        None,
        None,
    ))
}

async fn run_case(forged: bool, capability: bool) -> FusionUsage {
    let transport = Arc::new(EvidenceTransport {
        state: Mutex::new(WireState::default()),
        forged,
        capability,
    });
    let service = service(transport.clone());
    let api = Arc::new(orchestrator::ProviderApiAdapter::new(service.clone()));
    let pool = Arc::new(agent::StateMachinePool::new(
        Arc::new(test_harness::mocks::MockRuntimeSpawner::default()),
        2,
    ));
    let read = Arc::new(ReadFixture {
        name: "Read",
        capability,
        permissions: AtomicUsize::new(0),
        calls: AtomicUsize::new(0),
    });
    let mut registry = tool_api::ToolRegistry::new();
    registry.register_builtin(read.clone());
    // The locked panel definition explicitly names all four read-only tools.
    // resolve_subagent_tools rejects missing explicit names; it does not simply
    // filter them. Register inert fixtures for the three uncalled tools instead
    // of weakening the real resolver/runner guard.
    for name in ["Grep", "Glob", "WebFetch"] {
        registry.register_builtin(Arc::new(ReadFixture {
            name,
            capability: false,
            permissions: AtomicUsize::new(0),
            calls: AtomicUsize::new(0),
        }));
    }
    let registry = Arc::new(registry);
    let spawner = Arc::new(
        agent::PoolSubagentSpawner::new(pool)
            .with_api_client(api)
            .with_tool_registry(registry.clone()),
    );
    let invoker = Arc::new(tool_api::RegistryToolInvoker::new(registry.clone()));
    assert!(Arc::ptr_eq(invoker.registry_arc(), &registry));
    let catalog = MODELS
        .into_iter()
        .map(|model| fusion_engine::CatalogModel {
            profile: "anthropic".into(),
            model: model.into(),
            hints: FusionModelHints {
                eligible: true,
                judge_eligible: true,
                quality_rank: 90,
                ..Default::default()
            },
            structured_output: true,
            limits: fusion_engine::ModelLimits {
                context_window_tokens: Some(200_000),
                max_input_tokens: Some(180_000),
                max_output_tokens: Some(32_000),
            },
        })
        .collect::<Vec<_>>();
    let executor = Arc::new(
        fusion_engine::FusionOrchestrator::new(
            spawner,
            Arc::new(sidequery::ProviderSideQueryClient::from_service(service)),
            Arc::new(fusion_engine::FusionRuntimeConfig {
                analysis_protocol_retries: 0,
                ..fusion_engine::FusionRuntimeConfig::defaults()
            }),
            Arc::new(catalog),
        )
        .with_price_book(Arc::new(Prices))
        .with_panel_admission(),
    );
    let request = FusionRequest {
        schema_version: 1,
        origin: FusionOrigin::Slash,
        prompt: "Review a.rs with Read".into(),
        preset: FusionPreset::Quality,
        models: Some(
            MODELS
                .into_iter()
                .map(|model| FusionModelRef {
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
        conversation_id: None,
        workflow_run_id: None,
    };
    let inheritance = FusionInheritance::new(
        platform_api::subagent_spawn::SubagentInheritance {
            tool_invoker: invoker,
            budget: Arc::new(Budget),
        },
        tokio_util::sync::CancellationToken::new(),
    );
    let prepared = executor
        .prepare(
            FusionSubmission::new(
                request,
                inheritance,
                FusionRunIdentity::new(FusionRunId::generated(), None, FusionOrigin::Slash, None),
            )
            .unwrap(),
        )
        .unwrap();
    let outcome = tokio::time::timeout(
        Duration::from_secs(15),
        prepared.activate(FusionActivation::now(), None),
    )
    .await
    .expect("offline Fusion must drain");
    assert_eq!(outcome.facts.allocated_panels, Some(2));
    let result = outcome
        .result
        .expect("citation rejection remains a usable NeedsParent result");
    if forged {
        assert!(matches!(
            result.decision,
            FusionDecision::NeedsParent { .. }
        ));
        assert!(result.final_text.contains("evidence") || result.final_text.contains("citation"));
    } else {
        assert!(matches!(result.decision, FusionDecision::Merged));
    }
    assert_eq!(read.calls.load(Ordering::SeqCst), 2);
    assert_eq!(read.permissions.load(Ordering::SeqCst), 2);
    let state = transport.state.lock().unwrap();
    assert_eq!(state.panel_calls.len(), 2);
    assert!(state.panel_calls.values().all(|calls| *calls == 2));
    assert_eq!(state.analyst, 1);
    assert_eq!(state.synth, 1);
    assert_eq!(state.verified_rows, if capability { 2 } else { 0 });
    assert!(
        result.usage.input_tokens >= INPUT,
        "billed synthesis input survives"
    );
    assert!(
        result.usage.output_tokens >= OUTPUT,
        "billed synthesis output survives"
    );
    assert!(
        result.usage.realized_nano_usd >= INPUT + OUTPUT,
        "billed synthesis price survives"
    );
    result.usage
}

#[tokio::test]
async fn fusion_evidence_real_pool_wire_and_merge_preserve_citations_and_invalid_usage() {
    let valid = run_case(false, true).await;
    let invalid = run_case(true, true).await;
    assert_eq!(valid.input_tokens, invalid.input_tokens);
    assert_eq!(valid.output_tokens, invalid.output_tokens);
    assert_eq!(valid.realized_nano_usd, invalid.realized_nano_usd);
}

#[tokio::test]
async fn fusion_evidence_legacy_read_without_receipt_is_never_host_verified() {
    run_case(false, false).await;
}
