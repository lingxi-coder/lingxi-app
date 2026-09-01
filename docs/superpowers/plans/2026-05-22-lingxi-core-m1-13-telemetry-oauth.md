# LingXi Core M1 · Plan 13 · Telemetry + Anthropic OAuth

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans.

**Goal:** `lingxi-telemetry` (AnalyticsBus + AnalyticsSink trait + FeatureFlagsClient + PII marker newtypes + killswitch) and `lingxi-anthropic-oauth` (ClaudeAiOAuthClient with PKCE/state/loopback A5, AnthropicAuthResolver with 9 sources A6, ClaudeAiLimitsTracker).

**Depends on:** Plans 01-12.

---

## File Structure

```
crates/telemetry/
├── Cargo.toml
└── src/{lib, sink, bus, pii, feature_flags, killswitch}.rs

crates/anthropic-oauth/
├── Cargo.toml
└── src/{lib, client, config, resolver, limits, pkce, callback}.rs
```

---

## Task 1: lingxi-telemetry

```toml
[package]
name = "lingxi-telemetry"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[dependencies]
lingxi-protocol = { path = "../protocol" }
lingxi-platform-api = { path = "../platform-api" }
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true
async-trait.workspace = true
tokio = { version = "1", features = ["sync"] }
tracing.workspace = true
```

```rust
// sink.rs
use async_trait::async_trait;
use std::collections::HashMap;

pub type LogEventMetadata = HashMap<String, AnalyticsValue>;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum AnalyticsValue {
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    None,
}

#[async_trait]
pub trait AnalyticsSink: Send + Sync {
    async fn log_event(&self, name: &str, metadata: LogEventMetadata);
    async fn log_event_async(&self, name: &str, metadata: LogEventMetadata);
    fn name(&self) -> &str;
}
```

```rust
// pii.rs (A8 — real newtypes, not type aliases)
use serde::{Deserialize, Serialize};

/// Marker newtype: caller asserts the inner string is NOT code or filepaths.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Verified(String);
impl Verified {
    pub fn assert_safe(value: String) -> Self { Self(value) }
    pub fn into_inner(self) -> String { self.0 }
    pub fn as_str(&self) -> &str { &self.0 }
}

/// Marker newtype: routes to privileged BQ proto columns via `_PROTO_*` keys.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PiiTagged(String);
impl PiiTagged {
    pub fn assert_pii_tagged_column(value: String) -> Self { Self(value) }
    pub fn into_inner(self) -> String { self.0 }
}

/// Strip `_PROTO_*` keys from a payload destined for general-access sinks.
pub fn strip_proto_fields(metadata: &mut crate::sink::LogEventMetadata) {
    metadata.retain(|k, _| !k.starts_with("_PROTO_"));
}
```

```rust
// killswitch.rs
use std::sync::atomic::{AtomicBool, Ordering};

pub struct Killswitch {
    active: AtomicBool,
}

impl Killswitch {
    pub fn new() -> Self { Self { active: AtomicBool::new(false) } }
    pub fn is_active(&self) -> bool { self.active.load(Ordering::Acquire) }
    pub fn activate(&self) { self.active.store(true, Ordering::Release); }
}

impl Default for Killswitch { fn default() -> Self { Self::new() } }
```

