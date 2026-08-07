//! Same-process MCP provider for mobile local apps.
//!
//! The provider is intentionally the only MCP server registered by the mobile
//! composition root. It accepts only `McpTransportSpec::InProcess` with the
//! fixed `local_apps` registry key, never reads `.mcp.json`, and exposes no
//! stdio or remote transport surface.

use async_trait::async_trait;
use client_protocol::local_apps::builtin_app_templates;
use local_apps::AppService;
use protocol::McpConnectionId;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use traits::{
    ElicitRequestDto, ElicitResultDto, McpError, McpNotificationStream, McpPromptDto,
    McpRawConnection, McpResourceContentDto, McpResourceDto, McpToolDto, McpToolResultDto,
    McpTransport, McpTransportKind, McpTransportSpec, ServerCapabilitiesDto,
};

pub const LOCAL_APPS_REGISTRY_KEY: &str = "local_apps";
const MAX_INPUT_BYTES: usize = 256 * 1024;

/// Host operations that are deliberately outside the catalog state machine.
///
/// Data mutations, UI control, runtime process changes and Git restoration all
/// cross additional trust/lifecycle boundaries. The provider delegates those
/// operations to this host-owned broker rather than acquiring filesystem,
/// WebView or process handles itself.
#[async_trait]
pub trait LocalAppsMcpHost: Send + Sync {
    async fn manage_runtime(&self, input: Value) -> Result<Value, String>;
    async fn query_data(&self, input: Value) -> Result<Value, String>;
    async fn mutate_data(&self, input: Value) -> Result<Value, String>;
    async fn inspect_ui(&self, input: Value) -> Result<Value, String>;
    async fn act_on_ui(&self, input: Value) -> Result<Value, String>;
    async fn restore_checkpoint(&self, input: Value) -> Result<Value, String>;
}

/// Mobile-local implementation of the MCP transport boundary.
pub struct LocalAppsMcpTransport {
    root: PathBuf,
    service: OnceLock<Arc<AppService>>,
    host: OnceLock<Arc<dyn LocalAppsMcpHost>>,
    connections: StdMutex<HashSet<McpConnectionId>>,
}

