#!/usr/bin/env python3
"""Stage and verify SDK support alongside LingXi's single Rust FFI library."""
import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import struct

ABIS = {"arm64-v8a": 183, "x86_64": 62}
SUPPORT = {
    "libproot.so", "libproot-loader.so", "libmobile_linux_policy_launcher.so",
}
REMOVED_SUPPORT = {"libpty_bridge.so", "libmksh.so", "libtoybox.so"}


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def inventory(source):
    manifest = json.loads((source / "native-manifest.json").read_text())
    if manifest.get("kind") != "native-support-only" or manifest.get("contains_rust_core") is not False:
        raise ValueError("LingXi requires native-support-only artifacts without a second Rust core")
    entries = {}
    expected = {(abi, name) for abi in ABIS for name in SUPPORT}
    for entry in manifest.get("artifacts", []):
        path = PurePosixPath(entry["path"])
        if len(path.parts) != 3 or path.parts[0] != "jniLibs" or path.is_absolute() or ".." in path.parts:
            raise ValueError(f"unsafe SDK artifact path: {path}")
        key = path.parts[1:]
        if key not in expected or key in entries or entry.get("abi") != key[0]:
            raise ValueError(f"unexpected or duplicate SDK artifact: {path}")
        artifact = source / path
        if artifact.is_symlink() or not artifact.is_file() or sha256(artifact) != entry["sha256"]:
            raise ValueError(f"SDK native artifact digest mismatch: {path}")
        entries[key] = entry
    if set(entries) != expected:
        raise ValueError("SDK support must contain all three PRoot artifacts for both Android ABIs")
    return entries


def verify_elf(path, machine):
    with path.open("rb") as file:
        header = file.read(20)
    if len(header) != 20 or header[:6] != b"\x7fELF\x02\x01" or struct.unpack_from("<H", header, 18)[0] != machine:
        raise ValueError(f"wrong ELF64 architecture: {path}")


def verify(source, destination):
    entries = inventory(source)
    for abi, machine in ABIS.items():
        for name in SUPPORT | {"libandroid_aar.so"}:
            artifact = (source / "jniLibs" if name in SUPPORT else destination) / abi / name
            if artifact.is_symlink():
                raise ValueError(f"symlinked native artifact: {artifact}")
            verify_elf(artifact, machine)
            if name in SUPPORT and sha256(artifact) != entries[(abi, name)]["sha256"]:
                raise ValueError(f"SDK artifact differs from the verified build: {artifact}")
        if any((destination / abi / name).exists() for name in SUPPORT | REMOVED_SUPPORT):
            raise ValueError("host jniLibs must not contain SDK or removed shell helpers")
        for name in ("libmobile_linux_runtime.so", "libmobile_linux_ffi.so"):
            if (destination / abi / name).exists():
                raise ValueError("full SDK FFI must not be packaged alongside libandroid_aar.so")


def verify_maven(root, revision):
    manifest = json.loads((root / "sdk-artifacts.json").read_text())
    if manifest.get("source_revision") != revision or manifest.get("source_dirty") is not False:
        raise ValueError("SDK Maven artifacts must come from the clean Cargo-locked source revision")
    if manifest.get("native_support_only") is not True:
        raise ValueError("LingXi requires installer/native-support Maven artifacts without full SDK FFI")
    version = manifest["version"]
    required = {
        f"io/github/lingxi-coder/{name}/{version}/{name}-{version}.{extension}"
        for name in ("mobile-linux-installer", "mobile-linux-native-support")
        for extension in ("aar", "pom")
    }
    if not required <= manifest["files"].keys():
        raise ValueError("SDK Maven manifest must include installer/support AARs and transitive dependency metadata")
    for relative, expected in manifest["files"].items():
        path = PurePosixPath(relative)
        if path.is_absolute() or ".." in path.parts:
            raise ValueError("unsafe Maven artifact path")
        artifact = root / path
        if artifact.is_symlink() or sha256(artifact) != expected:
            raise ValueError(f"SDK Maven artifact digest mismatch: {relative}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("verify",))
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--jni-root", type=Path, required=True)
    parser.add_argument("--maven-dir", type=Path)
    parser.add_argument("--expected-revision")
    args = parser.parse_args()
    try:
        verify(args.source, args.jni_root)
        if args.maven_dir:
            verify_maven(args.maven_dir, args.expected_revision)
    except (OSError, ValueError, KeyError) as error:
        parser.exit(1, f"mobile-linux-native: {error}\n")
    print(f"{args.action}: native support and LingXi FFI at {args.jni_root}")


if __name__ == "__main__":
    main()
