#!/usr/bin/env python3
"""Exercise iOS CI storage ownership, artifact checks, and workflow ordering."""

from __future__ import annotations

import importlib.util
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts/ci/ios_ci_storage.py"
WORKFLOW = ROOT / ".github/workflows/clients.yml"
SPEC = importlib.util.spec_from_file_location("ios_ci_storage", SCRIPT)
assert SPEC and SPEC.loader
STORAGE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(STORAGE)


def write(path: Path, data: bytes = b"test input") -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)


class IosCiStorageTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tempdir = tempfile.TemporaryDirectory(prefix="ios-storage-test-")
        self.addCleanup(self.tempdir.cleanup)
        self.base = Path(self.tempdir.name).resolve()
        self.workspace = self.base / "workspace"
        self.runner_temp = self.base / "runner temp"
        self.workspace.mkdir()
        self.runner_temp.mkdir()
        self.env = {
            **os.environ,
            "GITHUB_ACTIONS": "true",
            "GITHUB_JOB": "ios",
            "GITHUB_RUN_ID": "123456",
            "GITHUB_RUN_ATTEMPT": "2",
            "GITHUB_WORKSPACE": str(self.workspace),
            "RUNNER_TEMP": str(self.runner_temp),
        }

    def run_helper(self, *args: str, env: dict[str, str] | None = None) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [sys.executable, str(SCRIPT), *args], env=env or self.env,
            text=True, capture_output=True, check=False,
        )

    def prepare(self) -> Path:
        result = self.run_helper("prepare")
        self.assertEqual(result.returncode, 0, result.stderr)
        return Path(result.stdout.strip())

    def assert_released(self, target: Path) -> None:
        self.assertTrue(target.is_dir())
        self.assertEqual([item.name for item in target.iterdir()], [STORAGE.MARKER])

    def create_inputs(self, target: Path) -> tuple[Path, Path]:
        ios = self.workspace / "apps/ios/native"
        generated = ios / "Generated"
        for name in ("client.swift", "harness_runtime.swift", "LingxiCodeBindings.swift", "module.modulemap", "clientFFI.h"):
            write(generated / name, name.encode())
        frameworks = ios / "Frameworks"
        for name in ("GStreamer", "MobileLinuxNativeSupport", "sherpa-onnx", "onnxruntime"):
            write(frameworks / f"{name}.xcframework/Info.plist")
        # A normal, entirely internal framework symlink is permitted.
        internal = frameworks / "onnxruntime.xcframework"
        write(internal / "archive.a", b"onnx archive")
        (internal / "libonnxruntime.a").unlink(missing_ok=True)
        (internal / "libonnxruntime.a").symlink_to("archive.a")

        rootfs = ios / "build/linux-runtime/openminis"
        write(rootfs / "manifest.json", b"{}")
        write(rootfs / "resources/alpine-rootfs.zip", b"rootfs")
        write(rootfs / "resources/RootfsPatch.bundle/Info.plist")
        # The unbundled guest root can contain links meaningful only inside
        # Linux. It must stay untouched and must not block CI cleanup.
        guest = rootfs / "resources/alpine-rootfs"
        guest.mkdir(exist_ok=True)
        (guest / "guest-absolute-link").unlink(missing_ok=True)
        (guest / "guest-absolute-link").symlink_to("/guest-only/target")
        write(self.workspace / "fixtures/local-apps/v2/static/lingxi-app.json", b"fixture")

        ffi = frameworks / "LingxiCodeFFI.xcframework"
        libraries = []
        for identifier, variant, architectures, content, source in (
            ("ios-arm64", None, ["arm64"], b"device archive", target / "xcframework-work/aarch64-apple-ios/libios_framework.a"),
            ("ios-arm64_x86_64-simulator", "simulator", ["arm64", "x86_64"], b"simulator archive", target / "xcframework-work/libios_framework.a"),
        ):
            write(source, content)
            write(ffi / identifier / "libios_framework.a", content)
            for name in ("module.modulemap", "clientFFI.h"):
                write(ffi / identifier / "Headers" / name, (generated / name).read_bytes())
            entry = {
                "LibraryIdentifier": identifier,
                "LibraryPath": "libios_framework.a",
                "HeadersPath": "Headers",
                "SupportedArchitectures": architectures,
                "SupportedPlatform": "ios",
            }
            if variant:
                entry["SupportedPlatformVariant"] = variant
            libraries.append(entry)
        write(ffi / "Info.plist", plistlib.dumps({"AvailableLibraries": libraries}))
        write(target / "debug/deps/transient.o", b"temporary build object")
        return generated, ffi

    def test_releases_only_owned_intermediates_and_preserves_inputs(self) -> None:
        unrelated = self.runner_temp / "another-job"
        write(unrelated / "keep", b"other job")
        target = self.prepare()
        generated, ffi = self.create_inputs(target)
        preserved = {
            p: p.read_bytes() for tree in (self.workspace, unrelated)
            for p in tree.rglob("*") if p.is_file() and not p.is_symlink()
        }
        result = self.run_helper("release", str(target))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assert_released(target)
        self.assertTrue((generated / "client.swift").is_file())
        self.assertTrue((ffi / "Info.plist").is_file())
        self.assertTrue((ffi.parent / "onnxruntime.xcframework/libonnxruntime.a").is_symlink())
        self.assertTrue((self.workspace / "apps/ios/native/build/linux-runtime/openminis/resources/alpine-rootfs/guest-absolute-link").is_symlink())
        self.assertEqual({p: p.read_bytes() for p in preserved}, preserved)
        self.assertIn("before iOS intermediate cleanup", result.stderr)
        self.assertIn("after iOS intermediate cleanup", result.stderr)
        self.assertEqual(self.run_helper("report", str(target)).returncode, 0)

    def test_rejects_framework_symlink_into_build_target(self) -> None:
        target = self.prepare()
        _, ffi = self.create_inputs(target)
        archive = ffi / "ios-arm64/libios_framework.a"
        archive.unlink()
        archive.symlink_to(target / "xcframework-work/aarch64-apple-ios/libios_framework.a")
        result = self.run_helper("release", str(target))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("symlink", result.stderr)
        self.assertTrue(target.exists())
        self.assertTrue(archive.exists())

    def test_rejects_archive_or_header_mismatch(self) -> None:
        for name in ("ios-arm64/libios_framework.a", "ios-arm64/Headers/clientFFI.h"):
            with self.subTest(name=name):
                target = self.prepare()
                _, ffi = self.create_inputs(target)
                original = (ffi / name).read_bytes()
                write(ffi / name, b"different")
                result = self.run_helper("release", str(target))
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("differs", result.stderr)
                self.assertTrue(target.exists())
                write(ffi / name, original)

    def test_rejects_missing_rootfs_and_missing_simulator_architecture(self) -> None:
        target = self.prepare()
        _, ffi = self.create_inputs(target)
        rootfs = self.workspace / "apps/ios/native/build/linux-runtime/openminis/resources/alpine-rootfs.zip"
        rootfs.unlink()
        result = self.run_helper("release", str(target))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("alpine-rootfs.zip", result.stderr)
        self.assertTrue(target.exists())
        write(rootfs, b"rootfs")
        info = plistlib.loads((ffi / "Info.plist").read_bytes())
        info["AvailableLibraries"][1]["SupportedArchitectures"] = ["arm64"]
        write(ffi / "Info.plist", plistlib.dumps(info))
        result = self.run_helper("release", str(target))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("both simulator architectures", result.stderr)
        self.assertTrue(target.exists())

    def test_rejects_tampered_marker_other_run_and_other_job(self) -> None:
        target = self.prepare()
        self.create_inputs(target)
        other = {**self.env, "GITHUB_RUN_ATTEMPT": "3"}
        self.assertNotEqual(self.run_helper("release", str(target), env=other).returncode, 0)
        wrong_job = {**self.env, "GITHUB_JOB": "android"}
        self.assertNotEqual(self.run_helper("release", str(target), env=wrong_job).returncode, 0)
        (target / ".lingxi-ios-ci-storage.json").write_text("{}")
        result = self.run_helper("release", str(target))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("marker", result.stderr)
        self.assertTrue(target.exists())

    def test_rejects_symlink_target_and_arbitrary_directories(self) -> None:
        target = self.prepare()
        self.create_inputs(target)
        real = self.runner_temp / "saved-target"
        target.rename(real)
        target.symlink_to(real, target_is_directory=True)
        result = self.run_helper("release", str(target))
        self.assertNotEqual(result.returncode, 0)
        self.assertTrue(real.exists())
        self.assertNotEqual(self.run_helper("release", str(self.workspace)).returncode, 0)
        self.assertTrue(self.workspace.exists())

    def test_rejects_replaced_directory_with_copied_ownership_marker(self) -> None:
        target = self.prepare()
        self.create_inputs(target)
        displaced = self.runner_temp / "displaced-owned-target"
        target.rename(displaced)
        target.mkdir()
        shutil.copyfile(displaced / STORAGE.MARKER, target / STORAGE.MARKER)
        write(target / "keep.txt", b"preserve the replacement")
        result = self.run_helper("release", str(target))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("marker", result.stderr)
        self.assertEqual((target / "keep.txt").read_bytes(), b"preserve the replacement")
        self.assertTrue((displaced / "xcframework-work").is_dir())

    def test_directory_replacement_during_cleanup_never_deletes_replacement(self) -> None:
        target = self.prepare()
        self.create_inputs(target)
        displaced = self.runner_temp / "displaced-owned-target"
        replacement = self.runner_temp / "unrelated-output"
        write(replacement / "keep.txt", b"preserve the replacement")
        original = STORAGE.remove_owned_contents

        def replace_then_remove(target_fd: int) -> None:
            target.rename(displaced)
            replacement.rename(target)
            original(target_fd)

        ctx = {"run_id": "123456", "attempt": "2", "temp": str(self.runner_temp), "workspace": str(self.workspace)}
        with patch.object(STORAGE, "remove_owned_contents", side_effect=replace_then_remove):
            with self.assertRaises(ValueError):
                STORAGE.release(ctx, str(target))
        self.assertEqual((target / "keep.txt").read_bytes(), b"preserve the replacement")
        self.assert_released(displaced)

    def test_subdirectory_replacement_during_cleanup_never_deletes_replacement(self) -> None:
        target = self.prepare()
        self.create_inputs(target)
        displaced = target / "displaced-work"
        replacement = self.workspace / "unrelated-output"
        write(replacement / "keep.txt", b"preserve the replacement")
        original = STORAGE.remove_owned_directory
        swapped = False

        def replace_then_remove(parent_fd: int, name: str, expected: os.stat_result) -> None:
            nonlocal swapped
            if name == "xcframework-work":
                swapped = True
                (target / name).rename(displaced)
                replacement.rename(target / name)
            original(parent_fd, name, expected)

        ctx = {"run_id": "123456", "attempt": "2", "temp": str(self.runner_temp), "workspace": str(self.workspace)}
        with patch.object(STORAGE, "remove_owned_directory", side_effect=replace_then_remove):
            with self.assertRaisesRegex(ValueError, "subdirectory changed"):
                STORAGE.release(ctx, str(target))
        self.assertTrue(swapped)
        self.assertEqual((target / "xcframework-work/keep.txt").read_bytes(), b"preserve the replacement")
        self.assertTrue((displaced / "aarch64-apple-ios/libios_framework.a").is_file())

    def test_subdirectory_symlink_swap_during_cleanup_does_not_follow_link(self) -> None:
        target = self.prepare()
        self.create_inputs(target)
        displaced = target / "displaced-work"
        outside = self.workspace / "unrelated-output"
        write(outside / "keep.txt", b"preserve symlink target")
        original = STORAGE.remove_owned_directory
        swapped = False

        def replace_then_remove(parent_fd: int, name: str, expected: os.stat_result) -> None:
            nonlocal swapped
            if name == "xcframework-work":
                swapped = True
                (target / name).rename(displaced)
                (target / name).symlink_to(outside, target_is_directory=True)
            original(parent_fd, name, expected)

        ctx = {"run_id": "123456", "attempt": "2", "temp": str(self.runner_temp), "workspace": str(self.workspace)}
        with patch.object(STORAGE, "remove_owned_directory", side_effect=replace_then_remove):
            with self.assertRaises(OSError):
                STORAGE.release(ctx, str(target))
        self.assertTrue(swapped)
        self.assertEqual((outside / "keep.txt").read_bytes(), b"preserve symlink target")
        self.assertTrue((displaced / "aarch64-apple-ios/libios_framework.a").is_file())

    def test_new_target_entry_during_cleanup_is_reported(self) -> None:
        target = self.prepare()
        self.create_inputs(target)
        original = STORAGE.remove_directory_contents

        def add_entry(target_fd: int, *, preserve_marker: bool = False) -> None:
            original(target_fd, preserve_marker=preserve_marker)
            if preserve_marker:
                write(target / "late-output", b"concurrent output")

        ctx = {"run_id": "123456", "attempt": "2", "temp": str(self.runner_temp), "workspace": str(self.workspace)}
        with patch.object(STORAGE, "remove_directory_contents", side_effect=add_entry):
            with self.assertRaisesRegex(ValueError, "gained entries"):
                STORAGE.release(ctx, str(target))
        self.assertEqual((target / "late-output").read_bytes(), b"concurrent output")

    def test_preserves_hardlinks_and_unlinks_directory_symlinks(self) -> None:
        target = self.prepare()
        _, ffi = self.create_inputs(target)
        source = target / "xcframework-work/aarch64-apple-ios/libios_framework.a"
        packaged = ffi / "ios-arm64/libios_framework.a"
        packaged.unlink()
        os.link(source, packaged)
        unrelated = self.workspace / "unrelated"
        write(unrelated / "keep.txt", b"preserve hardlink")
        os.link(unrelated / "keep.txt", target / "hardlink-to-preserve")
        (target / "directory-symlink").symlink_to(unrelated, target_is_directory=True)
        result = self.run_helper("release", str(target))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(packaged.read_bytes(), b"device archive")
        self.assertEqual((unrelated / "keep.txt").read_bytes(), b"preserve hardlink")
        self.assert_released(target)

    def test_refuses_workspace_temp_overlap_and_non_ci(self) -> None:
        nested = self.workspace / "runner-temp"
        nested.mkdir()
        result = self.run_helper("prepare", env={**self.env, "RUNNER_TEMP": str(nested)})
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("separate", result.stderr)
        self.assertEqual(list(nested.iterdir()), [])
        result = self.run_helper("prepare", env={**self.env, "GITHUB_ACTIONS": "false"})
        self.assertNotEqual(result.returncode, 0)

    @unittest.skipUnless(sys.platform == "darwin" and shutil.which("xcrun") and shutil.which("xcodebuild"),
                         "the real XCFramework packaging probe requires Xcode")
    def test_real_xcodebuild_packaging_is_independent_of_intermediates(self) -> None:
        target = self.prepare()
        generated, ffi = self.create_inputs(target)
        write(generated / "clientFFI.h", b"int ci_probe(void);\n")
        write(generated / "module.modulemap", b'module ClientFFI { header "clientFFI.h" export * }\n')
        source = self.base / "probe.c"
        write(source, b"int ci_probe(void) { return 42; }\n")
        work = target / "xcframework-work"
        device = work / "aarch64-apple-ios/libios_framework.a"
        sim_libraries = []
        for name, triple in (
            ("device", "arm64-apple-ios26.0"),
            ("arm64-sim", "arm64-apple-ios26.0-simulator"),
            ("x86_64-sim", "x86_64-apple-ios26.0-simulator"),
        ):
            obj = self.base / f"{name}.o"
            library = device if name == "device" else self.base / f"{name}.a"
            subprocess.run(["xcrun", "clang", "-target", triple, "-c", str(source), "-o", str(obj)], check=True, capture_output=True)
            subprocess.run(["xcrun", "libtool", "-static", "-o", str(library), str(obj)], check=True, capture_output=True)
            if name != "device":
                sim_libraries.append(library)
        sim = work / "libios_framework.a"
        subprocess.run(["xcrun", "lipo", "-create", *map(str, sim_libraries), "-output", str(sim)], check=True, capture_output=True)
        shutil.rmtree(ffi)
        subprocess.run([
            "xcodebuild", "-create-xcframework",
            "-library", str(device), "-headers", str(generated),
            "-library", str(sim), "-headers", str(generated),
            "-output", str(ffi),
        ], check=True, capture_output=True)
        result = self.run_helper("release", str(target))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue((ffi / "Info.plist").is_file())
        self.assert_released(target)

    def test_workflow_keeps_real_simulator_tests_after_cleanup(self) -> None:
        text = WORKFLOW.read_text()
        ios = text.split("\n  ios:\n", 1)[1]
        self.assertLess(ios.index("Allocate private iOS build storage"), ios.index("Build XCFramework + Swift bindings"))
        self.assertLess(ios.index("Build XCFramework + Swift bindings"), ios.index("Release verified iOS build intermediates"))
        self.assertLess(ios.index("Release verified iOS build intermediates"), ios.index("xcodebuild test (simulator round-trip)"))
        self.assertIn("CARGO_TARGET_DIR: ${{ steps.ios-storage.outputs.target }}", ios)
        self.assertIn("LINGXI_XCFRAMEWORK_BUILD_DIR: ${{ steps.ios-storage.outputs.target }}/xcframework-work", ios)
        self.assertIn("xcodebuild test \\", ios)
        self.assertIn("-scheme LingxiCode", ios)
        self.assertNotIn("continue-on-error", ios)


if __name__ == "__main__":
    unittest.main()
