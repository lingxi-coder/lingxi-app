//! Host-owned evidence capture for Fusion tool calls.
//!
//! Evidence is deliberately a private-to-the-host side channel.  A tool call
//! can mint a receipt only after the registry has completed the permission
//! checks and returned a non-error [`ToolCallResult`].  The model-facing JSON
//! value remains untouched; receipt references and the bounded material store
//! never enter that value.
//!
//! This module is the bounded foundation for the later actual-wire delivery
//! observer.  A captured receipt is therefore only `fetched`: this module does
//! not infer `included_in_request` from conversation history or from strings
//! that happen to contain an id.

use crate::fusion::EvidenceKind;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fmt;
use std::io::{self, Write};
use std::sync::{Arc, Mutex, MutexGuard};
use uuid::Uuid;

mod delivery;
pub use delivery::{
    CapturedToolEvidence, EvidenceDelivery, EvidenceHistoryBinding, SelectedEvidence,
};

/// Maximum material retained for one successful evidence receipt.
pub const MAX_EVIDENCE_BYTES_PER_RECEIPT: usize = 512 * 1024;
/// Maximum material retained by one Fusion panel.
pub const MAX_EVIDENCE_BYTES_PER_PANEL: usize = 2 * 1024 * 1024;
/// Maximum material retained by one Fusion run across all panels.
pub const MAX_EVIDENCE_BYTES_PER_RUN: usize = 16 * 1024 * 1024;

/// What the retained material establishes about a panel-authored excerpt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceExcerptStatus {
    /// No excerpt was claimed.
    NotRequested,
    /// Exact text occurs in complete retained material.
    Present,
    /// Exact text occurs in the retained prefix; the rest was not retained.
    PresentInPrefix,
    /// Complete material does not contain the requested excerpt.
    NotFound,
    /// A truncated tail may contain the excerpt; the host cannot decide.
    UnknownTruncated,
}

/// Frozen, host-produced provenance projection, never constructible from JSON.
/// It attests retrieval/delivery, not correctness of the model's interpretation.
#[derive(Clone, PartialEq, Eq, serde::Serialize)]
pub struct EvidenceAttestation {
    receipt_ref: String,
    kind: EvidenceKind,
    source: &'static str,
    fetched_at_ms: Option<u64>,
    status_code: Option<u16>,
    digest_hex: String,
    body_bytes: usize,
    stored_bytes: usize,
    truncated: bool,
    fetched: bool,
    included_in_request: bool,
    excerpt_status: EvidenceExcerptStatus,
}

impl fmt::Debug for EvidenceAttestation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EvidenceAttestation")
            .field("included_in_request", &self.included_in_request)
            .field("excerpt_status", &self.excerpt_status)
            .finish_non_exhaustive()
    }
}

impl EvidenceAttestation {
    /// Opaque report-local citation reference.
    #[must_use]
    pub fn receipt_ref(&self) -> &str {
        &self.receipt_ref
    }
    /// Whether this reference may support a synthesis citation. Unknown quotes
    /// remain visible as metadata, but cannot become positively verified quotes.
    #[must_use]
    pub fn allows_citation(&self) -> bool {
        self.fetched
            && self.included_in_request
            && matches!(
                self.excerpt_status,
                EvidenceExcerptStatus::NotRequested
                    | EvidenceExcerptStatus::Present
                    | EvidenceExcerptStatus::PresentInPrefix
            )
    }
    /// Exact excerpt support status, without interpreting the model's claim.
    #[must_use]
    pub fn excerpt_status(&self) -> EvidenceExcerptStatus {
        self.excerpt_status
    }
}

/// Where a successful URL result came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceSource {
    /// A native workspace tool result.
    NativeTool,
    /// A WebFetch result served from the host cache.
    WebFetchCache,
    /// A WebFetch result fetched from the network.
    WebFetchNetwork,
}

/// Exact native-tool extractor authorized for a Fusion evidence capture.
///
/// This is intentionally a Rust-only capability rather than a serialized tool
/// name or a broad evidence category. A concrete registered tool
/// implementation must opt into one reviewed result shape; model JSON and
/// aliases cannot select an extractor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceCapability {
    /// Successful text result from the native `Read` tool.
    Read,
    /// Successful result from the native `Grep` tool.
    Grep,
    /// Successful result from the native `Glob` tool.
    Glob,
    /// Successful final 2xx result from the native `WebFetch` tool.
    WebFetch,
}

impl EvidenceCapability {
    const fn kind(self) -> EvidenceKind {
        match self {
            Self::Read | Self::Grep | Self::Glob => EvidenceKind::File,
            Self::WebFetch => EvidenceKind::Url,
        }
    }
}

/// Opaque host-minted receipt reference.
///
/// This type intentionally does not implement `Serialize`; it is suitable for
/// trusted host plumbing and later report-local citation validation, not for
/// model JSON, telemetry, or durable task state.
#[derive(Clone, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct EvidenceReceiptRef(String);

impl EvidenceReceiptRef {
    /// Borrow the opaque reference for trusted host-side handoff.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for EvidenceReceiptRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for EvidenceReceiptRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("EvidenceReceiptRef(<opaque>)")
    }
}

/// Strong host-bound identity for the tool-result block that produced a
/// receipt.  It is intentionally not constructible from a model string.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct EvidenceBlockRef(Uuid);

impl fmt::Debug for EvidenceBlockRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("EvidenceBlockRef(<opaque>)")
    }
}

