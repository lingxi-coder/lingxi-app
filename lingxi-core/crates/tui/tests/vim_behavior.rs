//! M7-08 exhaustive vim behavior tests. High-risk area (M7 design §4 R1):
//! motion correctness is table-driven so the matrix is scannable and dense.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use lingxi_tui::components::prompt_input::vim::{
    handle_vim_key, Register, VimEffect, VimMode, VimOutcome, VimState,
};

/// Build a `KeyEvent` for a single char. 'G' carries SHIFT (so map back-ends
/// that inspect modifiers behave like the real terminal).
fn k(c: char) -> KeyEvent {
    let mods = if c.is_uppercase() {
        KeyModifiers::SHIFT
    } else {
        KeyModifiers::NONE
    };
    KeyEvent::new(KeyCode::Char(c), mods)
}

/// Drive a key sequence in NORMAL mode from (text, offset), applying each
/// effect between keys. Returns the final (text, offset).
fn run_normal(text: &str, offset: usize, keys: &str) -> (String, usize) {
    let mut state = VimState {
        mode: VimMode::Normal,
        ..VimState::default()
    };
    let mut buf = text.to_string();
    let mut off = offset;
    for ch in keys.chars() {
        let key = if ch == '⎋' {
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)
        } else {
            k(ch)
        };
        match handle_vim_key(&mut state, &buf, off, key) {
            VimOutcome::Effect(VimEffect::Move(o)) => off = o.min(buf.len()),
            VimOutcome::Effect(VimEffect::Edit { text, cursor }) => {
                buf = text;
                off = cursor.min(buf.len());
            }
            VimOutcome::Effect(VimEffect::None) | VimOutcome::Pending | VimOutcome::PassThrough => {
            }
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
        ("hello", 0, "h", "hello", 0),   // clamp left
        ("hello", 5, "l", "hello", 5),   // clamp right
        ("ab\ncd", 0, "j", "ab\ncd", 3), // down to line2
        ("ab\ncd", 3, "k", "ab\ncd", 0), // up to line1
        ("abc", 1, "k", "abc", 1),       // up on line1 = no-op
        // --- w b e (incl. punctuation) ---
        ("foo bar baz", 0, "w", "foo bar baz", 4),
        ("foo bar baz", 0, "ww", "foo bar baz", 8),
        ("foo.bar", 0, "w", "foo.bar", 3), // land on '.'
        ("foo.bar", 3, "w", "foo.bar", 4), // '.' -> 'bar'
        ("foo bar", 0, "e", "foo bar", 2), // end of 'foo'
        ("foo bar", 6, "b", "foo bar", 4), // b from 'r' -> start 'bar'
        // --- 0 ^ $ ---
        ("  hello", 4, "0", "  hello", 0),
        ("  hello", 4, "^", "  hello", 2),
        ("hello", 0, "$", "hello", 5),
        ("ab\ncd", 0, "$", "ab\ncd", 2), // $ on logical line0
        // --- gg G ---
        ("a\nb\nc", 4, "gg", "a\nb\nc", 0),
        ("a\nb\nc", 0, "G", "a\nb\nc", 4),
        ("a\nb\nc", 0, "2gg", "a\nb\nc", 2), // Ngg -> line N
        // --- f t ---
        ("abcdc", 0, "fc", "abcdc", 2),
        ("abcdc", 0, "2fc", "abcdc", 4),
        ("abcdc", 0, "tc", "abcdc", 1),
        ("abc", 0, "fz", "abc", 0), // not found -> no move
        // --- counts ---
        ("a b c d e", 0, "3w", "a b c d e", 6), // 4th word 'd'
        ("hello", 0, "3l", "hello", 3),
        ("ab\ncd\nef", 0, "2j", "ab\ncd\nef", 6), // down twice -> line3
    ];
    for (i, (text, off, keys, want_text, want_off)) in cases.iter().enumerate() {
        let (got_text, got_off) = run_normal(text, *off, keys);
        assert_eq!(
            &got_text, want_text,
            "case {i}: text after {keys:?} on {text:?}"
        );
        assert_eq!(
            got_off, *want_off,
            "case {i}: offset after {keys:?} on {text:?}"
        );
    }
}

