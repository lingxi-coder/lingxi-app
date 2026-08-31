//! Shared remote MCP transport for mobile and desktop platforms.
//!
//! This module owns the HTTP/SSE JSON-RPC connection and the MCP protocol
//! state machine.  Process creation and child reaping intentionally stay in
//! the platform-specific stdio transport; callers only need to retain this
//! value and use [`RemoteMcpTransport::connection_for`] to bridge a live
//! connection into `mcp::McpClient`.

use async_trait::async_trait;
use jsonrpc::{Connection, ConnectionError, InboundHandler, Request, Response, RouterError};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use traits::{
    ElicitRequestDto, ElicitResultDto, McpConnectOptions, McpConnectResult, McpError,
    McpNegotiatedProtocol, McpNotificationDto, McpNotificationStream, McpPromptDto, McpProtocolEra,
    McpRawConnection, McpResourceContentDto, McpResourceDto, McpResourceTemplateDto, McpToolDto,
    McpToolResultDto, McpTransport, McpTransportKind, McpTransportSpec, ServerCapabilitiesDto,
};

/// Legacy MCP protocol revision used when modern negotiation is unavailable.
pub const MCP_PROTOCOL_VERSION: &str = "2025-11-25";
/// Modern protocol revision used by the discovery probe.
pub const MODERN_PROTOCOL_VERSION: &str = "2026-07-28";
const CLIENT_DESCRIPTION: &str = "An agentic coding tool";
const MCP_CLIENT_NAME: &str = "lingxi";
const MCP_CLIENT_TITLE: &str = "LingXi";
const MCP_WEBSITE_URL: &str = "https://claude.com/claude-code";
const MCP_SKILLS_EXTENSION_KEY: &str = "io.modelcontextprotocol/skills";
const MAX_PROBE_TIMEOUT_MS: u64 = 5_000;

fn bounded_probe_timeout_ms(requested: Option<u64>) -> u64 {
    requested
        .unwrap_or(MAX_PROBE_TIMEOUT_MS)
        .min(MAX_PROBE_TIMEOUT_MS)
}

/// Shared remote HTTP/SSE MCP transport.
#[derive(Default)]
pub struct RemoteMcpTransport {
    connections: Arc<Mutex<HashMap<protocol::McpConnectionId, Arc<Connection>>>>,
    negotiated: Arc<Mutex<HashMap<protocol::McpConnectionId, McpNegotiatedProtocol>>>,
}

/// Synchronous cleanup for a cancelled connect/initialize future. The registry
/// wraps transport operations in a timeout; dropping that future must not leave
/// an HTTP broker (or an SSE GET) retained in the connection map.
struct RemoteConnectionCleanupGuard<'a> {
    transport: &'a RemoteMcpTransport,
    id: protocol::McpConnectionId,
    armed: bool,
}

impl RemoteConnectionCleanupGuard<'_> {
    fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for RemoteConnectionCleanupGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.transport.remove(self.id);
        }
    }
}

impl RemoteMcpTransport {
    /// Construct an empty remote transport.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Return the live JSON-RPC connection owned by this transport.
    #[must_use]
    pub fn connection_for(&self, id: protocol::McpConnectionId) -> Option<Arc<Connection>> {
        self.connections.lock().ok()?.get(&id).cloned()
    }

    fn negotiated_for(&self, id: protocol::McpConnectionId) -> Option<McpNegotiatedProtocol> {
        self.negotiated.lock().ok()?.get(&id).cloned()
    }

    fn remove(&self, id: protocol::McpConnectionId) {
        if let Ok(mut connections) = self.connections.lock() {
            if let Some(connection) = connections.remove(&id) {
                connection.close();
            }
        }
        if let Ok(mut negotiated) = self.negotiated.lock() {
            negotiated.remove(&id);
        }
    }

    fn remaining(deadline: tokio::time::Instant) -> Option<Duration> {
        deadline.checked_duration_since(tokio::time::Instant::now())
    }

