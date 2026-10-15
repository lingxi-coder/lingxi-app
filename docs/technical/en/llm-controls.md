# Output, inference & cache controls

Apply validated request-local contracts without weakening schemas or confusing reasoning budgets with monetary limits.

<a id="inference-controls"></a>

## Thinking, effort and service tier

ThinkingConfig contains optional mode (Disabled/Enabled/Adaptive), budget (Tokens/Dynamic) and effort. ServiceTier independently selects Standard/Fast. The selected model row and profile’s declared wire mapping determine which combinations can be encoded; unsupported explicit controls fail admission. Missing metadata means unknown, not support.

| Control | Meaning | Accounting implication |
| --- | --- | --- |
| ThinkingBudget::Tokens | Provider reasoning-token budget or target, with declared min/max/hard_limit facts | Not a currency budget; host still enforces spend admission |
| ReasoningEffort | Explicit supported level under the active mode | Can change token consumption; does not itself change unit prices |
| ServiceTier::Fast | Endpoint/model-specific fast wire option | Keep requested and executed tiers distinct; unknown executed tier cannot establish a Fast price |

InferenceFeatures overlays normalize mode-incompatible defaults. A request-local observed fast_capability affects the selected primary model only, not shared configuration or fallback accounts. InferenceReport retains provider-reported execution facts.

<a id="structured-output"></a>

## Strict output and local validation

OutputFormat is Text, JsonObject or JsonSchema { name, schema, strict }. It is separate from strict function-tool inputs. Encoders preserve schema constraints and reject unsupported/conflicting options; they do not rewrite schemas to make a call succeed. Local schema validation disables external network/file resolution. Decode only a completed response; refusal, truncation, tools and pause are separate outcomes.

- Stream collectors reconstruct terminal content and then validate; individual JSON deltas are not standalone documents. Validation errors retain response/events and usage for host accounting.
- First-party Qwen and DeepSeek JSON Object requests require JSON/json in explicit system/user text. Their checks do not count assistant text, tool descriptions or attachment payloads. Strict schema support is admitted per model/route rather than protocol compatibility.

### Complete and validate the same output contract

```rust
use lingxi_llm_client::{LlmClient, RequestOptions};
use lingxi_llm_client::protocol::{ChatRequest, ConversationMessage, OutputFormat};
use serde_json::{json, Value};

pub async fn structured_answer(
    client: &LlmClient,
    options: &RequestOptions,
) -> Result<Value, Box<dyn std::error::Error>> {
    let mut request = ChatRequest::new("gpt-6-luna");
    request.output_format = OutputFormat::JsonSchema {
        name: "answer".into(), strict: true,
        schema: json!({"type":"object","properties":{"answer":{"type":"string"}},
            "required":["answer"],"additionalProperties":false}),
    };
    request.messages.push(ConversationMessage::user_text("Give a short greeting."));
    let response = client.chat().complete_in("openai", &request, options).await?;
    Ok(response.structured_json(&request.output_format)?)
}
```

Verification method: Compile check (llm-structured-output)

<a id="finite-decisions"></a>

## Finite single-choice decisions

DecisionRequest shares text/image context across one or more questions, each with unique nonempty question IDs and option IDs. DecisionService uses verified strict structured-output routes. models() lists admitted models; an arbitrary catalog structured_output flag, compatible gateway or account is insufficient.

### One finite-choice question with retained call report

```rust
use lingxi_llm_client::{LlmClient, RequestOptions};
use lingxi_llm_client::protocol::{DecisionContextPart, DecisionOption, DecisionQuestion, DecisionRequest, DecisionResult};

pub async fn classify(
    client: &LlmClient,
    options: &RequestOptions,
) -> Result<DecisionResult, Box<dyn std::error::Error>> {
    let request = DecisionRequest {
        model: "gpt-6-luna".into(),
        context: vec![DecisionContextPart::Text { text: "The build completed successfully.".into() }],
        questions: vec![DecisionQuestion {
            id: "build_status".into(), prompt: "What is the build status?".into(),
            options: vec![
                DecisionOption { id: "passed".into(), label: "Passed".into() },
                DecisionOption { id: "failed".into(), label: "Failed".into() },
            ],
        }],
    };
    Ok(client.decisions().decide_in("openai", &request, options).await?)
}
```

Verification method: Compile check (llm-decision)

Refused, Incomplete, InvalidResult and ProviderWithReport retain call reports where available. Answers must be offered IDs for every original question; invalid completed results do not weaken constraints or fail over. report.usage is the final attempt; prior_attempts retains earlier reported usage separately.

<a id="caches-and-embeddings"></a>

## Prompt caches, response caches and embeddings

ChatRequest.prompt_cache contains TTL and explicit original-request breakpoints; an empty policy preserves provider defaults. Native OpenAI Responses cache controls, Gemini context-cache resources and OpenRouter gateway response-cache policy have separate contracts. A response-cache HIT/MISS observation comes from known response headers, never from zero usage. Do not treat cache support on one route as permission to encode it on another.

