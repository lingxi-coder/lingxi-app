# M7-02 syntect Highlighting + StructuredDiff Viewer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire `syntect` (syntax highlighting) and `similar` (line + word diff) into the `lingxi-tui` render module so that markdown code fences highlight by language and Edit/Write tool results render as a colored, claude-code-parity `StructuredDiff`.

**Architecture:** Two new pure-function modules under `crates/tui/src/render/` (created by the prerequisite M7-01): `syntax.rs` wraps a lazily-built `syntect::parsing::SyntaxSet` + `ThemeSet`, detects language from a markdown fence info-string (```` ```rust ````) **or** a file path/extension, maps the active `TuiTheme` to a bundled syntect `.tmTheme`, and returns `Vec<StyledLine>` (the M7-01 styled-line type). `diff.rs` uses `similar::TextDiff` for line-level diffing, pairs adjacent remove/add lines for word-level intra-line diffing (`similar::TextDiff::from_words`), and renders gutter (right-aligned line number) + sigil (`+`/`-`/` `) + per-line syntax-colored content with green/red line backgrounds — matching `claude-code/src/components/StructuredDiff/Fallback.tsx`. Both functions are stateless and snapshot-testable in isolation. M7-01's markdown fenced-code **placeholder spans** are then routed through `syntax.rs`, and M6's `UserToolResultMessage` gains an Edit/Write branch that renders the body as a `StructuredDiff`.

**Tech Stack:** Rust 2021, pinned toolchain 1.82.0 (`lingxi-code/rust-toolchain.toml`), `iocraft = "=0.8.3"` (`View`, not `Box`), `syntect = "=5.3.0"` (default-features off, `default-fancy` feature → pure-Rust `fancy-regex` backend, NO `onig` C dependency), `similar = "=2.7.0"` (already in the workspace lockfile; line + word diff), `insta` (existing dev-dep, YAML snapshots). The M7-01 `render::StyledLine` / `render::StyledSpan` types and the `render::markdown` placeholder-fence emitter are prerequisites.

**Prerequisite — M7-01 (must land first):** This plan fills placeholders left by M7-01. M7-01 creates `crates/tui/src/render/mod.rs`, `render/ansi.rs` (the expanded ANSI parser moved from `src/ansi.rs`), and `render/markdown.rs`, and defines the styled-line types. **The exact type names `StyledLine` and `StyledSpan` are assumed from the M7 design §2.3 (`-> Vec<StyledLine>`); the implementer MUST confirm the real names M7-01 shipped by reading `crates/tui/src/render/mod.rs` before Task 2, and substitute the actual names everywhere this plan writes `StyledLine` / `StyledSpan` if M7-01 chose different names.** M7-01 also leaves markdown code fences emitting a placeholder span variant (design §3 M7-01: "Code fences emit a placeholder span (filled by M7-02)") — Task 12 below replaces that.

**Locked types and naming (used consistently across all tasks below):**
- `render::StyledLine` — one rendered line: `pub struct StyledLine { pub spans: Vec<StyledSpan> }` (M7-01-defined; confirm).
- `render::StyledSpan` — `pub struct StyledSpan { pub text: String, pub fg: <color>, pub bg: <color>, pub bold: bool }` where `<color>` is whatever color type M7-01 settled on (likely `iocraft::Color` or an internal `RgbColor`). **Confirm the field names and color type from M7-01's `render/mod.rs` before Task 2.** This plan uses field names `text` / `fg` / `bg` / `bold`; rename to match M7-01 if they differ.
- `render::syntax::highlight(code: &str, lang: Option<&str>, theme: &TuiTheme) -> Vec<StyledLine>` — the public syntax entry point (design §2.3).
- `render::syntax::detect_language(info_string: Option<&str>, path: Option<&str>) -> Option<String>` — returns a syntect-resolvable language token or `None` (plain fallback).
- `render::diff::render(old: &str, new: &str, path: Option<&str>, theme: &TuiTheme) -> Vec<StyledLine>` — the public diff entry point (design §2.3 signature `render(old, new, theme)`, extended with `path` for syntax language detection per claude-code's `filePath` prop).
- `crate::theme::TuiTheme` — the M6 theme struct (`ASSISTANT`/`USER`/`ERROR`/`DIM` consts today; M7-15 expands it to a registry). M7-02 adds a `tm_theme_name(&self) -> &str` accessor mapping the active theme to a bundled syntect `.tmTheme` name (default `"base16-ocean.dark"`). Until M7-15 lands the registry, `TuiTheme` is a unit struct, so Task 6 adds the mapping as a free function `tm_theme_for(theme: &TuiTheme) -> &'static str` returning the single default; M7-15 generalizes it.

**Parity caveat (design §0 Q3 — make explicit in every test):** Syntax-highlight parity means **equivalent look, NOT byte-identical to highlight.js**. highlight.js token classification cannot be reproduced in Rust. Therefore **every snapshot test asserts STRUCTURE, not exact per-token colors**: line count, which spans are colored vs. plain (i.e. `span.fg != default`), `+`/`-`/` ` markers, gutter line numbers, hunk-header presence, word-diff span boundaries. Snapshots redact/normalize concrete color values (see Task 3 step 1 for the `normalize_colors` helper) so a syntect theme bump never breaks the suite. The literal-lock in design §2.8 explicitly exempts per-token syntax colors.

---

## File Structure

**New files (2):**
- `crates/tui/src/render/syntax.rs` — `syntect` wrapper: lazy `SyntaxSet`/`ThemeSet`, `detect_language`, `highlight`, theme→`.tmTheme` resolution, syntect-`Style`→`StyledSpan` conversion. ~220 lines incl. tests.
- `crates/tui/src/render/diff.rs` — `StructuredDiff`: `similar` line diff, adjacent-pair word diff, gutter/sigil/content layout, per-line syntax coloring, large-diff truncation. ~300 lines incl. tests.

**Modified files (4):**
- `crates/tui/Cargo.toml` — add `syntect = "=5.3.0"` (no-default-features, `default-fancy`) + `similar = "=2.7.0"` (Task 1).
- `crates/tui/src/render/mod.rs` (M7-01-created) — `pub mod syntax;` + `pub mod diff;` (Task 2).
- `crates/tui/src/render/markdown.rs` (M7-01-created) — replace the fenced-code placeholder span with a call to `render::syntax::highlight` (Task 12).
- `crates/tui/src/components/messages/user_tool_result.rs` — add an Edit/Write branch that detects the tool name and renders `old_string`/`new_string` (Edit) or empty→`content` (Write) as a `StructuredDiff` via `render::diff::render` (Task 13).

**Test files (3, plus inline `#[cfg(test)]` in each new module):**
- `crates/tui/tests/render_syntax.rs` — insta structure-snapshots: rust, python, js, json, unknown-lang fallback (plain), empty code block.
- `crates/tui/tests/render_diff.rs` — insta structure-snapshots: pure-add, pure-remove, modify (mixed), word-level intra-line, empty-diff, large-diff truncation.
- `crates/tui/tests/render_edit_write_diff.rs` — behavior: an Edit tool result and a Write tool result route through `StructuredDiff`.

---

## Task 1: Add `syntect` + `similar` deps and verify the MSRV 1.82 gate

> **This is the hard gate (design §4 R2). Do it FIRST. If the build fails and cannot be made to pass with the transitive pins below, STOP and follow the GATE FALLBACK note at the end of this task before writing any renderer code.**

**Files:**
- Modify: `crates/tui/Cargo.toml:22` (after the `iocraft = "=0.8.3"` line, in `[dependencies]`)
- Modify: `lingxi-code/Cargo.lock` (via `cargo update --precise`, committed)

- [ ] **Step 1: Add the two dependencies, exact-pinned, with the pure-Rust backend.**

  Edit `crates/tui/Cargo.toml`, in `[dependencies]` immediately after the `crossterm` line:

  ```toml
  # Syntax highlighting — pure-Rust fancy-regex backend (NO onig C dep). MSRV 1.82
  # verified in M7-02 Task 1 with the transitive pins below.
  syntect = { version = "=5.3.0", default-features = false, features = ["default-fancy"] }
  # Line- + word-level diff for StructuredDiff. Reuses the version already in
  # the workspace lockfile (2.7.0); 3.x requires Rust 1.85 > our MSRV 1.82.
  similar = "=2.7.0"
  ```

  Rationale (verified during planning): `default-onig` pulls the `onig` C dependency; `default-fancy` selects the pure-Rust `fancy-regex` backend, which is the design §2.2 requirement. `similar` 3.1.1 declares `rust-version = 1.85` (> MSRV 1.82); `2.7.0` declares 1.60 and is already resolved in the workspace lockfile.

- [ ] **Step 2: Apply the transitive-dependency pins that 1.82 requires.**

  A bare `cargo build` will pull transitive deps that need `edition2024` (unsupported by Cargo 1.82) and fail. The following pins were verified to compile on the 1.82.0 toolchain during planning. Run from inside `lingxi-code/`:

  ```bash
  cargo update -p indexmap --precise 2.7.1   # 2.14.0 needs edition2024 (already 2.7.1 in lockfile — no-op confirms)
  cargo update -p time --precise 0.3.36       # 0.3.47 (via plist) needs edition2024
  cargo update -p plist --precise 1.7.0       # 1.9.0 forces the new time
  ```

  Note: `bincode 1.3.3` and `fancy-regex 0.16.2` (syntect's pinned transitive) are already 1.82-compatible and need no pin. If `cargo build` surfaces a *different* edition2024 offender after these three, pin it down the same way (find the oldest version whose `rust-version` is ≤ 1.82 via `cargo info <crate>` and the crates.io version list).

- [ ] **Step 3: Run the MSRV gate build from inside `lingxi-code/`.**

  Run: `cargo build -p lingxi-tui`
  Expected: `Finished dev profile` with `syntect v5.3.0`, `similar v2.7.0`, `fancy-regex v0.16.2`, `plist v1.7.0`, `time v0.3.36` all compiling. No `edition2024` error, no `onig`/`onig_sys` in the build (confirm with `cargo tree -p lingxi-tui -i onig` printing "package ID specification `onig` did not match any packages").

  > **GATE FALLBACK (design §4 R2) — only if Step 3 cannot be made green:**
  > If syntect will not build on 1.82 with the full iocraft/crossterm tree even after pinning transitive deps, do NOT block the milestone. Degrade in this order, document the choice in a `// GATE FALLBACK:` comment at the top of `render/syntax.rs`, and continue with the rest of this plan unchanged (the `highlight` signature stays identical so `diff.rs`, markdown, and tests are unaffected):
  > 1. **`two-face` crate** (`two-face = "=0.4.x"`, MSRV-check it) — bundles extra syntaxes/themes over syntect and sometimes resolves differently. Try it as a drop-in for the `SyntaxSet`/`ThemeSet` load.
  > 2. **Minimal hand-rolled tokenizer** — a keyword/string/comment/number tokenizer for the top-5 languages only (rust, python, js, json, plus a plain fallback). Color keywords (a static per-language word list) blue, string literals green, line/block comments dim, numbers cyan, everything else default. This still returns `Vec<StyledLine>` so the rest of the plan is untouched; the snapshot tests (which assert structure, not exact colors) still pass. Document the reduced language set in the module header and in M7-16's literal-lock catalog.
  > Either fallback keeps `render::syntax::highlight` and `render::diff::render` signatures intact. Mark the deviation for the M7-16 release notes.

- [ ] **Step 4: Commit.**

  ```bash
  git add crates/tui/Cargo.toml Cargo.lock
  git commit -m "plan(M7-02 T1): add syntect 5.3.0 (fancy-regex) + similar 2.7.0, MSRV 1.82 gate green

  Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
  ```

---

## Task 2: Register the syntax + diff modules and confirm M7-01 types

**Files:**
- Read: `crates/tui/src/render/mod.rs` (M7-01-created — confirm `StyledLine`/`StyledSpan` names + color type)
- Modify: `crates/tui/src/render/mod.rs`
- Create: `crates/tui/src/render/syntax.rs` (stub)
- Create: `crates/tui/src/render/diff.rs` (stub)

- [ ] **Step 1: Confirm the M7-01 styled-line types.**

  Open `crates/tui/src/render/mod.rs`. Record the exact name and field layout of the per-line type (this plan calls it `StyledLine`) and the per-span type (`StyledSpan`), and the color type used (`iocraft::Color` vs an internal RGB type). If the names differ from this plan, do a find-and-replace of `StyledLine`/`StyledSpan`/field names across the remaining tasks as you go. Do NOT invent a new type — reuse M7-01's.

- [ ] **Step 2: Create the two module stubs.**

  `crates/tui/src/render/syntax.rs`:
  ```rust
  //! syntect syntax highlighting wrapper (M7-02).
  //!
  //! Parity (design §0 Q3): equivalent-look highlighting, NOT byte-identical to
  //! highlight.js. Tests assert which spans are colored, not exact colors.
  use crate::render::StyledLine;
  use crate::theme::TuiTheme;

  /// Highlight `code` for `lang` (a fence info-string token or detected
  /// language), themed by `theme`. Unknown/None lang → one plain StyledLine
  /// per input line. Never panics.
  #[must_use]
  pub fn highlight(_code: &str, _lang: Option<&str>, _theme: &TuiTheme) -> Vec<StyledLine> {
      Vec::new() // implemented in Task 4
  }
  ```

  `crates/tui/src/render/diff.rs`:
  ```rust
  //! StructuredDiff viewer (M7-02) — similar line+word diff, syntect-colored.
  //!
  //! Layout parity: claude-code/src/components/StructuredDiff/Fallback.tsx.
  use crate::render::StyledLine;
  use crate::theme::TuiTheme;

  /// Render a structured diff of `old` → `new`. `path` drives syntax language
  /// detection (claude-code's `filePath` prop). Never panics.
  #[must_use]
  pub fn render(_old: &str, _new: &str, _path: Option<&str>, _theme: &TuiTheme) -> Vec<StyledLine> {
      Vec::new() // implemented in Tasks 7-11
  }
  ```

- [ ] **Step 3: Register the modules.**

  In `crates/tui/src/render/mod.rs`, add after the existing module declarations:
  ```rust
  pub mod diff;
  pub mod syntax;
  ```

- [ ] **Step 4: Build to confirm the stubs compile.**

  Run (from inside `lingxi-code/`): `cargo build -p lingxi-tui`
  Expected: `Finished` with no errors (two dead-code-allowed stubs).

- [ ] **Step 5: Commit.**

  ```bash
  git add crates/tui/src/render/
  git commit -m "plan(M7-02 T2): register render::syntax + render::diff stubs

  Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
  ```

---

## Task 3: Language detection from fence info-string and file path

**Files:**
- Modify: `crates/tui/src/render/syntax.rs`
- Test: inline `#[cfg(test)] mod tests` in `syntax.rs`

- [ ] **Step 1: Write the failing test for `detect_language`.**

  Add to `syntax.rs`:
  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;

      #[test]
      fn detects_from_fence_info_string() {
          assert_eq!(detect_language(Some("rust"), None).as_deref(), Some("rust"));
          // info-string may carry extra metadata: "```rust,ignore" -> first token
          assert_eq!(detect_language(Some("rust,ignore"), None).as_deref(), Some("rust"));
          assert_eq!(detect_language(Some("python"), None).as_deref(), Some("python"));
      }

      #[test]
      fn detects_from_path_extension() {
          assert_eq!(detect_language(None, Some("src/main.rs")).as_deref(), Some("rs"));
          assert_eq!(detect_language(None, Some("a/b/app.py")).as_deref(), Some("py"));
          assert_eq!(detect_language(None, Some("data.json")).as_deref(), Some("json"));
      }

      #[test]
      fn fence_info_string_wins_over_path() {
          assert_eq!(detect_language(Some("js"), Some("file.py")).as_deref(), Some("js"));
      }

      #[test]
      fn no_lang_returns_none() {
          assert_eq!(detect_language(None, None), None);
          assert_eq!(detect_language(Some(""), None), None);
          assert_eq!(detect_language(None, Some("Makefile")), None); // no extension
      }
  }
  ```

- [ ] **Step 2: Run the test to verify it fails.**

  Run: `cargo test -p lingxi-tui detect_language`
  Expected: FAIL — `detect_language` not found.

- [ ] **Step 3: Implement `detect_language`.**

  Add to `syntax.rs` (above the tests):
  ```rust
  /// Resolve a language token from a fence info-string (preferred) or a file
  /// path extension. Returns a token suitable for syntect's
  /// `find_syntax_by_token` / `find_syntax_by_extension`. `None` → plain.
  #[must_use]
  pub fn detect_language(info_string: Option<&str>, path: Option<&str>) -> Option<String> {
      // Fence info-string wins. CommonMark info-strings may carry metadata
      // after the language (e.g. "rust,ignore" or "ts {1,3}"); take the first
      // whitespace/comma-delimited token.
      if let Some(info) = info_string {
          let token = info
              .split(|c: char| c.is_whitespace() || c == ',')
              .next()
              .unwrap_or("")
              .trim();
          if !token.is_empty() {
              return Some(token.to_string());
          }
      }
      // Fall back to the file extension.
      if let Some(p) = path {
          if let Some(ext) = std::path::Path::new(p).extension().and_then(|e| e.to_str()) {
              if !ext.is_empty() {
                  return Some(ext.to_string());
              }
          }
      }
      None
  }
  ```

- [ ] **Step 4: Run the test to verify it passes.**

  Run: `cargo test -p lingxi-tui detect_language`
  Expected: PASS (4 tests).

- [ ] **Step 5: Commit.**

  ```bash
  git add crates/tui/src/render/syntax.rs
  git commit -m "plan(M7-02 T3): language detection from fence info-string + file extension

  Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
  ```

---

## Task 4: syntect highlight → `Vec<StyledLine>` with plain fallback

**Files:**
- Modify: `crates/tui/src/render/syntax.rs`
- Test: inline `#[cfg(test)] mod tests`

- [ ] **Step 1: Write the failing tests for `highlight`.**

  Add to the `tests` module in `syntax.rs`:
  ```rust
  #[test]
  fn highlight_rust_keeps_line_count_and_colors_some_spans() {
      let theme = TuiTheme;
      let code = "fn main() {\n    let x = 1;\n}\n";
      let lines = highlight(code, Some("rust"), &theme);
      // STRUCTURE assertion (parity = equivalent look, not exact colors):
      assert_eq!(lines.len(), 3, "one StyledLine per source line");
      // At least one span on the keyword line is non-default colored.
      let any_colored = lines.iter().flat_map(|l| &l.spans).any(|s| is_colored(s));
      assert!(any_colored, "rust highlight produced at least one colored span");
  }

  #[test]
  fn highlight_unknown_lang_is_plain_one_span_per_line() {
      let theme = TuiTheme;
      let lines = highlight("alpha\nbeta\n", Some("not-a-language"), &theme);
      assert_eq!(lines.len(), 2);
      for l in &lines {
          assert_eq!(l.spans.len(), 1, "plain fallback = single span per line");
          assert!(!is_colored(&l.spans[0]), "plain fallback span is uncolored");
      }
  }

  #[test]
  fn highlight_none_lang_is_plain() {
      let theme = TuiTheme;
      let lines = highlight("just text\n", None, &theme);
      assert_eq!(lines.len(), 1);
      assert!(!is_colored(&lines[0].spans[0]));
  }

  #[test]
  fn highlight_empty_is_empty() {
      let theme = TuiTheme;
      assert!(highlight("", Some("rust"), &theme).is_empty());
  }

  // Helper: a span is "colored" if its fg differs from the plain default.
  // Substitute the real default for M7-01's color type.
  fn is_colored(s: &StyledSpan) -> bool {
      s.fg != <DEFAULT_FG>
  }
  ```

  > **Implementer note:** replace `StyledSpan`, the `.fg` field, and `<DEFAULT_FG>` with the real M7-01 names/value confirmed in Task 2 Step 1. If M7-01's plain color is `iocraft::Color::Reset`, then `<DEFAULT_FG>` is `iocraft::Color::Reset`.

- [ ] **Step 2: Run the tests to verify they fail.**

  Run: `cargo test -p lingxi-tui highlight_`
  Expected: FAIL — `highlight` returns empty.

- [ ] **Step 3: Implement `highlight` with lazy syntect assets + plain fallback.**

  Replace the stub in `syntax.rs`:
  ```rust
  use std::sync::OnceLock;
  use syntect::easy::HighlightLines;
  use syntect::highlighting::{Style as SynStyle, ThemeSet};
  use syntect::parsing::SyntaxSet;
  use syntect::util::LinesWithEndings;
  use crate::render::StyledSpan;

  fn syntax_set() -> &'static SyntaxSet {
      static SS: OnceLock<SyntaxSet> = OnceLock::new();
      SS.get_or_init(SyntaxSet::load_defaults_newlines)
  }

  fn theme_set() -> &'static ThemeSet {
      static TS: OnceLock<ThemeSet> = OnceLock::new();
      TS.get_or_init(ThemeSet::load_defaults)
  }

  /// One plain (uncolored) StyledLine per input line — the fallback when the
  /// language is unknown or absent.
  fn plain_lines(code: &str) -> Vec<StyledLine> {
      code.lines()
          .map(|line| StyledLine {
              spans: vec![StyledSpan {
                  text: line.to_string(),
                  fg: <DEFAULT_FG>,
                  bg: <DEFAULT_BG>,
                  bold: false,
              }],
          })
          .collect()
  }

  #[must_use]
  pub fn highlight(code: &str, lang: Option<&str>, theme: &TuiTheme) -> Vec<StyledLine> {
      if code.is_empty() {
          return Vec::new();
      }
      let ss = syntax_set();
      // Resolve the syntax: by token first (info-string), then by extension.
      let syntax = lang.and_then(|l| {
          ss.find_syntax_by_token(l)
              .or_else(|| ss.find_syntax_by_extension(l))
      });
      let Some(syntax) = syntax else {
          return plain_lines(code);
      };
      let tm = &theme_set().themes[tm_theme_for(theme)];
      let mut hl = HighlightLines::new(syntax, tm);
      let mut out = Vec::new();
      for line in LinesWithEndings::from(code) {
          // highlight_line never panics on the bundled syntaxes; on the rare
          // regex error, degrade that line to plain rather than unwrap-panic.
          let ranges = hl.highlight_line(line, ss).unwrap_or_default();
          let spans = if ranges.is_empty() {
              vec![StyledSpan {
                  text: line.trim_end_matches('\n').to_string(),
                  fg: <DEFAULT_FG>,
                  bg: <DEFAULT_BG>,
                  bold: false,
              }]
          } else {
              ranges
                  .into_iter()
                  .map(|(st, text)| syn_span_to_styled(st, text))
                  .collect()
          };
          out.push(StyledLine { spans });
      }
      out
  }

  /// Convert a syntect (Style, &str) range into a StyledSpan. Strips the
  /// trailing newline so StyledLine spans never carry "\n".
  fn syn_span_to_styled(st: SynStyle, text: &str) -> StyledSpan {
      use syntect::highlighting::FontStyle;
      StyledSpan {
          text: text.trim_end_matches('\n').to_string(),
          fg: syn_color_to_color(st.foreground),
          bg: <DEFAULT_BG>, // code blocks keep terminal bg; only fg is themed
          bold: st.font_style.contains(FontStyle::BOLD),
      }
  }
  ```

  Add `syn_color_to_color` mapping `syntect::highlighting::Color { r, g, b, a }` to M7-01's color type. If M7-01's color type is `iocraft::Color`, use `iocraft::Color::Rgb { r, g, b }` (drop alpha). If it is an internal RGB struct, construct that. **Use the real conversion for the M7-01 color type confirmed in Task 2.**

- [ ] **Step 4: Run the tests to verify they pass.**

  Run: `cargo test -p lingxi-tui highlight_`
  Expected: PASS (4 tests).

- [ ] **Step 5: Commit.**

  ```bash
  git add crates/tui/src/render/syntax.rs
  git commit -m "plan(M7-02 T4): syntect highlight to Vec<StyledLine> with plain fallback

  Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
  ```

---

## Task 5: Syntax structure-snapshot suite (rust/python/js/json/unknown/empty)

**Files:**
- Create: `crates/tui/tests/render_syntax.rs`
- Snapshots: `crates/tui/tests/snapshots/` (insta auto-creates)

- [ ] **Step 1: Write the structure-snapshot tests.**

  Create `crates/tui/tests/render_syntax.rs`:
  ```rust
  //! Structure snapshots for render::syntax (M7-02).
  //! PARITY (design §0 Q3): we snapshot STRUCTURE — line count, and per span
  //! whether it is "colored" (fg != default) or "plain" — NOT exact colors.
  //! Concrete colors are normalized so a syntect theme bump never breaks us.
  use lingxi_tui::render::syntax::highlight;
  use lingxi_tui::render::StyledLine;
  use lingxi_tui::theme::TuiTheme;

  /// Render each line as "C"/"P" per span (Colored / Plain) + the text, so the
  /// snapshot captures structure without baking in concrete ANSI colors.
  fn structure(lines: &[StyledLine]) -> String {
      lines
          .iter()
          .map(|l| {
              l.spans
                  .iter()
                  .map(|s| {
                      let flag = if s.fg != /* DEFAULT_FG */ Default::default() { 'C' } else { 'P' };
                      format!("[{flag}]{}", s.text)
                  })
                  .collect::<String>()
          })
          .collect::<Vec<_>>()
          .join("\n")
  }

  #[test]
  fn snapshot_rust() {
      let s = structure(&highlight("fn main() {\n    let x = 1;\n}\n", Some("rust"), &TuiTheme));
      insta::assert_snapshot!(s);
  }

  #[test]
  fn snapshot_python() {
      let s = structure(&highlight("def f(x):\n    return x + 1\n", Some("python"), &TuiTheme));
      insta::assert_snapshot!(s);
  }

  #[test]
  fn snapshot_js() {
      let s = structure(&highlight("const a = () => 42;\n", Some("js"), &TuiTheme));
      insta::assert_snapshot!(s);
  }

  #[test]
  fn snapshot_json() {
      let s = structure(&highlight("{\n  \"k\": 1\n}\n", Some("json"), &TuiTheme));
      insta::assert_snapshot!(s);
  }

  #[test]
  fn snapshot_unknown_lang_is_all_plain() {
      let s = structure(&highlight("alpha\nbeta\n", Some("klingon"), &TuiTheme));
      // Every span flagged [P]; assert structurally AND snapshot.
      assert!(!s.contains("[C]"), "unknown lang must be all-plain");
      insta::assert_snapshot!(s);
  }

  #[test]
  fn snapshot_empty_code_block() {
      let s = structure(&highlight("", Some("rust"), &TuiTheme));
      assert!(s.is_empty());
      insta::assert_snapshot!(s);
  }
  ```

  > **Implementer note:** substitute the real default-fg sentinel for `Default::default()` in the `flag` computation (the M7-01 color type confirmed in Task 2). If `lingxi_tui::render::StyledLine`/`StyledSpan` are not `pub`-exported at the crate root, export them from `render/mod.rs` (`pub use`) or reference via `lingxi_tui::render::...` per their actual path.

- [ ] **Step 2: Run the tests and accept the snapshots.**

  Run: `cargo test -p lingxi-tui --test render_syntax`
  Expected: 6 tests create `.snap.new` files. Review them — confirm rust/python/js/json show a mix of `[C]` and `[P]` spans (some colored), unknown/empty are all-plain/empty. Then accept:
  ```bash
  cargo insta accept
  ```
  Re-run `cargo test -p lingxi-tui --test render_syntax` → PASS (6).

- [ ] **Step 3: Commit.**

  ```bash
  git add crates/tui/tests/render_syntax.rs crates/tui/tests/snapshots/
  git commit -m "plan(M7-02 T5): syntax structure-snapshots (rust/py/js/json/unknown/empty)

  Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
  ```

---

## Task 6: Theme → syntect `.tmTheme` mapping

**Files:**
- Read: `crates/tui/src/theme.rs` (current M6 `TuiTheme` unit struct)
- Modify: `crates/tui/src/render/syntax.rs`
- Test: inline `#[cfg(test)] mod tests`

- [ ] **Step 1: Write the failing test for `tm_theme_for`.**

  Add to the `tests` module in `syntax.rs`:
  ```rust
  #[test]
  fn tm_theme_for_returns_a_bundled_theme_name() {
      let name = tm_theme_for(&TuiTheme);
      // The default syntect ThemeSet must contain whatever name we map to,
      // or HighlightLines::new would panic on the index in Task 4.
      assert!(theme_set().themes.contains_key(name), "{name} is a bundled theme");
  }
  ```

- [ ] **Step 2: Run the test to verify it fails.**

  Run: `cargo test -p lingxi-tui tm_theme_for`
  Expected: FAIL — `tm_theme_for` not found.

- [ ] **Step 3: Implement `tm_theme_for`.**

  Add to `syntax.rs`:
  ```rust
  /// Map the active TUI theme to a bundled syntect `.tmTheme` name. Today
  /// `TuiTheme` is the M6 unit struct (one fixed palette), so this returns the
  /// single dark default. M7-15 (theme picker) expands `TuiTheme` into a
  /// registry and generalizes this to a per-theme lookup (light themes map to
  /// "InspiredGitHub", etc.). The returned name MUST exist in
  /// `ThemeSet::load_defaults().themes` (default set ships base16-ocean.dark,
  /// base16-ocean.light, base16-eighties.dark, base16-mocha.dark,
  /// InspiredGitHub, Solarized (dark), Solarized (light)).
  #[must_use]
  pub fn tm_theme_for(_theme: &TuiTheme) -> &'static str {
      "base16-ocean.dark"
  }
  ```

- [ ] **Step 4: Run the test to verify it passes.**

  Run: `cargo test -p lingxi-tui tm_theme_for`
  Expected: PASS.

- [ ] **Step 5: Commit.**

  ```bash
  git add crates/tui/src/render/syntax.rs
  git commit -m "plan(M7-02 T6): map active theme to bundled syntect .tmTheme (default base16-ocean.dark)

  Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
  ```

---

## Task 7: Diff line classification (similar line diff → add/remove/context)

**Files:**
- Read: `claude-code/src/components/StructuredDiff/Fallback.tsx` (lines ~125-225: `transformLinesToObjects`, `processAdjacentLines`; ~349-410: `formatDiff`, gutter/sigil layout)
- Modify: `crates/tui/src/render/diff.rs`
- Test: inline `#[cfg(test)] mod tests`

- [ ] **Step 1: Read the claude-code layout reference and lock the structure.**

  From `Fallback.tsx`, lock these structural facts into a comment block at the top of `diff.rs`:
  - Each diff line is `add` / `remove` / `nochange` (context).
  - Gutter = right-aligned line number (`padStart(maxWidth)`) + one space; then the sigil column: `+` for add, `-` for remove, ` ` (space) for context, then one space; then content.
  - `maxWidth` = width of the largest line number (`maxLineNumber.toString().length`).
  - Added lines get a green line background (`diffAdded`), removed lines red (`diffRemoved`); content text keeps its syntax colors layered over the line bg.
  - Adjacent remove→add runs are paired for word-level diffing (Task 9); `CHANGE_THRESHOLD = 0.4` decides word-diff vs. plain full-line when lines are too dissimilar.

- [ ] **Step 2: Write the failing test for line classification.**

  Add to `diff.rs`:
  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;

      #[test]
      fn classify_pure_add() {
          let rows = diff_rows("a\n", "a\nb\n");
          // a = context, b = add
          assert_eq!(rows.iter().map(|r| r.kind).collect::<Vec<_>>(),
                     vec![LineKind::Context, LineKind::Add]);
      }

      #[test]
      fn classify_pure_remove() {
          let rows = diff_rows("a\nb\n", "a\n");
          assert_eq!(rows.iter().map(|r| r.kind).collect::<Vec<_>>(),
                     vec![LineKind::Context, LineKind::Remove]);
      }

      #[test]
      fn classify_modify_is_remove_then_add() {
          let rows = diff_rows("foo\n", "bar\n");
          assert_eq!(rows.iter().map(|r| r.kind).collect::<Vec<_>>(),
                     vec![LineKind::Remove, LineKind::Add]);
      }

      #[test]
      fn classify_empty_diff_is_all_context() {
          let rows = diff_rows("a\nb\n", "a\nb\n");
          assert!(rows.iter().all(|r| r.kind == LineKind::Context));
          assert_eq!(rows.len(), 2);
      }
  }
  ```

- [ ] **Step 3: Run the test to verify it fails.**

  Run: `cargo test -p lingxi-tui --lib diff::tests::classify`
  Expected: FAIL — `diff_rows`, `LineKind`, `DiffRow` not found.

- [ ] **Step 4: Implement line classification with `similar`.**

  Add to `diff.rs`:
  ```rust
  use similar::{ChangeTag, TextDiff};

  /// Classification of one rendered diff line.
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum LineKind {
      Add,
      Remove,
      Context,
  }

  /// One row in the structured diff before layout.
  #[derive(Debug, Clone)]
  pub struct DiffRow {
      pub kind: LineKind,
      pub text: String,
      /// New-file line number for Add/Context; old-file line number for Remove.
      pub line_no: usize,
  }

  /// Run a line-level diff and classify each change. Line numbers follow
  /// claude-code: removed lines number against the old file, added/context
  /// against the new file.
  #[must_use]
  pub fn diff_rows(old: &str, new: &str) -> Vec<DiffRow> {
      let diff = TextDiff::from_lines(old, new);
      let mut rows = Vec::new();
      let mut old_no = 1usize;
      let mut new_no = 1usize;
      for change in diff.iter_all_changes() {
          let text = change.value().trim_end_matches('\n').to_string();
          match change.tag() {
              ChangeTag::Delete => {
                  rows.push(DiffRow { kind: LineKind::Remove, text, line_no: old_no });
                  old_no += 1;
              }
              ChangeTag::Insert => {
                  rows.push(DiffRow { kind: LineKind::Add, text, line_no: new_no });
                  new_no += 1;
              }
              ChangeTag::Equal => {
                  rows.push(DiffRow { kind: LineKind::Context, text, line_no: new_no });
                  old_no += 1;
                  new_no += 1;
              }
          }
      }
      rows
  }
  ```

- [ ] **Step 5: Run the test to verify it passes.**

  Run: `cargo test -p lingxi-tui --lib diff::tests::classify`
  Expected: PASS (4 tests).

- [ ] **Step 6: Commit.**

  ```bash
  git add crates/tui/src/render/diff.rs
  git commit -m "plan(M7-02 T7): diff line classification via similar (add/remove/context)

  Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
  ```

---

## Task 8: Diff layout — gutter + sigil + green/red lines + syntax-colored content

**Files:**
- Modify: `crates/tui/src/render/diff.rs`
- Test: inline `#[cfg(test)] mod tests`

