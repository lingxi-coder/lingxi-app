# llm-client provider catalog — Phase 2 (GitHub Copilot auth) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Dispatch a fresh implementer subagent per task with two-stage review (spec then quality). Per the project's concurrent-agent worktree hazard: run edit-agents **SEQUENTIALLY** in one checkout, never in parallel. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Add GitHub Copilot as a working `llm-client` provider — the interactive device-flow login mechanism (behind a mockable HTTP seam) plus a synchronous request authenticator that injects the Copilot header set — and register the `github-copilot` preset in the catalog.

**Architecture:** Verified against opencode's actual `copilot.ts`: the GitHub OAuth-App token (`CLIENT_ID = "Ov23li8tweQw6odWQebz"`) is used **directly** as `Authorization: Bearer` against `api.githubcopilot.com` — **no `copilot_internal/v2/token` exchange, no caching** (`expires: 0`). So request-time auth is pure synchronous header injection (fits the existing `Authenticator` trait), and the only network call is the one-time device-flow login. Copilot reuses the existing `OpenAiChatCodec` (`ProtocolFamily::OpenAiChat`); a new additive `AuthStrategy::CopilotBearer` variant selects the new `CopilotAuthenticator` in `client.rs::authenticate`. The device flow goes through a small injected `CopilotHttp` async seam (the crate has no HTTP-client dependency — transport is always injected, same as `Transport`), so the begin/poll state machine is fully offline-testable.

**Tech Stack:** Rust, `serde`/`serde_json` (existing deps), `include_str!`, the crate's `BoxFuture` async-seam pattern (no `async_trait`). Reuses existing `Authenticator`, `ProviderRequest`, `CredentialConfig`, `ModelRegistry`, and the Phase 1 `catalog` module.

**Spec:** `docs/superpowers/specs/2026-06-13-llm-client-provider-catalog-design.md` (Phase 2 section, corrected commit `7ad98938`).

**Base branch:** `parity-llm-client-3a` (Phase 1 already merged at `05930126`; spec correction at `7ad98938`).

---

## Conventions (apply to every task)

- Cargo root `lingxi-code/`; run cargo from there, git from the worktree repo root with explicit paths.
- **NEVER `git add -A`** — untracked `codex/`, `liter-llm/`, `opencode/`, `.codegraph/` at repo root. Stage only named paths.
- Lints: `cd lingxi-code && cargo clippy -p llm-client --all-targets --no-deps -- -D warnings`; crate builds under `-D missing-docs` — every new `pub` item needs a `///` doc.
- TDD: write the failing test, OBSERVE RED, then implement.
- Commit trailer EXACTLY (own line, blank line before):
  ```
  Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
  ```
  Commit with `git commit -F <tempfile>`.
- Frozen crates: do **not** touch `lingxi-code/platform-api` or `lingxi-code/protocol`. This plan touches only `lingxi-code/llm-client`. (`AuthStrategy` lives in `llm-client/src/config.rs` — adding a variant there is fine; it is NOT a frozen crate.)
- `engine-mobile` + `orchestrator` must keep building (Task 6 verifies — adding an `AuthStrategy` variant can break exhaustive matches in consumers).

## File structure (Phase 2)

- Create `lingxi-code/llm-client/src/copilot/mod.rs` — module root + re-exports.
- Create `lingxi-code/llm-client/src/copilot/auth.rs` — `CopilotAuthenticator`, `CopilotSecret`, consts.
- Create `lingxi-code/llm-client/src/copilot/login.rs` — `CopilotHttp` seam, `CopilotLogin`, `DeviceCodeResponse`, `PollOutcome`.
- Modify `lingxi-code/llm-client/src/lib.rs` — `pub mod copilot;` + re-exports.
- Modify `lingxi-code/llm-client/src/config.rs` — add `AuthStrategy::CopilotBearer`.
- Modify `lingxi-code/llm-client/src/client.rs` — wire `CopilotBearer` → `CopilotAuthenticator`.
- Modify `lingxi-code/llm-client/scripts/refresh-models-dev.sh` — add `github-copilot` to the provider list.
- Create `lingxi-code/llm-client/data/models-dev/github-copilot.json` — vendored slice.
- Modify `lingxi-code/llm-client/src/catalog/presets.rs` — add the `github-copilot` preset + update count test.
- Modify `lingxi-code/llm-client/tests/catalog_test.rs` — 3→4 providers + copilot routing assertion.
- Create `lingxi-code/llm-client/tests/copilot_auth_test.rs` — client-level prepare() header test.

## Out of scope for Phase 2 (recorded so it is not lost)

