use super::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_state_is_insert() {
        let s = VimState::default();
        assert_eq!(s.mode, VimMode::Insert);
        assert_eq!(s.command, CommandState::Idle);
        assert!(s.pending_operator.is_none());
    }

    #[test]
    fn mode_indicator_strings() {
        assert_eq!(mode_indicator(VimMode::Normal), "-- NORMAL --");
        assert_eq!(mode_indicator(VimMode::Insert), "-- INSERT --");
        assert_eq!(mode_indicator(VimMode::Visual), "-- VISUAL --");
    }

    #[test]
    fn m7_08_adds_no_telemetry_events() {
        // M7-08 ships 0 new telemetry events. The M7-16 audit (which IS the
        // "real M7 total" decision this guard deferred to) locked the registry
        // at 330: vim itself still registers NOTHING —
        // `tengu_tui_vim_mode_entered` stayed DEFERRED (Esc-from-Insert churn,
        // no aggregator). The +4 vs the 326 M6 baseline is M7-16's
        // screen_opened/screen_closed/search_opened + lingxi_core_v0_8_0_released
        // (none from vim). This guard fails if vim accidentally mints an event.
        // Baseline 330 − 6 (grep/glob fabricated events removed, #29) = 324
        // → 330 (CronDelete/CronList +6, LSP.7b) → 334 (FileRead analytics +4, W36/#13)
        // → 343 (config migrations +9) → 344 (permission flow +1) → 347 (coordinator swarm +3).
        // Strict-parity (2.1.195): −3 tengu_tool_todo_write_* (D1), −1 tengu_cost_recorded
        // (D2), −2 session-resume consolidation (D3) → 341.
        // cc 2.1.198 M2: +2 AWS auth-refresh trust-gate events → 343.
        assert_eq!(telemetry::tengu::ALL_EVENT_NAMES.len(), 343);
    }

    #[test]
    fn m7_09_adds_no_telemetry_events() {
        // M7-09 ships 0 new telemetry events. Registry locked at 330 by the
        // M7-16 audit; vim operators/visual emit nothing (per-keystroke
        // telemetry is explicitly NOT done; `vim_mode_entered` stayed deferred).
        // The +4 vs the 326 baseline is all M7-16 (screen/search + release
        // marker). This guard fails if vim registers a new event.
        // Baseline 330 − 6 (grep/glob fabricated events removed, #29) = 324
        // → 330 (CronDelete/CronList +6, LSP.7b) → 334 (FileRead analytics +4, W36/#13)
        // → 343 (config migrations +9) → 344 (permission flow +1) → 347 (coordinator swarm +3).
        // Strict-parity (2.1.195): −3 tengu_tool_todo_write_* (D1), −1 tengu_cost_recorded
        // (D2), −2 session-resume consolidation (D3) → 341.
        // cc 2.1.198 M2: +2 AWS auth-refresh trust-gate events → 343.
        assert_eq!(telemetry::tengu::ALL_EVENT_NAMES.len(), 343);
    }

    #[test]
    fn deferred_text_object_after_operator_is_noop_not_panic() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut s = VimState {
            mode: VimMode::Normal,
            ..VimState::default()
        };
        // 'd' then 'i' (would be `diw` text-object in full vim) -> deferred -> no-op.
        handle_vim_key(
            &mut s,
            "foo bar",
            1,
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
        );
        let out = handle_vim_key(
            &mut s,
            "foo bar",
            1,
            KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE),
        );
        assert_eq!(out, VimOutcome::Effect(VimEffect::None));
        assert_eq!(s.command, CommandState::Idle);
        assert_eq!(s.register, Register::default()); // nothing deleted/yanked
    }
}

#[cfg(test)]
mod cursor_tests {
    use super::*;

    fn cur(text: &str, off: usize) -> VimCursor<'_> {
        VimCursor { text, offset: off }
    }

    #[test]
    fn left_right_clamp_and_utf8() {
        // "héllo": 'é' is 2 bytes (offsets 1..3).
        assert_eq!(cur("héllo", 0).left().offset, 0); // clamp at 0
        assert_eq!(cur("héllo", 0).right().offset, 1);
        assert_eq!(cur("héllo", 1).right().offset, 3); // skip whole 'é'
        assert_eq!(cur("héllo", 3).left().offset, 1);
        assert_eq!(cur("hi", 2).right().offset, 2); // clamp at end
    }

    #[test]
    fn logical_line_bounds() {
        let t = "abc\ndefg\nhi";
        // cursor in middle of line 2 (offset 6 = 'f')
        assert_eq!(cur(t, 6).start_of_logical_line().offset, 4); // 'd'
        assert_eq!(cur(t, 6).end_of_logical_line().offset, 8); // after 'g' (the \n)
                                                               // line 1 has no leading blanks -> first_non_blank == start
        assert_eq!(cur("  xy", 3).first_non_blank().offset, 2); // 'x'
    }

    #[test]
    fn down_up_preserve_column_clamped() {
        let t = "abcd\nef\nghij";
        // on line0 col3 ('d'), down -> line1 but line1 len 2 -> clamp to end (col2 = after 'f')
        let c = cur(t, 3).down_logical_line();
        assert_eq!(c.offset, 7); // line1 = "ef" at 5..7, end is 7
                                 // from there, down -> line2 col2 = 'i' (offset 8+2=10)
        let c2 = cur(t, 7).down_logical_line();
        assert_eq!(c2.offset, 10);
        // up from line2 col2 -> line1 clamp end = 7
        assert_eq!(cur(t, 10).up_logical_line().offset, 7);
    }

    #[test]
    fn down_up_preserve_char_column_multibyte() {
        // 'é' is 2 bytes. "éé\nabcd":
        //   line0 "éé" = bytes 0..4 (é@0..2, é@2..4), '\n'@4, line1 "abcd" = 5..9.
        // Cursor at char-col 2 of line0 = byte 4 (end of "éé").
        // j must preserve the CHAR column (2), landing on 'c' (byte 7) — NOT the
        // byte column (4) which would land past 'd' at byte 9.
        let t = "éé\nabcd";
        assert_eq!(cur(t, 4).down_logical_line().offset, 7); // 'c'

        // k reverse: from char-col 2 of "abcd" (byte 7 = 'c') back up to "éé".
        // char-col 2 of "éé" is byte 4 (end-of-line position for a 2-char line).
        assert_eq!(cur(t, 7).up_logical_line().offset, 4);
    }

    #[test]
    fn down_clamps_to_shorter_multibyte_dest_line() {
        // "ééé\nx": line0 "ééé" = 0..6, '\n'@6, line1 "x" = 7..8.
        // Cursor at char-col 3 of line0 = byte 6 (end of "ééé").
        // j into "x" (only 1 char) clamps to the dest line end = byte 8.
        let t = "ééé\nx";
        assert_eq!(cur(t, 6).down_logical_line().offset, 8);
    }

    #[test]
    fn down_char_column_mixed_multibyte_then_ascii() {
        // "éabc\nxyzw": é@0..2, a@2, b@3, c@4, '\n'@5, x@6, y@7, z@8, w@9.
        // Cursor on line0 at char-col 2 = 'b' (byte 3).
        // j must land on char-col 2 of "xyzw" = 'z' (byte 8), NOT byte-col 3 = 'w'(9).
        let t = "éabc\nxyzw";
        assert_eq!(cur(t, 3).down_logical_line().offset, 8); // 'z'
    }

    #[test]
    fn first_last_line_and_goto() {
        let t = "one\ntwo\nthree";
        assert_eq!(cur(t, 9).start_of_first_line().offset, 0);
        assert_eq!(cur(t, 0).start_of_last_line().offset, 8); // 'three'
        assert_eq!(cur(t, 0).go_to_line(2).offset, 4); // 'two' (1-indexed)
        assert_eq!(cur(t, 0).go_to_line(99).offset, 8); // clamp to last
    }
}

