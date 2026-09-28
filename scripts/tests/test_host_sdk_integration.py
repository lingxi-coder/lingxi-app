#!/usr/bin/env python3
"""Negative regressions for host-only native linking and final APK validation."""
import importlib.util
import json
from pathlib import Path
import shutil
import struct
import subprocess
import sys
import tempfile
import unittest
import zipfile

HOST = Path(__file__).resolve().parents[2]


def module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


native = module("native", HOST / "clients/android/scripts/mobile-linux-native.py")
apk = module("apk", HOST / "clients/android/scripts/verify-mobile-linux-apk.py")
policy = module("policy", (Path(__file__).resolve().parent / "../local-apps/verify-local-app-host.py"))
assets = module("assets", HOST / "clients/android/scripts/stage-mobile-linux-assets.py")


class NativeIntegrationTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.source = self.root / "sdk"
        self.jni = self.root / "jni"
        entries = []
        for abi, machine in native.ABIS.items():
            for name in native.SUPPORT | {"libandroid_aar.so"}:
                directory = self.jni / abi if name == "libandroid_aar.so" else self.source / "jniLibs" / abi
                directory.mkdir(parents=True, exist_ok=True)
                path = directory / name
                header = bytearray(20)
                header[:6] = b"\x7fELF\x02\x01"
                struct.pack_into("<H", header, 18, machine)
                path.write_bytes(header + name.encode())
                if name in native.SUPPORT:
                    entries.append({"path": path.relative_to(self.source).as_posix(), "abi": abi, "sha256": native.sha256(path)})
        self.manifest = {"kind": "native-support-only", "contains_rust_core": False, "artifacts": entries}
        self.save_manifest()

    def save_manifest(self):
        (self.source / "native-manifest.json").write_text(json.dumps(self.manifest))

    def make_apk(self, compression=zipfile.ZIP_DEFLATED):
        target = self.root / "app.apk"
        with zipfile.ZipFile(target, "w", compression=compression) as archive:
            for abi in native.ABIS:
                for name in native.SUPPORT | {"libandroid_aar.so"}:
                    root = self.source / "jniLibs" if name in native.SUPPORT else self.jni
                    archive.write(root / abi / name, f"lib/{abi}/{name}")
        return target

    def test_native_only_and_final_apk(self):
        native.verify(self.source, self.jni)
        apk.verify_apk(self.make_apk(), self.source, self.jni)

    def test_full_sdk_core_is_rejected(self):
        self.manifest["contains_rust_core"] = True
        self.save_manifest()
        with self.assertRaisesRegex(ValueError, "second Rust core"):
            native.verify(self.source, self.jni)

    def test_duplicate_host_native_helper_is_rejected(self):
        path = self.source / "jniLibs/arm64-v8a/libproot.so"
        shutil.copy2(path, self.jni / "arm64-v8a/libproot.so")
        with self.assertRaisesRegex(ValueError, "duplicated"):
            native.verify(self.source, self.jni)

    def test_tampered_sdk_artifact_is_rejected(self):
        (self.source / "jniLibs/arm64-v8a/libproot.so").write_bytes(b"modified")
        with self.assertRaisesRegex(ValueError, "digest mismatch"):
            native.verify(self.source, self.jni)

    def test_missing_abi_is_rejected(self):
        self.manifest["artifacts"] = [entry for entry in self.manifest["artifacts"] if entry["abi"] == "arm64-v8a"]
        self.save_manifest()
        with self.assertRaisesRegex(ValueError, "both Android ABIs"):
            native.verify(self.source, self.jni)

    def test_wrong_elf_machine_is_rejected(self):
        (self.jni / "arm64-v8a/libandroid_aar.so").write_bytes((self.jni / "x86_64/libandroid_aar.so").read_bytes())
        with self.assertRaisesRegex(ValueError, "architecture"):
            native.verify(self.source, self.jni)

    def test_uncompressed_helpers_are_rejected(self):
        with self.assertRaisesRegex(ValueError, "useLegacyPackaging"):
            apk.verify_apk(self.make_apk(zipfile.ZIP_STORED), self.source, self.jni)

    def test_apk_changed_after_staging_is_rejected(self):
        archive = self.make_apk()
        (self.jni / "arm64-v8a/libandroid_aar.so").write_bytes((self.jni / "arm64-v8a/libandroid_aar.so").read_bytes() + b"different")
        with self.assertRaisesRegex(ValueError, "differs from verified staging"):
            apk.verify_apk(archive, self.source, self.jni)

    def test_dirty_or_mismatched_maven_source_is_rejected(self):
        root = self.root / "maven"
        root.mkdir()
        for revision, dirty in (("a" * 40, True), ("b" * 40, False)):
            (root / "sdk-artifacts.json").write_text(json.dumps({"source_revision": revision, "source_dirty": dirty}))
            with self.assertRaisesRegex(ValueError, "clean Cargo-locked"):
                native.verify_maven(root, "a" * 40)

    def test_current_host_policy(self):
        policy.validate_host_policy(HOST)


class ActiveRootfsEvidenceTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.sdk, self.input = self.root / "sdk", self.root / "input"
        self.pinned = self.sdk / "docs/mobile-linux/releases/3.24.2/arm64-v8a"
        self.supplied = self.input / "arm64-v8a"
        self.pinned.mkdir(parents=True)
        self.supplied.mkdir(parents=True)
        self.archive = self.supplied / "rootfs.tar.gz"
        self.archive.write_bytes(b"actual release payload")
        self.manifest = {
            "rootfs_version": "3.24.2", "runtime": "android-proot", "platform": "android", "abi": "arm64",
            "archive": {"filename": self.archive.name, "sha256": assets.sha256(self.archive), "size_bytes": self.archive.stat().st_size},
        }
        for name in assets.EVIDENCE_FILES:
            value = self.manifest if name == "rootfs-manifest.json" else {"source_toolchain_pins_sha256": "current-pin"} if name == "producer-inputs.json" else {}
            (self.pinned / name).write_text(json.dumps(value))
            shutil.copy2(self.pinned / name, self.supplied / name)

    def inspect(self):
        return assets.inspect_release(self.sdk, self.input, "arm64-v8a", "3.24.2", "current-pin")

    def test_pinned_release_identity(self):
        manifest, archive = self.inspect()
        self.assertEqual(manifest["rootfs_version"], "3.24.2")
        self.assertEqual(archive, self.archive)

    def test_caller_cannot_substitute_min_rootfs_identity(self):
        self.manifest["archive"]["sha256"] = "a" * 64
        (self.supplied / "rootfs-manifest.json").write_text(json.dumps(self.manifest))
        with self.assertRaisesRegex(ValueError, "differs from the locked SDK"):
            self.inspect()

    def test_archive_tampering_rejected(self):
        self.archive.write_bytes(b"different archive")
        with self.assertRaisesRegex(ValueError, "archive differs"):
            self.inspect()

    def test_missing_supported_abi_is_not_filled_with_legacy_pins(self):
        with self.assertRaisesRegex(ValueError, "missing or unsafe release evidence"):
            assets.inspect_release(self.sdk, self.input, "x86_64", "3.24.2", "current-pin")

    def test_evidence_requires_current_toolchain_pin(self):
        with self.assertRaisesRegex(ValueError, "another SDK toolchain pin"):
            assets.inspect_release(self.sdk, self.input, "arm64-v8a", "3.24.2", "different-pin")

    def test_ios_base_reader_uses_active_source_pin(self):
        script = (HOST / "clients/ios/scripts/build-linux-runtime.sh").read_text()
        reader = script.split("<<'FETCH'\n", 1)[1].split("\nFETCH", 1)[0]
        pins = self.root / "runtime-pins.json"
        pins.write_text(json.dumps({"alpine": {"version": "3.24.2", "minirootfs": {"aarch64": {
            "url": "https://invalid.example/not-requested", "sha256": assets.sha256(self.archive),
        }}}}))
        result = subprocess.run([sys.executable, "-c", reader, str(pins), str(self.archive)], capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), "3.24.2")
        pins.write_text(json.dumps({"rootfs": {"version": "3.21.3", "archives": {}}}))
        result = subprocess.run([sys.executable, "-c", reader, str(pins), str(self.archive)], capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)


if __name__ == "__main__":
    unittest.main()
