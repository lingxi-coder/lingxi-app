#!/usr/bin/env python3
"""Compare the port's CLI surface against a Claude Code oracle binary.

    scripts/parity_surface.py <oracle-binary> [--cli target/release/lingxi-cli]

Why this exists as a script rather than an ad-hoc command: three separate
ad-hoc probes during the 2.1.220 audit produced findings that were entirely
artifacts of the probe.

  * A one-level walk of the port's subcommands, diffed against an oracle that
    registers most flags on NESTED ones, reported 39 missing flags. The real
    number was 2. Every "missing" flag was present one level down.
  * `--help` on an UNRECOGNISED path silently prints the ROOT help instead of
    failing, so a comparison that trusts exit status reads the root
    description as the description of 20 different subcommands.
  * A grep pattern for two identifiers that never share a line reported a
    feature unimplemented; the call site had been there for months.

Each of those cost a correction cycle, and one of them was committed and
recommended as work before being caught. So this script REFUSES TO REPORT
until it has proved it can see things it knows are there.

SELF-VERIFICATION (`--check`, run automatically before every comparison):

  1. Every command path is resolved by walking real `Commands:` blocks, never
     by guessing a prefix.
  2. A path is only accepted if its help is DISTINCT from the root help —
     the direct defence against the silent root-help fallback.
  3. The walk must find a set of known-nested flags (`plugin prune --dry-run`
     and friends). If it does not, the traversal is broken and the script
     exits non-zero WITHOUT printing a gap list, because a broken traversal
     does not produce a small wrong answer — it produces a large one.

An absence this script reports has survived those checks. It is still a
HYPOTHESIS about the surface, not a finding about behaviour: two sides can
advertise the same flag and do different things with it.
"""

from __future__ import annotations

import argparse
import re
import subprocess
import sys

# Flags known to live on NESTED subcommands. If the traversal cannot see these
# it is walking one level, which is the exact bug that produced the 39-flag
# phantom list — so their absence fails the run instead of being reported.
CANARY_NESTED_FLAGS = ["--dry-run", "--keep-data", "--strict", "--push"]

# Hidden compatibility command whose option string is present in the Claude
# binary but whose parent is feature-gated out of the public `mcp --help`.
# LingXi implements the same path; include it explicitly so a binary-string
# oracle does not report a false absence merely because help discovery cannot
# reach the gated parent.
PORT_EXTRA_PATHS = [["mcp", "xaa", "login"]]

HELP_TIMEOUT_S = 60


def run_help(cli: str, path: list[str]) -> str:
    try:
        r = subprocess.run(
            [cli, *path, "--help"], capture_output=True, text=True, timeout=HELP_TIMEOUT_S
        )
    except (OSError, subprocess.SubprocessError):
        return ""
    return r.stdout + r.stderr


def subcommands(help_text: str) -> list[str]:
    """Names inside the `Commands:` block, in order."""
    out: list[str] = []
    in_block = False
    for line in help_text.splitlines():
        if line.strip() == "Commands:":
            in_block = True
            continue
        if in_block:
            if not line.strip():
                break
            # Command rows start with EXACTLY two spaces. Wrapped description
            # lines are aligned farther right; accepting arbitrary indentation
            # previously invented paths such as `auto-mode and critique`.
            m = re.match(r"^  ([a-z][a-z0-9-]*(?:\|[a-z][a-z0-9-]*)*)\s", line)
            if m and m.group(1) != "help":
                # Commander renders aliases as `plugin|plugins`; either token
                # reaches the same command, so walk the canonical first name.
                out.append(m.group(1).split("|", 1)[0])
    return out


def walk(
    cli: str,
    max_depth: int = 3,
    extra_paths: list[list[str]] | None = None,
) -> tuple[dict[str, str], set[str]]:
    """Every reachable command path → its help text, plus every long flag.

    A path whose help is IDENTICAL to the root's is discarded: that is the
    signature of the silent root-help fallback, not of a real command.
    """
    root = run_help(cli, [])
    if not root:
        return {}, set()
    helps: dict[str, str] = {"": root}
    flags: set[str] = set(re.findall(r"(--[a-z0-9][a-z0-9-]*)", root))
    stack: list[list[str]] = [[s] for s in subcommands(root)]
    stack.extend(extra_paths or [])
    while stack:
        path = stack.pop()
        text = run_help(cli, path)
        if not text or text == root:
            # Unrecognised path: the CLI fell back to root help.
            continue
        helps[" ".join(path)] = text
        flags |= set(re.findall(r"(--[a-z0-9][a-z0-9-]*)", text))
        if len(path) < max_depth:
            stack.extend(path + [s] for s in subcommands(text))
    return helps, flags


def oracle_flags(binary: str) -> set[str]:
    data = open(binary, "rb").read()
    return {m.group(1).decode() for m in re.finditer(rb'\.option\("(--[a-z0-9][a-z0-9-]*)', data)}


def self_check(helps: dict[str, str], flags: set[str]) -> list[str]:
    problems = []
    if len(helps) < 10:
        problems.append(f"only {len(helps)} command paths resolved — traversal looks broken")
    if not any(" " in p for p in helps):
        problems.append("no NESTED command path resolved — the walk stayed at depth 1")
    missing_canaries = [f for f in CANARY_NESTED_FLAGS if f not in flags]
    if missing_canaries:
        problems.append(
            "known-nested flags not seen: "
            + " ".join(missing_canaries)
            + " (the walk is not reaching nested subcommands)"
        )
    return problems


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("oracle", help="path to the Claude Code binary")
    ap.add_argument("--cli", default="target/release/lingxi-cli")
    ap.add_argument("--check", action="store_true", help="run the self-check and stop")
    args = ap.parse_args()

    helps, port = walk(args.cli, extra_paths=PORT_EXTRA_PATHS)
    problems = self_check(helps, port)
    if problems:
        print("SELF-CHECK FAILED — refusing to report a gap list:", file=sys.stderr)
        for p in problems:
            print(f"  - {p}", file=sys.stderr)
        print(
            "\nA broken traversal does not produce a small wrong answer, it produces\n"
            "a large one. Fix the walk before trusting any absence it reports.",
            file=sys.stderr,
        )
        return 2
    print(f"self-check OK: {len(helps)} command paths, {len(port)} long flags")
    if args.check:
        return 0

    oracle = oracle_flags(args.oracle)
    missing = sorted(oracle - port)
    print(f"\noracle flags: {len(oracle)}   port flags: {len(port)}")
    print(f"in oracle, not in port: {len(missing)}")
    for f in missing:
        print(f"  {f}")
    print(
        "\nThese are SURFACE hypotheses. Before recording any of them as a gap,\n"
        "confirm at the call site — `<cli> <sub> --help`, or the code path, not a\n"
        "grep pattern. Surface presence also says nothing about behaviour."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
