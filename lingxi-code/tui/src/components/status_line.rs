//! `StatusLine` — the 1-row top zone.
//!
//! Field order (left → right, space-separated per byte-lock L1):
//!     model  cwd  $cost  ctx%  mode
//!
//! Locked literals:
//!   L1 separator = " "
//!   L2 zero-cost = "$0.0000" (M6-06 — claude-code 4-decimal parity)
//!   L3 context% = "{:.0}%"

use std::path::PathBuf;

use iocraft::prelude::*;
use permission::PermissionMode;
use unicode_width::UnicodeWidthStr;

use crate::render::ansi::parse_ansi;
use crate::theme::Theme;

/// Props for `StatusLine`.
#[derive(Props)]
pub struct StatusLineProps {
    /// Display name of the active model.
    pub model: String,
    /// Working directory for the session.
    pub cwd: PathBuf,
    /// Pre-formatted cost string (e.g. `"$0.0000"`).
    pub cost: String,
    /// Context window utilisation in `[0.0, 1.0]`.
    pub context_pct: f32,
    /// Active permission mode.
    pub permission_mode: PermissionMode,
    /// (M7-15) Active palette — the status line text color reads from
    /// `theme.text`, so it recolors with the picker.
    pub theme: Theme,
    /// (A6) Custom status-line text — the resolved stdout of the user's
    /// `statusLine: {type:'command'}` hook, already passed through
    /// [`format_custom_status_line`]. `None` (the default) preserves the
    /// built-in `model cwd cost ctx% mode` row byte-for-byte. `Some(text)`
    /// renders the custom text through the in-tree ANSI parser with `padding_x`
    /// left/right padding and truncation to `width`, mirroring claude-code's
    /// `StatusLine.tsx` `<Text dimColor wrap="truncate"><Ansi>…` branch.
    pub custom: Option<String>,
    /// (A6) Horizontal padding (cells) applied on each side of the custom text,
    /// mirroring claude-code's `<Box paddingX={paddingX}>`. Read from
    /// `statusLine.padding` (default `0`). Ignored on the built-in path.
    pub padding_x: usize,
    /// (A6) Terminal width (cells) the custom text is truncated to (after
    /// subtracting `2 * padding_x`). `0` disables truncation. Ignored on the
    /// built-in path.
    pub width: usize,
}

impl Default for StatusLineProps {
    fn default() -> Self {
        Self {
            model: String::new(),
            cwd: PathBuf::from("."),
            cost: "$0.0000".to_string(),
            context_pct: 0.0,
            permission_mode: PermissionMode::Default,
            theme: Theme::dark(),
            custom: None,
            padding_x: 0,
            width: 0,
        }
    }
}

/// Map a `PermissionMode` to its short status-line label.
#[must_use]
pub fn mode_label(m: PermissionMode) -> &'static str {
    match m {
        PermissionMode::Default => "default",
        PermissionMode::Plan => "plan",
        PermissionMode::AcceptEdits => "acceptEdits",
        PermissionMode::BypassPermissions => "bypassPermissions",
        PermissionMode::DontAsk => "dontAsk",
        PermissionMode::Bubble => "bubble",
        PermissionMode::Auto => "auto",
    }
}

/// Format the full status line per the M6-02 byte-locks.
#[must_use]
pub fn format_status_line(
    model: &str,
    cwd: &std::path::Path,
    cost: &str,
    context_pct: f32,
    mode: PermissionMode,
) -> String {
    format!(
        "{} {} {} {:.0}% {}",
        model,
        cwd.display(),
        cost,
        context_pct * 100.0,
        mode_label(mode),
    )
}

