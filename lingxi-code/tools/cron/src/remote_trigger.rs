//! `RemoteTriggerTool` — manage scheduled remote LingXi agents (triggers)
//! via the claude.ai CCR API.
//!
//! 1:1 port of claude-code's
//! `src/tools/RemoteTriggerTool/RemoteTriggerTool.ts` (+ `prompt.ts`). The tool
//! drives the network through [`tool_api::BuiltinToolContext::http`]
//! (`Arc<dyn HttpTransport>`); the OAuth access token and organization UUID are
//! resolved in-process via a [`ClaudeAiAuthProvider`] handed to
//! [`RemoteTriggerTool::new`] at the registration site — the token never reaches
//! the shell.
//!
//! Auth resolution is decoupled from the shared [`tool_api::BuiltinToolContext`]
//! (which is constructed in dozens of places): instead of adding a field there,
//! the composition root wires a concrete [`ClaudeAiAuthProvider`] only at the
//! RemoteTrigger registration site (desktop). Construction sites without an
//! auth backend (mobile, tests) pass `None`, in which case the pre-flight
//! "not authenticated" error fires.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use once_cell::sync::Lazy;
use permission::result::PermissionMetadata;
use permission::{PermissionDecisionReason, PermissionResult};
use serde_json::{json, Value};
use telemetry::sink::{AnalyticsValue, LogEventMetadata};
use telemetry::tengu::tool::{
    REMOTE_TRIGGER_COMPLETED, REMOTE_TRIGGER_FAILED, REMOTE_TRIGGER_STARTED,
};
use telemetry::AnalyticsBus;

use protocol::{HttpMethod, HttpRequest};
use tool_api::context::ToolUseContext;
use tool_api::progress::ToolProgressSender;
use tool_api::tool_trait::{
    DescriptionOptions, InterruptBehavior, PromptOptions, Tool, ToolCallResult, ToolError,
    ToolStaticContext, ValidationError,
};

/// Tool name byte-lock. Asserted by `parity_registry.rs`.
pub const REMOTE_TRIGGER_TOOL_NAME: &str = "RemoteTrigger";

/// `anthropic-beta` header value (TS `TRIGGERS_BETA`).
const TRIGGERS_BETA: &str = "ccr-triggers-2026-01-30";

/// `anthropic-version` header value.
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Per-request timeout (TS `timeout: 20_000`).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// Default `BASE_API_URL` (TS `getOauthConfig().BASE_API_URL` — the production
/// default; see `claude-code/src/constants/oauth.ts`). The composition-root
/// provider overrides this when the host resolves a non-default API base.
pub const DEFAULT_BASE_API_URL: &str = "https://api.anthropic.com";

/// Resolves the current refreshed claude.ai OAuth access token + organization
/// UUID for [`RemoteTriggerTool`].
///
/// Lives in the cron crate (NOT in `traits/`, which is frozen). A concrete impl
/// is wired at the composition root (`apps/engine-desktop`) backed by the
/// credential store; tests inject a mock. The optional [`Self::base_api_url`]
/// override mirrors TS `getOauthConfig().BASE_API_URL` (defaults to
/// [`DEFAULT_BASE_API_URL`]).
pub trait ClaudeAiAuthProvider: Send + Sync {
    /// Current (refreshed) claude.ai OAuth access token, or `None` when the user
    /// is not authenticated with a claude.ai account.
    fn access_token(&self) -> Option<String>;

    /// Stable organization UUID for the authenticated account, or `None` when it
    /// cannot be resolved.
    fn org_uuid(&self) -> Option<String>;

    /// API base URL the triggers endpoint is built against. Defaults to
    /// [`DEFAULT_BASE_API_URL`]; the host overrides it when it resolves a
    /// non-default base (env / staging).
    fn base_api_url(&self) -> String {
        DEFAULT_BASE_API_URL.to_string()
    }
}

/// `RemoteTriggerTool` — manage scheduled cloud agent routines via the
/// claude.ai CCR API. Drives the network over `ctx.http`.
pub struct RemoteTriggerTool {
    pub(crate) ctx: tool_api::BuiltinToolContext,
    auth: Option<Arc<dyn ClaudeAiAuthProvider>>,
}

