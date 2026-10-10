#!/usr/bin/env python3
"""Resolve the mobile Linux SDK from its exact locked Cargo identity."""

import argparse
import json
from pathlib import Path
import subprocess
import sys
import tomllib

from runtime_source import inspect_metadata


WORKSPACE = Path(__file__).resolve().parents[2]
PACKAGE_LIST = (Path(__file__).resolve().parent / "mobile-linux-packages.json")


def inspect_sdk_metadata(metadata, dependency, package_list, *, development_root=None):
    return inspect_metadata(
        metadata, dependency, package_list,
        anchor="mobile-linux-api", layout="crates/mobile-linux-api/Cargo.toml", development_root=development_root,
    )


def resolve_sdk(workspace=WORKSPACE):
    workspace = Path(workspace).resolve()
    manifest = tomllib.loads((workspace / "Cargo.toml").read_text(encoding="utf-8"))
    dependency = manifest["workspace"]["dependencies"]["mobile-linux-api"]
    result = subprocess.run(
        ["cargo", "metadata", "--locked", "--format-version=1", "--all-features",
         "--manifest-path", str(workspace / "Cargo.toml")],
        cwd=workspace, check=True, capture_output=True, text=True, encoding="utf-8",
    )
    package_list = json.loads(PACKAGE_LIST.read_text(encoding="utf-8"))
    patch = manifest.get("patch", {}).get(package_list["repository"], {}).get("mobile-linux-api", {})
    development_root = None
    if "path" in patch:
        patched_manifest = (workspace / patch["path"] / "Cargo.toml").resolve()
        development_root = patched_manifest.parents[2]
        if patched_manifest.relative_to(development_root).as_posix() != "crates/mobile-linux-api/Cargo.toml":
            raise ValueError("unexpected declared mobile SDK development patch layout")
    resolved = inspect_sdk_metadata(json.loads(result.stdout), dependency, package_list,
                                    development_root=development_root)
    if not (Path(resolved["root"]) / "Cargo.toml").is_file():
        raise ValueError("resolved mobile Linux SDK has no workspace manifest")
    return resolved


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    output = parser.add_mutually_exclusive_group()
    output.add_argument("--root", action="store_true")
    output.add_argument("--json", action="store_true")
    args = parser.parse_args()
    try:
        result = resolve_sdk()
    except (KeyError, ValueError, OSError, subprocess.CalledProcessError) as error:
        detail = error.stderr.strip() if isinstance(error, subprocess.CalledProcessError) else str(error)
        print(f"mobile-linux-source: {detail}", file=sys.stderr)
        return 1
    print(json.dumps(result, indent=2) if args.json else result["root"])
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
