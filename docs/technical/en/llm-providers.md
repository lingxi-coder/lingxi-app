# Providers, catalogs & pricing

Bind profiles, filter by region, manage catalogs, access native resources, and estimate cost from observed facts.

<a id="profiles"></a>

## Provider profiles

A provider identifies a service. A profile identifies a connection: protocol, endpoint, models, authentication strategy, and optional native service routes. Built-in profiles cover OpenAI, Anthropic, Gemini, DeepSeek, Kimi, Qwen, MiniMax, GLM/Z.AI, xAI, OpenRouter, and GitHub Copilot.

The built-in wire families include OpenAI Responses, Chat Completions, Anthropic Messages, and Gemini. Azure, Bedrock, Vertex, and Foundry have adapters for configured profiles; an adapter is not a built-in account or endpoint.

- Use client.providers() for visible connections, client.chat().models() for chat models, and client.images().models() for image models.
- Choose Region::ChinaMainland or Region::International before building. Lists, resolution, and failover respect this region; custom profiles without a regions declaration permit both.
- Kimi, Qwen, and MiniMax regional profiles use distinct credentials. Match the profile to the account and key region; keys are not interchangeable.

CapabilitySupport::Unknown is distinct from Unsupported. Catalog metadata, implemented adapters, account permissions, and successful live calls establish different facts.

<a id="managed-catalog"></a>

## Manage persistent configuration

### Managed client · async-host excerpt / 托管客户端片段

```rust
use lingxi_llm_client::{builtin_providers, LlmClientBuilder};
use lingxi_llm_client::protocol::Region;

let profiles = builtin_providers()?.into_iter()
        .filter(|profile| profile.profile_name == "openai").collect::<Vec<_>>();
let (client, config) = LlmClientBuilder::new(&profiles)?
    .with_region(Region::International)
    .build_managed()?;
config.set_config_dir("./config").await?;
let snapshot = client.snapshot();
let profile = snapshot.profile("openai");
```

Verification method: API excerpt

### LlmClientBuilder::build_managed

```text
pub fn build_managed(self) -> Result<(LlmClient, ClientConfigManager), BuildError>
```

Return the shareable client and a separate configuration manager.

### ClientConfigManager::set_config_dir

```text
pub async fn set_config_dir(&self, path: impl AsRef<Path>) -> Result<(), ProviderStoreError>
```

Load and persist providers.json. Call again at startup to restore saved configuration.

### ClientConfigManager::add_provider

```text
pub async fn add_provider(&self, profile: ProviderProfile) -> Result<(), ProviderStoreError>
```

Add or replace a complete connection profile.

### ClientConfigManager::sync_provider

```text
pub async fn sync_provider(&self, name: &str, credential: Option<&Secret<String>>) -> Result<usize, ProviderStoreError>
```

Refresh one profile using its own account credential and supported directory API.

### ClientConfigManager::set_model_visibility

```text
pub async fn set_model_visibility(&self, name: &str, wire: &str, visible: bool) -> Result<(), ProviderStoreError>
```

Set visibility of the selected model row.

Configuration v3 rejects v1/v2 with UnsupportedVersion; it does not silently migrate them. Updates publish a new snapshot for future operations. In-flight calls keep their captured configuration.

<a id="native-services"></a>

## Provider-native resources

Use client.provider::<T>(profile) to bind an exact connection to its provider client. Public clients include OpenAiClient, AnthropicClient, GoogleClient, DeepSeekClient, KimiClient, QwenClient, MiniMaxClient, ZhipuClient, XaiClient, OpenRouterClient, and GithubCopilotClient.

Native resources keep their own types and support boundaries: files, retrieval, batches, background jobs, remote tools, speech, and realtime sessions vary by provider. Use the generated current Rustdoc for complete resource types and signatures, and the provider resources chapter for scope and lifecycle constraints. Realtime audio device capture and playback remain in the host.

### Provider binding · excerpt with an existing client / 供应商绑定片段