    fn decorate_params(&self, id: protocol::McpConnectionId, method: &str, params: Value) -> Value {
        let Some(protocol) = self.negotiated_for(id) else {
            return params;
        };
        if protocol.era == McpProtocolEra::Modern && modern_request_requires_meta(method) {
            let mut object = params.as_object().cloned().unwrap_or_default();
            object.insert("_meta".into(), modern_meta(&protocol.version));
            Value::Object(object)
        } else {
            params
        }
    }

    async fn call_rpc(
        &self,
        conn: &McpRawConnection,
        method: &str,
        params: Value,
    ) -> Result<Value, McpError> {
        let connection = self.connection_for(conn.connection_id).ok_or_else(|| {
            McpError::Connection(format!("no such connection: {}", conn.connection_id))
        })?;
        let mut result = connection
            .call(
                method,
                self.decorate_params(conn.connection_id, method, params),
            )
            .await
            .map_err(|error| map_call_err(&error))?;
        if self
            .negotiated_for(conn.connection_id)
            .is_some_and(|protocol| {
                protocol.era == McpProtocolEra::Modern && modern_request_requires_meta(method)
            })
        {
            validate_modern_envelope(&mut result)?;
        }
        Ok(result)
    }

    async fn connect_before(
        &self,
        spec: &McpTransportSpec,
        deadline: tokio::time::Instant,
    ) -> Result<McpRawConnection, McpError> {
        let Some(remaining) = Self::remaining(deadline) else {
            return Err(McpError::Connection(
                "MCP connection deadline exceeded".into(),
            ));
        };
        tokio::time::timeout(remaining, self.connect(spec))
            .await
            .map_err(|_| McpError::Connection("MCP connection deadline exceeded".into()))?
    }

    async fn initialize_before(
        &self,
        conn: &McpRawConnection,
        version: &str,
        deadline: tokio::time::Instant,
    ) -> Result<ServerCapabilitiesDto, McpError> {
        let Some(remaining) = Self::remaining(deadline) else {
            return Err(McpError::Connection(
                "MCP connection deadline exceeded".into(),
            ));
        };
        tokio::time::timeout(
            remaining,
            self.initialize_with_version(conn, version, remaining),
        )
        .await
        .map_err(|_| McpError::Connection("MCP connection deadline exceeded".into()))?
    }

    async fn probe_modern(
        &self,
        conn: &McpRawConnection,
        deadline: tokio::time::Instant,
    ) -> Result<Option<String>, McpError> {
        let connection = self.connection_for(conn.connection_id).ok_or_else(|| {
            McpError::Connection(format!("no such connection: {}", conn.connection_id))
        })?;
        let mut probe = modern_probe_params();
        let mut corrective_retry = false;
        loop {
            let Some(timeout) = Self::remaining(deadline) else {
                return Err(McpError::Connection(
                    "MCP connection deadline exceeded".into(),
                ));
            };
            match connection
                .call_with_timeout_probe::<Value, Value>("server/discover", probe.clone(), timeout)
                .await
            {
                Ok(reply) => {
                    return Ok(reply
                        .get("protocolVersion")
                        .and_then(Value::as_str)
                        .filter(|version| *version == MODERN_PROTOCOL_VERSION)
                        .map(str::to_string));
                }
                Err(ConnectionError::Router(RouterError::Remote(error))) => {
                    if matches!(http_status(&error.data), Some(401 | 403)) {
                        return Err(handshake_error(&ConnectionError::Router(
                            RouterError::Remote(error),
                        )));
                    }
                    if error.code == -32022 {
                        if corrective_retry {
                            return Ok(None);
                        }
                        corrective_retry = true;
                        let supported = error
                            .data
                            .as_ref()
                            .and_then(|data| data.get("supported"))
                            .and_then(Value::as_array)
                            .and_then(|items| {
                                items.iter().find_map(|value| {
                                    value.as_str().filter(|v| *v == MODERN_PROTOCOL_VERSION)
                                })
                            });
                        if let Some(version) = supported {
                            probe["_meta"]["io.modelcontextprotocol/protocolVersion"] =
                                Value::String(version.to_string());
                            probe["protocolVersion"] = Value::String(version.to_string());
                            continue;
                        }
                        return Ok(None);
                    }
                    if is_modern_compatibility_error(error.code) {
                        return Ok(None);
                    }
                    return Err(McpError::Internal(format!(
                        "MCP modern discovery failed: code={}, message={}",
                        error.code, error.message
                    )));
                }
                Err(ConnectionError::Router(RouterError::Deserialize(_))) => return Ok(None),
                Err(ConnectionError::Router(RouterError::WrongResponseId { .. })) => {
                    return Ok(None)
                }
                Err(error) => return Err(map_call_err(&error)),
            }
        }
    }