/// Sanitized receipt metadata exposed to trusted host code.
///
/// The body itself stays in the isolated store.  `digest_hex` attests to the
/// complete body, while `stored_bytes` and `truncated` describe the retained
/// prefix.  The locator is represented only by a digest, avoiding raw paths,
/// URLs, prompts, and provider ids in outward DTOs.
#[derive(Clone, PartialEq, Eq)]
pub struct EvidenceReceipt {
    receipt_ref: EvidenceReceiptRef,
    block_ref: EvidenceBlockRef,
    scope: EvidenceScope,
    /// Trusted evidence category selected by the concrete Tool implementation.
    pub kind: EvidenceKind,
    /// Provenance for this successful fetch/read.
    pub source: EvidenceSource,
    /// WebFetch's host-provided fetch timestamp, when available.
    pub fetched_at_ms: Option<u64>,
    /// HTTP status for WebFetch, when available.
    pub status_code: Option<u16>,
    /// Digest of the full unbounded material returned by the tool.
    pub digest_hex: String,
    /// Full material length before the per-receipt cap.
    pub body_bytes: usize,
    /// Bytes retained in the isolated store.
    pub stored_bytes: usize,
    /// True when the full body was not retained.
    pub truncated: bool,
    /// True only because this receipt was created by an authorized capture.
    pub fetched: bool,
    /// Reserved for the later actual-wire observer.  Capture never sets it.
    pub included_in_request: bool,
    /// Metadata fields above remain available even when all body bytes are
    /// exhausted by panel/run caps.
    pub metadata_complete: bool,
    locator_digest_hex: Option<String>,
    /// Opaque locator for the captured search result, never a model-supplied
    /// path or a claim that every matching file was read. Only Grep/Glob mint it.
    search_result_locator: Option<String>,
}

impl fmt::Debug for EvidenceReceipt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EvidenceReceipt")
            .field("kind", &self.kind)
            .field("source", &self.source)
            .field("fetched_at_ms", &self.fetched_at_ms)
            .field("status_code", &self.status_code)
            .field("digest_hex", &self.digest_hex)
            .field("body_bytes", &self.body_bytes)
            .field("stored_bytes", &self.stored_bytes)
            .field("truncated", &self.truncated)
            .field("fetched", &self.fetched)
            .field("included_in_request", &self.included_in_request)
            .field("metadata_complete", &self.metadata_complete)
            .finish_non_exhaustive()
    }
}

impl EvidenceReceipt {
    /// Return the opaque host-minted citation reference.
    #[must_use]
    pub fn receipt_ref(&self) -> &EvidenceReceiptRef {
        &self.receipt_ref
    }

    /// Return the strong host-bound result-block identity for later delivery
    /// observation.  This is not a model-authored tool-use id.
    #[must_use]
    pub fn block_ref(&self) -> &EvidenceBlockRef {
        &self.block_ref
    }

    /// Digest of the trusted source locator, if the tool returned one.
    #[must_use]
    pub fn locator_digest_hex(&self) -> Option<&str> {
        self.locator_digest_hex.as_deref()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct EvidenceScope {
    run: Uuid,
    panel: Uuid,
}

struct RunLedger {
    id: Uuid,
    bytes_used: Mutex<usize>,
}

impl fmt::Debug for RunLedger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RunLedger")
            .field("bytes_used", &lock(&self.bytes_used))
            .finish_non_exhaustive()
    }
}

struct PanelLedger {
    scope: EvidenceScope,
    state: Mutex<PanelState>,
}

impl fmt::Debug for PanelLedger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = lock(&self.state);
        f.debug_struct("PanelLedger")
            .field("bytes_used", &state.bytes_used)
            .field("frozen", &state.frozen)
            .field("receipt_count", &state.records.len())
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Default)]
struct PanelState {
    bytes_used: usize,
    frozen: bool,
    records: BTreeMap<EvidenceReceiptRef, EvidenceRecord>,
}

struct EvidenceRecord {
    receipt: EvidenceReceipt,
    body: Vec<u8>,
    /// Correlation only.  It is never copied into [`EvidenceReceipt`].
    tool_use_id: Option<String>,
}

impl fmt::Debug for EvidenceRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EvidenceRecord")
            .field("receipt", &self.receipt)
            .field("body_bytes", &self.body.len())
            .field("has_tool_use_correlation", &self.tool_use_id.is_some())
            .finish_non_exhaustive()
    }
}

/// Host-owned ledger shared by a Fusion run.
#[derive(Clone)]
pub struct EvidenceRun {
    ledger: Arc<RunLedger>,
}

impl fmt::Debug for EvidenceRun {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EvidenceRun")
            .field("bytes_used", &self.bytes_used())
            .finish_non_exhaustive()
    }
}

impl EvidenceRun {
    /// Mint a fresh run scope.
    #[must_use]
    pub fn new() -> Self {
        Self {
            ledger: Arc::new(RunLedger {
                id: Uuid::new_v4(),
                bytes_used: Mutex::new(0),
            }),
        }
    }

    /// Create a fresh immutable panel scope under this run.
    #[must_use]
    pub fn new_panel(&self) -> EvidenceContext {
        EvidenceContext {
            run: Arc::clone(&self.ledger),
            panel: Arc::new(PanelLedger {
                scope: EvidenceScope {
                    run: self.ledger.id,
                    panel: Uuid::new_v4(),
                },
                state: Mutex::new(PanelState::default()),
            }),
        }
    }

    /// Return the total retained body bytes across this run.
    #[must_use]
    pub fn bytes_used(&self) -> usize {
        *lock(&self.ledger.bytes_used)
    }
}

