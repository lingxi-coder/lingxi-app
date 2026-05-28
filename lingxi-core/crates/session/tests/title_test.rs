//! T2 tests — byte-lock `extract_title` against the cases in plan T0 step 3.

use lingxi_session::jsonl::{extract_title, JsonlMessage};
use serde_json::json;

fn make_user(content: serde_json::Value) -> JsonlMessage {
    serde_json::from_value(json!({
        "type": "user",
        "uuid": "11111111-1111-1111-1111-111111111111",
        "parentUuid": null,
        "sessionId": "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
        "timestamp": "2026-05-25T12:00:00.000Z",
        "cwd": "/tmp",
        "version": "0.6.0",
        "isSidechain": false,
        "userType": "external",
        "message": {"role": "user", "content": content}
    }))
    .expect("user message")
}

#[test]
fn empty_messages_returns_empty_string() {
    let messages: Vec<JsonlMessage> = vec![];
    assert_eq!(extract_title(&messages), "");
}

#[test]
fn no_user_message_returns_empty_string() {
    let mut m = make_user(json!("hi"));
    m.message_type = "assistant".to_string();
    assert_eq!(extract_title(&[m]), "");
}

#[test]
fn empty_user_content_returns_empty_string() {
    let m = make_user(json!(""));
    assert_eq!(extract_title(&[m]), "");
}

#[test]
fn short_string_content_is_returned_verbatim() {
    let m = make_user(json!("hello world"));
    assert_eq!(extract_title(&[m]), "hello world");
}

#[test]
fn exactly_50_chars_is_returned_without_ellipsis() {
    let s: String = "a".repeat(50);
    let m = make_user(json!(s.clone()));
    assert_eq!(extract_title(&[m]), s);
}

#[test]
fn fifty_one_chars_is_truncated_with_ellipsis() {
    let s: String = "a".repeat(51);
    let m = make_user(json!(s));
    let title = extract_title(&[m]);
    assert_eq!(title.chars().count(), 51); // 50 'a' + 1 ellipsis
    assert!(title.ends_with('…'));
    assert_eq!(title.chars().take(50).collect::<String>(), "a".repeat(50));
}

#[test]
fn cjk_truncation_by_char_count_not_byte_count() {
    // 60 CJK chars = 180 UTF-8 bytes; we want exactly 50 chars + ellipsis.
    let s: String = "中".repeat(60);
    let m = make_user(json!(s));
    let title = extract_title(&[m]);
    assert_eq!(title.chars().count(), 51); // 50 '中' + '…'
    assert!(title.ends_with('…'));
}

#[test]
fn array_content_uses_first_text_block() {
    let m = make_user(json!([
        {"type": "text", "text": "first"},
        {"type": "text", "text": "second"}
    ]));
    assert_eq!(extract_title(&[m]), "first");
}

#[test]
fn array_content_with_only_image_returns_empty() {
    let m = make_user(json!([
        {"type": "image", "source": {"type": "base64", "data": "xxxx"}}
    ]));
    assert_eq!(extract_title(&[m]), "");
}

#[test]
fn whitespace_is_trimmed_before_truncation() {
    let m = make_user(json!("   hi   "));
    assert_eq!(extract_title(&[m]), "hi");
}

#[test]
fn skips_assistant_to_find_user() {
    let assistant = serde_json::from_value::<JsonlMessage>(json!({
        "type": "assistant",
        "uuid": "22222222-2222-2222-2222-222222222222",
        "parentUuid": null,
        "sessionId": "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
        "timestamp": "2026-05-25T12:00:00.000Z",
        "cwd": "/tmp",
        "version": "0.6.0",
        "isSidechain": false,
        "userType": "external",
        "message": {"role": "assistant", "content": "I am the model"}
    }))
    .unwrap();
    let user = make_user(json!("real prompt"));
    assert_eq!(extract_title(&[assistant, user]), "real prompt");
}
