//! Theme picker screen (claude-code `ThemePicker.tsx` parity).
//!
//! Reached via the `/theme` command → `Screen::Theme`. Arrow keys move the
//! highlight and *live-preview* the theme on `AppState` (the whole UI
//! re-renders because `app::render_screen` reads `AppState.theme` every frame);
//! Enter commits the setting; Esc cancels and restores the setting that was
//! active on open.
//!
//! Literal-lock (byte-for-byte from claude-code `ThemePicker.tsx`): the seven
//! option labels in [`OPTION_LABELS`], the bold `Theme` header, the
//! `Choose the text style that looks best with your terminal` sub-header, the
//! `❯ ` pointer on the highlighted row, and a `StructuredDiff` preview (reusing
//! M7-02 `render::diff`) over the locked `greet()` snippet.
#![forbid(unsafe_code)]

use crossterm::event::{KeyCode, KeyEvent};
use iocraft::prelude::*;

use crate::render::diff;
use crate::state::AppState;
use crate::theme::{theme_for, Theme, ThemeName, ThemeSetting};

/// Locked option labels (claude-code `ThemePicker.tsx`), aligned 1:1 with
/// [`ThemePickerState::OPTIONS`].
pub const OPTION_LABELS: [&str; 7] = [
    "Auto (match terminal)",
    "Dark mode",
    "Light mode",
    "Dark mode (colorblind-friendly)",
    "Light mode (colorblind-friendly)",
    "Dark mode (ANSI colors only)",
    "Light mode (ANSI colors only)",
];

/// Locked bold header (claude-code `ThemePicker.tsx`, non-onboarding form).
pub const HEADER: &str = "Theme";
/// Locked sub-header.
pub const SUB_HEADER: &str = "Choose the text style that looks best with your terminal";

/// The fixed preview snippet (claude-code `ThemePicker.tsx`): a one-line
/// `console.log` change rendered as a `StructuredDiff`. `OLD` is the removed
/// line, `NEW` the added line.
pub const PREVIEW_OLD: &str = "function greet() {\n  console.log(\"Hello, World!\");\n}\n";
/// New (added) side of the locked preview snippet.
pub const PREVIEW_NEW: &str = "function greet() {\n  console.log(\"Hello, Claude!\");\n}\n";

/// What the key handler tells the caller to do with the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemePickerOutcome {
    /// Stay open (highlight moved / preview changed / inert key).
    Stay,
    /// Enter — committed the highlighted setting; close the screen.
    Commit,
    /// Esc — restored the prior setting; close the screen.
    Cancel,
}

/// Picker state: the highlight index + the setting to restore on cancel.
///
/// The option list itself is the static [`Self::OPTIONS`]; only the highlight
/// and the restore-on-cancel setting vary, so the state stays tiny and
/// `Clone`-cheap (carried inline in `Screen::Theme`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemePickerState {
    /// Highlighted index into [`Self::OPTIONS`] / [`OPTION_LABELS`].
    pub highlighted: usize,
    /// Setting active when the picker opened (restored on Esc).
    prior: ThemeSetting,
}

impl ThemePickerState {
    /// Option values, in `THEME_SETTINGS` order (auto first).
    pub const OPTIONS: [ThemeSetting; 7] = ThemeSetting::ALL;

    /// Open the picker focused on the currently-active setting.
    #[must_use]
    pub fn new(current: ThemeSetting) -> Self {
        let highlighted = Self::OPTIONS
            .iter()
            .position(|o| *o == current)
            .unwrap_or(0);
        Self {
            highlighted,
            prior: current,
        }
    }

    /// The setting active when the picker opened (restored on cancel).
    #[must_use]
    pub fn prior(&self) -> ThemeSetting {
        self.prior
    }

    /// The setting currently under the highlight.
    #[must_use]
    pub fn highlighted_setting(&self) -> ThemeSetting {
        Self::OPTIONS[self.highlighted]
    }

    fn preview(&self, app: &mut AppState) {
        // Preview mutates the active palette but NOT the stored preference,
        // so a cancel can cleanly restore via `set_theme(prior)`.
        app.theme = theme_for(self.highlighted_setting().resolve());
    }
}

impl Default for ThemePickerState {
    /// Default picker = focused on `Auto` (the first option), restoring to
    /// `Auto` on cancel. Convenience for the `/theme` open when the caller does
    /// not pre-seed the current setting.
    fn default() -> Self {
        Self::new(ThemeSetting::Auto)
    }
}

/// Handle one key in the picker. Pure state transition; the caller owns
/// persistence + closing the screen.
///
/// `app` is mutated for the LIVE PREVIEW (`app.theme` follows the highlight)
/// and, on commit/cancel, for the final `set_theme`. The picker `state` is a
/// separate borrow from `app` — the live key path takes the screen state OUT of
/// `active_screen` before calling this, so there is no double-mut-borrow.
pub fn theme_picker_handle_key(
    state: &mut ThemePickerState,
    app: &mut AppState,
    key: KeyEvent,
) -> ThemePickerOutcome {
    match key.code {
        KeyCode::Up => {
            state.highlighted = state.highlighted.saturating_sub(1);
            state.preview(app);
            ThemePickerOutcome::Stay
        }
        KeyCode::Down => {
            let last = ThemePickerState::OPTIONS.len() - 1;
            state.highlighted = (state.highlighted + 1).min(last);
            state.preview(app);
            ThemePickerOutcome::Stay
        }
        KeyCode::Enter => {
            app.set_theme(state.highlighted_setting());
            ThemePickerOutcome::Commit
        }
        KeyCode::Esc | KeyCode::Char('q') => {
            // Restore the palette + stored preference that were active on open.
            app.set_theme(state.prior);
            ThemePickerOutcome::Cancel
        }
        _ => ThemePickerOutcome::Stay,
    }
}

