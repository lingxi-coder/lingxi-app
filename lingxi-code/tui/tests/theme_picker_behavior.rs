//! M7-15 Task 4/5 — theme picker behavior: highlight/preview/commit/cancel +
//! the `/theme` command open path.

use crossterm::event::{KeyCode, KeyEvent};
use tui::screens::theme::{theme_picker_handle_key, ThemePickerOutcome, ThemePickerState};
use tui::screens::Screen;
use tui::state::AppState;
use tui::theme::{theme_for, Theme, ThemeName, ThemeSetting};

fn key(c: KeyCode) -> KeyEvent {
    KeyEvent::new(c, crossterm::event::KeyModifiers::NONE)
}

#[test]
fn down_arrow_moves_highlight_and_live_previews() {
    let mut app = AppState::default_for_tests();
    let mut st = ThemePickerState::new(app.theme_setting); // starts on current setting
    let start = st.highlighted;
    let out = theme_picker_handle_key(&mut st, &mut app, key(KeyCode::Down));
    assert_eq!(out, ThemePickerOutcome::Stay);
    assert_eq!(st.highlighted, start + 1);
    // Live preview: app.theme now reflects the highlighted option (not yet the
    // committed setting).
    let previewed = ThemePickerState::OPTIONS[st.highlighted];
    assert_eq!(app.theme, theme_for(previewed.resolve()));
}

#[test]
fn up_arrow_clamps_at_top() {
    let mut app = AppState::default_for_tests();
    let mut st = ThemePickerState::new(ThemeSetting::Auto); // highlight 0
    let out = theme_picker_handle_key(&mut st, &mut app, key(KeyCode::Up));
    assert_eq!(out, ThemePickerOutcome::Stay);
    assert_eq!(st.highlighted, 0); // saturating_sub keeps it at the top
}

#[test]
fn down_arrow_clamps_at_bottom() {
    let mut app = AppState::default_for_tests();
    let mut st = ThemePickerState::new(ThemeSetting::Named(ThemeName::DarkAnsi)); // last
    let last = ThemePickerState::OPTIONS.len() - 1;
    assert_eq!(st.highlighted, last);
    let _ = theme_picker_handle_key(&mut st, &mut app, key(KeyCode::Down));
    assert_eq!(st.highlighted, last); // clamped
}

#[test]
fn enter_commits_highlighted_setting() {
    let mut app = AppState::default_for_tests();
    let mut st = ThemePickerState::new(app.theme_setting);
    // Move to "light" (index 2 in OPTIONS: auto, dark, light, ...).
    st.highlighted = 2;
    let out = theme_picker_handle_key(&mut st, &mut app, key(KeyCode::Enter));
    assert_eq!(out, ThemePickerOutcome::Commit);
    assert_eq!(app.theme_setting, ThemeSetting::Named(ThemeName::Light));
    assert_eq!(app.theme, Theme::light());
}

#[test]
fn esc_cancels_and_restores_prior_theme() {
    let mut app = AppState::default_for_tests();
    let prior = app.theme_setting;
    let mut st = ThemePickerState::new(prior);
    theme_picker_handle_key(&mut st, &mut app, key(KeyCode::Down)); // preview drift
    let out = theme_picker_handle_key(&mut st, &mut app, key(KeyCode::Esc));
    assert_eq!(out, ThemePickerOutcome::Cancel);
    // Restored to what it was before opening.
    assert_eq!(app.theme_setting, prior);
    assert_eq!(app.theme, theme_for(prior.resolve()));
}

#[test]
fn q_cancels_like_esc() {
    let mut app = AppState::default_for_tests();
    let prior = app.theme_setting;
    let mut st = ThemePickerState::new(prior);
    theme_picker_handle_key(&mut st, &mut app, key(KeyCode::Down));
    let out = theme_picker_handle_key(&mut st, &mut app, key(KeyCode::Char('q')));
    assert_eq!(out, ThemePickerOutcome::Cancel);
    assert_eq!(app.theme_setting, prior);
}

#[test]
fn slash_theme_opens_picker_screen() {
    let mut app = AppState::default_for_tests();
    // Drive the same submit path the live REPL uses.
    app.prompt_text = "/theme".to_string();
    app.prompt_cursor = "/theme".len();
    let should_run = tui::app::dispatch(tui::events::keymap::KeyAction::Submit, &mut app);
    assert!(!should_run, "/theme opens a screen, never runs a turn");
    // The Theme screen is now active.
    assert!(matches!(app.active_screen.as_ref(), Some(Screen::Theme(_))));
    // It does not echo as a user message.
    assert!(
        app.messages.is_empty(),
        "/theme must not echo as a user message"
    );
}

#[test]
fn ctrl_t_toggles_syntax_highlighting_disabled() {
    // (theme-syntax-toggle) Ctrl+T flips both the picker state and AppState's
    // session-level `syntax_highlighting_disabled`; the picker stays open.
    use crossterm::event::KeyModifiers;
    let mut app = AppState::default_for_tests();
    app.open_theme_picker();
    let Some(Screen::Theme(mut st)) = app.active_screen.clone() else {
        panic!("theme picker open");
    };
    assert!(!st.syntax_disabled);
    assert!(!app.syntax_highlighting_disabled);

    let ctrl_t = KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL);
    let out = theme_picker_handle_key(&mut st, &mut app, ctrl_t);
    assert_eq!(out, ThemePickerOutcome::Stay, "Ctrl+T keeps the picker open");
    assert!(st.syntax_disabled, "picker state flipped");
    assert!(app.syntax_highlighting_disabled, "AppState flipped");

    // Toggling again restores it.
    theme_picker_handle_key(&mut st, &mut app, KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL));
    assert!(!st.syntax_disabled);
    assert!(!app.syntax_highlighting_disabled);
}