- **Copilot token exchange / caching** — does not exist in opencode; the GitHub token is used directly. Not built.
- **`Copilot-Vision-Request` header** — only needed for image requests; inspecting encoded provider JSON for images is deferred.
- **Per-request `x-initiator` user/agent distinction** — set to the static `"agent"` default (LingXi is an agentic client). Refinement deferred.
- **GitHub Enterprise** (`copilot-api.<domain>`) — v1 targets `github.com`/`api.githubcopilot.com` only.
- **The host's `CopilotHttp` implementation + the interactive sleep/UI loop** — that is host/desktop wiring, not llm-client; this plan delivers the mechanism + mock-tested logic.

---

### Task 0: worktree + baseline

**Files:** none (setup).

- [ ] **Step 1:** Create an isolated worktree via the `superpowers:using-git-worktrees` skill, branched off `parity-llm-client-3a` (current HEAD `7ad98938`). Suggested name `provider-catalog-p2`. Do NOT work in the primary checkout (it has standing uncommitted work).

- [ ] **Step 2:** Fresh-worktree build:

Run: `cd lingxi-code && cargo build -p llm-client`
Expected: builds clean.

- [ ] **Step 3:** Baseline test count:

Run: `cd lingxi-code && cargo test -p llm-client 2>&1 | grep -E 'test result:' | awk '{s+=$4} END{print "baseline passed:", s}'`
Expected: `143`. No commit.

---

### Task 1: vendor the github-copilot slice

**Files:**
- Modify: `lingxi-code/llm-client/scripts/refresh-models-dev.sh`
- Create: `lingxi-code/llm-client/data/models-dev/github-copilot.json`

- [ ] **Step 1:** In `refresh-models-dev.sh`, add `github-copilot` to the provider loop. Change the line:

```bash
for provider in openrouter deepseek zhipuai-coding-plan; do
```
to:
```bash
for provider in openrouter deepseek zhipuai-coding-plan github-copilot; do
```

- [ ] **Step 2:** Run the script (network; produces deterministic committed JSON):

Run: `lingxi-code/llm-client/scripts/refresh-models-dev.sh`
Expected: prints all four `wrote …` lines incl. `github-copilot.json: 23 models` (count may drift slightly upstream — record what you see), then `done`. The other three files should be unchanged or re-written identically.

- [ ] **Step 3:** Verify it parses:

Run: `python3 -c "import json; d=json.load(open('lingxi-code/llm-client/data/models-dev/github-copilot.json')); print('name=', d['name'], 'api=', d.get('api'), 'models=', len(d['models']))"`
Expected: `name= GitHub Copilot api= https://api.githubcopilot.com models= 23` (count may drift).

- [ ] **Step 4:** Commit ONLY these two paths. (If the script rewrote the existing three slices with no content change, `git add` only the two named paths; if it changed them, inspect — upstream drift is acceptable but mention it.)

```bash
git add lingxi-code/llm-client/scripts/refresh-models-dev.sh \
        lingxi-code/llm-client/data/models-dev/github-copilot.json
git commit -F - <<'EOF'
feat(llm-client): vendor models.dev github-copilot slice

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
```

---

### Task 2: CopilotAuthenticator + redacting secret

**Files:**
- Create: `lingxi-code/llm-client/src/copilot/auth.rs`
- Create: `lingxi-code/llm-client/src/copilot/mod.rs`
- Modify: `lingxi-code/llm-client/src/lib.rs`

- [ ] **Step 1:** Create `lingxi-code/llm-client/src/copilot/auth.rs` with EXACTLY this content:

