#!/usr/bin/env python3
"""Diff what the port's CLI DOES against a Claude Code oracle, by execution.

    scripts/parity_behaviour.py <oracle-binary> [--cli target/debug/lingxi-cli]
                                [--only mcp] [--show-identical]

`parity_surface.py` compares which flags EXIST. This compares what running a
command actually produces: stdout, stderr, and exit status, for both binaries,
against the same sandboxed HOME.

WHY A SCRIPT AND NOT AD-HOC SHELL. Four separate findings during the 2.1.220
audit were artifacts of the probe rather than facts about the code:

  * zsh does not word-split an unquoted variable, so `$cmd --help` with
    cmd="mcp xaa setup" arrives as ONE argv element. The CLI does not recognise
    it, falls back to ROOT help, and the diff reads as "five subcommands are
    missing". The same bug made both binaries treat "auto-mode config" as a
    chat PROMPT and run a real model session.
  * Two "findings" were interleaved stdout from concurrent processes sharing a
    terminal; each vanished when re-run alone with stdin closed.

So this harness never goes through a shell: argv is always a list, stdin is
always /dev/null, and every command runs in its own sandboxed HOME.

NORMALISATION. The port is a differently-named product, so a raw diff is all
branding noise. Known-legitimate rebrands are folded to a neutral token before
comparing (binary name, product name, dot-dir, memory filename). What survives
is a candidate BEHAVIOUR difference — still a hypothesis, because two sides can
print the same bytes and write different files.

A command is only compared when BOTH sides are non-interactive and terminating.
Anything that would open a browser, prompt, or start a server is skipped by
name and reported as skipped, so the summary never implies coverage it does not
have.
"""

from __future__ import annotations

import argparse
import os
import re
import shutil
import subprocess
import sys
import tempfile

TIMEOUT_S = 60

# Commands that block, prompt, serve, or reach the network. Skipped BY NAME and
# reported, rather than silently excluded.
SKIP = {
    ("mcp", "serve"),
    ("mcp", "login"),
    ("mcp", "logout"),
    ("mcp", "xaa", "login"),
    ("auth", "login"),
    ("auth", "logout"),
    ("setup-token",),
    ("install",),
    ("update",),
    ("gateway",),
    ("remote-control",),
    ("doctor",),
    ("ultrareview",),
    ("attach",),
    ("agents",),
}

# Read-only verbs worth running with no arguments.
PROBES: list[tuple[str, ...]] = [
    ("mcp", "list"),
    ("mcp", "reset-project-choices"),
    ("mcp", "xaa", "show"),
    ("plugin", "list"),
    ("plugin", "marketplace", "list"),
    ("project", "list"),
    ("auto-mode", "config"),
    ("auto-mode", "defaults"),
    ("auto-mode", "critique"),
    ("auth", "status"),
    ("mcp", "get", "nonexistent-server"),
    ("plugin", "validate"),
]


# Commands that WRITE. Each entry is a sequence of argv runs applied to the same
# sandboxed HOME, after which the resulting config is compared.
#
# stdout alone is not enough here: two sides can print the same confirmation and
# persist different JSON. The config FILE is the assertion.
WRITE_PROBES: list[tuple[str, list[list[str]]]] = [
    ("mcp add stdio (user)", [["mcp", "add", "demo", "--scope", "user", "--", "echo", "hi"]]),
    ("mcp add http (user)",
     [["mcp", "add", "web", "--scope", "user", "--transport", "http", "https://x.example/mcp"]]),
    ("mcp add-json (user)",
     [["mcp", "add-json", "js", '{"type":"stdio","command":"echo","args":["j"]}', "--scope", "user"]]),
    ("mcp add then remove",
     [["mcp", "add", "gone", "--scope", "user", "--", "echo", "x"],
      ["mcp", "remove", "gone", "--scope", "user"]]),
    ("mcp add duplicate name",
     [["mcp", "add", "dup", "--scope", "user", "--", "echo", "1"],
      ["mcp", "add", "dup", "--scope", "user", "--", "echo", "2"]]),
]

