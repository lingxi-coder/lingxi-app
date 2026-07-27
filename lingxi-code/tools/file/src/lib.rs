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

/// Suffix claude-code appends to a successful Edit/Write/MultiEdit result (binary
/// const `Pyn`, with a U+2014 em-dash), telling the model it already holds the
/// current file content. claude-code gates it on the file not being
/// user-modified / stale-recovered; LingXi's non-interactive path is always
/// current, so it is appended unconditionally. The leading space is part of the
/// literal (it follows the base message directly).
pub const FILE_STATE_CURRENT_SUFFIX: &str =
    " (file state is current in your context — no need to Read it back)";

mod dir_validate;
#[cfg(test)]
pub(crate) mod test_env;
pub mod edit;
pub mod file_meta;
pub mod glob;
pub mod grep;
#[cfg(feature = "image-read")]
pub mod image_read;
pub mod multi_edit;
pub mod notebook_edit;
pub mod notebook_read;
#[cfg(feature = "pdf-read")]
pub mod pdf_read;
#[cfg(feature = "pdf-render")]
mod pdf_render;
pub mod quotes;
pub mod read;
pub mod ripgrep_mode;
pub mod shared;
pub mod structured_patch;
pub mod write;

pub use edit::FileEditTool;
pub use glob::GlobTool;
pub use grep::GrepTool;
pub use multi_edit::MultiEditTool;
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

/// Richer stale-file message for the call-time re-check — byte-locked to
/// claude-code `Vbn` (binary offset confirmed via grep: "This commonly happens
/// when a linter or formatter run via Bash rewrites the file"). This is the
/// message from `m5p()` (the call-time stale detector in `FileEditTool.ts`),
/// distinct from the validate-phase [`FILE_UNEXPECTEDLY_MODIFIED_ERROR`].
/// `m5p` throws a `FileStateError(Vbn)` on the stale path when the edit can
/// potentially apply (`hnl(lTo(...)) === true`) but the file was modified; the
/// error propagates to `validateInput`'s caller. LingXi uses a unified `call`
/// path (no separate validate vs call phase), so `Vbn` belongs at the same
/// `stale_read` site as `FILE_UNEXPECTEDLY_MODIFIED_ERROR`, distinguishable by
/// whether a partial-apply is feasible (binary: `hnl(lTo(...))` true). For
/// simplicity, LingXi emits `Vbn` as the primary staleness message whenever
/// the `check_read_before_write` guard detects a changed mtime with different
/// content, bringing the model-facing text into parity with the binary's most
/// common stale-path.
pub const FILE_CONTENT_CHANGED_LINTER_MESSAGE: &str =
    "File content has changed since it was last read. This commonly happens when a linter or formatter run via Bash rewrites the file. Call Read on this file to refresh, then retry the edit.";

