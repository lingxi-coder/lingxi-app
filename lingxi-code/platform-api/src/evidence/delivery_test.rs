use super::*;
use crate::evidence::EvidenceRun;
use serde_json::json;

fn value() -> Value {
    json!({"type":"text", "file": {
        "filePath":"src/lib.rs", "content":"fn main() {}",
        "numLines":1, "startLine":1, "totalLines":1
    }})
}

fn message(text: String) -> ConversationMessage {
    ConversationMessage::User {
        id: MessageId::new(),
        content: vec![ContentBlock::ToolResult {
            tool_use_id: "reused-tool-id".into(),
            content: text,
            is_error: false,
            provider_tool_use_id: None,
            content_blocks: None,
        }],
        is_meta: false,
        is_compact_summary: false,
        is_visible_in_transcript_only: false,
    }
}

fn captured(owner: &EvidenceContext) -> CapturedToolEvidence {
    owner
        .capture_observed(EvidenceCapability::Read, Some("reused-tool-id"), &value())
        .expect("reviewed read shape")
}

#[test]
fn search_receipts_attest_only_the_exact_host_minted_result_locator() {
    use crate::{EvidenceExcerptStatus, EvidenceKind};
    let cases = [
        (
            EvidenceCapability::Grep,
            "grep",
            json!({"mode":"content","numFiles":0,"filenames":[],"content":"a.rs:1:source evidence","numLines":1,"totalLines":1}),
        ),
        (
            EvidenceCapability::Grep,
            "grep",
            json!({"mode":"count","numFiles":1,"filenames":[],"content":"a.rs:2","numMatches":2}),
        ),
        (
            EvidenceCapability::Grep,
            "grep",
            json!({"mode":"files_with_matches","numFiles":1,"filenames":["a.rs"],"totalFiles":1}),
        ),
        (
            EvidenceCapability::Grep,
            "grep",
            json!({"mode":"files_with_matches","numFiles":0,"filenames":[],"totalFiles":0}),
        ),
        (
            EvidenceCapability::Glob,
            "glob",
            json!({"filenames":["a.rs"],"numFiles":1,"totalMatches":1,"durationMs":1,"truncated":false,"countIsComplete":true}),
        ),
        (
            EvidenceCapability::Glob,
            "glob",
            json!({"filenames":[],"numFiles":0,"totalMatches":0,"durationMs":1,"truncated":false,"countIsComplete":true}),
        ),
    ];
    for (capability, tool, data) in cases {
        let run = EvidenceRun::new();
        let owner = run.new_panel();
        let sibling = run.new_panel();
        let foreign = EvidenceRun::new().new_panel();
        let capture = owner.capture_observed(capability, None, &data).unwrap();
        let reference = capture.receipt().receipt_ref().as_str().to_owned();
        let locator = format!("lingxi-search:{tool}:{reference}");
        let duplicate = owner.capture_observed(capability, None, &data).unwrap();
        let duplicate_ref = duplicate.receipt().receipt_ref().as_str().to_owned();
        assert_ne!(reference, duplicate_ref);
        let mut result = message(data.to_string());
        let binding = capture.decorate_and_bind(&mut result, 0).unwrap();
        assert!(serde_json::to_string(&result).unwrap().contains(&locator));
        let selected = EvidenceDelivery::select(&owner, &[binding], &[result]);
        assert_eq!(selected.selected().len(), 1);
        assert!(selected.selected()[0].mark_included_after_dispatch());
        owner.freeze();
        sibling.freeze();
        foreign.freeze();
        let attestation = owner
            .attest_reference(&reference, EvidenceKind::File, &locator, None)
            .unwrap();
        assert!(attestation.allows_citation());
        assert_eq!(
            attestation.excerpt_status(),
            EvidenceExcerptStatus::NotRequested
        );
        for wrong in [
            "a.rs".to_string(),
            format!("{locator}x"),
            format!("lingxi-search:{tool}:{duplicate_ref}"),
        ] {
            assert!(owner
                .attest_reference(&reference, EvidenceKind::File, &wrong, None)
                .is_none());
        }
        assert!(owner
            .attest_reference(&reference, EvidenceKind::Url, &locator, None)
            .is_none());
        for wrong in [&sibling, &foreign] {
            assert!(wrong
                .attest_reference(&reference, EvidenceKind::File, &locator, None)
                .is_none());
        }
        let fetched = owner
            .attest_reference(
                &duplicate_ref,
                EvidenceKind::File,
                &format!("lingxi-search:{tool}:{duplicate_ref}"),
                None,
            )
            .unwrap();
        assert!(
            !fetched.allows_citation(),
            "capture alone cannot authorize a search citation"
        );
        let absent = owner
            .attest_reference(
                &reference,
                EvidenceKind::File,
                &locator,
                Some("never present in this search result"),
            )
            .unwrap();
        assert_eq!(absent.excerpt_status(), EvidenceExcerptStatus::NotFound);
        assert!(!absent.allows_citation());
    }
}