#[cfg(test)]
mod word_tests {
    use super::*;
    fn cur(text: &str, off: usize) -> VimCursor<'_> {
        VimCursor { text, offset: off }
    }

    #[test]
    fn next_word_skips_to_next_word_start() {
        let t = "foo bar baz";
        assert_eq!(cur(t, 0).next_vim_word().offset, 4); // 'b' of bar
        assert_eq!(cur(t, 4).next_vim_word().offset, 8); // 'b' of baz
        assert_eq!(cur(t, 8).next_vim_word().offset, 11); // end (no next)
    }

    #[test]
    fn next_word_treats_punctuation_as_its_own_word() {
        let t = "foo.bar";
        // from 'f': over the word "foo" then land on '.'
        assert_eq!(cur(t, 0).next_vim_word().offset, 3); // '.'
                                                         // from '.': over the punctuation run then land on "bar"
        assert_eq!(cur(t, 3).next_vim_word().offset, 4); // 'b'
    }

    #[test]
    fn prev_word_goes_to_word_start() {
        let t = "foo bar baz";
        assert_eq!(cur(t, 8).prev_vim_word().offset, 4); // start of 'bar'
        assert_eq!(cur(t, 5).prev_vim_word().offset, 4); // inside 'bar' -> its start
        assert_eq!(cur(t, 2).prev_vim_word().offset, 0); // inside 'foo' -> 0
    }

    #[test]
    fn end_word_lands_on_last_char_of_word() {
        let t = "foo bar";
        assert_eq!(cur(t, 0).end_vim_word().offset, 2); // 'o' (last of foo)
        assert_eq!(cur(t, 2).end_vim_word().offset, 6); // 'r' (last of bar)
    }

    #[test]
    fn word_motions_handle_punctuation_boundaries() {
        let t = "a, b";
        assert_eq!(cur(t, 0).end_vim_word().offset, 1); // ',' is end of next "word"
    }
}

#[cfg(test)]
mod find_tests {
    use super::*;
    fn cur(text: &str, off: usize) -> VimCursor<'_> {
        VimCursor { text, offset: off }
    }

    #[test]
    fn f_lands_on_char() {
        let t = "abcdabcd";
        assert_eq!(cur(t, 0).find_character('c', FindKind::F, 1), Some(2));
        assert_eq!(cur(t, 0).find_character('c', FindKind::F, 2), Some(6)); // 2nd c
        assert_eq!(cur(t, 0).find_character('z', FindKind::F, 1), None);
    }

    #[test]
    fn t_lands_before_char() {
        let t = "abcdabcd";
        assert_eq!(cur(t, 0).find_character('c', FindKind::T, 1), Some(1)); // before first c
    }

    #[test]
    fn big_f_searches_backward() {
        let t = "abcdabcd";
        assert_eq!(cur(t, 7).find_character('a', FindKind::BigF, 1), Some(4));
        assert_eq!(cur(t, 7).find_character('a', FindKind::BigF, 2), Some(0));
    }

    #[test]
    fn big_t_lands_after_char_backward() {
        let t = "abcdabcd";
        // from offset 7 ('d'), backward till 'a' (at 4) -> land just after it = 5
        assert_eq!(cur(t, 7).find_character('a', FindKind::BigT, 1), Some(5));
    }
}

#[cfg(test)]
mod resolve_tests {
    use super::*;
    fn cur(text: &str, off: usize) -> VimCursor<'_> {
        VimCursor { text, offset: off }
    }

    #[test]
    fn single_step_motions() {
        assert_eq!(resolve_motion(Motion::Right, cur("hello", 0), 1).offset, 1);
        assert_eq!(resolve_motion(Motion::Left, cur("hello", 3), 1).offset, 2);
        assert_eq!(
            resolve_motion(Motion::LineEnd, cur("hello", 0), 1).offset,
            5
        );
        assert_eq!(
            resolve_motion(Motion::LineStart, cur("hello", 3), 1).offset,
            0
        );
    }

    #[test]
    fn count_repeats_motion() {
        assert_eq!(resolve_motion(Motion::Right, cur("hello", 0), 3).offset, 3);
        assert_eq!(
            resolve_motion(Motion::NextWord, cur("a b c d", 0), 2).offset,
            4
        ); // 'c'
    }

    #[test]
    fn count_breaks_early_at_bound() {
        // Right 100 times on "hi" stops at end (offset 2), not panic.
        assert_eq!(resolve_motion(Motion::Right, cur("hi", 0), 100).offset, 2);
        // Up on first line is a no-op; count doesn't matter.
        assert_eq!(resolve_motion(Motion::Up, cur("abc", 1), 5).offset, 1);
    }

    #[test]
    fn down_crosses_logical_lines() {
        let t = "abc\ndef\nghi";
        assert_eq!(resolve_motion(Motion::Down, cur(t, 1), 1).offset, 5); // line2 col1 = 'e'
        assert_eq!(resolve_motion(Motion::Down, cur(t, 1), 2).offset, 9); // line3 col1 = 'h'
    }
}

