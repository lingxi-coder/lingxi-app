//! Mobile MCP transport composition.
//!
//! Local Apps and remote MCP servers have separate ownership boundaries.  The
//! composite only routes a connection id; it does not merge the Local Apps
//! registry into the remote transport or infer a route from a server name.

use async_trait::async_trait;
use platform_common::RemoteMcpTransport;
use protocol::McpConnectionId;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use traits::{
    ElicitRequestDto, ElicitResultDto, McpConnectOptions, McpConnectResult, McpError,
    McpNotificationStream, McpPromptDto, McpRawConnection, McpResourceContentDto, McpResourceDto,
    McpResourceTemplateDto, McpToolDto, McpToolResultDto, McpTransport, McpTransportKind,
    McpTransportSpec, ServerCapabilitiesDto,
};

use crate::local_apps_mcp::LocalAppsMcpTransport;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route {
    LocalApps,
    Remote,
}

/// Routes each live mobile MCP connection to its owning transport.
pub(crate) struct MobileMcpTransport {
    local_apps: Arc<LocalAppsMcpTransport>,
    remote: Arc<RemoteMcpTransport>,
    routes: Mutex<HashMap<McpConnectionId, Route>>,
}

impl MobileMcpTransport {
    pub(crate) fn new(
        local_apps: Arc<LocalAppsMcpTransport>,
        remote: Arc<RemoteMcpTransport>,
    ) -> Self {
        Self {
            local_apps,
            remote,
            routes: Mutex::new(HashMap::new()),
        }
    }

    fn route_for(&self, id: McpConnectionId) -> Result<Route, McpError> {
        self.routes
            .lock()
            .map_err(|_| McpError::Internal("mobile MCP route map poisoned".into()))?
            .get(&id)
            .copied()
            .ok_or_else(|| McpError::Connection(format!("no such connection: {id}")))
    }

    fn remember(&self, id: McpConnectionId, route: Route) -> Result<(), McpError> {
        self.routes
            .lock()
            .map_err(|_| McpError::Internal("mobile MCP route map poisoned".into()))?
            .insert(id, route);
        Ok(())
    }

    fn forget(&self, id: McpConnectionId) -> Result<(), McpError> {
        self.routes
            .lock()
            .map_err(|_| McpError::Internal("mobile MCP route map poisoned".into()))?
            .remove(&id);
        Ok(())
    }

    fn is_local(spec: &McpTransportSpec) -> bool {
        matches!(spec, McpTransportSpec::InProcess { .. })
    }

    fn is_remote(spec: &McpTransportSpec) -> bool {
        matches!(
            spec,
            McpTransportSpec::Sse { .. } | McpTransportSpec::Http { .. }
        )
    }
}

#[async_trait]
impl McpTransport for MobileMcpTransport {
    async fn connect(&self, spec: &McpTransportSpec) -> Result<McpRawConnection, McpError> {
        let (route, connection) = if Self::is_local(spec) {
            (Route::LocalApps, self.local_apps.connect(spec).await?)
        } else if Self::is_remote(spec) {
            (Route::Remote, self.remote.connect(spec).await?)
        } else {
            return Err(McpError::UnsupportedTransport(spec.transport_kind()));
        };
        if let Err(error) = self.remember(connection.connection_id, route) {
            match route {
                Route::LocalApps => self.local_apps.disconnect_sync(connection.connection_id),
                Route::Remote => self.remote.disconnect_sync(connection.connection_id),
            }
            return Err(error);
        }
        Ok(connection)
    }

    async fn connect_and_initialize(
        &self,
        spec: &McpTransportSpec,
        options: McpConnectOptions,
    ) -> Result<McpConnectResult, McpError> {
        let (route, result) = if Self::is_local(spec) {
            (
                Route::LocalApps,
                self.local_apps
                    .connect_and_initialize(spec, options)
                    .await?,
            )
        } else if Self::is_remote(spec) {
            (
                Route::Remote,
                self.remote.connect_and_initialize(spec, options).await?,
            )
        } else {
            return Err(McpError::UnsupportedTransport(spec.transport_kind()));
        };
        if let Err(error) = self.remember(result.connection.connection_id, route) {
            match route {
                Route::LocalApps => self
                    .local_apps
                    .disconnect_sync(result.connection.connection_id),
                Route::Remote => self.remote.disconnect_sync(result.connection.connection_id),
            }
            return Err(error);
        }
        Ok(result)
    }

    async fn initialize(&self, conn: &McpRawConnection) -> Result<ServerCapabilitiesDto, McpError> {
        match self.route_for(conn.connection_id)? {
            Route::LocalApps => self.local_apps.initialize(conn).await,
            Route::Remote => self.remote.initialize(conn).await,
        }
    }

    async fn list_tools(&self, conn: &McpRawConnection) -> Result<Vec<McpToolDto>, McpError> {
        match self.route_for(conn.connection_id)? {
            Route::LocalApps => self.local_apps.list_tools(conn).await,
            Route::Remote => self.remote.list_tools(conn).await,
        }
    }