impl Default for EvidenceRun {
    fn default() -> Self {
        Self::new()
    }
}

/// Optional trusted sink installed on a Fusion panel's tool invoker.
///
/// Cloning this handle does not create a sibling scope: all clones refer to
/// the same panel ledger.  A separate call to [`EvidenceRun::new_panel`]
/// creates a sibling that cannot resolve this panel's receipt references.
#[derive(Clone)]
pub struct EvidenceContext {
    run: Arc<RunLedger>,
    panel: Arc<PanelLedger>,
}

impl fmt::Debug for EvidenceContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EvidenceContext")
            .field("bytes_used", &self.bytes_used())
            .field("frozen", &self.is_frozen())
            .finish_non_exhaustive()
    }
}

impl PartialEq for EvidenceContext {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.run, &other.run) && Arc::ptr_eq(&self.panel, &other.panel)
    }
}

impl Eq for EvidenceContext {}

impl EvidenceContext {
    /// Resolve a report-authored reference only in this frozen panel scope.
    /// Kind and locator must match the captured native source. Forged, sibling,
    /// or modified references never manufacture an attestation.
    #[must_use]
    pub fn attest_reference(
        &self,
        reference: &str,
        kind: EvidenceKind,
        locator: &str,
        excerpt: Option<&str>,
    ) -> Option<EvidenceAttestation> {
        let panel = lock(&self.panel.state);
        if !panel.frozen {
            return None;
        }
        let record = panel
            .records
            .get(&EvidenceReceiptRef(reference.to_owned()))?;
        let receipt = &record.receipt;
        if receipt.scope != self.panel.scope
            || receipt.kind != kind
            || receipt.locator_digest_hex.as_deref()
                != Some(digest_hex(locator.as_bytes()).as_str())
        {
            return None;
        }
        let excerpt_status = match excerpt.filter(|text| !text.is_empty()) {
            None => EvidenceExcerptStatus::NotRequested,
            Some(excerpt) => {
                let raw_match =
                    std::str::from_utf8(&record.body).is_ok_and(|body| body.contains(excerpt));
                let parsed_match = !receipt.truncated
                    && serde_json::from_slice::<Value>(&record.body)
                        .ok()
                        .is_some_and(|value| material_contains(&value, excerpt));
                match (raw_match || parsed_match, receipt.truncated) {
                    (true, false) => EvidenceExcerptStatus::Present,
                    (true, true) => EvidenceExcerptStatus::PresentInPrefix,
                    (false, false) => EvidenceExcerptStatus::NotFound,
                    (false, true) => EvidenceExcerptStatus::UnknownTruncated,
                }
            }
        };
        Some(EvidenceAttestation {
            receipt_ref: reference.to_owned(),
            kind,
            source: match receipt.source {
                EvidenceSource::NativeTool => "native_tool",
                EvidenceSource::WebFetchCache => "web_fetch_cache",
                EvidenceSource::WebFetchNetwork => "web_fetch_network",
            },
            fetched_at_ms: receipt.fetched_at_ms,
            status_code: receipt.status_code,
            digest_hex: receipt.digest_hex.clone(),
            body_bytes: receipt.body_bytes,
            stored_bytes: receipt.stored_bytes,
            truncated: receipt.truncated,
            fetched: receipt.fetched,
            included_in_request: receipt.included_in_request,
            excerpt_status,
        })
    }

    /// Capture one already-authorized, successful tool result.
    ///
    /// The registry calls this only for a trusted Fusion policy after
    /// `Tool::call` returned `Ok` and `ToolCallResult::is_error == false`.
    /// Unsupported capabilities, HTTP failures, redirect notices, malformed
    /// output, and frozen panels return `None` without affecting the tool's
    /// normal result.
    #[must_use]
    pub fn capture_success(
        &self,
        capability: EvidenceCapability,
        tool_use_id: Option<&str>,
        data: &Value,
    ) -> Option<EvidenceReceipt> {
        let material = EvidenceMaterial::from_tool_result(capability, data)?;
        // Snapshot only a bounded prefix while streaming the complete body
        // through the digest/count writer. A large Grep/Glob/URL result never
        // becomes an unbounded temporary allocation in the evidence store.
        let snapshot = material.snapshot(MAX_EVIDENCE_BYTES_PER_RECEIPT).ok()?;
        let full_len = snapshot.full_len;
        let body_digest_hex = snapshot.digest_hex;
        let receipt_ref = EvidenceReceiptRef(format!("evr_{}", Uuid::new_v4().simple()));
        let search_result_locator = match capability {
            EvidenceCapability::Grep => Some(format!("lingxi-search:grep:{receipt_ref}")),
            EvidenceCapability::Glob => Some(format!("lingxi-search:glob:{receipt_ref}")),
            EvidenceCapability::Read | EvidenceCapability::WebFetch => None,
        };
        let block_ref = EvidenceBlockRef(Uuid::new_v4());

        // The run lock is always acquired before the panel lock.  No other
        // operation acquires them in the opposite order.
        let mut run_bytes = lock(&self.run.bytes_used);
        let mut panel = lock(&self.panel.state);
        if panel.frozen {
            return None;
        }

        let run_remaining = MAX_EVIDENCE_BYTES_PER_RUN.saturating_sub(*run_bytes);
        let panel_remaining = MAX_EVIDENCE_BYTES_PER_PANEL.saturating_sub(panel.bytes_used);
        let retained_limit = full_len
            .min(MAX_EVIDENCE_BYTES_PER_RECEIPT)
            .min(run_remaining)
            .min(panel_remaining);
        let retained_prefix_len = utf8_prefix_len(&snapshot.prefix, retained_limit);
        let body = snapshot.prefix[..retained_prefix_len].to_vec();
        *run_bytes = run_bytes.saturating_add(body.len());
        panel.bytes_used = panel.bytes_used.saturating_add(body.len());

        let receipt = EvidenceReceipt {
            receipt_ref: receipt_ref.clone(),
            block_ref,
            scope: self.panel.scope,
            kind: capability.kind(),
            source: material.source,
            fetched_at_ms: material.fetched_at_ms,
            status_code: material.status_code,
            digest_hex: body_digest_hex,
            body_bytes: full_len,
            stored_bytes: body.len(),
            truncated: body.len() < full_len,
            fetched: true,
            included_in_request: false,
            metadata_complete: material.metadata_complete,
            locator_digest_hex: material
                .locator
                .as_deref()
                .or(search_result_locator.as_deref())
                .map(|locator| digest_hex(locator.as_bytes())),
            search_result_locator,
        };
        let record = EvidenceRecord {
            receipt: receipt.clone(),
            body,
            tool_use_id: tool_use_id.map(str::to_owned),
        };
        panel.records.insert(receipt_ref, record);
        Some(receipt)
    }

