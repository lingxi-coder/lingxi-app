//! Fusion-owned construction helpers for host evidence scopes.
//!
//! The actual capture implementation lives in `platform-api` so the tool
//! registry can depend on it without a cycle. Fusion owns run/panel scope
//! construction and later report-local citation plumbing.

pub use platform_api::evidence::{
    EvidenceBlockRef, EvidenceCapability, EvidenceContext, EvidenceReceipt, EvidenceReceiptRef,
    EvidenceRun, EvidenceSource, MAX_EVIDENCE_BYTES_PER_PANEL, MAX_EVIDENCE_BYTES_PER_RECEIPT,
    MAX_EVIDENCE_BYTES_PER_RUN,
};

/// Host-owned evidence scopes for one Fusion run.
#[derive(Clone, Debug, Default)]
pub struct FusionEvidenceRun {
    inner: EvidenceRun,
}

impl FusionEvidenceRun {
    /// Mint a fresh run scope.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: EvidenceRun::new(),
        }
    }

    /// Create a fresh panel scope that shares only the run byte budget.
    #[must_use]
    pub fn panel(&self) -> FusionEvidencePanel {
        FusionEvidencePanel {
            inner: self.inner.new_panel(),
        }
    }

    /// Retained bytes across all panels in this run.
    #[must_use]
    pub fn bytes_used(&self) -> usize {
        self.inner.bytes_used()
    }
}

/// One immutable panel-owned evidence sink.
#[derive(Clone, Debug)]
pub struct FusionEvidencePanel {
    inner: EvidenceContext,
}

impl FusionEvidencePanel {
    /// Borrow the trusted context used by a panel's tool invoker builder.
    #[must_use]
    pub fn context(&self) -> EvidenceContext {
        self.inner.clone()
    }

    /// Freeze capture after the panel drains.
    pub fn freeze(&self) {
        self.inner.freeze();
    }

    /// Whether capture has been frozen.
    #[must_use]
    pub fn is_frozen(&self) -> bool {
        self.inner.is_frozen()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn panels_share_run_cap_but_not_receipts() {
        let run = FusionEvidenceRun::new();
        let left = run.panel();
        let right = run.panel();
        let value = json!({
            "type": "text",
            "file": {
                "filePath": "src/lib.rs",
                "content": "fn main() {}",
                "numLines": 1,
                "startLine": 1,
                "totalLines": 1,
            }
        });
        let left_receipt = left
            .context()
            .capture_success(EvidenceCapability::Read, None, &value);
        assert!(left_receipt.is_some());
        assert!(right
            .context()
            .resolve(left_receipt.unwrap().receipt_ref())
            .is_none());
    }
}
