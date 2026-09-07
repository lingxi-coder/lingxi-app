use super::*;
use crate::convert::{ensure_tool_result_pairing, normalize_messages_for_api, to_llm_messages};
use platform_api::{EvidenceContext, EvidenceRun};
use protocol::{ContentBlock as Block, ToolUseId};
use serde_json::json;

pub(crate) fn fixture() -> (EvidenceContext, Vec<ConversationMessage>, EvidenceDelivery) {
    let owner = EvidenceRun::new().new_panel();
    let value = json!({"type":"text", "file":{"filePath":"src/lib.rs", "content":"proof Ω", "numLines":1, "startLine":1, "totalLines":1}});
    let id = ToolUseId::new();
    let mut messages = vec![
        ConversationMessage::user(MessageId::new(), "read".into()),
        ConversationMessage::Assistant {
            id: MessageId::new(),
            content: vec![Block::ToolUse {
                id: id.clone(),
                name: "Read".into(),
                input: json!({}),
                provider_id: None,
            }],
            stop_reason: None,
        },
        ConversationMessage::User {
            id: MessageId::new(),
            content: vec![Block::ToolResult {
                tool_use_id: id,
                content: value.to_string(),
                is_error: false,
                provider_tool_use_id: None,
                content_blocks: None,
            }],
            is_meta: false,
            is_compact_summary: false,
            is_visible_in_transcript_only: false,
        },
    ];
    let capture = owner
        .capture_observed(platform_api::EvidenceCapability::Read, None, &value)
        .unwrap();
    let binding = capture.decorate_and_bind(&mut messages[2], 0).unwrap();
    let delivery = EvidenceDelivery::select(&owner, &[binding], &messages);
    (owner, messages, delivery)
}

fn canonical(messages: Vec<ConversationMessage>) -> Vec<Message> {
    to_llm_messages(ensure_tool_result_pairing(normalize_messages_for_api(
        messages,
    )))
    .unwrap()
}

fn request(messages: Vec<ConversationMessage>, delivery: EvidenceDelivery) -> LlmRequest {
    let tagged = tag_conversation(&delivery, &messages).unwrap();
    let twin = canonical(tagged.messages.clone());
    let messages = canonical(messages);
    let mut request = LlmRequest::new("test-model");
    request.evidence = map_canonical(&messages, &twin, tagged);
    request.messages = messages;
    request.max_tokens = Some(100);
    request
}

#[test]
fn four_codecs_and_anthropic_wrappers_keep_nonce_private_and_preserve_capability() {
    let codecs: Vec<Box<dyn WireCodec>> = vec![
        Box::new(crate::AnthropicMessagesCodec::new(
            "https://fake",
            "2023-06-01",
        )),
        Box::new(crate::OpenAiChatCodec::new("https://fake")),
        Box::new(crate::OpenAiResponsesCodec::new("https://fake")),
        Box::new(crate::GeminiCodec::new("https://fake")),
        Box::new(crate::BedrockClaudeCodec::new("https://fake")),
        Box::new(crate::VertexClaudeCodec::new("https://fake")),
    ];
    for codec in codecs {
        let (owner, messages, delivery) = fixture();
        let request = request(messages, delivery);
        let encoded = codec.encode_request(&request).unwrap();
        let proof =
            PreparedEvidenceProof::prepare(&request, codec.as_ref(), &encoded.body_json).unwrap();
        assert!(!owner.receipts()[0].included_in_request);
        assert!(!encoded
            .body_json
            .to_string()
            .contains("lingxi-private-evidence-"));
        assert!(proof.mark_submitted(&encoded.body_json));
        assert!(owner.receipts()[0].included_in_request);
        assert!(
            proof.mark_submitted(&encoded.body_json),
            "retry is idempotent"
        );
    }
}