- [ ] **Step 1: Write the failing test for `render` layout structure.**

  Add to the `tests` module in `diff.rs`:
  ```rust
  use crate::theme::TuiTheme;

  // Render a row to "<sigil><gutter-trimmed> <content>" for structural assertion.
  fn rowline(l: &StyledLine) -> String {
      l.spans.iter().map(|s| s.text.clone()).collect()
  }

  #[test]
  fn render_pure_add_has_plus_sigil_and_green_bg() {
      let lines = render("a\n", "a\nb\n", Some("x.txt"), &TuiTheme);
      assert_eq!(lines.len(), 2);
      // The add line's rendered text contains "+" sigil and the content "b".
      let add = &lines[1];
      let joined = rowline(add);
      assert!(joined.contains('+'), "add line carries + sigil: {joined:?}");
      assert!(joined.contains('b'));
      // At least one span on the add line has the green add background.
      assert!(add.spans.iter().any(|s| s.bg == add_bg(&TuiTheme)),
              "add line has green background");
  }

  #[test]
  fn render_pure_remove_has_minus_sigil_and_red_bg() {
      let lines = render("a\nb\n", "a\n", Some("x.txt"), &TuiTheme);
      let rem = lines.iter().find(|l| rowline(l).contains('-')).expect("a - line");
      assert!(rowline(rem).contains('b'));
      assert!(rem.spans.iter().any(|s| s.bg == remove_bg(&TuiTheme)),
              "remove line has red background");
  }

  #[test]
  fn render_gutter_has_line_numbers() {
      let lines = render("a\n", "a\nb\n", Some("x.txt"), &TuiTheme);
      // Context line "a" is line 1, add line "b" is line 2.
      assert!(rowline(&lines[0]).contains('1'));
      assert!(rowline(&lines[1]).contains('2'));
  }

  #[test]
  fn render_empty_diff_all_context_no_sigils() {
      let lines = render("a\nb\n", "a\nb\n", Some("x.txt"), &TuiTheme);
      assert_eq!(lines.len(), 2);
      for l in &lines {
          let j = rowline(l);
          assert!(!j.contains('+') && !j.contains('-'),
                  "context lines carry neither + nor - sigil: {j:?}");
      }
  }
  ```