    async fn initialize_with_version(
        &self,
        conn: &McpRawConnection,
        version: &str,
        timeout: Duration,
    ) -> Result<ServerCapabilitiesDto, McpError> {
        let connection = self.connection_for(conn.connection_id).ok_or_else(|| {
            McpError::Connection(format!("no such connection: {}", conn.connection_id))
        })?;
        let result: Value = connection
            .call_with_timeout(
                "initialize",
                self.decorate_params(
                    conn.connection_id,
                    "initialize",
                    initialize_params_for_version(version),
                ),
                timeout,
            )
            .await
            .map_err(|error| handshake_error(&error))?;
        let caps = result
            .get("capabilities")
            .and_then(Value::as_object)
            .ok_or_else(|| {
                McpError::Handshake("initialize result missing `capabilities` object".into())
            })?;
        let dto = capabilities_from_wire(caps, result.get("capabilities"));
        connection
            .notify("notifications/initialized", json!({}))
            .map_err(|error| McpError::Handshake(error.to_string()))?;
        Ok(dto)
    }
}

#[async_trait]
impl McpTransport for RemoteMcpTransport {
    async fn connect(&self, spec: &McpTransportSpec) -> Result<McpRawConnection, McpError> {
        let id = protocol::McpConnectionId::new();
        let connection = match spec {
            McpTransportSpec::Sse { url, headers, .. } => crate::connect_sse(url, None, headers)
                .await
                .map_err(McpError::from)?,
            McpTransportSpec::Http { url, headers, .. } => {
                crate::connect_http(url, None, headers, Some(Duration::from_secs(60)))
                    .await
                    .map_err(McpError::from)?
            }
            other => return Err(McpError::UnsupportedTransport(other.transport_kind())),
        };
        let connection = Arc::new(connection);
        connection
            .register_handler("ping", Arc::new(PingHandler))
            .await;
        self.connections
            .lock()
            .map_err(|_| McpError::Internal("remote connection map poisoned".into()))?
            .insert(id, connection);
        Ok(McpRawConnection { connection_id: id })
    }

    async fn connect_and_initialize(
        &self,
        spec: &McpTransportSpec,
        options: McpConnectOptions,
    ) -> Result<McpConnectResult, McpError> {
        let deadline = tokio::time::Instant::now()
            .checked_add(Duration::from_millis(options.deadline_ms))
            .ok_or_else(|| McpError::Connection("MCP connection deadline overflow".into()))?;
        let requested = options.expected_era.unwrap_or(McpProtocolEra::Legacy);
        let mut negotiated = McpNegotiatedProtocol {
            era: McpProtocolEra::Legacy,
            version: MCP_PROTOCOL_VERSION.to_string(),
        };
        if requested == McpProtocolEra::Modern {
            let probe = self.connect_before(spec, deadline).await?;
            let probe_guard = RemoteConnectionCleanupGuard {
                transport: self,
                id: probe.connection_id,
                armed: true,
            };
            let probe_cap =
                Duration::from_millis(bounded_probe_timeout_ms(options.probe_timeout_ms));
            let probe_deadline = tokio::time::Instant::now()
                .checked_add(probe_cap)
                .unwrap_or(deadline);
            match self
                .probe_modern(&probe, std::cmp::min(deadline, probe_deadline))
                .await
            {
                Ok(Some(version)) => {
                    negotiated = McpNegotiatedProtocol {
                        era: McpProtocolEra::Modern,
                        version,
                    };
                }
                Ok(None) => {}
                Err(error) => {
                    return Err(error);
                }
            }
            drop(probe_guard);
        }
        let connection = self.connect_before(spec, deadline).await?;
        if let Ok(mut map) = self.negotiated.lock() {
            map.insert(connection.connection_id, negotiated.clone());
        }
        let live_guard = RemoteConnectionCleanupGuard {
            transport: self,
            id: connection.connection_id,
            armed: true,
        };
        match self
            .initialize_before(&connection, &negotiated.version, deadline)
            .await
        {
            Ok(capabilities) => {
                live_guard.disarm();
                Ok(McpConnectResult {
                    connection,
                    capabilities,
                    negotiated,
                })
            }
            Err(error) => Err(error),
        }
    }

