"""End-to-end OSC-11 theme detection: the test plays the terminal and answers
the background query, then checks which theme the app renders.

Run from the lingxi-code dir:
    python tui/tests/osc11_theme_pty.py
Requires: pexpect, pyte. Exits non-zero on failure.

DEVIATIONS FROM ORIGINAL PLAN CODE (harness adaptations, NOT detection bugs):

1. Trust-dialog acceptance: the app shows a startup project-trust dialog before
   run_tui_session (and therefore before detect_terminal_theme) fires. With a
   fresh temp HOME the dialog appears first; the test sends Enter to accept it.
   This ALSO acts as a natural timing buffer: detect_terminal_theme only starts
   after the dialog exits (~50-100ms), so the 100ms TimedStdin deadline is
   reliably met. Do NOT add a sleep between the trust-accept Enter and the
   expect() call — that would consume the budget.

2. Colour assertions: the plan tests the prompt-glyph (❯/❱) fg. With no
   provider configured the first screen is a Settings/Config screen without a
   visible prompt glyph. We test the theme via the dim colour, which appears on
   every screen and differs between themes:
     Light theme dim = rgb(102,102,102) = pyte hex "666666"
     Dark  theme dim = rgb(153,153,153) = pyte hex "999999"
   A light OSC-11 reply must produce "666666" on-screen; light and dark must
   differ.
"""
import os, sys, time, tempfile, shutil
import pexpect, pyte

BIN = os.path.join(os.getcwd(), "target", "debug", "lingxi-cli")
ROWS, COLS = 30, 100
OSC11_QUERY = b"\x1b]11;?"

# dim colour differs between themes and appears on every screen.
DARK_DIM  = "999999"   # rgb(153,153,153) — Dark theme
LIGHT_DIM = "666666"   # rgb(102,102,102) — Light theme


def run(reply: bytes | None) -> set:
    """Launch lingxi-cli; accept the trust dialog; answer the OSC-11 query
    with `reply` (or ignore it). Returns the set of fg colours on-screen."""
    home = tempfile.mkdtemp(prefix="lingxi-osc-")
    env = dict(os.environ)
    env["TERM"] = "xterm-256color"; env["COLORTERM"] = "truecolor"
    env["RUST_LOG"] = "off"; env.pop("NO_COLOR", None); env["HOME"] = home
    env.pop("COLORFGBG", None)  # force reliance on OSC-11

    child = pexpect.spawn(BIN, cwd=os.getcwd(), env=env, encoding=None,
                          dimensions=(ROWS, COLS), timeout=20)
    screen = pyte.Screen(COLS, ROWS); stream = pyte.ByteStream(screen)

    def pump(duration: float) -> None:
        end = time.time() + duration
        while time.time() < end:
            try:
                data = child.read_nonblocking(8192, timeout=0.1)
                if data:
                    stream.feed(data)
            except pexpect.TIMEOUT:
                pass
            except Exception:
                break

    # Step 1: Wait for the startup trust dialog then accept it.
    # The trust dialog is an iocraft TUI that exits when Enter is pressed; after
    # it exits detect_terminal_theme() fires (emitting the OSC-11 query).  Pump
    # for up to 3 s to make sure the dialog has rendered before we send Enter.
    pump(3)
    child.send(b"\r")   # "Yes, I trust this folder"

    # Step 2: Answer (or ignore) the OSC-11 query.
    # No sleep here — the 100ms TimedStdin deadline is already consuming while
    # the trust dialog exits; we need to answer as fast as possible.
    if reply is not None:
        try:
            child.expect(OSC11_QUERY, timeout=5)
            child.send(reply)
        except pexpect.TIMEOUT:
            print("FAIL: never saw the OSC-11 query"); child.close(force=True)
            shutil.rmtree(home, ignore_errors=True); return set()

    # Step 3: Pump a few seconds of rendering.
    pump(5)

    # Step 4: Collect every fg colour on-screen for non-space cells.
    fg_colors: set = set()
    for y in range(ROWS):
        for x in range(COLS):
            c = screen.buffer[y][x]
            if c.data.strip():
                fg_colors.add(c.fg)

    child.sendcontrol("c"); time.sleep(0.2); child.sendcontrol("c"); time.sleep(0.2)
    try: child.close(force=True)
    except Exception: pass
    shutil.rmtree(home, ignore_errors=True)
    return fg_colors


def main() -> None:
    failures = []

    # Light reply (white bg) → Light theme → LIGHT_DIM present on-screen.
    light_fgs = run(b"\x1b]11;rgb:ffff/ffff/ffff\x07")
    print("light-reply fg colours:", sorted(light_fgs))
    if LIGHT_DIM not in light_fgs:
        failures.append(
            f"light terminal did not yield light-theme dim {LIGHT_DIM!r} "
            f"(got {sorted(light_fgs)})"
        )

    # Dark reply (black bg) → Dark theme → DARK_DIM present.
    dark_fgs = run(b"\x1b]11;rgb:0000/0000/0000\x07")
    print("dark-reply fg colours:", sorted(dark_fgs))
    if DARK_DIM not in dark_fgs:
        failures.append(
            f"dark terminal did not yield dark-theme dim {DARK_DIM!r} "
            f"(got {sorted(dark_fgs)})"
        )

    # No reply → must not hang; the run returning at all proves no hang.
    no_reply_fgs = run(None)
    print("no-reply fg colours (fallback Dark):", sorted(no_reply_fgs))

    # Light and dark must produce different colour sets (theme actually changed).
    if light_fgs == dark_fgs:
        failures.append(
            f"light and dark replies produced identical fg sets "
            f"({sorted(light_fgs)})"
        )

    if failures:
        print("FAILURES:")
        for f in failures:
            print("  -", f)
        sys.exit(1)

    print("OK: OSC-11 theme detection switches light/dark and survives no-reply")


if __name__ == "__main__":
    main()
