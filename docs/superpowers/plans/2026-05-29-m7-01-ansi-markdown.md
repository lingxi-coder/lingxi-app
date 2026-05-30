# M7-01 — Full ANSI Parser + Markdown Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Create the `render/` module in `lingxi-tui`, upgrade M6's 8/16-color ANSI parser to full fidelity (256-color + truecolor + safely-skipped cursor/erase sequences) under a new shared styled-line model, and add a CommonMark markdown renderer (`pulldown-cmark`) that emits `Vec<StyledLine>` with a placeholder span for fenced code blocks (filled by M7-02).

**Architecture:** A new `lingxi-tui/src/render/` module owns three things: (1) a shared styled-line value model (`StyleColor`, `SpanStyle`, `StyledSpan`, `StyledLine`) that supersedes M6's 16-color-only `AnsiStyle`; (2) `render/ansi.rs`, the moved-and-expanded ANSI parser that now decodes `38;5;N`/`48;5;N` (256-color) and `38;2;r;g;b`/`48;2;r;g;b` (truecolor) and silently consumes cursor-move/erase CSI sequences without corrupting text; (3) `render/markdown.rs`, a pure `render(text, theme) -> Vec<StyledLine>` function over `pulldown-cmark` events handling headings, bold, italic, lists (ordered/unordered/nested), blockquote, inline code, links, with fenced code emitting a single `CodePlaceholder`-marked span. All three are pure functions tested with `insta` snapshots. The old `crate::ansi` module is deleted; its one consumer (`user_tool_result.rs`) migrates to `crate::render::ansi`.

**Tech Stack:** Rust 1.82.0 (pinned via `lingxi-code/rust-toolchain.toml`), iocraft `=0.8.3`, `pulldown-cmark = "=0.13.4"` (new — MSRV 1.71.1, verified < 1.82), `insta` 1.40 (existing dev-dep).

---

## Critical Context (read before starting)

**Workspace toolchain trap.** The workspace pins **rust 1.82.0** via `lingxi-code/rust-toolchain.toml`. ALL cargo commands in this plan MUST run from inside `lingxi-code/` (e.g. `cd lingxi-core && cargo …`). Running cargo from the repo root uses the host toolchain and produces spurious lint noise — this bit M6-08. Every verification command below already does this.

**iocraft facts (locked in M6).** iocraft `=0.8.3`. The layout element is `View`, NOT `Box`. Import via `use iocraft::prelude::*;`. `Color` comes from iocraft. M7-01 itself ships **pure value functions** (no iocraft components) — the iocraft mapping for `StyleColor` is a thin helper that M7-02+ renderers consume.

**No existing `StyledLine` type.** Verified: M6 renderers (`components/messages/*.rs`) return `String`. The only styled value type in the crate today is `AnsiSpan { style: AnsiStyle, text: String }` / `AnsiStyle { fg: AnsiColor, bg: AnsiColor, bold: bool }` / `AnsiColor` (16 named colors only), all in `src/ansi.rs`. This plan therefore **defines** `StyledLine`/`StyledSpan` in the new `render/` module rather than reusing an existing one — `render::ansi` and `render::markdown` both produce `Vec<StyledLine>`, the shared currency M7-02 (syntect), M7-04/05 (renderers), and M7-15 (theme) all build on.

**Old `AnsiColor` is insufficient.** M6's `AnsiColor` enum has 16 fixed variants — it cannot represent 256-color (`38;5;N`) or truecolor (`38;2;r;g;b`). The new `StyleColor` enum (Task 2) supersedes it: `Default` / `Named(NamedColor)` / `Indexed(u8)` / `Rgb(u8,u8,u8)`. The 16 M6 names live on as `NamedColor`. The old `src/ansi.rs` is deleted at Task 6 once its sole consumer is migrated.

**Sole `ansi` consumer.** `grep` confirms only `src/components/messages/user_tool_result.rs` imports `crate::ansi` (`parse_ansi`, `AnsiColor`, `AnsiSpan`, `AnsiStyle`) and maps colors to iocraft in `ansi_to_iocraft_color`. Task 6 migrates it to the new `render::ansi` API + the new `StyleColor` mapper. `src/lib.rs` declares `pub mod ansi;` — Task 6 replaces it with `pub mod render;`.

**Telemetry baseline.** `ALL_EVENT_NAMES.len() == 326`. M7-01 is pure rendering and adds **0 events**. Do not touch `src/telemetry.rs` or any event registry.

**claude-code references (submodule present, verified).** Read these before writing the markdown renderer:
- `claude-code/src/utils/markdown.ts` — `formatToken` is the literal reference for every markdown element. Key locked behaviors: h1 = bold+italic+underline + two EOLs; h2/h3+ = bold + two EOLs; `codespan` (inline code) uses the `permission` theme color; `code` (fenced) returns `token.text` verbatim when no highlighter (our placeholder case); blockquote prefixes each non-blank line with a dim `BLOCKQUOTE_BAR` then italic; unordered list item marker is `-`, ordered is `N.`; nested lists indent two spaces per depth (`'  '.repeat(listDepth)`); strikethrough (`del`) is intentionally disabled; `html`/`def`/`del` render to empty string.
- `claude-code/src/utils/cliHighlight.ts` — code-fence highlighting is delegated to a `CliHighlight` object; when absent, `code` falls back to plaintext. M7-01's placeholder span IS that "no highlighter yet" state; M7-02 supplies the highlighter.

**Known flakes (allow rerun, NOT caused by this plan):** `rapid_writes_collapse_to_single_event`, `writer_output_equals_single_turn_fixture`, `streaming_concurrent_tools_test`, `lingxi-platform-posix` fs_watch FSEvents timing tests.

**Commit discipline.** Every commit message subject is `plan(M7-01 TN): <subject>` and ends with the trailer:
```
Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
```
Local commits only. No remote push, no force-push, no `--amend`, no `--no-verify`. This work happens on the `m7-execution` worktree/branch created at the start of M7 per the design §6.5.

---

## File Structure

```
lingxi-code/crates/tui/
├── Cargo.toml                       MODIFY: add pulldown-cmark = "=0.13.4"
└── src/
    ├── lib.rs                       MODIFY: `pub mod ansi;` → `pub mod render;`
    ├── ansi.rs                      DELETE (moved+expanded into render/ansi.rs)
    ├── render/
    │   ├── mod.rs                   CREATE: module root; defines StyleColor,
    │   │                            NamedColor, SpanStyle, StyledSpan, StyledLine,
    │   │                            SpanKind; re-exports ansi + markdown
    │   ├── ansi.rs                  CREATE: expanded parser → Vec<StyledLine>
    │   │                            (16-color + 256 + truecolor + skip cursor/erase)
    │   └── markdown.rs              CREATE: pulldown-cmark → Vec<StyledLine>
    └── components/messages/
        └── user_tool_result.rs      MODIFY: migrate crate::ansi → crate::render
```

**Responsibility boundaries:**
- `render/mod.rs` — the shared value model only. No parsing logic. Defines the types every render submodule and downstream renderer speaks. Holds the `StyleColor → iocraft::Color` mapper (one place, reused by `user_tool_result.rs` and future M7-02+ consumers).
- `render/ansi.rs` — byte-level ANSI/SGR state machine. Input `&str`, output `Vec<StyledLine>` (splitting on `\n`). Knows nothing about markdown or theme.
- `render/markdown.rs` — `pulldown-cmark` event consumer. Input `(&str, &MarkdownTheme)`, output `Vec<StyledLine>`. Knows nothing about ANSI bytes. Emits a `SpanKind::CodePlaceholder` span for fenced blocks; never highlights.

---

## Task 1: Pin `pulldown-cmark` and verify MSRV 1.82 build

**Files:**
- Modify: `lingxi-code/crates/tui/Cargo.toml`

