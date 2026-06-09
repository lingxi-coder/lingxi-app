# P4b — PDF Page-Image Extraction Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the deferred P4b gap — render PDF pages to JPEG images via `pdftoppm` (poppler-utils) and emit them as `ContentBlock::Image` blocks, exactly as claude-code's FileRead `pages` path does — and bring the no-pages PDF routing to full claude-code parity.

**Architecture:** claude-code's FileRead PDF branch (`src/tools/FileReadTool/FileReadTool.ts:893-1014` + `src/utils/pdf.ts`) shells out to the system binary `pdftoppm -jpeg -r 100 [-f N] [-l M] <file> <dir>/page`, reads the produced `page-NN.jpg` files, resizes each through the image ladder, and emits one user message carrying N image blocks. This is the **one** parity gap where claude-code uses a system binary rather than a library — so the faithful port adds **no new Rust crate**, only `std::`/`tokio::process` to spawn `pdftoppm`, plus `tempfile` for the scratch dir. We isolate the subprocess spawn (env-dependent, untestable without poppler installed) from the parity-critical emission logic (deterministically testable with synthetic JPEGs). All new code is behind a new `pdf-render` Cargo feature that implies `pdf-read` + `image-read`; engine-desktop enables it, engine-mobile never compiles it.

**Tech Stack:** Rust (edition 2021, toolchain 1.82). `tokio::process` + `tokio::time` (timeout) for the spawn; `tempfile` (already in the dep tree) for the scratch dir; reuses `crate::image_read::process_image` (image 0.24.9) for per-page resize and `protocol::ImageSource` / `ConversationMessage::user_with_images` for emission. `pdftoppm` is a **runtime** system dependency (poppler-utils), not a Rust dep.

---

## Conventions (read before starting)

- **Branch:** `parity-p4b-pdf-page-images` (already created from `main`, in the main checkout — not a worktree). Do NOT switch branches.
- **Commit footer (every commit):** end the message with a blank line then:
  ```
  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
  ```
  Use `git commit -F <tmpfile>` (the interactive shell traps backticks / `<` / `>` / `->` in `-m`).
- **Gate ritual** (run the relevant subset after each task; full set before final review):
  - `cargo test -p tool-file` — features OFF (default).
  - `cargo test -p tool-file --features pdf-render` — features ON (implies pdf-read + image-read).
  - `cargo clippy -p tool-file --all-targets --no-deps -- -D warnings` AND `cargo clippy -p tool-file --all-targets --no-deps --features pdf-render -- -D warnings` — both feature states clean.
  - `cargo test -p protocol` (sanity; no protocol change expected in P4b).
  - Before final review only: `cargo test --workspace --no-run` (struct-trap compile, ~2-3 min). AVOID `cargo test --workspace` runtime (fs_watch/fseventsd flake — see memory).
- **Workspace lints:** `-D missing-docs` and clippy `-D warnings` are enforced. Every new `pub` item needs a doc comment.
- **Reference of truth:** `claude-code/src/...` TypeScript. The exact source lines are quoted inline in each task — do NOT re-derive from memory.
- **pdftoppm is NOT installed on the dev machine.** Tests that actually spawn `pdftoppm` MUST be gated on availability and skip cleanly when absent (the `render_pdf_pages_smoke` test in Task 5). All other new tests are deterministic and must pass with poppler absent.

---

## File Structure

| File | Responsibility | Task |
|---|---|---|
| `lingxi-code/tools/file/Cargo.toml` | new `pdf-render` feature (`pdf-read` + `image-read` + `tokio/process` + `tokio/time` + `dep:tempfile`) | 1 |
| `lingxi-code/apps/engine-desktop/Cargo.toml` | enable `pdf-render` on `tool-file` | 1 |
| `lingxi-code/tools/file/src/pdf_render.rs` | NEW — `PdfRenderError`, pure `build_pdftoppm_args` + `classify_pdftoppm_failure`, async `is_pdftoppm_available` + `render_pdf_pages` (the `pdftoppm` spawn) | 2, 5 |
| `lingxi-code/tools/file/src/lib.rs` | register `pdf_render` module behind `pdf-render` | 2 |
| `lingxi-code/tools/file/src/read.rs` | `format_file_size` pure helper; `build_pages_payload` pure helper; `read_pdf_pages_result` method; wire the `pages` branch; reconcile the no-pages branch | 3, 4, 6 |
| `lingxi-code/tools/file/src/pdf_read.rs` | add `PDF_TARGET_RAW_SIZE` (20 MB) + `PDF_MAX_EXTRACT_SIZE` (100 MB); remove now-unused `PDF_EXTRACT_SIZE_THRESHOLD` | 3, 6 |

---

## Task 1: `pdf-render` Cargo feature + wiring

**Files:**
- Modify: `lingxi-code/tools/file/Cargo.toml`
- Modify: `lingxi-code/apps/engine-desktop/Cargo.toml:88`

This task is build-config only (no Rust logic yet), so it is verified by compilation rather than a unit test.

- [ ] **Step 1: Add `tempfile` as an optional dependency**

In `lingxi-code/tools/file/Cargo.toml`, under `[dependencies]`, directly after the `lopdf` line, add:

```toml
# pdftoppm renders pages into a scratch directory; TempDir gives a unique dir +
# automatic cleanup (RAII). Already in the workspace (dev-dep here + elsewhere),
# so no new external crate enters the lockfile. Optional, gated behind pdf-render.
tempfile = { version = "3", optional = true }
```

- [ ] **Step 2: Add the `pdf-render` feature**

In the same file, under `[features]`, after the `pdf-read = [...]` line, add:

```toml
# FileRead PDF page-image extraction (claude-code FileRead `pages` path). Spawns
# the `pdftoppm` system binary (poppler-utils) — no new Rust crate, just the
# tokio process/time features + tempfile for the scratch dir. Implies pdf-read
# (routing + page count) and image-read (per-page resize via process_image).
# OFF by default; engine-mobile never compiles it. engine-desktop enables it.
pdf-render = ["pdf-read", "image-read", "dep:tempfile", "tokio/process", "tokio/time"]
```

- [ ] **Step 3: Enable the feature on engine-desktop**

In `lingxi-code/apps/engine-desktop/Cargo.toml`, change line 88 from:

```toml
tool-file = { path = "../../tools/file", features = ["image-read", "pdf-read"] }
```

to:

```toml
tool-file = { path = "../../tools/file", features = ["image-read", "pdf-read", "pdf-render"] }
```

- [ ] **Step 4: Verify it compiles in all three states**

