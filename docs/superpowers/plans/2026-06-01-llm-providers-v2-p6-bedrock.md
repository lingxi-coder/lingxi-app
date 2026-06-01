# LLM Providers v2 — P6: Bedrock (Claude) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]`.

**Goal:** Run Claude on AWS Bedrock — the Anthropic Messages body over Bedrock's `InvokeModel` endpoint, authenticated with AWS SigV4.

**Architecture (bounded by the frozen `traits::HttpTransport`):** `HttpTransport` exposes only `request` (full body) and `stream_sse` (SSE) — NOT raw bytes. Bedrock streaming uses AWS binary event-stream framing (not SSE), which we cannot deliver without modifying frozen `traits`. Therefore P6 ships **non-streaming Bedrock** via `POST …/invoke` (a single Anthropic-shaped JSON body, which `MessageResponse` deserializes directly) and a **synthetic single-shot `stream()`** that re-emits the completed response as a valid event sequence. This also removes any need for `aws-smithy-eventstream`.

`BedrockProvider` is a bespoke `LlmProvider` (like `AnthropicLlmProvider`), NOT a `GenericClient`/`WireCodec` (it needs `/invoke` + a synthetic stream + an Anthropic body). Auth is a new `SigV4Authenticator` (the P1 `Authenticator` seam) using `aws-sigv4` + `aws-credential-types` with **environment-variable credentials** (`AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY` / optional `AWS_SESSION_TOKEN`).

**Tech Stack:** Rust 1.82.0, `aws-sigv4` (1.x), `aws-credential-types` (1.x). Run cargo from `lingxi-code/`.

**Spec:** `2026-06-01-llm-providers-v2-design.md` §3.5, §8. Branch `llm-providers-v2` (P5 done, tag `llm-v2-p5`).

**Bounded decisions (documented):**
- **Non-streaming only** + synthetic stream (frozen-`traits` constraint). Real token streaming for Bedrock is deferred (would need a `traits::HttpTransport` raw-byte-stream method).
- **Env-var credentials** via `aws-credential-types` (no `aws-config` — avoids the heavy SDK runtime + Rust-1.82/edition2024 transitive risk). SSO / IMDS / profile discovery deferred (documented: set the AWS env vars).
- **Cost:** Bedrock-Claude uses `cost::ProviderId::Anthropic`. Bedrock model ids (e.g. `anthropic.claude-3-5-sonnet-20241022-v2:0`) won't match the Anthropic price keys, so cost may under-report until Bedrock price rows are added (a follow-up). No cost-crate change in P6.

**Parity gate:** Bedrock is net-new; nothing existing changes. Existing tests + `test-harness` parity stay green. Do NOT modify `traits/`.

---

## Task A: `aws-sigv4` deps + `SigV4Authenticator`

**Files:** `providers/Cargo.toml`, `providers/src/authenticator.rs`, `lib.rs`, dep-gate config if needed.

- [ ] **Step 1 — deps.** From `lingxi-code/`: `cargo add aws-sigv4 -p providers` and `cargo add aws-credential-types -p providers`. If a 1.x version pulls an edition2024 transitive that breaks Rust 1.82 (as happened with `gcp_auth` → `rustls-native-certs`), pin the offending transitive to the last 1.82-compatible version via `cargo update <crate> --precise <ver>` (document the pin). If `aws-sigv4` cannot be made to build under 1.82 with reasonable pins, STOP and report BLOCKED with the exact error.

