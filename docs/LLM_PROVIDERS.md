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

Bedrock runs Claude via `InvokeModel` (the Anthropic Messages body) with AWS
SigV4 signing. Set credentials via `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`,
and (optionally) `AWS_SESSION_TOKEN`. Bedrock is non-streaming — the full
response is re-emitted as a single-shot synthetic stream.

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

## Vision / image input

Image input is supported across all three built-in providers (Anthropic,
OpenAI, Gemini) via the **programmatic / API path**: place one or more
`ContentBlock::Image { source: ImageSource::Base64 { media_type, data } }` (or
`ImageSource::Url { url }`) blocks in a `ConversationMessage::User` `content`
vector alongside any `ContentBlock::Text` blocks. Each provider translates the
block to its native wire format automatically (`base64` for Anthropic/Gemini,
`image_url` for OpenAI).

**TUI paste-to-image wiring is a tracked follow-up.** The M7-10 paste path
records only a file-path `source` string per detected image in `PasteState`
(see `tui/src/components/prompt_input/image_paste.rs`); no bytes are read or
retained. At the submit site (`app.rs` `run_one_submit` / `dispatch
KeyAction::Submit`) only the plain prompt text string reaches the orchestrator
— the `paste` state is never consulted. Until the TUI wiring lands, image
references appear as `[Image #N]` placeholder text in submitted messages.

## Capabilities & limitations (v2)

- **Tool use** is native for Anthropic / OpenAI / Gemini (and the Azure /
  Vertex / Bedrock variants); the agentic loop works on each. A model whose
  provider lacks native function-calling is rejected when tools are present.
- **Vision** and **reasoning** (effort / thinking budget + trace decoding) are
  supported as described above.
- **Anthropic-only features** (prompt caching, server tools, citations) are
  omitted on other providers — never fabricated.
- **Documented limitations / follow-ups:**
  - **Bedrock is non-streaming** — it uses `InvokeModel` and re-emits the full
    response as a single-shot synthetic stream (the engine's HTTP transport
    exposes SSE / full-body, not AWS event-stream framing). Real token
    streaming for Bedrock is a follow-up.
  - **Signed-auth credentials** come from the environment (GCP
    `GOOGLE_APPLICATION_CREDENTIALS` / ADC; AWS env vars). SSO / IMDS / profile
    discovery and Azure AD tokens are follow-ups.
  - **Cost** is attributed per provider; Bedrock maps to Anthropic pricing and
    unpriced model ids record zero cost rather than an invented rate
    (Bedrock-specific rates are a follow-up).
  - **TUI paste-to-image** wiring is a tracked follow-up (see above; the API
    path works today).
  - **`/model` shows example model names** rather than the live configured set
    because the `ConversationOrchestrator` holds the API client as
    `Arc<dyn OrchestratorApiClient>` (no `available_models()` method), not as
    `Arc<dyn ModelRouter>`. The configured list is enumerable via
    `ModelRouter::available_models()` where the `ProviderRegistry` is in scope.
