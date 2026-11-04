#!/usr/bin/env python3
"""Exit successfully only when a main push changes audit records alone.

Any unknown event or Git failure requires the normal client jobs to run. The
workflow fetches complete history before calling this script; the diff itself
uses the immutable endpoints from the push event, not the current branch tip.
"""

import json
import os
import re
import subprocess
import sys


def git(*args: str) -> bytes:
    return subprocess.run(
        ["git", "--no-replace-objects", *args],
        check=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    ).stdout


def audit_only() -> bool:
    if os.environ.get("GITHUB_EVENT_NAME") != "push":
        return False
    if os.environ.get("GITHUB_REF") != "refs/heads/main":
        return False

    with open(os.environ["GITHUB_EVENT_PATH"], encoding="utf-8") as event_file:
        event = json.load(event_file)
    if not isinstance(event, dict) or event.get("ref") != "refs/heads/main":
        return False

    before, after = event.get("before"), event.get("after")
    if not all(isinstance(sha, str) and re.fullmatch(r"[0-9a-f]{40}", sha) for sha in (before, after)):
        return False
    if before == "0" * 40 or after == "0" * 40 or before == after:
        return False
    if after != os.environ.get("GITHUB_SHA"):
        return False

    # A shallow or incomplete history cannot prove that before is an ancestor.
    if git("rev-parse", "--is-shallow-repository").strip() != b"false":
        return False
    for sha in (before, after):
        if git("rev-parse", "--verify", f"{sha}^{{commit}}").strip() != sha.encode("ascii"):
            return False
    if git("rev-parse", "HEAD").strip() != after.encode("ascii"):
        return False
    git("merge-base", "--is-ancestor", before, after)

    # Disabling rename detection reports both the deleted old path and added
    # new path, including for moves into or out of the allowlisted directory.
    changed = git("diff", "--no-ext-diff", "--no-textconv", "--no-renames", "--name-only", "-z", before, after, "--")
    if not changed or not changed.endswith(b"\0"):
        return False
    paths = changed[:-1].split(b"\0")
    return all(
        path == b"supply-chain/audits.toml"
        or (path.startswith(b"supply-chain/reviews/") and path != b"supply-chain/reviews/")
        for path in paths
    )


def main() -> int:
    try:
        only_audits = audit_only()
    except (OSError, KeyError, TypeError, ValueError, subprocess.SubprocessError):
        only_audits = False
    print("Audit-only push; client jobs may be skipped." if only_audits else "Run all client jobs.")
    return 0 if only_audits else 1


if __name__ == "__main__":
    sys.exit(main())