#[test]
fn body_and_canonical_identity_mutation_fail_closed() {
    let (owner, messages, delivery) = fixture();
    let mut request = request(messages, delivery);
    let codec = crate::OpenAiChatCodec::new("https://fake");
    let body = codec.encode_request(&request).unwrap().body_json;
    let proof = PreparedEvidenceProof::prepare(&request, &codec, &body).unwrap();
    let mut changed = body.clone();
    changed["unrelated"] = json!(true);
    assert!(!proof.mark_submitted(&changed));
    request.messages[2].role = "assistant".into();
    assert!(PreparedEvidenceProof::prepare(&request, &codec, &body).is_none());
    assert!(!owner.receipts()[0].included_in_request);
    owner.freeze();
    assert!(!proof.mark_submitted(&body));
}

#[test]
fn copied_or_removed_source_cannot_be_mapped() {
    let (owner, mut messages, delivery) = fixture();
    if let ConversationMessage::User { id, .. } = &mut messages[2] {
        *id = MessageId::new();
    }
    assert!(tag_conversation(&delivery, &messages).is_none());
    assert!(!owner.receipts()[0].included_in_request);
    let (_, messages, delivery) = fixture();
    let tagged = tag_conversation(&delivery, &messages).unwrap();
    assert!(map_canonical(&[], &[], tagged).is_none());
}

#[test]
fn full_tree_validation_rejects_duplication_and_non_target_differences() {
    let (_, messages, delivery) = fixture();
    let tagged = tag_conversation(&delivery, &messages).unwrap();
    let tag = &tagged.tags[0];
    let ConversationMessage::User { content, .. } = &messages[2] else {
        panic!()
    };
    let Block::ToolResult {
        content: expected, ..
    } = &content[0]
    else {
        panic!()
    };
    let original = json!([expected, expected]);
    let duplicate = json!([tag.0, tag.0]);
    assert!(paired_mapping(&original, &duplicate, &tagged.tags).is_none());
    assert!(paired_mapping(
        &json!({"target":expected, "other":1}),
        &json!({"target":tag.0,"other":2}),
        &tagged.tags
    )
    .is_none());
    assert!(paired_mapping(
        &json!({"target":expected}),
        &json!({"target":format!("prefix {}",tag.0)}),
        &tagged.tags
    )
    .is_none());
}

#[test]
fn serialized_request_loses_evidence_authority() {
    let (_, messages, delivery) = fixture();
    let request = request(messages, delivery);
    let encoded = serde_json::to_value(&request).unwrap();
    assert!(encoded.get("evidence").is_none());
    let decoded: LlmRequest = serde_json::from_value(encoded).unwrap();
    assert!(decoded.evidence.is_none());
}

#[test]
fn final_service_options_can_reseal_but_message_changes_cannot() {
    let (owner, messages, delivery) = fixture();
    let request = request(messages, delivery);
    let codec = crate::AnthropicMessagesCodec::new("https://fake", "2023-06-01");
    let mut body = codec.encode_request(&request).unwrap().body_json;
    let mut proof = PreparedEvidenceProof::prepare(&request, &codec, &body).unwrap();
    body["anthropic_beta"] = json!(["test-beta"]);
    body["temperature"] = json!(0.5);
    assert!(!proof.mark_submitted(&body));
    assert!(proof.seal_final_body(&body));
    let mut changed = body.clone();
    changed["messages"][0]["role"] = json!("assistant");
    assert!(!proof.seal_final_body(&changed));
    assert!(!owner.receipts()[0].included_in_request);
    assert!(proof.mark_submitted(&body));
}

#[test]
fn raw_body_and_utf16_override_cannot_impersonate_proven_json() {
    let (owner, messages, delivery) = fixture();
    let request = request(messages, delivery);
    let codec = crate::OpenAiChatCodec::new("https://fake");
    let mut encoded = codec.encode_request(&request).unwrap();
    let mut proof = PreparedEvidenceProof::prepare(&request, &codec, &encoded.body_json).unwrap();
    encoded.body_bytes = Some(b"{}".to_vec());
    assert!(!proof.seal_final_request(&encoded));
    assert!(!proof.mark_request_submitted(&encoded));
    encoded.body_bytes = None;
    encoded
        .json_string_overrides
        .insert("/messages/2/content".into(), vec![65]);
    assert!(!proof.seal_final_request(&encoded));
    assert!(!proof.mark_request_submitted(&encoded));
    assert!(!owner.receipts()[0].included_in_request);
}
