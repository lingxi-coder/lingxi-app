//! File & search tools: Read, Write, Edit, NotebookEdit, Glob, Grep.
//!
//! Extracted from the `tools` monolith in M8-P5. Each tool takes a
//! `tool_api::BuiltinToolContext` at construction; `register_all` wires all
//! six into a `ToolRegistry`. Cross-tool helpers live in `tool_api::util`
//! (path validation, output truncation); file-only helpers live in `shared`.

#![forbid(unsafe_code)]
#![allow(
    clippy::cast_possible_wrap,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_lossless,
    clippy::match_wildcard_for_single_variants,
    clippy::single_match_else,
    clippy::needless_pass_by_value,
    clippy::too_many_lines,
    clippy::format_collect,
    clippy::similar_names,
    clippy::doc_markdown,
    clippy::manual_let_else
)]

pub mod edit;
pub mod file_meta;
pub mod glob;
pub mod grep;
pub mod notebook_edit;
pub mod quotes;
pub mod read;
pub mod shared;
pub mod write;

pub use edit::FileEditTool;
pub use glob::GlobTool;
pub use grep::GrepTool;
pub use notebook_edit::NotebookEditTool;
pub use read::FileReadTool;
pub use write::FileWriteTool;

/// Read-before-write staleness error: emitted when Edit / Write / NotebookEdit
/// detect that the target file changed on disk since the last `Read` (its
/// floor-truncated mtime advanced past the recorded read timestamp, and the
/// full-read content-equality fallback did not save it).
///
/// Shared by all three write tools (Batch F).
///
/// # Byte-exactness vs the TS reference (verified)
/// claude-code raises this in two distinct code paths with two *different*
/// strings:
///   * the user-facing validate / permission guard (`FileEditTool.ts:305-306`,
///     `FileWriteTool.ts:215-216`, `NotebookEditTool.ts:233-234`) returns
///     `behavior:'ask'` with the message
///     `"File has been modified since read, either by the user or by a linter.
///     Read it again before attempting to write it."` — this is the string the
///     model actually sees when the guard trips during validation.
///   * the call-time re-check (`FileEditTool.ts:465`,
///     `FileWriteTool.ts:292`, `NotebookEditTool.ts:292`) throws the
///     `FILE_UNEXPECTEDLY_MODIFIED_ERROR` constant from
///     `FileEditTool/constants.ts:10-11`, whose value is the *shorter*
///     `"File has been unexpectedly modified. Read it again before attempting
///     to write it."`.
///
/// The Rust tools have a single `call` entry point (no separate validate vs
/// call phase), and the model-facing guard in claude-code is the
/// validate-phase one, so we port the **validate-phase** "modified since read"
/// wording here (matching the spec's directive). The shorter `constants.ts`
/// literal is intentionally NOT used — documented divergence.
pub const FILE_UNEXPECTEDLY_MODIFIED_ERROR: &str =
    "File has been modified since read, either by the user or by a linter. Read it again before attempting to write it.";

/// Read-before-write "never read" error: emitted when Edit / Write /
/// NotebookEdit are asked to modify an EXISTING file that has no recorded
/// `Read` (or whose last read was a partial / range view).
///
/// Byte-exact to claude-code's validate-phase message
/// (`FileEditTool.ts:280-281`, `FileWriteTool.ts:202-203`,
/// `NotebookEditTool.ts:225-226`): `"File has not been read yet. Read it first
/// before writing to it."`.
pub const FILE_NOT_READ_ERROR: &str =
    "File has not been read yet. Read it first before writing to it.";