```rust
//! Synchronous GitHub Copilot request authenticator + redacting token wrapper.
//!
//! opencode parity: the GitHub OAuth-App token is used DIRECTLY as the bearer
//! credential (no `copilot_internal/v2/token` exchange), so authentication is
//! pure header injection and fits the synchronous [`Authenticator`] trait.

use crate::{Authenticator, LlmError, ProviderRequest};

/// `X-GitHub-Api-Version` header value sent to GitHub Copilot.
pub const COPILOT_API_VERSION: &str = "2026-06-01";
/// `User-Agent` sent to GitHub Copilot.
pub const COPILOT_USER_AGENT: &str = "LingXi-Code";

/// GitHub OAuth token used directly as the Copilot bearer credential.
///
/// The `Debug` impl is redacting so the token never reaches logs or errors.
#[derive(Clone)]
pub struct CopilotSecret(String);

impl CopilotSecret {
    /// Wrap a GitHub OAuth token.
    #[must_use]
    pub fn new(token: impl Into<String>) -> Self {
        Self(token.into())
    }
}

impl std::fmt::Debug for CopilotSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CopilotSecret(<redacted>)")
    }
}

/// Authenticator for GitHub Copilot's OpenAI-compatible endpoint. Injects the
/// Copilot header set and uses the GitHub OAuth token directly as the bearer.
#[derive(Debug, Clone)]
pub struct CopilotAuthenticator {
    secret: CopilotSecret,
}

impl CopilotAuthenticator {
    /// Create an authenticator from a GitHub OAuth token.
    #[must_use]
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            secret: CopilotSecret::new(token),
        }
    }
}

impl Authenticator for CopilotAuthenticator {
    fn apply(&self, mut request: ProviderRequest) -> Result<ProviderRequest, LlmError> {
        let headers = &mut request.headers;
        // Defensive parity with opencode: never let an x-api-key ride along.
        headers.remove("x-api-key");
        headers.insert(
            "Authorization".to_string(),
            format!("Bearer {}", self.secret.0),
        );
        headers.insert("User-Agent".to_string(), COPILOT_USER_AGENT.to_string());
        headers.insert("Openai-Intent".to_string(), "conversation-edits".to_string());
        headers.insert(
            "X-GitHub-Api-Version".to_string(),
            COPILOT_API_VERSION.to_string(),
        );
        // LingXi is an agentic client; per-request user/agent refinement is deferred.
        headers.insert("x-initiator".to_string(), "agent".to_string());
        Ok(request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn injects_copilot_headers_and_strips_x_api_key() {
        let mut request = ProviderRequest::post_json(
            "https://api.githubcopilot.com/chat/completions",
            json!({ "model": "gpt-5.4-nano" }),
        );
        request
            .headers
            .insert("x-api-key".to_string(), "leftover".to_string());

        let signed = CopilotAuthenticator::new("ght_token")
            .apply(request)
            .expect("applies");

        assert_eq!(
            signed.headers.get("Authorization"),
            Some(&"Bearer ght_token".to_string())
        );
        assert_eq!(
            signed.headers.get("X-GitHub-Api-Version"),
            Some(&"2026-06-01".to_string())
        );
        assert_eq!(
            signed.headers.get("Openai-Intent"),
            Some(&"conversation-edits".to_string())
        );
        assert_eq!(
            signed.headers.get("User-Agent"),
            Some(&"LingXi-Code".to_string())
        );
        assert_eq!(signed.headers.get("x-initiator"), Some(&"agent".to_string()));
        assert!(!signed.headers.contains_key("x-api-key"));
    }

    #[test]
    fn debug_does_not_leak_token() {
        let dbg_secret = format!("{:?}", CopilotSecret::new("supersecret"));
        assert!(!dbg_secret.contains("supersecret"));
        let dbg_auth = format!("{:?}", CopilotAuthenticator::new("supersecret"));
        assert!(!dbg_auth.contains("supersecret"));
    }
}
```

- [ ] **Step 2:** Create `lingxi-code/llm-client/src/copilot/mod.rs` (only `auth` exists this task):

```rust
//! GitHub Copilot provider support: device-flow login + request authenticator.

pub mod auth;

pub use auth::{CopilotAuthenticator, CopilotSecret, COPILOT_API_VERSION, COPILOT_USER_AGENT};
```

- [ ] **Step 3:** In `lingxi-code/llm-client/src/lib.rs`, add `pub mod copilot;` next to the other `pub mod` declarations (e.g. after `pub mod config;`), and add a re-export line near the other `pub use`:

```rust
pub use copilot::{CopilotAuthenticator, CopilotSecret};
```

- [ ] **Step 4:** OBSERVE RED. Temporarily change the `injects_copilot_headers_and_strips_x_api_key` assertion `assert!(!signed.headers.contains_key("x-api-key"));` to `assert!(signed.headers.contains_key("x-api-key"));` and run:

Run: `cd lingxi-code && cargo test -p llm-client -- copilot::auth`
Expected: FAIL. Then restore the `!`.

- [ ] **Step 5:** Run for real:

Run: `cd lingxi-code && cargo test -p llm-client -- copilot::auth`
Expected: both tests PASS.

- [ ] **Step 6:** Clippy:

Run: `cd lingxi-code && cargo clippy -p llm-client --all-targets --no-deps -- -D warnings`
Expected: clean.

- [ ] **Step 7:** Commit:

```bash
git add lingxi-code/llm-client/src/copilot/auth.rs \
        lingxi-code/llm-client/src/copilot/mod.rs \
        lingxi-code/llm-client/src/lib.rs
git commit -F - <<'EOF'
feat(llm-client): CopilotAuthenticator (direct-bearer + Copilot headers)

Synchronous header injection (no token exchange, opencode parity); redacting
CopilotSecret keeps the GitHub token out of Debug/logs.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
```

---

### Task 3: device-flow login (CopilotHttp seam + CopilotLogin)