- [ ] **Step 2 — `SigV4Authenticator`.** Add to `authenticator.rs`. Structure it so the signing core is testable WITHOUT touching process env or wall-clock:
```rust
/// Signs Bedrock requests with AWS SigV4 using environment-variable credentials
/// (`AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY` / optional `AWS_SESSION_TOKEN`).
pub struct SigV4Authenticator {
    region: String,
}

impl SigV4Authenticator {
    /// Construct for an AWS region (e.g. `us-east-1`).
    #[must_use]
    pub fn new(region: String) -> Self {
        Self { region }
    }

    /// Sign `req` in place using the given credentials + time (testable core).
    fn sign_in_place(
        &self,
        req: &mut HttpRequest,
        access_key: &str,
        secret_key: &str,
        session_token: Option<&str>,
        time: std::time::SystemTime,
    ) -> Result<(), ApiError> {
        // Build aws_credential_types::Credentials, aws_sigv4 v4::SigningParams
        // (region = self.region, name = "bedrock", time), a SignableRequest from
        // req.method/url/headers + SignableBody::Bytes(req.body bytes), call
        // aws_sigv4::http_request::sign, then push every signing-instruction header
        // onto req.headers. Map all aws-sigv4 errors → ApiError::Unauthorized.
        // ADAPT to the resolved aws-sigv4 1.x API (consult cargo doc).
        todo!("implement per aws-sigv4 1.x")
    }
}

#[async_trait]
impl Authenticator for SigV4Authenticator {
    async fn authorize(&self, req: &mut HttpRequest) -> Result<(), ApiError> {
        let access_key = std::env::var("AWS_ACCESS_KEY_ID")
            .map_err(|_| ApiError::Unauthorized("AWS_ACCESS_KEY_ID not set".to_string()))?;
        let secret_key = std::env::var("AWS_SECRET_ACCESS_KEY")
            .map_err(|_| ApiError::Unauthorized("AWS_SECRET_ACCESS_KEY not set".to_string()))?;
        let session_token = std::env::var("AWS_SESSION_TOKEN").ok();
        self.sign_in_place(
            req,
            &access_key,
            &secret_key,
            session_token.as_deref(),
            std::time::SystemTime::now(),
        )
    }
}
```
Replace the `todo!` with the real `aws-sigv4` 1.x signing. The canonical 1.x shape: build `aws_credential_types::Credentials::new(access, secret, session_token, None, "bedrock-env")`; an `Identity` from it; `aws_sigv4::sign::v4::SigningParams` via its builder (`.identity(&identity).region(&self.region).name("bedrock").time(time).settings(SigningSettings::default()).build()?`); a `SignableRequest::new(method_str, url, headers_iter, SignableBody::Bytes(body))?`; `aws_sigv4::http_request::sign(signable, &params.into())?.into_parts()` → `(instructions, _sig)`; then `for header in instructions.headers() { req.headers.push((name, value)) }`. Map the HTTP method enum to its `&str` ("POST"). Adapt exact names to the resolved version.

- [ ] **Step 3 — re-export** `SigV4Authenticator` in `lib.rs`.