    async fn initialize(&self, conn: &McpRawConnection) -> Result<ServerCapabilitiesDto, McpError> {
        let version = self
            .negotiated_for(conn.connection_id)
            .map(|protocol| protocol.version)
            .unwrap_or_else(|| MCP_PROTOCOL_VERSION.to_string());
        self.initialize_with_version(conn, &version, Duration::from_secs(60))
            .await
    }

    async fn list_tools(&self, conn: &McpRawConnection) -> Result<Vec<McpToolDto>, McpError> {
        let raw = self.call_rpc(conn, "tools/list", json!({})).await?;
        let parsed: ToolsListResult = serde_json::from_value(raw).map_err(internal_decode)?;
        Ok(parsed.tools.into_iter().map(RawTool::into_dto).collect())
    }

    async fn list_resources(
        &self,
        conn: &McpRawConnection,
    ) -> Result<Vec<McpResourceDto>, McpError> {
        let raw = self.call_rpc(conn, "resources/list", json!({})).await?;
        let parsed: ResourcesListResult = serde_json::from_value(raw).map_err(internal_decode)?;
        Ok(parsed
            .resources
            .into_iter()
            .map(|resource| McpResourceDto {
                uri: resource.uri,
                name: resource.name,
                mime_type: resource.mime_type,
            })
            .collect())
    }

    async fn list_resource_templates(
        &self,
        conn: &McpRawConnection,
    ) -> Result<Vec<McpResourceTemplateDto>, McpError> {
        let raw = self
            .call_rpc(conn, "resources/templates/list", json!({}))
            .await?;
        let parsed: ResourceTemplatesListResult =
            serde_json::from_value(raw).map_err(internal_decode)?;
        Ok(parsed
            .resource_templates
            .into_iter()
            .map(|template| McpResourceTemplateDto {
                uri_template: template.uri_template,
                name: template.name,
                description: template.description,
                mime_type: template.mime_type,
            })
            .collect())
    }

    async fn list_prompts(&self, conn: &McpRawConnection) -> Result<Vec<McpPromptDto>, McpError> {
        let raw = self.call_rpc(conn, "prompts/list", json!({})).await?;
        let parsed: PromptsListResult = serde_json::from_value(raw).map_err(internal_decode)?;
        Ok(parsed
            .prompts
            .into_iter()
            .map(|prompt| McpPromptDto {
                name: prompt.name,
                description: prompt.description,
                arguments: prompt
                    .arguments
                    .into_iter()
                    .map(|argument| traits::McpPromptArgumentDto {
                        name: argument.name,
                        description: argument.description,
                        required: argument.required,
                    })
                    .collect(),
            })
            .collect())
    }

