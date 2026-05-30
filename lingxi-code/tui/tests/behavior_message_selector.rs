//! M7-14 behavior: search overlay routing, jump-back, export safety.

use tui::components::message_selector::{
    export_transcript, handle_message_selector_key, message_line_offset, ExportError,
    SelectorAction, SelectorMode,
};
use tui::components::virtual_message_list::HeightCache;
use tui::state::{AppState, RenderedMessage};

mod support;
use support::fake_status;

fn push(st: &mut AppState, body: &str) {
    st.push_message(RenderedMessage::UserText {
        body: body.into(),
        timestamp: 0,
    });
}

/// Build an iocraft (crossterm-0.29) key-press event — the exact event type
/// the live `use_terminal_events` closure feeds `handle_live_key`.
fn live_key(code: iocraft::prelude::KeyCode) -> iocraft::prelude::KeyEvent {
    use iocraft::prelude::{KeyEvent, KeyEventKind, KeyModifiers};
    let mut k = KeyEvent::new(KeyEventKind::Press, code);
    k.modifiers = KeyModifiers::NONE;
    k
}

/// Render the live overlay to a string via the real `render_screen` seam,
/// so assertions exercise the same component the binary draws.
fn render_overlay(st: &AppState) -> String {
    use iocraft::ElementExt;
    let mut el = tui::app::render_screen(st, 20, 80);
    el.to_string()
}

#[test]
fn selecting_a_match_sets_scroll_offset_to_that_message() {
    let mut st = AppState::new(fake_status());
    for i in 0..40 {
        push(&mut st, &format!("line {i}"));
    }
    st.refresh_height_cache(80); // M7-03: populate the line cache
    let vh = 10;

    st.message_selector.open();
    // Type "line 5" → matches "line 5".
    for c in "line 5".chars() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        handle_message_selector_key(
            &mut st.message_selector,
            &st.messages,
            KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE),
        );
    }
    // The first filtered hit is message index 5.
    let target = st.message_selector.filtered[st.message_selector.selected_filtered];
    assert_eq!(target, 5);

    // Jump: caller sets scroll_offset from message_line_offset.
    let cache = HeightCache::build(&st.messages, 80);
    let offset = message_line_offset(&st.messages, &cache, target, vh);
    st.scroll_offset = offset;
    // 40 one-line msgs: line_at_start[5]=5, total 40 → offset = 40-5-10 = 25.
    assert_eq!(st.scroll_offset, 25);
}

#[test]
fn esc_closes_the_overlay_without_a_jump() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut st = AppState::new(fake_status());
    push(&mut st, "hello");
    st.message_selector.open();
    st.message_selector.refilter_all(&st.messages);
    let action = handle_message_selector_key(
        &mut st.message_selector,
        &st.messages.clone(),
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
    );
    assert_eq!(action, SelectorAction::Close);
    assert!(!st.message_selector.open);
}

#[test]
fn export_default_path_and_overwrite_confirm() {
    use std::fs;
    use tempfile::TempDir;
    let tmp = TempDir::new().unwrap();
    let msgs = vec![
        RenderedMessage::UserText {
            body: "q".into(),
            timestamp: 0,
        },
        RenderedMessage::AssistantText {
            body: "a".into(),
            timestamp: 0,
        },
    ];
    // First export writes the file.
    let p = export_transcript(&msgs, tmp.path(), "out.txt", false).unwrap();
    assert!(p.exists());
    // Second export to the same name without overwrite → refused, file kept.
    let original = fs::read_to_string(&p).unwrap();
    match export_transcript(&msgs, tmp.path(), "out.txt", false) {
        Err(ExportError::Exists(_)) => {}
        other => panic!("expected Exists, got {other:?}"),
    }
    assert_eq!(fs::read_to_string(&p).unwrap(), original);
}

// ─────────────────────────────────────────────────────────────────────────
// M7-14 REVIEW: the export flow must be REACHABLE and actually export.
// These drive the REAL seams (`dispatch` for the `/export` submit-intercept,
// `handle_live_key` for the priority-3 overlay key path). They FAIL against
// the pre-fix code where `/export` opened the plain search box and no key
// path ever called `export_transcript`.
// ─────────────────────────────────────────────────────────────────────────

