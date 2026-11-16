#!/usr/bin/env python3
"""Verify locked SDK evidence across equivalent writable-directory policies.

The published image lists the former workspace beneath the already writable
guest home. New SDKs moved that workspace without changing the image's
immutable payload or its effective writable scope. Keep the release evidence
unchanged and let the SDK verify every original archive and inventory check.
"""
import argparse
import importlib.util
import json
from pathlib import Path, PurePosixPath
import sys


def writable_scope(paths):
    if not isinstance(paths, (list, set)) or not paths:
        raise ValueError("invalid rootfs writable paths")
    checked = set()
    for value in paths:
        if not isinstance(value, str):
            raise ValueError("invalid rootfs writable path")
        path = PurePosixPath(value)
        if (not path.is_absolute() or path.as_posix() != value
                or ".." in path.parts or value in checked):
            raise ValueError("noncanonical or duplicate rootfs writable path")
        checked.add(value)
    # A writable ancestor already covers every descendant. Removing redundant
    # descendants permits neither extra writable files nor missing roots.
    return {value for value in checked
            if not any(parent.as_posix() in checked for parent in PurePosixPath(value).parents)}


def verify(sdk, evidence, archive):
    rootfs_scripts = sdk / "scripts/rootfs"
    sys.path.insert(0, str(rootfs_scripts))
    spec = importlib.util.spec_from_file_location("sdk_rootfs_evidence", rootfs_scripts / "verify-evidence.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    manifest = json.loads((evidence / "rootfs-manifest.json").read_text())
    writable = manifest.get("writable_paths")
    if writable_scope(writable) != writable_scope(module.tool.WRITABLE_ROOTS):
        raise ValueError("rootfs evidence changes the selected runtime's writable scope")
    # Only the equivalent redundant paths differ. All membership checks made
    # by the SDK still cover exactly the same files and directories.
    module.tool.WRITABLE_ROOTS = set(writable)
    module.verify(evidence, archive)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sdk-root", type=Path, required=True)
    parser.add_argument("--evidence-dir", type=Path, required=True)
    parser.add_argument("--archive", type=Path, required=True)
    args = parser.parse_args()
    try:
        verify(args.sdk_root, args.evidence_dir, args.archive)
    except (OSError, ValueError, KeyError) as error:
        parser.exit(1, f"verify-sdk-rootfs: {error}\n")
