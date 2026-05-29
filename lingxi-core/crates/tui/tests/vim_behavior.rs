//! M7-08 exhaustive vim behavior tests. High-risk area (M7 design §4 R1):
//! motion correctness is table-driven so the matrix is scannable and dense.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use lingxi_tui::components::prompt_input::vim::{
    handle_vim_key, VimEffect, VimMode, VimOutcome, VimState,
};

/// Build a KeyEvent for a single char. 'G' carries SHIFT (so map back-ends
/// that inspect modifiers behave like the real terminal).
fn k(c: char) -> KeyEvent {
    let mods = if c.is_uppercase() { KeyModifiers::SHIFT } else { KeyModifiers::NONE };
    KeyEvent::new(KeyCode::Char(c), mods)
}

/// Drive a key sequence in NORMAL mode from (text, offset), applying each
/// effect between keys. Returns the final (text, offset).
fn run_normal(text: &str, offset: usize, keys: &str) -> (String, usize) {
    let mut state = VimState { mode: VimMode::Normal, ..VimState::default() };
    let mut buf = text.to_string();
    let mut off = offset;
    for ch in keys.chars() {
        let key = if ch == '⎋' { KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE) } else { k(ch) };
        match handle_vim_key(&mut state, &buf, off, key) {
            VimOutcome::Effect(VimEffect::Move(o)) => off = o.min(buf.len()),
            VimOutcome::Effect(VimEffect::Edit { text, cursor }) => {
                buf = text;
                off = cursor.min(buf.len());
            }
            VimOutcome::Effect(VimEffect::None) | VimOutcome::Pending | VimOutcome::PassThrough => {}
        }
    }
    (buf, off)
}

#[test]
fn motion_matrix() {
    // (start_text, start_offset, keys, expected_text, expected_offset)
    let cases: &[(&str, usize, &str, &str, usize)] = &[
        // --- h j k l bounds ---
        ("hello", 2, "l", "hello", 3),
        ("hello", 2, "h", "hello", 1),
        ("hello", 0, "h", "hello", 0),            // clamp left
        ("hello", 5, "l", "hello", 5),            // clamp right
        ("ab\ncd", 0, "j", "ab\ncd", 3),          // down to line2
        ("ab\ncd", 3, "k", "ab\ncd", 0),          // up to line1
        ("abc", 1, "k", "abc", 1),                // up on line1 = no-op
        // --- w b e (incl. punctuation) ---
        ("foo bar baz", 0, "w", "foo bar baz", 4),
        ("foo bar baz", 0, "ww", "foo bar baz", 8),
        ("foo.bar", 0, "w", "foo.bar", 3),        // land on '.'
        ("foo.bar", 3, "w", "foo.bar", 4),        // '.' -> 'bar'
        ("foo bar", 0, "e", "foo bar", 2),        // end of 'foo'
        ("foo bar", 8.min(6), "b", "foo bar", 4), // b from 'r' -> start 'bar'
        // --- 0 ^ $ ---
        ("  hello", 4, "0", "  hello", 0),
        ("  hello", 4, "^", "  hello", 2),
        ("hello", 0, "$", "hello", 5),
        ("ab\ncd", 0, "$", "ab\ncd", 2),          // $ on logical line0
        // --- gg G ---
        ("a\nb\nc", 4, "gg", "a\nb\nc", 0),
        ("a\nb\nc", 0, "G", "a\nb\nc", 4),
        ("a\nb\nc", 0, "2gg", "a\nb\nc", 2),       // Ngg -> line N
        // --- f t ---
        ("abcdc", 0, "fc", "abcdc", 2),
        ("abcdc", 0, "2fc", "abcdc", 4),
        ("abcdc", 0, "tc", "abcdc", 1),
        ("abc", 0, "fz", "abc", 0),               // not found -> no move
        // --- counts ---
        ("a b c d e", 0, "3w", "a b c d e", 6),   // 4th word 'd'
        ("hello", 0, "3l", "hello", 3),
        ("ab\ncd\nef", 0, "2j", "ab\ncd\nef", 6), // down twice -> line3
    ];
    for (i, (text, off, keys, want_text, want_off)) in cases.iter().enumerate() {
        let (got_text, got_off) = run_normal(text, *off, keys);
        assert_eq!(&got_text, want_text, "case {i}: text after {keys:?} on {text:?}");
        assert_eq!(got_off, *want_off, "case {i}: offset after {keys:?} on {text:?}");
    }
}

