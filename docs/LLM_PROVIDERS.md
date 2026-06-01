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

## Capabilities & limitations (v1)

- **Tool use** is translated natively for all three providers (the agentic loop
  works on each). A model whose provider lacks native function-calling is
  rejected when tools are present, rather than silently degraded.
- **Anthropic-only features** (prompt caching, extended-thinking blocks, server
  tools, citations) are omitted on other providers — never fabricated.
- **Not yet supported:** reasoning-model parameters (`max_completion_tokens`),
  Azure OpenAI's deployment URL template, and Vertex/Bedrock signed auth. Cost
  is attributed per provider; unpriced models record zero cost rather than an
  invented rate.
