//! Invocation-scoped physical-attempt ceiling layered over the real host.
//! This owns no prices, registration authority, receipts, or settlement ledger.
//! Install only in an explicitly opted-in evaluation runtime; all comparisons
//! and stages in that invocation must share this same instance.

use async_trait::async_trait;
use llm_client::{
    LlmError, LlmRequest, ModelAttemptHooks, ModelAttemptLease, ModelAttemptSettlement,
    ModelAttemptUsageCompleteness, PreparedLlmCall, Usage,
};
use platform_api::ModelAttemptContext;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

pub(crate) const MAX_MODEL_CALLS: u32 = 256;

pub(crate) struct AttemptQuota {
    inner: Arc<dyn ModelAttemptHooks>,
    counters: Arc<Counters>,
}

struct Counters {
    maximum: u32,
    claimed: AtomicU32,
}

fn refused(reason: &str) -> LlmError {
    LlmError::CostUnavailable {
        message: reason.into(),
    }
}

impl AttemptQuota {
    pub(crate) fn new(
        inner: Arc<dyn ModelAttemptHooks>,
        maximum: u32,
    ) -> Result<Arc<Self>, LlmError> {
        if maximum == 0 || maximum > MAX_MODEL_CALLS {
            return Err(refused("evaluation model-call ceiling must be in 1..=256"));
        }
        Ok(Arc::new(Self {
            inner,
            counters: Arc::new(Counters {
                maximum,
                claimed: AtomicU32::new(0),
            }),
        }))
    }

    /// Conservative accepted marker attempts, not proof of remote billing.
    /// A later inner marker failure retains the slot; it is never refunded.
    pub(crate) fn claimed(&self) -> u32 {
        self.counters.claimed.load(Ordering::Acquire)
    }
}

#[async_trait]
impl ModelAttemptHooks for AttemptQuota {
    async fn begin(
        &self,
        context: &ModelAttemptContext,
        request: &LlmRequest,
        prepared: &PreparedLlmCall,
    ) -> Result<Box<dyn ModelAttemptLease>, LlmError> {
        // The real host alone authenticates registration, captured route and
        // budget. No query-source shortcut or second allowlist is introduced.
        let inner = self.inner.begin(context, request, prepared).await?;
        Ok(Box::new(QuotaLease {
            inner,
            counters: self.counters.clone(),
            marked: false,
        }))
    }
}

struct QuotaLease {
    inner: Box<dyn ModelAttemptLease>,
    counters: Arc<Counters>,
    marked: bool,
}

impl ModelAttemptLease for QuotaLease {
    fn mark_dispatched(&mut self) -> Result<(), LlmError> {
        if self.marked {
            return Err(refused(
                "evaluation physical attempt marker was already consumed",
            ));
        }
        // Even a refused marker may not be retried on this same lease.
        self.marked = true;
        self.counters
            .claimed
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |claimed| {
                claimed
                    .checked_add(1)
                    .filter(|next| *next <= self.counters.maximum)
            })
            .map_err(|_| refused("evaluation model-call ceiling exhausted before dispatch"))?;
        self.inner.mark_dispatched()
    }

    fn observe_usage(&mut self, usage: &Usage, completeness: ModelAttemptUsageCompleteness) {
        self.inner.observe_usage(usage, completeness);
    }

    fn finish(self: Box<Self>) -> Box<dyn ModelAttemptSettlement> {
        // Moving the real lease transfers its existing owned settlement. The
        // wrapper has no Drop handler: implicit drop runs the real lease Drop.
        self.inner.finish()
    }
}

#[cfg(test)]
#[path = "attempt_quota_tests.rs"]
mod tests;