#[cfg(test)]
mod transition_tests {
    use super::*;
    fn cur(text: &str, off: usize) -> VimCursor<'_> {
        VimCursor { text, offset: off }
    }

    #[test]
    fn i_inserts_at_cursor() {
        assert_eq!(
            enter_insert_effect('i', cur("hello", 2)),
            VimEffect::Move(2)
        );
    }

    #[test]
    fn a_inserts_after_cursor() {
        assert_eq!(
            enter_insert_effect('a', cur("hello", 2)),
            VimEffect::Move(3)
        );
        // at end: stays
        assert_eq!(
            enter_insert_effect('a', cur("hello", 5)),
            VimEffect::Move(5)
        );
    }

    #[test]
    fn cap_i_first_non_blank() {
        assert_eq!(enter_insert_effect('I', cur("  hi", 3)), VimEffect::Move(2));
    }

    #[test]
    fn cap_a_end_of_line() {
        assert_eq!(
            enter_insert_effect('A', cur("ab\ncd", 0)),
            VimEffect::Move(2)
        ); // end of line0
    }

    #[test]
    fn o_opens_line_below() {
        // "ab\ncd", cursor on line0 -> newline after line0, cursor at its start (offset 3)
        assert_eq!(
            enter_insert_effect('o', cur("ab\ncd", 1)),
            VimEffect::Edit {
                text: "ab\n\ncd".to_string(),
                cursor: 3
            }
        );
    }

    #[test]
    fn cap_o_opens_line_above() {
        // "ab\ncd", cursor on line1 ('c' @3) -> newline before line1, cursor at its start (offset 3)
        assert_eq!(
            enter_insert_effect('O', cur("ab\ncd", 3)),
            VimEffect::Edit {
                text: "ab\n\ncd".to_string(),
                cursor: 3
            }
        );
    }

    #[test]
    fn esc_clamps_past_end_of_line() {
        // In vim, Normal-mode cursor cannot sit on the trailing position of a
        // non-empty line; clamp left by one char.
        assert_eq!(esc_clamp("hello", 5), 4);
        assert_eq!(esc_clamp("hello", 3), 3); // already valid
        assert_eq!(esc_clamp("", 0), 0); // empty line: stay
        assert_eq!(esc_clamp("ab\ncd", 2), 1); // end of line0 -> clamp to 'b'
    }
}

#[cfg(test)]
mod dispatch_tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }
    fn esc() -> KeyEvent {
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)
    }

    #[test]
    fn esc_from_insert_enters_normal_and_clamps() {
        let mut s = VimState::default(); // Insert
        let out = handle_vim_key(&mut s, "hello", 5, esc());
        assert_eq!(s.mode, VimMode::Normal);
        assert_eq!(out, VimOutcome::Effect(VimEffect::Move(4)));
    }

    #[test]
    fn insert_mode_passes_through_chars() {
        let mut s = VimState::default(); // Insert
        let out = handle_vim_key(&mut s, "hi", 2, key('x'));
        assert_eq!(s.mode, VimMode::Insert);
        assert_eq!(out, VimOutcome::PassThrough); // default editing inserts 'x'
    }

    #[test]
    fn normal_h_l_move() {
        let mut s = VimState {
            mode: VimMode::Normal,
            ..VimState::default()
        };
        assert_eq!(
            handle_vim_key(&mut s, "hello", 2, key('l')),
            VimOutcome::Effect(VimEffect::Move(3))
        );
        assert_eq!(
            handle_vim_key(&mut s, "hello", 2, key('h')),
            VimOutcome::Effect(VimEffect::Move(1))
        );
    }

    #[test]
    fn normal_i_enters_insert() {
        let mut s = VimState {
            mode: VimMode::Normal,
            ..VimState::default()
        };
        let out = handle_vim_key(&mut s, "hello", 2, key('i'));
        assert_eq!(s.mode, VimMode::Insert);
        assert_eq!(out, VimOutcome::Effect(VimEffect::Move(2)));
    }

    #[test]
    fn count_then_motion() {
        let mut s = VimState {
            mode: VimMode::Normal,
            ..VimState::default()
        };
        // "3w" on "a b c d e" -> 4th word
        assert_eq!(
            handle_vim_key(&mut s, "a b c d e", 0, key('3')),
            VimOutcome::Pending
        );
        assert_eq!(s.command, CommandState::Count { digits: "3".into() });
        let out = handle_vim_key(&mut s, "a b c d e", 0, key('w'));
        assert_eq!(out, VimOutcome::Effect(VimEffect::Move(6))); // 'd'
        assert_eq!(s.command, CommandState::Idle); // reset after execute
    }

    #[test]
    fn zero_is_line_start_not_count() {
        let mut s = VimState {
            mode: VimMode::Normal,
            ..VimState::default()
        };
        let out = handle_vim_key(&mut s, "  hello", 4, key('0'));
        assert_eq!(out, VimOutcome::Effect(VimEffect::Move(0)));
    }

    #[test]
    fn caret_first_non_blank() {
        let mut s = VimState {
            mode: VimMode::Normal,
            ..VimState::default()
        };
        let out = handle_vim_key(&mut s, "  hello", 4, key('^'));
        assert_eq!(out, VimOutcome::Effect(VimEffect::Move(2)));
    }

    #[test]
    fn gg_goes_to_first_line() {
        let mut s = VimState {
            mode: VimMode::Normal,
            ..VimState::default()
        };
        assert_eq!(
            handle_vim_key(&mut s, "a\nb\nc", 4, key('g')),
            VimOutcome::Pending
        );
        assert_eq!(s.command, CommandState::G { count: 1 });
        let out = handle_vim_key(&mut s, "a\nb\nc", 4, key('g'));
        assert_eq!(out, VimOutcome::Effect(VimEffect::Move(0)));
    }

    #[test]
    fn count_gg_goes_to_line_n() {
        let mut s = VimState {
            mode: VimMode::Normal,
            ..VimState::default()
        };
        handle_vim_key(&mut s, "a\nb\nc", 0, key('2'));
        handle_vim_key(&mut s, "a\nb\nc", 0, key('g'));
        let out = handle_vim_key(&mut s, "a\nb\nc", 0, key('g'));
        assert_eq!(out, VimOutcome::Effect(VimEffect::Move(2))); // line 2 = 'b'
    }

    #[test]
    fn cap_g_goes_to_last_line() {
        let mut s = VimState {
            mode: VimMode::Normal,
            ..VimState::default()
        };
        let out = handle_vim_key(
            &mut s,
            "a\nb\nc",
            0,
            KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT),
        );
        assert_eq!(out, VimOutcome::Effect(VimEffect::Move(4))); // 'c'
    }

    #[test]
    fn f_char_finds() {
        let mut s = VimState {
            mode: VimMode::Normal,
            ..VimState::default()
        };
        assert_eq!(
            handle_vim_key(&mut s, "abcdc", 0, key('f')),
            VimOutcome::Pending
        );
        assert_eq!(
            handle_vim_key(&mut s, "abcdc", 0, key('c')),
            VimOutcome::Effect(VimEffect::Move(2))
        );
    }

    #[test]
    fn count_f_finds_nth() {
        let mut s = VimState {
            mode: VimMode::Normal,
            ..VimState::default()
        };
        handle_vim_key(&mut s, "abcdc", 0, key('2'));
        handle_vim_key(&mut s, "abcdc", 0, key('f'));
        let out = handle_vim_key(&mut s, "abcdc", 0, key('c'));
        assert_eq!(out, VimOutcome::Effect(VimEffect::Move(4))); // 2nd 'c'
    }

    #[test]
    fn find_not_found_is_noop() {
        let mut s = VimState {
            mode: VimMode::Normal,
            ..VimState::default()
        };
        handle_vim_key(&mut s, "abc", 0, key('f'));
        assert_eq!(
            handle_vim_key(&mut s, "abc", 0, key('z')),
            VimOutcome::Effect(VimEffect::None)
        );
        assert_eq!(s.command, CommandState::Idle);
    }

    #[test]
    fn unknown_normal_key_is_noop() {
        let mut s = VimState {
            mode: VimMode::Normal,
            ..VimState::default()
        };
        assert_eq!(
            handle_vim_key(&mut s, "abc", 0, key('q')),
            VimOutcome::Effect(VimEffect::None)
        );
    }

    // (M7-08 review) Normal mode must NOT swallow CONTROL/ALT key combos: they
    // are app-level bindings (Ctrl-C cancel, Ctrl-Alt-V vim toggle, …) that the
    // dispatcher owns. Returning `PassThrough` lets `handle_live_key` fall
    // through to `map_iocraft_key` + `dispatch`. Plain (NONE/SHIFT) keys still
    // route to vim so motions like `h`/`G`/`$` keep working.

    #[test]
    fn normal_ctrl_combo_passes_through() {
        let mut s = VimState {
            mode: VimMode::Normal,
            ..VimState::default()
        };
        // Ctrl-C in Normal mode must pass through to the cancel binding.
        let out = handle_vim_key(
            &mut s,
            "abc",
            0,
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        );
        assert_eq!(out, VimOutcome::PassThrough);
        assert_eq!(s.mode, VimMode::Normal, "mode must be untouched");
    }

    #[test]
    fn normal_ctrl_alt_v_passes_through() {
        let mut s = VimState {
            mode: VimMode::Normal,
            ..VimState::default()
        };
        // Ctrl-Alt-V (the vim toggle) must pass through, not be swallowed.
        let out = handle_vim_key(
            &mut s,
            "abc",
            0,
            KeyEvent::new(
                KeyCode::Char('v'),
                KeyModifiers::CONTROL | KeyModifiers::ALT,
            ),
        );
        assert_eq!(out, VimOutcome::PassThrough);
    }

    #[test]
    fn normal_alt_combo_passes_through() {
        let mut s = VimState {
            mode: VimMode::Normal,
            ..VimState::default()
        };
        let out = handle_vim_key(
            &mut s,
            "abc",
            0,
            KeyEvent::new(KeyCode::Char('b'), KeyModifiers::ALT),
        );
        assert_eq!(out, VimOutcome::PassThrough);
    }

    #[test]
    fn normal_plain_and_shift_keys_still_route_to_vim() {
        let mut s = VimState {
            mode: VimMode::Normal,
            ..VimState::default()
        };
        // Plain 'l' is a vim motion, NOT a pass-through.
        assert_eq!(
            handle_vim_key(&mut s, "hello", 0, key('l')),
            VimOutcome::Effect(VimEffect::Move(1))
        );
        // SHIFT 'G' (last line) is a vim motion, NOT a pass-through.
        assert_eq!(
            handle_vim_key(
                &mut s,
                "a\nb",
                0,
                KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT)
            ),
            VimOutcome::Effect(VimEffect::Move(2))
        );
    }

    #[test]
    fn ctrl_combo_passes_through_even_in_pending_count() {
        // A pending count must not trap a ctrl combo either — Ctrl-C should
        // still reach the cancel binding mid-count.
        let mut s = VimState {
            mode: VimMode::Normal,
            ..VimState::default()
        };
        assert_eq!(
            handle_vim_key(&mut s, "abc", 0, key('2')),
            VimOutcome::Pending
        );
        let out = handle_vim_key(
            &mut s,
            "abc",
            0,
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        );
        assert_eq!(out, VimOutcome::PassThrough);
    }
}