- [ ] **Step 4 — signing tests (deterministic, no env/clock races).** Test `sign_in_place` directly with FIXED credentials + a FIXED `SystemTime` (e.g. `UNIX_EPOCH + Duration::from_secs(1_440_938_160)` = 2015-08-30T12:36:00Z, AWS's doc example time):
```rust
    #[test]
    fn sigv4_signs_with_authorization_header_and_is_deterministic() {
        let auth = SigV4Authenticator::new("us-east-1".to_string());
        let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_440_938_160);
        let mk = || HttpRequest {
            method: protocol::HttpMethod::Post,
            url: "https://bedrock-runtime.us-east-1.amazonaws.com/model/m/invoke".to_string(),
            headers: vec![("content-type".to_string(), "application/json".to_string())],
            body: Some("{}".to_string()),
            timeout: None,
        };
        let mut a = mk();
        auth.sign_in_place(&mut a, "AKIDEXAMPLE", "secret", None, t).unwrap();
        let authz = a.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("authorization"));
        let (_, v) = authz.expect("authorization header present");
        assert!(v.starts_with("AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/"));
        assert!(v.contains("/us-east-1/bedrock/aws4_request"));
        // deterministic: same inputs → identical signature
        let mut b = mk();
        auth.sign_in_place(&mut b, "AKIDEXAMPLE", "secret", None, t).unwrap();
        assert_eq!(a.headers, b.headers);
    }
```

- [ ] **Step 5 — dep gate + clippy + commit.**
```bash
cargo build -p providers
bash scripts/check-deps.sh   # from lingxi-code/; add minimal §8.1 allowance / deny.toml license entries if it flags aws-* deps
cargo clippy -p providers --no-deps --all-targets -- -D warnings
git add lingxi-code/providers/Cargo.toml lingxi-code/providers/src/ lingxi-code/Cargo.lock
git commit -m "feat(llm-v2 P6): SigV4Authenticator (aws-sigv4, env credentials) for Bedrock

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task B: `BedrockProvider` + profile + registry

**Files:** `providers/src/bedrock.rs` (new), `lib.rs`, `providers/src/profile.rs`, `providers/src/registry.rs`.

- [ ] **Step 1 — `bedrock.rs`: the provider.**
```rust
//! Claude-on-Bedrock provider: the Anthropic Messages body over Bedrock's
//! `InvokeModel` endpoint + SigV4 auth. Non-streaming (`/invoke`); `stream()`
//! re-emits the completed response as a synthetic single-shot event stream
//! (the frozen `HttpTransport` exposes no raw byte stream for AWS event-stream
//! framing, so real Bedrock streaming is deferred).

use crate::authenticator::Authenticator;
use crate::capabilities::Capabilities;
use crate::provider::LlmProvider;
use crate::request::CanonicalRequest;
use api_client::types::{ContentBlockApi, ContentDelta, MessageDeltaPayload, MessageResponse, StreamEvent};
use api_client::ApiError;
use async_trait::async_trait;
use cost::ProviderId;
use futures::stream::{self, BoxStream, StreamExt};
use protocol::{HttpMethod, HttpRequest};
use serde_json::{json, Map, Value};
use std::sync::Arc;
use std::time::Duration;
use traits::HttpTransport;

/// A Claude-on-Bedrock provider.
pub struct BedrockProvider {
    region: String,
    transport: Arc<dyn HttpTransport>,
    authenticator: Arc<dyn Authenticator>,
    capabilities: Capabilities,
}

impl BedrockProvider {
    /// Construct for an AWS region with a (SigV4) authenticator + transport.
    #[must_use]
    pub fn new(region: String, transport: Arc<dyn HttpTransport>, authenticator: Arc<dyn Authenticator>) -> Self {
        Self { region, transport, authenticator, capabilities: Capabilities::anthropic() }
    }

    /// Build the Anthropic-on-Bedrock request body (no top-level `model`; carries
    /// `anthropic_version`). `messages`/`tools` serialize via their canonical
    /// (Anthropic-shaped) serde.
    fn build_body(req: &CanonicalRequest) -> Value {
        let mut body = Map::new();
        body.insert("anthropic_version".to_string(), json!("bedrock-2023-05-31"));
        body.insert("max_tokens".to_string(), json!(req.max_tokens));
        body.insert(
            "messages".to_string(),
            serde_json::to_value(&req.messages).unwrap_or(Value::Array(vec![])),
        );
        if let Some(s) = &req.system {
            body.insert("system".to_string(), json!(s));
        }
        if !req.tools.is_empty() {
            body.insert("tools".to_string(), Value::Array(req.tools.clone()));
        }
        Value::Object(body)
    }

    fn invoke_url(&self, model: &str) -> String {
        format!("https://bedrock-runtime.{}.amazonaws.com/model/{model}/invoke", self.region)
    }
}

/// Re-emit a completed `MessageResponse` as a valid single-shot event sequence.
fn synthesize_stream(resp: MessageResponse) -> Vec<StreamEvent> {
    let mut out = Vec::new();
    out.push(StreamEvent::MessageStart {
        message: MessageResponse {
            id: resp.id.clone(),
            model: resp.model.clone(),
            content: Vec::new(),
            stop_reason: None,
            usage: resp.usage,
        },
    });
    for (i, block) in resp.content.iter().enumerate() {
        let index = u32::try_from(i).unwrap_or(0);
        out.push(StreamEvent::ContentBlockStart { index, content_block: block.clone() });
        match block {
            ContentBlockApi::Text { text } => out.push(StreamEvent::ContentBlockDelta {
                index,
                delta: ContentDelta::TextDelta { text: text.clone() },
            }),
            ContentBlockApi::ToolUse { input, .. } => out.push(StreamEvent::ContentBlockDelta {
                index,
                delta: ContentDelta::InputJsonDelta { partial_json: input.to_string() },
            }),
            _ => {}
        }
        out.push(StreamEvent::ContentBlockStop { index });
    }
    out.push(StreamEvent::MessageDelta {
        delta: MessageDeltaPayload { stop_reason: resp.stop_reason.clone() },
        usage: Some(resp.usage),
    });
    out.push(StreamEvent::MessageStop);
    out
}

#[async_trait]
impl LlmProvider for BedrockProvider {
    fn id(&self) -> ProviderId { ProviderId::Anthropic }
    fn capabilities(&self) -> &Capabilities { &self.capabilities }

    async fn complete(&self, req: CanonicalRequest) -> Result<MessageResponse, ApiError> {
        let body = Self::build_body(&req);
        let mut http = HttpRequest {
            method: HttpMethod::Post,
            url: self.invoke_url(&req.model),
            headers: vec![("content-type".to_string(), "application/json".to_string())],
            body: Some(body.to_string()),
            timeout: Some(Duration::from_secs(600)),
        };
        self.authenticator.authorize(&mut http).await?;
        let resp = self.transport.request(http).await.map_err(ApiError::Http)?;
        if !(200..300).contains(&resp.status) {
            return Err(ApiError::Server { status: resp.status, body: resp.body });
        }
        serde_json::from_str::<MessageResponse>(&resp.body)
            .map_err(|e| ApiError::MalformedStream(format!("bedrock response decode: {e}")))
    }

    async fn stream(&self, req: CanonicalRequest) -> Result<BoxStream<'static, Result<StreamEvent, ApiError>>, ApiError> {
        let resp = self.complete(req).await?;
        Ok(stream::iter(synthesize_stream(resp).into_iter().map(Ok)).boxed())
    }
}
```
Add module + re-export in `lib.rs` (`pub mod bedrock;`, `pub use bedrock::BedrockProvider;`). Confirm `ApiError` has `Server { status, body }` and `MalformedStream(String)` variants (the OpenAI/Gemini decoders use them — copy the exact variant shapes). Add unit tests in `bedrock.rs`:
  - `build_body_has_anthropic_version_no_model`: `build_body` output has `anthropic_version == "bedrock-2023-05-31"`, `max_tokens`, `messages`, and NO top-level `model`.
  - `synthesize_stream_emits_valid_sequence`: a `MessageResponse` with one Text block + usage → events start with `MessageStart`, contain a `ContentBlockStart`+`TextDelta`+`ContentBlockStop`, then exactly one `MessageDelta` and end with `MessageStop`.

- [ ] **Step 2 — `profile.rs`: `Bedrock` kind + region.** Add `Bedrock` to `ProviderKind` (`///` doc). In `parse`, `"bedrock" => Ok(Self::Bedrock),` + expected-list update. `ProviderProfile` ALREADY has a `region` field (added in P5 for Vertex) — reuse it for Bedrock's AWS region (no new field). Add a test `parse_bedrock_profile` (`{"type":"bedrock","region":"us-east-1"}` → `kind == Bedrock`, `region == Some("us-east-1")`).

