//! Host-only capture-to-history provenance. Selection is NOT wire delivery.
//!
//! Selection alone never marks a receipt included. The trusted codec/transport
//! path must prove the selected blocks survived into the actual encoded request
//! and authorize dispatch before using the capability-based marking operation.

use super::{digest_hex, lock, EvidenceCapability, EvidenceContext, EvidenceReceipt};
use protocol::{ContentBlock, ConversationMessage, MessageId};
use serde_json::Value;
use std::fmt;
use std::sync::Arc;

/// A successful registry capture, consumed when the runner binds its result.
/// No serde implementation: model output cannot manufacture this authority.
pub struct CapturedToolEvidence {
    owner: EvidenceContext,
    receipt: EvidenceReceipt,
}

impl fmt::Debug for CapturedToolEvidence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CapturedToolEvidence(<opaque>)")
    }
}

impl EvidenceContext {
    /// Capture at the same authorized registry boundary as `capture_success`,
    /// retaining its opaque capability for the runner instead of discarding it.
    /// The caller must already have checked permission and `!is_error`.
    #[must_use]
    pub fn capture_observed(
        &self,
        capability: EvidenceCapability,
        tool_use_id: Option<&str>,
        data: &Value,
    ) -> Option<CapturedToolEvidence> {
        self.capture_success(capability, tool_use_id, data)
            .map(|receipt| CapturedToolEvidence {
                owner: self.clone(),
                receipt,
            })
    }
}

impl CapturedToolEvidence {
    /// Host-minted citation handle. Its text alone is never binding authority.
    #[must_use]
    pub fn receipt(&self) -> &EvidenceReceipt {
        &self.receipt
    }

    /// Bind the exact successful text tool-result block created by the runner.
    ///
    /// The current runner renders strings directly and other values as compact
    /// JSON, matching the store digest. Media/ephemeral-summary transformations
    /// are intentionally unsupported here. Use `decorate_and_bind` for the
    /// fixed host citation suffix; arbitrary renderer changes stay Fetched.
    /// Allocate the final message id first; never bind to a provider tool id.
    #[must_use]
    pub fn bind_result_block(
        self,
        message: &ConversationMessage,
        block_index: usize,
    ) -> Option<EvidenceHistoryBinding> {
        let block = result_block(message, block_index)?;
        let ContentBlock::ToolResult {
            content,
            is_error: false,
            content_blocks: None,
            ..
        } = block
        else {
            return None;
        };
        if content.len() != self.receipt.body_bytes
            || digest_hex(content.as_bytes()) != self.receipt.digest_hex
        {
            return None;
        }
        self.bind_exact_block(message, block_index)
    }

    /// Append a fixed host citation hint only to an exact captured text result,
    /// then bind the final complete block. The source digest excludes this hint.
    /// Unsupported media or modified material is left unchanged and unbound.
    #[must_use]
    pub fn decorate_and_bind(
        self,
        message: &mut ConversationMessage,
        block_index: usize,
    ) -> Option<EvidenceHistoryBinding> {
        let block = result_block(message, block_index)?;
        let ContentBlock::ToolResult {
            content,
            content_blocks: None,
            ..
        } = block
        else {
            return None;
        };
        if content.len() != self.receipt.body_bytes
            || digest_hex(content.as_bytes()) != self.receipt.digest_hex
            || self.owner.is_frozen()
        {
            return None;
        }
        let suffix = format!("\n\n<host-evidence-receipt>\nreceipt_ref: {}\nHost fetched this tool result. Cite this receipt_ref in evidence for this result; provenance does not verify your interpretation.\n</host-evidence-receipt>", self.receipt.receipt_ref().as_str());
        if let ConversationMessage::User { content, .. } = message {
            if let ContentBlock::ToolResult { content, .. } = &mut content[block_index] {
                content.push_str(&suffix);
            }
        }
        self.bind_exact_block(message, block_index)
    }

    fn bind_exact_block(
        self,
        message: &ConversationMessage,
        block_index: usize,
    ) -> Option<EvidenceHistoryBinding> {
        let block = result_block(message, block_index)?;
        let block_digest = block_digest(block)?;
        let binding = EvidenceHistoryBinding {
            owner: self.owner,
            receipt: self.receipt,
            message_id: message.id(),
            block_index,
            block_digest,
        };
        binding.is_live().then_some(binding)
    }
}

/// Runner-local provenance for one actual host-created history block.
/// Keep outside serialized history; possession, scope and position are required
/// in addition to integrity equality. A digest or tool id cannot create this.
#[derive(Clone, PartialEq, Eq)]
pub struct EvidenceHistoryBinding {
    owner: EvidenceContext,
    receipt: EvidenceReceipt,
    message_id: MessageId,
    block_index: usize,
    block_digest: String,
}

impl fmt::Debug for EvidenceHistoryBinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("EvidenceHistoryBinding(<opaque>)")
    }
}

impl EvidenceHistoryBinding {
    /// Whether its host message still exists in the runner's full history.
    /// This supports releasing obsolete capabilities after history replacement;
    /// actual request eligibility still requires `EvidenceDelivery::select`.
    #[must_use]
    pub fn is_in_history(&self, history: &[ConversationMessage]) -> bool {
        history
            .iter()
            .any(|message| message.id() == self.message_id)
    }

    fn is_live(&self) -> bool {
        let panel = lock(&self.owner.panel.state);
        !panel.frozen
            && panel
                .records
                .get(self.receipt.receipt_ref())
                .is_some_and(|record| {
                    record.receipt.block_ref == self.receipt.block_ref
                        && record.receipt.scope == self.receipt.scope
                        && self.receipt.scope == self.owner.panel.scope
                })
    }
}

