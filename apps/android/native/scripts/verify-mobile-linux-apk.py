#!/usr/bin/env python3
"""Verify final APK native and rootfs bytes against validated staging."""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import zipfile

spec = importlib.util.spec_from_file_location("native_support", Path(__file__).with_name("mobile-linux-native.py"))
native = importlib.util.module_from_spec(spec)
spec.loader.exec_module(native)


ROOTFS_EVIDENCE = {
    "rootfs-manifest.json", "rootfs-build.lock.json", "rootfs.spdx.json",
    "executable-allowlist.json", "apk-closure.json", "producer-inputs.json",
    "interpreter-alias-transformations.json",
}


def staged_assets(assets_root, source):
    if assets_root.is_symlink() or not assets_root.is_dir():
        raise ValueError("staged Mobile Linux rootfs assets are missing")
    pins_path = assets_root / "mobile-linux-pins.json"
    if pins_path.is_symlink() or not pins_path.is_file():
        raise ValueError("staged Mobile Linux rootfs pins are missing")
    pins = json.loads(pins_path.read_text())
    releases = pins.get("rootfs", {}).get("release_archives", {})
    if set(releases) != set(native.ABIS):
        raise ValueError("staged rootfs must cover both Android ABIs")
    required = {
        "mobile-linux-pins.json", "runtime-pins.json",
        "native-manifest.json", "mobile-linux-sdk-source.json", "licenses/NOTICE.md",
        "licenses/GPL-3.0-only.txt", "licenses/GPL-2.0-or-later.txt",
    }
    for abi, release in releases.items():
        filename = release.get("filename")
        if not isinstance(filename, str) or filename in ("", ".", "..") or Path(filename).name != filename:
            raise ValueError(f"unsafe rootfs archive filename for {abi}")
        asset_name = filename + ".bin"
        required.update(f"rootfs/{abi}/{name}" for name in ROOTFS_EVIDENCE | {asset_name})
        archive = assets_root / "rootfs" / abi / asset_name
        if archive.is_symlink() or not archive.is_file():
            raise ValueError(f"staged rootfs archive is missing for {abi}")
        if archive.stat().st_size != release.get("size_bytes") or native.sha256(archive) != release.get("sha256"):
            raise ValueError(f"staged rootfs archive differs from pinned digest for {abi}")
    files = {}
    for path in assets_root.rglob("*"):
        if path.is_symlink():
            raise ValueError(f"symlinked Mobile Linux asset: {path}")
        if path.is_file():
            files[path.relative_to(assets_root).as_posix()] = path
    if not required <= files.keys():
        raise ValueError(f"staged Mobile Linux assets are missing: {sorted(required - files.keys())}")
    for asset, original in (
        ("native-manifest.json", source / "native-manifest.json"),
        ("mobile-linux-sdk-source.json", source.parent / "sdk-source.json"),
    ):
        if files[asset].read_bytes() != original.read_bytes():
            raise ValueError(f"staged Mobile Linux source differs from verified native source: {asset}")
    return files


def archive_sha256(archive, name):
    digest = hashlib.sha256()
    with archive.open(name) as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def verify_apk(apk, source, jni_root, assets_root):
    native.verify(source, jni_root)
    assets = staged_assets(assets_root, source)
    expected = {
        f"lib/{abi}/{name}": native.sha256((source / "jniLibs" if name in native.SUPPORT else jni_root) / abi / name)
        for abi in native.ABIS for name in native.SUPPORT | {"libandroid_aar.so"}
    }
    with zipfile.ZipFile(apk) as archive:
        names = archive.namelist()
        if len(names) != len(set(names)):
            raise ValueError("duplicate entries in APK")
        for name, digest in expected.items():
            entry = archive.getinfo(name)
            if hashlib.sha256(archive.read(entry)).hexdigest() != digest:
                raise ValueError(f"APK native payload differs from verified staging: {name}")
            # PRoot helpers must be extracted into nativeLibraryDir.
            # useLegacyPackaging=true stores compressed native libraries.
            if entry.compress_type != zipfile.ZIP_DEFLATED:
                raise ValueError(f"APK native libraries require useLegacyPackaging=true: {name}")
        if any(name.endswith(("/libmobile_linux_runtime.so", "/libmobile_linux_ffi.so")) for name in names):
            raise ValueError("APK includes a second Rust core through the full SDK FFI")
        if any(name.endswith(tuple("/" + helper for helper in native.REMOVED_SUPPORT)) for name in names):
            raise ValueError("APK still contains removed Android host-shell helpers")
        asset_prefix = "assets/mobile-linux/"
        packaged_assets = {name.removeprefix(asset_prefix) for name in names if name.startswith(asset_prefix)}
        if packaged_assets != assets.keys():
            raise ValueError("APK Mobile Linux assets differ from verified staging")
        for relative, path in assets.items():
            if archive_sha256(archive, asset_prefix + relative) != native.sha256(path):
                raise ValueError(f"APK Mobile Linux asset differs from verified staging: {relative}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--apk-dir", type=Path, required=True)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--jni-root", type=Path, required=True)
    parser.add_argument("--assets-source", type=Path, required=True)
    args = parser.parse_args()
    try:
        apks = sorted(args.apk_dir.glob("*.apk"))
        if not apks:
            raise ValueError(f"no final APKs under {args.apk_dir}")
        for apk in apks:
            verify_apk(apk, args.source, args.jni_root, args.assets_source)
            print(f"verified final MobileLinux APK: {apk}")
    except (OSError, ValueError, KeyError, zipfile.BadZipFile) as error:
        parser.exit(1, f"mobile-linux-apk: {error}\n")


if __name__ == "__main__":
    main()