- [ ] **Step 3 — `registry.rs`: `Bedrock` build branch.**
```rust
            ProviderKind::Bedrock => {
                let region = profile.region.clone().unwrap_or_default();
                let authenticator = Arc::new(crate::authenticator::SigV4Authenticator::new(region.clone()))
                    as Arc<dyn crate::authenticator::Authenticator>;
                let provider = crate::bedrock::BedrockProvider::new(
                    region,
                    self.transport.clone(),
                    authenticator,
                );
                Arc::new(provider) as Arc<dyn crate::provider::LlmProvider>
            }
```
Add a registry test `bedrock_profile_resolves` (a `ProviderProfile` with `kind: Bedrock, region: Some("us-east-1")`, all other fields `None`) → `resolve("bedrock/anthropic.claude-3-5-sonnet-20241022-v2:0")` → `model == "anthropic.claude-3-5-sonnet-20241022-v2:0"`, `provider.id() == Anthropic`. (Note: the model id contains a `:` and `/` — confirm `ModelSpec::parse` splits only on the FIRST `/`, so `bedrock/anthropic.claude-...:0` → profile `bedrock`, model `anthropic.claude-...:0`. If `ModelSpec` mishandles this, note it — but it splits on the first `/` so it should be fine.)

- [ ] **Step 4 — gates + commit.**
```bash
cargo test -p providers
cargo clippy -p providers --no-deps --all-targets -- -D warnings
git add lingxi-code/providers/src/ lingxi-code/providers/Cargo.toml
git commit -m "feat(llm-v2 P6): Claude-on-Bedrock provider (InvokeModel + SigV4, synthetic stream)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task C: Phase gates + tag

- [ ] **Step 1:** `cargo test -p providers -p orchestrator -p test-harness` — all pass.
- [ ] **Step 2:** `cargo build --workspace` — Finished.
- [ ] **Step 3:** `cargo clippy -p providers --no-deps --all-targets -- -D warnings` — clean.
- [ ] **Step 4:** from `lingxi-code/`: `bash scripts/check-deps.sh` — OK (confirm no §8.1 violation from aws-* deps).
- [ ] **Step 5:** `git tag -a llm-v2-p6 -m "LLM Providers v2 P6: Claude-on-Bedrock"`.

---

## Self-Review

**Spec coverage (§3.5):** `BedrockProvider` with Anthropic-Bedrock body (`anthropic_version`, no `model`) (B1) ✓; `InvokeModel` URL (B1) ✓; SigV4 auth (A) ✓; `ProviderKind::Bedrock` + region reuse (B2) ✓; registry branch (B3) ✓; cost `Anthropic` (bounded) ✓; aws-sigv4 deps + dep-gate (A) ✓. **Deviations from spec (frozen-`traits`-driven, documented):** non-streaming + synthetic stream instead of event-stream; env-var creds instead of `aws-config`; no `aws-smithy-eventstream`; Bedrock pricing deferred. All justified above.

**Placeholder scan:** `BedrockProvider` + `synthesize_stream` + `build_body` + profile + registry are fully paste-ready. `SigV4Authenticator::sign_in_place` is design-level with the aws-sigv4 1.x API shape + an explicit adapt instruction (the exact API is version-specific) — bounded by the deterministic signing test + the build gate.

**Type consistency:** `BedrockProvider::new(String, Arc<dyn HttpTransport>, Arc<dyn Authenticator>)` consistent between `bedrock.rs` and registry. `SigV4Authenticator::new(String)` consistent. Reuses the existing `region` profile field (no new field). `synthesize_stream` emits the same event vocabulary the SSE decoders produce (`MessageStart`/`ContentBlock*`/`MessageDelta`/`MessageStop`), so the orchestrator reassembler accepts it.

**Blast-radius:** `BedrockProvider` is additive (new file); `ProviderKind::Bedrock` is a new arm; `region` field already exists. No external signature changes. New `ProviderKind` variant → the registry `match` gains an arm (same file). No `ProviderProfile` field additions (region reused) → no struct-literal blast radius this phase.
