# LLM Providers v2 — P1: Auth-Seam Refactor — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Introduce an async, request-aware `Authenticator` seam (the foundation for SigV4 + cloud token minting) and route all auth through it, removing the `auth` parameter from the pure `WireCodec::encode_request`.

**Architecture:** `WireCodec::encode_request(req)` becomes auth-agnostic (still pure/sync). A new `#[async_trait] Authenticator` trait mutates the fully-built `HttpRequest` just before transport; `GenericClient` holds `Arc<dyn Authenticator>` and calls `authorize()` after encode. `StaticAuth` wraps the v1 `Auth` enum (None/Bearer/Header). The OpenAI/Gemini codecs stop attaching headers; the registry wraps each profile's `Auth` in `StaticAuth`. The Anthropic provider does not use `WireCodec` and is untouched.

**Tech Stack:** Rust 1.82.0, `async-trait`, `futures`, `tokio` (all already deps of `providers`). Run all cargo from `lingxi-code/`.

**Spec:** `docs/superpowers/specs/2026-06-01-llm-providers-v2-design.md` §2.1. Branch `llm-providers-v2`.

**Parity gate:** OpenAI/Gemini request *bytes* are unchanged (same headers, attached one stage later); the entire `test-harness` parity suite must stay green. Do NOT modify the frozen `traits/` crate.

---

## File Structure

| File | Responsibility | Change |
|---|---|---|
| `lingxi-code/providers/src/authenticator.rs` | The `Authenticator` trait + `StaticAuth` impl | **Create** |
| `lingxi-code/providers/src/lib.rs` | Module decls + reexports | Add `authenticator` module + reexport |
| `lingxi-code/providers/src/codec.rs` | `WireCodec` trait | Drop `auth` param from `encode_request`; remove `use Auth` |
| `lingxi-code/providers/src/openai/mod.rs` | OpenAI `WireCodec` impl + tests | Drop `auth` param + `auth.apply`; fix tests |
| `lingxi-code/providers/src/gemini/mod.rs` | Gemini `WireCodec` impl + tests | Drop `auth` param + `auth.apply`; fix tests |
| `lingxi-code/providers/src/client.rs` | `GenericClient` + `MockCodec` tests | Field `auth: Auth` → `authenticator: Arc<dyn Authenticator>`; apply after encode; fix mock + tests |
| `lingxi-code/providers/src/registry.rs` | `ProviderRegistry::build` | Wrap each profile's `Auth` in `StaticAuth` |

`auth.rs` (the `Auth` enum + `apply`) is **unchanged** — `StaticAuth` wraps it.

---

## Task 1: Add the `Authenticator` trait + `StaticAuth` (additive)

**Files:**
- Create: `lingxi-code/providers/src/authenticator.rs`
- Modify: `lingxi-code/providers/src/lib.rs`

This task is purely additive — the crate stays green throughout.

- [ ] **Step 1: Create `authenticator.rs` with the trait, `StaticAuth`, and tests**

Create `lingxi-code/providers/src/authenticator.rs`:

```rust
//! Async, request-aware auth seam. `Authenticator::authorize` mutates a built
//! `HttpRequest` immediately before transport — the point where AWS SigV4 (which
//! signs over method + URI + headers + body + timestamp) and async cloud-token
//! minting (Vertex / Azure AD) must run. `StaticAuth` wraps the synchronous
//! [`Auth`] header styles (API keys); signed authenticators land in later phases.

use crate::auth::Auth;
use api_client::ApiError;
use async_trait::async_trait;
use protocol::HttpRequest;

/// Attaches authentication to a fully-built request immediately before it is
/// sent. Object-safe so `GenericClient` can hold an `Arc<dyn Authenticator>`.
#[async_trait]
pub trait Authenticator: Send + Sync {
    /// Attach or replace auth on `req` (headers, or a signed `Authorization`).
    ///
    /// # Errors
    /// Returns [`ApiError`] if credentials cannot be resolved or signing fails.
    async fn authorize(&self, req: &mut HttpRequest) -> Result<(), ApiError>;
}

/// An [`Authenticator`] for static API-key header styles — wraps the v1 [`Auth`]
/// enum (`None` / `Bearer` / `Header`). Performs no I/O; the async signature is
/// uniform with the signed authenticators added later.
#[derive(Debug, Clone)]
pub struct StaticAuth(pub Auth);

impl StaticAuth {
    /// Wrap an [`Auth`] header style as an [`Authenticator`].
    #[must_use]
    pub fn new(auth: Auth) -> Self {
        Self(auth)
    }
}

#[async_trait]
impl Authenticator for StaticAuth {
    async fn authorize(&self, req: &mut HttpRequest) -> Result<(), ApiError> {
        self.0.apply(&mut req.headers);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::HttpMethod;

    fn req() -> HttpRequest {
        HttpRequest {
            method: HttpMethod::Post,
            url: "https://x.local/v1".to_string(),
            headers: vec![("content-type".to_string(), "application/json".to_string())],
            body: Some("{}".to_string()),
            timeout: None,
        }
    }

    #[tokio::test]
    async fn static_bearer_attaches_authorization() {
        let a = StaticAuth::new(Auth::Bearer("sk-1".to_string()));
        let mut r = req();
        a.authorize(&mut r).await.unwrap();
        assert!(r
            .headers
            .iter()
            .any(|(k, v)| k == "authorization" && v == "Bearer sk-1"));
    }

    #[tokio::test]
    async fn static_header_attaches_named_header() {
        let a = StaticAuth::new(Auth::Header {
            name: "x-goog-api-key".to_string(),
            value: "k".to_string(),
        });
        let mut r = req();
        a.authorize(&mut r).await.unwrap();
        assert!(r.headers.iter().any(|(k, v)| k == "x-goog-api-key" && v == "k"));
    }

    #[tokio::test]
    async fn static_none_adds_no_header() {
        let a = StaticAuth::new(Auth::None);
        let mut r = req();
        let before = r.headers.len();
        a.authorize(&mut r).await.unwrap();
        assert_eq!(r.headers.len(), before);
    }
}
```

- [ ] **Step 2: Register the module + reexport in `lib.rs`**

In `lingxi-code/providers/src/lib.rs`, add the module declaration in alphabetical position (after `pub mod auth;` on line 12):

```rust
pub mod authenticator;
```

And add the reexport after `pub use auth::Auth;` (line 30):

```rust
pub use authenticator::{Authenticator, StaticAuth};
```

- [ ] **Step 3: Run the new tests — expect PASS**

Run (from `lingxi-code/`):
```bash
cargo test -p providers authenticator
```
Expected: the 3 `authenticator::tests::*` tests pass; whole crate still compiles.

- [ ] **Step 4: Clippy must be clean**

Run (from `lingxi-code/`):
```bash
cargo clippy -p providers --all-targets -- -D warnings
```
Expected: `Finished` with no warnings.

- [ ] **Step 5: Commit**

```bash
git add lingxi-code/providers/src/authenticator.rs lingxi-code/providers/src/lib.rs
git commit -m "feat(llm-v2 P1): add async Authenticator trait + StaticAuth"
```

---

## Task 2: Swap the auth seam (atomic refactor)

**Files:**
- Modify: `lingxi-code/providers/src/codec.rs`
- Modify: `lingxi-code/providers/src/openai/mod.rs`
- Modify: `lingxi-code/providers/src/gemini/mod.rs`
- Modify: `lingxi-code/providers/src/client.rs`
- Modify: `lingxi-code/providers/src/registry.rs`

This is one atomic refactor: changing the `WireCodec::encode_request` signature breaks every impl and call site at once, so all five files change together and the crate compiles green only at the end. The regression guard is the **existing** codec/client test suite (it must still pass) plus Task 1's `StaticAuth` tests. Apply the edits in the order below, then run the suite once.

- [ ] **Step 1: `codec.rs` — drop `auth` from the trait**

In `lingxi-code/providers/src/codec.rs`, delete the import line:
```rust
use crate::auth::Auth;
```
Replace the `encode_request` method (lines 14–22) with:
```rust
    /// Build the native HTTP request for `req` (auth-agnostic — the
    /// `GenericClient`'s `Authenticator` attaches credentials afterward).
    ///
    /// # Errors
    /// Returns [`CodecError`] if the request cannot be represented.
    fn encode_request(&self, req: &CanonicalRequest) -> Result<HttpRequest, CodecError>;
```