    /// Resolve a receipt only inside this immutable run/panel scope.
    #[must_use]
    pub fn resolve(&self, receipt_ref: &EvidenceReceiptRef) -> Option<EvidenceReceipt> {
        let panel = lock(&self.panel.state);
        panel.records.get(receipt_ref).and_then(|record| {
            // Keep the model/provider tool-use id as correlation-only state;
            // receipt lookup remains keyed by the host-minted reference.
            let _correlation = record.tool_use_id.as_deref();
            (record.receipt.receipt_ref == *receipt_ref).then(|| record.receipt.clone())
        })
    }

    /// List sanitized receipts retained by this panel. Raw bodies and
    /// correlation ids remain inside the isolated store.
    #[must_use]
    pub fn receipts(&self) -> Vec<EvidenceReceipt> {
        lock(&self.panel.state)
            .records
            .values()
            .map(|record| record.receipt.clone())
            .collect()
    }

    /// Read the retained body for a receipt from this exact panel scope.
    ///
    /// An empty vector is a valid result when metadata survived after a panel
    /// or run cap was exhausted; callers must inspect `stored_bytes` and
    /// `truncated` on the receipt rather than treating empty as a successful
    /// full capture.
    #[must_use]
    pub fn body(&self, receipt: &EvidenceReceipt) -> Option<Vec<u8>> {
        let panel = lock(&self.panel.state);
        let record = panel.records.get(receipt.receipt_ref())?;
        same_capture(&record.receipt, receipt, self.panel.scope).then(|| record.body.clone())
    }

    /// Freeze this panel after its drain.  Future capture attempts are ignored.
    pub fn freeze(&self) {
        lock(&self.panel.state).frozen = true;
    }

    /// Whether the panel has been frozen.
    #[must_use]
    pub fn is_frozen(&self) -> bool {
        lock(&self.panel.state).frozen
    }

    /// Return the retained body bytes for this panel, useful for host-side
    /// diagnostics without exposing the panel's raw records or ids.
    #[must_use]
    pub fn bytes_used(&self) -> usize {
        lock(&self.panel.state).bytes_used
    }

    /// Strong scope check used by later actual-wire delivery observers.
    #[must_use]
    pub fn owns(&self, receipt: &EvidenceReceipt) -> bool {
        self.resolve(receipt.receipt_ref())
            .is_some_and(|stored| same_capture(&stored, receipt, self.panel.scope))
    }

    /// Return whether the host capture correlated a particular tool-use id.
    ///
    /// This is intentionally a private-correlation query and does not make the
    /// model-authored id an authority for evidence delivery.
    #[cfg(test)]
    fn correlated_tool_use_id(&self, receipt: &EvidenceReceipt, tool_use_id: &str) -> bool {
        let panel = lock(&self.panel.state);
        panel
            .records
            .get(receipt.receipt_ref())
            .and_then(|record| record.tool_use_id.as_deref())
            == Some(tool_use_id)
    }
}

// `EvidenceReceipt` intentionally keeps scope private.  This helper is kept
// next to the type so future delivery code can replace it with a block-token
// comparison without exposing run/panel ids in DTOs.
fn receipt_scope(receipt: &EvidenceReceipt, scope: EvidenceScope) -> bool {
    receipt.scope == scope
}

fn material_contains(value: &Value, excerpt: &str) -> bool {
    match value {
        Value::String(text) => text.contains(excerpt),
        Value::Array(values) => values.iter().any(|value| material_contains(value, excerpt)),
        Value::Object(values) => values
            .values()
            .any(|value| material_contains(value, excerpt)),
        _ => false,
    }
}