impl RemoteTriggerTool {
    /// Construct.
    ///
    /// `auth` is the in-process OAuth resolver. Pass `Some(..)` at the desktop
    /// composition root (backed by the credential store); pass `None` where no
    /// auth backend is wired (mobile WIP, tests that don't exercise the network
    /// path) — in that case the pre-flight "not authenticated" error fires.
    #[must_use]
    pub fn new(
        ctx: tool_api::BuiltinToolContext,
        auth: Option<Arc<dyn ClaudeAiAuthProvider>>,
    ) -> Self {
        Self { ctx, auth }
    }
}

/// Input schema (1:1 with TS `inputSchema`):
/// `{ action: list|get|create|update|run, trigger_id?: /^[\w-]+$/, body?: object }`.
static SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "action": {
                "type": "string",
                "enum": ["list", "get", "create", "update", "run"]
            },
            "trigger_id": {
                "type": "string",
                "pattern": "^[\\w-]+$",
                "description": "Required for get, update, and run"
            },
            "body": {
                "type": "object",
                "description": "Required for create and update; optional for run"
            }
        },
        "required": ["action"]
    })
});

fn verified_str(s: &str) -> AnalyticsValue {
    AnalyticsValue::String(telemetry::pii::Verified::assert_safe(s.to_string()).into_inner())
}

async fn emit_failed(bus: &Arc<AnalyticsBus>, kind: &str, duration_ms: u64) {
    let mut md: LogEventMetadata = HashMap::new();
    md.insert("error_kind".into(), verified_str(kind));
    md.insert(
        "duration_ms".into(),
        AnalyticsValue::Int(duration_ms as i64),
    );
    bus.log_event(REMOTE_TRIGGER_FAILED, md).await;
}

/// `jsonStringify(res.data)` — axios parses a JSON body into an object, so
/// `JSON.stringify` re-serializes it compactly; a non-JSON body is left as a
/// string and stringified (quoted). Mirror that: parse → compact re-serialize;
/// on parse failure wrap the raw body as a JSON string.
fn json_stringify_body(body: &str) -> String {
    match serde_json::from_str::<Value>(body) {
        Ok(v) => serde_json::to_string(&v).unwrap_or_else(|_| body.to_string()),
        Err(_) => serde_json::to_string(&Value::String(body.to_string()))
            .unwrap_or_else(|_| body.to_string()),
    }
}

#[async_trait]
impl Tool for RemoteTriggerTool {
    fn name(&self) -> &str {
        REMOTE_TRIGGER_TOOL_NAME
    }

    fn search_hint(&self) -> Option<&str> {
        Some("manage scheduled cloud agent routines")
    }

    fn input_schema(&self) -> &Value {
        &SCHEMA
    }

    fn is_enabled(&self, _: &ToolStaticContext) -> bool {
        true
    }

    /// TS `shouldDefer: true`.
    fn should_defer(&self) -> bool {
        true
    }

    /// TS `maxResultSizeChars: 100_000`.
    fn max_result_size_chars(&self) -> usize {
        100_000
    }

    /// TS `isConcurrencySafe() { return true }`.
    fn is_concurrency_safe(&self, _: &Value) -> bool {
        true
    }

    /// TS `isReadOnly(input)` — `list` and `get` are read-only.
    fn is_read_only(&self, input: &Value) -> bool {
        matches!(
            input.get("action").and_then(Value::as_str),
            Some("list" | "get")
        )
    }

    fn is_destructive(&self, _: &Value) -> bool {
        false
    }

    /// Reaches the network (claude.ai CCR API).
    fn is_open_world(&self, _: &Value) -> bool {
        true
    }

    fn interrupt_behavior(&self, _: &Value) -> InterruptBehavior {
        InterruptBehavior::Block
    }

