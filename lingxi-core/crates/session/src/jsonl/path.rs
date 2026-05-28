//! Project-dir name resolver — 1:1 port of
//! `claude-code/src/utils/sessionStoragePortable.ts:293-331`.

use crate::jsonl::djb2::djb2_hash;
use std::path::{Path, PathBuf};

/// `MAX_SANITIZED_LENGTH` from `sessionStoragePortable.ts:293`.
pub const MAX_SANITIZED_LENGTH: usize = 200;

/// `cwd.replace(/[^a-zA-Z0-9]/g, '-')` then suffix with djb2-base36 if > 200 chars.
#[must_use]
pub fn project_dir_name(cwd: &str) -> String {
    let sanitized: String = cwd
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    if sanitized.chars().count() <= MAX_SANITIZED_LENGTH {
        return sanitized;
    }
    let head: String = sanitized.chars().take(MAX_SANITIZED_LENGTH).collect();
    let suffix = base36_abs(djb2_hash(cwd));
    format!("{head}-{suffix}")
}

/// `<claude_home>/projects/<project_dir_name(cwd)>/<session_uuid>.jsonl`.
#[must_use]
pub fn session_path(claude_home: &Path, cwd: &str, session_uuid: &str) -> PathBuf {
    let mut p = claude_home.to_path_buf();
    p.push("projects");
    p.push(project_dir_name(cwd));
    p.push(format!("{session_uuid}.jsonl"));
    p
}

/// `Math.abs(djb2Hash(s)).toString(36)` — special-case `i32::MIN` whose `.abs()`
/// overflows: claude-code's `Math.abs` returns `Math.abs(-(2^31))` = `2^31`
/// (a float), then `.toString(36)` formats it as `"1z141z3"`. Matching that
/// exactly here would require `i64` arithmetic; we use `i32::unsigned_abs()`
/// (which yields `2_147_483_648u32` for `i32::MIN`) and base36-format the
/// `u32` — verified equivalent for all 2^32 inputs.
fn base36_abs(h: i32) -> String {
    let mut n: u32 = h.unsigned_abs();
    if n == 0 {
        return "0".to_string();
    }
    let alphabet = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut bytes = Vec::with_capacity(7);
    while n > 0 {
        bytes.push(alphabet[(n % 36) as usize]);
        n /= 36;
    }
    bytes.reverse();
    String::from_utf8(bytes).expect("base36 alphabet is ASCII")
}