#[cfg(test)]
mod attestation_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn frozen_scope_checks_reference_kind_locator_and_exact_excerpt() {
        let run = EvidenceRun::new();
        let panel = run.new_panel();
        let sibling = run.new_panel();
        let value = json!({"type":"text","file":{"filePath":"a.rs","content":"first\nsecond","numLines":2,"startLine":1,"totalLines":2}});
        let receipt = panel
            .capture_success(EvidenceCapability::Read, None, &value)
            .unwrap();
        let reference = receipt.receipt_ref().as_str();
        assert!(panel
            .attest_reference(reference, EvidenceKind::File, "a.rs", None)
            .is_none());
        panel.freeze();
        sibling.freeze();
        assert!(sibling
            .attest_reference(reference, EvidenceKind::File, "a.rs", None)
            .is_none());
        assert!(panel
            .attest_reference(reference, EvidenceKind::Url, "a.rs", None)
            .is_none());
        assert!(panel
            .attest_reference(reference, EvidenceKind::File, "b.rs", None)
            .is_none());
        assert!(panel
            .attest_reference(&format!(" {reference}"), EvidenceKind::File, "a.rs", None)
            .is_none());
        let valid = panel
            .attest_reference(reference, EvidenceKind::File, "a.rs", Some("first\nsecond"))
            .unwrap();
        assert_eq!(valid.excerpt_status(), EvidenceExcerptStatus::Present);
        assert!(!valid.allows_citation(), "Fetched is not Included");
        assert_eq!(
            panel
                .attest_reference(reference, EvidenceKind::File, "a.rs", Some("third"))
                .unwrap()
                .excerpt_status(),
            EvidenceExcerptStatus::NotFound
        );
    }

    #[test]
    fn truncated_material_retains_metadata_without_claiming_tail_support() {
        let panel = EvidenceRun::new().new_panel();
        let value = json!({"type":"text","file":{"filePath":"a.rs","content":format!("prefix {} tail", "x".repeat(MAX_EVIDENCE_BYTES_PER_RECEIPT)),"numLines":1,"startLine":1,"totalLines":1}});
        let receipt = panel
            .capture_success(EvidenceCapability::Read, None, &value)
            .unwrap();
        panel.freeze();
        let prefix = panel
            .attest_reference(
                receipt.receipt_ref().as_str(),
                EvidenceKind::File,
                "a.rs",
                Some("prefix"),
            )
            .unwrap();
        assert_eq!(
            prefix.excerpt_status(),
            EvidenceExcerptStatus::PresentInPrefix
        );
        let tail = panel
            .attest_reference(
                receipt.receipt_ref().as_str(),
                EvidenceKind::File,
                "a.rs",
                Some("tail"),
            )
            .unwrap();
        assert_eq!(
            tail.excerpt_status(),
            EvidenceExcerptStatus::UnknownTruncated
        );
        assert!(!tail.allows_citation());
        assert!(serde_json::to_value(tail).unwrap()["truncated"]
            .as_bool()
            .unwrap());
    }
}

fn same_capture(
    stored: &EvidenceReceipt,
    supplied: &EvidenceReceipt,
    scope: EvidenceScope,
) -> bool {
    if !receipt_scope(supplied, scope) || stored.block_ref != supplied.block_ref {
        return false;
    }
    // Delivery is monotonic state, not capture identity. All remaining immutable
    // metadata must still match; forged public digest/length fields stay rejected.
    let mut normalized = supplied.clone();
    normalized.included_in_request = stored.included_in_request;
    stored == &normalized
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn utf8_prefix_len(bytes: &[u8], limit: usize) -> usize {
    let mut end = bytes.len().min(limit);
    while end > 0 && std::str::from_utf8(&bytes[..end]).is_err() {
        end -= 1;
    }
    end
}

fn digest_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    bytes_to_hex(&digest)
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let mut output = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        // Writing into a String is infallible.
        write!(output, "{byte:02x}").expect("writing into a String cannot fail");
    }
    output
}

struct EvidenceMaterial<'a> {
    body: MaterialBody<'a>,
    source: EvidenceSource,
    fetched_at_ms: Option<u64>,
    status_code: Option<u16>,
    locator: Option<&'a str>,
    metadata_complete: bool,
}

enum MaterialBody<'a> {
    ModelVisibleValue(&'a Value),
}

struct MaterialSnapshot {
    prefix: Vec<u8>,
    full_len: usize,
    digest_hex: String,
}

struct ValidatedWebFetch<'a> {
    source: EvidenceSource,
    fetched_at_ms: u64,
    status_code: u16,
    locator: &'a str,
}

fn validate_read_result(data: &Value) -> Option<&str> {
    let object = data.as_object()?;
    if object.get("type")?.as_str()? != "text" || has_failure_marker(object) {
        return None;
    }
    let file = object.get("file")?.as_object()?;
    let locator = nonempty_string(file.get("filePath")?)?;
    file.get("content")?.as_str()?;
    let num_lines = file.get("numLines")?.as_u64()?;
    file.get("startLine")?.as_u64()?;
    let total_lines = file.get("totalLines")?.as_u64()?;
    if num_lines > total_lines {
        return None;
    }
    if file
        .get("truncatedByTokenCap")
        .is_some_and(|value| !value.is_boolean())
    {
        return None;
    }
    Some(locator)
}

fn validate_grep_result(data: &Value) -> Option<()> {
    let object = data.as_object()?;
    if has_failure_marker(object) || !optional_u64(object, "appliedLimit") {
        return None;
    }
    if !optional_u64(object, "appliedOffset") {
        return None;
    }
    let mode = object.get("mode")?.as_str()?;
    let num_files = usize::try_from(object.get("numFiles")?.as_u64()?).ok()?;
    let filenames = string_array_len(object.get("filenames")?)?;
    match mode {
        "content" => {
            if num_files != 0 || filenames != 0 {
                return None;
            }
            object.get("content")?.as_str()?;
            let num_lines = object.get("numLines")?.as_u64()?;
            let total_lines = object.get("totalLines")?.as_u64()?;
            (num_lines <= total_lines).then_some(())
        }
        "count" => {
            if filenames != 0 {
                return None;
            }
            object.get("content")?.as_str()?;
            object.get("numMatches")?.as_u64()?;
            Some(())
        }
        "files_with_matches" => {
            if object.contains_key("content") || num_files != filenames {
                return None;
            }
            let total_files = usize::try_from(object.get("totalFiles")?.as_u64()?).ok()?;
            (num_files <= total_files).then_some(())
        }
        _ => None,
    }
}

