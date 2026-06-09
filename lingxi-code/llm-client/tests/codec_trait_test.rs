use llm_client::{LlmRequest, ProviderRequest, ProviderResponse, WireCodec};

#[derive(Debug)]
struct DummyCodec;

impl WireCodec for DummyCodec {
    fn encode_request(
        &self,
        request: &LlmRequest,
    ) -> Result<ProviderRequest, llm_client::LlmError> {
        Ok(ProviderRequest::post_json(
            "https://example.test/v1/messages",
            serde_json::json!({"model": request.model}),
        ))
    }

    fn decode_response(
        &self,
        response: ProviderResponse,
    ) -> Result<llm_client::LlmResponse, llm_client::LlmError> {
        assert_eq!(response.status, 200);
        Ok(llm_client::LlmResponse {
            id: "id".to_string(),
            model: "model".to_string(),
            content: vec![],
            usage: Default::default(),
            cost: None,
            provider_metadata: Default::default(),
        })
    }

    fn stream_decoder(&self) -> Box<dyn llm_client::StreamDecoder> {
        Box::new(llm_client::NoopStreamDecoder)
    }
}

#[test]
fn codec_returns_post_json_provider_request() {
    let request = DummyCodec.encode_request(&LlmRequest::new("model-a")).unwrap();

    assert_eq!(request.method, "POST");
    assert_eq!(request.url, "https://example.test/v1/messages");
    assert_eq!(request.body_json["model"], "model-a");
}
