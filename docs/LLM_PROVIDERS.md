# LLM Providers

LingXi defaults to Anthropic but can use any of several providers as the
main-loop model backend. Selection is by a `provider/model` string; a bare
string (or any `claude-*`) stays on Anthropic, so existing configs are
unchanged.

## Built-in providers

| Prefix | Provider | API key env | Notes |
|---|---|---|---|
| `anthropic/` (or bare / `claude-*`) | Anthropic | `ANTHROPIC_API_KEY` | Default; full feature parity |
| `openai/` | OpenAI | `OPENAI_API_KEY` | OpenAI Chat Completions |
| `gemini/` | Google Gemini | `GEMINI_API_KEY` | `generateContent` |

Examples:

```bash
# OpenAI
OPENAI_API_KEY=sk-... cargo run -p cli -- --model openai/gpt-4o
# Gemini
GEMINI_API_KEY=... cargo run -p cli -- --model gemini/gemini-2.0-flash
# Anthropic (default — unchanged)
ANTHROPIC_API_KEY=sk-ant-... cargo run -p cli -- --model claude-opus-4-7
```

## OpenAI-compatible endpoints (Groq, Together, Ollama, vLLM, OpenRouter, …)

Declare a named profile in `settings.json` under `providers`. Each profile has
a `type` (`anthropic` | `openai` | `gemini`), an optional `baseUrl`, and an
optional `apiKeyEnv` (the env var holding the key; `null` for no auth). Select
it as `profilename/model`.

```jsonc
{
  "model": "groq/llama-3.3-70b",
  "providers": {
    "groq":   { "type": "openai", "baseUrl": "https://api.groq.com/openai/v1", "apiKeyEnv": "GROQ_API_KEY" },
    "ollama": { "type": "openai", "baseUrl": "http://localhost:11434/v1", "apiKeyEnv": null }
  }
}
```

## Reasoning models

Configure reasoning per provider profile:

- **OpenAI o-series** (o1 / o3 / o4-mini, reasoning GPT-5): set
  `"reasoningEffort": "low" | "medium" | "high"`. The codec then sends
  `reasoning_effort`, uses `max_completion_tokens` (instead of `max_tokens`),
  and omits `temperature` (which those models reject).
- **Gemini 2.5** (flash / pro thinking): set `"thinkingBudget": <tokens>` →
  `generationConfig.thinkingConfig` (with `includeThoughts`).
- **Reasoning traces** (OpenAI / DeepSeek-R1 `reasoning_content`, Gemini
  `thought` parts) decode into the canonical `Thinking` block — shown, never
  re-sent into request history.

```jsonc
{
  "providers": {
    "openai":  { "type": "openai", "apiKeyEnv": "OPENAI_API_KEY", "reasoningEffort": "high" },
    "gemini":  { "type": "gemini", "apiKeyEnv": "GEMINI_API_KEY", "thinkingBudget": 2048 }
  }
}
```

## Azure OpenAI

```jsonc
{
  "model": "azure/gpt-4o",
  "providers": {
    "azure": {
      "type": "azureOpenAi",
      "baseUrl": "https://my-resource.openai.azure.com",
      "azureDeployment": "gpt-4o",
      "azureApiVersion": "2024-10-21",
      "apiKeyEnv": "AZURE_OPENAI_KEY"
    }
  }
}
```

Azure uses the OpenAI chat body over
`…/openai/deployments/{deployment}/chat/completions?api-version=…` with an
`api-key` header.

**Azure AD (Entra ID) auth** is also supported: when `apiKeyEnv` yields no key
and an `azureAd` block is configured, a bearer token is minted via the OAuth2
client-credentials grant and cached until it expires.

```jsonc
"azure": {
  "type": "azureOpenAi",
  "baseUrl": "https://my-resource.openai.azure.com",
  "azureDeployment": "gpt-4o",
  "azureApiVersion": "2024-10-21",
  "azureAd": { "tenant": "<tenant-id>", "clientIdEnv": "AZURE_CLIENT_ID", "clientSecretEnv": "AZURE_CLIENT_SECRET" }
}
```

## Vertex AI (Gemini)

```jsonc
{
  "model": "vertex/gemini-2.5-pro",
  "providers": {
    "vertex": { "type": "vertex", "project": "my-gcp-project", "region": "us-central1" }
  }
}
```

Vertex sends the Gemini body to the regional `…-aiplatform.googleapis.com`
endpoint, authenticated with a GCP OAuth2 access token. Credentials are
discovered by `gcp_auth`: set `GOOGLE_APPLICATION_CREDENTIALS` to a
service-account key file, or rely on gcloud ADC / the metadata server.

## Bedrock (Claude)

```jsonc
{
  "model": "bedrock/anthropic.claude-3-5-sonnet-20241022-v2:0",
  "providers": {
    "bedrock": { "type": "bedrock", "region": "us-east-1" }
  }
}
```