client.embeddings().embed uses an independent configured route with ServiceSetting inheritance/enable/disable semantics. Text vectors remain in input order; invalid indices/counts, non-finite values and inconsistent dimensions are errors. The client does not split, normalize, truncate, retry or use the Chat model catalog to reinterpret embedding model IDs. Multimodal Gemini embedding is a separate typed native API.

- Local tokenizer estimates do no credential/network/remote-file resolution and explicitly list unsupported/omitted content. They estimate the preferred route only and are never authoritative billing usage.
- structured_output, decisions, inference_controls and embeddings tests validate mock wire contracts and error retention. Strict-output accuracy, live cache hit rates and provider-model entitlements require live-account evidence.

## Sources and documentation baseline

[Source repository](https://github.com/lingxi-coder/llm-client)

- [llm-client/src/protocol/inference.rs](https://github.com/lingxi-coder/llm-client/blob/7333dbc3795b6bad893cb7287088d80931538630/src/protocol/inference.rs) — Committed source (7333dbc3795b6bad893cb7287088d80931538630)
- [llm-client/src/protocol/request_controls.rs](https://github.com/lingxi-coder/llm-client/blob/7333dbc3795b6bad893cb7287088d80931538630/src/protocol/request_controls.rs) — Committed source (7333dbc3795b6bad893cb7287088d80931538630)
- [llm-client/src/protocol/structured.rs](https://github.com/lingxi-coder/llm-client/blob/7333dbc3795b6bad893cb7287088d80931538630/src/protocol/structured.rs) — Committed source (7333dbc3795b6bad893cb7287088d80931538630)
- [llm-client/src/codecs/inference.rs](https://github.com/lingxi-coder/llm-client/blob/7333dbc3795b6bad893cb7287088d80931538630/src/codecs/inference.rs) — Committed source (7333dbc3795b6bad893cb7287088d80931538630)
- [llm-client/src/codecs/structured.rs](https://github.com/lingxi-coder/llm-client/blob/7333dbc3795b6bad893cb7287088d80931538630/src/codecs/structured.rs) — Committed source (7333dbc3795b6bad893cb7287088d80931538630)
- [llm-client/src/codecs/cache.rs](https://github.com/lingxi-coder/llm-client/blob/7333dbc3795b6bad893cb7287088d80931538630/src/codecs/cache.rs) — Committed source (7333dbc3795b6bad893cb7287088d80931538630)
- [llm-client/src/client/decisions.rs](https://github.com/lingxi-coder/llm-client/blob/7333dbc3795b6bad893cb7287088d80931538630/src/client/decisions.rs) — Committed source (7333dbc3795b6bad893cb7287088d80931538630)
- [llm-client/src/protocol/decision.rs](https://github.com/lingxi-coder/llm-client/blob/7333dbc3795b6bad893cb7287088d80931538630/src/protocol/decision.rs) — Committed source (7333dbc3795b6bad893cb7287088d80931538630)
- [llm-client/src/embeddings/mod.rs](https://github.com/lingxi-coder/llm-client/blob/7333dbc3795b6bad893cb7287088d80931538630/src/embeddings/mod.rs) — Committed source (7333dbc3795b6bad893cb7287088d80931538630)
- [llm-client/src/token_count/request.rs](https://github.com/lingxi-coder/llm-client/blob/7333dbc3795b6bad893cb7287088d80931538630/src/token_count/request.rs) — Committed source (7333dbc3795b6bad893cb7287088d80931538630)
- [llm-client/src/providers/openrouter/response_cache.rs](https://github.com/lingxi-coder/llm-client/blob/7333dbc3795b6bad893cb7287088d80931538630/src/providers/openrouter/response_cache.rs) — Committed source (7333dbc3795b6bad893cb7287088d80931538630)
- [llm-client/tests/structured_output.rs](https://github.com/lingxi-coder/llm-client/blob/7333dbc3795b6bad893cb7287088d80931538630/tests/structured_output.rs) — Committed source (7333dbc3795b6bad893cb7287088d80931538630)
- [llm-client/tests/decisions.rs](https://github.com/lingxi-coder/llm-client/blob/7333dbc3795b6bad893cb7287088d80931538630/tests/decisions.rs) — Committed source (7333dbc3795b6bad893cb7287088d80931538630)
- [llm-client/tests/embeddings.rs](https://github.com/lingxi-coder/llm-client/blob/7333dbc3795b6bad893cb7287088d80931538630/tests/embeddings.rs) — Committed source (7333dbc3795b6bad893cb7287088d80931538630)

- llm-client: HEAD 7333dbc3795b6bad893cb7287088d80931538630; Referenced input fingerprint sha256:51afdece5bf1e2d135f676f447d6ff43c84aaa464328f96ba19fa1e5010928af; Product pin 7333dbc3795b6bad893cb7287088d80931538630

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
