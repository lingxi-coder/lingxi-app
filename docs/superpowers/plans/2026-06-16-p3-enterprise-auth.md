# P3 — Enterprise auth (PAT + external tokens) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add two enterprise OpenAI auth modes — Personal Access Token (PAT, with a `whoami` lookup) and externally-supplied ChatGPT tokens — that feed the existing `openai-chatgpt` Codex-backend provider, reusing the P2 `Credential::ChatGptOAuth` + `ChatGptAuthenticator` (no new AuthStrategy/Credential/authenticator).

**Architecture:** Two new static `CredentialProvider`s in the `openai-oauth` crate (`PatCredentialProvider`, `ExternalTokensCredentialProvider`), both returning `Credential::ChatGptOAuth`. The engine selects the `openai-chatgpt` delegate by precedence (PAT env → external-tokens env → OAuth session), all env-var configured. Picker availability ORs the three sources.

**Tech Stack:** Rust workspace (`lingxi-code/`). `tokio`, injected `traits::HttpTransport`, `serde`. Tests use the `openai-oauth` `testsupport` mock-HTTP pattern. Run cargo with `CARGO_PROFILE_DEV_DEBUG=0` (disk near-full).

**Reference spec:** `docs/superpowers/specs/2026-06-16-p3-enterprise-auth-design.md`.

**Working dir:** `/Users/luolingfeng/Projects/LingXi-Next/lingxi-code`. Branch: `p3-enterprise-auth`.

## Reused P2 facts (verified)
- `llm_client::Credential::ChatGptOAuth { access_token: String, account_id: Option<String>, fedramp: bool }`; `AuthStrategy::ChatGptOAuth` + `ChatGptAuthenticator` already dispatch in `client.rs`.
- `llm_client::{CredentialProvider, CredentialScope, BoxFuture, Credential, LlmError}` — trait: `fn load<'a>(&'a self, scope: &'a CredentialScope) -> BoxFuture<'a, Result<Credential, LlmError>>`.
- `openai-oauth` already exports `OpenAiOAuthConfig`, `OAuthError`, `token_data::parse_id_token`. HTTP shape (from `device_code.rs`): `protocol::HttpRequest { method: protocol::HttpMethod::{Get,Post}, url, headers: Vec<(String,String)>, body: Option<String>, body_bytes: None, timeout: Option<Duration> }`; `traits::HttpTransport::request(req).await -> Result<resp,_>` where `resp.status: u16`, `resp.body: String`.
- `compute_availability(credentials, sources, anthropic_has_api_key, anthropic_has_oauth, openai_chatgpt_has_oauth)` — has an arm `"openai-chatgpt" => openai_chatgpt_has_oauth`. Called once in `apps/engine-desktop/src/lib.rs:2745` passing `has_openai_oauth`.
- Engine `openai-chatgpt` delegate built in `apps/engine-desktop/src/lib.rs` ~1414–1516: builds `openai_oauth_cfg`/`openai_oauth_client`, detects `get_openai_oauth_tokens()` → `init_refresh_driver` → `openai_oauth_state`, then inserts `OpenAiOAuthCredentialProvider` under `"openai-chatgpt"` (line ~1510–1516); `has_openai_oauth = openai_oauth_state.is_some()` (line ~1509).

---

## File Structure
- Create: `openai-oauth/src/pat.rs`, `openai-oauth/src/external_tokens.rs`
- Modify: `openai-oauth/src/config.rs` (authapi_base_url + whoami_url), `openai-oauth/src/lib.rs` (modules + exports)
- Modify: `provider-config/src/availability.rs` (rename param `openai_chatgpt_has_oauth` → `openai_chatgpt_available`)
- Modify: `apps/engine-desktop/src/lib.rs` (precedence selector + availability OR-flag)

---

## Task 1: `config.rs` — authapi base URL + whoami URL

**Files:** Modify `openai-oauth/src/config.rs`.

- [ ] **Step 1: Add the failing test** (append to the `#[cfg(test)] mod tests`):

```rust
    #[test]
    fn config_has_authapi_and_whoami() {
        let c = OpenAiOAuthConfig::default();
        assert_eq!(c.authapi_base_url, "https://auth.openai.com/api/accounts");
        assert_eq!(c.whoami_url(), "https://auth.openai.com/api/accounts/v1/user-auth-credential/whoami");
    }
```