/// Type a string into the live export flow via `handle_live_key`.
fn type_live(st: &mut AppState, text: &str) {
    for c in text.chars() {
        tui::root::handle_live_key(st, &live_key(iocraft::prelude::KeyCode::Char(c)), 20);
    }
}

/// (1) `/export` submit-intercept opens the export flow DIRECTLY (export
/// mode, "Export Conversation" + "Enter filename:" rendered) — NOT the plain
/// search box. Pre-fix this opened `MessageSelectorState::open()` (search).
#[test]
fn slash_export_opens_export_flow_not_search() {
    use tui::app::dispatch;
    use tui::events::keymap::KeyAction;
    let mut st = AppState::new(fake_status());
    push(&mut st, "hello");

    st.prompt_text = "/export".to_string();
    st.prompt_cursor = "/export".len();
    let should_run = dispatch(KeyAction::Submit, &mut st);
    assert!(!should_run, "/export opens a flow, never runs a turn");
    assert!(st.message_selector.open, "overlay must be open");
    assert_eq!(
        st.message_selector.mode,
        SelectorMode::Export,
        "/export opens the EXPORT flow, not the search box"
    );
    assert!(
        st.prompt_text.is_empty(),
        "prompt cleared on the intercepted slash"
    );
    // The default filename is pre-filled, ready to edit.
    assert!(
        std::path::Path::new(&st.message_selector.export.filename)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("txt")),
        "default filename pre-filled, got {:?}",
        st.message_selector.export.filename
    );

    let rendered = render_overlay(&st);
    assert!(
        rendered.contains("Export Conversation"),
        "title shown, got: {rendered}"
    );
    assert!(
        rendered.contains("Enter filename:"),
        "filename prompt shown, got: {rendered}"
    );
}

/// (2) Export flow → edit filename → Enter writes the file to the (injected
/// temp) export dir with the transcript content, and surfaces
/// "Conversation exported to: {path}". Drives the live key path end-to-end.
#[test]
fn export_flow_enter_writes_file_and_reports_path() {
    use std::fs;
    use tempfile::TempDir;
    let tmp = TempDir::new().unwrap();

    let mut st = AppState::new(fake_status());
    push(&mut st, "what is 2+2");
    st.push_message(RenderedMessage::AssistantText {
        body: "4".into(),
        timestamp: 0,
    });

    // Open the export flow + inject the temp export dir (no `~` touch).
    st.message_selector.open_export();
    st.message_selector.export_dir_override = Some(tmp.path().to_path_buf());
    // Replace the timestamped default with a deterministic name.
    st.message_selector.export.filename.clear();
    type_live(&mut st, "myexport.txt");
    assert_eq!(st.message_selector.export.filename, "myexport.txt");

    // Enter confirms → write happens on the live key path.
    tui::root::handle_live_key(&mut st, &live_key(iocraft::prelude::KeyCode::Enter), 20);

    let target = tmp.path().join("myexport.txt");
    assert!(target.exists(), "file must be written to the export dir");
    let body = fs::read_to_string(&target).unwrap();
    assert!(body.contains("what is 2+2"), "transcript content present");
    assert!(body.contains('4'), "assistant text present");

    // Success status surfaces the literal-locked string with the path.
    let status = st.message_selector.export.status.clone().unwrap();
    assert_eq!(
        status,
        format!("Conversation exported to: {}", target.display())
    );
    let rendered = render_overlay(&st);
    assert!(
        rendered.contains("Conversation exported to:"),
        "success line rendered, got: {rendered}"
    );
}

