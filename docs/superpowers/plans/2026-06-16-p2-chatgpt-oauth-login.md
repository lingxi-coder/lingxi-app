# P2 — ChatGPT account OAuth login Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a new `openai-oauth` crate + `llm-client` credential seam + engine wiring so a user can `/connect chatgpt`, log in to their OpenAI/ChatGPT account via OAuth, and use `openai-chatgpt` models through the Responses API against `https://chatgpt.com/backend-api/codex` — byte-aligned with codex.

**Architecture:** New crate `openai-oauth` mirrors the existing `anthropic-oauth` crate module-for-module, swapping in OpenAI's OAuth constants and adding device-code + RFC-8693 API-key minting. The one genuinely-new wire detail (the `ChatGPT-Account-ID` header) is a `Credential::ChatGptOAuth` + `AuthStrategy::ChatGptOAuth` + `ChatGptAuthenticator` triad in `llm-client`. A new `openai-chatgpt` provider preset routes to the Codex backend; the engine wires the OAuth credential provider through a generalized `MultiCredentialProvider` (per-credential-id OAuth delegates).

**Tech Stack:** Rust workspace (`lingxi-code/`). `tokio`, `reqwest`-style injected HTTP transport, `base64`/`sha2`/`rand` (PKCE), `secret::CredentialManager` (keychain). Tests use the `testsupport` mock-HTTP + virtual-clock pattern. Run cargo with `CARGO_PROFILE_DEV_DEBUG=0` (disk runs near-full).

**Reference spec:** `docs/superpowers/specs/2026-06-16-p2-chatgpt-oauth-login-design.md`.

**Working dir for all commands:** `/Users/luolingfeng/Projects/LingXi-Next/lingxi-code`. Branch: `p2-chatgpt-oauth-login`.

## PORTING STRATEGY (read first)

The `anthropic-oauth/` crate (in-repo, 3663 lines, fully tested) is the structural template. For each mirrored module, the task is: **copy `anthropic-oauth/src/<mod>.rs` → `openai-oauth/src/<mod>.rs`, then apply the listed deltas** (rename types `ClaudeAi*`→`OpenAi*`/`Anthropic`→`OpenAi`, swap the constants from `config.rs`, and the module-specific deltas called out). This is a precise executable instruction, not a placeholder — the source file is right there. Genuinely-new code is given literally below. After each port, the module must compile and its ported tests (adapted to the new constants) must pass.

## OpenAI OAuth constants (authoritative — used throughout)

```
issuer            = "https://auth.openai.com"
client_id         = "app_EMoamEEZ73f0CkXaXp7hrann"
authorize_url     = "{issuer}/oauth/authorize"
token_url         = "{issuer}/oauth/token"          # exchange + refresh + RFC-8693 mint
device_usercode   = "{issuer}/api/accounts/deviceauth/usercode"
device_token      = "{issuer}/api/accounts/deviceauth/token"
device_verify_url = "{issuer}/codex/device"
scopes            = "openid profile email offline_access api.connectors.read api.connectors.invoke"
authorize extras  = response_type=code, code_challenge_method=S256,
                    id_token_add_organizations=true, codex_cli_simplified_flow=true,
                    originator=<originator>, state=<state>
loopback_redirect = "http://localhost:1455/auth/callback"   # FIXED port 1455, fallback 1457
codex_backend     = "https://chatgpt.com/backend-api/codex"
id_token claims   = chatgpt_account_id (account_id), chatgpt_account_is_fedramp (fedramp)
```

---

## File Structure

- Create: `openai-oauth/` crate — `Cargo.toml` + `src/{lib,config,pkce,callback,token_data,client,device_code,refresh,credential_provider,handle,testsupport}.rs`
- Create: `llm-client/data/models-dev/openai-chatgpt.json` (hand-authored, 3 models)
- Modify: `llm-client/src/credentials.rs` (`Credential::ChatGptOAuth`), `config.rs` (`AuthStrategy::ChatGptOAuth`), `auth.rs` (`ChatGptAuthenticator`), `client.rs` (`authenticate_at` dispatch), `lib.rs` (re-export), `catalog/presets.rs` (preset + `credential_env: Option` + guards)
- Modify: `provider-config/src/credentials.rs` (`MultiCredentialProvider` per-credential-id OAuth delegates), `assemble.rs`/`lib.rs` (count guards)
- Modify: `apps/engine-desktop/src/lib.rs` (wiring), `apps/engine-desktop/src/connect.rs` (`ChatGptConnectDriver`)
- Modify: `commands/core/src/connect.rs` (`chatgpt` branch + usage hint)
- Modify: `orchestrator/src/provider_adapter.rs` (label)
- Modify: `Cargo.toml` (workspace member `openai-oauth`)

---

## Milestone 1 — `openai-oauth` crate scaffold + constants

### Task 1.1: Create the crate skeleton and register it in the workspace

**Files:** Create `openai-oauth/Cargo.toml`, `openai-oauth/src/lib.rs`; Modify `Cargo.toml`.

