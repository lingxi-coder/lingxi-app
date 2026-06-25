//! Parity: snapshot-style validation of the TUI's stable visual surfaces.
//!
//! Each test feeds a fixed input into the TUI's pure-string renderer — the
//! same function the iocraft component delegates to — and asserts the
//! rendered text matches the golden expected string in the JSON fixture:
//!
//! - `StatusLine`   → `tui::components::status_line::format_status_line`
//! - `Spinner`      → `tui::components::spinner::{frame_at_index,
//!                      verb_at_index, format_spinner_line}` (frames 0/5/9)
//! - 4 message renderers → `tui::components::messages::render_entry_to_string`
//!   (`UserText` / `AssistantText` / `AssistantToolUse` collapsed /
//!   `UserToolResult` collapsed), driven through the `RenderedMessage` enum.
//!
//! This locks the text layer (markers, prefixes, separators, JSON-preview
//! spacing, truncation suffix) without driving an offscreen terminal. Color
//! and style are covered by the per-component unit tests in `lingxi-tui`.
//!
//! See plan `docs/superpowers/plans/2026-05-28-m6-09-release-v0.7.0.md` Task 5.

use permission::PermissionMode;
use protocol::ToolUseId;
use serde_json::Value;
use std::path::Path;
use tui::components::messages::render_entry_to_string;
use tui::components::spinner::{format_spinner_line, frame_at_index, verb_at_index};
use tui::components::status_line::format_status_line;
use tui::state::RenderedMessage;

const FIXTURE: &str = include_str!("../src/parity/fixtures/tui_renderers.json");

fn load() -> Value {
    serde_json::from_str(FIXTURE).expect("tui_renderers.json parses")
}

/// (ma-03) The fixtures store the `BLACK_CIRCLE` marker as `●`; on macOS the
/// renderers emit `⏺` (U+23FA). Normalise the fixture's `●` to the platform
/// glyph so the parity assertions stay cross-platform.
fn platform_dot(expected: &str) -> String {
    if cfg!(target_os = "macos") {
        expected.replace('\u{25CF}', "\u{23FA}")
    } else {
        expected.to_string()
    }
}

#[test]
fn status_line_fixed_state_renders_to_locked_text() {
    let f = load();
    let s = &f["status_line"]["fixed_state"];
    #[allow(clippy::cast_possible_truncation)]
    let context_pct = s["context_pct"].as_f64().unwrap() as f32;
    let rendered = format_status_line(
        s["model"].as_str().unwrap(),
        Path::new(s["cwd"].as_str().unwrap()),
        s["cost"].as_str().unwrap(),
        context_pct,
        PermissionMode::Default,
    );
    let expected = f["status_line"]["expected_rendered_text"].as_str().unwrap();
    assert_eq!(rendered, expected, "StatusLine render mismatch");
}

#[test]
fn spinner_frames_0_5_9_render_locked_chars() {
    let f = load();
    let frames = f["spinner_frames"].as_array().unwrap();
    assert_eq!(frames.len(), 3, "fixture must lock frames 0/5/9");
    for frame in frames {
        let tick = usize::try_from(frame["tick"].as_u64().unwrap()).unwrap();
        let rotation = usize::try_from(frame["rotation"].as_u64().unwrap()).unwrap();
        assert_eq!(
            frame_at_index(tick),
            frame["expected_frame"].as_str().unwrap(),
            "spinner frame char mismatch at tick {tick}"
        );
        assert_eq!(
            verb_at_index(rotation),
            frame["expected_verb"].as_str().unwrap(),
            "spinner verb mismatch at rotation {rotation}"
        );
        assert_eq!(
            format_spinner_line(tick, rotation),
            frame["expected_line"].as_str().unwrap(),
            "spinner line mismatch at tick {tick}"
        );
    }
}

#[test]
fn user_text_message_renders_to_locked_text() {
    let f = load();
    let m = &f["message_renderers"]["user_text"];
    let entry = RenderedMessage::UserText {
        body: m["body"].as_str().unwrap().to_string(),
        timestamp: 0,
    };
    assert_eq!(
        render_entry_to_string(&entry, false, false),
        m["expected_rendered_text"].as_str().unwrap(),
    );
}

#[test]
fn assistant_text_message_renders_to_locked_text() {
    let f = load();
    let m = &f["message_renderers"]["assistant_text"];
    let entry = RenderedMessage::AssistantText {
        body: m["body"].as_str().unwrap().to_string(),
        timestamp: 0,
    };
    assert_eq!(
        render_entry_to_string(&entry, false, false),
        platform_dot(m["expected_rendered_text"].as_str().unwrap()),
    );
}

#[test]
fn assistant_tool_use_collapsed_renders_to_locked_text() {
    let f = load();
    let m = &f["message_renderers"]["assistant_tool_use_collapsed"];
    let entry = RenderedMessage::AssistantToolUse {
        id: ToolUseId::new(),
        tool: m["tool"].as_str().unwrap().to_string(),
        input: m["input"].clone(),
    };
    // collapsed = not expanded, not focused
    assert_eq!(
        render_entry_to_string(&entry, false, false),
        platform_dot(m["expected_rendered_text"].as_str().unwrap()),
    );
}

#[test]
fn user_tool_result_collapsed_renders_to_locked_text() {
    let f = load();
    let m = &f["message_renderers"]["user_tool_result_collapsed"];
    let entry = RenderedMessage::UserToolResult {
        id: ToolUseId::new(),
        tool: m["tool"].as_str().unwrap().to_string(),
        result: m["result"].clone(),
        // (M7-02) Non-diff tool result — no Edit/Write diff inputs.
        old_string: None,
        new_string: None,
        file_path: None,
    };
    assert_eq!(
        render_entry_to_string(&entry, false, false),
        m["expected_rendered_text"].as_str().unwrap(),
    );
}