#[test]
fn mode_transition_matrix() {
    // (start_text, start_offset, keys-in-normal, expected after entering insert+effect)
    let cases: &[(&str, usize, char, &str, usize)] = &[
        ("hello", 2, 'i', "hello", 2),  // i: before cursor
        ("hello", 2, 'a', "hello", 3),  // a: after cursor
        ("hello", 5, 'a', "hello", 5),  // a at end: stays
        ("  hi", 3, 'I', "  hi", 2),    // I: first non-blank
        ("ab\ncd", 0, 'A', "ab\ncd", 2),// A: end of line
        ("ab\ncd", 1, 'o', "ab\n\ncd", 3), // o: new line below
        ("ab\ncd", 3, 'O', "ab\n\ncd", 3), // O: new line above
    ];
    for (i, (text, off, key, want_text, want_off)) in cases.iter().enumerate() {
        let mut state = VimState { mode: VimMode::Normal, ..VimState::default() };
        let outcome = handle_vim_key(&mut state, text, *off, k(*key));
        assert_eq!(state.mode, VimMode::Insert, "case {i}: should enter Insert");
        let (got_text, got_off) = match outcome {
            VimOutcome::Effect(VimEffect::Move(o)) => (text.to_string(), o),
            VimOutcome::Effect(VimEffect::Edit { text, cursor }) => (text, cursor),
            other => panic!("case {i}: unexpected {other:?}"),
        };
        assert_eq!(&got_text, want_text, "case {i}");
        assert_eq!(got_off, *want_off, "case {i}");
    }
}

#[test]
fn esc_returns_to_normal_and_clamps() {
    let mut state = VimState::default(); // Insert
    let out = handle_vim_key(&mut state, "hello", 5, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(state.mode, VimMode::Normal);
    assert_eq!(out, VimOutcome::Effect(VimEffect::Move(4)));
}

#[test]
fn dollar_on_wrapped_line_is_logical_line_end() {
    // A long single logical line that would wrap on screen: $ goes to its end
    // regardless of display wrap (M7-08 logical-line semantics).
    let long = "the quick brown fox jumps over the lazy dog again and again";
    let (_t, off) = run_normal(long, 5, "$");
    assert_eq!(off, long.len());
}

// ===== Task 12: full handle_live_key seam (multi-line + vim-disabled) =====

use lingxi_tui::root::handle_live_key;
use lingxi_tui::state::AppState;

// iocraft's KeyEvent is `KeyEvent::new(kind, code)` with a public `modifiers`
// field; build a Press event for a char, carrying SHIFT for capitals.
fn live_char(c: char) -> iocraft::KeyEvent {
    let mut ev = iocraft::KeyEvent::new(iocraft::KeyEventKind::Press, iocraft::KeyCode::Char(c));
    if c.is_uppercase() {
        ev.modifiers = iocraft::KeyModifiers::SHIFT;
    }
    ev
}
fn live_esc() -> iocraft::KeyEvent {
    iocraft::KeyEvent::new(iocraft::KeyEventKind::Press, iocraft::KeyCode::Esc)
}

#[test]
fn multiline_jk_cross_lines_via_live_key() {
    let mut st = AppState::new(Default::default());
    st.vim_enabled = true;
    st.vim.mode = VimMode::Normal;
    st.prompt_text = "abc\ndef\nghi".into();
    st.prompt_cursor = 1; // line0 col1
    handle_live_key(&mut st, &live_char('j'), 24);
    assert_eq!(st.prompt_cursor, 5); // line1 col1 = 'e'
    handle_live_key(&mut st, &live_char('j'), 24);
    assert_eq!(st.prompt_cursor, 9); // line2 col1 = 'h'
    handle_live_key(&mut st, &live_char('k'), 24);
    assert_eq!(st.prompt_cursor, 5);
    handle_live_key(&mut st, &live_char('$'), 24);
    assert_eq!(st.prompt_cursor, 7); // end of "def"
}

#[test]
fn insert_mode_typing_flows_through_passthrough() {
    let mut st = AppState::new(Default::default());
    st.vim_enabled = true;
    st.vim.mode = VimMode::Normal;
    st.prompt_text = "ac".into();
    st.prompt_cursor = 1; // on 'c'
    handle_live_key(&mut st, &live_char('i'), 24); // enter insert before 'c'
    assert_eq!(st.vim.mode, VimMode::Insert);
    handle_live_key(&mut st, &live_char('b'), 24); // type 'b'
    assert_eq!(st.prompt_text, "abc");
    assert_eq!(st.prompt_cursor, 2);
    handle_live_key(&mut st, &live_esc(), 24); // back to Normal
    assert_eq!(st.vim.mode, VimMode::Normal);
}

#[test]
fn vim_disabled_is_unchanged_m6_editing() {
    let mut st = AppState::new(Default::default());
    st.vim_enabled = false; // OFF
    st.prompt_text = String::new();
    st.prompt_cursor = 0;
    // Type "ihj" — with vim OFF these are literal inserts, NOT vim commands.
    for c in ['i', 'h', 'j'] {
        handle_live_key(&mut st, &live_char(c), 24);
    }
    assert_eq!(st.prompt_text, "ihj");
    assert_eq!(st.prompt_cursor, 3);
}
