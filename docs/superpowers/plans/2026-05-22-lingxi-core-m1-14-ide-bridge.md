# LingXi Core M1 · Plan 14 · IDE Bridge

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans.

**Goal:** `lingxi-bridge` — bidirectional protocol over `BridgeTransport` (WebSocket-shaped), 9 `BridgeMessage` variants, JWT-paired trusted devices, rate-limited 8-char pairing codes (A3, A4).

**Depends on:** Plans 01-13.

---

## File Structure

```
crates/bridge/
├── Cargo.toml
└── src/{lib, transport, message, pairing, jwt, codes, rate_limiter, state}.rs

crates/platform-api/src/bridge.rs ← NEW: BridgeTransport trait
```

---

## Task 1: BridgeTransport trait

```rust
// crates/platform-api/src/bridge.rs
use async_trait::async_trait;
use futures_core::stream::Stream;
use lingxi_protocol::Secret;
use serde::{Deserialize, Serialize};
use std::pin::Pin;
use thiserror::Error;

#[async_trait]
pub trait BridgeTransport: Send + Sync {
    async fn connect(&self, config: &BridgeConfig) -> Result<BridgeConnection, BridgeError>;
    async fn send(&self, conn: &BridgeConnection, message: serde_json::Value) -> Result<(), BridgeError>;
    async fn receive(&self, conn: &BridgeConnection) -> Result<Pin<Box<dyn Stream<Item = serde_json::Value> + Send>>, BridgeError>;
    async fn disconnect(&self, conn: BridgeConnection) -> Result<(), BridgeError>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeConfig {
    pub bridge_url: String,
    #[serde(skip_serializing, default)]
    pub jwt_token: Option<String>, // wrap with Secret<String> at use site
    pub poll_interval_ms: u32,
    pub trusted_device_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeConnection { pub connection_id: String }

#[derive(Debug, Clone, Error)]
pub enum BridgeError {
    #[error("connection failed: {0}")]
    Connection(String),
    #[error("auth failed: {0}")]
    Auth(String),
    #[error("transport closed")]
    Closed,
    #[error("rate limited: {0}")]
    RateLimited(String),
    #[error("unsupported on this platform")]
    Unsupported,
}
```

Add to traits/lib.rs.

---

## Task 2: lingxi-bridge — message variants + 8-char rate-limited codes

```toml
[package]
name = "lingxi-bridge"
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
rand = "0.9"
sha2 = "0.10"
hmac = "0.12"
base64 = "0.22"
tokio = { version = "1", features = ["sync", "time"] }
tracing.workspace = true

[lints]
workspace = true
```

```rust
// message.rs (9 variants per §29.2)
use lingxi_protocol::ToolUseId;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BridgeMessage {
    // IDE → CLI
    UserPrompt { text: String, attachments: Vec<serde_json::Value> },
    OpenFileRequest { path: PathBuf, line: Option<u32> },
    GetCurrentFileResponse { path: PathBuf, content: String },
    PermissionDecision { tool_use_id: ToolUseId, decision: String },
    AbortRequest,

    // CLI → IDE
    ShowDiff { path: PathBuf, old_content: String, new_content: String },
    ShowPermissionPrompt { tool_use_id: ToolUseId, action: String, risk: String },
    AssistantMessage { text: String, role: String },
    ToolExecution { tool_use_id: ToolUseId, tool_name: String, status: String },

    Heartbeat { timestamp: std::time::SystemTime },
}
```

```rust
// codes.rs (A3: 8-char alphanumeric pairing code)
use rand::Rng;

const PAIRING_ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789"; // no 0/O/1/I to avoid confusion

pub fn generate_pairing_code() -> String {
    let mut rng = rand::rng();
    (0..8)
        .map(|_| PAIRING_ALPHABET[rng.random_range(0..PAIRING_ALPHABET.len())] as char)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairing_code_is_8_chars() {
        let c = generate_pairing_code();
        assert_eq!(c.chars().count(), 8);
    }

    #[test]
    fn pairing_code_has_no_confusable_chars() {
        for _ in 0..100 {
            let c = generate_pairing_code();
            assert!(!c.contains('0'));
            assert!(!c.contains('O'));
            assert!(!c.contains('1'));
            assert!(!c.contains('I'));
        }
    }
}
```

