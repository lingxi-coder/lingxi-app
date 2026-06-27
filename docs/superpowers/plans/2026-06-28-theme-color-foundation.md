# Theme & Color Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Auto-detect the terminal background (OSC-11) and color depth at startup so `Auto` theme resolution picks the correct existing theme on every terminal, fixing light-terminal white-on-white and low-color quantization app-wide.

**Architecture:** A new `tui/src/theme_detect.rs` runs a one-shot, pre-flight OSC-11 background query before iocraft mounts and caches the result (Light/Dark) in a process-global. `ThemeSetting::Auto.resolve()` combines that cached background (falling back to `$COLORFGBG` then Dark) with a `$COLORTERM`-derived color depth to select among the already-existing `{Dark, Light, DarkAnsi, LightAnsi}` themes. The six themes and the `ansi:` color mapper are untouched — we only *select* among them.

**Tech Stack:** Rust (edition from workspace), `crossterm` (raw mode), `libc` (unix `poll` for a timed stdin read), `std::io::IsTerminal`. PTY integration test in Python via `pexpect` + `pyte` (the repo's established TUI-testing tools).

**Spec:** `docs/superpowers/specs/2026-06-28-theme-color-foundation-design.md`

## Global Constraints

- Do NOT modify the six `ThemeName` palettes, the `ansi:<name>` → `Color` mapper, or `ThemeName::to_wire`/`from_wire` (byte-locked vs claude-code). Only *select* among existing themes.
- `Auto.resolve()` must remain a pure, non-blocking function (no terminal I/O); all I/O happens in the startup `detect_terminal_theme()` pre-flight.
- Detection is best-effort and must NEVER hang or panic: hard ~100 ms deadline; any failure (not a TTY, no reply, malformed, raw-mode error) falls back to `$COLORFGBG` → `Dark`.
- OSC-11 detection is unix-only (`#[cfg(unix)]`); on non-unix it is a no-op and Auto falls back.
- Detect once at startup (no live re-detection).
- Run all cargo commands without piping into `tail`/`grep` that would mask the exit code; use `... ; echo EXIT=$?` or read the file.
- In PTY tests, never `pkill -f lingxi-cli` (it can kill the user's running instance); kill the specific child pid only.
- Work in the `tui` crate. Run `cargo test -p tui` for Rust tests.

---

### Task 1: OSC-11 reply parsing + luminance (pure helpers)

**Files:**
- Create: `tui/src/theme_detect.rs`
- Modify: `tui/src/lib.rs` (add `mod theme_detect;`)
- Test: inline `#[cfg(test)]` module in `tui/src/theme_detect.rs`

**Interfaces:**
- Produces:
  - `pub(crate) fn parse_osc11_rgb(reply: &str) -> Option<(f64, f64, f64)>` — normalized 0..1 channels.
  - `pub(crate) fn luminance_is_light(r: f64, g: f64, b: f64) -> bool` — BT.709, `> 0.5`.

- [ ] **Step 1: Add the module declaration**

In `tui/src/lib.rs`, add alongside the other `mod` lines (e.g. next to `pub(crate) mod terminal;`):

```rust
pub(crate) mod theme_detect;
```

- [ ] **Step 2: Write the failing tests**

Create `tui/src/theme_detect.rs` with ONLY this test module for now:

```rust
//! Terminal background + color-depth detection feeding `ThemeSetting::Auto`.
//! One-shot OSC-11 pre-flight at startup; pure helpers below are I/O-free.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_16bit_white_and_black() {
        let (r, g, b) = parse_osc11_rgb("\x1b]11;rgb:ffff/ffff/ffff\x07").unwrap();
        assert!((r - 1.0).abs() < 1e-9 && (g - 1.0).abs() < 1e-9 && (b - 1.0).abs() < 1e-9);
        let (r, g, b) = parse_osc11_rgb("\x1b]11;rgb:0000/0000/0000\x1b\\").unwrap();
        assert_eq!((r, g, b), (0.0, 0.0, 0.0));
    }

    #[test]
    fn parses_8bit_channels() {
        let (r, g, b) = parse_osc11_rgb("rgb:ff/80/00").unwrap();
        assert!((r - 1.0).abs() < 1e-9);
        assert!((g - 128.0 / 255.0).abs() < 1e-9);
        assert_eq!(b, 0.0);
    }

    #[test]
    fn rejects_malformed() {
        assert!(parse_osc11_rgb("garbage").is_none());
        assert!(parse_osc11_rgb("rgb:zz/00/00").is_none());
        assert!(parse_osc11_rgb("rgb:ffff/ffff").is_none());
    }

    #[test]
    fn luminance_threshold() {
        assert!(luminance_is_light(1.0, 1.0, 1.0)); // white
        assert!(!luminance_is_light(0.0, 0.0, 0.0)); // black
        assert!(luminance_is_light(0.8, 0.8, 0.8)); // light grey
        assert!(!luminance_is_light(0.2, 0.2, 0.2)); // dark grey
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p tui theme_detect ; echo EXIT=$?`
Expected: FAIL — `cannot find function parse_osc11_rgb` / `luminance_is_light`.

- [ ] **Step 4: Write the minimal implementation**

At the TOP of `tui/src/theme_detect.rs` (above the test module), add:

```rust
/// Parse an OSC-11 background reply body (`...rgb:RRRR/GGGG/BBBB...`) into
/// 0..1-normalized channels. Tolerates 8- or 16-bit-per-channel hex and a
/// trailing terminator (BEL `\x07` or ST `\x1b\\`). `None` if not parseable.
pub(crate) fn parse_osc11_rgb(reply: &str) -> Option<(f64, f64, f64)> {
    let body = &reply[reply.find("rgb:")? + 4..];
    let mut parts = body.split('/');
    let r = parse_channel(parts.next()?)?;
    let g = parse_channel(parts.next()?)?;
    let b = parse_channel(parts.next()?)?;
    Some((r, g, b))
}

/// Parse one hex channel (1..=4 hex digits), trimming any trailing
/// non-hex bytes (the OSC terminator). `None` if there is no leading hex.
fn parse_channel(s: &str) -> Option<f64> {
    let hex: String = s.trim().chars().take_while(|c| c.is_ascii_hexdigit()).collect();
    if hex.is_empty() {
        return None;
    }
    let max = ((1u64 << (4 * hex.len() as u64)) - 1) as f64;
    let v = u64::from_str_radix(&hex, 16).ok()? as f64;
    Some(v / max)
}

/// ITU-R BT.709 relative luminance; `> 0.5` ⇒ a light background.
/// (Matches claude-code's `themeFromOscColor`.)
pub(crate) fn luminance_is_light(r: f64, g: f64, b: f64) -> bool {
    0.2126 * r + 0.7152 * g + 0.0722 * b > 0.5
}
```

Note: `rgb:zz/00/00` → `parse_channel("zz")` yields empty leading hex → `None`. `rgb:ffff/ffff` → third `parts.next()` is `None` → `None`. Both covered.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p tui theme_detect ; echo EXIT=$?`
Expected: PASS (4 tests).

- [ ] **Step 6: Commit**

```bash
git add tui/src/theme_detect.rs tui/src/lib.rs
git commit -m "feat(tui): OSC-11 rgb parse + BT.709 luminance helpers"
```

---

### Task 2: Color-depth detection

**Files:**
- Modify: `tui/src/theme.rs` (add `ColorDepth` + `color_depth()`)
- Test: inline `#[cfg(test)]` in `tui/src/theme.rs`

**Interfaces:**
- Produces:
  - `pub(crate) enum ColorDepth { Truecolor, Low }`
  - `pub(crate) fn color_depth_from(colorterm: Option<&str>, term: Option<&str>) -> ColorDepth` — pure, testable.
  - `pub(crate) fn color_depth() -> ColorDepth` — reads `$COLORTERM`/`$TERM`.

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` in `tui/src/theme.rs` (create the module if needed, `use super::*;`):

```rust
#[test]
fn color_depth_detection() {
    assert_eq!(color_depth_from(Some("truecolor"), None), ColorDepth::Truecolor);
    assert_eq!(color_depth_from(Some("24bit"), None), ColorDepth::Truecolor);
    assert_eq!(color_depth_from(None, Some("xterm-direct")), ColorDepth::Truecolor);
    assert_eq!(color_depth_from(None, Some("xterm-256color")), ColorDepth::Low);
    assert_eq!(color_depth_from(None, Some("screen")), ColorDepth::Low);
    assert_eq!(color_depth_from(None, None), ColorDepth::Low);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p tui color_depth_detection ; echo EXIT=$?`
Expected: FAIL — `cannot find ColorDepth` / `color_depth_from`.

- [ ] **Step 3: Write the implementation**

Add near the top of `tui/src/theme.rs` (after imports):

```rust
/// Terminal color capability used to choose between the truecolor themes
/// (`Dark`/`Light`) and the 16-color `-ansi` themes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ColorDepth {
    /// 24-bit color available — use the rgb themes.
    Truecolor,
    /// 256/16-color (or unknown) — use the `-ansi` themes to avoid the terminal
    /// quantizing truecolor SGR to the wrong nearest ANSI slot.
    Low,
}

/// Pure color-depth classification from `$COLORTERM` + `$TERM`.
pub(crate) fn color_depth_from(colorterm: Option<&str>, term: Option<&str>) -> ColorDepth {
    if matches!(colorterm, Some("truecolor") | Some("24bit")) {
        return ColorDepth::Truecolor;
    }
    if let Some(t) = term {
        if t.contains("direct") || t.contains("truecolor") {
            return ColorDepth::Truecolor;
        }
    }
    ColorDepth::Low
}

/// Color depth from the live environment.
pub(crate) fn color_depth() -> ColorDepth {
    color_depth_from(
        std::env::var("COLORTERM").ok().as_deref(),
        std::env::var("TERM").ok().as_deref(),
    )
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p tui color_depth_detection ; echo EXIT=$?`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add tui/src/theme.rs
git commit -m "feat(tui): COLORTERM/TERM color-depth detection"
```

---

### Task 3: Detected-background cache + `Auto.resolve()` integration

**Files:**
- Modify: `tui/src/theme_detect.rs` (cache)
- Modify: `tui/src/theme.rs` (`resolve_theme`, `resolve_auto`, `Auto.resolve()`)
- Test: inline `#[cfg(test)]` in `tui/src/theme.rs`

**Interfaces:**
- Consumes: `ColorDepth`, `color_depth()` (Task 2); `colorfgbg_theme` (existing, in `theme.rs`).
- Produces:
  - `pub(crate) fn set_detected_background(bg: Option<ThemeName>)` and `pub(crate) fn detected_background() -> Option<ThemeName>` (in `theme_detect.rs`).
  - `pub(crate) fn resolve_theme(background: ThemeName, depth: ColorDepth) -> ThemeName` (in `theme.rs`).
  - `pub(crate) fn resolve_auto(detected: Option<ThemeName>, colorfgbg: Option<&str>, depth: ColorDepth) -> ThemeName` (in `theme.rs`).
  - `ThemeSetting::Auto.resolve()` now uses the cache + depth.

- [ ] **Step 1: Add the cache to `theme_detect.rs`**

Add to `tui/src/theme_detect.rs` (top level, above the helpers):

```rust
use crate::theme::ThemeName;
use std::sync::OnceLock;

/// Process-global detected background (set once at startup). `Some(Light|Dark)`
/// when the OSC-11 query succeeded; `None`/unset otherwise.
static DETECTED_BACKGROUND: OnceLock<Option<ThemeName>> = OnceLock::new();

/// Record the startup OSC-11 detection result (idempotent; first write wins).
pub(crate) fn set_detected_background(bg: Option<ThemeName>) {
    let _ = DETECTED_BACKGROUND.set(bg);
}

/// The detected background (`Light`/`Dark`), or `None` if detection didn't run
/// or didn't resolve.
pub(crate) fn detected_background() -> Option<ThemeName> {
    DETECTED_BACKGROUND.get().copied().flatten()
}
```

- [ ] **Step 2: Write the failing tests (in `theme.rs`)**

Add to the `#[cfg(test)] mod tests` in `tui/src/theme.rs`:

```rust
#[test]
fn resolve_theme_four_cells() {
    use ColorDepth::*;
    assert_eq!(resolve_theme(ThemeName::Light, Truecolor), ThemeName::Light);
    assert_eq!(resolve_theme(ThemeName::Light, Low), ThemeName::LightAnsi);
    assert_eq!(resolve_theme(ThemeName::Dark, Truecolor), ThemeName::Dark);
    assert_eq!(resolve_theme(ThemeName::Dark, Low), ThemeName::DarkAnsi);
}

#[test]
fn resolve_auto_precedence() {
    use ColorDepth::Truecolor;
    // Detected background wins over COLORFGBG.
    assert_eq!(
        resolve_auto(Some(ThemeName::Light), Some("0;15"), Truecolor),
        ThemeName::Light
    );
    // No detection → COLORFGBG light (bg index 15) wins over the Dark default.
    assert_eq!(resolve_auto(None, Some("0;15"), Truecolor), ThemeName::Light);
    // No detection, no COLORFGBG → Dark.
    assert_eq!(resolve_auto(None, None, Truecolor), ThemeName::Dark);
    // Low color depth degrades to the -ansi theme.
    assert_eq!(resolve_auto(None, None, ColorDepth::Low), ThemeName::DarkAnsi);
}
```

(`0;15` is a `$COLORFGBG` value whose last component `15` is a light ANSI bg index — see the existing `colorfgbg_theme`.)

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p tui resolve_ ; echo EXIT=$?`
Expected: FAIL — `cannot find resolve_theme` / `resolve_auto`.

- [ ] **Step 4: Add `resolve_theme` + `resolve_auto`, and update `Auto.resolve()`**

Add to `tui/src/theme.rs` (near `colorfgbg_theme`):

```rust
/// Select the concrete theme from a detected background and color depth.
/// `background` is only ever `Light` or `Dark`; the `_` arm covers `Dark`.
pub(crate) fn resolve_theme(background: ThemeName, depth: ColorDepth) -> ThemeName {
    match (background, depth) {
        (ThemeName::Light, ColorDepth::Truecolor) => ThemeName::Light,
        (ThemeName::Light, ColorDepth::Low) => ThemeName::LightAnsi,
        (_, ColorDepth::Truecolor) => ThemeName::Dark,
        (_, ColorDepth::Low) => ThemeName::DarkAnsi,
    }
}

/// Pure `Auto` resolution: detected background → `$COLORFGBG` → Dark, combined
/// with color depth. Extracted from `resolve()` so the precedence is testable
/// without touching process-global state or the environment.
pub(crate) fn resolve_auto(
    detected: Option<ThemeName>,
    colorfgbg: Option<&str>,
    depth: ColorDepth,
) -> ThemeName {
    let background = detected
        .or_else(|| colorfgbg_theme(colorfgbg))
        .unwrap_or(ThemeName::Dark);
    resolve_theme(background, depth)
}
```

Then REPLACE the body of `ThemeSetting::resolve()` (the `ThemeSetting::Auto` arm) so it reads:

```rust
    #[must_use]
    pub fn resolve(self) -> ThemeName {
        match self {
            ThemeSetting::Auto => resolve_auto(
                crate::theme_detect::detected_background(),
                std::env::var("COLORFGBG").ok().as_deref(),
                color_depth(),
            ),
            ThemeSetting::Named(n) => n,
        }
    }
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p tui resolve_ ; echo EXIT=$?`
Expected: PASS.

- [ ] **Step 6: Run the full theme + state suites (no regressions)**

Run: `cargo test -p tui theme ; echo EXIT=$?` then `cargo test -p tui ; echo EXIT=$?`
Expected: PASS, 0 failed. (The byte-locked `to_wire` tests and `AppState` theme tests must still pass — palettes/wire strings are untouched.)

- [ ] **Step 7: Commit**

```bash
git add tui/src/theme.rs tui/src/theme_detect.rs
git commit -m "feat(tui): Auto theme resolution from detected bg + color depth"
```

---

### Task 4: Testable OSC-11 query core (`detect_with_io`)

**Files:**
- Modify: `tui/src/theme_detect.rs`
- Test: inline `#[cfg(test)]` in `tui/src/theme_detect.rs`

**Interfaces:**
- Consumes: `parse_osc11_rgb`, `luminance_is_light` (Task 1), `ThemeName`.
- Produces: `pub(crate) fn detect_with_io<R: std::io::Read, W: std::io::Write>(reader: R, writer: W) -> Option<ThemeName>`.

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` in `tui/src/theme_detect.rs`:

```rust
#[test]
fn detect_with_io_light_reply_and_sends_query() {
    let reply = b"\x1b]11;rgb:ffff/ffff/ffff\x07";
    let mut sent = Vec::new();
    let got = detect_with_io(std::io::Cursor::new(&reply[..]), &mut sent);
    assert_eq!(got, Some(ThemeName::Light));
    assert_eq!(sent, b"\x1b]11;?\x07"); // it sent the OSC-11 query
}

#[test]
fn detect_with_io_dark_reply() {
    let reply = b"\x1b]11;rgb:0000/0000/0000\x07";
    let got = detect_with_io(std::io::Cursor::new(&reply[..]), std::io::sink());
    assert_eq!(got, Some(ThemeName::Dark));
}

#[test]
fn detect_with_io_no_reply_is_none() {
    let got = detect_with_io(std::io::Cursor::new(&b""[..]), std::io::sink());
    assert_eq!(got, None);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p tui detect_with_io ; echo EXIT=$?`
Expected: FAIL — `cannot find function detect_with_io`.

- [ ] **Step 3: Write the implementation**

Add to `tui/src/theme_detect.rs`:

```rust
use std::io::{Read, Write};

/// The OSC-11 background query.
const OSC11_QUERY: &[u8] = b"\x1b]11;?\x07";

/// I/O-injectable detection core: write the OSC-11 query to `writer`, then read
/// the reply from `reader` until a terminator (BEL `\x07` or ST `\x1b\\`), EOF,
/// or a 1 KiB cap, and classify it. The `reader` owns timing — a real terminal
/// reader returns `Ok(0)` on timeout (see `detect_terminal_theme`), so this
/// loop ends without a reply and returns `None`.
pub(crate) fn detect_with_io<R: Read, W: Write>(mut reader: R, mut writer: W) -> Option<ThemeName> {
    writer.write_all(OSC11_QUERY).ok()?;
    writer.flush().ok()?;
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 64];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                let terminated = buf.contains(&0x07)
                    || buf.windows(2).any(|w| w == [0x1b, 0x5c]);
                if terminated || buf.len() > 1024 {
                    break;
                }
            }
        }
    }
    let (r, g, b) = parse_osc11_rgb(&String::from_utf8_lossy(&buf))?;
    Some(if luminance_is_light(r, g, b) {
        ThemeName::Light
    } else {
        ThemeName::Dark
    })
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p tui detect_with_io ; echo EXIT=$?`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add tui/src/theme_detect.rs
git commit -m "feat(tui): I/O-injectable OSC-11 detection core"
```

---

### Task 5: Live pre-flight `detect_terminal_theme()` + startup wiring

**Files:**
- Modify: `tui/Cargo.toml` (add `libc`)
- Modify: `tui/src/theme_detect.rs` (`TimedStdin`, `detect_terminal_theme`)
- Modify: `tui/src/session.rs` (call it in `run_tui_session` + `run_resume_picker`)

**Interfaces:**
- Consumes: `detect_with_io`, `set_detected_background` (Tasks 3–4).
- Produces: `pub fn detect_terminal_theme()` — runs once at startup, populates the cache (best-effort, unix-only).

- [ ] **Step 1: Add the `libc` dependency**

In `tui/Cargo.toml`, under `[dependencies]`, add (use the workspace version if `[workspace.dependencies]` defines `libc`, else pin):

```toml
libc = { workspace = true }
```

If the workspace does not define `libc`, instead add `libc = "0.2"`. Verify it resolves:

Run: `cargo build -p tui ; echo EXIT=$?`
Expected: PASS (builds with the new dep).

- [ ] **Step 2: Implement `TimedStdin` + `detect_terminal_theme`**

Add to `tui/src/theme_detect.rs`:

```rust
/// A `Read` over stdin that returns `Ok(0)` once a deadline passes, so a silent
/// terminal can't make detection hang. Uses `poll(2)` so there is NO background
/// reader thread that could steal the user's first keystroke.
#[cfg(unix)]
struct TimedStdin {
    deadline: std::time::Instant,
}

#[cfg(unix)]
impl Read for TimedStdin {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let now = std::time::Instant::now();
        if now >= self.deadline {
            return Ok(0);
        }
        let ms = (self.deadline - now).as_millis().min(i32::MAX as u128) as i32;
        let mut pfd = libc::pollfd {
            fd: libc::STDIN_FILENO,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: single valid pollfd; standard poll() usage.
        let ready = unsafe { libc::poll(&mut pfd, 1, ms) };
        if ready <= 0 {
            return Ok(0); // timeout or poll error → behave like EOF
        }
        // SAFETY: reading into the caller's buffer up to its length.
        let n = unsafe { libc::read(libc::STDIN_FILENO, buf.as_mut_ptr().cast(), buf.len()) };
        if n < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(n as usize)
        }
    }
}

/// One-shot startup pre-flight: query the terminal background via OSC-11 and
/// cache the result for `ThemeSetting::Auto`. Best-effort and non-blocking
/// (≤ ~100 ms). No-op unless both stdin and stdout are TTYs. Unix-only; on other
/// platforms Auto falls back to `$COLORFGBG`/Dark.
pub fn detect_terminal_theme() {
    #[cfg(unix)]
    {
        use std::io::IsTerminal;
        if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
            return;
        }
        if crossterm::terminal::enable_raw_mode().is_err() {
            return;
        }
        let reader = TimedStdin {
            deadline: std::time::Instant::now() + std::time::Duration::from_millis(100),
        };
        let bg = detect_with_io(reader, std::io::stdout());
        let _ = crossterm::terminal::disable_raw_mode();
        set_detected_background(bg);
    }
}
```

- [ ] **Step 3: Wire it into `run_tui_session`**

In `tui/src/session.rs`, find the existing safety-hook call in `run_tui_session`:

```rust
    crate::terminal::install_terminal_safety_hooks();
```

Immediately AFTER it, add:

```rust
    // Detect the terminal background (OSC-11) + color depth once, before the
    // first `Auto.resolve()` in `AppState::new`, so every screen renders with
    // the correct theme on light/dark and low-color terminals.
    crate::theme_detect::detect_terminal_theme();
```

- [ ] **Step 4: Wire it into `run_resume_picker`**

In `tui/src/session.rs`, find in `run_resume_picker`:

```rust
    crate::terminal::install_terminal_safety_hooks();
```

Immediately AFTER it, add the same line:

```rust
    crate::theme_detect::detect_terminal_theme();
```

- [ ] **Step 5: Build + run the full tui suite**

Run: `cargo build -p tui ; echo EXIT=$?` then `cargo test -p tui ; echo EXIT=$?`
Expected: PASS, 0 failed. (Detection is a no-op under `cargo test` — not a TTY — so existing tests are unaffected.)

- [ ] **Step 6: Commit**

```bash
git add tui/Cargo.toml tui/src/theme_detect.rs tui/src/session.rs
git commit -m "feat(tui): OSC-11 background pre-flight wired at TUI startup"
```

---

### Task 6: PTY integration test — harness plays the terminal

**Files:**
- Create: `tui/tests/osc11_theme_pty.py`

**Interfaces:**
- Consumes: the built `lingxi-cli` binary; `pexpect` + `pyte` (the repo's TUI-test tools).
- Produces: an end-to-end check that a light OSC-11 reply ⇒ Light theme, a dark reply ⇒ Dark theme, and no reply ⇒ no hang + Dark.

- [ ] **Step 1: Build the binary the test drives**

Run: `cargo build -p cli --bin lingxi-cli ; echo EXIT=$?`
Expected: PASS. Note the path: `<repo>/lingxi-code/target/debug/lingxi-cli`.

- [ ] **Step 2: Write the integration test**

Create `tui/tests/osc11_theme_pty.py`:

```python
"""End-to-end OSC-11 theme detection: the test plays the terminal and answers
the background query, then checks which theme the picker renders.

Run from the lingxi-code dir:
    python tui/tests/osc11_theme_pty.py
Requires: pexpect, pyte. Exits non-zero on failure.
"""
import os, sys, time, tempfile, shutil
import pexpect, pyte

BIN = os.path.join(os.getcwd(), "target", "debug", "lingxi-cli")
ROWS, COLS = 30, 100
OSC11_QUERY = b"\x1b]11;?"

def run(reply: bytes | None):
    """Launch lingxi-cli; answer the OSC-11 query with `reply` (or ignore it).
    Returns the fg color (pyte) of the input prompt glyph row's first text cell."""
    home = tempfile.mkdtemp(prefix="lingxi-osc-")
    env = dict(os.environ)
    env["TERM"] = "xterm-256color"; env["COLORTERM"] = "truecolor"
    env["RUST_LOG"] = "off"; env.pop("NO_COLOR", None); env["HOME"] = home
    env.pop("COLORFGBG", None)  # force reliance on OSC-11
    child = pexpect.spawn(BIN, cwd=os.getcwd(), env=env, encoding=None,
                          dimensions=(ROWS, COLS), timeout=20)
    screen = pyte.Screen(COLS, ROWS); stream = pyte.ByteStream(screen)
    # Answer (or ignore) the OSC-11 query in the startup window.
    if reply is not None:
        try:
            child.expect(OSC11_QUERY, timeout=5)
            child.send(reply)
        except pexpect.TIMEOUT:
            print("FAIL: never saw the OSC-11 query"); child.close(force=True)
            shutil.rmtree(home, ignore_errors=True); return None
    # Pump a few seconds of rendering.
    end = time.time() + 5
    while time.time() < end:
        try:
            data = child.read_nonblocking(8192, timeout=0.2)
            if data: stream.feed(data)
        except pexpect.TIMEOUT: pass
        except Exception: break
    # The input prompt glyph row: find the row containing the prompt glyph.
    fg = None
    for y, line in enumerate(screen.display):
        if line.strip().startswith("❯") or line.strip().startswith("❱"):
            for x in range(COLS):
                c = screen.buffer[y][x]
                if c.data.strip():
                    fg = c.fg; break
            break
    child.sendcontrol("c"); time.sleep(0.2); child.sendcontrol("c"); time.sleep(0.2)
    try: child.close(force=True)
    except Exception: pass
    shutil.rmtree(home, ignore_errors=True)
    return fg

def main():
    failures = []
    # Light reply (white bg) → Light theme → dark text (NOT white 'ffffff'/'default').
    light_fg = run(b"\x1b]11;rgb:ffff/ffff/ffff\x07")
    print("light-reply prompt fg:", light_fg)
    if light_fg in (None, "ffffff"):
        failures.append(f"light terminal did not yield dark text (fg={light_fg})")
    # Dark reply (black bg) → Dark theme → light text.
    dark_fg = run(b"\x1b]11;rgb:0000/0000/0000\x07")
    print("dark-reply prompt fg:", dark_fg)
    # No reply → must not hang; the run returning at all proves no hang.
    no_reply_fg = run(None)
    print("no-reply prompt fg (fallback Dark):", no_reply_fg)
    # Light and dark must differ (the theme actually changed).
    if light_fg == dark_fg:
        failures.append(f"light and dark replies produced the same fg ({light_fg})")
    if failures:
        print("FAILURES:"); [print("  -", f) for f in failures]; sys.exit(1)
    print("OK: OSC-11 theme detection switches light/dark and survives no-reply")

if __name__ == "__main__":
    main()
```

- [ ] **Step 3: Run the integration test**

Run (from `lingxi-code/`): `python tui/tests/osc11_theme_pty.py ; echo EXIT=$?`
Expected: prints differing `light-reply` vs `dark-reply` prompt fg colors and `OK: ...`; `EXIT=0`. (If `pexpect`/`pyte` aren't installed: `pip install pexpect pyte` first.)

- [ ] **Step 4: Commit**

```bash
git add tui/tests/osc11_theme_pty.py
git commit -m "test(tui): PTY OSC-11 theme-detection integration (harness answers the query)"
```

---

## Self-Review

**Spec coverage:**
- OSC-11 background query + BT.709 luminance → Tasks 1, 4, 5. ✓
- `Auto.resolve()` = detected → COLORFGBG → Dark, combined with depth → Task 3. ✓
- Color-depth detection + `-ansi` degradation selection → Tasks 2, 3. ✓
- Startup wiring before first `Auto.resolve()` (`AppState::new`) → Task 5. ✓
- Error handling: not-a-TTY / no-reply / malformed / raw-mode-fail / no hang → Tasks 4 (no-reply → None), 5 (TTY guard, raw-mode guard, TimedStdin deadline). ✓
- Untouched palettes/wire strings → enforced by Global Constraints + Task 3 Step 6 regression run. ✓
- Testing: pure units (1–4), PTY integration (6). ✓
- Known limitation (tmux passthrough) — inherent to the hand-rolled choice; no task needed (documented in spec). ✓

**Placeholder scan:** No TBD/TODO; every code step shows complete code; commands have expected output. ✓

**Type consistency:** `ColorDepth` (Task 2) used identically in Tasks 2–3; `resolve_theme`/`resolve_auto` signatures match between definition (Task 3) and `Auto.resolve()` call; `detect_with_io`/`set_detected_background`/`detected_background` signatures consistent across Tasks 3–5; `parse_osc11_rgb`/`luminance_is_light` consistent Tasks 1, 4. ✓

**Note for the implementer:** Tasks 1→5 are strictly ordered (each consumes the prior). Task 6 needs the binary built (Task 5 complete).