- [ ] **Step 1: Write `openai-oauth/Cargo.toml`** (mirror anthropic-oauth's deps exactly)

```toml
[package]
name = "openai-oauth"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[dependencies]
protocol = { path = "../protocol" }
traits = { path = "../traits" }
secret = { path = "../secret" }
telemetry = { path = "../telemetry" }
llm-client = { path = "../llm-client" }
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true
async-trait.workspace = true
url = "2"
base64 = "0.22"
rand = "0.9"
sha2 = "0.10"
urlencoding = "2"
tokio = { workspace = true, features = ["sync", "time", "net", "io-util", "macros", "rt-multi-thread"] }
tracing.workspace = true

[dev-dependencies]
futures = "0.3"
tokio = { workspace = true, features = ["test-util"] }

[lints]
workspace = true
```

- [ ] **Step 2: Write a minimal `openai-oauth/src/lib.rs`**

```rust
//! OpenAI / ChatGPT-account OAuth login for llm-client.
//!
//! Mirrors the `anthropic-oauth` crate: PKCE + device-code login, token
//! refresh, and an `llm_client::CredentialProvider` that serves a
//! `Credential::ChatGptOAuth` (bearer + ChatGPT-Account-ID). Byte-aligned with
//! codex's ChatGPT auth (see docs/superpowers/specs/2026-06-16-p2-chatgpt-oauth-login-design.md).

pub mod config;
```

- [ ] **Step 3: Add `openai-oauth` to the workspace `members` and `default-members`**

In `Cargo.toml`, add `"openai-oauth",` immediately after the `"anthropic-oauth",` line in BOTH the `members = [` list (near line 70) and the `default-members = [` list (near line 161).

- [ ] **Step 4: Verify it builds**

Run: `CARGO_PROFILE_DEV_DEBUG=0 cargo build -p openai-oauth 2>&1 | tail -3`
Expected: `Finished` (config module is empty-but-declared; add it in 1.2 before this passes — if it fails on missing `config`, proceed to 1.2 then re-run).

- [ ] **Step 5: Commit** (after 1.2 so it compiles)

### Task 1.2: `config.rs` — OpenAI OAuth constants

**Files:** Create `openai-oauth/src/config.rs`.

- [ ] **Step 1: Write the failing test**

```rust
// at the bottom of config.rs
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn config_has_codex_constants() {
        let c = OpenAiOAuthConfig::default();
        assert_eq!(c.client_id, "app_EMoamEEZ73f0CkXaXp7hrann");
        assert_eq!(c.token_url, "https://auth.openai.com/oauth/token");
        assert_eq!(c.authorize_url, "https://auth.openai.com/oauth/authorize");
        assert!(c.scopes.contains("offline_access"));
        assert_eq!(c.codex_backend, "https://chatgpt.com/backend-api/codex");
        assert_eq!(c.loopback_ports, [1455, 1457]);
    }
}
```

- [ ] **Step 2: Run it, expect FAIL** (`OpenAiOAuthConfig` undefined)

Run: `CARGO_PROFILE_DEV_DEBUG=0 cargo test -p openai-oauth config 2>&1 | tail -10`

- [ ] **Step 3: Implement `config.rs`**

```rust
//! Static OpenAI OAuth endpoints + client_id + scopes + Codex backend URL.
//! Constants verified against codex `login/src/server.rs` and `model-provider-info`.

/// OpenAI OAuth configuration (issuer, client_id, endpoints, scopes, backend).
#[derive(Debug, Clone)]
pub struct OpenAiOAuthConfig {
    pub issuer: String,
    pub client_id: String,
    pub authorize_url: String,
    pub token_url: String,
    pub device_usercode_url: String,
    pub device_token_url: String,
    pub device_verify_url: String,
    pub scopes: String,
    pub codex_backend: String,
    /// Loopback redirect ports, in preference order (codex allowlist: 1455, then 1457).
    pub loopback_ports: [u16; 2],
}

impl Default for OpenAiOAuthConfig {
    fn default() -> Self {
        let issuer = "https://auth.openai.com".to_string();
        Self {
            authorize_url: format!("{issuer}/oauth/authorize"),
            token_url: format!("{issuer}/oauth/token"),
            device_usercode_url: format!("{issuer}/api/accounts/deviceauth/usercode"),
            device_token_url: format!("{issuer}/api/accounts/deviceauth/token"),
            device_verify_url: format!("{issuer}/codex/device"),
            client_id: "app_EMoamEEZ73f0CkXaXp7hrann".to_string(),
            scopes: "openid profile email offline_access api.connectors.read api.connectors.invoke"
                .to_string(),
            codex_backend: "https://chatgpt.com/backend-api/codex".to_string(),
            loopback_ports: [1455, 1457],
            issuer,
        }
    }
}

impl OpenAiOAuthConfig {
    /// The loopback redirect URI for a chosen port.
    #[must_use]
    pub fn redirect_uri(&self, port: u16) -> String {
        format!("http://localhost:{port}/auth/callback")
    }
}
```

- [ ] **Step 4: Run it, expect PASS.** Run: `CARGO_PROFILE_DEV_DEBUG=0 cargo test -p openai-oauth config 2>&1 | tail -6`

- [ ] **Step 5: Commit**

```bash
git add openai-oauth/Cargo.toml openai-oauth/src/lib.rs openai-oauth/src/config.rs Cargo.toml
git commit -m "feat(openai-oauth): crate scaffold + OpenAI OAuth config constants"
```

---

## Milestone 2 — leaf modules (pkce, callback, token_data)

### Task 2.1: Port `pkce.rs`

**Files:** Create `openai-oauth/src/pkce.rs`; Modify `lib.rs` (`pub mod pkce;`).

- [ ] **Step 1: Copy the template.** `cp anthropic-oauth/src/pkce.rs openai-oauth/src/pkce.rs`. This module is provider-agnostic (RFC-7636 verifier/S256 challenge + state). Deltas: none beyond ensuring no `anthropic`-specific imports remain (it has none).
- [ ] **Step 2: Add `pub mod pkce;` to `lib.rs`.**
- [ ] **Step 3: Run the ported tests.** Run: `CARGO_PROFILE_DEV_DEBUG=0 cargo test -p openai-oauth pkce 2>&1 | tail -6`. Expected: PASS (challenge == S256(verifier); state is url-safe random).
- [ ] **Step 4: Commit** `feat(openai-oauth): port pkce module`.

### Task 2.2: Port `callback.rs` with FIXED ports

**Files:** Create `openai-oauth/src/callback.rs`; Modify `lib.rs`.

- [ ] **Step 1: Copy** `anthropic-oauth/src/callback.rs` → `openai-oauth/src/callback.rs`.
- [ ] **Step 2: Apply deltas:**
  - Callback path: anthropic uses `/callback`; change to `/auth/callback`.
  - Binding: anthropic binds ephemeral `:0`. Change `CallbackListener::bind` to try the fixed ports in order: attempt `127.0.0.1:1455`, on `AddrInUse` fall back to `127.0.0.1:1457`, returning the bound port. Keep the rest (state validation, success page, `CallbackParams { code, state }`) identical.
- [ ] **Step 3: Write a test for the port-fallback + path** (add to the module's test mod):

```rust
#[tokio::test]
async fn binds_fixed_port_or_fallback() {
    let l = CallbackListener::bind().await.expect("bind");
    assert!(l.port() == 1455 || l.port() == 1457);
}
```

- [ ] **Step 4: Add `pub mod callback;` to `lib.rs`. Run tests.** Run: `CARGO_PROFILE_DEV_DEBUG=0 cargo test -p openai-oauth callback 2>&1 | tail -8`. Expected: PASS. (If port 1455 is busy in CI the test still passes via 1457.)
- [ ] **Step 5: Commit** `feat(openai-oauth): port callback listener (fixed ports 1455/1457, /auth/callback)`.

### Task 2.3: `token_data.rs` — parse id_token claims

**Files:** Create `openai-oauth/src/token_data.rs`; Modify `lib.rs`.

- [ ] **Step 1: Write the failing test** (a fixture JWT with the codex claims):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    // header.payload.sig with payload {"https://api.openai.com/auth":{"chatgpt_account_id":"acc_123","chatgpt_account_is_fedramp":false}}
    fn fixture_jwt() -> String {
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"none"}"#);
        let payload = URL_SAFE_NO_PAD.encode(
            br#"{"https://api.openai.com/auth":{"chatgpt_account_id":"acc_123","chatgpt_account_is_fedramp":false},"email":"u@example.com"}"#,
        );
        format!("{header}.{payload}.sig")
    }
    #[test]
    fn parses_account_id_and_fedramp() {
        let claims = parse_id_token(&fixture_jwt()).expect("parse");
        assert_eq!(claims.account_id.as_deref(), Some("acc_123"));
        assert!(!claims.fedramp);
        assert_eq!(claims.email.as_deref(), Some("u@example.com"));
    }
}
```

- [ ] **Step 2: Run, expect FAIL** (`parse_id_token` undefined).

- [ ] **Step 3: Implement `token_data.rs`** (parse the middle JWT segment; read claims from the `https://api.openai.com/auth` object, matching codex `token_data.rs`):

