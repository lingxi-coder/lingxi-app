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
| Feature gating | **Per-component cargo features**, default-on for `engine-desktop`, **off for `engine-mobile`/minimal**. |
| PDF inline path | **Approved** additive `ContentBlock::Document` variant on the frozen `protocol` crate (additive enum variant; existing serialization unchanged). |

## 3. New dependencies

| Crate | Version | Used by | Feature | Source/why |
|---|---|---|---|---|
| `tree-sitter` + `tree-sitter-bash` | match codex workspace | `permission` | `bash-ast` | claude-code's bash AST path; codex `shell-command` is the Rust how-to |
| `image` | `0.25.9` (matches codex) | `tools/file` | `image-read` | decode/resize/encode; codex `utils/image` reference |
| `htmd` (HTML→markdown) | latest stable | `tools/web` | `web-markdown` | closest Rust analog to claude-code's `turndown` |
| PDF render crate (e.g. `pdfium-render`) | TBD in planning | `tools/file` | `pdf-read` | **phase 4b only**; page→image extraction |

All are normal crates.io deps (no vendoring). Each pinned in `Cargo.lock`; net-new packages are expected and acceptable per the policy change above.

## 4. Components

Each component is independent and lands as its own wave.

### Component 1 — tree-sitter bash AST · `permission`
- **Truth:** `claude-code/src/utils/bash/treeSitterAnalysis.ts` (+ how it's consumed in `src/tools/BashTool/bashSecurity.ts`). **How-to:** codex `shell-command/{bash.rs,parse_command.rs,command_safety}`.
- Port `treeSitterAnalysis.ts` → `permission/src/bash_tree_sitter.rs`, producing a `TreeSitterAnalysis` (command names, flags, operators, separators).
- Wire as the **optional precision layer** in `bash_security.rs`, mirroring TS's `treeSitter?: TreeSitterAnalysis | null`: the already-ported legacy shell-quote regex path stays primary; the AST refines it (the comments in `bash_security.rs` already mark the tree-sitter short-circuits as "N/A here").
- Behind feature `bash-ast`; when the feature is off, behavior is exactly today's legacy path (byte-identical).
- **Parity impact:** changes verdicts on some commands → **locked bash parity fixtures may shift.** Coordinator re-blesses after verifying against TS. This is the only component needing locked-fixture coordination.

### Component 2 — Image reading in FileRead · `tools/file`
- **Truth:** `claude-code/src/tools/FileReadTool/imageProcessor.ts` + `src/utils/imageResizer.ts` (uses `sharp`: resize-to-fit + re-encode jpeg quality 80). Constants: `IMAGE_MAX_WIDTH`, `IMAGE_MAX_HEIGHT`, `API_IMAGE_MAX_BASE64_SIZE` (`src/constants/apiLimits.ts`). **How-to:** codex `utils/image`. **Crate:** `image`.
- Detect image media-type (jpeg/png/gif/webp) → decode → resize-to-fit the claude-code thresholds → re-encode → base64. Emit an existing **`ContentBlock::Image`** (`ImageSource::Base64` — *no frozen change*).
- Behind feature `image-read`.
- **Open item (resolve in planning):** confirm the tool-result→message channel carries image content blocks (FileRead currently returns text). If not, design the minimal additive seam.

### Component 3 — WebFetch HTML→markdown + Haiku · `tools/web`
- **Truth:** `claude-code/src/tools/WebFetchTool/utils.ts`. Flow: fetch → `turndown` HTML→md → truncate to `MAX_MARKDOWN_LENGTH` → secondary (Haiku) model with the user's prompt → return model output. Constants: `CACHE_TTL_MS=15min`, `MAX_URL_LENGTH=2000`, `MAX_HTTP_CONTENT_LENGTH=10MB`, `MAX_MARKDOWN_LENGTH=100_000`. **Crate:** `htmd`. No codex code.
- In `web_fetch.rs`: fetch (exists) → `htmd` HTML→md → truncate → call the **secondary/Haiku model via api-client** (resolve the small-model id via the existing model getter) → return. Reuse existing cache / `url_safety` / `blocklist`.
- Behind feature `web-markdown`.
- **Functional parity only** — final output is model-generated (non-deterministic). htmd's markdown will not byte-match turndown; acceptable.

### Component 4 — PDF reading in FileRead · `tools/file` (phased)
- **Truth:** `claude-code/src/utils/pdf.ts` + `pdfUtils.ts` (`readPDF`/`getPDFPageCount`/`extractPDFPages`, `parsePDFPageRange`, `isPDFSupported`). Constants: `PDF_EXTRACT_SIZE_THRESHOLD=3MB`, `PDF_MAX_PAGES_PER_READ=20`, `PDF_AT_MENTION_INLINE_THRESHOLD=10`.
- **4a (light, no decoder):** PDF below the extract threshold → base64 → **`ContentBlock::Document`** (the approved additive frozen variant; Anthropic document shape). Port the page-range parsing + size thresholds.
- **4b (heavy, deferred follow-on):** PDF at/above threshold → extract pages → **images** (reuses `ContentBlock::Image`, no frozen change) → PDF-render crate. `PDF_MAX_PAGES_PER_READ` cap.
- Behind feature `pdf-read`.

## 5. Cross-cutting

- **Frozen surfaces:** only 4a touches frozen `protocol` (one additive `ContentBlock::Document` variant — approved). Components 1/2/3/4b avoid all frozen changes. `traits/` untouched.
- **Feature flags:** `bash-ast`, `image-read`, `web-markdown`, `pdf-read`; `engine-desktop` enables all, `engine-mobile`/minimal enables none. Feature-off path is byte-identical to today.
- **Locked fixtures:** only Component 1 (bash) shifts locked fixtures; coordinator re-bless after TS verification. The others add new behavior gated behind a feature, so existing locked fixtures stay green.
- **Sequencing (independent; by value/cleanliness):** ① image-read → ② bash-ast → ③ web-markdown → ④a pdf-doc → ④b pdf-pages.
- **Per-component gate:** `cargo test --workspace --no-run` struct-trap + `-p <crate> -p test-harness` + `clippy -D warnings`, plus the new feature build (`--features <name>`).

## 6. Testing

- **Component 1:** unit tests over representative commands (AST vs legacy agreement + the cases the AST is meant to fix); re-blessed bash parity fixtures.
- **Component 2:** decode/resize/encode round-trips against sample images per format; oversize→resize; base64-size cap.
- **Component 3:** HTML→md conversion unit tests (structural, not byte-exact); truncation at `MAX_MARKDOWN_LENGTH`; the Haiku call mocked at the api-client seam.
- **Component 4:** page-range parsing; size-threshold routing (doc-block vs page-images); page-count cap.

## 7. Risks & open items (resolve in planning)

1. **Tool-result multimodal channel** (Component 2/4): confirm FileRead can return image/document content blocks; design the additive seam if absent.
2. **htmd fidelity** (Component 3): validate htmd output is acceptable vs turndown on representative pages; fall back to `html2md` if needed.
3. **PDF render crate** (4b): pick a maintained, license-clean, buildable crate; this is why 4b is deferred.
4. **Secondary-model resolution** (Component 3): confirm the small/Haiku model id getter exists and is reachable from `tools/web` (or thread it via the tool context).
5. **Binary weight** (mobile): verified opt-out via features; confirm `engine-mobile` enables none.

## 8. Out of scope

- Byte-for-byte HTML→markdown (different lib than turndown; functional parity only).
- PDF 4b page-image extraction is a deferred follow-on after 4a ships.
- Any change to `traits/`; any change to frozen `protocol` beyond the single approved `ContentBlock::Document` variant.
