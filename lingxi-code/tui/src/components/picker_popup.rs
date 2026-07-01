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
use crate::render_iocraft::StyleColorIocraftExt;

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

    // Title row: title (bold, terminal default fg) <-----> esc (dim).
    //
    // We render the title and the typed search in the terminal's DEFAULT
    // foreground (no explicit color) rather than `theme.text`. claude-code's
    // `select.tsx`/`ListItem` paint ordinary text with `color={undefined}` —
    // the terminal's own fg, which is contrast-safe on ANY background. Using
    // `theme.text` (white in the dark palette) makes the text INVISIBLE on a
    // light terminal whenever `Auto` resolves to Dark (no `$COLORFGBG`, no
    // OSC-11) — the white-on-white bug. Dim/accent colors stay (they read on
    // both); only the would-be-`theme.text` spots drop to the default fg.
    let title_owned = title.to_string();
    let title_row = element! {
        View(width: 100pct, flex_direction: FlexDirection::Row, justify_content: JustifyContent::SpaceBetween) {
            Text(content: title_owned, weight: Weight::Bold)
            Text(content: "esc".to_string(), color: theme.dim.to_iocraft())
        }
    };

    // Search row: dim "Search" placeholder, or the typed query in the default fg.
    let search_row: AnyElement<'static> = if search.is_empty() {
        element! { Text(content: "Search".to_string(), color: theme.dim.to_iocraft()) }.into_any()
    } else {
        element! { Text(content: search.to_string()) }.into_any()
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
            PopupLine::Item {
                marker,
                label,
                detail,
                badge,
                selected,
            } => render_item(*marker, label, detail, badge, *selected, theme),
        })
        .collect();

    let footer_el: Option<AnyElement<'static>> = footer.map(|f| {
        let f = f.to_string();
        element! {
            View(flex_direction: FlexDirection::Column, padding_top: 1) {
                Text(content: f, color: theme.dim.to_iocraft())
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
                border_color: theme.dim.to_iocraft(),
                flex_direction: FlexDirection::Column,
                padding_left: 2,
                padding_right: 2,
                padding_top: 1,
                padding_bottom: 1,
            ) {
                #(std::iter::once(title_row.into_any()))
                View(flex_direction: FlexDirection::Column, padding_top: 1) {
                    #(std::iter::once(search_row))
                }
                #(body.into_iter())
                #(footer_el.into_iter())
            }
        }
    }
    .into_any()
}

/// Render one selectable row, 1:1 with claude-code's `ListItem`: the focused
/// (selected) row carries a leading pointer `❯` and renders its label in the
/// accent (`suggestion`) color; other rows reserve a 2-col blank so columns
/// align. Connected rows keep a green `✓`; the active-model dot keeps `●`. A
/// right-aligned `suggestion` badge trails when present.
///
/// We deliberately do NOT use a full-width `background_color` bar. A truecolor
/// `Rgb` background (the old opencode-style peach highlight) is emitted as a
/// 24-bit SGR unconditionally — on a non-truecolor terminal it quantizes to the
/// nearest ANSI slot (often RED), and because iocraft only brackets the
/// bg-reset on rows that *carry* a background, the still-active color bleeds via
/// background-color-erase onto the unguarded rows below, painting multiple
/// full-width red bars. A pointer + foreground color has none of that: nothing
/// to quantize, nothing to leak.
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
    // claude-code `ListItem`: `figures.pointer` (❯) on the focused row, a 2-col
    // blank otherwise so the columns stay aligned.
    let pointer = if selected {
        "\u{276F} ".to_string()
    } else {
        "  ".to_string()
    };
    // Connected/active markers keep their own color even when focused (focus and
    // state stack independently, as in claude-code); the label takes the accent
    // on the focused row.
    let marker_color = match marker {
        PopupMarker::Check => theme.success,
        PopupMarker::Dot => theme.suggestion,
        PopupMarker::None => theme.dim,
    };
    let label_owned = label.to_string();
    // Focused row → accent (`suggestion`); other rows → terminal DEFAULT fg (no
    // explicit color), so labels stay visible on light terminals (claude-code
    // `ListItem` renders unfocused rows with `color={undefined}`).
    let label_el: AnyElement<'static> = if selected {
        element! { Text(content: label_owned, color: theme.suggestion.to_iocraft()) }.into_any()
    } else {
        element! { Text(content: label_owned) }.into_any()
    };
    let detail_owned = if detail.is_empty() {
        String::new()
    } else {
        format!("  {detail}")
    };
    let badge_owned = badge.to_string();
    let has_badge = !badge.is_empty();
    element! {
        View(width: 100pct, flex_direction: FlexDirection::Row, justify_content: JustifyContent::SpaceBetween) {
            View(flex_direction: FlexDirection::Row) {
                Text(content: pointer, color: theme.suggestion.to_iocraft())
                Text(content: glyph, color: marker_color.to_iocraft())
                #(std::iter::once(label_el))
                #((!detail_owned.is_empty()).then(|| element! { Text(content: detail_owned.clone(), color: theme.dim.to_iocraft()) }))
            }
            #(has_badge.then(|| element! { Text(content: badge_owned.clone(), color: theme.suggestion.to_iocraft()) }))
        }
    }
    .into_any()
}
