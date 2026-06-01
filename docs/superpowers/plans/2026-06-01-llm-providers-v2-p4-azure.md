# LLM Providers v2 — P4: Azure OpenAI — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]`.

**Goal:** Support Azure OpenAI as a provider profile — the OpenAI chat-completions wire over Azure's deployment-URL + `api-key` header.

**Architecture:** Reuse the OpenAI codec body. `OpenAiCodec` gains a `UrlStyle` field (`OpenAi` | `Azure { deployment, api_version }`); a new `new_azure(...)` constructor sets the Azure style while the existing `new(base_url, reasoning_effort)` is UNCHANGED (no external-caller blast radius). The registry builds an Azure profile with `StaticAuth(Auth::Header { "api-key", key })`.

**Tech Stack:** Rust 1.82.0, serde_json. Run cargo from `lingxi-code/`.

**Spec:** `2026-06-01-llm-providers-v2-design.md` §3.3. Branch `llm-providers-v2` (P3 done, tag `llm-v2-p3`).

**Deviation note (bounded):** cost attribution for Azure uses the existing `ProviderId::OpenAICompatible { name }` (e.g. `name = "azure"`), not a new dedicated `AzureOpenAi` cost variant — Azure is wire-compatible with OpenAI and this avoids a cost-crate change. (A dedicated cost variant can come later if per-Azure pricing is needed.)

