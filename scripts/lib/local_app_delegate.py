#!/usr/bin/env python3
"""Run a Local App runtime tool from the Local App checkout this product pins.

The tools live in the Local App repository (`scripts/runtime`). They take that checkout as
`--repo-root` and the SDK as `--sdk-root`; both come from this product's Cargo pins, so a bare
invocation attests exactly what the product ships. A `--repo-root` the caller names is replaced
for the same reason; an explicit `--sdk-root` is kept (development inputs only).
"""
from pathlib import Path
import subprocess
import sys

sys.dont_write_bytecode = True
LIB = Path(__file__).resolve().parent


def resolved_root(resolver):
    return subprocess.check_output([sys.executable, str(LIB / resolver), "--root"], text=True).strip()


def run(tool, argv):
    local_app = resolved_root("local_app_source.py")
    argv = list(argv)
    if "--repo-root" in argv:
        argv[argv.index("--repo-root") + 1] = local_app
    else:
        argv = ["--repo-root", local_app, *argv]
    if "--sdk-root" not in argv:
        argv = ["--sdk-root", resolved_root("mobile_linux_source.py"), *argv]
    return subprocess.call([sys.executable, str(Path(local_app) / "scripts/runtime" / tool), *argv])
