//! M7-10 behavior: paste coalescing + image-ref insertion.
//! - A multi-line paste inserts as ONE block (no per-line submit).
//! - An image payload → `[Image #N]` ref + recorded attachment metadata.
//! - The counter increments across multiple images.

use std::time::{Duration, Instant};

use lingxi_tui::components::prompt_input::{
    apply_paste_block, AttachmentKind, PasteCoalescer, PasteState,
};

/// Feed a string into a coalescer as a tight 1ms-apart burst, then flush.
/// Returns the single coalesced block (proving multi-line stays one unit).
fn coalesce_burst(text: &str) -> String {
    let base = Instant::now();
    let mut c = PasteCoalescer::new();
    for (i, ch) in text.chars().enumerate() {
        let now = base + Duration::from_millis(i as u64); // 1ms apart → one burst
        assert_eq!(
            c.push_char(ch, now),
            None,
            "burst should not flush mid-stream"
        );
    }
    c.flush_now().expect("non-empty burst flushes")
}

#[test]
fn multiline_paste_coalesces_to_single_block() {
    let block = coalesce_burst("line1\nline2\nline3");
    assert_eq!(block, "line1\nline2\nline3", "the whole paste is one block");
    // Inserting it is a single mutation — there is no submit in this path.
    let r = apply_paste_block("", 0, &block, PasteState::default());
    assert_eq!(r.prompt, "line1\nline2\nline3");
    assert_eq!(r.cursor, "line1\nline2\nline3".len());
}

#[test]
fn image_payload_inserts_ref_and_records_metadata() {
    let block = coalesce_burst("/tmp/screenshot.png");
    let r = apply_paste_block("here: ", 6, &block, PasteState::default());
    assert_eq!(r.prompt, "here: [Image #1]");
    assert_eq!(r.state.attachments.len(), 1);
    assert_eq!(r.state.attachments[0].id, 1);
    assert_eq!(r.state.attachments[0].kind, AttachmentKind::Image);
    assert_eq!(r.state.attachments[0].source, "/tmp/screenshot.png");
}

#[test]
fn counter_increments_for_multiple_images() {
    let mut state = PasteState::default();
    let r1 = apply_paste_block("", 0, &coalesce_burst("/a/one.png"), state);
    assert_eq!(r1.prompt, "[Image #1]");
    state = r1.state.clone();
    let r2 = apply_paste_block(&r1.prompt, r1.cursor, &coalesce_burst("/b/two.jpg"), state);
    assert_eq!(r2.prompt, "[Image #1][Image #2]");
    assert_eq!(r2.state.attachments.len(), 2);
    assert_eq!(r2.state.next_image_id, 3);
}