Bedrock runs Claude via the Anthropic Messages body with AWS SigV4 signing.
Non-streaming turns use `InvokeModel`; **streaming turns use
`InvokeModelWithResponseStream`** and decode the AWS binary event-stream
incrementally (each chunk is mapped to a canonical streaming event), so Bedrock
streams token-by-token like the other providers.

**Credential discovery** follows a layered chain (no AWS SDK runtime — kept off
to preserve the Rust 1.82 toolchain): environment
(`AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY` / optional `AWS_SESSION_TOKEN`)
→ the shared credentials file (`~/.aws/credentials`, honoring `AWS_PROFILE` /
`AWS_SHARED_CREDENTIALS_FILE`) → a profile's `credential_process` (the common
way `aws configure sso` / `aws-vault` surface short-lived SSO credentials)
→ IMDSv2 (EC2/ECS instance-role credentials). Resolved credentials are cached
for 5 minutes. Native `sso_session` config (the cached OIDC token in
`~/.aws/sso/cache`) is not read directly — use `credential_process` or run
`aws sso login`.

## Routing — aliases, fallback, retry

A sibling `routing` block adds a thin router over the configured providers:

```jsonc
{
  "routing": {
    "aliases":  { "fast": "groq/llama-3.3-70b", "smart": "anthropic/claude-opus-4-7" },
    "fallback": { "smart": ["openai/gpt-4o", "gemini/gemini-2.0-flash"] },
    "retry":    { "maxAttempts": 3, "backoffMs": 250 }
  }
}
```

- **aliases** — friendly names resolving to a `provider/model`.
- **fallback** — on a *transient* error (429 / 5xx / rate-limit / unexpected
  stream end) the router advances to the next `provider/model`. A *terminal*
  error (bad request, auth, capability mismatch) is NOT failed over.
- **retry** — transient errors retry up to `maxAttempts` with linear backoff.
  Failover / retry decide on the initial request only (no mid-stream switch).

Fallback chains should be **capability-homogeneous**: the vision / tool-use
guardrails gate on the *primary* member's capabilities, so every member of a
chain should support the same modalities (e.g. don't mix a vision-capable
primary with a text-only fallback for an image request).

## Vision / image input

Image input is supported across all three built-in providers (Anthropic,
OpenAI, Gemini) via the **programmatic / API path**: place one or more
`ContentBlock::Image { source: ImageSource::Base64 { media_type, data } }` (or
`ImageSource::Url { url }`) blocks in a `ConversationMessage::User` `content`
vector alongside any `ContentBlock::Text` blocks. Each provider translates the
block to its native wire format automatically (`base64` for Anthropic/Gemini,
`image_url` for OpenAI).

**TUI paste-to-image** is wired end-to-end through the orchestrator: the M7-10
paste path records image file paths in `PasteState`; `PasteState::take_image_paths()`
drains them at submit time into `OrchestratorHandle::run_turn_streaming_with_images`,
which reads each file, detects the media type, base64-encodes it, and appends a
`ContentBlock::Image` to the outgoing user message (a read/type error aborts the
turn cleanly). The only remaining hookup is the TUI's streaming submit loop
calling `spawn_streaming_turn` (not yet bound to a key handler this milestone);
the orchestrator/protocol path and the path-extraction API are complete.

## Capabilities & limitations (v2)

- **Tool use** is native for Anthropic / OpenAI / Gemini (and the Azure /
  Vertex / Bedrock variants); the agentic loop works on each. A model whose
  provider lacks native function-calling is rejected when tools are present.
- **Vision** and **reasoning** (effort / thinking budget + trace decoding) are
  supported as described above.
- **Anthropic-only features** (prompt caching, server tools, citations) are
  omitted on other providers — never fabricated.
- **Bedrock streams** token-by-token via `InvokeModelWithResponseStream` (AWS
  event-stream decoded by a crate-local pure-Rust frame decoder — the official
  `aws-smithy-eventstream` floors `aws-smithy-types` at a rustc-1.88 version,
  incompatible with the pinned Rust 1.82 toolchain).
- **Signed-auth credential discovery** is layered: AWS (env → shared
  credentials file → `credential_process` → IMDSv2), GCP
  (`GOOGLE_APPLICATION_CREDENTIALS` / ADC / metadata), and Azure AD
  (client-credentials token). Native AWS `sso_session` config is read only via
  `credential_process` (run `aws sso login`).
- **Cost** is attributed per provider: Bedrock has its own reference price rows
  (`ProviderId::AmazonBedrock`); Vertex reuses Gemini and Azure reuses OpenAI
  list prices. Unpriced model ids still record zero cost rather than an
  invented rate.
- **`/model` lists the live configured set** — `provider/model` ids plus
  `@aliases` — via `OrchestratorApiClient::available_models()` (delegating to
  `ModelRouter`), falling back to a static example list only when no router is
  wired. Declare provider-local model ids with the `models` profile field
  (e.g. `"groq": { "type": "openAi", …, "models": ["llama-3.3-70b"] }`).
- **Remaining follow-up:** binding the TUI streaming submit loop to a key
  handler so pasted images flow from the prompt UI (the orchestrator/protocol
  path is complete).
