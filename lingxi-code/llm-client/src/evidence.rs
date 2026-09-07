//! Private-to-host provenance mapping through the real conversion and codecs.
//! Tagged twins never leave these pure mapping helpers; no nonce is transmitted.

use crate::{LlmRequest, Message, WireCodec};
use platform_api::{EvidenceDelivery, SelectedEvidence};
use protocol::{ConversationMessage, MessageId};
use serde_json::Value;
use sha2::{Digest, Sha256};

#[derive(Clone, PartialEq)]
struct Mapping {
    selection: SelectedEvidence,
    pointer: String,
    expected: ExpectedText,
}

#[derive(Clone, Copy, PartialEq)]
struct ExpectedText {
    len: usize,
    digest: [u8; 32],
}

impl ExpectedText {
    fn new(text: &str) -> Self {
        Self {
            len: text.len(),
            digest: Sha256::digest(text.as_bytes()).into(),
        }
    }
    fn matches(&self, value: &Value) -> bool {
        value
            .as_str()
            .is_some_and(|text| text.len() == self.len && Self::new(text) == *self)
    }
}

/// Host-only immutable provenance. No serde implementation or public constructor.
#[derive(Clone, Default, PartialEq)]
pub struct CanonicalEvidence {
    mappings: Vec<Mapping>,
    source_digest: [u8; 32],
}

impl std::fmt::Debug for CanonicalEvidence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CanonicalEvidence")
            .field("count", &self.mappings.len())
            .finish()
    }
}

pub(crate) struct TaggedConversation {
    pub messages: Vec<ConversationMessage>,
    tags: Vec<(String, SelectedEvidence, ExpectedText)>,
}

pub(crate) fn tag_conversation(
    delivery: &EvidenceDelivery,
    messages: &[ConversationMessage],
) -> Option<TaggedConversation> {
    if !delivery.revalidate_sources(messages) {
        return None;
    }
    let mut twin = messages.to_vec();
    let mut tags = Vec::new();
    for selection in delivery.selected() {
        let ConversationMessage::User { content, .. } = twin.get_mut(selection.message_index())?
        else {
            return None;
        };
        let protocol::ContentBlock::ToolResult {
            content,
            content_blocks: None,
            is_error: false,
            ..
        } = content.get_mut(selection.block_index())?
        else {
            return None;
        };
        let nonce = format!("lingxi-private-evidence-{}", MessageId::new());
        tags.push((nonce.clone(), selection.clone(), ExpectedText::new(content)));
        *content = nonce;
    }
    (!tags.is_empty()).then_some(TaggedConversation {
        messages: twin,
        tags,
    })
}

pub(crate) fn map_canonical(
    original: &[Message],
    tagged: &[Message],
    source: TaggedConversation,
) -> Option<CanonicalEvidence> {
    let mappings = paired_mapping(
        &serde_json::to_value(original).ok()?,
        &serde_json::to_value(tagged).ok()?,
        &source.tags,
    )?;
    (!mappings.is_empty()).then_some(CanonicalEvidence {
        mappings,
        source_digest: canonical_digest(original)?,
    })
}

/// Prepared proof contains no nonce; only validated original positions/content.
pub(crate) struct PreparedEvidenceProof {
    mappings: Vec<Mapping>,
    digest: [u8; 32],
    payload_digest: [u8; 32],
}

impl std::fmt::Debug for PreparedEvidenceProof {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedEvidenceProof")
            .field("count", &self.mappings.len())
            .finish()
    }
}

impl PreparedEvidenceProof {
    pub(crate) fn seal_final_request(&mut self, request: &crate::ProviderRequest) -> bool {
        request.body_bytes.is_none()
            && request.json_string_overrides.is_empty()
            && self.seal_final_body(&request.body_json)
    }

    pub(crate) fn mark_request_submitted(&self, request: &crate::ProviderRequest) -> bool {
        request.body_bytes.is_none()
            && request.json_string_overrides.is_empty()
            && self.mark_submitted(&request.body_json)
    }

    pub(crate) fn prepare(
        request: &LlmRequest,
        codec: &dyn WireCodec,
        body: &Value,
    ) -> Option<Self> {
        let evidence = request.evidence.as_ref()?;
        if canonical_digest(&request.messages)? != evidence.source_digest {
            return None;
        }
        let mut messages = serde_json::to_value(&request.messages).ok()?;
        let mut tags = Vec::with_capacity(evidence.mappings.len());
        for mapping in &evidence.mappings {
            let leaf = messages.pointer_mut(&mapping.pointer)?;
            if !mapping.expected.matches(leaf) {
                return None;
            }
            let nonce = format!("lingxi-private-evidence-{}", MessageId::new());
            *leaf = Value::String(nonce.clone());
            tags.push((nonce, mapping.selection.clone(), mapping.expected));
        }
        let mut twin = request.clone();
        twin.evidence = None;
        twin.messages = serde_json::from_value(messages).ok()?;
        // Pure encoding only. Never authenticate, log, cache or submit the twin.
        let encoded = codec.encode_request(&twin).ok()?;
        let mappings = paired_mapping(body, &encoded.body_json, &tags)?;
        (!mappings.is_empty()).then(|| Self {
            mappings,
            digest: body_digest(body),
            payload_digest: payload_digest(body),
        })
    }