/// Pure render oracle: header + sub-header + option rows (`❯ ` pointer on the
/// highlight) + the locked preview snippet (diff plain text). Used by the
/// snapshot/behavior tests.
#[must_use]
pub fn render_theme_picker_to_string(state: &ThemePickerState) -> String {
    let mut out = format!("{HEADER}\n{SUB_HEADER}\n");
    for (i, label) in OPTION_LABELS.iter().enumerate() {
        let pointer = if i == state.highlighted {
            "\u{276F} "
        } else {
            "  "
        };
        out.push_str(pointer);
        out.push_str(label);
        out.push('\n');
    }
    // Preview: the locked diff snippet flattened to plain text (the colors live
    // in the component; the oracle keeps the text for layout regressions).
    let preview = diff::render(PREVIEW_OLD, PREVIEW_NEW, Some("greet.js"), ThemeName::Dark);
    for line in preview {
        out.push_str(&line.plain_text());
        out.push('\n');
    }
    out
}

/// Props for [`ThemePickerScreen`].
#[derive(Default, Props)]
pub struct ThemePickerScreenProps {
    /// The picker state (highlight + restore setting). Cloned from
    /// `active_screen`.
    pub state: ThemePickerState,
    /// The live (highlighted/previewed) palette. Colors the preview diff +
    /// header. Comes from `AppState.theme`.
    pub theme: Theme,
    /// The live theme name — drives the syntect `.tmTheme` of the preview's
    /// code coloring (Task 6).
    pub theme_name: ThemeName,
}

/// iocraft component: bold header, sub-header, the seven option rows
/// (`❯ ` pointer + `permission`-accent on the highlight), and the locked
/// `StructuredDiff` preview rendered under the live theme.
#[component]
pub fn ThemePickerScreen(props: &ThemePickerScreenProps) -> impl Into<AnyElement<'static>> {
    let state = props.state.clone();
    let theme = props.theme;
    let highlighted = state.highlighted;

    // Option rows.
    let rows: Vec<(String, bool)> = OPTION_LABELS
        .iter()
        .enumerate()
        .map(|(i, label)| {
            let pointer = if i == highlighted { "\u{276F} " } else { "  " };
            (format!("{pointer}{label}"), i == highlighted)
        })
        .collect();

    // Preview: the locked diff snippet rendered as a `StructuredDiff`. The
    // active `theme_name` flows into the syntect highlighter so the preview
    // code recolors live with the highlighted theme.
    let preview = diff::render(PREVIEW_OLD, PREVIEW_NEW, Some("greet.js"), props.theme_name);
    let preview_rows: Vec<AnyElement<'static>> = preview
        .into_iter()
        .map(|line| {
            let spans: Vec<AnyElement<'static>> = line
                .spans
                .into_iter()
                .map(|s| {
                    let fg = s.style.fg.to_iocraft();
                    let bg = s.style.bg.to_iocraft();
                    let weight = if s.style.bold {
                        Weight::Bold
                    } else {
                        Weight::Normal
                    };
                    element! {
                        View(background_color: bg) {
                            Text(content: s.text, color: fg, weight: weight)
                        }
                    }
                    .into_any()
                })
                .collect();
            element! { View(flex_direction: FlexDirection::Row) { #(spans) } }.into_any()
        })
        .collect();

    element! {
        View(flex_direction: FlexDirection::Column, padding: 1) {
            Text(content: HEADER, color: theme.permission, weight: Weight::Bold)
            Text(content: SUB_HEADER, color: theme.dim, weight: Weight::Bold)
            View(flex_direction: FlexDirection::Column, padding_top: 1) {
                #(rows.into_iter().map(|(line, sel)| {
                    let color = if sel { theme.suggestion } else { theme.dim };
                    element! { Text(content: line, color: color) }
                }))
            }
            View(
                flex_direction: FlexDirection::Column,
                border_style: BorderStyle::Round,
                padding: 1,
                margin_top: 1,
            ) {
                #(preview_rows)
            }
            View(margin_top: 1) {
                Text(content: "Up/Down select   Enter apply   Esc cancel".to_string(), color: theme.dim)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_align_with_options() {
        assert_eq!(OPTION_LABELS.len(), ThemePickerState::OPTIONS.len());
    }

    #[test]
    fn options_are_theme_settings_in_order() {
        assert_eq!(ThemePickerState::OPTIONS, ThemeSetting::ALL);
        assert_eq!(ThemePickerState::OPTIONS[0], ThemeSetting::Auto);
        assert_eq!(
            ThemePickerState::OPTIONS[2],
            ThemeSetting::Named(ThemeName::Light)
        );
    }

    #[test]
    fn new_focuses_current_setting() {
        let st = ThemePickerState::new(ThemeSetting::Named(ThemeName::Light));
        assert_eq!(st.highlighted, 2);
        assert_eq!(st.prior(), ThemeSetting::Named(ThemeName::Light));
    }

    #[test]
    fn render_marks_highlighted_row() {
        let st = ThemePickerState::new(ThemeSetting::Named(ThemeName::Light));
        let s = render_theme_picker_to_string(&st);
        assert!(s.contains("\u{276F} Light mode\n"));
        assert!(s.starts_with("Theme\n"));
        assert!(s.contains(SUB_HEADER));
        // Preview diff carries both sides of the locked snippet.
        assert!(s.contains("Hello, World!"));
        assert!(s.contains("Hello, Claude!"));
    }
}
