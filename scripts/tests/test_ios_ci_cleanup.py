#!/usr/bin/env python3
"""Exercise the iOS CI cleanup boundary with disposable build fixtures."""
import contextlib
import importlib.util
import io
from pathlib import Path
import plistlib
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import patch
import sys


sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("ios_ci_cleanup", ROOT / "scripts/ci/reclaim_ios_ci_build_space.py")
assert SPEC and SPEC.loader
cleanup = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(cleanup)


class CleanupTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        base = Path(self.temp.name).resolve()
        self.repo = base / "repo"
        self.runner_temp = base / "runner-temp"
        self.runner_temp.mkdir()
        self.target = self.runner_temp / "lingxi-ios-cargo-123-2"
        self.target.mkdir()
        (self.target / "intermediate").write_text("cargo")
        self.scratch = self.repo / "apps/ios/native/build/xcframework"
        self.scratch.mkdir(parents=True)
        (self.scratch / "intermediate").write_text("assembly")
        self.native = self.repo / "apps/ios/native"
        self.frameworks = self.native / "Frameworks"
        self.framework = self.frameworks / "ClientFFI.xcframework"
        self.generated = self.native / "Generated"
        self.generated.mkdir()
        (self.generated / "client.swift").write_text("bindings")
        self.evidence = self.runner_temp / "sdk-rootfs/manifest.json"
        self.evidence.parent.mkdir()
        self.evidence.write_text("evidence")
        self.libraries = []
        for identifier, variant, architectures in (
            ("ios-arm64", None, ["arm64"]),
            ("ios-arm64_x86_64-simulator", "simulator", ["arm64", "x86_64"]),
        ):
            headers = self.framework / identifier / "Headers"
            headers.mkdir(parents=True)
            (headers / "module.modulemap").write_text("module")
            (headers / "clientFFI.h").write_text("header")
            (headers.parent / "libios_framework.a").write_text("archive")
            entry = {
                "LibraryIdentifier": identifier, "LibraryPath": "libios_framework.a",
                "HeadersPath": "Headers", "SupportedPlatform": "ios",
                "SupportedArchitectures": architectures,
            }
            if variant:
                entry["SupportedPlatformVariant"] = variant
            self.libraries.append(entry)
        self.write_plist()

    def write_plist(self):
        with (self.framework / "Info.plist").open("wb") as output:
            plistlib.dump({"CFBundlePackageType": "XFWK", "AvailableLibraries": self.libraries}, output)

    def reclaim(self, run_id="123", run_attempt="2"):
        def architectures(path):
            return {"arm64", "x86_64"} if "simulator" in str(path) else {"arm64"}
        with patch.object(cleanup, "_archive_architectures", side_effect=architectures):
            with patch.object(cleanup.subprocess, "run"), contextlib.redirect_stdout(io.StringIO()):
                cleanup.reclaim(self.repo, self.runner_temp, run_id, run_attempt)

    def assert_intermediates_preserved(self):
        self.assertTrue((self.target / "intermediate").is_file())
        self.assertTrue((self.scratch / "intermediate").is_file())

    def test_removes_only_completed_intermediates_and_preserves_outputs(self):
        (self.generated / "safe-link.swift").symlink_to("client.swift")
        self.reclaim()
        self.assertFalse(self.target.exists())
        self.assertFalse(self.scratch.exists())
        self.assertEqual(self.evidence.read_text(), "evidence")
        self.assertEqual((self.generated / "safe-link.swift").read_text(), "bindings")
        self.assertTrue((self.framework / "ios-arm64/libios_framework.a").is_file())
        self.assertTrue((self.framework / "ios-arm64_x86_64-simulator/libios_framework.a").is_file())

    def test_rejects_a_link_from_final_outputs_into_an_intermediate(self):
        (self.generated / "unsafe.swift").symlink_to(self.target / "intermediate")
        with self.assertRaisesRegex(cleanup.CleanupError, "output link leaves"):
            self.reclaim()
        self.assert_intermediates_preserved()

    def test_rejects_a_framework_archive_link_into_an_intermediate(self):
        archive = self.framework / "ios-arm64/libios_framework.a"
        archive.unlink()
        archive.symlink_to(self.scratch / "intermediate")
        with self.assertRaisesRegex(cleanup.CleanupError, "output link leaves"):
            self.reclaim()
        self.assert_intermediates_preserved()

    def test_ignores_unrelated_valid_framework_metadata(self):
        other = self.frameworks / "Other.xcframework"
        other.mkdir()
        with (other / "Info.plist").open("wb") as output:
            plistlib.dump({"CFBundlePackageType": "XFWK", "AvailableLibraries": [
                {"LibraryPath": "libother.a", "SupportedPlatform": "ios"},
            ]}, output)
        self.reclaim()
        self.assertTrue((other / "Info.plist").is_file())
        self.assertFalse(self.target.exists())
        self.assertFalse(self.scratch.exists())

    def test_rejects_missing_simulator_architecture_before_deleting(self):
        self.libraries[1]["SupportedArchitectures"] = ["arm64"]
        self.write_plist()
        with self.assertRaisesRegex(cleanup.CleanupError, "missing.*x86_64"):
            self.reclaim()
        self.assert_intermediates_preserved()

    def test_rejects_missing_archive_before_deleting(self):
        (self.framework / "ios-arm64/libios_framework.a").unlink()
        with self.assertRaisesRegex(cleanup.CleanupError, "incomplete XCFramework"):
            self.reclaim()
        self.assert_intermediates_preserved()

    def test_rejects_multiple_matching_ffi_frameworks(self):
        shutil.copytree(self.framework, self.frameworks / "AnotherFFI.xcframework")
        with self.assertRaisesRegex(cleanup.CleanupError, "expected exactly one FFI XCFramework, found 2"):
            self.reclaim()
        self.assert_intermediates_preserved()

    def test_rejects_missing_ffi_library_candidate(self):
        for entry in self.libraries:
            entry["LibraryPath"] = "libother.a"
        self.write_plist()
        with self.assertRaisesRegex(cleanup.CleanupError, "expected exactly one FFI XCFramework, found 0"):
            self.reclaim()
        self.assert_intermediates_preserved()

    def test_rejects_one_matching_slice_and_one_unrelated_library(self):
        self.libraries[0]["LibraryPath"] = "libother.a"
        self.write_plist()
        with self.assertRaisesRegex(cleanup.CleanupError, "does not match the FFI ABI"):
            self.reclaim()
        self.assert_intermediates_preserved()

    def test_rejects_malformed_metadata_in_other_staged_framework(self):
        (self.frameworks / "Unknown.xcframework").mkdir()
        with self.assertRaisesRegex(cleanup.CleanupError, "cannot read XCFramework metadata"):
            self.reclaim()
        self.assert_intermediates_preserved()

    def test_rejects_archive_missing_a_declared_architecture(self):
        with patch.object(cleanup, "_archive_architectures", return_value={"arm64"}):
            with self.assertRaisesRegex(cleanup.CleanupError, "missing.*x86_64"):
                cleanup.reclaim(self.repo, self.runner_temp, "123", "2")
        self.assert_intermediates_preserved()

    def test_rejects_symlinked_cleanup_directory(self):
        (self.target / "intermediate").unlink()
        self.target.rmdir()
        self.target.symlink_to(self.scratch, target_is_directory=True)
        with self.assertRaisesRegex(cleanup.CleanupError, "expected real directory"):
            self.reclaim()
        self.assertTrue((self.scratch / "intermediate").is_file())

    def test_rejects_untrusted_run_identifiers(self):
        with self.assertRaisesRegex(cleanup.CleanupError, "decimal numbers"):
            self.reclaim("../elsewhere")
        self.assert_intermediates_preserved()

    def test_workflow_keeps_full_build_then_cleans_before_xcode_test(self):
        workflow = (ROOT / ".github/workflows/clients.yml").read_text()
        ios = workflow.split("  ios:\n", 1)[1]
        self.assertIn("CARGO_TARGET_DIR: ${{ runner.temp }}/lingxi-ios-cargo-${{ github.run_id }}-${{ github.run_attempt }}", ios)
        self.assertLess(ios.index("Build XCFramework + Swift bindings"), ios.index("Reclaim completed iOS build intermediates"))
        self.assertLess(ios.index("Reclaim completed iOS build intermediates"), ios.index("xcodebuild test (simulator round-trip)"))
        self.assertIn("x86_64-apple-ios", ios)

    @unittest.skipUnless(sys.platform == "darwin" and shutil.which("xcrun") and shutil.which("xcodebuild"),
                         "requires the macOS Xcode toolchain")
    def test_real_xcframework_survives_reclaim(self):
        source = self.runner_temp / "fixture.c"
        source.write_text("int fixture(void) { return 1; }\n")
        headers = self.runner_temp / "headers"
        headers.mkdir()
        (headers / "module.modulemap").write_text('module Fixture { header "fixture.h" }\n')
        (headers / "fixture.h").write_text("int fixture(void);\n")
        archives = []
        for name, triple in (
            ("device", "arm64-apple-ios26.0"),
            ("sim-arm", "arm64-apple-ios26.0-simulator"),
            ("sim-x86", "x86_64-apple-ios26.0-simulator"),
        ):
            directory = self.runner_temp / name
            directory.mkdir()
            obj = directory / "fixture.o"
            archive = directory / "libios_framework.a"
            subprocess.run(["xcrun", "clang", "-target", triple, "-c", str(source), "-o", str(obj)],
                           check=True, capture_output=True)
            subprocess.run(["xcrun", "libtool", "-static", "-o", str(archive), str(obj)],
                           check=True, capture_output=True)
            archives.append(archive)
        simulator = self.runner_temp / "simulator/libios_framework.a"
        simulator.parent.mkdir()
        subprocess.run(["xcrun", "lipo", "-create", str(archives[1]), str(archives[2]), "-output", str(simulator)],
                       check=True, capture_output=True)
        shutil.rmtree(self.framework)
        subprocess.run([
            "xcodebuild", "-create-xcframework", "-library", str(archives[0]), "-headers", str(headers),
            "-library", str(simulator), "-headers", str(headers), "-output", str(self.framework),
        ], check=True, capture_output=True)
        with contextlib.redirect_stdout(io.StringIO()):
            cleanup.reclaim(self.repo, self.runner_temp, "123", "2")
        self.assertFalse(self.target.exists())
        self.assertFalse(self.scratch.exists())
        self.assertEqual(len(cleanup.validate_framework(self.framework)), 2)
        self.assertEqual(self.evidence.read_text(), "evidence")


if __name__ == "__main__":
    unittest.main()
