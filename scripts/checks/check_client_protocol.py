#!/usr/bin/env python3
"""Host-owned native half of the protocol's obsolete selector regression."""
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
# The native clients never choose a runtime profile themselves; the Host does. Scan every native source that is left, so
# a client that grows a selector again is caught wherever it is written. Only tracked files count: the UniFFI bindings
# are generated locally, ignored by git, and carry the runtime's own types.
tracked = subprocess.run(
    ["git", "ls-files", "-z", "--", "apps/ios/native", "apps/android/native"],
    cwd=ROOT, check=True, capture_output=True, text=True,
).stdout.split("\0")
NATIVE_SOURCES = [ROOT / name for name in sorted(tracked) if name.endswith((".swift", ".kt"))]
FORBIDDEN = ("RuntimeProfileSelection", "runtimeProfileSelection", "app_runtime_profile_selection_requested")
if not NATIVE_SOURCES:
    raise SystemExit("CLIENT-PROTOCOL FAIL: no native client sources found to scan")
for source in NATIVE_SOURCES:
    text = source.read_text()
    for token in FORBIDDEN:
        if token in text:
            raise SystemExit(f"CLIENT-PROTOCOL FAIL: {source.relative_to(ROOT)} contains obsolete {token}")
print(f"CLIENT-PROTOCOL OK: {len(NATIVE_SOURCES)} native client sources keep runtime-profile selection Host-owned")

# These small compile-time fixtures belong to retained host consumers. Keep
# their bytes tied to the same Cargo revision as the runtime, without invoking
# Cargo or reading its checkout in production application code.
import json
import sys
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))
from runtime_source import resolve_runtime

runtime_root = Path(resolve_runtime()["root"])
mirrors = json.loads((Path(__file__).resolve().parent / "../lib/runtime-fixture-mirrors.json").read_text())
if not mirrors:
    raise SystemExit("CLIENT-PROTOCOL FAIL: runtime fixture mirror inventory is empty")
for mirror in mirrors:
    host_path = ROOT / mirror["host"]
    upstream_path = runtime_root / mirror["upstream"]
    if host_path.read_bytes() != upstream_path.read_bytes():
        raise SystemExit(
            f"CLIENT-PROTOCOL FAIL: {mirror['host']} differs from pinned {mirror['upstream']}"
        )
print(f"CLIENT-PROTOCOL OK: {len(mirrors)} host fixture mirrors match the pinned runtime")