#[test]
fn mode_transition_matrix() {
    // (start_text, start_offset, keys-in-normal, expected after entering insert+effect)
    let cases: &[(&str, usize, char, &str, usize)] = &[
        ("hello", 2, 'i', "hello", 2),     // i: before cursor
        ("hello", 2, 'a', "hello", 3),     // a: after cursor
        ("hello", 5, 'a', "hello", 5),     // a at end: stays
        ("  hi", 3, 'I', "  hi", 2),       // I: first non-blank
        ("ab\ncd", 0, 'A', "ab\ncd", 2),   // A: end of line
        ("ab\ncd", 1, 'o', "ab\n\ncd", 3), // o: new line below
        ("ab\ncd", 3, 'O', "ab\n\ncd", 3), // O: new line above
    ];
    for (i, (text, off, key, want_text, want_off)) in cases.iter().enumerate() {
        let mut state = VimState {
            mode: VimMode::Normal,
            ..VimState::default()
        };
        let outcome = handle_vim_key(&mut state, text, *off, k(*key));
        assert_eq!(state.mode, VimMode::Insert, "case {i}: should enter Insert");
        let (got_text, got_off) = match outcome {
            VimOutcome::Effect(VimEffect::Move(o)) => ((*text).to_string(), o),
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
    let out = handle_vim_key(
        &mut state,
        "hello",
        5,
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
    );
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
use lingxi_tui::state::{AppState, StatusSnapshot};

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
/// A live (iocraft) char key carrying the given modifiers.
fn live_char_mods(c: char, mods: iocraft::KeyModifiers) -> iocraft::KeyEvent {
    let mut ev = iocraft::KeyEvent::new(iocraft::KeyEventKind::Press, iocraft::KeyCode::Char(c));
    ev.modifiers = mods;
    ev
}
/// Ctrl-Alt-V — the `KeyAction::ToggleVim` binding.
fn live_ctrl_alt_v() -> iocraft::KeyEvent {
    live_char_mods(
        'v',
        iocraft::KeyModifiers::CONTROL | iocraft::KeyModifiers::ALT,
    )
}
/// Ctrl-C — the `KeyAction::Cancel` binding.
fn live_ctrl_c() -> iocraft::KeyEvent {
    live_char_mods('c', iocraft::KeyModifiers::CONTROL)
}

// ===== (M7-08 review) vim toggle must be modal-independent =====

#[test]
fn toggle_vim_off_from_normal_mode() {
    // The reported usability bug: in Normal mode, Ctrl-Alt-V was swallowed by
    // the priority-4 vim branch and never reached the ToggleVim binding.
    let mut st = AppState::new(StatusSnapshot::default());
    st.vim_enabled = true;
    st.vim.mode = VimMode::Normal;
    handle_live_key(&mut st, &live_ctrl_alt_v(), 24);
    assert!(
        !st.vim_enabled,
        "Ctrl-Alt-V must toggle vim OFF from Normal"
    );
}

#[test]
fn toggle_vim_off_from_insert_mode() {
    // Regression guard: toggling off from Insert still works.
    let mut st = AppState::new(StatusSnapshot::default());
    st.vim_enabled = true;
    st.vim.mode = VimMode::Insert;
    handle_live_key(&mut st, &live_ctrl_alt_v(), 24);
    assert!(
        !st.vim_enabled,
        "Ctrl-Alt-V must toggle vim OFF from Insert"
    );
}

#[test]
fn ctrl_c_in_normal_mode_still_cancels() {
    // Normal mode previously swallowed Ctrl-C (returned Effect(None) and the
    // key never reached the Cancel binding). With a non-empty prompt, Cancel
    // clears the buffer — that is the observable contract here.
    let mut st = AppState::new(StatusSnapshot::default());
    st.vim_enabled = true;
    st.vim.mode = VimMode::Normal;
    st.prompt_text = "draft".into();
    st.prompt_cursor = 5;
    handle_live_key(&mut st, &live_ctrl_c(), 24);
    assert_eq!(st.prompt_text, "", "Ctrl-C in Normal must cancel/clear");
    assert_eq!(st.prompt_cursor, 0);
}

#[test]
fn plain_normal_motion_keys_still_move_cursor() {
    // Guard against over-broad pass-through: plain h/j/k/l in Normal must still
    // route to vim motion, not be inserted as text.
    let mut st = AppState::new(StatusSnapshot::default());
    st.vim_enabled = true;
    st.vim.mode = VimMode::Normal;
    st.prompt_text = "hello".into();
    st.prompt_cursor = 0;
    handle_live_key(&mut st, &live_char('l'), 24);
    assert_eq!(st.prompt_cursor, 1, "'l' moves right");
    handle_live_key(&mut st, &live_char('l'), 24);
    assert_eq!(st.prompt_cursor, 2);
    handle_live_key(&mut st, &live_char('h'), 24);
    assert_eq!(st.prompt_cursor, 1, "'h' moves left");
    assert_eq!(st.prompt_text, "hello", "motion keys must not edit text");
}

#[test]
fn permission_dialog_wins_over_ctrl_alt_v() {
    // Priority order guard: a pending permission (priority 1) must consume
    // Ctrl-Alt-V before the toggle ever fires. vim stays enabled.
    use lingxi_permission::gate::PermissionRequest;
    let mut st = AppState::new(StatusSnapshot::default());
    st.vim_enabled = true;
    st.vim.mode = VimMode::Normal;
    st.pending_permission = Some(lingxi_tui::state::PendingPermission {
        request: PermissionRequest::BypassPermissionsMode,
    });
    handle_live_key(&mut st, &live_ctrl_alt_v(), 24);
    assert!(
        st.vim_enabled,
        "permission focus-trap must consume the key; vim stays enabled"
    );
}

#[test]
fn open_palette_wins_over_ctrl_alt_v() {
    // Priority order guard: an open palette (priority 3) consumes Ctrl-Alt-V
    // before the vim branch / toggle. vim stays enabled.
    let mut st = AppState::new(StatusSnapshot::default());
    st.vim_enabled = true;
    st.vim.mode = VimMode::Normal;
    st.palette.open = true;
    handle_live_key(&mut st, &live_ctrl_alt_v(), 24);
    assert!(
        st.vim_enabled,
        "open palette must consume the key; vim stays enabled"
    );
}

#[test]
fn multiline_jk_cross_lines_via_live_key() {
    let mut st = AppState::new(StatusSnapshot::default());
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
    let mut st = AppState::new(StatusSnapshot::default());
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
    let mut st = AppState::new(StatusSnapshot::default());
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

// ===== Task 10: operator×motion matrix + register/paste roundtrips =====

#[test]
fn operator_motion_matrix() {
    // (start_text, start_offset, keys, expected_text, expected_offset)
    let cases: &[(&str, usize, &str, &str, usize)] = &[
        // ---- d × motions ----
        ("foo bar", 0, "dw", "bar", 0), // delete word + trailing space
        ("foo bar", 0, "de", " bar", 0), // delete to end of word (inclusive)
        ("foo bar", 4, "d$", "foo ", 3), // delete to end of line
        // d0 from the first 'l' (offset 4) deletes "  he" -> "llo" (cursor 0).
        ("  hello", 4, "d0", "llo", 0), // delete to line start (exclusive)
        ("foo bar baz", 0, "dl", "oo bar baz", 0), // dl == x: delete one char right
        ("abcde", 0, "dfc", "de", 0),   // delete through find 'c' (inclusive)
        ("abcde", 0, "dtc", "cde", 0),  // delete up-to 'c' (t: stops before)
        ("a\nb\nc", 0, "dj", "c", 0),   // linewise: delete lines 0..1
        ("a\nb\nc", 0, "dG", "", 0),    // linewise: delete to last line
        ("a\nb\nc", 4, "dgg", "", 0),   // linewise: delete to first line
        // ---- c × motions ----
        ("foo bar", 0, "cw", " bar", 0), // cw -> ce (end of word), enter insert
        // c$ deletes "bar" and enters Insert at `from`=4 (end of "foo "); unlike
        // d$ the cursor is NOT clamped to the last char — you are now typing.
        ("foo bar", 4, "c$", "foo ", 4), // change to EOL (Insert at offset 4)
        // ---- y (buffer unchanged; cursor moves) ----
        ("foo bar", 0, "yw", "foo bar", 0), // yank word: buffer unchanged
        ("foo bar", 4, "y$", "foo bar", 4), // yank to EOL: cursor at range start
        // ---- doubled ops ----
        ("a\nb\nc", 2, "dd", "a\nc", 2), // delete line1
        ("a\nb\nc", 0, "yy", "a\nb\nc", 0), // yank line: buffer unchanged
        ("ab\ncd", 0, "cc", "\ncd", 0),  // clear line, enter insert
        // ---- counts ----
        ("a b c d e", 0, "3dw", "d e", 0), // 3 words
        ("a b c d e", 0, "d3w", "d e", 0), // inner count, same result
        ("a\nb\nc\nd", 0, "2dd", "c\nd", 0), // 2 lines
        ("a\nb\nc", 0, "2yy", "a\nb\nc", 0), // yank 2 lines: buffer unchanged
    ];
    for (i, (text, off, keys, want_text, want_off)) in cases.iter().enumerate() {
        let (got_text, got_off) = run_normal(text, *off, keys);
        assert_eq!(&got_text, want_text, "case {i}: text after {keys:?} on {text:?}");
        assert_eq!(got_off, *want_off, "case {i}: offset after {keys:?} on {text:?}");
    }
}

/// Issue #2 (carry-forward from M7-08): operator endpoints at the BUFFER TAIL /
/// end-of-line / last word must produce correct (non-overflowing) ranges. These
/// lock that inclusive motions (`e`/`$`) extend exactly one char past the target
/// and never over-delete past `len`, and that `x` at the last char clamps right.
#[test]
fn operator_buffer_tail_matrix() {
    let cases: &[(&str, usize, &str, &str, usize)] = &[
        // de on the LAST word: cursor on 'b' of "bar", e lands on 'r' (last char,
        // offset 6, NOT len 7); inclusive +1 -> [4,7) deletes "bar" -> "foo ".
        ("foo bar", 4, "de", "foo ", 3),
        // dw on the LAST word: w from 'b' lands at len (no next word); range [4,7)
        // deletes "bar" -> "foo " (cursor clamps to last char of "foo " = 3).
        ("foo bar", 4, "dw", "foo ", 3),
        // d$ at end-of-line: $ lands at len for the last line; inclusive +1 is a
        // no-op at len (no overflow) -> [4,7) deletes "bar" -> "foo ".
        ("foo bar", 4, "d$", "foo ", 3),
        // de on a single trailing word that is the whole buffer.
        ("hello", 0, "de", "", 0),
        // x on the LAST char: delete 'o', cursor clamps left to 'l' (offset 3).
        ("hello", 4, "x", "hell", 3),
        // x on a 1-char buffer -> "" cursor 0 (max_off floors at 0).
        ("a", 0, "x", "", 0),
        // dd on the last line consumes the preceding '\n' (no orphan newline).
        ("a\nb\nc", 4, "dd", "a\nb", 2),
        // ye on the last word: yank "bar", buffer unchanged, cursor at range start.
        ("foo bar", 4, "ye", "foo bar", 4),
    ];
    for (i, (text, off, keys, want_text, want_off)) in cases.iter().enumerate() {
        let (got_text, got_off) = run_normal(text, *off, keys);
        assert_eq!(&got_text, want_text, "tail case {i}: text after {keys:?} on {text:?}");
        assert_eq!(got_off, *want_off, "tail case {i}: offset after {keys:?} on {text:?}");
    }
}

#[test]
fn yank_then_paste_roundtrip() {
    // yy then p: yank line0, paste below -> duplicated line.
    let (text, _off) = run_normal("hello\nworld", 0, "yyp");
    assert_eq!(text, "hello\nhello\nworld");
}

#[test]
fn delete_then_paste_charwise() {
    // x on "abc" -> "bc" reg "a"; p pastes "a" after cursor -> "bac".
    let (text, off) = run_normal("abc", 0, "xp");
    assert_eq!(text, "bac");
    assert_eq!(off, 1); // cursor on pasted 'a'
}

#[test]
fn register_holds_last_yank_or_delete() {
    // After dw the register holds "foo "; it survives a later paste elsewhere.
    let (text, _off) = run_normal("foo bar", 0, "dw$p");
    assert!(text.contains("foo "));
}

// ===== Task 11: visual-mode behavior (charwise + linewise via the seam) =====

/// Drive a key sequence starting in NORMAL, where `v`/`V` flips to Visual and
/// subsequent motions move the cursor (selection end). Applies each effect.
fn run_visual(text: &str, offset: usize, keys: &str) -> (String, usize, VimMode) {
    let mut state = VimState {
        mode: VimMode::Normal,
        ..VimState::default()
    };
    let mut buf = text.to_string();
    let mut off = offset;
    for ch in keys.chars() {
        let key = if ch == '⎋' {
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)
        } else {
            k(ch)
        };
        match handle_vim_key(&mut state, &buf, off, key) {
            VimOutcome::Effect(VimEffect::Move(o)) => off = o.min(buf.len()),
            VimOutcome::Effect(VimEffect::Edit { text, cursor }) => {
                buf = text;
                off = cursor.min(buf.len());
            }
            VimOutcome::Effect(VimEffect::None) | VimOutcome::Pending | VimOutcome::PassThrough => {}
        }
    }
    (buf, off, state.mode)
}

#[test]
fn visual_charwise_delete_matrix() {
    // (start_text, start_offset, keys, expected_text, expected_offset)
    let cases: &[(&str, usize, &str, &str, usize)] = &[
        ("hello", 0, "vlld", "lo", 0),    // v + ll (cursor->2) + d -> delete "hel"
        ("hello", 0, "vlly", "hello", 0), // yank: buffer unchanged, cursor to start
        ("hello", 0, "v$d", "", 0),       // v + $ + d -> delete whole line "hello"
        ("hello", 1, "vlld", "ho", 1),    // v from 1 + ll (cursor->3) + d -> delete "ell"
    ];
    for (i, (text, off, keys, want_text, want_off)) in cases.iter().enumerate() {
        let (got_text, got_off, mode) = run_visual(text, *off, keys);
        assert_eq!(&got_text, want_text, "case {i}: {keys:?} on {text:?}");
        assert_eq!(got_off, *want_off, "case {i}");
        // d/y return to Normal.
        assert_eq!(mode, VimMode::Normal, "case {i}: should be back in Normal");
    }
}

#[test]
fn visual_linewise_delete() {
    // V + j (cursor to line1) + d on "a\nb\nc": delete lines 0..1 -> "c".
    let (text, off, mode) = run_visual("a\nb\nc", 0, "Vjd");
    assert_eq!(text, "c");
    assert_eq!(off, 0);
    assert_eq!(mode, VimMode::Normal);
}

#[test]
fn visual_c_enters_insert() {
    let (text, _off, mode) = run_visual("hello", 0, "vlc");
    // v + l (cursor->1) + c -> delete "he" (inclusive of cursor char) -> "llo", Insert.
    assert_eq!(text, "llo");
    assert_eq!(mode, VimMode::Insert);
}

#[test]
fn visual_esc_returns_to_normal_no_edit() {
    let (text, _off, mode) = run_visual("hello", 0, "vll⎋");
    assert_eq!(text, "hello"); // no edit
    assert_eq!(mode, VimMode::Normal);
}

#[test]
fn visual_count_motion_then_delete() {
    // v + 2l (cursor->2) + d on "hello": delete "hel" -> "lo".
    let (text, _off, mode) = run_visual("hello", 0, "v2ld");
    assert_eq!(text, "lo");
    assert_eq!(mode, VimMode::Normal);
}

#[test]
fn visual_gg_and_cap_g_extend_selection() {
    // V then G (cursor to last line) then d on "a\nb\nc": delete all lines -> "".
    let (text, _off, mode) = run_visual("a\nb\nc", 0, "VGd");
    assert_eq!(text, "");
    assert_eq!(mode, VimMode::Normal);
    // V on last line then gg (cursor to first line) then d -> delete all -> "".
    let (text2, _off2, mode2) = run_visual("a\nb\nc", 4, "Vggd");
    assert_eq!(text2, "");
    assert_eq!(mode2, VimMode::Normal);
}

/// Direct register-content check: dw stores "foo " charwise; yy stores "a\n"
/// linewise. Confirms the register tagging the matrix relies on.
#[test]
fn register_linewise_tagging() {
    let mut state = VimState {
        mode: VimMode::Normal,
        ..VimState::default()
    };
    // dw -> charwise register "foo ".
    handle_vim_key(&mut state, "foo bar", 0, k('d'));
    handle_vim_key(&mut state, "foo bar", 0, k('w'));
    assert_eq!(
        state.register,
        Register {
            text: "foo ".into(),
            linewise: false
        }
    );
    // yy -> linewise register "a\n".
    let mut state2 = VimState {
        mode: VimMode::Normal,
        ..VimState::default()
    };
    handle_vim_key(&mut state2, "a\nb", 0, k('y'));
    handle_vim_key(&mut state2, "a\nb", 0, k('y'));
    assert_eq!(
        state2.register,
        Register {
            text: "a\n".into(),
            linewise: true
        }
    );
}
