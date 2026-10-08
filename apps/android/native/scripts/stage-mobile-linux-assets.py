#!/usr/bin/env python3
"""Stage Android rootfs bytes anchored to the Cargo-locked SDK release evidence."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

ABIS = {"arm64-v8a": "arm64", "x86_64": "x86_64"}
EVIDENCE_FILES = (
    "rootfs-manifest.json", "rootfs-build.lock.json", "rootfs.spdx.json",
    "executable-allowlist.json", "apk-closure.json", "producer-inputs.json",
    "interpreter-alias-transformations.json",
)


def sha256(path):
    with path.open("rb") as file:
        return hashlib.file_digest(file, "sha256").hexdigest()


def checked_file(path):
    if path.is_symlink() or not path.is_file():
        raise ValueError(f"missing or unsafe release evidence: {path}")
    return path


def inspect_release(sdk, source, abi, version, pins):
    # The fixed SDK checkout is the trust anchor; self-consistent caller files
    # cannot substitute a different package inventory or a minirootfs digest.
    pinned = sdk / "docs/mobile-linux/releases" / version / abi
    supplied = source / abi
    for name in EVIDENCE_FILES:
        expected = checked_file(pinned / name)
        actual = checked_file(supplied / name)
        if sha256(actual) != sha256(expected):
            raise ValueError(f"release evidence differs from the locked SDK: {abi}/{name}")
    manifest = json.loads((pinned / "rootfs-manifest.json").read_text())
    if (manifest.get("rootfs_version"), manifest.get("runtime"), manifest.get("platform"), manifest.get("abi")) != (version, "android-proot", "android", ABIS[abi]):
        raise ValueError(f"release evidence target/version mismatch: {abi}")
    provenance = json.loads((pinned / "producer-inputs.json").read_text())
    producer_digest = provenance.get("source_toolchain_pins_sha256", "")
    if len(producer_digest) != 64 or any(c not in "0123456789abcdef" for c in producer_digest):
        raise ValueError(f"release evidence has no valid producer pin: {abi}")
    # The producer recorded its historical full-file digest before the x86
    # closure status was promoted. Compare active supply-chain identities,
    # not the status-only JSON bytes, to keep that provenance intact.
    closure = json.loads((pinned / "apk-closure.json").read_text())
    if closure.get("artifacts") != pins["apk_artifacts"][abi]["artifacts"]:
        raise ValueError(f"release APK closure differs from active SDK pins: {abi}")
    node = provenance.get("node_provenance", {})
    for key in ("version", "url", "sha256", "configure_args", "license", "build_packages", "builder_images"):
        if node.get(key) != pins["node_source"][key]:
            raise ValueError(f"release Node source differs from active SDK pins: {abi}")
    record = manifest["archive"]
    filename = record["filename"]
    if not isinstance(filename, str) or filename in ("", ".", "..") or Path(filename).name != filename or "\\" in filename:
        raise ValueError(f"unsafe release archive filename: {filename!r}")
    archive = checked_file(supplied / filename)
    if archive.stat().st_size != record["size_bytes"] or sha256(archive) != record["sha256"]:
        raise ValueError(f"release archive differs from the locked SDK artifact: {abi}")
    return manifest, archive


def stage(source, output, sdk, local_app, native):
    output = output.resolve()
    for protected in (source, sdk, local_app, native):
        protected = protected.resolve()
        if output.is_relative_to(protected) or protected.is_relative_to(output):
            raise ValueError(f"output overlaps immutable inputs: {protected}")
    pins_path = checked_file(sdk / "docs/toolchains/runtime-pins.json")
    pins = json.loads(pins_path.read_text())
    version = pins["alpine"]["version"]
    if not isinstance(version, str) or Path(version).name != version or version in ("", ".", ".."):
        raise ValueError("invalid active SDK rootfs version")
    digest = sha256(pins_path)
    releases = {}
    for abi in ABIS:
        manifest, archive = inspect_release(sdk, source, abi, version, pins)
        subprocess.run([
            sys.executable, str(sdk / "scripts/rootfs/verify-evidence.py"),
            "--evidence-dir", str(source / abi), "--archive", str(archive),
        ], check=True)
        releases[abi] = (manifest, archive)
    # No previous staged output is touched until every ABI and source check passes.
    output.parent.mkdir(parents=True, exist_ok=True)
    temporary = Path(tempfile.mkdtemp(prefix=f".{output.name}-", dir=output.parent))
    try:
        archives = {}
        for abi, (manifest, archive) in releases.items():
            destination = temporary / "rootfs" / abi
            destination.mkdir(parents=True)
            for name in EVIDENCE_FILES:
                shutil.copy2(source / abi / name, destination / name)
            # Android's asset merger expands *.gz and drops that suffix. Keep
            # the exact gzip bytes under an opaque APK asset name instead.
            shutil.copy2(archive, destination / (archive.name + ".bin"))
            archives[abi] = dict(manifest["archive"])
        compatibility_pins = {
            "schema_version": 2,
            "source_toolchain_pins_sha256": digest,
            "rootfs": {"version": version, "release_archives": archives},
        }
        (temporary / "mobile-linux-pins.json").write_text(json.dumps(compatibility_pins, indent=2) + "\n")
        shutil.copy2(pins_path, temporary / "runtime-pins.json")
        shutil.copy2(local_app / "docs/runtime/local-app-runtime-pins.json", temporary / "local-app-runtime-pins.json")
        licenses = temporary / "licenses"
        licenses.mkdir()
        shutil.copy2(sdk / "docs/mobile-linux/LICENSES/NOTICE.md", licenses / "NOTICE.md")
        for name in ("GPL-3.0-only.txt", "GPL-2.0-or-later.txt"):
            shutil.copy2(checked_file(native / "licenses" / name), licenses / name)
        shutil.copy2(native / "native-manifest.json", temporary / "native-manifest.json")
        shutil.copy2(native.parent / "sdk-source.json", temporary / "mobile-linux-sdk-source.json")
        if output.exists():
            shutil.rmtree(output)
        temporary.rename(output)
    finally:
        if temporary.exists():
            shutil.rmtree(temporary)
    print(f"staged active SDK {version} release assets: {output}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("input", "output", "sdk-root", "local-app-root", "native-root"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    try:
        stage(args.input, args.output, args.sdk_root, args.local_app_root, args.native_root)
    except (OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"stage-mobile-linux-assets: {error}\n")


if __name__ == "__main__":
    main()