#[cfg(test)]
mod m7_09_types_tests {
    use super::*;

    #[test]
    fn default_register_is_empty_charwise() {
        let s = VimState::default();
        assert_eq!(s.register, Register::default());
        assert!(s.register.text.is_empty());
        assert!(!s.register.linewise);
        assert!(s.visual.is_none());
    }

    #[test]
    fn operator_command_states_constructible() {
        let a = CommandState::Operator {
            op: Operator::Delete,
            count: 1,
        };
        let b = CommandState::OperatorCount {
            op: Operator::Change,
            count: 1,
            digits: "3".into(),
        };
        let c = CommandState::OperatorFind {
            op: Operator::Yank,
            count: 2,
            kind: FindKind::F,
        };
        let d = CommandState::OperatorG {
            op: Operator::Delete,
            count: 1,
        };
        assert_ne!(a, b);
        assert_ne!(c, d);
    }

    #[test]
    fn visual_state_records_anchor_and_kind() {
        let v = VisualState {
            anchor: 3,
            linewise: true,
        };
        assert_eq!(v.anchor, 3);
        assert!(v.linewise);
    }

    #[test]
    fn last_char_len_handles_utf8_and_empty() {
        assert_eq!(last_char_len("hi"), 1);
        assert_eq!(last_char_len("hé"), 2); // 'é' is 2 bytes
        assert_eq!(last_char_len(""), 1); // empty -> 1
    }
}

#[cfg(test)]
mod motion_class_tests {
    use super::*;

    #[test]
    fn inclusive_motions_are_e_and_dollar() {
        assert!(is_inclusive_motion('e'));
        assert!(is_inclusive_motion('$'));
        assert!(!is_inclusive_motion('w'));
        assert!(!is_inclusive_motion('0'));
        assert!(!is_inclusive_motion('h'));
    }

    #[test]
    fn linewise_motions_are_jk_and_g() {
        assert!(is_linewise_motion("j"));
        assert!(is_linewise_motion("k"));
        assert!(is_linewise_motion("G"));
        assert!(is_linewise_motion("gg"));
        assert!(!is_linewise_motion("w"));
        assert!(!is_linewise_motion("$"));
    }

    #[test]
    fn operator_motion_map_covers_required_keys() {
        assert_eq!(motion_for_operator_key('w'), Some(Motion::NextWord));
        assert_eq!(motion_for_operator_key('b'), Some(Motion::PrevWord));
        assert_eq!(motion_for_operator_key('e'), Some(Motion::EndWord));
        assert_eq!(motion_for_operator_key('$'), Some(Motion::LineEnd));
        assert_eq!(motion_for_operator_key('0'), Some(Motion::LineStart));
        assert_eq!(motion_for_operator_key('^'), Some(Motion::FirstNonBlank));
        assert_eq!(motion_for_operator_key('h'), Some(Motion::Left));
        assert_eq!(motion_for_operator_key('l'), Some(Motion::Right));
        assert_eq!(motion_for_operator_key('j'), Some(Motion::Down));
        assert_eq!(motion_for_operator_key('k'), Some(Motion::Up));
        assert_eq!(motion_for_operator_key('z'), None);
    }
}