# Config keys the oracle writes as first-run bootstrap/telemetry state and the
# port deliberately does not (machine + user IDs, migration flags). Compared
# separately would be noise; they are not what these commands are about.
BOOTSTRAP_KEYS = {
    "firstStartTime", "machineID", "userID", "migrationVersion",
    "opusProMigrationComplete", "sonnet1m45MigrationComplete",
    "seenNotifications", "hasResetAutoModeOptInForDefaultOffer",
    "hasCompletedOnboarding", "lastOnboardingVersion", "projects",
    "cachedChangelog", "changelogLastFetched", "fallbackAvailableWarningThreshold",
    "subscriptionNoticeCount", "hasAvailableSubscription", "installMethod",
}


def read_config(home: str) -> dict:
    """The user config each side writes, minus first-run bootstrap keys."""
    import json

    for name in (".claude.json", ".lingxi.json"):
        path = os.path.join(home, name)
        if os.path.isfile(path):
            try:
                with open(path) as fh:
                    doc = json.load(fh)
            except (OSError, ValueError):
                return {"«unreadable»": name}
            return {k: v for k, v in doc.items() if k not in BOOTSTRAP_KEYS}
    return {}


def run_write_probes(oracle: str, cli: str, cli_name: str) -> tuple[int, list[str]]:
    import json

    same, diffs = 0, []
    for label, runs in WRITE_PROBES:
        oh, ph = tempfile.mkdtemp(prefix="wp-o-"), tempfile.mkdtemp(prefix="wp-p-")
        try:
            o_out, p_out = [], []
            for argv in runs:
                t, _ = run(oracle, argv, oh)
                o_out.append(normalise(t, "claude"))
                t, _ = run(cli, argv, ph)
                p_out.append(normalise(t, cli_name))
            o_cfg, p_cfg = read_config(oh), read_config(ph)
        finally:
            shutil.rmtree(oh, ignore_errors=True)
            shutil.rmtree(ph, ignore_errors=True)
        # Paths inside stdout already normalise to «tmp».
        if o_cfg == p_cfg and o_out == p_out:
            same += 1
            continue
        detail = [f"\n=== WRITE DIFFERS: {label} ==="]
        if o_out != p_out:
            for a, b in zip(o_out, p_out):
                if a != b:
                    detail.append(f"  stdout oracle: {a[:200]}")
                    detail.append(f"  stdout port  : {b[:200]}")
        if o_cfg != p_cfg:
            detail.append(f"  config oracle: {json.dumps(o_cfg, sort_keys=True)[:400]}")
            detail.append(f"  config port  : {json.dumps(p_cfg, sort_keys=True)[:400]}")
        diffs.append("\n".join(detail))
    return same, diffs


def normalise(text: str, cli_name: str) -> str:
    """Fold known-legitimate branding differences to neutral tokens."""
    t = text
    # ORDER MATTERS. The dot-dir and memory-file rebrands must fold BEFORE the
    # bare product name, or ".claude" becomes ".«cli»" while ".lingxi" becomes
    # "«dotdir»" and two identical lines are reported as differing. That bug
    # made `auto-mode defaults` look divergent right after it had been
    # regenerated to byte-identical.
    t = t.replace(".claude", "«dotdir»").replace(".lingxi", "«dotdir»")
    t = t.replace("CLAUDE.local.md", "«memlocal»").replace("LINGXI.local.md", "«memlocal»")
    t = t.replace("CLAUDE.md", "«memfile»").replace("LINGXI.md", "«memfile»")
    t = t.replace("Claude Code", "«product»").replace("LingXi", "«product»")
    t = t.replace(cli_name, "«cli»").replace("claude", "«cli»")
    # Absolute paths and timestamps differ per run, not per implementation.
    t = re.sub(r"/(?:private/)?(?:tmp|var)/[^\s\"']+", "«tmp»", t)
    t = re.sub(r"\d{4}-\d{2}-\d{2}T[\d:.]+Z?", "«ts»", t)
    # The port emits provider-boot diagnostics the oracle has no concept of.
    t = "\n".join(
        ln
        for ln in t.splitlines()
        if "WARN engine_desktop" not in ln and not ln.startswith("Note: default model")
    )
    return t.strip()


