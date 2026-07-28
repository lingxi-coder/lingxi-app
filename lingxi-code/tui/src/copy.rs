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

/// Clipboard route that completed the copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardTransport {
    /// A platform clipboard program (`pbcopy`, `clip`, Wayland, or X11).
    Native,
    /// A tmux server buffer.
    Tmux,
    /// OSC 52 written to the controlling terminal.
    Osc52,
}

/// Copy failure after every supported transport was attempted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardCopyError {
    message: String,
}

impl std::fmt::Display for ClipboardCopyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ClipboardCopyError {}

const MAX_OSC52_INPUT_BYTES: usize = 100_000;
const SCREEN_PASSTHROUGH_CHUNK_BYTES: usize = 240;

/// Copy text using the parity transport order: native clipboard, tmux buffer,
/// then OSC 52.  X11 writes also mirror the value into PRIMARY selection.
///
/// OSC 52 is emitted only as a terminal control sequence.  Under tmux it uses
/// DCS passthrough; under GNU screen the inner sequence is split into bounded
/// passthrough chunks so screen's control-string limit cannot truncate it.
pub fn copy_to_clipboard(text: &str) -> Result<ClipboardTransport, ClipboardCopyError> {
    // A remote native copy writes the remote host's clipboard, not the
    // terminal user's clipboard.  Claude therefore skips it over SSH and
    // relies on mux/OSC 52 forwarding.
    if !is_ssh() && try_native_clipboard(text) {
        return Ok(ClipboardTransport::Native);
    }
    if std::env::var_os("TMUX").is_some()
        && (write_command("tmux", &["load-buffer", "-w", "-"], text.as_bytes())
            || write_command("tmux", &["load-buffer", "-"], text.as_bytes()))
    {
        return Ok(ClipboardTransport::Tmux);
    }
    if text.len() > MAX_OSC52_INPUT_BYTES {
        return Err(ClipboardCopyError {
            message: format!(
                "clipboard content is too large for terminal fallback ({} bytes)",
                text.len()
            ),
        });
    }

    let envelope = if std::env::var_os("TMUX").is_some() {
        Osc52Envelope::Tmux
    } else if is_gnu_screen() {
        Osc52Envelope::Screen
    } else {
        Osc52Envelope::Plain
    };
    let sequence = osc52_sequence(text, envelope);
    use std::io::Write as _;
    let mut stdout = std::io::stdout().lock();
    let osc_result = stdout
        .write_all(sequence.as_bytes())
        .and_then(|()| stdout.flush());
    if let Err(error) = osc_result {
        return Err(ClipboardCopyError {
            message: format!("clipboard unavailable: {error}"),
        });
    }
    Ok(ClipboardTransport::Osc52)
}

/// Compatibility wrapper for existing fire-and-forget call sites.
///
/// The TUI confirmation is emitted before the off-thread copy.  Keep the
/// existing no-panic behavior while the richer [`copy_to_clipboard`] API lets
/// full-screen copy-on-select surface a single failure toast.
pub fn copy_to_clipboard_native(text: &str) {
    let _ = copy_to_clipboard(text);
}

fn try_native_clipboard(text: &str) -> bool {
    let bytes = text.as_bytes();
    if cfg!(target_os = "macos") {
        return write_command("pbcopy", &[], bytes);
    }
    if cfg!(target_os = "windows") {
        return write_command(
            "powershell",
            &["-NoProfile", "-Command", "$input | Set-Clipboard"],
            bytes,
        ) || write_command("clip", &[], bytes);
    }

    if write_command("wl-copy", &[], bytes) {
        let _ = write_command("wl-copy", &["--primary"], bytes);
        return true;
    }
    if write_command("xclip", &["-selection", "clipboard"], bytes) {
        // X11 has two user-visible clipboards.  PRIMARY mirrors native
        // copy-on-select without making its failure invalidate CLIPBOARD.
        let _ = write_command("xclip", &["-selection", "primary"], bytes);
        return true;
    }
    if write_command("xsel", &["--clipboard", "--input"], bytes) {
        let _ = write_command("xsel", &["--primary", "--input"], bytes);
        return true;
    }
    false
}

