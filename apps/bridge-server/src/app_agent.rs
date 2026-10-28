//! Private host-to-child JSONL transport for a single isolated Local App task.
//!
//! This endpoint is never exposed to a renderer. Credentials/configuration enter
//! only through bounded stdin; stdout contains a small typed protocol, never
//! debug formatting of configuration, provider errors, thought or chat history.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use cost::{
    CostCalculator, ModelRef as CostModelRef, PricingCatalog, ProviderId as CostProviderId,
};
use harness_runtime::desktop::app_invocation::{
    AppChatProfile, AppInvocationIdentity, AppInvocationProfile, DEFAULT_MAX_TURNS,
    DEFAULT_TIMEOUT_MS, MAX_MAX_TURNS, MAX_TIMEOUT_MS, prepare_app_config,
};
use harness_runtime::desktop::{DesktopConfig, DesktopRuntime, SharedCredentialStack};
use lingxi_core::host::permission_gate::{PermissionDecision, PermissionGate};
use lingxi_core::host::{CostSnapshot, OrchestratorHandle, OutputStream};
use lingxi_core::types::utf16_json::Utf16JsonProjection;
use lingxi_core::types::{ContentBlock, ConversationMessage, MessageId, ToolUseId};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tool_api::ask_user_question::AskUserQuestionExchange;
use tool_api::{SessionToolGrantLease, SessionToolPolicy};

const PROTOCOL_VERSION: u32 = 1;
const MAX_START_BYTES: usize = 116 * 1024 * 1024;
const MAX_CONTROL_BYTES: usize = 512 * 1024;
const MAX_OUTPUT_BYTES: usize = 2 * 1024 * 1024;
const MAX_PROMPT_BYTES: usize = 256 * 1024;
const MAX_SCHEMA_BYTES: usize = 64 * 1024;
const CANCELLED: u8 = 1;
const INTERRUPTED: u8 = 2;
const INVALID_CONTROL: u8 = 3;
const TIMED_OUT: u8 = 4;

/// Versioned private stdin start payload. These fields are resolved by the
/// trusted host, not accepted from `window.localApp.agent.run` unchanged.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartRequest {
    #[serde(rename = "type")]
    kind: StartKind,
    pub protocol_version: u32,
    pub run_id: String,
    pub app_id: String,
    pub version_id: String,
    pub invocation_id: String,
    pub cwd: PathBuf,
    pub config_home: PathBuf,
    #[serde(default)]
    pub global_config_path: Option<PathBuf>,
    pub model: String,
    #[serde(default)]
    pub provider_profiles: Option<BTreeMap<String, Value>>,
    #[serde(default)]
    pub routing: Option<Value>,
    #[serde(default)]
    pub credentials: Value,
    pub tool_policy: SessionToolPolicy,
    #[serde(default)]
    pub mcp_configs: Vec<mcp::McpServerConfig>,
    pub input: local_app::v2::agent::Input,
    #[serde(default)]
    pub app_chat: Option<AppChatProfile>,
    #[serde(default)]
    pub skill_source_directory: PathBuf,
    pub release_revision: u64,
    #[serde(default)]
    pub skill_specs: Vec<local_app::v2::AgentSkill>,
    #[serde(default)]
    pub granted_skills: Vec<String>,
    #[serde(default)]
    pub release_files: Vec<local_app::v2::ReleaseFile>,
    pub model_capabilities: AgentModelCapabilities,
    #[serde(default = "default_max_turns")]
    pub max_turns: u32,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentModelCapabilities {
    pub vision: bool,
    pub structured_output: bool,
}
fn default_max_turns() -> u32 {
    DEFAULT_MAX_TURNS
}
fn default_timeout_ms() -> u64 {
    DEFAULT_TIMEOUT_MS
}

/// Catalog output is host-only: configurations may carry sensitive headers.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CatalogRequest {
    #[serde(rename = "type")]
    kind: CatalogKind,
    pub protocol_version: u32,
    pub cwd: PathBuf,
    pub config_home: PathBuf,
    #[serde(default)]
    pub global_config_path: Option<PathBuf>,
    #[serde(default)]
    pub credentials: Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum StartKind {
    Start,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum CatalogKind {
    Catalog,
}
enum InitialRequest {
    Start(StartRequest),
    Catalog(CatalogRequest),
}

fn parse_initial(bytes: &[u8]) -> Result<InitialRequest, &'static str> {
    #[derive(Deserialize)]
    struct Header {
        #[serde(rename = "type")]
        kind: String,
    }
    let header: Header = serde_json::from_slice(bytes).map_err(|_| "invalid_start")?;
    // Parse directly from source rather than serde's internally-tagged enum
    // buffer, which cannot preserve RawValue JSON schemas (including UTF-16).
    match header.kind.as_str() {
        "start" => serde_json::from_slice(bytes)
            .map(InitialRequest::Start)
            .map_err(|_| "invalid_start"),
        "catalog" => serde_json::from_slice(bytes)
            .map(InitialRequest::Catalog)
            .map_err(|_| "invalid_start"),
        _ => Err("invalid_start"),
    }
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Control {
    Cancel,
    #[serde(rename_all = "camelCase")]
    PermissionResponse {
        request_id: u64,
        approved: bool,
    },
    #[serde(rename_all = "camelCase")]
    QuestionResponse {
        request_id: u64,
        answers: Vec<QuestionAnswer>,
    },
    #[serde(rename_all = "camelCase")]
    QuestionCancel {
        request_id: u64,
    },
    CredentialUpdate {
        credentials: Value,
    },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct QuestionAnswer {
    question: String,
    answer: String,
}

/// Reads up to one bounded JSONL frame without ever allocating an unbounded line.
async fn read_frame<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    limit: usize,
) -> Result<Option<Vec<u8>>, &'static str> {
    let mut bytes = Vec::new();
    loop {
        let buffer = reader.fill_buf().await.map_err(|_| "stdin_read_failed")?;
        if buffer.is_empty() {
            return if bytes.is_empty() {
                Ok(None)
            } else {
                Err("truncated_frame")
            };
        }
        let count = buffer
            .iter()
            .position(|b| *b == b'\n')
            .map_or(buffer.len(), |n| n + 1);
        if bytes.len().saturating_add(count) > limit + 1 {
            return Err("frame_too_large");
        }
        let complete = buffer[count - 1] == b'\n';
        bytes.extend_from_slice(&buffer[..count]);
        reader.consume(count);
        if complete {
            bytes.pop();
            if bytes.last() == Some(&b'\r') {
                bytes.pop();
            }
            if bytes.is_empty() {
                return Err("empty_frame");
            }
            return Ok(Some(bytes));
        }
    }
}

struct WriterState {
    output: Box<dyn AsyncWrite + Send + Unpin>,
    sequence: u64,
    terminal: bool,
}
struct EventSink {
    run_id: String,
    state: Mutex<WriterState>,
    cancel: CancellationToken,
}
impl EventSink {
    fn new(
        run_id: String,
        output: impl AsyncWrite + Send + Unpin + 'static,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            run_id,
            state: Mutex::new(WriterState {
                output: Box::new(output),
                sequence: 0,
                terminal: false,
            }),
            cancel,
        }
    }
    async fn emit(&self, mut event: Value) {
        let mut state = self.state.lock().await;
        if state.terminal {
            return;
        }
        state.sequence += 1;
        event["protocolVersion"] = json!(PROTOCOL_VERSION);
        event["runId"] = json!(self.run_id);
        event["sequence"] = json!(state.sequence);
        let is_terminal = event["type"] == "finished";
        let Ok(mut bytes) = serde_json::to_vec(&event) else {
            self.cancel.cancel();
            return;
        };
        // The catalog has its own bounded host-only result path below.
        if bytes.len() > MAX_OUTPUT_BYTES {
            self.cancel.cancel();
            return;
        }
        bytes.push(b'\n');
        if state.output.write_all(&bytes).await.is_err() || state.output.flush().await.is_err() {
            self.cancel.cancel();
        }
        state.terminal = is_terminal;
    }
    async fn fail(&self, code: &'static str, message: &'static str) {
        self.emit(
            json!({"type":"finished","status":"failed","error":{"code":code,"message":message}}),
        )
        .await;
    }
}

