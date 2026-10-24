//! Private host-to-child JSONL transport for a single isolated Local App task.
//!
//! This endpoint is never exposed to a renderer. Credentials/configuration enter
//! only through bounded stdin; stdout contains a small typed protocol, never
//! debug formatting of configuration, provider errors, thought or chat history.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;
use client::protocol::commands::ImageRefDto;
use harness_runtime::desktop::app_invocation::{
    prepare_app_config, AppInvocationIdentity, AppInvocationProfile, DEFAULT_MAX_TURNS,
    DEFAULT_TIMEOUT_MS,
};
use harness_runtime::desktop::{DesktopConfig, DesktopRuntime, SharedCredentialStack};
use lingxi_core::host::permission_gate::{PermissionDecision, PermissionGate};
use lingxi_core::host::{CostSnapshot, OrchestratorHandle, OutputStream};
use lingxi_core::types::utf16_json::Utf16JsonProjection;
use lingxi_core::types::ToolUseId;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio_util::sync::CancellationToken;
use tool_api::ask_user_question::AskUserQuestionExchange;
use tool_api::{SessionToolGrantLease, SessionToolPolicy};

const PROTOCOL_VERSION: u32 = 1;
const MAX_START_BYTES: usize = 116 * 1024 * 1024;
const MAX_CONTROL_BYTES: usize = 512 * 1024;
const MAX_OUTPUT_BYTES: usize = 2 * 1024 * 1024;
const MAX_PROMPT_BYTES: usize = 256 * 1024;
const MAX_SCHEMA_BYTES: usize = 256 * 1024;
const MAX_IMAGE_BYTES: usize = 20 * 1024 * 1024;
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
    pub prompt: String,
    #[serde(default)]
    pub images: Vec<ImageRefDto>,
    #[serde(default)]
    pub output_schema: Option<Box<serde_json::value::RawValue>>,
    #[serde(default = "default_max_turns")]
    pub max_turns: u32,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
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
    error: AtomicU8,
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
        self.sink
            .emit(json!({"type":"usage","usage":usage(cost)}))
            .await;
    }
    async fn emit_terminal_api_error(&self, _: Option<u16>, _: &str, _: &str) {
        self.error.store(2, Ordering::Release);
    }
}
fn usage(cost: &CostSnapshot) -> Value {
    json!({"inputTokens":cost.input_tokens,"outputTokens":cost.output_tokens,"cacheReadTokens":cost.cache_read_tokens,"cacheCreationTokens":cost.cache_creation_tokens,"totalTokens":cost.total_tokens,"totalUsd":cost.total_usd,"apiCalls":cost.api_calls})
}

fn credentials(value: Value) -> Result<crate::boot::CredentialEnvelope, &'static str> {
    let value = if value.is_null() { json!({}) } else { value };
    let bytes = serde_json::to_vec(&value).map_err(|_| "invalid_credentials")?;
    crate::boot::read_credential_envelope(&mut std::io::Cursor::new(bytes))
        .map_err(|_| "invalid_credentials")
}

fn validate_images(images: &[ImageRefDto]) -> Result<(), &'static str> {
    if images.len() > 4 {
        return Err("invalid_images");
    }
    for image in images {
        if image.base64.len() > MAX_IMAGE_BYTES.div_ceil(3) * 4 {
            return Err("invalid_images");
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&image.base64)
            .map_err(|_| "invalid_images")?;
        if bytes.is_empty()
            || bytes.len() > MAX_IMAGE_BYTES
            || base64::engine::general_purpose::STANDARD.encode(&bytes) != image.base64
        {
            return Err("invalid_images");
        }
        let mime = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            "image/png"
        } else if bytes.starts_with(b"\xff\xd8\xff") {
            "image/jpeg"
        } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
            "image/gif"
        } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
            "image/webp"
        } else {
            return Err("invalid_images");
        };
        if image.media_type != mime {
            return Err("invalid_images");
        }
    }
    Ok(())
}
fn prepare_images(images: Vec<ImageRefDto>) -> Result<Vec<ImageRefDto>, &'static str> {
    images
        .into_iter()
        .map(|image| {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(&image.base64)
                .map_err(|_| "invalid_images")?;
            // Reuse the shared model image budget: complete decoder validation,
            // camera EXIF orientation and bounded dimensions, without filesystem paths.
            let image =
                tool_api::util::image_budget::process_image(bytes).map_err(|_| "invalid_images")?;
            Ok(ImageRefDto {
                media_type: image.media_type,
                base64: image.base64,
            })
        })
        .collect()
}

