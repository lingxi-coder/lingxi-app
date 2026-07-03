//! `/copy [N]` — copy a recent assistant response to the system clipboard
//! (plan Phase 8).
//!
//! The pure arg-parser is a 1:1 port of the iocraft backend's
//! `tui/src/commands/copy.rs` (itself a behavioral port of claude-code
//! `commands/copy/copy.tsx`): every display string below is byte-locked to
//! that surface. `/copy` copies the latest assistant text, `/copy N` reaches
//! back to the Nth-latest (1 = latest), capped at [`MAX_LOOKBACK`].
//!
//! The parser here works over the ALREADY-COLLECTED newest-first text list —
//! [`crate::chat_widget::ChatWidget::cmd_copy`] collects it from the
//! transcript's committed `AssistantTextCell`s (non-empty bodies only, which
//! excludes tool-use-only turns and API errors by construction, exactly like
//! claude-code's `collectRecentAssistantTexts` filter). The clipboard write
//! itself is the app layer's job ([`copy_to_clipboard_native`], executed on
//! `ChatOutcome::CopyToClipboard`), keeping the widget free of subprocess
//! side effects.

/// Newest-first lookback cap — byte-locked to claude-code `MAX_LOOKBACK`.
pub const MAX_LOOKBACK: usize = 20;

/// Outcome of a `/copy [N]` command — the display message plus (on success)
/// the text to write to the clipboard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CopyCommand {
    /// Copy succeeds: `text` goes to the clipboard, `display` confirms.
    Copy {
        /// The assistant text to write to the system clipboard.
        text: String,
        /// The `Copied to clipboard (C characters, L lines)` confirmation.
        display: String,
    },
    /// Nothing to copy / bad arg / out-of-range — `display` is the error body.
    Error {
        /// The error message body (rendered as an error `SystemText`).
        display: String,
    },
}

/// Build the `Copied to clipboard (C characters, L lines)` confirmation
/// (claude-code `copyOrWriteToFile`: `lineCount = '\n' count + 1`,
/// `charCount = text.length` — chars here are Unicode scalars).
fn copied_confirmation(text: &str) -> String {
    let char_count = text.chars().count();
    let line_count = text.matches('\n').count() + 1;
    format!("Copied to clipboard ({char_count} characters, {line_count} lines)")
}

/// Parse a `/copy [N]` command against the newest-first assistant `texts`
/// (index 0 = latest). `args` is the text after the command word.
#[must_use]
pub fn parse_copy_command(texts: &[String], args: &str) -> CopyCommand {
    if texts.is_empty() {
        return CopyCommand::Error {
            display: "No assistant message to copy".to_string(),
        };
    }

    // `/copy N` reaches back N-1 messages (1 = latest, 2 = second-to-latest).
    let mut age: usize = 0;
    let arg = args.trim();
    if !arg.is_empty() {
        // claude-code uses JS `Number(arg)` then `Number.isInteger(n) && n >= 1`;
        // `parse::<usize>()` rejects floats/exponents/negatives identically.
        match arg.parse::<usize>() {
            Ok(n) if n >= 1 => {
                if n > texts.len() {
                    let noun = if texts.len() == 1 {
                        "message"
                    } else {
                        "messages"
                    };
                    return CopyCommand::Error {
                        display: format!("Only {} assistant {noun} available to copy", texts.len()),
                    };
                }
                age = n - 1;
            }
            _ => {
                return CopyCommand::Error {
                    display: format!(
                        "Usage: /copy [N] where N is 1 (latest), 2, 3, \u{2026} Got: {arg}"
                    ),
                };
            }
        }
    }

    let text = texts[age].clone();
    let display = copied_confirmation(&text);
    CopyCommand::Copy { text, display }
}

/// Shell out to a native clipboard utility, writing `text` to its stdin.
/// Best-effort: a missing binary or non-zero exit is ignored (claude-code
/// `copyNative` / `execFileNoThrow`). Probes the same per-platform utilities
/// claude-code uses; on Linux it tries the Wayland tool first, then X11.
/// Ported from the iocraft backend's `root::copy_to_clipboard_native`.
pub fn copy_to_clipboard_native(text: &str) {
    use std::io::Write as _;
    use std::process::{Command, Stdio};

    // (cmd, args) candidates in probe order for the current platform.
    let candidates: &[(&str, &[&str])] = if cfg!(target_os = "macos") {
        &[("pbcopy", &[])]
    } else if cfg!(target_os = "windows") {
        &[("clip", &[])]
    } else {
        // Linux/other: Wayland (wl-copy) → X11 (xclip → xsel).
        &[
            ("wl-copy", &[]),
            ("xclip", &["-selection", "clipboard"]),
            ("xsel", &["--clipboard", "--input"]),
        ]
    };

    for (cmd, args) in candidates {
        let spawned = Command::new(cmd)
            .args(*args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        let Ok(mut child) = spawned else {
            continue; // binary not found — try the next candidate
        };
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
            // Drop stdin to signal EOF before waiting.
        }
        // Wait so the pipe is fully consumed; ignore the exit status. Stop
        // after the first utility that successfully spawned.
        let _ = child.wait();
        return;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(bodies: &[&str]) -> Vec<String> {
        bodies.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn no_arg_copies_the_latest() {
        let out = parse_copy_command(&texts(&["latest\nline two"]), "");
        assert_eq!(
            out,
            CopyCommand::Copy {
                text: "latest\nline two".to_string(),
                display: "Copied to clipboard (15 characters, 2 lines)".to_string(),
            }
        );
    }

    #[test]
    fn n_reaches_back_and_out_of_range_reports_count() {
        let list = texts(&["newest", "older"]);
        assert!(matches!(
            parse_copy_command(&list, "2"),
            CopyCommand::Copy { ref text, .. } if text == "older"
        ));
        assert_eq!(
            parse_copy_command(&list, "3"),
            CopyCommand::Error {
                display: "Only 2 assistant messages available to copy".to_string(),
            }
        );
        // Singular noun for a single message.
        assert_eq!(
            parse_copy_command(&texts(&["only"]), "5"),
            CopyCommand::Error {
                display: "Only 1 assistant message available to copy".to_string(),
            }
        );
    }

    #[test]
    fn bad_args_and_empty_transcript_error() {
        assert_eq!(
            parse_copy_command(&[], ""),
            CopyCommand::Error {
                display: "No assistant message to copy".to_string(),
            }
        );
        for bad in ["0", "-1", "1.5", "abc"] {
            assert_eq!(
                parse_copy_command(&texts(&["x"]), bad),
                CopyCommand::Error {
                    display: format!(
                        "Usage: /copy [N] where N is 1 (latest), 2, 3, \u{2026} Got: {bad}"
                    ),
                },
                "arg {bad}"
            );
        }
    }
}
