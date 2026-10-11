#!/usr/bin/env python3
"""Resolve the pinned Harness checkout through Cargo, without cache guessing."""

import argparse
import json
from pathlib import Path
import re
import subprocess
import sys
import tomllib


WORKSPACE = Path(__file__).resolve().parents[2]
PACKAGE_LIST = (Path(__file__).resolve().parent / "harness-runtime-packages.json")


def inspect_metadata(metadata, dependency, package_list, *, anchor="harness-runtime", layout="crates/runtime/Cargo.toml", development_root=None):
    """Validate one pinned checkout or one explicitly declared development root."""
    revision = dependency.get("rev", "")
    repository = package_list["repository"]
    if dependency.get("git") != repository or not re.fullmatch(r"[0-9a-f]{40}", revision):
        raise ValueError(f"{anchor} must use its canonical Git URL and a full commit SHA")
    expected_source = f"git+{repository}?rev={revision}#{revision}"
    development_root = Path(development_root).resolve() if development_root else None
    def valid_identity(package):
        if development_root is None:
            return package.get("source") == expected_source
        if package.get("source") is not None:
            return False
        try:
            Path(package["manifest_path"]).resolve().relative_to(development_root)
        except (ValueError, KeyError):
            return False
        return True
    owned = set(package_list["packages"] + package_list["vendored_packages"])
    packages = {}
    for package in metadata["packages"]:
        name = package["name"]
        if name not in owned:
            continue
        if not valid_identity(package):
            raise ValueError(f"{name} has a different source: {package.get('source')!r}")
        if name in packages:
            raise ValueError(f"multiple Cargo identities for migrated package {name}")
        packages[name] = package
    if anchor not in packages:
        raise ValueError(f"locked Cargo metadata does not contain {anchor}")
    manifest = Path(packages[anchor]["manifest_path"]).resolve()
    root = manifest.parents[len(Path(layout).parts) - 1]
    if manifest.relative_to(root).as_posix() != layout:
        raise ValueError(f"unexpected {anchor} source layout")
    if development_root is not None and root != development_root:
        raise ValueError("unexpected development source layout")
    for name, package in packages.items():
        try:
            Path(package["manifest_path"]).resolve().relative_to(root)
        except ValueError as error:
            raise ValueError(f"{name} is outside the locked {anchor} checkout") from error
    # Check declared edges too, including dependencies inactive on this target.
    for package in metadata["packages"]:
        for edge in package.get("dependencies", []):
            if edge["name"] not in owned:
                continue
            if edge.get("path"):
                if not valid_identity(package):
                    raise ValueError(f"{package['name']} retains a local dependency on {edge['name']}")
                try:
                    Path(edge["path"]).resolve().relative_to(root)
                except ValueError as error:
                    raise ValueError(f"{package['name']} retains a local dependency on {edge['name']}") from error
            elif edge.get("source") not in (expected_source, expected_source.rsplit("#", 1)[0]):
                # A declared development patch unifies the canonical repository
                # even when consumers retain different immutable Git pins.
                # Every resolved identity above must still be local to its one root.
                canonical_edge = rf"git\+{re.escape(repository)}\?rev=([0-9a-f]{{40}})(?:#\1)?"
                if development_root is None or not re.fullmatch(canonical_edge, edge.get("source") or ""):
                    raise ValueError(f"{package['name']} declares another source for {edge['name']}")
    return {
        "root": str(root),
        "manifest_path": str(manifest),
        "revision": revision,
        "source": f"path+{root}" if development_root else expected_source,
        "development": development_root is not None,
        "packages": {name: package["manifest_path"] for name, package in sorted(packages.items())},
    }


def resolve_runtime(workspace=WORKSPACE):
    workspace = Path(workspace)
    manifest = tomllib.loads((workspace / "Cargo.toml").read_text(encoding="utf-8"))
    dependency = manifest["workspace"]["dependencies"]["harness-runtime"]
    package_list = json.loads(PACKAGE_LIST.read_text(encoding="utf-8"))
    command = [
        "cargo", "metadata", "--locked", "--format-version=1", "--all-features",
        "--manifest-path", str(workspace / "Cargo.toml"),
    ]
    result = subprocess.run(command, cwd=workspace, check=True, capture_output=True, text=True, encoding="utf-8")
    patch = manifest.get("patch", {}).get(package_list["repository"], {}).get("harness-runtime", {})
    development_root = None
    if "path" in patch:
        patched_manifest = (workspace / patch["path"] / "Cargo.toml").resolve()
        development_root = patched_manifest.parents[2]
        if patched_manifest.relative_to(development_root).as_posix() != "crates/runtime/Cargo.toml":
            raise ValueError("unexpected declared Harness development patch layout")
    resolved = inspect_metadata(json.loads(result.stdout), dependency, package_list, development_root=development_root)
    if not (Path(resolved["root"]) / "Cargo.toml").is_file():
        raise ValueError("resolved Harness checkout has no workspace manifest")
    return resolved


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    output = parser.add_mutually_exclusive_group()
    output.add_argument("--root", action="store_true", help="print the upstream repository root")
    output.add_argument("--json", action="store_true", help="print provenance and package paths")
    args = parser.parse_args()
    try:
        resolved = resolve_runtime()
    except (KeyError, ValueError, OSError, subprocess.CalledProcessError) as error:
        detail = error.stderr.strip() if isinstance(error, subprocess.CalledProcessError) else str(error)
        print(f"runtime-source: {detail}", file=sys.stderr)
        return 1
    print(json.dumps(resolved, indent=2) if args.json else resolved["root"])
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
