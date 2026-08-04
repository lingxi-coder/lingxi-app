#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
PINS="${REPO_ROOT}/docs/mobile-linux/mobile-linux-pins.json"
OPENMINIS="${OPENMINIS_SOURCE:-${REPO_ROOT}/docs/superpowers/references/OpenMinis}"
PROOT="${PROOT_SOURCE:-${OPENMINIS}/deps/proot}"

python3 - "${PINS}" "${OPENMINIS}" "${PROOT}" <<'PY'
import hashlib
import json
import pathlib
import subprocess
import sys

pins_path, openminis_path, proot_path = map(pathlib.Path, sys.argv[1:])
pins = json.loads(pins_path.read_text())

if pins.get("schema_version") != 1:
    raise SystemExit("unsupported mobile-linux pin schema")
if pins.get("supported_abis") != ["arm64-v8a", "x86_64"]:
    raise SystemExit("supported_abis must be exactly arm64-v8a and x86_64")

def git(repo: pathlib.Path, *args: str) -> bytes:
    return subprocess.check_output(["git", "-C", str(repo), *args])

def verify_repo(name: str, repo: pathlib.Path) -> None:
    component = pins["components"][name]
    commit = component["commit"]
    actual_commit = git(repo, "rev-parse", "HEAD").decode().strip()
    if actual_commit != commit:
        raise SystemExit(f"{name} HEAD mismatch: expected {commit}, got {actual_commit}")
    archive = git(repo, "archive", "--format=tar", commit)
    actual_archive_hash = hashlib.sha256(archive).hexdigest()
    if actual_archive_hash != component["git_archive_sha256"]:
        raise SystemExit(f"{name} git archive SHA-256 mismatch")

verify_repo("openminis", openminis_path)
verify_repo("proot", proot_path)

pty = pins["components"]["pty_bridge"]
pty_path = openminis_path / pty["path"]
if hashlib.sha256(pty_path.read_bytes()).hexdigest() != pty["sha256"]:
    raise SystemExit("pinned PTY bridge source SHA-256 mismatch")

talloc = pins["components"]["talloc"]
for relative_path, expected_hash in talloc["openminis_vendored_files"].items():
    actual_hash = hashlib.sha256((openminis_path / relative_path).read_bytes()).hexdigest()
    if actual_hash != expected_hash:
        raise SystemExit(f"pinned talloc source SHA-256 mismatch: {relative_path}")

for abi, item in pins["rootfs"]["archives"].items():
    if len(item["sha256"]) != 64 or any(c not in "0123456789abcdef" for c in item["sha256"]):
        raise SystemExit(f"invalid rootfs SHA-256 for {abi}")

# NOTE: this script deliberately does NOT validate `rootfs.release_archives`.
# No such key is committed (see the "KNOWN UNANCHORED STEP" section of
# docs/mobile-linux/README.md), so any loop over it here iterates zero times and
# turns a documented gap into a green check. `rootfs_tool.py
# verify-release-archive` validates that pin's shape at the point of use, where
# a malformed value has something to reject.

print(f"mobile-linux pins verified: {pins_path}")
PY

bash "${SCRIPT_DIR}/verify-local-app-supply-chain.sh"
