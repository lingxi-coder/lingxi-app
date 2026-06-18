//! End-to-end OAuth 2.1 + PKCE flow for remote (SSE/HTTP) MCP servers, against
//! a local mock authorization server.
//!
//! Covers (task TDD list):
//! - discovery → DCR → PKCE → authorize URL → loopback callback → token
//!   exchange, and that the resulting connection attaches `Authorization:
//!   Bearer <access>` to the SSE/HTTP spec;
//! - a static-token server (`oauth: None`) is connected with its spec
//!   UNCHANGED (regression guard);
//! - refresh-on-expiry: a stored, expired token + refresh token drives a
//!   `refresh_token` grant and attaches the new access token;
//! - 401-on-connect: the transport 401s once, the registry refreshes and
//!   retries, and the retried spec carries the new Bearer.

use async_trait::async_trait;
use mcp::connection::{ConfigScope, McpServerConfig};
use mcp::oauth::{self, OnAuthorizationUrl};
use mcp::registry::{McpRegistry, OAuthDeps};
use protocol::{
    HttpMethod, HttpRequest, HttpResponse, SecretKindDto, SecureStorageData, SecureStorageMetadata,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use traits::http::SseStream;
use traits::{
    Clock, ElicitRequestDto, ElicitResultDto, HttpError, HttpTransport, McpError,
    McpNotificationStream, McpOAuthConfigDto, McpPromptDto, McpRawConnection, McpResourceContentDto,
    McpResourceDto, McpToolDto, McpToolResultDto, McpTransport, McpTransportKind, McpTransportSpec,
    SecureStorage, SecureStorageBackend, SecureStorageError, ServerCapabilitiesDto,
};

// ---------------------------------------------------------------------------
// Mock authorization server (HTTP transport with canned, body-aware routes).
// ---------------------------------------------------------------------------

/// A canned HTTP response.
#[derive(Clone)]
struct Canned {
    status: u16,
    body: String,
}

/// HTTP transport that fakes the authorization server:
/// - GET `.well-known/oauth-authorization-server` → AS metadata
/// - GET `.well-known/oauth-protected-resource`   → 404 (force the 8414 path)
/// - POST `/register`                             → DCR (issues `client_id`)
/// - POST `/token` with `grant_type=authorization_code` → first token set
/// - POST `/token` with `grant_type=refresh_token`      → refreshed token set
struct MockAs {
    requests: Mutex<Vec<HttpRequest>>,
    exchange_body: String,
    refresh_body: String,
}

impl MockAs {
    fn new(exchange_body: &str, refresh_body: &str) -> Arc<Self> {
        Arc::new(Self {
            requests: Mutex::new(Vec::new()),
            exchange_body: exchange_body.into(),
            refresh_body: refresh_body.into(),
        })
    }

    fn route(&self, req: &HttpRequest) -> Canned {
        let url = &req.url;
        let body = req.body.as_deref().unwrap_or("");
        if url.contains("oauth-protected-resource") {
            // Force the RFC 8414 direct-against-server fallback.
            return Canned {
                status: 404,
                body: String::new(),
            };
        }
        if url.contains("oauth-authorization-server") {
            return Canned {
                status: 200,
                body: r#"{
                    "authorization_endpoint": "https://as.example.com/authorize",
                    "token_endpoint": "https://as.example.com/token",
                    "registration_endpoint": "https://as.example.com/register",
                    "scopes_supported": ["mcp:read"]
                }"#
                .into(),
            };
        }
        if url.contains("/register") {
            return Canned {
                status: 201,
                body: r#"{"client_id": "dyn-client-9"}"#.into(),
            };
        }
        if url.contains("/token") {
            if body.contains("grant_type=refresh_token") {
                return Canned {
                    status: 200,
                    body: self.refresh_body.clone(),
                };
            }
            return Canned {
                status: 200,
                body: self.exchange_body.clone(),
            };
        }
        Canned {
            status: 404,
            body: String::new(),
        }
    }

    fn requests(&self) -> Vec<HttpRequest> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait]