    async fn call_tool(
        &self,
        conn: &McpRawConnection,
        tool: &str,
        input: Value,
    ) -> Result<McpToolResultDto, McpError> {
        let connection = self.connection_for(conn.connection_id).ok_or_else(|| {
            McpError::Connection(format!("no such connection: {}", conn.connection_id))
        })?;
        let timeout = Duration::from_secs(60);
        let mut raw: Value = connection
            .call_with_timeout(
                "tools/call",
                self.decorate_params(
                    conn.connection_id,
                    "tools/call",
                    json!({ "name": tool, "arguments": input }),
                ),
                timeout,
            )
            .await
            .map_err(|error| match &error {
                ConnectionError::Router(RouterError::Timeout(_)) => McpError::Timeout {
                    server: String::new(),
                    tool: tool.to_string(),
                    secs: timeout.as_secs(),
                },
                ConnectionError::Router(RouterError::Remote(remote))
                    if remote.code == jsonrpc::METHOD_NOT_FOUND =>
                {
                    McpError::ToolNotFound(tool.to_string())
                }
                _ => map_call_err(&error),
            })?;
        if self
            .negotiated_for(conn.connection_id)
            .is_some_and(|protocol| {
                protocol.era == McpProtocolEra::Modern && modern_request_requires_meta("tools/call")
            })
        {
            validate_modern_envelope(&mut raw)?;
        }
        let parsed: ToolCallResult = serde_json::from_value(raw).map_err(internal_decode)?;
        Ok(McpToolResultDto {
            content: parsed.content,
            is_error: parsed.is_error,
            meta: parsed.meta,
            structured_content: parsed.structured_content,
        })
    }

    async fn read_resource(
        &self,
        conn: &McpRawConnection,
        uri: &str,
    ) -> Result<McpResourceContentDto, McpError> {
        let raw = self
            .call_rpc(conn, "resources/read", json!({ "uri": uri }))
            .await?;
        let parsed: ResourceReadResult = serde_json::from_value(raw).map_err(internal_decode)?;
        let first = parsed
            .contents
            .into_iter()
            .next()
            .ok_or_else(|| McpError::Internal("resources/read returned no contents".into()))?;
        Ok(McpResourceContentDto {
            uri: first.uri.unwrap_or_else(|| uri.to_string()),
            content: first.text.or(first.blob).unwrap_or_default(),
        })
    }

    async fn ping(&self, conn_id: protocol::McpConnectionId) -> Result<(), McpError> {
        let _: Value = self
            .call_rpc(
                &McpRawConnection {
                    connection_id: conn_id,
                },
                "ping",
                json!({}),
            )
            .await?;
        Ok(())
    }

    async fn notifications(
        &self,
        conn: &McpRawConnection,
    ) -> Result<McpNotificationStream, McpError> {
        let connection = self.connection_for(conn.connection_id).ok_or_else(|| {
            McpError::Connection(format!("no such connection: {}", conn.connection_id))
        })?;
        let stream =
            futures::stream::unfold(connection.notifications(), |mut receiver| async move {
                loop {
                    match receiver.recv().await {
                        Ok(notification) => {
                            return Some((
                                McpNotificationDto {
                                    method: notification.method,
                                    params: notification.params.unwrap_or(Value::Null),
                                },
                                receiver,
                            ));
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
                    }
                }
            });
        Ok(Box::pin(stream))
    }

    async fn handle_elicitation(
        &self,
        _conn: &McpRawConnection,
        _req: ElicitRequestDto,
    ) -> Result<ElicitResultDto, McpError> {
        Err(McpError::Internal(
            "remote mcp elicitation delegated to lingxi-mcp::McpClient".into(),
        ))
    }

    async fn disconnect(&self, conn_id: protocol::McpConnectionId) -> Result<(), McpError> {
        self.remove(conn_id);
        Ok(())
    }

    fn disconnect_sync(&self, conn_id: protocol::McpConnectionId) {
        self.remove(conn_id);
    }

    fn supported_transports(&self) -> Vec<McpTransportKind> {
        vec![McpTransportKind::Sse, McpTransportKind::Http]
    }
}

struct PingHandler;

#[async_trait]
impl InboundHandler for PingHandler {
    async fn handle(&self, request: Request) -> Response {
        Response::success(request.id, json!({}))
    }
}

/// Build the protocol-versioned initialize request shared by remote and stdio.
pub fn initialize_params_for_version(version: &str) -> Value {
    json!({
        "protocolVersion": version,
        "capabilities": {"roots": {}, "elicitation": {}},
        "clientInfo": {
            "name": MCP_CLIENT_NAME,
            "title": MCP_CLIENT_TITLE,
            "version": env!("CARGO_PKG_VERSION"),
            "description": CLIENT_DESCRIPTION,
            "websiteUrl": MCP_WEBSITE_URL,
        },
    })
}

/// Build the modern `server/discover` request envelope.
pub fn modern_probe_params() -> Value {
    json!({
        "_meta": modern_meta(MODERN_PROTOCOL_VERSION),
        "protocolVersion": MODERN_PROTOCOL_VERSION,
    })
}

/// Build the closed modern request metadata envelope.
pub fn modern_meta(version: &str) -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": version,
        "io.modelcontextprotocol/clientInfo": {
            "name": MCP_CLIENT_NAME,
            "title": MCP_CLIENT_TITLE,
            "version": env!("CARGO_PKG_VERSION"),
            "description": CLIENT_DESCRIPTION,
            "websiteUrl": MCP_WEBSITE_URL,
        },
        "io.modelcontextprotocol/clientCapabilities": {"roots": {}, "elicitation": {}},
    })
}