- [ ] **Step 2: `openai/mod.rs` — drop `auth` from impl + tests**

In `lingxi-code/providers/src/openai/mod.rs`:

Delete the import (line 7):
```rust
use crate::auth::Auth;
```

Replace the `encode_request` impl (lines 34–52) with:
```rust
    fn encode_request(&self, req: &CanonicalRequest) -> Result<HttpRequest, CodecError> {
        let body = encode::encode_chat_body(req);
        let mut headers = vec![("content-type".to_string(), "application/json".to_string())];
        if req.stream {
            headers.push(("accept".to_string(), "text/event-stream".to_string()));
        }
        Ok(HttpRequest {
            method: HttpMethod::Post,
            url: format!("{}/chat/completions", self.base_url),
            headers,
            body: Some(body.to_string()),
            timeout: Some(std::time::Duration::from_secs(600)),
        })
    }
```

Replace the two tests (lines 67–87) with (the bearer-header assertion moves to `StaticAuth`'s tests from Task 1):
```rust
    #[test]
    fn encode_request_targets_chat_completions() {
        let codec = OpenAiCodec::new(None);
        let req = CanonicalRequest::new("gpt-4o");
        let http = codec.encode_request(&req).unwrap();
        assert_eq!(http.url, "https://api.openai.com/v1/chat/completions");
        assert!(http
            .headers
            .iter()
            .any(|(k, v)| k == "content-type" && v == "application/json"));
    }

    #[test]
    fn custom_base_url_is_used() {
        let codec = OpenAiCodec::new(Some("https://api.groq.com/openai/v1".to_string()));
        let http = codec
            .encode_request(&CanonicalRequest::new("llama"))
            .unwrap();
        assert_eq!(http.url, "https://api.groq.com/openai/v1/chat/completions");
    }
```

- [ ] **Step 3: `gemini/mod.rs` — drop `auth` from impl + tests**

In `lingxi-code/providers/src/gemini/mod.rs`:

Delete the import (line 7):
```rust
use crate::auth::Auth;
```

Replace the `encode_request` impl (lines 34–61) with:
```rust
    fn encode_request(&self, req: &CanonicalRequest) -> Result<HttpRequest, CodecError> {
        let body = encode::encode_generate_body(req);
        let url = if req.stream {
            format!(
                "{}/models/{}:streamGenerateContent?alt=sse",
                self.base_url, req.model
            )
        } else {
            format!("{}/models/{}:generateContent", self.base_url, req.model)
        };
        let mut headers = vec![("content-type".to_string(), "application/json".to_string())];
        if req.stream {
            headers.push(("accept".to_string(), "text/event-stream".to_string()));
        }
        Ok(HttpRequest {
            method: HttpMethod::Post,
            url,
            headers,
            body: Some(body.to_string()),
            timeout: Some(std::time::Duration::from_secs(600)),
        })
    }
```

Replace the two tests (lines 76–104) with:
```rust
    #[test]
    fn non_stream_url_has_model_and_generate_content() {
        let codec = GeminiCodec::new(None);
        let req = CanonicalRequest::new("gemini-2.0-flash");
        let http = codec.encode_request(&req).unwrap();
        assert_eq!(
            http.url,
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.0-flash:generateContent"
        );
        assert!(http
            .headers
            .iter()
            .any(|(k, v)| k == "content-type" && v == "application/json"));
    }

    #[test]
    fn stream_url_uses_stream_endpoint() {
        let codec = GeminiCodec::new(None);
        let mut req = CanonicalRequest::new("gemini-2.0-flash");
        req.stream = true;
        let http = codec.encode_request(&req).unwrap();
        assert!(http
            .url
            .ends_with("/models/gemini-2.0-flash:streamGenerateContent?alt=sse"));
    }
```

- [ ] **Step 4: `client.rs` — `GenericClient` holds an `Authenticator`**

In `lingxi-code/providers/src/client.rs`:

Replace the import (line 4):
```rust
use crate::auth::Auth;
```
with:
```rust
use crate::authenticator::Authenticator;
```

Replace the struct field (line 22, `auth: Auth,`) with:
```rust
    authenticator: Arc<dyn Authenticator>,
```

Replace the `new` constructor (lines 30–45) with:
```rust
    /// Construct a client from a codec, authenticator, transport, id, and caps.
    #[must_use]
    pub fn new(
        codec: C,
        authenticator: Arc<dyn Authenticator>,
        transport: Arc<dyn HttpTransport>,
        id: ProviderId,
        capabilities: Capabilities,
    ) -> Self {
        Self {
            codec,
            authenticator,
            transport,
            id,
            capabilities,
        }
    }
```

Replace the body of `complete` (lines 108–115) with:
```rust
    async fn complete(&self, req: CanonicalRequest) -> Result<MessageResponse, ApiError> {
        let mut http = self
            .codec
            .encode_request(&req)
            .map_err(|e| ApiError::Http(HttpError::InvalidRequest(e.to_string())))?;
        self.authenticator.authorize(&mut http).await?;
        let resp = self.transport.request(http).await.map_err(ApiError::Http)?;
        self.codec.decode_response(resp.status, &resp.body)
    }
```

Replace the body of `stream` (lines 117–134) with:
```rust
    async fn stream(
        &self,
        req: CanonicalRequest,
    ) -> Result<BoxStream<'static, Result<StreamEvent, ApiError>>, ApiError> {
        let mut req = req;
        req.stream = true;
        let mut http = self
            .codec
            .encode_request(&req)
            .map_err(|e| ApiError::Http(HttpError::InvalidRequest(e.to_string())))?;
        self.authenticator.authorize(&mut http).await?;
        let wire = self
            .transport
            .stream_sse(http)
            .await
            .map_err(ApiError::Http)?;
        let decoder = self.codec.new_stream_decoder();
        Ok(pump_stream(wire, decoder))
    }
```

In the test module, replace the `MockCodec::encode_request` signature (lines 150–154) with:
```rust
        fn encode_request(&self, _req: &CanonicalRequest) -> Result<HttpRequest, CodecError> {
```
(Delete the `_auth: &Auth,` parameter line.)

Replace the test `client()` helper (lines 198–206) with:
```rust
    fn client(transport: MockTransport) -> GenericClient<MockCodec> {
        GenericClient::new(
            MockCodec,
            Arc::new(crate::authenticator::StaticAuth::new(crate::auth::Auth::None)),
            Arc::new(transport),
            cost::ProviderId::OpenAI,
            Capabilities::anthropic(),
        )
    }
```

- [ ] **Step 5: `registry.rs` — wrap each profile's `Auth` in `StaticAuth`**

In `lingxi-code/providers/src/registry.rs`, in `build`, replace the OpenAI branch's auth+client construction (lines 90–111) with:
```rust
            ProviderKind::OpenAi => {
                let key = self.api_key_for(profile);
                let auth = if key.is_empty() {
                    crate::auth::Auth::None
                } else {
                    crate::auth::Auth::Bearer(key)
                };
                let authenticator = Arc::new(crate::authenticator::StaticAuth::new(auth))
                    as Arc<dyn crate::authenticator::Authenticator>;
                let codec = crate::openai::OpenAiCodec::new(profile.base_url.clone());
                let id = if name == "openai" {
                    cost::ProviderId::OpenAI
                } else {
                    cost::ProviderId::OpenAICompatible {
                        name: name.to_string(),
                    }
                };
                let client = crate::client::GenericClient::new(
                    codec,
                    authenticator,
                    self.transport.clone(),
                    id,
                    crate::capabilities::Capabilities::openai(),
                );
                Arc::new(client) as Arc<dyn crate::provider::LlmProvider>
            }
```

Replace the Gemini branch (lines 113–132) with:
```rust
            ProviderKind::Gemini => {
                let key = self.api_key_for(profile);
                let auth = if key.is_empty() {
                    crate::auth::Auth::None
                } else {
                    crate::auth::Auth::Header {
                        name: "x-goog-api-key".to_string(),
                        value: key,
                    }
                };
                let authenticator = Arc::new(crate::authenticator::StaticAuth::new(auth))
                    as Arc<dyn crate::authenticator::Authenticator>;
                let codec = crate::gemini::GeminiCodec::new(profile.base_url.clone());
                let client = crate::client::GenericClient::new(
                    codec,
                    authenticator,
                    self.transport.clone(),
                    cost::ProviderId::GoogleGemini,
                    crate::capabilities::Capabilities::gemini(),
                );
                Arc::new(client) as Arc<dyn crate::provider::LlmProvider>
            }
```

- [ ] **Step 6: Run the full `providers` test suite — expect PASS**

Run (from `lingxi-code/`):
```bash
cargo test -p providers
```
Expected: all tests pass (the existing codec/client/registry tests + Task 1's authenticator tests). No compile errors.

- [ ] **Step 7: Clippy must be clean**

Run (from `lingxi-code/`):
```bash
cargo clippy -p providers --all-targets -- -D warnings
```
Expected: `Finished`, no warnings (watch for unused `Auth` imports — they should all be removed by the edits above).

- [ ] **Step 8: Commit**

```bash
git add lingxi-code/providers/src/codec.rs lingxi-code/providers/src/openai/mod.rs lingxi-code/providers/src/gemini/mod.rs lingxi-code/providers/src/client.rs lingxi-code/providers/src/registry.rs
git commit -m "refactor(llm-v2 P1): route auth through Authenticator; drop auth param from WireCodec::encode_request"
```

---

## Task 3: Parity + workspace gates (P1 checkpoint)

**Files:** none (verification only).

P1 changes no wire bytes and no dependencies, so the back-compat suites must be green and the workspace must build unchanged.

- [ ] **Step 1: Orchestrator back-compat (the `ProviderApiAdapter` path)**

Run (from `lingxi-code/`):
```bash
cargo test -p orchestrator
```
Expected: all pass (the adapter resolves through `ModelRouter`; the seam change is internal to `providers`).

- [ ] **Step 2: Parity suite (Anthropic/OpenAI/Gemini byte-locks)**

Run (from `lingxi-code/`):
```bash
cargo test -p test-harness
```
Expected: 0 failures. (OpenAI/Gemini request bytes are unchanged; Anthropic path untouched.)

- [ ] **Step 3: Workspace build**

Run (from `lingxi-code/`):
```bash
cargo build --workspace
```
Expected: `Finished`.

- [ ] **Step 4: Dependency-graph gate (no-op sanity for P1)**

Run (from `lingxi-code/`):
```bash
bash scripts/check-deps.sh
```
Expected: `OK — 73 workspace crates` (P1 adds no dependencies).

- [ ] **Step 5: Tag the phase locally**

```bash
git tag -a llm-v2-p1 -m "LLM Providers v2 P1: auth-seam refactor"
```

---

## Self-Review

**Spec coverage (§2.1):** `Authenticator` trait (Task 1) ✓; `StaticAuth` wraps `Auth` (Task 1) ✓; `encode_request` drops `auth` (Task 2 Step 1) ✓; `GenericClient` holds `Arc<dyn Authenticator>` + applies after encode (Task 2 Step 4) ✓; OpenAI/Gemini codecs stop attaching headers (Task 2 Steps 2–3) ✓; registry wraps `Auth` in `StaticAuth` (Task 2 Step 5) ✓; Anthropic untouched (not in any task) ✓; parity gate (Task 3) ✓.

**Placeholder scan:** No TBD/TODO; every code step shows full code; every command shows expected output.

**Type consistency:** `Authenticator::authorize(&self, &mut HttpRequest) -> Result<(), ApiError>` used identically in Task 1 (def), Task 2 Step 4 (call in `complete`/`stream`). `StaticAuth::new(Auth) -> Self` used in Task 1 (def), Task 2 Step 4 (client helper) and Step 5 (registry). `encode_request(&self, &CanonicalRequest)` consistent across codec.rs trait + both codec impls + MockCodec. `Arc<dyn Authenticator>` consistent in field, `new` param, and registry casts.