- [ ] **Step 2: Run the test to verify it fails.**

  Run: `cargo test -p lingxi-tui --lib diff::tests::render_`
  Expected: FAIL — `render` returns empty; `add_bg`/`remove_bg` not found.

- [ ] **Step 3: Implement the layout + per-line syntax coloring.**

  Replace the `render` stub in `diff.rs`:
  ```rust
  use crate::render::{StyledLine, StyledSpan};
  use crate::render::syntax;

  /// Green background for added lines (claude-code `diffAdded`). Maps to
  /// M7-01's color type. M7-15 will source these from the active Theme.
  #[must_use]
  pub fn add_bg(_theme: &TuiTheme) -> <COLOR> { /* dark green */ <ADD_BG> }
  /// Red background for removed lines (claude-code `diffRemoved`).
  #[must_use]
  pub fn remove_bg(_theme: &TuiTheme) -> <COLOR> { /* dark red */ <REMOVE_BG> }

  /// Sigil character for a line kind: '+' / '-' / ' '.
  fn sigil(kind: LineKind) -> char {
      match kind {
          LineKind::Add => '+',
          LineKind::Remove => '-',
          LineKind::Context => ' ',
      }
  }

  #[must_use]
  pub fn render(old: &str, new: &str, path: Option<&str>, theme: &TuiTheme) -> Vec<StyledLine> {
      let rows = diff_rows(old, new);
      if rows.is_empty() {
          return Vec::new();
      }
      // Gutter width = widest line number, right-aligned.
      let max_no = rows.iter().map(|r| r.line_no).max().unwrap_or(1);
      let gutter_w = max_no.to_string().len();
      let lang = syntax::detect_language(None, path);

      rows.into_iter()
          .map(|row| {
              let bg = match row.kind {
                  LineKind::Add => add_bg(theme),
                  LineKind::Remove => remove_bg(theme),
                  LineKind::Context => <DEFAULT_BG>,
              };
              // Gutter span: "  12 " right-aligned + sigil + space.
              let gutter = format!("{:>w$} {} ", row.line_no, sigil(row.kind), w = gutter_w);
              let mut spans = vec![StyledSpan {
                  text: gutter,
                  fg: <DIM_FG>,
                  bg,
                  bold: false,
              }];
              // Content: syntax-highlight the single line, then overlay the
              // line bg onto each content span (keep syntect fg, force diff bg).
              let highlighted = syntax::highlight(&row.text, lang.as_deref(), theme);
              if let Some(first) = highlighted.into_iter().next() {
                  for mut s in first.spans {
                      s.bg = bg;
                      spans.push(s);
                  }
              } else {
                  // Empty line — still carry the bg so the row colors fully.
                  spans.push(StyledSpan { text: String::new(), fg: <DEFAULT_FG>, bg, bold: false });
              }
              StyledLine { spans }
          })
          .collect()
  }
  ```

  > **Implementer note:** substitute `<COLOR>`, `<ADD_BG>`, `<REMOVE_BG>`, `<DEFAULT_BG>`, `<DIM_FG>`, `<DEFAULT_FG>` with M7-01's color type and concrete values. Reuse the M6 `TuiTheme::DIM`/`ERROR` palette intent: add bg = a dark green, remove bg = a dark red (claude-code `diffAdded`/`diffRemoved`). If M7-01's color type is `iocraft::Color`, `<ADD_BG>` = `iocraft::Color::Rgb { r: 0x00, g: 0x40, b: 0x00 }` and `<REMOVE_BG>` = `iocraft::Color::Rgb { r: 0x40, g: 0x00, b: 0x00 }` (dark, low-saturation so syntax fg stays readable).