#[cfg(test)]
mod apply_op_tests {
    use super::*;
    fn cur(text: &str, off: usize) -> VimCursor<'_> {
        VimCursor { text, offset: off }
    }

    #[test]
    fn range_exclusive_for_w() {
        // dw on "foo bar": w moves 0->4, range [0,4) exclusive (not inclusive).
        let r = operator_range(cur("foo bar", 0), 4, 'w', Operator::Delete, 1);
        assert_eq!((r.from, r.to, r.linewise), (0, 4, false));
    }

    #[test]
    fn range_inclusive_for_e_and_dollar() {
        // de on "foo bar": e moves 0->2 ('o'), inclusive -> to = 3.
        let r = operator_range(cur("foo bar", 0), 2, 'e', Operator::Delete, 1);
        assert_eq!((r.from, r.to, r.linewise), (0, 3, false));
        // d$ on "foo": $ moves 0->3 (== len), inclusive but already at end -> to stays 3.
        let r2 = operator_range(cur("foo", 0), 3, '$', Operator::Delete, 1);
        assert_eq!((r2.from, r2.to, r2.linewise), (0, 3, false));
    }

    #[test]
    fn range_cw_changes_to_end_of_word_like_ce() {
        // cw on "foo bar" from 0: special-cased to end-of-word -> through 'o' (to=3),
        // NOT to start of next word (4). This is the claude-code cw->ce rule.
        let r = operator_range(cur("foo bar", 0), 4, 'w', Operator::Change, 1);
        assert_eq!((r.from, r.to, r.linewise), (0, 3, false));
    }

    #[test]
    fn range_linewise_for_j() {
        // dj on "a\nb\nc" from offset 0: j is linewise, deletes lines 0..1 incl
        // trailing newline of line1 -> [0, 4) ("a\nb\n").
        let r = operator_range(cur("a\nb\nc", 0), 2, 'j', Operator::Delete, 1);
        assert!(r.linewise);
        assert_eq!((r.from, r.to), (0, 4));
    }

    #[test]
    fn apply_delete_sets_register_and_edits() {
        let (effect, reg, enter_insert) = apply_operator(Operator::Delete, "foo bar", 0, 4, false);
        assert_eq!(
            reg,
            Register {
                text: "foo ".into(),
                linewise: false
            }
        );
        assert_eq!(
            effect,
            VimEffect::Edit {
                text: "bar".into(),
                cursor: 0
            }
        );
        assert!(!enter_insert);
    }

    #[test]
    fn apply_yank_keeps_text_moves_cursor_to_from() {
        let (effect, reg, enter_insert) = apply_operator(Operator::Yank, "foo bar", 4, 7, false);
        assert_eq!(
            reg,
            Register {
                text: "bar".into(),
                linewise: false
            }
        );
        assert_eq!(effect, VimEffect::Move(4)); // buffer unchanged, cursor to range start
        assert!(!enter_insert);
    }

    #[test]
    fn apply_change_edits_and_requests_insert() {
        let (effect, reg, enter_insert) = apply_operator(Operator::Change, "foo bar", 0, 3, false);
        assert_eq!(
            reg,
            Register {
                text: "foo".into(),
                linewise: false
            }
        );
        assert_eq!(
            effect,
            VimEffect::Edit {
                text: " bar".into(),
                cursor: 0
            }
        );
        assert!(enter_insert);
    }

    #[test]
    fn apply_linewise_delete_tags_register_linewise() {
        // delete "a\n" (line 0) from "a\nb": register linewise, text "a\nb" -> "b".
        let (effect, reg, _) = apply_operator(Operator::Delete, "a\nb", 0, 2, true);
        assert!(reg.linewise);
        assert_eq!(reg.text, "a\n");
        assert_eq!(
            effect,
            VimEffect::Edit {
                text: "b".into(),
                cursor: 0
            }
        );
    }
}

#[cfg(test)]
mod line_op_tests {
    use super::*;
    fn cur(text: &str, off: usize) -> VimCursor<'_> {
        VimCursor { text, offset: off }
    }

    #[test]
    fn dd_deletes_current_line_register_linewise() {
        // dd on line 1 ('b') of "a\nb\nc": delete "b\n", register linewise.
        let (effect, reg, enter_insert) = line_op(Operator::Delete, cur("a\nb\nc", 2), 1);
        assert!(reg.linewise);
        assert_eq!(reg.text, "b\n");
        assert_eq!(
            effect,
            VimEffect::Edit {
                text: "a\nc".into(),
                cursor: 2
            }
        );
        assert!(!enter_insert);
    }

    #[test]
    fn dd_last_line_consumes_preceding_newline() {
        // dd on last line ('c') of "a\nb\nc": delete to EOF; preceding '\n' consumed
        // so no orphan trailing newline. Result "a\nb".
        let (effect, _reg, _) = line_op(Operator::Delete, cur("a\nb\nc", 4), 1);
        match effect {
            VimEffect::Edit { text, .. } => assert_eq!(text, "a\nb"),
            other => panic!("expected Edit, got {other:?}"),
        }
    }

    #[test]
    fn count_dd_deletes_n_lines() {
        // 2dd on "a\nb\nc\nd" from line0: delete "a\nb\n" -> "c\nd".
        let (effect, reg, _) = line_op(Operator::Delete, cur("a\nb\nc\nd", 0), 2);
        assert_eq!(reg.text, "a\nb\n");
        match effect {
            VimEffect::Edit { text, cursor } => {
                assert_eq!(text, "c\nd");
                assert_eq!(cursor, 0);
            }
            other => panic!("expected Edit, got {other:?}"),
        }
    }

    #[test]
    fn yy_yanks_keeps_buffer_cursor_to_line_start() {
        // yy on line1 of "a\nb\nc": register "b\n" linewise, buffer unchanged, cursor->line start (2).
        let (effect, reg, enter_insert) = line_op(Operator::Yank, cur("a\nb\nc", 3), 1);
        assert_eq!(
            reg,
            Register {
                text: "b\n".into(),
                linewise: true
            }
        );
        assert_eq!(effect, VimEffect::Move(2));
        assert!(!enter_insert);
    }

    #[test]
    fn cc_clears_line_enters_insert_at_line_start() {
        // cc on line1 of "ab\ncd\nef": clear "cd" -> "ab\n\nef", enter insert at line start (3).
        let (effect, _reg, enter_insert) = line_op(Operator::Change, cur("ab\ncd\nef", 4), 1);
        assert_eq!(
            effect,
            VimEffect::Edit {
                text: "ab\n\nef".into(),
                cursor: 3
            }
        );
        assert!(enter_insert);
    }

    #[test]
    fn cc_single_line_buffer_clears_to_empty() {
        let (effect, _reg, enter_insert) = line_op(Operator::Change, cur("hello", 2), 1);
        assert_eq!(
            effect,
            VimEffect::Edit {
                text: String::new(),
                cursor: 0
            }
        );
        assert!(enter_insert);
    }
}