/// Whether a request carries the modern metadata envelope.
pub fn modern_request_requires_meta(method: &str) -> bool {
    matches!(
        method,
        "tools/list"
            | "tools/call"
            | "resources/list"
            | "resources/templates/list"
            | "resources/read"
            | "resources/directory/read"
            | "prompts/list"
            | "prompts/get"
    )
}

/// Validate and unwrap a modern MCP result envelope.
pub fn validate_modern_envelope(result: &mut Value) -> Result<(), McpError> {
    let Some(object) = result.as_object_mut() else {
        return Err(McpError::Handshake(
            "modern MCP reply must be an object envelope".into(),
        ));
    };
    match object
        .remove("resultType")
        .and_then(|value| value.as_str().map(str::to_string))
    {
        Some(result_type) if result_type == "complete" => Ok(()),
        Some(result_type) => Err(McpError::Handshake(format!(
            "unsupported modern MCP resultType: {result_type}"
        ))),
        None => Err(McpError::Handshake(
            "modern MCP reply missing resultType".into(),
        )),
    }
}

/// Extract the MCP skills directory-read extension flag.
pub fn directory_read_capability(capabilities: Option<&Value>) -> bool {
    capabilities
        .and_then(|value| value.get("extensions"))
        .and_then(|value| value.get(MCP_SKILLS_EXTENSION_KEY))
        .and_then(Value::as_object)
        .and_then(|value| value.get("directoryRead"))
        .and_then(Value::as_bool)
        == Some(true)
}

#[cfg(test)]
mod tests {
    use super::bounded_probe_timeout_ms;

    #[test]
    fn caller_probe_timeout_is_clamped_to_shared_remote_cap() {
        assert_eq!(bounded_probe_timeout_ms(None), 5_000);
        assert_eq!(bounded_probe_timeout_ms(Some(1_000)), 1_000);
        assert_eq!(bounded_probe_timeout_ms(Some(50_000)), 5_000);
    }
}

/// Decode the wire capability presence map into the shared DTO.
pub fn capabilities_from_wire(
    capabilities: &serde_json::Map<String, Value>,
    raw_capabilities: Option<&Value>,
) -> ServerCapabilitiesDto {
    ServerCapabilitiesDto {
        tools: capabilities.contains_key("tools"),
        resources: capabilities.contains_key("resources"),
        prompts: capabilities.contains_key("prompts"),
        logging: capabilities.contains_key("logging"),
        directory_read: directory_read_capability(raw_capabilities),
        experimental: capabilities
            .get("experimental")
            .and_then(|value| serde_json::from_value(value.clone()).ok())
            .unwrap_or_default(),
        extensions: capabilities
            .get("extensions")
            .and_then(Value::as_object)
            .map(|value| {
                value
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect()
            })
            .unwrap_or_default(),
    }
}

