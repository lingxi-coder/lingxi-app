#!/usr/bin/env python3
"""Compatibility launcher for the Cargo-locked upstream runtime tool."""
import pathlib
import subprocess
import sys
resolver = pathlib.Path(__file__).resolve().parent.parent / "runtime_source.py"
root = subprocess.check_output([sys.executable, str(resolver), "--root"], text=True).strip()
args = sys.argv[1:]
if "--repo-root" in args:
    args[args.index("--repo-root") + 1] = root
raise SystemExit(subprocess.call([sys.executable, str(pathlib.Path(root) / "scripts/mobile-linux/verify-local-app-supply-chain.py"), *args]))