struct InteractionState {
    next_id: AtomicU64,
    permissions: Mutex<HashMap<u64, oneshot::Sender<bool>>>,
    questions: Mutex<HashMap<u64, AskUserQuestionExchange>>,
    lease: Mutex<Option<SessionToolGrantLease>>,
    oauth: Mutex<Option<Arc<llm_runtime::auth::openai::HostSession>>>,
    reason: AtomicU8,
    cancel: CancellationToken,
    sink: Arc<EventSink>,
}
impl InteractionState {
    fn new(sink: Arc<EventSink>, cancel: CancellationToken) -> Self {
        Self {
            next_id: AtomicU64::new(1),
            permissions: Mutex::new(HashMap::new()),
            questions: Mutex::new(HashMap::new()),
            lease: Mutex::new(None),
            oauth: Mutex::new(None),
            reason: AtomicU8::new(0),
            cancel,
            sink,
        }
    }
    async fn cancel(&self, reason: u8) {
        let _ = self
            .reason
            .compare_exchange(0, reason, Ordering::AcqRel, Ordering::Acquire);
        if let Some(lease) = self.lease.lock().await.as_ref() {
            lease.revoke();
        }
        self.cancel.cancel();
        self.permissions.lock().await.clear();
        self.questions.lock().await.clear();
    }
}
#[async_trait]
impl PermissionGate for InteractionState {
    async fn has_pending_request(&self) -> bool {
        !self.permissions.lock().await.is_empty()
    }
    async fn check(&self, name: &str, input: &Utf16JsonProjection) -> PermissionDecision {
        // This internal validator cannot create external effects and is not a user grant.
        if name == orchestrator::structured_output::STRUCTURED_OUTPUT_TOOL_NAME {
            return PermissionDecision::Allow;
        }
        if self.cancel.is_cancelled() {
            return denied();
        }
        let Ok(input) = input.try_to_serde_value_with_array_budget(100_000) else {
            return denied();
        };
        let request_id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        self.permissions.lock().await.insert(request_id, sender);
        self.sink
            .emit(json!({"type":"permission","requestId":request_id,"toolName":name,"input":input}))
            .await;
        let approved = tokio::select! {
            biased;
            _ = self.cancel.cancelled() => false,
            answer = receiver => answer.unwrap_or(false),
        };
        self.permissions.lock().await.remove(&request_id);
        if approved && !self.cancel.is_cancelled() {
            PermissionDecision::Allow
        } else {
            denied()
        }
    }
}
fn denied() -> PermissionDecision {
    PermissionDecision::Deny {
        reason: "The Local App tool request was not approved.".into(),
    }
}

fn runtime_build_error_code(error: harness_runtime::desktop::BuildError) -> &'static str {
    // Schema diagnostics may include user schema fragments. Classify the typed
    // failure without formatting it across the application boundary.
    match error {
        harness_runtime::desktop::BuildError::StructuredOutputSchema(_) => "invalid_schema",
        _ => "runtime_initialization_failed",
    }
}

struct NoPermissionSink;
#[async_trait]
impl client::adapter::PermissionRequestSink for NoPermissionSink {
    async fn emit_request(&self, _: client::protocol::permission::PermissionRequest) {}
}

struct AppOutput {
    sink: Arc<EventSink>,
    text: Mutex<String>,
    current_assistant_text: Mutex<String>,
    last_assistant_text: Mutex<String>,
    assistant_message_id: Mutex<Option<String>>,
    error: AtomicU8,
    cost_baseline: Mutex<Option<CostSnapshot>>,
    completed_model_turns: AtomicU32,
}
#[async_trait]
impl OutputStream for AppOutput {
    async fn emit_text(&self, text: &Utf16JsonProjection) {
        let Some(text) = text.string_at("") else {
            return;
        };
        let mut accumulated = self.text.lock().await;
        if accumulated.len().saturating_add(text.len()) > MAX_OUTPUT_BYTES / 2 {
            self.error.store(1, Ordering::Release);
            self.sink.cancel.cancel();
            return;
        }
        accumulated.push_str(text);
        drop(accumulated);
        let mut current = self.current_assistant_text.lock().await;
        if current.len().saturating_add(text.len()) > MAX_OUTPUT_BYTES / 2 {
            self.error.store(1, Ordering::Release);
            self.sink.cancel.cancel();
            return;
        }
        current.push_str(text);
        drop(current);
        self.sink.emit(json!({"type":"text","delta":text})).await;
    }
    async fn emit_tool_call(&self, id: &ToolUseId, tool: &str, _: &Utf16JsonProjection) {
        self.sink
            .emit(json!({"type":"tool_started","toolCallId":id.to_string(),"tool":tool}))
            .await;
    }
    async fn emit_tool_result(
        &self,
        id: &ToolUseId,
        tool: &str,
        _: &Utf16JsonProjection,
        _: &Utf16JsonProjection,
        _: Option<&Utf16JsonProjection>,
    ) {
        self.sink
            .emit(json!({"type":"tool_finished","toolCallId":id.to_string(),"tool":tool}))
            .await;
    }
    async fn emit_end_turn(&self, _: &str, cost: &CostSnapshot) {
        let completed = self.completed_model_turns.fetch_add(1, Ordering::AcqRel) + 1;
        self.sink
            .emit(json!({"type":"progress","phase":"thinking","completedModelTurns":completed}))
            .await;
        let baseline = self.cost_baseline.lock().await.clone().unwrap_or_default();
        self.sink
            .emit(json!({"type":"usage","usage":usage(cost, &baseline, completed)}))
            .await;
    }
    async fn emit_terminal_api_error(&self, _: Option<u16>, _: &str, _: &str) {
        self.error.store(2, Ordering::Release);
    }
    async fn emit_assistant_message_identity(&self, message_id: &lingxi_core::types::MessageId) {
        *self.assistant_message_id.lock().await = Some(message_id.to_string());
        let text = {
            let mut current = self.current_assistant_text.lock().await;
            std::mem::take(&mut *current)
        };
        *self.last_assistant_text.lock().await = text;
    }
}
fn usage(cost: &CostSnapshot, baseline: &CostSnapshot, turns: u32) -> Value {
    let input = cost.input_tokens.saturating_sub(baseline.input_tokens);
    let output = cost.output_tokens.saturating_sub(baseline.output_tokens);
    let amount = (cost.total_usd - baseline.total_usd).max(0.0);
    let mut value = serde_json::Map::new();
    value.insert("inputTokens".into(), json!(input));
    value.insert("outputTokens".into(), json!(output));
    value.insert("turns".into(), json!(turns));
    value.insert("costEstimated".into(), json!(!cost.unknown_models));
    if !cost.unknown_models {
        value.insert("costUsd".into(), json!(amount));
    }
    Value::Object(value)
}

fn side_query_usage(usage: &cost::Usage, turns: Option<u32>, model: &str, profile: &str) -> Value {
    let mut value = serde_json::Map::new();
    value.insert("inputTokens".into(), json!(usage.tokens.input));
    value.insert("outputTokens".into(), json!(usage.tokens.output));
    if let Some(turns) = turns {
        value.insert("turns".into(), json!(turns));
    }
    let provider = CostProviderId::Custom {
        name: profile.to_owned(),
    };
    let reference = CostModelRef {
        provider,
        model: model.to_owned(),
    };
    let pricing = PricingCatalog::builtin_reference();
    if let Ok((rate, _)) = pricing.resolve(&reference) {
        let nanos = CostCalculator::calculate_nano_usd(usage, &rate);
        value.insert("costUsd".into(), json!(nanos as f64 / 1_000_000_000.0));
        value.insert("costEstimated".into(), json!(true));
    }
    Value::Object(value)
}

fn credentials(value: Value) -> Result<crate::boot::CredentialEnvelope, &'static str> {
    let value = if value.is_null() { json!({}) } else { value };
    let bytes = serde_json::to_vec(&value).map_err(|_| "invalid_credentials")?;
    crate::boot::read_credential_envelope(&mut std::io::Cursor::new(bytes))
        .map_err(|_| "invalid_credentials")
}

fn validate_agent_input(input: &local_app::v2::agent::Input) -> Result<(), &'static str> {
    fn has_only_local_schema_references(value: &Value) -> bool {
        match value {
            Value::Object(fields) => fields.iter().all(|(key, value)| {
                if matches!(key.as_str(), "$ref" | "$dynamicRef" | "$recursiveRef") {
                    value.as_str().is_some_and(|reference| reference.starts_with('#'))
                } else {
                    has_only_local_schema_references(value)
                }
            }),
            Value::Array(values) => values.iter().all(has_only_local_schema_references),
            _ => true,
        }
    }
    local_app::v2::agent::validate_input(input).map_err(|_| "invalid_input")?;
    if input.prompt.len() > MAX_PROMPT_BYTES {
        return Err("invalid_input");
    }
    if input.output_schema.as_ref().is_some_and(|schema| {
        serde_json::to_vec(schema).map_or(true, |bytes| bytes.len() > MAX_SCHEMA_BYTES)
            || !has_only_local_schema_references(schema)
    }) {
        return Err("invalid_schema");
    }
    Ok(())
}

