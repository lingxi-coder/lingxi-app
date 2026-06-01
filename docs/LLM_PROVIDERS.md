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

## Capabilities & limitations (v1)

- **Tool use** is translated natively for all three providers (the agentic loop
  works on each). A model whose provider lacks native function-calling is
  rejected when tools are present, rather than silently degraded.
- **Anthropic-only features** (prompt caching, extended-thinking blocks, server
  tools, citations) are omitted on other providers — never fabricated.
- **Not yet supported:** image/vision input, reasoning-model parameters
  (`max_completion_tokens`), Azure OpenAI's deployment URL template, and
  Vertex/Bedrock signed auth. Cost is attributed per provider; unpriced models
  record zero cost rather than an invented rate.
