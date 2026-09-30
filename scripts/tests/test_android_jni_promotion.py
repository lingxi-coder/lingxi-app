#!/usr/bin/env python3
"""Execute the real JNI build script with a tiny Cargo/NDK failure boundary."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "apps/android/native/scripts/build-jni.sh"
ABIS = ("arm64-v8a", "x86_64")
PACKAGE = Path("com/lingxi/code/bindings")
LIBRARY = "libandroid_aar.so"

MOCK_TOOL = r'''#!/usr/bin/env python3
import os
from pathlib import Path
import subprocess
import sys

tool = Path(sys.argv[0]).name
args = sys.argv[1:]
if tool == "rustup":
    if args[:2] == ["show", "active-toolchain"]:
        print("fixture-toolchain (default)")
    elif args[:2] == ["target", "list"]:
        print("aarch64-linux-android\nx86_64-linux-android")
elif tool == "cargo":
    if args[0] == "ndk":
        output = Path(args[args.index("-o") + 1])
        for abi in ("arm64-v8a", "x86_64"):
            target = output / abi / "libandroid_aar.so"
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(f"new-library-{abi}".encode())
        target = Path(os.environ["CARGO_TARGET_DIR"]) / "aarch64-linux-android/release/libandroid_aar.so"
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(b"unstripped metadata fixture")
    elif args[0] == "run":
        output = Path(args[args.index("--out-dir") + 1]) / "com/lingxi/code/bindings"
        output.mkdir(parents=True, exist_ok=True)
        (output / "partial.kt").write_text("partially generated")
        if os.environ.get("JNI_FIXTURE_FAIL_BINDGEN") == "1":
            print("fixture bindgen failed after writing partial output", file=sys.stderr)
            sys.exit(17)
        (output / "partial.kt").unlink()
        for component in ("client", "runtime", "android"):
            package = output / component
            package.mkdir()
            (package / "bindings.kt").write_text(f"new bindings for {component}")
elif tool == "curl":
    output = Path(args[args.index("-o") + 1])
    counter = Path(os.environ["JNI_FIXTURE_DOWNLOAD_COUNT"])
    counter.write_text(str(int(counter.read_text()) + 1 if counter.exists() else 1))
    if os.environ.get("JNI_FIXTURE_FAIL_DOWNLOAD") == "1":
        output.write_bytes(b"partial archive")
        print("fixture interrupted archive transfer", file=sys.stderr)
        sys.exit(22)
    output.write_bytes(b"incorrect archive" if os.environ.get("JNI_FIXTURE_BAD_DOWNLOAD") == "1"
                       else b"tiny voice runtime fixture")
elif tool in ("cp", "mv"):
    target = Path(args[-1])
    source = Path(args[-2])
    final = Path(os.environ["JNI_FIXTURE_FINAL_JNI"]) / "x86_64/libandroid_aar.so"
    flag = Path(os.environ["JNI_FIXTURE_FAILURE_MARKER"])
    actual_target = target / source.name if target.is_dir() and tool == "cp" else target
    if os.environ.get("JNI_FIXTURE_FAIL_AFTER_BACKUP") == "1" and tool == "mv" and source == final and target.name == "old" and not flag.exists():
        status = subprocess.call([os.environ["JNI_FIXTURE_REAL_MV"]] + args)
        if status:
            sys.exit(status)
        flag.write_text("mv: interrupted after moving the old artifact")
        print("fixture failure after backup rename", file=sys.stderr)
        sys.exit(19)
    if os.environ.get("JNI_FIXTURE_FAIL_PROMOTION") == "1" and actual_target == final and not flag.exists():
        flag.write_text(f"{tool}: failed second ABI promotion")
        print("fixture second ABI promotion failure", file=sys.stderr)
        sys.exit(19)
    sys.exit(subprocess.call([os.environ[f"JNI_FIXTURE_REAL_{tool.upper()}"]] + args))
'''


def files(root):
    return {str(path.relative_to(root)): path.read_bytes()
            for path in root.rglob("*") if path.is_file()} if root.exists() else {}


class AndroidJniPromotionTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="jni-promotion-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.repo = self.root / "fixture repo"
        self.native = self.repo / "apps/android/native"
        script = self.native / "scripts/build-jni.sh"
        script.parent.mkdir(parents=True)
        shutil.copy2(SCRIPT, script)
        self.script = script
        ffi = self.repo / "apps/android/ffi/uniffi.toml"
        ffi.parent.mkdir(parents=True)
        ffi.write_text("# unused mock bindgen config\n")
        (self.repo / "Cargo.toml").write_text("# no real Cargo is invoked\n")
        archive = self.native / "app/libs/fixture-runtime.aar"
        archive.parent.mkdir(parents=True)
        archive.write_bytes(b"tiny voice runtime fixture")
        manifest = self.repo / "resources/voice/models.json"
        manifest.parent.mkdir(parents=True)
        manifest.write_text(json.dumps({"runtime": {"version": "fixture", "android": {
            "name": archive.name, "url": "https://example.invalid/never-downloaded.aar",
            "sha256": hashlib.sha256(archive.read_bytes()).hexdigest()}}}))
        self.ndk = self.root / "mock ndk"
        for tag in ("darwin-x86_64", "linux-x86_64"):
            strip = self.ndk / f"toolchains/llvm/prebuilt/{tag}/bin/llvm-strip"
            strip.parent.mkdir(parents=True)
            strip.write_text("#!/bin/sh\nexit 0\n")
            strip.chmod(0o755)
        self.bin = self.root / "mock tools"
        self.bin.mkdir()
        for tool in ("cargo", "cargo-ndk", "rustc", "rustup", "curl", "cp", "mv"):
            mock = self.bin / tool
            mock.write_text(MOCK_TOOL)
            mock.chmod(0o755)
        self.jni = self.native / "app/src/play/jniLibs"
        self.kotlin = self.native / "app/src/main/java"
        self.failure_marker = self.root / "promotion-failed"
        self.aar = archive
        self.download_count = self.root / "download-count"
        self.env = {**os.environ, "PATH": str(self.bin) + os.pathsep + os.environ["PATH"],
                    "ANDROID_NDK_HOME": str(self.ndk), "CARGO_TARGET_DIR": str(self.repo / "target"),
                    "JNI_FIXTURE_FINAL_JNI": str(self.jni),
                    "JNI_FIXTURE_FAILURE_MARKER": str(self.failure_marker),
                    "JNI_FIXTURE_DOWNLOAD_COUNT": str(self.download_count),
                    "JNI_FIXTURE_REAL_CP": shutil.which("cp"), "JNI_FIXTURE_REAL_MV": shutil.which("mv")}
        # Exercise defaults even if the developer's environment has output overrides.
        self.env.pop("LINGXI_ANDROID_JNILIBS_DIR", None)
        self.env.pop("LINGXI_KOTLIN_OUT", None)
        self.handwritten = self.kotlin / "com/lingxi/code/Handwritten.kt"
        self.handwritten.parent.mkdir(parents=True)
        self.handwritten.write_text("handwritten app source")
        (self.handwritten.parent / "Handwritten.java").write_text("class Handwritten {}")
        for abi in ABIS:
            support = self.jni / abi / "libnative-support.so"
            support.parent.mkdir(parents=True)
            support.write_bytes(f"existing native support for {abi}".encode())

    def seed_previous_pair(self):
        for abi in ABIS:
            (self.jni / abi / LIBRARY).write_bytes(f"old-library-{abi}".encode())
        old = self.kotlin / PACKAGE / "old-component/stale.kt"
        old.parent.mkdir(parents=True)
        old.write_text("old bindings")

    def run_script(self, **settings):
        result = subprocess.run(["bash", str(self.script), "--variant", "play"], cwd=self.root,
                                env={**self.env, **settings}, text=True, capture_output=True, timeout=15)
        return result

    def assert_preserved(self, before):
        self.assertEqual((files(self.jni), files(self.kotlin)), before)
        self.assertFalse((self.kotlin / ".jni-promotion.lock").exists())
        self.assertFalse(any(self.kotlin.rglob(".jni-stage.*")))
        self.assertFalse(any(self.jni.rglob(".jni-stage.*")))

    def test_failed_bindgen_preserves_previous_pair(self):
        self.seed_previous_pair()
        before = (files(self.jni), files(self.kotlin))
        result = self.run_script(JNI_FIXTURE_FAIL_BINDGEN="1")
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("bindgen failed", result.stderr)
        self.assert_preserved(before)

    def test_second_abi_promotion_failure_rolls_back_previous_pair(self):
        self.seed_previous_pair()
        before = (files(self.jni), files(self.kotlin))
        result = self.run_script(JNI_FIXTURE_FAIL_PROMOTION="1")
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertTrue(self.failure_marker.exists(), result.stdout + result.stderr)
        self.assert_preserved(before)

    def test_backup_command_failure_after_rename_restores_the_moved_original(self):
        self.seed_previous_pair()
        before = (files(self.jni), files(self.kotlin))
        result = self.run_script(JNI_FIXTURE_FAIL_AFTER_BACKUP="1")
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("after backup rename", result.stderr)
        self.assert_preserved(before)

    def test_success_replaces_owned_artifacts_and_retains_handwritten_and_support_files(self):
        self.seed_previous_pair()
        support = {abi: (self.jni / abi / "libnative-support.so").read_bytes() for abi in ABIS}
        result = self.run_script()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        for abi in ABIS:
            self.assertEqual((self.jni / abi / LIBRARY).read_bytes(), f"new-library-{abi}".encode())
            self.assertEqual((self.jni / abi / "libnative-support.so").read_bytes(), support[abi])
        self.assertEqual(self.handwritten.read_text(), "handwritten app source")
        self.assertEqual((self.handwritten.parent / "Handwritten.java").read_text(), "class Handwritten {}")
        generated = files(self.kotlin / PACKAGE)
        self.assertEqual(len(generated), 3)
        self.assertTrue(all(value.startswith(b"new bindings") for value in generated.values()))

    def test_generation_failure_with_no_previous_artifacts_leaves_no_partial_pair(self):
        before = (files(self.jni), files(self.kotlin))
        result = self.run_script(JNI_FIXTURE_FAIL_BINDGEN="1")
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assert_preserved(before)

    def test_promotion_failure_with_no_previous_artifacts_removes_new_pair(self):
        before = (files(self.jni), files(self.kotlin))
        result = self.run_script(JNI_FIXTURE_FAIL_PROMOTION="1")
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertTrue(self.failure_marker.exists(), result.stdout + result.stderr)
        self.assert_preserved(before)

    def test_success_with_no_previous_artifacts_publishes_complete_pair(self):
        result = self.run_script()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        for abi in ABIS:
            self.assertEqual((self.jni / abi / LIBRARY).read_bytes(), f"new-library-{abi}".encode())
            self.assertTrue((self.jni / abi / "libnative-support.so").exists())
        self.assertEqual(len(files(self.kotlin / PACKAGE)), 3)
        self.assertEqual(self.handwritten.read_text(), "handwritten app source")

    def test_another_publisher_lock_rejects_without_replacing_artifacts_or_releasing_its_lock(self):
        self.seed_previous_pair()
        before = (files(self.jni), files(self.kotlin))
        lock = self.jni / ".jni-promotion.lock"
        lock.mkdir()
        result = self.run_script()
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("publication is locked", result.stderr)
        self.assert_preserved(before)
        self.assertTrue(lock.exists())

    def assert_download_cleanup(self):
        self.assertFalse(any(self.aar.parent.glob(".sherpa-aar-download.*")))

    def test_interrupted_download_does_not_poison_a_healthy_retry(self):
        self.seed_previous_pair()
        before = (files(self.jni), files(self.kotlin))
        self.aar.unlink()
        first = self.run_script(JNI_FIXTURE_FAIL_DOWNLOAD="1")
        self.assertNotEqual(first.returncode, 0, first.stdout + first.stderr)
        self.assertFalse(self.aar.exists())
        self.assert_download_cleanup()
        self.assert_preserved(before)
        second = self.run_script()
        self.assertEqual(second.returncode, 0, second.stdout + second.stderr)
        self.assertEqual(self.download_count.read_text(), "2")
        self.assertEqual(self.aar.read_bytes(), b"tiny voice runtime fixture")
        self.assert_download_cleanup()

    def test_checksum_mismatch_does_not_poison_a_healthy_retry(self):
        self.aar.unlink()
        first = self.run_script(JNI_FIXTURE_BAD_DOWNLOAD="1")
        self.assertNotEqual(first.returncode, 0, first.stdout + first.stderr)
        self.assertIn("checksum mismatch", first.stderr)
        self.assertFalse(self.aar.exists())
        self.assert_download_cleanup()
        second = self.run_script()
        self.assertEqual(second.returncode, 0, second.stdout + second.stderr)
        self.assertEqual(self.download_count.read_text(), "2")
        self.assertEqual(self.aar.read_bytes(), b"tiny voice runtime fixture")

    def test_preexisting_corrupt_cache_is_replaced_only_by_a_validated_download(self):
        self.aar.write_bytes(b"preexisting partial archive")
        result = self.run_script()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(self.download_count.read_text(), "1")
        self.assertEqual(self.aar.read_bytes(), b"tiny voice runtime fixture")
        self.assert_download_cleanup()

    def test_failed_refresh_of_corrupt_cache_leaves_no_corrupt_final_archive(self):
        self.aar.write_bytes(b"preexisting partial archive")
        result = self.run_script(JNI_FIXTURE_FAIL_DOWNLOAD="1")
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertFalse(self.aar.exists())
        self.assert_download_cleanup()
        retry = self.run_script()
        self.assertEqual(retry.returncode, 0, retry.stdout + retry.stderr)
        self.assertEqual(self.download_count.read_text(), "2")

    def test_valid_cache_is_reused_without_a_network_attempt(self):
        before = self.aar.read_bytes()
        result = self.run_script(JNI_FIXTURE_FAIL_DOWNLOAD="1", JNI_FIXTURE_BAD_DOWNLOAD="1")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(self.aar.read_bytes(), before)
        self.assertFalse(self.download_count.exists())
        self.assert_download_cleanup()


if __name__ == "__main__":
    unittest.main(verbosity=2)
