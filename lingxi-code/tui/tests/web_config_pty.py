"""End-to-end PTY smoke for the /web provider configuration picker.

Run from the lingxi-code dir:
    python3 tui/tests/web_config_pty.py

The test accepts the startup trust dialog, opens `/web`, verifies the provider
picker and detail section render, moves the selection, and verifies the detail
changes. It spawns the specific child and closes it; never broad-pkill.
"""
import os, sys, time, tempfile, shutil
import pexpect, pyte

BIN = os.path.join(os.getcwd(), "target", "debug", "lingxi-cli")
ROWS, COLS = 30, 100


def frame_text(screen: "pyte.Screen") -> str:
    return "\n".join(screen.display)


def run() -> str:
    home = tempfile.mkdtemp(prefix="lingxi-web-")
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
        pump(3)
        child.send(b"\r")
        pump(1)

        child.send(b"/web")
        pump(0.5)
        child.send(b"\x1b")
        pump(0.3)
        child.send(b"\r")
        pump(2)
        first = frame_text(screen)

        child.send(b"\x1b[B")
        pump(1.5)
        second = frame_text(screen)
    finally:
        child.sendcontrol("c"); time.sleep(0.2); child.sendcontrol("c"); time.sleep(0.2)
        try: child.close(force=True)
        except Exception: pass
        shutil.rmtree(home, ignore_errors=True)

    return first + "\n=====SELECTION-MOVED=====\n" + second


def main() -> None:
    failures = []
    combined = run()
    first, second = combined.split("=====SELECTION-MOVED=====")
    print("first frame:\n", first)
    print("second frame:\n", second)

    for expected in ["Configure web search", "Auto", "DuckDuckGo", "Enter configure/select"]:
        if expected not in first:
            failures.append(f"missing {expected!r} in /web picker")

    if "Auto — active" not in first:
        failures.append("first detail does not show Auto active")
    if "DuckDuckGo" not in second or "keyless" not in second:
        failures.append("Down did not switch detail to DuckDuckGo/keyless")

    if failures:
        print("FAILURES:")
        for f in failures:
            print("  -", f)
        sys.exit(1)

    print("OK: /web picker renders providers and updates detail on selection")


if __name__ == "__main__":
    main()