#[cfg(test)]
mod x_tests {
    use super::*;
    fn cur(text: &str, off: usize) -> VimCursor<'_> {
        VimCursor { text, offset: off }
    }

    #[test]
    fn x_deletes_char_under_cursor() {
        // x on "hello" at 0: delete 'h' -> "ello", register "h", cursor 0.
        let (effect, reg) = delete_char_x(cur("hello", 0), 1);
        assert_eq!(
            reg,
            Register {
                text: "h".into(),
                linewise: false
            }
        );
        assert_eq!(
            effect,
            VimEffect::Edit {
                text: "ello".into(),
                cursor: 0
            }
        );
    }

    #[test]
    fn count_x_deletes_n_chars() {
        // 3x on "hello" at 0: delete "hel" -> "lo", cursor 0.
        let (effect, reg) = delete_char_x(cur("hello", 0), 3);
        assert_eq!(reg.text, "hel");
        assert_eq!(
            effect,
            VimEffect::Edit {
                text: "lo".into(),
                cursor: 0
            }
        );
    }

    #[test]
    fn x_clamps_cursor_to_last_char() {
        // x on last char of "ab" at 1: delete 'b' -> "a"; cursor clamps to 0.
        let (effect, _reg) = delete_char_x(cur("ab", 1), 1);
        assert_eq!(
            effect,
            VimEffect::Edit {
                text: "a".into(),
                cursor: 0
            }
        );
    }

    #[test]
    fn x_at_eof_is_noop() {
        let (effect, reg) = delete_char_x(cur("ab", 2), 1);
        assert_eq!(effect, VimEffect::None);
        assert_eq!(reg, Register::default()); // unchanged
    }

    #[test]
    fn count_x_overshoot_clamps_at_eof() {
        // 9x on "ab" at 0: delete both -> "", cursor 0.
        let (effect, reg) = delete_char_x(cur("ab", 0), 9);
        assert_eq!(reg.text, "ab");
        assert_eq!(
            effect,
            VimEffect::Edit {
                text: String::new(),
                cursor: 0
            }
        );
    }
}

#[cfg(test)]
mod paste_tests {
    use super::*;
    fn cur(text: &str, off: usize) -> VimCursor<'_> {
        VimCursor { text, offset: off }
    }
    fn reg_char(s: &str) -> Register {
        Register {
            text: s.into(),
            linewise: false,
        }
    }
    fn reg_line(s: &str) -> Register {
        Register {
            text: s.into(),
            linewise: true,
        }
    }

    #[test]
    fn charwise_p_inserts_after_cursor() {
        // p with register "X" on "ab" at 0: insert after 'a' -> "aXb", cursor on 'X' (1).
        let effect = paste(true, 1, &reg_char("X"), cur("ab", 0));
        assert_eq!(
            effect,
            VimEffect::Edit {
                text: "aXb".into(),
                cursor: 1
            }
        );
    }

    #[test]
    fn charwise_cap_p_inserts_before_cursor() {
        // P with register "X" on "ab" at 1: insert at cursor -> "aXb", cursor on 'X' (1).
        let effect = paste(false, 1, &reg_char("X"), cur("ab", 1));
        assert_eq!(
            effect,
            VimEffect::Edit {
                text: "aXb".into(),
                cursor: 1
            }
        );
    }

    #[test]
    fn charwise_p_repeats_count_times() {
        // 3p with register "X" on "ab" at 0: "aXXXb", cursor on last 'X' (3).
        let effect = paste(true, 3, &reg_char("X"), cur("ab", 0));
        assert_eq!(
            effect,
            VimEffect::Edit {
                text: "aXXXb".into(),
                cursor: 3
            }
        );
    }

    #[test]
    fn charwise_p_at_eof_inserts_at_cursor() {
        // p with register "X" on "ab" at 2 (EOF): insert at cursor -> "abX", cursor on 'X' (2).
        let effect = paste(true, 1, &reg_char("X"), cur("ab", 2));
        assert_eq!(
            effect,
            VimEffect::Edit {
                text: "abX".into(),
                cursor: 2
            }
        );
    }

    #[test]
    fn linewise_p_opens_line_below() {
        // p with linewise register "x\n" on "a\nb" at 0 (line0): new line below -> "a\nx\nb",
        // cursor at start of pasted line (2).
        let effect = paste(true, 1, &reg_line("x\n"), cur("a\nb", 0));
        assert_eq!(
            effect,
            VimEffect::Edit {
                text: "a\nx\nb".into(),
                cursor: 2
            }
        );
    }

    #[test]
    fn linewise_cap_p_opens_line_above() {
        // P with linewise register "x\n" on "a\nb" at 2 (line1): new line above -> "a\nx\nb",
        // cursor at start of pasted line (2).
        let effect = paste(false, 1, &reg_line("x\n"), cur("a\nb", 2));
        assert_eq!(
            effect,
            VimEffect::Edit {
                text: "a\nx\nb".into(),
                cursor: 2
            }
        );
    }

    #[test]
    fn empty_register_is_noop() {
        let effect = paste(true, 1, &Register::default(), cur("ab", 0));
        assert_eq!(effect, VimEffect::None);
    }
}

