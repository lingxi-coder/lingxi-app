//! Parity (M7-16 T5): structure-level lock of the M7 renderer surface.
//!
//! Mirrors the M6 `parity_tui_renderers.rs` pattern — feed a fixed input into
//! the pure renderer the iocraft component delegates to, assert the rendered
//! text/structure matches the golden value in `parity_tui_renderers_m7.json`.
//!
//! Coverage:
//! - M7-04/05 message renderers: compact-boundary marker, bash-input prefix,
//!   image placeholder, grouped-tool-use group header (via each renderer's
//!   pure `render_*_to_string` / label oracle).
//! - `render::markdown` element STRUCTURE: heading, bold/italic, list,
//!   blockquote, inline-code, fenced code block (line count + fence lang) — the
//!   plain-text projection of the `StyledLine`s, NOT per-token color.
//! - `render::syntax::highlight` STRUCTURE: rust/json highlighted + unknown-lang
//!   plain fallback — line counts + first-line text, NOT per-token color
//!   (explicit literal-lock exception per spec §0 Q3).
//! - `render::diff::render` layout: hunk header `@@ ... @@`, `+`/`-` markers,
//!   every expected minus/plus body line present in the gutter rows.
//!
//! Per-token highlight COLOR is deliberately NOT locked here (parity =
//! equivalent look). Color/style is covered by the lingxi-tui insta snapshots.

use lingxi_tui::render::markdown::{render as render_markdown, MarkdownTheme};
use lingxi_tui::render::syntax::highlight;
use lingxi_tui::render::{diff, StyleColor, StyledLine};
use lingxi_tui::theme::ThemeName;
use serde_json::Value;

const FIXTURE: &str = include_str!("../src/parity/fixtures/parity_tui_renderers_m7.json");

fn load() -> Value {
    serde_json::from_str(FIXTURE).expect("parity_tui_renderers_m7.json parses")
}

/// A plain-text `MarkdownTheme` — the structure tests ignore color (the
/// `code_theme` is immaterial for the plain-text projection).
fn md_theme() -> MarkdownTheme {
    MarkdownTheme {
        inline_code: StyleColor::Default,
        code_theme: ThemeName::Dark,
    }
}

fn plain(lines: &[StyledLine]) -> Vec<String> {
    lines.iter().map(StyledLine::plain_text).collect()
}

// ---- M7-04/05 renderers -----------------------------------------------------

#[test]
fn compact_boundary_marker_is_locked() {
    let f = load();
    let r = &f["renderers"]["compact_boundary"];
    assert_eq!(
        lingxi_tui::components::messages::compact_boundary::render_compact_boundary_to_string(),
        r["expected_rendered_text"].as_str().unwrap(),
    );
}

#[test]
fn bash_input_prefix_is_locked() {
    let f = load();
    let r = &f["renderers"]["bash_input"];
    assert_eq!(
        lingxi_tui::components::messages::bash_input::render_bash_input_to_string(
            r["command"].as_str().unwrap()
        ),
        r["expected_rendered_text"].as_str().unwrap(),
    );
}

#[test]
fn image_placeholder_is_locked() {
    let f = load();
    let r = &f["renderers"]["image"];
    let id = r["image_id"].as_u64();
    assert_eq!(
        lingxi_tui::components::messages::image::render_image_label(id, None),
        r["expected_rendered_text"].as_str().unwrap(),
    );
}

#[test]
fn grouped_tool_use_header_is_locked() {
    let f = load();
    let r = &f["renderers"]["grouped_tool_use"];
    let header = lingxi_tui::components::messages::grouped_tool_use::render_grouped_to_string(
        r["tool"].as_str().unwrap(),
        &[],
        false,
    );
    assert_eq!(header, r["expected_group_header"].as_str().unwrap());
}

// ---- markdown structure -----------------------------------------------------

#[test]
fn markdown_elements_render_locked_structure() {
    let f = load();
    let md = &f["markdown"];
    for key in [
        "heading",
        "bold_italic",
        "list",
        "blockquote",
        "inline_code",
    ] {
        let case = &md[key];
        let got = plain(&render_markdown(
            case["input"].as_str().unwrap(),
            &md_theme(),
        ));
        let want: Vec<String> = case["expected_structure"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert_eq!(got, want, "markdown `{key}` structure mismatch");
    }
}

#[test]
fn markdown_fenced_block_locks_lang_and_line_count() {
    let f = load();
    let c = &f["markdown"]["fenced_block"];
    let lines = render_markdown(c["input"].as_str().unwrap(), &md_theme());
    let got = plain(&lines);
    let want: Vec<String> = c["expected_lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        got.len(),
        usize::try_from(c["expected_line_count"].as_u64().unwrap()).unwrap(),
        "fenced-block line count mismatch (``` fences are dropped)"
    );
    assert_eq!(got, want, "fenced-block body mismatch");
}

// ---- syntax structure (NOT per-token color) --------------------------------

#[test]
fn syntax_highlight_locks_line_count_and_first_line_text() {
    let f = load();
    let sy = &f["syntax"];
    for key in ["rust", "json", "unknown"] {
        let c = &sy[key];
        let lines = highlight(
            c["code"].as_str().unwrap(),
            Some(c["lang"].as_str().unwrap()),
            ThemeName::Dark,
        );
        let p = plain(&lines);
        assert_eq!(
            p.len(),
            usize::try_from(c["expected_line_count"].as_u64().unwrap()).unwrap(),
            "syntax `{key}` line-count mismatch"
        );
        assert_eq!(
            p.first().map(String::as_str),
            Some(c["expected_first_line_text"].as_str().unwrap()),
            "syntax `{key}` first-line text mismatch"
        );
    }
}

// ---- diff layout ------------------------------------------------------------

#[test]
fn diff_add_remove_modify_has_locked_markers_and_hunk_header() {
    let f = load();
    let d = &f["diff"]["add_remove_modify"];
    let lines = diff::render(
        d["old"].as_str().unwrap(),
        d["new"].as_str().unwrap(),
        d["path"].as_str(),
        ThemeName::Dark,
    );
    let text = plain(&lines);

    // Hunk header present with the locked `@@ ... @@` shape.
    let prefix = d["expected_hunk_header_prefix"].as_str().unwrap();
    assert!(
        text.iter().any(|l| l.trim_start().starts_with(prefix)),
        "no hunk header line starting with `{prefix}` in {text:?}"
    );
    assert!(
        text.iter()
            .any(|l| l == d["expected_hunk_header"].as_str().unwrap()),
        "exact hunk header `{}` missing in {text:?}",
        d["expected_hunk_header"].as_str().unwrap()
    );

    // Every expected minus line appears on a `-`-marked gutter row.
    for minus in d["expected_minus_lines"].as_array().unwrap() {
        let m = minus.as_str().unwrap();
        assert!(
            text.iter().any(|l| l.contains(" - ") && l.contains(m)),
            "minus line `{m}` missing a `-`-marked row in {text:?}"
        );
    }
    // Every expected plus line appears on a `+`-marked gutter row.
    for plus in d["expected_plus_lines"].as_array().unwrap() {
        let p = plus.as_str().unwrap();
        assert!(
            text.iter().any(|l| l.contains(" + ") && l.contains(p)),
            "plus line `{p}` missing a `+`-marked row in {text:?}"
        );
    }
}
