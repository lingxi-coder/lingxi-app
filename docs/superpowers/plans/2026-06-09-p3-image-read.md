# P3 — Image reading in FileRead (multimodal, via the existing new_messages seam)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `FileReadTool` read image files (png/jpg/jpeg/gif/webp) and surface them to the model as multimodal content — matching claude-code's FileRead image path — instead of rejecting them as binary. Behind a `image-read` cargo feature (on for engine-desktop, off for engine-mobile). Feature off ⇒ byte-identical (images still hit the binary guard).

**Architecture:** Today `read.rs` rejects every image at the binary guard (`read.rs:610`, `looks_binary` — image headers contain NUL bytes). P3 adds an image branch *before* that guard: detect an image by extension, decode + resize/re-encode it to fit claude-code's size/dimension budget via the `image` crate (codex `utils/image` how-to), base64-encode it, and emit it as a `ContentBlock::Image` on the **existing** `ToolCallResult.new_messages` seam (verified: the turn loop appends `new_messages` to history → the next provider request encodes the image block for Anthropic/OpenAI/Gemini). **No frozen-protocol change** — `ContentBlock::Image`/`ImageSource::Base64`/`ConversationMessage::user_with_images` already exist. (The additive `ContentBlock::Document` for PDF is P4.)

**Tech Stack:** Rust 1.82; `image` `0.25` (`default-features = false`, features `jpeg/png/gif/webp`, matching codex), `base64`. Verdict/output is in-crate tested — no coordinator fixtures.