**Files:**
- Create: `lingxi-code/llm-client/src/copilot/login.rs`
- Modify: `lingxi-code/llm-client/src/copilot/mod.rs`
- Modify: `lingxi-code/llm-client/src/lib.rs`

- [ ] **Step 1:** Create `lingxi-code/llm-client/src/copilot/login.rs` with EXACTLY this content:

```rust
//! GitHub Copilot device-flow login (RFC 8628) over an injected JSON-POST seam.
//!
//! The crate owns no HTTP client (transport is always injected). [`CopilotLogin`]
//! holds the pure begin/poll state machine + backoff math; the host implements
//! [`CopilotHttp`] and drives the sleep/retry loop and any UI.

use serde_json::{json, Value};

use crate::copilot::auth::CopilotSecret;
use crate::transport::BoxFuture;
use crate::LlmError;

/// GitHub OAuth App client id (opencode's public Copilot app).
pub const COPILOT_CLIENT_ID: &str = "Ov23li8tweQw6odWQebz";

const DEVICE_CODE_URL: &str = "https://github.com/login/device/code";
const ACCESS_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";

/// Minimal JSON-POST seam for the two device-flow calls.
///
/// Host implementations MUST send `Accept: application/json` (GitHub otherwise
/// form-encodes the response) and a `User-Agent`. Transport/TLS/timeout failures
/// map to [`LlmError::Transport`].
pub trait CopilotHttp: Send + Sync {
    /// POST `url` with a JSON body; return the parsed JSON response.
    fn post_json<'a>(
        &'a self,
        url: &'a str,
        body: &'a Value,
    ) -> BoxFuture<'a, Result<Value, LlmError>>;
}

/// Device-code grant returned by [`CopilotLogin::begin`].
#[derive(Debug, Clone)]
pub struct DeviceCodeResponse {
    /// Code the user types at `verification_uri`.
    pub user_code: String,
    /// URL the user opens to authorize.
    pub verification_uri: String,
    /// Opaque device code used when polling.
    pub device_code: String,
    /// Server-recommended polling interval (seconds).
    pub interval_secs: u64,
}

/// Classified result of one [`CopilotLogin::poll_once`].
#[derive(Debug)]
pub enum PollOutcome {
    /// Authorization complete; carries the GitHub OAuth token.
    Success(CopilotSecret),
    /// Not authorized yet; sleep `interval_secs` and poll again.
    Pending {
        /// Seconds to wait before the next poll.
        interval_secs: u64,
    },
    /// Server asked us to slow down; sleep `interval_secs` and poll again.
    SlowDown {
        /// Backed-off seconds to wait (RFC 8628 §3.5).
        interval_secs: u64,
    },
    /// Terminal failure (e.g. `access_denied`, `expired_token`).
    Failed {
        /// Server-reported error code.
        error: String,
    },
}

/// Device-flow login driver. The host owns the sleep+retry loop.
pub struct CopilotLogin<H: CopilotHttp> {
    http: H,
    client_id: String,
}

impl<H: CopilotHttp> CopilotLogin<H> {
    /// Create a login driver over the given HTTP seam.
    #[must_use]
    pub fn new(http: H) -> Self {
        Self {
            http,
            client_id: COPILOT_CLIENT_ID.to_string(),
        }
    }

    /// Step 1 — request a device code. The caller displays `user_code` +
    /// `verification_uri`, then polls.
    pub async fn begin(&self) -> Result<DeviceCodeResponse, LlmError> {
        let body = json!({ "client_id": self.client_id, "scope": "read:user" });
        let v = self.http.post_json(DEVICE_CODE_URL, &body).await?;
        Ok(DeviceCodeResponse {
            user_code: str_field(&v, "user_code")?,
            verification_uri: str_field(&v, "verification_uri")?,
            device_code: str_field(&v, "device_code")?,
            interval_secs: v.get("interval").and_then(Value::as_u64).unwrap_or(5),
        })
    }

    /// Step 2 — poll once for the token. Returns a classified [`PollOutcome`]
    /// with the recommended next interval; the caller sleeps and re-polls on
    /// `Pending`/`SlowDown`.
    pub async fn poll_once(&self, dc: &DeviceCodeResponse) -> Result<PollOutcome, LlmError> {
        let body = json!({
            "client_id": self.client_id,
            "device_code": dc.device_code,
            "grant_type": "urn:ietf:params:oauth:grant-type:device_code",
        });
        let v = self.http.post_json(ACCESS_TOKEN_URL, &body).await?;

        if let Some(token) = v.get("access_token").and_then(Value::as_str) {
            return Ok(PollOutcome::Success(CopilotSecret::new(token)));
        }
        match v.get("error").and_then(Value::as_str) {
            Some("authorization_pending") => Ok(PollOutcome::Pending {
                interval_secs: dc.interval_secs,
            }),
            Some("slow_down") => {
                // RFC 8628 §3.5: add 5s, or use the server-provided interval.
                let server = v.get("interval").and_then(Value::as_u64);
                Ok(PollOutcome::SlowDown {
                    interval_secs: server.unwrap_or(dc.interval_secs + 5),
                })
            }
            Some(other) => Ok(PollOutcome::Failed {
                error: other.to_string(),
            }),
            None => Ok(PollOutcome::Failed {
                error: "no access_token and no error in response".to_string(),
            }),
        }
    }
}

fn str_field(v: &Value, key: &str) -> Result<String, LlmError> {
    v.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| LlmError::InvalidRequest {
            message: format!("copilot device-flow response missing '{key}'"),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockHttp(Value);
    impl CopilotHttp for MockHttp {
        fn post_json<'a>(
            &'a self,
            _url: &'a str,
            _body: &'a Value,
        ) -> BoxFuture<'a, Result<Value, LlmError>> {
            let v = self.0.clone();
            Box::pin(async move { Ok(v) })
        }
    }

    fn dc() -> DeviceCodeResponse {
        DeviceCodeResponse {
            user_code: "WDJB-MJHT".to_string(),
            verification_uri: "https://github.com/login/device".to_string(),
            device_code: "dev-code".to_string(),
            interval_secs: 5,
        }
    }

    #[tokio::test]
    async fn begin_parses_device_code() {
        let login = CopilotLogin::new(MockHttp(json!({
            "user_code": "WDJB-MJHT",
            "verification_uri": "https://github.com/login/device",
            "device_code": "dev-code",
            "interval": 7
        })));
        let parsed = login.begin().await.expect("begin ok");
        assert_eq!(parsed.user_code, "WDJB-MJHT");
        assert_eq!(parsed.device_code, "dev-code");
        assert_eq!(parsed.interval_secs, 7);
    }

    #[tokio::test]
    async fn poll_success_yields_token() {
        let login = CopilotLogin::new(MockHttp(json!({ "access_token": "ght_abc" })));
        match login.poll_once(&dc()).await.expect("poll ok") {
            PollOutcome::Success(secret) => {
                // Token reaches the authenticator as a bearer, but never via Debug.
                assert!(!format!("{secret:?}").contains("ght_abc"));
            }
            other => panic!("expected Success, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn poll_pending_returns_device_interval() {
        let login = CopilotLogin::new(MockHttp(json!({ "error": "authorization_pending" })));
        match login.poll_once(&dc()).await.unwrap() {
            PollOutcome::Pending { interval_secs } => assert_eq!(interval_secs, 5),
            other => panic!("expected Pending, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn poll_slow_down_without_server_interval_adds_five() {
        let login = CopilotLogin::new(MockHttp(json!({ "error": "slow_down" })));
        match login.poll_once(&dc()).await.unwrap() {
            PollOutcome::SlowDown { interval_secs } => assert_eq!(interval_secs, 10),
            other => panic!("expected SlowDown, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn poll_slow_down_uses_server_interval_when_present() {
        let login = CopilotLogin::new(MockHttp(json!({ "error": "slow_down", "interval": 42 })));
        match login.poll_once(&dc()).await.unwrap() {
            PollOutcome::SlowDown { interval_secs } => assert_eq!(interval_secs, 42),
            other => panic!("expected SlowDown, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn poll_other_error_is_terminal() {
        let login = CopilotLogin::new(MockHttp(json!({ "error": "access_denied" })));
        match login.poll_once(&dc()).await.unwrap() {
            PollOutcome::Failed { error } => assert_eq!(error, "access_denied"),
            other => panic!("expected Failed, got {other:?}"),
        }
    }
}
```