fn validate_glob_result(data: &Value) -> Option<()> {
    let object = data.as_object()?;
    if has_failure_marker(object) {
        return None;
    }
    let filenames = string_array_len(object.get("filenames")?)?;
    let num_files = usize::try_from(object.get("numFiles")?.as_u64()?).ok()?;
    let total_matches = usize::try_from(object.get("totalMatches")?.as_u64()?).ok()?;
    object.get("durationMs")?.as_u64()?;
    let truncated = object.get("truncated")?.as_bool()?;
    let count_is_complete = object.get("countIsComplete")?.as_bool()?;
    (count_is_complete
        && filenames == num_files
        && num_files <= total_matches
        && truncated == (num_files < total_matches))
        .then_some(())
}

fn validate_webfetch_result(data: &Value) -> Option<ValidatedWebFetch<'_>> {
    let object = data.as_object()?;
    let status_code = u16::try_from(object.get("code")?.as_u64()?).ok()?;
    if !(200..=299).contains(&status_code) {
        return None;
    }
    object.get("bytes")?.as_u64()?;
    object.get("durationMs")?.as_u64()?;
    nonempty_string(object.get("codeText")?)?;
    object.get("result")?.as_str()?;
    let locator = nonempty_string(object.get("url")?)?;
    let source = match object.get("source")?.as_str()? {
        "cache" => EvidenceSource::WebFetchCache,
        "network" => EvidenceSource::WebFetchNetwork,
        _ => return None,
    };
    let fetched_at_ms = object.get("fetched_at_ms")?.as_u64()?;
    // These two fields are installed only by WebFetch's trusted Fusion result
    // wrapper. Their presence prevents an ordinary/unwrapped success shape
    // from being accepted as a Fusion observation.
    let truncation_limit = object.get("truncation_limit_bytes")?.as_u64()?;
    if truncation_limit != 64 * 1024 || !object.get("truncated")?.is_boolean() {
        return None;
    }
    Some(ValidatedWebFetch {
        source,
        fetched_at_ms,
        status_code,
        locator,
    })
}

fn nonempty_string(value: &Value) -> Option<&str> {
    value.as_str().filter(|value| !value.is_empty())
}

fn string_array_len(value: &Value) -> Option<usize> {
    let values = value.as_array()?;
    values.iter().all(Value::is_string).then_some(values.len())
}

fn optional_u64(object: &serde_json::Map<String, Value>, key: &str) -> bool {
    object.get(key).is_none_or(Value::is_u64)
}

fn has_failure_marker(object: &serde_json::Map<String, Value>) -> bool {
    object.contains_key("error") || object.contains_key("status")
}

impl<'a> EvidenceMaterial<'a> {
    fn from_tool_result(capability: EvidenceCapability, data: &'a Value) -> Option<Self> {
        let (source, fetched_at_ms, status_code, locator) = match capability {
            EvidenceCapability::Read => (
                EvidenceSource::NativeTool,
                None,
                None,
                Some(validate_read_result(data)?),
            ),
            EvidenceCapability::Grep => {
                validate_grep_result(data)?;
                (EvidenceSource::NativeTool, None, None, None)
            }
            EvidenceCapability::Glob => {
                validate_glob_result(data)?;
                (EvidenceSource::NativeTool, None, None, None)
            }
            EvidenceCapability::WebFetch => {
                let validated = validate_webfetch_result(data)?;
                (
                    validated.source,
                    Some(validated.fetched_at_ms),
                    Some(validated.status_code),
                    Some(validated.locator),
                )
            }
        };
        Some(Self {
            // `RegistryToolInvoker` returns only this Value.  The subagent
            // runner renders strings verbatim and every other Value with
            // `Value::to_string`; snapshot() mirrors that transformation.
            body: MaterialBody::ModelVisibleValue(data),
            source,
            fetched_at_ms,
            status_code,
            locator,
            metadata_complete: true,
        })
    }

    fn snapshot(&self, prefix_limit: usize) -> io::Result<MaterialSnapshot> {
        let mut writer = DigestPrefixWriter::new(prefix_limit);
        match self.body {
            MaterialBody::ModelVisibleValue(Value::String(text)) => {
                writer.write_all(text.as_bytes())?
            }
            MaterialBody::ModelVisibleValue(value) => {
                serde_json::to_writer(&mut writer, value).map_err(io::Error::other)?
            }
        }
        Ok(writer.finish())
    }
}

struct DigestPrefixWriter {
    digest: Sha256,
    prefix: Vec<u8>,
    total: usize,
    prefix_limit: usize,
}

impl DigestPrefixWriter {
    fn new(prefix_limit: usize) -> Self {
        Self {
            digest: Sha256::new(),
            prefix: Vec::with_capacity(prefix_limit),
            total: 0,
            prefix_limit,
        }
    }

    fn finish(self) -> MaterialSnapshot {
        let digest = self.digest.finalize();
        MaterialSnapshot {
            prefix: self.prefix,
            full_len: self.total,
            digest_hex: bytes_to_hex(&digest),
        }
    }
}

