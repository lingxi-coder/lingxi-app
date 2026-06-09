use crate::{ContentBlock, LlmError, LlmRequest, LlmResponse, NoopStreamDecoder, ProviderRequest, ProviderResponse, StreamDecoder, ToolDeclaration, Usage, WireCodec};

use serde_json::Value;

#[derive(Debug, Clone)]
#[allow(missing_docs)]
pub struct GeminiCodec {
    base_url: String,
}

impl GeminiCodec {
    #[must_use]
    #[allow(missing_docs)]
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
        }
    }

    fn generate_content_url(&self, model: &str) -> String {
        format!(
            "{}/models/{}:generateContent",
            self.base_url.trim_end_matches('/'),
            model
        )
    }
}

impl WireCodec for GeminiCodec {
    fn encode_request(&self, request: &LlmRequest) -> Result<ProviderRequest, LlmError> {
        let mut body = serde_json::Map::new();
        body.insert("contents".to_string(), Value::Array(encode_messages(&request.messages)));

        if let Some(system) = &request.system {
            body.insert(
                "systemInstruction".to_string(),
                serde_json::json!({"parts": [{"text": system}]}),
            );
        }

        if !request.tools.is_empty() {
            body.insert("tools".to_string(), serde_json::json!([{ "functionDeclarations": request.tools.iter().map(encode_tool).collect::<Vec<_>>() }]));
        }

        let mut provider_request = ProviderRequest::post_json(
            self.generate_content_url(&request.model),
            Value::Object(body),
        );
        provider_request
            .headers
            .insert("content-type".to_string(), "application/json".to_string());

        Ok(provider_request)
    }

    fn decode_response(&self, response: ProviderResponse) -> Result<LlmResponse, LlmError> {
        decode_response_body(response.body_json)
    }

    fn stream_decoder(&self) -> Box<dyn StreamDecoder> {
        Box::new(NoopStreamDecoder)
    }
}

fn encode_messages(messages: &[crate::Message]) -> Vec<Value> {
    let mut out = Vec::new();

    for message in messages {
        let role = match message.role.as_str() {
            "assistant" => "model",
            other => other,
        };

        let mut parts = Vec::new();
        for block in &message.content {
            match block {
                ContentBlock::Text { text } => parts.push(serde_json::json!({"text": text})),
                ContentBlock::ToolCall { name, input, .. } => parts.push(serde_json::json!({
                    "functionCall": {
                        "name": name,
                        "args": input,
                    }
                })),
                ContentBlock::ToolResult { output, .. } => parts.push(serde_json::json!({
                    "functionResponse": {
                        "name": "",
                        "response": {"result": output},
                    }
                })),
                ContentBlock::Image { .. } | ContentBlock::Document { .. } | ContentBlock::Reasoning { .. } => {}
            }
        }

        if !parts.is_empty() {
            out.push(serde_json::json!({"role": role, "parts": parts}));
        }
    }

    out
}

fn encode_tool(tool: &ToolDeclaration) -> Value {
    serde_json::json!({
        "name": tool.name,
        "description": tool.description,
        "parameters": tool.input_schema,
    })
}

fn decode_response_body(body_json: Value) -> Result<LlmResponse, LlmError> {
    let id = body_json
        .get("responseId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let model = body_json
        .get("modelVersion")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    let candidate = body_json
        .get("candidates")
        .and_then(Value::as_array)
        .and_then(|candidates| candidates.first())
        .ok_or_else(|| LlmError::InvalidRequest {
            message: "Gemini response missing candidates".to_string(),
        })?;

    let mut content = Vec::new();
    if let Some(parts) = candidate
        .get("content")
        .and_then(|content| content.get("parts"))
        .and_then(Value::as_array)
    {
        for part in parts {
            if let Some(text) = part.get("text").and_then(Value::as_str) {
                if !text.is_empty() {
                    content.push(ContentBlock::Text {
                        text: text.to_string(),
                    });
                }
                continue;
            }

            if let Some(function_call) = part.get("functionCall") {
                content.push(ContentBlock::ToolCall {
                    id: String::new(),
                    name: function_call
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    input: function_call
                        .get("args")
                        .cloned()
                        .unwrap_or_else(|| Value::Object(serde_json::Map::new())),
                });
            }
        }
    }

    let usage = body_json
        .get("usageMetadata")
        .map(|usage| Usage {
            billable_tokens: crate::TokenUsage {
                input: usage
                    .get("promptTokenCount")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                output: usage
                    .get("candidatesTokenCount")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                ..Default::default()
            },
            ..Default::default()
        })
        .unwrap_or_default();

    Ok(LlmResponse {
        id,
        model,
        content,
        usage,
        cost: None,
        provider_metadata: body_json,
    })
}
