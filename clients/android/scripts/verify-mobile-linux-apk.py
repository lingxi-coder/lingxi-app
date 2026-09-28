#!/usr/bin/env python3
"""Verify final APK native bytes and executable-library extraction packaging."""
import argparse
import hashlib
import importlib.util
from pathlib import Path
import zipfile

spec = importlib.util.spec_from_file_location("native_support", Path(__file__).with_name("mobile-linux-native.py"))
native = importlib.util.module_from_spec(spec)
spec.loader.exec_module(native)


def verify_apk(apk, source, jni_root):
    native.verify(source, jni_root)
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
            # PRoot and legacy shells must be extracted into nativeLibraryDir.
            # useLegacyPackaging=true stores compressed native libraries.
            if entry.compress_type != zipfile.ZIP_DEFLATED:
                raise ValueError(f"APK native libraries require useLegacyPackaging=true: {name}")
        if any(name.endswith(("/libmobile_linux_runtime.so", "/libmobile_linux_ffi.so")) for name in names):
            raise ValueError("APK includes a second Rust core through the full SDK FFI")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--apk-dir", type=Path, required=True)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--jni-root", type=Path, required=True)
    args = parser.parse_args()
    try:
        apks = sorted(args.apk_dir.glob("*.apk"))
        if not apks:
            raise ValueError(f"no final APKs under {args.apk_dir}")
        for apk in apks:
            verify_apk(apk, args.source, args.jni_root)
            print(f"verified final MobileLinux APK: {apk}")
    except (OSError, ValueError, KeyError, zipfile.BadZipFile) as error:
        parser.exit(1, f"mobile-linux-apk: {error}\n")


if __name__ == "__main__":
    main()