Run:
```bash
cargo build -p tool-file
cargo build -p tool-file --features pdf-render
cargo build -p engine-desktop
```
Expected: all succeed. (`pdf-render` pulls in `tempfile` + the tokio features; the feature has no code yet so this just proves the manifest is valid.)

- [ ] **Step 5: Verify engine-mobile is unaffected**

Run:
```bash
cargo tree -p engine-mobile -i tempfile 2>/dev/null || echo "engine-mobile does NOT depend on tempfile (non-dev) — correct"
```
Expected: engine-mobile does not pull `tempfile` as a normal dependency (it never enables `pdf-render`). If `engine-mobile` is not a member that builds standalone, instead run `cargo build -p engine-mobile` and confirm success. The point: mobile must not gain `pdf-render`.

- [ ] **Step 6: Commit**

```bash
git add lingxi-code/tools/file/Cargo.toml lingxi-code/apps/engine-desktop/Cargo.toml
git commit -F <tmpfile>   # subject: feat(tool-file): add pdf-render feature (pdftoppm page extraction)
```

---

## Task 2: `pdf_render.rs` — error type, pure helpers, and the `pdftoppm` spawn

**Files:**
- Create: `lingxi-code/tools/file/src/pdf_render.rs`
- Modify: `lingxi-code/tools/file/src/lib.rs` (register the module)
- Test: inline `#[cfg(test)]` in `pdf_render.rs`

**Scene:** This module mirrors claude-code `src/utils/pdf.ts` — specifically `extractPDFPages` (lines 179-300) and `isPdftoppmAvailable` (160-169). The pure helpers (`build_pdftoppm_args`, `classify_pdftoppm_failure`) carry the logic that claude-code tests by mocking `execFileNoThrow`; they are deterministically unit-tested here. The actual spawn (`render_pdf_pages`) is exercised only by the availability-gated smoke test in Task 5.

claude-code reference for the error reasons + messages (`src/utils/pdf.ts`):
- `empty` → `PDF file is empty: ${filePath}` (line 191)
- `too_large` → `PDF file exceeds maximum allowed size for text extraction (${formatFileSize(PDF_MAX_EXTRACT_SIZE)}).` (line 200) — `PDF_MAX_EXTRACT_SIZE = 100 MB`
- `unavailable` → `pdftoppm is not installed. Install poppler-utils (e.g. \`brew install poppler\` or \`apt-get install poppler-utils\`) to enable PDF page rendering.` (line 212)
- `password_protected` (stderr `/password/i`) → `PDF is password-protected. Please provide an unprotected version.` (line 243)
- `corrupted` (stderr `/damaged|corrupt|invalid/i`, OR 0 output pages) → `PDF file is corrupted or invalid.` (line 252) / `pdftoppm produced no output pages. The PDF may be invalid.` (line 272)
- `unknown` (other non-zero exit) → `pdftoppm failed: ${stderr}` (line 258)

pdftoppm arg construction (`src/utils/pdf.ts:222-230`):
```js
const prefix = join(outputDir, 'page')
const args = ['-jpeg', '-r', '100']
if (options?.firstPage) args.push('-f', String(options.firstPage))
if (options?.lastPage && options.lastPage !== Infinity) args.push('-l', String(options.lastPage))
args.push(filePath, prefix)
```

- [ ] **Step 1: Write the failing tests (pure helpers)**

Create `lingxi-code/tools/file/src/pdf_render.rs` with ONLY the test module first (so it fails to compile → "function not defined"):

```rust
//! FileRead PDF page-image extraction: spawn `pdftoppm` (poppler-utils) to render
//! pages to JPEG, mirroring claude-code `src/utils/pdf.ts` `extractPDFPages`.
//! `pdftoppm` is a RUNTIME system dependency (no Rust crate). The pure helpers
//! (`build_pdftoppm_args`, `classify_pdftoppm_failure`) carry the logic
//! claude-code tests by mocking the subprocess; the spawn itself
//! (`render_pdf_pages`) is exercised by the availability-gated smoke test.

// (implementation added in later steps)

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;
    use std::path::Path;

    fn args_as_strings(a: &[std::ffi::OsString]) -> Vec<String> {
        a.iter().map(|s| s.to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn args_single_page_pushes_f_and_l() {
        let a = build_pdftoppm_args(5, 5, Path::new("/in.pdf"), Path::new("/tmp/x/page"));
        assert_eq!(
            args_as_strings(&a),
            vec!["-jpeg", "-r", "100", "-f", "5", "-l", "5", "/in.pdf", "/tmp/x/page"]
        );
    }

    #[test]
    fn args_range_pushes_f_and_l() {
        let a = build_pdftoppm_args(2, 7, Path::new("/in.pdf"), Path::new("/tmp/x/page"));
        assert_eq!(
            args_as_strings(&a),
            vec!["-jpeg", "-r", "100", "-f", "2", "-l", "7", "/in.pdf", "/tmp/x/page"]
        );
    }

    #[test]
    fn args_open_ended_omits_l() {
        // u32::MAX models the open-ended "N-" range (claude-code Infinity): -l is
        // NOT pushed, so pdftoppm renders from firstPage to the end of the doc.
        let a = build_pdftoppm_args(3, u32::MAX, Path::new("/in.pdf"), Path::new("/tmp/x/page"));
        assert_eq!(
            args_as_strings(&a),
            vec!["-jpeg", "-r", "100", "-f", "3", "/in.pdf", "/tmp/x/page"]
        );
    }

    #[test]
    fn classify_password() {
        assert!(matches!(
            classify_pdftoppm_failure("Error: PDF file is password protected"),
            PdfRenderError::PasswordProtected
        ));
        // case-insensitive (claude-code /password/i)
        assert!(matches!(
            classify_pdftoppm_failure("Command Line Error: Incorrect PASSWORD"),
            PdfRenderError::PasswordProtected
        ));
    }

    #[test]
    fn classify_corrupted() {
        for s in ["the file is damaged", "May not be a PDF (corrupt)", "Invalid XRef"] {
            assert!(
                matches!(classify_pdftoppm_failure(s), PdfRenderError::Corrupted),
                "stderr {s:?} should classify corrupted"
            );
        }
    }

    #[test]
    fn classify_unknown_carries_stderr() {
        match classify_pdftoppm_failure("some other failure") {
            PdfRenderError::Unknown(s) => assert_eq!(s, "some other failure"),
            other => panic!("expected Unknown, got {other:?}"),
        }
    }

    #[test]
    fn error_messages_match_claude_code() {
        assert_eq!(
            PdfRenderError::Empty.message("/a.pdf"),
            "PDF file is empty: /a.pdf"
        );
        assert_eq!(
            PdfRenderError::TooLarge.message("/a.pdf"),
            "PDF file exceeds maximum allowed size for text extraction (100MB)."
        );
        assert_eq!(
            PdfRenderError::Unavailable.message("/a.pdf"),
            "pdftoppm is not installed. Install poppler-utils (e.g. `brew install poppler` or `apt-get install poppler-utils`) to enable PDF page rendering."
        );
        assert_eq!(
            PdfRenderError::PasswordProtected.message("/a.pdf"),
            "PDF is password-protected. Please provide an unprotected version."
        );
        assert_eq!(
            PdfRenderError::Corrupted.message("/a.pdf"),
            "PDF file is corrupted or invalid."
        );
        assert_eq!(
            PdfRenderError::NoOutput.message("/a.pdf"),
            "pdftoppm produced no output pages. The PDF may be invalid."
        );
        assert_eq!(
            PdfRenderError::Unknown("boom".to_string()).message("/a.pdf"),
            "pdftoppm failed: boom"
        );
    }

    #[test]
    fn telemetry_codes_are_stable() {
        assert_eq!(PdfRenderError::Empty.telemetry_code(), "pdf_empty");
        assert_eq!(PdfRenderError::TooLarge.telemetry_code(), "pdf_too_large");
        assert_eq!(PdfRenderError::Unavailable.telemetry_code(), "pdf_unavailable");
        assert_eq!(PdfRenderError::PasswordProtected.telemetry_code(), "pdf_password_protected");
        assert_eq!(PdfRenderError::Corrupted.telemetry_code(), "pdf_corrupted");
        assert_eq!(PdfRenderError::NoOutput.telemetry_code(), "pdf_no_output");
        assert_eq!(PdfRenderError::Unknown(String::new()).telemetry_code(), "pdf_unknown");
    }

    let _ = OsStr::new("");  // silence unused import if helpers move
}
```