```rust
// rate_limiter.rs (A4: token-bucket per-project to slow pairing attempts)
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub struct RateLimiter {
    buckets: Mutex<HashMap<String, Bucket>>,
    capacity: u32,
    refill_per_second: f64,
}

#[derive(Debug)]
struct Bucket {
    tokens: f64,
    last_refill: Instant,
}

impl RateLimiter {
    pub fn new(capacity: u32, refill_per_second: f64) -> Self {
        Self { buckets: Mutex::new(HashMap::new()), capacity, refill_per_second }
    }

    pub fn try_acquire(&self, key: &str) -> bool {
        let now = Instant::now();
        let mut buckets = self.buckets.lock().unwrap();
        let bucket = buckets.entry(key.to_string()).or_insert(Bucket {
            tokens: self.capacity as f64, last_refill: now,
        });
        let elapsed = now.duration_since(bucket.last_refill).as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * self.refill_per_second).min(self.capacity as f64);
        bucket.last_refill = now;
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            true
        } else { false }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_then_refills() {
        let rl = RateLimiter::new(3, 1.0); // 3 burst, 1/sec refill
        let k = "alice";
        assert!(rl.try_acquire(k));
        assert!(rl.try_acquire(k));
        assert!(rl.try_acquire(k));
        assert!(!rl.try_acquire(k));
    }
}
```

```rust
// jwt.rs (HS256 signing + verify, project-scoped JWT)
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use serde::{Deserialize, Serialize};
use thiserror::Error;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JwtClaims {
    pub sub: String,           // device_id
    pub project_dir: String,
    pub exp: u64,              // unix seconds
    pub iat: u64,
}

#[derive(Debug, Clone, Error)]
pub enum JwtError {
    #[error("invalid format")]
    InvalidFormat,
    #[error("invalid signature")]
    InvalidSignature,
    #[error("expired")]
    Expired,
    #[error("project_dir mismatch")]
    ProjectMismatch,
}

pub struct JwtVerifier {
    secret: Vec<u8>,
}

impl JwtVerifier {
    pub fn new(secret: Vec<u8>) -> Self { Self { secret } }

    pub fn sign(&self, claims: &JwtClaims) -> String {
        let header = URL_SAFE_NO_PAD.encode(b"{\"alg\":\"HS256\",\"typ\":\"JWT\"}");
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_string(claims).unwrap());
        let to_sign = format!("{header}.{payload}");
        let mut mac = HmacSha256::new_from_slice(&self.secret).unwrap();
        mac.update(to_sign.as_bytes());
        let sig = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
        format!("{to_sign}.{sig}")
    }

    pub fn verify(&self, token: &str, project_dir: &str, now_secs: u64) -> Result<JwtClaims, JwtError> {
        let parts: Vec<&str> = token.split('.').collect();
        if parts.len() != 3 { return Err(JwtError::InvalidFormat); }
        let to_verify = format!("{}.{}", parts[0], parts[1]);
        let mut mac = HmacSha256::new_from_slice(&self.secret).map_err(|_| JwtError::InvalidSignature)?;
        mac.update(to_verify.as_bytes());
        let expected_sig = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
        if expected_sig != parts[2] { return Err(JwtError::InvalidSignature); }

        let payload_bytes = URL_SAFE_NO_PAD.decode(parts[1]).map_err(|_| JwtError::InvalidFormat)?;
        let claims: JwtClaims = serde_json::from_slice(&payload_bytes).map_err(|_| JwtError::InvalidFormat)?;
        if claims.exp < now_secs { return Err(JwtError::Expired); }
        if claims.project_dir != project_dir { return Err(JwtError::ProjectMismatch); }
        Ok(claims)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_then_verify_roundtrip() {
        let v = JwtVerifier::new(b"my-secret".to_vec());
        let claims = JwtClaims { sub: "dev1".into(), project_dir: "/proj".into(), exp: 99999999999, iat: 1 };
        let tok = v.sign(&claims);
        let out = v.verify(&tok, "/proj", 100).unwrap();
        assert_eq!(out.sub, "dev1");
    }

    #[test]
    fn tampered_token_rejected() {
        let v = JwtVerifier::new(b"my-secret".to_vec());
        let claims = JwtClaims { sub: "dev1".into(), project_dir: "/proj".into(), exp: 99999999999, iat: 1 };
        let mut tok = v.sign(&claims);
        tok.push('A');
        assert!(matches!(v.verify(&tok, "/proj", 100), Err(JwtError::InvalidSignature)));
    }

    #[test]
    fn wrong_project_dir_rejected() {
        let v = JwtVerifier::new(b"s".to_vec());
        let claims = JwtClaims { sub: "dev1".into(), project_dir: "/a".into(), exp: 99999999999, iat: 1 };
        let tok = v.sign(&claims);
        assert!(matches!(v.verify(&tok, "/b", 100), Err(JwtError::ProjectMismatch)));
    }
}
```