```rust
// bus.rs
use crate::killswitch::Killswitch;
use crate::sink::{AnalyticsSink, LogEventMetadata};
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

pub struct AnalyticsBus {
    sink: RwLock<Option<Arc<dyn AnalyticsSink>>>,
    pending: Mutex<VecDeque<QueuedEvent>>,
    max_pending: usize,
    killswitch: Killswitch,
}

#[derive(Debug, Clone)]
struct QueuedEvent {
    name: String,
    metadata: LogEventMetadata,
    is_async: bool,
}

impl AnalyticsBus {
    pub fn new() -> Self {
        Self {
            sink: RwLock::new(None),
            pending: Mutex::new(VecDeque::new()),
            max_pending: 1000,
            killswitch: Killswitch::new(),
        }
    }

    /// Sync log. If no sink attached, buffer.
    pub async fn log_event(&self, name: &str, metadata: LogEventMetadata) {
        if self.killswitch.is_active() { return; }
        if let Some(sink) = self.sink.read().await.as_ref() {
            sink.log_event(name, metadata).await;
        } else {
            self.buffer(QueuedEvent { name: name.into(), metadata, is_async: false }).await;
        }
    }

    pub async fn attach_sink(&self, sink: Arc<dyn AnalyticsSink>) {
        *self.sink.write().await = Some(sink.clone());
        // Drain pending.
        let mut pending = self.pending.lock().await;
        while let Some(e) = pending.pop_front() {
            sink.log_event(&e.name, e.metadata).await;
        }
    }

    pub fn killswitch(&self) -> &Killswitch { &self.killswitch }

    async fn buffer(&self, e: QueuedEvent) {
        let mut p = self.pending.lock().await;
        if p.len() >= self.max_pending { p.pop_front(); }
        p.push_back(e);
    }
}

impl Default for AnalyticsBus { fn default() -> Self { Self::new() } }
```

```rust
// feature_flags.rs
use lingxi_platform_api::{RuntimeError, RuntimeSpawner};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum FeatureValue { Bool(bool), Number(f64), String(String), Json(serde_json::Value) }

#[async_trait::async_trait]
pub trait FeatureFlagsFetcher: Send + Sync {
    async fn fetch(&self) -> Result<HashMap<String, FeatureValue>, String>;
}

pub struct FeatureFlagsClient {
    cache: RwLock<HashMap<String, FeatureValue>>,
    cache_ttl: Duration,
    fetcher: Arc<dyn FeatureFlagsFetcher>,
    runtime: Arc<dyn RuntimeSpawner>,
}

impl FeatureFlagsClient {
    pub fn new(fetcher: Arc<dyn FeatureFlagsFetcher>, runtime: Arc<dyn RuntimeSpawner>) -> Self {
        Self { cache: RwLock::new(HashMap::new()), cache_ttl: Duration::from_secs(300), fetcher, runtime }
    }

    pub async fn get_bool(&self, key: &str, default: bool) -> bool {
        match self.cache.read().await.get(key) {
            Some(FeatureValue::Bool(b)) => *b,
            _ => default,
        }
    }

    pub async fn start_refresh_loop(self: Arc<Self>) -> Result<(), RuntimeError> {
        let me = self.clone();
        self.runtime.spawn("feature-flags-refresh", Box::pin(async move {
            loop {
                if let Ok(values) = me.fetcher.fetch().await {
                    *me.cache.write().await = values;
                }
                tokio::time::sleep(me.cache_ttl).await;
            }
        })).await?;
        Ok(())
    }
}
```

```rust
// lib.rs
#![forbid(unsafe_code)]
pub mod bus;
pub mod feature_flags;
pub mod killswitch;
pub mod pii;
pub mod sink;

pub use bus::AnalyticsBus;
pub use feature_flags::{FeatureFlagsClient, FeatureFlagsFetcher, FeatureValue};
pub use killswitch::Killswitch;
pub use pii::{strip_proto_fields, PiiTagged, Verified};
pub use sink::{AnalyticsSink, AnalyticsValue, LogEventMetadata};
```

Commit:
```bash
cargo test -p lingxi-telemetry
git add crates/telemetry
git commit -m "feat(telemetry): bus + sink trait + PII newtypes + feature flags + killswitch"
```

---

## Task 2: lingxi-anthropic-oauth

```toml
[package]
name = "lingxi-anthropic-oauth"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[dependencies]
lingxi-protocol = { path = "../protocol" }
lingxi-platform-api = { path = "../platform-api" }
lingxi-secret = { path = "../secret" }
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true
async-trait.workspace = true
url = "2"
base64 = "0.22"
rand = "0.9"
sha2 = "0.10"
tokio = { version = "1", features = ["sync", "time"] }
tracing.workspace = true

[lints]
workspace = true
```