- [ ] **Step 2:** Update `lingxi-code/llm-client/src/copilot/mod.rs` to:

```rust
//! GitHub Copilot provider support: device-flow login + request authenticator.

pub mod auth;
pub mod login;

pub use auth::{CopilotAuthenticator, CopilotSecret, COPILOT_API_VERSION, COPILOT_USER_AGENT};
pub use login::{CopilotHttp, CopilotLogin, DeviceCodeResponse, PollOutcome, COPILOT_CLIENT_ID};
```

- [ ] **Step 3:** In `lib.rs`, extend the copilot re-export to:

```rust
pub use copilot::{
    CopilotAuthenticator, CopilotHttp, CopilotLogin, CopilotSecret, DeviceCodeResponse, PollOutcome,
};
```

- [ ] **Step 4:** OBSERVE RED. Temporarily change `poll_slow_down_without_server_interval_adds_five`'s `assert_eq!(interval_secs, 10)` to `assert_eq!(interval_secs, 5)` and run:

Run: `cd lingxi-code && cargo test -p llm-client -- copilot::login`
Expected: FAIL (`10 != 5`). Then restore `10`.

- [ ] **Step 5:** Run for real:

Run: `cd lingxi-code && cargo test -p llm-client -- copilot::login`
Expected: all 6 tests PASS.

