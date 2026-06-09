# Design: close the dep-blocked parity gaps (image, PDF, WebFetch HTML→md, tree-sitter bash AST)

- **Date:** 2026-06-09
- **Status:** Approved (design); ready for implementation planning
- **Reference of truth:** `claude-code/` (leaked TS). `codex/` (OpenAI Codex CLI, Rust, MIT/Apache) is a *Rust how-to reference only*, not a parity source.

## 1. Context & goal

Four parity gaps were left open because of a **self-imposed "no new external dependencies"** rule:
WebFetch HTML→markdown, image reading, PDF reading, and the tree-sitter bash AST path. The
workspace builds against crates.io (no vendor/ dir, no air-gapped `.cargo` override), and
**claude-code itself uses these libraries** — so adding the parity-faithful Rust equivalents
*increases* 1:1 fidelity rather than violating it.

**Goal:** implement all four to **functional parity** with claude-code, each landing as its own
gated wave (the established batch workflow).

## 2. Decisions (locked)

| Decision | Choice |
|---|---|
| Dependency policy | Lift "no new deps" for the specific parity-faithful crates below. |
| Implementation approach | **A** — add the same underlying crates; port *logic* from claude-code TS; use codex Rust only as a how-to for driving the libraries. |
| Fidelity bar | **Functional parity.** Byte-parity only where a locked fixture pins it (bash). |
| Feature gating | Component deps are **optional/default-off in the owning tool crates**; `engine-desktop` enables them explicitly; `engine-mobile`/minimal must not pull them. |
| Multimodal tool results | Reuse the **existing** `ToolCallResult.new_messages` seam (already `Vec<ConversationMessage>`, already used by SkillTool): FileRead emits image/PDF as follow-up user-message content blocks. **No new tool-result content type**; `ContentBlock::ToolResult { content: String }` is untouched. |
| PDF inline path | Approved **additive** `ContentBlock::Document` variant + `DocumentSource::Base64` on the frozen `protocol` crate. Existing `ContentBlock` serialization is unaffected. |

## 3. New dependencies

| Crate | Version | Used by | Feature | Source/why |
|---|---|---|---|---|
| `tree-sitter` + `tree-sitter-bash` | match codex workspace | `permission` | `bash-ast` | claude-code's bash AST path; codex `shell-command` is the Rust how-to |
| `image` | `0.25.9` (matches codex) | `tools/file` | `image-read` | decode/resize/encode; codex `utils/image` reference |
| `htmd` (HTML→markdown) | exact version pinned in planning | `tools/web` | `web-markdown` | closest Rust analog to claude-code's `turndown`; must pass MSRV/license check |
| PDF render crate (e.g. `pdfium-render`) | exact candidate/version pinned in 4b planning | `tools/file` | `pdf-pages` | **phase 4b only**; page→image extraction; must pass MSRV/license/build checks |

All are normal crates.io deps (no vendoring). Each is pinned in `Cargo.lock`; net-new packages are expected and acceptable per the policy change above.
Implementation planning must pin exact versions (no unqualified "latest stable"), verify the workspace MSRV (`rust-version = 1.82`), and prefer
`default-features = false` where the crate allows it. Heavy deps stay optional so `engine-mobile` does not resolve them accidentally.

## 4. Components

Each component lands as its own wave. Component 0 is the prerequisite seam for Components 2 and 4.

### Component 0 — multimodal seam · frozen `protocol` + provider codecs
- **Frozen-protocol additions (additive only):** a `ContentBlock::Document { source: DocumentSource }` variant
  and a `DocumentSource::Base64 { media_type, data }` type, mirroring the existing `ContentBlock::Image` / `ImageSource`.
  No existing variant or field changes; `ContentBlock::ToolResult { content: String }` is left exactly as-is.
- **No new tool-result content type.** The existing `ToolCallResult.new_messages: Vec<ConversationMessage>`
  (`tool-api/src/tool_trait.rs:237`, already used by SkillTool) is the carrier: FileRead emits the image/PDF as a
  follow-up user `ConversationMessage` containing `ContentBlock::Image` / `ContentBlock::Document`. This matches
  claude-code's `newMessages` pattern (`FileReadTool.ts` `mapToolResultToToolResultBlockParam`) and reaches
  functional parity **without** inventing a `ToolResultContent`/`ToolResultBlock` layer or touching the tool-result wire shape.
- **Blast radius (real, from the new variant):** `ContentBlock::Document` forces a new arm in every exhaustive
  `ContentBlock` match — ~21 files: Anthropic / OpenAI / Gemini / Bedrock encoders, compaction (`grouping`,
  `microcompact`, `ptl_retry`, `strip_media`, `autocompact`), replay/export, `message_size`, client-adapter.
  Anthropic emits the native document/image block; OpenAI/Gemini/Bedrock map to their native media shape, or fall
  back to page-images (4b) / a text placeholder when the provider can't carry it.
- **Fixtures:** existing `ContentBlock` string/JSON fixtures stay byte-identical; add new media fixtures only.