- [ ] **Step 4: Run the test to verify it passes.**

  Run: `cargo test -p lingxi-tui --lib diff::tests::render_`
  Expected: PASS (4 tests).

- [ ] **Step 5: Commit.**

  ```bash
  git add crates/tui/src/render/diff.rs
  git commit -m "plan(M7-02 T8): diff layout — gutter + sigil + green/red lines + syntax content

  Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
  ```

---

## Task 9: Word-level intra-line diff for adjacent remove/add pairs

**Files:**
- Read: `claude-code/src/components/StructuredDiff/Fallback.tsx` (lines ~226-315: `calculateWordDiffs`, `generateWordDiffElements`, `CHANGE_THRESHOLD`)
- Modify: `crates/tui/src/render/diff.rs`
- Test: inline `#[cfg(test)] mod tests`

- [ ] **Step 1: Write the failing test for word-level diffing.**

  Add to the `tests` module in `diff.rs`:
  ```rust
  #[test]
  fn word_diff_highlights_only_changed_words() {
      // "function oldName(param)" -> "function newName(param)": only the
      // word "oldName"/"newName" should carry the intra-line emphasis bg.
      let lines = render(
          "function oldName(param)\n",
          "function newName(param)\n",
          Some("x.js"),
          &TuiTheme,
      );
      // One remove row + one add row.
      let rem = lines.iter().find(|l| rowline(l).contains('-')).unwrap();
      let add = lines.iter().find(|l| rowline(l).contains('+')).unwrap();
      // The changed-word span ("oldName"/"newName") carries the EMPHASIS bg,
      // while the unchanged "function "/"(param)" spans carry the line bg.
      assert!(rem.spans.iter().any(|s| s.text.contains("oldName") && s.bg == remove_word_bg(&TuiTheme)));
      assert!(add.spans.iter().any(|s| s.text.contains("newName") && s.bg == add_word_bg(&TuiTheme)));
      // The shared word "function " is NOT emphasized.
      assert!(add.spans.iter().any(|s| s.text.contains("function") && s.bg != add_word_bg(&TuiTheme)));
  }

  #[test]
  fn word_diff_skipped_when_lines_too_dissimilar() {
      // Wildly different lines (> CHANGE_THRESHOLD changed) fall back to
      // whole-line coloring: no word-emphasis spans.
      let lines = render("aaaaaaaa\n", "zzzzzzzz\n", Some("x.txt"), &TuiTheme);
      let add = lines.iter().find(|l| rowline(l).contains('+')).unwrap();
      assert!(add.spans.iter().all(|s| s.bg != add_word_bg(&TuiTheme)),
              "dissimilar lines use whole-line coloring, not word emphasis");
  }
  ```