impl Write for DigestPrefixWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.digest.update(bytes);
        self.total = self
            .total
            .checked_add(bytes.len())
            .ok_or_else(|| io::Error::other("evidence body length overflow"))?;
        let remaining = self.prefix_limit.saturating_sub(self.prefix.len());
        self.prefix
            .extend_from_slice(&bytes[..bytes.len().min(remaining)]);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Arc;

    fn read_result(content: &str) -> Value {
        json!({
            "type": "text",
            "file": {
                "filePath": "/private/project/a.txt",
                "content": content,
                "numLines": 1,
                "startLine": 1,
                "totalLines": 1,
            }
        })
    }

    fn fetch_result(code: u16, source: &str) -> Value {
        json!({
            "bytes": 5,
            "code": code,
            "codeText": "OK",
            "result": "hello",
            "durationMs": 1,
            "url": "https://example.test/a",
            "source": source,
            "fetched_at_ms": 1234,
            "truncation_limit_bytes": 65536,
            "truncated": false,
        })
    }

    fn grep_result(mode: &str) -> Value {
        match mode {
            "content" => json!({
                "mode": "content",
                "numFiles": 0,
                "filenames": [],
                "content": "src/lib.rs:1:needle",
                "numLines": 1,
                "totalLines": 1,
            }),
            "count" => json!({
                "mode": "count",
                "numFiles": 1,
                "filenames": [],
                "content": "src/lib.rs:1\n\nFound 1 total occurrence across 1 file.",
                "numMatches": 1,
            }),
            "files_with_matches" => json!({
                "mode": "files_with_matches",
                "filenames": ["src/lib.rs"],
                "numFiles": 1,
                "totalFiles": 1,
            }),
            other => json!({"mode": other}),
        }
    }

    fn glob_result() -> Value {
        json!({
            "filenames": ["src/lib.rs"],
            "durationMs": 1,
            "numFiles": 1,
            "truncated": false,
            "totalMatches": 1,
            "countIsComplete": true,
        })
    }

    #[test]
    fn scopes_and_host_ids_are_isolated() {
        let run = EvidenceRun::new();
        let first = run.new_panel();
        let sibling = run.new_panel();
        let first_receipt = first
            .capture_success(
                EvidenceCapability::Read,
                Some("toolu_reused"),
                &read_result("one"),
            )
            .expect("supported read result");
        let second_receipt = first
            .capture_success(
                EvidenceCapability::Read,
                Some("toolu_reused"),
                &read_result("two"),
            )
            .expect("supported read result");

        assert_ne!(
            first_receipt.receipt_ref(),
            second_receipt.receipt_ref(),
            "reused model ids still mint fresh host receipt refs"
        );
        assert!(first.resolve(first_receipt.receipt_ref()).is_some());
        assert!(sibling.resolve(first_receipt.receipt_ref()).is_none());
        assert!(!sibling.owns(&first_receipt));
        let mut forged = first_receipt.clone();
        forged.block_ref = EvidenceBlockRef(Uuid::new_v4());
        assert!(
            !first.owns(&forged),
            "a copied receipt cannot forge its block token"
        );
        assert!(first.correlated_tool_use_id(&first_receipt, "toolu_reused"));
        assert!(first_receipt.fetched);
        assert!(!first_receipt.included_in_request);
        let debug = format!("{first_receipt:?} {first:?}");
        assert!(!debug.contains("/private/project"));
        assert!(!debug.contains("toolu_reused"));
        assert!(!debug.contains(first_receipt.receipt_ref().as_str()));
    }

    #[test]
    fn body_and_digest_match_the_exact_runner_visible_value() {
        let panel = EvidenceRun::new().new_panel();
        let value = read_result("one\n界");
        let expected = value.to_string();
        let receipt = panel
            .capture_success(EvidenceCapability::Read, None, &value)
            .expect("valid native Read result");
        assert_eq!(panel.body(&receipt), Some(expected.as_bytes().to_vec()));
        assert_eq!(receipt.body_bytes, expected.len());
        assert_eq!(receipt.digest_hex, digest_hex(expected.as_bytes()));
        assert_eq!(receipt.kind, EvidenceKind::File);
    }

    #[test]
    fn utf8_and_receipt_caps_retain_truthful_metadata() {
        let run = EvidenceRun::new();
        let panel = run.new_panel();
        let content = "界".repeat(MAX_EVIDENCE_BYTES_PER_RECEIPT);
        let oversized = panel
            .capture_success(EvidenceCapability::Read, None, &read_result(&content))
            .expect("supported read result");
        assert!(oversized.truncated);
        assert!(oversized.stored_bytes <= MAX_EVIDENCE_BYTES_PER_RECEIPT);
        assert!(
            MAX_EVIDENCE_BYTES_PER_RECEIPT - oversized.stored_bytes < "界".len(),
            "UTF-8 truncation should discard only the incomplete suffix"
        );
        assert_eq!(
            panel.body(&oversized).unwrap().len(),
            oversized.stored_bytes
        );
        assert!(std::str::from_utf8(&panel.body(&oversized).unwrap()).is_ok());
        assert!(oversized.metadata_complete);
        assert_eq!(run.bytes_used(), panel.bytes_used());

        for _ in 0..4 {
            panel
                .capture_success(
                    EvidenceCapability::Read,
                    None,
                    &read_result(&"x".repeat(MAX_EVIDENCE_BYTES_PER_RECEIPT)),
                )
                .expect("metadata receipt survives panel cap");
        }
        let exhausted = panel
            .capture_success(EvidenceCapability::Read, None, &read_result("after cap"))
            .expect("metadata remains after cap exhaustion");
        assert_eq!(exhausted.stored_bytes, 0);
        assert!(exhausted.truncated);
        assert!(exhausted.metadata_complete);
        assert_eq!(panel.body(&exhausted), Some(Vec::new()));
        assert!(panel.bytes_used() <= MAX_EVIDENCE_BYTES_PER_PANEL);
        assert!(run.bytes_used() <= MAX_EVIDENCE_BYTES_PER_RUN);
    }

    #[test]
    fn concurrent_panels_share_an_exact_run_cap_and_keep_zero_byte_metadata() {
        let run = EvidenceRun::new();
        let panels: Vec<_> = (0..9).map(|_| run.new_panel()).collect();
        let value = Arc::new(read_result(&"x".repeat(MAX_EVIDENCE_BYTES_PER_RECEIPT)));
        std::thread::scope(|scope| {
            for panel in panels {
                let value = Arc::clone(&value);
                scope.spawn(move || {
                    for _ in 0..5 {
                        panel
                            .capture_success(EvidenceCapability::Read, None, value.as_ref())
                            .expect("valid capture retains at least metadata");
                    }
                    assert!(panel.bytes_used() <= MAX_EVIDENCE_BYTES_PER_PANEL);
                });
            }
        });
        assert_eq!(run.bytes_used(), MAX_EVIDENCE_BYTES_PER_RUN);

        let zero_remaining = run
            .new_panel()
            .capture_success(
                EvidenceCapability::Read,
                None,
                &read_result("after run cap"),
            )
            .expect("run exhaustion retains metadata");
        assert_eq!(zero_remaining.stored_bytes, 0);
        assert!(zero_remaining.truncated);
        assert!(zero_remaining.metadata_complete);
    }

    #[test]
    fn only_reviewed_native_success_shapes_are_captured() {
        let panel = EvidenceRun::new().new_panel();
        for mode in ["content", "count", "files_with_matches"] {
            assert!(panel
                .capture_success(EvidenceCapability::Grep, None, &grep_result(mode))
                .is_some());
        }
        assert!(panel
            .capture_success(EvidenceCapability::Glob, None, &glob_result())
            .is_some());

        let mut unknown_mode = grep_result("unknown");
        unknown_mode["content"] = json!("not a recognized success");
        assert!(panel
            .capture_success(EvidenceCapability::Grep, None, &unknown_mode)
            .is_none());
        let mut failed_grep = grep_result("content");
        failed_grep["status"] = json!("failed");
        assert!(panel
            .capture_success(EvidenceCapability::Grep, None, &failed_grep)
            .is_none());
        let mut malformed_grep = grep_result("files_with_matches");
        malformed_grep["numFiles"] = json!(2);
        assert!(panel
            .capture_success(EvidenceCapability::Grep, None, &malformed_grep)
            .is_none());

        assert!(panel
            .capture_success(EvidenceCapability::Glob, None, &json!({}))
            .is_none());
        let mut inconsistent_glob = glob_result();
        inconsistent_glob["truncated"] = json!(true);
        assert!(panel
            .capture_success(EvidenceCapability::Glob, None, &inconsistent_glob)
            .is_none());

        let mut malformed_read = read_result("text");
        malformed_read["type"] = json!("image");
        assert!(panel
            .capture_success(EvidenceCapability::Read, None, &malformed_read)
            .is_none());
        assert!(panel
            .capture_success(
                EvidenceCapability::Glob,
                None,
                &read_result("wrong extractor")
            )
            .is_none());
    }

    #[test]
    fn webfetch_only_captures_final_success_and_preserves_cache_metadata() {
        let panel = EvidenceRun::new().new_panel();
        assert!(panel
            .capture_success(
                EvidenceCapability::WebFetch,
                None,
                &fetch_result(500, "network")
            )
            .is_none());
        assert!(panel
            .capture_success(
                EvidenceCapability::WebFetch,
                None,
                &fetch_result(302, "network")
            )
            .is_none());
        let cached = panel
            .capture_success(
                EvidenceCapability::WebFetch,
                None,
                &fetch_result(200, "cache"),
            )
            .expect("2xx cache result");
        assert_eq!(cached.source, EvidenceSource::WebFetchCache);
        assert_eq!(cached.fetched_at_ms, Some(1234));
        assert_eq!(cached.status_code, Some(200));
        assert!(!cached.included_in_request);
        assert!(cached.locator_digest_hex().is_some());

        for missing in [
            "source",
            "fetched_at_ms",
            "truncation_limit_bytes",
            "truncated",
        ] {
            let mut malformed = fetch_result(200, "network");
            malformed.as_object_mut().unwrap().remove(missing);
            assert!(panel
                .capture_success(EvidenceCapability::WebFetch, None, &malformed)
                .is_none());
        }
        assert!(panel
            .capture_success(
                EvidenceCapability::WebFetch,
                None,
                &fetch_result(200, "unknown")
            )
            .is_none());
    }

    #[test]
    fn freeze_stops_capture_without_erasing_prior_metadata() {
        let panel = EvidenceRun::new().new_panel();
        let receipt = panel
            .capture_success(
                EvidenceCapability::Read,
                None,
                &read_result("before freeze"),
            )
            .expect("capture before freeze");
        panel.freeze();
        assert!(panel.is_frozen());
        assert!(panel
            .capture_success(EvidenceCapability::Read, None, &read_result("after freeze"))
            .is_none());
        assert!(panel.resolve(receipt.receipt_ref()).is_some());
    }
}