/// (3) §4 R10: exporting onto an EXISTING file shows the overwrite-confirm
/// prompt; WITHOUT confirm the file is unchanged; WITH `y` it is overwritten.
#[test]
fn export_flow_overwrite_confirm_protects_existing_file() {
    use std::fs;
    use tempfile::TempDir;
    let tmp = TempDir::new().unwrap();
    let target = tmp.path().join("dup.txt");
    fs::write(&target, b"ORIGINAL").unwrap();

    let mut st = AppState::new(fake_status());
    push(&mut st, "new transcript content");

    st.message_selector.open_export();
    st.message_selector.export_dir_override = Some(tmp.path().to_path_buf());
    st.message_selector.export.filename.clear();
    type_live(&mut st, "dup.txt");

    // First Enter → target exists, write REFUSED, overwrite prompt armed.
    tui::root::handle_live_key(&mut st, &live_key(iocraft::prelude::KeyCode::Enter), 20);
    assert!(
        st.message_selector.export.awaiting_overwrite,
        "must arm the overwrite-confirm prompt"
    );
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        "ORIGINAL",
        "§4 R10: file must be UNCHANGED before explicit confirm"
    );
    let rendered = render_overlay(&st);
    assert!(
        rendered.contains("already exists") && rendered.contains("Overwrite?"),
        "overwrite prompt rendered, got: {rendered}"
    );

    // Pressing 'n' cancels on a fresh state armed at the same prompt — the
    // file stays unchanged (AppState isn't Clone, so rebuild the scenario).
    {
        let mut st_no = AppState::new(fake_status());
        push(&mut st_no, "different content");
        st_no.message_selector.open_export();
        st_no.message_selector.export_dir_override = Some(tmp.path().to_path_buf());
        st_no.message_selector.export.filename.clear();
        type_live(&mut st_no, "dup.txt");
        tui::root::handle_live_key(&mut st_no, &live_key(iocraft::prelude::KeyCode::Enter), 20);
        assert!(st_no.message_selector.export.awaiting_overwrite);
        tui::root::handle_live_key(
            &mut st_no,
            &live_key(iocraft::prelude::KeyCode::Char('n')),
            20,
        );
        assert_eq!(
            fs::read_to_string(&target).unwrap(),
            "ORIGINAL",
            "declining overwrite leaves the file untouched"
        );
        assert_eq!(
            st_no.message_selector.export.status.as_deref(),
            Some("Export cancelled"),
            "declining surfaces the cancel literal"
        );
    }

    // Pressing 'y' confirms → now it is overwritten.
    tui::root::handle_live_key(&mut st, &live_key(iocraft::prelude::KeyCode::Char('y')), 20);
    let body = fs::read_to_string(&target).unwrap();
    assert!(
        body.contains("new transcript content"),
        "confirmed overwrite replaces the file, got: {body:?}"
    );
    assert!(
        st.message_selector
            .export
            .status
            .as_deref()
            .unwrap()
            .starts_with("Conversation exported to:"),
        "success after confirm"
    );
}

/// (4) Esc cancels the export flow: nothing is written and "Export cancelled"
/// is surfaced.
#[test]
fn export_flow_esc_cancels_and_writes_nothing() {
    use tempfile::TempDir;
    let tmp = TempDir::new().unwrap();

    let mut st = AppState::new(fake_status());
    push(&mut st, "secret");
    st.message_selector.open_export();
    st.message_selector.export_dir_override = Some(tmp.path().to_path_buf());
    st.message_selector.export.filename.clear();
    type_live(&mut st, "wont-write.txt");

    // Esc cancels.
    tui::root::handle_live_key(&mut st, &live_key(iocraft::prelude::KeyCode::Esc), 20);
    assert_eq!(
        st.message_selector.export.status.as_deref(),
        Some("Export cancelled"),
        "Esc surfaces the cancel literal"
    );
    let rendered = render_overlay(&st);
    assert!(
        rendered.contains("Export cancelled"),
        "cancel line rendered, got: {rendered}"
    );
    // No file was created in the export dir.
    assert!(
        !tmp.path().join("wont-write.txt").exists(),
        "Esc must write nothing"
    );
    // The next key dismisses the overlay (closes).
    tui::root::handle_live_key(&mut st, &live_key(iocraft::prelude::KeyCode::Char(' ')), 20);
    assert!(!st.message_selector.open, "overlay closed after dismiss");
}