impl HttpTransport for MockAs {
    async fn request(&self, req: HttpRequest) -> Result<HttpResponse, HttpError> {
        let canned = self.route(&req);
        self.requests.lock().unwrap().push(req);
        Ok(HttpResponse {
            status: canned.status,
            headers: vec![],
            body: canned.body,
        })
    }
    async fn stream_sse(&self, _req: HttpRequest) -> Result<SseStream, HttpError> {
        Err(HttpError::InvalidRequest("sse unused".into()))
    }
}

// ---------------------------------------------------------------------------
// Fixed clock + in-memory secure storage.
// ---------------------------------------------------------------------------

struct TestClock {
    secs: AtomicU64,
}
impl TestClock {
    fn new(secs: u64) -> Arc<Self> {
        Arc::new(Self {
            secs: AtomicU64::new(secs),
        })
    }
}
impl Clock for TestClock {
    fn now(&self) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(self.secs.load(Ordering::SeqCst))
    }
}

#[derive(Default)]
struct MemStorage {
    map: Mutex<HashMap<(String, String), SecureStorageData>>,
}
impl MemStorage {
    fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
}
#[async_trait]
impl SecureStorage for MemStorage {
    async fn store(
        &self,
        service: &str,
        account: &str,
        data: SecureStorageData,
    ) -> Result<(), SecureStorageError> {
        self.map
            .lock()
            .unwrap()
            .insert((service.into(), account.into()), data);
        Ok(())
    }
    async fn retrieve(
        &self,
        service: &str,
        account: &str,
    ) -> Result<Option<SecureStorageData>, SecureStorageError> {
        Ok(self
            .map
            .lock()
            .unwrap()
            .get(&(service.into(), account.into()))
            .cloned())
    }
    async fn delete(&self, service: &str, account: &str) -> Result<(), SecureStorageError> {
        self.map
            .lock()
            .unwrap()
            .remove(&(service.into(), account.into()));
        Ok(())
    }
    async fn list(&self, service: &str) -> Result<Vec<String>, SecureStorageError> {
        Ok(self
            .map
            .lock()
            .unwrap()
            .keys()
            .filter(|(s, _)| s == service)
            .map(|(_, a)| a.clone())
            .collect())
    }
    fn is_encrypted(&self) -> bool {
        false
    }
    fn backend(&self) -> SecureStorageBackend {
        SecureStorageBackend::PlainText
    }
}

// ---------------------------------------------------------------------------
// Recording MCP transport: captures the spec it was connected with, and can be
// configured to 401 the first N connect attempts.
// ---------------------------------------------------------------------------

struct RecordingTransport {
    seen_specs: Mutex<Vec<McpTransportSpec>>,
    fail_401_first: AtomicUsize,
}
impl RecordingTransport {
    fn new(fail_401_first: usize) -> Arc<Self> {
        Arc::new(Self {
            seen_specs: Mutex::new(Vec::new()),
            fail_401_first: AtomicUsize::new(fail_401_first),
        })
    }
    fn last_spec(&self) -> McpTransportSpec {
        self.seen_specs.lock().unwrap().last().cloned().unwrap()
    }
    fn connect_count(&self) -> usize {
        self.seen_specs.lock().unwrap().len()
    }
}

/// Pull `Authorization` out of an SSE/HTTP spec's headers, if present.
fn spec_auth_header(spec: &McpTransportSpec) -> Option<String> {
    match spec {
        McpTransportSpec::Sse { headers, .. } | McpTransportSpec::Http { headers, .. } => {
            headers.get("Authorization").cloned()
        }
        _ => None,
    }
}

