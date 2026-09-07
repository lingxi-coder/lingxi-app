//! Fusion-owned construction helpers for host evidence scopes.
//!
//! The actual capture implementation lives in `platform-api` so the tool
//! registry can depend on it without a cycle. Fusion owns run/panel scope
//! construction and later report-local citation plumbing.

pub use platform_api::evidence::{
    EvidenceAttestation, EvidenceBlockRef, EvidenceCapability, EvidenceContext, EvidenceReceipt,
    EvidenceReceiptRef, EvidenceRun, EvidenceSource, MAX_EVIDENCE_BYTES_PER_PANEL,
    MAX_EVIDENCE_BYTES_PER_RECEIPT, MAX_EVIDENCE_BYTES_PER_RUN,
};

pub(crate) mod citations;

/// Report-local association created only by the host after producer drain.
#[derive(Clone, Debug, serde::Serialize)]
pub struct HostPanelEvidence {
    /// Sanitized report-local evidence id, not a provider or run id.
    pub evidence_id: String,
    /// Immutable projection with no raw store access.
    pub attestation: EvidenceAttestation,
    /// Exact report-local excerpt only when supported by retained material.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub excerpt: Option<String>,
}

pub(crate) fn attest_report(
    context: &EvidenceContext,
    report: &platform_api::PanelReport,
) -> Vec<HostPanelEvidence> {
    report
        .evidence
        .iter()
        .filter_map(|evidence| {
            let attestation = context.attest_reference(
                evidence.receipt_ref.as_deref()?,
                evidence.kind,
                &evidence.locator,
                evidence.excerpt.as_deref(),
            )?;
            let excerpt = matches!(
                attestation.excerpt_status(),
                platform_api::EvidenceExcerptStatus::Present
                    | platform_api::EvidenceExcerptStatus::PresentInPrefix
            )
            .then(|| evidence.excerpt.clone())
            .flatten();
            Some(HostPanelEvidence {
                evidence_id: evidence.id.clone(),
                attestation,
                excerpt,
            })
        })
        .collect()
}

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
pub(crate) mod tests {
    use super::*;
    use serde_json::json;

    pub(crate) fn included_host_evidence() -> HostPanelEvidence {
        let context = EvidenceRun::new().new_panel();
        let value = json!({"type":"text", "file":{"filePath":"a.rs", "content":"source", "numLines":1,"startLine":1,"totalLines":1}});
        let capture = context
            .capture_observed(EvidenceCapability::Read, None, &value)
            .unwrap();
        let reference = capture.receipt().receipt_ref().as_str().to_owned();
        let message = protocol::ConversationMessage::User {
            id: protocol::MessageId::new(),
            content: vec![protocol::ContentBlock::ToolResult {
                tool_use_id: "tool".into(),
                content: value.to_string(),
                is_error: false,
                provider_tool_use_id: None,
                content_blocks: None,
            }],
            is_meta: false,
            is_compact_summary: false,
            is_visible_in_transcript_only: false,
        };
        let binding = capture.bind_result_block(&message, 0).unwrap();
        let delivery = platform_api::EvidenceDelivery::select(&context, &[binding], &[message]);
        assert!(delivery.selected()[0].mark_included_after_dispatch());
        context.freeze();
        HostPanelEvidence {
            evidence_id: "e1".into(),
            attestation: context
                .attest_reference(
                    &reference,
                    platform_api::EvidenceKind::File,
                    "a.rs",
                    Some("source"),
                )
                .unwrap(),
            excerpt: Some("source".into()),
        }
    }

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