/// Read-state staleness guard shared by Edit / Write / NotebookEdit (Batch F).
///
/// 1:1 port of claude-code's read-before-write guard `FOg`
/// (`FileEditTool.ts`, offset-verified in the 2.1.212 binary):
///   * no recorded `Read` at all → refuse with [`FILE_NOT_READ_ERROR`]
///     (`if(!r) throw PWn`);
///   * mtime not advanced past the recorded read (`bY(e) <= r.timestamp`) →
///     proceed;
///   * mtime advanced, but the read covered the WHOLE file
///     ([`read_covers_full_file`] = `wMe`) and its recorded content still
///     equals the current on-disk content (`RMe`) → proceed (cloud sync /
///     antivirus touched the mtime without changing bytes);
///   * otherwise → [`FILE_CONTENT_CHANGED_LINTER_MESSAGE`] (claude-code `LWn`).
///
/// ## 2.1.212 fix — offset/limit reads are no longer rejected
/// Before 2.1.212 (and in LingXi's prior guard) a read done WITH `offset`/
/// `limit` was treated as "not read" and rejected outright, *before* even
/// checking the mtime. 2.1.212 gives the `wMe` full-read helper a new
/// `limit===void 0 → true` branch and — crucially — the guard now raises
/// "File has not been read yet" ONLY when NO read-state entry exists. A ranged
/// read still HAS an entry, so it falls through to the mtime / content checks:
/// editing a file previously read with offset/limit now succeeds when the file
/// is unchanged on disk.
///
/// ## Edit-applies recovery (`zQi`) lives in the caller
/// claude-code's Edit path additionally recovers a stale read when the edit
/// still applies cleanly to the current content. LingXi keeps that in
/// `edit.rs` (`stale_edit_applies`, gated on the CONTENT-CHANGED error), so
/// this shared guard — reused verbatim by Write / NotebookEdit, which have no
/// such recovery — stays limited to the entry / mtime / full-read checks.
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
/// claude-code's `wMe` also short-circuits on a dedicated `isPartialView` flag
/// (set only when a full read is token-cap-truncated for very-long-line
/// files). LingXi's `ReadFileEntry` does not track it; it is treated as
/// `false`. Such a read records a *truncated* slice, so the content-equality
/// gate (`current_full_content == entry.content`) already fails for it — the
/// outcome (stale, not full-read) is unchanged.
pub fn check_read_before_write(
    map: &tool_api::read_file_state::ReadFileStateMap,
    canon: &std::path::Path,
    current_mtime_ms: i64,
    current_full_content: &str,
) -> Result<(), tool_api::tool_trait::ToolError> {
    use tool_api::tool_trait::ToolError;

    let entry = match tool_api::read_file_state::get(map, canon) {
        Some(e) => e,
        // No recorded read at all → refuse. (claude-code `FOg`: `if(!r) throw
        // PWn`.) A ranged / offset read still HAS an entry, so it is NOT
        // rejected here — it falls through to the mtime / content checks below.
        None => return Err(ToolError::InvalidInput(FILE_NOT_READ_ERROR.into())),
    };

    // mtime not advanced past the recorded read → not stale. (claude-code:
    // `if(bY(e) <= r.timestamp) return false` — an equal floored mtime proceeds.)
    if current_mtime_ms <= entry.mtime_ms {
        return Ok(());
    }

    // mtime advanced: a full-file read (`wMe`) whose recorded content still
    // equals the current on-disk content is safe to proceed despite the bumped
    // mtime (cloud sync / antivirus can touch mtime without changing bytes).
    // (claude-code: `if(wMe(r) && RMe(r,t)) return false`.)
    if read_covers_full_file(&entry) && current_full_content == entry.content {
        return Ok(());
    }

    // The file changed on disk since the read → stale. claude-code throws
    // `gPe(LWn)`, and in 2.1.212 `LWn` is exactly the linter/formatter-context
    // message — byte-locked to [`FILE_CONTENT_CHANGED_LINTER_MESSAGE`].
    Err(ToolError::InvalidInput(
        FILE_CONTENT_CHANGED_LINTER_MESSAGE.into(),
    ))
}

/// claude-code `wMe(e)` (2.1.212) — whether a recorded read captured the
/// ENTIRE file, so its content may be compared against the current on-disk
/// bytes in the [`check_read_before_write`] staleness fallback.
///
/// ```text
/// wMe(e){
///   if((e.offset??1)>1||e.isPartialView) return false;      // ranged / truncated view
///   if(e.limit===void 0) return true;                        // unbounded read → whole file
///   return e.content!=="" && Cu(e.content,"\n")+1 < e.limit;  // limit not reached → EOF
/// }
/// ```
///
/// The `limit===void 0 → true` short-circuit is the 2.1.212 delta (absent in
/// 2.1.211). `isPartialView` (see the guard's Divergence note) is treated as
/// `false`. `Cu(content,"\n")+1` is claude-code's line count; a bounded read
/// whose line count is strictly below its `limit` reached EOF before the cap,
/// so it holds the whole file.
fn read_covers_full_file(entry: &tool_api::read_file_state::ReadFileEntry) -> bool {
    // (e.offset ?? 1) > 1 → a read that skipped leading lines is not full.
    if entry.offset.unwrap_or(1) > 1 {
        return false;
    }
    // e.limit === void 0 → unbounded read → whole file. (2.1.212 delta.)
    let Some(limit) = entry.limit else {
        return true;
    };
    // Bounded read: full iff the limit was never reached (line count strictly
    // below the cap ⇒ EOF hit first). Empty content never counts as full.
    !entry.content.is_empty() && (entry.content.matches('\n').count() as u64 + 1) < limit
}