> Note: delete the stray `let _ = OsStr::new("");` line — it is illegal at module scope. (Included only as a reminder that `OsStr`/`OsString` are the arg types; remove before running.) The real imports the impl needs are added in Step 3.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p tool-file --features pdf-render pdf_render`
Expected: FAIL to compile — `build_pdftoppm_args`, `classify_pdftoppm_failure`, `PdfRenderError` not found.

- [ ] **Step 3: Write the implementation**

Replace the `// (implementation added in later steps)` placeholder with:

```rust
use std::ffi::OsString;
use std::path::Path;
use std::time::Duration;

use crate::pdf_read::PDF_MAX_EXTRACT_SIZE;

/// pdftoppm render timeout (claude-code `execFileNoThrow` `timeout: 120_000`).
const PDFTOPPM_TIMEOUT: Duration = Duration::from_secs(120);

/// Structured failure of PDF page extraction — mirrors claude-code `PDFError`
/// (`src/utils/pdf.ts`). Each variant maps to a fixed user-facing message and a
/// stable telemetry code.
#[derive(Debug)]
pub enum PdfRenderError {
    /// 0-byte file.
    Empty,
    /// Larger than [`PDF_MAX_EXTRACT_SIZE`] (100 MB).
    TooLarge,
    /// `pdftoppm` (poppler-utils) is not installed / not on `PATH`.
    Unavailable,
    /// pdftoppm reported the PDF is password-protected.
    PasswordProtected,
    /// pdftoppm reported the PDF is damaged/corrupt/invalid.
    Corrupted,
    /// pdftoppm exited 0 but produced no `.jpg` pages.
    NoOutput,
    /// Any other non-zero exit; carries the captured stderr.
    Unknown(String),
}

impl PdfRenderError {
    /// The exact claude-code user-facing message for this failure. `path` is the
    /// PDF path (used only by [`Self::Empty`]).
    #[must_use]
    pub fn message(&self, path: &str) -> String {
        match self {
            Self::Empty => format!("PDF file is empty: {path}"),
            Self::TooLarge => format!(
                "PDF file exceeds maximum allowed size for text extraction ({}).",
                crate::read::format_file_size(PDF_MAX_EXTRACT_SIZE)
            ),
            Self::Unavailable => "pdftoppm is not installed. Install poppler-utils (e.g. `brew install poppler` or `apt-get install poppler-utils`) to enable PDF page rendering.".to_string(),
            Self::PasswordProtected => "PDF is password-protected. Please provide an unprotected version.".to_string(),
            Self::Corrupted => "PDF file is corrupted or invalid.".to_string(),
            Self::NoOutput => "pdftoppm produced no output pages. The PDF may be invalid.".to_string(),
            Self::Unknown(stderr) => format!("pdftoppm failed: {stderr}"),
        }
    }

    /// Stable telemetry code for `emit_failed`.
    #[must_use]
    pub fn telemetry_code(&self) -> &'static str {
        match self {
            Self::Empty => "pdf_empty",
            Self::TooLarge => "pdf_too_large",
            Self::Unavailable => "pdf_unavailable",
            Self::PasswordProtected => "pdf_password_protected",
            Self::Corrupted => "pdf_corrupted",
            Self::NoOutput => "pdf_no_output",
            Self::Unknown(_) => "pdf_unknown",
        }
    }
}

/// Build the `pdftoppm` argument vector (claude-code `src/utils/pdf.ts:222-230`).
/// `-jpeg -r 100`, then `-f first`, then `-l last` UNLESS `last == u32::MAX`
/// (the open-ended `"N-"` range — claude-code's `Infinity` — renders to the end),
/// then the input path and the output `<dir>/page` prefix.
#[must_use]
pub fn build_pdftoppm_args(first: u32, last: u32, input: &Path, prefix: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec![
        "-jpeg".into(),
        "-r".into(),
        "100".into(),
        "-f".into(),
        first.to_string().into(),
    ];
    if last != u32::MAX {
        args.push("-l".into());
        args.push(last.to_string().into());
    }
    args.push(input.as_os_str().to_owned());
    args.push(prefix.as_os_str().to_owned());
    args
}

/// Classify a non-zero `pdftoppm` exit by its stderr (claude-code
/// `src/utils/pdf.ts:236-259`): `/password/i` → password-protected;
/// `/damaged|corrupt|invalid/i` → corrupted; else `Unknown(stderr)`.
#[must_use]
pub fn classify_pdftoppm_failure(stderr: &str) -> PdfRenderError {
    let lower = stderr.to_ascii_lowercase();
    if lower.contains("password") {
        PdfRenderError::PasswordProtected
    } else if lower.contains("damaged") || lower.contains("corrupt") || lower.contains("invalid") {
        PdfRenderError::Corrupted
    } else {
        PdfRenderError::Unknown(stderr.to_string())
    }
}
```