- [ ] **Step 2: Run the test to verify it fails.**

  Run: `cargo test -p lingxi-tui --lib diff::tests::word_diff`
  Expected: FAIL — `add_word_bg`/`remove_word_bg` not found; no word emphasis.

- [ ] **Step 3: Implement word-level diffing.**

  In `diff.rs`, add the emphasis colors and a word-diff pass. The `render` function must, after `diff_rows`, detect adjacent `Remove*`→`Add*` runs (mirroring claude-code `processAdjacentLines`), pair them line-for-line, and for each pair run `similar::TextDiff::from_words` between the remove text and add text. If the changed fraction ≤ `CHANGE_THRESHOLD` (0.4), emit per-word spans where the changed words on the remove line get `remove_word_bg` and on the add line get `add_word_bg`; unchanged words keep the line bg. Otherwise fall through to the whole-line coloring from Task 8.

  ```rust
  /// claude-code CHANGE_THRESHOLD: above this changed-fraction, word diffing is
  /// abandoned for whole-line coloring (lines too dissimilar to align words).
  const CHANGE_THRESHOLD: f64 = 0.4;

  /// Brighter emphasis backgrounds for the changed *words* within a paired
  /// remove/add line (claude-code `diffRemovedWord` / `diffAddedWord`).
  #[must_use]
  pub fn add_word_bg(_t: &TuiTheme) -> <COLOR> { <ADD_WORD_BG> }     // brighter green
  #[must_use]
  pub fn remove_word_bg(_t: &TuiTheme) -> <COLOR> { <REMOVE_WORD_BG> } // brighter red

  /// Build the content spans for one line of a paired word-diff. `is_add`
  /// selects which side's changes to emphasize. Returns `None` if the two
  /// lines are too dissimilar (caller falls back to whole-line coloring).
  fn word_diff_spans(
      remove_text: &str,
      add_text: &str,
      is_add: bool,
      theme: &TuiTheme,
      line_bg: <COLOR>,
  ) -> Option<Vec<StyledSpan>> {
      let wd = TextDiff::from_words(remove_text, add_text);
      // Changed fraction = changed words / total words on this side.
      let (mut changed, mut total) = (0usize, 0usize);
      for ch in wd.iter_all_changes() {
          let relevant = match ch.tag() {
              ChangeTag::Equal => { total += 1; false }
              ChangeTag::Delete => { if !is_add { total += 1; changed += 1; } true }
              ChangeTag::Insert => { if is_add { total += 1; changed += 1; } true }
          };
          let _ = relevant;
      }
      if total == 0 || (changed as f64 / total as f64) > CHANGE_THRESHOLD {
          return None;
      }
      let emph_bg = if is_add { add_word_bg(theme) } else { remove_word_bg(theme) };
      let mut spans = Vec::new();
      for ch in wd.iter_all_changes() {
          let show = matches!(ch.tag(), ChangeTag::Equal)
              || (is_add && ch.tag() == ChangeTag::Insert)
              || (!is_add && ch.tag() == ChangeTag::Delete);
          if !show { continue; }
          let emphasized = !matches!(ch.tag(), ChangeTag::Equal);
          spans.push(StyledSpan {
              text: ch.value().to_string(),
              fg: <DEFAULT_FG>,
              bg: if emphasized { emph_bg } else { line_bg },
              bold: false,
          });
      }
      Some(spans)
  }
  ```

  Wire this into `render`: before emitting an add/remove row, look back/ahead to find its paired counterpart; if a pair exists and `word_diff_spans` returns `Some`, use those content spans (prepended with the gutter) instead of the syntax-highlighted whole-line spans from Task 8. Keep Task 8's path for unpaired/context lines and for the `None` (too-dissimilar) fallback.