- [ ] **Step 6:** Clippy:

Run: `cd lingxi-code && cargo clippy -p llm-client --all-targets --no-deps -- -D warnings`
Expected: clean.

- [ ] **Step 7:** Commit:

```bash
git add lingxi-code/llm-client/src/copilot/login.rs \
        lingxi-code/llm-client/src/copilot/mod.rs \
        lingxi-code/llm-client/src/lib.rs
git commit -F - <<'EOF'
feat(llm-client): Copilot device-flow login over injected CopilotHttp seam

Pure begin/poll_once state machine + RFC 8628 slow_down backoff; host owns the
sleep/retry loop. Fully mock-tested (pending/slow_down/success/failed).

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
```

---

### Task 4: AuthStrategy::CopilotBearer + client wiring

**Files:**
- Modify: `lingxi-code/llm-client/src/config.rs`
- Modify: `lingxi-code/llm-client/src/client.rs`
- Create: `lingxi-code/llm-client/tests/copilot_auth_test.rs`

- [ ] **Step 1:** Write the failing client-level test. Create `lingxi-code/llm-client/tests/copilot_auth_test.rs`:

```rust
//! The client auth path selects CopilotAuthenticator for AuthStrategy::CopilotBearer.

use std::sync::Arc;

use llm_client::client::DefaultLlmClient;
use llm_client::{
    AuthStrategy, Capabilities, ClientConfig, Credential, CredentialConfig, LlmRequest,
    ModelProfile, PricingConfig, ProtocolFamily, ProviderId, ProviderProfile,
    StaticCredentialProvider,
};

fn copilot_config() -> ClientConfig {
    ClientConfig {
        providers: vec![ProviderProfile {
            provider_id: ProviderId::OpenAICompatible { name: "github-copilot".to_string() },
            profile_name: "github-copilot".to_string(),
            base_url: "https://api.githubcopilot.com".to_string(),
            protocol: ProtocolFamily::OpenAiChat,
            auth: AuthStrategy::CopilotBearer,
            credential: CredentialConfig::Static { id: "gh".to_string() },
            models: vec![ModelProfile {
                display_model: "GPT-5.4 nano".to_string(),
                request_model: "gpt-5.4-nano".to_string(),
                billing_model: "gpt-5.4-nano".to_string(),
                aliases: vec![],
                capabilities: Capabilities { streaming: true, tools: true, ..Default::default() },
            }],
            pricing: PricingConfig::default(),
        }],
    }
}

#[tokio::test]
async fn copilot_bearer_injects_copilot_headers_through_prepare() {
    let client = DefaultLlmClient::from_config(copilot_config())
        .unwrap()
        .with_credential_provider(Arc::new(StaticCredentialProvider::new(
            Credential::BearerToken("ght_live".to_string()),
        )));

    let prepared = client.prepare(&LlmRequest::new("gpt-5.4-nano")).await.unwrap();
    let h = &prepared.provider_request.headers;

    assert_eq!(h.get("Authorization"), Some(&"Bearer ght_live".to_string()));
    assert_eq!(h.get("X-GitHub-Api-Version"), Some(&"2026-06-01".to_string()));
    assert_eq!(h.get("Openai-Intent"), Some(&"conversation-edits".to_string()));
    assert_eq!(h.get("x-initiator"), Some(&"agent".to_string()));
}
```

- [ ] **Step 2:** Run it; confirm it FAILS to compile (no `AuthStrategy::CopilotBearer`):

Run: `cd lingxi-code && cargo test -p llm-client --test copilot_auth_test`
Expected: FAIL — `no variant ... CopilotBearer`. Quote it.

- [ ] **Step 3:** In `lingxi-code/llm-client/src/config.rs`, add the variant to the `AuthStrategy` enum (after `OAuthBearer`):

```rust
    /// GitHub Copilot: GitHub OAuth token used directly as the bearer, plus the
    /// Copilot header set (see [`crate::CopilotAuthenticator`]).
    CopilotBearer,
```

- [ ] **Step 4:** In `lingxi-code/llm-client/src/client.rs`:
  (a) add `CopilotAuthenticator` to the existing `use crate::{...}` import block at the top (alongside `ApiKeyAuthenticator`, `BearerAuthenticator`).
  (b) In `authenticate`, add `AuthStrategy::CopilotBearer` to the secret-loading arm and an inner match arm. Change the outer arm header from:

```rust
            AuthStrategy::ApiKey | AuthStrategy::Bearer | AuthStrategy::OAuthBearer => {
```
to:
```rust
            AuthStrategy::ApiKey
            | AuthStrategy::Bearer
            | AuthStrategy::OAuthBearer
            | AuthStrategy::CopilotBearer => {
```
  and inside that arm's inner `match (&entry.auth, &entry.protocol) { ... }`, add as the FIRST arm:

```rust
                    (AuthStrategy::CopilotBearer, _) => {
                        Box::new(CopilotAuthenticator::new(secret))
                    }
```

- [ ] **Step 5:** Run the test; expect PASS:

Run: `cd lingxi-code && cargo test -p llm-client --test copilot_auth_test`
Expected: PASS.

- [ ] **Step 6:** Full crate test + clippy (an added enum variant can surface non-exhaustive-match warnings elsewhere in the crate):

Run: `cd lingxi-code && cargo test -p llm-client 2>&1 | grep -E 'test result:' | awk '{s+=$4; f+=$6} END{print "passed:", s, "failed:", f}'`
Expected: failed: 0.
Run: `cd lingxi-code && cargo clippy -p llm-client --all-targets --no-deps -- -D warnings`
Expected: clean. (If any in-crate `match` on `AuthStrategy` is now non-exhaustive, handle `CopilotBearer` explicitly there — do NOT add a catch-all `_` arm that would hide future variants.)

- [ ] **Step 7:** Commit:

```bash
git add lingxi-code/llm-client/src/config.rs \
        lingxi-code/llm-client/src/client.rs \
        lingxi-code/llm-client/tests/copilot_auth_test.rs
git commit -F - <<'EOF'
feat(llm-client): AuthStrategy::CopilotBearer wired to CopilotAuthenticator

prepare() now injects the Copilot header set for CopilotBearer routes.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
```

---

### Task 5: github-copilot preset in the catalog

**Files:**
- Modify: `lingxi-code/llm-client/src/catalog/presets.rs`
- Modify: `lingxi-code/llm-client/tests/catalog_test.rs`

- [ ] **Step 1:** Update the integration test for 4 providers. In `lingxi-code/llm-client/tests/catalog_test.rs`:
  (a) change `presets_cover_three_providers_with_models` to assert `4` and rename it to `presets_cover_four_providers_with_models`:

```rust
#[test]
fn presets_cover_four_providers_with_models() {
    let catalog = builtin_presets();
    // openrouter + deepseek + glm-coding + github-copilot.
    assert_eq!(catalog.providers.len(), 4);
    for p in &catalog.providers {
        assert!(!p.models.is_empty(), "{} has models", p.profile_name);
    }
}
```
  (b) append a copilot routing test:

```rust
#[test]
fn copilot_routes_to_githubcopilot_with_copilot_bearer() {
    use llm_client::AuthStrategy;
    let catalog = builtin_presets();
    let cp = catalog
        .providers
        .iter()
        .find(|p| p.profile_name == "github-copilot")
        .expect("github-copilot preset present");
    assert_eq!(cp.protocol, ProtocolFamily::OpenAiChat);
    assert_eq!(cp.base_url, "https://api.githubcopilot.com");
    assert_eq!(cp.auth, AuthStrategy::CopilotBearer);
}
```

- [ ] **Step 2:** Run; confirm FAIL (still 3 providers, no copilot):

Run: `cd lingxi-code && cargo test -p llm-client --test catalog_test`
Expected: FAIL (`3 != 4`, and the copilot find panics). Quote it.

- [ ] **Step 3:** In `lingxi-code/llm-client/src/catalog/presets.rs`:
  (a) add the embedded slice constant beside the others:

```rust
const GITHUB_COPILOT: &str = include_str!("../../data/models-dev/github-copilot.json");
```
  (b) add a fourth `Preset` to the `vec![...]` in `presets()` (after the glm-coding entry):

```rust
        // GitHub Copilot: OpenAI-compatible wire; GitHub OAuth token used
        // directly as the bearer via AuthStrategy::CopilotBearer (no exchange).
        Preset {
            profile_name: "github-copilot",
            base_url: "https://api.githubcopilot.com",
            protocol: ProtocolFamily::OpenAiChat,
            auth: AuthStrategy::CopilotBearer,
            provider_id: ProviderId::OpenAICompatible { name: "github-copilot".to_string() },
            credential_env: "GITHUB_TOKEN",
            slice_json: GITHUB_COPILOT,
        },
```
  (c) update the in-module unit test `every_preset_yields_expected_model_counts` to expect 4 providers and add the copilot count assertion (use the count printed by Task 1, e.g. 23):