fn valid_id(id: &str) -> bool {
    !id.trim().is_empty() && id.len() <= 256 && !id.chars().any(char::is_control)
}
fn valid_app_id(id: &str) -> bool {
    // Match the AppLibrary identifier contract before using it as a path segment.
    !id.is_empty()
        && id.len() <= 80
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}
fn validate_start(request: &StartRequest) -> Result<(), &'static str> {
    if request.protocol_version != PROTOCOL_VERSION {
        return Err("protocol_version");
    }
    if ![
        &request.run_id,
        &request.app_id,
        &request.version_id,
        &request.invocation_id,
    ]
    .into_iter()
    .all(|id| valid_id(id))
        || !valid_app_id(&request.app_id)
    {
        return Err("invalid_identity");
    }
    validate_paths(
        &request.cwd,
        &request.config_home,
        request.global_config_path.as_deref(),
    )?;
    if request.model.is_empty()
        || request.model.len() > 256
        || request.model.chars().any(char::is_control)
    {
        return Err("invalid_model");
    }
    if request.max_turns == 0
        || request.timeout_ms == 0
        || request.max_turns > MAX_MAX_TURNS
        || request.timeout_ms > MAX_TIMEOUT_MS
    {
        return Err("invalid_limits");
    }
    validate_agent_input(&request.input)?;
    if request
        .input
        .max_turns
        .is_some_and(|turns| turns > request.max_turns)
        || request
            .input
            .timeout_ms
            .is_some_and(|timeout| timeout > request.timeout_ms)
    {
        return Err("invalid_limits");
    }
    if (!request.input.tools.is_empty() || request.input.output_schema.is_some())
        && !request.model_capabilities.structured_output
    {
        return Err("model_structured_output_unsupported");
    }
    if request.release_revision == 0
        || !request.skill_source_directory.is_absolute()
        || request.skill_specs.len() > 128
        || request.granted_skills.len() > 128
        || request.release_files.len() > 20_000
    {
        return Err("invalid_skills");
    }
    // AppLibrary resolves config-home aliases before returning release paths.
    // Resolve only this trusted anchor: canonicalizing the expected release
    // itself would also authorize a symlink into another app or outside home.
    let expected_source = std::fs::canonicalize(&request.config_home)
        .map_err(|_| "invalid_skills")?
        .join("local-apps/v2/apps")
        .join(&request.app_id)
        .join("releases")
        .join(request.release_revision.to_string())
        .join("source");
    if request.skill_source_directory != expected_source {
        return Err("invalid_skills");
    }
    if !request.input.skills.iter().all(|id| {
        request.granted_skills.contains(id)
            && request.skill_specs.iter().any(|skill| skill.id == *id)
    }) {
        return Err("invalid_skills");
    }
    let source_metadata =
        std::fs::symlink_metadata(&request.skill_source_directory).map_err(|_| "invalid_skills")?;
    if source_metadata.file_type().is_symlink()
        || !source_metadata.is_dir()
        || std::fs::canonicalize(&request.skill_source_directory).map_err(|_| "invalid_skills")?
            != expected_source
    {
        return Err("invalid_skills");
    }
    if request.mcp_configs.len() > 128 {
        return Err("invalid_mcp_config");
    }
    if request
        .skill_specs
        .iter()
        .map(|skill| &skill.id)
        .collect::<std::collections::HashSet<_>>()
        .len()
        != request.skill_specs.len()
        || request
            .granted_skills
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len()
            != request.granted_skills.len()
    {
        return Err("invalid_skills");
    }
    Ok(())
}
fn validate_paths(
    cwd: &std::path::Path,
    home: &std::path::Path,
    global: Option<&std::path::Path>,
) -> Result<(), &'static str> {
    if !cwd.is_absolute() || !home.is_absolute() || global.is_some_and(|p| !p.is_absolute()) {
        return Err("invalid_directories");
    }
    Ok(())
}

/// Set process-wide configuration ownership once, before any runtime/settings read.
fn set_config_owner(
    home: &std::path::Path,
    global: Option<&std::path::Path>,
) -> Result<(), &'static str> {
    migrations::global_config::set_config_home_override(home.to_path_buf())
        .map_err(|_| "config_owner_conflict")?;
    lingxi_core::settings::loader::set_user_settings_path_override(home.join("settings.json"))
        .map_err(|_| "config_owner_conflict")?;
    if let Some(path) = global {
        migrations::global_config::set_global_config_path_override(path.to_path_buf())
            .map_err(|_| "config_owner_conflict")?;
    }
    Ok(())
}

struct TokenRequester {
    sink: Arc<EventSink>,
}
impl llm_runtime::auth::openai::HostRefreshRequester for TokenRequester {
    fn request_refresh(&self) {
        let sink = self.sink.clone();
        tokio::spawn(async move {
            sink.emit(json!({"type":"credential_required","provider":"codex"}))
                .await;
        });
    }
}

async fn credential_stack(
    config: &DesktopConfig,
    envelope: crate::boot::CredentialEnvelope,
    state: &Arc<InteractionState>,
) -> Result<SharedCredentialStack, &'static str> {
    // MCP OAuth retains the trusted host vault. Provider credentials are a
    // separate in-memory manager seeded only with the bounded parent envelope.
    let mut shared = harness_runtime::desktop::build_shared_credential_stack_with_policy(
        &config.lingxi_home,
        false,
        lingxi_core::host::CredentialStoragePolicy::NativeOrMemory,
    )
    .await
    .map_err(|_| "credential_storage_unavailable")?;
    shared.credentials = Arc::new(secret::CredentialManager::new(
        Arc::new(lingxi_core::host::InMemorySecureStorage::new()),
        shared.clock.clone(),
        shared.http.clone(),
    ));
    let host = Arc::new(llm_runtime::auth::openai::HostSession::new(
        shared.clock.clone(),
    ));
    host.request_refreshes_through(Arc::new(TokenRequester {
        sink: state.sink.clone(),
    }));
    if let Some(session) = envelope.openai_oauth {
        host.serve(crate::boot::openai_host_token(&session));
    }
    *state.oauth.lock().await = Some(host.clone());
    shared.openai_host_session = Some(host);
    crate::boot::seed_parent_supplied_provider_keys(
        &shared.catalog_registry,
        &shared.credentials,
        &envelope.provider_keys,
    )
    .await;
    Ok(shared)
}

fn base_config(
    cwd: PathBuf,
    config_home: PathBuf,
    model: String,
    providers: Option<BTreeMap<String, Value>>,
    routing: Option<Value>,
    gate: Arc<InteractionState>,
) -> DesktopConfig {
    let mut config = DesktopConfig::default();
    config.cwd = cwd;
    config.lingxi_home = config_home;
    config.default_model = model;
    config.provider_profiles = providers;
    config.routing = routing;
    config.api_base = crate::boot::resolve_api_base();
    config.isolated_credential_storage = true;
    config.credential_storage_policy = lingxi_core::host::CredentialStoragePolicy::NativeOrMemory;
    config.injected_permission_gate = Some(gate);
    config.host_workspace_trusted = Some(true);
    // Catalog assembly must register the same host-question tool as a run.
    // A real run replaces this unused catalog channel with its question pump.
    let (question_tx, _) = mpsc::channel(1);
    config.ask_user_question_tx = Some(question_tx);
    config
}