    /// Service-level options/betas may change outer request fields. Re-seal only
    /// if the entire codec-proven conversation payload (including absent keys)
    /// remains exact. Changes to roles, ids, ordering or content drop proof.
    fn seal_final_body(&mut self, body: &Value) -> bool {
        if payload_digest(body) != self.payload_digest
            || self.mappings.iter().any(|mapping| {
                !body
                    .pointer(&mapping.pointer)
                    .is_some_and(|leaf| mapping.expected.matches(leaf))
            })
        {
            return false;
        }
        self.digest = body_digest(body);
        true
    }

    /// Must follow successful model-attempt dispatch authorization with no await
    /// before the actual HTTP/SSE submission. Body mutation invalidates proof.
    fn mark_submitted(&self, body: &Value) -> bool {
        if body_digest(body) != self.digest
            || self.mappings.iter().any(|mapping| {
                !body
                    .pointer(&mapping.pointer)
                    .is_some_and(|leaf| mapping.expected.matches(leaf))
            })
        {
            return false;
        }
        self.mappings.iter().fold(true, |valid, mapping| {
            mapping.selection.mark_included_after_dispatch() && valid
        })
    }
}

fn canonical_digest(messages: &[Message]) -> Option<[u8; 32]> {
    let mut value = serde_json::to_value(messages).ok()?;
    // The service adds cache breakpoints after normalization. They do not
    // change block provenance; every other canonical field remains protected.
    for message in value.as_array_mut()? {
        for block in message.get_mut("content")?.as_array_mut()? {
            block.as_object_mut()?.remove("cache_control");
        }
    }
    Some(body_digest(&value))
}

#[cfg(test)]
#[path = "evidence_test.rs"]
pub(crate) mod tests;

fn body_digest(value: &Value) -> [u8; 32] {
    struct HashWriter(Sha256);
    impl std::io::Write for HashWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.update(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = HashWriter(Sha256::new());
    serde_json::to_writer(&mut writer, value).expect("JSON Value serialization is infallible");
    writer.0.finalize().into()
}

fn payload_digest(body: &Value) -> [u8; 32] {
    // Include presence separately so absent and explicit null cannot coincide.
    body_digest(&serde_json::json!([
        [body.get("messages").is_some(), body.get("messages")],
        [body.get("input").is_some(), body.get("input")],
        [body.get("contents").is_some(), body.get("contents")],
    ]))
}

// Walk the pair once. Every difference must be a whole nonce leaf whose
// original value matches the owned capture. Shape/order/non-target changes
// fail closed. A dropped nonce is permitted; a duplicated nonce is not.
fn paired_mapping(
    original: &Value,
    tagged: &Value,
    tags: &[(String, SelectedEvidence, ExpectedText)],
) -> Option<Vec<Mapping>> {
    let lookup: std::collections::HashMap<&str, usize> = tags
        .iter()
        .enumerate()
        .map(|(index, tag)| (tag.0.as_str(), index))
        .collect();
    if lookup.len() != tags.len() {
        return None;
    }
    let mut seen = vec![false; tags.len()];
    let mut result = Vec::new();
    let mut pending = vec![(original, tagged, String::new())];
    while let Some((left, right, pointer)) = pending.pop() {
        // A nonce collision in the original input is never treated as authority.
        if left
            .as_str()
            .is_some_and(|value| lookup.contains_key(value))
        {
            return None;
        }
        if let Some(index) = right.as_str().and_then(|value| lookup.get(value)).copied() {
            if seen[index] || !tags[index].2.matches(left) {
                return None;
            }
            seen[index] = true;
            result.push(Mapping {
                selection: tags[index].1.clone(),
                pointer,
                expected: tags[index].2,
            });
            continue;
        }
        match (left, right) {
            (Value::Object(left), Value::Object(right)) if left.len() == right.len() => {
                for (key, value) in left {
                    let other = right.get(key)?;
                    let escaped = key.replace('~', "~0").replace('/', "~1");
                    pending.push((value, other, format!("{pointer}/{escaped}")));
                }
            }
            (Value::Array(left), Value::Array(right)) if left.len() == right.len() => {
                for (index, (value, other)) in left.iter().zip(right).enumerate() {
                    pending.push((value, other, format!("{pointer}/{index}")));
                }
            }
            _ if left == right => {}
            _ => return None,
        }
    }
    Some(result)
}