impl LocalAppsMcpTransport {
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            service: OnceLock::new(),
            host: OnceLock::new(),
            connections: StdMutex::new(HashSet::new()),
        }
    }

    pub fn attach_service(&self, service: Arc<AppService>) -> Result<(), Arc<AppService>> {
        self.service.set(service)
    }

    pub fn attach_host(
        &self,
        host: Arc<dyn LocalAppsMcpHost>,
    ) -> Result<(), Arc<dyn LocalAppsMcpHost>> {
        self.host.set(host)
    }

    fn service(&self) -> Result<&Arc<AppService>, McpError> {
        self.service.get().ok_or_else(|| {
            McpError::Internal("local apps service is still starting; retry shortly".into())
        })
    }

    fn host(&self) -> Result<&Arc<dyn LocalAppsMcpHost>, McpError> {
        self.host.get().ok_or_else(|| {
            McpError::Internal(
                "local apps host capability is unavailable in this build; no state was changed"
                    .into(),
            )
        })
    }

    fn ensure_connection(&self, conn: &McpRawConnection) -> Result<(), McpError> {
        let connections = self
            .connections
            .lock()
            .map_err(|_| McpError::Internal("local apps connection registry poisoned".into()))?;
        if connections.contains(&conn.connection_id) {
            Ok(())
        } else {
            Err(McpError::Connection(
                "local apps MCP connection is no longer active".into(),
            ))
        }
    }

    fn validate_input(input: &Value) -> Result<(), McpError> {
        let size = serde_json::to_vec(input)
            .map_err(|error| McpError::Internal(format!("invalid tool input: {error}")))?
            .len();
        if size > MAX_INPUT_BYTES {
            return Err(McpError::Internal(format!(
                "tool input is {size} bytes; limit is {MAX_INPUT_BYTES}"
            )));
        }
        if !input.is_object() {
            return Err(McpError::Internal(
                "tool input must be a JSON object".into(),
            ));
        }
        Ok(())
    }

    fn required_string<'a>(input: &'a Value, field: &str) -> Result<&'a str, McpError> {
        input
            .get(field)
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| McpError::Internal(format!("missing non-empty {field:?}")))
    }

    fn result(value: Value) -> McpToolResultDto {
        let text = serde_json::to_string(&value).unwrap_or_else(|_| "{}".into());
        McpToolResultDto {
            content: json!([{ "type": "text", "text": text }]),
            is_error: false,
            structured_content: Some(value),
            ..Default::default()
        }
    }

    fn tool_error(message: impl Into<String>) -> McpToolResultDto {
        McpToolResultDto {
            content: json!([{ "type": "text", "text": message.into() }]),
            is_error: true,
            ..Default::default()
        }
    }

    fn app_error(error: local_apps::AppError) -> McpToolResultDto {
        let code = serde_json::to_value(error.code())
            .ok()
            .and_then(|value| value.as_str().map(ToOwned::to_owned))
            .unwrap_or_else(|| "unknown".into());
        Self::tool_error(format!(
            "local apps request failed ({}): {}. Refresh app details and retry with the latest revision.",
            code,
            error
        ))
    }

    fn tool(name: &str, description: &str, input_schema: Value) -> McpToolDto {
        McpToolDto {
            server_name: String::new(),
            tool_name: name.into(),
            description: description.into(),
            input_schema,
            full_name: String::new(),
            search_hint: Some("local app".into()),
            always_load: Some(true),
        }
    }

    fn tool_catalog() -> Vec<McpToolDto> {
        let app_id = json!({ "type": "string", "pattern": "^[a-z0-9][a-z0-9-]{0,63}$" });
        vec![
            Self::tool(
                "list",
                "List local apps and their workflow state. Read-only; use before get or runtime actions. The page is bounded by `limit` (default 50, max 100); when `has_more` is true, narrow with `query`.",
                json!({"type":"object","properties":{"query":{"type":"string","maxLength":200},"limit":{"type":"integer","minimum":1,"maximum":100}}}),
            ),
            Self::tool(
                "get",
                "Get one local app's record, design draft, runtime and checkpoints. Read-only.",
                json!({"type":"object","properties":{"app_id":app_id.clone()},"required":["app_id"],"additionalProperties":false}),
            ),
            Self::tool(
                "create",
                "Create a local app record. The app starts in authoring_questionnaire while its design questionnaire is set up; wait for that to complete before opening the design wizard. This never confirms the design or starts generation.",
                json!({"type":"object","properties":{"name":{"type":"string","minLength":1,"maxLength":200},"template":{"enum":["dashboard","crud_tracker","content_showcase","form_utility"]},"conversation_id":{"type":"string","maxLength":128}},"required":["name","template"],"additionalProperties":false}),
            ),
            Self::tool(
                "propose_design",
                "Propose a structured design patch for explicit user review. The patch is stored but not applied.",
                json!({"type":"object","properties":{"app_id":app_id.clone(),"patch":{"type":"object","properties":{"ops":{"type":"array","maxItems":64,"items":{"type":"object","properties":{"op":{"enum":["set","remove"]},"field_id":{"type":"string","maxLength":64},"value":{"type":"object"}},"required":["op","field_id"]}},"note":{"type":"string","maxLength":2000}},"required":["ops"]}},"required":["app_id","patch"],"additionalProperties":false}),
            ),
            Self::tool(
                "manage_runtime",
                "Start, stop, restart, open, suspend or resume a generated app through the host runtime manager.",
                json!({"type":"object","properties":{"app_id":app_id.clone(),"action":{"enum":["start","stop","restart","open","suspend","resume"]}},"required":["app_id","action"],"additionalProperties":false}),
            ),
            Self::tool(
                "query_data",
                "Query one declared app collection with bounded pagination, sorting and structured filters. Raw SQL is never accepted.",
                json!({
                    "type":"object",
                    "properties":{
                        "app_id":app_id.clone(),
                        "collection":{"type":"string","minLength":1,"maxLength":100},
                        "limit":{"type":"integer","minimum":1,"maximum":100},
                        "cursor":{"type":"string"},
                        "filter":{"type":"object"},
                        "filters":{"type":"array","maxItems":16},
                        "sort":{
                            "oneOf":[
                                {"type":"string","minLength":1,"maxLength":100},
                                {
                                    "type":"object",
                                    "properties":{
                                        "key":{"type":"string","minLength":1,"maxLength":100},
                                        "kind":{"enum":["record_id","created_at","updated_at","revision","field"]},
                                        "field_id":{"type":"string","minLength":1,"maxLength":100},
                                        "direction":{"enum":["ascending","descending","asc","desc"]}
                                    },
                                    "additionalProperties":true
                                }
                            ]
                        },
                        "sort_key":{
                            "oneOf":[
                                {"type":"string","minLength":1,"maxLength":100},
                                {
                                    "type":"object",
                                    "properties":{
                                        "kind":{"enum":["record_id","created_at","updated_at","revision","field"]},
                                        "field_id":{"type":"string","minLength":1,"maxLength":100}
                                    },
                                    "additionalProperties":true
                                }
                            ]
                        },
                        "sort_direction":{"enum":["ascending","descending","asc","desc"]}
                    },
                    "required":["app_id","collection"],
                    "additionalProperties":false
                }),
            ),
            Self::tool(
                "mutate_data",
                "Create, update or delete up to 50 records in one declared collection. First use requires a user capability grant.",
                json!({"type":"object","properties":{"app_id":app_id.clone(),"collection":{"type":"string","minLength":1,"maxLength":100},"operations":{"type":"array","minItems":1,"maxItems":50}},"required":["app_id","collection","operations"],"additionalProperties":false}),
            ),
            Self::tool(
                "inspect_ui",
                "Inspect the structured accessibility/DOM snapshot of a running local app. Never executes JavaScript.",
                json!({"type":"object","properties":{"app_id":app_id.clone(),"selector":{"type":"string","maxLength":500}},"required":["app_id"],"additionalProperties":false}),
            ),
            Self::tool(
                "act_on_ui",
                "Perform one structured UI action. Allowed actions are click, fill, select, toggle, scroll, navigate, back and reload; arbitrary JavaScript is rejected.",
                json!({
                    "type":"object",
                    "properties":{
                        "app_id":app_id.clone(),
                        "action":{"enum":["click","fill","select","toggle","scroll","navigate","back","reload"]},
                        "target":{
                            "oneOf":[
                                {"type":"string","maxLength":500},
                                {
                                    "type":"object",
                                    "properties":{
                                        "element_id":{"type":"string","maxLength":500},
                                        "role":{"type":"string","maxLength":100},
                                        "name":{"type":"string","maxLength":500}
                                    },
                                    "additionalProperties":false
                                }
                            ]
                        },
                        "value":{"type":["string","number","boolean"]}
                    },
                    "required":["app_id","action"],
                    "additionalProperties":false
                }),
            ),
            Self::tool(
                "read_logs",
                "Read a bounded tail of an app-owned log file. Paths cannot escape the app logs directory.",
                json!({"type":"object","properties":{"app_id":app_id.clone(),"log":{"enum":["generation","build","runtime"]},"max_bytes":{"type":"integer","minimum":1,"maximum":65536}},"required":["app_id"],"additionalProperties":false}),
            ),
            Self::tool(
                "list_checkpoints",
                "List Git-backed code checkpoints for one app. Read-only and does not affect SQLite data.",
                json!({"type":"object","properties":{"app_id":app_id.clone()},"required":["app_id"],"additionalProperties":false}),
            ),
            Self::tool(
                "restore_checkpoint",
                "Request restoration of an app code checkpoint. Every call requires explicit user confirmation and never rolls back app data.",
                json!({"type":"object","properties":{"app_id":app_id,"checkpoint_id":{"type":"string","minLength":1,"maxLength":128}},"required":["app_id","checkpoint_id"],"additionalProperties":false}),
            ),
        ]
    }

    async fn call(&self, tool: &str, input: Value) -> Result<McpToolResultDto, McpError> {
        Self::validate_input(&input)?;
        let service = self.service()?;
        let result = match tool {
            "list" => {
                let query = input
                    .get("query")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty());
                let limit = input
                    .get("limit")
                    .and_then(Value::as_u64)
                    .unwrap_or(50)
                    .clamp(1, 100) as usize;
                let mut apps = service.list_apps().await;
                if let Some(query) = query {
                    apps.retain(|app| app.name.to_lowercase().contains(&query.to_lowercase()));
                }
                let total = apps.len();
                apps.truncate(limit);
                Self::result(json!({
                    "apps": apps,
                    "count": apps.len(),
                    "total": total,
                    "has_more": total > apps.len(),
                    "templates": builtin_app_templates()
                }))
            }
            "get" => {
                let app_id = Self::required_string(&input, "app_id")?;
                let record = match service.record(app_id).await {
                    Ok(value) => value,
                    Err(error) => return Ok(Self::app_error(error)),
                };
                let draft = service.draft(app_id).await.map_err(|error| {
                    McpError::Internal(format!("failed to read design draft: {error}"))
                })?;
                let runtime = service.runtime_record(app_id).await.map_err(|error| {
                    McpError::Internal(format!("failed to read runtime: {error}"))
                })?;
                let checkpoints = service.list_checkpoints(app_id).await.map_err(|error| {
                    McpError::Internal(format!("failed to list checkpoints: {error}"))
                })?;
                Self::result(json!({
                    "app": record,
                    "design": draft,
                    "runtime": runtime,
                    "checkpoints": checkpoints
                }))
            }
            "create" => {
                let name = Self::required_string(&input, "name")?;
                // TODO(local-apps#questionnaire, Task 10): the tool schema
                // below still advertises/requires a `template` enum for
                // backward input compatibility, but the core no longer has a
                // template concept — `AppRecord`/`AppService::create_app` now
                // take a free-text `brief` instead. Task 10 rewrites this
                // schema (and the request shape) around the conversational
                // design flow to accept a real brief. Until then `template`
                // (if sent) is accepted and ignored, and `name` doubles as
                // the brief. This is DELIBERATELY pinned, not silent:
                // `create_persists_name_as_brief_until_task_10_adds_a_real_one`
                // below asserts `AppRecord.brief == name` and will fail the
                // moment this changes — Task 10 cannot land a real brief
                // field without that test forcing it to touch this comment
                // and this call.

                // TODO(local-apps#questionnaire, Task 10): a fresh app now
                // starts in `authoring_questionnaire` (Task 3), not
                // `collecting_spec` — `open_designer` requires
                // `collecting_spec | generation_failed` and would refuse it.
                // This tool used to open the designer gate immediately after
                // create; that step is gone until Task 4/8 wire the
                // questionnaire-authoring LLM round trip that carries the app
                // to `collecting_spec`. The tool description above was
                // updated to match (no more "open its human design wizard"
                // promise); Task 10 still owns the real conversational
                // rewrite of this tool's shape around that round trip.
                let conversation_id = input
                    .get("conversation_id")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned);
                let record = match service.create_app(Some(name), name, conversation_id).await {
                    Ok(record) => record,
                    Err(error) => return Ok(Self::app_error(error)),
                };
                Self::result(json!({
                    "app": record,
                    "next_step": "The app is being set up; wait for its questionnaire before designing."
                }))
            }
            "propose_design" => {
                let app_id = Self::required_string(&input, "app_id")?;
                // Patch ops cross the seam through `raise_patch`: this wire is
                // protocol snake_case `field_id`, the core persists camelCase
                // `fieldId` (client-protocol/src/local_apps.rs module doc).
                let patch: client_protocol::local_apps::AppDesignPatchDto =
                    serde_json::from_value(input.get("patch").cloned().unwrap_or(Value::Null))
                        .map_err(|error| {
                            McpError::Internal(format!("invalid design patch: {error}"))
                        })?;
                let patch = match crate::local_apps_bridge::raise_patch(patch) {
                    Ok(patch) => patch,
                    Err(error) => return Ok(Self::app_error(error)),
                };
                match service.store_suggestion(app_id, patch).await {
                    Ok(suggestion) => Self::result(json!({
                        "suggestion": suggestion,
                        "applied": false,
                        "next_step": "The user must explicitly apply or dismiss this suggestion."
                    })),
                    Err(error) => Self::app_error(error),
                }
            }
            "manage_runtime" => match self.host()?.manage_runtime(input).await {
                Ok(value) => Self::result(value),
                Err(message) => Self::tool_error(message),
            },
            "query_data" => match self.host()?.query_data(input).await {
                Ok(value) => Self::result(value),
                Err(message) => Self::tool_error(message),
            },
            "mutate_data" => match self.host()?.mutate_data(input).await {
                Ok(value) => Self::result(value),
                Err(message) => Self::tool_error(message),
            },
            "inspect_ui" => match self.host()?.inspect_ui(input).await {
                Ok(value) => Self::result(value),
                Err(message) => Self::tool_error(message),
            },
            "act_on_ui" => match self.host()?.act_on_ui(input).await {
                Ok(value) => Self::result(value),
                Err(message) => Self::tool_error(message),
            },
            "read_logs" => {
                let app_id = Self::required_string(&input, "app_id")?;
                local_apps::ids::validate_app_id(app_id)
                    .map_err(|error| McpError::Internal(error.to_string()))?;
                service
                    .record(app_id)
                    .await
                    .map_err(|error| McpError::Internal(error.to_string()))?;
                let log = input
                    .get("log")
                    .and_then(Value::as_str)
                    .unwrap_or("runtime");
                if !matches!(log, "generation" | "build" | "runtime") {
                    return Ok(Self::tool_error(
                        "log must be generation, build, or runtime",
                    ));
                }
                let max_bytes = input
                    .get("max_bytes")
                    .and_then(Value::as_u64)
                    .unwrap_or(16_384)
                    .clamp(1, 65_536) as usize;
                let relative = PathBuf::from("apps")
                    .join(app_id)
                    .join("logs")
                    .join(format!("{log}.log"));
                let root = self.root.clone();
                let body = match tokio::task::spawn_blocking(move || {
                    traits::rooted_fs::read_to_string_limited(&root, &relative, 16 * 1024 * 1024)
                })
                .await
                .map_err(|error| McpError::Internal(format!("log reader failed: {error}")))?
                {
                    Ok(body) => body,
                    Err(traits::FsError::NotFound(_)) => String::new(),
                    Err(error) => {
                        return Ok(Self::tool_error(format!("failed to read app log: {error}")))
                    }
                };
                let mut start = body.len().saturating_sub(max_bytes);
                while start < body.len() && !body.is_char_boundary(start) {
                    start += 1;
                }
                let tail = &body[start..];
                Self::result(json!({
                    "app_id": app_id,
                    "log": log,
                    "tail": tail,
                    "truncated": start > 0
                }))
            }
            "list_checkpoints" => {
                let app_id = Self::required_string(&input, "app_id")?;
                match service.list_checkpoints(app_id).await {
                    Ok(checkpoints) => Self::result(json!({"checkpoints": checkpoints})),
                    Err(error) => Self::app_error(error),
                }
            }
            "restore_checkpoint" => match self.host()?.restore_checkpoint(input).await {
                Ok(value) => Self::result(value),
                Err(message) => Self::tool_error(message),
            },
            _ => return Err(McpError::ToolNotFound(tool.into())),
        };
        Ok(result)
    }
}

