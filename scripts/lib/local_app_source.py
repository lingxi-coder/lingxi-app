#!/usr/bin/env python3
"""Resolve the Local App checkout from its exact locked Cargo identity."""

import argparse
import json
from pathlib import Path
import subprocess
import sys
import tomllib

from runtime_source import inspect_metadata


WORKSPACE = Path(__file__).resolve().parents[2]
PACKAGE_LIST = (Path(__file__).resolve().parent / "local-app-packages.json")


def inspect_local_app_metadata(metadata, dependency, package_list):
    return inspect_metadata(
        metadata, dependency, package_list,
        anchor="local-app-builder-contracts", layout="crates/local-app-builder-contracts/Cargo.toml",
    )


def resolve_local_app(workspace=WORKSPACE):
    workspace = Path(workspace).resolve()
    manifest = tomllib.loads((workspace / "Cargo.toml").read_text(encoding="utf-8"))
    dependency = manifest["workspace"]["dependencies"]["local-app-builder-contracts"]
    result = subprocess.run(
        ["cargo", "metadata", "--locked", "--format-version=1", "--all-features",
         "--manifest-path", str(workspace / "Cargo.toml")],
        cwd=workspace, check=True, capture_output=True, text=True, encoding="utf-8",
    )
    resolved = inspect_local_app_metadata(
        json.loads(result.stdout), dependency, json.loads(PACKAGE_LIST.read_text(encoding="utf-8")),
    )
    if not (Path(resolved["root"]) / "Cargo.toml").is_file():
        raise ValueError("resolved Local App checkout has no workspace manifest")
    return resolved


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    output = parser.add_mutually_exclusive_group()
    output.add_argument("--root", action="store_true")
    output.add_argument("--json", action="store_true")
    args = parser.parse_args()
    try:
        result = resolve_local_app()
    except (KeyError, ValueError, OSError, subprocess.CalledProcessError) as error:
        detail = error.stderr.strip() if isinstance(error, subprocess.CalledProcessError) else str(error)
        print(f"local-app-source: {detail}", file=sys.stderr)
        return 1
    print(json.dumps(result, indent=2) if args.json else result["root"])
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
