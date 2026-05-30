//! T3 tests — confirm `SessionMetadata` `Ord` is mtime-desc with filename-asc tiebreaker.

use lingxi_session::jsonl::SessionMetadata;
use std::path::PathBuf;
use std::time::{Duration, UNIX_EPOCH};
use uuid::Uuid;

fn meta(uuid_byte: u8, secs: u64, name: &str) -> SessionMetadata {
    SessionMetadata {
        uuid: Uuid::from_bytes([uuid_byte; 16]),
        title: format!("title-{uuid_byte}"),
        modified: UNIX_EPOCH + Duration::from_secs(secs),
        message_count: 1,
        path: PathBuf::from(name),
    }
}

#[test]
fn ord_sorts_newest_first() {
    let mut v = vec![
        meta(1, 100, "a.jsonl"),
        meta(2, 300, "b.jsonl"),
        meta(3, 200, "c.jsonl"),
    ];
    v.sort();
    assert_eq!(v[0].modified, UNIX_EPOCH + Duration::from_secs(300));
    assert_eq!(v[1].modified, UNIX_EPOCH + Duration::from_secs(200));
    assert_eq!(v[2].modified, UNIX_EPOCH + Duration::from_secs(100));
}

#[test]
fn ord_ties_break_by_filename_ascending() {
    let mut v = vec![
        meta(1, 500, "z.jsonl"),
        meta(2, 500, "a.jsonl"),
        meta(3, 500, "m.jsonl"),
    ];
    v.sort();
    assert_eq!(v[0].path.to_string_lossy(), "a.jsonl");
    assert_eq!(v[1].path.to_string_lossy(), "m.jsonl");
    assert_eq!(v[2].path.to_string_lossy(), "z.jsonl");
}

#[test]
fn equality_requires_all_fields() {
    let a = meta(1, 100, "x");
    let b = meta(1, 100, "x");
    assert_eq!(a, b);
    let c = meta(1, 101, "x");
    assert_ne!(a, c);
}
