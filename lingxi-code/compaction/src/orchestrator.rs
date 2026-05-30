//! Compaction orchestrator — runs each layer in order, escalating only when
//! cheaper layers leave us over the autocompact threshold.

use crate::autocompact::{Autocompactor, CompactionError};
use crate::microcompact::{Microcompactor, TimeBasedMCConfig};
use crate::snip::SnipCompactor;
use crate::thresholds::CompactionLayer;
use protocol::ConversationMessage;
use std::time::SystemTime;

/// Result of one orchestrator pass.
#[derive(Debug, Clone)]
pub struct IterationCompactionResult {
    /// Compacted message list.
    pub messages: Vec<ConversationMessage>,
    /// Layers that actually fired this iteration, in order.
    pub layers_applied: Vec<CompactionLayer>,
    /// Approximate tokens freed across all layers.
    pub total_tokens_freed: u64,
}

/// Owns one instance of each layer + the autocompact threshold.
pub struct CompactionOrchestrator {
    /// Snip layer.
    pub snip: SnipCompactor,
    /// Microcompact layer.
    pub micro: Microcompactor,
    /// Autocompact layer.
    pub auto: Autocompactor,
    /// Token threshold above which autocompact fires.
    pub autocompact_threshold: u64,
}

impl CompactionOrchestrator {
    /// Build a fresh orchestrator with default per-layer config.
    #[must_use]
    pub fn new(autocompact_threshold: u64) -> Self {
        Self {
            snip: SnipCompactor,
            micro: Microcompactor {
                config: TimeBasedMCConfig::default(),
            },
            auto: Autocompactor::new(),
            autocompact_threshold,
        }
    }

    /// Run one full orchestrator pass. `snip_tokens_freed_already` lets the
    /// caller report snip work done outside this entry point.
    pub async fn process_iteration(
        &self,
        mut messages: Vec<ConversationMessage>,
        snip_tokens_freed_already: u64,
    ) -> Result<IterationCompactionResult, CompactionError> {
        let mut layers = Vec::new();
        let mut freed = snip_tokens_freed_already;
        if snip_tokens_freed_already > 0 {
            layers.push(CompactionLayer::Snip);
        }

        let micro = self.micro.compact(messages, SystemTime::now());
        if micro.cleared_count > 0 {
            layers.push(CompactionLayer::Microcompact);
        }
        messages = micro.messages;

        let estimated = crate::grouping::estimate_tokens_for_range(&messages);
        if estimated > self.autocompact_threshold {
            let result = self.auto.compact(messages.clone()).await?;
            messages.clone_from(&result.summary_messages);
            freed = freed.saturating_add(
                result
                    .pre_compact_token_count
                    .saturating_sub(result.post_compact_token_count),
            );
            layers.push(CompactionLayer::Autocompact);
        }
        Ok(IterationCompactionResult {
            messages,
            layers_applied: layers,
            total_tokens_freed: freed,
        })
    }
}
