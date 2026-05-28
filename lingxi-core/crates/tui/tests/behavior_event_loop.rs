//! Behavior test: feed `TuiEvent::Key(Esc)` and `Key(Ctrl-C)` events
//! through the keymap classifier and assert that `TuiApp` flips
//! `should_quit` correctly. This is the contract M6-02 will preserve
//! when it grows the event loop with real component dispatch.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use lingxi_tui::{
    events::keymap::{classify, KeyAction},
    TuiApp, TuiEvent,
};

#[test]
fn esc_does_not_quit_in_m6_01() {
    // M6-01: Esc is passthrough (Other). M6-05 wires Esc to permission
    // dialog deny; this test pins the M6-01 contract so M6-05 must
    // change it explicitly.
    let k = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
    let ev = TuiEvent::Key(k);
    match ev {
        TuiEvent::Key(k) => assert_eq!(classify(&k), KeyAction::Other),
        _ => panic!("expected Key"),
    }
}

#[test]
fn ctrl_c_classifies_as_quit() {
    let k = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    let ev = TuiEvent::Key(k);
    match ev {
        TuiEvent::Key(k) => assert_eq!(classify(&k), KeyAction::Quit),
        _ => panic!("expected Key"),
    }
}

#[test]
fn ctrl_d_classifies_as_quit() {
    let k = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL);
    let ev = TuiEvent::Key(k);
    match ev {
        TuiEvent::Key(k) => assert_eq!(classify(&k), KeyAction::Quit),
        _ => panic!("expected Key"),
    }
}

#[test]
fn quit_action_flips_app_state() {
    let mut app = TuiApp::new();
    assert!(!app.should_quit);
    let k = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    if classify(&k) == KeyAction::Quit {
        app.request_quit();
    }
    assert!(app.should_quit);
}