/// Transform a custom status-line command's raw stdout into the displayed
/// text, byte-faithful to claude-code's `executeStatusLineCommand`
/// (`hooks.ts:4640-4644`):
///
/// ```text
/// result.stdout
///   .trim()
///   .split('\n')
///   .flatMap(line => line.trim() || [])
///   .join('\n')
/// ```
///
/// i.e. trim the whole output, split on `\n`, trim each line, DROP blank lines
/// (lines that trim to empty), then re-join the survivors with `\n`. The
/// JS `String.prototype.trim()` strips ASCII + Unicode whitespace; Rust's
/// `str::trim` strips the Unicode `White_Space` set — equivalent for all the
/// whitespace a shell hook emits (spaces, tabs, `\r`, newlines).
///
/// Note `split('\n')` (not `split(/\r?\n/)`): a trailing `\r` on a CRLF line is
/// part of the per-line text and is removed by the per-line `trim`, matching
/// the TS exactly.
#[must_use]
// Byte-faithful to claude-code's `result.stdout.trim().split('\n')`. We do NOT
// use `str::lines()`: the spec is `split('\n')` (a bare `\r` is part of the
// line text and is stripped by the per-line `trim`, not by the splitter). The
// observable result happens to match `lines()` here, but the contract is
// `split('\n')`, so the pedantic suggestion is intentionally suppressed.
#[allow(clippy::str_split_at_newline)]
pub fn format_custom_status_line(stdout: &str) -> String {
    stdout
        .trim()
        .split('\n')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Truncate `text` to at most `max_cols` display columns (Unicode width),
/// mirroring Ink's `wrap="truncate"`. Returns the original string when it
/// already fits or when `max_cols == 0` (truncation disabled). Truncation is
/// hard (no ellipsis) — Ink's default truncate drops overflowing columns
/// without inserting a marker.
#[must_use]
pub fn truncate_to_width(text: &str, max_cols: usize) -> String {
    if max_cols == 0 || UnicodeWidthStr::width(text) <= max_cols {
        return text.to_string();
    }
    let mut acc = 0usize;
    let mut out = String::new();
    for ch in text.chars() {
        let w = UnicodeWidthStr::width(ch.to_string().as_str());
        if acc + w > max_cols {
            break;
        }
        acc += w;
        out.push(ch);
    }
    out
}

/// Status-line component.
///
/// Mirrors claude-code's `statusLineShouldDisplay = settings?.statusLine !==
/// undefined` (`StatusLine.tsx`): the status row renders ONLY when the user has
/// configured a custom `statusLine` command. With no command (`custom: None`,
/// the default) NOTHING is drawn — claude-code has no built-in `model cwd cost
/// ctx% mode` row. (LingXi previously rendered such a built-in row; it was
/// removed for strict 1:1.)
///
///   - `custom: None` (default) → nothing (empty `View`).
///   - `custom: Some(text)` → the custom command's already-transformed stdout,
///     parsed through the in-tree ANSI parser and rendered with `padding_x`
///     left/right padding, truncated to `width - 2*padding_x` columns. Mirrors
///     claude-code `StatusLine.tsx`'s
///     `<Box paddingX={paddingX}><Text dimColor wrap="truncate"><Ansi>…`.
#[component]
pub fn StatusLine(props: &StatusLineProps) -> impl Into<AnyElement<'static>> {
    let Some(custom) = props.custom.as_ref() else {
        // No custom statusLine command → render nothing (parity with
        // `statusLineShouldDisplay`). The model/cwd/cost props are unused here.
        return element! { View(height: 0) }.into_any();
    };
    render_custom(custom, props.padding_x, props.width, &props.theme)
}