#[cfg(test)]
mod op_dispatch_tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    fn key(c: char) -> KeyEvent {
        let m = if c.is_uppercase() {
            KeyModifiers::SHIFT
        } else {
            KeyModifiers::NONE
        };
        KeyEvent::new(KeyCode::Char(c), m)
    }
    fn esc() -> KeyEvent {
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)
    }
    fn normal() -> VimState {
        VimState {
            mode: VimMode::Normal,
            ..VimState::default()
        }
    }

    #[test]
    fn d_enters_operator_pending() {
        let mut s = normal();
        assert_eq!(
            handle_vim_key(&mut s, "foo bar", 0, key('d')),
            VimOutcome::Pending
        );
        assert_eq!(
            s.command,
            CommandState::Operator {
                op: Operator::Delete,
                count: 1
            }
        );
    }

    #[test]
    fn dw_deletes_word() {
        let mut s = normal();
        handle_vim_key(&mut s, "foo bar", 0, key('d'));
        let out = handle_vim_key(&mut s, "foo bar", 0, key('w'));
        assert_eq!(
            out,
            VimOutcome::Effect(VimEffect::Edit {
                text: "bar".into(),
                cursor: 0
            })
        );
        assert_eq!(s.command, CommandState::Idle);
        assert_eq!(s.register.text, "foo ");
    }

    #[test]
    fn de_deletes_to_end_of_word_inclusive() {
        let mut s = normal();
        handle_vim_key(&mut s, "foo bar", 0, key('d'));
        let out = handle_vim_key(&mut s, "foo bar", 0, key('e'));
        assert_eq!(
            out,
            VimOutcome::Effect(VimEffect::Edit {
                text: " bar".into(),
                cursor: 0
            })
        );
    }

    #[test]
    fn d_dollar_deletes_to_eol() {
        let mut s = normal();
        handle_vim_key(&mut s, "foo bar", 4, key('d'));
        let out = handle_vim_key(&mut s, "foo bar", 4, key('$'));
        assert_eq!(
            out,
            VimOutcome::Effect(VimEffect::Edit {
                text: "foo ".into(),
                cursor: 3
            })
        );
    }

    #[test]
    fn dd_deletes_line() {
        let mut s = normal();
        handle_vim_key(&mut s, "a\nb\nc", 2, key('d'));
        let out = handle_vim_key(&mut s, "a\nb\nc", 2, key('d'));
        assert_eq!(
            out,
            VimOutcome::Effect(VimEffect::Edit {
                text: "a\nc".into(),
                cursor: 2
            })
        );
        assert!(s.register.linewise);
    }

    #[test]
    fn cc_clears_line_and_enters_insert() {
        let mut s = normal();
        handle_vim_key(&mut s, "ab\ncd", 0, key('c'));
        let out = handle_vim_key(&mut s, "ab\ncd", 0, key('c'));
        assert_eq!(
            out,
            VimOutcome::Effect(VimEffect::Edit {
                text: "\ncd".into(),
                cursor: 0
            })
        );
        assert_eq!(s.mode, VimMode::Insert);
    }

    #[test]
    fn cw_changes_to_end_of_word_and_enters_insert() {
        let mut s = normal();
        handle_vim_key(&mut s, "foo bar", 0, key('c'));
        let out = handle_vim_key(&mut s, "foo bar", 0, key('w'));
        assert_eq!(
            out,
            VimOutcome::Effect(VimEffect::Edit {
                text: " bar".into(),
                cursor: 0
            })
        );
        assert_eq!(s.mode, VimMode::Insert);
    }

    #[test]
    fn yy_yanks_line_keeps_buffer() {
        let mut s = normal();
        handle_vim_key(&mut s, "a\nb\nc", 2, key('y'));
        let out = handle_vim_key(&mut s, "a\nb\nc", 2, key('y'));
        assert_eq!(out, VimOutcome::Effect(VimEffect::Move(2)));
        assert_eq!(
            s.register,
            Register {
                text: "b\n".into(),
                linewise: true
            }
        );
    }

    #[test]
    fn count_dw_multiplies() {
        // 3dw on "a b c d e" from 0: delete 3 words -> "d e".
        let mut s = normal();
        handle_vim_key(&mut s, "a b c d e", 0, key('3'));
        handle_vim_key(&mut s, "a b c d e", 0, key('d'));
        let out = handle_vim_key(&mut s, "a b c d e", 0, key('w'));
        assert_eq!(
            out,
            VimOutcome::Effect(VimEffect::Edit {
                text: "d e".into(),
                cursor: 0
            })
        );
    }

    #[test]
    fn d_count_w_inner_count_multiplies() {
        // d3w on "a b c d e" from 0: same as 3dw -> "d e".
        let mut s = normal();
        handle_vim_key(&mut s, "a b c d e", 0, key('d'));
        handle_vim_key(&mut s, "a b c d e", 0, key('3'));
        let out = handle_vim_key(&mut s, "a b c d e", 0, key('w'));
        assert_eq!(
            out,
            VimOutcome::Effect(VimEffect::Edit {
                text: "d e".into(),
                cursor: 0
            })
        );
    }

    #[test]
    fn count_yy_yanks_n_lines() {
        // 2yy on "a\nb\nc" from 0: register "a\nb\n".
        let mut s = normal();
        handle_vim_key(&mut s, "a\nb\nc", 0, key('2'));
        handle_vim_key(&mut s, "a\nb\nc", 0, key('y'));
        let out = handle_vim_key(&mut s, "a\nb\nc", 0, key('y'));
        assert_eq!(out, VimOutcome::Effect(VimEffect::Move(0)));
        assert_eq!(s.register.text, "a\nb\n");
    }

    #[test]
    fn df_char_deletes_through_find() {
        // df_c on "abcde" from 0: find 'c' at 2, inclusive -> delete "abc" -> "de".
        let mut s = normal();
        handle_vim_key(&mut s, "abcde", 0, key('d'));
        handle_vim_key(&mut s, "abcde", 0, key('f'));
        assert_eq!(
            s.command,
            CommandState::OperatorFind {
                op: Operator::Delete,
                count: 1,
                kind: FindKind::F
            }
        );
        let out = handle_vim_key(&mut s, "abcde", 0, key('c'));
        assert_eq!(
            out,
            VimOutcome::Effect(VimEffect::Edit {
                text: "de".into(),
                cursor: 0
            })
        );
    }

    #[test]
    fn d_cap_g_deletes_to_last_line() {
        // dG on "a\nb\nc" from 0: linewise delete all -> "".
        let mut s = normal();
        handle_vim_key(&mut s, "a\nb\nc", 0, key('d'));
        let out = handle_vim_key(&mut s, "a\nb\nc", 0, key('G'));
        match out {
            VimOutcome::Effect(VimEffect::Edit { text, .. }) => assert_eq!(text, ""),
            other => panic!("expected Edit, got {other:?}"),
        }
    }

    #[test]
    fn dgg_deletes_to_first_line() {
        // dgg on "a\nb\nc" from offset 4 (line2 'c'): linewise delete lines 0..2 -> "".
        let mut s = normal();
        handle_vim_key(&mut s, "a\nb\nc", 4, key('d'));
        assert_eq!(
            handle_vim_key(&mut s, "a\nb\nc", 4, key('g')),
            VimOutcome::Pending
        );
        assert_eq!(
            s.command,
            CommandState::OperatorG {
                op: Operator::Delete,
                count: 1
            }
        );
        let out = handle_vim_key(&mut s, "a\nb\nc", 4, key('g'));
        match out {
            VimOutcome::Effect(VimEffect::Edit { text, .. }) => assert_eq!(text, ""),
            other => panic!("expected Edit, got {other:?}"),
        }
    }

    #[test]
    fn x_deletes_char() {
        let mut s = normal();
        let out = handle_vim_key(&mut s, "hello", 0, key('x'));
        assert_eq!(
            out,
            VimOutcome::Effect(VimEffect::Edit {
                text: "ello".into(),
                cursor: 0
            })
        );
        assert_eq!(s.register.text, "h");
    }

    #[test]
    fn p_pastes_after() {
        let mut s = normal();
        s.register = Register {
            text: "X".into(),
            linewise: false,
        };
        let out = handle_vim_key(&mut s, "ab", 0, key('p'));
        assert_eq!(
            out,
            VimOutcome::Effect(VimEffect::Edit {
                text: "aXb".into(),
                cursor: 1
            })
        );
    }

    #[test]
    fn esc_cancels_operator_pending() {
        let mut s = normal();
        handle_vim_key(&mut s, "foo", 0, key('d'));
        let out = handle_vim_key(&mut s, "foo", 0, esc());
        assert_eq!(out, VimOutcome::Effect(VimEffect::None));
        assert_eq!(s.command, CommandState::Idle);
    }

    #[test]
    fn operator_motion_noop_when_motion_does_not_move() {
        // dh at offset 0: h is a no-op -> operator no-op, register untouched.
        let mut s = normal();
        handle_vim_key(&mut s, "abc", 0, key('d'));
        let out = handle_vim_key(&mut s, "abc", 0, key('h'));
        assert_eq!(out, VimOutcome::Effect(VimEffect::None));
        assert_eq!(s.register, Register::default());
        assert_eq!(s.command, CommandState::Idle);
    }
}

