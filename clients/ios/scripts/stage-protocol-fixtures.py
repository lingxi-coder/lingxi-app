#!/usr/bin/env python3
"""Stage immutable upstream test data into XCTest's device-readable bundle."""
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import sys

if sys.version_info < (3, 11):
    raise SystemExit("Harness fixture staging requires Python 3.11 or newer (tomllib)")
if shutil.which("cargo") is None:
    raise SystemExit("Harness fixture staging requires cargo on PATH")

host = Path(__file__).resolve().parents[3]
resolved = json.loads(subprocess.check_output([
    "python3", str(host / "lingxi-code/scripts/runtime_source.py"), "--json",
], text=True))
source = Path(resolved["root"]) / "crates/client-protocol/snapshots/compaction_hybrid_progress.json"
# Read before creating output: a missing upstream oracle fails the build.
data = source.read_bytes()
destination = Path(sys.argv[1])
destination.mkdir(parents=True, exist_ok=True)
shutil.copyfile(source, destination / source.name)
(destination / "source.json").write_text(json.dumps({
    "revision": resolved["revision"],
    "source": resolved["source"],
    "snapshot": "crates/client-protocol/snapshots/compaction_hybrid_progress.json",
    "sha256": hashlib.sha256(data).hexdigest(),
}, indent=2) + "\n")