#[async_trait]
impl McpTransport for RecordingTransport {
    async fn connect(&self, spec: &McpTransportSpec) -> Result<McpRawConnection, McpError> {
        self.seen_specs.lock().unwrap().push(spec.clone());
        if self.fail_401_first.load(Ordering::SeqCst) > 0 {
            self.fail_401_first.fetch_sub(1, Ordering::SeqCst);
            return Err(McpError::Connection("HTTP 401 Unauthorized".into()));
        }
        Ok(McpRawConnection {
            connection_id: protocol::McpConnectionId::new(),
        })
    }
    async fn initialize(&self, _c: &McpRawConnection) -> Result<ServerCapabilitiesDto, McpError> {
        Ok(ServerCapabilitiesDto {
            tools: false,
            resources: false,
            prompts: false,
            logging: false,
            experimental: HashMap::new(),
        })
    }
    async fn list_tools(&self, _c: &McpRawConnection) -> Result<Vec<McpToolDto>, McpError> {
        Ok(vec![])
    }
    async fn list_resources(&self, _c: &McpRawConnection) -> Result<Vec<McpResourceDto>, McpError> {
        Ok(vec![])
    }
    async fn list_prompts(&self, _c: &McpRawConnection) -> Result<Vec<McpPromptDto>, McpError> {
        Ok(vec![])
    }
    async fn call_tool(
        &self,
        _c: &McpRawConnection,
        _t: &str,
        _i: serde_json::Value,
    ) -> Result<McpToolResultDto, McpError> {
        Ok(McpToolResultDto::default())
    }
    async fn read_resource(
        &self,
        _c: &McpRawConnection,
        _u: &str,
    ) -> Result<McpResourceContentDto, McpError> {
        Err(McpError::Internal("unused".into()))
    }
    async fn ping(&self, _id: protocol::McpConnectionId) -> Result<(), McpError> {
        Ok(())
    }
    async fn notifications(
        &self,
        _c: &McpRawConnection,
    ) -> Result<McpNotificationStream, McpError> {
        unreachable!("notifications unused")
    }
    async fn handle_elicitation(
        &self,
        _c: &McpRawConnection,
        _r: ElicitRequestDto,
    ) -> Result<ElicitResultDto, McpError> {
        Err(McpError::Internal("unused".into()))
    }
    async fn disconnect(&self, _id: protocol::McpConnectionId) -> Result<(), McpError> {
        Ok(())
    }
    fn supported_transports(&self) -> Vec<McpTransportKind> {
        vec![McpTransportKind::Http]
    }
}

// ---------------------------------------------------------------------------
// Helpers.
// ---------------------------------------------------------------------------

fn http_cfg(name: &str, oauth: Option<McpOAuthConfigDto>) -> McpServerConfig {
    McpServerConfig {
        name: name.into(),
        spec: McpTransportSpec::Http {
            url: "https://mcp.example.com/v1".into(),
            headers: traits::McpHeaders::new(),
            oauth,
        },
        scope: ConfigScope::Project,
        disabled: false,
    }
}

fn oauth_block(client_id: Option<&str>) -> McpOAuthConfigDto {
    McpOAuthConfigDto {
        client_id: client_id.map(str::to_string),
        callback_port: None,
        auth_server_metadata_url: None,
        xaa: None,
    }
}

