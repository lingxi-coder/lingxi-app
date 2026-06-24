//! `PromptInputFooter` — the chrome below the editor: mode-indicator,
//! placeholder, the collapsed help hint, and a (M7-06 empty) suggestions area.
//! Live suggestion filtering arrives in M7-07.
//!
//! Literal lock (claude-code `PromptInput*`): the prompt glyph is `❯ `
//! (figures.pointer + space) and the help hint is `? for shortcuts`. The hint
//! is shown only when the buffer is empty and the vim mode-indicator is not
//! shown (claude-code `showHint = !suppressHint && !showVim`, with
//! `suppressHint = input.length > 0`). The `shift + ⏎ for newline` text lives
//! ONLY on the `?` help surface (`PromptInputHelpMenu`), never in the resting
//! footer.

use iocraft::prelude::*;

use crate::components::prompt_input::{mode_indicator, VimMode};
use crate::theme::TuiTheme;

/// The mode-indicator label for the footer. `None` when vim is disabled
/// (M6/default footer shows no mode line).
#[must_use]
pub fn footer_mode_label(vim_enabled: bool, mode: VimMode) -> Option<&'static str> {
    vim_enabled.then(|| mode_indicator(mode))
}

/// (M7-09) Mode-indicator label, distinguishing charwise vs linewise Visual.
/// `None` when vim is disabled. Supersedes [`footer_mode_label`] which cannot
/// tell `v` (`-- VISUAL --`) from `V` (`-- VISUAL LINE --`); the older fn is
/// retained for callers that don't track the linewise flag.
#[must_use]
pub fn footer_mode_label_v2(
    vim_enabled: bool,
    mode: VimMode,
    visual_linewise: bool,
) -> Option<&'static str> {
    if !vim_enabled {
        return None;
    }
    Some(match mode {
        VimMode::Normal => "-- NORMAL --",
        VimMode::Insert => "-- INSERT --",
        VimMode::Visual if visual_linewise => "-- VISUAL LINE --",
        VimMode::Visual => "-- VISUAL --",
    })
}

/// Which leading glyph the mode-indicator shows. M7-06 ships `Prompt`; the
/// `Bash` / vim-mode variants land in M7-07/M7-08.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FooterMode {
    /// Normal prompt mode — glyph `❯ `.
    #[default]
    Prompt,
    /// Bash mode — glyph `! ` (placeholder for M7-07; not yet keyboard-wired).
    Bash,
}

impl FooterMode {
    /// The leading glyph (claude-code PromptInputModeIndicator.tsx).
    #[must_use]
    pub fn glyph(self) -> &'static str {
        match self {
            FooterMode::Prompt => "❯ ",
            FooterMode::Bash => "! ",
        }
    }
}

/// Props for [`PromptInputFooter`].
///
/// `Default` is hand-rolled (not derived) because [`VimMode`] is a
/// contract-locked enum without a `Default` impl; the footer defaults its
/// mode to `Insert` (claude-code's initial vim mode) when vim is disabled the
/// label is suppressed anyway.
#[derive(Props)]
pub struct PromptInputFooterProps {
    /// Mode-indicator glyph selector.
    pub mode: FooterMode,
    /// Placeholder shown when the buffer is empty (None = no placeholder).
    pub placeholder: Option<String>,
    /// Whether the buffer is currently empty (drives placeholder visibility).
    pub is_empty: bool,
    /// (M7-08) Whether vim mode is enabled — gates the `-- MODE --` line.
    pub vim_enabled: bool,
    /// (M7-08) Current vim mode (only rendered when `vim_enabled`).
    pub vim_mode: VimMode,
    /// (M7-09) Whether the active Visual selection is linewise (`V`) vs charwise
    /// (`v`). Selects `-- VISUAL LINE --` over `-- VISUAL --`.
    pub vim_visual_linewise: bool,
}

