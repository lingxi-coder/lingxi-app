# LLM Providers v2 — P5: Vertex AI (Gemini) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]`.

**Goal:** Run Gemini models on Google Vertex AI — the Gemini `generateContent` body over Vertex's regional `publishers/google/models/...` URL, authenticated with a GCP OAuth2 access token (service account / ADC) via the async `Authenticator` seam.

**Architecture:** Reuse the Gemini codec body. `GeminiCodec` gains a `UrlStyle` (`GeminiApi` | `Vertex { project, region }`); a new `new_vertex(...)` constructor sets the Vertex style while the existing `new(base_url, thinking_budget)` is UNCHANGED (no blast radius). A new `GcpTokenAuthenticator` (impl of the P1 `Authenticator` trait) lazily mints + caches a bearer token via the `gcp_auth` crate and attaches `Authorization: Bearer …`. The registry builds a Vertex profile with that authenticator.

**Tech Stack:** Rust 1.82.0, `gcp_auth` (official GCP auth crate), `tokio::sync::OnceCell`. Run cargo from `lingxi-code/`.

**Spec:** `2026-06-01-llm-providers-v2-design.md` §3.4, §8. Branch `llm-providers-v2` (P4 done, tag `llm-v2-p4`). The brainstorm locked "official cloud crates" for signed auth.

**Bounded decisions:**
- Cost: Vertex-Gemini uses `cost::ProviderId::GoogleGemini` (it IS Gemini; reuses the Gemini price table) — no cost-crate change.
- Testability: the live token fetch needs real GCP credentials, so it is NOT unit-tested. P5's tests cover URL building, profile parsing, registry resolution, and authenticator construction. The token path is exercised only against a live GCP environment (documented).
- The model request still flows through `platform_api::HttpTransport`; `gcp_auth` is used ONLY to mint the token (its own credential-discovery I/O is an accepted side channel).