/// Read-state staleness guard shared by Edit / Write / NotebookEdit (Batch F).
///
/// Mirrors claude-code's read-before-write check
/// (`FileEditTool.ts:275-311`, `FileWriteTool.ts:198-219`,
/// `NotebookEditTool.ts:221-237`): for an EXISTING file, require a prior
/// full `Read`, and reject if the file's mtime advanced since that read unless
/// a full-read content-equality fallback proves the bytes are unchanged.
///
/// Arguments:
/// - `map`: the shared `read_file_state` registry (`ctx.read_file_state`).
/// - `canon`: the canonicalized absolute path used as the registry key (the
///   same key the `Read` tool wrote under).
/// - `current_mtime_ms`: the file's *current* floor-truncated mtime (ms).
/// - `current_full_content`: the file's *current* content, decoded the SAME
///   way `Read` stores it (so the content-equality fallback compares like with
///   like). Only consulted for the full-read fallback.
///
/// Returns `Ok(())` to proceed, or a [`tool_api::tool_trait::ToolError::InvalidInput`]
/// carrying the byte-exact error string when the write must be refused.
///
/// New-file creation MUST NOT call this — callers gate it behind "the file
/// exists" (TS `ENOENT → result:true` / `meta === null` skips the guard).
///
/// # Divergence (flagged)
/// claude-code tracks a dedicated `isPartialView` flag set on partial reads.
/// We approximate it via `entry.offset.is_some() || entry.limit.is_some()`,
/// since the Rust `Read` records the verbatim `offset`/`limit` it read with.
/// A full read records both as `None`; any range read records at least one as
/// `Some`, so the approximation matches in practice for the parity fixtures.
pub fn check_read_before_write(
    map: &tool_api::read_file_state::ReadFileStateMap,
    canon: &std::path::Path,
    current_mtime_ms: i64,
    current_full_content: &str,
) -> Result<(), tool_api::tool_trait::ToolError> {
    use tool_api::tool_trait::ToolError;

    let entry = match tool_api::read_file_state::get(map, canon) {
        Some(e) => e,
        // Never read (or no recorded read) → refuse. (TS:
        // `!readTimestamp` / `!lastRead` → "File has not been read yet…".)
        None => return Err(ToolError::InvalidInput(FILE_NOT_READ_ERROR.into())),
    };

    // `isPartialView` approximation: a range read (offset/limit present) does
    // not count as having "read" the whole file. (TS: `readTimestamp.isPartialView`.)
    let is_full_read = entry.offset.is_none() && entry.limit.is_none();
    if !is_full_read {
        return Err(ToolError::InvalidInput(FILE_NOT_READ_ERROR.into()));
    }

    // Staleness: mtime advanced past the recorded read timestamp.
    if current_mtime_ms > entry.mtime_ms {
        // Windows-timestamp content-equality fallback (TS:294-300 / 457-463):
        // a full read whose on-disk content still matches the recorded content
        // is safe to proceed despite the bumped mtime (cloud sync / antivirus
        // can touch mtime without changing bytes). `is_full_read` is already
        // guaranteed true here.
        if current_full_content == entry.content {
            return Ok(());
        }
        return Err(ToolError::InvalidInput(
            FILE_UNEXPECTEDLY_MODIFIED_ERROR.into(),
        ));
    }

    Ok(())
}

/// Register all six file/search tools against `reg`.
pub fn register_all(reg: &mut tool_api::ToolRegistry, ctx: tool_api::BuiltinToolContext) {
    use std::sync::Arc;
    reg.register_builtin(Arc::new(FileReadTool::new(ctx.clone())));
    reg.register_builtin(Arc::new(FileWriteTool::new(ctx.clone())));
    reg.register_builtin(Arc::new(FileEditTool::new(ctx.clone())));
    reg.register_builtin(Arc::new(NotebookEditTool::new(ctx.clone())));
    reg.register_builtin(Arc::new(GlobTool::new(ctx.clone())));
    reg.register_builtin(Arc::new(GrepTool::new(ctx)));
}

#[cfg(test)]
mod staleness_guard_tests {
    use super::*;
    use std::path::PathBuf;
    use tool_api::read_file_state::{new_read_file_state_map, set, ReadFileEntry};
    use tool_api::tool_trait::ToolError;