fn map_call_err(error: &ConnectionError) -> McpError {
    McpError::Internal(error.to_string())
}

fn internal_decode(error: serde_json::Error) -> McpError {
    McpError::Internal(error.to_string())
}

fn http_status(data: &Option<Value>) -> Option<u16> {
    data.as_ref()
        .and_then(|value| value.get("httpStatus"))
        .and_then(Value::as_u64)
        .and_then(|status| u16::try_from(status).ok())
}

fn is_modern_compatibility_error(code: i32) -> bool {
    matches!(code, jsonrpc::METHOD_NOT_FOUND | -32001 | -32020 | -32021)
}

fn handshake_error(error: &ConnectionError) -> McpError {
    if let ConnectionError::Router(RouterError::Remote(remote)) = error {
        if let Some(status) = http_status(&remote.data) {
            return McpError::HttpResponse {
                status,
                www_authenticate: remote
                    .data
                    .as_ref()
                    .and_then(|data| data.get("wwwAuthenticate"))
                    .and_then(Value::as_str)
                    .map(str::to_string),
            };
        }
    }
    McpError::Handshake(error.to_string())
}

#[derive(Deserialize)]
struct ToolsListResult {
    #[serde(default)]
    tools: Vec<RawTool>,
}

#[derive(Deserialize)]
struct RawTool {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(rename = "inputSchema", default)]
    input_schema: Value,
    #[serde(default, rename = "_meta")]
    meta: RawToolMeta,
}

#[derive(Deserialize, Default)]
struct RawToolMeta {
    #[serde(default, rename = "anthropic/requiresUserInteraction")]
    requires_user_interaction: bool,
}

impl RawTool {
    fn into_dto(self) -> McpToolDto {
        McpToolDto {
            server_name: String::new(),
            tool_name: self.name.clone(),
            description: self.description,
            input_schema: self.input_schema,
            full_name: format!("mcp____{}", self.name),
            search_hint: None,
            always_load: None,
            requires_user_interaction: self.meta.requires_user_interaction,
        }
    }
}

#[derive(Deserialize)]
struct ToolCallResult {
    #[serde(default)]
    content: Value,
    #[serde(rename = "isError", default)]
    is_error: bool,
    #[serde(rename = "_meta", default)]
    meta: Option<Value>,
    #[serde(rename = "structuredContent", default)]
    structured_content: Option<Value>,
}

#[derive(Deserialize)]
struct ResourcesListResult {
    #[serde(default)]
    resources: Vec<RawResource>,
}

#[derive(Deserialize)]
struct RawResource {
    uri: String,
    #[serde(default)]
    name: String,
    #[serde(rename = "mimeType")]
    mime_type: Option<String>,
}

#[derive(Deserialize)]
struct ResourceTemplatesListResult {
    #[serde(default, rename = "resourceTemplates")]
    resource_templates: Vec<RawResourceTemplate>,
}

#[derive(Deserialize)]
struct RawResourceTemplate {
    #[serde(rename = "uriTemplate")]
    uri_template: String,
    #[serde(default)]
    name: String,
    description: Option<String>,
    #[serde(rename = "mimeType")]
    mime_type: Option<String>,
}

#[derive(Deserialize)]
struct ResourceReadResult {
    #[serde(default)]
    contents: Vec<RawResourceContent>,
}

#[derive(Deserialize)]
struct RawResourceContent {
    uri: Option<String>,
    text: Option<String>,
    blob: Option<String>,
}

#[derive(Deserialize)]
struct PromptsListResult {
    #[serde(default)]
    prompts: Vec<RawPrompt>,
}

#[derive(Deserialize)]
struct RawPrompt {
    name: String,
    description: Option<String>,
    #[serde(default)]
    arguments: Vec<RawPromptArgument>,
}

#[derive(Deserialize)]
struct RawPromptArgument {
    name: String,
    description: Option<String>,
    #[serde(default)]
    required: bool,
}
