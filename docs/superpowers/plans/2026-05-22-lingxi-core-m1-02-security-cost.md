# LingXi Core M1 · Plan 02 · Security & Cost

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add three foundational gating crates — `lingxi-permission`, `lingxi-secret`, `lingxi-cost` — so every API call is gated by budget and permission, and secrets are stored in a type-safe `Secret<T>` over a `SecureStorage` backend.

**Architecture:** Three independent crates wired into the existing reducer via new Event/Effect variants. Permission engine resolves rules from 8 sources by priority. Secret crate wraps `secrecy::SecretBox<T>` with `Zeroize`-on-drop. Cost crate uses `u64 nano_usd` integer arithmetic against a `PricingCatalog` keyed by `(ProviderId, model)`.

**Tech Stack:** `secrecy` (Zeroize-on-drop), `zeroize`, `regex` (rule patterns), `serde`, `proptest`.

**References:** Spec §14 Permission · §16 Secret · §17 Cost · §4.9 SecureStorage trait · D18-D20 (decisions)

**Depends on:** Plan 01 complete (protocol, core, traits exist; mock infrastructure available).

---

## File Structure

```
crates/permission/                    ← §14
├── Cargo.toml
└── src/
    ├── lib.rs
    ├── mode.rs                       ← 5 external + 2 internal PermissionMode
    ├── rule.rs                       ← PermissionRule + 8 RuleSource
    ├── result.rs                     ← PermissionResult + PermissionDecisionReason
    ├── policy.rs                     ← PermissionPolicy::authorize
    ├── classifier.rs                 ← PermissionClassifier trait + ClassifierKind
    ├── dangerous_patterns.rs         ← static bash regex table
    ├── denial_tracking.rs            ← DenialTrackingState + thresholds
    ├── shadow.rs                     ← ShadowedRuleDetector
    └── update.rs                     ← PermissionUpdate

crates/secret/                        ← §16
├── Cargo.toml
└── src/
    ├── lib.rs
    ├── credential.rs                 ← CredentialManager + OAuth refresh-lock
    ├── keychain_prefetch.rs          ← KeychainPrefetch
    ├── scanner.rs                    ← SecretScanner + 30+ gitleaks rules
    ├── redaction.rs                  ← RedactionPolicy + 5 boundaries
    └── kinds.rs                      ← SecretKind enum

crates/cost/                          ← §17
├── Cargo.toml
└── src/
    ├── lib.rs
    ├── pricing.rs                    ← PricingCatalog + ModelPricing + TokenClass
    ├── usage.rs                      ← Usage + TokenUsage + ApiSpeed
    ├── calculator.rs                 ← CostCalculator (nano_usd, cache savings)
    ├── tracker.rs                    ← CostTracker (single-writer task)
    └── budget.rs                     ← BudgetEnforcer + realized_exceeded latch

crates/protocol/src/secret.rs         ← NEW: Secret<T>, SecureStorageData, RedactableContent
crates/protocol/src/effects.rs        ← MODIFY: extend Effect with new variants
crates/core/src/events.rs             ← MODIFY: extend Event with new variants
crates/platform-api/src/secure_storage.rs   ← NEW: SecureStorage trait
crates/test-harness/src/mocks/        ← NEW: mock_secure_storage.rs
```

---

## Task 1: Extend `lingxi-protocol` with secret DTOs

**Files:**
- Create: `crates/protocol/src/secret.rs`
- Modify: `crates/protocol/src/lib.rs`
- Modify: `crates/protocol/Cargo.toml`

- [ ] **Step 1: Add `secrecy` + `zeroize` deps**

Edit `crates/protocol/Cargo.toml`:

```toml
[dependencies]
# ... existing ...
secrecy = { version = "0.10", features = ["serde"] }
zeroize = "1.8"
```

- [ ] **Step 2: Write `Secret<T>` wrapper with tests**