```rust
// pkce.rs (A5)
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::Rng;
use sha2::{Digest, Sha256};

/// (code_verifier, code_challenge) — challenge is sent in the authorize URL,
/// verifier is held by us until token exchange.
pub fn generate_pkce() -> (String, String) {
    let mut rng = rand::rng();
    let bytes: [u8; 32] = std::array::from_fn(|_| rng.random());
    let verifier = URL_SAFE_NO_PAD.encode(bytes);

    let mut h = Sha256::new();
    h.update(verifier.as_bytes());
    let challenge = URL_SAFE_NO_PAD.encode(h.finalize());

    (verifier, challenge)
}

pub fn generate_state_token() -> String {
    let mut rng = rand::rng();
    let bytes: [u8; 16] = std::array::from_fn(|_| rng.random());
    URL_SAFE_NO_PAD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifier_and_challenge_distinct_per_call() {
        let (v1, _) = generate_pkce();
        let (v2, _) = generate_pkce();
        assert_ne!(v1, v2);
    }

    #[test]
    fn challenge_is_url_safe_base64() {
        let (_, c) = generate_pkce();
        assert!(!c.contains('+'));
        assert!(!c.contains('/'));
        assert!(!c.contains('='));
    }
}
```

```rust
// config.rs
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaudeAiOAuthConfig {
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub revocation_endpoint: String,
    pub profile_endpoint: String,
    pub client_id: String,
    pub redirect_uri: String,
    pub scopes: Vec<String>,
}

impl ClaudeAiOAuthConfig {
    /// Default config — endpoints per claude-code reference.
    pub fn default_with_port(port: u16) -> Self {
        Self {
            authorization_endpoint: "https://login.claude.ai/oauth/authorize".into(),
            token_endpoint: "https://login.claude.ai/oauth/token".into(),
            revocation_endpoint: "https://login.claude.ai/oauth/revoke".into(),
            profile_endpoint: "https://api.claude.ai/v1/me".into(),
            client_id: "lingxi-core".into(),
            redirect_uri: format!("http://127.0.0.1:{port}/callback"),
            scopes: vec!["profile".into(), "messages".into()],
        }
    }
}
```

```rust
// limits.rs
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SubscriptionType { Free, Pro, Max, Team, Enterprise, Unknown }

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClaudeAiLimitsState {
    pub subscription_type: Option<SubscriptionType>,
    pub message_count_window: u32,
    pub message_limit_window: u32,
    pub window_resets_at: Option<SystemTime>,
    pub extra_usage_dollars: f64,
    pub last_updated: Option<SystemTime>,
}

pub struct ClaudeAiLimitsTracker {
    state: Mutex<ClaudeAiLimitsState>,
}

impl ClaudeAiLimitsTracker {
    pub fn new() -> Self { Self { state: Mutex::new(ClaudeAiLimitsState::default()) } }

    pub fn update_from_headers(&self, headers: &HashMap<String, String>) {
        let mut s = self.state.lock().unwrap();
        if let Some(c) = headers.get("x-claudeai-window-count").and_then(|v| v.parse().ok()) { s.message_count_window = c; }
        if let Some(l) = headers.get("x-claudeai-window-limit").and_then(|v| v.parse().ok()) { s.message_limit_window = l; }
        s.last_updated = Some(SystemTime::now());
    }

    pub fn snapshot(&self) -> ClaudeAiLimitsState { self.state.lock().unwrap().clone() }
}

impl Default for ClaudeAiLimitsTracker { fn default() -> Self { Self::new() } }
```