- [ ] **Step 4: Run the test to verify it passes.**

  Run: `cargo test -p lingxi-tui --lib diff::tests::word_diff`
  Expected: PASS (2 tests). Re-run the Task 8 tests too: `cargo test -p lingxi-tui --lib diff::tests::render_` → still PASS.

- [ ] **Step 5: Commit.**

  ```bash
  git add crates/tui/src/render/diff.rs
  git commit -m "plan(M7-02 T9): word-level intra-line diff with CHANGE_THRESHOLD fallback

  Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
  ```

---

## Task 10: Hunk headers + large-diff truncation

**Files:**
- Modify: `crates/tui/src/render/diff.rs`
- Test: inline `#[cfg(test)] mod tests`

- [ ] **Step 1: Write the failing tests for hunk headers + truncation.**

  Add to the `tests` module in `diff.rs`:
  ```rust
  #[test]
  fn render_with_separated_changes_emits_hunk_header() {
      // Two change clusters separated by a long unchanged run -> the second
      // cluster is preceded by a hunk header "@@ ... @@".
      let old = (1..=40).map(|n| format!("line{n}")).collect::<Vec<_>>().join("\n") + "\n";
      let mut new_lines: Vec<String> = (1..=40).map(|n| format!("line{n}")).collect();
      new_lines[2] = "CHANGED_TOP".into();
      new_lines[37] = "CHANGED_BOTTOM".into();
      let new = new_lines.join("\n") + "\n";
      let lines = render(&old, &new, Some("x.txt"), &TuiTheme);
      let headers = lines.iter().filter(|l| rowline(l).contains("@@")).count();
      assert!(headers >= 1, "separated change clusters produce hunk header(s)");
  }

  #[test]
  fn render_truncates_large_diff_with_footer() {
      // A diff with > MAX_DIFF_LINES changed lines is truncated with a footer.
      let old = String::new();
      let new = (0..(MAX_DIFF_LINES + 50)).map(|n| format!("add{n}")).collect::<Vec<_>>().join("\n") + "\n";
      let lines = render(&old, &new, Some("x.txt"), &TuiTheme);
      assert!(lines.len() <= MAX_DIFF_LINES + 1, "truncated to cap + footer");
      let last = rowline(lines.last().unwrap());
      assert!(last.contains("more lines"), "truncation footer present: {last:?}");
  }
  ```

- [ ] **Step 2: Run the test to verify it fails.**

  Run: `cargo test -p lingxi-tui --lib diff::tests`
  Expected: FAIL — `MAX_DIFF_LINES` not found; no `@@` headers; no footer.

- [ ] **Step 3: Implement hunk grouping into the `similar` pass + the cap.**

  Use `similar`'s grouped diff to insert hunk headers and cap output. Add to `diff.rs`:
  ```rust
  /// Hard cap on rendered diff lines (claude-code shows a "… N more lines"
  /// footer past a budget). Mirrors the M6 UserToolResult MAX_LINES intent.
  pub const MAX_DIFF_LINES: usize = 100;

  /// Build a "@@ -oldStart,oldLen +newStart,newLen @@" header span.
  fn hunk_header(old_start: usize, old_len: usize, new_start: usize, new_len: usize) -> StyledLine {
      StyledLine {
          spans: vec![StyledSpan {
              text: format!("@@ -{old_start},{old_len} +{new_start},{new_len} @@"),
              fg: <DIM_FG>,   // claude-code renders hunk headers dim/cyan
              bg: <DEFAULT_BG>,
              bold: false,
          }],
      }
  }
  ```

  In `render`, switch from `iter_all_changes()` over the whole text to `TextDiff::grouped_ops(context_radius)` (context_radius = 3, matching unified-diff default): each group is a hunk. Before each group, emit `hunk_header(..)` computed from the group's first/last op line numbers. Keep a running count of emitted body rows; once it would exceed `MAX_DIFF_LINES`, stop and push a final footer line `format!("… {n} more lines")` where `n` is the number of remaining changed rows. (Reuse the same per-row coloring/word-diff logic from Tasks 8-9 inside each group.)

- [ ] **Step 4: Run the test to verify it passes.**

  Run: `cargo test -p lingxi-tui --lib diff::tests`
  Expected: PASS (all diff unit tests incl. the new two). Re-confirm Tasks 7-9 still pass.

- [ ] **Step 5: Commit.**

  ```bash
  git add crates/tui/src/render/diff.rs
  git commit -m "plan(M7-02 T10): hunk headers (grouped_ops) + large-diff truncation footer

  Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
  ```

---

## Task 11: Diff structure-snapshot suite

**Files:**
- Create: `crates/tui/tests/render_diff.rs`
- Snapshots: `crates/tui/tests/snapshots/`