**Parity gate:** the existing OpenAI path is untouched (`new` signature unchanged; `UrlStyle::OpenAi` reproduces today's URL exactly). Existing tests + `test-harness` parity stay green. Do NOT modify `traits/`.

---

## Task A: Azure OpenAI profile + codec URL style

**Files:** `providers/src/profile.rs`, `providers/src/openai/mod.rs`, `providers/src/registry.rs`.

- [ ] **Step 1 — `profile.rs`: `AzureOpenAi` kind + fields.**

Add `AzureOpenAi` to `ProviderKind`:
```rust
    /// Azure OpenAI (OpenAI chat wire over a deployment URL + `api-key` header).
    AzureOpenAi,
```
In `ProviderKind::parse`, accept it (both spellings):
```rust
            "azureOpenAi" | "azure" => Ok(Self::AzureOpenAi),
```
(add to the existing match, before the `other =>` arm; also update the error message's expected-list to include `azureOpenAi`).

Add to `ProviderProfile` (with `///` docs):
```rust
    /// Azure deployment name (Azure OpenAI only) — goes in the request URL path.
    pub azure_deployment: Option<String>,
    /// Azure API version (Azure OpenAI only) — the `api-version` query param.
    pub azure_api_version: Option<String>,
```
Set both `None` in `builtin_profiles` (all three built-ins). In `parse_profiles`, after the reasoning fields, parse:
```rust
        let azure_deployment = obj
            .get("azureDeployment")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        let azure_api_version = obj
            .get("azureApiVersion")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
```
and include both in the constructed `ProviderProfile`. Add a test `parse_azure_profile`:
```rust
    #[test]
    fn parse_azure_profile() {
        let mut raw = BTreeMap::new();
        raw.insert(
            "azure".to_string(),
            json!({"type":"azureOpenAi","baseUrl":"https://r.openai.azure.com",
                   "azureDeployment":"gpt-4o","azureApiVersion":"2024-10-21","apiKeyEnv":"AZURE_OPENAI_KEY"}),
        );
        let p = parse_profiles(Some(&raw)).unwrap();
        assert_eq!(p["azure"].kind, ProviderKind::AzureOpenAi);
        assert_eq!(p["azure"].azure_deployment.as_deref(), Some("gpt-4o"));
        assert_eq!(p["azure"].azure_api_version.as_deref(), Some("2024-10-21"));
    }
```

- [ ] **Step 2 — `openai/mod.rs`: `UrlStyle` + `new_azure` + URL branch.**

Add the enum (above `OpenAiCodec`):
```rust
/// How the OpenAI codec builds its request URL.
enum UrlStyle {
    /// `{base_url}/chat/completions` (OpenAI + OpenAI-compatible).
    OpenAi,
    /// `{base_url}/openai/deployments/{deployment}/chat/completions?api-version=…` (Azure).
    Azure {
        /// Azure deployment name.
        deployment: String,
        /// Azure `api-version` query value.
        api_version: String,
    },
}
```
Add a `url_style: UrlStyle` field to `OpenAiCodec` (keep `base_url` and `reasoning_effort`). In `new`, set `url_style: UrlStyle::OpenAi`. Add:
```rust
    /// Construct an Azure OpenAI codec: the OpenAI body over Azure's
    /// deployment URL. `base_url` is the resource endpoint
    /// (e.g. `https://my-resource.openai.azure.com`).
    #[must_use]
    pub fn new_azure(
        base_url: String,
        deployment: String,
        api_version: String,
        reasoning_effort: Option<crate::request::ReasoningEffort>,
    ) -> Self {
        Self {
            base_url,
            url_style: UrlStyle::Azure { deployment, api_version },
            reasoning_effort,
        }
    }
```
In `encode_request`, replace the `url:` construction with a `UrlStyle` match:
```rust
        let url = match &self.url_style {
            UrlStyle::OpenAi => format!("{}/chat/completions", self.base_url),
            UrlStyle::Azure { deployment, api_version } => format!(
                "{}/openai/deployments/{deployment}/chat/completions?api-version={api_version}",
                self.base_url
            ),
        };
```
and use `url` in the returned `HttpRequest`. Add a test:
```rust
    #[test]
    fn azure_url_style_targets_deployment_path() {
        let codec = OpenAiCodec::new_azure(
            "https://r.openai.azure.com".to_string(),
            "gpt-4o".to_string(),
            "2024-10-21".to_string(),
            None,
        );
        let http = codec.encode_request(&CanonicalRequest::new("gpt-4o")).unwrap();
        assert_eq!(
            http.url,
            "https://r.openai.azure.com/openai/deployments/gpt-4o/chat/completions?api-version=2024-10-21"
        );
    }
```

- [ ] **Step 3 — `registry.rs`: `AzureOpenAi` build branch.**

Add a match arm in `build` (after the `OpenAi` arm):
```rust
            ProviderKind::AzureOpenAi => {
                let key = self.api_key_for(profile);
                let auth = if key.is_empty() {
                    crate::auth::Auth::None
                } else {
                    crate::auth::Auth::Header {
                        name: "api-key".to_string(),
                        value: key,
                    }
                };
                let authenticator = Arc::new(crate::authenticator::StaticAuth::new(auth))
                    as Arc<dyn crate::authenticator::Authenticator>;
                let codec = crate::openai::OpenAiCodec::new_azure(
                    profile.base_url.clone().unwrap_or_default(),
                    profile.azure_deployment.clone().unwrap_or_default(),
                    profile.azure_api_version.clone().unwrap_or_default(),
                    profile.reasoning_effort,
                );
                let id = cost::ProviderId::OpenAICompatible {
                    name: name.to_string(),
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
Add a registry test that an Azure profile resolves:
```rust
    #[test]
    fn azure_profile_resolves() {
        let mut extra = BTreeMap::new();
        extra.insert(
            "azure".to_string(),
            ProviderProfile {
                kind: ProviderKind::AzureOpenAi,
                base_url: Some("https://r.openai.azure.com".to_string()),
                api_key_env: Some("AZURE_OPENAI_KEY".to_string()),
                reasoning_effort: None,
                thinking_budget: None,
                azure_deployment: Some("gpt-4o".to_string()),
                azure_api_version: Some("2024-10-21".to_string()),
            },
        );
        let r = registry(extra);
        let resolved = r.resolve("azure/gpt-4o").expect("azure resolves");
        assert_eq!(resolved.model, "gpt-4o");
        assert_eq!(
            resolved.provider.id(),
            cost::ProviderId::OpenAICompatible { name: "azure".to_string() }
        );
    }
```
(Also update any other `ProviderProfile { .. }` literal in the crate's tests to include the two new Azure fields = `None`, if the compiler flags them.)

- [ ] **Step 4 — gates + commit.**
```bash
cargo test -p providers
cargo clippy -p providers --no-deps --all-targets -- -D warnings
git add lingxi-code/providers/src/profile.rs lingxi-code/providers/src/openai/ lingxi-code/providers/src/registry.rs
git commit -m "feat(llm-v2 P4): Azure OpenAI provider (deployment URL + api-key header)"
```
Expected: all pass; the existing OpenAI URL test still passes (UrlStyle::OpenAi unchanged).

---

## Task B: Phase gates + tag

- [ ] **Step 1:** `cargo test -p providers -p orchestrator -p test-harness` — all pass.
- [ ] **Step 2:** `cargo build --workspace` — Finished.
- [ ] **Step 3:** `cargo clippy -p providers --no-deps --all-targets -- -D warnings` — clean. (A full-workspace `-D warnings` clippy trips the pre-existing frozen-`traits` `doc_markdown` lint — out of scope; `--no-deps` isolates providers' own code.)
- [ ] **Step 4:** from `lingxi-code/`: `bash scripts/check-deps.sh` — OK 73 (no new deps).
- [ ] **Step 5:** `git tag -a llm-v2-p4 -m "LLM Providers v2 P4: Azure OpenAI"`.

---

## Self-Review

**Spec coverage (§3.3):** `ProviderKind::AzureOpenAi` (A1) ✓; profile `azureDeployment`/`azureApiVersion` + parse (A1) ✓; `UrlStyle::Azure` URL `{base}/openai/deployments/{deployment}/chat/completions?api-version=…` (A2) ✓; `api-key` header auth (A3) ✓; registry build branch (A3) ✓. Cost via `OpenAICompatible{name}` — bounded deviation documented.

**Placeholder scan:** all steps show full code + exact assertions. **Blast-radius:** `new` signature is UNCHANGED (only `new_azure` is added), so external `OpenAiCodec::new` callers (incl. the test-harness parity test) are unaffected — confirmed by keeping the existing `new(base_url, reasoning_effort)` signature.

**Type consistency:** `UrlStyle` + `new_azure(String, String, String, Option<ReasoningEffort>)` consistent between `openai/mod.rs` and the registry call. `ProviderProfile`'s new Azure fields consistent across profile.rs, parse_profiles, builtin_profiles, and registry test literals.