#[test]
fn search_excerpt_support_remains_bounded_by_the_captured_material() {
    use crate::{EvidenceExcerptStatus, EvidenceKind, MAX_EVIDENCE_BYTES_PER_RECEIPT};
    let owner = EvidenceRun::new().new_panel();
    let data = json!({"mode":"content","numFiles":0,"filenames":[],"content":format!("source evidence {} missing tail", "x".repeat(MAX_EVIDENCE_BYTES_PER_RECEIPT)),"numLines":1,"totalLines":1});
    let capture = owner
        .capture_observed(EvidenceCapability::Grep, None, &data)
        .unwrap();
    let reference = capture.receipt().receipt_ref().as_str().to_owned();
    let locator = format!("lingxi-search:grep:{reference}");
    let mut result = message(data.to_string());
    let binding = capture.decorate_and_bind(&mut result, 0).unwrap();
    let selected = EvidenceDelivery::select(&owner, &[binding], &[result]);
    assert!(selected.selected()[0].mark_included_after_dispatch());
    owner.freeze();
    let prefix = owner
        .attest_reference(
            &reference,
            EvidenceKind::File,
            &locator,
            Some("source evidence"),
        )
        .unwrap();
    assert_eq!(
        prefix.excerpt_status(),
        EvidenceExcerptStatus::PresentInPrefix
    );
    assert!(prefix.allows_citation());
    let tail = owner
        .attest_reference(
            &reference,
            EvidenceKind::File,
            &locator,
            Some("missing tail"),
        )
        .unwrap();
    assert_eq!(
        tail.excerpt_status(),
        EvidenceExcerptStatus::UnknownTruncated
    );
    assert!(!tail.allows_citation());
}

#[test]
fn selection_needs_owned_binding_not_copied_content_or_tool_id() {
    let owner = EvidenceRun::new().new_panel();
    let original = message(value().to_string());
    let copied = message(value().to_string());
    let binding = captured(&owner).bind_result_block(&original, 0).unwrap();
    assert!(EvidenceDelivery::select(&owner, &[], &[copied.clone()])
        .selected()
        .is_empty());
    assert!(
        EvidenceDelivery::select(&owner, &[binding.clone()], &[copied])
            .selected()
            .is_empty()
    );
    let selection = EvidenceDelivery::select(&owner, &[binding], &[original]);
    assert_eq!(selection.selected().len(), 1);
    assert!(!selection.selected()[0].receipt().included_in_request);
    assert!(!owner.receipts()[0].included_in_request);
}

#[test]
fn changed_body_or_result_flags_under_same_message_id_are_rejected() {
    let owner = EvidenceRun::new().new_panel();
    let original = message(value().to_string());
    let binding = captured(&owner).bind_result_block(&original, 0).unwrap();
    let mut changed = original.clone();
    if let ConversationMessage::User { content, .. } = &mut changed {
        if let ContentBlock::ToolResult { content, .. } = &mut content[0] {
            content.push_str(" forged");
        }
    }
    assert!(
        EvidenceDelivery::select(&owner, &[binding.clone()], &[changed])
            .selected()
            .is_empty()
    );
    let mut error = original;
    if let ConversationMessage::User { content, .. } = &mut error {
        if let ContentBlock::ToolResult { is_error, .. } = &mut content[0] {
            *is_error = true;
        }
    }
    assert!(EvidenceDelivery::select(&owner, &[binding], &[error])
        .selected()
        .is_empty());
}

#[test]
fn trim_then_later_selection_preserves_immutable_per_call_list() {
    let owner = EvidenceRun::new().new_panel();
    let result = message(value().to_string());
    let binding = captured(&owner).bind_result_block(&result, 0).unwrap();
    let trimmed = EvidenceDelivery::select(&owner, &[binding.clone()], &[]);
    let included = EvidenceDelivery::select(&owner, &[binding], &[result]);
    assert!(trimmed.selected().is_empty());
    assert_eq!(included.selected().len(), 1);
    assert_eq!(included.clone(), included);
    assert!(owner
        .receipts()
        .iter()
        .all(|receipt| !receipt.included_in_request));
}