impl Default for PromptInputFooterProps {
    fn default() -> Self {
        Self {
            mode: FooterMode::default(),
            placeholder: None,
            is_empty: false,
            vim_enabled: false,
            vim_mode: VimMode::Insert,
            vim_visual_linewise: false,
        }
    }
}

/// Render the footer: an optional `-- MODE --` row (vim), the
/// `[glyph][placeholder?]` row, and — only when the buffer is empty and no vim
/// mode-indicator is shown — a dim `? for shortcuts` hint row.
#[component]
pub fn PromptInputFooter(props: &PromptInputFooterProps) -> impl Into<AnyElement<'static>> {
    let glyph = props.mode.glyph().to_string();
    let placeholder = if props.is_empty {
        props.placeholder.clone().unwrap_or_default()
    } else {
        String::new()
    };
    // (M7-08/M7-09) Vim mode indicator: shown only when vim is enabled; the v2
    // form distinguishes `-- VISUAL --` from `-- VISUAL LINE --`.
    let mode_label =
        footer_mode_label_v2(props.vim_enabled, props.vim_mode, props.vim_visual_linewise)
            .map(str::to_string);
    // claude-code `showHint = !suppressHint && !showVim`: the `? for shortcuts`
    // hint is shown only when the buffer is empty (`suppressHint = input.length
    // > 0`) and no vim mode-indicator is rendered (`showVim`). The
    // `shift + ⏎ for newline` text never appears in the resting footer.
    let show_hint = props.is_empty && mode_label.is_none();
    element! {
        View(flex_direction: FlexDirection::Column) {
            #(mode_label.map(|label| element! {
                View(flex_direction: FlexDirection::Row) {
                    Text(content: label, color: TuiTheme::DIM)
                }
            }))
            View(flex_direction: FlexDirection::Row) {
                Text(content: glyph, color: TuiTheme::DIM)
                Text(content: placeholder, color: TuiTheme::DIM)
            }
            #(show_hint.then(|| element! {
                View(flex_direction: FlexDirection::Row) {
                    Text(content: "? for shortcuts".to_string(), color: TuiTheme::DIM)
                }
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glyph_prompt_is_pointer() {
        assert_eq!(FooterMode::Prompt.glyph(), "❯ ");
    }

    #[test]
    fn glyph_bash_is_bang() {
        assert_eq!(FooterMode::Bash.glyph(), "! ");
    }
}

#[cfg(test)]
mod vim_indicator_tests {
    use super::*;
    use crate::components::prompt_input::VimMode;

    #[test]
    fn indicator_shown_when_vim_enabled() {
        assert_eq!(
            footer_mode_label(true, VimMode::Normal),
            Some("-- NORMAL --")
        );
        assert_eq!(
            footer_mode_label(true, VimMode::Insert),
            Some("-- INSERT --")
        );
    }

    #[test]
    fn indicator_hidden_when_vim_disabled() {
        assert_eq!(footer_mode_label(false, VimMode::Normal), None);
    }
}

#[cfg(test)]
mod visual_indicator_tests {
    use super::*;
    use crate::components::prompt_input::VimMode;

    #[test]
    fn charwise_visual_label() {
        assert_eq!(
            footer_mode_label_v2(true, VimMode::Visual, false),
            Some("-- VISUAL --")
        );
    }

    #[test]
    fn linewise_visual_label() {
        assert_eq!(
            footer_mode_label_v2(true, VimMode::Visual, true),
            Some("-- VISUAL LINE --")
        );
    }

    #[test]
    fn normal_insert_labels_unchanged() {
        assert_eq!(
            footer_mode_label_v2(true, VimMode::Normal, false),
            Some("-- NORMAL --")
        );
        assert_eq!(
            footer_mode_label_v2(true, VimMode::Insert, false),
            Some("-- INSERT --")
        );
    }

    #[test]
    fn disabled_is_none() {
        assert_eq!(footer_mode_label_v2(false, VimMode::Visual, true), None);
    }
}