async fn pump_controls<R: AsyncBufRead + Unpin>(mut reader: R, state: Arc<InteractionState>) {
    loop {
        let frame = tokio::select! {
            _ = state.cancel.cancelled() => break,
            frame = read_frame(&mut reader, MAX_CONTROL_BYTES) => frame,
        };
        let control = match frame {
            Ok(Some(bytes)) => match serde_json::from_slice::<Control>(&bytes) {
                Ok(c) => c,
                Err(_) => {
                    state.cancel(INVALID_CONTROL).await;
                    break;
                }
            },
            Ok(None) => {
                state.cancel(INTERRUPTED).await;
                break;
            }
            Err(_) => {
                state.cancel(INVALID_CONTROL).await;
                break;
            }
        };
        match control {
            Control::Cancel => {
                state.cancel(CANCELLED).await;
                break;
            }
            Control::PermissionResponse {
                request_id,
                approved,
            } => {
                if let Some(sender) = state.permissions.lock().await.remove(&request_id) {
                    let _ = sender.send(approved);
                }
            }
            Control::QuestionResponse {
                request_id,
                answers,
            } => {
                if let Some(exchange) = state.questions.lock().await.remove(&request_id) {
                    // The trusted host may supply free text, but must answer the
                    // actual questions exactly once rather than inject new keys.
                    if answers.len() == exchange.questions.len()
                        && exchange.questions.iter().zip(&answers).all(|(q, a)| {
                            q.question.string_at("") == Some(a.question.as_str())
                                && a.answer.len() <= 64 * 1024
                        })
                    {
                        let _ = exchange.resp_tx.send(
                            answers
                                .into_iter()
                                .map(|a| (a.question.into(), a.answer.into()))
                                .collect(),
                        );
                    }
                }
            }
            Control::QuestionCancel { request_id } => {
                state.questions.lock().await.remove(&request_id);
            }
            Control::CredentialUpdate { credentials: value } => {
                if let Ok(envelope) = credentials(value) {
                    if let (Some(host), Some(session)) =
                        (state.oauth.lock().await.as_ref(), envelope.openai_oauth)
                    {
                        host.serve(crate::boot::openai_host_token(&session));
                    }
                } else {
                    state.cancel(INVALID_CONTROL).await;
                    break;
                }
            }
        }
    }
}

async fn pump_questions(
    mut receiver: mpsc::Receiver<AskUserQuestionExchange>,
    state: Arc<InteractionState>,
) {
    loop {
        let exchange = tokio::select! { _ = state.cancel.cancelled() => break, e = receiver.recv() => match e { Some(e) => e, None => break } };
        let request_id = state.next_id.fetch_add(1, Ordering::Relaxed);
        let questions: Vec<_> = exchange.questions.iter().map(|q| json!({
            "question":q.question.string_at("").unwrap_or_default(),"header":q.header.string_at("").unwrap_or_default(),"multi_select":q.multi_select,
            "options":q.options.iter().map(|o| json!({"label":o.label.string_at("").unwrap_or_default(),"description":o.description.string_at("").unwrap_or_default()})).collect::<Vec<_>>()
        })).collect();
        state.questions.lock().await.insert(request_id, exchange);
        state
            .sink
            .emit(json!({"type":"question","requestId":request_id,"questions":questions}))
            .await;
    }
}

async fn execute(
    request: StartRequest,
    state: Arc<InteractionState>,
    output: Arc<AppOutput>,
) -> Result<Value, &'static str> {
    let prepared = harness_runtime::app_agent_input::prepare(&request.input)
        .await
        .map_err(|error| {
            if error == "attachment_type_unsupported" {
                "attachment_type_unsupported"
            } else {
                "invalid_attachments"
            }
        })?;
    let uses_vision = prepared.uses_vision;
    if uses_vision && !request.model_capabilities.vision {
        return Err("model_vision_unsupported");
    }
    if request.app_chat.is_none()
        && request.input.tools.is_empty()
        && request.input.skills.is_empty()
    {
        return execute_side_query(request, prepared.blocks, state).await;
    }
    let skill_instructions = harness_runtime::app_agent_skills::materialize(
        &request.skill_source_directory,
        &request.cwd,
        &request.skill_specs,
        &request.release_files,
        &request.input.skills,
    )
    .await
    .map_err(|_| "invalid_skills")?;
    let envelope = credentials(request.credentials.clone())?;
    let mut config = base_config(
        request.cwd.clone(),
        request.config_home.clone(),
        request.model.clone(),
        request.provider_profiles.clone(),
        request.routing.clone(),
        state.clone(),
    );
    config.api_key = envelope.api_key.clone().unwrap_or_default();
    config.api_key_source = llm_runtime::CredentialSource::Configured;
    config.cli_mcp_servers = request.mcp_configs.clone();
    config.json_schema = request
        .input
        .output_schema
        .as_ref()
        .map(|schema| {
            serde_json::to_string(schema)
                .map_err(|_| "invalid_schema")
                .and_then(|encoded| {
                    Utf16JsonProjection::parse(&encoded).map_err(|_| "invalid_schema")
                })
        })
        .transpose()?;
    let schema_required = config.json_schema.is_some();
    let (question_tx, question_rx) = mpsc::channel(8);
    config.ask_user_question_tx = Some(question_tx);
    let expected_mcp_tools = request.tool_policy.mcp_tools.clone();
    let mut profile = AppInvocationProfile::new(
        AppInvocationIdentity {
            app_id: request.app_id.clone(),
            version_id: request.version_id.clone(),
            invocation_id: request.invocation_id.clone(),
        },
        request.tool_policy.clone(),
    );
    profile.max_turns = request.max_turns;
    profile.timeout_ms = request.timeout_ms;
    profile.app_chat = request.app_chat.clone();
    profile.skill_instructions = skill_instructions;
    config = prepare_app_config(config, profile).map_err(|_| "invalid_profile")?;
    *state.lease.lock().await = config.session_tool_grant_lease.clone();
    let shared = credential_stack(&config, envelope, &state).await?;
    let runtime = tokio::select! {
        biased;
        _ = state.cancel.cancelled() => return Err("cancelled"),
        result = Box::pin(harness_runtime::desktop::build_with_credential_stack(config, output.clone(), Arc::new(NoPermissionSink), shared)) => result.map_err(runtime_build_error_code)?,
    };
    let actual_mcp_tools = runtime.mcp_registry.tool_identity_catalog().await;
    if !expected_mcp_tools
        .iter()
        .all(|grant| actual_mcp_tools.contains(grant))
    {
        if let Some(lease) = state.lease.lock().await.as_ref() {
            lease.revoke();
        }
        let _ = tokio::time::timeout(
            Duration::from_secs(10),
            runtime.session_lifecycle.shutdown_and_drain(),
        )
        .await;
        return Err("tool_changed");
    }
    let result = execute_runtime(
        &runtime,
        &request.model,
        request.app_chat.as_ref().map(|chat| chat.chat_id.as_str()),
        prepared.blocks,
        uses_vision,
        schema_required,
        state.clone(),
        output,
        question_rx,
    )
    .await;
    // Stop discovery/watchers and settle cost before publishing the sole terminal.
    if let Some(lease) = state.lease.lock().await.as_ref() {
        lease.revoke();
    }
    let _ = tokio::time::timeout(
        Duration::from_secs(10),
        runtime.session_lifecycle.shutdown_and_drain(),
    )
    .await;
    result
}

async fn execute_side_query(
    request: StartRequest,
    blocks: Vec<ContentBlock>,
    state: Arc<InteractionState>,
) -> Result<Value, &'static str> {
    let Some((profile, model)) = request.model.split_once('/') else {
        return Err("invalid_model");
    };
    if profile.is_empty() || model.is_empty() {
        return Err("invalid_model");
    }
    let envelope = credentials(request.credentials.clone())?;
    let mut config = base_config(
        request.cwd.clone(),
        request.config_home.clone(),
        request.model.clone(),
        request.provider_profiles.clone(),
        request.routing.clone(),
        state.clone(),
    );
    config.api_key = envelope.api_key.clone().unwrap_or_default();
    config.api_key_source = llm_runtime::CredentialSource::Configured;
    config.cli_mcp_servers = request.mcp_configs.clone();
    config.routing = None;
    let shared = credential_stack(&config, envelope, &state).await?;
    state
        .sink
        .emit(json!({"type":"started","model":request.model}))
        .await;
    let message = user_message(blocks);
    let message_id = format!("msg_{}", uuid::Uuid::new_v4());
    let result = if let Some(schema) = request.input.output_schema.clone() {
        let query = harness_runtime::desktop::app_invocation::strict_side_query_request(
            model.to_owned(),
            Some(profile.to_owned()),
            vec![message],
            schema,
        );
        tokio::select! {
            biased;
            _ = state.cancel.cancelled() => return Err("cancelled"),
            result = harness_runtime::desktop::app_invocation::run_side_query_json_schema_accounted(config, shared, query) => {
                match result {
                    Ok(response) => {
                        let usage = side_query_usage(&response.usage, Some(1), model, profile);
                        state.sink.emit(json!({"type":"usage","usage":usage})).await;
                        if state.cancel.is_cancelled() { return Err("cancelled"); }
                        json!({"type":"finished","status":"succeeded","text":"","result":response.value,"messageId":message_id,"usage":usage})
                    }
                    Err(error) => {
                        if let Some(usage) = error.usage {
                            state.sink.emit(json!({"type":"usage","usage":side_query_usage(&usage, error.turns, model, profile)})).await;
                        }
                        return Err(error.code);
                    }
                }
            }
        }
    } else {
        let mut query = harness_runtime::desktop::app_invocation::side_query_request(
            model.to_owned(),
            Some(profile.to_owned()),
            vec![message],
            None,
        );
        query.max_tokens = Some(8192);
        query.max_retries = 0;
        query.thinking = Some(llm_runtime::model::thinking::ThinkingConfig::Disabled);
        tokio::select! {
            biased;
            _ = state.cancel.cancelled() => return Err("cancelled"),
            result = harness_runtime::desktop::app_invocation::run_side_query_accounted(config, shared, query) => {
                match result {
                    Ok(response) => {
                        if !response.tool_calls.is_empty() { return Err("model_tools_unsupported"); }
                        let text = response.text.and_then(|value| value.string_at("").map(str::to_owned)).ok_or("model_response_empty")?;
                        let usage = side_query_usage(&response.usage, Some(1), model, profile);
                        state.sink.emit(json!({"type":"usage","usage":usage})).await;
                        if state.cancel.is_cancelled() { return Err("cancelled"); }
                        json!({"type":"finished","status":"succeeded","text":text,"messageId":message_id,"usage":usage})
                    }
                    Err(error) => {
                        if let Some(usage) = error.usage {
                            state.sink.emit(json!({"type":"usage","usage":side_query_usage(&usage, error.turns, model, profile)})).await;
                        }
                        return Err(error.code);
                    }
                }
            }
        }
    };
    serde_json::to_vec(&result)
        .map_err(|_| "invalid_result")
        .and_then(|bytes| {
            if bytes.len() > MAX_OUTPUT_BYTES - 512 {
                Err("result_too_large")
            } else {
                Ok(result)
            }
        })
}