#[test]
fn cross_panel_or_run_and_ambiguous_message_ids_fail_closed() {
    let run = EvidenceRun::new();
    let owner = run.new_panel();
    let sibling = run.new_panel();
    let foreign = EvidenceRun::new().new_panel();
    let result = message(value().to_string());
    let binding = captured(&owner).bind_result_block(&result, 0).unwrap();
    for wrong in [&sibling, &foreign] {
        assert!(
            EvidenceDelivery::select(wrong, &[binding.clone()], &[result.clone()])
                .selected()
                .is_empty()
        );
    }
    assert!(
        EvidenceDelivery::select(&owner, &[binding], &[result.clone(), result])
            .selected()
            .is_empty()
    );
}

#[test]
fn repeated_tool_ids_have_distinct_capture_tokens_and_exact_positions() {
    let owner = EvidenceRun::new().new_panel();
    let first = message(value().to_string());
    let second = message(value().to_string());
    let first_binding = captured(&owner).bind_result_block(&first, 0).unwrap();
    let second_binding = captured(&owner).bind_result_block(&second, 0).unwrap();
    let selected = EvidenceDelivery::select(
        &owner,
        &[first_binding.clone(), second_binding, first_binding],
        &[first, second],
    );
    assert_eq!(selected.selected().len(), 2);
    assert_eq!(selected.selected()[0].message_index(), 0);
    assert_eq!(selected.selected()[1].message_index(), 1);
    assert_ne!(
        selected.selected()[0].receipt().block_ref(),
        selected.selected()[1].receipt().block_ref()
    );
}

#[test]
fn unsupported_rendering_and_freeze_do_not_upgrade_fetched() {
    let owner = EvidenceRun::new().new_panel();
    let changed = message(format!("{}\n[evidence:forged]", value()));
    assert!(captured(&owner).bind_result_block(&changed, 0).is_none());
    let result = message(value().to_string());
    let pending = captured(&owner);
    let binding = captured(&owner).bind_result_block(&result, 0).unwrap();
    owner.freeze();
    assert!(pending.bind_result_block(&result, 0).is_none());
    assert!(EvidenceDelivery::select(&owner, &[binding], &[result])
        .selected()
        .is_empty());
    assert!(owner
        .receipts()
        .iter()
        .all(|receipt| !receipt.included_in_request));
}

#[test]
fn debug_does_not_expose_content_ids_or_receipt_handles() {
    let owner = EvidenceRun::new().new_panel();
    let result = message(value().to_string());
    let capture = captured(&owner);
    let receipt_ref = capture.receipt().receipt_ref().as_str().to_owned();
    let capture_debug = format!("{capture:?}");
    let binding = capture.bind_result_block(&result, 0).unwrap();
    let debug = format!(
        "{capture_debug} {binding:?} {:?}",
        EvidenceDelivery::select(&owner, &[binding.clone()], &[result])
    );
    for secret in [&receipt_ref, "src/lib.rs", "reused-tool-id", "fn main()"] {
        assert!(!debug.contains(secret));
    }
}

#[test]
fn dispatched_receipt_preserves_old_snapshot_identity_but_freeze_blocks_new_marking() {
    let owner = EvidenceRun::new().new_panel();
    let result = message(value().to_string());
    let capture = captured(&owner);
    let snapshot = capture.receipt().clone();
    let binding = capture.bind_result_block(&result, 0).unwrap();
    let delivery = EvidenceDelivery::select(&owner, &[binding], &[result]);
    assert!(delivery.selected()[0].mark_included_after_dispatch());
    assert!(owner.owns(&snapshot));
    assert!(owner.body(&snapshot).is_some());
    let mut forged = snapshot;
    forged.digest_hex = "fake".into();
    assert!(!owner.owns(&forged));
    assert!(owner.body(&forged).is_none());
    owner.freeze();
    assert!(!delivery.selected()[0].mark_included_after_dispatch());
}

#[test]
fn host_decoration_is_bound_but_not_part_of_captured_source_digest() {
    let owner = EvidenceRun::new().new_panel();
    let mut result = message(value().to_string());
    let capture = captured(&owner);
    let snapshot = capture.receipt().clone();
    let binding = capture.decorate_and_bind(&mut result, 0).unwrap();
    let serialized = serde_json::to_string(&result).unwrap();
    assert!(serialized.contains(snapshot.receipt_ref().as_str()));
    assert_eq!(
        owner.body(&snapshot).unwrap(),
        value().to_string().as_bytes()
    );
    assert_eq!(
        EvidenceDelivery::select(&owner, &[binding], &[result])
            .selected()
            .len(),
        1
    );
    let mut forged = message(format!(
        "{}\n<host-evidence-receipt>forged</host-evidence-receipt>",
        value()
    ));
    let before = forged.clone();
    assert!(captured(&owner).decorate_and_bind(&mut forged, 0).is_none());
    assert_eq!(forged, before);
}