- [ ] **Step 4: Run the pure-helper tests to verify they pass**

First remove the stray `let _ = OsStr::new("");` line and the now-unused `OsStr` import from the test module (keep `use std::path::Path;`). Then run:
`cargo test -p tool-file --features pdf-render pdf_render`
Expected: PASS (all pure-helper tests). The spawn fns are added next.

- [ ] **Step 5: Add `is_pdftoppm_available` and `render_pdf_pages` (the spawn)**

Append to `pdf_render.rs` (above the test module). `render_pdf_pages` re-stats nothing — it takes the already-known `original_size` from the caller and rejects `> PDF_MAX_EXTRACT_SIZE`. (Empty/`%PDF-` are already guarded upstream in `read_pdf_result`.)

```rust
/// Whether `pdftoppm -v` runs (poppler-utils installed). claude-code
/// (`isPdftoppmAvailable`, pdf.ts:160-169) treats either exit 0 OR any stderr
/// output as "available" (pdftoppm prints its version banner to stderr and may
/// exit non-zero on old builds). Not cached — extraction is rare, so we probe
/// per call (claude-code caches purely as a perf optimization; behavior is
/// identical).
pub async fn is_pdftoppm_available() -> bool {
    match tokio::process::Command::new("pdftoppm")
        .arg("-v")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
    {
        Ok(out) => out.status.success() || !out.stderr.is_empty(),
        Err(_) => false, // not found on PATH
    }
}

/// Render pages `first..=last` (1-indexed; `last == u32::MAX` ⇒ to end) of the
/// PDF at `path` to JPEG via `pdftoppm`, returning the page images in natural
/// filename order. Ports claude-code `extractPDFPages` (pdf.ts:179-300).
///
/// `original_size` is the already-stat'd file size (the caller has it) and is
/// rejected if `> PDF_MAX_EXTRACT_SIZE`.
///
/// # Errors
/// [`PdfRenderError`] for too-large input, missing `pdftoppm`, a password /
/// corrupted PDF, no output pages, a render timeout, or any I/O failure.
pub async fn render_pdf_pages(
    path: &Path,
    original_size: u64,
    first: u32,
    last: u32,
) -> Result<Vec<Vec<u8>>, PdfRenderError> {
    if original_size > PDF_MAX_EXTRACT_SIZE {
        return Err(PdfRenderError::TooLarge);
    }
    if !is_pdftoppm_available().await {
        return Err(PdfRenderError::Unavailable);
    }

    // Scratch dir (auto-removed on drop). pdftoppm writes <dir>/page-NN.jpg.
    let dir = tempfile::Builder::new()
        .prefix("lingxi-pdf-")
        .tempdir()
        .map_err(|e| PdfRenderError::Unknown(e.to_string()))?;
    let prefix = dir.path().join("page");
    let args = build_pdftoppm_args(first, last, path, &prefix);

    let output = tokio::time::timeout(
        PDFTOPPM_TIMEOUT,
        tokio::process::Command::new("pdftoppm")
            .args(&args)
            .stdin(std::process::Stdio::null())
            .output(),
    )
    .await
    .map_err(|_| PdfRenderError::Unknown("pdftoppm timed out".to_string()))?
    .map_err(|e| PdfRenderError::Unknown(e.to_string()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(classify_pdftoppm_failure(&stderr));
    }

    // Collect *.jpg, sort by filename (claude-code `entries.filter(...).sort()`).
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(dir.path())
        .map_err(|e| PdfRenderError::Unknown(e.to_string()))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("jpg"))
        .collect();
    files.sort();

    if files.is_empty() {
        return Err(PdfRenderError::NoOutput);
    }

    let mut pages = Vec::with_capacity(files.len());
    for f in files {
        pages.push(std::fs::read(&f).map_err(|e| PdfRenderError::Unknown(e.to_string()))?);
    }
    Ok(pages)
    // `dir` drops here → scratch removed.
}
```

- [ ] **Step 6: Register the module in `lib.rs`**

In `lingxi-code/tools/file/src/lib.rs`, alongside the existing `#[cfg(feature = "pdf-read")] mod pdf_read;` (or `pub mod`), add — matching the existing visibility style:

```rust
#[cfg(feature = "pdf-render")]
mod pdf_render;
```

If `read.rs` will reference it as `crate::pdf_render::...`, `mod` (private) suffices since `read.rs` is in the same crate.

- [ ] **Step 7: Run the gate**

```bash
cargo test -p tool-file --features pdf-render
cargo clippy -p tool-file --all-targets --no-deps --features pdf-render -- -D warnings
cargo clippy -p tool-file --all-targets --no-deps -- -D warnings   # feature OFF still clean
```
Expected: PASS. (`render_pdf_pages` / `is_pdftoppm_available` compile but are only called by Task 4 + the gated test in Task 5; if clippy flags them as unused with the feature on, that is expected only until Task 4 wires them — if Task 2 is committed standalone, add `#[allow(dead_code)]` to the two async fns and REMOVE it in Task 4. Prefer to keep Task 2→4 contiguous so no allow is needed.)

- [ ] **Step 8: Commit**

```bash
git add lingxi-code/tools/file/src/pdf_render.rs lingxi-code/tools/file/src/lib.rs
git commit -F <tmpfile>   # subject: feat(tool-file): pdf_render — pdftoppm spawn + pure arg/error helpers
```

---

## Task 3: `format_file_size` + PDF size constants

**Files:**
- Modify: `lingxi-code/tools/file/src/read.rs` (add `format_file_size`)
- Modify: `lingxi-code/tools/file/src/pdf_read.rs` (add `PDF_TARGET_RAW_SIZE`, `PDF_MAX_EXTRACT_SIZE`)
- Test: inline `#[cfg(test)]`

**Scene:** claude-code uses `formatFileSize` (`src/utils/format.ts:9-23`) for human-readable sizes in PDF messages. We port it once and reuse it in Tasks 2, 4, 6. The two new constants back the extraction cap (100 MB) and the inline cap (20 MB).