**Parity gate:** the existing Gemini API path is untouched (`new` unchanged; `UrlStyle::GeminiApi` reproduces today's URL). Existing tests + `test-harness` parity stay green. Do NOT modify `traits/`.

---

## Task A: `gcp_auth` dependency + `GcpTokenAuthenticator`

**Files:** `providers/Cargo.toml`, `providers/src/authenticator.rs` (or a new `authenticator/gcp.rs` module), `deny.toml` (repo root or `lingxi-code/` — wherever it lives), and re-export in `lib.rs`.

- [ ] **Step 1 — add the dependency.** From `lingxi-code/`, run `cargo add gcp_auth -p providers` (let it resolve the latest 0.x). Confirm `tokio` is a NON-dev dependency of `providers` with the `sync` feature (for `tokio::sync::OnceCell`); if it's currently dev-only, add `tokio = { workspace = true, features = ["sync"] }` to `[dependencies]`.

- [ ] **Step 2 — implement `GcpTokenAuthenticator`.** Add to the authenticator module:
```rust
/// Mints + caches a GCP OAuth2 access token (service account / ADC / metadata)
/// via `gcp_auth` and attaches it as `Authorization: Bearer …`. Used by Vertex.
/// The token provider is created lazily on first use (the registry builds
/// synchronously; provider discovery is async).
pub struct GcpTokenAuthenticator {
    cell: tokio::sync::OnceCell<std::sync::Arc<dyn /* gcp_auth token provider trait */>>,
}
```
Implement `Authenticator::authorize`:
1. `get_or_try_init` the token provider via `gcp_auth`'s provider-discovery entry point (async).
2. Request a token for the cloud-platform scope `"https://www.googleapis.com/auth/cloud-platform"`.
3. Push `("authorization", format!("Bearer {}", token_str))` onto `req.headers`.
4. Map any `gcp_auth` error to `ApiError` — prefer `ApiError::Unauthorized` (or the closest variant; check `api_client::ApiError`) with the error text.

**ADAPT to the real `gcp_auth` API** (the exact names vary by version — consult `cargo doc -p gcp_auth` or compiler errors). The canonical 0.x shape is roughly: `gcp_auth::provider().await -> Result<Arc<dyn TokenProvider>>`; `provider.token(&[scope]).await -> Result<Arc<Token>>`; `token.as_str()`. Use whatever compiles for the resolved version. Provide a `Default`/`new()` constructor that initializes an empty `OnceCell`.

- [ ] **Step 3 — re-export** `GcpTokenAuthenticator` from `lib.rs` (alongside `Authenticator`/`StaticAuth`).

- [ ] **Step 4 — construction smoke test.** Add a unit test that constructs `GcpTokenAuthenticator::new()` (or `default()`) and asserts it builds — do NOT call `.authorize()` (needs live GCP creds). Keep it minimal; the goal is to confirm the type + trait impl compile and the constructor works.

- [ ] **Step 5 — dependency gate.** From `lingxi-code/`:
```bash
cargo build -p providers
bash scripts/check-deps.sh
```
If `check-deps` fails because `gcp_auth` adds a dependency the §8.1 graph rules don't allow for `providers`, read `scripts/check-deps.sh` to see the rule and add the minimal allowance consistent with "providers is a leaf-ish crate that may use external auth crates". If `cargo deny`/`deny.toml` is part of the gate and flags `gcp_auth`'s transitive licenses, add the needed license allowances to `deny.toml` (gcp_auth's tree is MIT/Apache/ISC/etc.). Report exactly what you changed in the gate config.

- [ ] **Step 6 — clippy + commit.**
```bash
cargo clippy -p providers --no-deps --all-targets -- -D warnings
git add lingxi-code/providers/Cargo.toml lingxi-code/providers/src/ lingxi-code/Cargo.lock
# include deny.toml / scripts if changed
git commit -m "feat(llm-v2 P5): GcpTokenAuthenticator (gcp_auth) for Vertex signed auth"
```

---

## Task B: Gemini Vertex URL style + profile + registry

**Files:** `providers/src/profile.rs`, `providers/src/gemini/mod.rs`, `providers/src/registry.rs`.

- [ ] **Step 1 — `profile.rs`: `Vertex` kind + fields.**

Add `Vertex` to `ProviderKind` (with `///` doc). In `ProviderKind::parse`, add `"vertex" => Ok(Self::Vertex),` and update the expected-list error. Add to `ProviderProfile` (with `///` docs):
```rust
    /// GCP project id (Vertex only).
    pub project: Option<String>,
    /// GCP region, e.g. `us-central1` (Vertex only).
    pub region: Option<String>,
```
Set both `None` in all `builtin_profiles` literals. In `parse_profiles`, parse `project` and `region` (string fields, same pattern as `baseUrl`) and include them. Add a test `parse_vertex_profile` (`{"type":"vertex","project":"p","region":"us-central1"}` → `kind == Vertex`, `project == Some("p")`, `region == Some("us-central1")`).

- [ ] **Step 2 — `gemini/mod.rs`: `UrlStyle` + `new_vertex` + URL branch.**

Add above `GeminiCodec`:
```rust
/// How the Gemini codec builds its request URL.
enum UrlStyle {
    /// `{base_url}/models/{model}:generateContent` (Gemini API).
    GeminiApi,
    /// Vertex AI: `{region}-aiplatform.googleapis.com/v1/projects/{project}/locations/{region}/publishers/google/models/{model}:…`.
    Vertex {
        /// GCP project id.
        project: String,
        /// GCP region.
        region: String,
    },
}
```
Add `url_style: UrlStyle` to `GeminiCodec` (keep `base_url`, `thinking_budget`); `new` sets `UrlStyle::GeminiApi` (signature UNCHANGED). Add:
```rust
    /// Construct a Vertex AI Gemini codec (GCP project + region; bearer-token auth).
    #[must_use]
    pub fn new_vertex(project: String, region: String, thinking_budget: Option<u32>) -> Self {
        Self {
            base_url: String::new(),
            url_style: UrlStyle::Vertex { project, region },
            thinking_budget,
        }
    }
```
In `encode_request`, replace the `url` construction with a `UrlStyle` match. For `GeminiApi`, keep today's logic (stream vs non-stream against `self.base_url`). For `Vertex { project, region }`:
```rust
            UrlStyle::Vertex { project, region } => {
                let host = format!("https://{region}-aiplatform.googleapis.com/v1");
                let base = format!("{host}/projects/{project}/locations/{region}/publishers/google/models/{}", req.model);
                if req.stream {
                    format!("{base}:streamGenerateContent?alt=sse")
                } else {
                    format!("{base}:generateContent")
                }
            }
```
Add a test `vertex_url_targets_aiplatform_endpoint` asserting the non-stream URL = `https://us-central1-aiplatform.googleapis.com/v1/projects/p/locations/us-central1/publishers/google/models/gemini-2.5-pro:generateContent` for `new_vertex("p","us-central1",None)` + `CanonicalRequest::new("gemini-2.5-pro")`, and that the stream URL ends with `:streamGenerateContent?alt=sse`.

- [ ] **Step 3 — `registry.rs`: `Vertex` build branch.**
```rust
            ProviderKind::Vertex => {
                let authenticator = Arc::new(crate::authenticator::GcpTokenAuthenticator::new())
                    as Arc<dyn crate::authenticator::Authenticator>;
                let codec = crate::gemini::GeminiCodec::new_vertex(
                    profile.project.clone().unwrap_or_default(),
                    profile.region.clone().unwrap_or_default(),
                    profile.thinking_budget,
                );
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
Add a registry test `vertex_profile_resolves` (a `ProviderProfile` with `kind: Vertex, project: Some("p"), region: Some("us-central1")`, all other fields incl. the Azure/reasoning ones `None`) → `resolve("vertex/gemini-2.5-pro")` → `model == "gemini-2.5-pro"`, `provider.id() == GoogleGemini`. Update any other `ProviderProfile {..}` literal flagged by the compiler with `project: None, region: None`.

- [ ] **Step 4 — gates + commit.**
```bash
cargo test -p providers
cargo clippy -p providers --no-deps --all-targets -- -D warnings
git add lingxi-code/providers/src/profile.rs lingxi-code/providers/src/gemini/ lingxi-code/providers/src/registry.rs
git commit -m "feat(llm-v2 P5): Vertex AI Gemini provider (regional URL + GCP token auth)"
```

---

## Task C: Phase gates + tag

- [ ] **Step 1:** `cargo test -p providers -p orchestrator -p test-harness` — all pass.
- [ ] **Step 2:** `cargo build --workspace` — Finished.
- [ ] **Step 3:** `cargo clippy -p providers --no-deps --all-targets -- -D warnings` — clean.
- [ ] **Step 4:** from `lingxi-code/`: `bash scripts/check-deps.sh` — OK (crate count may rise from `gcp_auth`'s tree; confirm no §8.1 violation).
- [ ] **Step 5:** `git tag -a llm-v2-p5 -m "LLM Providers v2 P5: Vertex AI Gemini"`.

---

## Self-Review

**Spec coverage (§3.4):** `ProviderKind::Vertex` + `project`/`region` (B1) ✓; Gemini body reuse via `UrlStyle::Vertex` URL (B2) ✓; `GcpTokenAuthenticator` (A) ✓; registry Vertex branch wires the authenticator (B3) ✓; cost `GoogleGemini` (bounded) ✓; `gcp_auth` dep + dep-gate (A5) ✓.

**Placeholder scan:** the codec/profile/registry steps are fully paste-ready. The `GcpTokenAuthenticator` is specified at design level WITH the canonical `gcp_auth` API and an explicit "adapt to the resolved version" instruction — this is deliberate (the exact crate API depends on the resolved version) and bounded by the construction smoke test + the build gate, not a vague placeholder.

**Type consistency:** `UrlStyle`/`new_vertex(String,String,Option<u32>)` consistent between `gemini/mod.rs` and the registry. `GcpTokenAuthenticator::new()` used in registry + re-exported. `ProviderProfile` gains `project`/`region` consistently across profile.rs + registry literals (plus the Azure/reasoning fields from P3/P4).

**Blast-radius:** `GeminiCodec::new` signature UNCHANGED (only `new_vertex` added) → external callers (test-harness parity) unaffected. New `ProviderProfile` fields require updating struct literals in tests — Step B3 calls this out.