```rust
// resolver.rs (A6 — concrete priority)
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuthSource {
    /// Highest priority: managed contexts force OAuth.
    OAuthClaudeAi,
    /// Environment variable ANTHROPIC_AUTH_TOKEN (bearer).
    EnvAuthToken,
    /// Environment variable ANTHROPIC_API_KEY.
    EnvApiKey,
    /// File descriptor inheritance (managed launch).
    FileDescriptor,
    /// API key stored via SecureStorage.
    StoredApiKey,
    /// Legacy: settings.json apiKey field.
    SettingsApiKey,
    /// External script returns API key.
    ApiKeyHelper { script_path: PathBuf },
    /// AWS Bedrock credentials.
    AwsBedrock,
    None,
}

pub struct ResolverContext {
    pub managed_oauth_only: bool,           // CCR / Claude Desktop force OAuth
    pub env_auth_token: Option<String>,
    pub env_api_key: Option<String>,
    pub fd_present: bool,
    pub has_stored_oauth: bool,
    pub has_stored_api_key: bool,
    pub settings_api_key: Option<String>,
    pub api_key_helper_script: Option<PathBuf>,
    pub aws_present: bool,
}

pub fn resolve(ctx: &ResolverContext) -> AuthSource {
    if ctx.managed_oauth_only && ctx.has_stored_oauth { return AuthSource::OAuthClaudeAi; }
    if ctx.env_auth_token.is_some() { return AuthSource::EnvAuthToken; }
    if ctx.env_api_key.is_some() { return AuthSource::EnvApiKey; }
    if ctx.fd_present { return AuthSource::FileDescriptor; }
    if ctx.has_stored_oauth { return AuthSource::OAuthClaudeAi; }
    if ctx.has_stored_api_key { return AuthSource::StoredApiKey; }
    if ctx.settings_api_key.is_some() { return AuthSource::SettingsApiKey; }
    if let Some(p) = &ctx.api_key_helper_script { return AuthSource::ApiKeyHelper { script_path: p.clone() }; }
    if ctx.aws_present { return AuthSource::AwsBedrock; }
    AuthSource::None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_oauth_wins() {
        let ctx = ResolverContext {
            managed_oauth_only: true, env_auth_token: Some("x".into()), env_api_key: None,
            fd_present: false, has_stored_oauth: true, has_stored_api_key: false,
            settings_api_key: None, api_key_helper_script: None, aws_present: false,
        };
        assert!(matches!(resolve(&ctx), AuthSource::OAuthClaudeAi));
    }

    #[test]
    fn none_when_no_source() {
        let ctx = ResolverContext {
            managed_oauth_only: false, env_auth_token: None, env_api_key: None,
            fd_present: false, has_stored_oauth: false, has_stored_api_key: false,
            settings_api_key: None, api_key_helper_script: None, aws_present: false,
        };
        assert!(matches!(resolve(&ctx), AuthSource::None));
    }
}
```

```rust
// callback.rs — loopback HTTP listener for OAuth redirect (A5)
use std::net::SocketAddr;
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[derive(Debug, Clone, Error)]
pub enum CallbackError {
    #[error("bind failed: {0}")]
    Bind(String),
    #[error("invalid callback request: {0}")]
    InvalidRequest(String),
    #[error("state mismatch")]
    StateMismatch,
}

pub struct CallbackParams {
    pub code: String,
    pub state: String,
}

/// Bind on 127.0.0.1:port, wait for the single GET /callback?code=...&state=..., return params.
pub async fn await_callback(port: u16, expected_state: &str) -> Result<CallbackParams, CallbackError> {
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    let listener = TcpListener::bind(addr).await.map_err(|e| CallbackError::Bind(e.to_string()))?;
    let (mut stream, _) = listener.accept().await.map_err(|e| CallbackError::Bind(e.to_string()))?;
    let mut buf = vec![0u8; 4096];
    let n = stream.read(&mut buf).await.map_err(|e| CallbackError::InvalidRequest(e.to_string()))?;
    let req = String::from_utf8_lossy(&buf[..n]);

    let line = req.lines().next().ok_or(CallbackError::InvalidRequest("empty request".into()))?;
    let path = line.split_whitespace().nth(1).ok_or(CallbackError::InvalidRequest("no path".into()))?;
    let query = path.split_once('?').map(|(_, q)| q).unwrap_or("");
    let mut code = None;
    let mut state = None;
    for kv in query.split('&') {
        if let Some(v) = kv.strip_prefix("code=") { code = Some(v.to_string()); }
        if let Some(v) = kv.strip_prefix("state=") { state = Some(v.to_string()); }
    }
    let code = code.ok_or(CallbackError::InvalidRequest("missing code".into()))?;
    let state = state.ok_or(CallbackError::InvalidRequest("missing state".into()))?;
    if state != expected_state { return Err(CallbackError::StateMismatch); }

    let body = "Login complete. You can close this window.";
    let resp = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}", body.len(), body);
    let _ = stream.write_all(resp.as_bytes()).await;

    Ok(CallbackParams { code, state })
}
```