def run(binary: str, argv: list[str], home: str) -> tuple[str, int]:
    env = dict(os.environ, HOME=home, LINGXI_HOME=os.path.join(home, ".lingxi"))
    env.pop("CLAUDE_CODE_OAUTH_TOKEN", None)
    # Both sides gate the XAA group on this; without it neither registers the
    # group and the `mcp xaa` probes compare two "unknown command" errors.
    env["CLAUDE_CODE_ENABLE_XAA"] = "1"
    env["LINGXI_ENABLE_XAA"] = "1"
    try:
        r = subprocess.run(
            [binary, *argv],
            capture_output=True,
            text=True,
            timeout=TIMEOUT_S,
            stdin=subprocess.DEVNULL,
            env=env,
        )
    except subprocess.TimeoutExpired:
        return ("«TIMEOUT»", -1)
    except OSError as e:
        return (f"«SPAWN FAILED: {e}»", -1)
    return (r.stdout + r.stderr, r.returncode)


def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument("oracle")
    ap.add_argument("--cli", default="target/debug/lingxi-cli")
    ap.add_argument("--only", help="only probe paths starting with this token")
    ap.add_argument("--show-identical", action="store_true")
    args = ap.parse_args()

    cli_name = os.path.basename(args.cli)
    probes = [p for p in PROBES if not args.only or p[0] == args.only]

    same, differ, skipped = 0, [], []
    for path in probes:
        if path in SKIP:
            skipped.append(path)
            continue
        # A fresh HOME per command, so one command's writes cannot change the
        # next command's reading.
        oh, ph = tempfile.mkdtemp(prefix="par-o-"), tempfile.mkdtemp(prefix="par-p-")
        try:
            o_out, o_rc = run(args.oracle, list(path), oh)
            p_out, p_rc = run(args.cli, list(path), ph)
        finally:
            shutil.rmtree(oh, ignore_errors=True)
            shutil.rmtree(ph, ignore_errors=True)

        o_n, p_n = normalise(o_out, "claude"), normalise(p_out, cli_name)
        if o_n == p_n and o_rc == p_rc:
            same += 1
            if args.show_identical:
                print(f"IDENTICAL  {' '.join(path)}")
            continue
        differ.append((path, o_n, p_n, o_rc, p_rc))

    for path, o_n, p_n, o_rc, p_rc in differ:
        print(f"\n=== DIFFERS: {' '.join(path)}  (rc {o_rc} vs {p_rc}) ===")
        ol, pl = o_n.splitlines(), p_n.splitlines()
        # Show the FIRST DIFFERING line, not the head: for a large document the
        # heads are identical and printing them says nothing about the defect.
        i = next(
            (i for i in range(max(len(ol), len(pl)))
             if (ol[i] if i < len(ol) else None) != (pl[i] if i < len(pl) else None)),
            None,
        )
        if i is None:
            print(f"  (text identical; differs only in exit status {o_rc} vs {p_rc})")
            continue
        print(f"  first difference at line {i + 1} of {len(ol)}/{len(pl)}")
        print(f"  oracle: {(ol[i] if i < len(ol) else '«missing line»')[:400]}")
        print(f"  port  : {(pl[i] if i < len(pl) else '«missing line»')[:400]}")

    w_same, w_diffs = run_write_probes(args.oracle, args.cli, cli_name)
    for d in w_diffs:
        print(d)
    print(f"\nwrite probes identical: {w_same}   differing: {len(w_diffs)}")

    print(
        f"\nidentical: {same}   differing: {len(differ)}   "
        f"skipped (interactive/network/server): {len(skipped)}"
    )
    for p in skipped:
        print(f"  skipped: {' '.join(p)}")
    print(
        "\nA difference here is a HYPOTHESIS about behaviour, not a finding: two\n"
        "sides can print identical bytes and still write different files. Confirm\n"
        "at the call site before recording anything."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
