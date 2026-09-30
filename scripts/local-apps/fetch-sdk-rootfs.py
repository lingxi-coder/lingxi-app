#!/usr/bin/env python3
"""Fetch published rootfs bytes verified against the Cargo-locked SDK evidence."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import sys
import urllib.request

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))
from mobile_linux_source import resolve_sdk


def fetch(output):
    resolved = resolve_sdk()
    sdk = Path(resolved["root"]).resolve()
    pins = json.loads((sdk / "docs/toolchains/runtime-pins.json").read_text(encoding="utf-8"))
    version = pins["alpine"]["version"]
    config = Path(__file__).resolve().parents[1] / "lib/mobile-linux-rootfs-release.json"
    tag = json.loads(config.read_text(encoding="utf-8"))["tag"]
    repository = json.loads((config.parent / "mobile-linux-packages.json").read_text(encoding="utf-8"))["repository"].removesuffix(".git")
    output = output.resolve()
    if output.is_relative_to(sdk) or sdk.is_relative_to(output):
        raise ValueError("output overlaps immutable SDK source")
    for abi, asset_arch in (("arm64-v8a", "arm64"), ("x86_64", "x86_64")):
        evidence = sdk / "docs/mobile-linux/releases" / version / abi
        manifest = json.loads((evidence / "rootfs-manifest.json").read_text(encoding="utf-8"))
        record = manifest["archive"]
        filename = record["filename"]
        if Path(filename).name != filename or filename in ("", ".", "..") or "\\" in filename:
            raise ValueError("unsafe SDK archive filename")
        destination = output / abi
        destination.mkdir(parents=True, exist_ok=True)
        archive = destination / filename
        def valid(path):
            if path.is_symlink() or not path.is_file() or path.stat().st_size != record["size_bytes"]:
                return False
            with path.open("rb") as file:
                return hashlib.file_digest(file, "sha256").hexdigest() == record["sha256"]
        if not valid(archive):
            url = f"{repository}/releases/download/{tag}/mobile-linux-rootfs-android-{asset_arch}-v{version}.tar.gz"
            temporary = archive.with_suffix(".download")
            try:
                with urllib.request.urlopen(url, timeout=120) as response, temporary.open("wb") as file:
                    shutil.copyfileobj(response, file)
                if not valid(temporary):
                    raise ValueError(f"published rootfs differs from locked SDK digest: {abi}")
                temporary.replace(archive)
            finally:
                temporary.unlink(missing_ok=True)
        for source in evidence.glob("*.json"):
            shutil.copy2(source, destination / source.name)
        subprocess.run([sys.executable, str(sdk / "scripts/rootfs/verify-evidence.py"),
                        "--evidence-dir", str(destination), "--archive", str(archive)], check=True)
    print(f"Verified locked SDK release rootfs: {output}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    try:
        fetch(args.output)
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"fetch-sdk-rootfs: {error}\n")