#[async_trait]
impl McpTransport for LocalAppsMcpTransport {
    async fn connect(&self, spec: &McpTransportSpec) -> Result<McpRawConnection, McpError> {
        match spec {
            McpTransportSpec::InProcess { registry_key }
                if registry_key == LOCAL_APPS_REGISTRY_KEY => {}
            other => return Err(McpError::UnsupportedTransport(other.transport_kind())),
        }
        let connection_id = McpConnectionId::new();
        self.connections
            .lock()
            .map_err(|_| McpError::Internal("local apps connection registry poisoned".into()))?
            .insert(connection_id);
        Ok(McpRawConnection { connection_id })
    }

    async fn initialize(&self, conn: &McpRawConnection) -> Result<ServerCapabilitiesDto, McpError> {
        self.ensure_connection(conn)?;
        Ok(ServerCapabilitiesDto {
            tools: true,
            resources: false,
            prompts: false,
            logging: false,
            experimental: std::collections::HashMap::new(),
        })
    }

    async fn list_tools(&self, conn: &McpRawConnection) -> Result<Vec<McpToolDto>, McpError> {
        self.ensure_connection(conn)?;
        Ok(Self::tool_catalog())
    }

    async fn list_resources(
        &self,
        conn: &McpRawConnection,
    ) -> Result<Vec<McpResourceDto>, McpError> {
        self.ensure_connection(conn)?;
        Ok(Vec::new())
    }

