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

<a id="output-contract-choice"></a>

## Choose the smallest output contract

Use JsonPayload::parse for an exact JSON schema source, or from_generated_value for a small schema built by your own configuration UI. Provider JSON and partial tool arguments should remain source-carrying data until a completed, validated application boundary.

| Contract | Use | What to validate |
| --- | --- | --- |
| OutputFormat::Text | Human-readable prose | Stop reason and application-specific content requirements |
| OutputFormat::JsonObject | A JSON object whose fields the application checks | Object root, required application fields and valid value types |
| OutputFormat::JsonSchema | A typed object/array contract admitted by the route | Original schema, final response, schema match and target-type deserialization |
| ToolSpec.strict | Constrain function-tool input | Tool arguments separately from the final-answer output contract |

<a id="structured-result-errors"></a>

## Recover from structured result failures

| StructuredOutputErrorKind | Meaning | Action |
| --- | --- | --- |
| Incomplete | Not a final completed answer, or response still contains tool use | Handle refusal/truncation/tool flow before parsing application data |
| NoContract | The request used the text contract | Set the intended JSON output_format explicitly |
| InvalidJson / SchemaMismatch | Final output is invalid JSON or violates the requested schema | Retain the response and usage; reject it as application data |
| InvalidSchema / Deserialization | Local schema or target Rust type cannot accept the value | Fix the application contract/type rather than weakening provider constraints silently |

StructuredOutputError retains the response. StructuredStreamError also retains observed events for read failure, missing End, size limits and validation failure. Logging a content error must not discard the associated usage facts.

<a id="schema-design-recipe"></a>

## Make a schema usable by the application

1. Start from the object the application consumes: stable field names, explicit types and only the required fields. Keep any enum values aligned with the application’s actual accepted IDs.
2. Choose a route with the required strict-schema support before sending. A compatible protocol or a model name alone does not establish support for every schema keyword.
3. Pass the same OutputFormat to the request and structured_json/structured. For a stream, collect the complete response first; do not parse each text delta as JSON.
4. Keep validation failure separate from transport failure and retry policy. A complete but invalid business result is not a reason to bypass the contract or automatically switch accounts.

<a id="structured-output"></a>

## Strict output and local validation

OutputFormat is Text, JsonObject or JsonSchema { name, schema, strict }. It is separate from strict function-tool inputs. Encoders preserve schema constraints and reject unsupported/conflicting options; they do not rewrite schemas to make a call succeed. Local schema validation disables external network/file resolution. Decode only a completed response; refusal, truncation, tools and pause are separate outcomes.

- Stream collectors reconstruct terminal content and then validate; individual JSON deltas are not standalone documents. Validation errors retain response/events and usage for host accounting.
- First-party Qwen and DeepSeek JSON Object requests require JSON/json in explicit system/user text. Their checks do not count assistant text, tool descriptions or attachment payloads. Strict schema support is admitted per model/route rather than protocol compatibility.

### Complete and validate the same output contract

```rust
use lingxi_llm_client::{LlmClient, RequestOptions};
use lingxi_llm_client::protocol::{ChatRequest, ConversationMessage, JsonPayload, OutputFormat};

pub async fn structured_answer(
    client: &LlmClient,
    options: &RequestOptions,
) -> Result<JsonPayload, Box<dyn std::error::Error>> {
    let mut request = ChatRequest::new("gpt-6-luna");
    request.output_format = OutputFormat::JsonSchema {
        name: "answer".into(), strict: true,
        schema: JsonPayload::parse(r#"{"type":"object","properties":{"answer":{"type":"string"}},
            "required":["answer"],"additionalProperties":false}"#)?,
    };
    request.messages.push(ConversationMessage::user_text("Give a short greeting."));
    let response = client.chat().complete_in("openai", &request, options).await?;
    Ok(response.structured_json(&request.output_format)?)
}
```

Verification method: Compile check (llm-structured-output)

<a id="finite-decisions"></a>

## Provider-native decisions

Use the native resource for the configured protocol. OpenAI Decisions accepts input and named questions. TypeSafe SystemOne accepts state, questions and optional assets. The host selects the endpoint and passes credentials in RequestOptions.

### Ask a predicate with a configured Decisions resource

```rust
use lingxi_llm_client::providers::openai::decisions::{CreateRequest, Decisions};
use lingxi_llm_client::RequestOptions;
use serde_json::{json, Value};

pub async fn classify(
    service: &Decisions<'_>,
    model: &str,
    evidence: &str,
    options: &RequestOptions,
) -> Result<Value, Box<dyn std::error::Error>> {
    let request = CreateRequest {
        model: model.into(),
        input: json!(evidence),
        questions: vec![json!({
            "type": "predicate", "name": "passed", "instructions": "Did the build pass?"
        })],
    };
    Ok(service.create(&request, options).await?)
}
```

Verification method: Compile check (llm-decision)

