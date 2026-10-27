#!/usr/bin/env python3
"""Check that the full Node test runs exercised their required integrations.

The test commands retain Node's default spec reporter.  Treat a missing or
unrecognizable summary as a failure, and enumerate the platform skips rather
than interpreting a successful `npm test` exit as evidence of coverage.
"""

import argparse
import re
import sys
from collections import Counter
from pathlib import Path

REQUIRED = {
    "sdk": {
        "REST writes and reads behave like the in-memory model (nulls, empties, arrays, timestamps)",
        "listen delivers an initial put, then events that rebuild the same tree; a reconnect repeats the snapshot",
        "a tunnel runs over real streams: both directions, a 300 KB message, cleanup on close",
        "a peer close rejects pending RPCs over real RTDB streams in both directions",
        "another user's subtree is refused: a request fails with 401 after one fresh token, a stream ends once and is not retried",
        "read access withdrawn from a live stream ends it with one cancel, and the client does not reconnect",
    },
    "electron": {
        "7a. over a real RtdbClient and the repository rules: first pairing, a second connection, clean-up",
        "7b. over a real RtdbClient and the repository rules: the session flow, ciphertext only, nothing left behind",
        "7c. the namespace under test really enforces the repository rules (so 7a and 7b passing means the stack satisfies them)",
        "real Rust library owns Desktop static, Python and Node fixture lifecycles",
        "Desktop retains the serving release on commit failure and reuses prepared environments until Repair",
        "real Rust broker: cwd, UTF-8, control, resize, isolation, migration and cleanup",
        "raw Rust broker SIGTERM cleans jobs with stdin open and stdout backpressured",
        "raw Rust broker acknowledges real stdin EOF after terminal cleanup",
        "raw Rust broker rejects input probe after shell exit",
        "Linux loader resolves one app-owned ORIGIN entry through separator and token characters",
        "real Electron Local Apps: tool transport, library, independent windows, backends, permissions and durable updates",
        "real Electron Remote Control transport: hidden WebRTC window, MessagePort, 2 MiB both ways, lockdown, RtdbClient over net.fetch",
    },
}

# These tests are present in the full suite but intentionally require macOS or
# Windows APIs. A new skip, or a different reason, requires explicit review.
LINUX_SKIPS = {
    "native application lookup returns a bounded PNG and missing apps fall back": "SKIP",
    "native credential transactions prevent deleted values from being recached": "SKIP",
    "native broker registration recovers a matching plist after bootstrap failure": "SKIP",
    "native broker readiness gates operations without replaying mutations": "SKIP",
    "Electron 43 serializes browser File as an official inline AG-UI DataSource before IPC": "the isolated Electron 43 smoke runs on macOS",
    "native __FILE__ cannot disclose physical paths through a symlinked cache": "SKIP",
    "nested signing discovery includes Mach-O binaries and excludes ordinary resources": "requires native macOS binaries and plutil",
    "audio helper resource layout and plist declare the expected macOS voice permissions": "requires native macOS binaries and plutil",
    "the rewritten outer app Info.plist declares real native voice usage": "requires native macOS binaries and plutil",
    "macOS package verification requires the actual signed bundle to declare lingxi": "SKIP",
    "credential stability cannot substitute a development package for production": "SKIP",
    "the voice page reads the real OS microphone grant, not the page permission this app grants itself": "Electron exposes native media access status on macOS and Windows",
}

RESULT = re.compile(r"^\s*([✔﹣]) (.+) \([0-9]+(?:\.[0-9]+)?ms\)(?: # (.*))?$")
SUMMARY = re.compile(r"^ℹ (tests|suites|pass|fail|cancelled|skipped|todo) ([0-9]+)$")


def verify_log(text: str, suite: str) -> list[str]:
    passed: Counter[str] = Counter()
    skipped: Counter[tuple[str, str]] = Counter()
    summaries: dict[str, list[int]] = {}
    for line in text.splitlines():
        if match := RESULT.fullmatch(line):
            symbol, name, reason = match.groups()
            if symbol == "✔" and reason is None:
                passed[name] += 1
            elif symbol == "﹣":
                skipped[(name, reason or "")] += 1
        elif match := SUMMARY.fullmatch(line):
            name, count = match.groups()
            summaries.setdefault(name, []).append(int(count))

    errors: list[str] = []
    for name in sorted(REQUIRED[suite]):
        if passed[name] != 1:
            errors.append(f"required test did not pass exactly once: {name}")

    expected_skips = Counter(LINUX_SKIPS.items()) if suite == "electron" else Counter()
    for (name, reason), count in (skipped - expected_skips).items():
        errors.append(f"unexpected skipped test ({count}): {name} # {reason}")
    for (name, reason), count in (expected_skips - skipped).items():
        errors.append(f"expected Linux platform skip missing ({count}): {name} # {reason}")

    required_summary = ("tests", "suites", "pass", "fail", "cancelled", "skipped", "todo")
    for key in required_summary:
        if len(summaries.get(key, [])) != 1:
            errors.append(f"missing or ambiguous Node test summary: {key}")
    if all(len(summaries.get(key, [])) == 1 for key in required_summary):
        summary = {key: summaries[key][0] for key in required_summary}
        if summary["tests"] == 0 or summary["pass"] == 0:
            errors.append("the test run did not report any passing tests")
        for key in ("fail", "cancelled", "todo"):
            if summary[key] != 0:
                errors.append(f"Node reported {summary[key]} {key}")
        if summary["skipped"] != sum(skipped.values()):
            errors.append("the skipped summary does not match the recognized skip lines")
        if summary["pass"] + summary["fail"] + summary["cancelled"] + summary["skipped"] + summary["todo"] != summary["tests"]:
            errors.append("the Node test summary counts are inconsistent")
    return errors


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("suite", choices=tuple(REQUIRED))
    parser.add_argument("log", type=Path)
    args = parser.parse_args()
    errors = verify_log(args.log.read_text(encoding="utf-8"), args.suite)
    if errors:
        for error in errors:
            print(error, file=sys.stderr)
        return 1
    print(f"{args.suite}: required integrations passed; platform skips and Node summary verified")
    return 0


if __name__ == "__main__":
    sys.exit(main())