    async fn list_prompts(&self, conn: &McpRawConnection) -> Result<Vec<McpPromptDto>, McpError> {
        self.ensure_connection(conn)?;
        Ok(Vec::new())
    }

    async fn call_tool(
        &self,
        conn: &McpRawConnection,
        tool: &str,
        input: Value,
    ) -> Result<McpToolResultDto, McpError> {
        self.ensure_connection(conn)?;
        self.call(tool, input).await
    }

    async fn read_resource(
        &self,
        _conn: &McpRawConnection,
        _uri: &str,
    ) -> Result<McpResourceContentDto, McpError> {
        Err(McpError::Internal(
            "local apps exposes tools only; resources are disabled".into(),
        ))
    }

    async fn ping(&self, connection_id: McpConnectionId) -> Result<(), McpError> {
        self.ensure_connection(&McpRawConnection { connection_id })
    }

    async fn notifications(
        &self,
        conn: &McpRawConnection,
    ) -> Result<McpNotificationStream, McpError> {
        self.ensure_connection(conn)?;
        Ok(Box::pin(futures_util::stream::empty()))
    }

    async fn handle_elicitation(
        &self,
        _conn: &McpRawConnection,
        _request: ElicitRequestDto,
    ) -> Result<ElicitResultDto, McpError> {
        Err(McpError::Internal(
            "local apps approvals are resolved through client protocol events".into(),
        ))
    }