- [Configure auto-reply for a shared Bot](teams.md#auto-reply)

<a id="caches-and-embeddings"></a>

## Prompt caches, response caches and embeddings

ChatRequest.prompt_cache contains TTL and explicit original-request breakpoints; an empty policy preserves provider defaults. Native OpenAI Responses cache controls, Gemini context-cache resources and OpenRouter gateway response-cache policy have separate contracts. A response-cache HIT/MISS observation comes from known response headers, never from zero usage. Do not treat cache support on one route as permission to encode it on another.

client.embeddings().embed uses an independent configured route with ServiceSetting inheritance/enable/disable semantics. Text vectors remain in input order; invalid indices/counts, non-finite values and inconsistent dimensions are errors. The client does not split, normalize, truncate, retry or use the Chat model catalog to reinterpret embedding model IDs. Multimodal Gemini embedding is a separate typed native API.

- Local tokenizer estimates do no credential/network/remote-file resolution and explicitly list unsupported/omitted content. They estimate the preferred route only and are never authoritative billing usage.
- structured_output, decisions, inference_controls and embeddings tests validate mock wire contracts and error retention. Strict-output accuracy, live cache hit rates and provider-model entitlements require live-account evidence.

## Sources and documentation baseline

[Source repository](https://github.com/lingxi-coder/llm-client)

- [llm-client/src/protocol/inference.rs](https://github.com/lingxi-coder/llm-client/blob/63c8f7663b7cba7466538213f86c6df4a30442a9/src/protocol/inference.rs) — Committed source (63c8f7663b7cba7466538213f86c6df4a30442a9)
- [llm-client/src/protocol/request_controls.rs](https://github.com/lingxi-coder/llm-client/blob/63c8f7663b7cba7466538213f86c6df4a30442a9/src/protocol/request_controls.rs) — Committed source (63c8f7663b7cba7466538213f86c6df4a30442a9)
- [llm-client/src/protocol/structured.rs](https://github.com/lingxi-coder/llm-client/blob/63c8f7663b7cba7466538213f86c6df4a30442a9/src/protocol/structured.rs) — Committed source (63c8f7663b7cba7466538213f86c6df4a30442a9)
- [llm-client/src/codecs/inference.rs](https://github.com/lingxi-coder/llm-client/blob/63c8f7663b7cba7466538213f86c6df4a30442a9/src/codecs/inference.rs) — Committed source (63c8f7663b7cba7466538213f86c6df4a30442a9)
- [llm-client/src/codecs/structured.rs](https://github.com/lingxi-coder/llm-client/blob/63c8f7663b7cba7466538213f86c6df4a30442a9/src/codecs/structured.rs) — Committed source (63c8f7663b7cba7466538213f86c6df4a30442a9)
- [llm-client/src/codecs/cache.rs](https://github.com/lingxi-coder/llm-client/blob/63c8f7663b7cba7466538213f86c6df4a30442a9/src/codecs/cache.rs) — Committed source (63c8f7663b7cba7466538213f86c6df4a30442a9)
- [llm-client/src/providers/openai/decisions.rs](https://github.com/lingxi-coder/llm-client/blob/63c8f7663b7cba7466538213f86c6df4a30442a9/src/providers/openai/decisions.rs) — Committed source (63c8f7663b7cba7466538213f86c6df4a30442a9)
- [llm-client/src/providers/native_decision.rs](https://github.com/lingxi-coder/llm-client/blob/63c8f7663b7cba7466538213f86c6df4a30442a9/src/providers/native_decision.rs) — Committed source (63c8f7663b7cba7466538213f86c6df4a30442a9)
- [llm-client/src/embeddings/mod.rs](https://github.com/lingxi-coder/llm-client/blob/63c8f7663b7cba7466538213f86c6df4a30442a9/src/embeddings/mod.rs) — Committed source (63c8f7663b7cba7466538213f86c6df4a30442a9)
- [llm-client/src/token_count/request.rs](https://github.com/lingxi-coder/llm-client/blob/63c8f7663b7cba7466538213f86c6df4a30442a9/src/token_count/request.rs) — Committed source (63c8f7663b7cba7466538213f86c6df4a30442a9)
- [llm-client/src/providers/openrouter/response_cache.rs](https://github.com/lingxi-coder/llm-client/blob/63c8f7663b7cba7466538213f86c6df4a30442a9/src/providers/openrouter/response_cache.rs) — Committed source (63c8f7663b7cba7466538213f86c6df4a30442a9)
- [llm-client/tests/structured_output.rs](https://github.com/lingxi-coder/llm-client/blob/63c8f7663b7cba7466538213f86c6df4a30442a9/tests/structured_output.rs) — Committed source (63c8f7663b7cba7466538213f86c6df4a30442a9)
- [llm-client/tests/native_decisions.rs](https://github.com/lingxi-coder/llm-client/blob/63c8f7663b7cba7466538213f86c6df4a30442a9/tests/native_decisions.rs) — Committed source (63c8f7663b7cba7466538213f86c6df4a30442a9)
- [llm-client/tests/embeddings.rs](https://github.com/lingxi-coder/llm-client/blob/63c8f7663b7cba7466538213f86c6df4a30442a9/tests/embeddings.rs) — Committed source (63c8f7663b7cba7466538213f86c6df4a30442a9)
- [llm-client/src/protocol/json_payload.rs](https://github.com/lingxi-coder/llm-client/blob/63c8f7663b7cba7466538213f86c6df4a30442a9/src/protocol/json_payload.rs) — Committed source (63c8f7663b7cba7466538213f86c6df4a30442a9)
- [llm-client/src/client/stream.rs](https://github.com/lingxi-coder/llm-client/blob/63c8f7663b7cba7466538213f86c6df4a30442a9/src/client/stream.rs) — Committed source (63c8f7663b7cba7466538213f86c6df4a30442a9)

- llm-client: HEAD 63c8f7663b7cba7466538213f86c6df4a30442a9; Referenced input fingerprint sha256:2d8d308d351d3f94a029886f11db1bf931d3b28dd76d93e0f64e50665160e636; Product pin 63c8f7663b7cba7466538213f86c6df4a30442a9

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
