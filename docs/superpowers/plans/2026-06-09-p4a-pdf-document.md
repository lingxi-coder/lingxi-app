# P4a — PDF reading in FileRead (inline document block)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `FileReadTool` read small PDFs and surface them to the model as an inline document block — matching claude-code's FileRead PDF path — by adding the (approved) additive `ContentBlock::Document` variant and a PDF branch behind a `pdf-read` feature. Large/many-page/unsupported-model PDFs return claude-code's "use the pages parameter / extraction required" error (the page-image extraction is P4b, deferred — it needs a heavy PDF *render* crate not in codex).

**Architecture:** Adds an additive `ContentBlock::Document { source: DocumentSource }` to the frozen `protocol` crate (Anthropic/Bedrock auto-serialize it via serde; the OpenAI/Gemini encoders + ~9 other exhaustive `ContentBlock` matches get a new arm). `tool-file` gains a `pdf-read` feature (`lopdf` for the page-count routing check + `base64`); `read.rs` adds a PDF branch (parallel to P3's image branch, before the binary guard) that parses an additive `pages` input, applies claude-code's routing, and for an inline PDF emits the base64 as a `ContentBlock::Document` on the existing `new_messages` seam. Feature off ⇒ byte-identical (PDFs still hit the binary guard). **lopdf is a PDF *parser* (page count), not a renderer** — the renderer is P4b only.

**Tech Stack:** Rust 1.82; `lopdf` (pure-Rust PDF parser, page count) + `base64`, behind `pdf-read`. Verdict/output in-crate tested.

**Reference of truth:** `claude-code/src/tools/FileReadTool/FileReadTool.ts` (PDF routing), `src/utils/pdf.ts` + `pdfUtils.ts`, `src/constants/apiLimits.ts`. **Functional-parity bar** (page extraction deferred; lopdf page-count replaces pdfinfo).

---

## File Structure

- **Modify** `lingxi-code/protocol/src/messages.rs` — add `ContentBlock::Document` + `DocumentSource` + a wire round-trip test.
- **Modify** (1 arm each, ~8 files) the exhaustive `ContentBlock` matches: `providers/src/openai/encode.rs`, `providers/src/gemini/encode.rs`, `protocol/src/message_size.rs`, `compaction/src/strip_media.rs`, `core/src/prompt.rs`, `commands/core/src/export.rs`, `tui/src/replay.rs`, `client-adapter/src/turn.rs`; and 3 `matches!` predicates in `orchestrator/src/provider_adapter.rs`.
- **Modify** `lingxi-code/tools/file/Cargo.toml` — `pdf-read` feature (`lopdf` + `base64`).
- **Create** `lingxi-code/tools/file/src/pdf_read.rs` — detect + page count + routing helpers.
- **Modify** `lingxi-code/tools/file/src/{lib.rs,read.rs}` — the PDF branch + `pages` input.
- **Modify** `lingxi-code/apps/engine-desktop/Cargo.toml` — enable `pdf-read`.

**Constants** (claude-code `apiLimits.ts`): `PDF_EXTRACT_SIZE_THRESHOLD = 3*1024*1024`, `PDF_MAX_PAGES_PER_READ = 20`, `PDF_AT_MENTION_INLINE_THRESHOLD = 10`.

---

## Task 1: add `ContentBlock::Document` + `DocumentSource` (frozen, additive)

**Files:** Modify `lingxi-code/protocol/src/messages.rs`

- [ ] **Step 1: Write the failing wire test**

Add a test mirroring `image_base64_block_matches_anthropic_wire`:

```rust
    #[test]
    fn document_base64_block_matches_anthropic_wire() {
        let block = ContentBlock::Document {
            source: DocumentSource::Base64 {
                media_type: "application/pdf".to_string(),
                data: "JVBERi0=".to_string(),
            },
        };
        let v = serde_json::to_value(&block).unwrap();
        assert_eq!(
            v,
            serde_json::json!({
                "type": "document",
                "source": { "type": "base64", "media_type": "application/pdf", "data": "JVBERi0=" }
            })
        );
        let back: ContentBlock = serde_json::from_value(v).unwrap();
        assert_eq!(back, block);
    }
```

Run: `cargo test -p protocol document_base64_block_matches_anthropic_wire` → FAIL (no `Document` variant).

- [ ] **Step 2: Add the variant + the source enum**

In `enum ContentBlock { ... }`, add as the LAST variant (after `Image`):

```rust
    /// A document input (e.g. a PDF). Serializes to the Anthropic document-block
    /// wire shape; the OpenAI/Gemini codecs translate or drop it.
    Document {
        /// Where the document bytes come from.
        source: DocumentSource,
    },
```

Right after `enum ImageSource { ... }`, add:

```rust
/// Source of a [`ContentBlock::Document`]. Serializes to Anthropic's `source`
/// wire shape (`{"type":"base64","media_type":"application/pdf","data":…}`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DocumentSource {
    /// Inline base64-encoded document bytes.
    Base64 {
        /// MIME type, e.g. `application/pdf`.
        media_type: String,
        /// Base64-encoded document bytes (no `data:` prefix).
        data: String,
    },
}
```

If `DocumentSource` needs re-export (mirror `ImageSource`), add it to the `pub use` in `protocol/src/lib.rs`.

- [ ] **Step 3: Run to verify it passes**

Run: `cargo test -p protocol document_base64_block_matches_anthropic_wire` → PASS.
Run: `cargo build -p protocol` → builds (note: dependent crates will now have non-exhaustive-match errors — fixed in Task 2).

- [ ] **Step 4: Commit**

```bash
git add lingxi-code/protocol/src/messages.rs lingxi-code/protocol/src/lib.rs
git commit -m "feat(protocol): additive ContentBlock::Document + DocumentSource (Anthropic doc wire)"
```

---

## Task 2: handle `Document` in every exhaustive `ContentBlock` match (blast radius)

**Files:** the ~8 files + `provider_adapter.rs` below. Add ONE arm each, by analogy to the `Image` arm at the same site.

- [ ] **Step 1: Find all the broken matches**

Run: `cargo build --workspace 2>&1 | grep -E 'error\[E0004\]|not covered|pattern .* not covered' -A2 | head -60`
This lists every exhaustive `match` missing a `Document` arm. The known set (from research):

- [ ] **Step 2: Add each arm** (exact code; `DocumentSource` is `protocol::DocumentSource`):

1. `providers/src/openai/encode.rs` (`encode_user`, near the Image arm): OpenAI chat has no native PDF — emit a text placeholder so the model knows a PDF was present:
   ```rust
   ContentBlock::Document { .. } => {
       texts.push("[PDF document — not supported on this provider]".to_string());
   }
   ```
   (Match the actual local accumulator name — it may be `parts`/`texts`/`content`. If a text part can't be pushed there, fold `Document` into the existing skip arm instead.)
2. `providers/src/openai/encode.rs` (`encode_assistant` no-op arm): add `Document` to the skip arm: `ContentBlock::ToolResult { .. } | ContentBlock::Thinking { .. } | ContentBlock::Image { .. } | ContentBlock::Document { .. } => {}`.
3. `providers/src/gemini/encode.rs` (`encode_user`, near the Image arm): Gemini supports inline PDF via `inlineData`:
   ```rust
   ContentBlock::Document { source } => match source {
       protocol::DocumentSource::Base64 { media_type, data } => {
           parts.push(serde_json::json!({"inlineData": {"mimeType": media_type, "data": data}}));
       }
   },
   ```
4. `providers/src/gemini/encode.rs` (`encode_assistant` no-op arm): add `Document` to the skip arm.
5. `protocol/src/message_size.rs` (`content_block_size`, near the Image Base64 arm):
   ```rust
   ContentBlock::Document { source } => match source {
       DocumentSource::Base64 { data, .. } => data.len() as u64,
   },
   ```
6. `compaction/src/strip_media.rs` (`strip_one`, near the Image placeholder): add a `STRIPPED_DOCUMENT_PLACEHOLDER` const (`"[document]"`) next to `STRIPPED_IMAGE_PLACEHOLDER`, then:
   ```rust
   ContentBlock::Document { .. } => ContentBlock::Text { text: STRIPPED_DOCUMENT_PLACEHOLDER.to_string() },
   ```
   And update the module-level doc comment that currently says the Rust protocol has no Document variant.
7. `core/src/prompt.rs` (`content_blocks_to_api`, near the Image arm):
   ```rust
   ContentBlock::Document { source } => serde_json::json!({"type": "document", "source": source}),
   ```
8. `commands/core/src/export.rs` (`render_blocks`, near the Image `"[image]"` line):
   ```rust
   ContentBlock::Document { .. } => lines.push("[document]".to_string()),
   ```
9. `tui/src/replay.rs` (`push_user_block` AND `push_assistant_block` no-op arms): add `Document` to each existing skip arm (it's not replayed in scrollback, like `Image`).
10. `client-adapter/src/turn.rs` (`lower_content_block`, near the Image `=> None` arm): add `ContentBlock::Document { .. } => None` (no scrollback DTO, like Image). Update the dropped-variants doc comment.

- [ ] **Step 3: The 3 semantic `matches!` predicates in `orchestrator/src/provider_adapter.rs`** (these compile WITHOUT the arm but are wrong — TS `isMedia` counts documents):

   - `count_media` (~:60): add `|| matches!(b, ContentBlock::Document { .. })` so documents count toward `MAX_MEDIA_PER_REQUEST`.
   - `strip_excess_media` (~:95): add `|| matches!(b, ContentBlock::Document { .. })` to the retain/strip predicate so excess documents are trimmed too.
   - `messages_contain_image` (~:32): add `|| matches!(b, ContentBlock::Document { .. })` (a document should also route to a media-capable model). Update the stale "no Document variant" doc comment near :48.

- [ ] **Step 4: Build the whole workspace**

Run: `cargo test --workspace --no-run`
Expected: builds with no errors (all exhaustive matches now covered). If `grep -rn 'ContentBlock::' --include=*.rs lingxi-code | grep -i 'match\|=>'` surfaces a site the build missed (e.g. a `matches!` that should count documents), reconcile it.

- [ ] **Step 5: Test the touched crates**

Run: `cargo test -p protocol -p providers -p compaction -p orchestrator -p tui -p client-adapter -p commands-core 2>&1 | grep -E 'test result|FAILED'`
Expected: all pass (additive — existing behavior unchanged; documents simply now have a handled arm).

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(protocol+providers): handle ContentBlock::Document across encoders/strip/size/replay/adapter"
```

---

## Task 3: `tool-file` pdf-read feature + `pdf_read` module (DE-RISK lopdf)

**Files:** Modify `tools/file/Cargo.toml`; create `tools/file/src/pdf_read.rs`; modify `tools/file/src/lib.rs`

- [ ] **Step 1: Add the feature + deps**

In `tools/file/Cargo.toml` `[dependencies]` (base64 already optional from P3 — reuse it):

```toml
# PDF page-count parsing for FileRead PDF routing (claude-code's getPDFPageCount).
# Pure-Rust parser (NOT a renderer — page-image extraction is P4b). Optional +
# gated behind `pdf-read`.
lopdf = { version = "0.34", default-features = false, optional = true }
```

Extend `[features]`:

```toml
pdf-read = ["dep:lopdf", "dep:base64"]
```

- [ ] **Step 2: DE-RISK — verify lopdf builds on 1.82**

Run: `cargo check -p tool-file --features pdf-read`
Expected: success. **If it fails on an MSRV/edition2024 error**, try older `lopdf` lines in order: `0.32` → `0.31` → `0.30` (the `Document::load_mem(&bytes)?.get_pages().len()` API is stable across these). If `default-features = false` drops a needed feature (e.g. the parser), try without it. Pin the resolved version `= ` exact (`cargo tree -p tool-file --features pdf-read -i lopdf`). If no 0.3x builds on 1.82, report BLOCKED.

- [ ] **Step 3: Declare the module + write failing tests**

In `tools/file/src/lib.rs`:
```rust
#[cfg(feature = "pdf-read")]
pub mod pdf_read;
```

Create `pdf_read.rs` with tests first:
```rust
//! FileRead PDF reading: detect, page-count (via lopdf), and the routing helpers
//! that mirror claude-code FileRead's PDF branch (utils/pdf.ts + pdfUtils.ts).
//! lopdf is a pure-Rust PARSER (page count) — page-image extraction is P4b.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_pdf_extension_and_magic() {
        assert!(is_pdf_path(std::path::Path::new("a.pdf")));
        assert!(is_pdf_path(std::path::Path::new("a.PDF")));
        assert!(!is_pdf_path(std::path::Path::new("a.png")));
        assert!(looks_like_pdf(b"%PDF-1.7\n..."));
        assert!(!looks_like_pdf(b"not a pdf"));
    }

    #[test]
    fn parse_page_range_cases() {
        assert_eq!(parse_pdf_page_range("3"), Some((3, 3)));
        assert_eq!(parse_pdf_page_range("1-5"), Some((1, 5)));
        assert_eq!(parse_pdf_page_range("10-20"), Some((10, 20)));
        assert_eq!(parse_pdf_page_range("3-"), Some((3, u32::MAX))); // open-ended
        assert_eq!(parse_pdf_page_range(""), None);
        assert_eq!(parse_pdf_page_range("0"), None);
        assert_eq!(parse_pdf_page_range("5-1"), None); // inverted
        assert_eq!(parse_pdf_page_range("x"), None);
    }

    #[test]
    fn is_pdf_supported_excludes_haiku3() {
        assert!(is_pdf_supported("claude-sonnet-4-6"));
        assert!(!is_pdf_supported("claude-3-haiku-20240307"));
        assert!(!is_pdf_supported("anthropic.claude-3-haiku"));
    }
}
```
Run: `cargo test -p tool-file --features pdf-read --lib pdf_read::` → FAIL.

- [ ] **Step 4: Implement**

```rust
/// claude-code apiLimits.ts.
pub const PDF_EXTRACT_SIZE_THRESHOLD: u64 = 3 * 1024 * 1024;
pub const PDF_MAX_PAGES_PER_READ: u32 = 20;
pub const PDF_AT_MENTION_INLINE_THRESHOLD: u32 = 10;

/// `.pdf` extension (claude-code DOCUMENT_EXTENSIONS).
#[must_use]
pub fn is_pdf_path(path: &std::path::Path) -> bool {
    path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() == Some("pdf")
}

/// `%PDF-` magic (claude-code readPDF header check).
#[must_use]
pub fn looks_like_pdf(bytes: &[u8]) -> bool {
    bytes.starts_with(b"%PDF-")
}

/// claude-code isPDFSupported: every model EXCEPT one whose id contains `claude-3-haiku`.
#[must_use]
pub fn is_pdf_supported(model: &str) -> bool {
    !model.to_ascii_lowercase().contains("claude-3-haiku")
}

/// 1-indexed inclusive page range (claude-code parsePDFPageRange). `lastPage`
/// is `u32::MAX` for open-ended `"N-"`. `None` on invalid/empty/inverted.
#[must_use]
pub fn parse_pdf_page_range(pages: &str) -> Option<(u32, u32)> {
    let t = pages.trim();
    if t.is_empty() {
        return None;
    }
    if let Some(left) = t.strip_suffix('-') {
        let first: u32 = left.parse().ok()?;
        return (first >= 1).then_some((first, u32::MAX));
    }
    if let Some((l, r)) = t.split_once('-') {
        let first: u32 = l.trim().parse().ok()?;
        let last: u32 = r.trim().parse().ok()?;
        return (first >= 1 && last >= 1 && last >= first).then_some((first, last));
    }
    let page: u32 = t.parse().ok()?;
    (page >= 1).then_some((page, page))
}

/// Page count via lopdf (claude-code getPDFPageCount). `None` if the bytes don't
/// parse (mirrors pdfinfo returning null → the >10-page gate is skipped).
#[must_use]
pub fn pdf_page_count(bytes: &[u8]) -> Option<u32> {
    let doc = lopdf::Document::load_mem(bytes).ok()?;
    u32::try_from(doc.get_pages().len()).ok()
}
```

Run: `cargo test -p tool-file --features pdf-read --lib pdf_read::` → PASS.

- [ ] **Step 5: Commit**

```bash
git add lingxi-code/tools/file/Cargo.toml lingxi-code/tools/file/src/pdf_read.rs lingxi-code/tools/file/src/lib.rs lingxi-code/Cargo.lock
git commit -m "feat(tool-file): pdf-read feature + pdf_read module (detect, page-count, routing helpers)"
```

---

## Task 4: the FileRead PDF branch (routing + inline document)

**Files:** Modify `tools/file/src/read.rs`

- [ ] **Step 1: Add `pages` to the input schema (additive)**

In `INPUT_SCHEMA` (`read.rs:500`), add to `properties` (it's `additionalProperties:false`, so `pages` MUST be declared):
```rust
            "pages": { "type": "string" },
```

- [ ] **Step 2: Compute `is_pdf` + skip the text size-gate for PDFs**

Add a free helper near `is_image_path`:
```rust
/// `.pdf` (so it compiles with `pdf-read` off). Routes to the document path.
fn is_pdf_path(path: &std::path::Path) -> bool {
    path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() == Some("pdf")
}
```
Before the size gate, add `let is_pdf = cfg!(feature = "pdf-read") && is_pdf_path(&canon);` and change the gate to `if !is_image && !is_pdf && input_limit.is_none() && size > MAX_FILE_READ_SIZE {`.

- [ ] **Step 3: Add the PDF branch after `fs::read`, before the binary guard** (right after the image branch):
```rust
        #[cfg(feature = "pdf-read")]
        if is_pdf {
            let pages = input.get("pages").and_then(serde_json::Value::as_str).map(str::to_string);
            return self.read_pdf_result(&invocation_id, &canon, bytes, size, pages, started).await;
        }
```

- [ ] **Step 4: Add `read_pdf_result`** (the routing — port of claude-code FileReadTool.ts; the model id comes from the tool context — read how `read.rs` accesses the configured model, e.g. `self.ctx.default_model` like the WebFetch/permission code, and adapt):
```rust
    /// Read a PDF and return it as an inline document block (claude-code FileRead
    /// PDF path). Routing: `pages` ⇒ extraction (P4b, not yet implemented ⇒ the
    /// claude-code "use pages" guidance error); else page-count > 10 ⇒ error;
    /// unsupported model OR size > 3MB ⇒ extraction-required error; else inline.
    #[cfg(feature = "pdf-read")]
    async fn read_pdf_result(
        &self,
        invocation_id: &str,
        canon: &std::path::Path,
        bytes: Vec<u8>,
        original_size: u64,
        pages: Option<String>,
        started: std::time::Instant,
    ) -> Result<ToolCallResult, ToolError> {
        use crate::pdf_read::{
            is_pdf_supported, parse_pdf_page_range, pdf_page_count, PDF_AT_MENTION_INLINE_THRESHOLD,
            PDF_EXTRACT_SIZE_THRESHOLD, PDF_MAX_PAGES_PER_READ,
        };
        let model = self.ctx.default_model.clone(); // adapt to the real model accessor

        // pages supplied → validate, then defer to P4b extraction.
        if let Some(ref p) = pages {
            let Some((first, last)) = parse_pdf_page_range(p) else {
                self.emit_failed(invocation_id, "pdf_pages_invalid").await;
                return Err(ToolError::Io(format!(
                    "Invalid pages parameter: \"{p}\". Use formats like \"1-5\", \"3\", or \"10-20\". Pages are 1-indexed."
                )));
            };
            let range_size = if last == u32::MAX { PDF_MAX_PAGES_PER_READ + 1 } else { last - first + 1 };
            if range_size > PDF_MAX_PAGES_PER_READ {
                self.emit_failed(invocation_id, "pdf_pages_too_many").await;
                return Err(ToolError::Io(format!(
                    "Page range \"{p}\" exceeds maximum of {PDF_MAX_PAGES_PER_READ} pages per request. Please use a smaller range."
                )));
            }
            self.emit_failed(invocation_id, "pdf_extraction_unavailable").await;
            return Err(ToolError::Io(format!(
                "Reading specific PDF pages requires page extraction, which is not yet available. Read the whole PDF (omit pages) if it is small, or use a model that supports full PDFs."
            )));
        }

        // no pages → page-count gate.
        if let Some(count) = pdf_page_count(&bytes) {
            if count > PDF_AT_MENTION_INLINE_THRESHOLD {
                self.emit_failed(invocation_id, "pdf_too_many_pages").await;
                return Err(ToolError::Io(format!(
                    "This PDF has {count} pages, which is too many to read at once. Use the pages parameter to read specific page ranges (e.g., pages: \"1-5\"). Maximum {PDF_MAX_PAGES_PER_READ} pages per request."
                )));
            }
        }

        // unsupported model OR oversize → extraction-required (P4b).
        if !is_pdf_supported(&model) || original_size > PDF_EXTRACT_SIZE_THRESHOLD {
            self.emit_failed(invocation_id, "pdf_extraction_required").await;
            return Err(ToolError::Io(
                "Reading full PDFs is not supported with this model or this file is too large. Use a newer model, or use the pages parameter to read specific page ranges (e.g., pages: \"1-5\", maximum 20 pages per request).".to_string(),
            ));
        }

        // inline document path.
        use base64::Engine;
        let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
        let source = protocol::DocumentSource::Base64 { media_type: "application/pdf".to_string(), data };
        let msg = protocol::ConversationMessage::user_with_documents(
            protocol::MessageId::new(),
            String::new(),
            vec![source],
        );
        self.emit_completed(invocation_id, original_size, started.elapsed().as_millis() as u64).await;
        Ok(ToolCallResult {
            data: serde_json::json!({
                "type": "pdf",
                "file_path": canon.display().to_string(),
                "original_size": original_size,
                "model_content": format!("PDF file read: {} ({} bytes)", canon.display(), original_size),
            }),
            new_messages: vec![msg],
            context_modifier: None,
            mcp_meta: None,
        })
    }
```
NOTE: this uses `ConversationMessage::user_with_documents` — that constructor does NOT exist yet. Add it to `protocol/src/messages.rs` next to `user_with_images` (a Task-1 addition is cleaner, but it's fine here since it's protocol-additive):
```rust
    /// Like [`Self::user_with_images`] but for document sources (P4a).
    #[must_use]
    pub fn user_with_documents(id: MessageId, text: String, documents: Vec<DocumentSource>) -> Self {
        let mut content = Vec::new();
        if !text.is_empty() {
            content.push(ContentBlock::Text { text });
        }
        for source in documents {
            content.push(ContentBlock::Document { source });
        }
        Self::User { id, content }
    }
```
(Match the exact `Self::User { id, content }` shape `user_with_images` uses.) Reconcile `self.ctx.default_model`, `emit_completed`/`emit_failed`, `self.ctx` access against the real `read.rs` (mirror `read_image_result`).

- [ ] **Step 5: Test the inline + routing cases**

Add tests mirroring the image test: a tiny valid 1-page PDF (a minimal `%PDF-1.4 … 1 page` byte string, or build via lopdf) → inline document (`data["type"]=="pdf"`, `new_messages` has a `ContentBlock::Document { Base64 }`). And a routing test: a `pages: "1-25"` input → the cap error. (For "too many pages" / oversize you can unit-test the routing helpers directly in `pdf_read.rs` rather than constructing big PDFs.)

Run: `cargo test -p tool-file --features pdf-read,image-read --lib` → all pass; `cargo test -p tool-file --lib` (no features) → byte-identical (PDF still rejected by the binary guard).

- [ ] **Step 6: Commit**

```bash
git add lingxi-code/tools/file/src/read.rs lingxi-code/protocol/src/messages.rs
git commit -m "feat(tool-file): FileRead PDF branch - inline document block + routing (P4a)"
```

---

## Task 5: enable on desktop + gate

- [ ] **Step 1:** In `apps/engine-desktop/Cargo.toml`, change the `tool-file` dep to add `pdf-read` to its features (alongside `image-read`): `features = ["image-read", "pdf-read"]`. Do NOT change engine-mobile.
- [ ] **Step 2:** `cargo build -p engine-desktop && cargo build -p engine-mobile` → both build. `cargo tree -p engine-mobile 2>/dev/null | grep -c '^lopdf'` → `0`.
- [ ] **Step 3:** Full gate: `cargo test --workspace --no-run`; `cargo test -p tool-file -p protocol -p providers`; `cargo clippy -p tool-file -p protocol -p providers --all-targets --no-deps -- -D warnings` (and the `--features pdf-read,image-read` variant for tool-file). Confirm the FileRead text/notebook/image fixtures + the touched-crate suites are unchanged.
- [ ] **Step 4:** Commit: `git add -A && git commit -m "feat(engine-desktop): enable tool-file pdf-read feature"`.

---

## Notes & follow-ups

- **P4b (deferred):** page-image extraction (`pages` input, or oversize/unsupported/>10-page PDFs) — render each page to a JPEG and reuse P3's image path. Needs a PDF *render* crate (`pdfium-render` — C/native deps, or a pure-Rust rasterizer) NOT in codex; that's why 4b is its own plan. Until then, those cases return the claude-code-style "use pages / extraction required" error.
- **Functional-parity simplifications:** lopdf page-count replaces claude-code's `pdfinfo` shell-out (no poppler dependency); the inline `%PDF-` validity is implicitly checked by lopdf parsing; the error strings are claude-code-faithful where the same path exists, adapted where extraction is deferred.
- **engine-mobile** stays off `pdf-read` (lean); could opt in later for small-PDF document-block reading (lopdf is pure-Rust, light).