- [ ] **Step 2: Run, expect FAIL.** Run: `CARGO_PROFILE_DEV_DEBUG=0 cargo test -p openai-oauth config 2>&1 | tail -8`

- [ ] **Step 3: Add the field + default + helper.** In the `OpenAiOAuthConfig` struct add (after `codex_backend`):

```rust
    /// AuthAPI base URL for PAT whoami + (codex) account endpoints.
    pub authapi_base_url: String,
```

In `Default::default()` add (alongside the other `format!`s; reuse the existing `issuer` local):

```rust
            authapi_base_url: format!("{issuer}/api/accounts"),
```

In the `impl OpenAiOAuthConfig` block add:

```rust
    /// The PAT `whoami` endpoint (resolves account_id / fedramp for a PAT).
    #[must_use]
    pub fn whoami_url(&self) -> String {
        format!("{}/v1/user-auth-credential/whoami", self.authapi_base_url.trim_end_matches('/'))
    }
```

- [ ] **Step 4: Run, expect PASS.** Run: `CARGO_PROFILE_DEV_DEBUG=0 cargo test -p openai-oauth config 2>&1 | tail -6`
- [ ] **Step 5: Commit**

```bash
git add openai-oauth/src/config.rs
git commit -m "feat(openai-oauth): config authapi_base_url + whoami_url"
```

---

## Task 2: `pat.rs` — whoami + PatCredentialProvider

**Files:** Create `openai-oauth/src/pat.rs`; Modify `openai-oauth/src/lib.rs`.

- [ ] **Step 1: Write the module with impl + tests** (TDD: the tests are included; they fail until the impl compiles, then pass). Create `openai-oauth/src/pat.rs`:

```rust
//! Personal Access Token (PAT) auth for the OpenAI Codex backend.
//!
//! A PAT (`at-…`) is a long-lived bearer token. On load the engine resolves the
//! account_id / fedramp once via `whoami`, then a static credential provider
//! serves `Credential::ChatGptOAuth` per request (no refresh). Byte-aligned with
//! codex `login/src/auth/personal_access_token.rs`.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use llm_client::{BoxFuture, Credential, CredentialProvider, CredentialScope, LlmError};
use protocol::{HttpMethod, HttpRequest};
use serde::Deserialize;
use traits::HttpTransport;

use crate::client::OAuthError;
use crate::config::OpenAiOAuthConfig;

/// Account metadata resolved from the PAT `whoami` call.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PatMetadata {
    /// ChatGPT workspace/account id → `ChatGPT-Account-ID` header.
    pub account_id: Option<String>,
    /// FedRAMP account flag → `X-OpenAI-Fedramp` header.
    pub fedramp: bool,
    /// User email (best-effort).
    pub email: Option<String>,
    /// Subscription plan type (best-effort).
    pub plan: Option<String>,
}

#[derive(Deserialize)]
struct WhoamiResp {
    #[serde(default)]
    chatgpt_account_id: Option<String>,
    #[serde(default)]
    chatgpt_account_is_fedramp: bool,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    chatgpt_plan_type: Option<String>,
}

/// Resolve account metadata for a PAT via `GET {authapi}/v1/user-auth-credential/whoami`.
///
/// # Errors
/// [`OAuthError::TokenExchange`] on transport failure or non-200 status.
pub async fn whoami(
    cfg: &OpenAiOAuthConfig,
    http: &Arc<dyn HttpTransport>,
    pat: &str,
) -> Result<PatMetadata, OAuthError> {
    let req = HttpRequest {
        method: HttpMethod::Get,
        url: cfg.whoami_url(),
        headers: vec![
            ("authorization".into(), format!("Bearer {pat}")),
            ("accept".into(), "application/json".into()),
        ],
        body: None,
        body_bytes: None,
        timeout: Some(Duration::from_secs(15)),
    };
    let resp = http
        .request(req)
        .await
        .map_err(|e| OAuthError::TokenExchange(format!("whoami transport: {e}")))?;
    if resp.status != 200 {
        return Err(OAuthError::TokenExchange(format!(
            "whoami failed with status {}",
            resp.status
        )));
    }
    let raw: WhoamiResp = serde_json::from_str(&resp.body)
        .map_err(|e| OAuthError::TokenExchange(format!("whoami decode: {e}")))?;
    Ok(PatMetadata {
        account_id: raw.chatgpt_account_id,
        fedramp: raw.chatgpt_account_is_fedramp,
        email: raw.email,
        plan: raw.chatgpt_plan_type,
    })
}

/// Static credential provider for a PAT. Serves `Credential::ChatGptOAuth` with
/// the PAT as the bearer + the resolved account_id/fedramp. No refresh.
pub struct PatCredentialProvider {
    pat: String,
    account_id: Option<String>,
    fedramp: bool,
}

impl PatCredentialProvider {
    /// Build from a PAT + its resolved metadata (the engine calls [`whoami`] once).
    #[must_use]
    pub fn new(pat: impl Into<String>, metadata: PatMetadata) -> Self {
        Self { pat: pat.into(), account_id: metadata.account_id, fedramp: metadata.fedramp }
    }
}

impl fmt::Debug for PatCredentialProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PatCredentialProvider")
            .field("pat", &"[REDACTED]")
            .field("account_id", &self.account_id)
            .field("fedramp", &self.fedramp)
            .finish()
    }
}

impl CredentialProvider for PatCredentialProvider {
    fn load<'a>(
        &'a self,
        _scope: &'a CredentialScope,
    ) -> BoxFuture<'a, Result<Credential, LlmError>> {
        let cred = Credential::ChatGptOAuth {
            access_token: self.pat.clone(),
            account_id: self.account_id.clone(),
            fedramp: self.fedramp,
        };
        Box::pin(async move { Ok(cred) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testsupport::{Canned, MockHttp};

    fn cfg() -> OpenAiOAuthConfig {
        OpenAiOAuthConfig::default()
    }

    #[tokio::test]
    async fn whoami_parses_account_and_fedramp() {
        let http: Arc<dyn HttpTransport> = Arc::new(MockHttp::new(vec![Canned::json(
            200,
            r#"{"chatgpt_account_id":"acc_7","chatgpt_account_is_fedramp":true,"email":"u@x.com","chatgpt_plan_type":"pro"}"#,
        )]));
        let md = whoami(&cfg(), &http, "at-token").await.expect("ok");
        assert_eq!(md.account_id.as_deref(), Some("acc_7"));
        assert!(md.fedramp);
        assert_eq!(md.email.as_deref(), Some("u@x.com"));
    }

    #[tokio::test]
    async fn whoami_non_200_errors() {
        let http: Arc<dyn HttpTransport> = Arc::new(MockHttp::new(vec![Canned::json(401, "{}")]));
        assert!(whoami(&cfg(), &http, "at-bad").await.is_err());
    }

    #[tokio::test]
    async fn provider_returns_chatgpt_oauth() {
        let p = PatCredentialProvider::new(
            "at-token",
            PatMetadata { account_id: Some("acc_7".into()), fedramp: false, ..Default::default() },
        );
        let scope = CredentialScope::new(
            llm_client::ProviderId::OpenAICompatible { name: "openai-chatgpt".into() },
            "openai-chatgpt",
        );
        let got = p.load(&scope).await.expect("load");
        match got {
            Credential::ChatGptOAuth { access_token, account_id, fedramp } => {
                assert_eq!(access_token, "at-token");
                assert_eq!(account_id.as_deref(), Some("acc_7"));
                assert!(!fedramp);
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn debug_redacts_pat() {
        let p = PatCredentialProvider::new("at-secret", PatMetadata::default());
        assert!(!format!("{p:?}").contains("at-secret"));
    }
}
```

NOTE: verify the exact `testsupport` constructor names (`MockHttp::new`, `Canned::json`) by reading `openai-oauth/src/testsupport.rs` — match whatever the device_code/client tests use; adjust the test helper calls if they differ. Likewise confirm `OAuthError::TokenExchange` is a real variant (read `client.rs`); if the closest variant is named differently (e.g. `OAuthError::Http`/`OAuthError::DeviceCode`), use a general one and keep the message.

- [ ] **Step 2: Add `pub mod pat;` and exports to `lib.rs`.** Add `pub mod pat;` with the other `pub mod`s, and `pub use pat::{whoami, PatCredentialProvider, PatMetadata};` with the other `pub use`s.

