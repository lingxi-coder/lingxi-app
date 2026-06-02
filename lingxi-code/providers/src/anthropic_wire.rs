//! Pure mapper: an Anthropic streaming-event JSON object → the canonical
//! [`StreamEvent`].
//!
//! Bedrock's `invoke-with-response-stream` wraps each model chunk as an
//! Anthropic stream-event JSON (the same representation the live Anthropic SSE
//! path emits). Because [`StreamEvent`] is `#[serde(tag = "type")]` and the live
//! `api_client` path decodes it with the identical `serde_json::from_str`
//! (`api_client::anthropic::AnthropicProvider::parse_stream_event`), this mapper
//! stays in lockstep with the live path by construction — no parallel mapping
//! logic to drift.

use api_client::types::StreamEvent;
use api_client::ApiError;

/// Decode one Anthropic stream-event JSON payload into a canonical [`StreamEvent`].
///
/// # Errors
/// Returns [`ApiError::MalformedStream`] if the bytes are not a valid Anthropic
/// stream event.
pub fn map_event_json(json: &[u8]) -> Result<StreamEvent, ApiError> {
    serde_json::from_slice::<StreamEvent>(json)
        .map_err(|e| ApiError::MalformedStream(format!("bedrock anthropic event decode: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use api_client::types::ContentDelta;

    #[test]
    fn maps_content_block_delta_text() {
        let json = br#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hi"}}"#;
        let ev = map_event_json(json).unwrap();
        match ev {
            StreamEvent::ContentBlockDelta {
                index,
                delta: ContentDelta::TextDelta { text },
            } => {
                assert_eq!(index, 0);
                assert_eq!(text, "hi");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn maps_message_stop_and_rejects_garbage() {
        assert!(matches!(
            map_event_json(br#"{"type":"message_stop"}"#).unwrap(),
            StreamEvent::MessageStop
        ));
        assert!(map_event_json(b"not json").is_err());
    }
}