- [ ] **Step 1: Write the structure-snapshot tests.**

  Create `crates/tui/tests/render_diff.rs`:
  ```rust
  //! Structure snapshots for render::diff (M7-02).
  //! PARITY (design §0 Q3): snapshot STRUCTURE — sigils (+/-/space), gutter line
  //! numbers, hunk headers, and per span Colored/Plain/Emphasized — NOT exact
  //! per-token colors. Concrete colors are normalized to C/P/E flags.
  use lingxi_tui::render::diff::{render, add_word_bg, remove_word_bg};
  use lingxi_tui::render::StyledLine;
  use lingxi_tui::theme::TuiTheme;

  /// Flag each span: 'E' if it carries a word-emphasis bg, 'C' if fg colored,
  /// else 'P'. Prefix each line with the flag string + the literal text so the
  /// snapshot is color-stable but structure-revealing.
  fn structure(lines: &[StyledLine]) -> String {
      lines
          .iter()
          .map(|l| {
              l.spans
                  .iter()
                  .map(|s| {
                      let emph = s.bg == add_word_bg(&TuiTheme) || s.bg == remove_word_bg(&TuiTheme);
                      let flag = if emph { 'E' } else if s.fg != /* DEFAULT_FG */ Default::default() { 'C' } else { 'P' };
                      format!("[{flag}]{}", s.text)
                  })
                  .collect::<String>()
          })
          .collect::<Vec<_>>()
          .join("\n")
  }

  #[test]
  fn snapshot_pure_add() {
      insta::assert_snapshot!(structure(&render("a\n", "a\nb\n", Some("x.rs"), &TuiTheme)));
  }
  #[test]
  fn snapshot_pure_remove() {
      insta::assert_snapshot!(structure(&render("a\nb\n", "a\n", Some("x.rs"), &TuiTheme)));
  }
  #[test]
  fn snapshot_modify_mixed() {
      insta::assert_snapshot!(structure(&render("foo\nkeep\n", "bar\nkeep\n", Some("x.rs"), &TuiTheme)));
  }
  #[test]
  fn snapshot_word_level_intraline() {
      insta::assert_snapshot!(structure(&render(
          "function oldName(p)\n", "function newName(p)\n", Some("x.js"), &TuiTheme)));
  }
  #[test]
  fn snapshot_empty_diff() {
      let s = structure(&render("a\nb\n", "a\nb\n", Some("x.rs"), &TuiTheme));
      assert!(!s.contains('+') && !s.contains('-'));
      insta::assert_snapshot!(s);
  }
  #[test]
  fn snapshot_large_diff_truncation() {
      let new = (0..160).map(|n| format!("L{n}")).collect::<Vec<_>>().join("\n") + "\n";
      let s = structure(&render("", &new, Some("x.txt"), &TuiTheme));
      assert!(s.contains("more lines"), "truncation footer in snapshot");
      insta::assert_snapshot!(s);
  }
  ```

  > **Implementer note:** substitute the real default-fg sentinel for `Default::default()`. Ensure `add_word_bg`/`remove_word_bg`/`render` and `StyledLine` are `pub` and reachable at the asserted paths.

- [ ] **Step 2: Run the tests and accept the snapshots.**

  Run: `cargo test -p lingxi-tui --test render_diff`
  Expected: 6 `.snap.new` files. Review: pure-add shows `[..]+` rows with green-bg (no `[E]` since no pairing), pure-remove `[..]-` rows, modify shows a `-`/`+` pair, word-level shows `[E]` on the changed word only, empty has only context, large shows a `more lines` footer. Then:
  ```bash
  cargo insta accept
  ```
  Re-run → PASS (6).

- [ ] **Step 3: Commit.**

  ```bash
  git add crates/tui/tests/render_diff.rs crates/tui/tests/snapshots/
  git commit -m "plan(M7-02 T11): diff structure-snapshots (add/remove/modify/word/empty/truncation)

  Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
  ```

---

## Task 12: Route markdown code fences through syntect