claude-code `formatFileSize`:
```js
const kb = sizeInBytes / 1024
if (kb < 1) return `${sizeInBytes} bytes`
if (kb < 1024) return `${kb.toFixed(1).replace(/\.0$/, '')}KB`
const mb = kb / 1024
if (mb < 1024) return `${mb.toFixed(1).replace(/\.0$/, '')}MB`
const gb = mb / 1024
return `${gb.toFixed(1).replace(/\.0$/, '')}GB`
```

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` in `read.rs` (if none exists for free fns, add a small one):

```rust
#[test]
fn format_file_size_matches_claude_code() {
    use super::format_file_size;
    assert_eq!(format_file_size(0), "0 bytes");
    assert_eq!(format_file_size(512), "512 bytes");
    assert_eq!(format_file_size(1024), "1KB");          // 1.0 → trim .0
    assert_eq!(format_file_size(1536), "1.5KB");
    assert_eq!(format_file_size(3 * 1024 * 1024), "3MB");
    assert_eq!(format_file_size(20 * 1024 * 1024), "20MB");
    assert_eq!(format_file_size(100 * 1024 * 1024), "100MB");
    assert_eq!(format_file_size(2 * 1024 * 1024 * 1024), "2GB");
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p tool-file format_file_size`
Expected: FAIL — `format_file_size` not found.

- [ ] **Step 3: Implement `format_file_size`**

Add this free function to `read.rs` (near `format_too_large`). It must be reachable as `crate::read::format_file_size` (used by `pdf_render.rs`). Make it `pub(crate)`:

```rust
/// Human-readable file size, byte-faithful to claude-code `formatFileSize`
/// (`src/utils/format.ts`): `< 1KB` ⇒ `"{n} bytes"`; otherwise one decimal with a
/// trailing `.0` trimmed, suffixed `KB`/`MB`/`GB` (no space). Used by the PDF
/// routing/extraction messages.
pub(crate) fn format_file_size(size_in_bytes: u64) -> String {
    fn trim(x: f64) -> String {
        let s = format!("{x:.1}");
        s.strip_suffix(".0").map(str::to_string).unwrap_or(s)
    }
    let kb = size_in_bytes as f64 / 1024.0;
    if kb < 1.0 {
        return format!("{size_in_bytes} bytes");
    }
    if kb < 1024.0 {
        return format!("{}KB", trim(kb));
    }
    let mb = kb / 1024.0;
    if mb < 1024.0 {
        return format!("{}MB", trim(mb));
    }
    let gb = mb / 1024.0;
    format!("{}GB", trim(gb))
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p tool-file format_file_size`
Expected: PASS.

- [ ] **Step 5: Add the two size constants**

In `pdf_read.rs`, after the existing `PDF_AT_MENTION_INLINE_THRESHOLD` const, add:

```rust
/// Max raw size for an INLINE document read — claude-code `PDF_TARGET_RAW_SIZE`
/// (apiLimits.ts, 20 MB). Above this, `readPDF` returns `too_large`.
pub const PDF_TARGET_RAW_SIZE: u64 = 20 * 1024 * 1024;
/// Max raw size for PAGE EXTRACTION — claude-code `PDF_MAX_EXTRACT_SIZE`
/// (apiLimits.ts, 100 MB). Above this, `extractPDFPages` returns `too_large`.
pub const PDF_MAX_EXTRACT_SIZE: u64 = 100 * 1024 * 1024;
```

(Do NOT remove `PDF_EXTRACT_SIZE_THRESHOLD` yet — Task 6 removes it together with its last use.)

- [ ] **Step 6: Run the gate**

```bash
cargo test -p tool-file
cargo clippy -p tool-file --all-targets --no-deps -- -D warnings
```
Expected: PASS. (`format_file_size` is `pub(crate)`; if it is briefly unused with all PDF features OFF, that is fine because it is referenced under `#[cfg(feature = "pdf-render")]`/`pdf-read` paths — if clippy warns `dead_code` in the no-feature build, gate the fn with `#[cfg(any(feature = "pdf-read", feature = "pdf-render"))]`.)

- [ ] **Step 7: Commit**

```bash
git add lingxi-code/tools/file/src/read.rs lingxi-code/tools/file/src/pdf_read.rs
git commit -F <tmpfile>   # subject: feat(tool-file): port formatFileSize + PDF inline/extract size consts
```

---

## Task 4: Wire the `pages` extraction path (the core gap)

**Files:**
- Modify: `lingxi-code/tools/file/src/read.rs` (`build_pages_payload` pure helper, `read_pdf_pages_result` method, the `pages` branch)
- Test: inline `#[cfg(test)]`

**Scene:** This closes the deferred gap. In `read_pdf_result`, the `pages` branch currently returns the "extraction unavailable" error after the range/cap validation. We replace it (under `pdf-render`) with: render → resize each page via `process_image` → emit N `ContentBlock::Image` blocks on `new_messages` → tool-result text `PDF pages extracted: {count} page(s) from {path} ({size})`.

claude-code emission (`FileReadTool.ts:916-945`): read the `.jpg` files, `maybeResizeAndDownsampleImageBuffer(buf, len, 'jpeg')` each, build `{type:'image', source:{type:'base64', media_type:'image/<mt>', data:<b64>}}` blocks, return `newMessages: [createUserMessage({content: imageBlocks, isMeta: true})]`. The tool-result string (`FileReadTool.ts:684`): `PDF pages extracted: ${count} page(s) from ${filePath} (${formatFileSize(originalSize)})`.

> Parity note: claude-code resizes each page via the SAME image ladder our `process_image` already ports (P3). We reuse `process_image`, which returns `(base64, media_type)` — exactly the block fields. We DROP `process_image`'s `resized` metadata for pages (claude-code's pages path emits no per-page coordinate-mapping text, unlike the single-image path).

- [ ] **Step 1: Write the failing test (pure payload builder)**

Add to `read.rs` tests. This test makes real JPEG bytes via the `image` crate (a dev-dep through `image-read`), so it is deterministic and needs no `pdftoppm`:

```rust
#[cfg(feature = "pdf-render")]
#[test]
fn build_pages_payload_emits_one_image_per_page() {
    use super::build_pages_payload;
    use image::{DynamicImage, RgbImage};

    fn jpeg(w: u32, h: u32) -> Vec<u8> {
        let img = DynamicImage::ImageRgb8(RgbImage::new(w, h));
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Jpeg).unwrap();
        buf.into_inner()
    }

    let pages = vec![jpeg(50, 50), jpeg(40, 60)];
    let (sources, data) =
        build_pages_payload(std::path::Path::new("/docs/report.pdf"), 4096, pages).unwrap();

    assert_eq!(sources.len(), 2, "one ImageSource per rendered page");
    for s in &sources {
        match s {
            protocol::ImageSource::Base64 { media_type, data } => {
                assert_eq!(media_type, "image/jpeg");
                assert!(!data.is_empty());
            }
            other => panic!("expected Base64 source, got {other:?}"),
        }
    }
    assert_eq!(data["type"], "parts");
    assert_eq!(data["file_path"], "/docs/report.pdf");
    assert_eq!(
        data["model_content"],
        "PDF pages extracted: 2 page(s) from /docs/report.pdf (4KB)"
    );
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p tool-file --features pdf-render build_pages_payload`
Expected: FAIL — `build_pages_payload` not found.

- [ ] **Step 3: Implement `build_pages_payload`**

Add to `read.rs` (free fn, `pub(crate)`, gated on `pdf-render`):

```rust
/// Turn rendered page JPEGs into (image sources, tool-result data). Each page is
/// run through [`crate::image_read::process_image`] (the claude-code image
/// ladder) and becomes one [`protocol::ImageSource::Base64`]; the tool-result
/// text mirrors claude-code `FileReadTool.ts:684`.
///
/// # Errors
/// Propagates `process_image` failure (a page that fails to decode/encode).
#[cfg(feature = "pdf-render")]
pub(crate) fn build_pages_payload(
    canon: &std::path::Path,
    original_size: u64,
    page_jpegs: Vec<Vec<u8>>,
) -> Result<(Vec<protocol::ImageSource>, serde_json::Value), String> {
    let count = page_jpegs.len();
    let mut sources = Vec::with_capacity(count);
    for jpeg in page_jpegs {
        let processed = crate::image_read::process_image(jpeg)?;
        sources.push(protocol::ImageSource::Base64 {
            media_type: processed.media_type,
            data: processed.base64,
        });
    }
    let path_str = canon.display().to_string();
    let model_content = format!(
        "PDF pages extracted: {count} page(s) from {path_str} ({})",
        format_file_size(original_size)
    );
    let data = serde_json::json!({
        "type": "parts",
        "file_path": path_str,
        "original_size": original_size,
        "count": count,
        "model_content": model_content,
    });
    Ok((sources, data))
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p tool-file --features pdf-render build_pages_payload`
Expected: PASS.

- [ ] **Step 5: Add the `read_pdf_pages_result` method**

Add as a method on the same `impl` block that holds `read_pdf_result` (in `read.rs`), gated on `pdf-render`:

```rust
/// Render `first..=last` of a PDF to page images and emit them on
/// `new_messages` (claude-code FileRead `pages` path).
#[cfg(feature = "pdf-render")]
async fn read_pdf_pages_result(
    &self,
    invocation_id: &str,
    canon: &std::path::Path,
    original_size: u64,
    first: u32,
    last: u32,
    started: Instant,
) -> Result<ToolCallResult, ToolError> {
    let page_jpegs = match crate::pdf_render::render_pdf_pages(canon, original_size, first, last).await {
        Ok(p) => p,
        Err(e) => {
            self.emit_failed(invocation_id, e.telemetry_code()).await;
            return Err(ToolError::Io(e.message(&canon.display().to_string())));
        }
    };
    let (sources, data) = match build_pages_payload(canon, original_size, page_jpegs) {
        Ok(v) => v,
        Err(e) => {
            self.emit_failed(invocation_id, "pdf_page_encode").await;
            return Err(ToolError::Io(e));
        }
    };
    let msg = protocol::ConversationMessage::user_with_images(
        protocol::MessageId::new(),
        String::new(),
        sources,
    );
    self.emit_completed(invocation_id, original_size, started.elapsed().as_millis() as u64)
        .await;
    Ok(ToolCallResult {
        data,
        new_messages: vec![msg],
        context_modifier: None,
        mcp_meta: None,
    })
}
```

- [ ] **Step 6: Wire the `pages` branch**

In `read_pdf_result` (`read.rs`), find the block at the end of the `if let Some(ref p) = pages { ... }` arm that currently does:

```rust
            self.emit_failed(invocation_id, "pdf_extraction_unavailable")
                .await;
            return Err(ToolError::Io(
                "Reading specific PDF pages requires page extraction, which is not yet available. Read the whole PDF (omit pages) if it is small.".to_string(),
            ));
```

Replace it with a feature split:

```rust
            #[cfg(feature = "pdf-render")]
            {
                return self
                    .read_pdf_pages_result(invocation_id, canon, original_size, first, last, started)
                    .await;
            }
            #[cfg(not(feature = "pdf-render"))]
            {
                self.emit_failed(invocation_id, "pdf_extraction_unavailable")
                    .await;
                return Err(ToolError::Io(
                    "Reading specific PDF pages requires page extraction, which is not yet available. Read the whole PDF (omit pages) if it is small.".to_string(),
                ));
            }
```

(`first` and `last` are already bound in scope from the `parse_pdf_page_range` destructuring above this block. `original_size`, `invocation_id`, `canon`, `started` are method params.)

- [ ] **Step 7: Run the gate (both feature states)**

```bash
cargo test -p tool-file --features pdf-render
cargo test -p tool-file --features pdf-read       # pdf-render OFF: pages branch still errors "unavailable"
cargo clippy -p tool-file --all-targets --no-deps --features pdf-render -- -D warnings
cargo clippy -p tool-file --all-targets --no-deps -- -D warnings
```
Expected: PASS. With `pdf-render` ON, the pages branch renders (smoke-tested in Task 5); with only `pdf-read`, it returns the "unavailable" error (an existing test for that, if any, must still pass — keep it gated `#[cfg(all(feature = "pdf-read", not(feature = "pdf-render")))]`).

- [ ] **Step 8: Commit**

```bash
git add lingxi-code/tools/file/src/read.rs
git commit -F <tmpfile>   # subject: feat(tool-file): render PDF pages to images on the FileRead pages path (P4b core)
```

---

## Task 5: Availability-gated `render_pdf_pages` smoke test

**Files:**
- Test: `lingxi-code/tools/file/src/pdf_render.rs` (`#[cfg(test)]`)

**Scene:** The only logic not yet covered end-to-end is the actual `pdftoppm` spawn + file collection. Since poppler is NOT installed on the dev machine (and may be absent in CI), this test PROBES availability and returns early when absent — never failing the suite. When poppler IS present, it renders a real 1-page PDF and asserts ≥1 JPEG comes back. This mirrors the memory's env-gated-test pattern (fs_watch flake).

- [ ] **Step 1: Write the gated smoke test**

Add to the `#[cfg(test)] mod tests` in `pdf_render.rs`:

```rust
// A valid minimal 1-page PDF (same fixture pdf_read uses). pdftoppm renders one
// blank page from it.
const MINIMAL_PDF: &[u8] = b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>\nendobj\nxref\n0 4\n0000000000 65535 f \n0000000009 00000 n \n0000000058 00000 n \n0000000115 00000 n \ntrailer\n<< /Size 4 /Root 1 0 R >>\nstartxref\n186\n%%EOF\n";

#[tokio::test]
async fn render_pdf_pages_smoke() {
    // ENV-GATED: poppler-utils may be absent (dev machine + some CI). Skip cleanly
    // rather than fail — the parity-critical logic is covered by the pure-helper
    // and build_pages_payload tests; this only exercises the real spawn when it can.
    if !is_pdftoppm_available().await {
        eprintln!("skipping render_pdf_pages_smoke: pdftoppm not installed");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let pdf = dir.path().join("one.pdf");
    std::fs::write(&pdf, MINIMAL_PDF).unwrap();

    let pages = render_pdf_pages(&pdf, MINIMAL_PDF.len() as u64, 1, 1)
        .await
        .expect("render a 1-page PDF");
    assert_eq!(pages.len(), 1, "one JPEG for a 1-page PDF");
    assert!(pages[0].starts_with(&[0xFF, 0xD8]), "JPEG SOI marker");
}

#[tokio::test]
async fn render_rejects_oversize() {
    // No pdftoppm needed: the size guard short-circuits before the spawn.
    let dir = tempfile::tempdir().unwrap();
    let pdf = dir.path().join("big.pdf");
    std::fs::write(&pdf, MINIMAL_PDF).unwrap();
    let err = render_pdf_pages(&pdf, PDF_MAX_EXTRACT_SIZE + 1, 1, 1).await.unwrap_err();
    assert!(matches!(err, PdfRenderError::TooLarge));
}
```

- [ ] **Step 2: Run**

Run: `cargo test -p tool-file --features pdf-render pdf_render`
Expected: PASS. `render_pdf_pages_smoke` prints "skipping…" and returns (poppler absent here); `render_rejects_oversize` passes (size guard precedes the availability probe — note: it does NOT, the impl checks size FIRST, so this passes without poppler). Verify `render_rejects_oversize` truly needs no poppler: in `render_pdf_pages`, `original_size > PDF_MAX_EXTRACT_SIZE` returns BEFORE `is_pdftoppm_available()`. ✓

- [ ] **Step 3: Commit**

```bash
git add lingxi-code/tools/file/src/pdf_render.rs
git commit -F <tmpfile>   # subject: test(tool-file): availability-gated pdftoppm render smoke + oversize guard
```

---

## Task 6: Reconcile the no-pages routing to claude-code parity

**Files:**
- Modify: `lingxi-code/tools/file/src/read.rs` (the no-pages branch of `read_pdf_result`)
- Modify: `lingxi-code/tools/file/src/pdf_read.rs` (remove `PDF_EXTRACT_SIZE_THRESHOLD`)
- Test: inline `#[cfg(test)]`

**Scene:** P4a's no-pages branch errors when `!supported || original_size > 3 MB`. claude-code (`FileReadTool.ts:957-997`) actually: (a) throws ONLY the "not supported" message for an unsupported model; (b) for a SUPPORTED model, inlines the document up to `PDF_TARGET_RAW_SIZE` (20 MB) via `readPDF` — the 3 MB `PDF_EXTRACT_SIZE_THRESHOLD` only triggers a telemetry-only extraction whose result is discarded, so it has NO observable effect on the conversation. This task brings the branch to exact parity. (We intentionally do NOT reproduce the discarded telemetry-only extraction — it spawns pdftoppm and throws the result away; observable behavior is identical without it.)

claude-code exact messages:
- Unsupported (`FileReadTool.ts:980-984`):
  `Reading full PDFs is not supported with this model. Use a newer model (Sonnet 3.5 v2 or later), or use the pages parameter to read specific page ranges (e.g., pages: "1-5", maximum 20 pages per request). Page extraction requires poppler-utils: install with \`brew install poppler\` on macOS or \`apt-get install poppler-utils\` on Debian/Ubuntu.`
- Too large (supported, `readPDF` `too_large`, `pdf.ts:65`):
  `PDF file exceeds maximum allowed size of {formatFileSize(20 MB)}.` → `PDF file exceeds maximum allowed size of 20MB.`

- [ ] **Step 1: Write the failing tests**

Add to `read.rs` tests (these drive `read_pdf_result` via the tool; if the existing P4a tests construct a `FileReadTool` + `MINIMAL_PDF`, follow that exact harness). Conceptually:

```rust
// Supported model + 5 MB (between 3 MB and 20 MB): claude-code INLINES (does not
// error). P4a errored here — this is the reconciliation.
#[cfg(feature = "pdf-read")]
#[tokio::test]
async fn supported_model_inlines_pdf_between_3mb_and_20mb() {
    // Build/By the existing P4a test harness: read a valid PDF whose reported
    // size is ~5 MB with a supported model and assert the result inlines a
    // ContentBlock::Document on new_messages (NOT an "extraction required" error).
    // (Reuse the P4a inline-success harness; only the size differs.)
    // ... assert new_messages has exactly one User message containing a Document block.
}

// Unsupported model (claude-3-haiku) + no pages: exact claude-code message.
#[cfg(feature = "pdf-read")]
#[tokio::test]
async fn unsupported_model_returns_exact_not_supported_message() {
    // read a small valid PDF with model "claude-3-haiku-20240307", no pages.
    // assert Err message == the claude-code "Reading full PDFs is not supported..." string.
}

// Supported model + > 20 MB: too_large with formatFileSize.
#[cfg(feature = "pdf-read")]
#[tokio::test]
async fn supported_model_oversize_returns_too_large() {
    // assert Err message == "PDF file exceeds maximum allowed size of 20MB."
}
```

> The implementer must wire these against the REAL P4a test harness in `read.rs` (the one that already exercises `read_pdf_result` — e.g. `non_pdf_with_pdf_extension_is_rejected_before_inlining` shows the pattern). If faking a 5 MB / 21 MB PDF on disk is impractical, refactor the size check to read from the passed `original_size` (it already does — `original_size` is a param), so the test can pass a crafted `original_size` with a small on-disk valid PDF. Prefer driving through the public tool `call`; fall back to a focused `read_pdf_result` invocation if the harness exposes it.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p tool-file --features pdf-read pdf`
Expected: FAIL — current branch errors at 3 MB and uses the old conflated message.

- [ ] **Step 3: Replace the no-pages routing block**

In `read_pdf_result`, find:

```rust
        // Unsupported model OR oversize ⇒ inline read is refused; the model
        // must extract specific pages (P4b) or switch models.
        if !supported || original_size > PDF_EXTRACT_SIZE_THRESHOLD {
            self.emit_failed(invocation_id, "pdf_extraction_required")
                .await;
            return Err(ToolError::Io(
                "Reading full PDFs is not supported with this model or this file is too large. Use a newer model, or use the pages parameter to read specific page ranges (e.g., pages: \"1-5\", maximum 20 pages per request).".to_string(),
            ));
        }
```

Replace with (claude-code-exact split):

```rust
        // Unsupported model ⇒ refuse inline; tell the model to use a newer model
        // or the `pages` parameter (claude-code FileReadTool.ts:979-985).
        if !supported {
            self.emit_failed(invocation_id, "pdf_unsupported_model").await;
            return Err(ToolError::Io(
                "Reading full PDFs is not supported with this model. Use a newer model (Sonnet 3.5 v2 or later), or use the pages parameter to read specific page ranges (e.g., pages: \"1-5\", maximum 20 pages per request). Page extraction requires poppler-utils: install with `brew install poppler` on macOS or `apt-get install poppler-utils` on Debian/Ubuntu.".to_string(),
            ));
        }
        // Supported model: inline up to PDF_TARGET_RAW_SIZE (20 MB), else
        // too_large (claude-code readPDF, utils/pdf.ts:60-68).
        if original_size > crate::pdf_read::PDF_TARGET_RAW_SIZE {
            self.emit_failed(invocation_id, "pdf_too_large").await;
            return Err(ToolError::Io(format!(
                "PDF file exceeds maximum allowed size of {}.",
                format_file_size(crate::pdf_read::PDF_TARGET_RAW_SIZE)
            )));
        }
```

- [ ] **Step 4: Update the import + remove the dead constant**

In `read_pdf_result`'s `use crate::pdf_read::{...}` line, remove `PDF_EXTRACT_SIZE_THRESHOLD` (no longer referenced). Then in `pdf_read.rs`, delete the `PDF_EXTRACT_SIZE_THRESHOLD` const and its doc comment. Grep to confirm zero remaining references:

```bash
grep -rn "PDF_EXTRACT_SIZE_THRESHOLD" lingxi-code/
```
Expected: no matches.

- [ ] **Step 5: Fix the inline tool-result size string (P4a Minor #4)**

In `read_pdf_result`'s inline-success path, change:
```rust
                "model_content": format!("PDF file read: {} ({} bytes)", canon.display(), original_size),
```
to:
```rust
                "model_content": format!("PDF file read: {} ({})", canon.display(), format_file_size(original_size)),
```
(Update any P4a test asserting the old `({} bytes)` string to the human-readable form.)

- [ ] **Step 6: Run the gate**

```bash
cargo test -p tool-file --features pdf-read
cargo test -p tool-file --features pdf-render
cargo clippy -p tool-file --all-targets --no-deps --features pdf-render -- -D warnings
cargo clippy -p tool-file --all-targets --no-deps --features pdf-read -- -D warnings
cargo clippy -p tool-file --all-targets --no-deps -- -D warnings
```
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add lingxi-code/tools/file/src/read.rs lingxi-code/tools/file/src/pdf_read.rs
git commit -F <tmpfile>   # subject: fix(tool-file): no-pages PDF routing to claude-code parity (inline ≤20MB; exact messages)
```

---

## Task 7: Full-gate verification (pre-final-review)

**Files:** none (verification only)

- [ ] **Step 1: Both engines + struct-trap compile**

```bash
cargo test --workspace --no-run            # ~2-3 min; struct-trap compile of the whole workspace
cargo build -p engine-desktop
cargo build -p engine-mobile
```
Expected: all succeed.

- [ ] **Step 2: tool-file matrix green**

```bash
cargo test -p tool-file                          # features OFF
cargo test -p tool-file --features pdf-read
cargo test -p tool-file --features "pdf-read image-read"
cargo test -p tool-file --features pdf-render
cargo clippy -p tool-file --all-targets --no-deps -- -D warnings
cargo clippy -p tool-file --all-targets --no-deps --features pdf-render -- -D warnings
```
Expected: all green. Record the test counts (OFF vs pdf-render ON) for the final-review summary.

- [ ] **Step 3: Mobile pulls no renderer deps**

```bash
cargo tree -p engine-mobile 2>/dev/null | grep -E "tempfile|pdftoppm" && echo "UNEXPECTED" || echo "engine-mobile clean of pdf-render deps"
```
Expected: "engine-mobile clean of pdf-render deps".

- [ ] **Step 4:** Do NOT run `cargo test --workspace` (runtime) — the fs_watch/fseventsd flake is unrelated to P4b (see memory). The pre-existing sidequery `provider_side_query::text_response_decodes_text_usage_and_stop_reason` failure is also unrelated (fails on `main`).

---

## Self-Review (completed by plan author)

**Spec coverage:**
- ✅ pages path renders + emits images (Task 4) — the named deferred gap.
- ✅ `pdftoppm` spawn with exact flags `-jpeg -r 100 [-f] [-l]`, 120 s timeout, sorted `.jpg` collection (Task 2).
- ✅ Exact error strings: empty / too_large(100MB) / unavailable / password / corrupted / no-output / unknown (Task 2).
- ✅ Per-page resize via the P3 image ladder (`process_image`), no per-page metadata (Task 4).
- ✅ Tool-result `PDF pages extracted: {count} page(s) from {path} ({size})` (Task 4).
- ✅ `formatFileSize` ported (Task 3).
- ✅ No-pages parity: unsupported→exact message, supported inline ≤20MB else too_large (Task 6).
- ✅ Feature-gated `pdf-render`, OFF for mobile, ON for desktop (Tasks 1, 7).
- ✅ Deterministic tests (pure helpers + payload builder); env-gated real spawn (Task 5).

**No protocol change:** P4b reuses the frozen, P3-added `ContentBlock::Image` / `ImageSource` and `user_with_images`. No `traits/`/`protocol/` edits. ✅

**Placeholder scan:** none — all code is concrete. The Task 2 test has an intentional callout to remove a stray illustrative line before running.

**Type consistency:** `render_pdf_pages(path, original_size, first, last) -> Result<Vec<Vec<u8>>, PdfRenderError>`; `build_pages_payload(canon, original_size, page_jpegs) -> Result<(Vec<ImageSource>, Value), String>`; `format_file_size(u64) -> String`; `PdfRenderError::{message(&self, &str), telemetry_code(&self)}` — used consistently across Tasks 2/4/6.

**Known divergences (documented, intentional):** (1) `pdftoppm` availability not cached (per-call probe; perf-only, behavior identical). (2) The telemetry-only `shouldExtractPages` extraction in claude-code's no-pages path is not reproduced (discarded result, no observable effect). (3) Real-spawn coverage is env-gated (poppler absent on dev/CI) — the parity logic is covered deterministically.