```rust
// crates/protocol/src/secret.rs
//! Secret newtype + redactable DTOs.
//!
//! `Secret<T>` is a thin wrapper around `secrecy::SecretBox<T>`. The inner
//! buffer is zeroized on drop. Debug/Display always print `<redacted>`.
//! Code review can grep `expose_secret(` to audit every read.

use secrecy::{ExposeSecret, SecretBox};
use serde::{Deserialize, Serialize};
use std::fmt;
use zeroize::Zeroize;

/// Wrap any `Zeroize` type so it cannot leak via Debug/Display/Clone.
pub struct Secret<T: Zeroize>(SecretBox<T>);

impl<T: Zeroize + Default> Secret<T> {
    pub fn new(value: T) -> Self { Self(SecretBox::new(Box::new(value))) }
    pub fn expose_secret(&self) -> &T { self.0.expose_secret() }
}

impl<T: Zeroize> fmt::Debug for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Secret(<redacted>)")
    }
}

impl<T: Zeroize> fmt::Display for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<redacted>")
    }
}

// Explicit serde delegation so `Secret<T>` round-trips when the user opts in.
// We intentionally do NOT derive Clone — clones must be deliberate.
impl<T: Serialize + Zeroize> Serialize for Secret<T> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.0.expose_secret().serialize(s)
    }
}
impl<'de, T: Deserialize<'de> + Zeroize + Default> Deserialize<'de> for Secret<T> {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        T::deserialize(d).map(Self::new)
    }
}

#[derive(Clone)]
pub struct SecureStorageData {
    bytes: Secret<Vec<u8>>,
    pub metadata: SecureStorageMetadata,
}

impl fmt::Debug for SecureStorageData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SecureStorageData")
            .field("bytes", &"<redacted>")
            .field("metadata", &self.metadata)
            .finish()
    }
}

impl SecureStorageData {
    pub fn new(bytes: Vec<u8>, metadata: SecureStorageMetadata) -> Self {
        Self { bytes: Secret::new(bytes), metadata }
    }
    pub fn expose_secret_bytes(&self) -> &[u8] { self.bytes.expose_secret() }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecureStorageMetadata {
    pub created_at: std::time::SystemTime,
    pub last_accessed: Option<std::time::SystemTime>,
    pub kind: SecretKindDto,
}

/// String form of SecretKind for DTOs (avoids cycle with lingxi-secret).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretKindDto(pub String);

/// Content that may contain user secrets — pre-redaction.
#[derive(Clone)]
pub struct RedactableContent(String);

impl fmt::Debug for RedactableContent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RedactableContent(<{}B>)", self.0.len())
    }
}

impl RedactableContent {
    pub fn new(content: String) -> Self { Self(content) }
    pub fn expose_for_scan(&self) -> &str { &self.0 }
    pub fn len(&self) -> usize { self.0.len() }
    pub fn is_empty(&self) -> bool { self.0.is_empty() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_debug_redacts() {
        let s = Secret::new("sk-ant-superhot".to_string());
        let dbg = format!("{s:?}");
        assert!(!dbg.contains("sk-ant"), "leaked: {dbg}");
        assert!(dbg.contains("redacted"));
    }

    #[test]
    fn redactable_content_debug_redacts() {
        let r = RedactableContent::new("very_secret".into());
        assert!(!format!("{r:?}").contains("very_secret"));
    }

    #[test]
    fn secret_storage_data_debug_redacts() {
        let data = SecureStorageData::new(
            b"hot-bytes".to_vec(),
            SecureStorageMetadata {
                created_at: std::time::SystemTime::UNIX_EPOCH,
                last_accessed: None,
                kind: SecretKindDto("anthropic_api_key".into()),
            },
        );
        assert!(!format!("{data:?}").contains("hot-bytes"));
    }
}
```

- [ ] **Step 3: Re-export from lib.rs**

Edit `crates/protocol/src/lib.rs`, add `pub mod secret;` and:

```rust
pub use secret::{
    RedactableContent, Secret, SecretKindDto, SecureStorageData, SecureStorageMetadata,
};
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p lingxi-protocol --lib secret
```

Expected: 3 tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/protocol
git commit -m "feat(protocol): add Secret<T>, SecureStorageData, RedactableContent (Zeroize-on-drop)"
```

---

## Task 2: SecureStorage trait

**Files:**
- Create: `crates/platform-api/src/secure_storage.rs`
- Modify: `crates/platform-api/src/lib.rs`

- [ ] **Step 1: Implement trait + error**

```rust
// crates/platform-api/src/secure_storage.rs
use async_trait::async_trait;
use lingxi_protocol::SecureStorageData;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[async_trait]
pub trait SecureStorage: Send + Sync {
    async fn store(
        &self,
        service: &str,
        account: &str,
        data: SecureStorageData,
    ) -> Result<(), SecureStorageError>;

    async fn retrieve(
        &self,
        service: &str,
        account: &str,
    ) -> Result<Option<SecureStorageData>, SecureStorageError>;

    async fn delete(&self, service: &str, account: &str) -> Result<(), SecureStorageError>;

    async fn list(&self, service: &str) -> Result<Vec<String>, SecureStorageError>;

    fn is_encrypted(&self) -> bool;
    fn backend(&self) -> SecureStorageBackend;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SecureStorageBackend {
    MacOsKeychain,
    LinuxLibsecret,
    WindowsCredVault,
    AndroidKeystore,
    IosKeychain,
    EncryptedFile,
    PlainText,
}

#[derive(Debug, Clone, Error)]
pub enum SecureStorageError {
    #[error("not found: {service}/{account}")]
    NotFound { service: String, account: String },
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    #[error("backend unavailable: {0}")]
    BackendUnavailable(String),
    #[error("io error: {0}")]
    Io(String),
}
```

- [ ] **Step 2: Re-export + run check**

Add `pub mod secure_storage;` + `pub use secure_storage::*;` to `platform-api/src/lib.rs`.

```bash
cargo check -p lingxi-traits
```

- [ ] **Step 3: Commit**

```bash
git add crates/traits
git commit -m "feat(traits): add SecureStorage trait"
```

---

## Task 3: lingxi-secret crate — SecretKind + Scanner + RedactionPolicy

**Files:**
- Create: `crates/secret/Cargo.toml`
- Create: `crates/secret/src/{lib,kinds,scanner,redaction}.rs`

- [ ] **Step 1: Cargo.toml**

```toml
[package]
name = "lingxi-secret"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[dependencies]
lingxi-protocol = { path = "../protocol" }
lingxi-platform-api = { path = "../platform-api" }
serde.workspace = true
thiserror.workspace = true
regex = "1"
async-trait.workspace = true
tracing.workspace = true

[lints]
workspace = true
```

- [ ] **Step 2: kinds.rs**

```rust
//! Domain-typed labels for secrets in storage.
use lingxi_protocol::SecretKindDto;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SecretKind {
    AnthropicApiKey,
    AnthropicOAuthAccessToken,
    AnthropicOAuthRefreshToken,
    AwsCredentials,
    McpOAuthAccessToken { server: String },
    McpOAuthRefreshToken { server: String },
    GenericApiKey { provider: String },
}

impl SecretKind {
    pub fn as_dto(&self) -> SecretKindDto {
        SecretKindDto(serde_json::to_string(self).unwrap_or_else(|_| "<invalid>".into()))
    }
}
```

- [ ] **Step 3: scanner.rs — gitleaks rule subset + tests**

```rust
//! Secret scanner with high-confidence gitleaks rules.
//!
//! The bundled binary must not itself contain a complete credential-looking
//! token prefix — Anthropic's pattern is assembled at runtime from fragments.

use regex::Regex;

/// Spec for one detection rule.
pub struct SecretRuleSpec {
    pub id: &'static str,
    pub label: &'static str,
    pub source: String,
}

/// Public match — no offset, safe for telemetry.
#[derive(Debug, Clone)]
pub struct SecretDetection {
    pub rule_id: String,
    pub label: String,
}

pub struct SecretScanner {
    rules: Vec<(String, String, Regex)>, // (id, label, pattern)
}

impl SecretScanner {
    /// Build the standard rule set (subset of gitleaks high-confidence rules).
    pub fn builtin() -> Self {
        let specs = builtin_rule_specs();
        let rules = specs
            .into_iter()
            .filter_map(|s| Regex::new(&s.source).ok().map(|re| (s.id.to_string(), s.label.to_string(), re)))
            .collect();
        Self { rules }
    }

    pub fn scan(&self, content: &str) -> Vec<SecretDetection> {
        let mut hits = Vec::new();
        for (id, label, re) in &self.rules {
            if re.is_match(content) {
                hits.push(SecretDetection { rule_id: id.clone(), label: label.clone() });
            }
        }
        hits
    }

    /// Replace each match with `[REDACTED:<rule-id>]`.
    pub fn redact(&self, content: &str) -> String {
        let mut out = content.to_string();
        for (id, _, re) in &self.rules {
            let replacement = format!("[REDACTED:{id}]");
            out = re.replace_all(&out, replacement.as_str()).to_string();
        }
        out
    }
}

fn builtin_rule_specs() -> Vec<SecretRuleSpec> {
    // Anthropic key prefix built at runtime — avoids bundled-binary scan match.
    let ant_pfx = format!("{}-{}-{}", "sk", "ant", "api");
    let ant_pat = format!(r"\b({}03-[a-zA-Z0-9_\-]{{93}}AA)", ant_pfx);

    vec![
        SecretRuleSpec { id: "aws-access-token", label: "AWS Access Token",
            source: r"\b((?:A3T[A-Z0-9]|AKIA|ASIA|ABIA|ACCA)[A-Z2-7]{16})\b".into() },
        SecretRuleSpec { id: "gcp-api-key", label: "GCP API Key",
            source: r"\b(AIza[\w-]{35})\b".into() },
        SecretRuleSpec { id: "anthropic-api-key", label: "Anthropic API Key",
            source: ant_pat },
        SecretRuleSpec { id: "openai-api-key", label: "OpenAI API Key",
            source: r"\bsk-(?:proj-|svcacct-|admin-)?[A-Za-z0-9_-]{20,}".into() },
        SecretRuleSpec { id: "github-pat", label: "GitHub PAT",
            source: r"\bghp_[0-9a-zA-Z]{36}\b".into() },
        SecretRuleSpec { id: "github-fine-grained-pat", label: "GitHub Fine-Grained PAT",
            source: r"\bgithub_pat_[A-Za-z0-9_]{82}\b".into() },
        SecretRuleSpec { id: "gitlab-pat", label: "GitLab PAT",
            source: r"\bglpat-[0-9a-zA-Z_\-]{20}\b".into() },
        SecretRuleSpec { id: "slack-bot-token", label: "Slack Bot Token",
            source: r"\bxox[abprs]-[0-9a-zA-Z\-]{10,72}\b".into() },
        SecretRuleSpec { id: "stripe-secret-key", label: "Stripe Secret Key",
            source: r"\bsk_(live|test)_[0-9a-zA-Z]{24,}".into() },
        SecretRuleSpec { id: "digitalocean-pat", label: "DigitalOcean PAT",
            source: r"\bdop_v1_[a-f0-9]{64}\b".into() },
        SecretRuleSpec { id: "huggingface-token", label: "HuggingFace Token",
            source: r"\bhf_[a-zA-Z]{34}\b".into() },
        // ... 20+ more in production; representative subset here ...
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_github_pat() {
        let s = SecretScanner::builtin();
        let hits = s.scan("token=ghp_1234567890ABCDEFGHIJKLMNOPQRSTUVWXYZ");
        assert!(hits.iter().any(|h| h.rule_id == "github-pat"));
    }

    #[test]
    fn redacts_aws_access_token() {
        let s = SecretScanner::builtin();
        let red = s.redact("Hi AKIAIOSFODNN7EXAMPLE bye");
        assert!(!red.contains("AKIA"));
        assert!(red.contains("REDACTED:aws-access-token"));
    }

    #[test]
    fn clean_content_passes() {
        let s = SecretScanner::builtin();
        assert!(s.scan("just normal text here").is_empty());
    }
}
```

- [ ] **Step 4: redaction.rs**

```rust
use crate::scanner::{SecretScanner, SecretDetection};
use lingxi_protocol::RedactableContent;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RedactionBoundary {
    TranscriptWrite,
    LogOutput,
    TeamMemoryUpload,
    ConversationInjection,
    Telemetry,
}

#[derive(Debug, Clone, Copy)]
pub enum BoundaryPolicy {
    AlwaysRedact,
    RejectOnDetection,
    WarnOnly,
}

pub struct RedactionPolicy {
    scanner: Arc<SecretScanner>,
    boundary: HashMap<RedactionBoundary, BoundaryPolicy>,
}

#[derive(Debug, Clone)]
pub enum RedactionOutcome {
    /// No matches; original passes through.
    Clean { content: RedactableContent },
    /// Match(es) found; rewritten content + list of detections.
    Redacted { content: RedactableContent, detections: Vec<SecretDetection> },
    /// Boundary policy rejects egress; content discarded.
    Rejected { detections: Vec<SecretDetection> },
}

impl RedactionPolicy {
    pub fn with_defaults(scanner: Arc<SecretScanner>) -> Self {
        let mut map = HashMap::new();
        map.insert(RedactionBoundary::TranscriptWrite, BoundaryPolicy::AlwaysRedact);
        map.insert(RedactionBoundary::LogOutput, BoundaryPolicy::AlwaysRedact);
        map.insert(RedactionBoundary::TeamMemoryUpload, BoundaryPolicy::RejectOnDetection);
        map.insert(RedactionBoundary::ConversationInjection, BoundaryPolicy::AlwaysRedact);
        map.insert(RedactionBoundary::Telemetry, BoundaryPolicy::AlwaysRedact);
        Self { scanner, boundary: map }
    }

    pub fn process(&self, content: RedactableContent, b: RedactionBoundary) -> RedactionOutcome {
        let raw = content.expose_for_scan();
        let detections = self.scanner.scan(raw);
        if detections.is_empty() {
            return RedactionOutcome::Clean { content };
        }
        match self.boundary.get(&b).copied().unwrap_or(BoundaryPolicy::AlwaysRedact) {
            BoundaryPolicy::AlwaysRedact => {
                let rewritten = self.scanner.redact(raw);
                RedactionOutcome::Redacted {
                    content: RedactableContent::new(rewritten),
                    detections,
                }
            }
            BoundaryPolicy::RejectOnDetection => RedactionOutcome::Rejected { detections },
            BoundaryPolicy::WarnOnly => RedactionOutcome::Redacted {
                content,
                detections,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn telemetry_redacts_aws_token() {
        let scanner = Arc::new(SecretScanner::builtin());
        let policy = RedactionPolicy::with_defaults(scanner);
        let c = RedactableContent::new("token AKIAIOSFODNN7EXAMPLE here".into());
        let out = policy.process(c, RedactionBoundary::Telemetry);
        match out {
            RedactionOutcome::Redacted { content, detections } => {
                assert!(!content.expose_for_scan().contains("AKIA"));
                assert!(!detections.is_empty());
            }
            other => panic!("expected Redacted, got {other:?}"),
        }
    }

    #[test]
    fn team_memory_rejects_on_detection() {
        let scanner = Arc::new(SecretScanner::builtin());
        let policy = RedactionPolicy::with_defaults(scanner);
        let c = RedactableContent::new("ghp_1234567890ABCDEFGHIJKLMNOPQRSTUVWXYZ".into());
        let out = policy.process(c, RedactionBoundary::TeamMemoryUpload);
        assert!(matches!(out, RedactionOutcome::Rejected { .. }));
    }
}
```

- [ ] **Step 5: lib.rs**

```rust
#![forbid(unsafe_code)]
pub mod credential;
pub mod keychain_prefetch;
pub mod kinds;
pub mod redaction;
pub mod scanner;

pub use credential::CredentialManager;
pub use keychain_prefetch::KeychainPrefetch;
pub use kinds::SecretKind;
pub use redaction::{BoundaryPolicy, RedactionBoundary, RedactionOutcome, RedactionPolicy};
pub use scanner::{SecretDetection, SecretScanner};
```

- [ ] **Step 6: Add credential.rs + keychain_prefetch.rs stubs**

```rust
// crates/secret/src/credential.rs
use lingxi_protocol::{Secret, SecureStorageData, SecureStorageMetadata};
use lingxi_platform_api::{Clock, HttpTransport, SecureStorage, SecureStorageError};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, RwLock};
use thiserror::Error;

use crate::kinds::SecretKind;

#[derive(Debug, Clone, Error)]
pub enum CredentialError {
    #[error(transparent)]
    Storage(#[from] SecureStorageError),
    #[error("token expired and refresh failed: {0}")]
    RefreshFailed(String),
    #[error("no credential available")]
    Unavailable,
}

pub struct CredentialManager {
    storage: Arc<dyn SecureStorage>,
    clock: Arc<dyn Clock>,
    http: Arc<dyn HttpTransport>,
    refresh_lock: Mutex<()>,
    // caches are kept under RwLock; M1 only exposes api_key getter.
    api_key_cache: RwLock<Option<(Secret<String>, std::time::SystemTime)>>,
    api_key_ttl: Duration,
}

impl CredentialManager {
    pub fn new(
        storage: Arc<dyn SecureStorage>,
        clock: Arc<dyn Clock>,
        http: Arc<dyn HttpTransport>,
    ) -> Self {
        Self {
            storage,
            clock,
            http,
            refresh_lock: Mutex::new(()),
            api_key_cache: RwLock::new(None),
            api_key_ttl: Duration::from_secs(300),
        }
    }

    /// Returns the Anthropic API key, loading from SecureStorage on cache miss.
    pub async fn get_anthropic_api_key(&self) -> Result<Option<Secret<String>>, CredentialError> {
        // Fast path: cached and fresh.
        if let Some((s, cached_at)) = self.api_key_cache.read().await.as_ref() {
            if self.clock.elapsed_since(*cached_at) < self.api_key_ttl {
                return Ok(Some(Secret::new(s.expose_secret().clone())));
            }
        }
        // Slow path: load from storage.
        let raw = self.storage.retrieve("lingxi", "anthropic-api-key").await?;
        let Some(raw) = raw else { return Ok(None); };
        let s = String::from_utf8(raw.expose_secret_bytes().to_vec())
            .map_err(|_| CredentialError::Unavailable)?;
        let now = self.clock.now();
        let secret = Secret::new(s.clone());
        *self.api_key_cache.write().await = Some((Secret::new(s), now));
        Ok(Some(secret))
    }

    pub async fn store_anthropic_api_key(&self, key: &str) -> Result<(), CredentialError> {
        let metadata = SecureStorageMetadata {
            created_at: self.clock.now(),
            last_accessed: None,
            kind: SecretKind::AnthropicApiKey.as_dto(),
        };
        let data = SecureStorageData::new(key.as_bytes().to_vec(), metadata);
        self.storage.store("lingxi", "anthropic-api-key", data).await?;
        // invalidate cache
        *self.api_key_cache.write().await = None;
        Ok(())
    }
}
```

```rust
// crates/secret/src/keychain_prefetch.rs
//! Pre-warm the macOS keychain access prompt at startup so it doesn't
//! interrupt the first interactive moment.
use lingxi_protocol::SecureStorageData;
use lingxi_platform_api::{RuntimeError, RuntimeSpawner, SecureStorage, SecureStorageError};
use std::sync::Arc;
use tokio::sync::oneshot;

pub struct KeychainPrefetch {
    rx: tokio::sync::Mutex<Option<oneshot::Receiver<Result<Option<SecureStorageData>, SecureStorageError>>>>,
}

impl KeychainPrefetch {
    pub async fn start(
        storage: Arc<dyn SecureStorage>,
        runtime: &dyn RuntimeSpawner,
    ) -> Result<Self, RuntimeError> {
        let (tx, rx) = oneshot::channel();
        runtime.spawn(
            "keychain-prefetch",
            Box::pin(async move {
                let _ = tx.send(storage.retrieve("lingxi", "anthropic-api-key").await);
            }),
        ).await?;
        Ok(Self { rx: tokio::sync::Mutex::new(Some(rx)) })
    }

    pub async fn consume(&self) -> Option<Result<Option<SecureStorageData>, SecureStorageError>> {
        let rx = self.rx.lock().await.take()?;
        rx.await.ok()
    }
}
```

- [ ] **Step 7: Run tests**

```bash
cargo test -p lingxi-secret --lib
```

Expected: 5 tests pass (3 in scanner + 2 in redaction).

- [ ] **Step 8: Commit**

```bash
git add crates/secret
git commit -m "feat(secret): scanner, redaction policy, credential manager, keychain prefetch"
```

---

## Task 4: lingxi-cost crate — PricingCatalog + Usage + Calculator

**Files:**
- Create: `crates/cost/Cargo.toml`
- Create: `crates/cost/src/{lib,pricing,usage,calculator}.rs`

- [ ] **Step 1: Cargo.toml**

```toml
[package]
name = "lingxi-cost"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[dependencies]
lingxi-protocol = { path = "../protocol" }
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true

[lints]
workspace = true
```

- [ ] **Step 2: pricing.rs**

```rust
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::SystemTime;
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ModelRef {
    pub provider: ProviderId,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProviderId {
    Anthropic,
    OpenAI,
    GoogleGemini,
    OpenAICompatible { name: String },
    Custom { name: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TokenClass {
    Input,
    Output,
    CacheWrite,
    CacheRead,
    ReasoningOutput,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NonTokenBillableUnit {
    WebSearchRequest,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct MoneyPerToken {
    pub nano_usd_per_token: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPricing {
    pub model_ref: ModelRef,
    pub token_rates: HashMap<TokenClass, MoneyPerToken>,
    pub non_token_rates_nano_usd: HashMap<NonTokenBillableUnit, u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub effective_from: Option<SystemTime>,
    pub source: PricingSource,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PricingSource {
    BuiltInReference { provider: ProviderId },
    HostOverride { path: PathBuf },
    RemoteManagedSettings,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PricingResolution {
    ExactModel { model_ref: ModelRef },
    ProviderDefault { requested: ModelRef },
    UnpricedModel { requested: ModelRef },
}

#[derive(Debug, Clone, Error)]
pub enum CostError {
    #[error("unpriced model: {0:?}")]
    UnpricedModel(ModelRef),
}

pub struct PricingCatalog {
    entries: HashMap<ModelRef, ModelPricing>,
    provider_defaults: HashMap<ProviderId, ModelPricing>,
}

impl PricingCatalog {
    pub fn empty() -> Self {
        Self { entries: HashMap::new(), provider_defaults: HashMap::new() }
    }

    /// Builtin: 6 Anthropic tiers from claude-code reference + OpenAI placeholder.
    pub fn builtin_reference() -> Self {
        let mut c = Self::empty();
        // $3/$15 tier — Sonnet variants.
        c.insert_anthropic("claude-sonnet-4-6", 3_000, 15_000, 3_750, 300);
        c.insert_anthropic("claude-sonnet-4-5", 3_000, 15_000, 3_750, 300);
        c.insert_anthropic("claude-3-7-sonnet", 3_000, 15_000, 3_750, 300);
        // $5/$25 tier — Opus 4.5.
        c.insert_anthropic("claude-opus-4-5", 5_000, 25_000, 6_250, 500);
        // $5/$25 — Opus 4.6 standard.
        c.insert_anthropic("claude-opus-4-6", 5_000, 25_000, 6_250, 500);
        // $15/$75 — Opus 4 / 4.1.
        c.insert_anthropic("claude-opus-4-1", 15_000, 75_000, 18_750, 1_500);
        // Haiku 4.5 — $1/$5.
        c.insert_anthropic("claude-haiku-4-5", 1_000, 5_000, 1_250, 100);
        // Haiku 3.5 — $0.80/$4.
        c.insert_anthropic("claude-3-5-haiku", 800, 4_000, 1_000, 80);
        c
    }

    fn insert_anthropic(
        &mut self,
        model: &str,
        input_per_mtok_milli_usd: u64,
        output_per_mtok_milli_usd: u64,
        cache_write_per_mtok_milli_usd: u64,
        cache_read_per_mtok_milli_usd: u64,
    ) {
        // milli-USD per Mtok = nano-USD per token (10^-3 / 10^6 = 10^-9).
        let mr = ModelRef { provider: ProviderId::Anthropic, model: model.into() };
        let mut rates = HashMap::new();
        rates.insert(TokenClass::Input, MoneyPerToken { nano_usd_per_token: input_per_mtok_milli_usd });
        rates.insert(TokenClass::Output, MoneyPerToken { nano_usd_per_token: output_per_mtok_milli_usd });
        rates.insert(TokenClass::CacheWrite, MoneyPerToken { nano_usd_per_token: cache_write_per_mtok_milli_usd });
        rates.insert(TokenClass::CacheRead, MoneyPerToken { nano_usd_per_token: cache_read_per_mtok_milli_usd });

        let mut non_token = HashMap::new();
        non_token.insert(NonTokenBillableUnit::WebSearchRequest, 10_000_000); // $0.01/request

        self.entries.insert(mr.clone(), ModelPricing {
            model_ref: mr,
            token_rates: rates,
            non_token_rates_nano_usd: non_token,
            effective_from: None,
            source: PricingSource::BuiltInReference { provider: ProviderId::Anthropic },
        });
    }

    pub fn resolve(&self, mr: &ModelRef) -> Result<(ModelPricing, PricingResolution), CostError> {
        if let Some(p) = self.entries.get(mr) {
            return Ok((p.clone(), PricingResolution::ExactModel { model_ref: mr.clone() }));
        }
        if let Some(p) = self.provider_defaults.get(&mr.provider) {
            return Ok((p.clone(), PricingResolution::ProviderDefault { requested: mr.clone() }));
        }
        Err(CostError::UnpricedModel(mr.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_has_opus_4_6() {
        let c = PricingCatalog::builtin_reference();
        let mr = ModelRef { provider: ProviderId::Anthropic, model: "claude-opus-4-6".into() };
        let (p, res) = c.resolve(&mr).unwrap();
        assert!(matches!(res, PricingResolution::ExactModel { .. }));
        assert_eq!(p.token_rates[&TokenClass::Input].nano_usd_per_token, 5_000);
    }

    #[test]
    fn unpriced_model_errors() {
        let c = PricingCatalog::builtin_reference();
        let mr = ModelRef { provider: ProviderId::Anthropic, model: "claude-nonexistent".into() };
        assert!(matches!(c.resolve(&mr).unwrap_err(), CostError::UnpricedModel(_)));
    }
}
```

- [ ] **Step 3: usage.rs**

```rust
use serde::{Deserialize, Serialize};
use crate::pricing::TokenClass;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input: u64,
    pub output: u64,
    pub cache_write: u64,
    pub cache_read: u64,
    pub reasoning_output: u64,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Usage {
    pub tokens: TokenUsage,
    pub server_tool_use: Option<ServerToolUsage>,
    pub speed: Option<ApiSpeed>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct ServerToolUsage {
    pub web_search_requests: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ApiSpeed { Standard, Fast }

impl Usage {
    pub fn tokens_for(&self, class: TokenClass) -> u64 {
        match class {
            TokenClass::Input => self.tokens.input,
            TokenClass::Output => self.tokens.output,
            TokenClass::CacheWrite => self.tokens.cache_write,
            TokenClass::CacheRead => self.tokens.cache_read,
            TokenClass::ReasoningOutput => self.tokens.reasoning_output,
        }
    }

    pub fn total_tokens(&self) -> u64 {
        self.tokens.input
            + self.tokens.output
            + self.tokens.cache_write
            + self.tokens.cache_read
            + self.tokens.reasoning_output
    }

    pub fn add(&mut self, other: &Usage) {
        self.tokens.input += other.tokens.input;
        self.tokens.output += other.tokens.output;
        self.tokens.cache_write += other.tokens.cache_write;
        self.tokens.cache_read += other.tokens.cache_read;
        self.tokens.reasoning_output += other.tokens.reasoning_output;
        if let Some(s) = other.server_tool_use {
            self.server_tool_use
                .get_or_insert(ServerToolUsage::default())
                .web_search_requests += s.web_search_requests;
        }
    }
}
```

- [ ] **Step 4: calculator.rs**

```rust
use crate::pricing::{ModelPricing, NonTokenBillableUnit, TokenClass};
use crate::usage::Usage;

pub struct CostCalculator;

impl CostCalculator {
    /// Total cost in nano-USD. Saturating arithmetic — no panic on overflow.
    pub fn calculate_nano_usd(usage: &Usage, pricing: &ModelPricing) -> u64 {
        let mut total: u64 = 0;

        for (class, rate) in &pricing.token_rates {
            let tokens = usage.tokens_for(*class);
            total = total.saturating_add(tokens.saturating_mul(rate.nano_usd_per_token));
        }

        if let Some(s) = usage.server_tool_use {
            if let Some(per_req) = pricing
                .non_token_rates_nano_usd
                .get(&NonTokenBillableUnit::WebSearchRequest)
                .copied()
            {
                total = total.saturating_add((s.web_search_requests as u64).saturating_mul(per_req));
            }
        }

        total
    }

    pub fn cache_savings_nano_usd(usage: &Usage, pricing: &ModelPricing) -> u64 {
        let Some(input_rate) = pricing.token_rates.get(&TokenClass::Input) else { return 0; };
        let Some(cache_rate) = pricing.token_rates.get(&TokenClass::CacheRead) else { return 0; };
        let would_have_paid = usage.tokens.cache_read.saturating_mul(input_rate.nano_usd_per_token);
        let actually_paid = usage.tokens.cache_read.saturating_mul(cache_rate.nano_usd_per_token);
        would_have_paid.saturating_sub(actually_paid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pricing::{ModelRef, PricingCatalog, ProviderId};
    use crate::usage::TokenUsage;

    #[test]
    fn opus_4_6_input_only_correct() {
        let c = PricingCatalog::builtin_reference();
        let mr = ModelRef { provider: ProviderId::Anthropic, model: "claude-opus-4-6".into() };
        let (p, _) = c.resolve(&mr).unwrap();
        let usage = Usage {
            tokens: TokenUsage { input: 1_000_000, ..Default::default() },
            ..Default::default()
        };
        // 1M tokens × 5000 nano-USD/tok = 5e9 nano-USD = $5
        assert_eq!(CostCalculator::calculate_nano_usd(&usage, &p), 5_000_000_000);
    }

    #[test]
    fn cache_savings_positive_when_read_rate_lower() {
        let c = PricingCatalog::builtin_reference();
        let mr = ModelRef { provider: ProviderId::Anthropic, model: "claude-opus-4-6".into() };
        let (p, _) = c.resolve(&mr).unwrap();
        let usage = Usage {
            tokens: TokenUsage { cache_read: 1_000_000, ..Default::default() },
            ..Default::default()
        };
        let savings = CostCalculator::cache_savings_nano_usd(&usage, &p);
        assert!(savings > 0);
    }

    #[test]
    fn saturating_no_overflow() {
        let mut p = ModelPricing {
            model_ref: ModelRef { provider: ProviderId::Anthropic, model: "x".into() },
            token_rates: Default::default(),
            non_token_rates_nano_usd: Default::default(),
            effective_from: None,
            source: crate::pricing::PricingSource::BuiltInReference { provider: ProviderId::Anthropic },
        };
        p.token_rates.insert(TokenClass::Input, crate::pricing::MoneyPerToken {
            nano_usd_per_token: u64::MAX,
        });
        let usage = Usage {
            tokens: TokenUsage { input: u64::MAX, ..Default::default() },
            ..Default::default()
        };
        // Should not panic.
        let _ = CostCalculator::calculate_nano_usd(&usage, &p);
    }
}
```

- [ ] **Step 5: lib.rs + run tests**

```rust
#![forbid(unsafe_code)]
pub mod budget;
pub mod calculator;
pub mod pricing;
pub mod tracker;
pub mod usage;

pub use budget::{BudgetCheckResult, BudgetConfig, BudgetEnforcer, BudgetExceedPolicy};
pub use calculator::CostCalculator;
pub use pricing::{
    CostError, ModelPricing, ModelRef, MoneyPerToken, NonTokenBillableUnit, PricingCatalog,
    PricingResolution, PricingSource, ProviderId, TokenClass,
};
pub use tracker::{CostState, CostTracker, ModelUsage};
pub use usage::{ApiSpeed, ServerToolUsage, TokenUsage, Usage};
```

```bash
cargo test -p lingxi-cost --lib
```

Expected: 5 tests pass (2 in pricing + 3 in calculator).

- [ ] **Step 6: Commit**

```bash
git add crates/cost
git commit -m "feat(cost): PricingCatalog, Usage, CostCalculator (saturating, nano_usd)"
```

---

## Task 5: CostTracker + BudgetEnforcer

**Files:**
- Create: `crates/cost/src/tracker.rs`
- Create: `crates/cost/src/budget.rs`

- [ ] **Step 1: tracker.rs**

```rust
use crate::{calculator::CostCalculator, pricing::{PricingCatalog, PricingResolution}, usage::Usage, ModelRef};
use lingxi_protocol::SessionId;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, RwLock};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CostState {
    pub session_id: SessionId,
    pub total_nano_usd: u64,
    pub per_model_usage: HashMap<ModelRef, ModelUsage>,
    pub total_api_duration_ms: u64,
    pub total_api_duration_without_retries_ms: u64,
    pub total_tool_duration_ms: u64,
    pub unpriced_models: HashSet<ModelRef>,
    pub total_web_search_requests: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelUsage {
    pub model_ref: ModelRef,
    pub usage: Usage,
    pub cost_nano_usd: u64,
}

pub struct CostTracker {
    state: Arc<RwLock<CostState>>,
    catalog: Arc<PricingCatalog>,
    persist_tx: mpsc::Sender<CostState>,
}

impl CostTracker {
    /// `persist_tx` is a single-writer channel that drains to disk in a
    /// background task. This prevents concurrent writers from corrupting
    /// the persisted file.
    pub fn new(
        session_id: SessionId,
        catalog: Arc<PricingCatalog>,
        persist_tx: mpsc::Sender<CostState>,
    ) -> Self {
        let state = CostState { session_id, ..Default::default() };
        Self { state: Arc::new(RwLock::new(state)), catalog, persist_tx }
    }

    pub async fn record_api_response(
        &self,
        model_ref: ModelRef,
        usage: Usage,
        duration: Duration,
        retries: u32,
    ) {
        let (pricing, resolution) = match self.catalog.resolve(&model_ref) {
            Ok(p) => (Some(p.0), Some(p.1)),
            Err(_) => (None, None),
        };

        let cost = pricing
            .as_ref()
            .map(|p| CostCalculator::calculate_nano_usd(&usage, p))
            .unwrap_or(0);

        let mut state = self.state.write().await;
        state.total_nano_usd = state.total_nano_usd.saturating_add(cost);
        let dur_ms = duration.as_millis() as u64;
        state.total_api_duration_ms = state.total_api_duration_ms.saturating_add(dur_ms);
        if retries == 0 {
            state.total_api_duration_without_retries_ms =
                state.total_api_duration_without_retries_ms.saturating_add(dur_ms);
        }
        let entry = state
            .per_model_usage
            .entry(model_ref.clone())
            .or_insert_with(|| ModelUsage {
                model_ref: model_ref.clone(),
                usage: Usage::default(),
                cost_nano_usd: 0,
            });
        entry.usage.add(&usage);
        entry.cost_nano_usd = entry.cost_nano_usd.saturating_add(cost);
        if matches!(resolution, Some(PricingResolution::UnpricedModel { .. })) {
            state.unpriced_models.insert(model_ref);
        }
        if let Some(s) = usage.server_tool_use {
            state.total_web_search_requests = state
                .total_web_search_requests
                .saturating_add(s.web_search_requests);
        }
        let snap = state.clone();
        drop(state);
        let _ = self.persist_tx.send(snap).await;
    }

    pub async fn total_nano_usd(&self) -> u64 { self.state.read().await.total_nano_usd }
    pub async fn snapshot(&self) -> CostState { self.state.read().await.clone() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pricing::{ProviderId};
    use crate::usage::TokenUsage;

    #[tokio::test]
    async fn record_accumulates_cost() {
        let (tx, mut rx) = mpsc::channel(8);
        let tracker = CostTracker::new(
            SessionId::nil(),
            Arc::new(PricingCatalog::builtin_reference()),
            tx,
        );
        let mr = ModelRef { provider: ProviderId::Anthropic, model: "claude-opus-4-6".into() };
        tracker
            .record_api_response(
                mr.clone(),
                Usage { tokens: TokenUsage { input: 1000, output: 500, ..Default::default() }, ..Default::default() },
                Duration::from_millis(200),
                0,
            )
            .await;
        let snap = rx.recv().await.unwrap();
        // 1000 * 5000 + 500 * 25000 = 5_000_000 + 12_500_000 = 17_500_000 nano-USD = $0.0175
        assert_eq!(snap.total_nano_usd, 17_500_000);
    }
}
```

- [ ] **Step 2: budget.rs**

```rust
use crate::tracker::CostTracker;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetConfig {
    pub max_session_nano_usd: Option<u64>,
    pub max_turn_nano_usd: Option<u64>,
    pub max_turn_tokens: Option<u64>,
    pub warning_thresholds: Vec<f64>, // [0.5, 0.8, 0.95]
    pub on_exceed: BudgetExceedPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BudgetExceedPolicy { Halt, AskUser, WarnOnly }

pub struct BudgetEnforcer {
    config: BudgetConfig,
    cost_tracker: Arc<CostTracker>,
    warnings_fired: RwLock<HashSet<u32>>,
    realized_exceeded: AtomicBool,
}

#[derive(Debug, Clone)]
pub enum BudgetCheckResult {
    Ok,
    ThresholdWarning { pct: u32, current: u64, limit: u64 },
    Warn { current: u64, limit: u64 },
    AskUser { current: u64, limit: u64 },
    Halt { current: u64, limit: u64 },
}

impl BudgetEnforcer {
    pub fn new(config: BudgetConfig, cost_tracker: Arc<CostTracker>) -> Self {
        Self {
            config,
            cost_tracker,
            warnings_fired: RwLock::new(HashSet::new()),
            realized_exceeded: AtomicBool::new(false),
        }
    }

    /// Pre-API call gate. After the call returns, call `check_post_api_call`
    /// to latch the realized-exceeded flag if the actual cost overran.
    pub async fn check_pre_api_call(&self, estimated_cost_nano_usd: u64) -> BudgetCheckResult {
        if self.realized_exceeded.load(Ordering::Acquire) {
            let current = self.cost_tracker.total_nano_usd().await;
            let limit = self.config.max_session_nano_usd.unwrap_or(0);
            return BudgetCheckResult::Halt { current, limit };
        }
        let current = self.cost_tracker.total_nano_usd().await;
        let after = current.saturating_add(estimated_cost_nano_usd);
        if let Some(max) = self.config.max_session_nano_usd {
            if after > max {
                return match self.config.on_exceed {
                    BudgetExceedPolicy::Halt => BudgetCheckResult::Halt { current, limit: max },
                    BudgetExceedPolicy::AskUser => BudgetCheckResult::AskUser { current, limit: max },
                    BudgetExceedPolicy::WarnOnly => BudgetCheckResult::Warn { current, limit: max },
                };
            }
            // Threshold warning — cast to f64 only for the ratio comparison.
            #[allow(clippy::cast_precision_loss)]
            let ratio = after as f64 / max as f64;
            for &threshold in &self.config.warning_thresholds {
                if ratio >= threshold {
                    let pct = (threshold * 100.0) as u32;
                    let mut fired = self.warnings_fired.write().await;
                    if fired.insert(pct) {
                        return BudgetCheckResult::ThresholdWarning { pct, current, limit: max };
                    }
                }
            }
        }
        BudgetCheckResult::Ok
    }

    /// Latch the realized-exceeded flag if `realized_cost` puts the session
    /// over the budget. Subsequent `check_pre_api_call` returns Halt.
    pub async fn check_post_api_call(&self, _realized_cost: u64) {
        let total = self.cost_tracker.total_nano_usd().await;
        if let Some(max) = self.config.max_session_nano_usd {
            if total > max {
                self.realized_exceeded.store(true, Ordering::Release);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pricing::PricingCatalog;
    use lingxi_protocol::SessionId;
    use tokio::sync::mpsc;

    fn make_tracker() -> Arc<CostTracker> {
        let (tx, _rx) = mpsc::channel(8);
        Arc::new(CostTracker::new(SessionId::nil(), Arc::new(PricingCatalog::builtin_reference()), tx))
    }

    #[tokio::test]
    async fn under_budget_returns_ok() {
        let cfg = BudgetConfig {
            max_session_nano_usd: Some(1_000_000_000),
            max_turn_nano_usd: None,
            max_turn_tokens: None,
            warning_thresholds: vec![0.5, 0.8, 0.95],
            on_exceed: BudgetExceedPolicy::Halt,
        };
        let e = BudgetEnforcer::new(cfg, make_tracker());
        assert!(matches!(e.check_pre_api_call(10_000_000).await, BudgetCheckResult::Ok));
    }

    #[tokio::test]
    async fn over_budget_halts_with_halt_policy() {
        let cfg = BudgetConfig {
            max_session_nano_usd: Some(1_000),
            max_turn_nano_usd: None,
            max_turn_tokens: None,
            warning_thresholds: vec![],
            on_exceed: BudgetExceedPolicy::Halt,
        };
        let e = BudgetEnforcer::new(cfg, make_tracker());
        assert!(matches!(e.check_pre_api_call(10_000).await, BudgetCheckResult::Halt { .. }));
    }
}
```

- [ ] **Step 3: Run tests**

```bash
cargo test -p lingxi-cost
```

Expected: tests pass (incl. budget tests above).

- [ ] **Step 4: Commit**

```bash
git add crates/cost
git commit -m "feat(cost): CostTracker (single-writer persist) + BudgetEnforcer (latched realized-exceeded)"
```

---

## Task 6: lingxi-permission crate — Mode + Rule + Result

**Files:**
- Create: `crates/permission/Cargo.toml`
- Create: `crates/permission/src/{lib,mode,rule,result}.rs`

- [ ] **Step 1: Cargo.toml**

```toml
[package]
name = "lingxi-permission"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[dependencies]
lingxi-protocol = { path = "../protocol" }
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true
regex = "1"

[lints]
workspace = true
```

- [ ] **Step 2: mode.rs**

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PermissionMode {
    // External / user-addressable (settings.json + CLI accept these).
    Default,
    Plan,
    AcceptEdits,
    BypassPermissions,
    DontAsk,
    // Internal-only (rejected by settings/CLI validation).
    Bubble,
    Auto,
}

impl PermissionMode {
    pub fn is_external(self) -> bool {
        !matches!(self, Self::Bubble | Self::Auto)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bubble_is_internal() {
        assert!(!PermissionMode::Bubble.is_external());
    }

    #[test]
    fn default_is_external() {
        assert!(PermissionMode::Default.is_external());
    }
}
```

- [ ] **Step 3: rule.rs**

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PermissionRule {
    pub value: PermissionRuleValue,
    pub behavior: PermissionBehavior,
    pub source: PermissionRuleSource,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PermissionRuleValue {
    pub tool_name: String,
    pub rule_content: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PermissionBehavior { Allow, Deny, Ask }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PermissionRuleSource {
    UserSettings,
    ProjectSettings,
    LocalSettings,
    FlagSettings,
    PolicySettings,
    CliArg,
    Command,
    Session,
}

impl PermissionRuleSource {
    /// Priority order matching claude-code:
    /// userSettings → projectSettings → localSettings → flagSettings →
    /// policySettings → cliArg → command → session
    /// Higher index = higher priority.
    pub fn priority(self) -> u8 {
        match self {
            Self::UserSettings => 0,
            Self::ProjectSettings => 1,
            Self::LocalSettings => 2,
            Self::FlagSettings => 3,
            Self::PolicySettings => 4,
            Self::CliArg => 5,
            Self::Command => 6,
            Self::Session => 7,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_outranks_user_settings() {
        assert!(PermissionRuleSource::Session.priority() > PermissionRuleSource::UserSettings.priority());
    }
}
```

- [ ] **Step 4: result.rs**

```rust
use crate::rule::PermissionRule;
use crate::mode::PermissionMode;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::time::SystemTime;
use lingxi_protocol::RequestId;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "behavior", rename_all = "snake_case")]
pub enum PermissionResult {
    Allow {
        reason: PermissionDecisionReason,
        updated_input: Option<Value>,
        update_destination: Option<PermissionUpdateDestination>,
        metadata: PermissionMetadata,
    },
    Deny {
        reason: PermissionDecisionReason,
        explanation: Option<String>,
        metadata: PermissionMetadata,
    },
    Ask {
        reason: PermissionDecisionReason,
        prompt: PermissionPrompt,
        pending_classifier_check: Option<PendingClassifierCheck>,
        metadata: PermissionMetadata,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PermissionDecisionReason {
    MatchedRule { rule: PermissionRule },
    PermissionMode { mode: PermissionMode },
    SubcommandResults { reasons: HashMap<String, Box<PermissionResult>> },
    PermissionPromptTool { tool_name: String },
    ClassifierApproved { classifier: ClassifierKind, score: f64 },
    ClassifierRejected { classifier: ClassifierKind, score: f64 },
    HookOverride { hook_id: String, source: Option<String>, reason: Option<String> },
    AsyncAgent { reason: String },
    SandboxOverride { reason: SandboxOverrideReason },
    WorkingDirectory { reason: String },
    SafetyCheck { reason: String, classifier_approvable: bool },
    Other { reason: String },
    DenialLimitExceeded,
    AutoModeFallback,
    BypassPermissions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClassifierKind { Yolo, Bash, Transcript }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SandboxOverrideReason {
    ExcludedCommand,
    DangerouslyDisableSandbox,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PermissionMetadata {
    pub matched_rules: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionPrompt {
    pub title: String,
    pub message: String,
    pub options: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingClassifierCheck {
    pub classifier: ClassifierKind,
    pub request_id: RequestId,
    pub started_at: SystemTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PermissionUpdateDestination {
    UserSettings,
    ProjectSettings,
    LocalSettings,
    Session,
    CliArg,
}
```

- [ ] **Step 5: lib.rs**

```rust
#![forbid(unsafe_code)]
pub mod classifier;
pub mod dangerous_patterns;
pub mod denial_tracking;
pub mod mode;
pub mod policy;
pub mod result;
pub mod rule;
pub mod shadow;
pub mod update;

pub use mode::PermissionMode;
pub use policy::PermissionPolicy;
pub use result::{ClassifierKind, PermissionDecisionReason, PermissionResult, PermissionUpdateDestination, SandboxOverrideReason};
pub use rule::{PermissionBehavior, PermissionRule, PermissionRuleSource, PermissionRuleValue};
pub use update::PermissionUpdate;
```

- [ ] **Step 6: Stub the remaining modules**

```rust
// classifier.rs — full impl in Plan 03 Task X
pub use crate::result::ClassifierKind;

// dangerous_patterns.rs — static regex strings
pub const PATTERNS: &[&str] = &[
    r"rm\s+-rf\s+/",
    r"sudo\s+",
    r"curl\s+.*\|\s*(sh|bash)",
    r"chmod\s+777",
    r":\(\)\{.*:.*\|.*&.*\};:",
];

// denial_tracking.rs
use std::collections::HashMap;
use std::time::{Duration, SystemTime};
use crate::result::PermissionDecisionReason;

pub mod limits {
    pub const PER_TOOL_FALLBACK: u32 = 5;
    pub const GLOBAL_FALLBACK: u32 = 10;
    pub const RECORD_TTL: std::time::Duration = std::time::Duration::from_secs(300);
}

#[derive(Debug, Clone, Default)]
pub struct DenialTrackingState {
    pub per_tool_denials: HashMap<String, DenialRecord>,
    pub total_consecutive: u32,
    pub last_success_at: Option<SystemTime>,
}

#[derive(Debug, Clone)]
pub struct DenialRecord {
    pub consecutive_count: u32,
    pub last_denial_at: SystemTime,
    pub last_reason: PermissionDecisionReason,
}

// shadow.rs
use crate::rule::PermissionRule;

pub struct ShadowedRuleDetector;

impl ShadowedRuleDetector {
    pub fn find_shadowing(&self, _new_rule: &PermissionRule, _existing: &[&PermissionRule]) -> Option<PermissionRule> {
        None  // Full impl in Plan 03; M1 ships a no-op so policy compiles.
    }
}

// update.rs
use crate::rule::PermissionRule;
use crate::result::PermissionUpdateDestination;

#[derive(Debug, Clone)]
pub struct PermissionUpdate {
    pub rule: PermissionRule,
    pub destination: PermissionUpdateDestination,
}
```

- [ ] **Step 7: policy.rs — minimal authorize for M1.3 gate**

```rust
//! PermissionPolicy::authorize — rule-driven decision + mode fallback.
//! Classifiers and shadow detection are wired in Plan 03 (Tools System).

use crate::denial_tracking::DenialTrackingState;
use crate::mode::PermissionMode;
use crate::result::{PermissionDecisionReason, PermissionMetadata, PermissionResult, PermissionPrompt};
use crate::rule::{PermissionBehavior, PermissionRule, PermissionRuleSource};
use std::collections::HashMap;
use std::sync::Mutex;

pub struct PermissionPolicy {
    pub mode: PermissionMode,
    pub allow_rules: HashMap<PermissionRuleSource, Vec<PermissionRule>>,
    pub deny_rules: HashMap<PermissionRuleSource, Vec<PermissionRule>>,
    pub ask_rules: HashMap<PermissionRuleSource, Vec<PermissionRule>>,
    pub denial_tracking: Mutex<DenialTrackingState>,
    pub bypass_killswitch_active: bool,
}

impl PermissionPolicy {
    pub fn new(mode: PermissionMode) -> Self {
        Self {
            mode,
            allow_rules: HashMap::new(),
            deny_rules: HashMap::new(),
            ask_rules: HashMap::new(),
            denial_tracking: Mutex::new(DenialTrackingState::default()),
            bypass_killswitch_active: false,
        }
    }

    /// Resolve a tool call to a PermissionResult.
    pub fn authorize(&self, tool_name: &str, _input: &serde_json::Value) -> PermissionResult {
        // Source order matches D1 priority.
        let sources = [
            PermissionRuleSource::PolicySettings,
            PermissionRuleSource::Session,
            PermissionRuleSource::CliArg,
            PermissionRuleSource::Command,
            PermissionRuleSource::FlagSettings,
            PermissionRuleSource::LocalSettings,
            PermissionRuleSource::ProjectSettings,
            PermissionRuleSource::UserSettings,
        ];

        // Deny first.
        for src in &sources {
            if let Some(rules) = self.deny_rules.get(src) {
                if let Some(rule) = rules.iter().find(|r| r.value.tool_name == tool_name) {
                    return deny_with_rule(rule);
                }
            }
        }
        // Then allow.
        for src in &sources {
            if let Some(rules) = self.allow_rules.get(src) {
                if let Some(rule) = rules.iter().find(|r| r.value.tool_name == tool_name) {
                    return allow_with_rule(rule);
                }
            }
        }
        // Mode fallback.
        match self.mode {
            PermissionMode::BypassPermissions if !self.bypass_killswitch_active => {
                allow_with_mode(PermissionMode::BypassPermissions)
            }
            PermissionMode::DontAsk => deny_with_mode(PermissionMode::DontAsk),
            _ => ask_with_mode(self.mode, tool_name),
        }
    }
}

fn allow_with_rule(rule: &PermissionRule) -> PermissionResult {
    PermissionResult::Allow {
        reason: PermissionDecisionReason::MatchedRule { rule: rule.clone() },
        updated_input: None,
        update_destination: None,
        metadata: PermissionMetadata::default(),
    }
}
fn deny_with_rule(rule: &PermissionRule) -> PermissionResult {
    PermissionResult::Deny {
        reason: PermissionDecisionReason::MatchedRule { rule: rule.clone() },
        explanation: None,
        metadata: PermissionMetadata::default(),
    }
}
fn allow_with_mode(mode: PermissionMode) -> PermissionResult {
    PermissionResult::Allow {
        reason: PermissionDecisionReason::PermissionMode { mode },
        updated_input: None, update_destination: None,
        metadata: PermissionMetadata::default(),
    }
}
fn deny_with_mode(mode: PermissionMode) -> PermissionResult {
    PermissionResult::Deny {
        reason: PermissionDecisionReason::PermissionMode { mode },
        explanation: None, metadata: PermissionMetadata::default(),
    }
}
fn ask_with_mode(mode: PermissionMode, tool_name: &str) -> PermissionResult {
    PermissionResult::Ask {
        reason: PermissionDecisionReason::PermissionMode { mode },
        prompt: PermissionPrompt {
            title: format!("Allow {tool_name}?"),
            message: "The agent wants to use this tool.".into(),
            options: vec!["Allow once".into(), "Always allow".into(), "Deny".into()],
        },
        pending_classifier_check: None,
        metadata: PermissionMetadata::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_mode_asks_for_unknown_tool() {
        let p = PermissionPolicy::new(PermissionMode::Default);
        let r = p.authorize("Bash", &serde_json::json!({}));
        assert!(matches!(r, PermissionResult::Ask { .. }));
    }

    #[test]
    fn deny_rule_wins_over_allow() {
        let mut p = PermissionPolicy::new(PermissionMode::Default);
        p.allow_rules.entry(PermissionRuleSource::UserSettings).or_default().push(PermissionRule {
            value: PermissionRuleValue { tool_name: "Bash".into(), rule_content: None },
            behavior: PermissionBehavior::Allow,
            source: PermissionRuleSource::UserSettings,
        });
        p.deny_rules.entry(PermissionRuleSource::ProjectSettings).or_default().push(PermissionRule {
            value: PermissionRuleValue { tool_name: "Bash".into(), rule_content: None },
            behavior: PermissionBehavior::Deny,
            source: PermissionRuleSource::ProjectSettings,
        });
        let r = p.authorize("Bash", &serde_json::json!({}));
        assert!(matches!(r, PermissionResult::Deny { .. }));
    }

    #[test]
    fn dontask_denies_unmatched() {
        let p = PermissionPolicy::new(PermissionMode::DontAsk);
        assert!(matches!(p.authorize("Bash", &serde_json::json!({})), PermissionResult::Deny { .. }));
    }

    #[test]
    fn bypass_allows_unmatched() {
        let p = PermissionPolicy::new(PermissionMode::BypassPermissions);
        assert!(matches!(p.authorize("Bash", &serde_json::json!({})), PermissionResult::Allow { .. }));
    }
}
```

- [ ] **Step 8: Add `rule::PermissionRuleValue` re-export to lib.rs**

(Already done in Step 5.)

- [ ] **Step 9: Run tests**

```bash
cargo test -p lingxi-permission
```

Expected: 6 tests pass (2 in mode + 1 in rule + 3-4 in policy).

- [ ] **Step 10: Commit**

```bash
git add crates/permission
git commit -m "feat(permission): mode + rule + result + minimal policy (M1.3 gate)"
```

---

## Task 7: Wire Permission/Cost into reducer + new Event/Effect variants

**Files:**
- Modify: `crates/protocol/src/effects.rs`
- Modify: `crates/core/src/events.rs`

- [ ] **Step 1: Extend Effect**

Add to `Effect`:

```rust
// Cost
PersistCostState { state_json: serde_json::Value },
DisplayCostUpdate { snapshot_json: serde_json::Value },
EnforceBudget { estimated_cost_nano_usd: u64 },
FireBudgetWarning { pct: u32, current: u64, limit: u64 },
HaltOnBudget,

// Permission
EvaluatePermission { tool_use_id: lingxi_protocol::ToolUseId, tool_name: String, input: serde_json::Value },
PersistPermissionUpdate { update_json: serde_json::Value },
DetectShadowedRules { rule_json: serde_json::Value },

// Secret
StoreCredential { kind: String, data: lingxi_protocol::SecureStorageData },
RetrieveCredential { kind: String },
DeleteCredential { kind: String },
ScanForSecrets { boundary: String, content: lingxi_protocol::RedactableContent },
```

- [ ] **Step 2: Extend Event**

Add to `Event`:

```rust
CostRecorded { model: String, usage: serde_json::Value, cost_nano_usd: u64 },
BudgetThresholdReached { pct: u32, current: u64, limit: u64 },
BudgetExceeded { current: u64, limit: u64 },
PermissionGranted { call_id: lingxi_protocol::ToolUseId },
PermissionDenied { call_id: lingxi_protocol::ToolUseId },
SecretDetected { boundary: String, rule_id: String, redacted: bool },
```

- [ ] **Step 3: Run unit tests**

```bash
cargo test --workspace
```

Expected: all green.

- [ ] **Step 4: Commit**

```bash
git add crates/protocol crates/core
git commit -m "feat(core+protocol): Effect/Event variants for Permission/Secret/Cost"
```

---

## Task 8: Integration test — API call gated by budget

**Files:**
- Create: `crates/test-harness/tests/budget_gate.rs`

- [ ] **Step 1: Write scenario**

```rust
use lingxi_cost::{BudgetConfig, BudgetEnforcer, BudgetExceedPolicy, CostTracker, PricingCatalog};
use lingxi_protocol::SessionId;
use std::sync::Arc;
use tokio::sync::mpsc;

#[tokio::test]
async fn budget_halts_after_three_expensive_calls() {
    let (tx, _rx) = mpsc::channel(8);
    let tracker = Arc::new(CostTracker::new(
        SessionId::nil(),
        Arc::new(PricingCatalog::builtin_reference()),
        tx,
    ));
    let cfg = BudgetConfig {
        max_session_nano_usd: Some(30_000_000), // $0.03
        max_turn_nano_usd: None,
        max_turn_tokens: None,
        warning_thresholds: vec![0.5, 0.8],
        on_exceed: BudgetExceedPolicy::Halt,
    };
    let enforcer = BudgetEnforcer::new(cfg, tracker.clone());

    let mr = lingxi_cost::ModelRef {
        provider: lingxi_cost::ProviderId::Anthropic,
        model: "claude-opus-4-6".into(),
    };
    let u = lingxi_cost::Usage {
        tokens: lingxi_cost::TokenUsage { input: 2000, output: 0, ..Default::default() },
        ..Default::default()
    };

    // Call 1: under budget, expect Ok.
    assert!(matches!(
        enforcer.check_pre_api_call(10_000_000).await,
        lingxi_cost::BudgetCheckResult::Ok | lingxi_cost::BudgetCheckResult::ThresholdWarning { .. }
    ));
    tracker.record_api_response(mr.clone(), u, std::time::Duration::from_millis(100), 0).await;

    // Call 2: still under.
    let _ = enforcer.check_pre_api_call(10_000_000).await;
    tracker.record_api_response(mr.clone(), u, std::time::Duration::from_millis(100), 0).await;

    enforcer.check_post_api_call(10_000_000).await;

    // Call 3: now over, expect Halt.
    let result = enforcer.check_pre_api_call(15_000_000).await;
    assert!(matches!(result, lingxi_cost::BudgetCheckResult::Halt { .. }));
}
```

- [ ] **Step 2: Run**

```bash
cargo test -p lingxi-test-harness --test budget_gate
```

Expected: pass.

- [ ] **Step 3: Commit**

```bash
git add crates/test-harness/tests/budget_gate.rs
git commit -m "test: budget halts after exceeding session limit"
```

---

## Task 9: Plan exit — gate check

- [ ] **Step 1: Workspace check**

```bash
cargo check --workspace --no-default-features
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: all green.

- [ ] **Step 2: Tag**

```bash
git tag -a m1.5-security-cost -m "Plan 02 complete: permission + secret + cost crates"
```

---

## Self-Review

- D13 lingxi-permission → Tasks 6, 7
- D15 lingxi-secret → Tasks 1, 2, 3
- D16 lingxi-cost → Tasks 4, 5
- Spec §14.1 (7 modes) → Task 6 mode.rs ✓
- Spec §14.2 (8 rule sources) → Task 6 rule.rs ✓
- Spec §16.2 Secret<T> → Task 1 ✓
- Spec §17.1 PricingCatalog → Task 4 ✓
- Spec §17.5 BudgetEnforcer realized_exceeded latch → Task 5 ✓

Field consistency: `nano_usd_per_token` used identically across pricing/calculator/tracker/budget. ✓

## Execution Handoff

Next: **Plan 03 — Tools & Hooks** (`2026-05-22-lingxi-core-m1-03-tools-hooks.md`).