fn user_message(blocks: Vec<ContentBlock>) -> ConversationMessage {
    let mut message = ConversationMessage::user(MessageId::new(), String::new());
    if let ConversationMessage::User { content, .. } = &mut message {
        *content = blocks;
    }
    message
}

async fn execute_runtime(
    runtime: &DesktopRuntime,
    model: &str,
    chat_id: Option<&str>,
    blocks: Vec<ContentBlock>,
    uses_vision: bool,
    schema_required: bool,
    state: Arc<InteractionState>,
    output: Arc<AppOutput>,
    question_rx: mpsc::Receiver<AskUserQuestionExchange>,
) -> Result<Value, &'static str> {
    let listings = runtime.orchestrator.list_model_listings().await;
    let selected = listings
        .iter()
        .find(|row| {
            row.request_model == model
                || format!("{}/{}", row.provider_id, row.request_model) == model
        })
        .ok_or("model_unavailable")?;
    {
        let session = runtime.orchestrator.session();
        let session = session.lock().await;
        if (session.model != selected.request_model && session.model != model)
            || session
                .model_profile
                .as_deref()
                .is_some_and(|profile| profile != selected.provider_id)
        {
            return Err("model_selection_changed");
        }
    }
    if uses_vision && !selected.capabilities.vision {
        return Err("model_vision_unsupported");
    }
    if schema_required && !selected.capabilities.tools {
        return Err("model_structured_output_unsupported");
    }
    let questions = tokio::spawn(pump_questions(question_rx, state.clone()));
    state
        .sink
        .emit(json!({"type":"started","model":model}))
        .await;
    let content = serde_json::to_string(&blocks).map_err(|_| "invalid_attachments")?;
    let content = Utf16JsonProjection::parse(&content).map_err(|_| "invalid_attachments")?;
    let baseline = runtime.orchestrator.snapshot_cost().await;
    *output.cost_baseline.lock().await = Some(baseline.clone());
    let result = runtime
        .orchestrator
        .run_turn_streaming_with_cancel_projected_content(&content, state.cancel.clone(), None)
        .await;
    questions.abort();
    state.questions.lock().await.clear();
    state.permissions.lock().await.clear();
    let usage = usage(
        &runtime.orchestrator.snapshot_cost().await,
        &baseline,
        output.completed_model_turns.load(Ordering::Acquire),
    );
    state.sink.emit(json!({"type":"usage","usage":usage})).await;
    if state.cancel.is_cancelled() {
        return Err("cancelled");
    }
    if output.error.load(Ordering::Acquire) != 0 {
        return Err("model_execution_failed");
    }
    match result {
        Ok(orchestrator::conversation::TurnOutcome::EndTurn) => {}
        Ok(orchestrator::conversation::TurnOutcome::MaxTurns) => return Err("turn_limit_exceeded"),
        Ok(orchestrator::conversation::TurnOutcome::Cancelled) => return Err("cancelled"),
        Err(_) => return Err("model_execution_failed"),
    }
    let structured = runtime
        .structured_output_slot
        .as_ref()
        .and_then(|slot| slot.lock().ok()?.clone());
    if schema_required && structured.is_none() {
        return Err("structured_output_missing");
    }
    let result = structured
        .map(|value| value.try_to_serde_value_with_array_budget(100_000))
        .transpose()
        .map_err(|_| "result_too_large")?;
    if schema_required && result.as_ref().is_some_and(Value::is_null) {
        return Err("structured_output_not_representable");
    }
    let message_id = output.assistant_message_id.lock().await.clone().ok_or("assistant_message_identity_missing")?;
    let text = output.last_assistant_text.lock().await.clone();
    let mut terminal = json!({"type":"finished","status":"succeeded","text":text,"messageId":message_id,"usage":usage});
    if let Some(result) = result { terminal["result"] = result; }
    if let Some(chat_id) = chat_id { terminal["chatId"] = json!(chat_id); }
    if serde_json::to_vec(&terminal)
        .map_err(|_| "invalid_result")?
        .len()
        > MAX_OUTPUT_BYTES - 512
    {
        return Err("result_too_large");
    }
    Ok(terminal)
}

async fn catalog(request: CatalogRequest) -> Result<(), &'static str> {
    if request.protocol_version != PROTOCOL_VERSION {
        return Err("protocol_version");
    }
    validate_paths(
        &request.cwd,
        &request.config_home,
        request.global_config_path.as_deref(),
    )?;
    set_config_owner(&request.config_home, request.global_config_path.as_deref())?;
    std::env::set_current_dir(&request.cwd).map_err(|_| "invalid_directories")?;
    let resolved = crate::boot::resolve_desktop_config(&crate::boot::BridgeArgs {
        trusted_workspace: true,
        packaged_credential_stdin_only: true,
        session_config_home: Some(request.config_home.clone()),
        ..Default::default()
    });
    let global = request.global_config_path.unwrap_or_else(|| {
        migrations::global_config::global_config_path()
            .unwrap_or_else(|| PathBuf::from("/dev/null"))
    });
    let selected_mcp = mcp::load_mcp_servers(&request.cwd.join(".mcp.json"), &global, &request.cwd);
    let cancel = CancellationToken::new();
    let sink = Arc::new(EventSink::new(
        String::new(),
        tokio::io::sink(),
        cancel.clone(),
    ));
    let state = Arc::new(InteractionState::new(sink.clone(), cancel));
    let output = Arc::new(AppOutput {
        sink,
        text: Mutex::new(String::new()),
        current_assistant_text: Mutex::new(String::new()),
        last_assistant_text: Mutex::new(String::new()),
        assistant_message_id: Mutex::new(None),
        error: AtomicU8::new(0),
        cost_baseline: Mutex::new(None),
        completed_model_turns: AtomicU32::new(0),
    });
    let envelope = credentials(request.credentials)?;
    let mut config = base_config(
        request.cwd,
        request.config_home,
        resolved.default_model,
        resolved.provider_profiles,
        resolved.routing,
        state.clone(),
    );
    config.api_key = envelope.api_key.clone().unwrap_or_default();
    config.api_key_source = llm_runtime::CredentialSource::Configured;
    config.cli_mcp_servers = selected_mcp;
    config = prepare_app_config(
        config,
        AppInvocationProfile::new(
            AppInvocationIdentity {
                app_id: "host-catalog".into(),
                version_id: "catalog".into(),
                invocation_id: uuid::Uuid::new_v4().to_string(),
            },
            SessionToolPolicy::default(),
        ),
    )
    .map_err(|_| "invalid_profile")?;
    let provider_profiles = config.provider_profiles.clone();
    let routing = config.routing.clone();
    let shared = credential_stack(&config, envelope, &state).await?;
    let runtime = Box::pin(harness_runtime::desktop::build_with_credential_stack(
        config,
        output,
        Arc::new(NoPermissionSink),
        shared,
    ))
    .await
    .map_err(|_| "catalog_initialization_failed")?;
    let models = runtime.orchestrator.list_model_listings().await;
    // Snapshot each descriptor and its exact source config under one lock.
    // The host must not reconstruct identities from model-facing aliases.
    let connections = runtime.mcp_registry.connections.read().await;
    let mut names = connections.keys().collect::<Vec<_>>();
    names.sort();
    let mut mcp_configs = Vec::new();
    let mut mcp_tools = Vec::new();
    for name in names {
        let Some(connection) = connections.get(name) else {
            continue;
        };
        let (config, tools) = match connection {
            mcp::McpConnectionState::Connected { config, tools, .. }
            | mcp::McpConnectionState::Cached { config, tools, .. } => (config, tools),
            _ => continue,
        };
        let config_index = mcp_configs.len();
        for tool in tools {
            let identity = tool_api::McpToolIdentity::from_config_tool(config, tool);
            let mut row =
                serde_json::to_value(identity).map_err(|_| "catalog_serialization_failed")?;
            row["configIndex"] = json!(config_index);
            row["label"] = json!(format!("{} / {}", config.name, tool.tool_name));
            row["description"] = json!(
                tool.description
                    .string_at("")
                    .unwrap_or_default()
                    .chars()
                    .take(1024)
                    .collect::<String>()
            );
            mcp_tools.push(row);
        }
        mcp_configs.push(config.clone());
    }
    drop(connections);
    let registry = runtime.orchestrator.shared_tool_registry();
    let mut builtins = Vec::new();
    for name in harness_runtime::desktop::app_invocation::APP_AGENT_BUILTIN_TOOLS {
        if let Some(tool) = registry.find_registered(name) {
            let description = tool
                .description(
                    &Utf16JsonProjection::plain(json!({})),
                    &tool_api::tool_trait::DescriptionOptions {
                        is_non_interactive_session: false,
                    },
                )
                .await;
            builtins.push(json!({"name":name,"description":description.chars().take(1024).collect::<String>()}));
        }
    }
    let result = json!({"type":"catalog","protocolVersion":PROTOCOL_VERSION,"models":models,"builtins":builtins,"providerProfiles":provider_profiles,"routing":routing,"mcpTools":mcp_tools,"mcpConfigs":mcp_configs});
    let _ = tokio::time::timeout(
        Duration::from_secs(10),
        runtime.session_lifecycle.shutdown_and_drain(),
    )
    .await;
    let mut bytes = serde_json::to_vec(&result).map_err(|_| "catalog_serialization_failed")?;
    if bytes.len() > 8 * 1024 * 1024 {
        return Err("catalog_too_large");
    }
    bytes.push(b'\n');
    let mut stdout = tokio::io::stdout();
    stdout
        .write_all(&bytes)
        .await
        .map_err(|_| "stdout_closed")?;
    stdout.flush().await.map_err(|_| "stdout_closed")?;
    Ok(())
}

