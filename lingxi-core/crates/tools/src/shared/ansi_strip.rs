//! ANSI escape-sequence stripper.
//!
//! Removes the `\x1b\[[0-9;]*[a-zA-Z]` pattern from process output before it
//! is returned to the model. The regex literal is locked in spec §7 (M4-02
//! shell tools).

/// Locked regex literal — the canonical cross-platform parity contract.
/// The implementation below is a hand-rolled state machine that recognizes
/// the same language as this regex; the constant exists so test fixtures and
/// downstream consumers can assert the contract by string.
pub const ANSI_ESCAPE_REGEX_LITERAL: &str = r"\x1b\[[0-9;]*[a-zA-Z]";

/// Strip ANSI CSI escape sequences from `input`. Returns the cleaned string.
#[must_use]
pub fn strip_ansi(input: &str) -> String {
    strip_ansi_count(input).0
}

/// Strip ANSI CSI escape sequences and report how many characters were dropped.
///
/// State machine:
/// - `Normal`: pass char through; on `\x1b`, transition to `Esc`.
/// - `Esc`: expect `[`; on `[`, transition to `Csi`; on anything else, emit the
///   buffered `\x1b` and the current char and go back to `Normal`.
/// - `Csi`: consume digits and `;` greedily; on an ASCII alpha (`[a-zA-Z]`),
///   terminate the sequence (drop everything from `\x1b` through this char
///   inclusive) and go back to `Normal`; on EOF, emit the buffered bytes verbatim.
#[must_use]
pub fn strip_ansi_count(input: &str) -> (String, usize) {
    enum State {
        Normal,
        Esc,
        Csi,
    }
    let mut out = String::with_capacity(input.len());
    let mut dropped = 0usize;
    let mut state = State::Normal;
    let mut pending = String::new();
    for ch in input.chars() {
        match state {
            State::Normal => {
                if ch == '\x1b' {
                    state = State::Esc;
                    pending.clear();
                    pending.push(ch);
                } else {
                    out.push(ch);
                }
            }
            State::Esc => {
                if ch == '[' {
                    state = State::Csi;
                    pending.push(ch);
                } else {
                    out.push_str(&pending);
                    out.push(ch);
                    pending.clear();
                    state = State::Normal;
                }
            }
            State::Csi => {
                pending.push(ch);
                if ch.is_ascii_alphabetic() {
                    dropped += pending.chars().count();
                    pending.clear();
                    state = State::Normal;
                } else if !(ch.is_ascii_digit() || ch == ';') {
                    out.push_str(&pending);
                    pending.clear();
                    state = State::Normal;
                }
            }
        }
    }
    if !pending.is_empty() {
        out.push_str(&pending);
    }
    (out, dropped)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locked_regex_literal_unchanged() {
        assert_eq!(ANSI_ESCAPE_REGEX_LITERAL, r"\x1b\[[0-9;]*[a-zA-Z]");
    }

    #[test]
    fn strips_simple_color_sequence() {
        assert_eq!(strip_ansi("\x1b[31mred\x1b[0m"), "red");
    }

    #[test]
    fn strips_compound_attribute_sequence() {
        assert_eq!(strip_ansi("\x1b[1;31;42mbold-red-bg\x1b[0m"), "bold-red-bg");
    }

    #[test]
    fn passes_through_text_with_no_escapes() {
        assert_eq!(strip_ansi("hello world\n"), "hello world\n");
    }

    #[test]
    fn dropped_count_matches_actual_dropped_chars() {
        let (out, dropped) = strip_ansi_count("\x1b[31mred\x1b[0m");
        assert_eq!(out, "red");
        // "\x1b[31m" = 5 chars, "\x1b[0m" = 4 chars → 9 dropped total.
        assert_eq!(dropped, 9);
    }

    #[test]
    fn flushes_buffered_bytes_on_unexpected_char_inside_csi() {
        assert_eq!(strip_ansi("\x1b[3$keep"), "\x1b[3$keep");
    }

    #[test]
    fn flushes_buffered_bytes_on_eof_partial_csi() {
        assert_eq!(strip_ansi("\x1b[3"), "\x1b[3");
    }

    #[test]
    fn esc_without_bracket_passes_through() {
        assert_eq!(strip_ansi("\x1bZkeep"), "\x1bZkeep");
    }
}