```rust
//! Parse OpenAI id_token JWT claims (no signature verification — the token came
//! straight from our own token exchange over TLS). Mirrors codex token_data.rs
//! claim names.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde_json::Value;

/// Claims we care about from the OpenAI id_token.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IdTokenClaims {
    /// ChatGPT workspace/account id → `ChatGPT-Account-ID` header.
    pub account_id: Option<String>,
    /// FedRAMP account flag → `X-OpenAI-Fedramp` header.
    pub fedramp: bool,
    /// User email (best-effort, for display).
    pub email: Option<String>,
}

/// Parse the JWT's payload segment and extract the claims. Returns `None` if the
/// token is malformed (not three dot-separated base64url segments / bad JSON).
#[must_use]
pub fn parse_id_token(jwt: &str) -> Option<IdTokenClaims> {
    let payload_b64 = jwt.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload_b64).ok()?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    let auth = v.get("https://api.openai.com/auth");
    let account_id = auth
        .and_then(|a| a.get("chatgpt_account_id"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let fedramp = auth
        .and_then(|a| a.get("chatgpt_account_is_fedramp"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let email = v.get("email").and_then(Value::as_str).map(str::to_string);
    Some(IdTokenClaims { account_id, fedramp, email })
}
```

- [ ] **Step 4: Add `pub mod token_data;` to `lib.rs`. Run tests, expect PASS.**
- [ ] **Step 5: Commit** `feat(openai-oauth): id_token claim parsing (account_id, fedramp)`.

---

## Milestone 3 — llm-client credential seam (the genuinely-new wire detail)

> Done before the OAuth client/refresh so `credential_provider.rs` (M4) can return the new `Credential` variant.

### Task 3.1: Add `Credential::ChatGptOAuth`

**Files:** Modify `llm-client/src/credentials.rs`.

- [ ] **Step 1: Write the failing test** (append to `credentials.rs` test mod, or add one):

```rust
#[test]
fn chatgpt_oauth_credential_redacts_token_in_debug() {
    let c = Credential::ChatGptOAuth {
        access_token: "sk-secret".to_string(),
        account_id: Some("acc_1".to_string()),
        fedramp: false,
    };
    let dbg = format!("{c:?}");
    assert!(!dbg.contains("sk-secret"));
    assert!(dbg.contains("ChatGptOAuth"));
}
```

- [ ] **Step 2: Run, expect FAIL.**

- [ ] **Step 3: Add the variant** to the `Credential` enum (after `BearerToken(String)`):

```rust
    /// ChatGPT-account OAuth: bearer access token plus the `ChatGPT-Account-ID`
    /// header (and FedRAMP flag). Served by the openai-oauth credential provider.
    ChatGptOAuth {
        /// OAuth access token (bearer).
        access_token: String,
        /// ChatGPT workspace/account id (the `ChatGPT-Account-ID` header).
        account_id: Option<String>,
        /// Whether the account is FedRAMP (sets `X-OpenAI-Fedramp: true`).
        fedramp: bool,
    },
```

- [ ] **Step 4: Add the Debug arm** (in the `impl fmt::Debug for Credential` match):

```rust
            Self::ChatGptOAuth { account_id, fedramp, .. } => formatter
                .debug_struct("ChatGptOAuth")
                .field("access_token", &"[REDACTED]")
                .field("account_id", account_id)
                .field("fedramp", fedramp)
                .finish(),
```

- [ ] **Step 5: Run, expect PASS.** Run: `CARGO_PROFILE_DEV_DEBUG=0 cargo test -p llm-client credentials 2>&1 | tail -6`.
- [ ] **Step 6: Commit** `feat(llm-client): add Credential::ChatGptOAuth variant`.

### Task 3.2: Add `AuthStrategy::ChatGptOAuth` + `ChatGptAuthenticator`

**Files:** Modify `llm-client/src/config.rs`, `llm-client/src/auth.rs`, `llm-client/src/lib.rs`.

- [ ] **Step 1: Write the failing test** in `auth.rs` test mod:

```rust
#[test]
fn chatgpt_authenticator_sets_bearer_and_account_header() {
    let auth = ChatGptAuthenticator::new("tok-123", Some("acc_9".to_string()), false);
    let req = auth.apply(ProviderRequest::post_json("https://x/responses", serde_json::json!({}))).unwrap();
    assert_eq!(req.headers.get("Authorization").map(String::as_str), Some("Bearer tok-123"));
    assert_eq!(req.headers.get("ChatGPT-Account-ID").map(String::as_str), Some("acc_9"));
    assert!(!req.headers.contains_key("X-OpenAI-Fedramp"));
}

#[test]
fn chatgpt_authenticator_sets_fedramp_when_flagged() {
    let auth = ChatGptAuthenticator::new("t", None, true);
    let req = auth.apply(ProviderRequest::post_json("https://x/responses", serde_json::json!({}))).unwrap();
    assert_eq!(req.headers.get("X-OpenAI-Fedramp").map(String::as_str), Some("true"));
    assert!(!req.headers.contains_key("ChatGPT-Account-ID")); // None → omitted
}
```

- [ ] **Step 2: Run, expect FAIL.**

- [ ] **Step 3: Add `ChatGptOAuth` to `AuthStrategy`** in `config.rs` (after `CopilotBearer`):

```rust
    /// ChatGPT-account OAuth: bearer access token + `ChatGPT-Account-ID` header.
    ChatGptOAuth,
```

- [ ] **Step 4: Add `ChatGptAuthenticator` to `auth.rs`** (after `BearerAuthenticator`):

```rust
/// Authenticator for ChatGPT-account OAuth: `Authorization: Bearer` plus the
/// `ChatGPT-Account-ID` header (and `X-OpenAI-Fedramp` when set). Mirrors codex
/// `model-provider/src/bearer_auth_provider.rs`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatGptAuthenticator {
    token: String,
    account_id: Option<String>,
    fedramp: bool,
}

impl ChatGptAuthenticator {
    /// Create a ChatGPT OAuth authenticator.
    #[must_use]
    pub fn new(token: impl Into<String>, account_id: Option<String>, fedramp: bool) -> Self {
        Self { token: token.into(), account_id, fedramp }
    }
}

impl Authenticator for ChatGptAuthenticator {
    fn apply(&self, mut request: ProviderRequest) -> Result<ProviderRequest, LlmError> {
        let headers = &mut request.headers;
        headers.remove("x-api-key");
        headers.insert("Authorization".to_string(), format!("Bearer {}", self.token));
        if let Some(acc) = &self.account_id {
            headers.insert("ChatGPT-Account-ID".to_string(), acc.clone());
        }
        if self.fedramp {
            headers.insert("X-OpenAI-Fedramp".to_string(), "true".to_string());
        }
        Ok(request)
    }
}
```

- [ ] **Step 5: Re-export `ChatGptAuthenticator`** from `lib.rs` wherever `BearerAuthenticator`/`ApiKeyAuthenticator` are re-exported (grep `pub use ... auth::` / `BearerAuthenticator` in `lib.rs` and add `ChatGptAuthenticator`).
- [ ] **Step 6: Run, expect PASS.** Run: `CARGO_PROFILE_DEV_DEBUG=0 cargo test -p llm-client auth 2>&1 | tail -8`.
- [ ] **Step 7: Commit** `feat(llm-client): AuthStrategy::ChatGptOAuth + ChatGptAuthenticator`.

### Task 3.3: Dispatch `ChatGptOAuth` in `authenticate_at`

**Files:** Modify `llm-client/src/client.rs`.

- [ ] **Step 1: Write the failing test.** Add an integration-style test in `llm-client/tests/client_auth_test.rs` (follow the existing patterns there) that builds a client whose profile has `AuthStrategy::ChatGptOAuth` and a credential provider returning `Credential::ChatGptOAuth { access_token:"t", account_id:Some("a"), fedramp:false }`, runs `authenticate_at`, and asserts the request carries `Authorization: Bearer t` + `ChatGPT-Account-ID: a`. (If the existing test harness exposes a narrower seam, mirror the closest existing `client_auth_test.rs` case.)

- [ ] **Step 2: Run, expect FAIL** (ChatGptOAuth arm missing → falls through / wrong header).

- [ ] **Step 3: Add the match arm** in `authenticate_at` (in `client.rs`, the `match entry.auth { ... }`). Add a dedicated arm BEFORE the `ApiKey | Bearer | OAuthBearer | CopilotBearer` arm:

```rust
            AuthStrategy::ChatGptOAuth => {
                let Some(secret) = self.load_credential(entry, profile_name).await? else {
                    return Ok(request);
                };
                let crate::Credential::ChatGptOAuth { access_token, account_id, fedramp } = secret
                else {
                    // Credential provider returned the wrong shape for this strategy.
                    return Err(crate::LlmError::Authentication);
                };
                let authenticator = crate::ChatGptAuthenticator::new(access_token, account_id, fedramp);
                request = authenticator.apply(request)?;
            }
```

Note: `load_credential` returns `Option<Credential>` (see `client.rs:539`). Confirm the surrounding code uses `load_credential` vs `load_secret`; match the existing pattern (the `CopilotBearer` arm uses `load_secret` which yields a `String`; for ChatGptOAuth use `load_credential` to get the full `Credential`). If `load_credential` is private and returns `Option<Credential>`, this compiles; adjust the binding to the actual signature.

- [ ] **Step 4: Run, expect PASS.** Run: `CARGO_PROFILE_DEV_DEBUG=0 cargo test -p llm-client 2>&1 | grep -E "test result:|FAILED" | head`.
- [ ] **Step 5: Commit** `feat(llm-client): route AuthStrategy::ChatGptOAuth through ChatGptAuthenticator`.

---

## Milestone 4 — OAuth client, refresh, credential provider, device-code

### Task 4.1: Port `client.rs` (authorize URL + code exchange + RFC-8693 mint)

**Files:** Create `openai-oauth/src/client.rs`; Modify `lib.rs`.