#[cfg(test)]
mod visual_tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    fn key(c: char) -> KeyEvent {
        let m = if c.is_uppercase() {
            KeyModifiers::SHIFT
        } else {
            KeyModifiers::NONE
        };
        KeyEvent::new(KeyCode::Char(c), m)
    }
    fn esc() -> KeyEvent {
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)
    }
    fn normal() -> VimState {
        VimState {
            mode: VimMode::Normal,
            ..VimState::default()
        }
    }

    #[test]
    fn v_enters_visual_sets_anchor() {
        let mut s = normal();
        let out = handle_vim_key(&mut s, "hello", 2, key('v'));
        assert_eq!(s.mode, VimMode::Visual);
        assert_eq!(
            s.visual,
            Some(VisualState {
                anchor: 2,
                linewise: false
            })
        );
        assert_eq!(out, VimOutcome::Effect(VimEffect::None));
    }

    #[test]
    fn cap_v_enters_visual_linewise() {
        let mut s = normal();
        handle_vim_key(&mut s, "a\nb", 0, key('V'));
        assert_eq!(s.mode, VimMode::Visual);
        assert_eq!(
            s.visual,
            Some(VisualState {
                anchor: 0,
                linewise: true
            })
        );
    }

    #[test]
    fn visual_motion_moves_selection_end() {
        // v then l l on "hello" from 0: cursor moves to 2 (anchor stays 0).
        let mut s = normal();
        handle_vim_key(&mut s, "hello", 0, key('v'));
        let out = handle_vim_key(&mut s, "hello", 0, key('l'));
        assert_eq!(out, VimOutcome::Effect(VimEffect::Move(1)));
        let out2 = handle_vim_key(&mut s, "hello", 1, key('l'));
        assert_eq!(out2, VimOutcome::Effect(VimEffect::Move(2)));
        assert_eq!(
            s.visual,
            Some(VisualState {
                anchor: 0,
                linewise: false
            })
        );
    }

    #[test]
    fn visual_d_deletes_inclusive_selection() {
        // v (anchor 0) then d on "hello" with cursor at 2: charwise inclusive ->
        // delete [0,3) "hel" -> "lo", back to Normal.
        let mut s = normal();
        handle_vim_key(&mut s, "hello", 0, key('v')); // anchor 0
        let out = handle_vim_key(&mut s, "hello", 2, key('d'));
        assert_eq!(
            out,
            VimOutcome::Effect(VimEffect::Edit {
                text: "lo".into(),
                cursor: 0
            })
        );
        assert_eq!(s.mode, VimMode::Normal);
        assert!(s.visual.is_none());
        assert_eq!(s.register.text, "hel");
    }

    #[test]
    fn visual_y_yanks_selection_keeps_buffer() {
        let mut s = normal();
        handle_vim_key(&mut s, "hello", 0, key('v'));
        let out = handle_vim_key(&mut s, "hello", 2, key('y'));
        // yank moves cursor to range start, buffer unchanged.
        assert_eq!(out, VimOutcome::Effect(VimEffect::Move(0)));
        assert_eq!(
            s.register,
            Register {
                text: "hel".into(),
                linewise: false
            }
        );
        assert_eq!(s.mode, VimMode::Normal);
    }

    #[test]
    fn visual_c_deletes_and_enters_insert() {
        let mut s = normal();
        handle_vim_key(&mut s, "hello", 0, key('v'));
        let out = handle_vim_key(&mut s, "hello", 2, key('c'));
        assert_eq!(
            out,
            VimOutcome::Effect(VimEffect::Edit {
                text: "lo".into(),
                cursor: 0
            })
        );
        assert_eq!(s.mode, VimMode::Insert);
        assert!(s.visual.is_none());
    }

    #[test]
    fn visual_line_d_deletes_whole_lines() {
        // V then cursor on line1 ('b' @2) then d on "a\nb\nc":
        // linewise delete lines 0..1 -> "c".
        let mut s = normal();
        handle_vim_key(&mut s, "a\nb\nc", 0, key('V')); // anchor 0, linewise
        let out = handle_vim_key(&mut s, "a\nb\nc", 2, key('d')); // cursor on line1
        assert_eq!(
            out,
            VimOutcome::Effect(VimEffect::Edit {
                text: "c".into(),
                cursor: 0
            })
        );
        assert!(s.register.linewise);
        assert_eq!(s.register.text, "a\nb\n");
        assert_eq!(s.mode, VimMode::Normal);
    }

    #[test]
    fn visual_esc_returns_to_normal() {
        let mut s = normal();
        handle_vim_key(&mut s, "hello", 2, key('v'));
        let out = handle_vim_key(&mut s, "hello", 4, esc());
        assert_eq!(out, VimOutcome::Effect(VimEffect::Move(4))); // 4 < len 5 -> no clamp
        assert_eq!(s.mode, VimMode::Normal);
        assert!(s.visual.is_none());
    }

    #[test]
    fn visual_count_motion_extends() {
        // v then 2l on "hello" from 0: cursor -> 2.
        let mut s = normal();
        handle_vim_key(&mut s, "hello", 0, key('v'));
        assert_eq!(
            handle_vim_key(&mut s, "hello", 0, key('2')),
            VimOutcome::Pending
        );
        let out = handle_vim_key(&mut s, "hello", 0, key('l'));
        assert_eq!(out, VimOutcome::Effect(VimEffect::Move(2)));
    }
}