    async fn list_resources(
        &self,
        conn: &McpRawConnection,
    ) -> Result<Vec<McpResourceDto>, McpError> {
        match self.route_for(conn.connection_id)? {
            Route::LocalApps => self.local_apps.list_resources(conn).await,
            Route::Remote => self.remote.list_resources(conn).await,
        }
    }

    async fn list_resource_templates(
        &self,
        conn: &McpRawConnection,
    ) -> Result<Vec<McpResourceTemplateDto>, McpError> {
        match self.route_for(conn.connection_id)? {
            Route::LocalApps => self.local_apps.list_resource_templates(conn).await,
            Route::Remote => self.remote.list_resource_templates(conn).await,
        }
    }

    async fn list_prompts(&self, conn: &McpRawConnection) -> Result<Vec<McpPromptDto>, McpError> {
        match self.route_for(conn.connection_id)? {
            Route::LocalApps => self.local_apps.list_prompts(conn).await,
            Route::Remote => self.remote.list_prompts(conn).await,
        }
    }

    async fn call_tool(
        &self,
        conn: &McpRawConnection,
        tool: &str,
        input: Value,
    ) -> Result<McpToolResultDto, McpError> {
        match self.route_for(conn.connection_id)? {
            Route::LocalApps => self.local_apps.call_tool(conn, tool, input).await,
            Route::Remote => self.remote.call_tool(conn, tool, input).await,
        }
    }

    async fn read_resource(
        &self,
        conn: &McpRawConnection,
        uri: &str,
    ) -> Result<McpResourceContentDto, McpError> {
        match self.route_for(conn.connection_id)? {
            Route::LocalApps => self.local_apps.read_resource(conn, uri).await,
            Route::Remote => self.remote.read_resource(conn, uri).await,
        }
    }

    async fn ping(&self, conn_id: McpConnectionId) -> Result<(), McpError> {
        match self.route_for(conn_id)? {
            Route::LocalApps => self.local_apps.ping(conn_id).await,
            Route::Remote => self.remote.ping(conn_id).await,
        }
    }

    async fn notifications(
        &self,
        conn: &McpRawConnection,
    ) -> Result<McpNotificationStream, McpError> {
        match self.route_for(conn.connection_id)? {
            Route::LocalApps => self.local_apps.notifications(conn).await,
            Route::Remote => self.remote.notifications(conn).await,
        }
    }

    async fn handle_elicitation(
        &self,
        conn: &McpRawConnection,
        request: ElicitRequestDto,
    ) -> Result<ElicitResultDto, McpError> {
        match self.route_for(conn.connection_id)? {
            Route::LocalApps => self.local_apps.handle_elicitation(conn, request).await,
            Route::Remote => self.remote.handle_elicitation(conn, request).await,
        }
    }

    async fn disconnect(&self, conn_id: McpConnectionId) -> Result<(), McpError> {
        let route = self.route_for(conn_id)?;
        let result = match route {
            Route::LocalApps => self.local_apps.disconnect(conn_id).await,
            Route::Remote => self.remote.disconnect(conn_id).await,
        };
        if result.is_ok() {
            self.forget(conn_id)?;
        }
        result
    }

    fn disconnect_sync(&self, conn_id: McpConnectionId) {
        let route = self
            .routes
            .lock()
            .ok()
            .and_then(|mut routes| routes.remove(&conn_id));
        match route {
            Some(Route::LocalApps) => self.local_apps.disconnect_sync(conn_id),
            Some(Route::Remote) => self.remote.disconnect_sync(conn_id),
            None => {}
        }
    }

    fn supported_transports(&self) -> Vec<McpTransportKind> {
        vec![
            McpTransportKind::InProcess,
            McpTransportKind::Sse,
            McpTransportKind::Http,
        ]
    }
}