**Context:** Latest stable `pulldown-cmark` is `0.13.4` (verified on crates.io 2026-05-29; MSRV `rust-version = 1.71.1`, comfortably below the workspace's 1.82.0). The design §2.2 example string `=0.12.2` was illustrative; we pin the current latest that is MSRV-safe. Exact-pin discipline (`=`) matches `iocraft = "=0.8.3"`.

- [ ] **Step 1: Add the dependency, pinned exact**

In `lingxi-code/crates/tui/Cargo.toml`, under `[dependencies]`, immediately after the `iocraft = "=0.8.3"` line, add:

```toml
pulldown-cmark = { version = "=0.13.4", default-features = false }
```

(`default-features = false` drops the optional `getopts`-based `pulldown-cmark` binary; we only need the library parser. The `simd` feature stays off — pure-Rust, MSRV-safe.)

- [ ] **Step 2: Verify it resolves and builds on the pinned toolchain**

Run:
```bash
cd lingxi-core && cargo build -p lingxi-tui
```
Expected: PASS — compiles clean. The build downloads `pulldown-cmark 0.13.4` (and its `bitflags`/`unicase`/`pulldown-cmark-escape` transitive deps) and links them. If resolution picks any version other than `0.13.4`, the `=` pin is wrong — fix it.

- [ ] **Step 3: Confirm the locked version**

Run:
```bash
cd lingxi-core && cargo tree -p lingxi-tui -i pulldown-cmark
```
Expected: shows `pulldown-cmark v0.13.4` exactly once. No other version present.

- [ ] **Step 4: Commit**

```bash
cd lingxi-core && git add crates/tui/Cargo.toml Cargo.lock
git commit -m "$(cat <<'EOF'
plan(M7-01 T1): pin pulldown-cmark =0.13.4 (MSRV 1.71.1, verified on rust 1.82)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Define the shared styled-line value model (`render/mod.rs`)

**Files:**
- Create: `lingxi-code/crates/tui/src/render/mod.rs`

**Context:** This is the type vocabulary `render::ansi`, `render::markdown`, and all M7-02+ consumers share. `StyleColor` supersedes M6's `AnsiColor` so it can carry 256-color and truecolor. `SpanKind` lets markdown tag a span as a code-fence placeholder that M7-02 swaps for highlighted spans. No parsing here — just the data model + the one iocraft color mapper.

- [ ] **Step 1: Write the failing test**

Create `lingxi-code/crates/tui/src/render/mod.rs` with ONLY the test module first (so the build fails on the missing types):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn styled_line_holds_spans() {
        let line = StyledLine {
            spans: vec![
                StyledSpan::plain("hello "),
                StyledSpan {
                    text: "world".to_string(),
                    style: SpanStyle {
                        fg: StyleColor::Named(NamedColor::Red),
                        bold: true,
                        ..SpanStyle::default()
                    },
                    kind: SpanKind::Text,
                },
            ],
        };
        assert_eq!(line.spans.len(), 2);
        assert_eq!(line.plain_text(), "hello world");
        assert_eq!(line.spans[1].style.fg, StyleColor::Named(NamedColor::Red));
    }

    #[test]
    fn code_placeholder_span_is_tagged() {
        let span = StyledSpan::code_placeholder("fn main() {}", Some("rust"));
        assert_eq!(span.kind, SpanKind::CodePlaceholder { lang: Some("rust".to_string()) });
        assert_eq!(span.text, "fn main() {}");
    }

    #[test]
    fn default_style_is_plain_default() {
        let s = SpanStyle::default();
        assert_eq!(s.fg, StyleColor::Default);
        assert_eq!(s.bg, StyleColor::Default);
        assert!(!s.bold);
        assert!(!s.italic);
        assert!(!s.underline);
    }

    #[test]
    fn style_color_maps_to_iocraft() {
        use iocraft::Color;
        assert!(matches!(StyleColor::Default.to_iocraft(), Color::Reset));
        assert!(matches!(StyleColor::Named(NamedColor::Red).to_iocraft(), Color::DarkRed));
        assert!(matches!(StyleColor::Named(NamedColor::BrightRed).to_iocraft(), Color::Red));
        assert!(matches!(StyleColor::Rgb(10, 20, 30).to_iocraft(), Color::Rgb { r: 10, g: 20, b: 30 }));
        // 256-palette index resolves to an Rgb triple via the xterm cube.
        assert!(matches!(StyleColor::Indexed(196).to_iocraft(), Color::Rgb { .. }));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:
```bash
cd lingxi-core && cargo test -p lingxi-tui render::mod 2>&1 | head -30
```
Expected: FAIL — the file is not yet wired into `lib.rs` and the types are undefined (compile error). That is the expected failing state.

- [ ] **Step 3: Write the minimal implementation**

Prepend the following ABOVE the `#[cfg(test)] mod tests` block in `lingxi-code/crates/tui/src/render/mod.rs`:

```rust
//! Shared rendering primitives for the TUI surface (M7).
//!
//! This module owns the styled-line value model that every render
//! submodule and downstream message renderer speaks:
//!   - [`StyleColor`] — supersedes M6's 16-color `AnsiColor` with 256-color
//!     (`Indexed`) and truecolor (`Rgb`) support.
//!   - [`SpanStyle`] — fg/bg + bold/italic/underline attributes.
//!   - [`StyledSpan`] — one styled run; [`SpanKind`] tags code-fence
//!     placeholders that M7-02 fills with syntax-highlighted spans.
//!   - [`StyledLine`] — a single visual line (no embedded `\n`).
//!
//! `render::ansi` and `render::markdown` both produce `Vec<StyledLine>`.

pub mod ansi;
pub mod markdown;

use iocraft::Color;

/// The 16 named SGR colors (8 standard + 8 bright). Carried over from M6's
/// `AnsiColor`; lives inside [`StyleColor::Named`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamedColor {
    /// SGR 30 / 40.
    Black,
    /// SGR 31 / 41.
    Red,
    /// SGR 32 / 42.
    Green,
    /// SGR 33 / 43.
    Yellow,
    /// SGR 34 / 44.
    Blue,
    /// SGR 35 / 45.
    Magenta,
    /// SGR 36 / 46.
    Cyan,
    /// SGR 37 / 47.
    White,
    /// SGR 90 / 100.
    BrightBlack,
    /// SGR 91 / 101.
    BrightRed,
    /// SGR 92 / 102.
    BrightGreen,
    /// SGR 93 / 103.
    BrightYellow,
    /// SGR 94 / 104.
    BrightBlue,
    /// SGR 95 / 105.
    BrightMagenta,
    /// SGR 96 / 106.
    BrightCyan,
    /// SGR 97 / 107.
    BrightWhite,
}

/// A foreground or background color slot. Supersedes M6's `AnsiColor`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StyleColor {
    /// Terminal default — no override.
    #[default]
    Default,
    /// One of the 16 named SGR colors.
    Named(NamedColor),
    /// 256-color palette index (`38;5;N` / `48;5;N`).
    Indexed(u8),
    /// 24-bit truecolor (`38;2;r;g;b` / `48;2;r;g;b`).
    Rgb(u8, u8, u8),
}

impl StyleColor {
    /// Map to an iocraft [`Color`]. Named colors map to crossterm's "dark"
    /// range for standard and the non-dark range for bright, matching M6's
    /// `ansi_to_iocraft_color`. Indexed colors resolve through the xterm
    /// 256-color cube to an `Rgb` triple. `Rgb` passes through.
    #[must_use]
    pub fn to_iocraft(self) -> Color {
        match self {
            StyleColor::Default => Color::Reset,
            StyleColor::Named(n) => named_to_iocraft(n),
            StyleColor::Rgb(r, g, b) => Color::Rgb { r, g, b },
            StyleColor::Indexed(i) => {
                let (r, g, b) = xterm256_to_rgb(i);
                Color::Rgb { r, g, b }
            }
        }
    }
}

fn named_to_iocraft(n: NamedColor) -> Color {
    match n {
        NamedColor::Black => Color::Black,
        NamedColor::Red => Color::DarkRed,
        NamedColor::Green => Color::DarkGreen,
        NamedColor::Yellow => Color::DarkYellow,
        NamedColor::Blue => Color::DarkBlue,
        NamedColor::Magenta => Color::DarkMagenta,
        NamedColor::Cyan => Color::DarkCyan,
        NamedColor::White => Color::Grey,
        NamedColor::BrightBlack => Color::DarkGrey,
        NamedColor::BrightRed => Color::Red,
        NamedColor::BrightGreen => Color::Green,
        NamedColor::BrightYellow => Color::Yellow,
        NamedColor::BrightBlue => Color::Blue,
        NamedColor::BrightMagenta => Color::Magenta,
        NamedColor::BrightCyan => Color::Cyan,
        NamedColor::BrightWhite => Color::White,
    }
}

/// Resolve an xterm 256-color palette index to an 8-bit RGB triple.
/// 0..=15 are the system colors, 16..=231 the 6×6×6 cube, 232..=255 the
/// grayscale ramp. (Standard xterm mapping.)
fn xterm256_to_rgb(i: u8) -> (u8, u8, u8) {
    match i {
        0 => (0, 0, 0),
        1 => (128, 0, 0),
        2 => (0, 128, 0),
        3 => (128, 128, 0),
        4 => (0, 0, 128),
        5 => (128, 0, 128),
        6 => (0, 128, 128),
        7 => (192, 192, 192),
        8 => (128, 128, 128),
        9 => (255, 0, 0),
        10 => (0, 255, 0),
        11 => (255, 255, 0),
        12 => (0, 0, 255),
        13 => (255, 0, 255),
        14 => (0, 255, 255),
        15 => (255, 255, 255),
        16..=231 => {
            let c = i - 16;
            let r = c / 36;
            let g = (c % 36) / 6;
            let b = c % 6;
            let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            (level(r), level(g), level(b))
        }
        232..=255 => {
            let v = 8 + (i - 232) * 10;
            (v, v, v)
        }
    }
}

/// Visual attributes applied to a [`StyledSpan`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SpanStyle {
    /// Foreground color.
    pub fg: StyleColor,
    /// Background color.
    pub bg: StyleColor,
    /// Bold weight (SGR 1 / 22).
    pub bold: bool,
    /// Italic (SGR 3 / 23; markdown `em`).
    pub italic: bool,
    /// Underline (SGR 4 / 24; markdown h1).
    pub underline: bool,
}

/// What kind of content a span carries. Most spans are plain [`Text`].
/// [`CodePlaceholder`] marks a fenced code block that M7-02's syntect pass
/// replaces with highlighted spans — M7-01 never highlights.
///
/// [`Text`]: SpanKind::Text
/// [`CodePlaceholder`]: SpanKind::CodePlaceholder
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpanKind {
    /// Ordinary styled text.
    Text,
    /// Raw fenced-code content awaiting M7-02 highlighting. `lang` is the
    /// fence info-string (e.g. `rust`) when present.
    CodePlaceholder {
        /// Language hint from the fence info-string, if any.
        lang: Option<String>,
    },
}

/// One styled run of text. `text` never contains a newline — lines are
/// split into separate [`StyledLine`] values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyledSpan {
    /// UTF-8 text content of the run.
    pub text: String,
    /// Active style for this run.
    pub style: SpanStyle,
    /// Content classification.
    pub kind: SpanKind,
}

impl StyledSpan {
    /// A default-styled plain-text span.
    #[must_use]
    pub fn plain(text: impl Into<String>) -> Self {
        StyledSpan {
            text: text.into(),
            style: SpanStyle::default(),
            kind: SpanKind::Text,
        }
    }

    /// A styled plain-text span.
    #[must_use]
    pub fn styled(text: impl Into<String>, style: SpanStyle) -> Self {
        StyledSpan {
            text: text.into(),
            style,
            kind: SpanKind::Text,
        }
    }

    /// A code-fence placeholder span carrying the raw block text + lang hint.
    #[must_use]
    pub fn code_placeholder(text: impl Into<String>, lang: Option<&str>) -> Self {
        StyledSpan {
            text: text.into(),
            style: SpanStyle::default(),
            kind: SpanKind::CodePlaceholder {
                lang: lang.map(str::to_string),
            },
        }
    }
}

/// A single visual line: an ordered list of styled spans, no embedded `\n`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StyledLine {
    /// The spans composing this line, left to right.
    pub spans: Vec<StyledSpan>,
}

impl StyledLine {
    /// An empty line (used for spacing between block elements).
    #[must_use]
    pub fn empty() -> Self {
        StyledLine { spans: Vec::new() }
    }

    /// A line with a single default-styled span.
    #[must_use]
    pub fn plain(text: impl Into<String>) -> Self {
        StyledLine {
            spans: vec![StyledSpan::plain(text)],
        }
    }

    /// Concatenate every span's text (style-stripped). Useful for tests and
    /// width math.
    #[must_use]
    pub fn plain_text(&self) -> String {
        self.spans.iter().map(|s| s.text.as_str()).collect()
    }
}
```

- [ ] **Step 4: Wire the module so it compiles (temporary stub for submodules)**

`render/mod.rs` declares `pub mod ansi;` and `pub mod markdown;`, which do not exist yet. To compile Task 2 in isolation, create the two files as empty stubs now (they get real content in later tasks):

Create `lingxi-code/crates/tui/src/render/ansi.rs`:
```rust
//! ANSI parser — implemented in M7-01 Task 4/5.
```
Create `lingxi-code/crates/tui/src/render/markdown.rs`:
```rust
//! Markdown renderer — implemented in M7-01 Task 7/8.
```

Add to `lingxi-code/crates/tui/src/lib.rs`, immediately after the `pub mod permission_bridge;` line (keep `pub mod ansi;` for now — it is removed in Task 6):
```rust
pub mod render;
```

- [ ] **Step 5: Run test to verify it passes**

Run:
```bash
cd lingxi-core && cargo test -p lingxi-tui render:: 2>&1 | tail -20
```
Expected: PASS — the four tests in `render::mod::tests` pass.

- [ ] **Step 6: Commit**

```bash
cd lingxi-core && git add crates/tui/src/render/ crates/tui/src/lib.rs
git commit -m "$(cat <<'EOF'
plan(M7-01 T2): shared styled-line model (StyleColor/SpanStyle/StyledSpan/StyledLine)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Port the M6 ANSI parser to `render/ansi.rs` (16-color + StyledLine output)

**Files:**
- Modify: `lingxi-code/crates/tui/src/render/ansi.rs`
- Reference: `lingxi-code/crates/tui/src/ansi.rs` (the M6 original — do not delete yet)

**Context:** First move M6's parser into the new module, retargeting its output from `Vec<AnsiSpan>` to `Vec<StyledLine>` (splitting on `\n`) and its color model from `AnsiColor`/`AnsiStyle` to the new `StyleColor`/`SpanStyle`. 256/truecolor + cursor-skip come in Tasks 4/5; this task only re-establishes M6 parity under the new types so we have a green baseline.

- [ ] **Step 1: Write the failing test**

Replace the stub contents of `lingxi-code/crates/tui/src/render/ansi.rs` with the test module only (impl comes next step):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::{NamedColor, SpanKind, StyleColor};

    fn first_line(input: &str) -> StyledLine {
        parse_ansi(input).into_iter().next().unwrap_or_default()
    }

    #[test]
    fn red_err_then_reset() {
        let line = first_line("\x1b[31mERR\x1b[0m");
        assert_eq!(line.spans.len(), 1);
        assert_eq!(line.spans[0].text, "ERR");
        assert_eq!(line.spans[0].style.fg, StyleColor::Named(NamedColor::Red));
    }

    #[test]
    fn multi_param_sgr_bold_red() {
        let line = first_line("\x1b[1;31mhi\x1b[0m");
        assert_eq!(line.spans.len(), 1);
        assert_eq!(line.spans[0].text, "hi");
        assert_eq!(line.spans[0].style.fg, StyleColor::Named(NamedColor::Red));
        assert!(line.spans[0].style.bold);
    }

    #[test]
    fn empty_string_parses_to_empty_vec() {
        assert!(parse_ansi("").is_empty());
    }

    #[test]
    fn newline_splits_into_lines() {
        let lines = parse_ansi("a\nb");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].plain_text(), "a");
        assert_eq!(lines[1].plain_text(), "b");
    }

    #[test]
    fn all_spans_are_text_kind() {
        let line = first_line("\x1b[31mERR\x1b[0m");
        assert_eq!(line.spans[0].kind, SpanKind::Text);
    }

    #[test]
    fn malformed_unterminated_csi_does_not_panic() {
        let _ = parse_ansi("\x1b[31");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:
```bash
cd lingxi-core && cargo test -p lingxi-tui render::ansi 2>&1 | head -20
```
Expected: FAIL — `parse_ansi`, `StyledLine` import path resolved but `parse_ansi` undefined (compile error).

- [ ] **Step 3: Write the minimal implementation**

Prepend ABOVE the test module in `lingxi-code/crates/tui/src/render/ansi.rs`:

```rust
//! ANSI / SGR parser producing [`StyledLine`]s.
//!
//! Moved+expanded from M6's `src/ansi.rs`. M6 handled 8/16-color SGR +
//! reset/bold only. M7-01 adds 256-color (`38;5;N` / `48;5;N`), truecolor
//! (`38;2;r;g;b` / `48;2;r;g;b`), italic/underline attributes, and safely
//! skips cursor-move / erase CSI sequences (CUU/CUD/CUF/CUB/ED/EL) without
//! corrupting surrounding text. (256/truecolor + cursor-skip land in the
//! following tasks; this file first re-establishes M6 parity.)
//!
//! Operates on `&str` (UTF-8 already decoded). Output splits on `\n` into
//! one [`StyledLine`] per visual line. Never panics on malformed input.

use crate::render::{NamedColor, SpanStyle, StyleColor, StyledLine, StyledSpan};

/// Parse `input` into styled lines. Unsupported escape sequences are
/// silently skipped. Never panics.
#[must_use]
pub fn parse_ansi(input: &str) -> Vec<StyledLine> {
    if input.is_empty() {
        return Vec::new();
    }
    let mut lines: Vec<StyledLine> = Vec::new();
    let mut current: Vec<StyledSpan> = Vec::new();
    let mut style = SpanStyle::default();
    let mut buf = String::new();
    let bytes = input.as_bytes();
    let mut i = 0;

    // Flush `buf` into `current` as a span with the active style.
    macro_rules! flush_buf {
        () => {
            if !buf.is_empty() {
                current.push(StyledSpan::styled(std::mem::take(&mut buf), style));
            }
        };
    }

    while i < bytes.len() {
        let b = bytes[i];
        if b == b'\n' {
            flush_buf!();
            lines.push(StyledLine {
                spans: coalesce(std::mem::take(&mut current)),
            });
            i += 1;
            continue;
        }
        if b == 0x1b && i + 1 < bytes.len() {
            flush_buf!();
            let next = bytes[i + 1];
            if next == b'[' {
                // CSI — read params until a final byte in 0x40..=0x7E.
                let mut j = i + 2;
                let mut params = String::new();
                while j < bytes.len() {
                    let c = bytes[j];
                    if (0x40..=0x7E).contains(&c) {
                        break;
                    }
                    params.push(c as char);
                    j += 1;
                }
                if j >= bytes.len() {
                    // Unterminated CSI — drop it; flush what we have.
                    break;
                }
                let final_byte = bytes[j];
                if final_byte == b'm' {
                    apply_sgr(&params, &mut style);
                }
                // Else: non-SGR CSI (cursor/erase/mode) — skipped (Task 5
                // makes the skip explicit + tested).
                i = j + 1;
                continue;
            } else if next == b']' {
                // OSC — read to BEL (0x07) or ST (ESC \).
                let mut j = i + 2;
                while j < bytes.len() {
                    if bytes[j] == 0x07 {
                        j += 1;
                        break;
                    }
                    if bytes[j] == 0x1b && j + 1 < bytes.len() && bytes[j + 1] == b'\\' {
                        j += 2;
                        break;
                    }
                    j += 1;
                }
                i = j;
                continue;
            }
            // Other ESC-prefixed sequence (ESC c, ESC =, …). Skip 2 bytes.
            i += 2;
            continue;
        }
        buf.push(b as char);
        i += 1;
    }

    flush_buf!();
    if !current.is_empty() {
        lines.push(StyledLine {
            spans: coalesce(current),
        });
    }
    lines
}

/// Merge adjacent spans with identical style + kind so skipped escape
/// sequences (which split a run) collapse back into one span.
fn coalesce(spans: Vec<StyledSpan>) -> Vec<StyledSpan> {
    let mut out: Vec<StyledSpan> = Vec::with_capacity(spans.len());
    for span in spans {
        match out.last_mut() {
            Some(prev) if prev.style == span.style && prev.kind == span.kind => {
                prev.text.push_str(&span.text);
            }
            _ => out.push(span),
        }
    }
    out
}

/// Apply a semicolon-separated SGR parameter string to `style`. Empty
/// params (`\x1b[m`) reset. Unsupported codes are ignored. (256/truecolor
/// extended forms land in Task 4.)
fn apply_sgr(params: &str, style: &mut SpanStyle) {
    if params.is_empty() {
        *style = SpanStyle::default();
        return;
    }
    for tok in params.split(';') {
        let n: u16 = tok.parse().unwrap_or(0);
        match n {
            0 => *style = SpanStyle::default(),
            1 => style.bold = true,
            3 => style.italic = true,
            4 => style.underline = true,
            22 => style.bold = false,
            23 => style.italic = false,
            24 => style.underline = false,
            30 => style.fg = StyleColor::Named(NamedColor::Black),
            31 => style.fg = StyleColor::Named(NamedColor::Red),
            32 => style.fg = StyleColor::Named(NamedColor::Green),
            33 => style.fg = StyleColor::Named(NamedColor::Yellow),
            34 => style.fg = StyleColor::Named(NamedColor::Blue),
            35 => style.fg = StyleColor::Named(NamedColor::Magenta),
            36 => style.fg = StyleColor::Named(NamedColor::Cyan),
            37 => style.fg = StyleColor::Named(NamedColor::White),
            39 => style.fg = StyleColor::Default,
            40 => style.bg = StyleColor::Named(NamedColor::Black),
            41 => style.bg = StyleColor::Named(NamedColor::Red),
            42 => style.bg = StyleColor::Named(NamedColor::Green),
            43 => style.bg = StyleColor::Named(NamedColor::Yellow),
            44 => style.bg = StyleColor::Named(NamedColor::Blue),
            45 => style.bg = StyleColor::Named(NamedColor::Magenta),
            46 => style.bg = StyleColor::Named(NamedColor::Cyan),
            47 => style.bg = StyleColor::Named(NamedColor::White),
            49 => style.bg = StyleColor::Default,
            90 => style.fg = StyleColor::Named(NamedColor::BrightBlack),
            91 => style.fg = StyleColor::Named(NamedColor::BrightRed),
            92 => style.fg = StyleColor::Named(NamedColor::BrightGreen),
            93 => style.fg = StyleColor::Named(NamedColor::BrightYellow),
            94 => style.fg = StyleColor::Named(NamedColor::BrightBlue),
            95 => style.fg = StyleColor::Named(NamedColor::BrightMagenta),
            96 => style.fg = StyleColor::Named(NamedColor::BrightCyan),
            97 => style.fg = StyleColor::Named(NamedColor::BrightWhite),
            100 => style.bg = StyleColor::Named(NamedColor::BrightBlack),
            101 => style.bg = StyleColor::Named(NamedColor::BrightRed),
            102 => style.bg = StyleColor::Named(NamedColor::BrightGreen),
            103 => style.bg = StyleColor::Named(NamedColor::BrightYellow),
            104 => style.bg = StyleColor::Named(NamedColor::BrightBlue),
            105 => style.bg = StyleColor::Named(NamedColor::BrightMagenta),
            106 => style.bg = StyleColor::Named(NamedColor::BrightCyan),
            107 => style.bg = StyleColor::Named(NamedColor::BrightWhite),
            _ => { /* unsupported / extended (38/48) — handled in Task 4 */ }
        }
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run:
```bash
cd lingxi-core && cargo test -p lingxi-tui render::ansi 2>&1 | tail -20
```
Expected: PASS — all six tests pass.

- [ ] **Step 5: Commit**

```bash
cd lingxi-core && git add crates/tui/src/render/ansi.rs
git commit -m "$(cat <<'EOF'
plan(M7-01 T3): port M6 ANSI parser to render/ansi.rs (StyledLine output)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Add 256-color and truecolor SGR parsing

**Files:**
- Modify: `lingxi-code/crates/tui/src/render/ansi.rs`

**Context:** Extended SGR colors use a sub-sequence: `38;5;N` (256-color fg), `48;5;N` (256-color bg), `38;2;r;g;b` (truecolor fg), `48;2;r;g;b` (truecolor bg). These cannot be handled by the per-token `match` in Task 3 because they consume multiple following tokens. We rewrite `apply_sgr` to walk the parameter list with an index so `38`/`48` can pull their sub-parameters.

- [ ] **Step 1: Write the failing test**

Add these tests inside the existing `mod tests` block in `render/ansi.rs`:

```rust
    #[test]
    fn fg_256_color_indexed() {
        // 38;5;196 = bright red in the 256 palette.
        let line = first_line("\x1b[38;5;196mX\x1b[0m");
        assert_eq!(line.spans[0].text, "X");
        assert_eq!(line.spans[0].style.fg, StyleColor::Indexed(196));
    }

    #[test]
    fn bg_256_color_indexed() {
        let line = first_line("\x1b[48;5;21mX\x1b[0m");
        assert_eq!(line.spans[0].style.bg, StyleColor::Indexed(21));
    }

    #[test]
    fn fg_truecolor_rgb() {
        let line = first_line("\x1b[38;2;10;20;30mX\x1b[0m");
        assert_eq!(line.spans[0].style.fg, StyleColor::Rgb(10, 20, 30));
    }

    #[test]
    fn bg_truecolor_rgb() {
        let line = first_line("\x1b[48;2;200;100;50mX\x1b[0m");
        assert_eq!(line.spans[0].style.bg, StyleColor::Rgb(200, 100, 50));
    }

    #[test]
    fn truecolor_mixed_with_bold() {
        // bold + truecolor fg in one SGR.
        let line = first_line("\x1b[1;38;2;1;2;3mX\x1b[0m");
        assert!(line.spans[0].style.bold);
        assert_eq!(line.spans[0].style.fg, StyleColor::Rgb(1, 2, 3));
    }

    #[test]
    fn truncated_256_sequence_does_not_panic() {
        // 38;5 with no index — must not panic, must not corrupt.
        let _ = first_line("\x1b[38;5mtail");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run:
```bash
cd lingxi-core && cargo test -p lingxi-tui render::ansi::tests::fg_256_color_indexed render::ansi::tests::fg_truecolor_rgb 2>&1 | tail -20
```
Expected: FAIL — assertions fail (the `38`/`48` tokens currently fall into the ignored `_` arm, leaving fg/bg at `Default`).

- [ ] **Step 3: Replace `apply_sgr` with an index-walking version**

Replace the ENTIRE `fn apply_sgr(...)` body in `render/ansi.rs` with:

```rust
/// Apply a semicolon-separated SGR parameter string to `style`. Walks the
/// parameter list by index so the extended `38`/`48` color introducers can
/// pull their `5;N` (indexed) or `2;r;g;b` (truecolor) sub-parameters.
/// Empty params reset. Unsupported / truncated codes are ignored without
/// panicking.
fn apply_sgr(params: &str, style: &mut SpanStyle) {
    if params.is_empty() {
        *style = SpanStyle::default();
        return;
    }
    let parts: Vec<u16> = params.split(';').map(|t| t.parse().unwrap_or(0)).collect();
    let mut idx = 0;
    while idx < parts.len() {
        let n = parts[idx];
        match n {
            0 => *style = SpanStyle::default(),
            1 => style.bold = true,
            3 => style.italic = true,
            4 => style.underline = true,
            22 => style.bold = false,
            23 => style.italic = false,
            24 => style.underline = false,
            30..=37 => style.fg = StyleColor::Named(named_from_offset(n - 30)),
            39 => style.fg = StyleColor::Default,
            40..=47 => style.bg = StyleColor::Named(named_from_offset(n - 40)),
            49 => style.bg = StyleColor::Default,
            90..=97 => style.fg = StyleColor::Named(bright_from_offset(n - 90)),
            100..=107 => style.bg = StyleColor::Named(bright_from_offset(n - 100)),
            38 => {
                if let Some(color) = parse_extended_color(&parts, &mut idx) {
                    style.fg = color;
                }
            }
            48 => {
                if let Some(color) = parse_extended_color(&parts, &mut idx) {
                    style.bg = color;
                }
            }
            _ => { /* unsupported code — ignore */ }
        }
        idx += 1;
    }
}

/// Parse the sub-parameters of a `38`/`48` introducer. `idx` points at the
/// introducer (`38`/`48`); on success it is advanced past the consumed
/// sub-parameters. Returns `None` (consuming nothing extra) on a truncated
/// or unrecognized form.
fn parse_extended_color(parts: &[u16], idx: &mut usize) -> Option<StyleColor> {
    match parts.get(*idx + 1) {
        Some(5) => {
            let n = *parts.get(*idx + 2)?;
            *idx += 2;
            Some(StyleColor::Indexed(n as u8))
        }
        Some(2) => {
            let r = *parts.get(*idx + 2)?;
            let g = *parts.get(*idx + 3)?;
            let b = *parts.get(*idx + 4)?;
            *idx += 4;
            Some(StyleColor::Rgb(r as u8, g as u8, b as u8))
        }
        _ => None,
    }
}

/// Map an offset 0..=7 to the standard named color (SGR 30..=37 / 40..=47).
fn named_from_offset(off: u16) -> NamedColor {
    match off {
        0 => NamedColor::Black,
        1 => NamedColor::Red,
        2 => NamedColor::Green,
        3 => NamedColor::Yellow,
        4 => NamedColor::Blue,
        5 => NamedColor::Magenta,
        6 => NamedColor::Cyan,
        _ => NamedColor::White,
    }
}

/// Map an offset 0..=7 to the bright named color (SGR 90..=97 / 100..=107).
fn bright_from_offset(off: u16) -> NamedColor {
    match off {
        0 => NamedColor::BrightBlack,
        1 => NamedColor::BrightRed,
        2 => NamedColor::BrightGreen,
        3 => NamedColor::BrightYellow,
        4 => NamedColor::BrightBlue,
        5 => NamedColor::BrightMagenta,
        6 => NamedColor::BrightCyan,
        _ => NamedColor::BrightWhite,
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run:
```bash
cd lingxi-core && cargo test -p lingxi-tui render::ansi 2>&1 | tail -20
```
Expected: PASS — all Task 3 tests plus the six new extended-color tests pass.

- [ ] **Step 5: Commit**

```bash
cd lingxi-core && git add crates/tui/src/render/ansi.rs
git commit -m "$(cat <<'EOF'
plan(M7-01 T4): parse 256-color (38;5;N) and truecolor (38;2;r;g;b) SGR

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Verify cursor-move / erase sequences are skipped + ANSI insta snapshots

**Files:**
- Modify: `lingxi-code/crates/tui/src/render/ansi.rs` (tests only)
- Create: `lingxi-code/crates/tui/src/snapshots/` insta files (auto-generated by `cargo insta`)

**Context:** CUU/CUD/CUF/CUB (cursor up/down/forward/back, final bytes `A`/`B`/`C`/`D`), ED (erase display, `2J`), EL (erase line, `K`) are non-SGR CSI sequences. The Task 3 parser already drops them (only `m`-final CSI applies SGR), but the spec requires explicit, tested proof they are consumed without corrupting text. We also lock the ANSI output shape with insta snapshots (spec §5.2 budget: ANSI 6+ snapshots).

- [ ] **Step 1: Write the cursor-skip behavior tests**

Add inside the `mod tests` block in `render/ansi.rs`:

```rust
    #[test]
    fn cursor_up_is_skipped_without_corruption() {
        // CUU (cursor up) between two letters — text must remain "ab".
        let line = first_line("a\x1b[3Ab");
        assert_eq!(line.plain_text(), "ab");
        assert_eq!(line.spans.len(), 1);
    }

    #[test]
    fn cursor_forward_back_skipped() {
        let line = first_line("a\x1b[2Cb\x1b[1Dc");
        assert_eq!(line.plain_text(), "abc");
    }

    #[test]
    fn erase_display_skipped() {
        let line = first_line("x\x1b[2Jy");
        assert_eq!(line.plain_text(), "xy");
    }

    #[test]
    fn erase_line_skipped() {
        let line = first_line("x\x1b[Ky");
        assert_eq!(line.plain_text(), "xy");
    }

    #[test]
    fn cursor_skip_preserves_surrounding_color() {
        // Color set, cursor move, then text — color must still apply.
        let line = first_line("\x1b[31m\x1b[2Aerr\x1b[0m");
        assert_eq!(line.plain_text(), "err");
        assert_eq!(line.spans[0].style.fg, StyleColor::Named(NamedColor::Red));
    }

    #[test]
    fn osc_title_skipped() {
        let line = first_line("a\x1b]0;title\x07b");
        assert_eq!(line.plain_text(), "ab");
    }
```

- [ ] **Step 2: Add the snapshot tests**

Add inside the `mod tests` block in `render/ansi.rs`:

```rust
    #[test]
    fn snapshot_16_color() {
        insta::assert_yaml_snapshot!(parse_ansi("\x1b[31mERR\x1b[0m\x1b[1mB\x1b[0m tail"));
    }

    #[test]
    fn snapshot_256_color() {
        insta::assert_yaml_snapshot!(parse_ansi("\x1b[38;5;196mhot\x1b[48;5;21m bg\x1b[0m"));
    }

    #[test]
    fn snapshot_truecolor() {
        insta::assert_yaml_snapshot!(parse_ansi("\x1b[38;2;255;128;0morange\x1b[0m"));
    }

    #[test]
    fn snapshot_reset_midline() {
        insta::assert_yaml_snapshot!(parse_ansi("\x1b[1;32mok\x1b[0m done"));
    }

    #[test]
    fn snapshot_malformed_unterminated() {
        insta::assert_yaml_snapshot!(parse_ansi("before\x1b[38;5"));
    }

    #[test]
    fn snapshot_cursor_move_skipped() {
        insta::assert_yaml_snapshot!(parse_ansi("\x1b[31ma\x1b[2A\x1b[2Jb\x1b[0m"));
    }
```

- [ ] **Step 3: Run tests; review and accept the snapshots**

Run:
```bash
cd lingxi-core && cargo insta test --review -p lingxi-tui --accept 2>&1 | tail -25
```
Expected: the six behavior tests PASS immediately; the six `snapshot_*` tests generate new snapshot files under `crates/tui/src/snapshots/` and are accepted. Verify each accepted snapshot is sane (e.g. `snapshot_256_color` shows `Indexed: 196` for fg and `Indexed: 21` for bg; `snapshot_truecolor` shows `Rgb: [255, 128, 0]`; `snapshot_cursor_move_skipped` shows a single span `"ab"` with red fg).

- [ ] **Step 4: Run the full ansi test set to confirm green**

Run:
```bash
cd lingxi-core && cargo test -p lingxi-tui render::ansi 2>&1 | tail -10
```
Expected: PASS — all behavior + snapshot tests green.

- [ ] **Step 5: Commit**

```bash
cd lingxi-core && git add crates/tui/src/render/ansi.rs crates/tui/src/snapshots/
git commit -m "$(cat <<'EOF'
plan(M7-01 T5): test cursor/erase skip + lock ANSI insta snapshots

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: Migrate `user_tool_result.rs` off `crate::ansi`, delete the old module

**Files:**
- Modify: `lingxi-code/crates/tui/src/components/messages/user_tool_result.rs`
- Modify: `lingxi-code/crates/tui/src/lib.rs`
- Delete: `lingxi-code/crates/tui/src/ansi.rs`

**Context:** `user_tool_result.rs` is the only consumer of the old `crate::ansi`. It imports `parse_ansi`, `AnsiColor`, `AnsiSpan`, `AnsiStyle` and has its own `ansi_to_iocraft_color` mapper + `render_user_tool_result_body_spans`. The new `parse_ansi` returns `Vec<StyledLine>` (not `Vec<AnsiSpan>`) and colors are `StyleColor` (mapped via `StyleColor::to_iocraft`, replacing the local mapper). We rewrite the body-span pipeline to flatten the styled lines back into spans (the existing iocraft rendering iterates spans in a single `Row`, so flatten-with-newlines preserves behavior for the single-line Bash-output case M6 tested).

- [ ] **Step 1: Read the current consumer to confirm the migration surface**

Run:
```bash
cd lingxi-core && grep -n "crate::ansi\|AnsiSpan\|AnsiStyle\|AnsiColor\|ansi_to_iocraft_color\|render_user_tool_result_body_spans" crates/tui/src/components/messages/user_tool_result.rs
```
Expected: the import line (`use crate::ansi::{parse_ansi, AnsiColor, AnsiSpan};`), the secondary `use crate::ansi::AnsiStyle;`, the `render_user_tool_result_body_spans` fn (returns `Vec<AnsiSpan>`), the `ansi_to_iocraft_color` fn, and the `UserToolResultMessage` component body that maps spans → iocraft `Text`.

- [ ] **Step 2: Rewrite the import + body-span pipeline**

In `user_tool_result.rs`, replace the import line:
```rust
use crate::ansi::{parse_ansi, AnsiColor, AnsiSpan};
```
with:
```rust
use crate::render::ansi::parse_ansi;
use crate::render::StyledSpan;
```

Replace the secondary import `use crate::ansi::AnsiStyle;` (delete that line entirely — no longer needed).

Replace the `render_user_tool_result_body_spans` function with:
```rust
/// Produce the styled spans for the body. Only Bash output runs through the
/// ANSI parser; everything else is a single default-styled span over the
/// result body text. (Migrated to `render::ansi` in M7-01.)
///
/// The ANSI parser returns one `StyledLine` per visual line; this flattens
/// them into a single span vector, re-inserting `\n` between lines so the
/// existing single-`Row` renderer reproduces M6 behavior for one-line Bash
/// output (the only case M6 exercised).
#[must_use]
pub fn render_user_tool_result_body_spans(props: &UserToolResultProps) -> Vec<StyledSpan> {
    let body = body_text(&props.result);
    let (truncated, _dropped) = truncate(&body);
    if props.tool == "Bash" {
        let lines = parse_ansi(&truncated);
        let mut spans: Vec<StyledSpan> = Vec::new();
        for (li, line) in lines.into_iter().enumerate() {
            if li > 0 {
                spans.push(StyledSpan::plain("\n"));
            }
            spans.extend(line.spans);
        }
        spans
    } else {
        vec![StyledSpan::plain(truncated)]
    }
}
```

- [ ] **Step 3: Delete the local color mapper; use `StyleColor::to_iocraft`**

Delete the entire `fn ansi_to_iocraft_color(c: AnsiColor) -> Color { … }` function and its doc comment.

In the `UserToolResultMessage` component body, find the span-mapping closure that currently does:
```rust
            .map(|s| {
                let color = ansi_to_iocraft_color(s.style.fg);
                let weight = if s.style.bold {
                    Weight::Bold
                } else {
                    Weight::Normal
                };
                element! {
                    Text(content: s.text, color: color, weight: weight)
                }
                .into_any()
            })
```
and replace it with:
```rust
            .map(|s| {
                let color = s.style.fg.to_iocraft();
                let weight = if s.style.bold {
                    Weight::Bold
                } else {
                    Weight::Normal
                };
                element! {
                    Text(content: s.text, color: color, weight: weight)
                }
                .into_any()
            })
```

- [ ] **Step 4: Delete the old module + its declaration**

Delete the file:
```bash
cd lingxi-core && git rm crates/tui/src/ansi.rs
```

In `lingxi-code/crates/tui/src/lib.rs`, delete the line:
```rust
pub mod ansi;
```

- [ ] **Step 5: Build + run the affected tests**

Run:
```bash
cd lingxi-core && cargo test -p lingxi-tui user_tool_result 2>&1 | tail -20
```
Expected: PASS — the M6 `user_tool_result` tests still pass against the new API. If any snapshot test for this component shifts (it should not — output text + colors are equivalent), inspect the diff; an unintended change is a bug to fix, not accept.

- [ ] **Step 6: Confirm no dangling references to the old module**

Run:
```bash
cd lingxi-core && grep -rn "crate::ansi\|AnsiSpan\|AnsiStyle\|AnsiColor" crates/tui/src/ || echo "CLEAN"
```
Expected: `CLEAN` — no references to the deleted module remain.

- [ ] **Step 7: Commit**

```bash
cd lingxi-core && git add crates/tui/src/components/messages/user_tool_result.rs crates/tui/src/lib.rs
git add -u crates/tui/src/ansi.rs
git commit -m "$(cat <<'EOF'
plan(M7-01 T6): migrate user_tool_result to render::ansi; delete old ansi module

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: Markdown — theme + inline elements (bold, italic, inline code, links)

**Files:**
- Modify: `lingxi-code/crates/tui/src/render/markdown.rs`

**Context:** `render::markdown::render(text, theme) -> Vec<StyledLine>` walks `pulldown-cmark` events. This task lands the `MarkdownTheme` input struct and the inline elements: `Strong` → bold, `Emphasis` → italic, `Code` (inline) → `theme.inline_code` color, `Link` → display text + ` (url)` suffix, plain `Text`. Reference `claude-code/src/utils/markdown.ts`: `strong`→`chalk.bold`, `em`→`chalk.italic`, inline `codespan`→`color('permission', theme)` (a theme color — we expose it as `theme.inline_code`), link→clickable text (we render `text (url)` since the TUI cannot emit OSC 8 hyperlinks in a `StyledLine`). The theme is a plain color struct, NOT iocraft `Color` (those map at render time via `StyleColor::to_iocraft`), so markdown stays a pure value function testable without a terminal.

- [ ] **Step 1: Write the failing test**

Replace the stub contents of `render/markdown.rs` with the test module only:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::{SpanStyle, StyleColor};

    fn theme() -> MarkdownTheme {
        MarkdownTheme {
            inline_code: StyleColor::Named(crate::render::NamedColor::Magenta),
        }
    }

    #[test]
    fn plain_paragraph() {
        let lines = render("hello world", &theme());
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].plain_text(), "hello world");
        assert_eq!(lines[0].spans[0].style, SpanStyle::default());
    }

    #[test]
    fn bold_text() {
        let lines = render("a **b** c", &theme());
        // "a " | "b"(bold) | " c"
        let bold_span = lines[0].spans.iter().find(|s| s.text == "b").unwrap();
        assert!(bold_span.style.bold);
    }

    #[test]
    fn italic_text() {
        let lines = render("a *b* c", &theme());
        let it = lines[0].spans.iter().find(|s| s.text == "b").unwrap();
        assert!(it.style.italic);
    }

    #[test]
    fn inline_code_uses_theme_color() {
        let lines = render("run `cargo test` now", &theme());
        let code = lines[0].spans.iter().find(|s| s.text == "cargo test").unwrap();
        assert_eq!(code.style.fg, StyleColor::Named(crate::render::NamedColor::Magenta));
    }

    #[test]
    fn link_renders_text_and_url() {
        let lines = render("see [docs](https://x.io)", &theme());
        let joined = lines[0].plain_text();
        assert!(joined.contains("docs"));
        assert!(joined.contains("https://x.io"));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:
```bash
cd lingxi-core && cargo test -p lingxi-tui render::markdown 2>&1 | head -20
```
Expected: FAIL — `render`, `MarkdownTheme` undefined (compile error).

- [ ] **Step 3: Write the minimal implementation**

Prepend ABOVE the test module in `render/markdown.rs`:

```rust
//! CommonMark → [`StyledLine`] rendering via `pulldown-cmark`.
//!
//! Pure function: `render(text, theme) -> Vec<StyledLine>`. No iocraft, no
//! terminal, no async — colors are [`StyleColor`] values mapped to iocraft
//! only at draw time (`StyleColor::to_iocraft`).
//!
//! Literal reference: `claude-code/src/utils/markdown.ts` `formatToken`.
//! Handled here: paragraphs, headings, bold (`Strong`), italic (`Emphasis`),
//! inline code (`Code` → `theme.inline_code`), links (`text (url)`),
//! ordered/unordered/nested lists, blockquote, fenced code (emits a
//! [`SpanKind::CodePlaceholder`] span — M7-02 highlights it). Best-effort on
//! partial / unclosed input; never panics.
//!
//! Per claude-code: strikethrough is intentionally NOT parsed (the model
//! uses `~` for "approximately"); HTML/definitions render to nothing.

use crate::render::{SpanStyle, StyleColor, StyledLine, StyledSpan};
use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

/// Theme colors the markdown renderer needs. Kept minimal and decoupled
/// from iocraft so the renderer is a pure value function. Expand in M7-15.
#[derive(Debug, Clone, Copy)]
pub struct MarkdownTheme {
    /// Inline-code (`codespan`) foreground — claude-code uses the
    /// `permission` theme color here.
    pub inline_code: StyleColor,
}

/// Mutable inline styling state threaded through the event walk.
#[derive(Debug, Clone, Copy, Default)]
struct InlineState {
    bold: bool,
    italic: bool,
    underline: bool,
    code: bool,
}

impl InlineState {
    fn to_style(self, theme: &MarkdownTheme) -> SpanStyle {
        SpanStyle {
            fg: if self.code {
                theme.inline_code
            } else {
                StyleColor::Default
            },
            bg: StyleColor::Default,
            bold: self.bold,
            italic: self.italic,
            underline: self.underline,
        }
    }
}

/// Render CommonMark `text` to styled lines using `theme`. Strikethrough is
/// disabled to match claude-code; tables and footnotes are enabled by
/// `pulldown-cmark` defaults but only paragraph/heading/list/quote/code are
/// styled here (others fall through as their inline text).
#[must_use]
pub fn render(text: &str, theme: &MarkdownTheme) -> Vec<StyledLine> {
    let options = Options::ENABLE_TABLES;
    let parser = Parser::new_ext(text, options);

    let mut builder = Builder::new(theme);
    for event in parser {
        builder.handle(event);
    }
    builder.finish()
}

/// Accumulates styled lines while walking markdown events. Inline content is
/// appended to `pending` (the current line being built); block boundaries
/// flush `pending` into `lines`.
struct Builder<'a> {
    theme: &'a MarkdownTheme,
    lines: Vec<StyledLine>,
    pending: Vec<StyledSpan>,
    inline: InlineState,
}

impl<'a> Builder<'a> {
    fn new(theme: &'a MarkdownTheme) -> Self {
        Builder {
            theme,
            lines: Vec::new(),
            pending: Vec::new(),
            inline: InlineState::default(),
        }
    }

    /// Flush the in-progress line (if any) into `lines`.
    fn flush(&mut self) {
        if !self.pending.is_empty() {
            self.lines.push(StyledLine {
                spans: std::mem::take(&mut self.pending),
            });
        }
    }

    /// Append a styled-text span to the current line using current inline
    /// state.
    fn push_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.pending
            .push(StyledSpan::styled(text, self.inline.to_style(self.theme)));
    }

    fn handle(&mut self, event: Event<'_>) {
        match event {
            Event::Start(Tag::Strong) => self.inline.bold = true,
            Event::End(TagEnd::Strong) => self.inline.bold = false,
            Event::Start(Tag::Emphasis) => self.inline.italic = true,
            Event::End(TagEnd::Emphasis) => self.inline.italic = false,
            Event::Code(text) => {
                // inline code: force code color regardless of surrounding em.
                let prev = self.inline.code;
                self.inline.code = true;
                self.push_text(&text);
                self.inline.code = prev;
            }
            Event::Start(Tag::Link { dest_url, .. }) => {
                // Remember the URL to append after the link text closes.
                self.link_url = Some(dest_url.to_string());
            }
            Event::End(TagEnd::Link) => {
                if let Some(url) = self.link_url.take() {
                    self.push_text(&format!(" ({url})"));
                }
            }
            Event::Text(text) => self.push_text(&text),
            Event::SoftBreak | Event::HardBreak => {
                self.flush();
            }
            Event::End(TagEnd::Paragraph) => {
                self.flush();
                self.lines.push(StyledLine::empty());
            }
            _ => { /* block elements handled in Task 8 */ }
        }
    }

    fn finish(mut self) -> Vec<StyledLine> {
        self.flush();
        // Drop a trailing blank line for tidy output.
        if matches!(self.lines.last(), Some(l) if l.spans.is_empty()) {
            self.lines.pop();
        }
        self.lines
    }
}
```

Add the `link_url` field to `Builder` — update the struct definition and `new`:
- In `struct Builder<'a>`, add field after `inline: InlineState,`:
```rust
    link_url: Option<String>,
```
- In `Builder::new`, add to the struct literal after `inline: InlineState::default(),`:
```rust
            link_url: None,
```

- [ ] **Step 4: Run test to verify it passes**

Run:
```bash
cd lingxi-core && cargo test -p lingxi-tui render::markdown 2>&1 | tail -20
```
Expected: PASS — the five inline tests pass.

- [ ] **Step 5: Commit**

```bash
cd lingxi-core && git add crates/tui/src/render/markdown.rs
git commit -m "$(cat <<'EOF'
plan(M7-01 T7): markdown inline elements (bold/italic/inline-code/link)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: Markdown — block elements (headings, lists, blockquote)

**Files:**
- Modify: `lingxi-code/crates/tui/src/render/markdown.rs`

**Context:** Block elements per `claude-code/src/utils/markdown.ts`: h1 = bold+italic+underline, h2/h3+ = bold (we apply bold+underline for h1, bold for the rest, mirroring the visual weight; claude-code's h1 adds italic too — we set bold+italic+underline for h1, bold for h2+); unordered list item prefix `-`, ordered `N.`; nested lists indent two spaces per depth; blockquote prefixes each non-blank line with `│ ` (claude-code's `BLOCKQUOTE_BAR`) in dim + italic. We extend `Builder` with heading/list/quote state and handle the corresponding `Tag`/`TagEnd` events.

- [ ] **Step 1: Write the failing test**

Add inside the `mod tests` block in `render/markdown.rs`:

```rust
    #[test]
    fn h1_is_bold_italic_underline() {
        let lines = render("# Title", &theme());
        let span = &lines[0].spans[0];
        assert_eq!(span.text, "Title");
        assert!(span.style.bold);
        assert!(span.style.italic);
        assert!(span.style.underline);
    }

    #[test]
    fn h2_is_bold_only() {
        let lines = render("## Sub", &theme());
        let span = &lines[0].spans[0];
        assert!(span.style.bold);
        assert!(!span.style.italic);
        assert!(!span.style.underline);
    }

    #[test]
    fn unordered_list_marker() {
        let lines = render("- one\n- two", &theme());
        let texts: Vec<String> = lines.iter().map(|l| l.plain_text()).collect();
        assert!(texts.iter().any(|t| t == "- one"));
        assert!(texts.iter().any(|t| t == "- two"));
    }

    #[test]
    fn ordered_list_marker() {
        let lines = render("1. first\n2. second", &theme());
        let texts: Vec<String> = lines.iter().map(|l| l.plain_text()).collect();
        assert!(texts.iter().any(|t| t == "1. first"));
        assert!(texts.iter().any(|t| t == "2. second"));
    }

    #[test]
    fn nested_list_indents() {
        let lines = render("- a\n  - b", &theme());
        let texts: Vec<String> = lines.iter().map(|l| l.plain_text()).collect();
        assert!(texts.iter().any(|t| t == "- a"));
        assert!(texts.iter().any(|t| t == "  - b"));
    }

    #[test]
    fn blockquote_has_bar_prefix() {
        let lines = render("> quoted", &theme());
        let line = lines.iter().find(|l| l.plain_text().contains("quoted")).unwrap();
        assert!(line.plain_text().starts_with("│ "));
        let text_span = line.spans.iter().find(|s| s.text.contains("quoted")).unwrap();
        assert!(text_span.style.italic);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run:
```bash
cd lingxi-core && cargo test -p lingxi-tui render::markdown::tests::h1_is_bold_italic_underline render::markdown::tests::unordered_list_marker render::markdown::tests::blockquote_has_bar_prefix 2>&1 | tail -20
```
Expected: FAIL — block tags currently fall into the `_` arm, so headings/lists/quotes render as plain inline text without markers/styles.

- [ ] **Step 3: Extend `Builder` with block state + the blockquote constant**

In `render/markdown.rs`, add the constant near the top (after the `use` lines):
```rust
/// Dim vertical bar prefixing blockquote lines. Matches claude-code's
/// `BLOCKQUOTE_BAR` (`src/constants/figures.ts`).
const BLOCKQUOTE_BAR: &str = "│";
```

Extend `struct Builder<'a>` with block-tracking fields (add after `link_url: Option<String>,`):
```rust
    /// Active heading level, set between `Start(Heading)`/`End(Heading)`.
    heading: Option<HeadingLevel>,
    /// Stack of list contexts (outer to inner). `Some(n)` = ordered list at
    /// next item number `n`; `None` = unordered.
    list_stack: Vec<Option<u64>>,
    /// True while inside a blockquote (prefix lines with the bar + italic).
    in_blockquote: bool,
```
And in `Builder::new`, add to the struct literal:
```rust
            heading: None,
            list_stack: Vec::new(),
            in_blockquote: false,
```

- [ ] **Step 4: Handle the block events**

Replace the `_ => { /* block elements handled in Task 8 */ }` arm in `Builder::handle` with the following arms (insert them BEFORE the final `_ =>` catch-all; keep a `_ => {}` at the end):

```rust
            Event::Start(Tag::Heading { level, .. }) => {
                self.flush();
                self.heading = Some(level);
                match level {
                    HeadingLevel::H1 => {
                        self.inline.bold = true;
                        self.inline.italic = true;
                        self.inline.underline = true;
                    }
                    _ => self.inline.bold = true,
                }
            }
            Event::End(TagEnd::Heading(_)) => {
                self.flush();
                self.inline = InlineState::default();
                self.heading = None;
                self.lines.push(StyledLine::empty());
            }
            Event::Start(Tag::List(first)) => {
                self.list_stack.push(first);
            }
            Event::End(TagEnd::List(_)) => {
                self.list_stack.pop();
            }
            Event::Start(Tag::Item) => {
                self.flush();
                let depth = self.list_stack.len().saturating_sub(1);
                let indent = "  ".repeat(depth);
                let marker = match self.list_stack.last_mut() {
                    Some(Some(n)) => {
                        let m = format!("{n}. ");
                        *n += 1;
                        m
                    }
                    _ => "- ".to_string(),
                };
                self.pending
                    .push(StyledSpan::plain(format!("{indent}{marker}")));
            }
            Event::End(TagEnd::Item) => {
                self.flush();
            }
            Event::Start(Tag::BlockQuote(_)) => {
                self.in_blockquote = true;
                self.inline.italic = true;
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                self.in_blockquote = false;
                self.inline.italic = false;
            }
```

Now make blockquote lines carry the bar prefix. Update `flush` to prepend the bar when in a blockquote and the line has visible content:
```rust
    /// Flush the in-progress line (if any) into `lines`.
    fn flush(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        let mut spans = std::mem::take(&mut self.pending);
        if self.in_blockquote {
            let mut prefixed = vec![StyledSpan::styled(
                format!("{BLOCKQUOTE_BAR} "),
                SpanStyle {
                    fg: StyleColor::Named(crate::render::NamedColor::BrightBlack),
                    ..SpanStyle::default()
                },
            )];
            prefixed.append(&mut spans);
            spans = prefixed;
        }
        self.lines.push(StyledLine { spans });
    }
```

- [ ] **Step 5: Run test to verify it passes**

Run:
```bash
cd lingxi-core && cargo test -p lingxi-tui render::markdown 2>&1 | tail -20
```
Expected: PASS — all Task 7 inline tests plus the six block tests pass.

- [ ] **Step 6: Commit**

```bash
cd lingxi-core && git add crates/tui/src/render/markdown.rs
git commit -m "$(cat <<'EOF'
plan(M7-01 T8): markdown blocks (headings/lists/nested/blockquote)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 9: Markdown — fenced code emits a `CodePlaceholder` span

**Files:**
- Modify: `lingxi-code/crates/tui/src/render/markdown.rs`

**Context:** Fenced code blocks MUST NOT be highlighted here (M7-02 does that). M7-01 emits one span tagged `SpanKind::CodePlaceholder { lang }` carrying the raw block text + the fence info-string language. Per `claude-code/src/utils/markdown.ts` `code` case: with no highlighter the block is returned verbatim — our placeholder IS that verbatim-with-deferred-highlight state. `pulldown-cmark` delivers fenced code as `Start(CodeBlock(Fenced(lang)))`, one or more `Event::Text`, `End(CodeBlock)`. We accumulate the text and emit a single placeholder span at close.

- [ ] **Step 1: Write the failing test**

Add inside the `mod tests` block in `render/markdown.rs`:

```rust
    #[test]
    fn fenced_code_emits_placeholder_with_lang() {
        let md = "```rust\nfn main() {}\n```";
        let lines = render(md, &theme());
        // exactly one placeholder span carrying the raw code + lang hint.
        let ph = lines
            .iter()
            .flat_map(|l| &l.spans)
            .find(|s| matches!(s.kind, crate::render::SpanKind::CodePlaceholder { .. }))
            .expect("a CodePlaceholder span");
        assert_eq!(ph.text, "fn main() {}\n");
        assert_eq!(
            ph.kind,
            crate::render::SpanKind::CodePlaceholder { lang: Some("rust".to_string()) }
        );
    }

    #[test]
    fn fenced_code_without_lang_has_none() {
        let md = "```\nplain code\n```";
        let lines = render(md, &theme());
        let ph = lines
            .iter()
            .flat_map(|l| &l.spans)
            .find(|s| matches!(s.kind, crate::render::SpanKind::CodePlaceholder { .. }))
            .unwrap();
        assert_eq!(ph.kind, crate::render::SpanKind::CodePlaceholder { lang: None });
    }

    #[test]
    fn fenced_code_is_not_styled_as_inline() {
        let md = "```js\nconst x = 1;\n```";
        let lines = render(md, &theme());
        let ph = lines
            .iter()
            .flat_map(|l| &l.spans)
            .find(|s| matches!(s.kind, crate::render::SpanKind::CodePlaceholder { .. }))
            .unwrap();
        // placeholder text is raw — no bold/italic leaked in.
        assert!(!ph.style.bold);
        assert!(!ph.style.italic);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run:
```bash
cd lingxi-core && cargo test -p lingxi-tui render::markdown::tests::fenced_code 2>&1 | tail -20
```
Expected: FAIL — code-block events fall through to `_ => {}`, so the `Event::Text` inside the fence is rendered as ordinary paragraph text and no `CodePlaceholder` span is produced.

- [ ] **Step 3: Add code-block state + events**

In `render/markdown.rs`, extend `struct Builder<'a>` with (add after `in_blockquote: bool,`):
```rust
    /// When inside a fenced/indented code block: accumulates raw text and
    /// the language hint. `Some` between `Start(CodeBlock)`/`End(CodeBlock)`.
    code_block: Option<CodeBlockState>,
```
And in `Builder::new`:
```rust
            code_block: None,
```

Add the helper struct near `InlineState` (after its `impl`):
```rust
/// Buffered fenced/indented code-block content awaiting M7-02 highlighting.
#[derive(Debug, Default)]
struct CodeBlockState {
    lang: Option<String>,
    text: String,
}
```

Add the import for the code-block kind to the existing `use pulldown_cmark::{...}` line — change it to:
```rust
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
```

In `Builder::handle`, add these arms BEFORE the final `_ => {}` catch-all:
```rust
            Event::Start(Tag::CodeBlock(kind)) => {
                self.flush();
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => {
                        let trimmed = info.trim();
                        if trimmed.is_empty() {
                            None
                        } else {
                            // info-string may be "rust ignore" — take first word.
                            Some(trimmed.split_whitespace().next().unwrap().to_string())
                        }
                    }
                    CodeBlockKind::Indented => None,
                };
                self.code_block = Some(CodeBlockState {
                    lang,
                    text: String::new(),
                });
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some(cb) = self.code_block.take() {
                    self.lines.push(StyledLine {
                        spans: vec![StyledSpan::code_placeholder(cb.text, cb.lang.as_deref())],
                    });
                }
            }
```

The `Event::Text` arm must route into the code buffer when inside a fence. Replace the existing `Event::Text(text) => self.push_text(&text),` arm with:
```rust
            Event::Text(text) => {
                if let Some(cb) = self.code_block.as_mut() {
                    cb.text.push_str(&text);
                } else {
                    self.push_text(&text);
                }
            }
```

- [ ] **Step 4: Run test to verify it passes**

Run:
```bash
cd lingxi-core && cargo test -p lingxi-tui render::markdown 2>&1 | tail -20
```
Expected: PASS — all prior markdown tests plus the three fenced-code tests pass.

- [ ] **Step 5: Commit**

```bash
cd lingxi-core && git add crates/tui/src/render/markdown.rs
git commit -m "$(cat <<'EOF'
plan(M7-01 T9): fenced code emits CodePlaceholder span (M7-02 fills it)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 10: Markdown — partial / streaming input never panics

**Files:**
- Modify: `lingxi-code/crates/tui/src/render/markdown.rs`

**Context:** Streaming assistant output feeds half-written markdown to the renderer mid-token: an unclosed code fence, a half-open bold (`**bold without close`), a dangling list. The spec (§3 M7-01, §4 R5) requires best-effort rendering, never a panic. `pulldown-cmark` already tolerates unterminated constructs (it emits the open events and EOF without the matching close); the risk is our `Builder` leaving state set so `finish()` must flush whatever is buffered — including an open code block. We add a defensive flush of the code buffer in `finish` and lock the no-panic behavior with tests.

- [ ] **Step 1: Write the failing test**

Add inside the `mod tests` block in `render/markdown.rs`:

```rust
    #[test]
    fn unclosed_code_fence_does_not_panic_and_emits_placeholder() {
        // No closing ``` — streaming mid-block.
        let md = "intro\n```rust\nfn main() {";
        let lines = render(md, &theme());
        // intro paragraph present.
        assert!(lines.iter().any(|l| l.plain_text().contains("intro")));
        // the partial code still becomes a placeholder.
        let ph = lines
            .iter()
            .flat_map(|l| &l.spans)
            .find(|s| matches!(s.kind, crate::render::SpanKind::CodePlaceholder { .. }));
        assert!(ph.is_some(), "unclosed fence should still emit a placeholder");
    }

    #[test]
    fn unclosed_bold_does_not_panic() {
        let lines = render("text **still bold", &theme());
        assert!(lines.iter().any(|l| l.plain_text().contains("still bold")));
    }

    #[test]
    fn dangling_list_item_does_not_panic() {
        let _ = render("- a\n- ", &theme());
    }

    #[test]
    fn empty_input_yields_no_lines() {
        assert!(render("", &theme()).is_empty());
    }

    #[test]
    fn lone_special_chars_do_not_panic() {
        let _ = render("*_`#>[](", &theme());
        let _ = render("```", &theme());
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run:
```bash
cd lingxi-core && cargo test -p lingxi-tui render::markdown::tests::unclosed_code_fence_does_not_panic_and_emits_placeholder 2>&1 | tail -20
```
Expected: FAIL — with an unclosed fence, `pulldown-cmark` emits `End(CodeBlock)` at EOF in most cases, but to be safe the `finish()` path must flush a still-open `code_block`; currently `finish` ignores it, so the placeholder assertion may fail. (If pulldown-cmark already closes it, this test passes — in that case still apply Step 3 as a defensive guarantee and proceed.)

- [ ] **Step 3: Make `finish` flush a still-open code block + pending line**

Replace the `finish` method in `render/markdown.rs` with:

```rust
    fn finish(mut self) -> Vec<StyledLine> {
        // Defensive: an unterminated fence (streaming) leaves `code_block`
        // set with no `End(CodeBlock)` event — emit its placeholder so the
        // partial code still renders rather than vanishing.
        if let Some(cb) = self.code_block.take() {
            self.lines.push(StyledLine {
                spans: vec![StyledSpan::code_placeholder(cb.text, cb.lang.as_deref())],
            });
        }
        self.flush();
        // Drop a trailing blank line for tidy output.
        if matches!(self.lines.last(), Some(l) if l.spans.is_empty()) {
            self.lines.pop();
        }
        self.lines
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run:
```bash
cd lingxi-core && cargo test -p lingxi-tui render::markdown 2>&1 | tail -20
```
Expected: PASS — all markdown tests including the five partial-input tests pass with no panic.

- [ ] **Step 5: Commit**

```bash
cd lingxi-core && git add crates/tui/src/render/markdown.rs
git commit -m "$(cat <<'EOF'
plan(M7-01 T10): partial/streaming markdown renders best-effort, no panic

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 11: Markdown insta snapshots (each element + partial)

**Files:**
- Modify: `lingxi-code/crates/tui/src/render/markdown.rs` (tests only)
- Create: insta snapshot files under `lingxi-code/crates/tui/src/snapshots/`

**Context:** Lock the markdown output shape with insta snapshots — spec §5.2 budget is 8+ markdown snapshots: heading, bold/italic, nested list, blockquote, inline code, link, fenced-code-placeholder, partial unclosed fence. Use a fixed test theme so snapshots are deterministic.

- [ ] **Step 1: Add the snapshot tests**

Add inside the `mod tests` block in `render/markdown.rs`:

```rust
    #[test]
    fn snapshot_heading() {
        insta::assert_yaml_snapshot!(render("# Title\n\n## Sub", &theme()));
    }

    #[test]
    fn snapshot_bold_italic() {
        insta::assert_yaml_snapshot!(render("normal **bold** and *italic* mix", &theme()));
    }

    #[test]
    fn snapshot_nested_list() {
        insta::assert_yaml_snapshot!(render("- a\n  - b\n  - c\n- d", &theme()));
    }

    #[test]
    fn snapshot_ordered_list() {
        insta::assert_yaml_snapshot!(render("1. first\n2. second\n3. third", &theme()));
    }

    #[test]
    fn snapshot_blockquote() {
        insta::assert_yaml_snapshot!(render("> quoted line\n> second", &theme()));
    }

    #[test]
    fn snapshot_inline_code() {
        insta::assert_yaml_snapshot!(render("run `cargo test --workspace` now", &theme()));
    }

    #[test]
    fn snapshot_link() {
        insta::assert_yaml_snapshot!(render("see [the docs](https://example.io/guide)", &theme()));
    }

    #[test]
    fn snapshot_fenced_code_placeholder() {
        insta::assert_yaml_snapshot!(render("```rust\nfn main() {}\n```", &theme()));
    }

    #[test]
    fn snapshot_partial_unclosed_fence() {
        insta::assert_yaml_snapshot!(render("text\n```python\nprint(1)", &theme()));
    }
```

- [ ] **Step 2: Run tests; review and accept the snapshots**

Run:
```bash
cd lingxi-core && cargo insta test --review -p lingxi-tui --accept 2>&1 | tail -25
```
Expected: nine new snapshot files generated under `crates/tui/src/snapshots/` and accepted. Eyeball each: `snapshot_fenced_code_placeholder` must show a `CodePlaceholder` span with `lang: rust` and text `fn main() {}\n`; `snapshot_blockquote` lines must start with the `│ ` bar span; `snapshot_heading` h1 span must have bold+italic+underline true.

- [ ] **Step 3: Confirm full markdown set green**

Run:
```bash
cd lingxi-core && cargo test -p lingxi-tui render::markdown 2>&1 | tail -10
```
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
cd lingxi-core && git add crates/tui/src/render/markdown.rs crates/tui/src/snapshots/
git commit -m "$(cat <<'EOF'
plan(M7-01 T11): lock markdown insta snapshots (elements + partial)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 12: Module-level docs + `lib.rs` cleanup audit

**Files:**
- Modify: `lingxi-code/crates/tui/src/lib.rs`

**Context:** Tidy the crate root: update the M6-era doc comment that still references the old `ansi` module surface, and confirm `render` is publicly exposed for downstream M7 sub-plans. No behavior change — documentation + module visibility only.

- [ ] **Step 1: Confirm `render` is declared and the old `ansi` is gone**

Run:
```bash
cd lingxi-core && grep -n "pub mod render\|pub mod ansi" crates/tui/src/lib.rs
```
Expected: `pub mod render;` present, `pub mod ansi;` absent (removed in Task 6).

- [ ] **Step 2: Update the crate-root doc comment**

In `lingxi-code/crates/tui/src/lib.rs`, the top-of-file doc block lists M6 components. Append a line documenting the new module. Find the doc line:
```rust
//! See plan `docs/superpowers/plans/2026-05-28-m6-01-foundation.md`.
```
and insert ABOVE it:
```rust
//!
//! M7-01 adds the `render` module: a full ANSI parser (16-color + 256-color
//! + truecolor, cursor/erase skipped) and a CommonMark markdown renderer
//! (`pulldown-cmark`), both producing the shared `render::StyledLine` model.
//! See plan `docs/superpowers/plans/2026-05-29-m7-01-ansi-markdown.md`.
```

- [ ] **Step 3: Build to confirm docs compile (doc-comment links resolve)**

Run:
```bash
cd lingxi-core && cargo build -p lingxi-tui 2>&1 | tail -5
```
Expected: PASS — clean build.

- [ ] **Step 4: Commit**

```bash
cd lingxi-core && git add crates/tui/src/lib.rs
git commit -m "$(cat <<'EOF'
plan(M7-01 T12): document render module in crate root; lib.rs audit

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 13: Telemetry-count guard (no new events)

**Files:**
- Reference only: `lingxi-code/crates/tui/src/telemetry.rs` and the telemetry registry.

**Context:** M7-01 is pure rendering and registers **zero** new telemetry events. The baseline `ALL_EVENT_NAMES.len() == 326` must be unchanged. This task is an explicit guard so an accidental telemetry edit during M7-01 is caught before the gate.

- [ ] **Step 1: Locate the count assertion / registry test**

Run:
```bash
cd lingxi-core && grep -rn "ALL_EVENT_NAMES\|326" crates/ --include='*.rs' | grep -i "len\|count\|326" | head
```
Expected: find the existing test that asserts the registered event count (established in M3-06 / maintained through M6). Note its crate + test name.

- [ ] **Step 2: Run that count test to confirm it is still 326**

Run (substitute the test name found in Step 1; the count test typically lives in `lingxi-telemetry`):
```bash
cd lingxi-core && cargo test -p lingxi-telemetry all_event_names 2>&1 | tail -15
```
Expected: PASS — count is 326, unchanged. If this FAILS or the number differs, an unintended telemetry change slipped in during M7-01 — revert it; M7-01 adds no events.

- [ ] **Step 3: Confirm no telemetry files were touched by M7-01**

Run:
```bash
cd lingxi-core && git diff --name-only m7.0 2>/dev/null HEAD -- crates/tui/src/telemetry.rs crates/telemetry/ 2>/dev/null || git log --oneline --name-only -13 -- crates/tui/src/telemetry.rs crates/telemetry/ | head
```
Expected: no telemetry source changed across M7-01's commits. (If the `m7.0` ref does not exist, the fallback `git log` shows recent commits touching telemetry — none should be M7-01.)

- [ ] **Step 4: No commit**

This task makes no code change — it is a verification gate. Nothing to commit. Proceed to Task 14.

---

## Task 14: Workspace gate + annotated tag `m7.1`

**Files:**
- None modified — this is the full verification gate and tag.

**Context:** The sub-plan closes with the standard workspace gate (spec §5.4), run from inside `lingxi-code/`, followed by the local annotated tag `m7.1` (spec §6.4). Known flakes (listed in Critical Context) are allowed a rerun.

- [ ] **Step 1: Format check**

Run:
```bash
cd lingxi-core && cargo fmt --check
```
Expected: PASS — no diff. If it reports formatting, run `cargo fmt` and amend into the relevant task commit is NOT allowed; instead make a tidy follow-up commit `plan(M7-01 T14): cargo fmt` with the standard trailer.

- [ ] **Step 2: Clippy, deny warnings**

Run:
```bash
cd lingxi-core && cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -20
```
Expected: PASS — zero warnings. Fix any clippy finding in the offending file and commit as `plan(M7-01 T14): clippy fixes` with the trailer.

- [ ] **Step 3: Full workspace test**

Run:
```bash
cd lingxi-core && cargo test --workspace 2>&1 | tail -30
```
Expected: PASS. The new `render::` tests (≈ 6 ANSI behavior + 6 ANSI snapshots + 11 markdown behavior + 9 markdown snapshots + 4 model tests) all pass; M6 `user_tool_result` tests still pass. If only a known flake fails (`rapid_writes_collapse_to_single_event`, `writer_output_equals_single_turn_fixture`, `streaming_concurrent_tools_test`, posix fs_watch timing), rerun that single test once to confirm it is the flake and not a regression.

- [ ] **Step 4: Cross-platform compile gate (5 targets)**

Run:
```bash
cd lingxi-core && for t in x86_64-unknown-linux-gnu x86_64-apple-darwin x86_64-pc-windows-gnu aarch64-linux-android aarch64-apple-ios; do echo "=== $t ==="; cargo check --workspace --target "$t" 2>&1 | tail -3; done
```
Expected: each target reports `Finished`. (`pulldown-cmark` is pure Rust with no platform-specific deps, so all five should compile. If a target's toolchain/std is not installed, install it with `rustup target add <t>` first — same posture as v0.7.0.)

- [ ] **Step 5: Create the annotated tag**

Run:
```bash
cd lingxi-core && git -C "$(git rev-parse --show-toplevel)" tag -a m7.1 -m "$(cat <<'EOF'
M7-01: full ANSI parser (256/truecolor/cursor-skip) + markdown foundation

- render/ module: StyleColor/SpanStyle/StyledSpan/StyledLine model
- render/ansi.rs: 16-color + 256 (38;5;N) + truecolor (38;2;r;g;b),
  cursor/erase CSI safely skipped; output Vec<StyledLine>
- render/markdown.rs: pulldown-cmark =0.13.4 → Vec<StyledLine>
  (headings/bold/italic/lists/nested/blockquote/inline-code/link),
  fenced code → CodePlaceholder span (filled by M7-02), partial input
  renders best-effort without panic
- old src/ansi.rs deleted; user_tool_result migrated
- telemetry unchanged (326 events)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 6: Verify the tag exists and points at the latest M7-01 commit**

Run:
```bash
cd lingxi-core && git -C "$(git rev-parse --show-toplevel)" tag -l m7.1 && git -C "$(git rev-parse --show-toplevel)" show --no-patch --format="%H %s" m7.1
```
Expected: `m7.1` listed; shows the Task 12/14 head commit. Do NOT push (local only, per spec §6.4).

---

## Self-Review

**Spec coverage (design §3 M7-01 + supporting sections):**
- `render/` module created (mod.rs) — Task 2. ✔
- M6 `ansi.rs` moved+expanded to `render/ansi.rs` — Tasks 3–6. ✔
- 256-color `38;5;N` / `48;5;N` — Task 4. ✔
- Truecolor `38;2;r;g;b` / `48;2;r;g;b` — Task 4. ✔
- Cursor-move + erase (CUU/CUD/CUF/CUB/ED/EL) skipped without corruption — Task 5. ✔
- Update all call sites of old ansi module — Task 6 (sole consumer `user_tool_result.rs`; `lib.rs` decl). ✔
- `render/markdown.rs` CommonMark → `Vec<StyledLine>` (headings, bold, italic, lists ordered/unordered/nested, blockquote, inline code, links) — Tasks 7–8. ✔
- Fenced code emits PLACEHOLDER span (not highlighted) — Task 9. ✔
- Partial/streaming markdown renders best-effort, no panic — Task 10. ✔
- `pulldown-cmark` added to Cargo.toml, pinned EXACT, MSRV 1.82 verified, first task verifies dep builds — Task 1. ✔
- ANSI snapshots (256/truecolor/reset/malformed/cursor-skipped) — Task 5. ✔
- Markdown snapshots (heading/bold-italic/nested list/blockquote/inline code/link/fenced-placeholder/partial) — Task 11. ✔
- Telemetry untouched (326) — Task 13. ✔
- Workspace gate (fmt + clippy -D warnings + test, all from `lingxi-code/`) + 5-target compile + annotated tag `m7.1` — Task 14. ✔

**Placeholder scan:** No TBD/TODO/"add error handling"/"similar to Task N". Every code step shows complete code; every test step shows the test body; every command shows expected output.

**Type consistency:** `StyleColor`, `NamedColor`, `SpanStyle`, `StyledSpan`, `StyledLine`, `SpanKind`, `MarkdownTheme`, `Builder`, `InlineState`, `CodeBlockState`, `parse_ansi`, `render`, `StyleColor::to_iocraft`, `StyledSpan::{plain,styled,code_placeholder}`, `StyledLine::{empty,plain,plain_text}` are defined in Task 2 (model) / Task 3–4 (ansi) / Task 7–10 (markdown) and used consistently in later tasks. `render_user_tool_result_body_spans` return type changes `Vec<AnsiSpan>` → `Vec<StyledSpan>` consistently in Task 6. The `pulldown-cmark` event/tag names (`Tag::Strong`, `TagEnd::Strong`, `Tag::Heading{level,..}`, `TagEnd::Heading(_)`, `Tag::List(Option<u64>)`, `Tag::Item`, `Tag::BlockQuote(_)`, `Tag::CodeBlock(CodeBlockKind)`, `TagEnd::CodeBlock`, `Event::Code`, `Event::Text`, `Event::SoftBreak/HardBreak`, `CodeBlockKind::{Fenced,Indented}`) match the pinned `0.13.x` API.

**Known divergence (documented):** Design §2.2 example string was `pulldown-cmark = "=0.12.2"`; this plan pins `=0.13.4` (verified latest stable, MSRV 1.71.1 < 1.82) — recorded in Task 1 context. The 0.13 API uses `TagEnd` (split from `Tag`) and `Tag::BlockQuote(Option<BlockQuoteKind>)` / `Tag::List(Option<u64>)`; the task code uses the 0.13 shape. If execution finds a 0.13 API drift, fix the arm to match the pinned version's `Event`/`Tag`/`TagEnd` definitions — the semantics (which event marks bold/heading/list/code) are stable.