/// One eligible block's position in the selected canonical conversation.
/// This is preparation provenance, not evidence of provider submission.
#[derive(Clone, PartialEq, Eq)]
pub struct SelectedEvidence {
    binding: EvidenceHistoryBinding,
    message_index: usize,
}

impl SelectedEvidence {
    /// Revalidate this capability against the actual host input before mapping.
    #[must_use]
    pub fn revalidate_source(&self, messages: &[ConversationMessage]) -> bool {
        let selected = EvidenceDelivery::select(
            &self.binding.owner,
            std::slice::from_ref(&self.binding),
            messages,
        );
        selected.selected().iter().any(|item| item == self)
    }

    /// Upgrade only after trusted encoder proof and successful dispatch checks,
    /// immediately before transport submission. This is not remote acceptance.
    /// Callers must never invoke this at selection/preparation time.
    pub fn mark_included_after_dispatch(&self) -> bool {
        let mut panel = lock(&self.binding.owner.panel.state);
        if panel.frozen {
            return false;
        }
        let Some(record) = panel.records.get_mut(self.binding.receipt.receipt_ref()) else {
            return false;
        };
        if record.receipt.block_ref != self.binding.receipt.block_ref
            || record.receipt.scope != self.binding.receipt.scope
            || record.receipt.scope != self.binding.owner.panel.scope
        {
            return false;
        }
        record.receipt.included_in_request = true;
        true
    }

    /// Position in the actual selected history passed to the adapter.
    #[must_use]
    pub fn message_index(&self) -> usize {
        self.message_index
    }

    /// Block position within that selected message.
    #[must_use]
    pub fn block_index(&self) -> usize {
        self.binding.block_index
    }

    /// Captured metadata; `included_in_request` is not upgraded by selection.
    #[must_use]
    pub fn receipt(&self) -> &EvidenceReceipt {
        &self.binding.receipt
    }
}

impl fmt::Debug for SelectedEvidence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SelectedEvidence(<opaque>)")
    }
}

/// Immutable per-call selection, safe to retain across request preparation.
/// Never store a mutable current-turn list in a shared model-attempt context.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct EvidenceDelivery {
    selected: Arc<[SelectedEvidence]>,
}

impl fmt::Debug for EvidenceDelivery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EvidenceDelivery")
            .field("selected_count", &self.selected.len())
            .finish()
    }
}

impl EvidenceDelivery {
    /// Revalidate a whole immutable selection in one history scan. This avoids
    /// rescanning the full request for every receipt at the adapter boundary.
    #[must_use]
    pub fn revalidate_sources(&self, messages: &[ConversationMessage]) -> bool {
        let mut ids = std::collections::HashMap::new();
        for message in messages {
            *ids.entry(message.id()).or_insert(0usize) += 1;
        }
        self.selected.iter().all(|selection| {
            let binding = &selection.binding;
            binding.is_live()
                && ids.get(&binding.message_id) == Some(&1)
                && messages
                    .get(selection.message_index)
                    .is_some_and(|message| {
                        message.id() == binding.message_id
                            && result_block(message, binding.block_index)
                                .and_then(block_digest)
                                .as_deref()
                                == Some(binding.block_digest.as_str())
                    })
        })
    }

    /// Select only exact bindings surviving `cap_input_bytes`. That function
    /// clones selected messages and preserves their host ids and block order.
    /// Duplicate host ids fail closed instead of selecting an ambiguous copy.
    #[must_use]
    pub fn select(
        owner: &EvidenceContext,
        bindings: &[EvidenceHistoryBinding],
        messages: &[ConversationMessage],
    ) -> Self {
        let mut selected: Vec<SelectedEvidence> = Vec::new();
        for binding in bindings {
            if binding.owner != *owner || !binding.is_live() {
                continue;
            }
            let mut positions = messages
                .iter()
                .enumerate()
                .filter(|(_, message)| message.id() == binding.message_id);
            let Some((message_index, message)) = positions.next() else {
                continue;
            };
            if positions.next().is_some() {
                continue;
            }
            let Some(block) = result_block(message, binding.block_index) else {
                continue;
            };
            if block_digest(block).as_deref() != Some(binding.block_digest.as_str()) {
                continue;
            }
            if selected.iter().any(|item| {
                item.binding.receipt.block_ref == binding.receipt.block_ref
                    || (item.message_index == message_index
                        && item.binding.block_index == binding.block_index)
            }) {
                continue;
            }
            selected.push(SelectedEvidence {
                binding: binding.clone(),
                message_index,
            });
        }
        Self {
            selected: selected.into(),
        }
    }

    /// Borrow immutable selected provenance for the trusted adapter.
    #[must_use]
    pub fn selected(&self) -> &[SelectedEvidence] {
        &self.selected
    }
}

fn result_block(message: &ConversationMessage, index: usize) -> Option<&ContentBlock> {
    match message {
        ConversationMessage::User {
            content,
            is_visible_in_transcript_only: false,
            ..
        } => match content.get(index)? {
            block @ ContentBlock::ToolResult {
                is_error: false, ..
            } => Some(block),
            _ => None,
        },
        _ => None,
    }
}

fn block_digest(block: &ContentBlock) -> Option<String> {
    let mut writer = super::DigestPrefixWriter::new(0);
    serde_json::to_writer(&mut writer, block).ok()?;
    Some(writer.finish().digest_hex)
}

#[cfg(test)]
#[path = "delivery_test.rs"]
mod tests;