fn valid_id(id: &str) -> bool {
    !id.trim().is_empty() && id.len() <= 256 && !id.chars().any(char::is_control)
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
    if request.prompt.trim().is_empty() || request.prompt.len() > MAX_PROMPT_BYTES {
        return Err("invalid_prompt");
    }
    if request.max_turns == 0
        || request.timeout_ms == 0
        || request.max_turns > 10_000
        || request.timeout_ms > 86_400_000
    {
        return Err("invalid_limits");
    }
    if request
        .output_schema
        .as_ref()
        .is_some_and(|s| s.get().len() > MAX_SCHEMA_BYTES)
    {
        return Err("invalid_schema");
    }
    if request.mcp_configs.len() > 128 {
        return Err("invalid_mcp_config");
    }
    validate_images(&request.images)
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
    let images = tokio::task::spawn_blocking(move || prepare_images(request.images))
        .await
        .map_err(|_| "invalid_images")??;
    let envelope = credentials(request.credentials)?;
    let mut config = base_config(
        request.cwd,
        request.config_home,
        request.model.clone(),
        request.provider_profiles,
        request.routing,
        state.clone(),
    );
    config.api_key = envelope.api_key.clone().unwrap_or_default();
    config.api_key_source = llm_runtime::CredentialSource::Configured;
    config.cli_mcp_servers = request.mcp_configs;
    config.json_schema = request
        .output_schema
        .as_ref()
        .map(|s| Utf16JsonProjection::parse(s.get()))
        .transpose()
        .map_err(|_| "invalid_schema")?;
    let schema_required = config.json_schema.is_some();
    let (question_tx, question_rx) = mpsc::channel(8);
    config.ask_user_question_tx = Some(question_tx);
    let expected_mcp_tools = request.tool_policy.mcp_tools.clone();
    config = prepare_app_config(
        config,
        AppInvocationProfile {
            identity: AppInvocationIdentity {
                app_id: request.app_id,
                version_id: request.version_id,
                invocation_id: request.invocation_id,
            },
            tool_policy: request.tool_policy,
            max_turns: request.max_turns,
            timeout_ms: request.timeout_ms,
        },
    )
    .map_err(|_| "invalid_profile")?;
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
        request.prompt,
        images,
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
async fn execute_runtime(
    runtime: &DesktopRuntime,
    model: &str,
    prompt: String,
    images: Vec<ImageRefDto>,
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
    if !images.is_empty() && !selected.capabilities.vision {
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
    let sources = images
        .into_iter()
        .map(|image| orchestrator::conversation::ImageSource::Base64 {
            media_type: image.media_type,
            data: image.base64,
        })
        .collect();
    let result = runtime
        .orchestrator
        .run_turn_streaming_with_cancel_image_sources(prompt, sources, state.cancel.clone())
        .await;
    questions.abort();
    state.questions.lock().await.clear();
    state.permissions.lock().await.clear();
    let usage = usage(&runtime.orchestrator.snapshot_cost().await);
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
    let terminal = json!({"type":"finished","status":"succeeded","text":output.text.lock().await.clone(),"result":result,"usage":usage});
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
        error: AtomicU8::new(0),
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
            row["description"] = json!(tool
                .description
                .string_at("")
                .unwrap_or_default()
                .chars()
                .take(1024)
                .collect::<String>());
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
                error: AtomicU8::new(0),
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
        assert!(read_frame(&mut std::io::Cursor::new(b"12345\n"), 4)
            .await
            .is_err());
        assert!(read_frame(&mut std::io::Cursor::new(b"1234"), 4)
            .await
            .is_err());
    }
    #[test]
    fn image_validation_rejects_mime_spoofing_and_noncanonical_base64() {
        let image = ImageRefDto {
            media_type: "image/png".into(),
            base64: base64::engine::general_purpose::STANDARD.encode(b"\x89PNG\r\n\x1a\nfixture"),
        };
        assert!(validate_images(std::slice::from_ref(&image)).is_ok());
        assert!(validate_images(&[ImageRefDto {
            media_type: "image/jpeg".into(),
            ..image.clone()
        }])
        .is_err());
        assert!(validate_images(&[ImageRefDto {
            base64: format!("{}\n", image.base64),
            ..image.clone()
        }])
        .is_err());
        assert!(validate_images(&vec![image; 5]).is_err());
    }
    #[test]
    fn malformed_image_pixels_fail_before_runtime_execution() {
        let image = ImageRefDto {
            media_type: "image/png".into(),
            base64: base64::engine::general_purpose::STANDARD
                .encode(b"\x89PNG\r\n\x1a\nnot a complete image"),
        };
        assert!(validate_images(std::slice::from_ref(&image)).is_ok());
        assert_eq!(prepare_images(vec![image]).err(), Some("invalid_images"));
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
        assert!(serde_json::from_value::<Control>(
            json!({"type":"permission_response","requestId":1,"approved":true})
        )
        .is_ok());
        assert!(serde_json::from_value::<Control>(
            json!({"type":"permission_response","requestId":1,"approved":true,"always":true})
        )
        .is_err());
        assert!(serde_json::from_value::<Control>(
            json!({"type":"send_prompt","sessionId":"another-chat"})
        )
        .is_err());
    }

    #[test]
    fn start_wire_is_explicit_and_rejects_foreign_chat_authority() {
        let start = json!({
            "type":"start", "protocolVersion":1, "runId":"run-1",
            "appId":"food", "versionId":"v1", "invocationId":"window-1",
            "cwd":"/tmp/food-run", "configHome":"/tmp/host-config",
            "model":"openai/example", "credentials":{}, "toolPolicy":{"builtinTools":[],"mcpTools":[]},
            "prompt":"Describe the food", "outputSchema":{"type":"object"}
        });
        let InitialRequest::Start(request) =
            parse_initial(&serde_json::to_vec(&start).unwrap()).unwrap()
        else {
            panic!("expected start");
        };
        assert_eq!(request.max_turns, DEFAULT_MAX_TURNS);
        assert_eq!(request.timeout_ms, DEFAULT_TIMEOUT_MS);
        assert!(validate_start(&request).is_ok());
        let mut foreign = start;
        foreign["sessionId"] = json!("creating-chat");
        assert!(parse_initial(&serde_json::to_vec(&foreign).unwrap()).is_err());
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