/// Runs the private JSONL mode. No global tracing subscriber is installed, so
/// provider/MCP diagnostics cannot accidentally print secrets or corrupt stdout.
pub async fn run() -> anyhow::Result<()> {
    // A dependency panic must not dump its request/configuration payload. The
    // parent turns an abnormal process exit into an interrupted task record.
    std::panic::set_hook(Box::new(|_| {
        eprintln!("Local App Agent runtime terminated unexpectedly.");
    }));
    #[cfg(any(unix, windows))]
    if let Ok(executable) = std::env::current_exe() {
        harness_runtime::desktop::shell_supervisor::enable_supervisor(executable);
    }
    platform_posix::http::prewarm_native_roots();
    let mut reader = BufReader::new(tokio::io::stdin());
    let initial = match read_frame(&mut reader, MAX_START_BYTES).await {
        Ok(Some(bytes)) => parse_initial(&bytes).ok(),
        _ => None,
    };
    match initial {
        Some(InitialRequest::Catalog(request)) => {
            if let Err(code) = tokio::time::timeout(Duration::from_secs(90), catalog(request))
                .await
                .unwrap_or(Err("catalog_timeout"))
            {
                let mut stdout = tokio::io::stdout();
                let line = format!(
                    "{}\n",
                    json!({"type":"catalog_error","protocolVersion":PROTOCOL_VERSION,"error":{"code":code,"message":"Unable to load the host Agent catalog."}})
                );
                stdout.write_all(line.as_bytes()).await?;
                stdout.flush().await?;
            }
        }
        Some(InitialRequest::Start(request)) => {
            let cancel = CancellationToken::new();
            let sink = Arc::new(EventSink::new(
                if valid_id(&request.run_id) {
                    request.run_id.clone()
                } else {
                    String::new()
                },
                tokio::io::stdout(),
                cancel.clone(),
            ));
            if let Err(code) = validate_start(&request).and_then(|_| {
                set_config_owner(&request.config_home, request.global_config_path.as_deref())
            }) {
                sink.fail(code, "The host supplied an invalid Agent task.")
                    .await;
                return Ok(());
            }
            let state = Arc::new(InteractionState::new(sink.clone(), cancel));
            let control = tokio::spawn(pump_controls(reader, state.clone()));
            let timeout_state = state.clone();
            let timeout_ms = request.timeout_ms;
            let deadline = tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(timeout_ms)).await;
                timeout_state.cancel(TIMED_OUT).await;
            });
            let output = Arc::new(AppOutput {
                sink: sink.clone(),
                text: Mutex::new(String::new()),
                current_assistant_text: Mutex::new(String::new()),
                last_assistant_text: Mutex::new(String::new()),
                assistant_message_id: Mutex::new(None),
                error: AtomicU8::new(0),
                cost_baseline: Mutex::new(None),
                completed_model_turns: AtomicU32::new(0),
            });
            let mut execution = Box::pin(execute(request, state.clone(), output));
            let result = tokio::select! {
                result = &mut execution => result,
                _ = state.cancel.cancelled() => {
                    // Give the engine its cancellation drain opportunity; a hung
                    // provider/credential backend cannot keep the task alive forever.
                    tokio::time::timeout(Duration::from_secs(5), &mut execution).await.unwrap_or(Err("cancelled"))
                }
            };
            deadline.abort();
            control.abort();
            match state.reason.load(Ordering::Acquire) {
                CANCELLED => sink.emit(json!({"type":"finished","status":"cancelled"})).await,
                INTERRUPTED => sink.emit(json!({"type":"finished","status":"interrupted","error":{"code":"host_disconnected","message":"The host connection was interrupted."}})).await,
                TIMED_OUT => sink.fail("timeout", "The Agent task reached its time limit.").await,
                INVALID_CONTROL => sink.fail("invalid_control", "The host sent an invalid Agent control message.").await,
                _ => match result {
                    Ok(event) => sink.emit(event).await,
                    Err(code) => sink.fail(code, "The Agent task could not complete. Check the selected model, credentials and granted tools.").await,
                }
            }
        }
        None => {
            let sink = EventSink::new(String::new(), tokio::io::stdout(), CancellationToken::new());
            sink.fail(
                "invalid_start",
                "Expected a bounded version 1 Agent start or catalog message.",
            )
            .await;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_and_runs_register_the_host_question_tool() {
        let cancel = CancellationToken::new();
        let sink = Arc::new(EventSink::new(
            "catalog".into(),
            tokio::io::sink(),
            cancel.clone(),
        ));
        let config = base_config(
            "/tmp/app-run".into(),
            "/tmp/app-config".into(),
            "test/model".into(),
            None,
            None,
            Arc::new(InteractionState::new(sink, cancel)),
        );
        assert!(config.ask_user_question_tx.is_some());
    }

    #[tokio::test]
    async fn frames_are_bounded_and_newline_required() {
        let mut data = std::io::Cursor::new(b"1234\nnext\n");
        assert_eq!(
            read_frame(&mut data, 4).await.unwrap(),
            Some(b"1234".to_vec())
        );
        assert_eq!(
            read_frame(&mut data, 4).await.unwrap(),
            Some(b"next".to_vec())
        );
        assert!(
            read_frame(&mut std::io::Cursor::new(b"12345\n"), 4)
                .await
                .is_err()
        );
        assert!(
            read_frame(&mut std::io::Cursor::new(b"1234"), 4)
                .await
                .is_err()
        );
    }
    fn input() -> local_app::v2::agent::Input {
        local_app::v2::agent::Input {
            request_id: "request-1".into(),
            protocol_thread_id: None,
            protocol_parent_run_id: None,
            protocol_input_digest: None,
            protocol_message_sha256: None,
            prompt: "Describe this".into(),
            attachments: Vec::new(),
            content_parts: Vec::new(),
            output_schema: None,
            tools: Vec::new(),
            skills: Vec::new(),
            max_turns: None,
            timeout_ms: None,
        }
    }
    fn start_request(config_home: &std::path::Path) -> StartRequest {
        std::fs::create_dir_all(config_home).unwrap();
        let app_id = format!("a{}", "1".repeat(32));
        let library = local_app::v2::AppLibrary::open(config_home).unwrap();
        let source = library.source_path(&app_id, 1).unwrap();
        std::fs::create_dir_all(&source).unwrap();
        StartRequest {
            kind: StartKind::Start,
            protocol_version: PROTOCOL_VERSION,
            run_id: "run-1".into(),
            app_id,
            version_id: "a".repeat(64),
            invocation_id: "window-1".into(),
            cwd: config_home.join("work"),
            config_home: config_home.to_path_buf(),
            global_config_path: None,
            model: "test/model".into(),
            provider_profiles: None,
            routing: None,
            credentials: Value::Null,
            tool_policy: SessionToolPolicy::default(),
            mcp_configs: Vec::new(),
            input: input(),
            app_chat: None,
            skill_source_directory: source,
            release_revision: 1,
            skill_specs: Vec::new(),
            granted_skills: Vec::new(),
            release_files: Vec::new(),
            model_capabilities: AgentModelCapabilities {
                vision: true,
                structured_output: true,
            },
            max_turns: DEFAULT_MAX_TURNS,
            timeout_ms: DEFAULT_TIMEOUT_MS,
        }
    }

    #[test]
    fn start_source_keeps_exact_app_and_revision_ownership() {
        let temp = tempfile::tempdir().unwrap();
        let mut request = start_request(temp.path());
        assert_eq!(validate_start(&request), Ok(()));
        let source = request.skill_source_directory.clone();
        let app_id = request.app_id.clone();
        let home = std::fs::canonicalize(temp.path()).unwrap();
        let other_app = home.join("local-apps/v2/apps/other_App-2/releases/1/source");
        let other_revision = source.parent().unwrap().parent().unwrap().join("2/source");
        std::fs::create_dir_all(&other_app).unwrap();
        std::fs::create_dir_all(&other_revision).unwrap();

        request.app_id = "other_App-2".into();
        assert_eq!(validate_start(&request), Err("invalid_skills"));
        request.app_id = app_id;
        request.release_revision = 2;
        assert_eq!(validate_start(&request), Err("invalid_skills"));
        request.release_revision = 1;
        for foreign_source in [other_app, other_revision, source.join("../source")] {
            request.skill_source_directory = foreign_source;
            assert_eq!(validate_start(&request), Err("invalid_skills"));
        }
    }

    #[cfg(unix)]
    #[test]
    fn start_source_accepts_a_config_home_parent_symlink() {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        let physical = root.join("physical");
        std::fs::create_dir(&physical).unwrap();
        std::os::unix::fs::symlink(&physical, root.join("alias")).unwrap();
        let request = start_request(&root.join("alias/config"));
        assert_ne!(
            request.config_home,
            std::fs::canonicalize(&request.config_home).unwrap()
        );
        assert_eq!(validate_start(&request), Ok(()));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn start_source_accepts_the_macos_tmp_parent_alias() {
        let temp = tempfile::tempdir_in("/tmp").unwrap();
        let request = start_request(temp.path());
        assert!(request.config_home.starts_with("/tmp"));
        assert!(request.skill_source_directory.starts_with("/private/tmp"));
        assert_eq!(validate_start(&request), Ok(()));
    }

    #[cfg(unix)]
    #[test]
    fn start_source_rejects_a_final_source_symlink() {
        let temp = tempfile::tempdir().unwrap();
        let mut request = start_request(&temp.path().join("config"));
        let source = request.skill_source_directory.clone();
        let redirected = temp.path().join("redirected");
        std::fs::rename(&source, &redirected).unwrap();
        std::os::unix::fs::symlink(&redirected, &source).unwrap();
        assert_eq!(validate_start(&request), Err("invalid_skills"));
        // A trailing current-directory component can make symlink_metadata
        // follow the link; the unresolved expected path must still reject it.
        request.skill_source_directory = source.join(".");
        assert_eq!(validate_start(&request), Err("invalid_skills"));
    }

    #[cfg(unix)]
    #[test]
    fn start_source_rejects_symlink_escapes_below_config_home() {
        let app_id = format!("a{}", "1".repeat(32));
        for relative in [
            "local-apps".to_owned(),
            "local-apps/v2".into(),
            "local-apps/v2/apps".into(),
            format!("local-apps/v2/apps/{app_id}"),
            format!("local-apps/v2/apps/{app_id}/releases"),
            format!("local-apps/v2/apps/{app_id}/releases/1"),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let request = start_request(&temp.path().join("config"));
            let redirected = temp.path().join("redirected");
            let ancestor = std::fs::canonicalize(&request.config_home)
                .unwrap()
                .join(&relative);
            std::fs::rename(&ancestor, &redirected).unwrap();
            std::os::unix::fs::symlink(&redirected, &ancestor).unwrap();
            assert_eq!(
                validate_start(&request),
                Err("invalid_skills"),
                "{relative}"
            );
        }
    }

    #[test]
    fn start_source_rejects_missing_paths_and_non_directories() {
        let temp = tempfile::tempdir().unwrap();
        let mut request = start_request(temp.path());
        let home = request.config_home.clone();
        request.config_home = home.join("missing");
        assert_eq!(validate_start(&request), Err("invalid_skills"));
        request.config_home = home;
        std::fs::remove_dir(&request.skill_source_directory).unwrap();
        assert_eq!(validate_start(&request), Err("invalid_skills"));
        std::fs::write(&request.skill_source_directory, b"not a directory").unwrap();
        assert_eq!(validate_start(&request), Err("invalid_skills"));
    }

    #[test]
    fn start_source_requires_library_app_identifiers() {
        let temp = tempfile::tempdir().unwrap();
        let mut request = start_request(temp.path());
        for app_id in ["legacy_App-2".to_owned(), "X".repeat(80)] {
            request.app_id = app_id;
            request.skill_source_directory = std::fs::canonicalize(&request.config_home)
                .unwrap()
                .join("local-apps/v2/apps")
                .join(&request.app_id)
                .join("releases/1/source");
            std::fs::create_dir_all(&request.skill_source_directory).unwrap();
            assert_eq!(validate_start(&request), Ok(()));
        }
        for app_id in [
            "../other".to_owned(),
            "/absolute".into(),
            "app/other".into(),
            "app\\other".into(),
            ".".into(),
            "..".into(),
            "app:name".into(),
            "X".repeat(81),
        ] {
            request.app_id = app_id;
            assert_eq!(validate_start(&request), Err("invalid_identity"));
        }
        request.app_id = "valid-app".into();
        for version_id in [String::new(), "version\n".into(), "X".repeat(257)] {
            request.version_id = version_id;
            assert_eq!(validate_start(&request), Err("invalid_identity"));
        }
    }

    #[test]
    fn private_input_validation_rejects_paths_and_external_schema_references() {
        let mut value = input();
        value.attachments.push(local_app::v2::agent::Attachment {
            name: "../private.png".into(),
            media_type: "image/png".into(),
            base64: "AAAA".into(),
        });
        assert!(validate_agent_input(&value).is_err());
        for reference in ["$ref", "$dynamicRef", "$recursiveRef"] {
            let mut value = input();
            value.output_schema = Some(json!({"properties":{"item":{reference:"https://example.invalid/schema"}}}));
            assert_eq!(validate_agent_input(&value), Err("invalid_schema"));
        }
        let mut value = input();
        value.output_schema = Some(json!({"definitions":{"item":{"type":"string"}},"properties":{"item":{"$ref":"#/definitions/item"}}}));
        assert!(validate_agent_input(&value).is_ok());
    }
    #[tokio::test]
    async fn output_sink_retains_the_actual_assistant_message_identity_for_the_private_terminal() {
        let cancel = CancellationToken::new();
        let sink = Arc::new(EventSink::new("run-1".into(), tokio::io::sink(), cancel));
        let output = AppOutput {
            sink,
            text: Mutex::new(String::new()),
            current_assistant_text: Mutex::new(String::new()),
            last_assistant_text: Mutex::new(String::new()),
            assistant_message_id: Mutex::new(None),
            error: AtomicU8::new(0),
            cost_baseline: Mutex::new(None),
            completed_model_turns: AtomicU32::new(0),
        };
        let message_id = MessageId::new();
        output.emit_text(&Utf16JsonProjection::plain(json!("committed answer"))).await;
        output.emit_assistant_message_identity(&message_id).await;
        let expected_id = message_id.to_string();
        assert_eq!(output.assistant_message_id.lock().await.as_deref(), Some(expected_id.as_str()));
        assert_eq!(&*output.last_assistant_text.lock().await, "committed answer");
    }
    #[test]
    fn credentials_fail_without_echoing_input() {
        let secret = "DO_NOT_PRINT_THE_TOKEN";
        assert_eq!(
            credentials(json!({"unexpected":secret})).err(),
            Some("invalid_credentials")
        );
    }

    #[test]
    fn build_error_codes_distinguish_invalid_schema_without_diagnostics() {
        let diagnostic = "private schema value or provider token";
        let invalid_schema = harness_runtime::desktop::BuildError::StructuredOutputSchema(
            lingxi_core::host::json_schema_validator::SchemaCompileError::InvalidSchema(
                diagnostic.into(),
            ),
        );
        assert_eq!(runtime_build_error_code(invalid_schema), "invalid_schema");
        assert_eq!(
            runtime_build_error_code(harness_runtime::desktop::BuildError::ApiBase(
                diagnostic.into()
            )),
            "runtime_initialization_failed"
        );
    }
    #[test]
    fn control_protocol_rejects_ambient_chat_and_extra_authority() {
        assert!(
            serde_json::from_value::<Control>(
                json!({"type":"permission_response","requestId":1,"approved":true})
            )
            .is_ok()
        );
        assert!(
            serde_json::from_value::<Control>(
                json!({"type":"permission_response","requestId":1,"approved":true,"always":true})
            )
            .is_err()
        );
        assert!(
            serde_json::from_value::<Control>(
                json!({"type":"send_prompt","sessionId":"another-chat"})
            )
            .is_err()
        );
    }

    #[test]
    fn start_wire_is_explicit_and_rejects_foreign_chat_authority() {
        let config_home =
            std::env::temp_dir().join(format!("app-agent-wire-{}", uuid::Uuid::new_v4()));
        let app_id = format!("a{}", "1".repeat(32));
        let source = config_home
            .join("local-apps/v2/apps")
            .join(&app_id)
            .join("releases/1/source");
        std::fs::create_dir_all(&source).unwrap();
        let source = std::fs::canonicalize(source).unwrap();
        let release_digest = "a".repeat(64);
        let official: Value = serde_json::from_str(include_str!(
            "../../electron/vendor/local-app-agui/spec/fixtures/run-agent-input.ag-ui-1.0.3.json"
        ))
        .unwrap();
        let normalized = local_app::v2::agent::protocol::normalize_run_input(&official, &[]).unwrap();
        let mut private_input = normalized.input;
        private_input.tools.clear();
        private_input.skills.clear();
        let start = json!({
            "type":"start", "protocolVersion":1, "runId":"run-fixture-1",
            "appId":app_id.clone(), "versionId":release_digest.clone(), "invocationId":"window-1",
            "cwd":"/tmp/food-run", "configHome":config_home.to_string_lossy().to_string(),
            "skillSourceDirectory":source.to_string_lossy().to_string(), "releaseRevision":1, "skillSpecs":[], "grantedSkills":[], "releaseFiles":[],
            "modelCapabilities":{"vision":true,"structuredOutput":true},
            "model":"openai/example", "credentials":{}, "toolPolicy":{"builtinTools":[],"mcpTools":[]},
            "input":serde_json::to_value(private_input).unwrap()
        });
        let InitialRequest::Start(request) =
            parse_initial(&serde_json::to_vec(&start).unwrap()).unwrap()
        else {
            panic!("expected start");
        };
        assert_eq!(request.max_turns, DEFAULT_MAX_TURNS);
        assert_eq!(request.timeout_ms, DEFAULT_TIMEOUT_MS);
        assert!(validate_start(&request).is_ok());
        assert_eq!(request.input.request_id, "run-fixture-1");
        assert_eq!(request.input.protocol_thread_id.as_deref(), Some("thread-fixture-1"));
        assert_eq!(request.input.content_parts.len(), 2);
        let content_parts = serde_json::to_value(&request.input).unwrap();
        assert_eq!(content_parts["contentParts"][1]["kind"], "attachment");
        assert_eq!(content_parts["contentParts"][1]["mediaType"], "image/png");
        assert!(content_parts["contentParts"][1].get("media_type").is_none());
        let mut foreign = start;
        foreign["sessionId"] = json!("creating-chat");
        assert!(parse_initial(&serde_json::to_vec(&foreign).unwrap()).is_err());
        std::fs::remove_dir_all(config_home).unwrap();
    }

    #[tokio::test]
    async fn permission_answer_requires_exact_pending_request_id() {
        let cancel = CancellationToken::new();
        let sink = Arc::new(EventSink::new(
            "run".into(),
            tokio::io::sink(),
            cancel.clone(),
        ));
        let state = Arc::new(InteractionState::new(sink, cancel));
        let gate = state.clone();
        let check = tokio::spawn(async move {
            gate.check(
                "Bash",
                &Utf16JsonProjection::plain(json!({"command":"echo ok"})),
            )
            .await
        });
        tokio::task::yield_now().await;
        assert!(state.permissions.lock().await.contains_key(&1));
        let (mut sender, reader) = tokio::io::duplex(4096);
        let controls = tokio::spawn(pump_controls(BufReader::new(reader), state.clone()));
        sender
            .write_all(b"{\"type\":\"permission_response\",\"requestId\":2,\"approved\":true}\n")
            .await
            .unwrap();
        tokio::task::yield_now().await;
        assert!(!check.is_finished());
        sender
            .write_all(b"{\"type\":\"permission_response\",\"requestId\":1,\"approved\":true}\n")
            .await
            .unwrap();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), check)
                .await
                .unwrap()
                .unwrap(),
            PermissionDecision::Allow
        );
        controls.abort();
        assert!(state.permissions.lock().await.is_empty());
    }

    #[tokio::test]
    async fn disconnected_host_interrupts_and_revokes_before_returning() {
        let cancel = CancellationToken::new();
        let sink = Arc::new(EventSink::new(
            "run".into(),
            tokio::io::sink(),
            cancel.clone(),
        ));
        let state = Arc::new(InteractionState::new(sink, cancel));
        let lease = SessionToolGrantLease::new(SessionToolPolicy::default());
        *state.lease.lock().await = Some(lease.clone());
        pump_controls(std::io::Cursor::new(Vec::<u8>::new()), state.clone()).await;
        assert_eq!(state.reason.load(Ordering::Acquire), INTERRUPTED);
        assert!(state.cancel.is_cancelled());
        assert!(lease.is_revoked());
    }
    #[tokio::test]
    async fn terminal_is_emitted_once_and_stops_late_events() {
        let (writer, mut reader) = tokio::io::duplex(8192);
        let sink = EventSink::new("run-1".into(), writer, CancellationToken::new());
        sink.emit(json!({"type":"started"})).await;
        sink.emit(json!({"type":"finished","status":"cancelled"}))
            .await;
        sink.emit(json!({"type":"text","delta":"late"})).await;
        drop(sink);
        let mut text = String::new();
        tokio::io::AsyncReadExt::read_to_string(&mut reader, &mut text)
            .await
            .unwrap();
        let frames: Vec<Value> = text
            .lines()
            .map(|s| serde_json::from_str(s).unwrap())
            .collect();
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0]["sequence"], 1);
        assert_eq!(frames[1]["sequence"], 2);
        assert_eq!(frames[1]["runId"], "run-1");
    }
    #[tokio::test]
    async fn cancellation_revokes_captured_lease_and_denies_pending_approval() {
        let sink = Arc::new(EventSink::new(
            "run".into(),
            tokio::io::sink(),
            CancellationToken::new(),
        ));
        let state = Arc::new(InteractionState::new(sink, CancellationToken::new()));
        let lease = SessionToolGrantLease::new(SessionToolPolicy {
            builtin_tools: ["Bash".into()].into(),
            ..Default::default()
        });
        *state.lease.lock().await = Some(lease.clone());
        let gate = state.clone();
        let task = tokio::spawn(async move {
            gate.check(
                "Bash",
                &Utf16JsonProjection::plain(json!({"command":"echo test"})),
            )
            .await
        });
        tokio::task::yield_now().await;
        state.cancel(CANCELLED).await;
        assert!(lease.is_revoked());
        assert!(matches!(
            task.await.unwrap(),
            PermissionDecision::Deny { .. }
        ));
        assert!(state.permissions.lock().await.is_empty());
    }
}