/// Register all seven file/search tools against `reg`.
///
/// The binary's built-in-tool-names array (offset 188243808) includes
/// `MultiEdit` after `Edit`: `Read,Write,Edit,MultiEdit,Bash,Glob,Grep,…`.
/// `MultiEditTool` is name-routed into Edit dispatch (claude-code parity) and
/// registered here alongside the other built-ins.
pub fn register_all(reg: &mut tool_api::ToolRegistry, ctx: tool_api::BuiltinToolContext) {
    register_all_with_live_cwd(reg, ctx, None);
}

/// Register all seven file/search tools, injecting an optional shared live-cwd
/// cell (claude-code `getCwd()`/`Ct()`) into the tools that read the live cwd:
/// `Read` (the "File does not exist" note), `Glob`, and `Grep` (their default
/// search dir, "does not exist" notes, and result relativization). When `None`
/// (mobile / offline factory), every tool falls back to `ctx.workspace` —
/// byte-identical to [`register_all`]. The three write/edit tools and Notebook
/// take no cwd cell (they resolve absolute/trusted paths only).
pub fn register_all_with_live_cwd(
    reg: &mut tool_api::ToolRegistry,
    ctx: tool_api::BuiltinToolContext,
    live_cwd: Option<tool_api::LiveCwdCell>,
) {
    use std::sync::Arc;
    let read = match &live_cwd {
        Some(cell) => FileReadTool::new(ctx.clone()).with_live_cwd(cell.clone()),
        None => FileReadTool::new(ctx.clone()),
    };
    reg.register_builtin(Arc::new(read));
    reg.register_builtin(Arc::new(FileWriteTool::new(ctx.clone())));
    reg.register_builtin(Arc::new(FileEditTool::new(ctx.clone())));
    reg.register_builtin(Arc::new(MultiEditTool::new(ctx.clone())));
    reg.register_builtin(Arc::new(NotebookEditTool::new(ctx.clone())));
    let glob = match &live_cwd {
        Some(cell) => GlobTool::new(ctx.clone()).with_live_cwd(cell.clone()),
        None => GlobTool::new(ctx.clone()),
    };
    reg.register_builtin(Arc::new(glob));
    let grep = match live_cwd {
        Some(cell) => GrepTool::new(ctx).with_live_cwd(cell),
        None => GrepTool::new(ctx),
    };
    reg.register_builtin(Arc::new(grep));
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

    // ── Fix #3: Vbn stale-file message (linter/formatter context) ─────────────

    #[test]
    fn file_content_changed_linter_message_is_byte_locked() {
        // Binary `Vbn` literal (confirmed via grep -cF "This commonly happens when
        // a linter or formatter" = 2 hits in the oracle binary). Byte-exact to
        // claude-code's `Vbn` constant from the call-time stale-check `m5p()`.
        assert_eq!(
            FILE_CONTENT_CHANGED_LINTER_MESSAGE,
            "File content has changed since it was last read. This commonly happens when a linter or formatter run via Bash rewrites the file. Call Read on this file to refresh, then retry the edit."
        );
        // Confirm the linter/formatter context text is present.
        assert!(FILE_CONTENT_CHANGED_LINTER_MESSAGE.contains("linter or formatter"));
        assert!(FILE_CONTENT_CHANGED_LINTER_MESSAGE.contains("Call Read on this file"));
    }

    #[test]
    fn vbn_variant_emitted_on_stale_changed_content() {
        // When mtime advanced AND content differs, the staleness guard emits
        // FILE_CONTENT_CHANGED_LINTER_MESSAGE (Vbn), NOT FILE_UNEXPECTEDLY_MODIFIED_ERROR.
        let map = new_read_file_state_map();
        let p = PathBuf::from("/x");
        set(
            &map,
            p.clone(),
            ReadFileEntry {
                content: "old content".into(),
                mtime_ms: 100,
                offset: None,
                limit: None,
                from_read: true,
            },
        );
        let r = check_read_before_write(&map, &p, 200, "new content");
        match r.unwrap_err() {
            tool_api::tool_trait::ToolError::InvalidInput(m) => {
                assert_eq!(m, FILE_CONTENT_CHANGED_LINTER_MESSAGE);
            }
            other => panic!("expected InvalidInput with Vbn, got {other:?}"),
        }
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
    fn guard_offset_or_limit_read_unchanged_mtime_proceeds() {
        // 2.1.212: an offset/limit read is NOT rejected as "not read" just for
        // being ranged — with the mtime unchanged, the edit proceeds. (Before
        // 2.1.212 this raised FILE_NOT_READ_ERROR before the mtime check.)
        let map = new_read_file_state_map();
        let p = PathBuf::from("/x");
        // offset present, mtime unchanged ⇒ proceed.
        set(
            &map,
            p.clone(),
            ReadFileEntry {
                content: "c".into(),
                mtime_ms: 100,
                offset: Some(1),
                limit: None,
                from_read: true,
            },
        );
        assert!(check_read_before_write(&map, &p, 100, "c").is_ok());
        // limit present (offset None), mtime unchanged ⇒ proceed.
        set(
            &map,
            p.clone(),
            ReadFileEntry {
                content: "c".into(),
                mtime_ms: 100,
                offset: None,
                limit: Some(5),
                from_read: true,
            },
        );
        assert!(check_read_before_write(&map, &p, 100, "c").is_ok());
    }

    #[test]
    fn guard_ranged_stale_read_is_content_changed_not_not_read() {
        // A genuinely-partial (offset-skipped) read whose file changed on disk
        // yields the CONTENT-CHANGED (linter) message, NOT "not read" — so the
        // Edit tool's stale-recovery path can still apply. (claude-code `FOg`
        // has no entry ⇒ throw PWn; otherwise mtime-advanced ⇒ throw LWn.)
        let map = new_read_file_state_map();
        let p = PathBuf::from("/x");
        set(
            &map,
            p.clone(),
            ReadFileEntry {
                content: "line6\nline7".into(),
                mtime_ms: 100,
                offset: Some(6),
                limit: Some(2),
                from_read: true,
            },
        );
        assert_err_msg(
            check_read_before_write(&map, &p, 200, "whole new file"),
            FILE_CONTENT_CHANGED_LINTER_MESSAGE,
        );
    }

    #[test]
    fn read_covers_full_file_matches_wme() {
        let mk = |offset, limit, content: &str| ReadFileEntry {
            content: content.into(),
            mtime_ms: 0,
            offset,
            limit,
            from_read: true,
        };
        // Full read (no offset/limit) → whole file.
        assert!(read_covers_full_file(&mk(None, None, "a\nb")));
        // offset==1 with no limit → whole file ((offset??1)>1 is false).
        assert!(read_covers_full_file(&mk(Some(1), None, "a\nb")));
        // 2.1.212 delta: any unbounded read (limit==None) → whole file.
        assert!(read_covers_full_file(&mk(None, None, "")));
        // offset>1 → ranged, not full.
        assert!(!read_covers_full_file(&mk(Some(2), None, "a\nb")));
        // limit not reached (2 lines < limit 5) → EOF captured → full.
        assert!(read_covers_full_file(&mk(None, Some(5), "a\nb")));
        // limit reached exactly (3 lines, limit 3) → maybe more below → not full.
        assert!(!read_covers_full_file(&mk(None, Some(3), "a\nb\nc")));
        // empty content with a limit → not full.
        assert!(!read_covers_full_file(&mk(None, Some(5), "")));
    }

    #[test]
    fn regression_offset_limit_full_read_unchanged_proceeds() {
        // 2.1.212 gap: a read done WITH offset/limit that captured the whole
        // file (e.g. `Read(offset=1, limit=2000)` on a small file) must NOT be
        // rejected as "not read". With mtime unchanged, the edit proceeds.
        let map = new_read_file_state_map();
        let p = PathBuf::from("/x");
        set(
            &map,
            p.clone(),
            ReadFileEntry {
                content: "a\nb\nc".into(),
                mtime_ms: 100,
                offset: Some(1),
                limit: Some(2000),
                from_read: true,
            },
        );
        assert!(
            check_read_before_write(&map, &p, 100, "a\nb\nc").is_ok(),
            "offset/limit full-read + unchanged mtime must proceed"
        );
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
                from_read: true,
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
                from_read: true,
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
                from_read: true,
            },
        );
        // Now returns the richer Vbn message (linter/formatter context) — parity
        // with the binary's `m5p()` / `FileStateError(Vbn)` call-time path.
        assert_err_msg(
            check_read_before_write(&map, &p, 200, "new"),
            FILE_CONTENT_CHANGED_LINTER_MESSAGE,
        );
    }
}