- [ ] **Step 3: Run, expect PASS.** Run: `CARGO_PROFILE_DEV_DEBUG=0 cargo test -p openai-oauth pat 2>&1 | tail -10`
- [ ] **Step 4: Commit**

```bash
git add openai-oauth/src/pat.rs openai-oauth/src/lib.rs
git commit -m "feat(openai-oauth): PAT whoami + PatCredentialProvider"
```

---

## Task 3: `external_tokens.rs` — ExternalTokensCredentialProvider

**Files:** Create `openai-oauth/src/external_tokens.rs`; Modify `openai-oauth/src/lib.rs`.

- [ ] **Step 1: Write the module with impl + tests.** Create `openai-oauth/src/external_tokens.rs`:

```rust
//! Externally-supplied ChatGPT tokens (codex `ChatgptAuthTokens` mode).
//!
//! An enterprise auth server supplies a ready access token + account_id; we serve
//! it statically as `Credential::ChatGptOAuth` (no refresh — the external system
//! owns the lifecycle). Same header set as the OAuth-login path.

use std::fmt;

use llm_client::{BoxFuture, Credential, CredentialProvider, CredentialScope, LlmError};

/// Static credential provider over externally-supplied ChatGPT tokens.
pub struct ExternalTokensCredentialProvider {
    access_token: String,
    account_id: Option<String>,
    fedramp: bool,
}

impl ExternalTokensCredentialProvider {
    /// Build from a supplied access token + account_id. `fedramp` is parsed from
    /// the token's `id_token` claims when the token is a JWT (best-effort; else false).
    #[must_use]
    pub fn from_supplied(access_token: impl Into<String>, account_id: Option<String>) -> Self {
        let access_token = access_token.into();
        let fedramp = crate::token_data::parse_id_token(&access_token)
            .map(|c| c.fedramp)
            .unwrap_or(false);
        Self { access_token, account_id, fedramp }
    }
}

impl fmt::Debug for ExternalTokensCredentialProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExternalTokensCredentialProvider")
            .field("access_token", &"[REDACTED]")
            .field("account_id", &self.account_id)
            .field("fedramp", &self.fedramp)
            .finish()
    }
}

impl CredentialProvider for ExternalTokensCredentialProvider {
    fn load<'a>(
        &'a self,
        _scope: &'a CredentialScope,
    ) -> BoxFuture<'a, Result<Credential, LlmError>> {
        let cred = Credential::ChatGptOAuth {
            access_token: self.access_token.clone(),
            account_id: self.account_id.clone(),
            fedramp: self.fedramp,
        };
        Box::pin(async move { Ok(cred) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn returns_chatgpt_oauth_with_supplied_account() {
        let p = ExternalTokensCredentialProvider::from_supplied("plain-token", Some("acc_2".into()));
        let scope = CredentialScope::new(
            llm_client::ProviderId::OpenAICompatible { name: "openai-chatgpt".into() },
            "openai-chatgpt",
        );
        match p.load(&scope).await.expect("load") {
            Credential::ChatGptOAuth { access_token, account_id, fedramp } => {
                assert_eq!(access_token, "plain-token");
                assert_eq!(account_id.as_deref(), Some("acc_2"));
                assert!(!fedramp); // non-JWT → defaults false
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn debug_redacts_token() {
        let p = ExternalTokensCredentialProvider::from_supplied("secret-tok", None);
        assert!(!format!("{p:?}").contains("secret-tok"));
    }
}
```

- [ ] **Step 2: Add `pub mod external_tokens;` + `pub use external_tokens::ExternalTokensCredentialProvider;` to `lib.rs`.**
- [ ] **Step 3: Run, expect PASS.** Run: `CARGO_PROFILE_DEV_DEBUG=0 cargo test -p openai-oauth external_tokens 2>&1 | tail -8`
- [ ] **Step 4: Commit**

```bash
git add openai-oauth/src/external_tokens.rs openai-oauth/src/lib.rs
git commit -m "feat(openai-oauth): ExternalTokensCredentialProvider"
```

---

## Task 4: rename availability flag to `openai_chatgpt_available`