fn write_command(command: &str, args: &[&str], bytes: &[u8]) -> bool {
    use std::io::Write as _;
    use std::process::{Command, Stdio};

    let Ok(mut child) = Command::new(command)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    let wrote = child
        .stdin
        .take()
        .is_some_and(|mut stdin| stdin.write_all(bytes).is_ok());
    wrote && child.wait().is_ok_and(|status| status.success())
}

fn is_gnu_screen() -> bool {
    std::env::var("TERM")
        .ok()
        .is_some_and(|term| term.starts_with("screen"))
        || std::env::var_os("STY").is_some()
}

fn is_ssh() -> bool {
    ["SSH_CONNECTION", "SSH_CLIENT", "SSH_TTY"]
        .iter()
        .any(|name| std::env::var_os(name).is_some())
}

/// OSC 52 passthrough envelope selected from the host environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Osc52Envelope {
    /// Direct terminal.
    Plain,
    /// tmux DCS passthrough.
    Tmux,
    /// GNU screen chunked DCS passthrough.
    Screen,
}

/// Build an OSC 52 clipboard sequence.  Public for golden/platform tests.
#[must_use]
pub fn osc52_sequence(text: &str, envelope: Osc52Envelope) -> String {
    let payload = base64_encode(text.as_bytes());
    let inner = format!("\x1b]52;c;{payload}\x1b\\");
    match envelope {
        Osc52Envelope::Plain => inner,
        Osc52Envelope::Tmux => {
            // tmux passthrough requires every inner ESC to be doubled.
            format!("\x1bPtmux;{}\x1b\\", inner.replace('\x1b', "\x1b\x1b"))
        }
        Osc52Envelope::Screen => inner
            .as_bytes()
            .chunks(SCREEN_PASSTHROUGH_CHUNK_BYTES)
            .map(|chunk| {
                // `inner` is ASCII (control bytes + base64), so every chunk is
                // valid UTF-8 and can be passed through independently.
                format!("\x1bP{}\x1b\\", String::from_utf8_lossy(chunk))
            })
            .collect(),
    }
}

fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let a = chunk[0];
        let b = chunk.get(1).copied().unwrap_or(0);
        let c = chunk.get(2).copied().unwrap_or(0);
        output.push(char::from(ALPHABET[usize::from(a >> 2)]));
        output.push(char::from(
            ALPHABET[usize::from(((a & 0x03) << 4) | (b >> 4))],
        ));
        if chunk.len() > 1 {
            output.push(char::from(
                ALPHABET[usize::from(((b & 0x0f) << 2) | (c >> 6))],
            ));
        } else {
            output.push('=');
        }
        if chunk.len() > 2 {
            output.push(char::from(ALPHABET[usize::from(c & 0x3f)]));
        } else {
            output.push('=');
        }
    }
    output
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

    #[test]
    fn osc52_plain_encodes_unicode_without_visible_payload_text() {
        assert_eq!(
            osc52_sequence("武", Osc52Envelope::Plain),
            "\x1b]52;c;5q2m\x1b\\"
        );
    }

    #[test]
    fn osc52_tmux_uses_dcs_and_doubles_inner_escape() {
        assert_eq!(
            osc52_sequence("abc", Osc52Envelope::Tmux),
            "\x1bPtmux;\x1b\x1b]52;c;YWJj\x1b\x1b\\\x1b\\"
        );
    }

    #[test]
    fn osc52_screen_chunks_large_passthrough_payloads() {
        let text = "x".repeat(600);
        let encoded = osc52_sequence(&text, Osc52Envelope::Screen);
        assert!(encoded.starts_with("\x1bP\x1b]52;c;"));
        assert!(encoded.matches("\x1bP").count() > 1);
        // Every chunk contributes one outer DCS terminator, while the last
        // chunk also carries the inner OSC terminator.
        assert_eq!(
            encoded.matches("\x1bP").count() + 1,
            encoded.matches("\x1b\\").count()
        );
        // Removing one outer suffix from every DCS chunk reconstructs one
        // valid inner OSC (including its own ST suffix).
        let reconstructed: String = encoded
            .split("\x1bP")
            .skip(1)
            .map(|chunk| chunk.strip_suffix("\x1b\\").unwrap())
            .collect();
        assert_eq!(reconstructed, osc52_sequence(&text, Osc52Envelope::Plain));
    }

    #[test]
    fn base64_padding_is_rfc_4648() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
    }
}