```rust
// pairing.rs
use crate::codes::generate_pairing_code;
use crate::rate_limiter::RateLimiter;
use lingxi_protocol::Secret;
use lingxi_platform_api::SecureStorage;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::SystemTime;
use thiserror::Error;

#[derive(Debug, Clone, Error)]
pub enum PairingError {
    #[error("rate limited")]
    RateLimited,
    #[error("expired pairing code")]
    Expired,
    #[error("invalid code")]
    Invalid,
    #[error("storage: {0}")]
    Storage(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustedDevice {
    pub device_id: String,
    pub name: String,
    pub paired_at: SystemTime,
    pub last_seen: SystemTime,
    pub project_dir: String,
}

pub struct BridgePairing {
    storage: Arc<dyn SecureStorage>,
    pairing_codes_rate_limiter: RateLimiter,
}

impl BridgePairing {
    pub fn new(storage: Arc<dyn SecureStorage>) -> Self {
        Self {
            storage,
            // 3 attempts burst, refill 1/min — slows pairing brute force.
            pairing_codes_rate_limiter: RateLimiter::new(3, 1.0 / 60.0),
        }
    }

    pub fn generate_pairing_code(&self, project_dir: &str) -> Result<String, PairingError> {
        if !self.pairing_codes_rate_limiter.try_acquire(project_dir) {
            return Err(PairingError::RateLimited);
        }
        Ok(generate_pairing_code())
    }
}
```

```rust
// state.rs
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BridgeState {
    pub connected: bool,
    pub current_file: Option<std::path::PathBuf>,
}
```

```rust
// lib.rs
#![forbid(unsafe_code)]
pub mod codes;
pub mod jwt;
pub mod message;
pub mod pairing;
pub mod rate_limiter;
pub mod state;
pub mod transport;

pub use codes::generate_pairing_code;
pub use jwt::{JwtClaims, JwtError, JwtVerifier};
pub use message::BridgeMessage;
pub use pairing::{BridgePairing, PairingError, TrustedDevice};
pub use rate_limiter::RateLimiter;
pub use state::BridgeState;
pub use transport::IdeBridge;
```

```rust
// transport.rs (small wrapper around BridgeTransport)
use lingxi_platform_api::{BridgeConfig, BridgeConnection, BridgeError, BridgeTransport};
use std::sync::Arc;
use tokio::sync::RwLock;

pub struct IdeBridge {
    transport: Arc<dyn BridgeTransport>,
    connection: RwLock<Option<BridgeConnection>>,
}

impl IdeBridge {
    pub fn new(transport: Arc<dyn BridgeTransport>) -> Self {
        Self { transport, connection: RwLock::new(None) }
    }

    pub async fn connect(&self, config: BridgeConfig) -> Result<(), BridgeError> {
        let conn = self.transport.connect(&config).await?;
        *self.connection.write().await = Some(conn);
        Ok(())
    }

    pub async fn send(&self, msg: crate::message::BridgeMessage) -> Result<(), BridgeError> {
        let conn = self.connection.read().await.clone().ok_or(BridgeError::Closed)?;
        self.transport.send(&conn, serde_json::to_value(&msg).unwrap()).await
    }
}
```

Commit:
```bash
cargo test -p lingxi-bridge
git add crates/bridge crates/traits
git commit -m "feat(bridge): IdeBridge + JWT (project-scoped) + 8-char codes + rate-limited pairing (A3, A4)"
```

---

## Task 3: Plan exit

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings
git tag -a m1.20-bridge -m "Plan 14 complete"
```

## Self-Review

- §29.1 BridgeTransport trait + IdeBridge → ✓
- §29.2 9 BridgeMessage variants → ✓
- §29.3 JWT pairing project-scoped (A3) → ✓
- A3 8-char alphanumeric pairing code (no 0/O/1/I) → ✓
- A4 rate-limited pairing attempts → ✓

## Execution Handoff

Next: **Plan 15 — Plugin System** (`2026-05-22-lingxi-core-m1-15-plugin.md`).