impl mcp::RawConnectionProvider for MobileMcpTransport {
    fn connection_for(&self, id: McpConnectionId) -> Option<Arc<jsonrpc::Connection>> {
        match self.route_for(id).ok()? {
            Route::LocalApps => None,
            Route::Remote => self.remote.connection_for(id),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcp::RawConnectionProvider;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn one_request_http_server() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            // The test only needs one request (initialize); the HTTP writer
            // closes the connection after the response is consumed.
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let mut bytes = Vec::new();
            let mut chunk = [0_u8; 4096];
            loop {
                let read = stream.read(&mut chunk).await.unwrap_or(0);
                if read == 0 {
                    break;
                }
                bytes.extend_from_slice(&chunk[..read]);
                if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let body = String::from_utf8_lossy(&bytes);
            let id = body
                .split("\"id\":")
                .nth(1)
                .and_then(|value| value.split(|c| c == ',' || c == '}').next())
                .unwrap_or("1");
            let response_body =
                format!("{{\"jsonrpc\":\"2.0\",\"id\":{id},\"result\":{{\"capabilities\":{{}}}}}}");
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response_body.len(), response_body
            );
            let _ = stream.write_all(response.as_bytes()).await;
        });
        format!("http://{address}/mcp")
    }

    #[tokio::test]
    async fn route_raw_disconnect_and_unsupported_are_explicit() {
        let root = tempfile::tempdir().unwrap();
        let local = Arc::new(LocalAppsMcpTransport::new(root.path().to_path_buf()));
        let remote = Arc::new(RemoteMcpTransport::new());
        let transport = MobileMcpTransport::new(local, remote);

        let local_connection = transport
            .connect(&McpTransportSpec::InProcess {
                registry_key: "local_apps".into(),
            })
            .await
            .unwrap();
        assert!(
            transport
                .connection_for(local_connection.connection_id)
                .is_none(),
            "Local Apps is in-process and has no JSON-RPC raw connection"
        );
        transport
            .disconnect(local_connection.connection_id)
            .await
            .unwrap();
        assert!(matches!(
            transport.ping(local_connection.connection_id).await,
            Err(McpError::Connection(_))
        ));

        let unsupported = transport
            .connect(&McpTransportSpec::WebSocket {
                url: "ws://127.0.0.1:1".into(),
                headers: traits::McpHeaders::new(),
                headers_helper: None,
            })
            .await;
        assert!(matches!(
            unsupported,
            Err(McpError::UnsupportedTransport(McpTransportKind::WebSocket))
        ));
    }

    #[tokio::test]
    async fn remote_route_exposes_raw_connection_until_disconnect() {
        let root = tempfile::tempdir().unwrap();
        let local = Arc::new(LocalAppsMcpTransport::new(root.path().to_path_buf()));
        let remote = Arc::new(RemoteMcpTransport::new());
        let transport = MobileMcpTransport::new(local, remote);
        let url = one_request_http_server().await;
        let connection = transport
            .connect(&McpTransportSpec::Http {
                url,
                headers: traits::McpHeaders::new(),
                headers_helper: None,
                oauth: None,
            })
            .await
            .unwrap();
        assert!(transport.connection_for(connection.connection_id).is_some());
        tokio::time::timeout(Duration::from_secs(5), transport.initialize(&connection))
            .await
            .unwrap()
            .unwrap();
        transport
            .disconnect(connection.connection_id)
            .await
            .unwrap();
        assert!(transport.connection_for(connection.connection_id).is_none());
    }

    #[tokio::test]
    async fn remember_failure_cleans_up_underlying_local_and_remote_connections() {
        let root = tempfile::tempdir().unwrap();
        let local = Arc::new(LocalAppsMcpTransport::new(root.path().join("local")));
        let remote = Arc::new(RemoteMcpTransport::new());
        let transport = MobileMcpTransport::new(local.clone(), remote.clone());
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = transport.routes.lock().unwrap();
            panic!("poison route map for cleanup test");
        }));
        let error = transport
            .connect(&McpTransportSpec::InProcess {
                registry_key: "local_apps".into(),
            })
            .await
            .expect_err("poisoned route map must reject remember");
        assert!(matches!(error, McpError::Internal(_)));

        let remote_root = tempfile::tempdir().unwrap();
        let local = Arc::new(LocalAppsMcpTransport::new(remote_root.path().join("local")));
        let remote = Arc::new(RemoteMcpTransport::new());
        let transport = MobileMcpTransport::new(local, remote.clone());
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = transport.routes.lock().unwrap();
            panic!("poison route map for remote cleanup test");
        }));
        let url = one_request_http_server().await;
        let error = transport
            .connect_and_initialize(
                &McpTransportSpec::Http {
                    url,
                    headers: traits::McpHeaders::new(),
                    headers_helper: None,
                    oauth: None,
                },
                McpConnectOptions {
                    expected_era: Some(traits::McpProtocolEra::Legacy),
                    deadline_ms: 5_000,
                },
            )
            .await
            .expect_err("poisoned route map must reject initialized remember");
        assert!(matches!(error, McpError::Internal(_)));
    }

    #[tokio::test]
    async fn local_apps_is_cache_ineligible_with_composite_registry() {
        let root = tempfile::tempdir().unwrap();
        let cache_root = root.path().join("mcp-discovery-cache");
        let local = Arc::new(LocalAppsMcpTransport::new(root.path().join("apps")));
        let remote = Arc::new(RemoteMcpTransport::new());
        let transport = Arc::new(MobileMcpTransport::new(local, remote));
        let registry = mcp::McpRegistry::with_raw_conn(
            transport.clone() as Arc<dyn McpTransport>,
            transport.clone() as Arc<dyn mcp::RawConnectionProvider>,
        )
        .with_discovery_cache_store(mcp::DiscoveryCacheStore::new(&cache_root));
        registry
            .connect(mcp::McpServerConfig {
                name: "local_apps".into(),
                spec: McpTransportSpec::InProcess {
                    registry_key: "local_apps".into(),
                },
                scope: mcp::ConfigScope::Managed,
                disabled: false,
                timeout_ms: None,
                always_load: true,
                discovery_cache: None,
                config_error: None,
                metadata: Default::default(),
            })
            .await
            .unwrap();
        assert!(
            !cache_root.exists(),
            "Local Apps InProcess must never create discovery-cache entries"
        );
    }
}
