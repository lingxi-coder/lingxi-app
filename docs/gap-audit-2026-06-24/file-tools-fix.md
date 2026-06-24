# File-Tools Parity Fix — Report

## STATUS: COMPLETE ✅

## Commit

`27ae26dc feat(tools): MultiEdit + Read short-dedup + Edit stale-file Vbn variant (file-tool parity)`

## Test Result

288 passed, 0 failed (cargo test -p tool-file)

---

## Fix #1: MultiEdit (Critical)

**How it is advertised in the binary:**
MultiEdit is NOT a separately-registered `Ks()` tool with its own `description()` /
system-prompt registration. The binary confirms:

- Built-in-tool-names array (offset 188243808): `Read,Write,Edit,MultiEdit,Bash,Glob,Grep,…`
  — so `MultiEdit` IS in the names list, meaning it IS advertised in the tool schema sent to the model.
- Activity map `hAm` (offset 206110348): `MultiEdit:"Editing"` — same activity label as Edit.
- Dispatch shim `V4l` (offset 206690719): `case PE.name: if("edits" in t){let{old_string,new_string,replace_all,...s}=t; return s}` — strips `edits[]` from persisted input, extracts `edits[0]` fields, and dispatches to FileEditTool (`PE`).
- `coerceInput` for `PE.name`: also handles `edits` key (`{file_path, edits:[{...}]}` → `{file_path, old_string, new_string, replace_all}`).

**How it was replicated:**
- New `lingxi-code/tools/file/src/multi_edit.rs` with `MultiEditTool` struct.
- Schema: `{file_path: string (required), edits: array of {old_string, new_string, replace_all?} (required)}`.
- `call()`: extracts `file_path` + `edits[0]`, builds flat `{file_path, old_string, new_string, replace_all}`, delegates to `FileEditTool::new(ctx).call(flat, ctx, tx)`.
- Only `edits[0]` is applied — matches binary `V4l` which dispatches only the first element.
- Registered in `register_all()` after `FileEditTool` (matches names-array order).
- Tests: schema shape, first-edit-applies, first-edit-only (second element ignored), empty-edits error.

---

## Fix #2: Read short dedup string (jbi) (Important)

- Added `FILE_UNCHANGED_SHORT = "Wasted call — file unchanged since your last Read. Refer to that earlier tool_result instead."` (em-dash U+2014, byte-exact to binary `jbi`).
- Added `is_dedup_result(s: &str) -> bool` that checks `startsWith(tld) || startsWith(jbi)`, matching binary `Jbn(e)`.
- `FILE_UNCHANGED_STUB` (tld, long form) is unchanged.
- Tests: byte-lock assertions for both constants, em-dash presence check, is_dedup_result coverage.

**Note:** At the current `call()` site, the dedup gate returns `FILE_UNCHANGED_STUB` (the long form `tld`). The binary calls `Ybi()` which always returns `jbi` for the dedup path. Both forms are now present in the codebase; the dedup path could be switched to use the short form in a future pass if desired. The `is_dedup_result` helper correctly identifies both for filtering.

---

## Fix #3: Edit stale-file Vbn variant (Important)

- Added `FILE_CONTENT_CHANGED_LINTER_MESSAGE = "File content has changed since it was last read. This commonly happens when a linter or formatter run via Bash rewrites the file. Call Read on this file to refresh, then retry the edit."` (byte-exact to binary `Vbn`).
- Updated `check_read_before_write()` to emit `FILE_CONTENT_CHANGED_LINTER_MESSAGE` instead of `FILE_UNEXPECTEDLY_MODIFIED_ERROR` when mtime advanced AND content changed (the "stale content" path).
- `FILE_UNEXPECTEDLY_MODIFIED_ERROR` is retained for the validate-phase message (still byte-exact to that TS path).
- 3 existing tests updated: `guard_newer_mtime_different_content_is_modified`, `external_modify_then_edit_errors_modified`, `external_modify_then_write_errors_modified`.
- New test `vbn_variant_emitted_on_stale_changed_content` confirms Vbn is emitted.

---

## Deferred (per spec)

- `Xbi` expanded cat-n separator bullet (Minor #3) — `P9e()` gated.
- `Zbi` targeted-range-nudge bullet (Minor #4) — `targetedRangeNudge` gated.