/// Fire a fake-browser GET to the loopback `/callback` once `auth_url` is
/// surfaced. Extracts the redirect port + `state` from the URL.
async fn drive_browser(auth_url: &str) {
    // redirect_uri=http%3A%2F%2Flocalhost%3A{port}%2Fcallback
    let port = auth_url
        .split("localhost%3A")
        .nth(1)
        .and_then(|s| s.split("%2Fcallback").next())
        .expect("redirect port in auth url")
        .to_string();
    let state = auth_url
        .split("state=")
        .nth(1)
        .and_then(|s| s.split('&').next())
        .expect("state in auth url")
        .to_string();
    // Retry the connect briefly until the listener is bound.
    for _ in 0..50 {
        if let Ok(mut s) = tokio::net::TcpStream::connect(("127.0.0.1", port.parse::<u16>().unwrap()))
            .await
        {
            use tokio::io::AsyncWriteExt;
            let req = format!(
                "GET /callback?code=auth-code-xyz&state={state} HTTP/1.1\r\nHost: localhost\r\n\r\n"
            );
            let _ = s.write_all(req.as_bytes()).await;
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("callback listener never bound on port {port}");
}

/// `on_authorization_url` hook that pushes the surfaced URL onto a channel so
/// the test can drive the fake browser.
fn url_capture() -> (OnAuthorizationUrl, tokio::sync::mpsc::UnboundedReceiver<String>) {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let cb: OnAuthorizationUrl = Arc::new(move |url: &str| {
        let _ = tx.send(url.to_string());
    });
    (cb, rx)
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn full_flow_attaches_bearer_and_persists_tokens() {
    let exchange = r#"{"access_token":"access-1","refresh_token":"refresh-1","expires_in":3600}"#;
    let mock_as = MockAs::new(exchange, "{}");
    let transport = RecordingTransport::new(0);
    let storage = MemStorage::new();
    let clock = TestClock::new(1_000);
    let (on_url, mut url_rx) = url_capture();

    let registry = McpRegistry::new(transport.clone() as Arc<dyn McpTransport>).with_oauth(
        OAuthDeps {
            http: mock_as.clone() as Arc<dyn HttpTransport>,
            clock: clock.clone() as Arc<dyn Clock>,
            storage: storage.clone() as Arc<dyn SecureStorage>,
            on_authorization_url: on_url,
        },
    );

    // No client_id configured → DCR is exercised.
    let config = http_cfg("remote", Some(oauth_block(None)));

    // Drive the browser when the auth URL is surfaced, concurrently with connect.
    let browser = tokio::spawn(async move {
        let url = url_rx.recv().await.expect("auth url surfaced");
        drive_browser(&url).await;
    });

    let registry2 = Arc::new(registry);
    registry2.connect(config.clone()).await.expect("connect ok");
    browser.await.unwrap();

    // The transport saw the Bearer-augmented spec.
    assert_eq!(
        spec_auth_header(&transport.last_spec()).as_deref(),
        Some("Bearer access-1")
    );

    // Discovery hit oauth-authorization-server; DCR hit /register; token
    // exchange used the authorization_code grant with the PKCE verifier.
    let reqs = mock_as.requests();
    assert!(reqs.iter().any(|r| r.url.contains("oauth-authorization-server")));
    let register = reqs.iter().find(|r| r.url.contains("/register")).unwrap();
    assert_eq!(register.method, HttpMethod::Post);
    // FIX 3: the DCR client metadata carries the per-server client_name
    // `LingXi (${serverName})` (auth.ts:1419) and the advertised scope
    // (auth.ts:1428 / getScopeFromMetadata).
    let reg_body = register.body.as_deref().unwrap();
    assert!(
        reg_body.contains("\"client_name\":\"LingXi (remote)\""),
        "DCR client_name should be per-server; body={reg_body}"
    );
    assert!(
        reg_body.contains("\"scope\":\"mcp:read\""),
        "DCR metadata should include advertised scope; body={reg_body}"
    );
    // FIX 1: the persisted tokens carry the DCR-issued client_id so refresh
    // re-sends it (asserted on storage below).
    let token = reqs.iter().find(|r| r.url.contains("/token")).unwrap();
    let body = token.body.as_deref().unwrap();
    assert!(body.contains("grant_type=authorization_code"));
    assert!(body.contains("code=auth-code-xyz"));
    assert!(body.contains("code_verifier="));
    assert!(body.contains("client_id=dyn-client-9"));

    // Tokens were persisted under the server key.
    let key = oauth::server_key("remote", &config.spec);
    let stored = oauth::load_tokens(&(storage as Arc<dyn SecureStorage>), &key)
        .await
        .unwrap()
        .expect("tokens stored");
    assert_eq!(stored.access_token, "access-1");
    assert_eq!(stored.refresh_token.as_deref(), Some("refresh-1"));
    // FIX 1: the DCR-issued client_id is persisted, so a later silent refresh
    // re-sends it instead of an empty string.
    assert_eq!(stored.client_id.as_deref(), Some("dyn-client-9"));
}

#[tokio::test]
async fn static_token_server_spec_is_unchanged() {
    // oauth: None → the OAuth seam, even when wired, must not touch the spec.
    let mock_as = MockAs::new("{}", "{}");
    let transport = RecordingTransport::new(0);
    let storage = MemStorage::new();
    let clock = TestClock::new(0);
    let (on_url, _rx) = url_capture();

    let registry = McpRegistry::new(transport.clone() as Arc<dyn McpTransport>).with_oauth(
        OAuthDeps {
            http: mock_as as Arc<dyn HttpTransport>,
            clock: clock as Arc<dyn Clock>,
            storage: storage as Arc<dyn SecureStorage>,
            on_authorization_url: on_url,
        },
    );

    let mut headers = traits::McpHeaders::new();
    headers.insert("X-Static".to_string(), "preset".to_string());
    let config = McpServerConfig {
        name: "static".into(),
        spec: McpTransportSpec::Http {
            url: "https://static.example.com".into(),
            headers: headers.clone(),
            oauth: None,
        },
        scope: ConfigScope::Project,
        disabled: false,
    };

    registry.connect(config.clone()).await.expect("connect ok");

    // Spec is passed through verbatim: no Authorization header added.
    let seen = transport.last_spec();
    assert!(spec_auth_header(&seen).is_none(), "no Bearer for static server");
    if let McpTransportSpec::Http { headers: h, .. } = seen {
        assert_eq!(h.get("X-Static").map(String::as_str), Some("preset"));
        assert_eq!(h.len(), 1, "no extra headers injected");
    } else {
        panic!("expected Http spec");
    }
}

#[tokio::test]
async fn expired_token_triggers_refresh_and_attaches_new_bearer() {
    let refresh = r#"{"access_token":"access-2","refresh_token":"refresh-2","expires_in":3600}"#;
    let mock_as = MockAs::new("{}", refresh);
    let transport = RecordingTransport::new(0);
    let storage = MemStorage::new();
    let clock = TestClock::new(10_000);
    let (on_url, _rx) = url_capture();

    let config = http_cfg("refreshing", Some(oauth_block(Some("preset-client"))));
    let key = oauth::server_key("refreshing", &config.spec);

    // Seed an EXPIRED token (expires_at = 5_000 < now = 10_000) + refresh token
    // + a DCR-issued client_id that DIFFERS from the configured one, so the
    // refresh must prefer the persisted id (FIX 1).
    let stored = oauth::StoredTokens {
        access_token: "stale".into(),
        refresh_token: Some("refresh-1".into()),
        expires_at_unix: 5_000,
        client_id: Some("dcr-issued-7".into()),
    };
    let bytes = serde_json::to_vec(&stored).unwrap();
    let data = SecureStorageData::new(
        bytes,
        SecureStorageMetadata {
            created_at: SystemTime::UNIX_EPOCH,
            last_accessed: None,
            kind: SecretKindDto("mcp_oauth_tokens".into()),
        },
    );
    storage
        .store(oauth::MCP_OAUTH_SERVICE, &key, data)
        .await
        .unwrap();

    let registry = McpRegistry::new(transport.clone() as Arc<dyn McpTransport>).with_oauth(
        OAuthDeps {
            http: mock_as.clone() as Arc<dyn HttpTransport>,
            clock: clock as Arc<dyn Clock>,
            storage: storage.clone() as Arc<dyn SecureStorage>,
            on_authorization_url: on_url,
        },
    );

    registry.connect(config.clone()).await.expect("connect ok");

    // The refreshed access token is attached.
    assert_eq!(
        spec_auth_header(&transport.last_spec()).as_deref(),
        Some("Bearer access-2")
    );
    // A refresh_token grant was sent (no interactive flow / no callback).
    let token = mock_as
        .requests()
        .into_iter()
        .find(|r| r.url.contains("/token"))
        .unwrap();
    let refresh_body = token.body.as_deref().unwrap();
    assert!(refresh_body.contains("grant_type=refresh_token"));
    // FIX 1: the refresh carries the PERSISTED (DCR-issued) client_id, not the
    // configured one and NOT an empty string.
    assert!(
        refresh_body.contains("client_id=dcr-issued-7"),
        "refresh must re-send the persisted DCR client_id; body={refresh_body}"
    );
    assert!(!refresh_body.contains("client_id=preset-client"));
    assert!(!refresh_body.contains("client_id=&") && !refresh_body.ends_with("client_id="));
    // New tokens persisted — including the client_id, so the NEXT refresh re-sends it.
    let reloaded = oauth::load_tokens(&(storage as Arc<dyn SecureStorage>), &key)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reloaded.access_token, "access-2");
    assert_eq!(reloaded.client_id.as_deref(), Some("dcr-issued-7"));
}

#[tokio::test]
async fn connect_401_triggers_refresh_and_retry() {
    let refresh = r#"{"access_token":"access-3","refresh_token":"refresh-3","expires_in":3600}"#;
    let mock_as = MockAs::new("{}", refresh);
    // First connect attempt 401s; the second (post-refresh) succeeds.
    let transport = RecordingTransport::new(1);
    let storage = MemStorage::new();
    let clock = TestClock::new(1_000);
    let (on_url, _rx) = url_capture();

    let config = http_cfg("flaky", Some(oauth_block(Some("preset-client"))));
    let key = oauth::server_key("flaky", &config.spec);

    // Seed an UNEXPIRED token (so the first connect uses it and 401s) + refresh
    // + a persisted DCR client_id (FIX 1: the 401-reauth path re-sends it too).
    let stored = oauth::StoredTokens {
        access_token: "access-old".into(),
        refresh_token: Some("refresh-1".into()),
        expires_at_unix: 99_999,
        client_id: Some("dcr-issued-7".into()),
    };
    let bytes = serde_json::to_vec(&stored).unwrap();
    let data = SecureStorageData::new(
        bytes,
        SecureStorageMetadata {
            created_at: SystemTime::UNIX_EPOCH,
            last_accessed: None,
            kind: SecretKindDto("mcp_oauth_tokens".into()),
        },
    );
    storage
        .store(oauth::MCP_OAUTH_SERVICE, &key, data)
        .await
        .unwrap();

    let registry = McpRegistry::new(transport.clone() as Arc<dyn McpTransport>).with_oauth(
        OAuthDeps {
            http: mock_as.clone() as Arc<dyn HttpTransport>,
            clock: clock as Arc<dyn Clock>,
            storage: storage as Arc<dyn SecureStorage>,
            on_authorization_url: on_url,
        },
    );

    registry.connect(config).await.expect("connect ok after retry");

    // Two connect attempts: stale Bearer (401), then refreshed Bearer.
    assert_eq!(transport.connect_count(), 2);
    assert_eq!(
        spec_auth_header(&transport.last_spec()).as_deref(),
        Some("Bearer access-3")
    );
    assert!(mock_as.requests().into_iter().any(|r| {
        r.url.contains("/token")
            && r.body
                .as_deref()
                .map(|b| {
                    b.contains("grant_type=refresh_token")
                        // FIX 1: 401-reauth refresh re-sends the persisted client_id.
                        && b.contains("client_id=dcr-issued-7")
                })
                .unwrap_or(false)
    }));
}