```rust
use lingxi_llm_client::providers::OpenAiClient;

let openai = client.provider::<OpenAiClient>("openai")?;
let audio = openai.audio();
let batches = openai.batches();
let retrieval = openai.retrieval();
let background = openai.background();
```

Verification method: API excerpt

### LlmClient::provider

```text
pub fn provider<T: ProviderClient>(&self, profile_name: &str) -> Result<T, ProviderBindingError>
```

Reject a mismatched provider/profile binding rather than inferring another route.

### EmbeddingService::embed

```text
pub async fn embed(self, profile_name: &str, input: &EmbeddingRequest, options: &RequestOptions) -> Result<EmbeddingResponse, EmbeddingError>
```

Embed text using an enabled independent route. Model limits, dimensions and task support are validated without automatic splitting.

<a id="pricing"></a>

## Usage & frozen prices

Keep the executed profile, UsageReport and InferenceReport together. FrozenPricing captures the selected model row before dispatch, so later catalog updates cannot alter an attempt’s prices. quote() supports admission estimates; estimate() requires complete valid usage and confirmed execution facts.

### FrozenPricing::capture

```text
pub fn capture(profile: &ProviderProfile, display_model: &str, request_model: &str) -> Result<Self, LlmError>
```

Capture an unambiguous exact model row from configuration.

### FrozenPricing::quote

```text
pub fn quote(&self, context: &PricingContext) -> Result<PriceQuote, LlmError>
```

Quote selected rates without claiming actual usage.

### FrozenPricing::estimate

```text
pub fn estimate(&self, report: &UsageReport, inference: &InferenceReport, submission: Submission) -> Result<CostEstimate, LlmError>
```

Estimate the observed attempt with frozen rates and retained currency. Partial usage, unpublished prices, or an unknown executed Fast tier return CostUnavailable.

### LlmClient::estimate_local_tokens_in

```text
pub fn estimate_local_tokens_in(&self, profile: &str, request: &ChatRequest) -> Result<LocalTokenEstimate, LocalTokenCountError>
```

Estimate visible input offline using enabled mapped tokenizers; omissions are explicit and are not provider billing usage.

Reasoning effort changes token consumption, not the declared unit rate. Catalogs and cost estimates do not establish live provider availability, a final invoice, or a host accounting ledger.

<a id="authentication-hosting"></a>

## Authentication and hosting boundaries

A provider-compatible gateway does not become the official provider just because it shares a protocol. Exact endpoint and provider identity checks gate native resources, decisions, hosted tools and account APIs. Builder codec support and model metadata establish local capabilities; account authorization and regional endpoint access remain independent.

| Route/auth class | Host responsibility | SDK responsibility |
| --- | --- | --- |
| API key / bearer | Select account and refresh/store valid secret | Encode configured headers once for the selected route |
| Restricted ChatGPT / Copilot routes | Provide entitled account token and grant state | Enforce allowed route/resource policy; no generic API-key equivalence |
| Bedrock AWS SigV4 | Provide credential and exact signing configuration | Encode hosted protocol and authenticate final bytes |
| Vertex / Azure / Foundry | Provide project/location or deployment/resource and appropriate credential | Use the declared wrapper; native services opt in separately |

<a id="catalog-provenance"></a>

## Catalog provenance and verification

The checked-in provider TOML and capability matrices describe current implemented/admitted contracts, with provider/model/protocol-specific supported, unsupported or unknown facts. Directory sync can add observed models but does not invent their prices or native capabilities. A model removed from the static catalog can retain observed identity without retaining withdrawn static pricing.

- Record selected profile, wire model, source baseline and account region with validation results. Compile/mock tests prove public API/wire behavior; a live request separately proves that one account accepted one operation at that time.
- A full baseline mismatch between local SDK HEAD and the product’s dependency revision is visible in the handbook source data. Keep local path consumer verification separate from Git-pinned installation verification; do not edit production manifests or locks to make the examples compile.