### Component 1 — tree-sitter bash AST · `permission`
- **Truth:** `claude-code/src/utils/bash/treeSitterAnalysis.ts` (+ how it's consumed in `src/tools/BashTool/bashSecurity.ts`). **How-to:** codex `shell-command/{bash.rs,parse_command.rs,command_safety}`.
- Port `treeSitterAnalysis.ts` → `permission/src/bash_tree_sitter.rs`, producing a `TreeSitterAnalysis` (command names, flags, operators, separators).
- Wire as the **optional precision layer** in `bash_security.rs`, mirroring TS's `treeSitter?: TreeSitterAnalysis | null`: the already-ported legacy shell-quote regex path stays primary; the AST refines it (the comments in `bash_security.rs` already mark the tree-sitter short-circuits as "N/A here").
- Behind feature `bash-ast`; when the feature is off, behavior is exactly today's legacy path (byte-identical).
- **Parity impact:** changes verdicts on some commands → **locked bash parity fixtures may shift.** Coordinator re-blesses after verifying against TS. This is the only component needing locked-fixture coordination.

### Component 2 — Image reading in FileRead · `tools/file`
- **Truth:** `claude-code/src/tools/FileReadTool/FileReadTool.ts` (`readImageWithTokenBudget`, `mapToolResultToToolResultBlockParam`),
  `src/tools/FileReadTool/imageProcessor.ts`, and `src/utils/imageResizer.ts`. Constants: `IMAGE_MAX_WIDTH`,
  `IMAGE_MAX_HEIGHT`, `API_IMAGE_MAX_BASE64_SIZE`, `IMAGE_TARGET_RAW_SIZE`. **How-to:** codex `utils/image`. **Crate:** `image`.
- Detect image media-type (jpeg/png/gif/webp) from bytes, then port the claude-code resize/compression policy:
  - empty image errors early;
  - image within raw-size and dimension caps returns unchanged;
  - PNG tries palette/compression first to preserve transparency;
  - oversized images try JPEG qualities `80, 60, 40, 20`;
  - still-oversize images shrink again (1000px cap, then fallback 400px/quality 20);
  - final output is checked against base64/token budget, not only dimensions.
- Return FileRead data shape `type=image` for UI/hooks, and append the model-facing image via the existing
  `ToolCallResult.new_messages` as a follow-up user `ConversationMessage` carrying
  `ContentBlock::Image { source: ImageSource::Base64 { ... } }`. If dimensions are known, mirror claude-code's
  metadata `newMessages` text alongside it. The tool-result string content is unchanged.
- Behind feature `image-read`.
- Image reads bypass the text `MAX_FILE_READ_SIZE` cap, matching claude-code; their limits are image/token limits.

### Component 3 — WebFetch HTML→markdown + small-fast side query · `tools/web`
- **Truth:** `claude-code/src/tools/WebFetchTool/utils.ts`. Flow: fetch → `turndown` HTML→md → truncate to `MAX_MARKDOWN_LENGTH` → secondary small-fast model (Haiku in Anthropic parity mode) with the user's prompt → return model output. Constants: `CACHE_TTL_MS=15min`, `MAX_URL_LENGTH=2000`, `MAX_HTTP_CONTENT_LENGTH=10MB`, `MAX_MARKDOWN_LENGTH=100_000`. **Crate:** `htmd`. No codex code.
- In `web_fetch.rs`: fetch (exists) → HTML content-type detection → `htmd` HTML→md (raw text for non-HTML) → cache the produced markdown under the original URL → truncate → call a **side-query runner** → return the model output.
- Do **not** call `BuiltinToolContext.provider` / `AnthropicProvider` directly. Thread a generic side-query seam from the composition root, backed by the same provider routing used by the main orchestrator (`ModelRouter` / `ProviderApiAdapter`).
  The request is tagged with a **new** `QuerySource::WebFetchApply` variant (added to `sidequery/src/purposes.rs`, alongside `MemorySelector`/`Compaction`) so cost/usage is attributed by model and tokens.
- Small-model resolution is provider-aware: explicit WebFetch/small-fast override first; configured router alias/profile next; Anthropic Haiku default only when resolving through Anthropic; otherwise fall back to the current main-loop model.
- Behind feature `web-markdown`.
- **Functional parity only** — final output is model-generated (non-deterministic). htmd's markdown will not byte-match turndown; acceptable.

### Component 4 — PDF reading in FileRead · `tools/file` (phased)
- **Truth:** `claude-code/src/tools/FileReadTool/FileReadTool.ts` + `src/utils/pdf.ts` + `pdfUtils.ts`
  (`readPDF`/`getPDFPageCount`/`extractPDFPages`, `parsePDFPageRange`, `isPDFSupported`). Constants:
  `PDF_EXTRACT_SIZE_THRESHOLD=3MB`, `PDF_MAX_PAGES_PER_READ=20`, `PDF_AT_MENTION_INLINE_THRESHOLD=10`.
- Add the FileRead `pages` input and validate it before I/O:
  invalid format errors; open-ended ranges exceed the cap; any range over `PDF_MAX_PAGES_PER_READ` errors.
- Route exactly like claude-code:
  1. If `pages` is supplied, always use page extraction (4b), regardless of file size.
  2. Without `pages`, get page count; if `page_count > PDF_AT_MENTION_INLINE_THRESHOLD`, error and instruct the model/user to use `pages`.
  3. Compute `should_extract_pages = !pdf_document_supported(model/provider) || size > PDF_EXTRACT_SIZE_THRESHOLD`.
  4. If `should_extract_pages`, use page extraction (4b) or return the claude-code-style unsupported/extraction-required error until 4b lands.
  5. Otherwise read the whole PDF as base64 and inject a meta user `ContentBlock::Document`.
- **4a (light, no renderer, no new dependency):** implement the full-PDF document path plus validation/routing/error
  behavior. Tool result text is `PDF file read: <path> (<size>)`; the actual PDF bytes are carried in `new_messages` as
  `ContentBlock::Document`. 4a needs **only** the additive `ContentBlock::Document` variant (Component 0) + the
  page-range input — no decoder crate — so it ships unguarded by any dep-feature.
- **4b (heavy, deferred follow-on):** extract selected/all pages to images, resize via Component 2, inject meta user image blocks,
  and return `type=parts` tool data. Behind `pdf-pages`; depends on `image-read` (page images reuse Component 2).
- `pdf_document_supported` is provider-capability based. The Anthropic parity rule keeps the `claude-3-haiku` exclusion; non-Anthropic providers must either advertise PDF support or take the page-extraction fallback.

## 5. Cross-cutting

- **Protocol surfaces:** Component 0 is the only shared seam. It adds the additive `ContentBlock::Document` variant
  (+`DocumentSource`) to frozen `protocol` and a new arm in every exhaustive `ContentBlock` match (~21 files: provider
  encoders, compaction, strip-media, replay/export, client-adapter). It reuses the existing `ToolCallResult.new_messages`
  field — **no `tool-api` shape change, no new tool-result content type.** Existing `ContentBlock` serialization stays byte-identical.
- **Feature flags:** `bash-ast`, `image-read`, `web-markdown`, `pdf-pages`.
  Owning crates define these as default-off optional dependencies. (PDF 4a's document path adds no dependency, so it is
  unguarded code behind the always-present additive protocol variant — no feature.) `engine-desktop` enables all
  currently-shipping parity features; `engine-mobile`/minimal enables none unless a mobile-specific follow-up opts in.
- **Locked fixtures:** only Component 1 may re-bless existing bash verdict fixtures. Media/WebFetch/PDF add new fixtures;
  existing text/protocol fixtures must remain unchanged.
- **Sequencing:** ① multimodal seam → ② image-read → ③ bash-ast → ④ web-markdown side-query → ⑤ pdf-doc → ⑥ pdf-pages.
- **Per-component gate:** `cargo test --workspace --no-run` struct-trap + targeted `-p <crate> -p test-harness` tests +
  `clippy -D warnings`, plus feature builds (`--features <name>`).
  Add `cargo check -p engine-mobile --no-default-features` and `cargo tree -p engine-mobile` checks to prove optional deps are absent from mobile/minimal.

## 6. Testing

- **Component 1:** unit tests over representative commands (AST vs legacy agreement + the cases the AST is meant to fix); re-blessed bash parity fixtures.
- **Component 2:** decode/resize/encode round-trips against sample images per format; unchanged-small-image path; PNG transparency-preserving compression; quality fallback ladder; base64/token cap; tool-result media-block serialization.
- **Component 3:** HTML→md conversion unit tests (structural, not byte-exact); cache stores markdown before prompt application; truncation at `MAX_MARKDOWN_LENGTH`; side-query mocked at the generic runner seam; provider/model/cost attribution asserted.
- **Component 4:** page-range parsing and cap; `pages`-always-extract routing; page-count threshold error; document-supported vs extraction fallback; doc-block injection; page-image cap once 4b lands.

## 7. Risks & open items (resolve in planning)

1. **Protocol blast radius** (Component 0): the additive `ContentBlock::Document` variant forces a new arm in ~21 exhaustive `ContentBlock` matches (Anthropic/OpenAI/Gemini/Bedrock encoders, compaction, strip-media, replay). Audit each; preserve existing-variant JSON byte-for-byte; OpenAI/Gemini/Bedrock must map or fall back (page-images / text placeholder) for `Document`.
2. **htmd fidelity** (Component 3): validate htmd output is acceptable vs turndown on representative pages; fall back to `html2md` if needed.
3. **PDF render crate** (4b): pick a maintained, license-clean, buildable crate; this is why 4b is deferred.
4. **Provider media support** (Component 0/4): Anthropic document/image parity is required; OpenAI/Gemini must either map native media support or fail/fallback clearly.
5. **Small-fast model resolution** (Component 3): define the exact resolver order in implementation planning and test Anthropic, OpenAI, and Gemini model strings.
6. **Binary weight** (mobile): verify opt-out via features; confirm `engine-mobile` resolves none of the new optional deps.

## 8. Out of scope

- Byte-for-byte HTML→markdown (different lib than turndown; functional parity only).
- OCR/text extraction beyond claude-code's document block or page-image behavior.
- New mobile enablement for image/PDF/WebFetch parity features.