    #[test]
    fn file_not_read_error_is_byte_locked() {
        // FileEditTool.ts:280-281 / FileWriteTool.ts:202-203 /
        // NotebookEditTool.ts:225-226.
        assert_eq!(
            FILE_NOT_READ_ERROR,
            "File has not been read yet. Read it first before writing to it."
        );
    }

    #[test]
    fn modified_error_is_byte_locked_to_validate_phase_string() {
        // The model-facing validate-phase guard message
        // (FileEditTool.ts:305-306 / FileWriteTool.ts:215-216 /
        // NotebookEditTool.ts:233-234). NOTE: this intentionally differs from
        // the shorter `FileEditTool/constants.ts:10-11`
        // `FILE_UNEXPECTEDLY_MODIFIED_ERROR` literal (documented divergence).
        assert_eq!(
            FILE_UNEXPECTEDLY_MODIFIED_ERROR,
            "File has been modified since read, either by the user or by a linter. Read it again before attempting to write it."
        );
    }

    /// Assert the guard returned an `InvalidInput` error carrying exactly
    /// `expected` as its message (the `ToolError::Display` impl prefixes
    /// `"invalid input: "`, so we match on the `InvalidInput` payload).
    fn assert_err_msg(r: Result<(), ToolError>, expected: &str) {
        match r.unwrap_err() {
            ToolError::InvalidInput(m) => assert_eq!(m, expected),
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    #[test]
    fn guard_missing_entry_is_not_read() {
        let map = new_read_file_state_map();
        let r = check_read_before_write(&map, &PathBuf::from("/x"), 100, "content");
        assert_err_msg(r, FILE_NOT_READ_ERROR);
    }

    #[test]
    fn guard_partial_view_is_not_read() {
        let map = new_read_file_state_map();
        let p = PathBuf::from("/x");
        // offset present ⇒ partial view ⇒ not-read.
        set(
            &map,
            p.clone(),
            ReadFileEntry {
                content: "c".into(),
                mtime_ms: 100,
                offset: Some(1),
                limit: None,
            },
        );
        assert_err_msg(check_read_before_write(&map, &p, 100, "c"), FILE_NOT_READ_ERROR);
        // limit present (offset None) ⇒ also partial.
        set(
            &map,
            p.clone(),
            ReadFileEntry {
                content: "c".into(),
                mtime_ms: 100,
                offset: None,
                limit: Some(5),
            },
        );
        assert_err_msg(check_read_before_write(&map, &p, 100, "c"), FILE_NOT_READ_ERROR);
    }

    #[test]
    fn guard_unchanged_mtime_proceeds() {
        let map = new_read_file_state_map();
        let p = PathBuf::from("/x");
        set(
            &map,
            p.clone(),
            ReadFileEntry {
                content: "c".into(),
                mtime_ms: 100,
                offset: None,
                limit: None,
            },
        );
        // current == recorded ⇒ not stale ⇒ Ok.
        assert!(check_read_before_write(&map, &p, 100, "c").is_ok());
    }

    #[test]
    fn guard_newer_mtime_same_content_proceeds_via_fallback() {
        let map = new_read_file_state_map();
        let p = PathBuf::from("/x");
        set(
            &map,
            p.clone(),
            ReadFileEntry {
                content: "same".into(),
                mtime_ms: 100,
                offset: None,
                limit: None,
            },
        );
        // mtime advanced but content matches ⇒ fallback proceeds.
        assert!(check_read_before_write(&map, &p, 200, "same").is_ok());
    }

    #[test]
    fn guard_newer_mtime_different_content_is_modified() {
        let map = new_read_file_state_map();
        let p = PathBuf::from("/x");
        set(
            &map,
            p.clone(),
            ReadFileEntry {
                content: "old".into(),
                mtime_ms: 100,
                offset: None,
                limit: None,
            },
        );
        assert_err_msg(
            check_read_before_write(&map, &p, 200, "new"),
            FILE_UNEXPECTEDLY_MODIFIED_ERROR,
        );
    }
}