```rust
// client.rs (skeleton)
use crate::config::ClaudeAiOAuthConfig;
use crate::pkce::{generate_pkce, generate_state_token};
use lingxi_protocol::Secret;
use lingxi_secret::CredentialManager;
use lingxi_platform_api::HttpTransport;
use std::sync::Arc;
use thiserror::Error;

#[derive(Debug, Clone, Error)]
pub enum OAuthError {
    #[error("callback failed: {0}")]
    Callback(String),
    #[error("token exchange failed: {0}")]
    TokenExchange(String),
}

pub struct ClaudeAiOAuthClient {
    config: ClaudeAiOAuthConfig,
    http: Arc<dyn HttpTransport>,
    credentials: Arc<CredentialManager>,
}

impl ClaudeAiOAuthClient {
    pub fn new(config: ClaudeAiOAuthConfig, http: Arc<dyn HttpTransport>, credentials: Arc<CredentialManager>) -> Self {
        Self { config, http, credentials }
    }

    /// Full impl with browser open + token exchange lands in Plan 16 cli-demo.
    /// M1.19 ships the contract + PKCE/state generation + loopback callback wiring.
    pub fn build_authorize_url(&self) -> (String, String, String) {
        let (verifier, challenge) = generate_pkce();
        let state = generate_state_token();
        let scopes = self.config.scopes.join(" ");
        let url = format!(
            "{}?response_type=code&client_id={}&redirect_uri={}&scope={}&state={}&code_challenge={}&code_challenge_method=S256",
            self.config.authorization_endpoint,
            urlencoding::encode(&self.config.client_id),
            urlencoding::encode(&self.config.redirect_uri),
            urlencoding::encode(&scopes),
            urlencoding::encode(&state),
            urlencoding::encode(&challenge),
        );
        (url, verifier, state)
    }
}
```

(Add `urlencoding = "2"` to deps.)

```rust
// lib.rs
#![forbid(unsafe_code)]
pub mod callback;
pub mod client;
pub mod config;
pub mod limits;
pub mod pkce;
pub mod resolver;

pub use callback::{await_callback, CallbackError, CallbackParams};
pub use client::{ClaudeAiOAuthClient, OAuthError};
pub use config::ClaudeAiOAuthConfig;
pub use limits::{ClaudeAiLimitsState, ClaudeAiLimitsTracker, SubscriptionType};
pub use pkce::{generate_pkce, generate_state_token};
pub use resolver::{resolve, AuthSource, ResolverContext};
```

Commit:
```bash
cargo test -p lingxi-anthropic-oauth
git add crates/anthropic-oauth
git commit -m "feat(anthropic-oauth): PKCE + state + loopback callback + auth resolver"
```

---

## Task 3: Plan exit

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings
git tag -a m1.19-telemetry-oauth -m "Plan 13 complete"
```

## Self-Review

- §26.1 AnalyticsSink trait → ✓
- §26.2 PII newtypes (A8) → pii.rs ✓
- §26.3 AnalyticsBus with buffering + killswitch → bus.rs ✓
- §26.4 FeatureFlagsClient → feature_flags.rs ✓
- §30.1 ClaudeAiOAuthConfig + endpoints → config.rs ✓
- §30.2 AnthropicAuthResolver with 9 AuthSource (A6) → resolver.rs ✓
- §30.3 PKCE + state + loopback (A5) → pkce.rs + callback.rs ✓
- §30.4 ClaudeAiLimitsTracker → limits.rs ✓

## Execution Handoff

Next: **Plan 14 — IDE Bridge** (`2026-05-22-lingxi-core-m1-14-ide-bridge.md`).