/// Render the custom status-line text (first visual line only — the status row
/// is height 1, matching Ink's single-`<Text>` truncate) through the ANSI
/// parser, with `padding_x` cells of left padding and truncation to the inner
/// width. Each parsed span becomes a `Text` carrying its fg color + bold.
fn render_custom(
    custom: &str,
    padding_x: usize,
    width: usize,
    theme: &Theme,
) -> AnyElement<'static> {
    // Inner content width = total width minus both pads. Ink truncates the
    // text region, not the padding.
    let inner = width.saturating_sub(padding_x.saturating_mul(2));
    // Status row is a single line: take the first parsed visual line. A custom
    // command that emits multiple lines (already joined with `\n`) collapses to
    // its first row here, matching Ink's single-line `wrap="truncate"` <Text>.
    let parsed = parse_ansi(custom);
    let first = parsed.into_iter().next().unwrap_or_default();

    // Apply truncation across the whole line by accumulating display width and
    // dropping spans (and partial spans) past the budget.
    let mut remaining = if width == 0 { usize::MAX } else { inner };
    let mut span_elements: Vec<AnyElement<'static>> = Vec::new();
    for span in first.spans {
        if remaining == 0 {
            break;
        }
        let text = if remaining == usize::MAX {
            span.text.clone()
        } else {
            let t = truncate_to_width(&span.text, remaining);
            remaining = remaining.saturating_sub(UnicodeWidthStr::width(t.as_str()));
            t
        };
        if text.is_empty() {
            continue;
        }
        // claude-code renders the custom line `dimColor`; when the command
        // emits no explicit SGR color we fall back to the theme's dim color so
        // the whole row reads dim (matching `<Text dimColor>`). Explicit ANSI
        // colors from the command override.
        let color = match span.style.fg {
            crate::render::StyleColor::Default => theme.dim,
            other => other.to_iocraft(),
        };
        let weight = if span.style.bold {
            Weight::Bold
        } else {
            Weight::Normal
        };
        span_elements
            .push(element! { Text(content: text, color: color, weight: weight) }.into_any());
    }

    let pad = " ".repeat(padding_x);
    element! {
        View(flex_direction: FlexDirection::Row, height: 1) {
            Text(content: pad.clone(), color: theme.dim)
            #(span_elements)
            Text(content: pad, color: theme.dim)
        }
    }
    .into_any()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn format_matches_byte_locks() {
        let s = format_status_line(
            "claude-sonnet-4.5",
            &PathBuf::from("/a/b"),
            "$0.0000",
            0.42,
            PermissionMode::Default,
        );
        assert_eq!(s, "claude-sonnet-4.5 /a/b $0.0000 42% default");
    }

    #[test]
    fn status_line_props_carry_theme() {
        // (M7-15) StatusLineProps gains a `theme` field; default is dark.
        let props = StatusLineProps::default();
        assert_eq!(props.theme, Theme::dark());
    }

    #[test]
    fn mode_label_covers_all_variants() {
        assert_eq!(mode_label(PermissionMode::Default), "default");
        assert_eq!(mode_label(PermissionMode::Plan), "plan");
        assert_eq!(mode_label(PermissionMode::AcceptEdits), "acceptEdits");
        assert_eq!(
            mode_label(PermissionMode::BypassPermissions),
            "bypassPermissions"
        );
        assert_eq!(mode_label(PermissionMode::DontAsk), "dontAsk");
    }

    // ---- (A6) custom status line ----

    #[test]
    fn format_custom_status_line_trims_and_joins() {
        // Multi-line, blank-line dropping, per-line and whole-output trim.
        let raw = "  \n  hello  \n\n   world\t\n\n  ";
        // Whole trim → "hello  \n\n   world"; split→["hello  ","","   world"];
        // per-line trim+drop-empty → ["hello","world"]; join → "hello\nworld".
        assert_eq!(format_custom_status_line(raw), "hello\nworld");
    }

    #[test]
    fn format_custom_status_line_single_line_trailing_ws() {
        assert_eq!(format_custom_status_line("  one line  \t"), "one line");
    }

    #[test]
    fn format_custom_status_line_empty_is_empty() {
        assert_eq!(format_custom_status_line(""), "");
        assert_eq!(format_custom_status_line("   \n\n  \t  "), "");
    }

    #[test]
    fn format_custom_status_line_crlf_strips_carriage_return() {
        // split('\n') leaves the trailing \r on each line; per-line trim removes it.
        assert_eq!(format_custom_status_line("a\r\nb\r\n"), "a\nb");
    }

    #[test]
    fn truncate_to_width_basic() {
        assert_eq!(truncate_to_width("hello", 3), "hel");
        assert_eq!(truncate_to_width("hello", 5), "hello");
        assert_eq!(truncate_to_width("hello", 99), "hello");
        // 0 disables truncation.
        assert_eq!(truncate_to_width("hello", 0), "hello");
    }

    #[test]
    fn truncate_to_width_wide_char_not_split() {
        // CJK chars are width-2: budget 3 fits one (2) but not two (4); the
        // second would overflow so it is dropped whole, not split.
        assert_eq!(truncate_to_width("日本", 3), "日");
        assert_eq!(truncate_to_width("日本", 4), "日本");
    }

    #[test]
    fn custom_some_renders_custom_text_with_padding() {
        let mut element = element! {
            StatusLine(
                model: "claude-sonnet-4.5".to_string(),
                cwd: PathBuf::from("/a/b"),
                cost: "$0.0000".to_string(),
                context_pct: 0.42_f32,
                permission_mode: PermissionMode::Default,
                custom: Some("custom!".to_string()),
                padding_x: 2_usize,
                width: 40_usize,
            )
        };
        let rendered = element.to_string();
        // The custom text shows; the built-in line MUST NOT.
        assert!(rendered.contains("custom!"), "got: {rendered:?}");
        assert!(
            !rendered.contains("claude-sonnet-4.5"),
            "built-in line leaked into custom path: {rendered:?}"
        );
        // 2 cells of left padding before the text.
        assert!(
            rendered.contains("  custom!"),
            "padding missing: {rendered:?}"
        );
    }

    #[test]
    fn custom_none_renders_nothing() {
        // claude-code `statusLineShouldDisplay`: with no custom statusLine
        // command, the status row renders NOTHING (no built-in model/cwd/cost
        // row). Strict 1:1.
        let mut element = element! {
            StatusLine(
                model: "claude-sonnet-4.5".to_string(),
                cwd: PathBuf::from("/a/b"),
                cost: "$0.0000".to_string(),
                context_pct: 0.42_f32,
                permission_mode: PermissionMode::Default,
            )
        };
        let rendered = element.to_string();
        assert!(
            !rendered.contains("claude-sonnet-4.5"),
            "built-in status row must not render when no custom statusLine is set: {rendered:?}"
        );
        assert_eq!(
            rendered.trim(),
            "",
            "expected an empty status row, got: {rendered:?}"
        );
    }

    #[test]
    fn custom_truncates_to_inner_width() {
        // width 10, padding 2 → inner 6 columns → "abcdef" survives, rest dropped.
        let mut element = element! {
            StatusLine(
                custom: Some("abcdefghij".to_string()),
                padding_x: 2_usize,
                width: 10_usize,
            )
        };
        let rendered = element.to_string();
        assert!(rendered.contains("abcdef"), "got: {rendered:?}");
        assert!(!rendered.contains("abcdefg"), "not truncated: {rendered:?}");
    }
}
