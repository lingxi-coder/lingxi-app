#!/usr/bin/env python3
"""Host-owned native half of the protocol's obsolete selector regression."""
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FILES = [
    "clients/ios/Sources/LocalApps/LocalAppsModels.swift",
    "clients/ios/Sources/LocalApps/LocalAppsProtocolAdapter.swift",
    "clients/ios/Sources/LocalApps/LocalAppsStore.swift",
    "clients/android/app/src/main/java/com/lingxi/code/localapps/LocalAppsContract.kt",
    "clients/android/app/src/main/java/com/lingxi/code/localapps/LocalAppsViewModel.kt",
]
FORBIDDEN = ("RuntimeProfileSelection", "runtimeProfileSelection", "app_runtime_profile_selection_requested")
for relative in FILES:
    text = (ROOT / relative).read_text()
    for token in FORBIDDEN:
        if token in text:
            raise SystemExit(f"CLIENT-PROTOCOL FAIL: {relative} contains obsolete {token}")
print("CLIENT-PROTOCOL OK: all five native clients keep runtime-profile selection Host-owned")

# These small compile-time fixtures belong to retained host consumers. Keep
# their bytes tied to the same Cargo revision as the runtime, without invoking
# Cargo or reading its checkout in production application code.
import json
from runtime_source import resolve_runtime

runtime_root = Path(resolve_runtime()["root"])
mirrors = json.loads(Path(__file__).with_name("runtime-fixture-mirrors.json").read_text())
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