- [ ] **Step 1: Copy** `anthropic-oauth/src/client.rs` → `openai-oauth/src/client.rs`.
- [ ] **Step 2: Apply deltas:**
  - Rename `ClaudeAiOAuthClient` → `OpenAiOAuthClient`, `ClaudeAiOAuthConfig` → `OpenAiOAuthConfig`.
  - `build_authorize_url`: use `config.authorize_url`, `config.client_id`, `config.scopes`, and ADD the codex extra query params `id_token_add_organizations=true`, `codex_cli_simplified_flow=true`, `originator` (use a constant `"codex_cli_rs"` to match codex's originator; if codex uses a different literal, copy it verbatim from `login/src/server.rs` `originator()`), `code_challenge_method=S256`, `state`.
  - `exchange_code_*`: POST `config.token_url` form-encoded body `grant_type=authorization_code, code, redirect_uri, client_id, code_verifier`; parse `{ id_token, access_token, refresh_token }`. (Anthropic's exchange returns slightly different fields — adapt the response struct to these three.)
  - ADD `obtain_api_key(&self, id_token: &str) -> Result<String, _>`: POST `config.token_url` form-encoded `grant_type=urn:ietf:params:oauth:grant-type:token-exchange, client_id, requested_token=openai-api-key, subject_token=<id_token>, subject_token_type=urn:ietf:params:oauth:token-type:id_token`; parse `{ access_token }` (the minted key).
  - `init_refresh_driver`: keep the same lifecycle shape (constructs `AuthState`, spawns proactive task, returns `Arc<AuthState>`); it will use the openai `refresh.rs` from Task 4.2.
- [ ] **Step 3: Adapt the ported unit tests** to the OpenAI URL/params/body and add a test for `obtain_api_key`'s request body + response parse (mock HTTP via `testsupport`).
- [ ] **Step 4: Add `pub mod client;` to `lib.rs`. Run tests.** Run: `CARGO_PROFILE_DEV_DEBUG=0 cargo test -p openai-oauth client 2>&1 | tail -10`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(openai-oauth): port OAuth client (authorize, code exchange, RFC-8693 api-key mint)`.

### Task 4.2: Port `refresh.rs` (AuthState + RefreshDriver) and `testsupport.rs`

**Files:** Create `openai-oauth/src/refresh.rs`, `openai-oauth/src/testsupport.rs`; Modify `lib.rs`.

- [ ] **Step 1: Copy** `anthropic-oauth/src/testsupport.rs` → `openai-oauth/src/testsupport.rs` (mock HTTP, virtual clock, in-mem credential store). Adjust any anthropic-specific canned responses.
- [ ] **Step 2: Copy** `anthropic-oauth/src/refresh.rs` → `openai-oauth/src/refresh.rs`.
- [ ] **Step 3: Apply deltas:**
  - Refresh request: POST `config.token_url` JSON `{ client_id, grant_type:"refresh_token", refresh_token }`; parse `{ id_token?, access_token, refresh_token? }`.
  - After a successful refresh, RE-PARSE `id_token` (via `crate::token_data::parse_id_token`) and update the stored `account_id`/`fedramp` in `AuthState` so the credential provider serves the fresh account id. Add fields to `AuthState`'s token struct (or a sibling field) for `account_id: Option<String>` + `fedramp: bool`.
  - Proactive thresholds: refresh when `expires_at - now <= 5 min` OR `last_refresh` older than 8 days (codex parity).
- [ ] **Step 4: Adapt/keep the ported single-flight + proactive tests; add a test asserting account_id updates after a refresh returning a new id_token.**
- [ ] **Step 5: Add `pub mod refresh;` and `#[cfg(any(test, feature="testsupport"))] pub mod testsupport;` (match how anthropic-oauth gates testsupport) to `lib.rs`. Run tests.** Run: `CARGO_PROFILE_DEV_DEBUG=0 cargo test -p openai-oauth refresh 2>&1 | tail -10`.
- [ ] **Step 6: Commit** `feat(openai-oauth): port refresh driver (single-flight, proactive, account_id re-parse)`.

### Task 4.3: Port `credential_provider.rs` → serve `Credential::ChatGptOAuth`

**Files:** Create `openai-oauth/src/credential_provider.rs`; Modify `lib.rs`.

- [ ] **Step 1: Copy** `anthropic-oauth/src/credential_provider.rs` → `openai-oauth/src/credential_provider.rs`. Rename `OAuthCredentialProvider` → `OpenAiOAuthCredentialProvider`.
- [ ] **Step 2: Apply delta:** instead of returning `Credential::BearerToken(access_token)`, read `account_id`/`fedramp` from the (now-extended) `AuthState` token and return:

```rust
Ok(Credential::ChatGptOAuth { access_token: access_token_str, account_id, fedramp })
```

in both the not-expired and post-refresh paths.

- [ ] **Step 3: Write a test** (using `testsupport`): provider returns `ChatGptOAuth` with the seeded `account_id`; after expiry it refreshes and returns the updated token.
- [ ] **Step 4: Add `pub mod credential_provider;` to `lib.rs`. Run tests, expect PASS.**
- [ ] **Step 5: Commit** `feat(openai-oauth): credential provider serving Credential::ChatGptOAuth`.

### Task 4.4: `device_code.rs` — headless device-code flow

**Files:** Create `openai-oauth/src/device_code.rs`; Modify `lib.rs`.

- [ ] **Step 1: Write failing tests** (mock HTTP via testsupport): `request_device_code()` parses `{ device_auth_id, user_code, interval }`; `poll_for_token()` returns `Pending` on 403/404 and `Ready{authorization_code,..}` on 200.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::testsupport::*;
    #[tokio::test]
    async fn usercode_parses() {
        let http = MockHttp::new(vec![Canned::json(200, r#"{"device_auth_id":"d1","user_code":"ABCD","interval":"5"}"#)]);
        let cfg = crate::config::OpenAiOAuthConfig::default();
        let uc = request_device_code(&cfg, &http).await.expect("ok");
        assert_eq!(uc.user_code, "ABCD");
        assert_eq!(uc.device_auth_id, "d1");
    }
}
```

- [ ] **Step 2: Run, expect FAIL.**
- [ ] **Step 3: Implement `device_code.rs`** porting codex `login/src/device_code_auth.rs` shapes: POST `config.device_usercode_url` `{client_id}` → `DeviceUserCode{ device_auth_id, user_code, interval }`; POST `config.device_token_url` `{device_auth_id,user_code}` → on 200 `DeviceCode{authorization_code, code_challenge, code_verifier}`, on 403/404 `Poll::Pending`. Provide `run_device_code_login(cfg, http, clock)` that loops to the 15-min cap honoring `interval`, then hands the `authorization_code` (+ verifier) to `OpenAiOAuthClient::exchange_code_for_tokens`.
- [ ] **Step 4: Add `pub mod device_code;`. Run tests, expect PASS.**
- [ ] **Step 5: Commit** `feat(openai-oauth): device-code login flow`.

---

## Milestone 5 — login handle (orchestration + persistence)

### Task 5.1: Port `handle.rs` → `OpenAiOAuthHandle`

**Files:** Create `openai-oauth/src/handle.rs`; Modify `lib.rs`.

- [ ] **Step 1: Copy** `anthropic-oauth/src/handle.rs` → `openai-oauth/src/handle.rs`. Rename `OAuthHandle` → `OpenAiOAuthHandle`.
- [ ] **Step 2: Apply deltas:**
  - Use `OpenAiOAuthClient` + `OpenAiOAuthConfig`; redirect via `config.redirect_uri(bound_port)`; bind via the fixed-port `CallbackListener::bind()`.
  - After `exchange_code_for_tokens`, parse `id_token` for `account_id`/`fedramp` (via `token_data`), call `obtain_api_key` (store the minted key too), and persist via `CredentialManager::store_oauth_tokens(...)`. **Storage keys:** use OpenAI-specific keychain accounts. Check `secret/src/credential.rs` — if `store_oauth_tokens` hardcodes the `anthropic-oauth-*` account names, add a parallel `store_openai_oauth_tokens` / parameterize by a provider key prefix (see Task 5.2). Persist account_id in the meta entry.
  - Add `login_device_code()` driving `device_code::run_device_code_login` then the same persistence.
- [ ] **Step 3: Adapt ported tests** (injected browser opener no-op + mock callback) to the OpenAI flow.
- [ ] **Step 4: Add `pub mod handle;`. Run tests, expect PASS.**
- [ ] **Step 5: Commit** `feat(openai-oauth): login handle (PKCE browser + device-code) with persistence`.

### Task 5.2: OpenAI keychain storage in `secret`

**Files:** Modify `lingxi-code/secret/src/credential.rs`.

- [ ] **Step 1: Inspect** `secret/src/credential.rs` around the OAuth storage (`OAUTH_SERVICE`, `OAUTH_*_ACCOUNT`, `store_oauth_tokens`, `get_oauth_tokens`). Determine whether token storage is anthropic-specific.
- [ ] **Step 2: Write failing tests** for OpenAI token storage round-trip:

```rust
#[tokio::test]
async fn openai_oauth_tokens_round_trip() {
    let cm = test_manager();
    cm.store_openai_oauth_tokens("acc", "ref", expires(), &["openid".into()], Some("acc_1"), None).await.unwrap();
    let got = cm.get_openai_oauth_tokens().await.unwrap().unwrap();
    assert_eq!(got.access_token.expose_secret(), "acc");
    assert_eq!(got.account_id.as_deref(), Some("acc_1"));
}
```

(Adjust signature to match the existing `store_oauth_tokens` shape; add an `account_id` field to the stored meta.)

- [ ] **Step 2b: Run, expect FAIL.**
- [ ] **Step 3: Implement** `store_openai_oauth_tokens` / `get_openai_oauth_tokens` / `delete_openai_oauth_tokens` using distinct keychain accounts (`openai-oauth-access`, `openai-oauth-refresh`, `openai-oauth-meta`), with `account_id` in the meta JSON. Keep the anthropic functions untouched.
- [ ] **Step 4: Run, expect PASS.** Run: `CARGO_PROFILE_DEV_DEBUG=0 cargo test -p secret oauth 2>&1 | tail -8`.
- [ ] **Step 5: Commit** `feat(secret): OpenAI OAuth token keychain storage`.

---

## Milestone 6 — provider preset + slice + count guards

### Task 6.1: Make `Preset.credential_env` optional

**Files:** Modify `llm-client/src/catalog/presets.rs`.

- [ ] **Step 1:** Change the `Preset` struct field `credential_env: &'static str` → `credential_env: Option<&'static str>`.
- [ ] **Step 2:** Update the 5 existing preset entries (openrouter/deepseek/glm-coding/zai/github-copilot) and the P1 `openai` entry to wrap their env var in `Some(...)` (e.g. `credential_env: Some("OPENROUTER_API_KEY")`).
- [ ] **Step 3:** In `builtin_presets()`, change the credential construction:

```rust
            credential: match preset.credential_env {
                Some(var) => CredentialConfig::Env { var: var.to_string() },
                None => CredentialConfig::Static { id: preset.profile_name.to_string() },
            },
```

- [ ] **Step 4: Run the presets tests, expect PASS** (no behavior change yet). Run: `CARGO_PROFILE_DEV_DEBUG=0 cargo test -p llm-client catalog::presets 2>&1 | tail -6`.
- [ ] **Step 5: Commit** `refactor(llm-client): Preset.credential_env is Option (supports OAuth presets)`.

### Task 6.2: Hand-author the `openai-chatgpt` model slice

**Files:** Create `llm-client/data/models-dev/openai-chatgpt.json`.

- [ ] **Step 1: Write the slice** (models.dev-shaped; 3 Codex models):

```json
{
  "_comment": "Hand-authored (not from models.dev): codex's Codex-backend model list is dynamic (filter_by_auth) and not published on models.dev. Models reachable via ChatGPT-account login against https://chatgpt.com/backend-api/codex.",
  "id": "openai-chatgpt",
  "name": "OpenAI (ChatGPT login)",
  "env": [],
  "models": {
    "gpt-5.3-codex": { "id": "gpt-5.3-codex", "name": "GPT-5.3 Codex", "tool_call": true, "reasoning": true, "modalities": { "input": ["text"], "output": ["text"] } },
    "gpt-5-codex":   { "id": "gpt-5-codex",   "name": "GPT-5 Codex",   "tool_call": true, "reasoning": true, "modalities": { "input": ["text"], "output": ["text"] } },
    "gpt-5.2":       { "id": "gpt-5.2",       "name": "GPT-5.2",       "tool_call": true, "reasoning": true, "modalities": { "input": ["text"], "output": ["text"] } }
  }
}
```

- [ ] **Step 2: Verify it parses as a `ProviderSlice`** with 3 models:

```bash
python3 -c "import json; d=json.load(open('llm-client/data/models-dev/openai-chatgpt.json')); print('models:', len(d['models']), sorted(d['models']))"
```
Expected: `models: 3 ['gpt-5-codex', 'gpt-5.2', 'gpt-5.3-codex']`

- [ ] **Step 3: Commit** `feat(llm-client): hand-authored openai-chatgpt model slice (codex backend)`.

### Task 6.3: Add the `openai-chatgpt` preset + guards + label

**Files:** Modify `llm-client/src/catalog/presets.rs`, `provider-config/src/assemble.rs`, `provider-config/src/lib.rs`, `orchestrator/src/provider_adapter.rs`, `apps/engine-desktop/src/lib.rs`.

- [ ] **Step 1: Write the failing count-guard test** in `presets.rs`: change `providers.len()` from 6 to 7; add `assert_eq!(count("openai-chatgpt"), 3);` plus decision-locking asserts:

```rust
        let chatgpt = catalog.providers.iter().find(|p| p.profile_name == "openai-chatgpt").expect("present");
        assert_eq!(chatgpt.protocol, ProtocolFamily::OpenAiResponses);
        assert_eq!(chatgpt.auth, AuthStrategy::ChatGptOAuth);
        assert_eq!(chatgpt.base_url, "https://chatgpt.com/backend-api/codex");
```

- [ ] **Step 2: Run, expect FAIL** (`left: 6, right: 7`).
- [ ] **Step 3: Add the const + preset.** Add `const OPENAI_CHATGPT: &str = include_str!("../../data/models-dev/openai-chatgpt.json");` and the entry (after the P1 `openai` preset):

```rust
        // OpenAI via ChatGPT-account OAuth login: routes to the Codex backend
        // (Responses API). Credential is OAuth (no env var) → resolved by the
        // openai-oauth credential provider via MultiCredentialProvider, keyed by
        // credential_id "openai-chatgpt". See P2 design doc.
        Preset {
            profile_name: "openai-chatgpt",
            base_url: "https://chatgpt.com/backend-api/codex",
            protocol: ProtocolFamily::OpenAiResponses,
            auth: AuthStrategy::ChatGptOAuth,
            provider_id: ProviderId::OpenAICompatible { name: "openai-chatgpt".to_string() },
            credential_env: None,
            slice_json: OPENAI_CHATGPT,
        },
```

- [ ] **Step 4: Run presets tests, expect PASS.**
- [ ] **Step 5: Update provider-config guards:** `assemble.rs` provider count 7→8 + `assert!(names.contains(&"openai-chatgpt"))`; `lib.rs` smoke count 6→7. Run `CARGO_PROFILE_DEV_DEBUG=0 cargo test -p provider-config 2>&1 | grep -E "test result:|FAILED"`.
- [ ] **Step 6: Add labels** `"openai-chatgpt" => "OpenAI (ChatGPT login)"` to `provider_label` (orchestrator) and `provider_profile_label` (engine-desktop). Build both: `CARGO_PROFILE_DEV_DEBUG=0 cargo build -p orchestrator -p engine-desktop 2>&1 | tail -3`.
- [ ] **Step 7: Commit** `feat(llm-client): openai-chatgpt preset (Codex backend, ChatGptOAuth) + guards + label`.

---

## Milestone 7 — MultiCredentialProvider per-credential-id OAuth delegates

### Task 7.1: Generalize `MultiCredentialProvider` to a map of OAuth delegates

**Files:** Modify `provider-config/src/credentials.rs`.

- [ ] **Step 1: Write the failing test** (add to the test mod): two OAuth delegates, dispatched by credential_id:

```rust
#[tokio::test]
async fn routes_oauth_delegate_by_credential_id() {
    let anthropic = Arc::new(StubProvider(Credential::BearerToken("ANT".into())));
    let openai = Arc::new(StubProvider(Credential::ChatGptOAuth { access_token: "OAI".into(), account_id: Some("a".into()), fedramp: false }));
    let mut delegates: std::collections::BTreeMap<String, Arc<dyn CredentialProvider>> = Default::default();
    delegates.insert("anthropic-oauth".into(), anthropic);
    delegates.insert("openai-chatgpt".into(), openai);
    let mcp = MultiCredentialProvider::new(manager(), vec![], None, delegates);
    let got = mcp.load(&scope(ProviderId::OpenAICompatible{name:"openai-chatgpt".into()}, "openai-chatgpt", "openai-chatgpt")).await.unwrap();
    assert!(matches!(got, Credential::ChatGptOAuth { .. }));
}
```

(Add a small `StubProvider` test helper if one doesn't already exist in the test mod.)

- [ ] **Step 2: Run, expect FAIL** (signature mismatch / no per-id routing).

- [ ] **Step 3: Refactor the struct + constructor.** Replace the single `oauth_delegate: Option<Arc<dyn CredentialProvider>>` with `oauth_delegates: BTreeMap<String, Arc<dyn CredentialProvider>>`:

```rust
pub struct MultiCredentialProvider {
    credentials: Arc<secret::CredentialManager>,
    sources: BTreeMap<String, CredentialSource>,
    anthropic_api_key: Option<String>,
    oauth_delegates: BTreeMap<String, Arc<dyn CredentialProvider>>,
}

impl MultiCredentialProvider {
    #[must_use]
    pub fn new(
        credentials: Arc<secret::CredentialManager>,
        sources: Vec<CredentialSource>,
        anthropic_api_key: Option<String>,
        oauth_delegates: BTreeMap<String, Arc<dyn CredentialProvider>>,
    ) -> Self {
        let sources = sources.into_iter().map(|s| (s.credential_id.clone(), s)).collect();
        Self { credentials, sources, anthropic_api_key, oauth_delegates }
    }
}
```

Update the Debug impl field to `&self.oauth_delegates.keys().collect::<Vec<_>>()`.

- [ ] **Step 4: Refactor `load` dispatch:**

```rust
            // Any registered OAuth delegate wins for its credential_id (anthropic-oauth, openai-chatgpt, …).
            if let Some(delegate) = self.oauth_delegates.get(credential_id) {
                return delegate.load(scope).await;
            }
            match credential_id {
                "anthropic-api-key" => self.anthropic_api_key.clone().map(Credential::ApiKey).ok_or(LlmError::Authentication),
                other => self.load_provider_key(other).await,
            }
```

(Removes the hardcoded `"anthropic-oauth"` arm — it's now a delegate keyed by that id.)

- [ ] **Step 5: Update existing callers' compile.** Search the two call sites (`apps/engine-desktop/src/lib.rs`, `apps/engine-mobile/src/host.rs`) — they currently pass `oauth_delegate: Option<...>`. Update them to build a `BTreeMap` (anthropic delegate inserted under `"anthropic-oauth"` when present). (Engine-desktop is fully rewired in Task 8; for mobile, wrap the existing single delegate into a one-entry map to keep it compiling.)
- [ ] **Step 6: Run, expect PASS.** Run: `CARGO_PROFILE_DEV_DEBUG=0 cargo test -p provider-config 2>&1 | grep -E "test result:|FAILED"`.
- [ ] **Step 7: Commit** `refactor(provider-config): MultiCredentialProvider routes OAuth delegates per credential_id`.

---

## Milestone 8 — engine wiring + /connect chatgpt

### Task 8.1: `ChatGptConnectDriver` seam + engine construction

**Files:** Modify `apps/engine-desktop/src/connect.rs`, `apps/engine-desktop/src/lib.rs`.

- [ ] **Step 1:** In `apps/engine-desktop/src/connect.rs`, study the existing `CopilotConnectDriver` seam and add an analogous `ChatGptConnectDriver` trait + an impl wrapping `openai_oauth::OpenAiOAuthHandle::login` (browser PKCE) with a `login_device_code` fallback. Add `openai-oauth` to `apps/engine-desktop/Cargo.toml` deps.
- [ ] **Step 2:** In `apps/engine-desktop/src/lib.rs` build(): construct `OpenAiOAuthClient`/`OpenAiOAuthHandle`; detect stored OpenAI tokens via `credentials.get_openai_oauth_tokens()`; if present, `openai_oauth::init_refresh_driver(...)` → build `OpenAiOAuthCredentialProvider`; insert it into the `oauth_delegates` map under `"openai-chatgpt"` (alongside the existing anthropic delegate under `"anthropic-oauth"`); pass the map to `MultiCredentialProvider::new`.
- [ ] **Step 3:** Build the engine: `CARGO_PROFILE_DEV_DEBUG=0 cargo build -p engine-desktop 2>&1 | tail -5`. Expected: compiles.
- [ ] **Step 4:** Add a focused test if the build phase has a unit-testable helper (mirror any existing `oauth_subscriber_flag`-style test); otherwise rely on the compile + Task 9 integration. 
- [ ] **Step 5: Commit** `feat(engine-desktop): wire openai-oauth credential provider + ChatGptConnectDriver`.

### Task 8.2: `/connect chatgpt` branch

**Files:** Modify `commands/core/src/connect.rs`, and the registration where `ChatGptConnectDriver` is injected (mirror `connect_copilot`).

- [ ] **Step 1: Write the failing test** in `connect.rs` test mod (mirror the existing copilot test): a mock `ChatGptConnectDriver` that succeeds; `/connect chatgpt` returns `Connected chatgpt.`-style display.
- [ ] **Step 2: Run, expect FAIL.**
- [ ] **Step 3: Implement** the `chatgpt` (alias `openai-chatgpt`) branch in `ConnectHandler::handle`, paralleling the `github-copilot` branch but calling the `ChatGptConnectDriver`. Update the usage hint string to include `chatgpt`. Add the driver to `ConnectHandler::new` (mirror the copilot seam) and its registration in `register_core_connect`.
- [ ] **Step 4: Run, expect PASS.** Run: `CARGO_PROFILE_DEV_DEBUG=0 cargo test -p command-core connect 2>&1 | tail -8`.
- [ ] **Step 5: Commit** `feat(connect): /connect chatgpt drives the OpenAI OAuth login`.

---

## Milestone 9 — full integration verification

### Task 9.1: Workspace build + affected-crate tests

- [ ] **Step 1: Build the whole workspace.** Run: `CARGO_PROFILE_DEV_DEBUG=0 cargo build --workspace 2>&1 | tail -5`. Expected: `Finished`, no errors.
- [ ] **Step 2: Run all affected crates.** Run:

```bash
CARGO_PROFILE_DEV_DEBUG=0 cargo test -p openai-oauth -p llm-client -p provider-config -p secret -p command-core -p orchestrator -p engine-desktop 2>&1 | grep -E "test result:|FAILED|error\[|error:" | grep -v "0 passed; 0 failed"
```
Expected: only `test result: ok` lines; zero FAILED/error.

- [ ] **Step 3: Clippy the new crate + touched crates.** Run: `CARGO_PROFILE_DEV_DEBUG=0 cargo clippy -p openai-oauth -p llm-client -p provider-config 2>&1 | grep -E "warning:|error:" | head`. Expected: clean (fix any lints in the new crate).
- [ ] **Step 4: Commit** any clippy fixes: `chore(openai-oauth): clippy clean`.

No further commit. P2 complete.

---

## Notes for the implementer

- **Port, don't reinvent:** the `anthropic-oauth` crate is the working template. Diff your ported module against its source to confirm you changed only the listed deltas.
- **Secrets never logged:** preserve anthropic-oauth's redaction (URLs/bodies); `Credential::ChatGptOAuth`'s Debug is already redacted (Task 3.1).
- **Fixed ports:** the loopback redirect MUST be `localhost:1455` (fallback `1457`) — codex's OAuth app allowlists only these. Do not use ephemeral `:0`.
- **`originator` literal:** copy the exact value codex sends from `login/src/server.rs` `originator()` if it differs from `codex_cli_rs`.
- **Do NOT** implement PersonalAccessToken / ChatgptAuthTokens / AgentIdentity — those are P3.
- Every cargo command keeps the `CARGO_PROFILE_DEV_DEBUG=0` prefix. `command-core` is the crate name (singular).
- If a ported module pulls an anthropic-only dependency (e.g. `subscription`/`limits`/`scope_upgrade`/`profile`/`resolver`), it is NOT needed for P2 — do not port those modules; only port the ones listed (pkce, callback, token_data, client, refresh, credential_provider, device_code, handle, testsupport).