**Files:** Modify `provider-config/src/availability.rs`.

- [ ] **Step 1: Rename the param + arm.** In `compute_availability`, rename the 5th param `openai_chatgpt_has_oauth: bool` → `openai_chatgpt_available: bool`, and the match arm `"openai-chatgpt" => openai_chatgpt_has_oauth` → `"openai-chatgpt" => openai_chatgpt_available`. Update the doc comment to say the engine ORs PAT-env / external-env / OAuth-session.

- [ ] **Step 2: Update the in-file tests** that call `compute_availability(...)` — the existing P2 test `openai_chatgpt_available_when_oauth_session_present` and the others still pass positional bools; no signature arity change (just a rename), so they compile unchanged. Confirm by running.

- [ ] **Step 3: Run, expect PASS.** Run: `CARGO_PROFILE_DEV_DEBUG=0 cargo test -p provider-config 2>&1 | grep -E "test result:|FAILED" | tail`
- [ ] **Step 4: Commit**

```bash
git add provider-config/src/availability.rs
git commit -m "refactor(provider-config): rename availability flag to openai_chatgpt_available"
```

---

## Task 5: engine precedence selector + availability OR-flag

**Files:** Modify `apps/engine-desktop/src/lib.rs`.

Context: today (lines ~1414–1516) the engine builds `openai_oauth_client`, detects `get_openai_oauth_tokens()` → `openai_oauth_state`, sets `has_openai_oauth = openai_oauth_state.is_some()`, and inserts an `OpenAiOAuthCredentialProvider` under `"openai-chatgpt"`. We add PAT/external precedence BEFORE the OAuth path and produce a single delegate + availability bool.

- [ ] **Step 1: Add the precedence selector.** Immediately AFTER `openai_oauth_client` is constructed (before the `match credentials.get_openai_oauth_tokens().await` block ~line 1419), add:

```rust
    // (3.2a-pre) P3 enterprise precedence for the openai-chatgpt credential:
    // PAT env  >  external-tokens env  >  OAuth login session.
    // Each yields an Option<Arc<dyn CredentialProvider>>; the first hit wins and
    // skips the OAuth detection below.
    let mut openai_chatgpt_delegate: Option<Arc<dyn llm_client::CredentialProvider>> = None;
    if let Ok(pat) = std::env::var("OPENAI_PERSONAL_ACCESS_TOKEN") {
        if !pat.trim().is_empty() {
            match openai_oauth::whoami(&openai_oauth_cfg, &http, &pat).await {
                Ok(md) => {
                    openai_chatgpt_delegate = Some(Arc::new(
                        openai_oauth::PatCredentialProvider::new(pat, md),
                    ) as Arc<dyn llm_client::CredentialProvider>);
                }
                Err(e) => tracing::warn!(error = %e, "OPENAI_PERSONAL_ACCESS_TOKEN whoami failed; ignoring PAT"),
            }
        }
    }
    if openai_chatgpt_delegate.is_none() {
        match (
            std::env::var("OPENAI_CHATGPT_ACCESS_TOKEN").ok().filter(|s| !s.trim().is_empty()),
            std::env::var("OPENAI_CHATGPT_ACCOUNT_ID").ok().filter(|s| !s.trim().is_empty()),
        ) {
            (Some(tok), Some(acc)) => {
                openai_chatgpt_delegate = Some(Arc::new(
                    openai_oauth::ExternalTokensCredentialProvider::from_supplied(tok, Some(acc)),
                ) as Arc<dyn llm_client::CredentialProvider>);
            }
            (Some(_), None) | (None, Some(_)) => tracing::warn!(
                "incomplete external ChatGPT tokens: set BOTH OPENAI_CHATGPT_ACCESS_TOKEN and OPENAI_CHATGPT_ACCOUNT_ID"
            ),
            (None, None) => {}
        }
    }
```

(Note: `openai_oauth_cfg` is consumed by `init_refresh_driver` in the OAuth block below — borrow it here BEFORE that move. `whoami` takes `&cfg`, so pass `&openai_oauth_cfg`. If the borrow checker complains because the later block moves `openai_oauth_cfg`, clone it for the OAuth block, or move the whoami call to use a clone — adapt minimally.)