**Reference of truth:** `claude-code/src/tools/FileReadTool/{FileReadTool.ts,imageProcessor.ts}` + `src/utils/imageResizer.ts` + `src/constants/apiLimits.ts`. Rust how-to: `codex/codex-rs/utils/image/src/lib.rs`. **Functional-parity bar** (the `image` crate lacks sharp's PNG-palette quantization + WebP quality knobs; see follow-ups).

---

## File Structure

- **Modify** `lingxi-code/tools/file/Cargo.toml` — add optional `image` + `base64` behind an `image-read` feature.
- **Create** `lingxi-code/tools/file/src/image_read.rs` — `is_image_extension` + `process_image` (detect/decode/resize/encode) + tests.
- **Modify** `lingxi-code/tools/file/src/lib.rs` — declare the feature-gated module.
- **Modify** `lingxi-code/tools/file/src/read.rs` — the image branch (skip the text size-gate + binary guard for images; emit `ContentBlock::Image` via `new_messages`).
- **Modify** `lingxi-code/apps/engine-desktop/Cargo.toml` — enable `image-read` on the `tool-file` dependency.

**Constants ported from claude-code** (`apiLimits.ts`): `IMAGE_TARGET_RAW_SIZE = 3_932_160` (3.75 MB), `IMAGE_MAX_WIDTH/HEIGHT = 2000`; JPEG quality ladder `[80,60,40,20]`; secondary shrink width `1000` @ quality `20` (`imageResizer.ts`).

---

## Task 1: add the `image-read` feature + deps (DE-RISK)

**Files:** Modify `lingxi-code/tools/file/Cargo.toml`

- [ ] **Step 1: Add the optional deps + feature**

In `[dependencies]`:

```toml
# Image decode/resize/re-encode for FileRead image reading (claude-code's image
# path). Optional + gated behind `image-read`; matches codex (no defaults, the
# four codecs only). base64 encodes the processed image for the Anthropic source.
image = { version = "0.25", default-features = false, features = ["jpeg", "png", "gif", "webp"], optional = true }
base64 = { version = "0.22", optional = true }
```

Add (or extend) `[features]`:

```toml
[features]
# FileRead image reading. OFF by default (engine-mobile/minimal stay lean and
# never compile the image codecs); engine-desktop enables it.
image-read = ["dep:image", "dep:base64"]
```

- [ ] **Step 2: Verify both feature states build on 1.82**

Run: `cargo check -p tool-file && cargo check -p tool-file --features image-read`
Expected: both succeed. **If `--features image-read` fails on MSRV** (image 0.25 needs a newer rustc than 1.82): try `image = "0.24"` (the resize/encode API in Task 2 is stable across 0.24/0.25; only `ImageEncoder::write_image`'s `ColorType`→`ExtendedColorType` `.into()` may differ — drop the `.into()` on 0.24). If neither builds on 1.82, report BLOCKED with the error.
Pin the resolved versions (`cargo tree -p tool-file --features image-read -i image` / `-i base64`) as `= ` exacts.

- [ ] **Step 3: Commit**

```bash
git add lingxi-code/tools/file/Cargo.toml lingxi-code/Cargo.lock
git commit -m "build(tool-file): add image-read feature (image + base64)"
```

---

## Task 2: `image_read.rs` — detect + resize/encode

**Files:** Create `lingxi-code/tools/file/src/image_read.rs`; modify `lingxi-code/tools/file/src/lib.rs`

- [ ] **Step 1: Declare the module**

In `tools/file/src/lib.rs`, add near the other module declarations:

```rust
#[cfg(feature = "image-read")]
pub mod image_read;
```

- [ ] **Step 2: Write the failing tests**

Create `image_read.rs` with the tests first:

```rust
//! FileRead image reading: detect → decode → resize/re-encode to fit
//! claude-code's size/dimension budget. Ports the resize ladder of
//! claude-code `src/utils/imageResizer.ts`; Rust how-to mirrors codex
//! `utils/image`. Functional-parity (the `image` crate lacks sharp's PNG-palette
//! and WebP-quality knobs — oversized PNG/WebP fall to the JPEG ladder).

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, RgbImage};

    fn png_bytes(w: u32, h: u32) -> Vec<u8> {
        let img = DynamicImage::ImageRgb8(RgbImage::new(w, h));
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
        buf.into_inner()
    }

    #[test]
    fn detects_image_extensions() {
        for ok in ["a.png", "a.jpg", "a.JPEG", "a.gif", "a.webp"] {
            assert!(is_image_extension(std::path::Path::new(ok)), "{ok}");
        }
        for no in ["a.txt", "a.rs", "a", "a.tar.gz"] {
            assert!(!is_image_extension(std::path::Path::new(no)), "{no}");
        }
    }

    #[test]
    fn small_image_passes_through_unchanged() {
        let bytes = png_bytes(10, 10);
        let p = process_image(bytes.clone()).unwrap();
        assert_eq!(p.media_type, "image/png");
        assert!(p.resized.is_none(), "small image not resized");
        // round-trips: the base64 decodes back to the original bytes.
        use base64::Engine;
        assert_eq!(base64::engine::general_purpose::STANDARD.decode(&p.base64).unwrap(), bytes);
    }

    #[test]
    fn oversized_dimensions_are_resized_to_cap() {
        let bytes = png_bytes(3000, 1500);
        let p = process_image(bytes).unwrap();
        let (ow, oh, dw, dh) = p.resized.expect("resized");
        assert_eq!((ow, oh), (3000, 1500));
        assert!(dw <= IMAGE_MAX_DIM && dh <= IMAGE_MAX_DIM, "fits cap: {dw}x{dh}");
        assert_eq!(dw, 2000, "long side clamped to 2000");
        assert_eq!(p.media_type, "image/jpeg", "resized images re-encode as jpeg");
    }

    #[test]
    fn empty_image_errors() {
        assert!(process_image(vec![]).is_err());
    }
}
```

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test -p tool-file --features image-read --lib image_read::`
Expected: FAIL — missing `is_image_extension`/`process_image`/`IMAGE_MAX_DIM`.

- [ ] **Step 4: Implement**

Add ABOVE the test module:

```rust
use base64::Engine;
use image::codecs::jpeg::JpegEncoder;
use image::{DynamicImage, GenericImageView, ImageFormat};

/// claude-code `IMAGE_TARGET_RAW_SIZE` (apiLimits.ts): API_IMAGE_MAX_BASE64_SIZE*3/4 = 3.75 MB.
const IMAGE_TARGET_RAW_SIZE: usize = 3_932_160;
/// claude-code `IMAGE_MAX_WIDTH`/`IMAGE_MAX_HEIGHT`.
pub const IMAGE_MAX_DIM: u32 = 2000;
/// claude-code secondary-shrink width @ quality 20 (last resort).
const SECONDARY_SHRINK_WIDTH: u32 = 1000;
/// claude-code JPEG quality ladder (imageResizer.ts).
const JPEG_QUALITY_LADDER: [u8; 4] = [80, 60, 40, 20];

/// Image extensions claude-code routes to the image path (FileReadTool.ts:188).
#[must_use]
pub fn is_image_extension(path: &std::path::Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("png" | "jpg" | "jpeg" | "gif" | "webp")
    )
}

/// A processed image ready for an `ImageSource::Base64`.
pub struct ProcessedImage {
    pub base64: String,
    /// `image/png` | `image/jpeg` | `image/gif` | `image/webp`.
    pub media_type: String,
    /// `(orig_w, orig_h, disp_w, disp_h)` — present only when the image was resized.
    pub resized: Option<(u32, u32, u32, u32)>,
}

fn format_to_media_type(fmt: Option<ImageFormat>) -> String {
    match fmt {
        Some(ImageFormat::Jpeg) => "image/jpeg",
        Some(ImageFormat::Gif) => "image/gif",
        Some(ImageFormat::WebP) => "image/webp",
        _ => "image/png",
    }
    .to_string()
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn encode_jpeg(img: &DynamicImage, quality: u8) -> Option<Vec<u8>> {
    let mut buf = Vec::new();
    JpegEncoder::new_with_quality(&mut buf, quality)
        .encode_image(img)
        .ok()?;
    Some(buf)
}

/// Decode, then resize/re-encode to fit the size + dimension budget. Ports the
/// core of claude-code `maybeResizeAndDownsampleImageBuffer`: empty-guard →
/// fast-path-unchanged → dimension-resize(2000) → JPEG quality ladder → 1000px
/// secondary shrink. (Functional parity: oversized PNG/WebP go to the JPEG
/// ladder rather than sharp's PNG-palette / WebP-quality paths.)
///
/// # Errors
/// Empty input, or the bytes can't be decoded as a supported image.
pub fn process_image(bytes: Vec<u8>) -> Result<ProcessedImage, String> {
    if bytes.is_empty() {
        return Err("Image file is empty (0 bytes)".to_string());
    }
    let fmt = image::guess_format(&bytes).ok();
    let media_type = format_to_media_type(fmt);
    let img = image::load_from_memory(&bytes).map_err(|e| format!("failed to decode image: {e}"))?;
    let (w, h) = img.dimensions();

    // Fast path: within the raw-size + dimension caps → original bytes unchanged.
    if bytes.len() <= IMAGE_TARGET_RAW_SIZE && w <= IMAGE_MAX_DIM && h <= IMAGE_MAX_DIM {
        return Ok(ProcessedImage { base64: b64(&bytes), media_type, resized: None });
    }

    // Dimension resize to fit 2000x2000 (aspect-preserving) when over the cap.
    let working = if w > IMAGE_MAX_DIM || h > IMAGE_MAX_DIM {
        img.resize(IMAGE_MAX_DIM, IMAGE_MAX_DIM, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    let (dw, dh) = working.dimensions();

    // JPEG quality ladder → first encoding that fits the raw-size budget.
    for &q in &JPEG_QUALITY_LADDER {
        if let Some(enc) = encode_jpeg(&working, q) {
            if enc.len() <= IMAGE_TARGET_RAW_SIZE {
                return Ok(ProcessedImage {
                    base64: b64(&enc),
                    media_type: "image/jpeg".to_string(),
                    resized: Some((w, h, dw, dh)),
                });
            }
        }
    }

    // Secondary shrink: 1000px wide @ quality 20 — unconditional last resort.
    let sw = working.width().min(SECONDARY_SHRINK_WIDTH);
    let small = working.resize(sw, u32::MAX, image::imageops::FilterType::Triangle);
    let (sdw, sdh) = small.dimensions();
    let enc = encode_jpeg(&small, 20).ok_or_else(|| "jpeg encode failed".to_string())?;
    Ok(ProcessedImage {
        base64: b64(&enc),
        media_type: "image/jpeg".to_string(),
        resized: Some((w, h, sdw, sdh)),
    })
}
```

- [ ] **Step 5: Run to verify it passes**

Run: `cargo test -p tool-file --features image-read --lib image_read::`
Expected: PASS (4 tests).

- [ ] **Step 6: Commit**

```bash
git add lingxi-code/tools/file/src/image_read.rs lingxi-code/tools/file/src/lib.rs
git commit -m "feat(tool-file): image_read module - detect + resize/encode"
```

---

## Task 3: the FileRead image branch

**Files:** Modify `lingxi-code/tools/file/src/read.rs`

- [ ] **Step 1: Compute `is_image` early + skip the text size-gate for images**

Find the size gate (`read.rs:596`):

```rust
        if input_limit.is_none() && size > MAX_FILE_READ_SIZE {
```

Just BEFORE it, compute the image flag (uses the canonical path's extension; `cfg!` makes it always-false when the feature is off, so the legacy path is byte-identical):

```rust
        // Image files route to the multimodal image path (claude-code routes by
        // extension to readImageWithTokenBudget, bypassing the text size cap).
        let is_image = cfg!(feature = "image-read") && crate::is_image_path(&canon);
```

(Add a tiny free helper near the top of `read.rs` — it must compile in both feature states; the actual extension set lives in `image_read` but that module only exists under the feature, so duplicate the cheap check here:)

```rust
/// True for the FileRead image extensions (png/jpg/jpeg/gif/webp). Defined here
/// (not via `image_read`) so it compiles when `image-read` is off.
fn is_image_path(path: &std::path::Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref(),
        Some("png" | "jpg" | "jpeg" | "gif" | "webp")
    )
}
```

Change the size gate to skip images:

```rust
        if !is_image && input_limit.is_none() && size > MAX_FILE_READ_SIZE {
```

- [ ] **Step 2: Add the image branch after `fs::read`, before the binary guard**

Find the binary guard (`read.rs:609`):

```rust
        let head = &bytes[..bytes.len().min(NUL_SCAN_WINDOW)];
        if looks_binary(head) {
```

Insert BEFORE it:

```rust
        #[cfg(feature = "image-read")]
        if is_image {
            return self.read_image_result(&invocation_id, &canon, bytes, size).await;
        }
```

- [ ] **Step 3: Add the `read_image_result` method**

Add to `impl FileReadTool` (the method is feature-gated):

```rust
    /// Process an image file and return it as multimodal content. The pixels
    /// ride on `new_messages` as a `ContentBlock::Image` (the frozen tool-result
    /// content is text-only); the tool-result text is a short placeholder.
    /// Mirrors claude-code FileRead's image path (the model-facing image block
    /// + an optional `[Image: original …]` metadata message when resized).
    #[cfg(feature = "image-read")]
    async fn read_image_result(
        &self,
        invocation_id: &str,
        canon: &std::path::Path,
        bytes: Vec<u8>,
        original_size: u64,
    ) -> Result<ToolCallResult, ToolError> {
        let processed = match crate::image_read::process_image(bytes) {
            Ok(p) => p,
            Err(e) => {
                self.emit_failed(invocation_id, "image_process").await;
                return Err(ToolError::Io(e));
            }
        };

        let source = protocol::ImageSource::Base64 {
            media_type: processed.media_type.clone(),
            data: processed.base64,
        };
        // Metadata text only when actually resized (claude-code createImageMetadataText).
        let text = match processed.resized {
            Some((ow, oh, dw, dh)) => {
                let scale = f64::from(ow) / f64::from(dw.max(1));
                format!(
                    "[Image: original {ow}x{oh}, displayed at {dw}x{dh}. Multiply coordinates by {scale:.2} to map to original image.]"
                )
            }
            None => String::new(),
        };
        let msg = protocol::ConversationMessage::user_with_images(
            protocol::MessageId::new(),
            text,
            vec![source],
        );

        self.emit_completed_image(invocation_id, original_size).await;

        Ok(ToolCallResult {
            data: serde_json::json!({
                "type": "image",
                "file_path": canon.display().to_string(),
                "media_type": processed.media_type,
                "original_size": original_size,
                "model_content": "[Image content provided in the following message.]",
            }),
            new_messages: vec![msg],
            context_modifier: None,
            mcp_meta: None,
        })
    }
```

Note: `emit_completed_image` is a thin telemetry helper — if `read.rs` already has an `emit_completed`/`emit_failed` pattern, reuse it (match its exact signature). If a dedicated image-completed event doesn't exist, call the existing generic completion emit (or `emit_failed`'s sibling) with the image fields; the goal is just to not skip telemetry. Confirm the exact emit method names by reading the existing `emit_started`/`emit_failed` in `read.rs` and mirror them.

- [ ] **Step 4: Test the image branch end-to-end**

Add an async test in `read.rs`'s test module (mirror the existing FileRead test setup — find how other `call()` tests construct the tool + a temp file):

```rust
    #[cfg(feature = "image-read")]
    #[tokio::test]
    async fn reads_image_as_multimodal_new_message() {
        // Write a small PNG to a temp file under a trusted dir, call() it, and
        // assert: result.data["type"] == "image", and new_messages carries one
        // user message whose content has a ContentBlock::Image { Base64 }.
        // (Construct the tool + trusted-dir + temp file exactly like the sibling
        // text-read tests in this module; write PNG bytes via the `image` crate.)
        // Assert the image source media_type is "image/png" and new_messages.len()==1.
    }
```

Run: `cargo test -p tool-file --features image-read --lib reads_image` (and the whole `read` suite both feature states).
Expected: the image test passes; **all existing tests pass in BOTH feature states** (feature-off = images still rejected by the binary guard, byte-identical).

- [ ] **Step 5: Commit**

```bash
git add lingxi-code/tools/file/src/read.rs
git commit -m "feat(tool-file): FileRead image branch - emit ContentBlock::Image via new_messages"
```

---

## Task 4: enable on desktop + gate

**Files:** Modify `lingxi-code/apps/engine-desktop/Cargo.toml`

- [ ] **Step 1: Enable `image-read` on the desktop `tool-file` dep**

`grep -n 'tool-file' lingxi-code/apps/engine-desktop/Cargo.toml`, then add the feature (preserve the existing path form):

```toml
tool-file = { path = "../../tools/file", features = ["image-read"] }
```

Do NOT change `engine-mobile`'s `tool-file` dep (mobile stays lean — no image codecs).

- [ ] **Step 2: Build both engines + confirm isolation**

Run: `cargo build -p engine-desktop && cargo build -p engine-mobile`
Expected: both build. `cargo tree -p engine-desktop -i image` shows the crate; `cargo tree -p engine-mobile 2>/dev/null | grep -c '^image '` prints `0` (mobile pulls no `image`).

- [ ] **Step 3: Full gate**

- `cargo test --workspace --no-run` → struct-trap clean.
- `cargo test -p tool-file && cargo test -p tool-file --features image-read` → all pass.
- `cargo clippy -p tool-file --all-targets --no-deps -- -D warnings && cargo clippy -p tool-file --all-targets --features image-read --no-deps -- -D warnings` → clean.
- Confirm the FileRead parity fixtures/tests (text reads, notebook, dedup, too-large) are unchanged with the feature OFF.

- [ ] **Step 4: Commit**

```bash
git add lingxi-code/apps/engine-desktop/Cargo.toml lingxi-code/Cargo.lock
git commit -m "feat(engine-desktop): enable tool-file image-read feature"
```

---

## Notes & follow-ups

- **Functional-parity simplifications** (image 0.25 lacks sharp's knobs): oversized PNG/WebP go straight to the JPEG quality ladder instead of sharp's PNG-palette-quantization / WebP-quality-preserve steps; the result still fits the budget and the model sees it. GIF is decode-only (re-encoded as JPEG when resized).
- **Token-budget aggressive path** (`readImageWithTokenBudget` + `compressImageBuffer`'s multi-strategy progressive scale + 400×400 fallback): not ported — the size ladder caps raw bytes at 3.75 MB (≈ 5 MB base64), within normal token budgets. A follow-up can add the `ceil(base64*0.125) > maxTokens` check + the 400×400 q20 fallback.
- **Tool-result placement:** claude-code puts the image block *inside* the `tool_result` content; lingxi's frozen `ContentBlock::ToolResult.content` is text-only, so the image rides on `new_messages` (a follow-up user message) instead — functionally equivalent (the model sees the image right after the tool result), and avoids a frozen-protocol change.
- **engine-mobile** could opt into `image-read` later if mobile FileRead should read images; left off here per the lean-mobile convention.
