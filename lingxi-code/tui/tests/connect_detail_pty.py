"""End-to-end PTY check for the /connect picker detail panel (T2b).

The test plays the terminal: accepts the startup trust dialog, types `/connect`
to open the bare-provider picker, then asserts the highlighted provider's
DETAIL SECTION (connected state, models, sign-in method) is on the framebuffer,
and that moving the selection (Down) updates which provider's detail is shown.

Run from the lingxi-code dir:
    python tui/tests/connect_detail_pty.py
Requires: pexpect, pyte. Exits non-zero on failure.

The default catalog surfaces anthropic / github-copilot / openai-chatgpt, so the
picker is non-empty on a fresh temp HOME; the detail section therefore renders
"Models: …", "Sign in: …", and "<label> — not connected" for the highlighted
provider. Pressing Down switches the highlighted provider, so the "<label> —"
detail line changes. Spawns the SPECIFIC child and closes it; never broad-pkill.
"""
import os, sys, time, tempfile, shutil
import pexpect, pyte

BIN = os.path.join(os.getcwd(), "target", "debug", "lingxi-cli")
ROWS, COLS = 30, 100


def frame_text(screen: "pyte.Screen") -> str:
    return "\n".join(screen.display)


def run() -> str:
    """Launch lingxi-cli; accept the trust dialog; open /connect; capture the
    framebuffer with the FIRST provider highlighted, then press Down and capture
    again. Returns the two framebuffers joined by a marker for assertion."""
    home = tempfile.mkdtemp(prefix="lingxi-connect-")
    env = dict(os.environ)
    env["TERM"] = "xterm-256color"; env["COLORTERM"] = "truecolor"
    env["RUST_LOG"] = "off"; env.pop("NO_COLOR", None); env["HOME"] = home

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

    try:
        # Step 1: startup trust dialog → accept it.
        pump(3)
        child.send(b"\r")
        pump(1)

        # Step 2: type `/connect` then submit → open the bare provider picker.
        # Typing triggers the command palette; Esc dismisses it so Enter submits
        # the line instead of accepting a completion.
        child.send(b"/connect")
        pump(0.5)
        child.send(b"\x1b")  # Esc: dismiss palette
        pump(0.3)
        child.send(b"\r")    # submit /connect
        pump(2)
        first = frame_text(screen)

        # Step 3: move selection down → highlighted provider changes.
        child.send(b"\x1b[B")  # Down arrow
        pump(1.5)
        second = frame_text(screen)
    finally:
        child.sendcontrol("c"); time.sleep(0.2); child.sendcontrol("c"); time.sleep(0.2)
        try: child.close(force=True)
        except Exception: pass
        shutil.rmtree(home, ignore_errors=True)

    return first + "\n=====SELECTION-MOVED=====\n" + second


def detail_label_line(frame: str) -> str | None:
    """The '<label> — connected/not connected' detail line for the highlight."""
    for line in frame.splitlines():
        if "\u2014" in line and ("connected" in line):  # em-dash + state
            return line.strip()
    return None


def main() -> None:
    failures = []
    combined = run()
    first, second = combined.split("=====SELECTION-MOVED=====")
    print("first frame:\n", first)
    print("second frame:\n", second)

    # The detail section must render its fixed labels for the highlighted row.
    if "Models:" not in first:
        failures.append("detail section 'Models:' line absent from picker")
    if "Sign in:" not in first:
        failures.append("detail section 'Sign in:' line absent from picker")

    # Highlighted provider state line ("<label> — not connected") present.
    l1 = detail_label_line(first)
    l2 = detail_label_line(second)
    if l1 is None:
        failures.append("no '<label> — connected/not connected' detail line on first frame")
    # Moving the selection must update which provider's detail is shown.
    if l1 is not None and l2 is not None and l1 == l2:
        failures.append(f"selection move did not change detail line ({l1!r})")

    if failures:
        print("FAILURES:")
        for f in failures:
            print("  -", f)
        sys.exit(1)

    print("OK: /connect picker shows per-provider detail; selection updates it")


if __name__ == "__main__":
    main()
