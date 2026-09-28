#!/usr/bin/env python3
"""Invoke the supported tool from its Cargo-locked upstream source."""
import pathlib
import subprocess
import sys
resolver = pathlib.Path(__file__).resolve().parents[1] / "lib/runtime_source.py"
root = subprocess.check_output([sys.executable, str(resolver), "--root"], text=True).strip()
args = sys.argv[1:]
if "--repo-root" in args:
    args[args.index("--repo-root") + 1] = root
raise SystemExit(subprocess.call([sys.executable, str(pathlib.Path(root) / "scripts/local-apps/stage-local-app-runtime.py"), *args]))
