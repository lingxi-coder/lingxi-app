//! Shared "popup window" renderer for the `/connect` + `/model` pickers
//! (opencode-style): a centered, rounded-border box floating near the top of the
//! screen, with a bold title + right-aligned `esc` hint, a `Search` line, then
//! grouped/selectable rows. Pure render helper (no state) — the picker screens
//! own their reducers and feed structured [`PopupLine`]s in.
//!
//! iocraft 0.8 has no z-index overlay, so this renders INSTEAD OF the REPL (same
//! discipline as the other screens); the centered bordered box still reads as a
//! modal popup.

use iocraft::prelude::*;

use crate::theme::Theme;

/// Leading marker glyph for a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopupMarker {
    /// No marker (two spaces of lead).
    None,
    /// `✓` — connected / available (green).
    Check,
    /// `●` — the currently-active item (e.g. the live model), accent-colored.
    Dot,
}

/// One rendered line in the popup body.
#[derive(Debug, Clone)]
pub enum PopupLine {
    /// A non-selectable group header (e.g. "Popular", "Recent"), accent-bold.
    Header(String),
    /// A selectable row.
    Item {
        /// Leading marker.
        marker: PopupMarker,
        /// Primary label (e.g. "GitHub Copilot", "Claude Opus 4.8").
        label: String,
        /// Dim trailing detail on the same line (e.g. a description or the
        /// provider name). Empty = none.
        detail: String,
        /// Right-aligned badge (e.g. "Free"). Empty = none.
        badge: String,
        /// Whether this row is highlighted (peach background, like opencode).
        selected: bool,
    },
}

/// Peach highlight used for the selected row (opencode's accent).
const HIGHLIGHT: Color = Color::Rgb { r: 0xF2, g: 0xA9, b: 0x7E };

/// Render the popup. `title` is bold top-left, `esc` sits top-right; `search` is
/// the live query (dim placeholder when empty); `lines` are the grouped rows;
/// `footer` is an optional dim hint line (e.g. "Connect provider ctrl+a").
#[must_use]
pub fn render_picker_popup(
    title: &str,
    search: &str,
    lines: &[PopupLine],
    footer: Option<&str>,
    viewport_width: usize,
    viewport_height: usize,
    theme: &Theme,
) -> AnyElement<'static> {
    let box_w = viewport_width.saturating_sub(6).clamp(46, 88);
    let header_color = Color::Magenta;

    // Title row: title (bold) <-----> esc (dim).
    let title_owned = title.to_string();
    let title_row = element! {
        View(width: 100pct, flex_direction: FlexDirection::Row, justify_content: JustifyContent::SpaceBetween) {
            Text(content: title_owned, color: theme.text, weight: Weight::Bold)
            Text(content: "esc".to_string(), color: theme.dim)
        }
    };

    // Search row: dim "Search" placeholder, or the typed query.
    let (search_text, search_color) = if search.is_empty() {
        ("Search".to_string(), theme.dim)
    } else {
        (search.to_string(), theme.text)
    };

    // Body rows.
    let body: Vec<AnyElement<'static>> = lines
        .iter()
        .map(|line| match line {
            PopupLine::Header(label) => {
                let label = label.clone();
                element! {
                    View(flex_direction: FlexDirection::Column, padding_top: 1) {
                        Text(content: label, color: header_color, weight: Weight::Bold)
                    }
                }
                .into_any()
            }
            PopupLine::Item { marker, label, detail, badge, selected } => {
                render_item(*marker, label, detail, badge, *selected, theme)
            }
        })
        .collect();

    let footer_el: Option<AnyElement<'static>> = footer.map(|f| {
        let f = f.to_string();
        element! {
            View(flex_direction: FlexDirection::Column, padding_top: 1) {
                Text(content: f, color: theme.dim)
            }
        }
        .into_any()
    });

    element! {
        View(
            width: viewport_width as u16,
            height: viewport_height as u16,
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            padding_top: 1,
        ) {
            View(
                width: box_w as u16,
                border_style: BorderStyle::Round,
                border_color: theme.dim,
                flex_direction: FlexDirection::Column,
                padding_left: 2,
                padding_right: 2,
                padding_top: 1,
                padding_bottom: 1,
            ) {
                #(std::iter::once(title_row.into_any()))
                View(flex_direction: FlexDirection::Column, padding_top: 1) {
                    Text(content: search_text, color: search_color)
                }
                #(body.into_iter())
                #(footer_el.into_iter())
            }
        }
    }
    .into_any()
}

/// Render one selectable row. Selected → full-width peach background + black
/// text; otherwise marker(green/accent) + label(text) + detail(dim) + a
/// right-aligned dim badge.
fn render_item(
    marker: PopupMarker,
    label: &str,
    detail: &str,
    badge: &str,
    selected: bool,
    theme: &Theme,
) -> AnyElement<'static> {
    let glyph = match marker {
        PopupMarker::None => "  ".to_string(),
        PopupMarker::Check => "\u{2713} ".to_string(),
        PopupMarker::Dot => "\u{25CF} ".to_string(),
    };
    let badge_owned = badge.to_string();
    let has_badge = !badge.is_empty();

    if selected {
        // Whole row reads as one peach bar with black text (opencode highlight).
        let mut left = String::new();
        left.push_str(&glyph);
        left.push_str(label);
        if !detail.is_empty() {
            left.push_str("  ");
            left.push_str(detail);
        }
        element! {
            View(width: 100pct, background_color: HIGHLIGHT, flex_direction: FlexDirection::Row, justify_content: JustifyContent::SpaceBetween) {
                Text(content: left, color: Color::Black, weight: Weight::Bold)
                #(has_badge.then(|| element! { Text(content: badge_owned.clone(), color: Color::Black) }))
            }
        }
        .into_any()
    } else {
        let marker_color = match marker {
            PopupMarker::Check => theme.success,
            PopupMarker::Dot => theme.suggestion,
            PopupMarker::None => theme.dim,
        };
        let label_owned = label.to_string();
        let detail_owned = if detail.is_empty() {
            String::new()
        } else {
            format!("  {detail}")
        };
        element! {
            View(width: 100pct, flex_direction: FlexDirection::Row, justify_content: JustifyContent::SpaceBetween) {
                View(flex_direction: FlexDirection::Row) {
                    Text(content: glyph, color: marker_color)
                    Text(content: label_owned, color: theme.text)
                    #((!detail_owned.is_empty()).then(|| element! { Text(content: detail_owned.clone(), color: theme.dim) }))
                }
                #(has_badge.then(|| element! { Text(content: badge_owned.clone(), color: theme.suggestion) }))
            }
        }
        .into_any()
    }
}