- [Current protocol feature matrix source](https://github.com/lingxi-coder/llm-client)
- [Per-attempt usage and account query distinctions](llm-execution-accounting.md)

## Sources and documentation baseline

[Source repository](https://github.com/lingxi-coder/llm-client)

- [llm-client/src/client/store.rs](https://github.com/lingxi-coder/llm-client/blob/46240a65df198a654d7d6de1c0bd99c23fcbb979/src/client/store.rs) — Committed source (46240a65df198a654d7d6de1c0bd99c23fcbb979)
- [llm-client/src/client/resolve.rs](https://github.com/lingxi-coder/llm-client/blob/46240a65df198a654d7d6de1c0bd99c23fcbb979/src/client/resolve.rs) — Committed source (46240a65df198a654d7d6de1c0bd99c23fcbb979)
- [llm-client/src/configuration/model.rs](https://github.com/lingxi-coder/llm-client/blob/46240a65df198a654d7d6de1c0bd99c23fcbb979/src/configuration/model.rs) — Committed source (46240a65df198a654d7d6de1c0bd99c23fcbb979)
- [llm-client/src/configuration/sync.rs](https://github.com/lingxi-coder/llm-client/blob/46240a65df198a654d7d6de1c0bd99c23fcbb979/src/configuration/sync.rs) — Committed source (46240a65df198a654d7d6de1c0bd99c23fcbb979)
- [llm-client/src/protocol/provider.rs](https://github.com/lingxi-coder/llm-client/blob/46240a65df198a654d7d6de1c0bd99c23fcbb979/src/protocol/provider.rs) — Committed source (46240a65df198a654d7d6de1c0bd99c23fcbb979)
- [llm-client/src/client/frozen_pricing.rs](https://github.com/lingxi-coder/llm-client/blob/46240a65df198a654d7d6de1c0bd99c23fcbb979/src/client/frozen_pricing.rs) — Committed source (46240a65df198a654d7d6de1c0bd99c23fcbb979)
- [llm-client/src/token_count/request.rs](https://github.com/lingxi-coder/llm-client/blob/46240a65df198a654d7d6de1c0bd99c23fcbb979/src/token_count/request.rs) — Committed source (46240a65df198a654d7d6de1c0bd99c23fcbb979)
- [llm-client/data/capability-matrix-protocols.json](https://github.com/lingxi-coder/llm-client/blob/46240a65df198a654d7d6de1c0bd99c23fcbb979/data/capability-matrix-protocols.json) — Committed source (46240a65df198a654d7d6de1c0bd99c23fcbb979)
- [llm-client/data/capability-audit.json](https://github.com/lingxi-coder/llm-client/blob/46240a65df198a654d7d6de1c0bd99c23fcbb979/data/capability-audit.json) — Committed source (46240a65df198a654d7d6de1c0bd99c23fcbb979)
- [llm-client/data/providers/openai.toml](https://github.com/lingxi-coder/llm-client/blob/46240a65df198a654d7d6de1c0bd99c23fcbb979/data/providers/openai.toml) — Committed source (46240a65df198a654d7d6de1c0bd99c23fcbb979)
- [llm-client/data/providers/anthropic.toml](https://github.com/lingxi-coder/llm-client/blob/46240a65df198a654d7d6de1c0bd99c23fcbb979/data/providers/anthropic.toml) — Committed source (46240a65df198a654d7d6de1c0bd99c23fcbb979)
- [llm-client/data/providers/qwen.toml](https://github.com/lingxi-coder/llm-client/blob/46240a65df198a654d7d6de1c0bd99c23fcbb979/data/providers/qwen.toml) — Committed source (46240a65df198a654d7d6de1c0bd99c23fcbb979)
- [llm-client/tests/regions.rs](https://github.com/lingxi-coder/llm-client/blob/46240a65df198a654d7d6de1c0bd99c23fcbb979/tests/regions.rs) — Committed source (46240a65df198a654d7d6de1c0bd99c23fcbb979)

- llm-client: HEAD 46240a65df198a654d7d6de1c0bd99c23fcbb979; Referenced input fingerprint sha256:548db437ccaaeec3ce6ad3beb931ef868b263f6ad9b3c47309a3e00c514326dd; Product pin 46240a65df198a654d7d6de1c0bd99c23fcbb979

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