- [ ] **Step 2: Gate the existing OAuth detection on no prior hit.** Wrap the existing `match credentials.get_openai_oauth_tokens().await { ... }` block so it only runs when `openai_chatgpt_delegate.is_none()`:

```rust
    if openai_chatgpt_delegate.is_none() {
        match credentials.get_openai_oauth_tokens().await {
            // … existing arms unchanged …
        }
    }
```

- [ ] **Step 3: Build the OAuth delegate into the same slot.** Replace the existing delegate-insertion block (the `let has_openai_oauth = openai_oauth_state.is_some(); if let Some(state) = openai_oauth_state { … insert "openai-chatgpt" … }` at ~1509–1516) with: first fold the OAuth state into `openai_chatgpt_delegate` if still empty, then compute availability + insert once:

```rust
    // OAuth login fills the slot only if PAT/external didn't.
    if openai_chatgpt_delegate.is_none() {
        if let Some(state) = openai_oauth_state {
            let driver = std::sync::Arc::new(openai_oauth::RefreshDriver::new(state));
            openai_chatgpt_delegate = Some(std::sync::Arc::new(
                openai_oauth::OpenAiOAuthCredentialProvider::new(driver),
            ) as Arc<dyn llm_client::CredentialProvider>);
        }
    }
    let has_openai_chatgpt = openai_chatgpt_delegate.is_some();
    if let Some(d) = openai_chatgpt_delegate {
        oauth_delegates.insert("openai-chatgpt".to_string(), d);
    }
```

(The `oauth_delegates` map + the anthropic insertion above it stay as-is. Remove the old `has_openai_oauth`/`openai_oauth_state` insertion lines this replaces.)

- [ ] **Step 4: Feed availability the OR-flag.** At the `compute_availability(...)` call (~line 2745), change the 5th argument from `has_openai_oauth` to `has_openai_chatgpt`.

- [ ] **Step 5: Build + test the engine.** Run:

```bash
CARGO_PROFILE_DEV_DEBUG=0 cargo build -p engine-desktop 2>&1 | tail -8
CARGO_PROFILE_DEV_DEBUG=0 cargo test -p engine-desktop 2>&1 | grep -E "test result:|FAILED|error\[|error:" | tail
```
Expected: compiles; tests green. (If `has_openai_oauth` is referenced elsewhere, replace those refs with `has_openai_chatgpt`.)

- [ ] **Step 6: Commit**

```bash
git add apps/engine-desktop/src/lib.rs
git commit -m "feat(engine-desktop): openai-chatgpt credential precedence (PAT > external > OAuth) + availability"
```

---

## Task 6: full verification

- [ ] **Step 1: Workspace build.** Run: `CARGO_PROFILE_DEV_DEBUG=0 cargo build --workspace 2>&1 | tail -3`. Expected: `Finished`.
- [ ] **Step 2: Affected-crate tests.** Run:

```bash
CARGO_PROFILE_DEV_DEBUG=0 cargo test -p openai-oauth -p provider-config -p engine-desktop 2>&1 | grep -E "test result:|FAILED|error\[|error:" | grep -v "0 passed; 0 failed"
```
Expected: only `test result: ok` lines; zero FAILED/error.

- [ ] **Step 3: Clippy the new code.** Run: `CARGO_PROFILE_DEV_DEBUG=0 cargo clippy -p openai-oauth 2>&1 | grep -E "^warning:|^error:" | grep -v "unused manifest key"`. Expected: empty (fix any new lints — add field docs / backticks as in P2's clippy-clean).

No commit (verification only). P3 complete.

---

## Notes for the implementer
- **Reuse, don't reinvent:** PAT + external both return `Credential::ChatGptOAuth` and ride the existing `ChatGptAuthenticator` — do NOT add a new AuthStrategy/Credential/authenticator.
- Confirm `testsupport` mock-HTTP helper names and `OAuthError` variant names against the real source before relying on them in tests (Task 2 note).
- Do NOT touch the OAuth-login path's behavior beyond gating it on `openai_chatgpt_delegate.is_none()` and folding its delegate into the shared slot.
- **Out of scope:** AgentIdentity, interactive `/connect` for PAT, PAT keychain storage.
- Every cargo command keeps `CARGO_PROFILE_DEV_DEBUG=0`.