**Files:**
- Read: `crates/tui/src/render/markdown.rs` (M7-01 — find the fenced-code placeholder span emission)
- Modify: `crates/tui/src/render/markdown.rs`
- Test: inline `#[cfg(test)] mod tests` in `markdown.rs` (or a `tests/render_markdown_fence.rs` if M7-01 used an external test file — match M7-01's convention)

- [ ] **Step 1: Locate the M7-01 placeholder.**

  Open `render/markdown.rs`. Find where M7-01 handles a fenced code block (pulldown-cmark `Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(info)))` … `Event::Text` … `Event::End`). M7-01 currently emits a placeholder span for the fence body (design §3 M7-01). Note the exact local variable holding the fence info-string (the language) and the accumulated code text, and how `Vec<StyledLine>` lines are appended.

- [ ] **Step 2: Write the failing test for fence highlighting.**

  Add a test (matching M7-01's test-location convention):
  ```rust
  #[test]
  fn fenced_rust_block_is_syntax_highlighted() {
      // A markdown doc with a ```rust fence renders the code lines with at
      // least one colored span (structure, not exact color — §0 Q3).
      let md = "Here:\n\n```rust\nfn main() {}\n```\n";
      let lines = render(md, &TuiTheme); // M7-01 markdown entry point
      // Find the code line "fn main() {}" among the rendered lines.
      let code_line = lines.iter().find(|l| {
          l.spans.iter().map(|s| s.text.as_str()).collect::<String>().contains("fn main")
      }).expect("code line rendered");
      assert!(code_line.spans.iter().any(|s| s.fg != /* DEFAULT_FG */ Default::default()),
              "fenced rust is syntax-highlighted, not a plain placeholder");
  }

  #[test]
  fn fenced_unknown_lang_block_is_plain() {
      let md = "```klingon\nQapla'\n```\n";
      let lines = render(md, &TuiTheme);
      let code_line = lines.iter().find(|l| {
          l.spans.iter().map(|s| s.text.as_str()).collect::<String>().contains("Qapla")
      }).expect("code line rendered");
      assert!(code_line.spans.iter().all(|s| s.fg == /* DEFAULT_FG */ Default::default()),
              "unknown-lang fence falls back to plain");
  }
  ```

  > **Implementer note:** replace `render(md, &TuiTheme)` with the actual M7-01 markdown entry point signature confirmed in Step 1, and the default-fg sentinel.

- [ ] **Step 3: Run the test to verify it fails.**

  Run: `cargo test -p lingxi-tui fenced_`
  Expected: FAIL — the placeholder produces no colored spans.

- [ ] **Step 4: Replace the placeholder with a `syntax::highlight` call.**

  At the fence-body emission site, replace the placeholder span construction with:
  ```rust
  // M7-02: route the fenced code body through syntect. `info` is the fence
  // info-string ("rust", "python", …); empty/unknown -> plain fallback.
  let lang = crate::render::syntax::detect_language(
      if info.is_empty() { None } else { Some(info.as_ref()) },
      None,
  );
  let highlighted = crate::render::syntax::highlight(&code_buf, lang.as_deref(), theme);
  lines.extend(highlighted);
  ```
  Adapt `info`, `code_buf`, `lines`, and `theme` to the real local names from Step 1. Keep any surrounding fence decoration (border, padding) M7-01 already emits.

- [ ] **Step 5: Run the test to verify it passes.**

  Run: `cargo test -p lingxi-tui fenced_`
  Expected: PASS (2). Re-run the full M7-01 markdown test set to confirm no regression: `cargo test -p lingxi-tui markdown`.

- [ ] **Step 6: Commit.**

  ```bash
  git add crates/tui/src/render/markdown.rs crates/tui/tests/
  git commit -m "plan(M7-02 T12): route markdown code fences through render::syntax

  Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
  ```

---

## Task 13: Render Edit/Write tool results as StructuredDiff

**Files:**
- Read: `claude-code/src/components/FileEditToolDiff.tsx` + `crates/tui/src/components/messages/user_tool_result.rs` (M6 renderer — `body_text`, `UserToolResultMessage` component)
- Modify: `crates/tui/src/components/messages/user_tool_result.rs`
- Test: Create `crates/tui/tests/render_edit_write_diff.rs`

- [ ] **Step 1: Read references and lock the Edit/Write result shapes.**

  M6's `UserToolResultProps` carries `tool: String` and `result: serde_json::Value`. For Edit/Write the diff inputs come from the **tool call input** (claude-code `FileEditToolDiff` reads `old_string`/`new_string`/`file_path` for Edit, `content`/`file_path` for Write). The M6 renderer only has the *result* JSON. Determine where the call input is available at render time:
  - If `UserToolResultProps` already pairs to the call (M6-04 added `id: ToolUseId` correlating call↔result), the diff inputs must be threaded in. Add two optional fields to `UserToolResultProps`: `old_string: Option<String>` and `new_string: Option<String>` (populated by the dispatcher from the paired `AssistantToolUse.input` for Edit/Write tools). For Write, `old_string = None` (treated as empty → pure-add diff) and `new_string = content`. For Edit, both come from the input JSON.
  - Lock the input JSON keys from claude-code: Edit = `{ "file_path", "old_string", "new_string" }`; Write = `{ "file_path", "content" }`.

- [ ] **Step 2: Write the failing behavior test.**

  Create `crates/tui/tests/render_edit_write_diff.rs`:
  ```rust
  //! Edit/Write tool results render as a StructuredDiff (M7-02).
  use lingxi_tui::components::messages::user_tool_result::{
      render_edit_write_diff_lines, UserToolResultProps,
  };
  use lingxi_tui::theme::TuiTheme;

  #[test]
  fn edit_result_renders_structured_diff() {
      let lines = render_edit_write_diff_lines(
          "Edit",
          Some("foo()"),
          Some("bar()"),
          Some("src/a.rs"),
          &TuiTheme,
      );
      let joined: String = lines.iter()
          .flat_map(|l| l.spans.iter().map(|s| s.text.clone()))
          .collect();
      assert!(joined.contains('-') && joined.contains('+'), "Edit shows a -/+ diff");
      assert!(joined.contains("foo") && joined.contains("bar"));
  }

  #[test]
  fn write_result_renders_as_pure_add() {
      let lines = render_edit_write_diff_lines(
          "Write",
          None,                 // no prior content
          Some("new line 1\nnew line 2"),
          Some("src/b.rs"),
          &TuiTheme,
      );
      let joined: String = lines.iter()
          .flat_map(|l| l.spans.iter().map(|s| s.text.clone()))
          .collect();
      assert!(joined.contains('+'), "Write is a pure-add diff");
      assert!(!joined.contains('-'), "Write has no remove lines");
  }

  #[test]
  fn non_edit_tool_returns_none_for_diff() {
      // A Bash/Read result must NOT route through the diff path.
      assert!(!is_diff_tool("Bash"));
      assert!(!is_diff_tool("Read"));
      assert!(is_diff_tool("Edit"));
      assert!(is_diff_tool("Write"));
  }
  ```

  > Use the real `is_diff_tool` import path; add it as a `pub fn` in `user_tool_result.rs`.

- [ ] **Step 3: Run the test to verify it fails.**

  Run: `cargo test -p lingxi-tui --test render_edit_write_diff`
  Expected: FAIL — `render_edit_write_diff_lines`, `is_diff_tool` not found.

- [ ] **Step 4: Implement the Edit/Write diff branch.**

  In `user_tool_result.rs`:
  ```rust
  use crate::render::{diff, StyledLine};

  /// True for tools whose result is shown as a StructuredDiff.
  #[must_use]
  pub fn is_diff_tool(tool: &str) -> bool {
      matches!(tool, "Edit" | "Write" | "MultiEdit" | "NotebookEdit")
  }

  /// Build the StructuredDiff lines for an Edit/Write tool. Write = pure add
  /// (old = ""); Edit = old_string → new_string. `path` drives syntax lang.
  #[must_use]
  pub fn render_edit_write_diff_lines(
      _tool: &str,
      old_string: Option<&str>,
      new_string: Option<&str>,
      path: Option<&str>,
      theme: &TuiTheme,
  ) -> Vec<StyledLine> {
      let old = old_string.unwrap_or("");
      let new = new_string.unwrap_or("");
      diff::render(old, new, path, theme)
  }
  ```

  Then in the `UserToolResultMessage` component, add a branch (before the existing Bash/plain branches): when `is_diff_tool(&props.tool)` and the diff inputs are present, render the `render_edit_write_diff_lines` output as one `Text` element per `StyledLine` (one `View` row per line, spans as colored child `Text`s — mirror the existing Bash-span rendering loop in this file, mapping `StyledSpan.fg`/`.bg`/`.bold` to `iocraft` `Text` props). Fall through to the existing string/Bash rendering for non-diff tools.

  Add the two new optional fields to `UserToolResultProps` (`old_string`/`new_string`) and have the messages-mod dispatcher populate them from the paired call input for diff tools. (The dispatcher in `components/messages/mod.rs` already correlates by `ToolUseId` from M6-04.)

- [ ] **Step 5: Run the test to verify it passes.**

  Run: `cargo test -p lingxi-tui --test render_edit_write_diff`
  Expected: PASS (3). Re-run the M6 tool-result tests to confirm Bash/Read/plain paths are untouched: `cargo test -p lingxi-tui user_tool_result`.

- [ ] **Step 6: Commit.**

  ```bash
  git add crates/tui/src/components/messages/ crates/tui/tests/render_edit_write_diff.rs
  git commit -m "plan(M7-02 T13): Edit/Write tool results render as StructuredDiff

  Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
  ```

---

## Task 14: Workspace verification gate + tag `m7.2`

**Files:**
- Read/run only: full `lingxi-tui` + workspace suite.

- [ ] **Step 1: Confirm telemetry baseline is unchanged (M7-02 adds 0 events).**

  Run (from inside `lingxi-code/`): `cargo test --workspace event_names`
  Expected: the `ALL_EVENT_NAMES.len()` assertion still reads **326** (design §2.7: M7-02 adds zero telemetry events). If any test now reports a different count, M7-02 accidentally registered an event — revert that; M7-02 is render-only.

- [ ] **Step 2: Run the full workspace gate from inside `lingxi-code/`.**

  > Run from inside `lingxi-code/` — the toolchain pins 1.82.0 there; running from the repo root uses the host toolchain and produces spurious lint noise (this bit M6-08).

  ```bash
  cargo fmt --check
  cargo clippy --workspace --all-targets -- -D warnings
  cargo test --workspace
  cargo check --workspace --target x86_64-unknown-linux-gnu
  cargo check --workspace --target x86_64-apple-darwin
  cargo check --workspace --target x86_64-pc-windows-gnu
  cargo check --workspace --target aarch64-linux-android
  cargo check --workspace --target aarch64-apple-ios
  ```
  Expected: fmt clean; clippy clean; all tests pass; all 5 targets `cargo check` green (syntect's `fancy-regex` backend is pure-Rust → cross-compiles like any Rust crate, no `onig` C toolchain needed — this is the whole point of `default-fancy`).

  Known flakes (allowed rerun, NOT failures): `rapid_writes_collapse_to_single_event`, `writer_output_equals_single_turn_fixture`, `streaming_concurrent_tools_test`, `lingxi-platform-posix` fs_watch FSEvents timing tests. If only these fail, re-run them individually to confirm green.

- [ ] **Step 3: Sanity-confirm no `onig` leaked into the tree.**

  Run: `cargo tree -p lingxi-tui -i onig`
  Expected: "package ID specification `onig` did not match any packages" (pure-Rust backend confirmed; no C dependency).

- [ ] **Step 4: Tag the sub-plan.**

  ```bash
  git tag -a m7.2 -m "M7-02: syntect highlighting + StructuredDiff viewer"
  ```
  (Local tag only — no remote push, per design §6.4.)

- [ ] **Step 5: Final commit if any gate fixes were needed.**

  ```bash
  git add -A
  git commit -m "plan(M7-02 T14): workspace gate green (5 targets) + tag m7.2

  Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
  ```
  (Skip this commit if Steps 1-3 required no changes; the tag in Step 4 still stands.)

---

## Self-Review

**Spec coverage (design §3 M7-02 + task brief):**
- `render/syntax.rs` syntect wrapper, fence + path lang detection, theme→`.tmTheme`, `fancy-regex` backend, `Vec<StyledLine>` → Tasks 1-6. ✓
- `render/diff.rs` StructuredDiff with `similar` line + word diff, green/red, hunk headers, per-line syntax color, claude-code layout → Tasks 7-11. ✓
- Markdown code fences routed through syntax → Task 12. ✓
- Edit/Write results → StructuredDiff → Task 13. ✓
- `syntect` + `similar` added to Cargo.toml, exact-pinned, MSRV 1.82 verified → Task 1. ✓
- MSRV gate as Task 1, with GATE FALLBACK note (`two-face` / minimal tokenizer) → Task 1. ✓
- Parity caveat (structure, not per-token color) made explicit in every snapshot test → Tasks 5, 11, 12 (and the header). ✓
- Tests required: syntax {rust, python, js, json, unknown, empty} → Task 5 ✓; diff {add, remove, modify, word, empty, truncation} → Task 11 ✓.
- Telemetry baseline 326, +0 → Task 14 Step 1. ✓
- Final task = workspace gate (from inside `lingxi-code/`) + tag `m7.2` → Task 14. ✓
- Commit format `plan(M7-02 TN): <subject>` + Co-Authored-By trailer → every task. ✓
- iocraft 0.8.3 `View` not `Box`; run cargo from inside `lingxi-code/` → noted in header + Task 14. ✓

**Type consistency:** `StyledLine`/`StyledSpan` (M7-01-defined, confirm-before-use flagged in header + Task 2); `render::syntax::{detect_language, highlight, tm_theme_for}`; `render::diff::{render, diff_rows, DiffRow, LineKind, add_bg, remove_bg, add_word_bg, remove_word_bg, MAX_DIFF_LINES}`; `user_tool_result::{is_diff_tool, render_edit_write_diff_lines}` — all names used consistently across the tasks that reference them.

**Placeholder scan:** The `<COLOR>`/`<DEFAULT_FG>`/`<ADD_BG>` etc. tokens are deliberate, clearly-marked substitution points for M7-01's color type (whose exact name cannot be known until M7-01 lands) — each is accompanied by a concrete `iocraft::Color::Rgb` example so the implementer is never left guessing. This is the one unavoidable indirection given M7-01 is a prerequisite; it is bounded (one color type, six named values) and every site says exactly what to substitute.