    async fn check_permissions(&self, _: &Value, _: &ToolUseContext) -> PermissionResult {
        PermissionResult::Allow {
            reason: PermissionDecisionReason::Other {
                reason: "RemoteTrigger drives the claude.ai CCR API in-process".into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }

    /// TS `DESCRIPTION`.
    async fn description(&self, _: &Value, _: &DescriptionOptions) -> String {
        "Manage scheduled remote LingXi agents (triggers) via the claude.ai CCR API. Auth is handled in-process — the token never reaches the shell.".into()
    }

    /// TS `PROMPT`.
    async fn prompt(&self, _: &PromptOptions) -> String {
        "Call the claude.ai remote-trigger API. Use this instead of curl — the OAuth token is added automatically in-process and never exposed.\n\nActions:\n- list: GET /v1/code/triggers\n- get: GET /v1/code/triggers/{trigger_id}\n- create: POST /v1/code/triggers (requires body)\n- update: POST /v1/code/triggers/{trigger_id} (requires body, partial update)\n- run: POST /v1/code/triggers/{trigger_id}/run\n\nThe response is the raw JSON from the API.".into()
    }

    async fn validate_input(
        &self,
        input: &Value,
        _: &ToolUseContext,
    ) -> Result<(), ValidationError> {
        let action = input
            .get("action")
            .and_then(Value::as_str)
            .ok_or_else(|| ValidationError("RemoteTrigger: missing or non-string action".into()))?;
        if !matches!(action, "list" | "get" | "create" | "update" | "run") {
            return Err(ValidationError(format!(
                "RemoteTrigger: invalid action '{action}'"
            )));
        }
        if let Some(tid) = input.get("trigger_id") {
            let tid = tid.as_str().ok_or_else(|| {
                ValidationError("RemoteTrigger: trigger_id must be a string".into())
            })?;
            if !tid
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
                || tid.is_empty()
            {
                return Err(ValidationError(
                    "RemoteTrigger: trigger_id must match /^[\\w-]+$/".into(),
                ));
            }
        }
        if let Some(body) = input.get("body") {
            if !body.is_object() {
                return Err(ValidationError(
                    "RemoteTrigger: body must be a JSON object".into(),
                ));
            }
        }
        Ok(())
    }

    async fn call(
        &self,
        input: Value,
        _ctx: ToolUseContext,
        _progress: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        let started = Instant::now();
        let bus = self.ctx.bus.clone();

        let action = match input.get("action").and_then(Value::as_str) {
            Some(a) if matches!(a, "list" | "get" | "create" | "update" | "run") => a.to_string(),
            _ => {
                emit_failed(&bus, "invalid_action", started.elapsed().as_millis() as u64).await;
                return Err(ToolError::InvalidInput(
                    "RemoteTrigger: missing or invalid action".into(),
                ));
            }
        };
        let trigger_id = input
            .get("trigger_id")
            .and_then(Value::as_str)
            .map(str::to_string);
        let body = input.get("body").cloned();

        let mut md: LogEventMetadata = HashMap::new();
        md.insert("action".into(), verified_str(&action));
        bus.log_event(REMOTE_TRIGGER_STARTED, md).await;

        // ===== Pre-flight auth (byte-faithful errors) =====
        // TS: checkAndRefreshOAuthTokenIfNeeded(); getClaudeAIOAuthTokens()?.accessToken.
        let access_token = match self.auth.as_ref().and_then(|p| p.access_token()) {
            Some(t) if !t.is_empty() => t,
            _ => {
                emit_failed(&bus, "no_token", started.elapsed().as_millis() as u64).await;
                return Err(ToolError::Internal(
                    "Not authenticated with a claude.ai account. Run /login and try again.".into(),
                ));
            }
        };
        let org_uuid = match self.auth.as_ref().and_then(|p| p.org_uuid()) {
            Some(o) if !o.is_empty() => o,
            _ => {
                emit_failed(&bus, "no_org", started.elapsed().as_millis() as u64).await;
                return Err(ToolError::Internal(
                    "Unable to resolve organization UUID.".into(),
                ));
            }
        };

        let base = format!(
            "{}/v1/code/triggers",
            self.auth
                .as_ref()
                .map_or_else(|| DEFAULT_BASE_API_URL.to_string(), |p| p.base_api_url())
        );

        // ===== Method / URL / body dispatch (1:1 with TS switch) =====
        let (method, url, data): (HttpMethod, String, Option<Value>) = match action.as_str() {
            "list" => (HttpMethod::Get, base.clone(), None),
            "get" => {
                let Some(id) = trigger_id.as_deref() else {
                    emit_failed(
                        &bus,
                        "get_no_trigger_id",
                        started.elapsed().as_millis() as u64,
                    )
                    .await;
                    return Err(ToolError::InvalidInput("get requires trigger_id".into()));
                };
                (HttpMethod::Get, format!("{base}/{id}"), None)
            }
            "create" => {
                let Some(b) = body.clone() else {
                    emit_failed(&bus, "create_no_body", started.elapsed().as_millis() as u64).await;
                    return Err(ToolError::InvalidInput("create requires body".into()));
                };
                (HttpMethod::Post, base.clone(), Some(b))
            }
            "update" => {
                let Some(id) = trigger_id.as_deref() else {
                    emit_failed(
                        &bus,
                        "update_no_trigger_id",
                        started.elapsed().as_millis() as u64,
                    )
                    .await;
                    return Err(ToolError::InvalidInput("update requires trigger_id".into()));
                };
                let Some(b) = body.clone() else {
                    emit_failed(&bus, "update_no_body", started.elapsed().as_millis() as u64).await;
                    return Err(ToolError::InvalidInput("update requires body".into()));
                };
                (HttpMethod::Post, format!("{base}/{id}"), Some(b))
            }
            "run" => {
                let Some(id) = trigger_id.as_deref() else {
                    emit_failed(
                        &bus,
                        "run_no_trigger_id",
                        started.elapsed().as_millis() as u64,
                    )
                    .await;
                    return Err(ToolError::InvalidInput("run requires trigger_id".into()));
                };
                (
                    HttpMethod::Post,
                    format!("{base}/{id}/run"),
                    Some(json!({})),
                )
            }
            // unreachable — `action` validated above.
            _ => unreachable!("action validated"),
        };

        let request_body = data.map(|d| serde_json::to_string(&d).unwrap_or_else(|_| "{}".into()));

        let req = HttpRequest {
            method,
            url,
            headers: vec![
                ("Authorization".into(), format!("Bearer {access_token}")),
                ("Content-Type".into(), "application/json".into()),
                ("anthropic-version".into(), ANTHROPIC_VERSION.into()),
                ("anthropic-beta".into(), TRIGGERS_BETA.into()),
                ("x-organization-uuid".into(), org_uuid),
            ],
            body: request_body,
            body_bytes: None,
            timeout: Some(REQUEST_TIMEOUT),
        };

        // TS `validateStatus: () => true` — every status is a non-error result;
        // the body/status flow into the output unchanged. So a transport-level
        // `HttpError::Status` (non-2xx) is mapped back to a result, not an error.
        let resp = match self.ctx.http.request(req).await {
            Ok(r) => r,
            Err(traits::http::HttpError::Status { status, body }) => protocol::HttpResponse {
                status,
                headers: vec![],
                body,
                body_bytes: Vec::new(),
            },
            Err(e) => {
                emit_failed(&bus, "transport", started.elapsed().as_millis() as u64).await;
                return Err(ToolError::Io(format!(
                    "RemoteTrigger: HTTP transport error: {e}"
                )));
            }
        };

        let status = resp.status;
        let json = json_stringify_body(&resp.body);

        let mut md: LogEventMetadata = HashMap::new();
        md.insert(
            "duration_ms".into(),
            AnalyticsValue::Int(started.elapsed().as_millis() as i64),
        );
        md.insert("status".into(), AnalyticsValue::Int(i64::from(status)));
        bus.log_event(REMOTE_TRIGGER_COMPLETED, md).await;

        // Output `{ status, json }`; the result block renders `HTTP {status}\n{json}`.
        Ok(ToolCallResult {
            data: json!({
                "status": status,
                "json": json,
            }),
            model_content: None,
            new_messages: vec![],
            context_modifier: None,
            is_error: false,
            mcp_meta: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tool_api::test_support::{fresh_ctx, fresh_tx, shell_test_ctx};
    use traits::http::{HttpError, HttpTransport, SseStream};
    use traits::process::ProcessOutput;

    fn dummy_out() -> ProcessOutput {
        ProcessOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        }
    }

    /// Mock auth provider.
    struct MockAuth {
        token: Option<String>,
        org: Option<String>,
        base: String,
    }
    impl MockAuth {
        fn full() -> Self {
            Self {
                token: Some("tok-abc".into()),
                org: Some("org-123".into()),
                base: DEFAULT_BASE_API_URL.into(),
            }
        }
    }
    impl ClaudeAiAuthProvider for MockAuth {
        fn access_token(&self) -> Option<String> {
            self.token.clone()
        }
        fn org_uuid(&self) -> Option<String> {
            self.org.clone()
        }
        fn base_api_url(&self) -> String {
            self.base.clone()
        }
    }

    /// Records the last request and returns a canned response.
    struct RecordingHttp {
        last: Mutex<Option<HttpRequest>>,
        status: u16,
        body: String,
    }
    impl RecordingHttp {
        fn new(status: u16, body: &str) -> Arc<Self> {
            Arc::new(Self {
                last: Mutex::new(None),
                status,
                body: body.into(),
            })
        }
        fn take(&self) -> HttpRequest {
            self.last
                .lock()
                .unwrap()
                .take()
                .expect("a request was made")
        }
    }
    #[async_trait]
    impl HttpTransport for RecordingHttp {
        async fn request(&self, req: HttpRequest) -> Result<protocol::HttpResponse, HttpError> {
            *self.last.lock().unwrap() = Some(req);
            Ok(protocol::HttpResponse {
                status: self.status,
                headers: vec![],
                body: self.body.clone(),
                body_bytes: Vec::new(),
            })
        }
        async fn stream_sse(&self, _: HttpRequest) -> Result<SseStream, HttpError> {
            Err(HttpError::InvalidRequest("no sse".into()))
        }
    }

    fn header<'a>(req: &'a HttpRequest, name: &str) -> Option<&'a str> {
        req.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    fn tool_with(
        http: Arc<dyn HttpTransport>,
        auth: Option<Arc<dyn ClaudeAiAuthProvider>>,
    ) -> RemoteTriggerTool {
        let mut ctx = shell_test_ctx(dummy_out());
        ctx.http = http;
        RemoteTriggerTool::new(ctx, auth)
    }

    #[test]
    fn name_and_schema_locked() {
        assert_eq!(REMOTE_TRIGGER_TOOL_NAME, "RemoteTrigger");
        assert_eq!(SCHEMA["properties"]["action"]["enum"][0], json!("list"));
        assert_eq!(
            SCHEMA["properties"]["trigger_id"]["pattern"],
            json!("^[\\w-]+$")
        );
        assert_eq!(SCHEMA["required"], json!(["action"]));
    }

    #[test]
    fn metadata_matches_ts() {
        let tool = tool_with(RecordingHttp::new(200, "{}"), None);
        assert!(tool.should_defer());
        assert_eq!(tool.max_result_size_chars(), 100_000);
        assert!(tool.is_concurrency_safe(&json!({})));
        assert!(tool.is_read_only(&json!({"action": "list"})));
        assert!(tool.is_read_only(&json!({"action": "get"})));
        assert!(!tool.is_read_only(&json!({"action": "create"})));
        assert!(!tool.is_read_only(&json!({"action": "update"})));
        assert!(!tool.is_read_only(&json!({"action": "run"})));
        assert!(tool.is_open_world(&json!({})));
    }

    #[tokio::test]
    async fn list_builds_get_base_with_headers() {
        let http = RecordingHttp::new(200, r#"[{"id":"t1"}]"#);
        let tool = tool_with(http.clone(), Some(Arc::new(MockAuth::full())));
        let out = tool
            .call(json!({"action": "list"}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        let req = http.take();
        assert_eq!(req.method, HttpMethod::Get);
        assert_eq!(req.url, "https://api.anthropic.com/v1/code/triggers");
        assert!(req.body.is_none());
        assert_eq!(header(&req, "Authorization"), Some("Bearer tok-abc"));
        assert_eq!(header(&req, "Content-Type"), Some("application/json"));
        assert_eq!(header(&req, "anthropic-version"), Some("2023-06-01"));
        assert_eq!(
            header(&req, "anthropic-beta"),
            Some("ccr-triggers-2026-01-30")
        );
        assert_eq!(header(&req, "x-organization-uuid"), Some("org-123"));
        assert_eq!(req.timeout, Some(Duration::from_secs(20)));
        // Output { status, json } and compact re-serialization.
        assert_eq!(out.data["status"], json!(200));
        assert_eq!(out.data["json"], json!(r#"[{"id":"t1"}]"#));
    }

    #[tokio::test]
    async fn get_builds_get_base_id() {
        let http = RecordingHttp::new(200, r#"{"id":"t1"}"#);
        let tool = tool_with(http.clone(), Some(Arc::new(MockAuth::full())));
        tool.call(
            json!({"action": "get", "trigger_id": "t1"}),
            fresh_ctx(),
            fresh_tx(),
        )
        .await
        .expect("ok");
        let req = http.take();
        assert_eq!(req.method, HttpMethod::Get);
        assert_eq!(req.url, "https://api.anthropic.com/v1/code/triggers/t1");
        assert!(req.body.is_none());
    }

    #[tokio::test]
    async fn create_posts_base_with_body() {
        let http = RecordingHttp::new(201, r#"{"id":"new"}"#);
        let tool = tool_with(http.clone(), Some(Arc::new(MockAuth::full())));
        tool.call(
            json!({"action": "create", "body": {"name": "deploy"}}),
            fresh_ctx(),
            fresh_tx(),
        )
        .await
        .expect("ok");
        let req = http.take();
        assert_eq!(req.method, HttpMethod::Post);
        assert_eq!(req.url, "https://api.anthropic.com/v1/code/triggers");
        assert_eq!(req.body.as_deref(), Some(r#"{"name":"deploy"}"#));
    }

    #[tokio::test]
    async fn update_posts_base_id_with_body() {
        let http = RecordingHttp::new(200, r#"{"id":"t1"}"#);
        let tool = tool_with(http.clone(), Some(Arc::new(MockAuth::full())));
        tool.call(
            json!({"action": "update", "trigger_id": "t1", "body": {"schedule": "0 9 * * *"}}),
            fresh_ctx(),
            fresh_tx(),
        )
        .await
        .expect("ok");
        let req = http.take();
        assert_eq!(req.method, HttpMethod::Post);
        assert_eq!(req.url, "https://api.anthropic.com/v1/code/triggers/t1");
        assert_eq!(req.body.as_deref(), Some(r#"{"schedule":"0 9 * * *"}"#));
    }

    #[tokio::test]
    async fn run_posts_base_id_run_with_empty_body() {
        let http = RecordingHttp::new(202, r#"{"queued":true}"#);
        let tool = tool_with(http.clone(), Some(Arc::new(MockAuth::full())));
        let out = tool
            .call(
                json!({"action": "run", "trigger_id": "t1"}),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("ok");
        let req = http.take();
        assert_eq!(req.method, HttpMethod::Post);
        assert_eq!(req.url, "https://api.anthropic.com/v1/code/triggers/t1/run");
        assert_eq!(req.body.as_deref(), Some("{}"));
        assert_eq!(out.data["status"], json!(202));
        assert_eq!(out.data["json"], json!(r#"{"queued":true}"#));
    }

    #[tokio::test]
    async fn validate_status_true_non_2xx_is_a_result() {
        // 404 must NOT error — it flows into { status, json } (TS validateStatus).
        let http = RecordingHttp::new(404, r#"{"error":"not found"}"#);
        let tool = tool_with(http, Some(Arc::new(MockAuth::full())));
        let out = tool
            .call(
                json!({"action": "get", "trigger_id": "missing"}),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("404 is a result, not an error");
        assert_eq!(out.data["status"], json!(404));
        assert_eq!(out.data["json"], json!(r#"{"error":"not found"}"#));
    }

    #[tokio::test]
    async fn get_requires_trigger_id() {
        let tool = tool_with(
            RecordingHttp::new(200, "{}"),
            Some(Arc::new(MockAuth::full())),
        );
        let err = tool
            .call(json!({"action": "get"}), fresh_ctx(), fresh_tx())
            .await
            .expect_err("missing trigger_id");
        assert!(format!("{err}").contains("get requires trigger_id"));
    }

    #[tokio::test]
    async fn create_requires_body() {
        let tool = tool_with(
            RecordingHttp::new(200, "{}"),
            Some(Arc::new(MockAuth::full())),
        );
        let err = tool
            .call(json!({"action": "create"}), fresh_ctx(), fresh_tx())
            .await
            .expect_err("missing body");
        assert!(format!("{err}").contains("create requires body"));
    }

    #[tokio::test]
    async fn update_requires_trigger_id_and_body() {
        let tool = tool_with(
            RecordingHttp::new(200, "{}"),
            Some(Arc::new(MockAuth::full())),
        );
        let err = tool
            .call(
                json!({"action": "update", "body": {}}),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect_err("missing trigger_id");
        assert!(format!("{err}").contains("update requires trigger_id"));
        let err = tool
            .call(
                json!({"action": "update", "trigger_id": "t1"}),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect_err("missing body");
        assert!(format!("{err}").contains("update requires body"));
    }

    #[tokio::test]
    async fn run_requires_trigger_id() {
        let tool = tool_with(
            RecordingHttp::new(200, "{}"),
            Some(Arc::new(MockAuth::full())),
        );
        let err = tool
            .call(json!({"action": "run"}), fresh_ctx(), fresh_tx())
            .await
            .expect_err("missing trigger_id");
        assert!(format!("{err}").contains("run requires trigger_id"));
    }

    #[tokio::test]
    async fn no_token_preflight_error_is_byte_faithful() {
        // No auth provider at all.
        let tool = tool_with(RecordingHttp::new(200, "{}"), None);
        let err = tool
            .call(json!({"action": "list"}), fresh_ctx(), fresh_tx())
            .await
            .expect_err("no token");
        assert_eq!(
            format!("{err}"),
            "internal: Not authenticated with a claude.ai account. Run /login and try again."
        );

        // Provider present but token empty/None.
        let auth: Arc<dyn ClaudeAiAuthProvider> = Arc::new(MockAuth {
            token: None,
            org: Some("org".into()),
            base: DEFAULT_BASE_API_URL.into(),
        });
        let tool = tool_with(RecordingHttp::new(200, "{}"), Some(auth));
        let err = tool
            .call(json!({"action": "list"}), fresh_ctx(), fresh_tx())
            .await
            .expect_err("no token");
        assert!(format!("{err}")
            .contains("Not authenticated with a claude.ai account. Run /login and try again."));
    }

    #[tokio::test]
    async fn no_org_preflight_error_is_byte_faithful() {
        let auth: Arc<dyn ClaudeAiAuthProvider> = Arc::new(MockAuth {
            token: Some("tok".into()),
            org: None,
            base: DEFAULT_BASE_API_URL.into(),
        });
        let tool = tool_with(RecordingHttp::new(200, "{}"), Some(auth));
        let err = tool
            .call(json!({"action": "list"}), fresh_ctx(), fresh_tx())
            .await
            .expect_err("no org");
        assert_eq!(
            format!("{err}"),
            "internal: Unable to resolve organization UUID."
        );
    }

    #[tokio::test]
    async fn non_json_body_is_quoted() {
        // axios leaves a non-JSON body as a string; jsonStringify quotes it.
        let http = RecordingHttp::new(200, "plain text");
        let tool = tool_with(http, Some(Arc::new(MockAuth::full())));
        let out = tool
            .call(json!({"action": "list"}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        assert_eq!(out.data["json"], json!("\"plain text\""));
    }
}