    async fn disconnect(&self, connection_id: McpConnectionId) -> Result<(), McpError> {
        self.connections
            .lock()
            .map_err(|_| McpError::Internal("local apps connection registry poisoned".into()))?
            .remove(&connection_id);
        Ok(())
    }

    fn supported_transports(&self) -> Vec<McpTransportKind> {
        vec![McpTransportKind::InProcess]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_is_fixed_and_exposes_no_arbitrary_execution_surface() {
        let tools = LocalAppsMcpTransport::tool_catalog();
        let names: Vec<_> = tools.iter().map(|tool| tool.tool_name.as_str()).collect();
        assert_eq!(
            names,
            [
                "list",
                "get",
                "create",
                "propose_design",
                "manage_runtime",
                "query_data",
                "mutate_data",
                "inspect_ui",
                "act_on_ui",
                "read_logs",
                "list_checkpoints",
                "restore_checkpoint",
            ]
        );
        let schemas = tools
            .iter()
            .map(|tool| tool.input_schema.to_string())
            .collect::<String>()
            .to_lowercase();
        assert!(!schemas.contains("sql"));
        assert!(!schemas.contains("javascript"));
        assert!(!schemas.contains("package_manager"));
        assert!(schemas.contains("click"));
        assert!(schemas.contains("reload"));
    }

    async fn attached_transport(
        root: &std::path::Path,
    ) -> (LocalAppsMcpTransport, Arc<AppService>) {
        let transport = LocalAppsMcpTransport::new(root.to_path_buf());
        let service = Arc::new(
            AppService::load(
                root,
                Arc::new(local_apps::test_support::FixedClock::new(1)),
                Arc::new(local_apps::NoopContinuationSink),
                Arc::new(local_apps::NoopAppEventObserver),
            )
            .await
            .expect("load app service"),
        );
        assert!(transport.attach_service(Arc::clone(&service)).is_ok());
        (transport, service)
    }

    /// PINS the gap the `create` handler's TODO names: until Task 10 gives
    /// the `create` tool a real `brief` input, `AppRecord.brief` is exactly
    /// `name` — not a template tag, not empty, not anything else. This is
    /// deliberately a strong equality assertion (not "is non-empty" or "is
    /// present") so ANY future change to what `create` persists as `brief` —
    /// whether Task 10 wires a real one or someone quietly "improves" this
    /// call — fails this test and forces a conscious look at the comment
    /// above `service.create_app(Some(name), name, conversation_id)`, instead of
    /// silently shipping a still-wrong value.
    #[tokio::test]
    async fn create_persists_name_as_brief_until_task_10_adds_a_real_one() {
        let root = tempfile::tempdir().unwrap();
        let (transport, _service) = attached_transport(root.path()).await;
        let created = transport
            .call(
                "create",
                json!({"name": "Habit Tracker", "template": "dashboard"}),
            )
            .await
            .expect("create");
        let app = &created.structured_content.expect("structured")["app"];
        assert_eq!(app["name"], "Habit Tracker");
        assert_eq!(
            app["brief"], "Habit Tracker",
            "until Task 10 adds a real brief input, `create` must persist `name` as `brief` \
             verbatim — not a template tag, not empty, not silently something else"
        );
    }

    #[tokio::test]
    async fn propose_design_accepts_the_protocol_snake_case_patch_wire() {
        let root = tempfile::tempdir().unwrap();
        let (transport, service) = attached_transport(root.path()).await;
        let created = transport
            .call(
                "create",
                json!({"name": "Habits", "template": "dashboard"}),
            )
            .await
            .expect("create");
        let app_id = created.structured_content.expect("structured")["app"]["id"]
            .as_str()
            .expect("app id")
            .to_string();
        // `store_suggestion` needs a draft-editable state; a fresh app
        // starts in `authoring_questionnaire` (Task 3) until Task 4/8 wire
        // the real questionnaire-authoring round trip.
        local_apps::test_support::advance_to_collecting_spec(&service, &app_id).await;

        let result = transport
            .call(
                "propose_design",
                json!({
                    "app_id": app_id,
                    "patch": {"ops": [{
                        "op": "set",
                        "field_id": "pages",
                        "value": {"kind": "screen_list", "value": ["Home"]}
                    }]}
                }),
            )
            .await
            .expect("propose_design accepts the protocol wire");
        assert!(!result.is_error);
        assert!(result.structured_content.expect("structured")["suggestion"].is_object());
    }

    #[tokio::test]
    async fn list_reports_truncation_instead_of_claiming_a_complete_library() {
        let root = tempfile::tempdir().unwrap();
        let (transport, _service) = attached_transport(root.path()).await;
        for index in 0..3 {
            transport
                .call(
                    "create",
                    json!({"name": format!("App {index}"), "template": "dashboard"}),
                )
                .await
                .expect("create");
        }

        let page = transport
            .call("list", json!({"limit": 2}))
            .await
            .expect("list")
            .structured_content
            .expect("structured");
        assert_eq!(page["count"], 2);
        assert_eq!(page["total"], 3);
        assert_eq!(page["has_more"], true);

        let whole = transport
            .call("list", json!({"limit": 100}))
            .await
            .expect("list")
            .structured_content
            .expect("structured");
        assert_eq!(whole["count"], 3);
        assert_eq!(whole["has_more"], false);
    }

    #[tokio::test]
    async fn transport_rejects_every_non_inprocess_spec() {
        let root = tempfile::tempdir().unwrap();
        let transport = LocalAppsMcpTransport::new(root.path().to_path_buf());
        let spec = McpTransportSpec::Stdio {
            command: "local-apps".into(),
            args: Vec::new(),
            env: std::collections::HashMap::new(),
        };
        assert!(matches!(
            transport.connect(&spec).await,
            Err(McpError::UnsupportedTransport(McpTransportKind::Stdio))
        ));
    }
}