```rust
        assert_eq!(catalog.providers.len(), 4);
        // Exact counts guard against a truncated/partial re-vendor of a slice.
        assert_eq!(count("openrouter"), 337);
        assert_eq!(count("deepseek"), 4);
        assert_eq!(count("glm-coding"), 6);
        assert_eq!(count("github-copilot"), 23);
```
  (If Task 1 reported a copilot count other than 23, use that number here.)

- [ ] **Step 4:** Run both test targets; expect PASS:

Run: `cd lingxi-code && cargo test -p llm-client --test catalog_test`
Expected: PASS (all, incl. the two updated/new).
Run: `cd lingxi-code && cargo test -p llm-client -- catalog::presets`
Expected: PASS.

- [ ] **Step 5:** Clippy:

Run: `cd lingxi-code && cargo clippy -p llm-client --all-targets --no-deps -- -D warnings`
Expected: clean.

- [ ] **Step 6:** Commit:

```bash
git add lingxi-code/llm-client/src/catalog/presets.rs \
        lingxi-code/llm-client/tests/catalog_test.rs
git commit -F - <<'EOF'
feat(llm-client): register github-copilot preset in the catalog

OpenAiChat + CopilotBearer + Env{GITHUB_TOKEN}; builtin_presets() now covers
four providers and resolves Copilot models via ModelRegistry.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
```

---

### Task 6: full-crate verification + frozen-crate + consumers

**Files:** none (verification).

- [ ] **Step 1:** Full crate test:

Run: `cd lingxi-code && cargo test -p llm-client 2>&1 | grep -E 'test result:' | awk '{s+=$4; f+=$6} END{print "passed:", s, "failed:", f}'`
Expected: `failed: 0`; passed ≥ 143 + new tests (8 unit + 1 client + adjustments). Record the number.

- [ ] **Step 2:** Consumers build (an added `AuthStrategy` variant can break exhaustive matches downstream):

Run: `cd lingxi-code && cargo build -p llm-client -p orchestrator -p engine-mobile`
Expected: builds clean. If a consumer match on `AuthStrategy` is now non-exhaustive, that is in-scope to fix minimally (handle `CopilotBearer`); report any consumer edits.

- [ ] **Step 3:** Frozen-crate guard (delta vs the branch point must be 0):

Run: `git diff parity-llm-client-3a -- lingxi-code/traits lingxi-code/protocol | grep -c '^[-+]' || true`
Expected: `0`.

- [ ] **Step 4:** No stray untracked:

Run: `git status --short | grep -E '^\?\?' || echo "(clean)"`
Expected: `(clean)`.

- [ ] **Step 5:** No commit. Phase 2 complete — ready for final review + finishing-a-development-branch.

---

## Self-review (completed by plan author)

**Spec coverage (Phase 2, corrected):**
- Device-flow login (begin + classified poll + backoff) over a mockable seam → Task 3. ✓
- Synchronous direct-bearer authenticator + exact Copilot header set + x-api-key strip → Task 2. ✓
- New `AuthStrategy::CopilotBearer` wired in `client.rs` → Task 4. ✓
- github-copilot vendored slice + preset (OpenAiChat, base, CopilotBearer, GITHUB_TOKEN) → Tasks 1, 5. ✓
- Secret hygiene (redacting Debug) → Task 2 (`CopilotSecret`) + Task 3 (Success carries `CopilotSecret`). ✓
- Tests: device-flow classify+backoff, header injection+strip+redaction, client wiring, model resolve+routing → Tasks 2/3/4/5. ✓
- Deferred items (no exchange/cache, no vision header, no enterprise, static x-initiator, host loop) → recorded in "Out of scope". ✓

**Placeholder scan:** every code step shows complete code; the only data-dependent value is the copilot model count (Task 1 prints it; Task 5 uses it, default 23). No TBD/TODO. ✓

**Type consistency:** `CopilotAuthenticator`, `CopilotSecret`, `CopilotHttp`, `CopilotLogin`, `DeviceCodeResponse`, `PollOutcome`, `COPILOT_API_VERSION`/`COPILOT_USER_AGENT`/`COPILOT_CLIENT_ID`, `AuthStrategy::CopilotBearer`, `ProviderRequest::post_json`, `Credential::BearerToken`, `StaticCredentialProvider`, `BoxFuture` all match the crate definitions read from `auth.rs`/`transport.rs`/`protocol.rs`/`config.rs`/`client.rs`/`tests/*`. The `presets.rs` `Preset` shape matches Phase 1. ✓
